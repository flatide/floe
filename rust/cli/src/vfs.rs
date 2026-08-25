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
use floe_oasis::write::WCell;
use floe_ovm::{narrow_u32, BBox, Builder, PBVH_NONE};
use floe_tiler::hier::{cell_bboxes_full, rep_extent};
use floe_tiler::{path_bbox, xf_rep, Xf};
use std::io::Write;

const DEFAULT_PAGE_TARGET_MB: u64 = 1;
const MIB: u64 = 1 << 20;
/// Encoded payloads are retained for at most one ordered batch. The old
/// implementation encoded every page before writing design.ovp, so peak RSS
/// included the complete payload file (tens of GB on the 9.8G asset).
/// Retained payloads cost ~hundreds of KB each, so a deep batch is nearly
/// free RSS and spaces the encode barriers out (~5% on the bench sweep).
const ENCODE_BATCH_PER_JOB: usize = 8;
const ENCODE_BATCH_MAX: usize = 256;
/// Completed CellPlans retain fragment arenas and Morton-prepared placement
/// points until their batch is encoded - plan_batch is the dominant RSS
/// knob. The old default min(jobs, 16) had a real defect: the plan phase
/// spawns min(jobs, batch_len) threads, so any host with more than 16
/// cores planned on 16 of them and idled the rest. Default is now
/// PLAN_BATCH_PER_JOB x jobs (thread-starvation impossible, stragglers
/// amortized over a deeper queue); the governor below walks it back when
/// memory runs short. Batch size never changes the output bytes (metadata
/// is appended in cell order regardless - the jobs-determinism gates build
/// with different batch sizes and byte-compare).
const PLAN_BATCH_PER_JOB: usize = 4;
/// dynamic plan-batch governor: when MemAvailable drops below this,
/// halve the next batch (never below `jobs` - that would re-starve the
/// plan threads). Linux-only signal; hosts without it keep the default.
const GOVERNOR_MIN_AVAIL_GB: f64 = 4.0;
const BVH_LEAF: usize = 8;
/// pages per page-BVH leaf, and the run size at or below which a
/// (cell,layer) run gets no BVH at all (linear scan, root = NONE)
const PBVH_LEAF: usize = 8;
/// texts per text-BVH leaf / linear-scan threshold (v5 text index)
const TBVH_LEAF: usize = 8;


// ------------------------------------------------------------ build

pub fn vfs_cmd(args: &[String]) {
    let mut src: Option<String> = None;
    let mut outdir: Option<String> = None;
    let mut jobs: Option<usize> = None;
    let mut encode_batch: Option<usize> = None;
    let mut plan_batch: Option<usize> = None;
    let mut page_target_mb: Option<u64> = None;
    // Gate-only fault injection is still explicit CLI state so test
    // behavior never depends on the caller's shell environment.
    let mut kill_at: Option<String> = None;
    // coverage bitplanes are optional: off by default (extra build
    // time, and the viewer defaults to coverage off), --coverage to
    // include, --coverage-only to add design.ovc to an existing cache
    let mut coverage = false;
    let mut coverage_only = false;
    // rev 46b: recompute the meta.json minimap frontier from an
    // existing cache's design.ovm - no source parse, seconds even
    // on the 9.8G class
    let mut frontier_only = false;
    // #61: LOD variants are optional. They only serve the planner's
    // density gate, whose operating window the rev 41/43/45 ladder
    // largely absorbed - and their generation dominates monster-cell
    // build time (150M field: lod was ~half of a 164s cell plan).
    let mut lod = true;
    // slow-cell log threshold in seconds; 0 logs every cell (the
    // S6/S7 gates use it to observe fanout uptake). CLI state, not
    // an env var - same rule as --kill-at above.
    let mut slow_cell_s = 5.0f64;
    // explicit P2 shard-copy ceiling. Off-Linux there is no
    // MemAvailable, so without this flag the ceiling is UNBOUNDED
    // there - set it when indexing big files on macOS.
    let mut p2_shard_limit: Option<u64> = None;
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
            "--no-lod" => {
                lod = false;
                i += 1;
            }
            "--frontier-only" => {
                frontier_only = true;
                i += 1;
            }
            "--kill-at" => {
                kill_at = Some(args[i + 1].clone());
                i += 2;
            }
            "--slow-cell-s" => {
                slow_cell_s =
                    args[i + 1].parse().expect("slow cell seconds");
                i += 2;
            }
            "--p2-shard-limit-mb" => {
                let mb: u64 =
                    args[i + 1].parse().expect("shard limit MB");
                p2_shard_limit =
                    Some(mb.checked_mul(MIB).expect("shard limit"));
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
    let encode_batch = encode_batch.unwrap_or_else(|| {
        jobs.max(1)
            .saturating_mul(ENCODE_BATCH_PER_JOB)
            .min(ENCODE_BATCH_MAX)
    });
    let plan_batch = plan_batch.unwrap_or_else(|| {
        jobs.max(1).saturating_mul(PLAN_BATCH_PER_JOB)
    });
    let page_target_mb =
        page_target_mb.unwrap_or(DEFAULT_PAGE_TARGET_MB);
    let page_target_bytes = page_target_mb
        .checked_mul(MIB)
        .expect("limit exceeded: page target bytes");
    if frontier_only {
        let t0 = std::time::Instant::now();
        let v = floe_vfs::Vfs::open(&outdir).unwrap_or_else(|e| {
            eprintln!("--frontier-only: {}", e);
            std::process::exit(1);
        });
        let fj = frontier_json_planned(&v.ovm);
        patch_meta_frontier(&format!("{}/meta.json", outdir), &fj);
        eprintln!(
            "[vfs] frontier rebaked in {:.1}s -> {}/meta.json",
            t0.elapsed().as_secs_f64(),
            outdir
        );
        return;
    }
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
        // --kill-at: gate-only hook forcing death at the interrupt
        // points (tools/validate_vfs_marker.py).
        for f in [
            "design.ovm",
            "design.ovp",
            "design.ovt",
            "design.ovc",
            "labels.tsv",
            // legacy (pre-0.10) viewer file: scrub on rebuild so a
            // re-index actually reclaims the skeleton's bytes
            "skeleton.oas",
            "texts.tsv",
            "meta.json",
        ] {
            let _ = std::fs::remove_file(format!("{}/{}", outdir, f));
        }
        if kill_at.as_deref() == Some("marker-deleted") {
            eprintln!("[vfs] --kill-at marker-deleted");
            std::process::exit(9);
        }
        let (ovm_bytes, rbb, lmems, tstats) = build(
            &doc,
            size,
            mtime,
            &outdir,
            jobs,
            encode_batch,
            plan_batch,
            page_target_bytes,
            lod,
            slow_cell_s,
            p2_shard_limit,
        );
        if kill_at.as_deref() == Some("ovp-written") {
            eprintln!("[vfs] --kill-at ovp-written");
            std::process::exit(9);
        }
        // both payload files (ovp + ovt) exist, marker still absent
        if kill_at.as_deref() == Some("ovt-written") {
            eprintln!("[vfs] --kill-at ovt-written");
            std::process::exit(9);
        }
        // rev 46b: bake the minimap frontier through the real
        // planner. The built bytes move into an Ovm view (deep
        // validation included - a free extra gate) and move back
        // out for the design.ovm commit below.
        let (frontier, ovm_bytes) = {
            let ovm = floe_ovm::Ovm::from_bytes(ovm_bytes)
                .expect("reopen built ovm");
            let fj = frontier_json_planned(&ovm);
            match ovm.data {
                floe_ovm::Backing::Vec(v) => (fj, v),
                floe_ovm::Backing::Map(_) => unreachable!(),
            }
        };
        emit_viewer_side(&doc, &src, size, mtime, &outdir, &rbb,
                         &lmems, &tstats, &frontier);
        if coverage {
            write_coverage(&doc, &outdir, jobs);
        }
        if kill_at.as_deref() == Some("ovm-partial") {
            std::fs::write(
                format!("{}/design.ovm", outdir),
                &ovm_bytes[..ovm_bytes.len() / 2],
            )
            .expect("write partial ovm");
            eprintln!("[vfs] --kill-at ovm-partial");
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

/// rev 46b minimap frontier: baked at INDEX time through the real
/// planner - for every depth, plan_hier at a canonical fit scale
/// (nominal canvas, medium detail) and expand the WS frame set to
/// world boxes (floe_vfs::hier::frontier_boxes). The minimap is the
/// fit view's own frame set by construction; the old raw-hierarchy
/// DFS bake (rev 30) burned per-depth scan budgets in DFS order and
/// left regional holes on 184M-placement chips. The canonical
/// parameters ride in the meta object so the L9 gate can replay
/// them against vfsd mode=frontier and demand byte equality.
const FRONTIER_KEEP: usize = 6000;
/// depth buckets beyond this are meaningless on a minimap
const FRONTIER_DEPTH_CAP: u32 = 32;
/// canonical bake scale: a nominal canvas width at medium detail
const FRONTIER_CANVAS_PX: f64 = 1200.0;
const FRONTIER_CUT_PX: f64 = 3.0;

fn frontier_json_planned(v: &floe_ovm::Ovm) -> String {
    if v.n_cells == 0 {
        return "{\"depths\": []}".to_string();
    }
    let top = v.cell(v.top);
    let die = top.rbbox;
    let unit = v.unit;
    let die_um = ((die.x1 - die.x0).max(1) as f64 / unit)
        .max((die.y1 - die.y0).max(1) as f64 / unit)
        .max(1e-9);
    // the viewer's fit view: canvas px over the die + its 5% margin
    let px_per_um = FRONTIER_CANVAS_PX / (die_um * 1.05);
    let px_per_dbu = px_per_um / unit;
    let cut_dbu = (FRONTIER_CUT_PX / px_per_dbu) as i64;
    let vis = vec![0xffu8; (v.n_layers as usize + 7) / 8];
    let opts = floe_vfs::hier::HierOpts::default();
    let mut depths: Vec<String> = Vec::new();
    for d in 0..top.height.min(FRONTIER_DEPTH_CAP) {
        let req = floe_vfs::ViewReq {
            view: die,
            cut_dbu,
            vis: vis.clone(),
            depth: d,
            px_per_dbu,
        };
        let plan = floe_vfs::hier::plan_hier(v, &req, &opts);
        let (boxes, truncated) = floe_vfs::hier::frontier_boxes(
            v,
            &plan,
            FRONTIER_KEEP,
        );
        if truncated {
            eprintln!(
                "[vfs] frontier: depth {} hit the 8M expansion \
                 budget - minimap may under-sample dense regions",
                d
            );
        }
        let rows: Vec<String> = boxes
            .iter()
            .map(|(b, band)| {
                format!(
                    "[{},{},{},{},{}]",
                    b[0], b[1], b[2], b[3], band
                )
            })
            .collect();
        depths.push(format!("[{}]", rows.join(",")));
    }
    while depths.last().is_some_and(|d| d == "[]") {
        depths.pop();
    }
    format!(
        "{{\"keep\": {}, \"px_per_um\": {}, \"cut_px\": {}, \
         \"depths\": [{}]}}",
        FRONTIER_KEEP,
        px_per_um,
        FRONTIER_CUT_PX,
        depths.join(",")
    )
}

/// --frontier-only: splice a fresh "frontier" object into an
/// existing meta.json (the value holds no strings, so a balanced-
/// brace scan from its opening brace is exact)
fn patch_meta_frontier(path: &str, frontier: &str) {
    let meta = std::fs::read_to_string(path).expect("read meta");
    let key = "\"frontier\":";
    let k = meta.find(key).expect("meta has no frontier key");
    let open = k + key.len()
        + meta[k + key.len()..]
            .find('{')
            .expect("frontier value not an object");
    let mut depth = 0usize;
    let mut end = open;
    for (i, c) in meta[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = open + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(end > open, "unbalanced frontier object");
    let out =
        format!("{}{}{}", &meta[..open], frontier, &meta[end..]);
    std::fs::write(path, out).expect("write meta");
}

/// meta.json: the viewer-facing summary (dbu/bbox/grid/layers+
/// color/src + text index tallies). The far-view skeleton is
/// retired (rev 24) and the text sidecars are too (T4,
/// VFS_TEXT_PLAN.md): labels are request-scoped daemon responses
/// from the v5 text index - nothing here walks the hierarchy, so
/// this stage no longer scales with path expansion (the old
/// collect_all_texts residency, par.6 risk 0, is GONE; the minimap
/// frontier walk is min-size-pruned and budget-capped). grid
/// stays synthetic on the .tiles formula (legacy meta shape).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn emit_viewer_side(
    doc: &Doc,
    src: &str,
    size: u64,
    mtime: u64,
    outdir: &str,
    rbb: &[Option<(i64, i64, i64, i64)>],
    lmems: &[u64],
    tstats: &TextIndexStats,
    frontier: &str,
) {
    let t0 = std::time::Instant::now();
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
        .map(|&(l, d)| {
            let name = doc
                .layer_names
                .get(&(l, d))
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{}/{}", l, d));
            let aliases = doc
                .layer_aliases
                .get(&(l, d))
                .map(|names| {
                    names
                        .iter()
                        .map(|s| format!("\"{}\"", crate::jesc(s)))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            format!(
                "{{\"layer\": {}, \"datatype\": {}, \"name\": \
                 \"{}\", \"aliases\": [{}], \"color\": \"{}\", \
                 \"stored_shapes\": {}}}",
                l,
                d,
                crate::jesc(&name),
                aliases,
                crate::layer_color(l as usize),
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
         \"texts\": {{\"records\": {}, \"members\": {}, \
         \"cells\": {}, \"grid_reps\": {}, \"pts_reps\": {}, \
         \"ovt_bytes\": {}}},\n\
         \"frontier\": {}\n\
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
        tstats.records,
        tstats.members,
        tstats.cells,
        tstats.grid_reps,
        tstats.pts_reps,
        tstats.string_bytes + tstats.pts_bytes,
        frontier,
    );
    std::fs::write(format!("{}/meta.json", outdir), meta)
        .expect("write meta");
    eprintln!(
        "[vfs] meta ({:.1}s, rss {})",
        t0.elapsed().as_secs_f64(),
        rss()
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
    /// splitting; sibling ranges are disjoint by construction).
    /// `arena` indexes the owning ARENA SHARD (#60 P1/P2: one per
    /// serial/P1 layer, one per P2 frontier task) - the page's
    /// arena_slot names which shard that is.
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

/// arena_slot of a page whose recs reference no Pts scratch
const ARENA_NONE: u32 = u32::MAX;

/// resolve a page's arena shard from the cell's compact shard list
/// (slots exist only for shards that hold fragmented-Pts scratch)
fn arena_at(arenas: &[Arena], slot: u32) -> &Arena {
    static EMPTY: Arena = Vec::new();
    if slot == ARENA_NONE {
        &EMPTY
    } else {
        &arenas[slot as usize]
    }
}

#[derive(Default, Clone, Copy)]
struct SplitStats {
    fragments: u64,
    oversize_pages: u64,
    depth_capped: u64,
    lod_pages: u64,
    lod_grid_verbatim: u64,
}

/// hard recursion cap (records above it emit as-is + stat)
const SPLIT_MAX_DEPTH: u32 = 64;

/// split fanout (#60 P1): cap on threads per cell - each live
/// layer task holds its own recursion temporaries, so this is a
/// memory bound as much as a scheduling one
const SPLIT_PAR_MAX: usize = 8;
/// fanout engages only past this much split input (sum of record
/// byte estimates); below it a cell splits in well under a second
/// and thread setup is pure overhead
const SPLIT_PAR_MIN_BYTES: u64 = 4 * MIB;
/// P2 (#60) dominant-layer floor: below this even a dominant layer
/// is fast enough serially and the sharding copies don't amortize
const P2_MIN_BYTES: u64 = 8 * MIB;
/// P2 frontier cutoff floor - stops skewed splits from minting
/// piles of tiny sibling tasks
const P2_TASK_MIN_BYTES: u64 = 2 * MIB;
/// P2 hard cap on frontier tasks per layer
const P2_TASK_CAP: usize = 32;

/// Borrow one runnable planning slot without blocking. Cell planners, P1/P2
/// split helpers and LOD helpers all draw from the same counter, so the number
/// of CPU-active planning tasks never exceeds `--jobs`. A persistent cell
/// planner returns its slot while it is stalled beyond the ordered plan window
/// and must borrow it again before touching the next cell (#76).
fn try_borrow_plan_slot(
    free: &std::sync::atomic::AtomicIsize,
) -> bool {
    loop {
        let cur = free.load(std::sync::atomic::Ordering::Relaxed);
        if cur <= 0 {
            return false;
        }
        if free
            .compare_exchange(
                cur,
                cur - 1,
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
            )
            .is_ok()
        {
            return true;
        }
    }
}

/// Reserve up to `want` parked late-helper OS threads. These waiters do not
/// own a runnable planning slot until work is still pending and a planner
/// lends one, so they fix the one-shot #76 borrow without oversubscribing
/// CPU-active work. A separate cap prevents many concurrent cells from
/// multiplying the persistent planner thread count without bound.
fn reserve_late_waiters(
    free: Option<&std::sync::atomic::AtomicIsize>,
    want: usize,
) -> usize {
    let Some(free) = free else { return 0 };
    let mut got = 0usize;
    while got < want && try_borrow_plan_slot(free) {
        got += 1;
    }
    got
}

struct ReturnAtomicPermit<'a>(&'a std::sync::atomic::AtomicIsize);

impl Drop for ReturnAtomicPermit<'_> {
    fn drop(&mut self) {
        self.0
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// P2 (#60) frontier options. `threads` is the desired ceiling, not just the
/// helpers available at cell entry: parked late helpers may borrow slots
/// returned while the frontier is already running (#76b). The determinism
/// unit test forces threads = 1 to prove that frontier expansion, arena
/// sharding and the segment merge alone are byte-neutral vs the serial
/// recursion.
struct P2Opts<'a> {
    /// total split threads, this thread included
    threads: usize,
    /// frontier granularity target (threads x 4, capped)
    target_tasks: usize,
    /// frontier cutoff floor (production: P2_TASK_MIN_BYTES;
    /// small-fixture unit tests lower it to form a real frontier)
    task_min: u64,
    /// explicit ceiling on transient arena-sharding copies
    /// (--p2-shard-limit-mb, or a unit-test pin). None = derive
    /// from MemAvailable AT DECISION TIME - after the prefix ran
    /// and other layers/planners allocated, not at cell entry
    /// (review 0.11.43: the early snapshot missed both).
    shard_limit: Option<u64>,
    /// the shared plan-thread budget + this cell's borrowed lease:
    /// the memory fallback returns the lease IMMEDIATELY (it runs
    /// on one thread) so the tail can re-lend it to LOD or other
    /// cells instead of holding idle helpers to layer-plan end
    budget: Option<&'a std::sync::atomic::AtomicIsize>,
    lease: usize,
    /// bounded permits for helper OS threads that wait for a planning slot
    late_waiters: Option<&'a std::sync::atomic::AtomicIsize>,
    /// actual helpers that executed at least one frontier worker loop
    helpers_used: Option<&'a std::sync::atomic::AtomicUsize>,
    /// deterministic late-lending gate; production never sets this
    join_barrier: Option<&'a std::sync::Barrier>,
}

/// bytes currently reserved by in-flight P2 shardings across all
/// planner threads: two concurrent P2 cells must not budget the
/// SAME MemAvailable headroom (review 0.11.43 #1). Reserved from
/// the copy decision until task execution ends - past that the
/// copies are ordinary plan memory under the window governor.
static SHARD_RESERVED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// sharding headroom in bytes: the explicit cap when given, else
/// half the current MemAvailable above the governor floor;
/// unbounded when neither exists (off-Linux without the flag)
fn shard_headroom(explicit: Option<u64>) -> u64 {
    match explicit {
        Some(v) => v,
        None => crate::mem_available_gb()
            .map(|gb| {
                (((gb - GOVERNOR_MIN_AVAIL_GB).max(0.0) / 2.0)
                    * (1u64 << 30) as f64) as u64
            })
            .unwrap_or(u64::MAX),
    }
}

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
            // partition + per-side offset extents in ONE pass (the
            // two bbox rescans doubled the per-member cost on fill
            // monsters). Each source member is classified exactly
            // once - a swap pulls an already-classified element
            // into a position k has passed - so the extents equal
            // a rescan of the final slices.
            let mut m = lo;
            let (mut l0, mut l1) =
                ((i64::MAX, i64::MAX), (i64::MIN, i64::MIN));
            let (mut r0, mut r1) =
                ((i64::MAX, i64::MAX), (i64::MIN, i64::MIN));
            for k in lo..hi {
                let (ox, oy) = pts[order[k] as usize];
                let c = if ax == 0 { ox } else { oy };
                if 2 * c + sc2 < plane2 {
                    order.swap(k, m);
                    m += 1;
                    l0 = (l0.0.min(ox), l0.1.min(oy));
                    l1 = (l1.0.max(ox), l1.1.max(oy));
                } else {
                    r0 = (r0.0.min(ox), r0.1.min(oy));
                    r1 = (r1.0.max(ox), r1.1.max(oy));
                }
            }
            if m == lo || m == hi {
                return None; // coincident pile: one side empty
            }
            let base =
                r.bytes.saturating_sub(rep_est_n(r.members));
            let shift = |o0: (i64, i64), o1: (i64, i64)| BBox {
                x0: sb.x0 + o0.0,
                y0: sb.y0 + o0.1,
                x1: sb.x1 + o1.0,
                y1: sb.y1 + o1.1,
            };
            let bl = shift(l0, l1);
            let br = shift(r0, r1);
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
    /// which compact arena shard this page's Frag::Pts indices
    /// address (ARENA_NONE when it references none); assigned by
    /// the layer/P2 merge, inherited by the page's LOD variant
    arena_slot: u32,
    seq: u32,
    recs: Vec<PRec>,
    bbox: BBox,
    members: u64,
    max_w: i64,
    max_h: i64,
    /// max over records of min(w,h) - hairline-page detector (v6)
    max_min: i64,
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
    let (mut max_w, mut max_h, mut max_min) = (0i64, 0i64, 0i64);
    for r in recs.iter() {
        bb.grow(&r.bbox);
        members += r.members;
        max_w = max_w.max(r.w);
        max_h = max_h.max(r.h);
        max_min = max_min.max(r.w.min(r.h));
    }
    out.push(PageJob {
        ci,
        li,
        arena_slot: ARENA_NONE, // assigned at the layer merge
        seq: *seq,
        recs,
        bbox: bb,
        members,
        max_w,
        max_h,
        max_min,
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
/// the recursion decision of one split node - shared verbatim by
/// the serial recursion and the P2 frontier expansion so both
/// produce identical bytes
enum NodeStep {
    /// under target, unsplittable, depth-capped or spatially
    /// irreducible: emit as ONE page (after this node's oversize
    /// pages)
    Leaf(Vec<PRec>),
    /// everything went to oversize pages at this node
    Drained,
    /// recurse left then right (after this node's oversize pages)
    Split { lv: Vec<PRec>, rv: Vec<PRec> },
}

/// one split_pages node: classify, partition, quarantine - this
/// node's oversize (wide) pages come back as packed record groups
/// in `oversize` and MUST be emitted before anything the returned
/// step produces (emission order = page seq = bytes)
#[allow(clippy::too_many_arguments)]
fn split_node(
    cell: &floe_oasis::doc::Cell,
    arena: &mut Arena,
    recs: Vec<PRec>,
    st: &mut SplitStats,
    page_target_bytes: u64,
    depth: u32,
    oversize: &mut Vec<Vec<PRec>>,
) -> NodeStep {
    let bytes: u64 = recs.iter().map(|r| r.bytes as u64).sum();
    // the split gate is BYTES, not record count - with repetitions
    // a handful of records can be gigabytes, and page decode cost
    // is what the viewer pays. A lone record still splits when its
    // rep can fragment.
    let splittable = recs.len() >= 2
        || recs.iter().any(|r| can_frag(cell, r));
    if bytes <= page_target_bytes || !splittable {
        return NodeStep::Leaf(recs);
    }
    if depth >= SPLIT_MAX_DEPTH {
        st.depth_capped += 1;
        return NodeStep::Leaf(recs);
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
            oversize.push(std::mem::take(&mut acc));
            acc_bytes = 0;
        }
        acc_bytes += r.bytes as u64;
        acc.push(r);
    }
    if !acc.is_empty() {
        st.oversize_pages += 1;
        oversize.push(acc);
    }
    if lv.is_empty() && rv.is_empty() {
        return NodeStep::Drained;
    }
    if lv.is_empty() || rv.is_empty() {
        // nothing separable at this plane (coincident pile):
        // over-target but spatially irreducible - emit as-is
        lv.extend(rv);
        return NodeStep::Leaf(lv);
    }
    NodeStep::Split { lv, rv }
}

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
    let mut oversize: Vec<Vec<PRec>> = Vec::new();
    let step = split_node(
        cell,
        arena,
        recs,
        st,
        page_target_bytes,
        depth,
        &mut oversize,
    );
    for grp in oversize {
        emit_page(ci, li, grp, seq, out);
    }
    let (lv, rv) = match step {
        NodeStep::Leaf(r) => {
            emit_page(ci, li, r, seq, out);
            return;
        }
        NodeStep::Drained => return,
        NodeStep::Split { lv, rv } => (lv, rv),
    };
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
    arenas: &[Arena],
) -> (Vec<u8>, u64) {
    let cell = &doc.cells[job.ci];
    let arena = arena_at(arenas, job.arena_slot);
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
    b.page_max_min(job.max_min.max(0) as u64);
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
    arenas: &[std::sync::Arc<Vec<Arena>>],
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
    let arena_for = |job: &PageJob| -> &[Arena] {
        let ai = job
            .ci
            .checked_sub(arena_cell_base)
            .expect("page cell before arena batch");
        arenas
            .get(ai)
            .expect("page cell after arena batch")
            .as_slice()
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
    /// plan sub-phase seconds for the slow-cell log:
    /// [bvh, assemble, split+pbvh, lod, pts, sink]
    phase_s: [f32; 6],
    /// pre-encoded sections with CELL-LOCAL indexes (built in the
    /// parallel plan phase; the serial commit is memcpy + rebase)
    sink: floe_ovm::CellSink,
    place_order: Vec<u32>,            // local place idx, leaf order
    // (bbox, first, count, leaf, max_dim, max_min) -
    // `first` is cell-local; v7 size annotations ride along
    bvh: Vec<(BBox, u32, u16, bool, u32, u32)>,
    /// pages across this cell's (layer) runs, each run already in
    /// its page-BVH leaf order (seq is a name, not a position)
    pages: Vec<PageJob>,
    /// (layer_idx, page_lo, page_count, pbvh_root) - page_lo and
    /// root are cell-local (root PBVH_NONE = linear run)
    pranges: Vec<(u32, u32, u32, u32)>,
    /// (bbox, first, count, leaf, max_w, max_h) - leaf `first` is a
    /// cell-local page index, inner `first` a cell-local node index
    pbvh: Vec<(BBox, u32, u16, bool, u64, u64)>,
    /// fragmented-Pts offset scratch shared by this cell's jobs:
    /// compact ARENA SHARDS - one per serial/P1 layer that
    /// fragmented a Pts rep, one per P2 frontier task (+ prefix).
    /// PageJob.arena_slot names a page's shard.
    arenas: std::sync::Arc<Vec<Arena>>,
    split_stats: SplitStats,
    /// #60 P1/P2 instrumentation for the slow-cell log
    split_threads: usize,
    lod_threads: usize,
    layers_active: usize,
    /// P2 frontier task count (0 = no subtree fanout ran)
    p2_tasks: usize,
    /// bytes copied by P2 arena sharding
    shard_bytes: u64,
    /// P2 memory fallback ran (tasks serial, lease pre-returned)
    p2_fallback: bool,
    /// P2-eligible but no helper joined before its useful frontier work ended
    p2_starved: bool,
    /// ((layer, datatype), split seconds), heaviest first, <= 3
    top_split: Vec<((u32, u32), f32)>,
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

/// M7 LOD trigger: pages this dense get a merged-coverage variant.
/// M7-C field finding (sample9, fit view 0.044 px/um, depth 9): at
/// 4096 only 800 of 10560 pages had variants - the other ~9000
/// average ~1400 members each and drew EXACT, 7.5M records into a
/// 2.2M px screen (x3.4 saturation, hairline strokes bypass the
/// speckle). Zoom-driven saturation is invisible to any absolute
/// build threshold, so the floor is low: sub-256-member pages
/// cannot saturate a fit view even en masse, and a variant that
/// turns out useless is a few coverage rects in the ovp.
const LOD_MIN_MEMBERS: u64 = 256;
use floe_ovm::LOD_GRID;
/// records whose SHAPE spans at least this many grid cells in BOTH
/// axes pass through verbatim (individually visible blobs keep
/// their exact outline; only sub-cell "dust" is fused)
const LOD_PASS_CELLS: i64 = 4;
/// per-record member cap for LOD enumeration of SKEW grids -
/// orthogonal grids mark analytically at any count, but a skew
/// mega-grid (tiny OASIS bytes, 10^12 members) must never loop
/// (review finding); above the cap it passes through verbatim
const LOD_ENUM_CAP: u64 = 1 << 16;

/// cells [0,g) touched on one axis by the intervals
/// [lo + m*step, hi + m*step], m in [0,n) - per-cell existence
/// test, O(g) for ANY n (the analytic replacement for member
/// loops on orthogonal grids)
fn lod_axis_cells(
    lo: i64,
    hi: i64,
    step: i64,
    n: i64,
    b0: i64,
    bext: i64,
    g: i64,
) -> Vec<bool> {
    let mut out = vec![false; g as usize];
    if n <= 0 || bext <= 0 {
        return out;
    }
    let cell = |k: i64| -> i64 {
        b0 + (k as i128 * bext as i128 / g as i128) as i64
    };
    for k in 0..g {
        let (c0, c1) = (cell(k), cell(k + 1));
        let ok = if step == 0 {
            hi >= c0 && lo <= c1
        } else {
            // lo + m*step <= c1  AND  hi + m*step >= c0
            let (mlo, mhi) = if step > 0 {
                (
                    floe_tiler::div_ceil(c0 - hi, step),
                    floe_tiler::div_floor(c1 - lo, step),
                )
            } else {
                (
                    floe_tiler::div_ceil(c1 - lo, step),
                    floe_tiler::div_floor(c0 - hi, step),
                )
            };
            mlo.max(0) <= mhi.min(n - 1)
        };
        out[k as usize] = ok;
    }
    out
}

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
    st: &mut SplitStats,
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
        // rects: bbox IS the geometry, so bbox marking is exact and
        // only big-in-both-axes blobs need to stay verbatim.
        // polys/paths: bbox marking is only within the <=1-cell
        // overcoverage contract while the whole bbox fits ONE cell
        // (review finding: a 3-cell concave outline would smear) -
        // anything larger rides verbatim.
        let verbatim = match r.kind {
            0 => cw >= LOD_PASS_CELLS && ch >= LOD_PASS_CELLS,
            _ => cw > 1 || ch > 1,
        };
        if verbatim {
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
            (frag, Rep::Grid { na, nb, va, vb }) => {
                let (va, vb) = (*va, *vb);
                let (i0, i1, j0, j1) = match frag {
                    Frag::Grid { i0, i1, j0, j1 } => {
                        (i0 as i64, i1 as i64, j0 as i64, j1 as i64)
                    }
                    _ => (0, *na as i64, 0, *nb as i64),
                };
                let (ni, nj) = (i1 - i0, j1 - j0);
                let ortho = (va.1 == 0 && vb.0 == 0)
                    || (va.0 == 0 && vb.1 == 0);
                if ortho {
                    // analytic cell coverage: O(grid) for ANY
                    // member count (review finding: a 10^6 x 10^6
                    // grid record is bytes-tiny and stays in one
                    // page, but a member loop would be 10^12)
                    let (bx0, by0) =
                        (i0 * va.0 + j0 * vb.0, i0 * va.1 + j0 * vb.1);
                    let (xs_step, xs_n, ys_step, ys_n) =
                        if va.1 == 0 && vb.0 == 0 {
                            (va.0, ni, vb.1, nj)
                        } else {
                            (vb.0, nj, va.1, ni)
                        };
                    let xs = lod_axis_cells(
                        sb.x0 + bx0,
                        sb.x1 + bx0,
                        xs_step,
                        xs_n,
                        bb.x0,
                        bw,
                        g,
                    );
                    let ys = lod_axis_cells(
                        sb.y0 + by0,
                        sb.y1 + by0,
                        ys_step,
                        ys_n,
                        bb.y0,
                        bh,
                        g,
                    );
                    let mut rowmask =
                        vec![0u64; words_per_row];
                    for (x, &on) in xs.iter().enumerate() {
                        if on {
                            rowmask[x >> 6] |= 1u64 << (x & 63);
                        }
                    }
                    for (y, &on) in ys.iter().enumerate() {
                        if on {
                            let row = y * words_per_row;
                            for w in 0..words_per_row {
                                bits[row + w] |= rowmask[w];
                            }
                        }
                    }
                } else if (ni as u64 * nj as u64) > LOD_ENUM_CAP {
                    // skew mega-grid: never loop it - the record
                    // rides verbatim (exactness kept; counted so
                    // the cap is never silent)
                    st.lod_grid_verbatim += 1;
                    pass_members += r.members;
                    pass.push(r.clone());
                    continue;
                } else {
                    for i in i0..i1 {
                        for j in j0..j1 {
                            mark(
                                i * va.0 + j * vb.0,
                                i * va.1 + j * vb.1,
                            );
                        }
                    }
                }
            }
            (Frag::Whole, Rep::One) => mark(0, 0),
            (Frag::Whole, Rep::Pts(pl)) => {
                for &(ox, oy) in pl.iter() {
                    mark(ox, oy);
                }
            }
            (Frag::Grid { .. }, _) => {
                unreachable!("grid frag on non-grid")
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
    let (mut max_w, mut max_h, mut max_min) = (0i64, 0i64, 0i64);
    for rc in &rects {
        lbb.grow(&BBox {
            x0: rc.x,
            y0: rc.y,
            x1: rc.x + rc.w,
            y1: rc.y + rc.h,
        });
        max_w = max_w.max(rc.w);
        max_h = max_h.max(rc.h);
        max_min = max_min.max(rc.w.min(rc.h));
    }
    for r in &pass {
        lbb.grow(&r.bbox);
        max_w = max_w.max(r.w);
        max_h = max_h.max(r.h);
        max_min = max_min.max(r.w.min(r.h));
    }
    Some(PageJob {
        ci: exact.ci,
        li: exact.li,
        arena_slot: exact.arena_slot,
        seq: exact.seq,
        members: pass_members + rects.len() as u64,
        recs: pass,
        bbox: lbb,
        max_w,
        max_h,
        max_min,
        lod: floe_ovm::LOD_MERGED,
        lod_page: floe_ovm::LOD_PAGE_NONE,
        lod_rects: rects,
    })
}

/// one layer's split product with RUN-LOCAL indices (#60 P1): the
/// li-order merge in build_cell_plan rebases pages/pbvh onto
/// cell-local bases, so output bytes are independent of task
/// scheduling and thread count
struct LayerPlan {
    li: u32,
    /// pbvh leaf order when a tree exists, split emit order else.
    /// arena_slot is LAYER-LOCAL here (an index into `arenas`);
    /// the cell merge rebases it onto the cell's shard list.
    pages: Vec<PageJob>,
    /// empty = short run (linear scan, prange root PBVH_NONE);
    /// leaf `first` is a run-local page index, inner `first` a
    /// run-local node index
    pbvh: Vec<(BBox, u32, u16, bool, u64, u64)>,
    /// Pts scratch shards, non-empty only: one for a serial/P1
    /// layer, one per frontier task (+ prefix) under P2
    arenas: Vec<Arena>,
    stats: SplitStats,
    /// P2 frontier task count (0 = serial split)
    p2_tasks: usize,
    /// bytes copied by P2 arena sharding (0 = serial split)
    shard_bytes: u64,
    /// P2 memory fallback ran (frontier kept, tasks serial, no
    /// copies, helper lease already returned)
    p2_fallback: bool,
    secs: f32,
}

fn plan_layer(
    cell: &floe_oasis::doc::Cell,
    ci: usize,
    li: u32,
    recs: Vec<PRec>,
    page_target_bytes: u64,
) -> LayerPlan {
    let t = std::time::Instant::now();
    let mut arena: Arena = Vec::new();
    let mut stats = SplitStats::default();
    let mut seq = 0u32;
    let mut run: Vec<PageJob> = Vec::new();
    split_pages(
        cell,
        &mut arena,
        ci,
        li,
        recs,
        &mut seq,
        &mut run,
        &mut stats,
        page_target_bytes,
        0,
    );
    let arenas = if arena.is_empty() {
        Vec::new()
    } else {
        for j in run.iter_mut() {
            j.arena_slot = 0; // the layer's single shard
        }
        vec![arena]
    };
    finish_layer(li, run, arenas, stats, 0, 0, false, t)
}

/// pbvh + leaf-order permute over a layer's finished emission run,
/// shared by the serial path and the P2 merge - the tree is built
/// ONCE per layer, after the complete page order exists
#[allow(clippy::too_many_arguments)]
fn finish_layer(
    li: u32,
    run: Vec<PageJob>,
    arenas: Vec<Arena>,
    stats: SplitStats,
    p2_tasks: usize,
    shard_bytes: u64,
    p2_fallback: bool,
    t: std::time::Instant,
) -> LayerPlan {
    let mut pbvh: Vec<(BBox, u32, u16, bool, u64, u64)> = Vec::new();
    let pages = if run.len() <= PBVH_LEAF {
        // short run: linear scan beats a tree
        run
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
        let mut nodes: Vec<(BBox, u32, u16, bool, u64, u64)> =
            vec![(BBox::EMPTY, 0, 0, false, 0, 0)];
        split_pbvh(&mut nodes, &mut items, 0, 0);
        pbvh = nodes;
        // the run's jobs in leaf order (items order after the
        // split IS the directory order)
        let mut slots: Vec<Option<PageJob>> =
            run.into_iter().map(Some).collect();
        let mut pages = Vec::with_capacity(slots.len());
        for &(_, _, _, k) in &items {
            pages.push(slots[k].take().expect("pbvh perm"));
        }
        pages
    };
    LayerPlan {
        li,
        pages,
        pbvh,
        arenas,
        stats,
        p2_tasks,
        shard_bytes,
        p2_fallback,
        secs: t.elapsed().as_secs_f32(),
    }
}

/// P2 (#60): split one dominant layer via a serial frontier
/// expansion plus parallel subtree tasks. The segment list records
/// the serial DFS emission order (a node's oversize pages, then
/// its left subtree, then its right), seq is re-assigned over the
/// merged run and the pbvh is built once at the end - so the
/// output bytes equal plan_layer's at any thread count.
fn plan_layer_frontier(
    cell: &floe_oasis::doc::Cell,
    ci: usize,
    li: u32,
    recs: Vec<PRec>,
    page_target_bytes: u64,
    opts: &P2Opts,
) -> LayerPlan {
    let t = std::time::Instant::now();
    let total: u64 = recs.iter().map(|r| r.bytes as u64).sum();
    let cutoff =
        (total / opts.target_tasks.max(1) as u64).max(opts.task_min);
    if total / 2 < cutoff {
        // the frontier would be a single task: plain serial is
        // the same work without the sharding copies
        return plan_layer(cell, ci, li, recs, page_target_bytes);
    }
    enum Seg {
        Pages(Vec<PageJob>),
        Task(usize),
    }
    // ---- serial prefix: DFS pre-order via explicit stack (right
    // child pushed first so the left subtree pops - and merges -
    // first)
    let mut prefix: Arena = Vec::new();
    let mut stats = SplitStats::default();
    let mut segs: Vec<Seg> = Vec::new();
    let mut tasks: Vec<(Vec<PRec>, u32)> = Vec::new();
    let mut seq0 = 0u32; // placeholder ids, re-assigned at merge
    let mut stack: Vec<(Vec<PRec>, u32)> = vec![(recs, 0)];
    while let Some((recs, depth)) = stack.pop() {
        let bytes: u64 =
            recs.iter().map(|r| r.bytes as u64).sum();
        if bytes <= cutoff
            || tasks.len() + stack.len() + 1 >= P2_TASK_CAP
        {
            segs.push(Seg::Task(tasks.len()));
            tasks.push((recs, depth));
            continue;
        }
        let mut oversize: Vec<Vec<PRec>> = Vec::new();
        let step = split_node(
            cell,
            &mut prefix,
            recs,
            &mut stats,
            page_target_bytes,
            depth,
            &mut oversize,
        );
        let mut emitted: Vec<PageJob> = Vec::new();
        for grp in oversize {
            emit_page(ci, li, grp, &mut seq0, &mut emitted);
        }
        match step {
            NodeStep::Split { lv, rv } => {
                if !emitted.is_empty() {
                    segs.push(Seg::Pages(emitted));
                }
                stack.push((rv, depth + 1));
                stack.push((lv, depth + 1));
            }
            NodeStep::Leaf(r) => {
                emit_page(ci, li, r, &mut seq0, &mut emitted);
                segs.push(Seg::Pages(emitted));
            }
            NodeStep::Drained => {
                if !emitted.is_empty() {
                    segs.push(Seg::Pages(emitted));
                }
            }
        }
    }
    // ---- shard-copy ceiling (#60 review): the projected copies
    // are known exactly BEFORE any allocation happens. The limit
    // is evaluated HERE (not at cell entry) and admission goes
    // through the global reservation, so concurrent P2 cells
    // cannot each budget the same headroom. Past the ceiling the
    // frontier is kept but its tasks run serially against the
    // shared prefix arena - no copies, one thread, identical
    // bytes; the log shows p2_mem_fallback=1.
    let copy_bytes: u64 = tasks
        .iter()
        .flat_map(|(r, _)| r.iter())
        .map(|r| match r.frag {
            Frag::Pts { lo, hi, .. } => 4 * (hi - lo) as u64,
            _ => 0,
        })
        .sum();
    let mut reserved = false;
    if copy_bytes > 0 {
        let limit = shard_headroom(opts.shard_limit);
        loop {
            let cur = SHARD_RESERVED
                .load(std::sync::atomic::Ordering::Relaxed);
            if copy_bytes > limit.saturating_sub(cur) {
                break; // no headroom: serial fallback below
            }
            if SHARD_RESERVED
                .compare_exchange(
                    cur,
                    cur + copy_bytes,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
            {
                reserved = true;
                break;
            }
        }
    }
    if copy_bytes > 0 && !reserved {
        // memory fallback runs on ONE thread: hand the helper
        // lease back right away so the tail can re-lend it
        if let Some(b) = opts.budget {
            b.fetch_add(
                opts.lease as isize,
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        let p2_tasks = tasks.len();
        let mut outs: Vec<Vec<PageJob>> =
            Vec::with_capacity(tasks.len());
        for (recs, depth) in tasks {
            let mut out: Vec<PageJob> = Vec::new();
            let mut seq = 0u32;
            split_pages(
                cell,
                &mut prefix,
                ci,
                li,
                recs,
                &mut seq,
                &mut out,
                &mut stats,
                page_target_bytes,
                depth,
            );
            outs.push(out);
        }
        let mut arenas: Vec<Arena> = Vec::new();
        let slot = if prefix.is_empty() {
            ARENA_NONE
        } else {
            arenas.push(prefix);
            0u32
        };
        let mut run: Vec<PageJob> = Vec::new();
        for s in segs {
            match s {
                Seg::Pages(mut p) => {
                    for j in p.iter_mut() {
                        j.arena_slot = slot;
                    }
                    run.append(&mut p);
                }
                Seg::Task(k) => {
                    let mut out = std::mem::take(&mut outs[k]);
                    for j in out.iter_mut() {
                        j.arena_slot = slot;
                    }
                    run.append(&mut out);
                }
            }
        }
        for (i, j) in run.iter_mut().enumerate() {
            j.seq = narrow_u32(i as u64, "page seq");
        }
        return finish_layer(
            li, run, arenas, stats, p2_tasks, 0, true, t,
        );
    }
    // ---- arena sharding: copy each task's Frag::Pts ranges into
    // a task-local shard so parallel subtrees never touch shared
    // scratch. Entries referenced only by tasks free right after
    // their copies, so the transient overhead is one entry's
    // ranges, not the whole prefix arena.
    let mut retained = vec![false; prefix.len()];
    for s in &segs {
        if let Seg::Pages(pages) = s {
            for p in pages {
                for r in &p.recs {
                    if let Frag::Pts { arena, .. } = r.frag {
                        retained[arena as usize] = true;
                    }
                }
            }
        }
    }
    let mut refs: Vec<Vec<(usize, usize, u32, u32)>> =
        (0..prefix.len()).map(|_| Vec::new()).collect();
    for (ti, (recs, _)) in tasks.iter().enumerate() {
        for (ri, r) in recs.iter().enumerate() {
            if let Frag::Pts { arena, lo, hi } = r.frag {
                refs[arena as usize].push((ti, ri, lo, hi));
            }
        }
    }
    let mut shards: Vec<Arena> =
        (0..tasks.len()).map(|_| Arena::new()).collect();
    let mut shard_bytes = 0u64;
    for (a, list) in refs.into_iter().enumerate() {
        if list.is_empty() {
            continue;
        }
        let sb = prefix[a].shape_box;
        for (ti, ri, lo, hi) in list {
            let order =
                prefix[a].order[lo as usize..hi as usize].to_vec();
            shard_bytes += 4 * order.len() as u64;
            let na = narrow_u32(
                shards[ti].len() as u64,
                "fragment arena index",
            );
            let hi2 = narrow_u32(
                order.len() as u64,
                "fragment pts index",
            );
            shards[ti].push(PtsArena { shape_box: sb, order });
            tasks[ti].0[ri].frag =
                Frag::Pts { arena: na, lo: 0, hi: hi2 };
        }
        if !retained[a] {
            prefix[a].order = Vec::new(); // free promptly
        }
    }
    // ---- execute tasks, heaviest first; the segment list alone
    // fixes the merge order, so scheduling cannot change bytes
    let p2_tasks = tasks.len();
    let mut order: Vec<usize> = (0..tasks.len()).collect();
    order.sort_by_key(|&k| {
        std::cmp::Reverse(
            tasks[k].0.iter().map(|r| r.bytes as u64).sum::<u64>(),
        )
    });
    let cells: Vec<
        std::sync::Mutex<Option<(Vec<PRec>, u32, Arena)>>,
    > = tasks
        .into_iter()
        .zip(shards)
        .map(|((recs, depth), shard)| {
            std::sync::Mutex::new(Some((recs, depth, shard)))
        })
        .collect();
    let done: Vec<
        std::sync::Mutex<Option<(Vec<PageJob>, SplitStats, Arena)>>,
    > = (0..cells.len())
        .map(|_| std::sync::Mutex::new(None))
        .collect();
    let nextk = std::sync::atomic::AtomicUsize::new(0);
    let work = || loop {
        let i =
            nextk.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if i >= order.len() {
            return;
        }
        let k = order[i];
        let (recs, depth, mut arena) =
            cells[k].lock().unwrap().take().expect("p2 task");
        let mut st = SplitStats::default();
        let mut out: Vec<PageJob> = Vec::new();
        let mut seq = 0u32;
        split_pages(
            cell,
            &mut arena,
            ci,
            li,
            recs,
            &mut seq,
            &mut out,
            &mut st,
            page_target_bytes,
            depth,
        );
        *done[k].lock().unwrap() = Some((out, st, arena));
    };
    if opts.threads <= 1 || p2_tasks <= 1 {
        work();
    } else {
        let helper_cap = opts
            .threads
            .min(p2_tasks)
            .saturating_sub(1);
        let leased = opts.lease.min(helper_cap);
        let late = reserve_late_waiters(
            opts.late_waiters,
            helper_cap.saturating_sub(leased),
        );
        let stop = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|sc| {
            struct StopOnDrop<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for StopOnDrop<'_> {
                fn drop(&mut self) {
                    self.0.store(
                        true,
                        std::sync::atomic::Ordering::Release,
                    );
                }
            }
            let _stop_on_drop = StopOnDrop(&stop);
            for _ in 0..leased {
                sc.spawn(|| {
                    if let Some(used) = opts.helpers_used {
                        used.fetch_add(
                            1,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
                    work();
                });
            }
            for _ in 0..late {
                sc.spawn(|| {
                    let _waiter = ReturnAtomicPermit(
                        opts.late_waiters.expect("late waiter budget"),
                    );
                    loop {
                        if stop.load(
                            std::sync::atomic::Ordering::Acquire,
                        ) || nextk.load(
                            std::sync::atomic::Ordering::Relaxed,
                        ) >= order.len()
                        {
                            return;
                        }
                        let Some(budget) = opts.budget else {
                            return;
                        };
                        if try_borrow_plan_slot(budget) {
                            let _slot = ReturnAtomicPermit(budget);
                            if let Some(used) = opts.helpers_used {
                                used.fetch_add(
                                    1,
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                            }
                            if let Some(barrier) = opts.join_barrier {
                                barrier.wait();
                            }
                            work();
                            return;
                        }
                        std::thread::sleep(
                            std::time::Duration::from_millis(1),
                        );
                    }
                });
            }
            if let Some(barrier) = opts.join_barrier {
                barrier.wait();
            }
            work();
            stop.store(true, std::sync::atomic::Ordering::Release);
        });
    }
    // ---- merge: shard slots (prefix first, then task order),
    // pages in segment order, seq over the final run
    let mut arenas: Vec<Arena> = Vec::new();
    let prefix_slot = if retained.iter().any(|&r| r) {
        arenas.push(prefix);
        Some(0u32)
    } else {
        None
    };
    let mut results: Vec<
        Option<(Vec<PageJob>, SplitStats, Arena)>,
    > = done.into_iter().map(|m| m.into_inner().unwrap()).collect();
    let mut task_slot: Vec<Option<u32>> = vec![None; results.len()];
    for (k, r) in results.iter_mut().enumerate() {
        let (_, st, arena) = r.as_mut().expect("p2 task result");
        stats.fragments += st.fragments;
        stats.oversize_pages += st.oversize_pages;
        stats.depth_capped += st.depth_capped;
        stats.lod_pages += st.lod_pages;
        stats.lod_grid_verbatim += st.lod_grid_verbatim;
        if !arena.is_empty() {
            task_slot[k] = Some(narrow_u32(
                arenas.len() as u64,
                "cell arena count",
            ));
            arenas.push(std::mem::take(arena));
        }
    }
    let mut run: Vec<PageJob> = Vec::new();
    for s in segs {
        match s {
            Seg::Pages(mut p) => {
                let sl = prefix_slot.unwrap_or(ARENA_NONE);
                for j in p.iter_mut() {
                    j.arena_slot = sl;
                }
                run.append(&mut p);
            }
            Seg::Task(k) => {
                let (mut out, ..) =
                    results[k].take().expect("p2 seg result");
                let sl = task_slot[k].unwrap_or(ARENA_NONE);
                for j in out.iter_mut() {
                    j.arena_slot = sl;
                }
                run.append(&mut out);
            }
        }
    }
    for (i, j) in run.iter_mut().enumerate() {
        j.seq = narrow_u32(i as u64, "page seq");
    }
    if reserved {
        SHARD_RESERVED.fetch_sub(
            copy_bytes,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    finish_layer(
        li, run, arenas, stats, p2_tasks, shard_bytes, false, t,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_cell_plan(
    doc: &Doc,
    ci: usize,
    rbb: &[Option<(i64, i64, i64, i64)>],
    lidx: &std::collections::HashMap<(u32, u32), usize>,
    nl: usize,
    page_target_bytes: u64,
    lod: bool,
    plan_budget: &std::sync::atomic::AtomicIsize,
    late_waiters: Option<&std::sync::atomic::AtomicIsize>,
    late_join_barrier: Option<&std::sync::Barrier>,
    jobs: usize,
    p2_shard_limit: Option<u64>,
) -> CellPlan {
    let cell = &doc.cells[ci];
    let mut phase_s = [0f32; 6];
    let mut tmark = std::time::Instant::now();
    let mut lap = |slot: &mut f32| {
        let now = std::time::Instant::now();
        *slot += (now - tmark).as_secs_f32();
        tmark = now;
    };
    // ---- placements + instance BVH (leaf order = emit order)
    let sat32 = |v: i64| u32::try_from(v.max(0)).unwrap_or(u32::MAX);
    let mut items: Vec<(BBox, u32, u32, usize)> = cell
        .places
        .iter()
        .enumerate()
        .map(|(pi, pl)| {
            let cb = win_to_bbox(rbb[pl.cell]);
            // v7 node annotations: child rbbox max/min side (the
            // rotation-invariant pair the cut/hairline test uses)
            let cw = (cb.x1 - cb.x0).max(0);
            let ch = (cb.y1 - cb.y0).max(0);
            let (md, mn) =
                (sat32(cw.max(ch)), sat32(cw.min(ch)));
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
            (bb, md, mn, pi)
        })
        .collect();
    let (place_order, bvh) = if items.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        let mut nodes: Vec<(BBox, u32, u16, bool, u32, u32)> =
            Vec::new();
        nodes.push((BBox::EMPTY, 0, 0, false, 0, 0));
        split_bvh(&mut nodes, &mut items, 0, 0);
        let place_order =
            items.iter().map(|&(.., pi)| pi as u32).collect();
        (place_order, nodes)
    };
    lap(&mut phase_s[0]);
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
    lap(&mut phase_s[1]);
    // ---- per-layer page split (#60 P1): every non-empty layer is
    // an independent task with its own arena and run-local output
    let layer_inputs: Vec<(u32, Vec<PRec>)> = per_layer
        .into_iter()
        .enumerate()
        .filter(|(_, r)| !r.is_empty())
        .map(|(li, r)| (li as u32, r))
        .collect();
    let layers_active = layer_inputs.len();
    // ---- cell-level split mode (#60): dominant layer -> P2
    // subtree fanout, balanced layers -> P1 layer fanout, else
    // serial. ONE mode per cell: nesting would starve P2, because
    // P1 returns its helpers only after ALL layers finish - a
    // dominant layer's P2 would always see budget 0.
    //
    // P1/P2 and LOD share one runnable-task budget. Persistent cell
    // planners lend their slot while blocked beyond the ordered plan
    // window (#76), so a mid-build monster can use otherwise-idle CPUs
    // without exceeding --jobs. Fanout still stays off under governor
    // memory pressure because it multiplies live recursion temporaries.
    #[derive(PartialEq, Clone, Copy)]
    enum SplitMode {
        Serial,
        P1,
        P2,
    }
    let per_bytes: Vec<u64> = layer_inputs
        .iter()
        .map(|(_, r)| r.iter().map(|x| x.bytes as u64).sum())
        .collect();
    let split_bytes: u64 = per_bytes.iter().sum();
    let (max_k, max_bytes) = per_bytes
        .iter()
        .enumerate()
        .max_by_key(|&(_, &b)| b)
        .map(|(k, &b)| (k, b))
        .unwrap_or((0, 0));
    let mem_ok = crate::mem_available_gb()
        .is_none_or(|gb| gb >= GOVERNOR_MIN_AVAIL_GB);
    let mode = if !mem_ok {
        SplitMode::Serial
    } else if max_bytes >= P2_MIN_BYTES
        && max_bytes * 10 >= split_bytes * 6
    {
        // >= 60% of the split input in one layer: P1 cannot beat
        // that layer's serial time - go inside it
        SplitMode::P2
    } else if layers_active >= 2
        && split_bytes >= SPLIT_PAR_MIN_BYTES
    {
        SplitMode::P1
    } else {
        SplitMode::Serial
    };
    // `--jobs` is the scheduler contract. Re-reading
    // available_parallelism here could silently collapse explicitly requested
    // fanout to one under a container/affinity view even though the outer pool
    // already created `jobs` planners.
    let desired = jobs
        .max(1)
        .min(if mode == SplitMode::P1 {
            layers_active
        } else {
            usize::MAX
        })
        .min(SPLIT_PAR_MAX)
        .max(1);
    let mut split_helpers = 0usize;
    if mode != SplitMode::Serial && desired > 1 {
        while split_helpers + 1 < desired
            && try_borrow_plan_slot(plan_budget)
        {
            split_helpers += 1;
        }
    }
    let p2_helpers_used = std::sync::atomic::AtomicUsize::new(0);
    let layer_plans: Vec<LayerPlan> = if mode == SplitMode::Serial
        || desired <= 1
        || (mode == SplitMode::P1 && split_helpers == 0)
    {
        // operational fallback: without helpers neither fanout
        // pays. P2 with jobs>1 is different: it forms a frontier
        // even if no slot exists yet so late-lent slots can join.
        layer_inputs
            .into_iter()
            .map(|(li, recs)| {
                plan_layer(cell, ci, li, recs, page_target_bytes)
            })
            .collect()
    } else if mode == SplitMode::P2 {
        // the dominant layer fans its subtrees over all borrowed
        // helpers; the other layers are small by the dominance
        // test and run serially on this thread. The shard-copy
        // ceiling is evaluated inside the frontier at decision
        // time (see shard_headroom), not here.
        let opts = P2Opts {
            threads: desired,
            target_tasks: (desired * 4)
                .min(P2_TASK_CAP),
            task_min: P2_TASK_MIN_BYTES,
            shard_limit: p2_shard_limit,
            budget: Some(plan_budget),
            lease: split_helpers,
            late_waiters,
            helpers_used: Some(&p2_helpers_used),
            join_barrier: late_join_barrier,
        };
        layer_inputs
            .into_iter()
            .enumerate()
            .map(|(k, (li, recs))| {
                if k == max_k {
                    plan_layer_frontier(
                        cell,
                        ci,
                        li,
                        recs,
                        page_target_bytes,
                        &opts,
                    )
                } else {
                    plan_layer(
                        cell,
                        ci,
                        li,
                        recs,
                        page_target_bytes,
                    )
                }
            })
            .collect()
    } else {
        // P1: heaviest-first scheduling, li-order merge - load
        // balance freely, the merge order alone fixes the bytes
        let mut order: Vec<usize> =
            (0..layer_inputs.len()).collect();
        order.sort_by_key(|&k| std::cmp::Reverse(per_bytes[k]));
        let tasks: Vec<std::sync::Mutex<Option<(u32, Vec<PRec>)>>> =
            layer_inputs
                .into_iter()
                .map(|t| std::sync::Mutex::new(Some(t)))
                .collect();
        let slots: Vec<std::sync::Mutex<Option<LayerPlan>>> =
            (0..tasks.len())
                .map(|_| std::sync::Mutex::new(None))
                .collect();
        let nextk = std::sync::atomic::AtomicUsize::new(0);
        let work = || loop {
            let i = nextk
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if i >= order.len() {
                return;
            }
            let k = order[i];
            let (li, recs) =
                tasks[k].lock().unwrap().take().expect("layer task");
            let lp =
                plan_layer(cell, ci, li, recs, page_target_bytes);
            *slots[k].lock().unwrap() = Some(lp);
        };
        std::thread::scope(|sc| {
            for _ in 0..split_helpers {
                sc.spawn(&work);
            }
            work();
        });
        slots
            .into_iter()
            .map(|s| {
                s.into_inner().unwrap().expect("layer plan slot")
            })
            .collect()
    };
    // the memory fallback returns the lease inside the frontier
    // (it runs single-threaded) - do not return it twice
    let p2_fallback = layer_plans.iter().any(|lp| lp.p2_fallback);
    if split_helpers > 0 && !p2_fallback {
        plan_budget.fetch_add(
            split_helpers as isize,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    let split_threads = if p2_fallback {
        1
    } else if mode == SplitMode::P2 {
        1 + p2_helpers_used.load(
            std::sync::atomic::Ordering::Relaxed,
        )
    } else {
        split_helpers + 1
    };
    // #76b: count a P2 cell only if neither an entry-time lease nor a slot
    // lent while useful frontier work remained actually joined it. jobs=1
    // stays out because no pool could help there.
    let p2_starved = mode == SplitMode::P2
        && split_threads == 1
        && jobs > 1
        && desired > 1;
    // merge in li order (this is what fixes the output bytes)
    let mut pages: Vec<PageJob> = Vec::new();
    let mut pranges: Vec<(u32, u32, u32, u32)> = Vec::new();
    let mut pbvh: Vec<(BBox, u32, u16, bool, u64, u64)> = Vec::new();
    let mut arenas: Vec<Arena> = Vec::new();
    let mut split_stats = SplitStats::default();
    let mut top_split: Vec<((u32, u32), f32)> = Vec::new();
    let (mut p2_tasks, mut shard_bytes) = (0usize, 0u64);
    for mut lp in layer_plans {
        let run_lo = narrow_u32(pages.len() as u64, "cell page count");
        let run_count =
            narrow_u32(lp.pages.len() as u64, "run page count");
        let root = if lp.pbvh.is_empty() {
            PBVH_NONE
        } else {
            let root_local =
                narrow_u32(pbvh.len() as u64, "cell pbvh count");
            for (bb, first, count, leaf, mw, mh) in lp.pbvh {
                let f = if leaf {
                    run_lo + first
                } else {
                    root_local + first
                };
                pbvh.push((bb, f, count, leaf, mw, mh));
            }
            root_local
        };
        if !lp.arenas.is_empty() {
            // layer-local shard slots -> cell-local (compact:
            // LayerPlan carries only non-empty shards)
            let base =
                narrow_u32(arenas.len() as u64, "cell arena count");
            if base > 0 {
                for j in lp.pages.iter_mut() {
                    if j.arena_slot != ARENA_NONE {
                        j.arena_slot += base;
                    }
                }
            }
            arenas.extend(lp.arenas);
            narrow_u32(arenas.len() as u64, "cell arena count");
        }
        pranges.push((lp.li, run_lo, run_count, root));
        pages.append(&mut lp.pages);
        split_stats.fragments += lp.stats.fragments;
        split_stats.oversize_pages += lp.stats.oversize_pages;
        split_stats.depth_capped += lp.stats.depth_capped;
        split_stats.lod_pages += lp.stats.lod_pages;
        split_stats.lod_grid_verbatim += lp.stats.lod_grid_verbatim;
        p2_tasks += lp.p2_tasks;
        shard_bytes += lp.shard_bytes;
        top_split.push((doc.layer_order[lp.li as usize], lp.secs));
    }
    top_split.sort_by(|a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
    });
    top_split.truncate(3);
    lap(&mut phase_s[2]);
    // M7: LOD variants ride at the tail of the cell's page list -
    // pranges/pbvh reference only the exact runs before them, and
    // the exact->LOD link is rebased to global indices by the
    // append loop
    let n_exact = pages.len();
    // --no-lod (#61): an empty candidate list skips generation
    // entirely - every page keeps LOD_PAGE_NONE and the planner's
    // density gate simply never fires
    let cand: Vec<usize> = if lod {
        (0..n_exact)
            .filter(|&k| pages[k].members >= LOD_MIN_MEMBERS)
            .collect()
    } else {
        Vec::new()
    };
    // #60 monster cells: LOD variants are per-page independent, and
    // a fill cell can own thousands of dense pages (150M field: one
    // ESD dummy = 8821 pages; bench: lod was half the 164s plan).
    // Fan the generation out when the page count justifies threads;
    // results merge in page order, so bytes are unchanged.
    const LOD_PAR_MIN: usize = 64;
    let mut lod_threads = 1usize;
    if cand.len() >= LOD_PAR_MIN {
        // Threads beyond this planner's own come from the shared
        // runnable-task budget. Cell planners blocked on the ordered
        // plan window lend their slots, so this fanout also works for
        // monsters in the middle of a large build (#76).
        let desired = jobs
            .max(1)
            .min(cand.len())
            .min(16)
            .max(1);
        let mut extra = 0usize;
        while extra + 1 < desired
            && try_borrow_plan_slot(plan_budget)
        {
            extra += 1;
        }
        let late = reserve_late_waiters(
            late_waiters,
            desired.saturating_sub(extra + 1),
        );
        let slots: Vec<
            std::sync::Mutex<Option<(PageJob, SplitStats)>>,
        > = (0..cand.len())
            .map(|_| std::sync::Mutex::new(None))
            .collect();
        let nextk = std::sync::atomic::AtomicUsize::new(0);
        {
            // the planner thread itself always participates: the
            // slot-merge order (cand index) fixes output bytes at
            // any thread count
            let work = || loop {
                let i = nextk.fetch_add(
                    1,
                    std::sync::atomic::Ordering::Relaxed,
                );
                if i >= cand.len() {
                    return;
                }
                let mut lst = SplitStats::default();
                if let Some(job) = gen_lod_job(
                    doc,
                    cell,
                    arena_at(&arenas, pages[cand[i]].arena_slot),
                    &pages[cand[i]],
                    &mut lst,
                ) {
                    *slots[i].lock().unwrap() = Some((job, lst));
                }
            };
            if extra == 0 && late == 0 {
                work();
            } else {
                let helpers_used =
                    std::sync::atomic::AtomicUsize::new(0);
                let stop = std::sync::atomic::AtomicBool::new(false);
                std::thread::scope(|sc| {
                    struct StopOnDrop<'a>(
                        &'a std::sync::atomic::AtomicBool,
                    );
                    impl Drop for StopOnDrop<'_> {
                        fn drop(&mut self) {
                            self.0.store(
                                true,
                                std::sync::atomic::Ordering::Release,
                            );
                        }
                    }
                    let _stop_on_drop = StopOnDrop(&stop);
                    for _ in 0..extra {
                        sc.spawn(|| {
                            helpers_used.fetch_add(
                                1,
                                std::sync::atomic::Ordering::Relaxed,
                            );
                            work();
                        });
                    }
                    for _ in 0..late {
                        sc.spawn(|| {
                            let _waiter = ReturnAtomicPermit(
                                late_waiters
                                    .expect("late waiter budget"),
                            );
                            loop {
                                if stop.load(
                                    std::sync::atomic::Ordering::Acquire,
                                ) || nextk.load(
                                    std::sync::atomic::Ordering::Relaxed,
                                ) >= cand.len()
                                {
                                    return;
                                }
                                if try_borrow_plan_slot(plan_budget) {
                                    let _slot =
                                        ReturnAtomicPermit(plan_budget);
                                    helpers_used.fetch_add(
                                        1,
                                        std::sync::atomic::Ordering::Relaxed,
                                    );
                                    if let Some(barrier) =
                                        late_join_barrier
                                    {
                                        barrier.wait();
                                    }
                                    work();
                                    return;
                                }
                                std::thread::sleep(
                                    std::time::Duration::from_millis(1),
                                );
                            }
                        });
                    }
                    if let Some(barrier) = late_join_barrier {
                        barrier.wait();
                    }
                    work();
                    stop.store(
                        true,
                        std::sync::atomic::Ordering::Release,
                    );
                });
                lod_threads = 1 + helpers_used.load(
                    std::sync::atomic::Ordering::Relaxed,
                );
                plan_budget.fetch_add(
                    extra as isize,
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
        }
        for (i, &k) in cand.iter().enumerate() {
            if let Some((job, lst)) =
                slots[i].lock().unwrap().take()
            {
                split_stats.fragments += lst.fragments;
                split_stats.oversize_pages += lst.oversize_pages;
                split_stats.depth_capped += lst.depth_capped;
                split_stats.lod_pages += lst.lod_pages;
                split_stats.lod_grid_verbatim +=
                    lst.lod_grid_verbatim;
                pages[k].lod_page = narrow_u32(
                    pages.len() as u64,
                    "cell page count",
                );
                split_stats.lod_pages += 1;
                pages.push(job);
            }
        }
    } else {
        for &k in &cand {
            if let Some(job) = gen_lod_job(
                doc,
                cell,
                arena_at(&arenas, pages[k].arena_slot),
                &pages[k],
                &mut split_stats,
            ) {
                pages[k].lod_page = narrow_u32(
                    pages.len() as u64,
                    "cell page count",
                );
                split_stats.lod_pages += 1;
                pages.push(job);
            }
        }
    }
    lap(&mut phase_s[3]);
    let pts_prep: Vec<Option<floe_ovm::PtsPrepared>> = cell
        .places
        .iter()
        .map(|pl| match &pl.rep {
            Rep::Pts(p) => Some(floe_ovm::prepare_pts(p)),
            _ => None,
        })
        .collect();
    lap(&mut phase_s[4]);
    // ---- pre-encode the Builder sections with LOCAL indexes (the
    // serial pipeline commit used to re-walk every record here on
    // one thread; now it just memcpys and rebases)
    let mut sink = floe_ovm::CellSink::default();
    for &pi in &place_order {
        let pl = &cell.places[pi as usize];
        match &pts_prep[pi as usize] {
            Some(prep) => {
                sink.place_pts(
                    pl.cell as u32,
                    pl.x,
                    pl.y,
                    pl.rot,
                    pl.flip,
                    prep,
                );
            }
            None => {
                sink.place(
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
    for &(bb, first, count, leaf, md, mn) in &bvh {
        sink.bvh_node(&bb, first, count, leaf, md, mn);
    }
    for &(bb, first, count, leaf, mw, mh) in &pbvh {
        sink.pbvh_node(&bb, first, count, leaf, mw, mh);
    }
    for &(li, lo, cnt, root) in &pranges {
        sink.prange(li, lo, cnt, root);
    }
    lap(&mut phase_s[5]);
    CellPlan {
        phase_s,
        sink,
        place_order,
        bvh,
        pages,
        pranges,
        pbvh,
        arenas: std::sync::Arc::new(arenas),
        split_stats,
        split_threads,
        lod_threads,
        layers_active,
        p2_tasks,
        shard_bytes,
        p2_fallback,
        p2_starved,
        top_split,
    }
}

/// v5 text-index build tallies (heartbeat summary + meta.json)
#[derive(Default)]
struct TextIndexStats {
    records: u64,
    members: u64,
    cells: u64,
    grid_reps: u64,
    pts_reps: u64,
    string_bytes: u64,
    pts_bytes: u64,
}

/// local text-BVH split (median partition, same shape as
/// split_pbvh): nodes hold RUN-LOCAL first for leaves; the caller
/// rebases onto the emitted text order, which FOLLOWS the final
/// item order here - leaves are contiguous by construction
fn split_tbvh(
    nodes: &mut Vec<(BBox, u32, u16, bool)>,
    items: &mut [(BBox, u32)],
    lo: usize,
    slot: usize,
) {
    let mut bb = BBox::EMPTY;
    for (ib, _) in items.iter() {
        bb.grow(ib);
    }
    if items.len() <= TBVH_LEAF {
        nodes[slot] = (bb, lo as u32, items.len() as u16, true);
        return;
    }
    let wx = bb.x1 - bb.x0;
    let wy = bb.y1 - bb.y0;
    let mid = items.len() / 2;
    if wx >= wy {
        items.select_nth_unstable_by_key(mid, |(b, _)| b.x0 + b.x1);
    } else {
        items.select_nth_unstable_by_key(mid, |(b, _)| b.y0 + b.y1);
    }
    let l = nodes.len();
    nodes.push((BBox::EMPTY, 0, 0, false));
    nodes.push((BBox::EMPTY, 0, 0, false));
    nodes[slot] = (bb, l as u32, 2, false);
    let (a, c) = items.split_at_mut(mid);
    split_tbvh(nodes, a, lo, l);
    split_tbvh(nodes, c, lo + mid, l + 1);
}

/// cell-local text index of ONE cell (VFS_TEXT_PLAN.md par.3):
/// only doc.cells[ci].texts is read - no hierarchy walk, so build
/// cost/memory scale with SOURCE text records, never with path
/// expansion. Strings and Morton-ordered pts pools stream to the
/// design.ovt writer; fixed records go to the ovm builder. Runs
/// are per (cell, layer_idx); in-run order is a Morton pre-sort
/// (source_seq tie-break) refined by the deterministic BVH median
/// partition, so jobs count never changes a byte. Returns the
/// cell's (trange_start, trange_count).
fn build_cell_texts(
    doc: &Doc,
    ci: usize,
    lidx: &std::collections::HashMap<(u32, u32), usize>,
    b: &mut Builder,
    ovt: &mut std::io::BufWriter<std::fs::File>,
    ovt_off: &mut u64,
    st: &mut TextIndexStats,
) -> (u32, u32) {
    let cell = &doc.cells[ci];
    let tr_start = b.n_tranges();
    if cell.texts.is_empty() {
        return (tr_start, 0);
    }
    st.cells = st
        .cells
        .checked_add(1)
        .expect("limit exceeded: text-bearing cells");
    let mut groups: std::collections::BTreeMap<u32, Vec<u32>> =
        std::collections::BTreeMap::new();
    for (ti, t) in cell.texts.iter().enumerate() {
        let li = narrow_u32(
            lidx[&(t.layer, t.dt)] as u64,
            "text layer index",
        );
        groups
            .entry(li)
            .or_default()
            .push(narrow_u32(ti as u64, "cell text count"));
    }
    struct TRec {
        bbox: BBox,
        rep: u32,
        soff: u64,
        slen: u32,
        seq: u32,
        x: i64,
        y: i64,
    }
    for (li, idxs) in groups {
        // encode strings/reps in SOURCE order (that is the ovt
        // byte layout), then order the run records spatially
        let mut recs: Vec<TRec> = Vec::with_capacity(idxs.len());
        for &ti in &idxs {
            let t = &cell.texts[ti as usize];
            st.records = st
                .records
                .checked_add(1)
                .expect("limit exceeded: text records");
            st.members = st
                .members
                .checked_add(t.rep.members())
                .expect("limit exceeded: text members");
            let soff = *ovt_off;
            ovt.write_all(t.s.as_bytes()).expect("write ovt");
            *ovt_off = ovt_off
                .checked_add(t.s.len() as u64)
                .expect("limit exceeded: design.ovt bytes");
            st.string_bytes = st
                .string_bytes
                .checked_add(t.s.len() as u64)
                .expect("limit exceeded: text string bytes");
            let slen =
                narrow_u32(t.s.len() as u64, "text string length");
            let rep = match &t.rep {
                Rep::One => floe_ovm::TREP_NONE,
                Rep::Grid { na, nb, va, vb } => {
                    st.grid_reps = st
                        .grid_reps
                        .checked_add(1)
                        .expect("limit exceeded: text grid reps");
                    b.trep_grid(
                        narrow_u32(*na, "text rep na"),
                        narrow_u32(*nb, "text rep nb"),
                        *va,
                        *vb,
                    )
                }
                Rep::Pts(p) => {
                    st.pts_reps = st
                        .pts_reps
                        .checked_add(1)
                        .expect("limit exceeded: text pts reps");
                    let prep = floe_ovm::prepare_pts(p);
                    let pts_off = *ovt_off;
                    for &(px, py) in &prep.pts {
                        ovt.write_all(&px.to_le_bytes())
                            .expect("write ovt");
                        ovt.write_all(&py.to_le_bytes())
                            .expect("write ovt");
                    }
                    let pts_bytes = (prep.pts.len() as u64)
                        .checked_mul(16)
                        .expect("limit exceeded: text pts bytes");
                    *ovt_off = ovt_off
                        .checked_add(pts_bytes)
                        .expect("limit exceeded: design.ovt bytes");
                    st.pts_bytes = st
                        .pts_bytes
                        .checked_add(pts_bytes)
                        .expect("limit exceeded: text pts bytes");
                    let chunk_lo = b.n_tchunks();
                    for c in &prep.chunks {
                        b.tchunk(c);
                    }
                    b.trep_pts(
                        narrow_u32(
                            prep.pts.len() as u64,
                            "text pts count",
                        ),
                        pts_off,
                        chunk_lo,
                        narrow_u32(
                            prep.chunks.len() as u64,
                            "text pts chunks",
                        ),
                    )
                }
            };
            let (ex, ey) = rep_extent(&t.rep);
            let add = |a: i64, b: i64, field: &str| {
                a.checked_add(b)
                    .unwrap_or_else(|| panic!("limit exceeded: {}", field))
            };
            recs.push(TRec {
                bbox: BBox {
                    x0: add(t.x, ex.0.min(0), "text bbox x0"),
                    y0: add(t.y, ey.0.min(0), "text bbox y0"),
                    x1: add(t.x, ex.1.max(0), "text bbox x1"),
                    y1: add(t.y, ey.1.max(0), "text bbox y1"),
                },
                rep,
                soff,
                slen,
                seq: ti,
                x: t.x,
                y: t.y,
            });
        }
        // Morton pre-sort over bbox centers, source_seq tie-break
        let center = |r: &TRec| {
            (
                ((r.bbox.x0 as i128 + r.bbox.x1 as i128) / 2)
                    as i64,
                ((r.bbox.y0 as i128 + r.bbox.y1 as i128) / 2)
                    as i64,
            )
        };
        let (mut minx, mut miny) = (i64::MAX, i64::MAX);
        for r in &recs {
            let (cx, cy) = center(r);
            minx = minx.min(cx);
            miny = miny.min(cy);
        }
        let mut order: Vec<u32> = (0..recs.len() as u32).collect();
        order.sort_by_key(|&i| {
            let (cx, cy) = center(&recs[i as usize]);
            (
                floe_ovm::morton_key(cx, cy, minx, miny),
                recs[i as usize].seq,
            )
        });
        let text_lo = b.n_texts();
        let count =
            narrow_u32(recs.len() as u64, "trange text count");
        let root = if recs.len() <= TBVH_LEAF {
            for &i in &order {
                let r = &recs[i as usize];
                b.text(
                    narrow_u32(ci as u64, "cell index"),
                    li,
                    r.x,
                    r.y,
                    r.soff,
                    r.slen,
                    r.rep,
                    &r.bbox,
                    r.seq,
                );
            }
            floe_ovm::TBVH_NONE
        } else {
            // BVH partition decides the storage order (leaves are
            // contiguous item ranges); emit texts in that order
            let mut items: Vec<(BBox, u32)> = order
                .iter()
                .map(|&i| (recs[i as usize].bbox, i))
                .collect();
            let mut nodes: Vec<(BBox, u32, u16, bool)> =
                vec![(BBox::EMPTY, 0, 0, false)];
            split_tbvh(&mut nodes, &mut items, 0, 0);
            for &(_, i) in &items {
                let r = &recs[i as usize];
                b.text(
                    narrow_u32(ci as u64, "cell index"),
                    li,
                    r.x,
                    r.y,
                    r.soff,
                    r.slen,
                    r.rep,
                    &r.bbox,
                    r.seq,
                );
            }
            let base = b.n_tbvh();
            for &(bbb, first, cnt, leaf) in &nodes {
                let f = if leaf {
                    narrow_u32(
                        text_lo as u64 + first as u64,
                        "text index",
                    )
                } else {
                    narrow_u32(
                        base as u64 + first as u64,
                        "tbvh index",
                    )
                };
                b.tbvh_node(&bbb, f, cnt, leaf);
            }
            base
        };
        b.trange(li, text_lo, count, root);
    }
    (tr_start, b.n_tranges() - tr_start)
}

#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
fn build(
    doc: &Doc,
    size: u64,
    mtime: u64,
    outdir: &str,
    jobs: usize,
    encode_batch: usize,
    plan_batch: usize,
    page_target_bytes: u64,
    lod: bool,
    slow_cell_s: f64,
    p2_shard_limit: Option<u64>,
) -> (
    Vec<u8>,
    Vec<Option<(i64, i64, i64, i64)>>,
    Vec<u64>,
    TextIndexStats,
) {
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
    let dslots: Vec<
        std::sync::OnceLock<(Vec<u8>, Vec<u8>, u64, BBox)>,
    > = (0..n).map(|_| std::sync::OnceLock::new()).collect();
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
                            let mut tmask = vec![0u8; bw];
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
                            let _ = dslots[ci]
                                .set((mask, tmask, mems, bx));
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
    for slot in dslots {
        let (m, tm, mm, bx) =
            slot.into_inner().expect("direct slot unset");
        dmask.push(m);
        dtmask.push(tm);
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
    let mut rtmask: Vec<Vec<u8>> = vec![Vec::new(); n];
    for &ci in &topo {
        let mut hm = 0u32;
        let mut mm = dmembers[ci];
        let mut mask = dmask[ci].clone();
        let mut tmask = dtmask[ci].clone();
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
            for (a, b) in tmask.iter_mut().zip(&rtmask[c]) {
                *a |= *b;
            }
        }
        height[ci] = hm;
        rmembers[ci] = mm;
        rmask[ci] = mask;
        rtmask[ci] = tmask;
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
    // design.ovt (v5): text strings + pts pools stream here - the
    // text pass runs serially in cell order inside the batch loop
    // (cheap, source-local), so bytes are jobs-independent
    let ovt_path = format!("{}/design.ovt", outdir);
    let mut ovt = std::io::BufWriter::new(
        std::fs::File::create(&ovt_path).expect("create ovt"),
    );
    let mut ovt_off = 0u64;
    let mut tstats = TextIndexStats::default();
    let mut pages_bytes = 0u64;
    let mut pages_total = 0usize;
    let mut lrecs_stored = vec![0u64; nl];
    let mut split_total = SplitStats::default();
    let mut p2_starved_cells = 0usize;
    let planned_cells = std::sync::Arc::new(
        std::sync::atomic::AtomicUsize::new(0),
    );
    let planned_pages = std::sync::Arc::new(
        std::sync::atomic::AtomicUsize::new(0),
    );
    let encoded_pages = std::sync::Arc::new(
        std::sync::atomic::AtomicUsize::new(0),
    );
    let active_cell_plans = std::sync::Arc::new(
        std::sync::atomic::AtomicUsize::new(0),
    );
    let peak_cell_plans = std::sync::Arc::new(
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
        let active_cell_plans = active_cell_plans.clone();
        let peak_cell_plans = peak_cell_plans.clone();
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
                         planned={} encoded={} active_cells={} \
                         peak={} ({}s, rss {})",
                        planned_cells.load(Relaxed),
                        n,
                        planned_pages.load(Relaxed),
                        encoded_pages.load(Relaxed),
                        active_cell_plans.load(Relaxed),
                        peak_cell_plans.load(Relaxed),
                        elapsed,
                        rss()
                    );
                }
            }
        })
    };
    eprintln!(
        "[vfs] build: streaming pipeline {} cells ({} workers, \
         plan window {}, encode batch {}, page target {} MiB{})...",
        n,
        jobs.max(1),
        plan_batch,
        encode_batch.max(1),
        page_target_bytes / MIB,
        if lod { "" } else { ", lod off" }
    );
    // ---- streaming pipeline (rev 44): a persistent worker pool
    // plans cells inside a bounded in-flight window while THIS
    // thread commits them in cell order and encodes chunk by chunk.
    // The old per-batch scope joined every worker at each batch
    // boundary, so one monster cell (fill farms: millions of rep
    // fragments in ONE build_cell_plan call) idled the other
    // workers at the barrier - 150M field case: plan 155.5s while
    // append/encode sat at 2s. The committer is strictly ordered,
    // so output bytes are identical at every worker count and
    // window size.
    let mut commit_elapsed = std::time::Duration::ZERO;
    let mut encode_elapsed = std::time::Duration::ZERO;
    let t_pipeline = std::time::Instant::now();
    // window = max in-flight plans past the commit point (the RSS
    // knob the plan batch used to be); the governor halves it under
    // memory pressure, floor `jobs` (fewer starves the pool)
    let ring_cap = plan_batch.max(jobs.max(1));
    let window = std::sync::atomic::AtomicUsize::new(ring_cap);
    let next_cell = std::sync::atomic::AtomicUsize::new(0);
    let commit_base = std::sync::atomic::AtomicUsize::new(0);
    let ring: Vec<std::sync::Mutex<Option<CellPlan>>> =
        (0..ring_cap).map(|_| std::sync::Mutex::new(None)).collect();
    // a thread that panics leaves its ring slot empty forever and
    // the others polling it: this flag turns every wait loop into
    // an exit so the scope can join and propagate the panic
    // instead of hanging a multi-hour build
    let died = std::sync::atomic::AtomicBool::new(false);
    struct DiedFlag<'a>(&'a std::sync::atomic::AtomicBool);
    impl Drop for DiedFlag<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                self.0
                    .store(true, std::sync::atomic::Ordering::Release);
            }
        }
    }
    // Unified runnable planning budget (#76): persistent cell planners and
    // their P1/P2/LOD helpers share exactly `jobs` slots. A planner lends its
    // slot while blocked beyond the ordered plan window, then must reacquire
    // one before it can plan the claimed cell. This keeps active planning
    // work <= jobs while letting a mid-build monster consume CPUs that would
    // otherwise sit in the admission sleep. (The OS planner threads remain
    // parked; only runnable planning tasks are pooled.)
    let slow_s = slow_cell_s;
    let planners = jobs.max(1).min(n.max(1));
    let plan_budget = std::sync::atomic::AtomicIsize::new(
        jobs.max(1) as isize - planners as isize,
    );
    // Parked late helpers repair the one-shot #76 race: a P2/LOD phase that
    // starts at budget zero can consume slots lent later. They do not own a
    // CPU slot while parked, and this separate cap bounds extra OS threads.
    let late_waiters = std::sync::atomic::AtomicIsize::new(
        jobs.saturating_sub(1).min(16) as isize,
    );
    let window_slot_lends = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        // committer side: a panic below (commit/encode) must also
        // release workers stuck in the admission wait
        let _died_guard = DiedFlag(&died);
        for _ in 0..planners {
            let ring = &ring;
            let window = &window;
            let next_cell = &next_cell;
            let commit_base = &commit_base;
            let rbb = &rbb;
            let lidx = &lidx;
            let died = &died;
            let plan_budget = &plan_budget;
            let late_waiters = &late_waiters;
            let active_cell_plans = &active_cell_plans;
            let peak_cell_plans = &peak_cell_plans;
            let window_slot_lends = &window_slot_lends;
            s.spawn(move || loop {
                let ci = next_cell.fetch_add(
                    1,
                    std::sync::atomic::Ordering::Relaxed,
                );
                if ci >= n {
                    // This planner still owns its runnable slot here.
                    plan_budget.fetch_add(
                        1,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    return;
                }
                let admitted = || {
                    ci < commit_base
                        .load(std::sync::atomic::Ordering::Acquire)
                        + window
                            .load(
                                std::sync::atomic::Ordering::Relaxed,
                            )
                            .min(ring_cap)
                };
                if !admitted() {
                    // The claimed cell cannot enter the RSS-bounded window.
                    // Lend this planner's runnable slot to a P1/P2/LOD task;
                    // admission alone is not enough to resume because a
                    // helper may still own the slot.
                    plan_budget.fetch_add(
                        1,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    window_slot_lends.fetch_add(
                        1,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    loop {
                        if died.load(
                            std::sync::atomic::Ordering::Acquire,
                        ) {
                            // Slot was already returned before this wait.
                            return;
                        }
                        if admitted()
                            && try_borrow_plan_slot(plan_budget)
                        {
                            break;
                        }
                        std::thread::sleep(
                            std::time::Duration::from_millis(1),
                        );
                    }
                }
                let t = std::time::Instant::now();
                let active = active_cell_plans.fetch_add(
                    1,
                    std::sync::atomic::Ordering::Relaxed,
                ) + 1;
                peak_cell_plans.fetch_max(
                    active,
                    std::sync::atomic::Ordering::Relaxed,
                );
                struct ActiveCellGuard<'a>(
                    &'a std::sync::atomic::AtomicUsize,
                );
                impl Drop for ActiveCellGuard<'_> {
                    fn drop(&mut self) {
                        self.0.fetch_sub(
                            1,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
                }
                let _active_cell = ActiveCellGuard(active_cell_plans);
                let plan = match std::panic::catch_unwind(
                    std::panic::AssertUnwindSafe(|| {
                        build_cell_plan(
                            doc,
                            ci,
                            rbb,
                            lidx,
                            nl,
                            page_target_bytes,
                            lod,
                            plan_budget,
                            Some(late_waiters),
                            None,
                            jobs,
                            p2_shard_limit,
                        )
                    }),
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        died.store(
                            true,
                            std::sync::atomic::Ordering::Release,
                        );
                        std::panic::resume_unwind(e);
                    }
                };
                let secs = t.elapsed().as_secs_f64();
                if secs >= slow_s {
                    // straggler instrumentation: name the monster
                    // cells, their thread uptake (helpers exist
                    // only when the shared budget had free slots)
                    // and the per-layer split skew that decides
                    // whether P1 fanout can help (#60)
                    let mut top = String::new();
                    for ((l, d), s) in &plan.top_split {
                        top.push_str(&format!(
                            " L{}.{} {:.1}s",
                            l, d, s
                        ));
                    }
                    let p2 = if plan.p2_tasks > 0 {
                        format!(
                            " p2_tasks={} shard={}MiB{}{}",
                            plan.p2_tasks,
                            plan.shard_bytes / MIB,
                            if plan.p2_fallback {
                                " p2_mem_fallback=1"
                            } else {
                                ""
                            },
                            if plan.p2_starved {
                                " helpers=0"
                            } else {
                                ""
                            }
                        )
                    } else if plan.p2_starved {
                        String::from(" p2_eligible=1 helpers=0")
                    } else {
                        String::new()
                    };
                    eprintln!(
                        "[vfs] build: slow cell {} (ci {}/{}): \
                         plan {:.1}s (places {}, pages {}, frag \
                         splits {}; bvh {:.1} asm {:.1} split \
                         {:.1}/{}t{} lod {:.1}/{}t pts {:.1} sink \
                         {:.1}; layers {}, top{})",
                        doc.cells[ci].name,
                        ci,
                        n,
                        secs,
                        doc.cells[ci].places.len(),
                        plan.pages.len(),
                        plan.split_stats.fragments,
                        plan.phase_s[0],
                        plan.phase_s[1],
                        plan.phase_s[2],
                        plan.split_threads,
                        p2,
                        plan.phase_s[3],
                        plan.lod_threads,
                        plan.phase_s[4],
                        plan.phase_s[5],
                        plan.layers_active,
                        top
                    );
                }
                *ring[ci % ring_cap].lock().unwrap() = Some(plan);
            });
        }

        // ---- ordered committer (this thread): sink commit + texts
        // + cell records, then encode a chunk and drop its arenas
        let mut batch_jobs: Vec<PageJob> = Vec::new();
        let mut batch_arenas: Vec<std::sync::Arc<Vec<Arena>>> =
            Vec::new();
        let mut chunk_base = 0usize;
        for ci in 0..n {
            if ci % 64 == 0 {
                if let Some(avail) = crate::mem_available_gb() {
                    if avail < GOVERNOR_MIN_AVAIL_GB {
                        let cur = window.load(
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        let nextw = (cur / 2).max(jobs.max(1));
                        if nextw < cur {
                            eprintln!(
                                "[vfs] build: window governor {} \
                                 -> {} (MemAvailable {:.1} GB)",
                                cur, nextw, avail
                            );
                            window.store(
                                nextw,
                                std::sync::atomic::Ordering::Relaxed,
                            );
                        }
                    }
                }
            }
            let plan = loop {
                if let Some(pl) =
                    ring[ci % ring_cap].lock().unwrap().take()
                {
                    break pl;
                }
                if died.load(std::sync::atomic::Ordering::Acquire) {
                    // the owning worker crashed: its slot never
                    // fills - stop waiting so the scope joins and
                    // the original panic surfaces
                    panic!(
                        "build: a cell planner worker panicked \
                         (cell {} never arrived)",
                        ci
                    );
                }
                std::thread::sleep(
                    std::time::Duration::from_micros(200),
                );
            };
            commit_base.store(
                ci + 1,
                std::sync::atomic::Ordering::Release,
            );
            let ta = std::time::Instant::now();
            let CellPlan {
                sink,
                mut pages,
                arenas,
                split_stats,
                p2_starved,
                ..
            } = plan;
            batch_arenas.push(arenas);
            if p2_starved {
                p2_starved_cells += 1;
            }
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
            split_total.lod_grid_verbatim = split_total
                .lod_grid_verbatim
                .checked_add(split_stats.lod_grid_verbatim)
                .expect("limit exceeded: lod verbatim count");
            let cell = &doc.cells[ci];
            let page_base = pages_total as u64;
            let page_start = narrow_u32(page_base, "page count");
            let page_count =
                narrow_u32(pages.len() as u64, "cell page count");
            // one memcpy+rebase commit instead of the old
            // record-at-a-time re-walk (the pipeline's serial
            // bottleneck on placement-heavy chips)
            let bases = b.append_cell_sink(&sink, page_base);
            let place_base = bases.place_start;
            let (bvh_start, bvh_count) = if sink.n_bvh == 0 {
                (0, 0)
            } else {
                (bases.bvh_start, sink.n_bvh)
            };
            let pr_start = bases.prange_start;
            let pr_count = sink.n_pranges;
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

            let (tr_start, tr_count) = build_cell_texts(
                doc, ci, &lidx, &mut b, &mut ovt, &mut ovt_off,
                &mut tstats,
            );
            let mask_d = b.bitset(&dmask[ci]);
            let mask_r = b.bitset(&rmask[ci]);
            let mask_t = b.bitset(&rtmask[ci]);
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
                tr_start,
                tr_count,
                mask_t,
            );
            commit_elapsed += ta.elapsed();
            planned_cells.store(
                ci + 1,
                std::sync::atomic::Ordering::Relaxed,
            );
            planned_pages.store(
                pages_total,
                std::sync::atomic::Ordering::Relaxed,
            );
            let flush = ci + 1
                >= chunk_base
                    + window
                        .load(std::sync::atomic::Ordering::Relaxed)
                        .min(ring_cap)
                || ci + 1 == n;
            if flush {
                let te = std::time::Instant::now();
                encode_write_pages(
                    doc,
                    &batch_jobs,
                    &batch_arenas,
                    chunk_base,
                    encode_workers,
                    encode_batch,
                    &mut b,
                    &mut ovp,
                    &mut ovp_off,
                    &mut pages_bytes,
                    &encoded_pages,
                );
                encode_elapsed += te.elapsed();
                batch_jobs.clear();
                batch_arenas.clear();
                chunk_base = ci + 1;
                // this chunk's arenas and PtsPrepared drop here
            }
        }
    });
    debug_assert_eq!(
        plan_budget.load(std::sync::atomic::Ordering::Relaxed),
        jobs.max(1) as isize,
        "planning slot lease imbalance"
    );
    debug_assert_eq!(
        late_waiters.load(std::sync::atomic::Ordering::Relaxed),
        jobs.saturating_sub(1).min(16) as isize,
        "late helper waiter lease imbalance"
    );
    debug_assert_eq!(b.n_pages() as usize, pages_total);
    pipeline_on.store(false, std::sync::atomic::Ordering::Relaxed);
    heartbeat.join().expect("pipeline heartbeat");
    eprintln!(
        "[vfs] build: pipeline complete (wall {:.1}s, commit {:.1}s, \
         encode {:.1}s, cell-plan peak {}, rss {})",
        t_pipeline.elapsed().as_secs_f64(),
        commit_elapsed.as_secs_f64(),
        encode_elapsed.as_secs_f64(),
        peak_cell_plans.load(std::sync::atomic::Ordering::Relaxed),
        rss()
    );
    let lends =
        window_slot_lends.load(std::sync::atomic::Ordering::Relaxed);
    if lends > 0 {
        eprintln!(
            "[vfs] build: plan window lent {} idle slot(s) to the \
             shared P1/P2/LOD budget",
            lends
        );
    }
    if p2_starved_cells > 0 {
        // This means no runnable slot became available while useful frontier
        // work remained; the one-shot-at-entry race is covered by #76b.
        eprintln!(
            "[vfs] build: {} P2-eligible cell(s) ran serial (no \
             runnable slot joined before split completion)",
            p2_starved_cells
        );
    }

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
             pages, {} depth-capped, {} lod variants \
             ({} skew grids verbatim)",
            split_total.fragments,
            split_total.oversize_pages,
            split_total.depth_capped,
            split_total.lod_pages,
            split_total.lod_grid_verbatim
        );
    }

    ovp.flush().expect("flush ovp");
    ovt.flush().expect("flush ovt");
    eprintln!(
        "[vfs] build: text index {} records ({} members) in {} \
         cells, {} grid / {} pts reps, strings {:.1} MB + pts \
         {:.1} MB -> design.ovt (rss {})",
        tstats.records,
        tstats.members,
        tstats.cells,
        tstats.grid_reps,
        tstats.pts_reps,
        tstats.string_bytes as f64 / 1e6,
        tstats.pts_bytes as f64 / 1e6,
        rss()
    );
    // the caller writes design.ovm LAST (commit marker, after the
    // viewer-side files); ovp_len/ovt_len ride in the header so
    // open can verify both cache pairs
    let ovm_bytes = b.finish(ovp_off, ovt_off);
    eprintln!(
        "[vfs] {} pages ({}) + ovm {} in {:.1}s",
        pages_total,
        fmt_size(pages_bytes),
        fmt_size(ovm_bytes.len() as u64),
        t0.elapsed().as_secs_f64()
    );
    (ovm_bytes, rbb, lmems, tstats)
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
    nodes: &mut Vec<(BBox, u32, u16, bool, u32, u32)>,
    items: &mut [(BBox, u32, u32, usize)],
    lo: usize,
    slot: usize,
) {
    let mut bb = BBox::EMPTY;
    let (mut md, mut mn) = (0u32, 0u32);
    for (ib, imd, imn, _) in items.iter() {
        bb.grow(ib);
        md = md.max(*imd);
        mn = mn.max(*imn);
    }
    if items.len() <= BVH_LEAF {
        nodes[slot] =
            (bb, lo as u32, items.len() as u16, true, md, mn);
        return;
    }
    let wx = bb.x1 - bb.x0;
    let wy = bb.y1 - bb.y0;
    let mid = items.len() / 2;
    if wx >= wy {
        items.select_nth_unstable_by_key(mid, |(b, ..)| {
            b.x0 + b.x1
        });
    } else {
        items.select_nth_unstable_by_key(mid, |(b, ..)| {
            b.y0 + b.y1
        });
    }
    let l = nodes.len();
    nodes.push((BBox::EMPTY, 0, 0, false, 0, 0));
    nodes.push((BBox::EMPTY, 0, 0, false, 0, 0));
    nodes[slot] = (bb, l as u32, 2, false, md, mn);
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
        px_per_dbu: px_per_um / s,
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
    let mut req = make_req(
        &v,
        view.expect("--view required"),
        px,
        cut,
        depth,
        layers.as_deref(),
    );
    // --lod 0 renders the plan exact (M7 kill-switch parity)
    if rest.iter().any(|(k, val)| k == "--lod" && val == "0") {
        req.px_per_dbu = 0.0;
    }
    // --labels raw|sel (T2 gate interface): dump the label
    // planner's rows instead of the geometry JSON. raw = exact
    // pre-declutter candidates (oracle XOR); sel = the budgeted
    // screen-space selection the viewer would show.
    if let Some((_, mode)) =
        rest.iter().find(|(k, _)| k == "--labels")
    {
        let mut o = floe_vfs::text::LabelOpts::default();
        if mode == "raw" {
            o.raw = true;
            o.cand_cap = usize::MAX;
            o.member_budget = u64::MAX;
        }
        let t0 = std::time::Instant::now();
        let lp = v
            .plan_labels_with(&req, &o)
            .unwrap_or_else(|e| {
                eprintln!("{}", e);
                std::process::exit(1);
            });
        for r in &lp.rows {
            if r.block {
                println!(
                    "blk\t-\t{}\t{}\t{}\t{}\t{}",
                    r.x,
                    r.y,
                    r.rot,
                    r.white as u8,
                    crate::tsv_esc(&r.s)
                );
            } else {
                println!(
                    "txt\t{}/{}\t{}\t{}\t{}",
                    r.layer,
                    r.dt,
                    r.x,
                    r.y,
                    crate::tsv_esc(&r.s)
                );
            }
        }
        eprintln!(
            "[plan] labels {} rows ({} candidate records, {} \
             members visible, truncated={}) in {:.2}ms",
            lp.rows.len(),
            lp.stats.records_candidate,
            lp.stats.members_visible,
            lp.stats.truncated,
            t0.elapsed().as_secs_f64() * 1e3
        );
        return;
    }
    {
        let t0 = std::time::Instant::now();
        // --wash-px N overrides the M7-C wash threshold (field
        // tuning; 0 disables the wash entirely)
        let mut popts = floe_vfs::hier::HierOpts::default();
        if let Some((_, val)) =
            rest.iter().find(|(k, _)| k == "--wash-px")
        {
            popts.wash_px = val.parse().expect("wash-px");
        }
        // --hairline-f F: min-side cut factor (0 disables; the
        // threshold is F * cut_dbu)
        if let Some((_, val)) =
            rest.iter().find(|(k, _)| k == "--hairline-f")
        {
            popts.hairline = val.parse().expect("hairline-f");
        }
        // --thin-um F: rev 45 thin-frame lattice pitch in um (0
        // restores the rev 41 hairline cull for frames)
        if let Some((_, val)) =
            rest.iter().find(|(k, _)| k == "--thin-um")
        {
            popts.thin_lattice_um = val.parse().expect("thin-um");
        }
        let plan = floe_vfs::hier::plan_hier(&v.ovm, &req, &popts);
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
             \"kbox_merges\": {},\n  \"lod_pages\": {},\n  \
             \"washed_pages\": {},\n  \
             \"culled_bvh_size\": {},\n  \
             \"thin_frames\": {},\n  \
             \"plan_ms\": {:.2}\n}}",
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
            st.lod_swapped,
            st.washed_pages,
            st.culled_bvh_size,
            st.thin_frames,
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
///   gen=1 view=x0,y0,x1,y1 px=5 cut=2 depth=full lod=1 frames=1 \
///     labels=1 [hair=0.5] [thin=7.0] \
///     layers=all|none|11/0,12/0 out=/tmp/dir
/// mode=frontier (rev 46, session-less): plan at the given
///   px/cut/depth and write the world-space frame boxes as
///   out/frontier_GEN.tsv (x0 y0 x1 y1 band per row, spatially
///   capped) - response `gen=N frontier=path boxes=N plan_ms=F`
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
    let mut nolabels = false;
    let mut lod = true;
    let mut frames = true;
    let mut labels = true;
    let mut hair = 0.5f64;
    let mut thin = 7.0f64;
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
                if val == "none" {
                    layers = Some(Vec::new());
                } else if val != "all" && !val.is_empty() {
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
            // refinement rounds of one view: labels were already
            // delivered with the first round, skip the re-plan
            "nolabels" => nolabels = val == "1",
            "lod" => {
                lod = match val {
                    "0" => false,
                    "1" => true,
                    _ => return Err("lod".into()),
                }
            }
            "frames" => {
                frames = match val {
                    "0" => false,
                    "1" => true,
                    _ => return Err("frames".into()),
                }
            }
            "labels" => {
                labels = match val {
                    "0" => false,
                    "1" => true,
                    _ => return Err("labels".into()),
                }
            }
            // rev 41 hairline factor (min-side cut = hair * cut);
            // optional, default 0.5, 0 disables
            "hair" => hair = val.parse().map_err(|_| "hair")?,
            // rev 45 thin-frame lattice pitch in um; optional,
            // default 7.0, 0 restores the rev 41 frame cull
            "thin" => thin = val.parse().map_err(|_| "thin")?,
            _ => return Err(format!("unknown key {}", k)),
        }
    }
    // M5: the daemon speaks V4 (hier) only; `mode=` survives
    // solely to mark session-less probes (pick/snap/clip)
    let probe = mode == "probe" || mode == "hier_probe";
    let view = view.ok_or("view required")?;
    let out = out.ok_or("out required")?;
    let mut req =
        make_req(d.v, view, px, cut, depth, layers.as_deref());
    // rev 46: session-less minimap frontier - plan at the CANVAS
    // fit scale (px kept: the cut/band/lattice ladder must match
    // the main view), expand the WS frame set to world boxes
    if mode == "frontier" {
        return serve_frontier(d, &req, gen, hair, thin, &out);
    }
    // measurement paths are EXACT by construction: probes never
    // take the LOD density gate. lod=0 disables the gate via
    // lod_k inside serve_hier - NOT by erasing the screen scale:
    // the scale also drives frame rep fusing and the rev 35 tone
    // split, and zeroing it painted every box white the moment
    // LOD was toggled off (field report). Labels ride their own
    // px for the same reason.
    let label_px = if probe { 0.0 } else { req.px_per_dbu };
    if probe {
        req.px_per_dbu = 0.0;
    }
    let label_px = if nolabels || !labels { 0.0 } else { label_px };
    serve_hier(d, &req, gen, ack, reset, probe, stream_kb,
               label_px, frames, lod, hair, thin, &out)
}

/// rev 46 minimap frontier: plan_hier at the requested depth/scale
/// (viewer knobs passed through), expand the frame set to world
/// boxes (floe_vfs::hier::frontier_boxes), write one TSV
/// `x0 y0 x1 y1 band` per row. Session-less like the probes.
fn serve_frontier(
    d: &mut Daemon,
    req: &floe_vfs::ViewReq,
    gen: u64,
    hair: f64,
    thin: f64,
    out: &str,
) -> Result<String, String> {
    let t0 = std::time::Instant::now();
    let mut opts = floe_vfs::hier::HierOpts::default();
    opts.hairline = hair;
    opts.thin_lattice_um = thin;
    let plan = floe_vfs::hier::plan_hier(&d.v.ovm, req, &opts);
    let (boxes, truncated) =
        floe_vfs::hier::frontier_boxes(&d.v.ovm, &plan, 6000);
    if truncated {
        eprintln!(
            "[vfsd] frontier: gen {} hit the 8M expansion budget",
            gen
        );
    }
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let p = format!("{}/frontier_{}.tsv", out, gen);
    let mut w = String::with_capacity(boxes.len() * 40);
    for (bx, band) in &boxes {
        use std::fmt::Write as _;
        let _ = writeln!(
            w,
            "{}\t{}\t{}\t{}\t{}",
            bx[0], bx[1], bx[2], bx[3], band
        );
    }
    std::fs::write(&p, w).map_err(|e| e.to_string())?;
    Ok(format!(
        "gen={} frontier={} boxes={} plan_ms={:.2}",
        gen,
        p,
        boxes.len(),
        t0.elapsed().as_secs_f64() * 1e3
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
    label_px: f64,
    frames: bool,
    lod: bool,
    hair: f64,
    thin: f64,
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
    let mut opts = floe_vfs::hier::HierOpts::default();
    opts.hairline = hair;
    opts.thin_lattice_um = thin;
    if !frames {
        opts.frame_cap = 0;
    }
    if probe || !lod {
        // documented kill switch: exact plan, screen scale intact
        opts.lod_k = 0.0;
        opts.wash_px = 0.0;
    }
    if probe {
        // probes measure/pick: never hairline-cut (or lattice-thin)
        // what a click may target
        opts.hairline = 0.0;
        opts.thin_lattice_um = 0.0;
    }
    let plan = floe_vfs::hier::plan_hier(&v.ovm, req, &opts);
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
    // request-scoped labels (v5, VFS_TEXT_PLAN.md par.5.2): a
    // small per-gen TSV, kind-explicit rows (txt = design layer,
    // blk = runtime annotation). View-generation asset: the client
    // applies it with the same gen's delta or drops both.
    let (labels_path, nlabels, text_ms, text_stats) = if label_px <= 0.0 {
        (
            "-".to_string(),
            0usize,
            0.0,
            floe_vfs::text::TextStats::default(),
        )
    } else {
        let tl = std::time::Instant::now();
        let mut lreq = req.clone();
        lreq.px_per_dbu = label_px;
        let lp = {
            let mut lopts = floe_vfs::text::LabelOpts::default();
            lopts.hairline = hair;
            if !frames {
                lopts.blocks = false;
            }
            v.plan_labels_with(&lreq, &lopts)?
        };
        let ms = tl.elapsed().as_secs_f64() * 1e3;
        if lp.rows.is_empty() {
            ("-".to_string(), 0, ms, lp.stats)
        } else {
            let mut wbuf = String::new();
            for r in &lp.rows {
                if r.block {
                    wbuf.push_str(&format!(
                        "blk\t-\t{}\t{}\t{}\t{}\t{}\n",
                        r.x,
                        r.y,
                        r.rot,
                        r.white as u8,
                        crate::tsv_esc(&r.s)
                    ));
                } else {
                    wbuf.push_str(&format!(
                        "txt\t{}/{}\t{}\t{}\t{}\n",
                        r.layer,
                        r.dt,
                        r.x,
                        r.y,
                        crate::tsv_esc(&r.s)
                    ));
                }
            }
            let p = format!("{}/labels_{}.tsv", out, gen);
            std::fs::write(&p, wbuf)
                .map_err(|e| e.to_string())?;
            (p, lp.rows.len(), ms, lp.stats)
        }
    };
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
         max_depth={} \
         bytes={} members={} plan_ms={:.2} wc_cells={} \
         inst_edges={} frame_rects={} partial={} deferred={} \
         lod={} washed={} labels={} nlabels={} text_plan_ms={:.2} \
         labels_truncated={} text_bvh_nodes={} \
         text_place_bvh_nodes={} text_place_records={} \
         text_members_tested={} text_members_visible={} \
         resident_committed_mb={:.1} \
         resident_projected_mb={:.1} \
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
        if v.ovm.n_cells == 0 {
            0
        } else {
            v.ovm.cell(v.ovm.top).height
        },
        bytes,
        members,
        t0.elapsed().as_secs_f64() * 1e3,
        st.wc_cells,
        st.inst_edges,
        st.frame_rects,
        upd.partial as u8,
        upd.deferred,
        st.lod_swapped,
        st.washed_pages,
        labels_path,
        nlabels,
        text_ms,
        text_stats.truncated as u8,
        text_stats.tbvh_nodes,
        text_stats.place_bvh_nodes,
        text_stats.place_records_scanned,
        text_stats.members_tested,
        text_stats.members_visible,
        mb(upd.committed_bytes),
        mb(upd.projected_bytes),
        mb(upd.pending_new_bytes),
        mb(upd.pending_evict_bytes),
    ))
}

#[cfg(test)]
mod split_tests {
    use super::*;

    /// #76 runnable-slot contract: a planner stalled beyond the ordered
    /// window lends its slot, cannot resume while a helper owns it, and
    /// reacquires it after the helper returns it. The final increment mirrors
    /// planner exit and proves that every lease is accounted exactly once.
    #[test]
    fn plan_window_slot_lending_requires_reacquire() {
        let budget = std::sync::atomic::AtomicIsize::new(0);

        budget.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert!(try_borrow_plan_slot(&budget), "helper missed lent slot");
        assert_eq!(budget.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert!(
            !try_borrow_plan_slot(&budget),
            "planner resumed while helper still owned its slot"
        );

        budget.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert!(
            try_borrow_plan_slot(&budget),
            "planner did not reacquire the returned slot"
        );
        budget.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(budget.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    /// The field regression was dominated by a serial 8,821-page LOD phase.
    /// A modest dense Pts record plus a small test page target creates at
    /// least 64 LOD candidates and proves that lent runnable slots reach the
    /// LOD fanout, then return to the shared budget.
    #[test]
    fn lod_uses_lent_plan_slots_and_returns_them() {
        const DIE: i64 = 1_000_000;
        let doc = mini_doc(vec![pts_rec(76, 200_000, DIE, 100)]);
        let rbb = cell_bboxes_full(&doc);
        let lidx: std::collections::HashMap<(u32, u32), usize> =
            doc.layer_order
                .iter()
                .enumerate()
                .map(|(i, &k)| (k, i))
                .collect();
        let budget = std::sync::atomic::AtomicIsize::new(3);
        let plan = build_cell_plan(
            &doc,
            0,
            &rbb,
            &lidx,
            1,
            8 * 1024,
            true,
            &budget,
            None,
            None,
            4,
            None,
        );
        assert!(
            plan.lod_threads > 1,
            "LOD did not borrow the lent runnable slots"
        );
        assert_eq!(
            budget.load(std::sync::atomic::Ordering::Relaxed),
            3,
            "LOD helper lease was not returned"
        );
    }

    #[test]
    fn lod_borrows_slot_lent_after_phase_start() {
        const DIE: i64 = 1_000_000;
        let doc = mini_doc(vec![pts_rec(77, 200_000, DIE, 100)]);
        let rbb = cell_bboxes_full(&doc);
        let lidx: std::collections::HashMap<(u32, u32), usize> =
            doc.layer_order
                .iter()
                .enumerate()
                .map(|(i, &k)| (k, i))
                .collect();
        let budget = std::sync::atomic::AtomicIsize::new(0);
        let waiters = std::sync::atomic::AtomicIsize::new(1);
        let join_barrier = std::sync::Barrier::new(2);
        let plan = std::thread::scope(|sc| {
            sc.spawn(|| {
                while waiters.load(
                    std::sync::atomic::Ordering::Acquire,
                ) != 0
                {
                    std::thread::yield_now();
                }
                budget.fetch_add(
                    1,
                    std::sync::atomic::Ordering::Release,
                );
            });
            build_cell_plan(
                &doc,
                0,
                &rbb,
                &lidx,
                1,
                8 * 1024,
                true,
                &budget,
                Some(&waiters),
                Some(&join_barrier),
                4,
                None,
            )
        });
        assert!(
            plan.lod_threads > 1,
            "late-lent slot never joined LOD work"
        );
        assert_eq!(
            waiters.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "late LOD waiter permit leaked"
        );
        assert_eq!(
            budget.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "late LOD planning slot was not returned"
        );
    }

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

    /// #60 monster-cell repro (release, ignored): ~500 die-wide
    /// Pts reps x 50k offsets - every page split partitions every
    /// straddling rep, the ESD-dummy shape (150M field: one cell,
    /// 9.2M fragments, plan 164s). Run:
    /// cargo test --release -p floe-index -- --ignored monster \
    ///   --nocapture
    #[test]
    #[ignore]
    fn monster_cell_split_bench() {
        const DIE: i64 = 4_000_000;
        let recs: Vec<RectRec> = (0..500u64)
            .map(|k| pts_rec(k + 11, 50_000, DIE, 200))
            .collect();
        let doc = mini_doc(recs);
        let t = std::time::Instant::now();
        let plan = plan_of(&doc);
        eprintln!(
            "monster: {:.2}s pages {} fragments {} \
             (bvh {:.2} asm {:.2} split {:.2} lod {:.2} pts \
             {:.2} sink {:.2})",
            t.elapsed().as_secs_f64(),
            plan.pages.len(),
            plan.split_stats.fragments,
            plan.phase_s[0],
            plan.phase_s[1],
            plan.phase_s[2],
            plan.phase_s[3],
            plan.phase_s[4],
            plan.phase_s[5]
        );
    }

    /// PRec assembly for a rect-only single-layer cell (mirror of
    /// build_cell_plan's rect arm) - lets tests drive plan_layer /
    /// plan_layer_frontier directly
    fn assemble_rects(cell: &floe_oasis::doc::Cell) -> Vec<PRec> {
        cell.rects
            .iter()
            .enumerate()
            .map(|(idx, r)| {
                let (ex, ey) = rep_extent(&r.rep);
                PRec {
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
                }
            })
            .collect()
    }

    /// #60 P2 determinism: frontier expansion + arena sharding +
    /// segment merge alone (threads=1) must be byte-neutral vs the
    /// serial recursion, on a layer that exercises every split
    /// path - mid Pts floods, one giant Pts rep, die-wide Rep::One
    /// spines (parent oversize before left/right), a coincident
    /// pile and a negative-vector skew grid.
    #[test]
    fn p2_forced_frontier_is_byte_neutral() {
        const DIE: i64 = 1_000_000;
        let mut recs: Vec<RectRec> = (0..40u64)
            .map(|k| pts_rec(k + 3, 3_000, DIE, 150))
            .collect();
        recs.push(pts_rec(999, 200_000, DIE, 80)); // giant (P2-B)
        for k in 0..10i64 {
            recs.push(RectRec {
                layer: 1,
                dt: 0,
                x: 0,
                y: k * 90_000,
                w: DIE,
                h: 60,
                rep: Rep::One,
            });
        }
        recs.push(RectRec {
            layer: 1,
            dt: 0,
            x: DIE / 2,
            y: DIE / 2,
            w: 40,
            h: 40,
            rep: Rep::Pts(vec![(0, 0); 5].into()),
        });
        recs.push(RectRec {
            layer: 1,
            dt: 0,
            x: DIE / 2,
            y: 0,
            w: 30,
            h: 30,
            rep: Rep::Grid {
                na: 400,
                nb: 300,
                va: (-2000, 700),
                vb: (350, 2900),
            },
        });
        let doc = mini_doc(recs);
        let cell = &doc.cells[0];
        let a = plan_layer(cell, 0, 0, assemble_rects(cell), 64 * 1024);
        let b = plan_layer_frontier(
            cell,
            0,
            0,
            assemble_rects(cell),
            64 * 1024,
            &P2Opts {
                threads: 1,
                target_tasks: 8,
                task_min: 4096,
                shard_limit: Some(u64::MAX),
                budget: None,
                lease: 0,
                late_waiters: None,
                helpers_used: None,
                join_barrier: None,
            },
        );
        assert!(
            b.p2_tasks > 1,
            "frontier formed {} task(s)",
            b.p2_tasks
        );
        assert!(b.shard_bytes > 0, "sharding never ran");
        assert!(
            a.stats.oversize_pages > 0,
            "fixture lost its oversize path"
        );
        layer_bytes_equal(&doc, &a, &b);
    }

    /// #76b regression: the real field can reach the P2 decision before
    /// future-cell planners fill the (default jobs*4) window. The old
    /// one-shot borrow then fixed the whole split at one thread even when a
    /// slot was lent milliseconds later. Synchronize the lender on waiter
    /// registration so this gate proves late uptake, lease return and byte
    /// neutrality rather than relying on timing.
    #[test]
    fn p2_borrows_slot_lent_after_frontier_start() {
        const DIE: i64 = 1_000_000;
        let recs: Vec<RectRec> = (0..40u64)
            .map(|k| pts_rec(k + 3, 1_000, DIE, 150))
            .collect();
        let doc = mini_doc(recs);
        let cell = &doc.cells[0];
        let a = plan_layer(
            cell,
            0,
            0,
            assemble_rects(cell),
            16 * 1024,
        );
        let budget = std::sync::atomic::AtomicIsize::new(0);
        let waiters = std::sync::atomic::AtomicIsize::new(1);
        let used = std::sync::atomic::AtomicUsize::new(0);
        let join_barrier = std::sync::Barrier::new(2);
        let b = std::thread::scope(|sc| {
            sc.spawn(|| {
                while waiters.load(
                    std::sync::atomic::Ordering::Acquire,
                ) != 0
                {
                    std::thread::yield_now();
                }
                budget.fetch_add(
                    1,
                    std::sync::atomic::Ordering::Release,
                );
            });
            plan_layer_frontier(
                cell,
                0,
                0,
                assemble_rects(cell),
                16 * 1024,
                &P2Opts {
                    threads: 2,
                    target_tasks: 8,
                    task_min: 2 * 1024,
                    shard_limit: Some(u64::MAX),
                    budget: Some(&budget),
                    lease: 0,
                    late_waiters: Some(&waiters),
                    helpers_used: Some(&used),
                    join_barrier: Some(&join_barrier),
                },
            )
        });
        assert!(
            b.p2_tasks > 1,
            "late frontier formed {} task(s)",
            b.p2_tasks
        );
        assert_eq!(
            used.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "late-lent slot never joined the P2 work"
        );
        assert_eq!(
            waiters.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "late waiter permit leaked"
        );
        assert_eq!(
            budget.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "late planning slot was not returned"
        );
        layer_bytes_equal(&doc, &a, &b);
    }

    /// #60 review: past the shard-copy ceiling the frontier's
    /// tasks run SERIALLY against the prefix arena - no copies
    /// (shard=0), byte-identical output
    #[test]
    fn p2_shard_limit_serial_fallback() {
        const DIE: i64 = 1_000_000;
        let mut recs: Vec<RectRec> = (0..40u64)
            .map(|k| pts_rec(k + 3, 3_000, DIE, 150))
            .collect();
        recs.push(pts_rec(999, 200_000, DIE, 80));
        let doc = mini_doc(recs);
        let cell = &doc.cells[0];
        let a = plan_layer(cell, 0, 0, assemble_rects(cell), 64 * 1024);
        let lease = std::sync::atomic::AtomicIsize::new(0);
        let b = plan_layer_frontier(
            cell,
            0,
            0,
            assemble_rects(cell),
            64 * 1024,
            &P2Opts {
                threads: 4,
                target_tasks: 8,
                task_min: 4096,
                shard_limit: Some(0),
                budget: Some(&lease),
                lease: 3,
                late_waiters: None,
                helpers_used: None,
                join_barrier: None,
            },
        );
        assert!(b.p2_tasks > 1, "frontier never formed");
        assert!(b.p2_fallback, "fallback flag missing");
        assert_eq!(b.shard_bytes, 0, "fallback still copied");
        // the lease returns AT the fallback decision, not at
        // layer end - the tail can re-lend it immediately
        assert_eq!(
            lease.load(std::sync::atomic::Ordering::Relaxed),
            3
        );
        layer_bytes_equal(&doc, &a, &b);
    }

    /// pages, pbvh, stats and encoded payloads of two LayerPlans
    /// must agree byte-for-byte (arena layout is scratch and may
    /// differ - the encode goes through each plan's own shards)
    fn layer_bytes_equal(doc: &Doc, a: &LayerPlan, b: &LayerPlan) {
        assert_eq!(a.stats.fragments, b.stats.fragments);
        assert_eq!(a.stats.oversize_pages, b.stats.oversize_pages);
        assert_eq!(a.stats.depth_capped, b.stats.depth_capped);
        assert_eq!(a.pbvh, b.pbvh);
        assert_eq!(a.pages.len(), b.pages.len());
        for (x, y) in a.pages.iter().zip(b.pages.iter()) {
            assert_eq!(x.seq, y.seq);
            assert_eq!(x.bbox, y.bbox);
            assert_eq!(x.members, y.members);
            let (pa, ra) = encode_job(doc, x, a.arenas.as_slice());
            let (pb, rb) = encode_job(doc, y, b.arenas.as_slice());
            assert_eq!(ra, rb);
            assert_eq!(pa, pb, "payload differs at seq {}", x.seq);
        }
    }

    /// #60 P2 mode selection: a dominant single layer past
    /// P2_MIN_BYTES engages the frontier when budget exists and
    /// falls back to the untouched serial path (no frontier, no
    /// sharding) when it does not; both agree structurally.
    #[test]
    fn p2_mode_engages_and_matches_serial() {
        const DIE: i64 = 2_000_000;
        // 500 x 4400 members ~ 8.8MB: just past P2_MIN_BYTES
        let recs: Vec<RectRec> = (0..500u64)
            .map(|k| pts_rec(k + 7, 4_400, DIE, 120))
            .collect();
        let doc = mini_doc(recs);
        let rbb = cell_bboxes_full(&doc);
        let lidx: std::collections::HashMap<(u32, u32), usize> =
            doc.layer_order
                .iter()
                .enumerate()
                .map(|(i, &k)| (k, i))
                .collect();
        let mut plans = Vec::new();
        for budget in [0isize, 7] {
            let b = std::sync::atomic::AtomicIsize::new(budget);
            let jobs = if budget == 0 { 1 } else { 8 };
            plans.push(build_cell_plan(
                &doc, 0, &rbb, &lidx, 1, MIB, false, &b, None,
                None, jobs, None,
            ));
        }
        let (a, b) = (&plans[0], &plans[1]);
        // jobs=1 is the explicit serial reference; jobs=8 engages P2.
        assert_eq!(a.p2_tasks, 0, "sharding without helpers");
        assert_eq!(a.split_threads, 1);
        assert_eq!(a.shard_bytes, 0);
        assert!(b.p2_tasks > 1, "P2 never engaged with budget");
        assert!(b.split_threads > 1);
        assert_eq!(a.pranges, b.pranges);
        assert_eq!(a.pbvh, b.pbvh);
        assert_eq!(a.pages.len(), b.pages.len());
        for (x, y) in a.pages.iter().zip(b.pages.iter()) {
            assert_eq!(x.li, y.li);
            assert_eq!(x.seq, y.seq);
            assert_eq!(x.bbox, y.bbox);
            assert_eq!(x.recs.len(), y.recs.len());
        }
    }

    /// #60 P2 bench (release, ignored): the ORIGINAL monster shape
    /// - one layer owns all 500 die-wide Pts floods. Run:
    /// cargo test --release -p floe-index -- --ignored p2_bench \
    ///   --nocapture
    /// Caveat on P/E-core hosts (this M4 = 4P+4E): 8 threads
    /// straggle on E cores and can read WORSE than 4 - judge
    /// scaling on the 4-thread row (uniform-core Linux scales
    /// through 8).
    #[test]
    #[ignore]
    fn monster_cell_p2_bench() {
        const DIE: i64 = 4_000_000;
        let recs: Vec<RectRec> = (0..500u64)
            .map(|k| pts_rec(k + 11, 50_000, DIE, 200))
            .collect();
        let doc = mini_doc(recs);
        let rbb = cell_bboxes_full(&doc);
        let lidx: std::collections::HashMap<(u32, u32), usize> =
            doc.layer_order
                .iter()
                .enumerate()
                .map(|(i, &k)| (k, i))
                .collect();
        let mut plans = Vec::new();
        for budget in [0isize, 1, 3, 7] {
            let b = std::sync::atomic::AtomicIsize::new(budget);
            let t = std::time::Instant::now();
            let plan = build_cell_plan(
                &doc, 0, &rbb, &lidx, 1, MIB, true, &b, None,
                None, 8, None,
            );
            eprintln!(
                "p2 budget {}: plan {:.2}s (split {:.2}/{}t \
                 p2_tasks={} shard={}MiB lod {:.2}/{}t, pages {})",
                budget,
                t.elapsed().as_secs_f64(),
                plan.phase_s[2],
                plan.split_threads,
                plan.p2_tasks,
                plan.shard_bytes / MIB,
                plan.phase_s[3],
                plan.lod_threads,
                plan.pages.len(),
            );
            plans.push(plan);
        }
        let (a, b) = (&plans[0], &plans[1]);
        assert!(b.p2_tasks > 1, "P2 never engaged");
        assert_eq!(a.pranges, b.pranges);
        assert_eq!(a.pbvh, b.pbvh);
        assert_eq!(a.pages.len(), b.pages.len());
        // arena_slot intentionally NOT compared: P2 shards the
        // scratch differently than the serial single slot - the
        // output bytes (S7) are the oracle for that
        for (x, y) in a.pages.iter().zip(b.pages.iter()) {
            assert_eq!(x.li, y.li);
            assert_eq!(x.seq, y.seq);
            assert_eq!(x.bbox, y.bbox);
            assert_eq!(x.recs.len(), y.recs.len());
        }
    }

    /// #60 P1 fanout bench (release, ignored): the same rep flood
    /// spread over 8 layers, planned serial (budget 0) then fanned
    /// out (budget 7); the two plans must agree structurally. Run:
    /// cargo test --release -p floe-index -- --ignored fanout \
    ///   --nocapture
    #[test]
    #[ignore]
    fn monster_cell_fanout_bench() {
        const DIE: i64 = 4_000_000;
        const NL: u32 = 8;
        let recs: Vec<RectRec> = (0..500u64)
            .map(|k| {
                let mut r = pts_rec(k + 11, 50_000, DIE, 200);
                r.layer = 1 + (k % NL as u64) as u32;
                r
            })
            .collect();
        let mut c = floe_oasis::doc::Cell::default();
        c.name = String::from("T");
        c.rects = recs;
        let doc = Doc {
            unit: 1000.0,
            cells: vec![c],
            top: 0,
            layer_order: (0..NL).map(|l| (1 + l, 0)).collect(),
            norm_s: 0.0,
            layer_names: Default::default(),
            layer_aliases: Default::default(),
        };
        let rbb = cell_bboxes_full(&doc);
        let lidx: std::collections::HashMap<(u32, u32), usize> =
            doc.layer_order
                .iter()
                .enumerate()
                .map(|(i, &k)| (k, i))
                .collect();
        let mut plans = Vec::new();
        for budget in [0isize, 7] {
            let b = std::sync::atomic::AtomicIsize::new(budget);
            let t = std::time::Instant::now();
            let plan = build_cell_plan(
                &doc,
                0,
                &rbb,
                &lidx,
                NL as usize,
                MIB,
                true,
                &b,
                None,
                None,
                8,
                None,
            );
            eprintln!(
                "fanout budget {}: plan {:.2}s (split {:.2}/{}t \
                 lod {:.2}/{}t, pages {})",
                budget,
                t.elapsed().as_secs_f64(),
                plan.phase_s[2],
                plan.split_threads,
                plan.phase_s[3],
                plan.lod_threads,
                plan.pages.len(),
            );
            plans.push(plan);
        }
        let (a, b) = (&plans[0], &plans[1]);
        assert!(b.split_threads > 1, "fanout never engaged");
        assert_eq!(a.pranges, b.pranges);
        assert_eq!(a.pbvh, b.pbvh);
        assert_eq!(a.pages.len(), b.pages.len());
        for (x, y) in a.pages.iter().zip(b.pages.iter()) {
            assert_eq!(x.li, y.li);
            assert_eq!(x.seq, y.seq);
            assert_eq!(x.bbox, y.bbox);
            assert_eq!(x.arena_slot, y.arena_slot);
            assert_eq!(x.recs.len(), y.recs.len());
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
            layer_aliases: Default::default(),
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
        let budget = std::sync::atomic::AtomicIsize::new(0);
        build_cell_plan(
            doc, 0, &rbb, &lidx, 1, MIB, true, &budget, None,
            None, 1, None,
        )
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
                encode_job(doc, job, plan.arenas.as_slice());
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
            encode_job(doc, &plan.pages[k], plan.arenas.as_slice());
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

    /// orthogonal grids mark ANALYTICALLY (no member loop): the
    /// coverage contract must hold against the brute expansion
    #[test]
    fn lod_grid_analytic_superset() {
        let doc = mini_doc(vec![RectRec {
            layer: 1,
            dt: 0,
            x: 500,
            y: 700,
            w: 120,
            h: 90,
            rep: Rep::Grid {
                na: 400,
                nb: 400,
                va: (2400, 0),
                vb: (0, 2300),
            },
        }]);
        let plan = plan_of(&doc);
        let k = (0..plan.pages.len())
            .find(|&k| {
                plan.pages[k].lod_page != floe_ovm::LOD_PAGE_NONE
            })
            .expect("no lod page for a 160k-member grid");
        let e = &plan.pages[k];
        let bb = e.bbox;
        let eb = raster(&page_mems(&doc, &plan, k), &bb);
        let lb = raster(
            &page_mems(&doc, &plan, e.lod_page as usize),
            &bb,
        );
        for i in 0..eb.len() {
            assert!(!eb[i] || lb[i], "hole at cell {}", i);
        }
        let ok = dilate(&eb);
        for i in 0..lb.len() {
            assert!(!lb[i] || ok[i], "overcover at cell {}", i);
        }
    }

    /// a skew grid past the enumeration cap must never loop: it
    /// rides the LOD payload verbatim, rep intact
    #[test]
    fn lod_skew_mega_grid_rides_verbatim() {
        let doc = mini_doc(vec![RectRec {
            layer: 1,
            dt: 0,
            x: 0,
            y: 0,
            w: 80,
            h: 60,
            rep: Rep::Grid {
                na: 300,
                nb: 300,
                va: (2900, 1150),
                vb: (-600, 2800),
            },
        }]);
        let plan = plan_of(&doc);
        assert!(plan.split_stats.lod_grid_verbatim >= 1);
        let k = (0..plan.pages.len())
            .find(|&k| {
                plan.pages[k].lod_page != floe_ovm::LOD_PAGE_NONE
            })
            .expect("no lod page");
        let li = plan.pages[k].lod_page as usize;
        let mems = page_mems(&doc, &plan, li);
        assert_eq!(mems.len(), 90_000, "rep not kept verbatim");
    }

    /// v6 max_min: mixed-orientation thin wires keep max_w AND
    /// max_h large - only max_min proves the page is all-hairline.
    /// One fat record lifts the floor and saves the page.
    #[test]
    fn split_max_min_detects_hairline_pages() {
        const DIE: i64 = 1_000_000;
        let thin = |x: i64, y: i64, w: i64, h: i64| RectRec {
            layer: 1,
            dt: 0,
            x,
            y,
            w,
            h,
            rep: Rep::One,
        };
        let doc =
            mini_doc(vec![thin(0, 0, DIE, 90), thin(0, 5000, 80, DIE)]);
        let plan = plan_of(&doc);
        let j = plan
            .pages
            .iter()
            .find(|j| j.lod == floe_ovm::LOD_EXACT)
            .unwrap();
        assert!(j.max_w >= DIE && j.max_h >= DIE);
        assert_eq!(j.max_min, 90);
        let doc2 = mini_doc(vec![
            thin(0, 0, DIE, 90),
            thin(100, 100, 7000, 7000),
        ]);
        let plan2 = plan_of(&doc2);
        let j2 = plan2
            .pages
            .iter()
            .find(|j| j.lod == floe_ovm::LOD_EXACT)
            .unwrap();
        assert_eq!(j2.max_min, 7000);
    }

    /// sparse pages stay LOD-free (trigger threshold; M7-C lowered
    /// the floor to 256 - sub-256 pages cannot saturate a fit view
    /// even en masse, and the wash degrade covers them anyway)
    #[test]
    fn lod_trigger_skips_sparse() {
        const DIE: i64 = 1_000_000;
        let doc = mini_doc(vec![pts_rec(3, 200, DIE, 90)]);
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
