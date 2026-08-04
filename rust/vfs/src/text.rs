//! v5 label planner (VFS_TEXT_PLAN.md par.4): request-scoped
//! display labels from the cell-local text index.
//!
//! One deterministic hierarchy walk from the top, mirroring the
//! geometry planner's visit rules (visible layers, semantic depth,
//! cell-level cut) but tracking the full placement transform so
//! every candidate lands in top coordinates. Pruning: subtrees
//! whose RECURSIVE text-layer mask misses the visible set never
//! descend; a per-request member budget bounds array enumeration
//! (exhaustion sets `truncated`, never panics - labels are display
//! only). Candidates are exact BEFORE declutter: every text member
//! whose anchor lies in the viewport, each (path, member) once -
//! the oracle gates compare this set against klayout.
//!
//! Declutter is screen-space: one winner per world-anchored
//! cell_px bin (anchored to world DBU, so labels don't reshuffle
//! on pan), priority block > text, then layer order, then a stable
//! identity hash; the winner list is budget-capped. Block-name
//! candidates are synthesized at the depth boundary (r == 0) from
//! the child's cell name + placed bbox center - nothing is stored
//! (par.4.3).

use crate::hier::{grid_ranges, GridVis, REM_FULL};
use crate::{xf_bbox, ViewReq};
use floe_ovm::{
    bit_test, masks_intersect, BBox, Ovm, TrepV, PTS_CHUNK,
    TBVH_NONE, TREP_NONE,
};
use floe_tiler::Xf;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct LabelOpts {
    /// declutter bin size in screen pixels (one label per bin)
    pub cell_px: f64,
    /// max selected labels per request
    pub view_budget: usize,
    /// placement-member enumeration budget (arrays); exhaustion
    /// stops descending and sets stats.truncated
    pub member_budget: u64,
    /// cap on retained candidates/bins (memory guard; truncated)
    pub cand_cap: usize,
    /// min on-screen size (px, either axis) for a block label
    pub block_min_px: f64,
    /// gate mode: exact candidate dump - no bins, no size gate,
    /// no view budget (oracle XOR comparisons)
    pub raw: bool,
}

impl Default for LabelOpts {
    fn default() -> LabelOpts {
        LabelOpts {
            cell_px: 48.0,
            view_budget: 400,
            member_budget: 200_000,
            cand_cap: 65_536,
            block_min_px: 96.0,
            raw: false,
        }
    }
}

/// one selected label, display-ready. Text rows carry their design
/// (layer, dt); block rows are layer-free runtime annotations (the
/// viewer maps them onto its frame layer).
#[derive(Clone, Debug, PartialEq)]
pub struct LabelRow {
    pub block: bool,
    pub layer: u32,
    pub dt: u32,
    pub x: i64,
    pub y: i64,
    pub s: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextStats {
    pub tbvh_nodes: u64,
    pub records_candidate: u64,
    pub rep_chunks_scanned: u64,
    pub members_tested: u64,
    pub members_visible: u64,
    pub blocks_visible: u64,
    pub place_members_enumerated: u64,
    pub selected: u64,
    pub budget_dropped: u64,
    /// a budget/cap stopped the walk early - the label set is a
    /// deterministic prefix, not the complete candidate set
    pub truncated: bool,
}

#[derive(Debug)]
pub struct LabelPlan {
    pub rows: Vec<LabelRow>,
    pub stats: TextStats,
}

#[derive(Clone, Copy, Debug)]
enum Src {
    /// design.ovt string span
    Ovt(u64, u32),
    /// block: cell index (name resolved at output)
    Cell(u32),
}

#[derive(Clone, Copy, Debug)]
struct Cand {
    block: bool,
    /// text: ovm layer index (= display priority order); block: 0
    layer_pos: u32,
    hash: u64,
    x: i64,
    y: i64,
    src: Src,
}

impl Cand {
    /// total order: blocks first, then layer order, then a stable
    /// identity hash (spread), coordinates + source as final ties
    fn prio(&self) -> (u8, u32, u64, i64, i64, u64) {
        let s = match self.src {
            Src::Ovt(o, l) => (o << 8) | l as u64,
            Src::Cell(c) => c as u64,
        };
        (
            !self.block as u8,
            self.layer_pos,
            self.hash,
            self.x,
            self.y,
            s,
        )
    }
}

fn fnv(vals: &[u64]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &v in vals {
        for b in v.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

/// clip translated into a rep's offset space: offset o is a hit
/// iff anchor+o is in clip, i.e. o in clip-(ax,ay) (saturating -
/// widening only over-includes and the per-member point test is
/// exact)
fn offset_box(clip: &BBox, ax: i64, ay: i64) -> BBox {
    BBox {
        x0: clip.x0.saturating_sub(ax),
        y0: clip.y0.saturating_sub(ay),
        x1: clip.x1.saturating_sub(ax),
        y1: clip.y1.saturating_sub(ay),
    }
}

struct LWalk<'a> {
    v: &'a Ovm,
    ovt: &'a [u8],
    req: &'a ViewReq,
    opts: &'a LabelOpts,
    cut: u64,
    bin: i64,
    member_budget: u64,
    bins: HashMap<(i64, i64), Cand>,
    raw_out: Vec<Cand>,
    st: TextStats,
    done: bool,
}

impl<'a> LWalk<'a> {
    fn add(&mut self, c: Cand) {
        if self.opts.raw {
            self.raw_out.push(c);
            if self.raw_out.len() >= self.opts.cand_cap {
                self.st.truncated = true;
                self.done = true;
            }
            return;
        }
        let key =
            (c.x.div_euclid(self.bin), c.y.div_euclid(self.bin));
        match self.bins.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                if c.prio() < e.get().prio() {
                    e.insert(c);
                }
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(c);
                if self.bins.len() >= self.opts.cand_cap {
                    self.st.truncated = true;
                    self.done = true;
                }
            }
        }
    }

    fn emit_text(
        &mut self,
        li: u32,
        soff: u64,
        slen: u32,
        xf: &Xf,
        lx: i64,
        ly: i64,
    ) {
        self.st.members_visible += 1;
        let (gx, gy) = xf.apply(lx, ly);
        self.add(Cand {
            block: false,
            layer_pos: li,
            hash: fnv(&[
                gx as u64,
                gy as u64,
                li as u64,
                soff,
                slen as u64,
            ]),
            x: gx,
            y: gy,
            src: Src::Ovt(soff, slen),
        });
    }

    /// every visible member of one text record (anchor point test
    /// exact; Grid via the closed-form index rectangle, Pts via
    /// the Morton chunk ladder over the ovt pool)
    fn one_text(&mut self, ti: u32, li: u32, xf: &Xf, clip: &BBox) {
        let t = self.v.text(ti);
        if !t.bbox.intersects(clip) {
            return;
        }
        self.st.records_candidate += 1;
        if t.rep_idx == TREP_NONE {
            self.st.members_tested += 1;
            if clip.contains_pt(t.x, t.y) {
                self.emit_text(
                    li,
                    t.string_off,
                    t.string_len,
                    xf,
                    t.x,
                    t.y,
                );
            }
            return;
        }
        let rbox = offset_box(clip, t.x, t.y);
        match self.v.trep(t.rep_idx) {
            TrepV::Grid { na, nb, va, vb } => {
                let (i0, i1, j0, j1) = match grid_ranges(
                    na as i64, nb as i64, va, vb, &rbox,
                ) {
                    GridVis::Empty => return,
                    GridVis::Range { i0, i1, j0, j1 } => {
                        (i0, i1, j0, j1)
                    }
                };
                'grid: for j in j0..=j1 {
                    for i in i0..=i1 {
                        if self.member_budget == 0 {
                            self.st.truncated = true;
                            self.done = true;
                            break 'grid;
                        }
                        self.member_budget -= 1;
                        self.st.members_tested += 1;
                        let ox = i as i128 * va.0 as i128
                            + j as i128 * vb.0 as i128;
                        let oy = i as i128 * va.1 as i128
                            + j as i128 * vb.1 as i128;
                        let (Ok(ox), Ok(oy)) =
                            (i64::try_from(ox), i64::try_from(oy))
                        else {
                            continue;
                        };
                        if rbox.contains_pt(ox, oy) {
                            self.emit_text(
                                li,
                                t.string_off,
                                t.string_len,
                                xf,
                                t.x + ox,
                                t.y + oy,
                            );
                        }
                        if self.done {
                            break 'grid;
                        }
                    }
                }
            }
            TrepV::Pts {
                count,
                pts_off,
                chunk_lo,
                chunk_count,
            } => {
                'pts: for k in 0..chunk_count {
                    let cb = self.v.tchunk(chunk_lo + k);
                    self.st.rep_chunks_scanned += 1;
                    if !cb.intersects(&rbox) {
                        continue;
                    }
                    let lo = k * PTS_CHUNK as u32;
                    let hi =
                        (lo + PTS_CHUNK as u32).min(count);
                    for s in lo..hi {
                        if self.member_budget == 0 {
                            self.st.truncated = true;
                            self.done = true;
                            break 'pts;
                        }
                        self.member_budget -= 1;
                        self.st.members_tested += 1;
                        let (ox, oy) = floe_ovm::ovt_pt(
                            self.ovt, pts_off, s,
                        );
                        if rbox.contains_pt(ox, oy) {
                            self.emit_text(
                                li,
                                t.string_off,
                                t.string_len,
                                xf,
                                t.x + ox,
                                t.y + oy,
                            );
                        }
                        if self.done {
                            break 'pts;
                        }
                    }
                }
            }
        }
    }

    fn own_texts(&mut self, ci: u32, xf: &Xf, clip: &BBox) {
        let (ts, tc) = self.v.cell_tranges(ci);
        for tri in ts..ts + tc {
            if self.done {
                return;
            }
            let tr = self.v.trange(tri);
            if !bit_test(&self.req.vis, tr.layer_idx as usize) {
                continue;
            }
            if tr.tbvh_root == TBVH_NONE {
                for ti in tr.text_lo..tr.text_lo + tr.text_count {
                    self.one_text(ti, tr.layer_idx, xf, clip);
                    if self.done {
                        return;
                    }
                }
            } else {
                let mut stack = vec![tr.tbvh_root];
                while let Some(ni) = stack.pop() {
                    let n = self.v.tbvh(ni);
                    self.st.tbvh_nodes += 1;
                    if !n.bbox.intersects(clip) {
                        continue;
                    }
                    if n.leaf {
                        for k in 0..n.count as u32 {
                            self.one_text(
                                n.first + k,
                                tr.layer_idx,
                                xf,
                                clip,
                            );
                            if self.done {
                                return;
                            }
                        }
                    } else {
                        for k in 0..n.count as u32 {
                            stack.push(n.first + k);
                        }
                    }
                }
            }
        }
    }

    fn norm_r(&self, ci: u32, r: u32) -> u32 {
        if r == REM_FULL {
            return REM_FULL;
        }
        if r >= self.v.cell(ci).height {
            REM_FULL
        } else {
            r
        }
    }
}

impl crate::Vfs {
    /// request-scoped display labels (default knobs); px_per_dbu
    /// == 0 (probes) yields no labels unless opts.raw
    pub fn plan_labels(
        &self,
        req: &ViewReq,
    ) -> Result<LabelPlan, String> {
        plan_labels(&self.ovm, self.ovt(), req, &LabelOpts::default())
    }

    pub fn plan_labels_with(
        &self,
        req: &ViewReq,
        opts: &LabelOpts,
    ) -> Result<LabelPlan, String> {
        plan_labels(&self.ovm, self.ovt(), req, opts)
    }
}

pub fn plan_labels(
    v: &Ovm,
    ovt: &[u8],
    req: &ViewReq,
    opts: &LabelOpts,
) -> Result<LabelPlan, String> {
    let px = req.px_per_dbu;
    if v.n_cells == 0 || (px <= 0.0 && !opts.raw) {
        return Ok(LabelPlan {
            rows: Vec::new(),
            stats: TextStats::default(),
        });
    }
    let bin = if px > 0.0 {
        ((opts.cell_px / px).ceil() as i64).max(1)
    } else {
        1
    };
    let mut w = LWalk {
        v,
        ovt,
        req,
        opts,
        cut: req.cut_dbu.max(0) as u64,
        bin,
        member_budget: opts.member_budget,
        bins: HashMap::new(),
        raw_out: Vec::new(),
        st: TextStats::default(),
        done: false,
    };
    // block gate in DBU (either placed axis): raw mode has no px,
    // so no block synthesis there unless px is supplied
    let block_min_dbu = if px > 0.0 {
        (opts.block_min_px / px).max(1.0)
    } else {
        f64::INFINITY
    };

    let top = v.top;
    let tc = v.cell(top);
    let r0 = w.norm_r(
        top,
        if req.depth == u32::MAX { REM_FULL } else { req.depth },
    );
    let seed = req.view.intersect(&tc.rbbox);
    let mut stack: Vec<(u32, u32, Xf, BBox)> = Vec::new();
    if !seed.is_empty() {
        stack.push((top, r0, Xf::identity(), seed));
    }
    while let Some((ci, r, xf, clip)) = stack.pop() {
        if w.done {
            break;
        }
        w.own_texts(ci, &xf, &clip);
        if w.done {
            break;
        }
        let cell = v.cell(ci);
        for pli in cell.place_start as u64
            ..cell.place_start as u64 + cell.place_count as u64
        {
            if w.done {
                break;
            }
            let h = v.place_head(pli);
            let rb = v.cell_rbbox(h.child);
            if rb.is_empty() {
                continue;
            }
            let cw = (rb.x1 - rb.x0).max(0) as u64;
            let chh = (rb.y1 - rb.y0).max(0) as u64;
            let below_cut = cw < w.cut && chh < w.cut;
            let t0 = Xf::place(h.x, h.y, h.rot, h.flip);
            let b0 = xf_bbox(&t0, &rb);
            // depth boundary: the child renders as an outline
            // frame - synthesize its NAME as a block candidate
            // when it is big enough on screen to read (par.4.3)
            let block = r == 0
                && !below_cut
                && ((b0.x1 - b0.x0).max(0) as f64
                    >= block_min_dbu
                    || (b0.y1 - b0.y0).max(0) as f64
                        >= block_min_dbu);
            let descend = r != 0
                && !below_cut
                && masks_intersect(
                    v.bitset(v.cell_tmask_rec(h.child)),
                    &req.vis,
                );
            if !block && !descend {
                continue;
            }
            let child_r = if r == REM_FULL {
                REM_FULL
            } else if r > 0 {
                w.norm_r(h.child, r - 1)
            } else {
                0
            };
            let rbox = offset_box(&clip, 0, 0); // clip in parent frame
            let mut handle =
                |w: &mut LWalk, ox: i64, oy: i64| {
                    if w.member_budget == 0 {
                        w.st.truncated = true;
                        w.done = true;
                        return;
                    }
                    w.member_budget -= 1;
                    w.st.place_members_enumerated += 1;
                    let mb = BBox {
                        x0: b0.x0.saturating_add(ox),
                        y0: b0.y0.saturating_add(oy),
                        x1: b0.x1.saturating_add(ox),
                        y1: b0.y1.saturating_add(oy),
                    };
                    if !mb.intersects(&clip) {
                        return;
                    }
                    if block {
                        w.st.blocks_visible += 1;
                        let cx = ((mb.x0 as i128 + mb.x1 as i128)
                            / 2)
                            as i64;
                        let cy = ((mb.y0 as i128 + mb.y1 as i128)
                            / 2)
                            as i64;
                        let (gx, gy) = xf.apply(cx, cy);
                        w.add(Cand {
                            block: true,
                            layer_pos: 0,
                            hash: fnv(&[
                                gx as u64,
                                gy as u64,
                                h.child as u64,
                            ]),
                            x: gx,
                            y: gy,
                            src: Src::Cell(h.child),
                        });
                        return;
                    }
                    let tm = Xf::place(
                        h.x.saturating_add(ox),
                        h.y.saturating_add(oy),
                        h.rot,
                        h.flip,
                    );
                    let cclip = xf_bbox(&tm.invert(), &clip)
                        .intersect(&rb);
                    if !cclip.is_empty() {
                        stack.push((
                            h.child,
                            child_r,
                            xf.compose(&tm),
                            cclip,
                        ));
                    }
                };
            let _ = &rbox;
            match h.kind {
                0 => handle(&mut w, 0, 0),
                1 => {
                    // offsets whose translate of b0 still meets
                    // clip: closed-form index rectangle over the
                    // Minkowski box, per-member exact test in
                    // handle()
                    let mk = BBox {
                        x0: clip.x0.saturating_sub(b0.x1),
                        y0: clip.y0.saturating_sub(b0.y1),
                        x1: clip.x1.saturating_sub(b0.x0),
                        y1: clip.y1.saturating_sub(b0.y0),
                    };
                    match grid_ranges(
                        h.na as i64,
                        h.nb as i64,
                        h.va,
                        h.vb,
                        &mk,
                    ) {
                        GridVis::Empty => {}
                        GridVis::Range { i0, i1, j0, j1 } => {
                            'g: for j in j0..=j1 {
                                for i in i0..=i1 {
                                    let ox = i as i128
                                        * h.va.0 as i128
                                        + j as i128
                                            * h.vb.0 as i128;
                                    let oy = i as i128
                                        * h.va.1 as i128
                                        + j as i128
                                            * h.vb.1 as i128;
                                    let (Ok(ox), Ok(oy)) = (
                                        i64::try_from(ox),
                                        i64::try_from(oy),
                                    ) else {
                                        continue;
                                    };
                                    handle(&mut w, ox, oy);
                                    if w.done {
                                        break 'g;
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {
                    let Some(pr) = v.pts_ref(pli) else {
                        continue;
                    };
                    let mk = BBox {
                        x0: clip.x0.saturating_sub(b0.x1),
                        y0: clip.y0.saturating_sub(b0.y1),
                        x1: clip.x1.saturating_sub(b0.x0),
                        y1: clip.y1.saturating_sub(b0.y0),
                    };
                    if !pr.extent().intersects(&mk) {
                        continue;
                    }
                    'p: for k in 0..pr.n_chunks {
                        let cb = pr.chunk_bbox(k);
                        w.st.rep_chunks_scanned += 1;
                        if !cb.intersects(&mk) {
                            continue;
                        }
                        let (lo, hi) = pr.chunk_range(k);
                        for s in lo..hi {
                            let (ox, oy) = pr.pt(s);
                            if mk.contains_pt(ox, oy) {
                                handle(&mut w, ox, oy);
                                if w.done {
                                    break 'p;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // selection + string resolve (SELECTED rows only touch ovt
    // string bytes / cell names)
    let mut st = w.st;
    let cands: Vec<Cand> = if opts.raw {
        let mut c = w.raw_out;
        c.sort_by_key(|c| c.prio());
        c
    } else {
        let mut c: Vec<Cand> = w.bins.into_values().collect();
        c.sort_by_key(|c| c.prio());
        st.budget_dropped =
            c.len().saturating_sub(opts.view_budget) as u64;
        c.truncate(opts.view_budget);
        c
    };
    st.selected = cands.len() as u64;
    let mut rows = Vec::with_capacity(cands.len());
    for c in cands {
        let (layer, dt, s) = match c.src {
            Src::Ovt(off, len) => {
                let lr = v.layer(c.layer_pos);
                let end = (off + len as u64) as usize;
                let bytes = ovt
                    .get(off as usize..end)
                    .ok_or_else(|| {
                        "corrupt cache; rebuild: text string \
                         beyond design.ovt"
                            .to_string()
                    })?;
                let s = std::str::from_utf8(bytes)
                    .map_err(|_| {
                        "corrupt cache; rebuild: text string \
                         not UTF-8"
                            .to_string()
                    })?
                    .to_string();
                (lr.layer, lr.dt, s)
            }
            Src::Cell(ci) => (0, 0, v.cell(ci).name),
        };
        rows.push(LabelRow {
            block: matches!(c.src, Src::Cell(_)),
            layer,
            dt,
            x: c.x,
            y: c.y,
            s,
        });
    }
    Ok(LabelPlan { rows, stats: st })
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_oasis::doc::Rep;
    use floe_ovm::Builder;

    fn bx(x0: i64, y0: i64, x1: i64, y1: i64) -> BBox {
        BBox { x0, y0, x1, y1 }
    }

    /// fixture: SUB holds 3 texts (One "pin", 4x2 grid "g", 500-pt
    /// pts "p") on layer 7/0 plus one text on 9/1; TOP holds one
    /// own text, SUB at identity, SUB rotated 90 + flipped at an
    /// offset, and a 3x1 array of SUB. ovt built alongside.
    fn fixture() -> (Ovm, Vec<u8>) {
        let mut ovt: Vec<u8> = Vec::new();
        let mut b = Builder::new(1000.0, 0, 0, 2);
        b.top = 1;
        b.layer(7, 0, "TXT", 0, 0);
        b.layer(9, 1, "TXT2", 0, 0);
        let m_all = b.bitset(&[0b11]);
        let mut put = |ovt: &mut Vec<u8>, s: &str| -> (u64, u32) {
            let off = ovt.len() as u64;
            ovt.extend_from_slice(s.as_bytes());
            (off, s.len() as u32)
        };
        // ---- SUB (ci 0)
        let sub_tr = b.n_tranges();
        {
            // layer 0 run
            let t0 = b.n_texts();
            let (o1, l1) = put(&mut ovt, "pin");
            b.text(0, 0, 10, 20, o1, l1, TREP_NONE,
                   &bx(10, 20, 10, 20), 0);
            let (o2, l2) = put(&mut ovt, "g");
            let rg = b.trep_grid(4, 2, (100, 0), (0, 200));
            b.text(0, 0, 0, 0, o2, l2, rg,
                   &bx(0, 0, 300, 200), 1);
            let (o3, l3) = put(&mut ovt, "p");
            let src: Vec<(i64, i64)> = (0..500)
                .map(|i| ((i * 13) % 400, (i * 29) % 350))
                .collect();
            let prep = floe_ovm::prepare_pts(&src);
            let po = ovt.len() as u64;
            for &(x, y) in &prep.pts {
                ovt.extend_from_slice(&x.to_le_bytes());
                ovt.extend_from_slice(&y.to_le_bytes());
            }
            let clo = b.n_tchunks();
            for c in &prep.chunks {
                b.tchunk(c);
            }
            let rp = b.trep_pts(500, po, clo,
                                prep.chunks.len() as u32);
            let mut pb = prep.extent;
            pb.x0 += 3;
            pb.y0 += 4;
            pb.x1 += 3;
            pb.y1 += 4;
            b.text(0, 0, 3, 4, o3, l3, rp, &pb, 2);
            b.trange(0, t0, 3, TBVH_NONE);
            // layer 1 run
            let t1 = b.n_texts();
            let (o4, l4) = put(&mut ovt, "vdd");
            b.text(0, 1, 50, 50, o4, l4, TREP_NONE,
                   &bx(50, 50, 50, 50), 0);
            b.trange(1, t1, 1, TBVH_NONE);
        }
        // must CONTAIN every text member (the real build grows
        // rbbox by text extents; the planner clips to rbbox)
        let sub_bb = bx(0, 0, 402, 360);
        b.cell("SUB", 0, 1, &sub_bb, &sub_bb, 0, 0, 0, 0, 0, 0,
               0, 0, m_all, m_all, 0, sub_tr, 2, m_all);
        // ---- TOP (ci 1): own text + three placements of SUB
        let top_tr = b.n_tranges();
        {
            let t0 = b.n_texts();
            let (o, l) = put(&mut ovt, "TOPLBL");
            b.text(1, 0, 5000, 5000, o, l, TREP_NONE,
                   &bx(5000, 5000, 5000, 5000), 0);
            b.trange(0, t0, 1, TBVH_NONE);
        }
        let p0 = b.place(0, 0, 0, 0, false, &Rep::One);
        b.place(0, 2000, 100, 1, true, &Rep::One);
        b.place(
            0,
            0,
            3000,
            0,
            false,
            &Rep::Grid { na: 3, nb: 1, va: (600, 0), vb: (0, 0) },
        );
        let items = bx(-400, 0, 6000, 6000);
        let n0 = b.bvh_node(&items, p0 as u32, 3, true);
        let top_bb = bx(-400, 0, 6000, 6000);
        b.cell("TOP", 1, 0, &top_bb, &top_bb, p0 as u32, 3, 0, 0,
               n0, 1, 0, 0, m_all, m_all, 0, top_tr, 1, m_all);
        (Ovm::from_bytes(b.finish(0, ovt.len() as u64)).unwrap(),
         ovt)
    }

    fn rq(view: BBox, cut: i64, depth: u32, px: f64) -> ViewReq {
        ViewReq {
            view,
            cut_dbu: cut,
            vis: vec![0xff],
            depth,
            px_per_dbu: px,
        }
    }

    /// brute-force oracle: expand every placement member and text
    /// member through materialized reps, same depth/cut/vis rules
    fn brute(
        v: &Ovm,
        ovt: &[u8],
        req: &ViewReq,
    ) -> Vec<(u32, i64, i64, String)> {
        fn rep_offs(
            v: &Ovm,
            ovt: &[u8],
            t: &floe_ovm::TextV,
        ) -> Vec<(i64, i64)> {
            if t.rep_idx == TREP_NONE {
                return vec![(0, 0)];
            }
            match v.trep(t.rep_idx) {
                TrepV::Grid { na, nb, va, vb } => {
                    let mut o = Vec::new();
                    for j in 0..nb as i64 {
                        for i in 0..na as i64 {
                            o.push((
                                i * va.0 + j * vb.0,
                                i * va.1 + j * vb.1,
                            ));
                        }
                    }
                    o
                }
                TrepV::Pts { count, pts_off, .. } => (0..count)
                    .map(|s| floe_ovm::ovt_pt(ovt, pts_off, s))
                    .collect(),
            }
        }
        fn walk(
            v: &Ovm,
            ovt: &[u8],
            ci: u32,
            r: u32,
            xf: &Xf,
            req: &ViewReq,
            out: &mut Vec<(u32, i64, i64, String)>,
        ) {
            let (ts, tc) = v.cell_tranges(ci);
            for tri in ts..ts + tc {
                let tr = v.trange(tri);
                if !bit_test(&req.vis, tr.layer_idx as usize) {
                    continue;
                }
                for ti in tr.text_lo..tr.text_lo + tr.text_count {
                    let t = v.text(ti);
                    for (ox, oy) in rep_offs(v, ovt, &t) {
                        let (gx, gy) =
                            xf.apply(t.x + ox, t.y + oy);
                        if req.view.contains_pt(gx, gy) {
                            let s = std::str::from_utf8(
                                &ovt[t.string_off as usize
                                    ..(t.string_off
                                        + t.string_len as u64)
                                        as usize],
                            )
                            .unwrap()
                            .to_string();
                            out.push((
                                tr.layer_idx,
                                gx,
                                gy,
                                s,
                            ));
                        }
                    }
                }
            }
            if r == 0 {
                return;
            }
            let cell = v.cell(ci);
            for pli in cell.place_start as u64
                ..cell.place_start as u64
                    + cell.place_count as u64
            {
                let pl = v.place(pli);
                let rb = v.cell_rbbox(pl.child);
                let cw = (rb.x1 - rb.x0).max(0) as u64;
                let chh = (rb.y1 - rb.y0).max(0) as u64;
                let cut = req.cut_dbu.max(0) as u64;
                if cw < cut && chh < cut {
                    continue;
                }
                let offs: Vec<(i64, i64)> = match &pl.rep {
                    Rep::One => vec![(0, 0)],
                    Rep::Grid { na, nb, va, vb } => {
                        let mut o = Vec::new();
                        for j in 0..*nb as i64 {
                            for i in 0..*na as i64 {
                                o.push((
                                    i * va.0 + j * vb.0,
                                    i * va.1 + j * vb.1,
                                ));
                            }
                        }
                        o
                    }
                    Rep::Pts(p) => p.to_vec(),
                };
                let nr = if r == REM_FULL {
                    REM_FULL
                } else {
                    r - 1
                };
                for (ox, oy) in offs {
                    let t = Xf::place(
                        pl.x + ox,
                        pl.y + oy,
                        pl.rot,
                        pl.flip,
                    );
                    walk(
                        v,
                        ovt,
                        pl.child,
                        nr,
                        &xf.compose(&t),
                        req,
                        out,
                    );
                }
            }
        }
        let mut out = Vec::new();
        let r0 = if req.depth == u32::MAX {
            REM_FULL
        } else {
            req.depth
        };
        walk(v, ovt, v.top, r0, &Xf::identity(), req, &mut out);
        out.sort();
        out
    }

    fn raw_rows(
        v: &Ovm,
        ovt: &[u8],
        req: &ViewReq,
    ) -> Vec<(u32, i64, i64, String)> {
        let mut o = LabelOpts::default();
        o.raw = true;
        o.cand_cap = usize::MAX;
        o.member_budget = u64::MAX;
        let lp = plan_labels(v, ovt, req, &o).unwrap();
        assert!(!lp.stats.truncated);
        let mut got: Vec<(u32, i64, i64, String)> = lp
            .rows
            .iter()
            .filter(|r| !r.block)
            .map(|r| {
                // layer back to ovm index for oracle comparison
                let li = (0..v.n_layers)
                    .find(|&i| {
                        let l = v.layer(i);
                        (l.layer, l.dt) == (r.layer, r.dt)
                    })
                    .unwrap();
                (li, r.x, r.y, r.s.clone())
            })
            .collect();
        got.sort();
        got
    }

    /// raw candidates == brute-force oracle across depth, layer
    /// visibility, rotation/flip, arrays, pts, narrow/wide views
    #[test]
    fn raw_candidates_match_brute_force() {
        let (v, ovt) = fixture();
        let views = [
            bx(-400, 0, 6000, 6000),  // everything
            bx(0, 0, 400, 360),       // SUB at identity only
            bx(1400, 50, 2100, 600),  // rotated/flipped SUB
            bx(0, 3000, 2200, 3400),  // array band
            bx(4990, 4990, 5010, 5010), // top label point
        ];
        for view in views {
            for depth in [u32::MAX, 0, 1] {
                let req = rq(view, 0, depth, 0.0);
                assert_eq!(
                    raw_rows(&v, &ovt, &req),
                    brute(&v, &ovt, &req),
                    "view {:?} depth {}",
                    view,
                    depth
                );
            }
        }
        // layer visibility: only layer 9/1 visible
        let mut req = rq(bx(-400, 0, 6000, 6000), 0, u32::MAX, 0.0);
        req.vis = vec![0b10];
        assert_eq!(
            raw_rows(&v, &ovt, &req),
            brute(&v, &ovt, &req)
        );
        // cell-level cut: SUB (400x360) below a 500-dbu cut -> only
        // TOP's own label remains
        let req = rq(bx(-400, 0, 6000, 6000), 500, u32::MAX, 0.0);
        let got = raw_rows(&v, &ovt, &req);
        assert_eq!(got, brute(&v, &ovt, &req));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].3, "TOPLBL");
    }

    /// declutter: budget respected, selected is a subset of raw,
    /// two runs byte-identical, world-anchored bins stable
    #[test]
    fn declutter_budget_subset_deterministic() {
        let (v, ovt) = fixture();
        let req = rq(bx(-400, 0, 6000, 6000), 0, u32::MAX, 0.05);
        let mut o = LabelOpts::default();
        o.view_budget = 10;
        let a = plan_labels(&v, &ovt, &req, &o).unwrap();
        let b2 = plan_labels(&v, &ovt, &req, &o).unwrap();
        assert_eq!(a.rows, b2.rows);
        assert!(a.rows.len() <= 10);
        assert!(a.stats.selected as usize == a.rows.len());
        let raw = raw_rows(&v, &ovt, &req);
        for r in a.rows.iter().filter(|r| !r.block) {
            assert!(
                raw.iter().any(|(_, x, y, s)| (*x, *y, &r.s)
                    == (r.x, r.y, s)),
                "selected {:?} not in raw",
                r
            );
        }
        // px 0 (probe): no labels at all
        let req0 = rq(bx(-400, 0, 6000, 6000), 0, u32::MAX, 0.0);
        let p = plan_labels(&v, &ovt, &req0, &LabelOpts::default())
            .unwrap();
        assert!(p.rows.is_empty());
    }

    /// depth boundary blocks: r == 0 children big enough on screen
    /// become named block candidates; below the size gate they
    /// stay silent
    #[test]
    fn depth_zero_blocks_named_and_gated() {
        let (v, ovt) = fixture();
        // px chosen so SUB (400 dbu) is 400 px -> passes 96 px
        let req = rq(bx(-400, 0, 6000, 6000), 0, 0, 1.0);
        let lp = plan_labels(&v, &ovt, &req, &LabelOpts::default())
            .unwrap();
        let blocks: Vec<&LabelRow> =
            lp.rows.iter().filter(|r| r.block).collect();
        assert!(
            blocks.iter().any(|r| r.s == "SUB"),
            "{:?}",
            lp.rows
        );
        assert!(lp.stats.blocks_visible >= 3, "{:?}", lp.stats);
        // zoomed way out: SUB is 0.4 px -> gate silences blocks
        let req2 = rq(bx(-400, 0, 6000, 6000), 0, 0, 0.001);
        let lp2 =
            plan_labels(&v, &ovt, &req2, &LabelOpts::default())
                .unwrap();
        assert!(lp2.rows.iter().all(|r| !r.block), "{:?}", lp2.rows);
    }

    /// member budget exhaustion flags truncated instead of
    /// scanning forever
    #[test]
    fn member_budget_truncates() {
        let (v, ovt) = fixture();
        let req = rq(bx(-400, 0, 6000, 6000), 0, u32::MAX, 0.05);
        let mut o = LabelOpts::default();
        o.member_budget = 8;
        let lp = plan_labels(&v, &ovt, &req, &o).unwrap();
        assert!(lp.stats.truncated);
    }
}
