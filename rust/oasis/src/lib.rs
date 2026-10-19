//! Record-level OASIS (SEMI P39) stream reader - floe indexer spike S1.
//!
//! Goals that shape the design (and why klayout could not give them to
//! us from Python):
//! - REPETITIONS STAY RECORDS: a 2-billion-member fill array is one
//!   record with member arithmetic, never an expansion.
//! - STREAMING: one forward pass, no in-memory layout database.
//! - The scan inventory (cells / per-layer record+member counts /
//!   texts / placements) is validated against klayout's stored-shape
//!   counts on the same files.
//!
//! CBLOCKs (DEFLATE) are inflated inline; a record never straddles a
//! CBLOCK boundary (spec guarantee), so the record loop simply runs
//! over the inflated bytes with the same modal state.

use std::collections::{HashMap, HashSet};
use std::io::Read;

pub mod doc;
pub mod geom;
pub mod header;
pub mod inventory;
pub mod write;

use crate::geom::{grid_extent, Box4};

#[derive(Debug)]
pub enum OasisError {
    Io(std::io::Error),
    Format(String),
}

impl std::fmt::Display for OasisError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            OasisError::Io(e) => write!(f, "io: {}", e),
            OasisError::Format(s) => write!(f, "format: {}", s),
        }
    }
}

impl From<std::io::Error> for OasisError {
    fn from(e: std::io::Error) -> Self {
        OasisError::Io(e)
    }
}

pub(crate) type Result<T> = std::result::Result<T, OasisError>;

pub(crate) fn err<T>(pos: usize, msg: &str) -> Result<T> {
    Err(OasisError::Format(format!("@{}: {}", pos, msg)))
}

// ---------------------------------------------------------------- cursor

pub(crate) struct Cur<'a> {
    data: &'a [u8],
    pos: usize,
    /// byte offset of `data[0]` in the physical file (for messages)
    base: usize,
}

impl<'a> Cur<'a> {
    pub(crate) fn new(data: &'a [u8], base: usize) -> Self {
        Cur { data, pos: 0, base }
    }

    pub(crate) fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    pub(crate) fn here(&self) -> usize {
        self.base + self.pos
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub(crate) fn byte(&mut self) -> Result<u8> {
        match self.data.get(self.pos) {
            Some(b) => {
                self.pos += 1;
                Ok(*b)
            }
            None => err(self.here(), "unexpected end of stream"),
        }
    }

    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return err(self.here(), "unexpected end of stream (bytes)");
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// base-128 varint, 7 bits per byte, LSB group first
    pub(crate) fn uint(&mut self) -> Result<u64> {
        let mut v: u64 = 0;
        let mut shift = 0u32;
        loop {
            let b = self.byte()?;
            if shift >= 64 || (shift == 63 && (b & 0x7f) > 1) {
                return err(self.here(), "unsigned integer overflow");
            }
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
        }
    }

    /// sign-magnitude: LSB of the assembled varint is the sign
    pub(crate) fn sint(&mut self) -> Result<i64> {
        let u = self.uint()?;
        let mag = (u >> 1) as i64;
        Ok(if u & 1 == 1 { -mag } else { mag })
    }

    pub(crate) fn real(&mut self) -> Result<f64> {
        let t = self.uint()?;
        Ok(match t {
            0 => self.uint()? as f64,
            1 => -(self.uint()? as f64),
            2 => 1.0 / self.uint()? as f64,
            3 => -1.0 / self.uint()? as f64,
            4 => {
                let a = self.uint()? as f64;
                let b = self.uint()? as f64;
                a / b
            }
            5 => {
                let a = self.uint()? as f64;
                let b = self.uint()? as f64;
                -a / b
            }
            6 => f32::from_le_bytes(self.bytes(4)?.try_into().unwrap()) as f64,
            7 => f64::from_le_bytes(self.bytes(8)?.try_into().unwrap()),
            _ => return err(self.here(), "bad real type"),
        })
    }

    pub(crate) fn string(&mut self) -> Result<&'a [u8]> {
        let n = self.uint()? as usize;
        self.bytes(n)
    }

    /// g-delta: single-form octangular or two-form general
    pub(crate) fn g_delta(&mut self) -> Result<(i64, i64)> {
        let g = self.uint()?;
        if g & 1 == 0 {
            let dir = (g >> 1) & 7;
            let m = (g >> 4) as i64;
            Ok(match dir {
                0 => (m, 0),
                1 => (0, m),
                2 => (-m, 0),
                3 => (0, -m),
                4 => (m, m),
                5 => (-m, m),
                6 => (-m, -m),
                7 => (m, -m),
                _ => unreachable!(),
            })
        } else {
            let mag = (g >> 2) as i64;
            let x = if g & 2 == 2 { -mag } else { mag };
            let y = self.sint()?;
            Ok((x, y))
        }
    }
}

// ------------------------------------------------------------- repetition

/// Parsed enough to know the member count and the offset extents;
/// kept as a record. `ex`/`ey` are what `geom::rep_extent` reports for
/// the Rep the geometry parser builds from the same bytes (grids: the
/// corner arithmetic; point lists: min/max over the members, (0, 0)
/// included), so the inventory's boxes equal the Doc's without the
/// members ever being materialized.
#[derive(Clone, Copy, Debug, Default)]
pub struct Repetition {
    pub members: u64,
    pub ex: (i64, i64),
    pub ey: (i64, i64),
}

fn read_repetition(
    c: &mut Cur,
    modal: &mut Option<Repetition>,
    st: &mut ScanStats,
) -> Result<Repetition> {
    let t = c.uint()?;
    *st.rep_types.entry(t).or_default() += 1;
    let rep = match t {
        0 => match modal {
            Some(r) => *r,
            None => return err(c.here(), "repetition reuse before first"),
        },
        1 => {
            let xd = c.uint()?;
            let yd = c.uint()?;
            let dx = c.uint()? as i64;
            let dy = c.uint()? as i64;
            let (ex, ey) = grid_extent(xd + 2, yd + 2, (dx, 0), (0, dy));
            Repetition { members: (xd + 2) * (yd + 2), ex, ey }
        }
        2 => {
            let xd = c.uint()?;
            let dx = c.uint()? as i64;
            let (ex, ey) = grid_extent(xd + 2, 1, (dx, 0), (0, 0));
            Repetition { members: xd + 2, ex, ey }
        }
        3 => {
            let yd = c.uint()?;
            let dy = c.uint()? as i64;
            let (ex, ey) = grid_extent(yd + 2, 1, (0, dy), (0, 0));
            Repetition { members: yd + 2, ex, ey }
        }
        4 | 5 => {
            let xd = c.uint()?;
            let g = if t == 5 { c.uint()? as i64 } else { 1 };
            let (mut x, mut lo, mut hi) = (0i64, 0i64, 0i64);
            for _ in 0..=xd {
                x = x.wrapping_add((c.uint()? as i64).wrapping_mul(g));
                lo = lo.min(x);
                hi = hi.max(x);
            }
            Repetition { members: xd + 2, ex: (lo, hi), ey: (0, 0) }
        }
        6 | 7 => {
            let yd = c.uint()?;
            let g = if t == 7 { c.uint()? as i64 } else { 1 };
            let (mut y, mut lo, mut hi) = (0i64, 0i64, 0i64);
            for _ in 0..=yd {
                y = y.wrapping_add((c.uint()? as i64).wrapping_mul(g));
                lo = lo.min(y);
                hi = hi.max(y);
            }
            Repetition { members: yd + 2, ex: (0, 0), ey: (lo, hi) }
        }
        8 => {
            let nd = c.uint()?;
            let md = c.uint()?;
            let va = c.g_delta()?;
            let vb = c.g_delta()?;
            let (ex, ey) = grid_extent(nd + 2, md + 2, va, vb);
            Repetition { members: (nd + 2) * (md + 2), ex, ey }
        }
        9 => {
            let d = c.uint()?;
            let va = c.g_delta()?;
            let (ex, ey) = grid_extent(d + 2, 1, va, (0, 0));
            Repetition { members: d + 2, ex, ey }
        }
        10 | 11 => {
            let d = c.uint()?;
            let g = if t == 11 { c.uint()? as i64 } else { 1 };
            let (mut x, mut y) = (0i64, 0i64);
            let mut bb: Box4 = (0, 0, 0, 0);
            for _ in 0..=d {
                let (dx, dy) = c.g_delta()?;
                x = x.wrapping_add(dx.wrapping_mul(g));
                y = y.wrapping_add(dy.wrapping_mul(g));
                bb = (bb.0.min(x), bb.1.min(y), bb.2.max(x), bb.3.max(y));
            }
            Repetition { members: d + 2, ex: (bb.0, bb.2), ey: (bb.1, bb.3) }
        }
        _ => return err(c.here(), "bad repetition type"),
    };
    if matches!(t, 4 | 5 | 6 | 7 | 10 | 11) {
        // the geometry parser keeps these as Rep::Pts: one heap block
        // of 16 B per member (a type-0 reuse shares the block)
        st.pts_rep(rep.members);
    }
    *modal = Some(rep);
    Ok(rep)
}

/// point-list: consume it and return the vertex count the geometry
/// parser materializes (doc::read_points: the anchor, one vertex per
/// delta and, for a closed manhattan list, the implicit closing vertex)
/// and the vertices' box relative to the anchor. `spine`, when given,
/// receives the vertices themselves (the inventory needs a path's
/// spine for its exact outline box).
fn read_point_list(
    c: &mut Cur,
    closed: bool,
    mut spine: Option<&mut Vec<(i64, i64)>>,
) -> Result<(u64, Box4)> {
    let t = c.uint()?;
    let n = c.uint()?;
    let mut verts = n + 1;
    let (mut x, mut y) = (0i64, 0i64);
    let mut bb: Box4 = (0, 0, 0, 0);
    if let Some(s) = spine.as_deref_mut() {
        s.clear();
        s.push((0, 0));
    }
    macro_rules! step {
        ($dx:expr, $dy:expr) => {{
            x = x.wrapping_add($dx);
            y = y.wrapping_add($dy);
            bb = (bb.0.min(x), bb.1.min(y), bb.2.max(x), bb.3.max(y));
            if let Some(s) = spine.as_deref_mut() {
                s.push((x, y));
            }
        }};
    }
    match t {
        0 | 1 => {
            let mut horiz = t == 0;
            for _ in 0..n {
                let d = c.sint()?; // 1-delta
                if horiz {
                    step!(d, 0);
                } else {
                    step!(0, d);
                }
                horiz = !horiz;
            }
            if closed && (x != 0 || y != 0) {
                let u = if horiz { (0, y) } else { (x, 0) };
                if u != (x, y) && u != (0, 0) {
                    verts += 1;
                    if let Some(s) = spine.as_deref_mut() {
                        s.push(u);
                    }
                }
            }
        }
        2 => {
            for _ in 0..n {
                let g = c.uint()?; // 2-delta: direction + magnitude
                let m = (g >> 2) as i64;
                let (dx, dy) = match g & 3 {
                    0 => (m, 0),
                    1 => (0, m),
                    2 => (-m, 0),
                    _ => (0, -m),
                };
                step!(dx, dy);
            }
        }
        3 => {
            for _ in 0..n {
                let g = c.uint()?; // 3-delta
                let m = (g >> 3) as i64;
                let (dx, dy) = match g & 7 {
                    0 => (m, 0),
                    1 => (0, m),
                    2 => (-m, 0),
                    3 => (0, -m),
                    4 => (m, m),
                    5 => (-m, m),
                    6 => (-m, -m),
                    _ => (m, -m),
                };
                step!(dx, dy);
            }
        }
        4 => {
            for _ in 0..n {
                let (dx, dy) = c.g_delta()?;
                step!(dx, dy);
            }
        }
        5 => {
            // double-delta: each g-delta adds to the RUNNING delta
            let (mut rx, mut ry) = (0i64, 0i64);
            for _ in 0..n {
                let (dx, dy) = c.g_delta()?;
                rx = rx.wrapping_add(dx);
                ry = ry.wrapping_add(dy);
                step!(rx, ry);
            }
        }
        _ => return err(c.here(), "bad point-list type"),
    }
    Ok((verts, bb))
}

/// a coordinate field: absolute, or a delta on the modal in XYRELATIVE
/// mode (doc.rs coord)
fn coord(c: &mut Cur, modal_v: &mut i64, relative: bool) -> Result<()> {
    let v = c.sint()?;
    *modal_v = if relative { modal_v.wrapping_add(v) } else { v };
    Ok(())
}

// ------------------------------------------------------------ modal state

#[derive(Default)]
struct Modal {
    repetition: Option<Repetition>,
    layer: Option<u64>,
    datatype: Option<u64>,
    textlayer: Option<u64>,
    texttype: Option<u64>,
    /// vertex counts of the modal point lists (a POLYGON / PATH without
    /// the P bit reuses them), the modal CTRAPEZOID's vertex count and
    /// the modal text string's length: the Doc cost of a record that
    /// spells none of them out
    poly_verts: Option<u64>,
    path_verts: Option<u64>,
    ctrap_verts: Option<u64>,
    text_len: u64,
    /// last property had a value list (needed only for stream sync of
    /// PROPERTY-repeat records, which carry no fields at all)
    _last_prop: bool,
    // ---- coordinates and shape modals: the inventory's geometry (the
    // plain scan decodes and ignores them) - doc.rs's modal set: xy
    // mode, separate x/y for geometry, placements and texts, width /
    // height, the point lists, path extensions, the placement target
    relative: bool,
    geo_x: i64,
    geo_y: i64,
    pl_x: i64,
    pl_y: i64,
    tx_x: i64,
    tx_y: i64,
    geo_w: Option<i64>,
    geo_h: Option<i64>,
    circle_r: Option<i64>,
    ctrap_type: Option<u64>,
    /// the modal polygon point list's box relative to the anchor
    poly_box: Option<Box4>,
    /// the modal path point list (vertices relative to the anchor)
    path_pts: Vec<(i64, i64)>,
    path_set: bool,
    path_hw: Option<i64>,
    path_es: Option<i64>,
    path_ee: Option<i64>,
    /// the modal placement target and a counter that changes whenever a
    /// record spells a new one (the inventory's edge cache key)
    place: Option<inventory::Target>,
    place_gen: u64,
    /// the modal text string: None = none yet (doc.rs "text without
    /// string"), Some(None) = an inline string, Some(Some(r)) = a
    /// TEXTSTRING reference
    text_target: Option<Option<u64>>,
}

// ------------------------------------------------------------- scan stats

#[derive(Default, Debug)]
pub struct LayerStat {
    pub records: u64,
    pub members: u64,
}

/// Per-cell inventory in file order: what a cell-wise (streaming) build
/// holds for one cell, and the field question "how big is the biggest
/// cell" - 2026-10-19 a ~50 GB deck source with 5,461 cells died in
/// the syntax parse at 361 GiB of a 376 GiB host.
#[derive(Default, Debug, Clone)]
pub struct CellStat {
    /// CELL name (by-reference cells are named from the CELLNAME table
    /// after the scan; "#<ref>" when the table does not name them)
    pub name: String,
    cell_ref: Option<u64>,
    /// records inside the cell (CBLOCK bodies included)
    pub records: u64,
    pub shapes: u64,
    pub placements: u64,
    pub texts: u64,
    /// polygon / path vertices as the geometry parser materializes them
    pub points: u64,
    /// explicit point-list repetitions spelled out in this cell and
    /// their members
    pub pts_reps: u64,
    pub pts_rep_members: u64,
    /// estimated resident bytes of this cell in the parsed Doc (see
    /// ScanStats::doc_bytes)
    pub doc_bytes: u64,
    pub doc_bytes_pow2: u64,
}

/// in-memory record sizes of the geometry parser (rust/oasis/src/doc.rs)
const RECT_SZ: u64 = std::mem::size_of::<crate::doc::RectRec>() as u64;
const POLY_SZ: u64 = std::mem::size_of::<crate::doc::PolyRec>() as u64;
const PATH_SZ: u64 = std::mem::size_of::<crate::doc::PathRec>() as u64;
const PLACE_SZ: u64 = std::mem::size_of::<crate::doc::PlaceRec>() as u64;
const TEXT_SZ: u64 = std::mem::size_of::<crate::doc::TextRec>() as u64;
const CELL_SZ: u64 = std::mem::size_of::<crate::doc::Cell>() as u64;
/// a materialized vertex or repetition member: (i64, i64)
const PT_SZ: u64 = 16;

/// What the portable bundle's thread-caching allocator
/// (rust/cli/src/tcache.rs: power-of-two classes 16 B .. 32 KiB, larger
/// blocks pass through) makes of a heap block of `bytes`.
fn pow2_block(bytes: u64) -> u64 {
    if bytes == 0 {
        0
    } else if bytes <= 32_768 {
        bytes.max(16).next_power_of_two()
    } else {
        bytes
    }
}

/// CTRAPEZOID vertex count by type (doc::ctrapezoid_points: 16-23 are
/// triangles)
fn ctrap_verts(t: u64) -> u64 {
    if (16..=23).contains(&t) {
        3
    } else {
        4
    }
}

#[derive(Default, Debug)]
pub struct ScanStats {
    pub file_bytes: u64,
    pub cells: u64,
    pub cellnames: u64,
    pub placements: u64,
    pub placement_members: u64,
    pub cblocks: u64,
    pub cblock_bytes_inflated: u64,
    pub records: u64,
    /// record-id -> count (which OASIS constructs a file actually uses)
    pub record_ids: HashMap<u64, u64>,
    /// (layer, datatype) -> geometry record/member counts
    pub shapes: HashMap<(u64, u64), LayerStat>,
    /// (textlayer, texttype) -> text record/member counts
    pub texts: HashMap<(u64, u64), LayerStat>,
    /// repetition-type byte -> count (0 = modal reuse; 1-3/8-9 grids,
    /// 4-7/10-11 point lists) - tells whether a file's density is
    /// grid-encoded or explicit
    pub rep_types: HashMap<u64, u64>,
    pub unit: f64,
    /// polygon + path vertices as the geometry parser materializes them
    /// (16 B each in memory) and the largest single point list
    pub points: u64,
    pub points_max: u64,
    /// explicit point-list repetitions (types 4-7, 10, 11: one heap
    /// block of 16 B per member each; a type-0 reuse shares the block)
    /// and their members
    pub pts_reps: u64,
    pub pts_rep_members: u64,
    pub pts_rep_members_max: u64,
    /// estimated resident bytes of the parsed Doc: every record at its
    /// real size (RectRec / PolyRec / PathRec / PlaceRec / TextRec with
    /// the 56 B Rep inline) plus the per-record heap (vertices,
    /// point-list repetitions, text strings, cell names), before
    /// repetition normalization and without the source buffer.
    /// `doc_bytes_pow2` counts each heap block at the power-of-two class
    /// the portable bundle's thread-caching allocator uses.
    pub doc_bytes: u64,
    pub doc_bytes_pow2: u64,
    /// per-cell inventory in file order (the scan entry points fill it;
    /// the skim passes leave it empty)
    pub cell_stats: Vec<CellStat>,
    track_cells: bool,
    cur_cell: Option<CellStat>,
    /// CELLNAME table in file order: (explicit reference, name) -
    /// implicit (id 3) entries are numbered by position when resolving
    cellnames_seen: Vec<(Option<u64>, String)>,
    /// inventory mode: TEXTSTRING records in file order (explicit
    /// reference or implicit) and the references TEXT records make -
    /// an unresolved one is refused as doc.rs finish_inner refuses it
    textstrings_seen: Vec<Option<u64>>,
    text_refs: HashSet<u64>,
    /// the resolved table (finish): reference -> name, first entry wins
    /// as in the parallel geometry parse's union
    cellname_table: HashMap<u64, String>,
    /// the inventory collector of this parse (None: plain scan); a
    /// parallel scan appends its workers' collectors to `inv_parts` in
    /// file order and `inventory::finish` folds them
    inv: Option<Box<inventory::Part>>,
    inv_parts: Vec<inventory::Part>,
    /// implicit CELLNAME (id 3) / TEXTSTRING (id 5) records seen so far
    /// outside CBLOCKs: a cell range's parser seeds, as `cell_cuts`
    /// counts them (a parallel worker starts from its cut's bases)
    n3: u64,
    n5: u64,
    cblock_depth: u32,
    /// file offset of the END record (the last cell range's end)
    end_at: u64,
}

impl ScanStats {
    /// the scan entry points track cells; the skim pass (cell_cuts) runs
    /// on a default ScanStats and does not.
    /// `inventory`: with the per-cell geometry collector (inventory.rs)
    fn tracking_with(inventory: bool) -> ScanStats {
        ScanStats {
            track_cells: true,
            inv: inventory.then(|| Box::new(inventory::Part::new())),
            ..Default::default()
        }
    }

    fn open_inv_cell(&mut self, key: inventory::Target, at: u64) {
        let (n3, n5, depth) = (self.n3, self.n5, self.cblock_depth);
        if let Some(p) = self.inv.as_deref_mut() {
            p.open_cell(key, at, depth > 0, n3, n5);
        }
    }

    fn close_inv(&mut self, end: u64) {
        if let Some(p) = self.inv.as_deref_mut() {
            p.close(end);
        }
    }

    /// inventory mode refuses what doc.rs utf8() refuses
    fn check_utf8(&self, c: &Cur, b: &[u8]) -> Result<()> {
        if self.inv.is_some() && std::str::from_utf8(b).is_err() {
            return Err(OasisError::Format(format!(
                "@{}: non-utf8 name",
                c.here()
            )));
        }
        Ok(())
    }

    /// a name as the geometry parser keeps it: the inventory refuses
    /// what doc.rs refuses (non-UTF-8), the plain scan keeps a lossy copy
    fn name_string(&self, c: &Cur, b: &[u8]) -> Result<String> {
        if self.inv.is_some() {
            String::from_utf8(b.to_vec()).map_err(|_| {
                OasisError::Format(format!("@{}: non-utf8 name", c.here()))
            })
        } else {
            Ok(String::from_utf8_lossy(b).into_owned())
        }
    }

    /// one parsed record's share of the in-memory Doc: `fixed` bytes
    /// inline in its cell's record vector, `heap` bytes in its own block
    fn cost(&mut self, fixed: u64, heap: u64) {
        self.doc_bytes += fixed + heap;
        self.doc_bytes_pow2 += fixed + pow2_block(heap);
        if let Some(cs) = self.cur_cell.as_mut() {
            cs.doc_bytes += fixed + heap;
            cs.doc_bytes_pow2 += fixed + pow2_block(heap);
        }
    }

    fn pts_rep(&mut self, members: u64) {
        self.pts_reps += 1;
        self.pts_rep_members += members;
        self.pts_rep_members_max = self.pts_rep_members_max.max(members);
        if let Some(cs) = self.cur_cell.as_mut() {
            cs.pts_reps += 1;
            cs.pts_rep_members += members;
        }
        // Rep::Pts(Arc<[(i64, i64)]>): the two counters and the members
        self.cost(0, 16 + PT_SZ * members);
    }

    fn points(&mut self, verts: u64) {
        self.points += verts;
        self.points_max = self.points_max.max(verts);
        if let Some(cs) = self.cur_cell.as_mut() {
            cs.points += verts;
        }
    }

    fn open_cell(&mut self, name: String, cell_ref: Option<u64>) {
        if !self.track_cells {
            return;
        }
        self.close_cell();
        self.cur_cell = Some(CellStat {
            name,
            cell_ref,
            ..Default::default()
        });
        self.cost(CELL_SZ, 0);
    }

    fn close_cell(&mut self) {
        if let Some(cs) = self.cur_cell.take() {
            self.cell_stats.push(cs);
        }
    }

    /// after the last record (of the whole file - a parallel scan calls
    /// this once on the merged stats): name the by-reference cells from
    /// the CELLNAME table (implicit entries numbered by position, as the
    /// geometry parser does) and charge each cell its name string
    fn finish(&mut self) {
        self.close_cell();
        if !self.track_cells {
            return;
        }
        let mut names: HashMap<u64, String> = HashMap::new();
        let mut implicit = 0u64;
        for (r, name) in std::mem::take(&mut self.cellnames_seen) {
            let r = match r {
                Some(r) => r,
                None => {
                    implicit += 1;
                    implicit - 1
                }
            };
            names.entry(r).or_insert(name);
        }
        let (mut extra, mut extra2) = (0u64, 0u64);
        for cs in self.cell_stats.iter_mut() {
            if let Some(r) = cs.cell_ref {
                cs.name = match names.get(&r) {
                    Some(n) => n.clone(),
                    None => format!("#{}", r),
                };
            }
            let n = cs.name.len() as u64;
            cs.doc_bytes += n;
            cs.doc_bytes_pow2 += pow2_block(n);
            extra += n;
            extra2 += pow2_block(n);
        }
        self.doc_bytes += extra;
        self.doc_bytes_pow2 += extra2;
        self.cellname_table = names;
    }

    fn shape(&mut self, m: &Modal, c: &Cur, members: u64) -> Result<()> {
        let (l, d) = match (m.layer, m.datatype) {
            (Some(l), Some(d)) => (l, d),
            _ => return err(c.here(), "shape before layer/datatype modal"),
        };
        let e = self.shapes.entry((l, d)).or_default();
        e.records += 1;
        e.members += members;
        if let Some(cs) = self.cur_cell.as_mut() {
            cs.shapes += 1;
        }
        Ok(())
    }

    fn text(&mut self, m: &Modal, c: &Cur, members: u64) -> Result<()> {
        let (l, d) = match (m.textlayer, m.texttype) {
            (Some(l), Some(d)) => (l, d),
            _ => return err(c.here(), "text before textlayer/texttype modal"),
        };
        let e = self.texts.entry((l, d)).or_default();
        e.records += 1;
        e.members += members;
        if let Some(cs) = self.cur_cell.as_mut() {
            cs.texts += 1;
        }
        Ok(())
    }
}

// ----------------------------------------------------------- record layer

/// the record's repetition (members + extents), or the single member
fn rep_of(
    c: &mut Cur,
    modal: &mut Modal,
    st: &mut ScanStats,
    has_rep: bool,
) -> Result<Repetition> {
    if has_rep {
        read_repetition(c, &mut modal.repetition, st)
    } else {
        Ok(Repetition { members: 1, ex: (0, 0), ey: (0, 0) })
    }
}

/// the (layer, datatype) the geometry parser keys a shape by (checked
/// present by ScanStats::shape before this is called)
fn shape_layer(modal: &Modal) -> (u32, u32) {
    (
        modal.layer.unwrap_or(0) as u32,
        modal.datatype.unwrap_or(0) as u32,
    )
}

fn read_property_value(c: &mut Cur) -> Result<()> {
    let t = c.uint()?;
    match t {
        0..=5 => {
            // real forms sharing the type code
            match t {
                0 | 1 | 2 | 3 => {
                    c.uint()?;
                }
                4 | 5 => {
                    c.uint()?;
                    c.uint()?;
                }
                _ => unreachable!(),
            }
        }
        6 => {
            c.bytes(4)?;
        }
        7 => {
            c.bytes(8)?;
        }
        8 => {
            c.uint()?;
        }
        9 => {
            c.sint()?;
        }
        10 | 11 | 12 => {
            c.string()?;
        }
        13 | 14 | 15 => {
            c.uint()?; // propstring reference
        }
        _ => return err(c.here(), "bad property value type"),
    }
    Ok(())
}

#[derive(PartialEq)]
enum Rec {
    Normal,
    Cell,
    End,
}

/// Parse one record. `inflate=false` skips CBLOCK bodies without
/// decompressing (used by the parallel skim pass, which only needs
/// record boundaries - field consumption is modal-independent). With
/// an inventory collector on `st` the coordinates are followed as
/// doc.rs follows them and the records the geometry parser refuses
/// are refused here too.
fn record(
    c: &mut Cur,
    modal: &mut Modal,
    st: &mut ScanStats,
    inflate: bool,
) -> Result<Rec> {
    let at = c.here() as u64;
    let id = c.uint()?;
    if id == 2 {
        st.end_at = at;
        return Ok(Rec::End); // END: not counted (parallel workers
    }                        // never see it - keep both paths equal)
    st.records += 1;
    *st.record_ids.entry(id).or_default() += 1;
    match id {
        0 => {} // PAD
        1 => {
            // START: version, unit, offset-flag (+ table when flag==0)
            c.string()?;
            st.unit = c.real()?;
            if c.uint()? == 0 {
                for _ in 0..12 {
                    c.uint()?;
                }
            }
        }
        3 | 4 => {
            // CELLNAME
            let name = c.string()?;
            let r = if id == 4 { Some(c.uint()?) } else { None };
            if id == 3 && st.cblock_depth == 0 {
                st.n3 += 1;
            }
            if st.track_cells {
                let s = st.name_string(c, name)?;
                st.cellnames_seen.push((r, s));
            }
            st.cellnames += 1;
        }
        5 | 6 => {
            // TEXTSTRING
            let s = c.string()?;
            let r = if id == 6 { Some(c.uint()?) } else { None };
            if id == 5 && st.cblock_depth == 0 {
                st.n5 += 1;
            }
            if st.inv.is_some() {
                st.check_utf8(c, s)?;
                st.textstrings_seen.push(r);
            }
        }
        7 | 8 | 9 | 10 => {
            c.string()?;
            if id == 8 || id == 10 {
                c.uint()?;
            }
        }
        11 | 12 => {
            // LAYERNAME: name + layer interval + datatype interval; an
            // exact pair joins the layer order (doc.rs reg_layer) and
            // every rule names layers
            let name = c.string()?;
            let layer = crate::doc::name_interval(c)?;
            let datatype = crate::doc::name_interval(c)?;
            if st.inv.is_some() {
                let name = st.name_string(c, name)?;
                let part = st.inv.as_deref_mut().unwrap();
                if let (Some(l), Some(d)) = (layer.exact(), datatype.exact()) {
                    part.reg_layer(l, d);
                }
                part.rules.push(crate::doc::LayerNameRule {
                    name,
                    layer,
                    datatype,
                });
            }
        }
        13 => {
            let r = c.uint()?;
            st.cells += 1;
            *modal = Modal::default();
            st.open_cell(String::new(), Some(r));
            st.open_inv_cell(inventory::Target::Ref(r), at);
            return Ok(Rec::Cell);
        }
        14 => {
            let name = c.string()?;
            st.cells += 1;
            *modal = Modal::default();
            let s = st.name_string(c, name)?;
            if st.inv.is_some() {
                st.open_cell(s.clone(), None);
                st.open_inv_cell(inventory::Target::Name(s), at);
            } else {
                st.open_cell(s, None);
            }
            return Ok(Rec::Cell);
        }
        15 => modal.relative = false, // XYABSOLUTE
        16 => modal.relative = true,  // XYRELATIVE
        17 | 18 => {
            // PLACEMENT: C N X Y R AA F
            let info = c.byte()?;
            let (cbit, nbit) = (info & 0x80 != 0, info & 0x40 != 0);
            let (xbit, ybit) = (info & 0x20 != 0, info & 0x10 != 0);
            let rbit = info & 0x08 != 0;
            if cbit {
                let tgt = if nbit {
                    let r = c.uint()?;
                    st.inv.is_some().then_some(inventory::Target::Ref(r))
                } else {
                    let s = c.string()?;
                    if st.inv.is_some() {
                        Some(inventory::Target::Name(st.name_string(c, s)?))
                    } else {
                        None
                    }
                };
                if let Some(t) = tgt {
                    modal.place = Some(t);
                    modal.place_gen += 1;
                }
            }
            if id == 18 {
                if st.inv.is_some() {
                    return err(
                        c.here(),
                        "magnified/angled placement: out of spike scope",
                    );
                }
                if info & 0x04 != 0 {
                    c.real()?; // magnification
                }
                if info & 0x02 != 0 {
                    c.real()?; // angle
                }
            }
            if xbit {
                coord(c, &mut modal.pl_x, modal.relative)?;
            }
            if ybit {
                coord(c, &mut modal.pl_y, modal.relative)?;
            }
            let rep = rep_of(c, modal, st, rbit)?;
            st.placements += 1;
            st.placement_members += rep.members;
            st.cost(PLACE_SZ, 0);
            if let Some(cs) = st.cur_cell.as_mut() {
                cs.placements += 1;
            }
            if let Some(part) = st.inv.as_deref_mut() {
                let tgt = match modal.place.as_ref() {
                    Some(t) => t,
                    None => return err(c.here(), "placement without cell"),
                };
                part.place(
                    c.here(),
                    tgt,
                    modal.place_gen,
                    modal.pl_x,
                    modal.pl_y,
                    (info >> 1) & 3,
                    info & 0x01 != 0,
                    &rep,
                )?;
            }
        }
        19 => {
            // TEXT: 0CNXYRTL
            let info = c.byte()?;
            let (cbit, nbit) = (info & 0x40 != 0, info & 0x20 != 0);
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (tbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            if cbit {
                if nbit {
                    let r = c.uint()?;
                    modal.text_target = Some(Some(r));
                    modal.text_len = 0; // table string: not costed
                } else {
                    let s = c.string()?;
                    st.check_utf8(c, s)?;
                    modal.text_target = Some(None);
                    modal.text_len = s.len() as u64;
                }
            }
            if lbit {
                modal.textlayer = Some(c.uint()?);
            }
            if tbit {
                modal.texttype = Some(c.uint()?);
            }
            if xbit {
                coord(c, &mut modal.tx_x, modal.relative)?;
            }
            if ybit {
                coord(c, &mut modal.tx_y, modal.relative)?;
            }
            let rep = rep_of(c, modal, st, rbit)?;
            st.text(modal, c, rep.members)?;
            st.cost(TEXT_SZ, modal.text_len);
            if st.inv.is_some() {
                match modal.text_target {
                    None => return err(c.here(), "text without string"),
                    Some(Some(r)) => {
                        st.text_refs.insert(r);
                    }
                    Some(None) => {}
                }
            }
            if let Some(part) = st.inv.as_deref_mut() {
                let (l, d) = (
                    modal.textlayer.unwrap_or(0) as u32,
                    modal.texttype.unwrap_or(0) as u32,
                );
                let li = part.reg_layer(l, d);
                part.text(
                    c.here(),
                    li,
                    inventory::rep_box(modal.tx_x, modal.tx_y, modal.tx_x, modal.tx_y, &rep),
                )?;
            }
        }
        20 => {
            // RECTANGLE: SWHXYRDL
            let info = c.byte()?;
            let sbit = info & 0x80 != 0;
            let (wbit, hbit) = (info & 0x40 != 0, info & 0x20 != 0);
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (dbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            if lbit {
                modal.layer = Some(c.uint()?);
            }
            if dbit {
                modal.datatype = Some(c.uint()?);
            }
            if wbit {
                modal.geo_w = Some(c.uint()? as i64);
            }
            if hbit {
                modal.geo_h = Some(c.uint()? as i64);
            }
            if sbit {
                // S with H set is spec-invalid; doc.rs reads the H field
                // and then takes h = w, and so does this parser
                modal.geo_h = modal.geo_w;
            }
            if xbit {
                coord(c, &mut modal.geo_x, modal.relative)?;
            }
            if ybit {
                coord(c, &mut modal.geo_y, modal.relative)?;
            }
            let rep = rep_of(c, modal, st, rbit)?;
            st.shape(modal, c, rep.members)?;
            st.cost(RECT_SZ, 0);
            if let Some(part) = st.inv.as_deref_mut() {
                let w = match modal.geo_w {
                    Some(w) => w,
                    None => return Err(OasisError::Format("no width".into())),
                };
                let h = match modal.geo_h {
                    Some(h) => h,
                    None => return Err(OasisError::Format("no height".into())),
                };
                let (l, d) = shape_layer(modal);
                let li = part.reg_layer(l, d);
                part.shape(
                    c.here(),
                    li,
                    rep.members,
                    inventory::rep_box(
                        modal.geo_x,
                        modal.geo_y,
                        modal.geo_x + w,
                        modal.geo_y + h,
                        &rep,
                    ),
                )?;
            }
        }
        21 => {
            // POLYGON: 00PXYRDL
            let info = c.byte()?;
            let pbit = info & 0x20 != 0;
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (dbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            if lbit {
                modal.layer = Some(c.uint()?);
            }
            if dbit {
                modal.datatype = Some(c.uint()?);
            }
            if pbit {
                let (verts, bb) = read_point_list(c, true, None)?;
                modal.poly_verts = Some(verts);
                modal.poly_box = Some(bb);
            }
            if xbit {
                coord(c, &mut modal.geo_x, modal.relative)?;
            }
            if ybit {
                coord(c, &mut modal.geo_y, modal.relative)?;
            }
            let rep = rep_of(c, modal, st, rbit)?;
            st.shape(modal, c, rep.members)?;
            let verts = modal.poly_verts.unwrap_or(0);
            st.points(verts);
            st.cost(POLY_SZ, PT_SZ * verts);
            if let Some(part) = st.inv.as_deref_mut() {
                let pb = match modal.poly_box {
                    Some(b) => b,
                    None => return err(c.here(), "poly without points"),
                };
                let (l, d) = shape_layer(modal);
                let li = part.reg_layer(l, d);
                part.shape(
                    c.here(),
                    li,
                    rep.members,
                    inventory::rep_box(
                        modal.geo_x + pb.0,
                        modal.geo_y + pb.1,
                        modal.geo_x + pb.2,
                        modal.geo_y + pb.3,
                        &rep,
                    ),
                )?;
            }
        }
        22 => {
            // PATH: EWPXYRDL
            let info = c.byte()?;
            let ebit = info & 0x80 != 0;
            let wbit = info & 0x40 != 0;
            let pbit = info & 0x20 != 0;
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (dbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            if lbit {
                modal.layer = Some(c.uint()?);
            }
            if dbit {
                modal.datatype = Some(c.uint()?);
            }
            if wbit {
                modal.path_hw = Some(c.uint()? as i64);
            }
            if ebit {
                // extension scheme (doc.rs): 0 = reuse the modal, 1 =
                // flush, 2 = half-width, 3 = explicit
                let scheme = c.uint()?;
                let hw = modal.path_hw;
                modal.path_es = match (scheme >> 2) & 3 {
                    0 => modal.path_es,
                    1 => Some(0),
                    2 => hw,
                    _ => Some(c.sint()?),
                };
                modal.path_ee = match scheme & 3 {
                    0 => modal.path_ee,
                    1 => Some(0),
                    2 => hw,
                    _ => Some(c.sint()?),
                };
            }
            if pbit {
                let spine = if st.inv.is_some() {
                    Some(&mut modal.path_pts)
                } else {
                    None
                };
                let (verts, _) = read_point_list(c, false, spine)?;
                modal.path_verts = Some(verts);
                modal.path_set = true;
            }
            if xbit {
                coord(c, &mut modal.geo_x, modal.relative)?;
            }
            if ybit {
                coord(c, &mut modal.geo_y, modal.relative)?;
            }
            let rep = rep_of(c, modal, st, rbit)?;
            st.shape(modal, c, rep.members)?;
            let verts = modal.path_verts.unwrap_or(0);
            st.points(verts);
            st.cost(PATH_SZ, PT_SZ * verts);
            if let Some(part) = st.inv.as_deref_mut() {
                let hw = match modal.path_hw {
                    Some(v) => v,
                    None => return Err(OasisError::Format("no halfwidth".into())),
                };
                let es = match modal.path_es {
                    Some(v) => v,
                    None => return Err(OasisError::Format("no start extension".into())),
                };
                let ee = match modal.path_ee {
                    Some(v) => v,
                    None => return Err(OasisError::Format("no end extension".into())),
                };
                if !modal.path_set {
                    return err(c.here(), "path without points");
                }
                let (l, d) = shape_layer(modal);
                let li = part.reg_layer(l, d);
                let b4 = part.path_box(&modal.path_pts, modal.geo_x, modal.geo_y, hw, es, ee);
                part.shape(
                    c.here(),
                    li,
                    rep.members,
                    inventory::rep_box(b4.0, b4.1, b4.2, b4.3, &rep),
                )?;
            }
        }
        23 | 24 | 25 => {
            // TRAPEZOID: OWHXYRDL (+delta-a / delta-b)
            let info = c.byte()?;
            let (wbit, hbit) = (info & 0x40 != 0, info & 0x20 != 0);
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (dbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            if lbit {
                modal.layer = Some(c.uint()?);
            }
            if dbit {
                modal.datatype = Some(c.uint()?);
            }
            if wbit {
                modal.geo_w = Some(c.uint()? as i64);
            }
            if hbit {
                modal.geo_h = Some(c.uint()? as i64);
            }
            let da = if id == 25 { 0 } else { c.sint()? };
            let db = if id == 24 { 0 } else { c.sint()? };
            if xbit {
                coord(c, &mut modal.geo_x, modal.relative)?;
            }
            if ybit {
                coord(c, &mut modal.geo_y, modal.relative)?;
            }
            let rep = rep_of(c, modal, st, rbit)?;
            st.shape(modal, c, rep.members)?;
            // materialized as a four-vertex PolyRec
            st.points(4);
            st.cost(POLY_SZ, PT_SZ * 4);
            if let Some(part) = st.inv.as_deref_mut() {
                let w = match modal.geo_w {
                    Some(w) => w,
                    None => return Err(OasisError::Format("no width".into())),
                };
                let h = match modal.geo_h {
                    Some(h) => h,
                    None => return Err(OasisError::Format("no height".into())),
                };
                let (l, d) = shape_layer(modal);
                // the layer registers even when the shape degenerates
                let li = part.reg_layer(l, d);
                let pts = crate::doc::dedupe_ring(crate::doc::trapezoid_points(
                    info & 0x80 != 0,
                    w,
                    h,
                    da,
                    db,
                ));
                if pts.len() >= 3 {
                    part.shape(
                        c.here(),
                        li,
                        rep.members,
                        inventory::rep_box_pts(&pts, modal.geo_x, modal.geo_y, &rep),
                    )?;
                }
            }
        }
        26 => {
            // CTRAPEZOID: TWHXYRDL
            let info = c.byte()?;
            let tbit = info & 0x80 != 0;
            let (wbit, hbit) = (info & 0x40 != 0, info & 0x20 != 0);
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (dbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            if lbit {
                modal.layer = Some(c.uint()?);
            }
            if dbit {
                modal.datatype = Some(c.uint()?);
            }
            if tbit {
                let t = c.uint()?;
                modal.ctrap_type = Some(t);
                modal.ctrap_verts = Some(ctrap_verts(t));
            }
            if wbit {
                modal.geo_w = Some(c.uint()? as i64);
            }
            if hbit {
                modal.geo_h = Some(c.uint()? as i64);
            }
            if xbit {
                coord(c, &mut modal.geo_x, modal.relative)?;
            }
            if ybit {
                coord(c, &mut modal.geo_y, modal.relative)?;
            }
            let rep = rep_of(c, modal, st, rbit)?;
            st.shape(modal, c, rep.members)?;
            let verts = modal.ctrap_verts.unwrap_or(4);
            st.points(verts);
            st.cost(POLY_SZ, PT_SZ * verts);
            if let Some(part) = st.inv.as_deref_mut() {
                let t = match modal.ctrap_type {
                    Some(t) => t,
                    None => return Err(OasisError::Format("ctrapezoid without type".into())),
                };
                // the types that imply one dimension read only the other
                let w = match t {
                    20 | 21 => modal.geo_w.unwrap_or(0),
                    _ => match modal.geo_w {
                        Some(w) => w,
                        None => return Err(OasisError::Format("no width".into())),
                    },
                };
                let h = match t {
                    16..=19 | 22 | 23 | 25 => modal.geo_h.unwrap_or(0),
                    _ => match modal.geo_h {
                        Some(h) => h,
                        None => return Err(OasisError::Format("no height".into())),
                    },
                };
                let Some((pts, iw, ih)) = crate::doc::ctrapezoid_points(t, w, h) else {
                    return err(
                        c.here(),
                        &format!("CTRAPEZOID type {} is outside 0..=25", t),
                    );
                };
                // the implied dimensions feed the modals (a following
                // record without W/H sees them)
                if let Some(iw) = iw {
                    modal.geo_w = Some(iw);
                }
                if let Some(ih) = ih {
                    modal.geo_h = Some(ih);
                }
                let (l, d) = shape_layer(modal);
                let li = part.reg_layer(l, d);
                let pts = crate::doc::dedupe_ring(pts);
                if pts.len() >= 3 {
                    part.shape(
                        c.here(),
                        li,
                        rep.members,
                        inventory::rep_box_pts(&pts, modal.geo_x, modal.geo_y, &rep),
                    )?;
                }
            }
        }
        27 => {
            // CIRCLE: 00rXYRDL
            let info = c.byte()?;
            let radbit = info & 0x20 != 0;
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (dbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            if lbit {
                modal.layer = Some(c.uint()?);
            }
            if dbit {
                modal.datatype = Some(c.uint()?);
            }
            if radbit {
                let r = c.uint()?;
                modal.circle_r = Some(match i64::try_from(r) {
                    Ok(v) => v,
                    Err(_) => return err(c.here(), "circle radius out of range"),
                });
            }
            if xbit {
                coord(c, &mut modal.geo_x, modal.relative)?;
            }
            if ybit {
                coord(c, &mut modal.geo_y, modal.relative)?;
            }
            let rep = rep_of(c, modal, st, rbit)?;
            st.shape(modal, c, rep.members)?;
            // materialized as an inscribed 64-gon PolyRec
            st.points(64);
            st.cost(POLY_SZ, PT_SZ * 64);
            if let Some(part) = st.inv.as_deref_mut() {
                let r = match modal.circle_r {
                    Some(r) => r,
                    None => return err(c.here(), "circle without radius"),
                };
                // r == 0 defines no geometry and registers no layer
                if r > 0 {
                    let (l, d) = shape_layer(modal);
                    let li = part.reg_layer(l, d);
                    let pts = crate::doc::circle64(modal.geo_x, modal.geo_y, r);
                    part.shape(
                        c.here(),
                        li,
                        rep.members,
                        inventory::rep_box_pts(&pts, 0, 0, &rep),
                    )?;
                }
            }
        }
        28 => {
            // PROPERTY: UUUUVCNS
            let info = c.byte()?;
            let uuuu = (info >> 4) & 0x0f;
            let vbit = info & 0x08 != 0;
            let cbit = info & 0x04 != 0;
            let nbit = info & 0x02 != 0;
            let sbit = info & 0x01 != 0;
            let _ = sbit;
            if cbit {
                if nbit {
                    c.uint()?;
                } else {
                    c.string()?;
                }
            }
            if !vbit {
                let n = if uuuu == 15 { c.uint()? } else { uuuu as u64 };
                for _ in 0..n {
                    read_property_value(c)?;
                }
            }
        }
        29 => {} // PROPERTY repeat-last
        30 | 31 => {
            // XNAME: attribute + string (+ref)
            c.uint()?;
            c.string()?;
            if id == 31 {
                c.uint()?;
            }
        }
        32 => {
            // XELEMENT: attribute + b-string
            c.uint()?;
            c.string()?;
        }
        33 => {
            // XGEOMETRY: 000XYRDL, attribute, b-string
            if st.inv.is_some() {
                return err(c.here(), "XGEOMETRY: out of spike scope");
            }
            let info = c.byte()?;
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (dbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            c.uint()?; // attribute
            if lbit {
                modal.layer = Some(c.uint()?);
            }
            if dbit {
                modal.datatype = Some(c.uint()?);
            }
            c.string()?;
            if xbit {
                coord(c, &mut modal.geo_x, modal.relative)?;
            }
            if ybit {
                coord(c, &mut modal.geo_y, modal.relative)?;
            }
            let rep = rep_of(c, modal, st, rbit)?;
            st.shape(modal, c, rep.members)?;
        }
        34 => {
            // CBLOCK: comp-type, uncomp-count, comp-count, bytes
            let ctype = c.uint()?;
            let un = c.uint()? as usize;
            let cn = c.uint()? as usize;
            let comp = c.bytes(cn)?;
            if ctype != 0 {
                return err(c.here(), "unknown CBLOCK compression");
            }
            if inflate {
                let mut out = Vec::with_capacity(un);
                flate2::read::DeflateDecoder::new(comp)
                    .read_to_end(&mut out)
                    .map_err(OasisError::Io)?;
                if out.len() != un {
                    return err(c.here(), "CBLOCK inflate size mismatch");
                }
                st.cblocks += 1;
                st.cblock_bytes_inflated += un as u64;
                st.cblock_depth += 1;
                let mut sub = Cur::new(&out, c.here());
                while !sub.at_end() {
                    if record(&mut sub, modal, st, true)? == Rec::End {
                        return err(sub.here(), "END inside CBLOCK");
                    }
                }
                st.cblock_depth -= 1;
            }
        }
        _ => return err(c.here(), &format!("unknown record id {}", id)),
    }
    if let Some(cs) = st.cur_cell.as_mut() {
        cs.records += 1;
    }
    Ok(Rec::Normal)
}

const MAGIC: &[u8] = b"%SEMI-OASIS\r\n";

pub fn scan(data: &[u8]) -> Result<ScanStats> {
    scan_with(data, 1, false)
}

/// Parallel scan: a cheap skim pass (CBLOCK bodies skipped, not
/// inflated) splits the stream at CELL boundaries - modal state resets
/// there, so each contiguous cell-group parses independently. Workers
/// then run the full parser (inflating) over their byte ranges. No
/// GIL, no shared state: the whole file's parse work spreads over
/// `jobs` OS threads.
pub fn scan_parallel(data: &[u8], jobs: usize) -> Result<ScanStats> {
    scan_with(data, jobs, false)
}

/// The streaming build's first pass (docs/STREAMING_INDEX_PLAN.ko.md
/// §3, S1): the scan with the per-cell geometry collector on, folded
/// into the Inventory (inventory.rs: cell table, layer order, per-cell
/// boxes / masks / edges, the hierarchy fold).
pub fn inventory(data: &[u8], jobs: usize) -> Result<inventory::Inventory> {
    let st = scan_with(data, jobs, true)?;
    inventory::finish(st)
}

/// The scan frame: one record walk over the whole file, CBLOCKs
/// inflated. `jobs > 1`: the `cell_cuts` skim finds the CELL
/// boundaries, the head (tables before the first cell) parses inline,
/// workers run the full parser over contiguous cell groups (the same
/// grouping as the parallel geometry parse) and the results merge in
/// file order. `inventory` attaches the per-cell geometry collector.
fn scan_with(data: &[u8], jobs: usize, inventory: bool) -> Result<ScanStats> {
    if data.len() < MAGIC.len() || &data[..MAGIC.len()] != MAGIC {
        return err(0, "not an OASIS file (bad magic)");
    }
    let body = &data[MAGIC.len()..];
    if jobs <= 1 {
        let mut c = Cur::new(body, MAGIC.len());
        let mut st = ScanStats {
            file_bytes: data.len() as u64,
            ..ScanStats::tracking_with(inventory)
        };
        let mut modal = Modal::default();
        while !c.at_end() {
            if record(&mut c, &mut modal, &mut st, true)? == Rec::End {
                let end = st.end_at;
                st.close_inv(end);
                st.finish();
                return Ok(st); // END reached
            }
        }
        return err(c.here(), "stream ended without END record");
    }
    let (head_end, cuts, end_at) = cell_cuts(body, MAGIC.len())?;
    let mut st = ScanStats {
        file_bytes: data.len() as u64,
        ..ScanStats::tracking_with(inventory)
    };
    {
        let mut hc = Cur::new(&body[..head_end], MAGIC.len());
        let mut hm = Modal::default();
        while !hc.at_end() {
            record(&mut hc, &mut hm, &mut st, true)?;
        }
    }
    // a CELL record inside a CBLOCK ahead of the first visible one (the
    // skim does not inflate, so it belongs to the head) is still open
    // here: close it before the workers' cells merge behind it, or its
    // CellStat would land last while its inventory part comes first
    st.close_cell();
    let file_end = (MAGIC.len() + end_at) as u64;
    let n_units = cuts.len();
    if n_units > 0 {
        let per = n_units.div_ceil(jobs).max(1);
        let groups: Vec<((usize, u64, u64), usize)> = (0..n_units)
            .step_by(per)
            .map(|i| {
                let end = if i + per < n_units {
                    cuts[i + per].0
                } else {
                    end_at
                };
                (cuts[i], end)
            })
            .collect();
        let results: Vec<Result<ScanStats>> = std::thread::scope(|s| {
            let handles: Vec<_> = groups
                .iter()
                .map(|&((a, n3, n5), b)| {
                    s.spawn(move || {
                        let mut wc = Cur::new(&body[a..b], MAGIC.len() + a);
                        let mut wm = Modal::default();
                        let mut ws = ScanStats::tracking_with(inventory);
                        ws.n3 = n3;
                        ws.n5 = n5;
                        while !wc.at_end() {
                            if record(&mut wc, &mut wm, &mut ws, true)?
                                == Rec::End
                            {
                                return err(wc.here(), "END inside group");
                            }
                        }
                        ws.close_cell();
                        ws.close_inv((MAGIC.len() + b) as u64);
                        Ok(ws)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for r in results {
            merge(&mut st, r?);
        }
    }
    st.end_at = file_end;
    st.close_inv(file_end);
    st.finish();
    Ok(st)
}

fn merge(into: &mut ScanStats, mut other: ScanStats) {
    if let Some(p) = other.inv.take() {
        into.inv_parts.push(*p);
    }
    into.inv_parts.append(&mut other.inv_parts);
    into.points += other.points;
    into.points_max = into.points_max.max(other.points_max);
    into.pts_reps += other.pts_reps;
    into.pts_rep_members += other.pts_rep_members;
    into.pts_rep_members_max =
        into.pts_rep_members_max.max(other.pts_rep_members_max);
    into.doc_bytes += other.doc_bytes;
    into.doc_bytes_pow2 += other.doc_bytes_pow2;
    into.cell_stats.extend(other.cell_stats);
    into.cellnames_seen.extend(other.cellnames_seen);
    into.textstrings_seen.extend(other.textstrings_seen);
    into.text_refs.extend(other.text_refs);
    into.cells += other.cells;
    into.cellnames += other.cellnames;
    into.placements += other.placements;
    into.placement_members += other.placement_members;
    into.cblocks += other.cblocks;
    into.cblock_bytes_inflated += other.cblock_bytes_inflated;
    into.records += other.records;
    for (k, v) in other.shapes {
        let e = into.shapes.entry(k).or_default();
        e.records += v.records;
        e.members += v.members;
    }
    for (k, v) in other.texts {
        let e = into.texts.entry(k).or_default();
        e.records += v.records;
        e.members += v.members;
    }
    for (k, v) in other.record_ids {
        *into.record_ids.entry(k).or_default() += v;
    }
    for (k, v) in other.rep_types {
        *into.rep_types.entry(k).or_default() += v;
    }
}

/// Skim pass shared by the parallel scan and the parallel doc parse:
/// CELL record offsets (CBLOCK bodies skipped, not inflated) plus,
/// per cut, how many IMPLICIT name-table records (CELLNAME id 3,
/// TEXTSTRING id 5) precede it - chunk parsers seed their implicit
/// refnum counters from that prefix so global numbering survives the
/// split. Assumes name tables sit outside CBLOCKs (klayout does).
/// Returns (head_end, [(cut_offset, cellname_base, textstring_base)],
/// end_offset).
pub(crate) fn cell_cuts(
    body: &[u8],
    file_off: usize,
) -> Result<(usize, Vec<(usize, u64, u64)>, usize)> {
    let mut c = Cur::new(body, file_off);
    let mut st = ScanStats::default(); // discarded
    let mut modal = Modal::default();
    let mut head_end = None;
    let mut cuts: Vec<(usize, u64, u64)> = Vec::new();
    let (mut n3, mut n5) = (0u64, 0u64);
    loop {
        if c.at_end() {
            return err(c.here(), "stream ended without END record");
        }
        let at = c.pos;
        let id = {
            let mut pc = Cur::new(&body[at..], 0);
            pc.uint()?
        };
        match record(&mut c, &mut modal, &mut st, false)? {
            Rec::Cell => {
                if head_end.is_none() {
                    head_end = Some(at);
                }
                cuts.push((at, n3, n5));
            }
            Rec::End => {
                let end_at = at;
                return Ok((head_end.unwrap_or(end_at), cuts, end_at));
            }
            Rec::Normal => {
                if id == 3 {
                    n3 += 1;
                } else if id == 5 {
                    n5 += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    fn uint(v: u64, out: &mut Vec<u8>) {
        let mut v = v;
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                out.push(b | 0x80);
            } else {
                out.push(b);
                return;
            }
        }
    }

    fn sint(v: i64, out: &mut Vec<u8>) {
        uint((v.unsigned_abs() << 1) | (v < 0) as u64, out);
    }

    fn bstr(s: &[u8], out: &mut Vec<u8>) {
        uint(s.len() as u64, out);
        out.extend_from_slice(s);
    }

    /// two-form g-delta
    fn gdelta(dx: i64, dy: i64, out: &mut Vec<u8>) {
        uint((dx.unsigned_abs() << 2) | (((dx < 0) as u64) << 1) | 1, out);
        sint(dy, out);
    }

    /// Two cells named through an explicit CELLNAME table. A holds one
    /// of every record kind the Doc costs: a rectangle with an explicit
    /// point-list repetition that the next polygon reuses, manhattan
    /// polygons with and without the implicit closing vertex, a
    /// general-delta polygon reused by the next one, a path, a text and
    /// a placement of B. B holds two rectangles (one grid repetition)
    /// and a CTRAPEZOID.
    fn fixture() -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(MAGIC);
        // START: version, unit (real type 0), offset flag + table
        uint(1, &mut o);
        bstr(b"1.0", &mut o);
        uint(0, &mut o);
        uint(1000, &mut o);
        uint(0, &mut o);
        for _ in 0..12 {
            uint(0, &mut o);
        }
        uint(4, &mut o);
        bstr(b"A", &mut o);
        uint(0, &mut o);
        uint(4, &mut o);
        bstr(b"B", &mut o);
        uint(1, &mut o);
        // CELL A
        uint(13, &mut o);
        uint(0, &mut o);
        // rect: W H X Y R D L, 1/0, 10x20, repetition type 10 with
        // two g-deltas (3 members)
        uint(20, &mut o);
        o.push(0x7f);
        uint(1, &mut o);
        uint(0, &mut o);
        uint(10, &mut o);
        uint(20, &mut o);
        sint(0, &mut o);
        sint(0, &mut o);
        uint(10, &mut o);
        uint(1, &mut o);
        gdelta(5, 0, &mut o);
        gdelta(0, 5, &mut o);
        // poly1: manhattan, 2 deltas -> anchor + 2 + implicit closing
        // vertex = 4; repetition reuse (type 0)
        uint(21, &mut o);
        o.push(0x3c);
        uint(0, &mut o);
        uint(2, &mut o);
        sint(10, &mut o);
        sint(10, &mut o);
        sint(100, &mut o);
        sint(0, &mut o);
        uint(0, &mut o);
        // poly2: manhattan, 3 deltas ending on the anchor's axis -> 4,
        // no implicit vertex
        uint(21, &mut o);
        o.push(0x38);
        uint(0, &mut o);
        uint(3, &mut o);
        sint(10, &mut o);
        sint(10, &mut o);
        sint(-10, &mut o);
        sint(200, &mut o);
        sint(0, &mut o);
        // poly3: three general deltas -> 4 vertices
        uint(21, &mut o);
        o.push(0x38);
        uint(4, &mut o);
        uint(3, &mut o);
        gdelta(10, 0, &mut o);
        gdelta(0, 10, &mut o);
        gdelta(-10, 0, &mut o);
        sint(300, &mut o);
        sint(0, &mut o);
        // poly4: no point list - reuses poly3's (4 vertices)
        uint(21, &mut o);
        o.push(0x18);
        sint(400, &mut o);
        sint(0, &mut o);
        // path: E W P X Y, half-width 2, both extensions "0", two
        // general deltas -> 3 vertices
        uint(22, &mut o);
        o.push(0xf8);
        uint(2, &mut o);
        uint(5, &mut o);
        uint(4, &mut o);
        uint(2, &mut o);
        gdelta(10, 0, &mut o);
        gdelta(0, 10, &mut o);
        sint(500, &mut o);
        sint(0, &mut o);
        // text: C X Y T L with an explicit 5-byte string on 5/0
        uint(19, &mut o);
        o.push(0x5b);
        bstr(b"hello", &mut o);
        uint(5, &mut o);
        uint(0, &mut o);
        sint(0, &mut o);
        sint(50, &mut o);
        // placement of B: C N X Y
        uint(17, &mut o);
        o.push(0xf0);
        uint(1, &mut o);
        sint(1000, &mut o);
        sint(0, &mut o);
        // CELL B
        uint(13, &mut o);
        uint(1, &mut o);
        uint(20, &mut o);
        o.push(0x7b);
        uint(2, &mut o);
        uint(0, &mut o);
        uint(10, &mut o);
        uint(10, &mut o);
        sint(0, &mut o);
        sint(0, &mut o);
        // second rectangle: W/H modal, grid repetition type 2 (5 members)
        uint(20, &mut o);
        o.push(0x1c);
        sint(0, &mut o);
        sint(100, &mut o);
        uint(2, &mut o);
        uint(3, &mut o);
        uint(100, &mut o);
        // CTRAPEZOID type 0, 10 x 5
        uint(26, &mut o);
        o.push(0xf8);
        uint(0, &mut o);
        uint(10, &mut o);
        uint(5, &mut o);
        sint(0, &mut o);
        sint(200, &mut o);
        // END: 256 bytes in all
        let start = o.len();
        uint(2, &mut o);
        bstr(&[0u8; 252], &mut o);
        uint(0, &mut o);
        assert_eq!(o.len() - start, 256);
        o
    }

    fn pts_key(r: &crate::doc::Rep) -> Option<(usize, usize)> {
        match r {
            crate::doc::Rep::Pts(p) => Some((
                std::sync::Arc::as_ptr(p) as *const (i64, i64) as usize,
                p.len(),
            )),
            _ => None,
        }
    }

    /// the scan's cost model computed from the parsed Doc itself:
    /// records at their real sizes, vertex and text heap, the cell
    /// name, one block per distinct Rep::Pts allocation (modal reuse
    /// shares it)
    fn doc_model(cell: &crate::doc::Cell) -> u64 {
        let mut seen: std::collections::HashSet<(usize, usize)> =
            std::collections::HashSet::new();
        let mut reps = |r: &crate::doc::Rep| -> u64 {
            match pts_key(r) {
                Some(k) if seen.insert(k) => 16 + PT_SZ * k.1 as u64,
                _ => 0,
            }
        };
        let mut b = CELL_SZ + cell.name.len() as u64;
        for r in &cell.rects {
            b += RECT_SZ + reps(&r.rep);
        }
        for p in &cell.polys {
            b += POLY_SZ + PT_SZ * p.pts.len() as u64 + reps(&p.rep);
        }
        for p in &cell.paths {
            b += PATH_SZ + PT_SZ * p.pts.len() as u64 + reps(&p.rep);
        }
        for p in &cell.places {
            b += PLACE_SZ + reps(&p.rep);
        }
        for t in &cell.texts {
            b += TEXT_SZ + t.s.len() as u64 + reps(&t.rep);
        }
        b
    }

    #[test]
    fn scan_cell_stats_and_doc_estimate() {
        let data = fixture();
        let st = scan(&data).expect("scan");
        assert_eq!(st.cells, 2);
        assert_eq!(st.cell_stats.len(), 2);
        let a = &st.cell_stats[0];
        let b = &st.cell_stats[1];
        assert_eq!((a.name.as_str(), b.name.as_str()), ("A", "B"));
        assert_eq!((a.records, a.shapes, a.placements, a.texts), (8, 6, 1, 1));
        assert_eq!((b.records, b.shapes, b.placements, b.texts), (3, 3, 0, 0));
        // vertices as the geometry parser materializes them: 4 + 4 + 4
        // + 4 (modal reuse) + 3 (path) in A, the CTRAPEZOID's 4 in B
        assert_eq!((a.points, b.points), (19, 4));
        assert_eq!((st.points, st.points_max), (23, 4));
        // one explicit point-list repetition of 3 members, reused once
        assert_eq!((a.pts_reps, a.pts_rep_members, b.pts_reps), (1, 3, 0));
        assert_eq!(
            (st.pts_reps, st.pts_rep_members, st.pts_rep_members_max),
            (1, 3, 3)
        );
        assert_eq!(st.rep_types.get(&0), Some(&1));
        assert_eq!(st.rep_types.get(&10), Some(&1));
        assert_eq!(st.rep_types.get(&2), Some(&1));
        // the estimate is the parsed Doc's own size, cell by cell
        let doc = crate::doc::parse_doc(&data).expect("parse_doc");
        let mut total = 0;
        for cs in &st.cell_stats {
            let cell = doc
                .cells
                .iter()
                .find(|c| c.name == cs.name)
                .expect("doc cell");
            assert_eq!(cs.doc_bytes, doc_model(cell), "cell {}", cs.name);
            total += cs.doc_bytes;
        }
        assert_eq!(st.doc_bytes, total);
        // power-of-two classes round every heap block up, never down
        assert!(st.doc_bytes_pow2 > st.doc_bytes);
        assert_eq!(pow2_block(48), 64);
        assert_eq!(pow2_block(16 + 16 * 3), 64);
        assert_eq!(pow2_block(0), 0);
        assert_eq!(pow2_block(40_000), 40_000);
        // the parallel scan (one cell per worker) reports the same
        let par = scan_parallel(&data, 2).expect("scan_parallel");
        assert_eq!(par.cell_stats.len(), 2);
        for (x, y) in st.cell_stats.iter().zip(par.cell_stats.iter()) {
            assert_eq!(
                (x.name.as_str(), x.records, x.points, x.pts_rep_members),
                (y.name.as_str(), y.records, y.points, y.pts_rep_members)
            );
            assert_eq!((x.doc_bytes, x.doc_bytes_pow2), (y.doc_bytes, y.doc_bytes_pow2));
        }
        assert_eq!(
            (par.doc_bytes, par.doc_bytes_pow2, par.points, par.pts_reps),
            (st.doc_bytes, st.doc_bytes_pow2, st.points, st.pts_reps)
        );
        assert_eq!(par.records, st.records);
    }
}
