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
    def __init__(self, floe_dir, budget_mb=1024, binary=None,
                 stream_kb=None):
        """stream_kb: per-response cap on new hier payload (the
        progressive first paint); None = daemon default (24576),
        0 disables streaming."""
        self.dir = floe_dir
        self.tmp = tempfile.mkdtemp(prefix="floe_vfs_")
        self._last_files = []
        args = [binary or find_binary(), "vfsd", floe_dir,
                "--budget-mb", str(budget_mb)]
        if stream_kb is not None:
            args += ["--stream-kb", str(stream_kb)]
        self.proc = subprocess.Popen(
            args,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, bufsize=1)

    def request(self, gen, view_um, px_per_um, cut_px, layers=None,
                depth=None, probe=False, hier=False, ack=0,
                reset=False):
        """One plan/materialize round-trip. view_um = (x0,y0,x1,y1)
        in um; layers = [(l,d), ...] or None (=all); depth None =
        full. Returns the parsed response dict with 'mats' (list of
        (cellname, x, y, rot, flip)) and 'delta' (path or None);
        response files are deleted on the NEXT request.

        hier=True speaks the V4 protocol (rust/VFS_HIER.md par.3.5):
        the delta carries the whole working-set hierarchy, the
        response adds 'top' (gen top WC name) and 'names' (ci->name
        table path, once per daemon run); ack/reset drive the
        par.3.7 session transaction. probe+hier = mode=hier_probe
        (session-less)."""
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
        if hier:
            if probe:
                line += " mode=hier_probe"
            else:
                line += f" mode=hier ack={ack}"
                if reset:
                    line += " reset=1"
        elif probe:
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
        top = out.get("top", "-")
        out["top"] = None if top in ("-", "") else top
        nm = out.get("names", "-")
        out["names"] = None if nm in ("-", "") else nm
        fr = out.get("frames", "-")
        out["frames"] = None if fr in ("-", "") else fr
        if out["frames"]:
            self._last_files.append(out["frames"])
        mats = []
        mp = out.get("placements")
        if mp and mp != "-" and os.path.isfile(mp):
            with open(mp) as f:
                for ln in f:
                    p = ln.rstrip("\n").split("\t")
                    if len(p) >= 12:
                        # name x y rot flip na nb vax vay vbx vby design
                        mats.append((
                            p[0], int(p[1]), int(p[2]), int(p[3]),
                            p[4] == "1", int(p[5]), int(p[6]),
                            (int(p[7]), int(p[8])),
                            (int(p[9]), int(p[10])), p[11]))
                    elif len(p) >= 5:  # legacy single-placement rows
                        mats.append((
                            p[0], int(p[1]), int(p[2]), int(p[3]),
                            p[4] == "1", 1, 1, (0, 0), (0, 0),
                            p[5] if len(p) > 5 else p[0]))
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
