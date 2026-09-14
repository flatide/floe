#!/usr/bin/env python3
"""Owner note HTTP publication on private synthetic data; runtime PATH is empty."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request

from validate_web_cli import APP, INDEX, RENDERD, Client, read_json, wait
from validate_drc_ice import DB

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import drc

API = "/api/v1/drc/review/notes"


def fingerprint(paths):
    return {str(p): (p.stat().st_mtime_ns, p.stat().st_ctime_ns, p.stat().st_mode,
                    hashlib.sha256(p.read_bytes()).hexdigest()) for p in paths}


class Session:
    def __init__(self, source, pack, temps, reviewer, session_path, *, edit_waives=False, waives=None):
        self.session_path = session_path
        env = dict(os.environ, PATH="", TMPDIR=str(temps), FLOE_INDEX_BIN=str(INDEX),
                   FLOE_RENDERD_BIN=str(RENDERD), FLOE_REVIEWER="must-not-be-used", FLOE_FILL_EDIT="")
        args = [str(APP), "view", str(source), "--no-open", "--session-file", str(session_path),
                "--drc", str(pack), "--jobs", "2", "--raster-jobs", "1", "--budget-mb", "64",
                "--no-labels", "--frame-cache", "off"]
        if reviewer is not None:
            args += ["--drc-reviewer", reviewer]
        if edit_waives:
            args += ["--drc-edit-waives"]
        if waives is not None:
            args += ["--drc-waives", str(waives)]
        self.proc = subprocess.Popen(args, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            self.connect()
        except BaseException:
            if self.proc.poll() is None:
                self.proc.terminate()
            try:
                self.proc.communicate(timeout=15)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.communicate(timeout=5)
            raise

    def connect(self):
        self.client = Client(wait(lambda: read_json(self.session_path), self.proc))
        self.client.call("GET", API, code=401)
        self.client.call("POST", API + "/read", {}, code=401)
        self.client.call("POST", API + "/display", {}, code=401)
        self.client.login()
        catalog = wait(lambda: (lambda c: c if c["phase"] == "ready" else None)(
            self.client.call("GET", "/api/v1/drc")["drc"]), self.proc)
        startup = self.client.call("GET", "/api/v1/startup")["request"]
        startup["body"]["pixels"] = [257, 191]
        self.client.call("POST", "/api/v1/operations", startup, 202)
        opened = self.client.finished(1, self.proc)
        assert opened["phase"] == "succeeded", opened
        self.context = dict(drc_id=catalog["id"], revision=catalog["revision"], view_id=opened["view_id"])

    def close(self):
        if self.proc.poll() is None:
            self.client.call("DELETE", "/api/v1/session", code=204)
        out, err = self.proc.communicate(timeout=15)
        assert self.proc.returncode == 0, (out, err)
        assert self.client.token not in out + err
        assert not self.session_path.exists()

    def read(self, refs, code=200):
        return self.client.call("POST", API + "/read", dict(context=self.context, errors=refs), code)

    def prepare(self, snapshot, text, code=200):
        return self.client.call("POST", API + "/prepare",
                                dict(context=self.context, token=snapshot["token"], text=text), code)

    def display(self, refs, focus=None, code=200):
        return self.client.call("POST", API + "/display",
                                dict(context=self.context, errors=refs, focus=focus), code)

    def display_padded(self, ref, padding, code=200):
        c = self.client
        payload = json.dumps(dict(context=self.context, errors=[ref])).encode() + b" " * padding
        request = urllib.request.Request(c.origin + API + "/display", data=payload, method="POST",
                                         headers={"Origin": c.origin, "X-Floe-CSRF": c.csrf,
                                                  "Content-Type": "application/json"})
        try:
            response = c.opener.open(request, timeout=8)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            assert response.status == code, (response.status, response.read())
            return json.loads(response.read())

    def request(self, draft, seq, confirm=False):
        return dict(context=self.context, token=draft["token"], seq=str(seq),
                    approve=True, confirm_legacy=confirm)

    def finished(self, seq):
        return wait(lambda: (lambda v: v if v["phase"] in ("succeeded", "failed", "cancelled") else None)(
            self.client.call("GET", API + "/" + str(seq))), self.proc)


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-owner-notes-") as td:
        work = Path(td)
        temps = work / "temps"
        temps.mkdir()
        source = work / "layout.oas"
        shutil.copy2(fixture, source)
        subprocess.run([str(INDEX), "vfs", str(source), str(source) + ".floe", "--jobs", "2"],
                       check=True, capture_output=True, timeout=30)
        db = work / "합성.db"
        db.write_text(DB)
        subprocess.run([str(INDEX), "drc", str(db), "--jobs", "2"], check=True, capture_output=True, timeout=30)
        pack = Path(str(db) + ".ice")
        os.environ["FLOE_REVIEWER"] = "synthetic-notes-oracle"
        oracle = drc.IcePack(str(pack))
        references = [dict(check=str(ci), error=str(ei)) for ci, c in enumerate(oracle.checks)
                      for ei in range(len(c.errors))]
        assert len(references) >= 6
        inputs = [source, db, pack] + [p for p in Path(str(source) + ".floe").rglob("*") if p.is_file()]
        before = fingerprint(inputs)
        target = work / ".합성.db.notes.fixed-owner.fe"
        lock = Path(str(target) + ".lock")
        sessions = []
        try:
            s = Session(source, pack, temps, None, work / "disabled.session")
            sessions.append(s)
            s.client.call("GET", API, code=403)
            s.read(references[:1], 403)
            s.display(references[:1], code=403)
            s.close()
            sessions.remove(s)

            s = Session(source, pack, temps, "fixed-owner", work / "enabled.session")
            sessions.append(s)
            c = s.client
            assert c.call("GET", API)["reviewer"] == "fixed-owner"
            empty = s.display(references[:3], references[0])
            assert not empty["exists"] and not empty["cache_hit"]
            assert empty["context"] == s.context and empty["review_rev"] == "0"
            assert empty["focus"]["text"] is None and all(not r["noted"] for r in empty["rows"])
            assert s.display(references[:1])["cache_hit"]
            s.display([], code=413)
            s.display(references[:1] * 513, code=413)
            s.display([dict(check="00", error="0")], code=400)
            s.display([dict(check="99999", error="0")], code=400)
            # Valid JSON above 16 KiB reaches the handler, while shared 1 MiB
            # admission remains hard. Invalid long IDs still fail pack bounds.
            assert s.display_padded(references[0], 32 * 1024)["rows"][0]["noted"] is False
            s.display_padded(references[0], 1024 * 1024, 413)
            s.display([dict(check="18446744073709551615", error="18446744073709551615")] * 512, code=400)
            for field in ("reviewer", "path", "gids", "token", "approve"):
                c.call("POST", API + "/display", dict(context=s.context, errors=references[:1], **{field: "untrusted"}), 400)
            c.call("POST", API + "/display", dict(context=dict(s.context, revision="0" * 64), errors=references[:1]), 409)
            assert not target.exists() and not lock.exists()
            base = dict(context=s.context, errors=references[:2])
            for field in ("reviewer", "path", "gids"):
                c.call("POST", API + "/read", dict(base, **{field: "untrusted"}), 400)
            s.read([dict(check="00", error="0")], 400)
            s.read([dict(check="99999", error="0")], 400)
            s.read([], 413)
            s.read(references[:1] * 5001, 413)
            c.call("POST", API + "/read", dict(base, context=dict(s.context, revision="0" * 64)), 409)
            first = s.read(references[:2])
            assert first["selected_count"] == "2" and first["text"] is None and not first["mixed"]
            assert not target.exists() and not lock.exists()
            s.prepare(first, "x" * 65537, 413)
            assert not s.display(references[:1])["exists"]
            draft = s.prepare(first, "  한글 <script> & 메모\n두 번째 줄  ")
            # Display reads do not consume an edit snapshot/prepared token.
            assert not s.display(references[:2], references[0])["exists"]
            assert draft["text"] == "한글 <script> & 메모\n두 번째 줄" and not draft["clears"]
            assert not target.exists() and not lock.exists()
            s.prepare(first, "cannot reuse snapshot", 410)
            request = s.request(draft, 1)
            c.call("POST", API, dict(request, approve=False), 400)
            c.call("POST", API, dict(request, reviewer="another"), 400)
            c.call("POST", API, dict(request, token="f" * 64), 410)
            c.call("POST", API, dict(request, context=dict(s.context, revision="f" * 64)), 409)
            c.call("POST", API, request, 202)
            outcome = s.finished(1)
            assert outcome["phase"] == "succeeded" and outcome["published"] and not outcome["outcome_unknown"], outcome
            assert target.stat().st_mode & 0o777 == 0o600 and lock.stat().st_size == 0
            published = fingerprint([target, lock])
            assert c.call("POST", API, request, 202) == outcome
            c.call("POST", API, dict(request, approve=False), 409)
            c.call("POST", API + "/1/cancel", {}, 202)
            assert c.call("GET", API + "/1") == outcome and fingerprint([target, lock]) == published
            assert oracle._parse_notes(target.read_text(), oracle.total)
            assert oracle.get_note_gid(0) == draft["text"] and oracle.get_note_gid(1) == draft["text"]
            assert oracle.get_note_gid(2) is None
            displayed = s.display(references[:3], references[1])
            assert displayed["review_rev"] == "1" and not displayed["cache_hit"]
            assert [r["noted"] for r in displayed["rows"]] == [True, True, False]
            assert displayed["focus"] == dict(references[1], text=draft["text"])
            assert not displayed["legacy_unverified"]
            cross_rule = list(reversed(references)) + [references[0]]
            shown = s.display(cross_rule, references[0])
            assert shown["rows"] == [dict(ref, noted=oracle.get_note(int(ref["check"]), int(ref["error"])) is not None)
                                     for ref in cross_rule]
            assert shown["focus"] == dict(references[0], text=draft["text"])
            assert s.display([], references[2])["focus"]["text"] is None
            assert s.display(references[:1] * 512, references[1])["cache_hit"]
            assert fingerprint([target, lock]) == published
            readback = s.read(references[:2])
            assert readback["text"] == draft["text"] and not readback["legacy_unverified"]
            mixed = s.read(references[1:3])
            assert mixed["mixed"] and mixed["text"] is None and mixed["existing_count"] == "1"
            s.prepare(readback, "older preview", 410)
            c.call("POST", API + "/revoke", dict(token=mixed["token"]), 204)
            s.prepare(mixed, "revoked", 410)

            # A foreign cooperating writer changed the expected file after read.
            stale = s.read(references[:1])
            assert s.display(references[:1])["rows"][0]["noted"]
            original = target.read_bytes()
            target.write_bytes(original + b"\n# external edit\n")
            s.display(references[:1], code=409)  # no implicit reparse/adoption
            s.prepare(stale, "must not overwrite", 409)
            target.write_bytes(original)
            # One prepared draft, lock contention fails without replacing bytes.
            locked = s.prepare(s.read(references[:1]), "lock conflict")
            with lock.open("rb") as held:
                fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
                c.call("POST", API, s.request(locked, 2), 202)
                failed = s.finished(2)
                assert failed["phase"] == "failed" and failed["error"] == "review_changed", failed
                assert not failed["published"] and not failed["outcome_unknown"]
            assert target.read_bytes() == original
            # Maximum note size survives escaping beyond ordinary 16 KiB bodies.
            s.prepare(s.read(references[:1]), "invalid\x01control", 400)
            text = "\\" * 65536
            large = s.prepare(s.read(references[:1]), text)
            assert large["text"] == text
            c.call("POST", API, s.request(large, 3), 202)
            assert s.finished(3)["phase"] == "succeeded"
            assert s.read(references[:1])["text"] == text
            assert s.display([], references[0])["focus"]["text"] == text
            clear = s.prepare(s.read(references[:2]), "  \n ")
            assert clear["clears"] and clear["text"] == ""
            c.call("POST", API, s.request(clear, 4), 202)
            assert s.finished(4)["phase"] == "succeeded"
            assert target.exists() and "floe_note=" not in target.read_text()
            cleared = s.display(references[:2], references[0])
            assert cleared["review_rev"] == "3" and not cleared["cache_hit"]
            assert not any(r["noted"] for r in cleared["rows"]) and cleared["focus"]["text"] is None
            assert s.read(references[:2])["text"] is None
            # Leave an unapproved snapshot; logout must release its pack lease.
            s.close()
            sessions.remove(s)
            assert fingerprint(inputs) == before

            legacy = work / ".합성.db.notes.legacy-owner.fe"
            legacy.write_bytes(target.read_bytes())  # new inode, no binding xattr
            s = Session(source, pack, temps, "legacy-owner", work / "legacy.session")
            sessions.append(s)
            draft = s.prepare(s.read(references[:1]), "explicit adoption")
            assert draft["legacy_unverified"]
            s.client.call("POST", API, s.request(draft, 1), 400)
            assert "floe_note=" not in legacy.read_text()
            s.client.call("POST", API, s.request(draft, 1, True), 202)
            assert s.finished(1)["phase"] == "succeeded"
            assert not s.read(references[:1])["legacy_unverified"]
            s.close()
            sessions.remove(s)
            assert fingerprint(inputs) == before and not list(temps.iterdir())
            # A sidecar-shaped launcher credential is protected, never parsed
            # or echoed as a note. It is a NEW private test session only.
            credential = work / ".합성.db.notes.protected-owner.fe"
            s = Session(source, pack, temps, "protected-owner", credential)
            sessions.append(s)
            s.read(references[:1], 400)
            s.display(references[:1], code=400)
            s.close()
            sessions.remove(s)
            assert not credential.exists()
            assert not Path(str(credential) + ".lock").exists()
            assert fingerprint(inputs) == before and not list(temps.iterdir())
            assert not list(work.glob(".floe-review-*"))

            # A prepared edit cannot publish to an externally replaced pack,
            # even when the replacement has identical bytes/legacy headers.
            replaced_pack = work / "replacement.db.ice"
            shutil.copyfile(pack, replaced_pack)
            s = Session(source, replaced_pack, temps, "fixed-owner", work / "replacement.session")
            sessions.append(s)
            first = s.prepare(s.read(references[:1]), "original run")
            s.client.call("POST", API, s.request(first, 1), 202)
            assert s.finished(1)["phase"] == "succeeded"
            stale = s.prepare(s.read(references[:1]), "not the replacement run")
            replacement_target = work / ".replacement.db.notes.fixed-owner.fe"
            saved = replacement_target.read_bytes()
            next_pack = work / "next.ice"
            shutil.copyfile(pack, next_pack)
            next_pack.replace(replaced_pack)
            s.client.call("POST", API, s.request(stale, 2), 202)
            failed = s.finished(2)
            assert failed["phase"] == "failed" and failed["error"] == "drc_changed_or_corrupt", failed
            assert replacement_target.read_bytes() == saved
            s.read(references[:1], 422)
            s.display(references[:1], code=422)
            s.close()
            sessions.remove(s)

            # Rebuild consumes an unapproved draft and its read lease. Accepted
            # note receipts remain queryable across the DRC identity change.
            rebuild_db = work / "rebuild.db"
            rebuild_db.write_text(DB)
            s = Session(source, rebuild_db, temps, "build-owner", work / "rebuild.session")
            sessions.append(s)
            s.read(references[:1], 400)  # explicit pack is required, no implicit lookup
            s.display(references[:1], code=400)
            def build(seq, force):
                req = dict(s.context, seq=str(seq), approve=True, force=force, jobs=2)
                s.client.call("POST", "/api/v1/drc/builds", req, 202)
                done = wait(lambda: (lambda v: v if v["phase"] in ("succeeded", "failed", "cancelled") else None)(
                    s.client.call("GET", "/api/v1/drc/builds/" + str(seq))), s.proc)
                assert done["phase"] == "succeeded", done
                catalog = s.client.call("GET", "/api/v1/drc")["drc"]
                s.context = dict(s.context, drc_id=catalog["id"], revision=catalog["revision"])
            build(1, False)
            draft = s.prepare(s.read(references[:1]), "before rebuild")
            accepted = s.request(draft, 1)
            s.client.call("POST", API, accepted, 202)
            receipt = s.finished(1)
            assert receipt["phase"] == "succeeded"
            assert s.display(references[:1])["rows"][0]["noted"]
            old = s.request(s.prepare(s.read(references[:1]), "discard on rebuild"), 2)
            assert s.display(references[:1])["rows"][0]["noted"]  # cached model and draft both hold leases
            build(2, True)
            s.client.call("POST", API, old, 409)
            assert s.client.call("POST", API, accepted, 202) == receipt
            assert s.client.call("GET", API + "/1") == receipt
            s.read(references[:1], 422)  # old run binding requires explicit import, never auto-adopt
            s.display(references[:1], code=422)
            s.close()
            sessions.remove(s)
            assert fingerprint(inputs) == before and not list(temps.iterdir())
        finally:
            oracle.close()
            for s in sessions:
                if s.proc.poll() is None:
                    s.proc.send_signal(signal.SIGTERM)
                    try:
                        s.proc.communicate(timeout=15)
                    except subprocess.TimeoutExpired:
                        s.proc.kill()
                        s.proc.communicate(timeout=5)
    print("WEB DRC NOTES: ALL OK (fixed owner, auth, references, preview/approve/replay, conflicts, legacy, maximum text, tombstone, no input writes/reap)")
    print("WEB DRC NOTE DISPLAY: ALL OK (read-only badges/focus, bounded rows/text, cache hits, editor token preservation, publication invalidation, external-change rejection, identity/rebuild, no input writes/reap)")


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
