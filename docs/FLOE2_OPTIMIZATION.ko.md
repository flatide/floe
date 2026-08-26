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

## 4. 이슈 목록

| ID | 우선순위 | 상태 | 요약 | 다음 판정 |
|---|---:|---|---|---|
| F2R-01 | P1 | `DONE` | cache hit까지 128-page refine | cache-aware batch/unit+현장 gate 완료 |
| F2R-02 | P1 | `DONE` | 128px tile의 반복 hierarchy 순회 | 제품 기본 384px 승인 |
| F2R-03 | P1 | `DOING` | tile x layer x hierarchy 총 CPU 작업량 | 03a·03b record index 완료(§3.10~11). 실칩 확인과 2단계(work bin) 필요성 재평가 |
| F2R-04 | P1 | `DONE` | total에서 사라진 PNG/publish 37~44ms | write/sync/rename/handoff 계측 완료 |
| F2R-05 | P2 | `DONE` | jobs=8의 CPU/전력 비용 | decode 8/raster 4 분리 승인 |
| F2R-06 | P3 | `BLOCKED` | render마다 OS thread 생성 | startup_us가 병목일 때만 pool |
| F2R-07 | P1 | `DONE` | OVC coverage post-composite 회귀 | 제품 경로 제거 gate 유지 |
| F2R-08 | P1 | `DONE` | exact 재방문도 full raster/PNG 반복 | bounded PNG+scene 복원 gate 완료 |
| F2R-09 | P1 | `DONE` | 744-page pan에서 6회 full raster/PNG | 제품 round 1024 승인 |
| F2R-10 | P1 | `OPEN` | exact 밖 인접 pan은 full viewport raster | pan sweep 후 world-tile prototype 판정 |
| F2R-11 | P2 | `DESIGN` | page-round refinement가 누적 full PNG 반복 | final-tile streaming 채택 여부 결정 |
| F2R-12 | P1 | `DOING` | KLayout single-core parity와 Rust serial 기준선 | pin/split/serial 코드 완료, canonical 측정 남음 |

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
- **F2R-03b 2단계 (`OPEN`)** — visited record의 `Rep::Pts` member 전량
  스캔에 chunk bbox를 도입하고, frame당 1회 traversal의 (image tile ×
  paint plane) work bin과 cell×layer bbox subtree pruning을 구현한다.
  1단계로 tile 축 곱셈이 크게 줄었으므로, 2단계는 F2R-10 world tile/
  F2R-11 dependency graph 요구와 묶어 필요성을 재평가한 뒤 착수한다.
  bin key는 world/scale 정렬 tile로 확장 가능하게 설계한다.
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

### F2R-09 — interactive cold-miss round (`DONE`)

- 원인: 128-page decode round마다 지금까지의 누적 scene 전체를 다시 raster/PNG 게시
- 제품 정책: 1024 miss pages까지 single settled frame. 그 이상에서만 progressive
- 근거: GUI는 새 frame을 기다리는 동안 직전 완성 frame을 frozen preview로 표시하므로
  sub-second 작업의 partial PNG가 검은 화면을 막아 주는 역할을 하지 않음
- gate: adapter 기본 wire `round_pages=1024`, benchmark `--round-pages` 기본과 일치,
  1024 초과 unit/cancellation progressive 계약 유지

### F2R-10 — same-scale 인접 viewport retained 재사용 (`OPEN`)

확정된 현재 경계:

- GUI `last_frame`/`_covered()`는 floe와 floe2 공통이며 넓은 인접 cache가 아니다.
- floe는 persistent KLayout `Layout`/`LayoutView`와 resident page-cell을 유지한다.
- floe2의 decoded-page hit는 raw geometry read/decode만 줄이고, 새 viewport의 scene
  traversal/raster/PNG는 줄이지 않는다.
- exact frame cache는 zoom 복귀에는 유효하지만 좌표가 다른 인접 pan에는 맞지 않는다.

먼저 같은 zoom/detail/depth/layer에서 warm settle 후 X/Y로 화면 폭의
`1/16, 1/8, 1/4`만큼 이동하고 되돌아오는 trace를 각각 3회 측정한다. floe는
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
4. 부분 완료(2026-08-26): canonical sample9로 Rust serial/제품 profile을 3회
   측정했다(§3.10; bench에 `--frame-cache off`/`--perf-baseline` 표면 추가).
   남음: 같은 머신의 floe/KLayout GUI 세션 재측정(새 `[perf]` 라인으로
   version/worker 고정 확인)과 대표 실칩 trace.

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
3. F2R-10의 fixed-scale pan sweep과 persistent/new KLayout `LayoutView` 진단으로 인접
   cache 체감을 수치화한다.
4. F2R-03b 2단계(frame당 1회 tile×plane work bin, cell×layer bbox, Pts chunk
   bbox)는 F2R-10/11 요구와 묶어 필요성을 재평가한 뒤 진행한다. exact 재방문
   cache를 회귀시키지 않는 경우에만 편입한다.
5. 같은 work bin 위에서 F2R-10 world-aligned tile LRU를 작게 prototype하고 field trace
   20% gate를 넘을 때만 제품화한다.
6. 1024-page를 넘는 장시간 cold fixture로 F2R-11 final-tile streaming의 first/settled
   이득을 측정한 뒤 protocol 변경 여부를 결정한다. 500~700ms 이하 작업에는 refinement를
   만들지 않는다.
7. F2R-06은 thread startup 실측이 frame의 5%를 넘을 때만 수행한다. 대표 실칩에서
   4-worker tail imbalance가 확인될 때만 bounded adaptive tile/jobs를 다시 연다.

각 완료 항목은 이 표의 상태, before/after 중앙값, 실행 명령, 적용 커밋과 자동 gate를
같이 갱신한다.
