#!/usr/bin/env python3
"""Exercise the battery's actual selector without building or touching fixtures."""
from pathlib import Path
import re
import subprocess
import sys

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
    print(f"VALIDATION SELECTOR: ALL OK ({len(names)} gates; {cases} parser/predicate cases; no setup or test execution)")


if __name__ == "__main__":
    main()
