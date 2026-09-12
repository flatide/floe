//! Shared read-only dataset boundary for CLI and future session controllers.
//! Layout cache paths and deck composite specs are distinct variants.
use crate::{
    artifact,
    catalog::{self, Grid, Layout},
    jobdeck::{color::Mode, dataset::DeckSnapshot, geom::Skipped, index::is_deck},
    styles, Error, Result,
};
use floe_worker_client::{Layers, Style};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

#[derive(Debug)]
pub enum Dataset {
    Layout(Box<Layout>),
    Deck(Box<DeckSnapshot>),
}
impl Dataset {
    pub fn open(
        source: &Path,
        levels: Option<BTreeSet<i64>>,
        mode: Mode,
        cancelled: &AtomicUsize,
    ) -> Result<Self> {
        if is_deck(source) {
            Ok(Self::Deck(Box::new(DeckSnapshot::open(
                source, mode, levels, cancelled,
            )?)))
        } else {
            if levels.is_some() || mode != Mode::Level {
                return Err(Error::input("level/deck mode selection requires a jobdeck"));
            }
            Ok(Self::Layout(Box::new(Layout::open(source, cancelled)?)))
        }
    }
    pub fn is_deck(&self) -> bool {
        matches!(self, Self::Deck(_))
    }
    pub fn source(&self) -> &Path {
        match self {
            Self::Layout(l) => &l.source,
            Self::Deck(d) => &d.source,
        }
    }
    pub fn dbu(&self) -> f64 {
        match self {
            Self::Layout(l) => l.metadata.dbu,
            Self::Deck(d) => d.metadata.dbu,
        }
    }
    pub fn bbox(&self) -> [i64; 4] {
        match self {
            Self::Layout(l) => l.metadata.bbox,
            Self::Deck(d) => d.metadata.bbox,
        }
    }
    pub fn bbox_um(&self) -> [f64; 4] {
        self.bbox().map(|v| v as f64 * self.dbu())
    }
    pub fn grid(&self) -> &Grid {
        match self {
            Self::Layout(l) => &l.metadata.grid,
            Self::Deck(d) => &d.metadata.grid,
        }
    }
    pub fn source_stale(&self) -> bool {
        matches!(self,Self::Layout(l) if l.source_stale)
    }
    pub fn skipped(&self) -> &[Skipped] {
        match self {
            Self::Deck(d) => d.skipped(),
            _ => &[],
        }
    }
    pub fn resolve_layers(&self, spec: Option<&str>) -> Result<Layers> {
        match self {
            Self::Layout(l) => l.resolve_layers(spec),
            Self::Deck(d) => d.resolve_layers(spec),
        }
    }
    pub fn styles(&self, archival: bool) -> Result<Vec<Style>> {
        match self {
            Self::Layout(l) => styles::layout_styles(l, archival),
            Self::Deck(d) => d.styles(archival),
        }
    }
    pub fn output_path(&self, path: &Path) -> Result<PathBuf> {
        match self {
            Self::Layout(l) => artifact::output_path(path, l),
            Self::Deck(d) => {
                let props =
                    crate::jobdeck::dataset::props_source(&d.source, d.metadata.jobdeck.mode)?;
                let mut full = props.as_os_str().to_owned();
                full.push(".layerprops");
                d.analysis
                    .output_path(path, &[full.into(), props.with_extension("layerprops")])
            }
        }
    }
    pub fn info(&self) -> Value {
        match self {
            Self::Layout(l) => {
                json!({"source":l.source,"cache":l.directory,"source_stale":l.source_stale,"metadata":l.metadata})
            }
            // No private worker filename is exposed as a supposed cache. A deck
            // snapshot can be inspected without creating a spec file or daemon.
            Self::Deck(d) => {
                json!({"source":d.source,"cache":null,"source_stale":false,"metadata":d.metadata})
            }
        }
    }
    pub fn summary(&self, cancelled: &AtomicUsize) -> Result<String> {
        let Self::Deck(d) = self else {
            let Self::Layout(l) = self else {
                unreachable!()
            };
            return l.summary(cancelled);
        };
        let mut text = String::new();
        for r in self.skipped() {
            writeln!(
                text,
                "[jobdeck] skipped   : CHIP {} ${} {}: {} ({})",
                r.chip, r.idx, r.tc, r.reason, r.detail
            )
            .unwrap();
        }
        for line in d.analysis.summary()? {
            writeln!(text, "[jobdeck] {line}").unwrap();
        }
        for r in &d.spec.skipped {
            writeln!(
                text,
                "[jobdeck] skipped   : CHIP {} ${} {}: {} ({})",
                r.chip, r.idx, r.tc, r.reason, r.detail
            )
            .unwrap();
        }
        if !d.skipped().is_empty() {
            writeln!(
                text,
                "[jobdeck] INCOMPLETE: {} placement(s) will not be drawn",
                d.skipped().len()
            )
            .unwrap();
        }
        let bb = self.bbox_um();
        writeln!(
            text,
            "bbox       : ({:.1}, {:.1}) - ({:.1}, {:.1}) um",
            bb[0], bb[1], bb[2], bb[3]
        )
        .unwrap();
        writeln!(text, "{:>8}  {:<20} {:>11}", "layer", "name", "placements").unwrap();
        for row in &d.metadata.layers {
            writeln!(
                text,
                "{:>5}/{:<2} {:<20} {:>11}",
                row.layer,
                row.datatype,
                row.name,
                catalog::comma(row.stored_shapes)
            )
            .unwrap();
        }
        Ok(text)
    }
}
