#!/usr/bin/env python3
"""Default-budget review after saved-note display, with SVRF kept resident."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from cache_test_paths import vfs_cache, drc_pack
from validate_drc_ice import DB
from validate_web_cli import INDEX, wait
from validate_web_drc_notes import API, Session, fingerprint


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-review-budget-") as td:
        work = Path(td)
        temps = work / "runtime"
        temps.mkdir()
        source = work / "layout.oas"
        shutil.copy2(fixture, source)
        db = work / "synthetic.db"
        db.write_text(DB)
        rules = work / "synthetic.rules.json"
        rules.write_text(json.dumps(dict(format="floe-svrf-rules", version=1, checks={
            "GRGEOM.1_BFMOAT": dict(desc="synthetic review budget regression", constraints=[])})))
        for args in (("vfs", source, vfs_cache(source)), ("drc", db)):
            subprocess.run([str(INDEX), *map(str, args), "--jobs", "2"],
                           check=True, capture_output=True, timeout=60)
        pack = drc_pack(db)
        inputs = [source, db, pack, rules, *vfs_cache(source).glob("design.*")]
        before = fingerprint(inputs)
        refs = [dict(check="2", error="0")]
        # Renderer + picker 192 + DRC/SVRF 512 + one review 256:
        # 1024 fits, 1088 exactly fills 2048, 1089 must still fail honestly.
        for budget in (1024, 1088, 1089):
            tag = "budget-%d" % budget
            s = Session(source, pack, temps, tag, work / (tag + ".session"),
                        edit_waives=True, budget_mb=budget, rules=rules)
            try:
                c = s.client
                waive = "/api/v1/drc/review/waives"
                target = work / (".synthetic.db.waive." + tag)
                assert c.call("GET", "/api/v1/drc")["drc"]["metadata"]["svrf"]["matched"] == "1"

                def read_waives(code=200):
                    return c.call("POST", waive + "/read", dict(context=s.context, errors=refs), code)

                if budget == 1089:
                    assert s.display(refs, code=429)["error"] == "review_busy"
                    assert s.read(refs, code=429)["error"] == "review_busy"
                    assert read_waives(429)["error"] == "review_busy"
                    assert not target.exists() and not Path(str(target) + ".lock").exists()
                    continue

                assert not s.display(refs)["cache_hit"]
                assert s.display(refs)["cache_hit"]
                # Reclaiming the display must not discard a live note editor.
                note = s.read(refs)
                assert read_waives(429)["error"] == "review_busy"
                draft = s.prepare(note, "held note remains preparable")
                c.call("POST", API + "/revoke", dict(token=draft["token"]), 204)
                assert not s.display(refs)["cache_hit"]
                assert s.display(refs)["cache_hit"]

                snapshot = read_waives()
                assert snapshot["selected_count"] == "1" and snapshot["waived_count"] == "0"
                # Background display cannot exceed the budget or steal the
                # waive snapshot; neither can a second selection editor.
                assert s.display(refs, code=429)["error"] == "review_busy"
                assert s.read(refs, code=429)["error"] == "review_busy"
                draft = c.call("POST", waive + "/prepare",
                               dict(context=s.context, token=snapshot["token"], waived=True))
                assert not target.exists()
                c.call("POST", waive, s.request(draft, 1), 202)
                result = wait(lambda: (lambda r: r if r["phase"] in ("succeeded", "failed", "cancelled") else None)(
                    c.call("GET", waive + "/1")), s.proc)
                assert result["published"] and result["reader_applied"], result
                s.context["revision"] = result["reader_revision"]
                assert target.stat().st_mode & 0o777 == 0o600
                assert not s.display(refs)["cache_hit"]
                assert s.display(refs)["cache_hit"]
                restored = read_waives()
                assert restored["waived_count"] == "1"
                c.call("POST", waive + "/revoke", dict(token=restored["token"]), 204)
                assert not s.display(refs)["cache_hit"]
                assert not list(work.glob(".*.notes.*"))
            finally:
                s.close()
                assert fingerprint(inputs) == before
    print("web review default-budget admission: OK")


if __name__ == "__main__":
    main(sys.argv[1])
