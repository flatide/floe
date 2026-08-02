//! M0 spike file generator (VFS_HIER.md §5 M0). NOT product code —
//! emits synthetic OASIS via the same floe-oasis writer the tiler/vfs
//! delta path uses, so what klayout sees here is byte-for-byte the
//! record shapes the real system will emit.
//!
//!   m0_gen pts  <outdir>          Pts materialization matrix files
//!   m0_gen gens <outdir> <gens>   gen-delta binding spike files
//!
//! pts:  full type-10 files of N = 2 / 1024 / 1025 / 100k / 1M member
//!       offsets, plus per-window rebased subset files (selection
//!       0 / 1 / >=2 per §2.3 rebase rules). pts_manifest.tsv rows:
//!       n case wx0 wy0 wx1 wy1 expected full_file sub_file
//! gens: gen 1 defines page cells + WC cells; gen >= 2 defines ONLY
//!       its WC cells (+ one new page) and references resident page
//!       names with no definition in the file — the §3.3 binding
//!       question. Sidecars: gens_index.tsv, gens_pages.tsv,
//!       gens_hier.tsv (spike-local dialect of the §3.3 fallback
//!       format; rep col `p:` carries inline points, not a row ref).

use floe_oasis::doc::{RectRec, Rep};
use floe_oasis::write::{write_tree, WCell};
use std::fs;
use std::path::Path;

const UNIT: f64 = 1000.0; // 1000 grid/um -> dbu = 1 nm

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn rect(layer: u32, x: i64, y: i64, w: i64, h: i64) -> RectRec {
    RectRec { layer, dt: 0, x, y, w, h, rep: Rep::One }
}

fn write_file(path: &Path, cells: &[WCell]) {
    let bytes = write_tree(cells, UNIT).expect("write_tree");
    fs::write(path, bytes).expect("write file");
}

// ------------------------------------------------------------- pts

// CHIP local bbox is (0,0)-(100,100): B0 of §2.3.
const B0: i64 = 100;
// planted offsets in zones no LCG point (range < 2^20) can reach
const PA: (i64, i64) = (1_400_020, 1_400_020);
const PB: (i64, i64) = (1_600_000, 1_600_000);
const PC: (i64, i64) = (1_600_200, 1_600_150);

fn make_pts(n: usize) -> Vec<(i64, i64)> {
    assert!(n >= 2);
    let mut pts = vec![(0i64, 0i64)];
    for p in [PA, PB, PC] {
        if pts.len() < n {
            pts.push(p);
        }
    }
    let mut lcg = Lcg(0x5EED_F10E);
    while pts.len() < n {
        let x = (lcg.next() % 1_048_576) as i64;
        let y = (lcg.next() % 1_048_576) as i64;
        pts.push((x, y));
    }
    pts
}

fn visible(p: (i64, i64), w: (i64, i64, i64, i64)) -> bool {
    // touching semantics, matching klayout's touching iterators
    p.0 + B0 >= w.0 && p.0 <= w.2 && p.1 + B0 >= w.1 && p.1 <= w.3
}

fn gen_pts(outdir: &Path) {
    let chip = vec![rect(1, 0, 0, 100, 100), rect(2, 45, 45, 10, 10)];
    let mut manifest = String::new();
    for &n in &[2usize, 1024, 1025, 100_000, 1_000_000] {
        let pts = make_pts(n);
        let full_name = format!("pts_full_{}.oas", n);
        {
            let rep = Rep::Pts(pts.clone());
            let cells = vec![
                WCell {
                    name: "CHIP".into(),
                    rects: &chip,
                    polys: &[],
                    paths: &[],
                    texts: &[],
                    places: vec![],
                },
                WCell {
                    name: "TOP".into(),
                    rects: &[],
                    polys: &[],
                    paths: &[],
                    texts: &[],
                    places: vec![("CHIP", 0, 0, 0, false, &rep)],
                },
            ];
            write_file(&outdir.join(&full_name), &cells);
        }
        // selection windows: (case, window). case2 for n==2 spans
        // origin..PA so both members select.
        let w2 = if n == 2 {
            (-100, -100, 1_400_140, 1_400_140)
        } else {
            (1_599_900, 1_599_800, 1_600_400, 1_600_400)
        };
        let windows = [
            (0u32, (1_200_000, 1_200_000, 1_240_000, 1_240_000)),
            (1u32, (1_399_900, 1_399_900, 1_400_140, 1_400_140)),
            (2u32, w2),
        ];
        for (case, w) in windows {
            let s: Vec<(i64, i64)> =
                pts.iter().copied().filter(|&p| visible(p, w)).collect();
            let expect = match case {
                0 => 0usize,
                1 => 1,
                _ => 2,
            };
            assert_eq!(
                s.len(),
                expect,
                "planted selection drifted: n={} case={}",
                n,
                case
            );
            let sub_name = format!("pts_sel{}_{}.oas", case, n);
            // §2.3 rebase: 0 -> omit, 1 -> Rep::One at origin+p,
            // >=2 -> origin += p0, rep = [0, p_i - p0]
            let rep_holder;
            let places: Vec<(&str, i64, i64, u8, bool, &Rep)> =
                match s.len() {
                    0 => vec![],
                    1 => {
                        rep_holder = Rep::One;
                        vec![("CHIP", s[0].0, s[0].1, 0, false, &rep_holder)]
                    }
                    _ => {
                        let o = s[0];
                        rep_holder = Rep::Pts(
                            std::iter::once((0, 0))
                                .chain(
                                    s[1..]
                                        .iter()
                                        .map(|p| (p.0 - o.0, p.1 - o.1)),
                                )
                                .collect(),
                        );
                        vec![("CHIP", o.0, o.1, 0, false, &rep_holder)]
                    }
                };
            let cells = vec![
                WCell {
                    name: "CHIP".into(),
                    rects: &chip,
                    polys: &[],
                    paths: &[],
                    texts: &[],
                    places: vec![],
                },
                WCell {
                    name: "TOP".into(),
                    rects: &[],
                    polys: &[],
                    paths: &[],
                    texts: &[],
                    places,
                },
            ];
            write_file(&outdir.join(&sub_name), &cells);
            manifest.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                n, case, w.0, w.1, w.2, w.3, expect, full_name, sub_name
            ));
        }
        eprintln!("pts n={} done", n);
    }
    fs::write(outdir.join("pts_manifest.tsv"), manifest).unwrap();
}

// ------------------------------------------------------------ gens

struct PageDef {
    name: String,
    layer: u32,
    w: i64,
    h: i64,
}

fn gen_gens(outdir: &Path, ngens: u32) {
    let base_pages = vec![
        PageDef { name: "P0_1_0".into(), layer: 1, w: 500, h: 500 },
        PageDef { name: "P0_1_1".into(), layer: 1, w: 500, h: 500 },
        PageDef { name: "P1_1_0".into(), layer: 2, w: 300, h: 300 },
        PageDef { name: "P2_1_0".into(), layer: 3, w: 200, h: 200 },
    ];
    let mut pages_tsv = String::new();
    for p in &base_pages {
        pages_tsv.push_str(&format!(
            "{}\t{}\t0\t0\t0\t{}\t{}\n",
            p.name, p.layer, p.w, p.h
        ));
    }
    let mut index_tsv = String::new();
    let mut hier_tsv = String::new();
    for g in 1..=ngens {
        let new_page = PageDef {
            name: format!("P9_1_{}", g),
            layer: 4,
            w: 50,
            h: 50,
        };
        pages_tsv.push_str(&format!(
            "{}\t{}\t0\t0\t0\t{}\t{}\n",
            new_page.name, new_page.layer, new_page.w, new_page.h
        ));
        let w1 = format!("W{}_F_1", g);
        let w0 = format!("W{}_F_0", g);
        // rect vecs per cell (kept alive for WCell borrows)
        let page_rects: Vec<Vec<RectRec>> = base_pages
            .iter()
            .map(|p| vec![rect(p.layer, 0, 0, p.w, p.h)])
            .collect();
        let newp_rects = vec![rect(new_page.layer, 0, 0, new_page.w, new_page.h)];
        let frame_rects = vec![rect(255, 0, 0, 1200, 900)];
        let grid = Rep::Grid {
            na: 3,
            nb: 2,
            va: (400, 0),
            vb: (0, 400),
        };
        let pts = Rep::Pts(vec![(0, 0), (2000, 50), (2100, 700)]);
        let one = Rep::One;
        // W{g}_F_1: identity page + shared page + Grid array page +
        // Pts page + frame rect
        let w1_places: Vec<(&str, i64, i64, u8, bool, &Rep)> = vec![
            ("P1_1_0", 0, 0, 0, false, &one),
            ("P0_1_0", 3000, 0, 0, false, &one), // shared with W_F_0
            ("P2_1_0", 400, 0, 0, false, &grid),
            ("P1_1_0", 0, 1200, 0, false, &pts),
        ];
        let w0_places: Vec<(&str, i64, i64, u8, bool, &Rep)> = vec![
            ("P0_1_0", 0, 0, 0, false, &one),
            ("P0_1_1", 2000, 0, 1, false, &one), // rot 90
            (&new_page.name, 0, 2500, 0, false, &one),
            (&w1, 5000 + 10 * g as i64, 3000, (g % 4) as u8, false, &one),
        ];
        let mut cells: Vec<WCell> = Vec::new();
        if g == 1 {
            for (i, p) in base_pages.iter().enumerate() {
                cells.push(WCell {
                    name: p.name.clone(),
                    rects: &page_rects[i],
                    polys: &[],
                    paths: &[],
                    texts: &[],
                    places: vec![],
                });
            }
        }
        cells.push(WCell {
            name: new_page.name.clone(),
            rects: &newp_rects,
            polys: &[],
            paths: &[],
            texts: &[],
            places: vec![],
        });
        cells.push(WCell {
            name: w1.clone(),
            rects: &frame_rects,
            polys: &[],
            paths: &[],
            texts: &[],
            places: w1_places,
        });
        cells.push(WCell {
            name: w0.clone(),
            rects: &[],
            polys: &[],
            paths: &[],
            texts: &[],
            places: w0_places,
        });
        let fname = format!("gen{}.oas", g);
        write_file(&outdir.join(&fname), &cells);
        index_tsv.push_str(&format!(
            "{}\t{}\t{}\t{},{}\t{}\n",
            g, fname, w0, w0, w1, new_page.name
        ));
        // hier rows: gen parent kind target x y rot flip rep
        let mut row = |parent: &str,
                       kind: &str,
                       target: &str,
                       x: i64,
                       y: i64,
                       rot: u32,
                       rep: &str| {
            hier_tsv.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t0\t{}\n",
                g, parent, kind, target, x, y, rot, rep
            ));
        };
        row(&w1, "page", "P1_1_0", 0, 0, 0, "-");
        row(&w1, "page", "P0_1_0", 3000, 0, 0, "-");
        row(&w1, "page", "P2_1_0", 400, 0, 0, "g:3:2:400:0:0:400");
        row(&w1, "page", "P1_1_0", 0, 1200, 0, "p:0:0:2000:50:2100:700");
        row(&w1, "frame", "255:0", 0, 0, 0, "r:1200:900");
        row(&w0, "page", "P0_1_0", 0, 0, 0, "-");
        row(&w0, "page", "P0_1_1", 2000, 0, 1, "-");
        row(&w0, "page", &new_page.name, 0, 2500, 0, "-");
        row(&w0, "wc", &w1, 5000 + 10 * g as i64, 3000, g % 4, "-");
    }
    fs::write(outdir.join("gens_index.tsv"), index_tsv).unwrap();
    fs::write(outdir.join("gens_pages.tsv"), pages_tsv).unwrap();
    fs::write(outdir.join("gens_hier.tsv"), hier_tsv).unwrap();
    eprintln!("gens 1..{} done", ngens);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: m0_gen pts <outdir> | m0_gen gens <outdir> <ngens>";
    match args.get(1).map(|s| s.as_str()) {
        Some("pts") => {
            let out = Path::new(args.get(2).expect(usage));
            fs::create_dir_all(out).unwrap();
            gen_pts(out);
        }
        Some("gens") => {
            let out = Path::new(args.get(2).expect(usage));
            let n: u32 = args.get(3).expect(usage).parse().expect("ngens");
            fs::create_dir_all(out).unwrap();
            gen_gens(out, n);
        }
        _ => {
            eprintln!("{}", usage);
            std::process::exit(2);
        }
    }
}
