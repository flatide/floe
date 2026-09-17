#!/usr/bin/env python3
"""Observer-only timing: no retries, changed limits, hidden failure or argv log."""
import contextlib
import io
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
    print("WEB STARTUP TIMING: ALL OK (6 outcomes, unchanged call/deadline, no retry or argument log)")


if __name__ == "__main__":
    main()
