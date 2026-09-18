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

# ---- gate names and aliases ----------------------------------------
GATES="validation_selector vendor unit unit_vfs unit_render index_cli vfs_profile floe2 rust_scan \
rust_tiles rust_depth rust_meta rust_skel vfs vfs_render vfs_coverage \
occupancy vfs_hier vfs_lifecycle vfs_marker vfs_split vfs_text \
render_goldens render_speckle render_frames drc_ice svrf oasis_shapes \
jobdeck representatives rust_renderer klayout"
WEB_APP_GATES="app_cli cache_migration web_cli_inventory native_revision web_selfcheck web_portable runtime_smoke embedded_host app_render \
layerprops layer_defaults layer_palette palette_styles display_test app_clip managed_clip app_captures \
fe_embed drc_captures view_controller zoom_band minimap depth_keys web_wheel worker_queries \
view_stream managed_index owner_service web_cli web_local_sharing display_cli display_input web_handoff \
web_browse web_file_display web_startup instance_key web_drc web_drc_notes web_drc_waives web_review_budget \
web_autosave web_read_reviewer web_drc_open web_drc_rules web_drc_transfer web_svrf web_ui web_menu_inventory \
web_hangul app_jobdeck app_jobdeck_sources app_jobdeck_plan app_deck_render app_drc drc_review drc_build \
web_drc_build app_svrf svrf_native worker_client"
GATES="$GATES $WEB_APP_GATES"
alias_gates() {
    case "$1" in
        web)      echo "$WEB_APP_GATES" ;;
        quick)    echo "unit_vfs unit_render occupancy rust_renderer" ;;
        planner)  echo "unit_vfs occupancy jobdeck rust_renderer vfs_hier vfs_lifecycle" ;;
        occ)      echo "unit_vfs occupancy" ;;
        render)   echo "unit_render rust_renderer representatives render_goldens render_speckle render_frames klayout" ;;
        indexer)  echo "unit_vfs index_cli rust_scan rust_tiles rust_depth rust_meta rust_skel vfs vfs_render vfs_coverage vfs_split vfs_text vfs_marker representatives" ;;
        python)   echo "index_cli vfs_profile floe2 drc_ice svrf" ;;
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
            echo "available gates:"
            for g in $GATES; do echo "  $g"; done
            echo "aliases:"
            for a in quick planner occ render indexer python deck web; do
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
    if [ -z "$SELECTED" ]; then
        echo "empty gate selection (see --list)"; exit 2
    fi
fi
# gate NAME: whether NAME runs in this invocation
gate() {
    [ -z "$ONLY" ] && return 0
    for g in $SELECTED; do [ "$g" = "$1" ] && return 0; done
    return 1
}
RAN=

# Check checkout completeness before Cargo can hide a missing file behind a
# warm build cache, or fail with a less actionable dependency checksum error.
if gate vendor; then RAN="$RAN vendor"
    .venv/bin/python -B tools/validate_vendor.py; fi

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
echo "== floe2 product + accuracy gates (KLayout = oracle/generator only)"
if gate validation_selector; then RAN="$RAN validation_selector"
    .venv/bin/python -B tools/validate_validation_selector.py; fi
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
if gate app_cli; then RAN="$RAN app_cli"
    .venv/bin/python tools/validate_app_cli.py "$FLOE2_SMOKE_SRC"; fi
if gate cache_migration; then RAN="$RAN cache_migration"
    .venv/bin/python -B tools/validate_cache_migration.py "$FLOE2_SMOKE_SRC"; fi
if gate web_cli_inventory; then RAN="$RAN web_cli_inventory"
    .venv/bin/python -B tools/validate_web_cli_inventory.py; fi
if gate native_revision; then RAN="$RAN native_revision"
    .venv/bin/python -B tools/validate_native_revision.py; fi
if gate web_selfcheck; then RAN="$RAN web_selfcheck"
    .venv/bin/python -B tools/validate_web_selfcheck.py; fi
if gate web_portable; then RAN="$RAN web_portable"
    .venv/bin/python -B tools/validate_web_portable.py; fi
if gate runtime_smoke; then RAN="$RAN runtime_smoke"
    .venv/bin/python -B tools/validate_runtime_smoke.py; fi
if gate embedded_host; then RAN="$RAN embedded_host"
    .venv/bin/python -B tools/validate_desktop_env.py
    .venv/bin/python -B tools/validate_desktop_vendor.py
    (cd rust && FLOE_INDEX_BIN="$PWD/target/release/floe-index" \
        FLOE_RENDERD_BIN="$PWD/target/release/floe-renderd" \
        cargo test --release --offline --locked -p floe-app --test embedded_lifecycle -- --ignored); fi
if gate app_render; then RAN="$RAN app_render"
    .venv/bin/python tools/validate_app_render.py "$FLOE2_SMOKE_SRC"; fi
if gate layerprops; then RAN="$RAN layerprops"
    .venv/bin/python -B tools/validate_layerprops.py "$FLOE2_SMOKE_SRC"; fi
if gate layer_defaults; then RAN="$RAN layer_defaults"
    .venv/bin/python -B tools/validate_layer_defaults.py "$FLOE2_SMOKE_SRC"; fi
if gate layer_palette; then RAN="$RAN layer_palette"
    .venv/bin/python -B tools/validate_layer_palette.py; fi
if gate palette_styles; then RAN="$RAN palette_styles"
    .venv/bin/python -B tools/validate_palette_styles.py; fi
if gate display_test; then RAN="$RAN display_test"
    .venv/bin/python -B tools/validate_display_test.py; fi
if gate app_clip; then RAN="$RAN app_clip"
    .venv/bin/python -B tools/validate_app_clip.py "$FLOE2_SMOKE_SRC"; fi
if gate managed_clip; then RAN="$RAN managed_clip"
    .venv/bin/python -B tools/validate_managed_clip.py; fi
if gate app_captures; then RAN="$RAN app_captures"
    .venv/bin/python -B tools/validate_app_captures.py "$FLOE2_SMOKE_SRC"; fi
if gate fe_embed; then RAN="$RAN fe_embed"
    .venv/bin/python -B tools/validate_fe_embed.py; fi
if gate drc_captures; then RAN="$RAN drc_captures"
    .venv/bin/python -B tools/validate_drc_captures.py "$FLOE2_SMOKE_SRC"; fi
if gate view_controller; then RAN="$RAN view_controller"
    .venv/bin/python tools/validate_view_controller.py "$FLOE2_SMOKE_SRC"; fi
if gate zoom_band; then RAN="$RAN zoom_band"
    .venv/bin/python tools/validate_zoom_band.py; fi
if gate minimap; then RAN="$RAN minimap"
    .venv/bin/python tools/validate_minimap.py; fi
if gate depth_keys; then RAN="$RAN depth_keys"
    .venv/bin/python tools/validate_depth_keys.py; fi
if gate web_wheel; then RAN="$RAN web_wheel"
    .venv/bin/python -B tools/validate_web_wheel.py; fi
if gate worker_queries; then RAN="$RAN worker_queries"
    .venv/bin/python -B tools/validate_worker_queries.py; fi
if gate view_stream; then RAN="$RAN view_stream"
    .venv/bin/python tools/validate_view_stream.py "$FLOE2_SMOKE_SRC"; fi
if gate managed_index; then RAN="$RAN managed_index"
    .venv/bin/python tools/validate_managed_index.py "$FLOE2_SMOKE_SRC"; fi
if gate owner_service; then RAN="$RAN owner_service"
    .venv/bin/python tools/validate_owner_service.py "$FLOE2_SMOKE_SRC"; fi
if gate web_cli; then RAN="$RAN web_cli"
    .venv/bin/python tools/validate_web_cli.py "$FLOE2_SMOKE_SRC"; fi
if gate web_local_sharing; then RAN="$RAN web_local_sharing"
    .venv/bin/python -B tools/validate_web_local_sharing.py "$FLOE2_SMOKE_SRC"; fi
if gate display_cli; then RAN="$RAN display_cli"
    .venv/bin/python -B tools/validate_display_cli.py; fi
if gate display_input; then RAN="$RAN display_input"
    .venv/bin/python -B tools/validate_display_input.py; fi
if gate web_handoff; then RAN="$RAN web_handoff"
    .venv/bin/python -B tools/validate_web_handoff.py "$FLOE2_SMOKE_SRC"; fi
if gate web_browse; then RAN="$RAN web_browse"
    .venv/bin/python -B tools/validate_web_browse.py "$FLOE2_SMOKE_SRC"; fi
if gate web_file_display; then RAN="$RAN web_file_display"
    .venv/bin/python -B tools/validate_web_file_display.py; fi
if gate web_startup; then RAN="$RAN web_startup"
    .venv/bin/python -B tools/validate_web_startup_timing.py
    .venv/bin/python -B tools/validate_web_startup.py "$FLOE2_SMOKE_SRC"; fi
if gate instance_key; then RAN="$RAN instance_key"
    .venv/bin/python -B tools/validate_instance_key.py; fi
if gate web_drc; then RAN="$RAN web_drc"
    .venv/bin/python -B tools/validate_web_drc.py "$FLOE2_SMOKE_SRC"; fi
if gate web_drc_notes; then RAN="$RAN web_drc_notes"
    .venv/bin/python -B tools/validate_web_drc_notes.py "$FLOE2_SMOKE_SRC"; fi
if gate web_drc_waives; then RAN="$RAN web_drc_waives"
    .venv/bin/python -B tools/validate_web_drc_waives.py "$FLOE2_SMOKE_SRC"; fi
if gate web_review_budget; then RAN="$RAN web_review_budget"
    .venv/bin/python -B tools/validate_web_review_budget.py "$FLOE2_SMOKE_SRC"; fi
if gate web_autosave; then RAN="$RAN web_autosave"
    .venv/bin/python -B tools/validate_web_autosave.py "$FLOE2_SMOKE_SRC"; fi
if gate web_read_reviewer; then RAN="$RAN web_read_reviewer"
    .venv/bin/python -B tools/validate_web_read_reviewer.py "$FLOE2_SMOKE_SRC"; fi
if gate web_drc_open; then RAN="$RAN web_drc_open"
    .venv/bin/python -B tools/validate_web_drc_open.py "$FLOE2_SMOKE_SRC"; fi
if gate web_drc_rules; then RAN="$RAN web_drc_rules"
    .venv/bin/python -B tools/validate_web_drc_rules.py "$FLOE2_SMOKE_SRC"; fi
if gate web_drc_transfer; then RAN="$RAN web_drc_transfer"
    .venv/bin/python -B tools/validate_web_drc_transfer.py "$FLOE2_SMOKE_SRC"; fi
if gate web_svrf; then RAN="$RAN web_svrf"
    .venv/bin/python -B tools/validate_web_svrf.py "$FLOE2_SMOKE_SRC"; fi
if gate web_ui; then RAN="$RAN web_ui"
    node tools/validate_web_ui.cjs; fi
if gate web_menu_inventory; then RAN="$RAN web_menu_inventory"
    .venv/bin/python -B tools/validate_web_menu_inventory.py --require-complete; fi
if gate web_hangul; then RAN="$RAN web_hangul"
    .venv/bin/python -B tools/validate_web_hangul.py; fi
if gate app_jobdeck; then RAN="$RAN app_jobdeck"
    .venv/bin/python -B tools/validate_app_jobdeck.py; fi
if gate app_jobdeck_sources; then RAN="$RAN app_jobdeck_sources"
    .venv/bin/python -B tools/validate_app_jobdeck_sources.py; fi
if gate app_jobdeck_plan; then RAN="$RAN app_jobdeck_plan"
    .venv/bin/python -B tools/validate_app_jobdeck_plan.py; fi
if gate app_deck_render; then RAN="$RAN app_deck_render"
    .venv/bin/python -B tools/validate_app_deck_render.py; fi
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
for g in vfs vfs_render vfs_coverage vfs_hier vfs_lifecycle rust_renderer worker_client; do
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
if gate worker_client; then RAN="$RAN worker_client"
# Rust web migration M1a: same native frames through the Rust client and the
# existing Python adapter, including raw/PNG, labels, styles and cancel soak.
# A milestone $SRC can be enormous (or only one page). This process-lifecycle
# gate must always use the small multi-page valmini, like validate_floe2 above.
if [ "$SRC" = "$FLOE2_SMOKE_SRC" ]; then
    sh tools/validate_worker_client.sh "$FLOE2_SMOKE_SRC" "$VOUT"
else
    (
        worker_gate_dir=$(mktemp -d "${TMPDIR:-/tmp}/floe-worker-gate.XXXXXX")
        trap 'rm -rf "$worker_gate_dir"' EXIT HUP INT TERM
        rust/target/release/floe-index vfs "$FLOE2_SMOKE_SRC" \
            "$worker_gate_dir/valmini.floe" --jobs 2 >/dev/null
        sh tools/validate_worker_client.sh "$FLOE2_SMOKE_SRC" \
            "$worker_gate_dir/valmini.floe"
    )
fi
fi
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
if gate app_drc; then RAN="$RAN app_drc"
    .venv/bin/python -B tools/validate_app_drc.py; fi
if gate drc_review; then RAN="$RAN drc_review"
    .venv/bin/python -B tools/validate_drc_review.py; fi
if gate drc_build; then RAN="$RAN drc_build"
    .venv/bin/python -B tools/validate_drc_build.py; fi
if gate web_drc_build; then RAN="$RAN web_drc_build"
    .venv/bin/python -B tools/validate_web_drc_build.py "$FLOE2_SMOKE_SRC"; fi
if gate app_svrf; then RAN="$RAN app_svrf"
    .venv/bin/python -B tools/validate_app_svrf.py; fi
if gate svrf_native; then RAN="$RAN svrf_native"
    .venv/bin/python -B tools/validate_svrf_native.py; fi
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
