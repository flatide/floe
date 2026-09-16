#!/usr/bin/env python3
"""Native DRC review codecs/store/transfer parity against private Python fixtures."""
import hashlib
import json
import os
from pathlib import Path
from cache_test_paths import drc_pack
from floe.cachepath import db_name_of
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import drc
from validate_drc_ice import DB


def attributes(path):
    if hasattr(os, "listxattr"):
        return sorted((key, hashlib.sha256(os.getxattr(path, key)).hexdigest())
                      for key in os.listxattr(path))
    # macOS CPython lacks Linux's os.*xattr APIs; use its read-only system tool.
    assert sys.platform == "darwin", "an xattr reader is required for this gate"
    run = subprocess.run(["/usr/bin/xattr", "-lx", str(path)], capture_output=True, timeout=10)
    assert run.returncode == 0, run.stderr
    return hashlib.sha256(run.stdout).hexdigest()


def fingerprint(root):
    return {str(p.relative_to(root)): (p.stat().st_mtime_ns, p.stat().st_ctime_ns,
            p.stat().st_mode, hashlib.sha256(p.read_bytes()).hexdigest(),
            attributes(p))
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
            pack_path = drc_pack(source)
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
                    # The native whole-file import reads this immutable Python
                    # export, but must recompute deliberately invalid counters.
                    transfer = work / f"{source.name}.transfer-{len(case['waives'])}.waive"
                    export = saved.read_bytes()
                    transfer.write_bytes(export[:40+pack.total] + bytes([255]) * (4 * len(pack.checks)))
                    case["waives"].append({"input": list(tampered), "edits": edits,
                                           "output": list(export), "transfer_input": str(transfer),
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
                note_file = work / f"{source.name}.export.notes.fe"
                pack.note_export(str(note_file))
                exported = note_file.read_text()
                assert exported == pack._serialize_notes()
                pack.note_import(str(note_file))
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
        run = subprocess.run([binaries[0], "sidecars_and_note_edits_match_python", "--ignored", "--nocapture"],
                             env=dict(os.environ, PATH="", FLOE_DRC_REVIEW_ORACLE=str(oracle)),
                             capture_output=True, text=True, timeout=60)
        assert run.returncode == 0, run.stdout + run.stderr
        assert fingerprint(work) == before, "Rust codec execution wrote a file"
        print(run.stdout.strip())
        run = subprocess.run([binaries[0], "store_publication_matches_python", "--ignored", "--nocapture"],
                             env=dict(os.environ, PATH="", FLOE_DRC_REVIEW_ORACLE=str(oracle)),
                             capture_output=True, text=True, timeout=60)
        assert run.returncode == 0, run.stdout + run.stderr
        after = fingerprint(work)
        assert all(after.get(p) == stamp for p, stamp in before.items()), "store modified an existing input/review"
        allowed = set()
        for case in cases:
            name = db_name_of(case["pack"])
            for target in (f".{name}.waive.synthetic-rust-store", f".{name}.notes.synthetic-rust-store.fe"):
                allowed.update((target, target + ".lock"))
        assert set(after) - set(before) == allowed, "unexpected output or surviving staging file"
        # GTK can reopen every generated file. This read of the native note
        # tombstone must not be mistaken for native deletion of the user's file.
        for case in cases:
            pack = drc.IcePack(case["pack"])
            try:
                name = db_name_of(case["pack"])
                text = (work / f".{name}.notes.synthetic-rust-store.fe").read_text()
                assert pack._parse_notes(text, pack.total)
                assert pack.notes_list() == []
            finally:
                pack.close()
        print(run.stdout.strip())


if __name__ == "__main__":
    main()
