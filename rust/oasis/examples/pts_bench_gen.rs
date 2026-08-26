//! Throwaway F2R-03b 2a fixture helper (not product code).
//!
//!   pts_bench_gen gen <out.oas>   one cell, one 2x2um rect with a
//!                                 row-major-coherent 200k-point Pts rep
//!   pts_bench_gen dump <in.oas>   print the first points of every Pts
//!                                 rep so file order can be inspected

use floe_oasis::doc::{parse_doc, RectRec, Rep};
use floe_oasis::write::{write_tree, WCell};
use std::sync::Arc;

const UNIT: f64 = 1000.0; // dbu = 1 nm

fn main() {
    let mode = std::env::args().nth(1).expect("mode: gen|dump");
    let path = std::env::args().nth(2).expect("path");
    match mode.as_str() {
        "gen" => {
            let mut state = 42u64;
            let mut next = move || {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                state >> 33
            };
            let mut points: Vec<(i64, i64)> = (0..200_000)
                .map(|_| {
                    (
                        (next() % 2_000_000) as i64,
                        (next() % 2_000_000) as i64,
                    )
                })
                .collect();
            // Row-major file order: the shape real fill tools write.
            points.sort_by_key(|&(x, y)| (y / 50_000, x));
            let rep = Rep::Pts(Arc::from(points));
            let rects = [RectRec {
                layer: 1,
                dt: 0,
                x: 0,
                y: 0,
                w: 2_000,
                h: 2_000,
                rep,
            }];
            let cells = [WCell {
                name: "PTSFILL".into(),
                rects: &rects,
                polys: &[],
                paths: &[],
                texts: &[],
                places: vec![],
            }];
            let bytes = write_tree(&cells, UNIT).expect("write_tree");
            std::fs::write(&path, bytes).expect("write file");
            println!("wrote {path}");
        }
        "dump" => {
            let data = std::fs::read(&path).expect("read");
            let doc = parse_doc(&data).expect("parse");
            for cell in &doc.cells {
                for rect in &cell.rects {
                    if let Rep::Pts(points) = &rect.rep {
                        let head: Vec<_> = points.iter().take(6).collect();
                        println!(
                            "cell={} pts={} head={:?}",
                            cell.name,
                            points.len(),
                            head
                        );
                    }
                }
            }
        }
        other => panic!("unknown mode {other}"),
    }
}
