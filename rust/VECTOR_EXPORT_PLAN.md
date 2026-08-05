# FLOE viewport vector export plan

Status: design proposal for a later implementation  
Working format name: **FVX** (`.fvx`, FLOE Vector eXchange)  
Last reviewed: 2026-08-06

## 1. Purpose

Export the design geometry intersecting the current viewport into a compact,
standalone artifact for a dedicated viewer. The viewer must be able to turn
layers on and off without loading or decoding disabled layers.

This is a visualization format, not an archival replacement for OASIS. Use an
OASIS clip when exact manufacturing geometry or interchange with layout tools
is required.

The export may contain tens of millions of expanded polygon members. Therefore
the format and exporter must:

- preserve hierarchy and repetition instead of flattening every member;
- filter detail according to the deepest zoom the artifact promises to
  support;
- partition payload by layer and space;
- support bounded, progressive decoding in the dedicated viewer;
- reject or deterministically coarsen an export that exceeds its configured
  budget.

## 2. Scope

### 2.1 Required in the first version

- Geometry from the requested world-coordinate viewport.
- Rectangles, polygons, paths and repeated placements needed to draw it.
- One line color and one fill color per layer.
- Layer number, datatype, display name and initial visibility.
- Independent layer on/off control.
- Per-layer spatial chunk index.
- Grid and Pts repetitions without unconditional expansion.
- A declared fidelity limit describing the deepest supported zoom.
- Deterministic output and explicit resource-limit reporting.

### 2.2 Deliberately excluded

- Design text and generated block labels.
- `FRAME_LAYER` and hierarchy frontier outlines.
- Rulers, selection outlines, goto/DRC markers and other GUI overlays.
- Pixel-identical reproduction of the PNG renderer.
- Calibre/KLayout stipple phase compatibility.
- Editing, picking metadata and source cell names in FVX v1.
- Conversion of an existing `.floe2` cache without reading its geometry.

The exporter should use `.floe2` metadata and planning, but it creates a new
artifact by decoding and filtering selected records. Reindexing the source is
not part of export.

## 3. Why not SVG

SVG is acceptable for small deep-zoom views, but it is not the primary target
for this feature. XML text size, DOM construction, per-path object overhead and
editor/browser parsing make a viewport with millions of distinct primitives
unreliable. Combining shapes into a single `<path>` reduces DOM node count but
does not remove coordinate or rasterization cost.

A binary format does not make truly distinct geometry disappear. Its benefits
come from compact integer encoding, preserved repetition, layer-local I/O and
spatially bounded decoding. Detail filtering or geometric aggregation remains
mandatory when the source really contains millions of distinct visible
shapes.

## 4. Fidelity contract

### 4.1 User-facing parameters

A future CLI may use the following shape:

```text
floe export-vector DESIGN.oas \
  --bbox X0,Y0,X1,Y1 \
  --out view.fvx \
  --detail-view-width-um 50 \
  --reference-width-px 1920 \
  --detail-px 1.0 \
  --max-expanded-primitives 5000000 \
  --max-output-mb 512 \
  --budget-policy fail
```

The GUI supplies `--bbox` from the current viewport. If the exported viewport
is 100um wide and `--detail-view-width-um` is 50, the artifact promises useful
geometry through a further 2x zoom.

`--detail-view-height-um` and `--reference-height-px` may be accepted for an
explicit non-matching aspect ratio. Otherwise height is derived from the
exported viewport aspect ratio.

### 4.2 World-coordinate cut

Let:

```text
spp_x = detail_view_width_dbu  / reference_width_px
spp_y = detail_view_height_dbu / reference_height_px
spp_detail = max(spp_x, spp_y)
cut_dbu = ceil(detail_px * spp_detail)
```

After applying the placement transform, a geometry record may be omitted when:

```text
world_bbox_width < cut_dbu AND world_bbox_height < cut_dbu
```

The strict `<` comparison matches the existing VFS cut convention. A long,
thin wire survives even when its width is subpixel because it is still visible
as a line. An option that removes thin but long objects would be a different,
explicitly lossy policy and must not silently replace this rule.

The manifest records all inputs and the actual `cut_dbu`. A viewer zooming
beyond the declared detail view must show that it is beyond the artifact's
fidelity limit; it must not imply that missing geometry is an empty design.

### 4.3 Filtering granularity

VFS cell and page cut is useful for planning but is insufficient for export.
A selected page can contain one large record and many records below the cut.
Copying that OASIS page would bring all small records back.

The export path therefore performs:

1. VFS hierarchy, visible-layer, page-BVH and repetition-range planning.
2. Decode each selected unique exact page once.
3. Apply record-level transformed-bbox filtering.
4. Preserve or slice the visible repetition rather than expand it.
5. Encode the retained records into FVX spatial chunks.

The export viewport is a culling boundary. V1 does not need to geometrically
clip every polygon at the viewport edge: retaining an intersecting primitive
and using a viewer scissor avoids topology changes. Oversize records are stored
once in a per-layer oversize run rather than copied into every touched chunk.

## 5. Resource budgets

### 5.1 Separate storage and rendering costs

The exporter reports at least:

- encoded geometry records;
- unique symbols/pages;
- expanded primitive/member estimate;
- Grid and Pts repetition counts;
- uncompressed and compressed payload bytes;
- counts per layer;
- requested and actual cut.

One Grid record may encode a million members cheaply but still cost the viewer
a million draws. Consequently `encoded_records` and `expanded_primitives` are
separate budgets.

Recommended safety options are:

```text
--max-records
--max-expanded-primitives
--max-output-mb
--budget-policy fail|auto-coarsen
```

Expanded counts must use checked wide arithmetic and saturate only for
reporting. Overflow is an export error, never a wrapped small count.

### 5.2 Budget policy

`fail` is the default. It writes no final artifact and reports the dominant
layers and record/repetition types.

`auto-coarsen` raises `cut_dbu` using a deterministic search until every hard
budget is satisfied. The manifest records both requested and actual fidelity.
If long thin records keep the export above budget, coarsening with the normal
two-dimensional cut may not converge. In that case export fails and reports
why; it must not silently discard those records.

Never implement a "first N polygons" limit. Traversal-order truncation creates
spatial holes, biases layers, can depend on job count and makes an apparently
empty region indistinguishable from missing data.

## 6. Proposed FVX layout

FVX may initially be a single file with an offset table. A directory form is
acceptable for debugging, but the reader contract should be identical.

All integers are fixed little-endian or explicitly encoded varints. Readers do
not cast mapped bytes to Rust `repr(C)` structures. Every offset, count and
range is bounds checked.

```text
header
manifest
string table
layer directory
symbol directory
placement/repetition directory
per-layer spatial index
chunk directory
compressed geometry chunks
optional checksums
```

The header contains magic, version, file length, section offsets/counts, DBU,
export bbox and a manifest checksum. Publication uses temp-write, flush and
atomic rename. A partially written file is never accepted as an FVX artifact.

### 6.1 Manifest

The manifest contains:

- source fingerprint, without embedding the source path by default;
- export viewport and DBU;
- requested and actual detail parameters;
- deepest supported view width/height;
- reference pixel dimensions;
- all resource counts and whether auto-coarsening occurred;
- encoder and schema versions;
- compression codec;
- deterministic build options.

No design text, text strings or source cell names are needed in v1.

### 6.2 Layer directory

Each layer entry contains one copy of:

```text
layer number / datatype
display name
line color / fill color
default visibility
spatial-index root or range
chunk range
oversize-record range
record and expanded-member counts
```

Payload is physically grouped by layer. Disabled layers therefore require no
chunk reads or decompression.

### 6.3 Geometry command stream

Use specialized records instead of converting everything to generic polygons:

```text
RECT x0 y0 x1 y1
POLYGON point_count, delta-coded points
PATH width, point_count, delta-coded points
INSTANCE symbol_id, transform
GRID_INSTANCE symbol_id, na, nb, va, vb, transform
PTS_INSTANCE symbol_id, count, delta-coded offsets, transform
```

Coordinates remain integer DBU in v1. Zig-zag varints and chunk-local origins
make nearby coordinate deltas compact. Collinear duplicate vertices may be
removed, but topology-changing simplification and coordinate quantization are
deferred until separately specified and oracle-tested.

### 6.4 Symbols, hierarchy and repetition

Hierarchy is an encoding optimization even though the dedicated viewer does
not expose a cell tree. Store one symbol definition and repeated placements
rather than flattening every use.

Grid repetitions keep `na`, `nb`, `va` and `vb`. The viewer computes the
visible index rectangle analytically, including skew grids. Pts repetitions
use chunked Morton-ordered offset pools and visible-subset lookup. If a
repetition spans many spatial chunks, store it once in the layer oversize index
instead of duplicating the record.

The same Pts rebase and boundary-ownership invariants used by `.floe2` apply.
Export must have tests for rotations, mirrors, negative/skew vectors and points
on a viewport/chunk boundary.

### 6.5 Spatial chunks

The logical lookup key is:

```text
(layer_id, spatial_chunk_id, lod_level)
```

FVX v1 may have only one LOD level. Chunks are independently compressed and
carry bbox, offset, compressed/uncompressed lengths, record count, expanded
member estimate and checksum. A packed per-layer BVH or R-tree maps a viewport
to chunk ranges.

Start with approximately 256KiB to 1MiB decoded target size and tune from
measurements. Chunk construction is deterministic and independent of worker
count. Compression should initially reuse the repository's vendored deflate
path; zstd requires an explicit offline-vendoring decision.

## 7. Dedicated viewer data flow

The viewer opens and validates the header/directories with mmap, but faults in
payload only for enabled layers and visible chunks.

```text
viewport + enabled layers
  -> per-layer spatial-index query
  -> visible chunk and oversize-record selection
  -> bounded parallel decode
  -> repetition visible-range calculation
  -> tessellation/draw command generation
  -> render
```

Decoded chunks and tessellation results use bounded LRUs. Generation IDs cancel
stale decode and draw work during pan/zoom; completed stale chunks may remain in
cache. Progressive drawing should interleave spatial regions and layers rather
than finish one huge layer before showing the rest.

The renderer technology remains an implementation decision. A GPU renderer is
preferred for millions of members, but pre-tessellated triangles should not be
the storage default because they enlarge files and lose compact rectangle/path
representations. Tessellate visible chunks on demand and cache the result.

## 8. Multi-LOD extension

One fixed detail level is sufficient for the first implementation. If a 100um
overview and a 10um inspection view differ too much in density, extend the same
chunk key with two or three LOD levels.

Valid later operations include:

- subpixel record removal;
- collinear point removal;
- layer-local adjacent rectangle union;
- explicitly specified coordinate-grid snapping;
- optional occupancy/coverage proxies for overview-only levels.

Coverage proxies are not exact geometry and must be marked as such. They are
not part of FVX v1 unless measurement shows that detail filtering and preserved
repetition are insufficient.

## 9. Implementation stages

1. **Probe/export estimate**: reuse the VFS planner and report retained pages,
   records, expanded members and estimated bytes for a bbox/detail request.
   Write no FVX payload yet.
2. **Flat geometry prototype**: layer-local rectangle/polygon chunks, manifest,
   checksums and a diagnostic decoder. Gate on small fixtures.
3. **Repetition-preserving encoder**: symbols, transforms, Grid, Pts and
   oversize ownership. Confirm that repeated fixtures scale with records rather
   than expanded members.
4. **Spatial viewer**: mmap directories, layer toggles, per-layer BVH, decoded
   chunk LRU and progressive drawing.
5. **Budgets and auto-coarsen**: deterministic preflight, failure reports and
   manifest fidelity declaration.
6. **Large-chip gate**: closed-network measurements on the 9.8GB design and a
   smaller synthetic fixture with the same repetition/density pathologies.
7. **Optional multi-LOD**: only if a single fidelity level misses measured
   open/pan/zoom targets.

The exporter and viewer should initially be additive commands/components. They
must not change `.floe2`, the operating viewer or OASIS clip behavior.

## 10. Validation gates

### 10.1 Correctness

- Layer-by-layer comparison against a filtered KLayout/source oracle at the
  declared detail view.
- Retained geometry has zero transform, repetition and layer-assignment errors.
- Omitted geometry satisfies the documented cut rule.
- Grid/Pts repetitions, rotations, mirrors and negative/skew vectors pass.
- Chunk and export boundaries have single ownership with no gap or duplicate.
- Hidden layers cause zero payload decode in the dedicated viewer.
- Output bytes are identical across job counts.

Pixel XOR with the PNG viewer is not a gate because fill patterns, text,
frames and overlays are intentionally different.

### 10.2 Robustness

- Corrupt offsets, counts, varints, checksums and compressed lengths fail with
  a clear rebuild/re-export error and never panic.
- Interrupted export leaves no valid-looking final file.
- Resource counters use checked arithmetic.
- Unknown mandatory record types fail closed; optional sections are skipped by
  declared length.

### 10.3 Performance measurements

Record at least:

- plan/filter/encode/compress wall time and peak RSS;
- output bytes by section and layer;
- records and expanded members before/after filtering;
- viewer open time and metadata RSS;
- visible/decoded chunks and bytes per frame;
- decode, tessellation and draw time;
- cold first frame, warm pan and layer-toggle latency;
- cancellation count during continuous navigation.

Acceptance targets must be set after the probe stage. A small encoded file is
not sufficient if one compact repetition still expands to an unbounded draw
cost; storage and frame-time gates are both required.

## 11. Decisions to make before coding

- Final extension/name and whether distribution is one file or a directory.
- Dedicated viewer renderer technology and deployment targets.
- Whether paths remain paths or are converted to boundary polygons.
- Exact default budgets and decoded chunk target from real-chip probes.
- Whether coordinates remain exact DBU or a later lossy quantized mode is
  offered.
- Whether viewport-edge polygon clipping is ever needed.
- Whether auto-coarsen is exposed in the GUI or CLI only.
- Whether later LOD levels may contain coverage proxies.

These decisions do not block the estimate-only probe, which should be the first
implementation step.
