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

**2단계(진행 중)**: renderd retained geometry frame + GUI pan
16px-스냅 + 노출 띠만 재raster. 기대: 20% pan draw ~1.2s→~0.35s,
50%→~0.65s(이동량 비례 회복). kill switch `FLOE_RUST_PAN_REUSE=off`.
드래그 settle은 v1에서 full raster 유지.

## 4. 이슈 목록

| ID | 우선순위 | 상태 | 요약 | 다음 판정 |
|---|---:|---|---|---|
| F2R-01 | P1 | `DONE` | cache hit까지 128-page refine | cache-aware batch/unit+현장 gate 완료 |
| F2R-02 | P1 | `DONE` | 128px tile의 반복 hierarchy 순회 | 제품 기본 384px 승인 |
| F2R-03 | P1 | `DOING` | tile x layer x hierarchy 총 CPU 작업량 | 2c 완결(§3.17): depth-3 draw 332ms(3배 우세)·mid-zoom draw 498ms(2.2배 우세), 원점 11.7s→894ms(13배). 남음: 03c(트리거 미발화 유지) |
| F2R-04 | P1 | `DONE` | total에서 사라진 PNG/publish 37~44ms | write/sync/rename/handoff 계측 완료 |
| F2R-05 | P2 | `DONE` | jobs=8의 CPU/전력 비용 | decode 8/raster 4 분리 승인 |
| F2R-06 | P3 | `BLOCKED` | render마다 OS thread 생성 | startup_us가 병목일 때만 pool |
| F2R-07 | P1 | `DONE` | OVC coverage post-composite 회귀 | 제품 경로 제거 gate 유지 |
| F2R-08 | P1 | `DONE` | exact 재방문도 full raster/PNG 반복 | bounded PNG+scene 복원 gate 완료 |
| F2R-09 | P1 | `DONE` | 대형 cold 뷰에서 refinement 왕복이 draw를 ~3배로 | cost-aware collapse 구현(500ms 예산, §3.15) — 실칩 재측정 대기 |
| F2R-10 | P1 | `CLOSED` | exact 밖 인접 pan은 full viewport raster | 실칩 pan 실측으로 경쟁 축 해소(§3.16); world-tile은 F2R-03c 선행 조건부 |
| F2R-11 | P2 | `DESIGN` | page-round refinement가 누적 full PNG 반복 | 기본 no-refinement로 동기 약화; final-tile streaming은 보류 |
| F2R-12 | P1 | `DOING` | KLayout single-core parity와 Rust serial 기준선 | sample9 gate 통과 124%(§3.12); 실칩 p50/p95 남음 |
| F2R-13 | P1 | `DONE` | 인터랙티브 frame의 PNG 인코드/디코드 왕복 | 실칩 확인(§3.16): raw 0.8ms/pub 8.4ms, raster 무회귀 — wall ~50ms/frame + 주 스레드 디코드 제거 |
| F2R-14 | P1 | `DONE` | 장시간 세션에서 load 증가(미니맵 이동, fresh 뷰어보다 느림) | LRU 축출 전수 스캔 → O(log n) 색인(0.12.33, §3.18) + `evict N` telemetry — 실칩 장기 세션 재확인 대기 |
| F2R-15 | P1 | `DONE` | pick/snap이 대부분 "no object here" | 예산 모델 폐기·floe 정렬(0.12.36, §3.19) — 9.8G 실칩 확인 완료. cut 실명(결함 B)은 보류(설계 219259c) |
| F2R-16 | P1 | `DOING` | pan이 이동량과 무관하게 전체 재raster | 1단계 라벨 최상단(0.12.37, 사용자 승인) 완료. 2단계 retained+16px-스냅 띠 재사용 진행 중(§3.20) |

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
opt-in한다. budget 초과 재방문은 OS page cache가 인코드 .floe
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

각 완료 항목은 이 표의 상태, before/after 중앙값, 실행 명령, 적용 커밋과 자동 gate를
같이 갱신한다.
