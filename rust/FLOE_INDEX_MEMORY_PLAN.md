# floe-index memory-bounded build plan

Status: first bounded-pipeline implementation on branch
`index-memory-bounded-pipeline`; 9.8 GB field gate pending

## Implementation status

| Milestone | Branch state |
| --- | --- |
| M0 | syntax/normalize/source-drop and five-second pipeline RSS heartbeats implemented |
| M1 | ordered page encoding bounded by `--encode-batch` and written directly to OVP |
| M2 | consecutive cell planning bounded by `--plan-batch`, then immediately encoded and released |
| M3 | fragmented Pts scratch reduced from copied 16-byte coordinates to 4-byte source indices |
| M4 | modal Pts uses shared storage; duplicate decomposition is shared; normalize results are retained for at most eight cells |
| M5 | OVM Pts intermediate copy removed; full temporary-file section streaming and optional skeleton remain pending |

The first production comparison should use the same 16-worker baseline with:

```text
floe-index vfs chip.oas .chip.oas.ice --jobs 16 \
  --plan-batch 2 --encode-batch 32
```

If cell-plan RSS remains above the host limit, rerun with
`--plan-batch 1`. If it is comfortably below the limit, compare 2, 4 and the
default (16) to recover planning throughput. `--encode-batch` bounds completed
payloads, not the number of workers; 32 is a conservative starting point for
16 workers and nominal 1 MiB pages.

`--page-target-mb N` changes the decoded page-size estimate used by spatial
partitioning. Its default remains 1 MiB. Larger values reduce indexing work
and page count but can increase off-viewport geometry per selected page, so
they are an explicit field-measurement knob rather than an automatic default.

## 1. Field baseline and objective

The 9.8 GB production OASIS run establishes the current live-set shape:

| Stage | Wall time | Observed RSS |
| --- | ---: | ---: |
| parse + repetition normalization | 95 s | 166 GB |
| build cell plans | 195 s | 443 GB |
| encode 876,166 pages | 175 s | 463 GB |
| viewer-side skeleton | 875 s | 355 GB |

The immediate objective is to make peak memory proportional to the parsed
document plus a bounded amount of planning and encoding work, rather than the
sum of the document, every cell plan, every repetition-fragment arena, and
every encoded page. The long-term objective is to remove the full in-memory
document requirement as well.

The output contract does not change:

- page order and page bytes remain deterministic at every `--jobs` value;
- `design.ovp` is written before the `design.ovm` commit marker;
- interrupted builds remain unusable and are rebuilt from the source;
- page/member conservation and KLayout XOR gates remain exact.

## 2. Current retention causes

1. The source is read into one `Vec<u8>`, CBLOCKs are inflated into another
   complete buffer, and explicit repetition points expand to `(i64, i64)`.
2. modal `Rep::Pts` values are cloned into records; grid normalization clones
   and sorts candidate point lists and retains every decomposition result until
   the apply pass.
3. every `CellPlan` is completed before any plan is consumed. Fragmented Pts
   records copy their complete point list into a per-cell arena, and placement
   Pts preparation creates another Morton-ordered list.
4. every page payload is encoded before `design.ovp` writing begins.
5. OVM finishing concatenates the Pts pool and then copies every section into
   a final output vector.

Thread reduction limits concurrent scratch but does not bound completed plans
or payloads. Backpressure must be expressed in bytes/batches, not only worker
count.

## 3. Milestones

### M0 - attribution and reproducible gates

- report RSS after source read, syntax parse, repetition normalization, source
  drop, plan batches, page batches, OVP completion, and OVM completion;
- report retained plan/page/fragment/arena/payload counts and estimated bytes;
- keep stdout reserved for protocols; all heartbeat output stays on stderr;
- add a deterministic synthetic asset with enough pages to exercise more than
  one encoding batch.

### M1 - bounded ordered page encoding

- encode at most `page_batch` jobs at a time in parallel;
- write each completed batch directly to `design.ovp` in global page order;
- append page-directory records at the same point and drop payload buffers;
- default batch size is derived from jobs but has a hard upper bound;
- preserve byte identity with the previous all-payload implementation.

This removes memory proportional to the complete `.ovp` payload.

### M2 - bounded cell-plan pipeline

- build a bounded consecutive batch of cell plans in parallel;
- consume plans in cell order, append metadata, encode/write their pages, then
  drop the plans and fragment arenas before building the next batch;
- accumulate layer stored-record totals while consuming plans;
- keep only graph-wide metadata (recursive bboxes, masks, ranks and counters)
  for the full build.

The first implementation uses a deterministic cell batch. A later byte-token
semaphore may replace the cell-count bound for designs dominated by one cell.

### M3 - Pts scratch reduction

- replace copied coordinate arenas with `u32` index permutations over source
  Pts where the source document must remain available;
- if viewer-side skeleton generation is disabled, allow the builder to consume
  cells and move uniquely owned Pts storage instead;
- prepare placement Pts in bounded cell batches and release keyed Morton
  scratch before advancing.

### M4 - parse/normalize live-set reduction

- distinguish syntax-parse RSS from normalize RSS;
- share modal Pts storage (`Arc<[Point]>` or a document Pts pool) instead of
  cloning the full vector on reuse;
- normalize owned records in bounded order and apply each result immediately;
- avoid retaining all grid-decomposition results;
- evaluate source mmap/streaming and CBLOCK-to-temporary-spool parsing after
  the in-memory duplication is removed.

### M5 - streaming metadata commit and optional viewer-side assets

- write OVM sections to a temporary file without concatenating the Pts pool or
  creating one final metadata vector, then atomically install the marker last;
- make skeleton generation optional or derive it from a lightweight spool so
  page building can consume and release source geometry.

## 4. Gates

Routine gates:

- `cargo fmt --check`;
- `cargo test --workspace`;
- `sh tools/validate_rust.sh`;
- identical `design.ovm`, `design.ovp`, skeleton and sidecar bytes at jobs 1
  and a multi-worker run for deterministic fixtures;
- marker fault-injection recovery remains green.

Memory/performance gates on the 9.8 GB asset:

- M1 retains no complete payload vector; encode live bytes are bounded by the
  configured page batch;
- M2 build peak is at most `max(parse_peak * 1.5, 256 GB)` as an intermediate
  gate, with a final target below 192 GB before streaming parse;
- M4 parse/normalize peak is at most 128 GB;
- indexing wall time at the same worker count regresses by no more than 25%;
- cache size, selected pages and all XOR results remain unchanged.

The customer design and geometry-derived diagnostics remain outside Git. Only
aggregate counts, byte sizes, timings and RSS values may enter reports.

## 5. Sizing a source before it is indexed (`floe-index scan`)

Field 2026-10-19: a ~50 GB deck source - 1.97 billion records, 85% POLYGON,
5,461 cells each placed once, 781,767 CBLOCKs of ~120 KB inflated - was
killed in the syntax parse on a 376 GiB host: 361 GiB resident at 160 s
(the twelve-thread parse grew ~2.2 GiB/s), then swap until the kernel's
OOM killer at 1,425 s. No in-memory pipeline holds that file: at the record
sizes above its Doc is 0.5-1.3 TiB before normalization, and normalization
clones every record whose >= 16-member point-list repetition decomposes
into grids. `floe-index scan FILE [jobs]` (0.12.328) answers the sizing
question before an index is attempted - one pass over the source, nothing
retained beyond counters and one small struct per cell:

- `points` / `points_max`: polygon and path vertices as `doc.rs`
  materializes them (16 B each; the implicit closing vertex of a closed
  manhattan list, trapezoids as 4, CTRAPEZOID triangles as 3, circles as
  64), and the largest single point list;
- `pts_reps` / `pts_rep_members` / `pts_rep_members_max`: explicit
  point-list repetitions (types 4-7, 10, 11: one heap block each, 16 B per
  member; a type-0 reuse shares the block);
- `doc_bytes_est`: the parsed Doc's resident bytes - every record at its
  real `size_of` (RectRec 96, PolyRec 88, PathRec 112, PlaceRec 88, TextRec
  104; `Rep` is 56 B inline) plus the per-record heap (vertices, point-list
  repetitions, explicit text strings, cell names), before normalization
  and without the source buffer, which stays resident until normalization
  ends - so the parse-end RSS is `doc_bytes_est + file_bytes`;
- `doc_bytes_est_pow2`: the same with every per-record heap block at the
  power-of-two class the portable bundle's thread-caching allocator
  (`rust/cli/src/tcache.rs`, classes 16 B .. 32 KiB) hands out - a
  20-vertex polygon's 320 B block occupies 512 B there, and musl's own class
  rounding sits on top; the glibc build is close to `doc_bytes_est`. The
  two differ only for files whose records own heap blocks (polygons, paths,
  point-list repetitions);
- `cell_stats`: `records` and `doc_bytes` percentiles (max / p99 / p90 /
  p50) over the cells and the ten biggest cells by `doc_bytes` (`cell`,
  `records`, `shapes`, `placements`, `texts`, `points`, `pts_rep_members`,
  `doc_bytes`, `doc_bytes_pow2`), in file order whatever the worker count -
  the working set a cell-wise streaming build needs for its largest unit.
  The stderr summary line repeats the totals and the largest cell.

Checked against the `[vfs] syntax parsed` RSS of the in-memory build
(macOS system allocator, `--jobs 1`, source still resident):

| file | `doc_bytes_est` + `file_bytes` | parse-end RSS |
| --- | --- | --- |
| sample9 (145 MB, 1,188 cells, CBLOCK, rectangles) | 1.32 + 0.14 = 1.46 GiB | 1.47 GiB |
| synthetic routing chip (281 MB, 29 cells, 16 x 1 M-member point lists) | 2.71 + 0.26 = 2.97 GiB | 2.96 GiB |
| synthetic flat 16 M CTRAPEZOID file (54 MB compressed) | 2.27 + 0.05 = 2.32 GiB | 2.27 GiB |

The estimate is the floor for the whole run: the plan and encode phases
work on top of the resident Doc, and normalization can only add to it (the
routing chip above goes from 2.96 to 5.24 GiB there). Repetition
normalization, `cell_stats` and the per-cell working set are the inputs of
the streaming build this plan's M5 leads to.

The field scan (0.12.328, 2026-10-19) answered the unknowns: 48.5 billion
vertices (28.9 per polygon), 2.5 billion point-list repetition members
(10.5 per repetition), `doc_bytes_est` 927 GiB (1.05 TiB with the bundle's
power-of-two classes) + 49 GiB source, largest cell 1.18 M records /
415 MiB. The cell-wise two-pass streaming build that follows from these
numbers is designed in docs/STREAMING_INDEX_PLAN.ko.md (inventory pass on
the scan's frame, file-order cell parse into the existing planner /
committer / encoder, byte-identical output as the transition gate).
