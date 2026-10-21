//! `floe-index light SRC [--out DIR] [--jobs N] [--seg-bytes N]` - the light
//! cache's files (docs/DIRECT_READ_PLAN.ko.md §5.1, L4): one inventory pass
//! over the source writes design.inv (floe_oasis::invfile), the segment
//! directory design.seg (floe_oasis::segfile) paired with it, and a
//! meta.json with the viewer's keys and `"cache": "light"`, into DIR
//! (default: `.<source>.ice` beside the source, as the full index). The
//! renderer opens such a directory - no design.ovm, design.inv beside -
//! through floe_vfs::light and reads the geometry from the source. The
//! occupancy summary (design.ovo), the lock protocol and the product's
//! `--light` modes are L5's; a directory holding a full index is left alone.

use std::path::Path;

pub fn light_cmd(args: &[String]) {
    let mut src: Option<String> = None;
    let mut out: Option<String> = None;
    let mut jobs = 1usize;
    let mut seg_bytes = floe_oasis::segments::DEFAULT_SPAN_BYTES;
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
            "--seg-bytes" => {
                seg_bytes = args
                    .get(i + 1)
                    .and_then(|v| crate::parse_bytes(v))
                    .filter(|&v| v > 0)
                    .unwrap_or_else(|| {
                        eprintln!("--seg-bytes needs a byte count (e.g. 256K)");
                        std::process::exit(2);
                    });
                i += 1;
            }
            "--out" => {
                out = Some(args.get(i + 1).cloned().unwrap_or_else(|| {
                    eprintln!("--out needs a directory");
                    std::process::exit(2);
                }));
                i += 1;
            }
            a if a.starts_with("--") => {
                eprintln!("light: unknown option {}", a);
                std::process::exit(2);
            }
            a => {
                if src.is_some() {
                    eprintln!("light: one source file");
                    std::process::exit(2);
                }
                src = Some(a.to_string());
            }
        }
        i += 1;
    }
    let Some(src) = src else {
        eprintln!(
            "usage: floe-index light <file.oas> [--out DIR] [--jobs N] [--seg-bytes N]  \
             (the light cache: design.inv + design.seg + meta.json from one inventory pass; \
             DIR defaults to .<file.oas>.ice beside the source; docs/DIRECT_READ_PLAN.ko.md L4)"
        );
        std::process::exit(2);
    };
    let outdir = out.unwrap_or_else(|| crate::vfs::default_outdir(&src));
    if Path::new(&outdir).join("design.ovm").exists() {
        eprintln!(
            "light: {} holds a full index (design.ovm); its light files are the streaming build's - nothing written",
            outdir
        );
        std::process::exit(1);
    }
    let t0 = std::time::Instant::now();
    let (size, mtime) = crate::vfs::source_identity(&src);
    let data = std::fs::read(&src).unwrap_or_else(|e| {
        eprintln!("read {}: {}", src, e);
        std::process::exit(1);
    });
    let read_s = t0.elapsed().as_secs_f64();
    let id = floe_oasis::invfile::Identity {
        src_size: size,
        src_mtime: mtime,
    };
    let w = floe_vfs::light::write_files(&data, id, &outdir, jobs, seg_bytes).unwrap_or_else(|e| {
        eprintln!("light: {}: {}", src, e);
        std::process::exit(1);
    });
    let files_s = t0.elapsed().as_secs_f64() - read_s;
    if w.cells_inside > 0 {
        eprintln!(
            "light: {} segment(s) hold a CELL record inside a CBLOCK - the renderer will refuse the light cache (use the full index)",
            w.cells_inside
        );
    }
    // meta.json from the inventory read back (the viewer's keys; the pass
    // above wrote the index files)
    let (_, inv) = floe_oasis::invfile::read(Path::new(&outdir).join(floe_oasis::invfile::FILE_NAME).as_path())
        .unwrap_or_else(|e| {
            eprintln!("light: read back design.inv: {}", e);
            std::process::exit(1);
        });
    drop(data);
    let publish = |name: &str, bytes: &[u8]| -> std::io::Result<()> {
        let path = Path::new(&outdir).join(name);
        let tmp = Path::new(&outdir).join(format!("{}.tmp", name));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)
    };
    let meta = light_meta(&src, size, mtime, &inv);
    publish("meta.json", meta.as_bytes()).unwrap_or_else(|e| {
        eprintln!("light: write meta.json: {}", e);
        std::process::exit(1);
    });
    eprintln!(
        "[light] {}: {} cells, {} layers, {} records; read {:.1}s, inventory + files {:.1}s (jobs {})",
        src, w.cells, w.layers, w.records, read_s, files_s, jobs
    );
    eprintln!(
        "[light] {}: design.inv ({}), design.seg ({}; {} segments: {} CBLOCK, {} span of {}; {} layer rows), meta.json in {:.1}s",
        outdir,
        crate::vfs::fmt_size(w.inv_bytes as u64),
        crate::vfs::fmt_size(w.seg_bytes as u64),
        w.segments,
        w.cblocks,
        w.segments - w.cblocks,
        crate::vfs::fmt_size(seg_bytes),
        w.layer_rows,
        t0.elapsed().as_secs_f64()
    );
}

/// The light cache's meta.json: the full index's keys (cli emit_viewer_side)
/// from the inventory - no text index, no minimap frontier - and
/// `"cache": "light"`.
fn light_meta(src: &str, size: u64, mtime: u64, inv: &floe_oasis::inventory::Inventory) -> String {
    let top = inv.top;
    let bbox = inv.rbb.get(top).copied().flatten().unwrap_or((0, 0, 0, 0));
    let dbu = 1.0 / inv.unit();
    let n = {
        let t = (size as f64 / 6.0e6).sqrt().round_ties_even() as i64;
        t.clamp(4, 96)
    };
    let tw = ((bbox.2 - bbox.0) as f64 / n as f64).ceil() as i64;
    let th = ((bbox.3 - bbox.1) as f64 / n as f64).ceil() as i64;
    let mut stored: std::collections::HashMap<(u32, u32), u64> = std::collections::HashMap::new();
    for (i, &(l, d)) in inv.layer_order.iter().enumerate() {
        if inv.lmems[i] > 0 {
            stored.insert((l, d), inv.lmems[i]);
        }
    }
    let places = crate::palette_places(&inv.layer_order, &stored);
    let layers_json: Vec<String> = inv
        .layer_order
        .iter()
        .map(|&(l, d)| {
            let name = inv
                .layer_names
                .get(&(l, d))
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{}/{}", l, d));
            let aliases = inv
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
                "{{\"layer\": {}, \"datatype\": {}, \"name\": \"{}\", \"aliases\": [{}], \"color\": \"{}\", \"stored_shapes\": {}}}",
                l,
                d,
                crate::jesc(&name),
                aliases,
                crate::layer_color(places[&(l, d)]),
                stored.get(&(l, d)).copied().unwrap_or(0)
            )
        })
        .collect();
    let abs_src = if Path::new(src).is_absolute() {
        src.to_string()
    } else {
        std::env::current_dir()
            .expect("cwd")
            .join(src)
            .to_string_lossy()
            .into_owned()
    };
    let texts: u64 = inv.cells.iter().map(|c| c.texts).sum();
    let text_cells = inv.cells.iter().filter(|c| c.texts > 0).count();
    format!(
        "{{\n\
         \"version\": {},\n\
         \"vfs\": 1,\n\
         \"cache\": \"light\",\n\
         \"src\": {{\"path\": \"{}\", \"size\": {}, \"mtime\": {}}},\n\
         \"dbu\": {},\n\
         \"top_cell\": \"{}\",\n\
         \"bbox\": [{}, {}, {}, {}],\n\
         \"grid\": {{\"nx\": {}, \"ny\": {}, \"x0\": {}, \"y0\": {}, \"tile_w\": {}, \"tile_h\": {}}},\n\
         \"layers\": [\n{}\n],\n\
         \"texts\": {{\"records\": {}, \"members\": {}, \"cells\": {}, \"grid_reps\": 0, \"pts_reps\": 0, \"ovt_bytes\": 0}},\n\
         \"frontier\": {{\"depths\": []}}\n\
         }}\n",
        crate::CACHE_VERSION,
        crate::jesc(&abs_src),
        size,
        mtime,
        dbu,
        crate::jesc(&inv.cells.get(top).map(|c| c.name.as_str()).unwrap_or("")),
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
        texts,
        texts,
        text_cells,
    )
}
