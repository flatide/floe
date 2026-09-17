# 점유 요약(occupancy summary) 구현 계획 — 마스크 정책의 광역뷰

작성 2026-09-11, 1차 계획 리뷰(7건, §11) 반영 2026-09-11. 근거는 JOBDECK.ko.md
§10 실측 6·7. 착수 여부는 사용자 결정이며, 이 문서는 그 결정과 착수 뒤의 작업
순서를 위한 것이다.

## 0. 한 줄 요약

마스크 정책(`thin=keep`)의 광역뷰에서는 페이지 디코드·raster 대신 **소스·레이어별
점유 비트맵 피라미드**(셀 ≤ 화면 1 px인 레벨)를 화면 마스크로 투영해 그린다.
점유는 색인 시 **도형 교차**로 만든다(bbox 대체 없음). 근접뷰·exact는 지금처럼
exact keep. 제한 depth는 2026-09-16(M6)부터 배치 깊이별 비트 평면으로 요약한다.
일반 레이아웃 정책(`thin=cull`)은 바꾸지 않는다.

## 1. 왜 (실측)

| 실측 | 값 | 뜻 |
|---|---|---|
| 6: 덱 fit 뷰(1 px = 202 µm), keep | 32 s/프레임 (thin 25k 페이지 디코드 합 30.9 s, hairline 4,170만 paints 22.2 s) | keep 정책은 광역뷰에서 그대로 쓸 수 없다 |
| 7: 문제 영역 26 × 33 mm, 레이어 하나, fit keep 렌더 | 예산 초과로 거부(1.93 GB > 1 GiB) | exact keep으로는 광역뷰를 그릴 수 없는 영역이 실재 |
| 7: 같은 영역 fit cull 렌더 | 0.21 s, 81 px | 일반 정책은 점유 11 %의 영역을 빈 화면으로 보여 준다 |
| 7: 점유 요약 생성(4 µm exact 타일 렌더 16장) | 4.3 s | 생성 비용의 자릿수 추정. 상한이 아니다 |
| 7: 저장 | level 0(4 µm) 6.5 MB, fit 레벨(32 µm) 101 KB; 칩 전체 레이어당 약 15 MB | 감당 가능(bbox 면적 비례 — §5 상한) |
| 7: 광역뷰 페인트 | fit 레벨 점유 9.3만 셀 | ms 단위 |
| 7: 굵은 셀의 과대 표시(기준 = 1 px 셀 요약 92,710 px) | 2 px +16 %(jaccard 0.860), 4 px +39 %(0.722), 8 px +75 %(0.570) | 셀이 굵어질수록 과대가 커진다 → 셀 ≤ 1 px은 상한. exact 대비 오차가 아니라 1 px 셀 요약 대비 상대값(§3) |

## 2. 목표 / 비목표

목표
- keep 정책 광역뷰의 프레임 시간을 페이지 수와 무관하게(점유 셀 수에 비례) 만든다.
- 빈 공간의 **위치**를 화면 해상도에서 보존한다. 보장 범위는 §3의 1 px 팽창 규칙.
- 색인 시 한 번 만들고, 뷰어·`floe2 render`·덱이 같은 데이터를 쓴다.
- 요약이 없거나 유효하지 않은 캐시에서는 지금 동작(exact keep, 예산 초과 시 명시
  거부)을 유지하고 상태줄에 요약 없음과 이유를 표시한다.

비목표(이번 범위 밖)
- 일반 레이아웃 정책(cull)의 광역뷰 변경. 같은 데이터로 나중에 적용할 수 있지만
  별도 결정.
- bbox 채움, bbox당 밀도 숫자, 옛 `design.ovc`(512 해상도 밀도 float) 복구. 빈 공간
  위치를 잃으므로 제외. **생성에서도 bbox를 점유의 대체물로 쓰지 않는다**(§5).
- 크기 cut(양변 < cut)·자식 셀/BVH hairline 규칙 변경, LOD 재설계, hair 모드.
- 요약으로 그린 레이어에 대한 pick/snap/clip. 그 뷰에서는 대상에서 제외하고 상태줄에
  표시한다(§6). 질의 영역만 원본으로 조회하는 방식은 후속.

## 3. 표시 계약(오차 기준)

- **레벨**: 화면에 쓰는 요약 레벨은 셀 ≤ 1 px인 가장 굵은 레벨. 2배 피라미드이므로
  셀은 0.5~1 px.
- **투영과 오차 범위(1 px 팽창 규칙, 2차 리뷰 P1-3로 정정)**: 점유 셀은 **셀
  중심이 놓인 픽셀 하나**를 켠다(셀이 걸친 픽셀 전부가 아니다). 이유: 도형 → 셀
  (경계 밖 최대 1셀 ≤ 1 px)에 floor/ceil 투영을 더하면, exact raster가 중심
  표본화로 경계 안쪽 0.5 px를 비워 두는 것과 겹쳐 exact 픽셀에서 2 px 떨어진
  픽셀까지 켜졌다. 셀 ≤ 1 px에서 중심 투영이면 셀 중심은 도형 경계 밖 0.5 px
  미만이므로 **요약 픽셀 ⊆ exact 픽셀의 1 px 팽창(8-이웃)**이고, exact가 켠
  픽셀의 중심은 점유 셀 안이라 **exact 픽셀 ⊆ 요약 픽셀의 1 px 팽창**이다(셀
  하나는 픽셀 하나를 반드시 켜므로 셀·가는 선이 사라지지 않고, 인접 셀 중심은
  ≤ 1 px 간격이라 선이 끊기지 않는다). 따라서 빈 영역에서 exact 픽셀과 2 px 이상
  떨어진 픽셀은 항상 비어 있고, 폭 g px 간격은 최소 g − 2 px가 남는다(**3 px
  이상은 항상 ≥ 1 px**), **2 px 간격은 위상에 따라 닫힐 수 있다**. 도형
  가장자리를 따라 나란한 빈 띠는 1 px 폭만큼 먹힐 수 있다. 이전 "2셀 규칙"(2 px
  이상 보존)은 두 번째 확장을 빼먹은 것이었다. 셀 ≤ 0.5 px 레벨(진단
  `FLOE_RUST_OCCUPANCY_PX=0.5`)은 빈도를 줄인다(페인트 4배). 실측 7의 과대
  수치는 1 px 셀 요약을 기준으로 잰 상대값이며 exact 대비 오차는 gate 2·6에서
  따로 잰다.
- **점유 판정(생성 계약, M1 필수)**: 셀 비트 = "그 셀과 교차하는 도형이 하나라도
  있음"을 도형 교차로 판정한다. rect는 셀 범위, polygon/path는 셀 격자 위 보수적
  스캔 변환(§5), 반복은 §5의 닫힌형 조건을 만족할 때만 footprint, 아니면 멤버마다.
  처리 한계를 넘는 레이어는 근사로 저장하지 않고 "요약 없음(이유)"로 기록한다.
- **적용 조건(5개)**: 요청이 `thin=keep`이고, exact 요청이 아니고, `.ovo`가 기록한
  top·소스·레이어 테이블이 현재 캐시와 일치하고, 뷰의 µm/px ≥ base cell(level 0
  셀이 1 px 이하)이고, depth 조건: **v2 파일(FLOEOVO2, 2026-09-16 M6 — 배치
  깊이별 비트 평면)이면 어떤 depth든** 요청 depth 이하의 평면들의 OR을 그린다
  (그 depth에서 페이지 경로가 그리는 도형과 정확히 같은 집합); v1 파일(전 깊이
  평탄화) 또는 `FLOE_RUST_OCCUPANCY_DEPTH=off`에서는 depth가 그 레이어를 통째로
  그리는 값(무제한, 소스 계층 높이 이상, 또는 그 레이어의 페이지를 가진 가장 깊은
  셀의 깊이 이상 — 레이어 단위, 2026-09-15)일 때만. 하나라도 아니면 현행 경로
  (리뷰 2: depth 0/1에서 깊은 자식 도형이 요약으로 보이거나 `render --detail exact
  --thin keep`이 근사로 바뀌면 안 된다 — 깊이별 평면이 전자를 정확히 만족시킨다).
- **스타일(전용 경로)**: 셀 렉트를 도형처럼 raster에 넘기지 않는다(현행 hairline
  경로는 채움 패턴을 무시하고 solid로 칠하며, 선폭 > 1이면 셀마다 외곽선을 그려
  점유가 팽창한다 — 리뷰 3). 대신 (1) 점유를 화면 **마스크**로 투영하고(픽셀 = 그
  픽셀 사각형과 교차하는 점유 셀이 있음), (2) 마스크에 스타일을 적용한다: 마스크
  경계 픽셀(4-이웃 중 비점유가 있는 픽셀)은 외곽선 색 solid, 내부 픽셀은 레이어
  채움 패턴(speckle 포함). 채움이 없는(outline-only) 스타일은 내부를 비우고 경계만
  그린다. 선폭은 마스크 경계에서 항상 1 px(선폭 설정으로 팽창하지 않음). 이
  규칙은 큰 solid 도형의 exact(채움+외곽선)와 1 px 폭 형상의 exact(solid)를 함께
  근사한다. exact와의 차이는 gate 2(b)에서 경계 1 px 이내로 잰다.
- **레이어 순서**: 요약 레이어는 전체 레이어 순서 안 자기 자리에서 그린다(요약을
  geometry 뒤에 몰아 그리면 아래 레이어 요약이 위 레이어 exact를 덮는다 — 리뷰 3).
- **근접뷰**(µm/px < base cell): 현행 exact keep. 예산 초과는 현행대로 명시 거부
  (빈 화면을 정상 완료로 돌려주지 않는다).
- 프레임(계층 경계 박스), 라벨, 다른 레이어의 exact 렌더는 그대로 합성된다.

## 4. 데이터: `design.ovo`

(계획 원문. 구현된 형식은 SPEC-FORMATS `design.ovo`, 계획과 다른 세부는 §12.)

캐시 디렉터리(`.<src>.ice/`)의 선택적 sidecar. 소스 좌표계(world dbu, 캐시의 top 셀
기준으로 전체 계층을 평탄화)에서 레이어마다 피라미드.

```
header   magic "FLOEOVO1", 형식 버전, 소스 identity(meta.src의 size·mtime),
         ovm identity(design.ovm의 size·mtime), top 셀 이름, unit(dbu/µm),
         base_cell_dbu, world bbox(x0,y0,x1,y1 dbu), n_levels, n_layers,
         layer 테이블 오프셋
layer k  (layer, dt) = `Doc.layer_order[k]`(ovm 레이어 테이블과 같은 순서),
         status(ok | none:cells | none:work | none:size), 레벨 테이블
         (레벨마다 w, h, 오프셋, 바이트 수)
level L  cell = base_cell_dbu × 2^L, grid (w, h) = ceil(span/cell),
         비트맵 row-major, 행은 8비트 패딩, 원점 = world bbox x0/y0
         (level 0의 셀 (i, j) = [x0 + i·cell, x0 + (i+1)·cell) × [y0 + j·cell, …))
```

- 기준 셀: 옵션 `--occupancy-um`(기본은 자동, 2026-09-16 — 긴 변이 2,048셀 이상이
  되는 가장 굵은 4/2/1/0.5/0.25 µm; 8 mm 초과 칩은 4 µm). 근접뷰 경계(800 px 창에서
  3.2 mm 뷰)와 저장량의 절충. 2 µm면 4배.
- 레벨 수: 격자가 64 × 64 셀 이하가 될 때까지(35.8 mm·4 µm에서 9레벨, 4 µm~1 mm).
  상위 레벨은 하위의 OR-풀링이라 재생성 가능하지만 저장한다.
- 크기: dense 비트맵은 도형 수가 아니라 **world bbox 면적**에 비례한다(멀리 떨어진
  작은 도형 둘로도 커진다 — 리뷰 7). 35.8 mm 칩, 4 µm → 레이어당 level 0 약 9.5 MB,
  피라미드 합 약 13 MB. 상한은 §5. 타일 희소 인코딩은 후속.
- **유효성(로더)**: magic·버전, 오프셋·바이트 수·`(w·h + 7)/8`의 곱을 오버플로
  검사로 확인하고 파일 길이와 대조(잘린 파일 거부), layer 테이블 == ovm 레이어
  테이블, top == 캐시 top, 소스·ovm identity == 현재 값. 하나라도 어긋나면 파일
  전체를 "요약 없음(이유)"로 다루고 렌더는 현행 경로. `meta.json`의 `occupancy`
  항목은 안내용이며 판정은 `.ovo` 헤더로 한다(불일치 시 `.ovo` 기준, 없으면 없음).
- **게시**: `design.ovo.tmp`에 쓰고 fsync 뒤 rename. 실패·취소 시 tmp를 지우고 기존
  파일을 보존한다.
- **열린 뷰어**: renderd는 요약이 필요한 요청마다 `.ovo`의 (size, mtime)을 stat해
  바뀌었으면 다시 연다(rename 게시라 열려 있던 파일은 옛 내용 그대로). `--occupancy-
  only`로 추가한 요약은 다음 프레임부터 쓰인다.
- **버전**: `CACHE_VERSION`은 올리지 않는다(`is_stale()`이 버전 불일치를 재색인
  대상으로 보므로 "구 캐시 호환"과 충돌 — 리뷰 5). `.ovo`는 자체 형식 버전으로
  관리하는 선택적 sidecar이며, 없거나 무효하면 요약 없음.

## 5. 생성기(`floe-index vfs`)

(계획 원문. 구현된 규칙·상한·명령은 SPEC-INDEXER §6.5, 계획과 다른 세부는 §12.)

- 입력: 파싱된 `Doc`(셀·레코드·배치·반복, `Doc.layer_order`). 기존 `coverage.rs`의
  순회 골격(`SplatCtx`, `world_bbox`, `splat_place`의 배치 재귀와 `tiler::Xf` 합성,
  `grow_rep`)만 재사용하고, 밀도 폴딩(`splat_uniform_cell`, `recursive_area`의 면적
  추정)은 쓰지 않는다.
- 평탄화: top에서 재귀. 배치 변환(회전·미러 포함, 90° 배수)을 합성해 레코드를 world
  로 옮긴 뒤 level 0 셀에 마킹.
- **마킹(도형 교차, 필수)**:
  - rect: 셀 범위 마킹(정확).
  - polygon(파서가 trapezoid/ctrapezoid를 폴리곤으로 확장): 셀 격자 위 보수적 스캔
    변환. (a) 모든 변이 지나는 셀을 supercover로 마킹, (b) 변이 지나지 않는 셀은
    전부 안이거나 전부 밖이므로 행마다 셀 중심 y에서 변 교차점의 parity로 내부
    셀을 채운다. 결과는 "셀과 교차하는 도형" 판정과 정확히 같다(L자·테두리·
    대각선의 빈 공간 보존).
  - path: raster와 같은 규칙(반폭·끝 처리)으로 외곽 폴리곤을 만든 뒤 polygon 마킹.
  - **bbox 마킹은 없다**(리뷰 1). 이전 계획의 polygon/path bbox·Pts footprint bbox는
    큰 L자·테두리·대각선·떨어진 집단 사이의 빈 공간을 통째로 채우므로 폐기.
- **반복**:
  - Grid 닫힌형 조건: 회전·미러 합성 뒤 두 벡터가 축 정렬이고, 각 축의 빈 간격
    (피치 − 멤버 폭) < 셀이면 footprint 마킹 == 멤버별 마킹(셀보다 좁은 간격은 셀을
    통째로 담을 수 없다). 그 외(대각 벡터 등)는 멤버마다 마킹. 리뷰 반례(셀 10,
    도형 1×1, 100×2, 벡터 (4,4)·(4,−4): 실제 81셀, bbox 1,681셀)가 gate 1 fixture.
  - Pts: 항상 멤버마다. `pts_full_rep` 같은 footprint 대체 없음. 비용은 작업 예산
    으로만 제한한다.
- **처리 한계(리뷰 7)**: 레이어당 level 0 셀 수 상한(기본 2^30 셀 = 128 MB), 레이어당
  마킹 작업 예산(기본 2^31 셀 마킹), 파일 크기 상한(기본 1 GiB). 초과한 레이어는
  `status = none:<이유>`로 기록하고 다른 레이어는 계속 만든다(근사 저장 없음). 소스
  전체가 셀 수 상한을 넘으면 `--occupancy-um`을 키우라는 안내와 함께 파일을 만들지
  않는다.
- 병렬·메모리(2026-09-14 개정): 레이어는 순서대로, 한 레이어의 마킹을 `--jobs`
  스레드가 unit(top 레코드 조각·배치 멤버 범위, 단일 배치 top은 아래로 확장)으로
  나눠 각자의 level 0에 마킹하고 OR로 합친다. 메모리 = jobs × level 0 한 장. 결과는
  스레드 수와 무관하게 바이트 동일. 이전(레이어 단위 병렬)은 작업이 있는 레이어가
  둘뿐인 실칩에서 코어 둘만 썼다(실측 8-a). 시간은 상한이 아니라 로그로 관찰한다.
- 빈 레이어(2026-09-14): 양의 면적 도형이 없는 레이어는 `empty`(비트맵 없음)로
  기록하고 렌더러는 셀 0의 요약으로 다룬다(레이어 수에 포함, 페이지 없음).
- 취소: SIGINT·상위 취소 시 tmp 삭제, 기존 `.ovo` 보존.
- **옵션·기본**: 2026-09-16 사용자 결정 — `floe2 index deck.jb`의 소스는 **기본
  on**, 레이아웃 `floe2 index chip.oas`는 **기본 off**(`--occupancy`로 켬;
  `--no-occupancy`는 둘 다 끔; `--occupancy`에 현재 캐시가 요약을 갖지 않으면
  추가만; raw `floe-index vfs`는 명시 옵션). M5 결정(2026-09-15)의 "전부 기본
  on"을 바꿨다: 일반 레이아웃은 `thin:cull`이라 요약을 쓰지 않는다.
  `--occupancy-um F`(기본 4), `--occupancy-only`(기존 캐시에 추가·교체, ovm/ovp
  불변). M1~M4 동안은 opt-in이었다.
- **jobdeck 래퍼(리뷰 6)**: `_jobdeck_index()`는 argv를 직접 구성하고 이미 색인된
  소스를 대상에서 뺀다(`floe/cli.py`). 자동 전달되지 않으므로 명시 구현: 세 옵션
  전달, `--occupancy-only`면 색인된 소스도 대상에 포함, gate에서 실행 전후 ovm/ovp
  해시 불변과 헤더 `base_cell_dbu` == 요청값 확인.

## 6. 선택 규칙(플래너·renderd)

요청 단위로 정해진다(2026-09-11 정책 분리와 같은 원칙).

1. 조건: §3의 5개(`thin=keep`, exact 아님, `.ovo` 유효·일치, µm/px ≥ base cell,
   depth) 모두 참이고 킬 스위치가 아닐 때. depth는 v2 파일이면 어떤 값이든 되고
   요청 depth 이하의 평면을 OR해 그린다(`OvoFile::level_at_depth`; 조합은 캐시의
   `(레이어, 레벨, depth)` 캐시에 남는다). v1 파일·`FLOE_RUST_OCCUPANCY_DEPTH=off`
   에서는 `Cache::depth_is_full_for`(무제한·계층 높이 이상·레이어별 최대 깊이
   이상)인 레이어만 요약되고, 하나도 안 되면 `summary: none (depth)`. 레이어
   단위로 `.ovo`의 status가 ok인 레이어만 요약, none인 레이어는 현행 경로.
2. 레벨: 셀 ≤ 1 px인 가장 굵은 레벨 L. 덱은 소스 뷰 기준(덱 µm/px ÷ scale).
3. 플랜: 요약으로 그릴 레이어를 `vis` 마스크에서 뺀 채 기존 플랜을 돈다 → 그
   레이어의 페이지 선택·페이지 BVH·자식 순회가 생략된다. 프레임(r == 0)은 레이어와
   무관하므로 그대로. 나머지 레이어는 exact 경로 그대로.
4. renderd 전용 경로: 뷰와 교차하는 level L 셀을 밴드·타일 단위로 화면 마스크에
   투영하고(기존 raster 밴드 병렬화에 맞춤), §3의 경계/내부 규칙으로 스타일을
   적용한다. 페인트 위치는 `StyledGeometryRasterRequest.layers`(칠 순서, 뒤가 덮음)
   안에서 그 레이어의 차례. 셀을 도형으로 넘기지 않는다.
5. 근접뷰(µm/px < base cell)·exact: 현행 exact keep. 예산 초과는 현행 거부. 제한
   depth는 v2 파일에서 그 depth의 평면 요약(M6).
6. pick/snap: 게시된 query scene은 요약 레이어의 도형을 갖지 않으므로(생략됨)
   found=0이 된다. 요약 레이어는 그 뷰에서 pick/snap 대상에서 제외하고 상태줄에
   `summary layers: not pickable`을 표시한다(리뷰 7).
7. 프레임 줄·상태줄: `summary layers L cells N (level k, cell x um)`, 요약 없는
   keep 광역뷰면 `summary: none (<이유>)`(파일 없음·무효·레이어 status·킬 스위치).
8. 킬 스위치 `FLOE_RUST_OCCUPANCY=off`(요약 무시 → 현행), `floe-index plan
   --explain`에 verdict `summary`(레이어 단위).

## 7. 덱 통합

- 소스마다 자기 `design.ovo`. 배치의 소스 뷰(`(v − d)/scale`)에서 레벨을 고른다.
- 덱의 pass는 배치 하나 = 레이어 하나이므로, 요약 pass가 그 배치의 geometry pass를
  대체하면 배치 순서(= 레이어 순서)가 그대로 유지된다. 출력은 지금처럼 창 크기
  프레임으로 합성된다(step 2·3의 서브윈도·묶음·스트리밍 구조 그대로, 요약 pass는
  디코드가 없으므로 스트리밍 대상이 아니다).
- 적용 조건은 pass 단위로 §3과 같다: 덱 요청이 exact(`floe2 render --detail
  exact`)면 요약 없음; depth는 pass마다 그 소스의 v2 평면으로 처리한다(M6; v1
  파일이면 full인 레이어만). 뷰어의 덱 기본 depth는 M4에서 확인.
- 4단계 sub-cut wash와의 관계: 요약이 그리는 레이어는 sub-cut wash를 내지 않는다.
  요약이 없는 소스·레이어는 현행 wash.
- 카운터: 덱 프레임 줄에 `summary_passes`, `summary_cells`.

## 8. 검증(gate)

1. **생성 오라클(독립, 도형 교차)**: KLayout(Python)에서 셀마다 `Region(cell_box) &
   layer_region`이 비어 있지 않은지(경로는 KLayout이 폴리곤화)로 level 0 비트를
   만들어 `.ovo`와 완전 일치를 본다. 빈 셀 수가 같으므로 빈 공간 보존이 곧 검증된다.
   fixture: L자, 테두리(링), 대각선 polygon·path, 멀리 떨어진 Pts 두 집단, 대각
   벡터 Grid(리뷰 반례), 축 정렬 Grid의 간격 < 셀 / ≥ 셀, 회전·미러 배치, thin.oas·
   thinmix.oas. 상위 레벨 == 하위의 OR-풀링. bbox 교차를 정답으로 쓰지 않는다.
2. **렌더**: (a) 마스크 투영 == Python 기대 마스크(셀 중심 투영: 셀 중심이 놓인
   픽셀 하나), pan 위상 0/¼/½/¾ px, 피라미드 전환 직전·직후 배율, 레벨 선택이
   배율마다 맞는지. (b) exact 대비 품질: 같은 뷰의 exact 렌더(fixture는 작아 가능)와
   비교해 1 px 이웃에도 대응 요약 픽셀이 없는 exact 픽셀 = 0, 1 px 이웃에 exact
   픽셀이 없는 요약 픽셀 = 0(요약 ⊆ exact의 1 px 팽창, 그 역도, pan 위상마다),
   3 px 이상 빈 간격의 가운데 픽셀 보존,
   큰 rect의 채움·외곽선이 exact와 경계 1 px 이내. (c) 레이어 순서: 아래 요약 + 위
   exact fixture에서 위 레이어가 보인다.
3. 정책 불변: `thin=cull` 픽셀 불변(A/B), keep 근접뷰·exact 픽셀 불변, keep
   광역뷰는 요약(요약 없는 캐시에서는 현행과 동일; depth 0/1은 M6부터 그 depth의
   평면 요약이 페이지 경로와 1 px 이내).
4. 덱: 합성 순서·서브윈도·레벨 선택(scale 반영), 요약 레이어의 sub-cut wash 없음,
   exact 덱 렌더에서 요약 없음(depth 제한은 M6부터 평면 요약).
5. 운영: 킬 스위치 A/B, `--explain` verdict, perf 카운터 파싱, 잘린 파일·헤더 불일치
   (top·레이어·identity) 거부, 취소 뒤 tmp 없음·기존 파일 보존, `--occupancy-only`
   전후 ovm/ovp 해시 불변, jobdeck 래퍼 옵션 전달, pick/snap 제외 상태줄.
6. **실칩(실측 8로 기록)**: 실측 7 이미지와의 완전 일치는 목표가 아니다(격자 4.06 vs
   4 µm, 영역 bbox와 소스 bbox의 원점 차). 지표: exact가 가능한 근접뷰에서 missed/
   extra, fit 뷰 프레임 시간(목표 1 s 이하), 3 px 이상 빈 간격 손실, 생성 시간·크기·
   상한 도달 여부.

## 9. 단계

| 단계 | 내용 | 판정 |
|---|---|---|
| M1 | `design.ovo` 형식·유효성·atomic 게시, 도형 교차 마킹(rect/polygon/path/반복), 처리 한계·취소, `--occupancy`(opt-in)·`--occupancy-um`·`--occupancy-only`, jobdeck 래퍼 전달, gate 1·5(생성 부분) | **완료 2026-09-11(RENDERD 0.12.79, §12)**: fixture·valmini 오라클 완전 일치. 실칩(26 × 33 mm 추출본 85.6 MB): 41.1 s·34 MB·4레이어 ok·상한 도달 없음(§12 실측 8-a) |
| M2 | renderd 전용 마스크 경로(단일 소스), 5개 조건, 레벨 선택, 레이어 순서, pick/snap 제외, 킬 스위치, gate 2·3·5 | **완료 2026-09-11(RENDERD 0.12.80, §12)**: 단일 소스 keep 광역뷰가 요약으로 그려짐, cull·근접뷰·exact·depth 제한·킬 스위치 픽셀 불변 |
| M3 | 플래너에서 요약 레이어의 페이지·계층 생략, `--explain summary`, 카운터 | **완료 2026-09-11(RENDERD 0.12.81, §12)**: 요약 레이어의 페이지 0, 프레임 없는 요청은 요약 전용 서브트리 프루닝(wc_cells 0) |
| M4 | 덱 통합(소스 뷰 레벨, pass 대체, wash 억제, depth/exact 조건), gate 4 | **완료 2026-09-11(RENDERD 0.12.81, §12)**: mag 0.2 덱의 fit 뷰 픽셀 == 단일 소스 요약 픽셀. 덱 fit 뷰 시간은 실칩 실측 대기 |
| M6 | 배치 깊이별 비트 평면(FLOEOVO2): 제한 depth에서도 keep이 요약을 그림, 구간 depth 대비 | **완료 2026-09-16(RENDERD 0.12.90, §12 "M6")**: 레이어마다 도형이 있는 깊이별 평면, 요청 depth 이하의 OR; 공유 atomic 평면 마킹(스레드별 복제 없음); v1 파일은 full에서만; 킬 스위치 `FLOE_RUST_OCCUPANCY_DEPTH=off`; gate(깊이별 오라클, depth 0/1 렌더, v1·손상 테이블) |
| M5 | 실칩 실측 8, base cell·기본 on/off 확정, 문서(JOBDECK §10·FLOE2_OPTIMIZATION 결함 B) | **완료 2026-09-15(§12 "M5 마감")**: 8-a·8-c 생성(추출본 4.4 s·17 MB, 덱 667소스 9.9 분·172 MB), 8-d 뷰어(150 × 103 mm 덱 뷰 16.7 s → 0.15 s, depth 무관). 결정: 색인 기본 on(`--no-occupancy`), base cell 4 µm, 마스크는 keep + detail medium. 후속: 8-b 품질 샷, charge당 비용(scan), cull에서의 요약 |

각 단계는 킬 스위치와 gate를 갖추고 배터리 통과 뒤 커밋한다. 버전: Rust 변경 단계는
RENDERD_VERSION. `CACHE_VERSION`은 불변, `.ovo`는 자체 형식 버전.

## 10. 위험·미결

- 근접뷰와 요약 사이의 틈: µm/px가 base cell보다 작지만 exact가 예산을 넘는 뷰
  (예: 2 µm/px로 큰 영역). 기본 셀을 2 µm로 내리거나, 요약 level 0을 확대해 그리되
  과대 표시임을 상태줄에 표시하는 것 중 하나. 실측 8 뒤 결정.
- 1~2 px 빈 간격의 위상 의존 손실(§3). 셀 ≤ 0.5 px 레벨을 쓰면 1 px 이상 간격이
  보장되지만 셀 수 4배. 실측 8의 지표로 결정.
- 생성 비용: 배치 수가 극단(150M)이거나 Pts 멤버가 많은 소스의 마킹 시간. 작업
  예산 초과는 요약 없음으로 귀결되므로 안전하지만, 그 소스는 광역뷰 개선을 못 받는다.
- 저장: dense 비트맵의 bbox 면적 비례. 희소 소스는 셀 수 상한에 걸릴 수 있어 타일
  희소 인코딩이 후속 후보.
- 스타일 근사(§3 경계/내부 규칙)가 exact와 눈에 띄게 다를 수 있다(특히 채움 패턴이
  성긴 레이어의 큰 영역). gate 2(b)와 실칩 화면으로 확인.
- 일반 정책(cull)에도 요약을 쓸지: 데이터는 같으므로 가능하지만, 일반 광역뷰의
  화면이 바뀌므로 별도 결정.
- 실험 도구의 미세 렌더는 hairline parity의 y 편향을 포함하므로 도형 기준 생성기와
  1 px 정도 다를 수 있다. gate 오라클은 KLayout 도형 교차로 만든다.

## 12. M1 구현 기록 (2026-09-11, RENDERD 0.12.79)

구현: `rust/vfs/src/occupancy.rs`(생성기·직렬화·로더·유효성), `floe_tiler::
path_outline_any`(raster hull의 이식, render-core 테스트가 parity 고정),
`floe-index vfs --occupancy | --occupancy-only [--occupancy-um F]`, `floe-index
occupancy <cache>`, `floe2 index --occupancy | --occupancy-only | --occupancy-um`,
jobdeck 래퍼 전달, gate `tools/validate_occupancy.py`(배터리 포함). 형식은
SPEC-FORMATS `design.ovo`, 명령·상한·게시 규칙은 SPEC-INDEXER §6.5.

검증: fixture 3종(도형·반복·덱)과 valmini(4 µm 셀, 101 × 112 격자, 9레이어,
점유 22,353셀)의 level 0 **모든 셀**이 KLayout 도형 교차 오라클과 일치(missing 0,
extra 0). 상위 레벨 == OR 풀링. valmini 색인 + 요약 0.5 s.

계획과 다른 점(모두 이 문서의 계약 안에서 더 좁게 정한 것):
- identity는 `design.ovm`의 size·mtime이 아니라 **소스 size·mtime + top 이름 +
  레이어 테이블**(전체 색인에서는 `.ovo`를 쓸 때 ovm이 아직 없고, ovm 헤더가
  같은 소스 identity를 갖는다). `--occupancy-only`는 쓰기 전에 ovm과 대조한다.
- `meta.json`에 `occupancy` 항목을 두지 않는다. 판정은 `.ovo` 헤더뿐이며 확인은
  `floe-index occupancy`. (M2의 상태줄도 파일을 직접 읽는다.)
- 면적 0 도형(폭 0 rect·path, 공선 polygon)은 셀을 켜지 않는다(KLayout region
  판정과 일치). 면적이 양수인 polygon의 폭 0 돌기는 변 규칙으로 셀이 켜진다
  (보수적).
- hull이 거부되는 path(퇴화 spine, U-turn)는 건너뛰고 로그한다(raster도 같은
  spine을 거부). 실칩 로그에 `paths_skipped`가 0이 아니면 M2 전에 확인.
- Grid 닫힌형은 rect 반복에만 적용(polygon/path는 멤버마다).
- 작업 예산의 단위 = 켠 셀 + 반복 멤버 + polygon 변의 행 방문(레이어별).
  `--occupancy-max-cells/-work/-bytes`는 게이트용 명시 CLI 옵션.
- 취소: Rust는 시그널을 잡지 않는다. 게시가 rename이므로 죽어도 이전 파일은
  남고, 다음 실행이 시작 시 tmp를 지우며 `floe2 index` 래퍼가 자식 실패·중단 뒤
  tmp를 지운다. 로더는 tmp를 읽지 않는다.
- jobdeck 래퍼: `--occupancy`는 요약이 없는 색인된 소스에 `--occupancy-only`,
  색인 안 된 소스는 `--occupancy`로 색인; `--occupancy-only`는 색인된 소스마다
  재생성(색인 안 된 소스는 요약과 함께 색인). 있는 요약은 그대로("kept").
- 빈 레이어도 0 비트맵을 전부 저장한다(레이어가 많은 레이아웃의 저장량 후속
  최적화 후보).

남은 M1 판정: 실칩 생성 시간·크기·상한 도달·`paths_skipped`(사용자가 `floe2
index <src> --occupancy-only`로 실행해 `[vfs] occupancy …` 줄과 `floe-index
occupancy <cache>` 첫 줄을 기록).

### M2 구현 기록 (2026-09-11, RENDERD 0.12.80)

구현: `render-core/src/summary.rs`(SummaryPlane·SummarySelection·레벨 선택),
`Cache::summary_selection`(조건 판정, `.ovo`를 첫 사용 때 열고 size·mtime이
바뀌면 다시 염), `Cache::page_plan_request`, `FrameScene::set_summaries`,
raster `paint_summary_plane`(타일 밴드마다 1 px 후광을 포함한 화면 마스크 →
경계 solid/내부 채움), renderd `run_render`(요청마다 결정, RetainedKey에 요약
레벨·파일 stamp, 프레임 줄 `summary_layers/cells/pixels/level/cell_um/none/
pages`), 어댑터 `summary` 딕셔너리, 뷰어 상태줄 `summary N layers … (level k,
x um; not pickable)` 또는 keep 정책에서 `summary: none (<이유>)`. (2차 리뷰 뒤
마스크 투영은 셀 중심 → 픽셀 하나, §3.) 킬 스위치 `FLOE_RUST_OCCUPANCY=off`.
gate `RenderTests`(7건).

검증(2000 × 2000 µm fixture, 200 px = 10 µm/px, 4 µm 셀 → level 1 = 8 µm =
0.8 px): 화면 마스크 == Python으로 투영한 level 1(pan 위상 0/¼/½/¾ px, 250 px에서
level 1 = 1.0 px, 260 px에서 level 0, 600 px에서 `near`로 페이지 경로와 픽셀
동일); exact(cut 0) 대비 missed_far(1 px 이웃 없는 exact 픽셀) 0, extra_far(2 px
안에 exact 없는 요약 픽셀) 0, L자의 7 px 빈 모서리 보존; `thin=cull`·cut 0·
depth 0·킬 스위치·파일 없음·잘린 파일 모두 페이지 경로와 픽셀 동일(`none=
policy|exact|depth|off|nofile|invalid`); `none:work` 레이어는 페이지 경로로 자기
순서에 그려져 요약 레이어 위를 덮음; speckle 채움에서 내부 체커·경계 solid,
선폭 3에서도 경계 1 px; `--occupancy-only`로 바꾼 파일을 열린 데몬이 같은 뷰의
다음 프레임에 반영(retained 프레임 재사용 안 함).

계획과 다른 점:
- **적용 조건의 exact**: 어댑터의 `--detail exact`는 wire에서 `cut=0 exact=0`
  이므로 `cut_dbu == 0`도 exact로 본다(`none=exact`).
- **§6 3단계(vis에서 제외)** 대신 플래너 `ViewReq::page_skip`(요약 레이어의
  페이지만 선택·디코드 생략, `summary_pages` 카운터). vis에서 빼면 모든 가시
  레이어가 요약될 때 top 셀·프레임까지 사라져 scene이 무효가 됐다. 순회는
  그대로이므로 요약 전용 서브트리의 걷기 비용은 남는다(M3 후보: 프레임이 꺼진
  요청에서 프루닝).
- pick/snap 제외는 별도 코드가 아니라 결과다: 게시 scene에 요약 레이어의 페이지가
  없으므로 found=0. 상태줄이 `not pickable`을 말한다.
- 요약 pass 순서: 레이어 루프의 자기 슬롯에서 페이지 항목(비어 있음) 직전에
  칠한다(덱은 M4).

### M3·M4 구현 기록 (2026-09-11, RENDERD 0.12.81)

M3(플래너): `ViewReq::page_skip`(M2) 위에 `ViewReq::prune_skipped` — 서브트리
판정(top 조건·자식 BVH·배치의 `masks_intersect`)에 `vis − page_skip`을 쓰고,
페이지 선택은 `vis`를 그대로 쓴다. renderd·덱은 프레임이 꺼진 요청에서 켠다
(프레임이 켜지면 요약 레이어의 셀도 프레임을 내야 하므로 순회 유지). wash
마스크는 언제나 `vis − page_skip`: 요약 레이어는 sub-cut wash를 내지 않는다.
모든 가시 레이어가 요약·프루닝되면 플랜에 셀이 없으므로 `Cache::plan`이 top
working cell을 합성한다(요약 평면이 그려질 프레임). `--explain 1`은 건너뛴
페이지를 verdict `summary`로 적고, `floe-index plan --summary-layers a/b,..
[--prune-summary 1]`이 진단 입구다. gate `PlanCliTests`: 요약 레이어의 pages 0,
explain `summary`, 프루닝 시 wc_cells 0(다른 가시 레이어가 있으면 유지).

M4(덱): pass마다 소스 뷰의 PlanRequest(`(v − d)/scale`의 px_per_dbu)로
`summary_selection` → `page_plan_request(prune = !frames)` → scene에 평면 부착.
pass = 배치 하나 = 레이어 하나이므로 순서는 그대로다. 조건은 단일 소스와 같다
(덱 exact/cut 0·depth 제한·thin cull은 페이지 경로). 덱 프레임 줄
`summary_passes= summary_none_passes=(keep인데 nofile/invalid/near/layers)
summary_cells=`, 어댑터 `deck.summary_*`, 상태줄 `summary P passes C cells (not
pickable)`·`N passes without summary`. 킬 스위치는 같은 환경변수. gate
`DeckRenderTests`: mag 0.2 배치(2000 µm 소스 → 400 µm 덱 상자)의 fit 뷰 픽셀이
단일 소스의 0..2000 µm 요약 픽셀과 동일(레벨을 소스 뷰에서 고름), 킬 스위치
픽셀 == 단일 소스 페이지 경로, depth 0 → 요약 없음(none_passes 0), 파일 없음 →
`summary_none_passes` 1, frames on에서도 요약 유지.

남은 M4 판정: 덱 fit 뷰 프레임 시간(실칩, 사용자 실측: `floe2 index deck.jb
--occupancy` 뒤 뷰어 perf 줄의 `summary … passes`와 ms).

### M5 준비: 합성 실칩 (2026-09-11)

실칩을 쓸 수 없는 동안의 대리 자산 `tools/gen_maskchip.py`: 같은 전체 크기,
데이터는 `--region`(기본 26 × 33 mm 우하단)에만, 실측 5의 페이지 수치(ICV 셀
167.7 × 535 µm, 선 11.38 × 0.0806 µm·pitch 0.255 µm → 셀당 약 2만 9천 멤버, 변형
셀은 118.6 µm 선·0.1244 µm 폭·4.3 µm 바, 3/300 쌍둥이)와 실측 7의 점유(2 × 3쌍
클러스터를 지터 격자에 놓아 4 µm 셀 7.4 %)를 재현하고, 우하단 `ICV_BR`(2 µm 바
포함)로 현장 증상(cull에서 이웃 소실·자기만 잔존)을 만든다. `--cells N`이 페이지
수(N × 2 + 6), `--pitch`가 멤버 수를 정한다. 기본 700셀은 KLayout 상자 약 4,100만
개라 생성에 분 단위·GB 단위 메모리가 든다(`--cells 60 --pitch 1.0`은 초 단위).
`--jb`는 같은 소스를 mag 1로 두 level(3/0, 3/300)에 놓는 덱을 쓴다. 실칩 대비
빠진 것: 소스 153개의 덱 규모(pass 수), 페이지 2만 5천 개(기본 1,406개), 실제
반복 인코딩(KLayout 압축이 정한 배열).

### 2차 리뷰(M1~M4 구현, 4건, 2026-09-11) 반영 (RENDERD 0.12.82)

| # | 지적 | 판정 | 조치 |
|---|---|---|---|
| 1 P1 | Grid/Pts 배치 반복을 오프셋 `Vec`으로 전개해 작업 예산 안에서도 수십 GB 할당(10^6 멤버에 16 MB, 2^31이면 32 GiB) | 사실 | 멤버를 열거하면서 하나씩 charge·walk(벡터 없음). 단위 테스트: 10^10 멤버 Grid·2만 Pts 배치에서 예산 5,000 초과 시 charge가 예산 + 2 이내 |
| 2 P1 | hull이 거부된 PATH를 세기만 하고 레이어를 `ok`로 게시 → 요약 렌더가 도형 없이 정상 프레임 | 사실 | 거부된 path가 하나라도 있으면 그 레이어는 `none:unsupported`(비트맵 없음) → 페이지 경로(exact가 같은 path를 거부하면 그 오류가 보인다). 단위 테스트 + gate(U-turn fixture 상태·파일) |
| 3 P1 | "2 px 이상 빈 간격 보존"이 깨짐(도형→셀, 셀→픽셀 두 번 확장); gate가 빈 런을 직접 보지 않음 | 사실 | 투영을 **셀 중심 → 픽셀 하나**로 바꿈(floor/ceil은 exact의 중심 표본화와 겹쳐 2 px까지 벌어졌다 — 새 gate가 실제로 잡음). 계약을 1 px 팽창 규칙으로 정정(§3: 요약 ⊆ exact의 8-이웃 팽창이고 그 역도 성립, 3 px 이상 간격은 ≥ 1 px 보존, 2 px는 위상 의존). gate: 간격 2/3/4/5 px·세 위상 fixture를 pan 위상 4개에서 픽셀 단위로 검사(양방향 1 px, 3 px 간격 가운데 픽셀 보존). `FLOE_RUST_OCCUPANCY_PX`(진단)로 0.5 px 레벨 A/B 가능 |
| 4 P2 | 비트맵 오프셋이 헤더/테이블을 가리켜도 정상 파일로 읽힘(identity=ok, 잘못된 화면) | 사실 | 로더가 `off ≥ 테이블 끝`과 테이블 순서대로 겹침 없이 이어짐을 검사(위반 시 `inside the header`/`overlaps`로 거부 → 요약 없음). 단위 테스트 + gate |
| 후속 5 P2 | `FLOE_RUST_OCCUPANCY_PX`가 4까지 허용되는데 중심 투영은 셀마다 픽셀 하나만 켜므로 1 px 초과 셀에서 채워진 도형 내부가 격자처럼 뚫림(2 px: 4,096 중 1,600, 4 px: 400) | 사실 | 허용 범위를 0 < 값 ≤ 1로 제한(초과·비정상 값은 기본 1). 단위 테스트 `the_pixel_bound_knob_never_exceeds_one_pixel` |
| 후속 6 P3 | §8 gate 2의 "셀 → 픽셀 교차", "missed = 0" 표현이 새 구현과 다름 | 사실 | "셀 중심 투영", "1 px 이웃에도 대응 요약 픽셀이 없는 exact 픽셀 = 0(그 역도)"으로 정정 |

### 실측 8-a: 실칩 생성 (2026-09-14, 0.12.116 / RENDERD 0.12.83)

사용자 실측(폐쇄망). 소스는 bbox 25,969 × 32,969 µm(실측 7의 26 × 33 mm 문제
영역 추출본으로 보임, 85.6 MB, unit 20000 dbu/µm), `floe2 index <src>
--occupancy-only`, 기본 셀 4 µm:

```
[vfs] occupancy cell=4um (80000 dbu) grid=6493x8243 levels=9 layers=4 ok=4 34M (41.1s)
occupancy … cell_dbu=80000 base_um=4 bbox=-259479264,-329909482,259909534,329473848 grid=6493x8243 levels=9 layers=4 src_size=85624316 identity=ok
layer 3/0   status=ok work=730177006 set=4344143,1221018,338098,94075,27163,8105,2698,927,387
layer 3/1   status=ok work=0         set=0,0,0,0,0,0,0,0,0
layer 3/2   status=ok work=0         set=0,0,0,0,0,0,0,0,0
layer 3/300 status=ok work=729081740 set=4342426,1220836,338091,94074,27163,8105,2698,927,387
```

| 레벨 | 셀 | 격자 | 3/0 점유 | 실측 7 |
|---|---|---|---|---|
| 0 | 4 µm | 6493 × 8243 | 4,344,143 (8.1 %) | 7.4 % (4.06 µm 격자) |
| 1 | 8 µm | 3247 × 4122 | 1,221,018 (9.1 %) | |
| 2 | 16 µm | 1624 × 2061 | 338,098 (10.1 %) | |
| 3 | 32 µm | 812 × 1031 | 94,075 (11.2 %) | 11 % |
| 5 | 128 µm | 203 × 258 | 8,105 (15.5 %) | 15.8 % (130 µm) |
| 8 | 1024 µm | 26 × 33 | 387 (45.1 %) | |

판정:

- 상한 도달 없음, 4레이어 모두 ok, identity ok. 셀 5,350만(2^30의 5 %), 작업
  7.3억(2^31의 34 %), 파일 34 MB(레이어당 8.9 MB, 1 GiB의 3 %). 점유율은 실측 7과
  같은 곡선(격자 원점·4.06 vs 4 µm 차이 안).
- **생성 41.1 s**: 리뷰 7 P2가 경고한 대로 4.3 s 추정의 약 10배. 작업 7.3억 ≈ set의
  170배이므로 멤버가 하나씩 charge된 것이다(Grid 닫힌형은 span 셀만 charge하므로
  적용됐다면 작업이 set의 몇 배에 그친다). 이 소스의 선 반복은 한 축 간격 ≥ 셀인
  2-D Grid이거나 Pts로 보인다. 후속 후보: 한 축만 조밀한 Grid에 축별 닫힌형(느슨한
  축은 멤버마다, 조밀한 축은 span 한 번) — 뷰어 실측(8-b) 뒤 기본 on 여부와 함께
  결정한다. 색인 시간 대비 비율은 미기록.
- 3/1·3/2는 도형 없는 레이어(work 0, set 0). status ok라 요약 대상에 들어가
  상태줄 `summary 4 layers`로 세지지만 그리는 셀은 없다(페이지도 없으므로 무해).
- 3/300은 3/0과 level 0에서 1,717셀 차이: 쌍둥이가 완전 복제는 아니다(합성 실칩의
  `shapes.insert` 복제와 다른 점, level 3 이상은 동일).
- fit 뷰 예측: 1200 × 1000 px 창에서 33 µm/px → level 3(32 µm) 94,075셀, 1920 ×
  1080에서 level 2(16 µm) 338,098셀. 요약이 꺼지는(`near`) 경계는 1200 px 기준 뷰 폭
  4.8 mm.
- 추출본이므로 35.8 × 34.6 mm 원본의 작업량은 별도다. 같은 기하면 같은 작업이지만
  원본 인코딩(반복 없이 사각형 나열이면 파싱만 늘고 작업은 같다)에서 확인해야 하고,
  `none:work`가 나오면 `--occupancy-um`을 키우거나 작업 상한 노출을 결정한다.
- 남은 실측(8-b): 피크 메모리, 덱 소스 전체 생성 시간, 뷰어 상태줄(fit·킬 스위치
  A/B·`near` 전환 뷰·문제 영역 477.9 × 461.6 µm 회귀·cull 불변), 덱 fit(기준 25,138
  페이지 32,283 ms), 5 mm 뷰 요약 vs exact 샷, `FLOE_RUST_OCCUPANCY_PX=0.5` A/B.
- 현장 부산물: `floe-index index file.oas --occupancy-only`(레거시 타일 색인기)가
  옵션을 outdir로 받아 `--occupancy-only` 폴더에 타일 색인을 만들었다 → 모든
  서브커맨드가 모르는 `--` 인자를 exit 2로 거부(RENDERD 0.12.84, SPEC-INDEXER §1,
  gate `test_an_unknown_option_is_refused_instead_of_becoming_the_outdir`).
- **덱 전체 `--occupancy-only`(2026-09-14, 0.12.118)**: `time` 출력 `4422.297u
  1301.507s 39:42.76 240.2% … 14124184+19200920io`. 벽시계 2,383 s(39.7 분), CPU
  5,724 s(sys 23 %), 읽기 7.2 GB, 쓰기 9.8 GB. 래퍼는 소스를 순서대로 돌리므로
  벽시계는 소스별 시간의 합이고, 소스 안에서는 작업이 있는 레이어 수(2)만큼만
  병렬이라 CPU가 240 %에 머문다. 쓰기 9.8 GB는 dense 피라미드가 bbox 면적 × 레이어
  수 × 소스 수에 비례하기 때문(§4): 전체 크기 소스가 4레이어면 소스당 51.7 MB,
  153개면 7.9 GB. 이 가운데 도형 없는 레이어(실측 8-a의 3/1·3/2)는 0으로만 찬
  비트맵이다. 소스 수·소스별 `(Ns)` 줄·`.ovo` 총 용량은 미기록. 후속 후보:
  (1) 도형 없는 레이어는 비트맵 없이 `empty` 상태로 기록(여기서는 바이트 절반),
  (2) `--occupancy-only` 모드에서 래퍼가 소스 여러 개를 동시에 실행(벽시계 ÷ 코어),
  (3) 덱 기준 셀 8 µm(바이트 1/4, `near` 경계 2배), (4) 축별 닫힌형(마킹 작업).
  기본 on 여부는 이 40 분과 덱 색인 시간의 비율로 정한다.

### 후속 (1)·(2) 구현: 빈 레이어 `empty`, 마킹 병렬화 (2026-09-14, RENDERD 0.12.85)

사용자 결정: `--occupancy-only`는 과도기 작업이라 래퍼의 소스 동시 실행은 하지
않고 바이너리 안에서 병렬화한다.

- **빈 레이어**: 양의 면적 도형이 없는 레이어(레이어 테이블에만 있는 레이어, 폭 0
  도형뿐인 레이어)는 `status = 5 empty`, 비트맵 없음. 렌더러(`planes_for`)는 셀
  없는 plane으로 받아 요약 레이어 수에 세고 그리는 것은 없다(이전과 화면·상태줄
  동일). `none:size` 자리도 차지하지 않는다. 실측 8-a 구성(4레이어 중 2개 빈)에서
  파일 34 MB → 17 MB, 덱 9.8 GB → 약 절반. 구 로더(0.12.84 이하)는 status 5를
  "요약 없음"으로 읽어 페이지 경로(페이지 없음)로 가므로 화면은 같다.
- **마킹 병렬화**: 레이어는 순서대로, 한 레이어의 마킹을 `--jobs` 스레드가 unit
  (top 레코드 조각, 배치 멤버 범위; 단일 배치 top은 최대 4단계 확장, 4 × jobs개
  목표)으로 나눠 각자의 level 0 비트맵에 마킹, OR 병합. unit은 레코드의 반복을
  쪼개지 않고 단일(`One`) 배치만 통과하므로 charge가 분할과 무관하다 → 파일은
  스레드 수와 무관하게 바이트 동일(단위 테스트 `marking_in_parallel_matches_one_
  thread_bit_for_bit`, gate `…threads_write_the_same_file`). 작업 예산은 레이어 공유
  카운터(4,096 charge마다 flush, 초과 폭 ≤ jobs × 4,096; 단위 테스트
  `the_work_budget_is_shared_across_threads`). 메모리 jobs × level 0(실칩 13 MB ×
  jobs). 킬 스위치는 `--jobs 1`(단일 스레드 경로, 예산 판정 정확).
- 로컬 확인(합성, 8코어 Mac): 멤버별 charge가 되는 소스(셀당 8 × 3,600 선, x 간격
  8.6 µm ≥ 셀이라 닫힌형 없음, 배치 100 × 25, 3/0 작업 3.66억)에서 `--occupancy-only`
  jobs 1/2/4/8 = 2.0/1.2/0.8/0.7 s. 남는 직렬 부분은 파싱·풀링·25 MB 쓰기·단일
  레코드(1/0 테두리)다. `gen_maskchip` 300셀은 KLayout이 행을 1-D 반복으로 압축해
  닫힌형이 먹혀(작업 420만, 0.1 s) 스레드 효과가 보이지 않는다.
- 실칩과의 속도 차: 합성은 charge당 약 4.5 ns인데 실칩(8-a)은 charge 14.6억을
  스레드 2개 × 41 s에 처리해 약 56 ns로 12배 느리다. 후보는 레코드 종류: 선이
  RECTANGLE(20)이 아니라 POLYGON(21)·PATH(22)·TRAPEZOID(23~26)면 멤버마다 스캔
  변환을 한다. 축 정렬 4점 polygon·수평/수직 path를 rect 경로로 보내는 빠른 길이
  후속 후보 (5). 확인: `floe-index scan <추출본>.oas`의 `record_ids`·`rep_types`·
  `shapes`(실측 8-c에 포함).
- 기대: 실칩 단일 소스 41 s(코어 2개)는 코어 수에 반비례해 줄고, 덱 전체는 CPU
  5,724 s / 코어 수 + 쓰기(절반). 실측 8-c로 확인: `floe2 index <src>
  --occupancy-only --jobs N`의 `[vfs] occupancy … ok=2 empty=2 jobs=N 17M (Ts)` 줄,
  덱 `time`, `floe-index scan` 출력.

### M5 마감 (2026-09-15, 0.12.131)

사용자 결정(실측 8-a·8-c·8-d 뒤):

- **색인 기본 on** (2026-09-16에 범위 조정, 아래): `floe2 index`가 옵션 없이
  `design.ovo`를 만든다. `--no-occupancy`로 끄고, 현재 캐시에 요약이 없으면
  재색인 없이 추가한다(`--occupancy-only` 경로). 셀 프로파일 실행은 요약을
  요청하지 않는다. raw `floe-index vfs`는 그대로 명시 옵션. 비용: 파싱은
  색인과 공유, 마킹은 추출본 기준 48스레드 4.4 s, 파일은 빈 레이어 제외(추출본
  17 MB, 덱 172 MB).
  **2026-09-16 변경(사용자 결정)**: 기본 on은 **jobdeck의 소스만**이다.
  레이아웃 `floe2 index chip.oas`와 뷰어의 File > load layout 색인은 요약 없이
  색인하고(`--occupancy`로 opt-in; 요약 없는 캐시는 그대로 둠), `floe2 index
  deck.jb`와 File > load jobdeck은 소스마다 `--occupancy`를 준다. 이유: 일반
  레이아웃은 `thin:cull`이라 요약을 쓰지 않으므로 색인 시간·파일만 든다. gate
  `validate_index_cli`(레이아웃 기본 argv에 `--occupancy` 없음, `--no-occupancy`,
  `--occupancy`가 요약 없는 캐시에 추가, 기본은 그대로 둠),
  `validate_occupancy`(plain 레이아웃 = 요약 없음, `--occupancy` 추가/up to
  date, 덱 plain = 요약 있음, 덱 `--no-occupancy` = 없음).
- **base cell 4 µm 유지**: 요약은 1200 px 창 기준 뷰 폭 4.8 mm부터 켜지고, 8-d의
  뷰에서 `near` 구간의 불편이 없었다. 2 µm는 파일·생성 4배라 보류. (2026-09-16
  실측 9 뒤 변경: 작은 칩의 fit 뷰가 `near`라 칩 크기에서 자동 선택 — 큰 칩은 그대로
  4 µm. 아래 "base cell 자동 선택".)
- **마스크 소스는 keep + detail medium**: 요약이 켜진 광역뷰는 cut과 무관하게
  점유 셀을 그리므로 medium과 high가 같고, 근접뷰에서는 양축이 cut 미만인 것만
  빠진다(한 변이 긴 마크는 hairline으로 남음). 일반 레이아웃(cull)은 medium/high
  차이가 그대로 보이며 요약이 켜지지 않는다(`summary: none (policy)`). cull에서의
  요약(존재만 표시)은 별도 결정으로 남긴다.
- **뷰어 메뉴**: View > thin shapes at wide views > auto / keep (mask policy) /
  cull (layout policy, faster). 예전 "keep thin shapes (mask detail)" 체크 항목을
  대체하며, 상태줄 `thin:keep|cull`은 그대로.
- 남은 후속: 8-b 품질 샷(5 mm 뷰 요약 vs exact), `floe-index scan`으로 charge당
  비용(생성 시간의 다음 단계), cull에서의 요약.

### 실측 8-d: 뷰어 depth 7/7에서 요약이 꺼짐 (2026-09-15, RENDERD 0.12.88)

사용자 실측(0.12.86): depth 6/7과 7/7 모두 150,735 × 103,444 µm 뷰(1414 × 971 px,
medium)에서 16.7 s — `thin pages 15k kept`, `14851/14851 pages`, `dec sum 18974 ms`,
`paints 23.6M`, `raster wall 11704 ms`, 요약 항목 없음; depth 99(무제한)로 두면
요약이 켜지고 빨라짐(사용자 확인). 원인: 뷰어의 depth 값은 999 이상일 때만
"full"로 보내고 슬라이더의 최대(7)는 숫자 7로 가는데, 요약 조건이 `depth ==
FULL_DEPTH`(무제한 표식)여서 7/7이 `summary: none (depth)`로 떨어졌다. 플래너는
높이 이상의 유한 depth를 REM_FULL로 접어 모든 도형을 그리므로 결과는 같다.

조치(사용자 결정: depth는 자유롭게 바꾸는 값이므로 레이어별 조건까지): 캐시를 열
때 셀 DAG를 한 번 훑어(top에서의 최장 경로, 배치 레코드 순회) 레이어마다 "그
레이어의 페이지를 가진 가장 깊은 셀의 깊이"를 구하고(`Cache::layer_depth`), 요청
depth가 무제한이거나 소스 계층 높이 이상이거나 그 레이어의 최대 깊이 이상이면 그
레이어의 요약을 허용한다(`depth_is_full_for`). 요청 depth 안의 셀만 그리는 exact와
전체 깊이 평탄화인 요약이 그 조건에서 같은 도형 집합이기 때문이다. 덱은 pass마다
그 소스에서 판정. 보이는 레이어 중 하나도 못 넘으면 `none (depth)`, 일부만 넘으면
그 레이어만 요약(상태줄 `summary N layers`가 보이는 수보다 작다). gate: thinwide에
2/0 상자를 가진 자식(높이 1)을 두고 — 1/0은 depth 0에서도 요약 on·픽셀 동일, 2/0은
depth 0에서 none(depth)·depth 1에서 on, 둘 다 보이면 depth 0에서 1/0만; 덱(1/0)은
depth 0·1 모두 요약 pass 1·픽셀 동일.

실칩 확인(2026-09-15, 0.12.88, 같은 뷰·509 passes): depth 6 → `10/10 pages`(깊이
7에 페이지가 있는 레이어의 pass만 페이지 경로), depth 7 → `0/0 pages`, full →
`0/0 pages`(전부 요약). 프레임 시간: depth 6 `10 tiles, 156 ms = 55 load + 157 draw`,
depth 7 `0 tiles, 154 ms = 55 load + 156 draw`, full `0 tiles, 150 ms = 53 load + 157
draw` — 0.12.86의 같은 뷰 14,851 pages·16,721 ms(3,275 load + 11,851 draw)에서 약
100배. depth 6에 남은 10페이지는 시간에 나타나지 않는다. **8-d 종결**; fit 뷰 프레임
시간 목표(1 s 이하, §8 gate 6)는 실칩 덱에서 충족.

### 실측 8-c: 단일 소스 재생성 (2026-09-15, 0.12.120 / RENDERD 0.12.85)

```
occupancy cell=4um (80000 dbu) grid=6493x8243 levels=9 layers=4 ok=2 empty=2 jobs=48 17M (4.4s)
```

| | 8-a (0.12.83) | 8-c (0.12.85) |
|---|---|---|
| 스레드 | 레이어 병렬(작업 있는 레이어 2 → 코어 2) | 마킹 분할, jobs 48 |
| 시간 | 41.1 s | 4.4 s (9.3배) |
| 파일 | 34 MB (빈 레이어 2개 포함) | 17 MB (`empty` 2) |

판정: 스레드 시간 82 s(2 × 41.1)를 48로 나눈 1.7 s에 직렬 부분(85.6 MB 파싱,
풀링, 17 MB 쓰기·fsync, 레이어 존재 집합)이 더해진 값으로 읽힌다. 덱 전체(8-a
39.7 분)는 같은 비율이면 4~5 분, 쓰기는 절반 근처가 기대치 — 덱은 래퍼가
`--jobs`를 넘기므로 `floe2 index deck.jb --occupancy-only --jobs 48`로 잰다(기본
12). 레코드 종류 가설(charge당 56 ns)은 `floe-index scan` 출력 대기.

**덱 전체 재생성(2026-09-15, `--occupancy-only --jobs 48`)**: `time` 출력
`4534.575u 1067.024s 9:53.55 943.7% … 14345872+338912io`.

| | 8-a (0.12.83, jobs 12) | 8-c (0.12.85, jobs 48) |
|---|---|---|
| 벽시계 | 39.7 분 (2,383 s; 재색인 포함으로 판정, 아래) | 9.9 분 (594 s) |
| CPU | 5,724 s (240 %) | 5,602 s (944 %, 평균 9.4코어) |
| 읽기 / 쓰기 | 7.2 GB / 9.8 GB | 7.3 GB / 0.17 GB |

판정: CPU 총량은 같고(같은 마킹 작업) 병렬도만 2.4 → 9.4코어로 올라 벽시계가 1/4이
됐다. jobs 48인데 평균 9.4코어인 것은 소스마다 직렬인 파싱(읽기 7.3 GB)·풀링·
쓰기가 남아서다(Amdahl). 남은 지렛대는 charge당 비용(레코드 종류 가설 → scan
출력)이며, 소스 병렬 파싱은 사용자 결정대로 하지 않는다. 쓰기가 9.8 GB → 0.17 GB로
57배 준 것은 "빈 레이어 절반"으로는 설명되지 않는다. 후보: (a) 덱 소스 대부분의
레이어가 비어 있고 채워진 레이어는 bbox가 작은 소스에 있다, (b) 8-a 실행 때
캐시가 stale이라 래퍼가 일부 소스를 재색인(페이지 쓰기)했다. `du -ch */design.ovo`
합계와 래퍼 마지막 줄(`N built, F failed, K kept`), 8-a 로그의 `[jobdeck] index :`
줄 유무로 가린다. 덱 소스는 667개(사용자 확인, 153은 pass 수).

**집계(2026-09-15)**: `.ovo` 667개 172.0 MB, 상위 파일 17.9 × 3, 13.6, 8.9, 8.8,
1.4 × 2, 1.3 MB(나머지 661개는 그 이하), 레이어 2,251개 = `ok` 1,176 + `empty`
1,075. 판정: (a)는 기각. ok 레이어 1,176개가 172 MB(평균 0.15 MB)인데 같은 소스의
빈 레이어는 같은 bbox라 구 코드에서도 같은 크기였으므로, 빈 레이어가 더했을
바이트는 약 160 MB이지 9.6 GB가 아니다. 남는 설명은 (b): 8-a 실행이 소스를
재색인했다(읽기 7.2 GB의 소스에 페이지 ≈ 9.8 GB 쓰기가 맞아떨어진다). 따라서 8-a의
덱 39.7 분은 색인 + 요약이고 "덱 4.0배"는 비교가 아니다. 유효한 비교는 단일 소스
41.1 s → 4.4 s(둘 다 `[vfs] occupancy` 타이머)뿐이며, 덱 전체 요약 생성의 현재
값은 9.9 분·172 MB(파싱 bound)다. 확인: `find <소스루트> -name design.ovp -newermt
2026-09-13 | wc -l` = **533**(2026-09-15 확인): 667개 중 533개(80 %)가 8-a 때
재색인됐다 — (b) 확정. 이유는 미확인(stale 캐시: CACHE_VERSION 또는 소스 mtime).
스레드 48개가 레이어마다 level 0 비트맵(추출본 6.7 MB)을 하나씩
0으로 채우고 OR 병합하는 비용(소스당 약 0.6 GB의 페이지 0 채움)은 sys 시간의
일부다 — 작업량이 작은 레이어에서 스레드 수를 줄이는 것이 사소한 후속.

## 11. 1차 계획 리뷰(7건, 2026-09-11) 반영

| # | 지적 | 판정 | 반영 |
|---|---|---|---|
| 1 P1 | polygon/path bbox·큰 Pts footprint bbox·Grid "피치 ≤ 셀" footprint는 큰 빈 공간을 채운다(반례: 셀 10, 1×1, 100×2, (4,4)·(4,−4) → 실제 81셀, bbox 1,681셀) | 사실 | §3·§5: 도형 교차 마킹을 M1 필수로, Grid 닫힌형은 축 정렬 + 간격 < 셀일 때만, Pts는 멤버마다, 한계 초과는 근사 저장 대신 "요약 없음(이유)" |
| 2 P1 | 데이터는 전체 깊이 평탄화인데 선택 조건에 depth·exact가 없어 depth 0/1과 `--detail exact --thin keep`이 근사로 바뀐다 | 사실 | §3·§6·§7: 적용 조건 5개(top 일치, 유효 depth full, exact 아님 포함). 제한 depth는 현행 경로 |
| 3 P1 | 기존 rect raster 재사용은 점유 마스크와 다르다: hairline 경로(`paint_hairline_device_rect`)는 채움 패턴을 무시하고 solid, 선폭 > 1이면 셀마다 외곽선으로 팽창; "geometry 뒤 summary pass"는 레이어 순서를 깨뜨린다 | 사실 | §3·§6: 마스크 투영 → 경계 solid/내부 채움/outline-only 계약, 선폭 1 고정, `layers` 순서 안 페인트. 덱은 pass 대체로 순서 유지 |
| 4 P1 | "셀 ≤ 1 px → 1 px 미만 빈 공간만 손실"은 틀림(양쪽 경계 확장으로 2셀 폭 간격도 닫힘); gate 1의 bbox 오라클은 큰 빈 공간을 없애도 통과; 실측 7 이미지 완전 일치는 격자·원점이 달라 목표로 부적합 | 사실 | §3 2셀 규칙, 실측 7 수치는 상대값으로 재해석; §8 도형 교차 오라클·빈 공간 fixture·pan 위상·피라미드 전환·missed/extra·빈 런 손실 gate; 실칩은 지표로 판정 |
| 5 P2 | `.ovo` 유효성·게시·호환 계약 부재; `CACHE_VERSION`을 올리면서 구 캐시 호환은 `is_stale()`(버전 불일치 = 재색인)과 충돌 | 사실 | §4: identity·top·레이어 일치 검증, 오프셋·곱 검증·잘린 파일 거부, tmp + rename, meta 불일치 처리, 열린 뷰어의 stat 재오픈; `CACHE_VERSION` 불변, `.ovo` 자체 버전 |
| 6 P2 | jobdeck 래퍼 `_jobdeck_index()`는 argv를 직접 만들고 색인된 소스를 제외하므로 옵션이 자동 전달되지 않는다 | 사실 | §5: 세 옵션 전달, `--occupancy-only` 대상 선정 변경, ovm/ovp 불변·셀 크기 검증 gate |
| 7 P2 | 4.3 s는 상한이 아닌 추정; 레이어 병렬화는 거대 계층에서 병목을 못 푼다; dense 크기는 bbox 면적 비례; 기본 생성은 실측 뒤에; pick/snap은 게시 scene 조회라 found=0 | 사실 | §5: 셀 수·작업·파일 상한, `--jobs` 준수, 취소, M5까지 opt-in; §4 면적 비례 명시; §6 pick/snap 제외 + 상태줄 |

### 2026-09-16 — 기본값 조정과 캐시 이름 개명 (0.12.134 / RENDERD 0.12.89)

- **기본값**: 요약은 jobdeck의 소스에만 기본이고 레이아웃은 `--occupancy`
  opt-in이다(위 "색인 기본 on" 항목의 변경 기록). 뷰어의 layout 로드 색인도
  `floe2 index chip.oas`와 같은 argv(`--jobs 12 --no-lod`)를 쓴다.
- **이름**: 캐시 폴더 `<src>.floe/` → 숨김 `.<src>.ice/`, DRC pack `<db>.ice` →
  숨김 `.<db>.tray`. 규칙은 `floe/cachepath.py` 하나에 있고 구 이름은 발견 시
  자동 개명된다(재색인 없음; `FLOE_CACHE_MIGRATE=off`로 끔). 상세는
  docs/CACHE-NAMING.ko.md.

### 2026-09-16 — M6 배치 깊이별 비트 평면, FLOEOVO2 (0.12.135 / RENDERD 0.12.90)

현장(2026-09-16): 9.8 GB 일반 레이아웃을 `thin:keep`으로 열자 `decoded generation
budget exceeded: 2356612414 > 1073741824` — 요약이 있어도 제한 depth에서는 쓰이지
않아(전 깊이 평탄화라 깊은 자식 도형이 새어 나옴) keep이 페이지를 전부 디코드했다.
사용자 결정: "full depth가 아닐 때라도 keep이 되면 좋겠음", 나중의 depth 구간
(start~end)까지 대응하도록 **깊이별 비트 평면**.

- **형식 v2**: 레이어마다 도형이 실제로 있는 배치 깊이 d(0 = top 자기 레코드)마다
  비트 피라미드 하나. 깊이 ≥ 15는 평면 15에 접힌다(`DEPTH_CAP`). 요청 depth N은
  평면 d ≤ N의 OR, 무제한은 전부, 나중의 구간 [s, e]는 평면 s..e의 OR. v1 파일
  (`FLOEOVO1`)은 `depth=all` 평면 하나로 읽혀 무제한(또는 레이어별 full)에서만
  쓰인다 — 기존 캐시는 그대로 열리고, 제한 depth의 요약을 쓰려면
  `floe2 index … --occupancy-only`로 한 번 다시 만든다. SPEC-FORMATS `design.ovo`.
- **마킹**: 스레드별 level-0 비트맵 복제 + OR 병합 대신 레이어당 **공유 atomic
  평면**(깊이마다 한 장)에 `fetch_or`로 찍는다. 결과는 unit 분할과 무관해 스레드
  수에 대해 바이트 동일(gate 유지). 메모리 = 레이어의 깊이 수 × level 0 한 장
  (실칩 전체 35.8 × 34.6 mm, 4 µm: 평면당 9.6 MB) — 이전의 jobs × 한 장보다 작다.
  깊이는 unit(`Unit.depth`)과 walk가 넘긴다. 파일 크기는 "레이어에 도형이 있는
  깊이 수 × v1 크기"(도형이 없는 깊이는 평면이 없음), `none:size` 상한은 평면
  단위로 센다.
- **렌더러**: `summary_selection`이 v2 파일이면 보이는 레이어 전부를 요약 대상으로
  삼고 요청 depth(무제한·계층 높이 이상은 전부)로 평면을 조합한다. 조합 비트는
  `SummaryPlane`이 소유(`Arc<[u8]>`)하고 캐시 슬롯이 `(레이어, 레벨, depth 키)`로
  기억한다(512개 상한). v1 파일·`FLOE_RUST_OCCUPANCY_DEPTH=off`는 2026-09-15의
  레이어별 full 조건 그대로. `none` 이유의 판정 순서는 policy → exact → off →
  nofile/invalid → depth → near.
- **CLI**: `floe-index occupancy` 줄에 `planes=d:count,…`(v1은 `all`), `--depth N`
  덤프. `version=`은 파일의 버전.
- **gate**(`validate_occupancy`, 29 tests): 깊이별 평면 하나하나가 KLayout
  `RecursiveShapeIterator`의 `min_depth=max_depth=d` 오라클과 셀 단위 일치(픽스처
  3종 + valmini), 3단 계층 픽스처 `deep.oas`(1/0은 평면 0·1·2, 2/0은 1, 3/0은 2),
  thinwide의 FAR 자식(2/0, 빈 공간)이 depth 0에서는 요약에 없고 depth 1·full에서
  있음, depth-0 요약이 depth-0 페이지 경로와 1 px 이내, 킬 스위치에서 `none
  (depth)` 복원, 파일 크기 = 평면 단위 테이블 + 평면별 비트맵, 스레드 수 무관
  바이트 동일. Rust: `planes_follow_the_placement_depth…`, `depths_at_or_beyond
  _the_cap…`, 평면 테이블 순서·cap 위반·개수 불일치 거부, v1 왕복,
  `a_request_depth_draws_the_planes_at_or_above_it`.
- **남은 것**: 실칩 재생성 실측(파일 크기 = 레이어별 깊이 수 배; 9.8 GB 레이아웃의
  keep 제한 depth 광역뷰 시간), 근접뷰 예산 초과(단일 레이아웃 슬라이스 스트리밍)는
  사용자 판단으로 보류(근접뷰는 요약 대상이 아님).

### 실측 9 (2026-09-16, 150 MB 실칩, 0.12.89 = M6 이전) — 무음 5분, `none (near)`

사용자 보고: `--occupancy-only`에서 `parsed 5407 cells in 29.3s (12 threads,
source released, rss 13G)` 뒤 `occupancy cell=4um (16000 dbu) …`까지 약 5분간
로그가 없었고, 평소 색인 50초짜리 파일의 요약이 5분 가까이 걸렸다. floe2에서
`thin:keep`, depth */11의 광역뷰(fit)에 `summary: none (near)`.

- **무음**: 마킹은 레이어 순서로 돌고 끝에 한 줄만 찍었다. 조치(0.12.136 /
  RENDERD 0.12.91): 10 s 하트비트(`L/D marking: u/U units work xG (Ts)`)와
  레이어 완료 줄(SPEC-INDEXER §6.5). `Opts.progress` 콜백, gate
  `progress_lines_report_the_layers_without_a_summary`.
- **5분**: 원인 후보는 (1) `none:work` 레이어 — 작업 상한 2^31 charge를 다 쓰고
  포기하므로 그런 레이어 하나가 charge당 56 ns(실측 8-a 가설)면 2분을 태운다,
  (2) 레이어 수 × 레이어당 작업(추출본은 레이어 2개에 7.3억 charge). 판정에는
  `floe-index occupancy <cache>`의 `status=`·`work=` 줄과 새 진행 로그가 필요하다.
  `--occupancy-max-work N`으로 상한을 낮추면 포기가 빨라지고 올리면 그 레이어가
  요약을 얻는다. M6(공유 atomic 평면)에서의 시간은 재측정.
- **`none (near)`**: 요약은 level 0 셀(4 µm)이 화면 1 px 이하일 때만 쓴다(§3;
  더 굵은 셀을 픽셀 중심 투영으로 그리면 채움 안에 격자 구멍이 생긴다 — 2차
  리뷰 P2). 즉 뷰 폭 ≥ 4 µm × 창 픽셀 폭(1,400 px 창에서 5.6 mm, 4K에서 15 mm)
  이어야 하므로, 작은 칩의 fit 뷰나 큰 창에서는 depth와 무관하게 `near`가 된다.
  당장은 `floe2 index <src> --occupancy-only --occupancy-um 1`(또는 2)로 셀을
  줄이면 된다(파일·마킹 4~16배). 후속 결정 후보: 색인 시 base cell을 칩 크기로
  자동 선택(예: min(4 µm, 칩 폭/4096), 하한 0.25 µm) — M5의 "4 µm 고정"을 바꾸는
  일이라 사용자 결정 대기.
- **파이프 panic**(같은 날 후속): `floe-index occupancy … | head -1`이 `failed
  printing to stdout: Broken pipe (os error 32)` panic — Rust는 SIGPIPE를 무시한
  채 시작해 `println!`이 EPIPE에 panic한다. `floe-index`가 시작 시 SIGPIPE를
  기본(SIG_DFL)으로 되돌려 C 프로그램처럼 조용히 끝난다(0.12.137 / RENDERD
  0.12.92; libc 크레이트 없이 `extern "C" signal`). gate
  `test_a_reader_closing_the_pipe_early_does_not_panic`. renderd는 stdout이
  프로토콜이라 건드리지 않았다.
- **실측 9 데이터(사용자, 0.12.89)**: `cell_dbu=16000 base_um=4 grid=388x563
  levels=5 layers=337 identity=ok`; status `313 ok / 24 empty`(`none:work` 없음);
  work 상위 `705/59 1.18G, 685/59 0.52G, 692/59 0.32G, 502/59 0.22G, 213/192
  0.21G`; `total_work 5.44G`.
  - **칩 크기** 388 × 563 셀 × 4 µm = **1.55 × 2.25 mm**. fit 뷰(약 1,000 px)에서
    4 µm 셀 = 1.8 px > 1 px → `none (near)`가 맞다. 이 칩은 `--occupancy-um 1`
    (1,552 × 2,252 셀, 평면당 0.44 MB, 313 레이어에 대략 200~400 MB) 또는 2 µm
    (창 1,100 px까지, 약 50~100 MB)가 필요하다. 마킹 시간은 멤버 수가 지배하므로
    (아래) 셀을 줄여도 크게 늘지 않는다.
  - **5분**: `none:work`가 없으므로 예산 소진이 아니다. 54억 charge를 12스레드로
    약 270 s에 마킹 = 초당 2,000만 charge = 8-a에서 잰 **1스레드 속도(56 ns/charge)**
    와 같다 → 병렬이 먹지 않았다. work가 셀 수(21.8만)의 5,400배이므로 charge는
    반복 멤버가 지배한다(멤버당 1 charge + 셀). 원인: unit 분할이 개수 기준이라
    top의 배치 수가 4 × jobs를 넘으면 확장이 멈추고, 11.8억 charge짜리 블록 하나가
    unit 하나로 한 스레드에 남는다. 조치(0.12.138 / RENDERD 0.12.93): 작업량 기준
    분할(SPEC-INDEXER §6.5, `--occupancy-balance 0` 킬 스위치), gate
    `a_heavy_block_among_light_placements_is_split_by_work`(개수 분할은 블록을 unit
    0개로 남기고, 작업량 분할은 8개 이상으로 쪼갬; 파일 바이트 동일). 기대: 12
    스레드에서 5분 → 수십 초, 48스레드에서 10초대(효율 40% 가정). 재측정 대기.

### 2026-09-16 — base cell 자동 선택 (0.12.140 / RENDERD 0.12.95, 사용자 결정)

실측 9의 1.55 × 2.25 mm 칩은 4 µm 셀이 fit 뷰에서 1.8 px라 요약이 `near`로 꺼졌다.
결정: `--occupancy-um`을 생략하면 top의 긴 변이 **2,048셀 이상**이 되는 가장 굵은
4/2/1/0.5/0.25 µm를 고른다(`occupancy::auto_base_um_for_span`, `Opts.base_um` 0 =
`BASE_AUTO`). 8 mm 초과 칩은 4 µm 그대로(추출본·실칩·덱은 불변), 2.25 mm 칩은 1 µm
(1,552 × 2,252 셀, 평면당 0.44 MB), 1 mm 미만은 0.25 µm. 명시 `--occupancy-um`은
그대로다. 마킹 시간은 반복 멤버 수가 지배하므로 셀을 줄여도 크게 늘지 않고, 큰
사각형만 셀 수에 비례한다. 로그 `[vfs] occupancy cell=1um (4000 dbu, auto)`. gate
`the_automatic_base_cell_follows_the_chip_size`(규칙 표, 10 × 3 mm → 4 µm, 3 × 2 µm
→ 0.25 µm), `test_the_base_cell_follows_the_chip_size_unless_given`(CLI: 10 × 8 µm →
0.25, 3 × 2 mm → 1, 명시 4 → 4). 덱 소스는 크기가 제각각이라 작은 소스에 가는
셀이 붙는다 — 덱 합계 크기는 첫 재생성에서 확인(사용자: 덱은 지금 문제없음).

### 2026-09-16 — 일반 레이아웃 생성 비용: 레이어 재검색·atomic 경합·종료 대기

현장 보고: 9.8 GB 실칩은 공통 파싱 약 5분, 기존 인덱싱 약 30분에 비해
`floe2 index source.oas --occupancy-only --jobs 12`가 약 100분(사용자 실행 명령
확인). `--jobs 12`는 래퍼에서 Rust 인덱서와 occupancy 옵션까지 전달되며,
스레드 수 누락은 아니다. 일반 레이아웃의 sub-cut wash는 drawing을 약
1초에서 6~8초 이상으로 늘렸다. 이 실칩의 로그·입력은 이번 측정에 없으므로
100분의 원인별 비중이나 개선 후 시간을 확정하지 않는다.

코드에서 확인하고 수정한 비용:

1. **다른 레이어 재검색**: 레이어별 존재·깊이·작업량 계산이 전체 도형 목록을
   반복 검사하고, 마킹에서도 배치 멤버마다 셀의 모든 레이어 레코드를 다시
   검사했다. 한 번의 분류로 `(layer, datatype, cell)`별 참조 목록을 만들고
   모든 단계가 해당 레이어의 목록만 순회한다. 원본 geometry·반복은 복사하지
   않으며 목록은 레이어 완료 때 해제한다. 참조 포인터와 Vec 여유 용량·희소 맵
   메모리가 추가된다(SPEC-INDEXER §6.5).
2. **포화된 비트에 반복 atomic 쓰기**: 이미 켜진 점유 셀에도 `fetch_or`를 해
   여러 스레드가 같은 cache line의 쓰기 소유권을 주고받았다. relaxed load에서
   필요한 비트가 모두 있으면 OR을 생략한다. 비트는 0→1로만 바뀌므로 안전하다.
   오래된 값을 읽으면 OR이 한 번 더 발생할 뿐 누락되지 않는다. charge·도형
   교차·깊이·빈 공간·파일 형식은 유지한다.
3. **하트비트 종료**: 마킹 종료 후 250 ms sleep 중인 로그 스레드를 join해서
   짧은 레이어마다 최대 250 ms를 기다렸다. 종료 채널로 즉시 깨운다. 진행 줄의
   unit 수는 배정 수에서 **완료 수**로 바꾸고 실제 `workers=N`을 표시한다.
   분류 시간과 느린 준비 단계도 따로 출력한다.

로컬 release 빌드, 동일 입력·셀 크기·작업량, **3회 중앙값**(초). 파싱·페이지
색인·진행 로그 없는 라이브러리 측정이다. 밀집 경합을 분리해 재현하는 합성
입력이므로 실칩 전체에 대한 배율로 환산하면 안 된다.

| 합성 입력 | jobs | 수정 전 | 수정 후 |
|---|---:|---:|---:|
| 셀 안 64레이어 × 256개 레코드, 배치 1,024개 | 1 | 1.637 | 0.629 |
| 위와 같음 | 12 | 1.016 | 0.193 |
| 1레이어 × 8,192개 레코드, 배치 8,192개, 점유 셀 3개 | 1 | 2.109 | 1.971 |
| 위와 같음 | 12 | 2.759 | 0.519 |

두 사례의 jobs 1/12, 수정 전/후 `.ovo`는 모두 **바이트 동일**. 두 번째 사례에서
기존의 멀티스레드 역효과를 재현했으며 수정 후에는 병렬 이득이 생겼다. 재현:

```sh
cargo run --release --manifest-path rust/Cargo.toml -p floe-vfs \
  --example bench_occupancy -- mixed 12 /tmp/mixed.ovo
cargo run --release --manifest-path rust/Cargo.toml -p floe-vfs \
  --example bench_occupancy -- dense 12 /tmp/dense.ovo
```

별도 CLI 사례(337레이어, 레이어당 rect 1개, 1×1 점유 셀, jobs 12, 진행 로그
활성)는 occupancy 타이머 **85.7 s → 0.0 s**(0.1초 단위 표시), `.ovo` 바이트
동일. 이 값은 종료 대기의 영향만 보여 주며 100분 전체를 설명하지 않는다.

검증: `cargo test --manifest-path rust/Cargo.toml -p floe-vfs --lib` 65개 통과,
`tools/validate_occupancy.py` 34개 통과(KLayout 도형 교차·깊이 오라클,
피라미드·렌더링·캐시 교체 포함). 동시 중첩 span의 처음 쓰기와 포화 후 반복
쓰기가 비트와 빈 공간을 보존하는 Rust 회귀 테스트를 추가했다.

남은 실측: 실칩의 `grouped records`, 레이어별 `work`, `workers`, 완료 unit,
시간과 메모리. 반복 멤버를 실제로 방문하는 비용은 여전히 남으며 하나의 거대한
레코드 반복은 unit 내부에서 직렬이다. 이것이 지배하면 추가 작업은 반복 분할
또는 격자 위상·변환·깊이를 보존하는 셀 요약 재사용이다. 셀 비트맵을 임의 좌표에
단순 이동하면 격자 오차가 누적되므로 별도 정확도 검증 없이 적용하지 않는다.
일반 레이아웃에서 생성된 요약을 사용하려면 기존대로 `thin:keep`이 필요하다.
이번 수정은 일반 레이아웃의 thin 기본값·sub-cut 정책을 바꾸지 않는다.

### 2026-09-16 — sub-cut 규칙 기본 off: 광역뷰 존재는 요약이 맡는다

사용자 결정(FLOE2_OPTIMIZATION 결함 C 종결): 일반 레이아웃의 sub-cut wash는
느려지는 부작용에 비해 여전히 다 보이지 않고, 덱은 요약이 있으므로 쓰일 일이
없다 — 단일 레이아웃·덱 모두 기본 off(진단 `FLOE_RUST_SUB_CUT_WASH=on`,
`FLOE_RUST_DECK_WIDE=on`). 따라서 일반 레이아웃에서 광역뷰에 작은 도형의 존재를
보려면 `floe2 index <src> --occupancy`(또는 `--occupancy-only`) + `thin:keep`이
유일한 길이고, 요약 생성 시간이 실사용 조건이다: 150 MB 실칩은 위 재검색 제거
뒤 231.9 s(cell 1 µm auto), 다음 병목은 한 레코드의 거대 반복이 unit 안에서
직렬인 것(합성 재현: 레코드 반복 1,680만 멤버 jobs 1 = 0.406 s, jobs 12 =
0.388 s; 같은 멤버를 배치 반복으로 두면 0.569 → 0.149 s). 후속: 레코드 반복의
멤버 범위 unit 분할(per-member 경로만, `work`·파일 바이트 동일).


### 2026-09-16 — 레코드 반복의 멤버 범위 분할 (0.12.144 / RENDERD 0.12.99)

위 진단(레코드 자신의 Grid/Pts 반복은 unit 안에서 직렬)에 대한 리뷰 조건 네 가지와
처리:
1. **실칩의 주된 병목인지는 추정** — work는 멤버 수 외에 켠 셀·변 행·배치 반복의
   재방문도 세므로 1.18G charge만으로 단일 거대 레코드를 확정할 수 없다. 그래서
   이 분할은 "안전한 다음 최적화"로 넣고 실칩 확인은 로그(`grouped records`,
   `prepared`, `marking u/U units workers=N`, 레이어별 `ok … (Ts)`)로 한다.
2. **charge 보존** — 레코드 반복은 멤버 수를 한 번에 charge한다. 범위 unit은
   `m1 − m0`를 한 번에 charge하므로 정상 완료 시 합이 같다. 면적 0 rect·폭 0 path는
   범위마다 charge 없이 끝나고, 거부되는 path의 `paths_skipped`는 멤버 0을 가진
   범위만 세며, closed form 채움은 멤버 0의 범위만 한다. 예산 초과(`none:work`)는
   판정이 같고 work 값만 다를 수 있다(기존 계약).
3. **깊은 계층** — 무거운 자식의 Grid/Pts 배치(멤버 ≤ 64)는 멤버마다 자식 unit을
   따로 만들어 배치 반복 안의 거대 레코드도 분할에 닿는다(멤버 charge는 첫 unit의
   `extra`). 단일 배치는 기존대로 8단계까지 내려간다.
4. **기대치** — 스레드 수만큼이 아니라 합성 배치 반복의 3.8×(12스레드)가 상한
   근처이고, 상위 5개 레이어 2.45G는 전체 5.44G의 45 %다. 실칩 수치는 재측정으로.
검증: Rust `a_record_repetition_is_split_by_members_and_the_file_stays_the_same`
(rect/polygon/path × Grid/Pts × 회전·미러·깊이 × 직접 배치·Pts 배치·2×2 배열
배치, jobs 1/4, balance on/off → 파일·work 동일; closed form·면적 0은 미분할;
`none:work` 판정 동일), `GiantRepetitionTests`(KLayout 압축이 만든 반복 레코드,
jobs 1/4/`--occupancy-balance 0` 파일 동일, `floe2 index --occupancy-balance 0`
통과). 킬 스위치 `--occupancy-balance 0`을 `floe2 index`에도 노출했다.
합성 재현(로컬 release, 1 µm 상자 4096×4096 = 1,680만 멤버, 8 µm 피치, cell 4 µm):
레코드 반복 jobs 1/12 = 0.430/**0.102 s**(이전 0.406/0.388), 같은 멤버의 배치
반복 0.562/0.142 s, work 33,554,432 네 경우 동일. 실칩(231.9 s) 재측정 대기.


### 2026-09-17 — 요약 없는 광역뷰: 대표(page frontier)

사용자 설계(FLOE2_OPTIMIZATION 결함 C 후속 3, SPEC-PLANNER §3): 요약이 없는
일반 레이아웃은 컷이 버리던 페이지·배치를 대표(4^k개 중 하나, 줌 사이 포함 관계)
로 남겨 비용을 컷 시점 수준으로 묶는다. 요약이 주는 채워진 존재와 달리 무늬이고
빌드가 없다. 두 길은 공존: 요약이 있으면(`thin:keep`) 요약이 그리고, 없으면
대표가 남는다. fit급에서 대표가 느리면 그 줌 대역의 캐시(사전 계산)를 검토한다.
(2026-09-17 저녁: 플래너 쪽 대표는 비활성 — FLOE2_OPTIMIZATION 결함 C 종결 2. 요약
없는 레이아웃의 광역뷰는 다시 컷 아래를 버리며, 다음 답은 인덱싱 때 만드는 별도
파일의 대표 데이터다.)

### 일반 thin:cull의 별도 대표 파일 (0.12.154)

`--representatives` / `--representatives-only`는 `design.ovr`의 유한 점 샘플을
생성한다. occupancy 마킹과 별개이며, 일반 cull 플랜의 페이지 선택을 늘리지 않는다.
정확한 점유 요약의 대체물이 아니고 덱/keep 정책도 바꾸지 않는다.
생성 비용, 확대 시 밀도 한계와 사용법은 [REPRESENTATIVES](REPRESENTATIVES.ko.md) 참조.
