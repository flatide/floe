#!/usr/bin/env python3
"""Actual Rust CLI/HTTP/native lifecycle; Python is a test driver, never a runtime."""
import http.cookiejar
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
APP = Path(os.environ.get("FLOE_WEB_TEST_APP", ROOT / "rust/target/release/floe2-web"))
INDEX = ROOT / "rust/target/release/floe-index"
RENDERD = ROOT / "rust/target/release/floe-renderd"


def wait(test, proc, seconds=15):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        found = test()
        if found:
            return found
        assert proc.poll() is None, proc.communicate()
        time.sleep(.02)
    raise AssertionError("timed out waiting for Rust CLI")


def read_json(path):
    try:
        return json.loads(path.read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        return None


class Client:
    def __init__(self, session):
        self.origin = session["origin"]
        self.csrf = None
        self.opener = urllib.request.build_opener(
            urllib.request.ProxyHandler({}),
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
        self.token = session["url"].split("#bootstrap=")[1]
        self.bundle = session["bundle"]

    def call(self, method, path, body=None, code=200):
        headers = {"Origin": self.origin}
        if self.csrf:
            headers["X-Floe-CSRF"] = self.csrf
        data = None if body is None else json.dumps(body).encode()
        if data is not None:
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(self.origin + path, data=data,
                                         headers=headers, method=method)
        try:
            response = self.opener.open(request, timeout=8)
        except urllib.error.HTTPError as e:
            response = e
        with response:
            assert response.status == code, (response.status, response.read())
            data = response.read(1024 * 1024)
            return json.loads(data) if "application/json" in response.headers.get("content-type", "") else data

    def login(self):
        self.call("POST", "/api/v1/session/exchange",
                  dict(bootstrap=self.token, protocol=1, bundle="stale"), 426)
        auth = self.call("POST", "/api/v1/session/exchange",
                         dict(bootstrap=self.token, protocol=1, bundle=self.bundle))
        self.csrf = auth["csrf"]

    def finished(self, seq, proc):
        return wait(lambda: (lambda v: v if v["phase"] in
                    ("failed", "succeeded", "cancelled", "incomplete") else None)(
                    self.call("GET", "/api/v1/operations/" + str(seq))), proc, 30)


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-web-cli-") as td:
        work = Path(td)
        source = work / "설계 with spaces.oas"
        shutil.copy2(fixture, source)
        original = source.read_bytes()
        temps = work / "temps"
        temps.mkdir()
        session_path = work / "session.json"
        argv_path = work / "browser.argv"
        fake = work / "controlled-firefox"
        fake.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$FLOE_BROWSER_TEST_RECORD"\n'
                        'printf "%s" "$$" > "$FLOE_BROWSER_TEST_RECORD.pid"\nkill -STOP "$$"\n')
        fake.chmod(0o700)
        env = dict(os.environ, PATH="", TMPDIR=str(temps), FLOE_INDEX_BIN=str(INDEX),
                   FLOE_RENDERD_BIN=str(RENDERD), FLOE_BROWSER_TEST_RECORD=str(argv_path))
        for manual in (False, True):
            args = [str(APP), "view", str(source), "--session-file", str(session_path),
                    "--goto", "-10.9375,20,700", "--depth", "99", "--detail", "high",
                    "--thin", "keep", "--no-labels", "--jobs", "2", "--raster-jobs", "1"]
            args += ["--no-open"] if manual else ["--firefox", str(fake)]
            proc = subprocess.Popen(args, env=env, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, text=True)
            try:
                session = wait(lambda: read_json(session_path), proc)
                assert session_path.stat().st_mode & 0o777 == 0o600
                client = Client(session)
                page = client.call("GET", "/")
                assert b"Layout workspace" in page
                assert session["bundle"].encode() in page
                assert session["url"].encode() not in page
                client.call("GET", "/api/v1/startup", code=401)
                if not manual:
                    argv = wait(lambda: argv_path.read_text().splitlines() if argv_path.exists() else None, proc)
                    assert argv[:3] == ["--no-remote", "--new-instance", "--profile"]
                    profile = Path(argv[3])
                    assert profile.stat().st_mode & 0o777 == 0o700
                    assert argv[4] == "--new-window" and argv[5].startswith("file://")
                    assert client.token not in "\n".join(argv), "bootstrap leaked through argv"
                    launch = profile / "launch.html"
                    assert launch.stat().st_mode & 0o777 == 0o600
                    assert session["url"] in launch.read_text()
                client.login()
                startup = client.call("GET", "/api/v1/startup")["request"]
                assert startup["body"]["navigation"]["center_um"][0] == "-10.9375"
                assert startup["body"]["detail"] == "high"
                assert startup["body"]["depth"] == "99"
                startup["body"]["pixels"] = [1001, 733]
                client.call("POST", "/api/v1/operations", startup, 202)
                first = client.finished(1, proc)
                if not manual:
                    assert first["error"] == "index_unavailable"
                    assert not Path(str(source) + ".floe").exists(), "open silently indexed"
                    client.call("POST", "/api/v1/operations", dict(kind="index", seq="2",
                                source_id=startup["source_id"], options=dict(jobs=2)), 202)
                    assert client.finished(2, proc)["phase"] == "succeeded"
                    startup["seq"] = "3"
                    client.call("POST", "/api/v1/operations", startup, 202)
                    assert client.finished(3, proc)["phase"] == "succeeded"
                else:
                    assert first["phase"] == "succeeded"
                view = wait(lambda: (lambda v: v if v["status"] == "idle" else None)(
                    client.call("GET", "/api/v1/view")["view"]), proc)
                assert view["submitted"] == "1", "hidden initial fit render"
                assert view["pixels"] == [1001, 733]
                assert view["detail"] == "high" and view["depth"] == "99"
                assert view["effective_thin"] == "keep"
                bbox = list(map(float, view["bbox_dbu"]))
                unit = float(view["dbu_um"])
                assert abs((bbox[2] - bbox[0]) * unit - 700) < 1e-9
                assert abs((bbox[0] + bbox[2]) / 2 * unit + 10.9375) < 1e-9
                if manual:
                    proc.send_signal(signal.SIGINT)
                else:
                    client.call("DELETE", "/api/v1/session", code=204)
                out, err = proc.communicate(timeout=15)
                assert proc.returncode == (130 if manual else 0), (out, err)
                assert client.token not in out + err
                assert not session_path.exists(), "session credential file survived shutdown"
                assert not list(temps.iterdir()), "native/browser private files leaked"
                if not manual:
                    pid = int(Path(str(argv_path) + ".pid").read_text())
                    try:
                        os.kill(pid, 0)
                    except ProcessLookupError:
                        pass
                    else:
                        raise AssertionError("browser child survived shutdown")
            finally:
                if proc.poll() is None:
                    proc.terminate()
                    try:
                        proc.communicate(timeout=15)
                    except subprocess.TimeoutExpired:
                        proc.kill()
                        proc.communicate(timeout=5)
        # Existing file and symlink targets are never overwritten by session output.
        for target in (source, work / "session-link"):
            if target != source:
                target.symlink_to(source)
            run = subprocess.run([str(APP), "view", str(source), "--no-open",
                                  "--session-file", str(target)], env=env,
                                 capture_output=True, timeout=15)
            assert run.returncode != 0
            assert source.read_bytes() == original
        assert not list(temps.iterdir())
    print("WEB CLI: ALL OK (PATH empty, first generation, explicit index, private Firefox argv, logout/SIGINT, cleanup)")


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
