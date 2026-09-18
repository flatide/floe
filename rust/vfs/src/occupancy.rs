//! Occupancy pyramid (design.ovo) - docs/OCCUPANCY_PLAN.ko.md M1.
//!
//! Per source layer and placement depth (0 = the top cell's own
//! records; one bit plane per depth since 2026-09-16, so a limited
//! depth draws its own summary), one bit per grid cell of the
//! flattened top cell: 1 when the cell's OPEN box meets a shape's interior (a positive-area
//! intersection - the KLayout `Region & box` reading), pooled by OR
//! into 2x levels until the grid is at most TOP_GRID on both axes.
//! Built from geometry, never from a bbox (review 2026-09-11 P1-1):
//!   * rects mark their cell range; an axis-aligned Grid repetition
//!     whose gaps are narrower than a cell marks its footprint in one
//!     go (exactly the per-member result), any other repetition marks
//!     every member;
//!   * polygons take a conservative cell-grid scan conversion: every
//!     cell an edge passes through (open box), then the cells whose
//!     centre is inside by even-odd parity - a cell no edge crosses is
//!     wholly inside or wholly outside, so the centre decides exactly;
//!   * paths become the hull the raster paints (`path_outline_any`)
//!     and take the polygon rule;
//!   * zero-area shapes (w or h 0, zero-width paths, degenerate
//!     polygons) mark nothing, as KLayout's region would be empty.
//! Limits are per layer: a work budget (cells marked + members and
//! edge rows visited) and a file-size budget; a layer over a budget is
//! recorded as `none:<reason>` and never as an approximation. A grid
//! over the cell budget records every layer as `none:cells`.
//!
//! File (little-endian), version 2:
//!   magic "FLOEOVO2", version u32, unit f64, src_size u64,
//!   src_mtime u64, cell_dbu i64, bbox x0 y0 x1 y1 i64, n_levels u32,
//!   n_layers u32, top_len u16, top utf8, then per layer: layer u32,
//!   dt u32, status u8, work u64, n_planes u8, per plane: depth u8
//!   (placement depth of the shapes it holds, 0 = the top cell's own
//!   records; DEPTH_CAP holds that depth and every deeper one), per
//!   level: w u32, h u32, off u64, len u64 (absolute offsets; rows
//!   padded to whole bytes, bit i of a row at byte i/8 bit i%8).
//!   Planes are ascending by depth and only depths holding a shape
//!   have one. A request at depth N draws the OR of the planes with
//!   depth <= N; the unlimited depth draws them all. Version 1 files
//!   ("FLOEOVO1": one plane per layer, every depth flattened) read as
//!   a single plane of depth DEPTH_ALL, drawn by the unlimited depth
//!   only. The identity (src_size, src_mtime, top, layer table) must
//!   match the cache's design.ovm; a file that fails any check reads
//!   as "no summary".

use floe_oasis::doc::{Doc, PathRec, PlaceRec, PolyRec, RectRec, Rep};
use floe_ovm::Ovm;
use floe_tiler::hier::cell_bboxes;
use floe_tiler::{is_axis, path_outline_any, Xf};
use std::collections::HashMap;

pub const MAGIC: &[u8; 8] = b"FLOEOVO2";
pub const VERSION: u32 = 2;
/// version 1 (until 2026-09-16): one flattened plane per layer
pub const MAGIC_V1: &[u8; 8] = b"FLOEOVO1";
/// planes are kept per placement depth up to this one, which holds
/// that depth and every deeper shape (a request depth at or above it
/// draws everything, like the unlimited depth)
pub const DEPTH_CAP: u8 = 15;
/// the depth of a version-1 plane: every depth flattened together
pub const DEPTH_ALL: u8 = 255;
/// base cell in microns unless --occupancy-um says otherwise (the
/// library default; the CLI asks for BASE_AUTO)
pub const DEFAULT_BASE_UM: f64 = 4.0;
/// `Opts.base_um` of the automatic base cell (2026-09-16, user
/// decision): the coarsest of AUTO_CANDIDATES_UM whose grid's longer
/// side reaches AUTO_TARGET_CELLS, so a small chip's fit view still
/// has cells within a pixel in windows up to about 2,000 px (field: a
/// 1.55 x 2.25 mm chip at 4 um was 388 x 563 cells, 1.8 px per cell
/// at fit, "near"); a chip wider than 8 mm keeps 4 um
pub const BASE_AUTO: f64 = 0.0;
pub const AUTO_CANDIDATES_UM: [f64; 5] = [4.0, 2.0, 1.0, 0.5, 0.25];
pub const AUTO_TARGET_CELLS: f64 = 2048.0;

/// the automatic base cell for a top cell whose longer side is
/// `span_um` microns: the coarsest candidate reaching the target
/// cell count, else the finest
pub fn auto_base_um_for_span(span_um: f64) -> f64 {
    for &um in &AUTO_CANDIDATES_UM {
        if span_um / um >= AUTO_TARGET_CELLS {
            return um;
        }
    }
    AUTO_CANDIDATES_UM[AUTO_CANDIDATES_UM.len() - 1]
}
/// level-0 cells per layer before the whole file is `none:cells`
pub const DEFAULT_MAX_CELLS: u64 = 1 << 30;
/// marks (cells set + members + edge rows) per layer before `none:work`
pub const DEFAULT_MAX_WORK: u64 = 1 << 31;
/// bitmap bytes over all layers before the rest is `none:size`
pub const DEFAULT_MAX_BYTES: u64 = 1 << 30;
/// the pyramid stops once the grid is this small on both axes
pub const TOP_GRID: u32 = 64;

pub const STATUS_OK: u8 = 0;
pub const STATUS_NONE_CELLS: u8 = 1;
pub const STATUS_NONE_WORK: u8 = 2;
pub const STATUS_NONE_SIZE: u8 = 3;
/// a path the hull refuses (degenerate spine, U-turn): the layer has
/// no summary rather than one with a shape silently missing (review
/// 2026-09-11 (2nd) P1-2)
pub const STATUS_NONE_UNSUPPORTED: u8 = 4;
/// no positive-area shape on the layer (or a layer the table names
/// without a record): summarized as "nothing to draw" without bitmaps.
/// Field 2026-09-14: a deck-wide build wrote 9.8 GB, half of it
/// full-size zero pyramids of such layers.
pub const STATUS_EMPTY: u8 = 5;

/// charges a worker thread accumulates before adding them to the
/// layer's shared work total (the over-budget stop lands within
/// jobs x this of the budget)
pub const FLUSH_CHARGES: u64 = 1 << 12;

pub fn status_text(status: u8) -> &'static str {
    match status {
        STATUS_OK => "ok",
        STATUS_NONE_CELLS => "none:cells",
        STATUS_NONE_WORK => "none:work",
        STATUS_NONE_SIZE => "none:size",
        STATUS_NONE_UNSUPPORTED => "none:unsupported",
        STATUS_EMPTY => "empty",
        _ => "none:unknown",
    }
}

#[derive(Clone, Debug)]
pub struct Opts {
    /// base cell in microns; BASE_AUTO (0) picks it from the chip size
    pub base_um: f64,
    pub jobs: usize,
    pub max_cells: u64,
    pub max_work: u64,
    pub max_bytes: u64,
    /// progress lines (field 2026-09-16: a 150 MB chip's marking ran
    /// five minutes without a line): a heartbeat every 10 s while a
    /// layer is marked (units done, charges, seconds), one line per
    /// layer that took at least half a second or has no summary
    pub progress: Option<fn(&str)>,
    /// split a layer's marking into units by estimated work (field
    /// 2026-09-16: a 150 MB chip's top held enough placements that the
    /// count-based split never expanded the one block holding most of
    /// the work, and 12 threads ran at one thread's speed); false =
    /// the count-based split (`--occupancy-balance 0`, the kill switch)
    pub balanced_units: bool,
    /// stop the hierarchy walk at a placed cell whose recursive bbox
    /// fits in one grid cell (both extents <= the cell): its bbox is
    /// marked - at most a 2 x 2 block - into the planes of the depths
    /// where this layer has shapes under it, and an axis-aligned Grid
    /// of such a cell with a pitch <= the cell marks its footprint in
    /// one go. The result is a superset of the exact marking within
    /// one cell (exact <= pruned <= dilate(exact, 1 cell)); the cost
    /// stops growing with the number of instances below the cell size
    /// (field 2026-09-18: MAIN01's 800 M placements made the exact
    /// walk take hours). false = the exact walk
    /// (`--occupancy-prune 0`; the oracle gates use it)
    pub prune: bool,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts {
            base_um: DEFAULT_BASE_UM,
            jobs: 1,
            max_cells: DEFAULT_MAX_CELLS,
            max_work: DEFAULT_MAX_WORK,
            max_bytes: DEFAULT_MAX_BYTES,
            progress: None,
            balanced_units: true,
            prune: true,
        }
    }
}

/// recursive bbox of a cell (floe_tiler::hier::cell_bboxes)
type Win = (i64, i64, i64, i64);

/// The prune context of one layer build (Opts::prune): per cell the
/// recursive bbox, whether it fits one grid cell, and the bit mask of
/// the relative placement depths at which this layer has shapes under
/// the cell (bit 0 = the cell's own records; bit 31 saturates).
#[derive(Clone, Copy)]
struct Prune<'a> {
    bboxes: &'a [Option<Win>],
    small: &'a [bool],
    depth_mask: &'a [u32],
}

/// relative-depth masks of one layer, bottom-up over the cells that
/// hold it (`has`)
fn layer_depth_masks(doc: &Doc, shapes: &[CellShapes<'_>], has: &[bool]) -> Vec<u32> {
    fn go(doc: &Doc, ci: usize, shapes: &[CellShapes<'_>], has: &[bool], memo: &mut Vec<Option<u32>>, open: &mut Vec<bool>) -> u32 {
        if let Some(m) = memo[ci] {
            return m;
        }
        if open[ci] {
            return 0;
        }
        open[ci] = true;
        let mut m: u32 = if shapes[ci].is_empty() { 0 } else { 1 };
        for pl in &doc.cells[ci].places {
            if !has[pl.cell] {
                continue;
            }
            let cm = go(doc, pl.cell, shapes, has, memo, open);
            m |= (cm << 1) | (cm & (1 << 31));
        }
        open[ci] = false;
        memo[ci] = Some(m);
        m
    }
    let mut memo = vec![None; doc.cells.len()];
    let mut open = vec![false; doc.cells.len()];
    for ci in 0..doc.cells.len() {
        if has[ci] {
            go(doc, ci, shapes, has, &mut memo, &mut open);
        }
    }
    memo.into_iter().map(|m| m.unwrap_or(0)).collect()
}

/// An axis-aligned Grid placement whose pitch is at most the cell on
/// both axes: every grid cell of its footprint meets a member's bbox,
/// so the footprint (the union of the first and the last member's
/// world bbox) marks what the members would, in one fill.
fn grid_prunable(rep: &Rep, c: i64) -> bool {
    match rep {
        Rep::Grid { na, nb, va, vb } => {
            let axis = |v: &(i64, i64)| (v.0 == 0 || v.1 == 0) && v.0.abs() <= c && v.1.abs() <= c;
            *na >= 1 && *nb >= 1 && axis(va) && axis(vb)
        }
        _ => false,
    }
}

/// world bbox of a placed cell's recursive bbox
fn placed_bbox(b: Win, xf: &Xf, pl: &PlaceRec, dx: i64, dy: i64) -> (i128, i128, i128, i128) {
    let t = xf.compose(&Xf::place(pl.x + dx, pl.y + dy, pl.rot, pl.flip));
    let a = t.apply(b.0, b.1);
    let z = t.apply(b.2, b.3);
    (a.0.min(z.0) as i128, a.1.min(z.1) as i128, a.0.max(z.0) as i128, a.1.max(z.1) as i128)
}

/// how many plain placements deep the balanced split descends looking
/// for units of at most the budget
pub const MAX_EXPAND_DEPTH: usize = 8;
/// A Grid/Pts placement of a child heavier than the unit budget with
/// at most this many members is expanded member by member (each
/// member's units of its own, so a giant record inside reaches its
/// Members units); with more members the per-member Place units are
/// already parallel across the members.
pub const EXPAND_MEMBERS_MAX: u64 = 64;
/// at most this many Members units per record
const MEMBERS_UNITS_MAX: u64 = 4096;

/// seconds between the heartbeat lines of a layer's marking
pub const PROGRESS_EVERY_S: u64 = 10;

/// one pyramid level: rows padded to whole bytes
#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub w: u32,
    pub h: u32,
    pub bits: Vec<u8>,
}

impl Level {
    pub fn row_bytes(w: u32) -> usize {
        (w as usize + 7) / 8
    }

    pub fn get(&self, i: u32, j: u32) -> bool {
        if i >= self.w || j >= self.h {
            return false;
        }
        let b = self.bits[j as usize * Level::row_bytes(self.w) + (i / 8) as usize];
        (b >> (i % 8)) & 1 == 1
    }

    pub fn count(&self) -> u64 {
        self.bits.iter().map(|b| b.count_ones() as u64).sum()
    }

    /// OR another level of the same grid into this one
    pub fn or_with(&mut self, other: &Level) {
        debug_assert_eq!((self.w, self.h), (other.w, other.h));
        for (a, b) in self.bits.iter_mut().zip(other.bits.iter()) {
            *a |= *b;
        }
    }

    /// OR-pool 2x2 into the next level
    pub fn pool(&self) -> Level {
        let w2 = (self.w + 1) / 2;
        let h2 = (self.h + 1) / 2;
        let rb = Level::row_bytes(self.w);
        let rb2 = Level::row_bytes(w2);
        let mut bits = vec![0u8; rb2 * h2 as usize];
        for j2 in 0..h2 as usize {
            for dj in 0..2usize {
                let j = j2 * 2 + dj;
                if j >= self.h as usize {
                    continue;
                }
                let row = &self.bits[j * rb..j * rb + rb];
                let out = &mut bits[j2 * rb2..j2 * rb2 + rb2];
                for (i2, o) in out.iter_mut().enumerate() {
                    // output byte i2 covers input bits 16*i2 .. 16*i2+16
                    let src = 2 * i2;
                    let lo = *row.get(src).unwrap_or(&0) as u16;
                    let hi = *row.get(src + 1).unwrap_or(&0) as u16;
                    let v = lo | (hi << 8);
                    let mut acc = 0u8;
                    for k in 0..8 {
                        if (v >> (2 * k)) & 3 != 0 {
                            acc |= 1 << k;
                        }
                    }
                    *o |= acc;
                }
            }
        }
        Level { w: w2, h: h2, bits }
    }
}

/// one placement depth's pyramid of a layer
#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    /// placement depth of the shapes (0 = the top cell's own records,
    /// DEPTH_CAP = that depth and every deeper one, DEPTH_ALL = a
    /// version-1 file's flattening of every depth)
    pub depth: u8,
    pub levels: Vec<Level>,
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub layer: u32,
    pub dt: u32,
    pub status: u8,
    /// marks charged (cells set + members + edge rows)
    pub work: u64,
    /// one per placement depth holding a shape, ascending by depth;
    /// empty unless status == STATUS_OK
    pub planes: Vec<Plane>,
}

impl Layer {
    /// the flattening of every plane at one level (the version-1 view)
    pub fn level(&self, lv: usize) -> Option<Level> {
        self.level_at_depth(lv, None)
    }

    /// the OR of the planes a request depth draws (None = unlimited)
    /// at one level; None when no plane has the level
    pub fn level_at_depth(&self, lv: usize, depth: Option<u32>) -> Option<Level> {
        let mut out: Option<Level> = None;
        for plane in &self.planes {
            if !plane_drawn_at(plane.depth, depth) {
                continue;
            }
            let Some(level) = plane.levels.get(lv) else { continue };
            match &mut out {
                None => out = Some(level.clone()),
                Some(acc) => acc.or_with(level),
            }
        }
        out
    }
}

/// whether a plane of `plane_depth` is drawn by a request depth (None
/// = unlimited): a version-1 plane only by the unlimited depth, the
/// DEPTH_CAP plane by any depth at or above the cap
pub fn plane_drawn_at(plane_depth: u8, depth: Option<u32>) -> bool {
    match depth {
        None => true,
        Some(d) => plane_depth != DEPTH_ALL && plane_depth as u32 <= d,
    }
}

#[derive(Clone, Debug)]
pub struct Occupancy {
    pub unit: f64,
    pub src_size: u64,
    pub src_mtime: u64,
    pub top: String,
    pub cell_dbu: i64,
    /// world bbox of the top cell (closed-open), grid origin = x0/y0
    pub bbox: (i64, i64, i64, i64),
    pub w: u32,
    pub h: u32,
    pub n_levels: u32,
    pub layers: Vec<Layer>,
    /// paths whose hull the raster would refuse (skipped, logged)
    pub paths_skipped: u64,
}

/// pyramid depth for a level-0 grid
pub fn level_count(w: u32, h: u32) -> u32 {
    let (mut a, mut b, mut n) = (w, h, 1u32);
    while a > TOP_GRID || b > TOP_GRID {
        a = (a + 1) / 2;
        b = (b + 1) / 2;
        n += 1;
    }
    n
}

/// bitmap bytes of one layer's whole pyramid
pub fn layer_bytes(w: u32, h: u32) -> u64 {
    let (mut a, mut b) = (w, h);
    let mut total = Level::row_bytes(a) as u64 * b as u64;
    while a > TOP_GRID || b > TOP_GRID {
        a = (a + 1) / 2;
        b = (b + 1) / 2;
        total += Level::row_bytes(a) as u64 * b as u64;
    }
    total
}

fn floor_div(a: i128, b: i128) -> i128 {
    let d = a / b;
    if a % b != 0 && ((a < 0) != (b < 0)) {
        d - 1
    } else {
        d
    }
}

fn ceil_div(a: i128, b: i128) -> i128 {
    -floor_div(-a, b)
}

// ------------------------------------------------------------ builder

/// a level-0 bit plane shared by the marking threads: every thread
/// ORs its cells in with atomic word updates, so the result does not
/// depend on how the units are cut, and there is one plane per
/// placement depth instead of one per thread (2026-09-16; before, each
/// thread marked its own plane and they were OR-merged at the end)
struct SharedBits {
    w: u32,
    h: u32,
    stride: usize,
    words: Vec<std::sync::atomic::AtomicU64>,
}

impl SharedBits {
    /// Bits only transition 0 -> 1 during marking. If a relaxed load
    /// already contains the mask, a second writer cannot invalidate it.
    /// Avoid taking exclusive ownership of a cache line on every repeated
    /// mark in a dense region. A stale load merely causes an extra OR.
    fn or_word(&self, k: usize, mask: u64) {
        use std::sync::atomic::Ordering::Relaxed;
        let word = &self.words[k];
        if word.load(Relaxed) & mask != mask {
            word.fetch_or(mask, Relaxed);
        }
    }

    fn new(w: u32, h: u32) -> SharedBits {
        let stride = (w as usize + 63) / 64;
        SharedBits {
            w,
            h,
            stride,
            words: (0..stride * h as usize).map(|_| std::sync::atomic::AtomicU64::new(0)).collect(),
        }
    }

    /// set cells i0..=i1 of row j (callers clamp to the grid)
    fn set_span(&self, j: u32, i0: u32, i1: u32) {
        debug_assert!(i1 < self.w && j < self.h && i0 <= i1);
        let base = j as usize * self.stride;
        let (w0, w1) = ((i0 / 64) as usize, (i1 / 64) as usize);
        let lo_mask = u64::MAX << (i0 % 64);
        let hi_mask = u64::MAX >> (63 - i1 % 64);
        if w0 == w1 {
            self.or_word(base + w0, lo_mask & hi_mask);
        } else {
            self.or_word(base + w0, lo_mask);
            for k in w0 + 1..w1 {
                self.or_word(base + k, u64::MAX);
            }
            self.or_word(base + w1, hi_mask);
        }
    }

    fn any(&self) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        self.words.iter().any(|w| w.load(Relaxed) != 0)
    }

    fn to_level(&self) -> Level {
        use std::sync::atomic::Ordering::Relaxed;
        let rb = Level::row_bytes(self.w);
        let mut bits = vec![0u8; rb * self.h as usize];
        for j in 0..self.h as usize {
            let row = &self.words[j * self.stride..(j + 1) * self.stride];
            let out = &mut bits[j * rb..(j + 1) * rb];
            for (k, word) in row.iter().enumerate() {
                let le = word.load(Relaxed).to_le_bytes();
                let start = k * 8;
                let end = (start + 8).min(rb);
                if start < rb {
                    out[start..end].copy_from_slice(&le[..end - start]);
                }
            }
        }
        Level { w: self.w, h: self.h, bits }
    }
}

/// a layer's level-0 planes under construction: one per placement
/// depth 0..=min(max depth, DEPTH_CAP) the layer reaches
struct Planes {
    by_depth: Vec<SharedBits>,
}

impl Planes {
    fn new(w: u32, h: u32, max_depth: u32) -> Planes {
        let n = (max_depth.min(DEPTH_CAP as u32) + 1) as usize;
        Planes { by_depth: (0..n).map(|_| SharedBits::new(w, h)).collect() }
    }

    /// the plane of a placement depth (the cap and anything deeper,
    /// or a depth past the precomputed maximum, share the last one)
    fn plane(&self, depth: u32) -> &SharedBits {
        let k = (depth.min(DEPTH_CAP as u32) as usize).min(self.by_depth.len() - 1);
        &self.by_depth[k]
    }
}

/// References only: geometry and repetition vectors stay in Doc. Build once
/// for all layers, instead of scanning every record for each layer and again
/// at every instance. Sparse cell maps avoid a cells x layers allocation;
/// a layer's references are released immediately after it has been marked.
#[derive(Default)]
struct CellShapes<'a> {
    rects: Vec<&'a RectRec>,
    polys: Vec<&'a PolyRec>,
    paths: Vec<&'a PathRec>,
}

impl CellShapes<'_> {
    fn is_empty(&self) -> bool {
        self.rects.is_empty() && self.polys.is_empty() && self.paths.is_empty()
    }
}

type ShapeIndex<'a> = HashMap<(u32, u32), HashMap<usize, CellShapes<'a>>>;

fn index_shapes(doc: &Doc) -> ShapeIndex<'_> {
    let mut index: ShapeIndex<'_> = HashMap::new();
    for (ci, cell) in doc.cells.iter().enumerate() {
        for r in &cell.rects {
            index.entry((r.layer, r.dt)).or_default().entry(ci).or_default().rects.push(r);
        }
        for p in &cell.polys {
            index.entry((p.layer, p.dt)).or_default().entry(ci).or_default().polys.push(p);
        }
        for p in &cell.paths {
            index.entry((p.layer, p.dt)).or_default().entry(ci).or_default().paths.push(p);
        }
    }
    index
}

fn take_layer_shapes<'a>(index: &mut ShapeIndex<'a>, key: (u32, u32), cells: usize) -> Vec<CellShapes<'a>> {
    let mut out: Vec<_> = (0..cells).map(|_| CellShapes::default()).collect();
    if let Some(layer) = index.remove(&key) {
        for (ci, shapes) in layer {
            out[ci] = shapes;
        }
    }
    out
}

/// per cell: does the layer occur in the cell or its descendants
fn layer_presence(doc: &Doc, shapes: &[CellShapes<'_>]) -> Vec<bool> {
    fn walk(doc: &Doc, ci: usize, shapes: &[CellShapes<'_>], state: &mut [u8]) -> bool {
        match state[ci] {
            2 => return true,
            3 => return false,
            1 => return false, // cycle guard: an ancestor decides
            _ => {}
        }
        state[ci] = 1;
        let cell = &doc.cells[ci];
        let direct = !shapes[ci].is_empty();
        let mut has = direct;
        if !has {
            for pl in &cell.places {
                if walk(doc, pl.cell, shapes, state) {
                    has = true;
                    break;
                }
            }
        }
        state[ci] = if has { 2 } else { 3 };
        has
    }
    let mut state = vec![0u8; doc.cells.len()];
    let _ = walk(doc, doc.top, shapes, &mut state);
    // cells only reachable through a cycle guard stay "unknown": walk
    // them on their own so every reachable cell has a verdict
    for ci in 0..doc.cells.len() {
        if state[ci] == 0 || state[ci] == 1 {
            state[ci] = 0;
            let _ = walk(doc, ci, shapes, &mut state);
        }
    }
    state.iter().map(|&s| s == 2).collect()
}

/// the deepest placement level at which the layer has a record of
/// its own (0 = the top cell's), which is how many planes the marking
/// keeps; a cycle never extends a path
fn layer_max_depth(doc: &Doc, shapes: &[CellShapes<'_>], has: &[bool]) -> u32 {
    fn reach(doc: &Doc, ci: usize, shapes: &[CellShapes<'_>], has: &[bool], memo: &mut Vec<Option<Option<u32>>>) -> Option<u32> {
        if let Some(done) = memo[ci] {
            return done;
        }
        memo[ci] = Some(None); // in progress: a cycle back here adds nothing
        let cell = &doc.cells[ci];
        let direct = !shapes[ci].is_empty();
        let mut best: Option<u32> = if direct { Some(0) } else { None };
        for pl in &cell.places {
            if !has[pl.cell] {
                continue;
            }
            if let Some(d) = reach(doc, pl.cell, shapes, has, memo) {
                best = Some(best.map_or(d + 1, |b| b.max(d + 1)));
            }
        }
        memo[ci] = Some(best);
        best
    }
    let mut memo = vec![None; doc.cells.len()];
    reach(doc, doc.top, shapes, has, &mut memo).unwrap_or(0)
}

struct Marker<'a> {
    doc: &'a Doc,
    shapes: &'a [CellShapes<'a>],
    has: &'a [bool],
    ox: i64,
    oy: i64,
    c: i64,
    w: u32,
    h: u32,
    /// the layer's level-0 planes, one per placement depth
    planes: &'a Planes,
    /// placement depth of the shapes being marked (a unit or the walk
    /// sets it; a cell's own records are at its depth, its placements'
    /// one deeper)
    depth: u32,
    work: u64,
    max_work: u64,
    over: bool,
    paths_skipped: u64,
    /// the layer's work total across the worker threads (None on a
    /// single thread: the local count is exact)
    shared: Option<&'a std::sync::atomic::AtomicU64>,
    unflushed: u64,
    prune: Option<Prune<'a>>,
}

impl<'a> Marker<'a> {
    /// Opts::prune: the subtree of `ci` (placed by `xf`, at placement
    /// depth `depth`) as its bbox, into the plane of every relative
    /// depth at which this layer has shapes under it.
    fn mark_subtree(&mut self, ci: usize, xf: &Xf, depth: u32) -> bool {
        let Some(p) = self.prune else { return true };
        let Some(b) = p.bboxes[ci] else { return true };
        let a = xf.apply(b.0, b.1);
        let z = xf.apply(b.2, b.3);
        self.mark_world_rect_at_depths(
            (a.0.min(z.0) as i128, a.1.min(z.1) as i128, a.0.max(z.0) as i128, a.1.max(z.1) as i128),
            p.depth_mask[ci],
            depth,
        )
    }

    fn mark_world_rect_at_depths(&mut self, r: (i128, i128, i128, i128), mask: u32, depth: u32) -> bool {
        let saved = self.depth;
        let mut bit = 0u32;
        let mut ok = true;
        while ok && bit < 32 && (mask >> bit) != 0 {
            if mask & (1 << bit) != 0 {
                self.depth = depth.saturating_add(bit);
                ok = self.mark_world_rect(r.0, r.1, r.2, r.3);
            }
            bit += 1;
        }
        self.depth = saved;
        ok
    }

    /// Opts::prune for a Grid placement of a small cell with a pitch
    /// at most the cell: the footprint in one fill (see grid_prunable)
    fn mark_grid_footprint(&mut self, pl: &PlaceRec, xf: &Xf, depth: u32) -> bool {
        let Some(p) = self.prune else { return true };
        let Some(b) = p.bboxes[pl.cell] else { return true };
        let Rep::Grid { na, nb, va, vb } = &pl.rep else { return true };
        let (la, lb) = (*na as i64 - 1, *nb as i64 - 1);
        let first = placed_bbox(b, xf, pl, 0, 0);
        let last = placed_bbox(b, xf, pl, la * va.0 + lb * vb.0, la * va.1 + lb * vb.1);
        if !self.charge(1) {
            return false;
        }
        self.mark_world_rect_at_depths(
            (first.0.min(last.0), first.1.min(last.1), first.2.max(last.2), first.3.max(last.3)),
            p.depth_mask[pl.cell],
            depth.saturating_add(1),
        )
    }

    fn charge(&mut self, n: u64) -> bool {
        self.work = self.work.saturating_add(n);
        if self.work > self.max_work {
            self.over = true;
        }
        if let Some(shared) = self.shared {
            self.unflushed = self.unflushed.saturating_add(n);
            if self.unflushed >= FLUSH_CHARGES {
                self.flush(shared);
            }
        }
        !self.over
    }

    /// add the local charges to the layer's total: the other threads'
    /// marks count against the same budget
    fn flush(&mut self, shared: &std::sync::atomic::AtomicU64) {
        use std::sync::atomic::Ordering::Relaxed;
        let total = shared.fetch_add(self.unflushed, Relaxed).saturating_add(self.unflushed);
        self.unflushed = 0;
        if total > self.max_work {
            self.over = true;
        }
    }

    /// mark one unit (a cell's records on the layer, or a member range
    /// of one of its placements); false = over budget
    fn run_unit(&mut self, u: &Unit) -> bool {
        if self.over {
            return false;
        }
        if u.extra > 0 && !self.charge(u.extra) {
            return false;
        }
        let cell = &self.shapes[u.ci];
        match &u.kind {
            UnitKind::Shapes { rects, polys, paths, split_above } => {
                self.depth = u.depth;
                let (split, c) = (*split_above, self.c);
                for r in &cell.rects[rects.0..rects.1] {
                    if rect_splits(r, &u.xf, c, split) {
                        continue; // its Members units mark it
                    }
                    if !self.mark_rect_rec(r, &u.xf) {
                        return false;
                    }
                }
                for p in &cell.polys[polys.0..polys.1] {
                    if poly_splits(p, split) {
                        continue;
                    }
                    if !self.mark_poly_rec(p, &u.xf) {
                        return false;
                    }
                }
                for p in &cell.paths[paths.0..paths.1] {
                    if path_splits(p, split) {
                        continue;
                    }
                    if !self.mark_path_rec(p, &u.xf) {
                        return false;
                    }
                }
                true
            }
            UnitKind::Members { shape, idx, m0, m1 } => {
                self.depth = u.depth;
                match shape {
                    0 => self.mark_rect_range(cell.rects[*idx], &u.xf, *m0, *m1),
                    1 => self.mark_poly_range(cell.polys[*idx], &u.xf, *m0, *m1),
                    _ => self.mark_path_range(cell.paths[*idx], &u.xf, *m0, *m1),
                }
            }
            UnitKind::Place { pi, m0, m1 } => {
                let pl = &self.doc.cells[u.ci].places[*pi];
                // Grid/Pts members are charged one each, exactly as the
                // walk charges them; a plain placement is not charged
                if let Some(p) = self.prune {
                    if p.small[pl.cell] && grid_prunable(&pl.rep, self.c) && *m0 == 0 && *m1 == rep_members(&pl.rep) {
                        return self.mark_grid_footprint(pl, &u.xf, u.depth);
                    }
                }
                let charged = !matches!(pl.rep, Rep::One);
                for k in *m0..*m1 {
                    if charged && !self.charge(1) {
                        return false;
                    }
                    let (dx, dy) = rep_member(&pl.rep, k);
                    let base = u.xf.compose(&Xf::place(pl.x + dx, pl.y + dy, pl.rot, pl.flip));
                    if !self.walk(pl.cell, &base, u.depth + 1) {
                        return false;
                    }
                }
                true
            }
        }
    }

    /// cells of one axis whose open interval meets the open (lo, hi)
    fn range(&self, lo: i128, hi: i128, origin: i64, n: u32) -> Option<(u32, u32)> {
        if hi <= lo || n == 0 {
            return None;
        }
        let c = self.c as i128;
        let i0 = floor_div(lo - origin as i128, c).max(0);
        let i1 = (ceil_div(hi - origin as i128, c) - 1).min(n as i128 - 1);
        if i1 < i0 {
            return None;
        }
        Some((i0 as u32, i1 as u32))
    }

    fn span(&mut self, j: u32, i0: u32, i1: u32) -> bool {
        if !self.charge((i1 - i0) as u64 + 1) {
            return false;
        }
        self.planes.plane(self.depth).set_span(j, i0, i1);
        true
    }

    fn mark_world_rect(&mut self, x0: i128, y0: i128, x1: i128, y1: i128) -> bool {
        let (x0, x1) = (x0.min(x1), x0.max(x1));
        let (y0, y1) = (y0.min(y1), y0.max(y1));
        let Some((i0, i1)) = self.range(x0, x1, self.ox, self.w) else {
            return true;
        };
        let Some((j0, j1)) = self.range(y0, y1, self.oy, self.h) else {
            return true;
        };
        for j in j0..=j1 {
            if !self.span(j, i0, i1) {
                return false;
            }
        }
        true
    }

    /// every member's world offset (parent frame offsets through xf)
    fn for_each_member<F: FnMut(&mut Self, i128, i128) -> bool>(
        &mut self,
        rep: &Rep,
        xf: &Xf,
        f: F,
    ) -> bool {
        self.for_each_member_range(rep, xf, 0, u64::MAX, f)
    }

    /// members m0..m1 of `rep` in the enumeration order (Grid: i
    /// fastest, k = j * na + i), charged as one block before the
    /// first - so the ranges of one record charge what the whole
    /// repetition charges (a Members unit is one range, the walk the
    /// whole; 2026-09-16)
    fn for_each_member_range<F: FnMut(&mut Self, i128, i128) -> bool>(
        &mut self,
        rep: &Rep,
        xf: &Xf,
        m0: u64,
        m1: u64,
        mut f: F,
    ) -> bool {
        match rep {
            Rep::One => {
                if m0 == 0 && m1 > 0 {
                    f(self, 0, 0)
                } else {
                    true
                }
            }
            Rep::Grid { na, nb, va, vb } => {
                let n = na.saturating_mul(*nb);
                let (m0, m1) = (m0.min(n), m1.min(n));
                if m1 <= m0 {
                    return true;
                }
                if !self.charge(m1 - m0) {
                    return false;
                }
                let wa = xf.apply_vec(va.0, va.1);
                let wb = xf.apply_vec(vb.0, vb.1);
                let (mut i, mut j) = ((m0 % *na) as i128, (m0 / *na) as i128);
                for _ in m0..m1 {
                    let dx = i * wa.0 as i128 + j * wb.0 as i128;
                    let dy = i * wa.1 as i128 + j * wb.1 as i128;
                    if !f(self, dx, dy) {
                        return false;
                    }
                    i += 1;
                    if i == *na as i128 {
                        i = 0;
                        j += 1;
                    }
                }
                true
            }
            Rep::Pts(p) => {
                let n = p.len() as u64;
                let (m0, m1) = (m0.min(n), m1.min(n));
                if m1 <= m0 {
                    return true;
                }
                if !self.charge(m1 - m0) {
                    return false;
                }
                for &(dx, dy) in &p[m0 as usize..m1 as usize] {
                    let (wx, wy) = xf.apply_vec(dx, dy);
                    if !f(self, wx as i128, wy as i128) {
                        return false;
                    }
                }
                true
            }
        }
    }

    fn mark_rect_rec(&mut self, r: &RectRec, xf: &Xf) -> bool {
        self.mark_rect_range(r, xf, 0, u64::MAX)
    }

    /// members m0..m1 of a rect record (the whole record for 0..MAX).
    /// A closed-form grid is one fill of its footprint, done by the
    /// range holding member 0 - so the ranges of one record mark and
    /// charge exactly what the whole record does.
    fn mark_rect_range(&mut self, r: &RectRec, xf: &Xf, m0: u64, m1: u64) -> bool {
        if r.w <= 0 || r.h <= 0 {
            return true; // zero area: KLayout's region is empty
        }
        if let Some((x0, y0, x1, y1)) = rect_closed_form(r, xf, self.c) {
            return m0 > 0 || self.mark_world_rect(x0, y0, x1, y1);
        }
        let (mx0, my0, mx1, my1) = rect_world(r, xf);
        self.for_each_member_range(&r.rep, xf, m0, m1, |m, dx, dy| {
            m.mark_world_rect(mx0 + dx, my0 + dy, mx1 + dx, my1 + dy)
        })
    }

    /// conservative cell-grid scan conversion of one world polygon
    fn mark_world_poly(&mut self, pts: &[(i128, i128)]) -> bool {
        let n = pts.len();
        if n < 3 {
            return true;
        }
        // zero-area (colinear) polygons: KLayout's region is empty
        let mut area2 = 0i128;
        for k in 0..n {
            let (x1, y1) = pts[k];
            let (x2, y2) = pts[(k + 1) % n];
            area2 += x1 * y2 - x2 * y1;
        }
        if area2 == 0 {
            return true;
        }
        let c = self.c as i128;
        let (ox, oy) = (self.ox as i128, self.oy as i128);
        let (w, h) = (self.w as i128, self.h as i128);
        let ymin = pts.iter().map(|p| p.1).min().unwrap();
        let ymax = pts.iter().map(|p| p.1).max().unwrap();
        // (1) edge cells: every open cell an edge passes through
        for k in 0..n {
            let (ax, ay) = pts[k];
            let (bx, by) = pts[(k + 1) % n];
            if (ax, ay) == (bx, by) {
                continue;
            }
            if ay == by {
                if (ay - oy).rem_euclid(c) == 0 {
                    continue; // on a grid line: no open cell
                }
                let j = floor_div(ay - oy, c);
                if j < 0 || j >= h {
                    continue;
                }
                if let Some((i0, i1)) = self.range(ax.min(bx), ax.max(bx), self.ox, self.w) {
                    if !self.span(j as u32, i0, i1) {
                        return false;
                    }
                }
                continue;
            }
            if ax == bx {
                if (ax - ox).rem_euclid(c) == 0 {
                    continue;
                }
                let i = floor_div(ax - ox, c);
                if i < 0 || i >= w {
                    continue;
                }
                if let Some((j0, j1)) = self.range(ay.min(by), ay.max(by), self.oy, self.h) {
                    if !self.charge((j1 - j0) as u64 + 1) {
                        return false;
                    }
                    for j in j0..=j1 {
                        self.planes.plane(self.depth).set_span(j, i as u32, i as u32);
                    }
                }
                continue;
            }
            // general edge, oriented so dy > 0
            let ((x0, y0), (x1, y1)) = if ay < by { ((ax, ay), (bx, by)) } else { ((bx, by), (ax, ay)) };
            let (dx, dy) = (x1 - x0, y1 - y0);
            let j0 = floor_div(y0 - oy, c).max(0);
            let j1 = (ceil_div(y1 - oy, c) - 1).min(h - 1);
            let mut j = j0;
            while j <= j1 {
                if !self.charge(1) {
                    return false;
                }
                let ya = y0.max(oy + j * c);
                let yb = y1.min(oy + (j + 1) * c);
                if ya < yb {
                    let na = x0 * dy + dx * (ya - y0);
                    let nb = x0 * dy + dx * (yb - y0);
                    let (lo, hi) = (na.min(nb), na.max(nb));
                    let i0 = floor_div(lo - ox * dy, c * dy).max(0);
                    let i1 = (ceil_div(hi - ox * dy, c * dy) - 1).min(w - 1);
                    if i0 <= i1 && !self.span(j as u32, i0 as u32, i1 as u32) {
                        return false;
                    }
                }
                j += 1;
            }
        }
        // (2) interior cells: even-odd parity at the row centre. Cells
        // no edge crosses are wholly in or out, so the centre decides.
        let mut edges: Vec<((i128, i128), (i128, i128))> = Vec::with_capacity(n);
        for k in 0..n {
            let a = pts[k];
            let b = pts[(k + 1) % n];
            if a.1 == b.1 {
                continue;
            }
            edges.push(if a.1 < b.1 { (a, b) } else { (b, a) });
        }
        edges.sort_by_key(|e| e.0 .1);
        let j0 = floor_div(ymin - oy, c).max(0);
        let j1 = (ceil_div(ymax - oy, c) - 1).min(h - 1);
        let mut next = 0usize;
        let mut active: Vec<usize> = Vec::new();
        let mut xs: Vec<(i128, i128)> = Vec::new();
        let mut j = j0;
        while j <= j1 {
            let yc2 = 2 * oy + 2 * j * c + c;
            while next < edges.len() && 2 * edges[next].0 .1 <= yc2 {
                active.push(next);
                next += 1;
            }
            active.retain(|&e| yc2 < 2 * edges[e].1 .1);
            if !self.charge(active.len() as u64 + 1) {
                return false;
            }
            xs.clear();
            for &e in &active {
                let ((x0, y0), (x1, y1)) = edges[e];
                let (dx, dy) = (x1 - x0, y1 - y0);
                // crossing x = x0 + dx (yc - y0) / dy as num/den, den > 0
                xs.push((2 * dy * x0 + dx * (yc2 - 2 * y0), 2 * dy));
            }
            xs.sort_by(|a, b| (a.0 * b.1).cmp(&(b.0 * a.1)));
            let mut k = 0;
            while k + 1 < xs.len() {
                let (na, da) = xs[k];
                let (nb, db) = xs[k + 1];
                // centre 2ox + 2ic + c strictly between the crossings
                let i0 = floor_div(2 * na - (2 * ox + c) * da, 2 * c * da) + 1;
                let i1 = ceil_div(2 * nb - (2 * ox + c) * db, 2 * c * db) - 1;
                let i0 = i0.max(0);
                let i1 = i1.min(w - 1);
                if i0 <= i1 && !self.span(j as u32, i0 as u32, i1 as u32) {
                    return false;
                }
                k += 2;
            }
            j += 1;
        }
        true
    }

    fn mark_poly_pts(&mut self, local: &[(i64, i64)], rep: &Rep, xf: &Xf) -> bool {
        self.mark_poly_pts_range(local, rep, xf, 0, u64::MAX)
    }

    fn mark_poly_pts_range(&mut self, local: &[(i64, i64)], rep: &Rep, xf: &Xf, m0: u64, m1: u64) -> bool {
        let world: Vec<(i128, i128)> = local
            .iter()
            .map(|&(x, y)| {
                let (wx, wy) = xf.apply(x, y);
                (wx as i128, wy as i128)
            })
            .collect();
        let mut shifted = world.clone();
        self.for_each_member_range(rep, xf, m0, m1, |m, dx, dy| {
            if dx == 0 && dy == 0 {
                return m.mark_world_poly(&world);
            }
            for (s, p) in shifted.iter_mut().zip(&world) {
                *s = (p.0 + dx, p.1 + dy);
            }
            m.mark_world_poly(&shifted)
        })
    }

    fn mark_poly_rec(&mut self, p: &PolyRec, xf: &Xf) -> bool {
        self.mark_poly_pts(&p.pts, &p.rep, xf)
    }

    fn mark_poly_range(&mut self, p: &PolyRec, xf: &Xf, m0: u64, m1: u64) -> bool {
        self.mark_poly_pts_range(&p.pts, &p.rep, xf, m0, m1)
    }

    fn mark_path_rec(&mut self, p: &PathRec, xf: &Xf) -> bool {
        self.mark_path_range(p, xf, 0, u64::MAX)
    }

    /// a refused path (no outline) is counted once per record: by the
    /// range holding member 0
    fn mark_path_range(&mut self, p: &PathRec, xf: &Xf, m0: u64, m1: u64) -> bool {
        if p.hw <= 0 {
            return true; // zero width: KLayout's region is empty
        }
        match path_outline_any(&p.pts, p.hw, p.es, p.ee) {
            Ok(hull) => self.mark_poly_pts_range(&hull, &p.rep, xf, m0, m1),
            Err(_) => {
                if m0 == 0 {
                    self.paths_skipped += 1;
                }
                true
            }
        }
    }

    fn walk(&mut self, ci: usize, xf: &Xf, depth: u32) -> bool {
        if self.over || !self.has[ci] {
            return !self.over;
        }
        if let Some(p) = self.prune {
            if p.small[ci] {
                return self.mark_subtree(ci, xf, depth);
            }
        }
        // the cell's own records are at `depth`, its placements' one deeper
        self.depth = depth;
        let cell = &self.doc.cells[ci];
        for r in &self.shapes[ci].rects {
            if !self.mark_rect_rec(r, xf) {
                return false;
            }
        }
        for p in &self.shapes[ci].polys {
            if !self.mark_poly_rec(p, xf) {
                return false;
            }
        }
        for p in &self.shapes[ci].paths {
            if !self.mark_path_rec(p, xf) {
                return false;
            }
        }
        for pl in &cell.places {
            if !self.has[pl.cell] {
                continue;
            }
            match &pl.rep {
                Rep::One => {
                    let base = xf.compose(&Xf::place(pl.x, pl.y, pl.rot, pl.flip));
                    if !self.walk(pl.cell, &base, depth + 1) {
                        return false;
                    }
                }
                Rep::Grid { na, nb, va, vb } => {
                    if let Some(p) = self.prune {
                        if p.small[pl.cell] && grid_prunable(&pl.rep, self.c) {
                            if !self.mark_grid_footprint(pl, xf, depth) {
                                return false;
                            }
                            continue;
                        }
                    }
                    // member offsets live in the parent frame: place
                    // the child at (x + dx, y + dy) under the parent's
                    // xf. Members are walked as they are enumerated and
                    // charged one by one (review 2026-09-11 (2nd) P1-1:
                    // an offset vector of every member reached 32 GiB
                    // inside the work budget)
                    for j in 0..*nb as i64 {
                        for i in 0..*na as i64 {
                            if !self.charge(1) {
                                return false;
                            }
                            let (dx, dy) = (i * va.0 + j * vb.0, i * va.1 + j * vb.1);
                            let base = xf.compose(&Xf::place(pl.x + dx, pl.y + dy, pl.rot, pl.flip));
                            if !self.walk(pl.cell, &base, depth + 1) {
                                return false;
                            }
                        }
                    }
                }
                Rep::Pts(p) => {
                    for &(dx, dy) in p.iter() {
                        if !self.charge(1) {
                            return false;
                        }
                        let base = xf.compose(&Xf::place(pl.x + dx, pl.y + dy, pl.rot, pl.flip));
                        if !self.walk(pl.cell, &base, depth + 1) {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }
}

/// one parcel of a layer's marking for a worker thread: a cell's own
/// records on the layer (a slice of each record list), a member range
/// of one record's own repetition (2026-09-16, balanced split only:
/// a record's repetition used to run on one thread however many
/// there were), or a member range of one of its placements. The
/// marks are ORs and every charge is per member, per row or per
/// record (a refused path, a closed-form fill by the range holding
/// member 0), so the merged bits and the work do not depend on how
/// the units are cut: those of a single thread.
struct Unit {
    ci: usize,
    xf: Xf,
    /// placement depth of the cell (0 = the top)
    depth: u32,
    /// charges accounted before the marking: an expanded Grid/Pts
    /// placement member's own charge (the walk charges each member
    /// one), carried by the member's first unit
    extra: u64,
    kind: UnitKind,
}

enum UnitKind {
    /// slices of the cell's record lists; records with more members
    /// than `split_above` on the per-member path are skipped here and
    /// marked by their Members units (u64::MAX: none are)
    Shapes { rects: (usize, usize), polys: (usize, usize), paths: (usize, usize), split_above: u64 },
    /// members m0..m1 of one record (shape 0 rect / 1 poly / 2 path)
    Members { shape: u8, idx: usize, m0: u64, m1: u64 },
    Place { pi: usize, m0: u64, m1: u64 },
}

fn rep_members(rep: &Rep) -> u64 {
    match rep {
        Rep::One => 1,
        Rep::Grid { na, nb, .. } => na.saturating_mul(*nb),
        Rep::Pts(p) => p.len() as u64,
    }
}

/// a rect's world box under `xf` (member 0)
fn rect_world(r: &RectRec, xf: &Xf) -> (i128, i128, i128, i128) {
    let a = xf.apply(r.x, r.y);
    let b = xf.apply(r.x + r.w, r.y + r.h);
    (
        a.0.min(b.0) as i128,
        a.1.min(b.1) as i128,
        a.0.max(b.0) as i128,
        a.1.max(b.1) as i128,
    )
}

/// The closed form of a rect grid: along each axis the members repeat
/// at a pitch whose gap (pitch - member extent) is narrower than a
/// cell, so no cell can sit in a gap - the footprint marks exactly
/// the per-member cells, as one rect (x0, y0, x1, y1). The unit
/// generator asks the same question with the same xf and cell, so a
/// record on this path is never split (one fill, not members).
fn rect_closed_form(r: &RectRec, xf: &Xf, c: i64) -> Option<(i128, i128, i128, i128)> {
    let Rep::Grid { na, nb, va, vb } = &r.rep else {
        return None;
    };
    let wa = xf.apply_vec(va.0, va.1);
    let wb = xf.apply_vec(vb.0, vb.1);
    if !is_axis(&wa, &wb) {
        return None;
    }
    let (mx0, my0, mx1, my1) = rect_world(r, xf);
    let (px, nx, py, ny) = if wa.1 == 0 && wb.0 == 0 {
        (wa.0.abs(), *na, wb.1.abs(), *nb)
    } else {
        (wb.0.abs(), *nb, wa.1.abs(), *na)
    };
    let c = c as i128;
    let gx = px as i128 - (mx1 - mx0);
    let gy = py as i128 - (my1 - my0);
    if (nx <= 1 || gx < c) && (ny <= 1 || gy < c) {
        let (na1, nb1) = (*na as i128 - 1, *nb as i128 - 1);
        let xs = [
            0,
            wa.0 as i128 * na1,
            wb.0 as i128 * nb1,
            wa.0 as i128 * na1 + wb.0 as i128 * nb1,
        ];
        let ys = [
            0,
            wa.1 as i128 * na1,
            wb.1 as i128 * nb1,
            wa.1 as i128 * na1 + wb.1 as i128 * nb1,
        ];
        let (ex0, ex1) = (*xs.iter().min().unwrap(), *xs.iter().max().unwrap());
        let (ey0, ey1) = (*ys.iter().min().unwrap(), *ys.iter().max().unwrap());
        return Some((mx0 + ex0, my0 + ey0, mx1 + ex1, my1 + ey1));
    }
    None
}

/// Whether a record's own repetition is marked by Members units of
/// its own instead of inside a Shapes unit (2026-09-16: a record's
/// repetition ran serially however many threads there were): more
/// members than `split_above` on the per-member path. A closed-form
/// rect grid is one fill and a zero-area rect / zero-width path marks
/// nothing, so neither is worth splitting. The generator and the
/// Shapes runner decide with the same xf and cell.
fn rect_splits(r: &RectRec, xf: &Xf, c: i64, split_above: u64) -> bool {
    r.w > 0 && r.h > 0 && rep_members(&r.rep) > split_above && rect_closed_form(r, xf, c).is_none()
}

fn poly_splits(p: &PolyRec, split_above: u64) -> bool {
    rep_members(&p.rep) > split_above
}

fn path_splits(p: &PathRec, split_above: u64) -> bool {
    p.hw > 0 && rep_members(&p.rep) > split_above
}

/// member k's offset in the parent frame, in the walk's enumeration
/// order (i fastest)
fn rep_member(rep: &Rep, k: u64) -> (i64, i64) {
    match rep {
        Rep::One => (0, 0),
        Rep::Grid { na, va, vb, .. } => {
            if *na == 0 {
                return (0, 0);
            }
            let (i, j) = ((k % *na) as i64, (k / *na) as i64);
            (i * va.0 + j * vb.0, i * va.1 + j * vb.1)
        }
        Rep::Pts(p) => p[k as usize],
    }
}

/// a cell's units under `xf`: its record lists in `pieces` slices, its
/// placements' members in `pieces` ranges; plain placements are
/// descended into while `depth < expand`
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn collect_units(
    doc: &Doc,
    has: &[bool],
    shapes: &[CellShapes<'_>],
    ci: usize,
    xf: Xf,
    depth: usize,
    expand: usize,
    pieces: usize,
    small: Option<&[bool]>,
    c: i64,
    out: &mut Vec<Unit>,
) {
    let cell = &doc.cells[ci];
    let pieces = pieces.max(1);
    if !shapes[ci].is_empty() {
        let (nr, np, nq) = (shapes[ci].rects.len(), shapes[ci].polys.len(), shapes[ci].paths.len());
        let slice = |n: usize, t: usize| (n * t / pieces, n * (t + 1) / pieces);
        for t in 0..pieces {
            let (rects, polys, paths) = (slice(nr, t), slice(np, t), slice(nq, t));
            if rects.0 == rects.1 && polys.0 == polys.1 && paths.0 == paths.1 {
                continue;
            }
            out.push(Unit { ci, xf, depth: depth as u32, extra: 0, kind: UnitKind::Shapes { rects, polys, paths, split_above: u64::MAX } });
        }
    }
    for (pi, pl) in cell.places.iter().enumerate() {
        if !has[pl.cell] {
            continue;
        }
        if matches!(pl.rep, Rep::One) && depth < expand && !small.is_some_and(|s| s[pl.cell]) {
            let base = xf.compose(&Xf::place(pl.x, pl.y, pl.rot, pl.flip));
            collect_units(doc, has, shapes, pl.cell, base, depth + 1, expand, pieces, small, c, out);
            continue;
        }
        let members = rep_members(&pl.rep);
        if members == 0 {
            continue;
        }
        if small.is_some_and(|s| s[pl.cell]) && grid_prunable(&pl.rep, c) {
            // one fill (Marker::mark_grid_footprint): never split
            out.push(Unit { ci, xf, depth: depth as u32, extra: 0, kind: UnitKind::Place { pi, m0: 0, m1: members } });
            continue;
        }
        let chunk = ((members + pieces as u64 - 1) / pieces as u64).max(1);
        let mut m0 = 0u64;
        while m0 < members {
            let m1 = (m0 + chunk).min(members);
            out.push(Unit { ci, xf, depth: depth as u32, extra: 0, kind: UnitKind::Place { pi, m0, m1 } });
            m0 = m1;
        }
    }
}

/// per cell, the estimated marking work of the layer under it: its own
/// records on the layer (repetition members counted) plus every
/// placement's members times the child's weight; a cycle adds nothing
/// with the prune a small cell is one bbox mark and a prunable grid of one is one fill
fn layer_weights(doc: &Doc, shapes: &[CellShapes<'_>], has: &[bool], small: Option<&[bool]>, c: i64) -> Vec<u64> {
    #[allow(clippy::too_many_arguments)]
    fn weight(doc: &Doc, ci: usize, shapes: &[CellShapes<'_>], has: &[bool], small: Option<&[bool]>, c: i64, memo: &mut Vec<Option<u64>>, open: &mut Vec<bool>) -> u64 {
        if let Some(w) = memo[ci] {
            return w;
        }
        if small.is_some_and(|s| s[ci]) {
            memo[ci] = Some(1);
            return 1;
        }
        if open[ci] {
            return 0;
        }
        open[ci] = true;
        let cell = &doc.cells[ci];
        let mut w: u64 = 0;
        for r in &shapes[ci].rects {
            w = w.saturating_add(rep_members(&r.rep));
        }
        for p in &shapes[ci].polys {
            w = w.saturating_add(rep_members(&p.rep));
        }
        for p in &shapes[ci].paths {
            w = w.saturating_add(rep_members(&p.rep));
        }
        for pl in &cell.places {
            if !has[pl.cell] {
                continue;
            }
            let child = weight(doc, pl.cell, shapes, has, small, c, memo, open);
            if small.is_some_and(|s| s[pl.cell]) && grid_prunable(&pl.rep, c) {
                w = w.saturating_add(1);
            } else {
                w = w.saturating_add(rep_members(&pl.rep).saturating_mul(child.max(1)));
            }
        }
        open[ci] = false;
        memo[ci] = Some(w);
        w
    }
    let mut memo = vec![None; doc.cells.len()];
    let mut open = vec![false; doc.cells.len()];
    for ci in 0..doc.cells.len() {
        weight(doc, ci, shapes, has, small, c, &mut memo, &mut open);
    }
    memo.into_iter().map(|w| w.unwrap_or(0)).collect()
}

/// a cell's units under `xf`, each of at most `budget` estimated work
/// where the hierarchy allows: its own records in slices, a record
/// with more members than the budget (on the per-member path) in
/// member ranges of its own (Members), a plain placement heavier than
/// the budget descended into (to MAX_EXPAND_DEPTH), a Grid/Pts
/// placement of a heavy child with few members (EXPAND_MEMBERS_MAX)
/// expanded member by member, any other placement's members in
/// ranges sized by the child's weight. `c` is the cell (dbu): the
/// closed-form question of rect_splits.
#[allow(clippy::too_many_arguments)]
fn collect_units_weighted(
    doc: &Doc,
    has: &[bool],
    shapes: &[CellShapes<'_>],
    ci: usize,
    xf: Xf,
    depth: usize,
    budget: u64,
    weights: &[u64],
    c: i64,
    small: Option<&[bool]>,
    out: &mut Vec<Unit>,
) {
    let cell = &doc.cells[ci];
    let d = depth as u32;
    // own records: the giant ones (more members than the budget on the
    // per-member path) as member ranges of their own, the rest sliced
    let mut own: u64 = 0;
    let mut giants: Vec<(u8, usize, u64)> = Vec::new();
    for (idx, r) in shapes[ci].rects.iter().enumerate() {
        if rect_splits(r, &xf, c, budget) {
            giants.push((0, idx, rep_members(&r.rep)));
        } else {
            own = own.saturating_add(rep_members(&r.rep));
        }
    }
    for (idx, p) in shapes[ci].polys.iter().enumerate() {
        if poly_splits(p, budget) {
            giants.push((1, idx, rep_members(&p.rep)));
        } else {
            own = own.saturating_add(rep_members(&p.rep));
        }
    }
    for (idx, p) in shapes[ci].paths.iter().enumerate() {
        if path_splits(p, budget) {
            giants.push((2, idx, rep_members(&p.rep)));
        } else {
            own = own.saturating_add(rep_members(&p.rep));
        }
    }
    if own > 0 {
        let pieces = ((own + budget - 1) / budget).clamp(1, 4096) as usize;
        let (nr, np, nq) = (shapes[ci].rects.len(), shapes[ci].polys.len(), shapes[ci].paths.len());
        let slice = |n: usize, t: usize| (n * t / pieces, n * (t + 1) / pieces);
        for t in 0..pieces {
            let (rects, polys, paths) = (slice(nr, t), slice(np, t), slice(nq, t));
            if rects.0 == rects.1 && polys.0 == polys.1 && paths.0 == paths.1 {
                continue;
            }
            out.push(Unit {
                ci,
                xf,
                depth: d,
                extra: 0,
                kind: UnitKind::Shapes { rects, polys, paths, split_above: budget },
            });
        }
    }
    for (shape, idx, members) in giants {
        let per = budget.max((members + MEMBERS_UNITS_MAX - 1) / MEMBERS_UNITS_MAX).max(1);
        let mut m0 = 0u64;
        while m0 < members {
            let m1 = (m0 + per).min(members);
            out.push(Unit { ci, xf, depth: d, extra: 0, kind: UnitKind::Members { shape, idx, m0, m1 } });
            m0 = m1;
        }
    }
    for (pi, pl) in cell.places.iter().enumerate() {
        if !has[pl.cell] {
            continue;
        }
        let members = rep_members(&pl.rep);
        if members == 0 {
            continue;
        }
        let child = weights[pl.cell].max(1);
        let small_child = small.is_some_and(|s| s[pl.cell]);
        if small_child && grid_prunable(&pl.rep, c) {
            // one fill (Marker::mark_grid_footprint): never split
            out.push(Unit { ci, xf, depth: d, extra: 0, kind: UnitKind::Place { pi, m0: 0, m1: members } });
            continue;
        }
        if matches!(pl.rep, Rep::One) {
            if child > budget && depth < MAX_EXPAND_DEPTH && !small_child {
                let base = xf.compose(&Xf::place(pl.x, pl.y, pl.rot, pl.flip));
                collect_units_weighted(doc, has, shapes, pl.cell, base, depth + 1, budget, weights, c, small, out);
                continue;
            }
            out.push(Unit { ci, xf, depth: d, extra: 0, kind: UnitKind::Place { pi, m0: 0, m1: 1 } });
            continue;
        }
        if child > budget && members <= EXPAND_MEMBERS_MAX && depth < MAX_EXPAND_DEPTH && !small_child {
            // a few members of a heavy child: each member's units of
            // its own, so a giant record inside reaches its Members
            // units; the member's charge (the walk charges each Grid/
            // Pts member one) rides on the member's first unit
            for k in 0..members {
                let (dx, dy) = rep_member(&pl.rep, k);
                let base = xf.compose(&Xf::place(pl.x + dx, pl.y + dy, pl.rot, pl.flip));
                let start = out.len();
                collect_units_weighted(doc, has, shapes, pl.cell, base, depth + 1, budget, weights, c, small, out);
                if out.len() > start {
                    out[start].extra += 1;
                } else {
                    out.push(Unit { ci, xf, depth: d, extra: 0, kind: UnitKind::Place { pi, m0: k, m1: k + 1 } });
                }
            }
            continue;
        }
        let per = (budget / child).max(1);
        let mut m0 = 0u64;
        while m0 < members {
            let m1 = (m0 + per).min(members);
            out.push(Unit { ci, xf, depth: d, extra: 0, kind: UnitKind::Place { pi, m0, m1 } });
            m0 = m1;
        }
    }
}

/// the units of a layer for `jobs` threads. Balanced (the default,
/// 2026-09-16): each unit holds at most about 1/(4 x jobs) of the
/// layer's estimated work (`layer_weights`), so one heavy block among
/// many light placements is still spread over the threads (field: a
/// 150 MB chip's top had enough placements that the count-based split
/// below stopped expanding, and its heaviest block ran on one thread
/// for minutes). Count-based (`balanced` false): the top cell's own
/// units, then one level deeper through plain placements (at most
/// four) while there are fewer than 4 x jobs of them.
fn units_for(doc: &Doc, has: &[bool], shapes: &[CellShapes<'_>], jobs: usize, balanced: bool, c: i64, small: Option<&[bool]>) -> Vec<Unit> {
    let target = jobs.max(1) * 4;
    let mut units = Vec::new();
    if balanced {
        let weights = layer_weights(doc, shapes, has, small, c);
        let budget = (weights[doc.top] / target as u64).max(1);
        collect_units_weighted(doc, has, shapes, doc.top, Xf::identity(), 0, budget, &weights, c, small, &mut units);
        if !units.is_empty() {
            return units;
        }
    }
    for expand in 0..=4 {
        units.clear();
        collect_units(doc, has, shapes, doc.top, Xf::identity(), 0, expand, target, small, c, &mut units);
        if units.len() >= target {
            break;
        }
    }
    units
}

/// build the planes of one layer (status + one pyramid per placement
/// depth): the units are marked by `jobs` threads into the layer's
/// shared level-0 planes (field 2026-09-14: one thread per layer kept
/// two cores busy for 41 s on the real chip's two populated layers)
#[allow(clippy::too_many_arguments)]
fn build_layer(
    doc: &Doc,
    key: (u32, u32),
    shapes: &[CellShapes<'_>],
    has: &[bool],
    origin: (i64, i64),
    cell_dbu: i64,
    w: u32,
    h: u32,
    max_work: u64,
    jobs: usize,
    progress: Option<fn(&str)>,
    balanced: bool,
    prune: Option<(&[Option<Win>], &[bool])>,
) -> (Layer, u64) {
    let layer_with = |status: u8, work: u64, planes: Vec<Plane>| Layer { layer: key.0, dt: key.1, status, work, planes };
    if !has[doc.top] {
        return (layer_with(STATUS_EMPTY, 0, Vec::new()), 0);
    }
    let prepared = std::time::Instant::now();
    let depth_mask = prune.map(|_| layer_depth_masks(doc, shapes, has));
    let prune = prune.map(|(bboxes, small)| Prune { bboxes, small, depth_mask: depth_mask.as_deref().unwrap_or(&[]) });
    let units = units_for(doc, has, shapes, jobs, balanced, cell_dbu, prune.map(|p| p.small));
    let planes = Planes::new(w, h, layer_max_depth(doc, shapes, has));
    let threads = jobs.max(1).min(units.len()).max(1);
    let shared = std::sync::atomic::AtomicU64::new(0);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let completed = std::sync::atomic::AtomicUsize::new(0);
    if let Some(log) = progress {
        if prepared.elapsed().as_secs_f64() >= 0.5 {
            log(&format!("{}/{} prepared: {} units workers={} ({:.3}s)",
                key.0, key.1, units.len(), threads, prepared.elapsed().as_secs_f64()));
        }
    }
    let markers: Vec<Marker> = std::thread::scope(|s| {
        // Keep the sender inside the scope body: unwinding a failed
        // worker also disconnects the heartbeat before scope joins it.
        let (finished, finish) = std::sync::mpsc::sync_channel::<()>(1);
        if let Some(log) = progress {
            // Wake immediately when marking ends. Sleeping then joining
            // used to add up to 250 ms for EACH populated layer.
            let (units, completed, shared) = (&units, &completed, &shared);
            s.spawn(move || {
                use std::sync::atomic::Ordering::Relaxed;
                let t0 = std::time::Instant::now();
                while matches!(finish.recv_timeout(std::time::Duration::from_secs(PROGRESS_EVERY_S)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout))
                {
                    let work = if threads > 1 {
                        format!(" work {:.2}G", shared.load(Relaxed) as f64 / 1e9)
                    } else {
                        String::new()
                    };
                    log(&format!(
                        "{}/{} marking: {}/{} units workers={}{} ({}s)",
                        key.0, key.1, completed.load(Relaxed), units.len(),
                        threads, work, t0.elapsed().as_secs()
                    ));
                }
            });
        }
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let (units, next, shared, planes, completed) = (&units, &next, &shared, &planes, &completed);
                std::thread::Builder::new()
                    .stack_size(64 << 20)
                    .spawn_scoped(s, move || {
                        use std::sync::atomic::Ordering::Relaxed;
                        let mut m = Marker {
                            doc,
                            shapes,
                            has,
                            ox: origin.0,
                            oy: origin.1,
                            c: cell_dbu,
                            w,
                            h,
                            planes,
                            depth: 0,
                            work: 0,
                            max_work,
                            over: false,
                            paths_skipped: 0,
                            shared: if threads > 1 { Some(shared) } else { None },
                            unflushed: 0,
                            prune,
                        };
                        loop {
                            let k = next.fetch_add(1, Relaxed);
                            if k >= units.len() || !m.run_unit(&units[k]) {
                                break;
                            }
                            completed.fetch_add(1, Relaxed);
                        }
                        if let Some(shared) = m.shared {
                            m.flush(shared);
                        }
                        m
                    })
                    .expect("occupancy worker")
            })
            .collect();
        let markers: Vec<Marker> = handles.into_iter().map(|h| h.join().expect("occupancy worker")).collect();
        let _ = finished.send(());
        markers
    });
    let mut iter = markers.into_iter();
    let mut m = iter.next().expect("one marker");
    for other in iter {
        m.work = m.work.saturating_add(other.work);
        m.paths_skipped += other.paths_skipped;
        m.over |= other.over;
    }
    if m.over || m.work > max_work {
        return (layer_with(STATUS_NONE_WORK, m.work, Vec::new()), m.paths_skipped);
    }
    if m.paths_skipped > 0 {
        // a shape the summary cannot represent: no summary for the
        // layer (the page path draws it, or refuses it loudly), never
        // a summary with the shape missing
        return (layer_with(STATUS_NONE_UNSUPPORTED, m.work, Vec::new()), m.paths_skipped);
    }
    let mut out = Vec::new();
    for (depth, plane) in planes.by_depth.iter().enumerate() {
        if !plane.any() {
            // no shape at this depth (or none with positive area)
            continue;
        }
        let mut levels = vec![plane.to_level()];
        while levels.last().unwrap().w > TOP_GRID || levels.last().unwrap().h > TOP_GRID {
            let next = levels.last().unwrap().pool();
            levels.push(next);
        }
        out.push(Plane { depth: depth as u8, levels });
    }
    if out.is_empty() {
        // records without positive area (zero-width rects, degenerate
        // polygons): nothing to draw, no bitmap
        return (layer_with(STATUS_EMPTY, m.work, Vec::new()), 0);
    }
    (layer_with(STATUS_OK, m.work, out), 0)
}

/// build every layer's planes; each layer's marking runs on `jobs`
/// threads into shared planes (memory: one level-0 plane per placement
/// depth of the layer being marked)
pub fn build(doc: &Doc, src_size: u64, src_mtime: u64, opts: &Opts) -> Result<Occupancy, String> {
    if opts.base_um < 0.0 || !opts.base_um.is_finite() {
        return Err(format!("occupancy: base cell must be positive or 0 (auto), got {}", opts.base_um));
    }
    let bboxes = cell_bboxes(doc);
    let bbox = bboxes[doc.top].unwrap_or((0, 0, 0, 0));
    let span_x = (bbox.2 as i128 - bbox.0 as i128).max(0);
    let span_y = (bbox.3 as i128 - bbox.1 as i128).max(0);
    let base_um = if opts.base_um > 0.0 {
        opts.base_um
    } else {
        auto_base_um_for_span(span_x.max(span_y) as f64 / doc.unit)
    };
    let cell_dbu = ((base_um * doc.unit).round() as i64).max(1);
    let gw = ceil_div(span_x, cell_dbu as i128);
    let gh = ceil_div(span_y, cell_dbu as i128);
    let cells = (gw as u128) * (gh as u128);
    let top = doc.cells[doc.top].name.clone();
    let mut occ = Occupancy {
        unit: doc.unit,
        src_size,
        src_mtime,
        top,
        cell_dbu,
        bbox,
        w: 0,
        h: 0,
        n_levels: 1,
        layers: Vec::new(),
        paths_skipped: 0,
    };
    if cells > opts.max_cells as u128 || gw > u32::MAX as i128 || gh > u32::MAX as i128 {
        occ.layers = doc
            .layer_order
            .iter()
            .map(|&(l, d)| Layer { layer: l, dt: d, status: STATUS_NONE_CELLS, work: 0, planes: Vec::new() })
            .collect();
        return Ok(occ);
    }
    let (w, h) = (gw as u32, gh as u32);
    occ.w = w;
    occ.h = h;
    occ.n_levels = level_count(w, h);
    let per_layer = layer_bytes(w, h);
    let fit = if per_layer == 0 { doc.layer_order.len() } else { (opts.max_bytes / per_layer) as usize };
    // layers one after another, each marked by `jobs` threads; the
    // byte limit counts the pyramids written (one per plane; empty
    // and none:* layers take no room)
    let mut skipped = 0u64;
    let mut slot = 0usize;
    let mut layers = Vec::with_capacity(doc.layer_order.len());
    let indexing = std::time::Instant::now();
    if let Some(log) = opts.progress {
        log("grouping records by layer");
    }
    let mut index = index_shapes(doc);
    // Opts::prune: the cells whose whole subtree fits one grid cell
    let small: Vec<bool> = bboxes
        .iter()
        .map(|b| opts.prune && b.is_some_and(|b| b.2 - b.0 <= cell_dbu && b.3 - b.1 <= cell_dbu))
        .collect();
    if let Some(log) = opts.progress {
        log(&format!(
            "grouped records in {:.3}s; prune {} ({} of {} cells fit a {} dbu cell)",
            indexing.elapsed().as_secs_f64(),
            if opts.prune { "on" } else { "off" },
            small.iter().filter(|&&s| s).count(),
            small.len(),
            cell_dbu
        ));
    }
    for &key in &doc.layer_order {
        let shapes = take_layer_shapes(&mut index, key, doc.cells.len());
        let has = layer_presence(doc, &shapes);
        if !has[doc.top] {
            layers.push(Layer { layer: key.0, dt: key.1, status: STATUS_EMPTY, work: 0, planes: Vec::new() });
            continue;
        }
        if slot >= fit {
            layers.push(Layer { layer: key.0, dt: key.1, status: STATUS_NONE_SIZE, work: 0, planes: Vec::new() });
            continue;
        }
        let t0 = std::time::Instant::now();
        let (layer, sk) = build_layer(doc, key, &shapes, &has, (bbox.0, bbox.1), cell_dbu, w, h, opts.max_work, opts.jobs, opts.progress, opts.balanced_units,
            if opts.prune { Some((bboxes.as_slice(), small.as_slice())) } else { None });
        skipped += sk;
        if layer.status == STATUS_OK {
            slot += layer.planes.len();
        }
        if let Some(log) = opts.progress {
            let secs = t0.elapsed().as_secs_f64();
            if layer.status == STATUS_OK && secs >= 0.5 {
                let cells: u64 = layer.planes.iter().map(|p| p.levels.first().map_or(0, |l| l.count())).sum();
                let depths: Vec<String> = layer.planes.iter().map(|p| p.depth.to_string()).collect();
                log(&format!(
                    "{}/{} ok planes={} cells={} work={} ({:.1}s)",
                    key.0,
                    key.1,
                    depths.join(","),
                    cells,
                    layer.work,
                    secs
                ));
            } else if layer.status != STATUS_OK && layer.status != STATUS_EMPTY {
                log(&format!("{}/{} {} work={} ({:.1}s)", key.0, key.1, status_text(layer.status), layer.work, secs));
            }
        }
        layers.push(layer);
    }
    occ.layers = layers;
    occ.paths_skipped = skipped;
    Ok(occ)
}

// ---------------------------------------------------------- serialize

const HEADER_FIXED: usize = 8 + 4 + 8 + 8 + 8 + 8 + 32 + 4 + 4 + 2;
/// layer u32, dt u32, status u8, work u64, n_planes u8
const LAYER_FIXED: usize = 4 + 4 + 1 + 8 + 1;
/// a version-1 layer entry: no plane count
const LAYER_FIXED_V1: usize = 4 + 4 + 1 + 8;
/// depth u8
const PLANE_FIXED: usize = 1;
const LEVEL_ENTRY: usize = 4 + 4 + 8 + 8;

fn put32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put64(o: &mut Vec<u8>, v: u64) {
    o.extend_from_slice(&v.to_le_bytes());
}

fn planes_written(layer: &Layer) -> &[Plane] {
    if layer.status == STATUS_OK {
        &layer.planes
    } else {
        &[]
    }
}

pub fn write_ovo(occ: &Occupancy) -> Vec<u8> {
    let top = occ.top.as_bytes();
    let top_len = top.len().min(u16::MAX as usize);
    let nl = occ.layers.len();
    let n_levels = occ.n_levels as usize;
    let table_len: usize = occ
        .layers
        .iter()
        .map(|l| LAYER_FIXED + planes_written(l).len() * (PLANE_FIXED + n_levels * LEVEL_ENTRY))
        .sum();
    let body_start = HEADER_FIXED + top_len + table_len;
    let mut out = Vec::with_capacity(body_start);
    out.extend_from_slice(MAGIC);
    put32(&mut out, VERSION);
    out.extend_from_slice(&occ.unit.to_le_bytes());
    put64(&mut out, occ.src_size);
    put64(&mut out, occ.src_mtime);
    out.extend_from_slice(&occ.cell_dbu.to_le_bytes());
    for v in [occ.bbox.0, occ.bbox.1, occ.bbox.2, occ.bbox.3] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    put32(&mut out, occ.n_levels);
    put32(&mut out, nl as u32);
    out.extend_from_slice(&(top_len as u16).to_le_bytes());
    out.extend_from_slice(&top[..top_len]);
    let mut body: Vec<u8> = Vec::new();
    for layer in &occ.layers {
        put32(&mut out, layer.layer);
        put32(&mut out, layer.dt);
        out.push(layer.status);
        put64(&mut out, layer.work);
        let planes = planes_written(layer);
        out.push(planes.len() as u8);
        for plane in planes {
            out.push(plane.depth);
            for lv in 0..n_levels {
                match plane.levels.get(lv) {
                    Some(level) => {
                        put32(&mut out, level.w);
                        put32(&mut out, level.h);
                        put64(&mut out, (body_start + body.len()) as u64);
                        put64(&mut out, level.bits.len() as u64);
                        body.extend_from_slice(&level.bits);
                    }
                    None => {
                        put32(&mut out, 0);
                        put32(&mut out, 0);
                        put64(&mut out, 0);
                        put64(&mut out, 0);
                    }
                }
            }
        }
    }
    debug_assert_eq!(out.len(), body_start);
    out.extend_from_slice(&body);
    out
}

/// a version-1 file of the flattened planes (tests: the reader keeps
/// reading the files written before 2026-09-16)
#[cfg(test)]
pub fn write_ovo_v1(occ: &Occupancy) -> Vec<u8> {
    let top = occ.top.as_bytes();
    let nl = occ.layers.len();
    let n_levels = occ.n_levels as usize;
    let body_start = HEADER_FIXED + top.len() + nl * (LAYER_FIXED_V1 + n_levels * LEVEL_ENTRY);
    let mut out = Vec::with_capacity(body_start);
    out.extend_from_slice(MAGIC_V1);
    put32(&mut out, 1);
    out.extend_from_slice(&occ.unit.to_le_bytes());
    put64(&mut out, occ.src_size);
    put64(&mut out, occ.src_mtime);
    out.extend_from_slice(&occ.cell_dbu.to_le_bytes());
    for v in [occ.bbox.0, occ.bbox.1, occ.bbox.2, occ.bbox.3] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    put32(&mut out, occ.n_levels);
    put32(&mut out, nl as u32);
    out.extend_from_slice(&(top.len() as u16).to_le_bytes());
    out.extend_from_slice(top);
    let mut body: Vec<u8> = Vec::new();
    for layer in &occ.layers {
        put32(&mut out, layer.layer);
        put32(&mut out, layer.dt);
        out.push(layer.status);
        put64(&mut out, layer.work);
        for lv in 0..n_levels {
            match layer.level(lv) {
                Some(level) if layer.status == STATUS_OK => {
                    put32(&mut out, level.w);
                    put32(&mut out, level.h);
                    put64(&mut out, (body_start + body.len()) as u64);
                    put64(&mut out, level.bits.len() as u64);
                    body.extend_from_slice(&level.bits);
                }
                _ => {
                    put32(&mut out, 0);
                    put32(&mut out, 0);
                    put64(&mut out, 0);
                    put64(&mut out, 0);
                }
            }
        }
    }
    out.extend_from_slice(&body);
    out
}

// --------------------------------------------------------------- read

#[derive(Clone, Debug)]
pub struct LevelEntry {
    pub w: u32,
    pub h: u32,
    pub off: u64,
    pub len: u64,
}

#[derive(Clone, Debug)]
pub struct PlaneEntry {
    /// placement depth (DEPTH_CAP = the cap and deeper, DEPTH_ALL = a
    /// version-1 flattening)
    pub depth: u8,
    pub levels: Vec<LevelEntry>,
}

#[derive(Clone, Debug)]
pub struct LayerEntry {
    pub layer: u32,
    pub dt: u32,
    pub status: u8,
    pub work: u64,
    /// ascending by depth; empty unless status ok
    pub planes: Vec<PlaneEntry>,
}

/// a validated design.ovo (structure only; `validate_against` checks
/// the identity against the cache's design.ovm)
pub struct OvoFile {
    data: floe_ovm::Backing,
    pub version: u32,
    pub unit: f64,
    pub src_size: u64,
    pub src_mtime: u64,
    pub cell_dbu: i64,
    pub bbox: (i64, i64, i64, i64),
    pub w: u32,
    pub h: u32,
    pub n_levels: u32,
    pub top: String,
    pub layers: Vec<LayerEntry>,
}

fn g32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn g64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn gi64(b: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl std::fmt::Debug for OvoFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OvoFile(v{} top={:?} cell_dbu={} grid={}x{} levels={} layers={})",
            self.version,
            self.top,
            self.cell_dbu,
            self.w,
            self.h,
            self.n_levels,
            self.layers.len()
        )
    }
}

impl OvoFile {
    pub fn open(path: &str) -> Result<OvoFile, String> {
        let data = floe_ovm::map_file(path)?;
        OvoFile::parse(data).map_err(|e| format!("{}: {}", path, e))
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<OvoFile, String> {
        OvoFile::parse(floe_ovm::Backing::Vec(bytes))
    }

    fn parse(data: floe_ovm::Backing) -> Result<OvoFile, String> {
        let b: &[u8] = &data;
        let len = b.len();
        if len < HEADER_FIXED {
            return Err(format!("truncated occupancy file ({} bytes)", len));
        }
        let version = g32(b, 8);
        let v1 = match (&b[..8], version) {
            (m, 2) if m == MAGIC => false,
            (m, 1) if m == MAGIC_V1 => true,
            (m, v) if m == MAGIC || m == MAGIC_V1 => {
                return Err(format!("occupancy version {} (this build reads 1 and {})", v, VERSION));
            }
            _ => return Err("not an occupancy file (bad magic)".to_string()),
        };
        let unit = f64::from_le_bytes(b[12..20].try_into().unwrap());
        let src_size = g64(b, 20);
        let src_mtime = g64(b, 28);
        let cell_dbu = gi64(b, 36);
        let bbox = (gi64(b, 44), gi64(b, 52), gi64(b, 60), gi64(b, 68));
        let n_levels = g32(b, 76);
        let n_layers = g32(b, 80);
        let top_len = u16::from_le_bytes(b[84..86].try_into().unwrap()) as usize;
        if !(cell_dbu > 0) || !unit.is_finite() || !(unit > 0.0) {
            return Err("corrupt occupancy header (cell/unit)".to_string());
        }
        if n_levels == 0 || n_levels > 64 {
            return Err(format!("corrupt occupancy header (levels {})", n_levels));
        }
        if bbox.2 < bbox.0 || bbox.3 < bbox.1 {
            return Err("corrupt occupancy header (bbox)".to_string());
        }
        let gw = ceil_div(bbox.2 as i128 - bbox.0 as i128, cell_dbu as i128);
        let gh = ceil_div(bbox.3 as i128 - bbox.1 as i128, cell_dbu as i128);
        let mut o = HEADER_FIXED;
        if len < o + top_len {
            return Err("truncated occupancy file (top name)".to_string());
        }
        let top = std::str::from_utf8(&b[o..o + top_len])
            .map_err(|_| "corrupt occupancy header (top name)".to_string())?
            .to_string();
        o += top_len;
        let level_block = n_levels as usize * LEVEL_ENTRY;
        // the table's size is known up front for a version-1 file; a
        // version-2 table is walked entry by entry (planes per layer)
        let table_v1 = (n_layers as usize)
            .checked_mul(LAYER_FIXED_V1 + level_block)
            .ok_or_else(|| "corrupt occupancy header (layer table)".to_string())?;
        if v1 && len < o + table_v1 {
            return Err("truncated occupancy file (layer table)".to_string());
        }
        let mut layers = Vec::with_capacity(n_layers as usize);
        // first pass: the table (bounds only), to know where the body
        // starts; bitmaps must follow the table in table order without
        // overlap (review 2026-09-11 (2nd) P2-4: an offset into the
        // header read as a plausible bitmap)
        let mut cursor = o;
        let mut plane_counts = Vec::with_capacity(n_layers as usize);
        for k in 0..n_layers as usize {
            let fixed = if v1 { LAYER_FIXED_V1 } else { LAYER_FIXED };
            if len < cursor + fixed {
                return Err("truncated occupancy file (layer table)".to_string());
            }
            let status = b[cursor + 8];
            let n_planes = if v1 {
                1
            } else {
                b[cursor + 17] as usize
            };
            if !v1 && (status == STATUS_OK) != (n_planes > 0) {
                return Err(format!("corrupt occupancy layer {}: status {} with {} planes", k, status, n_planes));
            }
            cursor += fixed;
            let per_plane = if v1 { level_block } else { PLANE_FIXED + level_block };
            let planes_len = n_planes
                .checked_mul(per_plane)
                .ok_or_else(|| "corrupt occupancy header (layer table)".to_string())?;
            if len < cursor + planes_len {
                return Err("truncated occupancy file (layer table)".to_string());
            }
            cursor += planes_len;
            plane_counts.push(n_planes);
        }
        let body_start = cursor as u64;
        let mut prev_end = body_start;
        let (mut w, mut h) = (0u32, 0u32);
        for (k, &n_planes) in plane_counts.iter().enumerate() {
            let layer = g32(b, o);
            let dt = g32(b, o + 4);
            let status = b[o + 8];
            let work = g64(b, o + 9);
            o += if v1 { LAYER_FIXED_V1 } else { LAYER_FIXED };
            let mut planes = Vec::with_capacity(n_planes);
            let mut last_depth: Option<u8> = None;
            for p in 0..n_planes {
                let depth = if v1 {
                    DEPTH_ALL
                } else {
                    let d = b[o];
                    o += PLANE_FIXED;
                    d
                };
                if !v1 {
                    if depth > DEPTH_CAP || last_depth.is_some_and(|last| depth <= last) {
                        return Err(format!("corrupt occupancy layer {}: plane {} depth {}", k, p, depth));
                    }
                    last_depth = Some(depth);
                }
                let mut levels = Vec::with_capacity(n_levels as usize);
                let (mut ew, mut eh) = (gw, gh);
                for lv in 0..n_levels as usize {
                    let e = LevelEntry { w: g32(b, o), h: g32(b, o + 4), off: g64(b, o + 8), len: g64(b, o + 16) };
                    o += LEVEL_ENTRY;
                    if status == STATUS_OK {
                        if e.w as i128 != ew || e.h as i128 != eh {
                            return Err(format!(
                                "corrupt occupancy layer {} plane {} level {}: grid {}x{}, expected {}x{}",
                                k, p, lv, e.w, e.h, ew, eh
                            ));
                        }
                        let need = (Level::row_bytes(e.w) as u64)
                            .checked_mul(e.h as u64)
                            .ok_or_else(|| "corrupt occupancy level (size overflow)".to_string())?;
                        if e.len != need {
                            return Err(format!(
                                "corrupt occupancy layer {} plane {} level {}: {} bytes, expected {}",
                                k, p, lv, e.len, need
                            ));
                        }
                        let end = e
                            .off
                            .checked_add(e.len)
                            .ok_or_else(|| "corrupt occupancy level (offset overflow)".to_string())?;
                        if end > len as u64 {
                            return Err(format!(
                                "truncated occupancy file: layer {} plane {} level {} ends at {} of {} bytes",
                                k, p, lv, end, len
                            ));
                        }
                        if e.off < prev_end {
                            return Err(format!(
                                "corrupt occupancy layer {} plane {} level {}: bitmap offset {} {} {}",
                                k,
                                p,
                                lv,
                                e.off,
                                if e.off < body_start { "inside the header/table ending at" } else { "overlaps the previous bitmap ending at" },
                                prev_end
                            ));
                        }
                        prev_end = end;
                        if lv == 0 {
                            w = e.w;
                            h = e.h;
                        }
                    } else if e.len != 0 || e.off != 0 {
                        return Err(format!("corrupt occupancy layer {}: status {} with data", k, status));
                    }
                    ew = (ew + 1) / 2;
                    eh = (eh + 1) / 2;
                    levels.push(e);
                }
                if status == STATUS_OK {
                    planes.push(PlaneEntry { depth, levels });
                }
            }
            layers.push(LayerEntry { layer, dt, status, work, planes });
        }
        if w == 0 && h == 0 && gw <= u32::MAX as i128 && gh <= u32::MAX as i128 {
            w = gw as u32;
            h = gh as u32;
        }
        Ok(OvoFile { data, version, unit, src_size, src_mtime, cell_dbu, bbox, w, h, n_levels, top, layers })
    }

    /// whether the file holds one plane per placement depth (version
    /// 2), so a limited request depth can draw its own summary
    pub fn depth_aware(&self) -> bool {
        self.version >= 2
    }

    /// the identity the cache's marker commits: same source bytes,
    /// same top cell, same layer table in the same order
    pub fn validate_against(&self, ovm: &Ovm) -> Result<(), String> {
        if self.src_size != ovm.src_size || self.src_mtime != ovm.src_mtime {
            return Err(format!(
                "occupancy was built for source {}/{}, cache has {}/{}",
                self.src_size, self.src_mtime, ovm.src_size, ovm.src_mtime
            ));
        }
        let top = ovm.cell(ovm.top).name;
        if self.top != top {
            return Err(format!("occupancy top {:?}, cache top {:?}", self.top, top));
        }
        if self.layers.len() != ovm.n_layers as usize {
            return Err(format!("occupancy has {} layers, cache {}", self.layers.len(), ovm.n_layers));
        }
        for (k, e) in self.layers.iter().enumerate() {
            let l = ovm.layer(k as u32);
            if (l.layer, l.dt) != (e.layer, e.dt) {
                return Err(format!(
                    "occupancy layer {} is {}/{}, cache {}/{}",
                    k, e.layer, e.dt, l.layer, l.dt
                ));
            }
        }
        Ok(())
    }

    pub fn base_um(&self) -> f64 {
        self.cell_dbu as f64 / self.unit
    }

    /// (w, h, row-padded bits) of one plane's level, None unless the
    /// layer's status is ok
    pub fn plane_level(&self, k: usize, p: usize, lv: usize) -> Option<(u32, u32, &[u8])> {
        let layer = self.layers.get(k)?;
        if layer.status != STATUS_OK {
            return None;
        }
        let e = layer.planes.get(p)?.levels.get(lv)?;
        let b: &[u8] = &self.data;
        Some((e.w, e.h, &b[e.off as usize..(e.off + e.len) as usize]))
    }

    /// (w, h, row-padded bits) of the OR of the planes a request depth
    /// draws (None = unlimited) at one level: borrowed from the file
    /// when one plane is drawn, owned when several, `(0, 0, empty)`
    /// when none is (nothing of the layer at that depth); None unless
    /// the layer's status is ok and the level exists
    pub fn level_at_depth(&self, k: usize, lv: usize, depth: Option<u32>) -> Option<(u32, u32, std::borrow::Cow<'_, [u8]>)> {
        let layer = self.layers.get(k)?;
        if layer.status != STATUS_OK || !layer.planes.iter().any(|p| p.levels.len() > lv) {
            return None;
        }
        let mut acc: Option<(u32, u32, std::borrow::Cow<[u8]>)> = None;
        for (p, plane) in layer.planes.iter().enumerate() {
            if !plane_drawn_at(plane.depth, depth) {
                continue;
            }
            let Some((w, h, bits)) = self.plane_level(k, p, lv) else { continue };
            match &mut acc {
                None => acc = Some((w, h, std::borrow::Cow::Borrowed(bits))),
                Some((_, _, cow)) => {
                    let owned = cow.to_mut();
                    for (a, b) in owned.iter_mut().zip(bits.iter()) {
                        *a |= *b;
                    }
                }
            }
        }
        Some(acc.unwrap_or((0, 0, std::borrow::Cow::Borrowed(&[]))))
    }

    /// (w, h, row-padded bits) of the flattening of every plane at one
    /// level, None unless status ok
    pub fn level(&self, k: usize, lv: usize) -> Option<(u32, u32, std::borrow::Cow<'_, [u8]>)> {
        self.level_at_depth(k, lv, None)
    }

    pub fn get(&self, k: usize, lv: usize, i: u32, j: u32) -> bool {
        match self.level(k, lv) {
            Some((w, h, bits)) if i < w && j < h => {
                (bits[j as usize * Level::row_bytes(w) + (i / 8) as usize] >> (i % 8)) & 1 == 1
            }
            _ => false,
        }
    }

    pub fn count(&self, k: usize, lv: usize) -> u64 {
        match self.level(k, lv) {
            Some((_, _, bits)) => bits.iter().map(|b| b.count_ones() as u64).sum(),
            None => 0,
        }
    }

    /// cells set in one plane's level
    pub fn plane_count(&self, k: usize, p: usize, lv: usize) -> u64 {
        match self.plane_level(k, p, lv) {
            Some((_, _, bits)) => bits.iter().map(|b| b.count_ones() as u64).sum(),
            None => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_oasis::doc::{Cell, PlaceRec};
    use std::sync::Arc;

    fn doc_with(cells: Vec<Cell>, top: usize, layers: Vec<(u32, u32)>) -> Doc {
        Doc {
            unit: 1000.0,
            cells,
            top,
            layer_order: layers,
            norm_s: 0.0,
            layer_names: Default::default(),
            layer_aliases: Default::default(),
        }
    }

    fn rect(l: u32, x: i64, y: i64, w: i64, h: i64, rep: Rep) -> RectRec {
        RectRec { layer: l, dt: 0, x, y, w, h, rep }
    }

    fn cell(name: &str) -> Cell {
        Cell { name: name.to_string(), ..Default::default() }
    }

    fn opts(base_um: f64) -> Opts {
        Opts { base_um, jobs: 2, ..Opts::default() }
    }

    /// brute oracle: the open cell box meets the polygon interior with
    /// positive area (Sutherland-Hodgman clip, shoelace in f64)
    fn oracle_poly(pts: &[(i64, i64)], ox: i64, oy: i64, c: i64, i: u32, j: u32) -> bool {
        let x0 = (ox + i as i64 * c) as f64;
        let y0 = (oy + j as i64 * c) as f64;
        let x1 = x0 + c as f64;
        let y1 = y0 + c as f64;
        let mut cur: Vec<(f64, f64)> = pts.iter().map(|&(x, y)| (x as f64, y as f64)).collect();
        for side in 0..4 {
            let inside = |p: (f64, f64)| match side {
                0 => p.0 >= x0,
                1 => p.0 <= x1,
                2 => p.1 >= y0,
                _ => p.1 <= y1,
            };
            let cross = |a: (f64, f64), b: (f64, f64)| -> (f64, f64) {
                match side {
                    0 | 1 => {
                        let line = if side == 0 { x0 } else { x1 };
                        (line, a.1 + (b.1 - a.1) * (line - a.0) / (b.0 - a.0))
                    }
                    _ => {
                        let line = if side == 2 { y0 } else { y1 };
                        (a.0 + (b.0 - a.0) * (line - a.1) / (b.1 - a.1), line)
                    }
                }
            };
            let mut next = Vec::new();
            if cur.is_empty() {
                break;
            }
            let mut prev = *cur.last().unwrap();
            for &p in &cur {
                if inside(p) {
                    if !inside(prev) {
                        next.push(cross(prev, p));
                    }
                    next.push(p);
                } else if inside(prev) {
                    next.push(cross(prev, p));
                }
                prev = p;
            }
            cur = next;
        }
        let n = cur.len();
        if n < 3 {
            return false;
        }
        let mut a = 0.0;
        for k in 0..n {
            let (xa, ya) = cur[k];
            let (xb, yb) = cur[(k + 1) % n];
            a += xa * yb - xb * ya;
        }
        a.abs() > 1e-6
    }

    fn assert_poly_matches_oracle(pts: &[(i64, i64)], c: i64, w: u32, h: u32) {
        let planes = Planes::new(w, h, 0);
        let mut m = Marker {
            doc: &doc_with(vec![cell("T")], 0, vec![(1, 0)]),
            shapes: &[],
            has: &[true],
            ox: 0,
            oy: 0,
            c,
            w,
            h,
            planes: &planes,
            depth: 0,
            work: 0,
            max_work: u64::MAX,
            over: false,
            paths_skipped: 0,
            shared: None,
            unflushed: 0,
            prune: None,
        };
        let world: Vec<(i128, i128)> = pts.iter().map(|&(x, y)| (x as i128, y as i128)).collect();
        assert!(m.mark_world_poly(&world));
        let level = planes.by_depth[0].to_level();
        for j in 0..h {
            for i in 0..w {
                assert_eq!(
                    level.get(i, j),
                    oracle_poly(pts, 0, 0, c, i, j),
                    "cell ({}, {}) of {:?}",
                    i,
                    j,
                    pts
                );
            }
        }
    }

    #[test]
    fn polygon_scan_matches_the_clip_oracle_on_l_ring_and_diagonals() {
        // L shape with an empty corner larger than a cell
        assert_poly_matches_oracle(&[(0, 0), (90, 0), (90, 20), (30, 20), (30, 80), (0, 80)], 10, 10, 10);
        // ring as KLayout writes a hole: outer contour, cut, inner contour
        assert_poly_matches_oracle(
            &[
                (0, 0), (100, 0), (100, 100), (0, 100), (0, 50), (20, 50), (20, 80), (80, 80),
                (80, 20), (20, 20), (20, 50), (0, 50),
            ],
            10, 12, 12,
        );
        // diagonal band and a triangle whose vertices sit on grid lines
        assert_poly_matches_oracle(&[(0, 0), (15, 0), (100, 85), (100, 100), (85, 100), (0, 15)], 10, 11, 11);
        assert_poly_matches_oracle(&[(0, 0), (60, 0), (0, 60)], 10, 8, 8);
        // edges exactly on grid lines mark no cell outside the shape
        assert_poly_matches_oracle(&[(10, 10), (30, 10), (30, 30), (10, 30)], 10, 5, 5);
        // odd cell size: centres at half-dbu
        assert_poly_matches_oracle(&[(3, 4), (40, 9), (27, 33), (2, 20)], 7, 7, 6);
    }

    #[test]
    fn rect_grid_closed_form_equals_per_member_marking_and_diagonal_grids_stay_sparse() {
        // reviewer's counterexample (2026-09-11): cell 10, member 1x1,
        // 100x2 members along (4,4) and (4,-4) - bbox fill would light
        // 1,681 cells, the members touch 81
        let diag = rect(1, 0, 0, 1, 1, Rep::Grid { na: 100, nb: 2, va: (4, 4), vb: (4, -4) });
        let d = doc_with(vec![Cell { name: "T".into(), rects: vec![diag], ..Default::default() }], 0, vec![(1, 0)]);
        let occ = build(&d, 0, 0, &opts(0.01)).unwrap();
        assert_eq!(occ.cell_dbu, 10);
        let l0 = occ.layers[0].level(0).unwrap();
        // with the grid anchored at the bbox corner (0, -4) the members
        // touch 120 cells (the reviewer's 81 assumed cells centred on
        // the diagonal); the footprint would light 41 x 41 = 1,681
        assert_eq!((l0.w, l0.h), (41, 41));
        assert_eq!(l0.count(), 120);
        let members: Vec<RectRec> = (0..200)
            .map(|k| {
                let (i, j) = (k % 100, k / 100);
                rect(1, 4 * i + 4 * j, 4 * i - 4 * j, 1, 1, Rep::One)
            })
            .collect();
        let expanded = doc_with(
            vec![Cell { name: "T".into(), rects: members, ..Default::default() }],
            0,
            vec![(1, 0)],
        );
        let oe = build(&expanded, 0, 0, &opts(0.01)).unwrap();
        assert_eq!(oe.layers[0].level(0).unwrap(), l0);
        // axis grid with gaps narrower than a cell: closed form == members
        let tight = rect(1, 5, 5, 6, 6, Rep::Grid { na: 7, nb: 5, va: (9, 0), vb: (0, 8) });
        let loose = rect(1, 5, 5, 6, 6, Rep::Grid { na: 7, nb: 5, va: (20, 0), vb: (0, 30) });
        for r in [tight, loose] {
            let members: Vec<RectRec> = (0..35)
                .map(|k| {
                    let (i, j) = (k % 7, k / 7);
                    let Rep::Grid { va, vb, .. } = r.rep.clone() else { unreachable!() };
                    rect(1, r.x + i * va.0 + j * vb.0, r.y + i * va.1 + j * vb.1, r.w, r.h, Rep::One)
                })
                .collect();
            let a = doc_with(vec![Cell { name: "T".into(), rects: vec![r.clone()], ..Default::default() }], 0, vec![(1, 0)]);
            let b = doc_with(vec![Cell { name: "T".into(), rects: members, ..Default::default() }], 0, vec![(1, 0)]);
            let oa = build(&a, 0, 0, &opts(0.01)).unwrap();
            let ob = build(&b, 0, 0, &opts(0.01)).unwrap();
            assert_eq!(oa.layers[0].level(0), ob.layers[0].level(0));
        }
    }

    #[test]
    fn placements_rotate_mirror_and_repeat_and_the_pyramid_pools_by_or() {
        let mut child = cell("C");
        child.rects.push(rect(1, 0, 0, 30, 10, Rep::One)); // wide bar
        let mut top = cell("T");
        top.places.push(PlaceRec { cell: 1, x: 0, y: 0, rot: 0, flip: false, rep: Rep::One });
        top.places.push(PlaceRec { cell: 1, x: 100, y: 100, rot: 1, flip: false, rep: Rep::One });
        top.places.push(PlaceRec { cell: 1, x: 200, y: 0, rot: 0, flip: true, rep: Rep::Pts(Arc::from(vec![(0, 0), (0, 60)])) });
        top.places.push(PlaceRec { cell: 1, x: 0, y: 200, rot: 0, flip: false, rep: Rep::Grid { na: 3, nb: 1, va: (50, 0), vb: (0, 0) } });
        let d = doc_with(vec![top, child], 0, vec![(1, 0)]);
        let occ = build(&d, 7, 9, &opts(0.01)).unwrap();
        // every placement is one level down: a single plane of depth 1
        assert_eq!(occ.layers[0].planes.iter().map(|p| p.depth).collect::<Vec<_>>(), vec![1]);
        let l0 = occ.layers[0].level(0).unwrap();
        assert_eq!(occ.cell_dbu, 10);
        // the grid is anchored at the bbox corner (the mirrored bar
        // pulls y0 to -10), so address cells by world point
        let (ox, oy) = (occ.bbox.0, occ.bbox.1);
        assert_eq!((ox, oy), (0, -10));
        let cellf = |x: i64, y: i64| l0.get(((x - ox) / 10) as u32, ((y - oy) / 10) as u32);
        // (0,0) bar: x in [0,30), y in [0,10)
        assert!(cellf(5, 5) && cellf(25, 5) && !cellf(35, 5) && !cellf(5, 15));
        // rotated 90 at (100,100): x in [90,100], y in [100,130)
        assert!(cellf(95, 105) && cellf(95, 125) && !cellf(105, 105));
        // mirrored at (200,0): y in (-10, 0] -> cells y -10..0 ; second member at y +60
        assert!(cellf(205, -5) && cellf(225, -5) && cellf(205, 55));
        // 3-member x grid at y=200
        assert!(cellf(5, 205) && cellf(55, 205) && cellf(105, 205) && !cellf(35, 205));
        // pyramid: every set level-0 cell lights its parent, and only those
        let levels: Vec<Level> = (0..occ.n_levels as usize).map(|lv| occ.layers[0].level(lv).unwrap()).collect();
        for (lv, pair) in levels.windows(2).enumerate() {
            let (a, b) = (&pair[0], &pair[1]);
            for j in 0..b.h {
                for i in 0..b.w {
                    let any = a.get(2 * i, 2 * j) || a.get(2 * i + 1, 2 * j) || a.get(2 * i, 2 * j + 1) || a.get(2 * i + 1, 2 * j + 1);
                    assert_eq!(b.get(i, j), any, "level {} cell {},{}", lv + 1, i, j);
                }
            }
        }
        assert_eq!(occ.n_levels as usize, occ.layers[0].planes[0].levels.len());
        assert!(levels.last().unwrap().w <= TOP_GRID);
    }

    #[test]
    fn paths_take_the_raster_hull_and_zero_area_shapes_mark_nothing() {
        let mut top = cell("T");
        top.paths.push(PathRec { layer: 1, dt: 0, pts: vec![(0, 5), (50, 5)], hw: 3, es: 0, ee: 0, rep: Rep::One });
        top.paths.push(PathRec { layer: 1, dt: 0, pts: vec![(0, 50), (30, 80)], hw: 2, es: 0, ee: 0, rep: Rep::One });
        top.paths.push(PathRec { layer: 1, dt: 0, pts: vec![(80, 80), (90, 80)], hw: 0, es: 0, ee: 0, rep: Rep::One });
        top.rects.push(rect(1, 80, 0, 0, 20, Rep::One));
        top.polys.push(PolyRec { layer: 1, dt: 0, pts: vec![(60, 60), (70, 70), (65, 65)], rep: Rep::One });
        let d = doc_with(vec![top], 0, vec![(1, 0)]);
        let occ = build(&d, 0, 0, &opts(0.01)).unwrap();
        let l0 = occ.layers[0].level(0).unwrap();
        let (ox, oy) = (occ.bbox.0, occ.bbox.1);
        let cellf = |x: i64, y: i64| l0.get(((x - ox) / 10) as u32, ((y - oy) / 10) as u32);
        assert!(cellf(5, 5) && cellf(45, 5) && !cellf(5, 15));
        assert!(cellf(5, 55) && cellf(25, 75) && !cellf(5, 75));
        assert!(!cellf(85, 85) && !cellf(85, 5) && !cellf(65, 65));
        assert_eq!(occ.paths_skipped, 0);
    }

    #[test]
    fn a_huge_placement_array_is_charged_member_by_member_without_an_offset_vector() {
        // review 2026-09-11 (2nd) P1-1: 100k x 100k members (10^10)
        // stay inside a large budget's pre-charge, so the offset
        // vector alone asked for 160 GB; now each member is charged
        // and walked as it is enumerated, so the over-budget stop
        // lands within one member of the budget
        let mut child = cell("C");
        child.rects.push(rect(1, 0, 0, 5, 5, Rep::One));
        let mut top = cell("T");
        top.places.push(PlaceRec {
            cell: 1,
            x: 0,
            y: 0,
            rot: 0,
            flip: false,
            rep: Rep::Grid { na: 100_000, nb: 100_000, va: (1, 0), vb: (0, 0) },
        });
        let d = doc_with(vec![top, child], 0, vec![(1, 0)]);
        let budget = 5_000u64;
        let occ = build(&d, 0, 0, &Opts { base_um: 0.01, max_work: budget, prune: false, ..Opts::default() }).unwrap();
        assert_eq!(occ.layers[0].status, STATUS_NONE_WORK);
        assert!(occ.layers[0].work <= budget + 2, "charged {} for a budget of {}", occ.layers[0].work, budget);
        let pts: Vec<(i64, i64)> = (0..20_000).map(|k| (k * 10, 0)).collect();
        let mut top = cell("T");
        top.places.push(PlaceRec { cell: 1, x: 0, y: 0, rot: 0, flip: false, rep: Rep::Pts(Arc::from(pts)) });
        let mut child = cell("C");
        child.rects.push(rect(1, 0, 0, 5, 5, Rep::One));
        let d = doc_with(vec![top, child], 0, vec![(1, 0)]);
        let occ = build(&d, 0, 0, &Opts { base_um: 0.01, max_work: budget, prune: false, ..Opts::default() }).unwrap();
        assert_eq!(occ.layers[0].status, STATUS_NONE_WORK);
        assert!(occ.layers[0].work <= budget + 2);
    }

    #[test]
    fn marking_in_parallel_matches_one_thread_bit_for_bit() {
        // a grid and a point list of a child with its own grid of
        // rects, plus a record of the top's own: the units split the
        // members over the threads and the merged file is the one a
        // single thread writes (bits and work)
        let make_child = || {
            let mut child = cell("C");
            child.rects.push(rect(1, 0, 0, 4, 4, Rep::One));
            child.rects.push(rect(1, 0, 0, 1, 1, Rep::Grid { na: 5, nb: 5, va: (30, 0), vb: (0, 30) }));
            child.polys.push(PolyRec { layer: 1, dt: 0, pts: vec![(10, 0), (30, 0), (30, 25)], rep: Rep::One });
            child
        };
        let mut top = cell("T");
        top.rects.push(rect(1, 900, 900, 50, 50, Rep::One));
        top.places.push(PlaceRec { cell: 1, x: 100, y: 100, rot: 0, flip: false, rep: Rep::Grid { na: 37, nb: 11, va: (20, 0), vb: (0, 20) } });
        let pts: Vec<(i64, i64)> = (0..23).map(|k| (k * 15, (k % 3) * 40)).collect();
        top.places.push(PlaceRec { cell: 1, x: 0, y: 700, rot: 2, flip: true, rep: Rep::Pts(Arc::from(pts)) });
        let d = doc_with(vec![top, make_child()], 0, vec![(1, 0)]);
        let one = build(&d, 1, 2, &Opts { base_um: 0.01, jobs: 1, ..Opts::default() }).unwrap();
        let many = build(&d, 1, 2, &Opts { base_um: 0.01, jobs: 3, ..Opts::default() }).unwrap();
        assert_eq!(one.layers[0].status, STATUS_OK);
        assert!(one.layers[0].work > 37 * 11 + 23);
        assert_eq!(one.layers[0].work, many.layers[0].work);
        assert_eq!(write_ovo(&one), write_ovo(&many));
        let shapes = take_layer_shapes(&mut index_shapes(&d), (1, 0), d.cells.len());
        let has = layer_presence(&d, &shapes);
        assert!(units_for(&d, &has, &shapes, 3, false, 10, None).len() >= 12);
        assert!(units_for(&d, &has, &shapes, 3, true, 10, None).len() >= 12);
        // a top holding one die placement: the units come from below
        let mut leaf = cell("B");
        leaf.rects.push(rect(1, 0, 0, 7, 3, Rep::One));
        leaf.rects.push(rect(1, 50, 50, 7, 3, Rep::One));
        leaf.places.push(PlaceRec { cell: 3, x: 200, y: 0, rot: 0, flip: false, rep: Rep::Grid { na: 9, nb: 2, va: (40, 0), vb: (0, 40) } });
        let mut die = cell("A");
        die.places.push(PlaceRec { cell: 2, x: 5, y: 5, rot: 1, flip: false, rep: Rep::One });
        let mut top = cell("T");
        top.places.push(PlaceRec { cell: 1, x: 0, y: 0, rot: 0, flip: false, rep: Rep::One });
        let d = doc_with(vec![top, die, leaf, make_child()], 0, vec![(1, 0)]);
        let shapes = take_layer_shapes(&mut index_shapes(&d), (1, 0), d.cells.len());
        let has = layer_presence(&d, &shapes);
        for balanced in [false, true] {
            let units = units_for(&d, &has, &shapes, 3, balanced, 10, None);
            assert!(units.iter().all(|u| u.ci == 2), "units should sit in the leaf");
            assert!(units.len() >= 3, "{} units", units.len());
        }
        let one = build(&d, 1, 2, &Opts { base_um: 0.01, jobs: 1, ..Opts::default() }).unwrap();
        let many = build(&d, 1, 2, &Opts { base_um: 0.01, jobs: 3, ..Opts::default() }).unwrap();
        assert_eq!(write_ovo(&one), write_ovo(&many));
        assert!(one.layers[0].level(0).unwrap().count() > 20);
    }

    #[test]
    fn concurrent_overlapping_spans_preserve_bits_and_empty_gaps() {
        // Simultaneous first writes and repeated saturated writes, including
        // complete interior words, partial end words and row padding.
        let bits = SharedBits::new(259, 7);
        let spans: Vec<_> = (0..12u32).flat_map(|t| {
            (0..40u32).map(move |k| {
                let row = (t + k) % 7;
                let lo = (t * 13 + k * 7) % 190;
                (row, lo, (lo + k * 3).min(250))
            })
        }).collect();
        let mut expected = vec![false; 259 * 7];
        for &(row, lo, hi) in &spans {
            for x in lo..=hi {
                expected[(row * 259 + x) as usize] = true;
            }
        }
        std::thread::scope(|s| {
            for chunk in spans.chunks(40) {
                let bits = &bits;
                s.spawn(move || {
                    for _ in 0..64 {
                        for &(row, lo, hi) in chunk {
                            bits.set_span(row, lo, hi);
                        }
                    }
                });
            }
        });
        let level = bits.to_level();
        for row in 0..7 {
            for x in 0..259 {
                assert_eq!(level.get(x, row), expected[(row * 259 + x) as usize]);
            }
        }
        assert_eq!(level.count(), expected.iter().filter(|&&b| b).count() as u64);
    }

    #[test]
    fn a_record_repetition_is_split_by_members_and_the_file_stays_the_same() {
        // 2026-09-16: a record's own Grid/Pts repetition ran on one
        // thread however many there were (probe: 16.8 M members as one
        // record 0.41 s at jobs 1 and 0.39 s at jobs 12; the same
        // members as a placement repetition 0.57 -> 0.15 s). Giant
        // records - rect, polygon and path, Grid and Pts, under
        // rotation, mirroring and depth, placed plainly and inside a
        // Pts / 2 x 2 Grid placement of a heavy child - spread over
        // Members units. The bits and the work are those of one
        // thread, of the count-based units and of any thread count; a
        // closed-form rect grid stays one fill; a zero-area rect and a
        // zero-width path charge nothing either way
        let c = 10i64; // the 0.01 um cell at unit 1000
        let mut leaf = cell("LEAF");
        // per-member rect grid: 6 x 6 on a 40 pitch (gap 34 > cell)
        leaf.rects.push(rect(1, 0, 0, 6, 6, Rep::Grid { na: 400, nb: 400, va: (40, 0), vb: (0, 40) }));
        // closed-form rect grid: 6 x 6 on a 12 pitch (gap 6 < cell)
        leaf.rects.push(rect(1, 17000, 0, 6, 6, Rep::Grid { na: 150, nb: 150, va: (12, 0), vb: (0, 12) }));
        // a zero-area rect with a giant repetition: nothing, no charge
        leaf.rects.push(rect(1, 0, 17000, 0, 6, Rep::Grid { na: 300, nb: 300, va: (40, 0), vb: (0, 40) }));
        let pts: Vec<(i64, i64)> = (0..50_000i64).map(|k| ((k % 250) * 40, 17000 + (k / 250) * 40)).collect();
        leaf.polys.push(PolyRec { layer: 1, dt: 0, pts: vec![(0, 0), (8, 0), (0, 8)], rep: Rep::Pts(pts.into()) });
        leaf.paths.push(PathRec {
            layer: 1, dt: 0, pts: vec![(17000, 17000), (17030, 17000)], hw: 3, es: 0, ee: 0,
            rep: Rep::Grid { na: 250, nb: 250, va: (40, 0), vb: (0, 40) },
        });
        leaf.paths.push(PathRec {
            layer: 1, dt: 0, pts: vec![(0, 0), (30, 0)], hw: 0, es: 0, ee: 0,
            rep: Rep::Pts((0..3000i64).map(|k| (k * 7, k * 3)).collect()),
        });
        let mut heavy = cell("HEAVY");
        heavy.rects.push(rect(1, 0, 0, 6, 6, Rep::Grid { na: 400, nb: 400, va: (40, 0), vb: (0, 40) }));
        heavy.polys.push(PolyRec {
            layer: 1, dt: 0, pts: vec![(0, 0), (8, 0), (4, 8)],
            rep: Rep::Grid { na: 80, nb: 80, va: (0, 40), vb: (40, 0) },
        });
        let mut top = cell("T");
        top.places.push(PlaceRec { cell: 1, x: 5000, y: 7000, rot: 1, flip: true, rep: Rep::One });
        top.places.push(PlaceRec {
            cell: 1, x: 40000, y: 40000, rot: 2, flip: false,
            rep: Rep::Pts(vec![(0, 0), (30000, 0), (0, 30000)].into()),
        });
        top.places.push(PlaceRec {
            cell: 2, x: 90000, y: 0, rot: 3, flip: true,
            rep: Rep::Grid { na: 2, nb: 2, va: (30000, 0), vb: (0, 30000) },
        });
        let d = doc_with(vec![top, leaf, heavy], 0, vec![(1, 0)]);
        let shapes = take_layer_shapes(&mut index_shapes(&d), (1, 0), d.cells.len());
        let has = layer_presence(&d, &shapes);
        let units = units_for(&d, &has, &shapes, 12, true, c, None);
        let members = |shape: u8| {
            units.iter().filter(|u| matches!(u.kind, UnitKind::Members { shape: s, .. } if s == shape)).count()
        };
        assert!(members(0) >= 16 && members(1) >= 4 && members(2) >= 4,
            "rect {} poly {} path {} Members units of {}", members(0), members(1), members(2), units.len());
        // the closed-form grid (the leaf's rect 1) and the zero-area
        // rect (rect 2) are never split; the array's child reaches its
        // Members units through the member-wise expansion
        assert!(!units.iter().any(|u| u.ci == 1 && matches!(u.kind, UnitKind::Members { shape: 0, idx: 1 | 2, .. })));
        assert!(units.iter().any(|u| u.ci == 2 && matches!(u.kind, UnitKind::Members { .. })));
        // the heavy child's Pts (3 members) and 2 x 2 Grid placements
        // are expanded member by member, each member's charge on its
        // first unit: 7 charges, as the walk charges them
        assert_eq!(units.iter().map(|u| u.extra).sum::<u64>(), 7);
        let mk = |jobs, balanced| {
            build(&d, 0, 0, &Opts { base_um: 0.01, jobs, balanced_units: balanced, ..Opts::default() }).unwrap()
        };
        let (a, b, cnt) = (mk(4, true), mk(1, true), mk(4, false));
        assert_eq!(a.layers[0].status, STATUS_OK);
        assert!(a.layers[0].work > 1_000_000, "{}", a.layers[0].work);
        assert_eq!(write_ovo(&a), write_ovo(&b));
        assert_eq!(write_ovo(&a), write_ovo(&cnt));
        assert_eq!(a.layers[0].work, cnt.layers[0].work);
        // every shape sits at placement depth 1: one plane
        assert_eq!(a.layers[0].planes.iter().map(|p| p.depth).collect::<Vec<_>>(), vec![1]);
        // over the work budget the verdict is the same on every cut
        // (the work value at the trip point is not pinned)
        let over = |jobs, balanced| {
            build(&d, 0, 0, &Opts { base_um: 0.01, jobs, balanced_units: balanced, max_work: 50_000, ..Opts::default() })
                .unwrap()
                .layers[0]
                .status
        };
        assert_eq!((over(4, true), over(1, true), over(4, false)), (STATUS_NONE_WORK, STATUS_NONE_WORK, STATUS_NONE_WORK));
    }

    #[test]
    fn the_prune_marks_a_dense_grid_of_a_small_cell_in_one_fill() {
        // 2026-09-18: the same 100k x 100k grid that the exact walk
        // stops on (none:work above) is, with the prune (the default),
        // one footprint fill - a 5 dbu child at pitch 1 in 10 dbu cells
        let mut child = cell("C");
        child.rects.push(rect(1, 0, 0, 5, 5, Rep::One));
        let mut top = cell("T");
        top.places.push(PlaceRec {
            cell: 1,
            x: 0,
            y: 0,
            rot: 0,
            flip: false,
            rep: Rep::Grid { na: 100_000, nb: 100_000, va: (1, 0), vb: (0, 0) },
        });
        let d = doc_with(vec![top, child], 0, vec![(1, 0)]);
        let occ = build(&d, 0, 0, &Opts { base_um: 0.01, max_work: 50_000, ..Opts::default() }).unwrap();
        assert_eq!(occ.layers[0].status, STATUS_OK);
        assert!(occ.layers[0].work < 50_000, "work {}", occ.layers[0].work);
        // the fill is the members' footprint: x 0..100_004, y 0..5 in 10 dbu cells
        let plane = &occ.layers[0].planes[0];
        assert_eq!(plane.depth, 1);
        let l0 = &plane.levels[0];
        assert_eq!(l0.count(), (occ.w as u64) * (occ.h as u64));
    }

    #[test]
    fn the_work_budget_is_shared_across_threads() {
        // the review's huge grid on four threads: every thread flushes
        // its charges into the layer's total, so the over-budget stop
        // lands within jobs x FLUSH_CHARGES of the budget
        let mut child = cell("C");
        child.rects.push(rect(1, 0, 0, 5, 5, Rep::One));
        let mut top = cell("T");
        top.places.push(PlaceRec {
            cell: 1,
            x: 0,
            y: 0,
            rot: 0,
            flip: false,
            rep: Rep::Grid { na: 100_000, nb: 100_000, va: (1, 0), vb: (0, 0) },
        });
        let d = doc_with(vec![top, child], 0, vec![(1, 0)]);
        let budget = 50_000u64;
        let occ = build(&d, 0, 0, &Opts { base_um: 0.01, max_work: budget, jobs: 4, prune: false, ..Opts::default() }).unwrap();
        assert_eq!(occ.layers[0].status, STATUS_NONE_WORK);
        assert!(occ.layers[0].work > budget);
        assert!(
            occ.layers[0].work <= budget + 4 * FLUSH_CHARGES + 4,
            "charged {} for a budget of {}",
            occ.layers[0].work,
            budget
        );
    }

    #[test]
    fn a_layer_without_positive_area_shapes_is_empty_without_bitmaps() {
        let mut top = cell("T");
        top.rects.push(rect(1, 0, 0, 1000, 1000, Rep::One));
        // zero-width rects only, repeated
        top.rects.push(RectRec { layer: 2, dt: 0, x: 0, y: 0, w: 0, h: 500, rep: Rep::Grid { na: 3, nb: 1, va: (10, 0), vb: (0, 0) } });
        // layer 3 is in the table without a record
        let d = doc_with(vec![top], 0, vec![(2, 0), (1, 0), (3, 0)]);
        let occ = build(&d, 0, 0, &opts(0.01)).unwrap();
        assert_eq!(occ.layers[0].status, STATUS_EMPTY);
        assert!(occ.layers[0].planes.is_empty());
        assert_eq!(occ.layers[1].status, STATUS_OK);
        assert_eq!((occ.layers[2].status, occ.layers[2].work), (STATUS_EMPTY, 0));
        // empty layers take no room: a byte limit of one pyramid still
        // fits the ok layer that follows an empty one
        let tight = build(&d, 0, 0, &Opts { base_um: 0.01, max_bytes: layer_bytes(occ.w, occ.h), ..Opts::default() }).unwrap();
        assert_eq!(tight.layers[1].status, STATUS_OK);
        // the file holds only the ok layer's bitmaps; empty reads as nothing
        let bytes = write_ovo(&occ);
        let f = OvoFile::from_bytes(bytes.clone()).unwrap();
        assert_eq!(f.layers[0].status, STATUS_EMPTY);
        assert!(f.level(0, 0).is_none() && !f.get(0, 0, 0, 0) && f.count(0, 0) == 0);
        assert!(f.count(1, 0) > 0);
        // three layer entries, one plane (the ok layer's depth 0)
        let table = 3 * LAYER_FIXED + PLANE_FIXED + occ.n_levels as usize * LEVEL_ENTRY;
        assert_eq!(bytes.len(), HEADER_FIXED + 1 + table + layer_bytes(occ.w, occ.h) as usize);
        assert_eq!(status_text(STATUS_EMPTY), "empty");
    }

    #[test]
    fn a_refused_path_leaves_the_layer_without_a_summary() {
        // review 2026-09-11 (2nd) P1-2: a U-turn path was counted and
        // dropped, and the layer published as ok - a summary with a
        // shape missing. The layer is none:unsupported instead.
        let mut top = cell("T");
        top.paths.push(PathRec { layer: 1, dt: 0, pts: vec![(0, 0), (100, 0), (0, 0)], hw: 5, es: 0, ee: 0, rep: Rep::One });
        top.rects.push(rect(1, 0, 50, 40, 40, Rep::One));
        top.rects.push(RectRec { layer: 2, dt: 0, x: 0, y: 0, w: 40, h: 40, rep: Rep::One });
        let d = doc_with(vec![top], 0, vec![(1, 0), (2, 0)]);
        let occ = build(&d, 0, 0, &opts(0.01)).unwrap();
        assert_eq!(occ.layers[0].status, STATUS_NONE_UNSUPPORTED);
        assert!(occ.layers[0].planes.is_empty());
        assert_eq!(occ.layers[1].status, STATUS_OK);
        assert_eq!(occ.paths_skipped, 1);
        let f = OvoFile::from_bytes(write_ovo(&occ)).unwrap();
        assert_eq!(f.layers[0].status, STATUS_NONE_UNSUPPORTED);
        assert!(f.level(0, 0).is_none() && f.level(1, 0).is_some());
        assert_eq!(status_text(STATUS_NONE_UNSUPPORTED), "none:unsupported");
    }

    #[test]
    fn bitmap_offsets_inside_the_table_or_overlapping_are_refused() {
        // review 2026-09-11 (2nd) P2-4: an offset of 0 pointed a level
        // at the header and read as a valid bitmap
        let mut top = cell("T");
        top.rects.push(rect(1, 0, 0, 2000, 2000, Rep::One));
        let d = doc_with(vec![top], 0, vec![(1, 0)]);
        let occ = build(&d, 0, 0, &opts(0.01)).unwrap();
        assert!(occ.n_levels >= 2);
        let bytes = write_ovo(&occ);
        let table = HEADER_FIXED + 1 + LAYER_FIXED + PLANE_FIXED;
        let off0 = table + 8;
        let off1 = table + LEVEL_ENTRY + 8;
        let level0_off = u64::from_le_bytes(bytes[off0..off0 + 8].try_into().unwrap());
        let mut header = bytes.clone();
        header[off0..off0 + 8].copy_from_slice(&0u64.to_le_bytes());
        assert!(OvoFile::from_bytes(header).unwrap_err().contains("inside the header"));
        let mut overlap = bytes.clone();
        overlap[off1..off1 + 8].copy_from_slice(&level0_off.to_le_bytes());
        assert!(OvoFile::from_bytes(overlap).unwrap_err().contains("overlaps"));
        assert!(OvoFile::from_bytes(bytes).is_ok());
    }

    #[test]
    fn limits_record_none_statuses_instead_of_approximations() {
        let mut top = cell("T");
        top.rects.push(rect(1, 0, 0, 1000, 1000, Rep::One));
        top.rects.push(RectRec { layer: 2, dt: 0, x: 0, y: 0, w: 1000, h: 1000, rep: Rep::One });
        let d = doc_with(vec![top], 0, vec![(1, 0), (2, 0)]);
        let cells = build(&d, 0, 0, &Opts { base_um: 0.01, max_cells: 100, ..Opts::default() }).unwrap();
        assert!(cells.layers.iter().all(|l| l.status == STATUS_NONE_CELLS && l.planes.is_empty()));
        let work = build(&d, 0, 0, &Opts { base_um: 0.01, max_work: 100, ..Opts::default() }).unwrap();
        assert!(work.layers.iter().all(|l| l.status == STATUS_NONE_WORK));
        let size = build(&d, 0, 0, &Opts { base_um: 0.01, max_bytes: layer_bytes(100, 100), ..Opts::default() }).unwrap();
        assert_eq!(size.layers[0].status, STATUS_OK);
        assert_eq!(size.layers[1].status, STATUS_NONE_SIZE);
        // every variant still serializes and reads back
        for occ in [&cells, &work, &size] {
            let f = OvoFile::from_bytes(write_ovo(occ)).unwrap();
            assert_eq!(f.layers.len(), 2);
            assert_eq!(f.layers[0].status, occ.layers[0].status);
        }
    }

    #[test]
    fn a_heavy_block_among_light_placements_is_split_by_work() {
        // field 2026-09-16 (150 MB chip): the top's many placements
        // satisfied the count target, so the one block holding most
        // of the layer's records stayed a single unit and one thread
        // marked it for minutes. The balanced split descends into it
        // and slices its records; the count-based split does not.
        let mut light = cell("L");
        light.rects.push(rect(1, 0, 0, 3, 3, Rep::One));
        let mut heavy = cell("H");
        heavy.rects.push(rect(1, 0, 0, 1, 1, Rep::Grid { na: 300, nb: 300, va: (4, 0), vb: (0, 4) }));
        for k in 0..64 {
            heavy.rects.push(rect(1, 2000 + k * 5, 0, 2, 2, Rep::One));
        }
        let mut top = cell("T");
        for k in 0..60 {
            top.places.push(PlaceRec { cell: 1, x: k * 10, y: 5000, rot: 0, flip: false, rep: Rep::One });
        }
        top.places.push(PlaceRec { cell: 2, x: 0, y: 0, rot: 0, flip: false, rep: Rep::One });
        let d = doc_with(vec![top, light, heavy], 0, vec![(1, 0)]);
        let shapes = take_layer_shapes(&mut index_shapes(&d), (1, 0), d.cells.len());
        let has = layer_presence(&d, &shapes);
        let weights = layer_weights(&d, &shapes, &has, None, 10);
        assert_eq!((weights[1], weights[2]), (1, 90_000 + 64));
        assert_eq!(weights[0], 60 + 90_064);
        let by_count = units_for(&d, &has, &shapes, 4, false, 10, None);
        assert_eq!(by_count.iter().filter(|u| u.ci == 2).count(), 0, "count split leaves the block one unit");
        let balanced = units_for(&d, &has, &shapes, 4, true, 10, None);
        let in_block = balanced.iter().filter(|u| u.ci == 2).count();
        assert!(in_block >= 8, "{} units in the block of {}", in_block, balanced.len());
        // the split changes nothing in the file
        let a = build(&d, 0, 0, &Opts { base_um: 0.01, jobs: 4, balanced_units: true, ..Opts::default() }).unwrap();
        let b = build(&d, 0, 0, &Opts { base_um: 0.01, jobs: 4, balanced_units: false, ..Opts::default() }).unwrap();
        let c = build(&d, 0, 0, &Opts { base_um: 0.01, jobs: 1, balanced_units: true, ..Opts::default() }).unwrap();
        assert_eq!(write_ovo(&a), write_ovo(&b));
        assert_eq!(write_ovo(&a), write_ovo(&c));
        assert_eq!(a.layers[0].work, b.layers[0].work);
    }

    #[test]
    fn the_automatic_base_cell_follows_the_chip_size() {
        // the rule: coarsest of 4/2/1/0.5/0.25 um with >= 2048 cells
        // on the longer side; the field chips: 35.8 mm and 26 x 33 mm
        // keep 4 um, the 1.55 x 2.25 mm chip gets 1 um
        assert_eq!(auto_base_um_for_span(35_838.4), 4.0);
        assert_eq!(auto_base_um_for_span(32_969.0), 4.0);
        assert_eq!(auto_base_um_for_span(8_192.0), 4.0);
        assert_eq!(auto_base_um_for_span(8_191.0), 2.0);
        assert_eq!(auto_base_um_for_span(5_000.0), 2.0);
        assert_eq!(auto_base_um_for_span(2_252.0), 1.0);
        assert_eq!(auto_base_um_for_span(1_000.0), 0.25);
        assert_eq!(auto_base_um_for_span(0.0), 0.25);
        // through build: BASE_AUTO picks by the top's longer side, an
        // explicit cell is taken as given
        let mut top = cell("T");
        top.rects.push(rect(1, 0, 0, 10_000_000, 3_000_000, Rep::One)); // 10 x 3 mm at unit 1000
        let d = doc_with(vec![top], 0, vec![(1, 0)]);
        let auto = build(&d, 0, 0, &Opts { base_um: BASE_AUTO, jobs: 2, ..Opts::default() }).unwrap();
        assert_eq!(auto.cell_dbu, 4000);
        let given = build(&d, 0, 0, &Opts { base_um: 2.0, jobs: 2, ..Opts::default() }).unwrap();
        assert_eq!(given.cell_dbu, 2000);
        let mut small = cell("S");
        small.rects.push(rect(1, 0, 0, 3_000, 2_000, Rep::One)); // 3 x 2 um
        let d = doc_with(vec![small], 0, vec![(1, 0)]);
        let auto = build(&d, 0, 0, &Opts { base_um: BASE_AUTO, jobs: 1, ..Opts::default() }).unwrap();
        assert_eq!((auto.cell_dbu, auto.w, auto.h), (250, 12, 8));
        assert!(build(&d, 0, 0, &Opts { base_um: -1.0, ..Opts::default() }).is_err());
    }

    #[test]
    fn progress_lines_report_the_layers_without_a_summary() {
        static LINES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        fn collect(m: &str) {
            LINES.lock().unwrap().push(m.to_string());
        }
        let mut top = cell("T");
        top.rects.push(rect(1, 0, 0, 1000, 1000, Rep::One));
        top.rects.push(RectRec { layer: 2, dt: 0, x: 0, y: 0, w: 50, h: 50, rep: Rep::One });
        let d = doc_with(vec![top], 0, vec![(1, 0), (2, 0)]);
        let occ = build(&d, 0, 0, &Opts { base_um: 0.01, max_work: 100, progress: Some(collect), ..Opts::default() }).unwrap();
        assert_eq!(occ.layers[0].status, STATUS_NONE_WORK);
        let lines = LINES.lock().unwrap().clone();
        assert!(lines.iter().any(|l| l.starts_with("1/0 none:work work=")), "{:?}", lines);
        // a quick ok layer is not worth a line
        assert!(!lines.iter().any(|l| l.starts_with("2/0")), "{:?}", lines);
    }

    #[test]
    fn the_file_round_trips_and_a_truncated_or_edited_copy_is_refused() {
        let mut top = cell("TOP");
        top.rects.push(rect(1, 3, 4, 500, 700, Rep::One));
        let d = doc_with(vec![top], 0, vec![(1, 0), (5, 2)]);
        let occ = build(&d, 123, 456, &opts(0.01)).unwrap();
        let bytes = write_ovo(&occ);
        let f = OvoFile::from_bytes(bytes.clone()).unwrap();
        assert_eq!((f.src_size, f.src_mtime, f.top.as_str(), f.cell_dbu), (123, 456, "TOP", 10));
        assert_eq!(f.n_levels, occ.n_levels);
        assert_eq!((f.w, f.h), (occ.w, occ.h));
        for (k, layer) in occ.layers.iter().enumerate() {
            for (p, plane) in layer.planes.iter().enumerate() {
                assert_eq!(f.layers[k].planes[p].depth, plane.depth);
                for (lv, level) in plane.levels.iter().enumerate() {
                    let (w, h, bits) = f.plane_level(k, p, lv).unwrap();
                    assert_eq!((w, h), (level.w, level.h));
                    assert_eq!(bits, &level.bits[..]);
                    let (fw, fh, flat) = f.level(k, lv).unwrap();
                    assert_eq!((fw, fh), (w, h));
                    assert_eq!(&flat[..], &layer.level(lv).unwrap().bits[..]);
                }
            }
        }
        assert!(f.depth_aware());
        assert!(OvoFile::from_bytes(bytes[..bytes.len() - 1].to_vec()).unwrap_err().contains("truncated"));
        assert!(OvoFile::from_bytes(bytes[..40].to_vec()).unwrap_err().contains("truncated"));
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(OvoFile::from_bytes(bad).unwrap_err().contains("magic"));
        let mut wrong_len = bytes.clone();
        // first level entry's len sits after the layer and plane fixed parts
        let o = HEADER_FIXED + 3 + LAYER_FIXED + PLANE_FIXED + 16;
        wrong_len[o..o + 8].copy_from_slice(&1u64.to_le_bytes());
        assert!(OvoFile::from_bytes(wrong_len).unwrap_err().contains("expected"));
    }

    #[test]
    fn planes_follow_the_placement_depth_and_flatten_to_the_full_view() {
        // top: 1/0 at depth 0; A (placed twice) holds 1/0 and 2/0 at
        // depth 1 and B (inside A) holds 1/0 and 3/0 at depth 2
        let mut b_cell = cell("B");
        b_cell.rects.push(rect(1, 0, 0, 20, 20, Rep::One));
        b_cell.rects.push(RectRec { layer: 3, dt: 0, x: 30, y: 0, w: 20, h: 20, rep: Rep::One });
        let mut a_cell = cell("A");
        a_cell.rects.push(rect(1, 0, 0, 20, 20, Rep::One));
        a_cell.rects.push(RectRec { layer: 2, dt: 0, x: 30, y: 0, w: 20, h: 20, rep: Rep::One });
        a_cell.places.push(PlaceRec { cell: 2, x: 0, y: 100, rot: 0, flip: false, rep: Rep::One });
        let mut top = cell("T");
        top.rects.push(rect(1, 0, 0, 20, 20, Rep::One));
        top.places.push(PlaceRec { cell: 1, x: 200, y: 0, rot: 0, flip: false, rep: Rep::One });
        top.places.push(PlaceRec { cell: 1, x: 400, y: 0, rot: 0, flip: false, rep: Rep::One });
        let d = doc_with(vec![top, a_cell, b_cell], 0, vec![(1, 0), (2, 0), (3, 0)]);
        let shapes = take_layer_shapes(&mut index_shapes(&d), (1, 0), d.cells.len());
        let has = layer_presence(&d, &shapes);
        assert_eq!(layer_max_depth(&d, &shapes, &has), 2);
        let second = take_layer_shapes(&mut index_shapes(&d), (2, 0), d.cells.len());
        assert_eq!(layer_max_depth(&d, &second, &layer_presence(&d, &second)), 1);
        let occ = build(&d, 0, 0, &opts(0.01)).unwrap();
        let depths = |k: usize| occ.layers[k].planes.iter().map(|p| p.depth).collect::<Vec<_>>();
        assert_eq!((depths(0), depths(1), depths(2)), (vec![0, 1, 2], vec![1], vec![2]));
        let (ox, oy) = (occ.bbox.0, occ.bbox.1);
        let at = |k: usize, depth: Option<u32>, x: i64, y: i64| {
            occ.layers[k]
                .level_at_depth(0, depth)
                .map(|l| l.get(((x - ox) / 10) as u32, ((y - oy) / 10) as u32))
                .unwrap_or(false)
        };
        // depth 0: the top's own rect only; depth 1 adds A's; depth 2 B's
        assert!(at(0, Some(0), 5, 5) && !at(0, Some(0), 205, 5) && !at(0, Some(0), 205, 105));
        assert!(at(0, Some(1), 5, 5) && at(0, Some(1), 205, 5) && at(0, Some(1), 405, 5) && !at(0, Some(1), 205, 105));
        assert!(at(0, Some(2), 205, 105) && at(0, None, 205, 105) && at(0, Some(9), 405, 105));
        // 2/0 has nothing at depth 0: a plane without cells, not None
        assert_eq!(occ.layers[1].level_at_depth(0, Some(0)), None);
        assert!(!at(1, Some(0), 235, 5) && at(1, Some(1), 235, 5));
        // the file round-trips the planes and answers the same depths
        let f = OvoFile::from_bytes(write_ovo(&occ)).unwrap();
        assert!(f.depth_aware());
        assert_eq!(f.layers[0].planes.iter().map(|p| p.depth).collect::<Vec<_>>(), vec![0, 1, 2]);
        let cell_of = |x: i64, y: i64| (((x - ox) / 10) as u32, ((y - oy) / 10) as u32);
        let fat = |k: usize, depth: Option<u32>, x: i64, y: i64| {
            let (w, h, bits) = f.level_at_depth(k, 0, depth).unwrap();
            let (i, j) = cell_of(x, y);
            i < w && j < h && (bits[j as usize * Level::row_bytes(w) + (i / 8) as usize] >> (i % 8)) & 1 == 1
        };
        assert!(fat(0, Some(0), 5, 5) && !fat(0, Some(0), 205, 5));
        assert!(fat(0, Some(1), 205, 5) && !fat(0, Some(1), 205, 105));
        assert!(fat(0, None, 205, 105));
        // a depth drawing none of a layer's planes: (0, 0, empty)
        let (w, h, bits) = f.level_at_depth(2, 0, Some(1)).unwrap();
        assert_eq!((w, h, bits.len()), (0, 0, 0));
        // a version-1 file reads as one flattened plane, unlimited only
        let v1 = OvoFile::from_bytes(write_ovo_v1(&occ)).unwrap();
        assert!(!v1.depth_aware());
        assert_eq!(v1.layers[0].planes.len(), 1);
        assert_eq!(v1.layers[0].planes[0].depth, DEPTH_ALL);
        assert_eq!(v1.level(0, 0).unwrap().2, f.level(0, 0).unwrap().2);
        assert_eq!(v1.level_at_depth(0, 0, Some(0)).unwrap().0, 0);
        assert_eq!(v1.count(0, 0), f.count(0, 0));
    }

    #[test]
    fn depths_at_or_beyond_the_cap_share_the_last_plane() {
        // a chain of 17 placements: the leaf's rect sits at depth 17
        let mut cells = vec![cell("T")];
        for k in 1..=17 {
            cells[k - 1].places.push(PlaceRec { cell: k, x: 10, y: 0, rot: 0, flip: false, rep: Rep::One });
            cells.push(cell(&format!("C{}", k)));
        }
        cells[17].rects.push(rect(1, 0, 0, 5, 5, Rep::One));
        // and one at depth 15 exactly
        cells[15].rects.push(rect(1, 0, 50, 5, 5, Rep::One));
        let d = doc_with(cells, 0, vec![(1, 0)]);
        let occ = build(&d, 0, 0, &opts(0.01)).unwrap();
        assert_eq!(occ.layers[0].status, STATUS_OK);
        assert_eq!(occ.layers[0].planes.iter().map(|p| p.depth).collect::<Vec<_>>(), vec![DEPTH_CAP]);
        assert_eq!(occ.layers[0].level_at_depth(0, Some(14)), None);
        assert!(occ.layers[0].level_at_depth(0, Some(15)).unwrap().count() == 2);
        assert!(occ.layers[0].level_at_depth(0, Some(40)).unwrap().count() == 2);
        let f = OvoFile::from_bytes(write_ovo(&occ)).unwrap();
        assert_eq!(f.layers[0].planes[0].depth, DEPTH_CAP);
    }

    #[test]
    fn a_plane_table_out_of_order_or_beyond_the_cap_is_refused() {
        let mut child = cell("C");
        child.rects.push(rect(1, 0, 0, 20, 20, Rep::One));
        let mut top = cell("T");
        top.rects.push(rect(1, 100, 0, 20, 20, Rep::One));
        top.places.push(PlaceRec { cell: 1, x: 0, y: 0, rot: 0, flip: false, rep: Rep::One });
        let d = doc_with(vec![top, child], 0, vec![(1, 0)]);
        let occ = build(&d, 0, 0, &opts(0.01)).unwrap();
        assert_eq!(occ.layers[0].planes.len(), 2);
        let bytes = write_ovo(&occ);
        // the first plane's depth byte follows the layer fixed part
        let o = HEADER_FIXED + 1 + LAYER_FIXED;
        let mut swapped = bytes.clone();
        swapped[o] = 1; // plane 0 at depth 1, plane 1 at depth 1: not ascending
        assert!(OvoFile::from_bytes(swapped).unwrap_err().contains("depth"));
        let mut deep = bytes.clone();
        deep[o + PLANE_FIXED + occ.n_levels as usize * LEVEL_ENTRY] = DEPTH_CAP + 1;
        assert!(OvoFile::from_bytes(deep).unwrap_err().contains("depth"));
        let mut count = bytes.clone();
        count[o - 1] = 0; // status ok with no plane
        assert!(OvoFile::from_bytes(count).unwrap_err().contains("planes"));
        assert!(OvoFile::from_bytes(bytes).is_ok());
    }
}
