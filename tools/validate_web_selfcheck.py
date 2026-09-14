#!/usr/bin/env python3
"""Development-only native runtime diagnostic gate; never reads a design."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BINS = ROOT / "rust/target/release"
PYTHON = Path(sys.executable).resolve()


def script(path, body):
    path.write_text("#!" + str(PYTHON) + "\n" + body)
    path.chmod(0o700)
    return path


def invoke(app, env, *args, code=0, timeout=12):
    p = subprocess.run([str(app), *args], env=env, capture_output=True,
                       text=True, timeout=timeout)
    assert p.returncode == code, (args, p.returncode, p.stdout, p.stderr)
    return p


def wait_file(path, p):
    until = time.monotonic() + 5
    while not path.exists():
        assert p.poll() is None, p.communicate()
        assert time.monotonic() < until, str(path)
        time.sleep(.01)


def build_identity(work):
    helper = work / "build-info-helper"
    subprocess.run(["rustc", "--edition=2021", str(ROOT / "rust/app/build.rs"),
                    "-o", str(helper)], check=True, capture_output=True)
    clean = {k: v for k, v in os.environ.items() if not k.startswith("GIT_") and k != "FLOE_SRC_REV"}
    clean.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, TARGET="fixture-target")
    repo = work / "identity repository"
    app = repo / "rust/app"
    (app / "src").mkdir(parents=True)

    def git(*args):
        return subprocess.check_output(["git", "-C", str(repo), *args], env=clean,
                                       text=True, stderr=subprocess.PIPE).strip()

    def identity(cwd, env=clean, code=0):
        p = subprocess.run([str(helper)], cwd=cwd, env=env, capture_output=True, text=True, timeout=10)
        assert p.returncode == code, (p.stdout, p.stderr)
        if code:
            return p
        fields = dict(line[len("cargo:rustc-env="):].split("=", 1) for line in p.stdout.splitlines()
                      if line.startswith("cargo:rustc-env="))
        assert fields["FLOE_APP_TARGET"] == "fixture-target"
        return fields["FLOE_APP_REVISION"], p.stdout

    git("init", "-q")
    git("-c", "user.name=synthetic", "-c", "user.email=synthetic@example.invalid",
        "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "commit", "--allow-empty", "-qm", "fixture")
    head = git("rev-parse", "HEAD")
    assert identity(app)[0] == head
    (repo / "change.txt").write_text("private build identity fixture\n")
    assert identity(app)[0] == head + "+"
    archive = repo / "source ZIP/rust/app"
    (archive / "src").mkdir(parents=True)
    assert identity(archive)[0] == "unknown", "ZIP inherited the unrelated enclosing repository"
    linked = work / "linked worktree"
    git("worktree", "add", "-qb", "selfcheck-fixture", str(linked), "HEAD")
    linked_app = linked / "rust/app"
    (linked_app / "src").mkdir(parents=True)
    revision, watched = identity(linked_app)
    assert revision == head and "worktrees/" in watched and "refs/heads/selfcheck-fixture" in watched
    explicit = dict(clean, PATH="", FLOE_SRC_REV="offline-test-r1")
    assert identity(archive, explicit)[0] == "offline-test-r1"
    for value in ("", "bad revision", "bad\nrevision", "a" * 129):
        p = subprocess.run([str(helper)], cwd=archive, env=dict(explicit, FLOE_SRC_REV=value),
                           capture_output=True, text=True, timeout=10)
        assert p.returncode != 0 and "FLOE_SRC_REV must be" in p.stderr


def main():
    with tempfile.TemporaryDirectory(prefix="floe-web-selfcheck-") as td:
        work = Path(td)
        build_identity(work)
        runtime = work / "runtime"
        runtime.mkdir()
        app = BINS / "floe2-web"
        env = dict(os.environ, PATH="", TMPDIR=str(runtime),
                   FLOE_INDEX_BIN=str(BINS / "floe-index"),
                   FLOE_RENDERD_BIN=str(BINS / "floe-renderd"),
                   FLOE_FIREFOX_BIN=str(work / "missing-firefox"))
        version = invoke(app, env, "--version").stdout
        assert "revision " in version and "target " in version and "web " in version
        m = json.loads(invoke(app, env, "selfcheck", "--metadata-only").stdout)
        assert m["checks"] == [] and m["runtime_checked"] is False and "ok" not in m
        assert m["source_revision"] and m["web_bundle"] in version
        assert m["desktop_acceptance"] == "unverified" and not m["python_runtime"]
        assert not list(runtime.iterdir())
        report = json.loads(invoke(app, env, "selfcheck").stdout)
        assert report["ok"] and report["runtime_checked"]
        assert len(report["checks"]) == 2 and all(c["ok"] for c in report["checks"])
        assert not report["firefox_discovery"]["available"]
        assert not list(runtime.iterdir()), "worker scratch was not removed"
        bad_temp = work / "temporary runtime"
        bad_temp.mkdir()
        r = json.loads(invoke(app, dict(env, TMPDIR=str(bad_temp)), "selfcheck", code=1).stdout)
        assert not r["checks"][1]["ok"] and "wire path" in r["checks"][1]["error"]
        assert not list(bad_temp.iterdir()), "invalid wire temp root leaked scratch"

        # Detecting Firefox is informational and must never run its executable.
        marker = work / "firefox-ran"
        fake = script(work / "firefox", "from pathlib import Path\nPath(%r).touch()\n" % str(marker))
        browser = json.loads(invoke(app, dict(env, FLOE_FIREFOX_BIN=str(fake)), "selfcheck").stdout)
        assert browser["firefox_discovery"]["available"] and not browser["firefox_discovery"]["executed"]
        assert not marker.exists() and browser["desktop_acceptance"] == "unverified"

        missing = dict(env, FLOE_INDEX_BIN=str(work / "missing-index"))
        bad = json.loads(invoke(app, missing, "selfcheck", code=1).stdout)
        assert not bad["checks"][0]["ok"] and bad["checks"][1]["ok"]
        invoke(app, missing, "selfcheck", "--metadata-only")
        for args in [("design.oas",), ("--force",), ("--adjacent", "--adjacent")]:
            invoke(app, env, "selfcheck", *args, code=2)

        # Relocate exactly the three Rust files, then forbid every fallback.
        bundle = work / "portable 경로 with spaces"
        bundle.mkdir()
        for name in ("floe2-web", "floe-index", "floe-renderd"):
            shutil.copy2(BINS / name, bundle / name)
        strict = dict(env, FLOE_INDEX_BIN=str(work / "bad-index"), FLOE_RENDERD_BIN=str(work / "bad-renderd"))
        native = json.loads(invoke(bundle / "floe2-web", strict, "selfcheck", "--adjacent").stdout)
        assert native["ok"] and native["scope"] == "adjacent_only"
        assert [Path(c["path"]) for c in native["checks"]] == [(bundle / "floe-index").resolve(), (bundle / "floe-renderd").resolve()]
        invoke(bundle / "floe2-web", strict, "selfcheck", code=1)

        fake_index = work / "fake-index"
        for payload, diagnostic in [(b"floe-index wrong\n", "expected floe-index"),
                                    (b"\xff\n", "UTF-8"), (b"x" * 4097, "oversized")]:
            script(fake_index, "import sys\nsys.stdout.buffer.write(%r)\n" % payload)
            r = json.loads(invoke(app, dict(env, FLOE_INDEX_BIN=str(fake_index)), "selfcheck", code=1).stdout)
            assert diagnostic in r["checks"][0]["error"] and r["checks"][1]["ok"]
        fake_renderer = script(work / "fake-renderd", "print('invalid ready response', flush=True)\n")
        r = json.loads(invoke(app, dict(env, FLOE_RENDERD_BIN=str(fake_renderer)), "selfcheck", code=1).stdout)
        assert r["checks"][0]["ok"] and not r["checks"][1]["ok"]

        # A successful wrapper exit is not EOF. This explicitly owned test
        # descendant keeps its inherited stdout open until this harness ends it.
        holder_stop = work / "holder.stop"
        holder_done = work / "holder.done"
        script(fake_index, "import os,time\nfrom pathlib import Path\n"
               "print(%r, flush=True)\n"
               "pid=os.fork()\n"
               "if pid == 0:\n"
               "    until=time.monotonic()+30\n"
               "    while time.monotonic()<until and not Path(%r).exists():\n        time.sleep(.01)\n"
               "    Path(%r).touch()\n    os._exit(0)\n"
               "os._exit(0)\n" %
               ("floe-index " + m["index_version"], str(holder_stop), str(holder_done)))
        started = time.monotonic()
        try:
            r = json.loads(invoke(app, dict(env, FLOE_INDEX_BIN=str(fake_index)), "selfcheck", code=1, timeout=9).stdout)
            assert "timed out" in r["checks"][0]["error"]
            assert 4.5 < time.monotonic() - started < 9
        finally:
            # Signal through this private fixture control, never a numeric
            # descendant PID that could have exited and been reused.
            holder_stop.touch()
            until = time.monotonic() + 2
            while not holder_done.exists() and time.monotonic() < until:
                time.sleep(.01)
            assert holder_done.exists(), "stdout holder did not stop"

        pid_file = work / "version.pid"
        script(fake_index, "import os,time\nfrom pathlib import Path\nPath(%r).write_text(str(os.getpid()))\ntime.sleep(30)\n" % str(pid_file))
        p = subprocess.Popen([str(app), "selfcheck"], env=dict(env, FLOE_INDEX_BIN=str(fake_index)),
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            wait_file(pid_file, p)
            p.send_signal(signal.SIGTERM)
            out, err = p.communicate(timeout=3)
            assert p.returncode == 143, (p.returncode, out, err)
            try:
                os.kill(int(pid_file.read_text()), 0)
            except ProcessLookupError:
                pass
            else:
                raise AssertionError("cancelled version child not reaped")
        finally:
            if p.poll() is None:
                p.kill()
                p.wait()
        assert not list(runtime.iterdir())
    print("WEB SELFCHECK: ALL OK (native relocation/PATH empty, identity, no browser/source writes, invalid tools, bounded pipe EOF, cancellation/reap)")


if __name__ == "__main__":
    main()
