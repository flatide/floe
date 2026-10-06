//! Occupancy density (design.ovs; 2026-10-06, opt-in: user "for one pixel's
//! dot the search goes much too far - put something the plans can consult
//! into the index", then "prototype it with design.ovo's bits and a small
//! mean cover, and compare on the synthetic chip", then "push it, I will try
//! it on a real chip").
//!
//! Pass 2 of the density stack asks, for each pixel pass 1 left open, which
//! visible layer covers it from the top and how densely; the hierarchy walk
//! answers that from the placements (field 2026-10-06: 25 M nodes and 20 s
//! for a full-depth all-layer view, 7.5 M nodes for 937 open pixels).
//! design.ovo already holds the where - per layer and placement depth, a bit
//! per cell of the flattened top cell, in 2x levels - but not the how much.
//! This file holds it: per layer, depth plane and level of design.ovo, for
//! each group of OVS_GROUP x OVS_GROUP cells, the area its sub-cut shapes
//! cover over the cells the plane's bits mark, as a byte (255: all of them).
//! Pass 2 then needs no walk: at a pixel a layer is present where the bit of
//! its cell is set, at the group's mean cover (floe_render_core::occ).
//!
//! Built from the index (design.ovm, design.ovb, design.ovh, design.ovp) and
//! design.ovo's grid: a cell that fits one level-0 cell stands for its cover
//! by layer and relative depth (cover::CellCover) where its placement puts
//! its box's centre; a larger one is walked into, its pages adding their
//! occupancy grids (design.ovb) where they lie. A shape counts at a level
//! while its smaller side is under OVS_SUB_CUT_CELLS of the level's cells
//! (pass 1 draws the larger ones at the views the level serves; class_of):
//! a cell under one level-0 cell holds nothing larger, a page whose largest
//! shape is under the level-0 cut counts whole, and a page holding a larger
//! shape is decoded once for its shapes by class - left out, the synthetic
//! MAIN01 1/10 lost the 27 % of its records such pages hold (1,649 pages;
//! layer 59/1 alone drew no dots where the walk drew 309 px). Coarser levels
//! sum the finer groups, their classes with them.
//!
//! File (little-endian), version 1: magic "FLOEOVS1", version u32, group u32,
//! src_size u64, src_mtime u64, cell_dbu i64, bbox x0 y0 x1 y1 i64, n_levels
//! u32, n_layers u32, then per layer: n_planes u8 (design.ovo's planes, 0
//! unless its status is ok), per plane: depth u8, per level: gw u32, gh u32,
//! off u64, len u64 (absolute; len 0: every group 0; else the gw x gh bytes
//! row by row, deflated).

use crate::cover::CellCover;
use crate::occupancy::{OvoFile, DEPTH_CAP, STATUS_OK};
use floe_oasis::doc::Rep;
use floe_ovm::{occ_cell, occ_coverage, occ_edge, Ovm, PageOcc, OCC_GRID};
use floe_tiler::Xf;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::rc::Rc;

pub const OVS_MAGIC: &[u8; 8] = b"FLOEOVS1";
pub const OVS_VERSION: u32 = 1;
/// cells per side of a group: one byte of a design.ovo row
pub const OVS_GROUP: u32 = 8;
/// a page or cell counts below this many level-0 cells (its largest side)
pub const OVS_SUB_CUT_CELLS: i64 = 3;
/// a Grid placement of a small cell past this many members is spread over
/// the footprint of its members' centres instead of placed member by member
const SPREAD_MEMBERS: u64 = 4096;

#[derive(Clone, Debug, Default)]
pub struct OvsStats {
    /// placements walked into (cells larger than a level-0 cell)
    pub walked: u64,
    /// small-cell members placed by their cover
    pub small: u64,
    /// Grid placements spread over their footprint
    pub spread: u64,
    /// pages added by their occupancy grids, and pages holding a shape past
    /// the cut (added by their decoded records' classes)
    pub pages: u64,
    pub big_pages: u64,
    /// distinct pages decoded for their classes, their stored bytes, and
    /// the seconds it took
    pub decoded: u64,
    pub decoded_bytes: u64,
    pub decode_s: f64,
    /// the level-0 group grids held at the walk's end (one per layer, plane
    /// and class with any area) and their bytes: the build's memory
    pub grids: u64,
    pub grid_bytes: u64,
    /// plane levels written, and those all zero
    pub levels: u64,
    pub empty_levels: u64,
    pub bytes: u64,
}

struct Builder<'a> {
    ovm: &'a Ovm,
    cover: &'a CellCover,
    c0: i64,
    x0: i64,
    y0: i64,
    w0: u32,
    h0: u32,
    gw0: u32,
    gh0: u32,
    thr: i64,
    n_levels: u32,
    /// per layer, the design.ovo plane of each placement depth
    plane_of: Vec<[Option<u8>; DEPTH_CAP as usize + 1]>,
    small: Vec<bool>,
    /// per (layer, plane, class), the level-0 groups' covered area (dbu^2,
    /// f32: a byte of mean comes of it); class c counts at levels c and up
    /// (class_of)
    acc: HashMap<(u32, u8, u8), Vec<f32>>,
    /// small cells placed: (cell, placement depth, group) -> members
    counts: HashMap<(u32, u8, u32), f64>,
    /// design.ovp, for the pages holding a shape past the cut, and those
    /// pages' classes once decoded
    ovp: Option<std::fs::File>,
    subs: HashMap<u32, Option<Rc<PageSub>>>,
    stats: OvsStats,
}

/// A decoded page's covered area by class: per class present, the area its
/// records of that class cover in each of the OCC_GRID x OCC_GRID cells over
/// the page bbox (floe_ovm::occ_edge's cells, as design.ovb's grid)
struct PageSub {
    classes: Vec<(u8, Vec<f32>)>,
}

/// One record's members (`rep`) at local offsets: `each` per member up to
/// SPREAD_MEMBERS; past that none, and Some(the members' count and the
/// footprint of their offsets) to spread instead
fn members_of(rep: &Rep, mut each: impl FnMut(i64, i64)) -> Option<(f64, (i64, i64, i64, i64))> {
    match rep {
        Rep::One => each(0, 0),
        Rep::Grid { na, nb, va, vb } => {
            let n = na.saturating_mul(*nb);
            if n > SPREAD_MEMBERS {
                let (la, lb) = (*na as i64 - 1, *nb as i64 - 1);
                let xs = [0, la * va.0, lb * vb.0, la * va.0 + lb * vb.0];
                let ys = [0, la * va.1, lb * vb.1, la * va.1 + lb * vb.1];
                let fp = (*xs.iter().min().unwrap(), *ys.iter().min().unwrap(), *xs.iter().max().unwrap(), *ys.iter().max().unwrap());
                return Some((n as f64, fp));
            }
            for jb in 0..*nb as i64 {
                for ia in 0..*na as i64 {
                    each(ia * va.0 + jb * vb.0, ia * va.1 + jb * vb.1);
                }
            }
        }
        Rep::Pts(pts) => {
            for &(dx, dy) in pts.iter() {
                each(dx, dy);
            }
        }
    }
    None
}

/// A shape's class by its smaller side: the first level whose cut (`thr` at
/// level 0, OVS_SUB_CUT_CELLS of its cells, doubling a level) it is under;
/// None at or past the last level's
pub fn class_of(thr: i64, n_levels: u32, min_side: i64) -> Option<u8> {
    let mut t = thr;
    for c in 0..n_levels {
        if min_side < t {
            return Some(c as u8);
        }
        t = t.saturating_mul(2);
    }
    None
}

impl Builder<'_> {
    fn plane(&self, k: u32, depth: u32) -> Option<u8> {
        self.plane_of.get(k as usize)?[depth.min(DEPTH_CAP as u32) as usize]
    }

    fn class_of(&self, min_side: i64) -> Option<u8> {
        class_of(self.thr, self.n_levels, min_side)
    }

    /// Page `pi`'s classes (PageSub), decoded on first use; None without
    /// design.ovp or where the page will not decode
    fn page_sub(&mut self, pi: u32) -> Option<Rc<PageSub>> {
        if let Some(known) = self.subs.get(&pi) {
            return known.clone();
        }
        let started = std::time::Instant::now();
        let made = self.decode_sub(pi).map(Rc::new);
        self.stats.decode_s += started.elapsed().as_secs_f64();
        self.subs.insert(pi, made.clone());
        made
    }

    fn decode_sub(&mut self, pi: u32) -> Option<PageSub> {
        use std::io::Seek;
        let pg = self.ovm.page(pi);
        if pg.codec != floe_ovm::CODEC_OASIS {
            return None;
        }
        let f = self.ovp.as_mut()?;
        let mut buf = vec![0u8; pg.csize as usize];
        f.seek(std::io::SeekFrom::Start(pg.file_off)).ok()?;
        f.read_exact(&mut buf).ok()?;
        self.stats.decoded += 1;
        self.stats.decoded_bytes += buf.len() as u64;
        let doc = floe_oasis::doc::parse_doc(&buf).ok()?;
        let cell = doc.cells.first()?;
        let bb = pg.bbox;
        let (ex, ey) = (bb.x1 - bb.x0, bb.y1 - bb.y0);
        let g = OCC_GRID as usize;
        let mut grids: Vec<Option<Vec<f32>>> = vec![None; self.n_levels as usize];
        // one record: its box (local), its area, its members
        let put = |grids: &mut Vec<Option<Vec<f32>>>, class: u8, (x0, y0, x1, y1): (i64, i64, i64, i64), area: f64, rep: &Rep| {
            let grid = grids[class as usize].get_or_insert_with(|| vec![0f32; g * g]);
            let (cx, cy) = ((x0 + x1) / 2, (y0 + y1) / 2);
            let spread = members_of(rep, |dx, dy| {
                let i = occ_cell(bb.x0, ex, cx + dx) as usize;
                let j = occ_cell(bb.y0, ey, cy + dy) as usize;
                grid[j * g + i] += area as f32;
            });
            if let Some((n, (fx0, fy0, fx1, fy1))) = spread {
                // the members' centres over their footprint, each grid cell
                // its share
                let (ax0, ax1, ay0, ay1) = (cx + fx0, cx + fx1, cy + fy0, cy + fy1);
                let (w, h) = ((ax1 - ax0).max(1) as f64, (ay1 - ay0).max(1) as f64);
                let (i0, i1) = (occ_cell(bb.x0, ex, ax0), occ_cell(bb.x0, ex, ax1));
                let (j0, j1) = (occ_cell(bb.y0, ey, ay0), occ_cell(bb.y0, ey, ay1));
                for j in j0..=j1 {
                    let oy = if ay1 > ay0 { (ay1.min(occ_edge(bb.y0, ey, j + 1)) - ay0.max(occ_edge(bb.y0, ey, j))).max(0) as f64 / h } else { 1.0 };
                    for i in i0..=i1 {
                        let ox = if ax1 > ax0 { (ax1.min(occ_edge(bb.x0, ex, i + 1)) - ax0.max(occ_edge(bb.x0, ex, i))).max(0) as f64 / w } else { 1.0 };
                        grid[j as usize * g + i as usize] += (n * area * ox * oy) as f32;
                    }
                }
            }
        };
        for r in &cell.rects {
            if let Some(class) = self.class_of(r.w.min(r.h)) {
                put(&mut grids, class, (r.x, r.y, r.x + r.w, r.y + r.h), r.w as f64 * r.h as f64, &r.rep);
            }
        }
        for p in &cell.polys {
            let (Some(x0), Some(x1)) = (p.pts.iter().map(|q| q.0).min(), p.pts.iter().map(|q| q.0).max()) else { continue };
            let (y0, y1) = (p.pts.iter().map(|q| q.1).min().unwrap_or(0), p.pts.iter().map(|q| q.1).max().unwrap_or(0));
            if let Some(class) = self.class_of((x1 - x0).min(y1 - y0)) {
                let twice: i128 = p.pts.iter().zip(p.pts.iter().cycle().skip(1)).map(|(a, b)| a.0 as i128 * b.1 as i128 - b.0 as i128 * a.1 as i128).sum();
                put(&mut grids, class, (x0, y0, x1, y1), twice.unsigned_abs() as f64 / 2.0, &p.rep);
            }
        }
        for p in &cell.paths {
            let (Some(x0), Some(x1)) = (p.pts.iter().map(|q| q.0).min(), p.pts.iter().map(|q| q.0).max()) else { continue };
            let (y0, y1) = (p.pts.iter().map(|q| q.1).min().unwrap_or(0), p.pts.iter().map(|q| q.1).max().unwrap_or(0));
            let (x0, y0, x1, y1) = (x0 - p.hw, y0 - p.hw, x1 + p.hw, y1 + p.hw);
            if let Some(class) = self.class_of((x1 - x0).min(y1 - y0)) {
                let len: f64 = p.pts.windows(2).map(|s| (((s[1].0 - s[0].0) as f64).powi(2) + ((s[1].1 - s[0].1) as f64).powi(2)).sqrt()).sum();
                let area = (len + (p.es + p.ee) as f64).max(0.0) * 2.0 * p.hw as f64;
                put(&mut grids, class, (x0, y0, x1, y1), area, &p.rep);
            }
        }
        let classes: Vec<(u8, Vec<f32>)> = grids.into_iter().enumerate().filter_map(|(c, grid)| grid.map(|grid| (c as u8, grid))).collect();
        Some(PageSub { classes })
    }

    fn group(&self, x: i64, y: i64) -> Option<u32> {
        let i = (x - self.x0).div_euclid(self.c0);
        let j = (y - self.y0).div_euclid(self.c0);
        if i < 0 || j < 0 || i >= self.w0 as i64 || j >= self.h0 as i64 {
            return None;
        }
        Some((j as u32 / OVS_GROUP) * self.gw0 + i as u32 / OVS_GROUP)
    }

    fn add(&mut self, k: u32, p: u8, class: u8, x: i64, y: i64, area: f64) {
        let Some(g) = self.group(x, y) else { return };
        let n = (self.gw0 * self.gh0) as usize;
        self.acc.entry((k, p, class)).or_insert_with(|| vec![0.0; n])[g as usize] += area as f32;
    }

    fn count(&mut self, child: u32, depth: u32, x: i64, y: i64, n: f64) {
        let Some(g) = self.group(x, y) else { return };
        *self.counts.entry((child, depth.min(DEPTH_CAP as u32) as u8, g)).or_insert(0.0) += n;
    }

    /// `n` members spread evenly over a footprint (world, the members'
    /// centres): each group takes its share of the footprint
    fn spread_count(&mut self, child: u32, depth: u32, (fx0, fy0, fx1, fy1): (i64, i64, i64, i64), n: f64) {
        let gside = self.c0 * OVS_GROUP as i64;
        let (w, h) = ((fx1 - fx0).max(1) as f64, (fy1 - fy0).max(1) as f64);
        let gi0 = (fx0 - self.x0).div_euclid(gside).max(0);
        let gi1 = (fx1 - self.x0).div_euclid(gside).min(self.gw0 as i64 - 1);
        let gj0 = (fy0 - self.y0).div_euclid(gside).max(0);
        let gj1 = (fy1 - self.y0).div_euclid(gside).min(self.gh0 as i64 - 1);
        for gj in gj0..=gj1 {
            let (ya, yb) = (self.y0 + gj * gside, self.y0 + (gj + 1) * gside);
            let oy = if fy1 > fy0 { (fy1.min(yb) - fy0.max(ya)).max(0) as f64 / h } else { 1.0 };
            for gi in gi0..=gi1 {
                let (xa, xb) = (self.x0 + gi * gside, self.x0 + (gi + 1) * gside);
                let ox = if fx1 > fx0 { (fx1.min(xb) - fx0.max(xa)).max(0) as f64 / w } else { 1.0 };
                let share = n * ox * oy;
                if share > 0.0 {
                    let g = gj as u32 * self.gw0 + gi as u32;
                    *self.counts.entry((child, depth.min(DEPTH_CAP as u32) as u8, g)).or_insert(0.0) += share;
                }
            }
        }
    }

    fn walk(&mut self, ci: u32, xf: &Xf, depth: u32) {
        let ovm = self.ovm;
        // its own pages, by their occupancy grids
        let (start, count) = ovm.cell_pranges(ci);
        for pri in start..start.saturating_add(count) {
            let pr = ovm.prange(pri);
            let k = pr.layer_idx;
            let Some(p) = self.plane(k, depth) else { continue };
            for pi in pr.page_lo..pr.page_lo.saturating_add(pr.page_count) {
                let pg = ovm.page(pi);
                if pg.max_min as i64 >= self.thr {
                    // a shape past the cut: its records by class, decoded
                    // once (FLOE_OVS_DECODE=off: left out, as first built)
                    self.stats.big_pages += 1;
                    let Some(sub) = self.page_sub(pi) else { continue };
                    let bb = pg.bbox;
                    let (ex, ey) = (bb.x1 - bb.x0, bb.y1 - bb.y0);
                    let g = OCC_GRID as usize;
                    for (class, grid) in &sub.classes {
                        for cy in 0..OCC_GRID {
                            let (ya, yb) = (occ_edge(bb.y0, ey, cy), occ_edge(bb.y0, ey, cy + 1));
                            for cx in 0..OCC_GRID {
                                let area = grid[cy as usize * g + cx as usize];
                                if area > 0.0 {
                                    let (xa, xb) = (occ_edge(bb.x0, ex, cx), occ_edge(bb.x0, ex, cx + 1));
                                    let (wx, wy) = xf.apply((xa + xb) / 2, (ya + yb) / 2);
                                    self.add(k, p, *class, wx, wy, f64::from(area));
                                }
                            }
                        }
                    }
                    continue;
                }
                self.stats.pages += 1;
                match ovm.page_occ(pi) {
                    Some(PageOcc::Grid(levels)) => {
                        let bb = pg.bbox;
                        let (ex, ey) = (bb.x1 - bb.x0, bb.y1 - bb.y0);
                        for cy in 0..OCC_GRID {
                            let (ya, yb) = (occ_edge(bb.y0, ey, cy), occ_edge(bb.y0, ey, cy + 1));
                            for cx in 0..OCC_GRID {
                                let level = levels[(cy * OCC_GRID + cx) as usize];
                                if level == 0 {
                                    continue;
                                }
                                let (xa, xb) = (occ_edge(bb.x0, ex, cx), occ_edge(bb.x0, ex, cx + 1));
                                let area = occ_coverage(level) * (xb - xa) as f64 * (yb - ya) as f64;
                                let (wx, wy) = xf.apply((xa + xb) / 2, (ya + yb) / 2);
                                self.add(k, p, 0, wx, wy, area);
                            }
                        }
                    }
                    Some(PageOcc::Total(area)) => {
                        let bb = pg.bbox;
                        let (wx, wy) = xf.apply((bb.x0 + bb.x1) / 2, (bb.y0 + bb.y1) / 2);
                        self.add(k, p, 0, wx, wy, area);
                    }
                    None => {}
                }
            }
        }
        // its placements: a small child by its cover, a larger one walked
        let (ps, pc) = ovm.cell_places(ci);
        for i in ps as u64..ps as u64 + pc as u64 {
            let pl = ovm.place(i);
            let child = pl.child;
            if child >= ovm.n_cells {
                continue;
            }
            if self.small[child as usize] {
                let rb = ovm.cell_rbbox(child);
                let (lx, ly) = ((rb.x0 + rb.x1) / 2, (rb.y0 + rb.y1) / 2);
                let at = |dx: i64, dy: i64| xf.compose(&Xf::place(pl.x + dx, pl.y + dy, pl.rot, pl.flip)).apply(lx, ly);
                match &pl.rep {
                    Rep::One => {
                        let (wx, wy) = at(0, 0);
                        self.count(child, depth + 1, wx, wy, 1.0);
                        self.stats.small += 1;
                    }
                    Rep::Grid { na, nb, va, vb } => {
                        let n = na.saturating_mul(*nb);
                        self.stats.small += n;
                        if n > SPREAD_MEMBERS {
                            let (la, lb) = (*na as i64 - 1, *nb as i64 - 1);
                            let corners = [
                                at(0, 0),
                                at(la * va.0, la * va.1),
                                at(lb * vb.0, lb * vb.1),
                                at(la * va.0 + lb * vb.0, la * va.1 + lb * vb.1),
                            ];
                            let fx0 = corners.iter().map(|c| c.0).min().unwrap_or(0);
                            let fx1 = corners.iter().map(|c| c.0).max().unwrap_or(0);
                            let fy0 = corners.iter().map(|c| c.1).min().unwrap_or(0);
                            let fy1 = corners.iter().map(|c| c.1).max().unwrap_or(0);
                            self.spread_count(child, depth + 1, (fx0, fy0, fx1, fy1), n as f64);
                            self.stats.spread += 1;
                        } else {
                            for jb in 0..*nb as i64 {
                                for ia in 0..*na as i64 {
                                    let (wx, wy) = at(ia * va.0 + jb * vb.0, ia * va.1 + jb * vb.1);
                                    self.count(child, depth + 1, wx, wy, 1.0);
                                }
                            }
                        }
                    }
                    Rep::Pts(pts) => {
                        self.stats.small += pts.len() as u64;
                        for &(dx, dy) in pts.iter() {
                            let (wx, wy) = at(dx, dy);
                            self.count(child, depth + 1, wx, wy, 1.0);
                        }
                    }
                }
            } else {
                let members: Vec<(i64, i64)> = match &pl.rep {
                    Rep::One => vec![(0, 0)],
                    Rep::Grid { na, nb, va, vb } => {
                        let mut out = Vec::with_capacity((*na * *nb) as usize);
                        for jb in 0..*nb as i64 {
                            for ia in 0..*na as i64 {
                                out.push((ia * va.0 + jb * vb.0, ia * va.1 + jb * vb.1));
                            }
                        }
                        out
                    }
                    Rep::Pts(pts) => pts.to_vec(),
                };
                for (dx, dy) in members {
                    let m = xf.compose(&Xf::place(pl.x + dx, pl.y + dy, pl.rot, pl.flip));
                    self.stats.walked += 1;
                    self.walk(child, &m, depth + 1);
                }
            }
        }
    }

    /// the small cells' covers, each where its members were counted: the
    /// part of its cover at each relative depth, into the plane of the
    /// depth it lands at
    fn settle(&mut self) {
        let counts = std::mem::take(&mut self.counts);
        let mut slices: HashMap<u32, Vec<Vec<(u32, f64)>>> = HashMap::new();
        let n_groups = (self.gw0 * self.gh0) as usize;
        for ((child, d, g), n) in counts {
            let sl = slices.entry(child).or_insert_with(|| depth_slices(self.ovm, self.cover, child));
            for (j, layers) in sl.iter().enumerate() {
                for &(k, area) in layers {
                    let Some(p) = self.plane_of.get(k as usize).and_then(|row| row[(d as usize + j).min(DEPTH_CAP as usize)]) else {
                        continue;
                    };
                    self.acc.entry((k, p, 0)).or_insert_with(|| vec![0.0; n_groups])[g as usize] += (n * area) as f32;
                }
            }
        }
    }
}

/// A cell's covered area by layer at each relative depth (0: its own
/// shapes): the differences of CellCover's areas with 0, 1, ... levels
/// shown below it.
fn depth_slices(ovm: &Ovm, cover: &CellCover, ci: u32) -> Vec<Vec<(u32, f64)>> {
    let height = ovm.cell_height(ci);
    let mut out = Vec::with_capacity(height as usize + 1);
    let mut prev: HashMap<u32, f64> = HashMap::new();
    for j in 0..=height {
        let cum = cover.areas(ovm, ci, j);
        let mut slice = Vec::new();
        let mut now: HashMap<u32, f64> = HashMap::new();
        for &(k, area) in cum.iter() {
            now.insert(k, area);
            let d = area - prev.get(&k).copied().unwrap_or(0.0);
            if d > 0.0 {
                slice.push((k, d));
            }
        }
        out.push(slice);
        prev = now;
    }
    out
}

fn deflate(bytes: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
    enc.write_all(bytes).expect("deflate design.ovs");
    enc.finish().expect("deflate design.ovs")
}

/// design.ovs for an index (`ovm`, with design.ovb attached), its design.ovo
/// and its cells' cover, with its pages (`ovp`, design.ovp: the pages that
/// hold a shape past the cut decoded for their smaller shapes; None leaves
/// them out): (the file's bytes, what the build did).
pub fn build(ovm: &Ovm, ovo: &OvoFile, cover: &CellCover, ovp: Option<&str>) -> Result<(Vec<u8>, OvsStats), String> {
    ovo.validate_against(ovm)?;
    if !ovm.has_page_occ() {
        return Err("design.ovs needs design.ovb (the pages' occupancy grids)".into());
    }
    let c0 = ovo.cell_dbu;
    if c0 <= 0 || ovo.w == 0 || ovo.h == 0 {
        return Err("design.ovo has no grid".into());
    }
    let mut plane_of = vec![[None; DEPTH_CAP as usize + 1]; ovo.layers.len()];
    for (k, layer) in ovo.layers.iter().enumerate() {
        if layer.status != STATUS_OK {
            continue;
        }
        for (p, plane) in layer.planes.iter().enumerate() {
            let d = plane.depth.min(DEPTH_CAP) as usize;
            plane_of[k][d] = Some(p as u8);
        }
    }
    let small: Vec<bool> = (0..ovm.n_cells)
        .map(|ci| {
            let b = ovm.cell_rbbox(ci);
            !b.is_empty() && b.x1 - b.x0 <= c0 && b.y1 - b.y0 <= c0
        })
        .collect();
    let (gw0, gh0) = (ovo.w.div_ceil(OVS_GROUP), ovo.h.div_ceil(OVS_GROUP));
    let mut b = Builder {
        ovm,
        cover,
        c0,
        x0: ovo.bbox.0,
        y0: ovo.bbox.1,
        w0: ovo.w,
        h0: ovo.h,
        gw0,
        gh0,
        thr: OVS_SUB_CUT_CELLS * c0,
        n_levels: ovo.n_levels,
        plane_of,
        small,
        acc: HashMap::new(),
        counts: HashMap::new(),
        ovp: match ovp {
            Some(path) => Some(std::fs::File::open(path).map_err(|e| format!("{}: {}", path, e))?),
            None => None,
        },
        subs: HashMap::new(),
        stats: OvsStats::default(),
    };
    if ovm.top < ovm.n_cells {
        b.walk(ovm.top, &Xf::identity(), 0);
    }
    b.settle();
    b.stats.grids = b.acc.len() as u64;
    b.stats.grid_bytes = b.acc.values().map(|grid| (grid.len() * std::mem::size_of::<f32>()) as u64).sum();
    // the decoded pages' classes are spent
    b.subs.clear();
    let n_levels = ovo.n_levels as usize;
    // per layer, per plane, per level: (gw, gh, stored bytes)
    let mut blobs: Blobs = Vec::with_capacity(ovo.layers.len());
    for (k, layer) in ovo.layers.iter().enumerate() {
        let mut planes_out = Vec::new();
        if layer.status == STATUS_OK {
            for (p, plane) in layer.planes.iter().enumerate() {
                // each class's groups at the level being written (pooled
                // level by level); a level sums the classes up to its own
                let mut classes: Vec<Option<Vec<f64>>> =
                    (0..n_levels).map(|c| b.acc.remove(&(k as u32, p as u8, c as u8)).map(|grid| grid.into_iter().map(f64::from).collect())).collect();
                let (mut gw, mut gh) = (gw0, gh0);
                let mut levels_out = Vec::with_capacity(n_levels);
                for lv in 0..n_levels {
                    let Some((w, h, bits)) = ovo.plane_level(k, p, lv) else {
                        levels_out.push((0, 0, Vec::new()));
                        continue;
                    };
                    let (lgw, lgh) = (w.div_ceil(OVS_GROUP), h.div_ceil(OVS_GROUP));
                    if lv > 0 {
                        for grid in classes.iter_mut().flatten() {
                            let mut pooled = vec![0.0f64; (lgw * lgh) as usize];
                            for gj in 0..gh {
                                for gi in 0..gw {
                                    let (ti, tj) = (gi / 2, gj / 2);
                                    if ti < lgw && tj < lgh {
                                        pooled[(tj * lgw + ti) as usize] += grid[(gj * gw + gi) as usize];
                                    }
                                }
                            }
                            *grid = pooled;
                        }
                    }
                    gw = lgw;
                    gh = lgh;
                    let mut areas = vec![0.0f64; (gw * gh) as usize];
                    for grid in classes[..=lv].iter().flatten() {
                        for (a, &g) in areas.iter_mut().zip(grid.iter()) {
                            *a += g;
                        }
                    }
                    let cell = (c0 as f64) * (1u64 << lv) as f64;
                    let row_bytes = (w as usize).div_ceil(8);
                    let mut means = vec![0u8; (gw * gh) as usize];
                    let mut any = false;
                    for gj in 0..gh {
                        for gi in 0..gw {
                            let a = areas[(gj * gw + gi) as usize];
                            if !(a > 0.0) {
                                continue;
                            }
                            let mut occ = 0u32;
                            for r in 0..OVS_GROUP {
                                let row = gj * OVS_GROUP + r;
                                if row >= h {
                                    break;
                                }
                                occ += bits[row as usize * row_bytes + gi as usize].count_ones();
                            }
                            if occ == 0 {
                                continue;
                            }
                            let mean = (a / (occ as f64 * cell * cell)).clamp(0.0, 1.0);
                            means[(gj * gw + gi) as usize] = ((mean * 255.0).round() as u8).max(1);
                            any = true;
                        }
                    }
                    b.stats.levels += 1;
                    if any {
                        levels_out.push((gw, gh, deflate(&means)));
                    } else {
                        b.stats.empty_levels += 1;
                        levels_out.push((gw, gh, Vec::new()));
                    }
                }
                planes_out.push((plane.depth, levels_out));
            }
        }
        blobs.push(planes_out);
    }
    let out = encode(ovo.src_size, ovo.src_mtime, ovo.cell_dbu, ovo.bbox, ovo.n_levels, &blobs);
    b.stats.bytes = out.len() as u64;
    Ok((out, b.stats))
}

/// Per layer, per plane, its (depth, per level (gw, gh, the stored bytes:
/// empty for all zero, else deflated)).
pub type Blobs = Vec<Vec<(u8, Vec<(u32, u32, Vec<u8>)>)>>;

/// design.ovs's bytes: header, table, body (the module's format; build
/// writes it, tests make small ones)
pub fn encode(src_size: u64, src_mtime: u64, cell_dbu: i64, bbox: (i64, i64, i64, i64), n_levels: u32, blobs: &Blobs) -> Vec<u8> {
    let header_len = 8 + 4 + 4 + 8 + 8 + 8 + 32 + 4 + 4;
    let table_len: usize = blobs.iter().map(|planes| 1 + planes.iter().map(|(_, levels)| 1 + levels.len() * 24).sum::<usize>()).sum();
    let mut out = Vec::with_capacity(header_len + table_len);
    out.extend_from_slice(OVS_MAGIC);
    out.extend_from_slice(&OVS_VERSION.to_le_bytes());
    out.extend_from_slice(&OVS_GROUP.to_le_bytes());
    out.extend_from_slice(&src_size.to_le_bytes());
    out.extend_from_slice(&src_mtime.to_le_bytes());
    out.extend_from_slice(&cell_dbu.to_le_bytes());
    for v in [bbox.0, bbox.1, bbox.2, bbox.3] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&n_levels.to_le_bytes());
    out.extend_from_slice(&(blobs.len() as u32).to_le_bytes());
    let body_start = (header_len + table_len) as u64;
    let mut body: Vec<u8> = Vec::new();
    for planes in blobs {
        out.push(planes.len() as u8);
        for (depth, levels) in planes {
            out.push(*depth);
            for (gw, gh, data) in levels {
                out.extend_from_slice(&gw.to_le_bytes());
                out.extend_from_slice(&gh.to_le_bytes());
                out.extend_from_slice(&(body_start + body.len() as u64).to_le_bytes());
                out.extend_from_slice(&(data.len() as u64).to_le_bytes());
                body.extend_from_slice(data);
            }
        }
    }
    debug_assert_eq!(out.len() as u64, body_start);
    out.extend_from_slice(&body);
    out
}

/// One plane's levels in the file: (gw, gh, offset, length) each.
#[derive(Clone, Debug)]
pub struct OvsPlane {
    pub depth: u8,
    pub levels: Vec<(u32, u32, u64, u64)>,
}

/// A read design.ovs (structure checked; `validate_against` checks it
/// belongs to a design.ovo).
pub struct OvsFile {
    data: Vec<u8>,
    pub group: u32,
    pub src_size: u64,
    pub src_mtime: u64,
    pub cell_dbu: i64,
    pub bbox: (i64, i64, i64, i64),
    pub n_levels: u32,
    pub layers: Vec<Vec<OvsPlane>>,
}

impl std::fmt::Debug for OvsFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OvsFile(cell_dbu={} levels={} layers={} bytes={})", self.cell_dbu, self.n_levels, self.layers.len(), self.data.len())
    }
}

fn g32(b: &[u8], o: usize) -> Result<u32, String> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).ok_or_else(|| "design.ovs: truncated".to_string())
}
fn g64(b: &[u8], o: usize) -> Result<u64, String> {
    b.get(o..o + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap())).ok_or_else(|| "design.ovs: truncated".to_string())
}

impl OvsFile {
    pub fn open(path: &str) -> Result<OvsFile, String> {
        let data = std::fs::read(path).map_err(|e| format!("{}: {}", path, e))?;
        OvsFile::from_bytes(data)
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<OvsFile, String> {
        if data.len() < 80 || &data[..8] != OVS_MAGIC {
            return Err("design.ovs: not an occupancy density file".into());
        }
        let version = g32(&data, 8)?;
        if version != OVS_VERSION {
            return Err(format!("design.ovs: version {} (this build reads {})", version, OVS_VERSION));
        }
        let group = g32(&data, 12)?;
        let src_size = g64(&data, 16)?;
        let src_mtime = g64(&data, 24)?;
        let cell_dbu = g64(&data, 32)? as i64;
        let bbox = (g64(&data, 40)? as i64, g64(&data, 48)? as i64, g64(&data, 56)? as i64, g64(&data, 64)? as i64);
        let n_levels = g32(&data, 72)?;
        let n_layers = g32(&data, 76)?;
        let mut at = 80usize;
        let mut layers = Vec::with_capacity(n_layers as usize);
        for _ in 0..n_layers {
            let n_planes = *data.get(at).ok_or("design.ovs: truncated")? as usize;
            at += 1;
            let mut planes = Vec::with_capacity(n_planes);
            for _ in 0..n_planes {
                let depth = *data.get(at).ok_or("design.ovs: truncated")?;
                at += 1;
                let mut levels = Vec::with_capacity(n_levels as usize);
                for _ in 0..n_levels {
                    let (gw, gh, off, len) = (g32(&data, at)?, g32(&data, at + 4)?, g64(&data, at + 8)?, g64(&data, at + 16)?);
                    at += 24;
                    if off.checked_add(len).is_none_or(|end| end > data.len() as u64) {
                        return Err("design.ovs: a level past the end".into());
                    }
                    levels.push((gw, gh, off, len));
                }
                planes.push(OvsPlane { depth, levels });
            }
            layers.push(planes);
        }
        Ok(OvsFile { data, group, src_size, src_mtime, cell_dbu, bbox, n_levels, layers })
    }

    /// Whether this file was built for `ovo`: the same source, grid and planes.
    pub fn validate_against(&self, ovo: &OvoFile) -> Result<(), String> {
        if (self.src_size, self.src_mtime) != (ovo.src_size, ovo.src_mtime) {
            return Err("design.ovs was built for another source".into());
        }
        if self.cell_dbu != ovo.cell_dbu || self.bbox != ovo.bbox || self.n_levels != ovo.n_levels {
            return Err("design.ovs was built on another occupancy grid".into());
        }
        if self.group != OVS_GROUP || self.layers.len() != ovo.layers.len() {
            return Err("design.ovs: another group or layer table".into());
        }
        for (k, layer) in ovo.layers.iter().enumerate() {
            let want: Vec<u8> = if layer.status == STATUS_OK { layer.planes.iter().map(|p| p.depth).collect() } else { Vec::new() };
            let have: Vec<u8> = self.layers[k].iter().map(|p| p.depth).collect();
            if want != have {
                return Err(format!("design.ovs: layer {} has other planes than design.ovo", k));
            }
        }
        Ok(())
    }

    /// The (gw, gh, means) of one plane's level: a byte per group row by
    /// row (0 where the plane covers nothing under the cut).
    pub fn means(&self, k: usize, p: usize, lv: usize) -> Option<(u32, u32, Vec<u8>)> {
        let &(gw, gh, off, len) = self.layers.get(k)?.get(p)?.levels.get(lv)?;
        let n = gw as usize * gh as usize;
        if len == 0 {
            return Some((gw, gh, vec![0; n]));
        }
        let mut out = Vec::with_capacity(n);
        let mut dec = flate2::read::DeflateDecoder::new(&self.data[off as usize..(off + len) as usize]);
        dec.read_to_end(&mut out).ok()?;
        (out.len() == n).then_some((gw, gh, out))
    }

    pub fn bytes(&self) -> usize {
        self.data.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shape_counts_from_the_first_level_whose_cut_it_is_under() {
        // level 0's cut 300 dbu, doubling: 300, 600, 1200 over three levels
        assert_eq!(class_of(300, 3, 0), Some(0));
        assert_eq!(class_of(300, 3, 299), Some(0));
        assert_eq!(class_of(300, 3, 300), Some(1));
        assert_eq!(class_of(300, 3, 599), Some(1));
        assert_eq!(class_of(300, 3, 1199), Some(2));
        // at or past the last level's cut: pass 1 draws it at every level
        assert_eq!(class_of(300, 3, 1200), None);
        assert_eq!(class_of(300, 0, 1), None);
    }

    #[test]
    fn members_go_one_by_one_up_to_the_spread_and_as_a_footprint_past_it() {
        let mut seen = Vec::new();
        assert_eq!(members_of(&Rep::One, |x, y| seen.push((x, y))), None);
        assert_eq!(seen, vec![(0, 0)]);
        seen.clear();
        let grid = Rep::Grid { na: 3, nb: 2, va: (10, 0), vb: (0, 5) };
        assert_eq!(members_of(&grid, |x, y| seen.push((x, y))), None);
        assert_eq!(seen, vec![(0, 0), (10, 0), (20, 0), (0, 5), (10, 5), (20, 5)]);
        seen.clear();
        let pts = Rep::Pts(std::sync::Arc::from(vec![(0, 0), (7, -3)]));
        assert_eq!(members_of(&pts, |x, y| seen.push((x, y))), None);
        assert_eq!(seen, vec![(0, 0), (7, -3)]);
        seen.clear();
        // past SPREAD_MEMBERS: no member, the count and the footprint of the
        // offsets (a skew grid's four corners)
        let big = Rep::Grid { na: 100, nb: 41, va: (10, 1), vb: (-2, 20) };
        assert_eq!(members_of(&big, |x, y| seen.push((x, y))), Some((4100.0, (-80, 0, 990, 899))));
        assert!(seen.is_empty());
    }

    #[test]
    fn the_file_reads_back_what_was_written() {
        let means: Vec<u8> = (0..6).map(|v| v * 40).collect();
        let blobs: Blobs = vec![
            vec![(0, vec![(3, 2, deflate(&means)), (2, 1, Vec::new())]), (3, vec![(3, 2, Vec::new()), (2, 1, deflate(&[255, 1]))])],
            Vec::new(),
        ];
        let bytes = encode(123, 456, 4000, (-10, -20, 30, 40), 2, &blobs);
        let f = OvsFile::from_bytes(bytes.clone()).unwrap();
        assert_eq!((f.group, f.src_size, f.src_mtime, f.cell_dbu, f.bbox, f.n_levels), (OVS_GROUP, 123, 456, 4000, (-10, -20, 30, 40), 2));
        assert_eq!(f.layers.len(), 2);
        assert_eq!(f.layers[0].iter().map(|p| p.depth).collect::<Vec<_>>(), vec![0, 3]);
        assert!(f.layers[1].is_empty());
        assert_eq!(f.means(0, 0, 0), Some((3, 2, means)));
        assert_eq!(f.means(0, 0, 1), Some((2, 1, vec![0, 0])));
        assert_eq!(f.means(0, 1, 1), Some((2, 1, vec![255, 1])));
        assert_eq!(f.means(0, 2, 0), None);
        assert_eq!(f.means(1, 0, 0), None);
        // refused: another magic, another version, a table past the end
        let mut other = bytes.clone();
        other[0] = b'X';
        assert!(OvsFile::from_bytes(other).is_err());
        let mut version = bytes.clone();
        version[8] = 9;
        assert!(OvsFile::from_bytes(version).unwrap_err().contains("version"));
        assert!(OvsFile::from_bytes(bytes[..bytes.len() - 3].to_vec()).is_err());
    }
}
