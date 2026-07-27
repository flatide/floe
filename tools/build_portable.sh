#!/usr/bin/env bash
# Build a fully self-contained "floe-portable" bundle for locked-down
# Linux hosts (no root, no apt, no system Python, no PyGObject).
#
# The bundle is a relocatable python-build-standalone CPython - which
# ALREADY includes tkinter + Tcl/Tk - with klayout/numpy/pillow (all
# manylinux wheels, self-contained .so) and the floe package installed
# into it. Ship the tarball, extract anywhere, run:
#
#     tar xzf floe-portable-<arch>.tar.gz
#     ./python/bin/python -m floe view data/chip.oas
#
# Run THIS script on an internet- (or mirror-) capable Linux machine of
# the SAME arch as the target. It does not need root either.
#
# Env overrides:
#   PBS_TARBALL  URL or local path to a python-build-standalone
#                "*-install_only.tar.gz" (tkinter is built in). If unset,
#                downloads the pinned release below.
#   WHEELS       local wheel dir -> installs with --no-index (closed net)
#   FLOE_SRC     path to the floe package dir (default: repo's floe/)
#   OUT          output tarball (default: floe-portable-<arch>.tar.gz)
#   ARCH         target triple for the default PBS url (default: host)
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
FLOE_SRC="${FLOE_SRC:-$here/../floe}"
ARCH="${ARCH:-$(uname -m)-unknown-linux-gnu}"

# Pinned python-build-standalone release (override PBS_TARBALL to change).
# Pick the CPython version you want the bundle to run; it bundles Tk 8.6.
PBS_TAG="${PBS_TAG:-20240814}"
PBS_PY="${PBS_PY:-3.11.9}"
PBS_BASE="https://github.com/astral-sh/python-build-standalone/releases/download"
PBS_TARBALL="${PBS_TARBALL:-$PBS_BASE/$PBS_TAG/cpython-$PBS_PY+$PBS_TAG-$ARCH-install_only.tar.gz}"

OUT="${OUT:-floe-portable-$(uname -m).tar.gz}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
echo "[portable] work dir: $work"

# 1) fetch/extract the relocatable, Tk-bundled Python -------------------
tb="$work/python.tar.gz"
if [ -f "$PBS_TARBALL" ]; then
    cp "$PBS_TARBALL" "$tb"
else
    echo "[portable] downloading $PBS_TARBALL"
    if command -v curl >/dev/null; then curl -fL "$PBS_TARBALL" -o "$tb"
    else wget -O "$tb" "$PBS_TARBALL"; fi
fi
tar -C "$work" -xzf "$tb"          # -> $work/python/
PY="$work/python/bin/python3"
[ -x "$PY" ] || { echo "no python at $PY (bad tarball layout?)"; exit 1; }

# tkinter must be present in the standalone build (it is, for PBS)
"$PY" -c "import tkinter; print('[portable] bundled tk', tkinter.TkVersion)"

# 2) install the pip deps into it (self-contained wheels) ---------------
"$PY" -m pip install --upgrade pip >/dev/null
if [ -n "${WHEELS:-}" ]; then
    echo "[portable] installing from local wheels: $WHEELS"
    "$PY" -m pip install --no-index --find-links "$WHEELS" klayout numpy pillow
else
    "$PY" -m pip install klayout numpy pillow
fi

# 3) drop the floe package into site-packages ---------------------------
site="$(cd "$work/python" && ./bin/python3 -c \
    'import site; print(site.getsitepackages()[0])')"
echo "[portable] site-packages: $site"
rm -rf "$site/floe"
cp -r "$FLOE_SRC" "$site/floe"
find "$site/floe" -name '__pycache__' -type d -prune -exec rm -rf {} +

# 4) verify the whole stack imports and floe runs -----------------------
"$PY" -c "import tkinter, klayout.db, numpy, PIL, floe; \
tkinter.Tk(); print('[portable] all imports OK, PIL', PIL.__version__)"
"$PY" -m floe --version

# 5) package ------------------------------------------------------------
tar -C "$work" -czf "$OUT" python
echo
echo "[portable] wrote $OUT ($(du -h "$OUT" | cut -f1))"
echo "[portable] on the target host:"
echo "    tar xzf $(basename "$OUT")"
echo "    ./python/bin/python -m floe view <file.oas>"
