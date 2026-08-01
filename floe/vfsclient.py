"""Client for the floe-index vfsd sidecar daemon (rust/VFS.md).

The render service spawns one vfsd per cache and talks a line-based
kv protocol over stdin/stdout. Responses reference a delta OASIS
file (new working-set pages, spliced bytes) and a placements TSV;
both live in a private temp dir and are deleted after the caller
applies them.
"""

import os
import shutil
import subprocess
import sys
import tempfile


def find_binary():
    """floe-index discovery: PATH, then the dev tree, then next to
    the interpreter (venv/portable layouts)."""
    p = shutil.which("floe-index")
    if p:
        return p
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    for cand in (
        os.path.join(here, "rust", "target", "release", "floe-index"),
        os.path.join(here, "dist", "floe-index-linux-gnu"),
        os.path.join(here, "dist", "floe-index-linux-x86_64"),
        os.path.join(os.path.dirname(sys.executable), "floe-index"),
    ):
        if os.path.isfile(cand) and os.access(cand, os.X_OK):
            return cand
    raise RuntimeError(
        "floe-index binary not found (PATH, rust/target/release, "
        "dist/) - the VFS cache needs the rust daemon")


class VfsClient:
    def __init__(self, floe_dir, budget_mb=1024, binary=None):
        self.dir = floe_dir
        self.tmp = tempfile.mkdtemp(prefix="floe_vfs_")
        self._last_files = []
        self.proc = subprocess.Popen(
            [binary or find_binary(), "vfsd", floe_dir,
             "--budget-mb", str(budget_mb)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, bufsize=1)

    def request(self, gen, view_um, px_per_um, cut_px, layers=None,
                depth=None, probe=False):
        """One plan/materialize round-trip. view_um = (x0,y0,x1,y1)
        in um; layers = [(l,d), ...] or None (=all); depth None =
        full. Returns the parsed response dict with 'mats' (list of
        (cellname, x, y, rot, flip)) and 'delta' (path or None);
        response files are deleted on the NEXT request."""
        for p in self._last_files:
            try:
                os.unlink(p)
            except OSError:
                pass
        self._last_files = []
        lspec = "all" if layers is None else ",".join(
            f"{l}/{d}" for (l, d) in layers) or "all"
        dspec = "full" if depth is None else str(max(0, int(depth)))
        line = (f"gen={gen} view={view_um[0]},{view_um[1]},"
                f"{view_um[2]},{view_um[3]} px={px_per_um} "
                f"cut={cut_px} depth={dspec} layers={lspec} "
                f"out={self.tmp}")
        if probe:
            line += " mode=probe"
        self.proc.stdin.write(line + "\n")
        self.proc.stdin.flush()
        resp = self.proc.stdout.readline()
        if not resp:
            raise RuntimeError("vfsd died")
        out = {}
        for tok in resp.split():
            k, _, v = tok.partition("=")
            out[k] = v
        if "error" in out:
            raise RuntimeError(f"vfsd: {out['error']}")
        delta = out.get("delta", "-")
        out["delta"] = None if delta == "-" else delta
        fr = out.get("frames", "-")
        out["frames"] = None if fr in ("-", "") else fr
        if out["frames"]:
            self._last_files.append(out["frames"])
        mats = []
        mp = out.get("placements")
        if mp and mp != "-" and os.path.isfile(mp):
            with open(mp) as f:
                for ln in f:
                    parts = ln.rstrip("\n").split("\t")
                    if len(parts) >= 5:
                        mats.append((parts[0], int(parts[1]),
                                     int(parts[2]), int(parts[3]),
                                     parts[4] == "1",
                                     parts[5] if len(parts) > 5
                                     else parts[0]))
            self._last_files.append(mp)
        if out["delta"]:
            self._last_files.append(out["delta"])
        out["mats"] = mats
        ev = out.get("evict", "-")
        out["evict"] = [] if ev in ("-", "") else ev.split(",")
        for k in ("pages", "new", "bytes", "members", "nframes"):
            if k in out:
                try:
                    out[k] = int(out[k])
                except ValueError:
                    pass
        return out

    def stop(self):
        try:
            self.proc.stdin.write("quit\n")
            self.proc.stdin.flush()
        except Exception:
            pass
        try:
            self.proc.wait(timeout=1.0)
        except Exception:
            self.proc.kill()
        shutil.rmtree(self.tmp, ignore_errors=True)
