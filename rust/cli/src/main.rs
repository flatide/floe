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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "tile" {
        return tile_cmd(&args[2..]);
    }
    if args.len() >= 3 && args[1] == "index" {
        return index_cmd(&args[2..]);
    }
    if args.len() < 3 || args[1] != "scan" {
        eprintln!(
            "usage: floe-index scan <file.oas> [jobs]\n       \
             floe-index tile <file.oas> <outdir> \
             --grid x0,y0,tw,th,nx,ny --edges e0,e1,e2\n       \
             floe-index index <file.oas> [outdir] \
             [--tile-bytes N] [--bands um,um,um]"
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
    use floe_oasis::doc::{PolyRec, RectRec, Rep};
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
    let merged: Vec<(usize, Vec<RectRec>, Vec<PolyRec>)> = included
        .iter()
        .map(|&i| {
            let vc = &tree.cells[i];
            let mut rects = Vec::new();
            let mut polys = Vec::new();
            for band in &vc.bands {
                rects.extend(band.rects.iter().cloned());
                polys.extend(band.polys.iter().cloned());
            }
            (i, rects, polys)
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
    for (i, rects, polys) in &merged {
        wcells.push(floe_oasis::write::WCell {
            name: names[*i].clone(),
            rects,
            polys,
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

fn index_cmd(args: &[String]) {
    let mut src: Option<String> = None;
    let mut outdir: Option<String> = None;
    let mut tile_bytes = TILE_TARGET_BYTES;
    let mut bands_um: Vec<f64> = BANDS_UM.to_vec();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
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
    let data = std::fs::read(&src).expect("read src");
    let size = data.len() as u64;
    let mtime = std::fs::metadata(&src)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .expect("mtime");
    let t_read = t_all.elapsed().as_secs_f64();

    let t1 = Instant::now();
    let doc = match floe_oasis::doc::parse_doc(&data) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("parse {}: {}", src, e);
            std::process::exit(1);
        }
    };
    let t_parse = t1.elapsed().as_secs_f64();
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
    let mut files = 0u64;
    let mut tiles_written = 0u64;
    let mut members = 0u64;
    let mut lod_json: Vec<String> = Vec::new();
    let mut dens_json: Vec<String> = Vec::new();
    let t2 = Instant::now();
    for r in 0..grid.ny {
        for c in 0..grid.nx {
            let tree = match hier.build_tile(r, c) {
                Ok(Some(t)) => t,
                Ok(None) => continue,
                Err(e) => {
                    eprintln!("tile {},{}: {}", r, c, e);
                    std::process::exit(1);
                }
            };
            members += tree.members;
            tiles_written += 1;
            files += write_band_files(&doc, &tree, &outdir, r, c, nbands);
            if let Some(d) =
                write_lod_file(&doc, &tree, &outdir, r, c, LOD_SHAPE_CAP)
            {
                lod_json.push(format!("\"{},{}\": {}", r, c, d));
            }
            if let Some((arrs, cells)) = tree.density(DENSITY_LEVELS) {
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
                dens_json.push(format!(
                    "\"{},{}\": {{{}}}",
                    r,
                    c,
                    parts.join(", ")
                ));
            }
        }
    }
    let t_tiles = t2.elapsed().as_secs_f64();

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
         \"stats\": {{\"read_s\": {:.1}, \"parse_s\": {:.1}, \
         \"tiles_s\": {:.1}, \"read_mode\": \"rust\", \
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
        t_read + t_parse,
        t_parse,
        t_tiles,
        t_all.elapsed().as_secs_f64(),
        doc.cells.len(),
        tiles_written,
    );
    std::fs::write(format!("{}/meta.json", outdir), meta)
        .expect("write meta");
    println!(
        "{{\n  \"cells\": {},\n  \"members\": {},\n  \
         \"band_files\": {},\n  \"tiles\": {},\n  \
         \"grid\": \"{}x{}\",\n  \
         \"read_s\": {:.3},\n  \"parse_s\": {:.3},\n  \
         \"tiles_s\": {:.3},\n  \"total_s\": {:.3}\n}}",
        doc.cells.len(),
        members,
        files,
        tiles_written,
        grid.nx,
        grid.ny,
        t_read,
        t_parse,
        t_tiles,
        t_all.elapsed().as_secs_f64()
    );
}
