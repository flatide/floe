//! Far-zoom skeleton (cache.build_skeleton parity) and the full-text
//! walk feeding both the skeleton labels and the search sidecar.
//!
//! Skeleton: depth-0 content of the far view - large top-level shapes
//! on their design layers, outline boxes + cell-name labels of big
//! first-level cells on the synthetic layer 255/0, large shapes
//! harvested from big level-k cells (k <= 2) on per-level twin layers
//! (datatype + k*30000), text labels on the level-1 twin. Same layer
//! encoding the render service already consumes.
//!
//! Texts: unlike the Python indexer (whose collection is bounded and
//! thins over-budget layers spatially), the record-level walk keeps
//! EVERY text without expansion: an entry per (instance path, text
//! record) with the repetition factors carried symbolically in top
//! frame. The skeleton label set stays bounded like Python's; the
//! sidecar keeps the lot for search.

use crate::hier::cell_bboxes;
use crate::{path_bbox, rep_offsets, xf_rep, Xf};
use floe_oasis::doc::{Doc, PathRec, PolyRec, RectRec, Rep, TextRec};
use std::collections::HashMap;

pub const SKEL_DETAIL_DT: u32 = 30_000;
pub const SKEL_DETAIL_LEVELS: u32 = 2;
pub const SKEL_SHAPE_CAP: u64 = 300_000;
pub const SKEL_TEXT_CAP: u64 = 50_000;
const SKEL_CONTAINER_SKIP: u64 = 60_000;

pub struct Skeleton {
    pub rects: Vec<RectRec>,
    pub polys: Vec<PolyRec>,
    pub paths: Vec<PathRec>,
    pub texts: Vec<TextRec>,
    /// harvested shape members (meta skeleton.shapes)
    pub shapes: u64,
    /// kept labels (meta skeleton.texts)
    pub labels: u64,
    /// text layers thinned by the label budget (meta texts_thinned)
    pub thinned: Vec<(u32, u32)>,
}

/// One text record on one instance path, in top coordinates. The
/// expanded member set is x/y plus every combination of one offset
/// from each factor (all factors already rotated into top frame).
pub struct TextEntry {
    pub layer: u32,
    pub dt: u32,
    pub x: i64,
    pub y: i64,
    pub factors: Vec<Rep>,
    pub s: String,
}

impl TextEntry {
    pub fn members(&self) -> u64 {
        self.factors.iter().map(|f| f.members()).product()
    }
}

fn poly_wh(p: &PolyRec) -> (i64, i64) {
    let (mut x0, mut y0, mut x1, mut y1) =
        (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for &(x, y) in &p.pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    (x1 - x0, y1 - y0)
}

fn xf_rect(r: &RectRec, xf: &Xf, dt: u32) -> RectRec {
    let a = xf.apply(r.x, r.y);
    let b = xf.apply(r.x + r.w, r.y + r.h);
    RectRec {
        layer: r.layer,
        dt,
        x: a.0.min(b.0),
        y: a.1.min(b.1),
        w: (a.0 - b.0).abs(),
        h: (a.1 - b.1).abs(),
        rep: xf_rep(&r.rep, xf),
    }
}

fn xf_poly(p: &PolyRec, xf: &Xf, dt: u32) -> PolyRec {
    PolyRec {
        layer: p.layer,
        dt,
        pts: p.pts.iter().map(|&(x, y)| xf.apply(x, y)).collect(),
        rep: xf_rep(&p.rep, xf),
    }
}

fn xf_path(p: &PathRec, xf: &Xf, dt: u32) -> PathRec {
    // orthogonal transform: half-width and the along-spine
    // extensions are invariant, the spine and rep offsets rotate
    PathRec {
        layer: p.layer,
        dt,
        pts: p.pts.iter().map(|&(x, y)| xf.apply(x, y)).collect(),
        hw: p.hw,
        es: p.es,
        ee: p.ee,
        rep: xf_rep(&p.rep, xf),
    }
}

/// first k members of a repetition (rep_offsets order) - the
/// skeleton cap used to be enforced at record granularity, and one
/// die-wide repetition blew straight through it (a 9.8 GB chip
/// reported 8.49M skeleton shapes against the 300k budget)
fn take_members(rep: &Rep, k: u64) -> Rep {
    if k >= rep.members() {
        return rep.clone();
    }
    if k <= 1 {
        return Rep::One;
    }
    Rep::Pts(
        rep_offsets(rep)
            .into_iter()
            .take(k as usize)
            .collect(),
    )
}

/// klayout Box.center(): C++ integer division, truncation toward 0
fn half(a: i64, b: i64) -> i64 {
    (a + b) / 2
}

/// per-cell per-layer stored member counts INCLUDING texts - the
/// 60k fill-container skip mirrors klayout Shapes.size()
fn container_sizes(doc: &Doc) -> Vec<HashMap<(u32, u32), u64>> {
    doc.cells
        .iter()
        .map(|c| {
            let mut m: HashMap<(u32, u32), u64> = HashMap::new();
            for r in &c.rects {
                *m.entry((r.layer, r.dt)).or_default() += r.rep.members();
            }
            for p in &c.polys {
                *m.entry((p.layer, p.dt)).or_default() += p.rep.members();
            }
            for pa in &c.paths {
                *m.entry((pa.layer, pa.dt)).or_default() +=
                    pa.rep.members();
            }
            for t in &c.texts {
                *m.entry((t.layer, t.dt)).or_default() += t.rep.members();
            }
            m
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn harvest(
    doc: &Doc,
    bbs: &[Option<(i64, i64, i64, i64)>],
    csize: &[HashMap<(u32, u32), u64>],
    sk: &mut Skeleton,
    ci: usize,
    xf: &Xf,
    level: u32,
    min_feat: i64,
    big: i64,
    mut n: u64,
) -> u64 {
    let cell = &doc.cells[ci];
    let dtoff = level * SKEL_DETAIL_DT;
    for r in &cell.rects {
        if csize[ci][&(r.layer, r.dt)] > SKEL_CONTAINER_SKIP {
            continue;
        }
        if n >= SKEL_SHAPE_CAP {
            return n;
        }
        if r.w >= min_feat || r.h >= min_feat {
            let take = (SKEL_SHAPE_CAP - n).min(r.rep.members());
            let mut t = xf_rect(r, xf, r.dt + dtoff);
            t.rep = xf_rep(&take_members(&r.rep, take), xf);
            sk.rects.push(t);
            n += take;
        }
    }
    for p in &cell.polys {
        if csize[ci][&(p.layer, p.dt)] > SKEL_CONTAINER_SKIP {
            continue;
        }
        if n >= SKEL_SHAPE_CAP {
            return n;
        }
        let (w, h) = poly_wh(p);
        if w >= min_feat || h >= min_feat {
            let take = (SKEL_SHAPE_CAP - n).min(p.rep.members());
            let mut t = xf_poly(p, xf, p.dt + dtoff);
            t.rep = xf_rep(&take_members(&p.rep, take), xf);
            sk.polys.push(t);
            n += take;
        }
    }
    for pa in &cell.paths {
        if csize[ci][&(pa.layer, pa.dt)] > SKEL_CONTAINER_SKIP {
            continue;
        }
        if n >= SKEL_SHAPE_CAP {
            return n;
        }
        let bb = path_bbox(&pa.pts, pa.hw, pa.es, pa.ee);
        if bb.2 - bb.0 >= min_feat || bb.3 - bb.1 >= min_feat {
            let take = (SKEL_SHAPE_CAP - n).min(pa.rep.members());
            let mut t = xf_path(pa, xf, pa.dt + dtoff);
            t.rep = xf_rep(&take_members(&pa.rep, take), xf);
            sk.paths.push(t);
            n += take;
        }
    }
    if level >= SKEL_DETAIL_LEVELS {
        return n;
    }
    for pl in &cell.places {
        if n >= SKEL_SHAPE_CAP {
            break;
        }
        let cb = match bbs[pl.cell] {
            Some(b) => b,
            None => continue,
        };
        if cb.2 - cb.0 < big && cb.3 - cb.1 < big {
            continue;
        }
        for (ox, oy) in rep_offsets(&pl.rep) {
            if n >= SKEL_SHAPE_CAP {
                break;
            }
            let mxf = xf
                .compose(&Xf::place(pl.x + ox, pl.y + oy, pl.rot, pl.flip));
            n = harvest(doc, bbs, csize, sk, pl.cell, &mxf, level + 1,
                        min_feat, big, n);
        }
    }
    n
}

/// Full text walk: one entry per (instance path, text record), reps
/// symbolic, coordinates in top frame. Subtrees without texts prune.
pub fn collect_all_texts(doc: &Doc) -> Vec<TextEntry> {
    let mut has = vec![false; doc.cells.len()];
    // children-before-parents is not guaranteed in doc order: iterate
    // to a fixed point (cheap - depth passes over a cell list)
    loop {
        let mut changed = false;
        for (ci, cell) in doc.cells.iter().enumerate() {
            if has[ci] {
                continue;
            }
            if !cell.texts.is_empty()
                || cell.places.iter().any(|p| has[p.cell])
            {
                has[ci] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut out = Vec::new();
    fn walk(
        doc: &Doc,
        has: &[bool],
        ci: usize,
        xf: &Xf,
        factors: &[Rep],
        out: &mut Vec<TextEntry>,
    ) {
        let cell = &doc.cells[ci];
        for t in &cell.texts {
            let (x, y) = xf.apply(t.x, t.y);
            let mut f = factors.to_vec();
            if let r @ (Rep::Grid { .. } | Rep::Pts(_)) =
                xf_rep(&t.rep, xf)
            {
                f.push(r);
            }
            out.push(TextEntry {
                layer: t.layer,
                dt: t.dt,
                x,
                y,
                factors: f,
                s: t.s.clone(),
            });
        }
        for pl in &cell.places {
            if !has[pl.cell] {
                continue;
            }
            let base =
                xf.compose(&Xf::place(pl.x, pl.y, pl.rot, pl.flip));
            let mut f = factors.to_vec();
            if let r @ (Rep::Grid { .. } | Rep::Pts(_)) =
                xf_rep(&pl.rep, xf)
            {
                f.push(r);
            }
            walk(doc, has, pl.cell, &base, &f, out);
        }
    }
    walk(doc, &has, doc.top, &Xf::identity(), &[], &mut out);
    out
}

/// Expand one entry's members: x/y plus every factor-offset
/// combination (bounded by the caller via the layer classification).
fn expand(e: &TextEntry) -> Vec<(i64, i64)> {
    let mut pts = vec![(e.x, e.y)];
    for f in &e.factors {
        let offs = rep_offsets(f);
        let mut nxt = Vec::with_capacity(pts.len() * offs.len());
        for &(x, y) in &pts {
            for &(ox, oy) in &offs {
                nxt.push((x + ox, y + oy));
            }
        }
        pts = nxt;
    }
    pts
}

/// member j of an entry without expanding the rest: nested factor
/// decomposition (j digits over the factor sizes, last factor fastest)
fn member_at(e: &TextEntry, mut j: u64) -> (i64, i64) {
    let (mut x, mut y) = (e.x, e.y);
    for f in e.factors.iter().rev() {
        let m = f.members();
        let k = (j % m) as usize;
        j /= m;
        let (ox, oy) = match f {
            Rep::One => (0, 0),
            Rep::Grid { na, va, vb, .. } => {
                let i = (k as u64 % na) as i64;
                let jj = (k as u64 / na) as i64;
                (i * va.0 + jj * vb.0, i * va.1 + jj * vb.1)
            }
            Rep::Pts(p) => p[k],
        };
        x += ox;
        y += oy;
    }
    (x, y)
}

/// Skeleton label selection, mirroring collect_texts: per-layer slice
/// = 4*budget/n_text_layers; under-slice layers collected in full
/// (python-exact); over-slice layers keep ~slice members by a
/// deterministic arithmetic stride (python thins by spatial region
/// with klayout-iteration-order truncation, which is not reproducible
/// - the sidecar holds everything, so the label subset for thinned
/// layers is a display heuristic on both sides). Returns label tuples
/// (layer_order position, string, x, y) sorted like python plus the
/// thinned layer list.
fn select_labels(
    doc: &Doc,
    entries: &[TextEntry],
    budget: u64,
) -> (Vec<((u32, u32), usize, String, i64, i64)>, Vec<(u32, u32)>) {
    let mut order_pos: HashMap<(u32, u32), usize> = HashMap::new();
    for (i, &ld) in doc.layer_order.iter().enumerate() {
        order_pos.insert(ld, i);
    }
    let mut per_layer: HashMap<(u32, u32), u64> = HashMap::new();
    for e in entries {
        *per_layer.entry((e.layer, e.dt)).or_default() += e.members();
    }
    let mut labels: Vec<((u32, u32), usize, String, i64, i64)> =
        Vec::new();
    let mut thinned: Vec<(u32, u32)> = Vec::new();
    if per_layer.is_empty() || budget == 0 {
        return (labels, thinned);
    }
    let slice = (budget * 4 / per_layer.len() as u64).max(1);
    let mut layers: Vec<(u32, u32)> = per_layer.keys().copied().collect();
    layers.sort_by_key(|ld| order_pos.get(ld).copied().unwrap_or(usize::MAX));
    for ld in layers {
        let total = per_layer[&ld];
        let pos = order_pos.get(&ld).copied().unwrap_or(usize::MAX);
        let mine: Vec<&TextEntry> = entries
            .iter()
            .filter(|e| (e.layer, e.dt) == ld)
            .collect();
        if total <= slice {
            for e in &mine {
                for (x, y) in expand(e) {
                    labels.push((ld, pos, e.s.clone(), x, y));
                }
            }
        } else {
            thinned.push(ld);
            let stride = total.div_ceil(slice).max(1);
            let mut gidx = 0u64;
            for e in &mine {
                let m = e.members();
                let mut j = (stride - gidx % stride) % stride;
                while j < m {
                    let (x, y) = member_at(e, j);
                    labels.push((ld, pos, e.s.clone(), x, y));
                    j += stride;
                }
                gidx += m;
            }
        }
    }
    // python: tuples.sort(key=(layer_index, x, y, string)), then a
    // uniform stride down to the budget
    labels.sort_by(|a, b| {
        (a.1, a.3, a.4, &a.2).cmp(&(b.1, b.3, b.4, &b.2))
    });
    if labels.len() as u64 > budget {
        let step = labels.len() as f64 / budget as f64;
        labels = (0..budget)
            .map(|i| labels[(i as f64 * step) as usize].clone())
            .collect();
    }
    (labels, thinned)
}

/// Build the skeleton content (single SKEL_TOP cell worth of records).
pub fn build_skeleton(
    doc: &Doc,
    entries: &[TextEntry],
    budget: u64,
) -> Skeleton {
    let bbs = cell_bboxes(doc);
    let bb = bbs[doc.top].expect("empty top cell");
    let min_feat = ((bb.2 - bb.0).max(bb.3 - bb.1) / 500).max(1);
    let big = min_feat * 4;
    let csize = container_sizes(doc);
    let mut sk = Skeleton {
        rects: Vec::new(),
        polys: Vec::new(),
        paths: Vec::new(),
        texts: Vec::new(),
        shapes: 0,
        labels: 0,
        thinned: Vec::new(),
    };
    let mut n = 0u64;
    let topc = &doc.cells[doc.top];
    for r in &topc.rects {
        if n >= SKEL_SHAPE_CAP {
            break;
        }
        if r.w >= min_feat || r.h >= min_feat {
            let take = (SKEL_SHAPE_CAP - n).min(r.rep.members());
            let mut t = r.clone();
            t.rep = take_members(&r.rep, take);
            sk.rects.push(t);
            n += take;
        }
    }
    for p in &topc.polys {
        if n >= SKEL_SHAPE_CAP {
            break;
        }
        let (w, h) = poly_wh(p);
        if w >= min_feat || h >= min_feat {
            let take = (SKEL_SHAPE_CAP - n).min(p.rep.members());
            let mut t = p.clone();
            t.rep = take_members(&p.rep, take);
            sk.polys.push(t);
            n += take;
        }
    }
    for pa in &topc.paths {
        if n >= SKEL_SHAPE_CAP {
            break;
        }
        let bb = path_bbox(&pa.pts, pa.hw, pa.es, pa.ee);
        if bb.2 - bb.0 >= min_feat || bb.3 - bb.1 >= min_feat {
            let take = (SKEL_SHAPE_CAP - n).min(pa.rep.members());
            let mut t = pa.clone();
            t.rep = take_members(&pa.rep, take);
            sk.paths.push(t);
            n += take;
        }
    }
    'insts: for pl in &topc.places {
        if n >= SKEL_SHAPE_CAP {
            break;
        }
        let cb = match bbs[pl.cell] {
            Some(b) => b,
            None => continue,
        };
        if cb.2 - cb.0 < big && cb.3 - cb.1 < big {
            continue;
        }
        for (ox, oy) in rep_offsets(&pl.rep) {
            if n >= SKEL_SHAPE_CAP {
                break 'insts;
            }
            let xf = Xf::place(pl.x + ox, pl.y + oy, pl.rot, pl.flip);
            let a = xf.apply(cb.0, cb.1);
            let b2 = xf.apply(cb.2, cb.3);
            let (x0, x1) = (a.0.min(b2.0), a.0.max(b2.0));
            let (y0, y1) = (a.1.min(b2.1), a.1.max(b2.1));
            sk.rects.push(RectRec {
                layer: 255,
                dt: 0,
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
                rep: Rep::One,
            });
            sk.texts.push(TextRec {
                layer: 255,
                dt: 0,
                x: half(x0, x1),
                y: half(y0, y1),
                rep: Rep::One,
                s: doc.cells[pl.cell].name.clone(),
            });
            n = harvest(doc, &bbs, &csize, &mut sk, pl.cell, &xf, 1,
                        min_feat, big, n);
        }
    }
    sk.shapes = n;
    let (labels, thinned) = select_labels(doc, entries, budget);
    sk.labels = labels.len() as u64;
    sk.thinned = thinned;
    for ((l, d), _pos, s, x, y) in labels {
        // level-1 twin layer, rotation dropped like python's rebuild
        sk.texts.push(TextRec {
            layer: l,
            dt: d + SKEL_DETAIL_DT,
            x,
            y,
            rep: Rep::One,
            s,
        });
    }
    sk
}
