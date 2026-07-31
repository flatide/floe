fn main() {
    let data = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
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
