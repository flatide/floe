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
#   sh tools/validate_rust.sh --only occupancy,jobdeck   # a selection
#   sh tools/validate_rust.sh --only planner # an alias (see --list)
#   sh tools/validate_rust.sh --list         # gate names and aliases
#
# --only runs the named gates (comma-separated; names or aliases,
# 2026-09-17) after the same setup the whole battery uses, and the
# setup is REUSED when nothing it depends on changed: the release
# build is cargo's own freshness check, the legacy .tiles oracle is
# rebuilt only for the gates that read it and only when the python
# indexer changed, and the VFS cache is rebuilt only when floe-index
# or the source is newer than it. The whole battery (no --only) still
# rebuilds the VFS cache every time, so a cache-format change can
# never hide behind a stale one. The last line is the same
# "RUST VALIDATION: ALL OK" either way, naming the gates that ran.
#
# KLayout is a DEVELOPMENT dependency of this battery only: it generates
# the fixtures, builds the legacy .tiles meta-parity oracle and serves as
# the pixel/query accuracy oracle. The shipped product (floe2) needs none
# of it - validate_floe2.py pins the KLayout-free import and the product
# shell; the frozen KLayout shell floe is exercised here purely as the
# oracle (floe-legacy, 2026-09-08).
set -e
cd "$(dirname "$0")/.."

# ---- gate names (in battery order) and aliases ----------------------
GATES="unit unit_vfs unit_render index_cli vfs_profile floe2 rust_scan \
rust_tiles rust_depth rust_meta rust_skel vfs vfs_render vfs_coverage \
occupancy vfs_hier vfs_lifecycle vfs_marker vfs_split vfs_text \
render_goldens render_speckle render_frames drc_ice svrf oasis_shapes \
jobdeck representatives gen_main01 fit_budget sub_cut_box write_once rust_renderer klayout"
alias_gates() {
    case "$1" in
        quick)    echo "unit_vfs unit_render occupancy rust_renderer" ;;
        planner)  echo "unit_vfs occupancy jobdeck rust_renderer vfs_hier vfs_lifecycle fit_budget sub_cut_box" ;;
        occ)      echo "unit_vfs occupancy" ;;
        render)   echo "unit_render rust_renderer representatives fit_budget sub_cut_box write_once render_goldens render_speckle render_frames klayout" ;;
        indexer)  echo "unit_vfs index_cli rust_scan rust_tiles rust_depth rust_meta rust_skel vfs vfs_render vfs_coverage vfs_split vfs_text vfs_marker representatives" ;;
        python)   echo "index_cli vfs_profile floe2 drc_ice svrf gen_main01" ;;
        deck)     echo "jobdeck occupancy" ;;
        *)        echo "" ;;
    esac
}

SRC=
ONLY=
while [ $# -gt 0 ]; do
    case "$1" in
        --only)
            [ $# -ge 2 ] || { echo "usage: --only a,b,c"; exit 2; }
            ONLY="$ONLY,$2"; shift 2 ;;
        --only=*)
            ONLY="$ONLY,${1#--only=}"; shift ;;
        --list)
            echo "gates (battery order):"
            for g in $GATES; do echo "  $g"; done
            echo "aliases:"
            for a in quick planner occ render indexer python deck; do
                echo "  $a = $(alias_gates $a)"
            done
            exit 0 ;;
        -h|--help)
            sed -n 2,26p "$0"; exit 0 ;;
        --*)
            echo "unknown option $1 (see --list)"; exit 2 ;;
        *)
            SRC=$1; shift ;;
    esac
done

# the selection: every gate, or the names / aliases behind --only
SELECTED=
if [ -n "$ONLY" ]; then
    for name in $(echo "$ONLY" | tr ',' ' '); do
        [ -n "$name" ] || continue
        expanded=$(alias_gates "$name")
        if [ -n "$expanded" ]; then
            SELECTED="$SELECTED $expanded"
            continue
        fi
        known=0
        for g in $GATES; do [ "$g" = "$name" ] && known=1; done
        if [ $known = 0 ]; then
            echo "unknown gate '$name' (see --list)"; exit 2
        fi
        SELECTED="$SELECTED $name"
    done
fi
# gate NAME: whether NAME runs in this invocation
gate() {
    [ -z "$ONLY" ] && return 0
    for g in $SELECTED; do [ "$g" = "$1" ] && return 0; done
    return 1
}
RAN=

FLOE2_SMOKE_SRC=${TMPDIR:-/tmp}/floe-valmini/valmini.oas
mkdir -p "$(dirname "$FLOE2_SMOKE_SRC")"
# The floe2 lifecycle gate always uses the small deterministic fixture. A
# caller may pass a 100+GB milestone source as $SRC; copying and re-indexing it
# merely to test the product shell would be both destructive to time and RAM.
if [ ! -f "$FLOE2_SMOKE_SRC" ] || \
   [ tools/gen_valmini.py -nt "$FLOE2_SMOKE_SRC" ]; then
    # a regenerated source invalidates EVERY derived artefact: the
    # legacy .tiles, the VFS caches (.<src>.ice since 2026-09-16,
    # <src>.floe before; one may be a symlink to the other on a
    # long-lived host), the DRC packs. Leaving a stale cache behind
    # made `floe index --legacy` refuse the .tiles rebuild and `floe2
    # index` refuse the stale cache (2026-09-09, after the temp fixture
    # vanished from a long-lived $TMPDIR)
    SMOKE_DIR=$(dirname "$FLOE2_SMOKE_SRC")
    SMOKE_BASE=$(basename "$FLOE2_SMOKE_SRC")
    rm -rf "$FLOE2_SMOKE_SRC" "$FLOE2_SMOKE_SRC.tiles" \
        "${FLOE2_SMOKE_SRC%.oas}_rust.tiles" \
        "$SMOKE_DIR/.$SMOKE_BASE.ice" "$SMOKE_DIR/.$SMOKE_BASE.tray" \
        "$FLOE2_SMOKE_SRC.floe" "${FLOE2_SMOKE_SRC%.oas}_rust.floe" \
        "$FLOE2_SMOKE_SRC.ice" "${FLOE2_SMOKE_SRC%.oas}_rust.ice"
    .venv/bin/python tools/gen_valmini.py "$FLOE2_SMOKE_SRC"
fi
if [ -z "$SRC" ]; then
    SRC=$FLOE2_SMOKE_SRC
fi
# the legacy .tiles oracle serves the rust_* meta-parity gates only
NEEDS_TILES=0
for g in rust_scan rust_tiles rust_depth rust_meta rust_skel; do
    gate $g && NEEDS_TILES=1
done
if [ "$SRC" = "$FLOE2_SMOKE_SRC" ] && [ $NEEDS_TILES = 1 ]; then
    # the python .tiles is the meta-parity oracle: refresh it when the
    # python indexer itself changed, not only when the asset did (the
    # layer-palette change tripped this once - stale colors failed
    # validate_rust_meta on every host with an old cached .tiles)
    if [ ! -f "$SRC.tiles/meta.json" ] || \
       [ floe/cache.py -nt "$SRC.tiles/meta.json" ]; then
        rm -rf "$SRC.tiles"
        # the legacy indexer refuses to build .tiles beside a VFS cache
        # of the same source (.<src>.ice since 2026-09-16, <src>.floe
        # before): step both aside for the build
        VFSC="$(dirname "$SRC")/.$(basename "$SRC").ice"
        for c in "$VFSC" "$SRC.floe"; do
            if [ -e "$c" ]; then mv "$c" "$c.aside"; fi
        done
        PYTHONPATH=. .venv/bin/python -m floe index --legacy "$SRC" \
            >/dev/null || {
            for c in "$VFSC" "$SRC.floe"; do
                if [ -e "$c.aside" ]; then mv "$c.aside" "$c"; fi
            done
            echo "FAIL: legacy .tiles oracle build"; exit 1; }
        for c in "$VFSC" "$SRC.floe"; do
            if [ -e "$c.aside" ]; then mv "$c.aside" "$c"; fi
        done
    fi
fi
# the release build: cargo's own freshness check makes this free when
# nothing changed
(cd rust && PATH="$HOME/.cargo/bin:$PATH" \
    cargo build --release 2>/dev/null >/dev/null)
INDEX_BIN=rust/target/release/floe-index
# warm the freshly built renderer once: macOS scans a new executable on
# its first launch (seconds; a launch killed mid-scan stays cold), which
# made the first gate to start a worker time out (occupancy's deck
# worker, 2026-09-18)
if [ -x rust/target/release/floe-renderd ]; then
    echo "" | rust/target/release/floe-renderd >/dev/null 2>&1 || true
fi
echo "== floe2 product + accuracy gates (KLayout = oracle/generator only)"
if gate unit; then
    RAN="$RAN unit"
    (cd rust && PATH="$HOME/.cargo/bin:$PATH" cargo test --workspace)
fi
if gate unit_vfs && ! gate unit; then
    RAN="$RAN unit_vfs"
    (cd rust && PATH="$HOME/.cargo/bin:$PATH" cargo test --release --lib -p floe-vfs -p floe-ovm)
fi
if gate unit_render && ! gate unit; then
    RAN="$RAN unit_render"
    (cd rust && PATH="$HOME/.cargo/bin:$PATH" cargo test --release --lib -p floe-render-core)
fi
if gate index_cli; then RAN="$RAN index_cli"
    .venv/bin/python tools/validate_index_cli.py; fi
if gate vfs_profile; then RAN="$RAN vfs_profile"
    .venv/bin/python tools/validate_vfs_profile.py "$FLOE2_SMOKE_SRC"; fi
if gate floe2; then RAN="$RAN floe2"
    .venv/bin/python tools/validate_floe2.py "$FLOE2_SMOKE_SRC"; fi
OUT="${SRC%.oas}_rust.tiles"
if gate rust_scan; then RAN="$RAN rust_scan"
    .venv/bin/python tools/validate_rust_scan.py "$SRC"; fi
if gate rust_tiles; then RAN="$RAN rust_tiles"
    .venv/bin/python tools/validate_rust_tiles.py "$SRC" "$OUT"; fi
if gate rust_depth; then RAN="$RAN rust_depth"
    .venv/bin/python tools/validate_rust_depth.py "$SRC" "$OUT"; fi
if gate rust_meta; then RAN="$RAN rust_meta"
    .venv/bin/python tools/validate_rust_meta.py "$SRC" "$OUT"; fi
if gate rust_skel; then RAN="$RAN rust_skel"
    .venv/bin/python tools/validate_rust_skel.py "$SRC" "$OUT"; fi
# VFS V1 (rust/VFS.md): build a VFS cache (an explicit outdir, not the
# hidden default) and run the G5/G6 gates. The cache is rebuilt every
# time by the whole battery; a --only run reuses one that is newer
# than floe-index and the source.
VOUT="${SRC%.oas}_rust.ice"
NEEDS_VFS=0
for g in vfs vfs_render vfs_coverage vfs_hier vfs_lifecycle rust_renderer; do
    gate $g && NEEDS_VFS=1
done
if [ $NEEDS_VFS = 1 ]; then
    REBUILD=1
    if [ -n "$ONLY" ] && [ -f "$VOUT/meta.json" ] && \
       [ ! "$INDEX_BIN" -nt "$VOUT/meta.json" ] && \
       [ ! "$SRC" -nt "$VOUT/meta.json" ]; then
        REBUILD=0
        echo "== VFS cache reused: $VOUT"
    fi
    if [ $REBUILD = 1 ]; then
        rm -rf "$VOUT"
        "$INDEX_BIN" vfs "$SRC" "$VOUT" \
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
    fi
fi
if gate vfs; then RAN="$RAN vfs"
    .venv/bin/python tools/validate_vfs.py "$SRC" "$VOUT"; fi
if gate vfs_render; then RAN="$RAN vfs_render"
    .venv/bin/python tools/validate_vfs_render.py "$SRC" "$VOUT"; fi
if gate vfs_coverage; then RAN="$RAN vfs_coverage"
    .venv/bin/python tools/validate_vfs_coverage.py "$SRC" "$VOUT"; fi
# occupancy pyramid (design.ovo): every level-0 bit vs KLayout's shape
# intersection on fixtures + the asset at a coarse cell; CLI contract
if gate occupancy; then RAN="$RAN occupancy"
    .venv/bin/python tools/validate_occupancy.py "$SRC"; fi
if gate vfs_hier; then RAN="$RAN vfs_hier"
    .venv/bin/python tools/validate_vfs_hier.py "$SRC" "$VOUT"; fi
if gate vfs_lifecycle; then RAN="$RAN vfs_lifecycle"
    .venv/bin/python tools/validate_vfs_lifecycle.py "$SRC" "$VOUT"; fi
if gate vfs_marker; then RAN="$RAN vfs_marker"
    .venv/bin/python tools/validate_vfs_marker.py "$SRC"; fi
# rep-split page honesty (ovm v3): floor collapse + fragment
# conservation on a synthetic rep-flood asset
if gate vfs_split; then RAN="$RAN vfs_split"
    .venv/bin/python tools/validate_vfs_split.py; fi
# v5 text index: oracle XOR, declutter, corrupt, determinism,
# daemon label lifecycle
if gate vfs_text; then RAN="$RAN vfs_text"
    .venv/bin/python tools/validate_vfs_text.py; fi
# rasterization goldens for the rust-renderer replacement:
# bake (version-pinned) + determinism + policy discrimination
if gate render_goldens; then RAN="$RAN render_goldens"
    .venv/bin/python tools/validate_render_goldens.py; fi
# viewer speckle fill: common phase, opaque overlap, and the
# coverage composite staying out of speckled interiors
if gate render_speckle; then RAN="$RAN render_speckle"
    .venv/bin/python tools/validate_render_speckle.py; fi
# frame outline stacking: white over gray, 1px hollow, under design
if gate render_frames; then RAN="$RAN render_frames"
    .venv/bin/python tools/validate_render_frames.py; fi
# DRC pack (.<db>.tray): reading through the pack == ASCII parse
if gate drc_ice; then RAN="$RAN drc_ice"
    .venv/bin/python tools/validate_drc_ice.py; fi
# SVRF subset parser: preprocessing / derivation closure / check
# extraction / end-to-end vs gen_drcdb --svrf
if gate svrf; then RAN="$RAN svrf"
    .venv/bin/python tools/validate_svrf.py; fi
# OASIS TRAPEZOID / CTRAPEZOID (mask data): every record type through
# floe-index + clip must equal KLayout's reading of the same bytes
if gate oasis_shapes; then RAN="$RAN oasis_shapes"
    .venv/bin/python tools/validate_oasis_shapes.py; fi
# Calibre MDPView jobdeck (.jb): parser / hand-computed placements /
# MDPView colour order / header probe / CLI / batch index
if gate jobdeck; then RAN="$RAN jobdeck"
    .venv/bin/python tools/validate_jobdeck.py; fi
# design.ovr (OVR1) end to end: additive build preserves the cache,
# depth/kill switch/invalid fallback, and a failed OVR inside a
# combined index run leaves a complete base cache behind
if gate representatives; then RAN="$RAN representatives"
    .venv/bin/python tools/validate_representatives.py; fi
# the synthetic MAIN01 generator: legacy geometry byte-identical, chip
# geometry deterministic, KLayout-readable, indexable and chip-shaped
if gate gen_main01; then RAN="$RAN gen_main01"
    .venv/bin/python tools/validate_gen_main01.py; fi
# budget-fitted cut: a keep view over the generation budget is drawn at a
# lowered density instead of failing; frames that fit are untouched
if gate fit_budget; then RAN="$RAN fit_budget"
    .venv/bin/python tools/validate_fit_budget.py; fi
# sub-cut boxes: under thin keep with few layers visible, what the size cut
# drops stays as a box from index metadata; everything else unchanged
if gate sub_cut_box; then RAN="$RAN sub_cut_box"
    .venv/bin/python tools/validate_sub_cut_box.py; fi
# write-once tiles: planes painted in reverse, each pixel written once,
# covered work skipped - frames byte-identical to the ordered overwrite
if gate write_once; then RAN="$RAN write_once"
    .venv/bin/python tools/validate_write_once.py; fi
# in-tree CPU renderer: Python queue contract plus independent
# KLayout pixel oracle at deterministic serial/parallel settings
if gate rust_renderer; then RAN="$RAN rust_renderer"
    PYTHONDONTWRITEBYTECODE=1 FLOE_RENDERER=rust FLOE_RUST_ROUND_PAGES=4 \
        FLOE_INTEGRATION_SOURCE="$SRC" FLOE_INTEGRATION_CACHE="$VOUT" \
        .venv/bin/python tools/validate_rust_renderer.py; fi
if gate klayout; then RAN="$RAN klayout"
    echo "== KLayout pixel oracle (frozen shell as reference)"
    PYTHONDONTWRITEBYTECODE=1 .venv/bin/python tools/validate_klayout_oracle.py --jobs 1
    PYTHONDONTWRITEBYTECODE=1 .venv/bin/python tools/validate_klayout_oracle.py --jobs 8
fi
if [ -n "$ONLY" ]; then
    echo "RUST VALIDATION: ALL OK ($SRC; gates:$RAN)"
else
    echo "RUST VALIDATION: ALL OK ($SRC)"
fi
