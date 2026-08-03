//! V4 hierarchy-preserving planner (rust/VFS_HIER.md par.2).
//!
//! ONE topo-rank min-heap sweep over the .ovm v2 metadata propagates
//! clip regions (localview, up to K boxes) per WsKey = (cell,
//! remaining-depth) and emits a pruned COPY of the source hierarchy:
//! working-set cells holding page selections, child instance edges
//! (arrays preserved as arrays, nested arrays intact - zero
//! expansion), and frame rects (+rep) for below-cut children. The
//! flat plan/descend path stays alongside for A/B until M5.
//!
//! Correctness rule everywhere: over-inclusion is cost, omission is
//! a bug. Every conservative fallback (K-box merge, skew index bbox,
//! pts chunk-coarse, i128 saturation) only ever ADDS members.

use crate::{xf_bbox, ViewReq};
use floe_oasis::doc::Rep;
use floe_ovm::{bit_test, masks_intersect, BBox, Ovm, PBVH_NONE};
use floe_tiler::Xf;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};

/// remaining-depth sentinel: no depth truncation below this WC
pub const REM_FULL: u32 = u32::MAX;

/// (cell index, remaining depth) - the working-set cell identity.
/// Full-depth views collapse to one key per cell (r >= height folds
/// to REM_FULL, par.2.5), finite depth grows at most depth+1
/// variants per cell.
pub type WsKey = (u32, u32);

#[derive(Clone, Debug)]
pub struct HierOpts {
    /// localview boxes kept per WsKey before least-waste merging
    pub k_boxes: usize,
    /// pts reps at or below this emit a full (rebased) rep - above
    /// it the visible subset is selected by chunk scan (par.2.3)
    pub pts_full_rep: u32,
    /// per-request cap on per-offset visibility tests; exhausted
    /// scans degrade to whole-chunk inclusion (never omission)
    pub pts_enum_budget: u64,
    /// plan-wide cap on emitted frame rects (flat parity)
    pub frame_cap: usize,
}

impl Default for HierOpts {
    fn default() -> HierOpts {
        HierOpts {
            k_boxes: 4,
            pts_full_rep: 8192,
            pts_enum_budget: 200_000,
            frame_cap: 200_000,
        }
    }
}

/// one child edge of a working-set cell, in WC-local coordinates.
/// rep is emission-ready: One / full Grid / REBASED pts subset
/// (first offset (0,0) - the pool is Morton-ordered, so even a full
/// pts emission rebases onto its first slot).
#[derive(Clone, Debug, PartialEq)]
pub struct WsInst {
    pub child: WsKey,
    pub x: i64,
    pub y: i64,
    pub rot: u8,
    pub flip: bool,
    pub rep: Rep,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WsCell {
    pub key: WsKey,
    /// page directory indexes, sorted unique
    pub pages: Vec<u32>,
    pub insts: Vec<WsInst>,
    /// below-cut child outlines: offset-0 member bbox in WC-local
    /// coords + the placement rep (arrays outline per member)
    pub frames: Vec<(BBox, Rep)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HierStats {
    pub wc_cells: u64,
    pub wc_variants: u64,
    pub inst_edges: u64,
    pub frame_rects: u64,
    pub visited_bvh: u64,
    pub cull_layer: u64,
    pub cull_size: u64,
    pub cull_page_size: u64,
    pub culled_page_layer_roots: u64,
    pub culled_page_bvh_bbox: u64,
    pub culled_page_bvh_cut: u64,
    pub visited_page_bvh: u64,
    pub page_candidates: u64,
    pub pts_enumerated: u64,
    pub pts_fallback: u64,
    pub pts_offsets_scanned: u64,
    pub pts_selected: u64,
    pub pts_offsets_emitted: u64,
    pub pts_bytes_emitted: u64,
    pub grid_fallback_full: u64,
    pub kbox_merges: u64,
}

#[derive(Debug, PartialEq)]
pub struct HierPlan {
    pub top: WsKey,
    /// sorted by key
    pub wcells: Vec<WsCell>,
    /// union of wcell pages, sorted unique
    pub pages: Vec<u32>,
    /// streaming priority aligned with `pages`: squared distance
    /// from the page bbox to the nearest localview-box CENTER of a
    /// WC that selected it, in that WC's LOCAL frame (0 = the
    /// screen-center ray hits the page; min across shared uses).
    /// Page bboxes are cell-local, so only a local-frame metric
    /// orders "center first" correctly under placement/rotation.
    pub page_prio: Vec<u64>,
    pub stats: HierStats,
}

// ------------------------------------------------- K-box localview

fn area2(b: &BBox) -> i128 {
    if b.is_empty() {
        return 0;
    }
    (b.x1 - b.x0) as i128 * (b.y1 - b.y0) as i128
}

fn contains(a: &BBox, b: &BBox) -> bool {
    a.x0 <= b.x0 && a.y0 <= b.y0 && b.x1 <= a.x1 && b.y1 <= a.y1
}

/// K-box clip-region accumulator. Boxes are queried SEPARATELY (a
/// single union bbox would cover the void between two far-apart
/// appearances of a cell); over K the least-waste pair merges, ties
/// resolved by insertion order - same request, same plan, always.
#[derive(Clone, Debug, Default)]
struct KBox {
    boxes: Vec<BBox>,
}

impl KBox {
    fn add(&mut self, nb: BBox, k: usize, merges: &mut u64) {
        if nb.is_empty() {
            return;
        }
        for b in &self.boxes {
            if contains(b, &nb) {
                return;
            }
        }
        self.boxes.retain(|b| !contains(&nb, b));
        self.boxes.push(nb);
        while self.boxes.len() > k.max(1) {
            let (mut bi, mut bj, mut bw) = (0usize, 1usize, i128::MAX);
            for i in 0..self.boxes.len() {
                for j in (i + 1)..self.boxes.len() {
                    let mut u = self.boxes[i];
                    u.grow(&self.boxes[j]);
                    let w = area2(&u)
                        - area2(&self.boxes[i])
                        - area2(&self.boxes[j]);
                    if w < bw {
                        (bi, bj, bw) = (i, j, w);
                    }
                }
            }
            let b2 = self.boxes.remove(bj);
            self.boxes[bi].grow(&b2);
            *merges += 1;
        }
    }
}

// ------------------------------------------------ grid closed form

fn div_floor_i128(a: i128, b: i128) -> i128 {
    let q = a / b;
    let r = a % b;
    if r != 0 && ((r < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

fn div_ceil_i128(a: i128, b: i128) -> i128 {
    let q = a / b;
    let r = a % b;
    if r != 0 && ((r < 0) == (b < 0)) {
        q + 1
    } else {
        q
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridVis {
    Empty,
    /// inclusive index rectangle, clamped to [0,na) x [0,nb)
    Range { i0: i64, i1: i64, j0: i64, j1: i64 },
}

/// visible index rectangle of a grid rep against offset-region R
/// (member o = i*va + j*vb visible iff o in R). Exact for det != 0
/// (Cramer corners + floor/ceil) and for the writer's 1-D normal
/// forms; degenerate 2-D collapses to conservative full range -
/// over-inclusion is cost, omission is a bug (par.2.3). All math
/// i128: i64 products cannot overflow it.
pub fn grid_ranges(
    na: i64,
    nb: i64,
    va: (i64, i64),
    vb: (i64, i64),
    r: &BBox,
) -> GridVis {
    if r.is_empty() || na <= 0 || nb <= 0 {
        return GridVis::Empty;
    }
    let clamp = |i0: i128, i1: i128, j0: i128, j1: i128| -> GridVis {
        let i0 = i0.max(0);
        let i1 = i1.min(na as i128 - 1);
        let j0 = j0.max(0);
        let j1 = j1.min(nb as i128 - 1);
        if i0 > i1 || j0 > j1 {
            GridVis::Empty
        } else {
            GridVis::Range {
                i0: i0 as i64,
                i1: i1 as i64,
                j0: j0 as i64,
                j1: j1 as i64,
            }
        }
    };
    let det = va.0 as i128 * vb.1 as i128 - va.1 as i128 * vb.0 as i128;
    if det != 0 {
        let (mut i0, mut i1) = (i128::MAX, i128::MIN);
        let (mut j0, mut j1) = (i128::MAX, i128::MIN);
        for &(rx, ry) in &[
            (r.x0, r.y0),
            (r.x0, r.y1),
            (r.x1, r.y0),
            (r.x1, r.y1),
        ] {
            let ni =
                rx as i128 * vb.1 as i128 - ry as i128 * vb.0 as i128;
            let nj =
                va.0 as i128 * ry as i128 - va.1 as i128 * rx as i128;
            i0 = i0.min(div_floor_i128(ni, det));
            i1 = i1.max(div_ceil_i128(ni, det));
            j0 = j0.min(div_floor_i128(nj, det));
            j1 = j1.max(div_ceil_i128(nj, det));
        }
        return clamp(i0, i1, j0, j1);
    }
    // det == 0: the writer's 1-D normal forms are exact
    if nb == 1 {
        return match axis1d(na, va, r) {
            Some((a, b)) => clamp(a, b, 0, 0),
            None => GridVis::Empty,
        };
    }
    if na == 1 {
        return match axis1d(nb, vb, r) {
            Some((a, b)) => clamp(0, 0, a, b),
            None => GridVis::Empty,
        };
    }
    // collinear / zero-vector 2-D: conservative full range
    clamp(0, na as i128 - 1, 0, nb as i128 - 1)
}

/// t in [0,n) with t*v inside r on both axes (v may be negative or
/// zero per component)
fn axis1d(n: i64, v: (i64, i64), r: &BBox) -> Option<(i128, i128)> {
    let mut lo = 0i128;
    let mut hi = n as i128 - 1;
    for (vc, a, b) in [(v.0, r.x0, r.x1), (v.1, r.y0, r.y1)] {
        if vc == 0 {
            if !(a <= 0 && 0 <= b) {
                return None;
            }
        } else if vc > 0 {
            lo = lo.max(div_ceil_i128(a as i128, vc as i128));
            hi = hi.min(div_floor_i128(b as i128, vc as i128));
        } else {
            lo = lo.max(div_ceil_i128(b as i128, vc as i128));
            hi = hi.min(div_floor_i128(a as i128, vc as i128));
        }
        if lo > hi {
            return None;
        }
    }
    Some((lo, hi))
}

/// offset bbox of the 4 index-rectangle corners (i128, saturated to
/// i64 - saturation only widens, which is safe)
fn grid_ovis(
    i0: i64,
    i1: i64,
    j0: i64,
    j1: i64,
    va: (i64, i64),
    vb: (i64, i64),
) -> BBox {
    let sat = |v: i128| {
        v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    };
    let mut b = BBox::EMPTY;
    for &(i, j) in &[(i0, j0), (i0, j1), (i1, j0), (i1, j1)] {
        let ox = sat(i as i128 * va.0 as i128 + j as i128 * vb.0 as i128);
        let oy = sat(i as i128 * va.1 as i128 + j as i128 * vb.1 as i128);
        b.grow(&BBox { x0: ox, y0: oy, x1: ox, y1: oy });
    }
    b
}

/// b (+) (-o): the region of offsets/anchors whose translate of `o`
/// still meets `b` (Minkowski with the negated box; saturating -
/// widening is safe)
fn minkowski_neg(b: &BBox, o: &BBox) -> BBox {
    if b.is_empty() || o.is_empty() {
        return BBox::EMPTY;
    }
    BBox {
        x0: b.x0.saturating_sub(o.x1),
        y0: b.y0.saturating_sub(o.y1),
        x1: b.x1.saturating_sub(o.x0),
        y1: b.y1.saturating_sub(o.y0),
    }
}

// ------------------------------------------------------ the sweep

pub fn plan_hier(v: &Ovm, req: &ViewReq, opts: &HierOpts) -> HierPlan {
    let mut h = Hier {
        v,
        req,
        opts,
        cut: req.cut_dbu.max(0) as u64,
        lv: HashMap::new(),
        heap: BinaryHeap::new(),
        out: BTreeMap::new(),
        pages_all: BTreeSet::new(),
        prio: HashMap::new(),
        st: HierStats::default(),
        pts_budget: opts.pts_enum_budget,
        frames_total: 0,
    };
    let top_ci = v.top;
    let r0 = h.norm_r(
        top_ci,
        if req.depth == u32::MAX { REM_FULL } else { req.depth },
    );
    if v.n_cells > 0 {
        let tc = v.cell(top_ci);
        let w = (tc.rbbox.x1 - tc.rbbox.x0).max(0) as u64;
        let hh = (tc.rbbox.y1 - tc.rbbox.y0).max(0) as u64;
        if masks_intersect(v.bitset(tc.lmask_rec), &req.vis)
            && !tc.rbbox.is_empty()
            && !(w < h.cut && hh < h.cut)
        {
            let seed = req.view.intersect(&tc.rbbox);
            h.contribute((top_ci, r0), seed);
        }
    }
    while let Some(Reverse((_, ci, r))) = h.heap.pop() {
        h.expand(ci, r);
    }
    let mut st = h.st;
    st.wc_cells = h.out.len() as u64;
    st.wc_variants =
        h.out.keys().filter(|&&(_, r)| r != REM_FULL).count() as u64;
    let pages: Vec<u32> = h.pages_all.into_iter().collect();
    let page_prio: Vec<u64> = pages
        .iter()
        .map(|pi| {
            h.prio
                .get(pi)
                .map(|&d| d.min(u64::MAX as u128) as u64)
                .unwrap_or(u64::MAX)
        })
        .collect();
    HierPlan {
        top: (top_ci, r0),
        wcells: h.out.into_values().collect(),
        pages,
        page_prio,
        stats: st,
    }
}

/// squared distance from the center of `b` to the box `p`
/// (0 when the center lies inside)
fn dist2_center_to_box(b: &BBox, p: &BBox) -> u128 {
    let cx = (b.x0 as i128 + b.x1 as i128) / 2;
    let cy = (b.y0 as i128 + b.y1 as i128) / 2;
    let dx = (p.x0 as i128 - cx).max(cx - p.x1 as i128).max(0);
    let dy = (p.y0 as i128 - cy).max(cy - p.y1 as i128).max(0);
    (dx * dx + dy * dy) as u128
}

struct Hier<'a> {
    v: &'a Ovm,
    req: &'a ViewReq,
    opts: &'a HierOpts,
    cut: u64,
    lv: HashMap<WsKey, KBox>,
    /// (rank, ci, r) min-heap: every parent has a strictly smaller
    /// rank, so a key's localview is FINAL when it pops - one
    /// expansion per key, no fixpoint (par.2.3)
    heap: BinaryHeap<Reverse<(u32, u32, u32)>>,
    out: BTreeMap<WsKey, WsCell>,
    pages_all: BTreeSet<u32>,
    /// page -> min squared center distance (streaming priority)
    prio: HashMap<u32, u128>,
    st: HierStats,
    pts_budget: u64,
    frames_total: usize,
}

impl<'a> Hier<'a> {
    fn norm_r(&self, ci: u32, r: u32) -> u32 {
        if r == REM_FULL {
            return REM_FULL;
        }
        if self.v.n_cells == 0 {
            return r;
        }
        // r >= height: truncation is a no-op, fold to the sentinel
        // variant (par.2.5) - leaves always land on REM_FULL
        if r >= self.v.cell(ci).height {
            REM_FULL
        } else {
            r
        }
    }

    fn contribute(&mut self, key: WsKey, b: BBox) {
        if b.is_empty() {
            return;
        }
        let e = self.lv.entry(key).or_default();
        let first = e.boxes.is_empty();
        e.add(b, self.opts.k_boxes, &mut self.st.kbox_merges);
        if first {
            self.heap.push(Reverse((
                self.v.cell(key.0).topo_rank,
                key.0,
                key.1,
            )));
        }
    }

    fn expand(&mut self, ci: u32, r: u32) {
        let key = (ci, r);
        let boxes = self.lv.get(&key).expect("lv seeded").boxes.clone();
        let cell = self.v.cell(ci);
        let mut wc = WsCell {
            key,
            pages: Vec::new(),
            insts: Vec::new(),
            frames: Vec::new(),
        };
        // ---- own pages: (cell,layer) runs, layer roots skip whole,
        // per-box queries dedup into one sorted set
        let mut psel: BTreeSet<u32> = BTreeSet::new();
        for pri in cell.prange_start
            ..cell.prange_start + cell.prange_count
        {
            let pr = self.v.prange(pri);
            if !bit_test(&self.req.vis, pr.layer_idx as usize) {
                self.st.culled_page_layer_roots += 1;
                continue;
            }
            if pr.pbvh_root == PBVH_NONE {
                for pi in pr.page_lo..pr.page_lo + pr.page_count {
                    self.st.page_candidates += 1;
                    let p = self.v.page(pi);
                    if p.max_w < self.cut && p.max_h < self.cut {
                        self.st.cull_page_size += 1;
                        continue;
                    }
                    if boxes.iter().any(|b| p.bbox.intersects(b)) {
                        psel.insert(pi);
                    }
                }
            } else {
                for b in &boxes {
                    self.walk_pbvh(pr.pbvh_root, b, &mut psel);
                }
            }
        }
        for &pi in &psel {
            let pb = self.v.page(pi).bbox;
            let d = boxes
                .iter()
                .map(|b| dist2_center_to_box(b, &pb))
                .min()
                .unwrap_or(u128::MAX);
            self.prio
                .entry(pi)
                .and_modify(|e| *e = (*e).min(d))
                .or_insert(d);
        }
        self.pages_all.extend(psel.iter().copied());
        wc.pages = psel.into_iter().collect();
        // ---- children (r = 0 truncates: pages only, no frames -
        // flat parity, par.2.5)
        if r != 0 && cell.bvh_count != 0 {
            // Inline triage during the walk (field fix, 9.8G class:
            // 184M placements). Collecting every visible instance
            // into a set BEFORE cut classification made a wide view
            // O(visible instances) in MEMORY - tens of millions of
            // entries - before a single frame emitted. Below-cut
            // children now resolve inline with no-alloc accessors;
            // only ABOVE-cut edges (few, bounded by real content)
            // are gathered for the multi-box contribution pass.
            let cut = self.cut;
            let mut edges: BTreeSet<u64> = BTreeSet::new();
            let mut framed: HashSet<u64> = HashSet::new();
            for b in &boxes {
                let mut stack = vec![cell.bvh_start];
                while let Some(ni) = stack.pop() {
                    let node = self.v.bvh(ni);
                    self.st.visited_bvh += 1;
                    if !node.bbox.intersects(b) {
                        continue;
                    }
                    if !node.leaf {
                        for k in 0..node.count as u32 {
                            stack.push(node.first + k);
                        }
                        continue;
                    }
                    for k in 0..node.count as u64 {
                        let pli = node.first as u64 + k;
                        if edges.contains(&pli)
                            || framed.contains(&pli)
                        {
                            continue;
                        }
                        let h = self.v.place_head(pli);
                        let rb = self.v.cell_rbbox(h.child);
                        if rb.is_empty() {
                            framed.insert(pli);
                            continue;
                        }
                        let cw = (rb.x1 - rb.x0).max(0) as u64;
                        let chh = (rb.y1 - rb.y0).max(0) as u64;
                        if cw < cut && chh < cut {
                            // below-cut: frame inline, never enters
                            // a candidate set. Once the frame cap
                            // is hit we stop deduping too - the set
                            // must not grow with the visit count.
                            self.st.cull_size += 1;
                            if self.frames_total
                                < self.opts.frame_cap
                            {
                                framed.insert(pli);
                                self.frame_below_cut(
                                    &mut wc, pli, &h, &rb, &boxes,
                                );
                            }
                            continue;
                        }
                        if !masks_intersect(
                            self.v.bitset(
                                self.v.cell_lmask_rec(h.child),
                            ),
                            &self.req.vis,
                        ) {
                            self.st.cull_layer += 1;
                            framed.insert(pli);
                            continue;
                        }
                        edges.insert(pli);
                    }
                }
            }
            for pli in edges {
                self.place_edge(&mut wc, r, pli, &boxes);
            }
        }
        self.out.insert(key, wc);
    }

    fn walk_pbvh(
        &mut self,
        root: u32,
        b: &BBox,
        psel: &mut BTreeSet<u32>,
    ) {
        let mut stack = vec![root];
        while let Some(ni) = stack.pop() {
            let n = self.v.pbvh(ni);
            self.st.visited_page_bvh += 1;
            if n.max_w < self.cut && n.max_h < self.cut {
                self.st.culled_page_bvh_cut += 1;
                continue;
            }
            if !n.bbox.intersects(b) {
                self.st.culled_page_bvh_bbox += 1;
                continue;
            }
            if n.leaf {
                for pi in n.first..n.first + n.count as u32 {
                    self.st.page_candidates += 1;
                    let p = self.v.page(pi);
                    if p.max_w < self.cut && p.max_h < self.cut {
                        self.st.cull_page_size += 1;
                        continue;
                    }
                    if p.bbox.intersects(b) {
                        psel.insert(pi);
                    }
                }
            } else {
                for k in 0..n.count as u32 {
                    stack.push(n.first + k);
                }
            }
        }
    }

    /// below-cut child -> outline frame. Per-member outlines
    /// (rect+rep) are the accurate form, but below a ~2-cut member
    /// pitch they fuse into a solid wash that BURIES real geometry,
    /// and huge sparse pts lists would re-materialize O(count)
    /// offsets per plan - both degrade to the whole-rep footprint
    /// box (flat parity). Footprint visibility test via the rep
    /// extent (zero-copy for pts).
    fn frame_below_cut(
        &mut self,
        wc: &mut WsCell,
        pli: u64,
        h: &floe_ovm::PlaceHead,
        rb: &BBox,
        boxes: &[BBox],
    ) {
        let t0 = Xf::place(h.x, h.y, h.rot, h.flip);
        let b0 = xf_bbox(&t0, rb);
        let fuse_pitch = 2 * self.cut.max(1);
        let (rect, rep, fp) = match h.kind {
            0 => (b0, Rep::One, b0),
            1 => {
                let ov = grid_ovis(
                    0,
                    h.na as i64 - 1,
                    0,
                    h.nb as i64 - 1,
                    h.va,
                    h.vb,
                );
                let fp = grow_by_offsets(&b0, &ov);
                let pv = |v: (i64, i64)| {
                    v.0.unsigned_abs().max(v.1.unsigned_abs())
                };
                let mut pitch = u64::MAX;
                if h.na > 1 {
                    pitch = pitch.min(pv(h.va));
                }
                if h.nb > 1 {
                    pitch = pitch.min(pv(h.vb));
                }
                if pitch != u64::MAX && pitch < fuse_pitch {
                    (fp, Rep::One, fp)
                } else {
                    let rep = Rep::Grid {
                        na: h.na as u64,
                        nb: h.nb as u64,
                        va: h.va,
                        vb: h.vb,
                    };
                    (b0, rep, fp)
                }
            }
            _ => {
                let pr = self.v.pts_ref(pli).expect("pts kind");
                let ext = pr.extent();
                let fp = grow_by_offsets(&b0, &ext);
                let area = (ext.x1 - ext.x0).max(1) as u128
                    * (ext.y1 - ext.y0).max(1) as u128;
                let c2 = fuse_pitch as u128;
                if pr.count as u128 * c2 * c2 > area
                    || pr.count > self.opts.pts_full_rep
                {
                    (fp, Rep::One, fp)
                } else {
                    let mut pts =
                        Vec::with_capacity(pr.count as usize);
                    for s in 0..pr.count {
                        pts.push(pr.pt(s));
                    }
                    (b0, Rep::Pts(pts), fp)
                }
            }
        };
        if boxes.iter().any(|b| fp.intersects(b)) {
            wc.frames.push((rect, rep));
            self.frames_total += 1;
            self.st.frame_rects += 1;
        }
    }

    fn place_edge(
        &mut self,
        wc: &mut WsCell,
        r: u32,
        pli: u64,
        boxes: &[BBox],
    ) {
        let h = self.v.place_head(pli);
        let child = self.v.cell(h.child);
        if !masks_intersect(
            self.v.bitset(child.lmask_rec),
            &self.req.vis,
        ) {
            self.st.cull_layer += 1;
            return;
        }
        if child.rbbox.is_empty() {
            return;
        }
        let t0 = Xf::place(h.x, h.y, h.rot, h.flip);
        let b0 = xf_bbox(&t0, &child.rbbox);
        // cell-level cut (par.2.4): local dims, swap-invariant under
        // quarter turns, so above/below-cut is a property of the
        // CELL, never of the member. (The BVH walk triages most
        // below-cut placements inline before this fn is reached.)
        let cw = (child.rbbox.x1 - child.rbbox.x0).max(0) as u64;
        let ch = (child.rbbox.y1 - child.rbbox.y0).max(0) as u64;
        if cw < self.cut && ch < self.cut {
            self.st.cull_size += 1;
            if self.frames_total < self.opts.frame_cap {
                self.frame_below_cut(
                    wc, pli, &h, &child.rbbox, boxes,
                );
            }
            return;
        }
        let child_r = if r == REM_FULL {
            REM_FULL
        } else {
            self.norm_r(h.child, r - 1)
        };
        let ckey = (h.child, child_r);
        match h.kind {
            0 => {
                let mut vis = false;
                for b in boxes {
                    if !b0.intersects(b) {
                        continue;
                    }
                    let c = xf_bbox(&t0.invert(), b)
                        .intersect(&child.rbbox);
                    if !c.is_empty() {
                        vis = true;
                        self.contribute(ckey, c);
                    }
                }
                if vis {
                    wc.insts.push(WsInst {
                        child: ckey,
                        x: h.x,
                        y: h.y,
                        rot: h.rot,
                        flip: h.flip,
                        rep: Rep::One,
                    });
                    self.st.inst_edges += 1;
                }
            }
            1 => {
                let mut vis = false;
                for b in boxes {
                    let rbox = minkowski_neg(b, &b0);
                    let (i0, i1, j0, j1) = match grid_ranges(
                        h.na as i64,
                        h.nb as i64,
                        h.va,
                        h.vb,
                        &rbox,
                    ) {
                        GridVis::Empty => continue,
                        GridVis::Range { i0, i1, j0, j1 } => {
                            (i0, i1, j0, j1)
                        }
                    };
                    if h.na > 1
                        && h.nb > 1
                        && grid_degenerate(h.va, h.vb)
                    {
                        self.st.grid_fallback_full += 1;
                    }
                    // O_vis = bbox of the visible-index corners;
                    // localview(child) gains T0^-1(b (+) -O_vis)
                    // clipped to rbbox (par.2.3 step 3)
                    let ovis = grid_ovis(i0, i1, j0, j1, h.va, h.vb);
                    let shifted = minkowski_neg(b, &ovis);
                    let c = xf_bbox(&t0.invert(), &shifted)
                        .intersect(&child.rbbox);
                    if !c.is_empty() {
                        vis = true;
                        self.contribute(ckey, c);
                    }
                }
                if vis {
                    // ONE CellInstArray, full na x nb: off-view
                    // members are klayout's clip problem; nesting
                    // stays nested (zero expansion)
                    wc.insts.push(WsInst {
                        child: ckey,
                        x: h.x,
                        y: h.y,
                        rot: h.rot,
                        flip: h.flip,
                        rep: Rep::Grid {
                            na: h.na as u64,
                            nb: h.nb as u64,
                            va: h.va,
                            vb: h.vb,
                        },
                    });
                    self.st.inst_edges += 1;
                }
            }
            _ => {
                let pr = self.v.pts_ref(pli).expect("pts kind");
                let count = pr.count;
                let ext = pr.extent();
                self.st.pts_enumerated += 1;
                // one scan feeds BOTH the emission selection (slot
                // identity, par.2.3) and per-box O_vis - the ladder:
                // exact scan (small) / chunk-pruned scan / whole-
                // chunk inclusion on a dry budget / extent. Every
                // rung only ever over-includes.
                let mut sel: BTreeSet<u32> = BTreeSet::new();
                let mut contribs: Vec<BBox> = Vec::new();
                for b in boxes {
                    let rbox = minkowski_neg(b, &b0);
                    if !ext.intersects(&rbox) {
                        continue; // extent is the FIRST-PASS reject
                    }
                    let mut ovis = BBox::EMPTY;
                    if count <= self.opts.pts_full_rep {
                        // small rep: ALWAYS exact-scan, never against
                        // the budget - 8192xK point tests are
                        // microseconds, and the old budget-dry
                        // fallback (O_vis = extent) let ONE drained
                        // request balloon every later child localview
                        // to its whole cell: the view=cwb pathology
                        // reborn through pts (9.8G field case, depth
                        // 9 at a 0.1um view: 29k pages / 2.3G
                        // members). The enum budget bounds the BIG
                        // scans below.
                        self.st.pts_offsets_scanned += count as u64;
                        for s in 0..count {
                            let (ox, oy) = pr.pt(s);
                            if rbox.contains_pt(ox, oy) {
                                sel.insert(s);
                                ovis.grow(&pt_box(ox, oy));
                            }
                        }
                    } else {
                        for k in 0..pr.n_chunks {
                            let cb = pr.chunk_bbox(k);
                            if !cb.intersects(&rbox) {
                                continue;
                            }
                            let (lo, hi) = pr.chunk_range(k);
                            let n = (hi - lo) as u64;
                            if self.pts_budget >= n {
                                self.pts_budget -= n;
                                self.st.pts_offsets_scanned += n;
                                for s in lo..hi {
                                    let (ox, oy) = pr.pt(s);
                                    if rbox.contains_pt(ox, oy) {
                                        sel.insert(s);
                                        ovis.grow(&pt_box(ox, oy));
                                    }
                                }
                            } else {
                                // dry budget: include the WHOLE
                                // chunk (bounded over-inclusion,
                                // zero omission)
                                self.st.pts_fallback += 1;
                                for s in lo..hi {
                                    sel.insert(s);
                                }
                                ovis.grow(&cb);
                            }
                        }
                    }
                    if !ovis.is_empty() {
                        let shifted = minkowski_neg(b, &ovis);
                        let c = xf_bbox(&t0.invert(), &shifted)
                            .intersect(&child.rbbox);
                        if !c.is_empty() {
                            contribs.push(c);
                        }
                    }
                }
                if contribs.is_empty() {
                    return;
                }
                for c in contribs {
                    self.contribute(ckey, c);
                }
                // emission: small -> ALL slots (full rep); big ->
                // selected subset. Both REBASE (par.2.3): the pool
                // is Morton-ordered, so even "full" re-anchors on
                // its first slot.
                let emit: Vec<u32> = if count <= self.opts.pts_full_rep
                {
                    (0..count).collect()
                } else {
                    sel.into_iter().collect()
                };
                if emit.is_empty() {
                    return;
                }
                self.st.pts_selected += emit.len() as u64;
                let sub = |a: i64, b: i64| -> i64 {
                    i64::try_from(a as i128 - b as i128)
                        .expect("pts rebase overflow")
                };
                let (dx, dy, rep) = if emit.len() == 1 {
                    let (ox, oy) = pr.pt(emit[0]);
                    (ox, oy, Rep::One)
                } else {
                    let (ox0, oy0) = pr.pt(emit[0]);
                    let mut pts = Vec::with_capacity(emit.len());
                    pts.push((0i64, 0i64));
                    for &s in &emit[1..] {
                        let (ox, oy) = pr.pt(s);
                        pts.push((sub(ox, ox0), sub(oy, oy0)));
                    }
                    (ox0, oy0, Rep::Pts(pts))
                };
                self.st.pts_offsets_emitted += emit.len() as u64;
                self.st.pts_bytes_emitted += 16 * emit.len() as u64;
                wc.insts.push(WsInst {
                    child: ckey,
                    x: h.x + dx,
                    y: h.y + dy,
                    rot: h.rot,
                    flip: h.flip,
                    rep,
                });
                self.st.inst_edges += 1;
            }
        }
    }
}

fn pt_box(x: i64, y: i64) -> BBox {
    BBox { x0: x, y0: y, x1: x, y1: y }
}

fn grid_degenerate(va: (i64, i64), vb: (i64, i64)) -> bool {
    va.0 as i128 * vb.1 as i128 - va.1 as i128 * vb.0 as i128 == 0
}

/// widen a base box by an offset-extent box (member footprints)
fn grow_by_offsets(base: &BBox, off: &BBox) -> BBox {
    if base.is_empty() || off.is_empty() {
        return BBox::EMPTY;
    }
    BBox {
        x0: base.x0.saturating_add(off.x0),
        y0: base.y0.saturating_add(off.y0),
        x1: base.x1.saturating_add(off.x1),
        y1: base.y1.saturating_add(off.y1),
    }
}

/// cut-frame outline layer (drawn hollow by the viewer; shared with
/// the skeleton's cell-outline convention)
pub const FRAME_LAYER: u32 = 255;
pub const FRAME_DT: u32 = 0;

/// gen-ephemeral working-set cell name `W<gen>_<r>_<ci>`, r = "F"
/// for the full-depth sentinel (par.3.1): digits/fixed chars only -
/// design names travel once via the names= table, never in cell
/// names (whitespace/unicode hygiene).
pub fn ws_name(gen: u64, key: WsKey) -> String {
    if key.1 == REM_FULL {
        format!("W{}_F_{}", gen, key.0)
    } else {
        format!("W{}_{}_{}", gen, key.1, key.0)
    }
}

impl crate::Vfs {
    /// V4 hierarchy-preserving plan (default knobs)
    pub fn plan_hier(&self, req: &ViewReq) -> HierPlan {
        plan_hier(&self.ovm, req, &HierOpts::default())
    }

    /// hier delta (par.3.2): ONE OASIS = new pages spliced verbatim
    /// + authored working-set cells (identity page instances, child
    /// CellInstArray edges, frame rects on FRAME_LAYER). Resident
    /// pages are referenced by name with no definition in the file
    /// (M0-verified klayout name binding). The gen's top WC is the
    /// file's single top, so the delta stays parse_doc-able.
    pub fn delta_hier(
        &self,
        plan: &HierPlan,
        new_pages: &[u32],
        gen: u64,
        avail: Option<&std::collections::HashSet<u32>>,
    ) -> Result<Vec<u8>, String> {
        use floe_oasis::doc::RectRec;
        use floe_oasis::write::{
            splice_tree, tree_body, write_tree, WCell,
        };
        let payloads = self.read_page_payloads(new_pages)?;
        // owned storage first - WCell borrows names/rects/reps
        struct Pre {
            name: String,
            rects: Vec<RectRec>,
            places: Vec<(String, i64, i64, u8, bool, Rep)>,
        }
        let pre: Vec<Pre> = plan
            .wcells
            .iter()
            .map(|w| {
                let rects: Vec<RectRec> = w
                    .frames
                    .iter()
                    .map(|(b, rep)| RectRec {
                        layer: FRAME_LAYER,
                        dt: FRAME_DT,
                        x: b.x0,
                        y: b.y0,
                        w: (b.x1 - b.x0).max(1),
                        h: (b.y1 - b.y0).max(1),
                        rep: rep.clone(),
                    })
                    .collect();
                let mut places =
                    Vec::with_capacity(w.pages.len() + w.insts.len());
                for &pi in &w.pages {
                    // streaming (par.5 M3.5): a partial delta must
                    // only reference MATERIALIZED pages - a name
                    // with no definition anywhere would make
                    // klayout mint an empty ghost cell that nothing
                    // ever evicts (review finding)
                    if let Some(a) = avail {
                        if !a.contains(&pi) {
                            continue;
                        }
                    }
                    places.push((
                        self.page_name(pi),
                        0,
                        0,
                        0u8,
                        false,
                        Rep::One,
                    ));
                }
                for i in &w.insts {
                    places.push((
                        ws_name(gen, i.child),
                        i.x,
                        i.y,
                        i.rot,
                        i.flip,
                        i.rep.clone(),
                    ));
                }
                Pre { name: ws_name(gen, w.key), rects, places }
            })
            .collect();
        let wcells: Vec<WCell> = pre
            .iter()
            .map(|p| WCell {
                name: p.name.clone(),
                rects: &p.rects,
                polys: &[],
                paths: &[],
                texts: &[],
                places: p
                    .places
                    .iter()
                    .map(|(n, x, y, r, f, rep)| {
                        (n.as_str(), *x, *y, *r, *f, rep)
                    })
                    .collect(),
            })
            .collect();
        let mut bodies: Vec<&[u8]> =
            payloads.iter().map(|p| tree_body(p)).collect();
        let authored;
        if !wcells.is_empty() {
            authored = write_tree(&wcells, self.ovm.unit)
                .map_err(|e| e.to_string())?;
            bodies.push(tree_body(&authored));
        }
        Ok(splice_tree(self.ovm.unit, &bodies))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_ovm::Builder;
    use floe_tiler::hier::rep_extent;

    fn bx(x0: i64, y0: i64, x1: i64, y1: i64) -> BBox {
        BBox { x0, y0, x1, y1 }
    }

    fn rq(view: BBox, cut: i64, depth: u32) -> ViewReq {
        ViewReq { view, cut_dbu: cut, vis: vec![0xff], depth }
    }

    // ---- fixture: cells listed CHILDREN-FIRST (index order is a
    // topo order, so rank = n-1-ci gives parent < child), heights
    // and recursive bboxes computed here, one layer (L1/0), one
    // linear prange per paged cell, one-leaf instance BVH.
    struct FCell {
        name: &'static str,
        pages: Vec<(BBox, u64, u64)>,
        places: Vec<(usize, i64, i64, u8, bool, Rep)>,
    }

    fn place_bbox(
        rbb: &BBox,
        x: i64,
        y: i64,
        rot: u8,
        flip: bool,
        rep: &Rep,
    ) -> BBox {
        let xf = Xf::place(x, y, rot, flip);
        let base = xf_bbox(&xf, rbb);
        let (ex, ey) = rep_extent(rep);
        BBox {
            x0: base.x0 + ex.0.min(0),
            y0: base.y0 + ey.0.min(0),
            x1: base.x1 + ex.1.max(0),
            y1: base.y1 + ey.1.max(0),
        }
    }

    fn fixture(cells: &[FCell], top: usize) -> Ovm {
        let n = cells.len();
        let mut height = vec![0u32; n];
        let mut rbb = vec![BBox::EMPTY; n];
        for ci in 0..n {
            let mut b = BBox::EMPTY;
            for (pb, _, _) in &cells[ci].pages {
                b.grow(pb);
            }
            for (c, x, y, rot, flip, rep) in &cells[ci].places {
                assert!(*c < ci, "fixture must be children-first");
                height[ci] = height[ci].max(height[*c] + 1);
                b.grow(&place_bbox(&rbb[*c], *x, *y, *rot, *flip, rep));
            }
            rbb[ci] = b;
        }
        let mut b = Builder::new(1000.0, 0, 0, 1);
        b.top = top as u32;
        b.layer(1, 0, "L1", 0, 0);
        let m1 = b.bitset(&[1]);
        for ci in 0..n {
            assert!(cells[ci].places.len() <= 8, "one-leaf bvh cap");
            let place_base = b.n_places() as u32;
            let mut items = BBox::EMPTY;
            for (c, x, y, rot, flip, rep) in &cells[ci].places {
                b.place(*c as u32, *x, *y, *rot, *flip, rep);
                items.grow(&place_bbox(
                    &rbb[*c], *x, *y, *rot, *flip, rep,
                ));
            }
            let (bvh_start, bvh_count) =
                if cells[ci].places.is_empty() {
                    (0, 0)
                } else {
                    (
                        b.bvh_node(
                            &items,
                            place_base,
                            cells[ci].places.len() as u16,
                            true,
                        ),
                        1,
                    )
                };
            let page_start = b.n_pages();
            for (k, (pb, mw, mh)) in
                cells[ci].pages.iter().enumerate()
            {
                b.page(
                    ci as u32, 0, k as u32, pb, 0, 0, 0, 1, 1, *mw,
                    *mh,
                );
            }
            let page_count = b.n_pages() - page_start;
            let (pr_start, pr_count) = if page_count > 0 {
                (
                    b.prange(0, page_start, page_count, PBVH_NONE),
                    1u32,
                )
            } else {
                (b.n_pranges(), 0)
            };
            b.cell(
                cells[ci].name,
                height[ci],
                (n - 1 - ci) as u32,
                &rbb[ci],
                &rbb[ci],
                place_base,
                cells[ci].places.len() as u32,
                page_start,
                page_count,
                bvh_start,
                bvh_count,
                pr_start,
                pr_count,
                m1,
                m1,
                1,
            );
        }
        Ovm::from_bytes(b.finish(0)).unwrap()
    }

    // ---- brute reference: every rep member expanded, exact
    // per-path local view, same cut/layer/depth predicates - the
    // correctness oracle. hier must NEVER select fewer pages.
    fn brute(v: &Ovm, req: &ViewReq) -> BTreeSet<u32> {
        fn walk(
            v: &Ovm,
            ci: u32,
            xf: &Xf,
            r: u32,
            req: &ViewReq,
            cut: u64,
            out: &mut BTreeSet<u32>,
        ) {
            let cell = v.cell(ci);
            if !masks_intersect(v.bitset(cell.lmask_rec), &req.vis) {
                return;
            }
            if cell.rbbox.is_empty() {
                return;
            }
            let w = (cell.rbbox.x1 - cell.rbbox.x0).max(0) as u64;
            let h = (cell.rbbox.y1 - cell.rbbox.y0).max(0) as u64;
            if w < cut && h < cut {
                return;
            }
            let wb = xf_bbox(xf, &cell.rbbox);
            if !wb.intersects(&req.view) {
                return;
            }
            let inv = xf.invert();
            let lview = xf_bbox(&inv, &req.view);
            for pi in
                cell.page_start..cell.page_start + cell.page_count
            {
                let p = v.page(pi);
                if !bit_test(&req.vis, p.layer_idx as usize) {
                    continue;
                }
                if p.max_w < cut && p.max_h < cut {
                    continue;
                }
                if p.bbox.intersects(&lview) {
                    out.insert(pi);
                }
            }
            if r == 0 {
                return;
            }
            for pli in cell.place_start
                ..cell.place_start + cell.place_count
            {
                let pl = v.place(pli as u64);
                let offs: Vec<(i64, i64)> = match &pl.rep {
                    Rep::One => vec![(0, 0)],
                    Rep::Grid { na, nb, va, vb } => {
                        let mut o = Vec::new();
                        for i in 0..*na as i64 {
                            for j in 0..*nb as i64 {
                                o.push((
                                    i * va.0 + j * vb.0,
                                    i * va.1 + j * vb.1,
                                ));
                            }
                        }
                        o
                    }
                    Rep::Pts(p) => p.clone(),
                };
                let cr =
                    if r == REM_FULL { REM_FULL } else { r - 1 };
                for (ox, oy) in offs {
                    let m = xf.compose(&Xf::place(
                        pl.x + ox,
                        pl.y + oy,
                        pl.rot,
                        pl.flip,
                    ));
                    walk(v, pl.child, &m, cr, req, cut, out);
                }
            }
        }
        let mut out = BTreeSet::new();
        let r0 = if req.depth == u32::MAX {
            REM_FULL
        } else {
            req.depth
        };
        walk(
            v,
            v.top,
            &Xf::identity(),
            r0,
            req,
            req.cut_dbu.max(0) as u64,
            &mut out,
        );
        out
    }

    fn hier_pages(p: &HierPlan) -> BTreeSet<u32> {
        p.pages.iter().copied().collect()
    }

    fn check(v: &Ovm, req: &ViewReq, expect_equal: bool) -> HierPlan {
        let plan = plan_hier(v, req, &HierOpts::default());
        let b = brute(v, req);
        let h = hier_pages(&plan);
        assert!(
            h.is_superset(&b),
            "MISS: brute {:?} not within hier {:?}",
            b,
            h
        );
        if expect_equal {
            assert_eq!(h, b, "hier over-included");
        }
        plan
    }

    // -------------------------------------------------- pure grid

    #[test]
    fn grid_ranges_no_miss_and_near_tight() {
        let vas = [(7, 0), (0, 7), (7, 3), (-5, 2), (4, 4), (0, 0)];
        let vbs = [(0, 9), (9, 0), (2, -6), (8, 8), (0, 0)];
        let rs = [
            bx(-10, -10, 10, 10),
            bx(0, 0, 0, 0),
            bx(5, 3, 40, 29),
            bx(-40, -33, -5, -6),
            bx(13, -25, 57, -2),
        ];
        for &va in &vas {
            for &vb in &vbs {
                for &(na, nb) in
                    &[(1i64, 1i64), (2, 1), (1, 3), (4, 3), (5, 5)]
                {
                    for r in &rs {
                        let g = grid_ranges(na, nb, va, vb, r);
                        let mut exact = Vec::new();
                        for i in 0..na {
                            for j in 0..nb {
                                let ox = i * va.0 + j * vb.0;
                                let oy = i * va.1 + j * vb.1;
                                if r.contains_pt(ox, oy) {
                                    exact.push((i, j));
                                }
                            }
                        }
                        match g {
                            GridVis::Empty => assert!(
                                exact.is_empty(),
                                "MISS va{:?} vb{:?} n{}x{} r{:?}: {:?}",
                                va, vb, na, nb, r, exact
                            ),
                            GridVis::Range { i0, i1, j0, j1 } => {
                                for &(i, j) in &exact {
                                    assert!(
                                        i0 <= i && i <= i1
                                            && j0 <= j && j <= j1,
                                        "member ({},{}) outside \
                                         [{},{}]x[{},{}] va{:?} \
                                         vb{:?} r{:?}",
                                        i, j, i0, i1, j0, j1, va,
                                        vb, r
                                    );
                                }
                                // tightness claims only where the
                                // method is tight: axis-aligned
                                // invertible (+-1 from floor/ceil)
                                // and the 1-D interval forms; skew
                                // corner bboxes may be wider (still
                                // conservative, checked above)
                                let det = va.0 as i128
                                    * vb.1 as i128
                                    - va.1 as i128 * vb.0 as i128;
                                let axis = det != 0
                                    && va.1 == 0
                                    && vb.0 == 0;
                                let oned = det == 0
                                    && (na == 1 || nb == 1);
                                if !exact.is_empty() && (axis || oned)
                                {
                                    let ti0 = exact
                                        .iter()
                                        .map(|e| e.0)
                                        .min()
                                        .unwrap();
                                    let ti1 = exact
                                        .iter()
                                        .map(|e| e.0)
                                        .max()
                                        .unwrap();
                                    assert!(
                                        i0 >= ti0 - 1 && i1 <= ti1 + 1,
                                        "index slack va{:?} vb{:?}",
                                        va, vb
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ----------------------------------------------- plan fixtures

    fn nested_grid() -> Ovm {
        fixture(
            &[
                FCell {
                    name: "LEAF",
                    pages: vec![(bx(0, 0, 100, 100), 100, 100)],
                    places: vec![],
                },
                FCell {
                    name: "MID",
                    pages: vec![],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Grid {
                            na: 3,
                            nb: 3,
                            va: (200, 0),
                            vb: (0, 200),
                        },
                    )],
                },
                FCell {
                    name: "TOP",
                    pages: vec![(bx(0, 0, 2000, 2000), 50, 50)],
                    places: vec![(
                        1,
                        0,
                        0,
                        0,
                        false,
                        Rep::Grid {
                            na: 2,
                            nb: 2,
                            va: (1000, 0),
                            vb: (0, 1000),
                        },
                    )],
                },
            ],
            2,
        )
    }

    #[test]
    fn nested_grid_narrow_equality_and_shape() {
        let v = nested_grid();
        // inside member (1,1) of MID(0,0) - single leaf reached
        let plan =
            check(&v, &rq(bx(210, 210, 290, 290), 0, u32::MAX), true);
        // arrays stay arrays: ONE grid edge per level, no expansion
        let top = plan
            .wcells
            .iter()
            .find(|w| w.key == (2, REM_FULL))
            .unwrap();
        assert_eq!(top.insts.len(), 1);
        assert!(matches!(
            top.insts[0].rep,
            Rep::Grid { na: 2, nb: 2, .. }
        ));
        let mid = plan
            .wcells
            .iter()
            .find(|w| w.key == (1, REM_FULL))
            .unwrap();
        assert_eq!(mid.insts.len(), 1);
        assert!(matches!(
            mid.insts[0].rep,
            Rep::Grid { na: 3, nb: 3, .. }
        ));
        assert_eq!(plan.stats.inst_edges, 2);
        // member straddle
        check(&v, &rq(bx(150, 50, 450, 150), 0, u32::MAX), true);
        // whole die
        check(&v, &rq(bx(-10, -10, 2400, 2400), 0, u32::MAX), true);
        // page cut: TOP page (max 50) drops, leaf pages stay
        let plan =
            check(&v, &rq(bx(210, 210, 290, 290), 60, u32::MAX), true);
        let b = brute(&v, &rq(bx(210, 210, 290, 290), 60, u32::MAX));
        assert!(!b.iter().any(|&pi| v.page(pi).cell == 2));
        assert_eq!(hier_pages(&plan), b);
        // off-die view: empty plan
        let plan = plan_hier(
            &v,
            &rq(bx(90000, 90000, 90010, 90010), 0, u32::MAX),
            &HierOpts::default(),
        );
        assert!(plan.wcells.is_empty() && plan.pages.is_empty());
    }

    #[test]
    fn rot_flip_equality() {
        // asymmetric leaf under every quarter-turn/flip
        let mut places = Vec::new();
        for k in 0..8u8 {
            places.push((
                0usize,
                (k as i64) * 300,
                100,
                k % 4,
                k >= 4,
                Rep::One,
            ));
        }
        let v = fixture(
            &[
                FCell {
                    name: "L",
                    pages: vec![(bx(0, 0, 40, 10), 40, 10)],
                    places: vec![],
                },
                FCell { name: "T", pages: vec![], places },
            ],
            1,
        );
        for k in 0..8i64 {
            check(
                &v,
                &rq(
                    bx(k * 300 - 50, 40, k * 300 + 50, 70),
                    0,
                    u32::MAX,
                ),
                true,
            );
        }
        check(&v, &rq(bx(-100, -100, 2500, 300), 0, u32::MAX), true);
    }

    #[test]
    fn shared_child_kbox() {
        // one cell instanced far apart; leaf has 4 corner pages so a
        // merged single-box localview would over-select
        let leaf_pages = vec![
            (bx(0, 0, 50, 50), 50, 50),
            (bx(150, 0, 200, 50), 50, 50),
            (bx(0, 150, 50, 200), 50, 50),
            (bx(150, 150, 200, 200), 50, 50),
        ];
        let v = fixture(
            &[
                FCell { name: "L", pages: leaf_pages, places: vec![] },
                FCell {
                    name: "T",
                    pages: vec![],
                    places: vec![
                        (0, 0, 0, 0, false, Rep::One),
                        (0, 100_000, 0, 0, false, Rep::One),
                    ],
                },
            ],
            1,
        );
        // view covers corner page 0 of copy A and corner page 3 of
        // copy B: exact per-box queries keep it to two pages
        let view = bx(-10, -10, 100_180, 190);
        let req = ViewReq {
            view,
            cut_dbu: 0,
            vis: vec![0xff],
            depth: u32::MAX,
        };
        // brute equality needs the corner windows, not the whole
        // spanning box - use two-box behavior via narrow checks
        let plan4 = plan_hier(&v, &req, &HierOpts::default());
        let b = brute(&v, &req);
        assert!(hier_pages(&plan4).is_superset(&b));
        // K=1 forces a single merged box: still no miss, possibly
        // more pages
        let mut o1 = HierOpts::default();
        o1.k_boxes = 1;
        let plan1 = plan_hier(&v, &req, &o1);
        assert!(hier_pages(&plan1).is_superset(&hier_pages(&plan4)));
        // narrow window inside copy A corner 0 only: strict equality
        check(&v, &rq(bx(5, 5, 45, 45), 0, u32::MAX), true);
    }

    // ------------------------------------------------------- pts

    fn pts_small() -> (Ovm, Vec<(i64, i64)>) {
        let offs = vec![
            (0, 0),
            (5_000, 0),
            (10_000, 3_000),
            (20_000, 20_000),
            (20_000, 20_000), // duplicate member (legal multiset)
        ];
        let v = fixture(
            &[
                FCell {
                    name: "CHIP",
                    pages: vec![(bx(0, 0, 10, 10), 10, 10)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![],
                    places: vec![(
                        0,
                        100,
                        200,
                        0,
                        false,
                        Rep::Pts(offs.clone()),
                    )],
                },
            ],
            1,
        );
        (v, offs)
    }

    /// world member anchors implied by the emitted inst edges to
    /// `child` (multiset, top-level fixture: parent frame == world)
    fn emitted_anchors(plan: &HierPlan, child_ci: u32) -> Vec<(i64, i64)> {
        let mut out = Vec::new();
        for w in &plan.wcells {
            for i in &w.insts {
                if i.child.0 != child_ci {
                    continue;
                }
                match &i.rep {
                    Rep::One => out.push((i.x, i.y)),
                    Rep::Pts(p) => {
                        for (ox, oy) in p {
                            out.push((i.x + ox, i.y + oy));
                        }
                    }
                    Rep::Grid { na, nb, va, vb } => {
                        for a in 0..*na as i64 {
                            for c in 0..*nb as i64 {
                                out.push((
                                    i.x + a * va.0 + c * vb.0,
                                    i.y + a * va.1 + c * vb.1,
                                ));
                            }
                        }
                    }
                }
            }
        }
        out.sort_unstable();
        out
    }

    #[test]
    fn pts_small_full_rep_rebased() {
        let (v, offs) = pts_small();
        // window over the third member only
        let plan = check(
            &v,
            &rq(bx(10_090, 3_190, 10_120, 3_220), 0, u32::MAX),
            true,
        );
        // small path emits the FULL member set, rebased: anchors
        // must equal base + every original offset (duplicate kept)
        let mut want: Vec<(i64, i64)> = offs
            .iter()
            .map(|&(x, y)| (100 + x, 200 + y))
            .collect();
        want.sort_unstable();
        assert_eq!(emitted_anchors(&plan, 0), want);
        // rebased rep anchors at (0,0)
        let t = plan.wcells.iter().find(|w| w.key.0 == 1).unwrap();
        match &t.insts[0].rep {
            Rep::Pts(p) => assert_eq!(p[0], (0, 0)),
            r => panic!("expected pts, got {:?}", r),
        }
    }

    fn pts_big() -> (Ovm, Vec<(i64, i64)>) {
        // three far clusters, 3000 offsets each (> PTS_FULL_REP)
        let mut offs = Vec::new();
        for (cx, cy) in
            [(0i64, 0i64), (500_000, 0), (1_000_000, 500_000)]
        {
            for i in 0..3000i64 {
                offs.push((
                    cx + (i * 37) % 2000,
                    cy + (i * 91) % 2000,
                ));
            }
        }
        let v = fixture(
            &[
                FCell {
                    name: "CHIP",
                    pages: vec![(bx(0, 0, 10, 10), 10, 10)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Pts(offs.clone()),
                    )],
                },
            ],
            1,
        );
        (v, offs)
    }

    #[test]
    fn pts_small_ignores_budget_no_extent_cascade() {
        // MID is a big cell with a page FAR from the view; TOP
        // places MID via a small pts rep whose extent spans the
        // whole die. With a drained budget the old fallback set
        // O_vis = extent, ballooning MID's localview to its whole
        // rbbox and pulling the far page in - the view=cwb
        // pathology through pts (9.8G depth-9 field case). Small
        // reps now always exact-scan: the far page must stay out.
        let v = fixture(
            &[
                FCell {
                    name: "MID",
                    pages: vec![
                        (bx(0, 0, 100, 100), 100, 100),
                        (bx(900_000, 0, 900_100, 100), 100, 100),
                    ],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Pts(vec![
                            (0, 0),
                            (500_000, 0),
                            (1_000_000, 0),
                        ]),
                    )],
                },
            ],
            1,
        );
        let mut o = HierOpts::default();
        o.pts_enum_budget = 0; // drained from the start
        let req = rq(bx(-10, -10, 150, 150), 0, u32::MAX);
        let plan = plan_hier(&v, &req, &o);
        // only the near page of the offset-(0,0) member is visible
        assert_eq!(plan.pages, vec![0], "{:?}", plan.pages);
        assert!(hier_pages(&plan).is_superset(&brute(&v, &req)));
    }

    #[test]
    fn pts_big_subset_exact_and_budget_fallback() {
        let (v, offs) = pts_big();
        // window over cluster B only
        let view = bx(499_000, -1_000, 503_000, 3_000);
        let req = rq(view, 0, u32::MAX);
        let plan = check(&v, &req, true);
        assert!(plan.stats.pts_offsets_scanned > 0);
        assert_eq!(plan.stats.pts_fallback, 0);
        // emitted anchors == exactly the offsets whose chip bbox
        // (10x10 at offset) touches the window
        let mut want: Vec<(i64, i64)> = offs
            .iter()
            .copied()
            .filter(|&(x, y)| {
                x + 10 >= view.x0
                    && x <= view.x1
                    && y + 10 >= view.y0
                    && y <= view.y1
            })
            .collect();
        want.sort_unstable();
        assert_eq!(emitted_anchors(&plan, 0), want);
        // dry budget: whole-chunk fallback may over-include but
        // must never miss
        let mut o = HierOpts::default();
        o.pts_enum_budget = 64;
        let plan2 = plan_hier(&v, &req, &o);
        assert!(plan2.stats.pts_fallback > 0);
        let got = emitted_anchors(&plan2, 0);
        let gs: BTreeSet<(i64, i64)> = got.iter().copied().collect();
        for w in &want {
            assert!(gs.contains(w), "budget fallback dropped {:?}", w);
        }
        assert!(hier_pages(&plan2).is_superset(&brute(&v, &req)));
    }

    // ------------------------------------------------ depth/frames

    #[test]
    fn depth_variants_per_path() {
        // TOP places A (which places B) and B directly at another
        // spot; finite depth truncates PER PATH (par.2.5)
        let v = fixture(
            &[
                FCell {
                    name: "LEAF",
                    pages: vec![(bx(0, 0, 20, 20), 20, 20)],
                    places: vec![],
                },
                FCell {
                    name: "B",
                    pages: vec![(bx(0, 0, 200, 200), 200, 200)],
                    places: vec![(0, 50, 50, 0, false, Rep::One)],
                },
                FCell {
                    name: "A",
                    pages: vec![],
                    places: vec![(1, 0, 0, 0, false, Rep::One)],
                },
                FCell {
                    name: "TOP",
                    pages: vec![],
                    places: vec![
                        (2, 0, 0, 0, false, Rep::One),
                        (1, 10_000, 0, 0, false, Rep::One),
                    ],
                },
            ],
            3,
        );
        let view = bx(-100, -100, 11_000, 400);
        for d in [1u32, 2, 3, u32::MAX] {
            check(&v, &rq(view, 0, d), true);
        }
        // d=2: B via A has r=0 (pages only), direct B has r=1 which
        // folds to FULL (height(B)=1) - both variants must exist
        let plan =
            plan_hier(&v, &rq(view, 0, 2), &HierOpts::default());
        let keys: Vec<WsKey> =
            plan.wcells.iter().map(|w| w.key).collect();
        assert!(keys.contains(&(1, 0)), "{:?}", keys);
        assert!(keys.contains(&(1, REM_FULL)), "{:?}", keys);
        assert!(plan.stats.wc_variants >= 1);
        // the truncated variant must NOT reach LEAF; the full one
        // must
        let b0 = plan
            .wcells
            .iter()
            .find(|w| w.key == (1, 0))
            .unwrap();
        assert!(b0.insts.is_empty());
        let bf = plan
            .wcells
            .iter()
            .find(|w| w.key == (1, REM_FULL))
            .unwrap();
        assert_eq!(bf.insts.len(), 1);
    }

    #[test]
    fn below_cut_frames_keep_rep() {
        let v = fixture(
            &[
                FCell {
                    name: "S",
                    pages: vec![(bx(0, 0, 5, 5), 5, 5)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![(bx(0, 0, 4000, 100), 4000, 100)],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Grid {
                            na: 4,
                            nb: 1,
                            va: (1000, 0),
                            vb: (0, 0),
                        },
                    )],
                },
            ],
            1,
        );
        let plan = check(&v, &rq(bx(0, 0, 4000, 100), 10, u32::MAX), true);
        let t = plan.wcells.iter().find(|w| w.key.0 == 1).unwrap();
        assert_eq!(t.frames.len(), 1);
        let (fb, frep) = &t.frames[0];
        assert_eq!(*fb, bx(0, 0, 5, 5));
        assert!(matches!(frep, Rep::Grid { na: 4, .. }));
        assert!(t.insts.is_empty());
        // no S working-set cell, no S pages
        assert!(!plan.wcells.iter().any(|w| w.key.0 == 0));
        assert!(plan.pages.iter().all(|&pi| v.page(pi).cell != 0));
        // window over member 3 of the array only: frame still comes
        // (footprint test is per whole-rep extent)
        let plan2 = plan_hier(
            &v,
            &rq(bx(2_900, 0, 3_100, 90), 10, u32::MAX),
            &HierOpts::default(),
        );
        let t2 =
            plan2.wcells.iter().find(|w| w.key.0 == 1).unwrap();
        assert_eq!(t2.frames.len(), 1);
        let plan3 = plan_hier(
            &v,
            &rq(bx(4_500, 0, 4_800, 90), 10, u32::MAX),
            &HierOpts::default(),
        );
        if let Some(t3) =
            plan3.wcells.iter().find(|w| w.key.0 == 1)
        {
            assert_eq!(t3.frames.len(), 0);
        }
    }

    #[test]
    fn below_cut_dense_frames_fuse_to_footprint() {
        // sub-pixel pitch (15 < 2*cut=20): per-member outlines
        // would fuse into a solid wash burying real geometry -
        // the frame degrades to ONE footprint box (flat parity)
        let v = fixture(
            &[
                FCell {
                    name: "S",
                    pages: vec![(bx(0, 0, 5, 5), 5, 5)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![(bx(0, 0, 4000, 100), 4000, 100)],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Grid {
                            na: 200,
                            nb: 1,
                            va: (15, 0),
                            vb: (0, 0),
                        },
                    )],
                },
            ],
            1,
        );
        let plan = check(
            &v,
            &rq(bx(0, 0, 4000, 100), 10, u32::MAX),
            true,
        );
        let t = plan.wcells.iter().find(|w| w.key.0 == 1).unwrap();
        assert_eq!(t.frames.len(), 1);
        let (fb, frep) = &t.frames[0];
        assert!(matches!(frep, Rep::One), "{:?}", frep);
        // footprint spans the whole array, not one member
        assert_eq!(*fb, bx(0, 0, 15 * 199 + 5, 5));
    }

    #[test]
    fn below_cut_huge_sparse_pts_frame_fuses() {
        // sparse (members far apart) but HUGE: per-member frame
        // would re-materialize O(count) offsets every plan - the
        // count cap degrades it to one footprint box
        let n = 9000usize;
        let offs: Vec<(i64, i64)> = (0..n as i64)
            .map(|i| (i % 100 * 1_000_000, i / 100 * 1_000_000))
            .collect();
        let v = fixture(
            &[
                FCell {
                    name: "S",
                    pages: vec![(bx(0, 0, 5, 5), 5, 5)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![(
                        bx(0, 0, 99_000_010, 100),
                        99_000_010,
                        100,
                    )],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Pts(offs),
                    )],
                },
            ],
            1,
        );
        let plan = plan_hier(
            &v,
            &rq(bx(0, 0, 99_000_010, 200), 10, u32::MAX),
            &HierOpts::default(),
        );
        let t = plan.wcells.iter().find(|w| w.key.0 == 1).unwrap();
        assert_eq!(t.frames.len(), 1);
        assert!(matches!(t.frames[0].1, Rep::One));
        // spacing 1mm >> 2*cut, so ONLY the count cap explains the
        // fuse (9000 > pts_full_rep 8192)
        assert!(9000 > HierOpts::default().pts_full_rep);
    }

    #[test]
    fn pbvh_equals_linear() {
        // 20 pages in a row; one ovm linear, one with a hand-built
        // 3-node page BVH - identical selections, tree stats move
        let mk = |with_tree: bool| -> Ovm {
            let mut b = Builder::new(1000.0, 0, 0, 1);
            b.top = 0;
            b.layer(1, 0, "L1", 0, 0);
            let m1 = b.bitset(&[1]);
            for k in 0..20u32 {
                let x = k as i64 * 100;
                b.page(
                    0,
                    0,
                    k,
                    &bx(x, 0, x + 90, 50),
                    0,
                    0,
                    0,
                    1,
                    1,
                    90,
                    50,
                );
            }
            let root = if with_tree {
                let l0 = b.pbvh_node(
                    &bx(0, 0, 990, 50),
                    0,
                    10,
                    true,
                    90,
                    50,
                );
                let _l1 = b.pbvh_node(
                    &bx(1000, 0, 1990, 50),
                    10,
                    10,
                    true,
                    90,
                    50,
                );
                b.pbvh_node(&bx(0, 0, 1990, 50), l0, 2, false, 90, 50)
            } else {
                PBVH_NONE
            };
            let pr = b.prange(0, 0, 20, root);
            b.cell(
                "T",
                0,
                0,
                &bx(0, 0, 1990, 50),
                &bx(0, 0, 1990, 50),
                0,
                0,
                0,
                20,
                0,
                0,
                pr,
                1,
                m1,
                m1,
                1,
            );
            Ovm::from_bytes(b.finish(0)).unwrap()
        };
        let lin = mk(false);
        let tree = mk(true);
        for view in [
            bx(120, 0, 380, 50),
            bx(0, 0, 1990, 50),
            bx(1500, 10, 1620, 40),
        ] {
            let req = rq(view, 0, u32::MAX);
            let a = plan_hier(&lin, &req, &HierOpts::default());
            let c = plan_hier(&tree, &req, &HierOpts::default());
            assert_eq!(a.pages, c.pages);
            assert!(c.stats.visited_page_bvh > 0);
        }
        // narrow view culls the far subtree by bbox
        let req = rq(bx(120, 0, 380, 50), 0, u32::MAX);
        let c = plan_hier(&tree, &req, &HierOpts::default());
        assert!(c.stats.culled_page_bvh_bbox > 0);
        // page-level cut prunes whole subtrees pre-leaf
        let req = rq(bx(0, 0, 1990, 50), 200, u32::MAX);
        let c = plan_hier(&tree, &req, &HierOpts::default());
        assert!(c.stats.culled_page_bvh_cut > 0);
        assert!(c.pages.is_empty());
    }

    // ---------------------------------------------- delta (par.3.2)

    #[test]
    fn page_prio_is_local_frame() {
        // CHILD sits at (100k,100k): its page bbox is CELL-LOCAL
        // (0..100), so a world-center metric would call it "far"
        // even when it fills the screen center (review finding).
        // The planner must price it through the WC's LOCAL view.
        let v = fixture(
            &[
                FCell {
                    name: "C",
                    pages: vec![(bx(0, 0, 100, 100), 100, 100)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![(bx(0, 0, 80, 80), 80, 80)],
                    places: vec![(
                        0,
                        100_000,
                        100_000,
                        0,
                        false,
                        Rep::One,
                    )],
                },
            ],
            1,
        );
        // wide view covering both pages; the child's LOCAL view box
        // covers its whole page (center inside -> prio 0) while the
        // top's own page sits ~50k off the top-frame view center
        let req =
            rq(bx(-10, -10, 100_140, 100_140), 0, u32::MAX);
        let plan = plan_hier(&v, &req, &HierOpts::default());
        assert_eq!(plan.pages, vec![0, 1]);
        // child page: the local view box covers it -> center inside
        // -> prio 0; top's own page is far from the view center
        assert_eq!(plan.page_prio[0], 0, "{:?}", plan.page_prio);
        assert!(plan.page_prio[1] > 0, "{:?}", plan.page_prio);
        // narrow view exactly on the child: still prio 0
        let req2 =
            rq(bx(99_990, 99_990, 100_110, 100_110), 0, u32::MAX);
        let plan2 = plan_hier(&v, &req2, &HierOpts::default());
        assert_eq!(plan2.pages, vec![0]);
        assert_eq!(plan2.page_prio, vec![0]);
    }

    #[test]
    fn delta_hier_roundtrip_full() {
        use floe_oasis::doc::{parse_doc, RectRec};
        use floe_oasis::write::{write_tree, WCell};
        let mk_page = |name: &str, w: i64| {
            let r = RectRec {
                layer: 1,
                dt: 0,
                x: 0,
                y: 0,
                w,
                h: 20,
                rep: Rep::One,
            };
            write_tree(
                &[WCell {
                    name: name.into(),
                    rects: std::slice::from_ref(&r),
                    polys: &[],
                    paths: &[],
                    texts: &[],
                    places: vec![],
                }],
                1000.0,
            )
            .unwrap()
        };
        let p0 = mk_page("P0_0_0", 100);
        let p1 = mk_page("P1_0_0", 500);
        let mut ovp = p0.clone();
        ovp.extend_from_slice(&p1);
        let dir = std::env::temp_dir()
            .join(format!("floe_m2_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ovp_path = dir.join("design.ovp");
        std::fs::write(&ovp_path, &ovp).unwrap();
        // matching metadata: LEAF(page P0_0_0) under TOP(page
        // P1_0_0) via a 2x1 grid at (0,100)
        let mut b = Builder::new(1000.0, 0, 0, 1);
        b.top = 1;
        b.layer(1, 0, "L1", 0, 0);
        let m = b.bitset(&[1]);
        let leafbb = bx(0, 0, 100, 20);
        b.page(
            0,
            0,
            0,
            &leafbb,
            0,
            p0.len() as u64,
            p0.len() as u64,
            1,
            1,
            100,
            20,
        );
        b.page(
            1,
            0,
            0,
            &bx(0, 0, 500, 20),
            p0.len() as u64,
            p1.len() as u64,
            p1.len() as u64,
            1,
            1,
            500,
            20,
        );
        let pl = b.place(
            0,
            0,
            100,
            0,
            false,
            &Rep::Grid { na: 2, nb: 1, va: (200, 0), vb: (0, 0) },
        );
        let n0 =
            b.bvh_node(&bx(0, 100, 300, 120), pl as u32, 1, true);
        let pr0 = b.prange(0, 0, 1, PBVH_NONE);
        b.cell(
            "LEAF", 0, 1, &leafbb, &leafbb, 0, 0, 0, 1, 0, 0, pr0,
            1, m, m, 1,
        );
        let pr1 = b.prange(0, 1, 1, PBVH_NONE);
        b.cell(
            "TOPC",
            1,
            0,
            &bx(0, 0, 500, 120),
            &bx(0, 0, 500, 120),
            0,
            1,
            1,
            1,
            n0,
            1,
            pr1,
            1,
            m,
            m,
            1,
        );
        let ovm = Ovm::from_bytes(b.finish(ovp.len() as u64)).unwrap();
        let vfs = crate::Vfs {
            ovm,
            ovp_path: ovp_path.to_string_lossy().into_owned(),
        };
        let req = rq(bx(-10, -10, 600, 200), 0, u32::MAX);
        let plan =
            plan_hier(&vfs.ovm, &req, &HierOpts::default());
        assert_eq!(plan.pages, vec![0, 1]);
        let delta =
            vfs.delta_hier(&plan, &plan.pages, 3, None).unwrap();
        // deterministic emission
        assert_eq!(
            delta,
            vfs.delta_hier(&plan, &plan.pages, 3, None).unwrap()
        );
        let doc = parse_doc(&delta).unwrap();
        // single-top OASIS: the gen's top WC
        assert_eq!(doc.cells[doc.top].name, "W3_F_1");
        let mut names: Vec<&str> =
            doc.cells.iter().map(|c| c.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["P0_0_0", "P1_0_0", "W3_F_0", "W3_F_1"]
        );
        let topc = &doc.cells[doc.top];
        assert_eq!(topc.places.len(), 2);
        // identity page instance first, then the child WC edge with
        // the array preserved
        assert_eq!(doc.cells[topc.places[0].cell].name, "P1_0_0");
        assert!(matches!(topc.places[0].rep, Rep::One));
        let wleaf = &doc.cells[topc.places[1].cell];
        assert_eq!(wleaf.name, "W3_F_0");
        assert!(matches!(
            topc.places[1].rep,
            Rep::Grid { na: 2, nb: 1, .. }
        ));
        assert_eq!((topc.places[1].x, topc.places[1].y), (0, 100));
        assert_eq!(wleaf.places.len(), 1);
        assert_eq!(
            doc.cells[wleaf.places[0].cell].name,
            "P0_0_0"
        );
        // incremental delta: no page bodies, resident refs stay
        // undefined (parse_doc materializes them as empty cells,
        // klayout binds them by name - M0)
        let inc = vfs.delta_hier(&plan, &[], 4, None).unwrap();
        let doc2 = parse_doc(&inc).unwrap();
        assert_eq!(doc2.cells[doc2.top].name, "W4_F_1");
        let ghost = doc2
            .cells
            .iter()
            .find(|c| c.name == "P0_0_0")
            .unwrap();
        assert!(ghost.rects.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delta_hier_frames_and_pts_authored() {
        use floe_oasis::doc::parse_doc;
        // frames: below-cut grid child -> WC rect on 255/0 with the
        // rep preserved (authored-only delta, new_pages = [])
        let v = fixture(
            &[
                FCell {
                    name: "S",
                    pages: vec![(bx(0, 0, 5, 5), 5, 5)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![(bx(0, 0, 4000, 100), 4000, 100)],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Grid {
                            na: 4,
                            nb: 1,
                            va: (1000, 0),
                            vb: (0, 0),
                        },
                    )],
                },
            ],
            1,
        );
        let plan = plan_hier(
            &v,
            &rq(bx(0, 0, 4000, 100), 10, u32::MAX),
            &HierOpts::default(),
        );
        let vfs = crate::Vfs { ovm: v, ovp_path: String::new() };
        let delta = vfs.delta_hier(&plan, &[], 9, None).unwrap();
        let doc = parse_doc(&delta).unwrap();
        let top = &doc.cells[doc.top];
        assert_eq!(top.name, "W9_F_1");
        assert_eq!(top.rects.len(), 1);
        let fr = &top.rects[0];
        assert_eq!((fr.layer, fr.dt), (FRAME_LAYER, FRAME_DT));
        assert!(matches!(fr.rep, Rep::Grid { na: 4, .. }));
        // referenced page is a ghost (undefined) cell here
        assert!(doc
            .cells
            .iter()
            .any(|c| c.name == "P1_0_0" && c.rects.is_empty()));
        // pts: rebased subset survives the writer/parser round trip
        let (v2, offs) = pts_small();
        let plan2 = plan_hier(
            &v2,
            &rq(bx(10_090, 3_190, 10_120, 3_220), 0, u32::MAX),
            &HierOpts::default(),
        );
        let vfs2 = crate::Vfs { ovm: v2, ovp_path: String::new() };
        let delta2 = vfs2.delta_hier(&plan2, &[], 11, None).unwrap();
        let doc2 = parse_doc(&delta2).unwrap();
        let t2 = &doc2.cells[doc2.top];
        let pl = t2
            .places
            .iter()
            .find(|p| doc2.cells[p.cell].name.starts_with("W11_"))
            .unwrap();
        let mut got: Vec<(i64, i64)> = match &pl.rep {
            Rep::Pts(p) => {
                assert_eq!(p[0], (0, 0), "rebased anchor");
                p.iter().map(|&(x, y)| (pl.x + x, pl.y + y)).collect()
            }
            r => panic!("expected pts, got {:?}", r),
        };
        got.sort_unstable();
        let mut want: Vec<(i64, i64)> = offs
            .iter()
            .map(|&(x, y)| (100 + x, 200 + y))
            .collect();
        want.sort_unstable();
        assert_eq!(got, want);
    }

    #[test]
    fn deterministic_plans() {
        let (v, _) = pts_big();
        let req = rq(bx(-1_000, -1_000, 1_100_000, 600_000), 0, u32::MAX);
        let a = plan_hier(&v, &req, &HierOpts::default());
        let b = plan_hier(&v, &req, &HierOpts::default());
        assert_eq!(a, b);
        // K overflow: 6 spread copies of one leaf force merges
        let mut places = Vec::new();
        for k in 0..6i64 {
            places.push((
                0usize,
                k * 40_000,
                (k % 2) * 25_000,
                0u8,
                false,
                Rep::One,
            ));
        }
        let v2 = fixture(
            &[
                FCell {
                    name: "L",
                    pages: vec![(bx(0, 0, 100, 100), 100, 100)],
                    places: vec![],
                },
                FCell { name: "T", pages: vec![], places },
            ],
            1,
        );
        let req2 =
            rq(bx(-500, -500, 250_000, 30_000), 0, u32::MAX);
        let p1 = plan_hier(&v2, &req2, &HierOpts::default());
        let p2 = plan_hier(&v2, &req2, &HierOpts::default());
        assert_eq!(p1, p2);
        assert!(hier_pages(&p1).is_superset(&brute(&v2, &req2)));
        // direct K-merge determinism: 6 disjoint boxes through K=4
        // (identical planner contributions dedup by containment, so
        // the merge path is exercised at the accumulator level)
        let boxes = [
            bx(0, 0, 10, 10),
            bx(1000, 0, 1010, 10),
            bx(0, 1000, 10, 1010),
            bx(1000, 1000, 1010, 1010),
            bx(500, 500, 510, 510),
            bx(2000, 2000, 2010, 2010),
        ];
        let (mut m1, mut m2) = (0u64, 0u64);
        let mut k1 = KBox::default();
        let mut k2 = KBox::default();
        for b in boxes {
            k1.add(b, 4, &mut m1);
        }
        for b in boxes {
            k2.add(b, 4, &mut m2);
        }
        assert!(m1 > 0);
        assert_eq!(m1, m2);
        assert_eq!(k1.boxes, k2.boxes);
        assert!(k1.boxes.len() <= 4);
        // no box lost: every input is inside some kept box
        for b in boxes {
            assert!(k1.boxes.iter().any(|k| contains(k, &b)));
        }
    }
}
