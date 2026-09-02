use floe_ovm::BBox;
use floe_vfs::hier::WsKey;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::font::{normalized_chars, GlyphAtlas};
use crate::page_index::RecordSet;
use crate::repetition::{for_each_visible_offset, for_each_visible_offset_chunked};
use crate::transform::OrthoTransform;
use crate::{FrameScene, RenderCancellation, RenderStats, ViewBox};

const MAX_IMAGE_PIXELS: u64 = 268_435_456;
const MAX_WORKERS: u16 = 256;
pub const MAX_TILE_SIZE: u16 = 4096;
pub const DEFAULT_TILE_SIZE: u16 = 128;
const DEVICE_ONE: i128 = 1i128 << 32;
const DEVICE_HALF: i128 = DEVICE_ONE / 2;
const MAX_DEVICE_COORD: i128 = 1i128 << 96;
const MAX_LABEL_GLYPHS: usize = 262_144;

/// Exact viewport used for world-to-pixel mapping, in layout database units.
///
/// Planning still uses an integer [`ViewBox`].  Keeping this viewport separate
/// prevents half-DBU target boxes from being rounded before rasterization.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterViewBox {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl RasterViewBox {
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Result<Self, String> {
        let view = Self { x0, y0, x1, y1 };
        view.validate()?;
        Ok(view)
    }

    pub fn from_integer(view: ViewBox) -> Self {
        Self {
            x0: view.x0 as f64,
            y0: view.y0 as f64,
            x1: view.x1 as f64,
            y1: view.y1 as f64,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if !self.x0.is_finite()
            || !self.y0.is_finite()
            || !self.x1.is_finite()
            || !self.y1.is_finite()
        {
            return Err("raster view contains a non-finite coordinate".to_string());
        }
        if self.x0 > self.x1 || self.y0 > self.y1 {
            return Err(format!(
                "invalid raster view: expected x0<=x1 and y0<=y1, got {},{},{},{}",
                self.x0, self.y0, self.x1, self.y1
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeometryRasterRequest {
    pub view: RasterViewBox,
    pub width: u32,
    pub height: u32,
    pub background: [u8; 4],
    pub foreground: [u8; 4],
    pub workers: u16,
    /// Width and height of independently owned square image tiles.
    pub tile_size: u16,
}

impl GeometryRasterRequest {
    pub fn validate(&self) -> Result<(), String> {
        self.view.validate()?;
        if self.view.x0 == self.view.x1 || self.view.y0 == self.view.y1 {
            return Err("raster view must have positive width and height".to_string());
        }
        if self.width == 0 || self.height == 0 {
            return Err("image width and height must be positive".to_string());
        }
        if self.workers == 0 || self.workers > MAX_WORKERS {
            return Err(format!(
                "raster workers must be in 1..={}, got {}",
                MAX_WORKERS, self.workers
            ));
        }
        if self.tile_size == 0 || self.tile_size > MAX_TILE_SIZE {
            return Err(format!(
                "raster tile size must be in 1..={}, got {}",
                MAX_TILE_SIZE, self.tile_size
            ));
        }
        let pixels = self.width as u64 * self.height as u64;
        if pixels > MAX_IMAGE_PIXELS {
            return Err(format!(
                "image limit exceeded: {} pixels (max {})",
                pixels, MAX_IMAGE_PIXELS
            ));
        }
        Ok(())
    }
}

/// Device-anchored interior fill for one design paint plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerFill {
    /// Opaque fill on every covered pixel.
    Solid,
    /// Calibre-style 50% checkerboard shared by every design layer.
    Speckle,
    /// A KLayout-compatible 16x16 stipple. Source rows and bits are written
    /// top-to-bottom/left-to-right. KLayout phases the source row by the
    /// framebuffer height (`row + height - 1`); columns stay left-anchored.
    /// Bit 15 is source column 0.
    Pattern([u16; 16]),
    /// No interior fill; the one-device-pixel geometry outline remains.
    Clear,
}

/// One visible design paint plane. Slice order is paint order: later entries
/// overwrite earlier entries with opaque pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerStyle {
    pub layer_idx: u32,
    pub color: [u8; 4],
    pub fill: LayerFill,
    /// Device-pixel outline width, matching KLayout's supported viewer range.
    pub outline_width: u8,
}

/// Styled live-view request. `raster.foreground` is ignored; each paint plane
/// supplies its own color. Planning visibility must be kept consistent with
/// `layers` by the caller.
#[derive(Clone, Debug, PartialEq)]
pub struct StyledGeometryRasterRequest {
    pub raster: GeometryRasterRequest,
    pub layers: Vec<LayerStyle>,
    pub hierarchy_frames: bool,
    /// Convert design-layer colors to deterministic luminance. Structural
    /// white/gray hierarchy-frame colors are unchanged.
    pub mono: bool,
}

impl StyledGeometryRasterRequest {
    pub fn validate(&self) -> Result<(), String> {
        self.raster.validate()?;
        let mut seen = BTreeSet::new();
        for layer in &self.layers {
            if !(1..=8).contains(&layer.outline_width) {
                return Err(format!(
                    "styled layer {} outline width must be in 1..=8, got {}",
                    layer.layer_idx, layer.outline_width
                ));
            }
            if !seen.insert(layer.layer_idx) {
                return Err(format!("duplicate styled layer index: {}", layer.layer_idx));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl RgbaFrame {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn png_bytes(&self) -> Result<Vec<u8>, String> {
        crate::png::encode_rgba(self.width, self.height, &self.pixels)
    }

    pub fn write_png(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let bytes = self.png_bytes()?;
        std::fs::write(path.as_ref(), bytes)
            .map_err(|error| format!("write {}: {}", path.as_ref().display(), error))
    }
}

pub struct GeometryRasterReport {
    pub frame: RgbaFrame,
    pub stats: RenderStats,
    pub rect_record_tests: u64,
    pub rectangle_member_paints: u64,
    pub polygon_record_tests: u64,
    pub polygon_member_paints: u64,
    pub path_record_tests: u64,
    pub path_member_paints: u64,
    pub frame_record_tests: u64,
    pub frame_member_paints: u64,
    pub deferred_frame_tests: u64,
    pub label_tile_paints: u64,
    pub label_pixel_paints: u64,
    pub labels_truncated: bool,
    pub partial: bool,
}

pub fn render_geometry_occupancy(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
) -> Result<GeometryRasterReport, String> {
    request.validate()?;
    render_geometry(scene, request, RenderMode::Occupancy, None, false)
}

pub fn render_geometry_occupancy_cancellable(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    generation: u64,
    cancellation: &RenderCancellation,
) -> Result<GeometryRasterReport, String> {
    request.validate()?;
    render_geometry(
        scene,
        request,
        RenderMode::Occupancy,
        Some(RenderGuard {
            generation,
            cancellation,
        }),
        false,
    )
}

pub fn render_geometry_styled(
    scene: &FrameScene,
    request: &StyledGeometryRasterRequest,
) -> Result<GeometryRasterReport, String> {
    request.validate()?;
    render_geometry(scene, &request.raster, RenderMode::Styled(request), None, true)
}

/// Styled render with the work bin disabled — the per-tile walk
/// reference path (kill switch and the bin-equality oracle).
pub fn render_geometry_styled_unbinned(
    scene: &FrameScene,
    request: &StyledGeometryRasterRequest,
) -> Result<GeometryRasterReport, String> {
    request.validate()?;
    render_geometry(scene, &request.raster, RenderMode::Styled(request), None, false)
}

pub fn render_geometry_styled_cancellable(
    scene: &FrameScene,
    request: &StyledGeometryRasterRequest,
    generation: u64,
    cancellation: &RenderCancellation,
) -> Result<GeometryRasterReport, String> {
    request.validate()?;
    render_geometry(
        scene,
        &request.raster,
        RenderMode::Styled(request),
        Some(RenderGuard {
            generation,
            cancellation,
        }),
        true,
    )
}

/// `render_geometry_styled_cancellable` with the work bin disabled
/// (FLOE_RUST_WORK_BIN=off kill switch in the daemon).
pub fn render_geometry_styled_unbinned_cancellable(
    scene: &FrameScene,
    request: &StyledGeometryRasterRequest,
    generation: u64,
    cancellation: &RenderCancellation,
) -> Result<GeometryRasterReport, String> {
    request.validate()?;
    render_geometry(
        scene,
        &request.raster,
        RenderMode::Styled(request),
        Some(RenderGuard {
            generation,
            cancellation,
        }),
        false,
    )
}

#[derive(Clone, Copy)]
enum RenderMode<'a> {
    Occupancy,
    Styled(&'a StyledGeometryRasterRequest),
}

struct PreparedLabels {
    atlas: GlyphAtlas,
    rows: Vec<PreparedLabel>,
    /// Row indices grouped per selection, in row order. A field view
    /// carried 207k rows scanned once per (tile x plane) - ~78M
    /// selection tests per frame (2026-08-28); the walk now touches
    /// only its own plane's rows.
    by_layer: std::collections::HashMap<u32, Vec<u32>>,
    block_gray: Vec<u32>,
    block_white: Vec<u32>,
}

struct PreparedLabel {
    block: bool,
    white: bool,
    layer_idx: Option<u32>,
    rotation: u8,
    anchor: (f64, f64),
    glyphs: Vec<PreparedGlyph>,
    /// Half-open device-pixel bounds used to reject all other raster tiles.
    bbox: (i64, i64, i64, i64),
}

struct PreparedGlyph {
    ch: char,
    x: f64,
    y: f64,
}

impl PreparedLabels {
    fn build(
        scene: &FrameScene,
        request: &GeometryRasterRequest,
    ) -> Result<(Option<Self>, bool), String> {
        if scene.labels().is_empty() {
            return Ok((None, false));
        }

        // Keep a deterministic whole-label prefix. A pathological hierarchy
        // name must not discard the geometry frame or force the atlas to
        // allocate for text that will never be drawn.
        let mut total_glyphs = 0usize;
        let mut label_count = 0usize;
        for label in scene.labels() {
            let glyphs = label.text.chars().count();
            let Some(next_total) = total_glyphs.checked_add(glyphs) else {
                break;
            };
            if next_total > MAX_LABEL_GLYPHS {
                break;
            }
            total_glyphs = next_total;
            label_count += 1;
        }
        let labels_truncated = label_count != scene.labels().len();
        let labels = &scene.labels()[..label_count];
        if labels.is_empty() {
            return Ok((None, labels_truncated));
        }
        let atlas = GlyphAtlas::build(labels, scene.label_font_px())?;
        let view = request.view;
        let span_x = view.x1 - view.x0;
        let span_y = view.y1 - view.y0;
        let mut rows = Vec::with_capacity(labels.len());
        for label in labels {
            let chars: Vec<char> = normalized_chars(&label.text).collect();
            let mut advance = 0.0f64;
            let mut previous = None;
            for &ch in &chars {
                if let Some(left) = previous {
                    advance += f64::from(atlas.kern(left, ch));
                }
                advance += f64::from(atlas.glyph(ch).advance);
                previous = Some(ch);
            }
            let baseline = f64::from((atlas.ascent + atlas.descent) * 0.5);
            let mut pen = -advance * 0.5;
            let mut glyphs = Vec::with_capacity(chars.len());
            previous = None;
            for ch in chars {
                if let Some(left) = previous {
                    pen += f64::from(atlas.kern(left, ch));
                }
                let glyph = atlas.glyph(ch);
                glyphs.push(PreparedGlyph {
                    ch,
                    x: pen + f64::from(glyph.xmin),
                    y: baseline - f64::from(glyph.ymin) - glyph.height as f64,
                });
                pen += f64::from(glyph.advance);
                previous = Some(ch);
            }
            let anchor = (
                (label.x as f64 - view.x0) * request.width as f64 / span_x,
                (view.y1 - label.y as f64) * request.height as f64 / span_y,
            );
            let rotation = label.rotation & 3;
            let bbox = prepared_label_bbox(&atlas, &glyphs, anchor, rotation);
            rows.push(PreparedLabel {
                block: label.block,
                white: label.white,
                layer_idx: label.layer_idx,
                rotation,
                anchor,
                glyphs,
                bbox,
            });
        }
        let mut by_layer: std::collections::HashMap<u32, Vec<u32>> =
            std::collections::HashMap::new();
        let mut block_gray = Vec::new();
        let mut block_white = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let index = index as u32;
            if row.block {
                if row.white {
                    block_white.push(index);
                } else {
                    block_gray.push(index);
                }
            } else if let Some(layer_idx) = row.layer_idx {
                by_layer.entry(layer_idx).or_default().push(index);
            }
        }
        Ok((
            Some(Self {
                atlas,
                rows,
                by_layer,
                block_gray,
                block_white,
            }),
            labels_truncated,
        ))
    }
}

fn rotate_label_offset(x: f64, y: f64, rotation: u8) -> (f64, f64) {
    match rotation & 3 {
        0 => (x, y),
        // Database rotations are counter-clockwise in Y-up coordinates;
        // framebuffer rows point down.
        1 => (y, -x),
        2 => (-x, -y),
        3 => (-y, x),
        _ => unreachable!(),
    }
}

fn prepared_label_bbox(
    atlas: &GlyphAtlas,
    glyphs: &[PreparedGlyph],
    anchor: (f64, f64),
    rotation: u8,
) -> (i64, i64, i64, i64) {
    let mut bounds = None::<(f64, f64, f64, f64)>;
    for placed in glyphs {
        let glyph = atlas.glyph(placed.ch);
        if glyph.width == 0 || glyph.height == 0 {
            continue;
        }
        for (x, y) in [
            (placed.x, placed.y),
            (placed.x + glyph.width as f64, placed.y),
            (placed.x, placed.y + glyph.height as f64),
            (
                placed.x + glyph.width as f64,
                placed.y + glyph.height as f64,
            ),
        ] {
            let (dx, dy) = rotate_label_offset(x, y, rotation);
            let point = (anchor.0 + dx, anchor.1 + dy);
            bounds = Some(match bounds {
                Some((x0, y0, x1, y1)) => (
                    x0.min(point.0),
                    y0.min(point.1),
                    x1.max(point.0),
                    y1.max(point.1),
                ),
                None => (point.0, point.1, point.0, point.1),
            });
        }
    }
    bounds
        .map(|(x0, y0, x1, y1)| {
            (
                x0.floor() as i64,
                y0.floor() as i64,
                x1.ceil() as i64,
                y1.ceil() as i64,
            )
        })
        .unwrap_or((0, 0, 0, 0))
}

#[derive(Clone, Copy)]
struct RenderGuard<'a> {
    generation: u64,
    cancellation: &'a RenderCancellation,
}

impl RenderGuard<'_> {
    fn check(self) -> Result<(), String> {
        self.cancellation.check(self.generation)
    }
}

fn check_cancelled(guard: Option<RenderGuard<'_>>) -> Result<(), String> {
    if let Some(guard) = guard {
        guard.check()?;
    }
    Ok(())
}

fn check_member_cancelled(guard: Option<RenderGuard<'_>>, member: &mut u16) -> Result<(), String> {
    if *member == 0 {
        check_cancelled(guard)?;
    }
    *member = member.wrapping_add(1) & 1023;
    Ok(())
}

fn render_geometry(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    mode: RenderMode<'_>,
    guard: Option<RenderGuard<'_>>,
    work_bin: bool,
) -> Result<GeometryRasterReport, String> {
    check_cancelled(guard)?;
    let (prepared_labels, labels_truncated) = match mode {
        RenderMode::Occupancy => (None, false),
        RenderMode::Styled(_) => PreparedLabels::build(scene, request)?,
    };
    let tile_size = u32::from(request.tile_size);
    let tile_columns = request.width.div_ceil(tile_size);
    let tile_rows = request.height.div_ceil(tile_size);
    let tile_count_u64 = u64::from(tile_columns) * u64::from(tile_rows);
    let tile_count: usize = tile_count_u64
        .try_into()
        .map_err(|_| format!("raster tile count limit exceeded: {tile_count_u64}"))?;
    let worker_count = usize::from(request.workers).min(tile_count).max(1);
    let mut counters = RasterCounters::default();
    let mut stats = RenderStats {
        workers_used: worker_count.try_into().unwrap_or(u16::MAX),
        tiles: tile_count.try_into().unwrap_or(u32::MAX),
        ..RenderStats::default()
    };
    let started = Instant::now();
    // F2R-03b 2c: one collection walk replaces the per-tile x
    // per-plane hierarchy walks. Falls back to the walk (None) when
    // the item cap is exceeded; occupancy always walks.
    let bin = match mode {
        RenderMode::Styled(styled) if work_bin => {
            let stroke_pixels = styled
                .layers
                .iter()
                .map(|layer| layer.outline_width)
                .max()
                .unwrap_or(1);
            collect_work_bin(scene, request, styled, stroke_pixels, guard, &mut stats)?
        }
        _ => None,
    };
    stats.work_bin_items = bin.as_ref().map(|bin| bin.items).unwrap_or(0);
    let bin = bin.as_ref();
    let next_tile = AtomicUsize::new(0);
    let tiles = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let next_tile = &next_tile;
            let prepared_labels = prepared_labels.as_ref();
            handles.push(scope.spawn(move || {
                let mut outputs = Vec::new();
                loop {
                    let tile_index = next_tile.fetch_add(1, Ordering::Relaxed);
                    if tile_index >= tile_count {
                        break;
                    }
                    let tile_x = tile_index % tile_columns as usize;
                    let tile_y = tile_index / tile_columns as usize;
                    let col0 = tile_boundary(request.width, tile_x, tile_size);
                    let col1 = tile_boundary(request.width, tile_x + 1, tile_size);
                    let row0 = tile_boundary(request.height, tile_y, tile_size);
                    let row1 = tile_boundary(request.height, tile_y + 1, tile_size);
                    let tile_started = Instant::now();
                    let mut output = raster_tile(
                        scene,
                        request,
                        mode,
                        bin,
                        prepared_labels,
                        guard,
                        col0,
                        col1,
                        row0,
                        row1,
                    )?;
                    output.stats.raster_tile_max_us = tile_started
                        .elapsed()
                        .as_micros()
                        .try_into()
                        .unwrap_or(u64::MAX);
                    outputs.push(output);
                }
                Ok::<_, String>(outputs)
            }));
        }

        let mut tiles = Vec::with_capacity(tile_count);
        let mut worker_error = None;
        for handle in handles {
            match handle.join() {
                Ok(Ok(outputs)) => {
                    for output in outputs {
                        add_stats(&mut stats, &output.stats);
                        counters.add(&output.counters);
                        tiles.push(output.tile);
                    }
                }
                Ok(Err(error)) => {
                    worker_error.get_or_insert(error);
                }
                Err(_) => {
                    worker_error.get_or_insert_with(|| "raster worker panicked".to_string());
                }
            };
        }
        if let Some(error) = worker_error {
            return Err(error);
        }
        Ok(tiles)
    })?;
    check_cancelled(guard)?;
    let frame = assemble_tiles(request, tiles, tile_columns, tile_rows)?;
    check_cancelled(guard)?;
    stats.raster_us = started.elapsed().as_micros().try_into().unwrap_or(u64::MAX);
    Ok(GeometryRasterReport {
        frame,
        stats,
        rect_record_tests: counters.rect_records,
        rectangle_member_paints: counters.rectangle_members_drawn,
        polygon_record_tests: counters.polygon_records,
        polygon_member_paints: counters.polygon_members_drawn,
        path_record_tests: counters.path_records,
        path_member_paints: counters.path_members_drawn,
        frame_record_tests: counters.frame_records,
        frame_member_paints: counters.frame_members_drawn,
        deferred_frame_tests: counters.deferred_frame_records,
        label_tile_paints: counters.label_tiles_drawn,
        label_pixel_paints: counters.label_pixels_drawn,
        labels_truncated,
        partial: scene.is_partial(),
    })
}

#[derive(Clone)]
struct RasterBand {
    width: u32,
    height: u32,
    col0: u32,
    col1: u32,
    row0: u32,
    row1: u32,
    pixels: Vec<u8>,
}

impl RasterBand {
    #[cfg(test)]
    fn new(request: &GeometryRasterRequest, row0: u32, row1: u32) -> Result<Self, String> {
        Self::new_tile(request, 0, request.width, row0, row1)
    }

    fn new_tile(
        request: &GeometryRasterRequest,
        col0: u32,
        col1: u32,
        row0: u32,
        row1: u32,
    ) -> Result<Self, String> {
        if col0 >= col1 || col1 > request.width || row0 >= row1 || row1 > request.height {
            return Err("invalid raster tile bounds".to_string());
        }
        let byte_len = ((col1 - col0) as usize)
            .checked_mul((row1 - row0) as usize)
            .and_then(|value| value.checked_mul(4))
            .ok_or_else(|| "raster tile byte length overflow".to_string())?;
        let mut pixels = vec![0u8; byte_len];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&request.background);
        }
        Ok(Self {
            width: request.width,
            height: request.height,
            col0,
            col1,
            row0,
            row1,
            pixels,
        })
    }

    fn tile_width(&self) -> u32 {
        self.col1 - self.col0
    }
}

fn tile_boundary(length: u32, index: usize, tile_size: u32) -> u32 {
    (index as u64 * u64::from(tile_size)).min(u64::from(length)) as u32
}

struct RasterTileOutput {
    tile: RasterBand,
    stats: RenderStats,
    counters: RasterCounters,
}

/// F2R-03b 2c: frame-level work bin. One cancellable traversal per
/// round collects every visible page/wash/frame item in DFS order;
/// tile workers then serve their planes from the bin instead of
/// re-walking the hierarchy per tile x plane. Ancestor bbox culling
/// is a conservative superset filter and the per-plane item order is
/// the walk's DFS order, so the per-tile paint sequence - and every
/// output byte - matches the walk exactly. Styled mode only.
struct WorkBin {
    /// Per styled-plane items (page queries and washes), DFS order.
    planes: Vec<Vec<PlaneItem>>,
    /// Frame-carrying cell visits, DFS order; band-filtered at paint.
    frames: Vec<FrameItem>,
    items: u64,
    /// Soft cap while a trial expansion runs (§3.17): charging past it
    /// raises WORK_BIN_TRIAL_STOP so the edge rolls back and defers,
    /// never the whole-frame fallback.
    trial_limit: Option<u64>,
    /// Shared records for the deferred items' `edge` indices.
    deferred_edges: Vec<DeferredEdge>,
    /// Tile-side mini bins (trials off) defer directly on a fast-gate
    /// failure instead of measuring - their deferred items replay
    /// through the legacy per-plane walk.
    trials_enabled: bool,
    /// The collection's plane wiring, stored after the walk so tiles
    /// can re-run the same combined walk culled to their own view.
    query: Vec<u64>,
    plane_bits: Vec<Option<usize>>,
    plane_of: std::collections::HashMap<u32, usize>,
}

enum PlaneItem {
    /// One cell visit's decoded pages for this plane (field 2026-08-28:
    /// per-(visit, page) items blew the cap on flat 150k-instance
    /// fanouts; per-(visit, plane) keeps the count at the visit scale).
    /// The tile scans the cell's page list exactly like the walk does.
    Cell {
        world_bbox: BBox,
        transform: OrthoTransform,
        inverse: OrthoTransform,
        cell: WsKey,
    },
    Wash {
        world_bbox: BBox,
    },
    /// An instance left unexpanded (its measured expansion overran the
    /// item budget, §3.15/§3.17). The tile resolves it through the
    /// combined mini walk for its `edge`, falling back to the walk's
    /// own per-plane code when the mini itself overruns - pixels are
    /// unchanged either way. `bit` is this plane's 2b mask bit,
    /// mirroring the walk's per-plane descent gate.
    Deferred {
        transform: OrthoTransform,
        inverse: OrthoTransform,
        cell: WsKey,
        inst: usize,
        bit: Option<usize>,
        edge: u32,
    },
}

enum FrameItem {
    Cell {
        world_bbox: BBox,
        transform: OrthoTransform,
        inverse: OrthoTransform,
        cell: WsKey,
    },
    Deferred {
        transform: OrthoTransform,
        inverse: OrthoTransform,
        cell: WsKey,
        inst: usize,
        edge: u32,
    },
}

/// One deferred instance edge (§3.17): every deferred item of the edge
/// shares this record, and each tile runs ONE combined collection walk
/// per edge (culled to the tile view) instead of a hierarchy re-walk
/// per plane - the source of the depth-3 field view's 923k visits and
/// 60.8M edge gates.
struct DeferredEdge {
    transform: OrthoTransform,
    inverse: OrthoTransform,
    cell: WsKey,
    inst: usize,
}

/// Instances whose members x subtree item weight exceed this stay
/// unexpanded in the bin and are walked per tile instead — the guard
/// covers dense reps, deep multiplications, and their mix alike.
/// Sentinel error for the visible-member count pass: enumeration
/// stopped because the count already exceeds the expansion budget.
const WORK_BIN_COUNT_STOP: &str = "work-bin visible-member count stop";

/// True when the repetition places at most `limit` members inside the
/// view. Enumerates via the same visibility pruning the expansion (and
/// the deferred tile path) would use, stopping right past the limit so
/// a huge visible array costs O(limit), not O(members). Enumeration
/// errors conservatively defer - the tile path re-runs the same
/// enumeration and surfaces the real error.
fn visible_members_within(
    rep: &floe_oasis::doc::Rep,
    base_bbox: BBox,
    local_view: BBox,
    limit: u64,
) -> bool {
    let mut count = 0u64;
    let walk = for_each_visible_offset(rep, base_bbox, local_view, |_, _| {
        count += 1;
        if count > limit {
            return Err(WORK_BIN_COUNT_STOP.to_string());
        }
        Ok(())
    });
    match walk {
        Ok(_) => count <= limit,
        Err(_) => false,
    }
}

/// Item cap (~64MB of items); past it the round falls back to the
/// per-tile walk (`work_bin_items=0` telemetry), pixels unchanged.
const WORK_BIN_MAX_ITEMS: u64 = 768 * 1024;

/// Internal marker: the collection walk aborts through the normal
/// error channel when the cap is hit and the caller turns it into
/// the fallback instead of a render error.
const WORK_BIN_OVERFLOW: &str = "work-bin item cap exceeded";

/// Internal marker: a trial expansion ran past its soft item limit;
/// the caller rolls the bin back and defers that one edge (§3.17 —
/// static weights overcount nested full-member products, so the gate
/// measures the real expansion instead of predicting it).
const WORK_BIN_TRIAL_STOP: &str = "work-bin trial expansion stop";

/// Bin state to restore when a trial expansion overruns its limit.
struct WorkBinCheckpoint {
    items: u64,
    plane_lens: Vec<usize>,
    frames_len: usize,
    deferred_edges_len: usize,
}

impl WorkBin {
    fn empty(planes: usize, trials_enabled: bool) -> Self {
        WorkBin {
            planes: (0..planes).map(|_| Vec::new()).collect(),
            frames: Vec::new(),
            items: 0,
            trial_limit: None,
            deferred_edges: Vec::new(),
            trials_enabled,
            query: Vec::new(),
            plane_bits: Vec::new(),
            plane_of: std::collections::HashMap::new(),
        }
    }

    fn charge(&mut self) -> Result<(), String> {
        self.items += 1;
        if let Some(limit) = self.trial_limit {
            if self.items > limit {
                return Err(WORK_BIN_TRIAL_STOP.to_string());
            }
        }
        if self.items > WORK_BIN_MAX_ITEMS {
            return Err(WORK_BIN_OVERFLOW.to_string());
        }
        Ok(())
    }

    fn checkpoint(&self) -> WorkBinCheckpoint {
        WorkBinCheckpoint {
            items: self.items,
            plane_lens: self.planes.iter().map(Vec::len).collect(),
            frames_len: self.frames.len(),
            deferred_edges_len: self.deferred_edges.len(),
        }
    }

    fn rollback(&mut self, checkpoint: &WorkBinCheckpoint) {
        for (plane, &len) in self.planes.iter_mut().zip(&checkpoint.plane_lens) {
            plane.truncate(len);
        }
        self.frames.truncate(checkpoint.frames_len);
        self.deferred_edges.truncate(checkpoint.deferred_edges_len);
        self.items = checkpoint.items;
    }
}

/// §3.17 tile-side combined walk: ONE collection walk per (tile,
/// deferred edge), culled to the tile view, replaces the per-plane
/// hierarchy re-walks of the legacy deferred path. The mini's plane
/// lists replay at the edge's slot in each plane's DFS order, so the
/// paint sequence - and the pixels - are identical. An edge whose
/// tile-local expansion still overruns the item cap keeps the legacy
/// per-plane walk (mini = None), as do this mini's own deferrals.
fn build_deferred_minis(
    scene: &FrameScene,
    bin: &WorkBin,
    want_frames: bool,
    cull_view: BBox,
    guard: Option<RenderGuard<'_>>,
    stats: &mut RenderStats,
) -> Result<Vec<Option<WorkBin>>, String> {
    let mut minis = Vec::with_capacity(bin.deferred_edges.len());
    let mut path = Vec::new();
    let mut plane_scratch: Vec<(u64, BBox)> = vec![(0, BBox::EMPTY); bin.plane_bits.len()];
    let mut visit_seq = 0u64;
    for edge in &bin.deferred_edges {
        let parent = scene.cell(edge.cell).ok_or_else(|| {
            format!("internal error: binned cell {:?} left the scene", edge.cell)
        })?;
        let instance = parent.insts.get(edge.inst).ok_or_else(|| {
            format!("internal error: binned instance {} left the scene", edge.inst)
        })?;
        let child_bbox = scene.cell_bbox(instance.child).ok_or_else(|| {
            format!("invalid plan: missing bbox for child {:?}", instance.child)
        })?;
        let base_place =
            OrthoTransform::place(instance.x, instance.y, instance.rot, instance.flip)?;
        let base_bbox = base_place.apply_bbox(child_bbox)?;
        let local_view = edge.inverse.apply_bbox(cull_view)?;
        let mut mini = WorkBin::empty(bin.plane_bits.len(), false);
        let mut cancel_member = 0u16;
        let walk = for_each_visible_offset(&instance.rep, base_bbox, local_view, |ox, oy| {
            check_member_cancelled(guard, &mut cancel_member)?;
            let x = checked_add(instance.x, ox, "instance x")?;
            let y = checked_add(instance.y, oy, "instance y")?;
            let local = OrthoTransform::place(x, y, instance.rot, instance.flip)?;
            let child_world = edge.transform.compose(&local)?;
            collect_cell(
                scene,
                &mut mini,
                &bin.plane_of,
                &bin.query,
                &bin.plane_bits,
                want_frames,
                cull_view,
                guard,
                instance.child,
                child_world,
                &mut path,
                &mut plane_scratch,
                &mut visit_seq,
                stats,
            )
        });
        match walk {
            Ok(_) => minis.push(Some(mini)),
            Err(error) if error == WORK_BIN_OVERFLOW => {
                path.clear();
                minis.push(None);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(minis)
}

fn collect_work_bin(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    styled: &StyledGeometryRasterRequest,
    stroke_pixels: u8,
    guard: Option<RenderGuard<'_>>,
    stats: &mut RenderStats,
) -> Result<Option<WorkBin>, String> {
    let cull_view = tile_world_view(request, 0, request.width, 0, request.height, stroke_pixels)?;
    let mut plane_of = std::collections::HashMap::new();
    for (plane, layer) in styled.layers.iter().enumerate() {
        plane_of.insert(layer.layer_idx, plane);
    }
    let layer_indices: Vec<u32> = styled.layers.iter().map(|layer| layer.layer_idx).collect();
    let query = scene.layer_query_words(&layer_indices);
    let plane_bits: Vec<Option<usize>> = styled
        .layers
        .iter()
        .map(|layer| scene.layer_mask_bit(layer.layer_idx))
        .collect();
    let mut bin = WorkBin::empty(styled.layers.len(), true);
    let mut path = Vec::new();
    // Stamped per-plane scratch: one row per plane, valid only while
    // its stamp equals the current visit - avoids a per-visit alloc
    // across (measured) 100k+ visits.
    let mut plane_scratch: Vec<(u64, BBox)> = vec![(0, BBox::EMPTY); styled.layers.len()];
    let mut visit_seq = 0u64;
    let walk = collect_cell(
        scene,
        &mut bin,
        &plane_of,
        &query,
        &plane_bits,
        styled.hierarchy_frames,
        cull_view,
        guard,
        scene.top(),
        OrthoTransform::identity(),
        &mut path,
        &mut plane_scratch,
        &mut visit_seq,
        stats,
    );
    match walk {
        Ok(()) => {
            bin.query = query;
            bin.plane_bits = plane_bits;
            bin.plane_of = plane_of;
            Ok(Some(bin))
        }
        Err(error) if error == WORK_BIN_OVERFLOW => {
            stats.work_bin_overflow_items = bin.items;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_cell(
    scene: &FrameScene,
    bin: &mut WorkBin,
    plane_of: &std::collections::HashMap<u32, usize>,
    query: &[u64],
    plane_bits: &[Option<usize>],
    want_frames: bool,
    cull_view: BBox,
    guard: Option<RenderGuard<'_>>,
    key: WsKey,
    world_transform: OrthoTransform,
    path: &mut Vec<WsKey>,
    plane_scratch: &mut Vec<(u64, BBox)>,
    visit_seq: &mut u64,
    stats: &mut RenderStats,
) -> Result<(), String> {
    check_cancelled(guard)?;
    if path.contains(&key) {
        return Err(format!("invalid plan: hierarchy cycle at {:?}", key));
    }
    let cell = scene
        .cell(key)
        .ok_or_else(|| format!("invalid plan: missing working cell {:?}", key))?;
    stats.hier_cells_visited = stats.hier_cells_visited.saturating_add(1);
    path.push(key);
    let inverse = world_transform.invert()?;
    let local_view = inverse.apply_bbox(cull_view)?;

    // One item per (visit, plane): union the plane's page bboxes for
    // the tile filter; the tile re-runs the page scan itself.
    *visit_seq += 1;
    let stamp = *visit_seq;
    for &page_id in &cell.pages {
        let Some(page) = scene.page(page_id) else {
            continue;
        };
        let Some(&plane) = plane_of.get(&page.layer_idx) else {
            continue;
        };
        if !page.bbox.intersects(&local_view) {
            continue;
        }
        let entry = &mut plane_scratch[plane];
        if entry.0 == stamp {
            entry.1.grow(&page.bbox);
        } else {
            *entry = (stamp, page.bbox);
        }
    }
    for (plane, entry) in plane_scratch.iter().enumerate() {
        if entry.0 != stamp {
            continue;
        }
        bin.charge()?;
        let world_bbox = world_transform.apply_bbox(entry.1)?;
        bin.planes[plane].push(PlaneItem::Cell {
            world_bbox,
            transform: world_transform,
            inverse,
            cell: key,
        });
    }
    for &(layer_idx, wash) in &cell.washes {
        let Some(&plane) = plane_of.get(&layer_idx) else {
            continue;
        };
        let world_bbox = world_transform.apply_bbox(wash)?;
        if !world_bbox.intersects(&cull_view) {
            continue;
        }
        bin.charge()?;
        bin.planes[plane].push(PlaneItem::Wash { world_bbox });
    }
    if want_frames && !cell.frames.is_empty() {
        // The walk reaches a non-top cell only when its bbox meets the
        // view, so the item filter mirrors that; the top cell is
        // always visited and gets the whole-frame bbox.
        let world_bbox = if path.len() == 1 {
            cull_view
        } else {
            let cell_bbox = scene
                .cell_bbox(key)
                .ok_or_else(|| format!("invalid scene: bbox for working cell {:?} is missing", key))?;
            world_transform.apply_bbox(cell_bbox)?
        };
        bin.charge()?;
        bin.frames.push(FrameItem::Cell {
            world_bbox,
            transform: world_transform,
            inverse,
            cell: key,
        });
    }

    for (inst_index, instance) in cell.insts.iter().enumerate() {
        check_cancelled(guard)?;
        let child_bbox = scene
            .cell_bbox(instance.child)
            .ok_or_else(|| format!("invalid plan: missing bbox for child {:?}", instance.child))?;
        if child_bbox.is_empty() {
            continue;
        }
        // Combined 2b gate: one pass serves every plane, so a child is
        // pruned only when its subtree holds NONE of the styled layers
        // and (when frames are on) no hierarchy frames.
        if !scene.subtree_intersects(instance.child, query, want_frames) {
            stats.subtrees_pruned = stats.subtrees_pruned.saturating_add(1);
            continue;
        }
        let members = instance.rep.members();
        let weight = scene.subtree_item_weight(instance.child);
        let base_place =
            OrthoTransform::place(instance.x, instance.y, instance.rot, instance.flip)?;
        let base_bbox = base_place.apply_bbox(child_bbox)?;
        // §3.17 deferral gate, third iteration: expansion is walked
        // once here versus once per tile x plane when deferred (the
        // depth-3 field view re-walked one deferred array at 9x the
        // cover, 60.8M edge gates). The static projection - visible
        // members x subtree weight - overcounts nested repetitions
        // (child weights multiply FULL member counts while the walk
        // culls by view), so a fast-gate failure does not defer: it
        // runs the REAL expansion against a soft item limit (half the
        // remaining cap, always below the whole-frame fallback cap)
        // and only an edge that truly overruns rolls back and defers.
        // Deterministic in DFS order and collection is single-threaded,
        // so jobs/tile counts cannot change the outcome, and either
        // outcome paints identical pixels.
        let budget = WORK_BIN_MAX_ITEMS.saturating_sub(bin.items) / 4;
        let member_limit = budget / weight.max(1);
        let fast_expand = weight <= budget
            && (members <= member_limit
                || visible_members_within(&instance.rep, base_bbox, local_view, member_limit));
        let mut trial: Option<(WorkBinCheckpoint, usize, Option<u64>)> = None;
        let mut deferred = false;
        if !fast_expand {
            if !bin.trials_enabled {
                // Tile-side mini collection: deferral there falls back
                // to the legacy per-plane walk, so measuring is not
                // worth the doomed-walk cost - defer directly.
                deferred = true;
            } else {
                // Half the remaining cap: with the tile-side combined
                // mini walk (§3.17), a deferred edge is no longer a
                // per-plane re-walk disaster, so the trial stays short
                // rather than chasing edges that barely fit (the field
                // edge measured past even 7/8 of the cap).
                let headroom = WORK_BIN_MAX_ITEMS.saturating_sub(bin.items) / 2;
                // subtree_intersects guaranteed queried content below
                // this edge, so every visible member emits at least one
                // item: more visible members than the soft cap is a
                // certain overrun - defer without a doomed trial walk.
                if members > headroom
                    && !visible_members_within(&instance.rep, base_bbox, local_view, headroom)
                {
                    deferred = true;
                } else {
                    let limit = bin
                        .items
                        .saturating_add(headroom)
                        .min(bin.trial_limit.unwrap_or(u64::MAX));
                    trial =
                        Some((bin.checkpoint(), path.len(), bin.trial_limit.replace(limit)));
                }
            }
        }
        if !deferred {
            let mut cancel_member = 0u16;
            let attempt = for_each_visible_offset(
                &instance.rep,
                base_bbox,
                local_view,
                |offset_x, offset_y| {
                    check_member_cancelled(guard, &mut cancel_member)?;
                    let x = checked_add(instance.x, offset_x, "instance x")?;
                    let y = checked_add(instance.y, offset_y, "instance y")?;
                    let local = OrthoTransform::place(x, y, instance.rot, instance.flip)?;
                    let child_world = world_transform.compose(&local)?;
                    collect_cell(
                        scene,
                        bin,
                        plane_of,
                        query,
                        plane_bits,
                        want_frames,
                        cull_view,
                        guard,
                        instance.child,
                        child_world,
                        path,
                        plane_scratch,
                        visit_seq,
                        stats,
                    )
                },
            );
            if let Some((checkpoint, path_len, outer_limit)) = trial {
                bin.trial_limit = outer_limit;
                if matches!(&attempt, Err(error) if error.as_str() == WORK_BIN_TRIAL_STOP) {
                    // The edge measured past its soft limit: restore
                    // the bin and the DFS path exactly and defer it.
                    bin.rollback(&checkpoint);
                    path.truncate(path_len);
                    deferred = true;
                }
            }
            if !deferred {
                let visit = attempt?;
                stats.rep_members_tested =
                    stats.rep_members_tested.saturating_add(visit.tested);
                continue;
            }
        }
        if members > 1 {
            stats.work_bin_defer_rep = stats.work_bin_defer_rep.saturating_add(1);
        } else {
            stats.work_bin_defer_single = stats.work_bin_defer_single.saturating_add(1);
        }
        stats.work_bin_defer_weight_max = stats.work_bin_defer_weight_max.max(weight);
        let edge = u32::try_from(bin.deferred_edges.len())
            .map_err(|_| "work-bin deferred edge count overflow".to_string())?;
        bin.deferred_edges.push(DeferredEdge {
            transform: world_transform,
            inverse,
            cell: key,
            inst: inst_index,
        });
        // One deferred item per plane the subtree can actually paint
        // (the walk's own per-plane descent gate), plus one for the
        // band walks.
        for (plane, bit) in plane_bits.iter().enumerate() {
            if scene.subtree_paints(instance.child, *bit) {
                bin.charge()?;
                bin.planes[plane].push(PlaneItem::Deferred {
                    transform: world_transform,
                    inverse,
                    cell: key,
                    inst: inst_index,
                    bit: *bit,
                    edge,
                });
            }
        }
        if want_frames && scene.subtree_has_frames(instance.child) {
            bin.charge()?;
            bin.frames.push(FrameItem::Deferred {
                transform: world_transform,
                inverse,
                cell: key,
                inst: inst_index,
                edge,
            });
        }
    }
    path.pop();
    Ok(())
}

/// Serves one tile from the bin: same plane order, same per-plane DFS
/// item order, same record queries as the walk.
#[allow(clippy::too_many_arguments)]
fn raster_tile_from_bin(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    styled: &StyledGeometryRasterRequest,
    bin: &WorkBin,
    labels: Option<&PreparedLabels>,
    band: &mut RasterBand,
    cull_view: BBox,
    stats: &mut RenderStats,
    counters: &mut RasterCounters,
    guard: Option<RenderGuard<'_>>,
    record_scratch: &mut RecordSet,
) -> Result<(), String> {
    // §3.17: resolve every deferred edge once for this tile before the
    // band/plane sequence consumes it from all sides.
    let minis = if bin.deferred_edges.is_empty() {
        Vec::new()
    } else {
        build_deferred_minis(
            scene,
            bin,
            styled.hierarchy_frames,
            cull_view,
            guard,
            stats,
        )?
    };
    let walk_frames = styled.hierarchy_frames && !bin.frames.is_empty();
    if styled.hierarchy_frames {
        if walk_frames {
            for frame_band in [2, 3, 1] {
                check_cancelled(guard)?;
                replay_frame_items(
                    scene,
                    request,
                    band,
                    cull_view,
                    frame_band,
                    frame_paint(frame_band),
                    stats,
                    counters,
                    guard,
                    &bin.frames,
                    Some(&minis),
                )?;
            }
        }
        render_prepared_labels(
            labels,
            band,
            LabelSelection::Block { white: false },
            [128, 128, 128, 255],
            counters,
            guard,
        )?;
    }
    for (plane, layer) in styled.layers.iter().enumerate() {
        check_cancelled(guard)?;
        let color = if styled.mono {
            monochrome(layer.color)
        } else {
            layer.color
        };
        let paint = PaintStyle {
            color,
            fill: layer.fill,
            stroke: StrokeStyle::Solid,
            stroke_width: layer.outline_width,
        };
        replay_plane_items(
            scene,
            request,
            band,
            cull_view,
            stats,
            counters,
            guard,
            record_scratch,
            layer,
            plane,
            paint,
            &bin.planes[plane],
            Some(&minis),
        )?;
        render_prepared_labels(
            labels,
            band,
            LabelSelection::Layer(layer.layer_idx),
            color,
            counters,
            guard,
        )?;
    }
    if styled.hierarchy_frames {
        check_cancelled(guard)?;
        if walk_frames {
            replay_frame_items(
                scene,
                request,
                band,
                cull_view,
                0,
                frame_paint(0),
                stats,
                counters,
                guard,
                &bin.frames,
                Some(&minis),
            )?;
        }
        render_prepared_labels(
            labels,
            band,
            LabelSelection::Block { white: true },
            [255, 255, 255, 255],
            counters,
            guard,
        )?;
    }
    Ok(())
}

/// Consumes one plane-item sequence: the bin's own list, or a mini
/// bin's list replayed at a deferred edge's slot (minis = None there,
/// so a mini's own deferrals take the legacy per-plane walk). Order is
/// DFS in both cases, so the paint sequence matches the walk.
#[allow(clippy::too_many_arguments)]
fn replay_plane_items(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    band: &mut RasterBand,
    cull_view: BBox,
    stats: &mut RenderStats,
    counters: &mut RasterCounters,
    guard: Option<RenderGuard<'_>>,
    record_scratch: &mut RecordSet,
    layer: &LayerStyle,
    plane: usize,
    paint: PaintStyle,
    items: &[PlaneItem],
    minis: Option<&[Option<WorkBin>]>,
) -> Result<(), String> {
    for item in items {
        match item {
            PlaneItem::Cell {
                world_bbox,
                transform,
                inverse,
                cell,
            } => {
                if !world_bbox.intersects(&cull_view) {
                    continue;
                }
                let visited = scene.cell(*cell).ok_or_else(|| {
                    format!("internal error: binned cell {:?} left the scene", cell)
                })?;
                let local_view = inverse.apply_bbox(cull_view)?;
                for &page_id in &visited.pages {
                    let Some(page) = scene.page(page_id) else {
                        continue;
                    };
                    if page.layer_idx != layer.layer_idx || !page.bbox.intersects(&local_view)
                    {
                        continue;
                    }
                    raster_page_records(
                        band,
                        request,
                        page,
                        page_id,
                        local_view,
                        *transform,
                        stats,
                        counters,
                        paint,
                        guard,
                        record_scratch,
                    )?;
                }
            }
            PlaneItem::Wash { world_bbox } => {
                if !world_bbox.intersects(&cull_view) {
                    continue;
                }
                counters.rect_records = counters.rect_records.saturating_add(1);
                stats.primitives_tested = stats.primitives_tested.saturating_add(1);
                stats.rep_members_tested = stats.rep_members_tested.saturating_add(1);
                if paint_world_rect(band, request, *world_bbox, paint)? {
                    counters.rectangle_members_drawn =
                        counters.rectangle_members_drawn.saturating_add(1);
                    stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(1);
                    stats.primitives_drawn = stats.primitives_drawn.saturating_add(1);
                }
            }
            PlaneItem::Deferred {
                transform,
                inverse,
                cell,
                inst,
                bit,
                edge,
            } => {
                if let Some(minis) = minis {
                    if let Some(Some(mini)) = minis.get(*edge as usize) {
                        replay_plane_items(
                            scene,
                            request,
                            band,
                            cull_view,
                            stats,
                            counters,
                            guard,
                            record_scratch,
                            layer,
                            plane,
                            paint,
                            &mini.planes[plane],
                            None,
                        )?;
                        continue;
                    }
                }
                let parent = scene.cell(*cell).ok_or_else(|| {
                    format!("internal error: binned cell {:?} left the scene", cell)
                })?;
                let instance = parent.insts.get(*inst).ok_or_else(|| {
                    format!("internal error: binned instance {inst} left the scene")
                })?;
                let child_bbox = scene.cell_bbox(instance.child).ok_or_else(|| {
                    format!("invalid plan: missing bbox for child {:?}", instance.child)
                })?;
                let base_place =
                    OrthoTransform::place(instance.x, instance.y, instance.rot, instance.flip)?;
                let base_bbox = base_place.apply_bbox(child_bbox)?;
                let local_view = inverse.apply_bbox(cull_view)?;
                let mut deferred_path = Vec::new();
                let mut cancel_member = 0u16;
                let visit = for_each_visible_offset(
                    &instance.rep,
                    base_bbox,
                    local_view,
                    |offset_x, offset_y| {
                        check_member_cancelled(guard, &mut cancel_member)?;
                        let x = checked_add(instance.x, offset_x, "instance x")?;
                        let y = checked_add(instance.y, offset_y, "instance y")?;
                        let local = OrthoTransform::place(x, y, instance.rot, instance.flip)?;
                        let child_world = transform.compose(&local)?;
                        render_cell(
                            scene,
                            request,
                            band,
                            cull_view,
                            stats,
                            counters,
                            GeometrySelection::Layer(layer.layer_idx),
                            SubtreePrune::Layer(*bit),
                            paint,
                            guard,
                            instance.child,
                            child_world,
                            &mut deferred_path,
                            record_scratch,
                        )
                    },
                )?;
                stats.rep_members_tested =
                    stats.rep_members_tested.saturating_add(visit.tested);
            }
        }
    }
    Ok(())
}

/// Consumes one frame-item sequence: the bin's own list, or a mini
/// bin's list replayed at a deferred edge's slot (minis = None there,
/// so a mini's own deferrals take the legacy band walk).
#[allow(clippy::too_many_arguments)]
fn replay_frame_items(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    band: &mut RasterBand,
    cull_view: BBox,
    selected_band: u8,
    paint: PaintStyle,
    stats: &mut RenderStats,
    counters: &mut RasterCounters,
    guard: Option<RenderGuard<'_>>,
    items: &[FrameItem],
    minis: Option<&[Option<WorkBin>]>,
) -> Result<(), String> {
    for item in items {
        match item {
            FrameItem::Cell {
                world_bbox,
                transform,
                inverse,
                cell,
            } => {
                if !world_bbox.intersects(&cull_view) {
                    continue;
                }
                let cell = scene.cell(*cell).ok_or_else(|| {
                    format!("internal error: binned cell {:?} left the scene", cell)
                })?;
                let local_view = inverse.apply_bbox(cull_view)?;
                raster_cell_frames(
                    band,
                    request,
                    cell,
                    selected_band,
                    local_view,
                    *transform,
                    stats,
                    counters,
                    paint,
                    guard,
                )?;
            }
            FrameItem::Deferred {
                transform,
                inverse,
                cell,
                inst,
                edge,
            } => {
                if let Some(minis) = minis {
                    if let Some(Some(mini)) = minis.get(*edge as usize) {
                        replay_frame_items(
                            scene,
                            request,
                            band,
                            cull_view,
                            selected_band,
                            paint,
                            stats,
                            counters,
                            guard,
                            &mini.frames,
                            None,
                        )?;
                        continue;
                    }
                }
                let parent = scene.cell(*cell).ok_or_else(|| {
                    format!("internal error: binned cell {:?} left the scene", cell)
                })?;
                let instance = parent.insts.get(*inst).ok_or_else(|| {
                    format!("internal error: binned instance {inst} left the scene")
                })?;
                let child_bbox = scene.cell_bbox(instance.child).ok_or_else(|| {
                    format!("invalid plan: missing bbox for child {:?}", instance.child)
                })?;
                let base_place = OrthoTransform::place(
                    instance.x,
                    instance.y,
                    instance.rot,
                    instance.flip,
                )?;
                let base_bbox = base_place.apply_bbox(child_bbox)?;
                let local_view = inverse.apply_bbox(cull_view)?;
                let mut deferred_path = Vec::new();
                let mut cancel_member = 0u16;
                let visit = for_each_visible_offset(
                    &instance.rep,
                    base_bbox,
                    local_view,
                    |offset_x, offset_y| {
                        check_member_cancelled(guard, &mut cancel_member)?;
                        let x = checked_add(instance.x, offset_x, "instance x")?;
                        let y = checked_add(instance.y, offset_y, "instance y")?;
                        let local = OrthoTransform::place(x, y, instance.rot, instance.flip)?;
                        let child_world = transform.compose(&local)?;
                        render_frame_band(
                            scene,
                            request,
                            band,
                            cull_view,
                            stats,
                            counters,
                            selected_band,
                            paint,
                            guard,
                            instance.child,
                            child_world,
                            &mut deferred_path,
                        )
                    },
                )?;
                stats.rep_members_tested = stats.rep_members_tested.saturating_add(visit.tested);
            }
        }
    }
    Ok(())
}


/// The pre-2c styled tile path: per-plane hierarchy walks. Kept
/// verbatim as the work-bin fallback and byte-equality reference.
#[allow(clippy::too_many_arguments)]
fn raster_tile_walk_styled(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    styled: &StyledGeometryRasterRequest,
    labels: Option<&PreparedLabels>,
    band: &mut RasterBand,
    cull_view: BBox,
    stats: &mut RenderStats,
    counters: &mut RasterCounters,
    guard: Option<RenderGuard<'_>>,
    path: &mut Vec<WsKey>,
    record_scratch: &mut RecordSet,
) -> Result<(), String> {
    // The masks are subtree-cumulative, so a frame-free plan
    // skips all four band walks in one test (labels still run).
    let walk_frames = styled.hierarchy_frames && scene.subtree_has_frames(scene.top());
    if styled.hierarchy_frames {
        if walk_frames {
            for frame_band in [2, 3, 1] {
                check_cancelled(guard)?;
                render_frame_band(
                    scene,
                    request,
                    band,
                    cull_view,
                    stats,
                    counters,
                    frame_band,
                    frame_paint(frame_band),
                    guard,
                    scene.top(),
                    OrthoTransform::identity(),
                    path,
                )?;
            }
        }
        render_prepared_labels(
            labels,
            band,
            LabelSelection::Block { white: false },
            [128, 128, 128, 255],
            counters,
            guard,
        )?;
    }
    for layer in &styled.layers {
        check_cancelled(guard)?;
        render_cell(
            scene,
            request,
            band,
            cull_view,
            stats,
            counters,
            GeometrySelection::Layer(layer.layer_idx),
            SubtreePrune::Layer(scene.layer_mask_bit(layer.layer_idx)),
            PaintStyle {
                color: if styled.mono {
                    monochrome(layer.color)
                } else {
                    layer.color
                },
                fill: layer.fill,
                stroke: StrokeStyle::Solid,
                stroke_width: layer.outline_width,
            },
            guard,
            scene.top(),
            OrthoTransform::identity(),
            path,
            record_scratch,
        )?;
        render_prepared_labels(
            labels,
            band,
            LabelSelection::Layer(layer.layer_idx),
            if styled.mono {
                monochrome(layer.color)
            } else {
                layer.color
            },
            counters,
            guard,
        )?;
    }
    if styled.hierarchy_frames {
        check_cancelled(guard)?;
        if walk_frames {
            render_frame_band(
                scene,
                request,
                band,
                cull_view,
                stats,
                counters,
                0,
                frame_paint(0),
                guard,
                scene.top(),
                OrthoTransform::identity(),
                path,
            )?;
        }
        render_prepared_labels(
            labels,
            band,
            LabelSelection::Block { white: true },
            [255, 255, 255, 255],
            counters,
            guard,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn raster_tile(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    mode: RenderMode<'_>,
    bin: Option<&WorkBin>,
    labels: Option<&PreparedLabels>,
    guard: Option<RenderGuard<'_>>,
    col0: u32,
    col1: u32,
    row0: u32,
    row1: u32,
) -> Result<RasterTileOutput, String> {
    check_cancelled(guard)?;
    let mut band = RasterBand::new_tile(request, col0, col1, row0, row1)?;
    let stroke_pixels = match mode {
        RenderMode::Occupancy => 1,
        RenderMode::Styled(styled) => styled
            .layers
            .iter()
            .map(|layer| layer.outline_width)
            .max()
            .unwrap_or(1),
    };
    let cull_view = tile_world_view(request, col0, col1, row0, row1, stroke_pixels)?;
    let mut stats = RenderStats::default();
    let mut counters = RasterCounters::default();
    let mut path = Vec::new();
    let mut record_scratch = RecordSet::default();
    match mode {
        RenderMode::Occupancy => {
            render_cell(
                scene,
                request,
                &mut band,
                cull_view,
                &mut stats,
                &mut counters,
                GeometrySelection::All,
                SubtreePrune::Off,
                PaintStyle::solid(request.foreground),
                guard,
                scene.top(),
                OrthoTransform::identity(),
                &mut path,
                &mut record_scratch,
            )?;
        }
        RenderMode::Styled(styled) => {
            if let Some(bin) = bin {
                raster_tile_from_bin(
                    scene,
                    request,
                    styled,
                    bin,
                    labels,
                    &mut band,
                    cull_view,
                    &mut stats,
                    &mut counters,
                    guard,
                    &mut record_scratch,
                )?;
            } else {
                raster_tile_walk_styled(
                    scene,
                    request,
                    styled,
                    labels,
                    &mut band,
                    cull_view,
                    &mut stats,
                    &mut counters,
                    guard,
                    &mut path,
                    &mut record_scratch,
                )?;
            }
        }
    }
    check_cancelled(guard)?;
    Ok(RasterTileOutput {
        tile: band,
        stats,
        counters,
    })
}

#[derive(Clone, Copy)]
enum LabelSelection {
    Block { white: bool },
    Layer(u32),
}

impl LabelSelection {
    fn includes(self, label: &PreparedLabel) -> bool {
        match self {
            Self::Block { white } => label.block && label.white == white,
            Self::Layer(layer_idx) => !label.block && label.layer_idx == Some(layer_idx),
        }
    }
}

fn render_prepared_labels(
    labels: Option<&PreparedLabels>,
    band: &mut RasterBand,
    selection: LabelSelection,
    color: [u8; 4],
    counters: &mut RasterCounters,
    guard: Option<RenderGuard<'_>>,
) -> Result<(), String> {
    let Some(labels) = labels else {
        return Ok(());
    };
    let tile = (
        i64::from(band.col0),
        i64::from(band.row0),
        i64::from(band.col1),
        i64::from(band.row1),
    );
    static EMPTY: Vec<u32> = Vec::new();
    let group = match selection {
        LabelSelection::Block { white: true } => &labels.block_white,
        LabelSelection::Block { white: false } => &labels.block_gray,
        LabelSelection::Layer(layer_idx) => {
            labels.by_layer.get(&layer_idx).unwrap_or(&EMPTY)
        }
    };
    let mut cancel_member = 0u16;
    for &row in group {
        check_member_cancelled(guard, &mut cancel_member)?;
        let label = &labels.rows[row as usize];
        debug_assert!(selection.includes(label));
        if label.bbox.2 <= tile.0
            || label.bbox.0 >= tile.2
            || label.bbox.3 <= tile.1
            || label.bbox.1 >= tile.3
        {
            continue;
        }
        let mut label_drew = false;
        for placed in &label.glyphs {
            let glyph = labels.atlas.glyph(placed.ch);
            for glyph_row in 0..glyph.height {
                for glyph_col in 0..glyph.width {
                    let alpha = glyph.alpha[glyph_row * glyph.width + glyph_col];
                    if alpha == 0 {
                        continue;
                    }
                    let (dx, dy) = rotate_label_offset(
                        placed.x + glyph_col as f64 + 0.5,
                        placed.y + glyph_row as f64 + 0.5,
                        label.rotation,
                    );
                    let col = (label.anchor.0 + dx).floor() as i64;
                    let row = (label.anchor.1 + dy).floor() as i64;
                    if col < tile.0 || col >= tile.2 || row < tile.1 || row >= tile.3 {
                        continue;
                    }
                    let local_col = col as usize - band.col0 as usize;
                    let local_row = row as usize - band.row0 as usize;
                    let offset = (local_row * band.tile_width() as usize + local_col) * 4;
                    blend_text_pixel(&mut band.pixels[offset..offset + 4], color, alpha);
                    counters.label_pixels_drawn = counters.label_pixels_drawn.saturating_add(1);
                    label_drew = true;
                }
            }
        }
        if label_drew {
            counters.label_tiles_drawn = counters.label_tiles_drawn.saturating_add(1);
        }
    }
    Ok(())
}

fn blend_text_pixel(target: &mut [u8], color: [u8; 4], coverage: u8) {
    if coverage == 255 {
        target.copy_from_slice(&color);
        return;
    }
    let coverage = u32::from(coverage);
    let inverse = 255 - coverage;
    for channel in 0..4 {
        target[channel] =
            ((u32::from(color[channel]) * coverage + u32::from(target[channel]) * inverse + 127)
                / 255) as u8;
    }
}

#[cfg(test)]
fn band_world_view(
    request: &GeometryRasterRequest,
    row0: u32,
    row1: u32,
    stroke_pixels: u8,
) -> Result<BBox, String> {
    tile_world_view(request, 0, request.width, row0, row1, stroke_pixels)
}

fn tile_world_view(
    request: &GeometryRasterRequest,
    col0: u32,
    col1: u32,
    row0: u32,
    row1: u32,
    stroke_pixels: u8,
) -> Result<BBox, String> {
    let view = request.view;
    let width = request.width as f64;
    let height = request.height as f64;
    let span_x = view.x1 - view.x0;
    let span_y = view.y1 - view.y0;
    let left = view.x0 + col0 as f64 * span_x / width;
    let right = view.x0 + col1 as f64 * span_x / width;
    let lower = view.y1 - row1 as f64 * span_y / height;
    let upper = view.y1 - row0 as f64 * span_y / height;
    // Geometry fill is followed by a device-pixel outline. Expand band and
    // viewport culling by the configured footprint so a shape immediately
    // outside the fill view can still paint its boundary into the image.
    let stroke_margin_y = span_y / height * f64::from(stroke_pixels);
    let stroke_margin_x = span_x / width * f64::from(stroke_pixels);
    Ok(BBox {
        x0: checked_rounded_bound((left - stroke_margin_x).floor(), "raster tile x0")?,
        y0: checked_rounded_bound((lower - stroke_margin_y).floor(), "raster tile lower y")?,
        x1: checked_rounded_bound((right + stroke_margin_x).ceil(), "raster tile x1")?,
        y1: checked_rounded_bound((upper + stroke_margin_y).ceil(), "raster tile upper y")?,
    })
}

fn assemble_tiles(
    request: &GeometryRasterRequest,
    mut tiles: Vec<RasterBand>,
    tile_columns: u32,
    tile_rows: u32,
) -> Result<RgbaFrame, String> {
    let expected_count = u64::from(tile_columns) * u64::from(tile_rows);
    if tiles.len() as u64 != expected_count {
        return Err("raster workers returned an incomplete tile set".to_string());
    }
    tiles.sort_unstable_by_key(|tile| (tile.row0, tile.col0));
    let byte_len = (request.width as usize)
        .checked_mul(request.height as usize)
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| "image byte length overflow".to_string())?;
    let mut pixels = vec![0u8; byte_len];
    let tile_size = u32::from(request.tile_size);
    for (index, tile) in tiles.into_iter().enumerate() {
        let tile_x = index % tile_columns as usize;
        let tile_y = index / tile_columns as usize;
        let expected_col0 = tile_boundary(request.width, tile_x, tile_size);
        let expected_col1 = tile_boundary(request.width, tile_x + 1, tile_size);
        let expected_row0 = tile_boundary(request.height, tile_y, tile_size);
        let expected_row1 = tile_boundary(request.height, tile_y + 1, tile_size);
        if tile.width != request.width
            || tile.height != request.height
            || (tile.col0, tile.col1, tile.row0, tile.row1)
                != (expected_col0, expected_col1, expected_row0, expected_row1)
        {
            return Err("raster worker returned an invalid tile".to_string());
        }
        let tile_width = tile.tile_width() as usize;
        let tile_row_bytes = tile_width * 4;
        for local_row in 0..(tile.row1 - tile.row0) as usize {
            let source = local_row * tile_row_bytes;
            let target = ((tile.row0 as usize + local_row) * request.width as usize
                + tile.col0 as usize)
                * 4;
            pixels[target..target + tile_row_bytes]
                .copy_from_slice(&tile.pixels[source..source + tile_row_bytes]);
        }
    }
    Ok(RgbaFrame {
        width: request.width,
        height: request.height,
        pixels,
    })
}

#[derive(Default)]
struct RasterCounters {
    rect_records: u64,
    rectangle_members_drawn: u64,
    polygon_records: u64,
    polygon_members_drawn: u64,
    path_records: u64,
    path_members_drawn: u64,
    frame_records: u64,
    frame_members_drawn: u64,
    deferred_frame_records: u64,
    label_tiles_drawn: u64,
    label_pixels_drawn: u64,
}

impl RasterCounters {
    fn add(&mut self, other: &Self) {
        self.rect_records = self.rect_records.saturating_add(other.rect_records);
        self.rectangle_members_drawn = self
            .rectangle_members_drawn
            .saturating_add(other.rectangle_members_drawn);
        self.polygon_records = self.polygon_records.saturating_add(other.polygon_records);
        self.polygon_members_drawn = self
            .polygon_members_drawn
            .saturating_add(other.polygon_members_drawn);
        self.path_records = self.path_records.saturating_add(other.path_records);
        self.path_members_drawn = self
            .path_members_drawn
            .saturating_add(other.path_members_drawn);
        self.frame_records = self.frame_records.saturating_add(other.frame_records);
        self.frame_members_drawn = self
            .frame_members_drawn
            .saturating_add(other.frame_members_drawn);
        self.deferred_frame_records = self
            .deferred_frame_records
            .saturating_add(other.deferred_frame_records);
        self.label_tiles_drawn = self
            .label_tiles_drawn
            .saturating_add(other.label_tiles_drawn);
        self.label_pixels_drawn = self
            .label_pixels_drawn
            .saturating_add(other.label_pixels_drawn);
    }
}

#[derive(Clone, Copy)]
enum GeometrySelection {
    All,
    Layer(u32),
}

impl GeometrySelection {
    fn includes(self, layer_idx: u32) -> bool {
        match self {
            Self::All => true,
            Self::Layer(selected) => selected == layer_idx,
        }
    }
}

/// Subtree gate for the per-plane hierarchy walk (F2R-03b 2b).
#[derive(Clone, Copy)]
enum SubtreePrune {
    /// Occupancy paints every layer: no subtree can be skipped.
    Off,
    /// Styled plane: skip children whose subtree holds no decoded page
    /// or wash for this dense scene layer bit (None = nowhere at all).
    Layer(Option<usize>),
}

#[derive(Clone, Copy)]
enum StrokeStyle {
    Solid,
    Dotted,
}

#[derive(Clone, Copy)]
struct PaintStyle {
    color: [u8; 4],
    fill: LayerFill,
    stroke: StrokeStyle,
    stroke_width: u8,
}

impl PaintStyle {
    fn solid(color: [u8; 4]) -> Self {
        Self {
            color,
            fill: LayerFill::Solid,
            stroke: StrokeStyle::Solid,
            stroke_width: 1,
        }
    }

    /// Per-pixel fill rule. Production spans go through `fill_span`; this
    /// stays as the oracle the span specializations are tested against.
    #[cfg(test)]
    fn fills(self, row: u32, col: u32, height: u32) -> bool {
        match self.fill {
            LayerFill::Solid => true,
            LayerFill::Clear => false,
            LayerFill::Speckle => (row + col) & 1 == 0,
            LayerFill::Pattern(rows) => {
                let source_row = row.wrapping_add(height - 1) & 15;
                let source_col = col & 15;
                let word = rows[source_row as usize];
                word & (1 << (15 - source_col)) != 0
            }
        }
    }

    fn strokes(self, step: u64) -> bool {
        match self.stroke {
            StrokeStyle::Solid => true,
            StrokeStyle::Dotted => step & 1 == 0,
        }
    }
}

fn frame_paint(frame_band: u8) -> PaintStyle {
    match frame_band {
        0 => PaintStyle {
            color: [255, 255, 255, 255],
            fill: LayerFill::Clear,
            stroke: StrokeStyle::Solid,
            stroke_width: 1,
        },
        1 => PaintStyle {
            color: [128, 128, 128, 255],
            fill: LayerFill::Clear,
            stroke: StrokeStyle::Solid,
            stroke_width: 1,
        },
        2 => PaintStyle::solid([128, 128, 128, 255]),
        3 => PaintStyle {
            color: [128, 128, 128, 255],
            fill: LayerFill::Clear,
            stroke: StrokeStyle::Dotted,
            stroke_width: 1,
        },
        _ => unreachable!("validated hierarchy frame band"),
    }
}

fn monochrome(color: [u8; 4]) -> [u8; 4] {
    let luminance =
        (299 * u32::from(color[0]) + 587 * u32::from(color[1]) + 114 * u32::from(color[2]) + 500)
            / 1000;
    let luminance = luminance as u8;
    [luminance, luminance, luminance, color[3]]
}

fn add_stats(total: &mut RenderStats, worker: &RenderStats) {
    total.primitives_tested = total
        .primitives_tested
        .saturating_add(worker.primitives_tested);
    total.primitives_drawn = total
        .primitives_drawn
        .saturating_add(worker.primitives_drawn);
    total.rep_members_tested = total
        .rep_members_tested
        .saturating_add(worker.rep_members_tested);
    total.rep_members_drawn = total
        .rep_members_drawn
        .saturating_add(worker.rep_members_drawn);
    total.hier_cells_visited = total
        .hier_cells_visited
        .saturating_add(worker.hier_cells_visited);
    total.subtrees_pruned = total.subtrees_pruned.saturating_add(worker.subtrees_pruned);
    total.raster_tile_max_us = total.raster_tile_max_us.max(worker.raster_tile_max_us);
}

/// Queries one decoded page's record index against a tile-local view
/// and paints the intersecting records. Shared verbatim by the
/// hierarchy walk and the work-bin tile path (F2R-03b 2c) so the two
/// produce identical paint sequences.
#[allow(clippy::too_many_arguments)]
fn raster_page_records(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    page: &crate::DecodedPage,
    page_id: u32,
    local_view: BBox,
    world_transform: OrthoTransform,
    stats: &mut RenderStats,
    counters: &mut RasterCounters,
    paint: PaintStyle,
    guard: Option<RenderGuard<'_>>,
    record_scratch: &mut RecordSet,
) -> Result<(), String> {
    let geometry = page
        .doc
        .cells
        .get(page.doc.top)
        .ok_or_else(|| format!("corrupt page {}: invalid top cell", page_id))?;
    if !geometry.places.is_empty() || !geometry.texts.is_empty() {
        return Err(format!(
            "corrupt page {}: geometry page contains placements or text",
            page_id
        ));
    }
    // Record enumeration is driven by the page's decode-time extent
    // index: records whose full repetition extent cannot reach this
    // tile's local view are never visited (F2R-03b). Corrupt or
    // overflowing records are indexed as always-visible, so the
    // validation errors below stay reachable.
    page.index
        .rects()
        .for_each_intersecting(local_view, record_scratch, |record| {
            let rect = geometry
                .rects
                .get(record as usize)
                .ok_or_else(|| format!("corrupt page {}: stale record index", page_id))?;
            counters.rect_records = counters.rect_records.saturating_add(1);
            stats.primitives_tested = stats.primitives_tested.saturating_add(1);
            if rect.w < 0 || rect.h < 0 {
                return Err(format!(
                    "corrupt page {}: negative rectangle size {}x{}",
                    page_id, rect.w, rect.h
                ));
            }
            if rect.w == 0 || rect.h == 0 {
                return Ok(());
            }
            let x1 = rect
                .x
                .checked_add(rect.w)
                .ok_or_else(|| format!("rectangle x overflow in page {}", page_id))?;
            let y1 = rect
                .y
                .checked_add(rect.h)
                .ok_or_else(|| format!("rectangle y overflow in page {}", page_id))?;
            let base = BBox {
                x0: rect.x,
                y0: rect.y,
                x1,
                y1,
            };
            let mut drawn = 0u64;
            let mut cancel_member = 0u16;
            let visit = for_each_visible_offset_chunked(
                &rect.rep,
                page.index.pts_chunks(&rect.rep),
                base,
                local_view,
                |offset_x, offset_y| {
                    check_member_cancelled(guard, &mut cancel_member)?;
                    let local = translate_bbox(base, offset_x, offset_y)?;
                    let world = world_transform.apply_bbox(local)?;
                    if paint_world_rect(band, request, world, paint)? {
                        drawn = drawn.saturating_add(1);
                    }
                    Ok(())
                },
            )?;
            stats.rep_members_tested = stats.rep_members_tested.saturating_add(visit.tested);
            stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(drawn);
            stats.primitives_drawn = stats.primitives_drawn.saturating_add(drawn);
            counters.rectangle_members_drawn =
                counters.rectangle_members_drawn.saturating_add(drawn);
            Ok(())
        })?;

    page.index
        .polys()
        .for_each_intersecting(local_view, record_scratch, |record| {
            let polygon = geometry
                .polys
                .get(record as usize)
                .ok_or_else(|| format!("corrupt page {}: stale record index", page_id))?;
            counters.polygon_records = counters.polygon_records.saturating_add(1);
            stats.primitives_tested = stats.primitives_tested.saturating_add(1);
            let base = polygon_bbox(&polygon.pts).ok_or_else(|| {
                format!(
                    "corrupt page {}: polygon has fewer than 3 vertices",
                    page_id
                )
            })?;
            let mut drawn = 0u64;
            let mut cancel_member = 0u16;
            // One scratch per record, reused by every repetition member.
            let mut world_points = Vec::with_capacity(polygon.pts.len());
            let visit = for_each_visible_offset_chunked(
                &polygon.rep,
                page.index.pts_chunks(&polygon.rep),
                base,
                local_view,
                |offset_x, offset_y| {
                    check_member_cancelled(guard, &mut cancel_member)?;
                    world_points.clear();
                    for &(x, y) in &polygon.pts {
                        let x = checked_add(x, offset_x, "polygon x")?;
                        let y = checked_add(y, offset_y, "polygon y")?;
                        world_points.push(world_transform.apply(x, y)?);
                    }
                    if paint_world_polygon(band, request, &world_points, paint)? {
                        drawn = drawn.saturating_add(1);
                    }
                    Ok(())
                },
            )?;
            stats.rep_members_tested = stats.rep_members_tested.saturating_add(visit.tested);
            stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(drawn);
            stats.primitives_drawn = stats.primitives_drawn.saturating_add(drawn);
            counters.polygon_members_drawn =
                counters.polygon_members_drawn.saturating_add(drawn);
            Ok(())
        })?;

    page.index
        .paths()
        .for_each_intersecting(local_view, record_scratch, |record| {
            let path_record = geometry
                .paths
                .get(record as usize)
                .ok_or_else(|| format!("corrupt page {}: stale record index", page_id))?;
            counters.path_records = counters.path_records.saturating_add(1);
            stats.primitives_tested = stats.primitives_tested.saturating_add(1);
            let outline = checked_path_outline(
                &path_record.pts,
                path_record.hw,
                path_record.es,
                path_record.ee,
            )
            .map_err(|error| format!("page {}: {}", page_id, error))?;
            let centerline = checked_path_centerline(&path_record.pts)?
                .ok_or_else(|| format!("corrupt page {}: path spine is degenerate", page_id))?;
            let base = polygon_bbox(&outline).ok_or_else(|| {
                format!("corrupt page {}: path outline is degenerate", page_id)
            })?;
            let mut drawn = 0u64;
            let mut cancel_member = 0u16;
            // One outline/centerline scratch pair per record, reused by
            // every repetition member.
            let mut world_points = Vec::with_capacity(outline.len());
            let mut world_centerline = Vec::with_capacity(centerline.len());
            let visit = for_each_visible_offset_chunked(
                &path_record.rep,
                page.index.pts_chunks(&path_record.rep),
                base,
                local_view,
                |offset_x, offset_y| {
                    check_member_cancelled(guard, &mut cancel_member)?;
                    world_points.clear();
                    for &(x, y) in &outline {
                        let x = checked_add(x, offset_x, "path x")?;
                        let y = checked_add(y, offset_y, "path y")?;
                        world_points.push(world_transform.apply(x, y)?);
                    }
                    world_centerline.clear();
                    for &(x, y) in &centerline {
                        let x = checked_add(x, offset_x, "path centerline x")?;
                        let y = checked_add(y, offset_y, "path centerline y")?;
                        world_centerline.push(world_transform.apply(x, y)?);
                    }
                    if paint_world_path(band, request, &world_points, &world_centerline, paint)?
                    {
                        drawn = drawn.saturating_add(1);
                    }
                    Ok(())
                },
            )?;
            stats.rep_members_tested = stats.rep_members_tested.saturating_add(visit.tested);
            stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(drawn);
            stats.primitives_drawn = stats.primitives_drawn.saturating_add(drawn);
            counters.path_members_drawn = counters.path_members_drawn.saturating_add(drawn);
            Ok(())
        })?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_cell(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    band: &mut RasterBand,
    cull_view: BBox,
    stats: &mut RenderStats,
    counters: &mut RasterCounters,
    selection: GeometrySelection,
    prune: SubtreePrune,
    paint: PaintStyle,
    guard: Option<RenderGuard<'_>>,
    key: WsKey,
    world_transform: OrthoTransform,
    path: &mut Vec<WsKey>,
    record_scratch: &mut RecordSet,
) -> Result<(), String> {
    check_cancelled(guard)?;
    if path.contains(&key) {
        return Err(format!("invalid plan: hierarchy cycle at {:?}", key));
    }
    let cell = scene
        .cell(key)
        .ok_or_else(|| format!("invalid plan: missing working cell {:?}", key))?;
    stats.hier_cells_visited = stats.hier_cells_visited.saturating_add(1);
    path.push(key);
    let local_view = world_transform.invert()?.apply_bbox(cull_view)?;

    for &page_id in &cell.pages {
        check_cancelled(guard)?;
        let Some(page) = scene.page(page_id) else {
            continue;
        };
        // The planner selects pages for the whole viewport, while this hot
        // loop runs independently for every image tile. Reject a page in
        // cell-local coordinates before walking any of its records. Without
        // this gate every tile rescanned every selected page; sample9's
        // 858x789 frame repeated the same record walks 49 times at 128px.
        if !selection.includes(page.layer_idx) || !page.bbox.intersects(&local_view) {
            continue;
        }
        raster_page_records(
            band,
            request,
            page,
            page_id,
            local_view,
            world_transform,
            stats,
            counters,
            paint,
            guard,
            record_scratch,
        )?;
    }
    for &(layer_idx, wash) in &cell.washes {
        check_cancelled(guard)?;
        if !selection.includes(layer_idx) {
            continue;
        }
        counters.rect_records = counters.rect_records.saturating_add(1);
        stats.primitives_tested = stats.primitives_tested.saturating_add(1);
        stats.rep_members_tested = stats.rep_members_tested.saturating_add(1);
        let world = world_transform.apply_bbox(wash)?;
        if paint_world_rect(band, request, world, paint)? {
            counters.rectangle_members_drawn = counters.rectangle_members_drawn.saturating_add(1);
            stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(1);
            stats.primitives_drawn = stats.primitives_drawn.saturating_add(1);
        }
    }
    if matches!(selection, GeometrySelection::All) {
        counters.deferred_frame_records = counters
            .deferred_frame_records
            .saturating_add(cell.frames.len().try_into().unwrap_or(u64::MAX));
    }
    for instance in &cell.insts {
        check_cancelled(guard)?;
        let child_bbox = scene
            .cell_bbox(instance.child)
            .ok_or_else(|| format!("invalid plan: missing bbox for child {:?}", instance.child))?;
        if child_bbox.is_empty() {
            continue;
        }
        // Subtree mask gate (F2R-03b 2b): a child whose subtree holds no
        // decoded page or wash for this plane's layer cannot change a
        // pixel, so its repetition is never expanded. Runs after the
        // bbox lookup so the missing-child validation stays reachable.
        if let SubtreePrune::Layer(bit) = prune {
            if !scene.subtree_paints(instance.child, bit) {
                stats.subtrees_pruned = stats.subtrees_pruned.saturating_add(1);
                continue;
            }
        }
        let base_place =
            OrthoTransform::place(instance.x, instance.y, instance.rot, instance.flip)?;
        let base_bbox = base_place.apply_bbox(child_bbox)?;
        let mut cancel_member = 0u16;
        let visit = for_each_visible_offset(
            &instance.rep,
            base_bbox,
            local_view,
            |offset_x, offset_y| {
                check_member_cancelled(guard, &mut cancel_member)?;
                let x = checked_add(instance.x, offset_x, "instance x")?;
                let y = checked_add(instance.y, offset_y, "instance y")?;
                let local = OrthoTransform::place(x, y, instance.rot, instance.flip)?;
                let child_world = world_transform.compose(&local)?;
                render_cell(
                    scene,
                    request,
                    band,
                    cull_view,
                    stats,
                    counters,
                    selection,
                    prune,
                    paint,
                    guard,
                    instance.child,
                    child_world,
                    path,
                    record_scratch,
                )
            },
        )?;
        stats.rep_members_tested = stats.rep_members_tested.saturating_add(visit.tested);
    }
    path.pop();
    Ok(())
}

/// Paints one visited cell's hierarchy-frame records for a band.
/// Shared verbatim by the band walk and the work-bin tile path
/// (F2R-03b 2c).
#[allow(clippy::too_many_arguments)]
fn raster_cell_frames(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    cell: &floe_vfs::hier::WsCell,
    selected_band: u8,
    local_view: BBox,
    world_transform: OrthoTransform,
    stats: &mut RenderStats,
    counters: &mut RasterCounters,
    paint: PaintStyle,
    guard: Option<RenderGuard<'_>>,
) -> Result<(), String> {
for (bbox, repetition, frame_band) in &cell.frames {
    check_cancelled(guard)?;
    if *frame_band > 3 {
        return Err(format!(
            "invalid plan: hierarchy frame band {} is outside 0..=3",
            frame_band
        ));
    }
    if *frame_band != selected_band {
        continue;
    }
    counters.frame_records = counters.frame_records.saturating_add(1);
    stats.primitives_tested = stats.primitives_tested.saturating_add(1);
    let mut drawn = 0u64;
    let mut cancel_member = 0u16;
    let visit =
        for_each_visible_offset(repetition, *bbox, local_view, |offset_x, offset_y| {
            check_member_cancelled(guard, &mut cancel_member)?;
            let local = translate_bbox(*bbox, offset_x, offset_y)?;
            let world = world_transform.apply_bbox(local)?;
            if paint_world_rect(band, request, world, paint)? {
                drawn = drawn.saturating_add(1);
            }
            Ok(())
        })?;
    stats.rep_members_tested = stats.rep_members_tested.saturating_add(visit.tested);
    stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(drawn);
    stats.primitives_drawn = stats.primitives_drawn.saturating_add(drawn);
    counters.frame_members_drawn = counters.frame_members_drawn.saturating_add(drawn);
}
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_frame_band(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    band: &mut RasterBand,
    cull_view: BBox,
    stats: &mut RenderStats,
    counters: &mut RasterCounters,
    selected_band: u8,
    paint: PaintStyle,
    guard: Option<RenderGuard<'_>>,
    key: WsKey,
    world_transform: OrthoTransform,
    path: &mut Vec<WsKey>,
) -> Result<(), String> {
    check_cancelled(guard)?;
    if path.contains(&key) {
        return Err(format!("invalid plan: hierarchy cycle at {:?}", key));
    }
    let cell = scene
        .cell(key)
        .ok_or_else(|| format!("invalid plan: missing working cell {:?}", key))?;
    stats.hier_cells_visited = stats.hier_cells_visited.saturating_add(1);
    path.push(key);
    let local_view = world_transform.invert()?.apply_bbox(cull_view)?;

    raster_cell_frames(
        band,
        request,
        cell,
        selected_band,
        local_view,
        world_transform,
        stats,
        counters,
        paint,
        guard,
    )?;
    for instance in &cell.insts {
        check_cancelled(guard)?;
        let child_bbox = scene
            .cell_bbox(instance.child)
            .ok_or_else(|| format!("invalid plan: missing bbox for child {:?}", instance.child))?;
        if child_bbox.is_empty() {
            continue;
        }
        // Frame-mask gate (F2R-03b 2b): band walks only paint hierarchy
        // frames, so a frame-free subtree is skipped whole.
        if !scene.subtree_has_frames(instance.child) {
            stats.subtrees_pruned = stats.subtrees_pruned.saturating_add(1);
            continue;
        }
        let base_place =
            OrthoTransform::place(instance.x, instance.y, instance.rot, instance.flip)?;
        let base_bbox = base_place.apply_bbox(child_bbox)?;
        let mut cancel_member = 0u16;
        let visit = for_each_visible_offset(
            &instance.rep,
            base_bbox,
            local_view,
            |offset_x, offset_y| {
                check_member_cancelled(guard, &mut cancel_member)?;
                let x = checked_add(instance.x, offset_x, "instance x")?;
                let y = checked_add(instance.y, offset_y, "instance y")?;
                let local = OrthoTransform::place(x, y, instance.rot, instance.flip)?;
                let child_world = world_transform.compose(&local)?;
                render_frame_band(
                    scene,
                    request,
                    band,
                    cull_view,
                    stats,
                    counters,
                    selected_band,
                    paint,
                    guard,
                    instance.child,
                    child_world,
                    path,
                )
            },
        )?;
        stats.rep_members_tested = stats.rep_members_tested.saturating_add(visit.tested);
    }
    path.pop();
    Ok(())
}

fn translate_bbox(bbox: BBox, x: i64, y: i64) -> Result<BBox, String> {
    Ok(BBox {
        x0: checked_add(bbox.x0, x, "bbox x0")?,
        y0: checked_add(bbox.y0, y, "bbox y0")?,
        x1: checked_add(bbox.x1, x, "bbox x1")?,
        y1: checked_add(bbox.y1, y, "bbox y1")?,
    })
}

pub(crate) fn checked_path_outline(
    points: &[(i64, i64)],
    half_width: i64,
    start_extension: i64,
    end_extension: i64,
) -> Result<Vec<(i64, i64)>, String> {
    if half_width < 0 {
        return Err(format!("corrupt path: negative half-width {}", half_width));
    }

    // Mirror floe-tiler's normalization with checked intermediates before
    // calling its KLayout-parity helper, whose public contract uses i64.
    let mut spine = Vec::with_capacity(points.len());
    for &point in points {
        if spine.last() == Some(&point) {
            continue;
        }
        if spine.len() >= 2 {
            let a = spine[spine.len() - 2];
            let b = spine[spine.len() - 1];
            let first = checked_path_vector(a, b)?;
            let second = checked_path_vector(b, point)?;
            if checked_path_cross(first, second)? == 0
                && (first.0.signum(), first.1.signum()) == (second.0.signum(), second.1.signum())
            {
                spine.pop();
            }
        }
        spine.push(point);
    }
    if spine.len() < 2 {
        return Err("unsupported path: spine has fewer than two distinct vertices".to_string());
    }

    let mut directions = Vec::with_capacity(spine.len() - 1);
    let mut manhattan = true;
    for segment in spine.windows(2) {
        let direction = checked_path_direction(segment[0], segment[1])?;
        if direction.0 != 0 && direction.1 != 0 {
            manhattan = false;
        }
        directions.push(direction);
    }
    if !manhattan {
        for pair in spine.windows(3) {
            let first = checked_path_vector(pair[0], pair[1])?;
            let second = checked_path_vector(pair[1], pair[2])?;
            if checked_path_cross(first, second)? == 0
                && (first.0.signum(), first.1.signum()) == (-second.0.signum(), -second.1.signum())
            {
                return Err("unsupported path: U-turn join".to_string());
            }
        }
        return checked_polyline_path_outline(&spine, half_width, start_extension, end_extension);
    }
    for pair in directions.windows(2) {
        if pair[0].0 == -pair[1].0 && pair[0].1 == -pair[1].1 {
            return Err("unsupported path: U-turn join".to_string());
        }
    }

    let last = spine.len() - 1;
    let start_dx = checked_path_product(directions[0].0, start_extension, "start extension x")?;
    let start_dy = checked_path_product(directions[0].1, start_extension, "start extension y")?;
    spine[0] = (
        checked_path_value(spine[0].0 as i128 - start_dx as i128, "path start x")?,
        checked_path_value(spine[0].1 as i128 - start_dy as i128, "path start y")?,
    );
    let end_dx = checked_path_product(directions[last - 1].0, end_extension, "end extension x")?;
    let end_dy = checked_path_product(directions[last - 1].1, end_extension, "end extension y")?;
    spine[last] = (
        checked_path_value(spine[last].0 as i128 + end_dx as i128, "path end x")?,
        checked_path_value(spine[last].1 as i128 + end_dy as i128, "path end y")?,
    );

    let normal = |direction: (i64, i64)| (-direction.1, direction.0);
    let mut offsets = Vec::with_capacity(spine.len());
    offsets.push(normal(directions[0]));
    for index in 1..last {
        let before = normal(directions[index - 1]);
        let after = normal(directions[index]);
        offsets.push((before.0 + after.0, before.1 + after.1));
    }
    offsets.push(normal(directions[last - 1]));
    for (&point, &offset) in spine.iter().zip(&offsets) {
        let dx = checked_path_product(offset.0, half_width, "path half-width x")?;
        let dy = checked_path_product(offset.1, half_width, "path half-width y")?;
        checked_path_value(point.0 as i128 + dx as i128, "path outline x")?;
        checked_path_value(point.0 as i128 - dx as i128, "path outline x")?;
        checked_path_value(point.1 as i128 + dy as i128, "path outline y")?;
        checked_path_value(point.1 as i128 - dy as i128, "path outline y")?;
    }

    floe_tiler::path_outline(points, half_width, start_extension, end_extension)
        .ok_or_else(|| "path outline helper rejected a checked Manhattan path".to_string())
}

pub(crate) fn checked_path_centerline(
    points: &[(i64, i64)],
) -> Result<Option<Vec<(i64, i64)>>, String> {
    let mut spine = Vec::with_capacity(points.len());
    for &point in points {
        if spine.last() == Some(&point) {
            continue;
        }
        if spine.len() >= 2 {
            let a = spine[spine.len() - 2];
            let b = spine[spine.len() - 1];
            let first = checked_path_vector(a, b)?;
            let second = checked_path_vector(b, point)?;
            if checked_path_cross(first, second)? == 0
                && (first.0.signum(), first.1.signum()) == (second.0.signum(), second.1.signum())
            {
                spine.pop();
            }
        }
        spine.push(point);
    }
    if spine.len() < 2 {
        return Ok(None);
    }

    // KLayout applies PATH begin/end extensions to the geometry hull, but
    // draws the styled centerline on the original spine. Extending this line
    // changes its device-pixel slope after integer rounding and can shift the
    // stroke along the entire first or last segment.
    Ok(Some(spine))
}

fn checked_polyline_path_outline(
    spine: &[(i64, i64)],
    half_width: i64,
    start_extension: i64,
    end_extension: i64,
) -> Result<Vec<(i64, i64)>, String> {
    let mut units = Vec::with_capacity(spine.len() - 1);
    for segment in spine.windows(2) {
        let (dx, dy) = checked_path_vector(segment[0], segment[1])?;
        let length = (dx as f64).hypot(dy as f64);
        if !length.is_finite() || length == 0.0 {
            return Err("invalid non-Manhattan path length".to_string());
        }
        units.push((dx as f64 / length, dy as f64 / length));
    }

    let width = half_width as f64;
    let first = units[0];
    let last = units[units.len() - 1];
    let mut left = Vec::with_capacity(spine.len() * 2);
    let mut right = Vec::with_capacity(spine.len() * 2);
    push_path_delta(
        &mut left,
        spine[0],
        (
            -first.0 * start_extension as f64 - first.1 * width,
            -first.1 * start_extension as f64 + first.0 * width,
        ),
    )?;
    push_path_delta(
        &mut right,
        spine[0],
        (
            -first.0 * start_extension as f64 + first.1 * width,
            -first.1 * start_extension as f64 - first.0 * width,
        ),
    )?;

    for index in 1..spine.len() - 1 {
        let before = units[index - 1];
        let after = units[index];
        let turn = before.0 * after.1 - before.1 * after.0;
        if !turn.is_finite() || turn == 0.0 {
            return Err("invalid non-Manhattan path join".to_string());
        }
        append_path_join(&mut left, spine[index], before, after, turn, width, 1.0)?;
        append_path_join(&mut right, spine[index], before, after, turn, width, -1.0)?;
    }

    let end = spine[spine.len() - 1];
    push_path_delta(
        &mut left,
        end,
        (
            last.0 * end_extension as f64 - last.1 * width,
            last.1 * end_extension as f64 + last.0 * width,
        ),
    )?;
    push_path_delta(
        &mut right,
        end,
        (
            last.0 * end_extension as f64 + last.1 * width,
            last.1 * end_extension as f64 - last.0 * width,
        ),
    )?;

    // KLayout's hull starts at the right side of the path start, walks the
    // left side forward, and returns along the right side in reverse.
    let mut outline = Vec::with_capacity(left.len() + right.len());
    push_distinct(&mut outline, right[0]);
    for point in left {
        push_distinct(&mut outline, point);
    }
    for point in right.into_iter().skip(1).rev() {
        push_distinct(&mut outline, point);
    }
    if outline.len() > 1 && outline.first() == outline.last() {
        outline.pop();
    }
    Ok(outline)
}

fn append_path_join(
    side_points: &mut Vec<(i64, i64)>,
    vertex: (i64, i64),
    before: (f64, f64),
    after: (f64, f64),
    turn: f64,
    half_width: f64,
    side: f64,
) -> Result<(), String> {
    let before_normal = (-before.1 * half_width * side, before.0 * half_width * side);
    let after_normal = (-after.1 * half_width * side, after.0 * half_width * side);
    let normal_delta = (
        after_normal.0 - before_normal.0,
        after_normal.1 - before_normal.1,
    );
    let along_before = (normal_delta.0 * after.1 - normal_delta.1 * after.0) / turn;

    // KLayout clips only the outer side of an acute corner. The two clip
    // points are the touching square-cap corners of the adjacent segments.
    // For shallower corners the normal-line intersection is the miter point.
    let outer = turn * side < 0.0;
    let tolerance = f64::EPSILON * half_width.abs().max(1.0) * 16.0;
    if outer && along_before.abs() > half_width + tolerance {
        push_path_delta(
            side_points,
            vertex,
            (
                before_normal.0 + before.0 * half_width,
                before_normal.1 + before.1 * half_width,
            ),
        )?;
        push_path_delta(
            side_points,
            vertex,
            (
                after_normal.0 - after.0 * half_width,
                after_normal.1 - after.1 * half_width,
            ),
        )?;
    } else {
        push_path_delta(
            side_points,
            vertex,
            (
                before_normal.0 + before.0 * along_before,
                before_normal.1 + before.1 * along_before,
            ),
        )?;
    }
    Ok(())
}

fn push_path_delta(
    points: &mut Vec<(i64, i64)>,
    base: (i64, i64),
    delta: (f64, f64),
) -> Result<(), String> {
    let dx = checked_rounded_f64(delta.0, "path outline dx")?;
    let dy = checked_rounded_f64(delta.1, "path outline dy")?;
    let point = (
        checked_add(base.0, dx, "path outline x")?,
        checked_add(base.1, dy, "path outline y")?,
    );
    push_distinct(points, point);
    Ok(())
}

fn push_distinct(points: &mut Vec<(i64, i64)>, point: (i64, i64)) {
    if points.last() != Some(&point) {
        points.push(point);
    }
}

fn checked_rounded_f64(value: f64, field: &str) -> Result<i64, String> {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(format!("coordinate overflow: {} = {}", field, value));
    }
    Ok(value.round() as i64)
}

fn checked_path_direction(start: (i64, i64), end: (i64, i64)) -> Result<(i64, i64), String> {
    let (dx, dy) = checked_path_vector(start, end)?;
    Ok((dx.signum(), dy.signum()))
}

fn checked_path_vector(start: (i64, i64), end: (i64, i64)) -> Result<(i64, i64), String> {
    Ok((
        checked_path_value(end.0 as i128 - start.0 as i128, "path segment dx")?,
        checked_path_value(end.1 as i128 - start.1 as i128, "path segment dy")?,
    ))
}

fn checked_path_cross(first: (i64, i64), second: (i64, i64)) -> Result<i128, String> {
    (first.0 as i128 * second.1 as i128)
        .checked_sub(first.1 as i128 * second.0 as i128)
        .ok_or_else(|| "coordinate overflow: path segment cross product".to_string())
}

fn checked_path_product(value: i64, scale: i64, field: &str) -> Result<i64, String> {
    checked_path_value(value as i128 * scale as i128, field)
}

fn checked_path_value(value: i128, field: &str) -> Result<i64, String> {
    value
        .try_into()
        .map_err(|_| format!("coordinate overflow: {} = {}", field, value))
}

fn checked_add(a: i64, b: i64, field: &str) -> Result<i64, String> {
    let value = a as i128 + b as i128;
    value
        .try_into()
        .map_err(|_| format!("coordinate overflow: {} = {}", field, value))
}

/// KLayout hairline parity (2026-08-27, RENDERER-TESTS §픽셀 정책
/// 헤어라인 실측): a member whose device extent rounds to zero on an
/// axis collapses to the nearest-grid pixel on that axis — KLayout
/// snaps edges to the pixel grid before scan-converting, so a
/// sub-pixel feature lights exactly one pixel per collapsed axis
/// instead of every pixel it touches. The collapse also skips the
/// whole fill+stroke pipeline, which is the dominant per-member cost
/// in hairline-scale views. Solid strokes only: dotted hierarchy
/// frames keep their band styling.
///
/// Correctness note: the world bbox is exact for every caller (rect =
/// itself, polygon/path = vertex bbox), the device map is monotone
/// affine, and a connected shape's axis projection is an interval, so
/// the collapsed rect equals the snapped shape exactly. Using the
/// bbox for all three primitives keeps the representation-exact
/// contract (same world rect as RECT/POLYGON/PATH renders the same).
fn hairline_world_bbox(
    request: &GeometryRasterRequest,
    world: BBox,
    paint: PaintStyle,
) -> Result<Option<(i128, i128, i128, i128)>, String> {
    // 1px solid strokes only. A wider outline paints far more than
    // the collapsed cells (KLayout w4 A/B: 84,303 vs 24,158 px — an
    // interior-diff regression, not band noise), so thick-stroked
    // members keep the full fill+stroke pipeline until KLayout's
    // per-width collapse rule is measured. Dotted keeps the frame
    // band styling.
    if !matches!(paint.stroke, StrokeStyle::Solid) || paint.stroke_width != 1 {
        return Ok(None);
    }
    let (x0, y1) = world_to_device(request, world.x0, world.y0)?;
    let (x1, y0) = world_to_device(request, world.x1, world.y1)?;
    let sub_x = x1 - x0 < DEVICE_ONE;
    let sub_y = y1 - y0 < DEVICE_ONE;
    if !sub_x && !sub_y {
        return Ok(None);
    }
    let point = sub_x && sub_y;
    let (cx0, cx1) = hairline_axis_span(x0, x1, sub_x, point, 0);
    let (cy0, cy1) = hairline_axis_span(y0, y1, sub_y, point, -1);
    Ok(Some((cx0, cy0, cx1, cy1)))
}

/// Measured KLayout hairline placement (32px-aligned and 858px
/// fractional probes agree):
/// - a POINT (both axes sub-pixel) lands in the single pixel nearest
///   its center — an 0.8px box matches round(center) exactly, smaller
///   boxes shift the threshold by ~0.05px (inside the P-a band);
/// - a WIRE's narrow axis lights the rounded pixel of EACH edge (two
///   columns when the edges round apart, ~w probability);
/// - the y axis carries a constant -1 pixel bias in both cases;
/// - the long axis keeps its edge-snapped span.
fn hairline_axis_span(v0: i128, v1: i128, sub: bool, point: bool, bias: i128) -> (i128, i128) {
    if sub && point {
        let cell = floor_div(v0 + v1 + DEVICE_ONE, 2 * DEVICE_ONE) + bias;
        (cell, cell + 1)
    } else if sub {
        let lo = floor_div(v0 + DEVICE_HALF, DEVICE_ONE) + bias;
        let hi = floor_div(v1 + DEVICE_HALF, DEVICE_ONE) + bias;
        (lo, hi + 1)
    } else {
        (
            floor_div(v0 + DEVICE_HALF, DEVICE_ONE),
            floor_div(v1 + DEVICE_HALF, DEVICE_ONE),
        )
    }
}

/// Paints a collapsed hairline rect as solid spans in the member's
/// color — KLayout draws sub-pixel features via their outline line,
/// so the fill pattern does not apply.
fn paint_hairline_device_rect(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    rect: (i128, i128, i128, i128),
    paint: PaintStyle,
) -> Result<bool, String> {
    let (x0, y0, x1, y1) = rect;
    let first_row = y0.max(band.row0 as i128);
    let end_row = y1.min(band.row1 as i128);
    let first_col = x0.max(band.col0 as i128);
    let end_col = x1.min(band.col1 as i128);
    if first_row >= end_row || first_col >= end_col {
        return Ok(false);
    }
    let first_row = checked_usize(first_row, "hairline first row")?;
    let end_row = checked_usize(end_row, "hairline end row")?;
    let first_col = checked_usize(first_col, "hairline first column")?;
    let end_col = checked_usize(end_col, "hairline end column")?;
    let solid = PaintStyle {
        fill: LayerFill::Solid,
        ..paint
    };
    let mut drew = false;
    for row in first_row..end_row {
        if fill_span(band, solid, request.height, row, first_col, end_col) {
            drew = true;
        }
    }
    Ok(drew)
}

fn paint_world_rect(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    world: BBox,
    paint: PaintStyle,
) -> Result<bool, String> {
    if let Some(rect) = hairline_world_bbox(request, world, paint)? {
        return paint_hairline_device_rect(band, request, rect, paint);
    }
    let filled = fill_world_rect(band, request, world, paint)?;
    let stroked = stroke_world_polygon(
        band,
        request,
        &[
            (world.x0, world.y0),
            (world.x1, world.y0),
            (world.x1, world.y1),
            (world.x0, world.y1),
        ],
        paint,
    )?;
    Ok(filled || stroked)
}

fn paint_world_polygon(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    points: &[(i64, i64)],
    paint: PaintStyle,
) -> Result<bool, String> {
    if let Some(world) = polygon_bbox(points) {
        if let Some(rect) = hairline_world_bbox(request, world, paint)? {
            return paint_hairline_device_rect(band, request, rect, paint);
        }
    }
    let filled = fill_world_polygon(band, request, points, paint)?;
    let stroked = stroke_world_polygon(band, request, points, paint)?;
    Ok(filled || stroked)
}

fn paint_world_path_outline(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    points: &[(i64, i64)],
    paint: PaintStyle,
) -> Result<bool, String> {
    let filled = fill_world_polygon(band, request, points, paint)?;
    let stroked = stroke_world_polygon(band, request, points, paint)?;
    Ok(filled || stroked)
}

fn paint_world_path(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    outline: &[(i64, i64)],
    centerline: &[(i64, i64)],
    paint: PaintStyle,
) -> Result<bool, String> {
    if let Some(world) = polygon_bbox(outline) {
        if let Some(rect) = hairline_world_bbox(request, world, paint)? {
            // The centerline lies inside the collapsed outline bbox, so
            // its stroke pass is covered by the collapsed spans.
            return paint_hairline_device_rect(band, request, rect, paint);
        }
    }
    let outlined = paint_world_path_outline(band, request, outline, paint)?;
    let centered = stroke_world_polyline(band, request, centerline, paint)?;
    Ok(outlined || centered)
}

fn fill_world_rect(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    world: BBox,
    paint: PaintStyle,
) -> Result<bool, String> {
    let (x0, y1) = world_to_device(request, world.x0, world.y0)?;
    let (x1, y0) = world_to_device(request, world.x1, world.y1)?;
    let centered =
        fill_device_rect_with_phase(band, request, x0, y0, x1, y1, FillPhase::PixelCenter, paint)?;
    let boundary = fill_device_rect_with_phase(
        band,
        request,
        x0,
        y0,
        x1,
        y1,
        FillPhase::LowerBoundary,
        paint,
    )?;
    Ok(centered || boundary)
}

fn fill_world_polygon(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    points: &[(i64, i64)],
    paint: PaintStyle,
) -> Result<bool, String> {
    if points.len() < 3 {
        return Err("polygon has fewer than 3 vertices".to_string());
    }
    let mut device = Vec::with_capacity(points.len());
    for &(x, y) in points {
        device.push(world_to_device(request, x, y)?);
    }
    let centered =
        fill_device_polygon_with_phase(band, request, &device, FillPhase::PixelCenter, paint)?;
    let boundary =
        fill_device_polygon_with_phase(band, request, &device, FillPhase::LowerBoundary, paint)?;
    Ok(centered || boundary)
}

#[allow(clippy::too_many_arguments)]
fn fill_device_rect_with_phase(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    x0: i128,
    y0: i128,
    x1: i128,
    y1: i128,
    phase: FillPhase,
    paint: PaintStyle,
) -> Result<bool, String> {
    let (first_row, end_row) = fill_phase_rows(y0, y1, phase)?;
    let (first_col, end_col) = fill_phase_columns(x0, x1, phase)?;
    let first_row = first_row.max(band.row0 as i128);
    let end_row = end_row.min(band.row1 as i128);
    let first_col = first_col.max(band.col0 as i128);
    let end_col = end_col.min(band.col1 as i128);
    if first_row >= end_row || first_col >= end_col {
        return Ok(false);
    }
    let first_row = checked_usize(first_row, "rectangle first row")?;
    let end_row = checked_usize(end_row, "rectangle end row")?;
    let first_col = checked_usize(first_col, "rectangle first column")?;
    let end_col = checked_usize(end_col, "rectangle end column")?;
    let mut drew = false;
    for row in first_row..end_row {
        if fill_span(band, paint, request.height, row, first_col, end_col) {
            drew = true;
        }
    }
    Ok(drew)
}

/// Paints one device-row span with the interior fill rule, matching
/// `PaintStyle::fills` pixel for pixel. `row` and the half-open column range
/// are already clamped to the band, so the per-pixel checked conversions of
/// the former loop cannot fail and the fill kind is decided once per span.
fn fill_span(
    band: &mut RasterBand,
    paint: PaintStyle,
    frame_height: u32,
    row: usize,
    first_col: usize,
    end_col: usize,
) -> bool {
    if first_col >= end_col {
        return false;
    }
    let row_offset = (row - band.row0 as usize) * band.tile_width() as usize;
    let col_base = band.col0 as usize;
    match paint.fill {
        LayerFill::Clear => false,
        LayerFill::Solid => {
            let start = (row_offset + first_col - col_base) * 4;
            let end = (row_offset + end_col - col_base) * 4;
            for pixel in band.pixels[start..end].chunks_exact_mut(4) {
                pixel.copy_from_slice(&paint.color);
            }
            true
        }
        LayerFill::Speckle => {
            let mut col = first_col + ((row + first_col) & 1);
            let drew = col < end_col;
            while col < end_col {
                let offset = (row_offset + col - col_base) * 4;
                band.pixels[offset..offset + 4].copy_from_slice(&paint.color);
                col += 2;
            }
            drew
        }
        LayerFill::Pattern(rows) => {
            let source_row = (row as u32).wrapping_add(frame_height - 1) & 15;
            let word = rows[source_row as usize];
            if word == 0 {
                return false;
            }
            let mut drew = false;
            for col in first_col..end_col {
                if word & (1u16 << (15 - (col & 15))) == 0 {
                    continue;
                }
                let offset = (row_offset + col - col_base) * 4;
                band.pixels[offset..offset + 4].copy_from_slice(&paint.color);
                drew = true;
            }
            drew
        }
    }
}

fn polygon_bbox(points: &[(i64, i64)]) -> Option<BBox> {
    if points.len() < 3 {
        return None;
    }
    let mut bbox = BBox::EMPTY;
    for &(x, y) in points {
        bbox.grow(&BBox {
            x0: x,
            y0: y,
            x1: x,
            y1: y,
        });
    }
    Some(bbox)
}

fn stroke_world_polygon(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    points: &[(i64, i64)],
    paint: PaintStyle,
) -> Result<bool, String> {
    if points.len() < 2 {
        return Ok(false);
    }
    // Convert each vertex once; the previous per-segment form converted
    // every vertex twice (as an end and again as the next start).
    let first = world_to_stroke_vertex(request, points[0])?;
    let mut start = first;
    let mut drew = false;
    for &point in &points[1..] {
        let end = world_to_stroke_vertex(request, point)?;
        if stroke_device_segment(band, request, start, end, paint)? {
            drew = true;
        }
        start = end;
    }
    if stroke_device_segment(band, request, start, first, paint)? {
        drew = true;
    }
    Ok(drew)
}

fn stroke_world_polyline(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    points: &[(i64, i64)],
    paint: PaintStyle,
) -> Result<bool, String> {
    if points.len() < 2 {
        return Ok(false);
    }
    let mut start = world_to_stroke_vertex(request, points[0])?;
    let mut drew = false;
    for &point in &points[1..] {
        let end = world_to_stroke_vertex(request, point)?;
        if stroke_device_segment(band, request, start, end, paint)? {
            drew = true;
        }
        start = end;
    }
    Ok(drew)
}

fn world_to_stroke_vertex(
    request: &GeometryRasterRequest,
    point: (i64, i64),
) -> Result<(f64, f64), String> {
    let view = request.view;
    let span_x = view.x1 - view.x0;
    let span_y = view.y1 - view.y0;
    let x = (point.0 as f64 - view.x0) * request.width as f64 / span_x;
    let lower_y = (point.1 as f64 - view.y0) * request.height as f64 / span_y;
    let x = (x + 0.5).floor();
    let y = request.height as f64 - 1.0 - (lower_y + 0.5).floor();
    if !x.is_finite() || !y.is_finite() {
        return Err("coordinate overflow: edge device vertex".to_string());
    }
    Ok((x, y))
}

fn stroke_device_segment(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    start: (f64, f64),
    end: (f64, f64),
    paint: PaintStyle,
) -> Result<bool, String> {
    let stroke_low = -((i64::from(paint.stroke_width) - 1) / 2);
    let stroke_high = i64::from(paint.stroke_width) / 2;
    let Some((x0, y0, x1, y1)) = clip_device_segment(
        start.0,
        start.1,
        end.0,
        end.1,
        -(stroke_high as f64),
        request.width as f64 - 1.0 - stroke_low as f64,
        -(stroke_high as f64),
        request.height as f64 - 1.0 - stroke_low as f64,
    ) else {
        return Ok(false);
    };
    let mut x0 = checked_rounded_f64(x0, "edge x0")?;
    let mut y0 = checked_rounded_f64(y0, "edge y0")?;
    let x1 = checked_rounded_f64(x1, "edge x1")?;
    let y1 = checked_rounded_f64(y1, "edge y1")?;
    // A solid axis-aligned segment visits each Bresenham step exactly once
    // along one axis, so the painted union is one rectangular block. Writing
    // it as clamped row spans skips the per-step overlapping block writes;
    // dotted strokes keep the stepped path below.
    if matches!(paint.stroke, StrokeStyle::Solid) && (x0 == x1 || y0 == y1) {
        let row_lo = (y0.min(y1) + stroke_low).max(band.row0 as i64);
        let row_hi = (y0.max(y1) + stroke_high).min(band.row1 as i64 - 1);
        let col_lo = (x0.min(x1) + stroke_low).max(band.col0 as i64);
        let col_hi = (x0.max(x1) + stroke_high).min(band.col1 as i64 - 1);
        if row_lo > row_hi || col_lo > col_hi {
            return Ok(false);
        }
        let width = band.tile_width() as usize;
        for row in row_lo..=row_hi {
            let local_row = row as usize - band.row0 as usize;
            let start = (local_row * width + col_lo as usize - band.col0 as usize) * 4;
            let end = (local_row * width + col_hi as usize + 1 - band.col0 as usize) * 4;
            for pixel in band.pixels[start..end].chunks_exact_mut(4) {
                pixel.copy_from_slice(&paint.color);
            }
        }
        return Ok(true);
    }
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut error = dx + dy;
    let mut drew = false;
    let mut step = 0u64;
    loop {
        if paint.strokes(step) {
            for stroke_y in y0 + stroke_low..=y0 + stroke_high {
                if stroke_y < band.row0 as i64 || stroke_y >= band.row1 as i64 {
                    continue;
                }
                for stroke_x in x0 + stroke_low..=x0 + stroke_high {
                    if stroke_x < band.col0 as i64 || stroke_x >= band.col1 as i64 {
                        continue;
                    }
                    let local_row = stroke_y as usize - band.row0 as usize;
                    let local_col = stroke_x as usize - band.col0 as usize;
                    let offset = (local_row * band.tile_width() as usize + local_col) * 4;
                    band.pixels[offset..offset + 4].copy_from_slice(&paint.color);
                    drew = true;
                }
            }
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let doubled = error.saturating_mul(2);
        if doubled >= dy {
            error += dy;
            x0 += sx;
        }
        if doubled <= dx {
            error += dx;
            y0 += sy;
        }
        step = step.saturating_add(1);
    }
    Ok(drew)
}

#[allow(clippy::too_many_arguments)]
fn clip_device_segment(
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    xmin: f64,
    xmax: f64,
    ymin: f64,
    ymax: f64,
) -> Option<(f64, f64, f64, f64)> {
    if xmin > xmax || ymin > ymax {
        return None;
    }
    let dx = x1 - x0;
    let dy = y1 - y0;
    let mut first = 0.0f64;
    let mut last = 1.0f64;
    for (p, q) in [
        (-dx, x0 - xmin),
        (dx, xmax - x0),
        (-dy, y0 - ymin),
        (dy, ymax - y0),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let ratio = q / p;
        if p < 0.0 {
            if ratio > last {
                return None;
            }
            first = first.max(ratio);
        } else {
            if ratio < first {
                return None;
            }
            last = last.min(ratio);
        }
    }
    Some((
        x0 + first * dx,
        y0 + first * dy,
        x0 + last * dx,
        y0 + last * dy,
    ))
}

#[derive(Clone, Copy)]
struct ActiveEdge {
    x0: i128,
    y0: i128,
    dx: i128,
    dy: i128,
    first_row: i64,
    end_row: i64,
}

#[derive(Clone, Copy)]
enum FillPhase {
    PixelCenter,
    LowerBoundary,
}

fn fill_phase_rows(y0: i128, y1: i128, phase: FillPhase) -> Result<(i128, i128), String> {
    match phase {
        // Both sampling phases use a half-open y rule so shared vertices
        // always contribute exactly one incident edge.
        FillPhase::PixelCenter => Ok((
            floor_div(
                y0.checked_sub(DEVICE_HALF)
                    .ok_or_else(|| "coordinate overflow: polygon first row".to_string())?,
                DEVICE_ONE,
            )
            .checked_add(1)
            .ok_or_else(|| "coordinate overflow: polygon first row".to_string())?,
            floor_div(
                y1.checked_sub(DEVICE_HALF)
                    .ok_or_else(|| "coordinate overflow: polygon end row".to_string())?,
                DEVICE_ONE,
            )
            .checked_add(1)
            .ok_or_else(|| "coordinate overflow: polygon end row".to_string())?,
        )),
        FillPhase::LowerBoundary => Ok((floor_div(y0, DEVICE_ONE), floor_div(y1, DEVICE_ONE))),
    }
}

fn fill_phase_columns(x0: i128, x1: i128, phase: FillPhase) -> Result<(i128, i128), String> {
    match phase {
        FillPhase::PixelCenter => Ok((
            floor_div(
                x0.checked_sub(DEVICE_HALF)
                    .ok_or_else(|| "coordinate overflow: polygon first column".to_string())?,
                DEVICE_ONE,
            )
            .checked_add(1)
            .ok_or_else(|| "coordinate overflow: polygon first column".to_string())?,
            floor_div(
                x1.checked_sub(DEVICE_HALF)
                    .ok_or_else(|| "coordinate overflow: polygon end column".to_string())?,
                DEVICE_ONE,
            )
            .checked_add(1)
            .ok_or_else(|| "coordinate overflow: polygon end column".to_string())?,
        )),
        FillPhase::LowerBoundary => Ok((
            floor_div(x0, DEVICE_ONE)
                .checked_add(1)
                .ok_or_else(|| "coordinate overflow: polygon first column".to_string())?,
            ceil_div(x1, DEVICE_ONE),
        )),
    }
}

#[cfg(test)]
fn fill_world_polygon_with_phase(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    points: &[(i64, i64)],
    phase: FillPhase,
    paint: PaintStyle,
) -> Result<bool, String> {
    if points.len() < 3 {
        return Err("polygon has fewer than 3 vertices".to_string());
    }
    let mut device = Vec::with_capacity(points.len());
    for &(x, y) in points {
        device.push(world_to_device(request, x, y)?);
    }
    fill_device_polygon_with_phase(band, request, &device, phase, paint)
}

fn fill_device_polygon_with_phase(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    device: &[(i128, i128)],
    phase: FillPhase,
    paint: PaintStyle,
) -> Result<bool, String> {
    let mut edges = Vec::with_capacity(device.len());
    for index in 0..device.len() {
        let (mut x0, mut y0) = device[index];
        let (mut x1, mut y1) = device[(index + 1) % device.len()];
        if y0 == y1 {
            continue;
        }
        if y0 > y1 {
            std::mem::swap(&mut x0, &mut x1);
            std::mem::swap(&mut y0, &mut y1);
        }
        let (first_row, end_row) = fill_phase_rows(y0, y1, phase)?;
        if first_row >= end_row {
            continue;
        }
        edges.push(ActiveEdge {
            x0,
            y0,
            dx: x1
                .checked_sub(x0)
                .ok_or_else(|| "coordinate overflow: polygon edge dx".to_string())?,
            dy: y1
                .checked_sub(y0)
                .ok_or_else(|| "coordinate overflow: polygon edge dy".to_string())?,
            first_row: checked_i64(first_row, "polygon first row")?,
            end_row: checked_i64(end_row, "polygon end row")?,
        });
    }
    if edges.is_empty() {
        return Ok(false);
    }
    edges.sort_unstable_by_key(|edge| (edge.first_row, edge.end_row, edge.x0));
    let first_row = edges
        .iter()
        .map(|edge| edge.first_row)
        .min()
        .unwrap_or(0)
        .max(0);
    let end_row = edges
        .iter()
        .map(|edge| edge.end_row)
        .max()
        .unwrap_or(0)
        .min(band.row1 as i64);
    let first_row = first_row.max(band.row0 as i64);
    if first_row >= end_row {
        return Ok(false);
    }

    let mut next_edge = 0usize;
    while next_edge < edges.len() && edges[next_edge].first_row < first_row {
        next_edge += 1;
    }
    let mut active: Vec<ActiveEdge> = edges
        .iter()
        .copied()
        .filter(|edge| edge.first_row < first_row && edge.end_row > first_row)
        .collect();
    let mut drew = false;
    let mut intersections = Vec::with_capacity(active.len());
    for row in first_row..end_row {
        while next_edge < edges.len() && edges[next_edge].first_row == row {
            active.push(edges[next_edge]);
            next_edge += 1;
        }
        active.retain(|edge| edge.end_row > row);
        let scan_y = match phase {
            FillPhase::PixelCenter => row as i128 * DEVICE_ONE + DEVICE_HALF,
            FillPhase::LowerBoundary => (row as i128 + 1) * DEVICE_ONE,
        };
        intersections.clear();
        for edge in &active {
            let rise = scan_y
                .checked_sub(edge.y0)
                .ok_or_else(|| "coordinate overflow: polygon edge rise".to_string())?;
            let product = rise
                .checked_mul(edge.dx)
                .ok_or_else(|| "coordinate overflow: polygon intersection".to_string())?;
            let delta = floor_div(product, edge.dy);
            intersections.push(
                edge.x0
                    .checked_add(delta)
                    .ok_or_else(|| "coordinate overflow: polygon x".to_string())?,
            );
        }
        intersections.sort_unstable();
        if intersections.len() % 2 != 0 {
            return Err(format!(
                "invalid polygon: odd edge count {} at row {}",
                intersections.len(),
                row
            ));
        }
        for pair in intersections.chunks_exact(2) {
            let (first_col, end_col) = fill_phase_columns(pair[0], pair[1], phase)?;
            let first_col = first_col.max(band.col0 as i128);
            let end_col = end_col.min(band.col1 as i128);
            if first_col >= end_col {
                continue;
            }
            let first_col = checked_usize(first_col, "polygon first column")?;
            let end_col = checked_usize(end_col, "polygon end column")?;
            if fill_span(
                band,
                paint,
                request.height,
                row as usize,
                first_col,
                end_col,
            ) {
                drew = true;
            }
        }
    }
    Ok(drew)
}

fn world_to_device(
    request: &GeometryRasterRequest,
    x: i64,
    y: i64,
) -> Result<(i128, i128), String> {
    let view = request.view;
    let span_x = view.x1 - view.x0;
    let span_y = view.y1 - view.y0;
    let x = scale_device_f64(
        x as f64 - view.x0,
        request.width,
        span_x,
        "polygon device x",
    )?;
    let y = scale_device_f64(
        view.y1 - y as f64,
        request.height,
        span_y,
        "polygon device y",
    )?;
    Ok((x, y))
}

fn scale_device_f64(offset: f64, pixels: u32, span: f64, field: &str) -> Result<i128, String> {
    let value = offset * pixels as f64 * DEVICE_ONE as f64 / span;
    if !value.is_finite() || value < -(MAX_DEVICE_COORD as f64) || value > MAX_DEVICE_COORD as f64 {
        return Err(format!("coordinate overflow: {}", field));
    }
    Ok(value.floor() as i128)
}

fn floor_div(numerator: i128, denominator: i128) -> i128 {
    debug_assert!(denominator > 0);
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder < 0 {
        quotient - 1
    } else {
        quotient
    }
}

fn ceil_div(numerator: i128, denominator: i128) -> i128 {
    debug_assert!(denominator > 0);
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder > 0 {
        quotient + 1
    } else {
        quotient
    }
}

fn checked_i64(value: i128, field: &str) -> Result<i64, String> {
    value
        .try_into()
        .map_err(|_| format!("limit exceeded: {} = {}", field, value))
}

fn checked_rounded_bound(value: f64, field: &str) -> Result<i64, String> {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(format!("coordinate overflow: {} = {}", field, value));
    }
    Ok(value as i64)
}

fn checked_usize(value: i128, field: &str) -> Result<usize, String> {
    value
        .try_into()
        .map_err(|_| format!("limit exceeded: {} = {}", field, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DecodedPage, RenderLabel, DEFAULT_LABEL_FONT_PX};
    use floe_oasis::doc::{Cell, Doc, PathRec, PolyRec, RectRec, Rep};
    use floe_vfs::hier::{HierPlan, HierStats, WsCell, WsInst, REM_FULL};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    fn request() -> GeometryRasterRequest {
        GeometryRasterRequest {
            view: RasterViewBox::new(0.0, 0.0, 10.0, 10.0).unwrap(),
            width: 10,
            height: 10,
            background: [0, 0, 0, 255],
            foreground: [255, 255, 255, 255],
            workers: 1,
            tile_size: DEFAULT_TILE_SIZE,
        }
    }

    fn paint(request: &GeometryRasterRequest) -> PaintStyle {
        PaintStyle::solid(request.foreground)
    }

    #[test]
    fn preserves_fractional_view_phase_and_expands_stroke_culling() {
        assert!(RasterViewBox::new(f64::NAN, 0.0, 1.0, 1.0).is_err());
        assert!(RasterViewBox::new(1.0, 0.0, 0.0, 1.0).is_err());
        let request = GeometryRasterRequest {
            view: RasterViewBox::new(-0.5, -0.5, 9.5, 9.5).unwrap(),
            ..request()
        };
        assert_eq!(
            world_to_device(&request, 0, 0).unwrap(),
            (DEVICE_HALF, 9 * DEVICE_ONE + DEVICE_HALF)
        );
        assert_eq!(
            band_world_view(&request, 0, 5, 1).unwrap(),
            BBox {
                x0: -2,
                y0: 3,
                x1: 11,
                y1: 11,
            }
        );
    }

    #[test]
    fn rejects_device_coordinates_outside_checked_q32_domain() {
        let request = GeometryRasterRequest {
            view: RasterViewBox::new(0.0, 0.0, 1.0, 1.0).unwrap(),
            ..request()
        };
        let error = world_to_device(&request, i64::MAX, 0).unwrap_err();
        assert!(error.contains("coordinate overflow: polygon device x"));
    }

    fn full_band(request: &GeometryRasterRequest) -> RasterBand {
        RasterBand::new(request, 0, request.height).unwrap()
    }

    fn pixel_at_band(band: &RasterBand, x: usize, y: usize) -> [u8; 4] {
        let offset =
            ((y - band.row0 as usize) * band.tile_width() as usize + x - band.col0 as usize) * 4;
        band.pixels[offset..offset + 4].try_into().unwrap()
    }

    #[test]
    fn fills_expected_pixels_with_inverted_y() {
        let request = request();
        let mut frame = full_band(&request);
        assert!(fill_world_rect(
            &mut frame,
            &request,
            BBox {
                x0: 2,
                y0: 3,
                x1: 4,
                y1: 5,
            },
            paint(&request),
        )
        .unwrap());
        let lit: Vec<(usize, usize)> = frame
            .pixels
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, pixel)| pixel[0] == 255)
            .map(|(index, _)| (index % 10, index / 10))
            .collect();
        assert_eq!(lit, vec![(2, 5), (3, 5), (2, 6), (3, 6)]);
    }

    #[test]
    fn axis_aligned_polygon_paint_matches_rectangle_paint() {
        let request = request();
        let mut rectangle = full_band(&request);
        let mut polygon = rectangle.clone();
        let bbox = BBox {
            x0: 2,
            y0: 3,
            x1: 4,
            y1: 5,
        };
        paint_world_rect(&mut rectangle, &request, bbox, paint(&request)).unwrap();
        paint_world_polygon(
            &mut polygon,
            &request,
            &[(2, 3), (4, 3), (4, 5), (2, 5)],
            paint(&request),
        )
        .unwrap();
        assert_eq!(polygon.pixels, rectangle.pixels);
    }

    #[test]
    fn rectangle_fast_path_matches_polygon_phase_matrix() {
        let paint = PaintStyle {
            color: [37, 211, 89, 255],
            fill: LayerFill::Speckle,
            stroke: StrokeStyle::Solid,
            stroke_width: 1,
        };
        for offset in [-0.125, -0.05, 0.0, 0.025, 0.05, 0.125] {
            let request = GeometryRasterRequest {
                view: RasterViewBox::new(offset, offset, 10.0 + offset, 10.0 + offset).unwrap(),
                ..request()
            };
            for bbox in [
                BBox {
                    x0: -1,
                    y0: -1,
                    x1: 3,
                    y1: 4,
                },
                BBox {
                    x0: 2,
                    y0: 3,
                    x1: 4,
                    y1: 5,
                },
                BBox {
                    x0: 7,
                    y0: 6,
                    x1: 12,
                    y1: 11,
                },
            ] {
                let points = [
                    (bbox.x0, bbox.y0),
                    (bbox.x1, bbox.y0),
                    (bbox.x1, bbox.y1),
                    (bbox.x0, bbox.y1),
                ];
                let mut rectangle = full_band(&request);
                let mut polygon = rectangle.clone();
                fill_world_rect(&mut rectangle, &request, bbox, paint).unwrap();
                fill_world_polygon(&mut polygon, &request, &points, paint).unwrap();
                assert_eq!(
                    polygon.pixels, rectangle.pixels,
                    "offset={offset} bbox={bbox:?}"
                );
            }
        }
    }

    #[test]
    fn fill_span_matches_per_pixel_fill_oracle() {
        let request = GeometryRasterRequest {
            view: RasterViewBox::new(0.0, 0.0, 40.0, 40.0).unwrap(),
            width: 40,
            height: 40,
            background: [0, 0, 0, 255],
            foreground: [255, 255, 255, 255],
            workers: 1,
            tile_size: DEFAULT_TILE_SIZE,
        };
        let mut pattern = [0u16; 16];
        for (row, word) in pattern.iter_mut().enumerate() {
            *word = 0b1010_0110_0001_1101u16.rotate_left(row as u32);
        }
        for fill in [
            LayerFill::Solid,
            LayerFill::Speckle,
            LayerFill::Pattern(pattern),
            LayerFill::Pattern([0u16; 16]),
            LayerFill::Clear,
        ] {
            let paint = PaintStyle {
                color: [200, 30, 90, 255],
                fill,
                stroke: StrokeStyle::Solid,
                stroke_width: 1,
            };
            // A tile with odd origins exercises the local offset arithmetic
            // and every speckle parity and pattern column phase.
            let mut band = RasterBand::new_tile(&request, 3, 27, 5, 23).unwrap();
            let mut oracle = band.clone();
            for row in 5..23usize {
                for (first_col, end_col) in [(3usize, 27usize), (7, 8), (10, 10)] {
                    let drew = fill_span(&mut band, paint, request.height, row, first_col, end_col);
                    let mut oracle_drew = false;
                    for col in first_col..end_col {
                        if !paint.fills(row as u32, col as u32, request.height) {
                            continue;
                        }
                        let offset = ((row - 5) * oracle.tile_width() as usize + (col - 3)) * 4;
                        oracle.pixels[offset..offset + 4].copy_from_slice(&paint.color);
                        oracle_drew = true;
                    }
                    assert_eq!(drew, oracle_drew, "fill={fill:?} row={row}");
                }
            }
            assert_eq!(band.pixels, oracle.pixels, "fill={fill:?}");
        }
    }

    /// The original stepped stroke loop, kept verbatim as the oracle for
    /// the axis-aligned solid span fast path.
    fn stroke_device_segment_reference(
        band: &mut RasterBand,
        request: &GeometryRasterRequest,
        start: (f64, f64),
        end: (f64, f64),
        paint: PaintStyle,
    ) -> Result<bool, String> {
        let stroke_low = -((i64::from(paint.stroke_width) - 1) / 2);
        let stroke_high = i64::from(paint.stroke_width) / 2;
        let Some((x0, y0, x1, y1)) = clip_device_segment(
            start.0,
            start.1,
            end.0,
            end.1,
            -(stroke_high as f64),
            request.width as f64 - 1.0 - stroke_low as f64,
            -(stroke_high as f64),
            request.height as f64 - 1.0 - stroke_low as f64,
        ) else {
            return Ok(false);
        };
        let mut x0 = checked_rounded_f64(x0, "edge x0")?;
        let mut y0 = checked_rounded_f64(y0, "edge y0")?;
        let x1 = checked_rounded_f64(x1, "edge x1")?;
        let y1 = checked_rounded_f64(y1, "edge y1")?;
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut error = dx + dy;
        let mut drew = false;
        let mut step = 0u64;
        loop {
            if paint.strokes(step) {
                for stroke_y in y0 + stroke_low..=y0 + stroke_high {
                    if stroke_y < band.row0 as i64 || stroke_y >= band.row1 as i64 {
                        continue;
                    }
                    for stroke_x in x0 + stroke_low..=x0 + stroke_high {
                        if stroke_x < band.col0 as i64 || stroke_x >= band.col1 as i64 {
                            continue;
                        }
                        let local_row = stroke_y as usize - band.row0 as usize;
                        let local_col = stroke_x as usize - band.col0 as usize;
                        let offset = (local_row * band.tile_width() as usize + local_col) * 4;
                        band.pixels[offset..offset + 4].copy_from_slice(&paint.color);
                        drew = true;
                    }
                }
            }
            if x0 == x1 && y0 == y1 {
                break;
            }
            let doubled = error.saturating_mul(2);
            if doubled >= dy {
                error += dy;
                x0 += sx;
            }
            if doubled <= dx {
                error += dx;
                y0 += sy;
            }
            step = step.saturating_add(1);
        }
        Ok(drew)
    }

    #[test]
    fn axis_aligned_stroke_span_matches_stepped_oracle() {
        let request = GeometryRasterRequest {
            view: RasterViewBox::new(0.0, 0.0, 30.0, 30.0).unwrap(),
            width: 30,
            height: 30,
            background: [0, 0, 0, 255],
            foreground: [255, 255, 255, 255],
            workers: 1,
            tile_size: DEFAULT_TILE_SIZE,
        };
        let segments = [
            ((4.0, 9.0), (21.0, 9.0)),   // horizontal inside the tile
            ((21.0, 12.0), (4.0, 12.0)), // horizontal, reversed direction
            ((11.0, 2.0), (11.0, 28.0)), // vertical crossing tile rows
            ((2.0, 14.0), (2.0, 14.0)),  // degenerate point segment
            ((0.0, 3.0), (29.0, 3.0)),   // horizontal above the tile rows
            ((5.0, 5.0), (17.0, 20.0)),  // diagonal: same stepped path
        ];
        for stroke_width in [1u8, 2, 3, 8] {
            let paint = PaintStyle {
                color: [90, 140, 250, 255],
                fill: LayerFill::Solid,
                stroke: StrokeStyle::Solid,
                stroke_width,
            };
            let mut fast = RasterBand::new_tile(&request, 3, 25, 6, 26).unwrap();
            let mut oracle = fast.clone();
            for (start, end) in segments {
                let drew_fast =
                    stroke_device_segment(&mut fast, &request, start, end, paint).unwrap();
                let drew_oracle =
                    stroke_device_segment_reference(&mut oracle, &request, start, end, paint)
                        .unwrap();
                assert_eq!(
                    drew_fast, drew_oracle,
                    "width={stroke_width} segment={start:?}->{end:?}"
                );
            }
            assert_eq!(fast.pixels, oracle.pixels, "width={stroke_width}");
        }
    }

    #[test]
    fn half_phase_fill_is_exact_across_rect_polygon_and_path_outline() {
        let request = GeometryRasterRequest {
            view: RasterViewBox::new(0.05, 0.05, 10.05, 10.05).unwrap(),
            width: 100,
            height: 100,
            ..request()
        };
        let bbox = BBox {
            x0: 2,
            y0: 2,
            x1: 8,
            y1: 8,
        };
        let points = [(2, 2), (8, 2), (8, 8), (2, 8)];
        let centerline = [(2, 5), (8, 5)];
        let path_outline = checked_path_outline(&centerline, 3, 0, 0).unwrap();
        assert_eq!(polygon_bbox(&path_outline), Some(bbox));

        let mut rectangle = full_band(&request);
        let mut polygon = rectangle.clone();
        let mut path = rectangle.clone();
        let paint = paint(&request);
        paint_world_rect(&mut rectangle, &request, bbox, paint).unwrap();
        paint_world_polygon(&mut polygon, &request, &points, paint).unwrap();
        paint_world_path(&mut path, &request, &path_outline, &centerline, paint).unwrap();

        assert_eq!(polygon.pixels, rectangle.pixels);
        assert_eq!(path.pixels, rectangle.pixels);
    }

    #[test]
    fn paints_rectangle_with_one_pixel_device_outline() {
        let request = GeometryRasterRequest {
            view: RasterViewBox::new(0.0, 0.0, 4.0, 4.0).unwrap(),
            width: 4,
            height: 4,
            background: [0, 0, 0, 255],
            foreground: [255, 255, 255, 255],
            workers: 1,
            tile_size: DEFAULT_TILE_SIZE,
        };
        let mut band = full_band(&request);
        paint_world_rect(
            &mut band,
            &request,
            BBox {
                x0: 1,
                y0: 1,
                x1: 3,
                y1: 3,
            },
            paint(&request),
        )
        .unwrap();
        let lit: Vec<(usize, usize)> = band
            .pixels
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, pixel)| pixel[0] == 255)
            .map(|(index, _)| (index % 4, index / 4))
            .collect();
        assert_eq!(
            lit,
            vec![
                (1, 0),
                (2, 0),
                (3, 0),
                (1, 1),
                (2, 1),
                (3, 1),
                (1, 2),
                (2, 2),
                (3, 2),
            ]
        );
    }

    #[test]
    fn edge_stroke_keeps_global_phase_across_worker_bands() {
        let request = request();
        let mut full = full_band(&request);
        stroke_device_segment(&mut full, &request, (0.0, 9.0), (9.0, 2.0), paint(&request))
            .unwrap();

        let mut upper = RasterBand::new(&request, 0, 5).unwrap();
        let mut lower = RasterBand::new(&request, 5, 10).unwrap();
        stroke_device_segment(
            &mut upper,
            &request,
            (0.0, 9.0),
            (9.0, 2.0),
            paint(&request),
        )
        .unwrap();
        stroke_device_segment(
            &mut lower,
            &request,
            (0.0, 9.0),
            (9.0, 2.0),
            paint(&request),
        )
        .unwrap();
        upper.pixels.extend_from_slice(&lower.pixels);

        assert_eq!(upper.pixels, full.pixels);
    }

    #[test]
    fn primitive_paint_is_seamless_across_two_dimensional_tiles() {
        let mut request = request();
        request.tile_size = 3;
        let mut pattern = [0u16; 16];
        for (row, bits) in pattern.iter_mut().enumerate() {
            *bits = if row % 2 == 0 { 0xaaaa } else { 0x5555 };
        }
        let paint = PaintStyle {
            color: [19, 211, 83, 255],
            fill: LayerFill::Pattern(pattern),
            stroke: StrokeStyle::Dotted,
            stroke_width: 4,
        };
        let primitives = |target: &mut RasterBand| -> Result<(), String> {
            paint_world_rect(
                target,
                &request,
                BBox {
                    x0: 1,
                    y0: 1,
                    x1: 8,
                    y1: 7,
                },
                paint,
            )?;
            paint_world_polygon(target, &request, &[(0, 2), (9, 8), (7, 0)], paint)?;
            paint_world_path_outline(target, &request, &[(0, 4), (3, 9), (9, 2)], paint)?;
            Ok(())
        };

        let mut full = full_band(&request);
        primitives(&mut full).unwrap();
        let columns = request.width.div_ceil(u32::from(request.tile_size));
        let rows = request.height.div_ceil(u32::from(request.tile_size));
        let mut tiles = Vec::new();
        for tile_y in 0..rows as usize {
            for tile_x in 0..columns as usize {
                let mut tile = RasterBand::new_tile(
                    &request,
                    tile_boundary(request.width, tile_x, 3),
                    tile_boundary(request.width, tile_x + 1, 3),
                    tile_boundary(request.height, tile_y, 3),
                    tile_boundary(request.height, tile_y + 1, 3),
                )
                .unwrap();
                primitives(&mut tile).unwrap();
                tiles.push(tile);
            }
        }
        let tiled = assemble_tiles(&request, tiles, columns, rows).unwrap();
        assert_eq!(tiled.pixels, full.pixels);
    }

    #[test]
    fn even_outline_width_uses_klayout_device_bias() {
        let request = request();
        let mut band = full_band(&request);
        stroke_device_segment(
            &mut band,
            &request,
            (5.0, 0.0),
            (5.0, 9.0),
            PaintStyle {
                color: [255, 0, 0, 255],
                fill: LayerFill::Clear,
                stroke: StrokeStyle::Solid,
                stroke_width: 4,
            },
        )
        .unwrap();
        let columns: Vec<usize> = (0..10)
            .filter(|&x| pixel_at_band(&band, x, 5) == [255, 0, 0, 255])
            .collect();
        assert_eq!(columns, vec![4, 5, 6, 7]);
    }

    #[test]
    fn fills_non_manhattan_triangle_with_half_open_edge_rule() {
        let request = GeometryRasterRequest {
            view: RasterViewBox::new(0.0, 0.0, 4.0, 4.0).unwrap(),
            width: 4,
            height: 4,
            background: [0, 0, 0, 255],
            foreground: [255, 255, 255, 255],
            workers: 1,
            tile_size: DEFAULT_TILE_SIZE,
        };
        let mut frame = full_band(&request);
        fill_world_polygon_with_phase(
            &mut frame,
            &request,
            &[(0, 0), (4, 0), (0, 4)],
            FillPhase::PixelCenter,
            paint(&request),
        )
        .unwrap();
        let lit: Vec<(usize, usize)> = frame
            .pixels
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, pixel)| pixel[0] == 255)
            .map(|(index, _)| (index % 4, index / 4))
            .collect();
        assert_eq!(
            lit,
            vec![
                (0, 0),
                (0, 1),
                (1, 1),
                (0, 2),
                (1, 2),
                (2, 2),
                (0, 3),
                (1, 3),
                (2, 3),
                (3, 3),
            ]
        );
    }

    #[test]
    fn rejects_unbounded_allocation() {
        let mut request = request();
        request.width = u32::MAX;
        assert!(request.validate().unwrap_err().contains("limit exceeded"));
    }

    #[test]
    fn rejects_invalid_worker_count() {
        let mut request = request();
        request.workers = 0;
        assert!(request.validate().unwrap_err().contains("workers"));
        request.workers = MAX_WORKERS + 1;
        assert!(request.validate().unwrap_err().contains("workers"));
        request.workers = 1;
        request.tile_size = 0;
        assert!(request.validate().unwrap_err().contains("tile size"));
    }

    #[test]
    fn path_outline_matches_parent_manhattan_contract() {
        let outline = checked_path_outline(&[(10, 10), (20, 10), (20, 20)], 2, 3, 4).unwrap();
        assert_eq!(
            outline,
            vec![(7, 12), (18, 12), (18, 24), (22, 24), (22, 8), (7, 8)]
        );
    }

    #[test]
    fn outlines_diagonal_paths_like_klayout_and_rejects_unsafe_arithmetic() {
        assert_eq!(
            checked_path_outline(&[(0, 0), (2, 2)], 1, 0, 0).unwrap(),
            vec![(1, -1), (-1, 1), (1, 3), (3, 1)]
        );
        assert_eq!(
            checked_path_outline(&[(17_000, 0), (20_000, 0), (22_000, 2_000)], 250, 0, 0).unwrap(),
            vec![
                (17_000, -250),
                (17_000, 250),
                (19_896, 250),
                (21_823, 2_177),
                (22_177, 1_823),
                (20_104, -250),
            ]
        );
        assert_eq!(
            checked_path_outline(&[(25_000, 0), (28_000, 0), (26_000, 2_000)], 250, 0, 0).unwrap(),
            vec![
                (25_000, -250),
                (25_000, 250),
                (27_396, 250),
                (25_823, 1_823),
                (26_177, 2_177),
                (28_354, 0),
                (28_250, -250),
            ]
        );
        assert!(
            checked_path_outline(&[(i64::MIN, 0), (i64::MAX, 0)], 1, 0, 0)
                .unwrap_err()
                .contains("path segment dx")
        );
        assert_eq!(
            checked_path_outline(&[(0, 0), (10, 0), (0, 0)], 1, 0, 0).unwrap_err(),
            "unsupported path: U-turn join"
        );
        assert_eq!(
            checked_path_outline(&[(3, 4), (3, 4)], 1, 0, 0).unwrap_err(),
            "unsupported path: spine has fewer than two distinct vertices"
        );
    }

    fn styled_page(page_id: u32, layer_idx: u32, bbox: BBox) -> Arc<DecodedPage> {
        let doc = Doc {
            unit: 1.0,
            cells: vec![Cell {
                name: format!("P{page_id}"),
                rects: vec![RectRec {
                    layer: layer_idx,
                    dt: 0,
                    x: bbox.x0,
                    y: bbox.y0,
                    w: bbox.x1 - bbox.x0,
                    h: bbox.y1 - bbox.y0,
                    rep: Rep::One,
                }],
                ..Cell::default()
            }],
            top: 0,
            layer_order: vec![(layer_idx, 0)],
            norm_s: 0.0,
            layer_names: HashMap::new(),
            layer_aliases: HashMap::new(),
        };
        Arc::new(DecodedPage {
            page_id,
            layer_idx,
            bbox,
            encoded_bytes: 1,
            records: 1,
            members: 1,
            index: crate::PageIndex::build(&doc),
            doc,
        })
    }

    fn styled_scene(frames: Vec<(BBox, Rep, u8)>) -> FrameScene {
        styled_scene_with_labels(frames, Vec::new())
    }

    #[test]
    fn page_bbox_prunes_record_walks_per_image_tile() {
        let top = (0, REM_FULL);
        let left = BBox {
            x0: 1,
            y0: 1,
            x1: 4,
            y1: 9,
        };
        let right = BBox {
            x0: 16,
            y0: 1,
            x1: 19,
            y1: 9,
        };
        let plan = HierPlan {
            top,
            wcells: vec![WsCell {
                key: top,
                pages: vec![0, 1],
                insts: Vec::new(),
                frames: Vec::new(),
                washes: Vec::new(),
            }],
            pages: vec![0, 1],
            page_prio: vec![0, 1],
            stats: HierStats::default(),
        };
        let mut bounds = BTreeMap::new();
        bounds.insert(
            top,
            BBox {
                x0: 0,
                y0: 0,
                x1: 20,
                y1: 10,
            },
        );
        let scene = FrameScene::from_test_parts(
            plan,
            vec![styled_page(0, 0, left), styled_page(1, 1, right)],
            bounds,
        )
        .unwrap();
        let raster = GeometryRasterRequest {
            view: RasterViewBox::new(0.0, 0.0, 20.0, 10.0).unwrap(),
            width: 20,
            height: 10,
            workers: 2,
            tile_size: 10,
            ..request()
        };
        let report = render_geometry_styled(
            &scene,
            &StyledGeometryRasterRequest {
                raster,
                layers: vec![
                    LayerStyle {
                        layer_idx: 0,
                        color: [255, 0, 0, 255],
                        fill: LayerFill::Solid,
                        outline_width: 1,
                    },
                    LayerStyle {
                        layer_idx: 1,
                        color: [0, 255, 0, 255],
                        fill: LayerFill::Solid,
                        outline_width: 1,
                    },
                ],
                hierarchy_frames: false,
                mono: false,
            },
        )
        .unwrap();
        assert_eq!(report.rect_record_tests, 2);
        assert_eq!(pixel(&report.frame, 2, 5), [255, 0, 0, 255]);
        assert_eq!(pixel(&report.frame, 17, 5), [0, 255, 0, 255]);
    }

    fn styled_scene_with_labels(
        frames: Vec<(BBox, Rep, u8)>,
        labels: Vec<RenderLabel>,
    ) -> FrameScene {
        styled_scene_with_label_font(frames, labels, DEFAULT_LABEL_FONT_PX)
    }

    fn styled_scene_with_label_font(
        frames: Vec<(BBox, Rep, u8)>,
        labels: Vec<RenderLabel>,
        label_font_px: f32,
    ) -> FrameScene {
        let top = (0, REM_FULL);
        let plan = HierPlan {
            top,
            wcells: vec![WsCell {
                key: top,
                pages: vec![0, 1],
                insts: Vec::new(),
                frames,
                washes: Vec::new(),
            }],
            pages: vec![0, 1],
            page_prio: vec![0, 1],
            stats: HierStats::default(),
        };
        let mut bounds = BTreeMap::new();
        bounds.insert(
            top,
            BBox {
                x0: 0,
                y0: 0,
                x1: 10,
                y1: 10,
            },
        );
        FrameScene::from_test_parts_with_labels(
            plan,
            vec![
                styled_page(
                    0,
                    0,
                    BBox {
                        x0: 1,
                        y0: 1,
                        x1: 9,
                        y1: 9,
                    },
                ),
                styled_page(
                    1,
                    1,
                    BBox {
                        x0: 2,
                        y0: 2,
                        x1: 8,
                        y1: 8,
                    },
                ),
            ],
            bounds,
            Arc::from(labels),
            label_font_px,
        )
        .unwrap()
    }

    #[test]
    fn unsupported_path_fails_the_render_instead_of_being_deferred() {
        let page_id = 7;
        let top = (0, REM_FULL);
        let bbox = BBox {
            x0: 0,
            y0: 0,
            x1: 10,
            y1: 10,
        };
        let plan = HierPlan {
            top,
            wcells: vec![WsCell {
                key: top,
                pages: vec![page_id],
                insts: Vec::new(),
                frames: Vec::new(),
                washes: Vec::new(),
            }],
            pages: vec![page_id],
            page_prio: vec![0],
            stats: HierStats::default(),
        };
        let doc = Doc {
            unit: 1.0,
            cells: vec![Cell {
                name: "U_TURN".to_string(),
                paths: vec![PathRec {
                    layer: 1,
                    dt: 0,
                    pts: vec![(1, 5), (9, 5), (1, 5)],
                    hw: 1,
                    es: 0,
                    ee: 0,
                    rep: Rep::One,
                }],
                ..Cell::default()
            }],
            top: 0,
            layer_order: vec![(1, 0)],
            norm_s: 0.0,
            layer_names: HashMap::new(),
            layer_aliases: HashMap::new(),
        };
        let decoded = Arc::new(DecodedPage {
            page_id,
            layer_idx: 0,
            bbox,
            encoded_bytes: 1,
            records: 1,
            members: 1,
            index: crate::PageIndex::build(&doc),
            doc,
        });
        let scene = FrameScene::from_test_parts(plan, vec![decoded], BTreeMap::from([(top, bbox)]))
            .unwrap();

        let error = render_geometry_occupancy(&scene, &request())
            .err()
            .expect("unsupported PATH must fail the render");
        assert_eq!(error, "page 7: unsupported path: U-turn join");
    }

    fn pixel(frame: &RgbaFrame, x: usize, y: usize) -> [u8; 4] {
        let offset = (y * frame.width as usize + x) * 4;
        frame.pixels[offset..offset + 4].try_into().unwrap()
    }

    #[test]
    fn record_index_pruning_matches_unpruned_pixels() {
        // In-view geometry mixed with records far outside the viewport,
        // including a far-anchored Pts repetition whose one member reaches
        // back into view: pruning must drop work but never a pixel.
        let make_doc = || {
            let mut rects = Vec::new();
            for i in 0..40i64 {
                rects.push(RectRec {
                    layer: 1,
                    dt: 0,
                    x: (i % 8) * 3 - 6,
                    y: (i / 8) * 3 - 6,
                    w: 2,
                    h: 2,
                    rep: if i % 3 == 0 {
                        Rep::Grid {
                            na: 4,
                            nb: 2,
                            va: (5, 0),
                            vb: (0, 7),
                        }
                    } else {
                        Rep::One
                    },
                });
            }
            for i in 0..40i64 {
                rects.push(RectRec {
                    layer: 1,
                    dt: 0,
                    x: 1_000 + i * 10,
                    y: -2_000,
                    w: 4,
                    h: 4,
                    rep: Rep::One,
                });
            }
            // A large Pts fill record: a few members near the viewport,
            // the remaining chunks far away (exercises the 2a chunk skip).
            let mut fill_offsets: Vec<(i64, i64)> = Vec::new();
            for index in 0..24i64 {
                fill_offsets.push(((index % 6) * 4, (index / 6) * 4));
            }
            while fill_offsets.len() < 320 {
                let index = fill_offsets.len() as i64;
                fill_offsets.push((40_000 + index * 8, 40_000));
            }
            rects.push(RectRec {
                layer: 1,
                dt: 0,
                x: 0,
                y: 0,
                w: 2,
                h: 2,
                rep: Rep::Pts(Arc::from(fill_offsets)),
            });
            Doc {
                unit: 1000.0,
                cells: vec![Cell {
                    name: "IDX".to_string(),
                    rects,
                    polys: vec![
                        PolyRec {
                            layer: 1,
                            dt: 0,
                            pts: vec![(1, 1), (6, 2), (4, 6)],
                            rep: Rep::One,
                        },
                        PolyRec {
                            layer: 1,
                            dt: 0,
                            pts: vec![(900, 900), (920, 905), (910, 930)],
                            rep: Rep::Pts(Arc::from([(0, 0), (-895, -897)])),
                        },
                    ],
                    paths: vec![
                        PathRec {
                            layer: 1,
                            dt: 0,
                            pts: vec![(0, 8), (9, 8)],
                            hw: 1,
                            es: 0,
                            ee: 0,
                            rep: Rep::One,
                        },
                        PathRec {
                            layer: 1,
                            dt: 0,
                            pts: vec![(500, 0), (560, 0)],
                            hw: 2,
                            es: 1,
                            ee: 1,
                            rep: Rep::One,
                        },
                    ],
                    ..Cell::default()
                }],
                top: 0,
                layer_order: vec![(1, 0)],
                norm_s: 0.0,
                layer_names: HashMap::new(),
                layer_aliases: HashMap::new(),
            }
        };
        let bbox = BBox {
            x0: -3_000,
            y0: -3_000,
            x1: 3_000,
            y1: 3_000,
        };
        let scene_with = |index: fn(&Doc) -> crate::PageIndex| {
            let doc = make_doc();
            let decoded = Arc::new(DecodedPage {
                page_id: 0,
                layer_idx: 1,
                bbox,
                encoded_bytes: 1,
                records: 1,
                members: 1,
                index: index(&doc),
                doc,
            });
            let top = (0, REM_FULL);
            let plan = HierPlan {
                top,
                wcells: vec![WsCell {
                    key: top,
                    pages: vec![0],
                    insts: Vec::new(),
                    frames: Vec::new(),
                    washes: Vec::new(),
                }],
                pages: vec![0],
                page_prio: vec![0],
                stats: HierStats::default(),
            };
            FrameScene::from_test_parts(plan, vec![decoded], BTreeMap::from([(top, bbox)])).unwrap()
        };
        let request = GeometryRasterRequest {
            view: RasterViewBox::new(0.0, 0.0, 30.0, 30.0).unwrap(),
            width: 30,
            height: 30,
            background: [0, 0, 0, 255],
            foreground: [255, 255, 255, 255],
            workers: 2,
            tile_size: 16,
        };
        let pruned =
            render_geometry_occupancy(&scene_with(crate::PageIndex::build), &request).unwrap();
        let unpruned =
            render_geometry_occupancy(&scene_with(crate::PageIndex::unpruned), &request).unwrap();
        assert_eq!(pruned.frame.pixels(), unpruned.frame.pixels());
        assert_eq!(
            pruned.rectangle_member_paints,
            unpruned.rectangle_member_paints
        );
        assert_eq!(pruned.polygon_member_paints, unpruned.polygon_member_paints);
        assert_eq!(pruned.path_member_paints, unpruned.path_member_paints);
        assert!(pruned.polygon_member_paints >= 2, "Pts member must survive");
        assert!(
            pruned.rect_record_tests < unpruned.rect_record_tests,
            "pruning must drop far records: {} vs {}",
            pruned.rect_record_tests,
            unpruned.rect_record_tests
        );
        assert!(pruned.path_record_tests < unpruned.path_record_tests);
        assert!(
            pruned.stats.rep_members_tested < unpruned.stats.rep_members_tested,
            "chunked Pts must skip far chunks: {} vs {}",
            pruned.stats.rep_members_tested,
            unpruned.stats.rep_members_tested
        );
    }

    /// Hierarchy for the 2b mask tests: top holds a layer-0 page and
    /// instantiates child A (layer-0 page, gridded) plus child B
    /// (layer-1 page, gridded, in view) and child C whose only page
    /// stays deferred. Layer plane 0 must prune B and C whole.
    fn masked_scene(corrupt_b: bool) -> FrameScene {
        let top = (0, REM_FULL);
        let child_a = (1, REM_FULL);
        let child_b = (2, REM_FULL);
        let child_c = (3, REM_FULL);
        let unit = BBox {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 2,
        };
        let grid = Rep::Grid {
            na: 3,
            nb: 3,
            va: (4, 0),
            vb: (0, 4),
        };
        let inst = |child, x, y| WsInst {
            child,
            x,
            y,
            rot: 0,
            flip: false,
            rep: grid.clone(),
        };
        let plan = HierPlan {
            top,
            wcells: vec![
                WsCell {
                    key: top,
                    pages: vec![0],
                    insts: vec![inst(child_a, 2, 2), inst(child_b, 4, 2), inst(child_c, 2, 4)],
                    frames: Vec::new(),
                    washes: Vec::new(),
                },
                WsCell {
                    key: child_a,
                    pages: vec![1],
                    insts: Vec::new(),
                    frames: Vec::new(),
                    washes: Vec::new(),
                },
                WsCell {
                    key: child_b,
                    pages: vec![2],
                    insts: Vec::new(),
                    frames: Vec::new(),
                    washes: Vec::new(),
                },
                WsCell {
                    key: child_c,
                    pages: vec![3],
                    insts: Vec::new(),
                    frames: Vec::new(),
                    washes: Vec::new(),
                },
            ],
            pages: vec![0, 1, 2, 3],
            page_prio: vec![0, 1, 2, 3],
            stats: HierStats::default(),
        };
        let page_b = if corrupt_b {
            let doc = Doc {
                unit: 1.0,
                cells: vec![Cell {
                    name: "BAD".to_string(),
                    rects: vec![RectRec {
                        layer: 1,
                        dt: 0,
                        x: 0,
                        y: 0,
                        w: -1,
                        h: 2,
                        rep: Rep::One,
                    }],
                    ..Cell::default()
                }],
                top: 0,
                layer_order: vec![(1, 0)],
                norm_s: 0.0,
                layer_names: HashMap::new(),
                layer_aliases: HashMap::new(),
            };
            Arc::new(DecodedPage {
                page_id: 2,
                layer_idx: 1,
                bbox: unit,
                encoded_bytes: 1,
                records: 1,
                members: 1,
                index: crate::PageIndex::build(&doc),
                doc,
            })
        } else {
            styled_page(2, 1, unit)
        };
        let span = BBox {
            x0: 0,
            y0: 0,
            x1: 16,
            y1: 16,
        };
        let bounds = BTreeMap::from([
            (top, span),
            (child_a, unit),
            (child_b, unit),
            (child_c, unit),
        ]);
        FrameScene::from_test_parts(
            plan,
            vec![
                styled_page(0, 0, unit),
                styled_page(1, 0, unit),
                page_b,
                // page 3 stays deferred: child C prunes on every plane
            ],
            bounds,
        )
        .unwrap()
    }

    fn masked_request() -> StyledGeometryRasterRequest {
        StyledGeometryRasterRequest {
            raster: GeometryRasterRequest {
                view: RasterViewBox::new(0.0, 0.0, 16.0, 16.0).unwrap(),
                width: 16,
                height: 16,
                workers: 2,
                tile_size: 8,
                ..request()
            },
            layers: vec![
                LayerStyle {
                    layer_idx: 0,
                    color: [255, 0, 0, 255],
                    fill: LayerFill::Solid,
                    outline_width: 1,
                },
                LayerStyle {
                    layer_idx: 7, // styled but present nowhere in the scene
                    color: [0, 0, 255, 255],
                    fill: LayerFill::Solid,
                    outline_width: 1,
                },
            ],
            hierarchy_frames: false,
            mono: false,
        }
    }

    /// One-layer scene over a 320x320-unit world rendered at 32px
    /// (10 units/px), so sub-pixel features are expressible in i64
    /// world coordinates.
    fn hairline_scene(rects: Vec<RectRec>, polys: Vec<PolyRec>, paths: Vec<PathRec>) -> FrameScene {
        let doc = Doc {
            unit: 1.0,
            cells: vec![Cell {
                name: "HAIR".to_string(),
                rects,
                polys,
                paths,
                ..Cell::default()
            }],
            top: 0,
            layer_order: vec![(1, 0)],
            norm_s: 0.0,
            layer_names: HashMap::new(),
            layer_aliases: HashMap::new(),
        };
        let bbox = BBox {
            x0: 0,
            y0: 0,
            x1: 320,
            y1: 320,
        };
        let decoded = Arc::new(DecodedPage {
            page_id: 0,
            layer_idx: 1,
            bbox,
            encoded_bytes: 1,
            records: 1,
            members: 1,
            index: crate::PageIndex::build(&doc),
            doc,
        });
        let top = (0, REM_FULL);
        let plan = HierPlan {
            top,
            wcells: vec![WsCell {
                key: top,
                pages: vec![0],
                insts: Vec::new(),
                frames: Vec::new(),
                washes: Vec::new(),
            }],
            pages: vec![0],
            page_prio: vec![0],
            stats: HierStats::default(),
        };
        FrameScene::from_test_parts(plan, vec![decoded], BTreeMap::from([(top, bbox)])).unwrap()
    }

    fn hairline_request() -> StyledGeometryRasterRequest {
        StyledGeometryRasterRequest {
            raster: GeometryRasterRequest {
                view: RasterViewBox::new(0.0, 0.0, 320.0, 320.0).unwrap(),
                width: 32,
                height: 32,
                workers: 1,
                tile_size: DEFAULT_TILE_SIZE,
                ..request()
            },
            layers: vec![LayerStyle {
                layer_idx: 1,
                color: [255, 255, 255, 255],
                fill: LayerFill::Solid,
                outline_width: 1,
            }],
            hierarchy_frames: false,
            mono: false,
        }
    }

    fn lit_pixels(frame: &RgbaFrame) -> Vec<(usize, usize)> {
        let mut lit = Vec::new();
        for row in 0..32 {
            for col in 0..32 {
                if pixel(frame, col, row) != [0, 0, 0, 255] {
                    lit.push((col, row));
                }
            }
        }
        lit
    }

    #[test]
    fn hairline_point_collapses_to_klayout_cell() {
        // Device x 2.4..2.9 -> round(center 2.65) = col 3; device y
        // 1.1..1.6 -> round(center 1.35) - 1 = row 0 (measured KLayout
        // y bias). Exactly one pixel, placed like the oracle.
        let rect = RectRec {
            layer: 1,
            dt: 0,
            x: 24,
            y: 304,
            w: 5,
            h: 5,
            rep: Rep::One,
        };
        let report = render_geometry_styled(
            &hairline_scene(vec![rect], Vec::new(), Vec::new()),
            &hairline_request(),
        )
        .unwrap();
        assert_eq!(lit_pixels(&report.frame), vec![(3, 0)]);
    }

    #[test]
    fn hairline_wire_lights_each_rounded_edge_column() {
        // Vertical wires 20px tall. Edges at device x 12.4/12.7 round
        // apart -> two columns; edges at 20.1/20.4 round together ->
        // one column. Rows are the edge-snapped span.
        let wire = |x, w| RectRec {
            layer: 1,
            dt: 0,
            x,
            y: 40,
            w,
            h: 200,
            rep: Rep::One,
        };
        let report = render_geometry_styled(
            &hairline_scene(vec![wire(124, 3), wire(201, 3)], Vec::new(), Vec::new()),
            &hairline_request(),
        )
        .unwrap();
        let lit = lit_pixels(&report.frame);
        let columns: std::collections::BTreeSet<usize> = lit.iter().map(|&(c, _)| c).collect();
        assert_eq!(columns.into_iter().collect::<Vec<_>>(), vec![12, 13, 20]);
        let rows: std::collections::BTreeSet<usize> = lit.iter().map(|&(_, r)| r).collect();
        assert_eq!(rows.len(), 20, "edge-snapped 20-row span");
    }

    #[test]
    fn hairline_collapse_is_representation_exact() {
        // The same sub-pixel world rect as RECTANGLE / POLYGON / PATH
        // must collapse to the same pixel (device-bbox rule).
        let rect = RectRec {
            layer: 1,
            dt: 0,
            x: 24,
            y: 304,
            w: 5,
            h: 4,
            rep: Rep::One,
        };
        let poly = PolyRec {
            layer: 1,
            dt: 0,
            pts: vec![(24, 304), (29, 304), (29, 308), (24, 308)],
            rep: Rep::One,
        };
        let path = PathRec {
            layer: 1,
            dt: 0,
            pts: vec![(24, 306), (29, 306)],
            hw: 2,
            es: 0,
            ee: 0,
            rep: Rep::One,
        };
        let request = hairline_request();
        let as_rect = render_geometry_styled(
            &hairline_scene(vec![rect], Vec::new(), Vec::new()),
            &request,
        )
        .unwrap();
        let as_poly = render_geometry_styled(
            &hairline_scene(Vec::new(), vec![poly], Vec::new()),
            &request,
        )
        .unwrap();
        let as_path = render_geometry_styled(
            &hairline_scene(Vec::new(), Vec::new(), vec![path]),
            &request,
        )
        .unwrap();
        assert_eq!(as_rect.frame.pixels(), as_poly.frame.pixels());
        assert_eq!(as_rect.frame.pixels(), as_path.frame.pixels());
        assert_eq!(lit_pixels(&as_rect.frame).len(), 1, "non-vanish, single cell");
    }

    #[test]
    fn hairline_fast_path_requires_unit_stroke_width() {
        // A 2-8px outline paints far more than the collapsed cells
        // (KLayout w4 A/B: 84,303 vs 24,158 px), so only width-1
        // solid strokes may take the fast path. Wider widths must
        // keep the full fill+stroke pipeline and stay
        // representation-exact.
        let sub_rect = RectRec {
            layer: 1,
            dt: 0,
            x: 24,
            y: 304,
            w: 5,
            h: 4,
            rep: Rep::One,
        };
        let request_with = |width: u8| {
            let mut styled = hairline_request();
            styled.layers[0].outline_width = width;
            styled
        };
        let mut previous = 0usize;
        for width in [1u8, 2, 4, 8] {
            let report = render_geometry_styled(
                &hairline_scene(vec![sub_rect.clone()], Vec::new(), Vec::new()),
                &request_with(width),
            )
            .unwrap();
            let lit = lit_pixels(&report.frame).len();
            if width == 1 {
                assert_eq!(lit, 1, "width 1 collapses to one cell");
            } else {
                assert!(
                    lit > previous,
                    "width {width} must stroke wider: {lit} vs {previous}"
                );
            }
            previous = lit;
        }
        // representation-exact must hold on the non-collapsed path too
        let poly = PolyRec {
            layer: 1,
            dt: 0,
            pts: vec![(24, 304), (29, 304), (29, 308), (24, 308)],
            rep: Rep::One,
        };
        let path = PathRec {
            layer: 1,
            dt: 0,
            pts: vec![(24, 306), (29, 306)],
            hw: 2,
            es: 0,
            ee: 0,
            rep: Rep::One,
        };
        let request = request_with(4);
        let as_rect = render_geometry_styled(
            &hairline_scene(vec![sub_rect], Vec::new(), Vec::new()),
            &request,
        )
        .unwrap();
        let as_poly = render_geometry_styled(
            &hairline_scene(Vec::new(), vec![poly], Vec::new()),
            &request,
        )
        .unwrap();
        let as_path = render_geometry_styled(
            &hairline_scene(Vec::new(), Vec::new(), vec![path]),
            &request,
        )
        .unwrap();
        assert_eq!(as_rect.frame.pixels(), as_poly.frame.pixels());
        assert_eq!(as_rect.frame.pixels(), as_path.frame.pixels());
    }

    #[test]
    fn hairline_collapse_skips_dotted_strokes() {
        let request = hairline_request().raster;
        let world = BBox {
            x0: 24,
            y0: 304,
            x1: 29,
            y1: 309,
        };
        let dotted = hairline_world_bbox(
            &request,
            world,
            PaintStyle {
                stroke: StrokeStyle::Dotted,
                ..PaintStyle::solid([255, 255, 255, 255])
            },
        )
        .unwrap();
        assert!(dotted.is_none(), "dotted frames keep their band styling");
        let solid = hairline_world_bbox(
            &request,
            world,
            PaintStyle::solid([255, 255, 255, 255]),
        )
        .unwrap();
        assert!(solid.is_some());
    }

    #[test]
    fn work_bin_matches_the_walk_byte_for_byte() {
        // The 2c gate: for hierarchy scenes with reps, masks, frames,
        // washes and hairlines, the binned render must equal the
        // per-tile walk in pixels and member paints across worker and
        // tile-size combinations.
        let with_config = |styled: &StyledGeometryRasterRequest, workers: u16, tile: u16| {
            let mut request = styled.clone();
            request.raster.workers = workers;
            request.raster.tile_size = tile;
            request
        };
        let masked = masked_request();
        let scenes: Vec<(FrameScene, StyledGeometryRasterRequest)> = vec![
            (masked_scene(false), masked.clone()),
            (
                masked_scene(false),
                StyledGeometryRasterRequest {
                    hierarchy_frames: true,
                    ..masked.clone()
                },
            ),
            (
                hairline_scene(
                    vec![RectRec {
                        layer: 1,
                        dt: 0,
                        x: 24,
                        y: 304,
                        w: 5,
                        h: 5,
                        rep: Rep::Grid {
                            na: 4,
                            nb: 3,
                            va: (40, 0),
                            vb: (0, 40),
                        },
                    }],
                    vec![PolyRec {
                        layer: 1,
                        dt: 0,
                        pts: vec![(10, 10), (200, 40), (90, 260)],
                        rep: Rep::One,
                    }],
                    vec![PathRec {
                        layer: 1,
                        dt: 0,
                        pts: vec![(20, 200), (300, 200), (300, 60)],
                        hw: 8,
                        es: 0,
                        ee: 0,
                        rep: Rep::One,
                    }],
                ),
                hairline_request(),
            ),
        ];
        for (scene, styled) in &scenes {
            for &workers in &[1u16, 4] {
                for &tile in &[8u16, DEFAULT_TILE_SIZE] {
                    let request = with_config(styled, workers, tile);
                    let walk = render_geometry_styled_unbinned(scene, &request).unwrap();
                    let bin = render_geometry_styled(scene, &request).unwrap();
                    assert!(bin.stats.work_bin_items > 0, "bin must engage");
                    assert_eq!(walk.stats.work_bin_items, 0);
                    assert_eq!(bin.frame.pixels(), walk.frame.pixels());
                    assert_eq!(
                        bin.rectangle_member_paints,
                        walk.rectangle_member_paints
                    );
                    assert_eq!(bin.polygon_member_paints, walk.polygon_member_paints);
                    assert_eq!(bin.path_member_paints, walk.path_member_paints);
                    assert_eq!(bin.frame_member_paints, walk.frame_member_paints);
                }
            }
        }
    }

    #[test]
    fn work_bin_expands_dense_repetitions_within_budget() {
        // A 70x70 instance grid (4,900 members, weight 1) projects
        // well inside the item budget, so the uniform §3.17 gate
        // expands it - per-(visit,plane) items keep the volume linear
        // in visible members, nowhere near the 768k cap - and the
        // pixels must still match the walk.
        let top = (0, REM_FULL);
        let child = (1, REM_FULL);
        let unit = BBox {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 2,
        };
        let span = BBox {
            x0: 0,
            y0: 0,
            x1: 320,
            y1: 320,
        };
        let bounds = BTreeMap::from([(top, span), (child, unit)]);
        let make_scene = || {
            let plan = HierPlan {
                top,
                wcells: vec![
                    WsCell {
                        key: top,
                        pages: Vec::new(),
                        insts: vec![WsInst {
                            child,
                            x: 2,
                            y: 2,
                            rot: 0,
                            flip: false,
                            rep: Rep::Grid {
                                na: 70,
                                nb: 70,
                                va: (4, 0),
                                vb: (0, 4),
                            },
                        }],
                        frames: Vec::new(),
                        washes: Vec::new(),
                    },
                    WsCell {
                        key: child,
                        pages: vec![0],
                        insts: Vec::new(),
                        frames: vec![(unit, Rep::One, 1)],
                        washes: Vec::new(),
                    },
                ],
                pages: vec![0],
                page_prio: vec![0],
                stats: HierStats::default(),
            };
            FrameScene::from_test_parts(
                plan,
                vec![styled_page(0, 1, unit)],
                bounds.clone(),
            )
            .unwrap()
        };
        let request = StyledGeometryRasterRequest {
            hierarchy_frames: true,
            ..hairline_request()
        };
        let walk = render_geometry_styled_unbinned(&make_scene(), &request).unwrap();
        let bin = render_geometry_styled(&make_scene(), &request).unwrap();
        assert!(
            bin.stats.work_bin_items > 4000,
            "dense grid within budget must expand: {} items",
            bin.stats.work_bin_items
        );
        assert_eq!(bin.stats.work_bin_overflow_items, 0, "no cap fallback");
        assert_eq!(bin.stats.work_bin_defer_rep, 0, "nothing deferred");
        assert_eq!(bin.stats.work_bin_defer_single, 0, "nothing deferred");
        assert_eq!(bin.frame.pixels(), walk.frame.pixels());
        assert_eq!(
            bin.rectangle_member_paints,
            walk.rectangle_member_paints
        );
        assert_eq!(bin.frame_member_paints, walk.frame_member_paints);
        assert!(bin.rectangle_member_paints > 1000, "grid must paint");
    }

    #[test]
    fn work_bin_trial_expands_past_pessimistic_projection() {
        // A 60x60 grid whose child weighs 64 projects 3,600 x 64 =
        // 230k - past the x4-factored fast gate - but its real
        // expansion (234k items) fits the trial's soft limit (half the
        // cap), so the measured trial must expand it instead of
        // trusting the pessimistic projection (§3.17 third iteration).
        let top = (0, REM_FULL);
        let mid = (1, REM_FULL);
        let leaf = (2, REM_FULL);
        let unit = BBox {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 2,
        };
        let mid_span = BBox {
            x0: 0,
            y0: 0,
            x1: 32,
            y1: 32,
        };
        let span = BBox {
            x0: 0,
            y0: 0,
            x1: 320,
            y1: 320,
        };
        let bounds = BTreeMap::from([(top, span), (mid, mid_span), (leaf, unit)]);
        let make_scene = || {
            let leaf_insts: Vec<WsInst> = (0..64)
                .map(|index| WsInst {
                    child: leaf,
                    x: (index % 8) * 4,
                    y: (index / 8) * 4,
                    rot: 0,
                    flip: false,
                    rep: Rep::One,
                })
                .collect();
            let plan = HierPlan {
                top,
                wcells: vec![
                    WsCell {
                        key: top,
                        pages: Vec::new(),
                        insts: vec![WsInst {
                            child: mid,
                            x: 2,
                            y: 2,
                            rot: 0,
                            flip: false,
                            rep: Rep::Grid {
                                na: 60,
                                nb: 60,
                                va: (4, 0),
                                vb: (0, 4),
                            },
                        }],
                        frames: Vec::new(),
                        washes: Vec::new(),
                    },
                    WsCell {
                        key: mid,
                        pages: Vec::new(),
                        insts: leaf_insts,
                        frames: Vec::new(),
                        washes: Vec::new(),
                    },
                    WsCell {
                        key: leaf,
                        pages: vec![0],
                        insts: Vec::new(),
                        frames: Vec::new(),
                        washes: Vec::new(),
                    },
                ],
                pages: vec![0],
                page_prio: vec![0],
                stats: HierStats::default(),
            };
            FrameScene::from_test_parts(
                plan,
                vec![styled_page(0, 1, unit)],
                bounds.clone(),
            )
            .unwrap()
        };
        let request = StyledGeometryRasterRequest {
            hierarchy_frames: false,
            ..hairline_request()
        };
        let walk = render_geometry_styled_unbinned(&make_scene(), &request).unwrap();
        let bin = render_geometry_styled(&make_scene(), &request).unwrap();
        assert_eq!(bin.stats.work_bin_defer_rep, 0, "trial must expand");
        assert_eq!(bin.stats.work_bin_defer_single, 0);
        assert!(
            bin.stats.work_bin_items > 200_000,
            "the measured expansion must land in the bin: {} items",
            bin.stats.work_bin_items
        );
        assert_eq!(bin.stats.work_bin_overflow_items, 0, "no cap fallback");
        assert_eq!(bin.frame.pixels(), walk.frame.pixels());
        assert_eq!(
            bin.rectangle_member_paints,
            walk.rectangle_member_paints
        );
    }

    #[test]
    fn work_bin_trial_rolls_back_edges_that_truly_overrun() {
        // A 60x60 grid of weight-129 subtrees (64 leaves, each with
        // two layers of pages and a frame) really emits ~690k items -
        // past the trial's soft limit (half the 768k cap) - so the
        // trial must stop, roll the bin and DFS path back exactly, and
        // defer that one edge. Small tiles then exercise the §3.17
        // combined mini walk end to end: multi-plane replay order,
        // frame-band replay, and per-tile view culling - all
        // byte-identical to the walk.
        let top = (0, REM_FULL);
        let mid = (1, REM_FULL);
        let leaf = (2, REM_FULL);
        let unit = BBox {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 2,
        };
        let mid_span = BBox {
            x0: 0,
            y0: 0,
            x1: 32,
            y1: 32,
        };
        let span = BBox {
            x0: 0,
            y0: 0,
            x1: 320,
            y1: 320,
        };
        let bounds = BTreeMap::from([(top, span), (mid, mid_span), (leaf, unit)]);
        let make_scene = || {
            let leaf_insts: Vec<WsInst> = (0..64)
                .map(|index| WsInst {
                    child: leaf,
                    x: (index % 8) * 4,
                    y: (index / 8) * 4,
                    rot: 0,
                    flip: false,
                    rep: Rep::One,
                })
                .collect();
            let plan = HierPlan {
                top,
                wcells: vec![
                    WsCell {
                        key: top,
                        pages: Vec::new(),
                        insts: vec![WsInst {
                            child: mid,
                            x: 2,
                            y: 2,
                            rot: 0,
                            flip: false,
                            rep: Rep::Grid {
                                na: 60,
                                nb: 60,
                                va: (4, 0),
                                vb: (0, 4),
                            },
                        }],
                        frames: Vec::new(),
                        washes: Vec::new(),
                    },
                    WsCell {
                        key: mid,
                        pages: Vec::new(),
                        insts: leaf_insts,
                        frames: Vec::new(),
                        washes: Vec::new(),
                    },
                    WsCell {
                        key: leaf,
                        pages: vec![0, 1],
                        insts: Vec::new(),
                        frames: vec![(unit, Rep::One, 1)],
                        washes: Vec::new(),
                    },
                ],
                pages: vec![0, 1],
                page_prio: vec![0, 0],
                stats: HierStats::default(),
            };
            FrameScene::from_test_parts(
                plan,
                vec![styled_page(0, 1, unit), styled_page(1, 2, unit)],
                bounds.clone(),
            )
            .unwrap()
        };
        let mut request = StyledGeometryRasterRequest {
            hierarchy_frames: true,
            ..hairline_request()
        };
        request.layers.push(LayerStyle {
            layer_idx: 2,
            color: [255, 0, 0, 255],
            fill: LayerFill::Speckle,
            outline_width: 1,
        });
        request.raster.tile_size = 8;
        let walk = render_geometry_styled_unbinned(&make_scene(), &request).unwrap();
        let bin = render_geometry_styled(&make_scene(), &request).unwrap();
        assert_eq!(bin.stats.work_bin_defer_rep, 1, "one rolled-back edge");
        assert!(
            bin.stats.work_bin_items < 100,
            "rollback must leave a tiny bin: {} items",
            bin.stats.work_bin_items
        );
        assert!(bin.stats.work_bin_defer_weight_max >= 64);
        assert_eq!(bin.stats.work_bin_overflow_items, 0, "no cap fallback");
        assert!(
            bin.stats.hier_cells_visited < walk.stats.hier_cells_visited,
            "combined mini walks must beat the per-plane walks: {} vs {}",
            bin.stats.hier_cells_visited,
            walk.stats.hier_cells_visited
        );
        assert_eq!(bin.frame.pixels(), walk.frame.pixels());
        assert_eq!(
            bin.rectangle_member_paints,
            walk.rectangle_member_paints
        );
        assert_eq!(bin.frame_member_paints, walk.frame_member_paints);
        assert!(bin.frame_member_paints > 0, "frames must replay");
    }

    #[test]
    fn work_bin_expands_heavy_single_placements_instead_of_deferring() {
        // A single placement (members=1) of a wide flat block: its
        // subtree weight (4,550) is far past WORK_BIN_DEFER_MEMBERS,
        // but deferring it makes every tile re-walk the block (§3.17
        // field: 9x the cover on a depth-limited chip view). The bin
        // must expand it - one collection walk, items independent of
        // tile count - within the item budget.
        let top = (0, REM_FULL);
        let mid = (1, REM_FULL);
        let leaf = (2, REM_FULL);
        let unit = BBox {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 2,
        };
        let span = BBox {
            x0: 0,
            y0: 0,
            x1: 320,
            y1: 320,
        };
        let bounds = BTreeMap::from([(top, span), (mid, span), (leaf, unit)]);
        let make_scene = || {
            let leaf_insts: Vec<WsInst> = (0..4550)
                .map(|index| WsInst {
                    child: leaf,
                    x: 2 + (index % 70) * 4,
                    y: 2 + (index / 70) * 4,
                    rot: 0,
                    flip: false,
                    rep: Rep::One,
                })
                .collect();
            let plan = HierPlan {
                top,
                wcells: vec![
                    WsCell {
                        key: top,
                        pages: Vec::new(),
                        insts: vec![WsInst {
                            child: mid,
                            x: 0,
                            y: 0,
                            rot: 0,
                            flip: false,
                            rep: Rep::One,
                        }],
                        frames: Vec::new(),
                        washes: Vec::new(),
                    },
                    WsCell {
                        key: mid,
                        pages: Vec::new(),
                        insts: leaf_insts,
                        frames: Vec::new(),
                        washes: Vec::new(),
                    },
                    WsCell {
                        key: leaf,
                        pages: vec![0],
                        insts: Vec::new(),
                        frames: vec![(unit, Rep::One, 1)],
                        washes: Vec::new(),
                    },
                ],
                pages: vec![0],
                page_prio: vec![0],
                stats: HierStats::default(),
            };
            FrameScene::from_test_parts(
                plan,
                vec![styled_page(0, 1, unit)],
                bounds.clone(),
            )
            .unwrap()
        };
        let mut request = StyledGeometryRasterRequest {
            hierarchy_frames: true,
            ..hairline_request()
        };
        // Many small tiles: a deferred block would multiply its walk by
        // the tile count, an expanded one is collected exactly once.
        request.raster.tile_size = 8;
        let walk = render_geometry_styled_unbinned(&make_scene(), &request).unwrap();
        let bin = render_geometry_styled(&make_scene(), &request).unwrap();
        assert!(
            bin.stats.work_bin_items > 4096,
            "single placement must expand into the bin: {} items",
            bin.stats.work_bin_items
        );
        assert_eq!(bin.stats.work_bin_overflow_items, 0, "no cap fallback");
        assert_eq!(bin.stats.work_bin_defer_rep, 0, "nothing deferred");
        assert_eq!(bin.stats.work_bin_defer_single, 0, "nothing deferred");
        assert_eq!(bin.stats.work_bin_defer_weight_max, 0);
        assert!(
            bin.stats.hier_cells_visited < walk.stats.hier_cells_visited / 4,
            "expanded bin must not re-walk the block per tile: {} vs walk {}",
            bin.stats.hier_cells_visited,
            walk.stats.hier_cells_visited
        );
        assert_eq!(bin.frame.pixels(), walk.frame.pixels());
        assert_eq!(
            bin.rectangle_member_paints,
            walk.rectangle_member_paints
        );
        assert_eq!(bin.frame_member_paints, walk.frame_member_paints);
    }

    #[test]
    fn subtree_mask_pruning_matches_full_mask_pixels() {
        let request = masked_request();
        let masked = render_geometry_styled(&masked_scene(false), &request).unwrap();
        let full =
            render_geometry_styled(&masked_scene(false).with_full_masks(), &request).unwrap();
        assert_eq!(masked.frame.pixels(), full.frame.pixels());
        assert_eq!(
            masked.rectangle_member_paints,
            full.rectangle_member_paints
        );
        assert_eq!(full.stats.subtrees_pruned, 0);
        assert!(
            masked.stats.subtrees_pruned > 0,
            "layer-1-only and deferred-only subtrees must be pruned"
        );
        assert!(
            masked.stats.hier_cells_visited < full.stats.hier_cells_visited,
            "mask must cut hierarchy visits: {} vs {}",
            masked.stats.hier_cells_visited,
            full.stats.hier_cells_visited
        );
    }

    #[test]
    fn subtree_mask_keeps_corrupt_records_reachable() {
        // The corrupt page is decoded on layer 1, so a layer-1 plane must
        // still descend into child B and surface the validation error.
        let request = StyledGeometryRasterRequest {
            layers: vec![LayerStyle {
                layer_idx: 1,
                color: [255, 0, 0, 255],
                fill: LayerFill::Solid,
                outline_width: 1,
            }],
            ..masked_request()
        };
        let error = render_geometry_styled(&masked_scene(true), &request)
            .err()
            .expect("corrupt record must still be reached");
        assert!(
            error.contains("negative rectangle size"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn frame_band_walk_prunes_frame_free_subtrees() {
        // Child A carries the only hierarchy frame; child B holds layer-0
        // geometry but no frames, so the band walks skip it whole while
        // the geometry plane still paints it.
        let top = (0, REM_FULL);
        let child_a = (1, REM_FULL);
        let child_b = (2, REM_FULL);
        let unit = BBox {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 2,
        };
        let inst = |child, x| WsInst {
            child,
            x,
            y: 2,
            rot: 0,
            flip: false,
            rep: Rep::Grid {
                na: 3,
                nb: 1,
                va: (4, 0),
                vb: (0, 0),
            },
        };
        let make_scene = || {
            let plan = HierPlan {
                top,
                wcells: vec![
                    WsCell {
                        key: top,
                        pages: Vec::new(),
                        insts: vec![inst(child_a, 2), inst(child_b, 4)],
                        frames: Vec::new(),
                        washes: Vec::new(),
                    },
                    WsCell {
                        key: child_a,
                        pages: Vec::new(),
                        insts: Vec::new(),
                        frames: vec![(unit, Rep::One, 1)],
                        washes: Vec::new(),
                    },
                    WsCell {
                        key: child_b,
                        pages: vec![0],
                        insts: Vec::new(),
                        frames: Vec::new(),
                        washes: Vec::new(),
                    },
                ],
                pages: vec![0],
                page_prio: vec![0],
                stats: HierStats::default(),
            };
            let span = BBox {
                x0: 0,
                y0: 0,
                x1: 16,
                y1: 16,
            };
            let bounds =
                BTreeMap::from([(top, span), (child_a, unit), (child_b, unit)]);
            FrameScene::from_test_parts(plan, vec![styled_page(0, 0, unit)], bounds).unwrap()
        };
        let request = StyledGeometryRasterRequest {
            layers: vec![LayerStyle {
                layer_idx: 0,
                color: [255, 0, 0, 255],
                fill: LayerFill::Solid,
                outline_width: 1,
            }],
            hierarchy_frames: true,
            ..masked_request()
        };
        // The per-plane WALK prunes the frame-free subtree; the work
        // bin's combined gate rightly keeps it (child B holds styled
        // geometry), so the prune assertions pin the unbinned path.
        let masked = render_geometry_styled_unbinned(&make_scene(), &request).unwrap();
        let full =
            render_geometry_styled_unbinned(&make_scene().with_full_masks(), &request).unwrap();
        assert_eq!(masked.frame.pixels(), full.frame.pixels());
        assert_eq!(masked.frame_member_paints, full.frame_member_paints);
        assert!(masked.frame_member_paints > 0, "frame must still paint");
        assert_eq!(full.stats.subtrees_pruned, 0);
        assert!(
            masked.stats.subtrees_pruned > 0,
            "band walk must skip the frame-free subtree"
        );
        let binned = render_geometry_styled(&make_scene(), &request).unwrap();
        assert_eq!(binned.frame.pixels(), masked.frame.pixels());
        assert_eq!(binned.frame_member_paints, masked.frame_member_paints);
    }

    #[test]
    fn subtree_mask_floods_cycles_so_their_error_stays_reachable() {
        let top = (0, REM_FULL);
        let child = (1, REM_FULL);
        let unit = BBox {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 2,
        };
        let inst = |target| WsInst {
            child: target,
            x: 0,
            y: 0,
            rot: 0,
            flip: false,
            rep: Rep::One,
        };
        let plan = HierPlan {
            top,
            wcells: vec![
                WsCell {
                    key: top,
                    pages: vec![0],
                    insts: vec![inst(child)],
                    frames: Vec::new(),
                    washes: Vec::new(),
                },
                WsCell {
                    key: child,
                    pages: Vec::new(),
                    insts: vec![inst(top)],
                    frames: Vec::new(),
                    washes: Vec::new(),
                },
            ],
            pages: vec![0],
            page_prio: vec![0],
            stats: HierStats::default(),
        };
        let bounds = BTreeMap::from([(top, unit), (child, unit)]);
        let scene =
            FrameScene::from_test_parts(plan, vec![styled_page(0, 0, unit)], bounds).unwrap();
        let request = StyledGeometryRasterRequest {
            layers: vec![LayerStyle {
                layer_idx: 0,
                color: [255, 0, 0, 255],
                fill: LayerFill::Solid,
                outline_width: 1,
            }],
            ..masked_request()
        };
        let error = render_geometry_styled(&scene, &request)
            .err()
            .expect("cycle must not be masked away");
        assert!(error.contains("hierarchy cycle"), "unexpected error: {error}");
    }

    #[test]
    fn styled_layers_share_speckle_phase_and_preserve_paint_order() {
        let scene = styled_scene(Vec::new());
        let raster = request();
        let red = LayerStyle {
            layer_idx: 0,
            color: [255, 0, 0, 255],
            fill: LayerFill::Speckle,
            outline_width: 1,
        };
        let blue = LayerStyle {
            layer_idx: 1,
            color: [0, 0, 255, 255],
            fill: LayerFill::Speckle,
            outline_width: 1,
        };
        let report = render_geometry_styled(
            &scene,
            &StyledGeometryRasterRequest {
                raster,
                layers: vec![red, blue],
                hierarchy_frames: false,
                mono: false,
            },
        )
        .unwrap();
        assert_eq!(pixel(&report.frame, 4, 4), blue.color);
        assert_eq!(pixel(&report.frame, 5, 4), raster.background);

        let blue_only = render_geometry_styled(
            &scene,
            &StyledGeometryRasterRequest {
                raster,
                layers: vec![blue],
                hierarchy_frames: false,
                mono: false,
            },
        )
        .unwrap();
        assert_eq!(pixel(&blue_only.frame, 4, 4), blue.color);
        assert_eq!(pixel(&blue_only.frame, 5, 4), raster.background);

        let mono = render_geometry_styled(
            &scene,
            &StyledGeometryRasterRequest {
                raster,
                layers: vec![blue],
                hierarchy_frames: false,
                mono: true,
            },
        )
        .unwrap();
        assert_eq!(pixel(&mono.frame, 4, 4), [29, 29, 29, 255]);

        let parallel = render_geometry_styled(
            &scene,
            &StyledGeometryRasterRequest {
                raster: GeometryRasterRequest {
                    workers: 4,
                    ..raster
                },
                layers: vec![red, blue],
                hierarchy_frames: false,
                mono: false,
            },
        )
        .unwrap();
        assert_eq!(parallel.frame.pixels, report.frame.pixels);
    }

    #[test]
    fn styled_custom_pattern_uses_klayout_frame_height_phase() {
        let scene = styled_scene(Vec::new());
        let mut rows = [0u16; 16];
        // A 10px-tall frame maps device row 4 to source row 13.
        rows[13] = 1 << 11;
        let report = render_geometry_styled(
            &scene,
            &StyledGeometryRasterRequest {
                raster: request(),
                layers: vec![LayerStyle {
                    layer_idx: 0,
                    color: [0, 255, 0, 255],
                    fill: LayerFill::Pattern(rows),
                    outline_width: 1,
                }],
                hierarchy_frames: false,
                mono: false,
            },
        )
        .unwrap();
        assert_eq!(pixel(&report.frame, 4, 4), [0, 255, 0, 255]);
        assert_eq!(pixel(&report.frame, 4, 5), request().background);
    }

    #[test]
    fn styled_path_strokes_its_original_centerline() {
        let request = request();
        let mut band = full_band(&request);
        let paint = PaintStyle {
            color: [0, 255, 0, 255],
            fill: LayerFill::Clear,
            stroke: StrokeStyle::Solid,
            stroke_width: 1,
        };
        paint_world_path(
            &mut band,
            &request,
            &[(1, 3), (9, 3), (9, 7), (1, 7)],
            &[(1, 5), (9, 5)],
            paint,
        )
        .unwrap();
        let band_pixel = |x: usize, y: usize| -> [u8; 4] {
            let offset = (y * request.width as usize + x) * 4;
            band.pixels[offset..offset + 4].try_into().unwrap()
        };
        assert_eq!(band_pixel(5, 4), paint.color);
        assert_eq!(band_pixel(5, 5), request.background);
    }

    #[test]
    fn hierarchy_frames_stack_gray_under_design_and_white_over_it() {
        let scene = styled_scene(vec![
            (
                BBox {
                    x0: 3,
                    y0: 3,
                    x1: 7,
                    y1: 7,
                },
                Rep::One,
                0,
            ),
            (
                BBox {
                    x0: 2,
                    y0: 2,
                    x1: 8,
                    y1: 8,
                },
                Rep::One,
                1,
            ),
            (
                BBox {
                    x0: 0,
                    y0: 0,
                    x1: 2,
                    y1: 2,
                },
                Rep::One,
                2,
            ),
            (
                BBox {
                    x0: 0,
                    y0: 8,
                    x1: 2,
                    y1: 10,
                },
                Rep::One,
                3,
            ),
        ]);
        let report = render_geometry_styled(
            &scene,
            &StyledGeometryRasterRequest {
                raster: request(),
                layers: vec![LayerStyle {
                    layer_idx: 0,
                    color: [255, 0, 0, 255],
                    fill: LayerFill::Solid,
                    outline_width: 1,
                }],
                hierarchy_frames: true,
                mono: false,
            },
        )
        .unwrap();
        assert_eq!(pixel(&report.frame, 2, 7), [255, 0, 0, 255]);
        assert_eq!(pixel(&report.frame, 3, 6), [255, 255, 255, 255]);
        assert_eq!(pixel(&report.frame, 0, 9), [128, 128, 128, 255]);
        assert_eq!(report.frame_record_tests, 4);
        assert_eq!(report.frame_member_paints, 4);
        assert_eq!(report.deferred_frame_tests, 0);
    }

    #[test]
    fn tile_sizes_and_worker_counts_are_byte_identical() {
        let scene = styled_scene(vec![(
            BBox {
                x0: 1,
                y0: 1,
                x1: 9,
                y1: 9,
            },
            Rep::One,
            0,
        )]);
        let style = LayerStyle {
            layer_idx: 0,
            color: [231, 41, 97, 255],
            fill: LayerFill::Speckle,
            outline_width: 4,
        };
        let baseline = render_geometry_styled(
            &scene,
            &StyledGeometryRasterRequest {
                raster: request(),
                layers: vec![style],
                hierarchy_frames: true,
                mono: false,
            },
        )
        .unwrap()
        .frame;

        for tile_size in [1, 3, 4, 7, DEFAULT_TILE_SIZE] {
            for workers in [1, 4] {
                let mut raster = request();
                raster.tile_size = tile_size;
                raster.workers = workers;
                let report = render_geometry_styled(
                    &scene,
                    &StyledGeometryRasterRequest {
                        raster,
                        layers: vec![style],
                        hierarchy_frames: true,
                        mono: false,
                    },
                )
                .unwrap();
                assert_eq!(report.frame, baseline, "tile={tile_size} workers={workers}");
            }
        }
    }

    #[test]
    fn styled_request_rejects_duplicate_layer_planes() {
        let layer = LayerStyle {
            layer_idx: 7,
            color: [1, 2, 3, 255],
            fill: LayerFill::Solid,
            outline_width: 1,
        };
        let error = StyledGeometryRasterRequest {
            raster: request(),
            layers: vec![layer, layer],
            hierarchy_frames: false,
            mono: false,
        }
        .validate()
        .unwrap_err();
        assert!(error.contains("duplicate styled layer"));

        let error = StyledGeometryRasterRequest {
            raster: request(),
            layers: vec![LayerStyle {
                outline_width: 0,
                ..layer
            }],
            hierarchy_frames: false,
            mono: false,
        }
        .validate()
        .unwrap_err();
        assert!(error.contains("outline width"));
    }

    #[test]
    fn cancellable_render_rejects_only_stale_generations() {
        let scene = styled_scene(Vec::new());
        let cancellation = RenderCancellation::new();
        cancellation.cancel_before(10);
        let request = StyledGeometryRasterRequest {
            raster: request(),
            layers: vec![LayerStyle {
                layer_idx: 0,
                color: [255, 0, 0, 255],
                fill: LayerFill::Solid,
                outline_width: 1,
            }],
            hierarchy_frames: false,
            mono: false,
        };
        let error = render_geometry_styled_cancellable(&scene, &request, 9, &cancellation)
            .err()
            .expect("stale generation must be cancelled");
        assert!(error.contains("render cancelled"));
        assert!(render_geometry_styled_cancellable(&scene, &request, 10, &cancellation).is_ok());
    }

    #[test]
    fn renders_hierarchy_grid_without_scene_expansion() {
        let top = (0, REM_FULL);
        let child = (1, REM_FULL);
        let plan = HierPlan {
            top,
            wcells: vec![
                WsCell {
                    key: top,
                    pages: Vec::new(),
                    insts: vec![WsInst {
                        child,
                        x: 2,
                        y: 0,
                        rot: 0,
                        flip: false,
                        rep: Rep::Grid {
                            na: 2,
                            nb: 1,
                            va: (4, 0),
                            vb: (0, 0),
                        },
                    }],
                    frames: Vec::new(),
                    washes: Vec::new(),
                },
                WsCell {
                    key: child,
                    pages: vec![0],
                    insts: Vec::new(),
                    frames: Vec::new(),
                    washes: Vec::new(),
                },
            ],
            pages: vec![0],
            page_prio: vec![0],
            stats: HierStats::default(),
        };
        let decoded_doc = Doc {
            unit: 1000.0,
            cells: vec![Cell {
                name: "P".to_string(),
                rects: vec![RectRec {
                    layer: 1,
                    dt: 0,
                    x: 0,
                    y: 0,
                    w: 2,
                    h: 2,
                    rep: Rep::One,
                }],
                polys: vec![PolyRec {
                    layer: 1,
                    dt: 0,
                    pts: vec![(2, 0), (4, 0), (4, 4), (2, 4)],
                    rep: Rep::One,
                }],
                paths: vec![
                    PathRec {
                        layer: 1,
                        dt: 0,
                        pts: vec![(0, 1), (2, 1)],
                        hw: 1,
                        es: 0,
                        ee: 0,
                        rep: Rep::One,
                    },
                    PathRec {
                        layer: 1,
                        dt: 0,
                        pts: vec![(0, 0), (2, 2)],
                        hw: 1,
                        es: 0,
                        ee: 0,
                        rep: Rep::One,
                    },
                ],
                ..Cell::default()
            }],
            top: 0,
            layer_order: vec![(1, 0)],
            norm_s: 0.0,
            layer_names: HashMap::new(),
            layer_aliases: HashMap::new(),
        };
        let decoded = Arc::new(DecodedPage {
            page_id: 0,
            layer_idx: 0,
            bbox: BBox {
                x0: 0,
                y0: 0,
                x1: 4,
                y1: 4,
            },
            encoded_bytes: 1,
            records: 1,
            members: 1,
            index: crate::PageIndex::build(&decoded_doc),
            doc: decoded_doc,
        });
        let mut bounds = BTreeMap::new();
        bounds.insert(
            top,
            BBox {
                x0: 2,
                y0: 0,
                x1: 10,
                y1: 4,
            },
        );
        bounds.insert(child, decoded.bbox);
        let scene = FrameScene::from_test_parts(plan, vec![decoded], bounds).unwrap();
        let mut raster_request = GeometryRasterRequest {
            view: RasterViewBox::new(0.0, 0.0, 10.0, 5.0).unwrap(),
            width: 10,
            height: 4,
            background: [0, 0, 0, 255],
            foreground: [255, 255, 255, 255],
            workers: 3,
            tile_size: DEFAULT_TILE_SIZE,
        };
        let report = render_geometry_occupancy(&scene, &raster_request).unwrap();
        raster_request.workers = 1;
        let serial = render_geometry_occupancy(&scene, &raster_request).unwrap();
        assert_eq!(report.frame, serial.frame);
        let lit: Vec<(usize, usize)> = report
            .frame
            .pixels()
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, pixel)| pixel[0] == 255)
            .map(|(index, _)| (index % 10, index / 10))
            .collect();
        assert_eq!(
            lit,
            vec![
                (4, 0),
                (5, 0),
                (6, 0),
                (8, 0),
                (9, 0),
                (2, 1),
                (3, 1),
                (4, 1),
                (5, 1),
                (6, 1),
                (7, 1),
                (8, 1),
                (9, 1),
                (1, 2),
                (2, 2),
                (3, 2),
                (4, 2),
                (5, 2),
                (6, 2),
                (7, 2),
                (8, 2),
                (9, 2),
                (1, 3),
                (2, 3),
                (3, 3),
                (4, 3),
                (5, 3),
                (6, 3),
                (7, 3),
                (8, 3),
                (9, 3),
            ]
        );
        assert_eq!(serial.rectangle_member_paints, 2);
        assert_eq!(serial.polygon_member_paints, 2);
        assert_eq!(serial.path_member_paints, 4);
        assert_eq!(serial.path_record_tests, 4);
    }

    fn design_label(text: &str, x: i64, y: i64, rotation: u8, layer_idx: u32) -> RenderLabel {
        RenderLabel {
            block: false,
            white: false,
            layer_idx: Some(layer_idx),
            x,
            y,
            rotation,
            text: text.to_string(),
        }
    }

    fn label_request(workers: u16, tile_size: u16) -> GeometryRasterRequest {
        GeometryRasterRequest {
            view: RasterViewBox::new(0.0, 0.0, 10.0, 10.0).unwrap(),
            width: 100,
            height: 100,
            background: [0, 0, 0, 255],
            foreground: [255, 255, 255, 255],
            workers,
            tile_size,
        }
    }

    fn colored_bounds(frame: &RgbaFrame, color_channel: usize) -> (usize, usize, usize, usize) {
        let mut bounds = (usize::MAX, usize::MAX, 0usize, 0usize);
        for (index, rgba) in frame.pixels().chunks_exact(4).enumerate() {
            let x = index % frame.width() as usize;
            let y = index / frame.width() as usize;
            if (30..70).contains(&x) && (30..70).contains(&y) && rgba[color_channel] != 0 {
                bounds.0 = bounds.0.min(x);
                bounds.1 = bounds.1.min(y);
                bounds.2 = bounds.2.max(x);
                bounds.3 = bounds.3.max(y);
            }
        }
        bounds
    }

    #[test]
    fn bundled_labels_center_rotate_and_follow_layer_visibility() {
        let red = LayerStyle {
            layer_idx: 0,
            color: [255, 0, 0, 255],
            fill: LayerFill::Clear,
            outline_width: 1,
        };
        let render = |rotation, layers: Vec<LayerStyle>| {
            render_geometry_styled(
                &styled_scene_with_labels(Vec::new(), vec![design_label("AB", 5, 5, rotation, 0)]),
                &StyledGeometryRasterRequest {
                    raster: label_request(2, 16),
                    layers,
                    hierarchy_frames: false,
                    mono: false,
                },
            )
            .unwrap()
        };
        let horizontal = render(0, vec![red]);
        let vertical = render(1, vec![red]);
        let hb = colored_bounds(&horizontal.frame, 0);
        let vb = colored_bounds(&vertical.frame, 0);
        assert!(hb.0 < 50 && hb.2 >= 50 && hb.1 < 50 && hb.3 >= 50);
        assert!(vb.0 < 50 && vb.2 >= 50 && vb.1 < 50 && vb.3 >= 50);
        assert_eq!(hb.2 - hb.0, vb.3 - vb.1);
        assert_eq!(hb.3 - hb.1, vb.2 - vb.0);
        assert!(horizontal.label_pixel_paints > 0);
        assert!(vertical.label_pixel_paints > 0);

        let hidden = render(
            0,
            vec![LayerStyle {
                layer_idx: 1,
                color: [0, 0, 255, 255],
                fill: LayerFill::Clear,
                outline_width: 1,
            }],
        );
        assert_eq!(hidden.label_pixel_paints, 0);
        assert_eq!(colored_bounds(&hidden.frame, 0).0, usize::MAX);
    }

    #[test]
    fn label_pixels_are_identical_across_tiles_and_worker_counts() {
        let labels = (0..4)
            .map(|rotation| design_label("VDD_PIN", 5, 5, rotation, 0))
            .collect();
        let scene = styled_scene_with_labels(Vec::new(), labels);
        let style = LayerStyle {
            layer_idx: 0,
            color: [31, 211, 97, 255],
            fill: LayerFill::Clear,
            outline_width: 1,
        };
        let render = |workers, tile_size| {
            render_geometry_styled(
                &scene,
                &StyledGeometryRasterRequest {
                    raster: label_request(workers, tile_size),
                    layers: vec![style],
                    hierarchy_frames: false,
                    mono: false,
                },
            )
            .unwrap()
        };
        let serial = render(1, 100);
        let parallel = render(8, 13);
        assert_eq!(serial.frame, parallel.frame);
        assert_eq!(serial.label_pixel_paints, parallel.label_pixel_paints);
        assert!(parallel.label_tile_paints > serial.label_tile_paints);
    }

    #[test]
    fn oversized_label_is_truncated_without_losing_geometry() {
        let label = design_label(&"X".repeat(MAX_LABEL_GLYPHS + 1), 5, 5, 0, 0);
        let scene = styled_scene_with_labels(Vec::new(), vec![label]);
        let report = render_geometry_styled(
            &scene,
            &StyledGeometryRasterRequest {
                raster: label_request(2, 16),
                layers: vec![LayerStyle {
                    layer_idx: 0,
                    color: [255, 0, 0, 255],
                    fill: LayerFill::Solid,
                    outline_width: 1,
                }],
                hierarchy_frames: false,
                mono: false,
            },
        )
        .unwrap();

        assert!(report.labels_truncated);
        assert_eq!(report.label_pixel_paints, 0);
        assert!(report.rectangle_member_paints > 0);
    }

    #[test]
    fn bundled_font_pixels_match_golden_crc32() {
        let labels = (0..4)
            .map(|rotation| design_label("Floe_19", 5, 5, rotation, 2))
            .collect();
        let scene = styled_scene_with_label_font(Vec::new(), labels, 19.0);
        let report = render_geometry_styled(
            &scene,
            &StyledGeometryRasterRequest {
                raster: label_request(7, 13),
                layers: vec![LayerStyle {
                    layer_idx: 2,
                    color: [29, 211, 103, 220],
                    fill: LayerFill::Clear,
                    outline_width: 1,
                }],
                hierarchy_frames: false,
                mono: false,
            },
        )
        .unwrap();
        assert_eq!(report.label_pixel_paints, 1_852);
        assert_eq!(crc32fast::hash(report.frame.pixels()), 0xfa90_edf6);
    }

    #[test]
    fn label_coverage_blends_style_alpha_deterministically() {
        let mut target = [10, 20, 30, 40];
        blend_text_pixel(&mut target, [110, 120, 130, 140], 128);
        assert_eq!(target, [60, 70, 80, 90]);
        blend_text_pixel(&mut target, [1, 2, 3, 4], 255);
        assert_eq!(target, [1, 2, 3, 4]);
    }

    #[test]
    fn block_label_tone_obeys_frame_paint_stack() {
        let labels = vec![
            RenderLabel {
                block: true,
                white: false,
                layer_idx: None,
                x: 3,
                y: 5,
                rotation: 0,
                text: "GRAY".to_string(),
            },
            RenderLabel {
                block: true,
                white: true,
                layer_idx: None,
                x: 7,
                y: 5,
                rotation: 0,
                text: "WHITE".to_string(),
            },
        ];
        let report = render_geometry_styled(
            &styled_scene_with_labels(Vec::new(), labels),
            &StyledGeometryRasterRequest {
                raster: label_request(4, 16),
                layers: Vec::new(),
                hierarchy_frames: true,
                mono: false,
            },
        )
        .unwrap();
        let gray_max = report
            .frame
            .pixels()
            .chunks_exact(4)
            .enumerate()
            .filter(|(index, _)| (15..45).contains(&(index % 100)))
            .map(|(_, rgba)| rgba[0])
            .max()
            .unwrap();
        let white_max = report
            .frame
            .pixels()
            .chunks_exact(4)
            .enumerate()
            .filter(|(index, _)| (55..90).contains(&(index % 100)))
            .map(|(_, rgba)| rgba[0])
            .max()
            .unwrap();
        assert_eq!(gray_max, 128);
        assert_eq!(white_max, 255);
    }
}
