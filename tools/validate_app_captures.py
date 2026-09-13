#!/usr/bin/env python3
"""Rust batch/mosaic parity and atomicity, private synthetic sources only.

Python/KLayout/Pillow are development oracles. Rust runs with PATH empty.
"""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

from PIL import Image
from validate_app_render import APP, INDEX, RENDERD, ROOT, digest, wait_marker


def invoke(args, env, code=0, python=False, stdin=None, cwd=ROOT):
    binary = [sys.executable, "-B", "-m", "floe2"] if python else [str(APP)]
    p = subprocess.run(binary + list(map(str, args)), env=env, cwd=cwd,
                       input=stdin, capture_output=True, text=True, timeout=60)
    assert p.returncode == code, (args, p.returncode, p.stdout, p.stderr)
    return p


def images(output, batch):
    if batch:
        return {p.name: p for p in output.iterdir() if p.suffix == ".png"}
    return {p.name[len(output.stem):]: p for p in output.parent.glob(output.stem + "*.png")}


def compare(source, work, env, tag, args, batch_text=None, stdin=False, code=0):
    batch = batch_text is not None
    results, docs = [], []
    for side, jobs, python in (("j1", 1, False), ("j8", 8, False), ("python", 1, True)):
        output = work / (f"{tag}-{side}" + ("" if batch else ".png"))
        report = work / f"{tag}-{side}.json"
        extra = []
        if batch:
            file = work / (tag + ".shots")
            file.write_text(batch_text)
            extra = ["--batch", "-" if stdin else file]
        p = invoke(["render", source, *args, *extra, "--out", output, "--report", report],
                   dict(env, FLOE_RUST_JOBS=str(jobs), TMPDIR=str(work / "oracle-temp") if python else env["TMPDIR"]), code=code, python=python,
                   stdin=batch_text if stdin else None)
        if code == 3:
            assert "INCOMPLETE" in p.stdout + p.stderr or "incomplete" in p.stdout + p.stderr
        doc = json.loads(report.read_text())
        for row in doc["shots"]:
            row.pop("ms")
            row.pop("out")
            if not batch:
                row.pop("name")
        results.append(images(output, batch))
        docs.append(doc)
    assert docs[0] == docs[1] == docs[2], (tag, docs)
    assert results[0].keys() == results[1].keys() == results[2].keys(), (tag, results)
    for key, first in results[0].items():
        assert first.read_bytes() == results[1][key].read_bytes(), (tag, key, "jobs changed PNG bytes")
        with Image.open(first) as a, Image.open(results[2][key]) as b:
            assert a.size == b.size and a.convert("RGBA").tobytes() == b.convert("RGBA").tobytes(), (tag, key, "pixel mismatch")
    return docs[0]


def fake_worker(work):
    version = subprocess.check_output([str(RENDERD), "--version"], text=True).split()[1]
    path = work / "fake-renderd"
    path.write_text(f'''#!{sys.executable}
import json, os, pathlib, struct, sys, time, zlib
mode = os.environ.get("FAKE_MODE", "normal")
def log(kind, fields=None):
    with open(os.environ["FAKE_LOG"], "a") as f: f.write(json.dumps([kind, fields or {{}}]) + "\\n")
def hold(phase):
    if mode == phase:
        pathlib.Path(os.environ["FAKE_MARKER"]).write_text(str(os.getpid()))
        time.sleep(60)
def chunk(k, d): return struct.pack(">I",len(d)) + k + d + struct.pack(">I",zlib.crc32(k+d)&0xffffffff)
log("ready")
hold("ready")
print("ready version={version}",flush=True)
counter, deck = 0, False
for line in sys.stdin:
    words = line.split()
    if not words: continue
    kind, d = words[0], dict(w.split("=",1) for w in words[1:])
    log(kind,d)
    hold(kind)
    if kind == "open":
        deck = "deck" in d
        print("opened unit=" + os.environ.get("FAKE_UNIT", "1000") + " max_depth=6",flush=True)
    elif kind == "style": print("styled epoch="+d["epoch"],flush=True)
    elif kind == "render":
        counter += 1
        hold("render"+str(counter))
        if mode == "fail2" and counter == 2:
            print("error code=render gen="+d["gen"]+" message=ENOSPC",flush=True)
            continue
        w,h = int(d["w"]),int(d["h"])
        rgba = bytes((counter*17 % 256,41,83,255))*(w*h)
        if d["frame_format"] == "raw": data = b"FLOERAW1"+struct.pack("<II",w,h)+rgba
        else:
            data = b"\\x89PNG\\r\\n\\x1a\\n"+chunk(b"IHDR",struct.pack(">IIBBBBB",w,h,8,6,0,0,0))+chunk(b"IDAT",zlib.compress(b"".join(b"\\0"+rgba[y*w*4:(y+1)*w*4] for y in range(h)),6))+chunk(b"IEND",b"")
        pathlib.Path(d["out"]).write_bytes(data)
        partial = mode in ("partial", "deferred")
        scene = "scene_gen=0 scene_round=0 scene_complete=0 scene_summary=0" if deck else "scene_gen="+d["gen"]+" scene_round=1 scene_complete="+str(int(not partial))+" scene_summary=0"
        print("frame gen="+d["gen"]+" round=1 final=1 partial="+str(int(partial))+" deferred="+("2" if mode == "deferred" else "0")+" labels_truncated=0 style_epoch="+d["style_epoch"]+" format="+d["frame_format"]+" png="+d["out"]+" "+scene,flush=True)
    elif kind == "quit": break
''')
    path.chmod(0o700)
    return path


def safety(source, work, env, deck, deck_unit):
    target = work / "protected.png"
    report = work / "protected.json"
    log = work / "worker.commands"
    marker = work / "worker.phase"
    fake = fake_worker(work)
    fake_env = dict(env, FLOE_RENDERD_BIN=str(fake), FAKE_LOG=str(log), FAKE_MARKER=str(marker))
    args = ["render", source, "--corners=0,0,100,100", "--size=10,10", "--px=16x12", "--keep-tiles", "--out", target, "--report", report]
    kept = [target.with_name(target.stem + '_' + tag + '.png') for tag in ("tl", "tr", "bl", "br")]
    def reset():
        for p in [target, report, *kept]: p.write_bytes(b"previous export")
        log.unlink(missing_ok=True)
        marker.unlink(missing_ok=True)
    def unchanged():
        assert all(p.read_bytes() == b"previous export" for p in [target, report, *kept])
        assert not list(work.glob(".floe-shot-*"))
        assert not list(Path(env["TMPDIR"]).iterdir()), list(Path(env["TMPDIR"]).iterdir())
    for mode, code in (("fail2", 1), ("partial", 3)):
        reset()
        p = invoke(args, dict(fake_env, FAKE_MODE=mode), code=code)
        assert "0 artifact(s) already saved" in p.stderr
        unchanged()
    for phase in ("ready", "open", "style", "render2"):
        for sig in (signal.SIGINT, signal.SIGTERM):
            reset()
            proc = subprocess.Popen([str(APP), *map(str, args)], cwd=ROOT,
                env=dict(fake_env, FAKE_MODE=phase), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                wait_marker(marker, proc)
                pid = int(marker.read_text())
                start = time.monotonic()
                proc.send_signal(sig)
                stdout, stderr = proc.communicate(timeout=8)
                assert proc.returncode == 128+sig, (phase, stdout, stderr)
                assert time.monotonic()-start < 5
                try: os.kill(pid,0)
                except ProcessLookupError: pass
                else: raise AssertionError("worker not reaped")
            finally:
                if proc.poll() is None: proc.kill(); proc.communicate()
            unchanged()
    # One native lifetime for one plain shot and one four-tile mosaic.
    reset()
    batch = work / "lifetime.shots"
    batch.write_text("first bbox=0,0,10,10\nsecond corners=0,0,100,100 size=10,10 keep_tiles=yes\n")
    invoke(["render", source, "--batch", batch, "--px=16x12", "--out", work / "lifetime"], fake_env)
    commands = [json.loads(line) for line in log.read_text().splitlines()]
    assert [kind for kind, _ in commands].count("ready") == 1
    assert [kind for kind, _ in commands].count("open") == 1
    assert [kind for kind, _ in commands].count("style") == 1
    renders = [d for kind, d in commands if kind == "render"]
    assert [d["gen"] for d in renders] == ["1","2","3","4","5"]
    assert [d["frame_format"] for d in renders] == ["png","raw","raw","raw","raw"]
    # A later row failure keeps the committed first PNG, never half-publishes
    # the failing mosaic, and explains why the old report was not replaced.
    reset()
    output = work / "later-failure"
    output.mkdir()
    second = [output / ("second" + suffix + ".png") for suffix in ("", "_tl", "_tr", "_bl", "_br")]
    for p in second: p.write_bytes(b"previous export")
    p = invoke(["render", source, "--batch", batch, "--px=16x12", "--out", output, "--report", report],
               dict(fake_env, FAKE_MODE="fail2"), code=1)
    assert "1 artifact(s) already saved" in p.stderr
    assert (output / "first.png").read_bytes().startswith(b"\x89PNG")
    assert all(p.read_bytes() == b"previous export" for p in second)
    assert report.read_bytes() == b"previous export" and not list(output.glob(".floe-shot-*"))
    assert not list(Path(env["TMPDIR"]).iterdir())
    # Known deck deferrals remain publishable ONLY as incomplete; count each tile.
    reset()
    invoke(["render", deck, "--corners=0,0,100,100", "--size=10,10", "--px=16x12", "--out", target, "--report", report],
           dict(fake_env, FAKE_MODE="deferred", FAKE_UNIT=str(deck_unit)), code=3)
    doc = json.loads(report.read_text())
    assert not doc["complete"] and not doc["shots"][0]["complete"]
    assert doc["shots"][0]["over_budget_pages"] == doc["jobdeck"]["over_budget_pages"] == 8
    # Invalid plans/collisions must fail before even starting the fake worker.
    for text in ("x\nx\n", "A\na\n", "../bad bbox=0,0,1,1\n", "\"\" bbox=0,0,1,1\n",
                 "x corners=0,0,100,100 size=10,10 keep_tiles=yes\nx_tl\n",
                 "x line=nan\n", "x labels=1\n", "x linecolor=한글\n", "x layers=UNKNOWN\n"):
        reset()
        batch.write_text(text)
        out = work / "invalid-new-directory"
        invoke(["render", source, "--batch", batch, "--out", out], fake_env, code=2)
        assert not out.exists() and not log.exists(), text
    batch.write_text("x bbox=0,0,10,10\n")
    for output, report_path in ((work / "collision", work / "collision/x.png"),
                                (work / "report-dir", work / "report-dir"),
                                (Path(str(source)+".floe/new-output"), report)):
        reset()
        invoke(["render", source, "--batch", batch, "--out", output, "--report", report_path], fake_env, code=2)
        assert not output.exists() and not log.exists()
    invoke(["render", source, "--batch", batch, "--out", work / "input-protect", "--report", batch], fake_env, code=2)
    assert batch.read_text() == "x bbox=0,0,10,10\n"
    reset()
    output = work / "missing-report-output"
    invoke(["render", source, "--batch", batch, "--out", output, "--report", work / "missing-report/report.json"], fake_env, code=1)
    assert not output.exists() and not log.exists()
    # A lexicographic sibling must not hide an output-file ancestor collision.
    batch.write_text("x bbox=0,0,10,10\nx.png.json bbox=0,0,10,10\n")
    output = work / "ancestor-collision"
    invoke(["render", source, "--batch", batch, "--out", output, "--report", output / "x.png/report.json"], fake_env, code=2)
    assert not output.exists() and not log.exists()
    # Do not block forever on a producer that keeps --batch - open.
    for sig in (signal.SIGINT, signal.SIGTERM):
        proc = subprocess.Popen([str(APP), "render", str(source), "--batch=-", "--out", str(work / "stdin-cancel")],
                                cwd=ROOT, env=fake_env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            proc.stdin.write("shot bbox=0,0,10,10\n"); proc.stdin.flush()
            time.sleep(.2)
            proc.send_signal(sig)
            # Leave the producer open: EOF must not be what releases the CLI.
            proc.wait(timeout=5)
            proc.stdin.close(); proc.stdin = None
            stdout, stderr = proc.communicate(timeout=5)
            assert proc.returncode == 128+sig, (stdout,stderr)
        finally:
            if proc.poll() is None: proc.kill(); proc.communicate()
        assert not (work / "stdin-cancel").exists()


def main(fixture):
    from validate_jobdeck import DECK, build_oas
    with tempfile.TemporaryDirectory(prefix="floe-app-captures-") as td:
        work = Path(td)
        source = work / "원본 with spaces.oas"
        shutil.copy2(fixture, source)
        temp = work / "worker-temp"; temp.mkdir()
        (work / "oracle-temp").mkdir()
        env = {k:v for k,v in os.environ.items() if not k.startswith("FLOE_")}
        env.update(PATH="", FLOE_INDEX_BIN=str(INDEX), FLOE_RENDERD_BIN=str(RENDERD), FLOE_PRODUCT="floe2",
                   PYTHONPATH=str(ROOT), PYTHONDONTWRITEBYTECODE="1", TMPDIR=str(temp))
        invoke(["index", source, "--jobs=2"], env)
        for name,dbu,w,h,layers in (("chipA.oas",5e-5,2000,2550,[(123,43),(456,0)]),
                                   ("chipB.oas",.001,1000,1000,[(456,0),(7,2)]), ("mark.oas",.002,100,100,[(999,0)])):
            build_oas(work/name,dbu,w,h,layers,"TOP")
            invoke(["index",work/name,"--jobs=2"],env)
        deck = work / "deck.jb"; deck.write_text(DECK)
        missing = work / "missing.jb"; missing.write_text(DECK.replace("TC=mark.oas","TC=absent.oas"))
        caches = {p: digest(p) for p in work.glob("*.floe")}
        compare(source,work,env,"points",["--mosaic-at=0,90;90,90;90,0;0,0","--size=20,12","--px=63x47","--keep-tiles"])
        compare(source,work,env,"corners",["--corners=100,90,0,0","--size=25,15","--px=63x47","--line=1","--line-color=#123456","--keep-tiles"])
        compare(source,work,env,"half",["--corners=-10.9375,-10.9375,100,100","--size=30,20","--px=61x49","--line=0.5","--line-color=#abcDEF","--stretch","--detail=high","--frames","--labels","--depth=1"])
        compare(source,work,env,"wide-line",["--corners=0,0,100,100","--size=20,20","--px=21x19","--line=1000"])
        compare(source,work,env,"single-batch",["--at=400,400","--size=2,2"],"one bbox=0,0,20,12 px=41x29 layers=1/0\n")
        text = '''# full-line comment
"첫 장" bbox=0,0,20,12 layers=1/0 px=41x29
second at=10000nm,10um size=20,10 px=43x31 stretch=yes
quad mosaic="0,90;90,90;90,0;0,0" size=20,12 px=31x23 line=0 keep_tiles=true
corner corners=0,0,100,90 size=20,12 px=33x25 linecolor=#12Ff65 line=3.5
empty bbox=1000000,1000000,1000001,1000001 px=17
'''
        compare(source,work,env,"multi",["--at=100,100","--size=5,5","--thin=keep"],text)
        compare(source,work,env,"stdin",["--px=23x17"],"stdin bbox=0,0,20,12\n",stdin=True)
        for i, path in enumerate((deck,missing)):
            info = json.loads(invoke(["info",path,"--json"],env).stdout)["metadata"]
            bbox = [v*info["dbu"] for v in info["bbox"]]
            size = [(bbox[2]-bbox[0])/2,(bbox[3]-bbox[1])/2]
            args = ["--corners="+','.join(map(str,bbox)), "--size="+','.join(map(str,size)),"--px=39x27","--keep-tiles"]
            compare(path,work,env,"deck"+str(i),args,code=0 if i==0 else 3)
            compare(path,work,env,"deck-batch"+str(i),["--px=31x23"],"fit\nmask layers=METAL1\n",code=0 if i==0 else 3)
        info = json.loads(invoke(["info",deck,"--json"],env).stdout)["metadata"]
        safety(source,work,env,deck,info["dbu"])
        assert all(digest(p)==before for p,before in caches.items()), "source caches changed"
        assert not list(temp.iterdir())
    print("RUST APP CAPTURES: ALL OK (Python pixels/reports, j1/j8 bytes, single worker, staged mosaics, errors/signals)")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
