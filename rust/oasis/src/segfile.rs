//! `design.seg` — the segment directory (docs/DIRECT_READ_PLAN.ko.md
//! §5.1, §5.2; step L2): every CBLOCK inside a cell and every span
//! between them, with the modal snapshot a parser needs to read it on
//! its own and the per-layer page statistics the planner reads. The
//! inventory pass collects it (segments.rs, `inventory_segments`); the
//! streaming build writes it beside design.inv; `floe-index inventory
//! --seg PATH` writes it alone and `--load-seg PATH` reads it back.
//!
//! Format FLOESEG1 (little-endian):
//!
//! - header 80 B: magic `FLOESEG1`, version u32 (1), header_len u32
//!   (80), body_len u64, body_crc32 u32, flags u32 (0), src_size u64,
//!   src_mtime u64 (the source's identity, as design.inv and
//!   design.ovm carry it), inv_crc u32 (the body CRC of the design.inv
//!   this directory belongs to: the pair key), n_cells u32, n_layers
//!   u32, n_segments u32, span_bytes u64, n_layer_rows u64.
//! - body: sections `tag u32, len u64, payload` in this order:
//!   CELLS (1): n_cells × (first u32, count u32) - a cell's segments
//!   are one run of the table, in file order; SEGS (2): n_segments ×
//!   112 B rows sorted by (cell, offset); LAYERS (3): n_layer_rows ×
//!   80 B rows, a segment's rows at its `layers_first`, in (layer,
//!   datatype) order; SNAPS (4): the snapshot pool - a segment's bytes
//!   at its `snapshot_off`, `snapshot_len` (0: the default snapshot,
//!   a cell's first segment), varint-encoded (see `encode_snapshot`).
//!
//! The decoder checks every length, count, index and range (a row's
//! cell against the cell table, its layer rows and snapshot against
//! the pools, a layer index against the layer count, a segment's bytes
//! against the source size) and the body's CRC, and refuses a file
//! that fails any of them with a position.

use crate::geom::Box4;
use crate::inventory::Inventory;
use crate::invfile::{crc32, Identity};
use crate::segments::{Carried, PlaceModal, SegStat, Snapshot, TextModal};
use crate::{err, Result};
use std::collections::HashMap;
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"FLOESEG1";
pub const VERSION: u32 = 1;
pub const FILE_NAME: &str = "design.seg";
const HEADER_LEN: usize = 80;
const SEG_ROW: usize = 112;
const LAYER_ROW: usize = 80;

const TAG_CELLS: u32 = 1;
const TAG_SEGS: u32 = 2;
const TAG_LAYERS: u32 = 3;
const TAG_SNAPS: u32 = 4;

/// SegRow::flags
pub const F_CBLOCK: u8 = 1;
pub const F_PLACEMENTS: u8 = 2;
pub const F_TEXTS: u8 = 4;
pub const F_CELLS_INSIDE: u8 = 8;
pub const F_CHAIN_REP: u8 = 16;
pub const F_CHAIN_POLY: u8 = 32;
pub const F_CHAIN_PATH: u8 = 64;

/// one segment of the table
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegRow {
    /// index into the inventory's cells
    pub cell: u32,
    pub flags: u8,
    pub offset: u64,
    pub bytes: u64,
    pub inflated: u64,
    pub records: u32,
    pub shapes: u32,
    pub members: u64,
    pub placements: u32,
    pub texts: u32,
    /// EMPTY_BOX without a shape
    pub bbox: Box4,
    pub layers_first: u32,
    pub layers_count: u32,
    pub snapshot_off: u32,
    pub snapshot_len: u32,
    pub cells_inside: u32,
    pub chain_list_max: u32,
}

impl SegRow {
    pub fn is_cblock(&self) -> bool {
        self.flags & F_CBLOCK != 0
    }
}

/// one (segment, layer) row: the page statistics
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerRow {
    /// index into the inventory's layer_order
    pub layer_idx: u32,
    pub records: u64,
    pub members: u64,
    pub bbox: Box4,
    pub max_w: i64,
    pub max_h: i64,
    pub max_min: i64,
}

/// the directory in memory: what the file holds
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegFile {
    pub inv_crc: u32,
    pub n_cells: u32,
    pub n_layers: u32,
    pub span_bytes: u64,
    /// per cell: (first segment, count)
    pub cells: Vec<(u32, u32)>,
    pub segs: Vec<SegRow>,
    pub layers: Vec<LayerRow>,
    pub snaps: Vec<u8>,
}

// ------------------------------------------------------------ varints

fn put_uvar(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn put_svar(out: &mut Vec<u8>, v: i64) {
    put_uvar(out, ((v << 1) ^ (v >> 63)) as u64);
}

struct Vr<'a> {
    d: &'a [u8],
    pos: usize,
    base: usize,
}

impl<'a> Vr<'a> {
    fn at(&self) -> usize {
        self.base + self.pos
    }
    fn u8(&mut self) -> Result<u8> {
        match self.d.get(self.pos) {
            Some(b) => {
                self.pos += 1;
                Ok(*b)
            }
            None => err(self.at(), "design.seg: snapshot truncated"),
        }
    }
    fn uvar(&mut self) -> Result<u64> {
        let mut v = 0u64;
        let mut shift = 0u32;
        loop {
            let b = self.u8()?;
            if shift >= 64 {
                return err(self.at(), "design.seg: varint overflow");
            }
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
        }
    }
    fn svar(&mut self) -> Result<i64> {
        let u = self.uvar()?;
        Ok(((u >> 1) as i64) ^ -((u & 1) as i64))
    }
    fn bytes(&mut self) -> Result<Vec<u8>> {
        let n = self.uvar()? as usize;
        if n > self.d.len() - self.pos {
            return err(self.at(), "design.seg: snapshot bytes past the end");
        }
        let v = self.d[self.pos..self.pos + n].to_vec();
        self.pos += n;
        Ok(v)
    }
    fn carried(&mut self) -> Result<Carried> {
        match self.u8()? {
            0 => Ok(Carried::None),
            1 => Ok(Carried::Inline(self.bytes()?)),
            2 => Ok(Carried::Ref {
                seg: self.uvar()? as u32,
                off: self.uvar()? as u32,
                len: self.uvar()? as u32,
            }),
            _ => err(self.at(), "design.seg: carried-field tag"),
        }
    }
    fn done(&self) -> Result<()> {
        if self.pos != self.d.len() {
            return err(self.at(), "design.seg: snapshot has bytes left over");
        }
        Ok(())
    }
}

fn put_carried(out: &mut Vec<u8>, c: &Carried) {
    match c {
        Carried::None => out.push(0),
        Carried::Inline(b) => {
            out.push(1);
            put_uvar(out, b.len() as u64);
            out.extend_from_slice(b);
        }
        Carried::Ref { seg, off, len } => {
            out.push(2);
            put_uvar(out, *seg as u64);
            put_uvar(out, *off as u64);
            put_uvar(out, *len as u64);
        }
    }
}

const P_RELATIVE: u32 = 1 << 0;
const P_LAYER: u32 = 1 << 1;
const P_DATATYPE: u32 = 1 << 2;
const P_TEXTLAYER: u32 = 1 << 3;
const P_TEXTTYPE: u32 = 1 << 4;
const P_GEO_W: u32 = 1 << 5;
const P_GEO_H: u32 = 1 << 6;
const P_CIRCLE_R: u32 = 1 << 7;
const P_CTRAP: u32 = 1 << 8;
const P_PATH_HW: u32 = 1 << 9;
const P_PATH_ES: u32 = 1 << 10;
const P_PATH_EE: u32 = 1 << 11;
const P_PLACE: u32 = 1 << 12;
const P_REP: u32 = 1 << 13;
const P_POLY: u32 = 1 << 14;
const P_PATH: u32 = 1 << 15;
const P_TEXT: u32 = 1 << 16;

/// a snapshot's bytes: a presence mask (uvarint), the six coordinates
/// (svarints), then the present fields in mask order; the default
/// snapshot encodes to nothing (a cell's first segment)
pub fn encode_snapshot(s: &Snapshot) -> Vec<u8> {
    if s.is_default() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(48);
    let mut mask = 0u32;
    let set = |mask: &mut u32, bit: u32, on: bool| {
        if on {
            *mask |= bit;
        }
    };
    set(&mut mask, P_RELATIVE, s.relative);
    set(&mut mask, P_LAYER, s.layer.is_some());
    set(&mut mask, P_DATATYPE, s.datatype.is_some());
    set(&mut mask, P_TEXTLAYER, s.textlayer.is_some());
    set(&mut mask, P_TEXTTYPE, s.texttype.is_some());
    set(&mut mask, P_GEO_W, s.geo_w.is_some());
    set(&mut mask, P_GEO_H, s.geo_h.is_some());
    set(&mut mask, P_CIRCLE_R, s.circle_r.is_some());
    set(&mut mask, P_CTRAP, s.ctrap_type.is_some());
    set(&mut mask, P_PATH_HW, s.path_hw.is_some());
    set(&mut mask, P_PATH_ES, s.path_es.is_some());
    set(&mut mask, P_PATH_EE, s.path_ee.is_some());
    set(&mut mask, P_PLACE, s.place.is_some());
    set(&mut mask, P_REP, s.rep != Carried::None);
    set(&mut mask, P_POLY, s.poly != Carried::None);
    set(&mut mask, P_PATH, s.path != Carried::None);
    set(&mut mask, P_TEXT, s.text != TextModal::None);
    put_uvar(&mut out, mask as u64);
    for v in [s.geo_x, s.geo_y, s.pl_x, s.pl_y, s.tx_x, s.tx_y] {
        put_svar(&mut out, v);
    }
    for v in [s.layer, s.datatype, s.textlayer, s.texttype].into_iter().flatten() {
        put_uvar(&mut out, v);
    }
    for v in [s.geo_w, s.geo_h, s.circle_r].into_iter().flatten() {
        put_svar(&mut out, v);
    }
    if let Some(t) = s.ctrap_type {
        put_uvar(&mut out, t);
    }
    for v in [s.path_hw, s.path_es, s.path_ee].into_iter().flatten() {
        put_svar(&mut out, v);
    }
    match &s.place {
        Some(PlaceModal::Ref(r)) => {
            out.push(0);
            put_uvar(&mut out, *r);
        }
        Some(PlaceModal::Name(n)) => {
            out.push(1);
            put_uvar(&mut out, n.len() as u64);
            out.extend_from_slice(n.as_bytes());
        }
        None => {}
    }
    for c in [&s.rep, &s.poly, &s.path] {
        if *c != Carried::None {
            put_carried(&mut out, c);
        }
    }
    match &s.text {
        TextModal::None => {}
        TextModal::Ref(r) => {
            out.push(0);
            put_uvar(&mut out, *r);
        }
        TextModal::Inline(c) => {
            out.push(1);
            put_carried(&mut out, c);
        }
    }
    out
}

/// the snapshot `bytes` encode (`base`: where they are in the file,
/// for messages)
pub fn decode_snapshot(bytes: &[u8], base: usize) -> Result<Snapshot> {
    if bytes.is_empty() {
        return Ok(Snapshot::default());
    }
    let mut r = Vr {
        d: bytes,
        pos: 0,
        base,
    };
    let mask = r.uvar()? as u32;
    let mut s = Snapshot {
        relative: mask & P_RELATIVE != 0,
        ..Snapshot::default()
    };
    s.geo_x = r.svar()?;
    s.geo_y = r.svar()?;
    s.pl_x = r.svar()?;
    s.pl_y = r.svar()?;
    s.tx_x = r.svar()?;
    s.tx_y = r.svar()?;
    if mask & P_LAYER != 0 {
        s.layer = Some(r.uvar()?);
    }
    if mask & P_DATATYPE != 0 {
        s.datatype = Some(r.uvar()?);
    }
    if mask & P_TEXTLAYER != 0 {
        s.textlayer = Some(r.uvar()?);
    }
    if mask & P_TEXTTYPE != 0 {
        s.texttype = Some(r.uvar()?);
    }
    if mask & P_GEO_W != 0 {
        s.geo_w = Some(r.svar()?);
    }
    if mask & P_GEO_H != 0 {
        s.geo_h = Some(r.svar()?);
    }
    if mask & P_CIRCLE_R != 0 {
        s.circle_r = Some(r.svar()?);
    }
    if mask & P_CTRAP != 0 {
        s.ctrap_type = Some(r.uvar()?);
    }
    if mask & P_PATH_HW != 0 {
        s.path_hw = Some(r.svar()?);
    }
    if mask & P_PATH_ES != 0 {
        s.path_es = Some(r.svar()?);
    }
    if mask & P_PATH_EE != 0 {
        s.path_ee = Some(r.svar()?);
    }
    if mask & P_PLACE != 0 {
        s.place = Some(match r.u8()? {
            0 => PlaceModal::Ref(r.uvar()?),
            1 => {
                let b = r.bytes()?;
                match String::from_utf8(b) {
                    Ok(n) => PlaceModal::Name(n),
                    Err(_) => return err(r.at(), "design.seg: placement name is not UTF-8"),
                }
            }
            _ => return err(r.at(), "design.seg: placement tag"),
        });
    }
    if mask & P_REP != 0 {
        s.rep = r.carried()?;
    }
    if mask & P_POLY != 0 {
        s.poly = r.carried()?;
    }
    if mask & P_PATH != 0 {
        s.path = r.carried()?;
    }
    if mask & P_TEXT != 0 {
        s.text = match r.u8()? {
            0 => TextModal::Ref(r.uvar()?),
            1 => TextModal::Inline(r.carried()?),
            _ => return err(r.at(), "design.seg: text tag"),
        };
    }
    r.done()?;
    if s.is_default() {
        return err(base, "design.seg: a default snapshot spelled out");
    }
    Ok(s)
}

// ------------------------------------------------------------ building

impl SegFile {
    /// the directory of the segments `segs` collected with `inv` (the
    /// same walk: the collector's cell numbers are the scan's CELL
    /// records in file order, resolved to the inventory's cells by
    /// name; its layers are resolved to the inventory's layer order)
    pub fn from_segments(segs: &[SegStat], inv: &Inventory, span_bytes: u64) -> Result<SegFile> {
        let index = inv.cell_index();
        let lidx: HashMap<(u32, u32), u32> = inv
            .layer_order
            .iter()
            .enumerate()
            .map(|(i, &k)| (k, i as u32))
            .collect();
        // the inventory's cell of each segment
        let mut cell_of = Vec::with_capacity(segs.len());
        for (i, s) in segs.iter().enumerate() {
            let name = match inv.scan.cell_stats.get(s.cell as usize) {
                Some(c) => &c.name,
                None => return err(i, "design.seg: segment of a cell the scan has no record for"),
            };
            match index.get(name) {
                Some(&ci) => cell_of.push(ci as u32),
                None => return err(i, &format!("design.seg: segment of an unknown cell {:?}", name)),
            }
        }
        // sorted by (cell, offset): the order of the table
        let mut order: Vec<usize> = (0..segs.len()).collect();
        order.sort_by_key(|&i| (cell_of[i], segs[i].offset, i));
        let mut perm = vec![0u32; segs.len()];
        for (new, &old) in order.iter().enumerate() {
            perm[old] = new as u32;
        }
        let n_cells = inv.cells.len();
        let mut cells = vec![(0u32, 0u32); n_cells];
        let mut rows = Vec::with_capacity(segs.len());
        let mut layers = Vec::new();
        let mut snaps = Vec::new();
        for &old in &order {
            let s = &segs[old];
            let ci = cell_of[old] as usize;
            if cells[ci].1 == 0 {
                cells[ci].0 = rows.len() as u32;
            }
            cells[ci].1 += 1;
            let layers_first = layers.len() as u32;
            for l in &s.layer_stats {
                let key = (l.layer.0 as u32, l.layer.1 as u32);
                let layer_idx = match lidx.get(&key) {
                    Some(&i) => i,
                    None => {
                        return err(
                            s.offset as usize,
                            &format!("design.seg: layer {}/{} not in the inventory", key.0, key.1),
                        )
                    }
                };
                layers.push(LayerRow {
                    layer_idx,
                    records: l.records,
                    members: l.members,
                    bbox: l.bbox,
                    max_w: l.max_w,
                    max_h: l.max_h,
                    max_min: l.max_min,
                });
            }
            // the snapshot's references follow the table's order
            let mut snap = s.snapshot.clone();
            for c in [&mut snap.rep, &mut snap.poly, &mut snap.path] {
                if let Carried::Ref { seg, .. } = c {
                    *seg = *perm.get(*seg as usize).unwrap_or(seg);
                }
            }
            if let TextModal::Inline(Carried::Ref { seg, .. }) = &mut snap.text {
                *seg = *perm.get(*seg as usize).unwrap_or(seg);
            }
            let sb = encode_snapshot(&snap);
            let snapshot_off = snaps.len() as u32;
            snaps.extend_from_slice(&sb);
            let mut flags = 0u8;
            if s.cblock {
                flags |= F_CBLOCK;
            }
            if s.placements > 0 {
                flags |= F_PLACEMENTS;
            }
            if s.texts > 0 {
                flags |= F_TEXTS;
            }
            if s.cells_inside > 0 {
                flags |= F_CELLS_INSIDE;
            }
            if s.chained & crate::segments::CHAIN_REP != 0 {
                flags |= F_CHAIN_REP;
            }
            if s.chained & crate::segments::CHAIN_POLY != 0 {
                flags |= F_CHAIN_POLY;
            }
            if s.chained & crate::segments::CHAIN_PATH != 0 {
                flags |= F_CHAIN_PATH;
            }
            rows.push(SegRow {
                cell: ci as u32,
                flags,
                offset: s.offset,
                bytes: s.bytes,
                inflated: s.inflated,
                records: narrow(s.records, "records")?,
                shapes: narrow(s.shapes, "shapes")?,
                members: s.members,
                placements: narrow(s.placements, "placements")?,
                texts: narrow(s.texts, "texts")?,
                bbox: s.bbox,
                layers_first,
                layers_count: s.layer_stats.len() as u32,
                snapshot_off,
                snapshot_len: sb.len() as u32,
                cells_inside: s.cells_inside,
                chain_list_max: s.chain_list_max.min(u32::MAX as u64) as u32,
            });
        }
        Ok(SegFile {
            inv_crc: 0,
            n_cells: n_cells as u32,
            n_layers: inv.layer_order.len() as u32,
            span_bytes,
            cells,
            segs: rows,
            layers,
            snaps,
        })
    }

    /// the segments of cell `ci` (table order = file order)
    pub fn cell_segments(&self, ci: usize) -> &[SegRow] {
        match self.cells.get(ci) {
            Some(&(first, count)) => &self.segs[first as usize..(first + count) as usize],
            None => &[],
        }
    }

    /// the layer rows of segment `i`
    pub fn seg_layers(&self, i: usize) -> &[LayerRow] {
        let s = &self.segs[i];
        &self.layers[s.layers_first as usize..(s.layers_first + s.layers_count) as usize]
    }

    /// the snapshot of segment `i`
    pub fn snapshot(&self, i: usize) -> Result<Snapshot> {
        let s = &self.segs[i];
        let (a, b) = (s.snapshot_off as usize, (s.snapshot_off + s.snapshot_len) as usize);
        decode_snapshot(&self.snaps[a..b], a)
    }
}

fn narrow(v: u64, what: &str) -> Result<u32> {
    u32::try_from(v).map_err(|_| crate::OasisError::Format(format!("design.seg: {} past u32", what)))
}

// ------------------------------------------------------------ bytes

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
    fn bx(&mut self, b: &Box4) {
        self.i64(b.0);
        self.i64(b.1);
        self.i64(b.2);
        self.i64(b.3);
    }
    fn section(&mut self, tag: u32, payload: Wr) {
        self.u32(tag);
        self.u64(payload.0.len() as u64);
        self.0.extend_from_slice(&payload.0);
    }
}

/// the file's bytes for `f` describing the source `id`
pub fn encode(f: &SegFile, id: Identity) -> Vec<u8> {
    let mut cells = Wr(Vec::with_capacity(f.cells.len() * 8));
    for &(first, count) in &f.cells {
        cells.u32(first);
        cells.u32(count);
    }
    let mut segs = Wr(Vec::with_capacity(f.segs.len() * SEG_ROW));
    for s in &f.segs {
        let at = segs.0.len();
        segs.u32(s.cell);
        segs.u8(s.flags);
        segs.u8(0);
        segs.u8(0);
        segs.u8(0);
        segs.u64(s.offset);
        segs.u64(s.bytes);
        segs.u64(s.inflated);
        segs.u32(s.records);
        segs.u32(s.shapes);
        segs.u64(s.members);
        segs.u32(s.placements);
        segs.u32(s.texts);
        segs.bx(&s.bbox);
        segs.u32(s.layers_first);
        segs.u32(s.layers_count);
        segs.u32(s.snapshot_off);
        segs.u32(s.snapshot_len);
        segs.u32(s.cells_inside);
        segs.u32(s.chain_list_max);
        debug_assert_eq!(segs.0.len() - at, SEG_ROW);
    }
    let mut layers = Wr(Vec::with_capacity(f.layers.len() * LAYER_ROW));
    for l in &f.layers {
        let at = layers.0.len();
        layers.u32(l.layer_idx);
        layers.u32(0);
        layers.u64(l.records);
        layers.u64(l.members);
        layers.bx(&l.bbox);
        layers.i64(l.max_w);
        layers.i64(l.max_h);
        layers.i64(l.max_min);
        debug_assert_eq!(layers.0.len() - at, LAYER_ROW);
    }
    let mut body = Wr(Vec::new());
    body.section(TAG_CELLS, cells);
    body.section(TAG_SEGS, segs);
    body.section(TAG_LAYERS, layers);
    body.section(TAG_SNAPS, Wr(f.snaps.clone()));
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
    h.u32(f.inv_crc);
    h.u32(f.n_cells);
    h.u32(f.n_layers);
    h.u32(f.segs.len() as u32);
    h.u64(f.span_bytes);
    h.u64(f.layers.len() as u64);
    debug_assert_eq!(h.0.len(), HEADER_LEN);
    h.0.extend_from_slice(&body.0);
    h.0
}

struct Rd<'a> {
    d: &'a [u8],
    pos: usize,
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
            return err(self.at(), "design.seg: truncated");
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
    fn bx(&mut self) -> Result<Box4> {
        Ok((self.i64()?, self.i64()?, self.i64()?, self.i64()?))
    }
    fn section(&mut self, tag: u32) -> Result<Rd<'a>> {
        let t = self.u32()?;
        if t != tag {
            return err(self.at(), &format!("design.seg: section {} where {} belongs", t, tag));
        }
        let len = self.u64()?;
        if len > (self.d.len() - self.pos) as u64 {
            return err(self.at(), "design.seg: section past the end");
        }
        let base = self.at();
        let payload = self.take(len as usize)?;
        Ok(Rd::new(payload, base))
    }
    fn done(&self, what: &str) -> Result<()> {
        if self.pos != self.d.len() {
            return err(self.at(), &format!("design.seg: {} has bytes left over", what));
        }
        Ok(())
    }
}

/// the header alone: the identity, the pair key and the counts
pub fn peek(bytes: &[u8]) -> Result<(Identity, u32, u32, u32, u32)> {
    if bytes.len() < HEADER_LEN {
        return err(0, "design.seg: shorter than its header");
    }
    if &bytes[..8] != MAGIC {
        return err(0, "design.seg: not a FLOESEG1 file");
    }
    let mut r = Rd::new(&bytes[8..HEADER_LEN], 8);
    let version = r.u32()?;
    if version != VERSION {
        return err(8, &format!("design.seg: version {} (this build reads {})", version, VERSION));
    }
    if r.u32()? as usize != HEADER_LEN {
        return err(12, "design.seg: header length");
    }
    let body_len = r.u64()?;
    let _crc = r.u32()?;
    let _flags = r.u32()?;
    let id = Identity {
        src_size: r.u64()?,
        src_mtime: r.u64()?,
    };
    let inv_crc = r.u32()?;
    let n_cells = r.u32()?;
    let n_layers = r.u32()?;
    let n_segments = r.u32()?;
    if body_len != (bytes.len() - HEADER_LEN) as u64 {
        return err(16, "design.seg: body length differs from the file's");
    }
    Ok((id, inv_crc, n_cells, n_layers, n_segments))
}

/// the directory the bytes hold, after every check (see the module notes)
pub fn decode(bytes: &[u8]) -> Result<(Identity, SegFile)> {
    let (id, inv_crc, n_cells, n_layers, n_segments) = peek(bytes)?;
    let mut h = Rd::new(&bytes[24..HEADER_LEN], 24);
    let crc = h.u32()?;
    let _flags = h.u32()?;
    let _size = h.u64()?;
    let _mtime = h.u64()?;
    let _inv_crc = h.u32()?;
    let _nc = h.u32()?;
    let _nl = h.u32()?;
    let _ns = h.u32()?;
    let span_bytes = h.u64()?;
    let n_layer_rows = h.u64()?;
    let body = &bytes[HEADER_LEN..];
    if crc32(body) != crc {
        return err(HEADER_LEN, "design.seg: body checksum differs (truncated or damaged)");
    }
    let mut r = Rd::new(body, HEADER_LEN);
    // CELLS
    let mut cells = Vec::with_capacity(n_cells as usize);
    {
        let mut s = r.section(TAG_CELLS)?;
        if s.d.len() != n_cells as usize * 8 {
            return err(s.at(), "design.seg: cell table size");
        }
        let mut next = 0u32;
        for _ in 0..n_cells {
            let first = s.u32()?;
            let count = s.u32()?;
            if count > 0 {
                if first != next {
                    return err(s.at(), "design.seg: a cell's segments are not the next run");
                }
                next = match first.checked_add(count) {
                    Some(v) if v <= n_segments => v,
                    _ => return err(s.at(), "design.seg: a cell's run past the segments"),
                };
            }
            cells.push((first, count));
        }
        if next != n_segments {
            return err(s.at(), "design.seg: segments outside every cell's run");
        }
        s.done("CELLS")?;
    }
    // SEGS
    let mut segs = Vec::with_capacity(n_segments as usize);
    {
        let mut s = r.section(TAG_SEGS)?;
        if s.d.len() != n_segments as usize * SEG_ROW {
            return err(s.at(), "design.seg: segment table size");
        }
        let mut layers_next = 0u64;
        let mut snaps_next = 0u64;
        for i in 0..n_segments as usize {
            let at = s.at();
            let cell = s.u32()?;
            let flags = s.u8()?;
            let _ = s.take(3)?;
            let offset = s.u64()?;
            let bytes_ = s.u64()?;
            let inflated = s.u64()?;
            let records = s.u32()?;
            let shapes = s.u32()?;
            let members = s.u64()?;
            let placements = s.u32()?;
            let texts = s.u32()?;
            let bbox = s.bx()?;
            let layers_first = s.u32()?;
            let layers_count = s.u32()?;
            let snapshot_off = s.u32()?;
            let snapshot_len = s.u32()?;
            let cells_inside = s.u32()?;
            let chain_list_max = s.u32()?;
            if cell >= n_cells {
                return err(at, "design.seg: segment cell past the cell count");
            }
            let (first, count) = cells[cell as usize];
            if (i as u32) < first || i as u32 >= first + count {
                return err(at, "design.seg: segment outside its cell's run");
            }
            if layers_first as u64 != layers_next {
                return err(at, "design.seg: layer rows out of order");
            }
            layers_next += layers_count as u64;
            if snapshot_off as u64 != snaps_next {
                return err(at, "design.seg: snapshots out of order");
            }
            snaps_next += snapshot_len as u64;
            if offset.checked_add(bytes_).map_or(true, |e| e > id.src_size) || bytes_ == 0 {
                return err(at, "design.seg: segment bytes outside the source");
            }
            if flags & F_CBLOCK == 0 && inflated != bytes_ {
                return err(at, "design.seg: a span's inflated bytes differ from its bytes");
            }
            segs.push(SegRow {
                cell,
                flags,
                offset,
                bytes: bytes_,
                inflated,
                records,
                shapes,
                members,
                placements,
                texts,
                bbox,
                layers_first,
                layers_count,
                snapshot_off,
                snapshot_len,
                cells_inside,
                chain_list_max,
            });
        }
        if layers_next != n_layer_rows {
            return err(s.at(), "design.seg: layer rows differ from the header's count");
        }
        s.done("SEGS")?;
    }
    // LAYERS
    let mut layers = Vec::with_capacity(n_layer_rows as usize);
    {
        let mut s = r.section(TAG_LAYERS)?;
        if s.d.len() as u64 != n_layer_rows * LAYER_ROW as u64 {
            return err(s.at(), "design.seg: layer table size");
        }
        for _ in 0..n_layer_rows {
            let at = s.at();
            let layer_idx = s.u32()?;
            let _ = s.u32()?;
            let records = s.u64()?;
            let members = s.u64()?;
            let bbox = s.bx()?;
            let max_w = s.i64()?;
            let max_h = s.i64()?;
            let max_min = s.i64()?;
            if layer_idx >= n_layers {
                return err(at, "design.seg: layer index past the layer count");
            }
            layers.push(LayerRow {
                layer_idx,
                records,
                members,
                bbox,
                max_w,
                max_h,
                max_min,
            });
        }
        s.done("LAYERS")?;
    }
    // SNAPS
    let snaps = {
        let s = r.section(TAG_SNAPS)?;
        s.d.to_vec()
    };
    r.done("body")?;
    let f = SegFile {
        inv_crc,
        n_cells,
        n_layers,
        span_bytes,
        cells,
        segs,
        layers,
        snaps,
    };
    // every snapshot decodes, and references stay inside the table
    let mut end = 0usize;
    for (i, s) in f.segs.iter().enumerate() {
        end = (s.snapshot_off + s.snapshot_len) as usize;
        if end > f.snaps.len() {
            return err(HEADER_LEN, "design.seg: snapshot past the pool");
        }
        let snap = f.snapshot(i)?;
        for c in [&snap.rep, &snap.poly, &snap.path] {
            if let Carried::Ref { seg, .. } = c {
                if *seg as usize >= f.segs.len() || f.segs[*seg as usize].cell != s.cell {
                    return err(HEADER_LEN, "design.seg: a snapshot refers outside its cell");
                }
            }
        }
        if let TextModal::Inline(Carried::Ref { seg, .. }) = &snap.text {
            if *seg as usize >= f.segs.len() || f.segs[*seg as usize].cell != s.cell {
                return err(HEADER_LEN, "design.seg: a snapshot refers outside its cell");
            }
        }
    }
    if end != f.snaps.len() {
        return err(HEADER_LEN, "design.seg: snapshot pool has bytes left over");
    }
    Ok((id, f))
}

/// `encode` to `path` through `path.tmp` + rename; the byte count
pub fn write(path: &Path, f: &SegFile, id: Identity) -> std::io::Result<usize> {
    let bytes = encode(f, id);
    let tmp = path.with_extension("seg.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(bytes.len())
}

pub fn read(path: &Path) -> Result<(Identity, SegFile)> {
    let bytes = std::fs::read(path)?;
    decode(&bytes)
}

/// `bytes` decode to a directory whose encoding is `bytes` again
pub fn roundtrip(bytes: &[u8]) -> Result<()> {
    let (id, f) = decode(bytes)?;
    if encode(&f, id) != bytes {
        return err(0, "design.seg: the decoded directory encodes to other bytes");
    }
    Ok(())
}

/// the body CRC a design.inv's bytes carry (the pair key)
pub fn inv_crc_of(inv_bytes: &[u8]) -> Option<u32> {
    if inv_bytes.len() < 28 || &inv_bytes[..8] != crate::invfile::MAGIC {
        return None;
    }
    Some(u32::from_le_bytes(inv_bytes[24..28].try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::fixtures::{adversarial, head_cblock_cell};
    use crate::segments::DEFAULT_SPAN_BYTES;

    const ID: Identity = Identity {
        src_size: 1 << 20,
        src_mtime: 1_760_000_000,
    };

    fn files() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("adversarial", adversarial(false, false)),
            ("adversarial tables first", adversarial(true, false)),
            ("adversarial cblock", adversarial(false, true)),
            ("adversarial tables first + cblock", adversarial(true, true)),
            ("head cblock cell", head_cblock_cell()),
        ]
    }

    #[test]
    fn the_directory_round_trips_and_follows_the_inventory() {
        for (name, data) in files() {
            let id = Identity {
                src_size: data.len() as u64,
                ..ID
            };
            let (inv, col) = crate::inventory_segments(&data, 1, DEFAULT_SPAN_BYTES).expect(name);
            let f = SegFile::from_segments(&col.segs, &inv, DEFAULT_SPAN_BYTES).expect(name);
            let bytes = encode(&f, id);
            assert_eq!(&bytes[..8], MAGIC);
            let (id2, back) = decode(&bytes).expect(name);
            assert_eq!(id2, id);
            assert_eq!(back, f, "{}", name);
            assert_eq!(encode(&back, id2), bytes, "{}", name);
            roundtrip(&bytes).expect(name);
            // every segment under its cell, in file order, with the
            // inventory's layers; the cell table covers the table
            let mut covered = 0u32;
            for (ci, &(first, count)) in f.cells.iter().enumerate() {
                covered += count;
                let run = f.cell_segments(ci);
                assert_eq!(run.len(), count as usize);
                for w in run.windows(2) {
                    assert!(w[0].offset < w[1].offset, "{}: file order in cell {}", name, ci);
                }
                for s in run {
                    assert_eq!(s.cell as usize, ci);
                    let _ = first;
                }
            }
            assert_eq!(covered as usize, f.segs.len(), "{}", name);
            for (i, s) in f.segs.iter().enumerate() {
                for l in f.seg_layers(i) {
                    assert!((l.layer_idx as usize) < inv.layer_order.len());
                    assert!(l.records >= 1);
                }
                let sum: u64 = f.seg_layers(i).iter().map(|l| l.records).sum();
                assert_eq!(sum, s.shapes as u64, "{}: a shape is on one layer", name);
                let snap = f.snapshot(i).expect("snapshot");
                let first_of_cell = f.cells[s.cell as usize].0 as usize == i;
                if first_of_cell {
                    assert!(snap.is_default(), "{}: a cell's first segment starts from nothing", name);
                }
            }
            // the parallel walk writes the same bytes
            let (inv3, col3) = crate::inventory_segments(&data, 3, DEFAULT_SPAN_BYTES).expect(name);
            let f3 = SegFile::from_segments(&col3.segs, &inv3, DEFAULT_SPAN_BYTES).expect(name);
            assert_eq!(encode(&f3, id), bytes, "{}: jobs 3", name);
        }
    }

    #[test]
    fn the_loader_refuses_damage() {
        let data = adversarial(false, true);
        let id = Identity {
            src_size: data.len() as u64,
            ..ID
        };
        let (inv, col) = crate::inventory_segments(&data, 1, DEFAULT_SPAN_BYTES).unwrap();
        let f = SegFile::from_segments(&col.segs, &inv, DEFAULT_SPAN_BYTES).unwrap();
        let bytes = encode(&f, id);
        for cut in 0..bytes.len() {
            let e = decode(&bytes[..cut]).err().unwrap_or_else(|| panic!("a file cut at {} decoded", cut));
            let _ = format!("{}", e);
        }
        for at in [HEADER_LEN, HEADER_LEN + 50, bytes.len() - 1] {
            let mut d = bytes.clone();
            d[at] ^= 0x55;
            let e = format!("{}", decode(&d).err().expect("damage decoded"));
            assert!(e.contains("checksum"), "{}", e);
        }
        let mut d = bytes.clone();
        d.push(0);
        assert!(format!("{}", decode(&d).err().unwrap()).contains("body length"));
        let mut d = bytes.clone();
        d[0] = b'X';
        assert!(format!("{}", decode(&d).err().unwrap()).contains("not a FLOESEG1"));
        // a segment claiming bytes past the source: the header's size
        // field, with the checksum untouched (it covers the body only)
        let mut d = bytes.clone();
        d[32..40].copy_from_slice(&(f.segs[0].offset).to_le_bytes());
        let e = format!("{}", decode(&d).err().expect("decoded past the source"));
        assert!(e.contains("outside the source"), "{}", e);
    }

    #[test]
    fn snapshots_encode_every_field() {
        let s = Snapshot {
            relative: true,
            layer: Some(7),
            datatype: Some(0),
            textlayer: Some(3),
            texttype: Some(9),
            geo_x: -12345,
            geo_y: 67890,
            pl_x: 1,
            pl_y: -1,
            tx_x: 0,
            tx_y: 1 << 40,
            geo_w: Some(10),
            geo_h: Some(-3),
            circle_r: Some(5),
            ctrap_type: Some(21),
            path_hw: Some(2),
            path_es: Some(0),
            path_ee: Some(-2),
            place: Some(PlaceModal::Name("MID".into())),
            rep: Carried::Inline(vec![4, 1, 5, 5]),
            poly: Carried::Ref {
                seg: 3,
                off: 77,
                len: 9,
            },
            path: Carried::None,
            text: TextModal::Inline(Carried::Inline(vec![3, b'l', b'b', b'l'])),
        };
        let b = encode_snapshot(&s);
        assert_eq!(decode_snapshot(&b, 0).unwrap(), s);
        let s2 = Snapshot {
            place: Some(PlaceModal::Ref(4)),
            text: TextModal::Ref(2),
            ..Snapshot::default()
        };
        let b2 = encode_snapshot(&s2);
        assert_eq!(decode_snapshot(&b2, 0).unwrap(), s2);
        assert!(encode_snapshot(&Snapshot::default()).is_empty());
        assert!(decode_snapshot(&[], 0).unwrap().is_default());
        assert!(decode_snapshot(&b[..b.len() - 1], 0).is_err());
    }
}
