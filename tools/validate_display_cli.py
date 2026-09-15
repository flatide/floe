#!/usr/bin/env python3
"""Native diagnostic CLI lifecycle. Fake Firefox only; no browser acceptance claim."""
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile

from validate_web_cli import APP, Client, read_json, wait


def main():
    with tempfile.TemporaryDirectory(prefix="floe-display-cli-") as td:
        work = Path(td)
        temps = work / "temps"
        temps.mkdir()
        env = dict(os.environ, PATH="", TMPDIR=str(temps),
                   FLOE_INDEX_BIN=str(work / "missing-index"),
                   FLOE_RENDERD_BIN=str(work / "missing-renderd"),
                   FLOE_FIREFOX_BIN=str(work / "missing-firefox"))
        fake = work / "fake firefox"
        args_file = work / "argv.json"
        fake_exit = work / "fake-exit"
        # Executed only as the explicitly supplied fake Firefox binary.
        fake.write_text(f"#!{sys.executable}\nimport json, pathlib, sys, time\n"
                        f"pathlib.Path({str(args_file)!r}).write_text(json.dumps(sys.argv[1:]))\n"
                        f"while not pathlib.Path({str(fake_exit)!r}).exists(): time.sleep(.1)\n")
        fake.chmod(0o700)
        for end in ("logout", "signal", "browser-exit"):
            session_path = work / "session 한글.json"
            argv = [str(APP), "displaytest", "--session-file", str(session_path)]
            argv += ["--firefox", str(fake)] if end == "browser-exit" else ["--no-open"]
            with subprocess.Popen(argv, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as proc:
                try:
                    session = wait(lambda: read_json(session_path), proc)
                    assert session_path.stat().st_mode & 0o777 == 0o600
                    assert session["mode"] == "display-test"
                    client = Client(session)
                    page = client.call("GET", "/")
                    assert b"Display diagnostics" in page and b"Layout workspace" not in page
                    assert session["bundle"].encode() in page
                    assert session["url"].encode() not in page
                    client.call("GET", "/api/v1/display-test/raw", code=401)
                    client.login()
                    caps = client.call("GET", "/api/v1/capabilities")
                    assert caps["display_only"] is True
                    for name in ("render", "catalog", "index_open", "file_picker", "launcher", "drc", "exports"):
                        assert caps[name] is False, name
                    raw = client.call("GET", "/api/v1/display-test/raw")
                    assert len(raw) == 230416 and raw[:8] == b"FLOERAW1"
                    png = client.call("GET", "/api/v1/display-test/png")
                    assert png[:8] == b"\x89PNG\r\n\x1a\n" and len(png) < 16384
                    client.call("GET", "/api/v1/view", code=404)
                    if end == "logout":
                        client.call("DELETE", "/api/v1/session", code=204)
                    elif end == "signal":
                        proc.send_signal(signal.SIGINT)
                    else:
                        args = wait(lambda: read_json(args_file), proc)
                        assert args[:3] == ["--no-remote", "--new-instance", "--profile"]
                        profile = Path(args[3])
                        assert profile.stat().st_mode & 0o777 == 0o700
                        assert args[-1].startswith("file://") and "#bootstrap=" not in " ".join(args)
                        # The fake exits cooperatively; no process enumeration/signaling.
                        fake_exit.touch()
                    stdout, stderr = proc.communicate(timeout=10)
                    assert proc.returncode == (130 if end == "signal" else 0), stderr
                    assert not stdout and client.token.encode() not in stderr
                    assert not session_path.exists() and not list(temps.iterdir())
                finally:
                    if proc.poll() is None:
                        proc.send_signal(signal.SIGINT)
                        proc.communicate(timeout=10)
        existing = work / "preserve.json"
        existing.write_text("do not replace")
        link = work / "preserve-link"
        link.symlink_to(existing)
        for target in (existing, link):
            run = subprocess.run([str(APP), "displaytest", "--no-open", "--session-file", str(target)],
                                 env=env, capture_output=True, timeout=10)
            assert run.returncode != 0 and existing.read_text() == "do not replace"
            assert not list(temps.iterdir())
        for args in (["--help"], ["input.png"], ["--port", "65536"]):
            run = subprocess.run([str(APP), "displaytest", *args], env=env, capture_output=True, timeout=10)
            assert (run.returncode == 0) == (args == ["--help"])
            assert not list(temps.iterdir())
        # Invalid explicit/environment browser discovery must not fall through or create a session.
        run = subprocess.run([str(APP), "displaytest"], env=env, capture_output=True, timeout=10)
        assert run.returncode != 0 and not list(temps.iterdir())
    print("DISPLAY CLI: ALL OK (empty PATH, missing native workers, auth, logout/SIGINT, private fake Firefox, no overwrite/cleanup)")


if __name__ == "__main__":
    main()
