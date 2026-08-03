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

const DEFAULT_PAGE_TARGET_MB: u64 = 1;
const MIB: u64 = 1 << 20;
/// Encoded payloads are retained for at most one ordered batch. The old
/// implementation encoded every page before writing design.ovp, so peak RSS
/// included the complete payload file (tens of GB on the 9.8G asset).
const ENCODE_BATCH_PER_JOB: usize = 2;
const ENCODE_BATCH_MAX: usize = 256;
/// Completed CellPlans retain fragment arenas and Morton-prepared placement
/// points. Bound the default independently from CPU count; `--plan-batch`
/// remains available for hosts that deliberately trade RAM for throughput.
const PLAN_BATCH_MAX: usize = 16;
const BVH_LEAF: usize = 8;
/// pages per page-BVH leaf, and the run size at or below which a
/// (cell,layer) run gets no BVH at all (linear scan, root = NONE)
const PBVH_LEAF: usize = 8;


// ------------------------------------------------------------ build

pub fn vfs_cmd(args: &[String]) {
    let mut src: Option<String> = None;
    let mut outdir: Option<String> = None;
    let mut jobs: Option<usize> = None;
    let mut encode_batch: Option<usize> = None;
    let mut plan_batch: Option<usize> = None;
    let mut page_target_mb: Option<u64> = None;
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
            "--encode-batch" => {
                encode_batch = Some(
                    args[i + 1]
                        .parse::<usize>()
                        .expect("encode batch")
                        .max(1),
                );
                i += 2;
            }
            "--plan-batch" => {
                plan_batch = Some(
                    args[i + 1]
                        .parse::<usize>()
                        .expect("plan batch")
                        .max(1),
                );
                i += 2;
            }
            "--page-target-mb" => {
                let mb = args[i + 1]
                    .parse::<u64>()
                    .expect("page target MB");
                assert!(mb > 0, "page target MB must be positive");
                page_target_mb = Some(mb);
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
    let encode_batch = encode_batch.unwrap_or_else(|| {
        jobs.max(1)
            .saturating_mul(ENCODE_BATCH_PER_JOB)
            .min(ENCODE_BATCH_MAX)
    });
    let plan_batch =
        plan_batch.unwrap_or_else(|| jobs.max(1).min(PLAN_BATCH_MAX));
    let page_target_mb =
        page_target_mb.unwrap_or(DEFAULT_PAGE_TARGET_MB);
    let page_target_bytes = page_target_mb
        .checked_mul(MIB)
        .expect("limit exceeded: page target bytes");
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
    let doc = match floe_oasis::doc::parse_doc_parallel_phased(
        &data,
        jobs,
        |_| {
            eprintln!(
                "[vfs] syntax parsed in {:.1}s (source still resident, \
                 rss {})",
                t1.elapsed().as_secs_f64(),
                rss()
            );
        },
    ) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("parse {}: {}", src, e);
            std::process::exit(1);
        }
    };
    parsing.store(false, std::sync::atomic::Ordering::Relaxed);
    eprintln!(
        "[vfs] repetition normalization complete in {:.1}s \
         (rss {})",
        doc.norm_s,
        rss()
    );
    drop(data);
    eprintln!(
        "[vfs] parsed {} cells in {:.1}s ({} threads, source released, \
         rss {})",
        doc.cells.len(),
        t1.elapsed().as_secs_f64(),
        jobs,
        rss()
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
        let (ovm_bytes, rbb, lmems) = build(
            &doc,
            size,
            mtime,
            &outdir,
            jobs,
            encode_batch,
            plan_batch,
            page_target_bytes,
        );
        if kill_at.as_deref() == Some("ovp-written") {
            eprintln!("[vfs] FLOE_KILL_AT=ovp-written");
            std::process::exit(9);
        }
        emit_viewer_side(&doc, &src, size, mtime, &outdir, &rbb,
                         &lmems);
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
    rbb: &[Option<(i64, i64, i64, i64)>],
    lmems: &[u64],
) {
    let t0 = std::time::Instant::now();
    // staged heartbeat: these are serial passes and on a 9.8G-class
    // chip each takes minutes - silence here read as a hang (field
    // report during M4)
    let stage = std::sync::Arc::new(std::sync::Mutex::new(
        "collecting texts",
    ));
    let hb_on = std::sync::Arc::new(
        std::sync::atomic::AtomicBool::new(true),
    );
    {
        let stage = stage.clone();
        let hb_on = hb_on.clone();
        std::thread::spawn(move || {
            use std::sync::atomic::Ordering::Relaxed;
            let t = std::time::Instant::now();
            let mut last = 0u64;
            loop {
                std::thread::sleep(
                    std::time::Duration::from_millis(200),
                );
                if !hb_on.load(Relaxed) {
                    return;
                }
                let e = t.elapsed().as_secs();
                if e >= last + 5 {
                    last = e;
                    eprintln!(
                        "[vfs] viewer-side: {}... ({}s, rss {})",
                        *stage.lock().unwrap(),
                        e,
                        rss()
                    );
                }
            }
        });
    }
    let set_stage = |s: &'static str| {
        *stage.lock().unwrap() = s;
    };
    let entries = floe_tiler::skel::collect_all_texts(doc);
    set_stage("building skeleton");
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

    // meta.json (VFS flavor). rbb comes from the build (the
    // recursive-bbox pass ran there once already - re-walking every
    // record serially here doubled the silent tail on big chips)
    set_stage("meta");
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
    for (i, &(l, d)) in doc.layer_order.iter().enumerate() {
        if lmems[i] > 0 {
            stored.insert((l, d), lmems[i]);
        }
    }
    for cell in &doc.cells {
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
    hb_on.store(false, std::sync::atomic::Ordering::Relaxed);
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
    bbox: BBox, // rep-subset extent (page bbox / spatial split)
    bytes: u32, // pre-encode estimate
    members: u64,
    w: i64, // single-feature dims (cut criterion, rep excluded)
    h: i64,
    frag: Frag,
}

/// which subset of the source record's repetition this PRec covers.
/// Fragmentation is what keeps page bboxes honest on rep-heavy
/// layers: a fill record repeated across the whole die otherwise
/// drags its page's bbox out to die width, and once every page of
/// the layer holds one such record the planner (correctly) selects
/// them ALL for any viewport - the 9.8G field floor: a point view
/// at depth 0 pulled 2151 pages / ~100 s of parse.
#[derive(Clone, Copy)]
enum Frag {
    Whole,
    /// [lo,hi) into Arena[arena] offsets (reordered in place while
    /// splitting; sibling ranges are disjoint by construction)
    Pts { arena: u32, lo: u32, hi: u32 },
    /// index sub-rectangle [i0,i1) x [j0,j1) of a Grid rep
    Grid { i0: u64, i1: u64, j0: u64, j1: u64 },
}

/// Per-record scratch for fragmented Pts reps. The source offsets stay in the
/// parsed Doc; only a u32 permutation is partitioned in place. The former
/// coordinate copy cost 16 bytes/member and was retained for every planned
/// cell, while this costs 4 bytes/member and is released after one cell batch.
struct PtsArena {
    shape_box: BBox,
    order: Vec<u32>,
}

type Arena = Vec<PtsArena>;

#[derive(Default, Clone, Copy)]
struct SplitStats {
    fragments: u64,
    oversize_pages: u64,
    depth_capped: u64,
    lod_pages: u64,
}

/// hard recursion cap (records above it emit as-is + stat)
const SPLIT_MAX_DEPTH: u32 = 64;

fn rep_est_n(n: u64) -> u32 {
    (4u64.saturating_mul(n)).min(u32::MAX as u64) as u32
}

fn rep_est(rep: &Rep) -> u32 {
    match rep {
        Rep::One => 0,
        Rep::Grid { .. } => 10,
        Rep::Pts(p) => rep_est_n(p.len() as u64),
    }
}

fn rec_rep<'a>(cell: &'a floe_oasis::doc::Cell, r: &PRec) -> &'a Rep {
    match r.kind {
        0 => &cell.rects[r.idx as usize].rep,
        1 => &cell.polys[r.idx as usize].rep,
        _ => &cell.paths[r.idx as usize].rep,
    }
}

/// shape bbox at the rep origin (rep extent excluded)
fn rec_shape_box(cell: &floe_oasis::doc::Cell, r: &PRec) -> BBox {
    match r.kind {
        0 => {
            let x = &cell.rects[r.idx as usize];
            BBox { x0: x.x, y0: x.y, x1: x.x + x.w, y1: x.y + x.h }
        }
        1 => pts_bbox(&cell.polys[r.idx as usize].pts),
        _ => {
            let pa = &cell.paths[r.idx as usize];
            let b = path_bbox(&pa.pts, pa.hw, pa.es, pa.ee);
            BBox { x0: b.0, y0: b.1, x1: b.2, y1: b.3 }
        }
    }
}

fn axis_c2(b: &BBox, ax: u8) -> i64 {
    if ax == 0 { b.x0 + b.x1 } else { b.y0 + b.y1 }
}

fn axis_lo(b: &BBox, ax: u8) -> i64 {
    if ax == 0 { b.x0 } else { b.y0 }
}

fn axis_hi(b: &BBox, ax: u8) -> i64 {
    if ax == 0 { b.x1 } else { b.y1 }
}

/// world bbox of a grid index sub-rectangle: the lattice offset is
/// linear in (i,j), so min/max are attained at the four corners -
/// valid for skew vectors too
fn grid_frag_bbox(
    sb: &BBox,
    va: (i64, i64),
    vb: (i64, i64),
    i0: u64,
    i1: u64,
    j0: u64,
    j1: u64,
) -> BBox {
    let (i0, im) = (i0 as i64, (i1 - 1) as i64);
    let (j0, jm) = (j0 as i64, (j1 - 1) as i64);
    let mut bb = BBox::EMPTY;
    for &(i, j) in &[(i0, j0), (im, j0), (i0, jm), (im, jm)] {
        let ox = i * va.0 + j * vb.0;
        let oy = i * va.1 + j * vb.1;
        bb.grow(&BBox {
            x0: sb.x0 + ox,
            y0: sb.y0 + oy,
            x1: sb.x1 + ox,
            y1: sb.y1 + oy,
        });
    }
    bb
}

fn pts_order_bbox(
    sb: &BBox,
    pts: &[(i64, i64)],
    order: &[u32],
) -> BBox {
    let mut bb = BBox::EMPTY;
    for &i in order {
        let (ox, oy) = pts[i as usize];
        bb.grow(&BBox {
            x0: sb.x0 + ox,
            y0: sb.y0 + oy,
            x1: sb.x1 + ox,
            y1: sb.y1 + oy,
        });
    }
    bb
}

fn can_frag(cell: &floe_oasis::doc::Cell, r: &PRec) -> bool {
    r.members >= 2
        && matches!(rec_rep(cell, r), Rep::Grid { .. } | Rep::Pts(_))
}

/// split r's repetition members along `ax` at `plane2` (doubled
/// world coordinate). OWNERSHIP: member center*2 < plane2 -> left,
/// >= plane2 -> right (duplicates each classified individually, so
/// multiset counts are conserved). Returns None when the record
/// cannot usefully split - Rep::One, a coincident pile (one side
/// would be empty), or a grid orthogonal to the axis - and the
/// caller keeps it whole on its center's side.
fn frag_split(
    cell: &floe_oasis::doc::Cell,
    arena: &mut Arena,
    r: &PRec,
    ax: u8,
    plane2: i64,
) -> Option<(PRec, PRec)> {
    let child = |bbox: BBox, members: u64, bytes: u32, frag: Frag| {
        PRec { bbox, bytes, members, frag, ..*r }
    };
    match (r.frag, rec_rep(cell, r)) {
        (Frag::Grid { .. }, Rep::Grid { na: _, nb: _, va, vb })
        | (Frag::Whole, Rep::Grid { na: _, nb: _, va, vb }) => {
            let (va, vb) = (*va, *vb);
            let (i0, i1, j0, j1) = match r.frag {
                Frag::Grid { i0, i1, j0, j1 } => (i0, i1, j0, j1),
                _ => match rec_rep(cell, r) {
                    Rep::Grid { na, nb, .. } => (0, *na, 0, *nb),
                    _ => unreachable!(),
                },
            };
            // pick the lattice dimension with the larger extent
            // contribution along ax (u128: |v|*(n-1) can pass i64)
            let cai = (if ax == 0 { va.0 } else { va.1 })
                .unsigned_abs() as u128
                * (i1 - i0 - 1) as u128;
            let caj = (if ax == 0 { vb.0 } else { vb.1 })
                .unsigned_abs() as u128
                * (j1 - j0 - 1) as u128;
            if cai == 0 && caj == 0 {
                return None; // orthogonal: no reduction along ax
            }
            let sb = rec_shape_box(cell, r);
            let split_i = (cai >= caj && i1 - i0 >= 2)
                || j1 - j0 < 2;
            let (l, rt) = if split_i {
                if i1 - i0 < 2 {
                    return None;
                }
                let m = (i0 + i1) / 2;
                ((i0, m, j0, j1), (m, i1, j0, j1))
            } else {
                let m = (j0 + j1) / 2;
                ((i0, i1, j0, m), (i0, i1, m, j1))
            };
            let base = r.bytes.saturating_sub(10);
            let mk = |(a, b, c, d): (u64, u64, u64, u64)| {
                child(
                    grid_frag_bbox(&sb, va, vb, a, b, c, d),
                    (b - a) * (d - c),
                    base.saturating_add(10),
                    Frag::Grid { i0: a, i1: b, j0: c, j1: d },
                )
            };
            Some((mk(l), mk(rt)))
        }
        (Frag::Pts { .. }, _) | (Frag::Whole, Rep::Pts(_)) => {
            let (a, lo, hi) = match r.frag {
                Frag::Pts { arena, lo, hi } => {
                    (arena as usize, lo as usize, hi as usize)
                }
                _ => {
                    let pts = match rec_rep(cell, r) {
                        Rep::Pts(p) => p,
                        _ => unreachable!(),
                    };
                    let sb = rec_shape_box(cell, r);
                    let count = narrow_u32(
                        pts.len() as u64,
                        "fragment pts count",
                    );
                    arena.push(PtsArena {
                        shape_box: sb,
                        order: (0..count).collect(),
                    });
                    (arena.len() - 1, 0, pts.len())
                }
            };
            let pts = match rec_rep(cell, r) {
                Rep::Pts(p) => p,
                _ => unreachable!(),
            };
            let (sb, order) = {
                let e = &mut arena[a];
                (e.shape_box, &mut e.order)
            };
            let sc2 = axis_c2(&sb, ax);
            let off_ax =
                |p: (i64, i64)| if ax == 0 { p.0 } else { p.1 };
            let mut m = lo;
            for k in lo..hi {
                if 2 * off_ax(pts[order[k] as usize]) + sc2 < plane2 {
                    order.swap(k, m);
                    m += 1;
                }
            }
            if m == lo || m == hi {
                return None; // coincident pile: one side empty
            }
            let base =
                r.bytes.saturating_sub(rep_est_n(r.members));
            let bl = pts_order_bbox(&sb, pts, &order[lo..m]);
            let br = pts_order_bbox(&sb, pts, &order[m..hi]);
            let mk = |bb: BBox, s: usize, e: usize| {
                child(
                    bb,
                    (e - s) as u64,
                    base.saturating_add(rep_est_n((e - s) as u64)),
                    Frag::Pts {
                        arena: narrow_u32(
                            a as u64,
                            "fragment arena index",
                        ),
                        lo: narrow_u32(s as u64, "fragment pts index"),
                        hi: narrow_u32(e as u64, "fragment pts index"),
                    },
                )
            };
            Some((mk(bl, lo, m), mk(br, m, hi)))
        }
        _ => None,
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
    /// LOD_EXACT or LOD_MERGED (M7 coverage variant)
    lod: u8,
    /// exact page -> its LOD twin. Cell-local page index at plan
    /// time; the append loop rebases it to the global page index
    /// (same fixup pattern as pranges/pbvh).
    lod_page: u32,
    /// LOD jobs only: grid-merged coverage rectangles (small
    /// members fused; `recs` then holds the verbatim passthrough
    /// records)
    lod_rects: Vec<RectRec>,
}

fn emit_page(
    ci: usize,
    li: u32,
    recs: Vec<PRec>,
    seq: &mut u32,
    out: &mut Vec<PageJob>,
) {
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
        recs,
        bbox: bb,
        members,
        max_w,
        max_h,
        lod: floe_ovm::LOD_EXACT,
        lod_page: floe_ovm::LOD_PAGE_NONE,
        lod_rects: Vec::new(),
    });
    *seq = seq.checked_add(1).expect("page seq overflow");
}

/// spatial-split a layer's records into page groups. Straddling
/// Grid/Pts records are FRAGMENTED at the split plane (par. Frag)
/// instead of dragging a whole-die bbox onto one side; a single
/// over-target rep record splits even alone (the old
/// recs.len()>MIN gate left one huge Pts record as one page).
/// Ownership at the plane: center*2 < plane2 -> left, else right.
#[allow(clippy::too_many_arguments)]
fn split_pages(
    cell: &floe_oasis::doc::Cell,
    arena: &mut Arena,
    ci: usize,
    li: u32,
    recs: Vec<PRec>,
    seq: &mut u32,
    out: &mut Vec<PageJob>,
    st: &mut SplitStats,
    page_target_bytes: u64,
    depth: u32,
) {
    let bytes: u64 = recs.iter().map(|r| r.bytes as u64).sum();
    // the split gate is BYTES, not record count - with repetitions
    // a handful of records can be gigabytes, and page decode cost
    // is what the viewer pays. A lone record still splits when its
    // rep can fragment.
    let splittable = recs.len() >= 2
        || recs.iter().any(|r| can_frag(cell, r));
    if bytes <= page_target_bytes || !splittable {
        emit_page(ci, li, recs, seq, out);
        return;
    }
    if depth >= SPLIT_MAX_DEPTH {
        st.depth_capped += 1;
        emit_page(ci, li, recs, seq, out);
        return;
    }
    let mut bb = BBox::EMPTY;
    for r in recs.iter() {
        bb.grow(&r.bbox);
    }
    let ax: u8 = if bb.x1 - bb.x0 >= bb.y1 - bb.y0 { 0 } else { 1 };
    let mut recs = recs;
    let mid = recs.len() / 2;
    recs.select_nth_unstable_by_key(mid, |r| axis_c2(&r.bbox, ax));
    let plane2 = axis_c2(&recs[mid].bbox, ax);
    let node_ext = axis_hi(&bb, ax) - axis_lo(&bb, ax);
    let mut lv: Vec<PRec> = Vec::with_capacity(mid + 1);
    let mut rv: Vec<PRec> = Vec::with_capacity(recs.len() - mid);
    let mut wide: Vec<PRec> = Vec::new();
    for r in recs {
        let crosses = axis_lo(&r.bbox, ax) * 2 < plane2
            && axis_hi(&r.bbox, ax) * 2 > plane2;
        if crosses {
            // EVERY crossing rep fragments - a byte floor here let
            // 24-byte two-member scatter records (klayout folding
            // leftovers) stay whole and drag quadrant pages back
            // out to die width. The cost is one duplicated base
            // header per split; the alternative is a poisoned page
            // bbox.
            if let Some((a, b)) =
                frag_split(cell, arena, &r, ax, plane2)
            {
                st.fragments += 1;
                lv.push(a);
                rv.push(b);
                continue;
            }
            // unsplittable crosser (Rep::One, coincident pile,
            // axis-orthogonal grid): if it spans most of this
            // node, QUARANTINE it into an oversize page at this
            // level - center-assigning it would poison a child's
            // bbox at every scale (die rings, long spines).
            // Smaller crossers stay: the slack they add is
            // bounded by half the node extent.
            let ext = axis_hi(&r.bbox, ax) - axis_lo(&r.bbox, ax);
            if ext * 2 > node_ext {
                wide.push(r);
                continue;
            }
        }
        if axis_c2(&r.bbox, ax) < plane2 {
            lv.push(r);
        } else {
            rv.push(r);
        }
    }
    // oversize pages emit at this node, packed by bytes
    let mut acc: Vec<PRec> = Vec::new();
    let mut acc_bytes = 0u64;
    for r in wide {
        if acc_bytes + r.bytes as u64 > page_target_bytes
            && !acc.is_empty()
        {
            st.oversize_pages += 1;
            emit_page(ci, li, std::mem::take(&mut acc), seq, out);
            acc_bytes = 0;
        }
        acc_bytes += r.bytes as u64;
        acc.push(r);
    }
    if !acc.is_empty() {
        st.oversize_pages += 1;
        emit_page(ci, li, acc, seq, out);
    }
    if lv.is_empty() && rv.is_empty() {
        return;
    }
    if lv.is_empty() || rv.is_empty() {
        // nothing separable at this plane (coincident pile):
        // over-target but spatially irreducible - emit as-is
        lv.extend(rv);
        emit_page(ci, li, lv, seq, out);
        return;
    }
    split_pages(
        cell,
        arena,
        ci,
        li,
        lv,
        seq,
        out,
        st,
        page_target_bytes,
        depth + 1,
    );
    split_pages(
        cell,
        arena,
        ci,
        li,
        rv,
        seq,
        out,
        st,
        page_target_bytes,
        depth + 1,
    );
}

/// materialized fragment repetition: base shift + subset rep. Pts
/// subsets REBASE - the first offset must be (0,0) (doc.rs Rep::Pts
/// invariant, and write.rs emits successive g-deltas from it); a
/// 1-member subset collapses to Rep::One. Grid subsets shift the
/// base to the (i0,j0) lattice corner; a collapsed i-dimension
/// swaps the vectors so `na >= 2` holds (the writer's nb==1 arms
/// encode na-2 as a uint).
fn frag_rep(
    rep: &Rep,
    frag: &Frag,
    arena: &Arena,
) -> Option<((i64, i64), Rep)> {
    match *frag {
        Frag::Whole => None,
        Frag::Pts { arena: a, lo, hi } => {
            let src = match rep {
                Rep::Pts(p) => p,
                _ => unreachable!("pts frag on non-pts"),
            };
            let order = &arena[a as usize].order
                [lo as usize..hi as usize];
            let (bx, by) = src[order[0] as usize];
            if order.len() == 1 {
                return Some(((bx, by), Rep::One));
            }
            let sub: Vec<(i64, i64)> = order
                .iter()
                .map(|&i| {
                    let (x, y) = src[i as usize];
                    (x - bx, y - by)
                })
                .collect();
            Some(((bx, by), Rep::Pts(sub.into())))
        }
        Frag::Grid { i0, i1, j0, j1 } => {
            let (va, vb) = match rep {
                Rep::Grid { va, vb, .. } => (*va, *vb),
                _ => unreachable!("grid frag on non-grid"),
            };
            let dx = i0 as i64 * va.0 + j0 as i64 * vb.0;
            let dy = i0 as i64 * va.1 + j0 as i64 * vb.1;
            let (ni, nj) = (i1 - i0, j1 - j0);
            let rep = if ni == 1 && nj == 1 {
                Rep::One
            } else if ni == 1 {
                Rep::Grid { na: nj, nb: 1, va: vb, vb: va }
            } else {
                Rep::Grid { na: ni, nb: nj, va, vb }
            };
            Some(((dx, dy), rep))
        }
    }
}

/// encode one page job to its OASIS payload (parallel-safe: reads
/// doc immutably, allocates only its own buffers)
fn encode_job(
    doc: &Doc,
    job: &PageJob,
    arena: &Arena,
) -> (Vec<u8>, u64) {
    let cell = &doc.cells[job.ci];
    let mut rects: Vec<RectRec> = Vec::new();
    let mut polys: Vec<PolyRec> = Vec::new();
    let mut paths: Vec<PathRec> = Vec::new();
    for r in &job.recs {
        match r.kind {
            0 => {
                let mut x = cell.rects[r.idx as usize].clone();
                if let Some(((dx, dy), rep)) =
                    frag_rep(&x.rep, &r.frag, arena)
                {
                    x.x += dx;
                    x.y += dy;
                    x.rep = rep;
                }
                rects.push(x);
            }
            1 => {
                let mut po = cell.polys[r.idx as usize].clone();
                if let Some(((dx, dy), rep)) =
                    frag_rep(&po.rep, &r.frag, arena)
                {
                    for q in po.pts.iter_mut() {
                        q.0 += dx;
                        q.1 += dy;
                    }
                    po.rep = rep;
                }
                polys.push(po);
            }
            _ => {
                let mut pa = cell.paths[r.idx as usize].clone();
                if let Some(((dx, dy), rep)) =
                    frag_rep(&pa.rep, &r.frag, arena)
                {
                    for q in pa.pts.iter_mut() {
                        q.0 += dx;
                        q.1 += dy;
                    }
                    pa.rep = rep;
                }
                paths.push(pa);
            }
        }
    }
    for lr in &job.lod_rects {
        rects.push(lr.clone());
    }
    let name = if job.lod == floe_ovm::LOD_EXACT {
        floe_ovm::page_cell_name(job.ci as u32, job.li, job.seq)
    } else {
        floe_ovm::lod_cell_name(job.ci as u32, job.li, job.seq)
    };
    let wc = WCell {
        name,
        rects: &rects,
        polys: &polys,
        paths: &paths,
        texts: &[],
        places: Vec::new(),
    };
    floe_oasis::write::write_tree_sized(&[wc], doc.unit)
        .expect("page payload")
}

#[allow(clippy::too_many_arguments)]
fn write_encoded_page(
    b: &mut Builder,
    ovp: &mut std::io::BufWriter<std::fs::File>,
    job: &PageJob,
    payload: &[u8],
    raw: u64,
    ovp_off: &mut u64,
    pages_bytes: &mut u64,
) {
    std::io::Write::write_all(ovp, payload).expect("write ovp");
    b.page(
        narrow_u32(job.ci as u64, "cell index"),
        job.li,
        job.seq,
        &job.bbox,
        *ovp_off,
        payload.len() as u64,
        raw,
        (job.recs.len() + job.lod_rects.len()) as u64,
        job.members,
        job.max_w.max(0) as u64,
        job.max_h.max(0) as u64,
        job.lod,
        job.lod_page,
    );
    *ovp_off = ovp_off
        .checked_add(payload.len() as u64)
        .expect("limit exceeded: ovp bytes");
    *pages_bytes = pages_bytes
        .checked_add(payload.len() as u64)
        .expect("limit exceeded: page payload bytes");
}

/// Encode and commit one consecutive cell-plan batch. `page_jobs` is already
/// in global page order; result slots restore that order after parallel work.
/// The task/result channels and the result vector together own at most one
/// batch of payloads, independent of the complete design page count.
#[allow(clippy::too_many_arguments)]
fn encode_write_pages(
    doc: &Doc,
    page_jobs: &[PageJob],
    arenas: &[std::sync::Arc<Arena>],
    arena_cell_base: usize,
    workers: usize,
    batch_limit: usize,
    b: &mut Builder,
    ovp: &mut std::io::BufWriter<std::fs::File>,
    ovp_off: &mut u64,
    pages_bytes: &mut u64,
    encoded_done: &std::sync::atomic::AtomicUsize,
) {
    let ptotal = page_jobs.len();
    if ptotal == 0 {
        return;
    }
    let arena_for = |job: &PageJob| -> &Arena {
        let ai = job
            .ci
            .checked_sub(arena_cell_base)
            .expect("page cell before arena batch");
        arenas.get(ai).expect("page cell after arena batch")
    };
    if workers <= 1 || ptotal == 1 {
        for job in page_jobs {
            let (payload, raw) = encode_job(doc, job, arena_for(job));
            write_encoded_page(
                b,
                ovp,
                job,
                &payload,
                raw,
                ovp_off,
                pages_bytes,
            );
            encoded_done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        return;
    }

    let result_batch = batch_limit.max(1).min(ptotal);
    std::thread::scope(|s| {
        let (task_tx, task_rx) =
            std::sync::mpsc::sync_channel::<usize>(result_batch);
        let task_rx = std::sync::Arc::new(std::sync::Mutex::new(task_rx));
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel::<
            (usize, Vec<u8>, u64),
        >(result_batch);
        for _ in 0..workers.min(ptotal) {
            let task_rx = task_rx.clone();
            let result_tx = result_tx.clone();
            let arena_for = &arena_for;
            s.spawn(move || loop {
                let i = match task_rx.lock().unwrap().recv() {
                    Ok(i) => i,
                    Err(_) => return,
                };
                let job = &page_jobs[i];
                let (payload, raw) = encode_job(doc, job, arena_for(job));
                if result_tx.send((i, payload, raw)).is_err() {
                    return;
                }
            });
        }
        drop(result_tx);
        for base in (0..ptotal).step_by(result_batch) {
            let end = (base + result_batch).min(ptotal);
            for i in base..end {
                task_tx.send(i).expect("encode task worker");
            }
            let mut results: Vec<Option<(Vec<u8>, u64)>> =
                (base..end).map(|_| None).collect();
            for _ in base..end {
                let (i, payload, raw) =
                    result_rx.recv().expect("encode result worker");
                assert!(i >= base && i < end, "encode result outside batch");
                results[i - base] = Some((payload, raw));
            }
            for (i, slot) in (base..end).zip(results) {
                let (payload, raw) = slot.expect("encode result slot");
                write_encoded_page(
                    b,
                    ovp,
                    &page_jobs[i],
                    &payload,
                    raw,
                    ovp_off,
                    pages_bytes,
                );
                encoded_done
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        drop(task_tx);
    });
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
    /// fragmented-Pts offset scratch shared by this cell's jobs
    arena: std::sync::Arc<Arena>,
    split_stats: SplitStats,
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

/// M7 LOD trigger: pages this dense get a merged-coverage variant
const LOD_MIN_MEMBERS: u64 = 4096;
/// coverage grid resolution (per axis) - at the zoom band where a
/// variant activates, one cell is at or below one screen pixel
const LOD_GRID: i64 = 128;
/// records whose SHAPE spans at least this many grid cells in BOTH
/// axes pass through verbatim (individually visible blobs keep
/// their exact outline; only sub-cell "dust" is fused)
const LOD_PASS_CELLS: i64 = 4;

/// build the merged-coverage LOD variant of one page: mark every
/// small member's footprint on a LOD_GRID^2 bitmap over the page
/// bbox, fuse the covered cells into maximal-run rectangles (RLE
/// rows + vertical join), keep large records verbatim. Conservative
/// by construction: coverage is a superset of the exact page's,
/// overcoverage bounded by one grid cell. Returns None when the
/// page has no fusable content.
fn gen_lod_job(
    doc: &Doc,
    cell: &floe_oasis::doc::Cell,
    arena: &Arena,
    exact: &PageJob,
) -> Option<PageJob> {
    let bb = exact.bbox;
    let (bw, bh) = (bb.x1 - bb.x0, bb.y1 - bb.y0);
    if bw <= 0 || bh <= 0 {
        return None;
    }
    let g = LOD_GRID;
    let words_per_row = (g as usize).div_ceil(64);
    let mut bits = vec![0u64; g as usize * words_per_row];
    let mut pass: Vec<PRec> = Vec::new();
    let (mut pass_members, mut fused_any) = (0u64, false);
    // cell index of a coordinate (clamped); cells are half-open
    let gx_of = |x: i64| -> i64 {
        (((x - bb.x0) as i128 * g as i128) / bw as i128)
            .clamp(0, (g - 1) as i128) as i64
    };
    let gy_of = |y: i64| -> i64 {
        (((y - bb.y0) as i128 * g as i128) / bh as i128)
            .clamp(0, (g - 1) as i128) as i64
    };
    for r in &exact.recs {
        let sb = rec_shape_box(cell, r);
        let (sw, sh) = (sb.x1 - sb.x0, sb.y1 - sb.y0);
        // shape extent in cells (ceil): big-in-both-axes records
        // stay exact
        let cw = (sw as i128 * g as i128 / bw.max(1) as i128) as i64;
        let ch = (sh as i128 * g as i128 / bh.max(1) as i128) as i64;
        if cw >= LOD_PASS_CELLS && ch >= LOD_PASS_CELLS {
            pass_members += r.members;
            pass.push(r.clone());
            continue;
        }
        fused_any = true;
        let mut mark = |ox: i64, oy: i64| {
            let (x0, x1) = (gx_of(sb.x0 + ox), gx_of(sb.x1 + ox));
            let (y0, y1) = (gy_of(sb.y0 + oy), gy_of(sb.y1 + oy));
            for y in y0..=y1 {
                let row = y as usize * words_per_row;
                for x in x0..=x1 {
                    bits[row + (x as usize >> 6)] |=
                        1u64 << (x as usize & 63);
                }
            }
        };
        match (r.frag, rec_rep(cell, r)) {
            (Frag::Pts { arena: a, lo, hi }, rep) => {
                let src = match rep {
                    Rep::Pts(pl) => pl,
                    _ => unreachable!("pts frag on non-pts"),
                };
                for &slot in &arena[a as usize].order
                    [lo as usize..hi as usize]
                {
                    let (ox, oy) = src[slot as usize];
                    mark(ox, oy);
                }
            }
            (Frag::Grid { i0, i1, j0, j1 }, rep) => {
                let (va, vb) = match rep {
                    Rep::Grid { va, vb, .. } => (*va, *vb),
                    _ => unreachable!("grid frag on non-grid"),
                };
                for i in i0 as i64..i1 as i64 {
                    for j in j0 as i64..j1 as i64 {
                        mark(
                            i * va.0 + j * vb.0,
                            i * va.1 + j * vb.1,
                        );
                    }
                }
            }
            (Frag::Whole, Rep::One) => mark(0, 0),
            (Frag::Whole, Rep::Grid { na, nb, va, vb }) => {
                for i in 0..*na as i64 {
                    for j in 0..*nb as i64 {
                        mark(
                            i * va.0 + j * vb.0,
                            i * va.1 + j * vb.1,
                        );
                    }
                }
            }
            (Frag::Whole, Rep::Pts(pl)) => {
                for &(ox, oy) in pl.iter() {
                    mark(ox, oy);
                }
            }
        }
    }
    if !fused_any {
        return None; // everything is large: the exact page IS the LOD
    }
    // fuse: rows into runs, identical-span runs into taller rects.
    // Cell boundaries come from the SAME integer division both ways
    // so adjacent rects share edges exactly (no cracks).
    let (l, d) = doc.layer_order[exact.li as usize];
    let cx = |gx: i64| bb.x0 + (gx as i128 * bw as i128 / g as i128) as i64;
    let cy = |gy: i64| bb.y0 + (gy as i128 * bh as i128 / g as i128) as i64;
    let mut rects: Vec<RectRec> = Vec::new();
    // open runs from the previous row: (x0, x1, y_start)
    let mut open: Vec<(i64, i64, i64)> = Vec::new();
    for y in 0..=g {
        let mut runs: Vec<(i64, i64)> = Vec::new();
        if y < g {
            let row = y as usize * words_per_row;
            let mut x = 0i64;
            while x < g {
                let w = bits[row + (x as usize >> 6)];
                if w & (1u64 << (x as usize & 63)) == 0 {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < g {
                    let w = bits[row + (x as usize >> 6)];
                    if w & (1u64 << (x as usize & 63)) == 0 {
                        break;
                    }
                    x += 1;
                }
                runs.push((start, x));
            }
        }
        let mut next_open: Vec<(i64, i64, i64)> = Vec::new();
        for &(x0, x1) in &runs {
            match open.iter().position(|&(ox0, ox1, _)| {
                ox0 == x0 && ox1 == x1
            }) {
                Some(k) => next_open.push(open.swap_remove(k)),
                None => next_open.push((x0, x1, y)),
            }
        }
        for &(x0, x1, ys) in &open {
            let (wx0, wx1) = (cx(x0), cx(x1));
            let (wy0, wy1) = (cy(ys), cy(y));
            rects.push(RectRec {
                layer: l,
                dt: d,
                x: wx0,
                y: wy0,
                w: (wx1 - wx0).max(1),
                h: (wy1 - wy0).max(1),
                rep: Rep::One,
            });
        }
        open = next_open;
    }
    if rects.is_empty() && pass.is_empty() {
        return None;
    }
    let mut lbb = BBox::EMPTY;
    let (mut max_w, mut max_h) = (0i64, 0i64);
    for rc in &rects {
        lbb.grow(&BBox {
            x0: rc.x,
            y0: rc.y,
            x1: rc.x + rc.w,
            y1: rc.y + rc.h,
        });
        max_w = max_w.max(rc.w);
        max_h = max_h.max(rc.h);
    }
    for r in &pass {
        lbb.grow(&r.bbox);
        max_w = max_w.max(r.w);
        max_h = max_h.max(r.h);
    }
    Some(PageJob {
        ci: exact.ci,
        li: exact.li,
        seq: exact.seq,
        members: pass_members + rects.len() as u64,
        recs: pass,
        bbox: lbb,
        max_w,
        max_h,
        lod: floe_ovm::LOD_MERGED,
        lod_page: floe_ovm::LOD_PAGE_NONE,
        lod_rects: rects,
    })
}

fn build_cell_plan(
    doc: &Doc,
    ci: usize,
    rbb: &[Option<(i64, i64, i64, i64)>],
    lidx: &std::collections::HashMap<(u32, u32), usize>,
    nl: usize,
    page_target_bytes: u64,
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
            frag: Frag::Whole,
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
            frag: Frag::Whole,
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
            frag: Frag::Whole,
        });
    }
    let mut pages: Vec<PageJob> = Vec::new();
    let mut pranges: Vec<(u32, u32, u32, u32)> = Vec::new();
    let mut pbvh: Vec<(BBox, u32, u16, bool, u64, u64)> = Vec::new();
    let mut arena: Arena = Vec::new();
    let mut split_stats = SplitStats::default();
    for (li, recs) in per_layer.into_iter().enumerate() {
        if recs.is_empty() {
            continue;
        }
        let mut seq = 0u32;
        let mut run: Vec<PageJob> = Vec::new();
        split_pages(
            cell,
            &mut arena,
            ci,
            li as u32,
            recs,
            &mut seq,
            &mut run,
            &mut split_stats,
            page_target_bytes,
            0,
        );
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
    // M7: LOD variants ride at the tail of the cell's page list -
    // pranges/pbvh reference only the exact runs before them, and
    // the exact->LOD link is rebased to global indices by the
    // append loop
    let n_exact = pages.len();
    for k in 0..n_exact {
        if pages[k].members < LOD_MIN_MEMBERS {
            continue;
        }
        if let Some(job) = gen_lod_job(doc, cell, &arena, &pages[k])
        {
            pages[k].lod_page = narrow_u32(
                pages.len() as u64,
                "cell page count",
            );
            split_stats.lod_pages += 1;
            pages.push(job);
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
    CellPlan {
        place_order,
        bvh,
        pages,
        pranges,
        pbvh,
        pts_prep,
        arena: std::sync::Arc::new(arena),
        split_stats,
    }
}

fn build(
    doc: &Doc,
    size: u64,
    mtime: u64,
    outdir: &str,
    jobs: usize,
    encode_batch: usize,
    plan_batch: usize,
    page_target_bytes: u64,
) -> (Vec<u8>, Vec<Option<(i64, i64, i64, i64)>>, Vec<u64>) {
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
    // layer table records are STORED records (post-fragmentation),
    // so they are summed from the page jobs after the split - see
    // below; only source-count lrecs would disagree with the page
    // directory whenever reps fragmented. (lrecs stays as the
    // scan-parity lower bound: stored >= source.)
    let _ = &lrecs;

    // ---- bounded cell-plan + page-encode pipeline. The old phase barrier
    // retained every CellPlan, fragmented-Pts Arena and prepared placement
    // list before the first page was encoded (443 GB on the 9.8G asset).
    // Consecutive batches preserve cell/page order but release all batch
    // scratch immediately after its pages reach design.ovp.
    let plan_batch = plan_batch.max(1).min(n.max(1));
    let encode_workers = jobs.max(1);
    let ovp_path = format!("{}/design.ovp", outdir);
    let mut ovp = std::io::BufWriter::new(
        std::fs::File::create(&ovp_path).expect("create ovp"),
    );
    let mut ovp_off = 0u64;
    let mut pages_bytes = 0u64;
    let mut pages_total = 0usize;
    let mut lrecs_stored = vec![0u64; nl];
    let mut split_total = SplitStats::default();
    let planned_cells = std::sync::Arc::new(
        std::sync::atomic::AtomicUsize::new(0),
    );
    let planned_pages = std::sync::Arc::new(
        std::sync::atomic::AtomicUsize::new(0),
    );
    let encoded_pages = std::sync::Arc::new(
        std::sync::atomic::AtomicUsize::new(0),
    );
    let pipeline_on = std::sync::Arc::new(
        std::sync::atomic::AtomicBool::new(true),
    );
    let pipeline_start = std::time::Instant::now();
    let heartbeat = {
        let planned_cells = planned_cells.clone();
        let planned_pages = planned_pages.clone();
        let encoded_pages = encoded_pages.clone();
        let pipeline_on = pipeline_on.clone();
        std::thread::spawn(move || {
            use std::sync::atomic::Ordering::Relaxed;
            let mut last = 0u64;
            loop {
                std::thread::sleep(
                    std::time::Duration::from_millis(200),
                );
                if !pipeline_on.load(Relaxed) {
                    return;
                }
                let elapsed = pipeline_start.elapsed().as_secs();
                if elapsed >= last + 5 {
                    last = elapsed;
                    eprintln!(
                        "[vfs] build: pipeline cells {}/{} pages \
                         planned={} encoded={} ({}s, rss {})",
                        planned_cells.load(Relaxed),
                        n,
                        planned_pages.load(Relaxed),
                        encoded_pages.load(Relaxed),
                        elapsed,
                        rss()
                    );
                }
            }
        })
    };
    eprintln!(
        "[vfs] build: bounded pipeline {} cells ({} workers, \
         plan batch {}, encode batch {}, page target {} MiB)...",
        n,
        jobs.max(1),
        plan_batch,
        encode_batch.max(1),
        page_target_bytes / MIB
    );
    let mut plan_elapsed = std::time::Duration::ZERO;
    let mut encode_elapsed = std::time::Duration::ZERO;
    for cell_base in (0..n).step_by(plan_batch) {
        let cell_end = (cell_base + plan_batch).min(n);
        let batch_len = cell_end - cell_base;
        let tp = std::time::Instant::now();
        let pslots: Vec<std::sync::OnceLock<CellPlan>> =
            (0..batch_len).map(|_| std::sync::OnceLock::new()).collect();
        let next = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|s| {
            let nthreads = jobs.max(1).min(batch_len);
            for _ in 0..nthreads {
                let pslots = &pslots;
                let next = &next;
                let rbb = &rbb;
                let lidx = &lidx;
                s.spawn(move || loop {
                    let local = next.fetch_add(
                        1,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    if local >= batch_len {
                        return;
                    }
                    let ci = cell_base + local;
                    let plan = build_cell_plan(
                        doc,
                        ci,
                        rbb,
                        lidx,
                        nl,
                        page_target_bytes,
                    );
                    let _ = pslots[local].set(plan);
                });
            }
        });
        plan_elapsed += tp.elapsed();

        // Append metadata in cell order and retain only this batch's page jobs
        // and fragment arenas for the immediately following encode pass.
        let mut batch_jobs: Vec<PageJob> = Vec::new();
        let mut batch_arenas: Vec<std::sync::Arc<Arena>> =
            Vec::with_capacity(batch_len);
        for (local, slot) in pslots.into_iter().enumerate() {
            let ci = cell_base + local;
            let CellPlan {
                place_order,
                bvh,
                mut pages,
                pranges,
                pbvh,
                pts_prep,
                arena,
                split_stats,
            } = slot.into_inner().expect("cell plan unset");
            batch_arenas.push(arena);
            split_total.fragments = split_total
                .fragments
                .checked_add(split_stats.fragments)
                .expect("limit exceeded: fragment count");
            split_total.oversize_pages = split_total
                .oversize_pages
                .checked_add(split_stats.oversize_pages)
                .expect("limit exceeded: oversize page count");
            split_total.depth_capped = split_total
                .depth_capped
                .checked_add(split_stats.depth_capped)
                .expect("limit exceeded: depth-capped count");
            split_total.lod_pages = split_total
                .lod_pages
                .checked_add(split_stats.lod_pages)
                .expect("limit exceeded: lod page count");
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
                        narrow_u32(
                            place_base + first as u64,
                            "place index",
                        )
                    } else {
                        narrow_u32(
                            base as u64 + first as u64,
                            "bvh index",
                        )
                    };
                    b.bvh_node(&bb, f, count, leaf);
                }
                (
                    base,
                    narrow_u32(bvh.len() as u64, "cell bvh count"),
                )
            };
            let page_base = pages_total as u64;
            let page_start = narrow_u32(page_base, "page count");
            let page_count =
                narrow_u32(pages.len() as u64, "cell page count");
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
                let root = if root == PBVH_NONE {
                    PBVH_NONE
                } else {
                    narrow_u32(
                        pb_base as u64 + root as u64,
                        "pbvh index",
                    )
                };
                b.prange(
                    li,
                    narrow_u32(page_base + lo as u64, "page index"),
                    cnt,
                    root,
                );
            }
            let pr_count =
                narrow_u32(pranges.len() as u64, "prange count");
            for job in pages.iter_mut() {
                if job.lod_page != floe_ovm::LOD_PAGE_NONE {
                    job.lod_page = narrow_u32(
                        page_base + job.lod_page as u64,
                        "page index",
                    );
                }
                if job.lod != floe_ovm::LOD_EXACT {
                    // LOD variants are derived data: the layer
                    // table (and the G5 gates) count exact only
                    continue;
                }
                let count = &mut lrecs_stored[job.li as usize];
                *count = count
                    .checked_add(job.recs.len() as u64)
                    .expect("limit exceeded: stored layer records");
            }
            pages_total = pages_total
                .checked_add(pages.len())
                .expect("limit exceeded: page count");
            batch_jobs.append(&mut pages);

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
        planned_cells.store(
            cell_end,
            std::sync::atomic::Ordering::Relaxed,
        );
        planned_pages.store(
            pages_total,
            std::sync::atomic::Ordering::Relaxed,
        );
        let te = std::time::Instant::now();
        encode_write_pages(
            doc,
            &batch_jobs,
            &batch_arenas,
            cell_base,
            encode_workers,
            encode_batch,
            &mut b,
            &mut ovp,
            &mut ovp_off,
            &mut pages_bytes,
            &encoded_pages,
        );
        encode_elapsed += te.elapsed();
        debug_assert_eq!(b.n_pages() as usize, pages_total);
        // batch_jobs, batch_arenas and every PtsPrepared drop here.
    }
    pipeline_on.store(false, std::sync::atomic::Ordering::Relaxed);
    heartbeat.join().expect("pipeline heartbeat");
    eprintln!(
        "[vfs] build: pipeline complete (plan {:.1}s, encode {:.1}s, \
         rss {})",
        plan_elapsed.as_secs_f64(),
        encode_elapsed.as_secs_f64(),
        rss()
    );

    for (i, &(l, d)) in doc.layer_order.iter().enumerate() {
        debug_assert!(lrecs_stored[i] >= lrecs[i]);
        let nm = doc
            .layer_names
            .get(&(l, d))
            .cloned()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{}/{}", l, d));
        b.layer(l, d, &nm, lrecs_stored[i], lmems[i]);
    }

    if split_total.fragments > 0
        || split_total.oversize_pages > 0
        || split_total.depth_capped > 0
        || split_total.lod_pages > 0
    {
        eprintln!(
            "[vfs] build: rep-split {} fragments, {} oversize \
             pages, {} depth-capped, {} lod variants",
            split_total.fragments,
            split_total.oversize_pages,
            split_total.depth_capped,
            split_total.lod_pages
        );
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
    (ovm_bytes, rbb, lmems)
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

/// --inspect: decode a sample of the selected pages and report
/// WHERE the bytes come from - wide pages (bbox >= half the owning
/// cell's extent), rep-type record/member breakdown, and for pages
/// of the TOP cell (identity frame) the fraction of members whose
/// bbox actually intersects the viewport. This is the field probe
/// that separates "planner over-selects" from "page bboxes are
/// honest but the geometry really is everywhere" (9.8G floor).
fn inspect_plan(
    dir: &str,
    v: &floe_vfs::Vfs,
    req: &floe_vfs::ViewReq,
    plan: &floe_vfs::hier::HierPlan,
    rest: &[(String, String)],
) {
    use std::io::{Read, Seek, SeekFrom};
    let cap: usize = rest
        .iter()
        .find(|(k, _)| k == "--inspect-pages")
        .and_then(|(_, val)| val.parse().ok())
        .unwrap_or(64);
    let top_ci = plan.top.0;
    let (mut wide_pages, mut wide_members) = (0u64, 0u64);
    for &pi in &plan.pages {
        let p = v.ovm.page(pi);
        let cb = v.ovm.cell_rbbox(p.cell);
        let (cw, ch) = (cb.x1 - cb.x0, cb.y1 - cb.y0);
        let (pw, ph) =
            (p.bbox.x1 - p.bbox.x0, p.bbox.y1 - p.bbox.y0);
        if (cw > 0 && pw * 4 >= cw * 3)
            || (ch > 0 && ph * 4 >= ch * 3)
        {
            wide_pages += 1;
            wide_members += p.members;
        }
    }
    let step = plan.pages.len().div_ceil(cap.max(1)).max(1);
    let ovp = format!("{}/design.ovp", dir);
    let mut f = std::fs::File::open(&ovp).expect("open ovp");
    let (mut n_one, mut n_grid, mut n_pts) = (0u64, 0u64, 0u64);
    let (mut sampled, mut sampled_top) = (0u64, 0u64);
    let (mut mem_total, mut mem_top, mut mem_vis) =
        (0u64, 0u64, 0u64);
    let vis_of = |b: &BBox, rep: &Rep, view: &BBox| -> (u64, u64) {
        let mut t = 0u64;
        let mut vis = 0u64;
        let mut tally = |ox: i64, oy: i64| {
            t += 1;
            let m = BBox {
                x0: b.x0 + ox,
                y0: b.y0 + oy,
                x1: b.x1 + ox,
                y1: b.y1 + oy,
            };
            if m.intersects(view) {
                vis += 1;
            }
        };
        match rep {
            Rep::One => tally(0, 0),
            Rep::Grid { na, nb, va, vb } => {
                for i in 0..*na as i64 {
                    for j in 0..*nb as i64 {
                        tally(
                            i * va.0 + j * vb.0,
                            i * va.1 + j * vb.1,
                        );
                    }
                }
            }
            Rep::Pts(pl) => {
                for &(x, y) in pl.iter() {
                    tally(x, y);
                }
            }
        }
        (t, vis)
    };
    for (i, &pi) in plan.pages.iter().enumerate() {
        if i % step != 0 {
            continue;
        }
        let p = v.ovm.page(pi);
        let mut buf = vec![0u8; p.csize as usize];
        f.seek(SeekFrom::Start(p.file_off)).expect("seek ovp");
        f.read_exact(&mut buf).expect("read ovp");
        let pd = floe_oasis::doc::parse_doc(&buf)
            .expect("page payload parse");
        sampled += 1;
        let is_top = p.cell == top_ci;
        if is_top {
            sampled_top += 1;
        }
        let c = &pd.cells[0];
        for r in &c.rects {
            match r.rep {
                Rep::One => n_one += 1,
                Rep::Grid { .. } => n_grid += 1,
                Rep::Pts(_) => n_pts += 1,
            }
            let b = BBox {
                x0: r.x,
                y0: r.y,
                x1: r.x + r.w,
                y1: r.y + r.h,
            };
            let (t, vis) = vis_of(&b, &r.rep, &req.view);
            mem_total += t;
            if is_top {
                mem_top += t;
                mem_vis += vis;
            }
        }
        for po in &c.polys {
            match po.rep {
                Rep::One => n_one += 1,
                Rep::Grid { .. } => n_grid += 1,
                Rep::Pts(_) => n_pts += 1,
            }
            let b = pts_bbox(&po.pts);
            let (t, vis) = vis_of(&b, &po.rep, &req.view);
            mem_total += t;
            if is_top {
                mem_top += t;
                mem_vis += vis;
            }
        }
        for pa in &c.paths {
            match pa.rep {
                Rep::One => n_one += 1,
                Rep::Grid { .. } => n_grid += 1,
                Rep::Pts(_) => n_pts += 1,
            }
            let b4 = path_bbox(&pa.pts, pa.hw, pa.es, pa.ee);
            let b = BBox { x0: b4.0, y0: b4.1, x1: b4.2, y1: b4.3 };
            let (t, vis) = vis_of(&b, &pa.rep, &req.view);
            mem_total += t;
            if is_top {
                mem_top += t;
                mem_vis += vis;
            }
        }
    }
    println!(
        "{{\n  \"inspect\": {{\n    \"selected_pages\": {},\n    \
         \"cell_wide_pages\": {},\n    \
         \"cell_wide_members\": {},\n    \
         \"sampled_pages\": {},\n    \
         \"sampled_top_pages\": {},\n    \
         \"rep_records\": {{\"one\": {}, \"grid\": {}, \
         \"pts\": {}}},\n    \
         \"sampled_members\": {},\n    \
         \"sampled_top_members\": {},\n    \
         \"sampled_top_visible\": {},\n    \
         \"top_visible_ratio\": {:.6}\n  }}\n}}",
        plan.pages.len(),
        wide_pages,
        wide_members,
        sampled,
        sampled_top,
        n_one,
        n_grid,
        n_pts,
        mem_total,
        mem_top,
        mem_vis,
        if mem_top > 0 {
            mem_vis as f64 / mem_top as f64
        } else {
            0.0
        }
    );
}

pub fn plan_cmd(args: &[String]) {
    let (dir, view, px, cut, depth, layers, rest) =
        parse_common(args);
    // M5: hier is the only planner; --mode is accepted and
    // ignored for script compatibility
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
    {
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
        if rest.iter().any(|(k, _)| k == "--inspect") {
            inspect_plan(&dir, &v, &req, &plan, &rest);
        }
        return;
    }
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
        hier: floe_vfs::HierSession::new(budget_mb << 20),
        names_sent: false,
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
    hier: floe_vfs::HierSession,
    names_sent: bool,
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
    // M5: the daemon speaks V4 (hier) only; `mode=` survives
    // solely to mark session-less probes (pick/snap/clip)
    let probe = mode == "probe" || mode == "hier_probe";
    let view = view.ok_or("view required")?;
    let out = out.ok_or("out required")?;
    let req =
        make_req(d.v, view, px, cut, depth, layers.as_deref());
    serve_hier(d, &req, gen, ack, reset, probe, stream_kb, &out)
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

#[cfg(test)]
mod split_tests {
    use super::*;

    /// deterministic LCG so tests need no rand crate
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, m: i64) -> i64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as i64).rem_euclid(m)
        }
    }

    fn pts_rec(
        seed: u64,
        n: usize,
        die: i64,
        w: i64,
    ) -> RectRec {
        let mut g = Lcg(seed);
        let mut pos: Vec<(i64, i64)> =
            (0..n).map(|_| (g.next(die), g.next(die))).collect();
        let first = pos[0];
        for p in pos.iter_mut() {
            *p = (p.0 - first.0, p.1 - first.1);
        }
        RectRec {
            layer: 1,
            dt: 0,
            x: first.0,
            y: first.1,
            w,
            h: w,
            rep: Rep::Pts(pos.into()),
        }
    }

    fn mini_doc(rects: Vec<RectRec>) -> Doc {
        let mut c = floe_oasis::doc::Cell::default();
        c.name = String::from("T");
        c.rects = rects;
        Doc {
            unit: 1000.0,
            cells: vec![c],
            top: 0,
            layer_order: vec![(1, 0)],
            norm_s: 0.0,
            layer_names: Default::default(),
        }
    }

    fn plan_of(doc: &Doc) -> CellPlan {
        let rbb = cell_bboxes_full(doc);
        let lidx: std::collections::HashMap<(u32, u32), usize> =
            doc.layer_order
                .iter()
                .enumerate()
                .map(|(i, &k)| (k, i))
                .collect();
        build_cell_plan(doc, 0, &rbb, &lidx, 1, MIB)
    }

    /// expand every member of a cell's records into
    /// (layer, dt, kind, bbox) tuples - the conservation oracle
    fn expand_cell(c: &floe_oasis::doc::Cell) -> Vec<Mem> {
        let mut v: Vec<Mem> = Vec::new();
        for r in &c.rects {
            let b = BBox {
                x0: r.x,
                y0: r.y,
                x1: r.x + r.w,
                y1: r.y + r.h,
            };
            expand_rep(&r.rep, |ox, oy| {
                v.push((r.layer, r.dt, 0, b.x0 + ox, b.y0 + oy,
                        b.x1 + ox, b.y1 + oy));
            });
        }
        for p in &c.polys {
            let b = pts_bbox(&p.pts);
            expand_rep(&p.rep, |ox, oy| {
                v.push((p.layer, p.dt, 1, b.x0 + ox, b.y0 + oy,
                        b.x1 + ox, b.y1 + oy));
            });
        }
        for pa in &c.paths {
            let b4 = path_bbox(&pa.pts, pa.hw, pa.es, pa.ee);
            expand_rep(&pa.rep, |ox, oy| {
                v.push((pa.layer, pa.dt, 2, b4.0 + ox, b4.1 + oy,
                        b4.2 + ox, b4.3 + oy));
            });
        }
        v.sort_unstable();
        v
    }

    type Mem = (u32, u32, u8, i64, i64, i64, i64);

    fn expand_rep(rep: &Rep, mut f: impl FnMut(i64, i64)) {
        match rep {
            Rep::One => f(0, 0),
            Rep::Grid { na, nb, va, vb } => {
                for i in 0..*na as i64 {
                    for j in 0..*nb as i64 {
                        f(i * va.0 + j * vb.0, i * va.1 + j * vb.1);
                    }
                }
            }
            Rep::Pts(p) => {
                for &(x, y) in p.iter() {
                    f(x, y);
                }
            }
        }
    }

    /// encode every page of the plan, parse it back, expand - the
    /// multiset must equal the source cell's expansion EXACTLY
    /// (duplicate offsets preserved with their counts), and every
    /// member bbox must lie inside its page's bbox
    fn roundtrip(doc: &Doc, plan: &CellPlan) -> Vec<Mem> {
        let mut got: Vec<Mem> = Vec::new();
        // conservation is an EXACT-page contract; LOD variants are
        // derived coverage and are gated separately below
        for job in plan
            .pages
            .iter()
            .filter(|j| j.lod == floe_ovm::LOD_EXACT)
        {
            let (payload, _raw) =
                encode_job(doc, job, &plan.arena);
            // page payloads are complete OASIS files (CBLOCKs
            // inflate inside the parser)
            let pd = floe_oasis::doc::parse_doc(&payload)
                .expect("page parse");
            assert_eq!(pd.cells.len(), 1);
            let mems = expand_cell(&pd.cells[0]);
            for m in &mems {
                assert!(
                    m.3 >= job.bbox.x0
                        && m.4 >= job.bbox.y0
                        && m.5 <= job.bbox.x1
                        && m.6 <= job.bbox.y1,
                    "member outside page bbox: {:?} vs {:?}",
                    m,
                    job.bbox
                );
            }
            got.extend(mems);
        }
        got.sort_unstable();
        got
    }

    fn assert_conserved(doc: &Doc) -> CellPlan {
        let plan = plan_of(doc);
        let want = expand_cell(&doc.cells[0]);
        let got = roundtrip(doc, &plan);
        assert_eq!(want.len(), got.len(), "member count");
        assert_eq!(want, got, "member multiset");
        plan
    }

    /// die-wide scattered Pts reps MUST split into spatially tight
    /// pages - the 9.8G floor regression (a point view selected
    /// every page of the layer)
    #[test]
    fn scattered_pts_pages_are_local() {
        const DIE: i64 = 1_000_000;
        let doc = mini_doc(vec![
            pts_rec(7, 300_000, DIE, 100),
            pts_rec(99, 300_000, DIE, 150),
        ]);
        let plan = assert_conserved(&doc);
        assert!(plan.pages.len() >= 2, "{}", plan.pages.len());
        let members: u64 = plan
            .pages
            .iter()
            .filter(|j| j.lod == floe_ovm::LOD_EXACT)
            .map(|j| j.members)
            .sum();
        assert_eq!(members, 600_000);
        // page bboxes must stay near-disjoint: total covered area
        // <= 2x die area (the broken build had every page at ~90%
        // of the die: 4 pages summed to ~3.6x). LOD variants are
        // derived coverage, not part of the partition.
        let mut area = 0i128;
        for j in plan
            .pages
            .iter()
            .filter(|j| j.lod == floe_ovm::LOD_EXACT)
        {
            area += (j.bbox.x1 - j.bbox.x0) as i128
                * (j.bbox.y1 - j.bbox.y0) as i128;
        }
        assert!(
            area <= 2 * (DIE as i128) * (DIE as i128),
            "pages cover {}x die area",
            area / ((DIE as i128) * (DIE as i128))
        );
    }

    /// klayout folding leftovers: tiny 2-member records whose two
    /// points sit on opposite die sides. A byte floor once exempted
    /// them from fragmentation and every quadrant page bbox grew
    /// back to ~die width (repfloor field repro)
    #[test]
    fn small_wide_pairs_still_fragment() {
        const DIE: i64 = 1_000_000;
        let mut recs = vec![
            pts_rec(7, 200_000, DIE, 100),
            pts_rec(99, 200_000, DIE, 150),
        ];
        let mut g = Lcg(1234);
        for _ in 0..500 {
            let (x0, y) = (g.next(DIE / 4), g.next(DIE));
            let x1 = DIE * 3 / 4 + g.next(DIE / 4);
            recs.push(RectRec {
                layer: 1,
                dt: 0,
                x: x0,
                y,
                w: 120,
                h: 120,
                rep: Rep::Pts(vec![(0, 0), (x1 - x0, 0)].into()),
            });
        }
        let doc = mini_doc(recs);
        let plan = assert_conserved(&doc);
        assert!(plan.pages.len() >= 2);
        let mut area = 0i128;
        for j in plan
            .pages
            .iter()
            .filter(|j| j.lod == floe_ovm::LOD_EXACT)
        {
            area += (j.bbox.x1 - j.bbox.x0) as i128
                * (j.bbox.y1 - j.bbox.y0) as i128;
        }
        assert!(
            area <= 2 * (DIE as i128) * (DIE as i128),
            "wide pairs poisoned pages: {}x die",
            area / ((DIE as i128) * (DIE as i128))
        );
    }

    /// duplicate offsets (counted!), negative coordinates
    #[test]
    fn conservation_dup_offsets_negative_coords() {
        const DIE: i64 = 1_000_000;
        let mut pts = vec![(0, 0), (0, 0), (0, 0)]; // dup at base
        let mut g = Lcg(5);
        for _ in 0..150_000 {
            let x = g.next(DIE) - DIE / 2; // negative half-plane
            let y = g.next(DIE) - DIE / 2;
            pts.push((x, y));
            if pts.len() % 1000 == 0 {
                pts.push((x, y)); // scattered duplicates
            }
        }
        let doc = mini_doc(vec![
            RectRec {
                layer: 1,
                dt: 0,
                x: -DIE / 3,
                y: -DIE / 3,
                w: 80,
                h: 90,
                rep: Rep::Pts(pts.into()),
            },
            pts_rec(42, 150_000, DIE, 100),
        ]);
        assert_conserved(&doc);
    }

    /// grids: axis-aligned, negative vector, skew - fragmented at
    /// index boundaries, conserved through encode->parse (the
    /// writer's na>=2 normalization would panic or corrupt here)
    #[test]
    fn conservation_grids_axis_neg_skew() {
        const DIE: i64 = 1_000_000;
        let g = |va: (i64, i64), vb: (i64, i64), na, nb, x, y| {
            RectRec {
                layer: 1,
                dt: 0,
                x,
                y,
                w: 70,
                h: 60,
                rep: Rep::Grid { na, nb, va, vb },
            }
        };
        let doc = mini_doc(vec![
            // dense axis grid spanning the die
            g((1000, 0), (0, 1000), 900, 900, 0, 0),
            // negative vector grid
            g((-800, 0), (0, -700), 500, 400, DIE - 10, DIE - 10),
            // skew grid
            g((900, 350), (-200, 800), 600, 500, DIE / 2, 0),
            // 1-column grid (nb collapse exercised on split)
            g((0, 2000), (1, 0), 400, 1, DIE / 3, 0),
            // anchor so the layer has a non-grid too
            pts_rec(3, 200_000, DIE, 50),
        ]);
        assert_conserved(&doc);
    }

    /// a SINGLE over-target Pts record must split by itself (the
    /// old recs.len()>64 gate left it as one whole-die page)
    #[test]
    fn single_huge_pts_splits_alone() {
        const DIE: i64 = 1_000_000;
        let doc = mini_doc(vec![pts_rec(11, 400_000, DIE, 90)]);
        let plan = assert_conserved(&doc);
        assert!(
            plan.pages.len() >= 2,
            "single huge rep stayed one page"
        );
    }

    /// unsplittable wide Rep::One records (die ring / spine) are
    /// quarantined into oversize pages; LOCAL pages stay tight
    #[test]
    fn wide_one_quarantined_local_pages_tight() {
        const DIE: i64 = 1_000_000;
        let mut recs = vec![
            pts_rec(7, 300_000, DIE, 100),
            pts_rec(15, 300_000, DIE, 100),
        ];
        for k in 0..40i64 {
            recs.push(RectRec {
                layer: 1,
                dt: 0,
                x: 0,
                y: k * 20_000,
                w: DIE, // full-die spine
                h: 50,
                rep: Rep::One,
            });
        }
        let doc = mini_doc(recs);
        let plan = assert_conserved(&doc);
        assert!(plan.split_stats.oversize_pages >= 1);
        // pages NOT containing a spine must stay narrow (LOD
        // variants of spine-bearing pages fuse the spine into
        // coverage, so the exact partition is what's gated)
        let mut tight = 0;
        for j in plan
            .pages
            .iter()
            .filter(|j| j.lod == floe_ovm::LOD_EXACT)
        {
            let has_spine =
                j.recs.iter().any(|r| r.w == DIE);
            if !has_spine {
                assert!(
                    j.bbox.x1 - j.bbox.x0 <= DIE * 7 / 10,
                    "local page polluted to {}",
                    j.bbox.x1 - j.bbox.x0
                );
                tight += 1;
            }
        }
        assert!(tight >= 2, "no local pages left");
    }

    /// rasterize members onto the LOD grid of `bb` with the SAME
    /// inclusive cell mapping the generator uses
    fn raster(mems: &[Mem], bb: &BBox) -> Vec<bool> {
        let g = LOD_GRID;
        let (bw, bh) = (bb.x1 - bb.x0, bb.y1 - bb.y0);
        let gx = |x: i64| -> i64 {
            (((x - bb.x0) as i128 * g as i128) / bw as i128)
                .clamp(0, (g - 1) as i128) as i64
        };
        let gy = |y: i64| -> i64 {
            (((y - bb.y0) as i128 * g as i128) / bh as i128)
                .clamp(0, (g - 1) as i128) as i64
        };
        let mut bits = vec![false; (g * g) as usize];
        for m in mems {
            for y in gy(m.4)..=gy(m.6) {
                for x in gx(m.3)..=gx(m.5) {
                    bits[(y * g + x) as usize] = true;
                }
            }
        }
        bits
    }

    fn dilate(bits: &[bool]) -> Vec<bool> {
        let g = LOD_GRID;
        let mut out = bits.to_vec();
        for y in 0..g {
            for x in 0..g {
                if !bits[(y * g + x) as usize] {
                    continue;
                }
                for (dx, dy) in
                    [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)]
                {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && nx < g && ny >= 0 && ny < g {
                        out[(ny * g + nx) as usize] = true;
                    }
                }
            }
        }
        out
    }

    fn page_mems(
        doc: &Doc,
        plan: &CellPlan,
        k: usize,
    ) -> Vec<Mem> {
        let (payload, _raw) =
            encode_job(doc, &plan.pages[k], &plan.arena);
        let pd = floe_oasis::doc::parse_doc(&payload)
            .expect("page parse");
        expand_cell(&pd.cells[0])
    }

    /// M7 contract: the LOD variant's coverage is a SUPERSET of
    /// the exact page's at grid resolution, and overcoverage is
    /// bounded by one cell of dilation. Links must point at a
    /// MERGED page of the same (layer, seq).
    #[test]
    fn lod_coverage_superset_and_bounded() {
        const DIE: i64 = 1_000_000;
        let doc = mini_doc(vec![
            pts_rec(7, 200_000, DIE, 100),
            pts_rec(99, 200_000, DIE, 150),
        ]);
        let plan = plan_of(&doc);
        let mut checked = 0;
        for k in 0..plan.pages.len() {
            let e = &plan.pages[k];
            if e.lod != floe_ovm::LOD_EXACT
                || e.lod_page == floe_ovm::LOD_PAGE_NONE
            {
                continue;
            }
            let l = &plan.pages[e.lod_page as usize];
            assert_eq!(l.lod, floe_ovm::LOD_MERGED);
            assert_eq!((l.li, l.seq), (e.li, e.seq));
            let bb = e.bbox;
            let eb = raster(&page_mems(&doc, &plan, k), &bb);
            let lb = raster(
                &page_mems(&doc, &plan, e.lod_page as usize),
                &bb,
            );
            for i in 0..eb.len() {
                assert!(
                    !eb[i] || lb[i],
                    "LOD hole at cell {} of page {}",
                    i,
                    k
                );
            }
            let ok = dilate(&eb);
            for i in 0..lb.len() {
                assert!(
                    !lb[i] || ok[i],
                    "LOD overcover beyond 1 cell at {} of page {}",
                    i,
                    k
                );
            }
            checked += 1;
        }
        assert!(checked >= 1, "no LOD pages generated");
    }

    /// sparse pages stay LOD-free (trigger threshold)
    #[test]
    fn lod_trigger_skips_sparse() {
        const DIE: i64 = 1_000_000;
        let doc = mini_doc(vec![pts_rec(3, 1000, DIE, 90)]);
        let plan = plan_of(&doc);
        assert!(plan
            .pages
            .iter()
            .all(|j| j.lod == floe_ovm::LOD_EXACT
                && j.lod_page == floe_ovm::LOD_PAGE_NONE));
    }

    /// records large in BOTH axes ride the LOD payload verbatim -
    /// their exact outlines survive; only sub-cell dust is fused
    #[test]
    fn lod_passthrough_keeps_large_records() {
        const DIE: i64 = 1_000_000;
        let mut recs = vec![pts_rec(7, 200_000, DIE, 100)];
        // a big block: DIE/8 x DIE/8 >> 4 cells at G=128
        recs.push(RectRec {
            layer: 1,
            dt: 0,
            x: DIE / 3,
            y: DIE / 3,
            w: DIE / 8,
            h: DIE / 8,
            rep: Rep::One,
        });
        let doc = mini_doc(recs);
        let plan = plan_of(&doc);
        let mut found = false;
        for k in 0..plan.pages.len() {
            let e = &plan.pages[k];
            if e.lod_page == floe_ovm::LOD_PAGE_NONE {
                continue;
            }
            let mems =
                page_mems(&doc, &plan, e.lod_page as usize);
            if mems.iter().any(|m| {
                m.5 - m.3 == DIE / 8 && m.6 - m.4 == DIE / 8
            }) {
                found = true;
            }
        }
        assert!(found, "big record not passed through verbatim");
    }

    /// plane ownership is explicit: member center*2 < plane2 goes
    /// left, >= plane2 goes right (points exactly ON the plane are
    /// right-owned)
    #[test]
    fn plane_ownership_boundary() {
        let mut c = floe_oasis::doc::Cell::default();
        c.name = String::from("T");
        c.rects = vec![RectRec {
            layer: 1,
            dt: 0,
            x: 0,
            y: 0,
            w: 10,
            h: 10,
            rep: Rep::Pts(vec![
                (0, 0),
                (495, 0), // center*2 = 1000 == plane2 -> RIGHT
                (494, 0), // center*2 = 998 < plane2 -> LEFT
                (1000, 0),
            ].into()),
        }];
        let doc = mini_doc(c.rects.clone());
        let r = PRec {
            kind: 0,
            idx: 0,
            bbox: BBox { x0: 0, y0: 0, x1: 1010, y1: 10 },
            bytes: 32,
            members: 4,
            w: 10,
            h: 10,
            frag: Frag::Whole,
        };
        let mut arena: Arena = Vec::new();
        let (a, b) = frag_split(
            &doc.cells[0],
            &mut arena,
            &r,
            0,
            1000, // plane2 (doubled): plane at x-center 500
        )
        .expect("splittable");
        assert_eq!(a.members, 2, "left: (0,0) and (494,0)");
        assert_eq!(b.members, 2, "right: (495,0) and (1000,0)");
        assert!(a.bbox.x1 <= 504 + 10);
        assert!(b.bbox.x0 >= 495);
    }
}
