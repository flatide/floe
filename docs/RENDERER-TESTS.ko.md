# 러스트 렌더러 대체 — 테스트 계약 (klayout 치환 프로젝트용)

최신화: 2026-08-22 (floe 0.11.45). 별도 진행 중인 자체 rust 렌더러가
floe 뷰어의 klayout 경로를 대체하려면 무엇을 소비하고, 무엇을
재현하고, 어떤 오라클을 통과해야 하는지의 정본 목록. 렌더러 쪽
저장소가 아니라 **floe 쪽에서 보증하는 계약**을 적는다.

관련이지만 별개인 문서: `rust/VECTOR_EXPORT_PLAN.md`(FVX) — 뷰포트
지오메트리를 독립 아티팩트로 **내보내는** 포맷 설계. 본 문서는 뷰어
런타임의 파스+래스터 경로 자체의 대체를 다룬다.

## 1. 대체 범위 — 지금 klayout이 하는 일

| 위치 | 역할 | 대체 여부 |
|---|---|---|
| `floe/viewport.py` | vfsd 델타 OASIS 파스 → 워킹셋 Layout 상주, WC top 스위칭, 페이지 셀 add/drop/evict, pick/snap 지오메트리 쿼리 | **대체 대상** |
| `floe/service.py` | 전용 프로세스에서 klayout C++ 렌더(GIL 격리) + 커버리지 numpy 팔레트 합성 + 프레임/라벨 오버레이 준비 | **대체 대상** (프로세스 분리 요구 자체는 렌더러가 GIL-프리면 소멸) |
| `floe/render.py` | headless PNG (`klayout.lay`) — CLI `render`/`clip`/캡처 | **대체 대상** |
| `tools/validate_*.py` | 게이트 오라클(멤버 recount, 픽셀 XOR) | **유지** — klayout은 대체 후에도 기준자(oracle)로 남는다 |
| `floe/cache.py`, `floe/cli.py` | 레거시 .tiles 인덱서, DRC/SVRF 등 비렌더 경로 | 무관 |

## 2. 입력 계약 (렌더러가 소비하는 것)

- **VFS 캐시(`.<src>.ice/`)**: `design.ovm`(v7 메타·인덱스) + `design.ovp`(페이지
  페이로드) + `design.ovt`(텍스트 풀) + `meta.json`(뷰어 요약·미니맵
  프런티어). 구조 정본: `docs/SPEC-FORMATS.ko.md`. 오픈 검증 규칙
  (ovm이 커밋한 ovp/ovt 바이트 길이 일치)은 `floe-vfs::Vfs::open`이
  이미 구현 — 직접 열지 말고 이 크레이트를 쓰는 것을 권장.
- **페이지 페이로드**: 각각이 **독립된 완전한 OASIS 파일**(내부
  CBLOCK 압축). 셀 하나만 담고(`Vfs::page_name()`: exact는
  `P{ci}_{li}_{seq}`, LOD 변종은 `q…` 접두), 좌표는 **셀-로컬
  dbu**. 도형 = RECTANGLE/POLYGON/PATH + repetition
  (One/Grid/Pts — Pts는 비전개, 1M 멤버 = 레코드 1개). 소스의
  CIRCLE(record 27)은 인덱싱 파스에서 **내접 64각형 폴리곤**으로
  변환되므로(0.11.46, 상수 코사인 테이블 = 플랫폼 무관 바이트)
  페이지 페이로드에 CIRCLE은 나타나지 않는다 — 렌더러는 원
  프리미티브가 필요 없다. 배치 변환은
  페이지에 없고 델타/플랜의 placement(x,y,rot,flip,rep)가 준다.
- **데몬 프로토콜(운영 경로)**: plan → delta(ack-gen 트랜잭션) →
  apply. 델타 = WC top `W{gen}_{r}_{ci}` + 페이지 셀 스플라이스,
  names 테이블은 데몬 런당 1회, 라벨은 요청별 응답(페이지에 텍스트
  없음). 정본: `docs/SPEC-VFSD.ko.md`. 세션 의미(스테일 드롭, 부분
  적용 폴트 복구, 제로 예산 축출)는 L1~L9 게이트가 고정.
- **직접 접근(브링업/벤치 경로)**: `floe-vfs` 공개 API —
  `Vfs::open(dir)`, `hier::plan_hier(&ovm, &req, &opts)`(뷰 →
  페이지 목록), **`Vfs::read_page_batch(&[u32]) ->
  Vec<(u32, Vec<u8>)>`** (0.11.45 신설: 데몬 없이 페이지 바이트
  획득 — 호출 순서 보존·중복 허용·범위 밖 = Err, IO는 파일순).
  렌더러 단독 파서/래스터 벤치는 이 경로로 시작하면 된다.

## 3. 시각 계약 (재현해야 할 페인트 규칙)

정본: `docs/SPEC-VIEWER.ko.md`, `docs/SPEC-PLANNER.ko.md`,
`rust/VFS_HIER.md`. 요점만:

- **페인트 순서**: 회색(프레임 바닥) < 디자인 < 흰 1px 외곽 —
  `validate_render_frames`가 고정.
- **도형 픽셀 규칙 — area-true (2026-09-22, 사용자 결정, 뷰어 기본)**: 아래 §4의 KLayout 규칙은
  채우기 두 표본(픽셀 중심 + 아래 경계)과 각 변을 담은 픽셀의 1px 외곽을 합쳐 도형을 **축마다 약
  1px 키운다**(3.8 px 막대 → 4.79 px, 간격 −1 px; 1.2·1.5 px 간격은 닫힘). 1px 미만 도형은 최소
  1px(가장자리 반올림으로 2px까지)이라 0.1 px 선이 1 px 간격이면 모든 열이 켜진다(면적의 10배).
  area-true는 (1) **사각형 — 폭 우선**(2026-09-22 사용자 결정, 모든 크기): 축마다 m = ceil(w − t) px
  (w의 정수 픽셀은 항상, 소수 부분이 t보다 크면 1 px 더)를 도형 가운데(floor(중심 − m/2 + 1/2))에
  놓고, t는 그 축의 **세계 박스 해시 순위**(가로·세로 독립 — 같은 t를 쓰면 0.5×0.5 px가 25 %가 아니라
  50 % 남는다). **배열(OASIS 반복 Grid) 멤버는 번호로 분산**한다: 세계 x로 멤버를 옮기는 번호 p와 다른
  번호 s에 대해 t_x = frac(u_x + vdc(p) + φ·s)(vdc = 비트 반전 번호, φ = 황금비), t_y는 p·s를 바꿔서 —
  한 줄 안의 이웃이 몫만큼 넓어지고(소수 0.5면 하나 건너 하나), 줄마다 φ만큼 밀려 줄이 서로 같지
  않으며, 한 줄 배열에서도 두 축이 독립(0.5×0.5 px 25 %). **축 정렬 격자는 세계 격자 번호로
  센다**(2026-09-22, ADAPTIVE_CUT_DENSITY_PLAN §4.2 후보 2): pitch P인 축의 번호는 멤버 세계 bbox
  모서리 div_euclid P, u는 정규화한 pitch·위상(모서리 rem_euclid P; 반복 없는 축은 고정 좌표)·세계
  크기의 해시 — 레코드의 첫 멤버·개수·번호는 쓰지 않는다. 그래서 같은 세계 격자는 한 Grid로든,
  인덱서의 분할 조각(`frag_rep`가 시작점을 옮기고 번호를 다시 센다)으로든, 축을 바꾸거나 pitch를
  음수로 쓰든, 회전·반사 배치로든 같은 멤버를 넓힌다(게이트: 한 배열·32개 셀 두 번 배치·90° 회전
  열이 같은 5760 px — 레코드별 번호일 때는 720·1200 px가 달랐다. 같은 레이아웃을 페이지 목표
  16 MiB·1 MiB로 인덱싱해 Grid가 실제로 두 페이지에 잘려도 pan 5곳에서 같은 픽셀 — 레코드별 번호일
  때 258 px 차이). 1멤버 조각(`Rep::One`)과 1×1 Grid는 단일 도형과 같은 세계 박스 해시를 쓴다.
  2차원 격자의 한 줄 조각은 페이지 파일에 1차원 반복으로 기록돼 다른 축 벡터가 사라지므로(처음부터
  한 줄로 저장된 배열과 구분할 정보가 없다) **그 한 줄의 격자**로 센다 — 남은 축의 pitch·위상과
  다른 축의 고정 좌표. 그 줄을 다시 나눈 조각끼리는 같고 줄 안의 분산도 유지되지만, 잘려 나온 원래
  2차원 격자와는 선택이 달라 조각 간 동일성 보장 범위 밖이다(2026-09-23 검토, 단위 테스트
  `a_fragment_that_lost_an_axis_ranks_as_its_own_row`; 게이트: 2×64 격자를 행 사이로 자른 한 줄
  조각 둘이 따로 저장한 두 행과 같은 863 px를 켜고, 자르지 않은 격자와는 300 px가 다르다 — 밀도는
  같고 넓어지는 멤버만 다르다 — 그 합성 뷰의 관측값이지 일반 크기가 아니다). 실칩에 그런 조각이
  있는지는 인덱서 빌드 로그의 `rep-split … (G grid pieces: R one-row of a 2-D grid, O one-member)`
  줄로 본다(SPEC-INDEXER §2). 이 수는 셀 정의에 저장된 조각 수라 화면 영향의 크기가 아니다: R이
  하나여도 그 셀이 반복 배치되면 배치마다 이음매가 보이고, O(1멤버 조각)도 해시로 돌아가 선택을
  바꾼다. R·O가 0일 때만 이 경로가 없다고 말할 수 있다.
  세계 격자 순위는 생존 멤버를 직접 열거할 수 있는 규칙이기도 하다(ADAPTIVE_CUT_DENSITY_PLAN §4.3
  1단계, 2026-09-23: vdc의 2진 구간 = 등차수열; 단위 테스트
  `sub_pixel_survivors_can_be_enumerated_without_visiting_every_member` — 전수 검사와 같은 집합을
  w_x × 멤버 + 줄당 ~110번의 방문으로, 0.05 px에서 20배 적게). **렌더러 연결(0.12.207, 2026-09-25,
  CUT_DENSITY_DESIGN §10.7):** 멤버가 한 축에서 1 px 미만이고 그 축이 반복 방향인 격자 배열은
  살 수 있는 멤버만 걷는다(`SurvivorWalk`; 0.12.208부터 미리 목록을 만들지 않고 전수 검사의 순서대로
  줄마다 등차수열들을 작은 heap으로 합쳐 하나씩 넘기며, 그리기가 멈추면(타일이 참, 취소) 바로 멈춘다 —
  2026-09-25 검토). 바깥 반복 번호(i) 방향으로 걷는 경우만 모든 줄의 커서를 한 heap에 두며 커서 상한
  65,536개(약 1.5 MB)를 넘으면 전수 검사를 쓴다. 두 방향이 모두 되면 안쪽 번호(j) 방향을 먼저 고른다.
  후보 집합은 멤버 폭의 상한(첫 멤버의
  장치 폭 + 2 단위)으로 구한 P_c 구간에 2^-40 여유를 둔 초집합이고, 후보마다 원래 규칙으로 다시
  판정하므로 픽셀은 전수 검사와 같다. 구간은 인덱스 범위의 해상도(2^(⌈log₂ L⌉+1))에서 끊어 줄당 약
  2 log₂ L 블록이다. 음수 세계 번호와 0을 가로지르는 범위는 2의 보수 잉여류로, 축 교환·음의
  pitch·회전·대칭 배치는 반복 번호→세계 번호의 부호 있는 대응으로, WIDTH_C는 P_c 상한으로 다룬다.
  목록이 멤버의 절반을 넘으면 전수 검사로 돌아간다. 가로질러 가는 축이 반복 방향이 아닌 경우(열의
  막대가 열 방향으로 가는 등 — Weyl 항뿐)도 전수 검사다. 킬 스위치는 `FLOE_RUST_SURVIVOR_LIST=off`
  (`GeometryRasterRequest::survivor_list`). 단위 테스트
  `listed_survivors_are_the_walks_drawn_members`: 무작위 600 배열(1·2차원, 8 방향, 음수, WIDTH_C 1·4)에서
  걸은 멤버 중 규칙을 통과한 것이 전수 검사의 그린 멤버와 순서까지 같다. 걷기가 적용된 145건은 8.2배
  적게 걷는다. 첫 멤버에서 멈추는 그리기에는 정확히 한 번만 넘기고, 커서는 상한을 넘지 않는다.
  `a_frame_draws_the_same_with_the_survivor_list`: 타일 2종, worker 1·3, c 1·4에서 프레임이
  바이트 동일하고 검사 멤버는 1/3 미만이다. 손으로 돌리는 `survivor_walk_in_a_nearly_full_tile_timing`
  (--ignored): 열린 픽셀 하나만 남은 타일 뒤의 200만 멤버 배열에서 래스터가 목록 켬 75 µs, 끔 102 µs
  (미리 목록을 만들던 0.12.207은 15.5 ms).
  **배치 격자(진단, 기본 꺼짐, 0.12.209, CUT_DENSITY_DESIGN §10.8):** `FLOE_RUST_PLACE_LATTICE=on`이면
  축 정렬 배치 배열 아래의 단일 도형(사각형, 다각형, path)이 그 배열의 세계 격자로 순위를 받는다. 같은 도형을
  평면 Grid로 저장했을 때와 같은 순위이며, 기준은 경로 위의 가장 안쪽 격자 배열이다. 다각형과 path는 평면
  격자 배열에서도 1 px 미만 유지 판정을 격자 순위로 한다(`lattice_area_rank`: 생존 확률은 면적 기반 그대로).
  작은 잎 셀(단일 사각형·다각형 8개 이하, 모두 1 px 미만)의 배치 배열은 생존 목록으로 살 수 있는 멤버만
  방문한다. 순위는 이 빠른 열거와 무관하게 bin·타일 walk·미룬 간선에서 같다. 단위 테스트는
  `a_placed_shape_ranks_as_its_flat_array_under_the_placement_lattice`,
  `the_placement_survivor_walk_draws_what_every_member_draws`,
  `a_deferred_placement_array_draws_as_the_walk_under_the_placement_lattice`다. 걷기의 준비는
  부모 셀 방문마다 다시 돌므로 상한이 있다(0.12.210). 그리는 레이어의 path, 아홉 번째 도형, 본 레코드(컷
  대상 포함)와 다각형 정점 합 1,024 초과 가운데 하나라도 해당하면 바로 전수 방문으로 물러난다
  (`the_placement_walk_preparation_is_bounded`). 걷기의 결과(걸음, 또는 전수 방문의 사유 15가지)는
  1·2차원 배열별로 `RenderStats::place_walks`에 세어 renderd `place_walks=`와 워커 결과로 내보낸다
  (0.12.211, `placement_walk_outcomes_are_counted`). 생존 걷기의 비용 모형은
  블록 수를 항의 확률로 추정하고 멤버 방문을 레코드 배열 4, 배치 배열 16블록으로 센다(0.12.209).
  **밀도 스택(진단, 기본 꺼짐, 0.12.226/renderd 0.12.216, CUT_DENSITY_DESIGN §10.10):**
  `FLOE_RUST_DENSITY_STACK=top`이면 프레임을 두 번 그린다. 1패스는 계획(컷 3 px, max)을 지금처럼 칠하며 —
  그려지는 도형은 헤어라인까지 모두 원본 — 원본이 덮은 영역(스페클·패턴 구멍, clear 내부 포함)을 기록하고,
  2패스는 같은 뷰를 컷 0.5 px로 다시 계획한 장면에서 긴 변이 1패스 컷 미만인 레코드(밀도)를 위 평면부터
  그린다: 최상위 평면의 밀도는 자기 원본이 쓰거나 덮은 곳 밖이면 하위 원본 위에도 덮어쓰고, 나머지 평면의
  밀도는 어떤 원본도 쓰거나 덮지 않고 위 밀도가 차지하지 않은(떨어진 도형의 순위 0 픽셀 포함) 픽셀에만
  쓴다(`DensityStack`, write-once 타일에서만; LayerRasterSession의 두 블록 — 2패스 페이지는 블록 경계에서
  1패스의 마스크로 걸러 디코드한다, `BlockDemand::density_pages`). 격자 배열의 차지 영역은 배열
  단위(`array_footprint`)라 생존 걷기 켬·끔과 같다. 단위 테스트는
  `the_density_stack_draws_the_cut_shapes_in_the_top_plane_and_the_empty_space`,
  `a_dropped_array_member_claims_its_pixels_under_the_density_stack`,
  `the_density_stack_draws_one_solid_plane_as_the_finer_cut_and_every_path_alike`다.
  **같은 세계 박스의 중복**(단일 사각형 둘, 같은 Grid 둘, Grid와 그 조각)은 한 벌과 같은 픽셀을
  켜고 레코드 순서도 무관하다(단위 테스트 `duplicates_of_a_shape_draw_as_one`); Grid 멤버 위의 단일
  사각형은 다른 경로(자기 세계 박스 해시)라 두 결정의 합집합 — 같은 박스 중심에 놓이므로 더 넓은
  쪽 — 을 켜고 그 밖은 켜지 않는다. 비스듬한 격자는
  레코드 번호를 역산하고(u는 그 배치의 첫 멤버 세계 박스 순위, 한 줄 배열의 번호는 배열이 놓인
  방향의 축에 vdc), 공선 2차원 격자·점 목록(Pts)·단일 도형은 세계 박스 해시. 그 픽셀들에 레이어 fill, 테두리가 외곽선. 명세: **개별 폭의 정수 부분과 평균 폭을
  보존하고, 좁은 간격과 픽셀 충돌에는 양자화 오차를 허용한다.** 폭은 pan에 불변(도형이 통째로 옮겨질
  뿐)이고 축소하면 단조로 줄며(1px 미만 변은 w > t일 때만 — 축소하면 부분집합), 상자는 도형이
  건드리는 픽셀을 벗어나지 않아 실제 간격이 2 px 이상이면 1 px 이상 남는다. 2 px 미만 간격은 닫힐 수
  있고(예: [10.1, 11.6]·[12.6, 14.1] px의 1.5 px 막대 둘이 모두 2 px면 [10, 12)·[12, 14)), 겹쳐 그려지는
  정도가 pan으로 달라져 전체 켜진 픽셀 수는 개별 폭과 달리 pan에 따라 조금 변한다. (2) **다각형·경로**:
  픽셀 중심이 안에 드는 픽셀을 칠하고 외곽선은 그 집합의 안쪽 테두리(행 스팬의 끝과 위·아래 행이
  덮지 않는 부분 — 밴드 밖·화면 위쪽 밖의 행도 훑어 타일·화면 가장자리와 무관). 비스듬한 변은 폭이
  하나로 정의되지 않아 폭 우선이 아니며, 폭·간격·밀도는 **소수 픽셀 위치에 대한 평균으로만** 보존된다.
  1px 미만 변이 있는 다각형·경로는 그 변을 중심 한 픽셀, 긴 변을 픽셀 중심 스팬으로 그리되 **자기
  면적 ÷ max(w,1)·max(h,1)**의 비율로 남긴다(세계 박스 해시 순위; bbox 면적이 아니다).
  측정(게이트, 0/¼/½/¾ px pan의 켜진 열 비율 평균): 배열 격자 1.5/1.5 px 0.500 전 위상(픽셀 중심일 때
  0.667/0.333/0.333/0.667, 해시일 때 0.513), 7.6/2.4 px 0.768(덮임 0.767; 해시 0.742), 비정수 pitch
  2.7/2.1 px 0.559(0.558; 해시 0.521), 1px 미만 0.1/0.9 px 0.103(0.100; 해시 0.147); 배열이 아닌 폭이
  섞인 이웃(해시) 0.516(0.524).
  표시용 점(wash, wash 점, 저장 대표점)은 컷 아래를 대신 보이는 점이라 KLayout 규칙 그대로다.
  exact 프레임과 `floe-render-cli`는 KLayout 규칙이고,
  renderd의 `FLOE_RUST_AREA_TRUE=off`가 킬 스위치다. 게이트 `validate_area_true`.
  **추가 성김 보정(진단, 2026-09-23; ADAPTIVE_CUT_DENSITY_PLAN §4.2 후보 1)**: renderd의
  `FLOE_RUST_WIDTH_C=c`(c ≥ 1, 기본 1 = 위 규칙 그대로; 범위 밖 값은 1)는 사각형의 추가 픽셀을
  t < P_c(f) = f / (c − (c − 1) f)일 때만 준다(`GeometryRasterRequest::width_c`). c = 2면 0.05 px 변은
  20개 중 하나가 아니라 39개 중 하나, 1.99 px 변은 평균 1.98 px(f / c였다면 1.495 px로 2 px에서
  뛴다) — 정수 폭에서 연속이고 w에 단조이며 상자는 c = 1의 상자 안에 있다(floor(w) 아래로는
  내려가지 않는다). 의도적으로 어둡게 하는 근사라 기본은 1이다. **실칩 관측(2026-09-23, 사용자)**:
  c = 1·2·4·16·128을 비교해도 크게 달라지는 부분은 없고 미세한 차이만 있었다 — thin keep에서는 컷
  아래 도형이 모두 잘려 이 보정이 컷 이상 도형의 소수 부분에만 걸리기 때문으로 본다. 기본 1 유지. 단위
  테스트 `extra_sparsening_thins_the_extra_pixels_continuously`; 게이트: c = 2에서 1.5 px 배열의 네
  pan 열 비율이 덮임의 (1 + 1/3)/1.5, 0.5 px 배열은 덮임의 2/3, 새로 켜지는 열 없음(스티플 안쪽 열이
  테두리가 되면서 픽셀은 달라진다), 0.5는 c = 1과 픽셀까지 같다.
  측정(합성 1/10 칩, 1920×1080, thin keep, warm 래스터, 끔 → 켬): 3 px 컷은 잡음 범위(전 레이어 fit
  314 → 379 ms, ×4 372 → 324 ms), 1 px 컷은 전 레이어 fit 1113 → 783 ms, 마지막 10레이어 53 → 36 ms,
  ×16 313 → 313 ms. 다각형 테두리는 그 다각형의 행만 훑는다(타일 높이만큼 훑으면 ×16이 2배였다).
  리뷰(2026-09-22)로 고친 두 결함: 화면 위쪽에서 잘린 다각형이 맨 위 행에 가짜 테두리를 그렸다
  (스캔이 0행부터 시작해 화면 위 이웃 행을 빈 행으로 봤다 — 32×32 뷰 전체가 다각형 내부일 때 채움을
  끄면 32 px가 켜졌다); 1px 미만 다각형을 bbox 면적으로 남겨 0.8×0.8 px 삼각형이 같은 bbox의
  사각형만큼 켜졌다(지금 900개에 284 px 대 575 px).
  한계: 한 픽셀에 남은 도형이 여럿 떨어지면 겹쳐 조밀한 1px 미만 내용이 덮임보다 어둡게 보이고
  (0.1/0.1 px 격자 ×0.85, 0.25/0.25 ×0.65~0.84), 1px보다 훨씬 가는 선만 있는 레이아웃은 넓은
  뷰에서 면적만큼 흐려진다 — 고립된 가는 도형은 존재 표시 없이 비율대로 사라진다.
- **프레임(계층 프런티어)**: r==0 경계 셀 전용 + 멤버 컷, 40px
  2톤, 헤어라인은 7µm 격자 대표 샘플링(rev 45; `FLOE_THIN_UM`,
  `FLOE_HAIRLINE` 노브 의미 포함).
- **레이어 fill**: 스펙클 등 패턴은 **전 레이어 공통 위상**(구멍
  공유), 불투명 겹침 — `validate_render_speckle`이 고정. 패턴
  이름/비트맵은 flateyes와 공유하는 FILL_PATTERNS 테이블.
- **DRC 오버레이**: 에러 폴리곤 fill = **solid 50% alpha**(불투명
  체커 금지 — 재제안 금지 항목), 외곽 2px, halo 없음. 캡처
  (fe_embed)와 화면이 동일해야 한다.
- **LOD 변종**: 파생 데이터 — 커버리지 superset + 1셀 과피복 한도
  (`lod_coverage_superset_and_bounded` 유닛), 플래너가 밀도
  게이트로 스왑. 렌더러는 exact/LOD 페이지를 구분 없이 그린다.
- **커버리지(v)**: design.ovc 비트플레인 → 팔레트 합성, 스펙클
  구멍 안으로 침투 금지(`validate_render_speckle`의 포함관계).
- **라벨**: 90도 회전/폰트 크기 규약(#55에서 Cairo 오버레이로 이동
  예정 — 렌더러 대체와 합류 지점).

## 4. 오라클 게이트 — 렌더러가 통과해야 할 것

기존 게이트는 "klayout 대비" XOR이므로, 렌더러는 같은 자산·같은
뷰에서 klayout 오라클과 대조된다:

| 게이트 | 내용 | 렌더러 적용 |
|---|---|---|
| `validate_vfs_render` | hier 델타 → 6뷰 × 9레이어 렌더 XOR | 그대로 (렌더 함수만 치환) |
| `validate_render_speckle` | 공통 위상·가시성·불투명 겹침·커버리지 포함관계 | 그대로 |
| `validate_render_frames` | 회색<디자인<흰 페인트 순서, 1px 외곽 | 그대로 |
| L1~L9 (`validate_vfs_lifecycle`) | 세션 수명주기(apply 대체 시 필수) | apply를 렌더러 쪽으로 바꾸면 필수 |
| X1~X6 (`validate_vfs_text`) | 라벨 응답/declutter | 라벨 그리기 치환 시 |

### 픽셀 동일성 정책 (2026-08-22 확정)

klayout과의 완전 XOR=0은 요구하지 않는다(서브픽셀 규약 차이는
정당). 이 절의 대조는 **KLayout 규칙**(exact, `floe-render-cli`, renderd의
`FLOE_RUST_AREA_TRUE=off`)에 대한 것이다 — 뷰어 기본인 area-true(§3)는 의도적으로 다르며,
occupancy·jobdeck·representatives 게이트는 기준 렌더를 KLayout 규칙으로 고정해 비교한다(종전
계약의 확인이지 새 기본 경로의 검증은 아니다). 새 기본 경로는 `validate_area_true`와
write-once(두 규칙 모두 바이트 동일), layer_decode(고정 없음)가 덮는다. 대신 **바이너리 커버리지**(레이어당 on/off, oversampling 1,
AA 없음) 기준으로 아래 세 규칙을 모두 만족해야 한다 — 구현·자기
검증은 `tools/validate_render_goldens.py`:

- **P-a (경계 밴드)**: diff 픽셀은 골든 경계로부터 Chebyshev ≤1px
  밴드 **안에서만** 허용. 내부 diff 1픽셀 = 실패. (1px 위상
  시프트는 통과, 2px 시프트는 실패 — 자기검사로 고정.)
- **P-b (소멸 금지)**: 골든의 4-연결 성분은 크기 무관 후보와
  겹침 ≥1px — 1px 피처가 밴드 속으로 사라지는 것을 잡는다.
- **P-c (면적 드리프트)**: |on(후보)−on(골든)| ≤ max(16,
  0.75×골든 경계픽셀수) — half-open 채움 규약 차이(≈0.5×둘레)는
  허용, 전변 1px 성장(≈1.3×둘레)은 거부.

색·패턴·합성(스펙클/프레임/DRC 워시)은 이 정책의 대상이 아니고
§3의 speckle/frames 게이트가 따로 고정한다.

#### 헤어라인 스케일 실측 (2026-08-27 — 정책 결정 대기)

실칩 ~100µm 뷰 현장 관측("헤어라인이 다르게 처리되어 화면이 다름")을
`tools/hairline_ab.py`로 재현·분류했다 (858px/100µm, 1px≈0.117µm,
내부 diff **0** — P-a/P-b/P-c 계약 자체는 지켜진다):

| 클래스 | KLayout | Rust | 규모 |
|---|---|---|---|
| sub-pixel 점(0.05µm box × 300) | 항상 정확히 1px | 걸친 픽셀 전부 (2px 144개, 2×2 48개) | on 300→588 |
| 축정렬 극세 wire(0.03µm) | 한 행/열 | 스트래들 시 1px 위상 시프트/이중행 | diff 7,482 |
| 대각 극세 wire | 얇게 | 약간 굵게 | +982/-439 |

근원: **KLayout은 sub-pixel 도형을 edge 최근접 반올림으로 픽셀
하나에 collapse**하고(32px 정렬 프로브로 확인 — 분수 위치 ≥.5면 다음
픽셀로 스냅), **Rust는 2-phase fill + outline stroke가 도형이 걸친
모든 픽셀을 점등**한다. 정수 스케일에선 스트래들 셀에서만 갈라지지만
비정수 스케일(실뷰)에선 대부분의 sub-pixel 도형이 걸치므로 화면
질감(밝기·굵기)이 달라진다. Rust 출력은 KLayout의 superset에
가깝다(점 클래스 missing 0).

수렴 옵션: (A) device bbox가 축별 sub-pixel인 도형을 KLayout 규칙
(최근접 격자 collapse)으로 스냅 — representation-exact 계약을 지키려
rect/polygon/path 공통의 device-bbox 단계에서 적용해야 하며 대각
클래스는 잔차로 남는다. (B) 현행 유지(계약상 정당, 소멸 없음) +
제품 문서에 명시.

**A 채택·구현 (2026-08-27)**. KLayout 규칙을 세 프로브(32px 정렬
`hairline_probe`, 858px 분수 스윕 `kl_rule_probe`/`kl_switch_probe`/
`kl_wire_probe` — scratch 스크립트, 재현은 `tools/hairline_ab.py`)로
실측해 다음을 확정하고 `hairline_world_bbox`/`hairline_axis_span`
(raster.rs)에 구현했다. 조건: solid stroke + world bbox의 device
span이 한 축이라도 1px 미만. 해당 member는 fill+stroke 파이프라인을
건너뛰고 collapse된 rect를 member 색 solid로 칠한다(KLayout도
sub-pixel은 outline line으로 그리므로 패턴 미적용이 맞다).

- **점(양축 sub-pixel)**: `round(center)` 단일 셀, **y축은 −1 bias**
  (32px·858px 프로브 공통 실측). 0.8px box는 KLayout과 전 위치 일치,
  0.43px는 임계가 ~0.53이라 위치의 ~4%만 1px 오프.
- **wire(한 축만 sub-pixel)**: 좁은 축은 **양 edge의 round 픽셀**
  (edge가 갈라지면 2열 — 0.26px wire 기준 KLayout과 V 18/20·H 20/20
  일치), 긴 축은 edge-snap span.
- 대각(양축 모두 1px 이상인 얇은 도형)은 비대상 — 기존 경로.
- dotted stroke(frame band 3)는 비대상 — band 스타일 유지.
- **stroke width > 1도 비대상** (2026-08-28 리뷰 HIGH 수정): 최초
  구현이 StrokeStyle만 보고 collapse해 굵은 외곽선(2~8px)이 1px로
  뭉개졌다 — w4 A/B에서 l1-axis KLayout 84,303 vs Rust 24,158px,
  interior missing 44,206(밴드 밖 회귀). width==1 gate 후 84,042 vs
  84,303(±0.3%), interior 257로 수렴. `tools/hairline_ab.py
  <workdir> [width]`가 w별 A/B를 재현하고, unit oracle
  `hairline_fast_path_requires_unit_stroke_width`가 w {1,2,4,8}
  sweep과 w4 representation-exact를 고정한다. KLayout의 폭별
  collapse 규칙(폭 w hairline이 어느 픽셀들을 얻는가)은 측정 후
  별도 채택 판단으로 남긴다.

효과(`tools/hairline_ab.py`, 858px/100µm): sub-pixel 점 300개 on
588→**300**(KLayout과 동수), 그중 285개는 픽셀까지 일치. hairline
wire 밀도 19,532→24,158 vs KLayout 24,510(±1.4%). 내부 diff는 전후
모두 0. 잔여(계약 내 band): (1) 전역 y anchor 차이 — KLayout은 큰
도형의 top edge를 한 행 위까지 칠한다(이번 변경 이전부터 있던
관습 차이, oracle band로 흡수), (2) 소형 box 임계창 ~0.05px, (3)
대각 클래스. gate: `hairline_point/wire/representation/dotted` unit
4종(P-b 비소멸 포함) + 기존 oracle/골든 배터리.

### 래스터 골든 (PX1~PX5)

`tools/validate_render_goldens.py` — klayout 오라클로 마이크로
픽스처를 정확히 고정된 뷰포트에서 구워 두고, 렌더러 출력물을 위
정책으로 대조한다. **골든은 커밋하지 않는다**(klayout 버전·호스트
종속 — manifest에 버전 기록, 불일치 시 자동 재베이크):

| 케이스 | 내용 |
|---|---|
| PX1 | 반픽셀·¼픽셀·음수 원점·원점 교차 뷰포트 반올림 (정렬 박스 격자) |
| PX2 | 엣지 기울기: 수평/수직/45°/atan(1/3)/atan(2/7)/atan(3) 웨지 |
| PX3 | 1~8px 선폭, 수평/수직/45° |
| PX4 | concave 폴리곤: L/U/plus/comb/예각 노치 (vertex/join 픽셀) |
| PX5 | PATH flush/square/round/비대칭 extension + 90°/45°/135° 꺾임 |

각 케이스는 정렬 뷰와 반픽셀 오프셋 뷰 양쪽으로 렌더된다(총
13뷰). 하네스 자기검사: 재렌더 == 골든(결정성), 정책 판별력
(shift1 통과/shift2 실패/dilate 실패/성분 소멸 실패). 렌더러
프로젝트 사용법: 같은 픽스처(.oas는 workdir에 생성됨)·같은
뷰(manifest.json)로 렌더한 PNG를 `--candidate DIR`로 대조.

`tools/validate_klayout_oracle.py`는 별도 half-phase fixture에서 같은 월드
사각형을 RECTANGLE/POLYGON/PATH로 각각 기록한다. KLayout 세 mask의 exact
동일성과 Rust 세 mask의 exact 동일성을 각각 요구하고, 엔진 사이에는 위의
P-a/P-b/P-c를 적용한다. 이 gate는 primitive 종류에 따라 fill phase가 달라지는
회귀를 1픽셀 허용 밴드 뒤에 숨기지 않는다.

**대체 시점에 새로 정할 것** (잔여):

1. **pick/snap 동등성** — 현재 klayout Layout 쿼리로 구현
   (`service.py` `_SNAP_CAP=400`/`_PICK_CAP=64`). 렌더러가 지오메트리
   상주를 가져가면 같은 캡·같은 겹침 순환 의미를 보장해야 한다.
2. **[perf] A/B 벤치** — 아래 기준선 대비 회귀 금지.

## 5. 성능 기준선 (렌더러 대체의 성공 지표)

- 뷰어 병목은 **klayout OASIS 파스 단독**이다(M3.5 실측: vfsd
  0.04s vs 전체 첫 페인트 8.4s → 예산 스트리밍으로 0.93~1.08s).
  즉 파스+적재가 빨라지면 스트리밍 라운드 체계 자체를 단순화할
  여지가 생긴다.
- 9.8G 실칩(2026-08-22, 사무실): 대부분 Calibre 동급 이상, 첫 방문
  2–3s(콜드), 재방문 고속. **풀depth+mid zoom 첫 방문 9–10s 블랙**
  (#57 싼-첫-페인트와 동일 지점) — 렌더러 대체가 직접 개선해야 할
  1차 케이스.
- 측정 도구: 뷰어 터미널 `[perf] gen=.. round=.. new=.. bytes=..
  plan=..ms delta=..ms apply=..ms draw=..ms total=..ms lod=..
  refining=.. settled=..` (라운드별 비누적). 렌더러 A/B는 같은
  뷰 시퀀스에서 apply+draw 합으로 비교한다.

## 6. 픽스처

| 자산 | 위치/생성 | 용도 |
|---|---|---|
| valmini | `tools/gen_valmini.py` (스위트가 $TMPDIR에 생성; `data/m1/.valmini*.ice`는 0.11.44로 최신화됨) | 작은 적대 자산: 회전/미러 배치, 3계층, 비맨해튼, 텍스트 |
| sample9 | `tools/gen_sample9.py` | 145MB depth-9, 성능/실측 |
| repfloor / p2floor | `tools/validate_vfs_split.py`가 생성 | rep-flood(Pts/Grid 대량), 몬스터 셀 |
| frametest / thintest | `tools/gen_frametest.py` / `gen_thintest.py` | 프레임 톤/스택, rev 45 격자 |
| bench_m6fill | `data/bench_m6fill.oas` (166MB) | 단일 레이어 fill 팜 |
| 실칩 | 사무실 (9.8G 등) | 최종 실측 |

## 7. 실행 요약

```sh
# 캐시 만들기 + 플랜 확인
rust/target/release/floe-index vfs design.oas
rust/target/release/floe-index plan .design.oas.ice --mode hier \
    --view x0,y0,x1,y1 --px-per-um 5 --depth 0

# 렌더러 브링업: floe-vfs로 페이지 바이트 직접 획득
#   let v = floe_vfs::Vfs::open(".design.oas.ice")?;
#   let plan = floe_vfs::hier::plan_hier(&v.ovm, &req, &opts);
#   let pages = v.read_page_batch(&plan.pages)?;  // (idx, OASIS bytes)

# 오라클 게이트 (klayout 필요: .venv)
.venv/bin/python tools/validate_vfs_render.py <src.oas> <cache-dir>
.venv/bin/python tools/validate_render_speckle.py
.venv/bin/python tools/validate_render_frames.py
.venv/bin/python tools/validate_render_goldens.py            # 베이크+자기검사
.venv/bin/python tools/validate_render_goldens.py . --candidate out/  # 렌더러 대조
sh tools/validate_rust.sh        # 전체 스위트 (RUST VALIDATION: ALL OK)
```
