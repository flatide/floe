//! A loaded deck is a composite description, not a fictitious VFS directory.
//! The spec is staged later inside the worker's private lifetime workspace.
use super::{
    color::Mode,
    geom::{self, Skipped},
    plan::{Analysis, AnalysisOptions},
    spec::{self, CompositeSpec},
    view::{LayerMetadata, ViewRows},
};
use crate::{
    cache,
    catalog::{Grid, SourceInfo},
    check_cancelled,
    styles::{self, LayerProps},
    Error, ErrorKind, Result,
};
use floe_worker_client::{Fill, Layers, Style};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

#[derive(Debug, Serialize)]
pub struct DeckMetadata {
    pub dbu: f64,
    pub bbox: [i64; 4],
    pub layers: Vec<LayerMetadata>,
    pub src: SourceInfo,
    pub grid: Grid,
    pub vfs: bool,
    pub top_cell: String,
    pub jobdeck: DeckInfo,
}
#[derive(Debug, Serialize)]
pub struct DeckInfo {
    pub mode: Mode,
    pub chips: usize,
    pub identifiers: BTreeSet<i64>,
    pub levels: Option<BTreeSet<i64>>,
    pub sources: usize,
    pub placements: usize,
    pub skipped: Vec<Skipped>,
    pub colour_order: Vec<super::color::OrderRow>,
}
#[derive(Debug)]
pub struct DeckSnapshot {
    pub source: PathBuf,
    pub analysis: Analysis,
    pub rows: ViewRows,
    pub metadata: DeckMetadata,
    pub spec: CompositeSpec,
    pub layer_props: Vec<LayerProps>,
}
impl DeckSnapshot {
    pub fn open(
        source: &Path,
        mode: Mode,
        levels: Option<BTreeSet<i64>>,
        cancelled: &AtomicUsize,
    ) -> Result<Self> {
        let source = cache::absolute(source)?;
        let before = cache::fingerprint(&source)?;
        let levels = levels.filter(|s| !s.is_empty());
        let analysis = Analysis::open(
            &source,
            &AnalysisOptions {
                mode,
                load_ids: levels.clone(),
                skip_missing: true,
                ..Default::default()
            },
            cancelled,
        )?;
        let spec = spec::compose(&analysis, cancelled)?;
        spec.require_drawable()?;
        let rows = analysis.view_rows(cancelled)?;
        for row in &rows.rows {
            row.native_pair()?;
        }
        let dbu = analysis.model.stats.dbu;
        let bounds = analysis
            .model
            .stats
            .bbox_um
            .ok_or_else(|| Error::input("jobdeck places nothing"))?;
        let mut bbox = [0; 4];
        for (out, um) in bbox.iter_mut().zip(bounds) {
            *out = geom::to_grid(um, dbu)?.0;
        }
        let w = bbox[2]
            .checked_sub(bbox[0])
            .ok_or_else(|| Error::input("jobdeck bbox span overflow"))?
            .max(1);
        let h = bbox[3]
            .checked_sub(bbox[1])
            .ok_or_else(|| Error::input("jobdeck bbox span overflow"))?
            .max(1);
        let props_source = props_source(&source, mode)?;
        let layer_props = styles::load_props(&props_source)?;
        let mut layers = rows.metadata(&analysis.model.placements)?;
        let colors: BTreeMap<_, _> = layer_props
            .iter()
            .filter_map(|p| p.color.map(|c| (p.layer, c)))
            .collect();
        for layer in &mut layers {
            let key = (layer.layer as u32, layer.datatype as u32);
            if let Some(&color) = colors.get(&key) {
                layer.color = styles::color_text(color);
            }
        }
        if mode == Mode::Level {
            let heads: BTreeMap<_, _> = layers
                .iter()
                .filter(|r| r.jobdeck_head)
                .map(|r| (r.layer, r.color.clone()))
                .collect();
            for layer in &mut layers {
                layer.color = heads[&layer.layer].clone();
            }
        }
        let skipped = analysis
            .model
            .stats
            .skipped
            .iter()
            .chain(&spec.skipped)
            .cloned()
            .collect();
        let top_cell = if analysis.deck.name().is_empty() {
            source
                .file_name()
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .into()
        } else {
            analysis.deck.name().into()
        };
        let metadata = DeckMetadata {
            dbu,
            bbox,
            layers,
            src: SourceInfo {
                path: source.to_str().expect("UTF-8 path").into(),
                size: before.0,
                mtime: before.1,
            },
            grid: Grid {
                nx: 1,
                ny: 1,
                x0: bbox[0],
                y0: bbox[1],
                tile_w: w,
                tile_h: h,
            },
            vfs: true,
            top_cell,
            jobdeck: DeckInfo {
                mode,
                chips: analysis.deck.chips.len(),
                identifiers: analysis.deck.levels(),
                sources: analysis.deck.sources(levels.as_ref()).len(),
                levels,
                placements: analysis.model.placements.len(),
                skipped,
                colour_order: analysis.colors.order.clone(),
            },
        };
        check_cancelled(cancelled)?;
        if cache::fingerprint(&source)? != before {
            return Err(Error::new(
                ErrorKind::Cache,
                "jobdeck changed while loading; reopen it",
            ));
        }
        Ok(Self {
            source,
            analysis,
            rows,
            metadata,
            spec,
            layer_props,
        })
    }
    pub fn bbox_um(&self) -> [f64; 4] {
        self.metadata.bbox.map(|v| v as f64 * self.metadata.dbu)
    }
    pub fn resolve_layers(&self, spec: Option<&str>) -> Result<Layers> {
        self.rows.resolve_layers(&self.analysis.deck, spec)
    }
    pub fn skipped(&self) -> &[Skipped] {
        &self.metadata.jobdeck.skipped
    }
    pub fn styles(&self, archival: bool) -> Result<Vec<Style>> {
        let heads: BTreeSet<_> = self
            .metadata
            .layers
            .iter()
            .filter(|r| r.jobdeck_head)
            .map(|r| r.layer as u32)
            .collect();
        let mut fills = BTreeMap::new();
        let mut widths = BTreeMap::new();
        // The adapter only records known pattern slots and widths >1. Fill
        // and width inherit independently; a leaf color/width=1 record must
        // not mask a level head's stored width or valid pattern.
        for p in &self.layer_props {
            if let Some(fill) = styles::pattern(&p.fill) {
                fills.insert(p.layer, fill);
            }
            if p.width > 1 {
                widths.insert(p.layer, p.width);
            }
        }
        let mut out = Vec::new();
        for row in &self.metadata.layers {
            let key = (row.layer as u32, row.datatype as u32);
            let head = heads.contains(&key.0).then_some((key.0, 0));
            let color = styles::color(&row.color)
                .ok_or_else(|| Error::input("invalid deck layer color"))?;
            let fill = if archival {
                Fill::Solid
            } else {
                fills
                    .get(&key)
                    .or_else(|| head.and_then(|h| fills.get(&h)))
                    .cloned()
                    .unwrap_or(Fill::Speckle)
            };
            let width = if archival {
                1
            } else {
                widths
                    .get(&key)
                    .or_else(|| head.and_then(|h| widths.get(&h)))
                    .copied()
                    .unwrap_or(1)
            };
            out.push(Style {
                layer: key,
                color,
                fill,
                width,
            });
        }
        out.sort_by_key(|s| s.layer);
        Ok(out)
    }
}
/// Mode-specific property namespaces deliberately bypass obsolete CHIP-index
/// keys. Relative TC normalization is unrelated to this file path policy.
pub fn props_source(source: &Path, mode: Mode) -> Result<PathBuf> {
    if mode == Mode::Level {
        return Ok(source.to_owned());
    }
    let name = source
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::input("deck path has no UTF-8 filename"))?;
    let split = name
        .rfind('.')
        .filter(|&i| name[..i].chars().any(|c| c != '.'))
        .unwrap_or(name.len());
    let (stem, ext) = name.split_at(split);
    let suffix = if mode == Mode::Chip {
        "chip-by-level"
    } else {
        "layer"
    };
    Ok(source.with_file_name(format!("{stem}.{suffix}{ext}")))
}
