# floe2 renderer 최적화 추적

갱신일: 2026-08-26

이 문서는 floe2의 **실행 중 renderer** 성능 이슈를 재현 수치와 수용 gate로
추적하는 canonical 목록이다. 정확도·취소·게시 계약은
`docs/RUST_RENDERER_PLAN.ko.md`, 제품 경계는 `docs/FLOE2.md`를 따른다.
monster-cell 인덱싱과 #76 실행 슬롯 예산은 `docs/SPEC-INDEXER.ko.md`가
canonical이며 여기서 중복 추적하지 않는다.

## 1. 상태와 변경 원칙

**제품 상태(2026-09-08, 사용자 결정)**: floe2가 안정 상태에 들어와
`review/floe2`를 `main`으로 승격했다(fast-forward, 0.12.64). KLayout
셸 `floe`는 시장 릴리즈 없이 `floe-legacy` 브랜치·`floe-frozen-2026-09-08`
태그로 동결. KLayout은 oracle·생성기·신규 기능 선행 검증용으로만 남고,
웹 셸 계획은 floe2에만 적용하며, 버전은 0.12.x를 그대로 잇는다. floe2가
정식 상품화 대상이다. 이 문서의 "floe 대비" 실측은 동결된 KLayout 셸과의
비교로 계속 유효하다.

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
5. KLayout과의 비교는 제품 latency와 단일-core 효율을 분리한다. floe의 KLayout
   drawing worker를 1로 고정한 total을 single-raster 기준으로 삼고, Rust의 병렬
   raster는 사용자 latency를 줄이는 별도 가속으로 평가한다.

현재 floe2 제품 기본값은 page decode `jobs=min(8, host CPUs)`, raster
`jobs=min(4, decode jobs)`, `tile_px=384`, `round_pages=1024`이다. daemon의
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
FLOE_RUST_ROUND_PAGES=1024 \
  .venv/bin/python -m floe2 view data/sample9.oas
```

KLayout과 같은 단일 raster 기준은 decode만 병렬로 유지하고 framebuffer가
858x789px인 이 fixture를 한 image tile로 만든다.

```sh
FLOE_RUST_JOBS=8 \
FLOE_RUST_RASTER_JOBS=1 \
FLOE_RUST_TILE_PX=1024 \
  .venv/bin/python -m floe2 view data/sample9.oas --multi \
  --goto 13600,8600,500 --detail high --depth 999 --perf-baseline
```

backend-neutral 기본 성능은 환경변수 대신 두 제품에 동일한 preset으로 측정한다.
refinement/exact frame cache/LOD/frame/label을 끄고 page·working-set cache 및 최종
PNG publish는 남긴다. 첫 명령은 cold session, 이후 pan은 warm working-set을 잰다.

```sh
.venv/bin/python -m floe view data/sample9.oas --multi \
  --goto 13600,8600,700 --detail high --depth 999 --perf-baseline
.venv/bin/python -m floe2 view data/sample9.oas --multi \
  --goto 13600,8600,700 --detail high --depth 999 --perf-baseline
```

refinement만 비교할 때는 `--refinement off`를 사용한다. floe에서는 `stream_kb=0`,
floe2에서는 `round_pages=2^30` wire 값으로 대응해 intermediate frame을 만들지 않는다.
`--frame-cache off`는 floe2의 exact PNG LRU만 우회하며 decoded page LRU는 유지한다.

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

### 3.6 exact viewport 재방문 cache

다른 실행 환경의 700→560→700 detail-high 현장 trace는 cache-aware round 뒤에도
warm 700이 206ms였다. load는 0이지만 raster 158ms + PNG 36.4ms + publish 6.3ms를
다시 지불해, page cache만으로는 KLayout의 retained-render 이득을 재현하지 못했다.

daemon에 최근 최종 PNG 3개, 합계 최대 64MiB의 LRU를 추가했다. viewport/device
크기/render state가 bit-exact 일치하고 선택 page가 decoded LRU에 모두 남아 있으면
`FrameScene`만 재구성·게시하고 raster/PNG encode를 생략한다. scene을 같이 복원하므로
GUI pixbuf만 재사용할 때 생기는 pick/snap WYSIWYG 불일치가 없다.

858x789 sample9 detail-high hotspot release 실측은 첫 방문 198ms(raster 116.3,
PNG 27.0)에서 한 화면을 사이에 둔 exact 재방문 6ms(raster/PNG 0, publish 2.8)로
감소했다. 다른 좌표·크기·depth/cut/layer/style/font/mono는 cache miss이며 정상
raster 경로를 탄다. decoded page가 퇴거된 경우에도 cache를 억지로 쓰지 않는다.

### 3.7 cold pan의 반복 raster 제거

700µm 화면에서 위로 한 번 이동한 현장 trace는 신규 744 pages를 128씩 6 round로
나눴다. 860x804/384px 화면의 실제 image tile은 9개인데 누적 telemetry가 54개였고,
load 16ms에 비해 누적 raster 748ms + PNG 135.7ms + publish 26.3ms로 total 936ms가
됐다. 같은 화면의 floe는 301ms였다. 이전 frame이 frozen preview로 계속 보이는
interactive GUI에서 이 6개 partial의 first-paint 이득은 작고 settled 비용만 키웠다.

floe2 제품 round 기본을 1024 pages로 올렸다. 같은 방향의 local detail-high trace는
128-page 406ms에서 1024-page 220ms로 줄었고, 4096-page 228ms보다도 나쁘지 않았다.
raw daemon protocol fallback 128과 `FLOE_RUST_ROUND_PAGES` override는 유지한다.
1024를 넘는 매우 큰 miss 집합에는 progressive/cancellation이 계속 적용된다.

같은 현장 동작을 제품 기본으로 재측정한 결과는 floe 300ms(load 157/draw 143),
floe2 201ms(load 22/raster 142/PNG 22.4/publish 4.4)였다. 실제 image tile도
54에서 framebuffer의 정확한 개수인 9로 줄었다. 표시된 Rust raster와 KLayout draw는
142/143ms로 비슷하지만 전자는 raster만, 후자는 `save_image()`의 raster+PNG라 같은
phase는 아니다. page load 절감까지 합친 전체 latency는 floe2가 33% 빨랐다.

### 3.8 exact 밖의 인접 뷰 재사용과 refinement 재설계

사용자 관찰은 exact하지 않은 인접 viewport에서도 floe가 floe2보다 cache hit처럼
반응한다는 것이다. 아직 고정 pan sweep으로 확인한 결론은 아니지만 코드상 가능한
구조 차이는 명확하다.

- 공통 GUI는 `last_frame` 하나를 frozen preview로 표시하고, 같은 scale/render state의
  현재 viewport가 기존 frame 안에 들어올 때 `_covered()`로 새 render를 생략한다.
  현재 render bbox 여유는 축당 약 2px뿐이므로 backend별 인접 재사용 차이를 설명하지
  못한다.
- floe는 하나의 KLayout `Layout`과 `LayoutView`를 세션 내내 유지한다. VFS delta는
  working layout에 없는 page-cell만 parse/apply하고 resident cell은 남긴다. 따라서
  인접 pan은 이미 등록된 native cell/hierarchy/spatial 구조를 재사용한다.
- floe2는 decoded page `Arc`를 LRU에 남기므로 read/decode는 피하지만, viewport마다
  새 `HierPlan`/`FrameScene`을 조립하고 viewport-local image tile마다 layer/hierarchy를
  다시 순회해 full framebuffer를 만든다. 최종 PNG cache key도 viewport float bits를
  포함하므로 조금만 이동해도 miss다.
- KLayout `save_image()`의 `image_with_options()`는 호출마다 detached pixel buffer와
  `BitmapRedrawThreadCanvas`를 새로 만들어 완성 이미지를 그린다. 따라서 이 경로의
  장점은 이전 PNG를 그대로 재사용하는 데 있지 않고, persistent `Layout`의 native
  hierarchy/spatial 구조와 이미 apply된 page-cell을 재사용하는 데 있다.

이 문제와 cold refinement는 분리한다. 인접 pan은 same-scale world-aligned retained
tile/scene cache 문제이고, 대형 cold view는 first-paint/settled scheduling 문제다.
exact PNG LRU를 넓혀 둘을 함께 해결하지 않는다.

현재 Rust refinement도 미리 만든 PNG에 정밀 geometry를 덧붙이는 방식은 아니다.
exact hit만 이전 최종 PNG를 재사용하고, frozen preview는 기다리는 동안 보일 뿐이다.
실제 progressive round는 miss page를 추가 decode한 뒤 누적 `FrameScene` 전체를 다시
raster하고 full PNG를 새로 만든다. 장기 후보는 page-round PNG가 아니라 page→final
image-tile dependency를 만든 뒤, 필요한 page가 준비된 tile을 center-first로 한 번만
병렬 raster하여 raw RGBA/shared framebuffer에 게시하는 방식이다. sub-second 작업은
이전 frame을 frozen 상태로 유지하고 single final render만 하는 현재 정책을 우선한다.

### 3.9 KLayout 병렬성 조사와 single-raster 기준선

조사 범위는 저장소의 `.venv` KLayout 0.30.9와 2026-08-25 시점 upstream source다.
KLayout renderer 전체를 single-thread라고 부르면 정확하지 않다. GUI drawing은 오래전부터
서로 다른 layer를 여러 CPU에서 그릴 수 있고, `Display > Optimizations`의 worker 수로
조절한다. 공식 기본값은 1이다.
([Changelog](https://github.com/KLayout/klayout/blob/master/Changelog),
[thread 구조 설명](https://www.klayout.de/forum/discussion/2288/threaded-rendering-question))

floe의 실제 경로는 GUI paint가 아니다. `floe/service.py`의 단일 render service가
`floe/render.py::Renderer.render_png()`를 호출하고, 마지막에
`LayoutView.save_image()`로 PNG를 만든다. API의 "synchronous"는 호출이 완성까지
기다린다는 계약이며 최신 구현에서는 곧바로 single-thread를 뜻하지 않는다.
2026-08-25 upstream `image_with_options()`는 view가 synchronous이면 worker 0,
아니면 `drawing_workers()`를 넘기고 완료를 기다린다.
([API](https://www.klayout.de/doc/code/class_LayoutView.html),
[source](https://github.com/KLayout/klayout/blob/master/src/laybasic/laybasic/layLayoutCanvas.cc))

그러나 현재 floe 비교 환경은 실제로 단일 C++ raster다.

- KLayout 0.30.9의 새 `LayoutView`에서 `drawing-workers=1`이다.
- 설치된 0.30.9 binary의 color `image_with_options()`는 `RedrawThread::start()`에
  synchronous worker 0을 전달한다.
- 16-layer synthetic probe에서 config를 1과 4로 바꿔도 각각
  `wall=0.099s`, `process CPU=0.099s`로 동일했다.
- floe의 `_VIEW_CONFIG`는 `drawing-workers`를 변경하지 않는다.

따라서 현재 비교의 해석은 `병렬 page 계획/적재 + persistent KLayout Layout + 단일
C++ raster/PNG` 대 `병렬 page decode + Rust tile raster/PNG`다. 향후 KLayout 버전에서
`save_image()`의 worker 사용이 달라질 수 있으므로 기준선에는 KLayout 버전과 worker를
기록하고 floe 쪽 worker를 명시적으로 1로 고정해야 한다.

single-raster 최적화는 제품의 4-worker 기본값을 즉시 없애는 작업이 아니다. sample9
500um warm에서 floe/KLayout total은 129ms, Rust jobs=1/tile=1024는 149ms, 현재
Rust jobs=4/tile=384는 99ms다. Rust single의 처리 성능은 KLayout의 약 86.6%이며
95% gate는 total 136ms 이하다. 먼저 F2R-03으로 이 간격을 닫고, 대표 실칩에서도
통과한 뒤에만 raster=1 기본 또는 work 기반 adaptive 전환을 판정한다.

### 3.10 serial raster 분해와 F2R-03a 결과 (2026-08-26)

canonical sample9(858x789, hotspot 13600,8600, 500µm, detail high, frame cache
off, 3회 중앙값)에서 F2R-12 serial profile로 분해했다. 이 viewport의 plan은
43 pages, records 1,255,269, 실제 paint member 123,845로 전부 rectangle이다.

occupancy mode(전 layer 1회 순회)가 styled 40-layer와 같은 132ms를 기록해
**layer 축 중복·frame band·label은 이 fixture에서 무시 가능**함을 직접 확인했다.
view span 125/250/500µm sweep(§record test 1.07M 동일 조건 포함)으로 분리한
결과, record당 가시성 판정 상한 ≈33ns(≈40ms), 나머지 ≈90ms가 member paint
(≈700ns/member)였고, 그 지배 항은 **member당 4-segment Bresenham stroke**
(clip + per-step block 중복 쓰기)였다.

solid stroke의 axis-aligned segment는 Bresenham 합집합이 단일 블록이므로
clamped row span 쓰기로 치환했다(대각선·dotted는 기존 경로 유지). 결과:

| profile | 변경 전 raster/total | 변경 후 raster/total |
|---|---:|---:|
| serial r1/d8, tile 858 | 136.1 / 157ms | 43.8 / 66ms |
| 제품 r4/d8, tile 384 | 79.8 / 101ms | 26.0 / 48ms |

serial total 66ms는 F2R-12 gate 136ms를 크게 통과하며, 문서의 KLayout warm
129ms 대비 약 2배 빠르다(단, 이 머신의 KLayout GUI 재측정은 잔여 항목).
정확도는 PNG md5 동일(hotspot occupancy 전후), KLayout oracle jobs 1/8
ALL OK, PX1~PX5 golden 0 실패, renderer integration 22 tests OK,
`axis_aligned_stroke_span_matches_stepped_oracle`·
`fill_span_matches_per_pixel_fill_oracle` unit oracle로 고정했다.

남은 serial 항은 record 가시성 판정 ≈44ms다. page 내부에 spatial 구조가
없어 선택된 모든 record가 매 frame `for_each_visible_offset`를 타며, 제품
병렬 경로에서는 같은 판정이 tile 수만큼 반복된다(4-worker raster CPU
4x26=104ms vs serial 44ms). F2R-03b는 이에 따라 sub-page record pruning을
1순위로 재조준한다.

### 3.11 F2R-03b sub-page record index 결과 (2026-08-26)

decode 시 page의 record extent(base bbox ⊕ repetition offset 범위) BVH를
`DecodedPage::index`로 만들어 LRU에 상주시키고, raster의 record 열거를
tile-local view 교차 질의로 바꿨다(`rust/render-core/src/page_index.rs`).

- 보수성: overflow·손상 geometry 등 render가 명시 오류로 보고하는 record는
  전면 커버 extent로 색인해 오류 도달성을 보존한다. PATH extent는 render와
  같은 outline bbox를 사용해 member 단위로 동일하게 판정된다.
- 접근 순서: 첫 구현의 BVH leaf 순 방문은 random access로 serial 1-tile에서
  +3ms 회귀했다(43.8→46.9ms). per-worker bitset으로 히트를 모아 **record
  오름차순으로 재방문**하고, view가 root extent를 덮으면 순차 전량 스캔으로
  폴백해 회귀를 제거했다.
- page 내 record 도색 순서는 같은 paint plane(동일 색/fill)이라 pixel에
  영향이 없고, 방문 순서 변경은 telemetry 카운터에만 반영된다.

sample9 hotspot(858x789/500µm, 3회 중앙값) 결과:

| profile | index 전 raster | index 후 raster |
|---|---:|---:|
| serial r1/d8, tile 858 | 43.8ms | 44.0ms (동률) |
| 제품 r4/d8, tile 384 | 26.0ms | 20.4ms (total 48→43ms) |
| r8/d8, tile 128 | 58.7ms | 39.9ms |
| serial r1, tile 128 | 272.2ms | 207.6ms |

방문 record는 1,302,013→329,859로 75% 줄었고, tile이 작을수록(=tile 축
곱셈이 클수록) 이득이 커진다. serial 1-tile은 이미 paint-bound라 동률이다.
비용: 43-page 화면 기준 decode 19→32ms(8-way, LRU 상주당 1회), decoded
resident 161→188MB — index bytes는 `estimated_bytes()`로 LRU budget에
계량된다. 검증: hotspot occupancy PNG md5 동일, KLayout oracle jobs 1/8
ALL OK, PX1~PX5 golden 0 실패, integration 22 tests OK, unit gate
`record_index_pruning_matches_unpruned_pixels`(pruned/unpruned pixel·paint
동일 + 방문 감소)와 extent 보수성 테스트 추가.

남은 축소 후보: visited record의 `Rep::Pts` member 전량 스캔(chunk bbox),
tile×plane work bin(F2R-10/11 선행), 그리고 paint-bound가 된 serial의
잔여는 F2R-03c 영역이다.

### 3.12 같은 머신 KLayout 기준선과 F2R-12 gate 판정 (2026-08-26)

`tools/bench_floe2.py --backend klayout`이 같은 persistent trace로 stable
floe/KLayout render service를 headless 구동한다(제품/renderer env를 세션에
고정, KLayout worker는 첫 frame에 cold open 포함). 세션 시작의
`[perf] backend=klayout version=0.30.9 drawing-workers=1` 라인이 §3.9의
단일 C++ raster 고정을 실행 시점에 증명한다.

sample9 858x789 hotspot 500µm, detail high, LOD off, frames/labels on,
3회 중앙값(total ms):

| 조건 | floe/KLayout | floe2 serial r1/tile858 | floe2 제품 r4/384 |
|---|---:|---:|---:|
| hotspot warm | 82 (draw 80) | 66 (raster 44) | 43 (raster 20) |
| hotspot cold | 381 (apply 296 + draw 79) | 103 | 79 |

**F2R-12 gate(단일 raster에서 KLayout의 95%)는 sample9에서 통과**: Rust
serial total 66ms는 KLayout 82ms 대비 124% 처리 성능이다. §3.9의 86.6%는
F2R-03a/03b 이전 수치다. 대표 실칩 p50/p95 확인이 남는다.

F2R-10 사전 sweep(`--pan-sweep`: 1/16·1/8·1/4 폭/높이 이동과 복귀, 3회
중앙값):

| 동작 | KLayout | floe2 r4 cache-off | floe2 r4 cache-on |
|---|---:|---:|---:|
| 인접 pan | 73~96ms | 44~48ms | 44~49ms |
| 정확 복귀 | 81~82ms | 42~43ms | 첫 복귀 44ms, 이후 6~7ms(hit 5/6) |

이 fixture에서는 "floe가 인접 pan에서 cache hit처럼 빠르다"는 관찰이
재현되지 않는다: floe는 인접 pan마다 full draw ~80ms를 다시 지불하고
retained Layout의 이득은 load(apply ≤9ms)뿐이다. floe2는 모든 인접 pan에서
약 2배 빠르고 exact 복귀는 frame cache가 6~7ms로 처리한다. world-aligned
tile LRU(F2R-10)의 착수 판정은 관찰이 나온 실칩 trace에서 같은 sweep을
재측정한 뒤 내린다.

### 3.13 실칩 GUI 첫 관측 — deep-zoom cold 뷰 (2026-08-27)

실칩에서 두 제품 GUI를 depth 99(full), detail high로 열고
`goto 775,1125,2`(view 2.0 x 1.9µm)로 이동한 cold 측정. 같은 working set
(plan 246 pages, +194 new)에서:

| 제품 | total | load | draw |
|---|---:|---:|---:|
| floe/KLayout | 1048ms | 906 (plan+delta+apply) | 142 |
| floe2 | 564ms | 71 (plan+read+decode+scene) | 439 |

- **load 축은 F2R-10 재구성(§F2R-10 관찰)이 실칩에서 확정**: 같은 194 새
  page에 대해 KLayout delta authoring+apply 906ms vs Rust read+decode
  71ms, **12.8배**. end-to-end로 floe2가 1.9배 빠르다.
- **draw 축은 반대로 3.1배 열세**(439 vs 142ms, floe2는 r4 병렬 기준) —
  깊은 zoom + full depth에서 tile×plane hierarchy 재순회가 지배한다는
  F2R-03b 2단계 가설의 실칩 확인이다. sample9 hotspot(§3.12)에서는 이미
  KLayout보다 빠르므로, 격차는 record 상수가 아니라 traversal 곱셈 쪽이다.
  2b(cell×layer mask)가 이 regime를 직접 겨냥하고, 2c(tile×plane work
  bin) 착수 판정에는 같은 뷰의 bench `--perf-baseline` phase/telemetry
  (`wc_cells`/`inst_edges`/`hier_cells_visited`/`subtrees_pruned`)가
  필요하다.

**2b 반영 재측정(2026-08-27, 같은 뷰)**: floe2 **149ms = 72 load +
22 draw**. draw 439→22ms(-95%)로 traversal 곱셈 가설이 실칩에서
증명됐고, KLayout draw 142ms 대비 **6.5배 우위**로 뒤집혔다.
end-to-end 1048 vs 149ms — 7배. load는 예상대로 불변(71→72ms)이며
이제 이 뷰의 지배 phase다(48%; 나머지 ~55ms는 png/publish/wait).
판정: **2c work bin은 이 regime에서는 근거를 잃었다** — draw 잔량
22ms로, §F2R-03 2c의 "traversal 비중이 낮으면 F2R-11 채택 시점까지
미룬다" 조건에 해당한다. mid-zoom(넓은 span, wc_cells 수천)에서 draw가
다시 불거지는지만 후속 확인으로 남긴다. 다음 우선 축은 load(신규
page read+decode)와 재방문 load 잔량(F2R-10 pan-sweep)이다.

**실칩 추가 관측(2026-08-27, 정밀 수치 없음 — 재측정 대기)**: 여러
viewport 비교 중 특정 지점의 ~100µm 뷰에서 **load 10초+, draw 5초+**,
refinement 다회 발생. 대부분 floe보다 빠르지만 비슷한 지점들이 있었고
그런 경우 draw가 floe보다 느렸다. decode 8-thread가 항상 이상적으로
돌지 않는 듯한 느낌 보고. 가설 후보: (a) decode CPU의 index-build
비중(§3.11 +70%, §3.14 sample9 41%), (b) decode straggler/pool idle,
(c) 거대 working set의 refinement 다회 왕복, (d) mid-zoom traversal
잔량(2c 재개 조건), (e) r4 tile tail imbalance(F2R-06). §3.14의
telemetry가 이들을 분리 판정한다.

같은 세션에서 **~100µm 뷰 헤어라인 표시 상이**도 관측됐다. 재현·근원
확정은 RENDERER-TESTS.ko.md 픽셀 정책 §헤어라인 스케일 실측 참조 —
P-a 1px 밴드 계약 안(내부 diff 0)이지만 sub-pixel 도형을 KLayout은
1px로 collapse, Rust는 걸친 픽셀 전부를 점등해 mid-zoom 질감이
달라진다. **수렴안(A) 채택·구현(2026-08-27)**: KLayout의 collapse
규칙(점=round(center)·y−1 bias, wire=edge round 쌍)을 실측해 sub-pixel
member를 fill+stroke 파이프라인 진입 없이 collapse된 픽셀로 그린다.
점 300개 KLayout과 동수·95% 픽셀 일치. 이 fast path는 hairline
regime의 member당 비용도 제거하므로 mid-zoom draw 열세(위 관측)의
개선 후보이기도 하다 — 실칩 재측정으로 확인한다.

### 3.14 진단 telemetry (2026-08-27, 0.12.15)

위 관측을 분리 판정하기 위해 renderd frame 라인→어댑터→GUI perf
라인·`bench_floe2.py` JSON까지 다음 필드를 관통시켰다. GUI는 rust
구간에 `rounds N, dec sum/max/idx ms, tile-max ms, hier 방문/prune`
형태로 덧붙인다(모두 generation 누적, max류는 max).

| 필드 | 답하는 질문 |
|---|---|
| `rounds` | refinement가 몇 round 돌았나 (다회 왕복 = c) |
| `decode_sum_ms` vs `decode_ms`×`decode_workers` | pool 유휴율 (b) — 같으면 완전 가동 |
| `decode_max_ms` | 최다 소요 단일 page = straggler (b) |
| `index_ms` | decode CPU 중 record-index build 몫 (a; lazy-index 판정 입력) |
| `raster_tile_max_ms` | 최다 소요 image tile = r4 tail imbalance (e; F2R-06 gate) |
| `hier_cells_visited`/`subtrees_pruned` | traversal 규모/2b 절감 (d; 2c 재개 판정) |
| `rep_members_tested/drawn` | member 스캔 vs 실제 paint (d) |
| `mask_mb` | 2b subtree mask 실크기 (16MiB 상한 초과 시 full-mask 폴백 = prune 없음·pixel 불변; 리뷰 2026-08-28) |

리뷰 보강 2건(2026-08-28): (1) 2b mask 행렬(wcells × layer word)은
예산 밖 무제한 할당이었다 — checked 산술 + 16MiB 상한, 초과 시
full-mask 폴백(prune만 잃고 pixel 불변), `mask_mb`로 계측. (2)
record-index build(decode CPU의 41%)가 비취소 구간이었다 — 4096
record/chunk마다 취소 probe를 넣어 pan 후 stale generation이 큰
page 안에서 CPU를 계속 태우지 않는다.

sample9 hotspot(r4/384, detail high) 첫 판독: decode pool 가동률
92%(232.2 sum / 31.4 wall × 8w), straggler 없음(max 12.0ms),
**index build가 decode CPU의 41%**(96.0/232.2ms — lazy-index 후보
근거), raster 19.8ms 중 **tile-max 17.7ms(wall의 90%)** — r4 tail
imbalance의 첫 실측 신호(F2R-06 재개 조건 충족 여부는 실칩에서 확인).

### 3.15 실칩 문제 뷰 판정 — mid-zoom 100µm cold (2026-08-28)

**최종 재측정(같은 날, 0.12.22 기본 no-refinement)**: floe2
**2,919ms = 326 load + 2,473 draw** — 최초 11,656ms에서 4배, floe
6,220ms 대비 **2.1배 빠름**. draw 2,473 vs KLayout 2,252로 사실상
동률까지 수렴했다. 남은 신호: `bin off(cap@786k)` — work bin이 item
상한(768k)에 걸려 walk 폴백(수집 도달치 표시가 상한과 같아 실제
총량은 미지), tile-max 949ms(r4 tail — wall의 38%, F2R-06),
hier 1.8M/60.5M(walk 폴백의 per-tile gate). 잔여 후보의 기대 이득:
① dense-rep 지연 전개로 bin 적중(추정 −0.3~0.6초), ② tile 크기
축소로 tail 완화(FLOE_RUST_TILE_PX 실측으로 판정), ③ F2R-03c 1bpp
plane(구조적 per-member 상수). 경과 요약:
11.66s → 5.00s(cost-aware 2 round) → **2.92s**(단발 round).

**tile sweep 판정(같은 날)**: FLOE_RUST_TILE_PX 384/192/128 → draw
2.47/4.78/8.78초, pruned 60.5/155.1/311.3M — walk 모드 traversal이
tile 수(9/25/49)에 정확히 비례한다. ② adaptive tile은 이 regime에서
**기각**(역효과), 대신 회귀선에서 tile당 traversal ≈144ms →
**384px에서 draw의 ~53%(1.3초)가 traversal**로 확정. 이에 따라 ①을
구현했다: `Rep.members() > 4096`인 instance는 수집 시 전개하지 않고
plane별 Deferred item으로 남겨 tile이 기존 walk 코드로 지역
전개한다(2b per-plane gate 그대로, byte 동일 oracle로 고정 — dense
70×70 grid에서 bin item < 100, cap 미달, pixel 동일). 기대: bin
적중 시 draw 2.47→~1.2초. 실칩 `bin N items` 확인 대기.

**실칩 2차 피드백**: member 임계만으로는 여전히 `bin off(cap@786k)`
— pruned 60.5M 역산 결과 tile·plane당 ~15만 개의 **평평한 instance
fanout**(개별 배치, members=1)이 원인이라 per-(visit,page) item이
상한을 채웠다. 보강 2건: (1) item을 **(visit, plane) 단위로 통합**
— page 스캔은 tile 소비 시 walk과 같은 코드로 수행, 150k-edge
구조에서 item이 visit 규모로 떨어진다. (2) 지연 판정을 members가
아니라 **members × subtree item weight**(SceneMasks가 post-order로
계산, cycle은 포화→전량 지연)로 바꿔 dense rep·깊은 곱·혼합 어느
형태도 수집을 부풀릴 수 없다. byte 동일 oracle 유지(92 tests).
실칩 3차 확인 대기.

**비교 조건 정정(실칩 3차 정보)**: floe2 측정은 전부 frame off였고,
**floe도 frame off로 맞추면 draw 2,252→1,101ms** — 동일 조건에서
floe2 draw(2,496ms)는 아직 **2.3배 열세**다(total은 floe2 2.9s vs
floe ~5.1s로 여전히 우세). 이 1,101ms는 §3.15 분해의 floe2 paint
몫(~1.2s)과 일치하므로, **bin 적중 시 draw 동률**이 기대
시나리오이고 그 이하는 F2R-03c 영역이다. 별개로 이 조사에서
`render_prepared_labels`의 스캔 배수를 발견해 수정했다 — 호출마다
전체 row를 선별 검사(207k row × (tile 9 × plane ~40 + block pass)
≈ 78M/frame)하던 것을 build 시 selection별(layer/block) 그룹으로
나눠 자기 그룹만 순회한다(그룹 내 순서 = row 순서 → byte 불변).
labels on 뷰의 label 비용이 크게 줄어든다.

**실칩 4차 — bin 적중 확인(0.12.25)**: `bin 137k items`,
**1,667ms = 236 load + 1,296 draw**. draw 2,496→1,296ms(-48%)로
예측(~1.2초)과 일치하고, frames-off floe draw 1,101ms와 18% 이내다.
잔여 hier 1.6M/23.5M은 weight-gate가 지연한 heavy subtree들의
per-tile walk 몫.

**frames 경로 감사(2026-08-28, 사용자 요청)**: full depth에서 frame
on의 잔여 비용은 plan +22ms(vfsd frontier 계산, floe 공통)뿐 — band
walk 4회는 2b `subtree_has_frames(top)` gate가 스킵하고 bin frames는
빈 리스트, block label은 labels 전용 + 0.12.25 그룹화. **frame off
테스트 습관은 이제 불필요**(격차가 크게 나오면 버그 신호). depth
제한 뷰도 planner의 frame fusion(flatfan 10만 자식 → frame record
1개)·thin lattice가 폭발을 막는다(sample9 depth 4/6: +70µs).
이론적 잔여 2건 — bin FrameItem의 4-band 전수 스캔, band-3 dotted
hairline의 per-pixel stroke — 는 **depth 제한 실칩 뷰에서 frames
on/off draw 격차가 유의미할 때만** 착수한다. 이 뷰의 최종 경과: **11,656 → 5,001 → 2,919 →
1,667ms** (동일 토글 floe ~5.1s 대비 3.1배, 원점 대비 7배). 남은
draw 격차 ~0.2초의 다음 지렛대는 F2R-03c(1bpp plane)와 deferred
subtree의 tile binning이며, 우선순위는 다른 축(F2R-10 재방문
sweep 등) 실측 후 판단한다. **[갱신 2026-09-02: deferred-subtree
지렛대(§3.17, 0.12.32)로 이 뷰는 894ms까지 내려갔다 — 원점 대비
13배, floe 대비 5.7배, draw는 498ms로 frames-off floe(1,101ms)의
2.2배 우세. 잔여 격차는 소멸.]**

§3.13의 "load 10초+/draw 5초+" 지점을 0.12.19 telemetry로 실측한
결과 (view 100.0×96.6µm, 824x796, cut<0.122µm, 5698 pages/+5616
miss, 207k text places, labels partial):

| | total | load | draw |
|---|---:|---:|---:|
| floe/KLayout | 6220ms | 3967 (plan 1092+apply 2860) | 2252 (~12.9M drawn) |
| floe2 | 11656ms | **417** (plan 217+read 27+apply 173) | **10918** |

**load 축은 이 뷰에서도 floe2 완승(9.5배)** — 5616 page decode CPU
합 654ms(8w wall ≈82ms), straggler 없음(max 29ms), **index build는
16%(107ms)로 병목 아님** — lazy-index·인코드 캐시·budget 이슈 전부
이 뷰에선 해당 없음. read 27ms → OS page cache 가설도 무해 확인.

**draw 10.9초의 분해 — 두 개의 곱셈**:

1. **refinement 왕복 ×~3**: +5616 miss > round_pages 1024 → rounds
   5. 각 round가 누적 scene 전체를 재raster하므로 총 raster 작업량은
   단발 draw의 약 (1+…+5)/5 ≈ 3배. 단발이면 draw ≈3.6초, total
   ≈4.3초로 **floe(6.2초)보다 빨랐을 일**이다. round당 ~2.2초짜리
   중간 frame은 진행 표시 가치보다 비용이 크다 — F2R-09 정책(1024)의
   실칩 재조정 필요: raster 비용을 본 뒤 중간 round를 접는 cost-aware
   변형이 1순위 지렛대.
2. **tile×plane traversal ×45**: `hier 7.5M visited / 295.7M
   pruned` — 2b mask가 에지의 97.5%를 자르고 있음에도 gate 검사
   자체가 ~3억 회다. round×tile(45)×plane 곱 때문이며, 단일 walk당
   에지는 ~수만 개로 추정된다. **2c work bin 재개 조건이 이 뷰에서
   충족**됐다(§F2R-03 2c: mid-zoom에서 draw가 다시 불거지면 재개).
   부수: mask gate의 wcells 이진탐색을 인스턴스별 사전 해석 index로
   바꾸면 2c 이전에도 gate 상수를 줄일 수 있다.

잔여 신호: tile-max 979ms(r4 tail, F2R-06 관찰 지속), png 206ms.
per-member로는 KLayout ~175ns/member(12.9M/2252ms) vs floe2 추정
~280ns(누적 ~39M member-paint/10918ms) — round 정리 후 남는 격차는
2c·F2R-03c(1bpp plane) 영역이다.

### 3.16 실칩 pan 실측 — F2R-10 종결과 F2R-13 착수 (2026-09-01~02)

**실칩 pan 실측(2026-09-01, 사용자)**: 같은 뷰에서 상하좌우로 화면의
20%씩 이동하는 pan에서 **floe2 draw가 floe보다 약간 빠름**. §3.12
sample9 sweep과 부호가 일치하며, 이것으로 F2R-10의 두 판정 축이 모두
실칩에서 닫혔다 — load 축은 §3.15(9.5배 우세), draw 축은 이 실측.
최초 현장 관찰("인접 pan 열세")의 재현 실패가 실칩에서 확정됐다.

**world-tile LRU 기각(2026-09-01 코드 판정)**: 경쟁 축이 닫힌 뒤
남는 동기는 절대 지연(20% pan에서 픽셀의 ~80%가 재사용 가능한데
전체 viewport를 다시 raster)뿐인데, 구현 검토 결과 RGBA tile 캐시는
**자체 수용 gate(byte 동일)를 통과할 수 없다** — fill 위상이
device-anchored라(checkerboard는 device (x+y) parity, KLayout stipple
은 framebuffer height·device row로 위상화 — `LayerFill` 계약) 임의
pan 뒤 재사용 tile은 fresh raster와 stipple 위상이 어긋난다.
byte-exact 재사용은 world-순수 중간 표현(plane별 1bpp coverage/
outline mask)을 캐시하고 합성 시 device 위상 fill을 다시 적용하는
구조여야 하며, 이는 **F2R-03c(1bpp plane) 재작업이 선행**이라는
뜻이다. F2R-03c 착수 기준이 미충족이므로 world-tile도 함께 보류.
재개 조건: F2R-03c가 실측 기준으로 착수되고 pan 절대 지연이 실칩
UX 문제로 지목될 때. WEBUI_PLAN의 T3(world-tile delta 전송)도 같은
조건에 묶인다.

**F2R-13 착수(2026-09-02)**: 남은 무조건부 지렛대 중 최대는
인터랙티브 frame 왕복의 PNG 코덱이다. §3.15 최종 라인(1,667ms)의
분해에서 raster(~1.1s)는 이미 frames-off floe draw(1,101ms)와
동률이고, 잔여 ~135ms(png 인코드+publish+adapter read)와 GUI 주
스레드의 PixbufLoader PNG 디코드(미계측, 수십 ms급)가 매 인터랙티브
frame의 고정 비용이다(0.12.19 최초 계측에선 png 인코드만 206ms).
구현(0.12.26): renderd가 `frame_format=raw`에서 PNG 인코드 대신
`FLOERAW1` 헤더(magic+u32le w/h)+packed RGBA를 같은 원자적 publish
계약으로 게시하고, GUI는 `Pixbuf.new_from_bytes`로 디코드 없이
표시한다. 상세와 gate는 §F2R-13. 기대: 매 pan/zoom/style frame에서
~0.1~0.2초 절감 + 주 스레드 디코드 제거. **실칩 재측정 대기** —
perf 라인이 `png` 대신 `raw N.Nms`를 찍으면 새 경로다.

**F2R-13 실칩 검증(2026-09-02, 0.12.26)**: 같은 문제 뷰 cold —
**1,729ms = 341 load [207 plan+24 delta+110 apply] + 1,284 draw,
`raw 0.8ms/pub 8.4ms`**, text 73.5ms/207k places, tile-max 580ms,
bin 137k, hier 1.6M/23.5M. raw 경로 확정(인코드 0.8ms), draw
1,296→1,284로 raster 무회귀. 총시간이 1,667 대비 +62ms인 것은 load
+105ms(plan 207ms, 실행별 변동)이고 draw+잔여는 −43ms.

이 라인으로 잔여 산식이 풀렸다 — **기대치 정정**: 위 착수 문단의
"잔여 ~135ms(png+publish+read)"는 과대였다. 잔여의 대부분은 label
**text plan(~74ms — load_ms에 불포함, wall에는 포함)**이었고, 이전
png 인코드는 0.12.19의 206ms가 5-round 누적이라 단발 round 기준
~41ms + pub ~10ms였다. 따라서 renderd측 wall 절감은 frame당
**~50ms**이고, 나머지 몫(GUI 주 스레드 PixbufLoader 디코드 제거,
824x796에서 수십 ms급)은 wire 숫자에 보이지 않는 응답성 개선이다.
남은 draw 격차(1,284 vs floe frames-off 1,101)는 raster 자체
(draw_ms는 순수 raster)로, 문서대로 F2R-03c 영역이 맞다.

**pan 1步 실측(같은 날, ctrl+커서 1회)**: 1,512ms = 182 load
[149 plan+2 delta+32 apply] + 1,177 draw, **+1 new**(dec sum 17ms),
text 88ms/192k places, tile-max 520ms, bin 137k. 판정: decoded LRU가
인접 pan에서 완벽 적중(+1 page)하므로 pan 비용의 실체는 ① 전체
viewport 재raster ~1.18초(137k item, F2R-10 종결 판정대로 byte-exact
재사용은 F2R-03c 선행), ② **plan+text plan 재실행 ~237ms**(총시간의
16%)다. floe와 동률 유지(사용자 선행 실측 "약간 빠름")이므로 현재
gated 항목의 트리거는 여전히 미발화. pan UX를 절대 기준으로 더
줄이라는 요구가 생기면 착수 순서는 ②(인접 뷰의 plan/text-plan
재사용 — F2R-03c보다 훨씬 작은 작업) → ①(F2R-03c+world-tile mask)
순이다. tile-max 580/520ms(wall의 45%)는 F2R-06 tail 관찰 지속.

### 3.17 실칩 depth 제한 뷰 — deferred-subtree 지렛대 발화 (2026-09-02)

측정(detail high, depth 3, goto 3092,2013, view 100µm, 2,591 pages
cold, 사용자):

| | frames on | frames off |
|---|---|---|
| floe | 5,384 = 3,639 load + 1,745 draw (plan 822.5ms/19k) | 4,602 = 3,609 load + 993 draw (plan 665.6ms/0) |
| floe2 | 2,630 = 457 load + 2,050 draw (plan 320.9ms/18k) | 2,413 = 455 load + 1,929 draw (plan 318.5ms/18k) |

판정 5건:

1. **total은 floe2 1.9배 우세 유지**(양 토글 공통). load 축 8배.
2. **frames(+labels) 비용 감사 재확인**: floe2 draw +121ms/plan
   +2.4ms vs floe draw +752ms/plan +157ms — depth 제한 뷰에서도
   frames on이 사실상 공짜다(+121ms의 대부분은 467k-place label).
   frames off 테스트 습관 불필요 결론이 depth 제한에서도 성립.
3. **draw 축 1.94배 열세(1,929 vs 993) — deferred-subtree 지렛대
   발화**: bin이 3,018 item뿐인데 hier 930k/60.8M(mid-zoom 23.5M의
   2.6배). scene은 이미 depth-3로 잘려 있으므로 이것은 depth 과대
   추정이 아니라, **members=1 대형 블록(scene 기준 weight>4096)이
   지연되어 tile마다 per-plane 재walk를 반복**(≈9 커버)한 것.
   §F2R-03c에 적힌 "먼저 소진할 지렛대"의 실측 조건 충족.
4. **조치(0.12.27)**: 지연 gate를 분리 — 반복(members>1)은 기존
   `members×weight>4096`(수집 시간 보호) 유지, **단일 배치
   (members=1)는 잔여 item 예산(×4 안전계수) 안에서 항상 전개** —
   수집 walk 1회가 tile-walk ×tiles×planes보다 항상 싸다. 예산
   초과 시 지연(전 frame walk 폴백 방지). byte 동일은 정책 변경이라
   기존 oracle이 그대로 보증하며, 신규 정책 테스트(4,550-inst 단일
   배치 블록: 전개, hier가 tile 수 무관, cap 미달, pixel 동일)로
   고정. 기대: 이 뷰 hier 930k→~10만(1 커버), draw 감소. mid-zoom
   full-depth 뷰의 잔여 hier 1.6M/23.5M도 같은 경로로 준다. 실칩
   재측정 후 잔여 격차가 paint 지배면 F2R-03c 트리거를 재판정한다.
5. **"frames off인데 18k frames" 질문**: 그 숫자는 그려진 frame이
   아니라 **planner의 depth-frontier record 수**다 — floe2 planner는
   frames 토글과 무관하게 depth cut 계산의 산출물로 얻는다(on/off
   plan 차이 +2.4ms가 증거). floe는 frames off에서 frontier 계산을
   생략해 0을 찍는다(plan 665 vs 822ms). GUI 라벨을 `frames` →
   `frontier`로 바꿔 혼동을 제거했다(그리기 여부는 여전히 frames
   토글이 결정).

잔여 신호: tile-max 714ms(draw wall의 37%, F2R-06 관찰 지속),
dec sum 776/idx 125ms(decode 비병목 유지).

**재측정(2026-09-02, 0.12.27) — null**: 2,496ms = 490 load + 1,978
draw, bin 3,017 items, hier 923k/60.8M — 항목 4의 members=1 전개
gate가 이 뷰에서 **전혀 발화하지 않았다**(bin/hier/draw 불변). 남은
가설 두 가지: (a) 지연 원인이 반복 배열(members>1 — 기존 gate가
그대로 지연), (b) 단일 블록이지만 weight×4가 잔여 예산을 초과(예산
계수 과보수). 추측으로 정책을 재변경하는 대신 **지연 원인
telemetry(0.12.28)**를 추가했다: perf 라인 bin 세그먼트가
`(defer Nr+Ns wNNN)` — r=반복 gate 지연 에지 수, s=단일 예산 지연
에지 수, w=지연된 최대 단일 weight — 를 찍고, 새 `paints N`(rect+
polygon+path+frame member paint 합)이 paint 지배 여부(F2R-03c 축)를
같은 라인에서 보여준다. **다음 한 줄로 판정한다**: defer가 r
지배 → 반복 전개를 예산 gate로 통합하는 설계(수집 serial 대
tile-parallel 트레이드오프 포함), s 지배 → 예산 계수 조정(즉시),
둘 다 소수인데 paints가 크면(floe ~12.9M급 이상) → F2R-03c 트리거
재판정.

**원인 확정(2026-09-02, 0.12.28 라인)**: `defer 1r+0s w0, paints
1.2M` — **r 지배, 그것도 단 1개의 반복 배열 에지**가 depth-3
콘텐츠 전체를 들고 있고, 그 하나의 per-tile×plane 재전개가 hier
923k/60.8M의 전부다. paints 1.2M은 draw의 ~0.1s 몫 — **이 뷰의
1.94배 열세는 paint가 아니라 순수 traversal**이므로 F2R-03c는 이
뷰와 무관 확정.

**조치(0.12.29) — 지연 gate 통합**: 판정을 "**보이는 member 수 ×
weight ×4(다중 plane 안전계수) ≤ 잔여 item 예산**" 하나로
단일화했다. 100만-member 배열이라도 화면에 3개면 3개로 계산하고,
member 카운트는 예산을 넘는 순간 조기 중단해 거대 가시 배열도
O(예산)으로 판정한다. members>1 특례(4096 곱 gate)는 제거 — 전개는
수집에서 1회 걷는 반면 지연은 tile×plane마다 다시 걷므로, item
예산 안에서는 전개가 무조건 싸다. 예산 초과 시에만 지연(전-frame
walk 폴백 방지). 순수 정책이라 byte 불변이 기존 oracle로 보증되며
테스트를 재고정(94개): 70×70 dense grid는 이제 **전개**(예산 내),
60×60×weight-64(투영 230k)는 **지연**(예산 초과, 조기 중단 카운트).
기대: 이 뷰의 배열이 전개되어 hier 923k→~10만(수집 1커버), draw
1,978→~1.0-1.2s(floe 993 동률권). 실칩 재측정 대기.

**재측정 2차(0.12.29) — 다시 null, 원인은 투영 자체**: `defer
1r+0s`, hier 923k 불변 — 가시-member×weight 투영조차 예산 초과로
판정됐다. weight 계산(§F2R-03 2b SceneMasks)은 post-order로 전
cell을 덮는 정상 구현이므로, 남는 설명은 **중첩 반복의 구조적
과대계상**이다: weight는 하위 rep의 **전체** member 수를 곱해
올라가는 반면 실제 walk는 view-cull로 그 일부만 걷는다(×4 안전계수
과보수 가능성 중첩). 두 번의 예측 실패로 **예측을 버리고 측정으로
전환했다(0.12.30)**: fast gate가 실패해도 즉시 지연하지 않고 **soft
limit(잔여 cap의 절반, 전-frame 폴백 cap보다 항상 아래)을 건 실제
전개(trial)**를 수행하고, 정말 초과한 에지만 bin과 DFS 경로를
정확히 롤백한 뒤 지연한다. 가시 member 수 자체가 soft cap을 넘는
확실-초과는 trial 없이 지연(조기 중단 카운트, member당 최소 1
item이 보장되므로 보수적으로 성립). 수집은 단일 스레드·DFS 순서
결정적이라 결과가 jobs/tile 수와 무관하고 byte 동일이 유지된다.
테스트 95개: 투영 230k/실측 234k grid는 trial로 **전개**, 실측
448k grid는 **롤백 후 지연**(둘 다 byte 동일). 기대: 실칩 배열의
실측 item이 soft cap(≈38만) 이하라면 전개 — perf 라인 `defer
0r+0s` 확인. **그래도 `1r`이면** 실제 전개가 38만 item을 넘는
규모라는 뜻이며, 그때는 예산 조정이 아니라 tile-side 결합
walk(지연 에지의 plane 곱 제거)로 간다.

**재측정 3차(0.12.30) — 구조 확정**: `bin 7,521 items (defer 0r+1s
w610k), hier 1.0M, draw 2,067`. 배열 에지는 trial로 **전개
성공**(0r, bin 3,017→7,521). 남은 지연은 **members=1 단일 배치
하나** — 정적 weight 610k, trial이 soft limit(잔여의 1/2 ≈ 38만
item)까지 실제로 걷고 초과로 롤백했다. hier(~11.5만 visit/커버)와
대조하면 이 에지의 실체는 **~10만 visit × plane당 item 팽창
~4배 ≈ 40만 item** — 전체 cap(768k)에는 충분히 들어가는데 절반
soft limit이 막은 것이다. draw +89ms는 매 frame 반복되는 실패
trial의 낭비이며, 동시에 이 40만-item walk 자체가 ~100ms짜리로
싸다는 실측이기도 하다(수집 visit은 paint 없이 stamp 스캔뿐).
**조치(0.12.31)**: soft limit을 잔여의 1/2 → **7/8**(1/8은 형제
에지 예약, 전-frame 폴백 cap보다 항상 아래)로 상향. 이 에지가
커밋되고 실패-trial 낭비도 사라진다. 롤백 안전성 테스트는 704k
실측 grid로 재고정(95 tests). 기대: `defer 0r+0s`, hier
1.0M→~11.5만(수집 1커버), draw 2,067→**~0.7-1.1s**(paint 몫
~0.1s + 수집 ~0.2s + item 소비/decode). floe 993 동률권 진입 여부
확인.

**재측정 4차(0.12.31) — 상한 확정, 분기 실행**: `defer 0r+1s
w610k` 불변 — 7/8(≈68.8만 item)까지 실제로 걷고도 초과. 이
에지의 실제 item은 ~70만+로 **정적 weight(610k)보다 크고**(frontier
cell이 여러 plane에 걸치는 per-(visit,plane) item 팽창), 전체
cap(768k)으로도 못 담는 규모다. 예산 게임은 여기서 끝 — 문서에
박아둔 분기대로 **tile-side 결합 walk를 구현했다(0.12.32)**:
수집이 지연 에지를 `DeferredEdge` 레코드로 공유하고, tile은 자기
view로 cull된 **mini 수집 walk를 에지당 1회**(기존 collect_cell
재사용, trial 없음) 실행해 per-plane/frame 소비 시 mini의 DFS
목록을 지연 item의 슬롯에서 재생한다 — plane별 hierarchy
재walk(×planes)가 사라지고, tile 간 중복은 view culling으로 top
하강부만 남는다. mini가 자체 cap을 넘거나 내부 재지연이 남으면
기존 per-plane walk 폴백(byte 불변). trial soft limit은 1/2로
환원(지연이 싸져 긴 헛trial이 손해). 검증: 95 tests — 롤백
oracle을 2-layer+frames+tile8 구성으로 강화(다중 plane 재생 순서,
frame band 재생, hier 감소 assert 포함 byte 동일). 기대: 이 뷰
hier 923k(plane-곱 재walk) → 결합 1커버+top 하강(~15-25만),
draw 감소 — 폭은 다음 라인으로 판정하고, 남은 격차는
paints/decode 축으로 재분해한다.

**최종 판정(2026-09-02, 0.12.32 실칩)**: **833ms = 475 load + 332
draw**, bin 7,521, defer 0r+1s(설계대로 — cap 초과 에지는 mini가
소화), paints 1.2M 불변, **hier 424k/276k pruned** — gate 검사
60.8M→276k(220배), draw 2,067→**332ms(6.2배)**. 이 depth-3 뷰의
경과: draw 1,929~2,115 → 332ms로 **floe(993ms) 대비 3.0배 우세**,
total 2,413~2,496 → 833ms로 **floe(4,602ms) 대비 5.5배 우세**.
F2R-03 2c의 deferred-subtree 지렛대는 이것으로 **소진 완료**다.

남은 신호 두 가지(둘 다 비긴급): ① 실패 trial ~90ms/frame(1/2
limit까지 걷고 롤백하는 수집 몫 — draw 332의 ~27%)은 trial limit
축소로 줄일 수 있으나 mid-size 에지의 전개 이득과 상충해 실측
요구가 생길 때만 조정. ② 이제 이 뷰의 최대 단일 성분은 **plan
~350ms**(load 475의 대부분) — pan 실측(§3.16)의 plan+text 재실행
~237ms와 같은 축이며, 인접 뷰 plan 재사용이 다음 후보라는 판정을
재확인한다. 별개로 mid-zoom full-depth 뷰(§3.15, hier 1.6M
잔여)도 mini walk의 수혜가 예상되므로 재측정 1회 권장.

**spillover 확인 — §3.15 문제 뷰(2026-09-02, 0.12.32)**: 같은
빌드로 mid-zoom full-depth 뷰 재측정 —

| | total | load | draw | 신호 |
|---|---:|---:|---:|---|
| frames off | 894ms | 364 | **498** | bin 154k, defer 2r(w447k, mini 소화), paints 1.9M, hier 331k/**8,951** |
| frames on | 964ms | 351 | 507 | 동일 (draw +9ms — frames 무료 재확인) |

hier 1.6M/23.5M → 331k/8,951(gate 2,600배 감소), draw
1,284→498ms(2.6배). **원점 11,656 → 894ms(13배)**; 동일 토글 floe
~5.1s 대비 **5.7배**, draw는 frames-off floe 1,101ms 대비 **2.2배
우세**(0.12.25 시점의 "18% 열세"는 우세로 반전). frames on도
964ms로 floe 6,220 대비 6.5배. 이로써 추적한 두 실칩 뷰(mid-zoom
full-depth·depth-3)와 pan 축 전부에서 floe 대비 우세가 확정됐고,
2c는 두 뷰 모두에서 완결이다. 남은 공통 최대 성분은 plan
(216~350ms) — 인접 뷰 plan 재사용 후보 하나로 수렴.

### 3.18 장시간 세션의 load 증가 — LRU 축출 전수 스캔 (2026-09-02)

**관찰(사용자)**: detail medium에서 depth/zoom 무관 대부분 뷰가
300~800ms(load 지배)로 끝나는데, **미니맵으로 불규칙하게 이동**하다
보면 ~1,000ms까지 늘고(대부분 load 증가) 드물게 2,000ms+도 발생.
**같은 위치를 새 뷰어로 열어 바로 가면 200~300ms.**

**진단**: 새 뷰어가 더 빠르다는 게 결정적 — 신규 뷰어도 cold
decode를 전부 지불하므로 LRU miss/재decode만으로는 설명 불가, 즉
**세션 크기에 비례하는 오버헤드**다. 코드 확인 결과
`DecodedPageCache::evict_to_fit`이 **축출 1회당 HashMap 전수
min-스캔**을 돌았다: 예산(1024MB)이 가득 찬 세션에서 miss가 m개인
load는 O(m × 상주 n). 구조 시뮬레이션(상주 4만 page, miss 3천):
전수 스캔 **237ms** vs 색인 0.24ms — 관측된 +0.7~1.7초와 부합하고,
미니맵 불규칙 이동(m 큼)·만석 세션(n 큼)·fresh 뷰어(n≈0)의 세
증상을 모두 설명한다.

**수정(0.12.33)**: `(last_used, page_id)` BTreeMap 색인을
entries와 병행 유지해 축출을 **O(log n)** first-key로 바꿨다.
피해자 선정 순서는 기존 전수 스캔과 완전 동일(LRU→page id
tie-break)이라 상주 집합·telemetry·픽셀 전부 불변. 일관성
oracle(터치/재삽입/축출/budget 축소 churn에서 색인-엔트리 동기)
추가, render-core 96 tests. **telemetry**: perf 라인에 `evict N`
(이번 frame에 축출된 page 수, 0이면 생략) 추가 — nonzero가 계속
찍히면 working set이 budget을 넘는 세션이라는 신호로,
`FLOE_RUST_BUDGET_MB` opt-in 판단 자료가 된다(§F2R-10 운영 정책
유지: 기본 1024 고정).

**재확인 요청**: 오래 쓴 세션에서 미니맵 이동 시 load가 새 뷰어
수준으로 유지되는지, 2,000ms+ 스파이크가 사라지는지. `evict`가
크게 찍히는 위치는 budget 초과 재decode가 본체이므로 그건 수정
대상이 아니라 budget 정책 영역이다.

### 3.19 pick이 거의 안 됨 — 질의 예산 기아 (2026-09-02)

**관찰(사용자)**: "오브젝트 picking이 거의 되지 않음 — 큰
오브젝트도 대부분 안 잡히고 'no object here'." 결정적 재현
(sample9, depth 9, view 8318×7775µm): layer 36/38의 긴 막대는 pick
불가, layer 39의 두 막대는 가능; **모든 layer를 끄고 36/38/39만
켜면 전부 pick됨**.

경과: 1차 진단(detail cut 아래 geometry가 scene에서 실종 —
0.12.34의 cut-free micro-plan 질의)은 실재하는 결함이지만 주
증상이 아니었고 사용자 지시로 **원복**(0.12.35). sample9로 두 축을
분리 재현한 결과 **서로 다른 결함 2건**이 겹쳐 있었다:

- **결함 A — 질의 예산 기아(주범, 0.12.35에서 수정)**: pick/snap의
  member 예산(400)이 켜진 **모든 layer의 walk에 전역 공유**되고,
  `Rep::One`·Pts 스캔이 **가시성 검사 전에** 예산을 소모했다 —
  record 밀집 page 한두 개만 방문해도 클릭 근처도 아닌 도형들이
  400을 소진하고, 소진 시 QUERY_STOP이 **조용히 빈 결과로
  변환**되어 뒤 순서 layer의 막대는 검사조차 안 됐다. layer를 3개만
  켜면 walk가 작아 예산이 살아남는다 — 사용자의 토글 관찰과 정확히
  일치. sample9 재현: **같은 cut=0 scene에서 pick layers=all은 0/3,
  subset은 3/3 → 수정 후 양쪽 3/3.** 수정: ① 예산을 **query 영역
  안에 실제로 들어온 member만** 계상(영역 밖 스캔은 무료, 취소
  heartbeat 분리 유지), ② record Pts에 2a chunk index를 query
  경로에도 연결(스캔 자체를 prune), ③ 상한 400→4,096
  (SNAP_SHAPE_CAP 동일). 회귀 테스트 2건(visible-only 계상,
  밀집-원거리 layer가 예산을 굶기지 못함) 고정, render-core 97
  tests.
- **결함 B — detail cut 실명(실재, 보류)**: sample9에서도 확인 —
  layer 36 막대가 같은 좌표·같은 layer에서 cut=0이면 잡히고
  cut=3px(≈30µm)면 안 잡힌다(sub-cut 폭 도형은 wash로 그려질 뿐
  scene에 없음). 0.12.34가 이걸 고치는 수정이었으나 원복했고, 재론
  시 커밋 219259c에 설계·검증(micro-plan + cut-render parity)이
  있다. detail이 굵은 뷰에서 가늘고 긴 도형은 여전히 pick 불가로
  남는다.

**실칩 확인(2026-09-02, 사용자)**: "모든 layer 켠 상태에서 막대들
모두 pick 됨" — 결함 A 수정 확정. 결함 B(cut 실명)만 보류로 남는다.

**2차 반복(2026-09-04) — 예산 모델 자체를 폐기**: 9.8G 실칩(객체
수 극대)에서 여전히 pick 실패. 원인: 가시-only 계상으로 바꿔도
**밀집 fill 위 클릭은 반경 안 가시 member만 수만 개**라 상한
4,096이 다시 굶는다 — 예산으로 열거를 제한하는 모델 자체가
틀렸다. **floe 비교**: `_svc_pick`은 KLayout
RecursiveShapeIterator + 탐색 box — cell별 공간 색인이 box 안
도형만 자연 열거하고 member 예산이 없으며, 상한은 containment
후보 수(_PICK_CAP)뿐이다. floe 모델로 정렬(0.12.36):

- **member 예산을 작업 제한에서 제거** — 열거는 box 컬링(grid
  해석 범위·Pts chunk·bbox)으로 자연 제한. 상한은 4,096→**4.2M
  안전밸브**로만 남기고, **소진 시 조용한 빈 결과가 아니라
  에러**(`query member limit exceeded`)로 응답 — GUI가 "no object
  here" 대신 "pick error: …"를 표시한다(silent-empty 클래스 제거).
- SNAP_SHAPE_CAP 4,096→1M(최근접 탐색이 밀집 box에서 잘리지 않게).
- **2b subtree layer mask를 query walk에도 적용** — 해당 layer가
  없는 subtree는 layer별 walk에서 하강 자체를 생략(40-layer 칩의
  무익한 하강 제거; raster와 같은 gate라 오류 도달성 규칙 동일).
- pick 후보 상한 64는 floe의 _PICK_CAP과 같은 의미론으로 유지.

비용: 열거는 클릭 box 크기에 비례(밀집 60µm box × 40 layer ≈
수십만 member ≈ 수-수십 ms/클릭 — floe와 같은 차수). 검증:
sample9 재현 3/3 유지, 소진-은-에러 회귀 테스트 추가(98 tests),
KLayout query parity 배터리 유지.

**실칩 확인(2026-09-04, 사용자)**: 9.8G 칩에서 "pick 잘 되는 것
확인함" — 예산 모델 폐기로 종결. 남은 것은 결함 B(cut 실명,
보류)뿐이다.

**결함 B 1차 해제(2026-09-10)**: 실칩(35.8 × 34.6 mm)에서 81~124 nm 폭·최대
119 µm 길이의 선 영역이 detail high의 210 µm 뷰부터 사라짐. `floe-index plan
--explain 1`로 4페이지 모두 `cull_hair`(max_min 0.081/0.124 µm < hair 0.128 µm;
한 페이지는 4 nm 차이) 확정, `floe2 render`(exact)는 그림. 페이지 hairline 규칙만
기본 해제(`HierOpts::page_hairline=false`, 킬 스위치
`FLOE_RUST_PAGE_HAIRLINE=cull`): raster가 이미 그런 레코드를 전체 길이의 1 px
선으로 그리므로 새 근사 표현 없이 복구된다. 검증(리뷰어 3항목): gate
`ThinPageTests` — 가는 선 페이지의 cut 1 px 이미지 = exact 이미지, 굵은 레코드
하나를 더해도 선의 가시성 불변, 남긴 페이지 수(`thin pages N kept`)가 perf 줄에
표시. 실칩의 decode·raster 비용 실측은 이 카운터로 한다. 크기 cut 제거·밀도
사다리·LOD hair 모드·재인덱싱은 그 측정 뒤로 분리(리뷰어 권고).

**결함 B 종결(2026-09-15, 0.12.131)**: hairline 규칙 해제(keep)로 되살아난 광역뷰의
비용(실측 6: 덱 fit 32 s, 25k 페이지 디코드·4,170만 hairline)은 점유 요약
(docs/OCCUPANCY_PLAN.ko.md, `design.ovo`)이 대체했다. 실칩 덱의 150 × 103 mm 뷰가
16.7 s → 0.15 s(depth 무관), 요약 생성은 추출본 4.4 s·17 MB, 덱 667소스 9.9 분·
172 MB. 결정: `floe2 index` 기본으로 요약 생성(`--no-occupancy`로 끔; 2026-09-16부터
덱의 소스만 기본, 레이아웃은 `--occupancy` opt-in), base cell
4 µm, 마스크는 keep + detail medium(요약이 켜진 광역뷰는 cut과 무관), View 메뉴
thin 정책 서브메뉴. 일반 레이아웃은 cull 그대로(요약 없음). 실칩의 level 4
depth 0 박스(덱 sub-cut wash가 희소 페이지 bbox를 칠함)와 마크 소실은 희소
페이지를 그리는 규칙(RENDERD 0.12.87)으로 종결.

**결함 C — 일반 레이아웃의 sub-cut 페이지 소실(현장 2026-09-16, 0.12.139 /
RENDERD 0.12.94)**: 9.8 GB 일반(마스크 아님) 레이아웃의 한 레이어가 detail
high에서도 Calibre보다 훨씬 적게 보였다. 원인은 플래너의 `cull_size`: 모든 도형이
cut(high = 1 px)보다 작은 페이지는 bbox가 2 px를 넘으면 wash도 받지 못하고 통째로
버려졌다(콘택·비아·마크 배열이 도형 하나가 1 px 미만이 되는 줌부터 전부 사라짐).
덱은 2026-09-10/15의 sub-cut 규칙(밀집 → footprint wash, 희소 → `keep_sparse`
픽셀)으로 이미 살렸으나 단일 레이아웃 요청은 `sub_cut_wash: false`였다. 조치:
단일 레이아웃 요청도 같은 규칙(SPEC-PLANNER §3), 킬 스위치
`FLOE_RUST_SUB_CUT_WASH=off`. gate `SubCutTests`(0.2 µm 상자 200 × 200 배열 —
레코드 반복과 배치 반복 각각 — 이 10 µm/px에서 블록으로, 100 µm 간격 상자 5개는
픽셀로; 킬 스위치에서는 셋 다 없음). 광역 뷰 비용은 wash walk 예산이 막고, 실칩
재측정 대기.
**결함 C 후속 — cull의 hairline 페이지(사용자 결정 2026-09-16, 0.12.141 / RENDERD
0.12.96)**: 9.8 GB 레이아웃의 점유 요약 생성이 한 시간을 넘어(마킹이 한 스레드로
도는 별건, OCCUPANCY §12 실측 9) 요약 없이도 광역뷰에 존재가 보여야 한다는 요청.
`thin:cull`에서 hairline 컷 페이지·노드도 sub-cut 규칙을 탄다: 선의 픽셀 채움이
footprint의 1/8 이상이면 레이어 색 블록, 미만이면 페이지를 남겨 선을 그린다
(`WASH_MIN_COVERAGE_HAIR`, `FLOE_RUST_WASH_HAIR_COVERAGE`). keep은 그대로 정확.
gate `SubCutTests.test_hairline_pages_under_cull…`(0.1 × 190 µm 선 200개 = 블록,
400 µm 간격 3개 = 선, keep은 둘 다 정확, 킬 스위치는 둘 다 없음).
**결함 C 후속 2 — 중간 줌 draw 6 s(현장 2026-09-16, 0.12.142 / RENDERD 0.12.97)**:
0.12.95에서 150 MB 실칩을 thin:cull detail medium으로 열자 중간 줌 구간에서 draw가
6 s를 넘었고, 사용자가 `FLOE_RUST_SUB_CUT_WASH=off`로 이전 속도가 돌아오는 것을
확인했다 — 원인은 위 두 규칙이 한 프레임에 보태는 양(남긴 희소 페이지의 디코드·
hairline 픽셀, wash 블록 채움)에 상한이 없던 것. 조치: ① perf 줄·상태줄에
`sub-cut washes A/sparse B`(wash 수, 희소로 남긴 수)를 붙여 비용을 읽게 하고,
② 플랜당 예산 둘(SPEC-PLANNER §3 플랜당 예산): 희소 ink 16 Mpx, wash 면적 64 Mpx.
소진 뒤의 항목은 옛 cull대로 버리고(희소를 wash로 돌리지 않는다 — 거짓 블록)
`sub-cut over A/B`로 센다. 예산은 걷는 순서대로 쓰여 플랜이 결정적이다. 기본값은
잠정: 실칩의 느린 프레임 perf 줄(`sub-cut washes/sparse/over`, plan/decode/raster
µs)로 정한다. 진단 `FLOE_RUST_SUB_CUT_SPARSE_MPX=0` / `FLOE_RUST_SUB_CUT_WASH_MPX=0`
은 각각 희소 전부·wash 전부를 버려 어느 쪽이 느린지 가른다. gate
`SubCutTests.test_the_per_plan_budgets…`. 실칩 재측정 대기.
**결함 C 종결 — sub-cut 규칙 기본 off(사용자 결정 2026-09-16, 0.12.143 / RENDERD
0.12.98)**: "sub-cut wash는 속도가 느려지는 부작용과 그럼에도 완전히 보이지는
않는 단점이 있어서 일반 레이아웃에는 적용하지 않는 것이 좋겠음. 덱도 occupancy가
있으므로 sub-cut wash는 사용될 일이 없음." 단일 레이아웃 요청의 `sub_cut_wash`와
덱의 wide 정책(2026-09-10 4단계) 모두 기본 off. 코드는 남기고 진단 스위치
`FLOE_RUST_SUB_CUT_WASH=on`(단일)·`FLOE_RUST_DECK_WIDE=on`(덱)으로만 켠다. 일반
레이아웃의 광역뷰 존재는 점유 요약(`floe2 index --occupancy`) + `thin:keep`이
맡는다 — 그래서 요약 생성 시간(OCCUPANCY §12 실측 9 이후)이 실사용 조건이다.
gate: `SubCutTests`(기본 워커는 셋 다 없음·카운터 0, on 워커가 규칙·예산 검증),
`ThinPageTests`·`WideViewTests`(기본 = 옛 cull, on = 규칙), 실칩 증상 테스트
(기본 = 증상, on = wash).
**결함 C 후속 3 — 대표(page frontier), 사용자 설계 2026-09-17 (0.12.145 / RENDERD
0.12.100)**: "hairline이 보이던 뷰에서 2배 축소하면 면적이 4배라 다 살리면 4배를
그려야 하지만 4개 중 1개만 남기면 비슷한 비용으로 디테일을 살릴 수 있다. 살아남는
hairline은 끝까지 살아남게, frontier처럼." 옛 bbox 대체가 fit 뷰에서 거대 박스
하나로 남았던 원인은 단위(페이지 bbox)의 화면 크기에 상한이 없던 것. 구현은
SPEC-PLANNER §3 대표: 컷 항목이 문턱의 1/2^k 이하이면 run 안 index가 4^k의 배수인
것만 남기고 sub-cut 규칙대로 그린다(희소 → 픽셀, 밀집 → footprint wash). 뷰당
수는 컷 시점의 수로 일정, 집합은 줌 사이에 포함 관계, BVH는 index 구간으로
프루닝. 단일 레이아웃 요청만(덱은 요약), 킬 스위치 `FLOE_RUST_PAGE_REPS=off`.
`thin:cull`의 기본 그림이 다시 바뀐다: 컷 아래 내용이 사라지지 않고 대표 무늬로
남는다(요약처럼 채워진 면은 아님). 상태줄·perf 줄 `reps K/W/C`(kept/washed/
children). 실칩 확인 항목: fit 뷰 첫 프레임의 read/decode(대표 페이지의 cold
read), 중간 줌의 plan/draw, `reps` 카운트. fit급에서 많이 느리면 그 줌 대역용
캐시를 미리 만드는 방안(사용자)을 그 다음에 본다. gate `PageFrontierTests`.
리뷰 반영(2026-09-17, 0.12.146 / RENDERD 0.12.101): ① 거대 박스 — ink 추정을
멤버 수 × 최소변 × 긴변으로 바꾸고 대표는 항상 1/8 채움(L 두 선이 200 % 밀집으로
잡혀 정사각형이 wash되던 것; gate `test_an_l_of_two_hairlines…`). ② hairline만
컷되는 페이지의 BVH 프루닝 — `min(max_w, max_h) < page_hair` 노드를 run 구간으로
프루닝(한 방향 배선은 잡힘, 양방향 혼합 노드는 리프까지; 노드 max_min은 인덱스
형식 변경이라 보류). ③ 포함 관계 방향 정정: S(k+1) ⊆ S(k) — 넓은 뷰의 대표는
가까운 뷰에도 있었다(반대 방향 아님), index 0은 후보. ④ 뷰당 비용 일정은 조건부
기대치로 기록. gate가 정확한 집합(N → N/4 → N/16), page_candidates 감소, 절반
뷰의 대표 수, L을 확인한다.
현장 2026-09-17 2차(0.12.147 / RENDERD 0.12.102): "광역뷰에서 박스 하나만 남거나
조금 줌인해도 박스로 보이는 경우가 많다." 대표를 sub-cut 규칙(밀집 → wash)으로
그린 것이 원인 — 밀집 페이지의 bbox wash, 그리고 크기 컷 자식 배열의 footprint
wash(다이를 덮는 배열이면 박스 하나). 조치: 대표는 **wash하지 않는다**. 페이지
대표는 디코드해 그리고, 배치 대표는 펼치되 배열 멤버를 4^j분의 1로 솎는다
(`thin_grid` stride, Pts는 4^j번째 점; 남은 옥타브는 배치 index). 비용은 컷 시점에
그 페이지·배열을 그리던 비용이고 개수는 옥타브 솎기가 묶는다. 상태줄 `reps K
pages/C children`. gate `SubCutTests`(배열 레이어가 블록 대신 블록 안의 점 무늬),
`PageFrontierTests`(rep_washed 0). 실칩 재확인 대기.
현장 2026-09-17 3차 + 리뷰(0.12.148 / RENDERD 0.12.103): "광역뷰에서 좌하단에만
박스처럼 보이는 것이 확대하니 살아남은 hairline이고 Calibre보다 지나치게 정밀,
나머지 화면은 비어 있음." 예산 초과가 아니라 솎기 단위가 **페이지**라서: fit 뷰의
k=8(65,536개 중 하나)이 run 길이를 넘어 index 0 페이지(leaf 순서의 첫 공간
클러스터 = 좌하단)만 남고 그 안의 선을 전부 그렸다. 조치(리뷰 조건 반영):
① 솎기를 레코드·멤버 단위로 내리고 경로 전체에서 한 번만 적용(배치: 멤버 lm +
배치 index; 페이지: 디코드 예산의 Lp + 래스터의 레코드 index·멤버; rep 배치의 자식
셀은 통째로). ② Grid는 축 균형 stride, Pts는 2^lm번째 slot으로 바로 건너뜀.
③ 페이지 수가 아니라 **디코드 바이트** 예산(256 MiB)으로 re-plan. ④ run이 뷰에
맞은 줌의 레벨로 고정(두 축 포함 판정, 화면 이동에 안정). ⑤ cbvh 프루닝을 배치
모듈러스 하한(노드별 최대 멤버 표)으로 맞춤. 검증 항목: 화면 전체 분포(사분면),
밀도 감소, 예산 re-plan, L. 남은 실측: fit 뷰 cold/warm 디코드, L=0 대역(컷 직후,
아직 솎지 않음 — 옛 중간 줌 6~8 s가 남을 수 있음), 화면 이동 시 안정성.
현장 2026-09-17 4차(0.12.149 / RENDERD 0.12.104): ① fit 뷰가 Calibre와 같은
영역·크기의 **박스**(Calibre는 점 무늬), ② 밀도가 Calibre보다 낮음, ③ 예산 초과가
간혹, ④ full depth 10~40 s 또는 수 분(스레드 4개). 한 원인: run 상한을 "run이 뷰에
맞은 줌"으로 잡아 작은 run(블록·작은 셀의 페이지)이 모두 레벨 0 → 블록 안 컷 도형을
전부 그려 박스(①)·디코드 폭주(③④), 반면 큰 run은 옥타브대로 극도로 희소(②).
조치: 옥타브·run 상한을 버리고 **프레임당 항목 예산**(100만, `FLOE_RUST_REP_ITEMS_M`)
으로 전역 레벨을 정한다 — 세는 pass가 뷰 안 컷 항목을 구해 L = ⌈log2(항목/예산)⌉.
"4배 넓어지면 1/4"은 그대로이고 포화(레이아웃 전체가 뷰 안)에서 자연히 멈추며 fit
뷰 밀도가 예산만큼 오른다. 자식 BVH는 노드별 항목 log2 합 표로 O(노드)에 센다.
상태줄 `reps … L n`. gate `PageFrontierTests`(L = 1/0, 예산 축소 시 L 4·1/16·포함).
실칩 재확인: fit 뷰 모양·밀도·perf(read/decode/raster), 예산 초과 여부, full depth
프레임 시간, 화면 이동 시 L 변동.
현장 2026-09-17 5차(0.12.150 / RENDERD 0.12.105): depth 0은 Calibre와 비슷(조금 더
밀함), fit 뷰는 여전히 박스, full depth는 예산 초과·10 s. 원인: 대표 배치의 자식을
통째로 그려 — 서브픽셀 인스턴스 하나(픽셀 한 점)를 위해 자식의 모든 페이지를 디코드·
래스터했고, 항목을 멤버 × 자식 레코드 수로 세어 화면 단위(점)와 어긋났다. 조치(사용자
승인, 1단계): ① 컷 배치는 인스턴스 하나 = 항목 하나로 센다. ② 남긴 멤버는 자식 bbox
한 개(cut px 이하의 점)로 그리고 자식 페이지 디코드·하위 걷기를 하지 않는다
(`rep_dots`). 2단계(사용자 제안)는 인덱싱 때 (cell, layer)·구역·단계별 대표 점/선과
셀 계층 대표를 별도 파일로 만들어 광역뷰의 페이지 디코드와 세는 pass 자체를 없애는
것 — 느린 레이어 하나로 인덱싱 시간·파일 크기·cold 시간·최대 메모리를 먼저 비교한다.
현장 2026-09-17 6차(0.12.151 / RENDERD 0.12.106): depth 0 fit 뷰 + 줌인 7단계까지
여전히 박스, full depth는 여전히 세대 예산 오류. 박스: depth 0에는 배치가 없으므로
페이지 쪽 — 프레임 전역 항목 예산은 1/128로 솎아도 픽셀당 20멤버인 밀집 배열
레코드를 그대로 두어 영역이 채워졌다(Calibre의 점 무늬는 화면 밀도 기준). 조치:
레벨을 항목별 **화면 밀도**로(ink/면적, 기본 0.25; 컷 자식 BVH 노드가 아래 전체의
L을 정해 상속) — 세는 pass·전역 L·이동 시 흔들림이 없어진다. 예산 오류: 대표 페이지
예산 256 MiB가 플랜의 다른 페이지와 합쳐 세대 예산 1 GiB를 넘겼다 → 대표 예산을
세대 예산에서 다른 페이지 바이트를 뺀 나머지의 절반으로 제한(`ViewReq::decode_budget`).
gate `PageFrontierTests`(밀도 일정·저밀도 env·포함), `SubCutTests`(밀집 = 무늬, 희소 =
그대로). 실칩 재확인: 박스 → 점 무늬 여부, 예산 오류, full depth 시간, 밀도 튜닝은
`FLOE_RUST_REP_DENSITY`.
현장 2026-09-17 7차(0.12.152 / RENDERD 0.12.107): depth 0 fit 뷰는 여전히 박스, 줌인
5단계부터 점으로 바뀌어 9단계에 모두 점; full depth 전환 80 s 초과·ESC 무응답. 박스:
M7-C `wash_px` 붕괴가 대표 페이지에도 적용돼 2 px 이하 페이지가 bbox 렉트(2×2 px 정사각형)
로 그려졌고 이어진 페이지가 면을 이뤘다 — 줌인해 페이지가 2 px를 넘으면 솎은 도형(점)
으로 바뀐 것과 일치. 조치: 대표 페이지는 wash_px 붕괴에서 제외. 80 s: 컷 자식 BVH를
리프까지 내려가 배치마다 판정했다(예전 "184M 배치" 병증의 재발). 조치: 점 간격
(1/√밀도 = 2 px) 이하의 컷 서브트리는 중심 1 px 점 하나로 끝내고 걷지 않는다
(`rep_node_dot`; 걷기 비용이 화면 크기에 묶임). gate `SubCutTests.test_a_field_of_plain…`
(4만 개 단일 배치 밭 = 블록의 약 1/4 점, visited_bvh < 4만, 페이지 0; 2 px 페이지 =
점 하나, washed 0).
**결함 C 종결 2 — 대표(page frontier) 비활성(사용자 결정 2026-09-17, 0.12.153 /
RENDERD 0.12.108)**: 0.12.152에서도 depth 0 fit 뷰는 박스이고 depth 99는 60 s를 넘어
ESC로 중단. 원인 분석(수정 없이): ① 컷되지 않는 **혼합 페이지**(큰 도형 하나 +
hairline 수천 개)는 frontier 범위 밖이라 통째로 디코드·래스터되어 페이지 영역이
채워진다(Calibre는 페이지 구성과 무관하게 서브픽셀 도형을 솎는다); 2 px 이하의
비대표 페이지는 M7-C wash로 2×2 정사각형. ② depth 99: 점의 레이어 팬아웃(점 ×
보이는 레이어 수) 또는 대표 페이지 디코드(플랜 2회 + 최대 256 MiB cold read)로
추정되나 perf 줄이 없어 확정하지 못했다. 결론: 페이지 단위 컷 판정이 근본 한계라
플래너 쪽 대표로는 Calibre 모양과 비용을 함께 얻을 수 없다 → 인덱싱 때
(cell, layer)·구역·단계별 대표 점/선과 셀 계층 대표를 **별도 파일**로 만드는 방식
으로 전환(사용자 제안). 플래너의 frontier·sub-cut 규칙·예산은 코드에 남기되 뷰어
기본 off(`FLOE_RUST_PAGE_REPS=on`, `FLOE_RUST_SUB_CUT_WASH=on`, `FLOE_RUST_DECK_WIDE=on`
진단 전용). 뷰어는 2026-09-16 아침(443baf6 이전)의 그림 — `thin:cull`은 컷 아래를
버리고, 광역뷰 존재는 점유 요약이 유일 — 으로 돌아간다.

**후속 구현 — 별도 대표 파일 OVR1 (0.12.154)**: 위 frontier 기본 off는 유지한다.
`--representatives[-only]`로 인덱싱 때 논리 멤버 수를 세고 선택된 번호만 역산해
네이티브 점을 만든다. 프레임은 레이어/depth별 저장 점을 공간 조회하며 OVP를 추가
디코드하지 않는다. 두께 비율 대신 화면 분포 면적으로 솎고 출력은 262,144점 이하다.
생성 파일도 전역 4,194,304점 상한이 있다. 따라서 좁은 확대 영역과 긴 선의 길이는
근사이며 실칩 성능/Calibre 밀도 일치는 아직 검증하지 않았다.
[설계·운용·짧은 검증](REPRESENTATIVES.ko.md). 사용자 요청대로 긴 배터리는 커밋 후 별도 실행한다.


**정책 분리(2026-09-11, 사용자·리뷰어)**: 마스크(jobdeck)는 hairline이 많을 수밖에
없고 일반 레이아웃을 같은 기준에 맞추면 광역 뷰가 느려진다. 그래서 위 해제는
공유 기본값(`HierOpts::default()` 변경, 6134456)이 아니라 **요청별 정책**으로
바꿨다: renderd 프레임의 `thin=keep|cull`, 플래너 `ViewReq::page_hairline`. 일반
레이아웃 = cull(기존 성능 정책, 가는 도형이 광역 뷰에서 생략될 수 있음을 상태줄
`thin:cull`로 표시), jobdeck = keep, 단독 마스크 OASIS도 `--thin keep`/View > thin
shapes at wide views > keep으로 마스크 정책 선택 가능(같은 파일이 여는 방식에 따라
달라지지 않게; 2026-09-15 auto/keep/cull 서브메뉴).
양변 모두 작은 도형의 cut과 자식 셀·BVH의 hairline은 두 정책 공통(별도 실측 뒤
결정). 환경변수는 진단 override로만 남김.

**keep 정책의 광역뷰 실측(2026-09-11)**: 덱 fit 뷰(1 px = 202 µm)에서 thin 25k
페이지를 전부 디코드·raster해 32 s(decode 합 30.9 s, draw 22.2 s, paints 4,170만).
근접뷰 exact keep은 유지하되, 광역뷰는 빈 공간 위치를 보존하는 다중 해상도 점유
요약이 디코드를 **대체**해야 한다(리뷰어). bbox 채움·밀도 숫자 방식은 제외, 기존
LOD는 verbatim 경로 때문에 단정 불가. 결정용 실험 도구
`tools/occupancy_experiment.py`(JOBDECK.ko.md §10 실측 6). **실측 7 결과**: 요약
방식 성립 — 생성 4.3 s(26 × 33 mm 영역), 저장 레이어당 약 15 MB(4 µm 기준 셀),
광역뷰 페인트 9.3만 셀; exact keep은 예산 초과로 불가, cull은 81 px. 표시 오차
기준은 셀 ≤ 1 px(2 px 셀부터 빈 공간이 16 % 메워짐).

### 3.20 pan 재사용 — F2R-16 (2026-09-04)

**관찰(사용자)**: "pan 20% 이동과 50% 이동이 큰 차이가 없음.
로딩된 것과 그려진 것을 재활용하지 않는 것 같다." 진단: decode는
LRU가 재활용 중(+1 new)이지만 plan+text plan(~237ms)과 raster
전체(~1.2s)가 겹침과 무관하게 전체 뷰 기준으로 재실행된다. 표적은
"그려진 것의 재사용"이며, §3.16의 제약(fill 위상이 framebuffer
기준, 주기 16px) 때문에 **16px 배수 이동만 byte-exact 재사용이
가능**하다.

**사용자 결정(2026-09-04)**: 라벨을 **항상 최상단**에 그리도록
z-순서 변경 승인(옵션 a) — retained frame은 geometry 전용으로 두고
라벨은 매 frame 위에 새로 그려, labels on에서도 pan 재사용이
작동하게 한다. KLayout의 between-plane 라벨 순서와의 의도적 편차
1건(상위 plane geometry가 하위 layer 라벨을 더 이상 가리지 않음).

**1단계 완료(0.12.37)**: 라벨을 per-tile interleave에서 **조립 후
full-frame 단일 pass**(gray block → plane 순 layer 라벨 → white
block, 라벨 간 상대 순서 불변)로 이동. 부수 이득: 라벨 작업이 tile
수와 무관해짐(`label_tile_paints` 의미 변경). bin/walk 두 경로에서
라벨 배관 제거로 단순화. 검증: render-core 98 tests(라벨 결정성
테스트를 새 불변식으로 재고정), **KLayout STYLE oracle 포함 전체
배터리 통과**(밸미니 fixture 허용 범위 내 — 실칩에서 라벨이 더 잘
보이는 방향의 변화만 있음).

**2단계 완료(0.12.38)**: 구현 —

- **GUI**: 키보드 pan(50%/Ctrl 10%)과 미니맵 recenter의 이동량을
  **16 device px 배수로 스냅**(최대 8px 오차, 시각 무영향). 드래그
  settle은 임의 delta라 v1에선 full raster 유지.
- **renderd**: 마지막 final render의 **label-제외 geometry frame을
  retained**로 보관(zoom/depth/cut/layer/style epoch가 key, 라벨
  상태는 의도적으로 제외 — 라벨은 매 frame 위에 새로 그림). 요청이
  retained view의 16px-배수 pan이면 요청 view를 retained 격자에
  **정확히 재스냅**(클라이언트 float 오차 제거)하고, shift된
  retained를 base로 넘긴다.
- **render-core**: base의 유효 영역에 완전히 포함되는 tile은
  raster 대신 **memcpy**(`tiles_reused` 계측). reuse 발화 시 tile을
  64px로 축소(픽셀은 tile 크기 불변 — 기존 oracle — 이라 안전;
  384px에선 완전 포함 tile이 거의 없어 재사용율이 명목뿐이라서).
- kill switch `FLOE_RUST_PAN_REUSE=off`. perf 라인에
  `pan-reuse N tiles`.

검증: render-core 단위 oracle(speckle+stipple+frames+label 포함
16px-shift 재사용 = cold render와 byte 동일, 99 tests), 실daemon
integration(스냅 pan 렌더가 recolor/mono 상태까지 동일한 cold
worker 렌더와 **payload byte 동일**, 재사용 tile >100), 전체 배터리.
기대: 20% pan draw ~1.2s→**~0.3-0.4s**, 50%→~0.65s — 드디어
이동량에 비례. plan+text plan ~237ms는 별도 후속(비gated 후보)으로
남는다. **실칩 재측정 대기** — pan 시 perf 라인의
`pan-reuse N tiles` 표기가 발화 신호다.

**실칩 확인(2026-09-04, w1.5M 지연 에지가 있는 무거운 뷰)**:

| pan | total | draw | pan-reuse | paints |
|---|---:|---:|---:|---:|
| Ctrl+커서(10%) | 620ms | **378** | 143/~156 tiles (92%) | 198k |
| 커서(50%) | 1,054ms | **845** | 78/~156 tiles (50%) | 2.9M |

재사용율이 이동량과 정확히 비례하고(92%/50%), paints(198k vs
2.9M)가 띠만 칠함을 증명한다. 이동량 무관 ~1.5-2s였던 pan이 종결.
**남은 pan 바닥**(이동량 비례하지 않는 고정 몫): ① collection
재실행(bin 365-458k 재수집 + w1.5M 에지 2개의 실패 trial ~0.2s —
trial 결과 캐시 후보), ② plan+text plan ~150-200ms(인접 뷰 plan
재사용 후보). 요구 발생 시 이 순서로 착수.

### 3.22 F2R-17 — 배경 margin prefetch (2026-09-04, 사용자 제안)

**요청**: "pan 대비 현재 뷰의 상하좌우 50%씩을 백그라운드로 그리고,
클릭/줌으로 전혀 다른 뷰 이동 시 즉시 중단."

**구현(0.12.40)**: GUI의 휴면 코드였던 `_covered()`(frame이 뷰를
덮으면 재렌더 생략)와 bg-frame 수신 경로("silent margin upgrade")를
되살려 결합했다 —

- **GUI**: settle 직후 **즉시** margin render를 bg=True·다음
  generation으로 제출. 확장 폭은 **화살표 한 스텝(50%, 16px 스냅) +
  `_covered`의 10% 여유 pad**(≈사방 60%, 총 ~2.2w×2.2h) — 한 번의
  50% 이동이 정확히 가장자리에 닿아 재렌더되던 것을 실측으로 교정
  (2026-09-04). 300ms 유휴 지연도 제거(사용자 결정) — margin은 어떤
  사용자 렌더에도 즉시 취소되므로 지연할 이유가 없고, 즉시 제출이
  연속 stepping을 무음으로 만든다. 같은 영역의 margin이 비행 중이면
  재제출을 생략(자기-취소 라이브락 방지), 사용자 렌더가 나가면
  비행 기록을 리셋한다.
  margin frame이 오면 last_frame이 커지고 이후 ±50% 내 pan은
  `_covered()`가 **재렌더 자체를 생략(즉시 crop)**. margin 안을
  돌아다니다 중심을 벗어나면(0.35·vw 기준) margin을 재보충한다.
- **즉시 중단**: margin도 일반 generation이므로 이후의 어떤 사용자
  render(클릭·줌·pan)든 renderd generation frontier가 **mid-flight
  취소**한다(수집·raster의 세밀한 cancel check). pick/snap은 input
  스레드라 margin 진행 중에도 막히지 않는다.
- **renderd**: retained 재사용을 **크기 상이 frame 간**으로 일반화
  (RetainedKey에서 w/h 제거, per-axis scale 동일성+16px 격자 검사) —
  margin render는 직전 viewport frame을 **중앙에 memcpy**하고 ring만
  raster(4×→3× 비용), margin 이후의 큰 pan도 겹침만큼 재사용한다.
- margin은 exact frame cache를 오염하지 않게 frame_cache=off.

검증: renderd 단위(viewport↔margin 양방향 매핑·격자 스냅),
실daemon integration(margin이 viewport를 중앙 재사용, margin 내부
pan이 **156/156 tile 재사용 + cold render와 payload byte 동일**),
전체 배터리.

**최종 확인(2026-09-04, 사용자)**: margin 폭 교정(한 스텝+pad)과
즉시 제출 적용 후 "**연속 패닝이 거의 무음으로 잘 됨**" — F2R-17
종결. 이전 중간 확인: 20%/50% pan 모두
**pan-reuse 182/182 tiles, draw 1ms, total 9~14ms** — margin이 새
뷰를 전부 덮어 전량 memcpy. 잔여 9~14ms의 지배 성분은 publish
fsync(~4-7ms)+handoff로, pan 비용이 raster에서 게시 오버헤드로
이동했다(비긴급: raw frame의 fsync 생략 후보). margin 심부 이동은
crop-only(라인 없음, 0ms) 경로다. 실칩 재확인 항목: ① margin 완성 후 ±50% 내 pan이
즉시인지(재렌더 자체가 없어 perf 라인이 안 찍히는 게 정상), ②
margin 진행 중 클릭/줌이 즉시 반응하는지, ③ 백그라운드 margin의
CPU 사용이 거슬리지 않는지(뷰당 최대 ~3× viewport raster).

**P0 리뷰 지적 수정(2026-09-05, 0.12.43)**: margin 예약/제출이 공용
GUI(`gui.py`)에 있으면서 백엔드를 확인하지 않아 **기존 floe(KLayout)
에도 적용**되고 있었다 — settled 뷰마다 ~4.84× 픽셀의 확대 렌더를
KLayout 서비스가 일반 foreground 작업으로 수행하고(취소·재사용 없음),
서비스는 요청의 `bg=True`를 버리고 `bg=False`로 돌려줘 GUI가 그
결과를 보통 프레임처럼 처리했다(perf 라인·상태 갱신). floe/floe2
비교 측정도 오염될 수 있었다. 수정:
- `RustRenderWorker.supports_margin_prefetch = True`,
  `service.RenderWorker.supports_margin_prefetch = False`(명시적 롤백).
- GUI `_margin_enabled()` = worker capability **AND** `frame_cache_on`
  — `_schedule_margin`/`_submit_margin` 모두 이 게이트를 지난다.
  `--frame-cache off`/`--perf-baseline`은 F2R-16 pan 재사용과 함께
  margin도 끈다(백엔드 중립 타이밍 유지).
- `_covered()`는 margin이 꺼진 상태에서 정확 viewport 프레임(축당
  ≤2px 스냅 여유)만 인정 — 어떤 경로로든 남아 있는 큰 프레임의 crop을
  쓰지 않는다.
- 방어적으로 KLayout 서비스도 요청의 `bg`를 그대로 되돌린다.
- `--frame-cache`/`--perf-baseline` 도움말을 F2R-18 이후 실제 의미
  (retained pan 재사용 + margin prefetch)로 갱신.

검증: validator에 `test_margin_prefetch_is_a_rust_only_reuse_capability`
추가 — KLayout worker에는 margin이 절대 제출되지 않음, frame-cache off
에서도 제출되지 않음, Rust+on에서만 bg=True 확대 프레임 제출,
`_covered()` 큰 프레임 crop은 margin 활성 시에만. 한계: KLayout
서비스의 bg 에코는 실KLayout 배터리가 없어 코드 검토로만 확인.

**margin 폭 정밀화(2026-09-05, 사용자 지적, 0.12.44)**: "50% pan 재드로잉
때문에 준 여유가 너무 많아 불필요하게 느껴짐 — 50%까지 재드로잉 안
되게 딱 맞게 계산". 원인 재진단: 50% 스텝이 재렌더된 이유는 margin
폭이 아니라 `_covered()`가 매 변마다 **10% 안락 여유**를 요구했기
때문이었고(스텝 432px가 정확히 가장자리에 닿으면 여유 0 → 거부),
이전 수정은 그 요구를 margin 폭(+ceil16(10%)=+96px/변)으로 메꾼
것이었다. 이번 수정:
- margin 폭 = **정확히 한 스냅 스텝/변** (`_snap_pan_px(0.5·vw)`,
  화살표 키와 같은 함수). 858×802 뷰포트: 1916×1796 → **1724×1604**
  (raster 픽셀 3.44M → 2.77M, **−20%**).
- `_covered()`는 margin 모드에서 안락 여유를 요구하지 않는다(뷰가
  프레임 안에 있으면 crop; float 반올림만 1e-3px 흡수). 선제
  re-margin은 이미 `_schedule_margin`의 "선두 여유 < 0.35 뷰포트"
  규칙이 맡고 있어 안락 여유의 역할은 없어졌다. margin 비활성
  경로(KLayout·frame-cache off)는 main의 pad 의미를 그대로 둔다.
- 덮이는 스텝 수는 동일(한 스텝)이므로 연속 패닝 동작은 변하지
  않는다 — 다음 margin은 각 스텝 직후 재제출된다.

검증: validator 확장 — 착지한 margin 위에서 6방향(축 4 + 대각 2)
한 스텝 pan은 crop, 한 스텝+16px는 재렌더로 픽셀 단위 핀.

### 3.24 수직 pan 가장자리 깨짐 — row 매핑 부호 버그 (2026-09-04)

**보고(사용자)**: "팬을 하다 보면 가장자리 이미지가 비정상적으로
그려짐."

**재현·원인**: 8방향 pan을 cold render와 pixel-diff한 결과 —
x-전용 pan은 전부 일치, **y가 포함된 모든 pan에서 십수만 px
불일치**. raster의 row 0은 화면 위(=world y1)인데 `prepare_pan_reuse`
가 row offset을 **y0 기준(+ky)**으로 계산해 부호가 뒤집혀 있었다.
기존 oracle이 못 잡은 이유: render-core 단위는 x-이동만, integration
pan도 x-전용, margin은 상하 대칭 확장이라 **부호 오류가 우연히
상쇄**되는 유일한 y-케이스였다.

**수정(0.12.42)**: row offset을 top-edge 기준 `(old_y1 − new_y1)/spp`
으로 교정하고 view 스냅도 y1 기준으로 재구성. 부수 발견 수정 1건:
stipple row 위상이 frame **height**를 포함하므로(`row+height−1`)
재사용은 height가 mod 16으로 일치할 때만 byte-exact — 창 크기 변경
직후의 잘못된 재사용을 막는 guard 추가(margin의 2×16k 차이는 통과).
회귀 고정: ① renderd 단위에 row-내용 검증 수직 pan 케이스(부호가
뒤집히면 즉시 실패), ② integration pan을 **양축 이동**으로 강화,
③ 8방향 probe 전부 cold와 pixel 동일 재확인.

### 3.23 F2R-18 — exact frame cache 제거, retained LRU로 통합 (2026-09-04, 사용자 결정)

**결정(사용자)**: "지금 만든 기능으로 기존 캐시는 크게 의미가
없으므로 제거." — F2R-08의 exact frame cache(최종 payload 3장,
64MB 상한)를 제거하고 retained geometry frame으로 통합했다
(0.12.41):

- **retained가 3-entry LRU로 승격** — (render state × scale)당 1장,
  최신 우선. frame cache의 유일한 우위였던 **zoom 왕복 복원**을
  scale별 retained가 흡수한다: 왕복 복귀 = k=0 전량 재사용(전 tile
  memcpy + 라벨 재도색, ~10-20ms — 기존 cache hit과 동급이며 그
  scale의 margin까지 함께 살아 돌아온다).
- k=0 동일-뷰 재사용 허용(기존 "frame cache 소관" 제외 삭제),
  FrameCache/FrameCacheKey/CachedFrame 및 상한 삭제(−64MB).
- **`frame_cache` 플래그는 의미 유지** — off = 재방문/pan 재사용
  비활성(perf-baseline의 backend-중립 타이밍 계약 그대로). wire의
  `frame_cache_hit` 필드는 호환용으로 0 고정, GUI의 "frame-cache"
  표시는 제거. margin job도 이제 이 플래그를 따른다(payload cache
  오염 예외가 소멸했으므로).

검증: retained LRU 단위(scale별 교체·상한·epoch 격리), integration
재고정 — exact 재방문이 `tiles_reused == render_tiles`(전량 재사용)
로 **payload byte 동일**, frame_cache=off는 tiles_reused 0의 진짜
cold 재raster로 동일 byte. F2R-08의 수용 gate는 이 형태로 승계된다.

### 3.21 "draw 회귀" 보고 판정 — 한-tile mini 집중 (2026-09-04)

**보고(사용자)**: 새 뷰 라인 — 2,183ms = 255 load + **1,822 draw**,
**tile-max 1,583ms(draw의 87%)**, bin 426k(defer 2r **w1.6M**),
paints 4.5M, hier **5.1M**/814k, evict 1,542. "draw 회귀 같음."

**A/B 판정(같은 날)**: 0.12.36 renderd를 별도 빌드해 같은 sample9
trace로 HEAD와 비교 — raster 0.4~0.6 vs 0.7~1.0ms로 **구조적 회귀
없음**(차이는 retained geometry clone ~0.3ms 상수, frame 크기
비례·내용 무관). 라벨 최상단(labels off인 이 라인과 무관)·pan
reuse(미발화)도 이 frame 경로를 건드리지 않는다.

**실체**: 0.12.32 mini walk 이후 처음 관측된 **초대형 지연 에지의
한-tile 집중** 체제다. w1.6M짜리 지연 에지 2개의 콘텐츠가 소수
384px tile에 몰리면 ① 그 tile의 mini가 거대해지고(≤786k cap), ②
cap을 넘으면 **mini를 다 짓고 버린 뒤 legacy per-plane 재walk까지
이중 지불**하며(hier 5.1M의 정체), ③ 나머지 worker는 논다(tile-max
= wall의 87%). 뷰 클래스 문제지 37/38 회귀가 아니다 — 다만 실재
결함이다.

**조치(0.12.39)**: bin에 지연 에지가 있으면 render-core가 tile을
**128px로 축소**(수집 후 grid 결정으로 재배치). 픽셀은 tile 크기
불변(고정 oracle)이라 순수 스케줄링 변경 — 무거운 tile이 쪼개져
4-worker가 나눠 갖고, per-tile mini도 cap 아래로 내려가 overflow
이중낭비가 사라진다. 비용: per-tile item 필터 증가(이 뷰 426k×
~40 tiles ≈ +수십 ms) ≪ tail 해소. 기대: 이 뷰 draw 1,822 →
**~0.6-0.9s**(tile-max ~1.6s → 수백 ms). §3.15 tile-sweep의 "작은
tile 기각"은 walk-폴백 체제의 판정이었고 bin+mini 체제에는 적용되지
않는다. 실칩 재측정 대기 — 같은 뷰의 tile-max가 판정 지표다.

**실칩 확인(2026-09-04)**: 같은 뷰 — 393 load + **1,028 draw**,
**tile-max 189ms**(1,583→8.4배 붕괴, wall이 tail-bound→sum-bound로
전환), hier 5.1M→**1.2M**(overflow 이중낭비 소멸), paints 4.5M
불변. draw −44%. 이 뷰 계열의 잔여 비긴급 항목: 실패 trial
~0.2s(w1.5M 에지 2개가 매 frame 헛trial — trial 결과 캐시 후보),
per-tile item 필터(519k×~40 tiles — 2c 설계의 counting-sort binning
후보). 둘 다 요구 발생 시 착수.

### 3.25 F2R-20 — retained frame byte 예산·margin 픽셀 캡·전체 복제 제거 (2026-09-05, 리뷰 지적 HIGH)

**지적**: 페이지 캐시는 1GiB로 고정돼 있지만 retained geometry frame은
**개수(3)만** 제한하고 바이트는 제한하지 않았다. 최대 프레임은 ~1GiB
RGBA까지 허용되고, retained용 geometry frame 전체 복제(raster),
raw 게시 payload 연결 복제(renderd), header 제거 slice 복제(Python)가
겹쳤다. 860×804 창에서는 작지만 4K 창의 margin은 한 장 ~133MiB,
retained 3장만 ~400MiB — 공유 서버에서 페이지 예산 밖의 메모리 압박.

**수정(0.12.45)**:
1. **renderd retained byte 예산**: `FLOE_RUST_RETAINED_MB`(기본 256,
   0 = 보관 없음 = pan 재사용 off). 개수 3 **AND** 바이트 예산을 함께
   만족할 때까지 오래된 것부터 축출하고, 단독으로 예산을 넘는 프레임은
   보관하지 않는다. frame line `retained_bytes` → 어댑터 `retained_mb`
   → GUI perf 라인 `retained NMB`(잔류 수준, 누적 아님).
2. **GUI margin 픽셀 캡**: `MARGIN_MAX_MPIX=16`(64MiB RGBA),
   `FLOE_MARGIN_MAX_MPIX` 오버라이드. QHD(2560×1440)까지는 온전한 한
   스텝 margin(5122×2882, 56MB)이 그대로이고, 4K는
   양축을 하나의 비율로 비례 축소해 16px 배수로 내린다
   (5442×3058, +800/+448px/변, 63MB). 뷰포트가
   단독으로 캡을 채우면 margin을 만들지 않는다. "already margined"
   판정은 프레임 **자체 확장의 70%**로 상대화해 축소된 margin이 매 pan
   마다 재제출되지 않게 했다(일반 margin에서는 기존 0.35·뷰포트와 동치).
3. **복제 제거**: (a) raw 게시는 header(16B)+pixels **2회 write** —
   연결 payload 복제 삭제. (b) 레이블 행이 없는 렌더(frame off,
   `--perf-baseline`, labels off)는 geometry clone을 생략하고 **게시
   프레임 자체를 보관**(`geometry_is_frame`). 레이블이 있으면 위에
   칠해지므로 clone 1회는 남는다. (c) Python raw 읽기는 header를 따로
   읽어 pixels가 header-free로 도착 — slice 복제 삭제. 게시 직후
   프레임/PNG 버퍼를 즉시 해제.

RGBA 상한(창 크기별):

| 창 | viewport | margin(이전) | margin(캡 후) |
|---|---|---|---|
| 858×802 | 2.8MB | 11MB | 11MB |
| 2560×1440 | 14.7MB | 56MB | 56MB |
| 3840×2160 | 33MB | 133MB | 63MB |

retained 합계 ≤ min(3장, 256MiB); 픽셀 경로는 바뀌지 않아 byte
동일성은 그대로다(oracle 배터리).

**리뷰 후속(0.12.47) — frame-cache off에서도 geometry 복제·보관이
계속됨**: 재사용 *조회*는 `frame_cache` 게이트로 꺼졌지만 raster에
넘기는 `keep_geometry`는 `is_final`만 봐서 새 daemon의 첫
`frame_cache=0` 요청에서 `tiles_reused=0`인데 `retained_bytes=9011200`
이 나왔고, 라벨 on이면 전체 geometry clone 비용도 남았다. 보관 여부를
raster 호출 **전에** 한 함수(`retention_enabled`: kill switch·exact·
frame_cache·retained 예산 0)로 결정해 조회와 keep/clone/store가 같은
조건을 쓴다. 검증: renderd 단위(on/off/exact), 실daemon — 새 daemon에
`frame_cache=0` 두 번: 두 프레임 모두 retained 0MB·tiles_reused 0·
첫 렌더와 byte 동일.

검증: renderd 단위(byte 예산 축출 순서·초과 프레임 미보관·0 = 없음·
2-part 게시), render-core(레이블 없는 render는 `geometry_is_frame`이고
copy 없음, pan 재사용 byte 동일 유지), validator(QHD 온전·4K 캡
tight·top-up 판정·`retained_mb` wire·실daemon: 재방문 `retained_mb`>0,
`FLOE_RUST_RETAINED_MB=0`이면 tiles_reused 0 + 첫 렌더와 byte 동일),
전체 배터리. 한계: 4K 창 실측은 없음(정책 검증만); 레이블 on 경로의
clone 1회는 설계상 필요.

### 3.26 리뷰 MEDIUM 2건 — margin crop 라벨 정확도 게이트, 질의 스레드 (2026-09-05)

**(a) margin crop 경로의 라벨 정확도 게이트가 없음.** 라벨 선택은 요청
박스 기준(뷰당 4,096 cap, placement member 200k, 후보 cap 64k)인데,
margin은 확대 박스로 라벨을 고르고 실제 pan은 재계획 없이 이미지를
crop만 한다. 기존 통합 게이트는 margin 뒤에 **새 렌더**를 제출해 cold
와 비교하므로 GUI의 "렌더 없이 crop만" 경로를 검증하지 않았다.

분석: 세 가지 차이 원인이 있었다. ① declutter bin은 world-anchored
지만 후보는 **앵커가 요청 박스 안**일 때만 bin에 들어가므로, 뷰포트
경계가 지나는 bin의 승자가 두 계획에서 다를 수 있다(경계 라벨). ②
budget truncation — 4× 면적의 margin이 같은 cap을 먼저 소진할 수
있다. ③ block 이름은 "placed bbox ∩ clip"으로 선택되고 자기 bbox
중심에 놓이므로 이미 crop-정확(뷰포트와 교차하는 block은 margin과도
교차, 그 외 block의 이름은 뷰포트 밖).

수정(0.12.46):
- **bin-정렬 요청 박스**(`floe-vfs text.rs`, non-raw 모드): 계획
  박스를 bin 경계로 넓혀 모든 bin의 승자가 **world 위치만의 함수**가
  되게 했다. 직접 렌더도 경계 bin의 승자를 앵커가 프레임 밖이어도
  선택하고 clip해 그린다(KLayout 동작). raw 모드(oracle XOR)는 정확
  요청 박스를 유지.
- **margin은 geometry 전용이다**(GUI가 `labels=off`로 제출, 0.12.48).
  두 번의 중간안 — 면적비 budget(0.12.46: 뷰포트 4,096 cap vs margin
  4,875개로 착지 시 화면 라벨이 바뀜), 같은 budget + truncation 게이트
  (0.12.47) — 은 리뷰 재현으로 무너졌다: 화면 밖에 앵커된 위 레이어의
  200자 라벨 꼬리가 margin에서 화면 안 라벨 **위에** 칠해져(238px 변경,
  235px 빨간 성분 감소) "꼬리를 더 보여줄 뿐 기존 라벨은 보존"이라는
  계약이 성립하지 않았다. 라벨이 있는 한 crop은 안전할 수 없다.
- **라벨 on이면 margin을 화면에 쓰지 않는다**(reuse-only). 착지해도
  `last_frame`을 바꾸지 않고 renderd의 retained geometry로만 쓴다.
  대신 margin 안의 pan은 renderd **라벨 재합성 fast path**를 탄다:
  retained 프레임이 요청 전체를 덮으면(`valid == 전체`) page plan·
  decode·working set을 모두 생략하고(`PlannedView::empty`), 이
  뷰포트의 라벨만 계획해 memcpy된 geometry 위에 칠한다. 질의용
  published scene은 margin 렌더의 것(이 뷰를 포함)을 유지한다.
  GUI는 착지한 margin의 중심을 기억해 그 안의 settle에서 재예약하지
  않는다. 비용: text plan + label pass + publish — 실칩 기준 수십 ms,
  perf 라인은 찍힌다("0 tiles, +0 new, pan-reuse 전량").
- **라벨 off면 crop**: geometry-only 프레임은 16px 계약으로 byte
  정확하므로 이전처럼 0ms crop 경로를 유지한다(perf-baseline, frame
  off 모드).
- **oracle**: floe-vfs 단위 — bin-정렬로 큰 박스 계획을 뷰포트로
  제한하면 텍스트는 뷰포트 계획과 정확히 같고 block은 부분집합(pan 중
  경계 bin 라벨이 재배열되지 않는 효과는 유지); 실daemon 통합 —
  ① 라벨 off margin의 crop 영역(96px, 16px 오프셋)은 같은 뷰의 라벨
  off 직접 렌더와 **byte 동일**(엄격 비교), ② 라벨 on으로 margin 안을
  pan하면 tiles_reused == render_tiles, `tiles`(page plan) == 0,
  labels > 0, 그리고 cold 렌더와 **byte 동일**. 이전의 "추가 잉크
  band" 판정(64px → `label_extent_px`)은 계약이 아니었으므로 필드와
  함께 제거했다.

**사용자 결정(2026-09-05, 0.12.53) — 라벨 포함 margin 복귀**: 라벨
on에서 pan마다 라벨만 fast path로 뒤늦게 그려져 깜빡이는 것처럼
보인다는 지적. margin을 다시 라벨 포함으로 그리고(margin 박스 기준
bin-정렬 계획, 뷰포트와 같은 budget) GUI가 crop 소스로 채택해 pan이
라벨 포함 0ms crop이 되게 했다. 표시 규약으로 받아들인 것: 뷰포트
밖·margin 안에 앵커된 라벨은 글리프가 닿는 부분이 crop에 보이고 다른
라벨과 겹칠 수 있다(직접 렌더에서도 라벨 겹침은 가능; 선택이
world-anchored라 같은 자리는 항상 같은 라벨). budget에 걸린
margin(`labels_truncated`)은 crop하지 않고 geometry base + renderd
재사용에만 쓴다. margin 착지 시 뷰포트 계획 라벨 → margin 계획
라벨로 바뀌며 가장자리 꼬리가 나타날 수 있다(1회, pan마다는 아님).
fast path는 margin 밖 pan에 그대로 남는다. 검증: 라벨 포함 margin
두 번 렌더 byte 동일·labels>0·미truncated, 라벨 off crop 엄격 oracle
유지. **사용자 확인 완료(2026-09-05)**: 라벨 on pan에서 라벨이 즉시
함께 보임.

**실칩 판정 후 원복(2026-09-07, 0.12.55)**: 0.12.54는 "새 뷰가 다
그려진 뒤 이동"과 "마지막 pan 방향 한 스텝만 prefetch"를 넣었으나,
실칩 detail high에서는 렌더가 길어 그림이 멈춰 있는 시간이 조작감을
해쳤다(사용자 판정: "조작감이 좋지 않아서 그리 좋지 않음"). 사용자
지시로 `ffb9528`을 되돌려 0.12.53 동작(네 방향 라벨 포함 margin,
pan 즉시 이동, 착지 margin을 새 strip의 base로 blit)으로 복귀했다.
따라서 실칩에서 margin 착지 전 pan의 검은 strip은 **미해결**로 남는다.
남은 선택지(사용자 결정 대기): ① 방향 prefetch만 적용(동결 없이, 첫
pan은 여전히 strip), ② strip을 검정 대신 이전 프레임의 가장자리
확장이나 회색으로 채우는 시각 완화, ③ detail high의 draw 자체 단축
(F2R-03c 등 성능 축).

**리뷰 후속(0.12.49)**: ① HIGH — 레이어 A→B→A 토글 뒤 그림은 byte
동일하지만 A 전용 레이어의 pick/snap이 실패했다. 전량 재사용 fast path
가 published scene 갱신을 생략해 B 전용 scene으로 질의했기 때문.
`PublishedScene`에 계획 당시의 render key(RetainedKey)와 view를 넣고,
fast path는 published scene이 **요청을 그대로 서비스할 때만**(key 동일
AND view 포함) 탄다; 아니면 일반 경로로 계획·재게시한다. ② MEDIUM —
margin 안 첫 pan의 viewport 프레임이 같은 배율의 margin을 **교체**해
retained가 4MiB→1MiB로 줄고 두 번째 pan은 56/64 재사용 + page plan
재실행, GUI는 margin이 남은 줄 알고 재예약하지 않았다. `store_retained`
는 새 프레임을 **완전히 포함하는** 같은 상태·배율의 프레임이 있으면
그것을 최신으로 touch만 하고 새 프레임을 저장하지 않는다(같은 상태 =
겹치는 픽셀 동일). 검증: renderd 단위(포함 유지·포함하는 새 프레임은
교체·LRU touch), 실daemon — 전체→다른 레이어 하나→전체 순서에서 전량
재사용 + byte 동일 + snap/pick parity 재통과; margin 안 pan 뒤
`retained_mb` 불변, 두 번째 pan도 page plan 0.

**실측 후속(0.12.50) — 라벨 on pan 시 새 영역이 잠깐 검게 보임**:
사용자 보고 + 분석(512×512 재현: 커서키 50%, Shift+커서키 9.4%가
새 프레임 도착 전 검정; 라벨 off는 0%). 원인은 0.12.48의 설계 자체가
아니라 GUI가 착지한 margin을 **표시 base로도** 쓰지 않은 것이다:
라벨 on이면 margin을 버리고 작은 last_frame만 이동시켜 새 영역이
빌 자리가 없었고, 120ms debounce 뒤에야 fast path 프레임이 채웠다.
수정: ① 착지한 margin을 `_margin_frame`으로 보관하고 `_display`가
같은 상태·배율이면 **먼저** 현재 중심에 blit한 뒤 라벨 프레임을
겹친다 — 새 영역은 즉시 geometry, 겹침은 이전 라벨 프레임, fast
path 프레임이 곧 전체 교체. ② 커서 pan이 착지 margin 안이면 debounce
없이 즉시 제출(라벨 지연 = fast path 왕복만). 라벨 off 경로(crop)는
그대로. 검증: GTK 합성 테스트(겹침은 라벨 프레임 색, 새 strip은
margin 색, margin 없으면 검정), pan 즉시 제출 판정(안/경계/밖).
분석이 제안한 "라벨 별도 overlay"는 겹침 영역의 라벨을 이전 프레임이
이미 정확히 제공하므로 필요하지 않았다. **사용자 확인 완료
(2026-09-05)**: 라벨 on 커서/Shift+커서 pan에서 새 영역 검정 없음.

경위: valmini 진단(0.12.46)에서는 차이가 왼쪽 가장자리 29px의 "crop
에만 잉크"뿐이어서 꼬리를 수용 편차로 기록했으나, 리뷰의 합성 입력
(화면 안 빨간 라벨 + 화면 밖 위 레이어의 파란 200자 라벨)이 꼬리가
기존 라벨을 **덮는** 경우를 보였다. 라벨 pass는 plane 순서로 칠하므로
위 레이어의 꼬리는 아래 레이어 라벨 위에 놓인다 — 직접 렌더에서는
그 라벨이 선택되지 않아 빨간 라벨이 온전하다. 어느 crop 오프셋에서도
성립하는 판정을 한 장의 margin 이미지에서 얻을 수 없으므로 "라벨은
뷰포트 기준 재합성"으로 결론지었다. 실칩 확인 항목: 라벨 on 연속
패닝의 체감(라인당 수십 ms, 라인 찍힘)과 라벨 off의 무음 crop.

**(b) pick/snap이 renderd 입력 스레드를 점유.** stdin 디스패처가
snap/pick을 인라인 실행해 최악의 dense repetition 질의(member cap
4,194,304) 동안 새 render generation을 읽지 못하고 cancellation
frontier도 올라가지 않았다.

수정(0.12.46): 질의 전용 스레드(`query_worker`). 입력 스레드는
파싱만 하고 kind별 sequence frontier(`RenderCancellation` 재사용)를
올린 뒤 채널로 넘긴다 — 같은 kind의 더 새 질의가 오면 비행 중인 질의는
다음 member heartbeat에서 `superseded` 오류로 중단한다(GUI는 최신
seq의 응답만 읽으므로 무해). render-core에 `pick/snap_scene_cancellable`
추가. 킬 스위치 `FLOE_RUST_QUERY_INLINE=1`(인라인 복귀). 검증:
render-core(초과된 seq는 member를 만지기 전 중단, 현재 seq는 일반
경로와 동일 결과), renderd(스레드 왕복·Shutdown), 실daemon 통합의
snap/pick 스키마 비교 유지.

### 3.27 budget 사용처 검토와 계획 (2026-09-05, 41ee53a 기준)

사용자 요청으로 floe2의 budget/cap 사용처를 전수 조사하고, 외부 분석
의견 두 건과 대조해 합의한 결과다. 코드 변경은 없다.

**분류**
- 출력 불변 fallback(소진 시 비용만 변함): decoded LRU 1GiB, retained
  3장·256MiB, margin 16Mpx, work-bin 768k items·trial 잔여/2, subtree
  mask 16MiB, hier `pts_enum_budget` 200k·`k_boxes` 4, refinement
  500ms(제품 기본 refinement off라 평시 무관).
- 표시 생략, 표시됨(`labels partial`): 라벨 `view_budget` 4,096(bin
  유계라 텍스트로는 사실상 도달 불가, block 이름에서 도달)·
  `member_budget` 200k·`cand_cap` 65,536·`MAX_LABEL_GLYPHS` 262k.
- **조용히 출력에 영향**: snap 도형 cap(1.05M, 검사한 부분의 최선을
  정상 반환), `frame_cap` 200k(이후 경계 프레임 생략, 레코드 단위),
  경계 프레임 Pts > `pts_full_rep` 8,192는 footprint 박스 하나로
  융합(개수 임계값에 따른 표현 변경, 통계 필드 없음).
- 명시 오류: `QUERY_MEMBER_CAP` 4.19M(F2R-15 이후 안전밸브), 뷰당
  decoded 합 > 1GiB(`decoded generation budget exceeded`), 축퇴 grid
  1.05M, 이미지 268Mpx. pick 64는 floe parity(사용자 결정).
- 문서화된 근사 정책(별도 관리): detail cut, wash 2px, LOD `lod_k`,
  hairline, thin lattice, declutter bin.
- 제외: CTX 2M·중첩 rep 4M·비축 배열 8M은 구 flat/band tiler 경로의
  cap이다. 현재 `floe index`는 `floe-index vfs`로 진입하며 이 경로를
  쓰지 않으므로 floe2 인덱싱의 파일 한계 목록에서 뺀다.

**메모리 회계(수정)**: 1GiB는 decoded LRU 잔류량과 뷰당 decoded
합의 상한이고 판정은 decode **뒤**에 이루어져 순간 증가를 막지 못한다.
회계 밖: PublishedScene이 핀한 페이지(Arc, LRU 축출 후에도 상주),
mini-bin 합계(edge×tile×worker, item cap은 bin당), clip 누적(128
페이지씩 읽지만 geometry와 OASIS bytes를 한꺼번에 보유), 프레임 버퍼
복제, Python/GTK. `resident_bytes`는 LRU 잔류량이지 RSS가 아니므로
이것으로 여유를 판단하지 않는다. wire에 없는 fallback 계측: mini-bin
overflow 횟수, 실패 trial 시간, Pts fallback 횟수, mask fallback
플래그(`mask_bytes`는 full mask 시 크기만 줄어 명시적이지 않음).

**대안 검토(합의)**: 라벨을 화면 공간별로 균등 선택하는 방향은 우선
검토하되 안전 cap은 유지한 채 축정렬 규칙 Grid fast path부터 검증하고,
일반 Pts(bin별 후보 검색, chunk 단위 skip은 승자 규칙 변경)와 block
이름(계약 테스트가 "같은 bin의 독립 block 이름 전부 유지"를 고정),
긴 문자열(glyph 무계)은 별도 설계한다. frame 수의 화면 면적 상한은
겹침 때문에 성립하지 않으므로 cap 제거 근거가 아니다. 1bpp streaming
raster는 pick/snap이 PublishedScene의 페이지에 의존하므로 페이지
해제를 자동 해결하지 않고, mask도 plane 수에 비례(4K×200 plane ≈
198MiB)한다. 착수 순서는 F2R-23~27(§5, §6 10~14).

### 3.28 GUI — 메뉴/다이얼로그 뒤 키 명령이 죽는 문제 (2026-09-05, 0.12.51)

사용자 보고: 메뉴를 사용한 뒤 포커스가 뷰로 돌아오지 않아 `g` 같은 키
명령이 듣지 않음. 키 명령은 window의 key-press 핸들러가 받으므로
포커스 위치와 무관해야 하지만, 메뉴가 닫힌 뒤 메뉴바가 활성 상태로
남거나(일부 백엔드의 grab) window에 쓸 만한 포커스 위젯이 없으면
키가 캔버스까지 오지 않았다. 수정: 캔버스(scroller)를 포커스 가능한
키보드 홈으로 두고, `_focus_view`(메뉴바 deactivate + scroller
grab_focus, 없으면 window 포커스 해제)를 캔버스 클릭 시 동기로, 메뉴
`deactivate`와 모든 transient 다이얼로그 종료 뒤에는 idle로
(`_restore_keys`: present 후 `_focus_view`) 호출한다. 검증: 단위
테스트(메뉴바 deactivate → scroller 포커스, 포커스 불가 시 window
포커스 해제). 대화형 재현은 사용자 확인 대기.

후속(0.12.52): 파일 로드 뒤 `d`/`g`가 비프만 나는 보고. load
다이얼로그의 `run()` 종료 경로(취소·열기 모두)와 `open_file`(패널
재구성·worker 재시작 뒤)에 `_restore_keys`가 없었다 — quartz는
미처리 keyDown에 비프를 낸다. 두 곳에 추가. instance-forward 로드도
`open_file`을 지나므로 함께 해결. **사용자 확인 완료(2026-09-05)**:
메뉴·다이얼로그·파일 로드 뒤 키 명령 정상.

### 3.29 main 병합 (2026-09-07, 0.12.56)

사용자 요청("floe에 추가된 DRC note 기능이 floe2에 없음")으로
`origin/main`의 24개 커밋을 review/floe2에 병합했다. 두 제품은 같은
GUI(gui.py)를 쓰므로 floe2 전용 작업이 아니라 분기점(b0343b8) 이후
main에 쌓인 기능이 이 브랜치에 없었던 것이다. 들어온 것: DRC 검토
note(flateyes `.fe` per-reviewer sidecar, 좌상단 반투명 note 패널,
grid의 note 표시, 번들 두벌식 한글 입력), reviewer별 waive autosave와
`--floe-reviewer`, 미인덱스 소스 로드 시 인덱스 빌드 제안, 뷰어
UX(스크롤바·HighContrast 테마·object-pick 표시·Tab/룰러 동작), index
기본값(jobs 12, LOD off)과 monster-cell profile 재사용, 버전 정책
분리(`__version__`은 매 push, `RENDERD_VERSION`은 바이너리 재빌드
push에만 — 이 브랜치는 매번 재빌드하므로 둘을 함께 0.12.56으로).
충돌 5개 파일(버전 4 + gui.py의 load 다이얼로그: 키 복구와 인덱스
제안을 둘 다 유지). 검증: 전체 배터리(FLOE2 PRODUCT의 버전 게이트,
DRC ICE, index CLI, oracle) 통과, validator는 병합 후 note 패널
stub과 `__version__` import 보강.

### 3.30 GUI — DRC 그리드 waived 색 불일치 (2026-09-08, 0.12.57)

사용자 보고: waived 번호(녹색)를 클릭한 뒤 다른 번호를 클릭하면 앞
번호가 하늘색으로 바뀜. 상태 변화가 아니라 색 규칙 불일치였다:
2026-08-17(90dec4d)에 waived 색을 cyan→green으로 바꿀 때 페이지 전체
채우기(`_drc_grid_fill`)만 고치고, 현재 셀 표시가 옮겨갈 때 이전 셀을
되돌리는 경로(`_drc_cell_mark`)는 `#00ffff`를 유지했다. 두 경로를
하나의 formatter(`_drc_cell_markup`, 팔레트 상수 `DRC_GREEN`/
`DRC_RED`/`DRC_GOLD`에서 색 도출)로 합쳤다. Python-only 변경이라
`__version__`만 0.12.57(`RENDERD_VERSION` 0.12.56 유지, 재빌드 불필요).
검증: formatter 단위 테스트 + GUI 소스에 두 번째 waived 색이 없음을
단언.

### 3.31 GUI — DRC pane 검은 바탕·흰 글씨 (2026-09-08, 0.12.58)

사용자 요청: DRC pane도 레이어 pane처럼 기본을 검은 바탕에 흰 글씨로.
레이어 pane의 교훈(mac/retina에서 ScrolledWindow 하위 전체에 `*`
배경을 걸면 클립 밴드가 깨짐)을 따라 위젯 종류별로 범위를 좁힌
`.floe-drc` CSS 규칙을 두고, 세 스크롤러에는 레이어 pane의 스크롤바
클래스를, 스크롤되는 자식에는 `.floe-drc-bg`를 붙였다. 규칙 목록·
번호 그리드는 markup 색(waived 녹색·빨강·선택 금색·현재 셀 파랑)을
검은 바탕 위에 유지하고 선택 행은 레이어 pane과 같은 파랑이다.
pane CSS 전체를 모듈 상수 `PANEL_CSS`로 빼서 GTK 파싱 테스트로 문법
오류를 시작 전에 잡는다. Python-only(`__version__` 0.12.58). 대화형
확인은 사용자 몫.

후속(0.12.60, 사용자 조정): 검은 바탕은 **규칙 목록과 에러 번호
그리드만**으로 좁히고(클래스를 두 TreeView 자체에 부여, 스크롤러는
스크롤바 규칙만 공유), 상세 텍스트·검색창·버튼·필터는 테마 기본
(흰색)으로 되돌렸다. 선택된 에러 셀의 금색 배경은 waived 녹색 글씨와
대비가 ~1.1로 읽히지 않아 어두운 보라(`DRC_SEL_BG` #4a1f6b)로 바꿨다
— 빨강 ~3.9, 녹색 ~7.4의 대비이고 현재 셀의 파랑·검은 바탕과도
구분된다. 캔버스의 박스 선택 색(금색)은 그대로다.

후속(0.12.61, 사용자 조정): 선택 셀 배경을 다시 캔버스 마커와 같은
금색으로 통일하고, 대신 선택 셀 안의 번호를 같은 색상(hue)의 어두운
변형으로 그린다(waived `#006b3c`, 미waive `#b00020`; 금색 대비 각
~4.9/~4.8). 비선택 셀과 캔버스 마커의 밝은 녹색/빨강은 그대로다. 두
목록의 배경은 검정이 너무 어둡다는 판단으로 진한 회색(`#2b2b2b`)으로
바꿨다(흰 글씨 대비 ~12, 녹색 ~7.9, 빨강 ~4.1).

### 3.32 GUI — DRC 에러 보기의 in view 해제·클릭 이동·줌 유지 (2026-09-08, 0.12.59)

사용자 요청 세 가지. ① in view가 켜진 채 에러를 더블클릭해 보면 뷰가
그 에러로 좁혀져 목록의 다른 에러가 모두 사라지므로, 프레이밍 점프는
in view를 먼저 자동 해제한다(체크 버튼 핸들러가 그리드를 다시 채운
뒤 점프한 에러 셀에 마크를 되돌림; n/p 점프에도 같은 규칙). ② 에러를
보는 중(더블클릭 점프가 살아 있는 상태)에 목록의 다른 번호를 클릭하면
n/p와 동일하게 그 에러로 점프한다(보는 중이 아니면 종전처럼 마크와
상세만). ③ 보는 중에 줌인/줌아웃으로 배율을 바꾸면 이후 n/p와 클릭
이동은 배율을 유지하고 중심만 옮긴다(`goto(window=None)`); 새
더블클릭이나 Esc 뒤의 새 세션은 기본 프레이밍(DRC_VIEW_FRACTION)으로
돌아간다. 판정은 마지막 점프 직후의 spp와 현재 spp의 차이로 한다.
검증: 단위 테스트(in view 해제 순서, 기본/유지/재프레이밍 전이, 클릭
점프 여부). Python-only(`__version__` 0.12.59).

### 3.33 GUI — 창 최대화/복원 뒤 뷰 미갱신 (2026-09-08, 0.12.61)

사용자 보고: 창 제목줄 더블클릭으로 창을 키우거나 줄이면 뷰가
refresh되지 않음. 뷰 갱신은 캔버스 스크롤러의 `size-allocate`
핸들러(`_on_allocate`)에 걸려 있는데, macOS quartz의 네이티브 zoom은
이 신호가 캔버스까지 오지 않는 경우가 있어 이전 크기의 그림이 다음
pan까지 남는다. 25ms 폴링 틱에서 스크롤러의 실제 allocation을 마지막
값과 비교해 달라졌으면 같은 핸들러를 호출하는 fallback
(`_sync_allocation`)을 넣었다. 신호가 정상 도착하는 환경에서는 값이
같아 아무 일도 하지 않는다. 대화형 재현은 불가해 단위 테스트(변화
없음/변화/미실현 allocation)로 고정, 사용자 확인 대기.

정정(0.12.62): 문제는 **리눅스(원격 X)**에서 관측된 것이고 맥은
미확인. 원격 X에서는 신호 누락보다 스크롤 blit 문제(`_remote_x_scroll_
repaint`)와 같은 계열의 "그림은 바뀌었는데 expose가 떨어지지 않음"일
가능성이 있어, allocation 변경 뒤 40ms에 캔버스 image·scroller의
`queue_draw` + `process_updates(True)`로 동기 repaint를 강제하는 두
번째 안전장치를 넣었다(폴링 fallback은 그대로). 어느 경로가 실제로
동작하는지 보려면 `FLOE_GUI_DEBUG=1`로 실행 — "allocation WxH via
signal|poll" 라인이 찍힌다. 리눅스에서 0.12.62로 재확인 요청.

실측(리눅스, 0.12.62): allocation 라인은 **찍히고**(signal 경로 정상),
창 크기 변경 뒤 마우스를 한 번 클릭하면 정상이 됨. 즉 신호도 repaint
도 문제가 아니라, `size-allocate` 핸들러 **안에서** 동기적으로 새
크기의 pixbuf를 image에 올린 것이 GTK의 layout 패스 도중 크기 요청을
바꾸는 셈이어서 그 패스가 반영하지 못하고, 다음 이벤트(클릭 → 포커스/
pick → 재배치)에서야 새 배치가 잡힌 것이다. 0.12.63: allocation
핸들러는 크기만 기록하고 fit/redraw는 idle에서(`_after_allocate`,
더 새 allocation이 오면 무효) 실행한다. 강제 repaint는 그 뒤에 유지.

## 4. 이슈 목록

| ID | 우선순위 | 상태 | 요약 | 다음 판정 |
|---|---:|---|---|---|
| F2R-01 | P1 | `DONE` | cache hit까지 128-page refine | cache-aware batch/unit+현장 gate 완료 |
| F2R-02 | P1 | `DONE` | 128px tile의 반복 hierarchy 순회 | 제품 기본 384px 승인 |
| F2R-03 | P1 | `DOING` | tile x layer x hierarchy 총 CPU 작업량 | 2c 완결(§3.17, 원점 13배) + 지연-에지 tile 128 축소(0.12.39, §3.21 한-tile 집중 해소) 재측정 대기. 남음: 03c(트리거 미발화 유지) |
| F2R-04 | P1 | `DONE` | total에서 사라진 PNG/publish 37~44ms | write/sync/rename/handoff 계측 완료 |
| F2R-05 | P2 | `DONE` | jobs=8의 CPU/전력 비용 | decode 8/raster 4 분리 승인 |
| F2R-06 | P3 | `BLOCKED` | render마다 OS thread 생성 | startup_us가 병목일 때만 pool |
| F2R-07 | P1 | `DONE` | OVC coverage post-composite 회귀 | 제품 경로 제거 gate 유지 |
| F2R-08 | P1 | `DONE` | exact 재방문도 full raster/PNG 반복 | payload cache는 §3.23에서 제거·retained LRU로 승계(0.12.41, 사용자 결정) — 재방문 = k=0 전량 재사용 |
| F2R-09 | P1 | `DONE` | 대형 cold 뷰에서 refinement 왕복이 draw를 ~3배로 | cost-aware collapse 구현(500ms 예산, §3.15) — 실칩 재측정 대기 |
| F2R-10 | P1 | `CLOSED` | exact 밖 인접 pan은 full viewport raster | 실칩 pan 실측으로 경쟁 축 해소(§3.16); world-tile은 F2R-03c 선행 조건부 |
| F2R-11 | P2 | `DESIGN` | page-round refinement가 누적 full PNG 반복 | 기본 no-refinement로 동기 약화; final-tile streaming은 보류 |
| F2R-12 | P1 | `DOING` | KLayout single-core parity와 Rust serial 기준선 | sample9 gate 통과 124%(§3.12); 실칩 p50/p95 남음 |
| F2R-13 | P1 | `DONE` | 인터랙티브 frame의 PNG 인코드/디코드 왕복 | 실칩 확인(§3.16): raw 0.8ms/pub 8.4ms, raster 무회귀 — wall ~50ms/frame + 주 스레드 디코드 제거 |
| F2R-14 | P1 | `DONE` | 장시간 세션에서 load 증가(미니맵 이동, fresh 뷰어보다 느림) | LRU 축출 전수 스캔 → O(log n) 색인(0.12.33, §3.18) + `evict N` telemetry — 실칩 장기 세션 재확인 대기 |
| F2R-15 | P1 | `DONE` | pick/snap이 대부분 "no object here" | 예산 모델 폐기·floe 정렬(0.12.36, §3.19) — 9.8G 실칩 확인 완료. cut 실명(결함 B)은 보류(설계 219259c) |
| F2R-16 | P1 | `DONE` | pan이 이동량과 무관하게 전체 재raster | 실칩 확인(§3.20): 10% pan draw 378ms(재사용 92%)·50% 845ms(50%) — 이동량 비례 회복. 남은 pan 바닥: collection/trial ~0.3s, plan ~0.2s(비gated 후보) |
| F2R-17 | P1 | `DONE` | pan 대비 배경 margin prefetch(사용자 제안) | 한 스텝+pad margin·즉시 제출·비행 가드(§3.22) — 사용자 확인 완료: 연속 패닝 거의 무음. P0 리뷰 수정(0.12.43): Rust capability+frame-cache 게이트로 floe(KLayout)에는 적용되지 않음. 0.12.44: margin 폭 정확히 한 스텝/변(−20% raster, 사용자 지적). 실칩(폐쇄망) 확인만 남음 0.12.55: 0.12.54(방향 prefetch+동결) 실칩 detail high 조작감 저하로 원복, 검은 strip 미해결 |
| F2R-18 | P2 | `DONE` | exact frame cache가 retained와 중복(사용자 결정) | payload cache 제거, retained 3-entry LRU(scale별)로 통합(0.12.41, §3.23) — zoom 왕복은 k=0 전량 재사용으로 승계 |
| F2R-19 | P1 | `DONE` | 수직 pan에서 가장자리 깨짐 | 재사용 row 매핑 부호 교정 + height mod-16 guard(0.12.42, §3.24) — 8방향 pixel-diff 재확인, 실칩 재확인 대기 |
| F2R-20 | P1 | `DONE` | retained frame byte 예산·margin 픽셀 캡·전체 복제 제거(리뷰 HIGH) | `FLOE_RUST_RETAINED_MB`(256)·`FLOE_MARGIN_MAX_MPIX`(16)·raw 2-part 게시·레이블 없는 clone 생략·Python slice 복제 삭제(§3.25). 0.12.47: frame-cache off는 keep/clone/store도 안 함. 4K 실측 없음 |
| F2R-21 | P2 | `DONE` | margin crop 라벨 정확도(리뷰 MEDIUM ×2) | margin은 geometry 전용, 라벨 on이면 crop 없이 renderd 라벨 재합성 fast path(page plan·decode 생략), 라벨 off만 crop; bin-정렬 declutter 유지; oracle: 라벨 off crop byte 동일 + 라벨 on 전량 재사용 pan byte 동일(§3.26a, 0.12.48). 0.12.49: fast path는 published scene이 요청을 서비스할 때만(레이어 토글 pick 회귀), 포함하는 margin은 교체 대신 유지. 0.12.50: 착지 margin을 표시 base로 blit해 pan 새 영역 검정 제거 — 사용자 확인 완료. 0.12.53(사용자 결정): 라벨 포함 margin 복귀, pan은 라벨 포함 0ms crop, 꼬리 겹침은 표시 규약으로 수용, truncated margin만 crop 제외 — 사용자 확인 완료. 실칩(폐쇄망) 체감 확인 남음 |
| F2R-22 | P2 | `DONE` | pick/snap의 renderd 입력 스레드 점유(리뷰 MEDIUM) | 질의 스레드+kind별 seq frontier(superseded 중단), `FLOE_RUST_QUERY_INLINE=1` 킬 스위치(§3.26b) |
| F2R-23 | P1 | `TODO` | 불완전 결과·fallback 관측성(§3.27) | snap partial 표시/오류, frame 생략·footprint 융합 플래그, mini-bin overflow·실패 trial 시간·Pts fallback·mask fallback 계측, renderd RSS를 frame line에 |
| F2R-24 | P2 | `TODO` | decode 전 byte admission(§3.27) | 페이지 메타 크기 추정으로 뷰당 상한을 decode 전에 판정. 03c와 별개 |
| F2R-25 | P2 | `TODO` | 라벨 bin 기반 선택의 제한적 검증(§3.27) | 축정렬 규칙 Grid fast path만, cap 유지, 새 승자 규칙·block/Pts/긴 문자열 계약 문서화 후 |
| F2R-26 | P3 | `GATED` | 실측 기반 fallback 개선(§3.27) | F2R-23 계측으로 work-bin continuation·Pts 과포함·mask 전역 해제 중 실제 병목을 선택 |
| F2R-27 | P3 | `DESIGN` | generation streaming/1bpp + 질의 경로 분리(§3.27) | 실칩에서 뷰당 1GiB 오류 또는 peak 메모리 압박이 확인될 때, 03c와 pick/snap의 페이지 의존 구조를 함께 설계 |

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

### F2R-03 — 반복 hierarchy/layer traversal 제거와 raster 상수 비용

현재 구조는 `tile -> paint layer -> render_cell hierarchy -> page records`다. page bbox
prune과 큰 tile은 중복을 줄이는 완화책일 뿐이다. jobs=1도 tile=1024에서는 raster
126.5ms까지 내려왔지만 total 149ms로 floe 129ms의 95% 목표에는 아직 약 16% 부족하다.

2026-08-26 코드 분석으로 원인을 다음과 같이 확정했다
(`rust/render-core/src/raster.rs` 기준).

1. tile 축 중복: `raster_tile()`이 tile마다 `scene.top()`부터 hierarchy 전체를
   재순회한다. culling은 page bbox 교차와 repetition 해석 prune뿐이라 instance
   순회(place/compose/invert, member 전개)는 tile 수만큼 반복된다. §3.3의
   jobs=1 tile sweep(49 tile 744ms → 1 tile 126.5ms)이 이 곱셈의 실측이다.
2. layer 축 중복: tile당 styled layer마다 `render_cell()` 전체 순회에 frame
   band 4회가 더해져 tile 하나가 (L+4)회 hierarchy를 걷는다.
   `FrameScene::cell_bounds`는 cell당 전체 bbox 하나뿐이라 layer별 subtree
   pruning이 없고, 해당 layer에 아무것도 없는 subtree도 끝까지 재귀한다.
   `cell.pages`와 label rows도 layer마다 전체 재스캔한다. 순회 비용이
   `O(L × 전체 가시 instance)`로 스케일하며, 단일 tile에서도 남는 잔여 16%의
   주요 후보다.
3. `Rep::Pts`는 tile·layer 방문마다 전체 point를 스캔한다
   (`repetition.rs`). PLAN §7.2의 "선택 subset 또는 chunk bbox" 계획이
   미구현 상태다.
4. shape/pixel 상수 비용: 2-phase fill(PixelCenter ∪ LowerBoundary)에 무조건
   outline stroke가 더해지고 PATH는 4패스다. span 내부 루프가 pixel마다
   stipple 판정·u32 변환·4B RGBA 쓰기를 수행하고, member마다 world_points
   Vec, scanline row마다 intersections Vec을 새로 할당하며, 같은
   world→device 변환을 phase마다 반복한다. KLayout은 layer별 1bpp bitmap에
   word 단위 span을 쓰고 dither는 최종 합성에서 word 단위로 적용한다.

단계 (모두 jobs/tile byte 결정성과 half-phase oracle을 유지):

- **F2R-03a (`DONE` — sample9, 2026-08-26)** — 순회 구조 불변의 상수 비용
  제거. 적용 내용: `fill_span()` fill-종류별 특수화(기존 per-pixel `fills()`는
  test 전용 oracle로 강등, 동등성 unit gate 추가), polygon fill의 device 변환
  1회화, scanline row별 intersections 재사용, polygon/path member scratch
  재사용, stroke 꼭짓점 변환 절반화, **axis-aligned solid stroke의 span fast
  path**(stepped oracle 동등성 unit gate 추가). 결과와 검증은 §3.10 —
  serial raster 136→44ms, 제품 raster 80→26ms, 출력 byte 불변.
  대표 실칩 재확인은 F2R-12 잔여 측정과 함께 수행한다.
- **F2R-03b 1단계 (`DONE` — sample9, 2026-08-26)** — sub-page record extent
  index. 구현·수치·비용·검증은 §3.11. 제품 기본 raster 26.0→20.4ms,
  128px tile 58.7→39.9ms, serial 동률, 출력 byte 불변. repetition 선행
  전개 없이 extent만 색인하며 index bytes는 decoded LRU budget에 계량된다.
  대표 실칩 재확인은 F2R-12 잔여 측정과 함께 수행한다.
- **F2R-03b 2단계 (`READY` — 설계 확정 2026-08-26)** — 세 요소를 독립
  gate로 나눠 구현한다. 공통 원칙: 출력 byte 불변, repetition 선행 전개
  금지, 모든 보조 구조는 LRU/frame budget에 계량, query/clip 경로 불변.

  **2a. Pts chunk index** (`DONE` 2026-08-26 — decode 시, `PageIndex` 확장)
  - 문제: 1단계 후에도 방문된 record의 `Rep::Pts`는 tile·방문마다 전체
    point를 스캔한다(`repetition.rs` Pts arm). 실칩 fill 데이터는 수천
    point Pts가 흔하다.
  - 설계: point 수 임계(초안 256) 이상인 Pts에 대해 decode 시 파일 순서
    그대로 64-point chunk의 offset bbox 배열을 record별 side table로
    `PageIndex`에 보관한다(≈0.5B/point, `estimated_bytes()` 계량).
    raster 전용 진입점이 chunk bbox와 `offset_region`을 먼저 교차
    검사하고 교차 chunk의 point만 기존 순서로 스캔한다. 중복·원본 순서
    보존 계약(`pts_preserves_duplicates_and_source_order`)은 chunk 순차
    순회로 자동 유지된다.
  - gate: chunked/full-scan의 visible 집합·pixel 동일성 unit oracle,
    대형 Pts fixture에서 `rep_members_tested` 감소, KLayout oracle 유지.
  - 완료(2026-08-26): 구현에 두 가지 설계 보강이 추가됐다. (1) 공유 `Arc`
    point list 단위로 테이블을 dedupe한다(OASIS modal 재사용 대응). (2)
    **축별 선택도 gate** — 파일 순서가 공간적으로 무질서한 list는 chunk가
    전체를 덮어 prune이 0이므로 테이블을 버리고 기존 full-scan을 유지한다.
    한 축이라도 평균 chunk 폭이 전체의 1/4 이하면 유지하는데, KLayout
    writer가 point list를 y-정렬해 기록함을 실측으로 확인했으므로
    KLayout 계열 flow가 쓴 실칩 파일은 y축 선택도로 자연히 적중한다.
    200k-point 합성 fixture(858x789) 실측 `rep_members_tested`:
    100µm window에서 200,000→1,216(자체 row-major)/10,240(KLayout
    y-sorted, -95%), 1µm window에서 256/384. wall-time은 framebuffer
    clear/copy 바닥(≈0.3ms)에 가려 tiny-window에서 -43%로 나타난다.
    sample9는 대형 Pts가 없어 회귀 없음(PNG md5 동일). `floe-render-cli`
    raster 라인에 `rep_members_tested/drawn`을 추가했고 재현 generator는
    `rust/oasis/examples/pts_bench_gen.rs`다. 검증: 동등성·비선택 보호
    unit oracle, workspace 15 suite, KLayout oracle jobs 1/8, PX golden,
    integration 22 tests 전부 통과.

  **2b. cell×layer subtree mask** (`DONE` 2026-08-27 — FrameScene 조립 시)
  - 문제: `render_cell()`이 styled layer마다, `render_frame_band()`가
    band 4회 hierarchy 전체를 재귀하며, subtree에 해당 layer가 없어도
    instance repetition을 전개한다. 실칩 mid-zoom(wc_cells 수천 × layer
    수십 × tile 수)에서 이 곱셈이 지배 후보다.
  - 설계: `FrameScene` 조립에서 plan-local layer를 dense index로 매핑해
    cell별 subtree layer bitmask와 frames-보유 bit를 bottom-up 1회
    (memoized DFS, O(cells+edges+pages))로 만든다. per-frame 구조라 포맷
    변경이 없다. `Layer(l)` 순회는 subtree mask에 l이 없는 child로
    재귀하지 않고, frame band 순회는 frames 없는 subtree를 건너뛴다.
    band 범위 검증(>3 거부)은 방문 의존을 없애도록 scene 조립 1회로
    옮긴다.
  - gate: mask on/off pixel 동일성 unit oracle, 오류 도달성 유지(mask는
    pages/frames가 실제로 없는 subtree만 자르므로 오류 record를 숨길 수
    없음을 테스트로 고정), 신규 cell-방문 telemetry 감소 확인.
  - 완료(2026-08-27): `SceneMasks`가 decoded page의 `layer_idx`와 wash
    layer만 색인한다 — deferred page는 의도적으로 제외(순회도 건너뛰므로
    pixel 불변). 오류 도달성 보존 규칙 두 가지를 구현에 고정했다: (1)
    cycle back-edge는 조립을 실패시키는 대신 방문 cell을 full mask로
    flood해 순회가 계속 내려가 자체 cycle 오류에 도달하게 한다. (2) plan
    에 없는 child는 mask가 답하지 않고(무조건 true) 순회의 missing-cell
    검증이 그대로 발화한다. occupancy(`All`)는 gate 없음. telemetry
    `hier_cells_visited`/`subtrees_pruned`를 `RenderStats`와 render-cli
    raster 라인에 추가했다. 측정: sample9 hotspot(§3.12 설정)은 얕은
    계층(wc_cells 22)이라 회귀·이득 모두 없음(serial raster 44.6→43.6ms
    동률, occupancy PNG md5 동일). 합성 deep-hier fixture(8 layer × 8
    chain × depth 5 2x2 array, 800x778 tile128 j1)에서 styled raster
    24.3→16.6ms(-32%), `rep_members_tested` 223,424→101,512(-55%),
    `subtrees_pruned` 2,744, PNG md5 동일. 검증: mask on/off·corrupt
    도달성·cycle flood·frame prune unit oracle 포함 render-core 84개,
    `validate_rust.sh` 전체 배터리(KLayout oracle jobs 1/8, PX/STYLE
    골든, 실daemon integration) ALL OK. 실칩 deep-zoom 재측정(§3.13):
    **draw 439→22ms(-95%)**, KLayout draw 142ms 대비 6.5배 우위,
    end-to-end 1048 vs 149ms. 실칩 이득의 본체가 이 단계였다.

  **2c. frame당 1회 (image tile × paint plane) work bin**
  - 문제: 2b 후에도 tile마다 hierarchy walk 자체는 반복된다. 4-worker
    CPU 합계, F2R-10 world tile, F2R-11 dependency graph가 공통으로
    frame-level 가시 item 목록을 요구한다.
  - 설계: FrameScene 확정 후 round당 1회, 취소 가능한 단일 traversal이
    가시 (page_id, 누적 OrthoTransform) item과 frame-band item을
    수집한다. instance repetition은 frame view에 대한 해석적 가시
    범위만 전개하므로 item 수는 "한 tile=전체 화면" 순회의 방문 수와
    같다(선행 전개 아님). item world bbox로 교차 tile에 binning하고
    (plane, tile) counting-sort로 정렬하면 tile worker는 hierarchy 없이
    자기 bin의 item만 record index에 질의한다. plane 순서가 기존 paint
    순서를 보존하므로 pixel 불변.
  - 한도·폴백: item 수와 구성 시간에 상한을 두고 초과 시 현재 per-tile
    순회로 폴백한다(정확도 동일, `work_bin=off` telemetry). bin 메모리는
    frame 수명으로 계량한다.
  - 확장 키: binning 함수를 분리해 tile key를 viewport-local 대신
    world/scale 정렬 tile로도 계산할 수 있게 한다. F2R-10 world-tile
    LRU와 F2R-11 page→tile dependency가 같은 bin을 소비한다.
  - gate: bin on/off PNG byte 동일(jobs 1/4/8 × tile 128/384/858),
    hotspot 회귀 없음 + 실칩 mid-zoom 개선, bin 구성 중 취소를 포함한
    cancellation soak stale publish 0.

  **2c 완료(2026-08-28)**: FrameScene 확정 후 round당 1회의 취소
  가능한 수집 walk이 DFS 순서로 (page, transform)·wash·frame item을
  모으고, tile worker는 hierarchy 없이 자기 plane의 item을 tile bbox로
  걸러 기존 record-index 질의를 그대로 실행한다. 조상 bbox culling이
  보수적 superset 필터이고 plane별 item 순서가 walk의 DFS 순서라 tile
  별 paint 시퀀스가 walk과 동일 — **byte 동일 oracle**(scene 3종 ×
  jobs {1,4} × tile {8, 기본})로 고정했다. 설계 편차 1건: item을
  (plane, tile)로 counting-sort binning하는 대신 plane별 평면 리스트를
  tile마다 bbox 필터한다 — §3.15 규모(45 tile × ~수만 item)에서 스캔
  비용이 무시 가능해서이며, world-tile 확장 키(F2R-10/11)는 이 지점에
  그대로 남는다. 2b gate는 수집 walk의 **결합 질의**(styled layer 합
  ∪ frames)로 이동해 plane별 gate 3억 회(§3.15)가 walk 1회분으로
  줄어든다. 한도: item 상한 768k(≈64MB, frame 수명) 초과 시 기존
  per-tile walk 폴백(`work_bin_items=0` telemetry, pixel 불변). kill
  switch `FLOE_RUST_WORK_BIN=off`, render-cli `--work-bin`. 실측:
  deephier(8 layer × 49 tile, j1) hier 방문 17,808→2,729(-85%)·PNG
  md5 동일, sample9 제품 r4/384 raster 20.3→16.0ms(-21%). occupancy
  경로는 oracle 기준이라 walk 유지. 대형 실칩 mid-zoom 개선 폭은
  재측정으로 판정한다.

  **2c 지연 gate 재조정(2026-09-02, 0.12.27, §3.17)**: 실칩 depth
  제한 뷰에서 members=1 대형 블록의 지연이 tile별 재walk(hier
  930k/60.8M)로 나타나 gate를 분리했다 — 반복(members>1)은 기존
  `members×weight>4096`, 단일 배치는 잔여 item 예산(×4) 안에서 항상
  전개. 정책 변경이라 pixel 불변이 자동이며 신규 정책 테스트로
  고정(93 tests).

  **착수 판정과 순서**: 실칩 bench의 wc_cells/inst_edges/render_tiles/
  layer 수로 traversal 곱셈 비중을 추정해 2b·2c의 기대 이득을 정한다.
  구현 순서는 2a → 2b → (실칩 판정 후) 2c. record 수 대비 traversal
  비중이 낮으면 2c는 F2R-11 채택 시점까지 미룬다. 1단계 index build가
  실칩 decode에서 병목으로 나타나면(§3.11: sample9 decode +70%) index를
  첫 raster 사용 시 lazy 구축하는 변형을 2a와 함께 검토한다. OVP에
  사전 계산 index를 저장하는 안은 포맷 변경이라 별도 결정으로 남긴다.
- **F2R-03c (`DESIGN`, 착수 기준 확정 2026-08-28)** — 착수 조건:
  대표 실칩 p50/p95에서 **paint가 draw를 지배하면서 KLayout 대비
  1.5배 이상 느린 뷰가 실측될 때만** 진행한다. 2c 이후 실측(§3.15
  4차)에서 paint 몫은 KLayout과 동률(~1.1s vs 1.10s)이라 현재 기대
  수익(≤0.2s)이 재작업 규모(전체 paint 프리미티브 + 픽셀 계약 +
  oracle 재정비)에 크게 못 미친다. 먼저 소진할 지렛대: deferred
  subtree tile-binning(**소진 2026-09-02** — §3.17의 members=1 전개
  gate, 실칩 재측정 대기), 패턴 fill 지배 뷰 존재 여부 실측.
  본래 설계 — fill 파이프라인 재설계: 2-phase를 단일 edge walk
  통합, 장기적으로 layer별 1bpp plane + 최종 word 합성. paint 순서 계약
  (PLAN §8.3 "레이어 병렬 합성 금지")의 재개정이 필요할 수 있어 F2R-03a/b
  측정 뒤 별도 승인으로만 진행한다.

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
- `FLOE_RUST_RASTER_JOBS=1`은 KLayout 대비 core 효율을 재는 first-class profile로
  유지하되, 95% gate 전에는 제품의 자동 warm 전환에 사용하지 않음.
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

### F2R-08 — exact viewport 재방문 (`DONE`)

- key: float viewport bits, framebuffer, depth/cut, exact, layer selection,
  frames/labels/font, mono, decode cap, style epoch
- value: 최종 deterministic PNG와 label truncation 상태만 보관. `FrameScene`은 보관하지
  않아 decoded-page budget을 우회하지 않음
- bound: 최대 3 frames / 64MiB, style 변경·새 cache open에서 즉시 clear
- hit: 선택 page가 전부 decoded LRU resident일 때만 scene을 재구성하고 cached PNG를
  기존 fsync+atomic rename 경로로 게시
- gate: exact 두 번째 generation의 PNG bytes 동일, raster/png 0,
  `frame_cache_hit=1`, 복원 뒤 KLayout pick/snap parity 유지

### F2R-09 — interactive cold-miss round (`DONE`, 기본 refinement 해제 2026-08-28)

- 원인: 128-page decode round마다 지금까지의 누적 scene 전체를 다시 raster/PNG 게시
- 제품 정책: 1024 miss pages까지 single settled frame. 그 이상에서만 progressive
- 근거: GUI는 새 frame을 기다리는 동안 직전 완성 frame을 frozen preview로 표시하므로
  sub-second 작업의 partial PNG가 검은 화면을 막아 주는 역할을 하지 않음
- gate: adapter 기본 wire `round_pages=1024`, benchmark `--round-pages` 기본과 일치,
  1024 초과 unit/cancellation progressive 계약 유지
- **cost-aware collapse(2026-08-28, §3.15 재개분)**: 대형 cold 뷰에서
  round당 재raster가 draw 자체가 됐다(실측 rounds 5 = 단발 draw의
  ~3배, 11.7초 중 10.9초). 이제 어떤 round의 raster가
  `REFINEMENT_RASTER_BUDGET_US`(500ms; env
  `FLOE_RUST_REFINE_BUDGET_US` override)를 넘으면 남은 batch를 최종
  1 round로 병합한다 — 싼 round는 그대로 스트리밍하고(sample9
  round 8 강제 시 rounds 5 유지), 넘는 순간 정확히 한 번의 최종
  raster만 남긴다(예산 강제 축소 e2e: rounds 5→2, 누적 raster
  55→21ms). "500ms 이하 작업에 refinement를 만들지 않는다" 규칙의
  쌍대다. §3.15 뷰 예상: draw 10.9→~4.3초, total floe 역전 — 실측
  5.0초로 확인(§3.15 후속).
- **기본 refinement 해제(2026-08-28, 사용자 결정)**: cost-aware 2
  round(5.0초, floe 6.2초 역전)조차 단발 대비 손해라는 실칩 판정에
  따라 제품 기본을 **중간 frame 없음**(adapter wire
  `round_pages=2^30`)으로 바꿨다 — floe도 view당 draw 1회다. bench
  `--round-pages` 기본도 동일. 스트리밍 round는
  `FLOE_RUST_ROUND_PAGES`로 복원 가능하며 그 경우 500ms cost-aware
  collapse가 안전망으로 남는다. §3.15 뷰 기대: round1 재raster까지
  제거돼 draw 추가 감소.

### F2R-10 — same-scale 인접 viewport retained 재사용 (`CLOSED` 2026-09-01)

확정된 현재 경계:

- GUI `last_frame`/`_covered()`는 floe와 floe2 공통이며 넓은 인접 cache가 아니다.
- floe는 persistent KLayout `Layout`/`LayoutView`와 resident page-cell을 유지한다.
- floe2의 decoded-page hit는 raw geometry read/decode만 줄이고, 새 viewport의 scene
  traversal/raster/PNG는 줄이지 않는다.
- exact frame cache는 zoom 복귀에는 유효하지만 좌표가 다른 인접 pan에는 맞지 않는다.

관찰 갱신(2026-08-26): 실칩 재관측으로 floe의 인접 이점은 draw가 아니라
**load 단계**로 확정됐다 — 첫 방문 8~9초 loading이 재방문(정확한 위치가
아니어도)에서 2초 내외로 준다. §3.12의 sample9 sweep과 부호가 일치한다:
floe는 인접 pan마다 full draw를 다시 지불하지만(73~96ms), persistent
Layout에 apply된 page-cell은 세션 내내 남아 load(plan/delta/apply)만
급감한다(cold apply 296ms → warm 1~9ms). floe2의 대응물은 decoded-page
LRU(기본 `FLOE_RUST_BUDGET_MB=1024`)이므로, 실칩 working set이 budget을
넘거나 영역을 오가면 재방문에도 read/decode를 다시 지불한다. 따라서 실칩
sweep의 1차 판정 축은 draw 비교가 아니라 **재방문 load — floe의
plan/delta/apply vs floe2의 read/decode/cache_miss** 다. world-tile LRU
이전에 검토할 후보 대응은 (1) 실칩 프로파일 기반 decoded budget(호스트
메모리 비례 adaptive), (2) decode 처리량 — 1단계 record index build가
decode를 키우므로(§3.11 sample9 +70%) 실칩에서 병목이면 lazy 구축 변형,
(3) F2R-11 center-first streaming이다. bench report가 두 backend의 해당
phase를 모두 담으므로(§3.12) 같은 두 명령으로 바로 판정한다.

관찰 갱신(2026-08-27): 실칩 GUI cold deep-zoom 실측(§3.13)이 load 축을
수치로 확정했다 — 같은 +194 page에 KLayout load 906ms vs floe2 71ms
(12.8배), end-to-end 1048 vs 564ms. 남은 판정은 **재방문**(working set이
LRU에 남은 상태)의 load 잔량과 budget 초과 여부이며 pan-sweep trace로
잰다.

원인 확정(2026-08-28): "floe는 인근 이동이 캐시 히트인데 floe2는 다
다시 로딩한다"는 현장 체감의 근원은 **같은 1024MB 예산의 회계 기준
차이**다 — vfsd ledger는 ENCODED bytes(`usize_`), floe2 LRU는
실메모리(decoded+index) 기준이며 sample9 hotspot 실측 13.8MB vs
211.4MB, **15.3배**. 같은 숫자로 floe가 약 15배 넓은 방문 이력을
유지한다(그 대신 KLayout Layout의 실제 RAM은 회계 밖).

**운영 정책(2026-08-28, 사용자 결정)**: 호스트 RAM 비례 adaptive
기본값을 구현했다가 **철회**했다 — 뷰어가 공유 서버에서 돌므로
RAM 절반 기본값은 이웃 프로세스에 위험하다. 기본은 1024MB 고정
유지, floe급 보존이 필요한 세션만 `FLOE_RUST_BUDGET_MB`로 명시
opt-in한다. budget 초과 재방문은 OS page cache가 인코드 캐시
구간을 들고 있으므로 read가 아닌 **decode-only 비용**으로
기대한다(sample9 실측 read는 decode의 0.6%). 따라서 이 축의 다음
지렛대는 decode 절감 — index build가 decode CPU의 41%(§3.14)라
**lazy-index 변형**이 1순위이고, 실칩 `read_ms`가 유의미하게 나오면
(네트워크 저장소) 인코드 victim cache를 재검토한다. 세션 시작
`[perf] backend=rust renderd=… budget-mb=…` 라인이 유효값을 찍는다.

**종결(2026-09-01, §3.16)**: 실칩 pan 실측(상하좌우 20% 이동)에서
floe2 draw가 floe보다 약간 빠른 것으로 확인 — load 축(§3.15 9.5배)
에 이어 draw 축까지 실칩에서 닫혔다. 아래 world-tile 구현 후보는
**기각/보류**: fill 위상이 device-anchored라 RGBA tile 재사용은 byte
동일 gate를 통과할 수 없고, byte-exact 재사용은 F2R-03c(1bpp plane)
선행이 필요하다. 재개 조건과 판정 근거는 §3.16. 이하 절차·gate
기술은 기록용으로 유지한다.

먼저 같은 zoom/detail/depth/layer에서 warm settle 후 X/Y로 화면 폭의
`1/16, 1/8, 1/4`만큼 이동하고 되돌아오는 trace를 각각 3회 측정한다
(`tools/bench_floe2.py --pan-sweep`; sample9 결과는 §3.12). floe는
plan/new/apply/draw/total, floe2는 plan/cache hit/scene/raster/PNG/publish/total과
process CPU를 함께 기록한다. 진단용으로 같은 KLayout working `Layout`에서 persistent
`LayoutView`와 매 요청 새 `LayoutView`도 비교해 native retained renderer의 기여를
분리한다.

구현 후보는 viewport-local 384px tile이 아니라 world/scale에 고정된 RGBA tile LRU다.
key에는 world tile 좌표, scale, depth/cut, visible layer, style epoch, frame/mono 상태를
넣는다. label declutter는 viewport 의존이므로 geometry tile과 분리한다. 384px RGBA
64개는 약 36MiB이며 이 수준의 명시 상한 안에서 시작한다. zoom 변경은 miss로 처리하고
현재 exact frame cache는 정확한 zoom 복귀용으로 유지한다.

수용 gate:

- same-scale 인접 pan에서 `world_tile_hit/miss`를 계측하고 겹치는 tile을 다시 raster하지
  않음.
- 대표 1/8-width pan의 settled total 중앙값이 현 full-raster보다 20% 이상 개선되고,
  exact/cold trace는 10% 넘게 회귀하지 않음.
- speckle device phase, hierarchy/geometry edge, layer paint order가 jobs/tile 수와 무관하게
  기존 PNG와 byte-identical.
- cache는 page/generation budget을 우회하지 않고 style/depth/layer/scale 변경에서 정확히
  무효화됨.

### F2R-11 — PNG 없는 multi-thread final-tile refinement (`DESIGN`)

목표는 큰 cold view의 first paint를 유지하면서 같은 framebuffer를 round마다 다시
그리지 않는 것이다. 다음 구조는 후보이며 아직 제품 결정이 아니다.

1. 전체 viewport plan과 transformed page→image-tile dependency를 한 번 만든다.
2. page를 decode pool에서 병렬 로드한다.
3. 필요한 page가 모두 준비된 final tile을 center-first raster queue에 넣는다.
4. raster worker가 각 tile을 정확히 한 번 완성하고 generation-tagged raw RGBA/shared
   framebuffer에 게시한다.
5. interactive GUI는 tile-ready dirty rect만 합성한다. headless/export만 final PNG를
   한 번 encode하고 기존 atomic publish 계약을 탄다.

새로 decode된 page만 이전 pixels 위에 덧칠하는 방식은 채택하지 않는다. 새 page의
geometry가 기존 pixel보다 아래 paint plane에 놓일 수 있고 opaque speckle/outline/frame
순서도 있어 단순 incremental alpha composite는 정확하지 않다. 각 tile은 모든 의존
page가 준비된 뒤 최종 paint order로 한 번 그려야 한다.

결정 전에 query 계약도 고정해야 한다. 안전한 초기안은 partial tile이 보이는 동안
pick/snap은 이전 settled scene을 유지하고, 모든 tile 완료 시 새 `FrameScene`과 화면을
함께 전환하는 것이다. 부분 화면에 대한 tile별 query snapshot은 복잡도가 커 첫 구현
범위에서 제외한다.

수용 gate 후보:

- generation당 final image tile raster 횟수는 tile 수 이하이고 누적 full-frame pass는 0.
- direct-final 대비 settled overhead 10% 이하이면서 장시간 fixture first paint는 목표
  시간 안에 도착.
- jobs 1/4/8, tile 완료 순서와 무관하게 final RGBA/PNG bytes 동일.
- cancel 이후 stale tile/scene publish 0, raw framebuffer와 tile-ready 상태의 bounded 정리.
- GUI 경로의 intermediate PNG encode/write/sync/rename은 0이고 headless final atomic
  publish는 유지.

F2R-03의 work bin/transform 공유는 F2R-10의 world tile과 F2R-11의 dependency graph가
공통으로 요구하는 선행 작업이다.

비교 gate는 공통 `--refinement off`와 `--perf-baseline`을 먼저 실행해 direct-final
비용을 고정한 뒤 refinement on의 first/settled overhead를 별도로 계산한다. exact
frame cache hit는 이 비교에서 허용하지 않는다.

### F2R-12 — KLayout single-core parity와 split worker gate (`READY`)

목표는 멀티코어로 Rust의 중복 작업을 가리는 것이 아니라, 같은 single-raster 조건에서
KLayout의 95% 처리 성능을 먼저 달성하고 병렬 raster를 latency 가속으로만 평가하는
것이다. page decode와 raster의 역할이 다르므로 `tools/bench_floe2.py --jobs`가 둘을
같이 바꾸는 현재 scaling mode만으로는 이 조건을 고정할 수 없다.

착수 항목:

1. 완료(2026-08-26): floe `_VIEW_CONFIG`에 `drawing-workers=1`을 명시하고 render
   service 시작 시 `[perf] backend=klayout version=.. drawing-workers=..` 한 줄로
   실제 설정을 기록한다 (`floe/render.py`, `floe/service.py`).
2. 완료(2026-08-26): `tools/bench_floe2.py`에 `--decode-jobs`를 추가했다. 지정 시
   decode worker는 고정되고 `--jobs`는 raster worker만 sweep하며, 세션 로그와
   JSON report에 두 값을 분리 기록한다.
3. 완료(2026-08-26): `--serial` preset이 `decode_jobs=8`(미지정 시),
   `raster_jobs=[1]`, tile `min(4096, max(width, height))`를 고정해 한 화면을 한
   tile로 그린다.
4. 완료(sample9, 2026-08-26): canonical sample9로 Rust serial/제품 profile과
   같은 머신 KLayout 기준선을 3회씩 측정했다(§3.10~12). bench가
   `--backend klayout`으로 stable service를 같은 trace로 headless 구동하며,
   `[perf]` 라인이 version/worker 고정을 증명한다. gate는 124%로 통과.
   남음: 대표 실칩 p50/p95.

수용 gate:

- KLayout과 Rust 모두 refinement/LOD/frame/label/exact frame cache가 꺼진 동일
  `--perf-baseline` work를 선택함.
- sample9 500um warm Rust serial total이 149ms에서 136ms 이하로 내려감.
- 대표 실칩 p50/p95가 floe/KLayout single의 1/0.95배 안이고 pixel oracle과 jobs/tile
  byte 결정성이 유지됨.
- serial gate 통과 전에는 제품 기본 `decode=8/raster=4/tile=384`를 변경하지 않음.
- gate 통과 뒤에도 raster=1 기본과 adaptive 1/4 전환은 total latency와 CPU-seconds를
  함께 비교해 별도 승인함.

### F2R-13 — 인터랙티브 frame handoff의 PNG 제거 (`DONE` 2026-09-02, 0.12.26)

문제: 인터랙티브 경로의 frame 전달이 renderd PNG 인코드(§3.15 실칩
206ms/824x796, 최종 라인 잔여 ~135ms) → 파일 publish → adapter 파일
read → **GUI 주 스레드의 PixbufLoader PNG 디코드** 순서라, raster가
KLayout과 동률이 된 뒤에도 매 pan/zoom/style frame이 양쪽 코덱
비용을 고정으로 지불한다. floe(KLayout)는 화면 pixel buffer를 직접
쓰므로 이 왕복이 없다.

구현(0.12.26): render 명령에 `frame_format=raw|png`(renderd 기본
png — wire 하위호환). raw는 PNG 인코드 대신 `FLOERAW1` magic +
u32le width/height + packed RGBA payload를 **같은 원자적
publish/generation 계약**으로 게시한다. adapter는 헤더/크기를 job과
대조 검증 후 `rgba`로 전달하고, GUI는 `Pixbuf.new_from_bytes`로
디코드 없이 표시한다(카피 2회, ~2ms). exact frame cache는 payload
포맷을 key에 포함해(`FrameCacheKey.raw_frame`) 교차 히트를 막고,
raw hit는 GUI 디코드까지 사라진다. 인터랙티브 기본은 raw
(`FLOE_RUST_RAW_FRAME=off` kill switch), headless 소비자(CLI export,
DRC error sheet)는 job별 `frame_format=png`으로 실제 PNG 바이트를
유지한다. perf 라인은 `png N.Nms` 대신 `raw N.Nms`를 찍는다.

수용 gate:

- raw payload가 `RgbaFrame.pixels()`와 byte 동일(renderd unit), 크기
  검증은 adapter가 job 크기와 대조(validator unit — 손상 payload는
  error 응답).
- exact 재방문/frame_cache off/round partial 경로가 raw에서 동작
  (실daemon integration이 payload 동등으로 검증, partial 파일 잔존 0).
- PNG 소비자(export/DRC/probe/goldens)는 job override로 불변 —
  PX/STYLE golden과 KLayout oracle 무회귀.
- 실칩 perf 라인에서 raw 인코드+publish+read 합이 기존 png 대비
  감소, GUI 응답성 개선 확인 — **확인(2026-09-02, §3.16)**: raw
  0.8ms/pub 8.4ms(이전 단발 round png ~41ms+pub ~10ms), draw
  1,296→1,284 무회귀. wall 절감 ~50ms/frame + 주 스레드 디코드 제거.

WEBUI_PLAN 수렴: T2(loopback raw RGBA)의 payload가 이 포맷 그대로다
— gateway는 renderd의 raw 산출물을 재인코드 없이 스트리밍한다.

### F2R-23 — 불완전 결과·fallback 관측성 (`TODO`)

문제: §3.27의 "조용히 출력에 영향" 항목(snap 부분 결과, frame_cap
생략, 경계 프레임 Pts 융합)이 사용자와 perf 라인에 드러나지 않고,
출력 불변 fallback(mini-bin overflow, 실패 trial, Pts chunk 과포함,
mask 전역 해제)도 wire에 없어 성능 절벽의 원인을 실칩에서 특정할 수
없다. `resident_bytes`만으로는 peak 메모리를 알 수 없다.

계획: ① snap 응답에 partial 표시(또는 member cap처럼 오류) — GUI는
현재 seq의 응답에만 상태를 띄운다. ② frame line에
`frames_truncated`, `frames_fused` 카운트. ③ frame line에
`bin_mini_overflow`, `bin_trial_us`, `pts_fallback`, `mask_full` 추가,
adapter/bench/GUI perf 라인 전파. ④ renderd 현재 RSS(`rss_bytes`,
표준 라이브러리로 /proc 또는 mach 조회)를 frame line에 실어 peak
회계를 계측한다.

수용 gate: validator wire fixture에 새 필드 파싱, 합성 입력으로 각
플래그가 1회 이상 발화하는 단위 테스트, 실칩 perf 라인에서 필드 확인.
픽셀 경로 무변경(oracle 배터리).

### F2R-24 — decode 전 byte admission (`PARTIAL` 0.12.162)

2026-09-18: 플래너가 페이지 메타(레코드 수·저장 바이트)로 디코드 메모리를 추정해 decode
**전에** 판정한다. 다만 오류로 끝내지 않고 컷을 올려 예산에 맞춘다(0.12.162는 반 옥타브
사다리, 0.12.164부터는 아래 2차 보고에 따라 맞는 후보 중 가장 세밀한 컷)
(SPEC-PLANNER §3 "예산에 맞춘 컷"; 실칩: keep + detail high가 Calibre에 가장 가깝지만
depth 0에서도 예산 초과). decode 뒤 검사는 안전망으로 남아 있다. 남은 것: 추정치/실측치
비율의 frame line 기록.

실칩 확인(사용자, 2026-09-18, a05da06 / 0.12.162): 예산 초과가 더 이상 발생하지 않는다.
아직 기록되지 않은 값: 뷰별 컷 배수(`fit_pct`), 낮춘 밀도의 그림 품질, full depth의
추가 패스 plan 시간.

실칩 2차 보고(사용자, 2026-09-18, 1f6900c / 0.12.163): 그림은 Calibre와 유사하고 어느 정도
확대하면 Calibre보다 밀도가 높다. 그런데 detail medium의 fit view는 1초 이상, detail high의
fit view는 300 ms 정도이며 이 역전이 일정 줌 구간 동안 이어진다. 원인(코드 확인, 실칩
status 값은 미기록): 반 옥타브 사다리는 요청 컷에서만 올라가므로 high(1 px)는 1.41 → 2 →
2.83 → 4 px로 medium의 3 px를 건너뛴다. 가장 유력한 설명: medium은 3 px가 예산에 그대로
맞아 예산 가까이 decode하고(느림), high는 2.83 px가 넘쳐 4 px로 내려앉아 medium보다 거칠고
빨랐다(high가 더 빠르려면 high의 최종 컷이 medium보다 거칠어야 한다). "thin
keep 탐색이 medium에서 더 느리다"는 가설은 합성 입력에서 재현되지 않았다(맞춤이 없을 때
plan 시간은 medium이 더 짧다). 수정(0.12.164): 후보를 요청 컷 × 2^(k/4)(k=1..24)와 표준
detail 컷(1·3·5 px)으로 두고 이분 탐색으로 예산에 맞는 **가장 세밀한** 후보를 고른다.
high가 medium보다 거칠게 끝나지 않으므로 high는 medium 이상으로 느려진다(의도된 결과:
예산을 다 쓰는 뷰의 비용이 약 1초라는 뜻이며, 이 비용 자체를 줄이는 것은 별도 과제).
아직 기록되지 않은 값: medium/high 각각의 `cut<…um xN to fit budget`과 plan/decode/raster
시간 분해.

실칩 3차 관찰(사용자, 2026-09-18, 관찰한 빌드는 미기록): 박스로
바뀌는 부분이 있다는 것과 그 박스가 빨리 사라진다는 것을 빼면, 조금 더 거칠게 나와도
Calibre와 비슷하거나 더 세밀하다. 두 차이의 출처(코드 확인):
- 박스 = M7-C `wash_px` 붕괴(SPEC-PLANNER §3 "워시"): 화면에서 양축 2 px 이하인 페이지는
  디코드하지 않고 자기 레이어의 bbox 렉트 하나로 그린다. 이웃 페이지의 렉트가 이어져 채워진
  면이 된다. 페이지가 컷을 통과해야 하므로(가장 큰 레코드 ≥ 컷) 컷이 2 px 이상이면(detail
  medium, 또는 예산 맞춤으로 컷이 오른 high) 박스 없이 바로 사라진다.
- 빨리 사라짐 = 크기 컷: 페이지는 가장 큰 레코드가, 배치 BVH 서브트리는 가장 큰 자식 셀이
  컷(px) 아래로 내려가는 순간 통째로 빠진다(`cull_size`, `cull_pbvh`, `cull_cbvh`). 페이지
  하나가 레코드 천 개 남짓이라 수십 px 크기의 덩어리가 한 번에 없어진다. Calibre는 버리지
  않고 최소 픽셀 크기로 남긴다. 박스의 수명은 "가장 큰 레코드 ≥ 컷"과 "페이지 ≤ 2 px"가
  함께 성립하는 구간뿐이라 짧다.
이 경계를 무르게 하는 시도(sub-cut wash, page frontier 대표, OVR, 일반 레이아웃 occupancy)는
모두 걷기·생성 비용 때문에 보류 상태다. 판단: 나머지 그림이 Calibre 이상이므로 밀도를 더
낮출 여유가 있고, 그 여유는 예산을 가득 채우는 뷰(약 1초)의 시간을 줄이는 데 쓸 수 있다
(맞춤 목표를 예산보다 작게 두는 방식; 미결정, 목표치는 실칩의 `xN`과 시간 분해로 정한다).
주의: 이것은 박스·사라짐과 **별개의 과제**이고 방향은 반대다. 맞춘 컷이 곧 사라지는 문턱이므로
목표를 낮추면 컷이 더 올라 덩어리가 더 일찍 사라진다. 사라짐을 막는 쪽은 컷 아래 항목을
디코드 없이(페이지·BVH 노드의 bbox만으로) 남기는 일이고 비용은 걷기 시간이다.

시험 구현과 측정(2026-09-18, 미병합 — 패치는 세션 scratch에만 있다): 크기 컷이 버리는 것을
디코드 없이 박스로 남긴다. 화면 2 px 이하인 페이지·페이지 BVH 노드·배치 BVH 노드·배치
footprint는 박스 하나(노드 박스의 레이어는 아래 배치 최대 16개의 레이어 마스크 합), 더 넓은
노드는 내려간다. 박스 rect는 플랜당 400만 개 상한. 합성 MAIN01 1/10(`gen_main01_like.py
--scale 0.1`, 1 GB, 449 레이어), 1920×1080, thin keep, detail high(컷 1 px), plan 시간 ms:

| 뷰 | depth 0 | full·전 레이어 | full·1 레이어 | full·8 레이어 |
|---|---|---|---|---|
| fit | 89 → 87 | 1,409 → 5,205 | 574 → 2,103 | 838 → 2,720 |
| ×2 | 76 → 78 | 3,200 → 5,781 | | |
| ×4 | 38 → 37 | 2,976 → 4,716 | 1,094 → 1,701 | 1,654 → 2,456 |
| ×8 | 22 → 21 | 833 → 1,357 | 368 → 519 | 520 → 750 |
| ×16 이상 | 변화 없음 | 변화 없음 | | |

- depth 0: 비용도 효과도 없다. 합성 칩의 top 셀 페이지는 크고(페이지당 4만 레코드) 컷을
  통과해 박스가 생기지 않는다. 실칩 MAIN01의 depth 0 박스는 이 합성 파일로 재현되지 않는다.
- full depth: 방문 BVH 노드 640만 → 2,120만, plan 3~4배. 전 레이어 fit은 박스 상한에 걸려
  2,220만 개가 버려졌다(상한이 없으면 더 느리다). 박스 수는 (셀 변형 수) × (레이어 수)로 는다.
- 프레임 전체(plan + raster): 1 레이어 fit 0.54 → 2.1 s, 8 레이어 fit 0.92 → 12.6 s, 전 레이어
  fit은 약 9분. 박스가 셀 로컬 좌표라 래스터가 인스턴스마다 다시 그리기 때문이다.
- 그림: 기준은 1·8 레이어 fit과 ×4가 완전히 빈 화면(켜진 픽셀 0)이고, 박스를 켜면 점유
  영역이 보인다(1 레이어 fit 14만 px, ×4 23만 px).
판정: 이 형태(셀 로컬·레이어별 박스)로는 다층 광역뷰에 쓸 수 없다. 한 레이어 비교용으로는
2초대. 남은 길은 박스 수를 화면 크기에 묶는 것이다 — 화면에서 몇 px 이하인 셀 인스턴스는
안으로 들어가지 않고 인스턴스 하나를 박스 하나로 그리는 방식(미구현).

#### 합성 MAIN01 기준 측정 (2026-09-18, 사용자 보고: thin keep, fit에서 5번 줌할 때까지 빈 화면)

사용자가 `tools/gen_main01_like.py`로 main01.oas를 만들어 기준 파일로 삼았다. 같은 생성기의
1/10 크기(1 GB, 449 레이어)로 재현·측정했다. 1920×1080, thin keep, full depth, 뷰 중심 고정,
fit 대비 배율. 0.12.164.

| 배율 | high·전 레이어 | high·큰 예산(16 GB) | high·1 레이어 | medium·전 레이어 |
|---|---|---|---|---|
| ×1 | 7.8 s, 34만 px | 7.8 s, 34만 px | 0.6 s, **0 px** | 0.04 s, **0 px** |
| ×2 | 5.4 s, **0 px** (컷 ×4) | 22 s, 101만 px | 1.3 s, **0 px** | 1.3 s, **0 px** |
| ×4 | 5.2 s, **0 px** (컷 ×8) | 18 s, 180만 px | 1.2 s, **0 px** | 2.8 s, **0 px** |
| ×8 | 1.2 s, **0 px** (컷 ×16) | 187 s, 207만 px | 0.5 s, 12만 px | 1.0 s, **0 px** |
| ×16 | 4.5 s, 207만 px | 5.4 s | | 0.3 s, 207만 px |

빈 화면의 원인은 둘이다.
- **예산 맞춤이 화면을 비운다**(전 레이어 ×2~×8): 요청대로의 플랜은 예산을 넘고(1.0~1.4 GB),
  컷을 올리면 이 파일의 도형이 한 크기대(top 0.5~50 µm, leaf 0.05~5 µm)라 통째로 빠진다. 맞는
  컷 중 가장 세밀한 것이 "아무것도 없음"이고, 패스를 여러 번 돌아 plan만 5 s다. 실칩 기록의
  "한 크기대가 한 번에 빠질 위험"이 그대로 나타난 경우다.
- **크기 컷**(1 레이어 ×1~×4, medium ×1~×8): 모든 도형이 양변 모두 컷 미만이라 페이지가 하나도
  선택되지 않는다. 그런데도 plan은 0.6~2.7 s다 — 컷보다 큰 셀 변형 5천~1만 개를 펼쳐 BVH 노드
  640만~1,100만 개를 걷고 아무것도 고르지 못한다. 이 파일은 도형이 정사각형에 가까워 keep이
  살릴 긴 선도 없다(실칩은 긴 배선이 남는다).

예산이 무한이어도 프레임은 18~187 s다. 래스터가 지배한다: ×1은 켜진 픽셀 34만에 멤버 paint
1.14억(픽셀당 332회), ×2는 101만 px에 3.03억(300회). 레이어 449개가 같은 면적을 차례로 덮는
**레이어 간 덧칠**이다(레이어당 픽셀당 약 0.75회). paint 하나에 약 55 ns.

기존 스위치(같은 사다리, 전 레이어): `FLOE_RUST_SUB_CUT_WASH=on`은 빈 화면 대신 셀 전체를 한
색으로 덮는 wash가 나오고 plan 6.5 s; `FLOE_RUST_PAGE_REPS=on`은 점 무늬가 나오지만 plan
28~38 s, ×4·×8은 예산 초과 오류. 박스 실험(위)은 1 레이어 2.1 s, 전 레이어 9분. 박스 문턱을
2 → 4 → 8 → 16 px로 올리면 1 레이어 fit plan이 2.3 → 1.9 → 1.4 → 1.05 s(기준 0.63 s).

개선 방향(검토, 미구현, 우선순위 순):
1. **래스터 덧칠 제거(정확, 그림 불변)** — 구현됨, F2R-28: 레이어를 위에서 아래로 그리되 이미 쓴 픽셀은 다시 쓰지
   않고(불투명 덮어쓰기라 결과 동일), 기기 bbox가 이미 다 쓰인 항목·배열·셀 인스턴스는 건너뛴다.
   픽셀당 300회 → 수 회. 추정: ×1의 draw 5.4 s → 1 s 미만. 2·3의 전제다.
2. **예산 맞춤은 크기대가 아니라 밀도를 줄여야 한다** — 구현됨(0.12.166, 아래 2026-09-19 항목): 컷을 올려 플랜이 비거나 급감하면 컷은
   요청대로 두고 페이지를 2^k개 중 하나로 솎는다(결정적, 레이어마다 다른 위상). 빈 화면 대신
   성긴 그림. 1 없이는 그 그림도 십수 초다.
3. **사라지지 않는 박스는 인스턴스 단위로**: 화면 4~8 px 이하 서브트리는 들어가지 않고 박스 하나,
   박스는 레이어별 rect가 아니라 (레이어 마스크, bbox) 하나로 낸다. plan +0.8~1.3 s(1 레이어 fit).
   다층에서는 1이 있어야 래스터가 감당한다.
4. **인덱스에 (셀, 레이어)별 재귀 최대 도형 크기**: 컷보다 작은 도형만 있는 서브트리를 걷지 않고
   끝낸다(정확). 빈 결과에 쓰는 plan 0.6~3 s가 없어지고, 3은 "전부 컷 미만"인 셀을 내려가지 않고
   박스로 낼 수 있다. 인덱스 버전이 올라 재인덱싱이 필요하다.
5. occupancy·OVR(인덱스 시 생성)은 보류 그대로. 화면 크기에 묶인 비용을 주는 유일한 방식이지만
   생성 시간·용량이 문제였다.

문제: 뷰당 decoded 상한 판정이 라운드 decode **뒤**에 이루어져
(제품 기본은 단일 라운드라 뷰 전체 decode 뒤) 낭비와 순간 메모리
증가를 막지 못한다.

계획: 페이지 메타(encoded 크기, 레코드·멤버 수)로 decoded 크기를
보수적으로 추정해 decode 전에 합산·판정한다. 추정이 실측보다 작을 수
있으므로 decode 후 검사는 안전망으로 유지한다.

수용 gate: 합성 입력에서 상한 초과 뷰가 decode 없이 즉시 오류, 정상
뷰의 결과 byte 동일, 추정치/실측치 비율을 frame line에 기록.

#### 예산에 맞춘 밀도 (2026-09-19, 0.12.166 — 위 방향 2)

사용자 결정("2번 진행"). 예산을 넘는 플랜은 컷을 올리는 대신 요청 컷에서 밀도를 낮춘다
(SPEC-PLANNER §3 "예산에 맞춘 밀도"): (seq + 셀 + 레이어) mod 2^k로 페이지를 솎고, 남은
예산으로 큰 페이지부터 완전하게 채운다. 8배 넘게 초과하는 패스만 컷을 두 배로 올리고, 그
결과가 예산의 절반 미만이면(등급을 건너뜀) 한 단 아래를 끝까지 계획해 솎는다. 킬 스위치
`FLOE_RUST_FIT_THIN=off`(컷 사다리), `FLOE_RUST_FIT_BUDGET=off`(종전 오류).

합성 MAIN01 1/10, 1920×1080, thin keep, 전 레이어, full depth(프레임 시간 / 켜진 픽셀):

| 배율 | 사다리 high | 밀도 high | 밀도 medium | 예산 16 GB high |
|---|---|---|---|---|
| ×2 | 4.6 s / **0** | 14.8 s / 101만 | 3.2 s / 101만 | 22 s / 101만 |
| ×4 | 5.3 s / **0** | 11.9 s / 180만 | 7.9 s / 180만 | 18 s / 180만 |
| ×8 | 1.1 s / **0** | 11.0 s / 207만 | 1.8 s / 207만 | 187 s(write-once 전) / 207만 |

켜진 픽셀 수는 16 GB 프레임과 같고, 다른 픽셀은 ×2 129 px, ×4 37,206 px, ×8 26,782 px
(medium ×8은 0). 모두 `fit_thin` 1(완전 계층 아래 1/2). plan은 1패스(4.4~4.7 s; 사다리는
빈 결과에 4.6~5.3 s). 빈 화면 대신 그림이 나오는 만큼 프레임은 길어진다 — 이 뷰들의 실제
비용이며 draw의 대부분은 F2R-28에 적은 단일 스레드 단계다.
시행착오: ① seq만으로 솎으면 페이지가 하나뿐인 run이 어느 k에서도 남아 12단계 모두 실패하고
"큰 페이지만"으로 떨어졌다(합성 칩: 셀마다 레이어당 1페이지) → 위상을 셀로, ② 그래도 top
셀의 41개 레이어(각 1페이지, 74 MB)가 같은 위상이라 48 MB 예산에 못 맞춤 → 레이어도 위상에.
③ 8배 상한으로 컷을 두 배씩 올리면 12 MB 예산에서 다시 0페이지(등급을 건너뜀) → 절반 미만
규칙. 남은 것: medium의 fit과 1 레이어 뷰의 빈 화면은 크기 컷 문제다(방향 3·4). 실칩에서 페이지
단위로 비는 그림이 받아들일 만한지는 미확인.

#### 합성 MAIN01 생성기 보강 (2026-09-19, `--geometry chip` 기본)

사용자: 실칩 확인이 당분간 어렵다 → 합성 파일을 실칩에 가깝게. 개수·깊이·바이트 목표는
그대로 두고 **도형의 모양**만 바꿨다(`tools/gen_main01_like.py`; 이전 출력은 `--geometry
legacy`로 바이트까지 동일 — 2026-09-19 이전의 모든 측정은 legacy 파일에서 한 것이다).
- 레이어마다 역할(번호 mod 6): 박스, 가로 배선, via, 세로 배선, fill, 블록.
- 배선은 가늘고 길다: 레이어당 폭 몇 가지, 길이는 폭의 몇 배부터 셀 크기까지 로그 균등,
  20개 중 하나는 셀을 가로지르는 rail, 트랙을 따라 이어 놓는다. 폭은 계층에 따라 커진다
  (라이브러리 셀 0.02~0.08 µm, 블록 0.05~0.4 µm, top 0.4~10 µm) — 한 뷰에 크기대가 여럿.
- grid는 버스(긴 선의 반복), 짧고 가는 도형의 밭(0.45 × 0.022 µm, 0.8 × 1.0 µm 간격 같은),
  via·fill 배열. 다각형은 배선 크기의 L·계단. 배열 배치 50개 중 하나는 맞붙은 큰 배열.
- **라이브러리 셀 크기를 배치 수에서 계산**한다(부모를 약 한 번 덮게: 0.25~3 µm). legacy의
  leaf는 100 µm짜리가 top에만 수천만 번 놓여 부모를 수백 번 덮었다. 셀 안에 그릴 것이 없을
  때는 드러나지 않았지만, 배선을 넣자 fit 프레임 하나가 9분(bin 초과 → 타일·레이어별 walk).
게이트 `gen_main01`(tools/validate_gen_main01.py): legacy sha256 고정, chip은 `--jobs` 무관
결정적, KLayout이 3,521셀·449레이어로 읽고 floe-index가 인덱싱, top 셀에 가늘고 긴 도형
수만 개·크기 자릿수 4개 이상·라이브러리 셀은 다이의 1/500 미만. `fit_budget`·`write_once`
게이트는 장면을 고정하려고 `--geometry legacy`를 쓴다.

새 파일 1/10(909 MiB, 인덱스 56 s)의 기준 측정(0.12.166, 1920×1080, full depth, 프레임 s):

| 배율 | 전 레이어 keep high | 전 레이어 keep medium | via 1 레이어 keep high |
|---|---|---|---|
| fit | 1.65 (밀도 1/2) | 1.30 | 0.05, **0 px** |
| ×2 | 0.74 (1/2) | 0.57 (1/2) | 0.03, **0 px** |
| ×4 | 2.09 (1/2) | 0.33 (1/2) | 0.03, **0 px** |
| ×8 | 1.22 (1/2) | 0.26 (1/2) | 0.03, 12 px |
| ×16 | 0.39 | 0.18 | 0.18, 15만 px |

legacy 파일과 달라진 결론: ① 전 레이어 프레임이 0.1~2 s로, 사용자가 실칩에서 본 범위다.
② **방향 4(인덱스에 재귀 최대 도형 크기)는 필요가 없어졌다.** 빈 결과의 plan 0.6~3 s는
legacy의 100 µm leaf가 컷을 넘어 모두 펼쳐지던 탓이었고, 새 파일에서는 BVH의 셀 크기 주석
(v7)이 이미 걸러 16~37 ms다. ③ 방향 3(사라지지 않는 박스)의 대상은 그대로 남는다 — via
레이어 하나만 켜면 fit부터 ×4까지 빈 화면이다.

#### 방향 3: sub-cut 박스 (2026-09-19, 0.12.168) / 방향 4: 하지 않음

사용자 결정("생성기 보강 후 3번, 4번"). **방향 4는 측정 결과 필요가 없어 구현하지 않았다**
(위 생성기 항목 ②: 빈 결과의 plan 0.6~3 s는 legacy 합성 파일의 100 µm leaf 탓이었고, 칩 형태
파일에서는 v7 BVH 크기 주석이 이미 걸러 16~37 ms다). 남는 쓰임은 "배치 BVH 노드별 레이어
마스크"인데 — 박스의 걷기가 레이어와 무관하게 전 계층을 도는 것을 줄여 준다 — 인덱스 형식
변경(전체 재인덱싱)이라 아래 비용이 문제가 될 때로 미룬다.

방향 3(SPEC-PLANNER §3 "sub-cut 박스"): `thin keep`이고 보이는 레이어가 4개(0.12.171부터 16개) 이하이면 크기
컷이 버리는 것을 인덱스 메타만으로 4 px 이하 박스로 남긴다. 킬 스위치
`FLOE_RUST_SUB_CUT_BOX=off`. 칩 형태 합성 MAIN01 1/10, 1920×1080, keep high, full depth
(프레임 s / 켜진 픽셀; 끔 → 켬):

| 배율 | via 1 레이어 | 4 레이어 |
|---|---|---|
| fit | 0.04 / **0** → 0.40 / 71만 | 0.05 / 1.4만 → 0.52 / 85만 |
| ×2 | 0.02 / **0** → 0.32 / 123만 | 0.04 / 4만 → 0.49 / 160만 |
| ×4 | 0.03 / **0** → 0.38 / 112만 | 3.07 / 103만 → 1.07 / 183만 |
| ×8 | 0.03 / 12 → 0.43 / 84만 | 0.11 / 52만 → 0.57 / 123만 |
| ×16 | 0.14 / 15만 → 0.27 / 34만 | 0.05 / 13만 → 0.58 / 56만 |

비용은 plan 0.2~0.35 s(컷이 건너뛰던 BVH를 4 px까지 내려간다: 방문 노드 14만 → 171만)와
박스 paint다. 시행착오: ① 박스마다 레이어 벡터·비트셋 복사를 할당 → plan 1.3 s, 보이는
레이어 목록을 한 번 만들어 0.47 s. ② 박스 하나가 bin 항목 하나 → 항목 상한(76.8만) 초과로
타일·plane별 walk, 4 레이어 fit 1.75 s(draw 1.39 s) → wash를 128개 chunk 항목으로 묶어
0.52 s(draw 0.14 s). 이 묶음은 박스와 무관한 프레임도 살렸다(4 레이어 ×4: 3.07 → 1.07 s).
③ 8 레이어 ×8은 박스 없이도 화면이 다 차는데 0.1 → 1.1 s가 돼 상한을 4 레이어로 내렸다.
④ 넓은 배열(작은 셀의 메모리 배열)은 footprint가 박스보다 커서 통째로 빠졌다 → 밀집 축은
한 줄로, 성긴 축은 멤버별 박스로. 한계: 노드 박스의 레이어는 표본(최대 8배치)이라 드문
레이어가 빠질 수 있다; 유한 depth에서 자식의 재귀 레이어 마스크는 depth 제한을 모른다;
실칩 미확인.

#### 리뷰 수정 4건 (2026-09-19, 0.12.169): 밀도 맞춤의 포함 관계, sub-cut 박스의 정확성

리뷰(사용자 전달): 덧칠 제거는 유지. 밀도 맞춤(0.12.166)과 sub-cut 박스(0.12.168)에서 문제
4개를 재현. 모두 재현해 고쳤다(SPEC-PLANNER §3 두 절을 새 규칙으로 다시 썼다).

| # | 문제 | 재현(수정 전 → 후) | 수정 |
|---|---|---|---|
| 1 P1 | 제한 depth보다 깊은 도형의 박스 | depth 1, 도형은 depth 2: 박스 100개 → 0 | 레이어를 남은 depth 안에서 확인(`cell_bits`) |
| 2 P1 | 큰 배열이 거대한 채움 | 0.5 px·3 px 간격 30×30: 4,136 px → 900 px(멤버와 동일) | 멤버별 박스, 정말 맞닿는 축만 잇기 |
| 3 P1 | 8개 표본에 없는 레이어 소실 | 64배치 중 하나의 레이어, 클러스터 8개: 박스 0 → 8 | 노드 아래 모든 배치 확인(조기 종료) |
| 4 P2 | 뷰를 좁히면 페이지가 빠짐 | {0,1,8} → {0,4,8} | 고정 우선순위의 엄격한 접두사 |

3번을 재현하다 찾은 같은 종류의 결함: 인덱서가 같은 셀의 반복 배치를 점 목록(Pts)으로
합치는데, 박스보다 넓은 점 목록은 통째로 버리고 있었다(32개 중 하나인 셀이 3곳에 있으면 0 px).
점마다 박스를 그리게 했다(3 px = 실제와 동일). 상한(200만)을 넘으면 나중에 걷는 셀만 비던 것도
"한 단계 거칠게 다시 계획"으로 바꿨다.
4번의 대가: 0.12.166은 모든 크기 등급에 표본을 남겼지만 새 규칙은 접두사가 끝난 등급 아래를
남기지 않는다(확대 시 새로 들어오는 작은 등급이 기존 페이지를 밀어내지 않게 하려면 등급이
1순위여야 한다). 1~3번의 대가: 정확한 레이어 확인과 멤버별 박스로 프레임이 0.2~0.7 s 늘었다
(via ×2 0.32 → 0.57 s, 4 레이어 fit 0.52 → 1.18 s). 제 실수 기록: 작업 중 `git checkout`으로
미커밋 hier.rs를 되돌렸다가 저장해 둔 편집 스크립트로 복구했다 — 이후 체크포인트 사본을 둔다.

#### 인덱스 v8: 배치 BVH 노드의 레이어 마스크 (2026-09-19, 0.12.170)

사용자 결정("인덱스에 BVH 노드별 레이어 마스크 넣기도 진행, 재생성은 문제 없음"). 미뤄 두었던
방향 4의 남은 쓰임이다. ovm v8: 노드마다 `lmask_rec`·`lmask_direct` 합집합(SPEC-FORMATS).
- 플래너: 노드 박스가 full depth와 depth 경계 한 단 위에서 읽기 없이 답한다. 보이는 레이어가
  없는 배치 서브트리는 노드째 건너뛴다(`culled_bvh_layer`).
- 인덱서: 모든 셀 커밋 뒤 셀 단위 병렬 패스. 시행착오: 모든 노드에 기록하니 합성 1/10에서
  비트셋 1,800만 개(노드 합집합이 거의 다 다름), ovm +1.2 GB, 패스 8.1 s → 아래 배치 64개 이상인
  노드만 기록: 190만 노드, 비트셋 84만 개, ovm +0.2 GB(6.5 → 6.7 GiB), 패스 2.0 s, 전체
  인덱싱 시간은 잡음 범위. 작은 노드는 배치를 읽는다(최대 63개).
- 칩 형태 합성 MAIN01 1/10, keep high(0.12.169 → 0.12.170, 프레임 s): via 1 레이어 fit 0.6 →
  0.22, ×2 0.57 → 0.22, ×4 0.61 → 0.33; 4 레이어 fit 1.18 → 0.41, ×2 0.94 → 0.46, ×4 1.62 →
  1.12. plan(via fit) 545 → 170 ms, 읽기 2,900만 → 290만. 박스 수·켜진 픽셀은 동일.
- 이전(v7) 캐시는 열리지 않는다: `floe2 index <src> --force`. 실칩의 비트셋 수·크기 증가는
  미확인(합성 파일의 레이어 집합은 무작위라 실칩보다 나쁜 쪽일 것으로 본다).

#### sub-cut 박스의 레이어 상한 4 → 16, 박스당 rect 하나 (2026-09-19, 0.12.171)

사용자: "레이어 상한을 올리거나 없애서 확인". 박스가 레이어 수만큼 곱해지는 것이 비용의 원인이라
(32 레이어에서 rect 190만, 플랜 상한) 박스를 **가장 위 레이어의 rect 하나**로 바꾸고(SPEC-PLANNER
§3), 상한 없이 449 레이어까지 쟀다. 결론: 16개까지는 그림이 크게 좋아지고(fit 18만 → 90만 px,
0.04 → 0.36 s), 32개부터는 화면이 이미 차 있으며, 128개 이상은 모든 픽셀이 켜진 프레임에 1.3~
3.3 s를 더 쓴다. 상한을 16으로 올리고 없애지는 않았다. 표는 SPEC-PLANNER §3.
측정 중 제 스크립트 오류: 켜진 픽셀을 "왼쪽 위 픽셀과 다른 색"으로 세어, 칩이 화면을 덮는 ×2
이상에서 값이 틀렸다(끔/켬이 같은 수로 나옴) → 검정 배경 기준으로 고쳐 다시 쟀다. 시간 값은
처음부터 유효했다.

#### 리뷰 수정 2건 (2026-09-20, 0.12.172): 박스가 아래 레이어를 지움, frames 끔이 플래너에 닿지 않음

리뷰(사용자 전달, 0.12.171 기준): 노드 레이어 마스크의 합성은 검증 통과. 실제 사용 경로의 문제
2개를 재현해 고쳤다.

| # | 문제 | 재현(수정 전 → 후) | 수정 |
|---|---|---|---|
| 1 P1 | "가장 위 레이어 rect 하나"가 채움이 다른 아래 레이어를 지움 | 아래 레이어만 11,700 px, 위에 채움 없는 레이어를 켜면 600 px → 11,700 px | 박스는 대표하는 레이어마다 rect(0.12.168 방식으로 복귀) |
| 2 P2 | 뷰어의 frames 끔이 노드 프루닝에 연결되지 않음 | depth 0, frames 끔인데 프레임 64개 계획 → 0개 | `ViewReq::frames` / `PlanRequest::frames`, renderd가 `command.frames`를 넘김 |

1번은 제 전제가 틀렸다: "아래 레이어의 박스는 덮어쓰인다"는 채움이 같은 픽셀을 켤 때만 맞고,
주석의 "몇 픽셀짜리 박스"도 맞닿은 배열을 이은 150 px 박스에는 해당하지 않았다. 어떤 레이어가
가려지는지는 스타일을 아는 래스터가 정하고, write-once 래스터가 이미 가려진 rect를 건너뛴다.
같은 부하에서 잰 비용은 4 레이어 동일, 16 레이어 +0~15 %. 상한 16과 "없애도 이득 없음" 결론은
그대로다(32개 이상은 rect가 더 많아져 더 나쁘다).
2번은 단위 테스트가 `frame_cap: 0`을 직접 넣어 연결 누락을 못 잡았다 → 테스트를 요청
(`frames: false`)과 기본 옵션으로 바꿨고, 게이트 `sub_cut_box`에 두 재현 레이아웃을 넣었다
(같은 워커에서 frames 끔/켬/끔 → 계획된 프레임 0/64/0). 칩 형태 합성 1/10, thin cull, detail
medium에서 frames 끔의 plan 시간 이득은 작다(depth 5 ×8에서 8 → 2 ms, 나머지는 1 ms 이내): 이
파일은 depth 0~5의 plan이 이미 20 ms 이하다. 계획되는 프레임은 16~1,051개 → 0개.

#### F2R-28 확인 (2026-09-20, 사용자 질문): 컷 이상의 원본 도형도 위 레이어부터 그리고, 선은 어떻게 되나

맞다 — 스타일이 있는 단일 레이아웃 프레임 전부(keep·cull·exact, 컷 이상의 원본 도형 포함)가 위
레이어부터 그리고 한 번 쓴 픽셀은 다시 쓰지 않는다. "썼다"는 것은 그 레이어의 스타일이 **실제로 켠
픽셀**만이다: solid 채움은 내부 전체, speckle은 절반, clear는 외곽선만. 그래서 아래 레이어의 선은 위
레이어가 켜지 않은 픽셀에 그대로 찍히고, 위 레이어의 solid 채움·외곽선 아래에서는 가려진다 — 아래부터
덧칠하던 종전 결과(그리고 KLayout의 레이어 순서)와 같다. 실제 렌더(100 µm 사각형 두 개가 겹침, 위 =
2/0, 아래 = 1/0, keep, 컷 3 px; 겹친 내부 25,600 px과 위 사각형 안을 지나는 아래 사각형의 변 140 px):

| 위 채움 | 아래 채움 | 겹친 내부: 아래 / 위 / 빈 픽셀 | 아래 사각형의 변(140 px) 중 보이는 것 | write-once = 순서대로 덧칠 |
|---|---|---|---|---|
| solid | solid·clear | 0 / 25,600 / 0 | 0 | 바이트 동일 |
| speckle | solid | 12,800 / 12,800 / 0 | 140(선 70 + 채움 70) | 바이트 동일 |
| speckle | clear | 0 / 12,800 / 12,800 | 70 | 바이트 동일 |
| clear | solid | 25,600 / 0 / 0 | 140 | 바이트 동일 |
| clear | clear | 0 / 0 / 25,600 | 140 | 바이트 동일 |

항목 건너뛰기(`world_box_written`)는 도형의 box에 외곽선 폭 + 2 px를 더한 범위가 **전부** 쓰였을 때만
일어나므로 선이 그 때문에 빠지지는 않는다. "아래 레이어의 선을 위 레이어의 solid 채움 위에도 그린다"는
다른 표시 정책(KLayout 순서와 갈라짐)이며 지금은 아니다.

#### F2R-28 계측 (2026-09-20, 사용자 질문): 가려져 그릴 필요가 없는 도형이 페이지 단위로 빠지는가

**래스터에서는 빠지고, 계획·디코드에서는 빠지지 않는다.** 래스터는 ① 찬 타일의 남은 pass 전부,
② 열린 픽셀의 bbox 밖의 것, ③ 셀 인스턴스·페이지의 box(+ 외곽선 폭 + 2 px)가 전부 쓰인 것을
레코드를 보지 않고 건너뛴다(`once_passes_skipped`·`once_items_skipped`). 그러나 순서가 계획 → 디코드 →
래스터라서 가려질 페이지도 계획에 들어가 **디코드된 뒤에** 건너뛰어진다. 계측(진단용 scratch 패치,
트리에 없음; 합성 칩 1/10, 1920×1080, keep, medium): 디코드한 페이지 중 레코드 걷기에 한 번이라도
도달한 것 / 한 픽셀이라도 쓴 것.

| 전체 449 레이어 | fit | ×2 | ×4 | ×8 | ×16 |
|---|---|---|---|---|---|
| 디코드한 페이지 | 1,579 | 1,754 | 2,218 | 5,324 | 184 |
| 걷기에 도달 | 386 (24 %) | 195 (11 %) | 129 (6 %) | 55 (1 %) | 12 (7 %) |
| 픽셀을 씀 | 131 (8 %) | 99 (6 %) | 63 (3 %) | 34 (0.6 %) | 7 (4 %) |
| cold 프레임: 디코드 / 그리기 ms | 161 / 177 | 234 / 323 | 242 / 260 | 319 / 21 | 95 / 3 |

레이어가 적으면 가림이 거의 없다(앞 64개 레이어: 디코드 51~415, 도달 51~412, 씀 49~349). 즉 **모든
레이어를 켠 뷰(파일을 열었을 때의 기본)에서 디코드의 92~99 %가 한 픽셀도 못 쓰는 페이지**에 쓰인다 —
cold 프레임 시간의 절반 안팎이고, 예산(1024 MB)에 걸리는 크기에서는 더 나쁘다: 예산에 맞춘 밀도가
보이는 페이지를 솎는 동안 안 보이는 페이지가 예산을 차지한다.
없애려면 디코드 전에 가림을 알아야 한다. 후보: 점진 표시의 round를 **레이어 순서(위 먼저)**로 돌리고,
round가 끝날 때마다 타일 마스크(블록 완료 표시, F2R-29 0단계의 그것)를 보고 남은 페이지 중 어느
인스턴스도 열린 블록에 닿지 않는 것을 디코드 목록에서 뺀다 — 셀은 인스턴스가 공유하므로 판정은
타일 bin의 (셀 인스턴스, 변환)으로 한다(래스터의 건너뛰기 검사를 디코드 앞으로 옮기는 것). 미구현.

### F2R-29 — sub-cut 박스의 레이어 상한을 2단계 프레임으로 대체 (`DESIGN`, 코드 없음)

2026-09-20. 배경: 박스는 보이는 레이어가 16개 이하일 때만 켜지는 on/off다(빈 곳이 남아도 17번째
레이어에서 한꺼번에 사라진다). 사용자 요구: ① 위 레이어의 박스(나중에는 다른 모양일 수 있다)는
레이어 순서대로 위에 그려진다, ② speckle이 비워 둔 픽셀은 아래 레이어의 선·solid가 채운다(위상은
그대로). 둘 다 지금 구현의 동작이다(실제 렌더로 확인: 위 speckle 아래의 solid 블록 20,000 px,
0.3 px 선 300 px이 구멍에 찍힘; 덧칠 제거를 꺼도 같은 수). 사용자 제안: 픽셀 마스크로 채워진 곳을
보고 빈 곳에만 그리면 상한이 필요 없다. 아래는 그 분석이고, 리뷰(2026-09-20)의 조건 4개를 반영했다.

**왜 "다 그린 뒤 빈 곳에만"은 안 되고 "그 레이어 차례에 빈 곳에만"은 되는가.** 위에서 아래로 그릴
때 레이어 L의 차례에 마스크에 쓰여 있는 것은 L보다 위 레이어가 쓴 픽셀뿐이다. 그때 열린 곳에만
L의 박스를 그리면 요구 ①②를 지키며 지금 그림과 같다 — 래스터는 이미 그렇게 한다. 상한이 필요한
이유는 비용이 래스터 앞에 있어서다: 박스는 플래너가 픽셀이 하나도 없는 시점에 전부 만든다.

**B안(먼저 시도).** ① 박스 없이 그린다(1차). 블록마다 **완료 시점** T(b) = 그 블록에 열린 픽셀이
없어진 pass의 번호를 기록한다(끝까지 안 차면 ∞). pass 번호는 실제 paint 순서다: 프레임 밴드 0,
plane 위→아래, 프레임 밴드 1·3·2. ② 박스를 계획할 때 쓸모없는 것을 생략한다(아래 규칙). ③ 박스가
생긴 타일만 **원래 순서로 처음부터** 다시 그린다. 박스 없는 타일은 1차 픽셀 그대로. 픽셀마다 레이어
번호를 저장하지 않고 블록별 T(b)만으로 안전하게 생략할 수 있는 것은 ③ 덕분이다.
A안(래스터가 타일·레이어마다 필요할 때 인덱스를 걸어 박스를 만든다)은 래스터가 인덱스를 알아야
해서 구조 변경이 크다. B의 측정 결과가 부족할 때의 다음 단계로 둔다.

**생략 규칙(정확성).** 레이어 ρ의 plane이 그려지는 시각을 t(ρ)라 하면, 블록 집합 B에만 영향을 주는
레이어 ρ의 박스는 모든 b ∈ B에서 t(ρ) > T(b)일 때 아무 픽셀도 못 바꾼다. 후보 레이어 집합 S를 가진
노드·배치는 **가장 위 후보 레이어**만 보면 된다: t(top(S)) > max_b T(b)이면 통째로 생략. 통째로는
아니어도 t(ρ) > max_b T(b)인 레이어는 그 박스의 집합에서 뺀다(rect × 레이어 수가 준다).
- B는 그 노드가 **영향을 줄 수 있는 모든 블록**이다: bbox에 외곽선 폭 + hairline 보정 여유
  (래스터의 `world_box_written`과 같은 stroke + 2 px)를 더한 범위. 하나만 찼다고 버리지 않는다.
- 순서는 레이어 번호가 아니라 **실제 paint 순서**다. 지금 플래너는 (layer, datatype) 오름차순을
  가정한다(Python 어댑터가 그렇게 정렬해 스타일을 보낸다 — renderd의 계약은 아니다). B에서는
  renderd가 스타일의 순서를 요청에 실어 보내야 한다.
- 1차와 2차는 **같은 일반 도형**이어야 한다: 같은 요청(뷰·컷·depth·thin·예산 맞춤 결과)·같은
  스타일 epoch. 스타일이나 요청이 바뀌면 T(b)를 버린다. 박스는 페이지 선택을 바꾸지 않는다
  (단위 테스트가 확인: 디코드·확장 동일).

**셀 공유(가장 주의할 곳).** 같은 (셀, 남은 depth)의 모든 배치는 작업 셀 하나를 공유하고
(`contribute`: 배치마다 로컬 뷰 박스를 더해 `k_boxes` = 4개로 합친다), 셀 안의 걷기는 인스턴스의
화면 위치를 모른다. 한 인스턴스는 가려지고 다른 인스턴스는 보일 수 있으므로 **"가려짐"을 셀·BVH
노드 번호로 메모하면 안 된다.** 방안:
1. 판정을 노드가 아니라 **기여 단계**로 올린다. 부모가 자식 배치를 처리할 때는 그 배치의 변환과
   화면 footprint를 안다 → 그 footprint의 블록에서 max T(b)를 구해, 자식의 후보 레이어가 전부
   그 뒤에 그려지면 그 인스턴스는 박스용 로컬 뷰에 **기여하지 않는다**. 작업 셀은 일반 도형용
   로컬 뷰와 별도로 "박스용 로컬 뷰"와 "아직 의미 있는 가장 늦은 시각"(기여한 인스턴스들의 max)을
   갖는다. 모든 사용처가 가려졌을 때만 셀의 박스 걷기가 통째로 빠진다 — 보수적이고, 과포함은
   비용일 뿐 누락이 아니다.
2. **인스턴스가 하나뿐인 작업 셀**(top 셀, 한 번만 놓인 블록)은 변환이 하나이므로 노드 bbox를
   화면 블록에 직접 대 볼 수 있다. MAIN01은 placement가 depth 0~5에 몰려 있어 박스 걷기의 큰 몫이
   이런 셀일 가능성이 높다(미확인).
3. 공유 셀에서 박스를 계획하는 비용은 인스턴스 수가 아니라 **정의당 한 번**이다. 가려진
   인스턴스에서 낭비되는 것은 래스터의 컬링뿐이다.
로컬 뷰 박스가 4개로 합쳐지므로 열린 블록이 흩어져 있으면 박스용 로컬 뷰가 거의 전체로 부푼다.
박스용에 한해 k를 키우거나 2번의 직접 판정을 쓰는 것으로 보완한다.

**B가 없애지 못하는 것.** 1차에는 박스가 없으므로 작은 도형만 있는 장면은 완료된 블록이 거의 없다.
2차가 박스를 많이 만든 뒤 래스터에서야 서로 겹친다는 것을 안다(박스끼리의 중복). v8 노드 레이어
마스크도 그 레이어가 없는 노드는 잘 걸러내지만 거의 모든 노드에 있으면 효과가 작다. 따라서 상한을
없애도 **예산은 남긴다**: 박스 rect 200만 + 거칠기 단계, 배열당 멤버 262,144, 레이어 확인 읽기
6,400만, (추가) 박스 걷기의 방문 노드 예산. 이 최악의 경우(레이어 많음 + 전부 작은 도형)는 오늘은
아무것도 안 나오는 경우이고, B에서는 예산 안의 거친 박스가 나온다.

**아직 가설인 것(측정 전에는 주장하지 않는다).**
- "꽉 찬 다층 뷰에서는 추가 비용이 거의 없다" — 화면이 100 % 켜졌다는 것만으로는 알 수 없다.
  **어느 레이어에서 찼는지**가 결정한다: 블록이 L10에서 찼으면 L10보다 위의 박스는 여전히 필요하고,
  맨 아래 레이어에서야 찼다면 후보가 거의 다 살아남는다. 어제 측정(128·449 레이어에서 모든
  픽셀이 켜짐)은 최종 상태만 본 것이다.
- "비용이 열린 면적에 비례한다"(A안) — 레이어별 걷기가 v8 마스크로 얼마나 걸러지는지에 달렸다.
- speckle: 블록이 "찼다"고 되려면 아래 레이어의 외곽선·가는 선·solid가 체커보드의 구멍을 메워야
  한다. 중간 밀도 뷰에서 완료되는 블록이 얼마나 되는지 모른다.

**진행 순서(각 단계에 킬 스위치, 코드는 결정 후).**
0. **계측만** 먼저: 지금 프레임에서 블록별 완료 시점을 기록해 분포를 본다(박스 없음/있음). 같이
   "박스가 새로 쓴 픽셀 ÷ 박스가 켜려 한 픽셀"을 레이어 수별로 센다 — B가 줄일 수 있는 일의
   상한이다. 이 수치가 나쁘면(완료가 대부분 맨 아래에서 일어남) B를 만들지 않는다.
1. 2단계 프레임 + 인스턴스가 하나뿐인 셀의 직접 판정.
2. 기여 단계 판정(공유 셀).
3. 상한 제거(예산은 유지), 17번째 레이어의 경계 소멸.

**검증 지표**(1·16·32·128·449 레이어, fit~×16, 합성 칩과 legacy 파일): 블록 완료 시점의 분포,
생략한 노드 수와 실제 만든 박스 수, 다시 그린 타일의 비율, 1차 렌더·추가 계획·2차 렌더 시간과
최대 메모리, 같은 도형 선택에서 **지금의 한 번에 그리는 합성과 픽셀 일치**(B의 결과는 "박스를
전부 계획해 한 번에 그린 프레임"과 같아야 한다 — 상한을 512로 푼 현재 경로가 기준이다).
주의: pan 재사용으로 복사한 타일은 완료 시점이 없다(보수적으로 ∞), 점진 표시(refine) 중간
라운드의 T(b)는 쓰지 않는다.

#### F2R-29 0단계 계측 결과 (2026-09-20, `MEASURED` — 합성 칩 1/10만, 실칩 미확인)

방법(진단용, 트리에 넣지 않음 — 패치는 세션 scratch에 보관): 타일의 write-once 마스크 옆에
**그림자 마스크**를 두고 박스(wash rect)가 아닌 것이 켠 픽셀만 받는다 = 같은 프레임을 박스 없이
그렸을 때의 마스크. 추적 중의 생략 판정은 전부 그림자를 읽는다. pass가 끝날 때마다 그림자에 열린
픽셀이 없는 블록에 완료 시각 T(b)를 찍는다. 박스 rect마다: 닿는 모든 블록(장치 box + stroke +
2 px)이 자기 pass보다 먼저 완료됐는가(= B의 생략 규칙, rect 단위), 켜려 한 픽셀 / 새로 쓴 픽셀 /
그중 그림자에 이미 있던 픽셀. 조건: 1920×1080, thin keep, detail medium(3 px), frames·labels 끔,
레이어 상한 512로 해제, 보이는 레이어는 449개에서 고르게 뽑음, fit~×16. 검산: 추적을 켠 프레임과
끈 프레임의 픽셀이 70개 뷰 전부 일치, "생략 가능" rect가 새로 쓴 픽셀은 전 구간 0.

| 레이어 | 완료 블록(8 px) | 완료 시점(paint 순서) | 생략 가능 rect 32 px / 8 px | 픽셀 기준 상한¹ | 아무것도 못 쓴 rect | 박스 픽셀: 새로 씀 / 도형 아래 / 다른 박스 아래 |
|---|---|---|---|---|---|---|
| 1 | 0 % | — | 0 / 0 % | 0 % | 23~83 % | 4~21 / 0 / 79~96 % |
| 4 | 0~15 % | 위 1/3 | 0~5 / 0~9 % | 0~16 % | 35~90 % | 4~19 / 0~19 / 77~84 % |
| 16 | 1~28 % | 위·중간 | 1~11 / 2~16 % | 6~19 % | 28~93 % | 2~28 / 16~28 / 44~77 % |
| 32 | 3~80 % | 위·중간 1/3 | 2~27 / 3~59 % | 49~88 % | 84~99 % | 0~4 / 53~99 / 1~46 % |
| 64 | 4~80 % | 위·중간 1/3 | 2~32 / 3~60 % | 57~86 % | 88~99 % | 0~3 / 61~99 / 0~39 % |
| 128·449 fit·×2 | 40 %·91 % | 전부 위 1/3 | 27~30 / 31~42 % | 35~54 % | 96~99 % | 0.1~0.3 / 39~43 / 57~61 % |
| 128·449 ×4~×16 | 100 % | 전부 위 1/3 | 박스 rect가 하나도 시도되지 않음 (계획된 박스 83만~200만 개) | | | |

¹ 켠 픽셀이 전부 그림자에 이미 있던 rect — 픽셀 단위로 판정할 때의 상한.

시간(같은 뷰, 박스 끔 → 켬, 프레임 전체 s): 1~16 레이어 0.01~0.18 → 0.02~1.15, 32·64 레이어
0.02~0.85 → 0.13~1.81, 128·449 레이어 0.06~0.77 → 0.26~1.64. 늘어난 시간은 대부분 계획이다.
타일은 프레임당 15개뿐이고 4~64 레이어에서는 레이아웃이 걸친 타일 거의 전부에 박스가 있다(fit 9/15,
×2 이상 13~15/15) → "박스가 생긴
타일만 다시 그린다"는 절약이 없고 1차 렌더가 그대로 추가 비용이 된다.

판정:
- 가설 "꽉 찬 다층 뷰는 맨 아래에서야 찬다"는 합성 칩에서는 **틀렸다**: 128·449 레이어는 전부
  paint 순서의 위 1/3에서 완료된다. 이 구간(×4 이상)에서 박스 계획 0.2~1.45 s는 전부 낭비이고 B가
  전부 없앤다. 다만 이 이득은 B 전체가 아니라 **"박스 없이 먼저 그리고, 열린 블록이 없으면 끝"**
  이라는 가장 단순한 형태로 얻어진다(1차 프레임이 곧 최종 프레임이라 추가 비용 0).
- 32·64 레이어: 8 px 블록으로 rect의 31~60 %(fit~×4), 3~6 %(×8~×16)를 생략. 줄어드는 계획
  0.2~0.5 s ≈ 추가되는 1차 렌더 0.13~0.85 s → 이득 없음. 32 px 블록은 너무 거칠다(2~27 %).
- 16 레이어 이하(지금 박스가 켜지는 구간): 생략 0~16 %. 낭비의 본체는 **박스끼리의 중복**이다 —
  박스가 켜려 한 픽셀의 44~96 %가 다른 박스 아래이고 rect의 28~93 %가 아무것도 못 쓴다. 리뷰
  조건 ④ 그대로이며 B로는 줄지 않는다.
- 결론: B의 1~3단계(2단계 프레임 + 기여 단계 판정)는 **만들지 않는다**. 상한을 없애는 비용은
  32~449 레이어의 덜 찬 뷰에서 프레임당 +0.1~1.3 s, 얻는 것은 켜진 픽셀 +1~21 %다.
  다음 후보: ① "박스 없이 먼저, 열린 블록이 남을 때만 박스"(점진 표시의 첫 장으로 쓸 수 있다),
  ② 박스끼리의 중복이 어디서 생기는지(같은 셀 안 / 인스턴스끼리 / 레이어끼리) 계측 — keep의 계획
  비용(16 레이어 이하 0.01~1.0 s)을 줄이는 쪽은 이것이다.

#### F2R-29 A안 — 래스터가 필요할 때 박스를 만든다 (리뷰 2026-09-20 반영, `DESIGN`, 코드 없음)

리뷰의 판단: A가 박스 생성 비용과 메모리를 직접 줄이므로 장기적으로 더 유망하다(B를 먼저 권한 것은
기존 구조에서 구현·검증이 쉬워서였다). 타일마다 **위 레이어부터: 실제 도형 → 남은 영역의 박스 조회
→ 만든 즉시 그림 → 다음 레이어**. 위 레이어의 박스가 아래 레이어의 도형보다 먼저 자리를 차지하므로
레이어 순서가 유지된다. B와 달리 **앞서 그린 박스가 만든 가림**까지 다음 탐색에 쓴다.

불변식(검증 기준): 가림으로 빼는 것은 "어떤 픽셀도 못 쓰는 박스"뿐이므로, 예산에 걸리지 않는 한
결과는 **"박스를 전부 계획해 한 번에 그린 프레임"(상한을 푼 지금 경로)과 픽셀이 같아야 한다.**
같은 레이어 안에서 도형과 박스의 순서가 바뀌어도 픽셀이 같은지는 확인할 것(같은 색·같은 규칙이라
같을 것으로 보지만 미확인).

**계측(합성 칩 1/10만; 0단계와 같은 조건, 8 px 블록, 박스까지 포함한 실제 마스크 기준, 지금의 재생
순서).** rect가 차례가 왔을 때: `못 씀` = 새 픽셀 0(A가 만들 필요가 없는 rect의 픽셀 단위 상한),
`자기` = 자기 box + stroke + 2 px가 이미 다 쓰임(지금 래스터의 item skip과 같은 싼 검사),
`8 px`/`32 px` = 그 rect를 둘러싼 8/32 px 정렬 영역이 다 쓰임(그 크기의 노드에서 **탐색을 멈출 수
있는** 비율의 대용치). 픽셀은 박스가 켜려 한 픽셀의 행방이다.

| 레이어 | 뷰 | 못 씀 | 자기 | 8 px | 32 px | 픽셀: 새로 / 도형 아래 / **위 레이어 박스 아래** / 같은 레이어 박스 아래 |
|---|---|---|---|---|---|---|
| 1 | fit~×8 | 80~83 % | 38~58 % | 27~48 % | 6~29 % | 4~7 / 0 / 0 / 94~96 % |
| 4 | fit~×8 | 69~90 % | 53~59 % | 43~52 % | 19~31 % | 4~11 / 5~19 / 33~38 / 42~47 % |
| 16 | fit~×4 | 75~93 % | 38~67 % | 29~58 % | 16~35 % | 2~4 / 19~26 / 55~63 / 14~16 % |
| 16 | ×8·×16 | 28~41 % | 7~12 % | 4~9 % | 2~6 % | 24~28 / 16~28 / 25~34 / 19~26 % |
| 32·64 | fit~×4 | 95~99 % | 70~90 % | 60~87 % | 41~77 % | 0~1 / 53~100 / 0~42 / 0~4 % |
| 32·64 | ×8·×16 | 84~90 % | 12~34 % | 7~20 % | 4~12 % | 1~4 / 88~97 / 2~6 / 0~1 % |
| 128·449 | fit·×2 | 96~99 % | 41~60 % | 38~47 % | 29~30 % | 0.1~0.3 / 39~43 / 54~60 / 1~2 % |
| 128·449 | ×4~×16 | 타일이 도형만으로 꽉 참 → 조회 자체가 없음 (지금은 박스 83만~200만 개를 계획) | | | | |
| 1·4 | ×16 | 23~35 % | 1~2 % | 0 % | 0 % | 19~21 / 0 / 0~45 / 36~79 % |

읽는 법: ① 리뷰의 핵심 주장(박스가 만든 가림)은 수치로 확인된다 — 4~16 레이어와 128·449 fit·×2에서
박스 픽셀의 25~63 %가 **위 레이어의 박스 아래**다(B의 1차 마스크에는 없는 정보). ② 1 레이어에서도
94~96 %가 같은 레이어의 다른 박스 아래다 — 마스크를 보는 쪽만 줄일 수 있다. ③ 그러나 **탐색을
멈출 수 있는 비율(8·32 px)은 못 쓰는 rect의 비율보다 훨씬 낮다**: 16 레이어 이하에서 32 px 노드
기준 6~35 %, 8 px 기준 27~58 %. 즉 A는 rect 출력과 메모리는 크게(70~99 %) 줄이지만 탐색 비용은
절반 안팎만 줄일 가능성이 높다 — "비용이 열린 면적에 비례한다"는 여전히 보장할 수 없다(리뷰 3과
일치). ④ 확대한 뷰(×8·×16)는 박스가 실제로 픽셀을 쓰는 구간이라 가지치기가 거의 없다(박스 수도 적다).
주의: rect 단위 대용치이고 실제 노드 크기 분포·인스턴스별 걷기 비용은 재지 않았다.

**리뷰의 세 조건과 코드에서 본 것.**
1. *타일 × 레이어마다 BVH 전체를 다시 걷지 않는다.* v8 노드 마스크로 레이어가 없는 가지를 거르고,
   조회 영역으로 공간 프루닝하고, **내려가기 전에** 노드의 화면 범위(+ stroke + 2 px)가 이미 다
   쓰였으면 멈춘다. 코드에서 추가로 걸리는 것: 지금 플래너는 공유 작업 셀을 **정의당 한 번** 걷고
   래스터가 인스턴스마다 목록을 재생한다(박스 15만 개 → rect 55만 개). 화면 공간에서 걷는 A는
   **인스턴스 × 타일 × 레이어마다** 걷는다. 보완안 A′: 인스턴스가 많은 셀은 "(셀, 남은 depth,
   레이어)의 박스 목록"을 **처음 필요해질 때 한 번** 만들어 공유하고(지금 목록의 lazy 판 — 어느
   인스턴스도 열린 픽셀을 못 만나면 끝까지 안 만든다), 인스턴스가 하나뿐인 셀(top, 큰 블록)만
   마스크를 보며 직접 걷는다. 타일마다 마스크가 따로이므로 타일에 걸친 인스턴스는 타일마다
   조회된다(영역으로 제한).
2. *목록을 다 만들어 돌려주는 함수면 효과가 준다.* 조회는 방문자 형태: `covered(local bbox) → bool`
   (래스터가 변환해 마스크에 대 본다 — 노드·배열 범위·박스 모두에 같은 질문)과 `emit(local bbox)`
   (즉시 그림, 마스크 갱신). 배열은 조회 영역과 격자의 교집합 범위의 멤버만 열거하고, 맞닿는
   멤버를 하나로 잇는 규칙(pitch ≤ max(멤버, 1 px))은 그대로 둔다.
3. *작업 예산은 남는다.* 출력이 적어도 탐색은 클 수 있다(빈 타일, 성긴 배열, speckle 구멍). 노드
   방문·멤버 검사·레이어 확인 읽기에 (타일, 레이어) 단위 예산을 둔다 — 프레임 공용 예산은 스레드
   순서에 따라 결과가 달라지므로 쓰지 않는다. 소진되면 버리지 않고 **그 노드를 박스 하나로** 그린다
   (더 거친 stand-in). 이 경우 결과가 타일 분할에 의존하므로 pan 재사용 타일과의 이음매가 생길 수
   있다 — 열린 문제.

**인터페이스.** 래스터에 인덱스 내부를 섞지 않는다. `floe-vfs`에 전용 조회(레이어, 남은 depth, 셀,
조회 영역(로컬), 컷 크기, 방문자)를 두고 인덱스 메타데이터(mmap, 읽기 전용, 스레드 공유 가능 —
render-core는 이미 `floe-ovm`·`floe-vfs`에 의존)를 공유한다. 가려짐 판정은 호출한 타일·인스턴스의
현재 마스크로만 한다. 계획은 박스 대신 "이 작업 셀에는 레이어 집합 S의 sub-cut 내용이 있다"만
남긴다(v8 마스크 + 지금의 `cell_bits`; 그 memo는 플래너 지역이라 계획 때 미리 채우거나 잠금이
필요). 지금 `washes`를 읽는 곳을 함께 옮겨야 한다: 씬의 레이어 마스크(박스만 있는 셀이 레이어
프루닝에 잘리면 안 된다), 점 query, 상태줄의 박스 수, 프레임 캐시·pan 재사용, (보류 중인) 덱 합성.

**진행 순서(결정 후, 단계마다 킬 스위치; 플래너의 박스 경로는 현장 확인 전까지 남긴다).**
1. vfs 조회 인터페이스 + 단위 테스트: 가림 없이 돌리면 플래너가 만드는 박스 집합과 같다(레이어별).
2. 래스터가 레이어 차례마다 조회해 즉시 그린다(인스턴스가 하나뿐인 셀 직접 걷기 + A′ lazy 목록).
   검증: 상한을 푼 지금 경로와 픽셀 일치(합성 칩, 리뷰 재현 레이아웃 6종), plan/draw 시간과 최대
   메모리, 방문 노드 수 vs 만든 박스 수.
3. 예산, 상한(16) 제거, 플래너 박스 경로의 기본값 전환.
"박스 없이 먼저, 열린 블록이 남을 때만 박스"(0단계의 후보 ①)는 A에 포함된다(꽉 찬 타일은 조회하지
않는다).

### F2R-30 — 픽셀보다 작은 도형의 표본화("성긴 곳은 성기게") (`PROPOSAL`, 코드 없음)

2026-09-20 사용자 관찰(합성 칩 1/10, depth 0, detail medium, 109/2만): 사각형 배열이 축소하면
어두워지다가 어느 시점부터 밝아져 결국 밝은 사각형 하나로 뭉친다. Calibre는 작아지면서 살아남는
것과 아닌 것이 있어 떨어져 있는 도형은 아무리 축소해도 성기게 남는다(빈 공간이 표현에 포함된다).

재현(같은 파일의 109/2 top 레벨 배열 26×20 = 520 멤버, 멤버 ≈ 0.4 µm, pitch ≈ 1.2 µm, 실제 fill
≈ 0.1; 1920×1080, keep = cull 동일). 배열의 화면 box 안에서 켜진 픽셀의 비율:

| view µm | 67 | 107 | 172 | 274 | 439 | 703 | 1,124 | 1,799 | 2,878 | 4,604 | 7,367 | 11,787 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 켜진 비율 | 0.06 | 0.07 | 0.09 | 0.13 | 0.19 | 0.09 | 0.23 | 0.59 | 0.96 | 0.93 | 0.79 | 0 (size cut) |
| 켜진 픽셀 | 33,453 | 16,534 | 8,840 | 4,822 | 2,804 | **520** | **520** | **520** | 336 | 130 | 63 | 0 |

원인(2026-09-20 정정 — 처음에 "repetition 레코드를 배열 전체 크기로 size cut한다"고 적은 것은
틀렸다. 인덱서의 컷 기준은 멤버 하나의 크기다: `PRec.w/h`, "rep excluded"):
① **size cut은 페이지 단위다.** 페이지는 그 안의 **가장 큰 도형**이 컷보다 작을 때만 잘린다
(`walk_pbvh`: `max_w < cut && max_h < cut`). `plan --explain`으로 확인: 이 배열은 TOP의 109/2 페이지
67번에 있고, 그 페이지는 다이의 절반(8,569 × 15,993 µm)을 덮으며 멤버 1,439,122개, 가장 큰 도형이
12.2 × 14.0 µm다(68번이 나머지 절반, 1,452,667개). 판정은 703 µm 뷰에서도 `page exact` — 14 µm가
3 px보다 작아지는 뷰(약 9,000 µm)까지 페이지 전체가 그대로 선택된다. 즉 **컷 이하의 멤버가 sub-cut
단계를 거치지 않고 일반 도형으로 그려진다**(박스·thin 판정 없음, keep = cull).
② 래스터에는 도형별 size cut이 없고, 픽셀 정책(KLayout hairline parity)이 픽셀보다 작은 도형을
축마다 가장 가까운 픽셀 하나로 접어 **반드시 1픽셀을 켠다.** 그래서 703 µm부터 멤버 520개가 정확히
520픽셀을 켜고, pitch가 1 px 아래로 내려가면 전부 이어져 꽉 찬 사각형이 된다.
반대 경우도 같은 뿌리다: 작은 도형만 있는 페이지는 통째로 잘리고, 페이지 범위가 박스(4 px)보다
넓으면 keep에서도 아무 stand-in 없이 사라진다(작은 재현 레이아웃: `pages_size 1`, 박스 0, 켜진
픽셀 0). 사용자의 판단("대상이 탈락하지 않는다, 픽셀 마스크가 두 번 그리지 않을 뿐")이 맞다.
sub-cut 박스도 같은 성질이다(4 px 이하의 노드·배치는 채움과 무관하게 box 하나 = 꽉 찬 밝기).

제안: **두 축 모두 1 px보다 작은 도형("점")은 자기 영역이 픽셀 중심을 덮을 때만 그 픽셀을 켠다**
(덮지 않으면 탈락). 기대 밝기 = 실제 면적 비율(fill)이라 위 배열은 어느 배율에서도 약 0.1로 남고,
성긴 배열은 성기게 남는다. 한 축만 가는 도형(배선)은 지금처럼 hairline으로 남긴다(thin keep의 목적).
- 속도: (a) 정규 격자는 x·y가 분리된다 — "픽셀 중심을 덮는 열 번호 집합 × 행 번호 집합"을 픽셀
  열·행을 따라가며 구하면 비용이 멤버 수가 아니라 **켜지는 픽셀 수**에 비례한다(지금은 멤버마다 방문).
  (b) 픽셀보다 작은 배치는 레이어 확인 읽기 전에 O(1)로 탈락한다. (c) 서로 겹치지 않는 같은 레이어의
  stand-in은 한 픽셀 중심을 둘이 덮을 수 없으므로 F2R-29에서 잰 같은 레이어 박스끼리의 중복(1
  레이어에서 94~96 %)이 구조적으로 사라지고, 만들어지는 stand-in 수가 화면 픽셀 수로 유계가 된다.
- 주의: ① KLayout 픽셀 parity와 갈라진다 → `exact`는 그대로 두고 keep/cull에만, 킬 스위치와 함께.
  ② 정규 배열 × 정규 픽셀 격자 = 모아레(Calibre도 같다). ③ pan 재사용 타일과 맞으려면 뷰 원점이
  픽셀 격자에 고정돼야 한다. ④ 외딴 작은 도형 하나는 대부분의 배율에서 사라진다. ⑤ 1~4 px 노드
  박스는 채움 정보가 없어 여전히 꽉 찬 밝기다 — 인덱스에 노드별 덮임 비율을 넣으면(v9, 재인덱싱)
  같은 규칙(픽셀마다 덮임 비율만큼 켬)을 쓸 수 있다. ⑥ 예산에 맞춘 밀도(1/M 페이지)는 밝기를
  1/M로 더 낮춘다(성긴 방향이라 어긋나지는 않는다).
- 순서(결정 후): 1. 래스터의 점 규칙 + 격자 분리 열거(배열·`Pts`), 위 재현 표와 시간으로 검증,
  2. 플래너의 픽셀 이하 배치·박스에 같은 규칙, 3. (선택) v9 노드 덮임 비율.

#### F2R-30 보충 — "hair cut 없이 한쪽만 컷보다 작아도 자른다"(사용자 제안, 2026-09-20) 검토

지금의 페이지 기준: size cut = `max_w < cut && max_h < cut`(keep·cull 공통), hairline cut =
`max_min < cut × 0.5`(cull에서만; `max_min` = 레코드별 min(w, h)의 최대, v6). 제안을 페이지 단위로
옮기면 `max_w < cut || max_h < cut`가 아니라 **`max_min < cut`**이어야 한다(가로·세로 배선이 섞인
페이지는 max_w·max_h가 둘 다 크다 — v6가 max_min을 넣은 이유). 즉 "hairline 계수 0.5 → 1.0, keep에도
적용"과 같다. `floe-index plan`으로 측정(합성 칩 1/10, 배선 레이어 16개, 1920×1080, 3 px, 박스 켬):

| 뷰 | keep(지금) 페이지 / 멤버 / 디코드 | cull(지금) | min(w,h) < cut | 박스 수(세 경우) | plan ms |
|---|---|---|---|---|---|
| fit | 1,161 / 2,990,536 / 57.7 MB | 88 / 46,605 / 14.5 MB | 67 / 37,115 / 11.6 MB | 1.82 M / 1.82 M / 1.80 M | 389 / 381 / 393 |
| ×4 | 1,175 / 5,677,099 / 63.2 MB | 366 / 43.7 MB | 69 / 31.9 MB | 1.02 M (같음) | 1,470 / 1,319 / 1,309 |
| ×16 | 7 | 4 | 4 | 69 K (같음) | 21 / 16 / 21 |

- 전제("잘린 것은 박스가 보여 준다")가 가는 선에는 성립하지 않는다: 박스는 footprint가 **두 변 모두**
  4 px 이하일 때만 생긴다(`box_small`). 배선 페이지는 길어서 박스가 없다 — 표에서 페이지가 1,161 →
  67로 줄어도 박스 수는 늘지 않는다. 즉 이 규칙은 지금 상태에서는 "조금 더 공격적인 thin cull"이고
  fit에서 배선 페이지의 94 %가 아무 대체 표시 없이 사라진다.
- 가는 선의 stand-in은 박스로는 만들 수 없다: 도형 하나의 box는 지금 그리는 1 px 선과 같고(절약
  없음), 페이지의 box는 거짓 블록이다(`sub_cut_verdict`가 sparse 페이지를 wash하지 않는 이유).
  쓸 수 있는 것은 **밀도**다: 픽셀 중심 규칙을 축별로 적용(가는 축이 픽셀 중심선을 덮는 선만 그림 →
  밝기 = 실제 금속 밀도)하거나, 인덱스에 페이지별 덮임 비율을 넣어 디코드 없이 그 비율로 칠한다(v9;
  페이지가 공간적으로 뭉쳐 있을 때만 의미가 있다 — TOP 109/2처럼 다이 절반을 덮는 페이지는 안 된다).
- 얻을 수 있는 것: fit에서 디코드 57.7 → 11.6 MB, 페이지 1,161 → 67. plan 시간은 그대로다(박스 걷기가
  지배).
- 결론: 기준을 하나로 합치는 방향은 맞지만 **가는 선의 stand-in이 먼저** 있어야 한다. 순서: F2R-30
  1단계(점 + 축별 픽셀 중심 규칙) → 그 뒤에 컷 기준 통합.

#### F2R-30 개정 — 2단계 모델: 컷 이상은 원본, 컷 이하는 밀도 (사용자 방향 2026-09-20, `DESIGN`, 코드 없음)

사용자: 양변 4 px 이하만 박스로 그리는 지금의 로직은 사실 밀도 표현을 위한 것이니, **컷 이상의 원본을
그리는 단계**와 **컷 이하를 밀도로 표현하는 단계**로 나누면 된다. 헤어라인처럼 한쪽이 긴 것을 어떻게
밀도로 바꿀지는 결정할 부분이고, 페이지 안의 큰 도형 때문에 작은 것이 컷되지 않는 것은 고친다는 전제다.

해석: 박스는 "덮임 비율을 1로 고정한 1칸짜리 밀도 표현"이고, 래스터의 1픽셀 규칙은 "도형마다 밀도
1픽셀"이다 — 뭉쳐서 밝은 사각형이 되는 두 현상의 뿌리가 같다. 밀도 단계는 기대 밝기 = 실제 덮임
비율이어야 한다. 유용한 성질: 밀도는 **그 레이어의 모든 도형**(원본으로 그린 것 포함)의 덮임으로
만들어도 된다 — 원본이 쓴 픽셀은 write-once로 막혀 있고 같은 레이어라 색이 같으므로 2단계는 1단계가
무엇을 그렸는지 몰라도 된다. 레이어 순서는 지금의 박스와 같다(레이어 L의 차례에 원본 → 밀도).

목표 그림(mock, 코드 변경 없음: 같은 영역을 8배 해상도로 그려 픽셀별 덮임 비율을 구하고 ordered
dither로 켬; TOP 109/2 배열, `data/synthetic/mock_density_array_109_2.png` 왼쪽 = 지금, 오른쪽 = 밀도):

| view µm | 439 | 703 | 1,124 | 1,799 | 2,878 | 4,604 |
|---|---|---|---|---|---|---|
| 지금 켜진 픽셀 | 2,820 | 520 | 520 | 520 | 374 | 154 |
| 밀도 mock | 753 | 317 | 151 | 81 | 50 | 37 |
| 실제 덮임(px) | 725 | 346 | 172 | 96 | 53 | 34 |

지금은 축소할수록 영역 대비 밝기가 올라가 꽉 찬 사각형이 되고, 밀도는 어느 배율에서도 실제 덮임을
따라간다(성긴 채로 작아진다). mock에서 본 주의점: ① 1~3 px 도형(첫 열)은 밀도로 가면 사각형 모양이
부서진다 → medium의 컷 3 px을 그대로 밀도 경계로 쓸지, 밀도 경계는 1 px로 두고 1~3 px은 원본으로
그릴지 정해야 한다. ② Bayer 행렬은 배열 pitch와 간섭 무늬를 만든다 → 실제 구현은 해시/blue-noise 문턱.

결정할 것과 추천:
1. **밀도의 출처.** (가) 렌더 때 원본을 표본화(픽셀 중심 규칙; 인덱스 변경 없음, 그러나 컷 이하 페이지를
   디코드해야 해서 비용이 도형 수와 디코드 예산에 묶인다 — 지금 keep의 비용 구조 그대로). (나) **인덱스에
   미리 계산한 덮임(v9)**: 셀 × 레이어의 재귀 덮임을 작은 격자로 저장(아래에서 위로 한 번 합성 — v8
   마스크처럼 색인 때 배치를 한 번 훑는다). 렌더 비용은 픽셀 수에 비례하고 디코드가 없으며, 박스 걷기가
   "4 px 이하"가 아니라 "격자 한 칸이 1~2 px이 되는 셀"에서 멈추므로 방문 노드가 크게 준다(F2R-29
   계측: 박스 rect의 70~99 %가 아무것도 못 쓰고 plan 0.2~1.4 s). 추천은 (나). 단 큰 셀의 **자체 도형**은
   셀 격자로는 너무 거칠다(TOP 격자 32칸 = 570 µm) → 페이지 단위 덮임이 필요하고, 그러려면 아래 3의
   페이지 분리가 같이 와야 한다. 기존 점유 요약(`OCCUPANCY_PLAN`)과의 차이: 그쪽은 평탄화한 **이진**
   비트맵이라 pitch < 1 px 배열은 역시 꽉 차고(밀도 숫자는 당시 비목표), 이번 것은 계층적이고 비율값이다.
2. **헤어라인(긴 변 ≥ 컷, 가는 변 < 컷).** (a) 면적으로 밀도에 합산 — 규칙이 하나이고 디코드가 없다;
   버스는 회색 띠, 외딴 긴 선은 점선처럼 보인다. (b) 선은 통째로 살아남거나 탈락(축별 픽셀 중심 규칙) —
   Calibre 관찰과 가장 비슷하지만 선마다 판정해야 해서 디코드가 필요하다. (c) 긴 변이 N px(예: 32~64)
   이상인 선은 원본으로 남기고 나머지는 (a) — 전원 스트랩·top 배선은 선으로 남고 수는 적다. 추천은
   (a)+(c)이고, 결정 전에 배선 레이어 한 영역으로 같은 방식의 mock 세 장을 만들어 비교한다.
3. **페이지 누수.** 인덱서가 페이지를 크기 등급(옥타브)과 가는/굵은으로 나눠 담는다(재인덱싱) + 래스터에
   도형 단위 컷 검사(등급 경계의 2배 이내 누수 제거). 누수만 먼저 고치면 지금 밝게 뭉치던 것이 대체 표시
   없이 사라지므로 **밀도와 같이** 들어가야 한다.
순서(결정 후): ① v9 셀 × 레이어 덮임(스칼라) → 박스를 "덮임 비율만큼 켜는 박스"로(가장 작은 변경으로 밝은
사각형 해소, 걷기는 그대로) ② 페이지 등급 분리 + 페이지 덮임 + 도형 단위 컷(누수 수정과 그 stand-in)
③ 격자로 확장해 걷기를 줄임(속도) ④ 헤어라인 결정 반영. F2R-29 A안은 ③ 뒤에 남는 탐색 비용을 보고 판단.

#### F2R-30 1단계 — 도형 단위 컷 (0.12.173, 2026-09-20, `DONE` — 합성 칩만, 실칩 미확인)

사용자 지시: 먼저 페이지의 컷 로직을 바꿔 큰 도형 때문에 작은 것이 살아남지 않게 하고, 컷 조건을
"두 변 중 하나라도 컷보다 작으면"으로 바꾼다. 대상은 thin keep(최종적으로 keep 하나로 합친다).
구현(SPEC-PLANNER §3 "도형 단위 컷"): 플래너는 페이지를 `max_min < cut`으로 자르고(페이지 BVH 노드는
`min(max_w, max_h) < cut`), 남은 페이지 안에서는 래스터가 min(w, h) < 컷인 레코드를 건너뛴다(계획이
컷을 `HierStats::shape_cut`으로 실어 보낸다). 인덱스 형식은 그대로다(재인덱싱 없음) — 큰 도형과
섞인 페이지의 **디코드 비용**은 남는다(페이지를 크기 등급으로 나누는 인덱서 변경은 다음 단계).
킬 스위치 `FLOE_RUST_SHAPE_CUT=off`. 덱 합성·cull·exact·CLI plan은 그대로.

- 관찰된 배열(TOP 109/2, 26×20, 멤버 0.4 µm): keep에서 멤버가 3 px 미만이 되는 274 µm 뷰부터
  그려지지 않는다(종전: 9,000 µm 뷰까지 520 px이 남아 밝은 사각형으로 뭉침). **대체 표시는 없다** —
  페이지가 잘린 것이 아니라 페이지 안의 도형이 빠진 것이라 sub-cut 박스도 생기지 않는다. 밀도 표현이
  다음 단계다.
- 합성 칩 1/10, 1920×1080, keep, medium, 킬 스위치 → 기본(프레임 s / 켜진 픽셀):

| 레이어 | fit | ×2 | ×4 | ×8 | ×16 |
|---|---|---|---|---|---|
| 배선 16개 | 0.48 → 0.47 / 1,019 K → 892 K | 0.65 → 0.56 / 2,055 K → 1,799 K | 1.06 → 1.06 / 2,074 K → 1,896 K | 0.61 → 0.60 / 같음 | 0.08 → 0.05 / −386 px |
| 앞 16개(역할 혼합) | 0.80 → 0.80 / 같음 | 1.09 → 1.02 / 같음 | 1.54 → 1.39 / +1 px | 0.86 → 0.88 / 같음 | 0.19 → 0.18 / 1,202 K → 1,160 K |
| 전체 449개 | 0.66 → 0.21 / −217 px | 0.27 → 0.36 / −25 px | 0.13 → 0.31 / 같음 | 0.09 → 0.06 / 같음 | 0.14 → 0.03 / 같음 |

  시간은 거의 그대로다: 16 레이어는 계획(박스 걷기 0.26~1.3 s)이 지배하고 그것은 이 변경과 무관하다.
  449 레이어의 ×2·×4가 느려진 것은 종전에는 예산에 맞춘 밀도가 걸려(`fit_thin` 6·1) 페이지를 솎아
  그렸고, 지금은 페이지가 줄어 예산 안에 들어가 솎지 않고 전부 그리기 때문이다(그림은 더 완전하다).
  배선 레이어의 fit에서 켜진 픽셀이 12 % 줄었다 — 컷 아래 배선이 대체 표시 없이 빠진 몫이다.
- 주의(이력): thin keep은 2026-09-10 현장에서 81~124 nm 선이 페이지째 사라진 것 때문에 생겼다(덱의
  마스크 소스). 이번 변경은 plain 레이아웃의 keep에만 걸고 덱 경로는 그대로 둔다.
  **마스크 OASIS를 단독으로 열어 keep을 고른 경우는 새 규칙을 받는다**: 합성 마스크 칩의 모서리 뷰
  (detail high)는 keep 41,619 px → 0 px이 된다(가는 선이 전부 컷 아래). 이것이 occupancy 게이트 4건을
  깨뜨렸고, 그 테스트들은 마스크 정책(덱의 페이지 경로와 그 단일 소스 기준선, sub-cut wash 진단,
  생성 계약)을 보는 것이므로 기준선 워커 3곳을 `FLOE_RUST_SHAPE_CUT=off`로 고정했다. 밀도 표현이
  들어오기 전까지 단독 마스크 파일의 keep 광역뷰가 필요하면 킬 스위치를 쓴다.

#### 리뷰 수정 (2026-09-20, 0.12.174): 예산 초과 시 페이지 순위가 여전히 긴 변 기준

[P2] 도형 단위 컷은 작은 변으로 고르는데 `thin_to_budget`은 keep의 종전 설정(hairline 0)대로
max(max_w, max_h)로 크기 등급을 매겼다. 재현(단위 테스트로 고정): 같은 비용의 10000×4 페이지와 64×64
페이지, 컷 3, 한 페이지분 예산 → 배선 페이지만 남고 정사각형 페이지가 빠지며, 컷을 5로 올리면
정사각형이 다시 나타난다(컷을 올렸는데 페이지가 돌아옴). `fit_full_pct`도 긴 변의 등급(273,067 %)을
보고했다. 수정: `FitKey::{LongerSide{hairline}, SmallerSide}` — 등급은 "그 페이지가 아직 선택되는 가장
큰 컷"이고 도형 단위 컷에서는 그것이 `max_min`이다. 이제 컷 3과 5 모두 정사각형 페이지를 고르고
상태는 `complete from x21.33, none below x2.67`(작은 변 기준)이다. 킬 스위치·cull의 등급은 그대로.

### F2R-25 — 라벨 bin 기반 선택의 제한적 검증 (`TODO`)

문제: 라벨 budget 소진 시 걷기 순서의 prefix만 표시돼 dense 뷰에서
라벨이 비는 영역이 생긴다. 승자는 bin당 하나라 유계지만 비용은 멤버
열거에 있다.

계획(cap 유지): 축정렬 규칙 Grid의 pitch가 bin보다 작을 때 bin마다
중심 최근접 멤버를 닫힌 식으로 선택하는 fast path만 먼저 구현·검증
한다. 승자 규칙 변경(hash 우선순위 → 중심 근접)을 문서화하고, skew·
축퇴·유한 경계·계층 변환은 기존 경로를 유지한다. 일반 Pts(bin별 후보
검색, chunk skip), block 이름(계약 테스트: 같은 bin의 독립 block 이름
전부 유지), 긴 문자열(glyph 정책)은 계약을 먼저 정한 뒤 별도 이슈로
연다.

수용 gate: 축정렬 Grid 합성 입력에서 bin당 라벨 존재율 100%와 멤버
수 무관 계획 시간, 기존 declutter 결정성 테스트 통과, `labels
partial` 발화율 감소를 실칩에서 확인.

### F2R-26 — 실측 기반 fallback 개선 (`GATED`)

F2R-23 계측이 실칩에서 실제 병목을 가리킬 때만 착수한다. 후보:
work-bin continuation(실패 trial의 결과를 버리지 않고 남은 작업만
넘기는 부분 bin, DFS·plane 순서 보존, mini-bin 합계 byte 상한),
Pts 과포함 축소(chunk bbox 사다리 세분화), mask 공유·sparse 표현.

### F2R-28 — write-once 타일: 레이어 간 덧칠 제거 (`DONE` 0.12.165)

문제(합성 MAIN01 기준 측정, 위 F2R-24): 광역뷰에서 래스터가 켜진 픽셀 하나를 약 300번 칠한다.
449개 레이어가 같은 면적을 차례로 덮어쓰는 레이어 간 덧칠이고, 예산이 무한이어도 프레임이
18~187 s다.

구현(2026-09-18, `rust/render-core/src/raster.rs`): 모든 도형 paint는 plane의 한 색으로 하는
불투명 덮어쓰기이므로 프레임은 "픽셀에 마지막으로 쓴 plane이 이긴다"의 순수 함수다. 그래서
- 타일마다 픽셀당 1비트 마스크(`WriteOnce`)를 두고, pass 순서(프레임 밴드 2·3·1 → plane들 →
  프레임 밴드 0)를 **뒤집어** 그리며 픽셀은 **한 번만** 쓴다. 채움 규칙(solid·speckle·16×16
  stipple)은 64열 워드 마스크로 계산하고, 패턴이 꺼진 픽셀은 쓰지 않으므로 아래 plane에 열려
  있다. 외곽선·hairline·요약 plane·대표 span도 같은 경로로 쓴다. 라벨은 지금처럼 맨 끝의
  블렌딩 pass다.
- 건너뛰기: ① 타일의 모든 픽셀이 쓰이면 남은 pass를 끝낸다(`once_tiles`, `once_passes`).
  ② pass마다 컬링 뷰를 **아직 열린 픽셀의 경계 상자**(타일 자신의 stroke 여유 + 1 px)로
  줄인다 — 기존 페이지·레코드 범위·멤버 열거 컬링이 그대로 좁아진다. ③ cell visit·페이지·
  인스턴스 멤버·wash·점 chunk는 기기 bbox(stroke 폭 + 2 px 여유)가 모두 쓰였으면 열거하지
  않는다(`once_items`). ④ 멤버 열거는 타일이 차면 그 자리에서 멈춘다.
- 킬 스위치 `FLOE_RUST_WRITE_ONCE=off`(순서대로 덮어쓰는 기준 경로). Styled 모드 전용이고
  occupancy 모드·덱 합성·라벨·pan 재사용은 그대로다.

검증: 단위 테스트 2개 — 무작위 4-레이어 장면(배열·다각형·path·wash·프레임 밴드 4종, 채움 4종,
외곽선 1~3 px, 타일 8/16/64, bin·walk 양쪽)이 기준 경로와 바이트 동일하고 타일이 실제로
차는지; span 단위의 채움 규칙과 열린 픽셀 수. 변이 검사(순서를 뒤집지 않음, 열린 상자 여유를
줄임) 둘 다 실패로 잡힌다. 기존 테스트 119개는 write-once 기본 on으로 통과(그리기 순서·프레임
밴드 겹침·타일 크기 불변 포함; bin 대 walk 방문 수 비교 1건은 기준 경로로 고정). 배터리 게이트
`write_once`(tools/validate_write_once.py, `render` 별칭): 합성 MAIN01과 valmini의 18개
프레임(광역/근접, keep/cull, 프레임·라벨, depth 1/full)이 킬 스위치와 바이트 동일.

측정(합성 MAIN01 1/10, 1920×1080, thin keep, detail high, 전 레이어, 래스터 ms, 끔 → 켬; 모든
프레임 바이트 동일):

| 뷰 | 끔 | 켬 | 배 | 멤버 paint |
|---|---|---|---|---|
| fit | 6,148 | 3,542 | 1.7 | 1.14억 → 2,607만 |
| ×16 | 5,573 | 918 | 6.1 | 797만 → 38만 |
| ×32 | 265 | 81 | 3.3 | 210만 → 21만 |
| ×64 | 129 | 52 | 2.5 | 55만 → 12만 |
| ×2 (예산 16 GB) | 18,797 | 5,538 | 3.4 | 3.03억 → 1,034만 |
| ×4 (예산 16 GB) | 13,992 | 3,655 | 3.8 | 1.69억 → 677만 |
| ×8 (예산 16 GB) | 190,697 | 7,667 | 24.9 | 1.14억 → 146만 |

sample9(9 레이어, keep, 프레임·라벨): fit 228 → 11 ms(20배), ×3 217 → 15 ms(15배).
손해: 겹침이 없는 작은 프레임은 래스터가 0.1~1.0 ms 느려진다(valmini 1.3 → 1.5, 2.4 → 3.1,
7.9 → 8.9 ms; sample9 ×10·×40은 동률). 도형마다 마스크 워드를 읽고 쓰는 고정비다. 사전에
말한 "5 % 이내"는 ms 단위 프레임에서 지키지 못했고(10~25 %), 절대값은 1 ms 이하다.

남은 비용(샘플링): write-once 뒤의 draw 시간은 paint가 아니라 프레임당 단일 스레드 단계다 —
`SceneMasks::build`, work bin 수집(`collect_cell`), `subtree_intersects`. ×2(16 GB)의 5.5 s가
거의 이것이다. 타일을 128/64 px로 줄이면 fit의 paint는 더 줄지만(2,607만 → 447만/223만) ×16은
walk가 늘어 훨씬 느려지므로 기본 384 px를 유지했다. 가려진 영역의 손상 페이지 오류는 그
페이지를 열거하지 않으므로 보고되지 않는다(킬 스위치 경로는 종전대로 보고).

### F2R-27 — generation streaming/1bpp와 질의 경로 분리 (`DESIGN`)

뷰당 decoded 합 1GiB 오류 또는 peak 메모리 압박이 실칩에서 확인될
때 착수한다. plane별 1bpp mask에 페이지별로 누적한 뒤 페이지를
버리는 방향은 유효하지만, pick/snap이 PublishedScene의 decoded
페이지에 의존하므로 질의 경로가 필요 시 페이지를 다시 읽는 구조로
분리해야 하며, mask 메모리는 plane 수에 비례한다(4K×200 plane ≈
198MiB). F2R-03c와 함께 설계한다.

## 6. 남은 착수 순서

1. 완료(2026-08-26): F2R-12 worker pin·bench split·serial preset과 F2R-03a
   상수 비용 제거. sample9 serial 66ms로 gate 통과(§3.10). 남은 확인: 같은
   머신 KLayout GUI 재측정과 대표 실칩 trace.
2. 완료(2026-08-26): F2R-03b 1단계 sub-page record index. 제품 raster
   26.0→20.4ms, 128px tile -32%, serial 동률(§3.11).
3. 완료(2026-09-01): F2R-10 pan 판정 — sample9(§3.12)에 이어 실칩
   pan 실측(상하좌우 20%)에서도 floe2 draw 우세(§3.16). 경쟁 축 종결.
4. F2R-03b 완료: 2a Pts chunk(-95% member 테스트), 2b layer mask
   (deep-zoom 439→22ms), **2c work bin**(tile×plane walk 곱 제거,
   deephier hier -85%, sample9 제품 raster -21%, byte 동일 oracle).
   F2R-09 cost-aware round(rounds 5→2)와 함께 §3.15 문제 뷰의 두
   곱셈을 모두 제거 — 실칩 재측정으로 남은 격차(예상: per-member
   상수, F2R-03c 1bpp plane 영역)를 판정한다. load 축 후보들
   (lazy-index·인코드 캐시·budget)은 §3.15에서 비병목 판정, 후순위.
5. 기각/보류(2026-09-01, §3.16): F2R-10 world-aligned tile LRU —
   device-anchored fill 위상 때문에 RGBA 캐시는 byte gate 불통과.
   F2R-03c(1bpp plane) 착수 + pan 절대 지연의 실칩 UX 지목이 재개
   조건이며, 그때 WEBUI T3와 함께 재설계한다.
6. 완료(2026-09-02): F2R-13 raw RGBA frame handoff(0.12.26). 실칩
   확인 완료(§3.16): raw 0.8ms/pub 8.4ms, raster 무회귀, wall
   ~50ms/frame + 주 스레드 디코드 제거. 같은 실측에서 pan 1步의
   잔여 비용 순서도 확정 — 재raster ~1.18s(gated), plan+text plan
   ~237ms(비gated 후보, pan UX 요구 발생 시 1순위).
7. 완료(2026-09-02): F2R-03 2c deferred-subtree 지렛대 소진
   (0.12.27~32, §3.17) — 실칩 5회 왕복으로 원인 축차 확정(투영
   과대계상 → trial 실측 → cap 초과 확정) 후 tile-side 결합 mini
   walk로 종결. depth-3 실칩 뷰 draw 2,067→332ms(gate 220배 감소),
   floe 대비 draw 3.0배·total 5.5배. spillover 확인 완료: mid-zoom
   문제 뷰(§3.15)도 894ms(원점 13배, floe 대비 5.7배, draw 2.2배
   우세). 남은 비긴급 신호: 실패 trial ~90ms/frame(depth-3), plan
   216~350ms(두 뷰 공통 최대 단일 성분 — 인접 뷰 plan 재사용
   후보로 수렴).
8. 1024-page를 넘는 장시간 cold fixture로 F2R-11 final-tile streaming의 first/settled
   이득을 측정한 뒤 protocol 변경 여부를 결정한다. 500~700ms 이하 작업에는 refinement를
   만들지 않는다. 기본 no-refinement 채택 후 동기가 약해져 후순위다.
9. F2R-06은 thread startup 실측이 frame의 5%를 넘을 때만 수행한다. 대표 실칩에서
   4-worker tail imbalance가 확인될 때만 bounded adaptive tile/jobs를 다시 연다.

10. F2R-23 관측성(P1): snap partial, frame 생략·융합 플래그, mini/
    trial/Pts/mask 계측, RSS — 이후 모든 budget 판단의 근거.
11. F2R-24 decode 전 byte admission(P2): 작은 변경, 03c와 별개.
12. F2R-25 라벨 bin 선택 제한 검증(P2): 축정렬 Grid fast path, cap
    유지, 계약 문서화 후 확장.
13. F2R-26 실측 기반 fallback 개선(GATED): 10번 계측으로 병목 선택.
14. F2R-27 generation streaming/1bpp(DESIGN): 실칩 필요성 확인 후
    질의 경로 분리와 함께.

각 완료 항목은 이 표의 상태, before/after 중앙값, 실행 명령, 적용 커밋과 자동 gate를
같이 갱신한다.
