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
Rectangle, polygon, and PATH-outline interiors all use the same Q32.32
`PixelCenter | LowerBoundary` scan-conversion policy; rectangles retain an
allocation-free fast path driven by the same phase-bound helpers.

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
screens. It covers all 13 PX1-PX5 views, two half-phase representation-exact
checks, plus 14 styled checks for a half-pixel viewport, 16x16 pattern phase,
speckle, 1/2/4px outlines, overlapping paint
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
render gen=10 view=0,0,404000,447000 w=1200 h=800 depth=full cut=0 exact=0 layers=1/0,2/0 frames=on labels=on font_px=14 mono=off jobs=4 decode_jobs=8 tile_px=384 decode_pages=99 round_pages=32 style_epoch=1 out=/tmp/frame.png
clip seq=12 box=0,0,404000,447000 layers=1/0,2/0 jobs=4 out=/tmp/clip.oas
snap seq=20 x=1000 y=2000 r=10 layers=1/0,2/0
pick seq=21 x=1000 y=2000 r=3 nth=0 layers=1/0,2/0
cancel before_gen=11
info
quit
```

`font_px` is an integer screen-pixel size in `6..96`. The Python Rust worker
passes it on every render request, so `View > label size... (Rust renderer)` or
`floe2 view --label-font-px PX` changes size live without restarting the daemon
or rebuilding the index. The menu item is disabled for backends that do not
advertise this capability. `FLOE_RUST_LABEL_PX` remains only the headless
adapter fallback when a request omits the field.

The same request size controls glyph rasterization, world-anchored declutter
spacing, hierarchy-name fit, ellipsis fit, and hierarchy-name padding. Those decisions
are made before rasterization but are scaled from the bundled-font 14px
calibration, so changing the font does not leave a hidden KLayout/default-size
layout policy behind. The 14px default preserves the original selection and
pixel contract byte-for-byte.

Design-label orientation is derived from the full composed hierarchy
placement: the local text baseline follows its top-coordinate 0/90/180/270
degree direction. A reflected instance moves the anchor and baseline exactly
but does not mirror the glyph bitmap, keeping annotations readable. Block names
remain runtime annotations aligned to the long side of their top-coordinate
frame. This behavior is owned by the Rust renderer; KLayout's text renderer
was verified to draw transformed text horizontally and cannot serve as the
rotation oracle.

OASIS TEXT itself has no angle field. Quarter-turn orientation therefore comes
from composed record-17 hierarchy placements. Magnified/arbitrary-angle
record-18 placements remain an explicit parser/indexer scope error for all
geometry, not a text-rendering fallback; the renderer never silently draws such
text horizontally.

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
and rasterizing later rounds. Query traversal skips decoded pages whose
cell-local bbox misses the probe and examines at most 400 repetition members,
including non-visible members of sparse explicit-point repetitions. Snap also
examines at most 400 touching shapes and prefers any in-radius vertex over the
nearest edge. Pick retains at most 64 containing candidates, is
boundary-inclusive, sorts by
`(integer area, layer, datatype)`, and preserves `nth` overlap cycling.

Renderer repetition traversal rejects collinear or zero-vector two-dimensional
grids explicitly before enumerating them; the normal one-dimensional grid forms
remain supported. Page OASIS point-list and explicit-point repetition counts
are bounded by the remaining payload bytes before any proportional allocation,
and corrupt payloads return a page decode error rather than attempting an
unbounded allocation.

floe2 intentionally has no density-coverage path. Its CLI does not expose
`--coverage`/`--coverage-only`, the GUI has no `v` control or coverage request
state, and `RustRenderWorker` never opens or composites `design.ovc`. Old/shared
caches may retain the optional sidecar; floe2 ignores it. This was retired after
sample09 detail-high refinement measured 350ms without coverage and 980ms with
it, while producing no visible difference. Stable floe's KLayout-only optional
overlay remains outside this Rust product contract.

Page loading keeps the parent's file-order batched OVP read, then parses the
independent page OASIS payloads with up to `decode_jobs` workers. Raster tiles
use `jobs`; omitting `decode_jobs` preserves the legacy behavior of using
`jobs` for both phases. Parse completion
order cannot change decoded-page order, LRU insertion order, or which corrupt
page error is reported first. On the 506-page `sample9` full-depth mid-zoom
view, release decode median changed from 131.8ms at one worker to 41.9ms at four
and 31.7ms at eight workers (3.15x/4.16x); cached rounds perform no decode.

The daemon progressively publishes priority-ordered cache misses in batches.
All cache hits enter the first scene regardless of count; `round_pages=N`
limits only new misses, and a final miss tail no larger than half a batch is
coalesced into its predecessor. The default is 128 and `decode_pages=N` remains
the total page cap. Every successful response for the same generation includes
`round=N final=0|1 partial=0|1`; the output path is atomically replaced after
each round. A newer generation cancels the remaining decode/raster rounds.
Before this cache-aware policy, a 506-page `sample9` run with `round_pages=64`
published its first 600x600 partial frame in roughly 10ms over eight rounds;
the final PNG was byte-identical to single-shot rendering.

A historical cold-daemon gate on a 303-page dense `sample9` mid-zoom view measured
first/final frame latency of 17/290ms, 17/159ms, 21/122ms, and 31/99ms for
`round_pages=32/64/128/256` respectively at 600x600 with eight workers. The
default remains 128: it gives up 4ms of first-paint latency versus 64 while
saving 37ms to the final frame. A 100-generation 1000x700 pan burst at that
setting published zero stale frames, produced three frames for the latest
generation, and left no pending daemon job.

The daemon also keeps a bounded LRU of the last three settled deterministic
PNGs, capped at 64 MiB total. An exact request-key revisit can use it only when
every selected decoded page is still resident. The daemon rebuilds and
publishes the immutable `FrameScene` for the new generation, then atomically
publishes the cached PNG with `raster_us=0`, `png_us=0`, and
`frame_cache_hit=1`. It does not retain old scene Arcs outside the page budget,
so pick/snap stays synchronized with the visible cached frame. View bits,
framebuffer size, depth/cut, layers, frames/labels/font, mono, decode cap, and
style epoch are in the key; style changes and cache open clear the LRU.

The rasterizer uses independently owned 2D tile scratch buffers and dynamic
atomic work assignment. Tile completion order cannot affect pixels because the
coordinator copies every tile to its fixed framebuffer position. On the styled
1200x800 `valmini` view, 64/128/256px tiles and 1/4/8 workers all match the
former band renderer byte-for-byte. With 128px tiles, five-run median
`raster_us` was 94.733/27.511/23.083ms for 1/4/8 workers (3.44x and 4.10x).

See [RUST_RENDERER_PLAN.ko.md](RUST_RENDERER_PLAN.ko.md) for scope and gates.

## floe2 product boundary

`floe2` is the Rust-only product shell. The stable `floe` shell defaults to
KLayout, while both products share this renderer implementation, the canonical
`rust/` workspace, and the same VFS cache. Their GUI instance sockets are
separate, so both screens can run on one display for comparison.

```sh
.venv/bin/python -m floe2 view --multi design.oas
```

`floe2` rejects `FLOE_RENDERER=klayout` instead of falling back. Stable `floe`
accepts `FLOE_RENDERER=rust` only for explicit A/B scripts.
An unknown backend, malformed `MODULE:TYPE`, failed import, or non-callable
worker is a hard error; the hook never silently falls back and contaminates an
A/B run.

With the floe2 Rust backend, importing the cache reader, backend factory, and GTK
shell no longer imports `klayout.db`, `floe.render`, or `floe.viewport`. Shared
frame-layer/live-cap policy lives in a pure-Python module, while the legacy
database and renderer modules load only inside the KLayout worker. Therefore
floe2 `view`, `render`, `probe`, `info`, and `clip` can start on a KLayout-free
installation; the Python indexer and explicitly selected legacy commands still
require KLayout and remain in stable floe only.
The validation suite enforces this with a fresh subprocess whose import hook
rejects every `klayout` module.

Abstract mode remains intentionally KLayout-only. The Rust worker advertises
that capability as unavailable, so the GUI clears the state and disables the
menu/`a` action instead of submitting a render request that can never succeed.

The adapter is implemented at `floe/rust_render.py` and
accepts the existing `RenderWorker` constructor and queue contract. It owns one
persistent `floe-renderd`, translates
render/recolor/repattern/mono/pick/snap jobs,
converts layerprops and live 16x16 fills to deterministic Rust styles, maps
progressive telemetry back to the existing frame result schema, and cleans up
its private frame/style directory on shutdown. The default worker target is
already `floe.rust_render:RustRenderWorker`, so
`FLOE_RUST_WORKER` is needed only to override it.

The adapter requests `round_paths=1`, which gives every intermediate frame a
unique handoff path. This removes the race where the daemon could atomically
replace a shared path with round N+1 while Python was consuming the response
for round N; the unchanged daemon default still publishes to one path. Consumed
partial and final PNGs are removed immediately, as are style TSVs after the
daemon acknowledges them.

The real parent `Cache` integration test also submits a 100-generation
pan/zoom burst. It passed on both `valmini` and the 506-page `sample9`: none of
the previous 99 generations published a frame, only the latest generation
settled, and no partial file or pending adapter job remained.

The operational knobs are `FLOE_RENDERD_BIN`, `FLOE_RUST_JOBS` (page decode,
default up to 8 host CPUs), `FLOE_RUST_RASTER_JOBS` (default up to 4 and never
above decode jobs), `FLOE_RUST_BUDGET_MB` (1024), `FLOE_RUST_ROUND_PAGES` (1024),
`FLOE_RUST_TILE_PX` (384), and `FLOE_RUST_LABEL_PX` (14, whole-pixel range
6..96). The raw daemon fallback remains one shared jobs value and 128px when
the new fields are omitted. `FLOE_RUST_OPEN_TIMEOUT_S` and
`FLOE_RUST_CLIP_TIMEOUT_S` both default
to 300 seconds. Cold cache open runs outside the GTK thread, and the daemon is
placed in its own POSIX session so terminal SIGINT does not kill it.
A headless smoke test is:

```sh
FLOE_RENDERER=rust \
.venv/bin/python -B -m floe probe data/m1/valmini.oas
```

`floe render` also uses the worker when the Rust backend is selected. It keeps
the archival solid-fill policy of the previous command, waits for the settled
progressive frame, validates the PNG, and publishes it with fsync + atomic
replace. `--labels --label-font-px 19` renders design text using the bundled
font, including composed quarter-turn hierarchy orientation;
`--frames --depth N` additionally exports hierarchy-frontier boxes and names.
The default remains no frames or labels for byte-policy compatibility with the
old headless command.

Pass `--multi` when launching the GTK viewer so the request cannot forward to
an already-running KLayout process. Current adapter scope is render,
progressive refinement, visibility, depth, cut, frames, labels, color, fill,
width, mono, pick, snap, and exact OASIS clip. Clip builds a
cut=0/full-depth plan, decodes pages with the daemon worker count and persistent
LRU, flattens hierarchy/repetitions, and writes one `FLOE_CLIP` cell. Rectangle
type is preserved; paths become polygons like KLayout. Concave intersections
are split into components and diagonal boundary intersections use KLayout's
nearest-DBU, half-toward-positive-infinity rule. The daemon always writes a
private whitespace-free path; the Python adapter fsyncs and atomically replaces
the user-selected destination, so destination paths may contain spaces.

Label strings, positions, visibility, declutter, block-name fit, and budgets
come directly from the existing Rust VFS planner. The renderer uses a bundled
Noto Sans Mono font with center alignment, deterministic integer alpha
composition, and 0/90/180/270-degree rotation. It never consults the OS or
KLayout font engine. If the 262,144-glyph raster cap is reached, it renders a
deterministic whole-label prefix, reports `labels_truncated=1`, and still
publishes the geometry frame.

Frame telemetry separates `raster_us`, `png_us`, and atomic publication into
`publish_write_us`, `publish_sync_us`, and `publish_rename_us`; the Python
adapter adds its output-file handoff time. The GUI performance status and
`tools/bench_floe2.py` expose these fields. Publication still uses `sync_all()`:
the measured 3--6ms is too small to justify weakening the atomic publication
contract.
The GUI line also reports requested raster jobs, actual image tiles, tile size,
framebuffer dimensions, and `frame-cache` on an exact revisit. The field
benchmark accepts `--detail low|medium|high` and includes a
`hotspot_revisit` case. On an 858x789 `sample9` detail-high hotspot, the first
frame measured 198ms while the exact revisit after one intervening frame took
6ms with raster and PNG encode both zero.
The floe2 adapter uses 1024 miss pages per interactive round. A field pan with
744 misses previously produced six cumulative raster/PNG passes (54 reported
image tiles for a nine-tile framebuffer) and took 936ms. The larger product
batch preserves the frozen previous frame until one settled result; the raw
daemon fallback remains 128 and requests above 1024 misses still refine.
Abstract mode is a KLayout-specific feature
and is intentionally outside the Rust renderer scope; it will not be
implemented.

The native Python/GTK viewer runs directly on this Mac; XQuartz is not part of
the Rust-backend launch path. A real `sample9` full-depth mid-zoom session
measured 25ms cold (4ms load + 13ms draw), then 16ms for an adjacent forwarded
pan with no new page and 11ms for a 2x warm zoom. A native parent-adapter
`valmini` labels-on render selected 136 labels in 0.065ms and published 28,377
antialiased label pixels without `labels partial`.
