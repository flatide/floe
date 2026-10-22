#!/usr/bin/env python3
"""L4 of the direct-read plan (docs/DIRECT_READ_PLAN.ko.md §8.5): a light
cache - design.inv + design.seg beside the source, no design.ovm - renders
through the same planner and raster as the full index, pixel for pixel
where the full index draws every shape.

For the battery fixture (valmini), its KLayout rewrites (no CBLOCKs;
strict mode) and two generated files (tools/gen_route_chip.py --scale
0.05, tools/gen_main01_like.py --scale 0.001):
  - `floe-index light SRC --out LIGHT --occupancy` writes design.inv and
    design.seg that are byte for byte the streaming build's (`floe-index
    vfs` leaves them beside the full cache), a design.ovo byte for byte
    the streaming occupancy pass's (its instances from the segment
    directory's placements), and a meta.json with the full index's keys
    (dbu, top_cell, bbox, layers) and `"cache": "light"`; `--occupancy-only`
    adds the same summary to a light cache (refused on a full index or
    where no light cache is); `--force` replaces a full index;
  - floe-render-cli renders the full cache and the light directory at
    exact detail (cut 0): the whole extent, a near window, the near window
    at depth 1 with hierarchy frames, two layers alone - identical PNGs
    and the same working cells, instance edges and frames in the plan; a cut
    view (3 px) differs in at
    most 0.5 % of the lit pixels (the page partitions differ, so the page
    culls may; DIRECT_READ_PLAN §5.8);
  - the refusals: `floe-index light` into a directory holding a full
    index (exit 1), a light directory whose source changed since (the
    renderer refuses it by size and mtime), one that names no source.

usage: python tools/validate_light_cache.py valmini.oas
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "rust" / "target" / "release" / "floe-index"
RENDER = ROOT / "rust" / "target" / "release" / "floe-render-cli"
CUT_TOLERANCE = 0.005


def fail(msg):
    print("FAIL: %s" % msg)
    sys.exit(1)


def rewrite_with_klayout(src, out, cblocks, strict):
    import klayout.db as db
    ly = db.Layout()
    ly.read(str(src))
    opts = db.SaveLayoutOptions()
    opts.format = "OASIS"
    opts.oasis_write_cblocks = cblocks
    opts.oasis_strict_mode = strict
    ly.write(str(out), opts)
    ly._destroy()


def generate(script, out, *args):
    cmd = [sys.executable, "-B", str(ROOT / "tools" / script), str(out)] + list(args)
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    if r.returncode != 0 or not Path(out).is_file():
        fail("%s: %s\n%s" % (script, r.stdout[-800:], r.stderr[-1200:]))


def run(cmd, ok=(0,), what="", env=None):
    r = subprocess.run([str(c) for c in cmd], capture_output=True, text=True, env=env)
    if r.returncode not in ok:
        fail("%s: exit %d: %s" % (what or cmd[1], r.returncode, (r.stderr or r.stdout).strip()[-600:]))
    return r


def check_summary_budget(work, src):
    """The summary pass's placement cap follows --mem (a quarter at about
    96 B each; the fixed 50 M before 0.12.385): main01_like 0.001 has 1.29 M
    placements and a 62 MB top cell, so --mem 0.1 (107 MB: above the cell,
    a cap of about 280 K) skips design.ovo with the line that says so, while
    the default budget builds it (check_fixture)."""
    out = work / "main01_light_budget"
    r = run([BIN, "light", src, "--out", out, "--jobs", "2", "--occupancy", "--mem", "0.1"],
            what="floe-index light --occupancy --mem 0.1")
    if "placement records exceed" not in r.stderr or "--mem raises" not in r.stderr:
        fail("the budget's cap did not say so: %s" % r.stderr[-600:])
    if (out / "design.ovo").exists():
        fail("design.ovo was built past the budget's cap")
    if not (out / "design.inv").is_file() or not (out / "meta.json").is_file():
        fail("the light cache was not written under the cap")
    print("  summary pass: --mem 0.1 keeps too few placements for main01_like, skipped and said")


def check_unsuitable(work):
    """A stream with a cell defined inside a CBLOCK (floe_oasis's fixture
    head_cblock_cell, written by floe-dbg): `floe-index light` refuses it
    with exit 79 and writes nothing; `floe2 index` builds the full index in
    its place when the light cache was the rule's choice (FLOE_INDEX_LIGHT=on
    here, a jobdeck's source in the field) and ends with 79 under `--light`."""
    import json
    import os
    import shutil
    dbg = ROOT / "rust" / "target" / "release" / "floe-dbg"
    floe2 = ROOT / "rust" / "target" / "release" / "floe2"
    src = work / "head_cblock.oas"
    run([dbg, "fixture", "head-cblock", src], what="floe-dbg fixture")
    out = work / "head_light"
    r = run([BIN, "light", src, "--out", out, "--jobs", "1"], ok=(79,),
            what="floe-index light on a cell inside a CBLOCK")
    if "defined inside CBLOCKs" not in r.stderr:
        fail("the light refusal did not say why: %s" % r.stderr[-400:])
    if out.exists() and any(out.iterdir()):
        fail("the light refusal wrote files: %s" % sorted(p.name for p in out.iterdir()))
    env = dict(os.environ, FLOE_INDEX_LIGHT="on")
    r = run([floe2, "index", src, "--jobs", "1"], what="floe2 index (the fallback)", env=env)
    text = r.stdout + r.stderr
    if "the full index was built in its place" not in text:
        fail("floe2 index did not say it fell back to the full index:\n%s" % text[-1200:])
    cache = work / ".head_cblock.oas.ice"
    if not (cache / "design.ovm").is_file():
        fail("the fallback left no full index in %s" % cache)
    r = run([floe2, "info", src, "--json"], what="floe2 info")
    if json.loads(r.stdout).get("cache_kind") != "full":
        fail("info does not see the full index: %s" % r.stdout[-300:])
    src2 = work / "head_cblock_2.oas"
    shutil.copyfile(str(src), str(src2))
    r = run([floe2, "index", src2, "--light", "--jobs", "1"], ok=(79,),
            what="floe2 index --light on a cell inside a CBLOCK")
    text = r.stdout + r.stderr
    if "defined inside CBLOCKs" not in text or "built in its place" in text:
        fail("--light did not end with the refusal alone:\n%s" % text[-800:])
    if (work / ".head_cblock_2.oas.ice" / "design.ovm").exists():
        fail("--light built the full index")
    print("  cells inside CBLOCKs: light refused (79), the rule's light cache fell back to the full index, --light did not")


def render(cache, out, view, size, extra):
    cmd = [RENDER, cache, "--view", "%g,%g,%g,%g" % view, "--width", str(size), "--height", str(size),
           "--decode-pages", "100000000", "--jobs", "2", "--out", out] + extra
    r = run(cmd, what="floe-render-cli %s" % Path(cache).name)
    fields = {}
    for line in r.stdout.splitlines():
        parts = line.split("\t")
        for p in parts[1:]:
            k, _, v = p.partition("=")
            fields[(parts[0], k)] = v
    return fields


def png_lit_diff(a, b):
    """(pixels, lit in a, differing pixels) of two same-size PNGs"""
    from PIL import Image
    ia = Image.open(a).convert("RGBA")
    ib = Image.open(b).convert("RGBA")
    if ia.size != ib.size:
        fail("%s vs %s: sizes %s / %s" % (a, b, ia.size, ib.size))
    pa, pb = ia.tobytes(), ib.tobytes()
    n = ia.size[0] * ia.size[1]
    lit = 0
    diff = 0
    for i in range(n):
        q = pa[4 * i:4 * i + 4]
        if q[:3] != b"\0\0\0":
            lit += 1
        if q != pb[4 * i:4 * i + 4]:
            diff += 1
    return n, lit, diff


def check_fixture(name, src, work):
    t0 = time.time()
    full = work / ("%s_full" % src.stem)
    light = work / ("%s_light" % src.stem)
    shutil.rmtree(full, ignore_errors=True)
    shutil.rmtree(light, ignore_errors=True)
    run([BIN, "vfs", src, full, "--no-lod", "--occupancy", "--jobs", "2"], what="floe-index vfs")
    r = run([BIN, "light", src, "--out", light, "--occupancy", "--jobs", "2"], what="floe-index light")
    if "[light]" not in r.stderr:
        fail("%s: no [light] line: %s" % (name, r.stderr[-300:]))
    # the light command's files are the streaming build's, the summary
    # (from the segment directory's placements) the streaming pass's
    for f in ("design.inv", "design.seg", "design.ovo"):
        if (full / f).read_bytes() != (light / f).read_bytes():
            fail("%s: %s differs between the streaming build and floe-index light" % (name, f))
    if (light / "design.ovm").exists():
        fail("%s: the light directory holds a design.ovm" % name)
    # --occupancy-only adds the same summary to a light cache without one;
    # on a full index or no light cache it is refused
    plain = work / ("%s_plain" % src.stem)
    shutil.rmtree(plain, ignore_errors=True)
    run([BIN, "light", src, "--out", plain, "--jobs", "2"], what="floe-index light (no summary)")
    if (plain / "design.ovo").exists():
        fail("%s: a light build without --occupancy wrote design.ovo" % name)
    run([BIN, "light", src, "--out", plain, "--occupancy-only", "--jobs", "2"], what="floe-index light --occupancy-only")
    if (plain / "design.ovo").read_bytes() != (full / "design.ovo").read_bytes():
        fail("%s: --occupancy-only's design.ovo differs" % name)
    for f in ("design.inv", "design.seg", "meta.json"):
        if (plain / f).read_bytes() != (light / f).read_bytes():
            fail("%s: --occupancy-only touched %s" % (name, f))
    r = run([BIN, "light", src, "--out", full, "--occupancy-only"], ok=(1,), what="light --occupancy-only on a full index")
    if "adds design.ovo to a light cache" not in r.stderr:
        fail("%s: --occupancy-only on a full index: %s" % (name, r.stderr[-200:]))
    empty = work / ("%s_empty" % src.stem)
    empty.mkdir(exist_ok=True)
    run([BIN, "light", src, "--out", empty, "--occupancy-only"], ok=(1,), what="light --occupancy-only without a light cache")
    # --force over a full index replaces it with the light cache
    victim = work / ("%s_victim" % src.stem)
    shutil.rmtree(victim, ignore_errors=True)
    shutil.copytree(full, victim)
    run([BIN, "light", src, "--out", victim, "--jobs", "2", "--force"], what="floe-index light --force")
    if (victim / "design.ovm").exists() or (victim / "design.ovp").exists() or not (victim / "design.seg").is_file():
        fail("%s: --force did not replace the full index with the light cache: %s" % (name, sorted(p.name for p in victim.iterdir())))
    mf = json.loads((full / "meta.json").read_text())
    ml = json.loads((light / "meta.json").read_text())
    if ml.get("cache") != "light":
        fail("%s: meta.json lacks cache: light" % name)
    if ml["src"] != mf["src"]:
        fail("%s: meta.json src differs: %s vs %s" % (name, ml["src"], mf["src"]))
    for k in ("dbu", "top_cell", "bbox", "version"):
        if ml[k] != mf[k]:
            fail("%s: meta.json %s differs: %s vs %s" % (name, k, ml[k], mf[k]))
    if [(l["layer"], l["datatype"], l["name"]) for l in ml["layers"]] != \
            [(l["layer"], l["datatype"], l["name"]) for l in mf["layers"]]:
        fail("%s: meta.json layers differ" % name)
    # the views, in microns
    dbu = mf["dbu"]
    x0, y0, x1, y1 = [v * dbu for v in mf["bbox"]]
    w, h = x1 - x0, y1 - y0
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    near = (cx - w / 80, cy - h / 80, cx + w / 80, cy + h / 80)
    fit = (x0 - w * 0.02, y0 - h * 0.02, x1 + w * 0.02, y1 + h * 0.02)
    layers = ["%d/%d" % (l["layer"], l["datatype"]) for l in mf["layers"][:2]]
    views = [
        ("fit exact", fit, 512, ["--cut-px", "0"]),
        ("near exact", near, 512, ["--cut-px", "0"]),
        ("near depth 1", near, 512, ["--cut-px", "0", "--depth", "1"]),
        ("fit depth 1", fit, 512, ["--cut-px", "0", "--depth", "1"]),
        ("two layers", fit, 384, ["--cut-px", "0", "--layers", ",".join(layers)]),
    ]
    for label, view, size, extra in views:
        a = work / ("%s_%s_full.png" % (src.stem, label.replace(" ", "_")))
        b = work / ("%s_%s_light.png" % (src.stem, label.replace(" ", "_")))
        ff = render(full, a, view, size, extra)
        fl = render(light, b, view, size, extra)
        if a.read_bytes() != b.read_bytes():
            n, lit, diff = png_lit_diff(a, b)
            fail("%s: %s: PNGs differ (%d of %d lit pixels)" % (name, label, diff, lit))
        # (the plan's page count and members differ: the light pages are the
        # segments' and a span's box may reach into a view its shapes do not)
        for key in (("plan", "wc_cells"), ("plan", "inst_edges"), ("plan", "frame_rects")):
            if ff[key] != fl[key]:
                fail("%s: %s: plan %s %s (full) vs %s (light)" % (name, label, key[1], ff[key], fl[key]))
    # the cut view: the page partitions differ, the culls may
    a = work / ("%s_cut_full.png" % src.stem)
    b = work / ("%s_cut_light.png" % src.stem)
    render(full, a, fit, 512, ["--cut-px", "3"])
    render(light, b, fit, 512, ["--cut-px", "3"])
    n, lit, diff = png_lit_diff(a, b)
    if diff > max(16, lit * CUT_TOLERANCE):
        fail("%s: cut view: %d of %d lit pixels differ (over %.1f %%)" % (name, diff, lit, CUT_TOLERANCE * 100))
    # the refusals
    r = run([BIN, "light", src, "--out", full], ok=(1,), what="floe-index light into a full index")
    if "holds a full index" not in r.stderr:
        fail("%s: light into a full index: %s" % (name, r.stderr[-200:]))
    changed = work / ("%s_changed.oas" % src.stem)
    shutil.copy(src, changed)
    changed_light = work / ("%s_changed_light" % src.stem)
    run([BIN, "light", changed, "--out", changed_light], what="floe-index light (copy)")
    with open(changed, "ab") as f:
        f.write(b"\0")
    r = subprocess.run([str(RENDER), str(changed_light), "--view", "%g,%g,%g,%g" % fit, "--width", "64",
                        "--height", "64", "--cut-px", "0"], capture_output=True, text=True)
    if r.returncode == 0 or "is not the source" not in r.stderr:
        fail("%s: a changed source was not refused: %s" % (name, r.stderr[-300:]))
    nameless = work / ("%s_nameless" % src.stem)
    nameless.mkdir()
    for f in ("design.inv", "design.seg"):
        shutil.copy(light / f, nameless / f)
    r = subprocess.run([str(RENDER), str(nameless), "--view", "%g,%g,%g,%g" % fit, "--width", "64",
                        "--height", "64", "--cut-px", "0"], capture_output=True, text=True)
    if r.returncode == 0 or "does not say which source" not in r.stderr:
        fail("%s: a nameless light directory was not refused: %s" % (name, r.stderr[-300:]))
    print("OK  %-22s %d layers: %d exact views identical (same cells, edges, frames), cut view %d of %d lit pixels differ, "
          "light files + design.ovo = the streaming build's, --occupancy-only, --force, meta.json, refusals (%.1fs)"
          % (name, len(mf["layers"]), len(views), diff, lit, time.time() - t0))


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    src = Path(sys.argv[1]).resolve()
    for b in (BIN, RENDER):
        if not b.is_file():
            fail("release binary missing: %s" % b)
    if not src.is_file():
        fail("source missing: %s" % src)
    with tempfile.TemporaryDirectory(prefix="floe-light-") as td:
        work = Path(td)
        files = [("valmini", src)]
        plain = work / "valmini_nocblock.oas"
        rewrite_with_klayout(src, plain, cblocks=False, strict=False)
        files.append(("valmini no-cblock", plain))
        strict = work / "valmini_strict.oas"
        rewrite_with_klayout(src, strict, cblocks=True, strict=True)
        files.append(("valmini strict+cblock", strict))
        route = work / "route_chip.oas"
        generate("gen_route_chip.py", route, "--scale", "0.05", "--jobs", "2")
        files.append(("route_chip 0.05", route))
        main01 = work / "main01_like.oas"
        generate("gen_main01_like.py", main01, "--scale", "0.001", "--jobs", "2")
        files.append(("main01_like 0.001", main01))
        for name, path in files:
            check_fixture(name, path, work)
        check_summary_budget(work, main01)
        check_unsuitable(work)
    print("light_cache: ALL OK")


if __name__ == "__main__":
    main()
