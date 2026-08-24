# floe2 renderer 최적화 추적

갱신일: 2026-08-25

이 문서는 floe2의 **실행 중 renderer** 성능 이슈를 재현 수치와 수용 gate로
추적하는 canonical 목록이다. 정확도·취소·게시 계약은
`docs/RUST_RENDERER_PLAN.ko.md`, 제품 경계는 `docs/FLOE2.md`를 따른다.
monster-cell 인덱싱과 #76 실행 슬롯 예산은 `docs/SPEC-INDEXER.ko.md`가
canonical이며 여기서 중복 추적하지 않는다.

## 1. 상태와 변경 원칙

| 상태 | 의미 |
|---|---|
| `OPEN` | 원인 또는 구현 방향을 더 확인해야 함 |
| `READY` | 원인·수용 기준이 확정되어 구현 가능 |
| `DOING` | 구현과 gate 추가 진행 중 |
| `BLOCKED` | 선행 계측이나 설계 결정 필요 |
| `DONE` | 코드·자동 gate·운영 실측까지 완료 |

최적화는 다음 원칙을 지킨다.

1. pixel 정확도, jobs/tile 결정성, 취소 frontier와 atomic publish를 성능보다
   우선한다.
2. 동일 source·viewport·화면 크기에서 한 변수만 바꾸고 3회 중앙값을 기록한다.
3. wall time뿐 아니라 `raster_ms`, 총 CPU 작업량, PNG/publish, peak RSS와
   첫 frame/settled 시간을 함께 본다.
4. sample9 수치는 방향을 정하는 재현 gate다. 기본값 변경은 valmini와 대표 실칩
   trace에서 회귀가 없는 경우에만 승인한다.

현재 floe2 제품 기본값은 page decode `jobs=min(8, host CPUs)`, raster
`jobs=min(4, decode jobs)`, `tile_px=384`, `round_pages=128`이다. daemon의
하위 호환 protocol fallback은 raster/decode 공통 jobs와 128px tile을 유지한다.
환경변수 `FLOE_RUST_JOBS`, `FLOE_RUST_RASTER_JOBS`, `FLOE_RUST_TILE_PX`로
각 값을 독립 재현할 수 있다.

## 2. 고정 재현 조건

- source: `data/sample9.oas` (`tools/gen_sample9.py`; 현장 표기 sample09)
- mode: full depth, detail high, coverage 없음
- goto center: `x=13600um`, `y=8600um`
- GUI framebuffer: 약 `858x789px`
- zoom trace A: view `500 -> 400 -> 500um`
- zoom trace B: view `700 -> 560 -> 700um`
- 비교 backend: stable floe/KLayout과 floe2/Rust persistent session

즉시 비교용 실행 예:

```sh
FLOE_RUST_JOBS=8 \
FLOE_RUST_RASTER_JOBS=4 \
FLOE_RUST_TILE_PX=384 \
FLOE_RUST_ROUND_PAGES=128 \
  .venv/bin/python -m floe2 view data/sample9.oas
```

GUI의 `tiles`는 VFS 계획 page 수다. Rust의 실제 image tile 수는 별도
`render_tiles` telemetry로 판정한다.

## 3. 기준선

### 3.1 floe와 floe2, 기본 128px tile

| trace | floe total | floe2 total | floe2 표시 load/draw |
|---|---:|---:|---:|
| 500 첫 방문 | 534ms | 240ms | 26ms / 174ms |
| 400 warm | 107ms | 186ms | 0ms / 149ms |
| 500 warm 복귀 | 128~129ms | 207ms | 0ms / 166ms |

floe는 첫 방문 때 KLayout working layout의 page apply에 419ms를 쓰지만 warm
재방문에서는 이를 재사용한다. floe2도 decoded-page cache hit로 read/decode가 0이
되지만, viewport마다 전체 scene을 다시 raster/PNG publish하므로 warm 이득은 page
decode 부분으로 제한된다.

로그의 `draw`는 같은 뜻이 아니다. floe는 KLayout `save_image()`의 raster+PNG를
포함하고, floe2는 Rust `raster_us`만 표시한다. floe2 total에서 load/draw를 뺀
37~44ms에는 PNG encode, 임시 파일 write/sync, rename, protocol과 Python read가
섞여 있다.

### 3.2 256px tile 중간 확인

| trace | total | load | Rust raster(`draw`) | 128px 대비 total |
|---|---:|---:|---:|---:|
| 500 첫 방문 | 163ms | 30ms | 93ms | -32% |
| 400 warm | 130ms | 0ms | 86ms | -30% |
| 500 warm 복귀 | 130ms | 0ms | 91ms | -37% |

500um warm은 floe와 사실상 동률이 됐지만, Rust jobs=1의 같은 tile은 약 394ms가
걸렸다. 이는 KLayout보다 구조적으로 느리다는 결론이 아니라, tile마다 hierarchy를
다시 걷는 현재 구조에서 작은 tile과 단일 worker의 조합이 나쁘다는 결과다.

### 3.3 tile 및 jobs sweep

sample9 500um warm, `jobs=8`:

| tile_px | raster | total | render tiles |
|---:|---:|---:|---:|
| 64 | 394.6ms | 418ms | 182 |
| 96 | 217.5ms | 241ms | 81 |
| 128 | 154.5ms | 177ms | 49 |
| 192 | 99.9ms | 122ms | 25 |
| 256 | 81.3ms | 105ms | 16 |
| 384 | 74.8ms | 97ms | 9 |
| 512 | 89.4ms | 112ms | 4 |

`tile_px=256` jobs scaling:

| jobs | raster | total |
|---:|---:|---:|
| 1 | 394.3ms | 416ms |
| 2 | 200.3ms | 222ms |
| 4 | 105.2ms | 128ms |
| 8 | 76.5ms | 99ms |
| 16 | 81.9ms | 104ms |

384px는 4-worker에서 같은 화면을 약 99ms에 처리해 256px/8-worker와 같은 latency를
절반의 raster worker로 냈다. 대표 trace와 결정성 gate에 회귀가 없어 제품 기본값으로
채택했다. daemon을 직접 사용하는 기존 호출의 protocol fallback 128px는 바꾸지 않았다.

단일 worker도 tile 크기를 맞춰 다시 측정했다.

| jobs=1 tile_px | raster | total |
|---:|---:|---:|
| 128 | 744.4ms | 768ms |
| 256 | 381.8ms | 404ms |
| 512 | 203.5ms | 225ms |
| 768 | 188.1ms | 211ms |
| 1024 | 126.5ms | 149ms |

단일 worker 최적점 149ms는 나쁘지 않지만 floe warm 129ms 대비 약 16% 느려
"초기 load만 병렬, warm render는 단일" 정책은 채택하지 않았다. 아래의 decode 8 /
raster 4가 지연시간, 총 코어 사용과 95% 목표를 함께 만족하는 현재 기본점이다.

### 3.4 불필요한 refinement와 수정 결과

sample9 700um plan은 146 pages다. 수정 전 Rust는 cache hit 여부와 무관하게
`selected.len()`을 128-page range로 나눠 128+18 두 round를 만든다. 각 round가
현재까지의 전체 scene을 다시 raster하고 PNG를 다시 만든다.

| 조건 | 첫 frame | settled | cache 상태 | 누적 raster/png |
|---|---:|---:|---|---:|
| round=128, cold | 176ms | 303ms | miss 146 | 198.4 / 40.2ms |
| round=128, warm | 117ms | 244ms | hit 146, miss 0 | 192.6 / 40.4ms |
| round=256, cold | final 164ms | 164ms | miss 146 | 96.3 / 21.2ms |
| round=256, warm | final 120ms | 120ms | hit 146, miss 0 | 92.7 / 20.3ms |

이 화면에서는 progressive first frame(176ms)보다 한 번에 완성한 cold frame(164ms)이
더 빨랐다. 수정 후 cache hit page는 모두 첫 batch에 들어가고 miss에만 budget을
적용한다. 마지막 miss tail이 budget의 절반 이하면 앞 batch와 합친다. 같은 146-page
화면은 cold 182ms 1 frame, warm 118ms 1 frame이며 warm refine은 발생하지 않는다.

### 3.5 채택한 제품 기본값 결과

`decode_jobs=8`, `raster jobs=4`, `tile_px=384`, cache-aware round의 persistent
sample9 결과다. 측정 편차를 고려해도 floe warm의 95% 이내라는 목표를 넘는다.

| trace | floe | floe2 변경 전 | floe2 변경 후 |
|---|---:|---:|---:|
| 500 첫 방문 | 534ms | 240ms | 150ms |
| 400 warm | 107ms | 186ms | 86ms |
| 500 warm 복귀 | 128~129ms | 207ms | 99ms |
| 700 warm 복귀 | refine 없음 | 244ms, 2 frames | 124ms, 1 frame |

500 warm 기준 floe2는 floe보다 약 23% 빠르고 raster worker는 8개에서 4개로 줄었다.
첫 방문도 기존 floe 대비 약 3.6배 빠르다. 이는 KLayout의 retained renderer를 복제한
결과는 아니며, Rust는 매 viewport를 다시 raster한다. 다만 현재 사용성 trace에서는
그 구조 차이를 제한된 병렬성과 더 큰 image tile로 상쇄한다.

## 4. 이슈 목록

| ID | 우선순위 | 상태 | 요약 | 다음 판정 |
|---|---:|---|---|---|
| F2R-01 | P1 | `DONE` | cache hit까지 128-page refine | cache-aware batch/unit+현장 gate 완료 |
| F2R-02 | P1 | `DONE` | 128px tile의 반복 hierarchy 순회 | 제품 기본 384px 승인 |
| F2R-03 | P1 | `OPEN` | tile x layer x hierarchy 총 CPU 작업량 | work bin 설계·jobs=1 개선 |
| F2R-04 | P1 | `DONE` | total에서 사라진 PNG/publish 37~44ms | write/sync/rename/handoff 계측 완료 |
| F2R-05 | P2 | `DONE` | jobs=8의 CPU/전력 비용 | decode 8/raster 4 분리 승인 |
| F2R-06 | P3 | `BLOCKED` | render마다 OS thread 생성 | startup_us가 병목일 때만 pool |
| F2R-07 | P1 | `DONE` | OVC coverage post-composite 회귀 | 제품 경로 제거 gate 유지 |

## 5. 상세 이슈와 수용 기준

### F2R-01 — cache-aware progressive refinement

원인:

- `rust/renderd/src/main.rs`가
  `refinement_ranges(selected.len(), round_pages)`로 전체 선택 page를 분할한다.
- `DecodedPageCache::contains()`/hit 여부는 round 구성에 사용하지 않는다.
- 두 번째 round도 신규 18 pages만 합성하지 않고 누적 146 pages 전체를 raster하고
  PNG publish한다.
- stable floe의 VFS session은 stream budget을 committed cache에 없는 `cand`에만
  적용하므로 warm 계획은 partial이 아니다.

구현 결과:

1. 현재 cache hit page는 첫 scene에 모두 포함한다.
2. `round_pages` 또는 향후 byte/time budget은 miss 집합에만 적용한다.
3. 작은 tail은 앞 round와 합쳐, 첫 frame 비용이 direct-final보다 큰 refinement를
   만들지 않는다.
4. 매우 큰 cold miss에서만 progressive를 유지하고 generation budget/취소 계약은
   그대로 보존한다.

unit gate는 cold/mixed/warm/empty 선택에서 각 page가 정확히 한 번 포함되는지와
batch 경계를 검증한다. adapter/cancellation integration gate도 동일 protocol을 탄다.

수용 gate:

- 선택 page가 전부 cache hit면 page 수와 무관하게 frame 1개, `final=1`, miss 0.
- sample9 700 warm은 refine 없이 1회 settle하며 round=256 direct 기준의 1.15배 이내.
- sample9 700 cold에서 first frame과 settled가 direct-final보다 모두 느린 128+tail
  분할을 만들지 않음.
- mixed hit/miss fixture가 각 선택 page를 정확히 한 번 load하고 최종 pixel/PNG가
  jobs 1/4/8 및 round budget에 무관하게 동일.
- 100-generation cancellation soak의 stale publish/partial 잔류 0 유지.

### F2R-02 — image tile 기본값/adaptive 크기

원인:

- 각 `raster_tile()`은 모든 style layer를 순서대로 돌고 `render_cell()` hierarchy를
  다시 순회한다.
- 858x789 화면에서 128px는 49개, 256px는 16개의 독립 순회를 만든다.
- 너무 큰 tile은 worker 수와 tail balance를 잃으므로 단일 sample의 384px 최저점만
  보고 기본값을 정할 수 없다.

구현 결과:

- floe2 adapter 기본은 384px, raster 4 workers다.
- `FLOE_RUST_TILE_PX`와 `FLOE_RUST_RASTER_JOBS`로 현장 되돌림/재현이 가능하다.
- page bbox와 local tile view가 만나지 않으면 record walk 전에 제외한다.
- 장기적으로 framebuffer와 work count 기반 bounded adaptive 정책은 F2R-03 측정 뒤
  다시 검토한다.

수용 gate:

- sample9 500 warm raster 중앙값이 기존 128px보다 30% 이상 개선.
- field trace 전체에서 total 중앙값 10% 초과 회귀 없음.
- jobs 1/2/4/8/16, tile 64/128/256/384의 RGBA와 PNG bytes 동일.
- tile seam/KLayout oracle/cancellation/RSS gate 통과.

### F2R-03 — 반복 hierarchy/layer traversal 제거

현재 구조는 `tile -> paint layer -> render_cell hierarchy -> page records`다. page bbox
prune과 큰 tile은 중복을 줄이는 완화책일 뿐이다. jobs=1도 tile=1024에서는 raster
126.5ms까지 내려왔지만 total 149ms로 floe 129ms의 95% 목표에는 아직 약 16% 부족하다.

후보:

- frame당 WC/page를 visible image tile과 paint plane에 한 번 binning
- layer별 page 목록과 transformed bbox를 `FrameScene`에서 불변 공유
- 같은 hierarchy transform/repetition 가시 범위 결과를 tile/plane 사이 재사용

수용 gate:

- sample9 500 warm jobs=1 최적 tile total을 149ms에서 136ms 이하로 개선해
  floe 129ms 대비 95% 이상의 처리 성능을 달성.
- jobs=4가 현 jobs=8 latency에 근접하고 jobs=8은 추가 이득을 유지.
- bin memory가 decoded generation budget과 별도 무제한 복사본이 되지 않음.
- hierarchy/repetition/half-phase oracle과 worker/tile 결정성 전부 유지.

### F2R-04 — PNG/publish/handoff 계측과 임시 frame 게시

daemon은 PNG encode 외에 `publish_write/sync/rename_us`를, adapter는 file
read/unlink handoff를 각각 측정한다. GUI perf 상태와 `tools/bench_floe2.py`에도 같은
필드가 노출된다. sample9 warm에서 PNG 약 16~21ms, publish 약 3~6ms, adapter read
약 1ms로 기존 원인 불명 구간 대부분을 설명했다.

`sync_all()`은 3~6ms로 병목 비중이 작아 취소/atomic publish 계약을 약화시키지 않고
유지한다.

수용 gate:

- `total - known phases` 중앙값 5ms 이하 또는 명시적 queue/scheduling 필드로 설명.
- atomic rename, stale generation 미게시, partial/final 즉시 정리 계약 유지.
- ENOSPC/write/rename 오류가 cancelled로 위장되지 않음.

### F2R-05 — adaptive jobs와 CPU 비용

page decode와 raster의 worker 수를 분리했다. cold page read/decode는 8 worker를
유지하고, viewport raster는 4 worker/384px를 기본으로 사용한다. sample9 warm은
99ms로 8-worker/128px의 177~207ms보다 빠르며 burst의 worker 비용도 절반이다.

운영 방향:

- actual tile count로 worker를 제한하고 16-worker 역효과를 피함.
- `FLOE_RUST_RASTER_JOBS=1`은 진단/절약 profile로 남기되 자동 warm 전환은 하지 않음.
- idle daemon은 현재처럼 CPU를 소비하지 않음.

### F2R-06 — daemon-lifetime raster pool

현재 frame마다 scoped OS threads를 만든다. 아직 startup이 측정 병목이라는 증거가
없으므로 pool부터 구현하지 않는다. F2R-04에 `worker_start_us/join_us`를 추가한 뒤
frame의 5% 이상일 때만 `READY`로 전환한다.

### F2R-07 — density coverage 제거 (`DONE`)

sample9 detail-high 특정 줌에서 화면 차이 없이 refine 350ms -> 980ms였고, 매
progressive PNG마다 NumPy/Pillow decode/composite/encode를 반복했다. floe2의
CLI/UI/request/Rust worker에서 제거했으며 공유 cache의 `design.ovc`는 무시한다.
회귀 gate는 `tools/validate_floe2.py`와 `tools/validate_rust_renderer.py`가 소유한다.

## 6. 남은 착수 순서

1. F2R-03 frame work bin/transform 재사용으로 단일 worker total을 floe의 95% 안으로
   낮춘다. 현 제품 기본 성능을 회귀시키지 않는 경우에만 편입한다.
2. F2R-06은 thread startup 실측이 frame의 5%를 넘을 때만 수행한다.
3. 대표 실칩 trace에서 4-worker tail imbalance가 확인될 때만 bounded adaptive tile/jobs를
   다시 연다.

각 완료 항목은 이 표의 상태, before/after 중앙값, 실행 명령, 적용 커밋과 자동 gate를
같이 갱신한다.
