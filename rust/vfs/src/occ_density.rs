//! Occupancy density (design.ovs; 2026-10-06, drawn by default since
//! 0.12.317): what the density stack's pass 2 draws by unless
//! FLOE_RUST_DENSITY_OCC=off - no plan, no walk
//! and no decode at frame time (floe_render_core::occ).
//!
//! User 2026-10-06: "for one pixel's dot the search goes much too far - put
//! something the plans can consult into the index", "any depth, later a start
//! and an end depth", "the index file may grow too large". The field's
//! full-depth all-layer view planned pass 2 for 19-20 s (25.0M nodes, 15.0M
//! reads; 937 open pixels still 855 ms).
//!
//! Per layer and placement depth (a plane: 0 the top cell's own shapes,
//! DEPTH_CAP that depth and every deeper one) and per level of a grid of its
//! own (the top cell's box in cells of `--um` - auto: the smallest power of
//! two microns keeping its longer side within OVS_AUTO_CELLS cells - 2x
//! coarser a level down to OVS_TOP_GRID cells), the file holds a bit per cell
//! (the cell holds a shape under the cut of that layer and depth) and per
//! group of OVS_GROUP x OVS_GROUP cells a byte: the area those shapes cover
//! over the cells the bits mark (255: all of them). A shape is under a level's
//! cut while its larger side is under OVS_SUB_CUT_CELLS of the level's cells -
//! the frame's per-shape cut (renderd ShapeCut::Larger, the default; class_of:
//! pass 1 draws the larger ones at the views the level serves, a thin one
//! longer than the cut as a hairline). A frame draws at the level whose cut
//! covers its own (floe_render_core::occ::choose_level).
//!
//! Version 1 (91ecf7a) took its bits from design.ovo: 3,362 s for the
//! synthetic MAIN01 1/10 on a laptop, past 5 hours on the real chip (user
//! 2026-10-06: "this won't do"). Version 2 marks them in the one walk that
//! sums the cover:
//! - a cell under the cut on both sides (its box under OVS_SUB_CUT_CELLS
//!   cells) is counted where its placements put it, per (cell, depth): the
//!   cells its box covers and its members per group; at the end its cover by
//!   layer and relative depth (cover::CellCover) goes to those planes;
//! - a larger cell is walked into: its pages add their occupancy grids
//!   (design.ovb) where they lie, and a page holding a shape over the cut or
//!   without a grid is decoded once for its shapes by class - left out (the
//!   first prototype), the synthetic lost the 27 % of its records such pages
//!   hold (1,649 pages; layer 59/1 alone drew no dots where the walk drew 309
//!   px).
//! Coarser levels OR the bits and sum the areas, the classes up to the level.
//! A page that will not read or decode fails the build (the last file stays).
//! Version 3 (review 2026-10-07) classes shapes by the larger side (version 2
//! by the smaller one brought the frame's hairlines back as dots) and counts a
//! top under the cut; a version-2 file is refused until built again.
//!
//! File (little-endian), version 3: magic "FLOEOVS1", version u32, group u32,
//! src_size u64, src_mtime u64 (the index's), unit f64 (dbu per um),
//! cell_dbu i64, x0 i64, y0 i64, w u32, h u32 (level 0's cells), n_levels u32,
//! n_layers u32 (the index's), then per layer: n_planes u8, per plane: depth
//! u8, per level: the bits' off u64 and len u64, the means' off u64 and len
//! u64 (absolute; len 0: all zero; else deflated - the bits row by row in
//! row-padded bytes, least significant bit first; the means a byte per group
//! row by row).

use crate::cover::CellCover;
use crate::hier::FxMap;
use crate::occupancy::DEPTH_CAP;
use floe_oasis::doc::Rep;
use floe_ovm::{occ_coverage, occ_edge, Ovm, PageOcc, OCC_GRID};
use floe_tiler::Xf;
use std::collections::HashMap;
use std::io::{Read, Seek, Write};
use std::rc::Rc;

pub const OVS_MAGIC: &[u8; 8] = b"FLOEOVS1";
pub const OVS_VERSION: u32 = 3;
/// a big cell's file (a root view's): version 3 and its placement in the top
pub const OVS_VERSION_ROOT: u32 = 4;
/// a cell under the top this share of its box's area or more gets a file of
/// its own (`floe-index ovs --roots`)
pub const OVS_ROOT_SHARE: f64 = 0.25;
/// cells per side of a group: one byte of a row
pub const OVS_GROUP: u32 = 8;
/// a shape counts at a level while its larger side is under this many of the
/// level's cells; a cell under it on both sides is counted, not walked
pub const OVS_SUB_CUT_CELLS: i64 = 3;
/// the coarsest level has at most this many cells a side
pub const OVS_TOP_GRID: u32 = 64;
/// the automatic base cell keeps the top cell's longer side within this many
pub const OVS_AUTO_CELLS: f64 = 2048.0;
const BASE_STEPS_UM: [f64; 11] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0];
/// a Grid placement past this many members is spread over the footprint of
/// its members instead of placed member by member
const SPREAD_MEMBERS: u64 = 4096;
const HEADER_LEN: usize = 80;
/// version 4: the root cell, its placement's rot, flip and x, y
const ROOT_LEN: usize = 24;
/// cells a side of a sparse tile (Tiles): a u64 word a row
const TILE: u32 = 64;

/// The file's grid: level 0's cells over the top cell's box, and its levels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OvsGrid {
    /// dbu per um
    pub unit: f64,
    pub cell_dbu: i64,
    pub x0: i64,
    pub y0: i64,
    pub w: u32,
    pub h: u32,
    pub n_levels: u32,
}

impl OvsGrid {
    /// The grid over `bbox` (dbu: x0, y0, x1, y1) in cells of `base_um`;
    /// None for an empty box, a cell under a dbu or past u32 cells.
    pub fn new(unit: f64, base_um: f64, bbox: (i64, i64, i64, i64)) -> Option<OvsGrid> {
        let (x0, y0, x1, y1) = bbox;
        if !(unit > 0.0) || !(base_um > 0.0) || x1 <= x0 || y1 <= y0 {
            return None;
        }
        let cell_dbu = (base_um * unit).round() as i64;
        if cell_dbu <= 0 {
            return None;
        }
        let cells = |span: i64| u32::try_from((span as i128 + cell_dbu as i128 - 1) / cell_dbu as i128).ok();
        let (w, h) = (cells(x1 - x0)?, cells(y1 - y0)?);
        Some(OvsGrid { unit, cell_dbu, x0, y0, w, h, n_levels: level_count(w, h) })
    }

    /// The automatic base cell (um) for a box `span` dbu on its longer side:
    /// the smallest step keeping it within OVS_AUTO_CELLS cells.
    pub fn auto_base_um(unit: f64, span: i64) -> f64 {
        let span_um = span as f64 / unit;
        BASE_STEPS_UM.iter().copied().find(|&b| span_um / b <= OVS_AUTO_CELLS).unwrap_or(BASE_STEPS_UM[BASE_STEPS_UM.len() - 1])
    }

    /// (w, h) cells of level `lv`
    pub fn level_dims(&self, lv: u32) -> (u32, u32) {
        let s = 1u32 << lv.min(31);
        (self.w.div_ceil(s), self.h.div_ceil(s))
    }

    pub fn base_um(&self) -> f64 {
        self.cell_dbu as f64 / self.unit
    }

    /// level 0's cell (i or j) of a coordinate along x (`x`) or y, the
    /// first or last for one outside
    fn cell_of(&self, v: i64, x: bool) -> u32 {
        let (o, n) = if x { (self.x0, self.w) } else { (self.y0, self.h) };
        (v - o).div_euclid(self.cell_dbu).clamp(0, i64::from(n) - 1) as u32
    }

    /// level 0's cells [i0, i1] x [j0, j1] a box (x0, y0, x1, y1) covers
    fn cells(&self, (x0, y0, x1, y1): (i64, i64, i64, i64)) -> (u32, u32, u32, u32) {
        (self.cell_of(x0, true), self.cell_of((x1 - 1).max(x0), true), self.cell_of(y0, false), self.cell_of((y1 - 1).max(y0), false))
    }
}

/// The levels of a grid of `w` x `h` cells: 2x coarser each down to
/// OVS_TOP_GRID a side
fn level_count(w: u32, h: u32) -> u32 {
    let (mut a, mut b, mut n) = (w, h, 1u32);
    while a > OVS_TOP_GRID || b > OVS_TOP_GRID {
        a = a.div_ceil(2);
        b = b.div_ceil(2);
        n += 1;
    }
    n
}

/// Level 0's cells of coordinates, by a reciprocal corrected to the exact
/// floor (the walk takes four a member: i64 division was a fifth of it)
#[derive(Clone, Copy)]
struct FastCells {
    x0: i64,
    y0: i64,
    c: i64,
    inv: f64,
    w: i64,
    h: i64,
}

impl FastCells {
    fn new(g: &OvsGrid) -> FastCells {
        FastCells { x0: g.x0, y0: g.y0, c: g.cell_dbu, inv: 1.0 / g.cell_dbu as f64, w: i64::from(g.w), h: i64::from(g.h) }
    }

    /// floor(d / c) within [0, n)
    #[inline]
    fn at(&self, d: i64, n: i64) -> u32 {
        let mut q = (d as f64 * self.inv).floor() as i64;
        if (q + 1) * self.c <= d {
            q += 1;
        } else if q * self.c > d {
            q -= 1;
        }
        q.clamp(0, n - 1) as u32
    }

    /// OvsGrid::cells
    #[inline]
    fn cells(&self, (x0, y0, x1, y1): (i64, i64, i64, i64)) -> (u32, u32, u32, u32) {
        (
            self.at(x0 - self.x0, self.w),
            self.at((x1 - 1).max(x0) - self.x0, self.w),
            self.at(y0 - self.y0, self.h),
            self.at((y1 - 1).max(y0) - self.y0, self.h),
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct OvsStats {
    /// placements walked into (cells over the cut)
    pub walked: u64,
    /// members of cells under the cut counted, and the Grid placements of
    /// them spread over their footprint
    pub small: u64,
    pub spread: u64,
    /// page visits added by their occupancy grids, and by their decoded
    /// shapes (a shape over the cut, or no grid)
    pub pages: u64,
    pub big_pages: u64,
    /// distinct pages decoded, their stored bytes, the seconds it took and
    /// on how many threads
    pub decoded: u64,
    pub decoded_bytes: u64,
    pub decode_s: f64,
    pub decode_jobs: u64,
    /// seconds of the walk, of the cells under the cut's settling, of the
    /// levels' writing
    pub walk_s: f64,
    pub settle_s: f64,
    pub write_s: f64,
    /// (cell, depth) counts of cells under the cut and their sparse tiles
    pub small_keys: u64,
    pub small_tiles: u64,
    /// planes (layer, depth, class) held at the walk's end and their bytes:
    /// the build's memory
    pub grids: u64,
    pub grid_bytes: u64,
    /// plane levels written, and those with no cell set
    pub levels: u64,
    pub empty_levels: u64,
    pub bytes: u64,
    /// the cells under the top that got a file of their own
    pub roots: u64,
}

/// u64 words a row of `w` cells takes
fn words_of(w: u32) -> usize {
    (w as usize).div_ceil(64)
}

/// Sets cells [i0, i1] x [j0, j1] of a plane of `words` words a row.
fn set_cells(bits: &mut [u64], words: usize, (i0, i1, j0, j1): (u32, u32, u32, u32)) {
    let (w0, w1) = ((i0 / 64) as usize, (i1 / 64) as usize);
    for j in j0 as usize..=j1 as usize {
        for w in w0..=w1 {
            let lo = if w == w0 { i0 % 64 } else { 0 };
            let hi = if w == w1 { i1 % 64 } else { 63 };
            bits[j * words + w] |= (u64::MAX >> (63 - hi)) & (u64::MAX << lo);
        }
    }
}

/// The OR of each pair of neighbouring bits of `x`, in its low 32 bits.
fn pair_or(x: u64) -> u64 {
    let mut y = (x | (x >> 1)) & 0x5555_5555_5555_5555;
    y = (y | (y >> 1)) & 0x3333_3333_3333_3333;
    y = (y | (y >> 2)) & 0x0F0F_0F0F_0F0F_0F0F;
    y = (y | (y >> 4)) & 0x00FF_00FF_00FF_00FF;
    y = (y | (y >> 8)) & 0x0000_FFFF_0000_FFFF;
    (y | (y >> 16)) & 0x0000_0000_FFFF_FFFF
}

/// The next coarser level of a plane of `w` x `h` cells: a cell set where
/// any of its 2 x 2 is.
fn pool_bits(bits: &[u64], w: u32, h: u32) -> Vec<u64> {
    let words = words_of(w);
    let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
    let nwords = words_of(nw);
    let mut out = vec![0u64; nwords * nh as usize];
    for r in 0..nh as usize {
        let (a, b) = (2 * r, (2 * r + 1).min(h as usize - 1));
        for o in 0..nwords {
            let word = |at: usize| if at < words { bits[a * words + at] | bits[b * words + at] } else { 0 };
            out[r * nwords + o] = pair_or(word(2 * o)) | (pair_or(word(2 * o + 1)) << 32);
        }
    }
    out
}

/// The next coarser level of `gw` x `gh` groups' areas: each the sum of its
/// 2 x 2.
fn pool_area(area: &[f32], gw: u32, gh: u32) -> Vec<f32> {
    let nw = gw.div_ceil(2);
    let mut out = vec![0f32; (nw * gh.div_ceil(2)) as usize];
    for gj in 0..gh {
        for gi in 0..gw {
            out[((gj / 2) * nw + gi / 2) as usize] += area[(gj * gw + gi) as usize];
        }
    }
    out
}

/// The cells set in group (gi, gj) of a plane of `words` words a row and `h`
/// rows (a group is a byte of a word).
fn group_occ(bits: &[u64], words: usize, h: u32, gi: u32, gj: u32) -> u32 {
    let (w, shift) = ((gi * OVS_GROUP / 64) as usize, gi * OVS_GROUP % 64);
    (gj * OVS_GROUP..((gj + 1) * OVS_GROUP).min(h)).map(|r| ((bits[r as usize * words + w] >> shift) & 0xFF).count_ones()).sum()
}

/// A plane's rows as the file stores them: row-padded bytes.
fn row_bytes(bits: &[u64], words: usize, w: u32, h: u32) -> Vec<u8> {
    let rb = (w as usize).div_ceil(8);
    let mut out = Vec::with_capacity(rb * h as usize);
    for r in 0..h as usize {
        let row: Vec<u8> = bits[r * words..(r + 1) * words].iter().flat_map(|word| word.to_le_bytes()).collect();
        out.extend_from_slice(&row[..rb]);
    }
    out
}

/// Sparse level-0 bits: TILE x TILE-cell tiles, made where a cell is set (a
/// cell under the cut is placed over a part of the chip); the last tile
/// marked at hand (a placement's members lie close together).
#[derive(Default)]
struct Tiles {
    index: FxMap<u32, usize>,
    tiles: Vec<(u32, [u64; TILE as usize])>,
    last: Option<(u32, usize)>,
}

impl Tiles {
    #[inline]
    fn tile(&mut self, t: u32) -> &mut [u64; TILE as usize] {
        let at = match self.last {
            Some((lt, at)) if lt == t => at,
            _ => {
                let n = self.tiles.len();
                let at = *self.index.entry(t).or_insert(n);
                if at == n {
                    self.tiles.push((t, [0; TILE as usize]));
                }
                self.last = Some((t, at));
                at
            }
        };
        &mut self.tiles[at].1
    }

    fn set(&mut self, tiles_w: u32, (i0, i1, j0, j1): (u32, u32, u32, u32)) {
        for tj in j0 / TILE..=j1 / TILE {
            let (r0, r1) = (j0.max(tj * TILE) - tj * TILE, j1.min(tj * TILE + TILE - 1) - tj * TILE);
            for ti in i0 / TILE..=i1 / TILE {
                let (c0, c1) = (i0.max(ti * TILE) - ti * TILE, i1.min(ti * TILE + TILE - 1) - ti * TILE);
                let mask = (u64::MAX >> (63 - c1)) & (u64::MAX << c0);
                for row in &mut self.tile(tj * tiles_w + ti)[r0 as usize..=r1 as usize] {
                    *row |= mask;
                }
            }
        }
    }

    /// ORs them into a dense plane of `words` words a row and `h` rows (a
    /// tile's column is a word).
    fn or_into(&self, tiles_w: u32, bits: &mut [u64], words: usize, h: u32) {
        for (t, tile) in &self.tiles {
            let (ti, tj) = ((t % tiles_w) as usize, t / tiles_w);
            for (r, &row) in tile.iter().enumerate() {
                let at = tj * TILE + r as u32;
                if at >= h {
                    break;
                }
                bits[at as usize * words + ti] |= row;
            }
        }
    }
}

/// A cell under the cut placed at one depth: where its members are (the
/// cells their boxes cover) and how many in each group.
#[derive(Default)]
struct SmallAcc {
    groups: FxMap<u32, f32>,
    tiles: Tiles,
    /// the group being counted and its members so far (flush)
    last: Option<(u32, f32)>,
}

impl SmallAcc {
    #[inline]
    fn count(&mut self, g: u32, n: f32) {
        match &mut self.last {
            Some((lg, c)) if *lg == g => *c += n,
            _ => {
                self.flush();
                self.last = Some((g, n));
            }
        }
    }

    fn flush(&mut self) {
        if let Some((g, c)) = self.last.take() {
            *self.groups.entry(g).or_insert(0.0) += c;
        }
    }
}

/// One plane being summed: level 0's bits and its groups' covered area
/// (dbu^2; f32 - a byte of mean comes of it).
struct PlaneAcc {
    bits: Vec<u64>,
    area: Vec<f32>,
}

/// A decoded page's shapes by class: the area they cover in each of the
/// page's local cells of the grid's size (from the page box's corner).
struct PageSub {
    classes: Vec<(u8, Vec<(i32, i32, f32)>)>,
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

/// A shape's class by its larger side - the frame's per-shape cut (renderd
/// ShapeCut::Larger, the default: a thin shape longer than the cut is pass
/// 1's, a hairline; review 2026-10-07: by the smaller side 32 wires of 0.4 x
/// 32 um came back as dots, 639 px where the frame drew 396): the first
/// level whose cut (`thr` at level 0, OVS_SUB_CUT_CELLS of its cells,
/// doubling a level) it is under; None at or past the last level's
pub fn class_of(thr: i64, n_levels: u32, side: i64) -> Option<u8> {
    let mut t = thr;
    for c in 0..n_levels {
        if side < t {
            return Some(c as u8);
        }
        t = t.saturating_mul(2);
    }
    None
}

/// A box under a transform, as a box
fn world_rect(xf: &Xf, (x0, y0, x1, y1): (i64, i64, i64, i64)) -> (i64, i64, i64, i64) {
    let ((ax, ay), (bx, by)) = (xf.apply(x0, y0), xf.apply(x1, y1));
    (ax.min(bx), ay.min(by), ax.max(bx), ay.max(by))
}

/// The groups of cells [i0, i1] x [j0, j1] with the cells of it each holds:
/// (group index, cells)
fn groups_of(gw: u32, (i0, i1, j0, j1): (u32, u32, u32, u32), mut each: impl FnMut(u32, u32)) {
    for gj in j0 / OVS_GROUP..=j1 / OVS_GROUP {
        let cj = j1.min(gj * OVS_GROUP + OVS_GROUP - 1) - j0.max(gj * OVS_GROUP) + 1;
        for gi in i0 / OVS_GROUP..=i1 / OVS_GROUP {
            let ci = i1.min(gi * OVS_GROUP + OVS_GROUP - 1) - i0.max(gi * OVS_GROUP) + 1;
            each(gj * gw + gi, ci * cj);
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

/// Page `pi` decoded for its shapes by class (PageSub) with its stored
/// bytes; an error where it will not read or decode (review 2026-10-07: a
/// page that failed was taken for an empty one, and the build wrote a file
/// short of it over the last one)
fn decode_page(ovm: &Ovm, ovp: &mut std::fs::File, pi: u32, g: &OvsGrid, thr: i64) -> Result<(PageSub, u64), String> {
    let pg = ovm.page(pi);
    if pg.codec != floe_ovm::CODEC_OASIS {
        return Err(format!("page {}: codec {} (OASIS only)", pi, pg.codec));
    }
    let mut buf = vec![0u8; pg.csize as usize];
    ovp.seek(std::io::SeekFrom::Start(pg.file_off)).map_err(|e| format!("page {}: {}", pi, e))?;
    ovp.read_exact(&mut buf).map_err(|e| format!("page {}: {}", pi, e))?;
    let doc = floe_oasis::doc::parse_doc(&buf).map_err(|e| format!("page {}: {}", pi, e))?;
    let cell = doc.cells.first().ok_or_else(|| format!("page {}: no cell", pi))?;
    Ok((page_cells(cell, (pg.bbox.x0, pg.bbox.y0), g.cell_dbu, thr, g.n_levels), buf.len() as u64))
}

/// A page's shapes under the cut by class (PageSub): each record's box at
/// each member - a path's by its outline, extensions included - with its
/// area over the local cells of `c0` from `origin` it covers, by their
/// overlap
fn page_cells(cell: &floe_oasis::doc::Cell, (bx, by): (i64, i64), c0: i64, thr: i64, n_levels: u32) -> PageSub {
    let mut cells: FxMap<(i32, i32, u8), f32> = FxMap::default();
    let mut put = |class: u8, (x0, y0, x1, y1): (i64, i64, i64, i64), area: f64| {
        let at = |v: i64, o: i64| (v - o).div_euclid(c0);
        let (i0, i1, j0, j1) = (at(x0, bx), at((x1 - 1).max(x0), bx), at(y0, by), at((y1 - 1).max(y0), by));
        let (w, h) = ((x1 - x0).max(1) as f64, (y1 - y0).max(1) as f64);
        for j in j0..=j1 {
            let oy = if j0 == j1 { 1.0 } else { (y1.min(by + (j + 1) * c0) - y0.max(by + j * c0)).max(0) as f64 / h };
            for i in i0..=i1 {
                let ox = if i0 == i1 { 1.0 } else { (x1.min(bx + (i + 1) * c0) - x0.max(bx + i * c0)).max(0) as f64 / w };
                *cells.entry((i as i32, j as i32, class)).or_insert(0.0) += (area * ox * oy) as f32;
            }
        }
    };
    let mut record = |(x0, y0, x1, y1): (i64, i64, i64, i64), area: f64, rep: &Rep| {
        let Some(class) = class_of(thr, n_levels, (x1 - x0).max(y1 - y0)) else { return };
        let spread = members_of(rep, |dx, dy| put(class, (x0 + dx, y0 + dy, x1 + dx, y1 + dy), area));
        if let Some((n, (fx0, fy0, fx1, fy1))) = spread {
            put(class, (x0 + fx0, y0 + fy0, x1 + fx1, y1 + fy1), n * area);
        }
    };
    for r in &cell.rects {
        record((r.x, r.y, r.x + r.w, r.y + r.h), r.w as f64 * r.h as f64, &r.rep);
    }
    for p in &cell.polys {
        let (Some(x0), Some(x1)) = (p.pts.iter().map(|q| q.0).min(), p.pts.iter().map(|q| q.0).max()) else { continue };
        let (y0, y1) = (p.pts.iter().map(|q| q.1).min().unwrap_or(0), p.pts.iter().map(|q| q.1).max().unwrap_or(0));
        let twice: i128 = p.pts.iter().zip(p.pts.iter().cycle().skip(1)).map(|(a, b)| a.0 as i128 * b.1 as i128 - b.0 as i128 * a.1 as i128).sum();
        record((x0, y0, x1, y1), twice.unsigned_abs() as f64 / 2.0, &p.rep);
    }
    for p in &cell.paths {
        if p.pts.is_empty() {
            continue;
        }
        // the outline's box, the extensions in it (review 2026-10-07: the
        // spine's box grown by the half width left the ends' density out)
        let bbox = floe_tiler::path_bbox(&p.pts, p.hw, p.es, p.ee);
        let len: f64 = p.pts.windows(2).map(|s| (((s[1].0 - s[0].0) as f64).powi(2) + ((s[1].1 - s[0].1) as f64).powi(2)).sqrt()).sum();
        record(bbox, (len + (p.es + p.ee) as f64).max(0.0) * 2.0 * p.hw as f64, &p.rep);
    }
    // in order: the walk sums them as they come (a file the same bytes
    // every build)
    let mut cells: Vec<((i32, i32, u8), f32)> = cells.into_iter().collect();
    cells.sort_unstable_by_key(|&((i, j, class), _)| (class, j, i));
    let mut by_class: Vec<(u8, Vec<(i32, i32, f32)>)> = Vec::new();
    for ((i, j, class), area) in cells {
        match by_class.last_mut() {
            Some((c, list)) if *c == class => list.push((i, j, area)),
            _ => by_class.push((class, vec![(i, j, area)])),
        }
    }
    PageSub { classes: by_class }
}

/// Whether the walk decodes page `pi`: a shape whose larger side reaches
/// the cut (class_of), or no grid
fn decoded_by_walk(ovm: &Ovm, pi: u32, thr: i64) -> bool {
    let pg = ovm.page(pi);
    (pg.max_w.max(pg.max_h) as i64) >= thr || !matches!(ovm.page_occ(pi), Some(PageOcc::Grid(_)))
}

/// How often a long phase of the build says where it is (user 2026-10-07:
/// "no log while ovs indexes - 11 minutes into the real chip and no telling
/// how far it got"): FLOE_OVS_PROGRESS_S seconds (default 10; 0 at every
/// check).
fn progress_every() -> std::time::Duration {
    let s = std::env::var("FLOE_OVS_PROGRESS_S").ok().and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite() && *v >= 0.0);
    std::time::Duration::from_secs_f64(s.unwrap_or(10.0))
}

/// The build's progress lines: each phase as it starts and ends, and
/// within a long one what it has done every progress_every(), each with the
/// seconds since the build began.
pub struct Progress<'a> {
    say: &'a dyn Fn(&str),
    started: std::time::Instant,
    last: std::time::Instant,
    every: std::time::Duration,
}

impl<'a> Progress<'a> {
    pub fn new(say: &'a dyn Fn(&str)) -> Progress<'a> {
        let now = std::time::Instant::now();
        Progress { say, started: now, last: now, every: progress_every() }
    }

    fn line(&mut self, text: &str) {
        (self.say)(&format!("{} ({:.1} s)", text, self.started.elapsed().as_secs_f64()));
        self.last = std::time::Instant::now();
    }

    /// whether a long phase should say where it is
    fn due(&self) -> bool {
        self.last.elapsed() >= self.every
    }
}

/// a count for a progress line: 1.2M, 34k, 567
fn count(n: u64) -> String {
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => format!("{}k", n / 1000),
        1_000_000..=999_999_999 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.1}G", n as f64 / 1e9),
    }
}

/// The pages the walk decodes: those of the cells it walks into - from the
/// top through cells over the cut
fn pages_to_decode(ovm: &Ovm, small: &[bool], thr: i64, progress: &mut Progress) -> Vec<u32> {
    let mut seen = vec![false; ovm.n_cells as usize];
    let mut stack = vec![ovm.top];
    seen[ovm.top as usize] = true;
    let mut out = Vec::new();
    let (mut cells, mut read) = (0u64, 0u64);
    while let Some(ci) = stack.pop() {
        cells += 1;
        if progress.due() {
            progress.line(&format!("listing the pages to decode: {} cells, {} placements read, {} pages", count(cells), count(read), count(out.len() as u64)));
        }
        let (start, count) = ovm.cell_pranges(ci);
        for pri in start..start.saturating_add(count) {
            let pr = ovm.prange(pri);
            out.extend((pr.page_lo..pr.page_lo.saturating_add(pr.page_count)).filter(|&pi| decoded_by_walk(ovm, pi, thr)));
        }
        let (ps, pc) = ovm.cell_places(ci);
        read += pc as u64;
        for i in ps as u64..ps as u64 + pc as u64 {
            let child = ovm.place(i).child;
            if child < ovm.n_cells && !small[child as usize] && !seen[child as usize] {
                seen[child as usize] = true;
                stack.push(child);
            }
        }
    }
    out.sort_unstable_by_key(|&pi| ovm.page(pi).file_off);
    out.dedup();
    out
}

/// `pages` decoded on `jobs` threads, each with its own handle of
/// design.ovp, taking the next page as it is done - the largest first (the
/// top cell's own pages of a million records each would hold one thread
/// while the rest stood idle: the synthetic MAIN01 1/10 decoded 55 s on 8
/// threads by runs in file order)
fn decode_pages(ovm: &Ovm, ovp: &str, pages: &[u32], g: &OvsGrid, thr: i64, jobs: usize, progress: &mut Progress) -> Result<Vec<(u32, (PageSub, u64))>, String> {
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    let mut order: Vec<u32> = pages.to_vec();
    order.sort_unstable_by_key(|&pi| std::cmp::Reverse(ovm.page(pi).csize));
    let total_bytes: u64 = order.iter().map(|&pi| u64::from(ovm.page(pi).csize)).sum();
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let (done, done_bytes) = (AtomicUsize::new(0), AtomicU64::new(0));
    let jobs = jobs.clamp(1, order.len().max(1));
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..jobs)
            .map(|_| {
                let (order, next, failed, done, done_bytes) = (&order, &next, &failed, &done, &done_bytes);
                scope.spawn(move || -> Result<Vec<(u32, (PageSub, u64))>, String> {
                    let mut f = std::fs::File::open(ovp).map_err(|e| format!("{}: {}", ovp, e))?;
                    let mut out = Vec::new();
                    while !failed.load(Ordering::Relaxed) {
                        let at = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&pi) = order.get(at) else { break };
                        match decode_page(ovm, &mut f, pi, g, thr) {
                            Ok(made) => {
                                done.fetch_add(1, Ordering::Relaxed);
                                done_bytes.fetch_add(made.1, Ordering::Relaxed);
                                out.push((pi, made));
                            }
                            Err(e) => {
                                failed.store(true, Ordering::Relaxed);
                                return Err(e);
                            }
                        }
                    }
                    Ok(out)
                })
            })
            .collect();
        // where the threads are, while they work
        while !handles.iter().all(|handle| handle.is_finished()) {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if progress.due() {
                progress.line(&format!(
                    "decode: {}/{} pages, {:.0}/{:.0} MB on {} threads",
                    count(done.load(Ordering::Relaxed) as u64),
                    count(order.len() as u64),
                    done_bytes.load(Ordering::Relaxed) as f64 / 1e6,
                    total_bytes as f64 / 1e6,
                    jobs
                ));
            }
        }
        let mut out = Vec::with_capacity(order.len());
        let mut first_error = None;
        for handle in handles {
            match handle.join().map_err(|_| "a decode thread panicked".to_string()).and_then(|r| r) {
                Ok(made) => out.extend(made),
                Err(e) => first_error = first_error.or(Some(e)),
            }
        }
        first_error.map_or(Ok(out), Err)
    })
}

/// One file's share of the walk (an owner): the top's own content, or a big
/// cell's under the top (its first placement's) - on a part of the top's
/// grid aligned to its groups, its depths counted from the cell (user
/// 2026-10-07: two cells under the top drew their root views by the plans;
/// "record the cells over 25 % as the top unfolds and make them at once")
struct Owner {
    ci: u32,
    /// the top grid's cell of its corner (multiples of OVS_GROUP), its cells
    i0: u32,
    j0: u32,
    w: u32,
    h: u32,
    fast: FastCells,
    words: usize,
    gw: u32,
    gh: u32,
    tiles_w: u32,
    /// the cell's placement depth under the top (0: the top)
    depth0: u32,
    /// the cell's coordinates to the top's (its first placement)
    xf: Xf,
}

impl Owner {
    fn new(ci: u32, g: &OvsGrid, (i0, j0, w, h): (u32, u32, u32, u32), depth0: u32, xf: Xf) -> Owner {
        let fast = FastCells::new(&Owner::part(g, (i0, j0, w, h)));
        Owner { ci, i0, j0, w, h, fast, words: words_of(w), gw: w.div_ceil(OVS_GROUP), gh: h.div_ceil(OVS_GROUP), tiles_w: w.div_ceil(TILE), depth0, xf }
    }

    /// cells [i0, i0 + w) x [j0, j0 + h) of `g` as a grid, its own levels
    fn part(g: &OvsGrid, (i0, j0, w, h): (u32, u32, u32, u32)) -> OvsGrid {
        let (x0, y0) = (g.x0 + i64::from(i0) * g.cell_dbu, g.y0 + i64::from(j0) * g.cell_dbu);
        OvsGrid { unit: g.unit, cell_dbu: g.cell_dbu, x0, y0, w, h, n_levels: level_count(w, h) }
    }

    /// its own file's grid
    fn grid(&self, g: &OvsGrid) -> OvsGrid {
        Owner::part(g, (self.i0, self.j0, self.w, self.h))
    }
}

struct Builder<'a> {
    ovm: &'a Ovm,
    cover: &'a CellCover,
    g: OvsGrid,
    thr: i64,
    small: Vec<bool>,
    /// the top's own (0) and each big cell's under it, as met
    owners: Vec<Owner>,
    owner: usize,
    /// the big cells under the top, true until their first placement is met
    roots: FxMap<u32, bool>,
    smalls: FxMap<(u16, u32, u8), SmallAcc>,
    planes: FxMap<(u16, u32, u8, u8), PlaneAcc>,
    /// design.ovp, for the pages decoded, and their shapes once decoded
    ovp: std::fs::File,
    subs: FxMap<u32, Rc<PageSub>>,
    /// the first page that would not read or decode (the build fails)
    error: Option<String>,
    stats: OvsStats,
    progress: Progress<'a>,
    /// where the walk is: (placement, of) at the top, and in the cell it
    /// walks under it
    at: [(u64, u64); 2],
    at_cell: u32,
    ticks: u64,
}

impl Builder<'_> {
    fn plane(&mut self, owner: usize, k: u32, d: u8, class: u8) -> &mut PlaneAcc {
        let o = &self.owners[owner];
        let (n_bits, n_groups) = (o.words * o.h as usize, (o.gw * o.gh) as usize);
        self.planes.entry((owner as u16, k, d, class)).or_insert_with(|| PlaneAcc { bits: vec![0; n_bits], area: vec![0.0; n_groups] })
    }

    /// `area` over the cells a world box covers, an even share each - the
    /// owner's
    fn add_rect(&mut self, k: u32, d: u8, class: u8, rect: (i64, i64, i64, i64), area: f64) {
        let owner = self.owner;
        let (fast, words, gw) = (self.owners[owner].fast, self.owners[owner].words, self.owners[owner].gw);
        let cells = fast.cells(rect);
        let (i0, i1, j0, j1) = cells;
        let n = f64::from(i1 - i0 + 1) * f64::from(j1 - j0 + 1);
        let plane = self.plane(owner, k, d, class);
        set_cells(&mut plane.bits, words, cells);
        groups_of(gw, cells, |g, c| plane.area[g as usize] += (area * f64::from(c) / n) as f32);
    }

    /// A placement of `child` from the top under `xf`: the owner it starts
    /// - a big cell's first - else None (the top's)
    fn root_owner(&mut self, child: u32, xf: &Xf) -> Option<usize> {
        if self.roots.get(&child) != Some(&true) {
            return None;
        }
        self.roots.insert(child, false);
        let rb = self.ovm.cell_rbbox(child);
        let (i0, i1, j0, j1) = self.g.cells(world_rect(xf, (rb.x0, rb.y0, rb.x1, rb.y1)));
        let (i0, j0) = (i0 / OVS_GROUP * OVS_GROUP, j0 / OVS_GROUP * OVS_GROUP);
        self.owners.push(Owner::new(child, &self.g, (i0, j0, i1 - i0 + 1, j1 - j0 + 1), 1, *xf));
        Some(self.owners.len() - 1)
    }

    /// Page `pi`'s shapes by class (PageSub): decoded before the walk
    /// (decode_pages), else here; None where it will not read or decode -
    /// its error kept, the build fails
    fn page_sub(&mut self, pi: u32) -> Option<Rc<PageSub>> {
        if let Some(known) = self.subs.get(&pi) {
            return Some(Rc::clone(known));
        }
        if self.error.is_some() {
            return None;
        }
        let started = std::time::Instant::now();
        let made = decode_page(self.ovm, &mut self.ovp, pi, &self.g, self.thr);
        self.stats.decode_s += started.elapsed().as_secs_f64();
        match made {
            Ok((sub, bytes)) => {
                self.stats.decoded += 1;
                self.stats.decoded_bytes += bytes;
                let sub = Rc::new(sub);
                self.subs.insert(pi, Rc::clone(&sub));
                Some(sub)
            }
            Err(e) => {
                self.error = Some(e);
                None
            }
        }
    }

    /// every few thousand placements or members, what the walk has done -
    /// when progress_every() has passed
    fn tick(&mut self, n: u64) {
        self.ticks += n;
        if self.ticks < 4096 {
            return;
        }
        self.ticks = 0;
        if !self.progress.due() {
            return;
        }
        let (top, sub) = (self.at[0], self.at[1]);
        let under = if sub.1 > 0 { format!(", in {} {}/{}", self.ovm.cell(self.at_cell).name, count(sub.0), count(sub.1)) } else { String::new() };
        let line = format!(
            "walk: top placement {}/{}{}; {} cells walked into, {} members under the cut, {} pages by grid, {} decoded",
            count(top.0),
            count(top.1),
            under,
            count(self.stats.walked),
            count(self.stats.small),
            count(self.stats.pages),
            count(self.stats.big_pages)
        );
        self.progress.line(&line);
    }

    fn walk(&mut self, ci: u32, xf: &Xf, depth: u32) {
        let ovm = self.ovm;
        let owner = self.owner;
        let depth0 = self.owners[owner].depth0;
        let d = (depth - depth0).min(DEPTH_CAP as u32) as u8;
        // its own pages: by their occupancy grids, or decoded
        let (start, count) = ovm.cell_pranges(ci);
        for pri in start..start.saturating_add(count) {
            let pr = ovm.prange(pri);
            let k = pr.layer_idx;
            for pi in pr.page_lo..pr.page_lo.saturating_add(pr.page_count) {
                let pg = ovm.page(pi);
                let occ = if decoded_by_walk(ovm, pi, self.thr) { None } else { ovm.page_occ(pi) };
                if let Some(PageOcc::Grid(levels)) = occ {
                    self.stats.pages += 1;
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
                            self.add_rect(k, d, 0, world_rect(xf, (xa, ya, xb, yb)), area);
                        }
                    }
                    continue;
                }
                self.stats.big_pages += 1;
                let Some(sub) = self.page_sub(pi) else { continue };
                let (c0, bx, by) = (self.g.cell_dbu, pg.bbox.x0, pg.bbox.y0);
                for (class, cells) in &sub.classes {
                    for &(i, j, area) in cells {
                        let (wx, wy) = xf.apply(bx + i64::from(i) * c0 + c0 / 2, by + i64::from(j) * c0 + c0 / 2);
                        self.add_rect(k, d, *class, (wx, wy, wx + 1, wy + 1), f64::from(area));
                    }
                }
            }
        }
        // its placements: a cell under the cut counted where its members
        // are, a larger one walked
        let (ps, pc) = ovm.cell_places(ci);
        if depth == 1 {
            self.at_cell = ci;
        }
        for i in ps as u64..ps as u64 + pc as u64 {
            if depth < 2 {
                self.at[depth as usize] = (i - ps as u64 + 1, pc as u64);
                if depth == 0 {
                    self.at[1] = (0, 0);
                }
            }
            self.tick(1);
            let pl = ovm.place(i);
            let child = pl.child;
            if child >= ovm.n_cells {
                continue;
            }
            let at = |dx: i64, dy: i64| xf.compose(&Xf::place(pl.x + dx, pl.y + dy, pl.rot, pl.flip));
            if self.small[child as usize] {
                // member 0's world box, the others shifted by their offsets
                // in world terms
                let rb = ovm.cell_rbbox(child);
                let base = world_rect(&at(0, 0), (rb.x0, rb.y0, rb.x1, rb.y1));
                let shift = |dx: i64, dy: i64| {
                    let (ox, oy) = xf.apply_vec(dx, dy);
                    (base.0 + ox, base.1 + oy, base.2 + ox, base.3 + oy)
                };
                let cd = (depth + 1 - depth0).min(DEPTH_CAP as u32) as u8;
                let (fc, gw, tiles_w) = (self.owners[owner].fast, self.owners[owner].gw, self.owners[owner].tiles_w);
                let acc = self.smalls.entry((owner as u16, child, cd)).or_default();
                let mark = |acc: &mut SmallAcc, rect: (i64, i64, i64, i64)| {
                    let cells = fc.cells(rect);
                    acc.tiles.set(tiles_w, cells);
                    let (ci, cj) = ((cells.0 + cells.1) / 2, (cells.2 + cells.3) / 2);
                    acc.count((cj / OVS_GROUP) * gw + ci / OVS_GROUP, 1.0);
                };
                match &pl.rep {
                    Rep::One => {
                        mark(acc, base);
                        self.stats.small += 1;
                    }
                    Rep::Grid { na, nb, va, vb } => {
                        let n = na.saturating_mul(*nb);
                        self.stats.small += n;
                        if n > SPREAD_MEMBERS {
                            // the members over their footprint: every cell
                            // of it, the members by its groups' shares
                            let (la, lb) = (*na as i64 - 1, *nb as i64 - 1);
                            let fp = [(0, 0), (la * va.0, la * va.1), (lb * vb.0, lb * vb.1), (la * va.0 + lb * vb.0, la * va.1 + lb * vb.1)]
                                .iter()
                                .map(|&(dx, dy)| shift(dx, dy))
                                .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
                                .unwrap_or(base);
                            let cells = fc.cells(fp);
                            acc.tiles.set(tiles_w, cells);
                            let all = f64::from(cells.1 - cells.0 + 1) * f64::from(cells.3 - cells.2 + 1);
                            groups_of(gw, cells, |gi, c| acc.count(gi, (n as f64 * f64::from(c) / all) as f32));
                            self.stats.spread += 1;
                        } else {
                            for jb in 0..*nb as i64 {
                                for ia in 0..*na as i64 {
                                    mark(acc, shift(ia * va.0 + jb * vb.0, ia * va.1 + jb * vb.1));
                                }
                            }
                        }
                    }
                    Rep::Pts(pts) => {
                        self.stats.small += pts.len() as u64;
                        for &(dx, dy) in pts.iter() {
                            mark(acc, shift(dx, dy));
                        }
                    }
                }
                let members = pl.rep.members().min(SPREAD_MEMBERS);
                self.tick(members);
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
                    self.stats.walked += 1;
                    let placed = at(dx, dy);
                    // a big cell's first placement under the top: its own
                    // file's share
                    let next = if depth == 0 { self.root_owner(child, &placed) } else { None };
                    if let Some(next) = next {
                        self.owner = next;
                    }
                    self.walk(child, &placed, depth + 1);
                    self.owner = owner;
                }
            }
        }
    }

    /// The cells under the cut: each (cell, depth)'s marks and counts into
    /// the planes of its cover's layers and relative depths
    fn settle(&mut self) {
        // in order: the planes sum them as they come (a file the same bytes
        // every build)
        let mut smalls: Vec<((u16, u32, u8), SmallAcc)> = std::mem::take(&mut self.smalls).into_iter().collect();
        smalls.sort_unstable_by_key(|&(key, _)| key);
        smalls.iter_mut().for_each(|(_, acc)| acc.flush());
        self.stats.small_keys = smalls.len() as u64;
        self.stats.small_tiles = smalls.iter().map(|(_, acc)| acc.tiles.tiles.len() as u64).sum();
        let mut slices: HashMap<u32, Rc<Vec<Vec<(u32, f64)>>>> = HashMap::new();
        let n = smalls.len();
        for (at, ((owner, child, cd), acc)) in smalls.into_iter().enumerate() {
            let owner = owner as usize;
            let (words, h, tiles_w) = (self.owners[owner].words, self.owners[owner].h, self.owners[owner].tiles_w);
            if self.progress.due() {
                self.progress.line(&format!("settle: {}/{} cells under the cut", count(at as u64), count(n as u64)));
            }
            let sl = Rc::clone(slices.entry(child).or_insert_with(|| Rc::new(depth_slices(self.ovm, self.cover, child))));
            for (j, layers) in sl.iter().enumerate() {
                let d = (cd as usize + j).min(DEPTH_CAP as usize) as u8;
                for &(k, area) in layers {
                    let plane = self.plane(owner, k, d, 0);
                    acc.tiles.or_into(tiles_w, &mut plane.bits, words, h);
                    for (&g, &n) in &acc.groups {
                        plane.area[g as usize] += (f64::from(n) * area) as f32;
                    }
                }
            }
        }
    }
}

/// The levels of one plane on `g` from its classes at level 0 (bits, area):
/// each level's (bits stored, means stored) - the classes up to the level
/// OR'd and summed, pooled a level at a time; and the bytes stored.
fn plane_levels(g: &OvsGrid, mut cls: Vec<(u8, Vec<u64>, Vec<f32>)>, stats: &mut OvsStats) -> (Vec<(Vec<u8>, Vec<u8>)>, u64) {
    let (mut w, mut h) = (g.w, g.h);
    let (mut gw, mut gh) = (w.div_ceil(OVS_GROUP), h.div_ceil(OVS_GROUP));
    let mut levels = Vec::with_capacity(g.n_levels as usize);
    let mut stored = 0u64;
    for lv in 0..g.n_levels {
        if lv > 0 {
            for (_, bits, area) in cls.iter_mut() {
                *bits = pool_bits(bits, w, h);
                *area = pool_area(area, gw, gh);
            }
            (w, h) = g.level_dims(lv);
            (gw, gh) = (w.div_ceil(OVS_GROUP), h.div_ceil(OVS_GROUP));
        }
        let words = words_of(w);
        let mut bits = vec![0u64; words * h as usize];
        let mut area = vec![0f32; (gw * gh) as usize];
        for (c, cb, ca) in &cls {
            if u32::from(*c) <= lv {
                bits.iter_mut().zip(cb).for_each(|(a, b)| *a |= b);
                area.iter_mut().zip(ca).for_each(|(a, b)| *a += b);
            }
        }
        let cell = g.cell_dbu as f64 * f64::from(1u32 << lv);
        let mut means = vec![0u8; (gw * gh) as usize];
        let mut any = false;
        for gj in 0..gh {
            for gi in 0..gw {
                let a = f64::from(area[(gj * gw + gi) as usize]);
                if !(a > 0.0) {
                    continue;
                }
                let occ = group_occ(&bits, words, h, gi, gj);
                if occ == 0 {
                    continue;
                }
                let mean = (a / (f64::from(occ) * cell * cell)).clamp(0.0, 1.0);
                means[(gj * gw + gi) as usize] = ((mean * 255.0).round() as u8).max(1);
                any = true;
            }
        }
        stats.levels += 1;
        if !bits.iter().any(|&word| word != 0) {
            stats.empty_levels += 1;
            levels.push((Vec::new(), Vec::new()));
            continue;
        }
        let level = (deflate(&row_bytes(&bits, words, w, h)), if any { deflate(&means) } else { Vec::new() });
        stored += (level.0.len() + level.1.len()) as u64;
        levels.push(level);
    }
    (levels, stored)
}

/// ORs an owner's level-0 bits (`w` x `h`, `src_words` a row) into the top's
/// (`dst_words` a row) at its cell (i0, j0)
fn or_shifted(dst: &mut [u64], dst_words: usize, src: &[u64], src_words: usize, h: u32, (i0, j0): (u32, u32)) {
    let (word0, shift) = ((i0 / 64) as usize, i0 % 64);
    for r in 0..h as usize {
        let row = (j0 as usize + r) * dst_words;
        for w in 0..src_words {
            let v = src[r * src_words + w];
            if v == 0 {
                continue;
            }
            dst[row + word0 + w] |= v << shift;
            if shift > 0 && word0 + w + 1 < dst_words {
                dst[row + word0 + w + 1] |= v >> (64 - shift);
            }
        }
    }
}

/// What a build made: the top's design.ovs, each big cell's (its cell, its
/// name, its bytes - design.ovs.<cell>), and what the build did.
pub struct Built {
    pub top: Vec<u8>,
    pub roots: Vec<(u32, String, Vec<u8>)>,
    pub stats: OvsStats,
}

/// design.ovs for an index (`ovm` with design.ovb attached, its pages at
/// `ovp`) and its cells' cover, on a grid of `base_um` cells (None: auto),
/// and of each cell under the top whose box is `root_share` of the top's or
/// more (0: none) one of its own, in the same walk. `jobs` threads decode
/// the pages the walk needs before it (0: as many as the machine has).
/// `say` gets the progress lines (Progress).
#[allow(clippy::too_many_arguments)]
pub fn build(ovm: &Ovm, cover: &CellCover, ovp: &str, base_um: Option<f64>, jobs: usize, root_share: f64, say: &dyn Fn(&str)) -> Result<Built, String> {
    let mut progress = Progress::new(say);
    if !ovm.has_page_occ() {
        return Err("design.ovs needs design.ovb (the pages' occupancy grids)".into());
    }
    if ovm.top >= ovm.n_cells {
        return Err("the index has no top cell".into());
    }
    let top = ovm.cell_rbbox(ovm.top);
    if top.is_empty() {
        return Err("the top cell is empty".into());
    }
    let base = base_um.unwrap_or_else(|| OvsGrid::auto_base_um(ovm.unit, (top.x1 - top.x0).max(top.y1 - top.y0)));
    let g = OvsGrid::new(ovm.unit, base, (top.x0, top.y0, top.x1, top.y1)).ok_or_else(|| format!("no grid of {} um over the top cell", base))?;
    let thr = OVS_SUB_CUT_CELLS * g.cell_dbu;
    let small: Vec<bool> = (0..ovm.n_cells)
        .map(|ci| {
            let b = ovm.cell_rbbox(ci);
            !b.is_empty() && b.x1 - b.x0 < thr && b.y1 - b.y0 < thr
        })
        .collect();
    // the big cells under the top: their boxes `root_share` of its or more
    let area = |b: &floe_ovm::BBox| (b.x1 - b.x0) as f64 * (b.y1 - b.y0) as f64;
    let mut roots: FxMap<u32, bool> = FxMap::default();
    if root_share > 0.0 {
        let (ps, pc) = ovm.cell_places(ovm.top);
        for i in ps as u64..ps as u64 + pc as u64 {
            let child = ovm.place(i).child;
            if child < ovm.n_cells && !small[child as usize] {
                let b = ovm.cell_rbbox(child);
                if !b.is_empty() && area(&b) >= root_share * area(&top) {
                    roots.insert(child, true);
                }
            }
        }
    }
    progress.line(&format!(
        "grid {} um, {}x{} cells, {} levels; {} cells ({} under the cut), {} pages, {} layers; {} cells under the top of {} % its box or more",
        g.base_um(),
        g.w,
        g.h,
        g.n_levels,
        count(u64::from(ovm.n_cells)),
        count(small.iter().filter(|&&s| s).count() as u64),
        count(u64::from(ovm.n_pages)),
        ovm.n_layers,
        roots.len(),
        (root_share * 100.0).round()
    ));
    let ovp_path = ovp;
    let ovp = std::fs::File::open(ovp).map_err(|e| format!("{}: {}", ovp, e))?;
    let mut b = Builder {
        ovm,
        cover,
        g,
        thr,
        small,
        owners: vec![Owner::new(ovm.top, &g, (0, 0, g.w, g.h), 0, Xf::identity())],
        owner: 0,
        roots,
        smalls: FxMap::default(),
        planes: FxMap::default(),
        ovp,
        subs: FxMap::default(),
        error: None,
        stats: OvsStats::default(),
        progress,
        at: [(0, 0); 2],
        at_cell: 0,
        ticks: 0,
    };
    if b.small[ovm.top as usize] {
        // a top under the cut (review 2026-10-07: a 0.4 um square alone made
        // a file without a plane): its box and its cover, at depth 0
        let (fast, gw, tiles_w) = (b.owners[0].fast, b.owners[0].gw, b.owners[0].tiles_w);
        let cells = fast.cells((top.x0, top.y0, top.x1, top.y1));
        let acc = b.smalls.entry((0, ovm.top, 0)).or_default();
        acc.tiles.set(tiles_w, cells);
        acc.count(((cells.2 + cells.3) / 2 / OVS_GROUP) * gw + (cells.0 + cells.1) / 2 / OVS_GROUP, 1.0);
        b.stats.small += 1;
    } else {
        let started = std::time::Instant::now();
        let jobs = if jobs == 0 { std::thread::available_parallelism().map_or(4, |n| n.get()) } else { jobs };
        b.progress.line("listing the pages to decode (a shape over the cut, or no grid)");
        let pages = pages_to_decode(ovm, &b.small, thr, &mut b.progress);
        // from 0.0: an empty f64 sum is -0.0 ("-0 MB")
        let mb = pages.iter().fold(0.0, |mb, &pi| mb + f64::from(ovm.page(pi).csize) / 1e6);
        b.progress.line(&format!("decode: {} pages, {:.0} MB on {} threads", count(pages.len() as u64), mb, jobs));
        for (pi, (sub, bytes)) in decode_pages(ovm, ovp_path, &pages, &g, thr, jobs, &mut b.progress)? {
            b.stats.decoded += 1;
            b.stats.decoded_bytes += bytes;
            b.subs.insert(pi, Rc::new(sub));
        }
        b.stats.decode_s = started.elapsed().as_secs_f64();
        b.stats.decode_jobs = jobs as u64;
        b.progress.line(&format!("decoded {} pages in {:.1} s", count(b.stats.decoded), b.stats.decode_s));
        let started = std::time::Instant::now();
        b.progress.line(&format!("walk: {} placements at the top", count(u64::from(ovm.cell_places(ovm.top).1))));
        b.walk(ovm.top, &Xf::identity(), 0);
        b.stats.walk_s = started.elapsed().as_secs_f64();
        let line = format!(
            "walked in {:.1} s: {} cells walked into, {} members under the cut ({} arrays spread), {} pages by grid, {} decoded",
            b.stats.walk_s,
            count(b.stats.walked),
            count(b.stats.small),
            count(b.stats.spread),
            count(b.stats.pages),
            count(b.stats.big_pages)
        );
        b.progress.line(&line);
        if let Some(e) = b.error.take() {
            return Err(e);
        }
    }
    b.subs.clear();
    let started = std::time::Instant::now();
    let line = format!("settle: {} cells under the cut by their cover", count(b.smalls.len() as u64));
    b.progress.line(&line);
    b.settle();
    b.stats.settle_s = started.elapsed().as_secs_f64();
    let write_started = std::time::Instant::now();
    b.stats.grids = b.planes.len() as u64;
    b.stats.grid_bytes = b.planes.values().map(|p| (p.bits.len() * 8 + p.area.len() * 4) as u64).sum();
    // the planes by owner and layer: (depth, class) each
    let n_layers = ovm.n_layers as usize;
    let mut by_owner: Vec<Vec<Vec<(u8, u8)>>> = vec![vec![Vec::new(); n_layers]; b.owners.len()];
    for &(o, k, d, c) in b.planes.keys() {
        if let Some(list) = by_owner[o as usize].get_mut(k as usize) {
            list.push((d, c));
        }
    }
    by_owner.iter_mut().flatten().for_each(|list| list.sort_unstable());
    let line = format!(
        "settled in {:.1} s; write: {} planes ({:.0} MB) of {} layers, {} cells of their own",
        b.stats.settle_s,
        count(b.stats.grids),
        b.stats.grid_bytes as f64 / 1e6,
        by_owner.iter().map(|layers| layers.iter().filter(|list| !list.is_empty()).count()).max().unwrap_or(0),
        b.owners.len() - 1
    );
    b.progress.line(&line);
    let mut stats = std::mem::take(&mut b.stats);
    // each big cell's file first, its planes kept for the top's
    let mut roots = Vec::with_capacity(b.owners.len() - 1);
    for o in 1..b.owners.len() {
        let og = b.owners[o].grid(&g);
        let mut blobs: Blobs = Vec::with_capacity(n_layers);
        for (k, list) in by_owner[o].iter().enumerate() {
            let mut planes_out = Vec::new();
            let mut at = 0;
            while at < list.len() {
                let d = list[at].0;
                let cls: Vec<(u8, Vec<u64>, Vec<f32>)> = list[at..]
                    .iter()
                    .take_while(|&&(pd, _)| pd == d)
                    .filter_map(|&(_, c)| b.planes.get(&(o as u16, k as u32, d, c)).map(|p| (c, p.bits.clone(), p.area.clone())))
                    .collect();
                at += list[at..].iter().take_while(|&&(pd, _)| pd == d).count();
                planes_out.push((d, plane_levels(&og, cls, &mut stats).0));
            }
            blobs.push(planes_out);
        }
        let (x, y, rot, flip) = b.owners[o].xf.decompose();
        let root = OvsRoot { ci: b.owners[o].ci, x, y, rot, flip };
        let bytes = encode_file(ovm.src_size, ovm.src_mtime, &og, Some(&root), &blobs);
        let name = ovm.cell(b.owners[o].ci).name;
        b.progress.line(&format!("cell {} (#{}): {}x{} cells, {:.1} MB", name, b.owners[o].ci, og.w, og.h, bytes.len() as f64 / 1e6));
        roots.push((b.owners[o].ci, name, bytes));
    }
    // the top's: its own planes with every owner's at its place, its depths
    // one deeper
    let mut written = 0u64;
    let mut blobs: Blobs = Vec::with_capacity(n_layers);
    for k in 0..n_layers {
        if b.progress.due() {
            b.progress.line(&format!("write: layer {}/{}, {:.1} MB so far", k, n_layers, written as f64 / 1e6));
        }
        // (top depth, class, owner, owner depth) of every plane of the layer
        let mut parts: Vec<(u8, u8, usize, u8)> = Vec::new();
        for (o, owner) in b.owners.iter().enumerate() {
            for &(d, c) in &by_owner[o][k] {
                let td = (u32::from(d) + owner.depth0).min(DEPTH_CAP as u32) as u8;
                parts.push((td, c, o, d));
            }
        }
        parts.sort_unstable();
        let mut planes_out = Vec::new();
        let mut at = 0;
        while at < parts.len() {
            let td = parts[at].0;
            let mut cls: Vec<(u8, Vec<u64>, Vec<f32>)> = Vec::new();
            while at < parts.len() && parts[at].0 == td {
                let (_, c, o, d) = parts[at];
                at += 1;
                let Some(p) = b.planes.remove(&(o as u16, k as u32, d, c)) else { continue };
                let (top_words, top_gw) = (b.owners[0].words, b.owners[0].gw);
                if cls.last().map(|cl| cl.0) != Some(c) {
                    cls.push((c, vec![0u64; top_words * g.h as usize], vec![0f32; (top_gw * b.owners[0].gh) as usize]));
                }
                let (_, bits, area) = cls.last_mut().expect("a class");
                if o == 0 {
                    bits.iter_mut().zip(&p.bits).for_each(|(a, b)| *a |= b);
                    area.iter_mut().zip(&p.area).for_each(|(a, b)| *a += b);
                } else {
                    let owner = &b.owners[o];
                    or_shifted(bits, top_words, &p.bits, owner.words, owner.h, (owner.i0, owner.j0));
                    let (gi0, gj0) = (owner.i0 / OVS_GROUP, owner.j0 / OVS_GROUP);
                    for gj in 0..owner.gh {
                        for gi in 0..owner.gw {
                            area[((gj0 + gj) * top_gw + gi0 + gi) as usize] += p.area[(gj * owner.gw + gi) as usize];
                        }
                    }
                }
            }
            let (levels, stored) = plane_levels(&g, cls, &mut stats);
            written += stored;
            planes_out.push((td, levels));
        }
        blobs.push(planes_out);
    }
    let out = encode(ovm.src_size, ovm.src_mtime, &g, &blobs);
    stats.write_s = write_started.elapsed().as_secs_f64();
    stats.bytes = out.len() as u64;
    stats.roots = roots.len() as u64;
    let line = format!("written in {:.1} s: {:.1} MB, {} cells of their own", stats.write_s, stats.bytes as f64 / 1e6, roots.len());
    b.progress.line(&line);
    Ok(Built { top: out, roots, stats })
}

/// Per layer, per plane, its (depth, per level (the bits stored, the means
/// stored): each empty for all zero, else deflated).
pub type Blobs = Vec<Vec<(u8, Vec<(Vec<u8>, Vec<u8>)>)>>;

/// A big cell's file's cell and its placement in the top (the cell's
/// coordinates to the top's: Xf::place(x, y, rot, flip))
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OvsRoot {
    pub ci: u32,
    pub x: i64,
    pub y: i64,
    pub rot: u8,
    pub flip: bool,
}

/// design.ovs's bytes: header, table, body (the module's format; build
/// writes it, tests make small ones)
pub fn encode(src_size: u64, src_mtime: u64, g: &OvsGrid, blobs: &Blobs) -> Vec<u8> {
    encode_file(src_size, src_mtime, g, None, blobs)
}

/// A file's bytes: the top's (version 3) or, with `root`, a big cell's
/// (version 4: the root block after the header)
pub fn encode_file(src_size: u64, src_mtime: u64, g: &OvsGrid, root: Option<&OvsRoot>, blobs: &Blobs) -> Vec<u8> {
    let head = HEADER_LEN + if root.is_some() { ROOT_LEN } else { 0 };
    let table_len: usize = blobs.iter().map(|planes| 1 + planes.iter().map(|(_, levels)| 1 + levels.len() * 32).sum::<usize>()).sum();
    let mut out = Vec::with_capacity(head + table_len);
    out.extend_from_slice(OVS_MAGIC);
    out.extend_from_slice(&(if root.is_some() { OVS_VERSION_ROOT } else { OVS_VERSION }).to_le_bytes());
    out.extend_from_slice(&OVS_GROUP.to_le_bytes());
    out.extend_from_slice(&src_size.to_le_bytes());
    out.extend_from_slice(&src_mtime.to_le_bytes());
    out.extend_from_slice(&g.unit.to_le_bytes());
    out.extend_from_slice(&g.cell_dbu.to_le_bytes());
    out.extend_from_slice(&g.x0.to_le_bytes());
    out.extend_from_slice(&g.y0.to_le_bytes());
    out.extend_from_slice(&g.w.to_le_bytes());
    out.extend_from_slice(&g.h.to_le_bytes());
    out.extend_from_slice(&g.n_levels.to_le_bytes());
    out.extend_from_slice(&(blobs.len() as u32).to_le_bytes());
    debug_assert_eq!(out.len(), HEADER_LEN);
    if let Some(r) = root {
        out.extend_from_slice(&r.ci.to_le_bytes());
        out.extend_from_slice(&[r.rot, u8::from(r.flip), 0, 0]);
        out.extend_from_slice(&r.x.to_le_bytes());
        out.extend_from_slice(&r.y.to_le_bytes());
    }
    let body_start = (head + table_len) as u64;
    let mut body: Vec<u8> = Vec::new();
    for planes in blobs {
        out.push(planes.len() as u8);
        for (depth, levels) in planes {
            out.push(*depth);
            for (bits, means) in levels {
                for data in [bits, means] {
                    out.extend_from_slice(&(body_start + body.len() as u64).to_le_bytes());
                    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
                    body.extend_from_slice(data);
                }
            }
        }
    }
    debug_assert_eq!(out.len() as u64, body_start);
    out.extend_from_slice(&body);
    out
}

/// One plane in the file: its depth and per level (bits off, len, means
/// off, len).
#[derive(Clone, Debug)]
pub struct OvsPlane {
    pub depth: u8,
    pub levels: Vec<[u64; 4]>,
}

/// A read design.ovs (structure checked; `validate_against` checks it
/// belongs to an index).
pub struct OvsFile {
    data: Vec<u8>,
    pub group: u32,
    pub src_size: u64,
    pub src_mtime: u64,
    pub grid: OvsGrid,
    /// a big cell's file: its cell and placement (None: the top's)
    pub root: Option<OvsRoot>,
    pub layers: Vec<Vec<OvsPlane>>,
}

impl std::fmt::Debug for OvsFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OvsFile(grid={:?} layers={} bytes={})", self.grid, self.layers.len(), self.data.len())
    }
}

fn g32(b: &[u8], o: usize) -> Result<u32, String> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).ok_or_else(|| "design.ovs: truncated".to_string())
}
fn g64(b: &[u8], o: usize) -> Result<u64, String> {
    b.get(o..o + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap())).ok_or_else(|| "design.ovs: truncated".to_string())
}

fn inflate(stored: &[u8], n: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(n);
    flate2::read::DeflateDecoder::new(stored).read_to_end(&mut out).ok()?;
    (out.len() == n).then_some(out)
}

impl OvsFile {
    pub fn open(path: &str) -> Result<OvsFile, String> {
        let data = std::fs::read(path).map_err(|e| format!("{}: {}", path, e))?;
        OvsFile::from_bytes(data)
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<OvsFile, String> {
        if data.len() < HEADER_LEN || &data[..8] != OVS_MAGIC {
            return Err("design.ovs: not an occupancy density file".into());
        }
        let version = g32(&data, 8)?;
        if version != OVS_VERSION && version != OVS_VERSION_ROOT {
            return Err(format!("design.ovs: version {} (this build reads {} and {}: floe-index ovs builds it again)", version, OVS_VERSION, OVS_VERSION_ROOT));
        }
        let group = g32(&data, 12)?;
        let (src_size, src_mtime) = (g64(&data, 16)?, g64(&data, 24)?);
        let grid = OvsGrid {
            unit: f64::from_bits(g64(&data, 32)?),
            cell_dbu: g64(&data, 40)? as i64,
            x0: g64(&data, 48)? as i64,
            y0: g64(&data, 56)? as i64,
            w: g32(&data, 64)?,
            h: g32(&data, 68)?,
            n_levels: g32(&data, 72)?,
        };
        if grid.cell_dbu <= 0 || grid.w == 0 || grid.h == 0 || grid.n_levels == 0 || grid.n_levels > 32 {
            return Err("design.ovs: no grid".into());
        }
        let n_layers = g32(&data, 76)?;
        let mut at = HEADER_LEN;
        let root = if version == OVS_VERSION_ROOT {
            let flags = data.get(at + 4..at + 6).ok_or("design.ovs: truncated")?;
            let root = OvsRoot { ci: g32(&data, at)?, rot: flags[0] & 3, flip: flags[1] != 0, x: g64(&data, at + 8)? as i64, y: g64(&data, at + 16)? as i64 };
            at += ROOT_LEN;
            Some(root)
        } else {
            None
        };
        let mut layers = Vec::with_capacity(n_layers as usize);
        for _ in 0..n_layers {
            let n_planes = *data.get(at).ok_or("design.ovs: truncated")? as usize;
            at += 1;
            let mut planes = Vec::with_capacity(n_planes);
            for _ in 0..n_planes {
                let depth = *data.get(at).ok_or("design.ovs: truncated")?;
                at += 1;
                let mut levels = Vec::with_capacity(grid.n_levels as usize);
                for _ in 0..grid.n_levels {
                    let e = [g64(&data, at)?, g64(&data, at + 8)?, g64(&data, at + 16)?, g64(&data, at + 24)?];
                    at += 32;
                    for (off, len) in [(e[0], e[1]), (e[2], e[3])] {
                        if off.checked_add(len).is_none_or(|end| end > data.len() as u64) {
                            return Err("design.ovs: a level past the end".into());
                        }
                    }
                    levels.push(e);
                }
                planes.push(OvsPlane { depth, levels });
            }
            layers.push(planes);
        }
        Ok(OvsFile { data, group, src_size, src_mtime, grid, root, layers })
    }

    /// Whether this file was built for the index `ovm`: the same source and
    /// layer table.
    pub fn validate_against(&self, ovm: &Ovm) -> Result<(), String> {
        if (self.src_size, self.src_mtime) != (ovm.src_size, ovm.src_mtime) {
            return Err("design.ovs was built for another source".into());
        }
        if self.group != OVS_GROUP || self.layers.len() != ovm.n_layers as usize {
            return Err("design.ovs: another group or layer table".into());
        }
        if self.root.is_some_and(|r| r.ci >= ovm.n_cells) {
            return Err("design.ovs: a cell the index does not have".into());
        }
        Ok(())
    }

    /// One plane's level's bits: row-padded bytes row by row (all zero where
    /// none is set).
    pub fn bits(&self, k: usize, p: usize, lv: usize) -> Option<Vec<u8>> {
        let e = self.layers.get(k)?.get(p)?.levels.get(lv)?;
        let (w, h) = self.grid.level_dims(lv as u32);
        let n = (w as usize).div_ceil(8) * h as usize;
        if e[1] == 0 {
            return Some(vec![0; n]);
        }
        inflate(&self.data[e[0] as usize..(e[0] + e[1]) as usize], n)
    }

    /// One plane's level's means: a byte per group row by row (0 where it
    /// covers nothing under the cut).
    pub fn means(&self, k: usize, p: usize, lv: usize) -> Option<Vec<u8>> {
        let e = self.layers.get(k)?.get(p)?.levels.get(lv)?;
        let (w, h) = self.grid.level_dims(lv as u32);
        let n = w.div_ceil(OVS_GROUP) as usize * h.div_ceil(OVS_GROUP) as usize;
        if e[3] == 0 {
            return Some(vec![0; n]);
        }
        inflate(&self.data[e[2] as usize..(e[2] + e[3]) as usize], n)
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
    fn the_grid_covers_the_box_in_its_cells_down_to_the_top_grid() {
        // 1000 dbu a um; 18.7 x 10 mm: 16 um cells (1169 a side within 2048)
        assert_eq!(OvsGrid::auto_base_um(1000.0, 18_700_000), 16.0);
        assert_eq!(OvsGrid::auto_base_um(1000.0, 2_048_000), 1.0);
        assert_eq!(OvsGrid::auto_base_um(1000.0, 2_049_000), 2.0);
        assert_eq!(OvsGrid::auto_base_um(1000.0, 400_000), 0.25);
        let g = OvsGrid::new(1000.0, 16.0, (-5, 0, 18_700_000 - 5, 10_000_000)).unwrap();
        assert_eq!((g.cell_dbu, g.w, g.h), (16_000, 1169, 625));
        // 1169 x 625 -> 585 x 313 -> 293 x 157 -> 147 x 79 -> 74 x 40 -> 37 x 20
        assert_eq!(g.n_levels, 6);
        assert_eq!((g.level_dims(1), g.level_dims(5)), ((585, 313), (37, 20)));
        assert_eq!(g.cells((-5, 0, 16_000 - 5, 1)), (0, 0, 0, 0));
        assert_eq!(g.cells((-5, 0, 16_000 - 4, 16_001)), (0, 1, 0, 1));
        // outside: the edge cells
        assert_eq!(g.cells((-100_000, -100_000, 99_000_000, 99_000_000)), (0, 1168, 0, 624));
        // the reciprocal's cells are the exact ones, on the edges too
        let fast = FastCells::new(&g);
        for &v in &[-16_005i64, -6, -5, -4, 0, 15_994, 15_995, 15_996, 31_995, 31_996, 1_000_000, 18_699_994, 18_699_995, 99_000_000] {
            let rect = (v, v, v + 1, v + 1);
            assert_eq!(fast.cells(rect), g.cells(rect), "{}", v);
            let wide = (v, v - 7, v + 33_333, v + 16_000);
            assert_eq!(fast.cells(wide), g.cells(wide), "{}", v);
        }
        assert!(OvsGrid::new(1000.0, 16.0, (0, 0, 0, 10)).is_none());
        assert!(OvsGrid::new(1000.0, 0.0001, (0, 0, 10, 10)).is_none());
    }

    #[test]
    fn cells_set_pool_to_their_coarser_cells_and_count_by_group() {
        // 70 x 3 cells: two words a row
        let (w, h) = (70u32, 3u32);
        let words = words_of(w);
        let mut bits = vec![0u64; words * h as usize];
        set_cells(&mut bits, words, (0, 0, 0, 0));
        set_cells(&mut bits, words, (62, 65, 1, 2));
        set_cells(&mut bits, words, (69, 69, 2, 2));
        let get = |bits: &[u64], words: usize, i: u32, j: u32| (bits[j as usize * words + (i / 64) as usize] >> (i % 64)) & 1 == 1;
        let set: Vec<(u32, u32)> = (0..h).flat_map(|j| (0..w).map(move |i| (i, j))).filter(|&(i, j)| get(&bits, words, i, j)).collect();
        assert_eq!(set, vec![(0, 0), (62, 1), (63, 1), (64, 1), (65, 1), (62, 2), (63, 2), (64, 2), (65, 2), (69, 2)]);
        // 35 x 2: a cell where any of its 2 x 2 is
        let pooled = pool_bits(&bits, w, h);
        let pw = words_of(35);
        let set: Vec<(u32, u32)> = (0..2).flat_map(|j| (0..35).map(move |i| (i, j))).filter(|&(i, j)| get(&pooled, pw, i, j)).collect();
        assert_eq!(set, vec![(0, 0), (31, 0), (32, 0), (31, 1), (32, 1), (34, 1)]);
        // the bytes as stored: 9 a row, bit i of byte i / 8
        let stored = row_bytes(&bits, words, w, h);
        assert_eq!(stored.len(), 27);
        assert_eq!((stored[0], stored[9 + 7], stored[9 + 8], stored[18 + 8]), (1, 0xC0, 0x03, 0x23));
        // group (7, 0) holds cells 56-63 of rows 0-2: 62 and 63 of rows 1, 2
        assert_eq!((group_occ(&bits, words, h, 7, 0), group_occ(&bits, words, h, 8, 0), group_occ(&bits, words, h, 0, 0)), (4, 5, 1));
        assert_eq!(pool_area(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 3, 2), vec![1.0 + 2.0 + 4.0 + 5.0, 3.0 + 6.0]);
        // a rectangle's cells by group
        let mut seen = Vec::new();
        groups_of(9, (6, 9, 7, 8), |g, c| seen.push((g, c)));
        assert_eq!(seen, vec![(0, 2), (1, 2), (9, 2), (10, 2)]);
    }

    #[test]
    fn sparse_tiles_or_into_their_cells() {
        // 130 x 70 cells: 3 x 2 tiles
        let (w, h) = (130u32, 70u32);
        let tiles_w = w.div_ceil(TILE);
        let mut tiles = Tiles::default();
        tiles.set(tiles_w, (63, 64, 63, 64));
        tiles.set(tiles_w, (129, 129, 69, 69));
        assert_eq!(tiles.tiles.len(), 5);
        assert_eq!(tiles.index.len(), 5);
        let words = words_of(w);
        let mut dense = vec![0u64; words * h as usize];
        tiles.or_into(tiles_w, &mut dense, words, h);
        let mut direct = vec![0u64; words * h as usize];
        set_cells(&mut direct, words, (63, 64, 63, 64));
        set_cells(&mut direct, words, (129, 129, 69, 69));
        assert_eq!(dense, direct);
    }

    #[test]
    fn a_page_counts_its_shapes_by_their_larger_side_and_a_path_with_its_ends() {
        use floe_oasis::doc::{Cell, PathRec, RectRec};
        // cells of 1000 from (0, 0); the cut 3000 at level 0, 6000 at 1
        let rect = |x, y, w, h| RectRec { layer: 1, dt: 0, x, y, w, h, rep: Rep::One };
        let cell = Cell {
            rects: vec![
                // 3.5 x 3.5: class 1 (its larger side past level 0's cut)
                rect(10_000, 10_000, 3_500, 3_500),
                // a 0.4 x 32 wire: past every level's cut by its larger side
                rect(20_000, 0, 400, 32_000),
                // 0.5 x 0.5: class 0
                rect(1_200, 1_200, 500, 500),
            ],
            paths: vec![PathRec { layer: 1, dt: 0, pts: vec![(5_000, 30_500), (6_000, 30_500)], hw: 200, es: 1_500, ee: 1_500, rep: Rep::One }],
            ..Cell::default()
        };
        let sub = page_cells(&cell, (0, 0), 1_000, 3_000, 2);
        let class = |c: u8| sub.classes.iter().find(|(k, _)| *k == c).map(|(_, cells)| cells.clone()).unwrap_or_default();
        let cells = |c: u8| class(c).iter().map(|&(i, j, _)| (i, j)).collect::<Vec<_>>();
        // the square's 4 x 4 cells at class 1, the wire nowhere
        let square: Vec<(i32, i32)> = cells(1).into_iter().filter(|&(_, j)| j < 30).collect();
        assert_eq!(square.len(), 16);
        assert!(square.iter().all(|&(i, j)| (10..=13).contains(&i) && (10..=13).contains(&j)));
        assert!(class(0).iter().chain(class(1).iter()).all(|&(i, _, _)| i != 20));
        let area: f32 = class(1).iter().filter(|c| c.1 < 30).map(|c| c.2).sum();
        assert!((area - 3_500.0 * 3_500.0).abs() < 1.0, "{}", area);
        // the small square in its cell, at class 0
        assert_eq!(cells(0), vec![(1, 1)]);
        // the path's outline 3.5 to 7.5 um with its extensions - 4 um, class
        // 1 by its larger side - in row 30: cells 3 to 7 (its spine's box
        // grown by the half width took 4 to 6, class 0)
        let row: Vec<i32> = cells(1).iter().filter(|&&(_, j)| j == 30).map(|&(i, _)| i).collect();
        assert_eq!(row, vec![3, 4, 5, 6, 7]);
        let path_area: f32 = class(1).iter().filter(|c| c.1 == 30).map(|c| c.2).sum();
        assert!((path_area - 4_000.0 * 400.0).abs() < 1.0, "{}", path_area);
    }

    #[test]
    fn the_file_reads_back_what_was_written() {
        let g = OvsGrid::new(1000.0, 1.0, (-10, -20, 16_000 - 10, 9_000 - 20)).unwrap();
        assert_eq!((g.w, g.h, g.n_levels), (16, 9, 1));
        let bits: Vec<u8> = (0..18).collect();
        let means: Vec<u8> = vec![40, 0, 0, 200];
        let blobs: Blobs = vec![vec![(0, vec![(deflate(&bits), deflate(&means))]), (3, vec![(Vec::new(), Vec::new())])], Vec::new()];
        let bytes = encode(123, 456, &g, &blobs);
        let f = OvsFile::from_bytes(bytes.clone()).unwrap();
        assert_eq!((f.group, f.src_size, f.src_mtime, f.grid), (OVS_GROUP, 123, 456, g));
        assert_eq!(f.layers.len(), 2);
        assert_eq!(f.layers[0].iter().map(|p| p.depth).collect::<Vec<_>>(), vec![0, 3]);
        assert!(f.layers[1].is_empty());
        assert_eq!(f.bits(0, 0, 0), Some(bits));
        assert_eq!(f.means(0, 0, 0), Some(means));
        assert_eq!(f.bits(0, 1, 0), Some(vec![0; 18]));
        assert_eq!(f.means(0, 1, 0), Some(vec![0; 4]));
        assert_eq!(f.bits(0, 2, 0), None);
        assert_eq!(f.bits(1, 0, 0), None);
        // refused: another magic, another version (the first's too), a
        // table past the end
        let mut other = bytes.clone();
        other[0] = b'X';
        assert!(OvsFile::from_bytes(other).is_err());
        // a big cell's file: its cell and placement after the header
        let root = OvsRoot { ci: 7, x: -1_234_567, y: 89, rot: 3, flip: true };
        let rooted = encode_file(123, 456, &g, Some(&root), &blobs);
        let r = OvsFile::from_bytes(rooted).unwrap();
        assert_eq!((r.root, r.grid), (Some(root), g));
        assert_eq!((r.bits(0, 0, 0), r.means(0, 1, 0)), (f.bits(0, 0, 0), f.means(0, 1, 0)));
        assert_eq!(f.root, None);
        for old in [1u8, 2] {
            let mut version = bytes.clone();
            version[8] = old;
            assert!(OvsFile::from_bytes(version).unwrap_err().contains(&format!("version {}", old)));
        }
        assert!(OvsFile::from_bytes(bytes[..bytes.len() - 3].to_vec()).is_err());
    }

    #[test]
    fn a_cells_bits_go_into_the_tops_at_its_place() {
        // a 100 x 10 owner at the top's cell (72, 16): its words cross the
        // top's, shifted by 8
        let (w, h, tw, th) = (100u32, 10u32, 200u32, 40u32);
        let (words, top_words) = (words_of(w), words_of(tw));
        let mut src = vec![0u64; words * h as usize];
        let marks = [(0u32, 0u32), (63, 0), (64, 5), (55, 9), (99, 9)];
        for &(i, j) in &marks {
            set_cells(&mut src, words, (i, i, j, j));
        }
        let mut dst = vec![0u64; top_words * th as usize];
        set_cells(&mut dst, top_words, (3, 3, 2, 2));
        or_shifted(&mut dst, top_words, &src, words, h, (72, 16));
        let mut want = vec![0u64; top_words * th as usize];
        set_cells(&mut want, top_words, (3, 3, 2, 2));
        for &(i, j) in &marks {
            set_cells(&mut want, top_words, (72 + i, 72 + i, 16 + j, 16 + j));
        }
        assert_eq!(dst, want);
        // at a word's start: as it is
        let mut at0 = vec![0u64; top_words * th as usize];
        or_shifted(&mut at0, top_words, &src, words, h, (64, 0));
        let mut want0 = vec![0u64; top_words * th as usize];
        for &(i, j) in &marks {
            set_cells(&mut want0, top_words, (64 + i, 64 + i, j, j));
        }
        assert_eq!(at0, want0);
    }
}
