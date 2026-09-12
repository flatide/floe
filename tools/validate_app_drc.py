#!/usr/bin/env python3
"""Rust DRC read parity. All DB/ICE/waive files are synthetic and private.

Python generates and reads the oracle; Rust CLI/test execution has PATH empty.
Read operations must not create/modify any source, pack or review sidecar.
"""
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import drc
from validate_drc_ice import DB


def fingerprint(root):
    return {str(p.relative_to(root)): (p.stat().st_mtime_ns,
            hashlib.sha256(p.read_bytes()).hexdigest())
            for p in root.rglob("*") if p.is_file()}


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
        cases, cli_expected = [], []
        for db in (small, large):
            result = subprocess.run([str(index), "drc", str(db), "--jobs", "2"],
                                    capture_output=True, text=True, timeout=30)
            assert result.returncode == 0, result.stderr
            packed = Path(str(db) + ".ice")
            p = drc.IcePack(str(packed))
            case = {"pack": str(packed), "waives": p._waive_path, "cell": p.cell,
                    "precision": p.precision, "checks": [], "queries": []}
            flat = []
            for ci, ch in enumerate(p.checks):
                row = {"name": ch.name, "desc": ch.desc, "declared": ch.declared, "errors": []}
                for ei, error in enumerate(ch.errors):
                    status = 1 if error.num % 7 == 0 else 2 if error.num % 11 == 0 else 0
                    p.set_status(ci, ei, status)
                    row["errors"].append({"num": error.num, "kind": error.kind, "pts": error.pts, "status": status})
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
        env = dict(os.environ, PATH="", FLOE_DRC_ORACLE=str(oracle))
        before = fingerprint(work)
        run = subprocess.run([bins[0], "--ignored", "--nocapture"], env=env,
                             text=True, capture_output=True, timeout=60)
        assert run.returncode == 0, (run.stdout, run.stderr)
        assert "RUST DRC READER: ALL OK" in run.stdout
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
        # Same legacy temp fallback name, without touching the real global /tmp.
        db, packed, rules, _ = cli_expected[0]
        side = Path(cases[0]["waives"])
        backup = work / "review-backup"
        side.rename(backup)
        tmp = work / "private-temp"
        tmp.mkdir()
        name = packed.name[:-4]
        tag = hashlib.sha1(str(packed.absolute()).encode()).hexdigest()[:12]
        shutil.copyfile(backup, tmp / (".%s.waive.web-oracle-%s" % (name, tag)))
        fallback_before = fingerprint(work)
        result = subprocess.run([str(cli), "drc", str(packed), "--rules"],
                                env=dict(env, TMPDIR=str(tmp)), capture_output=True, text=True, timeout=10)
        assert result.returncode == 0, result.stderr
        assert json.loads(result.stdout) == rules
        assert fingerprint(work) == fallback_before
        backup.rename(side)
        # Explicit stale refusal, no accidental rebuild or silent wrong statuses.
        os.utime(small, (small.stat().st_atime, small.stat().st_mtime + 20))
        after = fingerprint(work)
        stale = subprocess.run([str(cli), "drc", str(small), "--rules"], env=env,
                               capture_output=True, text=True, timeout=10)
        assert stale.returncode != 0 and "stale DRC pack" in stale.stderr
        assert fingerprint(work) == after
        print(run.stdout.strip())
        print("RUST DRC CLI: ALL OK (fresh db/direct pack, rules/errors/status, readonly, empty PATH, stale refusal)")


if __name__ == "__main__":
    main()
