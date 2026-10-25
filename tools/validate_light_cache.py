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
    """The summary pass keeps no placement cap (0.12.398, DIRECT_READ_PLAN
    §8.21): main01_like 0.001 has 1.29 M placements and a 62 MB top cell,
    and until 0.12.397 `--mem 0.1` (107 MB: above the cell, a cap of about
    280 K at 96 B each) skipped design.ovo. Now the placement store takes
    them - in memory under this budget (1.29 M at 16 B is under a quarter
    of 107 MB), in the staging file design.places.tmp when
    FLOE_OCCUPANCY_PLACES=file says so (or the estimate passes the
    quarter) - and design.ovo is the default budget's bytes either way;
    the staging file is gone with the store."""
    reference = work / "main01_like_light" / "design.ovo"
    if not reference.is_file():
        fail("check_fixture's light cache of main01_like has no design.ovo")
    for backing, env_value in (("in memory", None), ("in the staging file", "file")):
        out = work / ("main01_light_budget_%s" % (env_value or "mem"))
        env = dict(os.environ)
        env.pop("FLOE_OCCUPANCY_PLACES", None)
        if env_value:
            env["FLOE_OCCUPANCY_PLACES"] = env_value
        r = run([BIN, "light", src, "--out", out, "--jobs", "2", "--occupancy", "--mem", "0.1"],
                what="floe-index light --occupancy --mem 0.1 (%s)" % backing, env=env)
        if "placement records exceed" in r.stderr or "is skipped" in r.stderr:
            fail("the summary pass still skips design.ovo under --mem 0.1: %s" % r.stderr[-600:])
        line = [l for l in r.stderr.splitlines() if "placements of" in l and "segment directory" in l]
        if not line or backing not in line[-1]:
            fail("the placement store's line does not say '%s': %s" % (backing, r.stderr[-800:]))
        if not (out / "design.ovo").is_file():
            fail("design.ovo was not built under --mem 0.1 (%s)" % backing)
        if (out / "design.ovo").read_bytes() != reference.read_bytes():
            fail("design.ovo under --mem 0.1 (%s) differs from the default budget's" % backing)
        if (out / "design.places.tmp").exists():
            fail("the placement store's staging file was left behind (%s)" % backing)
        if (out / "design.planes.tmp").exists():
            fail("the pyramids' spill file was left behind (%s)" % backing)
        for name in ("design.inv", "design.seg", "meta.json"):
            if not (out / name).is_file():
                fail("the light cache lacks %s under --mem 0.1 (%s)" % (name, backing))
        print("  summary pass: --mem 0.1 builds main01_like's design.ovo with the placements %s, the default budget's bytes" % backing)


def check_summary_prune(work, src):
    """A layout's frame with the summary on prunes the summarized subtrees
    when no hierarchy frame can come of the walk - frames off, or the depth
    unlimited (0.12.400, DIRECT_READ_PLAN §8.23; the deck's rule since
    0.12.395). Until 0.12.399 a layout pruned with frames off alone, so the
    viewer's default (frames on) walked - on a light cache filled - the
    whole hierarchy under a view the summary drew: main01_like 0.001's
    light cache with its summary, the prefill off so every expanded cell
    costs a fill round, captured at the fit with the summary: no `[light]
    round` with frames on or off and the two pictures byte-identical; at
    depth 1 the frames still draw (the pictures differ) and the rounds
    are the same either way."""
    floe2 = ROOT / "rust" / "target" / "release" / "floe2"
    env = dict(os.environ, FLOE_INDEX_BIN=str(BIN), FLOE_RENDERD_BIN=str(ROOT / "rust" / "target" / "release" / "floe-renderd"),
               FLOE_LIGHT_LOG="on", FLOE_LIGHT_PREFILL_PLACES="0", FLOE_LIGHT_PREFILL_MB="0")
    env.pop("FLOE_OCCUPANCY_PLACES", None)
    run([floe2, "index", src, "--light", "--occupancy", "--jobs", "2"], what="floe2 index --light --occupancy", env=env)

    def capture(frames, depth=None, labels="off"):
        out = work / ("prune_%s_%s_%s.png" % (frames, depth, labels))
        cmd = [floe2, "render", src, "--px", "400", "--detail", "medium", "--occupancy", "on",
               "--frames=%s" % frames, "--labels=%s" % labels, "--no-index", "--out", out]
        if depth is not None:
            cmd += ["--depth", str(depth)]
        r = run(cmd, what="floe2 render (frames %s, depth %s)" % (frames, depth), env=env)
        rounds = [l for l in r.stderr.splitlines() if l.startswith("[light] round")]
        return out.read_bytes(), rounds

    on, on_rounds = capture("on")
    off, off_rounds = capture("off")
    if off_rounds:
        fail("frames off at full depth: the summarized layout filled: %s" % off_rounds)
    if on_rounds:
        fail("frames on at full depth: no frame can appear, yet the walk filled: %s" % on_rounds)
    if on != off:
        fail("frames on/off pictures differ at full depth under the summary")
    # the labels (on by default in the viewer and the capture): a light
    # cache's label walk takes what the image holds and asks for no fill
    # (0.12.400) - at the summarized fit nothing is filled, so no label
    # walk fills either (0.12.399: five rounds, the die's names)
    lab, lab_rounds = capture("on", labels="on")
    if lab_rounds:
        fail("labels on at the summarized fit: the label walk filled: %s" % lab_rounds)
    d1_on, d1_on_rounds = capture("on", 1)
    d1_off, d1_off_rounds = capture("off", 1)
    if d1_on == d1_off:
        fail("depth 1 with frames should draw them")
    if len(d1_on_rounds) != len(d1_off_rounds):
        fail("depth 1: the rounds differ with frames on/off: %s vs %s" % (d1_on_rounds, d1_off_rounds))
    print("  summary prune: a layout's fit with the summary fills nothing with frames or labels on or off (depth 1 still draws the frames)")


def check_metadata_budget(work, src):
    """The light cache's metadata alone (no --occupancy) is the inventory
    pass's output and parses no cell, so a --mem below the largest cell
    builds it - the budget check is waived and said (review 2026-10-23,
    0.12.390) - while --occupancy under the same --mem keeps the refusal
    (exit 71: the summary pass parses the cells)."""
    out = work / "valmini_light_tiny_mem"
    r = run([BIN, "light", src, "--out", out, "--jobs", "1", "--mem", "0.00001"],
            what="floe-index light --mem 0.00001 (metadata only)")
    if "budget check is waived" not in r.stderr:
        fail("the waived check was not said: %s" % r.stderr[-600:])
    for name in ("design.inv", "design.seg", "meta.json"):
        if not (out / name).is_file():
            fail("the metadata-only light cache lacks %s under a tiny --mem" % name)
    if (out / "design.ovo").exists():
        fail("a metadata-only light wrote design.ovo")
    out2 = work / "valmini_light_tiny_mem_occ"
    r = run([BIN, "light", src, "--out", out2, "--jobs", "1", "--occupancy", "--mem", "0.00001"],
            ok=(71,), what="floe-index light --occupancy --mem 0.00001")
    if "needs about" not in r.stderr or "raise --mem" not in r.stderr:
        fail("the summary pass's refusal changed: %s" % r.stderr[-600:])
    if out2.exists() and any(out2.iterdir()):
        fail("the refused --occupancy run wrote files")
    print("  tiny --mem: the metadata-only light cache is built (check waived), --occupancy keeps the refusal (71)")


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
        check_summary_prune(work, main01)
        check_metadata_budget(work, src)
        check_unsuitable(work)
    print("light_cache: ALL OK")


if __name__ == "__main__":
    main()
