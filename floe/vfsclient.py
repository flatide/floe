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
                depth=None, probe=False, ack=0,
                reset=False, stream_kb=None, want_labels=True,
                lod=True, frames=True, labels=True):
        """One plan/materialize round-trip on the V4 hier protocol
        (rust/VFS_HIER.md par.3.5; flat retired in M5). view_um =
        (x0,y0,x1,y1) in um; layers = [(l,d), ...] or None (=all);
        depth None = full. The delta carries the whole working-set
        hierarchy; the response adds 'top' (gen top WC name) and
        'names' (ci->name table path, once per daemon run);
        lod selects merged variants per request; frames controls the
        hierarchy frontier and its block names; labels controls all
        request-scoped text planning;
        ack/reset drive the par.3.7 session transaction; probe =
        session-less exact query. Response files are deleted on
        the NEXT request."""
        for p in self._last_files:
            try:
                os.unlink(p)
            except OSError:
                pass
        self._last_files = []
        # None and [] are deliberately distinct: an empty design-layer
        # selection still requests finite-depth hierarchy frontiers.
        lspec = ("all" if layers is None else
                 ",".join(f"{l}/{d}" for (l, d) in layers) or "none")
        dspec = "full" if depth is None else str(max(0, int(depth)))
        line = (f"gen={gen} view={view_um[0]},{view_um[1]},"
                f"{view_um[2]},{view_um[3]} px={px_per_um} "
                f"cut={cut_px} depth={dspec} layers={lspec} "
                f"lod={1 if lod else 0} "
                f"frames={1 if frames else 0} "
                f"labels={1 if labels else 0} out={self.tmp}")
        # rev 41 hairline factor: min-side cut = FLOE_HAIRLINE * cut.
        # Field-tuning knob; unset keeps the daemon default (0.5),
        # 0 disables the hairline cut entirely.
        hair = os.environ.get("FLOE_HAIRLINE")
        if hair:
            line += f" hair={float(hair):g}"
        # rev 45 thin-frame lattice pitch (um): boundary boxes with
        # min side under the cut keep lattice representatives
        # instead of vanishing. Unset keeps the daemon default
        # (7.0), 0 restores the rev 41 frame cull.
        thin = os.environ.get("FLOE_THIN_UM")
        if thin:
            line += f" thin={float(thin):g}"
        if probe:
            line += " mode=probe"
        else:
            line += f" ack={ack}"
            if reset:
                line += " reset=1"
            if stream_kb is not None:
                # per-request budget override: the service adapts
                # it to its measured parse speed
                line += f" stream={int(stream_kb)}"
            if not want_labels:
                # refinement rounds: labels came with round 1
                line += " nolabels=1"
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
        if out["delta"]:
            self._last_files.append(out["delta"])
        # per-gen label file (v5): request-scoped like the delta -
        # read it this round, deleted on the NEXT request
        lb = out.get("labels", "-")
        out["labels"] = None if lb in ("-", "") else lb
        if out["labels"]:
            self._last_files.append(out["labels"])
        ev = out.get("evict", "-")
        out["evict"] = [] if ev in ("-", "") else ev.split(",")
        for k in ("pages", "new", "bytes", "members", "lod",
                  "max_depth",
                  "wc_cells", "inst_edges", "frame_rects",
                  "nlabels", "labels_truncated", "text_bvh_nodes",
                  "text_place_bvh_nodes", "text_place_records",
                  "text_members_tested", "text_members_visible"):
            if k in out:
                try:
                    out[k] = int(out[k])
                except ValueError:
                    pass
        for k in ("plan_ms", "text_plan_ms"):
            if k in out:
                try:
                    out[k] = float(out[k])
                except ValueError:
                    pass
        return out

    def frontier(self, gen, view_um, px_per_um, cut_px, depth):
        """rev 46 minimap frontier: session-less request planning at
        the CANVAS fit scale; returns [(x0, y0, x1, y1, band)] in
        dbu, the exact frame set a fit view at this depth draws
        (spatially capped daemon-side). The TSV is consumed and
        deleted here."""
        line = (f"gen={gen} mode=frontier view={view_um[0]},"
                f"{view_um[1]},{view_um[2]},{view_um[3]} "
                f"px={px_per_um} cut={cut_px} "
                f"depth={max(0, int(depth))} layers=all "
                f"out={self.tmp}")
        hair = os.environ.get("FLOE_HAIRLINE")
        if hair:
            line += f" hair={float(hair):g}"
        thin = os.environ.get("FLOE_THIN_UM")
        if thin:
            line += f" thin={float(thin):g}"
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
        path = out.get("frontier", "-")
        rows = []
        if path not in ("-", ""):
            try:
                with open(path) as f:
                    for ln in f:
                        p = ln.split()
                        if len(p) >= 5:
                            rows.append((int(p[0]), int(p[1]),
                                         int(p[2]), int(p[3]),
                                         int(p[4])))
            finally:
                try:
                    os.unlink(path)
                except OSError:
                    pass
        return rows

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
