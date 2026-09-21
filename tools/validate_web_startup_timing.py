#!/usr/bin/env python3
"""Observer-only timing: no retries, changed limits, hidden failure or argv log."""
import contextlib
import io
import json
import subprocess
from unittest.mock import patch

import validate_web_startup as startup


def main():
    argv = ["synthetic-oracle", "private-argument-not-for-logs"]
    options = dict(env={"PATH": "", "PRIVATE_VALUE": "not-for-logs"},
                   capture_output=True, text=True, timeout=30)
    cases = [subprocess.CompletedProcess(argv, code, "native output", "native error")
             for code in (0, 1, 101)]
    cases += [subprocess.TimeoutExpired(argv, 30, output="partial"),
              OSError("launch failed"), KeyboardInterrupt()]
    for result in cases:
        stream = io.StringIO()
        with patch.object(startup.time, "monotonic", side_effect=[100.0, 102.5]), \
                patch.object(startup.subprocess, "run") as run, \
                contextlib.redirect_stdout(stream):
            if isinstance(result, BaseException):
                run.side_effect = result
                try:
                    startup.measured_run("synthetic", argv, **options)
                except BaseException as caught:
                    assert caught is result, "observer replaced the original failure"
                else:
                    raise AssertionError("observer swallowed failure")
            else:
                run.return_value = result
                assert startup.measured_run("synthetic", argv, **options) is result
            run.assert_called_once_with(argv, **options)
        outcome = ("timeout" if isinstance(result, subprocess.TimeoutExpired) else
                   "launch-error" if isinstance(result, OSError) else
                   "interrupted" if isinstance(result, BaseException) else
                   f"exit={result.returncode}")
        assert stream.getvalue().splitlines() == [
            "WEB STARTUP STAGE: synthetic begin",
            f"WEB STARTUP STAGE: synthetic {outcome} wall=2.500s"]
    records = [
        dict(reason="compiler-artifact", fresh=True, executable="private-path"),
        dict(reason="compiler-artifact", fresh=False),
        dict(reason="compiler-artifact", fresh=1),
        dict(reason="compiler-message", message={"rendered": "private-message"}),
        dict(reason="build-script-executed", env=["private-environment"]),
        dict(reason="build-finished", success=True),
        dict(reason="private-reason"), [],
    ]
    output = "\n".join(map(json.dumps, records)) + "\nprivate-partial-json\n"
    error = ("   Blocking waiting for file lock on build directory\n"
             "Blocking waiting for file lock on package cache\nprivate-stderr\n")
    want = dict(fresh=1, built=1, artifacts_other=1, messages=1, scripts=1,
                finished=True, build_lock=1, cache_lock=1, ignored=3, truncated=False)
    assert startup.cargo_progress(output, error) == want
    assert startup.cargo_progress(output.encode(), error.encode()) == want
    assert startup.cargo_progress(None, None)["finished"] is None
    assert startup.cargo_progress('{"reason":"build-finished","success":1}', b'')["finished"] is None
    assert startup.cargo_progress('{"reason":"build-finished","success":false}', '')["finished"] is False
    assert startup.cargo_progress(b'\xff\xfe', b'\xff')["ignored"] == 1
    bounded = startup.cargo_progress('x' * (1024 * 1024 + 1) + '\n' + output,
                                     b'x' * (1024 * 1024 + 1) + b'\n' + error.encode())
    assert bounded == dict(want, ignored=4, truncated=True)
    assert startup.cargo_progress('한' * 400000, '')["truncated"] is True
    nested = startup.cargo_progress('[' * 2000 + ']' * 2000, '')
    assert nested["ignored"] == 1  # malformed/deep diagnostics cannot replace the failure
    # Same subprocess call and same exception even when a partial JSON stream
    # reports build-finished. Completion is decided by the original caller.
    cargo_options = dict(options, timeout=180)
    for result in [subprocess.CompletedProcess(argv, 101, output, error),
                   subprocess.TimeoutExpired(argv, 180, output=output.encode(), stderr=error.encode())]:
        stream = io.StringIO()
        with patch.object(startup.time, "monotonic", side_effect=[100.0, 280.0]), \
                patch.object(startup.subprocess, "run") as run, contextlib.redirect_stdout(stream):
            if isinstance(result, BaseException):
                run.side_effect = result
                try:
                    startup.measured_run("oracle-build", argv, report_cargo=True, **cargo_options)
                except subprocess.TimeoutExpired as caught:
                    assert caught is result
                else:
                    raise AssertionError("Cargo observer swallowed timeout")
            else:
                run.return_value = result
                assert startup.measured_run("oracle-build", argv, report_cargo=True, **cargo_options) is result
            run.assert_called_once_with(argv, **cargo_options)
        lines = stream.getvalue().splitlines()
        assert len(lines) == 3
        assert lines[2].startswith("WEB STARTUP CARGO: ")
        assert json.loads(lines[2].removeprefix("WEB STARTUP CARGO: ")) == want
        assert "private" not in stream.getvalue()
    print("WEB STARTUP TIMING: ALL OK (6 outcomes + bounded/redacted Cargo progress; unchanged calls/deadlines, no retry)")


if __name__ == "__main__":
    main()
