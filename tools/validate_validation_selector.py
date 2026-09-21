#!/usr/bin/env python3
"""Exercise the selector and oracle argv; no real builds or layout fixtures."""
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools/validate_rust.sh"


def main():
    source = SCRIPT.read_text()
    prelude, boundary, _ = source.partition("\nRAN=\n")
    assert boundary, "selector/setup boundary changed; audit before executing"
    # Execute only the actual parser and gate predicate. Never execute fixture
    # setup, Cargo or a gate body, including when testing malformed selections.
    probe = prelude + """
for g in $GATES; do
    if gate "$g"; then printf '%s\\n' "$g"; fi
done
"""
    def run(*args):
        return subprocess.run(
            ["sh", "-c", probe, str(SCRIPT), *args], cwd=ROOT,
            capture_output=True, text=True, timeout=10)

    listing = run("--list")
    assert listing.returncode == 0, listing.stderr
    table, marker, aliases = listing.stdout.partition("\naliases:\n")
    assert marker, listing.stdout
    names = [line.strip() for line in table.splitlines()[1:]]
    assert names and len(names) == len(set(names))
    assert set(names) == set(re.findall(r"\bif gate ([a-z][a-z0-9_]*)\b", source)), \
        "listed gates and executable gate guards differ"

    def selected(args, expected):
        result = run(*args)
        assert result.returncode == 0, (args, result.stdout, result.stderr)
        actual = result.stdout.splitlines()
        assert actual == [name for name in names if name in expected], (args, actual)
        return 1

    cases = selected([], set(names))
    cases += selected(["--only=unit"], {"unit"})
    cases += selected(
        ["--only", "unit,representatives", "--only", "web_ui,unit"],
        {"unit", "representatives", "web_ui"})
    for line in aliases.splitlines():
        name, sep, expansion = line.strip().partition(" = ")
        assert sep and expansion, line
        expanded = set(expansion.split())
        assert expanded <= set(names), (name, expanded)
        cases += selected(["--only", name], expanded)
    for args in [
        ["--only"], ["--only="], ["--only", ""],
        ["--only", ",,,"], ["--only= \t,"],
        ["--only", "", "--only="], ["--only", "not_a_gate"],
        ["--only", "unit,not_a_gate"], ["--not-an-option"],
    ]:
        result = run(*args)
        assert result.returncode == 2, (args, result.returncode, result.stdout, result.stderr)
        assert "ALL OK" not in result.stdout
        cases += 1
    # Exercise the real helper with a fake interpreter. Do not start KLayout,
    # read a layout or replace the checkout's Python environment.
    assert len(re.findall(r'^\s*build_legacy_oracle "\$SRC"', source, re.M)) == 1
    with tempfile.TemporaryDirectory(prefix="floe-oracle-argv-") as td:
        root = Path(td)
        python = root / ".venv/bin/python"
        python.parent.mkdir(parents=True)
        python.write_text('#!/bin/sh\nprintf "%s\\n" "$PYTHONPATH" "$@"\nexit "$ORACLE_TEST_EXIT"\n')
        python.chmod(0o700)
        src = str(root / "한국 mask with spaces.oas")
        for code in (0, 17):
            result = subprocess.run(
                ["sh", "-c", prelude + '\ncd "$ORACLE_TEST_ROOT"\nbuild_legacy_oracle "$ORACLE_SOURCE"\n', str(SCRIPT)],
                cwd=ROOT, env=dict(os.environ, ORACLE_TEST_ROOT=td,
                                  ORACLE_SOURCE=src, ORACLE_TEST_EXIT=str(code)),
                capture_output=True, text=True, timeout=10)
            assert result.returncode == code, (result.returncode, result.stderr)
            assert result.stdout.splitlines() == [".", "-m", "floe", "index", "--legacy", src, "--jobs", "1"]
            assert not Path(src).exists()
    print(f"VALIDATION SELECTOR: ALL OK ({len(names)} gates; {cases} parser/predicate cases; 2 isolated oracle argv/exit cases; no real fixture builds)")


if __name__ == "__main__":
    main()
