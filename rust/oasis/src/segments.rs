//! Segment statistics of an OASIS stream: what a sub-cell random-access
//! directory would find (docs/DIRECT_READ_PLAN.ko.md §4, §7, L0;
//! 2026-10-21). A segment is one CBLOCK (its body is one inflate, so
//! nothing finer can be read on its own) or, between CBLOCKs and in a
//! stream without them, a span of records closed at the first record
//! boundary past `span_bytes` (and at every CELL record: modal state
//! resets there). Per segment: its file bytes, records, the shapes'
//! box (repetitions widened), the distinct layers, placements and
//! texts, and whether a record in it reused a modal LIST spelled out
//! in an earlier segment (a point-list repetition, a polygon point
//! list, a path point list) - the lists a boundary snapshot would have
//! to carry or chain to. `summarize` turns the per-segment rows into
//! the questions L0 asks of a field file: how a segment's box compares
//! with its cell's (tile / band / full), how far the segments' boxes
//! overlap (their area sum over the cell's area), the layers per
//! segment, and the chain frequency. Collected by the scan frame
//! (lib.rs `record`) when `floe-index scan` runs; the inventory pass
//! does not collect it.

use crate::geom::{box_grow, box_is_empty, path_bbox, Box4, EMPTY_BOX};
use crate::{CellStat, ScanStats};

/// the span length of a stream without CBLOCKs (`--seg-bytes`)
pub const DEFAULT_SPAN_BYTES: u64 = 256 * 1024;

/// SegStat::chained bits
pub const CHAIN_REP: u8 = 1;
pub const CHAIN_POLY: u8 = 2;
pub const CHAIN_PATH: u8 = 4;

/// the segment's box against its cell's: both sides within a quarter
/// of the cell's = tile; one side over half = band; both over half =
/// full; the rest = other
pub const TILE_SHARE: f64 = 0.25;
pub const LONG_SHARE: f64 = 0.5;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SegStat {
    /// index into ScanStats::cell_stats (file order)
    pub cell: u32,
    pub cblock: bool,
    /// file offset of the first byte (a CBLOCK: its record id)
    pub offset: u64,
    /// file bytes (a CBLOCK: the whole record, header included)
    pub bytes: u64,
    /// bytes of the records themselves (a CBLOCK: its uncomp-count; a
    /// span: `bytes`)
    pub inflated: u64,
    pub records: u64,
    pub shapes: u64,
    /// the shapes' members (repetitions expanded, saturating)
    pub members: u64,
    pub placements: u64,
    pub texts: u64,
    /// the shapes' boxes, repetitions widened; EMPTY_BOX without one
    pub bbox: Box4,
    /// the shapes' distinct (layer, datatype) pairs, sorted
    pub layers: Vec<(u64, u64)>,
    /// CHAIN_* bits: a record reused a modal list an earlier segment
    /// spelled out
    pub chained: u8,
    /// the largest such list (repetition members or vertices)
    pub chain_list_max: u64,
}

/// the open segment and the modal-list bookkeeping of one parse
#[derive(Debug, Default)]
pub struct SegCollector {
    pub span_bytes: u64,
    pub segs: Vec<SegStat>,
    cur: Option<SegStat>,
    /// ordinal of the open segment within this parse (a worker's
    /// numbering starts over: it starts at a CELL record, where every
    /// modal list is dropped)
    next_id: u64,
    cur_id: u64,
    /// (segment ordinal, size) of the modal lists as last spelled out:
    /// a point-list repetition (grid repetitions are scalars and are
    /// not tracked), the polygon point list, the path point list
    rep_def: Option<(u64, u64)>,
    poly_def: Option<(u64, u64)>,
    path_def: Option<(u64, u64)>,
    /// path outline scratch
    spine: Vec<(i64, i64)>,
}

impl SegCollector {
    pub fn new(span_bytes: u64) -> SegCollector {
        SegCollector {
            span_bytes: span_bytes.max(1),
            ..Default::default()
        }
    }

    pub fn is_open(&self) -> bool {
        self.cur.is_some()
    }

    fn open(&mut self, cell: u32, cblock: bool, offset: u64) {
        debug_assert!(self.cur.is_none(), "segment already open");
        self.cur_id = self.next_id;
        self.next_id += 1;
        self.cur = Some(SegStat {
            cell,
            cblock,
            offset,
            bbox: EMPTY_BOX,
            ..Default::default()
        });
    }

    /// a span opens at the first record that needs one
    pub(crate) fn open_span(&mut self, cell: u32, offset: u64) {
        if self.cur.is_none() {
            self.open(cell, false, offset);
        }
    }

    /// a CBLOCK is a segment of its own: any span closes before it
    pub(crate) fn open_cblock(&mut self, cell: u32, offset: u64, inflated: u64) {
        self.close(offset);
        self.open(cell, true, offset);
        if let Some(s) = self.cur.as_mut() {
            s.inflated = inflated;
        }
    }

    /// the open segment ends at `end` (exclusive file offset)
    pub(crate) fn close(&mut self, end: u64) {
        if let Some(mut s) = self.cur.take() {
            s.bytes = end.saturating_sub(s.offset);
            if !s.cblock {
                s.inflated = s.bytes;
            }
            self.segs.push(s);
        }
    }

    /// after a record at the stream's top level: a span past its
    /// length closes at the record boundary
    pub(crate) fn record_end(&mut self, end: u64) {
        let full = match self.cur.as_ref() {
            Some(s) if !s.cblock => end.saturating_sub(s.offset) >= self.span_bytes,
            _ => false,
        };
        if full {
            self.close(end);
        }
    }

    /// a CELL record: modal lists are dropped
    pub(crate) fn cell_reset(&mut self) {
        self.rep_def = None;
        self.poly_def = None;
        self.path_def = None;
    }

    pub(crate) fn record(&mut self) {
        if let Some(s) = self.cur.as_mut() {
            s.records += 1;
        }
    }

    pub(crate) fn shape(&mut self, layer: u64, datatype: u64, members: u64, bb: Option<Box4>) {
        let Some(s) = self.cur.as_mut() else {
            return;
        };
        s.shapes += 1;
        s.members = s.members.saturating_add(members);
        if let Some(b) = bb {
            box_grow(&mut s.bbox, &b);
        }
        let key = (layer, datatype);
        if let Err(i) = s.layers.binary_search(&key) {
            s.layers.insert(i, key);
        }
    }

    pub(crate) fn place(&mut self) {
        if let Some(s) = self.cur.as_mut() {
            s.placements += 1;
        }
    }

    pub(crate) fn text(&mut self) {
        if let Some(s) = self.cur.as_mut() {
            s.texts += 1;
        }
    }

    /// a repetition spelled out: `pts` = Some(members) for a point-list
    /// kind (types 4-7, 10, 11), None for a grid
    pub(crate) fn rep_defined(&mut self, pts: Option<u64>) {
        self.rep_def = pts.map(|m| (self.cur_id, m));
    }

    pub(crate) fn rep_reused(&mut self) {
        let def = self.rep_def;
        self.reuse(CHAIN_REP, def);
    }

    pub(crate) fn poly_defined(&mut self, verts: u64) {
        self.poly_def = Some((self.cur_id, verts));
    }

    pub(crate) fn poly_reused(&mut self) {
        let def = self.poly_def;
        self.reuse(CHAIN_POLY, def);
    }

    pub(crate) fn path_defined(&mut self, verts: u64) {
        self.path_def = Some((self.cur_id, verts));
    }

    pub(crate) fn path_reused(&mut self) {
        let def = self.path_def;
        self.reuse(CHAIN_PATH, def);
    }

    fn reuse(&mut self, bit: u8, def: Option<(u64, u64)>) {
        let Some(s) = self.cur.as_mut() else {
            return;
        };
        if let Some((id, size)) = def {
            if id != self.cur_id {
                s.chained |= bit;
                s.chain_list_max = s.chain_list_max.max(size);
            }
        }
    }

    /// a path's box: the modal spine (anchor-relative) placed at the
    /// anchor, then geom::path_bbox (the build's own function)
    pub(crate) fn path_box(
        &mut self,
        deltas: &[(i64, i64)],
        ax: i64,
        ay: i64,
        hw: i64,
        es: i64,
        ee: i64,
    ) -> Box4 {
        self.spine.clear();
        self.spine
            .extend(deltas.iter().map(|&(dx, dy)| (ax + dx, ay + dy)));
        path_bbox(&self.spine, hw, es, ee)
    }

    /// a parallel scan appends a worker's segments behind the ones
    /// before it, its cells numbered from `cell_base`
    pub(crate) fn append(&mut self, mut other: SegCollector, cell_base: u32) {
        debug_assert!(other.cur.is_none(), "worker left a segment open");
        for s in other.segs.iter_mut() {
            s.cell += cell_base;
        }
        self.segs.append(&mut other.segs);
    }
}

// ------------------------------------------------------------ summary

/// max / p99 / p90 / p50 of a sorted list (nearest rank, rounded)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Quartet {
    pub max: u64,
    pub p99: u64,
    pub p90: u64,
    pub p50: u64,
}

impl Quartet {
    pub fn of(values: &mut Vec<u64>) -> Quartet {
        values.sort_unstable();
        Quartet {
            max: values.last().copied().unwrap_or(0),
            p99: pct(values, 0.99),
            p90: pct(values, 0.90),
            p50: pct(values, 0.50),
        }
    }
}

fn pct(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

fn pct_f(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

/// segments by the shape of their box against the cell's
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Classes {
    pub tile: u64,
    pub band: u64,
    pub full: u64,
    pub other: u64,
    /// no shape (placements, texts, properties only)
    pub empty: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Tile,
    Band,
    Full,
    Other,
    Empty,
}

impl Classes {
    fn add(&mut self, c: Class, weight: u64) {
        match c {
            Class::Tile => self.tile += weight,
            Class::Band => self.band += weight,
            Class::Full => self.full += weight,
            Class::Other => self.other += weight,
            Class::Empty => self.empty += weight,
        }
    }

    pub fn total(&self) -> u64 {
        self.tile + self.band + self.full + self.other + self.empty
    }
}

/// a segment's class against a cell box (`cell` = the union of the
/// cell's shape segments)
pub fn classify(seg: &Box4, cell: &Box4) -> Class {
    if box_is_empty(seg) || box_is_empty(cell) {
        return Class::Empty;
    }
    let share = |a: i64, b: i64, ca: i64, cb: i64| -> f64 {
        let (w, cw) = ((b - a).max(0) as f64, (cb - ca).max(0) as f64);
        if cw <= 0.0 {
            1.0
        } else {
            w / cw
        }
    };
    let fx = share(seg.0, seg.2, cell.0, cell.2);
    let fy = share(seg.1, seg.3, cell.1, cell.3);
    let (lx, ly) = (fx > LONG_SHARE, fy > LONG_SHARE);
    if lx && ly {
        Class::Full
    } else if lx || ly {
        Class::Band
    } else if fx <= TILE_SHARE && fy <= TILE_SHARE {
        Class::Tile
    } else {
        Class::Other
    }
}

fn area(b: &Box4) -> f64 {
    if box_is_empty(b) {
        return 0.0;
    }
    ((b.2 - b.0).max(0) as f64) * ((b.3 - b.1).max(0) as f64)
}

/// one cell's segments (the ten biggest cells by file bytes)
#[derive(Debug, Clone, Default)]
pub struct CellSegs {
    pub cell: u32,
    pub name: String,
    pub segments: u64,
    pub bytes: u64,
    pub shape_segments: u64,
    /// the shape segments' box areas summed over the cell box's area
    /// (0 without shapes)
    pub overlap: f64,
    pub classes: Classes,
    pub layers_p50: u64,
    pub chained: u64,
}

#[derive(Debug, Clone, Default)]
pub struct SegSummary {
    pub span_bytes: u64,
    pub segments: u64,
    pub cblocks: u64,
    pub spans: u64,
    pub bytes: Quartet,
    pub inflated: Quartet,
    pub records: Quartet,
    /// layers per shape segment
    pub layers: Quartet,
    pub with_shapes: u64,
    pub with_placements: u64,
    pub with_texts: u64,
    pub by_count: Classes,
    pub by_bytes: Classes,
    /// cells with two or more shape segments, and their overlap
    pub overlap_cells: u64,
    pub overlap_p50: f64,
    pub overlap_p90: f64,
    pub overlap_max: f64,
    pub chained: u64,
    pub chained_rep: u64,
    pub chained_poly: u64,
    pub chained_path: u64,
    pub chain_list_max: u64,
    pub cells: Vec<CellSegs>,
}

/// the L0 questions over the collected segments (None: not collected)
pub fn summarize(st: &ScanStats) -> Option<SegSummary> {
    let col = st.segments()?;
    let cells: &[CellStat] = &st.cell_stats;
    let mut sum = SegSummary {
        span_bytes: col.span_bytes,
        segments: col.segs.len() as u64,
        ..Default::default()
    };
    let (mut bytes, mut inflated, mut records, mut layers) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut per_cell: Vec<CellSegs> = Vec::new();
    let mut cell_at: Vec<Option<usize>> = vec![None; cells.len()];
    let mut cell_box: Vec<Box4> = vec![EMPTY_BOX; cells.len()];
    let mut cell_layers: Vec<Vec<u64>> = vec![Vec::new(); cells.len()];
    let mut cell_area_sum: Vec<f64> = vec![0.0; cells.len()];
    for s in &col.segs {
        if let Some(b) = cell_box.get_mut(s.cell as usize) {
            box_grow(b, &s.bbox);
        }
    }
    for s in &col.segs {
        if s.cblock {
            sum.cblocks += 1;
        } else {
            sum.spans += 1;
        }
        bytes.push(s.bytes);
        inflated.push(s.inflated);
        records.push(s.records);
        if s.shapes > 0 {
            sum.with_shapes += 1;
            layers.push(s.layers.len() as u64);
        }
        if s.placements > 0 {
            sum.with_placements += 1;
        }
        if s.texts > 0 {
            sum.with_texts += 1;
        }
        if s.chained != 0 {
            sum.chained += 1;
        }
        if s.chained & CHAIN_REP != 0 {
            sum.chained_rep += 1;
        }
        if s.chained & CHAIN_POLY != 0 {
            sum.chained_poly += 1;
        }
        if s.chained & CHAIN_PATH != 0 {
            sum.chained_path += 1;
        }
        sum.chain_list_max = sum.chain_list_max.max(s.chain_list_max);
        let ci = s.cell as usize;
        let class = match cell_box.get(ci) {
            Some(cb) if s.shapes > 0 => classify(&s.bbox, cb),
            _ => Class::Empty,
        };
        sum.by_count.add(class, 1);
        sum.by_bytes.add(class, s.bytes);
        if ci < cells.len() {
            let at = match cell_at[ci] {
                Some(at) => at,
                None => {
                    per_cell.push(CellSegs {
                        cell: s.cell,
                        name: cells[ci].name.clone(),
                        ..Default::default()
                    });
                    cell_at[ci] = Some(per_cell.len() - 1);
                    per_cell.len() - 1
                }
            };
            let pc = &mut per_cell[at];
            pc.segments += 1;
            pc.bytes += s.bytes;
            pc.classes.add(class, 1);
            if s.chained != 0 {
                pc.chained += 1;
            }
            if s.shapes > 0 {
                pc.shape_segments += 1;
                cell_layers[ci].push(s.layers.len() as u64);
                cell_area_sum[ci] += area(&s.bbox);
            }
        }
    }
    sum.bytes = Quartet::of(&mut bytes);
    sum.inflated = Quartet::of(&mut inflated);
    sum.records = Quartet::of(&mut records);
    sum.layers = Quartet::of(&mut layers);
    let mut overlaps: Vec<f64> = Vec::new();
    for pc in per_cell.iter_mut() {
        let ci = pc.cell as usize;
        let ca = area(&cell_box[ci]);
        pc.overlap = if pc.shape_segments > 0 && ca > 0.0 {
            cell_area_sum[ci] / ca
        } else {
            0.0
        };
        pc.layers_p50 = Quartet::of(&mut cell_layers[ci]).p50;
        if pc.shape_segments >= 2 && ca > 0.0 {
            overlaps.push(pc.overlap);
        }
    }
    overlaps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sum.overlap_cells = overlaps.len() as u64;
    sum.overlap_p50 = pct_f(&overlaps, 0.50);
    sum.overlap_p90 = pct_f(&overlaps, 0.90);
    sum.overlap_max = overlaps.last().copied().unwrap_or(0.0);
    // the ten biggest cells by file bytes, ties by file order
    per_cell.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.cell.cmp(&b.cell)));
    per_cell.truncate(10);
    sum.cells = per_cell;
    Some(sum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::write::W;
    use std::io::Write;

    const MAGIC: &[u8] = b"%SEMI-OASIS\r\n";

    fn start(w: &mut W) {
        w.out.extend_from_slice(MAGIC);
        w.uint(1);
        w.string(b"1.0");
        w.real_f64(1000.0);
        w.uint(0);
        for _ in 0..12 {
            w.uint(0);
        }
    }

    fn end(w: &mut W) {
        w.uint(2);
        w.uint(252);
        w.out.extend_from_slice(&[0u8; 252]);
        w.uint(0);
    }

    fn deflate(bytes: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::DeflateEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        );
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    fn cblock(w: &mut W, body: &[u8]) {
        let comp = deflate(body);
        w.uint(34);
        w.uint(0);
        w.uint(body.len() as u64);
        w.uint(comp.len() as u64);
        w.out.extend_from_slice(&comp);
    }

    /// RECT 1/0 at (0,0) 10x10 with an x point-list repetition of three
    /// members (0, 5, 10); POLYGON 2/0 at (0,0), three general deltas
    /// (box 0..20); the first segment's records
    fn part_one(b: &mut W) {
        b.uint(20);
        b.byte(0x80 | 0x40 | 0x10 | 0x08 | 0x04 | 0x02 | 0x01);
        b.uint(1);
        b.uint(0);
        b.uint(10);
        b.sint(0);
        b.sint(0);
        b.uint(4);
        b.uint(1);
        b.uint(5);
        b.uint(5);
        b.uint(21);
        b.byte(0x20 | 0x10 | 0x08 | 0x02 | 0x01);
        b.uint(2);
        b.uint(0);
        b.uint(4);
        b.uint(3);
        b.g_delta(20, 0);
        b.g_delta(0, 20);
        b.g_delta(-20, 0);
        b.sint(0);
        b.sint(0);
    }

    /// RECT 1/0 at (100,100) reusing the modal repetition (type 0) -
    /// a chained list of 3 (layer and datatype spelled out: the modal
    /// layer is the polygon's 2/0); POLYGON 2/0 at (200,200) without a point
    /// list - the chained polygon list of 4 vertices; PATH 3/0 at
    /// (300,300), half-width 2, explicit extensions 2/2, manhattan
    /// list +50 x +30 y
    fn part_two(b: &mut W) {
        b.uint(20);
        b.byte(0x80 | 0x10 | 0x08 | 0x04 | 0x02 | 0x01);
        b.uint(1);
        b.uint(0);
        b.sint(100);
        b.sint(100);
        b.uint(0);
        b.uint(21);
        b.byte(0x10 | 0x08 | 0x02 | 0x01);
        b.uint(2);
        b.uint(0);
        b.sint(200);
        b.sint(200);
        b.uint(22);
        b.byte(0x80 | 0x40 | 0x20 | 0x10 | 0x08 | 0x02 | 0x01);
        b.uint(3);
        b.uint(0);
        b.uint(2);
        b.uint((3 << 2) | 3);
        b.sint(2);
        b.sint(2);
        b.uint(0);
        b.uint(2);
        b.sint(50);
        b.sint(30);
        b.sint(300);
        b.sint(300);
    }

    /// PATH 3/0 at (400,400) without a point list - the chained path
    /// list of 3 vertices; RECT 4/0 at (0,500) 10x10, no repetition
    fn part_three(b: &mut W) {
        b.uint(22);
        b.byte(0x10 | 0x08);
        b.sint(400);
        b.sint(400);
        b.uint(20);
        b.byte(0x80 | 0x40 | 0x10 | 0x08 | 0x02 | 0x01);
        b.uint(4);
        b.uint(0);
        b.uint(10);
        b.sint(0);
        b.sint(500);
    }

    /// cell A's three parts in CBLOCKs, then a second cell B with one
    /// plain rectangle (so a parallel scan has two cells to cut at)
    fn fixture_cblocks() -> Vec<u8> {
        let mut w = W::new();
        start(&mut w);
        w.uint(14);
        w.string(b"A");
        for part in [part_one, part_two, part_three] {
            let mut b = W::new();
            part(&mut b);
            cblock(&mut w, &b.out);
        }
        w.uint(14);
        w.string(b"B");
        let mut b = W::new();
        part_three(&mut b);
        // the path without a list would be refused by nothing here: the
        // plain scan tolerates it; keep B to a rectangle only
        b.out.clear();
        b.uint(20);
        b.byte(0x80 | 0x40 | 0x10 | 0x08 | 0x02 | 0x01);
        b.uint(7);
        b.uint(0);
        b.uint(10);
        b.sint(0);
        b.sint(0);
        w.out.extend_from_slice(&b.out);
        end(&mut w);
        w.out
    }

    /// the same records of A plain (no CBLOCK), then B
    fn fixture_plain() -> Vec<u8> {
        let mut w = W::new();
        start(&mut w);
        w.uint(14);
        w.string(b"A");
        part_one(&mut w);
        part_two(&mut w);
        part_three(&mut w);
        w.uint(14);
        w.string(b"B");
        w.uint(20);
        w.byte(0x80 | 0x40 | 0x10 | 0x08 | 0x02 | 0x01);
        w.uint(7);
        w.uint(0);
        w.uint(10);
        w.sint(0);
        w.sint(0);
        end(&mut w);
        w.out
    }

    const A_SEG0: Box4 = (0, 0, 20, 20);
    const A_SEG1: Box4 = (100, 100, 352, 332);
    const A_SEG2: Box4 = (0, 398, 452, 510);
    const A_CELL: Box4 = (0, 0, 452, 510);

    #[test]
    fn cblocks_are_segments_with_boxes_layers_and_chains() {
        let data = fixture_cblocks();
        for jobs in [1usize, 3] {
            let st = crate::scan_segments(&data, jobs, DEFAULT_SPAN_BYTES).expect("scan");
            let segs = &st.segments().expect("collected").segs;
            assert_eq!(segs.len(), 4, "jobs {}", jobs);
            let a: Vec<&SegStat> = segs.iter().filter(|s| s.cell == 0).collect();
            assert_eq!(a.len(), 3);
            assert!(a.iter().all(|s| s.cblock));
            assert_eq!((a[0].records, a[0].shapes, a[0].members), (2, 2, 4));
            assert_eq!(a[0].layers, vec![(1, 0), (2, 0)]);
            assert_eq!(a[0].bbox, A_SEG0);
            assert_eq!(a[0].chained, 0);
            assert_eq!((a[1].records, a[1].shapes, a[1].members), (3, 3, 5));
            assert_eq!(a[1].layers, vec![(1, 0), (2, 0), (3, 0)]);
            assert_eq!(a[1].bbox, A_SEG1);
            assert_eq!(a[1].chained, CHAIN_REP | CHAIN_POLY);
            assert_eq!(a[1].chain_list_max, 4);
            assert_eq!((a[2].records, a[2].shapes), (2, 2));
            assert_eq!(a[2].layers, vec![(3, 0), (4, 0)]);
            assert_eq!(a[2].bbox, A_SEG2);
            assert_eq!(a[2].chained, CHAIN_PATH);
            assert_eq!(a[2].chain_list_max, 3);
            // the CBLOCK's uncompressed count and its whole record
            assert!(a.iter().all(|s| s.inflated > 0 && s.bytes > s.inflated / 8));
            assert!(a[0].offset < a[1].offset && a[1].offset < a[2].offset);
            assert_eq!(a[1].offset, a[0].offset + a[0].bytes);
            // B: one plain record = one span, no chain across the CELL
            let b = segs.iter().find(|s| s.cell == 1).expect("B's span");
            assert!(!b.cblock);
            assert_eq!((b.records, b.shapes, b.chained), (1, 1, 0));
            assert_eq!(b.layers, vec![(7, 0)]);
            assert_eq!(st.cell_stats[0].bbox, Some(A_CELL));
            assert_eq!(st.cell_stats[1].bbox, Some((0, 0, 10, 10)));
            // the summary
            let sum = summarize(&st).expect("summary");
            assert_eq!((sum.segments, sum.cblocks, sum.spans), (4, 3, 1));
            assert_eq!(sum.with_shapes, 4);
            assert_eq!(
                sum.by_count,
                Classes { tile: 1, band: 2, full: 1, other: 0, empty: 0 }
            );
            assert_eq!(sum.by_count.total(), 4);
            assert_eq!(sum.by_bytes.total(), segs.iter().map(|s| s.bytes).sum::<u64>());
            assert_eq!(
                (sum.chained, sum.chained_rep, sum.chained_poly, sum.chained_path),
                (2, 1, 1, 1)
            );
            assert_eq!(sum.chain_list_max, 4);
            assert_eq!(sum.layers, Quartet { max: 3, p99: 3, p90: 3, p50: 2 });
            assert_eq!(sum.overlap_cells, 1);
            let want = (400.0 + 252.0 * 232.0 + 452.0 * 112.0) / (452.0 * 510.0);
            assert!((sum.overlap_p50 - want).abs() < 1e-12, "{}", sum.overlap_p50);
            assert_eq!(sum.cells.len(), 2);
            assert_eq!(sum.cells[0].name, "A");
            assert_eq!((sum.cells[0].segments, sum.cells[0].shape_segments), (3, 3));
            assert_eq!(sum.cells[0].classes.tile, 1);
            assert_eq!(sum.cells[0].layers_p50, 2);
            assert_eq!(sum.cells[0].chained, 2);
            assert_eq!(sum.cells[1].name, "B");
            assert_eq!(sum.cells[1].overlap, 1.0);
        }
    }

    #[test]
    fn spans_close_at_their_length_and_at_cell_records() {
        let data = fixture_plain();
        // every record its own span: a span closes at the first record
        // boundary past its length
        let st = crate::scan_segments(&data, 1, 1).expect("scan");
        let segs = &st.segments().unwrap().segs;
        assert_eq!(segs.len(), 8);
        assert!(segs.iter().all(|s| !s.cblock && s.records == 1));
        assert!(segs.iter().take(7).all(|s| s.cell == 0));
        assert_eq!(segs[7].cell, 1);
        for pair in segs.windows(2) {
            if pair[0].cell == pair[1].cell {
                assert_eq!(pair[0].offset + pair[0].bytes, pair[1].offset);
            }
        }
        assert_eq!(segs.iter().map(|s| s.inflated).sum::<u64>(), segs.iter().map(|s| s.bytes).sum::<u64>());
        // the chains cross the artificial boundaries: the second rect
        // reuses the first's repetition, the second polygon the first's
        // list, the second path the first's
        let chained: Vec<(usize, u8, u64)> = segs
            .iter()
            .enumerate()
            .filter(|(_, s)| s.chained != 0)
            .map(|(i, s)| (i, s.chained, s.chain_list_max))
            .collect();
        assert_eq!(
            chained,
            vec![(2, CHAIN_REP, 3), (3, CHAIN_POLY, 4), (5, CHAIN_PATH, 3)]
        );
        let boxes: Vec<Box4> = segs.iter().map(|s| s.bbox).collect();
        assert_eq!(boxes[0], (0, 0, 20, 10));
        assert_eq!(boxes[1], (0, 0, 20, 20));
        assert_eq!(boxes[2], (100, 100, 120, 110));
        assert_eq!(boxes[3], (200, 200, 220, 220));
        assert_eq!(boxes[4], (298, 298, 352, 332));
        assert_eq!(boxes[5], (398, 398, 452, 432));
        assert_eq!(boxes[6], (0, 500, 10, 510));
        // one long span per cell: nothing chains inside a segment
        let one = crate::scan_segments(&data, 1, 1 << 30).expect("scan");
        let segs = &one.segments().unwrap().segs;
        assert_eq!(segs.len(), 2);
        assert_eq!((segs[0].records, segs[0].shapes, segs[0].chained), (7, 7, 0));
        assert_eq!(segs[0].bbox, A_CELL);
        assert_eq!(segs[0].layers, vec![(1, 0), (2, 0), (3, 0), (4, 0)]);
        assert_eq!((segs[1].cell, segs[1].records), (1, 1));
        let sum = summarize(&one).unwrap();
        assert_eq!((sum.segments, sum.spans, sum.chained), (2, 2, 0));
        assert_eq!(sum.by_count, Classes { tile: 0, band: 0, full: 2, other: 0, empty: 0 });
        assert_eq!(sum.overlap_cells, 0);
        // a span length of 60 bytes: boundaries fall where a record
        // ends past 60 bytes, never inside one
        let mid = crate::scan_segments(&data, 1, 60).expect("scan");
        let segs = &mid.segments().unwrap().segs;
        assert!(segs.len() > 2 && segs.len() < 8, "{}", segs.len());
        assert!(segs.iter().filter(|s| s.cell == 0).all(|s| s.bytes >= 60 || s.records >= 1));
        assert_eq!(segs.iter().map(|s| s.records).sum::<u64>(), 8);
    }

    #[test]
    fn a_parallel_scan_reports_the_same_segments() {
        for data in [fixture_cblocks(), fixture_plain()] {
            for span in [1u64, 60, DEFAULT_SPAN_BYTES] {
                let one = crate::scan_segments(&data, 1, span).expect("scan");
                let par = crate::scan_segments(&data, 3, span).expect("scan");
                assert_eq!(one.segments().unwrap().segs, par.segments().unwrap().segs);
                let (a, b) = (summarize(&one).unwrap(), summarize(&par).unwrap());
                assert_eq!((a.segments, a.chained, a.by_count), (b.segments, b.chained, b.by_count));
                assert_eq!(a.overlap_p50, b.overlap_p50);
            }
        }
    }

    #[test]
    fn the_plain_scan_leaves_segments_out() {
        let data = fixture_cblocks();
        let st = crate::scan_parallel(&data, 2).expect("scan");
        assert!(st.segments().is_none());
        assert!(summarize(&st).is_none());
        assert_eq!(st.cell_stats[0].bbox, None);
    }

    #[test]
    fn the_adversarial_streams_scan_with_segments() {
        use crate::inventory::fixtures;
        for (name, data) in [
            ("early", fixtures::adversarial(true, true)),
            ("late-plain", fixtures::adversarial(false, false)),
            ("head", fixtures::head_cblock_cell()),
        ] {
            for jobs in [1usize, 2] {
                let st = crate::scan_segments(&data, jobs, 64).unwrap_or_else(|e| panic!("{}: {}", name, e));
                let segs = &st.segments().unwrap().segs;
                let shapes: u64 = st.shapes.values().map(|s| s.records).sum();
                let in_segs: u64 = segs.iter().map(|s| s.shapes).sum();
                // every shape inside a visible cell lands in a segment
                // (a cell a head CBLOCK defines has none)
                assert!(in_segs <= shapes, "{}", name);
                if name != "head" {
                    assert_eq!(in_segs, shapes, "{}", name);
                }
                assert!(segs.iter().all(|s| (s.cell as usize) < st.cell_stats.len()), "{}", name);
                let sum = summarize(&st).unwrap();
                assert_eq!(sum.by_count.total(), sum.segments, "{}", name);
                assert!(sum.chained <= sum.segments);
            }
        }
    }

    #[test]
    fn classes_follow_the_box_shares() {
        let cell = (0, 0, 1000, 1000);
        assert_eq!(classify(&(0, 0, 250, 250), &cell), Class::Tile);
        assert_eq!(classify(&(0, 0, 251, 250), &cell), Class::Other);
        assert_eq!(classify(&(0, 0, 501, 10), &cell), Class::Band);
        assert_eq!(classify(&(0, 0, 10, 501), &cell), Class::Band);
        assert_eq!(classify(&(0, 0, 501, 501), &cell), Class::Full);
        assert_eq!(classify(&EMPTY_BOX, &cell), Class::Empty);
        // a degenerate cell side counts as fully covered
        assert_eq!(classify(&(0, 0, 10, 0), &(0, 0, 10, 0)), Class::Full);
        assert_eq!(Quartet::of(&mut vec![5, 1, 3]), Quartet { max: 5, p99: 5, p90: 5, p50: 3 });
    }
}
