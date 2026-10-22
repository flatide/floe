//! `design.inv` — the inventory pass's result (`inventory::Inventory`)
//! as a file: the first piece of the light cache of
//! docs/DIRECT_READ_PLAN.ko.md (§5.1, step L1), and the sidecar
//! STREAMING_INDEX_PLAN §10.2 deferred. The streaming build writes it
//! beside design.ovm; `floe-index inventory FILE --out PATH` writes it
//! alone and `--load PATH` reads it back.
//!
//! Format FLOEINV1 (little-endian throughout):
//!
//! - header 72 B: magic `FLOEINV1`, version u32 (1), header_len u32
//!   (72), body_len u64, body_crc32 u32 (IEEE, over the body), flags u32
//!   (0), src_size u64, src_mtime u64 (the source's identity, as
//!   design.ovm carries it), n_cells u32, n_layers u32, top u32,
//!   reserved u32, unit f64.
//! - body: sections in this order, each `tag u32, len u64, payload`:
//!   SCAN (1), LAYERS (2), HEAD (3), CELLS (4), NAMES (5), FOLD (6);
//!   the body ends with the last section (no trailer). A string is
//!   `u32 len + UTF-8`; a box is 4 × i64; an optional box is `u8 flag`
//!   then the box when the flag is 1; a map is `u32 n` and its entries
//!   sorted by key (the bytes of one inventory are therefore one
//!   sequence: the gate compares encodings).
//!
//! The decoder trusts nothing: every length is checked against the
//! bytes left, every index against the cell count, every mask against
//! the layer count, and the body's CRC against the header's, so a
//! truncated, damaged or foreign file is refused with a position
//! instead of being read as an inventory.

use crate::geom::{box_is_empty, Box4, EMPTY_BOX};
use crate::inventory::{CellRange, Edge, InvCell, Inventory};
use crate::{err, CellStat, LayerStat, Result, ScanStats};
use std::collections::HashMap;
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"FLOEINV1";
pub const VERSION: u32 = 1;
pub const FILE_NAME: &str = "design.inv";
pub const HEADER_LEN: usize = 72;

const TAG_SCAN: u32 = 1;
const TAG_LAYERS: u32 = 2;
const TAG_HEAD: u32 = 3;
const TAG_CELLS: u32 = 4;
const TAG_NAMES: u32 = 5;
const TAG_FOLD: u32 = 6;

/// the source the inventory describes, as the cache's identity names
/// it (`floe_ovm::Ovm::src_size` / `src_mtime`: byte length and
/// modification time in whole seconds since the epoch)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub src_size: u64,
    pub src_mtime: u64,
}

// ------------------------------------------------------------ crc32

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC_TABLE: [u32; 256] = crc_table();

/// CRC-32 (IEEE 802.3, the zip/png polynomial) of `data`
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

// ------------------------------------------------------------ writer

struct Wr(Vec<u8>);

impl Wr {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn count(&mut self, n: usize) {
        self.u32(u32::try_from(n).expect("count fits u32"));
    }
    fn str(&mut self, s: &str) {
        self.count(s.len());
        self.0.extend_from_slice(s.as_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.count(b.len());
        self.0.extend_from_slice(b);
    }
    fn bx(&mut self, b: &Box4) {
        self.i64(b.0);
        self.i64(b.1);
        self.i64(b.2);
        self.i64(b.3);
    }
    fn opt_box(&mut self, b: &Option<Box4>) {
        match b {
            Some(b) => {
                self.u8(1);
                self.bx(b);
            }
            None => self.u8(0),
        }
    }
    fn range(&mut self, r: &CellRange) {
        self.u64(r.start);
        self.u64(r.end);
        self.u64(r.cellname_base);
        self.u64(r.textstring_base);
    }
    /// one section: tag, payload length, payload
    fn section(&mut self, tag: u32, payload: Wr) {
        self.u32(tag);
        self.u64(payload.0.len() as u64);
        self.0.extend_from_slice(&payload.0);
    }
}

fn sorted_u64_map(m: &HashMap<u64, u64>) -> Vec<(u64, u64)> {
    let mut v: Vec<(u64, u64)> = m.iter().map(|(k, v)| (*k, *v)).collect();
    v.sort_unstable();
    v
}

fn sorted_layer_stats(m: &HashMap<(u64, u64), LayerStat>) -> Vec<((u64, u64), u64, u64)> {
    let mut v: Vec<_> = m.iter().map(|(k, s)| (*k, s.records, s.members)).collect();
    v.sort_unstable();
    v
}

fn section_scan(st: &ScanStats) -> Wr {
    let mut w = Wr(Vec::new());
    w.f64(st.unit);
    for v in [
        st.file_bytes,
        st.cells,
        st.cellnames,
        st.placements,
        st.placement_members,
        st.cblocks,
        st.cblock_bytes_inflated,
        st.records,
        st.points,
        st.points_max,
        st.pts_reps,
        st.pts_rep_members,
        st.pts_rep_members_max,
        st.doc_bytes,
        st.doc_bytes_pow2,
    ] {
        w.u64(v);
    }
    let ids = sorted_u64_map(&st.record_ids);
    w.count(ids.len());
    for (k, v) in ids {
        w.u64(k);
        w.u64(v);
    }
    for m in [&st.shapes, &st.texts] {
        let v = sorted_layer_stats(m);
        w.count(v.len());
        for ((l, d), r, mm) in v {
            w.u64(l);
            w.u64(d);
            w.u64(r);
            w.u64(mm);
        }
    }
    let reps = sorted_u64_map(&st.rep_types);
    w.count(reps.len());
    for (k, v) in reps {
        w.u64(k);
        w.u64(v);
    }
    w.count(st.cell_stats.len());
    for c in &st.cell_stats {
        w.str(&c.name);
        for v in [
            c.records,
            c.shapes,
            c.placements,
            c.texts,
            c.points,
            c.pts_reps,
            c.pts_rep_members,
            c.doc_bytes,
            c.doc_bytes_pow2,
        ] {
            w.u64(v);
        }
        // the record's shape box is the scan's extra (filled only when the
        // walk collected segments): not the inventory's, so the file reads
        // the same whichever walk produced it
        w.u8(0);
    }
    w
}

fn section_layers(inv: &Inventory) -> Wr {
    let mut w = Wr(Vec::new());
    w.count(inv.layer_order.len());
    for &(l, d) in &inv.layer_order {
        w.u32(l);
        w.u32(d);
    }
    w.count(inv.mask_bytes());
    for v in &inv.lrecs {
        w.u64(*v);
    }
    for v in &inv.lmems {
        w.u64(*v);
    }
    let mut names: Vec<_> = inv.layer_names.iter().collect();
    names.sort_unstable_by_key(|(k, _)| **k);
    w.count(names.len());
    for (k, s) in names {
        w.u32(k.0);
        w.u32(k.1);
        w.str(s);
    }
    let mut aliases: Vec<_> = inv.layer_aliases.iter().collect();
    aliases.sort_unstable_by_key(|(k, _)| **k);
    w.count(aliases.len());
    for (k, list) in aliases {
        w.u32(k.0);
        w.u32(k.1);
        w.count(list.len());
        for s in list {
            w.str(s);
        }
    }
    w
}

fn section_head(inv: &Inventory) -> Wr {
    let mut w = Wr(Vec::new());
    w.range(&inv.head);
    w.count(inv.top);
    w
}

fn section_cells(inv: &Inventory) -> Wr {
    let mut w = Wr(Vec::new());
    w.count(inv.cells.len());
    for c in &inv.cells {
        w.str(&c.name);
        w.count(c.ranges.len());
        for r in &c.ranges {
            w.range(r);
        }
        w.u32(c.cblock_defs);
        w.count(c.def_ranges.len());
        for v in &c.def_ranges {
            w.u64(*v);
        }
        for v in [
            c.records,
            c.shapes,
            c.placements,
            c.texts,
            c.points,
            c.doc_bytes,
            c.doc_bytes_pow2,
            c.dmembers,
        ] {
            w.u64(v);
        }
        w.bx(&c.dbox);
        w.bx(&c.dbox_notext);
        w.bytes(&c.dmask);
        w.bytes(&c.dtmask);
        w.bytes(&c.dsmask);
        w.count(c.edges.len());
        for e in &c.edges {
            w.count(e.child);
            w.u64(e.placements);
            w.u64(e.members);
            let mut used = 0u8;
            for (o, b) in e.obox.iter().enumerate() {
                if !box_is_empty(b) {
                    used |= 1 << o;
                }
            }
            w.u8(used);
            for (o, b) in e.obox.iter().enumerate() {
                if used & (1 << o) != 0 {
                    w.bx(b);
                }
            }
        }
    }
    w
}

fn section_names(inv: &Inventory) -> Wr {
    let mut w = Wr(Vec::new());
    for m in [&inv.cellnames, &inv.textstrings] {
        let mut v: Vec<_> = m.iter().collect();
        v.sort_unstable_by_key(|(k, _)| **k);
        w.count(v.len());
        for (k, s) in v {
            w.u64(*k);
            w.str(s);
        }
    }
    w
}

fn section_fold(inv: &Inventory) -> Wr {
    let mut w = Wr(Vec::new());
    for v in &inv.topo {
        w.count(*v);
    }
    for v in &inv.rank {
        w.u32(*v);
    }
    for v in &inv.height {
        w.u32(*v);
    }
    for v in &inv.rmembers {
        w.u64(*v);
    }
    for masks in [&inv.rmask, &inv.rtmask, &inv.rsmask] {
        for m in masks.iter() {
            w.bytes(m);
        }
    }
    for b in &inv.rbb {
        w.opt_box(b);
    }
    for b in &inv.rbb_notext {
        w.opt_box(b);
    }
    w
}

/// the file's bytes for `inv` describing the source `id`
pub fn encode(inv: &Inventory, id: Identity) -> Vec<u8> {
    let mut body = Wr(Vec::new());
    body.section(TAG_SCAN, section_scan(&inv.scan));
    body.section(TAG_LAYERS, section_layers(inv));
    body.section(TAG_HEAD, section_head(inv));
    body.section(TAG_CELLS, section_cells(inv));
    body.section(TAG_NAMES, section_names(inv));
    body.section(TAG_FOLD, section_fold(inv));
    let crc = crc32(&body.0);
    let mut h = Wr(Vec::with_capacity(HEADER_LEN + body.0.len()));
    h.0.extend_from_slice(MAGIC);
    h.u32(VERSION);
    h.u32(HEADER_LEN as u32);
    h.u64(body.0.len() as u64);
    h.u32(crc);
    h.u32(0);
    h.u64(id.src_size);
    h.u64(id.src_mtime);
    h.count(inv.cells.len());
    h.count(inv.layer_order.len());
    h.count(inv.top);
    h.u32(0);
    h.f64(inv.unit());
    debug_assert_eq!(h.0.len(), HEADER_LEN);
    h.0.extend_from_slice(&body.0);
    h.0
}

// ------------------------------------------------------------ reader

struct Rd<'a> {
    d: &'a [u8],
    pos: usize,
    /// where this reader's bytes start in the file (for messages)
    base: usize,
}

impl<'a> Rd<'a> {
    fn new(d: &'a [u8], base: usize) -> Rd<'a> {
        Rd { d, pos: 0, base }
    }
    fn at(&self) -> usize {
        self.base + self.pos
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.d.len() - self.pos {
            return err(self.at(), "design.inv: truncated");
        }
        let s = &self.d[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    /// a count that must be payable in bytes: every element takes at
    /// least `at_least` bytes, so a count past the bytes left is damage,
    /// not a large allocation
    fn count(&mut self, at_least: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n.saturating_mul(at_least.max(1)) > self.d.len() - self.pos {
            return err(self.at(), "design.inv: count past the end");
        }
        Ok(n)
    }
    fn index(&mut self, n: usize, what: &str) -> Result<usize> {
        let v = self.u32()? as usize;
        if v >= n {
            return err(self.at(), &format!("design.inv: {} {} past {}", what, v, n));
        }
        Ok(v)
    }
    fn str(&mut self) -> Result<String> {
        let n = self.count(1)?;
        let b = self.take(n)?;
        match std::str::from_utf8(b) {
            Ok(s) => Ok(s.to_string()),
            Err(_) => err(self.at(), "design.inv: string is not UTF-8"),
        }
    }
    fn bytes(&mut self) -> Result<Vec<u8>> {
        let n = self.count(1)?;
        Ok(self.take(n)?.to_vec())
    }
    fn mask(&mut self, width: usize) -> Result<Vec<u8>> {
        let m = self.bytes()?;
        if m.len() != width {
            return err(self.at(), "design.inv: layer mask width");
        }
        Ok(m)
    }
    fn bx(&mut self) -> Result<Box4> {
        Ok((self.i64()?, self.i64()?, self.i64()?, self.i64()?))
    }
    fn opt_box(&mut self) -> Result<Option<Box4>> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.bx()?)),
            _ => err(self.at(), "design.inv: box flag"),
        }
    }
    fn range(&mut self) -> Result<CellRange> {
        Ok(CellRange {
            start: self.u64()?,
            end: self.u64()?,
            cellname_base: self.u64()?,
            textstring_base: self.u64()?,
        })
    }
    fn done(&self, what: &str) -> Result<()> {
        if self.pos != self.d.len() {
            return err(self.at(), &format!("design.inv: {} has bytes left over", what));
        }
        Ok(())
    }
    /// the next section, which must carry `tag`
    fn section(&mut self, tag: u32) -> Result<Rd<'a>> {
        let t = self.u32()?;
        if t != tag {
            return err(self.at(), &format!("design.inv: section {} where {} belongs", t, tag));
        }
        let len = self.u64()?;
        if len > (self.d.len() - self.pos) as u64 {
            return err(self.at(), "design.inv: section past the end");
        }
        let base = self.at();
        let payload = self.take(len as usize)?;
        Ok(Rd::new(payload, base))
    }
}

fn read_scan(r: &mut Rd) -> Result<ScanStats> {
    let unit = r.f64()?;
    let mut v = [0u64; 15];
    for x in v.iter_mut() {
        *x = r.u64()?;
    }
    let n = r.count(16)?;
    let mut record_ids = HashMap::with_capacity(n);
    for _ in 0..n {
        let k = r.u64()?;
        record_ids.insert(k, r.u64()?);
    }
    let mut stats = [HashMap::new(), HashMap::new()];
    for m in stats.iter_mut() {
        let n = r.count(32)?;
        for _ in 0..n {
            let k = (r.u64()?, r.u64()?);
            let s = LayerStat {
                records: r.u64()?,
                members: r.u64()?,
            };
            m.insert(k, s);
        }
    }
    let [shapes, texts] = stats;
    let n = r.count(16)?;
    let mut rep_types = HashMap::with_capacity(n);
    for _ in 0..n {
        let k = r.u64()?;
        rep_types.insert(k, r.u64()?);
    }
    let n = r.count(4 + 9 * 8 + 1)?;
    let mut cell_stats = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let mut c = [0u64; 9];
        for x in c.iter_mut() {
            *x = r.u64()?;
        }
        if r.u8()? != 0 {
            return err(r.at(), "design.inv: cell record box flag");
        }
        cell_stats.push(CellStat {
            name,
            cell_ref: None,
            records: c[0],
            shapes: c[1],
            placements: c[2],
            texts: c[3],
            points: c[4],
            pts_reps: c[5],
            pts_rep_members: c[6],
            doc_bytes: c[7],
            doc_bytes_pow2: c[8],
            bbox: None,
        });
    }
    r.done("SCAN")?;
    Ok(ScanStats {
        file_bytes: v[0],
        cells: v[1],
        cellnames: v[2],
        placements: v[3],
        placement_members: v[4],
        cblocks: v[5],
        cblock_bytes_inflated: v[6],
        records: v[7],
        record_ids,
        shapes,
        texts,
        rep_types,
        unit,
        points: v[8],
        points_max: v[9],
        pts_reps: v[10],
        pts_rep_members: v[11],
        pts_rep_members_max: v[12],
        doc_bytes: v[13],
        doc_bytes_pow2: v[14],
        cell_stats,
        ..Default::default()
    })
}

struct Layers {
    order: Vec<(u32, u32)>,
    mask_bytes: usize,
    lrecs: Vec<u64>,
    lmems: Vec<u64>,
    names: HashMap<(u32, u32), String>,
    aliases: HashMap<(u32, u32), Vec<String>>,
}

fn read_layers(r: &mut Rd) -> Result<Layers> {
    let nl = r.count(8)?;
    let mut order = Vec::with_capacity(nl);
    for _ in 0..nl {
        order.push((r.u32()?, r.u32()?));
    }
    let mask_bytes = r.u32()? as usize;
    if mask_bytes != nl.div_ceil(8).max(1) {
        return err(r.at(), "design.inv: mask width does not fit the layer count");
    }
    let mut lrecs = Vec::with_capacity(nl);
    for _ in 0..nl {
        lrecs.push(r.u64()?);
    }
    let mut lmems = Vec::with_capacity(nl);
    for _ in 0..nl {
        lmems.push(r.u64()?);
    }
    let n = r.count(12)?;
    let mut names = HashMap::with_capacity(n);
    for _ in 0..n {
        let k = (r.u32()?, r.u32()?);
        names.insert(k, r.str()?);
    }
    let n = r.count(12)?;
    let mut aliases = HashMap::with_capacity(n);
    for _ in 0..n {
        let k = (r.u32()?, r.u32()?);
        let m = r.count(4)?;
        let mut list = Vec::with_capacity(m);
        for _ in 0..m {
            list.push(r.str()?);
        }
        aliases.insert(k, list);
    }
    r.done("LAYERS")?;
    Ok(Layers {
        order,
        mask_bytes,
        lrecs,
        lmems,
        names,
        aliases,
    })
}

fn read_cells(r: &mut Rd, n_cells: usize, mask_bytes: usize) -> Result<Vec<InvCell>> {
    let n = r.count(4)?;
    if n != n_cells {
        return err(r.at(), "design.inv: cell count differs from the header");
    }
    let mut cells = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let nr = r.count(32)?;
        let mut ranges = Vec::with_capacity(nr);
        for _ in 0..nr {
            ranges.push(r.range()?);
        }
        let cblock_defs = r.u32()?;
        let nd = r.count(8)?;
        let mut def_ranges = Vec::with_capacity(nd);
        for _ in 0..nd {
            def_ranges.push(r.u64()?);
        }
        let mut c = [0u64; 8];
        for x in c.iter_mut() {
            *x = r.u64()?;
        }
        let dbox = r.bx()?;
        let dbox_notext = r.bx()?;
        let dmask = r.mask(mask_bytes)?;
        let dtmask = r.mask(mask_bytes)?;
        let dsmask = r.mask(mask_bytes)?;
        let ne = r.count(4 + 8 + 8 + 1)?;
        let mut edges = Vec::with_capacity(ne);
        for _ in 0..ne {
            let child = r.index(n_cells, "edge child")?;
            let placements = r.u64()?;
            let members = r.u64()?;
            let used = r.u8()?;
            let mut obox = [EMPTY_BOX; 8];
            for (o, b) in obox.iter_mut().enumerate() {
                if used & (1 << o) != 0 {
                    *b = r.bx()?;
                }
            }
            edges.push(Edge {
                child,
                placements,
                members,
                obox,
            });
        }
        cells.push(InvCell {
            name,
            ranges,
            cblock_defs,
            def_ranges,
            records: c[0],
            shapes: c[1],
            placements: c[2],
            texts: c[3],
            points: c[4],
            doc_bytes: c[5],
            doc_bytes_pow2: c[6],
            dbox,
            dbox_notext,
            dmembers: c[7],
            dmask,
            dtmask,
            dsmask,
            edges,
        });
    }
    r.done("CELLS")?;
    Ok(cells)
}

fn read_names(r: &mut Rd) -> Result<(HashMap<u64, String>, HashMap<u64, String>)> {
    let mut maps = [HashMap::new(), HashMap::new()];
    for m in maps.iter_mut() {
        let n = r.count(12)?;
        for _ in 0..n {
            let k = r.u64()?;
            m.insert(k, r.str()?);
        }
    }
    r.done("NAMES")?;
    let [a, b] = maps;
    Ok((a, b))
}

struct Fold {
    topo: Vec<usize>,
    rank: Vec<u32>,
    height: Vec<u32>,
    rmembers: Vec<u64>,
    rmask: Vec<Vec<u8>>,
    rtmask: Vec<Vec<u8>>,
    rsmask: Vec<Vec<u8>>,
    rbb: Vec<Option<Box4>>,
    rbb_notext: Vec<Option<Box4>>,
}

fn read_fold(r: &mut Rd, n: usize, mask_bytes: usize) -> Result<Fold> {
    let mut topo = Vec::with_capacity(n);
    let mut seen = vec![false; n];
    for _ in 0..n {
        let v = r.index(n, "topo entry")?;
        if seen[v] {
            return err(r.at(), "design.inv: topo order repeats a cell");
        }
        seen[v] = true;
        topo.push(v);
    }
    let mut rank = Vec::with_capacity(n);
    for _ in 0..n {
        rank.push(r.u32()?);
    }
    let mut height = Vec::with_capacity(n);
    for _ in 0..n {
        height.push(r.u32()?);
    }
    let mut rmembers = Vec::with_capacity(n);
    for _ in 0..n {
        rmembers.push(r.u64()?);
    }
    let mut masks: [Vec<Vec<u8>>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for m in masks.iter_mut() {
        m.reserve(n);
        for _ in 0..n {
            m.push(r.mask(mask_bytes)?);
        }
    }
    let [rmask, rtmask, rsmask] = masks;
    let mut rbb = Vec::with_capacity(n);
    for _ in 0..n {
        rbb.push(r.opt_box()?);
    }
    let mut rbb_notext = Vec::with_capacity(n);
    for _ in 0..n {
        rbb_notext.push(r.opt_box()?);
    }
    r.done("FOLD")?;
    Ok(Fold {
        topo,
        rank,
        height,
        rmembers,
        rmask,
        rtmask,
        rsmask,
        rbb,
        rbb_notext,
    })
}

/// the header alone: the identity and the counts, without reading the
/// body (what an opener compares against the source before trusting
/// the file); the body's CRC is not checked here
/// the header's fields (`peek_header`)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InvHeader {
    pub identity: Identity,
    pub body_len: u64,
    /// the body's CRC-32 - what design.seg's `inv_crc` names
    pub body_crc: u32,
    pub n_cells: usize,
    pub n_layers: usize,
    pub top: usize,
    pub unit: f64,
}

/// The header alone (the file's first HEADER_LEN bytes): identity, counts,
/// top and unit, the body's length and CRC - without the body (a cache
/// probe reads this much and compares `body_len` with the file's size).
pub fn peek_header(bytes: &[u8]) -> Result<InvHeader> {
    if bytes.len() < HEADER_LEN {
        return err(0, "design.inv: shorter than its header");
    }
    if &bytes[..8] != MAGIC {
        return err(0, "design.inv: not a FLOEINV1 file");
    }
    let mut r = Rd::new(&bytes[8..HEADER_LEN], 8);
    let version = r.u32()?;
    if version != VERSION {
        return err(8, &format!("design.inv: version {} (this build reads {})", version, VERSION));
    }
    let header_len = r.u32()? as usize;
    if header_len != HEADER_LEN {
        return err(12, "design.inv: header length");
    }
    let body_len = r.u64()?;
    let body_crc = r.u32()?;
    let _flags = r.u32()?;
    let identity = Identity {
        src_size: r.u64()?,
        src_mtime: r.u64()?,
    };
    let n_cells = r.u32()? as usize;
    let n_layers = r.u32()? as usize;
    let top = r.u32()? as usize;
    let _reserved = r.u32()?;
    let unit = r.f64()?;
    Ok(InvHeader {
        identity,
        body_len,
        body_crc,
        n_cells,
        n_layers,
        top,
        unit,
    })
}

pub fn peek(bytes: &[u8]) -> Result<(Identity, usize, usize)> {
    if bytes.len() < HEADER_LEN {
        return err(0, "design.inv: shorter than its header");
    }
    if &bytes[..8] != MAGIC {
        return err(0, "design.inv: not a FLOEINV1 file");
    }
    let mut r = Rd::new(&bytes[8..HEADER_LEN], 8);
    let version = r.u32()?;
    if version != VERSION {
        return err(8, &format!("design.inv: version {} (this build reads {})", version, VERSION));
    }
    let header_len = r.u32()? as usize;
    if header_len != HEADER_LEN {
        return err(12, "design.inv: header length");
    }
    let body_len = r.u64()?;
    let _crc = r.u32()?;
    let _flags = r.u32()?;
    let id = Identity {
        src_size: r.u64()?,
        src_mtime: r.u64()?,
    };
    let n_cells = r.u32()? as usize;
    let n_layers = r.u32()? as usize;
    if body_len != (bytes.len() - HEADER_LEN) as u64 {
        return err(16, "design.inv: body length differs from the file's");
    }
    Ok((id, n_cells, n_layers))
}

/// the inventory the bytes hold, after every check the header and the
/// body allow (see the module notes)
pub fn decode(bytes: &[u8]) -> Result<(Identity, Inventory)> {
    let (id, n_cells, n_layers) = peek(bytes)?;
    let mut h = Rd::new(&bytes[16..HEADER_LEN], 16);
    let _body_len = h.u64()?;
    let crc = h.u32()?;
    let _flags = h.u32()?;
    let _size = h.u64()?;
    let _mtime = h.u64()?;
    let _n = h.u32()?;
    let _nl = h.u32()?;
    let top = h.u32()? as usize;
    let _reserved = h.u32()?;
    let unit = h.f64()?;
    let body = &bytes[HEADER_LEN..];
    if crc32(body) != crc {
        return err(HEADER_LEN, "design.inv: body checksum differs (truncated or damaged)");
    }
    if top >= n_cells {
        return err(56, "design.inv: top cell past the cell count");
    }
    let mut r = Rd::new(body, HEADER_LEN);
    let scan = read_scan(&mut r.section(TAG_SCAN)?)?;
    if scan.unit.to_bits() != unit.to_bits() {
        return err(HEADER_LEN, "design.inv: the header's unit differs from the scan's");
    }
    let layers = read_layers(&mut r.section(TAG_LAYERS)?)?;
    if layers.order.len() != n_layers {
        return err(HEADER_LEN, "design.inv: layer count differs from the header");
    }
    let (head, top2) = {
        let mut s = r.section(TAG_HEAD)?;
        let head = s.range()?;
        let t = s.index(n_cells, "top")?;
        s.done("HEAD")?;
        (head, t)
    };
    if top2 != top {
        return err(HEADER_LEN, "design.inv: the header's top differs from the body's");
    }
    let cells = read_cells(&mut r.section(TAG_CELLS)?, n_cells, layers.mask_bytes)?;
    let (cellnames, textstrings) = read_names(&mut r.section(TAG_NAMES)?)?;
    let fold = read_fold(&mut r.section(TAG_FOLD)?, n_cells, layers.mask_bytes)?;
    r.done("body")?;
    Ok((
        id,
        Inventory {
            scan,
            cells,
            top,
            head,
            cellnames,
            textstrings,
            layer_order: layers.order,
            layer_names: layers.names,
            layer_aliases: layers.aliases,
            lrecs: layers.lrecs,
            lmems: layers.lmems,
            topo: fold.topo,
            rank: fold.rank,
            height: fold.height,
            rmembers: fold.rmembers,
            rmask: fold.rmask,
            rtmask: fold.rtmask,
            rsmask: fold.rsmask,
            rbb: fold.rbb,
            rbb_notext: fold.rbb_notext,
        },
    ))
}

/// `encode` to `path` through `path.tmp` + rename; the byte count
pub fn write(path: &Path, inv: &Inventory, id: Identity) -> std::io::Result<usize> {
    let bytes = encode(inv, id);
    let tmp = path.with_extension("inv.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(bytes.len())
}

/// the file at `path`, decoded
pub fn read(path: &Path) -> Result<(Identity, Inventory)> {
    let bytes = std::fs::read(path)?;
    decode(&bytes)
}

/// `bytes` decode to an inventory whose encoding is `bytes` again: the
/// round trip the L1 gate holds (a file written by this build)
pub fn roundtrip(bytes: &[u8]) -> Result<()> {
    let (id, inv) = decode(bytes)?;
    if encode(&inv, id) != bytes {
        return err(0, "design.inv: the decoded inventory encodes to other bytes");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::fixtures::{adversarial, head_cblock_cell};

    fn streams() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("adversarial", adversarial(false, false)),
            ("adversarial tables first", adversarial(true, false)),
            ("adversarial cblock", adversarial(false, true)),
            ("adversarial tables first + cblock", adversarial(true, true)),
            ("head cblock cell", head_cblock_cell()),
        ]
    }

    const ID: Identity = Identity {
        src_size: 1234,
        src_mtime: 1_760_000_000,
    };

    /// the payload range of the body section `tag`
    fn section_range(bytes: &[u8], tag: u32) -> (usize, usize) {
        let mut at = HEADER_LEN;
        while at + 12 <= bytes.len() {
            let t = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            let len = u64::from_le_bytes(bytes[at + 4..at + 12].try_into().unwrap()) as usize;
            if t == tag {
                return (at + 12, at + 12 + len);
            }
            at += 12 + len;
        }
        panic!("section {} not in the bytes", tag);
    }

    #[test]
    fn the_round_trip_is_byte_identical_and_field_identical() {
        for (name, data) in streams() {
            let inv = crate::inventory(&data, 1).expect(name);
            let bytes = encode(&inv, ID);
            assert_eq!(&bytes[..8], MAGIC);
            let (id, back) = decode(&bytes).expect(name);
            assert_eq!(id, ID, "{}", name);
            assert_eq!(encode(&back, id), bytes, "{}: second encoding", name);
            roundtrip(&bytes).expect(name);
            let (pid, n, nl) = peek(&bytes).unwrap();
            assert_eq!((pid, n, nl), (ID, inv.cells.len(), inv.layer_order.len()));
            // the fields the consumers read
            assert_eq!(back.top, inv.top, "{}", name);
            assert_eq!(back.head, inv.head, "{}", name);
            assert_eq!(back.unit(), inv.unit(), "{}", name);
            assert_eq!(back.layer_order, inv.layer_order, "{}", name);
            assert_eq!(back.layer_names, inv.layer_names, "{}", name);
            assert_eq!(back.layer_aliases, inv.layer_aliases, "{}", name);
            assert_eq!(back.lrecs, inv.lrecs, "{}", name);
            assert_eq!(back.lmems, inv.lmems, "{}", name);
            assert_eq!(back.cellnames, inv.cellnames, "{}", name);
            assert_eq!(back.textstrings, inv.textstrings, "{}", name);
            assert_eq!(back.topo, inv.topo, "{}", name);
            assert_eq!(back.rank, inv.rank, "{}", name);
            assert_eq!(back.height, inv.height, "{}", name);
            assert_eq!(back.rmembers, inv.rmembers, "{}", name);
            assert_eq!(back.rmask, inv.rmask, "{}", name);
            assert_eq!(back.rtmask, inv.rtmask, "{}", name);
            assert_eq!(back.rsmask, inv.rsmask, "{}", name);
            assert_eq!(back.rbb, inv.rbb, "{}", name);
            assert_eq!(back.rbb_notext, inv.rbb_notext, "{}", name);
            assert_eq!(back.cells.len(), inv.cells.len(), "{}", name);
            for (a, b) in back.cells.iter().zip(&inv.cells) {
                assert_eq!(a.name, b.name);
                assert_eq!(a.ranges, b.ranges, "{} {}", name, b.name);
                assert_eq!(a.cblock_defs, b.cblock_defs);
                assert_eq!(a.def_ranges, b.def_ranges);
                assert_eq!(
                    (a.records, a.shapes, a.placements, a.texts, a.points),
                    (b.records, b.shapes, b.placements, b.texts, b.points)
                );
                assert_eq!((a.doc_bytes, a.doc_bytes_pow2, a.dmembers), (b.doc_bytes, b.doc_bytes_pow2, b.dmembers));
                assert_eq!((a.dbox, a.dbox_notext), (b.dbox, b.dbox_notext));
                assert_eq!((&a.dmask, &a.dtmask, &a.dsmask), (&b.dmask, &b.dtmask, &b.dsmask));
                assert_eq!(a.edges, b.edges, "{} {}", name, b.name);
            }
            let (s, t) = (&back.scan, &inv.scan);
            assert_eq!(
                (s.file_bytes, s.cells, s.records, s.placements, s.placement_members, s.cblocks),
                (t.file_bytes, t.cells, t.records, t.placements, t.placement_members, t.cblocks)
            );
            assert_eq!((s.doc_bytes, s.doc_bytes_pow2, s.points, s.pts_reps), (t.doc_bytes, t.doc_bytes_pow2, t.points, t.pts_reps));
            assert_eq!(s.record_ids, t.record_ids);
            assert_eq!(s.rep_types, t.rep_types);
            assert_eq!(s.cell_stats.len(), t.cell_stats.len());
            for (a, b) in s.cell_stats.iter().zip(&t.cell_stats) {
                assert_eq!((&a.name, a.records, a.doc_bytes), (&b.name, b.records, b.doc_bytes));
            }
            // the parallel scan's inventory is the same file, and so is
            // the walk that collects segments beside it
            let inv3 = crate::inventory(&data, 3).expect(name);
            assert_eq!(encode(&inv3, ID), bytes, "{}: jobs 3", name);
            let (inv_s, _) = crate::inventory_segments(&data, 2, 64).expect(name);
            assert_eq!(encode(&inv_s, ID), bytes, "{}: with segments", name);
        }
    }

    #[test]
    fn the_loader_refuses_damage() {
        let data = adversarial(false, true);
        let inv = crate::inventory(&data, 1).unwrap();
        let bytes = encode(&inv, ID);
        // every truncation, never a panic
        for cut in 0..bytes.len() {
            let e = decode(&bytes[..cut]).err().unwrap_or_else(|| panic!("a file cut at {} decoded", cut));
            let _ = format!("{}", e);
        }
        // a byte flipped anywhere in the body: the checksum
        for at in [HEADER_LEN, HEADER_LEN + 40, bytes.len() - 1] {
            let mut d = bytes.clone();
            d[at] ^= 0x55;
            let e = format!("{}", decode(&d).err().expect("damage decoded"));
            assert!(e.contains("checksum"), "{}", e);
        }
        // trailing bytes
        let mut d = bytes.clone();
        d.push(0);
        assert!(format!("{}", decode(&d).err().unwrap()).contains("body length"));
        // a foreign file and a later version
        let mut d = bytes.clone();
        d[0] = b'X';
        assert!(format!("{}", decode(&d).err().unwrap()).contains("not a FLOEINV1"));
        let mut d = bytes.clone();
        d[8] = 9;
        assert!(format!("{}", decode(&d).err().unwrap()).contains("version 9"));
        // a top past the cells: the header's field, with the checksum kept
        let mut d = bytes.clone();
        d[56..60].copy_from_slice(&(inv.cells.len() as u32).to_le_bytes());
        assert!(format!("{}", decode(&d).err().unwrap()).contains("top cell past"));
        // an edge child past the cells: the body's bytes with a fresh checksum
        let encoded_child = {
            let c = &inv.cells[inv.top];
            assert!(!c.edges.is_empty(), "the top places something");
            (c.edges[0].child as u32).to_le_bytes()
        };
        let (cs, ce) = section_range(&bytes, TAG_CELLS);
        let pos = bytes[cs..ce]
            .windows(4 + 8 + 8 + 1)
            .position(|w| w[..4] == encoded_child && w[20] != 0 && w[20] < 0x80)
            .map(|p| p + cs)
            .expect("an edge in the bytes");
        let mut d = bytes.clone();
        d[pos..pos + 4].copy_from_slice(&(inv.cells.len() as u32 + 7).to_le_bytes());
        let crc = crc32(&d[HEADER_LEN..]).to_le_bytes();
        d[24..28].copy_from_slice(&crc);
        let e = format!("{}", decode(&d).err().expect("bad child decoded"));
        assert!(e.contains("past"), "{}", e);
    }

    #[test]
    fn crc32_matches_the_reference_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn the_file_round_trips_through_the_disk() {
        let dir = std::env::temp_dir().join(format!("floe-invfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        let inv = crate::inventory(&adversarial(true, true), 2).unwrap();
        let n = write(&path, &inv, ID).unwrap();
        assert_eq!(n, std::fs::metadata(&path).unwrap().len() as usize);
        assert!(!dir.join("design.inv.tmp").exists());
        let (id, back) = read(&path).unwrap();
        assert_eq!(id, ID);
        assert_eq!(encode(&back, id), std::fs::read(&path).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
