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

현재 코드 기본값은 `jobs=min(8, host CPUs)`, `tile_px=128`,
`round_pages=128`이다. 아래의 256px 측정은 아직 기본값 변경이 아니라
`FLOE_RUST_TILE_PX=256` 실험 결과다.

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
FLOE_RUST_TILE_PX=256 \
FLOE_RUST_ROUND_PAGES=256 \
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

### 3.2 256px tile 현장 확인

| trace | total | load | Rust raster(`draw`) | 128px 대비 total |
|---|---:|---:|---:|---:|
| 500 첫 방문 | 163ms | 30ms | 93ms | -32% |
| 400 warm | 130ms | 0ms | 86ms | -30% |
| 500 warm 복귀 | 130ms | 0ms | 91ms | -37% |

500um warm은 floe와 사실상 동률이 됐지만, Rust jobs=1의 같은 알고리즘은 약
394ms가 걸려 총 CPU 작업량은 KLayout warm draw보다 여전히 크다. floe2의 8 worker는
idle 때 CPU를 쓰지 않지만 interactive render burst 동안 더 많은 코어·전력과
동시 작업 여유를 소비한다.

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

384px는 이 한 화면의 최저점이지만 tile이 9개뿐이라 다른 화면에서 tail imbalance가
커질 수 있다. 256px를 기본 후보로 두고 field trace로 일반성을 검증한다.

### 3.4 불필요한 refinement

sample9 700um plan은 146 pages다. 현재 Rust는 cache hit 여부와 무관하게
`selected.len()`을 128-page range로 나눠 128+18 두 round를 만든다. 각 round가
현재까지의 전체 scene을 다시 raster하고 PNG를 다시 만든다.

| 조건 | 첫 frame | settled | cache 상태 | 누적 raster/png |
|---|---:|---:|---|---:|
| round=128, cold | 176ms | 303ms | miss 146 | 198.4 / 40.2ms |
| round=128, warm | 117ms | 244ms | hit 146, miss 0 | 192.6 / 40.4ms |
| round=256, cold | final 164ms | 164ms | miss 146 | 96.3 / 21.2ms |
| round=256, warm | final 120ms | 120ms | hit 146, miss 0 | 92.7 / 20.3ms |

이 화면에서는 progressive first frame(176ms)보다 한 번에 완성한 cold frame(164ms)이
더 빠르다. refinement 자체를 폐기하지는 않지만, 고정 page-count 분할은 큰 cold
working set에 실제 first-paint 이득이 있을 때만 작동하도록 바꿔야 한다.

## 4. 이슈 목록

| ID | 우선순위 | 상태 | 요약 | 다음 판정 |
|---|---:|---|---|---|
| F2R-01 | P1 | `READY` | cache hit까지 128-page refine | miss 기반 round gate 구현 |
| F2R-02 | P1 | `READY` | 128px tile의 반복 hierarchy 순회 | 256px 기본 후보 field gate |
| F2R-03 | P1 | `OPEN` | tile x layer x hierarchy 총 CPU 작업량 | work bin 설계·jobs=1 개선 |
| F2R-04 | P1 | `READY` | total에서 사라진 PNG/publish 37~44ms | phase telemetry 분리 |
| F2R-05 | P2 | `BLOCKED` | jobs=8의 CPU/전력 비용 | F2R-03 뒤 adaptive jobs 판정 |
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

구현 방향:

1. 현재 cache hit page는 첫 scene에 모두 포함한다.
2. `round_pages` 또는 향후 byte/time budget은 miss 집합에만 적용한다.
3. 작은 tail은 앞 round와 합쳐, 첫 frame 비용이 direct-final보다 큰 refinement를
   만들지 않는다.
4. 매우 큰 cold miss에서만 progressive를 유지하고 generation budget/취소 계약은
   그대로 보존한다.

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

구현 방향:

- 1차 후보는 기본 256px.
- 장기적으로 framebuffer 크기, jobs와 계획 page/paint-plane 수로 192/256/384 중
  결정하는 bounded adaptive 정책을 검토한다.

수용 gate:

- sample9 500 warm raster 중앙값이 기존 128px보다 30% 이상 개선.
- field trace 전체에서 total 중앙값 10% 초과 회귀 없음.
- jobs 1/2/4/8/16, tile 64/128/256/384의 RGBA와 PNG bytes 동일.
- tile seam/KLayout oracle/cancellation/RSS gate 통과.

### F2R-03 — 반복 hierarchy/layer traversal 제거

현재 구조는 `tile -> paint layer -> render_cell hierarchy -> page records`다. 큰 tile은
중복을 줄이는 완화책일 뿐이며, sample9 tile=256에서도 jobs=1 raster가 약 394ms라
KLayout warm draw 104~125ms보다 총 CPU 작업이 크다.

후보:

- frame당 WC/page를 visible image tile과 paint plane에 한 번 binning
- layer별 page 목록과 transformed bbox를 `FrameScene`에서 불변 공유
- 같은 hierarchy transform/repetition 가시 범위 결과를 tile/plane 사이 재사용

수용 gate:

- sample9 500 warm jobs=1 raster를 현 394ms 대비 최소 2배 개선.
- jobs=4가 현 jobs=8 latency에 근접하고 jobs=8은 추가 이득을 유지.
- bin memory가 decoded generation budget과 별도 무제한 복사본이 되지 않음.
- hierarchy/repetition/half-phase oracle과 worker/tile 결정성 전부 유지.

### F2R-04 — PNG/publish/handoff 계측과 임시 frame 게시

현재 daemon의 `png_us`는 encode까지만 측정한다. `publish_png()`의
write/`sync_all()`/rename과 stdout response, adapter file read는 GUI total에만 들어가
37~44ms가 원인 불명 구간으로 남는다.

구현 순서:

1. daemon `write_us`, `sync_us`, `rename_us`와 adapter `read_us`를 각각 노출.
2. settled terminal log에는 Rust `raster/png/publish/handoff`를 같은 의미로 출력.
3. 계측 후에만 GUI 전용 임시 frame의 `sync_all()` 생략 여부를 결정한다.
4. clip과 headless export의 fsync+atomic replace 내구성은 유지한다.

수용 gate:

- `total - known phases` 중앙값 5ms 이하 또는 명시적 queue/scheduling 필드로 설명.
- atomic rename, stale generation 미게시, partial/final 즉시 정리 계약 유지.
- ENOSPC/write/rename 오류가 cancelled로 위장되지 않음.

### F2R-05 — adaptive jobs와 CPU 비용

현재 8 worker는 latency에는 유효하지만 sample9 warm frame의 총 raster CPU 작업량은
KLayout보다 크다. F2R-03 이전에 jobs만 줄이면 latency가 다시 나빠지므로 구조 개선
뒤 판정한다.

수용 방향:

- actual tile/work count로 worker를 제한하고 16-worker 역효과를 피함.
- local interactive 기본 latency와 shared-host CPU 절약 profile을 분리할 필요가
  있는지 field trace로 결정.
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

## 6. 권장 착수 순서

1. F2R-01 cache-aware refinement와 round 수 integration gate
2. F2R-02 기본 256px 후보를 전체 oracle/field trace로 승인
3. F2R-04 phase telemetry로 남은 37~44ms 분해
4. F2R-03 work bin으로 총 CPU 작업량 감소
5. F2R-05 jobs 기본/절약 profile 판정
6. F2R-06은 thread startup 실측이 임계값을 넘을 때만 수행

각 완료 항목은 이 표의 상태, before/after 중앙값, 실행 명령, 적용 커밋과 자동 gate를
같이 갱신한다.
