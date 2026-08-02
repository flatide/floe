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
use floe_ovm::{BBox, Builder};
use floe_tiler::hier::{cell_bboxes_full, rep_extent};
use floe_tiler::{path_bbox, xf_rep, Xf};
use std::io::Write;

const PAGE_TARGET_BYTES: usize = 1 << 20; // pre-encode estimate
const PAGE_MIN_RECORDS: usize = 64;
const BVH_LEAF: usize = 8;

// cut-frame outline layer (shared with the skeleton's cell-outline
// convention; drawn hollow by the viewer)
const FRAME_LAYER: u32 = 255;
const FRAME_DT: u32 = 0;

// ------------------------------------------------------------ build

pub fn vfs_cmd(args: &[String]) {
    let mut src: Option<String> = None;
    let mut outdir: Option<String> = None;
    let mut jobs: Option<usize> = None;
    // coverage bitplanes are optional: off by default (extra build
    // time, and the viewer defaults to coverage off), --coverage to
    // include, --coverage-only to add design.ovc to an existing cache
    let mut coverage = false;
    let mut coverage_only = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--jobs" => {
                jobs = Some(args[i + 1].parse().expect("jobs"));
                i += 2;
            }
            "--coverage" => {
                coverage = true;
                i += 1;
            }
            "--coverage-only" => {
                coverage_only = true;
                i += 1;
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
    if coverage_only {
        // add design.ovc to an existing cache (no re-tiling): the
        // pages/skeleton/meta stay as they are
        if !std::path::Path::new(&format!("{}/design.ovm", outdir))
            .exists()
        {
            eprintln!(
                "--coverage-only: {}/design.ovm not found (build \
                 the cache first)",
                outdir
            );
            std::process::exit(1);
        }
    } else {
        build(&doc, size, mtime, &outdir, jobs);
        emit_viewer_side(&doc, &src, size, mtime, &outdir);
    }
    if coverage || coverage_only {
        // coverage bitplanes (V3b): optional density overview
        let tc = std::time::Instant::now();
        let ovc = floe_vfs::coverage::write_ovc(
            &doc,
            &doc.layer_order,
            jobs,
        );
        std::fs::write(format!("{}/design.ovc", outdir), &ovc)
            .expect("write ovc");
        eprintln!(
            "[vfs] coverage {} ({:.1}s)",
            fmt_size(ovc.len() as u64),
            tc.elapsed().as_secs_f64()
        );
    }
    eprintln!(
        "[vfs] done in {:.1}s -> {}",
        t0.elapsed().as_secs_f64(),
        outdir
    );
}

/// skeleton.oas + texts.tsv + meta.json: the viewer-facing trio the
/// .ice used to carry - far view, text search, and the meta fields
/// the GUI reads (dbu/bbox/grid/layers+color/src). grid is synthetic
/// here (no tiles in the VFS cache): it only feeds the viewer's
/// live-vs-skel span heuristic and stays on the .ice formula.
fn emit_viewer_side(
    doc: &Doc,
    src: &str,
    size: u64,
    mtime: u64,
    outdir: &str,
) {
    let t0 = std::time::Instant::now();
    let entries = floe_tiler::skel::collect_all_texts(doc);
    let sk = floe_tiler::skel::build_skeleton(
        doc,
        &entries,
        floe_tiler::skel::SKEL_TEXT_CAP,
    );
    let skcell = WCell {
        name: "SKEL_TOP".to_string(),
        rects: &sk.rects,
        polys: &sk.polys,
        paths: &sk.paths,
        texts: &sk.texts,
        places: Vec::new(),
    };
    let bytes =
        write_tree(&[skcell], doc.unit).expect("skeleton bytes");
    std::fs::write(format!("{}/skeleton.oas", outdir), bytes)
        .expect("write skeleton");
    let mut sidecar: Vec<&floe_tiler::skel::TextEntry> =
        entries.iter().collect();
    sidecar.sort_by(|a, b| {
        (a.layer, a.dt, &a.s, a.x, a.y)
            .cmp(&(b.layer, b.dt, &b.s, b.x, b.y))
    });
    let mut tsv = String::new();
    let mut side_members = 0u64;
    for e in &sidecar {
        side_members += e.members();
        tsv.push_str(&format!(
            "{}/{}\t{}\t{}\t{}\t{}\n",
            e.layer,
            e.dt,
            e.x,
            e.y,
            crate::fmt_factors(&e.factors),
            crate::tsv_esc(&e.s)
        ));
    }
    std::fs::write(format!("{}/texts.tsv", outdir), tsv)
        .expect("write sidecar");

    // meta.json (VFS flavor)
    let rbb = cell_bboxes_full(doc);
    let bbox = rbb[doc.top].unwrap_or((0, 0, 0, 0));
    let dbu = 1.0 / doc.unit;
    let n = {
        let t = (size as f64 / 6.0e6).sqrt().round_ties_even()
            as i64;
        t.clamp(4, 96)
    };
    let tw = ((bbox.2 - bbox.0) as f64 / n as f64).ceil() as i64;
    let th = ((bbox.3 - bbox.1) as f64 / n as f64).ceil() as i64;
    let mut stored: std::collections::HashMap<(u32, u32), u64> =
        std::collections::HashMap::new();
    for cell in &doc.cells {
        for r in &cell.rects {
            *stored.entry((r.layer, r.dt)).or_default() +=
                r.rep.members();
        }
        for p in &cell.polys {
            *stored.entry((p.layer, p.dt)).or_default() +=
                p.rep.members();
        }
        for pa in &cell.paths {
            *stored.entry((pa.layer, pa.dt)).or_default() +=
                pa.rep.members();
        }
        for t in &cell.texts {
            *stored.entry((t.layer, t.dt)).or_default() +=
                t.rep.members();
        }
    }
    let layers_json: Vec<String> = doc
        .layer_order
        .iter()
        .enumerate()
        .map(|(i, &(l, d))| {
            let name = doc
                .layer_names
                .get(&(l, d))
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{}/{}", l, d));
            format!(
                "{{\"layer\": {}, \"datatype\": {}, \"name\": \
                 \"{}\", \"color\": \"{}\", \"stored_shapes\": {}}}",
                l,
                d,
                crate::jesc(&name),
                crate::layer_color(i),
                stored.get(&(l, d)).copied().unwrap_or(0)
            )
        })
        .collect();
    let abs_src = if std::path::Path::new(src).is_absolute() {
        src.to_string()
    } else {
        std::env::current_dir()
            .expect("cwd")
            .join(src)
            .to_string_lossy()
            .into_owned()
    };
    let meta = format!(
        "{{\n\
         \"version\": {},\n\
         \"vfs\": 1,\n\
         \"src\": {{\"path\": \"{}\", \"size\": {}, \
         \"mtime\": {}}},\n\
         \"dbu\": {},\n\
         \"top_cell\": \"{}\",\n\
         \"bbox\": [{}, {}, {}, {}],\n\
         \"grid\": {{\"nx\": {}, \"ny\": {}, \"x0\": {}, \
         \"y0\": {}, \"tile_w\": {}, \"tile_h\": {}}},\n\
         \"layers\": [\n{}\n],\n\
         \"skeleton\": {{\"file\": \"skeleton.oas\", \
         \"shapes\": {}, \"texts\": {}}},\n\
         \"texts_sidecar\": {{\"file\": \"texts.tsv\", \
         \"entries\": {}, \"members\": {}}}\n\
         }}\n",
        crate::CACHE_VERSION,
        crate::jesc(&abs_src),
        size,
        mtime,
        dbu,
        crate::jesc(&doc.cells[doc.top].name),
        bbox.0,
        bbox.1,
        bbox.2,
        bbox.3,
        n,
        n,
        bbox.0,
        bbox.1,
        tw,
        th,
        layers_json.join(",\n"),
        sk.shapes,
        sk.labels,
        sidecar.len(),
        side_members,
    );
    std::fs::write(format!("{}/meta.json", outdir), meta)
        .expect("write meta");
    eprintln!(
        "[vfs] skeleton {} shapes + {} labels, sidecar {} entries, \
         meta ({:.1}s)",
        sk.shapes,
        sk.labels,
        sidecar.len(),
        t0.elapsed().as_secs_f64()
    );
}

fn win_to_bbox(w: Option<(i64, i64, i64, i64)>) -> BBox {
    match w {
        None => BBox::EMPTY,
        Some((x0, y0, x1, y1)) => BBox { x0, y0, x1, y1 },
    }
}

/// per-record page item: kind 0 rect / 1 poly / 2 path
#[derive(Clone)]
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

/// one page's work: which records, and the metadata the split
/// already computed. Payload encoding (write_tree + deflate, the
/// build's hot cost) is deferred so it can run in parallel.
struct PageJob {
    ci: usize,
    li: u32,
    seq: u16,
    recs: Vec<PRec>,
    bbox: BBox,
    members: u64,
    max_w: i64,
    max_h: i64,
}

/// spatial-split a layer's records into page groups (same policy as
/// the old emit_pages, but produces jobs instead of writing bytes)
fn split_pages(
    ci: usize,
    li: u32,
    recs: &mut [PRec],
    seq: &mut u16,
    out: &mut Vec<PageJob>,
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
        split_pages(ci, li, a, seq, out);
        split_pages(ci, li, c, seq, out);
        return;
    }
    let mut bb = BBox::EMPTY;
    let mut members = 0u64;
    let (mut max_w, mut max_h) = (0i64, 0i64);
    for r in recs.iter() {
        bb.grow(&r.bbox);
        members += r.members;
        max_w = max_w.max(r.w);
        max_h = max_h.max(r.h);
    }
    out.push(PageJob {
        ci,
        li,
        seq: *seq,
        recs: recs.to_vec(),
        bbox: bb,
        members,
        max_w,
        max_h,
    });
    *seq = seq.checked_add(1).expect("page seq overflow");
}

/// encode one page job to its OASIS payload (parallel-safe: reads
/// doc immutably, allocates only its own buffers)
fn encode_job(doc: &Doc, job: &PageJob) -> Vec<u8> {
    let cell = &doc.cells[job.ci];
    let mut rects: Vec<RectRec> = Vec::new();
    let mut polys: Vec<PolyRec> = Vec::new();
    let mut paths: Vec<PathRec> = Vec::new();
    for r in &job.recs {
        match r.kind {
            0 => rects.push(cell.rects[r.idx as usize].clone()),
            1 => polys.push(cell.polys[r.idx as usize].clone()),
            _ => paths.push(cell.paths[r.idx as usize].clone()),
        }
    }
    let wc = WCell {
        name: floe_ovm::page_cell_name(job.ci as u32, job.li, job.seq),
        rects: &rects,
        polys: &polys,
        paths: &paths,
        texts: &[],
        places: Vec::new(),
    };
    write_tree(&[wc], doc.unit).expect("page payload")
}

fn build(doc: &Doc, size: u64, mtime: u64, outdir: &str, jobs: usize) {
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

    // page jobs accumulate across the cell loop (splitting only);
    // payloads encode in parallel afterwards
    let mut jobs_list: Vec<PageJob> = Vec::new();

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

        // ---- pages per layer, in layer order. Split into jobs now
        // (cheap); payloads encode in parallel after the cell loop.
        // Page index == job index (phase 3 appends in job order), so
        // page_start is the job count seen so far.
        let page_start = jobs_list.len() as u32;
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
            split_pages(ci, li as u32, &mut recs[..], &mut seq,
                        &mut jobs_list);
        }
        let page_count = jobs_list.len() as u32 - page_start;

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

    // phase 2: encode page payloads in parallel (write_tree +
    // deflate-6 is the build's hot cost; jobs are independent)
    let mut payloads: Vec<Vec<u8>> = Vec::new();
    payloads.resize_with(jobs_list.len(), Vec::new);
    if jobs > 1 && jobs_list.len() > 1 {
        // disjoint chunks_mut slices = safe parallel fill; pages are
        // ~uniform (1 MB target) so static chunking balances well
        let chunk = jobs_list.len().div_ceil(jobs).max(1);
        std::thread::scope(|s| {
            for (jc, pc) in jobs_list
                .chunks(chunk)
                .zip(payloads.chunks_mut(chunk))
            {
                s.spawn(move || {
                    for (job, slot) in jc.iter().zip(pc.iter_mut())
                    {
                        *slot = encode_job(doc, job);
                    }
                });
            }
        });
    } else {
        for (i, job) in jobs_list.iter().enumerate() {
            payloads[i] = encode_job(doc, job);
        }
    }

    // phase 3: write payloads sequentially (assigns ovp offsets) and
    // append page records in job order (page index == job index)
    let ovp_path = format!("{}/design.ovp", outdir);
    let mut ovp = std::io::BufWriter::new(
        std::fs::File::create(&ovp_path).expect("create ovp"),
    );
    let mut ovp_off = 0u64;
    let mut pages_bytes = 0u64;
    let pages_total = jobs_list.len() as u64;
    for (job, payload) in jobs_list.iter().zip(&payloads) {
        std::io::Write::write_all(&mut ovp, payload)
            .expect("write ovp");
        b.page(
            job.ci as u32,
            job.li,
            job.seq,
            &job.bbox,
            ovp_off,
            payload.len() as u32,
            payload.len() as u32,
            job.recs.len() as u32,
            job.members,
            job.max_w.clamp(0, u32::MAX as i64) as u32,
            job.max_h.clamp(0, u32::MAX as i64) as u32,
        );
        ovp_off += payload.len() as u64;
        pages_bytes += payload.len() as u64;
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

fn parse_common(
    args: &[String],
) -> (String, Option<(f64, f64, f64, f64)>, f64, f64, u32,
      Option<Vec<String>>, Vec<(String, String)>) {
    let mut dir: Option<String> = None;
    let mut view = None;
    let mut px_per_um = 5.0f64;
    let mut cut_px = 2.0f64;
    let mut depth = u32::MAX;
    let mut layers: Option<Vec<String>> = None;
    let mut rest: Vec<(String, String)> = Vec::new();
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
                depth = if args[i + 1] == "full" {
                    u32::MAX
                } else {
                    args[i + 1].parse().expect("depth")
                };
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
            a if a.starts_with("--") => {
                rest.push((
                    a.to_string(),
                    args.get(i + 1).cloned().unwrap_or_default(),
                ));
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
    (dir.expect("ovm dir"), view, px_per_um, cut_px, depth,
     layers, rest)
}

fn make_req(
    v: &floe_vfs::Vfs,
    view: (f64, f64, f64, f64),
    px_per_um: f64,
    cut_px: f64,
    depth: u32,
    layers: Option<&[String]>,
) -> floe_vfs::ViewReq {
    let s = v.ovm.unit;
    floe_vfs::ViewReq {
        view: BBox {
            x0: (view.0 * s) as i64,
            y0: (view.1 * s) as i64,
            x1: (view.2 * s) as i64,
            y1: (view.3 * s) as i64,
        },
        cut_dbu: (cut_px / px_per_um * s) as i64,
        vis: v.layer_mask(layers).expect("layers"),
        depth,
    }
}

pub fn plan_cmd(args: &[String]) {
    let (dir, view, px, cut, depth, layers, _) =
        parse_common(args);
    let v = floe_vfs::Vfs::open(&dir).expect("open vfs");
    let req = make_req(
        &v,
        view.expect("--view required"),
        px,
        cut,
        depth,
        layers.as_deref(),
    );
    let t0 = std::time::Instant::now();
    let plan = v.plan(&req);
    let (mut cbytes, mut ubytes) = (0u64, 0u64);
    let (mut records, mut members) = (0u64, 0u64);
    for &pi in &plan.pages {
        let p = v.ovm.page(pi);
        cbytes += p.csize as u64;
        ubytes += p.usize_ as u64;
        records += p.records as u64;
        members += p.members;
    }
    let st = &plan.stats;
    println!(
        "{{\n  \"pages\": {},\n  \"page_reads\": {},\n  \
         \"compressed_bytes\": {},\n  \"encoded_bytes\": {},\n  \
         \"records\": {},\n  \"members\": {},\n  \
         \"placements\": {},\n  \
         \"visited_cells\": {},\n  \"visited_bvh\": {},\n  \
         \"culled_subtrees_size\": {},\n  \
         \"culled_subtrees_layer\": {},\n  \
         \"culled_pages_size\": {},\n  \"frames\": {},\n  \
         \"materializations\": {},\n  \
         \"plan_ms\": {:.2}\n}}",
        plan.pages.len(),
        st.page_reads,
        cbytes,
        ubytes,
        records,
        members,
        plan.mats.len(),
        st.visited_cells,
        st.visited_bvh,
        st.cull_size,
        st.cull_layer,
        st.cull_page_size,
        plan.frames.len(),
        st.materializations,
        t0.elapsed().as_secs_f64() * 1e3
    );
}

// ------------------------------------------------------------- vfsd

/// stdio daemon for the viewer render service. Line protocol:
///   gen=1 view=x0,y0,x1,y1 px=5 cut=2 depth=full \
///     layers=11/0,12/0 out=/tmp/dir
/// response:
///   gen=1 pages=N new=N evict=name,.. delta=path placements=path \
///     bytes=N plan_ms=F resident_mb=F
/// "quit" or EOF ends the loop. Delta/placement files land under
/// out= and are the viewer's to delete after applying.
pub fn vfsd_cmd(args: &[String]) {
    let mut dir: Option<String> = None;
    let mut budget_mb: u64 = 1024;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--budget-mb" => {
                budget_mb = args[i + 1].parse().expect("budget");
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
    let v = floe_vfs::Vfs::open(&dir).expect("open vfs");
    let mut sess = floe_vfs::Session::new(budget_mb << 20);
    eprintln!(
        "[vfsd] {} ({} cells, {} pages), budget {} MB",
        dir, v.ovm.n_cells, v.ovm.n_pages, budget_mb
    );
    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        line.clear();
        use std::io::BufRead;
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "quit" {
            return;
        }
        match serve_one(&v, &mut sess, line) {
            Ok(resp) => println!("{}", resp),
            Err(e) => println!("error={}", e.replace(' ', "_")),
        }
        use std::io::Write as _;
        std::io::stdout().flush().ok();
    }
}

fn serve_one(
    v: &floe_vfs::Vfs,
    sess: &mut floe_vfs::Session,
    line: &str,
) -> Result<String, String> {
    let mut gen = 0u64;
    let mut view: Option<(f64, f64, f64, f64)> = None;
    let mut px = 5.0f64;
    let mut cut = 2.0f64;
    let mut depth = u32::MAX;
    let mut layers: Option<Vec<String>> = None;
    let mut out: Option<String> = None;
    let mut probe = false;
    for tok in line.split_whitespace() {
        let (k, val) = tok
            .split_once('=')
            .ok_or_else(|| format!("bad token {}", tok))?;
        match k {
            "gen" => gen = val.parse().map_err(|_| "gen")?,
            "view" => {
                let p: Vec<f64> = val
                    .split(',')
                    .map(|s| s.parse().unwrap_or(f64::NAN))
                    .collect();
                if p.len() != 4 || p.iter().any(|x| x.is_nan()) {
                    return Err("view".into());
                }
                view = Some((p[0], p[1], p[2], p[3]));
            }
            "px" => px = val.parse().map_err(|_| "px")?,
            "cut" => cut = val.parse().map_err(|_| "cut")?,
            "depth" => {
                depth = if val == "full" {
                    u32::MAX
                } else {
                    val.parse().map_err(|_| "depth")?
                }
            }
            "layers" => {
                if val != "all" && !val.is_empty() {
                    layers = Some(
                        val.split(',')
                            .map(|s| s.to_string())
                            .collect(),
                    );
                }
            }
            "out" => out = Some(val.to_string()),
            "mode" => probe = val == "probe",
            _ => return Err(format!("unknown key {}", k)),
        }
    }
    let view = view.ok_or("view required")?;
    let out = out.ok_or("out required")?;
    let t0 = std::time::Instant::now();
    let req = make_req(v, view, px, cut, depth, layers.as_deref());
    let plan = v.plan(&req);
    // probe = session-less exact query (pick/snap/clip): the delta
    // carries EVERY planned page, the working set is untouched
    let upd = if probe {
        floe_vfs::Update {
            new: plan.pages.clone(),
            evict: Vec::new(),
            mats: plan.mats.clone(),
        }
    } else {
        sess.apply(v, &plan, gen)
    };
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let delta_path = if upd.new.is_empty() {
        "-".to_string()
    } else {
        let bytes = v.delta(&upd.new)?;
        let p = format!("{}/delta_{}.oas", out, gen);
        std::fs::write(&p, &bytes).map_err(|e| e.to_string())?;
        p
    };
    let mats_path = format!("{}/mats_{}.tsv", out, gen);
    {
        let mut w = String::new();
        for m in &upd.mats {
            // columns: page-cell name, x, y, rot, flip, array rep
            // (na nb vax vay vbx vby), design cell name (last, for
            // pick/status - page cells are an implementation detail)
            let dname =
                v.ovm.cell(v.ovm.page(m.page).cell).name;
            w.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                v.page_name(m.page),
                m.x,
                m.y,
                m.rot,
                m.flip as u8,
                m.na,
                m.nb,
                m.va.0,
                m.va.1,
                m.vb.0,
                m.vb.1,
                dname
            ));
        }
        std::fs::write(&mats_path, w)
            .map_err(|e| e.to_string())?;
    }
    // frames: ephemeral per-view outlines for cut-dropped subtrees,
    // always regenerated (never part of the working set)
    let frames_path = if plan.frames.is_empty() {
        "-".to_string()
    } else {
        use floe_oasis::doc::RectRec;
        let rects: Vec<RectRec> = plan
            .frames
            .iter()
            .map(|b| RectRec {
                layer: FRAME_LAYER,
                dt: FRAME_DT,
                x: b.x0,
                y: b.y0,
                w: (b.x1 - b.x0).max(1),
                h: (b.y1 - b.y0).max(1),
                rep: floe_oasis::doc::Rep::One,
            })
            .collect();
        let wc = WCell {
            name: format!("FRAMES_{}", gen),
            rects: &rects,
            polys: &[],
            paths: &[],
            texts: &[],
            places: Vec::new(),
        };
        let bytes =
            write_tree(&[wc], v.ovm.unit).map_err(|e| e.to_string())?;
        let p = format!("{}/frames_{}.oas", out, gen);
        std::fs::write(&p, &bytes).map_err(|e| e.to_string())?;
        p
    };
    let evict: Vec<String> =
        upd.evict.iter().map(|&pi| v.page_name(pi)).collect();
    let mut bytes = 0u64;
    let mut members = 0u64;
    for &pi in &plan.pages {
        let p = v.ovm.page(pi);
        bytes += p.csize as u64;
        members += p.members;
    }
    Ok(format!(
        "gen={} pages={} new={} evict={} delta={} placements={} \
         frames={} nframes={} bytes={} members={} plan_ms={:.2} \
         resident_mb={:.1}",
        gen,
        plan.pages.len(),
        upd.new.len(),
        if evict.is_empty() {
            "-".to_string()
        } else {
            evict.join(",")
        },
        delta_path,
        mats_path,
        frames_path,
        plan.frames.len(),
        bytes,
        members,
        t0.elapsed().as_secs_f64() * 1e3,
        sess.resident_bytes() as f64 / (1 << 20) as f64
    ))
}
