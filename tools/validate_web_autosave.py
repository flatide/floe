#!/usr/bin/env python3
"""Actual web editor modules + Rust HTTP on private synthetic review files.

The headless text-control model is not browser/IME/click acceptance. Session
credentials go to the child over stdin, never argv, a committed file or logs.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import urllib.request

from validate_web_drc_notes import Session, fingerprint, ROOT, INDEX, DB


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-review-autosave-") as td:
        work = Path(td)
        temps = work / "temps"
        temps.mkdir()
        source = work / "synthetic.oas"
        shutil.copy2(fixture, source)
        subprocess.run([str(INDEX), "vfs", str(source), str(source) + ".floe", "--jobs", "2"],
                       check=True, capture_output=True, timeout=30)
        db = work / "synthetic.db"
        db.write_text(DB)
        subprocess.run([str(INDEX), "drc", str(db), "--jobs", "2"],
                       check=True, capture_output=True, timeout=30)
        pack = Path(str(db) + ".ice")
        inputs = [source, db, pack] + [p for p in Path(str(source) + ".floe").rglob("*") if p.is_file()]
        before = fingerprint(inputs)
        session = Session(source, pack, temps, "auto-test", work / "session.json", edit_waives=True)
        try:
            client = session.client
            jar = next(h.cookiejar for h in client.opener.handlers
                       if isinstance(h, urllib.request.HTTPCookieProcessor))
            cookie = "; ".join(f"{c.name}={c.value}" for c in jar)
            assert cookie
            rules = client.call("POST", "/api/v1/drc/" + session.context["drc_id"] + "/read",
                                dict(view_id=session.context["view_id"], revision=session.context["revision"],
                                     body=dict(kind="rules", start="0", search="", limit=64)))
            check = next(row["check"] for row in rules["rows"] if int(row["errors"]) >= 2)
            config = dict(origin=client.origin, csrf=client.csrf, cookie=cookie,
                          context=session.context,
                          references=[dict(check=check, error="0"), dict(check=check, error="1")])
            result = subprocess.run(
                ["node", str(ROOT / "rust/web/ui/review-autosave-native.cjs")],
                input=json.dumps(config), text=True, capture_output=True, timeout=60,
                env=dict(os.environ, NODE_OPTIONS=""),
            )
            assert client.csrf not in result.stdout + result.stderr
            assert cookie not in result.stdout + result.stderr
            assert all(c.value not in result.stdout + result.stderr for c in jar)
            assert client.token not in result.stdout + result.stderr
            assert result.returncode == 0, result.stdout + result.stderr
            print(result.stdout, end="")
            assert (work / ".synthetic.db.notes.auto-test.fe").is_file()
            assert (work / ".synthetic.db.waive.auto-test").is_file()
            assert fingerprint(inputs) == before, "review changed source/pack/cache"
        finally:
            try:
                session.close()
            finally:
                if session.proc.poll() is None:
                    session.proc.terminate()
                    try:
                        session.proc.communicate(timeout=10)
                    except subprocess.TimeoutExpired:
                        session.proc.kill()
                        session.proc.communicate(timeout=5)
    print("WEB AUTOSAVE GATE: ALL OK (synthetic-only writes; source/pack/cache immutable; native shutdown)")


if __name__ == "__main__":
    main(sys.argv[1])
