//! Record-level band partitioner (spike S2).
//!
//! Resolves the cell hierarchy into global-frame shape records
//! (placement members expand - they are few; SHAPE repetitions stay
//! records), then routes every record into (tile, band) buckets:
//! - a record fully inside one tile keeps its repetition,
//! - a grid repetition straddling tiles splits ARITHMETICALLY into
//!   per-tile sub-grids plus individually clipped boundary members -
//!   the klayout path exploded such arrays and re-detected them,
//! - geometry crossing a tile border is clipped to the tile box
//!   (rect intersection / Sutherland-Hodgman), matching clip_into.
//! Texts are dropped (tiles are text-free by design). Band = size
//! class of max(bbox w, h), same edges as the Python indexer.

use floe_oasis::doc::{Doc, PolyRec, RectRec, Rep};
use std::collections::HashMap;

pub mod hier;

// ------------------------------------------------------------ transform

/// Orthogonal transform: p -> M*p + t, M entries in {-1,0,1}.
#[derive(Clone, Copy)]
pub struct Xf {
    m: [[i64; 2]; 2],
    t: (i64, i64),
}

impl Xf {
    pub fn identity() -> Self {
        Xf { m: [[1, 0], [0, 1]], t: (0, 0) }
    }

    pub fn place(x: i64, y: i64, rot: u8, flip: bool) -> Self {
        // flip = mirror about the x axis, applied before rotation
        let f = if flip { -1 } else { 1 };
        let (c, s) = match rot & 3 {
            0 => (1, 0),
            1 => (0, 1),
            2 => (-1, 0),
            _ => (0, -1),
        };
        // rot * flip
        Xf { m: [[c, -s * f], [s, c * f]], t: (x, y) }
    }

    pub fn compose(&self, inner: &Xf) -> Xf {
        let a = self.m;
        let b = inner.m;
        Xf {
            m: [
                [
                    a[0][0] * b[0][0] + a[0][1] * b[1][0],
                    a[0][0] * b[0][1] + a[0][1] * b[1][1],
                ],
                [
                    a[1][0] * b[0][0] + a[1][1] * b[1][0],
                    a[1][0] * b[0][1] + a[1][1] * b[1][1],
                ],
            ],
            t: self.apply(inner.t.0, inner.t.1),
        }
    }

    pub fn apply(&self, x: i64, y: i64) -> (i64, i64) {
        (
            self.m[0][0] * x + self.m[0][1] * y + self.t.0,
            self.m[1][0] * x + self.m[1][1] * y + self.t.1,
        )
    }

    pub fn apply_vec(&self, x: i64, y: i64) -> (i64, i64) {
        (
            self.m[0][0] * x + self.m[0][1] * y,
            self.m[1][0] * x + self.m[1][1] * y,
        )
    }

    /// orthonormal +-1 matrix: inverse = transpose, t' = -M^T t
    pub fn invert(&self) -> Xf {
        let mt = [[self.m[0][0], self.m[1][0]], [self.m[0][1], self.m[1][1]]];
        let t = (
            -(mt[0][0] * self.t.0 + mt[0][1] * self.t.1),
            -(mt[1][0] * self.t.0 + mt[1][1] * self.t.1),
        );
        Xf { m: mt, t }
    }
}

// ---------------------------------------------------------------- grid

#[derive(Clone, Copy)]
pub struct Grid {
    pub x0: i64,
    pub y0: i64,
    pub tw: i64,
    pub th: i64,
    pub nx: i64,
    pub ny: i64,
}

impl Grid {
    fn tile_box(&self, r: i64, c: i64) -> (i64, i64, i64, i64) {
        let tx0 = self.x0 + c * self.tw;
        let ty0 = self.y0 + r * self.th;
        (tx0, ty0, tx0 + self.tw, ty0 + self.th)
    }
}

// --------------------------------------------------------------- output

#[derive(Default)]
pub struct Bucket {
    pub rects: Vec<RectRec>,
    pub polys: Vec<PolyRec>,
    pub members: u64,
}

pub struct TileOut {
    /// (r, c, band) -> content in absolute coordinates
    pub buckets: HashMap<(i64, i64, usize), Bucket>,
    pub texts_dropped: u64,
    pub contexts: u64,
}

pub struct Tiler {
    grid: Grid,
    /// ascending band edges in dbu (len 3 for 4 bands)
    edges: Vec<i64>,
    nb: usize,
}

const CTX_CAP: u64 = 2_000_000;
const DENSE_ITER_CAP: u64 = 8_000_000;

pub enum Piece {
    Rect(i64, i64, i64, i64), // x, y, w, h
    Poly(Vec<(i64, i64)>),
}

impl Tiler {
    pub fn new(grid: Grid, edges_dbu: Vec<i64>) -> Self {
        let nb = edges_dbu.len() + 1;
        Tiler { grid, edges: edges_dbu, nb }
    }

    fn band_of(&self, w: i64, h: i64) -> usize {
        let s = w.max(h);
        // band 0 = largest (>= last edge), last band = smallest
        for k in (1..self.nb).rev() {
            if s < self.edges[self.nb - 1 - k] {
                return k;
            }
        }
        0
    }

    pub fn run(&self, doc: &Doc) -> Result<TileOut, String> {
        let mut out = TileOut {
            buckets: HashMap::new(),
            texts_dropped: 0,
            contexts: 0,
        };
        // DFS with explicit stack: (cell, transform, symbolic stack
        // of placement repetitions in the global frame - placement
        // arrays are NOT expanded here; the largest repetition layer
        // survives to the emitted record, the small ones expand at
        // emission)
        let mut stack: Vec<(usize, Xf, Vec<Rep>)> =
            vec![(doc.top, Xf::identity(), Vec::new())];
        while let Some((ci, xf, ctx)) = stack.pop() {
            out.contexts += 1;
            if out.contexts > CTX_CAP {
                return Err(format!(
                    "placement contexts exceed {} (spike cap)",
                    CTX_CAP
                ));
            }
            let cell = &doc.cells[ci];
            out.texts_dropped += cell
                .texts
                .iter()
                .map(|t| t.rep.members())
                .sum::<u64>();
            for r in &cell.rects {
                self.emit_rect(r, &xf, &ctx, &mut out)?;
            }
            for p in &cell.polys {
                self.emit_poly(p, &xf, &ctx, &mut out)?;
            }
            for pl in &cell.places {
                let base = Xf::place(pl.x, pl.y, pl.rot, pl.flip);
                let mut child_ctx = ctx.clone();
                let rep_g = xf_rep(&pl.rep, &xf);
                if !matches!(rep_g, Rep::One) {
                    child_ctx.push(rep_g);
                }
                stack.push((pl.cell, xf.compose(&base), child_ctx));
            }
        }
        Ok(out)
    }

    // ---- rectangles --------------------------------------------------

    fn emit_rect(
        &self,
        r: &RectRec,
        xf: &Xf,
        ctx: &[Rep],
        out: &mut TileOut,
    ) -> Result<(), String> {
        // transform the base box; orthogonal, so it stays a box
        let (x1, y1) = xf.apply(r.x, r.y);
        let (x2, y2) = xf.apply(r.x + r.w, r.y + r.h);
        let (bx0, bx1) = (x1.min(x2), x1.max(x2));
        let (by0, by1) = (y1.min(y2), y1.max(y2));
        let (rep, outer) = fold_reps(xf_rep(&r.rep, xf), ctx)?;
        for (ox, oy) in outer {
        self.route(
            bx0 + ox,
            by0 + oy,
            bx1 + ox,
            by1 + oy,
            &rep,
            out,
            &mut |tile_box, mx, my| {
                let (cx0, cy0, cx1, cy1) = clip_box(
                    bx0 + ox + mx,
                    by0 + oy + my,
                    bx1 + ox + mx,
                    by1 + oy + my,
                    tile_box,
                );
                if cx1 > cx0 && cy1 > cy0 {
                    Some(Piece::Rect(cx0, cy0, cx1 - cx0, cy1 - cy0))
                } else {
                    None
                }
            },
            &mut |x, y| {
                Piece::Rect(
                    bx0 + ox + x,
                    by0 + oy + y,
                    bx1 - bx0,
                    by1 - by0,
                )
            },
            r.layer,
            r.dt,
        )?;
        }
        Ok(())
    }

    // ---- polygons ----------------------------------------------------

    fn emit_poly(
        &self,
        p: &PolyRec,
        xf: &Xf,
        ctx: &[Rep],
        out: &mut TileOut,
    ) -> Result<(), String> {
        let pts: Vec<(i64, i64)> =
            p.pts.iter().map(|&(x, y)| xf.apply(x, y)).collect();
        let (mut bx0, mut by0, mut bx1, mut by1) =
            (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for &(x, y) in &pts {
            bx0 = bx0.min(x);
            by0 = by0.min(y);
            bx1 = bx1.max(x);
            by1 = by1.max(y);
        }
        let (rep, outer) = fold_reps(xf_rep(&p.rep, xf), ctx)?;
        for (ox, oy) in outer {
        self.route(
            bx0 + ox,
            by0 + oy,
            bx1 + ox,
            by1 + oy,
            &rep,
            out,
            &mut |tile_box, mx, my| {
                let moved: Vec<(i64, i64)> = pts
                    .iter()
                    .map(|&(x, y)| (x + ox + mx, y + oy + my))
                    .collect();
                let clipped = clip_poly(&moved, tile_box);
                if clipped.len() >= 3 {
                    Some(Piece::Poly(clipped))
                } else {
                    None
                }
            },
            &mut |mx, my| {
                Piece::Poly(
                    pts.iter()
                        .map(|&(px, py)| (px + ox + mx, py + oy + my))
                        .collect(),
                )
            },
            p.layer,
            p.dt,
        )?;
        }
        Ok(())
    }

    // ---- shared routing ---------------------------------------------

    /// Route one record (base bbox + repetition, global frame) into
    /// tile/band buckets. `clip(tile_box, ox, oy)` returns the clipped
    /// piece of the member at offset (ox,oy); `whole(ox, oy)` the
    /// untouched member for fully-inside placement.
    #[allow(clippy::too_many_arguments)]
    fn route(
        &self,
        bx0: i64,
        by0: i64,
        bx1: i64,
        by1: i64,
        rep: &Rep,
        out: &mut TileOut,
        clip: &mut dyn FnMut((i64, i64, i64, i64), i64, i64) -> Option<Piece>,
        whole: &mut dyn FnMut(i64, i64) -> Piece,
        layer: u32,
        dt: u32,
    ) -> Result<(), String> {
        let g = self.grid;
        match rep {
            Rep::Grid { na, nb, va, vb }
                if is_axis(va, vb) && na * nb > 4 =>
            {
                // normalize to positive axis steps
                let (na, nb, ax0, ay0, dx, dy) =
                    norm_grid(*na as i64, *nb as i64, *va, *vb);
                // covered tile range
                let (tc_lo, tc_hi) = tile_range(
                    bx0 + ax0,
                    bx1 + ax0 + dx * (na - 1),
                    g.x0,
                    g.tw,
                    g.nx,
                );
                let (tr_lo, tr_hi) = tile_range(
                    by0 + ay0,
                    by1 + ay0 + dy * (nb - 1),
                    g.y0,
                    g.th,
                    g.ny,
                );
                for r in tr_lo..=tr_hi {
                    for c in tc_lo..=tc_hi {
                        let tb = g.tile_box(r, c);
                        // fully-inside member index ranges
                        let (fi0, fi1) = full_range(
                            bx0 + ax0,
                            bx1 + ax0,
                            dx,
                            na,
                            tb.0,
                            tb.2,
                        );
                        let (fj0, fj1) = full_range(
                            by0 + ay0,
                            by1 + ay0,
                            dy,
                            nb,
                            tb.1,
                            tb.3,
                        );
                        if fi0 <= fi1 && fj0 <= fj1 {
                            let piece =
                                whole(ax0 + fi0 * dx, ay0 + fj0 * dy);
                            let sub = if fi1 == fi0 && fj1 == fj0 {
                                Rep::One
                            } else {
                                Rep::Grid {
                                    na: (fi1 - fi0 + 1) as u64,
                                    nb: (fj1 - fj0 + 1) as u64,
                                    va: (dx, 0),
                                    vb: (0, dy),
                                }
                            };
                            let (w, h) = piece_bbox(&piece);
                            let band = self.band_of(w, h);
                            push(out, (r, c, band), piece, sub, layer, dt);
                        }
                        // boundary members: overlap the tile but are
                        // not fully inside on some axis
                        let (oi0, oi1) = overlap_range(
                            bx0 + ax0,
                            bx1 + ax0,
                            dx,
                            na,
                            tb.0,
                            tb.2,
                        );
                        let (oj0, oj1) = overlap_range(
                            by0 + ay0,
                            by1 + ay0,
                            dy,
                            nb,
                            tb.1,
                            tb.3,
                        );
                        for i in oi0..=oi1 {
                            for j in oj0..=oj1 {
                                let full_i = i >= fi0 && i <= fi1;
                                let full_j = j >= fj0 && j <= fj1;
                                if full_i && full_j {
                                    continue;
                                }
                                if let Some(pc) =
                                    clip(tb, ax0 + i * dx, ay0 + j * dy)
                                {
                                    let (w, h) = piece_bbox(&pc);
                                    let band = self.band_of(w, h);
                                    push(
                                        out,
                                        (r, c, band),
                                        pc,
                                        Rep::One,
                                        layer,
                                        dt,
                                    );
                                }
                            }
                        }
                    }
                }
            }
            _ => {
                // small grids, explicit points, singles, or diagonal
                // grids: per-member routing
                let n = rep.members();
                if n > DENSE_ITER_CAP {
                    return Err(format!(
                        "non-axis repetition with {} members (cap {})",
                        n, DENSE_ITER_CAP
                    ));
                }
                for (ox, oy) in rep_offsets(rep) {
                    let (mx0, my0, mx1, my1) =
                        (bx0 + ox, by0 + oy, bx1 + ox, by1 + oy);
                    let (tc_lo, tc_hi) =
                        tile_range(mx0, mx1, g.x0, g.tw, g.nx);
                    let (tr_lo, tr_hi) =
                        tile_range(my0, my1, g.y0, g.th, g.ny);
                    for r in tr_lo..=tr_hi {
                        for c in tc_lo..=tc_hi {
                            let tb = g.tile_box(r, c);
                            if mx0 >= tb.0
                                && my0 >= tb.1
                                && mx1 <= tb.2
                                && my1 <= tb.3
                            {
                                let piece = whole(ox, oy);
                                let (w, h) = piece_bbox(&piece);
                                let band = self.band_of(w, h);
                                push(
                                    out,
                                    (r, c, band),
                                    piece,
                                    Rep::One,
                                    layer,
                                    dt,
                                );
                            } else if let Some(pc) = clip(tb, ox, oy) {
                                let (w, h) = piece_bbox(&pc);
                                let band = self.band_of(w, h);
                                push(
                                    out,
                                    (r, c, band),
                                    pc,
                                    Rep::One,
                                    layer,
                                    dt,
                                );
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn piece_bbox(piece: &Piece) -> (i64, i64) {
    match piece {
        Piece::Rect(_, _, w, h) => (*w, *h),
        Piece::Poly(pts) => {
            let (mut x0, mut y0, mut x1, mut y1) =
                (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
            for &(x, y) in pts {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
            (x1 - x0, y1 - y0)
        }
    }
}

fn push(
    out: &mut TileOut,
    key: (i64, i64, usize),
    piece: Piece,
    rep: Rep,
    layer: u32,
    dt: u32,
) {
    let b = out.buckets.entry(key).or_default();
    b.members += rep.members();
    match piece {
        Piece::Rect(x, y, w, h) => {
            b.rects.push(RectRec { layer, dt, x, y, w, h, rep })
        }
        Piece::Poly(pts) => b.polys.push(PolyRec { layer, dt, pts, rep }),
    }
}

// ---------------------------------------------------------- rep helpers

const OUTER_CAP: u64 = 4_000_000;

/// Combine the shape's own repetition with the placement-context
/// layers: the layer with the most members survives as the emitted
/// record's repetition, every other layer expands into explicit
/// outer offsets (their Minkowski product).
fn fold_reps(own: Rep, ctx: &[Rep]) -> Result<(Rep, Vec<(i64, i64)>), String> {
    let mut layers: Vec<Rep> = Vec::with_capacity(ctx.len() + 1);
    if !matches!(own, Rep::One) {
        layers.push(own);
    }
    layers.extend(ctx.iter().cloned());
    if layers.is_empty() {
        return Ok((Rep::One, vec![(0, 0)]));
    }
    let imax = (0..layers.len())
        .max_by_key(|&i| layers[i].members())
        .unwrap();
    let keep = layers.swap_remove(imax);
    let mut outer: Vec<(i64, i64)> = vec![(0, 0)];
    for l in &layers {
        let offs = rep_offsets(l);
        if outer.len() as u64 * offs.len() as u64 > OUTER_CAP {
            return Err(format!(
                "nested repetition product exceeds {} (spike cap)",
                OUTER_CAP
            ));
        }
        let mut nxt = Vec::with_capacity(outer.len() * offs.len());
        for &(ax, ay) in &outer {
            for &(bx, by) in &offs {
                nxt.push((ax + bx, ay + by));
            }
        }
        outer = nxt;
    }
    Ok((keep, outer))
}

pub fn xf_rep(rep: &Rep, xf: &Xf) -> Rep {
    match rep {
        Rep::One => Rep::One,
        Rep::Grid { na, nb, va, vb } => Rep::Grid {
            na: *na,
            nb: *nb,
            va: xf.apply_vec(va.0, va.1),
            vb: xf.apply_vec(vb.0, vb.1),
        },
        Rep::Pts(p) => {
            Rep::Pts(p.iter().map(|&(x, y)| xf.apply_vec(x, y)).collect())
        }
    }
}

pub fn rep_offsets(rep: &Rep) -> Vec<(i64, i64)> {
    match rep {
        Rep::One => vec![(0, 0)],
        Rep::Grid { na, nb, va, vb } => {
            let mut v = Vec::with_capacity((na * nb) as usize);
            for j in 0..*nb as i64 {
                for i in 0..*na as i64 {
                    v.push((i * va.0 + j * vb.0, i * va.1 + j * vb.1));
                }
            }
            v
        }
        Rep::Pts(p) => p.clone(),
    }
}

pub fn is_axis(va: &(i64, i64), vb: &(i64, i64)) -> bool {
    (va.1 == 0 && vb.0 == 0) || (va.0 == 0 && vb.1 == 0)
}

/// normalize an axis grid to positive x-step (va) / y-step (vb);
/// returns (na, nb, anchor_shift_x, anchor_shift_y, dx, dy)
pub fn norm_grid(
    na: i64,
    nb: i64,
    va: (i64, i64),
    vb: (i64, i64),
) -> (i64, i64, i64, i64, i64, i64) {
    // put the x-step in va
    let (na, nb, va, vb) = if va.0 == 0 && vb.0 != 0 {
        (nb, na, vb, va)
    } else {
        (na, nb, va, vb)
    };
    let (mut ax, mut ay) = (0i64, 0i64);
    let mut dx = va.0;
    let mut dy = vb.1;
    if dx < 0 {
        ax += dx * (na - 1);
        dx = -dx;
    }
    if dy < 0 {
        ay += dy * (nb - 1);
        dy = -dy;
    }
    (na, nb, ax, ay, dx.max(1), dy.max(1))
}

/// tiles [lo..hi] overlapped by span [s0, s1) (clamped to the grid)
pub fn tile_range(s0: i64, s1: i64, g0: i64, step: i64, n: i64) -> (i64, i64) {
    let lo = ((s0 - g0).div_euclid(step)).clamp(0, n - 1);
    let hi = ((s1 - 1 - g0).div_euclid(step)).clamp(0, n - 1);
    (lo, hi)
}

/// member indexes whose [b0+i*d, b1+i*d] fits inside [t0, t1]
pub fn full_range(b0: i64, b1: i64, d: i64, n: i64, t0: i64, t1: i64) -> (i64, i64) {
    let lo = div_ceil(t0 - b0, d).max(0);
    let hi = div_floor(t1 - b1, d).min(n - 1);
    (lo, hi)
}

/// member indexes overlapping (t0, t1) at all
pub fn overlap_range(
    b0: i64,
    b1: i64,
    d: i64,
    n: i64,
    t0: i64,
    t1: i64,
) -> (i64, i64) {
    let lo = div_ceil(t0 - b1 + 1, d).max(0);
    let hi = div_floor(t1 - b0 - 1, d).min(n - 1);
    (lo, hi)
}

pub fn div_ceil(a: i64, b: i64) -> i64 {
    (a + b - 1).div_euclid(b)
}

pub fn div_floor(a: i64, b: i64) -> i64 {
    a.div_euclid(b)
}

// ------------------------------------------------------------- clipping

pub fn clip_box(
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
    t: (i64, i64, i64, i64),
) -> (i64, i64, i64, i64) {
    (x0.max(t.0), y0.max(t.1), x1.min(t.2), y1.min(t.3))
}

/// Sutherland-Hodgman against the axis-aligned tile box. Non-convex
/// subjects may come back with degenerate bridge edges; those are
/// area-neutral and the merged-Region XOR validation referees.
pub fn clip_poly(pts: &[(i64, i64)], t: (i64, i64, i64, i64)) -> Vec<(i64, i64)> {
    let mut cur: Vec<(i64, i64)> = pts.to_vec();
    for edge in 0..4 {
        if cur.is_empty() {
            return cur;
        }
        let mut next: Vec<(i64, i64)> = Vec::with_capacity(cur.len() + 4);
        let inside = |p: (i64, i64)| -> bool {
            match edge {
                0 => p.0 >= t.0,
                1 => p.1 >= t.1,
                2 => p.0 <= t.2,
                _ => p.1 <= t.3,
            }
        };
        let cross = |a: (i64, i64), b: (i64, i64)| -> (i64, i64) {
            let (line, vert) = match edge {
                0 => (t.0, true),
                1 => (t.1, false),
                2 => (t.2, true),
                _ => (t.3, false),
            };
            if vert {
                let dy = (b.1 - a.1) as i128 * (line - a.0) as i128
                    / (b.0 - a.0) as i128;
                (line, a.1 + dy as i64)
            } else {
                let dx = (b.0 - a.0) as i128 * (line - a.1) as i128
                    / (b.1 - a.1) as i128;
                (a.0 + dx as i64, line)
            }
        };
        let mut prev = *cur.last().unwrap();
        for &p in &cur {
            let pin = inside(p);
            let sin = inside(prev);
            if pin {
                if !sin {
                    next.push(cross(prev, p));
                }
                next.push(p);
            } else if sin {
                next.push(cross(prev, p));
            }
            prev = p;
        }
        cur = next;
    }
    // drop consecutive duplicates and a repeated closing vertex
    cur.dedup();
    if cur.len() > 1 && cur.first() == cur.last() {
        cur.pop();
    }
    cur
}
