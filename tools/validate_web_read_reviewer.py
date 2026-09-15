#!/usr/bin/env python3
"""Explicit ICE reviewer reading: real Rust HTTP, no review write authority.

Synthetic data only. This does not stand in for browser/field acceptance or
legacy ASCII/cache/temp-sidecar selection, which remain separate work.
"""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request

from validate_web_drc_notes import Session, fingerprint, INDEX, DB, API, wait, APP


def read_records(s, refs):
    c = s.context
    return s.client.call("POST", "/api/v1/drc/" + c["drc_id"] + "/read",
                         dict(view_id=c["view_id"], revision=c["revision"],
                              body=dict(kind="records", check=refs[0]["check"],
                                        errors=[r["error"] for r in refs])))


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-read-reviewer-") as td:
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
        live = []

        def session(tag=None, **options):
            s = Session(source, pack, temps, tag, work / "session.json", **options)
            live.append(s)
            return s

        def close(s):
            s.close()
            live.remove(s)

        try:
            # Seed through existing explicit write APIs, not a handcrafted file.
            writer = session("read-test", edit_waives=True)
            c = writer.context
            rules = writer.client.call("POST", "/api/v1/drc/" + c["drc_id"] + "/read",
                                       dict(view_id=c["view_id"], revision=c["revision"],
                                            body=dict(kind="rules", start="0", search="", limit=64)))
            check = next(r["check"] for r in rules["rows"] if int(r["errors"]) >= 2)
            refs = [dict(check=check, error=str(i)) for i in range(2)]
            draft = writer.prepare(writer.read(refs), "읽기 전용 saved note")
            writer.client.call("POST", API, writer.request(draft, 1), 202)
            assert writer.finished(1)["published"] is True
            waives = "/api/v1/drc/review/waives"
            snapshot = writer.client.call("POST", waives + "/read", dict(context=c, errors=refs))
            draft = writer.client.call("POST", waives + "/prepare",
                                       dict(context=c, token=snapshot["token"], waived=True))
            writer.client.call("POST", waives, writer.request(draft, 1), 202)
            receipt = wait(lambda: (lambda v: v if v["phase"] in ("succeeded", "failed", "cancelled") else None)(
                writer.client.call("GET", waives + "/1")), writer.proc)
            assert receipt["published"] is True
            close(writer)
            # GTK-era sidecars may have no binding xattr and live read-only.
            # Byte-copy into a new synthetic reviewer name (no xattr copied).
            for suffix in ["notes.{tag}.fe", "waive.{tag}"]:
                original = work / (".synthetic.db." + suffix.format(tag="read-test"))
                legacy = work / (".synthetic.db." + suffix.format(tag="legacy-read"))
                legacy.write_bytes(original.read_bytes())
                legacy.chmod(0o444)
            sidecars = list(work.glob(".synthetic.db.*"))
            assert len(sidecars) >= 2
            saved = fingerprint(sidecars)

            for tag, exists in [("read-test", True), ("legacy-read", True), ("uncreated", False)]:
                reader = session(read_reviewer=tag)
                model = reader.client.call("GET", API)
                assert model["available"] and model["editable"] is False and model["reviewer"] == tag
                assert model["operations"]["last_seq"] == "0" and model["autosave"] is False
                catalog = reader.client.call("GET", "/api/v1/drc")
                assert catalog["waives"] is None
                display = reader.display(refs, focus=refs[0])
                assert display["exists"] is exists and display["reviewer"] == tag
                assert display["legacy_unverified"] is (tag == "legacy-read")
                assert display["focus"]["text"] == ("읽기 전용 saved note" if exists else None)
                assert [r["noted"] for r in display["rows"]] == [exists, exists]
                assert [r["status"] for r in read_records(reader, refs)["rows"]] == ([1, 1] if exists else [0, 0])
                # Exercise all editor/transfer/artifact entry points, including
                # POST download and recovery, with authenticated owner credentials.
                for root in [API, waives]:
                    routes = [("POST", ""), ("POST", "/read"), ("POST", "/prepare"),
                              ("POST", "/revoke"), ("GET", "/1"), ("POST", "/1/cancel"),
                              ("GET", "/transfer"), ("POST", "/transfer"), ("POST", "/transfer/chunk"),
                              ("GET", "/transfer/1"), ("POST", "/transfer/1/cancel"),
                              ("GET", "/artifacts/1"), ("DELETE", "/artifacts/1"),
                              ("GET", "/artifacts/1/download")]
                    for method, suffix in routes:
                        reader.client.call(method, root + suffix, {} if method == "POST" else None, 403)
                    request = urllib.request.Request(reader.client.origin + root + "/artifacts/1/download",
                                                     data=("csrf=" + reader.client.csrf).encode(), method="POST",
                                                     headers={"Origin": reader.client.origin,
                                                              "Content-Type": "application/x-www-form-urlencoded"})
                    try:
                        response = reader.client.opener.open(request, timeout=8)
                    except urllib.error.HTTPError as error:
                        response = error
                    with response:
                        assert response.status == 403, "read-only form download was not denied"
                reader.client.call("GET", waives, code=403)
                close(reader)
                assert fingerprint(sidecars) == saved, "read-only review changed sidecars or metadata"
                assert set(work.glob(".synthetic.db.*")) == set(sidecars), "read created a sidecar/lock"
                assert fingerprint(inputs) == before

            # Unsupported source selection fails early instead of silently
            # showing no notes, parsing ASCII, or enabling a different writer.
            result = subprocess.run([str(APP), "view", str(source), "--drc", str(db),
                                     "--floe-reviewer", "read-test", "--no-open"],
                                    capture_output=True, text=True, timeout=10,
                                    env=dict(os.environ, PATH=""))
            assert result.returncode == 2 and "explicit ICE" in result.stderr
            fifo = work / "not-a-file.ice"
            os.mkfifo(fifo)
            result = subprocess.run([str(APP), "view", str(source), "--drc", str(fifo),
                                     "--floe-reviewer", "read-test", "--no-open"],
                                    capture_output=True, text=True, timeout=10,
                                    env=dict(os.environ, PATH=""))
            assert result.returncode == 2 and "regular file" in result.stderr
        finally:
            for s in live:
                if s.proc.poll() is None:
                    s.proc.terminate()
                try:
                    s.proc.communicate(timeout=15)
                except subprocess.TimeoutExpired:
                    s.proc.kill()
                    s.proc.communicate(timeout=5)
    print("WEB READ REVIEWER: ALL OK (explicit ICE/adjacent notes+waives; all write/transfer routes denied; no sidecar/pack/cache mutation; native shutdown)")


if __name__ == "__main__":
    main(sys.argv[1])
