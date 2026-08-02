//! VFS runtime (rust/VFS.md, V2): plan a viewport against design.ovm
//! without touching geometry, keep a working-set session, and build
//! delta OASIS files by splicing page payload bytes verbatim.

pub mod coverage;
pub mod hier;

use floe_oasis::write::{splice_tree, tree_body};
use floe_ovm::{BBox, Ovm, PlaceV};
use floe_oasis::doc::Rep;
use floe_tiler::hier::rep_extent;
use floe_tiler::Xf;

/// cap on cut-frame rectangles per view (pathological arrays of
/// below-cut cells would otherwise flood the delta)
const FRAME_CAP: usize = 200_000;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};

pub struct Vfs {
    pub ovm: Ovm,
    ovp_path: String,
}

#[derive(Clone, Debug)]
pub struct ViewReq {
    /// dbu, closed box
    pub view: BBox,
    /// features (and subtrees) smaller than this are culled
    pub cut_dbu: i64,
    /// visible-layer bitset (ovm layer order, bs_width bytes)
    pub vis: Vec<u8>,
    /// semantic depth limit (u32::MAX = full)
    pub depth: u32,
}

/// one placement of a page cell in the working-set top. na/nb/va/vb
/// carry a regular array so a cell array (SRAM bitcells, fill) stays
/// ONE placement instead of exploding into a Mat per member; single
/// placement = na=nb=1. Vectors are in WORLD coords.
#[derive(Clone, Copy, Debug)]
pub struct Mat {
    pub page: u32,
    pub x: i64,
    pub y: i64,
    pub rot: u8,
    pub flip: bool,
    pub na: u32,
    pub nb: u32,
    pub va: (i64, i64),
    pub vb: (i64, i64),
}

impl Mat {
    fn single(page: u32, x: i64, y: i64, rot: u8, flip: bool) -> Mat {
        Mat {
            page,
            x,
            y,
            rot,
            flip,
            na: 1,
            nb: 1,
            va: (0, 0),
            vb: (0, 0),
        }
    }
}

#[derive(Default, Debug)]
pub struct PlanStats {
    pub visited_cells: u64,
    pub visited_bvh: u64,
    pub cull_size: u64,
    pub cull_layer: u64,
    pub cull_page_size: u64,
    pub page_reads: u64,
    pub materializations: u64,
}

pub struct Plan {
    pub pages: Vec<u32>, // sorted unique
    pub mats: Vec<Mat>,
    /// world-frame bboxes of subtrees the size cut dropped: drawn as
    /// hollow outlines so a cut wide view keeps its floorplan instead
    /// of blanking (the coarse-LOD stand-in until V3 coverage)
    pub frames: Vec<BBox>,
    pub stats: PlanStats,
}

impl Vfs {
    pub fn open(dir: &str) -> Result<Vfs, String> {
        let ovm = Ovm::open(&format!("{}/design.ovm", dir))?;
        // viewer-side verification (VFS_HIER.md par.3.6): the marker
        // (design.ovm, structurally validated by from_bytes) commits
        // a specific ovp byte length - a mismatched pair is a broken
        // build or a mixed cache, never something to read through
        let ovp_path = format!("{}/design.ovp", dir);
        let ovp_size = std::fs::metadata(&ovp_path)
            .map_err(|e| format!("{}: {}", ovp_path, e))?
            .len();
        if ovp_size != ovm.ovp_len {
            return Err(format!(
                "corrupt cache; rebuild: design.ovp is {} bytes but \
                 design.ovm committed {}",
                ovp_size, ovm.ovp_len
            ));
        }
        Ok(Vfs { ovm, ovp_path })
    }

    pub fn page_name(&self, pi: u32) -> String {
        let p = self.ovm.page(pi);
        floe_ovm::page_cell_name(p.cell, p.layer_idx, p.seq)
    }

    /// visible-layer bitset from "l/d" or name specs (None = all)
    pub fn layer_mask(
        &self,
        specs: Option<&[String]>,
    ) -> Result<Vec<u8>, String> {
        let mut vis = vec![0u8; self.ovm.bs_width];
        match specs {
            None => vis.iter_mut().for_each(|b| *b = 0xff),
            Some(list) => {
                for spec in list {
                    let mut hit = false;
                    for li in 0..self.ovm.n_layers {
                        let lr = self.ovm.layer(li);
                        if lr.name == *spec
                            || format!("{}/{}", lr.layer, lr.dt)
                                == *spec
                        {
                            floe_ovm::bit_set(
                                &mut vis,
                                li as usize,
                            );
                            hit = true;
                        }
                    }
                    if !hit {
                        return Err(format!(
                            "layer {:?} not found",
                            spec
                        ));
                    }
                }
            }
        }
        Ok(vis)
    }

    pub fn plan(&self, req: &ViewReq) -> Plan {
        let mut st = PlanStats::default();
        let mut pages = HashSet::new();
        let mut mats = Vec::new();
        let mut frames = Vec::new();
        self.descend(
            self.ovm.top,
            &Xf::identity(),
            req,
            0,
            &mut pages,
            &mut mats,
            &mut frames,
            &mut st,
        );
        let mut pv: Vec<u32> = pages.into_iter().collect();
        pv.sort_unstable();
        Plan { pages: pv, mats, frames, stats: st }
    }

    #[allow(clippy::too_many_arguments)]
    fn descend(
        &self,
        ci: u32,
        xf: &Xf,
        req: &ViewReq,
        depth: u32,
        pages: &mut HashSet<u32>,
        mats: &mut Vec<Mat>,
        frames: &mut Vec<BBox>,
        st: &mut PlanStats,
    ) {
        let v = &self.ovm;
        let cell = v.cell(ci);
        st.visited_cells += 1;
        if !floe_ovm::masks_intersect(
            v.bitset(cell.lmask_rec),
            &req.vis,
        ) {
            st.cull_layer += 1;
            return;
        }
        let wb = xf_bbox(xf, &cell.rbbox);
        if !wb.intersects(&req.view) {
            return;
        }
        if (wb.x1 - wb.x0) < req.cut_dbu
            && (wb.y1 - wb.y0) < req.cut_dbu
        {
            st.cull_size += 1;
            return;
        }
        st.materializations += 1;
        let inv = xf.invert();
        let lview = xf_bbox(&inv, &req.view);
        let (px, py, prot, pflip) = xf.decompose();
        for pi in cell.page_start..cell.page_start + cell.page_count
        {
            let p = v.page(pi);
            st.page_reads += 1;
            if !floe_ovm::bit_test(&req.vis, p.layer_idx as usize) {
                continue;
            }
            if !p.bbox.intersects(&lview) {
                continue;
            }
            if p.max_w < req.cut_dbu.max(0) as u64
                && p.max_h < req.cut_dbu.max(0) as u64
            {
                st.cull_page_size += 1;
                continue;
            }
            pages.insert(pi);
            mats.push(Mat::single(pi, px, py, prot, pflip));
        }
        if depth >= req.depth || cell.bvh_count == 0 {
            return;
        }
        let mut stack = vec![cell.bvh_start];
        while let Some(ni) = stack.pop() {
            let node = v.bvh(ni);
            st.visited_bvh += 1;
            if !node.bbox.intersects(&lview) {
                continue;
            }
            if !node.leaf {
                for k in 0..node.count as u32 {
                    stack.push(node.first + k);
                }
                continue;
            }
            for k in 0..node.count as u64 {
                let pl = v.place(node.first as u64 + k);
                // offset-invariant culls BEFORE member enumeration
                let child = v.cell(pl.child);
                if !floe_ovm::masks_intersect(
                    v.bitset(child.lmask_rec),
                    &req.vis,
                ) {
                    st.cull_layer += 1;
                    continue;
                }
                let base = xf.compose(&Xf::place(
                    pl.x, pl.y, pl.rot, pl.flip,
                ));
                let cwb = xf_bbox(&base, &child.rbbox);
                // size is offset-invariant: a below-cut child is
                // below cut at every rep member. Frame it (a whole
                // array = one footprint outline) and skip descent.
                if !cwb.is_empty()
                    && (cwb.x1 - cwb.x0) < req.cut_dbu
                    && (cwb.y1 - cwb.y0) < req.cut_dbu
                {
                    st.cull_size += 1;
                    if frames.len() < FRAME_CAP {
                        frames.push(match &pl.rep {
                            Rep::One => cwb,
                            rep => rep_footprint(xf, &cwb, rep),
                        });
                    }
                    continue;
                }
                match &pl.rep {
                    Rep::One => self.descend(
                        pl.child,
                        &base,
                        req,
                        depth + 1,
                        pages,
                        mats,
                        frames,
                        st,
                    ),
                    Rep::Grid { na, nb, va, vb } => {
                        // preserve the array: descend the child ONCE
                        // (as if the whole cell were in view - klayout
                        // culls off-screen members at draw) and stamp
                        // the array rep onto the pages it produced, so
                        // a 2M-member SRAM array stays a handful of
                        // CellInstArray placements, not 2M Mats.
                        let start = mats.len();
                        let mut r2 = req.clone();
                        r2.view = cwb;
                        self.descend(
                            pl.child,
                            &base,
                            &r2,
                            depth + 1,
                            pages,
                            mats,
                            frames,
                            st,
                        );
                        let wva = xf.apply_vec(va.0, va.1);
                        let wvb = xf.apply_vec(vb.0, vb.1);
                        fold_array(
                            mats, start, *na as u32, *nb as u32,
                            wva, wvb,
                        );
                    }
                    rep => {
                        // irregular (point-list) rep: member count is
                        // bounded by the source list, so per-member
                        // expansion stays small
                        for (ox, oy) in self.visible_offsets(
                            xf, &pl, rep, &req.view,
                        ) {
                            let m = xf.compose(&Xf::place(
                                pl.x + ox,
                                pl.y + oy,
                                pl.rot,
                                pl.flip,
                            ));
                            self.descend(
                                pl.child,
                                &m,
                                req,
                                depth + 1,
                                pages,
                                mats,
                                frames,
                                st,
                            );
                        }
                    }
                }
            }
        }
    }

    /// rep member offsets whose child bbox can reach the view
    fn visible_offsets(
        &self,
        xf: &Xf,
        pl: &PlaceV,
        rep: &Rep,
        view: &BBox,
    ) -> Vec<(i64, i64)> {
        let cb = self.ovm.cell(pl.child).rbbox;
        let xfc =
            xf.compose(&Xf::place(pl.x, pl.y, pl.rot, pl.flip));
        let wb = xf_bbox(&xfc, &cb);
        if wb.is_empty() {
            return Vec::new();
        }
        let sx0 = view.x0 - wb.x1;
        let sx1 = view.x1 - wb.x0;
        let sy0 = view.y0 - wb.y1;
        let sy1 = view.y1 - wb.y0;
        let inv = xf.invert();
        let a = inv.apply_vec(sx0, sy0);
        let c = inv.apply_vec(sx1, sy1);
        let (ox0, ox1) = (a.0.min(c.0), a.0.max(c.0));
        let (oy0, oy1) = (a.1.min(c.1), a.1.max(c.1));
        let mut out = Vec::new();
        match rep {
            Rep::One => out.push((0, 0)),
            Rep::Grid { na, nb, va, vb } => {
                for j in 0..*nb as i64 {
                    let bx = j * vb.0;
                    let by = j * vb.1;
                    for i in axis_range(
                        *na as i64,
                        va.0,
                        ox0 - bx,
                        ox1 - bx,
                    )
                    .intersect(axis_range(
                        *na as i64,
                        va.1,
                        oy0 - by,
                        oy1 - by,
                    ))
                    .iter()
                    {
                        out.push((
                            i * va.0 + bx,
                            i * va.1 + by,
                        ));
                    }
                }
            }
            Rep::Pts(p) => {
                for &(x, y) in p {
                    if x >= ox0
                        && x <= ox1
                        && y >= oy0
                        && y <= oy1
                    {
                        out.push((x, y));
                    }
                }
            }
        }
        out
    }

    /// splice the given pages' payload bodies into one OASIS file
    pub fn delta(&self, pages: &[u32]) -> Result<Vec<u8>, String> {
        let mut f = std::fs::File::open(&self.ovp_path)
            .map_err(|e| format!("{}: {}", self.ovp_path, e))?;
        let mut payloads: Vec<Vec<u8>> =
            Vec::with_capacity(pages.len());
        // read in file order for sequential IO
        let mut order: Vec<u32> = pages.to_vec();
        order.sort_unstable_by_key(|&pi| {
            self.ovm.page(pi).file_off
        });
        for &pi in &order {
            let p = self.ovm.page(pi);
            let mut buf = vec![0u8; p.csize as usize];
            f.seek(SeekFrom::Start(p.file_off))
                .map_err(|e| e.to_string())?;
            f.read_exact(&mut buf).map_err(|e| e.to_string())?;
            payloads.push(buf);
        }
        let bodies: Vec<&[u8]> =
            payloads.iter().map(|p| tree_body(p)).collect();
        Ok(splice_tree(self.ovm.unit, &bodies))
    }
}

/// stamp a regular array (na x nb, world vectors wva/wvb) onto the
/// Mats added since `start`. Single Mats take the rep directly; a Mat
/// that already carries a rep (nested array) can't hold two, so its
/// outer members are materialized (bounded - nesting is shallow and
/// each level's count is modest).
fn fold_array(
    mats: &mut Vec<Mat>,
    start: usize,
    na: u32,
    nb: u32,
    wva: (i64, i64),
    wvb: (i64, i64),
) {
    let end = mats.len();
    for i in start..end {
        if mats[i].na == 1 && mats[i].nb == 1 {
            mats[i].na = na;
            mats[i].nb = nb;
            mats[i].va = wva;
            mats[i].vb = wvb;
        } else {
            let m = mats[i]; // keeps its inner rep; (0,0) stays here
            for jj in 0..nb as i64 {
                for ii in 0..na as i64 {
                    if ii == 0 && jj == 0 {
                        continue;
                    }
                    let mut c = m;
                    c.x += ii * wva.0 + jj * wvb.0;
                    c.y += ii * wva.1 + jj * wvb.1;
                    mats.push(c);
                }
            }
        }
    }
}

/// world footprint of a whole repetition whose offset-0 member has
/// world bbox `base_wb`: offsets translate in the parent frame, so
/// their world span is xf.apply_vec of the local offset extent.
fn rep_footprint(xf: &Xf, base_wb: &BBox, rep: &Rep) -> BBox {
    let ((ox0, ox1), (oy0, oy1)) = rep_extent(rep);
    let mut wx0 = i64::MAX;
    let mut wx1 = i64::MIN;
    let mut wy0 = i64::MAX;
    let mut wy1 = i64::MIN;
    for &(ox, oy) in
        &[(ox0, oy0), (ox1, oy0), (ox0, oy1), (ox1, oy1)]
    {
        let (dx, dy) = xf.apply_vec(ox, oy);
        wx0 = wx0.min(dx);
        wx1 = wx1.max(dx);
        wy0 = wy0.min(dy);
        wy1 = wy1.max(dy);
    }
    BBox {
        x0: base_wb.x0 + wx0,
        y0: base_wb.y0 + wy0,
        x1: base_wb.x1 + wx1,
        y1: base_wb.y1 + wy1,
    }
}

pub(crate) fn xf_bbox(xf: &Xf, b: &BBox) -> BBox {
    if b.is_empty() {
        return *b;
    }
    let a = xf.apply(b.x0, b.y0);
    let c = xf.apply(b.x1, b.y1);
    BBox {
        x0: a.0.min(c.0),
        y0: a.1.min(c.1),
        x1: a.0.max(c.0),
        y1: a.1.max(c.1),
    }
}

#[derive(Clone, Copy)]
pub struct IRange {
    pub lo: i64,
    pub hi: i64,
}

impl IRange {
    pub fn intersect(self, o: IRange) -> IRange {
        IRange { lo: self.lo.max(o.lo), hi: self.hi.min(o.hi) }
    }
    pub fn iter(self) -> impl Iterator<Item = i64> {
        self.lo..=self.hi
    }
}

/// i in [0, n) with lo <= i*step <= hi
pub fn axis_range(n: i64, step: i64, lo: i64, hi: i64) -> IRange {
    if step == 0 {
        return if lo <= 0 && 0 <= hi {
            IRange { lo: 0, hi: n - 1 }
        } else {
            IRange { lo: 1, hi: 0 }
        };
    }
    let (a, b) = if step > 0 {
        (
            floe_tiler::div_ceil(lo, step),
            floe_tiler::div_floor(hi, step),
        )
    } else {
        (
            floe_tiler::div_ceil(hi, step),
            floe_tiler::div_floor(lo, step),
        )
    };
    IRange { lo: a.max(0), hi: b.min(n - 1) }
}

// ------------------------------------------------------- session

/// Working-set bookkeeping: which page cells live in the viewer's
/// layout, LRU-evicted against a decoded-bytes budget. The viewer
/// applies `Update` verbatim: read the delta (new pages), drop the
/// evicted page cells, rebuild the top placements.
pub struct Session {
    resident: HashMap<u32, u64>, // page -> last-used generation
    bytes: HashMap<u32, u64>,
    resident_bytes: u64,
    pub budget_bytes: u64,
}

pub struct Update {
    pub new: Vec<u32>,
    pub evict: Vec<u32>,
    pub mats: Vec<Mat>,
}

impl Session {
    pub fn new(budget_bytes: u64) -> Session {
        Session {
            resident: HashMap::new(),
            bytes: HashMap::new(),
            resident_bytes: 0,
            budget_bytes,
        }
    }

    pub fn apply(
        &mut self,
        vfs: &Vfs,
        plan: &Plan,
        gen: u64,
    ) -> Update {
        let mut new = Vec::new();
        for &pi in &plan.pages {
            match self.resident.entry(pi) {
                std::collections::hash_map::Entry::Occupied(
                    mut e,
                ) => {
                    *e.get_mut() = gen;
                }
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(gen);
                    let b =
                        vfs.ovm.page(pi).usize_ as u64;
                    self.bytes.insert(pi, b);
                    self.resident_bytes += b;
                    new.push(pi);
                }
            }
        }
        // evict LRU pages not in this plan while over budget
        let mut evict = Vec::new();
        if self.resident_bytes > self.budget_bytes {
            let current: HashSet<u32> =
                plan.pages.iter().copied().collect();
            let mut cand: Vec<(u64, u32)> = self
                .resident
                .iter()
                .filter(|(pi, _)| !current.contains(pi))
                .map(|(&pi, &g)| (g, pi))
                .collect();
            cand.sort_unstable();
            for (_, pi) in cand {
                if self.resident_bytes <= self.budget_bytes {
                    break;
                }
                self.resident.remove(&pi);
                let b = self.bytes.remove(&pi).unwrap_or(0);
                self.resident_bytes -= b;
                evict.push(pi);
            }
        }
        Update { new, evict, mats: plan.mats.clone() }
    }

    pub fn resident_bytes(&self) -> u64 {
        self.resident_bytes
    }
    pub fn resident_pages(&self) -> usize {
        self.resident.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_oasis::write::{write_tree, WCell};

    #[test]
    fn splice_roundtrip() {
        let r = floe_oasis::doc::RectRec {
            layer: 1,
            dt: 0,
            x: 10,
            y: 20,
            w: 5,
            h: 5,
            rep: Rep::One,
        };
        let a = write_tree(
            &[WCell {
                name: "A".into(),
                rects: std::slice::from_ref(&r),
                polys: &[],
                paths: &[],
                texts: &[],
                places: Vec::new(),
            }],
            1000.0,
        )
        .unwrap();
        let b = write_tree(
            &[WCell {
                name: "B".into(),
                rects: std::slice::from_ref(&r),
                polys: &[],
                paths: &[],
                texts: &[],
                places: Vec::new(),
            }],
            1000.0,
        )
        .unwrap();
        let spliced = splice_tree(
            1000.0,
            &[tree_body(&a), tree_body(&b)],
        );
        let doc = floe_oasis::doc::parse_doc(&spliced);
        // two tops -> finish() rejects; parse must fail cleanly OR
        // we check at record level via scan
        assert!(doc.is_err());
        let st =
            floe_oasis::scan_parallel(&spliced, 1).unwrap();
        assert_eq!(st.cells, 2);
        let total: u64 =
            st.shapes.values().map(|s| s.records).sum();
        assert_eq!(total, 2);
    }
}
