//! floe-index spike CLI: `floe-index scan <file.oas>` prints a JSON
//! inventory (cells, per-layer record/member counts, texts,
//! placements) plus throughput - validated against klayout's counts
//! by tools/validate_rust_scan.py.

use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 || args[1] != "scan" {
        eprintln!("usage: floe-index scan <file.oas> [jobs]");
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
