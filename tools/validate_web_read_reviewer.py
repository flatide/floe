#!/usr/bin/env python3
"""Explicit ICE / current adjacent cache reviewer reads, no write authority.

Synthetic data only. This does not stand in for browser/field acceptance or
cache hot-reload/revision management. Legacy temporary reads use
exact GTK-derived names in a separate directory, outside the source roots.
"""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request

from validate_web_drc_notes import Session, fingerprint, INDEX, DB, API, wait, APP, drc


def read_records(s, refs):
    c = s.context
    return s.client.call("POST", "/api/v1/drc/" + c["drc_id"] + "/read",
                         dict(view_id=c["view_id"], revision=c["revision"],
                              body=dict(kind="records", check=refs[0]["check"],
                                        errors=[r["error"] for r in refs])))


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-read-reviewer-") as td, \
            tempfile.TemporaryDirectory(prefix="floe-legacy-sidecars-") as legacy_td:
        work = Path(td)
        temps = Path(legacy_td)
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

        def paths(tag):
            old_tag, old_temp = os.environ.get("FLOE_REVIEWER"), tempfile.tempdir
            os.environ["FLOE_REVIEWER"], tempfile.tempdir = tag, str(temps)
            try:
                return {"notes": [Path(drc.notes_autosave_path(str(pack))),
                                  Path(drc._notes_tmp_fallback(str(pack)))],
                        "waives": [Path(drc.waive_autosave_path(str(pack))),
                                   Path(drc._waive_tmp_fallback(str(pack)))]}
            finally:
                tempfile.tempdir = old_temp
                if old_tag is None:
                    os.environ.pop("FLOE_REVIEWER", None)
                else:
                    os.environ["FLOE_REVIEWER"] = old_tag

        def sidecar_files():
            return list(work.glob(".synthetic.db.*")) + list(temps.glob(".synthetic.db.*"))

        def session(tag=None, *, database=pack, **options):
            s = Session(source, database, temps, tag, work / "session.json", **options)
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
            original = paths("read-test")
            for tag in ["temporary-read", "adjacent-wins", "mixed"]:
                for kind in ["notes", "waives"]:
                    adjacent, temporary = paths(tag)[kind]
                    target = adjacent if tag == "adjacent-wins" or tag == "mixed" and kind == "notes" else temporary
                    target.write_bytes(original[kind][0].read_bytes())
                    target.chmod(0o444)
                    if tag == "adjacent-wins":
                        # A corrupt temporary file must not override a valid adjacent one.
                        temporary.write_bytes(b"not a valid sidecar")
            for kind in ["notes", "waives"]:
                # Similar names for another pack hash must never be considered.
                wrong_hash = paths("wrong-hash")[kind][1].with_name(paths("wrong-hash")[kind][1].name + "-other")
                wrong_hash.write_bytes(original[kind][0].read_bytes())
            sidecars = sidecar_files()
            assert len(sidecars) >= 2
            saved = fingerprint(sidecars)

            for tag, exists in [("read-test", True), ("legacy-read", True), ("uncreated", False),
                                ("temporary-read", True), ("adjacent-wins", True), ("mixed", True), ("wrong-hash", False)]:
                reader = session(read_reviewer=tag)
                roots = reader.client.call("GET", "/api/v1/browse")["roots"]
                assert len(roots) == 1 and roots[0]["name"] == f"1 · {work.name}", roots
                reader.client.call("POST", "/api/v1/browse",
                                   dict(kind="list", seq="1", directory=str(temps), filter="all_files", query=""), 409)
                model = reader.client.call("GET", API)
                assert model["available"] and model["editable"] is False and model["reviewer"] == tag
                assert model["operations"]["last_seq"] == "0" and model["autosave"] is False
                catalog = reader.client.call("GET", "/api/v1/drc")
                assert catalog["waives"] is None
                assert catalog["drc"]["metadata"]["review_cache"] == "explicit"
                display = reader.display(refs, focus=refs[0])
                assert str(temps) not in str(display), "full temporary path exposed"
                assert display["exists"] is exists and display["reviewer"] == tag, (tag, display)
                assert display["legacy_unverified"] is (exists and tag != "read-test")
                assert display["focus"]["text"] == ("읽기 전용 saved note" if exists else None)
                assert [r["noted"] for r in display["rows"]] == [exists, exists]
                assert [r["status"] for r in read_records(reader, refs)["rows"]] == ([1, 1] if exists else [0, 0])
                # Wire DTOs cannot choose another file or reviewer.
                for field, value in [("path", str(original["notes"][0])), ("reviewer", "read-test"),
                                     ("target", str(temps))]:
                    reader.client.call("POST", API + "/display",
                                       dict(context=reader.context, errors=refs, **{field: value}), 400)
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
                assert set(sidecar_files()) == set(sidecars), "read created a sidecar/lock"
                assert fingerprint(inputs) == before

            # Fixed read selection must not fall through a broken adjacent note,
            # follow symlinks, read a FIFO, or grant a hardlink alias.
            for bad in ["symlink", "broken-symlink", "fifo", "hardlink"]:
                tag = "bad-" + bad
                adjacent, temporary = paths(tag)["notes"]
                temporary.write_bytes(original["notes"][0].read_bytes())
                if bad == "symlink":
                    adjacent.symlink_to(temporary)
                elif bad == "broken-symlink":
                    adjacent.symlink_to(work / "missing")
                elif bad == "fifo":
                    os.mkfifo(adjacent)
                else:
                    os.link(temporary, adjacent)
                reader = session(read_reviewer=tag)
                assert reader.display(refs, focus=refs[0], code=400)["error"] == "invalid_drc_request"
                close(reader)
                adjacent.unlink()
                temporary.unlink()
            reader = session(read_reviewer="temporary-read")
            reader.display(refs, focus=refs[0])
            target = paths("temporary-read")["notes"][1]
            content = target.read_bytes()
            target.chmod(0o600)
            target.write_bytes(content.replace(b"saved note", b"edited externally"))
            assert reader.display(refs, focus=refs[0], code=409)["error"] == "review_changed"
            close(reader)

            # A fresh adjacent ICE is selected only for explicit read-reviewer
            # intent. Same sidecar names/binding as an explicit pack, no writes.
            for tag in ["read-test", "legacy-read", "mixed"]:
                reader = session(database=db, read_reviewer=tag)
                catalog = reader.client.call("GET", "/api/v1/drc")
                assert catalog["drc"]["metadata"]["format"] == "ice"
                assert catalog["drc"]["metadata"]["review_cache"] == "cache"
                assert reader.display(refs, refs[0])["focus"]["text"] == "읽기 전용 saved note"
                assert [r["status"] for r in read_records(reader, refs)["rows"]] == [1, 1]
                close(reader)
            reader = session(database=db)
            metadata = reader.client.call("GET", "/api/v1/drc")["drc"]["metadata"]
            assert metadata["format"] == "ascii" and metadata["review_cache"] is None
            reader.client.call("GET", API, code=403)
            close(reader)

            def fallback(expected):
                files = [source, db] + sidecar_files()
                if pack.is_file():
                    files.append(pack)
                snapshot = fingerprint(files)
                names = set(work.iterdir()) | set(temps.iterdir())
                reader = session(database=db, read_reviewer="read-test")
                catalog = reader.client.call("GET", "/api/v1/drc")
                metadata = catalog["drc"]["metadata"]
                assert metadata["format"] == "ascii" and metadata["review_cache"] == expected
                assert catalog["notes"] is None and catalog["waives"] is None
                assert not metadata["waives"]
                assert str(work) not in str(catalog) and str(temps) not in str(catalog)
                reader.client.call("GET", API, code=403)
                reader.client.call("POST", API + "/display", dict(context=reader.context, errors=refs), 403)
                assert [r["status"] for r in read_records(reader, refs)["rows"]] == [0, 0]
                close(reader)
                assert fingerprint(files) == snapshot, "fallback changed an input"
                assert set(work.iterdir()) | set(temps.iterdir()) == names, "fallback created files"

            packed_bytes, db_bytes = pack.read_bytes(), db.read_bytes()
            db_stat = db.stat()
            # GTK oracle: source-size and integer-second mtime, not a full hash.
            assert isinstance(drc.load_db(str(db)), drc.IcePack)
            os.utime(db, ns=(db_stat.st_atime_ns, db_stat.st_mtime_ns + 2_000_000_000))
            assert not isinstance(drc.load_db(str(db)), drc.IcePack)
            fallback("ignored")
            os.utime(db, ns=(db_stat.st_atime_ns, db_stat.st_mtime_ns))
            db.write_bytes(db_bytes + b"\n")
            os.utime(db, ns=(db_stat.st_atime_ns, db_stat.st_mtime_ns))
            fallback("ignored")
            db.write_bytes(db_bytes)
            os.utime(db, ns=(db_stat.st_atime_ns, db_stat.st_mtime_ns))
            for corrupt in [b"broken", packed_bytes[:12], packed_bytes[:8] + (1).to_bytes(4, "little")]:
                pack.write_bytes(corrupt)
                assert not isinstance(drc.load_db(str(db)), drc.IcePack)
                fallback("ignored")
            pack.unlink()
            fallback("missing")
            # FIFO/directory candidates are rejected without blocking and the
            # parser uses ASCII. Never run GTK's blocking file probe on a FIFO.
            os.mkfifo(pack)
            fallback("ignored")
            pack.unlink()
            pack.mkdir()
            fallback("ignored")
            pack.rmdir()
            # Scope rejection precedes Pack::open; no new root or browse grant.
            outside = temps / "not-authorized.ice"
            outside.write_bytes(packed_bytes)
            pack.symlink_to(outside)
            result = subprocess.run([str(APP), "view", str(source), "--drc", str(db),
                                     "--floe-reviewer", "read-test", "--no-open"],
                                    capture_output=True, text=True, timeout=10,
                                    env=dict(os.environ, PATH=""))
            assert result.returncode == 2 and "outside approved roots" in result.stderr
            pack.unlink()
            outside.unlink()
            # Fractional ASCII vertices stay exact through the fallback route.
            fractional = work / "fractional.db"
            fractional.write_text("TOP 1\nR\np 1 2\n.125 -.5\n.375 .5\n")
            reader = session(database=fractional, read_reviewer="read-test")
            c = reader.context
            geometry = reader.client.call("POST", "/api/v1/drc/" + c["drc_id"] + "/read",
                                          dict(view_id=c["view_id"], revision=c["revision"],
                                               body=dict(kind="geometry", check="0", error="0", start="0", limit=2)))
            assert geometry["points_um"] == [["0.125", "-0.5"], ["0.375", "0.5"]]
            assert "points_dbu" not in geometry
            close(reader)
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
    print("WEB READ REVIEWER: ALL OK (explicit/fresh-cache reads; stale/corrupt/missing/nonregular ASCII fallback + notices; GTK-derived sidecars; scope/alias rejection; fractional geometry; no writes/locks; native shutdown)")


if __name__ == "__main__":
    main(sys.argv[1])
