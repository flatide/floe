//! The whole-Doc pre-passes of the index build - recursive bbox,
//! direct masks / members / bbox with the per-layer totals, and the
//! topological fold - as one value with two sources: the parsed Doc
//! (`PrePass::from_doc`, the code build() always ran) and the
//! inventory pass (`PrePass::from_inventory`, floe_oasis::inventory,
//! docs/STREAMING_INDEX_PLAN.ko.md S1). `floe-index inventory FILE
//! --check` holds the two equal cell by cell (gate G1), with the Doc's
//! cell table, layer tables, top, edges and the no-text recursive
//! boxes (tiler cell_bboxes) on top.

use floe_oasis::doc::Doc;
use floe_oasis::geom::{box_is_empty, Box4, EMPTY_BOX};
use floe_oasis::inventory::{Edge, Inventory};
use floe_ovm::{narrow_u32, BBox};
use floe_tiler::hier::{cell_bboxes, cell_bboxes_full, rep_extent};
use floe_tiler::path_bbox;
use std::collections::HashMap;

pub struct PrePass {
    /// recursive bbox per cell, text anchors included (tiler
    /// cell_bboxes_full); None = nothing at all below the cell
    pub rbb: Vec<Option<(i64, i64, i64, i64)>>,
    /// own records' bbox, texts included (the OVM cell row) / excluded
    pub dbox: Vec<BBox>,
    pub dbox_notext: Vec<BBox>,
    pub dmask: Vec<Vec<u8>>,
    pub dtmask: Vec<Vec<u8>>,
    pub dmembers: Vec<u64>,
    /// per layer_order index: own shape records / members over all cells
    pub lrecs: Vec<u64>,
    pub lmems: Vec<u64>,
    pub topo: Vec<usize>,
    pub rank: Vec<u32>,
    pub height: Vec<u32>,
    pub rmembers: Vec<u64>,
    pub rmask: Vec<Vec<u8>>,
    pub rtmask: Vec<Vec<u8>>,
}

/// reverse-topological (children-before-parents) cell order so the
/// recursive fold converges in ONE pass instead of the old O(depth)
/// fixpoint. Iterative post-order DFS. A back-edge (cyclic
/// hierarchy - invalid OASIS) is a HARD build error: the planner's
/// rank sweep and the fold both assume a DAG, and rank assignment
/// here is what guarantees it (VFS_HIER.md par.2.3).
pub fn topo_order(doc: &Doc) -> Vec<usize> {
    let n = doc.cells.len();
    let mut order = Vec::with_capacity(n);
    let mut state = vec![0u8; n]; // 0 unvisited, 1 on-stack, 2 done
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        state[start] = 1;
        stack.push((start, 0));
        while let Some(&(ci, k)) = stack.last() {
            if k < doc.cells[ci].places.len() {
                stack.last_mut().unwrap().1 = k + 1;
                let c = doc.cells[ci].places[k].cell;
                if state[c] == 0 {
                    state[c] = 1;
                    stack.push((c, 0));
                } else if state[c] == 1 {
                    panic!(
                        "cycle in hierarchy involving cell '{}' - \
                         invalid OASIS (placement graph must be \
                         acyclic); indexing refused",
                        doc.cells[c].name
                    );
                }
            } else {
                order.push(ci);
                state[ci] = 2;
                stack.pop();
            }
        }
    }
    order
}

impl PrePass {
    /// what build() derives from the parsed Doc before the first cell
    /// plan: the recursive bboxes, then the direct (own-record)
    /// masks / members / bbox per cell AND the per-layer geometry
    /// totals in ONE parallel record pass (texts count into masks and
    /// bboxes but not into members or the paged totals), then the
    /// recursive fold in one topological pass (children before
    /// parents): height, recursive members, recursive layer masks -
    /// identical to the old O(depth) fixpoint's converged values.
    pub fn from_doc(doc: &Doc, jobs: usize) -> PrePass {
        let n = doc.cells.len();
        eprintln!("[vfs] build: recursive bbox ({} cells)...", n);
        let tbb = std::time::Instant::now();
        let rbb = cell_bboxes_full(doc);
        eprintln!(
            "[vfs] build: recursive bbox {:.1}s (rss {})",
            tbb.elapsed().as_secs_f64(),
            crate::vfs::rss()
        );
        let nl = doc.layer_order.len();
        let lidx: HashMap<(u32, u32), usize> = doc
            .layer_order
            .iter()
            .enumerate()
            .map(|(i, &k)| (k, i))
            .collect();
        let bw = nl.div_ceil(8).max(1);
        type Slot = (Vec<u8>, Vec<u8>, u64, BBox, BBox);
        let dslots: Vec<std::sync::OnceLock<Slot>> =
            (0..n).map(|_| std::sync::OnceLock::new()).collect();
        let (lrecs, lmems) = {
            let td = std::time::Instant::now();
            let next = std::sync::atomic::AtomicUsize::new(0);
            let done = std::sync::atomic::AtomicUsize::new(0);
            let nthreads = jobs.max(1).min(n.max(1));
            let parts: Vec<(Vec<u64>, Vec<u64>)> = std::thread::scope(|s| {
                {
                    let done = &done;
                    s.spawn(move || {
                        use std::sync::atomic::Ordering::Relaxed;
                        let mut last = td;
                        loop {
                            std::thread::sleep(std::time::Duration::from_millis(200));
                            if done.load(Relaxed) >= n {
                                return;
                            }
                            if last.elapsed().as_secs_f64() >= 5.0 {
                                last = std::time::Instant::now();
                                eprintln!(
                                    "[vfs] build: direct masks {}/{} \
                                     ({}s, rss {})",
                                    done.load(Relaxed),
                                    n,
                                    td.elapsed().as_secs(),
                                    crate::vfs::rss()
                                );
                            }
                        }
                    });
                }
                let dslots = &dslots;
                let next = &next;
                let done = &done;
                let lidx = &lidx;
                (0..nthreads)
                    .map(|_| {
                        s.spawn(move || {
                            use std::sync::atomic::Ordering::Relaxed;
                            let mut lr = vec![0u64; nl];
                            let mut lm = vec![0u64; nl];
                            loop {
                                let ci = next.fetch_add(1, Relaxed);
                                if ci >= n {
                                    break;
                                }
                                let cell = &doc.cells[ci];
                                let mut mask = vec![0u8; bw];
                                let mut tmask = vec![0u8; bw];
                                let mut mems = 0u64;
                                let mut bx = BBox::EMPTY;
                                let mut bxn = BBox::EMPTY;
                                for r in &cell.rects {
                                    let li = lidx[&(r.layer, r.dt)];
                                    let (ex, ey) = rep_extent(&r.rep);
                                    floe_ovm::bit_set(&mut mask, li);
                                    let b = BBox {
                                        x0: r.x + ex.0.min(0),
                                        y0: r.y + ey.0.min(0),
                                        x1: r.x + r.w + ex.1.max(0),
                                        y1: r.y + r.h + ey.1.max(0),
                                    };
                                    bx.grow(&b);
                                    bxn.grow(&b);
                                    let m = r.rep.members();
                                    mems += m;
                                    lr[li] += 1;
                                    lm[li] += m;
                                }
                                for p in &cell.polys {
                                    let li = lidx[&(p.layer, p.dt)];
                                    let bb = crate::vfs::pts_bbox(&p.pts);
                                    let (ex, ey) = rep_extent(&p.rep);
                                    floe_ovm::bit_set(&mut mask, li);
                                    let b = BBox {
                                        x0: bb.x0 + ex.0.min(0),
                                        y0: bb.y0 + ey.0.min(0),
                                        x1: bb.x1 + ex.1.max(0),
                                        y1: bb.y1 + ey.1.max(0),
                                    };
                                    bx.grow(&b);
                                    bxn.grow(&b);
                                    let m = p.rep.members();
                                    mems += m;
                                    lr[li] += 1;
                                    lm[li] += m;
                                }
                                for pa in &cell.paths {
                                    let li = lidx[&(pa.layer, pa.dt)];
                                    let b4 = path_bbox(&pa.pts, pa.hw, pa.es, pa.ee);
                                    let (ex, ey) = rep_extent(&pa.rep);
                                    floe_ovm::bit_set(&mut mask, li);
                                    let b = BBox {
                                        x0: b4.0 + ex.0.min(0),
                                        y0: b4.1 + ey.0.min(0),
                                        x1: b4.2 + ex.1.max(0),
                                        y1: b4.3 + ey.1.max(0),
                                    };
                                    bx.grow(&b);
                                    bxn.grow(&b);
                                    let m = pa.rep.members();
                                    mems += m;
                                    lr[li] += 1;
                                    lm[li] += m;
                                }
                                for t in &cell.texts {
                                    // anchors count into bboxes/masks
                                    // (render culling must not drop
                                    // label-only subtrees) but not members
                                    let li = lidx[&(t.layer, t.dt)];
                                    let (ex, ey) = rep_extent(&t.rep);
                                    floe_ovm::bit_set(&mut mask, li);
                                    // text-only mask (v5): the label
                                    // walk prunes text-free subtrees
                                    // with its recursive fold
                                    floe_ovm::bit_set(&mut tmask, li);
                                    bx.grow(&BBox {
                                        x0: t.x + ex.0.min(0),
                                        y0: t.y + ey.0.min(0),
                                        x1: t.x + ex.1.max(0),
                                        y1: t.y + ey.1.max(0),
                                    });
                                }
                                let _ = dslots[ci].set((mask, tmask, mems, bx, bxn));
                                done.fetch_add(1, Relaxed);
                            }
                            (lr, lm)
                        })
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|h| h.join().expect("direct worker"))
                    .collect()
            });
            let mut lrecs = vec![0u64; nl];
            let mut lmems = vec![0u64; nl];
            for (lr, lm) in parts {
                for i in 0..nl {
                    lrecs[i] += lr[i];
                    lmems[i] += lm[i];
                }
            }
            (lrecs, lmems)
        };
        let mut dmask: Vec<Vec<u8>> = Vec::with_capacity(n);
        let mut dtmask: Vec<Vec<u8>> = Vec::with_capacity(n);
        let mut dmembers: Vec<u64> = Vec::with_capacity(n);
        let mut dbox: Vec<BBox> = Vec::with_capacity(n);
        let mut dbox_notext: Vec<BBox> = Vec::with_capacity(n);
        for slot in dslots {
            let (m, tm, mm, bx, bxn) = slot.into_inner().expect("direct slot unset");
            dmask.push(m);
            dtmask.push(tm);
            dmembers.push(mm);
            dbox.push(bx);
            dbox_notext.push(bxn);
        }

        let topo = topo_order(doc);
        // topo rank: parents-before-children (reverse post-order) for
        // the planner's min-heap sweep (VFS_HIER.md par.2.3) - every
        // edge gets parent.rank < child.rank, and rank assignment
        // covering all cells IS the DAG guarantee (cycles panicked in
        // topo_order).
        let mut rank = vec![0u32; n];
        for (i, &ci) in topo.iter().rev().enumerate() {
            rank[ci] = narrow_u32(i as u64, "topo rank");
        }
        let mut height = vec![0u32; n];
        let mut rmembers = vec![0u64; n];
        let mut rmask: Vec<Vec<u8>> = vec![Vec::new(); n];
        let mut rtmask: Vec<Vec<u8>> = vec![Vec::new(); n];
        for &ci in &topo {
            let mut hm = 0u32;
            let mut mm = dmembers[ci];
            let mut mask = dmask[ci].clone();
            let mut tmask = dtmask[ci].clone();
            for pl in &doc.cells[ci].places {
                let c = pl.cell;
                hm = hm.max(
                    height[c]
                        .checked_add(1)
                        .unwrap_or_else(|| panic!("limit exceeded: hierarchy depth")),
                );
                mm = mm.saturating_add(rmembers[c].saturating_mul(pl.rep.members()));
                for (a, b) in mask.iter_mut().zip(&rmask[c]) {
                    *a |= *b;
                }
                for (a, b) in tmask.iter_mut().zip(&rtmask[c]) {
                    *a |= *b;
                }
            }
            height[ci] = hm;
            rmembers[ci] = mm;
            rmask[ci] = mask;
            rtmask[ci] = tmask;
        }
        PrePass {
            rbb,
            dbox,
            dbox_notext,
            dmask,
            dtmask,
            dmembers,
            lrecs,
            lmems,
            topo,
            rank,
            height,
            rmembers,
            rmask,
            rtmask,
        }
    }

    /// the same values from the inventory pass (no Doc)
    pub fn from_inventory(inv: &Inventory) -> PrePass {
        fn bbox(b: &Box4) -> BBox {
            BBox { x0: b.0, y0: b.1, x1: b.2, y1: b.3 }
        }
        PrePass {
            rbb: inv.rbb.clone(),
            dbox: inv.cells.iter().map(|c| bbox(&c.dbox)).collect(),
            dbox_notext: inv.cells.iter().map(|c| bbox(&c.dbox_notext)).collect(),
            dmask: inv.cells.iter().map(|c| c.dmask.clone()).collect(),
            dtmask: inv.cells.iter().map(|c| c.dtmask.clone()).collect(),
            dmembers: inv.cells.iter().map(|c| c.dmembers).collect(),
            lrecs: inv.lrecs.clone(),
            lmems: inv.lmems.clone(),
            topo: inv.topo.clone(),
            rank: inv.rank.clone(),
            height: inv.height.clone(),
            rmembers: inv.rmembers.clone(),
            rmask: inv.rmask.clone(),
            rtmask: inv.rtmask.clone(),
        }
    }
}

/// Counts the normalization pass changes (a point-list repetition of
/// 16+ members splits into grid records): taken from the Doc between
/// the syntax parse and `normalize_reps` (the parse hook), where the
/// inventory's record-level counts are comparable.
pub struct PreNorm {
    /// per layer_order index: own shape records
    pub lrecs: Vec<u64>,
    /// per cell: (child, placement records) in first-placement order
    pub children: Vec<Vec<(usize, u64)>>,
    pub texts: Vec<u64>,
    pub places: Vec<u64>,
}

impl PreNorm {
    pub fn from_doc(doc: &Doc) -> PreNorm {
        let lidx: HashMap<(u32, u32), usize> = doc
            .layer_order
            .iter()
            .enumerate()
            .map(|(i, &k)| (k, i))
            .collect();
        let mut lrecs = vec![0u64; doc.layer_order.len()];
        let mut children = Vec::with_capacity(doc.cells.len());
        let mut texts = Vec::with_capacity(doc.cells.len());
        let mut places = Vec::with_capacity(doc.cells.len());
        for cell in &doc.cells {
            for r in &cell.rects {
                lrecs[lidx[&(r.layer, r.dt)]] += 1;
            }
            for p in &cell.polys {
                lrecs[lidx[&(p.layer, p.dt)]] += 1;
            }
            for p in &cell.paths {
                lrecs[lidx[&(p.layer, p.dt)]] += 1;
            }
            let mut kids: Vec<(usize, u64)> = Vec::new();
            for pl in &cell.places {
                match kids.iter_mut().find(|k| k.0 == pl.cell) {
                    Some(k) => k.1 += 1,
                    None => kids.push((pl.cell, 1)),
                }
            }
            children.push(kids);
            texts.push(cell.texts.len() as u64);
            places.push(cell.places.len() as u64);
        }
        PreNorm { lrecs, children, texts, places }
    }
}

/// the Doc's edges as the inventory defines them: distinct children in
/// first-placement order, the member sum, the per-orientation union of
/// the placement anchors widened by their repetition extents
fn doc_edges(doc: &Doc, ci: usize) -> Vec<Edge> {
    let mut edges: Vec<Edge> = Vec::new();
    for pl in &doc.cells[ci].places {
        let (ex, ey) = rep_extent(&pl.rep);
        let o = (pl.rot & 3) as usize | ((pl.flip as usize) << 2);
        let bb: Box4 = (
            pl.x + ex.0.min(0),
            pl.y + ey.0.min(0),
            pl.x + ex.1.max(0),
            pl.y + ey.1.max(0),
        );
        let m = pl.rep.members();
        match edges.iter_mut().find(|e| e.child == pl.cell) {
            Some(e) => {
                e.placements += 1;
                e.members = e.members.saturating_add(m);
                floe_oasis::geom::box_grow(&mut e.obox[o], &bb);
            }
            None => {
                let mut obox = [EMPTY_BOX; 8];
                obox[o] = bb;
                edges.push(Edge { child: pl.cell, placements: 1, members: m, obox });
            }
        }
    }
    edges
}

fn b4(b: &BBox) -> (i64, i64, i64, i64) {
    (b.x0, b.y0, b.x1, b.y1)
}

/// G1: every value the build takes from the Doc pre-passes, the Doc's
/// cell and layer tables, the top, the edges and the no-text recursive
/// boxes, against the inventory. Returns the mismatches (empty = equal).
pub fn check(
    doc: &Doc,
    pre: &PrePass,
    prenorm: Option<&PreNorm>,
    inv: &Inventory,
) -> Vec<String> {
    const CAP: usize = 60;
    let mut out: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut bad = |msg: String| {
        total += 1;
        if out.len() < CAP {
            out.push(msg);
        }
    };
    if doc.cells.len() != inv.cells.len() {
        bad(format!("cells: doc {} vs inventory {}", doc.cells.len(), inv.cells.len()));
    }
    let n = doc.cells.len().min(inv.cells.len());
    for ci in 0..n {
        if doc.cells[ci].name != inv.cells[ci].name {
            bad(format!(
                "cell {}: name doc {:?} vs inventory {:?}",
                ci, doc.cells[ci].name, inv.cells[ci].name
            ));
        }
    }
    if doc.top != inv.top {
        bad(format!("top: doc {} vs inventory {}", doc.top, inv.top));
    }
    if doc.unit != inv.unit() {
        bad(format!("unit: doc {} vs inventory {}", doc.unit, inv.unit()));
    }
    if doc.layer_order != inv.layer_order {
        bad(format!(
            "layer_order: doc {:?} vs inventory {:?}",
            doc.layer_order, inv.layer_order
        ));
    }
    if doc.layer_names != inv.layer_names {
        bad("layer_names differ".to_string());
    }
    if doc.layer_aliases != inv.layer_aliases {
        bad("layer_aliases differ".to_string());
    }
    let ipre = PrePass::from_inventory(inv);
    if pre.lmems != ipre.lmems {
        bad(format!("lmems: doc {:?} vs inventory {:?}", pre.lmems, ipre.lmems));
    }
    if let Some(pn) = prenorm {
        if pn.lrecs != ipre.lrecs {
            bad(format!("lrecs: doc {:?} vs inventory {:?}", pn.lrecs, ipre.lrecs));
        }
    }
    if pre.topo != ipre.topo {
        bad("topo order differs".to_string());
    }
    let rbbn = cell_bboxes(doc);
    for ci in 0..n {
        let nm = &doc.cells[ci].name;
        macro_rules! same {
            ($what:expr, $a:expr, $b:expr) => {
                if $a != $b {
                    bad(format!("cell {} ({}): {} doc {:?} vs inventory {:?}", ci, nm, $what, $a, $b));
                }
            };
        }
        same!("rbb", pre.rbb[ci], ipre.rbb[ci]);
        same!("rbb_notext", rbbn[ci], inv.rbb_notext[ci]);
        same!("dbox", b4(&pre.dbox[ci]), b4(&ipre.dbox[ci]));
        same!("dbox_notext", b4(&pre.dbox_notext[ci]), b4(&ipre.dbox_notext[ci]));
        same!("dmembers", pre.dmembers[ci], ipre.dmembers[ci]);
        same!("dmask", pre.dmask[ci], ipre.dmask[ci]);
        same!("dtmask", pre.dtmask[ci], ipre.dtmask[ci]);
        same!("rank", pre.rank[ci], ipre.rank[ci]);
        same!("height", pre.height[ci], ipre.height[ci]);
        same!("rmembers", pre.rmembers[ci], ipre.rmembers[ci]);
        same!("rmask", pre.rmask[ci], ipre.rmask[ci]);
        same!("rtmask", pre.rtmask[ci], ipre.rtmask[ci]);
        let de = doc_edges(doc, ci);
        let ie = &inv.cells[ci].edges;
        let dk: Vec<usize> = de.iter().map(|e| e.child).collect();
        let ik: Vec<usize> = ie.iter().map(|e| e.child).collect();
        same!("children", dk, ik);
        for (a, b) in de.iter().zip(ie.iter()) {
            if a.child != b.child {
                break;
            }
            same!(format!("edge {} members", a.child), a.members, b.members);
            same!(format!("edge {} obox", a.child), a.obox, b.obox);
        }
        if let Some(pn) = prenorm {
            let pk: Vec<(usize, u64)> = pn.children[ci].clone();
            let ikp: Vec<(usize, u64)> = ie.iter().map(|e| (e.child, e.placements)).collect();
            same!("edge placements", pk, ikp);
            same!("texts", pn.texts[ci], inv.cells[ci].texts);
            same!("placements", pn.places[ci], inv.cells[ci].placements);
        }
        let empty_doc = pre.rbb[ci].is_none();
        let empty_inv = box_is_empty(&inv.cells[ci].dbox) && inv.cells[ci].edges.iter().all(|e| inv.rbb[e.child].is_none());
        same!("emptiness", empty_doc, empty_inv);
    }
    if total > out.len() {
        out.push(format!("... {} more", total - out.len()));
    }
    out
}

/// what differs between a parsed cell and the Doc's, briefly
fn describe_cell_diff(ci: usize, a: &floe_oasis::doc::Cell, b: &floe_oasis::doc::Cell) -> String {
    fn short(s: String) -> String {
        if s.len() > 240 {
            let cut = s.char_indices().nth(240).map(|(i, _)| i).unwrap_or(s.len());
            format!("{}...", &s[..cut])
        } else {
            s
        }
    }
    if a.name != b.name {
        return format!("cell {}: name {:?} vs doc {:?}", ci, a.name, b.name);
    }
    macro_rules! vecdiff {
        ($f:ident) => {
            if a.$f.len() != b.$f.len() {
                return format!(
                    "cell {} ({}): {} count {} vs doc {}",
                    ci,
                    b.name,
                    stringify!($f),
                    a.$f.len(),
                    b.$f.len()
                );
            }
            if let Some(i) = a.$f.iter().zip(&b.$f).position(|(x, y)| x != y) {
                return short(format!(
                    "cell {} ({}): {}[{}] {:?} vs doc {:?}",
                    ci,
                    b.name,
                    stringify!($f),
                    i,
                    a.$f[i],
                    b.$f[i]
                ));
            }
        };
    }
    vecdiff!(rects);
    vecdiff!(polys);
    vecdiff!(paths);
    vecdiff!(places);
    vecdiff!(texts);
    format!("cell {} ({}): differs", ci, b.name)
}

/// G2: every range of the file through the cell parser (the head, then
/// the cells' ranges in file order), the pieces of a cell joined in file
/// order and normalized on their own, against the whole-Doc parse's
/// cells record by record. Returns the mismatches and the piece count.
pub fn check_cells(data: &[u8], doc: &Doc, inv: &Inventory, jobs: usize) -> (Vec<String>, usize) {
    use floe_oasis::doc::{normalize_cells, parse_cell_range, Cell, CellPiece, CellTables};
    let cix = inv.cell_index();
    let tables = CellTables::new(&inv.cellnames, &inv.textstrings, &cix);
    let mut ranges: Vec<&floe_oasis::inventory::CellRange> = vec![&inv.head];
    for c in &inv.cells {
        for r in &c.ranges {
            ranges.push(r);
        }
    }
    ranges.sort_by_key(|r| r.start);
    let n = inv.cells.len();
    let threads = jobs.max(1);
    let per = ranges.len().div_ceil(threads).max(1);
    let parsed: Vec<Result<Vec<Vec<CellPiece>>, String>> = std::thread::scope(|s| {
        let tables = &tables;
        let ranges = &ranges;
        let handles: Vec<_> = (0..ranges.len())
            .step_by(per)
            .map(|i| {
                s.spawn(move || {
                    let mut out = Vec::new();
                    for r in &ranges[i..(i + per).min(ranges.len())] {
                        out.push(parse_cell_range(data, r, tables).map_err(|e| {
                            format!("range {}..{}: {}", r.start, r.end, e)
                        })?);
                    }
                    Ok(out)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("range parser"))
            .collect()
    });
    let mut pieces: Vec<Vec<Cell>> = (0..n).map(|_| Vec::new()).collect();
    let mut n_pieces = 0usize;
    let mut diffs: Vec<String> = Vec::new();
    for r in parsed {
        match r {
            Ok(per_range) => {
                for ps in per_range {
                    for p in ps {
                        if p.index >= n {
                            diffs.push(format!("piece for cell index {} beyond the inventory", p.index));
                            continue;
                        }
                        pieces[p.index].push(p.cell);
                        n_pieces += 1;
                    }
                }
            }
            Err(e) => diffs.push(e),
        }
    }
    if !diffs.is_empty() {
        return (diffs, n_pieces);
    }
    // a defined cell has pieces, an undefined target none (a range folds
    // the CELL records of one cell into one piece, so the piece count is
    // not the definition count; the record comparison below is the test)
    for (ci, ps) in pieces.iter().enumerate() {
        if ps.is_empty() != inv.cells[ci].def_ranges.is_empty() {
            diffs.push(format!(
                "cell {} ({}): {} pieces from {} defining range(s)",
                ci,
                inv.cells[ci].name,
                ps.len(),
                inv.cells[ci].def_ranges.len()
            ));
        }
    }
    let mut cells: Vec<Cell> = pieces
        .into_iter()
        .enumerate()
        .map(|(ci, ps)| {
            let mut cell = Cell { name: inv.cells[ci].name.clone(), ..Default::default() };
            for p in ps {
                cell.rects.extend(p.rects);
                cell.polys.extend(p.polys);
                cell.paths.extend(p.paths);
                cell.places.extend(p.places);
                cell.texts.extend(p.texts);
            }
            cell
        })
        .collect();
    let per_cells = n.div_ceil(threads).max(1);
    let found: Vec<String> = std::thread::scope(|s| {
        let handles: Vec<_> = cells
            .chunks_mut(per_cells)
            .enumerate()
            .map(|(k, chunk)| {
                s.spawn(move || {
                    let mut d = Vec::new();
                    for (j, cell) in chunk.iter_mut().enumerate() {
                        let ci = k * per_cells + j;
                        normalize_cells(std::slice::from_mut(cell), 1);
                        if ci >= doc.cells.len() {
                            d.push(format!("cell {}: beyond the Doc", ci));
                        } else if *cell != doc.cells[ci] {
                            d.push(describe_cell_diff(ci, cell, &doc.cells[ci]));
                        }
                    }
                    d
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("cell check"))
            .collect()
    });
    diffs.extend(found);
    if doc.cells.len() != n {
        diffs.push(format!("cells: doc {} vs inventory {}", doc.cells.len(), n));
    }
    (diffs, n_pieces)
}

/// `floe-index inventory <file.oas> [--jobs N] [--check] [--check-cells] [--cells]`:
/// the inventory pass on a source - the cell table, the layer order,
/// the hierarchy fold - as JSON, and with `--check` the Doc pre-passes
/// beside it, held equal cell by cell (G1); `--check-cells` adds the
/// cell parser: every range through parse_cell_range + normalize_cells
/// against the Doc's cells record by record (G2). Exit 1 on a difference.
pub fn inventory_cmd(args: &[String]) {
    let mut src: Option<String> = None;
    let mut jobs = 1usize;
    let mut do_check = false;
    let mut do_check_cells = false;
    let mut cells = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--jobs" => {
                jobs = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or_else(|| {
                    eprintln!("--jobs needs a number");
                    std::process::exit(2);
                });
                i += 1;
            }
            "--check" => do_check = true,
            "--check-cells" => {
                do_check = true;
                do_check_cells = true;
            }
            "--cells" => cells = true,
            a if a.starts_with("--") => {
                eprintln!("inventory: unknown option {}", a);
                std::process::exit(2);
            }
            a => src = Some(a.to_string()),
        }
        i += 1;
    }
    let src = match src {
        Some(s) => s,
        None => {
            eprintln!("usage: floe-index inventory <file.oas> [--jobs N] [--check] [--check-cells] [--cells]");
            std::process::exit(2);
        }
    };
    let t0 = std::time::Instant::now();
    let data = match std::fs::read(&src) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("read {}: {}", src, e);
            std::process::exit(1);
        }
    };
    let read_s = t0.elapsed().as_secs_f64();
    let t1 = std::time::Instant::now();
    let inv = match floe_oasis::inventory(&data, jobs) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("inventory {}: {}", src, e);
            std::process::exit(1);
        }
    };
    let inv_s = t1.elapsed().as_secs_f64();
    let n = inv.cells.len();
    let edges: usize = inv.cells.iter().map(|c| c.edges.len()).sum();
    let big = inv.cells.iter().max_by_key(|c| c.doc_bytes);
    let height_max = inv.height.iter().copied().max().unwrap_or(0);
    let fmt_box = |b: &Option<Box4>| match b {
        Some(b) => format!("[{}, {}, {}, {}]", b.0, b.1, b.2, b.3),
        None => "null".to_string(),
    };
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str(&format!("  \"file_bytes\": {},\n", inv.scan.file_bytes));
    out.push_str(&format!("  \"unit\": {},\n", inv.unit()));
    out.push_str(&format!("  \"records\": {},\n", inv.scan.records));
    out.push_str(&format!("  \"cells\": {},\n", n));
    out.push_str(&format!("  \"cell_records\": {},\n", inv.scan.cells));
    out.push_str(&format!("  \"layers\": {},\n", inv.layer_order.len()));
    out.push_str(&format!("  \"top\": {},\n", crate::json_str(&inv.cells[inv.top].name)));
    out.push_str(&format!("  \"top_index\": {},\n", inv.top));
    out.push_str(&format!("  \"bbox\": {},\n", fmt_box(&inv.rbb[inv.top])));
    out.push_str(&format!("  \"bbox_notext\": {},\n", fmt_box(&inv.rbb_notext[inv.top])));
    out.push_str(&format!("  \"placements\": {},\n", inv.scan.placements));
    out.push_str(&format!("  \"placement_members\": {},\n", inv.scan.placement_members));
    out.push_str(&format!("  \"edges\": {},\n", edges));
    out.push_str(&format!("  \"height\": {},\n", height_max));
    out.push_str(&format!("  \"top_members\": {},\n", inv.rmembers[inv.top]));
    out.push_str(&format!("  \"cblocks\": {},\n", inv.scan.cblocks));
    out.push_str(&format!("  \"doc_bytes_est\": {},\n", inv.scan.doc_bytes));
    out.push_str(&format!("  \"doc_bytes_est_pow2\": {},\n", inv.scan.doc_bytes_pow2));
    if let Some(c) = big {
        out.push_str(&format!(
            "  \"largest_cell\": {{\"cell\": {}, \"records\": {}, \"doc_bytes\": {}}},\n",
            crate::json_str(&c.name),
            c.records,
            c.doc_bytes
        ));
    }
    if cells {
        out.push_str("  \"cells_detail\": [");
        for (ci, c) in inv.cells.iter().enumerate() {
            if ci > 0 {
                out.push(',');
            }
            let kids: Vec<String> = c.edges.iter().map(|e| e.child.to_string()).collect();
            out.push_str(&format!(
                "\n    {{\"index\": {}, \"cell\": {}, \"records\": {}, \"placements\": {}, \
                 \"texts\": {}, \"doc_bytes\": {}, \"ranges\": [{}], \"dbox\": {}, \
                 \"rbb\": {}, \"rank\": {}, \"height\": {}, \"rmembers\": {}, \
                 \"children\": [{}]}}",
                ci,
                crate::json_str(&c.name),
                c.records,
                c.placements,
                c.texts,
                c.doc_bytes,
                c.ranges
                    .iter()
                    .map(|r| format!("[{}, {}]", r.start, r.end))
                    .collect::<Vec<_>>()
                    .join(", "),
                fmt_box(&(!box_is_empty(&c.dbox)).then_some(c.dbox)),
                fmt_box(&inv.rbb[ci]),
                inv.rank[ci],
                inv.height[ci],
                inv.rmembers[ci],
                kids.join(", ")
            ));
        }
        out.push_str("\n  ],\n");
    }
    let mut ok = true;
    if do_check {
        let t2 = std::time::Instant::now();
        let mut prenorm: Option<PreNorm> = None;
        let doc = match floe_oasis::doc::parse_doc_parallel_phased(&data, jobs, |d| {
            prenorm = Some(PreNorm::from_doc(d));
        }) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("parse {}: {}", src, e);
                std::process::exit(1);
            }
        };
        let pre = PrePass::from_doc(&doc, jobs);
        let diffs = check(&doc, &pre, prenorm.as_ref(), &inv);
        let doc_s = t2.elapsed().as_secs_f64();
        ok = diffs.is_empty();
        out.push_str(&format!(
            "  \"check\": {{\"ok\": {}, \"doc_s\": {:.3}, \"mismatches\": [{}]}},\n",
            ok,
            doc_s,
            diffs
                .iter()
                .map(|d| crate::json_str(d))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for d in &diffs {
            eprintln!("inventory check: {}", d);
        }
        eprintln!(
            "inventory check: {} ({} cells, {} layers; inventory {:.1}s, doc pre-pass {:.1}s)",
            if ok { "OK" } else { "MISMATCH" },
            n,
            inv.layer_order.len(),
            inv_s,
            doc_s
        );
        if do_check_cells {
            let t3 = std::time::Instant::now();
            let (diffs2, pieces) = check_cells(&data, &doc, &inv, jobs);
            let cells_s = t3.elapsed().as_secs_f64();
            let ok2 = diffs2.is_empty();
            ok = ok && ok2;
            out.push_str(&format!(
                "  \"check_cells\": {{\"ok\": {}, \"ranges\": {}, \"pieces\": {}, \"parse_s\": {:.3}, \"mismatches\": [{}]}},\n",
                ok2,
                1 + inv.cells.iter().map(|c| c.ranges.len()).sum::<usize>(),
                pieces,
                cells_s,
                diffs2
                    .iter()
                    .map(|d| crate::json_str(d))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            for d in &diffs2 {
                eprintln!("cell check: {}", d);
            }
            eprintln!(
                "cell check: {} ({} cells from {} pieces; parse + normalize + compare {:.1}s)",
                if ok2 { "OK" } else { "MISMATCH" },
                n,
                pieces,
                cells_s
            );
        }
    }
    out.push_str(&format!(
        "  \"read_s\": {:.3},\n  \"inventory_s\": {:.3},\n  \"inventory_mb_s\": {:.1}\n}}",
        read_s,
        inv_s,
        inv.scan.file_bytes as f64 / 1e6 / inv_s.max(1e-9)
    ));
    println!("{}", out);
    eprintln!(
        "inventory: {} records in {} cells ({} layers, height {}); top {} {}; \
         Doc about {} + source {}; largest cell {}: {} records, {}",
        inv.scan.records,
        n,
        inv.layer_order.len(),
        height_max,
        inv.cells[inv.top].name,
        fmt_box(&inv.rbb[inv.top]),
        crate::fmt_size(inv.scan.doc_bytes),
        crate::fmt_size(inv.scan.file_bytes),
        big.map(|c| c.name.as_str()).unwrap_or("-"),
        big.map(|c| c.records).unwrap_or(0),
        crate::fmt_size(big.map(|c| c.doc_bytes).unwrap_or(0))
    );
    if !ok {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_oasis::doc::{parse_doc_parallel_phased, PathRec, PlaceRec, PolyRec, RectRec, Rep, TextRec};
    use floe_oasis::inventory::fixtures;
    use floe_oasis::write::{write_tree, WCell};

    fn check_file(data: &[u8], jobs: usize) {
        let mut prenorm = None;
        let doc = parse_doc_parallel_phased(data, jobs, |d| {
            prenorm = Some(PreNorm::from_doc(d));
        })
        .expect("parse");
        let pre = PrePass::from_doc(&doc, jobs);
        let inv = floe_oasis::inventory(data, jobs).expect("inventory");
        let diffs = check(&doc, &pre, prenorm.as_ref(), &inv);
        assert!(diffs.is_empty(), "jobs {}: {:#?}", jobs, diffs);
        // the fold's own cross-check against the tiler's functions
        assert_eq!(inv.rbb, cell_bboxes_full(&doc));
        assert_eq!(inv.rbb_notext, cell_bboxes(&doc));
        assert_eq!(inv.topo, topo_order(&doc));
        // G2: the cell parser + per-cell normalization against the Doc
        let (diffs2, pieces) = check_cells(data, &doc, &inv, jobs);
        assert!(diffs2.is_empty(), "G2 jobs {}: {:#?}", jobs, diffs2);
        let defined = inv.cells.iter().filter(|c| !c.def_ranges.is_empty()).count();
        assert!(pieces >= defined, "{} pieces for {} defined cells", pieces, defined);
    }

    /// the hand-encoded adversarial stream (floe_oasis::inventory
    /// fixtures) against the real Doc pre-passes, every table
    /// placement and CBLOCK variant, sequential and split
    #[test]
    fn adversarial_fixture_matches_doc_prepass() {
        for (tables_first, cblock) in [(false, false), (true, false), (false, true), (true, true)] {
            let data = fixtures::adversarial(tables_first, cblock);
            for jobs in [1, 2, 3] {
                check_file(&data, jobs);
            }
        }
    }

    /// a write_tree hierarchy: every record kind the writer emits
    /// (modal XYRELATIVE coordinates, grouped repetitions re-referenced
    /// as type 0, CBLOCK bodies when they pay), rotated / mirrored
    /// placements with grid and point-list repetitions, texts, paths
    #[test]
    fn written_tree_matches_doc_prepass() {
        let pts: Vec<(i64, i64)> = (0..40).map(|i| (i * 37 % 1000, (i * 91) % 700)).collect();
        // repetitions normalize_reps splits (16+ members holding a grid):
        // a 4 x 4 lattice at pitch 10 plus an outlier, and the same with
        // a duplicate member and a far outlier - on rects and on
        // placements, so the post-normalization Doc (grid parts with
        // shifted anchors + leftover lists) meets the record-level
        // inventory in check()
        let mut grid16: Vec<(i64, i64)> = Vec::new();
        for j in 0..4 {
            for i in 0..4 {
                grid16.push((i * 10, j * 10));
            }
        }
        let mut split_a = grid16.clone();
        split_a.push((1000, 1));
        let split_a = Rep::Pts(split_a.into());
        let mut split_b = grid16.clone();
        split_b.push((10, 10));
        split_b.push((-500, 7));
        let split_b = Rep::Pts(split_b.into());
        let mut leaf_rects: Vec<RectRec> = (0..200)
            .map(|i| RectRec {
                layer: 1 + (i % 3) as u32,
                dt: 0,
                x: (i * 13) % 500,
                y: (i * 29) % 400,
                w: 10 + (i % 7),
                h: 10 + (i % 5),
                rep: match i % 4 {
                    0 => Rep::One,
                    1 => Rep::Grid { na: 3, nb: 2, va: (60, 0), vb: (0, 70) },
                    2 => Rep::Grid { na: 4, nb: 1, va: (15, 25), vb: (0, 0) },
                    _ => Rep::Pts(pts.clone().into()),
                },
            })
            .collect();
        leaf_rects.push(RectRec { layer: 7, dt: 0, x: 900, y: 900, w: 4, h: 4, rep: split_a.clone() });
        leaf_rects.push(RectRec { layer: 7, dt: 0, x: 950, y: 900, w: 4, h: 4, rep: split_b.clone() });
        let leaf_polys = vec![
            PolyRec { layer: 4, dt: 1, pts: vec![(0, 0), (30, 0), (30, 20), (10, 40)], rep: Rep::One },
            PolyRec { layer: 4, dt: 1, pts: vec![(100, 100), (160, 100), (130, 150)], rep: Rep::Grid { na: 2, nb: 3, va: (90, 0), vb: (0, 80) } },
        ];
        let leaf_paths = vec![
            PathRec { layer: 5, dt: 0, pts: vec![(0, 500), (200, 500), (200, 700)], hw: 8, es: 8, ee: 0, rep: Rep::One },
            PathRec { layer: 5, dt: 0, pts: vec![(300, 500), (380, 580)], hw: 5, es: 0, ee: 5, rep: Rep::Grid { na: 3, nb: 1, va: (0, 150), vb: (0, 0) } },
        ];
        let leaf_texts = vec![TextRec { layer: 63, dt: 63, x: -20, y: -30, rep: Rep::One, s: "L".to_string() }];
        let mid_texts = vec![TextRec { layer: 63, dt: 63, x: 5000, y: 5000, rep: Rep::Grid { na: 2, nb: 2, va: (100, 0), vb: (0, 100) }, s: "M".to_string() }];
        let one = Rep::One;
        let arr = Rep::Grid { na: 5, nb: 4, va: (1200, 0), vb: (0, 900) };
        let lst = Rep::Pts(vec![(0, 0), (3000, 100), (-200, 2500), (3100, 2600)].into());
        let mid = WCell {
            name: "MID".to_string(),
            rects: &[],
            polys: &[],
            paths: &[],
            texts: &mid_texts,
            places: vec![
                ("LEAF", 0, 0, 0, false, &one),
                ("LEAF", 700, 0, 1, false, &arr),
                ("LEAF", 0, 900, 2, true, &lst),
                ("LEAF", -4000, -4000, 3, true, &one),
                ("LEAF2", 100, 100, 0, true, &one),
                ("LEAF2", 3000, 3000, 1, false, &split_a),
                ("LEAF2", -3000, 500, 2, true, &split_b),
            ],
        };
        let leaf2_rects = vec![RectRec { layer: 9, dt: 0, x: 0, y: 0, w: 5, h: 5, rep: Rep::One }];
        let leaf2 = WCell { name: "LEAF2".to_string(), rects: &leaf2_rects, polys: &[], paths: &[], texts: &[], places: vec![] };
        let leaf = WCell { name: "LEAF".to_string(), rects: &leaf_rects, polys: &leaf_polys, paths: &leaf_paths, texts: &leaf_texts, places: vec![] };
        let top_rects = vec![RectRec { layer: 2, dt: 0, x: -100, y: -100, w: 50000, h: 50000, rep: Rep::One }];
        let top = WCell {
            name: "TOP".to_string(),
            rects: &top_rects,
            polys: &[],
            paths: &[],
            texts: &[],
            places: vec![
                ("MID", 0, 0, 0, false, &one),
                ("MID", 20000, 0, 1, true, &Rep::Grid { na: 2, nb: 1, va: (0, 12000), vb: (0, 0) }),
                ("LEAF", 40000, 40000, 2, false, &one),
            ],
        };
        // definition order: the top last (placement targets may be
        // defined later in the stream)
        let data = write_tree(&[leaf, mid, leaf2, top], 1000.0).expect("write_tree");
        for jobs in [1, 2, 4] {
            check_file(&data, jobs);
        }
        // the split repetitions did split: the normalized Doc holds more
        // rect and placement records than the stream spelled out
        let mut prenorm = None;
        let doc = parse_doc_parallel_phased(&data, 1, |d| {
            prenorm = Some(PreNorm::from_doc(d));
        })
        .expect("parse");
        let pn = prenorm.unwrap();
        let leaf_i = doc.cells.iter().position(|c| c.name == "LEAF").unwrap();
        let mid_i = doc.cells.iter().position(|c| c.name == "MID").unwrap();
        assert!(doc.cells[leaf_i].rects.len() > 202, "rect reps did not split");
        assert!(doc.cells[mid_i].places.len() > pn.places[mid_i] as usize, "placement reps did not split");
    }

    /// PlaceRec is the Doc's record; the written tree above keeps the
    /// writer's own placement order, which this pins for the test's
    /// benefit (a change in write_tree's order is not a parity bug)
    #[test]
    fn written_tree_placement_order_is_the_writers() {
        let one = Rep::One;
        let cell = WCell { name: "A".to_string(), rects: &[], polys: &[], paths: &[], texts: &[], places: vec![("B", 0, 0, 0, false, &one), ("C", 1, 1, 0, false, &one), ("B", 2, 2, 0, false, &one)] };
        let b_rects = vec![RectRec { layer: 1, dt: 0, x: 0, y: 0, w: 1, h: 1, rep: Rep::One }];
        let b = WCell { name: "B".to_string(), rects: &b_rects, polys: &[], paths: &[], texts: &[], places: vec![] };
        let c = WCell { name: "C".to_string(), rects: &b_rects, polys: &[], paths: &[], texts: &[], places: vec![] };
        let data = write_tree(&[cell, b, c], 1000.0).expect("write_tree");
        let inv = floe_oasis::inventory(&data, 1).expect("inventory");
        let doc = floe_oasis::doc::parse_doc(&data).expect("parse");
        let order: Vec<usize> = doc.cells[0].places.iter().map(|p: &PlaceRec| p.cell).collect();
        let kids: Vec<usize> = inv.cells[0].edges.iter().map(|e| e.child).collect();
        let mut first: Vec<usize> = Vec::new();
        for c in order {
            if !first.contains(&c) {
                first.push(c);
            }
        }
        assert_eq!(kids, first);
    }
}
