"""Cross-validate the Rust OASIS scanner against klayout (spike S1).

For each file: run `floe-index scan`, then load the file with klayout
(viewer mode) and compare cells, per-layer stored-shape member counts,
text member counts and per-cell placement record... klayout has no
record-level API from Python, so the comparable quantities are:
  - cells
  - per-(layer,datatype) stored shape members  == Shapes.size() sums
    (geometry only; klayout counts texts in the same containers, so
    the rust text members are added to the rust side per layer pair
    when the pair also exists as a text layer)
  - total text members

usage: python tools/validate_rust_scan.py file.oas [file2.oas ...]
"""
import json
import subprocess
import sys
import time

import klayout.db as db

RUST = "rust/target/release/floe-index"


def rust_scan(path, extra=()):
    out = subprocess.check_output([RUST, "scan", path] + list(extra))
    return json.loads(out)


def check_segments(path, r):
    """The segment statistics (docs/DIRECT_READ_PLAN.ko.md L0 / 8.1,
    0.12.374): the block is there and consistent with the scan's own
    counts, and a shorter span never gives fewer spans."""
    s = r.get("segments")
    assert s, "%s: no segments block" % path
    assert s["count"] == s["cblocks"] + s["spans"], s
    assert s["cblocks"] <= r["cblocks"], (s["cblocks"], r["cblocks"])
    for key in ("class_by_count", "class_by_bytes"):
        assert set(s[key]) == {"tile", "band", "full", "other", "empty"}, s[key]
    assert sum(s["class_by_count"].values()) == s["count"], s["class_by_count"]
    assert s["chained"]["segments"] <= s["count"], s["chained"]
    assert 0 <= s["with_shapes"] <= s["count"]
    assert s["span_bytes"] == 262144, s["span_bytes"]
    cells = s["cells"]
    assert 1 <= len(cells) <= 10, len(cells)
    for c in cells:
        assert c["segments"] >= 1 and c["overlap"] >= 0.0, c
        assert sum(c["classes"].values()) == c["segments"], c
    s2 = rust_scan(path, ["--seg-bytes", "4096"])["segments"]
    assert s2["span_bytes"] == 4096, s2["span_bytes"]
    assert s2["cblocks"] == s["cblocks"], (s2["cblocks"], s["cblocks"])
    assert s2["spans"] >= s["spans"], (s2["spans"], s["spans"])
    bb = s["class_by_bytes"]
    return ("segments %d (cblock %d, span %d; 4K spans %d) by bytes tile %d band %d "
            "full %d other %d empty %d; overlap p50 %.2f (%d cells); chained %d"
            % (s["count"], s["cblocks"], s["spans"], s2["spans"], bb["tile"],
               bb["band"], bb["full"], bb["other"], bb["empty"],
               s["overlap"]["p50"], s["overlap"]["cells"], s["chained"]["segments"]))


def klayout_counts(path, recount=()):
    """size() totals per layer pair (fast). Pairs listed in `recount`
    are re-counted by full each() expansion instead: Shapes.size() has
    an off-by-one corner case on some irregular-repetition containers
    (measured: size()=17 vs each()=18 on testchip's marker layer; the
    rust scanner agrees with each())."""
    ly = db.Layout(False)
    t0 = time.perf_counter()
    ly.read(path)
    dt = time.perf_counter() - t0
    per = {}
    texts = {}
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        key = "%d/%d" % (info.layer, info.datatype)
        tot = 0
        ntxt = 0
        for c in ly.each_cell():
            sh = c.shapes(li)
            if key in recount:
                tot += sum(1 for _ in sh.each())
            else:
                tot += sh.size()
            for _ in sh.each(db.Shapes.STexts):
                ntxt += 1
        if tot:
            per[key] = tot
        if ntxt:
            texts[key] = ntxt
    cells = sum(1 for _ in ly.each_cell())
    ly._destroy()
    return {"cells": cells, "shapes": per, "texts": texts,
            "read_s": dt}


def main():
    bad = 0
    for path in sys.argv[1:]:
        r = rust_scan(path)
        seg_line = check_segments(path, r)
        k = klayout_counts(path)
        # rust: geometry members per pair + text members per pair;
        # klayout Shapes.size() counts both in one container
        combined = {}
        for key, s in r["shapes"].items():
            combined[key] = combined.get(key, 0) + s["members"]
        for key, s in r["texts"].items():
            combined[key] = combined.get(key, 0) + s["members"]
        ok_cells = r["cells"] == k["cells"]
        if combined != k["shapes"]:
            # size() corner case? re-count differing pairs exactly
            diff = [key for key in set(combined) | set(k["shapes"])
                    if combined.get(key) != k["shapes"].get(key)]
            k = klayout_counts(path, recount=set(diff))
        ok_shapes = combined == k["shapes"]
        rt = sum(s["members"] for s in r["texts"].values())
        kt = sum(k["texts"].values())
        ok_texts = rt == kt
        status = "OK " if (ok_cells and ok_shapes and ok_texts) else "FAIL"
        if status == "FAIL":
            bad += 1
        print("%s %s: cells %d/%d, layer-pairs %d/%d, texts %d/%d, "
              "rust %.1f MB/s vs klayout read %.1fs"
              % (status, path, r["cells"], k["cells"],
                 len(combined), len(k["shapes"]), rt, kt,
                 r["scan_mb_s"], k["read_s"]))
        print("    " + seg_line)
        if not ok_shapes:
            rk, kk = set(combined), set(k["shapes"])
            for key in sorted(rk - kk)[:5]:
                print("   rust-only %s: %d" % (key, combined[key]))
            for key in sorted(kk - rk)[:5]:
                print("   klayout-only %s: %d" % (key, k["shapes"][key]))
            for key in sorted(rk & kk):
                if combined[key] != k["shapes"][key]:
                    print("   %s: rust %d vs klayout %d"
                          % (key, combined[key], k["shapes"][key]))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
