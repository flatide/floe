//! floe-index spike CLI: `floe-index scan <file.oas>` prints a JSON
//! inventory (cells, per-layer record/member counts, texts,
//! placements) plus throughput - validated against klayout's counts
//! by tools/validate_rust_scan.py.

use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "tile" {
        return tile_cmd(&args[2..]);
    }
    if args.len() < 3 || args[1] != "scan" {
        eprintln!(
            "usage: floe-index scan <file.oas> [jobs]\n       \
             floe-index tile <file.oas> <outdir> \
             --grid x0,y0,tw,th,nx,ny --edges e0,e1,e2"
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
            let base: Vec<String> = tree
                .cells
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
                .collect();
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
                                (
                                    bname[p.var].as_str(),
                                    p.x,
                                    p.y,
                                    p.rot,
                                    p.flip,
                                    &p.rep,
                                )
                            })
                            .collect(),
                    })
                    .collect();
                let bytes =
                    floe_oasis::write::write_tree(&wcells, doc.unit)
                        .expect("serialize");
                std::fs::write(
                    format!("{}/tiles_b{}/t_{}_{}.oas", outdir, k, r, c),
                    bytes,
                )
                .expect("write tile");
                files += 1;
            }
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
