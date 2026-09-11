# 점유 요약(occupancy summary) 구현 계획 — 마스크 정책의 광역뷰

작성 2026-09-11. 근거는 JOBDECK.ko.md §10 실측 6·7과 리뷰(2026-09-11).
착수 여부는 사용자 결정이며, 이 문서는 그 결정과 착수 뒤의 작업 순서를 위한
것이다.

## 0. 한 줄 요약

마스크 정책(`thin=keep`)의 광역뷰에서는 페이지 디코드·raster 대신 **소스·레이어별
점유 비트맵 피라미드**(셀 ≤ 화면 1 px인 레벨)를 그린다. 근접뷰는 지금처럼 exact
keep. 일반 레이아웃 정책(`thin=cull`)은 바꾸지 않는다.

## 1. 왜 (실측)

| 실측 | 값 | 뜻 |
|---|---|---|
| 6: 덱 fit 뷰(1 px = 202 µm), keep | 32 s/프레임 (thin 25k 페이지 디코드 합 30.9 s, hairline 4,170만 paints 22.2 s) | keep 정책은 광역뷰에서 그대로 쓸 수 없다 |
| 7: 문제 영역 26 × 33 mm, 레이어 하나, fit keep 렌더 | 예산 초과로 거부(1.93 GB > 1 GiB) | exact keep으로는 광역뷰를 그릴 수 없는 영역이 실재 |
| 7: 같은 영역 fit cull 렌더 | 0.21 s, 81 px | 일반 정책은 점유 11 %의 영역을 빈 화면으로 보여 준다 |
| 7: 점유 요약 생성(4 µm exact 타일 렌더 16장) | 4.3 s | 색인 시 생성 비용의 대리값 |
| 7: 저장 | level 0(4 µm) 6.5 MB, fit 레벨(32 µm) 101 KB; 칩 전체 레이어당 약 15 MB | 감당 가능 |
| 7: 광역뷰 페인트 | fit 레벨 점유 9.3만 셀 | ms 단위 |
| 7: 굵은 셀의 과대 표시(기준 1 px 셀 92,710 px) | 2 px +16 %(jaccard 0.860), 4 px +39 %(0.722), 8 px +75 %(0.570) | **셀은 화면 1 px 이하** |

## 2. 목표 / 비목표

목표
- keep 정책 광역뷰의 프레임 시간을 페이지 수와 무관하게(점유 셀 수에 비례) 만든다.
- 빈 공간의 **위치**를 화면 해상도에서 보존한다(1 px 미만의 빈 공간만 잃는다).
- 색인 시 한 번 만들고, 뷰어·`floe2 render`·덱이 같은 데이터를 쓴다.
- 요약이 없는 캐시(구 색인)에서는 지금 동작(exact keep, 예산 초과 시 명시 거부)을
  유지하고 상태줄에 요약 없음을 표시한다.

비목표(이번 범위 밖)
- 일반 레이아웃 정책(cull)의 광역뷰 변경. 같은 데이터로 나중에 적용할 수 있지만
  별도 결정.
- bbox 채움, bbox당 밀도 숫자, 옛 `design.ovc`(512 해상도 밀도 float) 복구. 빈 공간
  위치를 잃으므로 제외(리뷰).
- 크기 cut(양변 < cut)·자식 셀/BVH hairline 규칙 변경, LOD 재설계, hair 모드.
- pick/snap/clip이 요약을 대상으로 동작하는 것(요약은 표시 전용; 근접뷰에서
  exact로 전환하면 기존대로).

## 3. 표시 계약(오차 기준)

- 화면에 쓰는 요약 레벨은 **셀 ≤ 1 px**인 가장 굵은 레벨. 2배 피라미드이므로 셀은
  0.5~1 px. 과대 표시는 1 px 미만 빈 공간에 한정된다(실측 7: 2 px 셀부터 +16 %).
- 셀 점유 판정은 "그 셀과 교차하는 도형이 하나라도 있음". rect는 정확, polygon/
  path는 bbox로 판정한다(LOD와 같은 ≤ 1셀 과대 계약; 후속에서 정확 raster로
  바꿀 수 있음).
- 요약은 레이어 색·채움(speckle 포함)으로 셀 렉트를 칠한다. 셀이 1 px 미만이면
  raster의 hairline parity가 한 픽셀에 찍는다.
- 뷰의 µm/px가 기준 셀보다 작은 근접뷰는 exact keep(현행). 그 경계에서 exact가
  예산을 넘으면 현행대로 명시 거부(빈 화면을 정상 완료로 돌려주지 않는다 — 리뷰).
- 프레임(계층 경계 박스), 라벨, 다른 레이어의 exact 렌더는 그대로 합성된다.

## 4. 데이터: `design.ovo`

캐시 디렉터리(`<src>.floe/`)에 `design.ovo` 하나. 소스 좌표계(world dbu, 최상위 셀
기준으로 계층을 평탄화)에서 레이어마다 피라미드.

```
header   magic "FLOEOVO1", version, unit(dbu/µm), n_layers, base_cell_dbu,
         world bbox(x0,y0,x1,y1 dbu), n_levels, layer 테이블 오프셋
layer k  (layer, dt) = `Doc.layer_order[k]`(ovm 레이어 테이블과 같은 순서)
level L  cell = base_cell_dbu × 2^L, grid (w, h) = ceil(span/cell),
         비트맵 row-major, 행은 8비트 패딩, 원점 = world bbox x0/y0
         (level 0의 셀 (i, j) = [x0 + i·cell, x0 + (i+1)·cell) × [y0 + j·cell, …))
```

- 기준 셀: 옵션 `--occupancy-um`(기본 4 µm). 근접뷰 경계(800 px 창에서 3.2 mm 뷰)와
  저장량(칩 전체 레이어당 약 15 MB)의 절충. 2 µm면 4배.
- 레벨 수: 격자가 64 × 64 셀 이하가 될 때까지(35.8 mm·4 µm에서 9레벨, 4 µm~1 mm).
  상위 레벨은 하위의 OR-풀링이라 손실 없이 재생성 가능하지만, 디스크가 싸므로
  저장한다.
- 크기: 35.8 mm 칩, 4 µm → 레이어당 level 0 약 9.5 MB(비트), 피라미드 합 약 13 MB.
  레이어 20개면 260 MB. 마스크 소스는 보통 레이어 수가 적다.
- 메타(`meta.json`)에 `occupancy: {base_um, levels, layers}`를 적어 뷰어·CLI가 유무를
  안다.

## 5. 생성기(`floe-index vfs`)

- 입력: 파싱된 `Doc`(셀·레코드·배치·반복, `Doc.layer_order`). 기존 `coverage.rs`의
  순회 골격(`SplatCtx`, `world_bbox`, `splat_place`의 배치 재귀와 `tiler::Xf` 합성,
  `grow_rep`)만 재사용하고, 밀도 폴딩(`splat_uniform_cell`, `recursive_area`의 면적
  추정)은 쓰지 않는다.
- 평탄화: top에서 재귀. 배치 변환(회전·미러 포함)을 합성해 레코드 bbox를 world로
  옮긴 뒤 level 0 셀에 마킹. 회전이 90° 배수이므로 bbox 변환은 정확하다.
- 반복: Grid는 멤버마다 마킹하되 멤버 bbox가 셀보다 작고 피치가 셀 이하면 footprint
  전체를 한 번에 마킹(닫힌형, 결과 동일). Pts는 점마다. 멤버 수가 `pts_full_rep`를
  넘는 Pts는 footprint bbox 마킹(과대, 카운터로 보고 — LOD·프레임과 같은 가드).
- polygon/path: bbox 마킹(§3 계약). 통계에 `occ_poly_bbox` 수를 남긴다.
- 피라미드: level 0 완성 뒤 OR-풀링.
- 병렬: 레이어별 스레드(비트맵이 독립). 메모리: 레이어당 level 0 비트맵 ≈ 9.5 MB
  (35 mm/4 µm) + 상위 레벨 1/3.
- 시간 상한: 실측 7의 exact 렌더 4.3 s(영역)와 같은 자릿수. 150M 배치 급 칩은
  배치 순회가 지배하므로 LOD 패스와 같은 로그·타이머(`occ/Nt`)를 둔다.
- 옵션: `--occupancy-um F`(기본 4), `--no-occupancy`(생성 생략). `floe2 index`는
  기본 생성(비용이 초 단위이면), `--no-occupancy`로 끔. jobdeck·뷰어 인덱싱 경로는
  래퍼를 그대로 타므로 자동 적용. 기존 캐시에 추가만 하는 `--occupancy-only`
  (`--coverage-only` 전례).

## 6. 선택 규칙(플래너·renderd)

요청 단위로 정해진다(2026-09-11 정책 분리와 같은 원칙).

1. 조건: 요청 정책이 `thin=keep`이고, `design.ovo`가 있고, 킬 스위치가 아니며,
   뷰의 µm/px ≥ base cell(= level 0 셀이 1 px 이하).
2. 레벨: 셀 ≤ 1 px인 가장 굵은 레벨 L. 덱은 소스 뷰 기준(덱 µm/px ÷ scale).
3. 플랜: 요약으로 그릴 레이어를 `vis` 마스크에서 뺀 채 기존 플랜을 돈다 → 그
   레이어의 페이지 선택·페이지 BVH·자식 순회가 생략된다. 프레임(r == 0)은 레이어와
   무관하므로 그대로. 나머지 레이어는 exact 경로 그대로.
4. renderd: 요약 pass — 뷰와 교차하는 level L 셀을 레이어 스타일로 rect 페인트.
   전체 프레임 사상·타일 격자·채움 위상은 geometry pass와 같은
   `render_geometry_styled` 계열을 쓴다(셀 렉트를 레코드처럼 넘겨 hairline parity·
   speckle·선폭을 공유).
   순서: geometry pass 뒤, 프레임 over plane 앞(레이어 순서는 out 순).
5. 근접뷰(µm/px < base cell): 현행 exact keep. 예산 초과는 현행 거부.
6. 프레임 줄·상태줄: `summary layers L cells N (level k, cell x um)`, 요약 없는
   캐시에서 keep 광역뷰면 `summary: none (index without --occupancy)`.
7. 킬 스위치 `FLOE_RUST_OCCUPANCY=off`(요약 무시 → 현행), `floe-index plan
   --explain`에 verdict `summary`(레이어 단위).

## 7. 덱 통합

- 소스마다 자기 `design.ovo`. 배치의 소스 뷰(`(v − d)/scale`)에서 레벨을 고르고,
  요약 pass의 출력은 지금의 geometry pass처럼 창 크기 프레임으로 배치 순서에
  합성된다(step 2·3의 서브윈도·묶음·스트리밍 구조 그대로, 요약 pass는 디코드가
  없으므로 스트리밍 대상이 아니다).
- 4단계 sub-cut wash와의 관계: 요약이 그리는 레이어는 sub-cut wash를 내지 않는다
  (요약이 더 정확한 존재 표시). 요약이 없는 소스는 현행 wash.
- 카운터: 덱 프레임 줄에 pass별 요약 여부를 합산(`summary_passes`, `summary_cells`).

## 8. 검증(gate)

1. 생성 정확성: fixture(thin.oas, thinmix.oas, hier/frames, 회전·미러 배치, Grid·
   Pts 반복, polygon/path)에서 `design.ovo` level 0 == KLayout(Python)으로 계산한 셀
   점유(오라클: 셀마다 도형 교차 여부; polygon/path는 bbox 교차). 상위 레벨 ==
   하위의 OR-풀링.
2. 렌더 정확성: 요약 pass 픽셀 == 같은 배율에서 level L 비트맵을 직접 그린 기대
   이미지(Python). 셀 ≤ 1 px 레벨 선택이 배율마다 맞는지.
3. 정책 불변: `thin=cull` 픽셀 불변(A/B), keep 근접뷰 픽셀 불변(exact), keep 광역뷰는
   요약(요약 없는 캐시에서는 현행과 동일).
4. 덱: 합성 순서·서브윈도·레벨 선택(scale 반영), 요약 레이어의 sub-cut wash 없음.
5. 킬 스위치 A/B, `--explain` verdict, perf 카운터 파싱.
6. 실칩(실측 8로 기록): 문제 영역과 덱 fit 뷰의 프레임 시간·카운터, 실측 7의 요약
   이미지와 화면 비교. 목표: fit 뷰 프레임 1 s 이하, 화면은 실측 7의 fit 레벨과
   일치.

## 9. 단계

| 단계 | 내용 | 판정 |
|---|---|---|
| M1 | `design.ovo` 형식·생성기·`--occupancy-um`·메타, gate 1 | fixture 오라클 일치, 실칩 생성 시간·크기 기록 |
| M2 | renderd 요약 pass(단일 소스), 레벨 선택, 킬 스위치, gate 2·3·5 | 단일 소스 keep 광역뷰가 요약으로 그려짐, cull·근접뷰 불변 |
| M3 | 플래너에서 요약 레이어의 페이지·계층 생략, `--explain summary`, 카운터 | 광역뷰 플랜의 페이지 선택·cbvh가 요약 레이어에서 0 |
| M4 | 덱 통합(소스 뷰 레벨, 합성, wash 억제), gate 4 | 덱 fit 뷰 시간 |
| M5 | 실칩 실측 8, base cell·기본값 확정, 문서(JOBDECK §10·FLOE2_OPTIMIZATION 결함 B) | 목표 시간 달성 여부로 기본 on/off 결정 |

각 단계는 킬 스위치와 gate를 갖추고 배터리 통과 뒤 커밋한다. 버전: Rust 변경 단계는
RENDERD_VERSION, 색인 형식 추가는 캐시 버전(`meta.occupancy` 없으면 요약 없음으로
동작, 캐시 자체는 호환).

## 10. 위험·미결

- polygon/path bbox 과대: 큰 비직교 다각형(실 링, 대각선 배선)은 bbox가 크게 과대
  된다. M1에서 `occ_poly_bbox` 수와 면적 비율을 실칩에서 보고, 크면 후속으로 정확
  raster(폴리곤 스캔라인) 마킹.
- 근접뷰와 요약 사이의 틈: µm/px가 base cell보다 작지만 exact가 예산을 넘는 뷰
  (예: 2 µm/px로 큰 영역). 기본 셀을 2 µm로 내리거나, 요약 level 0을 확대해 그리되
  과대 표시임을 상태줄에 표시하는 것 중 하나. 실측 8 뒤 결정.
- 생성 비용: 배치 수가 극단(150M)인 칩에서 평탄화 순회 시간. Grid 닫힌형과
  footprint 가드로 상한을 두고 타이머로 확인.
- 저장: 레이어가 많은 일반 레이아웃에서 `--no-occupancy` 기본 여부. 마스크 소스는
  레이어가 적으므로 우선 기본 on으로 두고 실측 8에서 결정.
- 일반 정책(cull)에도 요약을 쓸지: 데이터는 같으므로 가능하지만, 일반 광역뷰의
  화면이 바뀌므로 별도 결정.
- 실험 도구의 미세 렌더는 hairline parity의 y 편향을 포함하므로 도형 기준 생성기와
  1 px 정도 다를 수 있다. gate 오라클은 KLayout 도형 교차로 만든다.
