#!/usr/bin/env python3
"""Owner waive API gate: private synthetic data, actual Rust runtime/Python oracle."""
import fcntl
import os
from pathlib import Path
from cache_test_paths import vfs_cache, drc_pack
import subprocess
import sys
import tempfile

from validate_web_drc_notes import Session, fingerprint, drc
from validate_web_cli import Client, INDEX, read_json, wait

API = "/api/v1/drc/review/waives"
REFS = [dict(check="0", error="0"), dict(check="0", error="1")]
ROUTES = [("GET", ""), ("POST", ""), ("POST", "/read"), ("POST", "/prepare"),
          ("POST", "/revoke"), ("GET", "/1"), ("POST", "/1/cancel")]


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-owner-waives-") as td:
        root = Path(td)
        temps = root / "runtime"
        temps.mkdir()
        source = root / "synthetic.oas"
        source.write_bytes(Path(fixture).read_bytes())
        db = root / "synthetic.db"
        db.write_text("TOP 1000\nWIDTH\n3 3 0\n" + "".join(
            "p %d 4\n%d 0\n%d 0\n%d 20\n%d 20\n" % (i + 1, i * 40, i * 40 + 20, i * 40 + 20, i * 40)
            for i in range(3)))
        for args in [["vfs", source, str(vfs_cache(source)), "--jobs", "2"], ["drc", db, "--jobs", "2"]]:
            p = subprocess.run([str(INDEX), *map(str, args)], capture_output=True, timeout=30)
            assert p.returncode == 0, p.stderr
        pack = drc_pack(db)
        inputs = [source, db, pack, *sorted(vfs_cache(source).glob("design.*"))]
        before = fingerprint(inputs)
        sessions = []

        def session(tag, enabled=False, waives=None):
            s = Session(source, pack, temps, tag, root / (str(len(sessions)) + ".session"),
                        edit_waives=enabled, waives=waives)
            sessions.append(s)
            return s

        def close(s):
            s.close()
            sessions.remove(s)

        def snapshot(s, refs=REFS, code=200):
            return s.client.call("POST", API + "/read", dict(context=s.context, errors=refs), code)

        def prepare(s, snap, waived, code=200):
            return s.client.call("POST", API + "/prepare",
                                 dict(context=s.context, token=snap["token"], waived=waived), code)

        def finished(s, seq):
            return wait(lambda: (lambda v: v if v["phase"] in ("succeeded", "failed", "cancelled") else None)(
                s.client.call("GET", API + "/" + str(seq))), s.proc)

        def adopt(s):
            d = wait(lambda: (lambda d: d if d["phase"] == "ready" else None)(
                s.client.call("GET", "/api/v1/drc")["drc"]), s.proc)
            assert d["id"] == s.context["drc_id"]
            s.context["revision"] = d["revision"]

        def statuses(s, context=None):
            c = context or s.context
            return s.client.call("POST", "/api/v1/drc/" + c["drc_id"] + "/read",
                                 dict(view_id=c["view_id"], revision=c["revision"],
                                      body=dict(kind="records", check="0", errors=["0", "1"])))

        try:
            # No reviewer and notes-only remain read-only for every waive route.
            for tag in [None, "notes-only"]:
                s = session(tag)
                for method, suffix in ROUTES:
                    s.client.call(method, API + suffix, {} if method == "POST" else None, 403)
                close(s)
            assert not list(root.glob("*.waive.*"))

            tag = "fixed-owner"
            target = root / (".synthetic.db.waive." + tag)
            lock = Path(str(target) + ".lock")
            s = session(tag, True)
            c = s.client
            unauthenticated = Client(read_json(s.session_path))
            for method, suffix in ROUTES:
                unauthenticated.call(method, API + suffix, {} if method == "POST" else None, 401)
            assert c.call("GET", API)["kind"] == "drc_waive"
            assert c.call("GET", API)["autosave"] is False
            for fields in [dict(path="/etc/passwd"), dict(reviewer="foreign"), dict(gids=["1"])]:
                c.call("POST", API + "/read", dict(context=s.context, errors=REFS, **fields), 400)
            snapshot(s, [], 413)
            snapshot(s, [dict(check="0", error="00")], 400)
            snap = snapshot(s, REFS + REFS)
            assert snap["selected_count"] == "2" and snap["waived_count"] == "0"
            assert snap["exists"] is False and not target.exists() and not lock.exists()
            c.call("POST", API + "/prepare", dict(context=s.context, token=snap["token"], text="not a waive"), 400)
            draft = prepare(s, snap, True)
            assert draft["changed_count"] == "2" and draft["waived"] is True
            assert not target.exists() and not lock.exists()
            old = dict(s.context)
            approved = s.request(draft, 1)
            c.call("POST", API, dict(approved, approve=False), 400)
            c.call("POST", API, approved, 202)
            result = finished(s, 1)
            assert result["published"] is True and result["reader_applied"] is True, result
            assert result["phase"] == "succeeded" and result["reader_revision"] != old["revision"]
            assert c.call("POST", API, approved, 202) == result  # old context replay, no second write
            c.call("POST", API, dict(approved, confirm_legacy=True), 409)
            c.call("POST", "/api/v1/drc/" + old["drc_id"] + "/read",
                   dict(view_id=old["view_id"], revision=old["revision"], body=dict(kind="rule", check="0")), 409)
            adopt(s)
            assert [r["status"] for r in statuses(s)["rows"]] == [1, 1]
            assert target.stat().st_mode & 0o777 == 0o600 and lock.stat().st_size == 0
            os.environ["FLOE_REVIEWER"] = tag
            oracle = drc.IcePack(str(pack))
            assert [oracle.get_status(0, i) for i in (0, 1)] == [1, 1]
            oracle.close()
            assert fingerprint(inputs) == before

            # A lock conflict reports no publication; old statuses remain.
            draft = prepare(s, snapshot(s), False)
            with lock.open("r+b") as held:
                fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
                c.call("POST", API, s.request(draft, 2), 202)
                failed = finished(s, 2)
                assert failed["phase"] == "failed" and failed["published"] is False, failed
            adopt(s)
            assert [r["status"] for r in statuses(s)["rows"]] == [1, 1]
            draft = prepare(s, snapshot(s, REFS[:1]), False)
            c.call("POST", API, s.request(draft, 3), 202)
            assert finished(s, 3)["reader_applied"] is True
            adopt(s)
            assert [r["status"] for r in statuses(s)["rows"]] == [0, 1]
            # A replacement after snapshot read must not be overwritten. Even
            # restoring the old inode changes ctime: reopen explicitly afterward.
            stale = snapshot(s)
            saved = root / "saved-native-sidecar"
            target.rename(saved)
            target.write_bytes(saved.read_bytes())
            replacement = target.read_bytes()
            prepare(s, stale, False, 409)
            assert target.read_bytes() == replacement
            os.replace(saved, target)
            snapshot(s, code=422)
            close(s)

            # Reopen automatically reads only the opted-in fixed tag's file.
            s = session(tag, True)
            assert [r["status"] for r in statuses(s)["rows"]] == [0, 1]
            close(s)

            # Existing read paths never confer authority to a different target.
            try:
                session("different", True, target)
                raise AssertionError("foreign read sidecar gained writes")
            except AssertionError as e:
                assert "waive write target must match the registered read sidecar" in str(e), e
            assert not (root / ".synthetic.db.waive.different").exists()

            # Legacy/no binding needs explicit adoption; unrelated statuses stay.
            legacy = root / ".synthetic.db.waive.legacy-owner"
            legacy_bytes = bytearray(target.read_bytes())
            legacy_bytes[42] = 239  # third, unselected error's reserved status
            legacy.write_bytes(legacy_bytes)
            s = session("legacy-owner", True)
            draft = prepare(s, snapshot(s, REFS[:1]), True)
            assert draft["legacy_unverified"] is True
            s.client.call("POST", API, s.request(draft, 1), 400)
            s.client.call("POST", API, s.request(draft, 1, True), 202)
            result = finished(s, 1)
            assert result["published"] is True and result["reader_applied"] is True, result
            adopt(s)
            assert [r["status"] for r in statuses(s)["rows"]] == [1, 1]
            assert legacy.read_bytes()[42] == 239
            close(s)
            assert fingerprint(inputs) == before
        finally:
            for s in sessions:
                s.close()
    print("RUST WEB DRC WAIVES: ALL OK (explicit opt-in, preview/approve, same-file refresh, replay/stale, lock/legacy, reopen, Python oracle)")


if __name__ == "__main__":
    main(sys.argv[1])
