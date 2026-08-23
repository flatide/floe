use floe_ovm::BBox;
use floe_vfs::hier::WsKey;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::repetition::for_each_visible_offset;
use crate::transform::OrthoTransform;
use crate::{FrameScene, RenderCancellation, RenderStats, ViewBox};

const MAX_IMAGE_PIXELS: u64 = 268_435_456;
const MAX_WORKERS: u16 = 256;
pub const MAX_TILE_SIZE: u16 = 4096;
pub const DEFAULT_TILE_SIZE: u16 = 128;
const DEVICE_ONE: i128 = 1i128 << 32;
const DEVICE_HALF: i128 = DEVICE_ONE / 2;

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
    pub partial: bool,
}

pub fn render_geometry_occupancy(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
) -> Result<GeometryRasterReport, String> {
    request.validate()?;
    render_geometry(scene, request, RenderMode::Occupancy, None)
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
    )
}

pub fn render_geometry_styled(
    scene: &FrameScene,
    request: &StyledGeometryRasterRequest,
) -> Result<GeometryRasterReport, String> {
    request.validate()?;
    render_geometry(scene, &request.raster, RenderMode::Styled(request), None)
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
    )
}

#[derive(Clone, Copy)]
enum RenderMode<'a> {
    Occupancy,
    Styled(&'a StyledGeometryRasterRequest),
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
) -> Result<GeometryRasterReport, String> {
    check_cancelled(guard)?;
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
    let next_tile = AtomicUsize::new(0);
    let tiles = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let next_tile = &next_tile;
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
                    outputs.push(raster_tile(
                        scene, request, mode, guard, col0, col1, row0, row1,
                    )?);
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

#[allow(clippy::too_many_arguments)]
fn raster_tile(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    mode: RenderMode<'_>,
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
                PaintStyle::solid(request.foreground),
                guard,
                scene.top(),
                OrthoTransform::identity(),
                &mut path,
            )?;
        }
        RenderMode::Styled(styled) => {
            if styled.hierarchy_frames {
                for frame_band in [2, 3, 1] {
                    check_cancelled(guard)?;
                    render_frame_band(
                        scene,
                        request,
                        &mut band,
                        cull_view,
                        &mut stats,
                        &mut counters,
                        frame_band,
                        frame_paint(frame_band),
                        guard,
                        scene.top(),
                        OrthoTransform::identity(),
                        &mut path,
                    )?;
                }
            }
            for layer in &styled.layers {
                check_cancelled(guard)?;
                render_cell(
                    scene,
                    request,
                    &mut band,
                    cull_view,
                    &mut stats,
                    &mut counters,
                    GeometrySelection::Layer(layer.layer_idx),
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
                    &mut path,
                )?;
            }
            if styled.hierarchy_frames {
                check_cancelled(guard)?;
                render_frame_band(
                    scene,
                    request,
                    &mut band,
                    cull_view,
                    &mut stats,
                    &mut counters,
                    0,
                    frame_paint(0),
                    guard,
                    scene.top(),
                    OrthoTransform::identity(),
                    &mut path,
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
    path.push(key);
    let local_view = world_transform.invert()?.apply_bbox(cull_view)?;

    for &page_id in &cell.pages {
        check_cancelled(guard)?;
        let Some(page) = scene.page(page_id) else {
            continue;
        };
        if !selection.includes(page.layer_idx) {
            continue;
        }
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
        counters.rect_records = counters
            .rect_records
            .saturating_add(geometry.rects.len().try_into().unwrap_or(u64::MAX));
        counters.polygon_records = counters
            .polygon_records
            .saturating_add(geometry.polys.len().try_into().unwrap_or(u64::MAX));
        counters.path_records = counters
            .path_records
            .saturating_add(geometry.paths.len().try_into().unwrap_or(u64::MAX));

        for rect in &geometry.rects {
            stats.primitives_tested = stats.primitives_tested.saturating_add(1);
            if rect.w < 0 || rect.h < 0 {
                return Err(format!(
                    "corrupt page {}: negative rectangle size {}x{}",
                    page_id, rect.w, rect.h
                ));
            }
            if rect.w == 0 || rect.h == 0 {
                continue;
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
            let visit =
                for_each_visible_offset(&rect.rep, base, local_view, |offset_x, offset_y| {
                    check_member_cancelled(guard, &mut cancel_member)?;
                    let local = translate_bbox(base, offset_x, offset_y)?;
                    let world = world_transform.apply_bbox(local)?;
                    if paint_world_rect(band, request, world, paint)? {
                        drawn = drawn.saturating_add(1);
                    }
                    Ok(())
                })?;
            stats.rep_members_tested = stats.rep_members_tested.saturating_add(visit.tested);
            stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(drawn);
            stats.primitives_drawn = stats.primitives_drawn.saturating_add(drawn);
            counters.rectangle_members_drawn =
                counters.rectangle_members_drawn.saturating_add(drawn);
        }

        for polygon in &geometry.polys {
            stats.primitives_tested = stats.primitives_tested.saturating_add(1);
            let base = polygon_bbox(&polygon.pts).ok_or_else(|| {
                format!(
                    "corrupt page {}: polygon has fewer than 3 vertices",
                    page_id
                )
            })?;
            let mut drawn = 0u64;
            let mut cancel_member = 0u16;
            let visit =
                for_each_visible_offset(&polygon.rep, base, local_view, |offset_x, offset_y| {
                    check_member_cancelled(guard, &mut cancel_member)?;
                    let mut world_points = Vec::with_capacity(polygon.pts.len());
                    for &(x, y) in &polygon.pts {
                        let x = checked_add(x, offset_x, "polygon x")?;
                        let y = checked_add(y, offset_y, "polygon y")?;
                        world_points.push(world_transform.apply(x, y)?);
                    }
                    if paint_world_polygon(band, request, &world_points, paint)? {
                        drawn = drawn.saturating_add(1);
                    }
                    Ok(())
                })?;
            stats.rep_members_tested = stats.rep_members_tested.saturating_add(visit.tested);
            stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(drawn);
            stats.primitives_drawn = stats.primitives_drawn.saturating_add(drawn);
            counters.polygon_members_drawn = counters.polygon_members_drawn.saturating_add(drawn);
        }

        for path_record in &geometry.paths {
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
            let base = polygon_bbox(&outline)
                .ok_or_else(|| format!("corrupt page {}: path outline is degenerate", page_id))?;
            let mut drawn = 0u64;
            let mut cancel_member = 0u16;
            let visit = for_each_visible_offset(
                &path_record.rep,
                base,
                local_view,
                |offset_x, offset_y| {
                    check_member_cancelled(guard, &mut cancel_member)?;
                    let mut world_points = Vec::with_capacity(outline.len());
                    for &(x, y) in &outline {
                        let x = checked_add(x, offset_x, "path x")?;
                        let y = checked_add(y, offset_y, "path y")?;
                        world_points.push(world_transform.apply(x, y)?);
                    }
                    let mut world_centerline = Vec::with_capacity(centerline.len());
                    for &(x, y) in &centerline {
                        let x = checked_add(x, offset_x, "path centerline x")?;
                        let y = checked_add(y, offset_y, "path centerline y")?;
                        world_centerline.push(world_transform.apply(x, y)?);
                    }
                    if paint_world_path(band, request, &world_points, &world_centerline, paint)? {
                        drawn = drawn.saturating_add(1);
                    }
                    Ok(())
                },
            )?;
            stats.rep_members_tested = stats.rep_members_tested.saturating_add(visit.tested);
            stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(drawn);
            stats.primitives_drawn = stats.primitives_drawn.saturating_add(drawn);
            counters.path_members_drawn = counters.path_members_drawn.saturating_add(drawn);
        }
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
    path.push(key);
    let local_view = world_transform.invert()?.apply_bbox(cull_view)?;

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

    for instance in &cell.insts {
        check_cancelled(guard)?;
        let child_bbox = scene
            .cell_bbox(instance.child)
            .ok_or_else(|| format!("invalid plan: missing bbox for child {:?}", instance.child))?;
        if child_bbox.is_empty() {
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

fn checked_path_outline(
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

fn checked_path_centerline(points: &[(i64, i64)]) -> Result<Option<Vec<(i64, i64)>>, String> {
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

fn paint_world_rect(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    world: BBox,
    paint: PaintStyle,
) -> Result<bool, String> {
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
    let centered =
        fill_world_polygon_with_phase(band, request, points, FillPhase::PixelCenter, paint)?;
    let boundary =
        fill_world_polygon_with_phase(band, request, points, FillPhase::LowerBoundary, paint)?;
    let stroked = stroke_world_polygon(band, request, points, paint)?;
    Ok(centered || boundary || stroked)
}

fn paint_world_path_outline(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    points: &[(i64, i64)],
    paint: PaintStyle,
) -> Result<bool, String> {
    let filled =
        fill_world_polygon_with_phase(band, request, points, FillPhase::LowerBoundary, paint)?;
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
    let Some((col0, col1, row0, row1)) = world_rect_pixel_bounds(request, world) else {
        return Ok(false);
    };
    let col0 = col0.max(band.col0);
    let col1 = col1.min(band.col1);
    let row0 = row0.max(band.row0);
    let row1 = row1.min(band.row1);
    if col0 >= col1 || row0 >= row1 {
        return Ok(false);
    }
    let mut drew = false;
    for row in row0..row1 {
        let local_row = row - band.row0;
        for col in col0..col1 {
            if !paint.fills(row, col, request.height) {
                continue;
            }
            let local_col = col - band.col0;
            let offset = (local_row as usize * band.tile_width() as usize + local_col as usize) * 4;
            band.pixels[offset..offset + 4].copy_from_slice(&paint.color);
            drew = true;
        }
    }
    Ok(drew)
}

fn world_rect_pixel_bounds(
    request: &GeometryRasterRequest,
    world: BBox,
) -> Option<(u32, u32, u32, u32)> {
    let view = request.view;
    let x0 = (world.x0 as f64).max(view.x0);
    let y0 = (world.y0 as f64).max(view.y0);
    let x1 = (world.x1 as f64).min(view.x1);
    let y1 = (world.y1 as f64).min(view.y1);
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    let span_x = view.x1 - view.x0;
    let span_y = view.y1 - view.y0;
    let col0 = scale_floor_f64(x0 - view.x0, request.width, span_x);
    let col1 = scale_ceil_f64(x1 - view.x0, request.width, span_x);
    let row0 = scale_floor_f64(view.y1 - y1, request.height, span_y);
    let row1 = scale_ceil_f64(view.y1 - y0, request.height, span_y);
    if col0 >= col1 || row0 >= row1 {
        return None;
    }
    Some((col0, col1, row0, row1))
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
    let mut drew = false;
    for index in 0..points.len() {
        let start = world_to_stroke_vertex(request, points[index])?;
        let end = world_to_stroke_vertex(request, points[(index + 1) % points.len()])?;
        if stroke_device_segment(band, request, start, end, paint)? {
            drew = true;
        }
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
    let mut drew = false;
    for segment in points.windows(2) {
        let start = world_to_stroke_vertex(request, segment[0])?;
        let end = world_to_stroke_vertex(request, segment[1])?;
        if stroke_device_segment(band, request, start, end, paint)? {
            drew = true;
        }
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
        let (first_row, end_row) = match phase {
            // Both sampling phases use a half-open y rule so shared vertices
            // always contribute exactly one incident edge.
            FillPhase::PixelCenter => (
                floor_div(y0 - DEVICE_HALF, DEVICE_ONE) + 1,
                floor_div(y1 - DEVICE_HALF, DEVICE_ONE) + 1,
            ),
            FillPhase::LowerBoundary => (floor_div(y0, DEVICE_ONE), floor_div(y1, DEVICE_ONE)),
        };
        if first_row >= end_row {
            continue;
        }
        edges.push(ActiveEdge {
            x0,
            y0,
            dx: x1 - x0,
            dy: y1 - y0,
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
        let mut intersections = Vec::with_capacity(active.len());
        for edge in &active {
            let rise = scan_y - edge.y0;
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
            let (first_col, end_col) = match phase {
                FillPhase::PixelCenter => (
                    floor_div(pair[0] - DEVICE_HALF, DEVICE_ONE) + 1,
                    floor_div(pair[1] - DEVICE_HALF, DEVICE_ONE) + 1,
                ),
                FillPhase::LowerBoundary => (
                    floor_div(pair[0], DEVICE_ONE) + 1,
                    ceil_div(pair[1], DEVICE_ONE),
                ),
            };
            let first_col = first_col.max(band.col0 as i128);
            let end_col = end_col.min(band.col1 as i128);
            if first_col >= end_col {
                continue;
            }
            let first_col = checked_usize(first_col, "polygon first column")?;
            let end_col = checked_usize(end_col, "polygon end column")?;
            let row_u32: u32 = row
                .try_into()
                .map_err(|_| format!("limit exceeded: polygon row = {}", row))?;
            let local_row = row as usize - band.row0 as usize;
            for col in first_col..end_col {
                let col_u32: u32 = col
                    .try_into()
                    .map_err(|_| format!("limit exceeded: polygon column = {}", col))?;
                if !paint.fills(row_u32, col_u32, request.height) {
                    continue;
                }
                let local_col = col - band.col0 as usize;
                let offset = (local_row * band.tile_width() as usize + local_col) * 4;
                band.pixels[offset..offset + 4].copy_from_slice(&paint.color);
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
    if !value.is_finite() || value < i128::MIN as f64 || value > i128::MAX as f64 {
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
    -floor_div(-numerator, denominator)
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

fn scale_floor_f64(offset: f64, pixels: u32, span: f64) -> u32 {
    (offset * pixels as f64 / span)
        .floor()
        .clamp(0.0, pixels as f64) as u32
}

fn scale_ceil_f64(offset: f64, pixels: u32, span: f64) -> u32 {
    (offset * pixels as f64 / span)
        .ceil()
        .clamp(0.0, pixels as f64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DecodedPage;
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
        Arc::new(DecodedPage {
            page_id,
            layer_idx,
            bbox,
            encoded_bytes: 1,
            records: 1,
            members: 1,
            doc: Doc {
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
            },
        })
    }

    fn styled_scene(frames: Vec<(BBox, Rep, u8)>) -> FrameScene {
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
        FrameScene::from_test_parts(
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
        let decoded = Arc::new(DecodedPage {
            page_id,
            layer_idx: 0,
            bbox,
            encoded_bytes: 1,
            records: 1,
            members: 1,
            doc: Doc {
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
            },
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
            doc: Doc {
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
            },
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
}
