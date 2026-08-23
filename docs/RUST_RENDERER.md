# floe Rust CPU renderer

Deterministic, multicore CPU renderer for `floe` OVM/OVP caches.

The Rust indexer, hierarchy planner, and renderer live in the same `rust/`
workspace. The renderer consumes their public APIs directly and does not copy
parser or planner code.

Current milestone: deterministic multicore styled-geometry rendering plus a
persistent, cancellable render daemon. The output traverses the full `HierPlan`,
including One/Grid/Pts repetitions, and writes a deterministic RGBA PNG. Rectangle, even-odd polygon, and exact
Manhattan PATH fill (including square-miter joins and end extensions), KLayout-
compatible arbitrary-angle polyline PATH fill (including acute-corner clipping),
layer visibility/color/paint order, shared-phase speckle, KLayout-phased 16x16
patterns, 1..8px outlines, the original-spine PATH style centerline, mono mode,
planner washes, and all four hierarchy-frame bands are implemented. PATH
begin/end extensions affect the hull but not that centerline. Degenerate and
U-turn PATHs are outside the verified operating-input scope and fail the render
explicitly; they are never silently omitted. Rendering uses CPU workers only;
no GPU path is planned.

```sh
cd rust
cargo run -p floe-render-cli -- \
  ../data/m1/valmini.oas.floe \
  --view 0,0,404,447 --width 1200 --height 800 \
  --depth full --cut-px 0 --decode-pages 99 --budget-mb 64 \
  --layers 1/0,2/0 --jobs 4 --tile-px 128 \
  --style '1/0,#3fff77,speckle,1' \
  --style '2/0,#af3fff,speckle,4' --frames on \
  --out /tmp/valmini-styled.png
```

`--view` coordinates are microns, matching `floe-index plan`. The command
prints a deterministic tab-separated plan/decode/raster summary. Raster
`*_tests` and `*_paints` fields are per-tile work counters and therefore vary
with `--tile-px`; they are not unique source-geometry counts.
Repeated `--style` options define bottom-to-top design paint order. A style is
`L/D,#RRGGBB,solid|speckle|clear|pat:HEX64[,outline-width]`; `--mono on` converts
design colors to luminance without changing structural frame colors.

The parent `floe` commit `030faf2` (`floe-index` 0.11.46) provides the PX1-PX5
accuracy policy, the renderer-facing `Vfs::read_page_batch` contract, and
deterministic source-parse normalization of OASIS CIRCLE records into inscribed
64-gon `PolyRec`s without an OVM/OVP format change. The Rust renderer passes all
13 PX1-PX5 views under P-a/P-b/P-c, with byte-identical PNGs for 1/4/8 workers.
CIRCLE is a compatibility fallback rather than part of the Calibre-screen
primitive contract; renderer pages contain only the normalized polygons.

## KLayout accuracy oracle

`tools/validate_klayout_oracle.py` independently writes OASIS fixtures, indexes
them through the release `floe-index`, renders the source through
KLayout and the cache through `floe-render-cli`, and compares the resulting
screens. It covers all 13 PX1-PX5 views plus 14 styled checks for a half-pixel
viewport, 16x16 pattern phase, speckle, 1/2/4px outlines, overlapping paint
order, visibility, PATH styling, and mono.

```sh
(cd rust && cargo build --release --workspace)
.venv/bin/python -B tools/validate_klayout_oracle.py --jobs 1
.venv/bin/python -B tools/validate_klayout_oracle.py --jobs 8
```

Geometry uses the parent's fixed P-a/P-b/P-c contract. Styled RGB must be exact
outside that accepted one-pixel geometry edge band expanded only by the active
device-line radius. This keeps pattern/color errors strict while allowing the
same documented binary edge choice at wider outlines. Both one- and eight-worker
runs currently pass. The oracle established two non-obvious KLayout rules now
covered by Rust unit tests: custom pattern row `r` samples source row
`(r + framebuffer_height - 1) mod 16`, and a styled PATH draws its centerline
on the unextended original spine.

`path-inventory` provides a bounded, parallel audit of the PATH records already
stored in one or more caches:

```sh
cargo run --release -p floe-render-cli --bin path-inventory -- \
  --jobs 8 --chunk-pages 256 /path/to/design.oas.floe
```

The current operating set found six PATH records/eight repetition members in
`valmini`, all accepted by the renderer, and no PATH records in `thintest`,
`stress30`, `sample9`, or `testchip_1g5` (30,456 pages across all five caches
and at least 2.31 GB of encoded pages). No U-turn, degenerate spine, negative
extension, or zero half-width PATH was found. OASIS PATH encodes start/end
extensions but has no round-cap primitive flag in the renderer page contract;
circle-like source geometry is expected to arrive as polygons after indexing.

## Persistent render daemon

`floe-renderd` owns one cache, a decoded-page LRU, the current style epoch, the
render worker, and the latest published immutable query scene for its process
lifetime. Its stdin/stdout protocol is line oriented; paths and field values
must not contain whitespace. Unlike the CLI, daemon `view` coordinates are raw
DBU.

```text
open cache=/abs/valmini.oas.floe budget_mb=64 jobs=4
style epoch=1 path=/tmp/valmini.styles
render gen=10 view=0,0,404000,447000 w=1200 h=800 depth=full cut=0 exact=0 layers=1/0,2/0 frames=on labels=on font_px=14 mono=off jobs=4 tile_px=128 decode_pages=99 round_pages=32 style_epoch=1 out=/tmp/frame.png
snap seq=20 x=1000 y=2000 r=10 layers=1/0,2/0
pick seq=21 x=1000 y=2000 r=3 nth=0 layers=1/0,2/0
cancel before_gen=11
info
quit
```

`font_px` is an integer screen-pixel size in `6..96`. The Python Rust worker
passes it on every render request, so `View > label size... (Rust renderer)` or
`floe view --label-font-px PX` changes size live without restarting the daemon
or rebuilding the index. The menu item is disabled for backends that do not
advertise this capability. `FLOE_RUST_LABEL_PX` remains only the headless
adapter fallback when a request omits the field.

Design-label orientation is derived from the full composed hierarchy
placement: the local text baseline follows its top-coordinate 0/90/180/270
degree direction. A reflected instance moves the anchor and baseline exactly
but does not mirror the glyph bitmap, keeping annotations readable. Block names
remain runtime annotations aligned to the long side of their top-coordinate
frame. This behavior is owned by the Rust renderer; KLayout's text renderer
was verified to draw transformed text horizontally and cannot serve as the
rotation oracle.

The style file is bottom-to-top, one `L/D COLOR FILL WIDTH` row per layer:

```text
1/0 #3fff77 speckle 1
2/0 #af3fff pat:8000800080008000800080008000800080008000800080008000800080008000 4
```

Render generations must increase strictly. Submitting generation `N` cancels
all older work, and `cancel before_gen=N` applies the same strict frontier.
PNG publication is a same-directory write/sync/rename transaction serialized
with frontier changes, so a stale generation cannot commit a frame. Successfully
decoded immutable pages remain reusable in the LRU. `exact=1` is accepted only
with `cut=0 depth=full frames=off`; conflicting options are errors.

Every successfully published refinement round atomically replaces the shared
query snapshot with that round's `FrameScene`. Snap and pick therefore inspect
exactly the decoded design geometry currently on screen, including hierarchy,
orthogonal transforms, repetitions, paths, and planner washes, while excluding
draw-only frames and live labels. They never load delta OASIS into KLayout and
never consult a stale KLayout shadow scene. The stdin thread clones the scene
`Arc` and performs the bounded query while the render worker continues decoding
and rasterizing later rounds. Snap examines at most 400 touching shapes and
prefers any in-radius vertex over the nearest edge. Pick retains at most 64
containing candidates, is boundary-inclusive, sorts by
`(integer area, layer, datatype)`, and preserves `nth` overlap cycling.

The GUI's optional density coverage (`v`) reuses `design.ovc` and the existing
NumPy/Pillow post-compositor; it does not invoke KLayout. Every progressive
Rust PNG is tinted with the live layer palette only when coverage is requested,
`cut_px > 0`, and the finest OVC texel projects to at most 160 screen pixels.
The neighborhood-aware mask fills only genuinely blank screen regions, keeping
speckled vector interiors intact. This display-density feature is distinct from
8-bit antialiased edge coverage, which remains outside the binary pixel
contract.

Page loading keeps the parent's file-order batched OVP read, then parses the
independent page OASIS payloads with up to `jobs` workers. Parse completion
order cannot change decoded-page order, LRU insertion order, or which corrupt
page error is reported first. On the 506-page `sample9` full-depth mid-zoom
view, release decode median changed from 131.8ms at one worker to 41.9ms at four
and 31.7ms at eight workers (3.15x/4.16x); cached rounds perform no decode.

The daemon progressively publishes priority-ordered pages in batches. Add
`round_pages=N` to select the batch size; the default is 128 and
`decode_pages=N` remains the total page cap. Every successful response for the
same generation includes `round=N final=0|1 partial=0|1`; the output path is
atomically replaced after each round. A newer generation cancels the remaining
decode/raster rounds. A 506-page `sample9` run with `round_pages=64` published
its first 600x600 partial frame in roughly 10ms over eight rounds, and the final
PNG was byte-identical to single-shot rendering.

A second cold-daemon gate on a 303-page dense `sample9` mid-zoom view measured
first/final frame latency of 17/290ms, 17/159ms, 21/122ms, and 31/99ms for
`round_pages=32/64/128/256` respectively at 600x600 with eight workers. The
default remains 128: it gives up 4ms of first-paint latency versus 64 while
saving 37ms to the final frame. A 100-generation 1000x700 pan burst at that
setting published zero stale frames, produced three frames for the latest
generation, and left no pending daemon job.

The rasterizer uses independently owned 2D tile scratch buffers and dynamic
atomic work assignment. Tile completion order cannot affect pixels because the
coordinator copies every tile to its fixed framebuffer position. On the styled
1200x800 `valmini` view, 64/128/256px tiles and 1/4/8 workers all match the
former band renderer byte-for-byte. With 128px tiles, five-run median
`raster_us` was 94.733/27.511/23.083ms for 1/4/8 workers (3.44x and 4.10x).

See [RUST_RENDERER_PLAN.ko.md](RUST_RENDERER_PLAN.ko.md) for scope and gates.

## floe backend selection

`floe` keeps KLayout as its default and rollback backend. Its GUI,
DRC snapshot renderer, and headless render probe now construct workers through
one lazy backend factory. The in-tree Rust adapter is selected explicitly:

```sh
FLOE_RENDERER=rust \
.venv/bin/python -m floe view --multi design.oas
```

Without `FLOE_RENDERER`, or with `FLOE_RENDERER=klayout`, behavior is unchanged.
An unknown backend, malformed `MODULE:TYPE`, failed import, or non-callable
worker is a hard error; the hook never silently falls back and contaminates an
A/B run.

The adapter is implemented at `floe/rust_render.py` and
accepts the existing `RenderWorker` constructor and queue contract. It owns one
persistent `floe-renderd`, translates
render/recolor/repattern/mono/pick/snap jobs and density-coverage frames,
converts layerprops and live 16x16 fills to deterministic Rust styles, maps
progressive telemetry back to the existing frame result schema, and cleans up
its private frame/style directory on shutdown. The default worker target is
already `floe.rust_render:RustRenderWorker`, so
`FLOE_RUST_WORKER` is needed only to override it.

The adapter requests `round_paths=1`, which gives every intermediate frame a
unique handoff path. This removes the race where the daemon could atomically
replace a shared path with round N+1 while Python was consuming the response
for round N; the unchanged daemon default still publishes to one path.

The real parent `Cache` integration test also submits a 100-generation
pan/zoom burst. It passed on both `valmini` and the 506-page `sample9`: none of
the previous 99 generations published a frame, only the latest generation
settled, and no partial file or pending adapter job remained.

The operational knobs are `FLOE_RENDERD_BIN`, `FLOE_RUST_JOBS` (default up to
8 host CPUs), `FLOE_RUST_BUDGET_MB` (1024), `FLOE_RUST_ROUND_PAGES` (128),
`FLOE_RUST_TILE_PX` (128), and `FLOE_RUST_LABEL_PX` (14, whole-pixel range
6..96).
A headless smoke test is:

```sh
FLOE_RENDERER=rust \
.venv/bin/python -B -m floe probe data/m1/valmini.oas
```

Pass `--multi` when launching the GTK viewer so the request cannot forward to
an already-running KLayout process. Current adapter scope is render,
progressive refinement, visibility, depth, cut, frames, labels, color, fill,
width, mono, pick, snap, and density coverage. Label strings, positions,
visibility, declutter,
block-name fit, and budgets come directly from the existing Rust VFS planner. The
renderer uses a bundled Noto Sans Mono font with center alignment, deterministic
integer alpha composition, and 0/90/180/270-degree rotation. It never consults
the OS or KLayout font engine. Abstract/clip return explicit errors.
Abstract mode is a KLayout-specific feature
and is intentionally outside the Rust renderer scope; it will not be
implemented. Clip remains follow-up work.

The native Python/GTK viewer runs directly on this Mac; XQuartz is not part of
the Rust-backend launch path. A real `sample9` full-depth mid-zoom session
measured 25ms cold (4ms load + 13ms draw), then 16ms for an adjacent forwarded
pan with no new page and 11ms for a 2x warm zoom. A native parent-adapter
`valmini` labels-on render selected 136 labels in 0.065ms and published 28,377
antialiased label pixels without `labels partial`.
