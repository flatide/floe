"""Where the viewer finds the Rust executables it runs: floe-index
(find_binary) and the floe2 command line (find_floe2).

The vfsd client that lived here (the frozen floe's KLayout tile path)
is the dev-only oracle's since P3 (tools/oracle/floe_oracle/vfsclient.py,
docs/SHARED_APP_LAYER.ko.md)."""


import os
import shutil
import sys


def find_binary():
    """Find the matched floe-index used by indexing and VFS clients.

    An explicit override wins, followed by the development build, the
    portable slot beside Python, and PATH.  Keeping PATH last prevents an
    older system binary from silently taking precedence over the package.
    """
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    candidates = []
    configured = os.environ.get("FLOE_INDEX_BIN")
    if configured:
        if os.path.isfile(configured) and os.access(configured, os.X_OK):
            return os.path.abspath(configured)
        raise RuntimeError(
            "FLOE_INDEX_BIN is set but is not an executable file: %s" %
            configured)
    candidates.extend((
        os.path.join(here, "rust", "target", "release", "floe-index"),
        os.path.join(os.path.dirname(sys.executable), "floe-index"),
    ))
    on_path = shutil.which("floe-index")
    if on_path:
        candidates.append(on_path)
    for cand in candidates:
        if os.path.isfile(cand) and os.access(cand, os.X_OK):
            return os.path.abspath(cand)
    raise RuntimeError(
        "floe-index binary not found; set FLOE_INDEX_BIN, build it with "
        "'cd rust && cargo build --release -p floe-index', or install it "
        "beside Python/on PATH (checked: %s)" %
        ", ".join(candidates or ["no candidates"]))


def find_floe2():
    """The Rust command line `floe2` (rust/floe2, docs/SHARED_APP_LAYER.ko.md):
    FLOE2_BIN - its launcher sets it when it starts the viewer - else the
    development build, the portable slot beside Python, and PATH."""
    configured = os.environ.get("FLOE2_BIN")
    if configured and os.path.isfile(configured) and \
            os.access(configured, os.X_OK):
        return os.path.abspath(configured)
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    candidates = [os.path.join(here, "rust", "target", "release", "floe2"),
                  os.path.join(os.path.dirname(sys.executable), "floe2")]
    on_path = shutil.which("floe2")
    if on_path:
        candidates.append(on_path)
    for cand in candidates:
        if os.path.isfile(cand) and os.access(cand, os.X_OK):
            return os.path.abspath(cand)
    raise RuntimeError(
        "the floe2 command line (Rust) not found; set FLOE2_BIN or build it "
        "with 'cd rust && cargo build --release -p floe2' (checked: %s)" %
        ", ".join(candidates))
