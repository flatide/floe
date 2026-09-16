#!/usr/bin/env python3
"""Actual picker -> runtime DRC replacement; private valmini/synthetic DRC only."""
import copy
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from validate_drc_ice import DB
from validate_web_cli import INDEX, wait
from validate_web_drc_notes import API, Session, fingerprint
from validate_web_drc_build import done as build_done, request as build_request


class Picker:
    def __init__(self, session, root_name):
        self.s, self.c = session, session.client
        roots = self.c.call("GET", "/api/v1/browse")["roots"]
        self.root = next(r["handle"] for r in roots if r["name"].endswith(root_name))

    def submit(self, kind, **args):
        seq = str(int(self.c.call("GET", "/api/v1/browse")["last_seq"]) + 1)
        request = dict(kind=kind, seq=seq, **args)
        self.c.call("POST", "/api/v1/browse", request, 202)
        return request

    def finish(self, request):
        result = wait(lambda: (lambda r: r if r["phase"] in ("succeeded", "failed", "cancelled") else None)(
            self.c.call("GET", "/api/v1/browse/" + request["seq"])), self.s.proc)
        assert self.c.call("POST", "/api/v1/browse", request, 202) == result, "replayed open changed context"
        assert result["request"] == request
        return result

    def handle(self, name, filter="drc_files"):
        result = self.finish(self.submit("list", directory=self.root, filter=filter, query=name))
        assert result["phase"] == "succeeded", result
        rows = result["result"]["page"]["rows"]
        return next((r["handle"] for r in rows if r["name"] == name), None)

    def open(self, handle, context=None):
        return self.finish(self.submit("open_drc", handle=handle,
                                       context=context or self.s.context))

    def reconnect(self, context=None):
        return self.finish(self.submit("reconnect_drc_review", context=context or self.s.context, approve=True))

    def accept(self, result):
        assert result["phase"] == "succeeded", result
        current = self.c.call("GET", "/api/v1/drc")["drc"]
        assert result["result"]["drc"] == current
        assert result["result"]["view_id"] == self.s.context["view_id"]
        self.s.context = dict(view_id=self.s.context["view_id"], drc_id=current["id"], revision=current["revision"])
        return current


def idle(s):
    return wait(lambda: (lambda v: v if v["status"] == "idle" else None)(
        s.client.call("GET", "/api/v1/view")["view"]), s.proc)


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-live-drc-") as td:
        work = Path(td).resolve()
        temps = work / "temps"
        temps.mkdir()
        source = work / "layout.oas"
        shutil.copy2(fixture, source)
        subprocess.run([str(INDEX), "vfs", str(source), str(source) + ".floe", "--jobs", "2"],
                       check=True, capture_output=True, timeout=60)
        a, b, bad = [work / name for name in ("first.db", "second.db", "bad.db")]
        a.write_text(DB)
        b.write_text(DB.replace("TOP", "SECOND"))
        # A header-only arbitrary string is a valid empty legacy ASCII DRC.
        # Use a recognized, truncated pack to exercise a genuine open failure.
        bad.write_bytes(b"FLOEICE\0\0\0\0\0")
        subprocess.run([str(INDEX), "drc", str(a), "--jobs", "2"], check=True, capture_output=True, timeout=30)
        pack = Path(str(a) + ".ice")
        protected = [source, a, b, bad, pack] + list(Path(str(source) + ".floe").rglob("*"))
        protected = [p for p in protected if p.is_file()]
        before = fingerprint(protected)
        s = Session(source, None, temps, None, work / "session.json")
        try:
            c, picker = s.client, Picker(s, work.name)
            view = idle(s)
            assert c.call("GET", "/api/v1/drc")["drc"] is None
            assert c.call("GET", "/api/v1/drc")["build"] is None
            initial = copy.deepcopy(s.context)
            handle = picker.handle(a.name)
            assert handle and picker.handle(pack.name, "all_files") is None
            for extra in (dict(path=str(a)), dict(reviewer="unapproved")):
                request = dict(kind="open_drc", seq="999", handle=handle, context=initial, **extra)
                c.call("POST", "/api/v1/browse", request, 400)
            c.call("POST", "/api/v1/browse", dict(kind="open_drc", seq="999", handle=str(a), context=initial), 409)
            first = picker.accept(picker.open(handle))
            assert first["metadata"]["format"] == "ice" and first["metadata"]["review_cache"] == "cache"
            assert c.call("GET", "/api/v1/drc")["review_grant"] is None
            assert picker.reconnect()["phase"] == "failed", "no launcher grant became a reviewer"
            assert c.call("GET", "/api/v1/drc")["drc"] == first
            assert idle(s) == view, "opening DRC changed the layout camera/state"
            old = copy.deepcopy(s.context)
            rejected = picker.open(picker.handle(b.name), initial)
            assert rejected["phase"] == "failed" and c.call("GET", "/api/v1/drc")["drc"]["id"] == first["id"]
            rejected = picker.open(picker.handle(bad.name))
            assert rejected["phase"] == "failed" and c.call("GET", "/api/v1/drc")["drc"]["id"] == first["id"]
            second = picker.accept(picker.open(picker.handle(b.name)))
            assert second["metadata"]["format"] == "ascii" and second["id"] != first["id"]
            assert c.call("GET", "/api/v1/drc/review/notes", code=403)["error"] == "review_disabled"
            build = build_request(second, s.context["view_id"], 1)
            c.call("POST", "/api/v1/drc/builds", dict(build, approve=False), 400)
            assert not Path(str(b) + ".ice").exists(), "DRC selection indexed without consent"
            c.call("POST", "/api/v1/drc/builds", build, 202)
            outcome = build_done(c, s.proc, 1)
            assert outcome["phase"] == "succeeded", outcome
            built = c.call("GET", "/api/v1/drc")["drc"]
            assert built["metadata"]["format"] == "ice" and built["id"] != second["id"]
            assert c.call("POST", "/api/v1/drc/builds", build, 202) == outcome
            s.context.update(drc_id=built["id"], revision=built["revision"])
            c.call("POST", "/api/v1/drc/" + old["drc_id"] + "/read",
                   dict(view_id=old["view_id"], revision=old["revision"], body=dict(kind="rules", start="0", search="", limit=64)), 404)
            explicit = picker.accept(picker.open(picker.handle(pack.name)))
            assert explicit["metadata"]["review_cache"] == "explicit"
            for _ in range(6):
                previous = s.context["drc_id"]
                request = picker.submit("open_drc", handle=picker.handle(b.name), context=s.context)
                c.call("POST", "/api/v1/browse/" + request["seq"] + "/cancel", {})
                result = picker.finish(request)
                if result["phase"] == "succeeded":
                    picker.accept(result)
                else:
                    assert result["phase"] == "cancelled", result
                    assert c.call("GET", "/api/v1/drc")["drc"]["id"] == previous
            assert idle(s) == view
            assert fingerprint(protected) == before, "runtime DRC selection modified an input/cache"
        finally:
            s.close()

        # A prior fixed writer keeps its receipts, never migrates authority or
        # an unapproved edit to the newly selected DB.
        protected.append(Path(str(b) + ".ice"))
        before = fingerprint(protected)
        s = Session(source, pack, temps, "runtime-owner", work / "writer.json")
        try:
            c, picker = s.client, Picker(s, work.name)
            # The first two fixture checks intentionally have no violations.
            ref = [dict(check="2", error="0")]
            draft = s.prepare(s.read(ref), "synthetic saved before replacement")
            request = s.request(draft, 1)
            c.call("POST", API, request, 202)
            saved = s.finished(1)
            assert saved["published"] is True, saved
            binding = c.call("GET", API)["binding_id"]
            assert saved["scope_id"] == binding
            transfer = dict(seq="1", context=copy.deepcopy(s.context), action="export")
            c.call("POST", API + "/transfer", transfer, 202)
            exported = wait(lambda: (lambda r: r if r["phase"] == "succeeded" else None)(
                c.call("GET", API + "/transfer/1")), s.proc)
            stale = s.request(s.prepare(s.read(ref), "must never migrate"), 2)
            old_context = copy.deepcopy(s.context)
            view = idle(s)
            picker.accept(picker.open(picker.handle(b.name)))
            status = c.call("GET", API)
            assert status["detached"] and not status["available"] and not status["editable"]
            assert c.call("GET", API + "/1") == saved
            assert c.call("POST", API, request, 202) == saved
            c.call("POST", API, stale, 409)
            c.call("POST", API + "/read", dict(context=s.context, errors=ref), 403)
            assert s.context != old_context and idle(s) == view
            assert not list(work.glob(".second.db.notes.*"))
            assert not list(work.glob(".second.db.waive.*"))
            assert fingerprint(protected) == before
            for extra in (dict(reviewer="different"), dict(editable=True), dict(path=str(a)), dict(autosave=True)):
                c.call("POST", "/api/v1/browse", dict(kind="reconnect_drc_review", seq="999", context=s.context, approve=True, **extra), 400)
            c.call("POST", "/api/v1/browse", dict(kind="reconnect_drc_review", seq="999", context=s.context, approve=False), 409)
            grant = c.call("GET", "/api/v1/drc")["review_grant"]
            assert grant == dict(available=True, reviewer="runtime-owner", notes_editable=True, waives_editable=False)
            assert picker.reconnect(old_context)["phase"] == "failed"
            result = picker.reconnect()
            picker.accept(result)
            assert result["result"]["review_registration_required"] is False
            reconnected = c.call("GET", API)
            assert reconnected["editable"] and reconnected["available"] and not reconnected["detached"]
            assert reconnected["binding_id"] != binding and reconnected["autosave"] is False
            assert reconnected["operations"] == status["operations"], "reconnect reset or rewrote save history"
            assert c.call("GET", API + "/1") == saved
            assert c.call("POST", API, request, 202) == saved
            assert c.call("POST", API + "/transfer", transfer, 202) == exported
            transfers = c.call("GET", API + "/transfer")
            assert transfers["operations"]["last_seq"] == "1" and transfers["artifacts"] == []
            c.call("POST", API, stale, 409)
            assert not list(work.glob(".second.db.notes.*")), "binding wrote a sidecar"
            draft = s.prepare(s.read(ref), "synthetic saved after explicit reconnect")
            c.call("POST", API, s.request(draft, 2), 202)
            saved_second = s.finished(2)
            assert saved_second["published"] and saved_second["scope_id"] == reconnected["binding_id"]
            assert saved_second["context"]["drc_id"] == s.context["drc_id"]
            assert c.call("POST", API, request, 202) == saved
            assert c.call("GET", API + "/1") == saved
            c.call("GET", "/api/v1/drc/review/waives", code=403)
            assert not c.call("GET", "/api/v1/drc")["review_grant"]["available"]
            assert picker.reconnect()["phase"] == "failed", "binding can only reconnect a detached reviewer"
            assert idle(s) == view and fingerprint(protected) == before
        finally:
            s.close()

        # Read-only launcher registration stays read-only; derived saved notes
        # are readable, but reconnect grants no editor/transfer endpoints.
        s = Session(source, pack, temps, None, work / "reader.json", read_reviewer="runtime-owner")
        try:
            c, picker = s.client, Picker(s, work.name)
            picker.accept(picker.open(picker.handle(b.name)))
            picker.accept(picker.reconnect())
            status = c.call("GET", API)
            assert status["available"] and not status["editable"] and not status["autosave"]
            assert c.call("GET", "/api/v1/drc")["review_grant"]["reviewer"] == "runtime-owner"
            assert s.display(ref, ref[0])["focus"]["text"] == "synthetic saved after explicit reconnect"
            s.read(ref, 403)
            c.call("GET", API + "/transfer", code=403)
            c.call("GET", "/api/v1/drc/review/waives", code=403)
        finally:
            s.close()

        # Waive authority is a separate fixed launcher grant. Reconnect loads
        # only its adjacent target; it never creates it or adopts a temp legacy.
        s = Session(source, pack, temps, "waive-owner", work / "waiver.json", edit_waives=True)
        try:
            c, picker = s.client, Picker(s, work.name)
            picker.accept(picker.open(picker.handle(b.name)))
            before_reader = c.call("GET", "/api/v1/drc")["drc"]
            unsafe_sidecar = work / ".second.db.waive.waive-owner"
            unsafe_sidecar.symlink_to(b)
            try:
                assert picker.reconnect()["phase"] == "failed"
                assert c.call("GET", "/api/v1/drc")["drc"] == before_reader
                assert c.call("GET", API)["detached"]
            finally:
                unsafe_sidecar.unlink()
            picker.accept(picker.reconnect())
            waive_api = "/api/v1/drc/review/waives"
            assert not list(work.glob(".second.db.waive.*"))
            snap = c.call("POST", waive_api + "/read", dict(context=s.context, errors=ref))
            draft = c.call("POST", waive_api + "/prepare", dict(context=s.context, token=snap["token"], waived=True))
            approve = copy.deepcopy(s.request(draft, 1))
            c.call("POST", waive_api, approve, 202)
            saved = wait(lambda: (lambda r: r if r["phase"] in ("succeeded", "failed", "cancelled") else None)(
                c.call("GET", waive_api + "/1")), s.proc)
            assert saved["published"] and saved["reader_applied"], saved
            catalog = c.call("GET", "/api/v1/drc")["drc"]
            s.context["revision"] = catalog["revision"]
            picker.accept(picker.open(picker.handle(a.name)))
            picker.accept(picker.reconnect())
            assert c.call("POST", waive_api, approve, 202) == saved
            assert c.call("GET", waive_api)["operations"]["last_seq"] == "1"
            assert not list(work.glob(".first.db.waive.*"))
            assert fingerprint(protected) == before
        finally:
            s.close()
        assert not list(temps.iterdir()), "native resources were not reaped"
    print("WEB DRC OPEN: ALL OK (initial/cache/ASCII/explicit ICE, scoped handles, approved build, stale/failure/cancel/replay, unchanged layout, launcher-only reviewer reconnect/read-only+notes+waives, receipt epochs and both ledgers preserved, no implicit sidecar writes, shutdown)")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
