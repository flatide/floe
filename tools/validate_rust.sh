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
#   sh tools/validate_rust.sh --changed      # the gates the changed files
#                                            # call for (see below)
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
# --changed (2026-10-06; user: "waiting for the whole battery every time
# is inefficient - run the checks that are related") is --only with the
# selection made from the files that differ from HEAD (the work tree,
# the index and untracked files; --changed=REF: from REF, for commits
# not yet pushed): each path names its gates (gates_for below), a path
# that everything hangs on - the parser, the index format, the indexer,
# this script - or one the table does not know calls for the whole
# battery, and documents call for none. --dry-run says which gates
# would run and stops; --files=a,b,c asks the same of the paths named
# instead of the changed ones. Every run ends with the seconds each
# gate took.
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
jobdeck representatives gen_main01 fit_budget sub_cut_box shape_cut write_once layer_decode area_true density_stack cell_tree index_lock worker_client cell_index rust_renderer klayout"
# (unit_renderd is unit's renderd part, as unit_vfs and unit_render are)
GATES=$(echo "$GATES" | sed 's/unit_render /unit_render unit_renderd /')
alias_gates() {
    case "$1" in
        quick)    echo "unit_vfs unit_render occupancy rust_renderer" ;;
        planner)  echo "unit_vfs occupancy jobdeck rust_renderer vfs_hier vfs_lifecycle fit_budget sub_cut_box shape_cut" ;;
        occ)      echo "unit_vfs occupancy" ;;
        render)   echo "unit_render unit_renderd rust_renderer representatives fit_budget sub_cut_box shape_cut write_once layer_decode area_true density_stack cell_tree render_goldens render_speckle render_frames klayout" ;;
        indexer)  echo "unit_vfs index_cli rust_scan rust_tiles rust_depth rust_meta rust_skel vfs vfs_render vfs_coverage vfs_split vfs_text vfs_marker representatives cell_tree index_lock" ;;
        python)   echo "index_cli vfs_profile floe2 drc_ice svrf gen_main01" ;;
        deck)     echo "jobdeck occupancy" ;;
        *)        echo "" ;;
    esac
}

# ---- --changed: the gates a changed path calls for -------------------
# What draws through renderd: its unit tests, the adapter and viewer
# contract, the deck gate (it holds renderer contracts of its own - the
# page budget's refusal at cut 0 among them) and the picture gates.
RENDER_GATES="unit_render unit_renderd rust_renderer jobdeck occupancy \
fit_budget sub_cut_box shape_cut write_once layer_decode area_true \
density_stack cell_tree index_lock representatives oasis_shapes floe2 klayout"
# The planner feeds every one of those, and the plan CLI's gates.
PLAN_GATES="unit_vfs $RENDER_GATES vfs_hier vfs_lifecycle vfs_marker \
vfs_split vfs_text vfs_profile"
# only version lines differ in FILE (every push bumps them)
versions_only() {
    git diff "$CHANGED_BASE" -- "$1" 2>/dev/null | grep '^[+-]' | \
        grep -v '^+++ \|^--- ' | \
        grep -qv '^[+-]version = "\|^[+-]__version__ = "\|^[+-]RENDERD_VERSION = "' \
        && return 1
    git ls-files --error-unmatch "$1" >/dev/null 2>&1
}
# gates_for PATH: gate names and aliases, ALL (the whole battery), or
# nothing. The first pattern that matches decides; a path no pattern
# knows is ALL.
gates_for() {
    case "$1" in
        docs/*|*.md|.gitignore|.venv|.venv/*|.claude/*|data/*|jobdecks_klayout/*)
            echo "" ;;
        tools/validate_rust.sh|tools/gen_valmini.py)
            echo ALL ;;
        tools/validate_klayout_oracle.py)
            echo klayout ;;
        tools/validate_*.py)
            gate_name=${1#tools/validate_}; gate_name=${gate_name%.py}
            for g in $GATES; do
                if [ "$g" = "$gate_name" ]; then echo "$g"; return; fi
            done
            echo "" ;;
        tools/gen_main01_like.py)
            echo "gen_main01 fit_budget" ;;
        tools/gen_mdpview_samples.py|tools/jobdeck_expected.json)
            echo jobdeck ;;
        tools/gen_drcdb.py|tools/gen_drc_db.py)
            echo "drc_ice svrf" ;;
        tools/*)
            # benches, experiments and generators no gate runs
            echo "" ;;
        rust/build-linux.sh)
            echo "" ;;
        rust/Cargo.lock|rust/*/Cargo.toml|floe/__init__.py)
            if versions_only "$1"; then echo "rust_renderer floe2 index_cli"
            else echo ALL; fi ;;
        rust/vfs/src/hier.rs|rust/vfs/src/cover.rs)
            echo "$PLAN_GATES" ;;
        rust/vfs/src/hiersum.rs)
            echo "unit_vfs cell_tree fit_budget rust_renderer" ;;
        rust/vfs/src/occupancy.rs)
            echo "unit_vfs occupancy jobdeck density_stack rust_renderer" ;;
        rust/vfs/src/representatives*)
            echo "unit_vfs representatives rust_renderer" ;;
        rust/render-core/src/deck.rs)
            echo "unit_render jobdeck occupancy rust_renderer" ;;
        rust/render-core/src/cells.rs)
            echo "unit_render cell_tree fit_budget rust_renderer" ;;
        rust/worker-client/*)
            echo "unit worker_client" ;;
        rust/app-core/*|rust/notices/*)
            echo "unit cell_index floe2" ;;
        rust/app-cli/*|rust/floe2/*|floe/gtkview.py)
            # the Rust command line floe2 (P1, docs/SHARED_APP_LAYER.ko.md)
            echo "unit floe2" ;;
        rust/render-core/*|rust/renderd/*|rust/render-cli/*|floe/rust_render.py)
            echo "$RENDER_GATES" ;;
        rust/dbg/*)
            echo unit ;;
        rust/*)
            # the parser, the index format, the tiler, the rest of the
            # VFS, floe-index, the vendored crates, the workspace
            echo ALL ;;
        floe/cache.py|floe/cachepath.py)
            echo ALL ;;
        floe/gui.py|floe/view_policy.py|floe/service.py|floe/viewport.py|floe/hangul.py|floe/fillpat.py|floe/coverage.py|floe/instance.py|floe/shots.py|floe/fe_embed.py|floe/*.def)
            echo "rust_renderer floe2 jobdeck density_stack index_lock" ;;
        floe/indexlock.py)
            # the cache locks' Python side (rust/vfs/src/lock.rs's twin)
            echo "index_lock jobdeck occupancy cell_tree drc_ice rust_renderer index_cli" ;;
        floe/render.py|floe/vfsclient.py)
            # the frozen KLayout shell: the oracle's side
            echo "render_goldens render_speckle render_frames vfs_render vfs_hier vfs_lifecycle floe2 klayout" ;;
        floe/jobdeck/*)
            echo "jobdeck occupancy rust_renderer index_lock" ;;
        floe/drc.py|floe/svrf.py)
            echo "drc_ice svrf index_lock" ;;
        floe/cli.py|floe/product.py|floe/__main__.py|floe2/*)
            echo "python jobdeck occupancy cell_tree index_lock" ;;
        *)
            echo ALL ;;
    esac
}

SRC=
ONLY=
CHANGED=0
CHANGED_BASE=HEAD
CHANGED_FILES=
DRY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --only)
            [ $# -ge 2 ] || { echo "usage: --only a,b,c"; exit 2; }
            ONLY="$ONLY,$2"; shift 2 ;;
        --only=*)
            ONLY="$ONLY,${1#--only=}"; shift ;;
        --changed)
            CHANGED=1; shift ;;
        --changed=*)
            CHANGED=1; CHANGED_BASE=${1#--changed=}; shift ;;
        --files=*)
            CHANGED=1; CHANGED_FILES=$(echo "${1#--files=}" | tr ',' ' '); shift ;;
        --dry-run)
            DRY=1; shift ;;
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

# --changed: the selection from the files that differ from the base
if [ $CHANGED = 1 ]; then
    git rev-parse --verify -q "$CHANGED_BASE" >/dev/null || {
        echo "--changed: unknown revision $CHANGED_BASE"; exit 2; }
    if [ -n "$CHANGED_FILES" ]; then
        FILES=$CHANGED_FILES
    else
        FILES=$( { git diff --name-only "$CHANGED_BASE" --; \
                   git ls-files --others --exclude-standard; } | sort -u)
    fi
    WHOLE=
    PICKED=
    for f in $FILES; do
        want=$(gates_for "$f")
        case " $want " in
            *" ALL "*) WHOLE="$WHOLE $f" ;;
            *) PICKED="$PICKED $want" ;;
        esac
    done
    if [ -n "$CHANGED_FILES" ]; then
        echo "== --files: $(echo $FILES | wc -w | tr -d ' ') paths"
    else
        echo "== --changed: $(echo $FILES | wc -w | tr -d ' ') files differ from $CHANGED_BASE"
    fi
    if [ -n "$WHOLE" ]; then
        echo "== the whole battery, for:$WHOLE"
        ONLY=
    elif [ -z "$(echo $PICKED)" ]; then
        echo "== no gate: nothing a gate checks changed"
        echo "RUST VALIDATION: ALL OK (--changed; gates: none)"
        exit 0
    else
        ONLY=$(echo $PICKED | tr ' ' ',')
    fi
fi
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
if [ $CHANGED = 1 ] || [ $DRY = 1 ]; then
    RUNS=; SKIPS=
    for g in $GATES; do
        hit=0
        if [ -z "$ONLY" ]; then hit=1; fi
        for sel in $SELECTED; do [ "$sel" = "$g" ] && hit=1; done
        # (unit holds its three parts)
        if [ $hit = 1 ]; then RUNS="$RUNS $g"; else SKIPS="$SKIPS $g"; fi
    done
    echo "== gates to run:$RUNS"
    [ -n "$SKIPS" ] && echo "== gates left out:$SKIPS"
    if [ $DRY = 1 ]; then exit 0; fi
fi
# gate NAME: whether NAME runs in this invocation
gate() {
    [ -z "$ONLY" ] && return 0
    for g in $SELECTED; do [ "$g" = "$1" ] && return 0; done
    return 1
}
RAN=
# lap NAME: gate NAME starts now (and the one before it ended): the
# seconds each took are listed at the end (2026-10-06)
LAPS=
LAP_NAME=
LAP_T0=$(date +%s)
BATTERY_T0=$LAP_T0
lap() {
    lap_now=$(date +%s)
    if [ -n "$LAP_NAME" ]; then
        LAPS="$LAPS $LAP_NAME=$((lap_now - LAP_T0))s"
    fi
    LAP_NAME=$1
    LAP_T0=$lap_now
}
lap setup

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
lap ""
echo "== floe2 product + accuracy gates (KLayout = oracle/generator only)"
if gate unit; then
    RAN="$RAN unit"; lap unit
    (cd rust && PATH="$HOME/.cargo/bin:$PATH" cargo test --workspace)
fi
if gate unit_vfs && ! gate unit; then
    RAN="$RAN unit_vfs"; lap unit_vfs
    (cd rust && PATH="$HOME/.cargo/bin:$PATH" cargo test --release --lib -p floe-vfs -p floe-ovm)
fi
if gate unit_render && ! gate unit; then
    RAN="$RAN unit_render"; lap unit_render
    (cd rust && PATH="$HOME/.cargo/bin:$PATH" cargo test --release --lib -p floe-render-core)
fi
if gate unit_renderd && ! gate unit; then
    RAN="$RAN unit_renderd"; lap unit_renderd
    (cd rust && PATH="$HOME/.cargo/bin:$PATH" cargo test --release -p floe-renderd)
fi
if gate index_cli; then RAN="$RAN index_cli"; lap index_cli
    .venv/bin/python tools/validate_index_cli.py; fi
if gate vfs_profile; then RAN="$RAN vfs_profile"; lap vfs_profile
    .venv/bin/python tools/validate_vfs_profile.py "$FLOE2_SMOKE_SRC"; fi
if gate floe2; then RAN="$RAN floe2"; lap floe2
    .venv/bin/python tools/validate_floe2.py "$FLOE2_SMOKE_SRC"; fi
OUT="${SRC%.oas}_rust.tiles"
if gate rust_scan; then RAN="$RAN rust_scan"; lap rust_scan
    .venv/bin/python tools/validate_rust_scan.py "$SRC"; fi
if gate rust_tiles; then RAN="$RAN rust_tiles"; lap rust_tiles
    .venv/bin/python tools/validate_rust_tiles.py "$SRC" "$OUT"; fi
if gate rust_depth; then RAN="$RAN rust_depth"; lap rust_depth
    .venv/bin/python tools/validate_rust_depth.py "$SRC" "$OUT"; fi
if gate rust_meta; then RAN="$RAN rust_meta"; lap rust_meta
    .venv/bin/python tools/validate_rust_meta.py "$SRC" "$OUT"; fi
if gate rust_skel; then RAN="$RAN rust_skel"; lap rust_skel
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
if gate vfs; then RAN="$RAN vfs"; lap vfs
    .venv/bin/python tools/validate_vfs.py "$SRC" "$VOUT"; fi
if gate vfs_render; then RAN="$RAN vfs_render"; lap vfs_render
    .venv/bin/python tools/validate_vfs_render.py "$SRC" "$VOUT"; fi
if gate vfs_coverage; then RAN="$RAN vfs_coverage"; lap vfs_coverage
    .venv/bin/python tools/validate_vfs_coverage.py "$SRC" "$VOUT"; fi
# occupancy pyramid (design.ovo): every level-0 bit vs KLayout's shape
# intersection on fixtures + the asset at a coarse cell; CLI contract
if gate occupancy; then RAN="$RAN occupancy"; lap occupancy
    .venv/bin/python tools/validate_occupancy.py "$SRC"; fi
if gate vfs_hier; then RAN="$RAN vfs_hier"; lap vfs_hier
    .venv/bin/python tools/validate_vfs_hier.py "$SRC" "$VOUT"; fi
if gate vfs_lifecycle; then RAN="$RAN vfs_lifecycle"; lap vfs_lifecycle
    .venv/bin/python tools/validate_vfs_lifecycle.py "$SRC" "$VOUT"; fi
if gate vfs_marker; then RAN="$RAN vfs_marker"; lap vfs_marker
    .venv/bin/python tools/validate_vfs_marker.py "$SRC"; fi
# rep-split page honesty (ovm v3): floor collapse + fragment
# conservation on a synthetic rep-flood asset
if gate vfs_split; then RAN="$RAN vfs_split"; lap vfs_split
    .venv/bin/python tools/validate_vfs_split.py; fi
# v5 text index: oracle XOR, declutter, corrupt, determinism,
# daemon label lifecycle
if gate vfs_text; then RAN="$RAN vfs_text"; lap vfs_text
    .venv/bin/python tools/validate_vfs_text.py; fi
# rasterization goldens for the rust-renderer replacement:
# bake (version-pinned) + determinism + policy discrimination
if gate render_goldens; then RAN="$RAN render_goldens"; lap render_goldens
    .venv/bin/python tools/validate_render_goldens.py; fi
# viewer speckle fill: common phase, opaque overlap, and the
# coverage composite staying out of speckled interiors
if gate render_speckle; then RAN="$RAN render_speckle"; lap render_speckle
    .venv/bin/python tools/validate_render_speckle.py; fi
# frame outline stacking: white over gray, 1px hollow, under design
if gate render_frames; then RAN="$RAN render_frames"; lap render_frames
    .venv/bin/python tools/validate_render_frames.py; fi
# DRC pack (.<db>.tray): reading through the pack == ASCII parse
if gate drc_ice; then RAN="$RAN drc_ice"; lap drc_ice
    .venv/bin/python tools/validate_drc_ice.py; fi
# SVRF subset parser: preprocessing / derivation closure / check
# extraction / end-to-end vs gen_drcdb --svrf
if gate svrf; then RAN="$RAN svrf"; lap svrf
    .venv/bin/python tools/validate_svrf.py; fi
# OASIS TRAPEZOID / CTRAPEZOID (mask data): every record type through
# floe-index + clip must equal KLayout's reading of the same bytes
if gate oasis_shapes; then RAN="$RAN oasis_shapes"; lap oasis_shapes
    .venv/bin/python tools/validate_oasis_shapes.py; fi
# Calibre MDPView jobdeck (.jb): parser / hand-computed placements /
# MDPView colour order / header probe / CLI / batch index
if gate jobdeck; then RAN="$RAN jobdeck"; lap jobdeck
    .venv/bin/python tools/validate_jobdeck.py; fi
# design.ovr (OVR1) end to end: additive build preserves the cache,
# depth/kill switch/invalid fallback, and a failed OVR inside a
# combined index run leaves a complete base cache behind
if gate representatives; then RAN="$RAN representatives"; lap representatives
    .venv/bin/python tools/validate_representatives.py; fi
# the synthetic MAIN01 generator: legacy geometry byte-identical, chip
# geometry deterministic, KLayout-readable, indexable and chip-shaped
if gate gen_main01; then RAN="$RAN gen_main01"; lap gen_main01
    .venv/bin/python tools/validate_gen_main01.py; fi
# budget-fitted cut: a keep view over the generation budget is drawn at a
# lowered density instead of failing; frames that fit are untouched
if gate fit_budget; then RAN="$RAN fit_budget"; lap fit_budget
    .venv/bin/python tools/validate_fit_budget.py; fi
# sub-cut boxes: under thin keep with few layers visible, what the size cut
# drops stays as a box from index metadata; everything else unchanged
if gate sub_cut_box; then RAN="$RAN sub_cut_box"; lap sub_cut_box
    .venv/bin/python tools/validate_sub_cut_box.py; fi
# per-shape cut: under thin keep the cut judges every shape by its smaller
# side - pages by max_min, shapes inside the pages that stay by the raster
if gate shape_cut; then RAN="$RAN shape_cut"; lap shape_cut
    .venv/bin/python tools/validate_shape_cut.py; fi
# write-once tiles: planes painted in reverse, each pixel written once,
# covered work skipped - frames byte-identical to the ordered overwrite
if gate write_once; then RAN="$RAN write_once"; lap write_once
    .venv/bin/python tools/validate_write_once.py; fi
# layer-decode probe: the same plan painted layer by layer over tiles that
# stay alive is byte-identical to the normal render (render_probe)
if gate layer_decode; then RAN="$RAN layer_decode"; lap layer_decode
    .venv/bin/python tools/validate_layer_decode.py; fi
# area-true drawing: a shape lights the pixels whose centres it covers, its
# outline is their rim, a sub-pixel shape is kept with the chance its area
# fills its pixels - widths and gaps kept, pan-stable, kill switch = KLayout
if gate area_true; then RAN="$RAN area_true"; lap area_true
    .venv/bin/python tools/validate_area_true.py; fi
# density stack (diagnostic FLOE_RUST_DENSITY_STACK=top): the top layer's
# density, the others' only where no original and no density above stands
if gate density_stack; then RAN="$RAN density_stack"; lap density_stack
    .venv/bin/python tools/validate_density_stack.py; fi
# cell tree (SPEC-VIEWER §8c): design.ovh's children / parents / instance
# counts vs KLayout on a hierarchy fixture, the daemon's cells / cell_find /
# cell_bbox / cell_insts answers vs a KLayout instance walk, the hier CLI and
# the inline / missing-summary / live-pickup contract
if gate cell_tree; then RAN="$RAN cell_tree"; lap cell_tree
    .venv/bin/python tools/validate_cell_tree.py; fi
# index locks (rust/vfs/src/lock.rs, floe/indexlock.py): a run writing a
# cache refuses another (whoever runs it), a whole rebuild refuses its
# readers and is refused by them, additions go beside readers on one host
if gate index_lock; then RAN="$RAN index_lock"; lap index_lock
    .venv/bin/python tools/validate_index_lock.py; fi
# the shared Rust app layer (feature/webui's crates, P0 2026-10-09):
# worker-client - the Rust renderd client - frame for frame against the
# Python adapter on valmini (raw/PNG, labels, styles, cancel soak); app-core's
# cell index (design.ovh added beside a live reader, kept on a rerun, stale /
# missing / busy / cancel refused)
if gate worker_client; then RAN="$RAN worker_client"; lap worker_client
    (
        worker_gate_dir=$(mktemp -d "${TMPDIR:-/tmp}/floe-worker-gate.XXXXXX")
        trap 'rm -rf "$worker_gate_dir"' EXIT HUP INT TERM
        rust/target/release/floe-index vfs "$FLOE2_SMOKE_SRC" \
            "$worker_gate_dir/valmini.floe" --jobs 2 >/dev/null
        sh tools/validate_worker_client.sh "$FLOE2_SMOKE_SRC" \
            "$worker_gate_dir/valmini.floe"
    )
fi
if gate cell_index; then RAN="$RAN cell_index"; lap cell_index
    (cd rust && FLOE_INDEX_BIN="$PWD/target/release/floe-index" cargo test --release --offline -p floe-app-core --lib cell_index &&
        FLOE_INDEX_BIN="$PWD/target/release/floe-index" cargo test --release --offline -p floe-app-core --test cell_index -- --ignored); fi
# in-tree CPU renderer: Python queue contract plus independent
# KLayout pixel oracle at deterministic serial/parallel settings
if gate rust_renderer; then RAN="$RAN rust_renderer"; lap rust_renderer
    PYTHONDONTWRITEBYTECODE=1 FLOE_RENDERER=rust FLOE_RUST_ROUND_PAGES=4 \
        FLOE_INTEGRATION_SOURCE="$SRC" FLOE_INTEGRATION_CACHE="$VOUT" \
        .venv/bin/python tools/validate_rust_renderer.py; fi
if gate klayout; then RAN="$RAN klayout"; lap klayout
    echo "== KLayout pixel oracle (frozen shell as reference)"
    PYTHONDONTWRITEBYTECODE=1 .venv/bin/python tools/validate_klayout_oracle.py --jobs 1
    PYTHONDONTWRITEBYTECODE=1 .venv/bin/python tools/validate_klayout_oracle.py --jobs 8
fi
lap ""
echo "== gate seconds ($(( $(date +%s) - BATTERY_T0 ))s in all):$LAPS"
if [ -n "$ONLY" ]; then
    echo "RUST VALIDATION: ALL OK ($SRC; gates:$RAN)"
else
    echo "RUST VALIDATION: ALL OK ($SRC)"
fi
