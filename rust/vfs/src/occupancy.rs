//! Occupancy pyramid (design.ovo) - docs/OCCUPANCY_PLAN.ko.md M1.
//!
//! Per source layer, one bit per grid cell of the flattened top cell:
//! 1 when the cell's OPEN box meets a shape's interior (a positive-area
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
//! File (little-endian):
//!   magic "FLOEOVO1", version u32, unit f64, src_size u64,
//!   src_mtime u64, cell_dbu i64, bbox x0 y0 x1 y1 i64, n_levels u32,
//!   n_layers u32, top_len u16, top utf8, then per layer: layer u32,
//!   dt u32, status u8, work u64, per level: w u32, h u32, off u64,
//!   len u64 (absolute offsets; rows padded to whole bytes, bit i of
//!   a row at byte i/8 bit i%8). The identity (src_size, src_mtime,
//!   top, layer table) must match the cache's design.ovm; a file that
//!   fails any check reads as "no summary".

use floe_oasis::doc::{Doc, PathRec, PolyRec, RectRec, Rep};
use floe_ovm::Ovm;
use floe_tiler::hier::cell_bboxes;
use floe_tiler::{is_axis, path_outline_any, Xf};

pub const MAGIC: &[u8; 8] = b"FLOEOVO1";
pub const VERSION: u32 = 1;
/// base cell in microns unless --occupancy-um says otherwise
pub const DEFAULT_BASE_UM: f64 = 4.0;
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

pub fn status_text(status: u8) -> &'static str {
    match status {
        STATUS_OK => "ok",
        STATUS_NONE_CELLS => "none:cells",
        STATUS_NONE_WORK => "none:work",
        STATUS_NONE_SIZE => "none:size",
        _ => "none:unknown",
    }
}

#[derive(Clone, Debug)]
pub struct Opts {
    pub base_um: f64,
    pub jobs: usize,
    pub max_cells: u64,
    pub max_work: u64,
    pub max_bytes: u64,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts {
            base_um: DEFAULT_BASE_UM,
            jobs: 1,
            max_cells: DEFAULT_MAX_CELLS,
            max_work: DEFAULT_MAX_WORK,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

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

#[derive(Clone, Debug)]
pub struct Layer {
    pub layer: u32,
    pub dt: u32,
    pub status: u8,
    /// marks charged (cells set + members + edge rows)
    pub work: u64,
    /// empty unless status == STATUS_OK
    pub levels: Vec<Level>,
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

struct Bits {
    w: u32,
    h: u32,
    stride: usize,
    words: Vec<u64>,
}

impl Bits {
    fn new(w: u32, h: u32) -> Bits {
        let stride = (w as usize + 63) / 64;
        Bits {
            w,
            h,
            stride,
            words: vec![0u64; stride * h as usize],
        }
    }

    /// set cells i0..=i1 of row j (callers clamp to the grid)
    fn set_span(&mut self, j: u32, i0: u32, i1: u32) {
        debug_assert!(i1 < self.w && j < self.h && i0 <= i1);
        let base = j as usize * self.stride;
        let (w0, w1) = ((i0 / 64) as usize, (i1 / 64) as usize);
        let lo_mask = u64::MAX << (i0 % 64);
        let hi_mask = u64::MAX >> (63 - i1 % 64);
        if w0 == w1 {
            self.words[base + w0] |= lo_mask & hi_mask;
        } else {
            self.words[base + w0] |= lo_mask;
            for k in w0 + 1..w1 {
                self.words[base + k] = u64::MAX;
            }
            self.words[base + w1] |= hi_mask;
        }
    }

    fn to_level(&self) -> Level {
        let rb = Level::row_bytes(self.w);
        let mut bits = vec![0u8; rb * self.h as usize];
        for j in 0..self.h as usize {
            let row = &self.words[j * self.stride..(j + 1) * self.stride];
            let out = &mut bits[j * rb..(j + 1) * rb];
            for (k, word) in row.iter().enumerate() {
                let le = word.to_le_bytes();
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

/// per cell: does the layer occur in the cell or its descendants
fn layer_presence(doc: &Doc, key: (u32, u32)) -> Vec<bool> {
    fn walk(doc: &Doc, ci: usize, key: (u32, u32), state: &mut [u8]) -> bool {
        match state[ci] {
            2 => return true,
            3 => return false,
            1 => return false, // cycle guard: an ancestor decides
            _ => {}
        }
        state[ci] = 1;
        let cell = &doc.cells[ci];
        let direct = cell.rects.iter().any(|r| (r.layer, r.dt) == key)
            || cell.polys.iter().any(|p| (p.layer, p.dt) == key)
            || cell.paths.iter().any(|p| (p.layer, p.dt) == key);
        let mut has = direct;
        if !has {
            for pl in &cell.places {
                if walk(doc, pl.cell, key, state) {
                    has = true;
                    break;
                }
            }
        }
        state[ci] = if has { 2 } else { 3 };
        has
    }
    let mut state = vec![0u8; doc.cells.len()];
    let _ = walk(doc, doc.top, key, &mut state);
    // cells only reachable through a cycle guard stay "unknown": walk
    // them on their own so every reachable cell has a verdict
    for ci in 0..doc.cells.len() {
        if state[ci] == 0 || state[ci] == 1 {
            state[ci] = 0;
            let _ = walk(doc, ci, key, &mut state);
        }
    }
    state.iter().map(|&s| s == 2).collect()
}

struct Marker<'a> {
    doc: &'a Doc,
    key: (u32, u32),
    has: Vec<bool>,
    ox: i64,
    oy: i64,
    c: i64,
    bits: Bits,
    work: u64,
    max_work: u64,
    over: bool,
    paths_skipped: u64,
}

impl<'a> Marker<'a> {
    fn charge(&mut self, n: u64) -> bool {
        self.work = self.work.saturating_add(n);
        if self.work > self.max_work {
            self.over = true;
        }
        !self.over
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
        self.bits.set_span(j, i0, i1);
        true
    }

    fn mark_world_rect(&mut self, x0: i128, y0: i128, x1: i128, y1: i128) -> bool {
        let (x0, x1) = (x0.min(x1), x0.max(x1));
        let (y0, y1) = (y0.min(y1), y0.max(y1));
        let Some((i0, i1)) = self.range(x0, x1, self.ox, self.bits.w) else {
            return true;
        };
        let Some((j0, j1)) = self.range(y0, y1, self.oy, self.bits.h) else {
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
        mut f: F,
    ) -> bool {
        match rep {
            Rep::One => f(self, 0, 0),
            Rep::Grid { na, nb, va, vb } => {
                if !self.charge(na.saturating_mul(*nb)) {
                    return false;
                }
                let wa = xf.apply_vec(va.0, va.1);
                let wb = xf.apply_vec(vb.0, vb.1);
                for j in 0..*nb as i128 {
                    for i in 0..*na as i128 {
                        let dx = i * wa.0 as i128 + j * wb.0 as i128;
                        let dy = i * wa.1 as i128 + j * wb.1 as i128;
                        if !f(self, dx, dy) {
                            return false;
                        }
                    }
                }
                true
            }
            Rep::Pts(p) => {
                if !self.charge(p.len() as u64) {
                    return false;
                }
                for &(dx, dy) in p.iter() {
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
        if r.w <= 0 || r.h <= 0 {
            return true; // zero area: KLayout's region is empty
        }
        let a = xf.apply(r.x, r.y);
        let b = xf.apply(r.x + r.w, r.y + r.h);
        let (mx0, mx1) = (a.0.min(b.0) as i128, a.0.max(b.0) as i128);
        let (my0, my1) = (a.1.min(b.1) as i128, a.1.max(b.1) as i128);
        if let Rep::Grid { na, nb, va, vb } = &r.rep {
            let wa = xf.apply_vec(va.0, va.1);
            let wb = xf.apply_vec(vb.0, vb.1);
            if is_axis(&wa, &wb) {
                // closed form: along each axis the members repeat at
                // a pitch whose gap (pitch - member extent) is narrower
                // than a cell, so no cell can sit in a gap - the
                // footprint marks exactly the per-member cells
                let (px, nx, py, ny) = if wa.1 == 0 && wb.0 == 0 {
                    (wa.0.abs(), *na, wb.1.abs(), *nb)
                } else {
                    (wb.0.abs(), *nb, wa.1.abs(), *na)
                };
                let c = self.c as i128;
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
                    return self.mark_world_rect(mx0 + ex0, my0 + ey0, mx1 + ex1, my1 + ey1);
                }
            }
        }
        self.for_each_member(&r.rep, xf, |m, dx, dy| {
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
        let (w, h) = (self.bits.w as i128, self.bits.h as i128);
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
                if let Some((i0, i1)) = self.range(ax.min(bx), ax.max(bx), self.ox, self.bits.w) {
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
                if let Some((j0, j1)) = self.range(ay.min(by), ay.max(by), self.oy, self.bits.h) {
                    if !self.charge((j1 - j0) as u64 + 1) {
                        return false;
                    }
                    for j in j0..=j1 {
                        self.bits.set_span(j, i as u32, i as u32);
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
        let world: Vec<(i128, i128)> = local
            .iter()
            .map(|&(x, y)| {
                let (wx, wy) = xf.apply(x, y);
                (wx as i128, wy as i128)
            })
            .collect();
        let mut shifted = world.clone();
        self.for_each_member(rep, xf, |m, dx, dy| {
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

    fn mark_path_rec(&mut self, p: &PathRec, xf: &Xf) -> bool {
        if p.hw <= 0 {
            return true; // zero width: KLayout's region is empty
        }
        match path_outline_any(&p.pts, p.hw, p.es, p.ee) {
            Ok(hull) => self.mark_poly_pts(&hull, &p.rep, xf),
            Err(_) => {
                self.paths_skipped += 1;
                true
            }
        }
    }

    fn walk(&mut self, ci: usize, xf: &Xf) -> bool {
        if self.over || !self.has[ci] {
            return !self.over;
        }
        let cell = &self.doc.cells[ci];
        for r in &cell.rects {
            if (r.layer, r.dt) == self.key && !self.mark_rect_rec(r, xf) {
                return false;
            }
        }
        for p in &cell.polys {
            if (p.layer, p.dt) == self.key && !self.mark_poly_rec(p, xf) {
                return false;
            }
        }
        for p in &cell.paths {
            if (p.layer, p.dt) == self.key && !self.mark_path_rec(p, xf) {
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
                    if !self.walk(pl.cell, &base) {
                        return false;
                    }
                }
                rep => {
                    // member offsets live in the parent frame: place
                    // the child at (x + dx, y + dy) under the parent's xf
                    let offsets: Vec<(i64, i64)> = match rep {
                        Rep::Grid { na, nb, va, vb } => {
                            if !self.charge(na.saturating_mul(*nb)) {
                                return false;
                            }
                            let mut v = Vec::with_capacity((na * nb) as usize);
                            for j in 0..*nb as i64 {
                                for i in 0..*na as i64 {
                                    v.push((i * va.0 + j * vb.0, i * va.1 + j * vb.1));
                                }
                            }
                            v
                        }
                        Rep::Pts(p) => {
                            if !self.charge(p.len() as u64) {
                                return false;
                            }
                            p.to_vec()
                        }
                        Rep::One => unreachable!(),
                    };
                    for (dx, dy) in offsets {
                        let base = xf.compose(&Xf::place(pl.x + dx, pl.y + dy, pl.rot, pl.flip));
                        if !self.walk(pl.cell, &base) {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }
}

/// build the pyramid of one layer (status + levels)
fn build_layer(
    doc: &Doc,
    key: (u32, u32),
    origin: (i64, i64),
    cell_dbu: i64,
    w: u32,
    h: u32,
    max_work: u64,
) -> (Layer, u64) {
    let mut m = Marker {
        doc,
        key,
        has: layer_presence(doc, key),
        ox: origin.0,
        oy: origin.1,
        c: cell_dbu,
        bits: Bits::new(w, h),
        work: 0,
        max_work,
        over: false,
        paths_skipped: 0,
    };
    let ok = m.walk(doc.top, &Xf::identity());
    if !ok || m.over {
        return (
            Layer { layer: key.0, dt: key.1, status: STATUS_NONE_WORK, work: m.work, levels: Vec::new() },
            m.paths_skipped,
        );
    }
    let mut levels = vec![m.bits.to_level()];
    while levels.last().unwrap().w > TOP_GRID || levels.last().unwrap().h > TOP_GRID {
        let next = levels.last().unwrap().pool();
        levels.push(next);
    }
    (
        Layer { layer: key.0, dt: key.1, status: STATUS_OK, work: m.work, levels },
        m.paths_skipped,
    )
}

/// build every layer's pyramid; `jobs` layers at a time
pub fn build(doc: &Doc, src_size: u64, src_mtime: u64, opts: &Opts) -> Result<Occupancy, String> {
    if !(opts.base_um > 0.0) || !opts.base_um.is_finite() {
        return Err(format!("occupancy: base cell must be positive, got {}", opts.base_um));
    }
    let cell_dbu = ((opts.base_um * doc.unit).round() as i64).max(1);
    let bboxes = cell_bboxes(doc);
    let bbox = bboxes[doc.top].unwrap_or((0, 0, 0, 0));
    let span_x = (bbox.2 as i128 - bbox.0 as i128).max(0);
    let span_y = (bbox.3 as i128 - bbox.1 as i128).max(0);
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
            .map(|&(l, d)| Layer { layer: l, dt: d, status: STATUS_NONE_CELLS, work: 0, levels: Vec::new() })
            .collect();
        return Ok(occ);
    }
    let (w, h) = (gw as u32, gh as u32);
    occ.w = w;
    occ.h = h;
    occ.n_levels = level_count(w, h);
    let per_layer = layer_bytes(w, h);
    let fit = if per_layer == 0 { doc.layer_order.len() } else { (opts.max_bytes / per_layer) as usize };
    let keys: Vec<(u32, u32)> = doc.layer_order.clone();
    let n = keys.len();
    let mut out: Vec<Option<Layer>> = (0..n).map(|_| None).collect();
    let mut skipped = 0u64;
    let jobs = opts.jobs.max(1).min(n.max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<(usize, Layer, u64)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..jobs)
            .map(|_| {
                let next = &next;
                let keys = &keys;
                std::thread::Builder::new()
                    .stack_size(64 << 20)
                    .spawn_scoped(s, move || {
                        use std::sync::atomic::Ordering::Relaxed;
                        let mut mine = Vec::new();
                        loop {
                            let k = next.fetch_add(1, Relaxed);
                            if k >= n {
                                break;
                            }
                            if k >= fit {
                                mine.push((
                                    k,
                                    Layer {
                                        layer: keys[k].0,
                                        dt: keys[k].1,
                                        status: STATUS_NONE_SIZE,
                                        work: 0,
                                        levels: Vec::new(),
                                    },
                                    0,
                                ));
                                continue;
                            }
                            let (layer, sk) = build_layer(doc, keys[k], (bbox.0, bbox.1), cell_dbu, w, h, opts.max_work);
                            mine.push((k, layer, sk));
                        }
                        mine
                    })
                    .expect("occupancy worker")
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().expect("occupancy worker")).collect()
    });
    for (k, layer, sk) in results {
        skipped += sk;
        out[k] = Some(layer);
    }
    occ.layers = out.into_iter().map(|l| l.expect("every layer built")).collect();
    occ.paths_skipped = skipped;
    Ok(occ)
}

// ---------------------------------------------------------- serialize

const HEADER_FIXED: usize = 8 + 4 + 8 + 8 + 8 + 8 + 32 + 4 + 4 + 2;
const LAYER_FIXED: usize = 4 + 4 + 1 + 8;
const LEVEL_ENTRY: usize = 4 + 4 + 8 + 8;

fn put32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put64(o: &mut Vec<u8>, v: u64) {
    o.extend_from_slice(&v.to_le_bytes());
}

pub fn write_ovo(occ: &Occupancy) -> Vec<u8> {
    let top = occ.top.as_bytes();
    let top_len = top.len().min(u16::MAX as usize);
    let nl = occ.layers.len();
    let n_levels = occ.n_levels as usize;
    let table_len = nl * (LAYER_FIXED + n_levels * LEVEL_ENTRY);
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
        for lv in 0..n_levels {
            match layer.levels.get(lv) {
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
    debug_assert_eq!(out.len(), body_start);
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
pub struct LayerEntry {
    pub layer: u32,
    pub dt: u32,
    pub status: u8,
    pub work: u64,
    pub levels: Vec<LevelEntry>,
}

/// a validated design.ovo (structure only; `validate_against` checks
/// the identity against the cache's design.ovm)
pub struct OvoFile {
    data: floe_ovm::Backing,
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
            "OvoFile(top={:?} cell_dbu={} grid={}x{} levels={} layers={})",
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
        if &b[..8] != MAGIC {
            return Err("not an occupancy file (bad magic)".to_string());
        }
        let version = g32(b, 8);
        if version != VERSION {
            return Err(format!("occupancy version {} (this build reads {})", version, VERSION));
        }
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
        let entry = LAYER_FIXED + n_levels as usize * LEVEL_ENTRY;
        let table = (n_layers as usize)
            .checked_mul(entry)
            .ok_or_else(|| "corrupt occupancy header (layer table)".to_string())?;
        if len < o + table {
            return Err("truncated occupancy file (layer table)".to_string());
        }
        let mut layers = Vec::with_capacity(n_layers as usize);
        let (mut w, mut h) = (0u32, 0u32);
        for k in 0..n_layers as usize {
            let layer = g32(b, o);
            let dt = g32(b, o + 4);
            let status = b[o + 8];
            let work = g64(b, o + 9);
            o += LAYER_FIXED;
            let mut levels = Vec::with_capacity(n_levels as usize);
            let (mut ew, mut eh) = (gw, gh);
            for lv in 0..n_levels as usize {
                let e = LevelEntry { w: g32(b, o), h: g32(b, o + 4), off: g64(b, o + 8), len: g64(b, o + 16) };
                o += LEVEL_ENTRY;
                if status == STATUS_OK {
                    if e.w as i128 != ew || e.h as i128 != eh {
                        return Err(format!(
                            "corrupt occupancy layer {} level {}: grid {}x{}, expected {}x{}",
                            k, lv, e.w, e.h, ew, eh
                        ));
                    }
                    let need = (Level::row_bytes(e.w) as u64)
                        .checked_mul(e.h as u64)
                        .ok_or_else(|| "corrupt occupancy level (size overflow)".to_string())?;
                    if e.len != need {
                        return Err(format!(
                            "corrupt occupancy layer {} level {}: {} bytes, expected {}",
                            k, lv, e.len, need
                        ));
                    }
                    let end = e
                        .off
                        .checked_add(e.len)
                        .ok_or_else(|| "corrupt occupancy level (offset overflow)".to_string())?;
                    if end > len as u64 {
                        return Err(format!(
                            "truncated occupancy file: layer {} level {} ends at {} of {} bytes",
                            k, lv, end, len
                        ));
                    }
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
            layers.push(LayerEntry { layer, dt, status, work, levels });
        }
        if w == 0 && h == 0 && gw <= u32::MAX as i128 && gh <= u32::MAX as i128 {
            w = gw as u32;
            h = gh as u32;
        }
        Ok(OvoFile { data, unit, src_size, src_mtime, cell_dbu, bbox, w, h, n_levels, top, layers })
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

    /// (w, h, row-padded bits) of one level, None unless status ok
    pub fn level(&self, k: usize, lv: usize) -> Option<(u32, u32, &[u8])> {
        let layer = self.layers.get(k)?;
        if layer.status != STATUS_OK {
            return None;
        }
        let e = layer.levels.get(lv)?;
        let b: &[u8] = &self.data;
        Some((e.w, e.h, &b[e.off as usize..(e.off + e.len) as usize]))
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
        let mut m = Marker {
            doc: &doc_with(vec![cell("T")], 0, vec![(1, 0)]),
            key: (1, 0),
            has: vec![true],
            ox: 0,
            oy: 0,
            c,
            bits: Bits::new(w, h),
            work: 0,
            max_work: u64::MAX,
            over: false,
            paths_skipped: 0,
        };
        let world: Vec<(i128, i128)> = pts.iter().map(|&(x, y)| (x as i128, y as i128)).collect();
        assert!(m.mark_world_poly(&world));
        let level = m.bits.to_level();
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
        let l0 = &occ.layers[0].levels[0];
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
        assert_eq!(oe.layers[0].levels[0], *l0);
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
            assert_eq!(oa.layers[0].levels[0], ob.layers[0].levels[0]);
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
        let l0 = &occ.layers[0].levels[0];
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
        for (lv, pair) in occ.layers[0].levels.windows(2).enumerate() {
            let (a, b) = (&pair[0], &pair[1]);
            for j in 0..b.h {
                for i in 0..b.w {
                    let any = a.get(2 * i, 2 * j) || a.get(2 * i + 1, 2 * j) || a.get(2 * i, 2 * j + 1) || a.get(2 * i + 1, 2 * j + 1);
                    assert_eq!(b.get(i, j), any, "level {} cell {},{}", lv + 1, i, j);
                }
            }
        }
        assert_eq!(occ.n_levels as usize, occ.layers[0].levels.len());
        assert!(occ.layers[0].levels.last().unwrap().w <= TOP_GRID);
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
        let l0 = &occ.layers[0].levels[0];
        let (ox, oy) = (occ.bbox.0, occ.bbox.1);
        let cellf = |x: i64, y: i64| l0.get(((x - ox) / 10) as u32, ((y - oy) / 10) as u32);
        assert!(cellf(5, 5) && cellf(45, 5) && !cellf(5, 15));
        assert!(cellf(5, 55) && cellf(25, 75) && !cellf(5, 75));
        assert!(!cellf(85, 85) && !cellf(85, 5) && !cellf(65, 65));
        assert_eq!(occ.paths_skipped, 0);
    }

    #[test]
    fn limits_record_none_statuses_instead_of_approximations() {
        let mut top = cell("T");
        top.rects.push(rect(1, 0, 0, 1000, 1000, Rep::One));
        top.rects.push(RectRec { layer: 2, dt: 0, x: 0, y: 0, w: 1000, h: 1000, rep: Rep::One });
        let d = doc_with(vec![top], 0, vec![(1, 0), (2, 0)]);
        let cells = build(&d, 0, 0, &Opts { base_um: 0.01, max_cells: 100, ..Opts::default() }).unwrap();
        assert!(cells.layers.iter().all(|l| l.status == STATUS_NONE_CELLS && l.levels.is_empty()));
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
            for (lv, level) in layer.levels.iter().enumerate() {
                let (w, h, bits) = f.level(k, lv).unwrap();
                assert_eq!((w, h), (level.w, level.h));
                assert_eq!(bits, &level.bits[..]);
            }
        }
        assert!(OvoFile::from_bytes(bytes[..bytes.len() - 1].to_vec()).unwrap_err().contains("truncated"));
        assert!(OvoFile::from_bytes(bytes[..40].to_vec()).unwrap_err().contains("truncated"));
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(OvoFile::from_bytes(bad).unwrap_err().contains("magic"));
        let mut wrong_len = bytes.clone();
        // first level entry's len sits after the layer fixed part
        let o = HEADER_FIXED + 3 + LAYER_FIXED + 16;
        wrong_len[o..o + 8].copy_from_slice(&1u64.to_le_bytes());
        assert!(OvoFile::from_bytes(wrong_len).unwrap_err().contains("expected"));
    }
}
