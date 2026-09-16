#!/usr/bin/env python3
"""Rust DRC read parity. All DB/ICE/waive files are synthetic and private.

Python generates and reads the oracle; Rust CLI/test execution has PATH empty.
Read operations must not create/modify any source, pack or review sidecar.
"""
import hashlib
import json
import os
from pathlib import Path
from cache_test_paths import drc_pack
from floe.cachepath import db_name_of, db_path_of
import random
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import drc
from validate_drc_ice import DB


def fingerprint(root):
    return {str(p.relative_to(root)): (p.stat().st_mtime_ns,
            hashlib.sha256(p.read_bytes()).hexdigest())
            for p in root.rglob("*") if p.is_file()}


def ascii_case(path):
    data = drc.load_ascii(str(path))
    return {"path": str(path), "cell": data.cell, "precision": data.precision,
            "checks": [{"name": c.name, "desc": c.desc, "declared": str(c.declared),
                        "errors": [{"num": e.num, "kind": e.kind, "pts": e.pts,
                                    "bbox": e.bbox()} for e in c.errors]}
                       for c in data.checks]}


def ascii_cli(cli, sources, work, env):
    for source in sources:
        data = drc.load_ascii(str(source))
        before = fingerprint(work)
        rules = [{"name": c.name, "errors": len(c.errors), "waived": 0} for c in data.checks]
        run = subprocess.run([str(cli), "drc", str(source), "--rules", "--errs", "missing"],
                             env=env, capture_output=True, text=True, timeout=15)
        assert run.returncode == 0, run.stderr
        assert json.loads(run.stdout) == rules
        seen = set()
        for c in data.checks:
            if c.name in seen:
                continue
            seen.add(c.name)
            expected = [{"local": i+1, "global": e.num, "kind": e.kind, "status": 0,
                         "bbox": [round(v, 4) for v in e.bbox()]} for i, e in enumerate(c.errors)]
            run = subprocess.run([str(cli), "drc", str(source), "--errs", c.name], env=env,
                                 capture_output=True, text=True, timeout=15)
            assert run.returncode == 0, run.stderr
            assert json.loads(run.stdout) == expected, (source, c.name)
        for flags in ([], ["--list"]):
            got = subprocess.run([str(cli), "drc", str(source), *flags], env=env,
                                 capture_output=True, text=True, timeout=15)
            want = subprocess.run([sys.executable, "-m", "floe", "drc", str(source), *flags], cwd=ROOT,
                                  capture_output=True, text=True, timeout=15)
            assert got.returncode == want.returncode == 0, (got.stderr, want.stderr)
            assert got.stdout == want.stdout, (source, flags, got.stdout[:500], want.stdout[:500])
        assert fingerprint(work) == before, "ASCII reads changed the source or created a sidecar"
    # Python :g, not Rust's default shortest decimal, is the summary contract.
    source = work / "precision.db"
    for precision in (1e-6, 0.0001, 0.9999999, 1e6, 1.23456789,
                      999999.5, 0.00009999995, 4e-5, 1e308, 1e-308):
        source.write_text(f"TOP {precision!r}\n")
        before = fingerprint(work)
        got = subprocess.run([str(cli), "drc", str(source)], env=env,
                             capture_output=True, text=True, timeout=10)
        assert got.returncode == 0, got.stderr
        assert got.stdout == f"{source}: cell TOP, precision {precision:g}\n0 checks, 0 errors\n"
        assert fingerprint(work) == before


def ascii_failures(cli, work, env):
    source = work / "fallback.db"
    source.write_text("TOP 1000\nR\np 99 1\n1.25 2.5\n")
    side = drc_pack(source)
    for content in (b"FLOEICE\0\1\0\0\0", b"FLOEICE\0\4\0\0\0", b"garbage"):
        side.write_bytes(content)
        before = fingerprint(work)
        got = subprocess.run([str(cli), "drc", str(source), "--rules"], env=env,
                             capture_output=True, text=True, timeout=10)
        assert got.returncode == 0 and "parsing ASCII instead" in got.stderr, got.stderr
        assert json.loads(got.stdout) == [{"name": "R", "errors": 1, "waived": 0}]
        if content.startswith(b"FLOEICE\0"):
            direct = subprocess.run([str(cli), "drc", str(side), "--rules"], env=env,
                                    capture_output=True, text=True, timeout=10)
            assert direct.returncode != 0, direct.stdout
        assert fingerprint(work) == before
    for text in ("", "TOP nan\n", "TOP 1\nR\np --1 1\n1 2\n",
                 "TOP 1\nR\np 1 1\ninf 2\n", "TOP 1\n" + "R" * (1024 * 1024 + 1)):
        bad = work / "invalid.db"
        bad.write_text(text)
        before = fingerprint(work)
        got = subprocess.run([str(cli), "drc", str(bad), "--rules"], env=env,
                             capture_output=True, text=True, timeout=10)
        assert got.returncode != 0 and got.returncode > 0, (got.returncode, got.stderr)
        assert not got.stdout and "panic" not in got.stderr
        assert fingerprint(work) == before
    # A real in-progress ASCII scan is cancelled, not an index subprocess.
    big = work / "cancel.db"
    big.write_text("TOP 1000\nR\n" + "p 1 1\n1.25 2.5\n" * 2_000_000)
    before = fingerprint(work)
    for sig in (signal.SIGINT, signal.SIGTERM):
        process = subprocess.Popen([str(cli), "drc", str(big), "--rules"], env=env,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        time.sleep(.12)
        assert process.poll() is None, "scan ended before signal injection"
        process.send_signal(sig)
        stdout, stderr = process.communicate(timeout=10)
        assert process.returncode != 0 and "cancelled" in stderr.lower(), (process.returncode, stderr)
        assert not stdout
    assert fingerprint(work) == before


def main():
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    built = subprocess.run([cargo, "test", "--offline", "--locked", "-p", "floe-app-core",
                            "--test", "drc_reader", "--no-run", "--message-format=json"],
                           cwd=ROOT / "rust", text=True, capture_output=True, timeout=180)
    assert built.returncode == 0, built.stderr
    bins = [r["executable"] for line in built.stdout.splitlines()
            if (r := json.loads(line)).get("reason") == "compiler-artifact"
            and r["target"]["name"] == "drc_reader" and r.get("executable")]
    assert len(bins) == 1
    with tempfile.TemporaryDirectory(prefix="floe-drc-read-") as td:
        work = Path(td)
        os.environ["FLOE_REVIEWER"] = "web-oracle"
        index = ROOT / "rust/target/release/floe-index"
        cli = ROOT / "rust/target/release/floe2-web"
        small, large = work / "한 글.db", work / "generated.db"
        small.write_text(DB.replace("-1 -2 -3 -4\n", "-1 -2 -3 -4\r\n"))
        gen = subprocess.run([sys.executable, str(ROOT / "tools/gen_drcdb.py"), str(large),
                              "--checks", "20", "--max-errors", "256", "--zeros", "3",
                              "--die=-20,-20,100,100"], capture_output=True, text=True, timeout=30)
        assert gen.returncode == 0, gen.stderr
        ascii_sources = []
        for source in (small, large):
            copy = work / ("ascii-" + source.name)
            shutil.copyfile(source, copy)
            ascii_sources.append(copy)
        fractional = ("FLOAT .125\nR\n-7 8 3\n\n fractional\ne 99 1\n.25 .5 1.125 1.75\n"
                      "p 17 5\n1.25 2.5 3.75 4.125 9\n-2.5 1_000.5\nNEXT\np 1 1\n-1e-5 2.5e1\n"
                      "__RVE_ERROR_TAG2__\ne 1 1\n1 2 3 4\nADMIN_RDBS\n0 0 0\n")
        for i, newline in enumerate(("\r\n", "\r")):
            source = work / f"fractional-{i}.db"
            source.write_bytes(fractional.replace("\n", newline).encode())
            ascii_sources.append(source)
        bad_utf8 = work / "utf8.db"
        bad_utf8.write_bytes(b"CELL\nR\xff\np 1 1\n1.25 -3.75")
        ascii_sources.append(bad_utf8)
        ascii_oracle = work / "ascii-oracle.json"
        ascii_oracle.write_text(json.dumps([ascii_case(p) for p in ascii_sources]))
        cases, cli_expected = [], []
        for db in (small, large):
            result = subprocess.run([str(index), "drc", str(db), "--jobs", "2"],
                                    capture_output=True, text=True, timeout=30)
            assert result.returncode == 0, result.stderr
            packed = drc_pack(db)
            p = drc.IcePack(str(packed))
            case = {"pack": str(packed), "waives": p._waive_path, "cell": p.cell,
                    "precision": p.precision, "checks": [], "queries": []}
            flat = []
            for ci, ch in enumerate(p.checks):
                row = {"name": ch.name, "desc": ch.desc, "declared": ch.declared, "errors": []}
                for ei, error in enumerate(ch.errors):
                    status = 1 if error.num % 7 == 0 else 2 if error.num % 11 == 0 else 0
                    p.set_status(ci, ei, status)
                    row["errors"].append({"num": error.num, "kind": error.kind, "pts": error.pts,
                                          "status": status, "cd": drc.cd_segments(error)})
                    flat.append((ci, ei, error, status))
                row["waived"] = p.status_counts(ci)[0]
                case["checks"].append(row)
            rng = random.Random(13)
            boxes = [[-1e5, -1e5, 1e5, 1e5], [1e5, 1e5, 1e6, 1e6]]
            for _ in range(18):
                x, y, w, h = rng.uniform(-30, 100), rng.uniform(-30, 100), rng.uniform(.01, 50), rng.uniform(.01, 50)
                boxes.append([x, y, x+w, y+h])
            boxes.extend(list(e.bbox()) for _, _, e, _ in flat[:8])
            for bbox in boxes:
                for waived in (None, True, False):
                    checks = None if len(case["queries"]) % 2 else list(range(0, len(p.checks), 2))
                    expected = []
                    for ci, ei, error, status in flat:
                        if checks is not None and ci not in checks or waived is not None and (status == 1) != waived:
                            continue
                        a = error.bbox()
                        if a[0] <= bbox[2] and a[2] >= bbox[0] and a[1] <= bbox[3] and a[3] >= bbox[1]:
                            expected.append([ci, ei, error.num])
                    case["queries"].append({"bbox": bbox, "checks": checks, "waived": waived,
                                            "limit": (1, 7, 2000)[len(case["queries"]) % 3], "expected": expected})
            rules = [{"name": c.name, "errors": len(c.errors), "waived": p.status_counts(i)[0]} for i, c in enumerate(p.checks)]
            errs = {}
            for ci, ch in enumerate(p.checks):
                if ch.name in errs:
                    continue
                errs[ch.name] = [{"local": i+1, "global": e.num, "kind": e.kind,
                                  "status": p.get_status(ci, i), "bbox": [round(v, 4) for v in e.bbox()]}
                                 for i, e in enumerate(ch.errors)]
            cli_expected.append((db, packed, rules, errs))
            p.close()
            cases.append(case)
        oracle = work / "oracle.json"
        oracle.write_text(json.dumps(cases))
        empty = work / "empty-path"
        empty.mkdir()
        env = dict(os.environ, PATH="", FLOE_DRC_ORACLE=str(oracle), FLOE_DRC_ASCII_ORACLE=str(ascii_oracle))
        before = fingerprint(work)
        run = subprocess.run([bins[0], "--ignored", "--nocapture"], env=env,
                             text=True, capture_output=True, timeout=60)
        assert run.returncode == 0, (run.stdout, run.stderr)
        assert "RUST DRC READER: ALL OK" in run.stdout
        assert "RUST ASCII DRC READER: ALL OK" in run.stdout
        for db, packed, rules, errs in cli_expected:
            for source in (db, packed):
                result = subprocess.run([str(cli), "drc", str(source), "--rules", "--floe-reviewer", "web-oracle"],
                                        env=env, capture_output=True, text=True, timeout=10)
                assert result.returncode == 0, result.stderr
                assert json.loads(result.stdout) == rules
            for name, expected in errs.items():
                result = subprocess.run([str(cli), "drc", str(packed), "--errs", name], env=env,
                                        capture_output=True, text=True, timeout=10)
                assert result.returncode == 0, result.stderr
                assert json.loads(result.stdout) == expected, name
        assert fingerprint(work) == before, "DRC reader modified/created files"
        ascii_cli(cli, ascii_sources, work, env)
        ascii_failures(cli, work, env)
        db, packed, rules, _ = cli_expected[0]
        side = Path(cases[0]["waives"])
        # A selected fresh pack with a corrupt review is NOT a reason to
        # silently fall back to ASCII and reset the user's review statuses.
        original = side.read_bytes()
        try:
            side.write_bytes(b"invalid waive")
            before = fingerprint(work)
            result = subprocess.run([str(cli), "drc", str(db), "--rules"], env=env,
                                    capture_output=True, text=True, timeout=10)
            assert result.returncode != 0 and not result.stdout
            assert "parsing ASCII instead" not in result.stderr
            assert fingerprint(work) == before
        finally:
            side.write_bytes(original)
        # DB-derived fallback name/hash, without touching the real global /tmp.
        backup = work / "review-backup"
        side.rename(backup)
        tmp = work / "private-temp"
        tmp.mkdir()
        name = db_name_of(packed)
        tag = hashlib.sha1(db_path_of(packed).encode()).hexdigest()[:12]
        shutil.copyfile(backup, tmp / (".%s.waive.web-oracle-%s" % (name, tag)))
        fallback_before = fingerprint(work)
        result = subprocess.run([str(cli), "drc", str(packed), "--rules"],
                                env=dict(env, TMPDIR=str(tmp)), capture_output=True, text=True, timeout=10)
        assert result.returncode == 0, result.stderr
        assert json.loads(result.stdout) == rules
        assert fingerprint(work) == fallback_before
        backup.rename(side)
        # Legacy stale fallback, no accidental rebuild or stale waive statuses.
        os.utime(small, (small.stat().st_atime, small.stat().st_mtime + 20))
        after = fingerprint(work)
        stale = subprocess.run([str(cli), "drc", str(small), "--rules"], env=env,
                               capture_output=True, text=True, timeout=10)
        assert stale.returncode == 0 and "stale DRC pack" in stale.stderr and "parsing ASCII instead" in stale.stderr
        assert json.loads(stale.stdout) == [{"name": c.name, "errors": len(c.errors), "waived": 0}
                                           for c in drc.load_ascii(str(small)).checks]
        assert fingerprint(work) == after
        print(run.stdout.strip())
        print("RUST DRC CLI: ALL OK (fresh pack/ASCII selection, fractional coordinates, metadata/list/JSON parity, readonly, empty PATH, stale/corrupt fallback, signals)")


if __name__ == "__main__":
    main()
