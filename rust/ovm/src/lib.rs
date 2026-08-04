//! design.ovm - the floe VFS metadata file, wire format v2
//! (rust/VFS_HIER.md par.3.6; v1 history in rust/VFS.md).
//!
//! Packed little-endian sections: strings, layers, cells, places
//! (+pts pool tail), layer bitsets (deduped), per-cell instance BVH,
//! page directory, page ranges ((cell,layer) runs), page BVH. No
//! file pointers - relative offsets and integer indexes only; fixed
//! width records; the reader never transmutes, every field is a
//! bounds-checked LE read so the same API works over read() today
//! and mmap later.
//!
//! v2 (breaking, version-gated - closed ecosystem, rebuild is the
//! migration): header carries ovp_len (cache-pair consistency);
//! cells carry height u32 + topo_rank (planner sweeps rank order,
//! no fixpoint) + prange range; pages carry seq u32 (65535-page
//! runs) and max_w/max_h u64 (no saturation mis-cut); pts pool
//! entries carry extent + Morton-ordered 256-offset chunk bboxes so
//! a narrow view scans few chunks zero-copy; every builder narrowing
//! is checked ("limit exceeded: <field>"), every open-path range is
//! checked ("corrupt cache; rebuild").

use floe_oasis::doc::Rep;

pub const MAGIC: &[u8; 8] = b"FLOEOVM1";
/// v3: same wire layout as v2, but page PARTITIONING changed
/// (repetition fragmentation + oversize isolation) - v2 caches
/// have semantically wrong page bboxes (die-wide on rep-heavy
/// layers), so the version gate forced a rebuild.
/// v4: LOD page variants (M7, VFS_HIER.md par.5) - the page lod
/// byte goes live (LOD_MERGED) and the pad at offset 68 becomes
/// the exact->LOD page link.
/// v5: cell-local text index (VFS_TEXT_PLAN.md) - five new
/// sections (TEXTS/TRANGES/TBVH/TREPS/TCHUNKS), cell records grow
/// trange range + recursive text-layer mask (CELL_LEN 144), the
/// header spare at 80 becomes ovt_len (design.ovt byte length:
/// string bytes + Morton-ordered text pts pools live THERE, the
/// ovm keeps only fixed records).
pub const VERSION: u32 = 5;

pub const HEADER_LEN: usize = 312;
pub const LAYER_LEN: usize = 32;
pub const CELL_LEN: usize = 144;
pub const PLACE_LEN: usize = 64;
pub const BVH_LEN: usize = 40;
pub const PAGE_LEN: usize = 96;
pub const PRANGE_LEN: usize = 16;
pub const PBVH_LEN: usize = 56;
pub const TEXT_LEN: usize = 80;
pub const TRANGE_LEN: usize = 16;
pub const TBVH_LEN: usize = 40;
pub const TREP_LEN: usize = 64;
pub const TCHUNK_LEN: usize = 32;

/// TextV.rep_idx value meaning "single anchor, no repetition"
pub const TREP_NONE: u32 = u32::MAX;
/// trange.tbvh_root value meaning "no BVH, linear-scan the run"
pub const TBVH_NONE: u32 = u32::MAX;

pub const LOD_EXACT: u8 = 0;
/// merged coverage variant (M7): small members of the exact twin
/// fused into grid-aligned rectangles, large records verbatim
pub const LOD_MERGED: u8 = 1;
/// "no LOD variant" sentinel for the exact page's lod_page link
pub const LOD_PAGE_NONE: u32 = u32::MAX;
/// LOD coverage grid resolution per axis (property of the v4
/// MERGED payloads; the planner's fidelity precondition - one
/// grid cell at or below one screen pixel - derives from it)
pub const LOD_GRID: i64 = 128;
/// codec 0 = plain OASIS single-cell file (CBLOCK inside)
pub const CODEC_OASIS: u8 = 0;

/// prange.pbvh_root value meaning "no BVH, linear-scan the run"
pub const PBVH_NONE: u32 = u32::MAX;
/// offsets per pts chunk (fixed; chunk bboxes are built over
/// Morton-ordered offsets)
pub const PTS_CHUNK: usize = 256;

const SEC_STRINGS: usize = 0;
const SEC_LAYERS: usize = 1;
const SEC_CELLS: usize = 2;
const SEC_PLACES: usize = 3;
const SEC_BITSETS: usize = 4;
const SEC_BVH: usize = 5;
const SEC_PAGEDIR: usize = 6;
const SEC_PRANGES: usize = 7;
const SEC_PBVH: usize = 8;
const SEC_TEXTS: usize = 9;
const SEC_TRANGES: usize = 10;
const SEC_TBVH: usize = 11;
const SEC_TREPS: usize = 12;
const SEC_TCHUNKS: usize = 13;
const N_SECTIONS: usize = 14;

// --------------------------------------------------- checked narrow

/// Checked narrowing (VFS_HIER.md par.3.6): silent truncation is a
/// build-integrity bug, so overflow is a hard build error naming the
/// field. Builder-side only - the reader uses checked arithmetic
/// with "corrupt cache" errors instead.
pub fn narrow_u32(v: u64, field: &str) -> u32 {
    u32::try_from(v).unwrap_or_else(|_| {
        panic!("limit exceeded: {} = {} (max {})", field, v, u32::MAX)
    })
}

pub fn narrow_u16(v: u64, field: &str) -> u16 {
    u16::try_from(v).unwrap_or_else(|_| {
        panic!("limit exceeded: {} = {} (max {})", field, v, u16::MAX)
    })
}

/// checked record-counter increment (u32 wrap would silently corrupt
/// section indexes long before memory gave out)
fn bump(c: &mut u32, what: &str) {
    *c = c
        .checked_add(1)
        .unwrap_or_else(|| panic!("limit exceeded: {} count", what));
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    pub fn intersect(&self, o: &BBox) -> BBox {
        BBox {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        }
    }
    pub fn contains_pt(&self, x: i64, y: i64) -> bool {
        !self.is_empty()
            && x >= self.x0
            && x <= self.x1
            && y >= self.y0
            && y <= self.y1
    }
}

fn pbox(out: &mut Vec<u8>, b: &BBox) {
    pi64(out, b.x0);
    pi64(out, b.y0);
    pi64(out, b.x1);
    pi64(out, b.y1);
}

// -------------------------------------------------- pts preparation

/// Build-side preprocessing of an irregular repetition: offsets are
/// reordered by Morton key over the extent (a rep is a member
/// MULTISET - original order carries no meaning, and keeping file
/// order would make every chunk bbox converge to the extent on
/// spatially-shuffled input). Key algorithm is FIXED for jobs/
/// platform determinism (VFS_HIER.md par.2.3): zx = u64(x - min_x)
/// via i128, key = interleave(zx -> even bits, zy -> odd bits) as
/// u128, ties by pre-sort index. After this, member identity IS the
/// pool slot index.
pub struct PtsPrepared {
    pub extent: BBox,
    pub chunks: Vec<BBox>,
    pub pts: Vec<(i64, i64)>,
}

/// spread the 64 bits of v to the even bit positions of a u128
fn spread_bits(v: u64) -> u128 {
    let mut x = v as u128;
    x = (x | (x << 32)) & 0x0000_0000_FFFF_FFFF_0000_0000_FFFF_FFFF;
    x = (x | (x << 16)) & 0x0000_FFFF_0000_FFFF_0000_FFFF_0000_FFFF;
    x = (x | (x << 8)) & 0x00FF_00FF_00FF_00FF_00FF_00FF_00FF_00FF;
    x = (x | (x << 4)) & 0x0F0F_0F0F_0F0F_0F0F_0F0F_0F0F_0F0F_0F0F;
    x = (x | (x << 2)) & 0x3333_3333_3333_3333_3333_3333_3333_3333;
    x = (x | (x << 1)) & 0x5555_5555_5555_5555_5555_5555_5555_5555;
    x
}

pub fn morton_key(x: i64, y: i64, min_x: i64, min_y: i64) -> u128 {
    let zx = (x as i128 - min_x as i128) as u64;
    let zy = (y as i128 - min_y as i128) as u64;
    spread_bits(zx) | (spread_bits(zy) << 1)
}

pub fn prepare_pts(src: &[(i64, i64)]) -> PtsPrepared {
    let mut extent = BBox::EMPTY;
    for &(x, y) in src {
        extent.grow(&BBox { x0: x, y0: y, x1: x, y1: y });
    }
    let mut keyed: Vec<(u128, u32)> = src
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| {
            (
                morton_key(x, y, extent.x0, extent.y0),
                narrow_u32(i as u64, "pts count"),
            )
        })
        .collect();
    keyed.sort_unstable();
    let pts: Vec<(i64, i64)> =
        keyed.iter().map(|&(_, i)| src[i as usize]).collect();
    let chunks = pts
        .chunks(PTS_CHUNK)
        .map(|c| {
            let mut b = BBox::EMPTY;
            for &(x, y) in c {
                b.grow(&BBox { x0: x, y0: y, x1: x, y1: y });
            }
            b
        })
        .collect();
    PtsPrepared { extent, chunks, pts }
}

// ----------------------------------------------------------- builder

/// Section-by-section builder. The caller appends places/BVH/pages/
/// pranges/pbvh for a cell and then the cell record with the ranges
/// it recorded. All narrowings are checked (panic "limit exceeded").
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
    pranges: Vec<u8>,
    n_pranges: u32,
    pbvh: Vec<u8>,
    n_pbvh: u32,
    texts: Vec<u8>,
    n_texts: u32,
    tranges: Vec<u8>,
    n_tranges: u32,
    tbvh: Vec<u8>,
    n_tbvh: u32,
    treps: Vec<u8>,
    n_treps: u32,
    tchunks: Vec<u8>,
    n_tchunks: u32,
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
            pranges: Vec::new(),
            n_pranges: 0,
            pbvh: Vec::new(),
            n_pbvh: 0,
            texts: Vec::new(),
            n_texts: 0,
            tranges: Vec::new(),
            n_tranges: 0,
            tbvh: Vec::new(),
            n_tbvh: 0,
            treps: Vec::new(),
            n_treps: 0,
            tchunks: Vec::new(),
            n_tchunks: 0,
        }
    }

    pub fn bitset_width(&self) -> usize {
        self.bs_width
    }

    pub fn name(&mut self, s: &str) -> (u32, u16) {
        let off = narrow_u32(self.names.len() as u64, "string pool size");
        self.names.extend_from_slice(s.as_bytes());
        (off, narrow_u16(s.len() as u64, "name length"))
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
        bump(&mut self.n_layers, "layer");
    }

    /// dedup pool index of a layer bitset (bs_width bytes)
    pub fn bitset(&mut self, bits: &[u8]) -> u32 {
        assert_eq!(bits.len(), self.bs_width, "bitset width");
        if let Some(&i) = self.bs_map.get(bits) {
            return i;
        }
        let i = narrow_u32(
            (self.bitsets.len() / self.bs_width) as u64,
            "bitset count",
        );
        self.bitsets.extend_from_slice(bits);
        self.bs_map.insert(bits.to_vec(), i);
        i
    }

    fn place_raw(
        &mut self,
        child: u32,
        x: i64,
        y: i64,
        rot: u8,
        flip: bool,
        kind: u8,
        na: u32,
        nb: u32,
        va: (i64, i64),
        vb: (i64, i64),
    ) -> u64 {
        let idx = self.n_places;
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

    /// appends one placement; returns its global index. Rep::Pts is
    /// Morton-prepared internally - the parallel build path prepares
    /// ahead and calls place_pts directly.
    pub fn place(
        &mut self,
        child: u32,
        x: i64,
        y: i64,
        rot: u8,
        flip: bool,
        rep: &Rep,
    ) -> u64 {
        match rep {
            Rep::One => self
                .place_raw(child, x, y, rot, flip, 0, 1, 1, (0, 0), (0, 0)),
            Rep::Grid { na, nb, va, vb } => self.place_raw(
                child,
                x,
                y,
                rot,
                flip,
                1,
                narrow_u32(*na, "rep na"),
                narrow_u32(*nb, "rep nb"),
                *va,
                *vb,
            ),
            Rep::Pts(p) => {
                let prep = prepare_pts(p);
                self.place_pts(child, x, y, rot, flip, &prep)
            }
        }
    }

    /// appends one irregular-rep placement from prepared (Morton-
    /// ordered) offsets. Pool entry layout: extent 4xi64 | n_chunks
    /// u32 | reserved u32 | chunk bbox 4xi64 x n | (i64,i64) x count.
    /// The place record stores na=count, va.0=entry byte offset.
    pub fn place_pts(
        &mut self,
        child: u32,
        x: i64,
        y: i64,
        rot: u8,
        flip: bool,
        prep: &PtsPrepared,
    ) -> u64 {
        let off = self.pts_pool.len() as u64;
        let off_i64 = i64::try_from(off).unwrap_or_else(|_| {
            panic!("limit exceeded: pts pool offset = {}", off)
        });
        pbox(&mut self.pts_pool, &prep.extent);
        p32(
            &mut self.pts_pool,
            narrow_u32(prep.chunks.len() as u64, "pts chunk count"),
        );
        p32(&mut self.pts_pool, 0);
        for c in &prep.chunks {
            pbox(&mut self.pts_pool, c);
        }
        for &(px, py) in &prep.pts {
            pi64(&mut self.pts_pool, px);
            pi64(&mut self.pts_pool, py);
        }
        let count = narrow_u32(prep.pts.len() as u64, "pts count");
        self.place_raw(
            child,
            x,
            y,
            rot,
            flip,
            2,
            count,
            1,
            (off_i64, 0),
            (0, 0),
        )
    }

    /// appends one instance-BVH node; returns its global index
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
        bump(&mut self.n_bvh, "bvh");
        idx
    }

    /// appends one page-BVH node (subtree max_w/max_h ride along for
    /// pre-leaf cut culling); returns its global index
    #[allow(clippy::too_many_arguments)]
    pub fn pbvh_node(
        &mut self,
        bbox: &BBox,
        first: u32,
        count: u16,
        leaf: bool,
        max_w: u64,
        max_h: u64,
    ) -> u32 {
        let idx = self.n_pbvh;
        let out = &mut self.pbvh;
        pbox(out, bbox);
        p32(out, first);
        p16(out, count);
        out.push(leaf as u8);
        out.push(0);
        p64(out, max_w);
        p64(out, max_h);
        assert_eq!(out.len() % PBVH_LEN, 0, "pbvh stride");
        bump(&mut self.n_pbvh, "pbvh");
        idx
    }

    /// appends one (cell,layer) page-run record; returns its index
    pub fn prange(
        &mut self,
        layer_idx: u32,
        page_lo: u32,
        page_count: u32,
        pbvh_root: u32,
    ) -> u32 {
        let idx = self.n_pranges;
        let out = &mut self.pranges;
        p32(out, layer_idx);
        p32(out, page_lo);
        p32(out, page_count);
        p32(out, pbvh_root);
        assert_eq!(out.len() % PRANGE_LEN, 0, "prange stride");
        bump(&mut self.n_pranges, "prange");
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
    pub fn n_pranges(&self) -> u32 {
        self.n_pranges
    }
    pub fn n_pbvh(&self) -> u32 {
        self.n_pbvh
    }
    pub fn n_texts(&self) -> u32 {
        self.n_texts
    }
    pub fn n_tranges(&self) -> u32 {
        self.n_tranges
    }
    pub fn n_tbvh(&self) -> u32 {
        self.n_tbvh
    }
    pub fn n_treps(&self) -> u32 {
        self.n_treps
    }
    pub fn n_tchunks(&self) -> u32 {
        self.n_tchunks
    }

    // ------------------------------------------- text index (v5)

    /// appends one cell-local text record. The string (and a Pts
    /// rep's point pool) lives in design.ovt - the ovm carries only
    /// its (offset, length). `bbox` is the anchor grown by the
    /// repetition extent; `seq` is the source-order tie-break.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        cell: u32,
        layer_idx: u32,
        x: i64,
        y: i64,
        string_off: u64,
        string_len: u32,
        rep_idx: u32,
        bbox: &BBox,
        seq: u32,
    ) -> u32 {
        let idx = self.n_texts;
        let out = &mut self.texts;
        p32(out, cell);
        p32(out, layer_idx);
        pi64(out, x);
        pi64(out, y);
        p64(out, string_off);
        p32(out, string_len);
        p32(out, rep_idx);
        pbox(out, bbox);
        p32(out, seq);
        p32(out, 0);
        assert_eq!(out.len() % TEXT_LEN, 0, "text stride");
        bump(&mut self.n_texts, "text");
        idx
    }

    /// appends one (cell,layer) text-run record; returns its index
    pub fn trange(
        &mut self,
        layer_idx: u32,
        text_lo: u32,
        text_count: u32,
        tbvh_root: u32,
    ) -> u32 {
        let idx = self.n_tranges;
        let out = &mut self.tranges;
        p32(out, layer_idx);
        p32(out, text_lo);
        p32(out, text_count);
        p32(out, tbvh_root);
        assert_eq!(out.len() % TRANGE_LEN, 0, "trange stride");
        bump(&mut self.n_tranges, "trange");
        idx
    }

    /// appends one text-BVH node; returns its global index
    pub fn tbvh_node(
        &mut self,
        bbox: &BBox,
        first: u32,
        count: u16,
        leaf: bool,
    ) -> u32 {
        let idx = self.n_tbvh;
        let out = &mut self.tbvh;
        pbox(out, bbox);
        p32(out, first);
        p16(out, count);
        p16(out, leaf as u16);
        assert_eq!(out.len() % TBVH_LEN, 0, "tbvh stride");
        bump(&mut self.n_tbvh, "tbvh");
        idx
    }

    /// appends a text Grid repetition descriptor
    pub fn trep_grid(
        &mut self,
        na: u32,
        nb: u32,
        va: (i64, i64),
        vb: (i64, i64),
    ) -> u32 {
        let idx = self.n_treps;
        let out = &mut self.treps;
        out.push(1);
        out.extend_from_slice(&[0, 0, 0]);
        p32(out, na);
        p32(out, nb);
        p32(out, 0);
        pi64(out, va.0);
        pi64(out, va.1);
        pi64(out, vb.0);
        pi64(out, vb.1);
        p64(out, 0);
        p32(out, 0);
        p32(out, 0);
        assert_eq!(out.len() % TREP_LEN, 0, "trep stride");
        bump(&mut self.n_treps, "trep");
        idx
    }

    /// appends a text Pts repetition descriptor: `count` Morton-
    /// ordered (i64,i64) pairs live in design.ovt at `pts_off`,
    /// chunk bboxes (PTS_CHUNK offsets each) in TCHUNKS at
    /// [chunk_lo, chunk_lo+chunk_count)
    pub fn trep_pts(
        &mut self,
        count: u32,
        pts_off: u64,
        chunk_lo: u32,
        chunk_count: u32,
    ) -> u32 {
        let idx = self.n_treps;
        let out = &mut self.treps;
        out.push(2);
        out.extend_from_slice(&[0, 0, 0]);
        p32(out, count);
        p32(out, 1);
        p32(out, chunk_lo);
        pi64(out, 0);
        pi64(out, 0);
        pi64(out, 0);
        pi64(out, 0);
        p64(out, pts_off);
        p32(out, chunk_count);
        p32(out, 0);
        assert_eq!(out.len() % TREP_LEN, 0, "trep stride");
        bump(&mut self.n_treps, "trep");
        idx
    }

    /// appends one text-pts chunk bbox; returns its global index
    pub fn tchunk(&mut self, bbox: &BBox) -> u32 {
        let idx = self.n_tchunks;
        pbox(&mut self.tchunks, bbox);
        assert_eq!(
            self.tchunks.len() % TCHUNK_LEN,
            0,
            "tchunk stride"
        );
        bump(&mut self.n_tchunks, "tchunk");
        idx
    }

    #[allow(clippy::too_many_arguments)]
    pub fn page(
        &mut self,
        cell: u32,
        layer_idx: u32,
        seq: u32,
        bbox: &BBox,
        file_off: u64,
        csize: u64,
        usize_: u64,
        records: u64,
        members: u64,
        max_w: u64,
        max_h: u64,
        lod: u8,
        lod_page: u32,
    ) {
        let out = &mut self.pages;
        p32(out, cell);
        p32(out, layer_idx);
        p32(out, seq);
        out.push(lod);
        out.push(CODEC_OASIS);
        p16(out, 0);
        pbox(out, bbox);
        p64(out, file_off);
        p32(out, narrow_u32(csize, "page csize"));
        p32(out, narrow_u32(usize_, "page usize"));
        p32(out, narrow_u32(records, "page records"));
        p32(out, lod_page);
        p64(out, members);
        p64(out, max_w);
        p64(out, max_h);
        assert_eq!(out.len() % PAGE_LEN, 0, "page stride");
        bump(&mut self.n_pages, "page");
    }

    /// v5: trailing (trange_start, trange_count, tmask_rec) wire
    /// the cell-local text index - tmask_rec is the RECURSIVE
    /// text-layer bitset (prunes text-free subtrees in the label
    /// walk; distinct from lmask_rec, which includes geometry).
    #[allow(clippy::too_many_arguments)]
    pub fn cell(
        &mut self,
        nm: &str,
        height: u32,
        topo_rank: u32,
        dbbox: &BBox,
        rbbox: &BBox,
        place_start: u32,
        place_count: u32,
        page_start: u32,
        page_count: u32,
        bvh_start: u32,
        bvh_count: u32,
        prange_start: u32,
        prange_count: u32,
        lmask_direct: u32,
        lmask_rec: u32,
        rec_members: u64,
        trange_start: u32,
        trange_count: u32,
        tmask_rec: u32,
    ) {
        let (no, nl) = self.name(nm);
        let out = &mut self.cells;
        p32(out, no);
        p16(out, nl);
        p16(out, 0);
        p32(out, height);
        p32(out, topo_rank);
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
        p32(out, prange_start);
        p32(out, prange_count);
        p32(out, trange_start);
        p32(out, trange_count);
        p32(out, tmask_rec);
        p32(out, 0);
        assert_eq!(out.len() % CELL_LEN, 0, "cell stride");
        bump(&mut self.n_cells, "cell");
    }

    /// ovp_len / ovt_len: byte lengths of the design.ovp/design.ovt
    /// this ovm commits - the reader (and the viewer's Vfs::open)
    /// verifies both pairs.
    pub fn finish(mut self, ovp_len: u64, ovt_len: u64) -> Vec<u8> {
        // The pts pool is logically the tail of the places section. Append
        // both source buffers directly to the final output instead of first
        // cloning the complete pool into `places`; on fill-heavy designs that
        // intermediate copy is one of the largest metadata live sets.
        let places_len = self
            .places
            .len()
            .checked_add(self.pts_pool.len())
            .expect("limit exceeded: places section bytes");
        let sec_lens = [
            self.names.len(),
            self.layers.len(),
            self.cells.len(),
            places_len,
            self.bitsets.len(),
            self.bvh.len(),
            self.pages.len(),
            self.pranges.len(),
            self.pbvh.len(),
            self.texts.len(),
            self.tranges.len(),
            self.tbvh.len(),
            self.treps.len(),
            self.tchunks.len(),
        ];
        let body_len = sec_lens
            .iter()
            .try_fold(0usize, |n, &len| n.checked_add(len))
            .expect("limit exceeded: ovm bytes");
        let mut out = Vec::with_capacity(
            HEADER_LEN
                .checked_add(body_len)
                .expect("limit exceeded: ovm bytes"),
        );
        out.extend_from_slice(MAGIC);
        p32(&mut out, VERSION);
        p32(&mut out, narrow_u32(self.bs_width as u64, "bitset width"));
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
        p64(&mut out, ovp_len);
        p64(&mut out, ovt_len);
        let mut off = HEADER_LEN as u64;
        for len in sec_lens {
            p64(&mut out, off);
            p64(&mut out, len as u64);
            off = off
                .checked_add(len as u64)
                .expect("limit exceeded: ovm bytes");
        }
        assert_eq!(out.len(), HEADER_LEN, "header layout");
        // Release each source section immediately after copying it.
        // Previously every Builder Vec remained allocated until the
        // complete final OVM copy existed, making finish() approach a
        // 2x metadata peak. Vec::with_capacity reserves address space;
        // pages become resident only as these sequential writes land.
        macro_rules! append_release {
            ($field:ident) => {{
                out.extend_from_slice(&self.$field);
                self.$field = Vec::new();
            }};
        }
        append_release!(names);
        append_release!(layers);
        append_release!(cells);
        append_release!(places);
        append_release!(pts_pool);
        append_release!(bitsets);
        append_release!(bvh);
        append_release!(pages);
        append_release!(pranges);
        append_release!(pbvh);
        append_release!(texts);
        append_release!(tranges);
        append_release!(tbvh);
        append_release!(treps);
        append_release!(tchunks);
        debug_assert_eq!(out.len(), HEADER_LEN + body_len);
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
    pub height: u32,
    pub topo_rank: u32,
    pub dbbox: BBox,
    pub rbbox: BBox,
    pub place_start: u32,
    pub place_count: u32,
    pub page_start: u32,
    pub page_count: u32,
    pub bvh_start: u32,
    pub bvh_count: u32,
    pub prange_start: u32,
    pub prange_count: u32,
    pub lmask_direct: u32,
    pub lmask_rec: u32,
    pub rec_members: u64,
    pub trange_start: u32,
    pub trange_count: u32,
    pub tmask_rec: u32,
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

/// raw place fields, rep NOT materialized (kind 0 One / 1 Grid via
/// na,nb,va,vb / 2 Pts via pts_ref)
#[derive(Debug, Clone, Copy)]
pub struct PlaceHead {
    pub child: u32,
    pub rot: u8,
    pub flip: bool,
    pub kind: u8,
    pub x: i64,
    pub y: i64,
    pub na: u32,
    pub nb: u32,
    pub va: (i64, i64),
    pub vb: (i64, i64),
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
    pub seq: u32,
    pub lod: u8,
    pub codec: u8,
    pub bbox: BBox,
    pub file_off: u64,
    pub csize: u32,
    pub usize_: u32,
    pub records: u32,
    /// exact page -> its LOD variant page index (LOD_PAGE_NONE if
    /// the page has no variant; always NONE on LOD pages)
    pub lod_page: u32,
    pub members: u64,
    pub max_w: u64,
    pub max_h: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct PrangeV {
    pub layer_idx: u32,
    pub page_lo: u32,
    pub page_count: u32,
    pub pbvh_root: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct PbvhV {
    pub bbox: BBox,
    pub first: u32,
    pub count: u16,
    pub leaf: bool,
    pub max_w: u64,
    pub max_h: u64,
}

/// one cell-local text record (v5). The string bytes live in
/// design.ovt at [string_off, string_off+string_len); bbox is the
/// anchor grown by the repetition extent.
#[derive(Debug, Clone, Copy)]
pub struct TextV {
    pub cell: u32,
    pub layer_idx: u32,
    pub x: i64,
    pub y: i64,
    pub string_off: u64,
    pub string_len: u32,
    /// TREPS index, TREP_NONE = single anchor
    pub rep_idx: u32,
    pub bbox: BBox,
    /// source-order tie-break within the owning cell
    pub seq: u32,
}

/// one (cell,layer) text run + its BVH root (TBVH_NONE = linear)
#[derive(Debug, Clone, Copy)]
pub struct TrangeV {
    pub layer_idx: u32,
    pub text_lo: u32,
    pub text_count: u32,
    pub tbvh_root: u32,
}

/// text repetition descriptor. Pts point pools are Morton-ordered
/// (i64,i64) LE pairs in design.ovt; chunk bboxes (PTS_CHUNK
/// offsets each) live in TCHUNKS.
#[derive(Debug, Clone, Copy)]
pub enum TrepV {
    Grid { na: u32, nb: u32, va: (i64, i64), vb: (i64, i64) },
    Pts { count: u32, pts_off: u64, chunk_lo: u32, chunk_count: u32 },
}

impl TrepV {
    pub fn members(&self) -> u64 {
        match self {
            TrepV::Grid { na, nb, .. } => *na as u64 * *nb as u64,
            TrepV::Pts { count, .. } => *count as u64,
        }
    }
}

/// text-pts member (ox, oy) at `slot` of a pool at `pts_off` inside
/// the design.ovt bytes (bounds guaranteed by open validation)
pub fn ovt_pt(ovt: &[u8], pts_off: u64, slot: u32) -> (i64, i64) {
    let o = pts_off as usize + slot as usize * 16;
    (gi64(ovt, o), gi64(ovt, o + 8))
}

/// zero-copy view of one pts pool entry: borrowed bytes, LE decode
/// per access (no Vec materialization - the planner's chunk scans go
/// through this; Ovm::place() still materializes for compat/tests)
pub struct PtsRef<'a> {
    entry: &'a [u8],
    pub count: u32,
    pub n_chunks: u32,
}

impl<'a> PtsRef<'a> {
    pub fn extent(&self) -> BBox {
        gbox(self.entry, 0)
    }
    pub fn chunk_bbox(&self, k: u32) -> BBox {
        gbox(self.entry, 40 + k as usize * 32)
    }
    /// slot range [lo, hi) covered by chunk k
    pub fn chunk_range(&self, k: u32) -> (u32, u32) {
        let lo = k * PTS_CHUNK as u32;
        (lo, (lo + PTS_CHUNK as u32).min(self.count))
    }
    pub fn pt(&self, slot: u32) -> (i64, i64) {
        let base =
            40 + self.n_chunks as usize * 32 + slot as usize * 16;
        (gi64(self.entry, base), gi64(self.entry, base + 8))
    }
}

pub struct Ovm {
    pub data: Backing,
    pub unit: f64,
    pub src_size: u64,
    pub src_mtime: u64,
    pub top: u32,
    pub n_layers: u32,
    pub n_cells: u32,
    pub n_pages: u32,
    pub n_places: u64,
    pub n_bvh: u32,
    pub n_pranges: u32,
    pub n_pbvh: u32,
    pub n_texts: u32,
    pub n_tranges: u32,
    pub n_tbvh: u32,
    pub n_treps: u32,
    pub n_tchunks: u32,
    pub ovp_len: u64,
    pub ovt_len: u64,
    pub bs_width: usize,
    secs: [(u64, u64); N_SECTIONS],
}

fn corrupt(msg: impl std::fmt::Display) -> String {
    format!("corrupt cache; rebuild: {}", msg)
}

/// reader byte source: an owned buffer (builder output, tests) or a
/// read-only file mapping (`Ovm::open`). The mapping makes open()
/// touch only what validation and the planner actually read - on a
/// 25 GB metadata asset the old whole-file read was the daemon's
/// startup cost. Safe against rebuilds by the marker protocol:
/// caches are replaced by DELETE + recreate (new inode), never
/// truncated in place, so a live mapping stays valid.
pub enum Backing {
    Vec(Vec<u8>),
    Map(memmap2::Mmap),
}

impl std::ops::Deref for Backing {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Backing::Vec(v) => v,
            Backing::Map(m) => m,
        }
    }
}

/// read-only file mapping as a Backing (design.ovt). Empty files
/// map to an owned empty buffer - mmap(0) is an error on most
/// platforms, and a no-text cache is perfectly valid.
pub fn map_file(path: &str) -> Result<Backing, String> {
    let f = std::fs::File::open(path)
        .map_err(|e| format!("open {}: {}", path, e))?;
    let len = f
        .metadata()
        .map_err(|e| format!("{}: {}", path, e))?
        .len();
    if len == 0 {
        return Ok(Backing::Vec(Vec::new()));
    }
    let map = unsafe { memmap2::Mmap::map(&f) }
        .map_err(|e| format!("mmap {}: {}", path, e))?;
    Ok(Backing::Map(map))
}

impl Ovm {
    /// full-strength open from an owned buffer: DEEP validation
    /// (per-record places / pts pool / instance-BVH bounds). Unit
    /// tests and gate fixtures come through here.
    pub fn from_bytes(data: Vec<u8>) -> Result<Ovm, String> {
        Ovm::validate(Backing::Vec(data), true)
    }

    /// production open: read-only mmap + SHALLOW validation - the
    /// header, section bounds and the small tables (layers, cells,
    /// pages, pranges, page-BVH) keep their full checks ("corrupt
    /// cache; rebuild" class), but the per-record sweeps over
    /// places / pts pool / instance BVH are skipped: they would
    /// fault in the whole multi-GB section and defeat the mapping.
    /// The marker protocol guarantees build completeness (par.3.6);
    /// deep-section damage beyond that is a builder bug, which the
    /// build gates own. Trade-off: reading such damage panics via
    /// accessor asserts instead of failing open cleanly.
    pub fn open(path: &str) -> Result<Ovm, String> {
        let f = std::fs::File::open(path)
            .map_err(|e| format!("open {}: {}", path, e))?;
        let map = unsafe { memmap2::Mmap::map(&f) }
            .map_err(|e| format!("mmap {}: {}", path, e))?;
        Ovm::validate(Backing::Map(map), false)
    }

    fn validate(data: Backing, deep: bool) -> Result<Ovm, String> {
        if data.len() < HEADER_LEN || &data[..8] != MAGIC {
            return Err("not an ovm file".into());
        }
        if g32(&data, 8) != VERSION {
            return Err(format!(
                "ovm version {} (this build reads v{}) - run: \
                 floe-index vfs <src> (rebuild)",
                g32(&data, 8),
                VERSION
            ));
        }
        let bs_width = g32(&data, 12) as usize;
        let top = g32(&data, 40);
        let n_layers = g32(&data, 44);
        let n_cells = g32(&data, 48);
        let n_pages = g32(&data, 52);
        let n_places = g64(&data, 56);
        let n_bvh = g32(&data, 64);
        let ovp_len = g64(&data, 72);
        let ovt_len = g64(&data, 80);
        let mut secs = [(0u64, 0u64); N_SECTIONS];
        for (i, s) in secs.iter_mut().enumerate() {
            let o = 88 + i * 16;
            *s = (g64(&data, o), g64(&data, o + 8));
            let end = s
                .0
                .checked_add(s.1)
                .ok_or_else(|| corrupt(format!("section {} wraps", i)))?;
            if end > data.len() as u64 {
                return Err(corrupt(format!("section {} out of range", i)));
            }
        }
        // ---- section-type structural validation (open gate): a
        // truncated or partly-written file must never look valid
        let sec = |i: usize| -> &[u8] {
            let (o, l) = secs[i];
            &data[o as usize..(o + l) as usize]
        };
        let exact = |i: usize, count: u64, stride: usize, what: &str| {
            let need = count
                .checked_mul(stride as u64)
                .ok_or_else(|| corrupt(format!("{} count wraps", what)))?;
            if secs[i].1 != need {
                return Err(corrupt(format!(
                    "{} section {} bytes, expected {}",
                    what, secs[i].1, need
                )));
            }
            Ok(())
        };
        exact(SEC_LAYERS, n_layers as u64, LAYER_LEN, "layer")?;
        exact(SEC_CELLS, n_cells as u64, CELL_LEN, "cell")?;
        exact(SEC_BVH, n_bvh as u64, BVH_LEN, "bvh")?;
        exact(SEC_PAGEDIR, n_pages as u64, PAGE_LEN, "page")?;
        if secs[SEC_PRANGES].1 % PRANGE_LEN as u64 != 0 {
            return Err(corrupt("prange section stride"));
        }
        let n_pranges = u32::try_from(secs[SEC_PRANGES].1 / PRANGE_LEN as u64)
            .map_err(|_| corrupt("prange count"))?;
        if secs[SEC_PBVH].1 % PBVH_LEN as u64 != 0 {
            return Err(corrupt("pbvh section stride"));
        }
        let n_pbvh = u32::try_from(secs[SEC_PBVH].1 / PBVH_LEN as u64)
            .map_err(|_| corrupt("pbvh count"))?;
        let derived = |i: usize, stride: usize, what: &str| {
            if secs[i].1 % stride as u64 != 0 {
                return Err(corrupt(format!(
                    "{} section stride",
                    what
                )));
            }
            u32::try_from(secs[i].1 / stride as u64)
                .map_err(|_| corrupt(format!("{} count", what)))
        };
        let n_texts = derived(SEC_TEXTS, TEXT_LEN, "text")?;
        let n_tranges = derived(SEC_TRANGES, TRANGE_LEN, "trange")?;
        let n_tbvh = derived(SEC_TBVH, TBVH_LEN, "tbvh")?;
        let n_treps = derived(SEC_TREPS, TREP_LEN, "trep")?;
        let n_tchunks = derived(SEC_TCHUNKS, TCHUNK_LEN, "tchunk")?;
        let places_need = n_places
            .checked_mul(PLACE_LEN as u64)
            .ok_or_else(|| corrupt("place count wraps"))?;
        if secs[SEC_PLACES].1 < places_need {
            return Err(corrupt("places section short"));
        }
        let pool_len = secs[SEC_PLACES].1 - places_need;
        // bs_width guards the modulo below - a zeroed header must not
        // become a divide-by-zero panic - and must actually hold
        // n_layers bits (a lying header would make every bitset test
        // an out-of-bounds panic later)
        if bs_width == 0 {
            return Err(corrupt("bitset width 0"));
        }
        if (bs_width as u64) * 8 < n_layers as u64 {
            return Err(corrupt("bitset width vs layer count"));
        }
        if secs[SEC_BITSETS].1 % bs_width as u64 != 0 {
            return Err(corrupt("bitset section stride"));
        }
        let n_bitsets = secs[SEC_BITSETS].1 / bs_width as u64;
        if n_cells > 0 && top >= n_cells {
            return Err(corrupt("top cell index"));
        }
        // layers: name ranges
        let lb = sec(SEC_LAYERS);
        let sb_len = secs[SEC_STRINGS].1;
        for i in 0..n_layers as usize {
            let b = &lb[i * LAYER_LEN..];
            if g32(b, 8) as u64 + g16(b, 12) as u64 > sb_len {
                return Err(corrupt(format!("layer {} name range", i)));
            }
        }
        // cells: every range and name checked once at open
        let cb = sec(SEC_CELLS);
        for i in 0..n_cells as usize {
            let b = &cb[i * CELL_LEN..];
            let no = g32(b, 0) as u64;
            let nl = g16(b, 4) as u64;
            if no + nl > sb_len {
                return Err(corrupt(format!("cell {} name range", i)));
            }
            let chk = |s: u32, c: u32, n: u64, what: &str| {
                if (s as u64) + (c as u64) > n {
                    return Err(corrupt(format!(
                        "cell {} {} range",
                        i, what
                    )));
                }
                Ok(())
            };
            chk(g32(b, 80), g32(b, 84), n_places, "place")?;
            chk(g32(b, 88), g32(b, 92), n_pages as u64, "page")?;
            chk(g32(b, 96), g32(b, 100), n_bvh as u64, "bvh")?;
            chk(g32(b, 120), g32(b, 124), n_pranges as u64, "prange")?;
            chk(g32(b, 128), g32(b, 132), n_tranges as u64, "trange")?;
            if g32(b, 104) as u64 >= n_bitsets
                || g32(b, 108) as u64 >= n_bitsets
                || g32(b, 136) as u64 >= n_bitsets
            {
                return Err(corrupt(format!("cell {} bitset index", i)));
            }
        }
        if deep {
            // places: child index, rep kind, pts pool entry bounds
            let pb = sec(SEC_PLACES);
            for i in 0..n_places as usize {
                let b = &pb[i * PLACE_LEN..];
                if g32(b, 0) >= n_cells {
                    return Err(corrupt(format!("place {} child", i)));
                }
                let kind = b[6];
                if kind > 2 {
                    return Err(corrupt(format!("place {} rep kind", i)));
                }
                if kind == 2 {
                    let off = gi64(b, 32);
                    let count = g32(b, 24) as u64;
                    if off < 0 {
                        return Err(corrupt(format!("place {} pts offset", i)));
                    }
                    let off = off as u64;
                    if off + 40 > pool_len {
                        return Err(corrupt(format!("place {} pts entry", i)));
                    }
                    let nch =
                        g32(&pb[places_need as usize + off as usize..], 32)
                            as u64;
                    let need = 40u64
                        .checked_add(nch.checked_mul(32).ok_or_else(
                            || corrupt("pts chunk count wraps"),
                        )?)
                        .and_then(|v| {
                            v.checked_add(count.checked_mul(16)?)
                        })
                        .ok_or_else(|| corrupt("pts entry wraps"))?;
                    if off + need > pool_len {
                        return Err(corrupt(format!(
                            "place {} pts entry range",
                            i
                        )));
                    }
                    if nch != count.div_ceil(PTS_CHUNK as u64) {
                        return Err(corrupt(format!(
                            "place {} pts chunk count",
                            i
                        )));
                    }
                }
            }
            // instance BVH: leaf ranges into places, inner into nodes
            let nb = sec(SEC_BVH);
            for i in 0..n_bvh as usize {
                let b = &nb[i * BVH_LEN..];
                let first = g32(b, 32) as u64;
                let count = g16(b, 36) as u64;
                let leaf = g16(b, 38) != 0;
                let lim = if leaf { n_places } else { n_bvh as u64 };
                if first + count > lim {
                    return Err(corrupt(format!("bvh node {} range", i)));
                }
            }
        }

        // pranges: layer + page run + optional pbvh root (the
        // planner indexes the vis bitset with layer_idx unchecked -
        // a corrupt value must die HERE, not as a panic there)
        let prb = sec(SEC_PRANGES);
        for i in 0..n_pranges as usize {
            let b = &prb[i * PRANGE_LEN..];
            if g32(b, 0) >= n_layers {
                return Err(corrupt(format!(
                    "prange {} layer index",
                    i
                )));
            }
            if g32(b, 4) as u64 + g32(b, 8) as u64 > n_pages as u64 {
                return Err(corrupt(format!("prange {} page range", i)));
            }
            let root = g32(b, 12);
            if root != PBVH_NONE && root >= n_pbvh {
                return Err(corrupt(format!("prange {} pbvh root", i)));
            }
        }
        // page BVH: leaf ranges into page dir, inner into nodes
        let pvb = sec(SEC_PBVH);
        for i in 0..n_pbvh as usize {
            let b = &pvb[i * PBVH_LEN..];
            let first = g32(b, 32) as u64;
            let count = g16(b, 36) as u64;
            let leaf = b[38] != 0;
            let lim =
                if leaf { n_pages as u64 } else { n_pbvh as u64 };
            if first + count > lim {
                return Err(corrupt(format!("pbvh node {} range", i)));
            }
        }
        // Potentially one-per-text tables stay out of the mmap-open
        // sweep. Their section strides/counts were checked above;
        // full range/kind/bbox validation belongs to from_bytes() and
        // offline gates, otherwise a repetition-heavy text index is
        // faulted into RSS before the first viewport.
        if deep {
            let trb = sec(SEC_TRANGES);
            for i in 0..n_tranges as usize {
                let b = &trb[i * TRANGE_LEN..];
                if g32(b, 0) >= n_layers {
                    return Err(corrupt(format!(
                        "trange {} layer index",
                        i
                    )));
                }
                if g32(b, 4) as u64 + g32(b, 8) as u64
                    > n_texts as u64
                {
                    return Err(corrupt(format!(
                        "trange {} text range",
                        i
                    )));
                }
                let root = g32(b, 12);
                if root != TBVH_NONE && root >= n_tbvh {
                    return Err(corrupt(format!(
                        "trange {} tbvh root",
                        i
                    )));
                }
            }
            let tvb = sec(SEC_TBVH);
            for i in 0..n_tbvh as usize {
                let b = &tvb[i * TBVH_LEN..];
                let bb = gbox(b, 0);
                if bb.x1 < bb.x0 || bb.y1 < bb.y0 {
                    return Err(corrupt(format!(
                        "tbvh node {} bbox inverted",
                        i
                    )));
                }
                let first = g32(b, 32) as u64;
                let count = g16(b, 36) as u64;
                let leaf = g16(b, 38) != 0;
                let lim = if leaf {
                    n_texts as u64
                } else {
                    n_tbvh as u64
                };
                if first + count > lim {
                    return Err(corrupt(format!(
                        "tbvh node {} range",
                        i
                    )));
                }
            }
            let tpb = sec(SEC_TREPS);
            for i in 0..n_treps as usize {
                let b = &tpb[i * TREP_LEN..];
                let na = g32(b, 4) as u64;
                let nb = g32(b, 8) as u64;
                match b[0] {
                    1 => {
                        if na == 0 || nb == 0 {
                            return Err(corrupt(format!(
                                "trep {} grid dims",
                                i
                            )));
                        }
                    }
                    2 => {
                        if na == 0 {
                            return Err(corrupt(format!(
                                "trep {} pts count",
                                i
                            )));
                        }
                        let clo = g32(b, 12) as u64;
                        let ccnt = g32(b, 56) as u64;
                        if clo + ccnt > n_tchunks as u64 {
                            return Err(corrupt(format!(
                                "trep {} chunk range",
                                i
                            )));
                        }
                        if ccnt != na.div_ceil(PTS_CHUNK as u64) {
                            return Err(corrupt(format!(
                                "trep {} chunk count",
                                i
                            )));
                        }
                        let need = na
                            .checked_mul(16)
                            .and_then(|v| v.checked_add(g64(b, 48)))
                            .ok_or_else(|| {
                                corrupt(format!("trep {} pts wraps", i))
                            })?;
                        if need > ovt_len {
                            return Err(corrupt(format!(
                                "trep {} pts beyond ovt_len",
                                i
                            )));
                        }
                    }
                    k => {
                        return Err(corrupt(format!(
                            "trep {} kind {}",
                            i, k
                        )))
                    }
                }
            }
            let tcb = sec(SEC_TCHUNKS);
            for i in 0..n_tchunks as usize {
                let bb = gbox(&tcb[i * TCHUNK_LEN..], 0);
                if bb.x1 < bb.x0 || bb.y1 < bb.y0 {
                    return Err(corrupt(format!(
                        "tchunk {} bbox inverted",
                        i
                    )));
                }
            }
        }

        // pages: owner/layer indexes + payload spans inside the
        // committed ovp
        let pgb = sec(SEC_PAGEDIR);
        let mut lod_claim =
            vec![0u8; (n_pages as usize).div_ceil(8)];
        for i in 0..n_pages as usize {
            let b = &pgb[i * PAGE_LEN..];
            if g32(b, 0) >= n_cells {
                return Err(corrupt(format!("page {} cell index", i)));
            }
            if g32(b, 4) >= n_layers {
                return Err(corrupt(format!("page {} layer index", i)));
            }
            let end = g64(b, 48)
                .checked_add(g32(b, 56) as u64)
                .ok_or_else(|| corrupt(format!("page {} span wraps", i)))?;
            if end > ovp_len {
                return Err(corrupt(format!(
                    "page {} beyond ovp_len ({} > {})",
                    i, end, ovp_len
                )));
            }
            // lod byte and codec must be KNOWN values - anything
            // else is a corrupt or future cache, not a silent
            // fallthrough (review finding)
            if b[12] != LOD_EXACT && b[12] != LOD_MERGED {
                return Err(corrupt(format!(
                    "page {} lod byte {}",
                    i, b[12]
                )));
            }
            if b[13] != CODEC_OASIS {
                return Err(corrupt(format!(
                    "page {} codec {}",
                    i, b[13]
                )));
            }
            // exact -> LOD link: in bounds, target is EXACTLY a
            // MERGED page of the same (cell, layer, seq), claimed
            // by only ONE exact page; LOD pages don't chain
            let lp = g32(b, 68);
            if b[12] != LOD_EXACT && lp != LOD_PAGE_NONE {
                return Err(corrupt(format!("page {} lod chain", i)));
            }
            if lp != LOD_PAGE_NONE {
                if lp >= n_pages {
                    return Err(corrupt(format!(
                        "page {} lod link {}",
                        i, lp
                    )));
                }
                let t = &pgb[lp as usize * PAGE_LEN..];
                if t[12] != LOD_MERGED
                    || g32(t, 0) != g32(b, 0)
                    || g32(t, 4) != g32(b, 4)
                    || g32(t, 8) != g32(b, 8)
                {
                    return Err(corrupt(format!(
                        "page {} lod link target {}",
                        i, lp
                    )));
                }
                let slot = &mut lod_claim[lp as usize >> 3];
                let bit = 1u8 << (lp as usize & 7);
                if *slot & bit != 0 {
                    return Err(corrupt(format!(
                        "lod page {} claimed twice",
                        lp
                    )));
                }
                *slot |= bit;
            }
        }
        // cross-table ownership (review round): every prange run
        // must hold EXACT pages of its own (cell, layer). Corrupt
        // metadata pointing a run into another cell's pages or at
        // LOD variants would otherwise open fine and draw the
        // wrong geometry (or hand a probe a merged variant).
        {
            let cb = sec(SEC_CELLS);
            let prb = sec(SEC_PRANGES);
            let pgb = sec(SEC_PAGEDIR);
            for ci in 0..n_cells as usize {
                let c = &cb[ci * CELL_LEN..];
                let (ps, pc) = (g32(c, 120), g32(c, 124));
                for k in 0..pc {
                    let pr = &prb[(ps + k) as usize
                        * PRANGE_LEN..];
                    let pl = g32(pr, 0);
                    let (lo, cnt) = (g32(pr, 4), g32(pr, 8));
                    for pi in lo..lo.saturating_add(cnt) {
                        let pg =
                            &pgb[pi as usize * PAGE_LEN..];
                        if g32(pg, 0) != ci as u32
                            || g32(pg, 4) != pl
                            || pg[12] != LOD_EXACT
                        {
                            return Err(corrupt(format!(
                                "cell {} prange {} page {} \
                                 ownership",
                                ci,
                                ps + k,
                                pi
                            )));
                        }
                    }
                }
            }
        }

        if deep {
            let contains_box = |outer: &BBox, inner: &BBox| {
                outer.x0 <= inner.x0
                    && outer.y0 <= inner.y0
                    && inner.x1 <= outer.x1
                    && inner.y1 <= outer.y1
            };
            // texts: per-record sweep (owner indexes, ovt string
            // spans, rep index, bbox sanity)
            let txb = sec(SEC_TEXTS);
            let tpb = sec(SEC_TREPS);
            let tcb = sec(SEC_TCHUNKS);
            for i in 0..n_texts as usize {
                let b = &txb[i * TEXT_LEN..];
                if g32(b, 0) >= n_cells {
                    return Err(corrupt(format!(
                        "text {} cell index",
                        i
                    )));
                }
                if g32(b, 4) >= n_layers {
                    return Err(corrupt(format!(
                        "text {} layer index",
                        i
                    )));
                }
                let send = g64(b, 24)
                    .checked_add(g32(b, 32) as u64)
                    .ok_or_else(|| {
                        corrupt(format!("text {} string wraps", i))
                    })?;
                if send > ovt_len {
                    return Err(corrupt(format!(
                        "text {} string beyond ovt_len",
                        i
                    )));
                }
                let rep = g32(b, 36);
                if rep != TREP_NONE && rep >= n_treps {
                    return Err(corrupt(format!(
                        "text {} rep index",
                        i
                    )));
                }
                let bb = gbox(b, 40);
                if bb.x1 < bb.x0 || bb.y1 < bb.y0 {
                    return Err(corrupt(format!(
                        "text {} bbox inverted",
                        i
                    )));
                }
                let (x, y) = (gi64(b, 8), gi64(b, 16));
                let anchor = BBox { x0: x, y0: y, x1: x, y1: y };
                if !contains_box(&bb, &anchor) {
                    return Err(corrupt(format!(
                        "text {} bbox misses anchor",
                        i
                    )));
                }
                if rep != TREP_NONE {
                    let rb = &tpb[rep as usize * TREP_LEN..];
                    match rb[0] {
                        1 => {
                            let na = g32(rb, 4) as i128;
                            let nb = g32(rb, 8) as i128;
                            let va = (gi64(rb, 16), gi64(rb, 24));
                            let vb = (gi64(rb, 32), gi64(rb, 40));
                            for &(ii, jj) in &[
                                (0, 0),
                                (na - 1, 0),
                                (0, nb - 1),
                                (na - 1, nb - 1),
                            ] {
                                let px = x as i128
                                    + ii * va.0 as i128
                                    + jj * vb.0 as i128;
                                let py = y as i128
                                    + ii * va.1 as i128
                                    + jj * vb.1 as i128;
                                let (Ok(px), Ok(py)) =
                                    (i64::try_from(px), i64::try_from(py))
                                else {
                                    return Err(corrupt(format!(
                                        "text {} grid bbox overflow",
                                        i
                                    )));
                                };
                                let p = BBox {
                                    x0: px,
                                    y0: py,
                                    x1: px,
                                    y1: py,
                                };
                                if !contains_box(&bb, &p) {
                                    return Err(corrupt(format!(
                                        "text {} bbox misses grid",
                                        i
                                    )));
                                }
                            }
                        }
                        2 => {
                            let lo = g32(rb, 12);
                            let cnt = g32(rb, 56);
                            for k in 0..cnt {
                                let cb = gbox(
                                    &tcb[(lo + k) as usize * TCHUNK_LEN..],
                                    0,
                                );
                                let translated = BBox {
                                    x0: x.checked_add(cb.x0).ok_or_else(|| {
                                        corrupt(format!(
                                            "text {} pts bbox overflow",
                                            i
                                        ))
                                    })?,
                                    y0: y.checked_add(cb.y0).ok_or_else(|| {
                                        corrupt(format!(
                                            "text {} pts bbox overflow",
                                            i
                                        ))
                                    })?,
                                    x1: x.checked_add(cb.x1).ok_or_else(|| {
                                        corrupt(format!(
                                            "text {} pts bbox overflow",
                                            i
                                        ))
                                    })?,
                                    y1: y.checked_add(cb.y1).ok_or_else(|| {
                                        corrupt(format!(
                                            "text {} pts bbox overflow",
                                            i
                                        ))
                                    })?,
                                };
                                if !contains_box(&bb, &translated) {
                                    return Err(corrupt(format!(
                                        "text {} bbox misses pts chunk {}",
                                        i, k
                                    )));
                                }
                            }
                        }
                        _ => unreachable!("trep kind checked above"),
                    }
                }
            }
            // trange ownership: every text belongs to EXACTLY one
            // run, run texts match the owning (cell, layer), every
            // trange belongs to exactly one cell, and each run's
            // TBVH is a tree over that run alone (no sharing, no
            // cycles - a shared/cyclic node would loop the planner)
            let cb = sec(SEC_CELLS);
            let trb = sec(SEC_TRANGES);
            let txb = sec(SEC_TEXTS);
            let tvb = sec(SEC_TBVH);
            let mut trange_owned = 0u64;
            let mut text_owned = 0u64;
            let mut tr_claim =
                vec![false; n_tranges as usize];
            let mut tx_claim = vec![false; n_texts as usize];
            let mut node_claim = vec![false; n_tbvh as usize];
            for ci in 0..n_cells as usize {
                let c = &cb[ci * CELL_LEN..];
                let (ts, tc) = (g32(c, 128), g32(c, 132));
                for k in 0..tc {
                    let tri = (ts + k) as usize;
                    if tr_claim[tri] {
                        return Err(corrupt(format!(
                            "trange {} claimed twice",
                            tri
                        )));
                    }
                    tr_claim[tri] = true;
                    trange_owned += 1;
                    let tr = &trb[tri * TRANGE_LEN..];
                    let tl = g32(tr, 0);
                    let (lo, cnt) = (g32(tr, 4), g32(tr, 8));
                    for ti in lo..lo.saturating_add(cnt) {
                        let t = &txb[ti as usize * TEXT_LEN..];
                        if g32(t, 0) != ci as u32
                            || g32(t, 4) != tl
                        {
                            return Err(corrupt(format!(
                                "cell {} trange {} text {} \
                                 ownership",
                                ci, tri, ti
                            )));
                        }
                        if tx_claim[ti as usize] {
                            return Err(corrupt(format!(
                                "text {} claimed twice",
                                ti
                            )));
                        }
                        tx_claim[ti as usize] = true;
                        text_owned += 1;
                    }
                    let root = g32(tr, 12);
                    if root == TBVH_NONE {
                        continue;
                    }
                    let mut stack = vec![root];
                    while let Some(ni) = stack.pop() {
                        if node_claim[ni as usize] {
                            return Err(corrupt(format!(
                                "tbvh node {} shared or cyclic",
                                ni
                            )));
                        }
                        node_claim[ni as usize] = true;
                        let nb = &tvb[ni as usize * TBVH_LEN..];
                        let nbb = gbox(nb, 0);
                        let first = g32(nb, 32);
                        let count = g16(nb, 36) as u32;
                        if g16(nb, 38) != 0 {
                            if first < lo
                                || first + count > lo + cnt
                            {
                                return Err(corrupt(format!(
                                    "tbvh node {} leaf outside \
                                     trange {}",
                                    ni, tri
                                )));
                            }
                            for ti in first..first + count {
                                let tbb = gbox(
                                    &txb[ti as usize * TEXT_LEN..],
                                    40,
                                );
                                if !contains_box(&nbb, &tbb) {
                                    return Err(corrupt(format!(
                                        "tbvh node {} bbox misses text {}",
                                        ni, ti
                                    )));
                                }
                            }
                        } else {
                            for k2 in 0..count {
                                let child = first + k2;
                                let cbb = gbox(
                                    &tvb[child as usize * TBVH_LEN..],
                                    0,
                                );
                                if !contains_box(&nbb, &cbb) {
                                    return Err(corrupt(format!(
                                        "tbvh node {} bbox misses child {}",
                                        ni, child
                                    )));
                                }
                                stack.push(child);
                            }
                        }
                    }
                }
            }
            if trange_owned != n_tranges as u64 {
                return Err(corrupt("tranges not fully owned"));
            }
            if text_owned != n_texts as u64 {
                return Err(corrupt("texts not fully owned"));
            }
        }

        Ok(Ovm {
            unit: f64::from_le_bytes(data[16..24].try_into().unwrap()),
            src_size: g64(&data, 24),
            src_mtime: g64(&data, 32),
            top,
            n_layers,
            n_cells,
            n_pages,
            n_places,
            n_bvh,
            n_pranges,
            n_pbvh,
            n_texts,
            n_tranges,
            n_tbvh,
            n_treps,
            n_tchunks,
            ovp_len,
            ovt_len,
            bs_width,
            secs,
            data,
        })
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
            height: g32(b, 8),
            topo_rank: g32(b, 12),
            dbbox: gbox(b, 16),
            rbbox: gbox(b, 48),
            place_start: g32(b, 80),
            place_count: g32(b, 84),
            page_start: g32(b, 88),
            page_count: g32(b, 92),
            bvh_start: g32(b, 96),
            bvh_count: g32(b, 100),
            lmask_direct: g32(b, 104),
            lmask_rec: g32(b, 108),
            rec_members: g64(b, 112),
            prange_start: g32(b, 120),
            prange_count: g32(b, 124),
            trange_start: g32(b, 128),
            trange_count: g32(b, 132),
            tmask_rec: g32(b, 136),
        }
    }

    /// no-alloc cell fields for the planner's hot triage path
    /// (Ovm::cell materializes the name String - 50M-instance walks
    /// must not allocate per visit)
    pub fn cell_rbbox(&self, i: u32) -> BBox {
        assert!(i < self.n_cells, "cell index");
        gbox(&self.sec(SEC_CELLS)[i as usize * CELL_LEN..], 48)
    }

    pub fn cell_lmask_rec(&self, i: u32) -> u32 {
        assert!(i < self.n_cells, "cell index");
        g32(&self.sec(SEC_CELLS)[i as usize * CELL_LEN..], 108)
    }

    /// recursive text-layer bitset index (v5) - the label walk's
    /// subtree prune; no-alloc like cell_lmask_rec
    pub fn cell_tmask_rec(&self, i: u32) -> u32 {
        assert!(i < self.n_cells, "cell index");
        g32(&self.sec(SEC_CELLS)[i as usize * CELL_LEN..], 136)
    }

    /// (trange_start, trange_count) without materializing the name
    pub fn cell_tranges(&self, i: u32) -> (u32, u32) {
        assert!(i < self.n_cells, "cell index");
        let b = &self.sec(SEC_CELLS)[i as usize * CELL_LEN..];
        (g32(b, 128), g32(b, 132))
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
            1 => Rep::Grid { na: na as u64, nb: nb as u64, va, vb },
            2 => {
                let r = self.pts_ref(i).expect("kind 2");
                let mut pts = Vec::with_capacity(na as usize);
                for k in 0..na {
                    pts.push(r.pt(k));
                }
                Rep::Pts(pts.into())
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

    /// place record header without rep materialization - the
    /// planner's hot path (kind 2 goes through pts_ref, zero-copy)
    pub fn place_head(&self, i: u64) -> PlaceHead {
        assert!(i < self.n_places, "place index");
        let b = &self.sec(SEC_PLACES)[i as usize * PLACE_LEN..];
        PlaceHead {
            child: g32(b, 0),
            rot: b[4],
            flip: b[5] != 0,
            kind: b[6],
            x: gi64(b, 8),
            y: gi64(b, 16),
            na: g32(b, 24),
            nb: g32(b, 28),
            va: (gi64(b, 32), gi64(b, 40)),
            vb: (gi64(b, 48), gi64(b, 56)),
        }
    }

    /// zero-copy pts pool entry of place i (None unless kind 2)
    pub fn pts_ref(&self, i: u64) -> Option<PtsRef<'_>> {
        assert!(i < self.n_places, "place index");
        let sec = self.sec(SEC_PLACES);
        let b = &sec[i as usize * PLACE_LEN..];
        if b[6] != 2 {
            return None;
        }
        let pool = &sec[self.n_places as usize * PLACE_LEN..];
        let off = gi64(b, 32) as usize;
        let entry = &pool[off..];
        Some(PtsRef {
            count: g32(b, 24),
            n_chunks: g32(entry, 32),
            entry,
        })
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
            seq: g32(b, 8),
            lod: b[12],
            codec: b[13],
            bbox: gbox(b, 16),
            file_off: g64(b, 48),
            csize: g32(b, 56),
            usize_: g32(b, 60),
            records: g32(b, 64),
            lod_page: g32(b, 68),
            members: g64(b, 72),
            max_w: g64(b, 80),
            max_h: g64(b, 88),
        }
    }

    pub fn prange(&self, i: u32) -> PrangeV {
        assert!(i < self.n_pranges, "prange index");
        let b = &self.sec(SEC_PRANGES)[i as usize * PRANGE_LEN..];
        PrangeV {
            layer_idx: g32(b, 0),
            page_lo: g32(b, 4),
            page_count: g32(b, 8),
            pbvh_root: g32(b, 12),
        }
    }

    pub fn pbvh(&self, i: u32) -> PbvhV {
        assert!(i < self.n_pbvh, "pbvh index");
        let b = &self.sec(SEC_PBVH)[i as usize * PBVH_LEN..];
        PbvhV {
            bbox: gbox(b, 0),
            first: g32(b, 32),
            count: g16(b, 36),
            leaf: b[38] != 0,
            max_w: g64(b, 40),
            max_h: g64(b, 48),
        }
    }

    pub fn bitset(&self, idx: u32) -> &[u8] {
        let s = self.sec(SEC_BITSETS);
        let o = idx as usize * self.bs_width;
        &s[o..o + self.bs_width]
    }

    pub fn text(&self, i: u32) -> TextV {
        assert!(i < self.n_texts, "text index");
        let b = &self.sec(SEC_TEXTS)[i as usize * TEXT_LEN..];
        TextV {
            cell: g32(b, 0),
            layer_idx: g32(b, 4),
            x: gi64(b, 8),
            y: gi64(b, 16),
            string_off: g64(b, 24),
            string_len: g32(b, 32),
            rep_idx: g32(b, 36),
            bbox: gbox(b, 40),
            seq: g32(b, 72),
        }
    }

    pub fn trange(&self, i: u32) -> TrangeV {
        assert!(i < self.n_tranges, "trange index");
        let b = &self.sec(SEC_TRANGES)[i as usize * TRANGE_LEN..];
        TrangeV {
            layer_idx: g32(b, 0),
            text_lo: g32(b, 4),
            text_count: g32(b, 8),
            tbvh_root: g32(b, 12),
        }
    }

    pub fn tbvh(&self, i: u32) -> NodeV {
        assert!(i < self.n_tbvh, "tbvh index");
        let b = &self.sec(SEC_TBVH)[i as usize * TBVH_LEN..];
        NodeV {
            bbox: gbox(b, 0),
            first: g32(b, 32),
            count: g16(b, 36),
            leaf: g16(b, 38) != 0,
        }
    }

    pub fn trep(&self, i: u32) -> TrepV {
        assert!(i < self.n_treps, "trep index");
        let b = &self.sec(SEC_TREPS)[i as usize * TREP_LEN..];
        match b[0] {
            1 => TrepV::Grid {
                na: g32(b, 4),
                nb: g32(b, 8),
                va: (gi64(b, 16), gi64(b, 24)),
                vb: (gi64(b, 32), gi64(b, 40)),
            },
            2 => TrepV::Pts {
                count: g32(b, 4),
                pts_off: g64(b, 48),
                chunk_lo: g32(b, 12),
                chunk_count: g32(b, 56),
            },
            k => panic!("ovm: trep kind {}", k),
        }
    }

    pub fn tchunk(&self, i: u32) -> BBox {
        assert!(i < self.n_tchunks, "tchunk index");
        gbox(&self.sec(SEC_TCHUNKS)[i as usize * TCHUNK_LEN..], 0)
    }
}

/// page payload cell name - the builder and every consumer (delta
/// splicer, viewer eviction) must agree on it
/// LOD variant page cell name: the exact page's name + "q". The
/// "P" prefix is load-bearing - the viewer collects/evicts page
/// cells by that prefix and resolves the design name from the ci
/// that follows it, so LOD pages ride the same paths untouched.
pub fn lod_cell_name(cell: u32, layer_idx: u32, seq: u32) -> String {
    format!("P{}_{}_{}q", cell, layer_idx, seq)
}

pub fn page_cell_name(cell: u32, layer_idx: u32, seq: u32) -> String {
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

    fn build_sample() -> Vec<u8> {
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
        b.place(
            0,
            1,
            2,
            0,
            false,
            &Rep::Pts(vec![(0, 0), (9, 9), (9, 9), (-4, 2)].into()),
        );
        let bb = BBox { x0: 0, y0: 0, x1: 100, y1: 50 };
        let n0 = b.bvh_node(&bb, p0 as u32, 2, true);
        b.page(1, 0, 0, &bb, 128, 10, 20, 3, 9, 60, 60, LOD_EXACT, LOD_PAGE_NONE);
        b.page(1, 0, 70000, &bb, 138, 11, 21, 4, 8, 61, 1 << 40, LOD_EXACT, LOD_PAGE_NONE);
        let pv0 = b.pbvh_node(&bb, 0, 2, true, 61, 1 << 40);
        let pr0 = b.prange(0, 0, 2, pv0);
        b.cell("LEAF", 0, 1, &bb, &bb, 0, 0, 0, 0, 0, 0, 0, 0, m0, m1, 9,
               0, 0, m0);
        b.cell(
            "TOP",
            1,
            0,
            &bb,
            &bb,
            p0 as u32,
            2,
            0,
            2,
            n0,
            1,
            pr0,
            1,
            m0,
            m1,
            18,
            0,
            0,
            m0,
        );
        b.finish(150, 0)
    }

    #[test]
    fn roundtrip_v2() {
        let bytes = build_sample();
        let v = Ovm::from_bytes(bytes).unwrap();
        assert_eq!(v.n_cells, 2);
        assert_eq!(v.top, 1);
        assert_eq!(v.ovp_len, 150);
        assert_eq!(v.n_pranges, 1);
        assert_eq!(v.n_pbvh, 1);
        assert_eq!(v.layer(0).name, "M1");
        let c = v.cell(1);
        assert_eq!(c.name, "TOP");
        assert_eq!((c.height, c.topo_rank), (1, 0));
        assert_eq!((c.place_count, c.prange_count), (2, 1));
        let pl = v.place(0);
        assert_eq!((pl.x, pl.y, pl.rot, pl.flip), (10, -20, 1, true));
        assert!(matches!(pl.rep, Rep::Grid { na: 3, nb: 2, .. }));
        // pts: member multiset preserved (incl. duplicate), Morton
        // reorder allowed
        let pl2 = v.place(1);
        match &pl2.rep {
            Rep::Pts(p) => {
                let mut got = p.to_vec();
                got.sort_unstable();
                assert_eq!(
                    got,
                    vec![(-4, 2), (0, 0), (9, 9), (9, 9)]
                );
            }
            r => panic!("{:?}", r),
        }
        // zero-copy accessor agrees with the materialized rep
        let r = v.pts_ref(1).unwrap();
        assert_eq!(r.count, 4);
        assert_eq!(r.n_chunks, 1);
        let ext = r.extent();
        assert_eq!((ext.x0, ext.y0, ext.x1, ext.y1), (-4, 0, 9, 9));
        assert_eq!(r.chunk_bbox(0), ext);
        assert_eq!(r.chunk_range(0), (0, 4));
        match &pl2.rep {
            Rep::Pts(p) => {
                for k in 0..4 {
                    assert_eq!(r.pt(k), p[k as usize]);
                }
            }
            _ => unreachable!(),
        }
        assert!(v.pts_ref(0).is_none());
        let n = v.bvh(0);
        assert!(n.leaf && n.count == 2);
        let pg1 = v.page(1);
        assert_eq!(
            (pg1.layer_idx, pg1.seq, pg1.file_off, pg1.max_h),
            (0, 70000, 138, 1 << 40)
        );
        let pr = v.prange(0);
        assert_eq!(
            (pr.layer_idx, pr.page_lo, pr.page_count, pr.pbvh_root),
            (0, 0, 2, 0)
        );
        let pn = v.pbvh(0);
        assert!(pn.leaf && pn.count == 2 && pn.max_h == 1 << 40);
        assert!(masks_intersect(
            v.bitset(v.cell(1).lmask_direct),
            &[0b10, 0]
        ));
    }

    /// v5 text index: one cell, one layer, three records (One /
    /// Grid / Morton Pts with 2 chunks), a trange with a 3-node
    /// TBVH - byte roundtrip through every accessor, then the
    /// text-specific corrupt gates (deep tier)
    #[test]
    fn text_sections_roundtrip_and_corrupt() {
        // ovt: "AB" + "hello" strings, then a 300-point pool
        let mut ovt: Vec<u8> = Vec::new();
        ovt.extend_from_slice(b"AB");
        ovt.extend_from_slice(b"hello");
        let src: Vec<(i64, i64)> = (0..300)
            .map(|i| ((i * 37) % 997 - 400, (i * 91) % 613))
            .collect();
        let prep = prepare_pts(&src);
        let pts_off = ovt.len() as u64;
        for &(x, y) in &prep.pts {
            ovt.extend_from_slice(&x.to_le_bytes());
            ovt.extend_from_slice(&y.to_le_bytes());
        }

        let build = |ovt_len: u64| -> Vec<u8> {
            let mut b = Builder::new(1000.0, 0, 0, 1);
            b.top = 0;
            b.layer(7, 0, "TXT", 0, 0);
            let m = b.bitset(&[1]);
            let rg = b.trep_grid(4, 3, (10, 0), (0, 20));
            let chunk_lo = b.n_tchunks();
            let prep2 = prepare_pts(&src);
            for c in &prep2.chunks {
                b.tchunk(c);
            }
            let rp = b.trep_pts(
                300,
                pts_off,
                chunk_lo,
                prep2.chunks.len() as u32,
            );
            let t0 = b.n_texts();
            let one = BBox { x0: 5, y0: 6, x1: 5, y1: 6 };
            b.text(0, 0, 5, 6, 0, 2, TREP_NONE, &one, 0);
            let gb = BBox { x0: 0, y0: 0, x1: 30, y1: 40 };
            b.text(0, 0, 0, 0, 2, 5, rg, &gb, 1);
            let mut pb = prep2.extent;
            pb.x0 += 1;
            pb.y0 += 1;
            pb.x1 += 1;
            pb.y1 += 1;
            b.text(0, 0, 1, 1, 0, 2, rp, &pb, 2);
            let r0 = b.tbvh_node(&one, t0, 1, true);
            let mut right = gb;
            right.grow(&pb);
            let _r1 = b.tbvh_node(&right, t0 + 1, 2, true);
            let mut ub = one;
            ub.grow(&gb);
            ub.grow(&pb);
            let root = b.tbvh_node(&ub, r0, 2, false);
            let tr = b.trange(0, t0, 3, root);
            let bb = BBox { x0: 0, y0: 0, x1: 100, y1: 100 };
            b.cell("T", 0, 0, &bb, &bb, 0, 0, 0, 0, 0, 0, 0, 0,
                   m, m, 0, tr, 1, m);
            b.finish(0, ovt_len)
        };
        let good = build(ovt.len() as u64);
        let v = Ovm::from_bytes(good.clone()).unwrap();
        assert_eq!(v.ovt_len, ovt.len() as u64);
        assert_eq!(
            (v.n_texts, v.n_tranges, v.n_tbvh, v.n_treps),
            (3, 1, 3, 2)
        );
        assert_eq!(v.n_tchunks, 300u32.div_ceil(PTS_CHUNK as u32));
        let c = v.cell(0);
        assert_eq!((c.trange_start, c.trange_count), (0, 1));
        assert_eq!(v.cell_tranges(0), (0, 1));
        assert_eq!(v.cell_tmask_rec(0), c.tmask_rec);
        let tr = v.trange(0);
        assert_eq!(
            (tr.layer_idx, tr.text_lo, tr.text_count),
            (0, 0, 3)
        );
        let t = v.text(0);
        assert_eq!((t.x, t.y, t.seq), (5, 6, 0));
        assert_eq!(t.rep_idx, TREP_NONE);
        assert_eq!(
            &ovt[t.string_off as usize
                ..(t.string_off + t.string_len as u64) as usize],
            b"AB"
        );
        match v.trep(v.text(1).rep_idx) {
            TrepV::Grid { na, nb, va, vb } => {
                assert_eq!((na, nb, va, vb), (4, 3, (10, 0), (0, 20)));
            }
            r => panic!("{:?}", r),
        }
        match v.trep(v.text(2).rep_idx) {
            TrepV::Pts { count, pts_off: po, chunk_lo, chunk_count } => {
                assert_eq!(count, 300);
                assert_eq!(po, pts_off);
                assert_eq!(chunk_count, v.n_tchunks - chunk_lo);
                // pool pt via the zero-copy helper == prepared pts
                for s in [0u32, 150, 299] {
                    assert_eq!(
                        ovt_pt(&ovt, po, s),
                        prep.pts[s as usize]
                    );
                }
                // chunk bboxes cover their slots
                let cb = v.tchunk(chunk_lo);
                let (lo, hi) = (0usize, PTS_CHUNK.min(300));
                let mut want = BBox::EMPTY;
                for &(x, y) in &prep.pts[lo..hi] {
                    want.grow(&BBox { x0: x, y0: y, x1: x, y1: y });
                }
                assert_eq!(cb, want);
            }
            r => panic!("{:?}", r),
        }
        let root = v.tbvh(tr.tbvh_root);
        assert!(!root.leaf && root.count == 2);

        let sec_off = |bytes: &[u8], s: usize| {
            u64::from_le_bytes(
                bytes[88 + s * 16..96 + s * 16].try_into().unwrap(),
            ) as usize
        };
        // (a) trange layer index out of range (deep tier)
        let mut c = good.clone();
        let o = sec_off(&c, SEC_TRANGES);
        c[o..o + 4].copy_from_slice(&99u32.to_le_bytes());
        let e = err_of(Ovm::from_bytes(c));
        assert!(e.contains("trange 0 layer index"), "{}", e);
        // (b) trep kind whitelist
        let mut c = good.clone();
        let o = sec_off(&c, SEC_TREPS);
        c[o] = 9;
        let e = err_of(Ovm::from_bytes(c));
        assert!(e.contains("trep 0 kind"), "{}", e);
        // (c) pts pool beyond ovt_len (header lies short)
        let short = build(ovt.len() as u64 - 1);
        let e = err_of(Ovm::from_bytes(short));
        assert!(e.contains("beyond ovt_len"), "{}", e);
        // (d) text stolen by another cell (ownership, deep)
        let mut c = good.clone();
        let o = sec_off(&c, SEC_TEXTS);
        c[o..o + 4].copy_from_slice(&7u32.to_le_bytes());
        let e = err_of(Ovm::from_bytes(c));
        assert!(e.contains("cell index"), "{}", e);
        // (e) tbvh cycle: root's child range {1, 2} includes the
        // root itself - in bounds, so only the DFS can catch it
        let mut c = good.clone();
        let o = sec_off(&c, SEC_TBVH);
        let root_i = tr.tbvh_root as usize;
        c[o + root_i * TBVH_LEN + 32..o + root_i * TBVH_LEN + 36]
            .copy_from_slice(&(root_i as u32 - 1).to_le_bytes());
        let e = err_of(Ovm::from_bytes(c));
        assert!(e.contains("shared or cyclic"), "{}", e);
        // (f) leaf range escaping its trange
        let mut c = good.clone();
        let o = sec_off(&c, SEC_TRANGES);
        c[o + 8..o + 12].copy_from_slice(&2u32.to_le_bytes());
        let e = err_of(Ovm::from_bytes(c));
        assert!(
            e.contains("leaf outside") || e.contains("fully owned"),
            "{}",
            e
        );
        // (g) a structurally valid but too-small node bbox would
        // silently cull labels unless the deep gate checks soundness
        let mut c = good.clone();
        let o = sec_off(&c, SEC_TBVH) + root_i * TBVH_LEN;
        c[o + 16..o + 24].copy_from_slice(&0i64.to_le_bytes());
        let e = err_of(Ovm::from_bytes(c));
        assert!(e.contains("bbox misses child"), "{}", e);
        // (h) chunk bbox inversion must not reach the planner
        let mut c = good.clone();
        let o = sec_off(&c, SEC_TCHUNKS);
        c[o..o + 8].copy_from_slice(&10i64.to_le_bytes());
        c[o + 16..o + 24].copy_from_slice(&0i64.to_le_bytes());
        let e = err_of(Ovm::from_bytes(c));
        assert!(e.contains("tchunk 0 bbox inverted"), "{}", e);
    }

    #[test]
    fn morton_prepare_deterministic() {
        let src: Vec<(i64, i64)> = (0..1000)
            .map(|i| ((i * 7919) % 501 - 250, (i * 104729) % 733))
            .collect();
        let a = prepare_pts(&src);
        let b = prepare_pts(&src);
        assert_eq!(a.pts, b.pts);
        assert_eq!(a.chunks, b.chunks);
        // multiset preserved
        let mut s1 = src.clone();
        let mut s2 = a.pts.clone();
        s1.sort_unstable();
        s2.sort_unstable();
        assert_eq!(s1, s2);
        // keys are non-decreasing in pool order
        for w in a.pts.windows(2) {
            let ka = morton_key(w[0].0, w[0].1, a.extent.x0, a.extent.y0);
            let kb = morton_key(w[1].0, w[1].1, a.extent.x0, a.extent.y0);
            assert!(ka <= kb);
        }
        // chunk bboxes cover their slots exactly
        assert_eq!(a.chunks.len(), 1000usize.div_ceil(PTS_CHUNK));
        for (k, cb) in a.chunks.iter().enumerate() {
            let lo = k * PTS_CHUNK;
            let hi = (lo + PTS_CHUNK).min(a.pts.len());
            let mut bb = BBox::EMPTY;
            for &(x, y) in &a.pts[lo..hi] {
                bb.grow(&BBox { x0: x, y0: y, x1: x, y1: y });
            }
            assert_eq!(*cb, bb);
        }
    }

    #[test]
    #[should_panic(expected = "limit exceeded: name length")]
    fn narrow_name_panics() {
        let mut b = Builder::new(1000.0, 0, 0, 1);
        let long = "x".repeat(70000);
        b.name(&long);
    }

    fn err_of(r: Result<Ovm, String>) -> String {
        match r {
            Err(e) => e,
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn corrupt_gates() {
        // lod link hardening (review round): bad lod byte, link
        // to a non-MERGED page, seq mismatch, double claim
        {
            let mut b = Builder::new(1000.0, 0, 0, 1);
            b.top = 0;
            b.layer(1, 0, "L", 2, 2);
            let m = b.bitset(&[1]);
            let pb = BBox { x0: 0, y0: 0, x1: 10, y1: 10 };
            b.page(0, 0, 0, &pb, 0, 0, 0, 1, 1, 1, 1, LOD_EXACT, 2);
            // seq 0 like page 0 (names are a build concern, not
            // an ovm invariant) so a retargeted link reaches the
            // double-claim check instead of the seq gate
            b.page(0, 0, 0, &pb, 0, 0, 0, 1, 1, 1, 1, LOD_EXACT,
                   LOD_PAGE_NONE);
            b.page(0, 0, 0, &pb, 0, 0, 0, 1, 1, 1, 1, LOD_MERGED,
                   LOD_PAGE_NONE);
            let pr = b.prange(0, 0, 2, PBVH_NONE);
            b.cell("T", 0, 0, &pb, &pb, 0, 0, 0, 3, 0, 0, pr, 1,
                   m, m, 2, 0, 0, m);
            let good = b.finish(0, 0);
            assert!(Ovm::from_bytes(good.clone()).is_ok());
            let pg0 = u64::from_le_bytes(
                good[88 + 16 * SEC_PAGEDIR
                    ..96 + 16 * SEC_PAGEDIR]
                    .try_into()
                    .unwrap(),
            ) as usize;
            // (a) bad lod byte
            let mut c = good.clone();
            c[pg0 + PAGE_LEN + 12] = 7;
            let e = err_of(Ovm::from_bytes(c));
            assert!(e.contains("lod byte"), "{}", e);
            // (b) exact linking to an EXACT page
            let mut c = good.clone();
            c[pg0 + 68..pg0 + 72]
                .copy_from_slice(&1u32.to_le_bytes());
            let e = err_of(Ovm::from_bytes(c));
            assert!(e.contains("lod link target"), "{}", e);
            // (c) two exacts claiming one LOD
            let mut c = good.clone();
            c[pg0 + PAGE_LEN + 68..pg0 + PAGE_LEN + 72]
                .copy_from_slice(&2u32.to_le_bytes());
            let e = err_of(Ovm::from_bytes(c));
            assert!(e.contains("claimed twice"), "{}", e);
            // (e) prange run extended to swallow the MERGED
            // page: cross-table ownership must reject it
            let mut c = good.clone();
            let pr0 = u64::from_le_bytes(
                c[88 + 16 * SEC_PRANGES..96 + 16 * SEC_PRANGES]
                    .try_into()
                    .unwrap(),
            ) as usize;
            c[pr0 + 8..pr0 + 12]
                .copy_from_slice(&3u32.to_le_bytes());
            let e = err_of(Ovm::from_bytes(c));
            assert!(e.contains("ownership"), "{}", e);
            // (d) seq mismatch: retarget the MERGED page's seq
            let mut c = good.clone();
            c[pg0 + 2 * PAGE_LEN + 8..pg0 + 2 * PAGE_LEN + 12]
                .copy_from_slice(&9u32.to_le_bytes());
            let e = err_of(Ovm::from_bytes(c));
            assert!(e.contains("lod link target"), "{}", e);
        }

        // version gate
        let mut bytes = build_sample();
        bytes[8] = 1;
        let e = err_of(Ovm::from_bytes(bytes));
        assert!(e.contains("ovm version 1"), "{}", e);
        // truncated tail -> section out of range
        let bytes = build_sample();
        let cut = bytes[..bytes.len() - 10].to_vec();
        let e = err_of(Ovm::from_bytes(cut));
        assert!(e.contains("corrupt cache"), "{}", e);
        // bs_width 0 guard (no divide-by-zero panic)
        let mut bytes = build_sample();
        bytes[12..16].copy_from_slice(&0u32.to_le_bytes());
        let e = err_of(Ovm::from_bytes(bytes));
        assert!(e.contains("bitset width 0"), "{}", e);
        // page span beyond committed ovp_len
        let mut b = Builder::new(1000.0, 0, 0, 1);
        b.top = 0;
        b.layer(1, 0, "L", 0, 0);
        let m = b.bitset(&[0]);
        let bb = BBox { x0: 0, y0: 0, x1: 1, y1: 1 };
        b.page(0, 0, 0, &bb, 100, 50, 50, 1, 1, 1, 1, LOD_EXACT, LOD_PAGE_NONE);
        b.cell("A", 0, 0, &bb, &bb, 0, 0, 0, 1, 0, 0, 0, 0, m, m, 1,
               0, 0, m);
        let e = err_of(Ovm::from_bytes(b.finish(120, 0)));
        assert!(e.contains("beyond ovp_len"), "{}", e);
        // corrupt page/prange layer or cell indexes must be an open
        // error, never a later planner panic (review finding)
        let sec_off = |bytes: &[u8], sec: usize| {
            u64::from_le_bytes(
                bytes[88 + sec * 16..96 + sec * 16].try_into().unwrap(),
            ) as usize
        };
        let mut bytes = build_sample();
        let po = sec_off(&bytes, 6); // pagedir
        bytes[po + 4..po + 8].copy_from_slice(&999u32.to_le_bytes());
        let e = err_of(Ovm::from_bytes(bytes));
        assert!(e.contains("page 0 layer index"), "{}", e);
        let mut bytes = build_sample();
        let po = sec_off(&bytes, 6);
        bytes[po..po + 4].copy_from_slice(&999u32.to_le_bytes());
        let e = err_of(Ovm::from_bytes(bytes));
        assert!(e.contains("page 0 cell index"), "{}", e);
        let mut bytes = build_sample();
        let ro = sec_off(&bytes, 7); // pranges
        bytes[ro..ro + 4].copy_from_slice(&999u32.to_le_bytes());
        let e = err_of(Ovm::from_bytes(bytes));
        assert!(e.contains("prange 0 layer index"), "{}", e);
    }
}
