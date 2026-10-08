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
  ../data/m1/.valmini.oas.ice \
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
  --jobs 8 --chunk-pages 256 /path/to/.design.oas.ice
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

The initial `ready version=...` handshake is an exact compatibility boundary.
Its Cargo package version must match the Python product and `floe-index` version;
the adapter rejects a missing or stale version before opening the cache. This is
also a field diagnostic: an old daemon cannot masquerade as current code when a
repetition rule changes.

```text
open cache=/abs/.valmini.oas.ice budget_mb=64 jobs=4
style epoch=1 path=/tmp/valmini.styles
render gen=10 view=0,0,404000,447000 w=1200 h=800 depth=full cut=0 exact=0 layers=1/0,2/0 frames=on labels=on font_px=14 mono=off frame_cache=1 jobs=4 decode_jobs=8 tile_px=384 decode_pages=99 round_pages=32 style_epoch=1 out=/tmp/frame.png
clip seq=12 box=0,0,404000,447000 layers=1/0,2/0 jobs=4 out=/tmp/clip.oas
snap seq=20 x=1000 y=2000 r=10 layers=1/0,2/0
pick seq=21 x=1000 y=2000 r=3 nth=0 layers=1/0,2/0
cell_sources seq=30
cells seq=31 src=0 cell=17
cell_find seq=32 src=-1 pat_hex=2a696e763f limit=2000
cell_bbox seq=33 src=0 cell=9
cell_insts seq=34 src=0 cell=9 view=0,0,404000,447000 cap=4096
cancel before_gen=11
info
quit
```

`root=<cell index>` on `render`, `clip`, `cell_bbox` and `cell_insts` is the
view root (docs/SPEC-VIEWER.ko.md §8c): the plan starts from that cell in ITS
coordinates (depth counted from it; the published pick/snap scene, the frame's
retained state and the fit memory are per root); absent = the top cell. A root
outside the cell table is an error; a jobdeck refuses a root (its sources' tops
are its cells). Under a root the occupancy summary (a flattening of the top) is
not used.

The five `cell_*` commands are the viewer's cell tree (docs/SPEC-VIEWER.ko.md
§8c). They run on the daemon's own hier thread over the hierarchy summary
`design.ovh` (docs/SPEC-FORMATS.ko.md) and never touch the render or the
pick/snap thread. Every answer repeats the kind and `seq`; a failure is
`<kind> seq=N found=0 code=<nohier|superseded|state|query> err_hex=<utf-8 hex>`
(`nohier`: build the summary with `floe-index hier <cache>`; `superseded`: a
newer `cell_insts` was already queued behind this one). `src` is a source
index of the open cache (always 0) or deck (spec order; `cell_find src=-1`
searches every source). Answers:

```text
cell_sources seq=30 found=1 n=<sources> sources=<src>:<placements>:<path_hex>,...
cells seq=31 src=0 found=1 cell=17 name_hex=<hex> insts=<under the top> height=<levels> unit=<source dbu> bbox=<rbbox|-> n=<rows> total=<distinct children> children=<ci>:<members>:<leaf 0|1>:<name_hex>,...   (cell omitted = the source's top; rows by name, at most 20,000)
cell_find seq=32 src=-1 found=1 total=<matches> n=<rows> matches=<src>:<ci>:<insts>:<name_hex>,...   (substring, or a whole-name glob with * ?, case-insensitively; rows by name, at most limit <= 5,000)
cell_bbox seq=33 src=0 cell=9 found=1 insts=<n> approx=<0|1> bbox=<x0,y0,x1,y1|->   (view coordinates: the deck's dbu for a deck; approx=1 = the extent of the top-level placements holding the cell)
cell_insts seq=34 src=0 cell=9 found=1 n=<rows> more=<0|1> visited=<walk cost> boxes=<x0,y0,x1,y1>;...   (instance boxes meeting the view, at most cap <= 4,096 and a 2,000,000-visit walk budget; more=1 = partial)
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
Under the density stack's sub-cut dots (`FLOE_RUST_DENSITY_DOTS=on`,
CUT_DENSITY_DESIGN §10.12 step 3) a frame first answers
`frame gen=N round=1 final=0 partial=1 deferred=1 density_round=1` with pass 1
alone (the originals, before pass 2 plans) at `<out>.gen-N.round-1.partial.<fmt>`,
then the final frame - byte for byte what one round draws. Never for a margin
(`bg`); `FLOE_RUST_DENSITY_PROGRESSIVE=off` draws one round. A newer generation
cancels the plans too (floe_vfs `HierOpts::stop` through `Cache::plan_cancellable`,
0.12.235): a zoom during pass 2 no longer waits for its plan to finish. The
adapter's `cancel(before_gen)` (the viewer's Esc, 0.12.251) sends
`cancel before_gen=N`; the daemon's `cancelled gen=N phase=render|queued` is
forwarded to the viewer as a `cancelled` result (the `before_gen=` ack is not).
`FLOE_RUST_DENSITY_FLOOR_PX` (diagnostic, default 1 since 0.12.261 - 0 before;
at the density cut there is no floor probe) is the dots' page and record floor
in px; a page whose every shape is under it is not drawn
(`FLOE_RUST_DENSITY_PAGE_DOTS=on` stands it in as dots, as before 0.12.261 -
they were far denser than the shapes); the frame line's `density_floor=<px>` is the records' cut
pass 2 actually planned at (that floor, the density cut, or what a budget fit
raised it to). `FLOE_RUST_DENSITY_BLOCK_PX` (diagnostic, default 4 - 8 in 0.12.257-0.12.260;
4..256 - 4..16 before 0.12.260;
0.12.257) is the dots' block: the planner walks a cut node down to a block, so
a larger block is less detail and less work; the frame line's
`density_block=<px>` reports it and the viewer's log line shows it
(`[density: dots, lit L px, block 4 px, floor 1 px, pass 2 plan N ms (...), P pages]`;
since 0.12.263 the lower bar shows only `density: lit L px, pass 2 plan N ms
(nodes .., reads .., cell dots ..)`, the whole tag being in the log line and the
bar's tooltip - `floe.gui.perf_status`). `lit` (since 0.12.264, the frame line's
`density_stack` lit count) is every pixel pass 2 lit - the dots standing for the
cells under the cut and the shapes under the cut it draws from pages by their
area, which look like dots too; `cell dots` (`dot items` until 0.12.263) counts
only the cells' dot items, so a view with no cell under the cut lights its
shapes with `cell dots 0`.
`FLOE_RUST_DENSITY_ONE_WALK=on` (diagnostic; the default in 0.12.259-0.12.260,
opt-in since 0.12.261 - its small-shape pages as dots were far denser than their
shapes and a field root view drew 13.8 s against 7.9 s) makes pass 2 plan once
instead of the floor probe and the budget fit, which walk the view up to five
times on the first frame at a scale: its pages at the cells' cut - pass 1's, in
hand, nothing more decoded - its records at `FLOE_RUST_DENSITY_FLOOR_PX` (there
the records' floor), a page all under the cut a dot item. The frame line's
`density_plan2=probe_us/fit_us/probes/passes/regions/nodes/page_nodes/page_candidates`
breaks pass 2's plans down (the log line's tag shows it after `pass 2 plan`,
the bar its nodes, reads and cell dots, and a probe, a fit past one pass with
the floor it raised and threads when there are any); since
0.12.262 it ends `/threads/reads/items` - the threads its regions were planned
on, the placement reads and the cells' dot items; since 0.12.266 it ends
`/probes_over/thinned` - the floor probes past pass 2's reserve and the sides
whose plan its budget fit thinned (the bar says `pass 2 over budget: floor probe,
thinned, N pages left out`, the last from `density_pages`). Pass 1's pages cost
that reserve nothing (`PlanRequest::free_pages`, floe_vfs `HierOpts::free_pages`;
user 2026-10-01: 37 pages of pass 1's, 201 MB by estimate, failed a 0 px probe
of the 128 MB reserve with nothing new to decode). Pass 2's regions are dealt
round robin to bands planned on threads - four, or the cores there are, by
default since 0.12.268 (user 2026-10-02: MAIN01's `ltv_top_RTG` fit view went
from over 6 s to under 4 on four threads); `FLOE_RUST_DENSITY_PLAN_THREADS=N`
(diagnostic) sets them, 1 one plan (the kill switch; the default until
0.12.267) - merged (`Cache::merge_plans`) and the merge fitted to the reserve
as the one plan would be (`Cache::fit_plan`, floe_vfs `fit_planned`; since
0.12.267 - before, only a view the reserve held whole took the threads); a fit
that would plan again - a decision at a coarser cut, pages past the fit's
overshoot - plans as one. Since 0.12.268 `density_plan2` ends with the cells'
dot items by where they came from - `/by_nodes/by_placements/by_arrays/
by_list_members/by_list_chunks/by_chunk_members/by_array_members/by_pages` - and
`/map_updates`, the dot block updates past the cells' grids (22 values; the log
line's tag shows them after `cell dots N [...]`, the nonzero ones). A cell's dot
blocks are kept in a dense grid over its view (floe_vfs `HierOpts::dot_grid`,
items of one block in a row summed, a point list's topmost layer found once and
a chunk of it in one block counted at once): the hash map's counts, unions and
order for less (the synthetic chip's `H01_00001` fit view: pass 2 planned
431-450 -> 376-400 ms on four threads, 768-771 -> 657-662 on one, the frames
byte for byte alike); `FLOE_RUST_DENSITY_DOT_GRID=off` is its kill switch.
Since 0.12.269 a plan of some regions (the threads' bands) walks a point
list's members by each of the cell's boxes, not by their bounds - the regions
dealt round robin span the view, so every thread counted every member of a
list across it (field 2026-10-02: `list members 86.6M`; a million random
vias, 1 / 2 / 4 threads counted 1 / 2 / 2.96 M) - and puts out only the
blocks a box holds whole (floe_vfs `dot_block_whole`), so the merge never keeps
a block counted in part; `Cache::merge_plans` looks its blocks up with the
planner's Fx hasher. `FLOE_RUST_DENSITY_DOT_BOXES=off` is the kill switch.
Since 0.12.270 pass 2's reserve in a frame is the fixed one (pass 1 plans to
leave it) or, when larger, what pass 1 left of the generation budget - its
pages' bytes taken off (`density_frame_reserve`; user 2026-10-02, the field
chip at depth 0: a root's own shapes under a pixel failed the 0 px floor's
probe of the 128 MB reserve while pass 1 held a few pages, and the view drew
nothing under the cut). The probe, the fit, the threads' fit and the decode
take it; `density_plan2`'s 23rd value is it in MB (`reserve_mb`, the log line's
`reserve R MB`). `FLOE_RUST_DENSITY_RESERVE_LEFT=off` is the kill switch.
Since 0.12.271 pass 2's budget fit is at the asked cut (floe_vfs
`HierOpts::dot_fit_at_cut`): one complete plan, its pages kept by priority to
the reserve, the cells' cut - pass 1's - never raised, and the threads' merged
plan fitted alike (field 2026-10-02: the top cell at depth 1 took `fit 75019 ms
x6 passes on 1 threads` up the cut ladder, the dots walked again each step).
`FLOE_RUST_DENSITY_FIT_LADDER=on` is the kill switch.
Since 0.12.277 the page spread is on by default where the index has
`design.ovb` (an index without it, or a page without a record, is not spread -
as before); `FLOE_RUST_DENSITY_PAGE_SPREAD=off` is the kill switch and `=on`
also spreads a page without a record over its box (`HierOpts::
dot_page_spread_boxes`), and pass 2 draws a decoded page's shapes under the
floor only where the spread is in effect. As 0.12.272 introduced it,
`FLOE_RUST_DENSITY_PAGE_SPREAD=on` (diagnostic, then off by default) draws
a page whose every shape is under the records' floor on both sides as its
shapes' dots without decoding it - over its box when it is wider than a box,
each block of the view taking its part's share, rounded by the block's dither
(floe_vfs `HierOpts::dot_page_spread`): even content keeps its look, lines of
shapes become dots spread over their pages. Since 0.12.274 a page whose
occupancy grid the cache holds (`design.ovb`, docs/SPEC-FORMATS.ko.md: which of
64 x 64 cells over its box hold a shape, written by the indexer unless
`--no-page-occupancy`) spreads over those cells only, an even share each, and
a page no wider than a box stands at their bounds (`HierOpts::dot_page_occ`;
user 2026-10-02: the spread "filled places where nothing is").
`FLOE_RUST_DENSITY_PAGE_OCC=off` is the kill switch; a cache without the file,
or with one built for another index (its pages, source size and mtime, ovp
length), spreads over the boxes. `density_plan2`'s 24th value is the pages so
placed (`occ_pages`, the log line's `pages N (M by occupancy)`). Since 0.12.275
the file holds each cell's covered area as a level (design.ovb v2: 4 bits, a
factor of two apart, deflated per page; a v1 file is not attached) and a
cell's dots are that area in px^2 - what the area-true raster lights - not an
even share of the page's members at its largest shape's area
(`HierOpts::dot_occ_cover`; user 2026-10-03, the routing chip's fill one zoom
step out: 0.3 um squares paged with 1 um array squares were drawn as dense as
the arrays); `FLOE_RUST_DENSITY_OCC_COVER=off` is the kill switch. With the
page spread on, pass 2 also draws a decoded page's shapes under the floor (its
raster's lower cut 0): a page decoded for its larger shapes no longer leaves
its small ones blank where a spread page draws theirs by area;
`FLOE_RUST_DENSITY_UNDER_FLOOR=drop` is the kill switch. Since 0.12.278 a page
under the floor whose occupancy cells would show past 4 px
(`FLOE_RUST_DENSITY_OCC_CELL_PX`, diagnostic) is decoded and drawn rather than
spread - a coarse cell's dots landed where no shape is (user 2026-10-03, the
field chip) - its spread's dots standing in when the budget fit leaves it out
(`HierOpts::dot_occ_decode`; `FLOE_RUST_DENSITY_OCC_DECODE=off` is the kill
switch, and UNDER_FLOOR=drop turns it off too); `density_plan2`'s 25th value
is the pages so decoded (`occ_decoded`). Since 0.12.294 such a page keeps
aside the dots of the blocks the cell's boxes hold whole, as a cell's own
dots are kept (`HierOpts::dot_occ_boxes`; `FLOE_RUST_DENSITY_OCC_BOXES=off` is
the kill switch: every block in view) - pass 2's regions dealt to four
threads each kept a dense page's every block in view aside, and the fit
sorted four times them. Since 0.12.280 a point list's chunk
(256 members in Morton order) whose blocks are all at their cap is passed over
unread - where its members are is the Morton run between its first and last,
in aligned squares (`HierOpts::dot_list_full`;
`FLOE_RUST_DENSITY_LIST_FULL=off` is the kill switch) - a dense chunk is read
every step-th member, each standing for the step, at least 16 for a block's
area of its run (`HierOpts::dot_list_sample`;
`FLOE_RUST_DENSITY_LIST_SAMPLE=off`), and the members read are counted into
their blocks run by run (`HierOpts::dot_list_fast`;
`FLOE_RUST_DENSITY_LIST_FAST=off`) - user 2026-10-03, the field chip with all
449 layers: `list members 765.7M`, pass 2 planned 32 s. Only the members read
count toward a list's SUB_CUT_BOX_ARRAY_MAX. `density_plan2` ends with the
chunks passed over and their members, and the chunks sampled and theirs
(29 values). Since 0.12.295 a list member standing for a share of a dot
(under half) is read one in the window of members that make a dot at most -
a power of two, a chunk at most, picked in the window by a dither of the
chunk, standing for the window's members (`HierOpts::dot_list_by_dot`;
`FLOE_RUST_DENSITY_LIST_BY_DOT=off` is the kill switch; user 2026-10-04, the
field chip: lists of dozens of vias, `list members 59.1M`, under the 64 a
chunk's step needs); the chunks count as sampled. Pass 2's floor probe plans
on the threads its fit does, the bands merged - over when a band or the
merge's pages pass the reserve (renderd `density_probe_threads`;
`FLOE_RUST_DENSITY_PROBE_THREADS=off` is the kill switch: the probe as one
plan, which is the plan when it holds - the field's 7.3 s on one thread). The raster
takes i64 and shifts where they give the i128 arithmetic's values: floor
and ceil divisions by a power of two, the orthogonal transform's rows, device
coordinates and widths within i64 (render-core `fast_arith`;
`FLOE_RUST_FAST_ARITH=off` is the kill switch; frames byte for byte alike,
the density raster 8-14 % faster on the synthetic chips).
Since 0.12.281 a whole list whose blocks are all full is passed over too
(`blocks_full`, the grid's 8 x 8 squares of full whole blocks), and pass 2's
regions are cells of the free space rather than a tile's bounding box of it
(a reviewer, 2026-10-03: a tile 1 % free spanned the tile, and the joint plan
was decided by those boxes' area): 32 px cells
(`FLOE_RUST_DENSITY_FREE_CELL_PX`, diagnostic) on a grid anchored to the world
- kept by a pan by whole pixels - counted over the frame whatever the tiles
(since 0.12.284), the top plane's where any pixel is free, the others' where
an eighth of the cell's part in the frame is (`FLOE_RUST_DENSITY_OTHERS_MIN`,
diagnostic; fewer: the originals show the cell), their runs cut by the tiles
for the threads, the two sides planned apart - no joint plan of every layer
over the whole top space.
`FLOE_RUST_DENSITY_FREE_CELLS=off` is the kill switch (the tile boxes, the
joint plan by area). `density_plan2` ends with the free pixels of the cells
planned, the top plane's and the others' (31 values). Since 0.12.283 the top
plane is the topmost visible layer the view's cell holds shapes of within the
depth (user 2026-10-03; renderd `density_held_top`, render-core
`Cache::layer_held` - the planner's cell_bits rule, a layer at a time from
the top, through the child-BVH nodes' masks, held past 2^18 placements read):
visible layers above it with neither shapes nor texts there leave the density
stack's planes (they draw nothing at that depth). `FLOE_RUST_DENSITY_TOP_HELD=off` is the kill
switch (the topmost visible layer, shapes or not). Since 0.12.286 a sub-cut
member under a pixel stands for its area in dots - a fraction kept in whole
dots by a dither of its block and itself - not one dot (user 2026-10-04, the
routing chip's fit view: 0.02 px VIAs a dot each lit a quarter of the frame
where their area is 0.03 %; `HierOpts::dot_area_share`,
`FLOE_RUST_DENSITY_AREA_SHARE=off` is the kill switch); from a pixel up as
before. Since 0.12.287 a frame zoomed out past the viewer's fit view of its
die (the plan's top cell's box, the view root or the layout's top, and the
viewer's 5 %) thins pass 2's dot items by (the fit's scale / the frame's)^power
- each keeps floor(count x gain + a dither of its cell and box) dots, none
dropped - decoded shapes as they are (user 2026-10-04: "draw it sparser the
more it is zoomed out"; renderd `density_zoom_gain`, `thin_dot_items`;
`FLOE_RUST_DENSITY_ZOOM_OUT=off` is the kill switch,
`FLOE_RUST_DENSITY_ZOOM_OUT_POWER` (1) diagnostic). A margin's render command
carries its viewport, `vw=`/`vh=`, the fit view being the viewport's.
Since 0.12.298 a render command may carry `density=on|off`, the viewer's
density toggle (View > density under the cut, `v`, `floe2 view --density`;
user 2026-10-05: "add a density on/off option to the viewer"): on draws the
density stack with the sub-cut dots (unless `FLOE_RUST_DENSITY_DOTS=off`),
off draws no pass 2; without it `FLOE_RUST_DENSITY_STACK` and
`FLOE_RUST_DENSITY_DOTS` decide as before (renderd `density_stack_on`,
`density_dots_on`). The setting is part of a retained frame's key
(`RetainedKey::density`): without it a frame retained without the density
would serve a request with it whole (the label-only path) - toggling at one
view would draw no density.
`density_plan2`'s 32nd value is the gain in thousandths. Since 0.12.288 a
cell's dot block of one layer is drawn only when its dots light a share of
its pixels set by the detail's cut - (cut - 1) / 16: high (1 px) none, medium
(3 px) 2 of a 4 px block's 16, low (5 px) 4 - too sparse, neither its dots nor
the pixels they stand for; decoded shapes are drawn where they are, not gated
(user 2026-10-04: zoomed out, the routing chip's wires spread over their
blocks "fill space where nothing was"; "a pixel should light only when the
shapes' size in it passes a level"; floe_vfs `HierOpts::dot_gate`,
`FLOE_RUST_DENSITY_GATE=off` is the kill switch,
`FLOE_RUST_DENSITY_GATE_SHARE` diagnostic). `density_plan2` ends with the
blocks left out and the dots a block needed (34 values; since 0.12.297 a
35th, the brightness's gain in thousandths, 0 when off). Since 0.12.290
pass 2 decodes the top plane's side's pages before the others' (they were
one list by distance from the view's centre under one reserve, and turning a
lower layer on took pages from the top plane: the routing chip's M8 over M1
under 256 MB kept 76 % of M8's pixels); the others take what is left (user
2026-10-04: "789's dots, lit alone, went with 787 on"; renderd
`density_top_first`, `FLOE_RUST_DENSITY_TOP_FIRST=off` is the kill switch).
Since 0.12.318 pass 1's budget fit keeps the pages top plane first as well
(user 2026-10-07, the field chip: with 7.59 and 14.367 on, 7.59 alone drew -
`none below x28.2` - its pages of larger shapes first in a fit by size alone):
the planes above the one the budget ends in whole at the asked cut, that one
by size class, none under it (floe_vfs `HierOpts::fit_rank`, renderd
`pass1_fit_rank` from the style list's order; the frame line's `fit_ranked`,
`fit_layers_whole`, `fit_layer_edge`, `fit_layers_out`; the status bar `top N
whole, L/D (...), K left out to fit budget`). `FLOE_RUST_FIT_TOP_FIRST=off` is
the kill switch: by size class alone, every layer at once.
`FLOE_RUST_DENSITY_ONLY=on` (0.12.291, diagnostic, off by default) draws the
density alone: pass 1 of a density frame reads no page and draws no shape
(its plan keeps the hierarchy), so pass 2's reserve is the whole budget - to
tell pass 1's budget from pass 2's own (user 2026-10-04: "789's dots still go
when 787 is on"); the viewer's density tag says `density only`. Since
0.12.293 the top planes are the topmost eight visible layers by the drawing
order - (layer, datatype) ascending, the last on top (user 2026-10-04:
"789.55 first, then 789.20, 789.0, 787.55, 787.20, 787.0, filling what is
empty"; 0.12.292 took the topmost layer number's datatypes): each is planned
on its own (a plan counts a sub-cut cell for its topmost layer alone), the
plans merged into pass 2's top side, and each draws its density over the
originals below it, the top first, kept out by the originals of the top
planes above it and its own (render-core
`GeometryRasterRequest::density_top_planes`); the layers under them in one
walk, where no original is (renderd `density_top_group`;
`FLOE_RUST_DENSITY_TOP_PLANES` sets how many, diagnostic;
`FLOE_RUST_DENSITY_TOP_GROUP=off` is the kill switch: the topmost alone).
Pass 2 decodes the pages in the drawing order down, each plane's after the
planes above it (`density_top_first`). Since 0.12.294 pass 1's shapes come
first: no plane's density, the top planes' neither, shows where an original
wrote or covers - the top planes, each still planned on its own, fill the
space left top first, and pass 2's top side plans that space alone (user
2026-10-04: "if pass 1 drew the shapes past the cut, density drawn only in
the space left will hardly jar"; render-core
`GeometryRasterRequest::density_shapes_first`, renderd `density_shapes_first`;
`FLOE_RUST_DENSITY_SHAPES_FIRST=off` is the kill switch: each top plane's
density over the originals below it, as 0.12.293). Since 0.12.299, under the
brightness on an index with design.ovb: pass 2's pages are cut at pass 1's
cut, so a page all under it is spread by its occupancy grid while the grid's
cells show no larger than `FLOE_RUST_DENSITY_OCC_CELL_PX` (4), whatever its
shapes' size - decoded past that or with no grid to go by, no floor probed
(renderd `density_ovb_first`, floe_vfs `HierOpts::dot_occ_first`;
`FLOE_RUST_DENSITY_OVB_FIRST=off` is the kill switch: the pages from the
density cut up decoded, as 0.12.298); a page a budget leaves out - the
planner's fit, or the frame's last check - is drawn by its occupancy record
instead (`HierOpts::dot_stand_in`, `Cache::stand_in_pages`;
`FLOE_RUST_DENSITY_STAND_IN=off` is the kill switch; `density_plan2`'s 36th
value counts the pages); and an item's cover keeps its fraction, a list
member read for a window standing over its block's part of the chunk
(`HierOpts::dot_bright_sums`; `FLOE_RUST_DENSITY_BRIGHT_SUMS=off` is the kill
switch). A reviewer 2026-10-05, checked against the exact cover: the routing
chip's fit view under 1 GB kept 110 of the 277 pages pass 2 decoded and drew
x0.49 of them; now x1.01 in a third of the time. Since 0.12.300 the
brightness no longer depends on the hierarchy the shapes are stored in: a
cell under the cut stands for the area its visible layers cover - its own
pages by design.ovb, its children by the hierarchy summary's member counts
(design.ovh, or one made in memory for a cache of 4 M placement records or
fewer), worked out on first use and ahead of it on a thread of its own - not
for its whole box (floe_vfs `cover::CellCover`, `HierOpts::cell_cover`,
`Cache::cell_cover`; `FLOE_RUST_DENSITY_CELL_COVER=off` is the kill switch,
`FLOE_RUST_DENSITY_COVER_WARM=off`, diagnostic, leaves it to the first use;
without design.ovb or a summary the box as before - `density_plan2`'s 37th
value says which, the viewer's tag `cell cover` / `cells by box`); a node no
wider than a box counts what its placements hold - all of them up to 32, past
that 16 of them, one in each equal run, each standing for its run - where it
counted its whole box once it had as many placements as sixteenths
(`HierOpts::dot_node_sample`; `FLOE_RUST_DENSITY_NODE_SAMPLE=off` is the kill
switch, `FLOE_RUST_DENSITY_NODE_READ_ALL` / `FLOE_RUST_DENSITY_NODE_SAMPLES`,
diagnostic, the two counts); and an item across a block boundary is shared
between the blocks it meets by its area, an array's members likewise, a point
list's members past a quarter block put one by one (`HierOpts::dot_item_share`;
`FLOE_RUST_DENSITY_ITEM_SHARE=off` is the kill switch). The three off draw
0.12.299 byte for byte. Measured against the exact cover: a reviewer's 4,096
squares of 1 dbu as a cell each 121 px at full colour -> 0 px, as flat; the
standard-cell layout's 2/0 alone x6.9 -> x1.01; cells of mixed cover placed
one by one x1.36 / x0.85 on their sparse / dense halves -> x1.01 / x1.01, 4 px
blocks corr 0.88 -> 0.99. Since 0.12.297 the
density is a brightness: a pixel shows min(1, g x the area pass 2's shapes
cover in it) of its plane's colour - never past it - the planes over one
another, the top first, g by the detail, 2^((5 - cut) / 2): low (5 px) 1,
medium (3 px) 2, high (1 px) 4; it replaces the dots' gate and the zoomed-out
gain (user 2026-10-05: "the brightness of a pixel by the shapes' size that
gathers on it", "never brighter than the original colour", "go on with g = 1,
2, 4"; renderd `density_bright_gain`; floe_vfs `HierOpts::dot_bright` - a
block counts the area its content covers in 1/16 px^2, at most its area over
g, none gated, an item's box what it stands for; render-core
`GeometryRasterRequest::density_bright` - a rectangle adds its overlaps, a
dot item its count over its box, a lattice array its column shares times its
row shares at once; `FLOE_RUST_DENSITY_BRIGHT=off` is the kill switch: the
dots as lit pixels, as 0.12.296; `FLOE_RUST_DENSITY_BRIGHT_GAIN`, diagnostic,
fixes g). A density pixel stays open: the frame bands under the planes paint
it and show through what the density leaves, composed at the tile's end.
`FLOE_RUST_DENSITY_SPREAD=off` is the spread's kill switch: a block's dots as a
compact box whose area is their count, every item by its box (with
`FLOE_RUST_DENSITY_BLOCK_PX=4`, the frames of 0.12.256); by default a block's
dot item is what its dots stand for within the block, its count carried apart
(`WsCell::dot_counts`) and lit exactly, spread over it. The adapter adds
`density_round: True` to that refining result and the viewer's status says
"drawing the density under the cut...". The pass-2 frame line also carries
`density_dots=items/over`.
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
`frame_cache=0` bypasses lookup and insertion for that request without
disabling the decoded-page LRU, which keeps warm backend comparisons focused
on scene/raster/PNG work.

The rasterizer uses independently owned 2D tile scratch buffers and dynamic
atomic work assignment. Tile completion order cannot affect pixels because the
coordinator copies every tile to its fixed framebuffer position. On the styled
1200x800 `valmini` view, 64/128/256px tiles and 1/4/8 workers all match the
former band renderer byte-for-byte. With 128px tiles, five-run median
`raster_us` was 94.733/27.511/23.083ms for 1/4/8 workers (3.44x and 4.10x).

Per-tile walk instance index (0.12.258; field 2026-10-01). Past the work bin's
item cap (`bin off(cap@786k)` in the status line), for a binned cell item and in
the density stack's lower-plane walk, each tile walks the hierarchy itself,
plane by plane. The plan lists a working cell's instances for the whole frame,
and that walk read every one of them per tile and plane to keep those meeting
the tile: a per-tile cost of the whole frame, so a frame's cost grew with the
square of its area. In the field a 2076x1232 frame drew in 21.4 s against
1.75 s for 796x804 at the same scale - 4x the pixels, 12x the time; the
instances read and pruned (`hier N/M pruned`) were 884.7 M against 83.8 M,
10.6x = 2.7x the tiles times 4x per tile. `FrameScene::inst_index` now buckets
the footprint of every instance of a cell of 64 or more - the union of its
members' boxes in the cell's frame (a grid's four corners, every point of a
list), what `for_each_visible_offset` meets the view with - on a grid built the
first time a tile asks; a tile walks the instances whose footprint meets its
view, in their order (a view over half the grid or more walks the list). An
instance it skips has no member box in the view: the old walk drew nothing of
it there, and frames are byte-identical. Kill switch `FLOE_RUST_INST_INDEX=off`
(diagnostic). Synthetic 1/10 chip, all layers, 1 um/px, the per-tile walk
forced (`FLOE_RUST_WORK_BIN=off`), warm: raster 9.1 -> 2.7 ms at 796x804 and
139.3 -> 13.8 ms at 2076x1232 (instances read 1.38 M -> 170 k and 13.8 M ->
636 k: 4x the pixels now read 3.8x); with the density stack and dots 11.8 ->
8.7 ms and 149.5 -> 24.8 ms; the 7 standard dots views (work bin on) hash as
before. A grid's own member range rounds outwards (`grid_ranges`), so the old
walk also visited a member just past the view; it draws nothing there unless
the child draws past its own box (a sub-cut dot box grown about a lone item at
a cell's edge can), which the bin path already leaves out of the tiles that
box misses.

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

Since 0.12.301 a frame whose pages' decoded charge passes the generation
budget is no longer an error (field 2026-10-05: two layers alone with the
density off failed most frames with `decoded generation budget exceeded:
1093017130 > 1073741824 bytes`). The planner fits a frame's pages to the
budget by an estimate of their decoded size; the charge of some pages was
past it - a page's record lists kept the room the parser grew them to, up to
twice their length, and a repetition list shared by a page's records was
charged once a record. Now a decoded page's record lists are cut to their
length (`FLOE_RUST_DECODE_SHRINK=off` is the kill switch: 0.12.300's charge) -
a rectangle page 124-217 B -> 120-124 B a record, no page of the synthetic
chips past its estimate - and a shared list is charged once
(`FLOE_RUST_CHARGE_SHARED=off`), so the fitted plan is drawn whole. The
density stack counts its pages as the parser read them
(`FLOE_RUST_DENSITY_AS_READ=off` counts them as held, and pass 2 has what the
cut lists free: the synthetic MAIN01 1/10 at full depth 8-20 % slower for the
same picture), so a density frame is the picture it was. Should a frame pass
the budget all the same, it is planned anew under the budget over what its
pages took, remembered for its layer set, depth, root and the density stack
on or off (`FLOE_RUST_BUDGET_REFIT=off`: the error, as 0.12.300; the frame
line's `fit_scale=` / `fit_refits=`, the viewer's `pages xN their
estimate`), and where no plan holds it draws the pages the budget holds and
reports the rest (`N pages over budget (not drawn)`). The viewer's margin
frame is not planned anew: it answers `dropped gen=N reason=budget`, and the
layers' remembered fits are forgotten with the raised scale, so the next
viewport frame decides over its margin's extent under it. An exact frame and
one the planner does not fit - no cut (an export at cut 0), or
`FLOE_RUST_FIT_BUDGET=off` - fail as before. Every frame that drew before
draws the same bytes.

Since 0.12.302 the fit of a new scale is decided by a plan made for that
alone (field 2026-10-05: the first frame at a scale was slow, the time under
`other`). The first frame at a scale plans the extent the viewer's margin
frame would have - twice the view a side - to decide the scale's budget fit,
and that plan walked every placement of the extent, most of it outside the
view, to keep its decision alone; its time was no phase's. Now it goes by
the hierarchy summary (design.ovh, or the one the daemon makes in memory): a
cell the extent covers whole takes its children from the summary and its
placements are not read; in a cell covered in part the walk looks only for
the children the summary leaves open and ends once each has a member whole
within the extent. The pages and the decision are the walk's - 975 first
frames compared, pixel for pixel - and with nothing of the index in the page
cache the first frame zoomed in 16 times on the synthetic MAIN01 1/10 takes
279-287 ms for 551-566 (the probe 63 for 336). `FLOE_RUST_FIT_PROBE_SUMMARY=off`
is the kill switch; a cache of more than four million placement records
without design.ovh walks as before (`floe-index hier` adds it).
`FLOE_RUST_FIT_PROBE_CHECK=on` (diagnostic) plans both ways and prints
`fit probe check: the same|DIFFERENT`. The frame line carries
`fit_probe_us=` and `fit_probe_walk=`; the status line shows `+ N fit probe`
from 100 ms and the log line `, fit probe N ms` (` (walk)` without a
summary).

The operational knobs are `FLOE_RENDERD_BIN`, `FLOE_RUST_JOBS` (page decode,
default up to 8 host CPUs), `FLOE_RUST_RASTER_JOBS` (default up to 4 and never
above decode jobs), `FLOE_RUST_BUDGET_MB` (1024), `FLOE_RUST_ROUND_PAGES` (1024),
`FLOE_RUST_TILE_PX` (384), and `FLOE_RUST_LABEL_PX` (14, whole-pixel range
6..96). The raw daemon fallback remains one shared jobs value and 128px when
the new fields are omitted. `FLOE_RUST_OPEN_TIMEOUT_S` and
`FLOE_RUST_CLIP_TIMEOUT_S` both default
to 300 seconds. Cold cache open runs outside the GTK thread, and the daemon is
placed in its own POSIX session so terminal SIGINT does not kill it.
For backend-neutral GUI timing, both products accept `--refinement off` and
`--frame-cache off`; `--perf-baseline` additionally disables LOD, hierarchy
frames, and labels. Rust maps refinement-off to one practical all-miss batch,
while frame-cache-off bypasses only the exact settled PNG LRU and preserves the
decoded-page cache.
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
