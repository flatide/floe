//! design.ovm - the floe VFS metadata file (see rust/VFS.md).
//!
//! Packed little-endian sections: strings, layers, cells, places
//! (+pts pool), layer bitsets (deduped), per-cell instance BVH,
//! page directory. No file pointers - relative offsets and integer
//! indexes only; fixed-width records; the reader never transmutes,
//! every field is a bounds-checked LE read so the same API works
//! over read() today and mmap later.

use floe_oasis::doc::Rep;

pub const MAGIC: &[u8; 8] = b"FLOEOVM1";
pub const VERSION: u32 = 1;

pub const HEADER_LEN: usize = 184;
pub const LAYER_LEN: usize = 32;
pub const CELL_LEN: usize = 112;
pub const PLACE_LEN: usize = 64;
pub const BVH_LEN: usize = 40;
pub const PAGE_LEN: usize = 88;

pub const LOD_EXACT: u8 = 0;
/// codec 0 = plain OASIS single-cell file (CBLOCK inside)
pub const CODEC_OASIS: u8 = 0;

const SEC_STRINGS: usize = 0;
const SEC_LAYERS: usize = 1;
const SEC_CELLS: usize = 2;
const SEC_PLACES: usize = 3;
const SEC_BITSETS: usize = 4;
const SEC_BVH: usize = 5;
const SEC_PAGEDIR: usize = 6;
const N_SECTIONS: usize = 7;

// ------------------------------------------------------------ encode

fn p16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn p32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn p64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn pi64(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&v.to_le_bytes());
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BBox {
    pub x0: i64,
    pub y0: i64,
    pub x1: i64,
    pub y1: i64,
}

impl BBox {
    pub const EMPTY: BBox = BBox {
        x0: i64::MAX,
        y0: i64::MAX,
        x1: i64::MIN,
        y1: i64::MIN,
    };
    pub fn is_empty(&self) -> bool {
        self.x1 < self.x0 || self.y1 < self.y0
    }
    pub fn grow(&mut self, o: &BBox) {
        if o.is_empty() {
            return;
        }
        self.x0 = self.x0.min(o.x0);
        self.y0 = self.y0.min(o.y0);
        self.x1 = self.x1.max(o.x1);
        self.y1 = self.y1.max(o.y1);
    }
    pub fn intersects(&self, o: &BBox) -> bool {
        !self.is_empty()
            && !o.is_empty()
            && self.x0 <= o.x1
            && o.x0 <= self.x1
            && self.y0 <= o.y1
            && o.y0 <= self.y1
    }
}

fn pbox(out: &mut Vec<u8>, b: &BBox) {
    pi64(out, b.x0);
    pi64(out, b.y0);
    pi64(out, b.x1);
    pi64(out, b.y1);
}

/// Section-by-section builder. The caller appends places/BVH/pages
/// for a cell and then the cell record with the ranges it recorded.
pub struct Builder {
    pub unit: f64,
    pub src_size: u64,
    pub src_mtime: u64,
    pub top: u32,
    names: Vec<u8>,
    layers: Vec<u8>,
    n_layers: u32,
    cells: Vec<u8>,
    n_cells: u32,
    places: Vec<u8>,
    n_places: u64,
    pts_pool: Vec<u8>,
    bitsets: Vec<u8>,
    bs_map: std::collections::HashMap<Vec<u8>, u32>,
    bs_width: usize,
    bvh: Vec<u8>,
    n_bvh: u32,
    pages: Vec<u8>,
    n_pages: u32,
}

impl Builder {
    pub fn new(
        unit: f64,
        src_size: u64,
        src_mtime: u64,
        n_layers_hint: usize,
    ) -> Builder {
        Builder {
            unit,
            src_size,
            src_mtime,
            top: 0,
            names: Vec::new(),
            layers: Vec::new(),
            n_layers: 0,
            cells: Vec::new(),
            n_cells: 0,
            places: Vec::new(),
            n_places: 0,
            pts_pool: Vec::new(),
            bitsets: Vec::new(),
            bs_map: std::collections::HashMap::new(),
            bs_width: n_layers_hint.div_ceil(8).max(1),
            bvh: Vec::new(),
            n_bvh: 0,
            pages: Vec::new(),
            n_pages: 0,
        }
    }

    pub fn bitset_width(&self) -> usize {
        self.bs_width
    }

    pub fn name(&mut self, s: &str) -> (u32, u16) {
        let off = self.names.len() as u32;
        self.names.extend_from_slice(s.as_bytes());
        (off, s.len() as u16)
    }

    pub fn layer(&mut self, l: u32, d: u32, nm: &str, recs: u64, mems: u64) {
        let (no, nl) = self.name(nm);
        let out = &mut self.layers;
        p32(out, l);
        p32(out, d);
        p32(out, no);
        p16(out, nl);
        p16(out, 0);
        p64(out, recs);
        p64(out, mems);
        assert_eq!(out.len() % LAYER_LEN, 0, "layer stride");
        self.n_layers += 1;
    }

    /// dedup pool index of a layer bitset (bs_width bytes)
    pub fn bitset(&mut self, bits: &[u8]) -> u32 {
        assert_eq!(bits.len(), self.bs_width, "bitset width");
        if let Some(&i) = self.bs_map.get(bits) {
            return i;
        }
        let i = (self.bitsets.len() / self.bs_width) as u32;
        self.bitsets.extend_from_slice(bits);
        self.bs_map.insert(bits.to_vec(), i);
        i
    }

    /// appends one placement; returns its global index
    pub fn place(
        &mut self,
        child: u32,
        x: i64,
        y: i64,
        rot: u8,
        flip: bool,
        rep: &Rep,
    ) -> u64 {
        let idx = self.n_places;
        let (kind, na, nb, va, vb) = match rep {
            Rep::One => (0u8, 1u32, 1u32, (0, 0), (0, 0)),
            Rep::Grid { na, nb, va, vb } => {
                (1, *na as u32, *nb as u32, *va, *vb)
            }
            Rep::Pts(p) => {
                let off = self.pts_pool.len() as i64;
                for &(px, py) in p {
                    pi64(&mut self.pts_pool, px);
                    pi64(&mut self.pts_pool, py);
                }
                (2, p.len() as u32, 1, (off, 0), (0, 0))
            }
        };
        let out = &mut self.places;
        p32(out, child);
        out.push(rot);
        out.push(flip as u8);
        out.push(kind);
        out.push(0);
        pi64(out, x);
        pi64(out, y);
        p32(out, na);
        p32(out, nb);
        pi64(out, va.0);
        pi64(out, va.1);
        pi64(out, vb.0);
        pi64(out, vb.1);
        assert_eq!(out.len() % PLACE_LEN, 0, "place stride");
        self.n_places += 1;
        idx
    }

    /// appends one BVH node; returns its global index
    pub fn bvh_node(
        &mut self,
        bbox: &BBox,
        first: u32,
        count: u16,
        leaf: bool,
    ) -> u32 {
        let idx = self.n_bvh;
        let out = &mut self.bvh;
        pbox(out, bbox);
        p32(out, first);
        p16(out, count);
        p16(out, leaf as u16);
        assert_eq!(out.len() % BVH_LEN, 0, "bvh stride");
        self.n_bvh += 1;
        idx
    }

    pub fn n_bvh(&self) -> u32 {
        self.n_bvh
    }
    pub fn n_places(&self) -> u64 {
        self.n_places
    }
    pub fn n_pages(&self) -> u32 {
        self.n_pages
    }

    #[allow(clippy::too_many_arguments)]
    pub fn page(
        &mut self,
        cell: u32,
        layer_idx: u32,
        seq: u16,
        bbox: &BBox,
        file_off: u64,
        csize: u32,
        usize_: u32,
        records: u32,
        members: u64,
        max_w: u32,
        max_h: u32,
    ) {
        let out = &mut self.pages;
        p32(out, cell);
        p32(out, layer_idx);
        p16(out, seq);
        out.push(LOD_EXACT);
        out.push(CODEC_OASIS);
        p32(out, 0);
        pbox(out, bbox);
        p64(out, file_off);
        p32(out, csize);
        p32(out, usize_);
        p32(out, records);
        p32(out, 0);
        p64(out, members);
        p32(out, max_w);
        p32(out, max_h);
        assert_eq!(out.len() % PAGE_LEN, 0, "page stride");
        self.n_pages += 1;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn cell(
        &mut self,
        nm: &str,
        height: u16,
        dbbox: &BBox,
        rbbox: &BBox,
        place_start: u32,
        place_count: u32,
        page_start: u32,
        page_count: u32,
        bvh_start: u32,
        bvh_count: u32,
        lmask_direct: u32,
        lmask_rec: u32,
        rec_members: u64,
    ) {
        let (no, nl) = self.name(nm);
        let out = &mut self.cells;
        p32(out, no);
        p16(out, nl);
        p16(out, height);
        pbox(out, dbbox);
        pbox(out, rbbox);
        p32(out, place_start);
        p32(out, place_count);
        p32(out, page_start);
        p32(out, page_count);
        p32(out, bvh_start);
        p32(out, bvh_count);
        p32(out, lmask_direct);
        p32(out, lmask_rec);
        p64(out, rec_members);
        assert_eq!(out.len() % CELL_LEN, 0, "cell stride");
        self.n_cells += 1;
    }

    pub fn finish(mut self) -> Vec<u8> {
        // pts pool rides at the tail of the places section
        self.places.extend_from_slice(&self.pts_pool);
        let secs: [&[u8]; N_SECTIONS] = [
            &self.names,
            &self.layers,
            &self.cells,
            &self.places,
            &self.bitsets,
            &self.bvh,
            &self.pages,
        ];
        let mut out = Vec::with_capacity(
            HEADER_LEN + secs.iter().map(|s| s.len()).sum::<usize>(),
        );
        out.extend_from_slice(MAGIC);
        p32(&mut out, VERSION);
        p32(&mut out, self.bs_width as u32); // flags slot: bitset width
        out.extend_from_slice(&self.unit.to_le_bytes());
        p64(&mut out, self.src_size);
        p64(&mut out, self.src_mtime);
        p32(&mut out, self.top);
        p32(&mut out, self.n_layers);
        p32(&mut out, self.n_cells);
        p32(&mut out, self.n_pages);
        p64(&mut out, self.n_places);
        p32(&mut out, self.n_bvh);
        p32(&mut out, 0);
        let mut off = HEADER_LEN as u64;
        for s in secs {
            p64(&mut out, off);
            p64(&mut out, s.len() as u64);
            off += s.len() as u64;
        }
        assert_eq!(out.len(), HEADER_LEN, "header layout");
        for s in [
            &self.names,
            &self.layers,
            &self.cells,
            &self.places,
            &self.bitsets,
            &self.bvh,
            &self.pages,
        ] {
            out.extend_from_slice(s);
        }
        out
    }
}

// ------------------------------------------------------------ decode

fn g16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().expect("ovm: short u16"))
}
fn g32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().expect("ovm: short u32"))
}
fn g64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().expect("ovm: short u64"))
}
fn gi64(b: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(b[o..o + 8].try_into().expect("ovm: short i64"))
}
fn gbox(b: &[u8], o: usize) -> BBox {
    BBox {
        x0: gi64(b, o),
        y0: gi64(b, o + 8),
        x1: gi64(b, o + 16),
        y1: gi64(b, o + 24),
    }
}

#[derive(Debug, Clone)]
pub struct LayerV {
    pub layer: u32,
    pub dt: u32,
    pub name: String,
    pub records: u64,
    pub members: u64,
}

#[derive(Debug, Clone)]
pub struct CellV {
    pub name: String,
    pub height: u16,
    pub dbbox: BBox,
    pub rbbox: BBox,
    pub place_start: u32,
    pub place_count: u32,
    pub page_start: u32,
    pub page_count: u32,
    pub bvh_start: u32,
    pub bvh_count: u32,
    pub lmask_direct: u32,
    pub lmask_rec: u32,
    pub rec_members: u64,
}

#[derive(Debug, Clone)]
pub struct PlaceV {
    pub child: u32,
    pub rot: u8,
    pub flip: bool,
    pub x: i64,
    pub y: i64,
    pub rep: Rep,
}

#[derive(Debug, Clone, Copy)]
pub struct NodeV {
    pub bbox: BBox,
    pub first: u32,
    pub count: u16,
    pub leaf: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct PageV {
    pub cell: u32,
    pub layer_idx: u32,
    pub seq: u16,
    pub lod: u8,
    pub codec: u8,
    pub bbox: BBox,
    pub file_off: u64,
    pub csize: u32,
    pub usize_: u32,
    pub records: u32,
    pub members: u64,
    pub max_w: u32,
    pub max_h: u32,
}

pub struct Ovm {
    pub data: Vec<u8>,
    pub unit: f64,
    pub src_size: u64,
    pub src_mtime: u64,
    pub top: u32,
    pub n_layers: u32,
    pub n_cells: u32,
    pub n_pages: u32,
    pub n_places: u64,
    pub n_bvh: u32,
    pub bs_width: usize,
    secs: [(u64, u64); N_SECTIONS],
}

impl Ovm {
    pub fn from_bytes(data: Vec<u8>) -> Result<Ovm, String> {
        if data.len() < HEADER_LEN || &data[..8] != MAGIC {
            return Err("not an ovm file".into());
        }
        if g32(&data, 8) != VERSION {
            return Err(format!("ovm version {}", g32(&data, 8)));
        }
        let mut secs = [(0u64, 0u64); N_SECTIONS];
        for (i, s) in secs.iter_mut().enumerate() {
            let o = 72 + i * 16;
            *s = (g64(&data, o), g64(&data, o + 8));
            if s.0 + s.1 > data.len() as u64 {
                return Err(format!("ovm section {} out of range", i));
            }
        }
        Ok(Ovm {
            unit: f64::from_le_bytes(
                data[16..24].try_into().unwrap(),
            ),
            src_size: g64(&data, 24),
            src_mtime: g64(&data, 32),
            top: g32(&data, 40),
            n_layers: g32(&data, 44),
            n_cells: g32(&data, 48),
            n_pages: g32(&data, 52),
            n_places: g64(&data, 56),
            n_bvh: g32(&data, 64),
            bs_width: g32(&data, 12) as usize,
            secs,
            data,
        })
    }

    pub fn open(path: &str) -> Result<Ovm, String> {
        let data = std::fs::read(path)
            .map_err(|e| format!("read {}: {}", path, e))?;
        Ovm::from_bytes(data)
    }

    fn sec(&self, i: usize) -> &[u8] {
        let (o, l) = self.secs[i];
        &self.data[o as usize..(o + l) as usize]
    }

    fn nm(&self, off: u32, len: u16) -> String {
        let s = self.sec(SEC_STRINGS);
        String::from_utf8_lossy(&s[off as usize..off as usize + len as usize])
            .into_owned()
    }

    pub fn layer(&self, i: u32) -> LayerV {
        assert!(i < self.n_layers, "layer index");
        let b = &self.sec(SEC_LAYERS)[i as usize * LAYER_LEN..];
        LayerV {
            layer: g32(b, 0),
            dt: g32(b, 4),
            name: self.nm(g32(b, 8), g16(b, 12)),
            records: g64(b, 16),
            members: g64(b, 24),
        }
    }

    pub fn cell(&self, i: u32) -> CellV {
        assert!(i < self.n_cells, "cell index");
        let b = &self.sec(SEC_CELLS)[i as usize * CELL_LEN..];
        CellV {
            name: self.nm(g32(b, 0), g16(b, 4)),
            height: g16(b, 6),
            dbbox: gbox(b, 8),
            rbbox: gbox(b, 40),
            place_start: g32(b, 72),
            place_count: g32(b, 76),
            page_start: g32(b, 80),
            page_count: g32(b, 84),
            bvh_start: g32(b, 88),
            bvh_count: g32(b, 92),
            lmask_direct: g32(b, 96),
            lmask_rec: g32(b, 100),
            rec_members: g64(b, 104),
        }
    }

    pub fn place(&self, i: u64) -> PlaceV {
        assert!(i < self.n_places, "place index");
        let sec = self.sec(SEC_PLACES);
        let b = &sec[i as usize * PLACE_LEN..];
        let kind = b[6];
        let na = g32(b, 24);
        let nb = g32(b, 28);
        let va = (gi64(b, 32), gi64(b, 40));
        let vb = (gi64(b, 48), gi64(b, 56));
        let rep = match kind {
            0 => Rep::One,
            1 => Rep::Grid {
                na: na as u64,
                nb: nb as u64,
                va,
                vb,
            },
            2 => {
                let pool =
                    &sec[self.n_places as usize * PLACE_LEN..];
                let mut pts = Vec::with_capacity(na as usize);
                let base = va.0 as usize;
                for k in 0..na as usize {
                    pts.push((
                        gi64(pool, base + k * 16),
                        gi64(pool, base + k * 16 + 8),
                    ));
                }
                Rep::Pts(pts)
            }
            k => panic!("ovm: rep kind {}", k),
        };
        PlaceV {
            child: g32(b, 0),
            rot: b[4],
            flip: b[5] != 0,
            x: gi64(b, 8),
            y: gi64(b, 16),
            rep,
        }
    }

    pub fn bvh(&self, i: u32) -> NodeV {
        assert!(i < self.n_bvh, "bvh index");
        let b = &self.sec(SEC_BVH)[i as usize * BVH_LEN..];
        NodeV {
            bbox: gbox(b, 0),
            first: g32(b, 32),
            count: g16(b, 36),
            leaf: g16(b, 38) != 0,
        }
    }

    pub fn page(&self, i: u32) -> PageV {
        assert!(i < self.n_pages, "page index");
        let b = &self.sec(SEC_PAGEDIR)[i as usize * PAGE_LEN..];
        PageV {
            cell: g32(b, 0),
            layer_idx: g32(b, 4),
            seq: g16(b, 8),
            lod: b[10],
            codec: b[11],
            bbox: gbox(b, 16),
            file_off: g64(b, 48),
            csize: g32(b, 56),
            usize_: g32(b, 60),
            records: g32(b, 64),
            members: g64(b, 72),
            max_w: g32(b, 80),
            max_h: g32(b, 84),
        }
    }

    pub fn bitset(&self, idx: u32) -> &[u8] {
        let s = self.sec(SEC_BITSETS);
        let o = idx as usize * self.bs_width;
        &s[o..o + self.bs_width]
    }
}

/// page payload cell name - the builder and every consumer (delta
/// splicer, viewer eviction) must agree on it
pub fn page_cell_name(cell: u32, layer_idx: u32, seq: u16) -> String {
    format!("P{}_{}_{}", cell, layer_idx, seq)
}

pub fn bit_test(bits: &[u8], i: usize) -> bool {
    bits[i / 8] & (1 << (i % 8)) != 0
}

pub fn bit_set(bits: &mut [u8], i: usize) {
    bits[i / 8] |= 1 << (i % 8);
}

pub fn masks_intersect(a: &[u8], b: &[u8]) -> bool {
    a.iter().zip(b).any(|(x, y)| x & y != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut b = Builder::new(1000.0, 42, 7, 10);
        b.top = 1;
        b.layer(5, 0, "M1", 3, 9);
        let m0 = b.bitset(&[0b10, 0]);
        let m1 = b.bitset(&[0b11, 0]);
        assert_eq!(m0, b.bitset(&[0b10, 0])); // dedup
        let p0 = b.place(
            0,
            10,
            -20,
            1,
            true,
            &Rep::Grid { na: 3, nb: 2, va: (5, 0), vb: (0, 7) },
        );
        b.place(0, 1, 2, 0, false, &Rep::Pts(vec![(0, 0), (9, 9)]));
        let bb = BBox { x0: 0, y0: 0, x1: 100, y1: 50 };
        let n0 = b.bvh_node(&bb, p0 as u32, 2, true);
        b.page(1, 0, 0, &bb, 128, 10, 20, 3, 9, 60, 60);
        b.page(1, 7, 1, &bb, 138, 11, 21, 4, 8, 61, 62);
        b.cell("LEAF", 0, &bb, &bb, 0, 0, 0, 0, 0, 0, m0, m1, 9);
        b.cell("TOP", 1, &bb, &bb, p0 as u32, 2, 0, 1, n0, 1, m0, m1, 18);
        let bytes = b.finish();
        let v = Ovm::from_bytes(bytes).unwrap();
        assert_eq!(v.n_cells, 2);
        assert_eq!(v.top, 1);
        assert_eq!(v.layer(0).name, "M1");
        let c = v.cell(1);
        assert_eq!(c.name, "TOP");
        assert_eq!(c.place_count, 2);
        let pl = v.place(0);
        assert_eq!((pl.x, pl.y, pl.rot, pl.flip), (10, -20, 1, true));
        assert!(matches!(pl.rep, Rep::Grid { na: 3, nb: 2, .. }));
        let pl2 = v.place(1);
        match &pl2.rep {
            Rep::Pts(p) => assert_eq!(p, &vec![(0, 0), (9, 9)]),
            r => panic!("{:?}", r),
        }
        let n = v.bvh(0);
        assert!(n.leaf && n.count == 2);
        let pg = v.page(0);
        assert_eq!((pg.records, pg.members, pg.max_w), (3, 9, 60));
        let pg1 = v.page(1);
        assert_eq!(
            (pg1.layer_idx, pg1.seq, pg1.file_off, pg1.records,
             pg1.members, pg1.max_w, pg1.max_h),
            (7, 1, 138, 4, 8, 61, 62)
        );
        assert!(masks_intersect(v.bitset(c.lmask_direct), &[0b10, 0]));
    }
}
