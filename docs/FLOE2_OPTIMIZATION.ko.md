# floe2 renderer 최적화 추적

갱신일: 2026-08-26

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

## 4. 이슈 목록

| ID | 우선순위 | 상태 | 요약 | 다음 판정 |
|---|---:|---|---|---|
| F2R-01 | P1 | `DONE` | cache hit까지 128-page refine | cache-aware batch/unit+현장 gate 완료 |
| F2R-02 | P1 | `DONE` | 128px tile의 반복 hierarchy 순회 | 제품 기본 384px 승인 |
| F2R-03 | P1 | `DOING` | tile x layer x hierarchy 총 CPU 작업량 | 03a~03b-**2c까지 완료** — 2c work bin으로 tile×plane 곱 제거(§3.15 후속). 남음: 실칩 재측정, 03c(1bpp plane) |
| F2R-04 | P1 | `DONE` | total에서 사라진 PNG/publish 37~44ms | write/sync/rename/handoff 계측 완료 |
| F2R-05 | P2 | `DONE` | jobs=8의 CPU/전력 비용 | decode 8/raster 4 분리 승인 |
| F2R-06 | P3 | `BLOCKED` | render마다 OS thread 생성 | startup_us가 병목일 때만 pool |
| F2R-07 | P1 | `DONE` | OVC coverage post-composite 회귀 | 제품 경로 제거 gate 유지 |
| F2R-08 | P1 | `DONE` | exact 재방문도 full raster/PNG 반복 | bounded PNG+scene 복원 gate 완료 |
| F2R-09 | P1 | `DONE` | 대형 cold 뷰에서 refinement 왕복이 draw를 ~3배로 | cost-aware collapse 구현(500ms 예산, §3.15) — 실칩 재측정 대기 |
| F2R-10 | P1 | `OPEN` | exact 밖 인접 pan은 full viewport raster | 실체는 load 재사용(관찰 갱신); 실칩 sweep의 load 축으로 판정 |
| F2R-11 | P2 | `DESIGN` | page-round refinement가 누적 full PNG 반복 | final-tile streaming 채택 여부 결정 |
| F2R-12 | P1 | `DOING` | KLayout single-core parity와 Rust serial 기준선 | sample9 gate 통과 124%(§3.12); 실칩 p50/p95 남음 |

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

  **착수 판정과 순서**: 실칩 bench의 wc_cells/inst_edges/render_tiles/
  layer 수로 traversal 곱셈 비중을 추정해 2b·2c의 기대 이득을 정한다.
  구현 순서는 2a → 2b → (실칩 판정 후) 2c. record 수 대비 traversal
  비중이 낮으면 2c는 F2R-11 채택 시점까지 미룬다. 1단계 index build가
  실칩 decode에서 병목으로 나타나면(§3.11: sample9 decode +70%) index를
  첫 raster 사용 시 lazy 구축하는 변형을 2a와 함께 검토한다. OVP에
  사전 계산 index를 저장하는 안은 포맷 변경이라 별도 결정으로 남긴다.
- **F2R-03c (`DESIGN`)** — fill 파이프라인 재설계: 2-phase를 단일 edge walk
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

### F2R-10 — same-scale 인접 viewport retained 재사용 (`OPEN`)

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
opt-in한다. budget 초과 재방문은 OS page cache가 인코드 .floe
구간을 들고 있으므로 read가 아닌 **decode-only 비용**으로
기대한다(sample9 실측 read는 decode의 0.6%). 따라서 이 축의 다음
지렛대는 decode 절감 — index build가 decode CPU의 41%(§3.14)라
**lazy-index 변형**이 1순위이고, 실칩 `read_ms`가 유의미하게 나오면
(네트워크 저장소) 인코드 victim cache를 재검토한다. 세션 시작
`[perf] backend=rust renderd=… budget-mb=…` 라인이 유효값을 찍는다.

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

## 6. 남은 착수 순서

1. 완료(2026-08-26): F2R-12 worker pin·bench split·serial preset과 F2R-03a
   상수 비용 제거. sample9 serial 66ms로 gate 통과(§3.10). 남은 확인: 같은
   머신 KLayout GUI 재측정과 대표 실칩 trace.
2. 완료(2026-08-26): F2R-03b 1단계 sub-page record index. 제품 raster
   26.0→20.4ms, 128px tile -32%, serial 동률(§3.11).
3. F2R-10 fixed-scale pan sweep은 sample9에서 완료(§3.12): 인접 pan 열세가
   재현되지 않았고 floe2가 전 구간 우세였다. 관찰이 나온 실칩 trace에서 같은
   `--pan-sweep`을 재측정한 뒤 world-tile 착수를 판정한다.
4. F2R-03b 완료: 2a Pts chunk(-95% member 테스트), 2b layer mask
   (deep-zoom 439→22ms), **2c work bin**(tile×plane walk 곱 제거,
   deephier hier -85%, sample9 제품 raster -21%, byte 동일 oracle).
   F2R-09 cost-aware round(rounds 5→2)와 함께 §3.15 문제 뷰의 두
   곱셈을 모두 제거 — 실칩 재측정으로 남은 격차(예상: per-member
   상수, F2R-03c 1bpp plane 영역)를 판정한다. load 축 후보들
   (lazy-index·인코드 캐시·budget)은 §3.15에서 비병목 판정, 후순위.
5. 같은 work bin 위에서 F2R-10 world-aligned tile LRU를 작게 prototype하고 field trace
   20% gate를 넘을 때만 제품화한다.
6. 1024-page를 넘는 장시간 cold fixture로 F2R-11 final-tile streaming의 first/settled
   이득을 측정한 뒤 protocol 변경 여부를 결정한다. 500~700ms 이하 작업에는 refinement를
   만들지 않는다.
7. F2R-06은 thread startup 실측이 frame의 5%를 넘을 때만 수행한다. 대표 실칩에서
   4-worker tail imbalance가 확인될 때만 bounded adaptive tile/jobs를 다시 연다.

각 완료 항목은 이 표의 상태, before/after 중앙값, 실행 명령, 적용 커밋과 자동 gate를
같이 갱신한다.
