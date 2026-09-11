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

use crate::cache::DecodedPage;
use crate::{
    render_geometry_styled_cancellable_windowed, Cache,
    CacheLayer, DecodedPageCache, FrameScene,
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

/// Device pixels of slack around a placement's bounds: the widest
/// outline stroke (8) plus the half-pixel coverage rule.
const WINDOW_SLACK: f64 = 9.0;

/// What a placement's device window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    /// The placement can touch only `(col0, row0, width, height)`.
    Part(u32, u32, u32, u32),
    /// The placement misses the frame.
    Outside,
    /// The window could not be bounded (a coordinate overflowed): the
    /// placement takes the full frame.
    Full,
}

/// The device window of a placement whose SOURCE bounds are `bbox`
/// (source dbu, exact integers) seen through `source_view` (the same
/// source-unit view the raster maps with). Review 2026-09-09 (5th)
/// P2-2: the first version mapped the DECK bounds through the deck
/// view; at a large offset (`dx` = 1e16, `scale` 0.001) the deck
/// coordinates round to multiples of 2 while the raster, which maps
/// source dbu through the source view, does not - the window landed
/// beside the placement and the shape vanished under the optimization
/// (the full-frame path drew it). The window now uses the raster's own
/// quantities and formula `(x - view.x0) * width / span`, so it agrees
/// with the raster up to the last-bit rounding the 9 px slack covers.
/// The raster runs on the FULL frame's mapping and tile grid, only
/// skipping tiles outside the window, so the pixels are the full
/// render's byte for byte.
pub fn subwindow(bbox: &BBox, source_view: &RasterViewBox, width: u32, height: u32) -> Window {
    if width == 0 || height == 0 || bbox.is_empty() {
        return Window::Outside;
    }
    let view = source_view;
    let span_x = view.x1 - view.x0;
    let span_y = view.y1 - view.y0;
    if !(span_x > 0.0) || !(span_y > 0.0) {
        return Window::Full;
    }
    // device columns grow with x, rows grow DOWN (device y = y1 - y)
    let px_x = |x: i64| (x as f64 - view.x0) * width as f64 / span_x;
    let px_y = |y: i64| (view.y1 - y as f64) * height as f64 / span_y;
    let (x0, x1) = (px_x(bbox.x0), px_x(bbox.x1));
    let (y0, y1) = (px_y(bbox.y1), px_y(bbox.y0));
    if [x0, x1, y0, y1].iter().any(|v| !v.is_finite()) {
        return Window::Full;
    }
    // beyond the frame on either side: outside (the comparisons are
    // made before any cast, which would saturate)
    let w = width as f64;
    let h = height as f64;
    if x1 + WINDOW_SLACK <= 0.0 || x0 - WINDOW_SLACK >= w || y1 + WINDOW_SLACK <= 0.0 || y0 - WINDOW_SLACK >= h {
        return Window::Outside;
    }
    let c0 = (x0 - WINDOW_SLACK).floor().max(0.0).min(w) as u32;
    let c1 = (x1 + WINDOW_SLACK).ceil().max(0.0).min(w) as u32;
    let r0 = (y0 - WINDOW_SLACK).floor().max(0.0).min(h) as u32;
    let r1 = (y1 + WINDOW_SLACK).ceil().max(0.0).min(h) as u32;
    if c0 >= c1 || r0 >= r1 {
        return Window::Outside;
    }
    Window::Part(c0, r0, c1 - c0, r1 - r0)
}

/// A window-sized `pass` (`w` x `h`, the raster's windowed frame) laid
/// opaque-over onto the `width`-wide `composite` at `(col0, row0)`.
pub fn overlay_window(composite: &mut [u8], width: u32, pass: &[u8], window: (u32, u32, u32, u32)) {
    let (col0, row0, w, h) = window;
    let stride = width as usize * 4;
    let len = w as usize * 4;
    for row in 0..h as usize {
        let at = (row0 as usize + row) * stride + col0 as usize * 4;
        overlay(&mut composite[at..at + len], &pass[row * len..(row + 1) * len]);
    }
}

/// `split_frame_planes` of a window-sized pass into the full-frame
/// under/over planes.
pub fn split_frame_planes_window(pass: &[u8], width: u32, under: &mut [u8], over: &mut [u8], window: (u32, u32, u32, u32)) {
    let (col0, row0, w, h) = window;
    let stride = width as usize * 4;
    let len = w as usize * 4;
    for row in 0..h as usize {
        let at = (row0 as usize + row) * stride + col0 as usize * 4;
        split_frame_planes(&pass[row * len..(row + 1) * len], &mut under[at..at + len], &mut over[at..at + len]);
    }
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
    /// Step 2 (analysis 2026-09-09): raster and composite only the
    /// sub-window of the frame a placement can touch, instead of the
    /// whole W x H per pass. Off = the full-frame path (kill switch
    /// FLOE_RUST_DECK_SUBWINDOW=off); pixels are identical either way.
    pub subwindow: bool,
    pub workers: u16,
    pub decode_workers: u16,
    pub tile_size: u16,
    pub decode_pages: Option<usize>,
    /// Step 4 (JOBDECK.ko.md section 11): the jobdeck wide-view
    /// policy - what the size cut would drop keeps its on-screen
    /// existence as a footprint wash (PlanRequest::sub_cut_wash).
    pub wide: bool,
    /// Step 3: a pass over the slice limit streams slice by slice
    /// (true) instead of stopping at the budget with a partial frame
    /// (false, the pre-step-3 behaviour kept as the kill switch).
    pub stream: bool,
    /// Keep all-thin pages (the mask policy, the deck's default): the
    /// page hairline rule is off for the passes' plans. false is the
    /// plain layout's cull.
    pub thin_keep: bool,
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
    /// Sum of the frames-only passes' raster times.
    pub frame_raster_us: u64,
    pub composite_us: u64,
    /// Placements that reused another placement's plan and scene
    /// (same source, same plan request) this frame.
    pub scene_reuses: u32,
    /// Wall-clock time of the raster batches (their sum: they run one
    /// after another); `stats.raster_us` is the sum over passes.
    pub raster_wall_us: u64,
    /// Passes rastered in parallel at most (`stats.workers_used` is the
    /// tile workers of one pass).
    pub pass_workers: u16,
    pub batches: u32,
    /// Largest batch charge: newly decoded pages plus window images.
    pub batch_bytes_max: u64,
    /// Step 3: passes whose pages did not fit the slice limit and
    /// were rastered slice by slice, and their slices in total.
    pub streamed_passes: u32,
    pub slices: u32,
    /// Step 4: sub-cut washes the planner emitted across the passes.
    pub wide_washes: u64,
    /// Planner culls summed over the passes' plans (perf line).
    pub culls: crate::cache::PlanCullCounts,
    /// Occupancy summary (docs/OCCUPANCY_PLAN.ko.md M4): passes drawn
    /// from their source's design.ovo, passes under the keep policy
    /// that wanted one and had none (no file / invalid / near /
    /// layers), and the summary cells painted.
    pub summary_passes: u32,
    pub summary_none_passes: u32,
    pub summary_cells: u64,
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
        let mut tally = Tally::default();
        // Step 2b (analysis 2026-09-09, no measurement at hand): two
        // repetitions dominate a multi-placement frame - the plan and
        // scene of the same source at the same scale (a row of the
        // same chip: identical plan requests), and the strictly serial
        // pass loop. Scenes are reused within the frame when the plan
        // request is identical (same source, same clipped source view,
        // cut, depth, layer), and prepared passes are rastered in
        // parallel in batches bounded by the page budget; compositing
        // stays in placement order, so pixels are unchanged.
        let mut scene_cache: Vec<(usize, PlanRequest, Arc<FrameScene>, u64)> = Vec::new();
        let mut cache_bytes = 0u64;
        let mut batch: Vec<PreparedPass> = Vec::new();
        let mut batch_bytes = 0u64;
        let mut batch_bytes_max = 0u64;
        let mut wide_washes = 0u64;
        let mut culls = crate::cache::PlanCullCounts::default();
        let mut summary_passes = 0u32;
        let mut summary_none_passes = 0u32;
        // FLOE_RUST_OCCUPANCY=off: the field kill switch of the summary
        let occupancy_off = std::env::var("FLOE_RUST_OCCUPANCY").as_deref() == Ok("off");
        for placed_index in 0..self.placements.len() {
            let (out, source_index) = {
                let placed = &self.placements[placed_index];
                (placed.spec.out, placed.spec.source)
            };
            if request.visible.as_ref().is_some_and(|visible| !visible.contains(&out)) {
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
            // the plan view is the source view clipped to the source's
            // own bounds (in f64, BEFORE any integer conversion - review
            // 2026-09-09 (6th): a placement far outside the frame, dx =
            // 1e16, overflowed i64 in its source view and failed the
            // whole frame; the plan clip is also the exact source-space
            // cull that replaced the deck-space one). Pages and cells
            // are selected by intersection, so two placements showing
            // the whole source ask the planner the same question - and
            // share its answer (scene reuse below).
            let plan_request = match source_plan_request(
                &source_view,
                request,
                &self.placements[placed_index].spec,
                &self.sources[source_index].top_bbox,
            )? {
                Some(plan_request) => plan_request,
                None => {
                    passes_skipped += 1;
                    continue;
                }
            };
            // occupancy summary per pass (M4): decided on the SOURCE
            // view (its px_per_dbu carries the placement scale), the
            // same conditions as a single cache; a summarized pass
            // plans no pages and paints its planes in the pass raster
            let summary = self.sources[source_index].cache.summary_selection(
                &plan_request,
                request.thin_keep,
                occupancy_off,
            )?;
            let plan_request = self.sources[source_index].cache.page_plan_request(
                &plan_request,
                &summary,
                !request.frames,
            )?;
            if summary.is_active() {
                summary_passes += 1;
            } else if request.thin_keep
                && matches!(
                    summary.none,
                    Some(crate::summary::NONE_NOFILE)
                        | Some(crate::summary::NONE_INVALID)
                        | Some(crate::summary::NONE_NEAR)
                        | Some(crate::summary::NONE_LAYERS)
                )
            {
                summary_none_passes += 1;
            }
            // the device sub-window this placement can touch (step 2):
            // the raster and the overlays run on it alone; the plan
            // keeps the whole source view so page selection is
            // unchanged
            let window = if request.subwindow {
                match subwindow(
                    &self.sources[source_index].top_bbox,
                    &source_view,
                    request.width,
                    request.height,
                ) {
                    Window::Part(c0, r0, w, h) => (c0, r0, w, h),
                    Window::Full => (0, 0, request.width, request.height),
                    Window::Outside => {
                        passes_skipped += 1;
                        continue;
                    }
                }
            } else {
                (0, 0, request.width, request.height)
            };
            let cached = scene_cache
                .iter()
                .find(|(src, req, _, _)| *src == source_index && *req == plan_request)
                .map(|entry| Arc::clone(&entry.2));
            let (scene, new_bytes) = match cached {
                Some(scene) => {
                    tally.scene_reuses += 1;
                    (scene, 0u64)
                }
                None => {
                    self.share_budget(source_index);
                    let source = &mut self.sources[source_index];
                    let planned = source.cache.plan(&plan_request)?;
                    stats.plan_us = stats.plan_us.saturating_add(planned.stats.plan_us);
                    plan_pages = plan_pages.saturating_add(planned.summary.pages);
                    wide_washes = wide_washes.saturating_add(planned.plan.stats.sub_cut_washes);
                    culls.add(&planned.summary.culls);
                    check_generation(cancellation, generation)?;
                    if planned
                        .plan
                        .wcells
                        .binary_search_by_key(&planned.plan.top, |cell| cell.key)
                        .is_err()
                    {
                        // The planner dropped the whole source: its top
                        // cell is below the cut at this scale (a mark
                        // placed at 0.2x on a full-deck view). A single
                        // cache never sees this - its top is the chip -
                        // so the scene would reject the plan; for a deck
                        // it is an empty pass, like any sub-cut cell.
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
                    let planned_pages = prioritized.len();
                    let selected: Vec<u32> = prioritized
                        .into_iter()
                        .take(request.decode_pages.unwrap_or(usize::MAX))
                        .map(|(_, page_id)| page_id)
                        .collect();
                    // Review 2026-09-10 (8th) P2-1: pages the request's
                    // decode_pages limit leaves out are missing from the
                    // frame whichever way the pass is rastered - the
                    // whole-scene path saw them as the scene's deferred
                    // pages, the streamed path (every slice scene is
                    // partial by construction) reported nothing. They
                    // are counted here, before either path.
                    let excluded = planned_pages.saturating_sub(selected.len());
                    if excluded > 0 {
                        deferred = deferred.saturating_add(excluded.try_into().unwrap_or(u32::MAX));
                        partial = true;
                    }
                    // Review 2026-09-09 P1-2: the scene's Arcs keep every
                    // page of this pass alive whatever the LRU evicted,
                    // so one pass is charged against the shared budget
                    // like a single-cache generation. Step 3 (analysis
                    // 2026-09-09): a pass whose pages do not fit the
                    // slice limit (half the budget) is no longer cut
                    // off at the budget (partial frame, pages "over
                    // budget (not drawn)") - it is STREAMED: its pages
                    // are rastered slice by slice on its window, each
                    // slice's scene dropped before the next is decoded
                    // (`stream_pass`). Every pass paints only its own
                    // layer in one colour, so the union of the slices
                    // is the whole-scene raster pixel for pixel.
                    let plan = Arc::new(planned.plan);
                    let slice_limit = (self.budget_bytes / STREAM_SLICE_DIV).max(1);
                    let mut chunks = decode_chunks(&source.cache, &selected, self.budget_bytes).into_iter();
                    let mut decoded = Vec::with_capacity(selected.len());
                    let mut pass_bytes = 0u64;
                    let mut streamed = false;
                    let mut over = false;
                    while let Some(chunk) = chunks.next() {
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
                            unique_pages.insert((source_index, page.page_id));
                            decoded.push(page);
                        }
                        check_generation(cancellation, generation)?;
                        if request.stream {
                            if pass_bytes >= slice_limit && chunks.len() > 0 {
                                streamed = true;
                                break;
                            }
                        } else if pass_bytes >= self.budget_bytes {
                            // FLOE_RUST_DECK_STREAM=off: stop at the
                            // budget, defer the rest (partial frame)
                            over = true;
                        }
                    }
                    pass_bytes_max = pass_bytes_max.max(pass_bytes);
                    if streamed {
                        // the passes prepared so far composite first
                        // (placement order), then this pass streams
                        // straight onto the composite
                        raster_batch(
                            &batch,
                            request,
                            generation,
                            cancellation,
                            &mut composite,
                            frames_under.as_deref_mut(),
                            frames_over.as_deref_mut(),
                            &mut tally,
                        )?;
                        batch.clear();
                        batch_bytes = 0;
                        let streamed = stream_pass(
                            source,
                            source_index,
                            Arc::clone(&plan),
                            decoded,
                            pass_bytes,
                            chunks.collect(),
                            slice_limit,
                            StreamTarget {
                                style,
                                window,
                                source_view,
                                request,
                                generation,
                                cancellation,
                            },
                            &mut composite,
                            frames_under.as_deref_mut(),
                            frames_over.as_deref_mut(),
                            &mut tally,
                            &mut stats,
                            &mut unique_pages,
                        )?;
                        pass_bytes_max = pass_bytes_max.max(streamed.pass_bytes_max);
                        pages = pages.saturating_add(streamed.pages);
                        scene_us = scene_us.saturating_add(streamed.scene_us);
                        continue;
                    }
                    let scene_started = std::time::Instant::now();
                    let mut scene = FrameScene::new_shared(&source.cache, plan, decoded)?;
                    if summary.is_active() {
                        scene.set_summaries(summary.planes.clone());
                    }
                    let scene = Arc::new(scene);
                    scene_us = scene_us.saturating_add(scene_started.elapsed().as_micros() as u64);
                    partial |= scene.is_partial();
                    pages = pages.saturating_add(scene.available_pages().try_into().unwrap_or(u32::MAX));
                    // the frame's scene cache never holds more than the
                    // budget: rather than evict selectively it starts
                    // over (the pages themselves stay in the LRUs)
                    if cache_bytes.saturating_add(pass_bytes) > self.budget_bytes {
                        scene_cache.clear();
                        cache_bytes = 0;
                    }
                    scene_cache.push((source_index, plan_request.clone(), Arc::clone(&scene), pass_bytes));
                    cache_bytes = cache_bytes.saturating_add(pass_bytes);
                    (scene, pass_bytes)
                }
            };
            check_generation(cancellation, generation)?;
            let frames = request.frames && scene.subtree_has_frames(scene.top());
            // Review 2026-09-09 (5th) P1-1: a batch holds every pass's
            // window image (geometry, and the frames plane) until it
            // is composited, and a reused scene costs no new decoded
            // bytes - so 64 screen-sized passes piled up 300 MB at a
            // 1 MiB budget. The images are charged to the batch like
            // the pages; a batch closes BEFORE a pass would push it
            // past half the budget (a lone pass always runs).
            let charge = new_bytes.saturating_add(pass_image_bytes(window, frames));
            if !batch.is_empty()
                && (batch_bytes.saturating_add(charge) > self.budget_bytes / 2
                    || batch.len() >= MAX_BATCH_PASSES)
            {
                raster_batch(
                    &batch,
                    request,
                    generation,
                    cancellation,
                    &mut composite,
                    frames_under.as_deref_mut(),
                    frames_over.as_deref_mut(),
                    &mut tally,
                )?;
                batch.clear();
                batch_bytes = 0;
            }
            batch.push(PreparedPass {
                scene,
                style,
                window,
                source_view,
                frames,
            });
            batch_bytes = batch_bytes.saturating_add(charge);
            batch_bytes_max = batch_bytes_max.max(batch_bytes);
        }
        if !batch.is_empty() {
            raster_batch(
                &batch,
                request,
                generation,
                cancellation,
                &mut composite,
                frames_under.as_deref_mut(),
                frames_over.as_deref_mut(),
                &mut tally,
            )?;
            batch.clear();
        }
        drop(scene_cache);
        accumulate_raster(&mut stats, &tally.stats);
        passes = passes.saturating_add(tally.passes);
        frame_passes = frame_passes.saturating_add(tally.frame_passes);
        frame_raster_us = frame_raster_us.saturating_add(tally.frame_raster_us);
        composite_us = composite_us.saturating_add(tally.composite_us);
        rectangle_member_paints = rectangle_member_paints.saturating_add(tally.rectangle_member_paints);
        polygon_member_paints = polygon_member_paints.saturating_add(tally.polygon_member_paints);
        path_member_paints = path_member_paints.saturating_add(tally.path_member_paints);
        frame_member_paints = frame_member_paints.saturating_add(tally.frame_member_paints);
        let scene_reuses = tally.scene_reuses;
        let raster_wall_us = tally.raster_wall_us;
        let pass_workers = tally.pass_workers;
        let batches = tally.batches;
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
            scene_reuses,
            raster_wall_us,
            pass_workers,
            batches,
            batch_bytes_max,
            streamed_passes: tally.streamed_passes,
            slices: tally.slices,
            wide_washes,
            culls,
            summary_passes,
            summary_none_passes,
            summary_cells: tally.summary_cells,
        })
    }
}

/// Bytes a pass's window images occupy until the batch composites
/// them: the geometry image, and the frames plane when the pass has
/// one.
fn pass_image_bytes(window: (u32, u32, u32, u32), frames: bool) -> u64 {
    let (_, _, w, h) = window;
    let image = (w as u64).saturating_mul(h as u64).saturating_mul(4);
    image.saturating_mul(if frames { 2 } else { 1 })
}

/// Passes rastered together at most (a batch is also bounded by half
/// the page budget of newly decoded pages).
const MAX_BATCH_PASSES: usize = 64;

/// Step 3: a pass whose decoded pages exceed budget / this streams in
/// slices of at most that size (a slice may overshoot by one decode
/// chunk, at most budget / DECODE_CHUNK_DIV of encoded bytes).
const STREAM_SLICE_DIV: u64 = 2;

/// Where a streamed pass paints: its style, device window and source
/// view, with the frame's request and cancellation.
struct StreamTarget<'a> {
    style: LayerStyle,
    window: (u32, u32, u32, u32),
    source_view: RasterViewBox,
    request: &'a DeckRenderRequest,
    generation: u64,
    cancellation: &'a RenderCancellation,
}

struct StreamReport {
    pass_bytes_max: u64,
    pages: u32,
    scene_us: u64,
}

/// Raster one placement slice by slice (step 3). `first` is the
/// slice decoded so far, `rest` the remaining decode chunks in
/// priority order; slices are cut at `slice_limit` decoded bytes. The
/// frames-only pass (hierarchy frames come from the plan, not from
/// pages) runs once on a page-less scene; each geometry slice is
/// laid opaque-over onto the composite in turn and dropped.
#[allow(clippy::too_many_arguments)]
fn stream_pass(
    source: &mut DeckSource,
    source_index: usize,
    plan: Arc<floe_vfs::hier::HierPlan>,
    first: Vec<Arc<DecodedPage>>,
    first_bytes: u64,
    rest: Vec<Vec<u32>>,
    slice_limit: u64,
    target: StreamTarget<'_>,
    composite: &mut [u8],
    mut frames_under: Option<&mut [u8]>,
    mut frames_over: Option<&mut [u8]>,
    tally: &mut Tally,
    stats: &mut RenderStats,
    unique_pages: &mut BTreeSet<(usize, u32)>,
) -> Result<StreamReport, String> {
    let request = target.request;
    let (col0, row0, win_w, win_h) = target.window;
    let device_window = [col0, row0, col0 + win_w, row0 + win_h];
    let raster = GeometryRasterRequest {
        view: target.source_view,
        width: request.width,
        height: request.height,
        background: [0, 0, 0, 0],
        foreground: [255, 255, 255, 255],
        workers: request.workers.max(1),
        tile_size: request.tile_size,
    };
    let mut report = StreamReport {
        pass_bytes_max: first_bytes,
        pages: 0,
        scene_us: 0,
    };
    if request.frames {
        let scene_started = std::time::Instant::now();
        let empty = FrameScene::new_shared(&source.cache, Arc::clone(&plan), Vec::new())?;
        report.scene_us = report.scene_us.saturating_add(scene_started.elapsed().as_micros() as u64);
        if empty.subtree_has_frames(empty.top()) {
            let frames_only = StyledGeometryRasterRequest {
                raster,
                layers: Vec::new(),
                hierarchy_frames: true,
                mono: request.mono,
            };
            // review 2026-09-10 (8th) P2-2: the frames raster is part
            // of the streamed pass's wall-clock like the geometry
            let raster_started = std::time::Instant::now();
            let out = render_geometry_styled_cancellable_windowed(
                &empty,
                &frames_only,
                target.generation,
                target.cancellation,
                device_window,
            )?;
            tally.raster_wall_us = tally
                .raster_wall_us
                .saturating_add(raster_started.elapsed().as_micros() as u64);
            tally.frame_raster_us = tally.frame_raster_us.saturating_add(out.stats.raster_us);
            let split_started = std::time::Instant::now();
            if let (Some(under), Some(over)) = (frames_under.as_deref_mut(), frames_over.as_deref_mut()) {
                split_frame_planes_window(out.frame.pixels(), request.width, under, over, target.window);
            }
            tally.composite_us = tally.composite_us.saturating_add(split_started.elapsed().as_micros() as u64);
            accumulate_raster(&mut tally.stats, &out.stats);
            tally.frame_member_paints = tally.frame_member_paints.saturating_add(out.frame_member_paints);
            tally.frame_passes += 1;
        }
    }
    let styled = StyledGeometryRasterRequest {
        raster,
        layers: vec![target.style],
        hierarchy_frames: false,
        mono: request.mono,
    };
    let mut slice = first;
    let mut rest = rest.into_iter();
    loop {
        let scene_started = std::time::Instant::now();
        let scene = FrameScene::new_shared(&source.cache, Arc::clone(&plan), std::mem::take(&mut slice))?;
        report.scene_us = report.scene_us.saturating_add(scene_started.elapsed().as_micros() as u64);
        report.pages = report
            .pages
            .saturating_add(scene.available_pages().try_into().unwrap_or(u32::MAX));
        let raster_started = std::time::Instant::now();
        let out = render_geometry_styled_cancellable_windowed(
            &scene,
            &styled,
            target.generation,
            target.cancellation,
            device_window,
        )?;
        tally.raster_wall_us = tally
            .raster_wall_us
            .saturating_add(raster_started.elapsed().as_micros() as u64);
        drop(scene);
        let overlay_started = std::time::Instant::now();
        overlay_window(composite, request.width, out.frame.pixels(), target.window);
        tally.composite_us = tally.composite_us.saturating_add(overlay_started.elapsed().as_micros() as u64);
        accumulate_raster(&mut tally.stats, &out.stats);
        tally.rectangle_member_paints =
            tally.rectangle_member_paints.saturating_add(out.rectangle_member_paints);
        tally.polygon_member_paints = tally.polygon_member_paints.saturating_add(out.polygon_member_paints);
        tally.path_member_paints = tally.path_member_paints.saturating_add(out.path_member_paints);
        tally.slices += 1;
        drop(out);
        check_generation(target.cancellation, target.generation)?;
        // the next slice
        let mut slice_bytes = 0u64;
        while slice_bytes < slice_limit {
            let Some(chunk) = rest.next() else { break };
            let (chunk_pages, decode_stats) = source.pages.load_cancellable(
                &source.cache,
                &chunk,
                request.decode_workers,
                target.generation,
                target.cancellation,
            )?;
            accumulate_decode(stats, &decode_stats);
            for page in chunk_pages {
                slice_bytes = slice_bytes
                    .checked_add(page.estimated_bytes())
                    .ok_or_else(|| "decoded generation byte charge overflow".to_string())?;
                unique_pages.insert((source_index, page.page_id));
                slice.push(page);
            }
            check_generation(target.cancellation, target.generation)?;
        }
        if slice.is_empty() {
            break;
        }
        report.pass_bytes_max = report.pass_bytes_max.max(slice_bytes);
    }
    tally.passes += 1;
    tally.streamed_passes += 1;
    tally.pass_workers = tally.pass_workers.max(1);
    tally.batches += 1;
    Ok(report)
}

/// A placement ready to raster: its scene (possibly shared with other
/// placements of the same source and plan), style, device window and
/// source view.
struct PreparedPass {
    scene: Arc<FrameScene>,
    style: LayerStyle,
    window: (u32, u32, u32, u32),
    source_view: RasterViewBox,
    frames: bool,
}

struct PassOutput {
    geometry: crate::GeometryRasterReport,
    frames: Option<crate::GeometryRasterReport>,
}

#[derive(Default)]
struct Tally {
    stats: RenderStats,
    passes: u32,
    frame_passes: u32,
    scene_reuses: u32,
    frame_raster_us: u64,
    composite_us: u64,
    /// Wall-clock time of the batches' parallel raster phases (the
    /// batches run one after another, so their sum is the frame's
    /// real raster time); `stats.raster_us` is the SUM over passes.
    raster_wall_us: u64,
    /// Passes rastered at once, at most.
    pass_workers: u16,
    batches: u32,
    streamed_passes: u32,
    slices: u32,
    rectangle_member_paints: u64,
    polygon_member_paints: u64,
    path_member_paints: u64,
    frame_member_paints: u64,
    summary_cells: u64,
}

fn raster_pass(
    pass: &PreparedPass,
    request: &DeckRenderRequest,
    workers: u16,
    generation: u64,
    cancellation: &RenderCancellation,
) -> Result<PassOutput, String> {
    let (col0, row0, win_w, win_h) = pass.window;
    let device_window = [col0, row0, col0 + win_w, row0 + win_h];
    let raster = GeometryRasterRequest {
        view: pass.source_view,
        width: request.width,
        height: request.height,
        background: [0, 0, 0, 0],
        foreground: [255, 255, 255, 255],
        workers,
        tile_size: request.tile_size,
    };
    // a frames-only pass only when this placement's plan holds a
    // hierarchy frame at all (analysis 2026-09-09: the pass ran, and
    // composited a full frame, for every placement)
    let frames = if pass.frames {
        let frames_only = StyledGeometryRasterRequest {
            raster,
            layers: Vec::new(),
            hierarchy_frames: true,
            mono: request.mono,
        };
        Some(render_geometry_styled_cancellable_windowed(
            &pass.scene,
            &frames_only,
            generation,
            cancellation,
            device_window,
        )?)
    } else {
        None
    };
    let styled = StyledGeometryRasterRequest {
        raster,
        layers: vec![pass.style],
        hierarchy_frames: false,
        mono: request.mono,
    };
    let geometry = render_geometry_styled_cancellable_windowed(
        &pass.scene,
        &styled,
        generation,
        cancellation,
        device_window,
    )?;
    Ok(PassOutput { geometry, frames })
}

/// Raster a batch of prepared passes in parallel (one raster worker
/// each when several run at once, the full worker count for a lone
/// pass), then composite them in placement order.
#[allow(clippy::too_many_arguments)]
fn raster_batch(
    passes: &[PreparedPass],
    request: &DeckRenderRequest,
    generation: u64,
    cancellation: &RenderCancellation,
    composite: &mut [u8],
    mut frames_under: Option<&mut [u8]>,
    mut frames_over: Option<&mut [u8]>,
    tally: &mut Tally,
) -> Result<(), String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    if passes.is_empty() {
        return Ok(());
    }
    let concurrency = usize::from(request.workers.max(1)).min(passes.len());
    let per_pass_workers: u16 = if concurrency > 1 { 1 } else { request.workers.max(1) };
    let next = AtomicUsize::new(0);
    let results: Vec<Mutex<Option<Result<PassOutput, String>>>> =
        (0..passes.len()).map(|_| Mutex::new(None)).collect();
    // Review 2026-09-09 (5th) P2-3: the per-pass raster times are
    // summed into `raster_us` (4 parallel passes read as twice the
    // frame's real time); the batch's wall-clock and its pass
    // parallelism are reported beside it.
    let raster_started = std::time::Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..concurrency {
            let next = &next;
            let results = &results;
            scope.spawn(move || loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                if index >= passes.len() {
                    break;
                }
                let out = raster_pass(&passes[index], request, per_pass_workers, generation, cancellation);
                if let Ok(mut slot) = results[index].lock() {
                    *slot = Some(out);
                }
            });
        }
    });
    tally.raster_wall_us = tally
        .raster_wall_us
        .saturating_add(raster_started.elapsed().as_micros() as u64);
    tally.pass_workers = tally.pass_workers.max(concurrency.try_into().unwrap_or(u16::MAX));
    tally.batches += 1;
    for (index, pass) in passes.iter().enumerate() {
        let out = results[index]
            .lock()
            .map_err(|_| "pass result lock poisoned".to_string())?
            .take()
            .ok_or_else(|| "a pass was not rastered".to_string())??;
        if let Some(report) = out.frames {
            tally.frame_raster_us = tally.frame_raster_us.saturating_add(report.stats.raster_us);
            let split_started = std::time::Instant::now();
            if let (Some(under), Some(over)) = (frames_under.as_deref_mut(), frames_over.as_deref_mut()) {
                split_frame_planes_window(report.frame.pixels(), request.width, under, over, pass.window);
            }
            tally.composite_us = tally.composite_us.saturating_add(split_started.elapsed().as_micros() as u64);
            accumulate_raster(&mut tally.stats, &report.stats);
            tally.frame_member_paints = tally.frame_member_paints.saturating_add(report.frame_member_paints);
            tally.frame_passes += 1;
        }
        let overlay_started = std::time::Instant::now();
        overlay_window(composite, request.width, out.geometry.frame.pixels(), pass.window);
        tally.composite_us = tally.composite_us.saturating_add(overlay_started.elapsed().as_micros() as u64);
        accumulate_raster(&mut tally.stats, &out.geometry.stats);
        tally.rectangle_member_paints =
            tally.rectangle_member_paints.saturating_add(out.geometry.rectangle_member_paints);
        tally.polygon_member_paints =
            tally.polygon_member_paints.saturating_add(out.geometry.polygon_member_paints);
        tally.path_member_paints = tally.path_member_paints.saturating_add(out.geometry.path_member_paints);
        tally.summary_cells = tally.summary_cells.saturating_add(out.geometry.summary_cell_paints);
        tally.passes += 1;
    }
    check_generation(cancellation, generation)
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
    source_view_of(view, placement)
}

fn source_view_of(view: &RasterViewBox, placement: &DeckPlacement) -> Result<RasterViewBox, String> {
    let map = |v: f64, d: f64| (v - d) / placement.scale;
    RasterViewBox::new(
        map(view.x0, placement.dx),
        map(view.y0, placement.dy),
        map(view.x1, placement.dx),
        map(view.y1, placement.dy),
    )
}

/// The plan request of a placement: None when its source view misses
/// the source's bounds (`top_bbox`, source dbu) - decided in f64 on
/// the whole-dbu view, so no coordinate is converted to an integer
/// before it is known to lie inside the source's (i64) bounds.
fn source_plan_request(
    source_view: &RasterViewBox,
    request: &DeckRenderRequest,
    placement: &DeckPlacement,
    top_bbox: &BBox,
) -> Result<Option<PlanRequest>, String> {
    if top_bbox.is_empty() {
        return Ok(None);
    }
    // Review 2026-09-09 (7th): the clip never passes the source's
    // bounds through f64 - at 2^60 both ends of a 1 dbu wide bound
    // round to the same f64 and the view came back inverted. A bound
    // the view reaches past is used as its exact i64; only a view edge
    // strictly inside the bounds is converted.
    let (Some(x0), Some(x1), Some(y0), Some(y1)) = (
        clip_low(source_view.x0.floor(), top_bbox.x0, "source view x0")?,
        clip_high(source_view.x1.ceil(), top_bbox.x1, "source view x1")?,
        clip_low(source_view.y0.floor(), top_bbox.y0, "source view y0")?,
        clip_high(source_view.y1.ceil(), top_bbox.y1, "source view y1")?,
    ) else {
        return Ok(None);
    };
    if x0 > x1 || y0 > y1 {
        return Ok(None);
    }
    // the source span from the DECK span divided once: every placement
    // of one scale gets the same px_per_dbu (and cut), so their plan
    // requests compare equal for the frame's scene reuse - a
    // difference of two shifted quotients would not
    let span_x = (request.view.x1 - request.view.x0) / placement.scale;
    let span_y = (request.view.y1 - request.view.y0) / placement.scale;
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
        view: ViewBox::new(x0, y0, x1, y1)?,
        cut_dbu,
        visible_layers: Some(vec![format!("{}/{}", placement.layer, placement.datatype)]),
        depth: request.depth,
        px_per_dbu,
        exact: request.exact,
        sub_cut_wash: request.wide,
        page_hairline: !request.thin_keep,
        summary_layers: Vec::new(),
        prune_summary: false,
    };
    plan.validate()?;
    Ok(Some(plan))
}

/// The lower bound of a plan view axis: the view's whole-dbu edge `v`
/// clipped to the source's bound `b`. The bound is returned as its
/// exact i64 whenever the view reaches it (compared in f64, where `b`
/// may have rounded either way - `.max(b)` after a conversion covers
/// the other direction); None when the view edge lies beyond every
/// i64 on the far side, i.e. the view misses the source.
fn clip_low(v: f64, b: i64, name: &str) -> Result<Option<i64>, String> {
    if v <= b as f64 {
        return Ok(Some(b));
    }
    if v > i64::MAX as f64 {
        return Ok(None);
    }
    Ok(Some(checked_bound(v, name)?.max(b)))
}

/// The upper bound: see `clip_low`.
fn clip_high(v: f64, b: i64, name: &str) -> Result<Option<i64>, String> {
    if v >= b as f64 {
        return Ok(Some(b));
    }
    if v < i64::MIN as f64 {
        return Ok(None);
    }
    Ok(Some(checked_bound(v, name)?.min(b)))
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

#[cfg(test)]
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
            subwindow: true,
            workers: 1,
            decode_workers: 1,
            tile_size: 64,
            decode_pages: None,
            wide: false,
            stream: true,
            thin_keep: true,
        };
        let p = placement(4.0, 0.0, 0.0);
        let sv = source_view(&request.view, &p).unwrap();
        let big = bbox(-1000, -1000, 1000, 1000);
        let plan = source_plan_request(&sv, &request, &p, &big).unwrap().unwrap();
        // 400 px over 200 source dbu: 2 px per source dbu, 0.5 deck-dbu per px
        assert_eq!(plan.px_per_dbu, 2.0);
        assert_eq!(plan.cut_dbu, 1);
        assert_eq!(plan.visible_layers, Some(vec!["1/0".to_string()]));
        assert_eq!((plan.view.x0, plan.view.y0, plan.view.x1, plan.view.y1), (0, 0, 200, 100));
        // clipped to the source bounds
        let plan = source_plan_request(&sv, &request, &p, &bbox(50, 20, 300, 300)).unwrap().unwrap();
        assert_eq!((plan.view.x0, plan.view.y0, plan.view.x1, plan.view.y1), (50, 20, 200, 100));
        // a view edge on the bound is still an intersection
        assert!(source_plan_request(&sv, &request, &p, &bbox(200, 100, 300, 300)).unwrap().is_some());
        // beside the source: no plan
        assert!(source_plan_request(&sv, &request, &p, &bbox(201, 0, 300, 300)).unwrap().is_none());
        assert!(source_plan_request(&sv, &request, &p, &BBox::EMPTY).unwrap().is_none());
    }

    #[test]
    fn offscreen_placement_at_a_huge_offset_is_skipped_not_an_error() {
        // Review 2026-09-09 (6th): a placement at dx = 1e16, scale 0.001
        // seen from a view near the origin has the source view
        // -1e19 .. -1e19, beyond i64 - it used to overflow the integer
        // conversion and fail the WHOLE frame. It misses the source's
        // bounds, decided in f64: an empty pass.
        let request = DeckRenderRequest {
            view: RasterViewBox::new(0.0, 0.0, 800.0, 400.0).unwrap(),
            width: 400,
            height: 200,
            depth: u32::MAX,
            cut_px: 1.0,
            exact: false,
            visible: None,
            frames: false,
            mono: false,
            subwindow: true,
            workers: 1,
            decode_workers: 1,
            tile_size: 64,
            decode_pages: None,
            wide: false,
            stream: true,
            thin_keep: true,
        };
        let p = placement(0.001, 1e16, 1e16);
        let sv = source_view(&request.view, &p).unwrap();
        assert!(sv.x0 < i64::MIN as f64);
        let plan = source_plan_request(&sv, &request, &p, &bbox(0, 0, 20_000_000, 20_000_000)).unwrap();
        assert!(plan.is_none());
    }

    #[test]
    fn plan_clip_keeps_exact_bounds_at_huge_coordinates() {
        // Review 2026-09-09 (7th): a source 1 dbu wide at 2^60 + 1 ..
        // 2^60 + 2 - both ends round to 2^60 in f64, and a clip that
        // went through f64 came back as x0 = 2^60 + 1 > x1 = 2^60, an
        // invalid view that failed the frame. The bounds the view
        // reaches past are used as their exact i64.
        let request = DeckRenderRequest {
            view: RasterViewBox::new(0.0, 0.0, 800.0, 400.0).unwrap(),
            width: 400,
            height: 200,
            depth: u32::MAX,
            cut_px: 1.0,
            exact: false,
            visible: None,
            frames: false,
            mono: false,
            subwindow: true,
            workers: 1,
            decode_workers: 1,
            tile_size: 64,
            decode_pages: None,
            wide: false,
            stream: true,
            thin_keep: true,
        };
        let big = 1i64 << 60;
        // the placement puts the deck view's origin at the source's
        // 2^60 - 100 (the view then spans 2^60 - 100 .. 2^60 + 700)
        let p = placement(1.0, -((big - 100) as f64), 0.0);
        let sv = source_view(&request.view, &p).unwrap();
        let source = bbox(big + 1, 0, big + 2, 1000);
        let plan = source_plan_request(&sv, &request, &p, &source).unwrap().unwrap();
        assert_eq!((plan.view.x0, plan.view.x1), (big + 1, big + 2));
        assert_eq!((plan.view.y0, plan.view.y1), (0, 400));
        // the view edge strictly inside a bound converts, and the
        // exact bound wins over a converted edge that rounded past it
        let source = bbox(big - 200, 0, big + 2, 1000);
        let plan = source_plan_request(&sv, &request, &p, &source).unwrap().unwrap();
        assert!(plan.view.x0 >= big - 200 && plan.view.x0 <= big - 100 + 256, "{}", plan.view.x0);
        assert_eq!(plan.view.x1, big + 2);
        // a source at the other end of i64 space is a miss, not an error
        assert!(source_plan_request(&sv, &request, &p, &bbox(-big, 0, -big + 5, 1000)).unwrap().is_none());
        // the axis helpers themselves
        assert_eq!(clip_low(5.0, 7, "t").unwrap(), Some(7));
        assert_eq!(clip_low(9.0, 7, "t").unwrap(), Some(9));
        assert_eq!(clip_low(1e19, 7, "t").unwrap(), None);
        assert_eq!(clip_high(9.0, 7, "t").unwrap(), Some(7));
        assert_eq!(clip_high(5.0, 7, "t").unwrap(), Some(5));
        assert_eq!(clip_high(-1e19, 7, "t").unwrap(), None);
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

    fn bbox(x0: i64, y0: i64, x1: i64, y1: i64) -> BBox {
        BBox { x0, y0, x1, y1 }
    }

    #[test]
    fn subwindows_cover_the_placement_plus_slack() {
        // the window is computed in SOURCE units through the source
        // view, like the raster: a 4x placement at (100, 200) seen
        // through the deck view 100..1100 x 200..1200 is the source
        // view 0..250 x 0..250
        let view = RasterViewBox::new(0.0, 0.0, 250.0, 250.0).unwrap();
        // a source in the middle of a 100x100 frame: 30..40 px plus
        // 9 px of slack each side (device rows grow down)
        let w = subwindow(&bbox(75, 75, 100, 100), &view, 100, 100);
        assert_eq!(w, Window::Part(21, 51, 28, 28));
        // the whole frame when the source covers it
        assert_eq!(subwindow(&bbox(-1, -1, 500, 500), &view, 100, 100), Window::Part(0, 0, 100, 100));
        // outside the view: no window
        assert_eq!(subwindow(&bbox(500, 500, 750, 750), &view, 100, 100), Window::Outside);
        assert_eq!(subwindow(&BBox::EMPTY, &view, 100, 100), Window::Outside);
        // clamped at the frame edge
        assert_eq!(subwindow(&bbox(0, 0, 250, 12), &view, 100, 100), Window::Part(0, 86, 100, 14));
        // far outside: no saturating cast decides it
        assert_eq!(subwindow(&bbox(i64::MIN / 2, 0, i64::MIN / 4, 10), &view, 100, 100), Window::Outside);
    }

    #[test]
    fn subwindow_at_a_huge_deck_offset_follows_the_raster() {
        // Review 2026-09-09 (5th) P2-2: dx = 1e16 - 1234 at scale 0.001,
        // a 100 dbu wide source at 1235400.. seen through the deck view
        // 1e16-10 .. 1e16+10 on 2000 px (100 px per deck unit). The
        // deck-space window of the first version put the source at
        // 1e16 + 1.4 .. 1.5, which f64 rounds to 1e16 + 2 both -
        // device 1200 with a 9 px slack, while the raster (source
        // space: 1224000 .. 1244000 over 2000 px) draws it at
        // 1140..1150. The window must contain the raster's pixels.
        let p = placement(0.001, 1e16 - 1234.0, 1e16 - 1234.0);
        let deck_view = RasterViewBox::new(1e16 - 10.0, 1e16 - 10.0, 1e16 + 10.0, 1e16 + 10.0).unwrap();
        let sv = source_view(&deck_view, &p).unwrap();
        assert_eq!((sv.x0, sv.x1), (1224000.0, 1244000.0));
        let source = bbox(1235400, 1235400, 1235500, 1235500);
        let w = subwindow(&source, &sv, 2000, 2000);
        assert_eq!(w, Window::Part(1131, 841, 28, 28));
        // and the deck-space mapping the review reproduced does miss it
        let deck_bbox = transform_bbox(&source, &p);
        let px = (deck_bbox[0] - deck_view.x0) * 2000.0 / (deck_view.x1 - deck_view.x0);
        assert!(px >= 1190.0, "deck-space column {px} is off by the f64 ulp at 1e16");
    }

    #[test]
    fn pass_images_are_charged_to_the_batch() {
        assert_eq!(pass_image_bytes((0, 0, 100, 50), false), 20_000);
        assert_eq!(pass_image_bytes((3, 4, 100, 50), true), 40_000);
    }

    #[test]
    fn window_overlays_place_a_window_sized_pass() {
        let mut composite = vec![0u8; 4 * 4 * 4]; // 4x4 frame
        let pass = vec![9, 9, 9, 255].repeat(4);   // a 2x2 window pass
        overlay_window(&mut composite, 4, &pass, (1, 2, 2, 2));
        let px = |x: usize, y: usize| composite[(y * 4 + x) * 4];
        assert_eq!((px(1, 2), px(2, 3), px(0, 2), px(3, 3), px(1, 1)), (9, 9, 0, 0, 0));
        let mut under = vec![0u8; 64];
        let mut over = vec![0u8; 64];
        // 2x2 window pass: gray, white / nothing, gray
        let frames = vec![128, 128, 128, 255, 255, 255, 255, 255, 0, 0, 0, 0, 128, 128, 128, 255];
        split_frame_planes_window(&frames, 4, &mut under, &mut over, (2, 0, 2, 2));
        assert_eq!(&under[(0 * 4 + 2) * 4..(0 * 4 + 3) * 4], &[128, 128, 128, 255]);
        assert_eq!(&over[(0 * 4 + 3) * 4..(0 * 4 + 4) * 4], &[255, 255, 255, 255]);
        assert_eq!(&under[(1 * 4 + 3) * 4..(1 * 4 + 4) * 4], &[128, 128, 128, 255]);
        assert_eq!(&under[(1 * 4 + 0) * 4..(1 * 4 + 1) * 4], &[0, 0, 0, 0], "outside the window");
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
