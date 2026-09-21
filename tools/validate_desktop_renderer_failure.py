#!/usr/bin/env python3
"""Explicit macOS native GUI gate; only a NEW synthetic no-reviewer session.

The development driver kills its own host's verified direct renderd child once.
It never accepts a PID, source, reviewer or output path. Python/ps are test-only;
the product gains no process-discovery API or automatic renderer restart.
"""
import os
from pathlib import Path
import queue
import re
import signal
import subprocess
import sys
import threading
import time
import unittest


def children(rows, parent):
    result = []
    for line in rows.splitlines():
        fields = line.strip().split(None, 2)
        if len(fields) == 3 and fields[0].isdigit() and fields[1].isdigit():
            if int(fields[1]) == parent:
                result.append((int(fields[0]), fields[2]))
    return result


def owned_worker(rows, parent, expected):
    matches = children(rows, parent)
    if len(matches) != 1 or matches[0][1] != expected or matches[0][0] <= 1:
        raise RuntimeError("expected exactly one direct child with the exact renderd executable")
    return matches[0][0]


def process_rows():
    # comm, not command: never inspect argv, environment or authentication data.
    return subprocess.run(["/bin/ps", "-axo", "pid=,ppid=,comm="], check=True,
                          capture_output=True, text=True, timeout=3).stdout


def run(host, renderer):
    proc = subprocess.Popen([str(host), "--smoke-test-renderer-failure"],
                            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT)
    lines = queue.Queue(maxsize=256)
    stopped = threading.Event()

    def read():
        try:
            while not stopped.is_set():
                raw = proc.stdout.readline(4096)
                if not raw:
                    break
                # Retain no raw diagnostics; only fixed QA verdicts leave here.
                text = raw.decode("utf-8", errors="replace").strip()
                if text.startswith("DESKTOP RENDERER "):
                    lines.put(text)
                elif re.fullmatch(r"\[desktop-smoke\] step=(?:5[0-4]) probe=(?:wait|renderer-[a-z-]+)", text):
                    lines.put(text)
        finally:
            lines.put(None)

    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    killed = None
    replacement = None
    reaped = False
    seen = set()
    deadline = time.monotonic() + 240
    next_scan = 0.0
    try:
        while time.monotonic() < deadline:
            try:
                line = lines.get(timeout=0.1)
            except queue.Empty:
                line = ""
            if line is None:
                break
            if line == "DESKTOP RENDERER KILL READY":
                if killed is not None or proc.poll() is not None:
                    raise RuntimeError("invalid renderer-kill readiness")
                candidate = owned_worker(process_rows(), proc.pid, str(renderer))
                # Revalidate immediately before the one targeted injection.
                if candidate != owned_worker(process_rows(), proc.pid, str(renderer)):
                    raise RuntimeError("renderer identity changed before injection")
                os.kill(candidate, signal.SIGKILL)
                killed = candidate
                print("DESKTOP RENDERER DRIVER: injected SIGKILL into verified direct child", flush=True)
            elif line == "DESKTOP RENDERER FAILURE OBSERVED":
                if killed is None:
                    raise RuntimeError("failure was not caused by the requested injection")
                seen.add("failed")
                print(line, flush=True)
            elif line == "DESKTOP RENDERER REOPENED":
                if "failed" not in seen:
                    raise RuntimeError("reopen preceded the failure observation")
                seen.add("reopened")
                print(line, flush=True)
            elif line.startswith("DESKTOP RENDERER FAILURE: OK ("):
                seen.add("completed")
                print(line, flush=True)
            elif line.startswith("DESKTOP RENDERER INPUTS: OK ("):
                seen.add("inputs")
                print(line, flush=True)
            elif line.startswith("[desktop-smoke] step="):
                print(line, flush=True)
            if killed is not None and proc.poll() is None and time.monotonic() >= next_scan:
                next_scan = time.monotonic() + 0.1
                rows = children(process_rows(), proc.pid)
                reaped = reaped or not any(pid == killed for pid, _ in rows)
                for pid, executable in rows:
                    if pid != killed and executable == str(renderer):
                        replacement = pid
        else:
            raise RuntimeError("native renderer failure QA timed out")
        code = proc.wait(timeout=10)
        if code != 0 or seen != {"failed", "reopened", "completed", "inputs"}:
            raise RuntimeError("native renderer failure QA did not finish all verdicts with exit 0")
        if killed is None or not reaped or replacement is None:
            raise RuntimeError("killed renderer reaping and replacement child were not both observed")
        print("DESKTOP RENDERER DRIVER: OK (old child reaped; different child; native exit 0)")
    finally:
        stopped.set()
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                # Only the same still-owned direct worker is eligible for forced
                # cleanup; never search by name across user applications.
                try:
                    candidate = owned_worker(process_rows(), proc.pid, str(renderer))
                    os.kill(candidate, signal.SIGKILL)
                except (RuntimeError, ProcessLookupError, subprocess.SubprocessError):
                    pass
                proc.kill()
                proc.wait(timeout=5)
        reader.join(timeout=2)
        proc.stdout.close()


class OwnershipTests(unittest.TestCase):
    def test_exact_parent_and_executable_with_spaces(self):
        rows = " 42 99 /private/tmp/Other.app/renderd\n 51 11 /private/tmp/space dir/floe-renderd\n"
        self.assertEqual(owned_worker(rows, 11, "/private/tmp/space dir/floe-renderd"), 51)

    def test_missing_wrong_or_ambiguous_child_is_never_a_target(self):
        for rows in ["", "51 12 /r", "51 11 /other", "51 11 /r\n52 11 /r",
                     "51 11 /r\n52 11 /index", "1 11 /r", "invalid ps row"]:
            with self.subTest(rows=rows), self.assertRaises(RuntimeError):
                owned_worker(rows, 11, "/r")


def main():
    if sys.argv[1:] == ["--self-test"]:
        unittest.main(argv=[sys.argv[0]])
        return
    if sys.platform != "darwin" or len(sys.argv) != 2:
        raise SystemExit("usage (macOS only): validate_desktop_renderer_failure.py FLOE2-DESKTOP-BINARY")
    host = Path(sys.argv[1]).resolve(strict=True)
    renderer = Path(os.environ.get("FLOE_RENDERD_BIN", host.parent / "floe-renderd")).resolve(strict=True)
    if not host.is_file() or not renderer.is_file() or not os.access(host, os.X_OK) or not os.access(renderer, os.X_OK):
        raise SystemExit("host and renderer must be executable files")
    try:
        run(host, renderer)
    except RuntimeError as error:
        # These are only the driver's fixed, non-sensitive verdicts above.
        raise SystemExit(f"DESKTOP RENDERER DRIVER: FAIL ({error})")
    except (OSError, subprocess.SubprocessError):
        # Avoid echoing paths/authentication from unexpected OS/host diagnostics.
        raise SystemExit("DESKTOP RENDERER DRIVER: FAIL (incomplete QA; no raw session diagnostics emitted)")


if __name__ == "__main__":
    main()
