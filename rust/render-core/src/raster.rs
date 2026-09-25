use floe_ovm::BBox;
use floe_vfs::hier::WsKey;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::font::{normalized_chars, GlyphAtlas};
use crate::page_index::RecordSet;
use crate::repetition::{for_each_visible_offset, for_each_visible_offset_chunked, visible_grid_range, RepVisit};
use floe_oasis::doc::Rep;
use crate::transform::OrthoTransform;
use crate::{FrameScene, RenderCancellation, RenderStats, ViewBox, PLACE_WALK_OUTCOMES};

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

    pub(crate) fn validate(&self) -> Result<(), String> {
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
    /// Area-true drawing (2026-09-22): a shape lights the pixels whose centres
    /// it covers and its outline is the rim of those pixels, so a w px shape
    /// lights w px on average and a g px gap stays g px (the KLayout rule
    /// grows every shape by about a pixel per axis and closes gaps up to
    /// ~1.5 px); a shape under a pixel on a side is kept with the chance its
    /// area fills the pixels it would light, ranked by its world box.
    /// false = the KLayout-measured rule (exact renders, the oracle gates).
    pub area_true: bool,
    /// The extra-sparsening strength of the width-first rule
    /// (ADAPTIVE_CUT_DENSITY_PLAN §4.2 candidate 1, diagnostic only): a
    /// rectangle's side of w px draws floor(w) + 1 px when its rank t is
    /// under P_c(frac(w)) = f / (c - (c - 1) f). c = 1 is the rule itself
    /// (the extra pixel with chance f, the mean width w); c > 1 darkens
    /// on purpose - c = 2 keeps a 0.05 px side 1 time in 39 instead of 1 in
    /// 20 - and stays monotone in w and continuous at whole widths, so the
    /// picture only thins, never jumps. renderd sets it from
    /// FLOE_RUST_WIDTH_C (default 1); every other request uses 1.
    pub width_c: f64,
    /// List the survivors of a sub-pixel array instead of testing every
    /// member (ADAPTIVE_CUT_DENSITY_PLAN §4.3 step 1, connected 2026-09-25;
    /// GridRanks::survivors): the same pixels, fewer members walked. renderd
    /// turns it off under FLOE_RUST_SURVIVOR_LIST=off (the kill switch).
    pub survivor_list: bool,
    /// The placement lattice (CUT_DENSITY_DESIGN §10.8, diagnostic, default
    /// off; renderd FLOE_RUST_PLACE_LATTICE=on): a single shape reached
    /// through a placement array whose vectors run along the world axes ranks
    /// on that array's world lattice - the ranks a flat array of the shape
    /// would take - and a flat polygon or path array ranks its sub-pixel keep
    /// on its own lattice; with `survivor_list` a placement array of a small
    /// cell then visits only the members whose shapes can survive.
    pub place_lattice: bool,
    /// Sub-cut arrays (CUT_DENSITY_DESIGN §10.9, diagnostic, default off;
    /// renderd FLOE_RUST_SHAPE_CUT=arrays, with shape_cut_max): a record under
    /// the per-shape cut stays when it is a whole array ranked on its world
    /// lattice - its members are drawn by the area-true rule, thinned by their
    /// size and walked by survivors - while any other record under the cut
    /// goes as before; polygon and path arrays keep by their lattice rank.
    pub sub_cut_arrays: bool,
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
    /// §F2R-16: lets the daemon rebuild a shifted copy of a retained
    /// geometry frame for pan reuse.
    pub fn from_pixels(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, String> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|value| value.checked_mul(4))
            .ok_or_else(|| "image byte length overflow".to_string())?;
        if pixels.len() != expected {
            return Err(format!(
                "pixel buffer is {} bytes, expected {}",
                pixels.len(),
                expected
            ));
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

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
    /// The label-free frame (§F2R-16), kept when the caller asked to
    /// retain it for pan reuse AND a label pass painted over `frame`.
    pub geometry_frame: Option<RgbaFrame>,
    /// §F2R-20: the caller asked to keep the geometry but no label
    /// pass touched `frame`, so `frame` IS the label-free geometry -
    /// retain it directly instead of a full-frame copy.
    pub geometry_is_frame: bool,
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
    /// occupancy summary (docs/OCCUPANCY_PLAN.ko.md M2): cells of the
    /// chosen level that met the frame, and the pixels they painted
    pub summary_cell_paints: u64,
    pub summary_pixel_paints: u64,
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

/// §F2R-16 pan reuse: a previous geometry-only frame, pre-shifted by
/// the caller into this request's device coordinates. Tiles fully
/// inside `valid` are copied instead of rastered - byte-exact only
/// when the pan delta was a multiple of the 16px fill-phase period
/// (the caller's contract).
pub struct FrameReuse {
    pub base: RgbaFrame,
    /// Valid device-pixel region of `base`: x0, y0, x1, y1.
    pub valid: [u32; 4],
}

/// `render_geometry_styled_cancellable` plus pan reuse (§F2R-16):
/// tiles inside `reuse.valid` come from the shifted previous geometry
/// frame, and `keep_geometry` returns this render's own label-free
/// frame for the next pan.
pub fn render_geometry_styled_cancellable_reuse(
    scene: &FrameScene,
    request: &StyledGeometryRasterRequest,
    generation: u64,
    cancellation: &RenderCancellation,
    reuse: Option<&FrameReuse>,
    keep_geometry: bool,
) -> Result<GeometryRasterReport, String> {
    request.validate()?;
    render_geometry_impl(
        scene,
        &request.raster,
        RenderMode::Styled(request),
        Some(RenderGuard {
            generation,
            cancellation,
        }),
        true,
        reuse,
        keep_geometry,
        None,
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

thread_local! {
    /// Test override of `write_once_enabled` for the calling thread.
    static WRITE_ONCE_OVERRIDE: std::cell::Cell<Option<bool>> = std::cell::Cell::new(None);
}

/// F2R-28 write-once tiles (see `WriteOnce`); kill switch
/// FLOE_RUST_WRITE_ONCE=off restores the ordered overwrite.
fn write_once_enabled() -> bool {
    if let Some(forced) = WRITE_ONCE_OVERRIDE.with(|value| value.get()) {
        return forced;
    }
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("FLOE_RUST_WRITE_ONCE").as_deref() != Ok("off"))
}

/// Internal marker: a member enumeration stopped because its tile has no
/// open pixel left. Raised and caught around one enumeration, never
/// across a function boundary.
const WRITE_ONCE_FULL: &str = "write-once tile full";

fn until_full<T>(result: Result<T, String>) -> Result<Option<T>, String> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error == WRITE_ONCE_FULL => Ok(None),
        Err(error) => Err(error),
    }
}

/// One paint pass of a styled tile: a hierarchy-frame band or a plane.
#[derive(Clone, Copy)]
enum TilePass {
    Frames(u8),
    Plane(usize),
}

/// The passes in overwrite order (frame bands 2, 3, 1, the planes, frame
/// band 0) - reversed for a write-once tile, where the first writer of a
/// pixel is its last overwriter.
fn tile_passes(planes: usize, walk_frames: bool, write_once: bool) -> Vec<TilePass> {
    let mut passes = Vec::with_capacity(planes + 4);
    if walk_frames {
        passes.extend([2u8, 3, 1].map(TilePass::Frames));
    }
    passes.extend((0..planes).map(TilePass::Plane));
    if walk_frames {
        passes.push(TilePass::Frames(0));
    }
    if write_once {
        passes.reverse();
    }
    passes
}

fn render_geometry(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    mode: RenderMode<'_>,
    guard: Option<RenderGuard<'_>>,
    work_bin: bool,
) -> Result<GeometryRasterReport, String> {
    render_geometry_impl(scene, request, mode, guard, work_bin, None, false, None)
}

/// A styled render restricted to the device window `[col0, row0, col1,
/// row1)` of the full `request` frame (jobdeck step 2, 2026-09-09):
/// the world-to-device mapping, the tile grid and every fill phase
/// are those of the full frame, so the window's pixels are byte-equal
/// to a full render; tiles outside it are left as background and the
/// work-bin collection is culled to the window.
pub fn render_geometry_styled_cancellable_windowed(
    scene: &FrameScene,
    request: &StyledGeometryRasterRequest,
    generation: u64,
    cancellation: &RenderCancellation,
    window: [u32; 4],
) -> Result<GeometryRasterReport, String> {
    request.validate()?;
    let [c0, r0, c1, r1] = window;
    if c0 >= c1 || r0 >= r1 || c1 > request.raster.width || r1 > request.raster.height {
        return Err(format!("raster window out of bounds: {window:?}"));
    }
    render_geometry_impl(
        scene,
        &request.raster,
        RenderMode::Styled(request),
        Some(RenderGuard {
            generation,
            cancellation,
        }),
        true,
        None,
        false,
        Some(window),
    )
}

#[allow(clippy::too_many_arguments)]
fn render_geometry_impl(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    mode: RenderMode<'_>,
    guard: Option<RenderGuard<'_>>,
    work_bin: bool,
    reuse: Option<&FrameReuse>,
    keep_geometry: bool,
    window: Option<[u32; 4]>,
) -> Result<GeometryRasterReport, String> {
    check_cancelled(guard)?;
    if let Some(reuse) = reuse {
        if reuse.base.width != request.width || reuse.base.height != request.height {
            return Err("pan reuse frame size mismatch".to_string());
        }
        let [vx0, vy0, vx1, vy1] = reuse.valid;
        if vx0 > vx1 || vy0 > vy1 || vx1 > request.width || vy1 > request.height {
            return Err("pan reuse valid region out of bounds".to_string());
        }
    }
    let (prepared_labels, labels_truncated) = match mode {
        RenderMode::Occupancy => (None, false),
        RenderMode::Styled(_) => PreparedLabels::build(scene, request)?,
    };
    let mut counters = RasterCounters::default();
    let mut stats = RenderStats::default();
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
            collect_work_bin(scene, request, styled, stroke_pixels, guard, &mut stats, window)?
        }
        _ => None,
    };
    stats.work_bin_items = bin.as_ref().map(|bin| bin.items).unwrap_or(0);
    // §3.21: deferred edges concentrate mini-walk work in whichever
    // tiles cover them - the field view put 87% of its wall in one
    // 384px tile, whose mini also overran the cap (built, discarded,
    // then re-walked by the legacy path). Smaller tiles spread that
    // work across the raster workers and keep each tile's mini under
    // the cap; pixels are tile-size invariant (a pinned oracle), so
    // the shrink is pure scheduling.
    let shrunk_request;
    let request = if request.tile_size > 128
        && bin
            .as_ref()
            .is_some_and(|bin| !bin.deferred_edges.is_empty())
    {
        shrunk_request = GeometryRasterRequest {
            tile_size: 128,
            ..*request
        };
        &shrunk_request
    } else {
        request
    };
    let tile_size = u32::from(request.tile_size);
    let tile_columns = request.width.div_ceil(tile_size);
    let tile_rows = request.height.div_ceil(tile_size);
    let tile_count_u64 = u64::from(tile_columns) * u64::from(tile_rows);
    let tile_count: usize = tile_count_u64
        .try_into()
        .map_err(|_| format!("raster tile count limit exceeded: {tile_count_u64}"))?;
    let worker_count = usize::from(request.workers).min(tile_count).max(1);
    stats.workers_used = worker_count.try_into().unwrap_or(u16::MAX);
    stats.tiles = tile_count.try_into().unwrap_or(u32::MAX);
    let bin = bin.as_ref();
    let write_once = matches!(mode, RenderMode::Styled(_)) && write_once_enabled();
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
                    let tile_started = Instant::now();
                    // a tile outside the device window is not rastered
                    // at all (jobdeck step 2): the frame comes back
                    // window-sized, so no band is made for it either
                    if let Some([wc0, wr0, wc1, wr1]) = window {
                        if col1 <= wc0 || col0 >= wc1 || row1 <= wr0 || row0 >= wr1 {
                            continue;
                        }
                    }
                    // §F2R-16: a tile fully inside the shifted previous
                    // frame's valid region copies its pixels instead of
                    // rastering - byte-exact under the 16px snap.
                    let reused = reuse.and_then(|reuse| {
                        let [vx0, vy0, vx1, vy1] = reuse.valid;
                        (col0 >= vx0 && col1 <= vx1 && row0 >= vy0 && row1 <= vy1).then(|| {
                            reused_tile_output(request, &reuse.base, col0, col1, row0, row1)
                        })
                    });
                    let mut output = match reused {
                        Some(output) => output?,
                        None => raster_tile(
                            scene,
                            request,
                            mode,
                            bin,
                            guard,
                            write_once,
                            col0,
                            col1,
                            row0,
                            row1,
                        )?,
                    };
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
    if let Some(window) = window {
        // a windowed render returns the window's pixels only (no label
        // pass, no retained geometry: the jobdeck composite draws the
        // labels itself, later)
        if let (Some(labels), RenderMode::Styled(_)) = (prepared_labels.as_ref(), mode) {
            if !labels.rows.is_empty() {
                return Err("a windowed render takes no labels".to_string());
            }
        }
        let frame = assemble_window(request, tiles, window)?;
        stats.raster_us = started.elapsed().as_micros().try_into().unwrap_or(u64::MAX);
        return Ok(GeometryRasterReport {
            frame,
            geometry_frame: None,
            geometry_is_frame: false,
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
            label_tile_paints: 0,
            label_pixel_paints: 0,
            labels_truncated,
            partial: scene.is_partial(),
            summary_cell_paints: counters.summary_cells_drawn,
            summary_pixel_paints: counters.summary_pixels_drawn,
        });
    }
    finish_geometry_frame(
        request,
        mode,
        scene,
        tiles,
        tile_columns,
        tile_rows,
        stats,
        counters,
        prepared_labels.as_ref(),
        labels_truncated,
        keep_geometry,
        started,
        guard,
    )
}

/// Assembles the rastered tiles into the frame, paints the label passes over
/// it and fills in the report - the tail every full-frame raster shares
/// (`render_geometry_impl` and `LayerRasterSession::finish`).
#[allow(clippy::too_many_arguments)]
fn finish_geometry_frame(
    request: &GeometryRasterRequest,
    mode: RenderMode<'_>,
    scene: &FrameScene,
    tiles: Vec<RasterBand>,
    tile_columns: u32,
    tile_rows: u32,
    mut stats: RenderStats,
    mut counters: RasterCounters,
    prepared_labels: Option<&PreparedLabels>,
    labels_truncated: bool,
    keep_geometry: bool,
    started: Instant,
    guard: Option<RenderGuard<'_>>,
) -> Result<GeometryRasterReport, String> {
    let mut frame = assemble_tiles(request, tiles, tile_columns, tile_rows)?;
    check_cancelled(guard)?;
    // §F2R-20: the retained copy exists only because labels paint over
    // the geometry below; with no label rows the published frame is
    // the geometry frame and the caller retains it without a copy
    // (a 4K margin frame is ~130 MiB - one copy fewer per settle).
    let label_pass = matches!(
        (prepared_labels, mode),
        (Some(labels), RenderMode::Styled(_)) if !labels.rows.is_empty()
    );
    let geometry_frame = (keep_geometry && label_pass).then(|| frame.clone());
    let geometry_is_frame = keep_geometry && !label_pass;
    // §F2R-16 (user call 2026-09-04): labels paint LAST, over every
    // geometry plane, in one full-frame pass - a deliberate deviation
    // from the KLayout between-plane order so a pan-reused geometry
    // frame can take fresh viewport-planned labels on top.
    if let (Some(labels), RenderMode::Styled(styled)) = (prepared_labels, mode) {
        frame = apply_label_passes(request, styled, labels, frame, &mut counters, guard)?;
    }
    check_cancelled(guard)?;
    stats.raster_us = started.elapsed().as_micros().try_into().unwrap_or(u64::MAX);
    Ok(GeometryRasterReport {
        frame,
        geometry_frame,
        geometry_is_frame,
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
        summary_cell_paints: counters.summary_cells_drawn,
        summary_pixel_paints: counters.summary_pixels_drawn,
    })
}

type RepSpan = (u32, u32, u32); // row, first column, exclusive end

/// Write-once state of one tile (F2R-28). Every geometry paint is an
/// opaque overwrite in its plane's single colour, so the frame is a pure
/// function of "the last plane that writes a pixel wins". Painting the
/// planes in REVERSE order and writing each pixel at most once gives the
/// same bytes - and lets everything that can only touch written pixels
/// be skipped: a tile whose pixels are all written ends its plane
/// sequence, an item whose device box is written is never enumerated.
/// Field 2026-09-18 (synthetic MAIN01, 449 layers): 332 member paints per
/// lit pixel, the layers covering one another.
/// Bit i of word w of a row is local column w * 64 + i; 1 = written.
#[derive(Clone)]
struct WriteOnce {
    words: usize,
    bits: Vec<u64>,
    open: u32,
    /// the open pixels' box found by the last scan, and `open` then
    open_box: Option<(u32, [usize; 4])>,
}

/// Which pixels of a span a fill lights (the interior rule of
/// `fill_span`, as a column mask).
#[derive(Clone, Copy)]
enum SpanRule {
    All,
    /// lit where (row + column) is even
    Speckle { row: usize },
    /// one 16-column stipple row, bit 15 = column 0 (mod 16)
    Pattern { word: u16 },
}

impl SpanRule {
    /// Lit columns of the 64 local columns starting at absolute column `col`.
    #[inline]
    fn mask(self, col: usize) -> u64 {
        match self {
            SpanRule::All => !0,
            SpanRule::Speckle { row } => {
                if (row + col) & 1 == 0 {
                    0x5555_5555_5555_5555
                } else {
                    0xAAAA_AAAA_AAAA_AAAA
                }
            }
            SpanRule::Pattern { word } => {
                // bit k of `lit` = column k (mod 16)
                let lit = u64::from(word.reverse_bits());
                let tiled = lit | lit << 16 | lit << 32 | lit << 48;
                tiled.rotate_right((col & 15) as u32)
            }
        }
    }
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
    rep_spans: Vec<RepSpan>,
    /// None: planes paint in order and overwrite (the reference path).
    once: Option<WriteOnce>,
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
            rep_spans: Vec::new(),
            once: None,
        })
    }

    fn tile_width(&self) -> u32 {
        self.col1 - self.col0
    }

    fn enable_write_once(&mut self) {
        let width = self.tile_width() as usize;
        let rows = (self.row1 - self.row0) as usize;
        let words = width.div_ceil(64);
        let mut bits = vec![0u64; words * rows];
        if width % 64 != 0 {
            // columns past the tile are never open
            let padding = !0u64 << (width % 64);
            for row in 0..rows {
                bits[row * words + words - 1] = padding;
            }
        }
        self.once = Some(WriteOnce {
            words,
            bits,
            open: (width * rows) as u32,
            open_box: None,
        });
    }

    /// The cull view of the next pass of a write-once tile: the world view
    /// of the bounding box of the pixels still open (one pixel wider than
    /// the tile's own stroke margin), never more than `cull_view`. What
    /// lies outside it can only touch written pixels. None: no open pixel.
    fn open_view(&mut self, request: &GeometryRasterRequest, cull_view: BBox, stroke_pixels: u8) -> Result<Option<BBox>, String> {
        let (tile_width, tile_rows) = (self.tile_width(), self.row1 - self.row0);
        let (col0, row0) = (self.col0, self.row0);
        let Some(once) = self.once.as_mut() else {
            return Ok(Some(cull_view));
        };
        if once.open == 0 {
            return Ok(None);
        }
        if once.open == tile_width * tile_rows {
            return Ok(Some(cull_view));
        }
        let rows = tile_rows as usize;
        let (mut r0, mut r1, mut c0, mut c1) = (usize::MAX, 0usize, usize::MAX, 0usize);
        if let Some((open, found)) = once.open_box {
            if open == once.open {
                [r0, r1, c0, c1] = found;
            }
        }
        for row in 0..if r0 == usize::MAX { rows } else { 0 } {
            for word in 0..once.words {
                let open = !once.bits[row * once.words + word];
                if open == 0 {
                    continue;
                }
                r0 = r0.min(row);
                r1 = row + 1;
                c0 = c0.min(word * 64 + open.trailing_zeros() as usize);
                c1 = c1.max(word * 64 + 64 - open.leading_zeros() as usize);
            }
        }
        if r0 == usize::MAX {
            return Ok(None);
        }
        once.open_box = Some((once.open, [r0, r1, c0, c1]));
        let view = tile_world_view(
            request,
            col0 + c0 as u32,
            col0 + c1 as u32,
            row0 + r0 as u32,
            row0 + r1 as u32,
            stroke_pixels.saturating_add(1),
        )?;
        Ok(Some(BBox {
            x0: view.x0.max(cull_view.x0),
            y0: view.y0.max(cull_view.y0),
            x1: view.x1.min(cull_view.x1),
            y1: view.y1.min(cull_view.y1),
        }))
    }

    /// Every pixel of the tile is written: nothing painted from here on
    /// can change it.
    #[inline]
    fn is_full(&self) -> bool {
        matches!(&self.once, Some(once) if once.open == 0)
    }

    /// Some pixel is written, so a region test can succeed.
    #[inline]
    fn any_written(&self) -> bool {
        matches!(&self.once, Some(once) if once.open < self.tile_width() * (self.row1 - self.row0))
    }

    /// Write-once span: the lit, still open pixels of absolute columns
    /// [first_col, end_col) of absolute row `row` take `color`. Returns
    /// whether the rule lights any pixel of the span - what the
    /// overwriting path reports, written or not.
    #[inline]
    fn write_once_span(&mut self, row: usize, first_col: usize, end_col: usize, color: [u8; 4], rule: SpanRule) -> bool {
        self.write_once_rows(row, row + 1, first_col, end_col, color, |_| rule)
    }

    /// One solid pixel (a stepped stroke, a summary cell).
    #[inline]
    fn write_once_pixel(&mut self, row: usize, col: usize, color: [u8; 4]) {
        let width = self.tile_width() as usize;
        let (local_row, local_col) = (row - self.row0 as usize, col - self.col0 as usize);
        let Some(once) = self.once.as_mut() else {
            return;
        };
        let slot = &mut once.bits[local_row * once.words + (local_col >> 6)];
        let bit = 1u64 << (local_col & 63);
        if *slot & bit == 0 {
            *slot |= bit;
            once.open -= 1;
            let from = (local_row * width + local_col) * 4;
            self.pixels[from..from + 4].copy_from_slice(&color);
        }
    }

    /// `write_once_span` over absolute rows [row0, row1) of one column
    /// range (a rect fill, a hairline, an axis-aligned stroke): the word
    /// range and the span masks are found once, a row costs its rule and
    /// one mask word per 64 columns.
    #[inline]
    fn write_once_rows(
        &mut self,
        row0: usize,
        row1: usize,
        first_col: usize,
        end_col: usize,
        color: [u8; 4],
        rule_of: impl Fn(usize) -> SpanRule,
    ) -> bool {
        let width = self.tile_width() as usize;
        let col_base = self.col0 as usize;
        let band_row0 = self.row0 as usize;
        let (c0, c1) = (first_col - col_base, end_col - col_base);
        let Some(once) = self.once.as_mut() else {
            return false;
        };
        let words = once.words;
        let (w0, w1) = (c0 >> 6, (c1 - 1) >> 6);
        let edge = |word: usize| {
            let origin = word << 6;
            let lo = c0.max(origin) - origin;
            let hi = c1.min(origin + 64) - origin;
            (lo, hi, if hi - lo == 64 { !0u64 } else { ((1u64 << (hi - lo)) - 1) << lo })
        };
        let mut lit_any = false;
        let mut newly = 0u32;
        for row in row0..row1 {
            let rule = rule_of(row);
            let speckle = matches!(rule, SpanRule::Speckle { .. });
            let local_row = row - band_row0;
            let bits = &mut once.bits[local_row * words..(local_row + 1) * words];
            let pixels = &mut self.pixels[local_row * width * 4..(local_row + 1) * width * 4];
            for word in w0..w1 + 1 {
                let origin = word << 6;
                let (lo, hi, span) = edge(word);
                let lit = span & rule.mask(col_base + origin);
                if lit == 0 {
                    continue;
                }
                lit_any = true;
                let todo = lit & !bits[word];
                if todo == 0 {
                    continue;
                }
                bits[word] |= todo;
                newly += todo.count_ones();
                if todo == span {
                    // an open solid stretch: the overwriting path's loop
                    for pixel in pixels[(origin + lo) * 4..(origin + hi) * 4].chunks_exact_mut(4) {
                        pixel.copy_from_slice(&color);
                    }
                    continue;
                }
                let first = todo.trailing_zeros() as usize;
                let last = 64 - todo.leading_zeros() as usize;
                if speckle && todo == lit {
                    // an open checkerboard stretch: every other pixel from
                    // the first lit one, the overwriting path's stride
                    for pair in pixels[(origin + first) * 4..(origin + last) * 4].chunks_mut(8) {
                        pair[..4].copy_from_slice(&color);
                    }
                } else if todo.count_ones() as usize * 4 >= last - first {
                    // dense (a stipple, a partly written stretch): one pass
                    for (bit, pixel) in (first..last).zip(pixels[(origin + first) * 4..(origin + last) * 4].chunks_exact_mut(4)) {
                        if todo >> bit & 1 != 0 {
                            pixel.copy_from_slice(&color);
                        }
                    }
                } else {
                    let mut left = todo;
                    while left != 0 {
                        let from = (origin + left.trailing_zeros() as usize) * 4;
                        pixels[from..from + 4].copy_from_slice(&color);
                        left &= left - 1;
                    }
                }
            }
        }
        once.open -= newly;
        lit_any
    }

    /// True when no pixel of absolute device rows [r0, r1) x columns
    /// [c0, c1) inside this tile is open: an item confined to the box
    /// cannot change the tile. An empty intersection is true.
    fn device_box_written(&self, r0: i128, r1: i128, c0: i128, c1: i128) -> bool {
        let Some(once) = &self.once else {
            return false;
        };
        let r0 = r0.max(self.row0 as i128);
        let r1 = r1.min(self.row1 as i128);
        let c0 = c0.max(self.col0 as i128);
        let c1 = c1.min(self.col1 as i128);
        if r0 >= r1 || c0 >= c1 {
            return true;
        }
        if once.open == 0 {
            return true;
        }
        let (c0, c1) = ((c0 - self.col0 as i128) as usize, (c1 - self.col0 as i128) as usize);
        for row in (r0 - self.row0 as i128) as usize..(r1 - self.row0 as i128) as usize {
            for word in c0 / 64..=(c1 - 1) / 64 {
                let lo = c0.max(word * 64) - word * 64;
                let hi = c1.min(word * 64 + 64) - word * 64;
                let span = if hi - lo == 64 { !0u64 } else { ((1u64 << (hi - lo)) - 1) << lo };
                if once.bits[row * once.words + word] & span != span {
                    return false;
                }
            }
        }
        true
    }

    /// `device_box_written` for a world box and everything painted from
    /// it: fills, the outline of `stroke_width`, the hairline collapse
    /// and its one-pixel row bias all stay within `stroke_width + 2`
    /// pixels of the box's device corners. A box the device mapping
    /// rejects is never skipped, so its error stays reachable.
    fn world_box_written(&self, request: &GeometryRasterRequest, world: BBox, stroke_width: u8) -> bool {
        if !self.any_written() {
            return false;
        }
        let (Ok((x0, y1)), Ok((x1, y0))) = (
            world_to_device(request, world.x0, world.y0),
            world_to_device(request, world.x1, world.y1),
        ) else {
            return false;
        };
        let margin = i128::from(stroke_width) + 2;
        self.device_box_written(
            floor_div(y0.min(y1), DEVICE_ONE) - margin,
            floor_div(y0.max(y1), DEVICE_ONE) + 1 + margin,
            floor_div(x0.min(x1), DEVICE_ONE) - margin,
            floor_div(x0.max(x1), DEVICE_ONE) + 1 + margin,
        )
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

/// §F2R-16: a tile served from the shifted previous geometry frame.
fn reused_tile_output(
    request: &GeometryRasterRequest,
    base: &RgbaFrame,
    col0: u32,
    col1: u32,
    row0: u32,
    row1: u32,
) -> Result<RasterTileOutput, String> {
    let mut band = RasterBand::new_tile(request, col0, col1, row0, row1)?;
    let tile_row_bytes = ((col1 - col0) as usize) * 4;
    let frame_row_bytes = (base.width as usize) * 4;
    for local_row in 0..(row1 - row0) as usize {
        let source = (row0 as usize + local_row) * frame_row_bytes + (col0 as usize) * 4;
        let target = local_row * tile_row_bytes;
        band.pixels[target..target + tile_row_bytes]
            .copy_from_slice(&base.pixels[source..source + tile_row_bytes]);
    }
    let stats = RenderStats {
        tiles_reused: 1,
        ..RenderStats::default()
    };
    Ok(RasterTileOutput {
        tile: band,
        stats,
        counters: RasterCounters::default(),
    })
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
    /// the collection's ranking, stored for the deferred edges' mini walks
    ranking: Option<PlaceRanking>,
}

/// What a collection walk needs to rank and walk placement arrays under the
/// placement lattice (GeometryRasterRequest::place_lattice).
#[derive(Clone)]
struct PlaceRanking {
    request: GeometryRasterRequest,
    /// per plane: whether its paint is the area-true rim (a 1 px solid outline)
    rim: Vec<bool>,
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
        /// the placement lattice the visit lies under (member_lattice)
        lattice: Option<(i64, i64)>,
    },
    // Rects of the plane in walk order, up to 128 to a chunk: the planner's
    // washes (page washes, sub-cut boxes) and the native display points of
    // design.ovr. A tile rejects a whole chunk instead of checking every rect.
    Points { world_bbox: BBox, points: Vec<BBox> },
    /// Representative SHAPES of design.ovr (OVR2): rects, boundary segments
    /// and fallback points in top coordinates, chunked like Points. They are
    /// painted as the shapes they are (paint_representative), never as a wash.
    Reps { world_bbox: BBox, prims: Vec<floe_vfs::representatives::Prim> },
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
        /// the placement lattice the parent cell lies under
        lattice: Option<(i64, i64)>,
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
    /// the placement lattice the parent cell lies under
    lattice: Option<(i64, i64)>,
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
            ranking: None,
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
        let ranking = bin.ranking.as_ref().ok_or_else(|| "internal error: a work bin without its ranking".to_string())?;
        let own = own_lattice(&ranking.request, &instance.rep, &edge.transform)?;
        let child_lattice = own.or(edge.lattice);
        let members = match own {
            Some(pitches) => placement_survivor_walk(
                scene,
                &ranking.request,
                instance,
                &edge.transform,
                pitches,
                base_bbox,
                local_view,
                &|layer| bin.plane_of.get(&layer).map(|&plane| ranking.rim[plane]),
                guard,
            stats,
            )?,
            None => None,
        };
        let mut member = |ox: i64, oy: i64| -> Result<(), String> {
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
                ranking,
                want_frames,
                cull_view,
                guard,
                instance.child,
                child_world,
                child_lattice,
                &mut path,
                &mut plane_scratch,
                &mut visit_seq,
                stats,
            )
        };
        let walk = match members {
            Some(walk) => walk.run(guard, &mut SurvivorWork::default(), &mut member),
            None => for_each_visible_offset(&instance.rep, base_bbox, local_view, &mut member),
        };
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
    window: Option<[u32; 4]>,
) -> Result<Option<WorkBin>, String> {
    let [wc0, wr0, wc1, wr1] = window.unwrap_or([0, 0, request.width, request.height]);
    let cull_view = tile_world_view(request, wc0, wc1, wr0, wr1, stroke_pixels)?;
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
    let ranking = PlaceRanking {
        request: *request,
        rim: styled.layers.iter().map(|layer| request.area_true && layer.outline_width == 1).collect(),
    };
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
        &ranking,
        styled.hierarchy_frames,
        cull_view,
        guard,
        scene.top(),
        OrthoTransform::identity(),
        None,
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
            bin.ranking = Some(ranking);
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
    ranking: &PlaceRanking,
    want_frames: bool,
    cull_view: BBox,
    guard: Option<RenderGuard<'_>>,
    key: WsKey,
    world_transform: OrthoTransform,
    lattice: Option<(i64, i64)>,
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
        // the page's METADATA: a layer-ordered frame collects before it has
        // decoded anything (LAYER_DECODE_PROBE_PLAN §4), and the same walk
        // then serves the mini bins of the deferred edges
        let Some((layer_idx, bbox)) = scene.page_geometry(page_id) else {
            continue;
        };
        let Some(&plane) = plane_of.get(&layer_idx) else {
            continue;
        };
        if !bbox.intersects(&local_view) {
            continue;
        }
        let entry = &mut plane_scratch[plane];
        if entry.0 == stamp {
            entry.1.grow(&bbox);
        } else {
            *entry = (stamp, bbox);
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
            lattice,
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
        // Washes share chunk items of up to 128 rects (as the zero-size
        // display points always did): the sub-cut boxes (floe_vfs hier.rs
        // SUB_CUT_BOX_PX) are washes of a few pixels, a million of them in a
        // wide view of a few layers, and one bin item each overran the item
        // cap and sent the frame down the per-tile, per-plane walk. The order
        // within the plane is the walk's, as before.
        if let Some(PlaneItem::Points { world_bbox: bounds, points }) = bin.planes[plane].last_mut() {
            if points.len() < 128 {
                bounds.grow(&world_bbox);
                points.push(world_bbox);
                continue;
            }
        }
        check_cancelled(guard)?;
        bin.charge()?;
        bin.planes[plane].push(PlaneItem::Points { world_bbox, points: vec![world_bbox] });
    }
    // OVR2 shapes ride on the top cell only (identity transform)
    if path.len() == 1 {
        for &(layer_idx, prim) in &cell.reps {
            let Some(&plane) = plane_of.get(&layer_idx) else {
                continue;
            };
            let world_bbox = prim.bbox();
            if !world_bbox.intersects(&cull_view) {
                continue;
            }
            if let Some(PlaneItem::Reps { world_bbox: bounds, prims }) = bin.planes[plane].last_mut() {
                if prims.len() < 128 {
                    bounds.grow(&world_bbox);
                    prims.push(prim);
                    continue;
                }
            }
            check_cancelled(guard)?;
            bin.charge()?;
            bin.planes[plane].push(PlaneItem::Reps { world_bbox, prims: vec![prim] });
        }
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
            let own = own_lattice(&ranking.request, &instance.rep, &world_transform)?;
            let child_lattice = own.or(lattice);
            let walk = match own {
                Some(pitches) => placement_survivor_walk(
                    scene,
                    &ranking.request,
                    instance,
                    &world_transform,
                    pitches,
                    base_bbox,
                    local_view,
                    &|layer| plane_of.get(&layer).map(|&plane| ranking.rim[plane]),
                    guard,
                stats,
                )?,
                None => None,
            };
            let mut member = |offset_x: i64, offset_y: i64| -> Result<(), String> {
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
                    ranking,
                    want_frames,
                    cull_view,
                    guard,
                    instance.child,
                    child_world,
                    child_lattice,
                    path,
                    plane_scratch,
                    visit_seq,
                    stats,
                )
            };
            let attempt = match walk {
                Some(walk) => walk.run(guard, &mut SurvivorWork::default(), &mut member),
                None => for_each_visible_offset(&instance.rep, base_bbox, local_view, &mut member),
            };
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
            lattice,
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
                    lattice,
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
    let mut rep_spans = std::mem::take(&mut band.rep_spans);
    for item in items {
        if band.is_full() {
            break;
        }
        match item {
            PlaneItem::Cell {
                world_bbox,
                transform,
                inverse,
                cell,
                lattice,
            } => {
                if !world_bbox.intersects(&cull_view) {
                    continue;
                }
                if band.world_box_written(request, *world_bbox, paint.stroke_width) {
                    stats.once_items_skipped = stats.once_items_skipped.saturating_add(1);
                    continue;
                }
                let visited = scene.cell(*cell).ok_or_else(|| {
                    format!("internal error: binned cell {:?} left the scene", cell)
                })?;
                let local_view = inverse.apply_bbox(cull_view)?;
                for (slot, &page_id) in visited.pages.iter().enumerate() {
                    let Some(page) = scene.page(page_id) else {
                        continue;
                    };
                    if page.layer_idx != layer.layer_idx || !page.bbox.intersects(&local_view)
                    {
                        continue;
                    }
                    if band.any_written() {
                        if band.is_full() {
                            break;
                        }
                        if let Ok(world) = transform.apply_bbox(page.bbox) {
                            if band.world_box_written(request, world, paint.stroke_width) {
                                stats.once_items_skipped = stats.once_items_skipped.saturating_add(1);
                                continue;
                            }
                        }
                    }
                    let level = visited.page_levels.get(slot).copied().unwrap_or(0);
                    raster_page_records(
                        band,
                        request,
                        page,
                        page_id,
                        level,
                        scene.plan().stats.shape_cut.min(i64::MAX as u64) as i64,
                        scene.plan().stats.shape_cut_max,
                        local_view,
                        *transform,
                        *lattice,
                        stats,
                        counters,
                        paint,
                        guard,
                        record_scratch,
                    )?;
                }
            }
            PlaneItem::Points { world_bbox, points } => {
                if !world_bbox.intersects(&cull_view) { continue; }
                if band.world_box_written(request, *world_bbox, paint.stroke_width) {
                    stats.once_items_skipped = stats.once_items_skipped.saturating_add(1);
                    continue;
                }
                check_cancelled(guard)?;
                let marker = marker_request(request);
                for &point in points {
                    if !point.intersects(&cull_view) { continue; }
                    counters.rect_records = counters.rect_records.saturating_add(1);
                    stats.primitives_tested = stats.primitives_tested.saturating_add(1);
                    stats.rep_members_tested = stats.rep_members_tested.saturating_add(1);
                    if paint_world_rect(band, &marker, point, paint)? {
                        counters.rectangle_members_drawn = counters.rectangle_members_drawn.saturating_add(1);
                        stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(1);
                        stats.primitives_drawn = stats.primitives_drawn.saturating_add(1);
                    }
                }
            }
            PlaneItem::Reps { world_bbox, prims } => {
                if !world_bbox.intersects(&cull_view) { continue; }
                if band.world_box_written(request, *world_bbox, paint.stroke_width) {
                    stats.once_items_skipped = stats.once_items_skipped.saturating_add(1);
                    continue;
                }
                check_cancelled(guard)?;
                let marker = marker_request(request);
                for prim in prims {
                    if !prim.bbox().intersects(&cull_view) { continue; }
                    counters.rect_records = counters.rect_records.saturating_add(1);
                    stats.primitives_tested = stats.primitives_tested.saturating_add(1);
                    stats.rep_members_tested = stats.rep_members_tested.saturating_add(1);
                    if queue_representative(band, &marker, prim, paint, &mut rep_spans, stats)? {
                        counters.rectangle_members_drawn = counters.rectangle_members_drawn.saturating_add(1);
                        stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(1);
                        stats.primitives_drawn = stats.primitives_drawn.saturating_add(1);
                    }
                }
            }
            PlaneItem::Deferred {
                transform,
                inverse,
                cell,
                inst,
                bit,
                edge,
                lattice,
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
                let own = own_lattice(request, &instance.rep, transform)?;
                let child_lattice = own.or(*lattice);
                let walk = match own {
                    Some(pitches) => placement_survivor_walk(
                        scene,
                        request,
                        instance,
                        transform,
                        pitches,
                        base_bbox,
                        local_view,
                        &|index| (index == layer.layer_idx).then(|| area_true_rim(request, paint)),
                        guard,
                    stats,
                    )?,
                    None => None,
                };
                let mut member = |offset_x: i64, offset_y: i64| -> Result<(), String> {
                    check_member_cancelled(guard, &mut cancel_member)?;
                    if band.any_written() {
                        if band.is_full() {
                            return Err(WRITE_ONCE_FULL.to_string());
                        }
                        let member = translate_bbox(base_bbox, offset_x, offset_y)?;
                        if band.world_box_written(request, transform.apply_bbox(member)?, paint.stroke_width) {
                            stats.once_items_skipped = stats.once_items_skipped.saturating_add(1);
                            return Ok(());
                        }
                    }
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
                        child_lattice,
                        &mut deferred_path,
                        record_scratch,
                    )
                };
                let visit = until_full(match walk {
                    Some(walk) => walk.run(guard, &mut SurvivorWork::default(), &mut member),
                    None => for_each_visible_offset(&instance.rep, base_bbox, local_view, &mut member),
                })?;
                stats.rep_members_tested = stats
                    .rep_members_tested
                    .saturating_add(visit.map_or(0, |visit| visit.tested));
            }
        }
    }
    flush_representative_spans(band, request, paint, &mut rep_spans, stats);
    band.rep_spans = rep_spans;
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


/// One tile's state across the pass sequence (docs/LAYER_DECODE_PROBE_PLAN.ko.md
/// §5). `raster_tile` runs every pass of a tile in one call; the layer-decode
/// probe runs one pass over every tile and decodes what the next layer needs
/// before the next, so the write-once mask, the pixels and the resolved
/// deferred edges have to outlive a single pass.
struct TileWork {
    band: RasterBand,
    /// the tile's own cull view; a write-once pass narrows it to the open
    /// pixels (`open_view`) without losing this one
    tile_view: BBox,
    /// §3.17 deferred edges resolved for this tile (bin path only)
    minis: Vec<Option<WorkBin>>,
    path: Vec<WsKey>,
    record_scratch: RecordSet,
    stats: RenderStats,
    counters: RasterCounters,
    /// no open pixel is left: the tile takes no further pass
    full: bool,
}

impl TileWork {
    fn new(
        request: &GeometryRasterRequest,
        stroke_pixels: u8,
        write_once: bool,
        col0: u32,
        col1: u32,
        row0: u32,
        row1: u32,
    ) -> Result<Self, String> {
        let mut band = RasterBand::new_tile(request, col0, col1, row0, row1)?;
        if write_once {
            band.enable_write_once();
        }
        Ok(TileWork {
            band,
            tile_view: tile_world_view(request, col0, col1, row0, row1, stroke_pixels)?,
            minis: Vec::new(),
            path: Vec::new(),
            record_scratch: RecordSet::default(),
            stats: RenderStats::default(),
            counters: RasterCounters::default(),
            full: false,
        })
    }

    /// §3.17: resolve every deferred edge once for this tile before the
    /// band/plane sequence consumes it from all sides.
    fn prepare_minis(
        &mut self,
        scene: &FrameScene,
        bin: Option<&WorkBin>,
        hierarchy_frames: bool,
        guard: Option<RenderGuard<'_>>,
    ) -> Result<(), String> {
        if let Some(bin) = bin.filter(|bin| !bin.deferred_edges.is_empty()) {
            self.minis = build_deferred_minis(
                scene,
                bin,
                hierarchy_frames,
                self.tile_view,
                guard,
                &mut self.stats,
            )?;
        }
        Ok(())
    }

    fn output(self) -> RasterTileOutput {
        RasterTileOutput {
            tile: self.band,
            stats: self.stats,
            counters: self.counters,
        }
    }
}

/// The pass sequence of a styled frame. It is a property of the frame, not of
/// a tile, so every tile takes the same passes in the same order and a pass
/// index means the same paint step in all of them.
fn styled_passes(
    scene: &FrameScene,
    styled: &StyledGeometryRasterRequest,
    bin: Option<&WorkBin>,
    write_once: bool,
) -> Vec<TilePass> {
    let walk_frames = styled.hierarchy_frames
        && match bin {
            // the masks are subtree-cumulative, so a frame-free plan
            // skips all four band walks in one test (labels still run)
            None => scene.subtree_has_frames(scene.top()),
            Some(bin) => !bin.frames.is_empty(),
        };
    tile_passes(styled.layers.len(), walk_frames, write_once)
}

/// One pass of one tile: the bin's item lists when the collection holds them,
/// the per-plane hierarchy walk otherwise. `remaining` counts this pass and
/// every pass after it - what a tile that fills here never runs.
#[allow(clippy::too_many_arguments)]
fn raster_tile_pass(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    styled: &StyledGeometryRasterRequest,
    bin: Option<&WorkBin>,
    work: &mut TileWork,
    pass: TilePass,
    remaining: usize,
    stroke_pixels: u8,
    guard: Option<RenderGuard<'_>>,
) -> Result<(), String> {
    check_cancelled(guard)?;
    // write-once: the pass sees only what can reach an open pixel
    let Some(cull_view) = work.band.open_view(request, work.tile_view, stroke_pixels)? else {
        work.stats.once_full_tiles = work.stats.once_full_tiles.saturating_add(1);
        work.stats.once_passes_skipped = work
            .stats
            .once_passes_skipped
            .saturating_add(remaining as u64);
        work.full = true;
        return Ok(());
    };
    if cull_view.x0 >= cull_view.x1 || cull_view.y0 >= cull_view.y1 {
        return Ok(());
    }
    let TileWork {
        band,
        minis,
        path,
        record_scratch,
        stats,
        counters,
        ..
    } = work;
    match pass {
        TilePass::Frames(frame_band) => match bin {
            Some(bin) => replay_frame_items(
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
                Some(minis),
            )?,
            None => render_frame_band(
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
            )?,
        },
        TilePass::Plane(plane) => {
            let layer = &styled.layers[plane];
            let paint = PaintStyle {
                color: if styled.mono {
                    monochrome(layer.color)
                } else {
                    layer.color
                },
                fill: layer.fill,
                stroke: StrokeStyle::Solid,
                stroke_width: layer.outline_width,
            };
            // an occupancy summary paints in the layer's own slot (M2):
            // the plane's page items are empty for a summarized layer
            if let Some(summary) = scene.summary_for(layer.layer_idx) {
                paint_summary_plane(band, request, summary, paint, counters)?;
            }
            match bin {
                Some(bin) => replay_plane_items(
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
                    Some(minis),
                )?,
                None => render_cell(
                    scene,
                    request,
                    band,
                    cull_view,
                    stats,
                    counters,
                    GeometrySelection::Layer(layer.layer_idx),
                    SubtreePrune::Layer(scene.layer_mask_bit(layer.layer_idx)),
                    paint,
                    guard,
                    scene.top(),
                    OrthoTransform::identity(),
                    None,
                    path,
                    record_scratch,
                )?,
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn raster_tile(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    mode: RenderMode<'_>,
    bin: Option<&WorkBin>,
    guard: Option<RenderGuard<'_>>,
    write_once: bool,
    col0: u32,
    col1: u32,
    row0: u32,
    row1: u32,
) -> Result<RasterTileOutput, String> {
    check_cancelled(guard)?;
    let stroke_pixels = match mode {
        RenderMode::Occupancy => 1,
        RenderMode::Styled(styled) => styled
            .layers
            .iter()
            .map(|layer| layer.outline_width)
            .max()
            .unwrap_or(1),
    };
    let mut work = TileWork::new(request, stroke_pixels, write_once, col0, col1, row0, row1)?;
    match mode {
        RenderMode::Occupancy => {
            let cull_view = work.tile_view;
            render_cell(
                scene,
                request,
                &mut work.band,
                cull_view,
                &mut work.stats,
                &mut work.counters,
                GeometrySelection::All,
                SubtreePrune::Off,
                PaintStyle::solid(request.foreground),
                guard,
                scene.top(),
                OrthoTransform::identity(),
                None,
                &mut work.path,
                &mut work.record_scratch,
            )?;
        }
        RenderMode::Styled(styled) => {
            work.prepare_minis(scene, bin, styled.hierarchy_frames, guard)?;
            let passes = styled_passes(scene, styled, bin, work.band.once.is_some());
            for (done, &pass) in passes.iter().enumerate() {
                if work.full {
                    break;
                }
                raster_tile_pass(
                    scene,
                    request,
                    styled,
                    bin,
                    &mut work,
                    pass,
                    passes.len() - done,
                    stroke_pixels,
                    guard,
                )?;
            }
        }
    }
    check_cancelled(guard)?;
    Ok(work.output())
}


/// Runs `f` on every tile, over `workers` threads that take the next tile as
/// they free up (the tile scheduling of `render_geometry_impl`, one pass at a
/// time instead of one whole tile).
fn for_each_tile<F>(tiles: &mut [TileWork], workers: usize, f: F) -> Result<(), String>
where
    F: Fn(&mut TileWork) -> Result<(), String> + Sync,
{
    if workers <= 1 || tiles.len() <= 1 {
        for tile in tiles.iter_mut() {
            f(tile)?;
        }
        return Ok(());
    }
    let queue = std::sync::Mutex::new(tiles.iter_mut().collect::<Vec<_>>());
    let (queue, f) = (&queue, &f);
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            handles.push(scope.spawn(move || {
                loop {
                    let next = queue
                        .lock()
                        .map_err(|_| "raster tile queue poisoned".to_string())?
                        .pop();
                    let Some(tile) = next else {
                        return Ok(());
                    };
                    f(tile)?;
                }
            }));
        }
        let mut error: Option<String> = None;
        for handle in handles {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(failed)) => {
                    error.get_or_insert(failed);
                }
                Err(_) => {
                    error.get_or_insert_with(|| "raster worker panicked".to_string());
                }
            }
        }
        match error {
            Some(failed) => Err(failed),
            None => Ok(()),
        }
    })
}

/// The tiles of one styled frame kept alive across its pass sequence
/// (docs/LAYER_DECODE_PROBE_PLAN.ko.md §5). `render_geometry_impl` runs every
/// pass of a tile in one call and returns a finished frame; a session runs ONE
/// pass over every tile and hands control back, so the caller can decode what
/// the next layer needs in between. The tile grid, the pass order, the
/// write-once rule and every coordinate are the normal path's, so the frame a
/// session finishes is byte-identical to `render_geometry_styled`'s.
///
/// Diagnostic only: no device window, no pan reuse, no retained geometry
/// frame, and the passes of one frame run to the end before `finish`.
pub struct LayerRasterSession {
    /// the effective request (the deferred-edge tile shrink applied)
    request: GeometryRasterRequest,
    bin: Option<WorkBin>,
    passes: Vec<TilePass>,
    stroke_pixels: u8,
    workers: usize,
    tiles: Vec<TileWork>,
    tile_columns: u32,
    tile_rows: u32,
    stats: RenderStats,
    labels: Option<PreparedLabels>,
    labels_truncated: bool,
    started: Instant,
    /// the plan's pages per styled plane, sorted unique: what a layer needs
    /// when nothing about coverage is known (LAYER_DECODE_PROBE_PLAN §6)
    pages_by_plane: Vec<Vec<u32>>,
}

/// What a block of layers still needs read, asked at the block's start with
/// every worker stopped (docs/LAYER_DECODE_PROBE_PLAN.ko.md §6). The masks it
/// reads are the ones the block starts from, so the answer does not depend on
/// which worker got which tile.
pub struct BlockDemand<'a> {
    request: &'a GeometryRasterRequest,
    styled: &'a StyledGeometryRasterRequest,
    scene: &'a FrameScene,
    bin: Option<&'a WorkBin>,
    tiles: &'a [std::sync::MutexGuard<'a, TileWork>],
    pages_by_plane: &'a [Vec<u32>],
}

/// Why the pages of one plane were asked for or left out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DemandStats {
    /// (page, instance) pairs looked at
    pub candidates: u64,
    /// pages asked for
    pub needed: u64,
    /// instances whose box is outside the frame
    pub out_of_view: u64,
    /// instances whose box is written to the last pixel already
    pub occluded: u64,
    /// items the walk could not decide (a deferred edge): their layer is
    /// asked for whole
    pub unsure: u64,
}

impl BlockDemand<'_> {
    /// Every device pixel `world` could paint is written already. False when
    /// that cannot be shown - the answer is never "skip it" on a doubt.
    fn written(&self, world: BBox, stroke_width: u8) -> bool {
        let Some((c0, c1, r0, r1)) = self.device_box(world, stroke_width) else {
            return false;
        };
        // outside the frame there is nothing to paint
        if c1 <= 0 || r1 <= 0 || c0 >= i128::from(self.request.width) || r0 >= i128::from(self.request.height) {
            return true;
        }
        self.tiles
            .iter()
            .all(|tile| tile.band.device_box_written(r0, r1, c0, c1))
    }

    /// The box can reach a pixel of the frame at all.
    fn in_frame(&self, world: BBox, stroke_width: u8) -> bool {
        let Some((c0, c1, r0, r1)) = self.device_box(world, stroke_width) else {
            return true;
        };
        c1 > 0
            && r1 > 0
            && c0 < i128::from(self.request.width)
            && r0 < i128::from(self.request.height)
    }

    /// `RasterBand::world_box_written`'s device box, shared so a page the
    /// probe leaves out is one the raster would have skipped anyway.
    fn device_box(&self, world: BBox, stroke_width: u8) -> Option<(i128, i128, i128, i128)> {
        let (Ok((x0, y1)), Ok((x1, y0))) = (
            world_to_device(self.request, world.x0, world.y0),
            world_to_device(self.request, world.x1, world.y1),
        ) else {
            return None;
        };
        let margin = i128::from(stroke_width) + 2;
        Some((
            floor_div(x0.min(x1), DEVICE_ONE) - margin,
            floor_div(x0.max(x1), DEVICE_ONE) + 1 + margin,
            floor_div(y0.min(y1), DEVICE_ONE) - margin,
            floor_div(y0.max(y1), DEVICE_ONE) + 1 + margin,
        ))
    }

    /// The plan's pages for this plane (`occlusion` false), or only those a
    /// pass could still paint into an open pixel (`occlusion` true). Pages are
    /// appended to `out`; a page any instance may still show is asked for.
    pub fn pages_for_plane(
        &self,
        plane: usize,
        occlusion: bool,
        out: &mut Vec<u32>,
    ) -> DemandStats {
        let mut stats = DemandStats::default();
        let all = self.pages_by_plane.get(plane).map(Vec::as_slice).unwrap_or(&[]);
        let Some(bin) = self.bin.filter(|_| occlusion) else {
            stats.candidates = all.len() as u64;
            stats.needed = all.len() as u64;
            out.extend_from_slice(all);
            return stats;
        };
        let Some(layer) = self.styled.layers.get(plane) else {
            return stats;
        };
        let first = out.len();
        for item in &bin.planes[plane] {
            let PlaneItem::Cell {
                world_bbox,
                transform,
                cell,
                ..
            } = item
            else {
                // a deferred edge is resolved per tile, during the paint: the
                // probe does not walk it again, it asks for the layer whole
                if matches!(item, PlaneItem::Deferred { .. }) {
                    stats.unsure += 1;
                    out.extend_from_slice(all);
                }
                continue;
            };
            if self.written(*world_bbox, layer.outline_width) {
                stats.occluded += 1;
                continue;
            }
            let Some(visited) = self.scene.cell(*cell) else {
                continue;
            };
            for &page_id in &visited.pages {
                let Some((page_layer, bbox)) = self.scene.page_geometry(page_id) else {
                    continue;
                };
                if page_layer != layer.layer_idx {
                    continue;
                }
                stats.candidates += 1;
                let Ok(world) = transform.apply_bbox(bbox) else {
                    out.push(page_id);
                    continue;
                };
                if !self.in_frame(world, layer.outline_width) {
                    stats.out_of_view += 1;
                    continue;
                }
                if self.written(world, layer.outline_width) {
                    stats.occluded += 1;
                    continue;
                }
                out.push(page_id);
            }
        }
        out[first..].sort_unstable();
        let end = first + partition_dedup(&mut out[first..]);
        out.truncate(end);
        stats.needed = (out.len() - first) as u64;
        stats
    }
}

/// `slice::partition_dedup` on a sorted slice (not stable in this toolchain):
/// the unique prefix's length.
fn partition_dedup(values: &mut [u32]) -> usize {
    let mut kept = 0usize;
    for at in 0..values.len() {
        if kept == 0 || values[kept - 1] != values[at] {
            values[kept] = values[at];
            kept += 1;
        }
    }
    kept
}

impl LayerRasterSession {
    /// Plans the frame's tiles and passes. `scene` must already carry every
    /// page the work-bin collection needs to see (the collection reads page
    /// metadata through `FrameScene::page`).
    pub fn begin(
        scene: &FrameScene,
        styled: &StyledGeometryRasterRequest,
        work_bin: bool,
        guard: Option<RenderGuard<'_>>,
    ) -> Result<Self, String> {
        styled.validate()?;
        let started = Instant::now();
        let mut stats = RenderStats::default();
        let (labels, labels_truncated) = PreparedLabels::build(scene, &styled.raster)?;
        let stroke_pixels = styled
            .layers
            .iter()
            .map(|layer| layer.outline_width)
            .max()
            .unwrap_or(1);
        let bin = if work_bin {
            collect_work_bin(
                scene,
                &styled.raster,
                styled,
                stroke_pixels,
                guard,
                &mut stats,
                None,
            )?
        } else {
            None
        };
        stats.work_bin_items = bin.as_ref().map(|bin| bin.items).unwrap_or(0);
        // §3.21: the deferred-edge tile shrink, as in `render_geometry_impl`
        let request = if styled.raster.tile_size > 128
            && bin
                .as_ref()
                .is_some_and(|bin| !bin.deferred_edges.is_empty())
        {
            GeometryRasterRequest {
                tile_size: 128,
                ..styled.raster
            }
        } else {
            styled.raster
        };
        let tile_size = u32::from(request.tile_size);
        let tile_columns = request.width.div_ceil(tile_size);
        let tile_rows = request.height.div_ceil(tile_size);
        let tile_count_u64 = u64::from(tile_columns) * u64::from(tile_rows);
        let tile_count: usize = tile_count_u64
            .try_into()
            .map_err(|_| format!("raster tile count limit exceeded: {tile_count_u64}"))?;
        let workers = usize::from(request.workers).min(tile_count).max(1);
        stats.workers_used = workers.try_into().unwrap_or(u16::MAX);
        stats.tiles = tile_count.try_into().unwrap_or(u32::MAX);
        let write_once = write_once_enabled();
        let mut tiles = Vec::with_capacity(tile_count);
        for tile_index in 0..tile_count {
            let tile_x = tile_index % tile_columns as usize;
            let tile_y = tile_index / tile_columns as usize;
            tiles.push(TileWork::new(
                &request,
                stroke_pixels,
                write_once,
                tile_boundary(request.width, tile_x, tile_size),
                tile_boundary(request.width, tile_x + 1, tile_size),
                tile_boundary(request.height, tile_y, tile_size),
                tile_boundary(request.height, tile_y + 1, tile_size),
            )?);
        }
        let passes = styled_passes(scene, styled, bin.as_ref(), write_once);
        // the plan's pages per plane: what a layer needs when coverage says
        // nothing, and the conservative answer for a deferred edge
        let mut planes_of_layer: Vec<(u32, usize)> = styled
            .layers
            .iter()
            .enumerate()
            .map(|(plane, layer)| (layer.layer_idx, plane))
            .collect();
        planes_of_layer.sort_unstable();
        let mut pages_by_plane: Vec<Vec<u32>> = vec![Vec::new(); styled.layers.len()];
        for &page_id in &scene.plan().pages {
            let Some((layer_idx, _)) = scene.page_geometry(page_id) else {
                continue;
            };
            if let Ok(at) = planes_of_layer.binary_search_by_key(&layer_idx, |entry| entry.0) {
                pages_by_plane[planes_of_layer[at].1].push(page_id);
            }
        }
        let hierarchy_frames = styled.hierarchy_frames;
        let bin_ref = bin.as_ref();
        for_each_tile(&mut tiles, workers, |tile| {
            tile.prepare_minis(scene, bin_ref, hierarchy_frames, guard)
        })?;
        Ok(LayerRasterSession {
            request,
            bin,
            passes,
            stroke_pixels,
            workers,
            tiles,
            tile_columns,
            tile_rows,
            stats,
            labels,
            labels_truncated,
            started,
            pages_by_plane,
        })
    }

    pub fn begin_cancellable(
        scene: &FrameScene,
        styled: &StyledGeometryRasterRequest,
        work_bin: bool,
        generation: u64,
        cancellation: &RenderCancellation,
    ) -> Result<Self, String> {
        Self::begin(
            scene,
            styled,
            work_bin,
            Some(RenderGuard {
                generation,
                cancellation,
            }),
        )
    }

    /// The styled plane a pass paints, if it paints one (a hierarchy frame
    /// band paints no layer).
    fn plane_of(pass: TilePass) -> Option<usize> {
        match pass {
            TilePass::Plane(plane) => Some(plane),
            TilePass::Frames(_) => None,
        }
    }

    pub fn passes(&self) -> usize {
        self.passes.len()
    }

    /// Runs the frame in blocks of `block` consecutive passes, calling
    /// `before_block` on this thread before each block with the styled planes
    /// it is about to paint. A worker takes a tile and paints the whole block
    /// into it, top layer first; the workers meet at a barrier only at the
    /// block's end, which is where the caller decides what the next block
    /// needs.
    ///
    /// Tiles are independent and a tile's own pass order never changes, so
    /// every block size paints the same pixels - `block >= passes()` is the
    /// normal render's schedule, `block == 1` stops at every layer. The
    /// barrier is what the caller pays for looking: it also takes the tile
    /// load balancing away, because the frame's time goes from the longest
    /// tile to the sum of the longest tile of each block (measured
    /// 2026-09-20: about +100 ms on a 449-layer frame at block 1).
    pub fn render_layered<F>(
        self,
        scene: &FrameScene,
        styled: &StyledGeometryRasterRequest,
        guard: Option<RenderGuard<'_>>,
        block: usize,
        mut before_block: F,
    ) -> Result<GeometryRasterReport, String>
    where
        F: FnMut(&[usize], &BlockDemand<'_>) -> Result<(), String>,
    {
        let block = block.max(1);
        let LayerRasterSession {
            request,
            bin,
            passes,
            stroke_pixels,
            workers,
            tiles,
            tile_columns,
            tile_rows,
            stats,
            labels,
            labels_truncated,
            started,
            pages_by_plane,
        } = self;
        let tiles: Vec<std::sync::Mutex<TileWork>> =
            tiles.into_iter().map(std::sync::Mutex::new).collect();
        let (bin, passes) = (bin.as_ref(), passes.as_slice());
        let barrier = std::sync::Barrier::new(workers + 1);
        let pass_index = AtomicUsize::new(0);
        let cursor = AtomicUsize::new(0);
        let stop = std::sync::atomic::AtomicBool::new(false);
        let failure = std::sync::Mutex::new(None::<String>);
        let (tiles_ref, barrier, pass_index, cursor, stop, failure) = (
            &tiles, &barrier, &pass_index, &cursor, &stop, &failure,
        );
        std::thread::scope(|scope| -> Result<(), String> {
            for _ in 0..workers {
                scope.spawn(move || loop {
                    barrier.wait();
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    let first = pass_index.load(Ordering::Relaxed);
                    let last = (first + block).min(passes.len());
                    loop {
                        let tile_index = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some(slot) = tiles_ref.get(tile_index) else {
                            break;
                        };
                        let Ok(mut tile) = slot.lock() else {
                            break;
                        };
                        let tile_started = Instant::now();
                        for at in first..last {
                            if tile.full {
                                break;
                            }
                            if let Err(error) = raster_tile_pass(
                                scene,
                                &request,
                                styled,
                                bin,
                                &mut tile,
                                passes[at],
                                passes.len() - at,
                                stroke_pixels,
                                guard,
                            ) {
                                if let Ok(mut failure) = failure.lock() {
                                    failure.get_or_insert(error);
                                }
                                break;
                            }
                        }
                        tile.stats.raster_tile_max_us =
                            tile.stats.raster_tile_max_us.saturating_add(
                                tile_started
                                    .elapsed()
                                    .as_micros()
                                    .try_into()
                                    .unwrap_or(u64::MAX),
                            );
                    }
                    barrier.wait();
                });
            }
            let mut result = Ok(());
            let mut planes: Vec<usize> = Vec::with_capacity(block);
            for at in (0..passes.len()).step_by(block) {
                planes.clear();
                planes.extend(
                    passes[at..(at + block).min(passes.len())]
                        .iter()
                        .filter_map(|&pass| Self::plane_of(pass)),
                );
                let guards = tiles
                    .iter()
                    .map(|tile| tile.lock().map_err(|_| "raster tile lock poisoned".to_string()))
                    .collect::<Result<Vec<_>, String>>()?;
                let demand = BlockDemand {
                    request: &request,
                    styled,
                    scene,
                    bin,
                    tiles: &guards,
                    pages_by_plane: &pages_by_plane,
                };
                let asked = before_block(&planes, &demand);
                drop(demand);
                drop(guards);
                if let Err(error) = asked {
                    result = Err(error);
                    break;
                }
                pass_index.store(at, Ordering::Relaxed);
                cursor.store(0, Ordering::Relaxed);
                barrier.wait();
                barrier.wait();
                let failed = failure.lock().map_err(|_| "raster failure lock poisoned".to_string())?.clone();
                if let Some(error) = failed {
                    result = Err(error);
                    break;
                }
            }
            stop.store(true, Ordering::Release);
            barrier.wait();
            result
        })?;
        let mut stats = stats;
        let mut counters = RasterCounters::default();
        let mut bands = Vec::with_capacity(tiles.len());
        for slot in tiles {
            let output = slot
                .into_inner()
                .map_err(|_| "raster tile lock poisoned".to_string())?
                .output();
            add_stats(&mut stats, &output.stats);
            counters.add(&output.counters);
            bands.push(output.tile);
        }
        finish_geometry_frame(
            &request,
            RenderMode::Styled(styled),
            scene,
            bands,
            tile_columns,
            tile_rows,
            stats,
            counters,
            labels.as_ref(),
            labels_truncated,
            false,
            started,
            guard,
        )
    }

    pub fn render_layered_cancellable<F>(
        self,
        scene: &FrameScene,
        styled: &StyledGeometryRasterRequest,
        generation: u64,
        cancellation: &RenderCancellation,
        block: usize,
        before_block: F,
    ) -> Result<GeometryRasterReport, String>
    where
        F: FnMut(&[usize], &BlockDemand<'_>) -> Result<(), String>,
    {
        self.render_layered(
            scene,
            styled,
            Some(RenderGuard {
                generation,
                cancellation,
            }),
            block,
            before_block,
        )
    }
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

/// One full-frame label pass over the assembled geometry (§F2R-16):
/// gray block labels, per-plane layer labels in plane order, white
/// block labels - the same label-vs-label order the per-tile passes
/// used, now unconditionally above all geometry.
fn apply_label_passes(
    request: &GeometryRasterRequest,
    styled: &StyledGeometryRasterRequest,
    labels: &PreparedLabels,
    frame: RgbaFrame,
    counters: &mut RasterCounters,
    guard: Option<RenderGuard<'_>>,
) -> Result<RgbaFrame, String> {
    let mut band = RasterBand {
        width: frame.width,
        height: frame.height,
        col0: 0,
        col1: frame.width,
        row0: 0,
        row1: frame.height,
        pixels: frame.pixels,
        rep_spans: Vec::new(),
        once: None,
    };
    if styled.hierarchy_frames {
        render_prepared_labels(
            Some(labels),
            &mut band,
            LabelSelection::Block { white: false },
            [128, 128, 128, 255],
            counters,
            guard,
        )?;
    }
    for layer in &styled.layers {
        check_cancelled(guard)?;
        let color = if styled.mono {
            monochrome(layer.color)
        } else {
            layer.color
        };
        render_prepared_labels(
            Some(labels),
            &mut band,
            LabelSelection::Layer(layer.layer_idx),
            color,
            counters,
            guard,
        )?;
    }
    if styled.hierarchy_frames {
        render_prepared_labels(
            Some(labels),
            &mut band,
            LabelSelection::Block { white: true },
            [255, 255, 255, 255],
            counters,
            guard,
        )?;
    }
    Ok(RgbaFrame {
        width: request.width,
        height: request.height,
        pixels: band.pixels,
    })
}

/// The rastered tiles that intersect `window` copied into a frame of
/// the window's size (the tiles' own pixels are the full frame's, so
/// the copy is a crop: byte-equal to the full render's window).
fn assemble_window(
    request: &GeometryRasterRequest,
    tiles: Vec<RasterBand>,
    window: [u32; 4],
) -> Result<RgbaFrame, String> {
    let [wc0, wr0, wc1, wr1] = window;
    let (w, h) = (wc1 - wc0, wr1 - wr0);
    let byte_len = (w as usize)
        .checked_mul(h as usize)
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| "image byte length overflow".to_string())?;
    let mut pixels = vec![0u8; byte_len];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&request.background);
    }
    let stride = w as usize * 4;
    for tile in tiles {
        if tile.width != request.width || tile.height != request.height {
            return Err("raster worker returned an invalid tile".to_string());
        }
        let c0 = tile.col0.max(wc0);
        let c1 = tile.col1.min(wc1);
        let r0 = tile.row0.max(wr0);
        let r1 = tile.row1.min(wr1);
        if c0 >= c1 || r0 >= r1 {
            continue;
        }
        let tile_stride = (tile.col1 - tile.col0) as usize * 4;
        for row in r0..r1 {
            let src = (row - tile.row0) as usize * tile_stride + (c0 - tile.col0) as usize * 4;
            let dst = (row - wr0) as usize * stride + (c0 - wc0) as usize * 4;
            let len = (c1 - c0) as usize * 4;
            pixels[dst..dst + len].copy_from_slice(&tile.pixels[src..src + len]);
        }
    }
    RgbaFrame::from_pixels(w, h, pixels)
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
    summary_cells_drawn: u64,
    summary_pixels_drawn: u64,
}

impl RasterCounters {
    fn add(&mut self, other: &Self) {
        self.summary_cells_drawn = self.summary_cells_drawn.saturating_add(other.summary_cells_drawn);
        self.summary_pixels_drawn = self
            .summary_pixels_drawn
            .saturating_add(other.summary_pixels_drawn);
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
    total.representative_spans = total.representative_spans.saturating_add(worker.representative_spans);
    total.representative_pixels = total.representative_pixels.saturating_add(worker.representative_pixels);
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
    total.once_full_tiles = total.once_full_tiles.saturating_add(worker.once_full_tiles);
    total.once_passes_skipped = total.once_passes_skipped.saturating_add(worker.once_passes_skipped);
    total.once_items_skipped = total.once_items_skipped.saturating_add(worker.once_items_skipped);
    total.raster_tile_max_us = total.raster_tile_max_us.max(worker.raster_tile_max_us);
    total.tiles_reused = total.tiles_reused.saturating_add(worker.tiles_reused);
    for (sum, walks) in total.place_walks.iter_mut().zip(worker.place_walks.iter()) {
        sum.0 = sum.0.saturating_add(walks.0);
        sum.1 = sum.1.saturating_add(walks.1);
    }
}

/// Queries one decoded page's record index against a tile-local view
/// and paints the intersecting records. Shared verbatim by the
/// hierarchy walk and the work-bin tile path (F2R-03b 2c) so the two
/// produce identical paint sequences.
#[allow(clippy::too_many_arguments)]
/// The page frontier's record thinning (floe_vfs::hier, WsCell::
/// page_levels): a page kept `level` levels below its cut draws one
/// item in 2^level - a record's repetition members absorb
/// min(level, log2 members) of them (balanced strides for a grid,
/// every 2^lm-th point) and its index within the page the rest.
/// None: the record is skipped; Borrowed: drawn as it is.
fn thin_record<'a>(rep: &'a Rep, level: u8, record: usize) -> Option<std::borrow::Cow<'a, Rep>> {
    if level == 0 {
        return Some(std::borrow::Cow::Borrowed(rep));
    }
    let lm = floe_vfs::hier::member_levels(rep.members(), level as u32);
    let lr = level as u32 - lm;
    if lr > 0 && record % (1usize << lr.min(60)) != 0 {
        return None;
    }
    if lm == 0 {
        return Some(std::borrow::Cow::Borrowed(rep));
    }
    Some(std::borrow::Cow::Owned(match rep {
        Rep::One => Rep::One,
        Rep::Grid { na, nb, va, vb } => {
            let (na, nb, va, vb) = floe_vfs::hier::thin_grid(*na, *nb, *va, *vb, lm);
            Rep::Grid { na, nb, va, vb }
        }
        Rep::Pts(p) => Rep::Pts(floe_vfs::hier::thin_pts(p, lm)),
    }))
}

/// the side the per-shape cut judges a shape's bbox by
fn cut_side_of(base: BBox, larger: bool) -> i64 {
    let (w, h) = (base.x1 - base.x0, base.y1 - base.y0);
    if larger { w.max(h) } else { w.min(h) }
}

fn raster_page_records(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    page: &crate::DecodedPage,
    page_id: u32,
    level: u8,
    shape_cut: i64,
    shape_cut_max: bool,
    local_view: BBox,
    world_transform: OrthoTransform,
    lattice: Option<(i64, i64)>,
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
            if band.is_full() {
                return Ok(());
            }
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
            // the per-shape cut (HierStats::shape_cut): the whole record,
            // its members share the size; by the smaller side, or by the
            // larger one when the hairlines are to stay (shape_cut_max:
            // the width-first drawing thins them by their width)
            let cut_side = if shape_cut_max { rect.w.max(rect.h) } else { rect.w.min(rect.h) };
            if rect.w == 0 || rect.h == 0 {
                return Ok(());
            }
            if cut_side < shape_cut && !keeps_sub_cut_array(request, paint, &rect.rep, level, &world_transform)? {
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
            let Some(rep) = thin_record(&rect.rep, level, record as usize) else {
                return Ok(());
            };
            let chunks = if matches!(rep, std::borrow::Cow::Borrowed(_)) {
                page.index.pts_chunks(&rect.rep)
            } else {
                None
            };
            // area-true members of a whole array spread their width decisions
            // by their index (GridRanks); a thinned repetition keeps the hash
            let grid = if area_true_rim(request, paint) && matches!(rep, std::borrow::Cow::Borrowed(_)) {
                match (&rect.rep, lattice) {
                    // the placement lattice: a single rectangle placed by a
                    // lattice array ranks on it, as the array of it would
                    (Rep::One, Some((px, py))) => Some(GridRanks::lattice(px, py, world_transform.apply_bbox(base)?)),
                    _ => GridRanks::new(&rect.rep, &world_transform, world_transform.apply_bbox(base)?)?,
                }
            } else {
                None
            };
            let mut drawn = 0u64;
            let mut cancel_member = 0u16;
            let walk = survivor_walk(request, grid.as_ref(), &rep, base, local_view, &world_transform)?;
            let mut member = |offset_x: i64, offset_y: i64| -> Result<(), String> {
                check_member_cancelled(guard, &mut cancel_member)?;
                if band.is_full() {
                    return Err(WRITE_ONCE_FULL.to_string());
                }
                let local = translate_bbox(base, offset_x, offset_y)?;
                let world = world_transform.apply_bbox(local)?;
                let painted = match grid.as_ref().and_then(|g| g.ranks(offset_x, offset_y, &world)) {
                    Some(ranks) => paint_width_first_rect(band, request, world, paint, ranks)?,
                    None => paint_world_rect(band, request, world, paint)?,
                };
                if painted {
                    drawn = drawn.saturating_add(1);
                }
                Ok(())
            };
            // a sub-pixel lattice array walks only the members that can
            // survive, in the member walk's order (the same pixels)
            let visit = until_full(match walk {
                Some(walk) => walk.run(guard, &mut SurvivorWork::default(), &mut member),
                None => for_each_visible_offset_chunked(&rep, chunks, base, local_view, &mut member),
            })?;
            stats.rep_members_tested = stats
                .rep_members_tested
                .saturating_add(visit.map_or(0, |visit| visit.tested));
            stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(drawn);
            stats.primitives_drawn = stats.primitives_drawn.saturating_add(drawn);
            counters.rectangle_members_drawn =
                counters.rectangle_members_drawn.saturating_add(drawn);
            Ok(())
        })?;

    page.index
        .polys()
        .for_each_intersecting(local_view, record_scratch, |record| {
            if band.is_full() {
                return Ok(());
            }
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
            if cut_side_of(base, shape_cut_max) < shape_cut && !keeps_sub_cut_array(request, paint, &polygon.rep, level, &world_transform)? {
                return Ok(());
            }
            let mut drawn = 0u64;
            let mut cancel_member = 0u16;
            // One scratch per record, reused by every repetition member.
            let mut world_points = Vec::with_capacity(polygon.pts.len());
            let Some(rep) = thin_record(&polygon.rep, level, record as usize) else {
                return Ok(());
            };
            let chunks = if matches!(rep, std::borrow::Cow::Borrowed(_)) {
                page.index.pts_chunks(&polygon.rep)
            } else {
                None
            };
            // the placement lattice: a sub-pixel polygon keeps by its lattice
            // rank - its own array's, or the placement array's for a single one
            let keep_lattice = area_keep_lattice(request, paint, &polygon.rep, matches!(rep, std::borrow::Cow::Borrowed(_)), &world_transform, lattice)?;
            let walk = match keep_lattice {
                Some((px, py)) => area_survivor_walk(request, &polygon.rep, base, polygon_area(&polygon.pts), local_view, &world_transform, px, py)?,
                None => None,
            };
            let mut member = |offset_x: i64, offset_y: i64| -> Result<(), String> {
                check_member_cancelled(guard, &mut cancel_member)?;
                if band.is_full() {
                    return Err(WRITE_ONCE_FULL.to_string());
                }
                world_points.clear();
                for &(x, y) in &polygon.pts {
                    let x = checked_add(x, offset_x, "polygon x")?;
                    let y = checked_add(y, offset_y, "polygon y")?;
                    world_points.push(world_transform.apply(x, y)?);
                }
                let rank = keep_lattice
                    .and_then(|(px, py)| polygon_bbox(&world_points).map(|world| lattice_area_rank(px, py, &world)));
                if paint_world_polygon_ranked(band, request, &world_points, paint, rank)? {
                    drawn = drawn.saturating_add(1);
                }
                Ok(())
            };
            let visit = until_full(match walk {
                Some(walk) => walk.run(guard, &mut SurvivorWork::default(), &mut member),
                None => for_each_visible_offset_chunked(&rep, chunks, base, local_view, &mut member),
            })?;
            stats.rep_members_tested = stats
                .rep_members_tested
                .saturating_add(visit.map_or(0, |visit| visit.tested));
            stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(drawn);
            stats.primitives_drawn = stats.primitives_drawn.saturating_add(drawn);
            counters.polygon_members_drawn =
                counters.polygon_members_drawn.saturating_add(drawn);
            Ok(())
        })?;

    page.index
        .paths()
        .for_each_intersecting(local_view, record_scratch, |record| {
            if band.is_full() {
                return Ok(());
            }
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
            if cut_side_of(base, shape_cut_max) < shape_cut && !keeps_sub_cut_array(request, paint, &path_record.rep, level, &world_transform)? {
                return Ok(());
            }
            let mut drawn = 0u64;
            let mut cancel_member = 0u16;
            // One outline/centerline scratch pair per record, reused by
            // every repetition member.
            let mut world_points = Vec::with_capacity(outline.len());
            let mut world_centerline = Vec::with_capacity(centerline.len());
            let Some(rep) = thin_record(&path_record.rep, level, record as usize) else {
                return Ok(());
            };
            let chunks = if matches!(rep, std::borrow::Cow::Borrowed(_)) {
                page.index.pts_chunks(&path_record.rep)
            } else {
                None
            };
            let keep_lattice = area_keep_lattice(request, paint, &path_record.rep, matches!(rep, std::borrow::Cow::Borrowed(_)), &world_transform, lattice)?;
            let visit = until_full(for_each_visible_offset_chunked(
                &rep,
                chunks,
                base,
                local_view,
                |offset_x, offset_y| {
                    check_member_cancelled(guard, &mut cancel_member)?;
                    if band.is_full() {
                        return Err(WRITE_ONCE_FULL.to_string());
                    }
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
                    let rank = keep_lattice
                        .and_then(|(px, py)| polygon_bbox(&world_points).map(|world| lattice_area_rank(px, py, &world)));
                    if paint_world_path_ranked(band, request, &world_points, &world_centerline, paint, rank)?
                    {
                        drawn = drawn.saturating_add(1);
                    }
                    Ok(())
                },
            ))?;
            stats.rep_members_tested = stats
                .rep_members_tested
                .saturating_add(visit.map_or(0, |visit| visit.tested));
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
    lattice: Option<(i64, i64)>,
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
    if band.is_full() {
        return Ok(());
    }
    stats.hier_cells_visited = stats.hier_cells_visited.saturating_add(1);
    path.push(key);
    let local_view = world_transform.invert()?.apply_bbox(cull_view)?;

    for (slot, &page_id) in cell.pages.iter().enumerate() {
        check_cancelled(guard)?;
        let Some(page) = scene.page(page_id) else {
            continue;
        };
        let level = cell.page_levels.get(slot).copied().unwrap_or(0);
        // The planner selects pages for the whole viewport, while this hot
        // loop runs independently for every image tile. Reject a page in
        // cell-local coordinates before walking any of its records. Without
        // this gate every tile rescanned every selected page; sample9's
        // 858x789 frame repeated the same record walks 49 times at 128px.
        if !selection.includes(page.layer_idx) || !page.bbox.intersects(&local_view) {
            continue;
        }
        if band.any_written() {
            if band.is_full() {
                break;
            }
            if let Ok(world) = world_transform.apply_bbox(page.bbox) {
                if band.world_box_written(request, world, paint.stroke_width) {
                    stats.once_items_skipped = stats.once_items_skipped.saturating_add(1);
                    continue;
                }
            }
        }
        raster_page_records(
            band,
            request,
            page,
            page_id,
            level,
            scene.plan().stats.shape_cut.min(i64::MAX as u64) as i64,
            scene.plan().stats.shape_cut_max,
            local_view,
            world_transform,
            lattice,
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
        if paint_world_rect(band, &marker_request(request), world, paint)? {
            counters.rectangle_members_drawn = counters.rectangle_members_drawn.saturating_add(1);
            stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(1);
            stats.primitives_drawn = stats.primitives_drawn.saturating_add(1);
        }
    }
    if path.len() == 1 {
        let mut rep_spans = std::mem::take(&mut band.rep_spans);
        for (layer_idx, prim) in &cell.reps {
            check_cancelled(guard)?;
            if !selection.includes(*layer_idx) {
                continue;
            }
            counters.rect_records = counters.rect_records.saturating_add(1);
            stats.primitives_tested = stats.primitives_tested.saturating_add(1);
            stats.rep_members_tested = stats.rep_members_tested.saturating_add(1);
            if queue_representative(band, &marker_request(request), prim, paint, &mut rep_spans, stats)? {
                counters.rectangle_members_drawn = counters.rectangle_members_drawn.saturating_add(1);
                stats.rep_members_drawn = stats.rep_members_drawn.saturating_add(1);
                stats.primitives_drawn = stats.primitives_drawn.saturating_add(1);
            }
        }
        flush_representative_spans(band, request, paint, &mut rep_spans, stats);
        band.rep_spans = rep_spans;
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
        let own = own_lattice(request, &instance.rep, &world_transform)?;
        let child_lattice = own.or(lattice);
        let walk = match own {
            Some(pitches) => placement_survivor_walk(
                scene,
                request,
                instance,
                &world_transform,
                pitches,
                base_bbox,
                local_view,
                &|layer| selection.includes(layer).then(|| area_true_rim(request, paint)),
                guard,
            stats,
            )?,
            None => None,
        };
        let mut member = |offset_x: i64, offset_y: i64| -> Result<(), String> {
            check_member_cancelled(guard, &mut cancel_member)?;
            if band.any_written() {
                if band.is_full() {
                    return Err(WRITE_ONCE_FULL.to_string());
                }
                let member = translate_bbox(base_bbox, offset_x, offset_y)?;
                if band.world_box_written(request, world_transform.apply_bbox(member)?, paint.stroke_width) {
                    stats.once_items_skipped = stats.once_items_skipped.saturating_add(1);
                    return Ok(());
                }
            }
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
                child_lattice,
                path,
                record_scratch,
            )
        };
        let visit = until_full(match walk {
            Some(walk) => walk.run(guard, &mut SurvivorWork::default(), &mut member),
            None => for_each_visible_offset(&instance.rep, base_bbox, local_view, &mut member),
        })?;
        stats.rep_members_tested = stats
            .rep_members_tested
            .saturating_add(visit.map_or(0, |visit| visit.tested));
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
    hairline_world_bbox_of(request, world, paint, None)
}

/// `hairline_world_bbox` for a shape whose area is not its box's (a polygon or
/// a path outline, in world units squared): the area-true draw keeps it with
/// the chance its OWN area fills the pixels it would light (review 2026-09-22:
/// a triangle half its box was kept as often as the box).
fn hairline_world_bbox_of(
    request: &GeometryRasterRequest,
    world: BBox,
    paint: PaintStyle,
    area: Option<f64>,
) -> Result<Option<(i128, i128, i128, i128)>, String> {
    hairline_world_bbox_ranked(request, world, paint, area, None)
}

/// `hairline_world_bbox_of` with the area-true keep decided by `rank` when
/// given (a lattice rank under the placement lattice, lattice_area_rank)
/// instead of the world box's own.
fn hairline_world_bbox_ranked(
    request: &GeometryRasterRequest,
    world: BBox,
    paint: PaintStyle,
    area: Option<f64>,
    rank: Option<f64>,
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
    if request.area_true {
        let view = request.view;
        let px_area = area.map(|a| {
            a * (request.width as f64 / (view.x1 - view.x0)) * (request.height as f64 / (view.y1 - view.y0))
        });
        return area_true_hairline(world, (x0, y0, x1, y1), sub_x, sub_y, px_area, rank).map(Some);
    }
    let point = sub_x && sub_y;
    let (cx0, cx1) = hairline_axis_span(x0, x1, sub_x, point, 0);
    let (cy0, cy1) = hairline_axis_span(y0, y1, sub_y, point, -1);
    Ok(Some((cx0, cy0, cx1, cy1)))
}

/// Area-true form of a shape under a pixel on a side (GeometryRasterRequest::
/// area_true): it lights one pixel across each such side - the one holding its
/// centre - and the pixel-centre span along a longer side, and it is KEPT with
/// the chance its area fills those pixels (a w px wide wire: w, a w x h px
/// point: w h; a polygon: its own area in px over max(w,1) max(h,1) - `area`,
/// None for a rectangle). The draw is decided by the shape's world rank, so a zoom-out
/// keeps a subset of what the zoom-in kept, a pan changes nothing, and the
/// kept shapes light, on average, as many pixels as the shapes cover: 0.1 px
/// wires 1 px apart light one column in ten instead of every column.
/// A dropped shape answers an empty box. `rank`, when given, stands for the
/// world box's rank (a lattice rank under the placement lattice).
fn area_true_hairline(
    world: BBox,
    device: (i128, i128, i128, i128),
    sub_x: bool,
    sub_y: bool,
    area: Option<f64>,
    rank: Option<f64>,
) -> Result<(i128, i128, i128, i128), String> {
    let (x0, y0, x1, y1) = device;
    let side = |d: i128| d.max(0) as f64 / DEVICE_ONE as f64;
    let keep = match area {
        None => side(x1 - x0).min(1.0) * side(y1 - y0).min(1.0),
        Some(area) => (area / (side(x1 - x0).max(1.0) * side(y1 - y0).max(1.0))).clamp(0.0, 1.0),
    };
    if rank.unwrap_or_else(|| area_rank(world)) >= keep {
        return Ok((0, 0, 0, 0));
    }
    let across = |v0: i128, v1: i128| {
        let cell = floor_div(v0 + v1, 2 * DEVICE_ONE);
        (cell, cell + 1)
    };
    let (cx0, cx1) = if sub_x { across(x0, x1) } else { fill_phase_columns(x0, x1, FillPhase::PixelCenter)? };
    let (cy0, cy1) = if sub_y { across(y0, y1) } else { fill_phase_rows(y0, y1, FillPhase::PixelCenter)? };
    Ok((cx0, cy0, cx1, cy1))
}

/// A polygon's area in world units squared (shoelace, even-odd net).
fn polygon_area(points: &[(i64, i64)]) -> f64 {
    let mut twice: i128 = 0;
    for k in 0..points.len() {
        let (x0, y0) = points[k];
        let (x1, y1) = points[(k + 1) % points.len()];
        twice += x0 as i128 * y1 as i128 - x1 as i128 * y0 as i128;
    }
    twice.unsigned_abs() as f64 / 2.0
}

/// A shape box's rank in [0, 1) for the area-true sub-pixel draw: a function
/// of its WORLD box only, so it is the same at every zoom, pan, tile and
/// worker, and exact duplicates share it (they light the same pixel anyway).
fn area_rank(world: BBox) -> f64 {
    salted_rank(world, 0)
}

/// `area_rank` under a salt: salts 1 and 2 are the x and y ranks of a
/// width-first rectangle - independent, so a 0.5 x 0.5 px box shows 1 time
/// in 4, not 1 in 2 (review 2026-09-22).
fn salted_rank(world: BBox, salt: u64) -> f64 {
    let mut h: u64 = 0x243F_6A88_85A3_08D3 ^ salt.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    for v in [world.x0, world.y0, world.x1, world.y1] {
        h = (h ^ v as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
        h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        h ^= h >> 31;
    }
    (h >> 11) as f64 / (1u64 << 53) as f64
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
    if band.once.is_some() {
        return Ok(band.write_once_rows(first_row, end_row, first_col, end_col, paint.color, |_| SpanRule::All));
    }
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
    if area_true_rim(request, paint) {
        return paint_area_true_rect(band, request, world, paint);
    }
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

/// The request a MARKER is painted with: washes, wash points and stored
/// representatives stand for what the cut dropped - a sub-pixel one is a dot
/// that must show, not a shape to keep by its area - so they keep the KLayout
/// rule whatever GeometryRasterRequest::area_true says.
fn marker_request(request: &GeometryRasterRequest) -> GeometryRasterRequest {
    GeometryRasterRequest { area_true: false, ..*request }
}

/// Whether a shape at least a pixel on both sides takes the area-true form
/// (GeometryRasterRequest::area_true): a 1 px solid outline, the style every
/// hairline collapse assumes too. A thicker or dotted outline keeps the
/// KLayout fill and edge stroke - a deliberate heavy style.
fn area_true_rim(request: &GeometryRasterRequest, paint: PaintStyle) -> bool {
    request.area_true && matches!(paint.stroke, StrokeStyle::Solid) && paint.stroke_width == 1
}

/// Area-true rectangle, WIDTH FIRST (user decision 2026-09-22), at any size:
/// each axis draws m = ceil(w - t) pixels - the whole pixels of its w px
/// always, one more when its fraction beats t - centred on the rectangle, t
/// the rectangle's world rank for that axis (`width_first_span`). The pixels
/// take the layer's fill and their rim is the outline. What it keeps: each
/// rectangle's whole pixels and, over rectangles, its mean width (a width
/// never changes under a pan, and shrinks monotonically when zooming out -
/// a sub-pixel side shows when w > t, the same shape at every scale below).
/// What it does not: a gap under 2 px may close and neighbours may land on
/// the same pixels (quantization error, accepted - the picture as a whole
/// comes first). The box never leaves the pixels the rectangle touches.
fn paint_area_true_rect(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    world: BBox,
    paint: PaintStyle,
) -> Result<bool, String> {
    paint_width_first_rect(band, request, world, paint, (salted_rank(world, 1), salted_rank(world, 2)))
}

/// `paint_area_true_rect` under given x and y ranks: an array member's come
/// from its index (GridRanks), any other rectangle's from its world box.
fn paint_width_first_rect(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    world: BBox,
    paint: PaintStyle,
    ranks: (f64, f64),
) -> Result<bool, String> {
    let (x0, y1) = world_to_device(request, world.x0, world.y0)?;
    let (x1, y0) = world_to_device(request, world.x1, world.y1)?;
    let Some((c0, c1)) = width_first_span_c(x0, x1, ranks.0, request.width_c) else {
        return Ok(false);
    };
    let Some((r0, r1)) = width_first_span_c(y0, y1, ranks.1, request.width_c) else {
        return Ok(false);
    };
    let (d0, d1) = (c0 * DEVICE_ONE, c1 * DEVICE_ONE);
    let (e0, e1) = (r0 * DEVICE_ONE, r1 * DEVICE_ONE);
    let filled = fill_device_rect_with_phase(band, request, d0, e0, d1, e1, FillPhase::PixelCenter, paint)?;
    let mut rim = false;
    for piece in [(c0, r0, c1, r0 + 1), (c0, r1 - 1, c1, r1), (c0, r0, c0 + 1, r1), (c1 - 1, r0, c1, r1)] {
        rim |= paint_hairline_device_rect(band, request, piece, paint)?;
    }
    Ok(filled || rim)
}

/// Width-first ranks of the members of one rectangle ARRAY (a Grid repetition;
/// user decision 2026-09-22): a world-box hash spreads the extra pixels of an
/// array at random - runs of wide and narrow members - so an array member's
/// rank comes from an index instead. For the world x axis, with ix the index
/// along x and iy the one along y, the x rank is frac(u_x + vdc(ix) + phi iy):
/// vdc (the bit-reversed index, van der Corput) lets any run of neighbours
/// take its share of extra pixels - a half makes it one in two - and phi iy
/// (the golden ratio, as a 64-bit Weyl step) shifts each row so the rows do
/// not repeat each other. The y rank swaps ix and iy, so in a one-row array
/// it is frac(u_y + phi ix) and the two axes stay independent (a 0.5 x 0.5 px
/// member shows 1 time in 4).
///
/// An array whose world repetition vectors run along x and y (the common,
/// axis-aligned case) counts on the WORLD LATTICE (user decision 2026-09-22,
/// ADAPTIVE_CUT_DENSITY_PLAN §4.2 candidate 2): along an axis of pitch P the
/// index is the member's world bbox corner div_euclid P, and u is a hash of
/// the pitches, the phases (corner rem_euclid P; the fixed coordinate on an
/// axis without repetition) and the member's world size - never of the
/// record's first member, count or numbering. So the same world lattice
/// draws the same members whether it is stored as one Grid, as the index's
/// fragments (frag_rep re-bases and renumbers them), with its axes swapped or
/// a pitch negated, or placed rotated or mirrored. What it cannot see: a
/// one-member fragment is a Rep::One, and a 1 x 1 grid ranks like one - the
/// single shape's world-box hash. A one-row piece of a 2-D grid is written as
/// a one-dimensional repetition (the page file drops the other vector), so
/// nothing tells it from an array stored as that row from the start: it ranks
/// as that row's own lattice - its pitch, the fixed coordinate across, the
/// same under further splits of the row - and apart from the 2-D lattice it
/// was cut from (review 2026-09-23). A skewed grid keeps the per-record index
/// (the record's own (i, j), u its first member's world-box ranks); a
/// collinear 2-D grid has no clean index and keeps the member's own world-box
/// ranks.
struct GridRanks {
    mode: GridMode,
    u: (f64, f64),
}

enum GridMode {
    /// world pitches along x and y (0 = no repetition along that axis)
    Lattice { px: i64, py: i64 },
    /// the record's own index (skewed grids)
    Index { va: (i64, i64), vb: (i64, i64), na: u64, nb: u64, det: i128, x_along_a: bool },
}

/// An array's repetition vectors in the world (the linear part of the transform).
fn world_vectors(va: (i64, i64), vb: (i64, i64), world_transform: &OrthoTransform) -> Result<((i64, i64), (i64, i64)), String> {
    let origin = world_transform.apply(0, 0)?;
    let a = world_transform.apply(va.0, va.1)?;
    let b = world_transform.apply(vb.0, vb.1)?;
    Ok(((a.0 - origin.0, a.1 - origin.1), (b.0 - origin.0, b.1 - origin.1)))
}

/// The world lattice of an array (GridRanks' lattice mode): every repeating
/// vector along x or y, two of them along different axes - its pitches
/// (px, py), 0 along an axis without repetition. None for a skewed or
/// collinear array or one without a repeating vector.
fn world_lattice(na: u64, nb: u64, wa: (i64, i64), wb: (i64, i64)) -> Option<(i64, i64)> {
    let along = |v: (i64, i64)| -> Option<(i64, i64)> {
        match v {
            (0, 0) => None,
            (x, 0) => Some((x.abs(), 0)),
            (0, y) => Some((0, y.abs())),
            _ => None,
        }
    };
    let steps = [(na > 1, wa), (nb > 1, wb)]
        .iter()
        .filter(|(many, _)| *many)
        .map(|&(_, v)| along(v))
        .collect::<Option<Vec<(i64, i64)>>>()?;
    if steps.is_empty() {
        return None;
    }
    let px = steps.iter().map(|s| s.0).max().unwrap_or(0);
    let py = steps.iter().map(|s| s.1).max().unwrap_or(0);
    // two repeating vectors must not share an axis
    let distinct = steps.len() < 2 || (steps[0].0 == 0) != (steps[1].0 == 0);
    distinct.then_some((px, py))
}

/// The world lattice of a placement array (GeometryRasterRequest::
/// place_lattice): its pitches when its vectors placed by `world_transform`
/// (the transform of the cell holding it) run along the world axes.
fn placement_lattice(rep: &Rep, world_transform: &OrthoTransform) -> Result<Option<(i64, i64)>, String> {
    let Rep::Grid { na, nb, va, vb } = rep else {
        return Ok(None);
    };
    let (wa, wb) = world_vectors(*va, *vb, world_transform)?;
    Ok(world_lattice(*na, *nb, wa, wb))
}

/// An instance's own placement lattice when the placement lattice is on and
/// it is a lattice array (None otherwise): the members below it lie under
/// it, and under the one above when it has none - the innermost lattice
/// array on the path. Every member walk - the work bin's collection, its
/// deferred edges' mini walks and their fallback, the per-tile walk - takes
/// its members' lattice this way, whether or not it lists survivors.
fn own_lattice(
    request: &GeometryRasterRequest,
    rep: &Rep,
    world_transform: &OrthoTransform,
) -> Result<Option<(i64, i64)>, String> {
    if !request.place_lattice {
        return Ok(None);
    }
    placement_lattice(rep, world_transform)
}

/// A placement array's cell is small enough to have its survivors listed at
/// most this many shapes on the drawn layers.
const SMALL_CELL_SHAPES: usize = 8;

/// The most work the preparation of a placement array's survivor walk may do
/// (review 2026-09-25): one unit per record of the drawn layers it looks at -
/// those the shape cut drops included - and one per polygon vertex. A cell
/// past it is walked member by member; the preparation runs again at every
/// visit of the array's parent, so it must stay small whatever the cell holds.
const PLACEMENT_PREP_WORK: u64 = 1024;

/// The survivor walk of a placement array under the placement lattice
/// (GeometryRasterRequest::place_lattice with survivor_list): `instance`, in
/// the cell placed at `parent_world`, a lattice array of pitches `pitches`.
/// Its cell must be a small leaf - no placements, frames, washes or stored
/// representatives, page levels 0, pages decoded, at most SMALL_CELL_SHAPES
/// single rectangles and polygons on the layers `layer_rim` says this walk
/// draws (Some(rim): drawn, rim = the area-true 1 px outline; a drawn layer
/// without it draws every shape whole) - and every one of those shapes under a
/// pixel along one lattice axis A for every member: a rectangle's width-first
/// chance there, a polygon's area-true keep chance (its rank lists on x when
/// the lattice repeats along x). Each shape is one term of the walk, keyed by
/// its own world box on the lattice (GridRanks::lattice, lattice_area_rank),
/// so the walk visits every member where some shape can be drawn and the
/// members it skips draw nothing: the pixels are those of every member's
/// visit. Shapes the shape cut drops are no term; a path, an array record or
/// any shape that may be a pixel or more along A leaves the members to the
/// full walk (None) - the ranks are the same either way.
#[allow(clippy::too_many_arguments)]
fn placement_survivor_walk(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    instance: &floe_vfs::hier::WsInst,
    parent_world: &OrthoTransform,
    pitches: (i64, i64),
    base_bbox: BBox,
    local_view: BBox,
    layer_rim: &dyn Fn(u32) -> Option<bool>,
    guard: Option<RenderGuard<'_>>,
    stats: &mut RenderStats,
) -> Result<Option<SurvivorWalk>, String> {
    if !request.survivor_list {
        return Ok(None);
    }
    let range = visible_grid_range(&instance.rep, base_bbox, local_view);
    let planned = plan_placement_walk(scene, request, instance, parent_world, pitches, range, layer_rim, guard)?;
    let members = range.map_or(0, |(i0, i1, j0, j1)| ((i1 - i0 + 1) as u64).saturating_mul((j1 - j0 + 1) as u64));
    let two = matches!(instance.rep, Rep::Grid { na, nb, .. } if na > 1 && nb > 1);
    let (outcome, walk) = match planned {
        Ok(walk) => (PlaceWalkOutcome::Walked, Some(walk)),
        Err(outcome) => (outcome, None),
    };
    let slot = &mut stats.place_walks[outcome as usize + if two { PLACE_WALK_OUTCOMES.len() } else { 0 }];
    slot.0 = slot.0.saturating_add(1);
    slot.1 = slot.1.saturating_add(members);
    Ok(walk)
}

/// The plan of `placement_survivor_walk`, or why every member is visited.
#[allow(clippy::too_many_arguments)]
fn plan_placement_walk(
    scene: &FrameScene,
    request: &GeometryRasterRequest,
    instance: &floe_vfs::hier::WsInst,
    parent_world: &OrthoTransform,
    pitches: (i64, i64),
    range: Option<(i64, i64, i64, i64)>,
    layer_rim: &dyn Fn(u32) -> Option<bool>,
    guard: Option<RenderGuard<'_>>,
) -> Result<Result<SurvivorWalk, PlaceWalkOutcome>, String> {
    use PlaceWalkOutcome as Why;
    let (px, py) = pitches;
    let Rep::Grid { va, vb, .. } = &instance.rep else {
        return Ok(Err(Why::NoAxis));
    };
    let Some(child) = scene.cell(instance.child) else {
        return Ok(Err(Why::NotLeaf));
    };
    if !child.insts.is_empty() || !child.frames.is_empty() || !child.washes.is_empty() || !child.reps.is_empty() {
        return Ok(Err(Why::NotLeaf));
    }
    let Some(range) = range else {
        return Ok(Err(Why::NoRange));
    };
    let shape_cut = scene.plan().stats.shape_cut.min(i64::MAX as u64) as i64;
    let shape_cut_max = scene.plan().stats.shape_cut_max;
    let member0 = parent_world.compose(&OrthoTransform::place(instance.x, instance.y, instance.rot, instance.flip)?)?;
    let c = request.width_c;
    // per rectangle its term along each axis it is under a pixel on; per
    // polygon its keep axis and term - a ninth shape, a path or work past
    // PLACEMENT_PREP_WORK gives up at once
    let mut rects: Vec<[Option<LatticeTerm>; 2]> = Vec::with_capacity(SMALL_CELL_SHAPES);
    let mut areas: Vec<(usize, LatticeTerm)> = Vec::with_capacity(SMALL_CELL_SHAPES);
    let mut work = 0u64;
    let mut heartbeat = 0u16;
    for (slot, &page_id) in child.pages.iter().enumerate() {
        check_cancelled(guard)?;
        if child.page_levels.get(slot).copied().unwrap_or(0) != 0 {
            return Ok(Err(Why::PageLevel));
        }
        let Some(page) = scene.page(page_id) else {
            return Ok(Err(Why::Undecoded));
        };
        let Some(rim) = layer_rim(page.layer_idx) else {
            continue;
        };
        let Some(geometry) = page.doc.cells.get(page.doc.top) else {
            return Ok(Err(Why::Undecoded));
        };
        if !geometry.paths.is_empty() {
            return Ok(Err(Why::Path));
        }
        for rect in &geometry.rects {
            check_member_cancelled(guard, &mut heartbeat)?;
            work += 1;
            if work > PLACEMENT_PREP_WORK {
                return Ok(Err(Why::PrepWork));
            }
            let base = BBox { x0: rect.x, y0: rect.y, x1: rect.x.saturating_add(rect.w), y1: rect.y.saturating_add(rect.h) };
            if rect.w <= 0 || rect.h <= 0 || cut_side_of(base, shape_cut_max) < shape_cut {
                continue;
            }
            if !rim {
                return Ok(Err(Why::NonRim));
            }
            if !matches!(rect.rep, Rep::One) {
                return Ok(Err(Why::ArrayRecord));
            }
            if rects.len() + areas.len() == SMALL_CELL_SHAPES {
                return Ok(Err(Why::Shapes));
            }
            let world = member0.apply_bbox(base)?;
            let (x0, y1) = world_to_device(request, world.x0, world.y0)?;
            let (x1, y0) = world_to_device(request, world.x1, world.y1)?;
            let ranks = GridRanks::lattice(px, py, world);
            let (ix, iy) = lattice_indices(px, py, &world);
            let first = [ix as i64, iy as i64];
            let term = |d: i128, u: f64| {
                let w = (d.max(0) + 2) as f64 / DEVICE_ONE as f64;
                (w < 1.0).then(|| LatticeTerm { u, first, p: if c > 1.0 { w / (c - (c - 1.0) * w) } else { w } })
            };
            rects.push([term(x1 - x0, ranks.u.0), term(y1 - y0, ranks.u.1)]);
        }
        for polygon in &geometry.polys {
            check_member_cancelled(guard, &mut heartbeat)?;
            work += 1 + polygon.pts.len() as u64;
            if work > PLACEMENT_PREP_WORK {
                return Ok(Err(Why::PrepWork));
            }
            let Some(base) = polygon_bbox(&polygon.pts) else {
                return Ok(Err(Why::NotSubPixel));
            };
            if cut_side_of(base, shape_cut_max) < shape_cut {
                continue;
            }
            if !rim {
                return Ok(Err(Why::NonRim));
            }
            if !matches!(polygon.rep, Rep::One) {
                return Ok(Err(Why::ArrayRecord));
            }
            if rects.len() + areas.len() == SMALL_CELL_SHAPES {
                return Ok(Err(Why::Shapes));
            }
            let Some(term) = area_term(request, px, py, member0.apply_bbox(base)?, polygon_area(&polygon.pts))? else {
                return Ok(Err(Why::NotSubPixel));
            };
            areas.push(term);
        }
    }
    if rects.is_empty() && areas.is_empty() {
        return Ok(Err(Why::NoShapes));
    }
    let (wa, wb) = world_vectors(*va, *vb, parent_world)?;
    Ok(SurvivorWalk::plan(&instance.rep, wa, wb, [px, py], range, [true, true], PLACEMENT_MEMBER_COST, |axis| {
        let mut terms = Vec::with_capacity(rects.len() + areas.len());
        for rect in &rects {
            match rect[axis] {
                Some(term) => terms.push(term),
                None => return Vec::new(),
            }
        }
        for &(along, term) in &areas {
            if along != axis {
                return Vec::new();
            }
            terms.push(term);
        }
        terms
    }))
}

/// A lattice's key for a shape whose world box is `world`: its phase along
/// each repeating axis and its size, and a salt of the pitches - the same for
/// every member.
fn lattice_key(px: i64, py: i64, world: BBox) -> (BBox, u64) {
    let key = BBox {
        x0: if px > 0 { world.x0.rem_euclid(px) } else { world.x0 },
        y0: if py > 0 { world.y0.rem_euclid(py) } else { world.y0 },
        x1: world.x1 - world.x0,
        y1: world.y1 - world.y0,
    };
    (key, (px as u64).rotate_left(21) ^ (py as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// The world indices of a member on the lattice (GridRanks::ranks).
fn lattice_indices(px: i64, py: i64, world: &BBox) -> (u64, u64) {
    (
        if px > 0 { world.x0.div_euclid(px) as u64 } else { 0 },
        if py > 0 { world.y0.div_euclid(py) as u64 } else { 0 },
    )
}

/// The area-true keep rank of a sub-pixel polygon or path on a world lattice
/// (the placement lattice): one rank, frac(u + vdc(k) + weyl(k')), k its
/// world index along x when the lattice repeats along x, else along y; the
/// keep chance stays its area's (area_true_hairline).
fn lattice_area_rank(px: i64, py: i64, world: &BBox) -> f64 {
    let (key, salt) = lattice_key(px, py, *world);
    let u = salted_rank(key, 5 ^ salt);
    let (ix, iy) = lattice_indices(px, py, world);
    let (k, across) = if px > 0 { (ix, iy) } else { (iy, ix) };
    (u + vdc53(k) + weyl53(across)).rem_euclid(1.0)
}

impl GridRanks {
    /// The lattice ranks of a shape whose world box is `world` (any member)
    /// on the world lattice (px, py): keyed by its phase and size.
    fn lattice(px: i64, py: i64, world: BBox) -> GridRanks {
        let (key, salt) = lattice_key(px, py, world);
        GridRanks {
            mode: GridMode::Lattice { px, py },
            u: (salted_rank(key, 3 ^ salt), salted_rank(key, 4 ^ salt)),
        }
    }

    fn new(rep: &Rep, world_transform: &OrthoTransform, base_world: BBox) -> Result<Option<GridRanks>, String> {
        let Rep::Grid { na, nb, va, vb } = rep else {
            return Ok(None);
        };
        let (wa, wb) = world_vectors(*va, *vb, world_transform)?;
        // no repeating vector: one member, ranked as the same shape stored alone
        if *na <= 1 && *nb <= 1 {
            return Ok(None);
        }
        if let Some((px, py)) = world_lattice(*na, *nb, wa, wb) {
            return Ok(Some(GridRanks::lattice(px, py, base_world)));
        }
        let (ua, ub) = ((wa.0.unsigned_abs(), wa.1.unsigned_abs()), (wb.0.unsigned_abs(), wb.1.unsigned_abs()));
        // which index is x's primary: a one-row array's own index goes to the
        // axis it runs along (a column of bars spreads its y decisions by vdc)
        let x_along_a = if *nb <= 1 {
            ua.0 >= ua.1
        } else if *na <= 1 {
            ub.0 < ub.1
        } else {
            ua.0 >= ub.0
        };
        Ok(Some(GridRanks {
            mode: GridMode::Index {
                va: *va,
                vb: *vb,
                na: *na,
                nb: *nb,
                det: va.0 as i128 * vb.1 as i128 - va.1 as i128 * vb.0 as i128,
                x_along_a,
            },
            u: (salted_rank(base_world, 1), salted_rank(base_world, 2)),
        }))
    }

    /// A skewed grid's member at this offset from the record's first: (i, j).
    fn index(&self, ox: i64, oy: i64) -> Option<(u64, u64)> {
        let GridMode::Index { va, vb, na, nb, det, .. } = self.mode else {
            return None;
        };
        let (ox, oy) = (ox as i128, oy as i128);
        let (va, vb) = ((va.0 as i128, va.1 as i128), (vb.0 as i128, vb.1 as i128));
        let along = |v: (i128, i128)| -> Option<u64> {
            let k = if v.0 != 0 { ox / v.0 } else if v.1 != 0 { oy / v.1 } else { 0 };
            u64::try_from(k).ok()
        };
        if det != 0 {
            let i = (ox * vb.1 - oy * vb.0) / det;
            let j = (va.0 * oy - va.1 * ox) / det;
            return Some((u64::try_from(i).ok()?, u64::try_from(j).ok()?));
        }
        if nb <= 1 {
            return Some((along(va)?, 0));
        }
        if na <= 1 {
            return Some((0, along(vb)?));
        }
        None
    }

    /// The member's (x, y) ranks: `world` is its world bbox, (ox, oy) its
    /// offset from the record's first member.
    fn ranks(&self, ox: i64, oy: i64, world: &BBox) -> Option<(f64, f64)> {
        let (ix, iy) = match self.mode {
            GridMode::Lattice { px, py } => (
                if px > 0 { world.x0.div_euclid(px) as u64 } else { 0 },
                if py > 0 { world.y0.div_euclid(py) as u64 } else { 0 },
            ),
            GridMode::Index { x_along_a, .. } => {
                let (i, j) = self.index(ox, oy)?;
                if x_along_a { (i, j) } else { (j, i) }
            }
        };
        let vdc = |k: u64| (k.reverse_bits() >> 11) as f64 / (1u64 << 53) as f64;
        let weyl = |k: u64| (k.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 11) as f64 / (1u64 << 53) as f64;
        let spread = |u: f64, p: u64, s: u64| (u + vdc(p) + weyl(s)).rem_euclid(1.0);
        Some((spread(self.u.0, ix, iy), spread(self.u.1, iy, ix)))
    }
}

/// A lattice member's rank term along the axis it is enumerated on: the
/// van der Corput value of its world index (GridRanks::ranks).
fn vdc53(k: u64) -> f64 {
    (k.reverse_bits() >> 11) as f64 / (1u64 << 53) as f64
}

/// The golden-ratio Weyl step of the index on the other axis (GridRanks::ranks).
fn weyl53(k: u64) -> f64 {
    (k.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 11) as f64 / (1u64 << 53) as f64
}

/// A lattice array's members are worth listing only when the list is at most
/// this share of the members (the rest of the time the scan is as cheap).
const SURVIVOR_LIST_SHARE: f64 = 0.5;

/// What a member of an array record costs the member walk (its transform,
/// ranks and width test), in the survivor walk's blocks.
const RECORD_MEMBER_COST: f64 = 4.0;

/// What a member of a placement array costs the member walk (a cell visit:
/// its transform, page scan and bin item), in the survivor walk's blocks.
const PLACEMENT_MEMBER_COST: f64 = 16.0;

/// The most progression cursors a survivor walk holds at once (review
/// 2026-09-25: a temporary memory bound, ~1.5 MB). A walk listed along the
/// outer repetition index merges every line's cursors in one heap; past this
/// many it is not started and the member walk runs instead. A walk along the
/// inner index holds one line's cursors, about 2 per bit of the resolution.
const SURVIVOR_CURSOR_CAP: usize = 1 << 16;

/// The outcome of a placement array's survivor walk (RenderStats::
/// place_walks, PLACE_WALK_OUTCOMES in this order): walked, or why every
/// member is visited. Of the plan's own reasons the furthest either axis got
/// is kept: NoAxis < AxisMismatch < Cost < CursorCap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PlaceWalkOutcome {
    Walked,
    NotLeaf,
    NoRange,
    PageLevel,
    Undecoded,
    NonRim,
    ArrayRecord,
    Path,
    Shapes,
    PrepWork,
    NotSubPixel,
    NoShapes,
    NoAxis,
    AxisMismatch,
    Cost,
    CursorCap,
}

/// One shape's rank along the listed axis A of a lattice array: a member
/// (i, j) at world indices k_A = first[A] + sign * (its index along A) and
/// k_B across ranks frac(u + vdc(k_A) + weyl(k_B)), and may be drawn only
/// when that is under `p` (an upper bound of its chance). A record's own array
/// has one term; the shapes of a cell placed by an array have one each.
#[derive(Clone, Copy, Debug)]
struct LatticeTerm {
    u: f64,
    first: [i64; 2],
    p: f64,
}

/// The work of one survivor walk, for tests: members handed to the draw and
/// the most cursors held at once.
#[derive(Clone, Copy, Debug, Default)]
struct SurvivorWork {
    walked: u64,
    peak_cursors: usize,
}

/// A planned walk over the members of a lattice array that CAN survive their
/// width-first draw (ADAPTIVE_CUT_DENSITY_PLAN §4.3 step 1 in the renderer,
/// 2026-09-25; made lazy after the review of the same day). Along a
/// sub-pixel world axis A of pitch P a member's rank is
/// frac(u + vdc(k_A) + weyl(k_B)) - k_A its world index along A, a signed
/// affine function of the repetition index i or j that runs along A, k_B the
/// one across - so for a fixed k_B the members under p have vdc(k_A) in one
/// interval mod 1: a union of dyadic intervals, each the arithmetic
/// progression k_A = rev_b(m) (mod 2^b). The interval is padded (2^-40
/// against the f64 sums) and cut at the index range's resolution (a finer
/// block holds one member at most), so the walk is a superset of the drawn
/// members and the caller re-tests each by the exact rule: the pixels are the
/// member walk's. It hands members over in the member walk's order (i, then
/// j), one line at a time, merging the line's progressions in a small heap -
/// nothing is prepared ahead, so a draw that stops (the tile is full, the
/// frame cancelled) stops the walk at once.
struct SurvivorWalk {
    va: (i64, i64),
    vb: (i64, i64),
    /// the listed world axis (0 = x, 1 = y), the repetition index along it
    /// (0 = i, 1 = j) and its sign
    axis: usize,
    index: usize,
    sign: i64,
    /// the repetition index across (0 = i, 1 = j) and its sign when it runs
    /// along the other world axis of a lattice with a pitch there
    across: Option<i64>,
    /// the walk's index rectangle (i0, i1, j0, j1)
    range: (i64, i64, i64, i64),
    /// the resolution the rank interval is cut at, in bits
    bits: u32,
    terms: Vec<LatticeTerm>,
}

impl SurvivorWalk {
    /// Plans the walk over `range` of the array (`rep`'s vectors in the local
    /// frame, `wa` / `wb` in the world) when listing pays: the listed axis is
    /// one whose repetition index is the inner one (j) if it qualifies, else
    /// the outer one, the cheaper when both indices do (`sub`: the world axes
    /// the terms can be listed on - each needs a bound there). An axis does
    /// not qualify when its list would exceed SURVIVOR_LIST_SHARE of the
    /// members or, along the outer index, hold more than SURVIVOR_CURSOR_CAP
    /// cursors; None when none does.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn plan(
        rep: &Rep,
        wa: (i64, i64),
        wb: (i64, i64),
        pitch: [i64; 2],
        range: (i64, i64, i64, i64),
        sub: [bool; 2],
        member_cost: f64,
        terms: impl Fn(usize) -> Vec<LatticeTerm>,
    ) -> Result<SurvivorWalk, PlaceWalkOutcome> {
        let Rep::Grid { na, nb, va, vb } = rep else {
            return Err(PlaceWalkOutcome::NoAxis);
        };
        let (i0, i1, j0, j1) = range;
        if i1 < i0 || j1 < j0 {
            return Err(PlaceWalkOutcome::NoRange);
        }
        // why no axis qualified: the furthest either got
        let mut why = PlaceWalkOutcome::NoAxis;
        // the repetition index (0 = i, 1 = j) that drives a world axis (0 = x,
        // 1 = y) and its sign: its vector runs along that axis
        let driver = |axis: usize| -> Option<(usize, i64)> {
            [(0usize, *na > 1, wa), (1, *nb > 1, wb)].into_iter().find_map(|(index, many, v)| {
                let (along, across) = if axis == 0 { (v.0, v.1) } else { (v.1, v.0) };
                (many && along != 0 && across == 0).then(|| (index, along.signum()))
            })
        };
        let count = (i1 - i0 + 1) as f64 * (j1 - j0 + 1) as f64;
        // 2^bits past the index range: a finer block holds one member at most
        let resolution = |values: i64| (64 - (values.max(1) as u64 - 1).leading_zeros()).min(52) + 1;
        let mut best: Option<(usize, (bool, f64), Vec<LatticeTerm>)> = None;
        for axis in [0usize, 1] {
            let Some((index, _)) = driver(axis) else {
                continue;
            };
            if !sub[axis] || pitch[axis] <= 0 {
                continue;
            }
            let terms = terms(axis);
            if terms.is_empty() {
                why = why.max(PlaceWalkOutcome::AxisMismatch);
                continue;
            }
            let (along, lines) = if index == 0 { (i1 - i0 + 1, j1 - j0 + 1) } else { (j1 - j0 + 1, i1 - i0 + 1) };
            let bits = resolution(along);
            let most = 2 * bits as usize + 4;
            // an outer-index walk holds every line's cursors
            if index == 0 && (lines as usize).saturating_mul(most).saturating_mul(terms.len()) > SURVIVOR_CURSOR_CAP {
                why = why.max(PlaceWalkOutcome::CursorCap);
                continue;
            }
            // a term's interval, p 2^bits blocks of the resolution (plus the
            // padding's), splits into about 2 log2 of that dyadic blocks
            let blocks: f64 = terms
                .iter()
                .map(|t| (2.0 * (t.p * (1u64 << bits) as f64 + 3.0).log2() + 2.0).min(most as f64))
                .sum();
            let chance: f64 = terms.iter().map(|t| t.p).sum::<f64>().min(1.0);
            // the walk: the blocks of every line and its members' visits;
            // the member walk: every member's visit, `member_cost` blocks each
            let cost = lines as f64 * blocks + chance * count * member_cost;
            if cost > SURVIVOR_LIST_SHARE * count * member_cost {
                why = why.max(PlaceWalkOutcome::Cost);
                continue;
            }
            // the inner index first: its walk prepares one line at a time, so
            // a draw that stops early has paid for one line; an outer-index
            // walk prepares every line before its first member
            let rank = (index == 0, cost);
            if best.as_ref().is_none_or(|(_, c, _)| rank < *c) {
                best = Some((axis, rank, terms));
            }
        }
        let Some((axis, _, terms)) = best else {
            return Err(why);
        };
        let (index, sign) = driver(axis).ok_or(PlaceWalkOutcome::NoAxis)?;
        let across = driver(1 - axis).filter(|&(other, _)| other != index && pitch[1 - axis] > 0).map(|(_, s)| s);
        let along = if index == 0 { i1 - i0 + 1 } else { j1 - j0 + 1 };
        Ok(SurvivorWalk { va: *va, vb: *vb, axis, index, sign, across, range, bits: resolution(along), terms })
    }

    /// The cursors (next index along A, stride) of line `o` (the index across)
    /// for every term, appended to `out`.
    fn line_cursors(&self, o: i64, out: &mut Vec<(i64, i64)>) {
        let (d0, d1) = if self.index == 0 { (self.range.0, self.range.1) } else { (self.range.2, self.range.3) };
        let scale = 1i64 << self.bits;
        let pad = 2f64.powi(-40);
        for term in &self.terms {
            // the world index across: from the index across when it drives the
            // other axis, 0 when that axis does not repeat (GridRanks::ranks)
            let k_across = match self.across {
                Some(s) => term.first[1 - self.axis] + s * o,
                None => 0,
            };
            // vdc(k_A) must lie in [lo, lo + p) mod 1: the blocks of 2^-bits
            // that meet the padded interval, as one or two runs on the circle
            let lo = (-(term.u + weyl53(k_across as u64))).rem_euclid(1.0);
            let from = ((lo - pad) * scale as f64).floor() as i64;
            let to = ((lo + term.p + pad) * scale as f64).ceil() as i64;
            let mut runs: [(i64, i64); 2] = [(0, 0); 2];
            let n = if to - from >= scale {
                runs[0] = (0, scale);
                1
            } else {
                let a = from.rem_euclid(scale);
                let b = a + (to - from);
                if b <= scale {
                    runs[0] = (a, b);
                    1
                } else {
                    runs[0] = (a, scale);
                    runs[1] = (0, b - scale);
                    2
                }
            };
            for &(lo_r, hi_r) in &runs[..n] {
                let (mut lo_r, hi_r) = (lo_r as u64, hi_r as u64);
                while lo_r < hi_r {
                    let mut shift = lo_r.trailing_zeros().min(self.bits);
                    while shift > 0 && lo_r + (1u64 << shift) > hi_r {
                        shift -= 1;
                    }
                    let bits = self.bits - shift;
                    let m = lo_r >> shift;
                    // the block fixes the low `bits` bits of k_A (reversed m)
                    let residue = if bits == 0 { 0 } else { m.reverse_bits() >> (64 - bits) };
                    let stride = 1i128 << bits;
                    // k_A = first + sign * d  ==  residue (mod 2^bits)
                    let want = (self.sign as i128 * (residue as i128 - term.first[self.axis] as i128)).rem_euclid(stride);
                    let d = d0 as i128 + (want - d0 as i128).rem_euclid(stride);
                    if d <= d1 as i128 {
                        out.push((d as i64, stride as i64));
                    }
                    lo_r += 1u64 << shift;
                }
            }
        }
    }

    /// Walks the members in the member walk's order, handing each to
    /// `member`; stops at the first error `member` returns (a full tile, a
    /// cancelled frame). The count is that of the members handed over.
    fn run(
        &self,
        guard: Option<RenderGuard<'_>>,
        work: &mut SurvivorWork,
        member: &mut dyn FnMut(i64, i64) -> Result<(), String>,
    ) -> Result<RepVisit, String> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let (i0, i1, j0, j1) = self.range;
        let (d1, o0, o1) = if self.index == 0 { (i1, j0, j1) } else { (j1, i0, i1) };
        let offset = |i: i64, j: i64| -> Result<(i64, i64), String> {
            let ox = i as i128 * self.va.0 as i128 + j as i128 * self.vb.0 as i128;
            let oy = i as i128 * self.va.1 as i128 + j as i128 * self.vb.1 as i128;
            Ok((checked_i64(ox, "grid offset x")?, checked_i64(oy, "grid offset y")?))
        };
        let mut heartbeat = 0u16;
        let mut cursors: Vec<(i64, i64)> = Vec::new();
        if self.index == 1 {
            // listed along j, the inner index: line by line (i ascending), the
            // line's progressions merged by j
            let mut heap: BinaryHeap<Reverse<(i64, i64)>> = BinaryHeap::new();
            for i in o0..=o1 {
                check_member_cancelled(guard, &mut heartbeat)?;
                cursors.clear();
                self.line_cursors(i, &mut cursors);
                work.peak_cursors = work.peak_cursors.max(cursors.len());
                heap.clear();
                heap.extend(cursors.iter().map(|&(j, stride)| Reverse((j, stride))));
                let mut last = i64::MIN;
                while let Some(Reverse((j, stride))) = heap.pop() {
                    if j + stride <= d1 {
                        heap.push(Reverse((j + stride, stride)));
                    }
                    // two shapes' progressions may meet on a member
                    if j == last {
                        continue;
                    }
                    last = j;
                    let (ox, oy) = offset(i, j)?;
                    work.walked += 1;
                    member(ox, oy)?;
                }
            }
        } else {
            // listed along i, the outer index: every line's progressions in one
            // heap, merged by (i, j)
            let mut heap: BinaryHeap<Reverse<(i64, i64, i64)>> = BinaryHeap::new();
            for j in o0..=o1 {
                check_member_cancelled(guard, &mut heartbeat)?;
                cursors.clear();
                self.line_cursors(j, &mut cursors);
                heap.extend(cursors.iter().map(|&(i, stride)| Reverse((i, j, stride))));
            }
            // SurvivorWalk::plan bounds the cursors of an outer-index walk
            debug_assert!(heap.len() <= SURVIVOR_CURSOR_CAP);
            work.peak_cursors = work.peak_cursors.max(heap.len());
            let mut last = (i64::MIN, i64::MIN);
            while let Some(Reverse((i, j, stride))) = heap.pop() {
                if i + stride <= d1 {
                    heap.push(Reverse((i + stride, j, stride)));
                }
                if (i, j) == last {
                    continue;
                }
                last = (i, j);
                let (ox, oy) = offset(i, j)?;
                work.walked += 1;
                member(ox, oy)?;
            }
        }
        Ok(RepVisit { tested: work.walked, visible: work.walked })
    }
}

/// The survivor walk of a width-first rectangle array: planned when the
/// request lists survivors, the array ranks on the world lattice
/// (GridRanks) and its members are under a pixel along an axis its
/// repetition runs on; None for the member walk. `base` is the record's first
/// member (local).
fn survivor_walk(
    request: &GeometryRasterRequest,
    grid: Option<&GridRanks>,
    rep: &Rep,
    base: BBox,
    local_view: BBox,
    world_transform: &OrthoTransform,
) -> Result<Option<SurvivorWalk>, String> {
    let Some(grid) = grid.filter(|_| request.survivor_list) else {
        return Ok(None);
    };
    let GridMode::Lattice { px, py } = grid.mode else {
        return Ok(None);
    };
    let Rep::Grid { va, vb, .. } = rep else {
        return Ok(None);
    };
    let Some(range) = visible_grid_range(rep, base, local_view) else {
        return Ok(None);
    };
    let world = world_transform.apply_bbox(base)?;
    let (x0, y1) = world_to_device(request, world.x0, world.y0)?;
    let (x1, y0) = world_to_device(request, world.x1, world.y1)?;
    // a member's device side differs from the first's by its rounding: two
    // units bound it; the chance of the extra pixel is P_c of that fraction
    let bound = |d: i128| {
        let w = (d.max(0) + 2) as f64 / DEVICE_ONE as f64;
        let c = request.width_c;
        (w < 1.0).then(|| if c > 1.0 { w / (c - (c - 1.0) * w) } else { w })
    };
    let chance = [bound(x1 - x0), bound(y1 - y0)];
    let origin = world_transform.apply(0, 0)?;
    let a = world_transform.apply(va.0, va.1)?;
    let b = world_transform.apply(vb.0, vb.1)?;
    let first = [
        if px > 0 { world.x0.div_euclid(px) } else { 0 },
        if py > 0 { world.y0.div_euclid(py) } else { 0 },
    ];
    Ok(SurvivorWalk::plan(
        rep,
        (a.0 - origin.0, a.1 - origin.1),
        (b.0 - origin.0, b.1 - origin.1),
        [px, py],
        range,
        [chance[0].is_some(), chance[1].is_some()],
        RECORD_MEMBER_COST,
        |axis| {
            chance[axis]
                .map(|p| LatticeTerm { u: if axis == 0 { grid.u.0 } else { grid.u.1 }, first, p })
                .into_iter()
                .collect()
        },
    )
    .ok())
}

/// Under the sub-cut arrays diagnostic (GeometryRasterRequest::sub_cut_arrays):
/// whether a record under the per-shape cut stays - a whole array (page level
/// 0, not thinned) ranked on its world lattice and drawn by the area-true rule,
/// whose members the width-first / area-true draw thins by their size and the
/// survivor walk lists; any other record under the cut goes.
fn keeps_sub_cut_array(
    request: &GeometryRasterRequest,
    paint: PaintStyle,
    rep: &Rep,
    level: u8,
    world_transform: &OrthoTransform,
) -> Result<bool, String> {
    Ok(request.sub_cut_arrays && level == 0 && area_true_rim(request, paint) && placement_lattice(rep, world_transform)?.is_some())
}

/// The world lattice a sub-pixel polygon or path keeps by under the placement
/// lattice (GeometryRasterRequest::place_lattice) - or the sub-cut arrays
/// diagnostic, for arrays: its own array's for a
/// whole lattice array (`whole`: not thinned by the page frontier), the
/// placement array's for a single shape, None otherwise (its world box's
/// rank) - and None when its paint is not the area-true rim.
fn area_keep_lattice(
    request: &GeometryRasterRequest,
    paint: PaintStyle,
    rep: &Rep,
    whole: bool,
    world_transform: &OrthoTransform,
    placed: Option<(i64, i64)>,
) -> Result<Option<(i64, i64)>, String> {
    if !(request.place_lattice || request.sub_cut_arrays) || !area_true_rim(request, paint) || !whole {
        return Ok(None);
    }
    match rep {
        Rep::One => Ok(placed),
        rep => placement_lattice(rep, world_transform),
    }
}

/// The survivor-walk term of a polygon or path of world box `world` and world
/// area `area` on the lattice (px, py): the axis its keep rank lists on (x
/// when the lattice repeats along x, lattice_area_rank) and a bound of its
/// keep chance over the members. None unless every member is under a pixel on
/// a side - area_true_hairline keeps only those by rank; a wider one is drawn
/// whole. The members' device sides differ by their rounding (two units).
fn area_term(request: &GeometryRasterRequest, px: i64, py: i64, world: BBox, area: f64) -> Result<Option<(usize, LatticeTerm)>, String> {
    let (x0, y1) = world_to_device(request, world.x0, world.y0)?;
    let (x1, y0) = world_to_device(request, world.x1, world.y1)?;
    let hi = |d: i128| (d.max(0) + 2) as f64 / DEVICE_ONE as f64;
    let lo = |d: i128| (d - 2).max(0) as f64 / DEVICE_ONE as f64;
    let (dw, dh) = (x1 - x0, y1 - y0);
    if hi(dw) >= 1.0 && hi(dh) >= 1.0 {
        return Ok(None);
    }
    let view = request.view;
    let px_area = area * (request.width as f64 / (view.x1 - view.x0)) * (request.height as f64 / (view.y1 - view.y0));
    let keep = (px_area / (lo(dw).max(1.0) * lo(dh).max(1.0))).clamp(0.0, 1.0);
    let (key, salt) = lattice_key(px, py, world);
    let (ix, iy) = lattice_indices(px, py, &world);
    Ok(Some((
        if px > 0 { 0 } else { 1 },
        LatticeTerm { u: salted_rank(key, 5 ^ salt), first: [ix as i64, iy as i64], p: keep },
    )))
}

/// The survivor walk of a sub-pixel polygon array on its own lattice (px, py)
/// under the placement lattice: the members whose keep rank can be under
/// their keep chance (area_term). None when the request does not list
/// survivors, a member may be a pixel or more on both sides, or listing does
/// not pay.
#[allow(clippy::too_many_arguments)]
fn area_survivor_walk(
    request: &GeometryRasterRequest,
    rep: &Rep,
    base: BBox,
    area: f64,
    local_view: BBox,
    world_transform: &OrthoTransform,
    px: i64,
    py: i64,
) -> Result<Option<SurvivorWalk>, String> {
    let Rep::Grid { va, vb, .. } = rep else {
        return Ok(None);
    };
    if !request.survivor_list {
        return Ok(None);
    }
    let Some(range) = visible_grid_range(rep, base, local_view) else {
        return Ok(None);
    };
    let Some((axis, term)) = area_term(request, px, py, world_transform.apply_bbox(base)?, area)? else {
        return Ok(None);
    };
    let (wa, wb) = world_vectors(*va, *vb, world_transform)?;
    Ok(SurvivorWalk::plan(rep, wa, wb, [px, py], range, [axis == 0, axis == 1], RECORD_MEMBER_COST, |listed| {
        if listed == axis { vec![term] } else { Vec::new() }
    })
    .ok())
}

/// One axis of a width-first rectangle: the side [v0, v1) (device units) is
/// w px; it draws m = ceil(w - t) pixels, none when m < 1, starting at
/// floor(centre - m/2 + 1/2). m is floor(w) or floor(w) + 1 (floor(w) + 1
/// exactly when the fraction exceeds t), non-decreasing in w.
fn width_first_span(v0: i128, v1: i128, t: f64) -> Option<(i128, i128)> {
    width_first_span_c(v0, v1, t, 1.0)
}

/// `width_first_span` with the extra pixel taken when t < P_c(frac(w)),
/// P_c(f) = f / (c - (c - 1) f) (GeometryRasterRequest::width_c): c = 1 is
/// the plain rule (P = f, so m = ceil(w - t)); a larger c thins the extra
/// pixels towards the whole width, never past floor(w), and P_c runs from
/// 0 to 1 over a whole pixel's fraction, so the mean width stays continuous
/// at whole widths (f / c alone would drop from 1.99 to 1.495 px at c = 2).
fn width_first_span_c(v0: i128, v1: i128, t: f64, c: f64) -> Option<(i128, i128)> {
    let w = (v1 - v0).max(0) as f64 / DEVICE_ONE as f64;
    let whole = w.floor();
    let f = w - whole;
    let p = if c > 1.0 { f / (c - (c - 1.0) * f) } else { f };
    let m = whole as i128 + i128::from(t < p);
    if m < 1 {
        return None;
    }
    let start = floor_div(v0 + v1 - m * DEVICE_ONE + DEVICE_ONE, 2 * DEVICE_ONE);
    Some((start, start + m))
}

/// Area-true polygon (and path outline), at least a pixel on both sides: the
/// pixels whose centres it covers take the layer's fill, and the pixels of
/// that set with a side on its border - the span ends, and what the row above
/// or below does not cover - are the outline. The rows next to the band are
/// scanned too, so the rim is the same whatever the tiling.
fn paint_area_true_polygon(
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
    // only the polygon's own rows (one more on each side) inside the band
    // and its two neighbour rows: a small polygon must not pay for the tile
    let (low, high) = device.iter().fold((i128::MAX, i128::MIN), |(a, b), &(_, y)| (a.min(y), b.max(y)));
    let lo = (band.row0 as i64 - 1).max(checked_i64(floor_div(low, DEVICE_ONE), "polygon low row")? - 1);
    let hi = (band.row1 as i64 + 1).min(checked_i64(floor_div(high, DEVICE_ONE), "polygon high row")? + 2);
    if lo >= hi {
        return Ok(false);
    }
    let mut rows: Vec<Vec<(i128, i128)>> = vec![Vec::new(); (hi - lo) as usize];
    scan_device_polygon(&device, FillPhase::PixelCenter, lo, hi, |row, first, end| {
        if first < end {
            rows[(row - lo) as usize].push((first, end));
        }
        Ok(())
    })?;
    let empty: Vec<(i128, i128)> = Vec::new();
    let at = |row: i64| -> &Vec<(i128, i128)> {
        if row < lo || row >= hi { &empty } else { &rows[(row - lo) as usize] }
    };
    let (row0, row1) = ((band.row0 as i64).max(lo), (band.row1 as i64).min(hi));
    let mut drew = false;
    for row in row0..row1 {
        for &(first, end) in at(row) {
            let first = first.max(band.col0 as i128);
            let end = end.min(band.col1 as i128);
            if first < end {
                let first = checked_usize(first, "polygon first column")?;
                let end = checked_usize(end, "polygon end column")?;
                drew |= fill_span(band, paint, request.height, row as usize, first, end);
            }
        }
    }
    let mut rim = Vec::new();
    for row in row0..row1 {
        rim.clear();
        for &(first, end) in at(row) {
            rim.push((first, first + 1));
            rim.push((end - 1, end));
            for neighbour in [at(row - 1), at(row + 1)] {
                let mut from = first;
                for &(a, b) in neighbour.iter() {
                    if b <= from || a >= end {
                        continue;
                    }
                    if a > from {
                        rim.push((from, a));
                    }
                    from = from.max(b);
                }
                if from < end {
                    rim.push((from, end));
                }
            }
        }
        for &(first, end) in &rim {
            drew |= paint_hairline_device_rect(band, request, (first, row as i128, end, row as i128 + 1), paint)?;
        }
    }
    Ok(drew)
}

/// Batch the already-selected hairlines, not the source records. The scratch
/// buffer is reused per tile, capped at 64K row spans, and never scans empty
/// layer pixels. All paints in this layer are opaque writes of the same colour.
fn queue_representative(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    prim: &floe_vfs::representatives::Prim,
    paint: PaintStyle,
    spans: &mut Vec<RepSpan>,
    stats: &mut RenderStats,
) -> Result<bool, String> {
    if prim.kind != floe_vfs::representatives::PRIM_SEGMENT {
        if let Some((x0, y0, x1, y1)) = hairline_world_bbox(request, prim.bbox(), paint)? {
            let c0 = x0.max(band.col0 as i128);
            let c1 = x1.min(band.col1 as i128);
            let r0 = y0.max(band.row0 as i128);
            let r1 = y1.min(band.row1 as i128);
            if c0 >= c1 || r0 >= r1 {
                return Ok(false);
            }
            if spans.len() + (r1 - r0) as usize > 65536 {
                flush_representative_spans(band, request, paint, spans, stats);
            }
            for r in r0..r1 {
                spans.push((r as u32, c0 as u32, c1 as u32));
            }
            return Ok(true);
        }
    }
    paint_representative(band, request, prim, paint)
}
fn flush_representative_spans(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    paint: PaintStyle,
    spans: &mut Vec<RepSpan>,
    stats: &mut RenderStats,
) {
    if spans.is_empty() {
        return;
    }
    spans.sort_unstable();
    let solid = PaintStyle {
        fill: LayerFill::Solid,
        ..paint
    };
    let mut run = spans[0];
    let mut paint_run = |run: RepSpan| {
        fill_span(
            band,
            solid,
            request.height,
            run.0 as usize,
            run.1 as usize,
            run.2 as usize,
        );
        stats.representative_spans += 1;
        stats.representative_pixels += (run.2 - run.1) as u64;
    };
    for &next in &spans[1..] {
        if next.0 == run.0 && next.1 <= run.2 {
            run.2 = run.2.max(next.2);
        } else {
            paint_run(run);
            run = next;
        }
    }
    paint_run(run);
    spans.clear();
}

/// One OVR2 representative: a rect takes the real rect's path (so a
/// sub-pixel width is the one-pixel hairline and the long axis keeps its
/// projected length - 4, 3, 2, 1 px as the view widens), a segment is
/// stroked as the boundary edge it is, a fallback point is a one-pixel rect.
fn paint_representative(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    prim: &floe_vfs::representatives::Prim,
    paint: PaintStyle,
) -> Result<bool, String> {
    use floe_vfs::representatives::PRIM_SEGMENT;
    if prim.kind == PRIM_SEGMENT {
        return stroke_world_polyline(band, request, &[(prim.x0, prim.y0), (prim.x1, prim.y1)], paint);
    }
    let paint = if prim.flags & floe_vfs::representatives::PRIM_MERGED_SOLID != 0 {
        PaintStyle { fill: LayerFill::Solid, ..paint }
    } else { paint };
    paint_world_rect(band, request, prim.bbox(), paint)
}

fn paint_world_polygon(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    points: &[(i64, i64)],
    paint: PaintStyle,
) -> Result<bool, String> {
    paint_world_polygon_ranked(band, request, points, paint, None)
}

/// `paint_world_polygon` with a sub-pixel polygon kept by `rank` when given
/// (hairline_world_bbox_ranked).
fn paint_world_polygon_ranked(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    points: &[(i64, i64)],
    paint: PaintStyle,
    rank: Option<f64>,
) -> Result<bool, String> {
    if let Some(world) = polygon_bbox(points) {
        let area = if request.area_true { Some(polygon_area(points)) } else { None };
        if let Some(rect) = hairline_world_bbox_ranked(request, world, paint, area, rank)? {
            return paint_hairline_device_rect(band, request, rect, paint);
        }
    }
    if area_true_rim(request, paint) {
        return paint_area_true_polygon(band, request, points, paint);
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
    paint_world_path_ranked(band, request, outline, centerline, paint, None)
}

/// `paint_world_path` with a sub-pixel path kept by `rank` when given
/// (hairline_world_bbox_ranked).
fn paint_world_path_ranked(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    outline: &[(i64, i64)],
    centerline: &[(i64, i64)],
    paint: PaintStyle,
    rank: Option<f64>,
) -> Result<bool, String> {
    if let Some(world) = polygon_bbox(outline) {
        let area = if request.area_true { Some(polygon_area(outline)) } else { None };
        if let Some(rect) = hairline_world_bbox_ranked(request, world, paint, area, rank)? {
            // The centerline lies inside the collapsed outline bbox, so
            // its stroke pass is covered by the collapsed spans.
            return paint_hairline_device_rect(band, request, rect, paint);
        }
    }
    // area-true: the outline's covered pixels and their rim; the spine
    // stroke would reach a pixel past a flush end
    if area_true_rim(request, paint) {
        return paint_area_true_polygon(band, request, outline, paint);
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
/// Per-pixel interior fill rule (the span forms in `fill_span` match
/// it pixel for pixel); the summary mask paints pixel by pixel.
fn fill_pixel_on(fill: LayerFill, row: u32, col: u32, height: u32) -> bool {
    match fill {
        LayerFill::Solid => true,
        LayerFill::Clear => false,
        LayerFill::Speckle => (row + col) & 1 == 0,
        LayerFill::Pattern(rows) => {
            let source_row = row.wrapping_add(height - 1) & 15;
            let word = rows[source_row as usize];
            word & (1 << (15 - (col & 15))) != 0
        }
    }
}

/// Occupancy summary of one layer into one tile (docs/OCCUPANCY_PLAN
/// .ko.md §3, §6 step 4): the level's occupied cells that meet the
/// tile (plus a one-pixel halo) are projected to a device mask - an
/// occupied cell lights the ONE pixel holding its centre (review
/// 2026-09-11 (2nd) P1-3: lighting every pixel a cell touched, on top
/// of the exact raster's centre sampling, reached two pixels past an
/// exact edge; with cells <= 1 px the centre rule keeps the summary
/// within one pixel of exact in every direction and never drops a
/// cell) - and the mask is styled: a lit pixel with an unlit
/// 4-neighbour is boundary and takes the colour solid (one pixel,
/// whatever the stroke width), an interior pixel takes the layer's
/// fill rule (solid / speckle / pattern / clear). The halo makes the
/// boundary decision independent of the tile grid, so pixels stay
/// tile-size invariant.
fn paint_summary_plane(
    band: &mut RasterBand,
    request: &GeometryRasterRequest,
    summary: &crate::summary::SummaryPlane,
    paint: PaintStyle,
    counters: &mut RasterCounters,
) -> Result<(), String> {
    let view = request.view;
    let width = request.width as f64;
    let height = request.height as f64;
    let span_x = view.x1 - view.x0;
    let span_y = view.y1 - view.y0;
    // the halo window in device pixels (columns/rows may be -1 or the
    // frame size; a pixel outside the frame still gets its mask value
    // from the cells, so the boundary test at the frame edge is real)
    let hc0 = band.col0 as i64 - 1;
    let hc1 = band.col1 as i64 + 1;
    let hr0 = band.row0 as i64 - 1;
    let hr1 = band.row1 as i64 + 1;
    let hw = (hc1 - hc0) as usize;
    let hh = (hr1 - hr0) as usize;
    // the lit pixels of the halo window are collected first: a layer
    // with none in this tile costs the cell scan only, never the mask
    // allocation and the pixel loop (field 2026-09-18: MAIN09 at 200 %
    // with 337 visible layers spent 804 ms drawing, most of it in
    // full-tile mask zeroing and scanning for layers with nothing here)
    let mut lit: Vec<(u32, u32)> = Vec::new();
    // world bounds of the halo window
    let wx0 = view.x0 + hc0 as f64 * span_x / width;
    let wx1 = view.x0 + hc1 as f64 * span_x / width;
    let wy0 = view.y1 - hr1 as f64 * span_y / height;
    let wy1 = view.y1 - hr0 as f64 * span_y / height;
    let cell = summary.cell_dbu as f64;
    let (ox, oy) = (summary.x0 as f64, summary.y0 as f64);
    if summary.w == 0 || summary.h == 0 || !(cell > 0.0) {
        return Ok(());
    }
    let i0 = ((wx0 - ox) / cell).floor().max(0.0) as i64;
    let i1 = (((wx1 - ox) / cell).ceil() as i64 - 1).min(summary.w as i64 - 1);
    let j0 = ((wy0 - oy) / cell).floor().max(0.0) as i64;
    let j1 = (((wy1 - oy) / cell).ceil() as i64 - 1).min(summary.h as i64 - 1);
    if i1 < i0 || j1 < j0 {
        return Ok(());
    }
    let bits = summary.bits();
    let row_bytes = summary.row_bytes();
    let mut cells = 0u64;
    for j in j0..=j1 {
        let row = &bits[j as usize * row_bytes..(j as usize + 1) * row_bytes];
        let mut i = i0;
        while i <= i1 {
            let byte = row[(i / 8) as usize];
            if byte == 0 {
                // eight empty cells at once
                i = (i / 8 + 1) * 8;
                continue;
            }
            if (byte >> (i % 8)) & 1 == 1 {
                cells += 1;
                // the pixel holding the cell's centre
                let mx = ox + (i as f64 + 0.5) * cell;
                let my = oy + (j as f64 + 0.5) * cell;
                let pc = ((mx - view.x0) * width / span_x).floor() as i64;
                let pr = ((view.y1 - my) * height / span_y).floor() as i64;
                if pc >= hc0 && pc < hc1 && pr >= hr0 && pr < hr1 {
                    lit.push(((pr - hr0) as u32, (pc - hc0) as u32));
                }
            }
            i += 1;
        }
    }
    counters.summary_cells_drawn = counters.summary_cells_drawn.saturating_add(cells);
    if lit.is_empty() {
        return Ok(());
    }
    let mut mask = vec![false; hw * hh];
    let mut row_lit = vec![false; hh];
    for &(mr, mc) in &lit {
        mask[mr as usize * hw + mc as usize] = true;
        row_lit[mr as usize] = true;
    }
    let tile_width = band.tile_width() as usize;
    let mut painted = 0u64;
    for r in band.row0..band.row1 {
        let mr = (r as i64 - hr0) as usize;
        // only lit pixels are painted, so a row without one is skipped
        // (its neighbours are read from the full mask when they matter)
        if !row_lit[mr] {
            continue;
        }
        for c in band.col0..band.col1 {
            let mc = (c as i64 - hc0) as usize;
            if !mask[mr * hw + mc] {
                continue;
            }
            let boundary = !mask[mr * hw + mc - 1]
                || !mask[mr * hw + mc + 1]
                || !mask[(mr - 1) * hw + mc]
                || !mask[(mr + 1) * hw + mc];
            if boundary || fill_pixel_on(paint.fill, r, c, request.height) {
                painted += 1;
                if band.once.is_some() {
                    band.write_once_pixel(r as usize, c as usize, paint.color);
                    continue;
                }
                let offset = ((r - band.row0) as usize * tile_width + (c - band.col0) as usize) * 4;
                band.pixels[offset..offset + 4].copy_from_slice(&paint.color);
            }
        }
    }
    counters.summary_pixels_drawn = counters.summary_pixels_drawn.saturating_add(painted);
    Ok(())
}

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
    if band.once.is_some() {
        let frame_height = request.height;
        return Ok(match paint.fill {
            LayerFill::Clear => false,
            LayerFill::Solid => band.write_once_rows(first_row, end_row, first_col, end_col, paint.color, |_| SpanRule::All),
            LayerFill::Speckle => band.write_once_rows(first_row, end_row, first_col, end_col, paint.color, |row| SpanRule::Speckle { row }),
            LayerFill::Pattern(rows) => band.write_once_rows(first_row, end_row, first_col, end_col, paint.color, |row| SpanRule::Pattern {
                word: rows[((row as u32).wrapping_add(frame_height - 1) & 15) as usize],
            }),
        });
    }
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
    if band.once.is_some() {
        let rule = match paint.fill {
            LayerFill::Clear => return false,
            LayerFill::Solid => SpanRule::All,
            LayerFill::Speckle => SpanRule::Speckle { row },
            LayerFill::Pattern(rows) => SpanRule::Pattern {
                word: rows[((row as u32).wrapping_add(frame_height - 1) & 15) as usize],
            },
        };
        return band.write_once_span(row, first_col, end_col, paint.color, rule);
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
        if band.once.is_some() {
            band.write_once_rows(row_lo as usize, row_hi as usize + 1, col_lo as usize, col_hi as usize + 1, paint.color, |_| SpanRule::All);
            return Ok(true);
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
                    if band.once.is_some() {
                        band.write_once_pixel(stroke_y as usize, stroke_x as usize, paint.color);
                        drew = true;
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
    let (row0, row1) = (band.row0 as i64, band.row1 as i64);
    let (col0, col1) = (band.col0 as i128, band.col1 as i128);
    let mut drew = false;
    scan_device_polygon(device, phase, row0, row1, |row, first_col, end_col| {
        let first_col = first_col.max(col0);
        let end_col = end_col.min(col1);
        if first_col >= end_col {
            return Ok(());
        }
        let first_col = checked_usize(first_col, "polygon first column")?;
        let end_col = checked_usize(end_col, "polygon end column")?;
        if fill_span(band, paint, request.height, row as usize, first_col, end_col) {
            drew = true;
        }
        Ok(())
    })?;
    Ok(drew)
}

/// The scanline of `fill_device_polygon_with_phase`: every row in
/// [row_lo, row_hi) gets its spans, unclamped, in column order - rows above
/// or below the image too (the area-true rim reads the row over the image's
/// top edge: an interior row there is not a border; review 2026-09-22).
fn scan_device_polygon(
    device: &[(i128, i128)],
    phase: FillPhase,
    row_lo: i64,
    row_hi: i64,
    mut emit: impl FnMut(i64, i128, i128) -> Result<(), String>,
) -> Result<(), String> {
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
        return Ok(());
    }
    edges.sort_unstable_by_key(|edge| (edge.first_row, edge.end_row, edge.x0));
    let first_row = edges
        .iter()
        .map(|edge| edge.first_row)
        .min()
        .unwrap_or(0);
    let end_row = edges
        .iter()
        .map(|edge| edge.end_row)
        .max()
        .unwrap_or(0)
        .min(row_hi);
    let first_row = first_row.max(row_lo);
    if first_row >= end_row {
        return Ok(());
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
            emit(row, first_col, end_col)?;
        }
    }
    Ok(())
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


    #[test]
    fn representative_spans_match_direct_pixels_with_overlap_styles_and_tiles() {
        use floe_vfs::representatives::{Prim, PRIM_RECT, PRIM_SEGMENT};
        let req = GeometryRasterRequest {
            view: RasterViewBox::new(-15., -25., 985., 975.).unwrap(),
            width: 100,
            height: 100,
            ..request()
        };
        let base = Prim {
            x0: 15,
            y0: 10,
            x1: 17,
            y1: 900,
            gate_dim: 4,
            thickness: 0,
            kind: PRIM_RECT,
            flags: 0,
            rank: 0,
        };
        let mut prims = vec![base; 1000];
        prims.extend([
            Prim {
                x0: 5,
                y0: 25,
                x1: 900,
                y1: 27,
                ..base
            },
            Prim {
                x0: 300,
                y0: 300,
                x1: 310,
                y1: 310,
                ..base
            },
            Prim {
                x0: 0,
                y0: 900,
                x1: 900,
                y1: 0,
                kind: PRIM_SEGMENT,
                ..base
            },
        ]);
        for fill in [
            LayerFill::Solid,
            LayerFill::Clear,
            LayerFill::Speckle,
            LayerFill::Pattern([0x5555; 16]),
        ] {
            for width in [1, 4] {
                let paint = PaintStyle {
                    fill,
                    stroke_width: width,
                    ..paint(&req)
                };
                let mut reference = full_band(&req);
                for p in &prims {
                    paint_representative(&mut reference, &req, p, paint).unwrap();
                }
                for tile in [10, 100] {
                    let mut assembled = vec![0u8; reference.pixels.len()];
                    let mut stats = RenderStats::default();
                    let mut scratch = Vec::new();
                    for y in (0..100).step_by(tile) {
                        for x in (0..100).step_by(tile) {
                            let mut band = RasterBand::new_tile(
                                &req,
                                x as u32,
                                (x + tile) as u32,
                                y as u32,
                                (y + tile) as u32,
                            )
                            .unwrap();
                            for p in &prims {
                                queue_representative(
                                    &mut band,
                                    &req,
                                    p,
                                    paint,
                                    &mut scratch,
                                    &mut stats,
                                )
                                .unwrap();
                            }
                            flush_representative_spans(
                                &mut band,
                                &req,
                                paint,
                                &mut scratch,
                                &mut stats,
                            );
                            for r in 0..tile {
                                assembled[((y + r) * 100 + x) * 4..((y + r) * 100 + x + tile) * 4]
                                    .copy_from_slice(&band.pixels[r * tile * 4..(r + 1) * tile * 4]);
                            }
                        }
                    }
                    assert_eq!(
                        assembled, reference.pixels,
                        "fill={fill:?} width={width} tile={tile}"
                    );
                    if width == 1 {
                        assert!(
                            stats.representative_pixels < 10000,
                            "overlapping pixels are unioned"
                        );
                    }
                }
            }
        }
    }

    fn request() -> GeometryRasterRequest {
        GeometryRasterRequest {
            view: RasterViewBox::new(0.0, 0.0, 10.0, 10.0).unwrap(),
            width: 10,
            height: 10,
            background: [0, 0, 0, 255],
            foreground: [255, 255, 255, 255],
            workers: 1,
            tile_size: DEFAULT_TILE_SIZE,
            area_true: false,
            width_c: 1.0,
            survivor_list: true,
            place_lattice: false,
            sub_cut_arrays: false,
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

    fn with_write_once<T>(on: bool, run: impl FnOnce() -> T) -> T {
        WRITE_ONCE_OVERRIDE.with(|value| value.set(Some(on)));
        let out = run();
        WRITE_ONCE_OVERRIDE.with(|value| value.set(None));
        out
    }

    /// Deterministic pseudo-random stream for the write-once scenes.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, bound: i64) -> i64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) % bound as u64) as i64
        }
    }

    fn once_page(page_id: u32, layer_idx: u32, span: i64, rng: &mut Lcg, dense: bool) -> Arc<DecodedPage> {
        let mut rects = Vec::new();
        let mut polys = Vec::new();
        let mut paths = Vec::new();
        for _ in 0..if dense { 40 } else { 8 } {
            let (w, h) = (1 + rng.next(if dense { 60 } else { 25 }), 1 + rng.next(if dense { 60 } else { 25 }));
            let rep = match rng.next(3) {
                0 => Rep::One,
                1 => Rep::Grid { na: 1 + rng.next(6) as u64, nb: 1 + rng.next(6) as u64, va: (3 + rng.next(30), 0), vb: (0, 3 + rng.next(30)) },
                _ => Rep::Pts(Arc::from(vec![(0, 0), (rng.next(40), rng.next(40)), (rng.next(40), 5 + rng.next(40))])),
            };
            rects.push(RectRec { layer: layer_idx, dt: 0, x: rng.next(span), y: rng.next(span), w, h, rep });
        }
        for _ in 0..4 {
            let (x, y, a) = (rng.next(span), rng.next(span), 4 + rng.next(40));
            polys.push(PolyRec {
                layer: layer_idx,
                dt: 0,
                pts: vec![(x, y), (x + a, y), (x + a, y + a / 2), (x + a / 2, y + a / 2), (x + a / 2, y + a), (x, y + a)],
                rep: Rep::One,
            });
            let (px, py, len) = (rng.next(span), rng.next(span), 10 + rng.next(80));
            paths.push(PathRec {
                layer: layer_idx,
                dt: 0,
                pts: vec![(px, py), (px + len, py), (px + len, py + len / 2)],
                hw: 1 + rng.next(4),
                es: 0,
                ee: 0,
                rep: Rep::One,
            });
        }
        let doc = Doc {
            unit: 1.0,
            cells: vec![Cell { name: format!("ONCE{page_id}"), rects, polys, paths, ..Cell::default() }],
            top: 0,
            layer_order: vec![(layer_idx, 0)],
            norm_s: 0.0,
            layer_names: HashMap::new(),
            layer_aliases: HashMap::new(),
        };
        // generous: every member, outline and path join lies inside
        let bbox = BBox { x0: -64, y0: -64, x1: span + 256, y1: span + 256 };
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

    /// Four layers over a top cell and an arrayed child, washes and every
    /// hierarchy-frame band; `dense` covers the view so tiles fill up.
    fn once_scene(seed: u64, dense: bool) -> FrameScene {
        let mut rng = Lcg(seed);
        let top = (0, REM_FULL);
        let child = (1, REM_FULL);
        let span = 320;
        let mut pages = Vec::new();
        for layer in 0..4u32 {
            pages.push(once_page(layer, layer, span, &mut rng, dense));
        }
        for layer in 0..4u32 {
            pages.push(once_page(4 + layer, layer, 40, &mut rng, false));
        }
        let child_box = BBox { x0: -64, y0: -64, x1: 40 + 256, y1: 40 + 256 };
        let top_box = BBox { x0: -400, y0: -400, x1: span + 700, y1: span + 700 };
        let frame = |rng: &mut Lcg, band: u8| {
            let (x, y) = (rng.next(span), rng.next(span));
            (BBox { x0: x, y0: y, x1: x + 2 + rng.next(90), y1: y + 2 + rng.next(90) }, Rep::One, band)
        };
        let plan = HierPlan {
            top,
            wcells: vec![
                WsCell {
                    key: top,
                    pages: vec![0, 1, 2, 3],
                    page_levels: Vec::new(),
                    insts: vec![
                        WsInst { child, x: 10, y: 20, rot: 0, flip: false, rep: Rep::Grid { na: 5, nb: 4, va: (61, 0), vb: (0, 67) } },
                        WsInst { child, x: 300, y: 40, rot: 1, flip: true, rep: Rep::One },
                    ],
                    frames: (0..8).map(|index| frame(&mut rng, index % 4)).collect(),
                    washes: (0..6)
                        .map(|index| {
                            let (x, y) = (rng.next(span), rng.next(span));
                            (index % 4, BBox { x0: x, y0: y, x1: x + 1 + rng.next(50), y1: y + 1 + rng.next(50) })
                        })
                        .collect(),
                    reps: Vec::new(),
                },
                WsCell {
                    key: child,
                    pages: vec![4, 5, 6, 7],
                    page_levels: Vec::new(),
                    insts: Vec::new(),
                    frames: vec![(BBox { x0: 0, y0: 0, x1: 40, y1: 40 }, Rep::One, 1)],
                    washes: vec![(2, BBox { x0: 5, y0: 5, x1: 9, y1: 30 })],
                    reps: Vec::new(),
                },
            ],
            pages: (0..8).collect(),
            page_prio: vec![0; 8],
            stats: HierStats::default(),
            explain: Vec::new(),
        };
        FrameScene::from_test_parts(plan, pages, BTreeMap::from([(top, top_box), (child, child_box)])).unwrap()
    }

    /// The same frame through `LayerRasterSession`: one pass at a time over
    /// tiles that stay alive, as the layer-decode probe paints it.
    fn session_frame(
        scene: &FrameScene,
        request: &StyledGeometryRasterRequest,
        work_bin: bool,
        block: usize,
    ) -> GeometryRasterReport {
        let session = LayerRasterSession::begin(scene, request, work_bin, None).unwrap();
        let expected = session.passes();
        assert!(expected > 0, "a styled frame has at least one pass");
        let (mut blocks, mut planes) = (0, 0);
        let report = session
            .render_layered(scene, request, None, block, |block, _| {
                blocks += 1;
                planes += block.len();
                Ok(())
            })
            .unwrap();
        assert_eq!(blocks, expected.div_ceil(block.max(1)), "one call per block");
        assert_eq!(planes, request.layers.len(), "every plane is announced once");
        report
    }

    #[test]
    fn write_once_frames_match_the_ordered_overwrite_byte_for_byte() {
        let mut stipple = [0u16; 16];
        for (row, word) in stipple.iter_mut().enumerate() {
            *word = 0x8421u16.rotate_left(row as u32);
        }
        let fills = [LayerFill::Solid, LayerFill::Speckle, LayerFill::Pattern(stipple), LayerFill::Clear];
        let colors = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 0, 255]];
        let mut full_tiles = 0u32;
        for (seed, dense) in [(1u64, false), (2, true), (3, true)] {
            for shift in 0..4usize {
                for (width, height, tile_size, view) in [
                    (96u32, 80u32, 16u16, (0.0, 0.0, 330.0, 275.0)),
                    (67, 53, 64, (-20.0, 10.0, 181.0, 169.0)),
                    (48, 48, 8, (100.0, 100.0, 148.0, 148.0)),
                ] {
                    let request = StyledGeometryRasterRequest {
                        raster: GeometryRasterRequest {
                            view: RasterViewBox::new(view.0, view.1, view.2, view.3).unwrap(),
                            width,
                            height,
                            workers: 3,
                            tile_size,
                            ..request()
                        },
                        layers: (0..4usize)
                            .map(|layer| LayerStyle {
                                layer_idx: layer as u32,
                                color: colors[layer],
                                fill: fills[(layer + shift) % 4],
                                outline_width: 1 + ((layer + shift) % 3) as u8,
                            })
                            .collect(),
                        hierarchy_frames: shift % 2 == 0,
                        mono: false,
                    };
                    let scene = once_scene(seed, dense);
                    let ordered = with_write_once(false, || render_geometry_styled(&scene, &request).unwrap());
                    // the layer-decode probe's retained tiles paint the same
                    // passes one at a time (LAYER_DECODE_PROBE_PLAN §5)
                    for work_bin in [true, false] {
                        for once in [true, false] {
                            // every block size paints the same frame: 1 = a stop
                            // at every layer, 3 = a block, 99 = the whole frame
                            for block in [1usize, 3, 99] {
                                let session = with_write_once(once, || session_frame(&scene, &request, work_bin, block));
                                assert!(
                                    session.frame.pixels() == ordered.frame.pixels(),
                                    "session (work_bin {work_bin}, write-once {once}, block {block}) differs: seed {seed} shift {shift} {width}x{height} tile {tile_size}"
                                );
                            }
                        }
                    }
                    let ordered_walk = with_write_once(false, || render_geometry_styled_unbinned(&scene, &request).unwrap());
                    let once = with_write_once(true, || render_geometry_styled(&scene, &request).unwrap());
                    let once_walk = with_write_once(true, || render_geometry_styled_unbinned(&scene, &request).unwrap());
                    assert_eq!(ordered.frame.pixels(), ordered_walk.frame.pixels());
                    assert_eq!(ordered.stats.once_full_tiles, 0);
                    let case = format!("seed {seed} shift {shift} {width}x{height} tile {tile_size}");
                    assert!(once.frame.pixels() == ordered.frame.pixels(), "binned write-once differs: {case}");
                    assert!(once_walk.frame.pixels() == ordered.frame.pixels(), "walked write-once differs: {case}");
                    let session = with_write_once(true, || session_frame(&scene, &request, true, 1));
                    assert_eq!(
                        (session.stats.once_full_tiles, session.stats.once_passes_skipped, session.stats.once_items_skipped),
                        (once.stats.once_full_tiles, once.stats.once_passes_skipped, once.stats.once_items_skipped),
                        "the session skips what the normal path skips: {case}"
                    );
                    assert!(once.frame.pixels().chunks_exact(4).any(|pixel| pixel != request.raster.background));
                    full_tiles += once.stats.once_full_tiles + once_walk.stats.once_full_tiles;
                }
            }
        }
        assert!(full_tiles > 0, "the dense scenes must fill tiles, or the early exits are untested");
    }

    #[test]
    fn write_once_spans_light_the_pixels_of_the_fill_rule_once() {
        let request = GeometryRasterRequest { width: 150, height: 9, ..request() };
        let mut stipple = [0u16; 16];
        for (row, word) in stipple.iter_mut().enumerate() {
            *word = 0xA531u16.rotate_right(row as u32);
        }
        for fill in [LayerFill::Solid, LayerFill::Speckle, LayerFill::Pattern(stipple), LayerFill::Clear] {
            // a tile that starts off a 64- and a 16-column boundary
            let mut ordered = RasterBand::new_tile(&request, 7, 143, 2, 9).unwrap();
            let mut once = ordered.clone();
            once.enable_write_once();
            let first = PaintStyle { fill, ..PaintStyle::solid([1, 2, 3, 255]) };
            let second = PaintStyle { fill, ..PaintStyle::solid([9, 8, 7, 255]) };
            let spans = [(2usize, 7usize, 143usize), (3, 60, 70), (3, 8, 9), (5, 63, 66), (8, 100, 143), (4, 7, 72)];
            for &(row, c0, c1) in &spans {
                // ordered: `first` then `second` overwrites; once: `second` wins by coming first
                fill_span(&mut ordered, first, request.height, row, c0, c1);
            }
            for &(row, c0, c1) in &spans[..3] {
                fill_span(&mut ordered, second, request.height, row, c0, c1);
                let lit = fill_span(&mut once, second, request.height, row, c0, c1);
                let mut probe = RasterBand::new_tile(&request, 7, 143, 2, 9).unwrap();
                assert_eq!(lit, fill_span(&mut probe, second, request.height, row, c0, c1), "{fill:?} row {row}");
            }
            for &(row, c0, c1) in &spans {
                fill_span(&mut once, first, request.height, row, c0, c1);
            }
            assert!(once.pixels == ordered.pixels, "{fill:?}");
            let lit = ordered.pixels.chunks_exact(4).filter(|pixel| **pixel != request.background).count() as u32;
            let width = once.tile_width() * (once.row1 - once.row0);
            assert_eq!(once.once.as_ref().unwrap().open, width - lit, "{fill:?}: open counts the unwritten pixels");
        }
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
            area_true: false,
            width_c: 1.0,
            survivor_list: true,
            place_lattice: false,
            sub_cut_arrays: false,
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
            area_true: false,
            width_c: 1.0,
            survivor_list: true,
            place_lattice: false,
            sub_cut_arrays: false,
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
            area_true: false,
            width_c: 1.0,
            survivor_list: true,
            place_lattice: false,
            sub_cut_arrays: false,
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
            area_true: false,
            width_c: 1.0,
            survivor_list: true,
            place_lattice: false,
            sub_cut_arrays: false,
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
                page_levels: Vec::new(),
                insts: Vec::new(),
                frames: Vec::new(),
                washes: Vec::new(),
                reps: Vec::new(),
            }],
            pages: vec![0, 1],
            page_prio: vec![0, 1],
            stats: HierStats::default(),
                    explain: Vec::new(),
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
                page_levels: Vec::new(),
                insts: Vec::new(),
                frames,
                washes: Vec::new(),
                reps: Vec::new(),
            }],
            pages: vec![0, 1],
            page_prio: vec![0, 1],
            stats: HierStats::default(),
                    explain: Vec::new(),
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
                page_levels: Vec::new(),
                insts: Vec::new(),
                frames: Vec::new(),
                washes: Vec::new(),
                reps: Vec::new(),
            }],
            pages: vec![page_id],
            page_prio: vec![0],
            stats: HierStats::default(),
                    explain: Vec::new(),
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
                    page_levels: Vec::new(),
                    insts: Vec::new(),
                    frames: Vec::new(),
                    washes: Vec::new(),
                    reps: Vec::new(),
                }],
                pages: vec![0],
                page_prio: vec![0],
                stats: HierStats::default(),
                            explain: Vec::new(),
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
            area_true: false,
            width_c: 1.0,
            survivor_list: true,
            place_lattice: false,
            sub_cut_arrays: false,
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
                    page_levels: Vec::new(),
                    insts: vec![inst(child_a, 2, 2), inst(child_b, 4, 2), inst(child_c, 2, 4)],
                    frames: Vec::new(),
                    washes: Vec::new(),
                    reps: Vec::new(),
                },
                WsCell {
                    key: child_a,
                    pages: vec![1],
                    page_levels: Vec::new(),
                    insts: Vec::new(),
                    frames: Vec::new(),
                    washes: Vec::new(),
                    reps: Vec::new(),
                },
                WsCell {
                    key: child_b,
                    pages: vec![2],
                    page_levels: Vec::new(),
                    insts: Vec::new(),
                    frames: Vec::new(),
                    washes: Vec::new(),
                    reps: Vec::new(),
                },
                WsCell {
                    key: child_c,
                    pages: vec![3],
                    page_levels: Vec::new(),
                    insts: Vec::new(),
                    frames: Vec::new(),
                    washes: Vec::new(),
                    reps: Vec::new(),
                },
            ],
            pages: vec![0, 1, 2, 3],
            page_prio: vec![0, 1, 2, 3],
            stats: HierStats::default(),
                    explain: Vec::new(),
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
        shape_cut_scene(rects, polys, paths, 0)
    }

    /// `hairline_scene` planned with the hairline-keeping cut (HierStats::
    /// shape_cut with shape_cut_max)
    fn shape_cut_max_scene(rects: Vec<RectRec>, polys: Vec<PolyRec>, paths: Vec<PathRec>, shape_cut: u64) -> FrameScene {
        shape_cut_scene_with(rects, polys, paths, HierStats { shape_cut, shape_cut_max: true, ..HierStats::default() })
    }

    /// `hairline_scene` planned with a per-shape cut (HierStats::shape_cut)
    fn shape_cut_scene(rects: Vec<RectRec>, polys: Vec<PolyRec>, paths: Vec<PathRec>, shape_cut: u64) -> FrameScene {
        shape_cut_scene_with(rects, polys, paths, HierStats { shape_cut, ..HierStats::default() })
    }

    fn shape_cut_scene_with(rects: Vec<RectRec>, polys: Vec<PolyRec>, paths: Vec<PathRec>, stats: HierStats) -> FrameScene {
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
                page_levels: Vec::new(),
                insts: Vec::new(),
                frames: Vec::new(),
                washes: Vec::new(),
                reps: Vec::new(),
            }],
            pages: vec![0],
            page_prio: vec![0],
            stats,
                    explain: Vec::new(),
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

    /// `hairline_request` under the area-true rule, `size` px square over the
    /// same 320-unit world, with the given tiling
    fn area_true_request(size: u32, tile_size: u16, workers: u16) -> StyledGeometryRasterRequest {
        let mut request = hairline_request();
        request.raster.area_true = true;
        request.raster.width = size;
        request.raster.height = size;
        request.raster.tile_size = tile_size;
        request.raster.workers = workers;
        request
    }

    fn lit_set(frame: &RgbaFrame, size: usize) -> BTreeSet<(usize, usize)> {
        let mut lit = BTreeSet::new();
        for row in 0..size {
            for col in 0..size {
                if pixel(frame, col, row) != [0, 0, 0, 255] {
                    lit.insert((col, row));
                }
            }
        }
        lit
    }

    /// even-odd inside test of a pixel centre (world units)
    fn centre_inside(pts: &[(i64, i64)], x: f64, y: f64) -> bool {
        let mut inside = false;
        for k in 0..pts.len() {
            let (x0, y0) = (pts[k].0 as f64, pts[k].1 as f64);
            let (x1, y1) = (pts[(k + 1) % pts.len()].0 as f64, pts[(k + 1) % pts.len()].1 as f64);
            if (y0 > y) != (y1 > y) && x < x0 + (y - y0) * (x1 - x0) / (y1 - y0) {
                inside = !inside;
            }
        }
        inside
    }

    #[test]
    fn area_true_polygons_light_exactly_the_pixels_whose_centres_they_cover() {
        // 10 units a pixel, solid fill: a polygon's lit set is its centre set -
        // no pixel of growth past it
        let lshape = vec![(153, 23), (297, 23), (297, 91), (211, 91), (211, 293), (153, 293)];
        let polys = vec![PolyRec { layer: 1, dt: 0, pts: lshape.clone(), rep: Rep::One }];
        let request = area_true_request(32, DEFAULT_TILE_SIZE, 1);
        let frame = render_geometry_styled(&hairline_scene(Vec::new(), polys, Vec::new()), &request).unwrap().frame;
        let mut want = BTreeSet::new();
        for row in 0..32usize {
            for col in 0..32usize {
                let (x, y) = ((col as f64 + 0.5) * 10.0, 320.0 - (row as f64 + 0.5) * 10.0);
                if centre_inside(&lshape, x, y) {
                    want.insert((col, row));
                }
            }
        }
        assert_eq!(lit_set(&frame, 32), want, "area-true polygon differs from its pixel-centre set");
    }

    #[test]
    fn width_first_rectangles_keep_their_whole_pixels_and_their_mean_width() {
        // one axis: m = ceil(w - t) is floor(w) or floor(w) + 1, the box stays
        // inside the pixels the side touches, m never grows when w shrinks,
        // and over ranks the mean m is w
        let one = DEVICE_ONE;
        let mut state = 0x0bad_5eed_1234_5678u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        for _ in 0..20_000 {
            let v0 = ((next() * 1000.0) * one as f64) as i128;
            let w = next() * 7.0;
            let v1 = v0 + (w * one as f64) as i128;
            let t = next();
            let wide = (v1 - v0) as f64 / one as f64;
            match width_first_span(v0, v1, t) {
                None => assert!(wide - t <= 0.0, "w {} t {}: dropped", wide, t),
                Some((a, b)) => {
                    let m = b - a;
                    assert!(m == wide.floor() as i128 || m == wide.floor() as i128 + 1, "w {}: {} px", wide, m);
                    assert_eq!(m > wide.floor() as i128, wide - wide.floor() > t, "w {} t {}", wide, t);
                    assert!(a >= floor_div(v0, one) && b <= ceil_div(v1, one), "w {}: box left its pixels", wide);
                    // half the side: never more pixels
                    let half = width_first_span(v0, v0 + (v1 - v0) / 2, t).map_or(0, |(a, b)| b - a);
                    assert!(half <= m);
                }
            }
        }
        // mean width over ranks, and independent axes: a 0.5 x 0.5 px box shows 1 in 4
        let (w, trials) = (2.35, 40_000usize);
        let mut total = 0i128;
        let (mut shown, mut quarter) = (0usize, 0usize);
        for k in 0..trials {
            let world = BBox { x0: k as i64 * 97, y0: 11, x1: k as i64 * 97 + 5, y1: 16 };
            total += width_first_span(0, (w * one as f64) as i128, salted_rank(world, 1)).map_or(0, |(a, b)| b - a);
            let half = (0.5 * one as f64) as i128;
            let x = width_first_span(0, half, salted_rank(world, 1)).is_some();
            let y = width_first_span(0, half, salted_rank(world, 2)).is_some();
            shown += x as usize;
            quarter += (x && y) as usize;
        }
        let mean = total as f64 / trials as f64;
        assert!((mean - w).abs() < 0.01, "mean width {}", mean);
        assert!((shown as f64 / trials as f64 - 0.5).abs() < 0.01);
        assert!((quarter as f64 / trials as f64 - 0.25).abs() < 0.01, "0.5 x 0.5 px shown {}", quarter as f64 / trials as f64);
        // in a frame: the 1.2 px gap between two 3.8 px bars stays open (the
        // boxes never leave the pixels the bars touch), each bar 3 or 4 px
        let rect = |x, y, w, h| RectRec { layer: 1, dt: 0, x, y, w, h, rep: Rep::One };
        let bars = vec![rect(13, 13, 38, 294), rect(63, 13, 38, 294)];
        let request = area_true_request(32, DEFAULT_TILE_SIZE, 1);
        let lit = lit_set(&render_geometry_styled(&hairline_scene(bars.clone(), Vec::new(), Vec::new()), &request).unwrap().frame, 32);
        assert!(lit.iter().all(|&(col, _)| col != 5), "the 1.2 px gap between the bars was closed");
        for range in [1..6usize, 6..11] {
            let cols: BTreeSet<usize> = lit.iter().map(|&(c, _)| c).filter(|c| range.contains(c)).collect();
            assert!(cols.len() == 3 || cols.len() == 4, "a 3.8 px bar drew {} columns", cols.len());
        }
        let mut klayout = request.clone();
        klayout.raster.area_true = false;
        let grown = lit_set(&render_geometry_styled(&hairline_scene(bars, Vec::new(), Vec::new()), &klayout).unwrap().frame, 32);
        assert!(grown.iter().any(|&(col, _)| col == 5), "the KLayout rule grows the first bar into the gap");
    }

    /// Every member of a Grid record placed by `transform`: (offset, world box).
    fn grid_members(local: BBox, rep: &Rep, transform: &OrthoTransform) -> Vec<((i64, i64), BBox)> {
        let Rep::Grid { na, nb, va, vb } = rep else { unreachable!() };
        let mut out = Vec::new();
        for j in 0..*nb as i64 {
            for i in 0..*na as i64 {
                let (ox, oy) = (i * va.0 + j * vb.0, i * va.1 + j * vb.1);
                let member = BBox { x0: local.x0 + ox, y0: local.y0 + oy, x1: local.x1 + ox, y1: local.y1 + oy };
                out.push(((ox, oy), transform.apply_bbox(member).unwrap()));
            }
        }
        out
    }

    /// (world box -> ranks) of every member of these records.
    fn member_ranks(records: &[(BBox, Rep, OrthoTransform)]) -> BTreeMap<(i64, i64, i64, i64), (f64, f64)> {
        let mut out = BTreeMap::new();
        for (local, rep, transform) in records {
            let grid = GridRanks::new(rep, transform, transform.apply_bbox(*local).unwrap()).unwrap().unwrap();
            for ((ox, oy), world) in grid_members(*local, rep, transform) {
                let ranks = grid.ranks(ox, oy, &world).expect("ranks");
                assert!(out.insert((world.x0, world.y0, world.x1, world.y1), ranks).is_none(), "a member twice");
            }
        }
        out
    }

    #[test]
    fn array_members_spread_their_extra_pixels_by_index() {
        // a row of 64 bars 1.5 px wide: exactly half draw 2 px, and never
        // more than two neighbours in a row decide alike ("one in two"); the
        // same along a column, and for a row placed rotated to vertical
        let runs = |wide: &[bool]| {
            let (mut best, mut run) = (1, 1);
            for k in 1..wide.len() {
                run = if wide[k] == wide[k - 1] { run + 1 } else { 1 };
                best = best.max(run);
            }
            best
        };
        let identity = OrthoTransform::identity();
        let rotated = OrthoTransform::place(0, 0, 1, false).unwrap();
        let bar = BBox { x0: 1000, y0: 2000, x1: 1015, y1: 2300 };
        for (rep, transform, axis) in [
            (Rep::Grid { na: 64, nb: 1, va: (30, 0), vb: (0, 0) }, &identity, 0),
            (Rep::Grid { na: 1, nb: 64, va: (0, 0), vb: (0, 30) }, &identity, 1),
            (Rep::Grid { na: 64, nb: 1, va: (0, 30), vb: (0, 0) }, &identity, 1),
            (Rep::Grid { na: 64, nb: 1, va: (30, 0), vb: (0, 0) }, &rotated, 1),
            // a skewed grid keeps the record's own index
            (Rep::Grid { na: 64, nb: 1, va: (30, 7), vb: (0, 0) }, &identity, 0),
        ] {
            let grid = GridRanks::new(&rep, transform, transform.apply_bbox(bar).unwrap()).unwrap().unwrap();
            let wide: Vec<bool> = grid_members(bar, &rep, transform)
                .iter()
                .map(|((ox, oy), world)| {
                    let ranks = grid.ranks(*ox, *oy, world).expect("an index");
                    1.5 - if axis == 0 { ranks.0 } else { ranks.1 } > 1.0
                })
                .collect();
            let count = wide.iter().filter(|w| **w).count();
            assert!((31..=33).contains(&count), "{:?}: {} of 64 wide", rep, count);
            assert!(runs(&wide) <= 2, "{:?}: a run of {} alike", rep, runs(&wide));
        }
        // a 32 x 32 array of sub-pixel points keeps the covered share
        let rep = Rep::Grid { na: 32, nb: 32, va: (20, 0), vb: (0, 20) };
        let grid = GridRanks::new(&rep, &identity, bar).unwrap().unwrap();
        for (w, h) in [(0.5, 0.5), (0.3, 0.7), (0.9, 0.2)] {
            let kept = grid_members(bar, &rep, &identity)
                .iter()
                .filter(|((ox, oy), world)| {
                    let (tx, ty) = grid.ranks(*ox, *oy, world).unwrap();
                    w > tx && h > ty
                })
                .count();
            let share = kept as f64 / 1024.0;
            assert!((share - w * h).abs() < 0.03, "{} x {} px points kept {}", w, h, share);
        }
        // a skewed grid finds its record indices; a collinear 2-D one keeps the hash
        let skew = GridRanks::new(&Rep::Grid { na: 5, nb: 7, va: (30, 10), vb: (-7, 40) }, &identity, bar).unwrap().unwrap();
        assert_eq!(skew.index(3 * 30 + 4 * -7, 3 * 10 + 4 * 40), Some((3, 4)));
        let collinear = GridRanks::new(&Rep::Grid { na: 5, nb: 7, va: (30, 0), vb: (60, 0) }, &identity, bar).unwrap().unwrap();
        assert_eq!(collinear.ranks(90, 0, &bar), None);
    }

    #[test]
    fn an_axis_aligned_array_ranks_the_same_however_it_is_stored() {
        // ADAPTIVE_CUT_DENSITY_PLAN §4.2 candidate 2: the same world lattice as
        // one Grid, as the index's re-based fragments, with its axes swapped,
        // a pitch negated, or placed rotated / mirrored - every member keeps
        // its ranks (the per-record index gave 10 of 64 different picks)
        let id = OrthoTransform::identity();
        let bar = BBox { x0: -1003, y0: 2000, x1: -988, y1: 2300 };
        let at = |dx: i64, dy: i64| BBox { x0: bar.x0 + dx, y0: bar.y0 + dy, x1: bar.x1 + dx, y1: bar.y1 + dy };
        let whole = member_ranks(&[(bar, Rep::Grid { na: 64, nb: 1, va: (30, 0), vb: (0, 0) }, id)]);
        assert_eq!(whole.len(), 64);
        let fragments = member_ranks(&[
            (bar, Rep::Grid { na: 32, nb: 1, va: (30, 0), vb: (0, 0) }, id),
            (at(32 * 30, 0), Rep::Grid { na: 20, nb: 1, va: (30, 0), vb: (0, 0) }, id),
            (at(52 * 30, 0), Rep::Grid { na: 12, nb: 1, va: (30, 0), vb: (0, 0) }, id),
        ]);
        assert_eq!(fragments, whole, "fragments of the lattice rank differently");
        let swapped = member_ranks(&[(bar, Rep::Grid { na: 1, nb: 64, va: (0, 0), vb: (30, 0) }, id)]);
        assert_eq!(swapped, whole, "the axis-swapped record ranks differently");
        let negated = member_ranks(&[(at(63 * 30, 0), Rep::Grid { na: 64, nb: 1, va: (-30, 0), vb: (0, 0) }, id)]);
        assert_eq!(negated, whole, "the negated pitch ranks differently");
        // a column of bars in a cell placed rotated (and mirrored) onto the row
        for (rot, flip) in [(1u8, false), (3, false), (1, true), (3, true)] {
            let place = OrthoTransform::place(0, 0, rot, flip).unwrap();
            let inverse = place.invert().unwrap();
            // the local member 0 that lands on the row's first bar, the local step
            // that lands on (+30, 0)
            let local = inverse.apply_bbox(bar).unwrap();
            let (sx, sy) = inverse.apply(30, 0).unwrap();
            let (ox, oy) = inverse.apply(0, 0).unwrap();
            let step = (sx - ox, sy - oy);
            let placed = member_ranks(&[(local, Rep::Grid { na: 64, nb: 1, va: step, vb: (0, 0) }, place)]);
            assert_eq!(placed, whole, "the rotated placement (rot {} flip {}) ranks differently", rot, flip);
        }
        // a 2-D lattice and its four 4 x 4 fragments
        let two = member_ranks(&[(bar, Rep::Grid { na: 8, nb: 8, va: (30, 0), vb: (0, 400) }, id)]);
        let quarters = member_ranks(&[
            (bar, Rep::Grid { na: 4, nb: 4, va: (30, 0), vb: (0, 400) }, id),
            (at(120, 0), Rep::Grid { na: 4, nb: 4, va: (30, 0), vb: (0, 400) }, id),
            (at(0, 1600), Rep::Grid { na: 4, nb: 4, va: (30, 0), vb: (0, 400) }, id),
            (at(120, 1600), Rep::Grid { na: 4, nb: 4, va: (0, 400), vb: (30, 0) }, id),
        ]);
        assert_eq!(quarters, two, "fragments of the 2-D lattice rank differently");
        // a different phase or size is a different lattice
        let shifted = member_ranks(&[(at(7, 0), Rep::Grid { na: 64, nb: 1, va: (30, 0), vb: (0, 0) }, id)]);
        assert_ne!(shifted.values().collect::<Vec<_>>(), whole.values().collect::<Vec<_>>());
    }

    #[test]
    fn a_fragment_that_lost_an_axis_ranks_as_its_own_row() {
        // review 2026-09-23: a one-row piece of a 2-D lattice is written as a
        // one-dimensional repetition - the page file drops the other vector
        // (oasis write.rs, the nb == 1 arms; doc.rs reads vb (0, 0) back) - so
        // it cannot be told from an array stored as that row from the start.
        // It ranks as that row's own lattice: with or without the dropped
        // vector, as a plain one-row array, under further splits of the row,
        // and it still spreads its extra pixels; it keys apart from the 2-D
        // lattice it was cut from (out of the identity guarantee). A one-member
        // piece (Rep::One) and a 1 x 1 grid take the single shape's hash.
        let id = OrthoTransform::identity();
        let bar = BBox { x0: -1003, y0: 2000, x1: -988, y1: 2300 };
        let at = |dx: i64, dy: i64| BBox { x0: bar.x0 + dx, y0: bar.y0 + dy, x1: bar.x1 + dx, y1: bar.y1 + dy };
        let row = |na: u64, vb: (i64, i64)| Rep::Grid { na, nb: 1, va: (30, 0), vb };
        let lattice = member_ranks(&[(bar, Rep::Grid { na: 64, nb: 8, va: (30, 0), vb: (0, 400) }, id)]);
        let of_lattice = |keep: &dyn Fn(&(i64, i64, i64, i64)) -> bool| {
            lattice.iter().filter(|(k, _)| keep(k)).map(|(k, v)| (*k, *v)).collect::<BTreeMap<_, _>>()
        };
        // the fourth row: frag_rep keeps vb with count 1 in memory (nj == 1)
        let dy = 3 * 400;
        let kept = member_ranks(&[(at(0, dy), row(64, (0, 400)), id)]);
        let written = member_ranks(&[(at(0, dy), row(64, (0, 0)), id)]);
        assert_eq!(kept.len(), 64);
        assert_eq!(kept, written, "the dropped vector changed the row's ranks");
        let split = member_ranks(&[(at(0, dy), row(20, (0, 0)), id), (at(20 * 30, dy), row(44, (0, 0)), id)]);
        assert_eq!(split, written, "a split of the row ranks differently");
        assert_ne!(written, of_lattice(&|k| k.1 == bar.y0 + dy), "the row piece ranks as the 2-D lattice");
        // it still spreads: of 64 bars 1.5 px wide half draw 2 px, never three alike in a row
        let wide: Vec<bool> = written.values().map(|ranks| 1.5 - ranks.0 > 1.0).collect();
        let count = wide.iter().filter(|w| **w).count();
        assert!((31..=33).contains(&count), "{} of 64 wide", count);
        assert!(wide.windows(3).all(|w| !(w[0] == w[1] && w[1] == w[2])), "three alike in a row");
        // the sixth column: frag_rep swaps the vectors (ni == 1)
        let dx = 5 * 30;
        let column = member_ranks(&[(at(dx, 0), Rep::Grid { na: 8, nb: 1, va: (0, 400), vb: (30, 0) }, id)]);
        let plain = member_ranks(&[(at(dx, 0), Rep::Grid { na: 1, nb: 8, va: (0, 0), vb: (0, 400) }, id)]);
        assert_eq!(column.len(), 8);
        assert_eq!(column, plain, "the swapped column piece ranks apart from a plain column");
        assert_ne!(column, of_lattice(&|k| k.0 == bar.x0 + dx), "the column piece ranks as the 2-D lattice");
        // one member: no grid ranks, and a frame draws a 1 x 1 grid as the shape alone
        assert!(GridRanks::new(&Rep::One, &id, bar).unwrap().is_none());
        assert!(GridRanks::new(&Rep::Grid { na: 1, nb: 1, va: (30, 0), vb: (0, 400) }, &id, bar).unwrap().is_none());
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = |span: i64| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as i64 % span
        };
        let dots: Vec<(i64, i64)> = (0..300).map(|_| (next(310), next(310))).collect();
        let draw = |rep: &Rep| {
            let rects = dots.iter().map(|&(x, y)| RectRec { layer: 1, dt: 0, x, y, w: 5, h: 5, rep: rep.clone() }).collect();
            let request = area_true_request(32, DEFAULT_TILE_SIZE, 1);
            lit_set(&render_geometry_styled(&hairline_scene(rects, Vec::new(), Vec::new()), &request).unwrap().frame, 32)
        };
        let alone = draw(&Rep::One);
        assert!(!alone.is_empty() && alone.len() < 300, "{} of 300 half-pixel dots lit", alone.len());
        assert_eq!(draw(&Rep::Grid { na: 1, nb: 1, va: (30, 0), vb: (0, 400) }), alone, "a 1 x 1 grid drew apart from the shape");
    }

    #[test]
    fn extra_sparsening_thins_the_extra_pixels_continuously() {
        // ADAPTIVE_CUT_DENSITY_PLAN §4.2 candidate 1 (diagnostic
        // FLOE_RUST_WIDTH_C): under c the extra pixel comes when t < P_c(f),
        // P_c(f) = f / (c - (c - 1) f). c = 1 is the plain rule; c = 2 keeps
        // a 0.05 px side about 1 time in 39 and a 1.99 px side 1.98 px on
        // average (f / c would give 1.495 - a jump at 2 px); the span under
        // c is within the plain rule's span, never under floor(w), and is
        // monotone in w at a fixed rank.
        let one = DEVICE_ONE as i128;
        let side = |w: f64| (w * one as f64).round() as i128;
        let mean = |w: f64, c: f64| {
            let n = 20_000;
            (0..n).map(|k| width_first_span_c(0, side(w), (k as f64 + 0.5) / n as f64, c).map_or(0, |(a, b)| b - a)).sum::<i128>() as f64 / n as f64
        };
        for w in [0.05, 0.5, 1.05, 1.5, 1.99, 2.0, 3.8] {
            assert!((mean(w, 1.0) - w).abs() < 0.001, "c = 1 keeps the mean width {} ({})", w, mean(w, 1.0));
            let f = w - w.floor();
            let p2 = f / (2.0 - f);
            assert!((mean(w, 2.0) - (w.floor() + p2)).abs() < 0.001, "c = 2 at {}: {} for {}", w, mean(w, 2.0), w.floor() + p2);
        }
        assert!((mean(0.05, 2.0) - 1.0 / 39.0).abs() < 0.001);
        assert!((mean(1.99, 2.0) - 1.980).abs() < 0.002);
        assert!(mean(1.99, 2.0) < mean(2.0, 2.0) && mean(2.0, 2.0) == 2.0, "continuous at a whole width");
        // containment and monotony at fixed ranks
        let mut state = 0x1234_5678u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        for _ in 0..5000 {
            let (t, w, v0) = (next(), next() * 6.0, (next() * 40.0 * one as f64) as i128);
            let plain = width_first_span_c(v0, v0 + side(w), t, 1.0);
            for c in [1.5, 2.0, 4.0] {
                let sparse = width_first_span_c(v0, v0 + side(w), t, c);
                match (plain, sparse) {
                    (None, Some(_)) => panic!("c = {} drew what c = 1 dropped (w {} t {})", c, w, t),
                    (Some((a, b)), Some((x, y))) => {
                        assert!(x >= a && y <= b, "c = {}: [{}, {}) left [{}, {}) (w {} t {})", c, x, y, a, b, w, t);
                        assert!(y - x >= w.floor() as i128, "c = {} under floor(w)", c);
                    }
                    _ => {}
                }
                let wider = width_first_span_c(v0, v0 + side(w + 0.37), t, c).map_or(0, |(a, b)| b - a);
                assert!(wider >= sparse.map_or(0, |(a, b)| b - a), "c = {}: not monotone in w", c);
            }
        }
    }

    /// ADAPTIVE_CUT_DENSITY_PLAN §4.3 step 1 (2026-09-23): can a lattice's
    /// sub-pixel survivors be listed without visiting every member? Under
    /// GridRanks' lattice rule a member (ix, iy) survives a w_x x w_y px
    /// draw when frac(u_x + vdc(ix) + weyl(iy)) < w_x and the swapped test
    /// for y. For a fixed row the x condition puts vdc(ix) in one interval
    /// (mod 1), and vdc maps a dyadic interval [m 2^-b, (m + 1) 2^-b) onto
    /// the arithmetic progression ix = rev_b(m) (mod 2^b) - so the row's
    /// candidates are the union of at most ~2 x 53 progressions, walked
    /// survivor by survivor; the y condition (it couples to ix through
    /// weyl(ix)) is then tested on those candidates only. The interval is
    /// widened by 2^-40 and every candidate re-tested with the exact f64
    /// rule, so the set is the full scan's by construction.
    fn lattice_survivors_by_enumeration(
        u: (f64, f64),
        cols: std::ops::Range<u64>,
        rows: std::ops::Range<u64>,
        w: (f64, f64),
    ) -> (Vec<(u64, u64)>, u64) {
        const BITS: u32 = 53;
        let vdc = |k: u64| (k.reverse_bits() >> 11) as f64 / (1u64 << BITS) as f64;
        let weyl = |k: u64| (k.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 11) as f64 / (1u64 << BITS) as f64;
        let rank = |u: f64, p: u64, s: u64| (u + vdc(p) + weyl(s)).rem_euclid(1.0);
        let scale = (1u64 << BITS) as f64;
        let pad = 2f64.powi(-40);
        let mut out = Vec::new();
        let mut tested = 0u64;
        for iy in rows {
            // vdc(ix) must lie in [lo, lo + w_x) mod 1, lo = -(u_x + weyl(iy))
            let lo = (-(u.0 + weyl(iy))).rem_euclid(1.0);
            let mut pieces: Vec<(u64, u64)> = Vec::new();
            let a = ((lo - pad).max(0.0) * scale) as u64;
            let b = ((lo + w.0 + pad).min(1.0) * scale).ceil() as u64;
            pieces.push((a, b.min(1 << BITS)));
            if lo + w.0 + pad > 1.0 {
                pieces.push((0, (((lo + w.0 + pad - 1.0) * scale).ceil() as u64).min(1 << BITS)));
            }
            if lo - pad < 0.0 {
                pieces.push((((lo - pad + 1.0) * scale) as u64, 1 << BITS));
            }
            for (mut lo_r, hi_r) in pieces {
                // dyadic decomposition of [lo_r, hi_r) in the 53-bit reversed space
                while lo_r < hi_r {
                    let mut s = lo_r.trailing_zeros().min(BITS);
                    while s > 0 && lo_r + (1u64 << s) > hi_r {
                        s -= 1;
                    }
                    let b = BITS - s;
                    let m = lo_r >> s;
                    // the top b bits of the reversed value m are the low b bits of ix, reversed
                    let residue = if b == 0 { 0 } else { m.reverse_bits() >> (64 - b) };
                    let stride = 1u64 << b;
                    let first = cols.start + (residue.wrapping_sub(cols.start) & (stride - 1));
                    let mut ix = first;
                    while ix < cols.end {
                        tested += 1;
                        if rank(u.0, ix, iy) < w.0 && rank(u.1, iy, ix) < w.1 {
                            out.push((ix, iy));
                        }
                        ix += stride;
                    }
                    lo_r += 1u64 << s;
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        (out, tested)
    }

    #[test]
    fn sub_pixel_survivors_can_be_enumerated_without_visiting_every_member() {
        // ADAPTIVE_CUT_DENSITY_PLAN §4.3 step 1: the enumeration's set is
        // the full scan's, and it visits about the survivors plus ~100
        // candidates a row instead of every member
        let vdc = |k: u64| (k.reverse_bits() >> 11) as f64 / (1u64 << 53) as f64;
        let weyl = |k: u64| (k.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 11) as f64 / (1u64 << 53) as f64;
        let rank = |u: f64, p: u64, s: u64| (u + vdc(p) + weyl(s)).rem_euclid(1.0);
        let u = (0.3719, 0.8231);
        for (cols, rows, w) in [
            (0..1_000_000u64, 0..1u64, (0.05, 1.0)),
            (0..2048u64, 0..2048u64, (0.05, 0.05)),
            (0..2048u64, 0..2048u64, (0.5, 0.5)),
            (1000..3000u64, 500..700u64, (0.02, 0.9)),
        ] {
            let members = (cols.end - cols.start) * (rows.end - rows.start);
            let t = std::time::Instant::now();
            let mut scan = Vec::new();
            for iy in rows.clone() {
                for ix in cols.clone() {
                    if rank(u.0, ix, iy) < w.0 && rank(u.1, iy, ix) < w.1 {
                        scan.push((ix, iy));
                    }
                }
            }
            scan.sort_unstable();
            let scan_us = t.elapsed().as_micros();
            let t = std::time::Instant::now();
            let (listed, tested) = lattice_survivors_by_enumeration(u, cols.clone(), rows.clone(), w);
            let list_us = t.elapsed().as_micros();
            assert_eq!(listed, scan, "{}x{} at {:?}: the enumeration differs from the scan", cols.end - cols.start, rows.end - rows.start, w);
            let share = scan.len() as f64 / members as f64;
            assert!((share - w.0 * w.1).abs() < 0.01 + 0.5 * w.0 * w.1 / (members as f64).sqrt(), "share {} for {:?}", share, w);
            assert!(tested < members, "tested {} of {} members", tested, members);
            // the candidates are the x survivors (w_x of the members, the y
            // test prunes them) plus the progression ends, ~110 a row
            let x_share = (w.0 * members as f64) as u64;
            assert!(tested <= x_share + x_share / 50 + 120 * (rows.end - rows.start), "tested {} for {} x candidates in {} rows", tested, x_share, rows.end - rows.start);
            eprintln!(
                "§4.3 step 1: {} members ({}x{}) at {:?} px: {} survivors; full scan {} us, enumeration tested {} ({:.1}x fewer) in {} us",
                members, cols.end - cols.start, rows.end - rows.start, w, scan.len(), scan_us, tested, members as f64 / tested as f64, list_us
            );
        }
    }

    /// SurvivorWalk (§4.3 step 1 in the renderer, 2026-09-25): over
    /// random lattice arrays - 1-D and 2-D, pitches of either sign, members
    /// under a pixel on one axis or both, first members on either side of 0,
    /// placed under all eight orientations, WIDTH_C 1 and 4 - the listed
    /// members that pass the exact width-first test are exactly the walk's
    /// drawn members, in the walk's order, and the list is a fraction of the
    /// members.
    #[test]
    fn listed_survivors_are_the_walks_drawn_members() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = |n: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % n
        };
        let span = 50_000i64;
        let mut request = area_true_request(64, DEFAULT_TILE_SIZE, 1).raster;
        request.view = RasterViewBox::new(-span as f64, -span as f64, span as f64, span as f64).unwrap();
        let world_view = BBox { x0: -span, y0: -span, x1: span, y1: span };
        let drawn = |request: &GeometryRasterRequest, world: BBox, ranks: (f64, f64)| {
            let (x0, y1) = world_to_device(request, world.x0, world.y0).unwrap();
            let (x1, y0) = world_to_device(request, world.x1, world.y1).unwrap();
            width_first_span_c(x0, x1, ranks.0, request.width_c).is_some()
                && width_first_span_c(y0, y1, ranks.1, request.width_c).is_some()
        };
        let (mut cases, mut listed_cases, mut members, mut walked) = (0, 0, 0u64, 0u64);
        for case in 0..600 {
            request.width_c = if case % 3 == 0 { 4.0 } else { 1.0 };
            // 1562.5 world units a pixel: sides of 8..600 units are 0.005..0.4 px
            // (one case in eight up to 1 px, where a list rarely pays)
            let wide = case % 8 == 7;
            let side = |thin: bool, n: &mut dyn FnMut(u64) -> u64| {
                if thin { 8 + n(if wide { 1600 } else { 600 }) as i64 } else { 2000 + n(20_000) as i64 }
            };
            let (thin_x, thin_y) = match next(3) { 0 => (true, false), 1 => (false, true), _ => (true, true) };
            let (w, h) = (side(thin_x, &mut next), side(thin_y, &mut next));
            let pitch = |n: &mut dyn FnMut(u64) -> u64| (40 + n(3000) as i64) * if n(2) == 0 { 1 } else { -1 };
            let place = OrthoTransform::place(next(20_000) as i64 - 10_000, next(20_000) as i64 - 10_000, next(4) as u8, next(2) == 1).unwrap();
            let inverse = place.invert().unwrap();
            // repetition vectors along the WORLD axes, stored in the cell's frame
            let local = |v: (i64, i64)| {
                let (a, o) = (inverse.apply(v.0, v.1).unwrap(), inverse.apply(0, 0).unwrap());
                (a.0 - o.0, a.1 - o.1)
            };
            let (wx, wy) = (local((pitch(&mut next), 0)), local((0, pitch(&mut next))));
            let rep = match next(4) {
                0 => Rep::Grid { na: 1 + next(3000), nb: 1, va: wx, vb: (0, 0) },
                1 => Rep::Grid { na: 1, nb: 1 + next(3000), va: (0, 0), vb: wy },
                2 => Rep::Grid { na: 1 + next(200), nb: 1 + next(200), va: wx, vb: wy },
                _ => Rep::Grid { na: 1 + next(200), nb: 1 + next(200), va: wy, vb: wx },
            };
            let (x, y) = (next(2 * span as u64) as i64 - span, next(2 * span as u64) as i64 - span);
            // the local box that lands w x h in the world
            let world_base = BBox { x0: x, y0: y, x1: x + w, y1: y + h };
            let base = inverse.apply_bbox(world_base).unwrap();
            let local_view = inverse.apply_bbox(world_view).unwrap();
            let Some(grid) = GridRanks::new(&rep, &place, place.apply_bbox(base).unwrap()).unwrap() else {
                continue;
            };
            if !matches!(grid.mode, GridMode::Lattice { .. }) {
                continue;
            }
            cases += 1;
            let mut walk = Vec::new();
            let visit = for_each_visible_offset(&rep, base, local_view, |ox, oy| {
                let world = place.apply_bbox(translate_bbox(base, ox, oy).unwrap()).unwrap();
                if drawn(&request, world, grid.ranks(ox, oy, &world).unwrap()) {
                    walk.push((ox, oy));
                }
                Ok(())
            })
            .unwrap();
            let Some(survivors) = survivor_walk(&request, Some(&grid), &rep, base, local_view, &place).unwrap() else {
                continue;
            };
            let mut listed = Vec::new();
            let mut work = SurvivorWork::default();
            survivors.run(None, &mut work, &mut |ox, oy| {
                listed.push((ox, oy));
                Ok(())
            })
            .unwrap();
            assert_eq!(work.walked, listed.len() as u64);
            listed_cases += 1;
            members += visit.tested;
            walked += listed.len() as u64;
            let kept: Vec<(i64, i64)> = listed
                .iter()
                .copied()
                .filter(|&(ox, oy)| {
                    let world = place.apply_bbox(translate_bbox(base, ox, oy).unwrap()).unwrap();
                    drawn(&request, world, grid.ranks(ox, oy, &world).unwrap())
                })
                .collect();
            assert_eq!(kept, walk, "case {}: {:?} {}x{} at ({}, {}) c {}", case, rep, w, h, x, y, request.width_c);
            assert!(listed.len() as u64 * 2 <= visit.tested + 1, "case {}: listed {} of {}", case, listed.len(), visit.tested);
            // the walk never holds more than one line's cursors along j, or the
            // cap along i
            assert!(work.peak_cursors <= SURVIVOR_CURSOR_CAP);
            // a draw that stops at once stops the walk at once
            let mut calls = 0;
            let mut stopped = SurvivorWork::default();
            let result = survivors.run(None, &mut stopped, &mut |_, _| {
                calls += 1;
                Err(WRITE_ONCE_FULL.to_string())
            });
            let first = usize::from(!listed.is_empty());
            assert!(result.is_err() == (first == 1) && calls == first && stopped.walked == first as u64, "case {}: {} calls", case, calls);
            // off: no walk
            let mut off = request;
            off.survivor_list = false;
            assert!(survivor_walk(&off, Some(&grid), &rep, base, local_view, &place).unwrap().is_none());
        }
        assert!(cases > 500 && listed_cases > 100, "{} lattice cases, {} listed", cases, listed_cases);
        eprintln!(
            "survivor lists: {} of {} lattice arrays listed, {} members walked instead of {} ({:.1}x fewer)",
            listed_cases, cases, walked, members, members as f64 / walked.max(1) as f64
        );
    }

    /// The listing in the frame: sub-pixel lattice arrays (dense dots both
    /// axes, bars thin across their row, members on both sides of 0) draw
    /// byte-identical frames with the list on and off, at two tilings, one and
    /// three workers and WIDTH_C 1 and 4, walking a fraction of the members.
    #[test]
    fn a_frame_draws_the_same_with_the_survivor_list() {
        let rect = |x, y, w, h, rep: Rep| RectRec { layer: 1, dt: 0, x, y, w, h, rep };
        let scene = hairline_scene(
            vec![
                // 1 x 1 unit dots at a 1-unit pitch over the frame and beyond 0
                rect(-40, -40, 1, 1, Rep::Grid { na: 400, nb: 400, va: (1, 0), vb: (0, 1) }),
                // bars 1 unit wide, 200 long, a 2-unit pitch, from x = -100
                rect(-100, 60, 1, 200, Rep::Grid { na: 300, nb: 1, va: (2, 0), vb: (0, 0) }),
                // a column of wires 1 unit high, a 3-unit pitch downwards
                rect(20, 310, 250, 1, Rep::Grid { na: 1, nb: 150, va: (0, 0), vb: (0, -3) }),
            ],
            Vec::new(),
            Vec::new(),
        );
        for (tile, workers, c) in [(DEFAULT_TILE_SIZE, 1u16, 1.0), (16, 3, 1.0), (16, 1, 4.0)] {
            let mut on = area_true_request(32, tile, workers);
            on.raster.width_c = c;
            let mut off = on.clone();
            off.raster.survivor_list = false;
            let a = render_geometry_styled(&scene, &on).unwrap();
            let b = render_geometry_styled(&scene, &off).unwrap();
            assert_eq!(a.frame, b.frame, "tile {} workers {} c {}", tile, workers, c);
            assert!(!lit_set(&a.frame, 32).is_empty());
            assert!(
                a.stats.rep_members_tested * 3 < b.stats.rep_members_tested,
                "listed {} of {} members",
                a.stats.rep_members_tested,
                b.stats.rep_members_tested
            );
            assert_eq!(a.stats.rep_members_drawn, b.stats.rep_members_drawn);
        }
    }

    /// Review 2026-09-25 (timing, run by hand with --ignored): a 2 M-member
    /// sub-pixel array behind a tile that one open pixel keeps from full - the
    /// first survivors fill it, so a walk that lists survivors must not
    /// prepare them all first.
    #[test]
    #[ignore]
    fn survivor_walk_in_a_nearly_full_tile_timing() {
        let rect = |x, y, w, h, rep: Rep| RectRec { layer: 1, dt: 0, x, y, w, h, rep };
        let scene = hairline_scene(
            vec![
                rect(100, 0, 25_500, 25_600, Rep::One),
                rect(0, 100, 100, 25_500, Rep::One),
                rect(0, 0, 30, 30, Rep::Grid { na: 2000, nb: 1000, va: (13, 0), vb: (0, 26) }),
            ],
            Vec::new(),
            Vec::new(),
        );
        let mut on = hairline_request();
        on.raster.area_true = true;
        on.raster.view = RasterViewBox::new(0.0, 0.0, 25_600.0, 25_600.0).unwrap();
        on.raster.width = 256;
        on.raster.height = 256;
        on.raster.tile_size = 384;
        let mut off = on.clone();
        off.raster.survivor_list = false;
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for k in 0..8 {
            for (list, times) in [(true, &mut a), (false, &mut b)] {
                let request = if list { &on } else { &off };
                let report = render_geometry_styled(&scene, request).unwrap();
                if k > 0 {
                    times.push(report.stats.raster_us);
                }
            }
        }
        a.sort_unstable();
        b.sort_unstable();
        let same = render_geometry_styled(&scene, &on).unwrap().frame == render_geometry_styled(&scene, &off).unwrap().frame;
        eprintln!("nearly full tile: raster list on {} us, off {} us (medians of 7), pixels identical {}", a[3], b[3], same);
    }

    /// A leaf cell's pages (layer, rectangles, polygons) placed by one
    /// instance (x, y, rot, flip, rep) in an otherwise empty top cell spanning
    /// `world` - the placement lattice's test scene.
    fn placed_scene(leaf: Vec<(u32, Vec<RectRec>, Vec<PolyRec>)>, inst: (i64, i64, u8, bool, Rep), world: BBox) -> FrameScene {
        placed_scene_cut(leaf, inst, world, HierStats::default())
    }

    /// `placed_scene` planned with the given stats (a per-shape cut)
    fn placed_scene_cut(leaf: Vec<(u32, Vec<RectRec>, Vec<PolyRec>)>, inst: (i64, i64, u8, bool, Rep), world: BBox, stats: HierStats) -> FrameScene {
        let (top, cell) = ((0, REM_FULL), (1, REM_FULL));
        let mut pages = Vec::new();
        let mut leaf_box = BBox::EMPTY;
        for (k, (layer, rects, polys)) in leaf.into_iter().enumerate() {
            let mut bbox = BBox::EMPTY;
            for r in &rects {
                bbox.grow(&BBox { x0: r.x, y0: r.y, x1: r.x + r.w, y1: r.y + r.h });
            }
            for q in &polys {
                bbox.grow(&polygon_bbox(&q.pts).unwrap());
            }
            leaf_box.grow(&bbox);
            let doc = Doc {
                unit: 1.0,
                cells: vec![Cell { name: format!("L{k}"), rects, polys, ..Cell::default() }],
                top: 0,
                layer_order: vec![(layer, 0)],
                norm_s: 0.0,
                layer_names: HashMap::new(),
                layer_aliases: HashMap::new(),
            };
            pages.push(Arc::new(DecodedPage {
                page_id: k as u32,
                layer_idx: layer,
                bbox,
                encoded_bytes: 1,
                records: 1,
                members: 1,
                index: crate::PageIndex::build(&doc),
                doc,
            }));
        }
        let n = pages.len() as u32;
        let (x, y, rot, flip, rep) = inst;
        let plan = HierPlan {
            top,
            wcells: vec![
                WsCell {
                    key: top,
                    pages: Vec::new(),
                    page_levels: Vec::new(),
                    insts: vec![WsInst { child: cell, x, y, rot, flip, rep }],
                    frames: Vec::new(),
                    washes: Vec::new(),
                    reps: Vec::new(),
                },
                WsCell {
                    key: cell,
                    pages: (0..n).collect(),
                    page_levels: Vec::new(),
                    insts: Vec::new(),
                    frames: Vec::new(),
                    washes: Vec::new(),
                    reps: Vec::new(),
                },
            ],
            pages: (0..n).collect(),
            page_prio: vec![0; n as usize],
            stats,
            explain: Vec::new(),
        };
        FrameScene::from_test_parts(plan, pages, BTreeMap::from([(top, world), (cell, leaf_box)])).unwrap()
    }

    /// An area-true request over `world` at `size` px with the placement
    /// lattice and the survivor list as given, layers 1 (white) and 2 (red).
    fn lattice_request(world: BBox, size: u32, tile: u16, workers: u16, place: bool, list: bool) -> StyledGeometryRasterRequest {
        let mut request = area_true_request(size, tile, workers);
        request.raster.view = RasterViewBox::new(world.x0 as f64, world.y0 as f64, world.x1 as f64, world.y1 as f64).unwrap();
        request.raster.place_lattice = place;
        request.raster.survivor_list = list;
        request.layers.push(LayerStyle { layer_idx: 2, color: [255, 0, 0, 255], fill: LayerFill::Solid, outline_width: 1 });
        request
    }

    /// The placement lattice (CUT_DENSITY_DESIGN §10.8): a single shape placed
    /// by a lattice array ranks as the flat array of it - the same pixels for
    /// sub-pixel rectangles (width first) and polygons (area-true keep), the
    /// cell placed upright or rotated, with the survivor list on and off.
    #[test]
    fn a_placed_shape_ranks_as_its_flat_array_under_the_placement_lattice() {
        let world = BBox { x0: 0, y0: 0, x1: 320, y1: 320 };
        let rect = |x, y, w, h, rep: Rep| RectRec { layer: 1, dt: 0, x, y, w, h, rep };
        let poly = |pts: Vec<(i64, i64)>, rep: Rep| PolyRec { layer: 1, dt: 0, pts, rep };
        let grid = Rep::Grid { na: 40, nb: 3, va: (7, 0), vb: (0, 90) };
        // 5 world units a pixel: a 1-unit bar is 0.2 px wide, the triangle's
        // keep chance 0.1
        let tri = |dx: i64, dy: i64| vec![(dx, dy), (dx + 1, dy), (dx, dy + 40)];
        let flat_rect = hairline_scene(vec![rect(3, 5, 1, 60, grid.clone())], Vec::new(), Vec::new());
        let flat_poly = hairline_scene(Vec::new(), vec![poly(tri(3, 5), grid.clone())], Vec::new());
        // the bar stored upright, and lying down in a cell placed a quarter turn
        let placed_rect = placed_scene(vec![(1, vec![rect(0, 0, 1, 60, Rep::One)], Vec::new())], (3, 5, 0, false, grid.clone()), world);
        let turned_rect = placed_scene(vec![(1, vec![rect(0, -1, 60, 1, Rep::One)], Vec::new())], (3, 5, 1, false, grid.clone()), world);
        let placed_poly = placed_scene(vec![(1, Vec::new(), vec![poly(tri(0, 0), Rep::One)])], (3, 5, 0, false, grid.clone()), world);
        for list in [false, true] {
            let on = lattice_request(world, 64, DEFAULT_TILE_SIZE, 1, true, list);
            let flat = render_geometry_styled(&flat_rect, &on).unwrap().frame;
            assert!(!lit_set(&flat, 64).is_empty());
            assert_eq!(render_geometry_styled(&placed_rect, &on).unwrap().frame, flat, "placed bar, list {}", list);
            assert_eq!(render_geometry_styled(&turned_rect, &on).unwrap().frame, flat, "turned bar, list {}", list);
            let flat = render_geometry_styled(&flat_poly, &on).unwrap().frame;
            let lit = lit_set(&flat, 64).len();
            assert!(lit > 0, "the triangles light nothing");
            assert_eq!(render_geometry_styled(&placed_poly, &on).unwrap().frame, flat, "placed triangle, list {}", list);
        }
        // off: the flat array keeps its own ranks, the placed shapes their hashes
        let off = lattice_request(world, 64, DEFAULT_TILE_SIZE, 1, false, true);
        assert_eq!(
            render_geometry_styled(&flat_rect, &off).unwrap().frame,
            render_geometry_styled(&flat_rect, &lattice_request(world, 64, DEFAULT_TILE_SIZE, 1, true, true)).unwrap().frame,
            "a flat rectangle array ranks on its lattice either way"
        );
    }

    /// The placement array's survivor walk draws what every member's visit
    /// draws: list on and off, the work bin and the per-tile walk, two tilings,
    /// one and three workers - byte-identical, with fewer cells visited; a
    /// cell whose other drawn layer holds a shape a pixel wide is walked member
    /// by member, and its sub-pixel shapes still light the pixels they light
    /// without that shape (the ranks do not depend on the walk).
    #[test]
    fn the_placement_survivor_walk_draws_what_every_member_draws() {
        // 25 world units a pixel: the bars are 0.04 px wide, the triangle's
        // keep chance 0.02; 400 x 2 members 0.16 px apart
        let world = BBox { x0: -200, y0: -200, x1: 1400, y1: 1400 };
        let rect = |layer, x, y, w, h| RectRec { layer, dt: 0, x, y, w, h, rep: Rep::One };
        let tri = PolyRec { layer: 1, dt: 0, pts: vec![(2, 0), (3, 0), (2, 150)], rep: Rep::One };
        let small = (1, vec![rect(1, 0, 0, 1, 250), rect(1, 1, 60, 1, 150)], vec![tri]);
        let big = (2, vec![rect(2, 0, 400, 200, 200)], Vec::new());
        let grid = Rep::Grid { na: 400, nb: 2, va: (4, 0), vb: (0, 700) };
        for (x, y, rot, flip) in [(-50, 10, 0u8, false), (-40, 1100, 2, true)] {
            let scene = placed_scene(vec![small.clone()], (x, y, rot, flip, grid.clone()), world);
            let reference = render_geometry_styled_unbinned(&scene, &lattice_request(world, 64, DEFAULT_TILE_SIZE, 1, true, false)).unwrap();
            assert!(!lit_set(&reference.frame, 64).is_empty());
            let mut walked = None;
            for (tile, workers) in [(DEFAULT_TILE_SIZE, 1u16), (16, 3)] {
                for list in [false, true] {
                    let request = lattice_request(world, 64, tile, workers, true, list);
                    let bin = render_geometry_styled(&scene, &request).unwrap();
                    let walk = render_geometry_styled_unbinned(&scene, &request).unwrap();
                    assert_eq!(bin.frame, reference.frame, "bin, tile {} workers {} list {} at {:?}", tile, workers, list, (x, y, rot, flip));
                    assert_eq!(walk.frame, reference.frame, "walk, tile {} workers {} list {} at {:?}", tile, workers, list, (x, y, rot, flip));
                    if tile == DEFAULT_TILE_SIZE {
                        walked.get_or_insert([0u64; 2])[list as usize] = bin.stats.hier_cells_visited;
                    }
                }
            }
            let [every, listed] = walked.unwrap();
            assert!(listed * 2 < every, "the walk visited {} cells of {}", listed, every);
            // a drawn shape a pixel wide on layer 2: member by member, the
            // same layer-1 pixels as without it
            let with_big = placed_scene(vec![small.clone(), big.clone()], (x, y, rot, flip, grid.clone()), world);
            let request = lattice_request(world, 64, DEFAULT_TILE_SIZE, 1, true, true);
            let both = render_geometry_styled(&with_big, &request).unwrap();
            let every_member = render_geometry_styled(&with_big, &lattice_request(world, 64, DEFAULT_TILE_SIZE, 1, true, false)).unwrap();
            assert_eq!(both.frame, every_member.frame);
            assert_eq!(both.stats.hier_cells_visited, every_member.stats.hier_cells_visited, "a pixel-wide shape leaves every member to the walk");
            let white = |frame: &RgbaFrame| {
                let mut set = BTreeSet::new();
                for row in 0..64 {
                    for col in 0..64 {
                        if pixel(frame, col, row) == [255, 255, 255, 255] {
                            set.insert((col, row));
                        }
                    }
                }
                set
            };
            assert_eq!(white(&both.frame), white(&reference.frame), "the big shape changed the small shapes' picks at {:?}", (x, y, rot, flip));
        }
    }

    /// The preparation of a placement array's survivor walk is bounded
    /// (review 2026-09-25): the records it looks at - those the shape cut
    /// drops included - and the polygon vertices count against
    /// PLACEMENT_PREP_WORK, and a ninth drawn shape gives up at once. Past a
    /// bound the members are visited one by one, the pixels unchanged.
    #[test]
    fn the_placement_walk_preparation_is_bounded() {
        let world = BBox { x0: -200, y0: -200, x1: 1400, y1: 1400 };
        let thin = |x: i64| RectRec { layer: 1, dt: 0, x, y: 0, w: 1, h: 250, rep: Rep::One };
        // 1 x 1 unit specks under the 75-unit (3 px) cut on both sides
        let specks = |n: i64| (0..n).map(|k| RectRec { layer: 1, dt: 0, x: (k % 50) * 3, y: 300 + (k / 50) * 3, w: 1, h: 1, rep: Rep::One });
        // a 1..2 unit wide comb 2 x `teeth` units tall: 2 teeth + 2 vertices
        let comb = |teeth: i64| {
            let mut pts = vec![(0, 0), (0, 2 * teeth)];
            for k in (0..teeth).rev() {
                pts.push((2, 2 * k + 2));
                pts.push((1, 2 * k + 1));
            }
            PolyRec { layer: 1, dt: 0, pts, rep: Rep::One }
        };
        let cut = HierStats { shape_cut: 75, shape_cut_max: true, ..HierStats::default() };
        let grid = Rep::Grid { na: 400, nb: 2, va: (4, 0), vb: (0, 700) };
        // the visits with the list on and off, and the walk's outcome (the
        // array is 2-D: RenderStats::place_walks' second half)
        let visits = |rects: Vec<RectRec>, polys: Vec<PolyRec>| {
            let scene = placed_scene_cut(vec![(1, rects, polys)], (-50, 10, 0, false, grid.clone()), world, cut.clone());
            let on = render_geometry_styled(&scene, &lattice_request(world, 64, DEFAULT_TILE_SIZE, 1, true, true)).unwrap();
            let off = render_geometry_styled(&scene, &lattice_request(world, 64, DEFAULT_TILE_SIZE, 1, true, false)).unwrap();
            assert_eq!(on.frame, off.frame);
            assert!(!lit_set(&on.frame, 64).is_empty());
            assert_eq!(off.stats.place_walks, [(0, 0); 32], "the list off plans no walk");
            let seen: Vec<PlaceWalkOutcome> = [
                PlaceWalkOutcome::Walked, PlaceWalkOutcome::Shapes, PlaceWalkOutcome::PrepWork,
            ]
            .into_iter()
            .filter(|&o| on.stats.place_walks[PLACE_WALK_OUTCOMES.len() + o as usize].0 > 0)
            .collect();
            (on.stats.hier_cells_visited, off.stats.hier_cells_visited, seen)
        };
        let walked = |(on, off, seen): (u64, u64, Vec<PlaceWalkOutcome>)| on * 2 < off && seen == [PlaceWalkOutcome::Walked];
        let declined = |(on, off, seen): (u64, u64, Vec<PlaceWalkOutcome>), why: PlaceWalkOutcome| on == off && seen == [why];
        // the cut's specks are no terms but count as work: 501 records walk
        assert!(walked(visits(std::iter::once(thin(0)).chain(specks(500)).collect(), Vec::new())));
        // 1,101 records are past the budget
        assert!(declined(visits(std::iter::once(thin(0)).chain(specks(1100)).collect(), Vec::new()), PlaceWalkOutcome::PrepWork));
        // eight drawn shapes walk, a ninth gives up
        assert!(walked(visits((0..8).map(|k| thin(4 * k)).collect(), Vec::new())));
        assert!(declined(visits((0..9).map(|k| thin(4 * k)).collect(), Vec::new()), PlaceWalkOutcome::Shapes));
        // a polygon's vertices count: 402 walk, 1,102 give up
        assert!(walked(visits(Vec::new(), vec![comb(200)])));
        assert!(declined(visits(Vec::new(), vec![comb(550)]), PlaceWalkOutcome::PrepWork));
    }

    /// The walk's outcomes as RenderStats::place_walks counts them, one
    /// placement array at a time: too small to pay (cost), a bar thin across
    /// the array's only axis (axis mismatch), a triangle a pixel wide (not
    /// sub-pixel), a path; 1-D arrays in the first half.
    #[test]
    fn placement_walk_outcomes_are_counted() {
        let world = BBox { x0: -200, y0: -200, x1: 1400, y1: 1400 };
        let outcome = |rects: Vec<RectRec>, polys: Vec<PolyRec>, rep: Rep| {
            let scene = placed_scene(vec![(1, rects, polys)], (-50, 10, 0, false, rep), world);
            let report = render_geometry_styled(&scene, &lattice_request(world, 64, DEFAULT_TILE_SIZE, 1, true, true)).unwrap();
            report
                .stats
                .place_walks
                .iter()
                .enumerate()
                .filter(|(_, walks)| walks.0 > 0)
                .map(|(k, walks)| (PLACE_WALK_OUTCOMES[k % 16], 1 + k / 16, walks.1))
                .collect::<Vec<_>>()
        };
        let thin = RectRec { layer: 1, dt: 0, x: 0, y: 0, w: 1, h: 250, rep: Rep::One };
        let lying = RectRec { layer: 1, dt: 0, x: 0, y: 0, w: 250, h: 1, rep: Rep::One };
        let wide = PolyRec { layer: 1, dt: 0, pts: vec![(0, 0), (100, 0), (0, 100)], rep: Rep::One };
        // a 0.8 px bar: its chance alone passes half the members
        let bold = RectRec { layer: 1, dt: 0, x: 0, y: 0, w: 20, h: 250, rep: Rep::One };
        assert_eq!(outcome(vec![bold], Vec::new(), Rep::Grid { na: 60, nb: 2, va: (30, 0), vb: (0, 700) }), vec![("cost", 2, 102)]);
        // even six members pay for a 0.04 px bar
        assert_eq!(outcome(vec![thin.clone()], Vec::new(), Rep::Grid { na: 3, nb: 2, va: (4, 0), vb: (0, 700) }), vec![("walked", 2, 6)]);
        assert_eq!(outcome(vec![thin.clone()], Vec::new(), Rep::Grid { na: 400, nb: 1, va: (4, 0), vb: (0, 0) }), vec![("walked", 1, 369)]);
        assert_eq!(outcome(vec![lying], Vec::new(), Rep::Grid { na: 400, nb: 1, va: (4, 0), vb: (0, 0) }), vec![("axis_mismatch", 1, 369)]);
        assert_eq!(outcome(Vec::new(), vec![wide], Rep::Grid { na: 400, nb: 1, va: (4, 0), vb: (0, 0) }), vec![("not_subpixel", 1, 369)]);
    }

    /// A placement array too large for the work bin is deferred; its tiles'
    /// mini walks (and their fallback) rank and list its members as the
    /// per-tile walk does.
    #[test]
    fn a_deferred_placement_array_draws_as_the_walk_under_the_placement_lattice() {
        let world = BBox { x0: 0, y0: 0, x1: 1600, y1: 1600 };
        let leaf = (1, vec![RectRec { layer: 1, dt: 0, x: 0, y: 0, w: 1, h: 30, rep: Rep::One }], Vec::new());
        let scene = placed_scene(vec![leaf], (0, 0, 0, false, Rep::Grid { na: 800, nb: 800, va: (2, 0), vb: (0, 2) }), world);
        let reference = render_geometry_styled_unbinned(&scene, &lattice_request(world, 32, 16, 1, true, false)).unwrap();
        assert!(!lit_set(&reference.frame, 32).is_empty());
        for list in [false, true] {
            let bin = render_geometry_styled(&scene, &lattice_request(world, 32, 16, 2, true, list)).unwrap();
            assert!(bin.stats.work_bin_defer_rep >= 1, "the array was not deferred");
            assert_eq!(bin.frame, reference.frame, "list {}", list);
        }
    }

    /// The sub-cut arrays diagnostic (CUT_DENSITY_DESIGN §10.9): under the
    /// hairline-keeping cut of 3 px, an array of 0.2 px dots and an array of
    /// sub-pixel triangles - both sides under the cut - are drawn as the same
    /// frame with no cut draws them (the area-true rule thins them by their
    /// size; polygon arrays keep by their lattice rank in this mode),
    /// byte-identical with the survivor list off; a single 1 px box under the
    /// cut stays cut and the box above it is the same.
    #[test]
    fn sub_cut_arrays_draw_their_members_by_their_size() {
        let rect = |x, y, w, h, rep: Rep| RectRec { layer: 1, dt: 0, x, y, w, h, rep };
        // 5 world units a pixel at 64 px: the cut is 15 units
        let dots = rect(0, 0, 1, 1, Rep::Grid { na: 50, nb: 50, va: (3, 0), vb: (0, 3) });
        let lone = rect(250, 250, 5, 5, Rep::One);
        let big = rect(200, 20, 60, 60, Rep::One);
        let tri = PolyRec { layer: 1, dt: 0, pts: vec![(0, 170), (2, 170), (0, 172)], rep: Rep::Grid { na: 50, nb: 20, va: (3, 0), vb: (0, 3) } };
        let records = || (vec![dots.clone(), lone.clone(), big.clone()], vec![tri.clone()]);
        let (rects, polys) = records();
        let cut = shape_cut_max_scene(rects, polys, Vec::new(), 15);
        let (rects, polys) = records();
        let uncut = shape_cut_max_scene(rects, polys, Vec::new(), 0);
        let request = |arrays: bool, list: bool| {
            let mut r = area_true_request(64, DEFAULT_TILE_SIZE, 1);
            r.raster.sub_cut_arrays = arrays;
            r.raster.survivor_list = list;
            r
        };
        let lit = |frame: &RgbaFrame, x0: usize, y0: usize, x1: usize, y1: usize| {
            lit_set(frame, 64).into_iter().filter(|&(c, r)| c >= x0 && c < x1 && r >= y0 && r < y1).collect::<BTreeSet<_>>()
        };
        // regions in device px (y down): the dots x 0..30, y 34..64; the
        // triangles x 0..30, y 17..30; the lone box x 50..51, y 13..14
        let max = render_geometry_styled(&cut, &request(false, true)).unwrap().frame;
        let arrays = render_geometry_styled(&cut, &request(true, true)).unwrap().frame;
        let every = render_geometry_styled(&cut, &request(true, false)).unwrap().frame;
        // the uncut frame under the same rule (polygon arrays keep by their
        // lattice rank in this mode): nothing is under its cut
        let none = render_geometry_styled(&uncut, &request(true, true)).unwrap().frame;
        assert_eq!(arrays, every, "the survivor list changed the sub-cut arrays");
        assert!(lit(&max, 0, 17, 31, 64).is_empty(), "max drew a sub-cut array");
        let dots_lit = lit(&arrays, 0, 34, 31, 64);
        assert!(!dots_lit.is_empty() && dots_lit == lit(&none, 0, 34, 31, 64), "the dots differ from the uncut frame");
        let tri_lit = lit(&arrays, 0, 17, 31, 31);
        assert!(!tri_lit.is_empty() && tri_lit == lit(&none, 0, 17, 31, 31), "the triangles differ from the uncut frame");
        assert!(lit(&arrays, 49, 12, 53, 16).is_empty() && !lit(&none, 49, 12, 53, 16).is_empty(), "the lone sub-cut box");
        assert_eq!(lit(&arrays, 39, 47, 53, 61), lit(&max, 39, 47, 53, 61), "the box above the cut");
    }

    #[test]
    fn duplicates_of_a_shape_draw_as_one() {
        // ADAPTIVE_CUT_DENSITY_PLAN §4.2 (review 2026-09-23): the same world
        // box stored twice on the same path - two single rectangles, a Grid
        // twice, a Grid and a fragment of it - lights exactly the pixels one
        // copy lights: the decision is the world box's (or the lattice's),
        // never the record's or the paint order's. A single rectangle over
        // a Grid member is another path (its own world-box hash), so the
        // pair lights the union of the two decisions - the wider one, since
        // both are centred on the same box - and nothing outside it.
        let request = area_true_request(32, DEFAULT_TILE_SIZE, 1);
        let draw = |rects: Vec<RectRec>| {
            lit_set(&render_geometry_styled(&hairline_scene(rects, Vec::new(), Vec::new()), &request).unwrap().frame, 32)
        };
        let rect = |x, y, w, h, rep: Rep| RectRec { layer: 1, dt: 0, x, y, w, h, rep };
        // 40 single 1.5 x 1.5 px boxes at scattered places (10 units a pixel)
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = |span: i64| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as i64 % span
        };
        let singles: Vec<RectRec> = (0..40).map(|_| rect(next(300), next(300), 15, 15, Rep::One)).collect();
        let once = draw(singles.clone());
        assert!(once.len() > 40, "{} px for 40 boxes", once.len());
        let mut twice = singles.clone();
        twice.extend(singles.iter().cloned());
        assert_eq!(draw(twice), once, "a duplicate single rectangle changed the pixels");
        let mut reversed = singles.clone();
        reversed.reverse();
        assert_eq!(draw(reversed), once, "the record order changed the pixels");
        // a row of 8 bars 1.5 px wide at 3 px: itself twice, and with a
        // fragment of itself (members 3..6, re-based as the index would)
        let row = rect(13, 50, 15, 200, Rep::Grid { na: 8, nb: 1, va: (30, 0), vb: (0, 0) });
        let lattice = draw(vec![row.clone()]);
        assert_eq!(draw(vec![row.clone(), row.clone()]), lattice, "a duplicate Grid changed the pixels");
        let piece = rect(13 + 3 * 30, 50, 15, 200, Rep::Grid { na: 3, nb: 1, va: (30, 0), vb: (0, 0) });
        assert_eq!(draw(vec![row.clone(), piece]), lattice, "a fragment over its Grid changed the pixels");
        // a single rectangle over member 5: another path - the pair lights
        // the union of the two decisions, at most 2 px wide, nothing else
        let member = rect(13 + 5 * 30, 50, 15, 200, Rep::One);
        let alone = draw(vec![member.clone()]);
        let pair = draw(vec![row.clone(), member]);
        assert_eq!(pair, lattice.union(&alone).cloned().collect(), "the pair drew outside its two decisions");
        let cols = |set: &BTreeSet<(usize, usize)>| set.iter().map(|&(col, _)| col).filter(|&c| (15..=18).contains(&c)).collect::<BTreeSet<_>>();
        assert!(cols(&pair).len() <= 2, "member 5 wider than 2 px: {:?}", cols(&pair));
        assert!(cols(&pair).is_superset(&cols(&lattice)));
    }

    #[test]
    fn width_first_keeps_a_sub_pixel_rectangle_with_the_chance_it_fills_its_pixel() {
        // many 0.3 px wires and 0.4 x 0.5 px points at scattered world boxes:
        // the kept share is the covered share (independent axes), the decision
        // is the world box's, and zooming out keeps a subset
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as i64
        };
        let one = DEVICE_ONE as i64;
        let (mut kept_wires, mut kept_points, n) = (0usize, 0usize, 40_000usize);
        for _ in 0..n {
            let (x, y) = (next() % 1_000_000, next() % 1_000_000);
            let wire = BBox { x0: x, y0: y, x1: x + (0.3 * one as f64) as i64, y1: y + 5 * one };
            let point = BBox { x0: x, y0: y, x1: x + (0.4 * one as f64) as i64, y1: y + (0.5 * one as f64) as i64 };
            for (b, kept) in [(&wire, &mut kept_wires), (&point, &mut kept_points)] {
                // one world unit = one device unit here, and half of it zoomed out
                let draw = |k: i128| {
                    let sx = width_first_span(b.x0 as i128 / k, b.x1 as i128 / k, salted_rank(*b, 1));
                    let sy = width_first_span(b.y0 as i128 / k, b.y1 as i128 / k, salted_rank(*b, 2));
                    sx.zip(sy)
                };
                match draw(1) {
                    Some(((a0, a1), _)) => {
                        *kept += 1;
                        assert_eq!(a1 - a0, 1, "one pixel across the thin side");
                    }
                    None => assert!(draw(2).is_none(), "a shape dropped when near came back when zoomed out"),
                }
            }
        }
        let share = |k: usize| k as f64 / n as f64;
        assert!((share(kept_wires) - 0.3).abs() < 0.015, "wires kept {}", share(kept_wires));
        assert!((share(kept_points) - 0.2).abs() < 0.015, "points kept {}", share(kept_points));
    }

    #[test]
    fn area_true_draws_no_rim_where_a_shape_runs_off_the_view() {
        // review 2026-09-22: a polygon around the whole view lit its top row -
        // the scan stopped at row 0, so the row above looked empty. With the
        // fill cleared only a rim can light a pixel, and there is none inside
        let big_poly = vec![(-200, -150), (600, -170), (650, 520), (-230, 480)];
        let request = |fill: LayerFill, dx: f64, dy: f64| {
            let mut request = area_true_request(32, 7, 3);
            request.raster.view = RasterViewBox::new(dx, dy, 320.0 + dx, 320.0 + dy).unwrap();
            request.layers[0].fill = fill;
            request
        };
        let polys = vec![PolyRec { layer: 1, dt: 0, pts: big_poly, rep: Rep::One }];
        let rects = vec![RectRec { layer: 1, dt: 0, x: -170, y: -190, w: 700, h: 690, rep: Rep::One }];
        for (rects, polys) in [(rects.clone(), Vec::new()), (Vec::new(), polys.clone())] {
            let scene = hairline_scene(rects, polys, Vec::new());
            for (dx, dy) in [(0.0, 0.0), (10.0, 0.0), (0.0, -10.0), (-10.0, 10.0)] {
                let frame = render_geometry_styled(&scene, &request(LayerFill::Clear, dx, dy)).unwrap().frame;
                assert!(lit_set(&frame, 32).is_empty(), "a rim inside the shape at pan ({}, {})", dx, dy);
                let solid = render_geometry_styled(&scene, &request(LayerFill::Solid, dx, dy)).unwrap().frame;
                assert_eq!(lit_set(&solid, 32).len(), 32 * 32);
            }
        }
    }

    #[test]
    fn area_true_keeps_a_sub_pixel_polygon_by_its_own_area() {
        // review 2026-09-22: 0.8 x 0.8 px triangles lit as many pixels as the
        // squares of the same box. 900 of each, 3 px apart (no two share a pixel)
        let (mut squares, mut triangles) = (Vec::new(), Vec::new());
        for j in 0..30i64 {
            for i in 0..30i64 {
                let (x, y) = (10 + i * 30 + (j * 7) % 11, 10 + j * 30 + (i * 5) % 13);
                squares.push(PolyRec { layer: 1, dt: 0, pts: vec![(x, y), (x + 8, y), (x + 8, y + 8), (x, y + 8)], rep: Rep::One });
                triangles.push(PolyRec { layer: 1, dt: 0, pts: vec![(x, y), (x + 8, y), (x, y + 8)], rep: Rep::One });
            }
        }
        let mut request = area_true_request(96, DEFAULT_TILE_SIZE, 1);
        request.raster.view = RasterViewBox::new(0.0, 0.0, 960.0, 960.0).unwrap();
        let lit = |polys: Vec<PolyRec>| {
            lit_set(&render_geometry_styled(&hairline_scene(Vec::new(), polys, Vec::new()), &request).unwrap().frame, 96).len()
        };
        let (square, triangle) = (lit(squares), lit(triangles));
        // expected 900 x 0.64 = 576 and 288
        assert!((square as f64 - 576.0).abs() < 60.0, "squares lit {}", square);
        assert!((triangle as f64 - 288.0).abs() < 45.0, "triangles lit {}", triangle);
    }

    #[test]
    fn area_true_pixels_do_not_depend_on_the_tiling() {
        // the rim reads the rows next to a band, so no tile or worker split
        // may change a pixel; sub-pixel ranks are world-anchored
        let rect = |x, y, w, h, rep| RectRec { layer: 1, dt: 0, x, y, w, h, rep };
        let rects = vec![
            rect(7, 9, 131, 47, Rep::One),
            rect(150, 150, 3, 90, Rep::Grid { na: 40, nb: 1, va: (4, 0), vb: (0, 0) }),
            rect(20, 200, 2, 2, Rep::Grid { na: 30, nb: 30, va: (3, 0), vb: (0, 3) }),
            rect(233, 17, 61, 29, Rep::Grid { na: 2, nb: 5, va: (33, 0), vb: (0, 41) }),
        ];
        let polys = vec![PolyRec { layer: 1, dt: 0, pts: vec![(141, 61), (311, 97), (253, 303), (171, 211)], rep: Rep::One }];
        let paths = vec![PathRec { layer: 1, dt: 0, pts: vec![(15, 120), (120, 120), (120, 190)], hw: 6, es: 3, ee: 0, rep: Rep::One }];
        let scene = hairline_scene(rects, polys, paths);
        let reference = render_geometry_styled(&scene, &area_true_request(96, DEFAULT_TILE_SIZE, 1)).unwrap().frame;
        assert!(!lit_set(&reference, 96).is_empty());
        for (tile, workers) in [(7u16, 1u16), (16, 3), (5, 4)] {
            let frame = render_geometry_styled(&scene, &area_true_request(96, tile, workers)).unwrap().frame;
            assert_eq!(frame, reference, "tile {} workers {}", tile, workers);
        }
    }

    #[test]
    fn the_shape_cut_drops_the_shapes_whose_smaller_side_is_under_it() {
        // 10 units a pixel. Large: a 100 x 100 rect and a 60 x 60 polygon.
        // Small on one side or both: an array of 20 x 20 rects, a 200 x 20
        // wire, a 20 x 20 polygon and a path 10 wide and 200 long.
        let rect = |x, y, w, h, rep| RectRec { layer: 1, dt: 0, x, y, w, h, rep };
        let large_rects = vec![rect(10, 10, 100, 100, Rep::One)];
        let large_polys = vec![PolyRec { layer: 1, dt: 0, pts: vec![(200, 20), (260, 20), (260, 80), (200, 80)], rep: Rep::One }];
        let mut rects = large_rects.clone();
        rects.push(rect(150, 150, 20, 20, Rep::Grid { na: 3, nb: 3, va: (40, 0), vb: (0, 40) }));
        rects.push(rect(10, 280, 200, 20, Rep::One));
        let mut polys = large_polys.clone();
        polys.push(PolyRec { layer: 1, dt: 0, pts: vec![(280, 280), (300, 280), (290, 300)], rep: Rep::One });
        let paths = vec![PathRec { layer: 1, dt: 0, pts: vec![(20, 240), (220, 240)], hw: 5, es: 0, ee: 0, rep: Rep::One }];
        let request = hairline_request();
        let frame = |scene: &FrameScene| render_geometry_styled(scene, &request).unwrap().frame;
        let everything = frame(&shape_cut_scene(rects.clone(), polys.clone(), paths.clone(), 0));
        let large_only = frame(&shape_cut_scene(large_rects, large_polys, Vec::new(), 0));
        assert_ne!(everything, large_only, "the small shapes are drawn without the cut");
        // a cut of 30: what is 20 or 10 on its smaller side goes, however long
        assert_eq!(frame(&shape_cut_scene(rects.clone(), polys.clone(), paths.clone(), 30)), large_only);
        // a smaller side equal to the cut is not under it
        assert_eq!(frame(&shape_cut_scene(rects.clone(), polys.clone(), paths.clone(), 10)), everything);
        // the cut is per shape: above the large shapes too, nothing is left
        let blank = frame(&shape_cut_scene(Vec::new(), Vec::new(), Vec::new(), 0));
        assert_eq!(frame(&shape_cut_scene(rects, polys, paths, 101)), blank);
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
                        page_levels: Vec::new(),
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
                        reps: Vec::new(),
                    },
                    WsCell {
                        key: child,
                        pages: vec![0],
                        page_levels: Vec::new(),
                        insts: Vec::new(),
                        frames: vec![(unit, Rep::One, 1)],
                        washes: Vec::new(),
                        reps: Vec::new(),
                    },
                ],
                pages: vec![0],
                page_prio: vec![0],
                stats: HierStats::default(),
                            explain: Vec::new(),
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
                        page_levels: Vec::new(),
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
                        reps: Vec::new(),
                    },
                    WsCell {
                        key: mid,
                        pages: Vec::new(),
                        page_levels: Vec::new(),
                        insts: leaf_insts,
                        frames: Vec::new(),
                        washes: Vec::new(),
                        reps: Vec::new(),
                    },
                    WsCell {
                        key: leaf,
                        pages: vec![0],
                        page_levels: Vec::new(),
                        insts: Vec::new(),
                        frames: Vec::new(),
                        washes: Vec::new(),
                        reps: Vec::new(),
                    },
                ],
                pages: vec![0],
                page_prio: vec![0],
                stats: HierStats::default(),
                            explain: Vec::new(),
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
                        page_levels: Vec::new(),
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
                        reps: Vec::new(),
                    },
                    WsCell {
                        key: mid,
                        pages: Vec::new(),
                        page_levels: Vec::new(),
                        insts: leaf_insts,
                        frames: Vec::new(),
                        washes: Vec::new(),
                        reps: Vec::new(),
                    },
                    WsCell {
                        key: leaf,
                        pages: vec![0, 1],
                        page_levels: Vec::new(),
                        insts: Vec::new(),
                        frames: vec![(unit, Rep::One, 1)],
                        washes: Vec::new(),
                        reps: Vec::new(),
                    },
                ],
                pages: vec![0, 1],
                page_prio: vec![0, 0],
                stats: HierStats::default(),
                            explain: Vec::new(),
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
                        page_levels: Vec::new(),
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
                        reps: Vec::new(),
                    },
                    WsCell {
                        key: mid,
                        pages: Vec::new(),
                        page_levels: Vec::new(),
                        insts: leaf_insts,
                        frames: Vec::new(),
                        washes: Vec::new(),
                        reps: Vec::new(),
                    },
                    WsCell {
                        key: leaf,
                        pages: vec![0],
                        page_levels: Vec::new(),
                        insts: Vec::new(),
                        frames: vec![(unit, Rep::One, 1)],
                        washes: Vec::new(),
                        reps: Vec::new(),
                    },
                ],
                pages: vec![0],
                page_prio: vec![0],
                stats: HierStats::default(),
                            explain: Vec::new(),
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
        // the walk's visit count is the ordered overwrite's: a write-once
        // tile that fills up stops its walk early, which is not what this
        // test measures
        let (walk, bin) = with_write_once(false, || {
            (
                render_geometry_styled_unbinned(&make_scene(), &request).unwrap(),
                render_geometry_styled(&make_scene(), &request).unwrap(),
            )
        });
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

    /// The label-free frame a caller would retain (§F2R-20): the copy
    /// when labels painted over the frame, else the frame itself.
    fn retained_geometry(report: &GeometryRasterReport) -> &RgbaFrame {
        if report.geometry_is_frame {
            &report.frame
        } else {
            report.geometry_frame.as_ref().expect("geometry frame kept")
        }
    }

    #[test]
    fn pan_reuse_matches_a_full_render_at_the_16px_snap() {
        // §F2R-16: views A and B differ by exactly 16 device pixels
        // (the fill-phase period), so tiles copied from A's shifted
        // geometry frame must be byte-identical to a cold render of B
        // - speckle parity, stipple phase, frames, and the on-top
        // label pass included.
        let top = (0, REM_FULL);
        let world = BBox {
            x0: 0,
            y0: 0,
            x1: 640,
            y1: 320,
        };
        let make_scene = || {
            let plan = HierPlan {
                top,
                wcells: vec![WsCell {
                    key: top,
                    pages: vec![0, 1],
                    page_levels: Vec::new(),
                    insts: Vec::new(),
                    frames: vec![(
                        BBox {
                            x0: 40,
                            y0: 40,
                            x1: 600,
                            y1: 280,
                        },
                        Rep::One,
                        1,
                    )],
                    washes: Vec::new(),
                    reps: Vec::new(),
                }],
                pages: vec![0, 1],
                page_prio: vec![0, 0],
                stats: HierStats::default(),
                            explain: Vec::new(),
            };
            FrameScene::from_test_parts(
                plan,
                vec![
                    styled_page(
                        0,
                        1,
                        BBox {
                            x0: 5,
                            y0: 5,
                            x1: 610,
                            y1: 200,
                        },
                    ),
                    styled_page(
                        1,
                        2,
                        BBox {
                            x0: 100,
                            y0: 120,
                            x1: 540,
                            y1: 310,
                        },
                    ),
                ],
                BTreeMap::from([(top, world)]),
            )
            .unwrap()
        };
        let styled = |x0: f64| StyledGeometryRasterRequest {
            raster: GeometryRasterRequest {
                view: RasterViewBox::new(x0, 0.0, x0 + 320.0, 320.0).unwrap(),
                width: 32,
                height: 32,
                workers: 2,
                tile_size: 8,
                ..request()
            },
            layers: vec![
                LayerStyle {
                    layer_idx: 1,
                    color: [40, 200, 90, 255],
                    fill: LayerFill::Speckle,
                    outline_width: 1,
                },
                LayerStyle {
                    layer_idx: 2,
                    color: [220, 80, 40, 255],
                    fill: LayerFill::Pattern([0x8421; 16]),
                    outline_width: 2,
                },
            ],
            hierarchy_frames: true,
            mono: false,
        };
        let cancellation = RenderCancellation::new();
        let scene = make_scene();
        let a = render_geometry_styled_cancellable_reuse(
            &scene,
            &styled(0.0),
            1,
            &cancellation,
            None,
            true,
        )
        .unwrap();
        let b_full = render_geometry_styled_cancellable_reuse(
            &scene,
            &styled(160.0),
            1,
            &cancellation,
            None,
            true,
        )
        .unwrap();
        // Shift A's geometry frame left by 16 px: B[x, y] = A[x+16, y].
        // §F2R-20: this scene carries no labels, so the raster reports
        // the published frame itself as the geometry (no copy made)
        assert!(a.geometry_is_frame, "label-free render keeps no copy");
        assert!(a.geometry_frame.is_none());
        let geometry_a = retained_geometry(&a);
        let mut base = vec![0u8; 32 * 32 * 4];
        for row in 0..32usize {
            for col in 0..16usize {
                let source = (row * 32 + col + 16) * 4;
                let target = (row * 32 + col) * 4;
                base[target..target + 4]
                    .copy_from_slice(&geometry_a.pixels()[source..source + 4]);
            }
        }
        let reuse = FrameReuse {
            base: RgbaFrame::from_pixels(32, 32, base).unwrap(),
            valid: [0, 0, 16, 32],
        };
        let b_reused = render_geometry_styled_cancellable_reuse(
            &scene,
            &styled(160.0),
            1,
            &cancellation,
            Some(&reuse),
            true,
        )
        .unwrap();
        assert_eq!(b_reused.stats.tiles_reused, 8, "2 columns x 4 rows");
        assert_eq!(b_reused.frame.pixels(), b_full.frame.pixels());
        assert_eq!(
            retained_geometry(&b_reused).pixels(),
            retained_geometry(&b_full).pixels()
        );
        assert!(b_full.stats.tiles_reused == 0);
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
                        page_levels: Vec::new(),
                        insts: vec![inst(child_a, 2), inst(child_b, 4)],
                        frames: Vec::new(),
                        washes: Vec::new(),
                        reps: Vec::new(),
                    },
                    WsCell {
                        key: child_a,
                        pages: Vec::new(),
                        page_levels: Vec::new(),
                        insts: Vec::new(),
                        frames: vec![(unit, Rep::One, 1)],
                        washes: Vec::new(),
                        reps: Vec::new(),
                    },
                    WsCell {
                        key: child_b,
                        pages: vec![0],
                        page_levels: Vec::new(),
                        insts: Vec::new(),
                        frames: Vec::new(),
                        washes: Vec::new(),
                        reps: Vec::new(),
                    },
                ],
                pages: vec![0],
                page_prio: vec![0],
                stats: HierStats::default(),
                            explain: Vec::new(),
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
                    page_levels: Vec::new(),
                    insts: vec![inst(child)],
                    frames: Vec::new(),
                    washes: Vec::new(),
                    reps: Vec::new(),
                },
                WsCell {
                    key: child,
                    pages: Vec::new(),
                    page_levels: Vec::new(),
                    insts: vec![inst(top)],
                    frames: Vec::new(),
                    washes: Vec::new(),
                    reps: Vec::new(),
                },
            ],
            pages: vec![0],
            page_prio: vec![0],
            stats: HierStats::default(),
                    explain: Vec::new(),
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

    /// jobdeck step 2: a windowed render is the full render's window,
    /// byte for byte, whatever the tile size - and its frame is the
    /// window's size.
    #[test]
    fn windowed_render_is_the_crop_of_the_full_render() {
        let scene = styled_scene(vec![
            (BBox { x0: 3, y0: 3, x1: 7, y1: 7 }, Rep::One, 0),
            (BBox { x0: 0, y0: 8, x1: 2, y1: 10 }, Rep::One, 3),
        ]);
        let cancellation = RenderCancellation::new();
        for tile_size in [2u16, 3, 10] {
            let raster = GeometryRasterRequest {
                tile_size,
                ..request()
            };
            let styled = StyledGeometryRasterRequest {
                raster,
                layers: vec![LayerStyle {
                    layer_idx: 0,
                    color: [255, 0, 0, 255],
                    fill: LayerFill::Speckle,
                    outline_width: 1,
                }],
                hierarchy_frames: true,
                mono: false,
            };
            let full = render_geometry_styled(&scene, &styled).unwrap();
            for window in [[2u32, 3, 8, 9], [0, 0, 10, 10], [7, 0, 10, 4]] {
                let part = render_geometry_styled_cancellable_windowed(
                    &scene,
                    &styled,
                    1,
                    &cancellation,
                    window,
                )
                .unwrap();
                let [c0, r0, c1, r1] = window;
                assert_eq!((part.frame.width(), part.frame.height()), (c1 - c0, r1 - r0));
                for y in r0..r1 {
                    for x in c0..c1 {
                        assert_eq!(
                            pixel(&part.frame, (x - c0) as usize, (y - r0) as usize),
                            pixel(&full.frame, x as usize, y as usize),
                            "tile {tile_size} window {window:?} at {x},{y}"
                        );
                    }
                }
            }
        }
        assert!(render_geometry_styled_cancellable_windowed(
            &scene,
            &StyledGeometryRasterRequest {
                raster: request(),
                layers: Vec::new(),
                hierarchy_frames: false,
                mono: false,
            },
            1,
            &cancellation,
            [5, 5, 12, 6],
        )
        .is_err(), "a window past the frame is refused");
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
                    page_levels: Vec::new(),
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
                    reps: Vec::new(),
                },
                WsCell {
                    key: child,
                    pages: vec![0],
                    page_levels: Vec::new(),
                    insts: Vec::new(),
                    frames: Vec::new(),
                    washes: Vec::new(),
                    reps: Vec::new(),
                },
            ],
            pages: vec![0],
            page_prio: vec![0],
            stats: HierStats::default(),
                    explain: Vec::new(),
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
            area_true: false,
            width_c: 1.0,
            survivor_list: true,
            place_lattice: false,
            sub_cut_arrays: false,
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
            area_true: false,
            width_c: 1.0,
            survivor_list: true,
            place_lattice: false,
            sub_cut_arrays: false,
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
        // §F2R-16: labels paint once in a full-frame pass, so the tile
        // grid no longer multiplies label work.
        assert_eq!(parallel.label_tile_paints, serial.label_tile_paints);
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

#[cfg(test)]
mod hull_parity_tests {
    /// The occupancy builder (docs/OCCUPANCY_PLAN.ko.md M1) marks the
    /// hull floe-tiler computes; it has to be the hull this raster
    /// paints. render-core keeps its checked copy for the pinned
    /// goldens, so the two are pinned equal here instead.
    #[test]
    fn tiler_path_outline_any_matches_the_raster_hull() {
        let spines: Vec<(Vec<(i64, i64)>, i64, i64, i64)> = vec![
            (vec![(0, 0), (100, 0), (100, 80)], 5, 0, 0),
            (vec![(0, 0), (100, 0), (100, 80)], 5, 3, -2),
            (vec![(0, 0), (60, 70)], 4, 0, 0),
            (vec![(0, 0), (50, 50), (100, 0)], 6, 2, 2),
            (vec![(0, 0), (80, 10), (160, 0), (240, 30)], 3, 0, 0),
            (vec![(0, 0), (50, 50), (60, 0)], 7, 0, 0),
            (vec![(0, 0), (0, 0), (50, 50), (100, 100), (150, 90)], 5, 1, 1),
            (vec![(10, 10), (10, 60), (40, 60), (40, 20)], 4, 2, 0),
        ];
        for (pts, hw, es, ee) in spines {
            let ours = super::checked_path_outline(&pts, hw, es, ee);
            let theirs = floe_tiler::path_outline_any(&pts, hw, es, ee);
            assert_eq!(ours, theirs, "spine {:?}", pts);
        }
        for pts in [
            vec![(0, 0)],
            vec![(0, 0), (50, 50), (0, 0)],
            vec![(0, 0), (10, 0), (0, 0)],
        ] {
            assert_eq!(
                super::checked_path_outline(&pts, 3, 0, 0).is_err(),
                floe_tiler::path_outline_any(&pts, 3, 0, 0).is_err(),
                "{:?}",
                pts
            );
        }
    }
}
