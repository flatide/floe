//! floe-index CLI.
//!
//!   scan  - JSON inventory (cells, per-layer record/member counts,
//!           texts, placements) + throughput; validated against
//!           klayout by tools/validate_rust_scan.py
//!   tile  - band partitioning with an EXPLICIT grid (debug/A-B path)
//!   index - the production build: grid choice, band tiles, LOD
//!           companions, density table, meta.json - a complete .ice
//!           (skeleton/texts land with milestone 3)

use std::time::Instant;

mod vfs;

#[cfg(target_env = "musl")]
mod tcache;

// musl's global-lock malloc serialized every worker (MAIN09: flat
// ~190s from 1 to 24 threads; glibc 60s), so the thread cache fronts
// it on the musl static build only. glibc and macOS have per-thread
// caches of their own, and the class rounding (power-of-two classes)
// plus retained free lists only inflate RSS there.
#[cfg(target_env = "musl")]
#[global_allocator]
static ALLOC: tcache::TCache = tcache::TCache;

// elsewhere the platform malloc is the whole point - pin it
// explicitly (the unadorned default is unspecified per the docs)
#[cfg(not(target_env = "musl"))]
#[global_allocator]
static ALLOC: std::alloc::System = std::alloc::System;

fn version() -> String {
    // the package version is bumped on EVERY push that touches
    // rust/, so it identifies a zip-carried build by itself; the git
    // hash is extra precision when the tree knows it
    let git = env!("FLOE_GIT");
    format!(
        "floe-index {}{} ({})",
        env!("CARGO_PKG_VERSION"),
        if git == "unknown" {
            String::new()
        } else {
            format!(" {}", git)
        },
        if cfg!(target_env = "musl") {
            "musl-static"
        } else if cfg!(target_os = "linux") {
            "gnu"
        } else {
            "native"
        }
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && (args[1] == "--version" || args[1] == "-V") {
        println!("{}", version());
        return;
    }
    // every run states which build it is - multiple binaries
    // circulate on the closed-network hosts
    eprintln!("[{}]", version());
    if args.len() >= 3 && args[1] == "tile" {
        return tile_cmd(&args[2..]);
    }
    if args.len() >= 3 && args[1] == "index" {
        return index_cmd(&args[2..]);
    }
    if args.len() >= 3 && args[1] == "vfs" {
        return vfs::vfs_cmd(&args[2..]);
    }
    if args.len() >= 3 && args[1] == "plan" {
        return vfs::plan_cmd(&args[2..]);
    }
    if args.len() >= 3 && args[1] == "vfsd" {
        return vfs::vfsd_cmd(&args[2..]);
    }
    if args.len() < 3 || args[1] != "scan" {
        eprintln!(
            "usage: floe-index scan <file.oas> [jobs]\n       \
             floe-index tile <file.oas> <outdir> \
             --grid x0,y0,tw,th,nx,ny --edges e0,e1,e2\n       \
             floe-index index <file.oas> [outdir] [--mem GB] \
             [--jobs N] [--tile-bytes N] [--bands um,um,um]\n       \
             floe-index vfs <file.oas> [outdir] [--jobs N] \
             [--plan-batch N] [--encode-batch N] \
             [--page-target-mb N] \
             [--coverage | --coverage-only]\n       \
             floe-index plan <outdir> --view x0,y0,x1,y1 \
             [--px-per-um N] [--cut-px N] [--layers a/b,..] \
             [--depth N]"
        );
        std::process::exit(2);
    }
    let path = &args[2];
    let jobs: usize = args
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let t0 = Instant::now();
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("read {}: {}", path, e);
            std::process::exit(1);
        }
    };
    let t_read = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    let st = match floe_oasis::scan_parallel(&data, jobs) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("scan {}: {}", path, e);
            std::process::exit(1);
        }
    };
    let t_scan = t1.elapsed().as_secs_f64();

    let mut shapes: Vec<_> = st.shapes.iter().collect();
    shapes.sort_by_key(|(k, _)| **k);
    let mut texts: Vec<_> = st.texts.iter().collect();
    texts.sort_by_key(|(k, _)| **k);

    // hand-rolled JSON keeps the spike dependency-free
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str(&format!("  \"file_bytes\": {},\n", st.file_bytes));
    out.push_str(&format!("  \"unit\": {},\n", st.unit));
    out.push_str(&format!("  \"records\": {},\n", st.records));
    out.push_str(&format!("  \"cells\": {},\n", st.cells));
    out.push_str(&format!("  \"cellnames\": {},\n", st.cellnames));
    out.push_str(&format!("  \"placements\": {},\n", st.placements));
    out.push_str(&format!(
        "  \"placement_members\": {},\n",
        st.placement_members
    ));
    out.push_str(&format!("  \"cblocks\": {},\n", st.cblocks));
    out.push_str(&format!(
        "  \"cblock_bytes_inflated\": {},\n",
        st.cblock_bytes_inflated
    ));
    let mut rids: Vec<_> = st.record_ids.iter().collect();
    rids.sort_by_key(|(k, _)| **k);
    let mut rts: Vec<_> = st.rep_types.iter().collect();
    rts.sort_by_key(|(k, _)| **k);
    out.push_str("  \"rep_types\": {");
    for (i, (k, v)) in rts.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!("\"{}\": {}", k, v));
    }
    out.push_str("},\n");
    out.push_str("  \"record_ids\": {");
    for (i, (k, v)) in rids.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!("\"{}\": {}", k, v));
    }
    out.push_str("},\n");
    out.push_str("  \"shapes\": {");
    for (i, ((l, d), s)) in shapes.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "\n    \"{}/{}\": {{\"records\": {}, \"members\": {}}}",
            l, d, s.records, s.members
        ));
    }
    out.push_str("\n  },\n  \"texts\": {");
    for (i, ((l, d), s)) in texts.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "\n    \"{}/{}\": {{\"records\": {}, \"members\": {}}}",
            l, d, s.records, s.members
        ));
    }
    out.push_str(&format!(
        "\n  }},\n  \"read_s\": {:.3},\n  \"scan_s\": {:.3},\n  \
         \"scan_mb_s\": {:.1}\n}}",
        t_read,
        t_scan,
        st.file_bytes as f64 / 1e6 / t_scan
    ));
    println!("{}", out);
}

fn tile_cmd(args: &[String]) {
    // <file.oas> <outdir> --grid x0,y0,tw,th,nx,ny --edges e0,e1,e2
    let mut src = None;
    let mut outdir = None;
    let mut grid = None;
    let mut edges = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--grid" => {
                let v: Vec<i64> = args[i + 1]
                    .split(',')
                    .map(|s| s.parse().expect("grid int"))
                    .collect();
                assert_eq!(v.len(), 6, "--grid x0,y0,tw,th,nx,ny");
                grid = Some(floe_tiler::Grid {
                    x0: v[0],
                    y0: v[1],
                    tw: v[2],
                    th: v[3],
                    nx: v[4],
                    ny: v[5],
                });
                i += 2;
            }
            "--edges" => {
                edges = Some(
                    args[i + 1]
                        .split(',')
                        .map(|s| s.parse().expect("edge int"))
                        .collect::<Vec<i64>>(),
                );
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
    let (src, outdir) = (src.expect("src"), outdir.expect("outdir"));
    let grid = grid.expect("--grid required");
    let edges = edges.expect("--edges required");

    let t0 = Instant::now();
    let data = std::fs::read(&src).expect("read src");
    let t_read = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    let doc = match floe_oasis::doc::parse_doc(&data) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("parse {}: {}", src, e);
            std::process::exit(1);
        }
    };
    let t_parse = t1.elapsed().as_secs_f64();
    let nbands = edges.len() + 1;
    for k in 0..nbands {
        std::fs::create_dir_all(format!("{}/tiles_b{}", outdir, k))
            .expect("mkdir");
    }
    // hierarchy-preserving path: per tile a variant tree, mirrored
    // into every band (klayout-parity naming: NAME[$v]__b<k>)
    let hier = floe_tiler::hier::HierTiler::new(&doc, grid, edges);
    let mut files = 0u64;
    let mut members = 0u64;
    let mut t_tile = 0.0f64;
    let mut t_write = 0.0f64;
    for r in 0..grid.ny {
        for c in 0..grid.nx {
            let tt = Instant::now();
            let tree = match hier.build_tile(r, c) {
                Ok(Some(t)) => t,
                Ok(None) => {
                    t_tile += tt.elapsed().as_secs_f64();
                    continue;
                }
                Err(e) => {
                    eprintln!("tile {},{}: {}", r, c, e);
                    std::process::exit(1);
                }
            };
            t_tile += tt.elapsed().as_secs_f64();
            members += tree.members;
            let tw = Instant::now();
            files += write_band_files(&doc, &tree, &outdir, r, c, nbands);
            t_write += tw.elapsed().as_secs_f64();
        }
    }
    println!(
        "{{\n  \"cells\": {},\n  \"members\": {},\n  \
         \"band_files\": {},\n  \
         \"read_s\": {:.3},\n  \"parse_s\": {:.3},\n  \
         \"tile_s\": {:.3},\n  \"write_s\": {:.3},\n  \
         \"total_s\": {:.3}\n}}",
        doc.cells.len(),
        members,
        files,
        t_read,
        t_parse,
        t_tile,
        t_write,
        t0.elapsed().as_secs_f64()
    );
}

/// Base cell names of a tile tree: root = "" (band/file specific),
/// full definitions their design name, clipped variants NAME$ord.
fn base_names(
    doc: &floe_oasis::doc::Doc,
    tree: &floe_tiler::hier::TileTree,
) -> Vec<String> {
    tree.cells
        .iter()
        .enumerate()
        .map(|(i, vc)| {
            if i == tree.root {
                String::new()
            } else if vc.ord == 0 {
                doc.cells[vc.design].name.clone()
            } else {
                format!("{}${}", doc.cells[vc.design].name, vc.ord)
            }
        })
        .collect()
}

/// Write one tile's per-band files (reach-pruned mirrors); returns
/// the number of files written.
fn write_band_files(
    doc: &floe_oasis::doc::Doc,
    tree: &floe_tiler::hier::TileTree,
    outdir: &str,
    r: i64,
    c: i64,
    nbands: usize,
) -> u64 {
    let base = base_names(doc, tree);
    let mut files = 0u64;
    for k in 0..nbands {
        if !tree.reach[k][tree.root] {
            continue;
        }
        let bname: Vec<String> = (0..tree.cells.len())
            .map(|i| {
                if i == tree.root {
                    format!("TILE_{}_{}_b{}", r, c, k)
                } else {
                    format!("{}__b{}", base[i], k)
                }
            })
            .collect();
        let wcells: Vec<floe_oasis::write::WCell> = tree
            .cells
            .iter()
            .enumerate()
            .filter(|(i, _)| tree.reach[k][*i])
            .map(|(i, vc)| floe_oasis::write::WCell {
                name: bname[i].clone(),
                rects: &vc.bands[k].rects,
                polys: &vc.bands[k].polys,
                paths: &vc.bands[k].paths,
                texts: &[],
                places: vc
                    .places
                    .iter()
                    .filter(|p| tree.reach[k][p.var])
                    .map(|p| {
                        (bname[p.var].as_str(), p.x, p.y, p.rot,
                         p.flip, &p.rep)
                    })
                    .collect(),
            })
            .collect();
        let bytes = floe_oasis::write::write_tree(&wcells, doc.unit)
            .expect("serialize");
        std::fs::write(
            format!("{}/tiles_b{}/t_{}_{}.oas", outdir, k, r, c),
            bytes,
        )
        .expect("write tile");
        files += 1;
    }
    files
}

/// Write one tile's LOD companion (cache._tile_lod parity): whole
/// levels kept under the cap, the first level beyond as GHOST bbox
/// rects on 254/0, deeper levels dropped; no cut -> the full tile.
/// Returns the depth the file serves when a cut happened.
fn write_lod_file(
    doc: &floe_oasis::doc::Doc,
    tree: &floe_tiler::hier::TileTree,
    outdir: &str,
    r: i64,
    c: i64,
    cap: u64,
) -> Option<usize> {
    use floe_oasis::doc::{PathRec, PolyRec, RectRec, Rep};
    let base = base_names(doc, tree);
    let name_of = |i: usize| -> String {
        if i == tree.root {
            format!("TILE_{}_{}", r, c)
        } else {
            base[i].clone()
        }
    };
    let cut = tree.lod_cut(cap);
    let (included, ghosts): (Vec<usize>, Vec<usize>) = match &cut {
        Some(lc) => (lc.kept.clone(), lc.ghosts.clone()),
        None => ((0..tree.cells.len()).collect(), Vec::new()),
    };
    // owned merged content first (WCell borrows slices)
    #[allow(clippy::type_complexity)]
    let merged: Vec<(usize, Vec<RectRec>, Vec<PolyRec>, Vec<PathRec>)> =
        included
            .iter()
            .map(|&i| {
                let vc = &tree.cells[i];
                let mut rects = Vec::new();
                let mut polys = Vec::new();
                let mut paths = Vec::new();
                for band in &vc.bands {
                    rects.extend(band.rects.iter().cloned());
                    polys.extend(band.polys.iter().cloned());
                    paths.extend(band.paths.iter().cloned());
                }
                (i, rects, polys, paths)
            })
            .collect();
    let ghost_rects: Vec<(usize, Vec<RectRec>)> = if ghosts.is_empty() {
        Vec::new()
    } else {
        let bb = tree.subtree_bboxes();
        ghosts
            .iter()
            .map(|&i| {
                let b = bb[i];
                (i, vec![RectRec {
                    layer: 254,
                    dt: 0,
                    x: b.0,
                    y: b.1,
                    w: b.2 - b.0,
                    h: b.3 - b.1,
                    rep: Rep::One,
                }])
            })
            .collect()
    };
    let names: Vec<String> =
        (0..tree.cells.len()).map(name_of).collect();
    let empty_polys: Vec<PolyRec> = Vec::new();
    let mut wcells: Vec<floe_oasis::write::WCell> = Vec::new();
    for (i, rects, polys, paths) in &merged {
        wcells.push(floe_oasis::write::WCell {
            name: names[*i].clone(),
            rects,
            polys,
            paths,
            texts: &[],
            places: tree.cells[*i]
                .places
                .iter()
                .map(|p| {
                    (names[p.var].as_str(), p.x, p.y, p.rot, p.flip,
                     &p.rep)
                })
                .collect(),
        });
    }
    for (i, rects) in &ghost_rects {
        wcells.push(floe_oasis::write::WCell {
            name: names[*i].clone(),
            rects,
            polys: &empty_polys,
            paths: &[],
            texts: &[],
            places: Vec::new(),
        });
    }
    let bytes = floe_oasis::write::write_tree(&wcells, doc.unit)
        .expect("serialize lod");
    std::fs::write(
        format!("{}/tiles_lod/t_{}_{}.oas", outdir, r, c),
        bytes,
    )
    .expect("write lod");
    cut.map(|lc| lc.depth)
}

// ------------------------------------------------------------ index

const CACHE_VERSION: u64 = 8;
const TILE_TARGET_BYTES: u64 = 6_000_000;
const GRID_MIN: i64 = 4;
const GRID_MAX: i64 = 96;
const LOD_SHAPE_CAP: u64 = 50_000;
const DENSITY_LEVELS: usize = 12;
const BANDS_UM: [f64; 3] = [0.125, 0.5, 2.0];

fn jesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out
}

/// colorsys.hsv_to_rgb + int(x*255) truncation, bit-for-bit - the
/// palette must match the Python indexer's so the same layout gets
/// the same colors from either builder
fn layer_color(i: usize) -> String {
    let h = ((i as f64) * 137.508) % 360.0 / 360.0;
    let (s, v) = (0.75f64, 1.0f64);
    let i6 = (h * 6.0) as i64;
    let f = h * 6.0 - i6 as f64;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i6 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    format!(
        "#{:02x}{:02x}{:02x}",
        (r * 255.0) as u8,
        (g * 255.0) as u8,
        (b * 255.0) as u8
    )
}

/// /proc metrics (Linux); None elsewhere - the governor is inert
/// on platforms without them
fn proc_kv_gb(path: &str, key: &str) -> Option<f64> {
    let s = std::fs::read_to_string(path).ok()?;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            let kb: f64 = rest
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse()
                .ok()?;
            return Some(kb / 1e6);
        }
    }
    None
}

fn mem_available_gb() -> Option<f64> {
    proc_kv_gb("/proc/meminfo", "MemAvailable:")
}

fn mem_total_gb() -> Option<f64> {
    proc_kv_gb("/proc/meminfo", "MemTotal:")
}

fn own_rss_gb() -> Option<f64> {
    proc_kv_gb("/proc/self/status", "VmRSS:")
}

fn peak_rss_gb() -> Option<f64> {
    proc_kv_gb("/proc/self/status", "VmHWM:")
}

/// ", rss X GB" suffix for phase-boundary log lines (empty off-Linux)
/// - attributes the peak (VmHWM) to a phase without host-side polling
fn rss_note() -> String {
    own_rss_gb()
        .map(|g| format!(", rss {:.1} GB", g))
        .unwrap_or_default()
}

/// du -h style: binary units, one decimal below 10
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

/// (total bytes, file count) of the files directly inside dir
fn dir_bytes(dir: &str) -> (u64, u64) {
    let (mut bytes, mut n) = (0u64, 0u64);
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if let Ok(md) = e.metadata() {
                if md.is_file() {
                    bytes += md.len();
                    n += 1;
                }
            }
        }
    }
    (bytes, n)
}

fn tsv_esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// repetition factors of a sidecar entry: g:na,nb,vax,vay,vbx,vby or
/// p:x y x y ... - ';'-joined, offsets already in top frame
fn fmt_factors(factors: &[floe_oasis::doc::Rep]) -> String {
    use floe_oasis::doc::Rep;
    factors
        .iter()
        .map(|f| match f {
            Rep::One => "1".to_string(),
            Rep::Grid { na, nb, va, vb } => format!(
                "g:{},{},{},{},{},{}",
                na, nb, va.0, va.1, vb.0, vb.1
            ),
            Rep::Pts(p) => format!(
                "p:{}",
                p.iter()
                    .map(|(x, y)| format!("{} {}", x, y))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn index_cmd(args: &[String]) {
    let mut src: Option<String> = None;
    let mut outdir: Option<String> = None;
    let mut tile_bytes = TILE_TARGET_BYTES;
    let mut bands_um: Vec<f64> = BANDS_UM.to_vec();
    let mut jobs: Option<usize> = None;
    let mut mem_cap: Option<f64> = None;
    let mut mem_floor: Option<f64> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--jobs" => {
                jobs = Some(args[i + 1].parse().expect("jobs"));
                i += 2;
            }
            "--mem" => {
                mem_cap = Some(args[i + 1].parse().expect("mem GB"));
                i += 2;
            }
            "--mem-floor" => {
                mem_floor =
                    Some(args[i + 1].parse().expect("mem floor GB"));
                i += 2;
            }
            "--tile-bytes" => {
                tile_bytes = args[i + 1].parse().expect("tile bytes");
                i += 2;
            }
            "--bands" => {
                bands_um = args[i + 1]
                    .split(',')
                    .map(|s| s.parse().expect("band um"))
                    .collect();
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
    let outdir = outdir.unwrap_or_else(|| format!("{}.ice", src));
    let abs_src = if std::path::Path::new(&src).is_absolute() {
        src.clone()
    } else {
        std::env::current_dir()
            .expect("cwd")
            .join(&src)
            .to_string_lossy()
            .into_owned()
    };

    let t_all = Instant::now();
    eprintln!("[index] reading {}...", src);
    let data = std::fs::read(&src).expect("read src");
    let size = data.len() as u64;
    let mtime = std::fs::metadata(&src)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .expect("mtime");
    let t_read = t_all.elapsed().as_secs_f64();
    eprintln!(
        "[index] read {:.2} GB in {:.1}s",
        size as f64 / 1e9,
        t_read
    );

    let jobs = jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    });
    let t1 = Instant::now();
    let doc = match floe_oasis::doc::parse_doc_parallel(&data, jobs) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("parse {}: {}", src, e);
            std::process::exit(1);
        }
    };
    let t_parse = t1.elapsed().as_secs_f64();
    eprintln!(
        "[index] parsed {} cells in {:.1}s ({} threads, \
         grid-normalize {:.1}s{})",
        doc.cells.len(),
        t_parse,
        jobs,
        doc.norm_s,
        rss_note()
    );
    let dbu = 1.0 / doc.unit;

    // meta bbox and grid derive from the PRE-strip layout: klayout's
    // cell bbox counts text anchors as point boxes
    let full_bb = floe_tiler::hier::cell_bboxes_full(&doc);
    let bbox = match full_bb[doc.top] {
        Some(b) => b,
        None => {
            eprintln!("empty top cell");
            std::process::exit(1);
        }
    };
    let n = (((size as f64 / tile_bytes as f64).sqrt())
        .round_ties_even() as i64)
        .clamp(GRID_MIN, GRID_MAX);
    let tile_w = (bbox.2 - bbox.0 + n - 1) / n;
    let tile_h = (bbox.3 - bbox.1 + n - 1) / n;
    let grid = floe_tiler::Grid {
        x0: bbox.0,
        y0: bbox.1,
        tw: tile_w,
        th: tile_h,
        nx: n,
        ny: n,
    };
    eprintln!(
        "[index] grid {}x{}, tile {:.0} x {:.0} um",
        grid.nx,
        grid.ny,
        grid.tw as f64 * dbu,
        grid.th as f64 * dbu
    );
    let edges: Vec<i64> = bands_um
        .iter()
        .map(|um| (um / dbu).round() as i64)
        .collect();
    let nbands = edges.len() + 1;
    for k in 0..nbands {
        std::fs::create_dir_all(format!("{}/tiles_b{}", outdir, k))
            .expect("mkdir");
    }
    std::fs::create_dir_all(format!("{}/tiles_lod", outdir))
        .expect("mkdir lod");

    let hier =
        floe_tiler::hier::HierTiler::new(&doc, grid, edges.clone());
    // tiles are independent (build + band files + lod + density per
    // tile): scoped threads pull coordinates off a shared counter;
    // results merge and sort by (r, c) so every output byte is
    // identical to the sequential build
    struct TRes {
        r: i64,
        c: i64,
        members: u64,
        files: u64,
        lod: Option<usize>,
        dens: Option<String>,
    }
    let coords: Vec<(i64, i64)> = (0..grid.ny)
        .flat_map(|r| (0..grid.nx).map(move |c| (r, c)))
        .collect();
    // memory governor: concurrent heavy tiles once swamped a host
    // (24 workers made zero progress while 4 sailed - swap storm,
    // the same failure the python indexer's governor was built for).
    // Workers pause BEFORE building another tile while the box's
    // MemAvailable sits under the floor or our own RSS tops --mem;
    // at least one builder always runs.
    let floor_gb: f64 = mem_floor.unwrap_or_else(|| {
        mem_total_gb().map(|t| (0.05 * t).max(4.0)).unwrap_or(0.0)
    });
    if mem_available_gb().is_some() {
        eprintln!(
            "[index] governor: floor {:.1} GB free{}",
            floor_gb,
            mem_cap
                .map(|c| format!(", --mem cap {:.0} GB", c))
                .unwrap_or_default()
        );
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let finished = std::sync::atomic::AtomicUsize::new(0);
    let active = std::sync::atomic::AtomicUsize::new(0);
    let waiting = std::sync::atomic::AtomicUsize::new(0);
    // cumulative wait episodes for the end-of-run summary (the
    // gauge above is the live count shown in heartbeats)
    let mem_waits = std::sync::atomic::AtomicUsize::new(0);
    // few tiles = long tiles: per-tile completion lines are the
    // useful signal there and stay quiet on fine grids
    let per_tile_log = coords.len() <= 64;
    let t2 = Instant::now();
    let mut all: Vec<TRes> = Vec::new();
    std::thread::scope(|sc| {
        // liveness monitor: heartbeats must not depend on tiles
        // COMPLETING (a 24-worker run on 25 heavy tiles once went
        // silent for its whole tiling phase)
        {
            let finished = &finished;
            let next = &next;
            let waiting = &waiting;
            let total = coords.len();
            sc.spawn(move || {
                use std::sync::atomic::Ordering::Relaxed;
                let mut last = Instant::now();
                loop {
                    std::thread::sleep(
                        std::time::Duration::from_millis(200),
                    );
                    let f = finished.load(Relaxed);
                    if f >= total {
                        return;
                    }
                    if last.elapsed().as_secs_f64() >= 5.0 {
                        last = Instant::now();
                        let s = next.load(Relaxed).min(total);
                        let w = waiting.load(Relaxed);
                        let wtxt = if w > 0 {
                            format!(", {} waiting (mem)", w)
                        } else {
                            String::new()
                        };
                        eprintln!(
                            "[index] tiles {} done, {} building{} / {} ({}s)",
                            f,
                            s - f - w,
                            wtxt,
                            total,
                            t2.elapsed().as_secs()
                        );
                    }
                }
            });
        }
        let mut hs = Vec::new();
        for _ in 0..jobs.max(1) {
            let hier = &hier;
            let doc = &doc;
            let outdir: &str = &outdir;
            let next = &next;
            let finished = &finished;
            let active = &active;
            let waiting = &waiting;
            let mem_waits = &mem_waits;
            let coords = &coords;
            hs.push(sc.spawn(move || -> Result<Vec<TRes>, String> {
                use std::sync::atomic::Ordering::Relaxed;
                let mut out = Vec::new();
                loop {
                    let i = next.fetch_add(1, Relaxed);
                    if i >= coords.len() {
                        break;
                    }
                    let (r, c) = coords[i];
                    let mut waited = false;
                    loop {
                        if active.load(Relaxed) == 0 {
                            break; // someone must always run
                        }
                        let mut hold = false;
                        if let Some(av) = mem_available_gb() {
                            if av < floor_gb {
                                hold = true;
                            }
                        }
                        if let (Some(cap), Some(rss)) =
                            (mem_cap, own_rss_gb())
                        {
                            if rss > cap {
                                hold = true;
                            }
                        }
                        if !hold {
                            break;
                        }
                        if !waited {
                            waited = true;
                            waiting.fetch_add(1, Relaxed);
                            mem_waits.fetch_add(1, Relaxed);
                        }
                        std::thread::sleep(
                            std::time::Duration::from_millis(300),
                        );
                    }
                    if waited {
                        waiting.fetch_sub(1, Relaxed);
                    }
                    active.fetch_add(1, Relaxed);
                    let tt = Instant::now();
                    let tree = match hier.build_tile(r, c) {
                        Ok(Some(t)) => t,
                        Ok(None) => {
                            active.fetch_sub(1, Relaxed);
                            finished.fetch_add(1, Relaxed);
                            continue;
                        }
                        Err(e) => {
                            return Err(format!(
                                "tile {},{}: {}",
                                r, c, e
                            ))
                        }
                    };
                    let files = write_band_files(
                        doc, &tree, outdir, r, c, nbands,
                    );
                    let lod = write_lod_file(
                        doc, &tree, outdir, r, c, LOD_SHAPE_CAP,
                    );
                    let dens = tree.density(DENSITY_LEVELS).map(
                        |(arrs, cells)| {
                            let mut parts: Vec<String> = arrs
                                .iter()
                                .map(|((l, d), arr)| {
                                    format!(
                                        "\"{}/{}\": [{}]",
                                        l,
                                        d,
                                        arr.iter()
                                            .map(|v| v.to_string())
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    )
                                })
                                .collect();
                            parts.push(format!(
                                "\"cells\": [{}]",
                                cells
                                    .iter()
                                    .map(|v| v.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ));
                            format!(
                                "\"{},{}\": {{{}}}",
                                r,
                                c,
                                parts.join(", ")
                            )
                        },
                    );
                    out.push(TRes {
                        r,
                        c,
                        members: tree.members,
                        files,
                        lod,
                        dens,
                    });
                    active.fetch_sub(1, Relaxed);
                    finished.fetch_add(1, Relaxed);
                    if per_tile_log {
                        eprintln!(
                            "[index] tile {},{}: {} members ({:.1}s)",
                            r,
                            c,
                            tree.members,
                            tt.elapsed().as_secs_f64()
                        );
                    }
                }
                Ok(out)
            }));
        }
        for h in hs {
            match h.join().expect("tile worker panicked") {
                Ok(v) => all.extend(v),
                Err(e) => {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            }
        }
    });
    all.sort_by_key(|t| (t.r, t.c));
    let files: u64 = all.iter().map(|t| t.files).sum();
    let tiles_written = all.len() as u64;
    let members: u64 = all.iter().map(|t| t.members).sum();
    let lod_json: Vec<String> = all
        .iter()
        .filter_map(|t| {
            t.lod.map(|d| format!("\"{},{}\": {}", t.r, t.c, d))
        })
        .collect();
    let dens_json: Vec<String> =
        all.iter().filter_map(|t| t.dens.clone()).collect();
    let t_tiles = t2.elapsed().as_secs_f64();
    eprintln!(
        "[index] {} band files ({} tiles) in {:.1}s{}; skeleton + \
         text sidecar...",
        files,
        tiles_written,
        t_tiles,
        rss_note()
    );

    // --- skeleton + full-text sidecar ---
    let t3 = Instant::now();
    let entries = floe_tiler::skel::collect_all_texts(&doc);
    let sk = floe_tiler::skel::build_skeleton(
        &doc,
        &entries,
        floe_tiler::skel::SKEL_TEXT_CAP,
    );
    let skcell = floe_oasis::write::WCell {
        name: "SKEL_TOP".to_string(),
        rects: &sk.rects,
        polys: &sk.polys,
        paths: &sk.paths,
        texts: &sk.texts,
        places: Vec::new(),
    };
    let bytes = floe_oasis::write::write_tree(&[skcell], doc.unit)
        .expect("serialize skeleton");
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
            fmt_factors(&e.factors),
            tsv_esc(&e.s)
        ));
    }
    std::fs::write(format!("{}/texts.tsv", outdir), tsv)
        .expect("write sidecar");
    let t_skel = t3.elapsed().as_secs_f64();
    eprintln!(
        "[index] skeleton {} shapes + {} labels, sidecar {} entries \
         ({:.1}s{})",
        sk.shapes,
        sk.labels,
        sidecar.len(),
        t_skel,
        rss_note()
    );
    let thinned_json = if sk.thinned.is_empty() {
        String::new()
    } else {
        format!(
            "\"texts_thinned\": [{}],\n",
            sk.thinned
                .iter()
                .map(|(l, d)| format!(
                    "{{\"layer\": {}, \"datatype\": {}}}",
                    l, d
                ))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    // per-layer stored member counts (shapes + texts, no instance
    // multiplicity) - klayout Shapes.size() over every cell
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
                "{{\"layer\": {}, \"datatype\": {}, \"name\": \"{}\", \
                 \"color\": \"{}\", \"stored_shapes\": {}}}",
                l,
                d,
                jesc(&name),
                layer_color(i),
                stored.get(&(l, d)).copied().unwrap_or(0)
            )
        })
        .collect();

    let bands_json = bands_um
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let meta = format!(
        "{{\n\
         \"version\": {},\n\
         \"src\": {{\"path\": \"{}\", \"size\": {}, \"mtime\": {}}},\n\
         \"dbu\": {},\n\
         \"top_cell\": \"{}\",\n\
         \"bbox\": [{}, {}, {}, {}],\n\
         \"grid\": {{\"nx\": {}, \"ny\": {}, \"x0\": {}, \"y0\": {}, \
         \"tile_w\": {}, \"tile_h\": {}}},\n\
         \"layers\": [\n{}\n],\n\
         \"density\": {{\"levels\": {}, \"tiles\": {{{}}}}},\n\
         \"lod\": {{\"cap\": {}, \"tiles\": {{{}}}}},\n\
         \"bands\": {{\"thresholds_um\": [{}]}},\n\
         {}\"skeleton\": {{\"file\": \"skeleton.oas\", \
         \"shapes\": {}, \"texts\": {}}},\n\
         \"texts_sidecar\": {{\"file\": \"texts.tsv\", \
         \"entries\": {}, \"members\": {}}},\n\
         \"stats\": {{\"read_s\": {:.1}, \"parse_s\": {:.1}, \
         \"tiles_s\": {:.1}, \"skel_s\": {:.1}, \
         \"read_mode\": \"rust\", \
         \"total_s\": {:.1}, \"cells\": {}, \"tile_files\": {}}}\n\
         }}\n",
        CACHE_VERSION,
        jesc(&abs_src),
        size,
        mtime,
        dbu,
        jesc(&doc.cells[doc.top].name),
        bbox.0,
        bbox.1,
        bbox.2,
        bbox.3,
        grid.nx,
        grid.ny,
        grid.x0,
        grid.y0,
        grid.tw,
        grid.th,
        layers_json.join(",\n"),
        DENSITY_LEVELS,
        dens_json.join(", "),
        LOD_SHAPE_CAP,
        lod_json.join(", "),
        bands_json,
        thinned_json,
        sk.shapes,
        sk.labels,
        sidecar.len(),
        side_members,
        t_read + t_parse,
        t_parse,
        t_tiles,
        t_skel,
        t_all.elapsed().as_secs_f64(),
        doc.cells.len(),
        tiles_written,
    );
    std::fs::write(format!("{}/meta.json", outdir), meta)
        .expect("write meta");

    // end-of-run cache summary: the closed-network hosts otherwise
    // need a manual du sweep to report the numbers back
    let mut rows: Vec<(String, u64, u64)> = (0..nbands)
        .map(|k| format!("tiles_b{}", k))
        .chain(std::iter::once("tiles_lod".to_string()))
        .map(|d| {
            let (b, n) = dir_bytes(&format!("{}/{}", outdir, d));
            (d, b, n)
        })
        .collect();
    for f in ["skeleton.oas", "texts.tsv", "meta.json"] {
        let b = std::fs::metadata(format!("{}/{}", outdir, f))
            .map(|m| m.len())
            .unwrap_or(0);
        rows.push((f.to_string(), b, 0));
    }
    let cache_bytes: u64 = rows.iter().map(|r| r.1).sum();
    eprintln!(
        "[index] cache {} (src {}, {:.2}x)",
        fmt_size(cache_bytes),
        fmt_size(size),
        cache_bytes as f64 / size.max(1) as f64
    );
    for (name, b, n) in &rows {
        if *n > 0 {
            eprintln!(
                "[index]   {:<12} {:>6}  {} files",
                name,
                fmt_size(*b),
                n
            );
        } else if !name.starts_with("tiles_") {
            eprintln!("[index]   {:<12} {:>6}", name, fmt_size(*b));
        } // band dirs that stayed empty are omitted
    }
    let waits =
        mem_waits.load(std::sync::atomic::Ordering::Relaxed);
    if let Some(hwm) = peak_rss_gb() {
        eprintln!(
            "[index] peak rss {:.1} GB, mem waits {}",
            hwm, waits
        );
    }
    eprintln!(
        "[index] done in {:.1}s -> {}",
        t_all.elapsed().as_secs_f64(),
        outdir
    );
    println!(
        "{{\n  \"cells\": {},\n  \"members\": {},\n  \
         \"band_files\": {},\n  \"tiles\": {},\n  \
         \"grid\": \"{}x{}\",\n  \
         \"src_bytes\": {},\n  \"cache_bytes\": {},\n  \
         \"read_s\": {:.3},\n  \"parse_s\": {:.3},\n  \
         \"tiles_s\": {:.3},\n  \"skel_s\": {:.3},\n  \
         \"total_s\": {:.3}\n}}",
        doc.cells.len(),
        members,
        files,
        tiles_written,
        grid.nx,
        grid.ny,
        size,
        cache_bytes,
        t_read,
        t_parse,
        t_tiles,
        t_skel,
        t_all.elapsed().as_secs_f64()
    );
}
