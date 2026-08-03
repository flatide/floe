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
floe-index vfs chip.oas chip.oas.floe --jobs 16 \
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
