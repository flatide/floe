#!/bin/sh
# Fast Rust-indexer validation loop. Default asset: valmini - the
# small adversarial layout (rotated/mirrored placements, 3 hierarchy
# levels, dense + straddling arrays, non-manhattan polygons, texts) -
# whose whole scan/XOR/depth trio runs in seconds. Big assets (midi,
# stress30 class) stay as occasional milestone gates: their oracle
# XOR alone costs minutes to an hour.
#
#   sh tools/validate_rust.sh                # valmini (generated on
#                                            # first use under $TMPDIR)
#   sh tools/validate_rust.sh path/to.oas    # any indexed asset
set -e
cd "$(dirname "$0")/.."
SRC=${1:-}
if [ -z "$SRC" ]; then
    SRC=${TMPDIR:-/tmp}/floe-valmini/valmini.oas
    mkdir -p "$(dirname "$SRC")"
    # regenerate when the generator changed (asset evolves with the
    # milestones: new record kinds get added here first)
    if [ ! -f "$SRC" ] || [ tools/gen_valmini.py -nt "$SRC" ]; then
        rm -rf "$SRC" "$SRC.tiles" "${SRC%.oas}_rust.tiles"
        .venv/bin/python tools/gen_valmini.py "$SRC"
    fi
    # the python .tiles is the meta-parity oracle: refresh it when the
    # python indexer itself changed, not only when the asset did (the
    # layer-palette change tripped this once - stale colors failed
    # validate_rust_meta on every host with an old cached .tiles)
    if [ ! -f "$SRC.tiles/meta.json" ] || \
       [ floe/cache.py -nt "$SRC.tiles/meta.json" ]; then
        rm -rf "$SRC.tiles"
        PYTHONPATH=. .venv/bin/python -m floe index "$SRC" >/dev/null
    fi
fi
(cd rust && PATH="$HOME/.cargo/bin:$PATH" \
    cargo build --release 2>/dev/null >/dev/null)
OUT="${SRC%.oas}_rust.tiles"
.venv/bin/python tools/validate_rust_scan.py "$SRC"
.venv/bin/python tools/validate_rust_tiles.py "$SRC" "$OUT"
.venv/bin/python tools/validate_rust_depth.py "$SRC" "$OUT"
.venv/bin/python tools/validate_rust_meta.py "$SRC" "$OUT"
.venv/bin/python tools/validate_rust_skel.py "$SRC" "$OUT"
# VFS V1 (rust/VFS.md): build .floe and run the G5/G6 gates
VOUT="${SRC%.oas}_rust.floe"
rm -rf "$VOUT"
rust/target/release/floe-index vfs "$SRC" "$VOUT" \
    --coverage --slow-cell-s 0 >/dev/null 2> "$VOUT.buildlog"
# small assets must never fan out (#60): no P2 frontier, no split
# helper threads - the thresholds keep tiny cells on the fast
# serial path
if grep -q "p2_tasks=" "$VOUT.buildlog"; then
    echo "FAIL: P2 frontier engaged on valmini"; exit 1
fi
if grep -Eq "split [0-9.]+/([2-9]|[1-9][0-9])t" "$VOUT.buildlog"; then
    echo "FAIL: split fanout engaged on valmini"; exit 1
fi
rm -f "$VOUT.buildlog"
.venv/bin/python tools/validate_vfs.py "$SRC" "$VOUT"
.venv/bin/python tools/validate_vfs_render.py "$SRC" "$VOUT"
.venv/bin/python tools/validate_vfs_coverage.py "$SRC" "$VOUT"
.venv/bin/python tools/validate_vfs_hier.py "$SRC" "$VOUT"
.venv/bin/python tools/validate_vfs_lifecycle.py "$SRC" "$VOUT"
.venv/bin/python tools/validate_vfs_marker.py "$SRC"
# rep-split page honesty (ovm v3): floor collapse + fragment
# conservation on a synthetic rep-flood asset
.venv/bin/python tools/validate_vfs_split.py
# v5 text index: oracle XOR, declutter, corrupt, determinism,
# daemon label lifecycle
.venv/bin/python tools/validate_vfs_text.py
# viewer speckle fill: common phase, opaque overlap, and the
# coverage composite staying out of speckled interiors
.venv/bin/python tools/validate_render_speckle.py
# frame outline stacking: white over gray, 1px hollow, under design
.venv/bin/python tools/validate_render_frames.py
# DRC .ice index sidecar: reading through the index == ASCII parse
.venv/bin/python tools/validate_drc_ice.py
# SVRF subset parser: preprocessing / derivation closure / check
# extraction / end-to-end vs gen_drcdb --svrf
.venv/bin/python tools/validate_svrf.py
echo "RUST VALIDATION: ALL OK ($SRC)"
