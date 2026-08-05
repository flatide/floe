"""Marker-protocol gate under FORCED KILLS (VFS_HIER.md par.3.6/7).

The indexer dies (--kill-at, a gate-only hook) at each of the
three interrupt points; opening the cache afterwards must say
"no cache" or "corrupt cache" - an interrupted build must NEVER look
like a valid cache. A final clean rebuild proves recovery.

  marker-deleted : killed right after the marker (design.ovm) died
  ovp-written    : killed after design.ovp, before the marker
  ovt-written    : killed after design.ovp+design.ovt, before it
  ovm-partial    : killed mid-marker-write (half the ovm bytes)

usage: python tools/validate_vfs_marker.py <src.oas> [bin]
"""
import functools
import os
import shutil
import subprocess
import sys
import tempfile

print = functools.partial(print, flush=True)


def main():
    src = sys.argv[1]
    fi = sys.argv[2] if len(sys.argv) > 2 else os.path.join(
        os.path.dirname(__file__), "..",
        "rust/target/release/floe-index")
    bad = []

    def chk(cond, msg):
        if not cond:
            bad.append(msg)
            print("FAIL", msg)

    out = tempfile.mkdtemp(prefix="floe_marker_") + "/c.floe"

    def build(kill=None):
        cmd = [fi, "vfs", src, out]
        if kill:
            cmd += ["--kill-at", kill]
        return subprocess.run(
            cmd,
            capture_output=True, text=True)

    def open_err():
        """plan against the cache; '' = opened fine"""
        p = subprocess.run(
            [fi, "plan", out, "--view", "0,0,1,1"],
            capture_output=True, text=True)
        if p.returncode == 0:
            return ""
        return (p.stderr.strip().splitlines() or ["?"])[-1]

    # control: a clean build opens
    r = build()
    chk(r.returncode == 0, "control build failed: %s" % r.stderr[-200:])
    chk(open_err() == "", "control cache does not open")

    for kill, want in [
        ("marker-deleted", ("No such file", "no cache")),
        ("ovp-written", ("No such file", "no cache")),
        ("ovt-written", ("No such file", "no cache")),
        ("ovm-partial", ("corrupt cache", "not an ovm")),
    ]:
        r = build(kill)
        chk(r.returncode != 0, "%s: indexer survived" % kill)
        e = open_err()
        chk(e != "", "%s: cache still opens" % kill)
        chk(any(w in e for w in want),
            "%s: unexpected error %r" % (kill, e))

    # recovery: ops rule 1 - just rebuild
    r = build()
    chk(r.returncode == 0, "rebuild failed")
    chk(open_err() == "", "rebuilt cache does not open")

    shutil.rmtree(os.path.dirname(out), ignore_errors=True)
    print("vfs-marker-checked 4 kill points + rebuild, failures: %d"
          % len(bad))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
