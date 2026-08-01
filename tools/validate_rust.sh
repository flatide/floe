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
        rm -rf "$SRC" "$SRC.ice" "${SRC%.oas}_rust.ice"
        .venv/bin/python tools/gen_valmini.py "$SRC"
    fi
    [ -f "$SRC.ice/meta.json" ] || \
        PYTHONPATH=. .venv/bin/python -m floe index "$SRC" >/dev/null
fi
(cd rust && PATH="$HOME/.cargo/bin:$PATH" \
    cargo build --release 2>/dev/null >/dev/null)
OUT="${SRC%.oas}_rust.ice"
.venv/bin/python tools/validate_rust_scan.py "$SRC"
.venv/bin/python tools/validate_rust_tiles.py "$SRC" "$OUT"
.venv/bin/python tools/validate_rust_depth.py "$SRC" "$OUT"
.venv/bin/python tools/validate_rust_meta.py "$SRC" "$OUT"
.venv/bin/python tools/validate_rust_skel.py "$SRC" "$OUT"
# VFS V1 (rust/VFS.md): build .floe and run the G5/G6 gates
VOUT="${SRC%.oas}_rust.floe"
rm -rf "$VOUT"
rust/target/release/floe-index vfs "$SRC" "$VOUT" >/dev/null 2>&1
.venv/bin/python tools/validate_vfs.py "$SRC" "$VOUT"
.venv/bin/python tools/validate_vfs_render.py "$SRC" "$VOUT"
echo "RUST VALIDATION: ALL OK ($SRC)"
