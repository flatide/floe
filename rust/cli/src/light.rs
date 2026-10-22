//! `floe-index light SRC [--out DIR] [--jobs N] [--seg-bytes N] [--occupancy
//! [--occupancy-um UM] ...] [--occupancy-only] [--mem GiB] [--force]` - the
//! light cache (docs/DIRECT_READ_PLAN.ko.md §5.1 / §5.6, L4-L5): one
//! inventory pass over a read-only mapping of the source (the streaming
//! build's, vfs.rs `stream_open`) writes design.inv (floe_oasis::invfile),
//! the segment directory design.seg (floe_oasis::segfile) paired with it,
//! and meta.json - the full index's keys and `"cache": "light"`; meta.json
//! is the commit marker (written last) - into DIR (default `.<source>.ice`
//! beside the source, as the full index). `--occupancy` adds the wide
//! views' summary design.ovo from the streaming occupancy pass (S5): the
//! walk's instances come from the placement-flagged segments of the
//! directory just written (floe_vfs::light), not from a pass over the whole
//! file. `--occupancy-only` adds design.ovo to a light cache that has none.
//! The renderer opens such a directory - no design.ovm, design.inv beside -
//! through floe_vfs::light and reads the geometry from the source.
//!
//! Locks (floe_vfs::lock, SPEC-INDEXER §4.5): a build takes the cache's
//! locks as `floe-index vfs` does - the readers out until meta.json is
//! written -, `--occupancy-only` holds other writers out, and the readers
//! too when it replaces a design.ovo they map. A directory holding a full
//! index (design.ovm) is left alone unless `--force` (which removes the
//! full index's files: the light cache replaces it). A jobdeck is handed to
//! `floe2 index DECK --light` (main.rs).

use std::path::Path;

pub fn light_cmd(args: &[String]) {
    let mut src: Option<String> = None;
    let mut out: Option<String> = None;
    let mut jobs: Option<usize> = None;
    let mut seg_bytes = floe_oasis::segments::DEFAULT_SPAN_BYTES;
    let mut occupancy = false;
    let mut occupancy_only = false;
    let mut force = false;
    let mut mem_gib: Option<f64> = None;
    let mut hold_at: Option<String> = None;
    let mut occ_opts = floe_vfs::occupancy::Opts::default();
    occ_opts.base_um = floe_vfs::occupancy::BASE_AUTO;
    let mut i = 0;
    let need = |i: usize, what: &str| -> String {
        args.get(i + 1).cloned().unwrap_or_else(|| {
            eprintln!("{} needs {}", args[i], what);
            std::process::exit(2);
        })
    };
    while i < args.len() {
        match args[i].as_str() {
            "--jobs" => {
                jobs = Some(need(i, "a number").parse().unwrap_or_else(|_| {
                    eprintln!("--jobs needs a number");
                    std::process::exit(2);
                }));
                i += 1;
            }
            "--seg-bytes" => {
                seg_bytes = crate::parse_bytes(&need(i, "a byte count"))
                    .filter(|&v| v > 0)
                    .unwrap_or_else(|| {
                        eprintln!("--seg-bytes needs a byte count (e.g. 256K)");
                        std::process::exit(2);
                    });
                i += 1;
            }
            "--out" => {
                out = Some(need(i, "a directory"));
                i += 1;
            }
            "--mem" => {
                let g: f64 = need(i, "GiB").parse().unwrap_or(f64::NAN);
                if !(g > 0.0) || !g.is_finite() {
                    eprintln!("--mem needs a positive number of GiB");
                    std::process::exit(2);
                }
                mem_gib = Some(g);
                i += 1;
            }
            "--occupancy" => occupancy = true,
            "--occupancy-only" => occupancy_only = true,
            "--occupancy-um" => {
                let um: f64 = need(i, "microns").parse().unwrap_or(f64::NAN);
                if !(um > 0.0) || !um.is_finite() {
                    eprintln!("--occupancy-um must be a positive number of microns");
                    std::process::exit(2);
                }
                // (the base cell alone, as `vfs`: the app passes --occupancy
                // or --occupancy-only beside it)
                occ_opts.base_um = um;
                i += 1;
            }
            "--occupancy-max-cells" => {
                occ_opts.max_cells = need(i, "a number").parse().expect("occupancy max cells");
                i += 1;
            }
            "--occupancy-max-work" => {
                occ_opts.max_work = need(i, "a number").parse().expect("occupancy max work");
                i += 1;
            }
            "--occupancy-max-bytes" => {
                occ_opts.max_bytes = need(i, "a number").parse().expect("occupancy max bytes");
                i += 1;
            }
            "--occupancy-balance" => {
                occ_opts.balanced_units = need(i, "0|1") != "0";
                i += 1;
            }
            "--occupancy-prune" => {
                occ_opts.prune = need(i, "0|1") != "0";
                i += 1;
            }
            "--force" => force = true,
            "--hold-at" => {
                hold_at = Some(need(i, "point:file"));
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
            "usage: floe-index light <file.oas> [--out DIR] [--jobs N] [--seg-bytes N] \
             [--occupancy [--occupancy-um UM] [--occupancy-balance 0|1] [--occupancy-prune 0|1]] \
             [--occupancy-only] [--mem GiB] [--force]  \
             (the light cache: design.inv + design.seg + meta.json from one inventory pass, \
             design.ovo from the streaming occupancy pass with --occupancy; DIR defaults to \
             .<file.oas>.ice beside the source; docs/DIRECT_READ_PLAN.ko.md L4-L5)"
        );
        std::process::exit(2);
    };
    if occupancy_only && force {
        eprintln!("light: --occupancy-only adds design.ovo to a light cache; not with --force");
        std::process::exit(2);
    }
    if occupancy_only {
        // (the summary is what it makes: --occupancy beside it changes nothing)
        occupancy = false;
    }
    let jobs = jobs.unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
    let outdir = out.unwrap_or_else(|| crate::vfs::default_outdir(&src));
    let t0 = std::time::Instant::now();
    occ_opts.jobs = jobs;
    occ_opts.progress = Some(|m: &str| eprintln!("[vfs] occupancy {}", m));
    // ---- the cache's locks, before the source is read (SPEC-INDEXER §4.5)
    if let Some(parent) = Path::new(&outdir).parent().filter(|p| !p.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(parent);
    }
    let (mode, what) = if occupancy_only {
        (floe_vfs::lock::Mode::Additive, "floe-index light --occupancy-only")
    } else {
        (floe_vfs::lock::Mode::Full, "floe-index light")
    };
    let mut lock = crate::vfs::lock_writer(floe_vfs::lock::Kind::Vfs, &outdir, mode, what);
    let here = |name: &str| Path::new(&outdir).join(name).exists();
    if occupancy_only && here("design.ovo") {
        // replacing a file the readers map holds them all out
        crate::vfs::exclude_readers_or_exit(&mut lock, floe_vfs::lock::Kind::Vfs, &outdir);
    }
    if let Some(spec) = hold_at.as_deref() {
        crate::vfs::gate_hold(spec, "locked");
    }
    // ---- what is there
    let full_here = here("design.ovm");
    if occupancy_only {
        if full_here || !here(floe_oasis::invfile::FILE_NAME) {
            eprintln!(
                "light --occupancy-only: {} holds {} - it adds design.ovo to a light cache (a full index takes `floe-index vfs SRC DIR --occupancy-only`)",
                outdir,
                if full_here { "a full index" } else { "no light cache" }
            );
            std::process::exit(1);
        }
        match floe_vfs::light::check(&outdir) {
            Ok(files) => {
                let (size, mtime) = crate::vfs::source_identity(&src);
                if (files.identity.src_size, files.identity.src_mtime) != (size, mtime) {
                    eprintln!(
                        "light --occupancy-only: {} was built for source {}/{} but the source is now {}/{}; rebuild the light cache",
                        outdir, files.identity.src_size, files.identity.src_mtime, size, mtime
                    );
                    std::process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("light --occupancy-only: {}", e);
                std::process::exit(1);
            }
        }
    } else if full_here && !force {
        eprintln!(
            "light: {} holds a full index (design.ovm); --force replaces it with the light cache (the streaming build leaves design.inv / design.seg beside a full index on its own)",
            outdir
        );
        std::process::exit(1);
    }
    // ---- the inventory pass over the mapped source (the streaming build's)
    let a = crate::vfs::StreamArgs {
        src: src.clone(),
        outdir: outdir.clone(),
        jobs,
        encode_batch: 1,
        plan_batch: 1,
        page_target_bytes: 1 << 20,
        lod: false,
        page_occ: false,
        hier: false,
        ovs: false,
        occupancy: occupancy || occupancy_only,
        slow_cell_s: 0.0,
        p2_shard_limit: None,
        kill_at: None,
        hold_at: hold_at.clone(),
        mem_gib,
        seg: true,
        seg_bytes,
        occ_opts: &occ_opts,
        auto: false,
    };
    let crate::vfs::StreamOpen {
        mut store,
        budget,
        size,
        mtime,
        inventory_s,
        segs,
    } = match crate::vfs::stream_open(&a) {
        Ok(o) => o,
        Err(d) => {
            eprintln!("[light] {}", d.why);
            std::process::exit(d.exit);
        }
    };
    let id = floe_oasis::invfile::Identity {
        src_size: size,
        src_mtime: mtime,
    };
    let publish = |name: &str, bytes: &[u8]| {
        let path = Path::new(&outdir).join(name);
        let tmp = Path::new(&outdir).join(format!("{}.tmp", name));
        let done = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, &path));
        if let Err(e) = done {
            eprintln!("light: write {}: {}", path.display(), e);
            std::process::exit(1);
        }
    };
    // a stream with a cell defined inside a CBLOCK: a segment then holds two
    // cells' records and the directory cannot tell them apart - the renderer
    // refuses such a cache (floe_vfs::light::LightIndex::open), so none is
    // written; `floe2 index` builds the full index in its place when the
    // light cache was the rule's choice (UNSUITABLE_EXIT)
    if !occupancy_only {
        let inside = segs
            .as_ref()
            .map_or(0, |c| c.segs.iter().filter(|s| s.cells_inside > 0).count());
        let cblock_cells = store.inventory().cells.iter().filter(|c| c.cblock_defs > 0).count();
        if inside > 0 || cblock_cells > 0 {
            eprintln!(
                "light: {} cell(s) of {} are defined inside CBLOCKs ({} segment(s) hold another cell's records) - the light cache cannot read them, nothing is written; the full index reads the stream (`floe2 index SRC --full`, `floe-index vfs SRC DIR`)",
                cblock_cells, src, inside
            );
            std::process::exit(floe_vfs::light::UNSUITABLE_EXIT);
        }
    }
    let mut written = String::new();
    if !occupancy_only {
        std::fs::create_dir_all(&outdir).unwrap_or_else(|e| {
            eprintln!("light: create {}: {}", outdir, e);
            std::process::exit(1);
        });
        crate::guard::check("inventory", crate::guard::CACHE_UNCHANGED);
        // the light cache's files anew (and, under --force, a full index's)
        clear_light_outputs(&outdir, force && full_here);
        let inv = store.inventory();
        let Some(col) = segs.as_ref() else {
            eprintln!("light: no segment directory was collected");
            std::process::exit(1);
        };
        let inv_bytes = floe_oasis::invfile::encode(inv, id);
        publish(floe_oasis::invfile::FILE_NAME, &inv_bytes);
        let mut f = floe_oasis::segfile::SegFile::from_segments(&col.segs, inv, seg_bytes).unwrap_or_else(|e| {
            eprintln!("light: segment directory: {}", e);
            std::process::exit(1);
        });
        f.inv_crc = floe_oasis::segfile::inv_crc_of(&inv_bytes).unwrap_or(0);
        let seg_out = floe_oasis::segfile::encode(&f, id);
        publish(floe_oasis::segfile::FILE_NAME, &seg_out);
        let meta = light_meta(&src, size, mtime, inv);
        // meta.json last: the light cache's commit marker
        publish("meta.json", meta.as_bytes());
        let cblocks = f.segs.iter().filter(|s| s.is_cblock()).count();
        written = format!(
            "design.inv ({}), design.seg ({}; {} segments: {} CBLOCK, {} span of {}; {} layer rows), meta.json",
            crate::vfs::fmt_size(inv_bytes.len() as u64),
            crate::vfs::fmt_size(seg_out.len() as u64),
            f.segs.len(),
            cblocks,
            f.segs.len() - cblocks,
            crate::vfs::fmt_size(seg_bytes),
            f.layers.len()
        );
        eprintln!(
            "[light] {}: {} cells, {} layers, {} records; inventory {:.1}s (jobs {}); {} -> {}",
            src,
            inv.cells.len(),
            inv.layer_order.len(),
            inv.scan.records,
            inventory_s,
            jobs,
            written,
            outdir
        );
        // the cache is complete: readers may open it while the summary is made
        lock.release_use();
        if let Some(spec) = hold_at.as_deref() {
            crate::vfs::gate_hold(spec, "committed");
        }
    }
    // ---- the summary (design.ovo): the streaming occupancy pass over the
    // store, its instances from the directory's placement segments
    let mut occupancy_s = 0.0;
    if occupancy || occupancy_only {
        let t_occ = std::time::Instant::now();
        let placements = store.inventory().scan.placements;
        let cap = crate::vfs::light_places_cap(budget);
        if placements > cap {
            eprintln!(
                "[light] {} placement records exceed the {} the summary pass keeps within its budget (a quarter of {} at about {} B each) - design.ovo is skipped, the light cache is complete without it; --mem raises the budget (STREAMING_INDEX_PLAN.ko.md §4.6)",
                placements,
                cap,
                crate::vfs::fmt_size(budget),
                crate::vfs::PLACE_BYTES_EST
            );
        } else {
            let li = floe_vfs::light::LightIndex::open(&outdir).unwrap_or_else(|e| {
                eprintln!("light: the directory just written does not open: {}", e);
                std::process::exit(1);
            });
            let places = li.all_placements(jobs).unwrap_or_else(|e| {
                eprintln!("light: placements from the segment directory: {}", e);
                std::process::exit(1);
            });
            let n_places: u64 = places.iter().map(|p| p.len() as u64).sum();
            eprintln!(
                "[light] placements of {} cells from the segment directory: {} records in {:.1}s",
                places.len(),
                n_places,
                t_occ.elapsed().as_secs_f64()
            );
            drop(li);
            store.set_places(places);
            crate::vfs::stream_occupancy(&mut store, &outdir, size, mtime, &occ_opts, jobs, budget, true, None);
        }
        occupancy_s = t_occ.elapsed().as_secs_f64();
    }
    eprintln!(
        "[light] done in {:.1}s -> {} (inventory {:.1}s{}{})",
        t0.elapsed().as_secs_f64(),
        outdir,
        inventory_s,
        if occupancy || occupancy_only {
            format!(", occupancy {:.1}s", occupancy_s)
        } else {
            String::new()
        },
        if occupancy_only { "; occupancy-only" } else { "" }
    );
}

/// The light cache's files removed before a build writes them anew; with
/// `full_too` (`--force` over a full index) the full index's files as well
/// (vfs.rs clear_outputs), so what is left is one light cache.
fn clear_light_outputs(outdir: &str, full_too: bool) {
    if full_too {
        crate::vfs::clear_outputs(outdir);
        return;
    }
    for f in [
        "design.inv",
        "design.inv.tmp",
        "design.seg",
        "design.seg.tmp",
        "design.ovo",
        "design.ovo.tmp",
        "meta.json",
        "meta.json.tmp",
    ] {
        let _ = std::fs::remove_file(format!("{}/{}", outdir, f));
    }
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
    // stored shapes per layer = the shape members (the full index's paged
    // members) + the texts on the layer: the inventory counts a cell's texts,
    // not its texts per layer, so a layer gets the texts of every cell whose
    // text mask holds it - exact when a cell's texts lie on one layer, an
    // upper bound otherwise. What matters is that a text-only layer counts
    // as held (the viewer's layer list and the palette's places are the full
    // index's, so a layout looks the same under either cache).
    let mut stored: std::collections::HashMap<(u32, u32), u64> = std::collections::HashMap::new();
    for (i, &(l, d)) in inv.layer_order.iter().enumerate() {
        let texts: u64 = inv
            .cells
            .iter()
            .filter(|c| c.texts > 0 && c.dtmask.get(i / 8).is_some_and(|b| (b >> (i % 8)) & 1 == 1))
            .map(|c| c.texts)
            .sum();
        let n = inv.lmems[i] + texts;
        if n > 0 {
            stored.insert((l, d), n);
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
        crate::jesc(inv.cells.get(top).map(|c| c.name.as_str()).unwrap_or("")),
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

#[cfg(test)]
mod tests {
    /// the summary pass's placement cap follows the streaming budget: a
    /// quarter of it at about PLACE_BYTES_EST each (the field host's 32 GiB
    /// default keeps about 90 M; the fixed 50 M before 0.12.385)
    #[test]
    fn the_summary_pass_keeps_a_quarter_of_the_budget_in_placements() {
        use crate::vfs::{light_places_cap, PLACE_BYTES_EST};
        assert_eq!(light_places_cap(32 << 30), (8u64 << 30) / PLACE_BYTES_EST);
        assert!(light_places_cap(32 << 30) > 80_000_000);
        assert!(light_places_cap(1 << 30) < 3_000_000);
        assert_eq!(light_places_cap(0), 1);
    }
}
