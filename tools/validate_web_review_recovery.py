#!/usr/bin/env python3
"""Explicit recovery HTTP on private synthetic inodes; no production fault hook.

Normal Rust publication supplies the real marker. A hard link reconstructs the
post-link/pre-unlink gap; actual SIGKILL coverage belongs to the core tests.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from cache_test_paths import drc_pack, vfs_cache
from validate_web_cli import Client, INDEX, read_json, wait
from validate_web_drc_notes import Session, fingerprint
from validate_web_drc_open import Picker, idle
from validate_drc_review import attributes

ROOT_API = "/api/v1/drc/review/"
REFS = [dict(check="0", error="0")]
ROUTES = [("GET", ""), ("POST", ""), ("POST", "/prepare"), ("POST", "/reconcile")]


class RecoverySession(Session):
    def connect(self):
        self.client = Client(wait(lambda: read_json(self.session_path), self.proc))
        self.client.login()
        self.catalog = wait(lambda: (lambda c: c if c["phase"] in ("ready", "error") else None)(
            self.client.call("GET", "/api/v1/drc")["drc"]), self.proc)
        startup = self.client.call("GET", "/api/v1/startup")["request"]
        startup["body"]["pixels"] = [257, 191]
        self.client.call("POST", "/api/v1/operations", startup, 202)
        opened = self.client.finished(1, self.proc)
        assert opened["phase"] == "succeeded", opened
        self.context = dict(drc_id=self.catalog["id"], revision=self.catalog["revision"],
                            view_id=opened["view_id"])


def finished(s, api, seq):
    def read():
        rows = s.client.call("GET", api)["operations"]["history"]
        return next((r for r in rows if r["seq"] == str(seq) and
                     r["phase"] in ("succeeded", "failed", "uncertain")), None)
    return wait(read, s.proc)


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-recovery-http-") as td:
        root = Path(td)
        temps = root / "runtime"
        temps.mkdir()
        source = root / "synthetic.oas"
        shutil.copy2(fixture, source)
        db = root / "synthetic.db"
        db.write_text("TOP 1000\nWIDTH\n1 1 0\np 1 4\n0 0\n20 0\n20 20\n0 20\n")
        for args in [["vfs", source, vfs_cache(source), "--jobs", "2"], ["drc", db, "--jobs", "2"]]:
            subprocess.run([str(INDEX), *map(str, args)], check=True, capture_output=True, timeout=30)
        pack = drc_pack(db)
        bad_rules = root / "invalid-synthetic.rules.json"
        bad_rules.write_text("{ deliberately invalid synthetic metadata")
        inputs = [source, db, pack, bad_rules, *sorted(vfs_cache(source).glob("design.*"))]
        before = fingerprint(inputs)
        marker = "com.floe.review-stage-v1" if sys.platform == "darwin" else "user.floe.review-stage-v1"
        decoy = root / ".floe-review-111-222.tmp"
        decoy.write_bytes(b"unrelated staging name; never scan or delete")

        for k in ("notes", "waives"):
            editor = ROOT_API + k
            api = editor + "/recovery"
            reviewer = "fixed-" + k
            target = root / (".synthetic.db.notes." + reviewer + ".fe" if k == "notes"
                             else ".synthetic.db.waive." + reviewer)
            # Real marked, bound publication, then stop before reconstructing gap.
            s = Session(source, pack, temps, reviewer, root / (k + "-publish.session"), edit_waives=True)
            try:
                c = s.client
                snap = c.call("POST", editor + "/read", dict(context=s.context, errors=REFS))
                draft = c.call("POST", editor + "/prepare", dict(context=s.context, token=snap["token"],
                               **(dict(text="synthetic preserved note") if k == "notes" else dict(waived=True))))
                c.call("POST", editor, s.request(draft, 1), 202)
                result = wait(lambda: (lambda r: r if r["phase"] == "succeeded" else None)(
                    c.call("GET", editor + "/1")), s.proc)
                assert result["published"] is True, result
            finally:
                s.close()
            content = target.read_bytes()
            attrs = attributes(target)
            if hasattr(os, "getxattr"):
                marked = os.getxattr(target, marker)
            else:
                assert sys.platform == "darwin"
                marked = bytes.fromhex(subprocess.run(["/usr/bin/xattr", "-px", marker, str(target)],
                                       check=True, capture_output=True, text=True, timeout=10).stdout)
            mark = json.loads(marked)
            stage = root / mark["stage"]
            assert stage.parent == root and not stage.exists() and mark["target"] == list(os.fsencode(target.name))
            os.link(target, stage)
            assert target.stat().st_nlink == 2
            inode = (target.stat().st_dev, target.stat().st_ino, target.stat().st_mtime_ns)
            # Writer/read-only grants cannot be inferred from a readable pack.
            for tag in (None, "reader"):
                s = RecoverySession(source, pack, temps, None, root / (k + "-" + str(tag) + ".session"),
                                    read_reviewer=reviewer if tag else None)
                try:
                    for method, suffix in ROUTES:
                        s.client.call(method, api + suffix, {} if method == "POST" else None, 403)
                    assert target.stat().st_nlink == 2 and stage.exists()
                finally:
                    s.close()

            # The CLI's default writer startup uses the explicit-waive reader,
            # which can load two links. Force an actual asynchronous metadata
            # failure separately; do not mislabel it as guarded-link rejection.
            s = RecoverySession(source, pack, temps, reviewer, root / (k + "-recover.session"),
                                edit_waives=True, rules=bad_rules if k == "waives" else None)
            try:
                c = s.client
                if k == "waives":
                    assert s.catalog["phase"] == "error", s.catalog
                anon = Client(read_json(s.session_path))
                for method, suffix in ROUTES:
                    anon.call(method, api + suffix, {} if method == "POST" else None, 401)
                for _ in range(3):
                    assert c.call("GET", api)["operations"]["last_seq"] == "0"
                assert stage.exists() and target.stat().st_nlink == 2
                for extra in (dict(path=str(target)), dict(reviewer="foreign"), dict(approve=True)):
                    c.call("POST", api + "/prepare", dict(context=s.context, **extra), 400)
                wrong = dict(s.context, revision="f" * 64)
                c.call("POST", api + "/prepare", dict(context=wrong), 409)
                p = c.call("POST", api + "/prepare", dict(context=s.context))
                assert p["name"] == target.name and p["bytes"] == str(len(content))
                assert p["scope"] == "exact_staging_link_only" and p["expires_in_ms"] == "30000"
                assert "stage" not in p and "path" not in p and stage.exists()
                req = dict(seq="1", context=s.context, token=p["token"], approve_recovery=True)
                c.call("POST", api, dict(req, approve_recovery=False), 400)
                c.call("POST", api, dict(req, path=str(target)), 400)
                c.call("POST", api, dict(req, token="f" * 64), 410)
                # Ordinary save/autosave approval is a different capability.
                c.call("POST", editor, s.request(p, 1), 410)
                assert stage.exists() and target.read_bytes() == content
                c.call("POST", api, req, 202)
                receipt = finished(s, api, 1)
                assert receipt["phase"] == "succeeded" and receipt["recovered"] is True, receipt
                assert receipt["reopen_required"] is True and receipt["directory_synced"] is True
                assert not stage.exists() and target.stat().st_nlink == 1
                assert target.read_bytes() == content
                assert inode == (target.stat().st_dev, target.stat().st_ino, target.stat().st_mtime_ns)
                assert attrs == attributes(target)
                once = fingerprint([target])
                assert c.call("POST", api, req, 202) == receipt  # old context, same receipt
                c.call("POST", api, dict(req, token="f" * 64), 409)
                assert c.call("POST", api + "/reconcile", dict(seq="1"), 202) == receipt
                assert fingerprint([target]) == once, "replay or read-only check mutated recovered file"
                # Even a new revision cannot query cached, unreloaded statuses.
                cat = c.call("GET", "/api/v1/drc")["drc"]
                assert cat["phase"] == "error" and cat["error"] == "drc_reopen_required", cat
                assert cat["revision"] == receipt["reader_revision"] != s.context["revision"]
                s.context = dict(s.context, revision=cat["revision"])
                c.call("POST", "/api/v1/drc/" + cat["id"] + "/read",
                       dict(view_id=s.context["view_id"], revision=cat["revision"], body=dict(kind="rule", check="0")), 409)
                # Explicit picker reopen, then explicit launcher-grant reconnect.
                idle(s)
                picker = Picker(s, root.name)
                # Picker lists the original DB, not its hidden cache sidecar.
                handle = picker.handle(db.name)
                assert handle is not None
                picker.accept(picker.open(handle))
                picker.accept(picker.reconnect())
                assert c.call("GET", "/api/v1/drc")["drc"]["phase"] == "ready"
                reread = c.call("POST", editor + "/read", dict(context=s.context, errors=REFS))
                assert reread["exists"] is True
                if k == "waives":
                    assert reread["waived_count"] == "1", reread
                c.call("POST", editor + "/revoke", dict(token=reread["token"]), 204)
                assert c.call("POST", api, req, 202) == receipt, "reconnect discarded recovery ledger"
                # Metadata changed after preview: reject BEFORE unlink, leave
                # both links and payload intact. This is not an uncertain write.
                os.link(target, stage)
                preview = c.call("POST", api + "/prepare", dict(context=s.context))
                old_mode = target.stat().st_mode & 0o777
                os.chmod(target, 0o640)
                changed = dict(seq="2", context=s.context, token=preview["token"], approve_recovery=True)
                c.call("POST", api, changed, 202)
                rejected = finished(s, api, 2)
                assert rejected["phase"] == "failed" and rejected["recovered"] is False, rejected
                assert rejected["reopen_required"] is True
                assert stage.exists() and target.stat().st_nlink == 2 and target.read_bytes() == content
                os.chmod(target, old_mode)
            finally:
                s.close()
            assert decoy.read_bytes() == b"unrelated staging name; never scan or delete"
            assert fingerprint(inputs) == before
        print("WEB REVIEW RECOVERY: ALL OK (real marked publication + modeled gap, notes/waives, failed metadata, permissions, explicit approval, replay, retired reader, explicit reopen, bytes/inode/xattrs/source preservation)")


if __name__ == "__main__":
    main(sys.argv[1])
