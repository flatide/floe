//! VFS V1: `vfs` builds <src>.floe/design.{ovm,ovp}, `plan`
//! simulates a viewport against the metadata without loading any
//! geometry (rust/VFS.md).
//!
//! Pages are CELL-LOCAL and unclipped - no tile cutting, no
//! variants, no per-tile rep fragmentation. Page payload = a
//! complete single-cell OASIS file via write_tree (CBLOCK-6,
//! conditional rep-group order), so decode is parse_doc and klayout
//! can validate pages directly. Texts stay OUT of pages (sidecar/
//! skeleton own them), which makes the G5 gate exact: page record/
//! member sums must equal the source's geometry scan.

use floe_oasis::doc::{Doc, PathRec, PolyRec, RectRec, Rep};
use floe_oasis::write::{write_tree, WCell};
use floe_ovm::{BBox, Builder, Ovm};
use floe_tiler::hier::{cell_bboxes_full, rep_extent};
use floe_tiler::{path_bbox, xf_rep, Xf};
use std::io::Write;

const PAGE_TARGET_BYTES: usize = 1 << 20; // pre-encode estimate
const PAGE_MIN_RECORDS: usize = 64;
const BVH_LEAF: usize = 8;

// ------------------------------------------------------------ build

pub fn vfs_cmd(args: &[String]) {
    let mut src: Option<String> = None;
    let mut outdir: Option<String> = None;
    let mut jobs: Option<usize> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--jobs" => {
                jobs = Some(args[i + 1].parse().expect("jobs"));
                i += 2;
            }
            a => {
                if src.is_none() {
                    src = Some(a.to_string());
                } else {
                    outdir = Some(a.to_string());
                }
                i += 1;
            }
        }
    }
    let src = src.expect("src");
    let outdir = outdir.unwrap_or_else(|| format!("{}.floe", src));
    let jobs = jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    });
    let t0 = std::time::Instant::now();
    eprintln!("[vfs] reading {}...", src);
    let data = std::fs::read(&src).expect("read src");
    let size = data.len() as u64;
    let mtime = std::fs::metadata(&src)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH).ok()
        })
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let t1 = std::time::Instant::now();
    let doc = match floe_oasis::doc::parse_doc_parallel(&data, jobs) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("parse {}: {}", src, e);
            std::process::exit(1);
        }
    };
    drop(data);
    eprintln!(
        "[vfs] parsed {} cells in {:.1}s ({} threads, \
         grid-normalize {:.1}s)",
        doc.cells.len(),
        t1.elapsed().as_secs_f64(),
        jobs,
        doc.norm_s
    );
    std::fs::create_dir_all(&outdir).expect("mkdir outdir");
    build(&doc, size, mtime, &outdir);
    eprintln!(
        "[vfs] done in {:.1}s -> {}",
        t0.elapsed().as_secs_f64(),
        outdir
    );
}

fn win_to_bbox(w: Option<(i64, i64, i64, i64)>) -> BBox {
    match w {
        None => BBox::EMPTY,
        Some((x0, y0, x1, y1)) => BBox { x0, y0, x1, y1 },
    }
}

/// per-record page item: kind 0 rect / 1 poly / 2 path
struct PRec {
    kind: u8,
    idx: u32,
    bbox: BBox, // incl rep extent (page bbox / spatial split)
    bytes: u32, // pre-encode estimate
    members: u64,
    w: i64, // single-feature dims (cut criterion, rep excluded)
    h: i64,
}

fn rep_est(rep: &Rep) -> u32 {
    match rep {
        Rep::One => 0,
        Rep::Grid { .. } => 10,
        Rep::Pts(p) => 4 * p.len() as u32,
    }
}

fn build(doc: &Doc, size: u64, mtime: u64, outdir: &str) {
    let t0 = std::time::Instant::now();
    let n = doc.cells.len();
    let rbb = cell_bboxes_full(doc);
    let nl = doc.layer_order.len();
    let lidx: std::collections::HashMap<(u32, u32), usize> = doc
        .layer_order
        .iter()
        .enumerate()
        .map(|(i, &k)| (k, i))
        .collect();

    // fixpoint passes over the DAG: heights, recursive members,
    // masks (children are not guaranteed to precede parents)
    let bw = nl.div_ceil(8).max(1);
    let mut dmask = vec![vec![0u8; bw]; n];
    let mut dmembers = vec![0u64; n];
    let mut dbox = vec![BBox::EMPTY; n];
    for (ci, cell) in doc.cells.iter().enumerate() {
        let mut grow = |b: &mut BBox,
                        li: usize,
                        bb: BBox,
                        mems: u64,
                        mask: &mut [u8]| {
            floe_ovm::bit_set(mask, li);
            b.grow(&bb);
            dmembers[ci] += mems;
        };
        for r in &cell.rects {
            let li = lidx[&(r.layer, r.dt)];
            let (ex, ey) = rep_extent(&r.rep);
            grow(
                &mut dbox[ci],
                li,
                BBox {
                    x0: r.x + ex.0.min(0),
                    y0: r.y + ey.0.min(0),
                    x1: r.x + r.w + ex.1.max(0),
                    y1: r.y + r.h + ey.1.max(0),
                },
                r.rep.members(),
                &mut dmask[ci],
            );
        }
        for p in &cell.polys {
            let li = lidx[&(p.layer, p.dt)];
            let bb = pts_bbox(&p.pts);
            let (ex, ey) = rep_extent(&p.rep);
            grow(
                &mut dbox[ci],
                li,
                BBox {
                    x0: bb.x0 + ex.0.min(0),
                    y0: bb.y0 + ey.0.min(0),
                    x1: bb.x1 + ex.1.max(0),
                    y1: bb.y1 + ey.1.max(0),
                },
                p.rep.members(),
                &mut dmask[ci],
            );
        }
        for pa in &cell.paths {
            let li = lidx[&(pa.layer, pa.dt)];
            let b4 = path_bbox(&pa.pts, pa.hw, pa.es, pa.ee);
            let (ex, ey) = rep_extent(&pa.rep);
            grow(
                &mut dbox[ci],
                li,
                BBox {
                    x0: b4.0 + ex.0.min(0),
                    y0: b4.1 + ey.0.min(0),
                    x1: b4.2 + ex.1.max(0),
                    y1: b4.3 + ey.1.max(0),
                },
                pa.rep.members(),
                &mut dmask[ci],
            );
        }
        for t in &cell.texts {
            // anchors count into bboxes/masks (render culling must
            // not drop label-only subtrees) but not into members
            let li = lidx[&(t.layer, t.dt)];
            let (ex, ey) = rep_extent(&t.rep);
            floe_ovm::bit_set(&mut dmask[ci], li);
            dbox[ci].grow(&BBox {
                x0: t.x + ex.0.min(0),
                y0: t.y + ey.0.min(0),
                x1: t.x + ex.1.max(0),
                y1: t.y + ey.1.max(0),
            });
        }
    }
    let mut rmask = dmask.clone();
    let mut rmembers = dmembers.clone();
    let mut height = vec![0u16; n];
    loop {
        let mut changed = false;
        for ci in 0..n {
            let mut hm = 0u16;
            let mut mm = dmembers[ci];
            let mut mask = dmask[ci].clone();
            for pl in &doc.cells[ci].places {
                hm = hm.max(height[pl.cell] + 1);
                mm = mm.saturating_add(
                    rmembers[pl.cell]
                        .saturating_mul(pl.rep.members()),
                );
                for (a, b) in
                    mask.iter_mut().zip(&rmask[pl.cell])
                {
                    *a |= *b;
                }
            }
            if hm != height[ci]
                || mm != rmembers[ci]
                || mask != rmask[ci]
            {
                height[ci] = hm;
                rmembers[ci] = mm;
                rmask[ci] = mask;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // per-layer geometry totals (texts excluded - they are not
    // paged; G5 compares page sums against these)
    let mut lrecs = vec![0u64; nl];
    let mut lmems = vec![0u64; nl];
    for cell in &doc.cells {
        for r in &cell.rects {
            let li = lidx[&(r.layer, r.dt)];
            lrecs[li] += 1;
            lmems[li] += r.rep.members();
        }
        for p in &cell.polys {
            let li = lidx[&(p.layer, p.dt)];
            lrecs[li] += 1;
            lmems[li] += p.rep.members();
        }
        for pa in &cell.paths {
            let li = lidx[&(pa.layer, pa.dt)];
            lrecs[li] += 1;
            lmems[li] += pa.rep.members();
        }
    }

    let mut b = Builder::new(doc.unit, size, mtime, nl);
    b.top = doc.top as u32;
    for (i, &(l, d)) in doc.layer_order.iter().enumerate() {
        let nm = doc
            .layer_names
            .get(&(l, d))
            .cloned()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{}/{}", l, d));
        b.layer(l, d, &nm, lrecs[i], lmems[i]);
    }

    let ovp_path = format!("{}/design.ovp", outdir);
    let mut ovp = std::io::BufWriter::new(
        std::fs::File::create(&ovp_path).expect("create ovp"),
    );
    let mut ovp_off = 0u64;
    let mut pages_total = 0u64;
    let mut pages_bytes = 0u64;

    for ci in 0..n {
        let cell = &doc.cells[ci];
        // ---- placements + instance BVH (leaf order = emit order)
        let place_base = b.n_places();
        assert!(place_base + (cell.places.len() as u64) < u32::MAX as u64);
        let mut items: Vec<(BBox, usize)> = cell
            .places
            .iter()
            .enumerate()
            .map(|(pi, pl)| {
                let cb = win_to_bbox(rbb[pl.cell]);
                let bb = if cb.is_empty() {
                    BBox {
                        x0: pl.x,
                        y0: pl.y,
                        x1: pl.x,
                        y1: pl.y,
                    }
                } else {
                    let xf =
                        Xf::place(pl.x, pl.y, pl.rot, pl.flip);
                    let a = xf.apply(cb.x0, cb.y0);
                    let c = xf.apply(cb.x1, cb.y1);
                    let g = xf_rep(&pl.rep, &Xf::identity());
                    let (ex, ey) = rep_extent(&g);
                    BBox {
                        x0: a.0.min(c.0) + ex.0.min(0),
                        y0: a.1.min(c.1) + ey.0.min(0),
                        x1: a.0.max(c.0) + ex.1.max(0),
                        y1: a.1.max(c.1) + ey.1.max(0),
                    }
                };
                (bb, pi)
            })
            .collect();
        let (bvh_start, bvh_count) = if items.is_empty() {
            (0, 0)
        } else {
            let mut nodes: Vec<(BBox, u32, u16, bool)> = Vec::new();
            nodes.push((BBox::EMPTY, 0, 0, false));
            split_bvh(&mut nodes, &mut items, 0, 0);
            let base = b.n_bvh();
            // emit places in reordered (leaf) order
            for &(_, pi) in items.iter() {
                let pl = &cell.places[pi];
                b.place(
                    pl.cell as u32,
                    pl.x,
                    pl.y,
                    pl.rot,
                    pl.flip as bool,
                    &pl.rep,
                );
            }
            for &(bb, first, count, leaf) in &nodes {
                let f = if leaf {
                    place_base as u32 + first
                } else {
                    base + first
                };
                b.bvh_node(&bb, f, count, leaf);
            }
            (base, nodes.len() as u32)
        };

        // ---- pages per layer, in layer order
        let page_start = b.n_pages();
        let mut per_layer: Vec<Vec<PRec>> =
            (0..nl).map(|_| Vec::new()).collect();
        for (idx, r) in cell.rects.iter().enumerate() {
            let li = lidx[&(r.layer, r.dt)];
            let (ex, ey) = rep_extent(&r.rep);
            per_layer[li].push(PRec {
                kind: 0,
                idx: idx as u32,
                bbox: BBox {
                    x0: r.x + ex.0.min(0),
                    y0: r.y + ey.0.min(0),
                    x1: r.x + r.w + ex.1.max(0),
                    y1: r.y + r.h + ey.1.max(0),
                },
                bytes: 16 + rep_est(&r.rep),
                members: r.rep.members(),
                w: r.w,
                h: r.h,
            });
        }
        for (idx, p) in cell.polys.iter().enumerate() {
            let li = lidx[&(p.layer, p.dt)];
            let bb = pts_bbox(&p.pts);
            let (ex, ey) = rep_extent(&p.rep);
            per_layer[li].push(PRec {
                kind: 1,
                idx: idx as u32,
                bbox: BBox {
                    x0: bb.x0 + ex.0.min(0),
                    y0: bb.y0 + ey.0.min(0),
                    x1: bb.x1 + ex.1.max(0),
                    y1: bb.y1 + ey.1.max(0),
                },
                bytes: 8 + 3 * p.pts.len() as u32 + rep_est(&p.rep),
                members: p.rep.members(),
                w: bb.x1 - bb.x0,
                h: bb.y1 - bb.y0,
            });
        }
        for (idx, pa) in cell.paths.iter().enumerate() {
            let li = lidx[&(pa.layer, pa.dt)];
            let b4 = path_bbox(&pa.pts, pa.hw, pa.es, pa.ee);
            let (ex, ey) = rep_extent(&pa.rep);
            per_layer[li].push(PRec {
                kind: 2,
                idx: idx as u32,
                bbox: BBox {
                    x0: b4.0 + ex.0.min(0),
                    y0: b4.1 + ey.0.min(0),
                    x1: b4.2 + ex.1.max(0),
                    y1: b4.3 + ey.1.max(0),
                },
                bytes: 14
                    + 3 * pa.pts.len() as u32
                    + rep_est(&pa.rep),
                members: pa.rep.members(),
                w: b4.2 - b4.0,
                h: b4.3 - b4.1,
            });
        }
        for (li, mut recs) in per_layer.into_iter().enumerate() {
            if recs.is_empty() {
                continue;
            }
            let mut seq = 0u16;
            let total = recs.len();
            emit_pages(
                doc, ci, li as u32, &mut recs[..], total, &mut seq,
                &mut b, &mut ovp, &mut ovp_off, &mut pages_total,
                &mut pages_bytes,
            );
        }
        let page_count = b.n_pages() - page_start;

        let mask_d = b.bitset(&dmask[ci]);
        let mask_r = b.bitset(&rmask[ci]);
        b.cell(
            &cell.name,
            height[ci],
            &dbox[ci],
            &win_to_bbox(rbb[ci]),
            place_base as u32,
            cell.places.len() as u32,
            page_start,
            page_count,
            bvh_start,
            bvh_count,
            mask_d,
            mask_r,
            rmembers[ci],
        );
    }
    ovp.flush().expect("flush ovp");
    let ovm_bytes = b.finish();
    let ovm_path = format!("{}/design.ovm", outdir);
    std::fs::write(&ovm_path, &ovm_bytes).expect("write ovm");
    eprintln!(
        "[vfs] {} pages ({}) + ovm {} in {:.1}s",
        pages_total,
        fmt_size(pages_bytes),
        fmt_size(ovm_bytes.len() as u64),
        t0.elapsed().as_secs_f64()
    );
}

fn pts_bbox(pts: &[(i64, i64)]) -> BBox {
    let mut b = BBox::EMPTY;
    for &(x, y) in pts {
        b.grow(&BBox { x0: x, y0: y, x1: x, y1: y });
    }
    b
}

/// binary BVH over (bbox, local place idx); reorders items so leaf
/// ranges are contiguous; nodes[slot] filled, children appended in
/// adjacent pairs (first = local node index of the left child)
fn split_bvh(
    nodes: &mut Vec<(BBox, u32, u16, bool)>,
    items: &mut [(BBox, usize)],
    lo: usize,
    slot: usize,
) {
    let mut bb = BBox::EMPTY;
    for (ib, _) in items.iter() {
        bb.grow(ib);
    }
    if items.len() <= BVH_LEAF {
        nodes[slot] = (bb, lo as u32, items.len() as u16, true);
        return;
    }
    let wx = bb.x1 - bb.x0;
    let wy = bb.y1 - bb.y0;
    let mid = items.len() / 2;
    if wx >= wy {
        items.select_nth_unstable_by_key(mid, |(b, _)| {
            b.x0 + b.x1
        });
    } else {
        items.select_nth_unstable_by_key(mid, |(b, _)| {
            b.y0 + b.y1
        });
    }
    let l = nodes.len();
    nodes.push((BBox::EMPTY, 0, 0, false));
    nodes.push((BBox::EMPTY, 0, 0, false));
    nodes[slot] = (bb, l as u32, 2, false);
    let (a, c) = items.split_at_mut(mid);
    split_bvh(nodes, a, lo, l);
    split_bvh(nodes, c, lo + mid, l + 1);
}

#[allow(clippy::too_many_arguments)]
fn emit_pages(
    doc: &Doc,
    ci: usize,
    li: u32,
    recs: &mut [PRec],
    total: usize,
    seq: &mut u16,
    b: &mut Builder,
    ovp: &mut std::io::BufWriter<std::fs::File>,
    ovp_off: &mut u64,
    pages_total: &mut u64,
    pages_bytes: &mut u64,
) {
    let bytes: u64 = recs.iter().map(|r| r.bytes as u64).sum();
    if bytes > PAGE_TARGET_BYTES as u64
        && recs.len() > PAGE_MIN_RECORDS
    {
        let mut bb = BBox::EMPTY;
        for r in recs.iter() {
            bb.grow(&r.bbox);
        }
        let mid = recs.len() / 2;
        if bb.x1 - bb.x0 >= bb.y1 - bb.y0 {
            recs.select_nth_unstable_by_key(mid, |r| {
                r.bbox.x0 + r.bbox.x1
            });
        } else {
            recs.select_nth_unstable_by_key(mid, |r| {
                r.bbox.y0 + r.bbox.y1
            });
        }
        let (a, c) = recs.split_at_mut(mid);
        emit_pages(
            doc, ci, li, a, total, seq, b, ovp, ovp_off,
            pages_total, pages_bytes,
        );
        emit_pages(
            doc, ci, li, c, total, seq, b, ovp, ovp_off,
            pages_total, pages_bytes,
        );
        return;
    }
    // assemble the page payload: single-cell OASIS via write_tree
    let cell = &doc.cells[ci];
    let mut rects: Vec<RectRec> = Vec::new();
    let mut polys: Vec<PolyRec> = Vec::new();
    let mut paths: Vec<PathRec> = Vec::new();
    let mut bb = BBox::EMPTY;
    let mut members = 0u64;
    let (mut max_w, mut max_h) = (0i64, 0i64);
    for r in recs.iter() {
        bb.grow(&r.bbox);
        members += r.members;
        max_w = max_w.max(r.w);
        max_h = max_h.max(r.h);
        match r.kind {
            0 => rects.push(cell.rects[r.idx as usize].clone()),
            1 => polys.push(cell.polys[r.idx as usize].clone()),
            _ => paths.push(cell.paths[r.idx as usize].clone()),
        }
    }
    let wc = WCell {
        name: "P".to_string(),
        rects: &rects,
        polys: &polys,
        paths: &paths,
        texts: &[],
        places: Vec::new(),
    };
    let payload =
        write_tree(&[wc], doc.unit).expect("page payload");
    ovp.write_all(&payload).expect("write ovp");
    b.page(
        ci as u32,
        li,
        *seq,
        &bb,
        *ovp_off,
        payload.len() as u32,
        payload.len() as u32,
        recs.len() as u32,
        members,
        max_w.clamp(0, u32::MAX as i64) as u32,
        max_h.clamp(0, u32::MAX as i64) as u32,
    );
    *ovp_off += payload.len() as u64;
    *pages_total += 1;
    *pages_bytes += payload.len() as u64;
    *seq = seq.checked_add(1).expect("page seq overflow");
    let _ = total;
}

fn fmt_size(bytes: u64) -> String {
    let mut v = bytes as f64;
    for unit in ["B", "K", "M", "G", "T"] {
        if v < 1024.0 || unit == "T" {
            return if v < 10.0 && unit != "B" {
                format!("{:.1}{}", v, unit)
            } else {
                format!("{:.0}{}", v, unit)
            };
        }
        v /= 1024.0;
    }
    unreachable!()
}

// ------------------------------------------------------------- plan

pub fn plan_cmd(args: &[String]) {
    let mut dir: Option<String> = None;
    let mut view: Option<(f64, f64, f64, f64)> = None;
    let mut px_per_um = 5.0f64;
    let mut cut_px = 2.0f64;
    let mut depth = u32::MAX;
    let mut layers: Option<Vec<String>> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--view" => {
                let v: Vec<f64> = args[i + 1]
                    .split(',')
                    .map(|s| s.parse().expect("view um"))
                    .collect();
                assert_eq!(v.len(), 4, "--view x0,y0,x1,y1 (um)");
                view = Some((v[0], v[1], v[2], v[3]));
                i += 2;
            }
            "--px-per-um" => {
                px_per_um = args[i + 1].parse().expect("px");
                i += 2;
            }
            "--cut-px" => {
                cut_px = args[i + 1].parse().expect("cut");
                i += 2;
            }
            "--depth" => {
                depth = args[i + 1].parse().expect("depth");
                i += 2;
            }
            "--layers" => {
                layers = Some(
                    args[i + 1]
                        .split(',')
                        .map(|s| s.to_string())
                        .collect(),
                );
                i += 2;
            }
            a => {
                if dir.is_none() {
                    dir = Some(a.to_string());
                }
                i += 1;
            }
        }
    }
    let dir = dir.expect("ovm dir");
    let v = Ovm::open(&format!("{}/design.ovm", dir))
        .expect("open ovm");
    let view = view.expect("--view required");
    // um -> dbu
    let s = v.unit;
    let vb = BBox {
        x0: (view.0 * s) as i64,
        y0: (view.1 * s) as i64,
        x1: (view.2 * s) as i64,
        y1: (view.3 * s) as i64,
    };
    // visible layer bitset
    let mut vis = vec![0u8; v.bs_width];
    match &layers {
        None => vis.iter_mut().for_each(|b| *b = 0xff),
        Some(list) => {
            for spec in list {
                let mut hit = false;
                for li in 0..v.n_layers {
                    let lr = v.layer(li);
                    let byname = lr.name == *spec;
                    let bypair = format!("{}/{}", lr.layer, lr.dt)
                        == *spec;
                    if byname || bypair {
                        floe_ovm::bit_set(&mut vis, li as usize);
                        hit = true;
                    }
                }
                assert!(hit, "layer {:?} not found", spec);
            }
        }
    }
    // cut threshold in dbu: features smaller than cut_px never
    // reach a pixel at this scale
    let cut_dbu = (cut_px / px_per_um * s) as i64;

    let t0 = std::time::Instant::now();
    let mut st = PlanStats::default();
    let mut pages: std::collections::HashSet<u32> =
        std::collections::HashSet::new();
    descend(
        &v, v.top, &Xf::identity(), &vb, &vis, cut_dbu, depth, 0,
        &mut pages, &mut st,
    );
    let mut cbytes = 0u64;
    let mut ubytes = 0u64;
    let mut records = 0u64;
    let mut members = 0u64;
    for &pi in &pages {
        let p = v.page(pi);
        cbytes += p.csize as u64;
        ubytes += p.usize_ as u64;
        records += p.records as u64;
        members += p.members;
    }
    println!(
        "{{\n  \"pages\": {},\n  \"page_reads\": {},\n  \
         \"compressed_bytes\": {},\n  \"encoded_bytes\": {},\n  \
         \"records\": {},\n  \"members\": {},\n  \
         \"visited_cells\": {},\n  \"visited_bvh\": {},\n  \
         \"culled_subtrees_size\": {},\n  \
         \"culled_subtrees_layer\": {},\n  \
         \"culled_pages_size\": {},\n  \"materializations\": {},\n  \
         \"plan_ms\": {:.2}\n}}",
        pages.len(),
        st.page_reads,
        cbytes,
        ubytes,
        records,
        members,
        st.visited_cells,
        st.visited_bvh,
        st.cull_size,
        st.cull_layer,
        st.cull_page_size,
        st.materializations,
        t0.elapsed().as_secs_f64() * 1e3
    );
}

#[derive(Default)]
struct PlanStats {
    visited_cells: u64,
    visited_bvh: u64,
    cull_size: u64,
    cull_layer: u64,
    cull_page_size: u64,
    page_reads: u64,
    materializations: u64,
}

/// transform a cell-local bbox to parent frame
fn xf_bbox(xf: &Xf, b: &BBox) -> BBox {
    let a = xf.apply(b.x0, b.y0);
    let c = xf.apply(b.x1, b.y1);
    BBox {
        x0: a.0.min(c.0),
        y0: a.1.min(c.1),
        x1: a.0.max(c.0),
        y1: a.1.max(c.1),
    }
}

#[allow(clippy::too_many_arguments)]
fn descend(
    v: &Ovm,
    ci: u32,
    xf: &Xf, // cell -> world
    view: &BBox,
    vis: &[u8],
    cut_dbu: i64,
    depth_limit: u32,
    depth: u32,
    pages: &mut std::collections::HashSet<u32>,
    st: &mut PlanStats,
) {
    let cell = v.cell(ci);
    st.visited_cells += 1;
    // recursive layer mask
    if !floe_ovm::masks_intersect(v.bitset(cell.lmask_rec), vis) {
        st.cull_layer += 1;
        return;
    }
    // subtree world bbox vs view
    let wb = xf_bbox(xf, &cell.rbbox);
    if !wb.intersects(view) {
        return;
    }
    // whole subtree below cut size -> proxy territory (V3); the
    // plan counts it culled
    if (wb.x1 - wb.x0) < cut_dbu && (wb.y1 - wb.y0) < cut_dbu {
        st.cull_size += 1;
        return;
    }
    st.materializations += 1;
    // pages of this cell: local-frame view test
    let inv = xf.invert();
    let lview = xf_bbox(&inv, view);
    for pi in cell.page_start..cell.page_start + cell.page_count {
        let p = v.page(pi);
        st.page_reads += 1;
        if !floe_ovm::bit_test(vis, p.layer_idx as usize) {
            continue;
        }
        if !p.bbox.intersects(&lview) {
            continue;
        }
        if (p.max_w as i64) < cut_dbu && (p.max_h as i64) < cut_dbu
        {
            st.cull_page_size += 1;
            continue;
        }
        pages.insert(pi);
    }
    if depth >= depth_limit {
        return;
    }
    // children via the instance BVH
    if cell.bvh_count == 0 {
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
            // offset-invariant culls BEFORE member enumeration: a
            // child too small on screen (or with no visible layer)
            // is too small at EVERY member offset - never expand
            // (valmini: 2.25M member visits -> 1 cull without this)
            let child = v.cell(pl.child);
            if !floe_ovm::masks_intersect(
                v.bitset(child.lmask_rec),
                vis,
            ) {
                st.cull_layer += 1;
                continue;
            }
            let base = xf.compose(&Xf::place(
                pl.x, pl.y, pl.rot, pl.flip,
            ));
            let cwb = xf_bbox(&base, &child.rbbox);
            if !cwb.is_empty()
                && (cwb.x1 - cwb.x0) < cut_dbu
                && (cwb.y1 - cwb.y0) < cut_dbu
            {
                st.cull_size += 1;
                continue;
            }
            match &pl.rep {
                Rep::One => descend(
                    v, pl.child, &base, view, vis, cut_dbu,
                    depth_limit, depth + 1, pages, st,
                ),
                rep => {
                    // visible member range only - offsets are in
                    // the PARENT frame (xf_rep at build kept them
                    // cell-local; transform per member)
                    for (ox, oy) in visible_offsets(
                        v, xf, &pl, rep, view,
                    ) {
                        let m = xf.compose(&Xf::place(
                            pl.x + ox,
                            pl.y + oy,
                            pl.rot,
                            pl.flip,
                        ));
                        descend(
                            v, pl.child, &m, view, vis, cut_dbu,
                            depth_limit, depth + 1, pages, st,
                        );
                    }
                }
            }
        }
    }
}

/// offsets of rep members whose child bbox can intersect the view
/// (arithmetic range for grids, filtered scan for point lists -
/// never expands blind)
fn visible_offsets(
    v: &Ovm,
    xf: &Xf,
    pl: &floe_ovm::PlaceV,
    rep: &Rep,
    view: &BBox,
) -> Vec<(i64, i64)> {
    let cb = v.cell(pl.child).rbbox;
    let xfc = xf.compose(&Xf::place(pl.x, pl.y, pl.rot, pl.flip));
    let wb = xf_bbox(&xfc, &cb);
    if wb.is_empty() {
        return Vec::new();
    }
    // slack in parent-frame units around the base placement
    let sx0 = view.x0 - wb.x1;
    let sx1 = view.x1 - wb.x0;
    let sy0 = view.y0 - wb.y1;
    let sy1 = view.y1 - wb.y0;
    // offsets are applied in the parent frame BEFORE xf; transform
    // slack back into parent frame via xf alone
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
                // solve i range for va stepping (axis or oblique)
                for i in grid_axis_range(
                    *na as i64, va.0, ox0 - bx, ox1 - bx,
                )
                .intersect(grid_axis_range(
                    *na as i64, va.1, oy0 - by, oy1 - by,
                ))
                .iter()
                {
                    out.push((i * va.0 + bx, i * va.1 + by));
                }
            }
        }
        Rep::Pts(p) => {
            for &(x, y) in p {
                if x >= ox0 && x <= ox1 && y >= oy0 && y <= oy1 {
                    out.push((x, y));
                }
            }
        }
    }
    out
}

#[derive(Clone, Copy)]
struct IRange {
    lo: i64,
    hi: i64, // inclusive; empty when lo > hi
}

impl IRange {
    fn intersect(self, o: IRange) -> IRange {
        IRange { lo: self.lo.max(o.lo), hi: self.hi.min(o.hi) }
    }
    fn iter(self) -> impl Iterator<Item = i64> {
        self.lo..=self.hi
    }
}

/// i in [0, n) with lo <= i*step <= hi (step may be 0 or negative)
fn grid_axis_range(n: i64, step: i64, lo: i64, hi: i64) -> IRange {
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
