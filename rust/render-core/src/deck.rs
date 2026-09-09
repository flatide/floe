//! Jobdeck composite (docs/JOBDECK.ko.md, M2): several caches drawn as
//! one frame.
//!
//! Every placement is an ordinary single-cache render whose viewport is
//! the deck viewport mapped into that source's own database units,
//! `(v - d) / scale`, painted onto an alpha-0 background and then laid
//! over the composite opaque-over, in deck layer order. Magnification
//! therefore lives only here, at the scene root: caches, hierarchy
//! plans and the raster stay integer and untouched, and the pixels
//! equal a single-cache render of the flattened composite (the gate's
//! KLayout oracle) because the world-to-device mapping of a scaled
//! coordinate is the same expression either way.
//!
//! The decoded-page budget is one number for the whole deck (the shared
//! servers pin it): every source keeps its own LRU, the source about to
//! decode is guaranteed at least half of the budget and the sum of all
//! residents never exceeds it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use floe_ovm::BBox;

use crate::{
    render_geometry_styled_cancellable, Cache, CacheLayer, DecodedPageCache, FrameScene,
    GeometryRasterRequest, LayerFill, LayerStyle, PlanRequest, RasterViewBox,
    RenderCancellation, RenderStats, RgbaFrame, StyledGeometryRasterRequest, ViewBox,
};

/// One `placement` line of a deck spec: source `source` drawn with its
/// layer `layer/datatype` as deck output layer `out`, scaled by `scale`
/// (deck dbu per source dbu) and offset by `dx`/`dy` (deck dbu).
#[derive(Clone, Debug, PartialEq)]
pub struct DeckPlacement {
    pub index: u32,
    pub source: usize,
    pub layer: u32,
    pub datatype: u32,
    pub out: u32,
    pub scale: f64,
    pub dx: f64,
    pub dy: f64,
    /// Paint order; ties break on `index`.
    pub order: u32,
}

/// One deck output layer: the visibility/style key of its placements.
/// `layer/datatype` is the pair the viewer keys it by (level view: the
/// level number/0; chip view: CHIP position/level, so a CHIP expands
/// into its levels in the layer panel); it defaults to `out/0`.
#[derive(Clone, Debug, PartialEq)]
pub struct DeckLayer {
    pub out: u32,
    pub layer: u32,
    pub datatype: u32,
    pub name: String,
    pub color: [u8; 4],
    pub fill: LayerFill,
    pub outline_width: u8,
}

/// The parsed deck spec (written by `floe.jobdeck.render`).
///
/// ```text
/// deck unit=2.5e-05
/// source path_hex=<hex utf-8 path of a .floe cache>
/// layer out=0 key=1/0 name_hex=<hex> color=#0000ff fill=solid width=1
/// placement source=0 layer=123/43 out=0 scale=8 dx=1640800000 dy=3200800000 order=0
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct DeckSpec {
    /// Deck database unit in micrometres.
    pub unit: f64,
    pub sources: Vec<String>,
    pub layers: Vec<DeckLayer>,
    pub placements: Vec<DeckPlacement>,
}

impl DeckSpec {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut unit = None;
        let mut sources = Vec::new();
        let mut layers: Vec<DeckLayer> = Vec::new();
        let mut placements: Vec<DeckPlacement> = Vec::new();
        for (line_index, raw) in text.lines().enumerate() {
            let line_no = line_index + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut tokens = line.split_whitespace();
            let kind = tokens.next().unwrap_or("");
            let fields = parse_fields(tokens).map_err(|error| format!("line {line_no}: {error}"))?;
            match kind {
                "deck" => {
                    reject_unknown(&fields, &["unit"], line_no)?;
                    let value: f64 = required_parse(&fields, "unit", line_no)?;
                    if !value.is_finite() || value <= 0.0 {
                        return Err(format!("line {line_no}: deck unit must be positive"));
                    }
                    if unit.replace(value).is_some() {
                        return Err(format!("line {line_no}: duplicate deck line"));
                    }
                }
                "source" => {
                    reject_unknown(&fields, &["path_hex"], line_no)?;
                    let path = unhex(required(&fields, "path_hex", line_no)?, "path_hex")
                        .map_err(|error| format!("line {line_no}: {error}"))?;
                    if path.is_empty() {
                        return Err(format!("line {line_no}: empty source path"));
                    }
                    sources.push(path);
                }
                "layer" => {
                    reject_unknown(
                        &fields,
                        &["out", "key", "name_hex", "color", "fill", "width"],
                        line_no,
                    )?;
                    let out: u32 = required_parse(&fields, "out", line_no)?;
                    if layers.iter().any(|layer| layer.out == out) {
                        return Err(format!("line {line_no}: duplicate deck layer {out}"));
                    }
                    let (layer, datatype) = match fields.get("key") {
                        Some(value) => parse_layer_pair(value)
                            .map_err(|error| format!("line {line_no}: {error}"))?,
                        None => (out, 0),
                    };
                    if layers
                        .iter()
                        .any(|l| l.layer == layer && l.datatype == datatype)
                    {
                        return Err(format!(
                            "line {line_no}: duplicate deck layer key {layer}/{datatype}"
                        ));
                    }
                    let name = match fields.get("name_hex") {
                        Some(value) => unhex(value, "name_hex")
                            .map_err(|error| format!("line {line_no}: {error}"))?,
                        None => format!("out{out}"),
                    };
                    let width: u8 = match fields.get("width") {
                        Some(value) => parse_value(value, "width")
                            .map_err(|error| format!("line {line_no}: {error}"))?,
                        None => 1,
                    };
                    if !(1..=8).contains(&width) {
                        return Err(format!("line {line_no}: width must be in 1..=8"));
                    }
                    layers.push(DeckLayer {
                        out,
                        layer,
                        datatype,
                        name,
                        color: parse_color(fields.get("color").map(String::as_str).unwrap_or("#ffffff"))
                            .map_err(|error| format!("line {line_no}: {error}"))?,
                        fill: parse_fill(fields.get("fill").map(String::as_str).unwrap_or("solid"))
                            .map_err(|error| format!("line {line_no}: {error}"))?,
                        outline_width: width,
                    });
                }
                "placement" => {
                    reject_unknown(
                        &fields,
                        &["source", "layer", "out", "scale", "dx", "dy", "order"],
                        line_no,
                    )?;
                    let source: usize = required_parse(&fields, "source", line_no)?;
                    let (layer, datatype) = parse_layer_pair(required(&fields, "layer", line_no)?)
                        .map_err(|error| format!("line {line_no}: {error}"))?;
                    let scale: f64 = required_parse(&fields, "scale", line_no)?;
                    if !scale.is_finite() || scale <= 0.0 {
                        return Err(format!("line {line_no}: scale must be positive"));
                    }
                    let dx: f64 = required_parse(&fields, "dx", line_no)?;
                    let dy: f64 = required_parse(&fields, "dy", line_no)?;
                    if !dx.is_finite() || !dy.is_finite() {
                        return Err(format!("line {line_no}: dx/dy must be finite"));
                    }
                    let index = u32::try_from(placements.len())
                        .map_err(|_| format!("line {line_no}: too many placements"))?;
                    placements.push(DeckPlacement {
                        index,
                        source,
                        layer,
                        datatype,
                        out: required_parse(&fields, "out", line_no)?,
                        scale,
                        dx,
                        dy,
                        order: match fields.get("order") {
                            Some(value) => parse_value(value, "order")
                                .map_err(|error| format!("line {line_no}: {error}"))?,
                            None => index,
                        },
                    });
                }
                other => return Err(format!("line {line_no}: unknown deck line kind {other:?}")),
            }
        }
        let unit = unit.ok_or_else(|| "deck spec has no deck line".to_string())?;
        if sources.is_empty() {
            return Err("deck spec names no sources".to_string());
        }
        if placements.is_empty() {
            return Err("deck spec has no placements".to_string());
        }
        for placement in &placements {
            if placement.source >= sources.len() {
                return Err(format!(
                    "placement {} names source {} but the spec has {} sources",
                    placement.index,
                    placement.source,
                    sources.len()
                ));
            }
            if !layers.iter().any(|layer| layer.out == placement.out) {
                return Err(format!(
                    "placement {} uses deck layer {} which has no layer line",
                    placement.index, placement.out
                ));
            }
        }
        Ok(Self {
            unit,
            sources,
            layers,
            placements,
        })
    }

    /// Placements in paint order: `order`, then spec order.
    pub fn ordered_placements(&self) -> Vec<&DeckPlacement> {
        let mut ordered: Vec<&DeckPlacement> = self.placements.iter().collect();
        ordered.sort_by_key(|placement| (placement.order, placement.index));
        ordered
    }
}

struct DeckSource {
    cache: Cache,
    pages: DecodedPageCache,
    layers: Vec<CacheLayer>,
    top_bbox: BBox,
    max_depth: u32,
}

struct Placed {
    spec: DeckPlacement,
    layer_idx: u32,
    /// The source's top-cell bounds in deck dbu, for culling.
    bbox: [f64; 4],
}

/// Pages decoded per budget check inside one pass, cut by their
/// encoded size (budget / DECODE_CHUNK_DIV each, one page at least, at
/// most DECODE_CHUNK_PAGES): a chunk may overshoot the budget by at
/// most its own decoded size.
const DECODE_CHUNK_PAGES: usize = 64;
const DECODE_CHUNK_DIV: u64 = 32;

/// Split the prioritized page list into decode chunks by encoded size.
fn decode_chunks(cache: &Cache, selected: &[u32], budget_bytes: u64) -> Vec<Vec<u32>> {
    let limit = (budget_bytes / DECODE_CHUNK_DIV).max(1);
    let mut chunks: Vec<Vec<u32>> = Vec::new();
    let mut chunk: Vec<u32> = Vec::new();
    let mut bytes = 0u64;
    for &page_id in selected {
        chunk.push(page_id);
        bytes = bytes.saturating_add(cache.page_encoded_bytes(page_id));
        if bytes >= limit || chunk.len() >= DECODE_CHUNK_PAGES {
            chunks.push(std::mem::take(&mut chunk));
            bytes = 0;
        }
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    chunks
}

/// An opened deck: caches, per-source decoded-page LRUs under one
/// budget, and the resolved placements in paint order.
pub struct Deck {
    unit: f64,
    sources: Vec<DeckSource>,
    source_paths: Vec<String>,
    layers: BTreeMap<u32, DeckLayer>,
    placements: Vec<Placed>,
    budget_bytes: u64,
}

/// Summary of an opened deck for the daemon's `opened`/`info` lines.
#[derive(Clone, Debug, PartialEq)]
pub struct DeckInfo {
    pub unit: f64,
    pub sources: usize,
    pub layers: usize,
    pub placements: usize,
    pub max_depth: u32,
    /// Union of every placement's transformed top-cell bounds, deck dbu.
    pub bbox: Option<[f64; 4]>,
}

/// A composite render request, viewport in deck dbu.
#[derive(Clone, Debug, PartialEq)]
pub struct DeckRenderRequest {
    pub view: RasterViewBox,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub cut_px: f64,
    pub exact: bool,
    /// Visible deck layers (`out`); None = all.
    pub visible: Option<BTreeSet<u32>>,
    /// Hierarchy frames of each source beyond `depth` (review 2026-09-09
    /// P1-3: a deck at depth 0 without them drew nothing for a source
    /// whose shapes live in child cells).
    pub frames: bool,
    pub mono: bool,
    pub workers: u16,
    pub decode_workers: u16,
    pub tile_size: u16,
    pub decode_pages: Option<usize>,
}

pub struct DeckRenderReport {
    pub frame: RgbaFrame,
    pub stats: RenderStats,
    /// Placements rastered / skipped as invisible or outside the view.
    pub passes: u32,
    pub passes_skipped: u32,
    pub pages: u32,
    pub plan_pages: u32,
    pub partial: bool,
    /// Pages the passes left undecoded because the budget was reached
    /// (field 2026-09-09: a mid-zoom pass wanted 2 GB against 1 GiB).
    pub deferred: u32,
    pub resident_bytes: u64,
    pub rectangle_member_paints: u64,
    pub polygon_member_paints: u64,
    pub path_member_paints: u64,
    pub frame_member_paints: u64,
    /// Largest one-pass decoded working set (bytes) - the deck's
    /// counterpart of the single-cache generation charge.
    pub pass_bytes_max: u64,
    /// Frames-only passes actually rastered (a plan without hierarchy
    /// frames skips its pass; analysis 2026-09-09).
    pub frame_passes: u32,
    /// Distinct pages decoded this frame vs the per-pass sum `pages`.
    pub unique_pages: u32,
    /// Time in scene assembly, frame-pass rasters and the overlay /
    /// final layering - the phases the frame line used to hide.
    pub scene_us: u64,
    pub frame_raster_us: u64,
    pub composite_us: u64,
}

impl Deck {
    pub fn open(spec: DeckSpec, budget_bytes: u64) -> Result<Self, String> {
        let mut sources = Vec::with_capacity(spec.sources.len());
        for path in &spec.sources {
            let cache = Cache::open(path).map_err(|error| format!("deck source {path}: {error}"))?;
            let info = cache.info();
            let top_bbox = cache.cell_bbox(info.top_cell)?;
            sources.push(DeckSource {
                layers: cache.layers(),
                pages: DecodedPageCache::new(budget_bytes),
                cache,
                top_bbox,
                max_depth: info.max_depth,
            });
        }
        let mut placements = Vec::with_capacity(spec.placements.len());
        for placement in spec.ordered_placements() {
            let source = &sources[placement.source];
            let layer_idx = source
                .layers
                .iter()
                .find(|layer| layer.layer == placement.layer && layer.datatype == placement.datatype)
                .map(|layer| layer.index)
                .ok_or_else(|| {
                    format!(
                        "placement {}: source {} has no layer {}/{}",
                        placement.index,
                        spec.sources[placement.source],
                        placement.layer,
                        placement.datatype
                    )
                })?;
            placements.push(Placed {
                bbox: transform_bbox(&source.top_bbox, placement),
                spec: placement.clone(),
                layer_idx,
            });
        }
        Ok(Self {
            unit: spec.unit,
            sources,
            source_paths: spec.sources,
            layers: spec.layers.into_iter().map(|layer| (layer.out, layer)).collect(),
            placements,
            budget_bytes,
        })
    }

    pub fn unit(&self) -> f64 {
        self.unit
    }

    pub fn source_paths(&self) -> &[String] {
        &self.source_paths
    }

    pub fn layers(&self) -> Vec<DeckLayer> {
        self.layers.values().cloned().collect()
    }

    /// Synthetic cache-layer view of the deck layers so the daemon's
    /// style file parser and `layers=` lists (`L/D`, name, `idx:out`)
    /// apply unchanged; the index is the `out`, the pair is the view's.
    pub fn style_layers(&self) -> Vec<CacheLayer> {
        self.layers
            .values()
            .map(|layer| CacheLayer {
                index: layer.out,
                layer: layer.layer,
                datatype: layer.datatype,
                name: layer.name.clone(),
            })
            .collect()
    }

    /// Replaces colour/fill/outline of the named deck layers; a layer
    /// absent from `styles` keeps what the spec gave it.
    pub fn set_styles(&mut self, styles: &[LayerStyle]) -> Result<(), String> {
        for style in styles {
            let layer = self
                .layers
                .get_mut(&style.layer_idx)
                .ok_or_else(|| format!("deck layer {} does not exist", style.layer_idx))?;
            layer.color = style.color;
            layer.fill = style.fill;
            layer.outline_width = style.outline_width;
        }
        Ok(())
    }

    pub fn info(&self) -> DeckInfo {
        let mut bbox: Option<[f64; 4]> = None;
        for placed in &self.placements {
            bbox = Some(match bbox {
                None => placed.bbox,
                Some(b) => [
                    b[0].min(placed.bbox[0]),
                    b[1].min(placed.bbox[1]),
                    b[2].max(placed.bbox[2]),
                    b[3].max(placed.bbox[3]),
                ],
            });
        }
        DeckInfo {
            unit: self.unit,
            sources: self.sources.len(),
            layers: self.layers.len(),
            placements: self.placements.len(),
            max_depth: self.sources.iter().map(|source| source.max_depth).max().unwrap_or(0),
            bbox,
        }
    }

    pub fn resident_bytes(&self) -> u64 {
        self.sources.iter().map(|source| source.pages.resident_bytes()).sum()
    }

    pub fn budget_bytes(&self) -> u64 {
        self.budget_bytes
    }

    /// Give source `index` room to decode: the other LRUs shrink
    /// (proportionally, least first by their own LRU order) until they
    /// hold at most half the budget, and `index` gets the rest.
    fn share_budget(&mut self, index: usize) {
        let budget = self.budget_bytes;
        let half = budget / 2;
        let others: u64 = self
            .sources
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, source)| source.pages.resident_bytes())
            .sum();
        if others > half && others > 0 {
            for (i, source) in self.sources.iter_mut().enumerate() {
                if i == index {
                    continue;
                }
                let resident = source.pages.resident_bytes();
                let target = ((resident as u128 * half as u128) / others as u128) as u64;
                source.pages.set_budget_bytes(target);
                source.pages.set_budget_bytes(budget);
            }
        }
        let others: u64 = self
            .sources
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, source)| source.pages.resident_bytes())
            .sum();
        let own = budget.saturating_sub(others);
        self.sources[index].pages.set_budget_bytes(own);
    }

    pub fn render(
        &mut self,
        request: &DeckRenderRequest,
        generation: u64,
        cancellation: &RenderCancellation,
    ) -> Result<DeckRenderReport, String> {
        request.view.validate()?;
        if request.view.x0 == request.view.x1 || request.view.y0 == request.view.y1 {
            return Err("deck view must have positive width and height".to_string());
        }
        if request.width == 0 || request.height == 0 {
            return Err("image width and height must be positive".to_string());
        }
        let byte_len = (request.width as usize)
            .checked_mul(request.height as usize)
            .and_then(|value| value.checked_mul(4))
            .ok_or_else(|| "image byte length overflow".to_string())?;
        // The geometry composite keeps alpha 0 wherever no pass painted
        // (review 2026-09-09 (3rd) P2-1: turning black pixels
        // transparent at the end treated black DESIGN as background and
        // let the gray frame wash show through it); the opaque black
        // background goes underneath only at the very end.
        let mut composite = vec![0u8; byte_len];
        // Review 2026-09-09 (2nd) P1-1: the single-cache raster paints
        // the gray frame bands under the design and the white band over
        // it, for the WHOLE scene. A deck must keep that order across
        // placements - every gray band first, then every placement's
        // geometry, then every white band - or a later placement's
        // geometry buries an earlier placement's white frame. With
        // frames on, each placement is rastered twice on its scene
        // (frames only, geometry only) and the frame pass is split by
        // its structural colours into an under plane and an over plane
        // that are laid down in three phases at the end.
        let mut frames_under = request.frames.then(|| vec![0u8; byte_len]);
        let mut frames_over = request.frames.then(|| vec![0u8; byte_len]);
        let mut stats = RenderStats::default();
        let mut passes = 0u32;
        let mut passes_skipped = 0u32;
        let mut pages = 0u32;
        let mut plan_pages = 0u32;
        let mut partial = false;
        let mut deferred = 0u32;
        let mut rectangle_member_paints = 0u64;
        let mut polygon_member_paints = 0u64;
        let mut path_member_paints = 0u64;
        let mut frame_member_paints = 0u64;
        let mut pass_bytes_max = 0u64;
        let mut frame_passes = 0u32;
        let mut unique_pages: BTreeSet<(usize, u32)> = BTreeSet::new();
        let mut scene_us = 0u64;
        let mut frame_raster_us = 0u64;
        let mut composite_us = 0u64;
        let view = [request.view.x0, request.view.y0, request.view.x1, request.view.y1];
        for placed_index in 0..self.placements.len() {
            let (out, source_index) = {
                let placed = &self.placements[placed_index];
                (placed.spec.out, placed.spec.source)
            };
            if request.visible.as_ref().is_some_and(|visible| !visible.contains(&out)) {
                passes_skipped += 1;
                continue;
            }
            if !boxes_intersect(&self.placements[placed_index].bbox, &view) {
                passes_skipped += 1;
                continue;
            }
            check_generation(cancellation, generation)?;
            let style = {
                let layer = self
                    .layers
                    .get(&out)
                    .ok_or_else(|| format!("deck layer {out} vanished"))?;
                LayerStyle {
                    layer_idx: self.placements[placed_index].layer_idx,
                    color: layer.color,
                    fill: layer.fill,
                    outline_width: layer.outline_width,
                }
            };
            let source_view = source_view(&request.view, &self.placements[placed_index].spec)?;
            let plan_request = source_plan_request(
                &source_view,
                request,
                &self.placements[placed_index].spec,
            )?;
            self.share_budget(source_index);
            let source = &mut self.sources[source_index];
            let planned = source.cache.plan(&plan_request)?;
            stats.plan_us = stats.plan_us.saturating_add(planned.stats.plan_us);
            plan_pages = plan_pages.saturating_add(planned.summary.pages);
            check_generation(cancellation, generation)?;
            if planned
                .plan
                .wcells
                .binary_search_by_key(&planned.plan.top, |cell| cell.key)
                .is_err()
            {
                // The planner dropped the whole source: its top cell is
                // below the cut at this scale (a mark placed at 0.2x on
                // a full-deck view). A single cache never sees this - its
                // top is the chip - so the scene would reject the plan;
                // for a deck it is an empty pass, like any sub-cut cell.
                passes_skipped += 1;
                continue;
            }
            let mut prioritized: Vec<(u64, u32)> = planned
                .plan
                .page_prio
                .iter()
                .copied()
                .zip(planned.plan.pages.iter().copied())
                .collect();
            prioritized.sort_unstable();
            let selected: Vec<u32> = prioritized
                .into_iter()
                .take(request.decode_pages.unwrap_or(usize::MAX))
                .map(|(_, page_id)| page_id)
                .collect();
            // Review 2026-09-09 P1-2: the scene's Arcs keep every page
            // of this pass alive whatever the LRU evicted, so one pass
            // is charged against the shared budget like a single-cache
            // generation. Field 2026-09-09: refusing the frame outright
            // ("2040099526 > 1073741824 bytes") left the viewer with an
            // error at a mid zoom; instead the pass decodes its pages
            // in priority order, chunk by chunk, and STOPS at the
            // budget - the frame is drawn from what fits and reports
            // the rest as deferred (partial), memory stays bounded.
            let mut decoded = Vec::with_capacity(selected.len());
            let mut pass_bytes = 0u64;
            let mut over = false;
            for chunk in decode_chunks(&source.cache, &selected, self.budget_bytes) {
                if over {
                    deferred = deferred.saturating_add(chunk.len() as u32);
                    continue;
                }
                let (chunk_pages, decode_stats) = source.pages.load_cancellable(
                    &source.cache,
                    &chunk,
                    request.decode_workers,
                    generation,
                    cancellation,
                )?;
                accumulate_decode(&mut stats, &decode_stats);
                for page in chunk_pages {
                    pass_bytes = pass_bytes
                        .checked_add(page.estimated_bytes())
                        .ok_or_else(|| "decoded generation byte charge overflow".to_string())?;
                    decoded.push(page);
                }
                if pass_bytes >= self.budget_bytes {
                    over = true;
                }
                check_generation(cancellation, generation)?;
            }
            pass_bytes_max = pass_bytes_max.max(pass_bytes);
            for page in &decoded {
                unique_pages.insert((source_index, page.page_id));
            }
            let scene_started = std::time::Instant::now();
            let scene = FrameScene::new_shared(&source.cache, Arc::new(planned.plan), decoded)?;
            scene_us = scene_us.saturating_add(scene_started.elapsed().as_micros() as u64);
            partial |= scene.is_partial();
            pages = pages.saturating_add(scene.available_pages().try_into().unwrap_or(u32::MAX));
            check_generation(cancellation, generation)?;
            let raster = GeometryRasterRequest {
                view: source_view,
                width: request.width,
                height: request.height,
                background: [0, 0, 0, 0],
                foreground: [255, 255, 255, 255],
                workers: request.workers,
                tile_size: request.tile_size,
            };
            // a frames-only pass only when this placement's plan holds
            // a hierarchy frame at all (analysis 2026-09-09: the pass
            // ran, and composited a full frame, for every placement)
            if let (Some(under), Some(over), true) = (
                frames_under.as_mut(),
                frames_over.as_mut(),
                scene.subtree_has_frames(scene.top()),
            ) {
                let frames_only = StyledGeometryRasterRequest {
                    raster,
                    layers: Vec::new(),
                    hierarchy_frames: true,
                    mono: request.mono,
                };
                let report =
                    render_geometry_styled_cancellable(&scene, &frames_only, generation, cancellation)?;
                frame_raster_us = frame_raster_us.saturating_add(report.stats.raster_us);
                let split_started = std::time::Instant::now();
                split_frame_planes(report.frame.pixels(), under, over);
                composite_us = composite_us.saturating_add(split_started.elapsed().as_micros() as u64);
                accumulate_raster(&mut stats, &report.stats);
                frame_member_paints = frame_member_paints.saturating_add(report.frame_member_paints);
                frame_passes += 1;
                check_generation(cancellation, generation)?;
            }
            let styled = StyledGeometryRasterRequest {
                raster,
                layers: vec![style],
                hierarchy_frames: false,
                mono: request.mono,
            };
            let report = render_geometry_styled_cancellable(&scene, &styled, generation, cancellation)?;
            let overlay_started = std::time::Instant::now();
            overlay(&mut composite, report.frame.pixels());
            composite_us = composite_us.saturating_add(overlay_started.elapsed().as_micros() as u64);
            accumulate_raster(&mut stats, &report.stats);
            rectangle_member_paints =
                rectangle_member_paints.saturating_add(report.rectangle_member_paints);
            polygon_member_paints = polygon_member_paints.saturating_add(report.polygon_member_paints);
            path_member_paints = path_member_paints.saturating_add(report.path_member_paints);
            passes += 1;
        }
        // opaque black ground, then (with frames) the gray bands of every
        // placement UNDER all geometry and the white band of every
        // placement OVER it
        let layering_started = std::time::Instant::now();
        let mut layered = vec![0u8; byte_len];
        for pixel in layered.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        if let Some(under) = frames_under.as_ref() {
            overlay(&mut layered, under);
        }
        overlay(&mut layered, &composite);
        if let Some(over) = frames_over.as_ref() {
            overlay(&mut layered, over);
        }
        composite_us = composite_us.saturating_add(layering_started.elapsed().as_micros() as u64);
        Ok(DeckRenderReport {
            frame: RgbaFrame::from_pixels(request.width, request.height, layered)?,
            stats,
            passes,
            passes_skipped,
            pages,
            plan_pages,
            partial,
            deferred,
            resident_bytes: self.resident_bytes(),
            rectangle_member_paints,
            polygon_member_paints,
            path_member_paints,
            frame_member_paints,
            pass_bytes_max,
            frame_passes,
            unique_pages: unique_pages.len().try_into().unwrap_or(u32::MAX),
            scene_us,
            frame_raster_us,
            composite_us,
        })
    }
}

fn check_generation(cancellation: &RenderCancellation, generation: u64) -> Result<(), String> {
    if cancellation.is_cancelled(generation) {
        return Err("render cancelled".to_string());
    }
    Ok(())
}

fn accumulate_decode(stats: &mut RenderStats, decode: &RenderStats) {
    stats.page_read_us = stats.page_read_us.saturating_add(decode.page_read_us);
    stats.page_decode_us = stats.page_decode_us.saturating_add(decode.page_decode_us);
    stats.page_decode_sum_us = stats.page_decode_sum_us.saturating_add(decode.page_decode_sum_us);
    stats.page_decode_max_us = stats.page_decode_max_us.max(decode.page_decode_max_us);
    stats.page_index_us = stats.page_index_us.saturating_add(decode.page_index_us);
    stats.decode_workers_used = stats.decode_workers_used.max(decode.decode_workers_used);
    stats.decoded_cache_hit = stats.decoded_cache_hit.saturating_add(decode.decoded_cache_hit);
    stats.decoded_cache_miss = stats.decoded_cache_miss.saturating_add(decode.decoded_cache_miss);
    stats.decoded_cache_evicted =
        stats.decoded_cache_evicted.saturating_add(decode.decoded_cache_evicted);
}

fn accumulate_raster(stats: &mut RenderStats, raster: &RenderStats) {
    stats.raster_us = stats.raster_us.saturating_add(raster.raster_us);
    stats.raster_tile_max_us = stats.raster_tile_max_us.max(raster.raster_tile_max_us);
    stats.tiles = stats.tiles.saturating_add(raster.tiles);
    stats.workers_used = stats.workers_used.max(raster.workers_used);
    stats.work_bin_items = stats.work_bin_items.saturating_add(raster.work_bin_items);
    stats.work_bin_overflow_items = stats
        .work_bin_overflow_items
        .max(raster.work_bin_overflow_items);
    stats.work_bin_defer_rep = stats.work_bin_defer_rep.saturating_add(raster.work_bin_defer_rep);
    stats.work_bin_defer_single = stats
        .work_bin_defer_single
        .saturating_add(raster.work_bin_defer_single);
    stats.work_bin_defer_weight_max = stats
        .work_bin_defer_weight_max
        .max(raster.work_bin_defer_weight_max);
    stats.primitives_tested = stats.primitives_tested.saturating_add(raster.primitives_tested);
    stats.primitives_drawn = stats.primitives_drawn.saturating_add(raster.primitives_drawn);
    stats.rep_members_tested = stats.rep_members_tested.saturating_add(raster.rep_members_tested);
    stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(raster.rep_members_drawn);
    stats.hier_cells_visited = stats.hier_cells_visited.saturating_add(raster.hier_cells_visited);
    stats.subtrees_pruned = stats.subtrees_pruned.saturating_add(raster.subtrees_pruned);
}

/// A frames-only pass paints nothing but the raster's structural
/// colours: gray bands (solid wash, dotted, hollow) that a single-cache
/// render lays UNDER the design and the white hollow band it lays OVER
/// it. Split the pass into those two planes so a deck can keep that
/// order across every placement.
pub fn split_frame_planes(pass: &[u8], under: &mut [u8], over: &mut [u8]) {
    for ((src, u), o) in pass
        .chunks_exact(4)
        .zip(under.chunks_exact_mut(4))
        .zip(over.chunks_exact_mut(4))
    {
        if src[3] == 0 {
            continue;
        }
        if src[0] == 255 && src[1] == 255 && src[2] == 255 {
            o.copy_from_slice(src);
        } else {
            u.copy_from_slice(src);
        }
    }
}

/// Opaque-over: every pass pixel the raster touched (alpha != 0)
/// replaces the composite pixel; untouched alpha-0 background shows
/// what was painted before.
pub fn overlay(composite: &mut [u8], pass: &[u8]) {
    for (dst, src) in composite.chunks_exact_mut(4).zip(pass.chunks_exact(4)) {
        if src[3] != 0 {
            dst.copy_from_slice(src);
        }
    }
}

/// The deck viewport in one placement's source units: `(v - d) / scale`.
pub fn source_view(view: &RasterViewBox, placement: &DeckPlacement) -> Result<RasterViewBox, String> {
    let map = |v: f64, d: f64| (v - d) / placement.scale;
    RasterViewBox::new(
        map(view.x0, placement.dx),
        map(view.y0, placement.dy),
        map(view.x1, placement.dx),
        map(view.y1, placement.dy),
    )
}

fn source_plan_request(
    source_view: &RasterViewBox,
    request: &DeckRenderRequest,
    placement: &DeckPlacement,
) -> Result<PlanRequest, String> {
    let span_x = source_view.x1 - source_view.x0;
    let span_y = source_view.y1 - source_view.y0;
    if !(span_x > 0.0) || !(span_y > 0.0) {
        return Err(format!(
            "placement {}: source view collapsed ({span_x} x {span_y})",
            placement.index
        ));
    }
    let px_per_dbu = (request.width as f64 / span_x).min(request.height as f64 / span_y);
    let cut_dbu = if request.exact || request.cut_px == 0.0 {
        0
    } else {
        checked_bound((request.cut_px / px_per_dbu).ceil(), "cut dbu")?
    };
    let plan = PlanRequest {
        view: ViewBox::new(
            checked_bound(source_view.x0.floor(), "source view x0")?,
            checked_bound(source_view.y0.floor(), "source view y0")?,
            checked_bound(source_view.x1.ceil(), "source view x1")?,
            checked_bound(source_view.y1.ceil(), "source view y1")?,
        )?,
        cut_dbu,
        visible_layers: Some(vec![format!("{}/{}", placement.layer, placement.datatype)]),
        depth: request.depth,
        px_per_dbu,
        exact: request.exact,
    };
    plan.validate()?;
    Ok(plan)
}

fn checked_bound(value: f64, name: &str) -> Result<i64, String> {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(format!("coordinate overflow: {name} = {value}"));
    }
    Ok(value as i64)
}

/// Source bounds (source dbu) in deck dbu: `scale * b + d`.
pub fn transform_bbox(bbox: &BBox, placement: &DeckPlacement) -> [f64; 4] {
    if bbox.is_empty() {
        return [f64::NAN, f64::NAN, f64::NAN, f64::NAN];
    }
    [
        placement.scale * bbox.x0 as f64 + placement.dx,
        placement.scale * bbox.y0 as f64 + placement.dy,
        placement.scale * bbox.x1 as f64 + placement.dx,
        placement.scale * bbox.y1 as f64 + placement.dy,
    ]
}

fn boxes_intersect(a: &[f64; 4], b: &[f64; 4]) -> bool {
    // a NaN box (empty source) never intersects
    a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1]
}

fn parse_fields<'a>(tokens: impl Iterator<Item = &'a str>) -> Result<BTreeMap<String, String>, String> {
    let mut fields = BTreeMap::new();
    for token in tokens {
        let (key, value) = token
            .split_once('=')
            .ok_or_else(|| format!("field without '=': {token}"))?;
        if key.is_empty() {
            return Err(format!("field with empty key: {token}"));
        }
        if fields.insert(key.to_string(), value.to_string()).is_some() {
            return Err(format!("duplicate field: {key}"));
        }
    }
    Ok(fields)
}

fn reject_unknown(fields: &BTreeMap<String, String>, allowed: &[&str], line_no: usize) -> Result<(), String> {
    for key in fields.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("line {line_no}: unknown field {key}"));
        }
    }
    Ok(())
}

fn required<'a>(fields: &'a BTreeMap<String, String>, name: &str, line_no: usize) -> Result<&'a str, String> {
    fields
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("line {line_no}: missing field {name}"))
}

fn required_parse<T>(fields: &BTreeMap<String, String>, name: &str, line_no: usize) -> Result<T, String>
where
    T: std::str::FromStr,
{
    parse_value(required(fields, name, line_no)?, name).map_err(|error| format!("line {line_no}: {error}"))
}

fn parse_value<T>(value: &str, name: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    value
        .parse::<T>()
        .map_err(|_| format!("invalid {name}: {value}"))
}

fn parse_layer_pair(value: &str) -> Result<(u32, u32), String> {
    let (layer, datatype) = value
        .split_once('/')
        .ok_or_else(|| format!("layer must be L/D: {value}"))?;
    Ok((parse_value(layer, "layer")?, parse_value(datatype, "datatype")?))
}

pub fn unhex(value: &str, field: &str) -> Result<String, String> {
    if !value.is_ascii() {
        return Err(format!("invalid {field}: non-hex byte"));
    }
    if value.len() % 2 != 0 {
        return Err(format!("invalid {field}: odd hex length"));
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for offset in (0..value.len()).step_by(2) {
        bytes.push(
            u8::from_str_radix(&value[offset..offset + 2], 16)
                .map_err(|_| format!("invalid {field}: non-hex byte"))?,
        );
    }
    String::from_utf8(bytes).map_err(|_| format!("invalid {field}: not UTF-8"))
}

pub fn parse_color(value: &str) -> Result<[u8; 4], String> {
    let hex = value
        .strip_prefix('#')
        .ok_or_else(|| format!("color must start with #: {value}"))?;
    if !hex.is_ascii() || (hex.len() != 6 && hex.len() != 8) {
        return Err(format!("color must have 6 or 8 hex digits: {value}"));
    }
    let byte = |offset: usize| {
        u8::from_str_radix(&hex[offset..offset + 2], 16).map_err(|_| format!("invalid color: {value}"))
    };
    Ok([byte(0)?, byte(2)?, byte(4)?, if hex.len() == 8 { byte(6)? } else { 255 }])
}

pub fn parse_fill(value: &str) -> Result<LayerFill, String> {
    match value {
        "solid" => Ok(LayerFill::Solid),
        "speckle" => Ok(LayerFill::Speckle),
        "clear" => Ok(LayerFill::Clear),
        _ => {
            let hex = value
                .strip_prefix("pat:")
                .ok_or_else(|| format!("unknown fill: {value}"))?;
            if !hex.is_ascii() || hex.len() != 64 {
                return Err("pat fill requires exactly 64 hex digits".to_string());
            }
            let mut rows = [0u16; 16];
            for (index, row) in rows.iter_mut().enumerate() {
                *row = u16::from_str_radix(&hex[index * 4..index * 4 + 4], 16)
                    .map_err(|_| format!("invalid pat fill: {value}"))?;
            }
            Ok(LayerFill::Pattern(rows))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> String {
        text.bytes().map(|byte| format!("{byte:02x}")).collect()
    }

    fn spec_text() -> String {
        format!(
            "deck unit=2.5e-05\n\
             source path_hex={}\n\
             source path_hex={}\n\
             layer out=0 name_hex={} color=#0000ff fill=solid width=1\n\
             layer out=1 key=2/7 name_hex={} color=#ffff00 fill=speckle width=2\n\
             placement source=1 layer=456/0 out=1 scale=2 dx=100 dy=200 order=1\n\
             placement source=0 layer=123/43 out=0 scale=8 dx=-8 dy=16 order=0\n",
            hex("/a/chipA.oas.floe"),
            hex("/b/chipB.oas.floe"),
            hex("$1 METAL1"),
            hex("$2 VIA1"),
        )
    }

    #[test]
    fn parses_a_spec_and_orders_placements() {
        let spec = DeckSpec::parse(&spec_text()).unwrap();
        assert_eq!(spec.unit, 2.5e-05);
        assert_eq!(spec.sources, vec!["/a/chipA.oas.floe", "/b/chipB.oas.floe"]);
        assert_eq!(spec.layers[1].name, "$2 VIA1");
        assert_eq!(spec.layers[1].fill, LayerFill::Speckle);
        assert_eq!(spec.layers[1].outline_width, 2);
        assert_eq!(spec.layers[0].color, [0, 0, 255, 255]);
        assert_eq!((spec.layers[0].layer, spec.layers[0].datatype), (0, 0), "key defaults to out/0");
        assert_eq!((spec.layers[1].layer, spec.layers[1].datatype), (2, 7));
        let ordered = spec.ordered_placements();
        assert_eq!(ordered[0].index, 1, "order 0 paints first");
        assert_eq!(ordered[0].scale, 8.0);
        assert_eq!((ordered[1].layer, ordered[1].datatype), (456, 0));
    }

    #[test]
    fn rejects_broken_specs() {
        let missing_unit = spec_text().replace("deck unit=2.5e-05\n", "");
        assert!(DeckSpec::parse(&missing_unit).unwrap_err().contains("no deck line"));
        let bad_source = spec_text().replace("placement source=1", "placement source=7");
        assert!(DeckSpec::parse(&bad_source).unwrap_err().contains("names source 7"));
        let bad_layer = spec_text().replace("out=1 scale=2", "out=9 scale=2");
        assert!(DeckSpec::parse(&bad_layer).unwrap_err().contains("deck layer 9"));
        let bad_scale = spec_text().replace("scale=8", "scale=0");
        assert!(DeckSpec::parse(&bad_scale).unwrap_err().contains("scale must be positive"));
        let unknown = spec_text().replace("order=1", "rot=1");
        assert!(DeckSpec::parse(&unknown).unwrap_err().contains("unknown field rot"));
        let dup = spec_text().replace("layer out=1", "layer out=0");
        assert!(DeckSpec::parse(&dup).unwrap_err().contains("duplicate deck layer 0"));
        let dup_key = spec_text().replace("key=2/7", "key=0/0");
        assert!(DeckSpec::parse(&dup_key).unwrap_err().contains("duplicate deck layer key 0/0"));
    }

    fn placement(scale: f64, dx: f64, dy: f64) -> DeckPlacement {
        DeckPlacement {
            index: 0,
            source: 0,
            layer: 1,
            datatype: 0,
            out: 0,
            scale,
            dx,
            dy,
            order: 0,
        }
    }

    #[test]
    fn source_view_inverts_the_placement() {
        let view = RasterViewBox::new(100.0, 200.0, 900.0, 600.0).unwrap();
        let p = placement(4.0, 100.0, 200.0);
        let sv = source_view(&view, &p).unwrap();
        assert_eq!((sv.x0, sv.y0, sv.x1, sv.y1), (0.0, 0.0, 200.0, 100.0));
        // and the transformed source bounds land back on the deck view
        let bbox = BBox {
            x0: 0,
            y0: 0,
            x1: 200,
            y1: 100,
        };
        assert_eq!(transform_bbox(&bbox, &p), [100.0, 200.0, 900.0, 600.0]);
        assert!(boxes_intersect(&transform_bbox(&bbox, &p), &[0.0, 0.0, 100.0, 200.0]));
        assert!(!boxes_intersect(&transform_bbox(&bbox, &p), &[0.0, 0.0, 99.0, 199.0]));
        assert!(!boxes_intersect(&transform_bbox(&BBox::EMPTY, &p), &[0.0, 0.0, 1e9, 1e9]));
    }

    #[test]
    fn plan_request_scales_pixels_per_dbu() {
        let request = DeckRenderRequest {
            view: RasterViewBox::new(0.0, 0.0, 800.0, 400.0).unwrap(),
            width: 400,
            height: 200,
            depth: u32::MAX,
            cut_px: 2.0,
            exact: false,
            visible: None,
            frames: false,
            mono: false,
            workers: 1,
            decode_workers: 1,
            tile_size: 64,
            decode_pages: None,
        };
        let p = placement(4.0, 0.0, 0.0);
        let sv = source_view(&request.view, &p).unwrap();
        let plan = source_plan_request(&sv, &request, &p).unwrap();
        // 400 px over 200 source dbu: 2 px per source dbu, 0.5 deck-dbu per px
        assert_eq!(plan.px_per_dbu, 2.0);
        assert_eq!(plan.cut_dbu, 1);
        assert_eq!(plan.visible_layers, Some(vec!["1/0".to_string()]));
        assert_eq!((plan.view.x0, plan.view.y0, plan.view.x1, plan.view.y1), (0, 0, 200, 100));
    }

    #[test]
    fn frame_planes_split_white_over_gray() {
        // gray hollow, white hollow, untouched, gray wash
        let pass = [128, 128, 128, 255, 255, 255, 255, 255, 0, 0, 0, 0, 128, 128, 128, 255];
        let mut under = vec![0u8; 16];
        let mut over = vec![0u8; 16];
        split_frame_planes(&pass, &mut under, &mut over);
        assert_eq!(under, vec![128, 128, 128, 255, 0, 0, 0, 0, 0, 0, 0, 0, 128, 128, 128, 255]);
        assert_eq!(over, vec![0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0]);
        // geometry laid between them: a placement's design covers the
        // gray wash but never the white frame - and BLACK design covers
        // the wash too (the geometry buffer keeps alpha 0 only where no
        // pass painted; review 3rd P2-1)
        let mut layered = vec![0, 0, 0, 255].repeat(4);
        overlay(&mut layered, &under);
        overlay(&mut layered, &[0, 0, 0, 255, 9, 9, 9, 255, 0, 0, 0, 0, 0, 0, 0, 255]);
        overlay(&mut layered, &over);
        assert_eq!(layered, vec![0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn overlay_is_opaque_over() {
        let mut composite = vec![0, 0, 0, 255, 9, 9, 9, 255];
        overlay(&mut composite, &[1, 2, 3, 255, 0, 0, 0, 0]);
        assert_eq!(composite, vec![1, 2, 3, 255, 9, 9, 9, 255]);
        overlay(&mut composite, &[0, 0, 0, 0, 4, 5, 6, 128]);
        assert_eq!(composite, vec![1, 2, 3, 255, 4, 5, 6, 128]);
    }

    #[test]
    fn colour_and_fill_specs() {
        assert_eq!(parse_color("#0a0b0c").unwrap(), [10, 11, 12, 255]);
        assert_eq!(parse_color("#0a0b0c80").unwrap(), [10, 11, 12, 128]);
        assert!(parse_color("0a0b0c").is_err());
        assert_eq!(parse_fill("clear").unwrap(), LayerFill::Clear);
        assert!(parse_fill("pat:00").is_err());
        assert_eq!(unhex(&hex("x y"), "f").unwrap(), "x y");
        assert!(unhex("abc", "f").is_err());
    }
}
