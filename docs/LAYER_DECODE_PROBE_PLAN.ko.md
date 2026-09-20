# 전체 플랜 유지 + 레이어 순서 디코드 검증 계획

상태: **1~3단계 구현·검증 완료(0.12.177), 실칩 측정 전**. 2026-09-20.
- 있는 것: `render_probe` 명령(`mode=baseline|ordered`), `LayerRasterSession`(프레임 내내 살아 있는
  타일과 pass 단위 그리기, 워커 풀 유지), 게이트 `tools/validate_layer_decode.py`, 벤치
  `tools/bench_layer_decode.py`. 이름은 계획의 제안과 다르다: 드라이버는 renderd의 `run_layer_probe`
  (플랜·selected·스타일을 만드는 `run_render`의 앞부분을 그대로 쓰기 위해서다), `render-core/src/
  layer_decode.rs`에는 모드와 보고 타입만 있다. `finish`는 `render_layered` 하나로 합쳤다(호출자가
  pass마다 콜백을 받는다). `render_probe`는 별도 명령이지만 `render`와 같은 필드 파서를 쓴다.
- 2단계(0.12.177): `Cache::page_geometry`(payload 없이 레이어·cell-local bbox)와
  `FrameScene::new_metadata` — 플랜의 **모든** 페이지를 아는 장면. 메인 bin·미니 bin·하위 레이어
  마스크가 전부 이 메타로 만들어지고(미니 bin은 같은 `collect_cell`을 타므로 함께 해결된다),
  디코드된 페이지는 `set_decoded_page(&self)`로 슬롯에 들어간다 — 장면도 마스크도 다시 만들지 않는다.
- 3단계(0.12.177): `BlockDemand::pages_for_plane` — 블록 시작 마스크로 그 블록의 필요 페이지를
  정한다. 공유 페이지는 **어느 인스턴스라도** 열릴 가능성이 있으면 읽고, 화면 밖은 따로 센다.
  deferred edge는 걷지 않고 그 레이어를 통째로 읽는다(`unsure`). `mode=occlusion`은 work bin과
  write-once 마스크가 있어야 하며 없으면 오류다(가림을 증명할 수 없다).
- 없는 것: 적응형 블록 크기, 예상 디코드량 기반 블록 제한, **실칩 측정**.
- 실칩에서 돌릴 것(대표 뷰마다, depth full과 0 둘 다):
  `tools/bench_layer_decode.py <cache> --modes baseline,ordered:8,occlusion:8,occlusion:1000
  --layers all --zooms 1,4,8 --repeat 3 --warm 2 --center <x,y um> --json <out>.json`.
  회수할 값은 `demand_occluded`/`demand_unsure`(deferred edge라 레이어를 통째로 읽은 수) 대
  `selected_pages`다 — 실제 공유 인스턴스와 deferred 배열에서 생략이 얼마나 남는지가 거기 드러난다.
  `occlusion:1000`은 가림을 쓰지 못하는 대조군이다(블록 하나는 아무것도 그려지기 전에 판정한다).
- 아래 §7의 상태 구분(`explicitly_deferred`)과 §8의 일부 항목은 아직 제안이다.

## 1. 목적과 비교 계약

전체 계층 플랜을 한 번 만든 뒤, 위 레이어를 그린 마스크로 아래 레이어의 불필요한 디코드를
생략한다. 이번 단계에서 확인할 것은 **실제 디코드 절약량과 그 판정에 추가되는 비용**이다.
플래너를 레이어별로 다시 실행하거나 가림을 BVH 계획에 반영하는 작업은 다음 단계다.

근거: [F2R-28](FLOE2_OPTIMIZATION.ko.md)의 2026-09-20 계측에서 합성 MAIN01 1/10,
449개 레이어의 디코드 페이지 중 76~99%가 래스터의 레코드 순회에 도달하지 않았다.
92~99%는 픽셀을 쓰지 못했다. 이 둘은 **실제 생략 가능한 디코드 비율이 아니다**.
같은 레이어 안의 가림, 도형별 컷, bbox의 빈 공간, 공유 페이지의 다른 인스턴스가 포함되기 때문이다.

세 모드를 같은 요청·스타일·예산으로 비교한다.

| 모드 | 디코드 | 그리기 | 목적 |
|---|---|---|---|
| `baseline` | 기존 selected 전체 | 기존 write-once 렌더, 최종 프레임 한 번 | 기준 그림·시간 |
| `ordered` | 같은 selected를 위 레이어부터, 생략 없음 | 픽셀·마스크를 유지하며 레이어별 한 번 | 순서 변경 자체의 비용·정확성 |
| `occlusion` | ordered에서 열린 픽셀에 닿을 가능성이 없는 페이지만 제외 | ordered와 동일 | 디코드 생략의 순이익 |

공통으로 `HierPlan`을 한 번만 생성한다. `plan.pages`, `wcells`, `page_levels`, washes,
`shape_cut`, `fit_*`, 페이지 우선순위, `decode_pages` 제한 결과를 바꾸지 않는다.
`selected`는 지금처럼 우선순위 정렬과 `decode_pages` 적용을 끝낸 집합이다.
예산에 맞춰 이미 솎인 페이지를 되살리거나, 레이어마다 전체 예산을 새로 배정하지 않는다.
플랜/selected/스타일 지문과 실제 비교 가능한 계획 카운터를 세 모드에서 대조한다.

첫 단계는 **레이어 사이의 가림만** 디코드 생략에 사용한다. 레이어 L 시작 시 마스크는 더 위의
레이어가 쓴 픽셀뿐이다. L에서 읽을 집합을 먼저 확정하고 그 집합을 모두 디코드한 뒤 L을 그린다.
같은 레이어 안에서 페이지를 조금씩 읽으며 가림을 갱신하는 기능은 후속 실험으로 분리한다.

## 2. 실행 범위와 진입점

GUI 기본 경로를 바꾸지 않는 **renderd 전용 진단 명령 `render_probe`**를 추가한다.
기존 render 요청 필드와 `make_plan_request`를 재사용하고 `mode=baseline|ordered|occlusion`을 받는다.
실행 도구는 `tools/validate_layer_decode.py`와 `tools/bench_layer_decode.py`로 분리한다.
벤치마크 도구가 요청·스타일·환경·바이너리 버전과 JSON 계측 결과를 저장한다.

- 대상: 일반 레이아웃, styled write-once, keep/cull, depth 0~full, medium을 포함한 기존 컷.
- 1차 검증은 `frames=off`, `labels=off`, retained/pan 재사용 off, OVO/OVR off,
  page reps/sub-cut wash 진단 경로 off, LOD 없는 캐시다. sub-cut box는 기존 설정 그대로 비교한다.
- 지원하지 않는 조합은 명시적으로 거절한다. 실험 모드가 실행된 것처럼 보고하며 구 경로로
  조용히 돌아가지 않는다. 각 결과에 실제 실행 모드와 환경을 적는다.
- 점진 프레임은 내보내지 않고 최종 프레임 한 번만 낸다. baseline도 같은 조건이다.
- 결과는 일반 `frame`과 구분한 `probe_frame` 응답으로 낸다. `PublishedScene`, GUI의 query/pick,
  retained 프레임은 갱신하지 않는다. 진단 결과를 정상 GUI 장면으로 취급하지 않는다.
- 현재 `floe-render-cli::make_request`는 `shape_cut=false`, `decode_budget=0` 등 뷰어와
  다른 설정이다. 이를 그대로 사용해 medium 뷰어의 결과라고 측정하지 않는다.

이 범위는 실제 레이아웃 파일을 headless로 측정하기 위한 것이며 GUI 적용 완료를 뜻하지 않는다.

## 3. 먼저 분리해야 하는 현재 결합

| 현재 코드 | 결합된 부분 | 변경 방향 |
|---|---|---|
| `renderd/src/main.rs::run_render` | 전체 selected → refinement batches → decoded_pages 누적 | 동일 plan/selected를 진단 드라이버에 전달 |
| `render-core/src/scene.rs::SceneMasks` | 디코드된 페이지만 레이어 마스크에 포함 | 조회용 메타 마스크와 실제 그리기용 가용 페이지를 분리 |
| `raster.rs::collect_cell` | page bbox/layer를 `scene.page()`에서 읽음 | 메타 조회 어댑터를 추가해 payload 없이 bin 수집 가능 |
| `raster.rs::render_geometry_impl` | 한 호출에서 bin·타일·모든 pass·결과 조립 완료 | 레이어 pass 하나를 기존 타일에 그리는 내부 API 추출 |
| `raster.rs::raster_tile` | 호출마다 새 RasterBand/WriteOnce 생성 | 진단에서는 프레임 끝까지 같은 타일·마스크 유지 |
| `page_cache.rs::load_cancellable` | 요청받은 page ID를 중복 제거·병렬 디코드 | 그대로 재사용, 호출 목록만 줄임 |

기존 일반 렌더 API의 동작은 유지한다. 공통 루틴 추출 뒤 기존 게이트로 회귀를 확인한다.

## 4. 메타데이터 장면과 작업 목록

`render-core`에 읽기 전용 `PreparedLayerPlan`을 둔다(이름은 제안).

- `Arc<HierPlan>`과 frozen selected, 원래 paint 순서의 스타일을 보관한다.
- page ID별 `layer_idx`, cell-local bbox, csize, usize, records를 OVM 디렉터리에서 읽는다.
  `Cache`에 범위를 검사하는 메타 조회 API를 추가하며 OVP payload는 읽지 않는다.
- 셀 bbox·하위 레이어 마스크·레이어별 selected 목록은 한 번 계산한다.
  이 마스크는 **selected 페이지의 메타**로 만들고 아직 디코드하지 않은 페이지도 포함한다.
  현재 `SceneMasks`를 빈 decoded map으로 만들면 실제 내용까지 프루닝되므로 재사용해서는 안 된다.
- 기존 collector의 bbox/layer 조회를 작은 어댑터로 분리한다. 기존 렌더는 decoded page로,
  진단은 OVM page meta로 같은 계층/배열/변환 규칙을 사용한다.
- bin은 가능하면 프레임당 한 번 만든다. `(셀 인스턴스, 변환, 레이어)` 항목을 유지하고,
  `(페이지 × 인스턴스 × 타일)` 목록으로 펼치지 않는다. 기존 item cap과 Deferred 구조를 유지한다.
- 세 모드의 공정한 비교를 위해 baseline은 기존 bin 경로를 사용하고,
  ordered/occlusion의 공통 메타 준비 비용은 따로 계측한다.

## 5. 프레임을 유지하는 래스터 API

`raster.rs` 안에 `LayerRasterSession`을 둔다(이름은 제안).
세부 WriteOnce 비트를 renderd가 직접 수정하도록 공개하지 않는다.

```text
begin(request, styles, prepared_plan)
coverage_snapshot()                    // 완료된 상위 레이어의 마스크, 읽기 전용
paint_layer(layer, decoded_lookup)      // 현재 타일 버퍼에 이어 그림
finish()                               // 한 번만 RGBA 프레임 조립
```

- 타일 분할은 시작할 때 고정한다. 레이어마다 타일 크기나 전역 픽셀 원점이 바뀌지 않는다.
- 같은 좌표 변환·stroke·speckle/pattern 위상·record cut·write-once 코드를 사용한다.
- styled.layers의 실제 paint 순서를 역순으로 처리한다. 레이어 번호나 page ID를 순서로 쓰지 않는다.
- 한 레이어 안의 실제 도형/wash/sub-cut box는 기존 DFS 순서로 재생한다.
  box만 있는 레이어도 pass를 실행하고 다음 레이어의 가림에 반영한다.
- 모든 타일의 L pass가 끝나면 그 마스크를 다음 레이어의 판정에 사용한다.
  판정과 쓰기를 동시에 하지 않으므로 작업자 실행 순서에 따라 생략 집합이 달라지지 않는다.
- 래스터 작업자는 타일을 병렬 처리한다. 가능하면 작업자 풀을 프레임 동안 유지한다.
  기존 디코드 함수가 호출마다 작업자를 만드는 비용도 전체 시간 및 별도 일정 관리 비용에 포함한다.
- 이미 찬 타일은 이후 그리기 pass를 받지 않는다. `occlusion`은 화면 전체가 차면 남은 레이어를
  디코드하지 않고 끝낸다. 대조군 `ordered`는 그 경우에도 남은 selected를 전부 로드해야 한다.
  그렇지 않으면 순서 변경 비용과 가림에 의한 절약을 분리할 수 없다.
- 매 레이어 전체 `FrameScene`/SceneMasks를 다시 만들거나 이전 레이어를 다시 그리지 않는다.
  실제 decoded 페이지 map만 갱신하고 공통 메타 장면은 공유한다.

## 6. 디코드 전 생략 판정

레이어 L 시작 시의 고정된 마스크로 `demand_pages(L, coverage)`를 실행한다.
필요 집합은 page ID 단위 OR다.

```text
needed[p] = 그 페이지의 어느 가시 인스턴스라도 열린 픽셀에 닿을 가능성이 있음
          또는 탐색을 끝내지 못해 가능성을 배제할 수 없음
```

1. 레이어의 bin 항목을 방문한다. 셀/배열의 전체 화면 범위가 가려졌으면 그 항목을 건너뛴다.
2. 일반 Cell 항목은 그 레이어의 selected 페이지 메타만 본다.
3. 페이지 bbox를 인스턴스 변환으로 화면에 옮기고 현재 래스터와 같은 stroke/반올림 여유를 붙인다.
   가능한 모든 타일의 픽셀이 이미 쓰였을 때만 그 인스턴스를 가려졌다고 판단한다.
4. 한 인스턴스라도 열린 곳에 닿으면 page ID를 needed에 넣는다. 그 페이지의 다른 인스턴스에 대한
   추가 필요성 검사는 생략할 수 있다. 실제 래스터는 그 페이지의 모든 가시 인스턴스를 정상 처리한다.
5. 레이어의 모든 후보를 처리한 뒤에만 `selected[L] - needed`를 생략 확정한다.
   첫 인스턴스가 가려졌다는 이유로 공유 페이지를 생략해서는 안 된다.

마스크는 배경색 비교가 아니라 **실제 write-once 비트**를 사용한다. 검은색 solid도 가림이고,
speckle/clear/pattern의 쓰지 않은 구멍은 가림이 아니다. `world_box_written`의 좌표·여유 계산을
공유한다. 완전히 찬 블록을 빠른 판정에 사용할 수 있지만 90%·99% 채움으로 생략하지 않는다.
블록 경계의 판정이 불확실하면 정확한 픽셀 비트를 확인하거나 needed로 둔다.

큰 Deferred 배열은 기존의 view 제한 열거를 쓰되 진단 자체가 무제한 멤버 순회가 되지 않도록
방문 예산을 둔다. 전체 범위의 가림이 증명되면 생략하고, 증명되지 않은 채 예산에 닿으면
**그 미해결 subtree의 해당 레이어 페이지를 모두 needed로 둔다.** 이는 셀 정의 DAG에서 모으며
인스턴스를 펼치지 않는다. 이 목록도 상한을 넘으면 해당 레이어 전체를 needed로 둔다.
예산 초과 시 도형을 버리거나 bbox로 대체하지 않는다. 실험의 그림은 언제나 같다.
오류·취소는 정상 전파하고, 탐색 예산 소진만 보수적인 needed 처리로 복구한다.

`ordered`는 이 가림 조회를 하지 않고 selected[L] 전부를 로드한다.
`occlusion`도 같은 레이어의 미래 wash/도형을 가림으로 예측하지 않는다.

## 7. 레이어 드라이버와 수명

`render-core/src/layer_decode.rs`에 진단용 드라이버를 두는 구성을 권한다.

```text
plan, selected = 기존 요청으로 한 번 생성/고정
prepared = prepare_metadata(plan, selected, styles)
session = begin(prepared)
loaded = 빈 decoded page map

for L in paint 순서의 역순:
    if mode == ordered:
        ids = selected[L]
    else:
        ids = demand_pages(L, session.coverage_snapshot())
    pages = page_cache.load_cancellable(ids, decode_workers)
    기존 generation budget 검사
    loaded에 추가
    session.paint_layer(L, loaded)

frame = session.finish()
진단 프레임과 계측 저장
```

첫 검증에서는 실제 로드한 decoded 참조를 프레임 끝까지 유지해 기존 generation budget 의미를
보존한다. 생략 페이지는 읽지 않지만 **처리한 레이어의 즉시 해제에 의한 메모리 절약은 이번 실험의
성과에 포함하지 않는다.** 예산 검사 제거·페이지당 가짜 크기·레이어별 예산 리셋을 하지 않는다.
선택된 전체가 기존 경로에서 예산을 넘는 경우, 해당 모드 실패를 기록하고 공통 상향 예산에서도
비교한다. baseline 실패를 속도 수치로 환산하지 않는다.

상태는 `loaded`, `occluded/no_open`, `explicitly_deferred`를 구분한다. 현재 FrameScene은
plan.pages 중 미디코드 전부를 deferred로 세므로 이를 그대로 결과 상태에 쓰지 않는다.
가림이 증명된 페이지 때문에 partial이 켜져서는 안 되며, 기존 decode_pages 제한으로 빠진 것은
여전히 partial이다. 진단 프레임 완료와 query용 완전 장면은 서로 다른 개념이다.

## 8. 계측과 결과 해석

레이어별 상세 결과는 JSON에, 프레임 요약은 한 줄에 출력한다. page ID별 상세 로그는 선택 옵션이다.

| 분류 | 기록할 값 |
|---|---|
| 동일성 | 버전/캐시 identity, 요청·스타일·plan·selected 지문, shape_cut, fit_* |
| 후보 | plan pages, selected unique pages, 레이어별 selected |
| 생략 | no-open unique pages, 전부 찬 타일/남은 레이어, 보수적 needed 수·사유 |
| 실제 디코드 | 요청 page IDs, cache hit/miss, 고유 miss pages, 로드한 페이지별 estimated_bytes 계정값 |
| 예상 절약량 | 생략 페이지의 csize/usize/records 및 page_memory 추정치 — actual과 구분 |
| 작업량 | probe 셀/인스턴스/페이지/bbox 검사 수, Deferred 방문, 예산 폴백 횟수 |
| 시간 | plan, metadata/bin, probe, read, decode wall/sum/max, scene 준비, raster, 대기/일정 관리, total |
| 메모리 | 새 메타·bin·mask 바이트, held decoded 참조의 고유 계정 바이트, page-cache resident, 프로세스 peak RSS |

- `usize`는 디코드 후 Rust 객체의 메모리가 아니다. 가림 페이지는 읽지 않았으므로 로드 후
  계정값을 안다고 표시하지 않는다. baseline에서 기록한 page별 `estimated_bytes`와 사후 대조한다.
  이 값도 메모리 예산을 위한 추정 계정이며 실제 allocator 사용량과 동일하다고 주장하지 않는다.
- 캐시에 이미 있던 페이지를 생략한 것은 새 디코드를 절약한 것과 다르다. hit 생략과 miss 생략을
  나눠 보고하고, 생략 검사는 캐시를 touch하거나 payload를 미리 읽지 않는다.
- no-open은 보수적인 plan이 고른 화면 밖 후보도 포함할 수 있다. 가능한 경우 사유를 구분하며
  전부를 상위 레이어 가림 효과로 부풀리지 않는다.
- 프레임에서 pixel을 못 썼다는 사후 결과만으로 사전 생략 가능하다고 계산하지 않는다.
- `baseline → ordered`는 순서 변경의 부가 비용, `ordered → occlusion`은 생략의 순이익,
  `baseline → occlusion`은 사용자가 체감할 최종 차이다. 새 메타/탐색 비용을 draw 밖에 숨기지 않는다.
- 디코드 wall은 레이어별 wall의 합, decode sum은 작업자 시간의 합으로 구분한다.

## 9. 검증과 벤치마크

먼저 작은 결정적 fixture로 다음을 고정한다. 세 모드의 최종 RGBA는 바이트까지 같아야 한다.

1. 위 solid가 아래 페이지를 완전히 가림: 아래 page ID가 실제 디코드 호출에 없어야 한다.
2. 위 speckle/clear/pattern의 구멍으로 아래가 보임: 필요한 페이지를 생략하지 않는다.
3. 같은 페이지 두 인스턴스 중 하나만 가림, 회전·반사·Grid/Pts 반복, 서로 다른 타일 경계.
4. bbox는 겹치지만 도형은 성긴 경우, 외곽선 폭 1~8, 음수·분수 원점, 한 픽셀만 열린 경우.
5. wash/sub-cut box만 있는 위 레이어, 렌더 색이 배경과 같은 solid, mono, 레이어 순서 반전.
6. 큰 Deferred/조회 예산 초과: 미해결 페이지는 로드하며 그림이 바뀌지 않는다.
7. shape_cut 및 fit_thin이 실제 적용되는 요청: 세 모드의 계획/선택 집합과 그림이 같다.
8. jobs 1/4/12, tile 64/128/384에서 동일 픽셀. 취소 후 새 요청에 이전 마스크가 남지 않는다.
9. cold/warm 캐시에서 hit/miss·생략 수의 의미, decoded 중복 로드 없음, 명시적 partial 유지.

관련 기존 게이트: unit_render, write_once, shape_cut, fit_budget, render_speckle,
render_goldens. 원래 API에서 frames/labels 동작이 바뀌지 않았는지는 render_frames와 기존
write_once 게이트로 확인한다. 새 gate는 `validate_rust.sh`에 등록하되 기본 경로 변경은 하지 않는다.

성능 측정은 단계적으로 한다.

- 우선 `main01_chip_p10.oas`, 1920×1080, keep, medium, depth 0/full,
  레이어 1/16/64/449, fit/×4/×8. 각 레이어 집합·paint 순서를 파일로 고정한다.
- 이득/퇴행 사례에 ×2/×16, 다른 depth 및 high를 추가한다. 109/2 단독도 성긴 대조군으로 포함한다.
- 렌더러 캐시 cold는 새 워커, warm은 각 모드의 같은 요청 재실행으로 정의한다.
  OS 파일 캐시까지 cold라고 부르지 않는다. 전역 purge는 하지 않는다.
- 모드별 독립 워커를 사용하고 측정 순서를 교차한다. 바이너리 최초 실행 검사를 예열에서 제외하고,
  반복 측정 중앙값과 범위를 함께 기록한다. 페이지 절약률로 시간 절약률을 추정하지 않는다.
- 합성 결과 뒤 MAIN01/MAIN09를 같은 도구로 측정한다. 합성 결과를 실칩 결과로 보고하지 않는다.

완료 조건: 픽셀/계획 계약 통과, 완전 가림 fixture에서 실제 디코드 생략 확인, 모든 새 비용을
포함한 세 모드 비교표 작성. 모든 뷰가 빨라야 실험 성공인 것은 아니다. 일부 뷰가 느리면 그 비용을
보여 주는 것이 이 단계의 결과다. GUI 기본 적용/다음 플래너 변경은 이 결과로 별도 판단한다.

## 10. 구현 순서와 변경 파일

1. **공통 pass 추출 + ordered 모드**: raster 세션으로 마스크/버퍼 유지. 전체 selected를 그대로
   디코드하고 baseline과 픽셀 일치부터 확인한다.
2. **메타 조회와 demand probe**: decoded 없이 selected 레이어/페이지를 찾고 공유 인스턴스 OR,
   bbox 가림, Deferred 보수적 폴백을 단위 테스트한다.
3. **occlusion 연결 + 계측**: 필요한 page ID만 기존 page cache에 요청하고 실제 miss 절약을 검증한다.
4. **합성 벤치마크와 결과 기록**: 계획 시간은 그대로임을 명시하고 decode 절약과 추가 비용을 분리한다.

| 파일 | 예정 변경 |
|---|---|
| `rust/render-core/src/raster.rs` | 유지형 타일 세션, 한 레이어 pass, 읽기 전용 coverage 조회, 메타 기반 collector |
| `rust/render-core/src/scene.rs` | 공통 plan 메타와 decoded 가용성 분리, 반복 전체 mask 생성 방지 |
| `rust/render-core/src/cache.rs` | 검증된 page meta 조회 API |
| `rust/render-core/src/layer_decode.rs` (신규) | 진단 드라이버, 필요 page 집합, 보수적 폴백, 보고서 |
| `rust/render-core/src/lib.rs`, `stats.rs` | 진단 API와 계측 타입 연결 |
| `rust/renderd/src/main.rs` | 동일 요청 변환을 쓰는 render_probe, 최종 진단 응답만 발행 |
| `tools/validate_layer_decode.py` (신규) | 픽셀·생략·공유·폴백·취소 gate |
| `tools/bench_layer_decode.py` (신규) | 세 모드 요청 재생, raw 비교와 JSON/표 보고 |
| `tools/validate_rust.sh`, `docs/SPEC-VALIDATION.ko.md` | 새 gate 등록 |
| `docs/FLOE2_OPTIMIZATION.ko.md` | F2R-28 계측에 실제 결과와 다음 단계 판정 추가 |

이번 실험에서는 `rust/vfs/src/hier.rs`의 선택 정책, 인덱스 형식, 밀도 표현, 레이어 상한을 바꾸지 않는다.

## 11. 1단계 결과 (0.12.175, 계측 보정과 블록 실행 0.12.176, 2026-09-20)

픽셀 계약은 통과했다. 단위 테스트(`write_once_frames_match_the_ordered_overwrite_byte_for_byte`에
붙였다)에서 36개 장면 × bin/walk × write-once on/off × 블록 1·3·99의 세션 프레임이 기존 경로와 바이트
동일하고, write-once가 건너뛴 타일·pass·항목 수도 같다. 게이트는 §9의 1~5·8과 블록 크기·published
scene·occlusion 거절을 덮는다(6·7·9는 2·3단계에서).

**계측 보정 2건(리뷰 2026-09-20).** 처음 표는 두 가지가 틀렸고, 고친 뒤 결론이 바뀌었다.
1. baseline은 bin·타일 준비를 자기 렌더 호출 안에서 하므로 그 시간이 `paint_us`에 들어가는데,
   세션은 `prepare_us`로 분리한다. paint끼리 비교하면 다른 것을 비교하게 된다 → 이제 두 모드 모두
   **`prepare_us + paint_us`(=raster)**로 본다. 이 보정으로 "16 레이어에서 3배 빨라진다"와 그 캐시
   해석은 **철회한다**(179 대 184 ms로 사실상 같다).
2. 워커가 모든 뷰에 재사용돼 뒤쪽 뷰가 앞 뷰의 페이지 캐시를 물려받았다 → 이제 **(뷰, 모드)마다 새
   워커**가 cold이고 같은 워커의 재실행이 warm이다. 프로세스 최초 실행은 예열로 뺀다.

**블록 실행(0.12.176).** 실제 paint 순서의 연속 N개 pass가 한 블록이다. 작업자는 타일 하나를 맡아 그
블록의 레이어들을 위에서 아래로 연속해서 그리고, **블록 끝에서만** 모든 작업자가 만나 드라이버가
마스크를 다시 본다. 타일 안의 순서가 그대로라 `raster_tile_pass`를 그대로 쓰고 그림은 블록 크기와
무관하게 같다. `render_probe ... block=N`(기본 1), `block >= pass 수`는 일반 렌더의 일정과 같다.
블록 안의 위 레이어가 만들 가림은 생략에 쓸 수 없다 — 더 읽는 대신 장벽이 1/N이다.

**순서와 블록의 값**(합성 칩 1/10, 1920×1080, keep, medium, frames·labels 끔, 래스터 워커 4,
cold raster ms = prepare + paint):

| 레이어 | 뷰 | baseline | ordered:1 | ordered:4 | ordered:8 |
|---|---|---|---|---|---|
| 1 | fit / ×4 / ×8 | 54 / 70 / 31 | 61 / 74 / 43 | 51 / 64 / 39 | 57 / 65 / 32 |
| 16 | fit / ×4 / ×8 | 179 / 118 / 73 | 184 / 141 / 106 | 181 / 124 / 93 | 183 / 131 / 90 |
| 64 | fit / ×4 / ×8 | 67 / 118 / 28 | 86 / 169 / 52 | 85 / 178 / 44 | 79 / 165 / 42 |
| 449 | fit / ×4 / ×8 | 180 / 286 / 27 | 273 / 383 / 61 | 254 / 331 / 46 | 228 / 328 / 41 |

- 1·16 레이어: 순서를 바꿔도 사실상 같다(±5 %, ×8만 +30 ms).
- 64·449 레이어: block 1은 +28~126 %. 장벽이 타일 간 부하 분산을 없애기 때문이다(프레임 시간이
  가장 긴 타일에서 블록마다 가장 긴 타일의 합으로 바뀐다). 워커 1개면 두 모드가 같다.
- 블록은 그 초과분을 대략 절반으로 줄인다(449 fit +93 → +48 ms, ×4 +97 → +42, ×8 +34 → +14).
  1/N로는 줄지 않는다: 4 → 8의 이득은 이미 작다.
- 3단계가 아낄 수 있는 디코드는 **상한**이 106~260 ms다(F2R-28: 그중 92~99 %가 한 픽셀도 못 쓴다).
  픽셀을 못 쓴 페이지라도 bbox가 열린 픽셀에 걸치면 사전에 생략할 수 없으므로 실제 생략량은
  3단계에서 재야 한다. 현재 장벽 비용(449 레이어 block 8에서 +14~48 ms)은 그 상한의 일부다.

**다음 순서**(사용자 지시): 계측 보정 → 블록 실행과 픽셀 일치(여기까지 완료) → 메타 조회 어댑터
(메인 bin·미니 bin·하위 레이어 마스크가 미디코드 페이지를 알아야 한다) → 같은 N의 ordered와
occlusion을 짝지어 실제 생략 비교. 블록 크기는 N=1·4·8로 비교한 뒤 결과에 따라 예상 디코드량으로
제한하는 방식을 검토한다(처음부터 적응형으로 하지 않는다).
