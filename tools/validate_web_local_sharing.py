#!/usr/bin/env python3
"""Opt-in Rust launcher + public guest shell + scoped auth, never a browser gate."""
import http.cookiejar
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request

from validate_web_cli import APP, INDEX, RENDERD, Client, read_json, wait
from validate_view_controller import digest


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-local-share-") as td:
        work = Path(td)
        source = work / "synthetic.oas"
        shutil.copy2(fixture, source)
        temps = work / "temps"
        temps.mkdir()
        env = dict(os.environ, PATH="", TMPDIR=str(temps), FLOE_INDEX_BIN=str(INDEX),
                   FLOE_RENDERD_BIN=str(RENDERD))
        result = subprocess.run([str(APP), "index", str(source), "--jobs", "2"],
                                env=env, capture_output=True, timeout=40)
        assert result.returncode == 0, result.stderr
        before = digest(Path(str(source) + ".floe"))
        source_before = source.read_bytes(), source.stat().st_mtime_ns
        review = work / "synthetic.db"
        review.write_text("TOP 1000\nWIDTH\n1 1 0\np 1 4\n10.125 10\n20.125 10\n20.125 20\n10.125 20\n")
        review_before = review.read_bytes(), review.stat().st_mtime_ns
        for enabled in (False, True):
            session_file = work / ("on.json" if enabled else "off.json")
            argv = [str(APP), "view", str(source), "--multi", "--no-open", "--session-file",
                    str(session_file), "--jobs", "1", "--raster-jobs", "1", "--frame-cache", "off"]
            if enabled:
                argv.extend(["--local-sharing", "--drc", str(review)])
            proc = subprocess.Popen(argv, env=env, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, text=True)
            try:
                session = wait(lambda: read_json(session_file), proc)
                owner = Client(session)
                owner.call("GET", "/guest/" + "a" * 64, code=200 if enabled else 404)
                owner.call("GET", "/guest/not-an-id", code=404)
                owner.login()
                caps = owner.call("GET", "/api/v1/capabilities")
                assert caps["share_grants"] is enabled
                owner.call("GET", "/api/v1/shares", code=200 if enabled else 404)
                if enabled:
                    page = owner.call("GET", "/guest/" + "a" * 64)
                    assert b"guest.js" in page and b"app.js" not in page
                    assert session["url"].encode() not in page and b"bootstrap=" not in page
                    for name in ("guest.js", "guest.css", "sharing.js", "guest-drc.js", "guest-drc-step.js", "guest-layers.js", "drc-geometry.js",
                                 "guest-display.js", "guest-query-wire.js", "guest-tools.js"):
                        actual = owner.call("GET", "/assets/" + session["bundle"] + "/" + name)
                        expected = Path(__file__).resolve().parents[1] / "rust/web/ui" / name
                        assert actual == expected.read_bytes(), "stale embedded guest asset: " + name
                    startup = owner.call("GET", "/api/v1/startup")["request"]
                    startup["seq"] = "1"
                    startup["body"]["pixels"] = [128, 96]
                    owner.call("POST", "/api/v1/operations", startup, 202)
                    assert owner.finished(1, proc)["phase"] == "succeeded"
                    view = wait(lambda: (lambda v: v if v["status"] == "idle" else None)(
                        owner.call("GET", "/api/v1/view")["view"]), proc)
                    drc = wait(lambda: (lambda d: d if d and d["phase"] == "ready" else None)(
                        owner.call("GET", "/api/v1/drc")["drc"]), proc)
                    for mode, share_drc in (("follow", False), ("explore", False), ("follow", True)):
                        body = dict(view_id=view["view_id"], base_state_rev=view["state_rev"],
                                    mode=mode, approve=False)
                        owner.call("POST", "/api/v1/shares", body, 403)
                        body["approve"] = True
                        if share_drc:
                            body["drc"] = dict(id=drc["id"], revision=drc["revision"], approve=True)
                        invite = owner.call("POST", "/api/v1/shares", body)
                        if share_drc:
                            assert invite["drc"] == dict(id=drc["id"], revision=drc["revision"])
                        else:
                            assert "drc" not in invite
                        base = "/api/v1/guest/" + invite["share_id"]
                        guest = urllib.request.build_opener(urllib.request.ProxyHandler({}),
                            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
                        csrf = None

                        def call(method, path, payload=None, code=200):
                            headers = {"Origin": session["origin"]}
                            if csrf:
                                headers["X-Floe-Guest-CSRF"] = csrf
                            if payload is not None:
                                headers["Content-Type"] = "application/json"
                            request = urllib.request.Request(session["origin"] + path,
                                data=None if payload is None else json.dumps(payload).encode(),
                                headers=headers, method=method)
                            try:
                                response = guest.open(request, timeout=8)
                            except urllib.error.HTTPError as error:
                                response = error
                            with response:
                                assert response.status == code, response.status
                                data = response.read(1024 * 1024)
                                return json.loads(data) if data else None

                        token = dict(invite=invite["invite"], protocol=1, bundle=session["bundle"])
                        csrf = call("POST", base + "/exchange", token)["csrf"]
                        call("POST", base + "/exchange", token, 401)
                        assert call("GET", base + "/session")["mode"] == mode
                        call("GET", "/api/v1/capabilities", code=401)
                        palette_request = dict(view_id=view["view_id"], state_rev=view["state_rev"], body={})
                        if mode == "follow":
                            palette = call("POST", base + "/layers", palette_request)
                            assert palette["view_id"] == view["view_id"]
                            assert palette["data"]["total"] > 0
                            assert all("name" in r and "parent" in r for r in palette["data"]["rows"])
                        else:
                            call("POST", base + "/layers", palette_request, 409)  # no implicit worker
                        if share_drc:
                            meta = call("GET", base + "/drc")
                            assert meta["data"]["format"] == "ascii" and meta["data"]["errors"] == "1"
                            assert meta["data"]["read_only"] is True
                            assert not {"notes", "reviewer", "svrf", "title", "source_id"} & meta["data"].keys()
                            envelope = dict(view_id=meta["view_id"], revision=drc["revision"])
                            rows = call("POST", base + "/drc/read", dict(envelope,
                                body=dict(kind="list", check="0", start="0", limit=64, in_view=False)))
                            assert len(rows["data"]["rows"]) == 1
                            geometry = call("POST", base + "/drc/read", dict(envelope,
                                body=dict(kind="geometry", check="0", error="0", start="0", limit=2048)))
                            assert "points_um" in geometry["data"] and "points_dbu" not in geometry["data"]
                            call("POST", base + "/drc/read", dict(envelope,
                                body=dict(kind="types", start="0", limit=1)), 403)
                        else:
                            call("GET", base + "/drc", code=403)
                        owner.call("DELETE", "/api/v1/shares/" + invite["share_id"], code=204)
                        call("GET", base + "/session", code=401)
                    assert owner.call("GET", "/api/v1/view")["view"]["state_rev"] == view["state_rev"]
                owner.call("DELETE", "/api/v1/session", code=204)
                out, err = proc.communicate(timeout=15)
                assert proc.returncode == 0, (out, err)
                assert session["url"] not in out + err and owner.token not in out + err
            finally:
                if proc.poll() is None:
                    proc.terminate()
                    proc.communicate(timeout=15)
        assert digest(Path(str(source) + ".floe")) == before
        assert (source.read_bytes(), source.stat().st_mtime_ns) == source_before
        assert (review.read_bytes(), review.stat().st_mtime_ns) == review_before
        assert not Path(str(review) + ".ice").exists(), "read-only sharing must not build a DRC pack"
        assert not list(temps.iterdir()), "local-share launcher leaked private worker files"
    print("WEB LOCAL SHARING CLI: ALL OK (default off, byte-exact guest assets, native open, follow/explore, separate DRC grant + fractional ASCII read, isolated auth, revoke, no implicit index/writes/worker leaks)")


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
