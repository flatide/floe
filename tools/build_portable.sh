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
# By default this uses the LATEST CPython (auto-resolved from the newest
# python-build-standalone release for this arch) and the LATEST
# klayout/numpy/pillow (unpinned pip). Override any of it via env:
#   PBS_PY       CPython line to pick, e.g. "3.13" or a full "3.13.1".
#                Unset = newest available. (klayout must have a wheel for
#                it; set a lower line if pip can't find one.)
#   PBS_TARBALL  URL or local path to a python-build-standalone
#                "*-install_only.tar.gz" (tkinter is built in). Set this on
#                a closed-network build machine that can't reach GitHub.
#   WHEELS       local wheel dir -> installs with --no-index (closed net)
#   FLOE_SRC     path to the floe package dir (default: repo's floe/)
#   OUT          output tarball (default: floe-portable-<arch>.tar.gz)
#   ARCH         target triple (default: <machine>-unknown-linux-gnu)
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
FLOE_SRC="${FLOE_SRC:-$here/../floe}"
ARCH="${ARCH:-$(uname -m)-unknown-linux-gnu}"
OUT="${OUT:-floe-portable-$(uname -m).tar.gz}"
API_URL="https://api.github.com/repos/astral-sh/python-build-standalone/releases/latest"

fetch() {  # fetch $1 -> stdout
    if command -v curl >/dev/null; then curl -fsSL "$1"
    else wget -qO- "$1"; fi
}

resolve_latest() {  # newest install_only tarball URL for ARCH (+ PBS_PY)
    local urls
    urls="$(fetch "$API_URL" \
        | grep -oE 'https://[^"]*-'"$ARCH"'-install_only\.tar\.gz')" || return 1
    if [ -n "${PBS_PY:-}" ]; then
        urls="$(printf '%s\n' "$urls" \
            | grep -E "/cpython-${PBS_PY//./\\.}(\.[0-9]+)?\+")"
    fi
    # sort by the embedded x.y.z version, take the newest
    printf '%s\n' "$urls" \
        | sed -E 's#.*/cpython-([0-9]+\.[0-9]+\.[0-9]+)\+.*#\1 &#' \
        | sort -V | tail -1 | cut -d' ' -f2-
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
echo "[portable] work dir: $work"

# 1) fetch/extract the relocatable, Tk-bundled Python -------------------
tb="$work/python.tar.gz"
if [ -n "${PBS_TARBALL:-}" ] && [ -f "$PBS_TARBALL" ]; then
    echo "[portable] using local $PBS_TARBALL"
    cp "$PBS_TARBALL" "$tb"
else
    url="${PBS_TARBALL:-$(resolve_latest)}"
    [ -n "$url" ] || { echo "could not resolve a python-build-standalone \
tarball for $ARCH (set PBS_TARBALL to a local file/url)"; exit 1; }
    echo "[portable] downloading $url"
    fetch "$url" > "$tb"
fi
tar -C "$work" -xzf "$tb"          # -> $work/python/
PY="$work/python/bin/python3"
[ -x "$PY" ] || { echo "no python at $PY (bad tarball layout?)"; exit 1; }
echo "[portable] python $("$PY" -V 2>&1 | awk '{print $2}')"

# tkinter must be present in the standalone build (it is, for PBS)
"$PY" -c "import tkinter; print('[portable] bundled tk', tkinter.TkVersion)"

# 2) install the pip deps into it (latest; force wheels, no source build)
"$PY" -m pip install --upgrade pip >/dev/null
if [ -n "${WHEELS:-}" ]; then
    echo "[portable] installing from local wheels: $WHEELS"
    "$PY" -m pip install --no-index --find-links "$WHEELS" \
        --only-binary=:all: klayout numpy pillow
else
    "$PY" -m pip install --only-binary=:all: klayout numpy pillow
fi
echo "[portable] klayout $("$PY" -c 'import klayout; print(klayout.__version__)')"

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
