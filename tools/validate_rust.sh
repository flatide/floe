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
#
# KLayout is a DEVELOPMENT dependency of this battery only: it generates
# the fixtures, builds the legacy .tiles meta-parity oracle and serves as
# the pixel/query accuracy oracle. The shipped product (floe2) needs none
# of it - validate_floe2.py pins the KLayout-free import and the product
# shell; the frozen KLayout shell floe is exercised here purely as the
# oracle (floe-legacy, 2026-09-08).
set -e
cd "$(dirname "$0")/.."
SRC=${1:-}
FLOE2_SMOKE_SRC=${TMPDIR:-/tmp}/floe-valmini/valmini.oas
mkdir -p "$(dirname "$FLOE2_SMOKE_SRC")"
# The floe2 lifecycle gate always uses the small deterministic fixture. A
# caller may pass a 100+GB milestone source as $SRC; copying and re-indexing it
# merely to test the product shell would be both destructive to time and RAM.
if [ ! -f "$FLOE2_SMOKE_SRC" ] || \
   [ tools/gen_valmini.py -nt "$FLOE2_SMOKE_SRC" ]; then
    # a regenerated source invalidates EVERY derived artefact: the
    # legacy .tiles, both .floe caches (one may be a symlink to the
    # other on a long-lived host), the .ice sidecars. Leaving a stale
    # .floe behind made `floe index --legacy` refuse the .tiles rebuild
    # and `floe2 index` refuse the stale cache (2026-09-09, after the
    # temp fixture vanished from a long-lived $TMPDIR)
    rm -rf "$FLOE2_SMOKE_SRC" "$FLOE2_SMOKE_SRC.tiles" \
        "${FLOE2_SMOKE_SRC%.oas}_rust.tiles" \
        "$FLOE2_SMOKE_SRC.floe" "${FLOE2_SMOKE_SRC%.oas}_rust.floe" \
        "$FLOE2_SMOKE_SRC.ice" "${FLOE2_SMOKE_SRC%.oas}_rust.ice"
    .venv/bin/python tools/gen_valmini.py "$FLOE2_SMOKE_SRC"
fi
if [ -z "$SRC" ]; then
    SRC=$FLOE2_SMOKE_SRC
fi
if [ "$SRC" = "$FLOE2_SMOKE_SRC" ]; then
    # the python .tiles is the meta-parity oracle: refresh it when the
    # python indexer itself changed, not only when the asset did (the
    # layer-palette change tripped this once - stale colors failed
    # validate_rust_meta on every host with an old cached .tiles)
    if [ ! -f "$SRC.tiles/meta.json" ] || \
       [ floe/cache.py -nt "$SRC.tiles/meta.json" ]; then
        rm -rf "$SRC.tiles"
        # the legacy indexer refuses to build .tiles beside a .floe of
        # the same source: step the VFS cache aside for the build
        if [ -e "$SRC.floe" ]; then mv "$SRC.floe" "$SRC.floe.aside"; fi
        PYTHONPATH=. .venv/bin/python -m floe index --legacy "$SRC" \
            >/dev/null || {
            [ -e "$SRC.floe.aside" ] && mv "$SRC.floe.aside" "$SRC.floe"
            echo "FAIL: legacy .tiles oracle build"; exit 1; }
        if [ -e "$SRC.floe.aside" ]; then mv "$SRC.floe.aside" "$SRC.floe"; fi
    fi
fi
(cd rust && PATH="$HOME/.cargo/bin:$PATH" \
    cargo build --release 2>/dev/null >/dev/null)
echo "== floe2 product + accuracy gates (KLayout = oracle/generator only)"
(cd rust && PATH="$HOME/.cargo/bin:$PATH" cargo test --workspace)
.venv/bin/python tools/validate_index_cli.py
.venv/bin/python tools/validate_vfs_profile.py "$FLOE2_SMOKE_SRC"
.venv/bin/python tools/validate_floe2.py "$FLOE2_SMOKE_SRC"
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
# occupancy pyramid (design.ovo): every level-0 bit vs KLayout's shape
# intersection on fixtures + the asset at a coarse cell; CLI contract
.venv/bin/python tools/validate_occupancy.py "$SRC"
.venv/bin/python tools/validate_vfs_hier.py "$SRC" "$VOUT"
.venv/bin/python tools/validate_vfs_lifecycle.py "$SRC" "$VOUT"
.venv/bin/python tools/validate_vfs_marker.py "$SRC"
# rep-split page honesty (ovm v3): floor collapse + fragment
# conservation on a synthetic rep-flood asset
.venv/bin/python tools/validate_vfs_split.py
# v5 text index: oracle XOR, declutter, corrupt, determinism,
# daemon label lifecycle
.venv/bin/python tools/validate_vfs_text.py
# rasterization goldens for the rust-renderer replacement:
# bake (version-pinned) + determinism + policy discrimination
.venv/bin/python tools/validate_render_goldens.py
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
# OASIS TRAPEZOID / CTRAPEZOID (mask data): every record type through
# floe-index + clip must equal KLayout's reading of the same bytes
.venv/bin/python tools/validate_oasis_shapes.py
# Calibre MDPView jobdeck (.jb): parser / hand-computed placements /
# MDPView colour order / header probe / CLI / batch index
.venv/bin/python tools/validate_jobdeck.py
# in-tree CPU renderer: Python queue contract plus independent
# KLayout pixel oracle at deterministic serial/parallel settings
PYTHONDONTWRITEBYTECODE=1 FLOE_RENDERER=rust FLOE_RUST_ROUND_PAGES=4 \
    FLOE_INTEGRATION_SOURCE="$SRC" FLOE_INTEGRATION_CACHE="$VOUT" \
    .venv/bin/python tools/validate_rust_renderer.py
echo "== KLayout pixel oracle (frozen shell as reference)"
PYTHONDONTWRITEBYTECODE=1 .venv/bin/python tools/validate_klayout_oracle.py --jobs 1
PYTHONDONTWRITEBYTECODE=1 .venv/bin/python tools/validate_klayout_oracle.py --jobs 8
echo "RUST VALIDATION: ALL OK ($SRC)"
