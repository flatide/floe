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
use floe_ovm::{narrow_u32, BBox, Builder, PBVH_NONE};
use floe_tiler::hier::{cell_bboxes_full, rep_extent};
use floe_tiler::{path_bbox, xf_rep, Xf};
use std::io::Write;

const PAGE_TARGET_BYTES: usize = 1 << 20; // pre-encode estimate
const PAGE_MIN_RECORDS: usize = 64;
const BVH_LEAF: usize = 8;
/// pages per page-BVH leaf, and the run size at or below which a
/// (cell,layer) run gets no BVH at all (linear scan, root = NONE)
const PBVH_LEAF: usize = 8;

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
    // parse heartbeat: a 9.8G parse is minutes of silence otherwise
    // (the build stages tick already; the parser is opaque, so tick
    // elapsed+rss from outside). All progress goes to stderr - the
    // protocol/plan output owns stdout.
    let parsing = std::sync::Arc::new(
        std::sync::atomic::AtomicBool::new(true),
    );
    {
        let parsing = parsing.clone();
        std::thread::spawn(move || {
            use std::sync::atomic::Ordering::Relaxed;
            let t = std::time::Instant::now();
            let mut last = 0u64;
            loop {
                std::thread::sleep(
                    std::time::Duration::from_millis(200),
                );
                if !parsing.load(Relaxed) {
                    return;
                }
                let e = t.elapsed().as_secs();
                if e >= last + 5 {
                    last = e;
                    eprintln!(
                        "[vfs] parsing... ({}s, rss {})",
                        e,
                        rss()
                    );
                }
            }
        });
    }
    let doc = match floe_oasis::doc::parse_doc_parallel(&data, jobs) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("parse {}: {}", src, e);
            std::process::exit(1);
        }
    };
    parsing.store(false, std::sync::atomic::Ordering::Relaxed);
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
        // add design.ovc to an existing cache (additive op, outside
        // the marker protocol): pages/skeleton/meta stay as they are
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
        write_coverage(&doc, &outdir, jobs);
    } else {
        // marker protocol (VFS_HIER.md par.3.6): design.ovm is the
        // commit marker. Kill it FIRST, delete the other outputs,
        // write everything, write the marker LAST - an interrupted
        // build reads as "no cache" (marker gone) or "corrupt cache"
        // (marker partial), never as a valid cache.
        // FLOE_KILL_AT: gate-only hook forcing death at the three
        // interrupt points (tools/validate_vfs_marker.py).
        let kill_at = std::env::var("FLOE_KILL_AT").ok();
        for f in [
            "design.ovm",
            "design.ovp",
            "design.ovc",
            "skeleton.oas",
            "texts.tsv",
            "meta.json",
        ] {
            let _ = std::fs::remove_file(format!("{}/{}", outdir, f));
        }
        if kill_at.as_deref() == Some("marker-deleted") {
            eprintln!("[vfs] FLOE_KILL_AT=marker-deleted");
            std::process::exit(9);
        }
        let ovm_bytes = build(&doc, size, mtime, &outdir, jobs);
        if kill_at.as_deref() == Some("ovp-written") {
            eprintln!("[vfs] FLOE_KILL_AT=ovp-written");
            std::process::exit(9);
        }
        emit_viewer_side(&doc, &src, size, mtime, &outdir);
        if coverage {
            write_coverage(&doc, &outdir, jobs);
        }
        if kill_at.as_deref() == Some("ovm-partial") {
            std::fs::write(
                format!("{}/design.ovm", outdir),
                &ovm_bytes[..ovm_bytes.len() / 2],
            )
            .expect("write partial ovm");
            eprintln!("[vfs] FLOE_KILL_AT=ovm-partial");
            std::process::exit(9);
        }
        std::fs::write(format!("{}/design.ovm", outdir), &ovm_bytes)
            .expect("write ovm");
        eprintln!(
            "[vfs] commit design.ovm ({})",
            fmt_size(ovm_bytes.len() as u64)
        );
    }
    eprintln!(
        "[vfs] done in {:.1}s -> {}",
        t0.elapsed().as_secs_f64(),
        outdir
    );
}

/// coverage bitplanes (V3b): optional density overview
fn write_coverage(doc: &Doc, outdir: &str, jobs: usize) {
    let tc = std::time::Instant::now();
    let ovc =
        floe_vfs::coverage::write_ovc(doc, &doc.layer_order, jobs);
    std::fs::write(format!("{}/design.ovc", outdir), &ovc)
        .expect("write ovc");
    eprintln!(
        "[vfs] coverage {} ({:.1}s)",
        fmt_size(ovc.len() as u64),
        tc.elapsed().as_secs_f64()
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
    seq: u32,
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
    seq: &mut u32,
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
fn encode_job(doc: &Doc, job: &PageJob) -> (Vec<u8>, u64) {
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
    floe_oasis::write::write_tree_sized(&[wc], doc.unit)
        .expect("page payload")
}

/// resident set size for the build heartbeat (Linux /proc; "?"
/// elsewhere) so a long silent stage on a big chip shows RSS growth.
fn rss() -> String {
    #[cfg(target_os = "linux")]
    if let Ok(s) = std::fs::read_to_string("/proc/self/statm") {
        if let Some(p) = s.split_whitespace().nth(1) {
            if let Ok(pages) = p.parse::<u64>() {
                return fmt_size(pages * 4096);
            }
        }
    }
    String::from("?")
}

/// reverse-topological (children-before-parents) cell order so the
/// recursive fold converges in ONE pass instead of the old O(depth)
/// fixpoint. Iterative post-order DFS. A back-edge (cyclic
/// hierarchy - invalid OASIS) is a HARD build error: the planner's
/// rank sweep and the fold both assume a DAG, and rank assignment
/// here is what guarantees it (VFS_HIER.md par.2.3).
fn topo_order(doc: &Doc) -> Vec<usize> {
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

/// per-cell plan produced in PARALLEL (the CPU-heavy work: instance
/// BVH + per-layer page splitting + page BVH + pts Morton prep).
/// The Builder appends these serially in cell order afterwards, so
/// `first`/base offsets are fixed up then and the .ovm/.ovp stay
/// byte-identical to the single-threaded build at any thread count.
struct CellPlan {
    place_order: Vec<u32>,            // local place idx, leaf order
    bvh: Vec<(BBox, u32, u16, bool)>, // `first` is cell-local
    /// pages across this cell's (layer) runs, each run already in
    /// its page-BVH leaf order (seq is a name, not a position)
    pages: Vec<PageJob>,
    /// (layer_idx, page_lo, page_count, pbvh_root) - page_lo and
    /// root are cell-local (root PBVH_NONE = linear run)
    pranges: Vec<(u32, u32, u32, u32)>,
    /// (bbox, first, count, leaf, max_w, max_h) - leaf `first` is a
    /// cell-local page index, inner `first` a cell-local node index
    pbvh: Vec<(BBox, u32, u16, bool, u64, u64)>,
    /// per local place idx: Morton-prepared pts (Rep::Pts only)
    pts_prep: Vec<Option<floe_ovm::PtsPrepared>>,
}

/// binary BVH over one (cell,layer) page run with subtree max_w/
/// max_h aggregates (pre-leaf cut culling). Same reorder trick as
/// split_bvh: items end up in leaf-concatenated order.
fn split_pbvh(
    nodes: &mut Vec<(BBox, u32, u16, bool, u64, u64)>,
    items: &mut [(BBox, u64, u64, usize)],
    lo: usize,
    slot: usize,
) {
    let mut bb = BBox::EMPTY;
    let (mut mw, mut mh) = (0u64, 0u64);
    for (ib, w, h, _) in items.iter() {
        bb.grow(ib);
        mw = mw.max(*w);
        mh = mh.max(*h);
    }
    if items.len() <= PBVH_LEAF {
        nodes[slot] =
            (bb, lo as u32, items.len() as u16, true, mw, mh);
        return;
    }
    let wx = bb.x1 - bb.x0;
    let wy = bb.y1 - bb.y0;
    let mid = items.len() / 2;
    if wx >= wy {
        items.select_nth_unstable_by_key(mid, |(b, _, _, _)| {
            b.x0 + b.x1
        });
    } else {
        items.select_nth_unstable_by_key(mid, |(b, _, _, _)| {
            b.y0 + b.y1
        });
    }
    let l = nodes.len();
    nodes.push((BBox::EMPTY, 0, 0, false, 0, 0));
    nodes.push((BBox::EMPTY, 0, 0, false, 0, 0));
    nodes[slot] = (bb, l as u32, 2, false, mw, mh);
    let (a, c) = items.split_at_mut(mid);
    split_pbvh(nodes, a, lo, l);
    split_pbvh(nodes, c, lo + mid, l + 1);
}

fn build_cell_plan(
    doc: &Doc,
    ci: usize,
    rbb: &[Option<(i64, i64, i64, i64)>],
    lidx: &std::collections::HashMap<(u32, u32), usize>,
    nl: usize,
) -> CellPlan {
    let cell = &doc.cells[ci];
    // ---- placements + instance BVH (leaf order = emit order)
    let mut items: Vec<(BBox, usize)> = cell
        .places
        .iter()
        .enumerate()
        .map(|(pi, pl)| {
            let cb = win_to_bbox(rbb[pl.cell]);
            let bb = if cb.is_empty() {
                BBox { x0: pl.x, y0: pl.y, x1: pl.x, y1: pl.y }
            } else {
                let xf = Xf::place(pl.x, pl.y, pl.rot, pl.flip);
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
    let (place_order, bvh) = if items.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        let mut nodes: Vec<(BBox, u32, u16, bool)> = Vec::new();
        nodes.push((BBox::EMPTY, 0, 0, false));
        split_bvh(&mut nodes, &mut items, 0, 0);
        let place_order =
            items.iter().map(|&(_, pi)| pi as u32).collect();
        (place_order, nodes)
    };
    // ---- pages per layer, in layer order
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
            bytes: 14 + 3 * pa.pts.len() as u32 + rep_est(&pa.rep),
            members: pa.rep.members(),
            w: b4.2 - b4.0,
            h: b4.3 - b4.1,
        });
    }
    let mut pages: Vec<PageJob> = Vec::new();
    let mut pranges: Vec<(u32, u32, u32, u32)> = Vec::new();
    let mut pbvh: Vec<(BBox, u32, u16, bool, u64, u64)> = Vec::new();
    for (li, mut recs) in per_layer.into_iter().enumerate() {
        if recs.is_empty() {
            continue;
        }
        let mut seq = 0u32;
        let mut run: Vec<PageJob> = Vec::new();
        split_pages(ci, li as u32, &mut recs[..], &mut seq, &mut run);
        let run_lo = narrow_u32(pages.len() as u64, "cell page count");
        let run_count = narrow_u32(run.len() as u64, "run page count");
        if run.len() <= PBVH_LEAF {
            // short run: linear scan beats a tree
            pranges.push((li as u32, run_lo, run_count, PBVH_NONE));
            pages.append(&mut run);
        } else {
            let mut items: Vec<(BBox, u64, u64, usize)> = run
                .iter()
                .enumerate()
                .map(|(k, j)| {
                    (
                        j.bbox,
                        j.max_w.max(0) as u64,
                        j.max_h.max(0) as u64,
                        k,
                    )
                })
                .collect();
            let root_local =
                narrow_u32(pbvh.len() as u64, "cell pbvh count");
            let base = pbvh.len();
            pbvh.push((BBox::EMPTY, 0, 0, false, 0, 0));
            {
                // split fills nodes with run-relative leaf ranges;
                // shift both leaf-first (page) and inner-first
                // (node) to cell-local bases afterwards
                let mut nodes: Vec<(BBox, u32, u16, bool, u64, u64)> =
                    vec![(BBox::EMPTY, 0, 0, false, 0, 0)];
                split_pbvh(&mut nodes, &mut items, 0, 0);
                pbvh.truncate(base);
                for (bb, first, count, leaf, mw, mh) in nodes {
                    let f = if leaf {
                        run_lo + first
                    } else {
                        root_local + first
                    };
                    pbvh.push((bb, f, count, leaf, mw, mh));
                }
            }
            // append the run's jobs in leaf order (items order
            // after the split IS the directory order)
            let mut slots: Vec<Option<PageJob>> =
                run.into_iter().map(Some).collect();
            for &(_, _, _, k) in &items {
                pages.push(slots[k].take().expect("pbvh perm"));
            }
            pranges.push((li as u32, run_lo, run_count, root_local));
        }
    }
    let pts_prep: Vec<Option<floe_ovm::PtsPrepared>> = cell
        .places
        .iter()
        .map(|pl| match &pl.rep {
            Rep::Pts(p) => Some(floe_ovm::prepare_pts(p)),
            _ => None,
        })
        .collect();
    CellPlan { place_order, bvh, pages, pranges, pbvh, pts_prep }
}

fn build(
    doc: &Doc,
    size: u64,
    mtime: u64,
    outdir: &str,
    jobs: usize,
) -> Vec<u8> {
    let t0 = std::time::Instant::now();
    let n = doc.cells.len();
    eprintln!("[vfs] build: recursive bbox ({} cells)...", n);
    let tbb = std::time::Instant::now();
    let rbb = cell_bboxes_full(doc);
    eprintln!(
        "[vfs] build: recursive bbox {:.1}s (rss {})",
        tbb.elapsed().as_secs_f64(),
        rss()
    );
    let nl = doc.layer_order.len();
    let lidx: std::collections::HashMap<(u32, u32), usize> = doc
        .layer_order
        .iter()
        .enumerate()
        .map(|(i, &k)| (k, i))
        .collect();

    // ---- direct (own-record) masks/members/bbox per cell AND the
    // per-layer geometry totals in ONE parallel record pass (both
    // were separate single-threaded O(records) sweeps). Texts count
    // into masks/bboxes but not into members or the paged totals.
    let bw = nl.div_ceil(8).max(1);
    let dslots: Vec<std::sync::OnceLock<(Vec<u8>, u64, BBox)>> =
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
                        std::thread::sleep(
                            std::time::Duration::from_millis(200),
                        );
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
                                rss()
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
                            let mut mems = 0u64;
                            let mut bx = BBox::EMPTY;
                            for r in &cell.rects {
                                let li = lidx[&(r.layer, r.dt)];
                                let (ex, ey) = rep_extent(&r.rep);
                                floe_ovm::bit_set(&mut mask, li);
                                bx.grow(&BBox {
                                    x0: r.x + ex.0.min(0),
                                    y0: r.y + ey.0.min(0),
                                    x1: r.x + r.w + ex.1.max(0),
                                    y1: r.y + r.h + ey.1.max(0),
                                });
                                let m = r.rep.members();
                                mems += m;
                                lr[li] += 1;
                                lm[li] += m;
                            }
                            for p in &cell.polys {
                                let li = lidx[&(p.layer, p.dt)];
                                let bb = pts_bbox(&p.pts);
                                let (ex, ey) = rep_extent(&p.rep);
                                floe_ovm::bit_set(&mut mask, li);
                                bx.grow(&BBox {
                                    x0: bb.x0 + ex.0.min(0),
                                    y0: bb.y0 + ey.0.min(0),
                                    x1: bb.x1 + ex.1.max(0),
                                    y1: bb.y1 + ey.1.max(0),
                                });
                                let m = p.rep.members();
                                mems += m;
                                lr[li] += 1;
                                lm[li] += m;
                            }
                            for pa in &cell.paths {
                                let li = lidx[&(pa.layer, pa.dt)];
                                let b4 = path_bbox(
                                    &pa.pts, pa.hw, pa.es, pa.ee,
                                );
                                let (ex, ey) = rep_extent(&pa.rep);
                                floe_ovm::bit_set(&mut mask, li);
                                bx.grow(&BBox {
                                    x0: b4.0 + ex.0.min(0),
                                    y0: b4.1 + ey.0.min(0),
                                    x1: b4.2 + ex.1.max(0),
                                    y1: b4.3 + ey.1.max(0),
                                });
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
                                bx.grow(&BBox {
                                    x0: t.x + ex.0.min(0),
                                    y0: t.y + ey.0.min(0),
                                    x1: t.x + ex.1.max(0),
                                    y1: t.y + ey.1.max(0),
                                });
                            }
                            let _ = dslots[ci].set((mask, mems, bx));
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
    let mut dmembers: Vec<u64> = Vec::with_capacity(n);
    let mut dbox: Vec<BBox> = Vec::with_capacity(n);
    for slot in dslots {
        let (m, mm, bx) =
            slot.into_inner().expect("direct slot unset");
        dmask.push(m);
        dmembers.push(mm);
        dbox.push(bx);
    }

    // ---- recursive fold in ONE topological pass (children before
    // parents), replacing the old O(depth) fixpoint: height,
    // recursive members, recursive layer mask. Result is identical to
    // the fixpoint's converged values.
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
    for &ci in &topo {
        let mut hm = 0u32;
        let mut mm = dmembers[ci];
        let mut mask = dmask[ci].clone();
        for pl in &doc.cells[ci].places {
            let c = pl.cell;
            hm = hm.max(height[c].checked_add(1).unwrap_or_else(
                || panic!("limit exceeded: hierarchy depth"),
            ));
            mm = mm.saturating_add(
                rmembers[c].saturating_mul(pl.rep.members()),
            );
            for (a, b) in mask.iter_mut().zip(&rmask[c]) {
                *a |= *b;
            }
        }
        height[ci] = hm;
        rmembers[ci] = mm;
        rmask[ci] = mask;
    }

    let mut b = Builder::new(doc.unit, size, mtime, nl);
    b.top = narrow_u32(doc.top as u64, "top cell index");
    for (i, &(l, d)) in doc.layer_order.iter().enumerate() {
        let nm = doc
            .layer_names
            .get(&(l, d))
            .cloned()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{}/{}", l, d));
        b.layer(l, d, &nm, lrecs[i], lmems[i]);
    }

    // ---- per-cell plans (instance BVH + page splitting) computed in
    // PARALLEL; the heavy per-record work leaves 31 cores idle when
    // serial. build_cell_plan is a pure fn of (doc, rbb, lidx), so
    // work-steal cells across threads into per-cell slots.
    let pslots: Vec<std::sync::OnceLock<CellPlan>> =
        (0..n).map(|_| std::sync::OnceLock::new()).collect();
    {
        let tp = std::time::Instant::now();
        let next = std::sync::atomic::AtomicUsize::new(0);
        let done = std::sync::atomic::AtomicUsize::new(0);
        let nthreads = jobs.max(1).min(n.max(1));
        std::thread::scope(|s| {
            {
                let done = &done;
                s.spawn(move || {
                    use std::sync::atomic::Ordering::Relaxed;
                    let mut last = tp;
                    loop {
                        std::thread::sleep(
                            std::time::Duration::from_millis(200),
                        );
                        if done.load(Relaxed) >= n {
                            return;
                        }
                        if last.elapsed().as_secs_f64() >= 5.0 {
                            last = std::time::Instant::now();
                            eprintln!(
                                "[vfs] build: cell plans {}/{} \
                                 ({}s, rss {})",
                                done.load(Relaxed),
                                n,
                                tp.elapsed().as_secs(),
                                rss()
                            );
                        }
                    }
                });
            }
            let pslots = &pslots;
            let next = &next;
            let done = &done;
            let rbb = &rbb;
            let lidx = &lidx;
            for _ in 0..nthreads {
                s.spawn(move || {
                    use std::sync::atomic::Ordering::Relaxed;
                    loop {
                        let ci = next.fetch_add(1, Relaxed);
                        if ci >= n {
                            break;
                        }
                        let plan =
                            build_cell_plan(doc, ci, rbb, lidx, nl);
                        let _ = pslots[ci].set(plan);
                        done.fetch_add(1, Relaxed);
                    }
                });
            }
        });
    }

    // ---- append plans serially in cell order (memcpy-bound): places
    // in leaf order, BVH nodes with the two bases fixed up, pages,
    // deduped bitsets, then the cell record. Same order as the old
    // single-threaded loop, so the .ovm/.ovp are byte-identical.
    let mut jobs_list: Vec<PageJob> = Vec::new();
    for (ci, slot) in pslots.into_iter().enumerate() {
        let CellPlan {
            place_order,
            bvh,
            mut pages,
            pranges,
            pbvh,
            pts_prep,
        } = slot.into_inner().expect("cell plan unset");
        let cell = &doc.cells[ci];
        let place_base = b.n_places();
        for &pi in &place_order {
            let pl = &cell.places[pi as usize];
            match &pts_prep[pi as usize] {
                Some(prep) => {
                    b.place_pts(
                        pl.cell as u32,
                        pl.x,
                        pl.y,
                        pl.rot,
                        pl.flip,
                        prep,
                    );
                }
                None => {
                    b.place(
                        pl.cell as u32,
                        pl.x,
                        pl.y,
                        pl.rot,
                        pl.flip,
                        &pl.rep,
                    );
                }
            }
        }
        let (bvh_start, bvh_count) = if bvh.is_empty() {
            (0, 0)
        } else {
            let base = b.n_bvh();
            for &(bb, first, count, leaf) in &bvh {
                let f = if leaf {
                    narrow_u32(place_base + first as u64, "place index")
                } else {
                    narrow_u32(base as u64 + first as u64, "bvh index")
                };
                b.bvh_node(&bb, f, count, leaf);
            }
            (base, narrow_u32(bvh.len() as u64, "cell bvh count"))
        };
        // page index == job index; phase 3 appends in job order
        let page_base = jobs_list.len() as u64;
        let page_start = narrow_u32(page_base, "page count");
        let page_count =
            narrow_u32(pages.len() as u64, "cell page count");
        // page BVH nodes + (cell,layer) runs: cell-local firsts
        // shift to the global bases here (same fixup pattern as the
        // instance BVH)
        let pb_base = b.n_pbvh();
        for &(bb, first, count, leaf, mw, mh) in &pbvh {
            let f = if leaf {
                narrow_u32(page_base + first as u64, "page index")
            } else {
                narrow_u32(
                    pb_base as u64 + first as u64,
                    "pbvh index",
                )
            };
            b.pbvh_node(&bb, f, count, leaf, mw, mh);
        }
        let pr_start = b.n_pranges();
        for &(li, lo, cnt, root) in &pranges {
            let r = if root == PBVH_NONE {
                PBVH_NONE
            } else {
                narrow_u32(pb_base as u64 + root as u64, "pbvh index")
            };
            b.prange(
                li,
                narrow_u32(page_base + lo as u64, "page index"),
                cnt,
                r,
            );
        }
        let pr_count =
            narrow_u32(pranges.len() as u64, "prange count");
        jobs_list.append(&mut pages);

        let mask_d = b.bitset(&dmask[ci]);
        let mask_r = b.bitset(&rmask[ci]);
        b.cell(
            &cell.name,
            height[ci],
            rank[ci],
            &dbox[ci],
            &win_to_bbox(rbb[ci]),
            narrow_u32(place_base, "place count"),
            narrow_u32(
                cell.places.len() as u64,
                "cell place count",
            ),
            page_start,
            page_count,
            bvh_start,
            bvh_count,
            pr_start,
            pr_count,
            mask_d,
            mask_r,
            rmembers[ci],
        );
    }

    // phase 2: encode page payloads in parallel (write_tree +
    // deflate-6 is the build's hot cost; jobs are independent)
    let mut payloads: Vec<(Vec<u8>, u64)> = Vec::new();
    payloads.resize_with(jobs_list.len(), Default::default);
    let ptotal = jobs_list.len();
    if jobs > 1 && ptotal > 1 {
        // disjoint chunks_mut slices = safe parallel fill; pages are
        // ~uniform (1 MB target) so static chunking balances well
        let te = std::time::Instant::now();
        let done = std::sync::atomic::AtomicUsize::new(0);
        let chunk = ptotal.div_ceil(jobs).max(1);
        std::thread::scope(|s| {
            {
                let done = &done;
                s.spawn(move || {
                    use std::sync::atomic::Ordering::Relaxed;
                    let mut last = te;
                    loop {
                        std::thread::sleep(
                            std::time::Duration::from_millis(200),
                        );
                        if done.load(Relaxed) >= ptotal {
                            return;
                        }
                        if last.elapsed().as_secs_f64() >= 5.0 {
                            last = std::time::Instant::now();
                            eprintln!(
                                "[vfs] build: encoding pages {}/{} \
                                 ({}s, rss {})",
                                done.load(Relaxed),
                                ptotal,
                                te.elapsed().as_secs(),
                                rss()
                            );
                        }
                    }
                });
            }
            let done = &done;
            for (jc, pc) in jobs_list
                .chunks(chunk)
                .zip(payloads.chunks_mut(chunk))
            {
                s.spawn(move || {
                    use std::sync::atomic::Ordering::Relaxed;
                    for (job, slot) in jc.iter().zip(pc.iter_mut())
                    {
                        *slot = encode_job(doc, job);
                        done.fetch_add(1, Relaxed);
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
    for (job, (payload, raw)) in jobs_list.iter().zip(&payloads) {
        std::io::Write::write_all(&mut ovp, payload)
            .expect("write ovp");
        b.page(
            narrow_u32(job.ci as u64, "cell index"),
            job.li,
            job.seq,
            &job.bbox,
            ovp_off,
            payload.len() as u64,
            *raw,
            job.recs.len() as u64,
            job.members,
            job.max_w.max(0) as u64,
            job.max_h.max(0) as u64,
        );
        ovp_off += payload.len() as u64;
        pages_bytes += payload.len() as u64;
    }
    ovp.flush().expect("flush ovp");
    // the caller writes design.ovm LAST (commit marker, after the
    // viewer-side files); ovp_len rides in the header so open can
    // verify the ovm/ovp pair
    let ovm_bytes = b.finish(ovp_off);
    eprintln!(
        "[vfs] {} pages ({}) + ovm {} in {:.1}s",
        pages_total,
        fmt_size(pages_bytes),
        fmt_size(ovm_bytes.len() as u64),
        t0.elapsed().as_secs_f64()
    );
    ovm_bytes
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
    let (dir, view, px, cut, depth, layers, rest) =
        parse_common(args);
    // --mode hier|flat: A/B switch (VFS_HIER.md par.3.5); flat
    // stays the default until M5 retires it
    let mode = rest
        .iter()
        .find(|(k, _)| k == "--mode")
        .map(|(_, v)| v.as_str())
        .unwrap_or("flat");
    let v = floe_vfs::Vfs::open(&dir).unwrap_or_else(|e| {
        eprintln!("{}", e);
        std::process::exit(1);
    });
    let req = make_req(
        &v,
        view.expect("--view required"),
        px,
        cut,
        depth,
        layers.as_deref(),
    );
    if mode == "hier" {
        let t0 = std::time::Instant::now();
        let plan = v.plan_hier(&req);
        let ms = t0.elapsed().as_secs_f64() * 1e3;
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
            "{{\n  \"mode\": \"hier\",\n  \"pages\": {},\n  \
             \"compressed_bytes\": {},\n  \"encoded_bytes\": {},\n  \
             \"records\": {},\n  \"members\": {},\n  \
             \"wc_cells\": {},\n  \"wc_variants\": {},\n  \
             \"inst_edges\": {},\n  \"frame_rects\": {},\n  \
             \"visited_bvh\": {},\n  \
             \"culled_subtrees_size\": {},\n  \
             \"culled_subtrees_layer\": {},\n  \
             \"culled_pages_size\": {},\n  \
             \"culled_page_layer_roots\": {},\n  \
             \"culled_page_bvh_bbox\": {},\n  \
             \"culled_page_bvh_cut\": {},\n  \
             \"visited_page_bvh\": {},\n  \
             \"page_candidates\": {},\n  \
             \"pts_enumerated\": {},\n  \"pts_fallback\": {},\n  \
             \"pts_offsets_scanned\": {},\n  \
             \"pts_selected\": {},\n  \
             \"pts_offsets_emitted\": {},\n  \
             \"pts_bytes_emitted\": {},\n  \
             \"grid_fallback_full\": {},\n  \
             \"kbox_merges\": {},\n  \"plan_ms\": {:.2}\n}}",
            plan.pages.len(),
            cbytes,
            ubytes,
            records,
            members,
            st.wc_cells,
            st.wc_variants,
            st.inst_edges,
            st.frame_rects,
            st.visited_bvh,
            st.cull_size,
            st.cull_layer,
            st.cull_page_size,
            st.culled_page_layer_roots,
            st.culled_page_bvh_bbox,
            st.culled_page_bvh_cut,
            st.visited_page_bvh,
            st.page_candidates,
            st.pts_enumerated,
            st.pts_fallback,
            st.pts_offsets_scanned,
            st.pts_selected,
            st.pts_offsets_emitted,
            st.pts_bytes_emitted,
            st.grid_fallback_full,
            st.kbox_merges,
            ms
        );
        return;
    }
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
    // progressive first paint: cap the NEW payload per response so
    // the client renders in ~1s rounds instead of one long parse
    // (VFS_HIER.md par.5 M3.5). 0 disables.
    let mut stream_kb: u64 = 24576;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--budget-mb" => {
                budget_mb = args[i + 1].parse().expect("budget");
                i += 2;
            }
            "--stream-kb" => {
                stream_kb = args[i + 1].parse().expect("stream");
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
    let v = floe_vfs::Vfs::open(&dir).unwrap_or_else(|e| {
        eprintln!("{}", e);
        std::process::exit(1);
    });
    let mut d = Daemon {
        v: &v,
        flat: floe_vfs::Session::new(budget_mb << 20),
        hier: floe_vfs::HierSession::new(budget_mb << 20),
        names_sent: false,
        mode_lock: None,
        stream_budget: stream_kb << 10,
    };
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
        match serve_one(&mut d, line) {
            Ok(resp) => println!("{}", resp),
            Err(e) => println!("error={}", e.replace(' ', "_")),
        }
        use std::io::Write as _;
        std::io::stdout().flush().ok();
    }
}

/// one daemon run's state: the legacy flat session and the hier
/// 2-phase session coexist for the M1-M4 A/B window, but a run
/// locks to whichever session-mode speaks first (mixing would split
/// the residency ledger across formats)
struct Daemon<'a> {
    v: &'a floe_vfs::Vfs,
    flat: floe_vfs::Session,
    hier: floe_vfs::HierSession,
    names_sent: bool,
    mode_lock: Option<bool>, // true = hier
    /// per-response cap on new payload bytes (0 = off)
    stream_budget: u64,
}

fn serve_one(d: &mut Daemon, line: &str) -> Result<String, String> {
    let mut gen = 0u64;
    let mut view: Option<(f64, f64, f64, f64)> = None;
    let mut px = 5.0f64;
    let mut cut = 2.0f64;
    let mut depth = u32::MAX;
    let mut layers: Option<Vec<String>> = None;
    let mut out: Option<String> = None;
    let mut mode = String::new();
    let mut ack = 0u64;
    let mut reset = false;
    let mut stream_kb: Option<u64> = None;
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
            "mode" => mode = val.to_string(),
            "ack" => ack = val.parse().map_err(|_| "ack")?,
            "reset" => reset = val == "1",
            "stream" => {
                stream_kb =
                    Some(val.parse().map_err(|_| "stream")?)
            }
            _ => return Err(format!("unknown key {}", k)),
        }
    }
    let hier_mode = mode == "hier" || mode == "hier_probe";
    let probe = mode == "probe" || mode == "hier_probe";
    if !probe {
        // session requests lock the run's format; probes are
        // session-less and free to mix
        match d.mode_lock {
            None => d.mode_lock = Some(hier_mode),
            Some(m) if m != hier_mode => {
                return Err("mode_switch".into())
            }
            _ => {}
        }
    }
    let view = view.ok_or("view required")?;
    let out = out.ok_or("out required")?;
    let req =
        make_req(d.v, view, px, cut, depth, layers.as_deref());
    if hier_mode {
        return serve_hier(
            d, &req, gen, ack, reset, probe, stream_kb, &out,
        );
    }
    let v = d.v;
    let sess = &mut d.flat;
    let t0 = std::time::Instant::now();
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

/// hier-mode request (VFS_HIER.md par.3.5/3.7): resolve the ack (or
/// reset) FIRST, then plan, then record this response as the new
/// pending txn. The response carries the WC top name and, once per
/// daemon run, the ci->design-name table.
#[allow(clippy::too_many_arguments)]
fn serve_hier(
    d: &mut Daemon,
    req: &floe_vfs::ViewReq,
    gen: u64,
    ack: u64,
    reset: bool,
    probe: bool,
    stream_kb: Option<u64>,
    out: &str,
) -> Result<String, String> {
    let v = d.v;
    let t0 = std::time::Instant::now();
    if !probe {
        if reset {
            d.hier.reset();
        } else {
            d.hier.resolve_ack(ack)?;
        }
    }
    let plan = v.plan_hier(req);
    let upd = if probe {
        // session-less exact query: the delta carries EVERY planned
        // page, the working-set ledger is untouched
        floe_vfs::HierUpdate {
            new: plan.pages.clone(),
            ..Default::default()
        }
    } else {
        // per-request override (the client adapts the budget to its
        // measured parse speed); absent = daemon flag default
        let budget = stream_kb
            .map(|kb| kb << 10)
            .unwrap_or(d.stream_budget);
        d.hier.apply(
            &v.ovm,
            &plan.pages,
            &plan.page_prio,
            gen,
            budget,
        )?
    };
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let (delta_path, top) = if plan.wcells.is_empty() {
        ("-".to_string(), "-".to_string())
    } else {
        // partial deltas reference only MATERIALIZED pages:
        // committed-in-ledger + this round's new (a reference with
        // no definition anywhere would mint an unevictable empty
        // ghost cell in the viewer)
        let avail: Option<std::collections::HashSet<u32>> =
            if upd.partial {
                let mut a: std::collections::HashSet<u32> =
                    upd.new.iter().copied().collect();
                for &pi in &plan.pages {
                    if !probe && d.hier.is_committed(pi) {
                        a.insert(pi);
                    }
                }
                Some(a)
            } else {
                None
            };
        let bytes =
            v.delta_hier(&plan, &upd.new, gen, avail.as_ref())?;
        let p = format!("{}/delta_{}.oas", out, gen);
        std::fs::write(&p, &bytes).map_err(|e| e.to_string())?;
        (p, floe_vfs::hier::ws_name(gen, plan.top))
    };
    // ci -> design-name table, once per daemon run (par.3.4); the
    // client loads it into memory and deletes the file
    let names_path = if d.names_sent {
        "-".to_string()
    } else {
        let mut w = String::new();
        for ci in 0..v.ovm.n_cells {
            w.push_str(&format!(
                "{}\t{}\n",
                ci,
                v.ovm.cell(ci).name
            ));
        }
        let p = format!("{}/names_{}.tsv", out, gen);
        std::fs::write(&p, w).map_err(|e| e.to_string())?;
        d.names_sent = true;
        p
    };
    let evict: Vec<String> =
        upd.evict.iter().map(|&pi| v.page_name(pi)).collect();
    let (mut bytes, mut members) = (0u64, 0u64);
    for &pi in &plan.pages {
        let p = v.ovm.page(pi);
        bytes += p.csize as u64;
        members += p.members;
    }
    let mb = |x: u64| x as f64 / (1 << 20) as f64;
    let st = &plan.stats;
    Ok(format!(
        "gen={} pages={} new={} evict={} delta={} top={} names={} \
         bytes={} members={} plan_ms={:.2} wc_cells={} \
         inst_edges={} frame_rects={} partial={} deferred={} \
         resident_committed_mb={:.1} resident_projected_mb={:.1} \
         pending_new_mb={:.1} pending_evict_mb={:.1}",
        gen,
        plan.pages.len(),
        upd.new.len(),
        if evict.is_empty() {
            "-".to_string()
        } else {
            evict.join(",")
        },
        delta_path,
        top,
        names_path,
        bytes,
        members,
        t0.elapsed().as_secs_f64() * 1e3,
        st.wc_cells,
        st.inst_edges,
        st.frame_rects,
        upd.partial as u8,
        upd.deferred,
        mb(upd.committed_bytes),
        mb(upd.projected_bytes),
        mb(upd.pending_new_bytes),
        mb(upd.pending_evict_bytes),
    ))
}
