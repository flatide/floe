//! Synthetic occupancy scaling probe, without source parsing or page indexing.
//! cargo run --release -p floe-vfs --example bench_occupancy -- mixed 12 /tmp/mixed.ovo
use floe_oasis::doc::{Cell, Doc, PlaceRec, RectRec, Rep};
use floe_vfs::occupancy::{build, write_ovo, Opts};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let case = args.get(1).map(String::as_str).unwrap_or("mixed");
    let jobs = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(1);
    let (layers, shapes, members) = match case {
        "mixed" => (64, 256, 1024),
        "dense" => (1, 8192, 8192),
        _ => panic!("expected mixed or dense"),
    };
    let mut leaf = Cell { name: "LEAF".into(), ..Cell::default() };
    for k in 0..shapes {
        for layer in 1..=layers {
            leaf.rects.push(RectRec {
                layer, dt: 0, x: (k % 64) * 10, y: (k / 64) * 10,
                w: 1, h: 200, rep: Rep::One,
            });
        }
    }
    let mut top = Cell { name: "TOP".into(), ..Cell::default() };
    top.places.push(PlaceRec {
        cell: 1, x: 0, y: 0, rot: 0, flip: false,
        rep: Rep::Grid { na: members, nb: 1, va: (1, 0), vb: (0, 0) },
    });
    let doc = Doc {
        unit: 1000.0, cells: vec![top, leaf], top: 0,
        layer_order: (1..=layers).map(|l| (l, 0)).collect(), norm_s: 0.0,
        layer_names: Default::default(), layer_aliases: Default::default(),
    };
    let start = std::time::Instant::now();
    let occ = build(&doc, 0, 0, &Opts { jobs, base_um: 4.0, ..Opts::default() }).unwrap();
    println!("case={case} jobs={jobs} seconds={:.6} work={} cells={}",
        start.elapsed().as_secs_f64(), occ.layers.iter().map(|l| l.work).sum::<u64>(),
        occ.layers.iter().flat_map(|l| &l.planes).map(|p| p.levels[0].count()).sum::<u64>());
    if let Some(path) = args.get(3) {
        std::fs::write(path, write_ovo(&occ)).unwrap();
    }
}
