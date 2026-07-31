//! Hierarchy-preserving band partitioner (production path).
//!
//! Reproduces klayout clip_into's variant mechanism at record level:
//! per tile, cells are clipped against a WINDOW propagated through
//! instance transforms; (cell, window) pairs memoize into variants -
//! an instance fully inside the window uses the shared full
//! definition `NAME__b<k>`, a straddling one a clipped variant
//! `NAME$v__b<k>`, exactly the naming the viewer already strips.
//! Each variant carries per-band content; per band the same tree is
//! mirrored and content-less subtrees are pruned, so the viewer's
//! DEPTH semantics (level = design level) survive unchanged.
//! Instance grid repetitions split arithmetically against the window
//! (interior sub-grid keeps its repetition record; boundary members
//! instantiate clipped variants singly).

use crate::{
    clip_box, clip_poly, div_ceil, div_floor, is_axis, norm_grid,
    path_bbox, path_outline, rep_offsets, xf_rep, Grid, Xf,
};
use floe_oasis::doc::{Doc, PathRec, PolyRec, RectRec, Rep};
use std::collections::HashMap;

const OUTER_CAP: u64 = 4_000_000;

type Win = (i64, i64, i64, i64); // x0, y0, x1, y1 (closed-open)

// ------------------------------------------------------------ cell bbox

/// local-frame bbox per design cell, content + descendants + reps.
/// Excludes texts: tiling windows mirror klayout clip AFTER the
/// source text strip.
pub fn cell_bboxes(doc: &Doc) -> Vec<Option<Win>> {
    cell_bboxes_opt(doc, false)
}

/// like cell_bboxes but with text anchor points included (klayout's
/// cell bbox counts texts as point boxes; the meta bbox and the grid
/// derive from the PRE-strip layout, so they need this variant)
pub fn cell_bboxes_full(doc: &Doc) -> Vec<Option<Win>> {
    cell_bboxes_opt(doc, true)
}

fn cell_bboxes_opt(doc: &Doc, with_texts: bool) -> Vec<Option<Win>> {
    fn walk(
        doc: &Doc,
        ci: usize,
        with_texts: bool,
        memo: &mut Vec<Option<Option<Win>>>,
    ) -> Option<Win> {
        if let Some(b) = memo[ci] {
            return b;
        }
        let cell = &doc.cells[ci];
        let mut b: Option<Win> = None;
        let mut grow = |x0: i64, y0: i64, x1: i64, y1: i64| {
            b = Some(match b {
                None => (x0, y0, x1, y1),
                Some(o) => (
                    o.0.min(x0),
                    o.1.min(y0),
                    o.2.max(x1),
                    o.3.max(y1),
                ),
            });
        };
        for r in &cell.rects {
            let (ex, ey) = rep_extent(&r.rep);
            grow(
                r.x.min(r.x + ex.0),
                r.y.min(r.y + ey.0),
                (r.x + r.w).max(r.x + r.w + ex.1),
                (r.y + r.h).max(r.y + r.h + ey.1),
            );
        }
        for p in &cell.polys {
            let (mut x0, mut y0, mut x1, mut y1) =
                (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
            for &(x, y) in &p.pts {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
            let (ex, ey) = rep_extent(&p.rep);
            grow(x0 + ex.0.min(0), y0 + ey.0.min(0),
                 x1 + ex.1.max(0), y1 + ey.1.max(0));
        }
        for pa in &cell.paths {
            let b = path_bbox(&pa.pts, pa.hw, pa.es, pa.ee);
            let (ex, ey) = rep_extent(&pa.rep);
            grow(b.0 + ex.0.min(0), b.1 + ey.0.min(0),
                 b.2 + ex.1.max(0), b.3 + ey.1.max(0));
        }
        if with_texts {
            for t in &cell.texts {
                let (ex, ey) = rep_extent(&t.rep);
                grow(t.x + ex.0.min(0), t.y + ey.0.min(0),
                     t.x + ex.1.max(0), t.y + ey.1.max(0));
            }
        }
        // avoid double-borrowing memo during child recursion
        let places = cell.places.clone();
        for pl in &places {
            if let Some(cb) = walk(doc, pl.cell, with_texts, memo) {
                let xf = Xf::place(pl.x, pl.y, pl.rot, pl.flip);
                let (a, bb) = (xf.apply(cb.0, cb.1), xf.apply(cb.2, cb.3));
                let (mut x0, mut x1) = (a.0.min(bb.0), a.0.max(bb.0));
                let (mut y0, mut y1) = (a.1.min(bb.1), a.1.max(bb.1));
                let rep_g = xf_rep(&pl.rep, &Xf::identity());
                let (ex, ey) = rep_extent(&rep_g);
                x0 += ex.0.min(0);
                x1 += ex.1.max(0);
                y0 += ey.0.min(0);
                y1 += ey.1.max(0);
                grow(x0, y0, x1, y1);
            }
        }
        memo[ci] = Some(b);
        b
    }
    let mut memo = vec![None; doc.cells.len()];
    for ci in 0..doc.cells.len() {
        walk(doc, ci, with_texts, &mut memo);
    }
    memo.into_iter().map(|m| m.unwrap()).collect()
}

/// min/max offset extents of a repetition: ((min_x, max_x), (min_y, max_y))
fn rep_extent(rep: &Rep) -> ((i64, i64), (i64, i64)) {
    match rep {
        Rep::One => ((0, 0), (0, 0)),
        Rep::Grid { na, nb, va, vb } => {
            let (na, nb) = (*na as i64 - 1, *nb as i64 - 1);
            let xs = [0, va.0 * na, vb.0 * nb, va.0 * na + vb.0 * nb];
            let ys = [0, va.1 * na, vb.1 * nb, va.1 * na + vb.1 * nb];
            (
                (*xs.iter().min().unwrap(), *xs.iter().max().unwrap()),
                (*ys.iter().min().unwrap(), *ys.iter().max().unwrap()),
            )
        }
        Rep::Pts(p) => {
            // single allocation-free pass: the routing pre-cull calls
            // this per (record, window)
            let (mut x0, mut x1, mut y0, mut y1) =
                (i64::MAX, i64::MIN, i64::MAX, i64::MIN);
            for &(x, y) in p {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
            ((x0, x1), (y0, y1))
        }
    }
}

// -------------------------------------------------------------- variants

#[derive(Default, Clone)]
pub struct BandContent {
    pub rects: Vec<RectRec>,
    pub polys: Vec<PolyRec>,
    pub paths: Vec<PathRec>,
}

pub struct VPlace {
    pub var: usize,
    pub x: i64,
    pub y: i64,
    pub rot: u8,
    pub flip: bool,
    pub rep: Rep,
}

pub struct VCell {
    pub design: usize,
    /// 0 = full definition, >0 = clipped variant ordinal
    pub ord: u32,
    pub bands: Vec<BandContent>,
    pub places: Vec<VPlace>,
}

pub struct TileTree {
    pub cells: Vec<VCell>,
    pub root: usize,
    /// reach[band][vcell] = subtree holds band content
    pub reach: Vec<Vec<bool>>,
    pub members: u64,
}

/// LOD whole-level cut: keep levels 0..=depth, ghost level depth+1.
pub struct LodCut {
    pub depth: usize,
    pub kept: Vec<usize>,
    pub ghosts: Vec<usize>,
}

impl TileTree {
    /// per-vcell stored member counts per (layer, dt), all bands
    /// together - klayout Shapes.size() counts array members, so this
    /// is the count the LOD cap and the density table are defined on
    pub fn cell_layer_members(&self) -> Vec<HashMap<(u32, u32), u64>> {
        self.cells
            .iter()
            .map(|vc| {
                let mut m: HashMap<(u32, u32), u64> = HashMap::new();
                for band in &vc.bands {
                    for r in &band.rects {
                        *m.entry((r.layer, r.dt)).or_default() +=
                            r.rep.members();
                    }
                    for p in &band.polys {
                        *m.entry((p.layer, p.dt)).or_default() +=
                            p.rep.members();
                    }
                    for pa in &band.paths {
                        *m.entry((pa.layer, pa.dt)).or_default() +=
                            pa.rep.members();
                    }
                }
                m
            })
            .collect()
    }

    /// distinct vcells by first-seen BFS level from the root - the
    /// level walk _tile_lod cuts on (variants sit at design depth)
    pub fn levels(&self) -> Vec<Vec<usize>> {
        let mut lvl = vec![usize::MAX; self.cells.len()];
        lvl[self.root] = 0;
        let mut levels = vec![vec![self.root]];
        loop {
            let mut nxt = Vec::new();
            for &i in levels.last().unwrap() {
                for p in &self.cells[i].places {
                    if lvl[p.var] == usize::MAX {
                        lvl[p.var] = levels.len();
                        nxt.push(p.var);
                    }
                }
            }
            if nxt.is_empty() {
                return levels;
            }
            levels.push(nxt);
        }
    }

    /// subtree bbox per vcell in its own frame (own content, placed
    /// children, repetition extents) - the GHOST outline klayout gets
    /// from Cell.bbox(). Forward pass: vcells are created children
    /// first, so child indexes are smaller than the parent's.
    pub fn subtree_bboxes(&self) -> Vec<Win> {
        let mut out: Vec<Win> = Vec::with_capacity(self.cells.len());
        for vc in &self.cells {
            let mut b: Option<Win> = None;
            let mut grow = |x0: i64, y0: i64, x1: i64, y1: i64| {
                b = Some(match b {
                    None => (x0, y0, x1, y1),
                    Some(o) => (
                        o.0.min(x0),
                        o.1.min(y0),
                        o.2.max(x1),
                        o.3.max(y1),
                    ),
                });
            };
            for band in &vc.bands {
                for r in &band.rects {
                    let (ex, ey) = rep_extent(&r.rep);
                    grow(
                        r.x + ex.0.min(0),
                        r.y + ey.0.min(0),
                        r.x + r.w + ex.1.max(0),
                        r.y + r.h + ey.1.max(0),
                    );
                }
                for p in &band.polys {
                    let (mut x0, mut y0, mut x1, mut y1) =
                        (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
                    for &(x, y) in &p.pts {
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x);
                        y1 = y1.max(y);
                    }
                    let (ex, ey) = rep_extent(&p.rep);
                    grow(x0 + ex.0.min(0), y0 + ey.0.min(0),
                         x1 + ex.1.max(0), y1 + ey.1.max(0));
                }
                for pa in &band.paths {
                    let b = path_bbox(&pa.pts, pa.hw, pa.es, pa.ee);
                    let (ex, ey) = rep_extent(&pa.rep);
                    grow(b.0 + ex.0.min(0), b.1 + ey.0.min(0),
                         b.2 + ex.1.max(0), b.3 + ey.1.max(0));
                }
            }
            for p in &vc.places {
                let cb = out[p.var]; // child index < parent index
                let xf = Xf::place(p.x, p.y, p.rot, p.flip);
                let (a, b2) = (xf.apply(cb.0, cb.1), xf.apply(cb.2, cb.3));
                let (mut x0, mut x1) = (a.0.min(b2.0), a.0.max(b2.0));
                let (mut y0, mut y1) = (a.1.min(b2.1), a.1.max(b2.1));
                let (ex, ey) = rep_extent(&p.rep); // offsets: parent frame
                x0 += ex.0.min(0);
                x1 += ex.1.max(0);
                y0 += ey.0.min(0);
                y1 += ey.1.max(0);
                grow(x0, y0, x1, y1);
            }
            // a vcell only exists with content or a content-bearing child
            out.push(b.expect("vcell without any extent"));
        }
        out
    }

    /// per-tile density table, mirroring cache._tile_density: walk
    /// level-synchronously with instance multiplicity (a cell placed
    /// at two depths counts at both, like the Python BFS); per layer
    /// the CUMULATIVE member count through each depth, the last entry
    /// folding everything deeper; "cells" = instances entering each
    /// level. Returns (per-layer arrays sorted by key, cells) or None
    /// when the tile has no shapes at all.
    pub fn density(
        &self,
        max_levels: usize,
    ) -> Option<(Vec<((u32, u32), Vec<u64>)>, Vec<u64>)> {
        let per_cell = self.cell_layer_members();
        let keys: Vec<(u32, u32)> = {
            let mut s: Vec<(u32, u32)> = per_cell
                .iter()
                .flat_map(|m| m.keys().copied())
                .collect();
            s.sort_unstable();
            s.dedup();
            s
        };
        let mut totals: HashMap<(u32, u32), u64> =
            keys.iter().map(|&k| (k, 0)).collect();
        let mut arrs: HashMap<(u32, u32), Vec<u64>> =
            keys.iter().map(|&k| (k, Vec::new())).collect();
        let mut cells_arr: Vec<u64> = Vec::new();
        let mut level: HashMap<usize, u64> = HashMap::new();
        level.insert(self.root, 1);
        let mut depth = 0usize;
        while !level.is_empty() {
            if depth <= max_levels {
                cells_arr.push(level.values().sum());
            }
            let mut nxt: HashMap<usize, u64> = HashMap::new();
            for (&i, &mult) in &level {
                for (k, n) in &per_cell[i] {
                    *totals.get_mut(k).unwrap() += n * mult;
                }
                for p in &self.cells[i].places {
                    *nxt.entry(p.var).or_default() +=
                        p.rep.members() * mult;
                }
            }
            if depth < max_levels {
                for k in &keys {
                    arrs.get_mut(k).unwrap().push(totals[k]);
                }
            }
            level = nxt;
            depth += 1;
        }
        let mut out: Vec<((u32, u32), Vec<u64>)> = Vec::new();
        for k in keys {
            let mut arr = arrs.remove(&k).unwrap();
            if let Some(last) = arr.last_mut() {
                *last = totals[&k];
            }
            if totals[&k] > 0 {
                out.push((k, arr));
            }
        }
        if out.is_empty() {
            None
        } else {
            Some((out, cells_arr))
        }
    }

    /// whole-level LOD cut under a stored-member cap, mirroring
    /// cache._tile_lod: keep levels while the running DISTINCT-cell
    /// member total stays <= cap. None = everything fits (the full
    /// tile doubles as its own LOD).
    pub fn lod_cut(&self, cap: u64) -> Option<LodCut> {
        let per_cell = self.cell_layer_members();
        let totals: Vec<u64> = per_cell
            .iter()
            .map(|m| m.values().sum::<u64>())
            .collect();
        let levels = self.levels();
        let count =
            |lv: &[usize]| lv.iter().map(|&i| totals[i]).sum::<u64>();
        let mut cut = 0usize;
        let mut cum = count(&levels[0]);
        while cut + 1 < levels.len() {
            cum += count(&levels[cut + 1]);
            if cum > cap {
                break;
            }
            cut += 1;
        }
        if cut + 1 >= levels.len() {
            return None;
        }
        Some(LodCut {
            depth: cut,
            kept: levels[..=cut].concat(),
            ghosts: levels[cut + 1].clone(),
        })
    }
}

pub struct HierTiler<'a> {
    pub doc: &'a Doc,
    pub grid: Grid,
    pub edges: Vec<i64>,
    pub nb: usize,
    pub bboxes: Vec<Option<Win>>,
}

struct TileBuild<'a> {
    t: &'a HierTiler<'a>,
    memo: HashMap<(usize, Win), Option<usize>>,
    cells: Vec<VCell>,
    ord_next: Vec<u32>,
    members: u64,
}

impl<'a> HierTiler<'a> {
    pub fn new(doc: &'a Doc, grid: Grid, edges_dbu: Vec<i64>) -> Self {
        let nb = edges_dbu.len() + 1;
        let bboxes = cell_bboxes(doc);
        HierTiler { doc, grid, edges: edges_dbu, nb, bboxes }
    }

    pub fn band_of(&self, w: i64, h: i64) -> usize {
        let s = w.max(h);
        for k in (1..self.nb).rev() {
            if s < self.edges[self.nb - 1 - k] {
                return k;
            }
        }
        0
    }

    /// Build the variant tree of one tile; None when the tile is empty.
    pub fn build_tile(&self, r: i64, c: i64) -> Result<Option<TileTree>, String> {
        let tb = (
            self.grid.x0 + c * self.grid.tw,
            self.grid.y0 + r * self.grid.th,
            self.grid.x0 + (c + 1) * self.grid.tw,
            self.grid.y0 + (r + 1) * self.grid.th,
        );
        let mut tb_ = TileBuild {
            t: self,
            memo: HashMap::new(),
            cells: Vec::new(),
            ord_next: vec![1; self.doc.cells.len()],
            members: 0,
        };
        let root = match tb_.variant(self.doc.top, tb)? {
            Some(v) => v,
            None => return Ok(None),
        };
        // per-band reachability for pruning
        let nb = self.nb;
        let n = tb_.cells.len();
        let mut reach = vec![vec![false; n]; nb];
        for k in 0..nb {
            // cells are created children-first (variant() recurses
            // before pushing itself), so child indexes are always
            // smaller than the parent's: one forward pass is exact
            for i in 0..n {
                let vc = &tb_.cells[i];
                let mut has = !vc.bands[k].rects.is_empty()
                    || !vc.bands[k].polys.is_empty()
                    || !vc.bands[k].paths.is_empty();
                if !has {
                    for p in &vc.places {
                        if reach[k][p.var] {
                            has = true;
                            break;
                        }
                    }
                }
                reach[k][i] = has;
            }
        }
        Ok(Some(TileTree {
            members: tb_.members,
            cells: tb_.cells,
            root,
            reach,
        }))
    }
}

impl<'a> TileBuild<'a> {
    /// Variant of design cell `ci` clipped to `win` (cell frame).
    fn variant(&mut self, ci: usize, win: Win) -> Result<Option<usize>, String> {
        let bb = match self.t.bboxes[ci] {
            Some(b) => b,
            None => return Ok(None), // empty cell
        };
        if bb.0 >= win.2 || bb.2 <= win.0 || bb.1 >= win.3 || bb.3 <= win.1 {
            return Ok(None); // fully outside
        }
        // canonicalize on win INTERSECT bbox: windows that differ only
        // outside the cell's extent clip identically, and klayout
        // shares those variants - without this, equivalent variants
        // duplicate and depth-level member counts run 2-3x high
        // (caught by the valmini level validation)
        let win = (
            win.0.max(bb.0),
            win.1.max(bb.1),
            win.2.min(bb.2),
            win.3.min(bb.3),
        );
        let full = win == bb;
        let key = (ci, win);
        if let Some(&v) = self.memo.get(&key) {
            return Ok(v);
        }
        // reserve the memo slot up front? recursion cannot revisit the
        // same key (windows shrink strictly down the tree), so insert
        // after building
        let cell = &self.t.doc.cells[ci];
        let nb = self.t.nb;
        let mut bands = vec![BandContent::default(); nb];
        let mut members = 0u64;
        for rec in &cell.rects {
            members += route_rect_window(
                self.t, rec, win, full, &mut bands,
            )?;
        }
        for rec in &cell.polys {
            members += route_poly_window(
                self.t, rec, win, full, &mut bands,
            )?;
        }
        for rec in &cell.paths {
            members += route_path_window(
                self.t, rec, win, full, &mut bands,
            )?;
        }
        let mut places: Vec<VPlace> = Vec::new();
        let plist = cell.places.clone();
        for pl in &plist {
            let cb = match self.t.bboxes[pl.cell] {
                Some(b) => b,
                None => continue,
            };
            let xf = Xf::place(pl.x, pl.y, pl.rot, pl.flip);
            // child bbox in parent frame (single member)
            let (a, b2) = (xf.apply(cb.0, cb.1), xf.apply(cb.2, cb.3));
            let (mx0, mx1) = (a.0.min(b2.0), a.0.max(b2.0));
            let (my0, my1) = (a.1.min(b2.1), a.1.max(b2.1));
            let rep = pl.rep.clone();
            // route members against the window like a "shape" of size
            // (mx1-mx0, my1-my0): interior sub-grid keeps the rep
            for act in route_members(
                win,
                full,
                (mx0, my0, mx1, my1),
                &rep,
            )? {
                match act {
                    Act::Single { ox, oy, inside: true } => {
                        // fully inside: shared full definition
                        if let Some(v) = self.variant(pl.cell, cb)? {
                            places.push(VPlace {
                                var: v,
                                x: pl.x + ox,
                                y: pl.y + oy,
                                rot: pl.rot,
                                flip: pl.flip,
                                rep: Rep::One,
                            });
                        }
                    }
                    Act::Single { ox, oy, inside: false } => {
                        // straddling member: window into child frame
                        let shifted = (
                            win.0 - ox,
                            win.1 - oy,
                            win.2 - ox,
                            win.3 - oy,
                        );
                        let wc = inv_window(&xf, shifted);
                        if let Some(v) = self.variant(pl.cell, wc)? {
                            places.push(VPlace {
                                var: v,
                                x: pl.x + ox,
                                y: pl.y + oy,
                                rot: pl.rot,
                                flip: pl.flip,
                                rep: Rep::One,
                            });
                        }
                    }
                    Act::Block { ox, oy, rep: sub } => {
                        // interior sub-grid: shared full def + rep
                        if let Some(v) = self.variant(pl.cell, cb)? {
                            places.push(VPlace {
                                var: v,
                                x: pl.x + ox,
                                y: pl.y + oy,
                                rot: pl.rot,
                                flip: pl.flip,
                                rep: sub,
                            });
                        }
                    }
                }
            }
        }
        let any = places.iter().len() > 0
            || bands.iter().any(|b| {
                !b.rects.is_empty()
                    || !b.polys.is_empty()
                    || !b.paths.is_empty()
            });
        let out = if any {
            let ord = if full {
                0
            } else {
                let o = self.ord_next[ci];
                self.ord_next[ci] = o + 1;
                o
            };
            self.cells.push(VCell { design: ci, ord, bands, places });
            self.members += members;
            Some(self.cells.len() - 1)
        } else {
            None
        };
        self.memo.insert(key, out);
        Ok(out)
    }
}

/// window in child frame: wc = xf^-1(win); orthogonal -> still a rect
fn inv_window(xf: &Xf, win: Win) -> Win {
    let inv = xf.invert();
    let a = inv.apply(win.0, win.1);
    let b = inv.apply(win.2, win.3);
    (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

// ------------------------------------------- record routing vs a window

/// One member-routing decision against a window.
pub enum Act {
    /// explicit member at offset; `inside` = fully within the window
    Single { ox: i64, oy: i64, inside: bool },
    /// interior sub-grid keeping a repetition record
    Block { ox: i64, oy: i64, rep: Rep },
}

/// Plan a repetition's members against one window.
fn route_members(
    win: Win,
    win_is_full: bool,
    b: Win, // base member bbox
    rep: &Rep,
) -> Result<Vec<Act>, String> {
    let mut acts: Vec<Act> = Vec::new();
    macro_rules! single {
        ($ox:expr, $oy:expr, $inside:expr) => {
            acts.push(Act::Single { ox: $ox, oy: $oy, inside: $inside })
        };
    }
    if win_is_full {
        // the whole cell is inside the window: no clipping anywhere
        match rep {
            Rep::One => {
                single!(0, 0, true);
                return Ok(acts);
            }
            r => {
                acts.push(Act::Block { ox: 0, oy: 0, rep: r.clone() });
                return Ok(acts);
            }
        }
    }
    // fast reject on the whole-repetition extent: a flat dense file
    // routes EVERY record against EVERY tile window, and without
    // this the Pts / non-axis branches expanded their member lists
    // (with allocations) for the 24 of 25 windows they never touch
    {
        let (ex, ey) = rep_extent(rep);
        if b.0 + ex.0 >= win.2
            || b.2 + ex.1 <= win.0
            || b.1 + ey.0 >= win.3
            || b.3 + ey.1 <= win.1
        {
            return Ok(acts);
        }
    }
    if matches!(rep, Rep::One) {
        // allocation-free single-member path (the bulk of flat files)
        let inside = b.0 >= win.0
            && b.1 >= win.1
            && b.2 <= win.2
            && b.3 <= win.3;
        single!(0, 0, inside);
        return Ok(acts);
    }
    match rep {
        Rep::Grid { na, nb, va, vb }
            if is_axis(va, vb) && na * nb > 4 =>
        {
            let (na, nb, ax0, ay0, dx, dy) =
                norm_grid(*na as i64, *nb as i64, *va, *vb);
            let (fi0, fi1) =
                full_range(b.0 + ax0, b.2 + ax0, dx, na, win.0, win.2);
            let (fj0, fj1) =
                full_range(b.1 + ay0, b.3 + ay0, dy, nb, win.1, win.3);
            if fi0 <= fi1 && fj0 <= fj1 {
                let sub = if fi0 == fi1 && fj0 == fj1 {
                    Rep::One
                } else {
                    Rep::Grid {
                        na: (fi1 - fi0 + 1) as u64,
                        nb: (fj1 - fj0 + 1) as u64,
                        va: (dx, 0),
                        vb: (0, dy),
                    }
                };
                let (ox, oy) = (ax0 + fi0 * dx, ay0 + fj0 * dy);
                match sub {
                    Rep::One => single!(ox, oy, true),
                    s => acts.push(Act::Block { ox, oy, rep: s }),
                }
            }
            let (oi0, oi1) =
                overlap_range(b.0 + ax0, b.2 + ax0, dx, na, win.0, win.2);
            let (oj0, oj1) =
                overlap_range(b.1 + ay0, b.3 + ay0, dy, nb, win.1, win.3);
            for i in oi0..=oi1 {
                for j in oj0..=oj1 {
                    if i >= fi0 && i <= fi1 && j >= fj0 && j <= fj1 {
                        continue;
                    }
                    single!(ax0 + i * dx, ay0 + j * dy, false);
                }
            }
            Ok(acts)
        }
        _ => {
            let n = rep.members();
            if n > OUTER_CAP {
                return Err(format!(
                    "non-axis repetition with {} members", n
                ));
            }
            for (ox, oy) in rep_offsets(rep) {
                let m = (b.0 + ox, b.1 + oy, b.2 + ox, b.3 + oy);
                if m.0 >= win.2 || m.2 <= win.0 || m.1 >= win.3
                    || m.3 <= win.1
                {
                    continue;
                }
                let inside = m.0 >= win.0 && m.1 >= win.1
                    && m.2 <= win.2 && m.3 <= win.3;
                single!(ox, oy, inside);
            }
            Ok(acts)
        }
    }
}

fn route_rect_window(
    t: &HierTiler,
    rec: &RectRec,
    win: Win,
    win_is_full: bool,
    bands: &mut [BandContent],
) -> Result<u64, String> {
    let b = (rec.x, rec.y, rec.x + rec.w, rec.y + rec.h);
    let mut members = 0u64;
    for act in route_members(win, win_is_full, b, &rec.rep)? {
        match act {
            Act::Single { ox, oy, inside: true } => {
                let k = t.band_of(rec.w, rec.h);
                bands[k].rects.push(RectRec {
                    x: rec.x + ox,
                    y: rec.y + oy,
                    rep: Rep::One,
                    ..*rec
                });
                members += 1;
            }
            Act::Single { ox, oy, inside: false } => {
                let (cx0, cy0, cx1, cy1) = clip_box(
                    b.0 + ox,
                    b.1 + oy,
                    b.2 + ox,
                    b.3 + oy,
                    win,
                );
                if cx1 > cx0 && cy1 > cy0 {
                    let k = t.band_of(cx1 - cx0, cy1 - cy0);
                    bands[k].rects.push(RectRec {
                        x: cx0,
                        y: cy0,
                        w: cx1 - cx0,
                        h: cy1 - cy0,
                        rep: Rep::One,
                        ..*rec
                    });
                    members += 1;
                }
            }
            Act::Block { ox, oy, rep: sub } => {
                let k = t.band_of(rec.w, rec.h);
                members += sub.members();
                bands[k].rects.push(RectRec {
                    x: rec.x + ox,
                    y: rec.y + oy,
                    rep: sub,
                    ..*rec
                });
            }
        }
    }
    Ok(members)
}

fn route_poly_window(
    t: &HierTiler,
    rec: &PolyRec,
    win: Win,
    win_is_full: bool,
    bands: &mut [BandContent],
) -> Result<u64, String> {
    let (mut x0, mut y0, mut x1, mut y1) =
        (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for &(x, y) in &rec.pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let mut members = 0u64;
    for act in route_members(win, win_is_full, (x0, y0, x1, y1), &rec.rep)? {
        match act {
            Act::Single { ox, oy, inside: true } => {
                let k = t.band_of(x1 - x0, y1 - y0);
                bands[k].polys.push(PolyRec {
                    layer: rec.layer,
                    dt: rec.dt,
                    pts: rec
                        .pts
                        .iter()
                        .map(|&(x, y)| (x + ox, y + oy))
                        .collect(),
                    rep: Rep::One,
                });
                members += 1;
            }
            Act::Single { ox, oy, inside: false } => {
                let moved: Vec<(i64, i64)> = rec
                    .pts
                    .iter()
                    .map(|&(x, y)| (x + ox, y + oy))
                    .collect();
                let clipped = clip_poly(&moved, win);
                if clipped.len() >= 3 {
                    let (mut a0, mut b0, mut a1, mut b1) =
                        (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
                    for &(x, y) in &clipped {
                        a0 = a0.min(x);
                        b0 = b0.min(y);
                        a1 = a1.max(x);
                        b1 = b1.max(y);
                    }
                    let k = t.band_of(a1 - a0, b1 - b0);
                    bands[k].polys.push(PolyRec {
                        layer: rec.layer,
                        dt: rec.dt,
                        pts: clipped,
                        rep: Rep::One,
                    });
                    members += 1;
                }
            }
            Act::Block { ox, oy, rep: sub } => {
                let k = t.band_of(x1 - x0, y1 - y0);
                members += sub.members();
                bands[k].polys.push(PolyRec {
                    layer: rec.layer,
                    dt: rec.dt,
                    pts: rec
                        .pts
                        .iter()
                        .map(|&(x, y)| (x + ox, y + oy))
                        .collect(),
                    rep: sub,
                });
            }
        }
    }
    Ok(members)
}

fn route_path_window(
    t: &HierTiler,
    rec: &PathRec,
    win: Win,
    win_is_full: bool,
    bands: &mut [BandContent],
) -> Result<u64, String> {
    let b = path_bbox(&rec.pts, rec.hw, rec.es, rec.ee);
    let mut members = 0u64;
    for act in route_members(win, win_is_full, b, &rec.rep)? {
        match act {
            Act::Single { ox, oy, inside: true } => {
                // klayout keeps a fully-covered path AS a path
                let k = t.band_of(b.2 - b.0, b.3 - b.1);
                bands[k].paths.push(PathRec {
                    layer: rec.layer,
                    dt: rec.dt,
                    pts: rec
                        .pts
                        .iter()
                        .map(|&(x, y)| (x + ox, y + oy))
                        .collect(),
                    hw: rec.hw,
                    es: rec.es,
                    ee: rec.ee,
                    rep: Rep::One,
                });
                members += 1;
            }
            Act::Single { ox, oy, inside: false } => {
                // klayout polygonizes a straddling path (measured:
                // square-join outline, then the window clip)
                let outline = path_outline(
                    &rec.pts, rec.hw, rec.es, rec.ee,
                )
                .ok_or_else(|| {
                    format!(
                        "non-manhattan path on layer {}/{} clipped                          at a tile boundary: not supported yet",
                        rec.layer, rec.dt
                    )
                })?;
                let moved: Vec<(i64, i64)> = outline
                    .iter()
                    .map(|&(x, y)| (x + ox, y + oy))
                    .collect();
                let clipped = clip_poly(&moved, win);
                if clipped.len() >= 3 {
                    let (mut a0, mut b0, mut a1, mut b1) =
                        (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
                    for &(x, y) in &clipped {
                        a0 = a0.min(x);
                        b0 = b0.min(y);
                        a1 = a1.max(x);
                        b1 = b1.max(y);
                    }
                    let k = t.band_of(a1 - a0, b1 - b0);
                    bands[k].polys.push(PolyRec {
                        layer: rec.layer,
                        dt: rec.dt,
                        pts: clipped,
                        rep: Rep::One,
                    });
                    members += 1;
                }
            }
            Act::Block { ox, oy, rep: sub } => {
                let k = t.band_of(b.2 - b.0, b.3 - b.1);
                members += sub.members();
                bands[k].paths.push(PathRec {
                    layer: rec.layer,
                    dt: rec.dt,
                    pts: rec
                        .pts
                        .iter()
                        .map(|&(x, y)| (x + ox, y + oy))
                        .collect(),
                    hw: rec.hw,
                    es: rec.es,
                    ee: rec.ee,
                    rep: sub,
                });
            }
        }
    }
    Ok(members)
}

fn full_range(b0: i64, b1: i64, d: i64, n: i64, t0: i64, t1: i64) -> (i64, i64) {
    let lo = div_ceil(t0 - b0, d).max(0);
    let hi = div_floor(t1 - b1, d).min(n - 1);
    (lo, hi)
}

fn overlap_range(
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
