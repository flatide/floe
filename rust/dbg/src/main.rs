fn main() {
    let args: Vec<String> = std::env::args().collect();
    // floe-dbg fixture NAME OUT: one of floe_oasis's hand-encoded streams
    // (floe_oasis::inventory::fixtures - what KLayout never writes) as a
    // file, for the gates (validate_light_cache: a cell defined inside a
    // CBLOCK)
    if args.get(1).map(String::as_str) == Some("fixture") {
        use floe_oasis::inventory::fixtures;
        let (name, out) = match (args.get(2), args.get(3)) {
            (Some(n), Some(o)) => (n.as_str(), o),
            _ => {
                eprintln!("usage: floe-dbg fixture head-cblock|adversarial|adversarial-cblock|dup-in-range|labelled-tree|cblock-cells|cblock-cells-twice|cblock-chain|cblock-chain-layered|cblock-pairs OUT");
                std::process::exit(2);
            }
        };
        let data = match name {
            "head-cblock" => fixtures::head_cblock_cell(),
            "adversarial" => fixtures::adversarial(false, false),
            "adversarial-cblock" => fixtures::adversarial(false, true),
            "dup-in-range" => fixtures::dup_in_range(),
            "labelled-tree" => fixtures::labelled_tree(),
            // 24 cells inside one CBLOCK of HOST's range (the marking
            // pass's shared range, 0.12.391)
            "cblock-cells" => fixtures::cells_in_one_cblock(24),
            // the same 24 cells defined on their own first, then again in
            // HOST's CBLOCK (the review of 0.12.391: one set of 25 cells
            // and 25 ranges, 0.12.392)
            "cblock-cells-twice" => fixtures::cells_defined_twice(24),
            // a chain of 32 cells, each one's CBLOCK defining the next with
            // 10,000 rects (the review of 0.12.392: one set, the file; the
            // markers walk it within the multi-definition span, 0.12.393)
            "cblock-chain" => fixtures::cells_chained(32, 10_000),
            // the review of 0.12.393: 63 tiny layer-1 cells, each followed by
            // a CBLOCK defining two big layer-2 cells (10,000 rects) that
            // chain - a layer-1 group must not materialize the big ones
            "cblock-chain-layered" => fixtures::cells_layered_chain(64, 10_000),
            // 64 independent pairs (a cell and one inside its CBLOCK, 2,000
            // rects): sets in progress together (0.12.394)
            "cblock-pairs" => fixtures::cells_paired(64, 2_000),
            _ => {
                eprintln!("floe-dbg fixture: unknown fixture {}", name);
                std::process::exit(2);
            }
        };
        if let Err(e) = std::fs::write(out, &data) {
            eprintln!("floe-dbg fixture: write {}: {}", out, e);
            std::process::exit(1);
        }
        println!("{}: {} bytes", out, data.len());
        return;
    }
    let data = std::fs::read(args.get(1).expect("usage: floe-dbg FILE | floe-dbg fixture NAME OUT")).unwrap();
    let doc = floe_oasis::doc::parse_doc(&data).unwrap();
    let grid = floe_tiler::Grid {
        x0: 0, y0: 0, tw: 2_700_000, th: 2_700_000, nx: 4, ny: 4,
    };
    let hier = floe_tiler::hier::HierTiler::new(&doc, grid, vec![125, 500, 2000]);
    println!("bboxes: top={:?} blk0={:?}", hier.bboxes[8], hier.bboxes[7]);
    let tree = hier.build_tile(3, 3).unwrap().unwrap();
    println!("tree cells: {}", tree.cells.len());
    for (i, vc) in tree.cells.iter().enumerate() {
        let shp: usize = vc.bands.iter()
            .map(|b| b.rects.len() + b.polys.len()).sum();
        println!(
            "  [{}] design={} ord={} shapes={} places={}",
            i, doc.cells[vc.design].name, vc.ord, shp, vc.places.len()
        );
    }
    println!("root={}", tree.root);
}
