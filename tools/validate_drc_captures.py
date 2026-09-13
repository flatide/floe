#!/usr/bin/env python3
"""DRC PNG/metadata parity + lifetime tests, synthetic private data only.

Python is the development oracle. The Rust runtime has an empty PATH and
never indexes a DRC source or creates a reviewer sidecar while capturing.
"""
import hashlib
import json
import math
import os
from pathlib import Path
import resource
import shutil
import signal
import subprocess
import sys
import tempfile
import time

from validate_app_render import APP, INDEX, RENDERD, ROOT, digest, wait_marker
from validate_app_captures import fake_worker, invoke

sys.path.insert(0, str(ROOT))
from floe import drc, fe_embed as fe

RULE = "RULE / 한글"


def write_db(path, fractional=False):
    # Rectangles, rotated CD, one/two edges, degenerates, fill cap boundary.
    records = [
        ("p", [(1000, 2000), (6000, 2000), (6000, 4000), (1000, 4000)]),
        ("p", [(4000, 0), (8000, 4000), (6000, 6000), (2000, 2000)]),
        ("e", [(1000, 2000), (6000, 2000)]),
        ("e", [(6000, 2000), (1000, 2000)]),
        ("e", [(2000, 1000), (2000, 7000)]),
        ("e", [(9000, 3000), (2000, 8000)]),
        ("e", [(1000, 2000), (6000, 2000), (1000, 4000), (6000, 4000)]),
        ("e", [(1000, 1000), (6000, 6000), (1000, 6000), (6000, 1000)]),
        ("e", [(1000, 2000), (1000, 2000)]),
        ("p", [(5000, 5000)]),
    ]
    for n in (256, 257):
        records.append(("p", [(round(5000 + 3000 * math.cos(i * 2 * math.pi / n)),
                               round(5000 + 3000 * math.sin(i * 2 * math.pi / n))) for i in range(n)]))
    text = [f"TOP 1000\n{RULE}\n{len(records)} {len(records)} 1\nRule File Pathname: /missing/path/deck.svrf\n"]
    for kind, points in records:
        text.append(f"{kind} 999 {len(points) // 2 if kind == 'e' else len(points)}\n")
        flat = [v + (.125 if fractional else 0) for p in points for v in p]
        step = 4 if kind == "e" else 2
        text.extend(" ".join(map(str, flat[i:i+step])) + "\n" for i in range(0, len(flat), step))
    text.append(f"{RULE}\n1 1 0\np 1 1\n9000 9000\nEMPTY\n0 0 0\n")
    path.write_text("".join(text))


def output_rows(text):
    out = []
    for line in text.splitlines():
        p = line.split("\t")
        if len(p) == 3 and p[0].isdigit() and p[1].isdigit():
            out.append((int(p[0]), int(p[1]), Path(p[2])))
    return out


def compare(source, db, work, env, tag, args=()):
    results = []
    for side, jobs, python in (("j1", 1, False), ("j8", 8, False), ("python", 1, True)):
        out = work / f"{tag}-{side}.png"
        run = invoke(["render", source, "--drc", db, "--drc-rule", RULE,
                      "--px", "401x79", *args, "--out", out],
                     dict(env, FLOE_RUST_JOBS=str(jobs), FLOE_RUST_RASTER_JOBS=str(jobs),
                          TMPDIR=str(work / "oracle-temp") if python else env["TMPDIR"]), python=python)
        rows = output_rows(run.stdout)
        assert rows, (tag, run.stdout, run.stderr)
        results.append(rows)
    assert [[r[:2] for r in rows] for rows in results].count([r[:2] for r in results[0]]) == 3
    for a, b, c in zip(*results):
        actual, parallel, expected = [r[2].read_bytes() for r in (a, b, c)]
        assert actual == parallel, (tag, a[:2], "jobs changed bytes")
        assert actual == expected, (tag, a[:2], fe.extract_text(actual), fe.extract_text(expected),
                                    "image equal", fe.insert_text(actual, None) == fe.insert_text(expected, None))
    return results[0]


def fingerprint(paths):
    return {str(p): (p.stat().st_mtime_ns, hashlib.sha256(p.read_bytes()).digest()) for p in paths}


def safety(source, db, work, env, unit):
    fake = fake_worker(work)
    log, marker, target = [work / p for p in ("worker.commands", "worker.phase", "protected.png")]
    fake_env = dict(env, FLOE_RENDERD_BIN=str(fake), FAKE_LOG=str(log), FAKE_MARKER=str(marker), FAKE_UNIT=str(unit))
    args = ["render", source, "--drc", db, "--drc-rule", RULE, "--px", "16", "--out", target]
    one = [*args, "--drc-err", "1"]
    def reset():
        target.write_bytes(b"previous export")
        log.unlink(missing_ok=True)
        marker.unlink(missing_ok=True)
    def clean():
        assert not list(work.glob(".floe-shot-*"))
        assert not list(Path(env["TMPDIR"]).iterdir()), list(Path(env["TMPDIR"]).iterdir())
    for mode in ("partial", "deferred"):
        reset()
        p = invoke(one, dict(fake_env, FAKE_MODE=mode), code=3)
        assert "0 DRC PNG(s) already saved" in p.stderr
        assert target.read_bytes() == b"previous export"
        clean()
    for phase in ("ready", "open", "style", "render"):
        for sig in (signal.SIGINT, signal.SIGTERM):
            reset()
            proc = subprocess.Popen([str(APP), *map(str, one)], cwd=ROOT,
                                    env=dict(fake_env, FAKE_MODE=phase), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                wait_marker(marker, proc)
                child = int(marker.read_text())
                start = time.monotonic()
                proc.send_signal(sig)
                stdout, stderr = proc.communicate(timeout=8)
                assert proc.returncode == 128 + sig and time.monotonic() - start < 5, (phase, stdout, stderr)
                try: os.kill(child, 0)
                except ProcessLookupError: pass
                else: raise AssertionError("worker not reaped")
                assert target.read_bytes() == b"previous export"
                clean()
            finally:
                if proc.poll() is None: proc.kill(); proc.communicate()
    reset()
    invoke([*args, "--drc-err", "1-3", "--drc-cap", "1"], fake_env)
    commands = [json.loads(line) for line in log.read_text().splitlines()]
    assert [c[0] for c in commands].count("ready") == 1
    assert [c[0] for c in commands].count("open") == 1
    assert [c[0] for c in commands].count("style") == 1
    renders = [c[1] for c in commands if c[0] == "render"]
    assert [c["gen"] for c in renders] == ["1", "2", "3"]
    assert all(c["cut"] in ("0", "0.0") and c["frames"] == c["labels"] == "1" and c["thin"] == "cull" for c in renders)
    reset()
    invoke([*one, "--detail", "medium", "--thin", "keep", "--label-font-px", "18"], fake_env)
    command = next(json.loads(line)[1] for line in log.read_text().splitlines() if json.loads(line)[0] == "render")
    assert command["cut"] == "3" and command["thin"] == "keep" and command["font_px"] == "18"
    for i in (1, 2, 3): (work / f"protected_{i}.png").write_bytes(b"previous export")
    p = invoke([*args, "--drc-err", "1-3"], dict(fake_env, FAKE_MODE="fail2"), code=1)
    assert "1 DRC PNG(s) already saved" in p.stderr
    assert fe.read_bytes((work / "protected_1.png").read_bytes())
    assert all((work / f"protected_{i}.png").read_bytes() == b"previous export" for i in (2, 3))
    clean()
    reset()
    for flags in (["--drc-cap", "0"], ["--drc-frac", "nan"], ["--drc-err", "0"],
                  ["--drc-err", "999"], ["--drc-err", "999-1000"], ["--drc-rule", "EMPTY"],
                  ["--batch", "-"], ["--report", work / "unused.json"], ["--bbox=0,0,1,1"],
                  ["--floe-reviewer", " "], ["--px", "4097"], ["--layers", "UNKNOWN"]):
        invoke([*one, *flags], fake_env, code=2)
        assert target.read_bytes() == b"previous export"
        assert not log.exists(), (flags, log.read_text() if log.exists() else "")
    protected = [source, db, Path(str(db)+".ice"), Path(str(source)+".layerprops"),
                 Path(str(source)+".floe")/"meta.json", Path(str(db)+".rules.json")]
    for path in protected:
        invoke([*one, "--out", path], fake_env, code=2)
        assert not log.exists()
    alias = work / "alias.png"
    alias.symlink_to(db)
    invoke([*one, "--out", alias], fake_env, code=2)
    alias.unlink()
    alias.hardlink_to(db)
    invoke([*one, "--out", alias], fake_env, code=2)
    alias.unlink()
    clean()
    # Metadata validation and staging failure must never publish a naked PNG.
    reset()
    broken = work / "corrupt-renderd"
    broken.write_text(fake.read_text().replace('write_bytes(data)', 'write_bytes(data[:-5])'))
    broken.chmod(0o700)
    invoke(one, dict(fake_env, FLOE_RENDERD_BIN=str(broken)), code=1)
    assert target.read_bytes() == b"previous export"
    clean()
    reset()
    broken.write_text(fake.read_text().replace('labels_truncated=0', 'labels_truncated=1'))
    invoke(one, dict(fake_env, FLOE_RENDERD_BIN=str(broken)), code=3)
    assert target.read_bytes() == b"previous export"
    clean()
    # A simulated write failure is in the PNG staging file, after renderer I/O.
    # preexec limits also cover the small fake worker PNG and command log, so
    # choose an annotation-dense polygon whose metadata alone exceeds 2 KiB.
    reset()
    def small_file_limit():
        resource.setrlimit(resource.RLIMIT_FSIZE, (2048, 2048))
        signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
    p = subprocess.run([str(APP), *map(str, [*args, "--drc-err", "12"])], cwd=ROOT,
                       env=fake_env, text=True, capture_output=True, timeout=20, preexec_fn=small_file_limit)
    assert p.returncode == 1 and "0 DRC PNG(s) already saved" in p.stderr, (p.stdout, p.stderr)
    assert "File too large" in p.stderr, p.stderr
    assert target.read_bytes() == b"previous export"
    clean()
    reset()
    truncated = work / "truncated.db"
    truncated.write_text("TOP 1000\nR\np 1 4\n0 0\n1 1\n")
    invoke([*one,"--drc",truncated,"--drc-rule","R"],fake_env,code=3)
    assert not log.exists() and target.read_bytes() == b"previous export"
    huge = work / "metadata-limit.db"
    huge.write_text("TOP 1000\nR\ne 1 100001\n"+"0 0 1000 1000\n"*100001)
    invoke([*one,"--drc",huge,"--drc-rule","R"],fake_env,code=2)
    assert not any(json.loads(line)[0] == "render" for line in log.read_text().splitlines())
    assert target.read_bytes() == b"previous export"
    clean()


def main():
    with tempfile.TemporaryDirectory(prefix="floe-drc-capture-") as tmp:
        work = Path(tmp)
        runtime = work / "runtime"
        runtime.mkdir()
        (work / "oracle-temp").mkdir()
        source = work / "한 글.oas"
        shutil.copyfile(sys.argv[1], source)
        env = dict(os.environ, PATH="", PYTHONPATH=str(ROOT), FLOE_REVIEWER="drc-capture-test",
                   FLOE_RENDERD_BIN=str(RENDERD), FLOE_INDEX_BIN=str(INDEX),
                   FLOE_RUST_JOBS="1", FLOE_RUST_RASTER_JOBS="4", FLOE_RUST_ROUND_PAGES=str(1 << 30),
                   TMPDIR=str(runtime), PYTHONDONTWRITEBYTECODE="1")
        invoke(["index", source, "--jobs", "2"], env)
        cache = Path(str(source)+".floe")
        before = digest(cache)
        meta = json.loads((cache/"meta.json").read_text())
        layer = meta["layers"][0]
        key = f'{layer["layer"]}/{layer["datatype"]}'
        props = Path(str(source)+".layerprops")
        props.write_text(f'{layer["layer"]}.{layer["datatype"]} skyblue diagonal_1 CUSTOM 0 3\n')
        db = work / "errors.db"
        write_db(db)
        compare(source, db, work, env, "ascii", ["--layers", key])
        # Native pack and existing reviewer status, never created by Rust capture.
        subprocess.run([str(INDEX), "drc", str(db), "--jobs", "2"], check=True, env=env, capture_output=True)
        os.environ["FLOE_REVIEWER"] = env["FLOE_REVIEWER"]
        pack = drc.IcePack(str(db)+".ice")
        pack.set_status(0, 0, 1)
        pack.set_status(0, 2, 2)
        waive = Path(pack._waive_path)
        pack.close()
        rules = Path(str(db)+".rules.json")
        rules.write_text(json.dumps({"format":"floe-svrf-rules", "version":1,
                                    "checks":{RULE:{"source_gds":[[layer["layer"],None]]}}}))
        inputs = [source, db, Path(str(db)+".ice"), waive, props, rules]
        initial = fingerprint(inputs)
        compare(source, db, work, env, "pack", [])
        compare(source, Path(str(db)+".ice"), work, env, "direct", ["--layers",key, "--drc-err","1-3"])
        for tag, flags in [
            ("cap", ["--drc-cap","2"]), ("range",["--drc-err","2-3","--drc-cap","1"]),
            ("clamp-lo",["--drc-err","3","--drc-frac","0"]),
            ("clamp-hi",["--drc-err","4","--drc-frac","9"]),
            ("depth",["--drc-err","1","--depth","0"]),
            ("all",["--drc-err","1","--layers","all","--drc-rules","/missing/ignored.json"]),
            ("none",["--drc-err","1","--layers",","]),
            ("bad-sidecar",["--drc-err","1","--drc-rules","/missing/warn.json"]),
        ]:
            compare(source, db, work, env, tag, flags)
        fractional = work / "fractional.db"
        write_db(fractional, True)
        compare(source, fractional, work, env, "fractional", ["--layers",key])
        # Automatic adjacent-deck name beats <db>.rules.json; unusable/missing
        # rule metadata falls back to all with an explicit warning, not none.
        adjacent = work / "deck.svrf.rules.json"
        adjacent.write_text(rules.read_text())
        compare(source, db, work, env, "adjacent", ["--drc-err","1"])
        adjacent.write_text('{"format":"floe-svrf-rules","version":1,"checks":{}}')
        compare(source, db, work, env, "no-metadata", ["--drc-err","1"])
        adjacent.unlink()
        # Complete deck follows the existing live composite path (no labels).
        from validate_jobdeck import THIN_DECK, build_thin_oas
        build_thin_oas(work / "thin.oas")
        invoke(["index", work / "thin.oas", "--jobs=2"], env)
        deck = work / "thin.jb"
        deck.write_text(THIN_DECK)
        compare(deck, db, work, env, "deck", ["--drc-err","1","--layers","all"])
        # Default filename is sanitized rule text and the cap only affects all.
        p = invoke(["render", source, "--drc",db,"--drc-rule",RULE,"--drc-err","1","--px","16"], env, cwd=work)
        assert output_rows(p.stdout)[0][2] == Path("RULE_한글.png")
        dotted = work / "dots.db"
        dotted.write_text("TOP 1000\n..\ne 1 1\n0 0 1000 0\ne 2 1\n0 0 1000 0\n")
        for python in (False, True):
            p = invoke(["render",source,"--drc",dotted,"--drc-rule","..","--px","16"],
                       env,python=python,cwd=work)
            assert [r[2] for r in output_rows(p.stdout)] == [Path(".._1.png"),Path(".._2.png")]
        safety(source, db, work, env, 1/meta["dbu"])
        assert fingerprint(inputs) == initial and digest(cache) == before
        assert not list(runtime.iterdir()), list(runtime.iterdir())
    print("RUST DRC CAPTURES: ALL OK (Python PNG/metadata, ASCII/pack/waives, CD/fill/legend, ranges, one worker, failures/signals)")


if __name__ == "__main__":
    main()
