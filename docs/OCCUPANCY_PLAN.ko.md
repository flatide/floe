# 점유 요약(occupancy summary) 구현 계획 — 마스크 정책의 광역뷰

작성 2026-09-11, 1차 계획 리뷰(7건, §11) 반영 2026-09-11. 근거는 JOBDECK.ko.md
§10 실측 6·7. 착수 여부는 사용자 결정이며, 이 문서는 그 결정과 착수 뒤의 작업
순서를 위한 것이다.

## 0. 한 줄 요약

마스크 정책(`thin=keep`)의 광역뷰에서는 페이지 디코드·raster 대신 **소스·레이어별
점유 비트맵 피라미드**(셀 ≤ 화면 1 px인 레벨)를 화면 마스크로 투영해 그린다.
점유는 색인 시 **도형 교차**로 만든다(bbox 대체 없음). 근접뷰·exact·제한 depth는
지금처럼 exact keep. 일반 레이아웃 정책(`thin=cull`)은 바꾸지 않는다.

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
- 빈 공간의 **위치**를 화면 해상도에서 보존한다. 보장 범위는 §3의 2셀 규칙.
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
- **오차 범위(2셀 규칙)**: 점유 판정이 셀 교차이므로 마스크는 도형의 각 경계에서
  최대 1셀 바깥까지 넓어진다. 따라서 빈 간격 g는 축 방향으로 g ≥ 2·셀이면 최소
  1셀 폭이 항상 남고(보존), g < 2·셀이면 위상에 따라 닫힐 수 있다. 화면에서는
  셀 ≤ 1 px이므로 **2 px 이상의 빈 간격은 항상 보존, 1~2 px는 위상 의존, 1 px
  미만은 보장 없음**. "1 px 미만만 잃는다"는 이전 표현은 틀렸다(리뷰 4). 실측 7의
  과대 수치는 1 px 셀 요약을 기준으로 잰 상대값이며 exact 대비 오차는 gate 2·6에서
  따로 잰다.
- **점유 판정(생성 계약, M1 필수)**: 셀 비트 = "그 셀과 교차하는 도형이 하나라도
  있음"을 도형 교차로 판정한다. rect는 셀 범위, polygon/path는 셀 격자 위 보수적
  스캔 변환(§5), 반복은 §5의 닫힌형 조건을 만족할 때만 footprint, 아니면 멤버마다.
  처리 한계를 넘는 레이어는 근사로 저장하지 않고 "요약 없음(이유)"로 기록한다.
- **적용 조건(5개)**: 요청이 `thin=keep`이고, exact 요청이 아니고, 유효 depth가
  full이고, `.ovo`가 기록한 top·소스·레이어 테이블이 현재 캐시와 일치하고, 뷰의
  µm/px ≥ base cell(level 0 셀이 1 px 이하)일 때만. 하나라도 아니면 현행 경로
  (리뷰 2: depth 0/1에서 깊은 자식 도형이 요약으로 보이거나 `render --detail exact
  --thin keep`이 근사로 바뀌면 안 된다).
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

캐시 디렉터리(`<src>.floe/`)의 선택적 sidecar. 소스 좌표계(world dbu, 캐시의 top 셀
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

- 기준 셀: 옵션 `--occupancy-um`(기본 4 µm). 근접뷰 경계(800 px 창에서 3.2 mm 뷰)와
  저장량의 절충. 2 µm면 4배.
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
- 병렬·메모리: 동시에 만드는 레이어 수 = `--jobs`. 메모리 = jobs × (level 0 +
  상위 1/3). 레이어가 적고 계층이 거대한 소스에서는 병렬화가 병목을 풀지 못하므로
  시간은 상한이 아니라 타이머·로그(`occ/Nt`, 레이어별 마킹 수·시간)로 관찰한다.
  실측 7의 4.3 s는 자릿수 추정일 뿐이다.
- 취소: SIGINT·상위 취소 시 tmp 삭제, 기존 `.ovo` 보존.
- **옵션·기본**: M5 실측 전까지 **opt-in**(`floe2 index --occupancy`, 기본 off).
  `--occupancy-um F`(기본 4), `--occupancy-only`(기존 캐시에 추가·교체, ovm/ovp
  불변). 기본 on 여부는 실측 8 뒤 결정.
- **jobdeck 래퍼(리뷰 6)**: `_jobdeck_index()`는 argv를 직접 구성하고 이미 색인된
  소스를 대상에서 뺀다(`floe/cli.py`). 자동 전달되지 않으므로 명시 구현: 세 옵션
  전달, `--occupancy-only`면 색인된 소스도 대상에 포함, gate에서 실행 전후 ovm/ovp
  해시 불변과 헤더 `base_cell_dbu` == 요청값 확인.

## 6. 선택 규칙(플래너·renderd)

요청 단위로 정해진다(2026-09-11 정책 분리와 같은 원칙).

1. 조건: §3의 5개(`thin=keep`, exact 아님, 유효 depth full, `.ovo` 유효·일치, µm/px
   ≥ base cell) 모두 참이고 킬 스위치가 아닐 때. 레이어 단위로 `.ovo`의 status가
   ok인 레이어만 요약, none인 레이어는 현행 경로.
2. 레벨: 셀 ≤ 1 px인 가장 굵은 레벨 L. 덱은 소스 뷰 기준(덱 µm/px ÷ scale).
3. 플랜: 요약으로 그릴 레이어를 `vis` 마스크에서 뺀 채 기존 플랜을 돈다 → 그
   레이어의 페이지 선택·페이지 BVH·자식 순회가 생략된다. 프레임(r == 0)은 레이어와
   무관하므로 그대로. 나머지 레이어는 exact 경로 그대로.
4. renderd 전용 경로: 뷰와 교차하는 level L 셀을 밴드·타일 단위로 화면 마스크에
   투영하고(기존 raster 밴드 병렬화에 맞춤), §3의 경계/내부 규칙으로 스타일을
   적용한다. 페인트 위치는 `StyledGeometryRasterRequest.layers`(칠 순서, 뒤가 덮음)
   안에서 그 레이어의 차례. 셀을 도형으로 넘기지 않는다.
5. 근접뷰(µm/px < base cell)·exact·제한 depth: 현행 exact keep. 예산 초과는 현행 거부.
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
- 적용 조건은 pass 단위로 §3과 같다: 덱 요청의 depth가 full이 아니거나 exact
  (`floe2 render --detail exact`)면 요약 없음. 뷰어의 덱 기본 depth는 M4에서 확인.
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
2. **렌더**: (a) 마스크 투영 == Python 기대 마스크(셀 → 픽셀 교차), pan 위상 0/¼/½/¾
   px, 피라미드 전환 직전·직후 배율, 레벨 선택이 배율마다 맞는지. (b) exact 대비
   품질: 같은 뷰의 exact 렌더(fixture는 작아 가능)와 비교해 missed(exact 점유 픽셀이
   비점유) = 0, extra ≤ exact 점유 픽셀의 1 px 이웃 밴드, 2 px 이상 빈 런 손실 0,
   큰 rect의 채움·외곽선이 exact와 경계 1 px 이내. (c) 레이어 순서: 아래 요약 + 위
   exact fixture에서 위 레이어가 보인다.
3. 정책 불변: `thin=cull` 픽셀 불변(A/B), keep 근접뷰·exact·depth 0/1 픽셀 불변,
   keep 광역뷰는 요약(요약 없는 캐시에서는 현행과 동일).
4. 덱: 합성 순서·서브윈도·레벨 선택(scale 반영), 요약 레이어의 sub-cut wash 없음,
   depth 제한·exact 덱 렌더에서 요약 없음.
5. 운영: 킬 스위치 A/B, `--explain` verdict, perf 카운터 파싱, 잘린 파일·헤더 불일치
   (top·레이어·identity) 거부, 취소 뒤 tmp 없음·기존 파일 보존, `--occupancy-only`
   전후 ovm/ovp 해시 불변, jobdeck 래퍼 옵션 전달, pick/snap 제외 상태줄.
6. **실칩(실측 8로 기록)**: 실측 7 이미지와의 완전 일치는 목표가 아니다(격자 4.06 vs
   4 µm, 영역 bbox와 소스 bbox의 원점 차). 지표: exact가 가능한 근접뷰에서 missed/
   extra, fit 뷰 프레임 시간(목표 1 s 이하), 2 px 이상 빈 런 손실, 생성 시간·크기·
   상한 도달 여부.

## 9. 단계

| 단계 | 내용 | 판정 |
|---|---|---|
| M1 | `design.ovo` 형식·유효성·atomic 게시, 도형 교차 마킹(rect/polygon/path/반복), 처리 한계·취소, `--occupancy`(opt-in)·`--occupancy-um`·`--occupancy-only`, jobdeck 래퍼 전달, gate 1·5(생성 부분) | **완료 2026-09-11(RENDERD 0.12.79, §12)**: fixture·valmini 오라클 완전 일치. 실칩 생성 시간·크기는 사용자 실측 대기 |
| M2 | renderd 전용 마스크 경로(단일 소스), 5개 조건, 레벨 선택, 레이어 순서, pick/snap 제외, 킬 스위치, gate 2·3·5 | **완료 2026-09-11(RENDERD 0.12.80, §12)**: 단일 소스 keep 광역뷰가 요약으로 그려짐, cull·근접뷰·exact·depth 제한·킬 스위치 픽셀 불변 |
| M3 | 플래너에서 요약 레이어의 페이지·계층 생략, `--explain summary`, 카운터 | **완료 2026-09-11(RENDERD 0.12.81, §12)**: 요약 레이어의 페이지 0, 프레임 없는 요청은 요약 전용 서브트리 프루닝(wc_cells 0) |
| M4 | 덱 통합(소스 뷰 레벨, pass 대체, wash 억제, depth/exact 조건), gate 4 | **완료 2026-09-11(RENDERD 0.12.81, §12)**: mag 0.2 덱의 fit 뷰 픽셀 == 단일 소스 요약 픽셀. 덱 fit 뷰 시간은 실칩 실측 대기 |
| M5 | 실칩 실측 8, base cell·기본 on/off 확정, 문서(JOBDECK §10·FLOE2_OPTIMIZATION 결함 B) | 목표 시간·지표 달성 여부로 기본값 결정 |

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
x um; not pickable)` 또는 keep 정책에서 `summary: none (<이유>)`. 킬 스위치
`FLOE_RUST_OCCUPANCY=off`. gate `RenderTests`(7건).

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
