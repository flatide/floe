#!/usr/bin/env python3
"""Owner whole-review transfers against private Python/native synthetic files."""
import json
import os
from pathlib import Path
from cache_test_paths import vfs_cache, drc_pack
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request

from validate_web_cli import Client, INDEX, read_json, wait
from validate_web_drc_notes import Session, fingerprint, drc

BASE = "/api/v1/drc/review/"
CHUNK = 1024 * 1024


def raw(client, method, path, data=None, headers=None, code=200, csrf=True):
    h = {"Origin": client.origin}
    if csrf and client.csrf:
        h["X-Floe-CSRF"] = client.csrf
    h.update(headers or {})
    request = urllib.request.Request(client.origin + path, data=data, method=method, headers=h)
    try:
        response = client.opener.open(request, timeout=10)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        body = response.read()
        assert response.status == code, (response.status, body[:2000])
        assert response.headers.get("cache-control") == "no-store"
        assert response.headers.get("x-content-type-options") == "nosniff"
        return body, response.headers


class Transfer:
    def __init__(self, session, kind):
        self.s = session
        self.c = session.client
        self.api = BASE + kind
        self.root = self.api + "/transfer"

    def status(self):
        return self.c.call("GET", self.root)

    def seq(self):
        return str(int(self.status()["operations"]["last_seq"]) + 1)

    def finished(self, seq):
        return wait(lambda: (lambda v: v if v["phase"] in ("succeeded", "failed", "cancelled") else None)(
            self.c.call("GET", self.root + "/" + str(seq))), self.s.proc, 30)

    def request(self, action, **fields):
        return dict(seq=self.seq(), context=dict(self.s.context), action=action, **fields)

    def start(self, action, **fields):
        req = self.request(action, **fields)
        self.c.call("POST", self.root, req, 202)
        return req, self.finished(req["seq"])

    def chunk(self, token, at, payload, *, seq=None, context=None, code=202):
        context = context or self.s.context
        seq = seq or self.seq()
        headers = {"Content-Type": "application/octet-stream", "X-Floe-Transfer-Token": token,
                   "X-Floe-Transfer-Seq": seq, "X-Floe-Transfer-Offset": str(at),
                   "X-Floe-DRC": context["drc_id"], "X-Floe-Revision": context["revision"],
                   "X-Floe-View": context["view_id"]}
        body, _ = raw(self.c, "POST", self.root + "/chunk", payload, headers, code)
        return seq, json.loads(body)

    def upload(self, data):
        req, result = self.start("import", bytes=str(len(data)))
        assert result["phase"] == "succeeded", result
        token = result["upload"]["token"]
        for at in range(0, len(data), CHUNK):
            seq, _ = self.chunk(token, at, data[at:at + CHUNK])
            assert self.finished(seq)["upload"]["received"] == str(min(at + CHUNK, len(data)))
        return token

    def export(self):
        _, result = self.start("export")
        assert result["phase"] == "succeeded", result
        return result["artifact"]

    def path(self, artifact):
        return self.api + "/artifacts/" + artifact["id"]

    def release(self, artifact):
        self.c.call("DELETE", self.path(artifact), code=204)

    def drain(self):
        wait(lambda: self.status()["usage"]["entries"] == 0, self.s.proc)

    def approve(self, preview):
        seq = str(int(self.c.call("GET", self.api)["operations"]["last_seq"]) + 1)
        req = dict(seq=seq, context=dict(self.s.context), token=preview["token"], approve=True, confirm_legacy=True)
        self.c.call("POST", self.api, dict(req, approve=False), 400)
        self.c.call("POST", self.api, dict(req, confirm_legacy=False), 400)
        self.c.call("POST", self.api, req, 202)
        result = wait(lambda: (lambda v: v if v["phase"] in ("succeeded", "failed", "cancelled") else None)(
            self.c.call("GET", self.api + "/" + seq)), self.s.proc)
        assert result["phase"] == "succeeded" and result["published"] is True, result
        assert self.c.call("POST", self.api, req, 202) == result
        self.c.call("POST", self.api, dict(req, confirm_legacy=False), 409)
        if self.api.endswith("waives"):
            assert result["reader_applied"] is True
            catalog = wait(lambda: (lambda v: v if v["phase"] == "ready" else None)(
                self.c.call("GET", "/api/v1/drc")["drc"]), self.s.proc)
            self.s.context["revision"] = catalog["revision"]
        return result


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-review-transfer-api-") as td:
        root = Path(td)
        temps = root / "runtime"
        temps.mkdir()
        source = root / "synthetic.oas"
        source.write_bytes(Path(fixture).read_bytes())
        db = root / "synthetic.db"
        n = 6007  # Whole waive import must not be limited to selected EDIT_ITEMS.
        db.write_text("TOP 1000\nWIDTH\n%d %d 0\n" % (n, n) + "".join(
            "p %d 4\n%d 0\n%d 0\n%d 20\n%d 20\n" % (i + 1, i * 40, i * 40 + 20, i * 40 + 20, i * 40)
            for i in range(n)))
        for args in [["vfs", source, str(vfs_cache(source)), "--jobs", "2"], ["drc", db, "--jobs", "2"]]:
            run = subprocess.run([str(INDEX), *map(str, args)], capture_output=True, timeout=30)
            assert run.returncode == 0, run.stderr
        pack = drc_pack(db)
        os.environ["FLOE_REVIEWER"] = "synthetic-python-oracle"
        oracle = drc.IcePack(str(pack))
        # >1MiB FE upload and >2MiB download, never one large JSON body.
        text = "한글 <script>\\n" + "x" * 59900
        oracle.set_note([0], "seed")
        fp = next(line.removeprefix("floe_pack=") for line in oracle._serialize_notes().splitlines() if line.startswith("floe_pack="))
        notes_input = ("floe_pack=" + fp + "\n" + "".join(f"floe_note={i}|{text}\n" for i in range(20))).encode()
        assert len(notes_input) > CHUNK
        note_file = root / "input.notes.fe"
        note_file.write_bytes(notes_input)
        oracle.note_import(str(note_file))
        expected_notes = oracle._serialize_notes().encode()
        assert len(expected_notes) > 2 * CHUNK
        for i, status in [(0, 1), (1, 255), (6006, 2)]:
            oracle.set_status(0, i, status)
        waive_file = root / "input.waive"
        oracle.waive_export(str(waive_file))
        expected_waives = waive_file.read_bytes()
        waive_input = expected_waives[:40+n] + bytes([255]) * 4
        oracle.close()
        inputs = [source, db, pack, note_file, waive_file, *sorted(vfs_cache(source).glob("design.*"))]
        before = fingerprint(inputs)
        sessions = []

        def session(tag, enabled=False):
            s = Session(source, pack, temps, tag, root / f"{len(sessions)}.session", edit_waives=enabled)
            sessions.append(s)
            return s

        def close(s):
            s.close()
            sessions.remove(s)

        try:
            for tag, kinds in [(None, ["notes", "waives"]), ("notes-only", ["waives"])]:
                s = session(tag)
                for k in kinds:
                    api = BASE + k
                    for method, path in [("GET", "/transfer"), ("POST", "/transfer"), ("POST", "/transfer/chunk"),
                                         ("GET", "/transfer/1"), ("POST", "/transfer/1/cancel"),
                                         ("GET", "/artifacts/1"), ("GET", "/artifacts/1/download"), ("DELETE", "/artifacts/1")]:
                        s.client.call(method, api + path, {} if method == "POST" else None, 403)
                close(s)
            s = session("fixed-owner", True)
            c = s.client
            unauth = Client(read_json(s.session_path))
            for k in ["notes", "waives"]:
                api = BASE + k
                for method, path in [("GET", "/transfer"), ("POST", "/transfer"), ("POST", "/transfer/chunk"),
                                     ("GET", "/transfer/1"), ("POST", "/transfer/1/cancel"),
                                     ("GET", "/artifacts/1"), ("GET", "/artifacts/1/download"), ("DELETE", "/artifacts/1")]:
                    unauth.call(method, api + path, {} if method == "POST" else None, 401)
            t = Transfer(s, "notes")
            target = root / ".synthetic.db.notes.fixed-owner.fe"
            assert not target.exists()
            for key in ["path", "reviewer", "gids", "approve", "filename"]:
                c.call("POST", t.root, dict(t.request("export"), **{key: "bad"}), 400)
            c.call("POST", t.root, t.request("import", bytes=str(512 * CHUNK + 1)), 413)
            c.call("POST", t.root, t.request("import", bytes="01"), 400)
            # Export is read-only, keeps an existing edit snapshot/preview, and
            # does not create the default sidecar/lock.
            snap = s.read([dict(check="0", error="0")])
            a = t.export()
            s.prepare(snap, "draft survives export")
            b = t.export()
            _, full = t.start("export")
            assert full["phase"] == "failed" and full["error"] == "drc_busy", full
            assert not target.exists() and not Path(str(target) + ".lock").exists()
            for artifact in [a, b]:
                data, h = raw(c, "GET", t.path(artifact) + "/download")
                assert b"floe_pack=" in data and b"floe_note=" not in data
                assert int(h["content-length"]) == len(data)
                t.release(artifact)
            t.drain()

            begin, state = t.start("import", bytes=str(len(notes_input)))
            token = state["upload"]["token"]
            assert c.call("POST", t.root, begin, 202) == state
            c.call("POST", t.root, dict(begin, bytes=str(len(notes_input) - 1)), 409)
            c.call("POST", t.root, t.request("prepare", token=token), 409)
            t.chunk(token, 1, notes_input[:50], code=409)
            t.chunk(token, 0, b"x" * (CHUNK + 1), code=413)
            seq, _ = t.chunk(token, 0, notes_input[:CHUNK])
            first = t.finished(seq)
            assert first["upload"]["received"] == str(CHUNK)
            assert t.chunk(token, 0, notes_input[:CHUNK], seq=seq)[1] == first
            t.chunk(token, 0, b"z" + notes_input[1:CHUNK], seq=seq, code=409)
            seq2, _ = t.chunk(token, CHUNK, notes_input[CHUNK:])
            assert t.finished(seq2)["upload"]["received"] == str(len(notes_input))
            prep_req, ready = t.start("prepare", token=token)
            preview = ready["preview"]
            assert preview["action"] == "replace_all" and preview["members"] == "20" and preview["groups"] == "20"
            assert preview["legacy_unverified"] and not preview["clears"] and not target.exists()
            assert c.call("POST", t.root, prep_req, 202) == ready
            assert t.chunk(token, 0, notes_input[:CHUNK], seq=seq)[1] == first
            t.approve(preview)
            assert target.read_bytes() == expected_notes
            t.drain()
            assert s.display([dict(check="0", error="0")])["rows"][0]["noted"]
            a = t.export()
            assert int(a["bytes"]) == len(expected_notes)
            info = c.call("GET", t.path(a))
            assert info["context"] == s.context and info["name"].endswith(".fe")
            raw(c, "GET", t.path(a) + "/download", headers={"Range": "bytes=0-3"}, code=416)
            raw(c, "GET", t.path(a) + "/download", code=401, csrf=False)
            data, _ = raw(c, "GET", t.path(a) + "/download")
            assert data == expected_notes
            data, _ = raw(c, "POST", t.path(a) + "/download", ("csrf=" + c.csrf).encode(),
                          {"Content-Type": "application/x-www-form-urlencoded"}, csrf=False)
            assert data == expected_notes
            raw(c, "POST", t.path(a) + "/download", b"csrf=" + b"0" * 64,
                {"Content-Type": "application/x-www-form-urlencoded"}, code=401, csrf=False)
            t.release(a)
            c.call("GET", t.path(a), code=410)
            t.drain()
            # External target changes after import initialization are conflicts,
            # never an overwrite with a stale whole-review model.
            token = t.upload(("floe_pack=" + fp + "\n").encode())
            target.write_bytes(expected_notes)
            _, stale = t.start("prepare", token=token)
            assert stale["phase"] == "failed" and stale["error"] == "review_changed", stale
            assert target.read_bytes() == expected_notes
            t.drain()
            old_export = t.export()
            token = t.upload(("floe_pack=" + fp + "\n").encode())
            _, ready = t.start("prepare", token=token)
            assert ready["preview"]["clears"] and ready["preview"]["members"] == "0"
            t.approve(ready["preview"])
            assert b"floe_note=" not in target.read_bytes()
            c.call("GET", t.path(old_export), code=410)
            t.drain()

            w = Transfer(s, "waives")
            wt = root / ".synthetic.db.waive.fixed-owner"
            token = w.upload(waive_input)
            _, ready = w.start("prepare", token=token)
            assert ready["preview"]["waived_count"] == "1" and not wt.exists()
            w.approve(ready["preview"])
            assert wt.read_bytes() == expected_waives
            w.drain()
            a = w.export()
            assert raw(c, "GET", w.path(a) + "/download")[0] == expected_waives
            # Artifact inventory must outlive the replay window, otherwise
            # chunked uploads can hide a charged export after page reload.
            _, began = w.start("import", bytes=str(len(waive_input)))
            token = began["upload"]["token"]
            for at in range(0, len(waive_input), 128):
                seq, _ = w.chunk(token, at, waive_input[at:at + 128])
                assert w.finished(seq)["phase"] == "succeeded"
            state = w.status()
            assert len(state["operations"]["history"]) == 32
            assert all(op["action"] == "chunk" for op in state["operations"]["history"])
            assert [item["id"] for item in state["artifacts"]] == [a["id"]]
            assert raw(c, "GET", w.path(a) + "/download")[0] == expected_waives
            c.call("POST", w.api + "/revoke", {"token": token}, 204)
            w.release(a)
            w.drain()
            token = w.upload(b"BAD!" + waive_input[4:])
            _, bad = w.start("prepare", token=token)
            assert bad["phase"] == "failed" and bad["error"] == "invalid_drc_request", bad
            assert wt.read_bytes() == expected_waives
            w.drain()
            # Cross-context/cross-kind tokens cannot acquire a different target.
            token = t.upload(("floe_pack=" + fp + "\n").encode())
            c.call("POST", w.root, w.request("prepare", token=token), 410)
            stale_context = dict(s.context, revision="0" * 64)
            c.call("POST", t.root, dict(t.request("prepare", token=token), context=stale_context), 409)
            c.call("POST", t.api + "/revoke", {"token": token}, 204)
            t.drain()
            # Cancelling asynchronous preparation never means review deletion.
            request = t.request("import", bytes=str(len(notes_input)))
            c.call("POST", t.root, request, 202)
            c.call("POST", t.root + "/" + request["seq"] + "/cancel", {}, 202)
            result = t.finished(request["seq"])
            assert result["phase"] in ("cancelled", "failed", "succeeded")
            upload = t.status()["upload"]
            if upload:
                c.call("POST", t.api + "/revoke", {"token": upload["token"]}, 204)
            t.drain()
            # Logout with a partially uploaded file releases its native lease,
            # file reservation and descriptor; no named staging file survives.
            _, partial = t.start("import", bytes=str(len(notes_input)))
            seq, _ = t.chunk(partial["upload"]["token"], 0, notes_input[:CHUNK])
            t.finished(seq)
            assert t.status()["usage"]["pending"] == 1
            close(s)
            assert not list(temps.rglob(".floe-transfer-*"))
            assert not list(temps.iterdir()), list(temps.iterdir())
            assert fingerprint(inputs) == before
        finally:
            for s in sessions:
                if s.proc.poll() is None:
                    s.proc.terminate()
                s.proc.communicate(timeout=15)
        print("WEB DRC TRANSFER: ALL OK (owner opt-in/auth, chunk/replay/whole replacement, >1MiB FE, 6007 statuses, Python bytes, preview/approve, conflicts, bounded artifacts/download, cancel/logout, input preservation)")


if __name__ == "__main__":
    main(sys.argv[1])
