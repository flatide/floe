#!/usr/bin/env python3
"""Pure Rust DRC review codec parity; Python writes only private synthetic data."""
import hashlib
import json
import os
from pathlib import Path
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
    cargo = shutil.which("cargo")
    assert cargo, "cargo is required for the development oracle"
    build = subprocess.run([cargo, "test", "--offline", "--locked", "-p", "floe-app-core",
                            "--test", "drc_review", "--no-run", "--message-format=json"],
                           cwd=ROOT / "rust", capture_output=True, text=True, timeout=180)
    assert build.returncode == 0, build.stderr
    binaries = [r["executable"] for line in build.stdout.splitlines()
                if (r := json.loads(line)).get("reason") == "compiler-artifact"
                and r["target"]["name"] == "drc_review" and r.get("executable")]
    assert len(binaries) == 1
    with tempfile.TemporaryDirectory(prefix="floe-review-codecs-") as td:
        work = Path(td)
        os.environ["FLOE_REVIEWER"] = "synthetic-web-review-codec"
        small, large = work / "한 글.db", work / "many.db"
        small.write_text(DB)
        gen = subprocess.run([sys.executable, str(ROOT / "tools/gen_drcdb.py"), str(large),
                              "--checks", "20", "--max-errors", "256", "--zeros", "3"],
                             capture_output=True, text=True, timeout=30)
        assert gen.returncode == 0, gen.stderr
        cases = []
        for source in (small, large):
            run = subprocess.run([str(ROOT / "rust/target/release/floe-index"), "drc", str(source),
                                  "--jobs", "2"], capture_output=True, text=True, timeout=30)
            assert run.returncode == 0, run.stderr
            pack_path = Path(str(source) + ".ice")
            before_pack = pack_path.read_bytes()
            pack = drc.IcePack(str(pack_path))
            try:
                refs = [(ci, ei) for ci, c in enumerate(pack.checks) for ei in range(len(c.errors))]
                assert len(refs) >= 6
                case = {"pack": str(pack_path), "centers": [pack._note_center(g) for g in range(pack.total)],
                        "waives": [], "notes": []}
                for g, (ci, ei) in enumerate(refs):
                    pack.set_status(ci, ei, (0, 1, 2, 255)[g % 4])
                saved = work / (source.name + ".export.waive")
                for edits in ([], [(0, 1), (1, 0), (len(refs)-1, 255)], [(2, 0), (2, 1)]):
                    pack.waive_export(str(saved))
                    raw = saved.read_bytes()
                    # Python's import recomputes counters; Rust must not trust them either.
                    tampered = raw[:40+pack.total] + bytes([255]) * (4 * len(pack.checks))
                    saved.write_bytes(tampered)
                    pack.waive_import(str(saved))
                    for gid, status in edits:
                        pack.set_status(*refs[gid], status)
                    pack.waive_export(str(saved))
                    case["waives"].append({"input": list(tampered), "edits": edits,
                                           "output": list(saved.read_bytes()),
                                           "counts": [pack.status_counts(ci)[0] for ci in range(len(pack.checks))]})

                def snapshot(step):
                    step.update(output=pack._serialize_notes(), groups=pack.notes_list(),
                                lookup=[pack.get_note_gid(g) for g in range(pack.total)])
                    case["notes"].append(step)

                edits = [([0, 1, 2, 2], "  첫 메모\nline "), ([1, 3], "new \\n | comma, <b>한글</b>"),
                         ([1], ""), ([4, 5], "tabs\tand \\ backslash"), ([2, 4], " regroup "),
                         ([0], ""), ([], "no members"), ([3], "second edit"), ([2, 4], "  ")]
                for ids, text in edits:
                    pack.set_note(ids, text)
                    snapshot({"op": "set", "ids": ids, "text": text})
                exported = pack._serialize_notes()
                assert pack._parse_notes(exported, pack.total)
                snapshot({"op": "parse", "input": exported})
                ids = list(range(min(pack.total, 5000)))
                pack.clear_note(ids)
                snapshot({"op": "set", "ids": ids, "text": ""})
                cases.append(case)
            finally:
                pack.close()
            assert pack_path.read_bytes() == before_pack, "Python oracle mutated pack"
        oracle = work / "oracle.json"
        oracle.write_text(json.dumps(cases))
        before = fingerprint(work)
        run = subprocess.run([binaries[0], "--ignored", "--nocapture"],
                             env=dict(os.environ, PATH="", FLOE_DRC_REVIEW_ORACLE=str(oracle)),
                             capture_output=True, text=True, timeout=60)
        assert run.returncode == 0, run.stdout + run.stderr
        assert fingerprint(work) == before, "Rust codec execution wrote a file"
        print(run.stdout.strip())


if __name__ == "__main__":
    main()
