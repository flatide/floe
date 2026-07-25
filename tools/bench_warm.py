#!/usr/bin/env python3
"""Warm-query benchmark: focache-based render/clip vs cold full-load baseline.

Uses the generator manifest as ground truth (markers must survive the
cache -> clip round trip).
"""

import json
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

OAS = os.path.join(ROOT, "data/testchip_1g5.oas")
PY = sys.executable


def run(args):
    t0 = time.perf_counter()
    r = subprocess.run([PY, "-m", "flatoas"] + args, cwd=ROOT,
                       capture_output=True, text=True)
    dt = time.perf_counter() - t0
    if r.returncode != 0:
        print(r.stdout, r.stderr)
        raise SystemExit(f"FAILED: {args}")
    return dt, r.stdout


def main():
    mf = json.load(open(OAS + ".manifest.json"))
    marks = {m["name"]: m for m in mf["markers"]}
    out = os.path.join(ROOT, "data")

    print("== warm query benchmarks (each spawns a fresh process: "
          "includes python+klayout startup) ==")

    # 1. small render at dense logic area
    blk = next(b for b in mf["blocks"] if b["name"] == "BLK_1_1")
    cx = (blk["bbox_nm"][0] + blk["bbox_nm"][2]) / 2000
    cy = (blk["bbox_nm"][1] + blk["bbox_nm"][3]) / 2000
    dt, _ = run(["render", OAS,
                 "--bbox", f"{cx-50},{cy-50},{cx+50},{cy+50}",
                 "--out", f"{out}/bench_logic_100um.png", "--px", "1000"])
    print(f"render 100um logic window : {dt:6.2f}s")

    # 2. render 1mm window, metal layers only
    dt, _ = run(["render", OAS,
                 "--bbox", f"{cx-500},{cy-500},{cx+500},{cy+500}",
                 "--layers", "M2,M3,M4,M5,M6",
                 "--out", f"{out}/bench_logic_1mm.png", "--px", "1200"])
    print(f"render 1mm metal window   : {dt:6.2f}s")

    # 3. SRAM area render (array-heavy tiles)
    mk = marks["MARK_SRAM"]
    x, y = mk["x_um"], mk["y_um"]
    dt, _ = run(["render", OAS, "--bbox", f"{x-20},{y-20},{x+20},{y+20}",
                 "--out", f"{out}/bench_sram_40um.png", "--px", "1000"])
    print(f"render 40um SRAM window   : {dt:6.2f}s")

    # 4. clip 200um around center marker + verify marker text
    mk = marks["MARK_CENTER"]
    x, y = mk["x_um"], mk["y_um"]
    clip_path = f"{out}/bench_clip_center.oas"
    dt, _ = run(["clip", OAS, "--bbox", f"{x-100},{y-100},{x+100},{y+100}",
                 "--out", clip_path])
    import klayout.db as db
    ly = db.Layout()
    ly.read(clip_path)
    li = ly.layer(63, 63)
    texts = []
    it = ly.begin_shapes(ly.top_cell().cell_index(), li)
    while not it.at_end():
        if it.shape().is_text():
            texts.append(it.shape().text_string)
        it.next()
    assert texts == ["MARK_CENTER"], f"marker check failed: {texts}"
    print(f"clip 200um @center        : {dt:6.2f}s "
          f"({os.path.getsize(clip_path)/1e6:.2f} MB, marker OK)")

    # 5. layer extract: full-die single fill layer via clip (all tiles)
    bb = [v / 1000 for v in mf["die_bbox_nm"]]
    dt, _ = run(["clip", OAS, "--bbox", f"{bb[0]},{bb[1]},{bb[2]},{bb[3]}",
                 "--layers", "M6_FILL", "--out", f"{out}/bench_m6fill.oas",
                 "--max-tiles", "100000"])
    print(f"extract M6_FILL full die  : {dt:6.2f}s "
          f"({os.path.getsize(out + '/bench_m6fill.oas')/1e6:.1f} MB)")

    print(f"\nbaseline: cold full read of source = ~46s, 5.7 GB RAM")


if __name__ == "__main__":
    main()
