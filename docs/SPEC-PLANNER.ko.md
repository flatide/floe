# SPEC: 계층 플래너 (rust/vfs/src/hier.rs)

정본 이력: `rust/VFS_HIER.md` (rev 1~46b — 모든 설계 결정과 현장
근거가 여기 있음. 규칙을 바꾸기 전 반드시 해당 rev를 읽을 것).
텍스트/라벨: `rust/vfs/src/text.rs`. 세션/델타: `rust/vfs/src/lib.rs`.

## 1. 개요

`plan_hier(&Ovm, &ViewReq, &HierOpts) -> HierPlan`

- **WsKey = (cell ci, 남은깊이 r)**, r=REM_FULL은 무절단. topo_rank
  min-heap 1패스: 부모가 항상 먼저 확정되어 localview(K-box, k_boxes=4,
  최소낭비 병합)가 완성된 뒤 자식이 팝된다(고정점 불필요).
- 출력 HierPlan: wcells(페이지 선택 + 자식 edge + 프레임 + 워시),
  pages(+스트리밍 우선순위 = 뷰 중심 거리²), stats.
- ViewReq::root (2026-09-29, 뷰어의 뷰 루트 — SPEC-VIEWER §8c): Some(ci)면
  플랜이 그 셀에서 그 셀의 좌표로 출발하고 depth도 그 셀부터 센다; None
  또는 셀 테이블 밖이면 탑(`plan_hier`의 `top_ci`, `text.rs plan_labels`
  동일; brute 오라클도 같은 규칙). HierPlan.top = 루트. 유닛
  `a_view_root_plans_that_cell_as_the_top_in_its_own_coordinates`.
- ViewReq: view(BBox dbu), cut_dbu, vis(레이어 비트마스크), depth,
  px_per_dbu(0 = 프로브/톤 없음).

## 2. HierOpts (기본값)

| 필드 | 기본 | 의미 |
|---|---|---|
| k_boxes | 4 | WsKey당 localview 박스 수 |
| pts_full_rep | 8192 | 이하 Pts는 전체 rep 방출, 초과는 청크 스캔/프레임 풋프린트 |
| pts_enum_budget | 200_000 | 요청당 오프셋 가시성 테스트 상한(소진=통째 포함) |
| frame_cap | 200_000 | 플랜 전체 프레임 엔트리 상한 (0=프레임 off) |
| lod_k | 4.0 | LOD 밀도 게이트 계수 (0=off) |
| wash_px | 2.0 | 워시 문턱 px (0=off); 요청의 `page_wash`가 꺼져 있으면 쓰지 않음(§3) |
| hairline | 0.5 | rev 41 min변 컷 계수 (0=off) |
| thin_lattice_um | 7.0 | rev 45 프레임 격자 피치 µm (0=rev 41 프레임 컬 복원) |
| thin_demote_px | 14.0 | 격자 1피치 화면 px가 이 미만이면 빈당 2→1 강등 |
| sub_cut_walk_budget | 200_000 | sub-cut 노드를 배치·페이지까지 내려가는 플랜당 걸음 수(소진=coarse wash) |
| sub_cut_sparse_px | 16 Mpx | sub-cut 희소 항목의 ink 예산(플랜당 화면 px, 소진=버림; §3) |
| sub_cut_wash_px | 64 Mpx | sub-cut wash 면적 예산(플랜당 화면 px, 소진=버림; §3) |

## 3. 컷/생략 사다리 (정확한 술어)

- 페이지: `(max_w<cut && max_h<cut) || max_min<page_hair` (v6 필드; 선형·
  pbvh 리프 동일). **page_hair는 요청의 정책**(`ViewReq::page_hairline`,
  2026-09-11 리뷰: 공유 기본값이 아니라 요청마다 명시): `true`(일반 레이아웃의
  성능 정책 — 모든 레코드가 가는 페이지를 광역 뷰에서 통째로 버림)면 hairline ×
  cut, `false`(마스크/jobdeck 정책)면 0. 실칩(2026-09-10)에서 81~124 nm 폭·최대
  119 µm 길이의 선 영역이 210 µm 뷰부터 지워진 것이 이 규칙이었다(4페이지 모두
  `cull_hair`, 문턱 0.128 µm에 4 nm 차이). raster는 남긴 가는 레코드를 전체
  길이의 1 px 선으로 그리고(KLayout hairline parity) `thin_pages_kept`로 센다.
  **sub-cut 페이지**(`ViewReq::sub_cut_wash`; 덱 pass는 2026-09-10부터, 단일
  레이아웃 요청은 2026-09-16부터 켰다가 **같은 날 사용자 결정으로 둘 다 기본 off**:
  느려지는 부작용에 비해 여전히 다 보이지는 않고, 덱은 점유 요약이 광역뷰를
  맡는다. 진단 `FLOE_RUST_SUB_CUT_WASH=on`(단일 레이아웃)·`FLOE_RUST_DECK_WIDE=on`
  (덱)으로만 켠다. 켰을 때의 규칙 — 현장: 9.8 GB 일반 레이아웃이 detail high에서
  Calibre보다 훨씬 적게 보임, 모든 도형이 cut 미만인 페이지가 통째로 `cull_size`
  됐다): 크기 컷(`max_w<cut && max_h<cut`)에 걸린 페이지는 버리지 않고,
  footprint 대비 멤버 채움이 1/256 이상이면 레이어 색 footprint wash
  (`sub_cut_washes`), 미만이면 페이지를 남겨 멤버를 픽셀로 그린다(`keep_sparse`,
  `sub_cut_sparse`; JOBDECK §11 4단계). 배치 BVH의 sub-cut 노드도 같다
  (`wash_sub_cut_child`). hairline 컷 항목(cull 정책, `Hier::hair_cut`)도 같은
  규칙이되 채움 문턱이 `WASH_MIN_COVERAGE_HAIR` = 1/8이다(선은 길이만큼 픽셀을
  세므로 빈 페이지의 긴 선 셋은 3~4 %로 남아 정확히 그려지고, 밀집 배선은 wash;
  진단 `FLOE_RUST_WASH_HAIR_COVERAGE`). 사용자 결정 2026-09-16: 9.8 GB 레이아웃의
  요약 생성이 한 시간을 넘어, 요약 없이도 광역뷰에 존재가 보여야 한다. keep은
  hairline 페이지를 그대로 남긴다(page_hair = 0). exact 요청은 제외. 켜는 스위치
  `FLOE_RUST_SUB_CUT_WASH=on`(단일 레이아웃), `FLOE_RUST_DECK_WIDE=on`(덱; 둘 다
  진단 전용, 기본 off); gate `validate_occupancy` `SubCutTests`(on 워커가 규칙을
  검증), `validate_jobdeck` `WideViewTests`·`ThinPageTests`.
  **별도 대표 파일 OVR1**(0.12.154, opt-in): 일반 뷰어의 cull 플랜 뒤에
  `design.ovr`의 네이티브 점만 보충한다. 원본 페이지 선택은 늘리지 않으며,
  depth·레이어·컷 필터와 화면 밀도/전역 점 수 상한을 적용한다.
  [REPRESENTATIVES](REPRESENTATIVES.ko.md) 참조. 아래 page frontier와 독립 경로다.

  **대표(page frontier) — 비활성**(사용자 결정 2026-09-17 저녁: 0.12.152에서도 fit
  뷰가 박스이고 depth 99 플랜이 60 s를 넘어, 인덱싱 때 대표 데이터를 별도 파일로
  만드는 방식으로 전환. 뷰어 기본 off, `FLOE_RUST_PAGE_REPS=on`으로만 켠다 — 진단.
  아래는 플래너 쪽 구현의 기록이다.)
  (`ViewReq::page_reps`, 사용자 설계 2026-09-17, 같은 날 리뷰 조건으로 단계화, 같은
  날 현장 4·5·6차로 레벨을 **화면 밀도**로 결정; 켜면 단일 레이아웃 요청에만, 덱
  pass·exact·probe는 off): 컷이 버리던 것을 없애지 않고
  **2^L개 중 하나**를 남긴다. L은 항목마다 그 항목의 ink 대 화면 면적으로 정한다
  (`density_level`): ink = 멤버 수 × max(1, 최소변 px) × max(1, 긴변 px)(멤버가 칠할
  수 있는 상한), 면적 = 항목 bbox의 화면 px(축당 최소 1), ink / 2^L ≤
  `HierOpts::rep_density`(기본 0.25, `FLOE_RUST_REP_DENSITY`) × 면적인 최소 L. 밀집
  배열·hairline 밭은 밀도의 점·선 무늬로 솎이고 희소 페이지는 그대로 그려진다. 축소하면
  면적이 옥타브당 1/4로 줄고(멤버는 1 px에서 멈춤) L이 2 오르며 1/4이 남는다("4개 중
  1개"); 항목별이라 화면 이동에 안정하다(앞서 시도한 프레임 전역 항목 예산은 밀집
  배열은 픽셀을 다 채우고 희소 영역은 비게 했으며 이동마다 흔들렸고, 그 전의 run 상한은
  작은 run을 모두 레벨 0으로 만들어 블록을 채웠다). 솎기는 경로 전체에서 **한 번만**
  적용하고 단계가 나눠 맡는다.
  - 배치 단계(플래너): 화면에서 밀도의 점 간격(1/√밀도 px, 기본 2 px) 이하인 컷 자식
    BVH 서브트리는 **점 하나**(중심의 1 px, 셀의 보이는 재귀 레이어; `rep_node_dot`)로
    끝나고 아래를 걷지 않는다 — 걷기 비용이 배치 수가 아니라 화면 크기에 묶인다(현장
    2026-09-17: 컷 서브트리를 모두 내려가자 full depth 전환이 80 s를 넘겼다). 그보다
    넓은 노드는 자식으로 내려가고, 리프의 컷 배치는 자기 footprint로 L을 정한다. 컷
    배치의 반복 멤버가 lm = min(L, ⌊log2 멤버 수⌋)를 맡아 2^lm개 중 하나(Grid는 축
    균형 stride `thin_grid`, Pts는 2^lm번째 slot; 멤버 0은 남음)를 남기고, 셀 안 배치
    index가 남은 L − lm을 맡는다(`place_rep`). 남긴 멤버는
    각각 **자식 bbox 한 개**를 자식의 보이는 레이어에 그린다(`rep_dots`, explain
    `rep_dots`): 컷 아래라 화면에서 cut px 이하의 점(hairline 셀은 가는 띠)이고, 그
    줌에서 인스턴스의 그림 그 자체다. 자식 페이지를 디코드하지 않고 자식 아래를 걷지도
    않는다(현장 2026-09-17: 자식을 통째로 그리면 픽셀 하나를 위해 자식의 모든 페이지를
    디코드했고 블록이 박스로 채워졌다). 배열 footprint 하나를 wash하지도 않는다(같은 날의
    fit 뷰 박스 하나). 세는 pass도 인스턴스 하나 = 항목 하나로 센다.
  - 페이지 단계(플래너): 페이지는 그릇이므로 뷰 안의 컷 페이지를 모두 남기되, 디코드
    예산 — `HierOpts::rep_decode_bytes` 256 MiB와, 렌더러의 세대 예산
    (`ViewReq::decode_budget`)에서 플랜의 다른 페이지 바이트를 뺀 나머지의 절반 중
    작은 쪽 — 을 `rep_decode_bytes` 합이 넘으면 플랜을 다시 해 페이지를 run 안 index로
    2^Lp개 중 하나만 남긴다(`rep_page_level`, `rep_replans`; 진단
    `FLOE_RUST_REP_DECODE_MB`). 남긴 페이지는 자기 L − Lp를 `WsCell::page_levels`로
    래스터에 넘긴다.
  - 레코드 단계(래스터, `thin_record`): 레코드의 반복 멤버가 min(level, ⌊log2 멤버 수⌋)를
    맡고 페이지 안 index가 나머지를 맡는다(2^lr의 배수만, index로 바로 건너뜀).
  집합은 frontier 격자 대표처럼 **아래로 포함**된다(S(L+1) ⊆ S(L): 넓은 뷰에 보이는
  것은 가까운 모든 뷰에도 있었고 축소 중 새로 나타나는 것은 없다). run의 첫 항목
  (index 0)은 어느 줌에서든 후보다. BVH 프루닝: 페이지 BVH는 run 구간에 2^Lp의 배수가
  없으면 건너뛰고(Lp = 0이면 모두 내려간다), 자식 BVH는 점 간격 이하 노드에서 멈춘다.
  뷰 안 컷 페이지의 메타데이터는 모두 읽는다(O(뷰 안 컷 페이지)). 대표 페이지는 M7-C
  `wash_px` 붕괴(2 px 이하 페이지 → bbox 렉트)를 타지 않는다 — 그 렉트가 fit 뷰의
  "박스"(2×2 px 정사각형이 이어진 면)였다.
  대표는 sub-cut 예산을 타지 않는다. 진단용 sub-cut 규칙의 wash 판정에 쓰는 ink
  추정은 멤버 수 × 최소변 × 긴변(px). 대표는 부분만 보이는 무늬이지 요약처럼 채워진
  면이 아니다. 킬 스위치 `FLOE_RUST_PAGE_REPS=off`(뷰어), `floe-index plan
  --page-reps 1`; 상태줄·perf 줄 `reps K pages/C children [L n] [P n]`.
  gate `PageFrontierTests`(121만 hairline이 32 페이지: 모든 줌에서 32 페이지 모두 남고
  L이 줌인할수록 내려감; 12,100선의 희소 레이어는 네 사분면이 켜지고 200/400/800 px의
  픽셀 밀도가 서로 3배 안·0.5 미만; `FLOE_RUST_REP_DENSITY=0.03125`면 L +3으로 약 1/8
  픽셀·포함 관계; `FLOE_RUST_REP_DECODE_MB=1`이면 re-plan해 16번째 페이지만·구간
  프루닝; L 두 선은 wash 없이 선 두 개; 킬 스위치는 0 px), `SubCutTests`(밀집 배열·
  배치·선 밭은 점·선 무늬, 희소는 그대로), `ThinPageTests`·`test_render_detail…`.
  **점유 요약 레이어**(`ViewReq::page_skip`, OCCUPANCY_PLAN M2): 비트셋에 든
  레이어는 페이지 범위(prange)에서 통째로 건너뛰어 선택·디코드가 없고
  `summary_pages`로 센다. 순회(자식·프레임·다른 레이어)는 `vis` 그대로이되,
  `ViewReq::prune_skipped`(renderd·덱: 프레임 없는 요청)면 서브트리 판정에
  `vis − page_skip`을 써서 요약 전용 서브트리를 걷지 않는다(`walk_vis`). wash
  마스크는 언제나 `vis − page_skip`(`wash_vis`). 오라클 `brute_with`도 같은
  마스크를 쓴다.
  기본: renderd 프레임의 `thin=keep|cull`(덱 worker keep, 일반 worker cull;
  뷰어 `--thin`/View 메뉴, `floe2 render --thin`), `floe-index plan
  --page-hairline 0|1`(기본 1). 진단 override `FLOE_RUST_PAGE_HAIRLINE=cull|keep`.
  자식 폴드·생략, BVH 프루닝, 프레임의 hairline은 두 정책 모두 그대로.
- 인스턴스 BVH 노드(rev 43): `(max_dim<cut) || (max_min<hair_prune)`,
  단 **hair_prune은 r==0 && thin_dbu>0이면 0**(rev 45 — thin 서브트리를
  방문해야 격자 샘플 가능; 양변 프루닝은 유지).
- r>0 유한 깊이 폴드: 자식 rbbox `(w<cut&&h<cut)||min<hair` → **침묵
  폴드**(rev 33: 프록시 박스 없음. 박스는 자기 깊이 경계에서만 — rev 37).
- REM_FULL 자식: 동일 술어로 생략(레이어별 프록시 없음 — 가짜 지오메트리
  금지).
- LOD 스왑: 충실도(실방출 셀 ≤1px 양축) AND members > lod_k×페이지
  화면px² → lod_page로 교체. 프로브는 px=0이라 구조적으로 exact. 요청 필드 `ViewReq::lod_swap`이
  켜져 있을 때만 — **renderd의 일반 레이아웃 프레임은 기본 끔**(2026-09-22 사용자 결정: LOD는 쓰지
  않는다; 인덱스도 `floe2 index --lod` 없이는 변종을 만들지 않고, 뷰어의 LOD 토글은 제거됐다),
  `FLOE_RUST_LOD=on`으로 변종이 있는 캐시에서 되돌린다. CLI(`floe-index plan`, `--lod 0`이 끔)·jobdeck·
  테스트 요청은 종전대로 켠다.
- 워시: 페이지 화면상 양축 ≤ wash_px → (layer, bbox) 렉트로 붕괴. 요청 필드 `ViewReq::page_wash`가
  켜져 있을 때만 — **renderd의 일반 레이아웃 프레임은 기본 끔**(2026-09-22 사용자 결정: 2×2 px 덩어리는
  KLayout 규칙으로 그리는 표시용 점이라 작은 페이지의 area-true 그리기를 가렸다; 이제 그 페이지를
  디코드해 그린다), `FLOE_RUST_PAGE_WASH=on`으로 되돌린다. exact 프레임, jobdeck, CLI(`floe-index plan`)와
  테스트 요청은 종전대로 켠다.

### 예산에 맞춘 컷 (budget-fitted cut, 0.12.162, 2026-09-18)

실칩: `thin keep` + detail high가 Calibre와 가장 가까운 그림이고 밀도는 오히려 높다. 그런데
광역뷰·많은 레이어·깊은 depth에서 "decoded generation budget exceeded"로 프레임이 실패했다
(depth 0에서도). 페이지를 무작위로 버리는 대신 **밀도를 낮춰서 맞춘다.**

- `plan_hier`는 `ViewReq::decode_budget`(renderd가 주는 세대 예산, 기본 1024 MB)이 있고
  `cut_dbu > 0`이면, 선택한 페이지의 디코드 메모리 추정(`page_memory` = 4096 + 레코드당
  192 B + 레코드당 12 B를 넘는 저장 바이트 × 6(꼭짓점·Pts 오프셋); 합성 MAIN01 실측 사각형
  레코드당 173 B의 약 1.1배)을 합산한다. 예산을 넘는 패스는 그 자리에서 버린다.
- 후보 컷(`fit_rungs`)은 요청 컷 × 2^(k/4), k = 1..24(× 64)와 그보다 큰 표준 detail 컷
  (1·3·5 px)이다. **맞는 컷 중 가장 세밀한 것**을 이분 탐색으로 고른다(넘는 패스는 중단,
  맞는 패스는 완전한 플랜; 약 log2(26)회). detail 컷이 후보에 있으므로 high 요청이
  medium이 그대로 계획하는 컷보다 거칠게 끝나는 일이 없다(실칩 2차 보고: 반 옥타브
  사다리에서 high가 2.83 px → 4 px로 건너뛰어 3 px가 맞는 medium보다 거칠고 빨랐다).
- keep 요청이 어떤 단에서도 안 맞으면(긴 헤어라인은 size 컷으로 빠지지 않는다) hairline
  컷을 켜고(cull) 요청 컷부터 같은 후보를 다시 탐색한다. 끝까지 안 맞으면 마지막 단의
  완전한 플랜을 `fit_over`로 표시해 돌려주고 렌더는 종전대로 예산을 보고한다.
- 요청만으로 결정적이고, 예산 안에 드는 프레임은 한 패스로 끝나며 픽셀이 바뀌지 않는다.
  exact(cut 0), 덱 pass, probe(`decode_budget` 0)는 건드리지 않는다.
- stats `fit_bytes/fit_pct/fit_cull/fit_passes/fit_over`(fit_pct = 맞춘 컷 ÷ 요청 컷 × 100, 0 = 그대로), frame line `fit_pct= fit_cull=
  fit_over=`, 상태줄의 컷 옆 `cut<…um xN to fit budget, hairlines culled`(말줄임되는 뒤쪽 진단 문자열이 아니라 앞쪽). 킬 스위치
  `FLOE_RUST_FIT_BUDGET=off`(종전 오류로 복귀).
- 위 사다리의 한계: 줄이는 단위가 크기 등급이라, 한 등급의 페이지만으로 예산을 넘는 뷰는 그
  등급이 통째로 빠진다. 합성 MAIN01에서 thin keep의 fit 다음 다섯 줌 단계가 **빈 화면**이었다
  (사용자 2026-09-19; 1/10 크기 재현: 전 레이어 ×2·×4·×8이 컷 ×4·×8·×16에서 0페이지, plan 5 s).

#### 디코드 크기가 추정을 넘을 때 (0.12.301, 2026-10-05)

실칩(사용자 2026-10-05): 787·789 두 레이어만 켜고 밀도를 끄면 대부분의 프레임이
`decoded generation budget exceeded: 1093017130 > 1073741824 bytes`로 실패했다. 전체 레이어에서는 거의 나지 않았다.

- 원인: 계획기는 페이지를 **추정**(`page_memory`)으로 예산에 맞추고, 렌더는 디코드한 페이지의 **부과 크기**
  (`DecodedPage::estimated_bytes`)를 예산과 견줘 넘으면 프레임을 오류로 끝냈다. 부과가 추정보다 큰 페이지만 모이면
  맞춘 프레임이 넘는다. 전체 레이어에서는 추정이 넉넉한 다른 페이지가 덮어 줬다(합성 MAIN01 1/10은 칩 전체로
  추정의 0.807배). 밀도를 켜면 1패스가 예산에서 예약(128 MB)을 뺀 만큼만 계획하므로 그 여유가 가렸다.
- 부과가 추정을 넘던 두 경우(0.12.300에서 재현):
  - 레코드 벡터의 여유 용량. 파서는 레코드를 push로 쌓아 용량이 길이의 최대 2배가 되고, 부과는 용량으로 셌다.
    - 사각형 페이지의 부과는 벡터가 찬 정도에 따라 레코드당 124~217 B였다(꽉 차면 레코드 96 B + 색인 24 B = 120 B,
      추정은 192 B). 합성 레이아웃 실측: 레코드 6.3만 개 페이지 124 B, 3.7만 개 192 B, 3.3만 개 216 B.
    - 사각형만 있는 레이어 하나(3.3만 개 페이지)를 25 MB 예산으로 보면 `28891938 > 26214400`(추정의 1.123배).
    - 실칩과 같은 모양: 합성 MAIN01 1/10에서 46/2·48/0 두 레이어만 켜고 밀도를 끈 detail high의 fit 뷰가 24 MB
      예산에서 `25635320 > 25165824`(예산의 1.019배 — 실칩 보고는 1.018배), 16 MB에서 `16908224 > 16777216`.
      이 칩의 449개 레이어 가운데 9개가 레이어 전체로 추정보다 크게 부과되고(1.013~1.104배; 46/2와 48/0은
      215·249쪽에 1.014·1.016배), 367개는 그런 페이지가 하나 이상 있다(가장 큰 것 1.231배).
  - 레코드들이 공유하는 반복 목록. OASIS의 modal 반복 재사용은 레코드들이 `Rep::Pts`의 Arc 하나를 공유하게 하는데
    부과는 레코드마다 셌다. 30,000점 목록을 40개 레코드가 쓰는 페이지가 추정의 8.7배로 부과됐다(메모리는 한 번).
- 고친 것 (각각 킬 스위치):
  1. **디코드한 페이지의 레코드 벡터를 길이에 맞게 줄인다** (render-core `decode_payload`의 `shrink_records`,
     `FLOE_RUST_DECODE_SHRINK=off`가 킬 스위치: 0.12.300의 부과).
     - 사각형 페이지의 부과가 레코드당 124~217 B → 120~124 B(추정의 0.62~0.65배)다. 전체 페이지의 부과는 라우팅 칩
       3,896 → 3,159 MB, 합성 MAIN01 1/10 3,999 → 3,171 MB(추정은 5,056 / 4,955 MB)다. 추정보다 크게 부과되는
       페이지가 합성 칩에 하나도 남지 않는다(가장 큰 페이지 1.231 → 0.787배).
     - 그래서 추정으로 맞춘 계획이 그대로 든다. 위 25 MB 예: 4쪽 다, 상주 15.8 MB. 두 레이어 예: 24 MB 13쪽, 16 MB
       12쪽 다. 다시 계획하지 않는다. 합성 MAIN01 1/10의 레이어를 하나씩(449개)과 전체로, 네 배율, 24 MB,
       밀도 끔(1,800 프레임): 0.12.300은 detail high에서 4, medium에서 3 프레임이 오류였고(모두 전체 레이어 —
       계획이 자기 추정으로도 예산을 넘는 아래 5의 경우) 지금은 0이다.
     - 시간(밀도 끔, 새 워커의 첫 프레임 / 그 뒤): 합성 MAIN01 1/10 full depth detail high fit 1,024~1,094 /
       715~765 ms → 1,017~1,083 / 677~735 ms(상주 894 → 694 MB), 라우팅 칩 깊이 0 detail high fit 319~339 /
       89~99 ms → 315~323 / 71~85 ms. 줄이는 비용은 보이지 않는다.
     - **밀도 스택은 페이지를 읽은 그대로의 크기로 센다** (`DecodedPage::grown_bytes` / `grown_charge`, renderd
       `density_as_read`; `FLOE_RUST_DENSITY_AS_READ=off`는 줄어든 크기로 센다).
       - 이유: 2패스는 1패스가 남긴 예산을 쓴다(`density_frame_reserve`). 1패스의 부과가 줄면 2패스가 더 많이
         디코드한다. 줄어든 크기로 세면 합성 MAIN01 1/10 full depth(밀도 켬)가 같은 그림에 fit 665·757 →
         775·817 ms, 2배 축소 759·848 → 911·964 ms였다(warm 중앙값 7회, 번갈아 두 번씩). 라우팅 칩 깊이 0의 2배
         확대는 2패스가 디코드한 페이지 86 → 133쪽, 점유로 대신한 페이지 172 → 125쪽, 프레임 평균 32.22 → 31.57
         (모두 디코드한 4 GB 기준 30.57: ×1.05 → ×1.03)이다.
       - 읽은 그대로 세면 2패스의 예약과 마지막 확인이 종전과 같아 밀도 프레임은 같은 그림이다. 시간은 같거나
         빠르다: 위 MAIN01 두 뷰가 벡터를 줄이지 않은 것(712·776, 806·875 ms)보다 느리지 않고, 라우팅 칩의 2배
         확대는 730·742 → 581·617 ms다(페이지 캐시가 같은 예산에 더 많이 담아 다시 디코드하는 페이지가 준다).
       - 줄어든 만큼을 2패스에 줄지는 2패스 예약(`FLOE_RUST_DENSITY_RESERVE_LEFT`)과 함께 사용자가 정할 일이다.
     - 덱 패스도 읽은 그대로의 크기로 센다(`deck.rs`): 패스의 조각 나눔과 예산에서 멈추는 자리가 종전과 같다.
       프레임의 세대 예산 확인과 페이지 캐시는 줄어든 크기로 센다: 캐시는 같은 예산에 더 많은 페이지를 둔다.
  2. **공유 목록은 한 번만 부과한다** (render-core `DecodedPage::estimated_bytes`의 `SharedLists`,
     `FLOE_RUST_CHARGE_SHARED=off`). modal 재사용이라 공유 레코드는 이어서 나오므로 직전 목록만 기억한다. 위 페이지가
     추정의 8.7배 → 0.54배다. 메모리가 실제로 한 번만 잡히는 것을 한 번만 세는 것이라 그림과 속도는 그대로다.
  3. **그래도 넘으면 오류 대신 다시 계획한다** (renderd `run_render` → `run_render_attempt`,
     `FLOE_RUST_BUDGET_REFIT=off`). 1·2 뒤에는 합성 칩에서 일어나지 않는다. 남은 경우(추정을 넘는 다른 모양의
     페이지)와 킬 스위치를 쓴 경우의 안전망이다. 아래 수치는 `FLOE_RUST_DECODE_SHRINK=off`로 잰 것이다.
     - 라운드를 읽다 누적 부과가 세대 예산을 넘으면, 읽은 페이지의 부과 ÷ 추정을 그 레이어 집합의 배율로 기억한다
       (`WorkerState::budget_scale`, 키 = 보이는 레이어·depth·root·밀도 스택 켬/끔, 3 % 여유, 이전 배율보다 5 % 이상
       크게).
     - 밀도 스택을 켠 프레임은 배율을 따로 둔다. 그 1패스는 2패스 예약을 뺀 예산으로 계획해 그 여유가 부과 초과를
       담는다. 밀도를 끈 프레임에서 생긴 배율을 같이 쓰면 필요 없이 1패스 페이지만 줄어든다.
     - 프레임을 처음부터 다시 계획한다. 계획 예산은 `scaled_decode_budget` = (1패스 예산) ÷ 배율이다.
       한 프레임에 최대 4번(`BUDGET_REFITS`). 처음 읽은 페이지는 페이지 캐시에 남아 있어 다시 디코드하지 않는다
       (두 레이어 예: 12쪽 모두 캐시 적중).
     - 배율을 올릴 때 그 레이어 집합·depth·root의 맞춤 기억(`fit_memory`, `fit_whole`)을 모든 배율·컷에서 지운다
       (`forget_fits`). 종전 예산으로 정한 결정이라서다. 지우지 않으면 뷰포트는 그 결정이 아직 맞아 그대로 쓰고,
       더 넓은 여백은 그 결정으로 매번 `dropped`가 된다. 다시 계획하는 프레임과 다음 뷰포트 프레임은 여백 범위로
       새로 결정한다.
     - 그 레이어 집합의 이후 프레임은 그 배율로 시작한다. 배율은 세션 동안 커지기만 한다(그림이 흔들리지 않게).
       캐시나 덱을 새로 열면 지운다.
     - 다시 계획한 프레임은 페이지를 내준다. 25 MB 예: 예산의 1/1.157로 한 번, 4쪽 대신 3쪽. 두 레이어 예: 24 MB는
       1/1.088, 16 MB는 1/1.050으로 한 번, 13쪽 대신 12쪽, 12쪽 대신 10쪽. 그래서 1이 먼저다.
  4. **여백(bg) 프레임이 넘으면 다시 계획하지 않고 버린다**(`dropped gen=N reason=budget`). 화면의 뷰포트는 종전
     예산으로 계획한 그림이라, 줄인 예산으로 계획한 여백이 도착하면 그림이 바뀐다. 배율은 올리고 기억은 지우므로
     다음 뷰포트 프레임이 여백 범위로 새로 결정한다. 여백 프리페치는 기본이 꺼져 있다(`--margin on`일 때만;
     2026-09-27). 켠 경우 0.12.300은 이 여백 프레임도 오류였고 뷰어는 여백의 오류도 상태줄에 보였다.
     - 두 레이어 예의 2배 확대, 24 MB: 0.12.300은 뷰포트 9쪽이 그려지고 여백마다 `25650912 > 25165824` 오류였다.
       지금은 여백도 13쪽으로 그려진다. `FLOE_RUST_DECODE_SHRINK=off`에서는 그 여백을 한 번 버리고(1/1.075),
       다음 뷰포트가 8쪽으로 새로 결정하며 이후 여백은 11쪽으로 그려진다.
  5. **어떤 계획도 안 들면 예산이 담는 만큼 그린다.** 계획이 자기 추정으로도 예산을 넘거나(`fit_over`) 4번을 다
     쓰면, 읽은 순서(뷰 중심에서 가까운 순)로 예산에 드는 페이지만 그리고 나머지는 `deferred`로 보고한다(상태줄
     `N pages over budget (not drawn)`). 그 프레임은 재사용용으로 보관하지 않는다.
  - exact 프레임과 계획기가 예산에 맞추지 않는 프레임(`decode_budget` 0, 컷 0, `FLOE_RUST_FIT_BUDGET=off`)은 종전대로
    오류다. 컷 0은 내보내기와 테스트가 쓰는 정확 프레임이라 페이지가 빠진 채 나오면 안 된다(jobdeck 게이트
    `test_p1_2_deck_pass_is_charged_against_the_page_budget`가 이 계약을 본다). 뷰어의 detail은 1·3·5 px라 늘 컷이 있다.
- 프레임 줄 끝에 `fit_scale=`(배율의 천분율, 0 = 없음)과 `fit_refits=`(이 프레임을 다시 계획한 횟수)가 붙는다. 상태줄은
  예산 맞춤 옆에 `, pages x1.18 their estimate`를 붙인다.
- 그림: 전에 그려지던 프레임은 0.12.300과 바이트 단위로 같다(합성 5개 레이아웃의 9개 뷰 × 밀도 켬·끔, 예산 1024·256·64
  MB의 54 프레임). `FLOE_RUST_DENSITY_AS_READ=off`면 밀도 프레임 3·1·5개가 달라진다 — 이 비교가 2패스의 셈을 본다.
- 남은 것 (사용자가 정할 것):
  - 추정 자체(`FIT_RECORD_BYTES` 192)는 그대로다. 이제 부과가 추정의 0.62~0.8배라, 추정을 낮추면 같은 예산에 페이지가
    더 든다(예산에 맞춘 그림과 속도가 전반으로 바뀐다).
  - 재면서 본 것: 예산에 통째로 들지 않는 뷰포트는 여백 범위(뷰의 2배)로 정한 결정으로 계획돼 예산의 30 % 안팎만
    쓴다. 조밀한 상자 레이아웃(1 µm²에 하나)의 31쪽(읽은 그대로 108 MB) 뷰가 예산 128 / 112 / 100 MB에서 11 / 10 /
    9쪽(37 / 34 / 30 MB)이다(0.12.300도 같다). 여백 프리페치는 기본이 꺼져 있으므로(2026-09-27) 그때는 뷰포트
    범위로 정하면 페이지가 더 든다 — 그림이 전반으로 바뀌는 일이라 손대지 않았다.
- 단위: render-core `a_repetition_list_shared_by_a_pages_records_is_charged_once`,
  `a_decoded_pages_record_lists_are_cut_to_their_length`, `a_decoded_page_keeps_the_charge_it_was_read_at`; renderd
  `a_layer_sets_budget_scale_cuts_the_plans_budget`, `a_raised_budget_scale_forgets_the_layer_sets_fits`.
  게이트는 `fit_budget`의 `refit_checks`(SPEC-VALIDATION).

#### 예산에 맞춘 밀도 (0.12.166, 선택 규칙은 0.12.169 — 기본; 위 사다리는 `FLOE_RUST_FIT_THIN=off`)

컷을 올려 크기 등급을 통째로 버리는 대신 **밀도를 낮춘다**(`plan_hier_thinned`,
`thin_to_budget`). 예산을 넘는 플랜은 자기 페이지를 **하나의 고정 우선순위**로 늘어놓고, 예산에
들어가는 **가장 긴 접두사**만 남긴다(`fit_priority`).
- 1순위 크기 등급, 큰 것 먼저. 등급 = 그 페이지가 아직 선택되는 가장 큰 컷(`fit_key`: 도형 단위 컷
  (keep, 0.12.173)은 **`max_min`**(0.12.174 — 리뷰: 긴 변으로 매기면 10000×4 배선 페이지가 64×64
  페이지를 밀어냈다), 킬 스위치의 keep은 긴 변의 최대, cull은 min(긴 변, 짧은 변 ÷ hairline))의 **옥타브**(절대 dbu — 등급이 뷰에 따라
  움직이지 않는다).
- 2순위 등급 안에서는 (seq + 셀 + 레이어)의 비트 반전값 오름차순(van der Corput 순서). seq는
  (셀, 레이어) run 안의 순번. 등급의 어떤 접두사도 고른 표본이고 더 긴 접두사는 그 확대집합이며,
  페이지가 하나뿐인 run(작은 셀, 내용이 적은 레이어)도 위상이 셀·레이어로 어긋나 같이 솎인다.
- 3순위 페이지 번호. 접두사는 **엄격**하다: 들어가지 않는 첫 페이지에서 끝난다(뒤의 작은
  페이지로 빈자리를 메우지 않는다).
- 결과: 접두사가 끝나는 등급 위는 완전, 그 등급은 표본, 그 아래 등급은 없다 — 세밀함은 가장
  작은 등급부터 빠지고, 들어가지 않는 등급은 버리지 않고 솎는다(빈 화면의 수정).
- **보장**: 순서가 뷰·예산과 무관하고 접두사가 엄격하므로, 같은 컷에서 **뷰를 좁히거나 예산을
  늘려도 화면 안에 남아 있는 페이지는 빠지지 않는다**. 확대하면 컷이 내려가 더 작은 등급이
  들어오는데, 모두 이미 그려진 페이지보다 뒤 순위다. (리뷰 2026-09-19, 0.12.166: 2^k 표본에
  큰 페이지를 덧채우던 방식은 넓은 뷰 {0, 1, 8} → 좁힌 뷰 {0, 4, 8}로 페이지 1이 화면 안에서
  사라졌다.) 예외: 셀 hairline 컷처럼 컷에 따라 후보 자체가 달라지는 드문 경우, 그리고 페이지
  하나가 예산보다 큰 경우(접두사가 거기서 끝난다).
- 패스는 순위를 매길 후보를 찾는 일만 한다: 요청 컷, 그 위의 2의 거듭제곱 컷(등급 전체) 순.
  추정 메모리가 예산의 `FIT_OVERSHOOT`(8)배를 넘는 패스는 버리고, 끝난 패스에 자리가 남으면
  접두사가 그 아래 등급에서 끝나므로 한 단 아래 컷을 상한 없이 끝까지 계획한다. keep → cull
  폴백은 없다. 첫 페이지도 안 들어가면 마지막 컷의 완전한 플랜을 `fit_over`로 돌려준다(종전 오류).
- stats/frame line `fit_pct`(계획한 컷 ÷ 요청 컷 × 100; 맞췄으면 100 이상, 그대로면 0),
  `fit_thin`(접두사가 끝난 등급에서 약 2^k개 중 하나; 0 = 표본 등급 없음), `fit_full_pct`(완전
  등급의 시작 ÷ 요청 컷 × 100; 0 = 없음), `fit_none_pct`(이 아래는 없음; 0 = 통째로 빠진 등급
  없음). 상태줄 `cut<…um [xN,] 1/M below xF, none below xG to fit budget`.
- legacy 합성 MAIN01 1/10, 전 레이어, keep, detail high(0.12.166 측정): ×2·×4·×8이 빈 화면 →
  예산 16 GB 프레임과 129 / 37,206 / 26,782 px만 다른 그림.
- **배율별 맞춤 고정(0.12.231, renderd 0.12.221; 현장 2026-09-27).** 위 보장은 뷰를 좁힐 때의 것이고,
  뷰를 **넓히면**(뷰어가 뷰포트 뒤에 그려 바꿔 끼우는 여백 프레임: 각 축 2배) 접두사가 더 일찍 끝나
  페이지가 빠진다 — 여백이 도착하는 순간 화면이 바뀌고, 팬 뒤 새 뷰포트는 또 다르게 솎였다(합성
  1/10 칩 ×2 전 레이어: 뷰포트 `none below x2.99` 대 여백 `x5.97`, 겹침 268,757 px 차이). 이제 renderd가
  결정을 **(레이어·depth·컷·thin·정확한 배율)마다 기억**한다(`WorkerState::fit_memory`, `FixedFit` =
  계획한 컷과 마지막으로 남긴 페이지의 `fit_priority`): 그 배율의 다음 프레임은 `plan_hier_fixed`로 같은
  문턱을 그대로 적용해(우선순위가 문턱 이하인 페이지만) 어느 프레임에서든 같은 페이지가 같은 운명을
  갖는다. 문턱 아래 페이지가 예산을 넘는 프레임만 다시 결정하고(`fit_redecided`, 상태줄 ` (refit)`) 기억을
  바꾼다. stats `fit_fixed`/`fit_redecided`, `HierStats::fit_decision`. 한 배율의 **첫 프레임은 여백
  프레임의 범위(각 축 2배)로 결정을 내린다**(계획 한 번 더, 디코드 없음): 여백이 도착해도 뷰포트가 이미
  같은 솎음이라 줌 뒤에도 그림이 바뀌지 않는다. 팬으로는 내용이 바뀌지 않고 줌에서만 다시 결정한다.
  게이트 `fit_budget`이 가운데 → 전체(여백) → 가운데 순서로 확인한다.
- **기억이 실제로 공유되도록(0.12.232, renderd 0.12.222; 현장 2026-09-27 "여전히 두 번 칠해짐",
  `--goto 7805.484,15578.763,4719.5` 뒤 50% 패닝 두 번).** 0.12.231은 세 가지로 새었다. (1) 키가 배율의
  **정확한 비트**였는데 뷰포트·여백·팬 프레임은 각자의 상자에서 px_per_dbu를 유도하므로 마지막 비트가
  달라 저마다 기억을 갖고 저마다 결정했다(합성 1/10 칩 1920×1777 스윕: ×8 (13654, 12774) um에서 뷰포트
  `1/461/230`, 여백 `1/3686/1843`, 둘 다 `fit_fixed`=1, 겹침 139,565 px 차이; ×4 126,475 px) — 이제 키는
  유효숫자 9자리(`scale_token`; 줌 단계는 1% 이상 떨어져 있다). (2) 예산 안에 다 들어간 계획은 결정을
  남기지 않아 뒤따르는 여백이 혼자 결정했다(×16: 뷰포트 `fit_thin` 0, 여백 `2/1843/921`, 47,046 px) — 이제
  `FixedFit::everything`(모든 페이지)을 남기고, 여백은 그 아래에서 그대로 들어가거나 넘치면 다시 결정한다
  (`fit_redecided`; 기억은 좁아지는 쪽으로만 바뀐다). (3) 여백의 한 변 확장은 반 화면을 16 px에 맞춘 값이라
  탐색 범위(정확히 반 화면)보다 최대 8 px 컸다 — 탐색은 16 px 더 넓게 본다. 단위
  `a_budget_fit_decided_once_is_applied_to_every_frame_at_the_scale`,
  `the_fit_memory_keys_one_scale_whatever_bits_a_frame_derives`; 게이트는 여백이 결정을 **적용**했는지
  (다시 결정하지 않음), 첫 가운데도 여백의 가운데와 같은지, 픽셀 일부만큼 옮긴 가운데가 기억을 공유하는지,
  예산 안의 프레임도 결정(everything)을 지니는지 본다.
  (4) 셋을 고친 뒤에도 **밀집한 쪽으로 팬**하면 여백(뷰의 4배 넓이)이 결정 아래에서 예산을 넘겨 다시 결정하고
  그대로 화면에 들어왔다(위 시나리오의 아래 방향: 여백 `1777/889` 대 뷰포트 `889/444`, 뷰포트 겹침 1,772 px,
  여백 전체 38,506 px). 여백은 "뷰포트가 이미 보이는 대로" 보여야 하는 프레임이므로 이제 뷰어가 `bg=on`으로
  표시하고(`RenderCommand::background`), renderd는 여백이 결정 아래에서 다시 결정해야 하면 그리지 않고
  `dropped gen=N reason=fit`으로 답한다 — 기억도 뷰포트도 그대로, 그 자리의 팬은 뷰포트를 렌더한다(여백 없이;
  뷰포트 자체가 예산을 넘겨야만 다시 결정하고 상태줄이 ` (refit)`으로 말한다). 어댑터는 `dropped`를 결과로
  올리고(뷰어는 `_margin_debug`만; `_margin_pending`은 유지 = 같은 자리에서 여백을 다시 청하지 않는 재청 방지),
  게이트는 구석 1/4에 서로 다른 셀 4개, 먼 절반에 200개인 전용 레이아웃(예산 1 MB; 합성 칩은 셀이 반복되어
  넓은 뷰도 같은 페이지를 써서 이 사례를 못 만든다)에서 구석 → 전체 여백(bg, 떨어짐) → 구석(같은 그림) →
  전체 뷰포트(다시 결정) 순서로 본다. 기억은 그 배율의 첫 프레임이 정하고 뷰포트가 넘칠 때만 좁아진다.
- **새 배율의 결정은 결정만 내는 계획으로(0.12.302, renderd 0.12.279; 현장 2026-10-05).** 한 배율의 첫 프레임은
  여백 범위(뷰의 2배)를 먼저 계획해 결정을 낸다(위). 이 사전 계획은 뷰포트 계획과 같은 걷기라서 범위 안의 배치를
  모두 읽었다 — 대부분 뷰 밖이고, 결정 말고는 버리는 것이다. 그 시간은 `plan`에 잡히지 않아 상태줄의 `other`로 갔다
  (실칩: 새 배율의 첫 프레임에서 1패스가 느리고 원인이 other; 여백 프리페치를 꺼도 사전 계획은 돈다).
  - 재현(합성 MAIN01 1/10 — design.ovm 7.2 GB, 밀도 끔, medium, full depth; 인덱스가 파일 캐시에 없는 조건 = 소스와
    인덱스의 APFS 복제본): 사전 계획이 fit 2.7~3.0 s, 4배 확대 0.77 s, 16배 확대 0.34 s였다. 파일 캐시에 올라온
    뒤에는 0.02~0.05 s다. 그 시간의 대부분은 범위가 통째로 덮는 셀의 배치를 읽는 데 갔다(4배 확대 0.63 s 중 0.58 s,
    16배 확대 0.33 s 중 0.25 s).
  - 뷰만으로 결정하는 규칙은 그림을 바꾼다(같은 칩, 25곳 × 4배율의 첫 프레임 가운데 달라지는 것): 뷰 범위로 예산
    전체에 맞추면 페이지가 지금의 1.3~8.4배(더 자세하고 느림), 뷰 범위로 예산의 넓이 비율(약 1/4)에 맞추면
    0.01~0.32배다 — 결정은 뷰 밖의 내용에 달려 있다. 그래서 결정은 그대로 두고 읽는 양을 줄였다.
  - 이제 사전 계획은 **결정만 내는 계획**이다(render-core `Cache::fit_decision_cancellable`, floe_vfs
    `HierOpts::decide_by`, `Hier::decide_children`). 계층 요약(design.ovh, 없으면 데몬이 메모리에 만든 것)으로 정할 수
    있는 것은 배치를 읽지 않고 정한다. 걷기가 배치를 펼치는 기준은 자식 셀만으로 정해지고(컷·헤어라인 대비 상자,
    레이어, 남은 depth), 상자 안에 통째로 든 배치가 하나라도 있는 자식은 "통째로 덮임"이다(자기 페이지 전부와, 차례로
    그 자식들).
    - 범위가 통째로 덮는 셀: 요약의 자식을 모두 통째로 덮임으로 넘긴다. 배치를 읽지 않는다.
    - 일부만 덮는 셀: 배치들의 범위(`Edge::extent`)가 상자 안에 다 드는 자식은 통째로 덮임, 상자와 만나지 않는
      자식은 없음. 나머지만 걷기로 찾고, 상자에 통째로 든 배치 레코드를 만난 자식은 더 보지 않는다. 다 찾으면
      걷기를 끝낸다.
    - depth의 끝(r = 0)은 자식이 외곽선뿐이라 읽지 않는다. sub-cut 박스도 만들지 않는다.
    - 계획의 페이지 집합이 걷기와 같으므로 결정이 같다. 인스턴스·프레임·박스·통계는 걷기와 다르다(결정만 쓴다).
  - 킬 스위치 `FLOE_RUST_FIT_PROBE_SUMMARY=off`: 걷기(0.12.301). 요약이 없으면 걷는다 — 배치 레코드가 400만
    (`HIER_INLINE_PLACES`)을 넘는데 design.ovh가 없는 캐시이고, `floe-index hier <cache>`(뷰어의 "build cell index")로
    만든다. `FLOE_RUST_FIT_PROBE_CHECK=on`(진단)은 둘 다 계획해 `[render-core] fit probe check: the same|DIFFERENT …`를
    찍고 걷기의 결정을 쓴다(실칩 확인용).
  - 사전 계획 시간은 프레임 줄의 `fit_probe_us=`(0 = 기억한 배율이거나 맞춤 없음)와 `fit_probe_walk=`(1 = 걸었다)로
    나온다. 어댑터는 단계 합에 넣어 `other`에서 빼고, 상태줄은 100 ms부터 `+ N fit probe`, 로그 줄은 있으면 늘
    `, fit probe N ms`(걸었으면 ` (walk)`)를 보인다.
  - 측정(위 재현 조건, 번갈아 두 번씩; 새 배율 첫 프레임의 전체 / 그중 사전 계획):

    | 새 배율의 첫 프레임 | 걷기 (0.12.301) | 요약 (0.12.302) |
    |---|---|---|
    | 16배 확대 | 566·551 ms / 336·337 ms | 279·287 ms / 63·63 ms |
    | 32배 확대 (그다음) | 105·94 ms / 34·34 ms | 77·77 ms / 17·16 ms |
    | 4배 확대 | 980·992 ms / 765·765 ms | 894·851 ms / 35·36 ms |
    | fit (세션 첫 프레임) | 3,875·3,556 ms / 3,000·2,741 ms | 3,806·3,601 ms / 46·39 ms |

    - 4배 확대는 뷰 자체가 범위 내용의 대부분이라 그 인덱스 읽기가 `plan`으로 옮겨간다(plan 14 → 621·613 ms).
      fit은 뷰가 칩 전체라 그대로다(plan 18·21 → 2,916·2,764 ms).
    - 인덱스가 파일 캐시에 있을 때 사전 계획의 합(같은 칩의 첫 프레임들, 번갈아 두 번씩): medium 100프레임
      2,310·3,204 → 2,001·1,986 ms, high 50프레임 3,137·4,122 → 2,189·1,901 ms, depth 2 + 프레임 75프레임
      1,059·1,097 → 851·854 ms. 최상위에 64종 셀의 배치 144만 개가 놓인 레이아웃은 75프레임 801·787 → 5·4 ms다
      (자식마다 통째로 든 배치를 하나 찾으면 끝난다).
  - 같은 결정: 요약과 걷기를 견준 첫 프레임 975개(합성 MAIN01 1/10 — medium·high·low, depth 0·1·2·full, 프레임
    켬·끔, 밀도 켬·끔, 예산 1024·256·24 MB, 두 레이어만; 라우팅 칩 1024·64 MB; 표준 셀; 배치 144만 개 레이아웃 16 MB)가
    페이지 수·맞춤·픽셀까지 같다. 진단 스위치로 275프레임 `the same`, `DIFFERENT` 0. 0.12.301 대비 54프레임(9개 뷰 ×
    밀도 켬·끔, 예산 1024·256·64 MB) 바이트 동일.
  - 알아 둘 것:
    - 사전 계획이 미리 읽어 두던 이웃의 인덱스는 이제 그쪽으로 팬할 때 읽는다(16배 확대에서 반 화면 팬의 plan
      6·3 → 31·32 ms, 한 화면 팬 46·43 → 65·64 ms).
    - 범위 경계에 걸친 셀의 배치는 여전히 읽는다(16배 확대의 남은 63 ms). 결정이 뷰 밖 내용에 달려 있는 한 없앨 수
      없는 부분이다.
    - design.ovh가 없고 배치 레코드가 400만 이하인 캐시는 첫 사전 계획 때 요약을 한 번 메모리에 만든다(표준 셀
      레이아웃 0.03~0.06 s; 셀 트리가 쓰는 것과 같은 요약).
  - 단위 vfs `a_plan_for_its_decision_alone_takes_a_covered_cells_children_from_the_summary`(계층 픽스처의 5,400 조합 —
    뷰 6 × depth 5 × 컷 4 × 예산 5 × 레이어 3 × 프레임·root 3 — 에서 페이지와 맞춤이 걷기와 같음; 통째로 덮인 칩은 배치
    노드 0개; 줄지어 놓인 배치 64개는 9개 대신 6개 노드에서 끝남; 다른 인덱스의 요약은 쓰지 않음; 크기 접기를 빼거나
    "덮음"을 "만남"으로 바꾸면 실패 — 확인). 게이트 `fit_budget`의 `probe_checks`(SPEC-VALIDATION).
- **밀도 스택의 2패스도 같은 규칙(0.12.233, renderd 0.12.223).** 2패스의 계획은 세대 예산에서 예약한 자기 예산
  (`density_reserve`, 기본 128 MB)으로 같은 예산 맞춤을 거치고 결정은 배율·면마다 기억(`density_fit_memory`),
  여백의 2패스가 다시 결정해야 하면 여백을 떨어뜨린다(CUT_DENSITY_DESIGN §10.10: fit 뷰에서 여백이 도착하면
  8,110 px가 바뀌던 원인 = 2패스가 "세대 예산의 남은 절반"으로 디코드해 프레임·캐시 상태에 따라 달랐다).
  뷰어에 `--margin on|off`(여백 준비만; 팬 재사용 유지)를 두었고, 0.12.234부터 **기본 off**다(사용자 결정
  2026-09-27: 여백이 도착해 덧그려지는 일 자체를 없앤다; 팬·줌마다 뷰포트 프레임을 렌더한다. on은 독립 인스턴스).
- **예산이 통째로 담는 프레임은 결정과 무관(0.12.265, renderd 0.12.244; 사용자 2026-10-01).** 결정은 배율마다 기억하고
  위치는 키에 없는데, 뷰어의 줌 단계는 칩 어디서나 같은 배율로 되풀이된다. 그래서 조밀한 곳에서 내린 문턱이 같은 단계의
  성긴 곳에서도 페이지를 버렸다.
  - 사용자 보고: 754.6 µm 뷰는 점이 있고, 603.7 µm 뷰는 `lit 0 px`(디코드 페이지 없음), 386.4 µm 뷰는 다시 점이 있었다.
  - 합성 1/10 칩 재현(1.483 µm/px, 509×809 px, medium + 점): (15567, 17094) µm 뷰를 처음 그리면 2패스 페이지 28개(18개
    디코드), lit 85k px. (15000, 16000) µm를 먼저 보고 돌아오면 같은 뷰가 페이지 0개, lit 69k px(셀 점만)였다.
  - 이제 `plan_hier_fixed`는 기억된 결정을 적용하기 전에, 요청 컷의 완전한 계획이 예산에 드는지 본다. 결정이 컷을
    올렸으면 요청 컷으로 한 번 계획해 본다(초과 시 `FIT_OVERSHOOT`배에서 중단). 들면 모든 페이지를 남기고
    `fit_decision` = `FixedFit::everything`(요청 컷), `HierStats::fit_whole`을 표시한다. 기억은 그대로 두어, 예산을
    넘는 프레임만 그 문턱을 쓰거나 다시 정한다.
  - renderd는 배율·면마다 마지막 뷰포트가 통째였는지 기억한다(`fit_whole`, `density_whole`). 그 자리에서 솎아야 하는
    여백은 `dropped reason=fit`이다(뷰포트보다 적게 보이므로).
  - 팬 재사용: 통째로 그린 프레임과 결정 아래에서 솎은 프레임을 섞지 않는다. 후보는 결정 또는 everything으로 찾고,
    계획 뒤 프레임의 결정과 다르면 버린다.
  - 계획을 건너뛰는 라벨 전용 재사용은 통째로 그린 프레임만 쓴다(솎은 프레임은 그 뷰가 통째인지 계획해야 안다).
  - 2패스의 하한 탐침(`FLOE_RUST_DENSITY_FLOOR_PX` < 밀도 컷)도 뷰포트는 프레임마다 가장 낮은 단에서 다시 잰다. 여백만
    뷰포트의 단을 따른다.
  - 단위: `a_budget_fit_decided_once_is_applied_to_every_frame_at_the_scale`. 여섯 페이지 결정 아래에서 예산 18이면
    18쪽 전부(everything)이고, 예산 10이면 그 여섯이다. 컷을 올린 결정 아래에서도 예산이 담으면 요청 컷이다.
  - 게이트: `fit_budget`(다시 정한 뒤의 구석 = 처음 구석), `density_stack`(`uneven` 레이아웃, 2패스 예약 1 MB: 전체를
    먼저 그린 뒤의 구석 = 새 워커의 구석).
- **다른 곳에서 만든 계획의 맞춤 `fit_planned`(0.12.267, renderd 0.12.246; 사용자 2026-10-01).** 밀도 스택 2패스를 영역별로
  스레드에서 계획해 합친 계획에, `plan_hier`가 같은 뷰에 하는 맞춤을 그대로 적용한다.
  - 순서: 예산이 통째로 담으면 everything, 기억된 결정(`fixed_fit`) 아래에서 들면 그 문턱, 아니면 `thin_to_budget`으로 다시
    결정(`fit_redecided`).
  - None(호출자가 한 계획으로 맞춤): 결정이 컷을 올렸을 때, 페이지가 `FIT_OVERSHOOT`배 예산을 넘을 때(맞춤은 더 거친 컷을
    다시 걷는다), 한 페이지도 안 들 때, 컷 사다리(`fit_thin` 끔).
  - `Vfs::fit_planned_in`, `Cache::fit_plan`. 0.12.266까지 스레드는 예약이 통째로 담는 뷰에서만 썼고, 솎아야 하는 느린 뷰는
    한 스레드였다.
  - 단위: `a_plan_made_elsewhere_fits_as_plan_hier_would`. 완전한 계획 + `fit_planned`가 `plan_hier`와 같다(예산 18·6·3 /
    여섯 쪽 결정 아래 10·18·3). 컷을 올린 결정과 초과(예산 2에 18쪽)는 None이다.
- **이미 들고 있는 페이지는 예산에서 0(0.12.266, renderd 0.12.245; 사용자 2026-10-01).** `HierOpts::free_pages`
  (`PlanRequest::free_pages`, 정렬된 페이지 번호)는 프레임이 이미 디코드해 든 페이지다. renderd는 밀도 스택 2패스의 계획에
  1패스의 페이지를 넘긴다.
  - 그 페이지는 예산이 묻는 곳에서 0으로 센다: 걷기의 `fit_bytes`(탐침의 한도), `unique_page_memory`, `thin_to_budget`,
    `plan_hier_fixed`. 맞춤의 문턱은 비용이 드는 페이지에 대해서만 정하고, 든 페이지는 문턱과 무관하게 남긴다.
  - 사례: 합성 칩 (15999.7, 15997.6) µm, 979.7 µm 뷰, 990×1000 px.
    - detail high + 하한 0 px: 탐침이 고른 37페이지가 모두 1패스의 것(새 페이지 0)인데 추정 201 MB로 128 MB 예약을
      넘어 실패했다. 그래서 `lit 0`이었고, 뷰의 3/4 오른쪽(그 페이지들이 빠진 곳)에서는 226k px였다. 이제 첫 뷰가
      113k px다(새 페이지 108개).
    - medium 기본: 2패스가 27페이지로 2.7k px였는데, 이제 86페이지 136k px다(예약을 크게 준 것과 같다).
  - 영역을 띠로 나눠 계획한 2패스도 합친 계획을 같은 맞춤에 넘기므로(아래 `fit_planned`) 든 페이지는 0으로 센다.
  - 단위: `pages_the_frame_holds_cost_the_budget_nothing`.
    - 12쪽을 들면 남은 6쪽이 예산 6에 통째로 든다(들지 않으면 6쪽으로 솎임).
    - 탐침 한도도 새 6쪽만 센다.
    - 솎는 맞춤도 든 두 쪽을 문턱 밖에서 남기고, 같은 결정을 다시 적용해도 같다.
- **컷 아래 점(0.12.247, renderd 0.12.232; 진단 `FLOE_RUST_DENSITY_DOTS=on`, CUT_DENSITY_DESIGN §10.12).**
  `HierOpts::sub_cut_dots = Some(몫)`(`Vfs::plan_hier_in`의 넷째 인자, `PlanRequest::sub_cut_dots`)이면 요청의
  컷은 **셀·자식 BVH 노드의 컷**이고 페이지·레코드는 컷 × 몫(`Hier::page_cut`; 래스터의 레코드 컷
  `HierStats::shape_cut`도 그것)이다. 컷 아래 배치·노드는 걷지 않고 점 항목으로 센다: 셀 좌표의 4×4 px 블록·레이어마다
  개수(항목당 max(1, floor(면적/2 px²)), 블록당 최대 8), 항목의 가장 위 레이어만, 축 정렬 배열은 블록마다 중심 멤버
  수를 인덱스 산술로, 블록 크기 이하 노드에서 멈춤, 시작 상자를 한 블록 넓혀 블록이 중심 항목을 모두 받고 영역
  상자마다 만나는 노드·페이지·배치를 한 번만 센다. 블록마다 wash 하나(면적 = (개수 + ½) × 2 px²)를 내며 상한 초과는
  다시 거칠게 계획하지 않고 버린다(`sub_cut_box_over`). 레이어 수 상한(`sub_cut_box_layers`)은 적용하지 않는다.
  단위 `the_sub_cut_dots_count_what_the_cut_drops_by_block_and_never_walk_into_it`.
  **1점보다 작은 멤버(0.12.286, renderd 0.12.264; 사용자 2026-10-04, 루트 칩 fit depth 1 "밀도점이 전체를 덮어 버림"):**
  `HierOpts::dot_area_share`(기본 켬, `FLOE_RUST_DENSITY_AREA_SHARE=off`가 킬 스위치).
  - 멤버 하나의 점 `member_share(면적)`: 1 px² 이상은 max(1, floor(면적/2))이고, 1 px² 미만은 면적 그대로의 분수다
    (`page_dots`와 같다).
  - 분수는 `whole_dots(점, 블록, item_salt(항목 상자, 레이어))`로 정수로 만든다. 블록과 항목으로 정한 디더라 프레임·
    계획·스레드와 무관하다. 0이면 점을 내지 않는다.
  - 적용하는 곳:
    - `add_dots`의 블록 이하 항목(`holds` 0이면 없음).
    - `array_dots`의 블록별 수, `dot_chunk`.
    - 리스트의 빠른 길(멤버마다 같은 규칙, 런은 정수 점을 모은다).
    - `box_child`·`box_node`·`node_holds`의 담는 수(멤버 수 × `member_dots`를 f64로 더한 뒤 정수로).
  - 단위 `a_member_under_a_pixel_stands_for_its_area_in_dots`.
  **detail 문턱(0.12.288, renderd 0.12.266; 사용자 2026-10-04, 루트 칩 depth 0 "축소하면 없던 공간에 점이 나타남", "하나의
  픽셀에 도형 크기의 합이 일정 수준을 넘어야 켜는 방식"):** `HierOpts::dot_gate`(기본 켬, `FLOE_RUST_DENSITY_GATE=off`가
  킬 스위치), `HierOpts::dot_gate_share`(None이면 컷으로; 진단 `FLOE_RUST_DENSITY_GATE_SHARE`).
  - 블록에 필요한 점 `dot_gate_min` = ceil(몫 × 블록 px² − 0.01), 최소 1, 최대 블록 상한. 몫은
    `dot_gate_share_of_cut(cut_px)` = (컷 − 1) / 16을 [0, ½]로 자른 값이다. high 1 px → 0, medium 3 px → 1/8(4 px 블록에
    2점), low 5 px → 1/4(4점).
  - `flush_dots`가 그보다 적은 블록을 빼고(`HierStats::dot_gated`), `settle_occ_fallback`도 미룬 항목에 같은 문턱
    (`HierStats::dot_gate_min`)을 쓴다. 블록은 한 계획 안에서 통째로 세므로(`dot_boxes`) 스레드 수와 무관하다.
  - 디코드한 페이지의 도형은 거르지 않는다(도형이 있는 자리에 그린다).
  - 병합(`Cache::merge_plans`)은 `dot_gated`를 더하고 `dot_gate_min`은 최댓값을 둔다.
  - 단위 `a_dot_block_too_sparse_for_the_detail_is_left_out`. 점 수 규칙을 보는 단위는 `dot_gate: false`로 고정한다.
  **밝기(0.12.297, renderd 0.12.274; 사용자 2026-10-05 "한 점에 누적되는 도형 크기 … 밝기 조절로 표시", "밝기는 원본
  색상보다 밝아질 수 없게", "g = 1·2·4로 진행"):** `HierOpts::dot_bright = Some(g)`(`Vfs::plan_hier_in`·`fit_planned_in`의
  마지막 인자, `PlanRequest::dot_bright`; renderd `density_bright_gain`이 점 모드에서 detail로 정한다. 기본 켬,
  `FLOE_RUST_DENSITY_BRIGHT=off`가 킬 스위치). 블록의 수는 점이 아니라 **덮은 면적**이다.
  - 단위는 1/16 px²(`DOT_BRIGHT_UNITS`)다. 멤버 하나는 크기와 무관하게 면적 × 16이다(`member_share_of`; 1 px² 이상도
    floor(면적/2)가 아니다). `page_dots`, 점유 격자(`occ_items`)의 칸별 덮임, `node_dots`의 페이지 덮임도 같은 단위다.
    상자로 세는 항목의 면적 단위(`dot_area`)는 1/16 px²다.
  - 블록 상한은 블록 면적 / g다(ceil(블록² × 16 / g)). 래스터의 min(1, g × 덮임)이 원본 색이 되는 곳이라, 그 이상은
    그림이 같다.
  - 블록은 64 px 이하다(`DOT_BRIGHT_BLOCK_PX_MAX`, `dot_block_px_of`). 수는 u16에 실리고, 64 px 블록 전체의 덮임이 g = 1에서
    64 × 64 × 16으로 그 끝이다. 진단 `FLOE_RUST_DENSITY_BLOCK_PX`가 더 커도 그렇다. renderd의 병합(`Cache::merge_plans`)과
    보고(`density_block`)도 같은 값을 쓴다.
  - 문턱이 없다(`gate_min` = 1). 리스트 표본(`by_dot_step`)은 면적(px²)으로 본다(점 모드와 같은 걸음).
  - 늘 퍼뜨림 경로로 수를 싣는다(`spread`). 항목 상자는 대표하는 범위(합집합 ∩ 블록) 그대로다. 수를 반 픽셀에 담으려고
    키우지 않는다.
  - 단위 `under_the_brightness_a_block_counts_the_area_its_content_covers`: 0.04 px² LEAF 6,000개의 수가 면적 × 16의 10 %
    안(문턱 켬, 빠진 블록 0), 외톨이 LEAF의 상자는 자기 상자(점 모드는 키운 상자), 9겹 격자의 블록은 g = 1·2·4에서
    1,024·512·256으로 멈춘다. 200 px를 빈틈없이 덮은 1 px LEAF를 128 px 블록으로 계획하면 항목이 64 px 이하이고, 모두
    상자 넓이를 센다.
  **점유 격자 먼저·빠진 페이지의 대체·항목의 소수부(0.12.299, renderd 0.12.276; 리뷰어 2026-10-05, CUT_DENSITY_DESIGN
  §10.12 "리뷰 보완 1").** 셋 다 `dot_bright`가 있을 때만 걸린다.
  - `HierOpts::dot_occ_first = Some(share)`(`PlanRequest::dot_occ_first`; renderd `density_ovb_first`,
    `FLOE_RUST_DENSITY_OVB_FIRST=off`가 킬 스위치). 호출자는 `sub_cut_dots`를 1로 준다(페이지 컷 = 셀 컷).
    - `decode_under_floor`: 페이지 컷 아래 페이지는 격자 칸이 `dot_occ_cell_px`보다 크게 보이거나, **격자가 없고**
      (`Ovm::page_occ_grid` ≠ Some(true)) 가장 큰 도형이 floor(cut_dbu × share) 이상이면 디코드한다. 나머지는
      `box_page`가 격자(또는 면적)로 퍼뜨린다.
    - 그래서 달라지는 것은 격자가 있고 칸이 충분히 작으며 가장 큰 도형이 [share × 컷, 컷)인 페이지뿐이다.
  - `HierOpts::dot_stand_in`(기본 켬, `FLOE_RUST_DENSITY_STAND_IN=off`). 걸리는 조건은 `stand_in_on`: 밝기, 점 계획,
    one walk 아님, 페이지 퍼뜨림과 점유가 켜져 있고 design.ovb가 있음.
    - `expand` 끝에서 작업 셀의 페이지 중 공짜가 아니고(`page_is_free`) 점유 레코드가 있는 것을
      `HierStats::occ_aside`에 적는다: `OccAside { key, page, under, boxes }`. `under`는 페이지 컷 아래인데 디코드한
      페이지다.
    - `stand_in_left_out(v, req, opts, plan, left)`: `left`가 None이면 계획에 더는 없는 페이지(맞춤이 버림), Some이면
      그 페이지들을 처리한다. (셀, 페이지)마다 상자를 모아 `occ_grid_items`(격자) 또는 `occ_total_items`(면적)로 블록
      항목을 만들고, `dot_occ_boxes`면 상자가 통째로 든 블록만 남겨 그 작업 셀의 wash와 수로 넣는다.
    - 부르는 곳은 `fit_at_cut`(맞춤 직후)과 `Vfs::stand_in_pages`(`Cache::stand_in_pages`, renderd의 디코드 뒤)다.
    - 통계: `dot_stood_in`(대체한 페이지), `dot_occ_pages`·`dot_by[7]`에 더하고, `dot_occ_decoded`는 남은 `under`
      페이지 수로 다시 센다. 병합(`Cache::merge_plans`)은 `occ_aside`를 잇는다.
    - 이 경로에서는 `decode_under_floor`가 즉시 대체(`occ_fallback`)를 만들지 않는다.
  - `HierOpts::dot_bright_sums`(기본 켬, `FLOE_RUST_DENSITY_BRIGHT_SUMS=off`).
    - `add_dots`, 블록 이하 항목: 담는 수가 있으면 min(담는 수, ceil(상자 단위)), 없으면 `whole_dots(상자 단위)`.
      끄면 1단위 이상에서 버림, 그리고 담는 수와의 최솟값.
    - `add_dots`, 넓은 항목: 블록 몫을 `whole_dots`로(0이면 내지 않음). 끄면 floor에 최소 1.
    - 리스트 멤버: 늘 `whole_dots(대표 수 × each)`. 대표 수가 1보다 크면 상자를 멤버 ∪ (블록 ∩ 청크 범위)로 하고,
      한 변이 블록의 절반보다 작으면 그 변을 블록 전체로 한다.
    - `box_page`: 상자 이하 격자 페이지와 면적만 있는 페이지의 덮임에 `dot_units()`를 곱한다.
  - 단위: `with_the_occupancy_first_a_page_under_the_cut_is_spread_by_a_fine_grid_whatever_its_shapes`,
    `a_page_a_budget_leaves_out_is_drawn_by_its_occupancy_record_instead`,
    `under_the_brightness_an_items_cover_keeps_its_fraction`,
    `under_the_brightness_a_list_member_read_for_a_window_stands_over_its_block`,
    `under_the_brightness_a_page_no_wider_than_a_box_counts_in_sixteenths_too`.
  **셀의 덮임·노드 표본·항목 나눔(0.12.300, renderd 0.12.277; 리뷰어 2026-10-05, CUT_DENSITY_DESIGN §10.12
  "리뷰 보완 2").** 셋 다 `dot_bright`가 있는 점 계획에서만 걸린다.
  - `HierOpts::cell_cover = Some(table)`(`Vfs::plan_hier_in`의 마지막 인자; render-core `Cache::cell_cover`,
    `FLOE_RUST_DENSITY_CELL_COVER=off`가 킬 스위치).
    - `cover::CellCover`(`rust/vfs/src/cover.rs`): `areas(ovm, ci, rem)`이 셀의 (레이어, dbu²) 목록을 준다. 자기
      prange들의 페이지 면적(`Ovm::page_occ_area`, 레코드 없는 페이지는 0)에, `rem`이 0이 아니면
      `HierSummary::children(ci)`의 엣지마다 `members × areas(child, rem − 1)`을 더한다. `rem`이 REM_FULL이거나 셀
      높이 이상이면 셀마다 한 번(`OnceLock`), 그보다 작으면 (셀, rem)마다 한 번 계산해 둔다. 자식은 부모보다 낮은 셀만
      따라간다. `warm`은 top부터 전부 계산한다.
    - `Hier::cell_cover(ci, rem, rb)`: `cover_within(areas, 상자 면적, 보이는 레이어)` = 상자 × (1 − Π(1 − 면적 / 상자)).
      보이는 레이어는 `wash_vis`(요약이 그리는 레이어 제외)다. 계획마다 셀별로 기억한다(`cover_memo`, 깊이 제한은
      `cover_limited`).
    - `Hier::member_cover(ci, rem, rb)` = min(상자 단위, 덮임 × ppd² × `dot_units()`). 표가 없으면 `member_dots(rb)`다.
      쓰는 곳: `box_child`의 작은 경우, 리스트의 `fast`·`dot_chunk`·한 멤버씩 경로, `array_dots`의 멤버 면적, 배열을
      멤버로 그릴 때의 묶음, `box_node`의 합산, `node_holds`·`node_sampled`.
    - `Cache::cell_cover`: design.ovb가 있고 `HierHandle::summary()`가 되면 표를 만들어 둔다. 없으면 2초 뒤 다시
      찾는다. 표를 만들면 `floe-cover-warm` 스레드가 `warm`을 돈다(`FLOE_RUST_DENSITY_COVER_WARM=off`, 진단).
      renderd는 밀도 프레임 시작에 한 번 부른다.
  - `HierOpts::dot_node_sample`(기본 켬, `FLOE_RUST_DENSITY_NODE_SAMPLE=off`), `dot_node_read_all`(32),
    `dot_node_samples`(16).
    - `node_holds`는 켜져 있으면 늘 `node_sampled(ni, fp, r, (lo, hi), boxed)`다. 배치 n개가 `read_all` 이하면 전부,
      넘으면 k = `samples`개 구간 [lo + i·n/k, lo + (i+1)·n/k)에서 `block_dither(i, ni, item_salt(fp, ni))`로 하나씩
      읽는다. 읽은 값 × n/k를 더해 가다 `boxed`에 닿으면 u64::MAX(상자)다. 읽기 예산이 다해도 상자다.
    - `box_node`의 점 분기: 마스크가 있는데 위아래가 다르거나, 마스크 없는 노드의 배치가 `read_all`을 넘으면
      가장 위 레이어만 찾고 담은 것은 `node_sampled`로 센다. 마스크가 같고 계획의 레이어가 하나도 없는 노드는 읽지 않는다.
    - 끄면 `boxed <= 배치 수`일 때 상자, 아니면 전부 읽는다(0.12.299).
  - `HierOpts::dot_item_share`(기본 켬, `FLOE_RUST_DENSITY_ITEM_SHARE=off`; `dot_bright_sums`가 켜져 있어야 한다).
    - `add_dots`: 블록 이하 항목이라도 x나 y로 블록 경계를 넘으면 넓은 항목의 경로로 간다. 블록마다 든 면적만큼을
      `whole_dots`(담은 것이 상자보다 적으면 올림 나머지를 넘기는 기존 방식)로 싣고 조각은 그 블록 안 부분이다.
    - `array_dots`: 축마다 블록 [lo, hi) 안에 든 멤버 수를 면적으로 센다(통째로 든 멤버는 산술로, 걸친 멤버는 하나씩.
      멤버가 서로 겹쳐 길이 / 피치가 64를 넘으면 중심 세기로 남는다). 블록 값 = 가로 × 세로 × 멤버 값, 조각은 그
      멤버들의 블록 안 범위다.
    - 리스트: 멤버가 블록의 1/4보다 넓고 리스트 멤버가 `SUB_CUT_BOX_ARRAY_MAX` 이하면 빠른 경로와 `dot_chunk`를 쓰지 않고
      `add_dots`로 한 멤버씩 넣는다.
  - 통계: `dot_cover_on`, `dot_cover_cells`, `dot_node_sampled`(병합은 OR와 합).
  - 단위: `a_cells_cover_is_its_pages_and_its_childrens_by_layer`,
    `under_the_brightness_a_sub_cut_cell_stands_for_its_shapes_cover_not_its_box`,
    `under_the_brightness_a_node_counts_what_its_placements_hold_not_its_box`,
    `under_the_brightness_an_item_across_blocks_is_shared_between_them`,
    `under_the_brightness_an_arrays_and_a_lists_members_are_shared_between_blocks_by_their_area`,
    cover.rs `layers_cover_a_box_as_if_independent_and_never_past_it`.
  **읽기(0.12.256):** 점 모드에서 마스크 없는 노드의 배치 읽기는 셀마다 가장 위 가시 레이어를 한 번만 구해 두고
  (`cell_top`, `top_memo`) 순위만 비교한다; 계획기의 정수 키 맵은 Fx식 해시(`FxMap`/`FxSet`). 그림은 같다.
  **블록과 퍼뜨림(0.12.257; 사용자 2026-10-01 "지금보다 덜 자세해도 괜찮을 것 같음"):** 블록은
  `HierOpts::dot_block_px`(기본 **4 px** — 0.12.257~0.12.260은 8 px, 블록이 크면 점이 없는 곳·경계 너머에 찍힘; `FLOE_RUST_DENSITY_BLOCK_PX` 4~256 — 0.12.260 전에는 4~16; 블록당 상한 = 블록 픽셀의 절반,
  `dot_block_cap`)이고, 노드는 max(컷, 블록)까지만 내려간다 — 걷기량이 블록 크기의 제곱에 반비례한다.
  `HierOpts::dot_spread`(기본 켬, `FLOE_RUST_DENSITY_SPREAD=off`가 킬 스위치)이면 블록의 wash는 점들이 대표하는 범위
  (블록 안; 개수를 절반 이하로 담도록 얇은 쪽부터 중심에서 키움 — 작은 항목 하나는 전과 같은 상자)이고 개수는
  `WsCell::dot_counts`(washes와 나란함, `page_levels`와 같은 방식)에 따로 싣는다. 항목의 개수는 **담은 것까지**다:
  블록 크기 이하의 배열은 멤버 수 × 멤버당 점(상자 면적이 아니라), 레이어 마스크가 없는 노드(64배치 미만)는 이미 도는
  배치 읽기에서 `place_head`로 같은 값을 더한다(합이 상자 면적의 개수에 닿고 가장 위 레이어도 찾으면 멈춤; 마스크가
  있는 노드는 상자 면적). `Cache::plan_layer_only`는 개수를 wash와 함께 거른다. `BLOCK_PX=4 SPREAD=off`가 0.12.256의
  규칙이다. 단위 `the_dots_blocks_spread_what_they_count_and_count_what_is_there`.
  **점 블록 격자(0.12.268, renderd 0.12.247; 사용자 2026-10-02, 실칩 `cell dots 87.2M`):** `HierOpts::dot_grid`(기본 켬,
  `FLOE_RUST_DENSITY_DOT_GRID=off`가 킬 스위치)는 해시맵과 개수·합집합·순서가 같은 결과를 더 싸게 만든다.
  - 셀을 걷기 전에(`begin_grid`) 블록 격자 `DotGrid`를 연다. 범위는 셀의 뷰 상자들의 범위에 `box_px`와 두 블록을
    더한 것이고, 블록 수 상한은 `DOT_GRID_MAX` = 2^21이다.
  - 점 항목은 `put_dots`로 들어간다. 같은 블록이면 런(`dot_run`)에 합친다. 다른 블록이면 런을 `put_block`으로
    격자에 넣고, 격자 밖이면 `dot_blocks` 해시맵에 넣는다.
  - 격자의 블록은 레이어 순 목록이다. `flush_dots`는 처음 만난 블록 번호를 정렬해 읽고(번호 순서가 키 순서다),
    해시맵의 정렬된 블록과 `merge_by_key`로 합친다.
  - 점 리스트는 가장 위 레이어를 배치마다 한 번 구한다. 뷰에 통째로 든 청크의 멤버 중심이 한 블록이면 한 번에
    센다(`dot_chunk`). 점 수와 상한은 멤버별로 센 것과 같고, 상자는 멤버 상자들의 합집합(b0 + 청크 범위)이다.
  - `HierStats::dot_by`(9개)는 출처별 항목 수다. 출처는 노드·배치·배열·리스트 멤버·리스트 청크와 그 멤버·배열
    멤버·페이지이고, 마지막 값은 해시맵 갱신 수다. 항목 수(`sub_cut_dot_items`)는 청크 멤버를 뺀 합과 같다.
  - 단위: `the_dots_grid_counts_and_orders_the_blocks_as_the_hash_map_does`,
    `a_dot_grid_and_the_hash_map_beyond_it_drain_in_key_order`.
  **영역별 계획의 점 리스트(0.12.269, renderd 0.12.248; 사용자 2026-10-02, 실칩 `list members 86.6M`):**
  `HierOpts::dot_boxes`(기본 켬, `FLOE_RUST_DENSITY_DOT_BOXES=off`가 킬 스위치)는 다음 둘을 한다.
  - 점 리스트의 멤버를 셀 상자들의 범위가 아니라 상자 하나하나로 거른다. renderd가 번갈아 나눈 영역은 범위가 뷰를
    가로질러, 스레드마다 리스트의 멤버를 다 셌다.
  - `flush_dots`가 어떤 상자에도 통째로 들지 않는 블록을 뺀다(`dot_block_whole`, `HierStats::dot_partial`).
    - 판정: 블록의 중심(반 dbu 간격)이 모두 한 상자 안에 있거나, 상자가 셀의 끝까지 닿아야 한다.
    - 계획의 영역이 보는 블록은 늘 통째다(시작 상자가 한 블록 더 넓고, 자식의 상자는 부모 상자의 역상을 포함한다).
    - 따라서 영역별 계획이 내는 블록은 한 계획의 블록과 같고, `merge_plans`의 "개수가 큰 것"이 부분 개수를 고르지
      않는다.
  - 단위: `a_point_list_is_walked_by_each_box_and_a_block_no_box_holds_whole_is_left_out`.
  **점 리스트의 빠른 길, 꽉 찬 블록, 표본(0.12.280, renderd 0.12.258; 사용자 2026-10-03, 실칩 449 레이어
  `list members 765.7M`, `pass 2 plan 36130ms`):** 리스트의 청크마다(가장 위 레이어가 있고 멤버가 블록 이하일 때) 이
  순서로 본다.
  1. `HierOpts::dot_list_full`(기본 켬, `FLOE_RUST_DENSITY_LIST_FULL=off`가 킬 스위치): `chunk_zone`이 청크의 첫·마지막
     멤버의 Morton 키로 멤버가 놓인 구간을 구한다. 그 구간을 정렬된 사각형(블록 1/4보다 작게 자르지 않음,
     `CHUNK_ZONE_SQUARES` 24개까지)으로 나누고, 넓이는 키 차이(dbu²)다. 사각형들의 중심 블록(`CHUNK_FULL_BLOCKS` 64개까지)이
     모두 격자에서 상한이면(런 포함) 청크를 건너뛴다(`chunk_full`). 어떤 상자에도 통째로 들지 않는 블록은 본다 치지
     않는다. 상한 블록의 항목은 블록 전체라 결과가 같다. 통계 `dot_full_chunks`·`dot_full_members`.
  2. `dot_chunk`(한 블록이면 한 번에, 그대로).
  3. `HierOpts::dot_list_fast`(기본 켬, `FLOE_RUST_DENSITY_LIST_FAST=off`가 킬 스위치): 멤버를 오프셋에서 바로 블록으로
     세고, 같은 블록이 이어지면 `end_list_run`으로 한 번에 넣는다(add_dots와 같은 결과).
     `HierOpts::dot_list_sample`(기본 켬, `FLOE_RUST_DENSITY_LIST_SAMPLE=off`가 킬 스위치): `chunk_step`이 블록 면적당
     멤버 수(n × 블록² / 구간 넓이)를 `CHUNK_SAMPLE_PER_BLOCK`(16)으로 나눈 값이 2 이상이면 그 2의 거듭제곱(최대
     `CHUNK_SAMPLE_STEP_MAX` 32)을 step으로 한다. step번째 멤버만 읽고 step개로 센다. 통계
     `dot_sampled_chunks`·`dot_sampled_members`.
     `HierOpts::dot_list_by_dot`(0.12.295, 기본 켬, `FLOE_RUST_DENSITY_LIST_BY_DOT=off`가 킬 스위치): 멤버의 점 몫 each가
     0.5 미만이면 `by_dot_step(each)`(each × 창 ≤ 1인 가장 큰 2의 거듭제곱, 청크 256 이하) 창마다 하나만 읽는다.
     - 창 안의 멤버는 `block_dither(창 시작, 청크 번호, 청크 상자·레이어의 salt)`로 고른다.
     - 그 멤버가 창의 실제 멤버 수 × each 점을 낸다(`whole_dots`).
     - `dot_list_sample`의 step보다 클 때만 쓰고, stride가 1일 때만 쓴다.
     - 통계는 `dot_sampled_*`에 함께 센다.
  - 리스트의 `SUB_CUT_BOX_ARRAY_MAX`에는 하나씩 읽은 멤버만 센다. `dot_by[3]`(리스트 멤버)와 항목 수도 읽은 멤버다.
  - 단위 `a_list_member_under_a_dot_is_read_one_in_the_members_that_make_a_dot`(0.12.295):
    - 0.04 px² LEAF 3만 개 리스트 하나는 16개에 하나를 읽는다(1,875개). 점 수는 모두 읽을 때와 면적의 10 % 안이다.
      사분면 영역 넷으로 나눠도 같다.
    - 셀 여덟에 10개짜리 리스트 여덟(2,000 dbu, 블록 다섯에 걸침)은 리스트마다 한 번만 읽는다(640 → 64).
    - `a_member_under_a_pixel_stands_for_its_area_in_dots`는 이 옵션을 끄고, 빠른 길과 느린 길이 같은지 본다.
  - 단위: `a_point_list_chunk_in_full_blocks_is_passed_over_and_a_dense_one_read_at_a_step`.
    - 픽스처: 20 dbu 격자의 4,096점 리스트 둘(두 번째는 10 dbu 비킴, 블록마다 약 400개)과 무작위 3,000점 리스트.
    - 확인하는 것:
      - 세 경우(전부 읽기 / 건너뛰기 / 건너뛰기+표본)의 항목이 같다.
      - 두 번째 리스트의 청크 12개 이상을 건너뛴다.
      - 표본으로 읽은 멤버가 더 적다.
      - 성긴 리스트는 표본을 쓰지 않는다.
      - 사분면 영역 넷으로 나눠도 위가 그대로다.
  **리스트 단위 건너뛰기(0.12.281, renderd 0.12.259; 사용자 2026-10-03, 실칩 0.12.280 `list chunks in full blocks 7.1M
  of 69.1M members` - 청크 평균 10개, 표준 셀형 작은 리스트):** `dot_list_full`이 청크보다 먼저 리스트 전체를 본다.
  - 가장 위 레이어가 있고 멤버가 블록 이하이면, 멤버 중심의 범위(`chunk_blocks(b0, extent)`)의 블록이 모두
    상한이거나 통째가 아닌지(`blocks_full`) 본다. 그러면 영역도 만들지 않고 리스트를 건너뛴다. 그 청크 수와 멤버 수를
    `dot_full_chunks`·`dot_full_members`에 더한다.
  - `blocks_full`은 격자 안의 범위만 본다(격자 밖 블록은 통째가 아니다; `dot_boxes`가 꺼져 있으면 범위가 격자 안이어야
    한다). `LIST_FULL_BLOCKS`(2^16) 블록까지다.
    - 먼저 지난번에 찾은 덜 찬 블록(`full_witness`)을 본다.
    - 8×8 블록 묶음(`DOT_FULL_SIDE`)은 상한에 찬 통째 블록 수(`DotGrid::full`, 레이어별)가 그 묶음의 통째 블록
      수(`square_whole`, 셀 걷기마다 처음 볼 때 셈)와 같으면 한 번에 통과한다. 아니면 블록마다 본다.
    - `DotGrid::put`은 블록이 상한에 닿았는지(`GRID_FILLED`)를 돌려주고, `put_block`이 통째 블록이면 그 묶음에 센다.
      격자를 비울 때 함께 비운다.
  - `chunk_full`도 사각형마다 `blocks_full`을 쓴다. 64개(`CHUNK_ZONE_MIN`) 미만 청크는 구간을 구하지 않는다.
  - 단위: `a_small_point_list_in_full_blocks_is_passed_over_whole`.
    - 4,096점 리스트가 블록을 채운 뒤, 그 안의 8점 리스트 7개를 리스트째 건너뛴다.
    - 전부 읽은 경우와 항목이 같다.
  **점 계획의 예산 맞춤은 요청한 컷에서 한 번(0.12.271, renderd 0.12.250; 사용자 2026-10-02, 실칩 `fit 75019 ms x6
  passes on 1 threads`):** `HierOpts::dot_fit_at_cut`(기본 켬, `FLOE_RUST_DENSITY_FIT_LADDER=on`이 킬 스위치).
  - 점 계획(`sub_cut_dots`)은 `plan_hier`가 요청한 컷에서 한 번 끝까지 계획하고 `fit_at_cut`으로 맞춘다.
    - 예산이 통째로 담으면 everything이다.
    - 같은 컷의 기억된 결정이 담으면 그 결정을 쓴다(`fit_under`).
    - 둘 다 아니면 `thin_to_budget`으로 페이지만 우선순위대로 남긴다.
    - 한 페이지도 들지 않으면 든 페이지만 남기고 결정은 없다(`HierStats::fit_dropped`).
  - 셀의 컷(점이 되는 컷)은 바뀌지 않는다. 컷 사다리는 셀의 컷을 페이지의 컷과 함께 올려 단마다 다시 걸었다.
  - `fit_planned`도 점 계획이면(옵션 또는 `stats.sub_cut_dots`) 같은 맞춤을 하고, None을 내지 않는다.
  - 단위: `a_dots_plan_over_its_budget_keeps_the_cells_cut_and_thins_its_pages_in_one_pass`.
    - 픽스처: 페이지 18쪽, 예산 2쪽.
    - 결과: 한 번에 큰 등급의 두 쪽이 남고, 점은 예산 없는 계획과 같다.
    - 합친 계획도 같은 결과를 낸다.
    - 사다리는 여러 번 걷고 컷을 올린다.
    - 반 쪽 예산이면 페이지 0, `fit_dropped`.
  **하한 아래 페이지를 점으로 퍼뜨리기(0.12.272 선택, 0.12.277 기본):** `HierOpts::dot_page_spread`.
  - 0.12.277부터 기본으로 켜진다(사용자 2026-10-03 "퍼뜨리기를 기본으로 켜줘"). 단, 색인에 design.ovb가 있을 때만 효과가 있다.
    design.ovb가 없는 색인이나 기록이 없는 페이지는 이전처럼 그리지 않는다.
  - `FLOE_RUST_DENSITY_PAGE_SPREAD=off`가 킬 스위치다. `=on`은 기록이 없는 페이지도 상자 전체에 멤버 기준으로 퍼뜨린다
    (`HierOpts::dot_page_spread_boxes`, 0.12.272의 진단 동작).
  - 대상: 모든 도형이 가로·세로 모두 레코드 하한(`page_cut`) 아래인 페이지. 디코드하지 않는다.
  - 박스 이하이면 항목 하나다. 더 넓으면 `spread_page`가 셀 뷰(`cell_view`) 안의 블록마다 `page_dots × (블록 ∩
    페이지) / 페이지 면적`을 블록 디더(`block_dither`)로 반올림해 넣는다.
  - 페이지 BVH 노드도 같은 조건이면 통과시켜(박스 이하는 노드 항목) 페이지에서 퍼뜨린다.
  - **점유 격자(2026-10-02 0.12.274 비트, 2026-10-03 0.12.275 덮인 면적):** `HierOpts::dot_page_occ`(기본 켬,
    `FLOE_RUST_DENSITY_PAGE_OCC=off`가 킬 스위치).
    - 색인에 design.ovb(SPEC-FORMATS)가 있으면 `spread_page_occ`가 페이지의 64×64칸 중 도형이 있는 칸에만 점을 둔다.
    - `HierOpts::dot_occ_cover`(기본 켬, `FLOE_RUST_DENSITY_OCC_COVER=off`가 킬 스위치): 칸의 점은 그 칸의 덮인
      면적(px², 단계가 뜻하는 비율 × 칸 넓이)이다. 면적 그대로 켜는 래스터가 1 px 미만 도형을 켜는 양과 같다.
      끄면 0.12.274처럼 `page_dots`(멤버 수 × 가장 큰 도형의 면적)를 표시된 칸에 같은 몫으로 나눈다.
    - 블록은 그 블록에 든 칸 부분들의 몫을 받는다. 뷰와 무관하게 블록 전체 기준이라 스레드 병합이 그대로 맞는다.
      소수는 블록 디더로 반올림하고, 항목은 그 부분들의 합집합을 나타낸다.
    - 박스 이하 페이지는 표시된 칸들의 범위(`occ_bounds`)에 항목 하나로 놓는다. 점 수는 덮인 면적 합
      (`occ_area_px`)을 페이지 디더로 반올림한 것이고, 0이면 항목을 내지 않는다.
    - 격자 없이 면적 합만 있는 페이지(큰 도형이 있는 페이지, SPEC-FORMATS)는 상자 전체에 뿌리되 점 수는 그
      면적(px²)이다(`spread_page`의 몫을 면적으로). 박스 이하면 같은 반올림으로 항목 하나다.
    - 칸이 큰 페이지(0.12.278, `HierOpts::dot_occ_decode`, 킬 스위치 `FLOE_RUST_DENSITY_OCC_DECODE=off`): 하한 아래
      페이지라도 칸(긴 변 / 64)이 화면에서 `dot_occ_cell_px`(4 px)를 넘으면 컷을 건너뛰고 선택해 디코드한다(선형 경로와
      페이지 BVH 잎 모두, `decode_under_floor`). 그 페이지의 퍼뜨리기 항목(`occ_items`)은 셀 키와 함께
      `HierStats::occ_fallback`에 따로 둔다. `fit_at_cut`이 끝나면 `settle_occ_fallback`이 빠진 페이지의 항목만 셀의
      washes에 더하고, 남은 페이지 수를 `dot_occ_decoded`로 둔다. 병합된 계획은 (셀, 페이지, 레이어, 블록)마다 큰 쪽
      하나만 쓴다. 예산 맞춤이 없는 계획은 모두 디코드한다.
      - 0.12.294(`HierOpts::dot_occ_boxes`, 킬 스위치 `FLOE_RUST_DENSITY_OCC_BOXES=off`): 따로 두는 항목은 셀의 상자가
        통째로 담는 블록(`dot_block_whole`)만이다. `flush_dots`의 `dot_boxes` 규칙과 같다.
        - 전에는 셀 뷰(상자들의 경계) 안의 모든 블록이었다.
        - 영역을 스레드에 나눠 주면 스레드마다 같은 블록을 두었고, 맞춤이 그만큼 더 정렬했다.
      - 단위 `a_page_decoded_under_the_floor_keeps_aside_the_blocks_a_box_holds_whole`:
        - 영역은 두 상자다. 하나는 페이지의 왼쪽 아래 모서리, 다른 하나는 오른쪽 위 모서리 너머에 있다.
        - 두 상자의 경계는 오른쪽 위 모서리를 담지만, 어느 상자도 그 모서리를 담지 않는다.
        - 켜면 왼쪽 아래에만 항목이 선다. 끄면 오른쪽 위에도 선다.
      - 단위 `a_page_under_the_floor_too_coarse_for_its_grid_is_decoded_or_its_dots_stand_in`: 600 px 페이지(칸 9.4 px)는
        예산이 넉넉하면 디코드되어 점이 없다. 예산이 없으면 그 점이 퍼뜨리기와 같은 항목 수·점 수로 선다. 40 px 페이지는
        그대로 퍼뜨린다.
    - 페이지 BVH 노드(0.12.276, `node_dots`): 하한 아래이고 박스 이하인 노드는 노드 상자에 항목 하나로 선다. 점 수는
      노드 아래 페이지들의 덮인 면적 합(`Ovm::page_occ_area`: 합계는 그대로, 격자는 칸 단계 × 칸 넓이, 인덱스를 연
      동안 페이지마다 한 번 계산)을 노드 디더로 반올림한 것이다. 기록이 없는 페이지는 `page_dots`로 더한다. 0이면
      항목을 내지 않는다. 색인이 없거나 `dot_occ_cover`가 꺼져 있으면 이전처럼 `page_dots`의 합(최소 1)이다.
      `dot_occ_pages`는 색인을 쓴 노드를 하나로 센다.
    - 표시가 없는 캐시는 상자 전체에 뿌린다. 표시로 놓은 페이지 수는 `HierStats::dot_occ_pages`(renderd
      `density_plan2`의 24번째 값 `occ_pages`)다.
    - 퍼뜨리기가 켜져 있으면 renderd는 디코드된 페이지의 하한 아래 도형도 그린다(2패스 래스터의 아래 컷 0,
      `FLOE_RUST_DENSITY_UNDER_FLOOR=drop`이 킬 스위치). 큰 도형 때문에 디코드된 페이지의 작은 도형이 빈칸으로
      남지 않고, 퍼뜨려진 페이지와 같은 면적 기준이 된다.
    - 단위: `a_page_spread_over_its_occupancy_grid_puts_the_area_its_cells_cover`.
      - 6,000 dbu 페이지의 두 모서리 칸(0~15, 56~63)만 단계 15(전부 덮임)면, 점은 두 모서리의 블록 4×4와
        2×2에만 상한까지 찬다. 멤버 수와 무관하다.
      - 단계 11(1/16)이면 약 70점, 단계 8(2^−7)이면 멤버가 4만이어도 약 9점이다.
      - 박스 이하 페이지(1 px² 덮임)는 표시된 칸 쪽 블록에 1점이다.
      - `dot_occ_cover`를 끄면 4만 멤버는 모서리마다 상한, 400 멤버는 약 100점이다.
      - `dot_page_occ`를 끄거나 표시가 없으면 상자 전체(15×15 블록)다.
    - 단위: `a_page_without_a_grid_spreads_the_area_its_shapes_cover_over_its_box` — 합계 50 px²인 120 px
      페이지(멤버 4만, 멤버 기준 1만 점)는 상자 위에 약 50점, 2.5 px²인 박스 이하 페이지는 2~3점이다.
      `dot_occ_cover`를 끄면 멤버 기준(블록마다 상한)이다.
    - 단위: `a_page_bvh_node_under_the_floor_counts_the_area_its_pages_cover` — 4.95×7.5 px 잎 노드 둘(페이지 10개씩,
      멤버 기준 페이지당 10점, 색인 기준 0.5 px²)은 색인으로 5점씩, 끄거나 색인이 없으면 상자 상한 18점씩이다.
  - 단위: `a_page_under_the_floor_spreads_its_shapes_dots_over_its_box`.
    - 4만 멤버 페이지: 15×15 블록이 상한까지 찬다. 범위 안에만 그리고, 다시 계획해도 같으며, 구석 뷰는 그 블록들만 낸다.
    - 400 멤버 페이지: 블록 수보다 적은 약 100점이다.
    - 끄면 그리지 않는다.
    - 픽스처: 무작위 3,000점 리스트, 사분면 넷을 대각선 둘로 나눔.
    - 각 계획은 멤버의 70 % 미만만 센다.
    - 낸 블록은 한 계획의 블록과 같고, 자기 사분면이 보는 블록을 모두 내며, 둘이 합쳐 뷰를 덮는다.
    - 스위치를 끄면 멤버를 전부 센다.
    - `dot_block_whole`의 경계 사례.
  **한 번 걷기(0.12.259):** `HierOpts::dot_records = Some(몫)`(`Vfs::plan_hier_in`의 다섯째 인자,
  `PlanRequest::dot_records`)이면 페이지는 셀의 컷에서 고르고(`Hier::page_cut` = 컷), 래스터의 레코드 컷
  (`HierStats::shape_cut`)은 컷 × 몫, 크기 컷된 페이지는 크기와 무관하게 점 항목(`box_page`; 개수 `page_dots`)이다.
  `plan_hier`는 이때 예산 맞춤을 하지 않는다. 점 모드의 크기 컷 페이지 BVH 노드도 점 항목이고(퍼뜨림에서), 걷기가 직접 넣은
  wash는 `dot_counts` 0으로 맞춰 둔다. 여러 블록에 걸친 항목의 몫은 소수점을 다음 블록으로 넘긴다. 단위
  `the_dots_one_walk_takes_pass_1s_pages_and_dots_the_pages_under_the_cut`, `a_wash_the_walk_pushes_itself_carries_no_dot_count`.
  **16 px 넘는 블록(0.12.260):** 상한 256 px(개수는 u16, 256 px 블록은 최대 32,768). 마스크가 레이어를 답하는 노드(64배치
  이상)도 상자 면적의 개수가 배치 수보다 많으면(블록이 약 11 px를 넘을 때) 배치의 멤버를 읽어 담은 만큼 센다(`node_holds`;
  상자 개수에 닿으면 멈춤). 기본 8 px에서는 상자 개수(최대 32)가 배치 수(64 이상)보다 작아 그대로다. 단위
  `a_block_past_16_px_counts_what_its_items_hold`.
  **페이지 점 끔(0.12.261):** `HierOpts::dot_pages`(기본 끔, `FLOE_RUST_DENSITY_PAGE_DOTS=on`)가 꺼져 있으면 점 모드에서
  모든 도형이 페이지 컷 아래인 페이지(`box_page`)와 그런 페이지 BVH 노드(`walk_pbvh`)는 점도 wash도 아니고 그리지 않는다.
  **탐침(0.12.248):** `HierOpts::probe_limit > 0`(`Vfs::plan_hier_in`의 다섯째 인자, `PlanRequest::probe_limit`)이면
  예산 맞춤 없이 그대로 계획하되 선택한 페이지의 추정 디코드 메모리(`fit_bytes`)가 한도를 넘는 순간 그 패스를
  버린다(`fit_over`) — renderd가 점 모드 2패스의 페이지 하한(0 px)이 예약에 드는지 볼 때 쓴다(CUT_DENSITY_DESIGN
  §10.12 2단계; 들면 그 계획을 쓰고, 넘치면 밀도 컷 1 px와 예산 맞춤).
  **취소(0.12.250, renderd 0.12.235):** `HierOpts::stop`(`PlanStop { before, generation }`; `Vfs::plan_hier_in`의
  여섯째 인자)이 걸리면 — 새 세대가 frontier를 올리면 — 걷기는 다음 셀 확장 또는 노드 방문 1,024회 안에 패스를 끝내고
  `HierStats::cancelled`를 세운다(계획은 불완전, 버릴 것). `Cache::plan_cancellable(요청, 세대, RenderCancellation)`이
  그런 계획을 `render cancelled`로 거부한다; renderd의 프레임 계획(1패스, 예산 탐침, 2패스)이 모두 이 경로다(현장
  2026-09-30: 2패스 계획 중 확대가 계획이 끝날 때까지 기다렸다). 단위 `a_tripped_stop_ends_the_plan_at_once`.
- **팬 재사용도 결정을 따른다(0.12.237, renderd 0.12.226; 4883533 리뷰 2026-09-28).** (1) 보존 프레임은 자기가
  계획된 결정(`RetainedFrame::fit`)을 지니고, 요청은 그 배율의 기억된 결정과 **같은** 프레임만 재사용한다
  (`prepare_pan_reuse`의 `fit` 인자; 기억은 스냅 전 요청으로 구한다 — 키가 위치에 무관). 이 프레임의 계획이
  다시 결정하면(`fit_redecided`) 이미 받은 재사용을 버린다 — 재사용 타일은 옛 결정의 페이지라 새 결정의
  페이지 옆에 섰다(합성 데이터·1 MiB·3 px: 240 타일 재사용이 전체 렌더와 16,178 px 차이; 여백 off에서도).
  (2) 재사용 탐색은 같은 상태·결정의 **모든** 보존 프레임을 새것부터 훑어 배율·격자가 맞는 첫 프레임을 쓴다
  (종전엔 최신 프레임 하나만 보고 배율이 다르면 포기 — A→B→A에서 0 타일). 단위
  `pan_reuse_finds_an_older_frame_at_the_scale_and_none_under_another_fit`; 게이트 `fit_budget`: 다시 결정한 전체
  프레임은 구석 프레임의 타일을 하나도 쓰지 않고(`tiles_reused` 0) 새 워커의 렌더와 바이트 동일, 같은 뷰를
  다시 청하면 새 결정 아래에서 재사용한다. (3) 보존 단계(`store_retained`, 0.12.238 / renderd 0.12.227, 2차
  리뷰): 같은 상태·배율의 기존 프레임이 새 프레임을 **포함**하면 기존 것을 남기고 새 것을 버리는 규칙에도
  결정 일치가 필요하다 — 다른 결정 아래의 포함 프레임이 자리를 지켜 그 배율에서는 아무것도 다시 재사용되지
  않았다(컬러 A → 흑백의 조밀한 B에서 재결정 → A 복귀: A를 반복 요청해도 재사용 0). 단위
  `a_containing_frame_under_another_fit_does_not_keep_its_place`.
- **1패스 예산은 위 plane부터(0.12.318, renderd 0.12.293; 사용자 2026-10-07 — 기본, `FLOE_RUST_FIT_TOP_FIRST=off`면
  크기 등급만).** 현장: 실칩에서 7.59와 14.367만 켜면 각각은 보이는데, 함께 켜면 7.59만 나오고 `none below x28.2`였다.
  우선순위가 켜진 모든 레이어의 페이지를 크기 등급으로만 늘어놓아, 큰 도형이 있는 7.59의 페이지가 예산을 먼저
  차지하고 14.367은 통째로 빠졌다. 그리기는 위 plane부터인데 예산은 크기부터였다. 사용자: "상위부터 그려야 하니
  14.367이 그려졌어야", "위에서부터 그려도 스페클로 채우므로 아래 도형의 선은 나타난다 — 선이 많아 화면이 가득 차는
  건 어쩔 수 없다", "1패스는 2패스와 달리 스페클 구멍에 아래 도형이 그려져야 한다".
  - 순위: renderd(`pass1_fit_rank`)가 켜진 레이어마다 그리기 순위를 준다. 스타일 목록의 순서이고 마지막 줄(맨 위)이
    0이다. 스타일이 없는 켜진 레이어는 그 뒤다. 1패스 요청에만 실린다(`PlanRequest::fit_rank` →
    `HierOpts::fit_rank`). 2패스는 자기 순서(`density_top_first`)를 그대로 쓴다.
  - 우선순위: `fit_priority` = (순위, 크기 등급 역순, 비트 반전 위상, 페이지). 접두사는 그대로 엄격하다. 예산이 끝나는
    plane 위는 요청 컷에서 완전하고, 그 plane은 크기 등급 큰 것부터, 그 아래는 없다. `FixedFit`에 `rank`가 붙어 기억한
    결정도 같은 순서로 적용된다.
  - 패스(`plan_hier_ranked`): 요청 컷의 한 패스다.
    - 위 순위들이 예산을 넘긴 순위는 그 순간 수집을 멈추고 걷기의 레이어(`walk_vis`)에서 빠진다
      (`RankWalk::budget`). 프레임이 필요한 곳은 종전 조건대로 걷는다.
    - 그래서 패스는 예산이 닿는 plane만 담는다. 한 plane이 혼자 `FIT_OVERSHOOT` 예산을 넘을 때만 그 plane과 아래를
      떨군다(`fit_rank_over`).
    - 위가 자리를 남기면 그 plane만 하한을 한 옥타브씩 올려 다시 계획한다(크기 사다리를 그 plane에만; 맨 위 plane이면
      하한이 계획의 컷).
  - 기억한 결정의 적용(`plan_hier_fixed_ranked`): 요청 그대로의 패스가 통째로 들면 전부 남긴다(종전 규칙). 아니면 결정의
    plane 위는 완전, 그 plane은 결정의 등급 이상이고, 그 아래는 걷지 않는다.
  - 상태줄: `cut<…um top N whole, L/D (1/M below xF, none below xG), K left out to fit budget`. 위에서 온전한 레이어 수,
    예산이 끝난 레이어와 그 등급, 빠진 레이어 수다. 프레임 줄은 `fit_ranked`, `fit_layers_whole`, `fit_layer_edge`,
    `fit_layers_out`이다.
  - 1패스의 스페클 구멍: write-once 타일은 "마지막에 쓴 plane이 이긴다"와 바이트 동일이라, 위 도형의 스페클 구멍에 아래
    도형이 그려진다(바꾸지 않음). 게이트로 확인했다: 14.367의 30 µm 사각형 아래 7.59 픽셀 108,484개 중 6,172개가
    보인다. 2패스의 밀도는 위 도형이 덮는 곳(구멍 포함)에 그리지 않는다(shapes first).
  - 측정(합성 MAIN01 1/10, 전 레이어, 1350×971, cut 3 px, 1 GB):
    - 계획과 사전 계획 시간은 같다(fit 30·31 ms, ×4 8·10 ms, ×64 110·113 ms).
    - 그림: fit 뷰가 `top 225 whole, 56/3 (1/2, none below x4.31), 223 left out`이다. 종전은 `1/8 below x4.31, none
      below x2.16`이었다.
    - 위 plane의 작은 도형까지 그려 그리는 양이 늘었다. ×4 프레임이 111~115 → 157~182 ms다. 예산 64 MB에서는 위 25개
      레이어가 완전하고 프레임이 42~52 → 960~1,315 ms다.
  - 사다리 끝에서도 `FIT_OVERSHOOT` 예산을 넘는 plane은 빠지고 그 위는 완전하다. 이때 결정은 "전부"가 아니라 그 위
    plane들까지다(`thin_to_budget`의 `short`: 계획이 프레임보다 모자란 지점; 바닥 하한으로 다시 계획한 plane이 예산에 꼭
    맞을 때도 같다).
  - 777ee08 리뷰 수정(0.12.319, renderd 0.12.294):
    - 순위별 비용은 페이지마다 한 번만 센다. 같은 셀이 두 깊이에 놓이면 같은 페이지를 지닌 작업 셀이 둘이다. 이를 두 번
      세어(예산 17,152바이트에 25,728) 예산이 통째로 담는 프레임에서 아래 plane을 떨궜고 결정은 "전부"였다.
    - 걷기가 plane을 떨군 계획의 결정은 어떤 경우에도 "전부"가 아니다(`short`).
    - 기억한 결정을 다시 적용할 때, 결정의 plane을 그 등급부터(바닥 하한, 또는 맨 위 plane의 올린 컷) 계획하면 결정 밖의
      페이지를 아예 수집하지 않는다. 요청 그대로의 패스가 그 plane을 혼자 `FIT_OVERSHOOT` 예산 넘게 떨궜거나 결정 밖
      페이지를 지녔으면, 그 plane은 "잘림"이다(`fit_under`의 `lacks`). 종전엔 `top 2 whole`이라 했는데 결정한 프레임은
      그 plane의 100페이지가 빠졌다고 했다.
    - 15c464d 리뷰 수정(0.12.320, renderd 0.12.295): 결정이 그 plane에서 처음 뺀 크기 등급(`FixedFit::below`)을 지닌다.
      다시 적용할 때 남긴 페이지의 등급과 처음 뺀 등급 사이가 비어 있어도 같은 경계를 말한다(종전: 같은 뷰가 처음
      `none below x5.12`, 다시 `x20.5` — 결정의 등급 − 1을 썼다). 뺀 페이지로 끝난 결정은 그 등급부터 다시 모아 같은
      페이지가 경계가 되고, 자기 등급에서 끝난 결정(`short`: 하한 아래는 모으지 않았다)은 결정의 등급부터 모으고 경계는
      `below`다. 크기 순(킬 스위치)에서는 `below`가 늘 `u32::MAX`라 종전과 같다.
  - 단위 vfs `the_budget_fit_keeps_the_top_plane_first`, `a_plane_past_the_ladders_reach_is_left_out_and_the_planes_above_kept_whole`,
    `a_page_two_working_cells_hold_counts_once_against_the_planes_above`,
    `a_decision_applied_again_says_what_its_plane_lacks`; 게이트 `fit_budget`의 `top_first_checks`(SPEC-VALIDATION).
- 한계: 솎는 단위가 페이지라 밀집 영역이 페이지 크기의 조각으로 빈다. 인스턴스가 공유하는
  페이지는 모든 인스턴스에서 같이 빠진다. 접두사가 끝난 등급 아래는 표본도 남지 않는다
  (0.12.166은 모든 등급에 표본을 남겼지만 확대 시 포함 관계를 지킬 수 없었다). 추정이 실측보다
  작으면 decode 뒤의 검사가 여전히 오류를 낸다(안전망). 게이트 `tools/validate_fit_budget.py`,
  단위 `a_plan_over_its_decode_budget_keeps_its_cut_and_lowers_the_density`,
  `narrowing_the_view_never_removes_a_budget_fitted_page_that_stays_in_view`(밀도),
  `a_plan_over_its_decode_budget_is_planned_at_the_finest_cut_that_fits`(사다리).

### 도형 단위 컷 (shape cut, 0.12.173, 2026-09-20) — `thin keep`

사용자 결정: 페이지의 큰 도형 때문에 작은 도형이 살아남지 않게 하고, 컷 조건을 "두 변 중 하나라도
컷보다 작으면"으로 바꾼다(대상은 thin keep — 최종적으로 keep 하나로 합친다). 종전 keep: 페이지는
`max_w < cut && max_h < cut`일 때만 잘렸고(페이지의 **가장 큰** 도형 기준), hairline cut
(`max_min < cut × 0.5`)은 꺼져 있어 한 변이 컷보다 긴 가는 도형은 전부 남았으며, 래스터에는 도형별
검사가 없었다. 합성 MAIN01의 TOP 109/2 페이지(멤버 144만 개, 가장 큰 도형 12.2 × 14.0 µm)는
0.4 µm 배열을 9,000 µm 뷰까지 그리기 대상으로 남겼다(F2R-30).
- **기본 변경(0.12.214, 사용자 결정 2026-09-25):** thin keep의 기본은 아래 **긴 변 기준**(`ViewReq::shape_cut_max`,
  CUT_DENSITY_DESIGN §10.6)이다. 이 절의 작은 변 기준(`ViewReq::shape_cut`)은 `FLOE_RUST_SHAPE_CUT=min`일 때만
  쓰고, `off`는 도형별 컷을 하지 않는다. 상태줄은 `cut<…um (larger side)` / `(min side)`로 구분하고, 프레임 줄은
  `shape_cut_max=1|0`을 보낸다.
- `ViewReq::shape_cut`(renderd: `!exact && thin_keep`이고 `FLOE_RUST_SHAPE_CUT=min`일 때; 0.12.173~0.12.213의
  기본; 덱 패스·probe·CLI plan은 끔, `floe-index plan --shape-cut 1`로 진단). 컷이 0이면 없다.
- 플래너: 페이지는 **`max_min < cut`**이면 잘린다(`max_min` = 레코드별 min(w, h)의 최대, v6 —
  크기 컷과 hairline 컷을 하나로, 계수 1.0). 페이지 BVH 노드는 `min(max_w, max_h) < cut`이면
  통째로(아래 모든 페이지의 max_min이 그 이하). 잘린 페이지는 종전 size cut과 같은 길을 간다:
  footprint가 박스(4 px) 이하이면 sub-cut 박스, 아니면 사라진다(밀도 표현은 F2R-30의 다음 단계).
- 래스터: 계획이 컷을 실어 보낸다(`HierStats::shape_cut`, dbu; 예산 맞춤으로 컷이 올라간 패스면 그
  컷). `raster_page_records`가 **min(w, h) < 컷인 레코드를 건너뛴다**(사각형은 w·h, 폴리곤·path는
  한 멤버의 bbox; repetition은 멤버 크기가 같으므로 레코드째 — 멤버 열거도 없다). 변환에 배율이
  없어(`OrthoTransform`) dbu로 바로 비교한다.
- 바뀌지 않는 것: thin cull, exact·컷 0, 덱 합성, 셀·배치의 컷(셀 bbox 기준; 가는 셀은 종전대로
  `max_min < cut × 0.5`에서 서브트리째 빠진다 — 그 안의 도형은 새 규칙으로도 전부 컷이라 그림은 같다),
  점 query(그리지 않은 도형도 잡힌다).
- 예산에 맞춘 밀도의 크기 등급도 같은 변을 본다(`FitKey::SmallerSide`, 0.12.174): 상태줄의
  `1/M below xF, none below xG`는 이 컷에서 **작은 변** 기준이다(긴 변 기준에서는 `FitKey::LongerSide`).
- 상태줄 `cut<…um (min side)`, 프레임 줄 `shape_cut=<dbu> shape_cut_max=0`.
- **긴 변 기준 — 기본(0.12.214; 2026-09-24의 진단 `FLOE_RUST_SHAPE_CUT=max`, CUT_DENSITY_DESIGN §10.6):** `ViewReq::shape_cut_max` —
  페이지는 0.12.173 이전 규칙(`max_w < cut && max_h < cut`)으로만 잘리고, 래스터는 **긴 변**이 컷 미만인
  레코드만 건너뛴다(`HierStats::shape_cut_max`). 한 변이 컷보다 긴 가는 도형(헤어라인)은 남아 폭 우선
  그리기(RENDERER-TESTS §3)가 폭만큼의 확률로 솎는다. 자식 셀과 자식 BVH도 같은 긴 변 기준이다(0.12.206,
  리뷰 2026-09-25): 작은 변이 `cut × 0.5` 미만인 가는 자식 셀·서브트리를 자르던 헤어라인 규칙(`max_min < hair`,
  `min(w, h) < hair` — 전체 깊이, 유한 깊이의 fold, 배치 확장 네 곳)을 max에서는 끄고(`Hier::child_hair` = 0)
  두 변 모두 컷 미만인 크기 컷만 남긴다. 같은 선을 부모 셀에 직접 두든 자식 셀로 배치하든 그림이 같다
  (종전에는 자식 셀 쪽이 0 px). 0.12.214부터 thin keep의 기본이다(변수 없음 또는 `max`). 상태줄
  `cut<…um (larger side)`, 프레임 줄 `shape_cut_max=1`. `off`는 종전대로 도형별 컷 없음(가는 자식 셀의 헤어라인
  컷은 종전 그대로 남는다).

### sub-cut 박스 (0.12.168, 정확성 수정 0.12.169, **0.12.182부터 기본 꺼짐**)

**0.12.182(사용자 결정 2026-09-21): thin keep에서 박스를 끄고 컷 아래는 밀도 표현으로 해결한다.**
박스는 `FLOE_RUST_SUB_CUT_BOX=on`일 때만 켜진다(진단용; 아래 규칙과 게이트는 그대로 남는다).
이유: 보이는 레이어가 적으면 박스 계획이 컷에 걸린 서브트리를 배치까지 걷고 상한을 넘으면 프레임
전체를 다시 계획해, 449 레이어 실칩에서 10개만 켠 fit 근처가 20초를 넘었다(FLOE2_OPTIMIZATION
"알려진 문제 → 박스 기본 끔"). 밀도 표현이 들어오기 전까지 그런 뷰의 컷 아래 도형은 비어 보인다.

실칩·합성 관찰: `thin keep` 그림은 Calibre와 비슷하지만 크기 컷이 버린 것은 **사라진다**
(합성 MAIN01의 via 레이어 하나는 fit부터 ×4까지 빈 화면; Calibre는 모든 도형을 최소 크기로
남긴다). `ViewReq::sub_cut_box`(renderd가 일반 레이아웃의 `thin keep` 요청에 켠다; 덱 pass·
probe·exact·CLI 플랜은 아님)이면 크기 컷이 버리는 것을 **인덱스 메타만으로 그린 박스**로 남긴다.
페이지는 하나도 더 디코드하지 않는다.
- 크기 컷에 걸린 페이지·페이지 BVH 노드·배치 BVH 노드·배치 footprint가 화면에서 양축
  `sub_cut_box_px`(4 px) 이하이면 박스 하나이고 그 아래는 도형을 찾아 방문하지 않는다. 더 넓은
  노드는 내려간다(컷은 통째로 건너뛰던 곳) — 걷기와 박스 수가 화면 크기에 묶인다.
- **박스는 거기 정말 있는 것만 말한다**(리뷰 2026-09-19 P1 두 건). 레이어는 요청 depth 안에
  실제로 도형이 있는 것만: 페이지는 정확, 배치는 `cell_bits(자식, 남은 depth)` — full depth면
  재귀 마스크, depth 경계(남은 0)면 자기 도형 마스크(`lmask_direct`), 그 사이는 자식의 배치를
  걸어 구한다(패스 안 memo, 재귀 마스크가 허락하는 것을 다 찾으면 중단). 배치 BVH 노드는 아래
  **모든** 배치에 같은 질문을 하고, 그 셀이 가질 수 있는 보이는 레이어를 다 찾으면 멈춘다
  (`sub_cut_box_reads`; 배치당 약 7 ns). 종전: 재귀 마스크를 그대로 써 depth 1 뷰에 depth 2
  도형의 박스가 나왔고, 노드는 배치 8개만 뽑아 64개 중 하나에만 있는 레이어가 사라졌다. 읽기
  예산 6,400만(`sub_cut_box_reads` 옵션)을 넘으면 그 노드는 찾은 것만(참인 양성) 그리고
  `sub_cut_box_unsure`로 센다.
- 배열: 단일 배치는 자기 bbox(컷 미만). footprint가 박스 이하인 배열은 박스 하나. 더 넓은
  축정렬 grid는 **멤버마다 자기 bbox**이고, 멤버가 화면에서 정말 맞닿는 축(간격 ≤ 멤버 크기,
  또는 1 px 이하)만 한 줄로 잇는다. 점 목록(Pts)은 뷰 안의 점마다 박스. (리뷰 P1: 간격이 박스
  이하라고 이으면 0.5 px 멤버·3 px 간격의 30×30 배열이 900 px이 아니라 89×89 px 채움이 됐다.)
  한 배열의 뷰 안 멤버가 262,144개를 넘으면 0번부터 s개 간격으로 솎는다(`sub_cut_box_strided`).
  기울어진 grid는 종전대로 버린다.
- **박스는 대표하는 레이어마다 rect 하나**(0.12.172; 0.12.168과 같다). 0.12.171은 paint 순서상
  가장 위 레이어의 rect 하나만 남겼는데, "위 레이어가 아래를 덮는다"는 전제는 채움이 같은 픽셀을
  켤 때만 맞다. 리뷰 2026-09-20: 두 레이어의 150 px 박스에서 위 레이어 채움이 없음(clear)이면
  아래 레이어 혼자 11,700 px이던 것이 외곽선 600 px만 남았다. 어떤 레이어가 무엇을 가리는지는
  스타일을 아는 래스터의 일이고(스타일은 플랜을 새로 만들지 않고도 바뀐다), write-once 래스터는
  픽셀이 모두 쓰인 rect를 이미 건너뛴다. 레이어 집합은 paint 순위 비트셋(`LayerSet`, 최대 512개
  레이어)으로 다룬다. 비용(같은 부하에서 0.12.171 대비): 4 레이어 동일, 16 레이어 fit 0.61 →
  0.70 s, ×4 1.99 → 2.11 s.
- **보이는 레이어가 `sub_cut_box_layers`(16)개 이하일 때만**(0.12.168은 4). 상한을 올리고 없애
  잰 결과(칩 형태 합성 1/10, keep high, 끔 → 켬, 프레임 s / 켜진 픽셀):

  (아래 표는 0.12.171의 "rect 하나" 방식으로 잰 값이다. 레이어마다 rect를 내는 지금 방식은 16개
  이하에서 0~15 % 느리고, 32개 이상은 rect 상한에 먼저 닿아 더 나쁘다 — 결론은 같다.)

  | 레이어 | fit | ×4 | ×8 |
  |---|---|---|---|
  | 4 | 0.04 / 11만 → 0.26 / 63만 | 0.06 / 35만 → 0.67 / 131만 | 0.04 / 21만 → 0.30 / 56만 |
  | 16 | 0.04 / 18만 → 0.36 / 90만 | 0.13 / 60만 → 1.24 / 168만 | 0.05 / 45만 → 0.85 / 159만 |
  | 32 | 0.13 / 84만 → 0.53 / 102만 | 0.24 / 204만 → 2.19 / 207만 | 0.06 / 193만 → 1.44 / 205만 |
  | 128 | 0.36 / 85만 → 0.79 / 102만 | 0.83 / 전부 → 4.09 / 전부 | 1.21 / 전부 → 2.49 / 전부 |
  | 449 | 0.59 / 85만 → 1.28 / 102만 | 2.53 / 전부 → 5.87 / 전부 | 0.34 / 전부 → 2.99 / 전부 |

  16개까지는 빈 곳이 크게 메워지고(2~5배) 비용은 0.3~1.2 s. 32개부터는 ×2 이상에서 화면이 이미
  90 % 이상 차 있고, 128개 이상은 모든 픽셀이 켜져 있는데 박스가 1.3~3.3 s를 더 쓴다(박스는 위
  레이어 plane이라 먼저 칠해지고, 아래 레이어가 덮을 자리였다). 그래서 상한은 없애지 않고 16.
  진단 `FLOE_RUST_SUB_CUT_BOX_LAYERS`(최대 512).
- 플랜당 rect 200만 개 상한. 넘으면 **같은 패스를 한 단계 거칠게** 다시 계획한다(박스 2배,
  배열은 한 칸 걸러; 최대 3단계, `sub_cut_box_level`) — 상한에서 그냥 버리면 나중에 걷는
  셀만 비었다. 3단계에서도 넘으면 `sub_cut_box_over`.
- 박스는 자기 크기만큼 정직하다(실제 도형에서 box_px 이내). 2 px 이하 페이지 wash(M7-C)와 같은
  표현이라, 페이지가 박스였다가 컷 아래로 내려가도 계속 박스다.
- stats/frame line `sub_cut_boxes`·`sub_cut_box_over`·`sub_cut_box_level`·`sub_cut_box_unsure`,
  상태줄 `boxes N [x2 coarser] [(+K over)] [(U unsure)]`. 킬 스위치 `FLOE_RUST_SUB_CUT_BOX=off`,
  진단 `FLOE_RUST_SUB_CUT_BOX_PX`, `floe-index plan --sub-cut-box 1 [--sub-cut-box-px N]`.
  래스터는 wash를 128개씩 묶은 chunk 항목으로 받는다.
- 비용(칩 형태 합성 1/10, keep high, 프레임 s, 0.12.168 → 0.12.169): via 1 레이어 ×2 0.32 →
  0.57, 4 레이어 fit 0.52 → 1.18, ×4 1.07 → 1.62. 늘어난 것은 정확한 레이어 확인(plan +0.1~
  0.5 s, 읽기 2,000만~5,800만)과 이어 붙이지 않은 배열 멤버다.
- **v8 노드 레이어 마스크**(0.12.170, SPEC-FORMATS bvh): 노드 박스는 full depth면 `lmask_rec`
  합집합, depth 경계 한 단 위(r = 1)면 `lmask_direct` 합집합을 **읽기 없이** 그대로 쓴다. 그
  사이 depth에서는 두 마스크가 답의 하한·상한이고, 둘이 다를 때만(또는 마스크가 없는 작은
  노드·v8 이전 방식의 픽스처) 배치를 읽는다. 같은 마스크로 **보이는 레이어가 하나도 없는 배치
  서브트리를 노드째 건너뛴다**(`culled_bvh_layer`; 배치마다 하던 `cull_layer` 판정을 노드에서.
  계층 프레임은 레이어와 무관하게 그리므로 full depth이거나 프레임이 꺼진 요청에서만).
  프레임이 꺼졌는지는 **요청**이 말한다(`ViewReq::frames`, 0.12.172): 뷰어는 기본 옵션
  (`frame_cap` 200,000)으로 계획하므로 0.12.170~171에서는 뷰어의 frames 스위치가 래스터에만 닿고
  플래너에는 닿지 않았다(리뷰 2026-09-20: depth 0에서 프레임을 꺼도 경계 프레임 64개를 계획).
  `frames` false면 경계 프레임을 계획하지 않고, 프레임 때문에 보이는 레이어가 없는 서브트리를
  걷지도 않는다. renderd만 뷰어 스위치를 넘기고 덱·probe·CLI 플랜은 true. 같은
  측정: via ×2 0.57 → 0.22 s, 4 레이어 fit 1.18 → 0.41 s, ×4 1.62 → 1.12 s; via fit의 plan
  545 → 170 ms, 읽기 2,900만 → 290만. 박스 수와 켜진 픽셀은 그대로다.
- 게이트 `sub_cut_box`(tools/validate_sub_cut_box.py: 합성 칩 + 리뷰 재현 레이아웃 4개), 단위
  `what_the_size_cut_drops_stays_as_a_box_under_the_sub_cut_boxes`,
  `a_sub_cut_box_shows_only_what_the_requested_depth_holds`(마스크 있는 인덱스와 없는 인덱스가
  같은 박스), `a_sub_cut_box_keeps_every_layer_it_stands_for`,
  `a_subtree_without_a_visible_layer_is_pruned_by_its_node_mask`(frames 끔은 요청으로 넣는다).

## 4. 프레임 (cell reference outline)

r==0 경계에서 `frame_depth_boundary`:

1. 멤버 박스(rep 공유 치수) `양변<cut` → 컬.
2. **thin 대역**(min<cut≤max, rev 45): `frame_thin_lattice` — 셀-로컬
   thin_dbu(=7µm×unit) 2D 격자 대표만:
   - Grid: 축별 stride k=ceil(격자/피치) 서브그리드, 구간 경계 오프셋
     {0,k-1}(1열=빈당 2, 2D=모서리 4; k=1 축은 전체 유지 — 이미 성김),
     demote면 {0}만. 닫힌형(멤버 열거 없음).
   - One/Pts: (자식 ci, 빈x, 빈y) 해시 첫 레코드 승리(배치 순서 결정적),
     Pts 부분집합은 첫 유지 멤버 리베이스((0,0)-first).
   - huge-pts(>pts_full_rep)는 풋프린트 1박스(메모리 가드).
   - 양변<cut 소멸은 불변. 격자는 레이아웃 앵커 = 줌 불변 대표.
3. 정상 박스: rep 그대로 방출(융합 없음 — rev 39).
4. **밴드**(rev 42, `frame_band`): min변 px ≥FRAME_WHITE_PX(25)=0 흰
   외곽(디자인 위), ≥FRAME_GRAY_PX(9)=1 회색 외곽, ≥FRAME_FILL_PX(5)=2
   회색 채움, 그 외 3 점선("*." 라인스타일) — 델타에서 dt=frame_dt+밴드.
   px=0(레거시/프로브)은 전부 밴드 0.
- 블록명: 프레임이 25px 이상일 때만(텍스트 플래너), 흰/회 톤 동조.

## 5. 미니맵 프런티어 전개

`frontier_boxes(&Ovm, &HierPlan, keep) -> Vec<([i64;4], u8 band)>`
(rev 46): WS 트리를 월드로 전개(스택: (WsKey, Xf); 프레임 rect+rep
멤버, inst rep 멤버 → 재귀 push; each_rep_offset 공유 예산 8M) →
64×64 그리드 셀당 PER_CELL(4) 스트리밍 keep(strict-greater 교체) →
최대-우선 라운드로빈 ≤keep. 인덱서 굽기와 vfsd `mode=frontier`가 함께
사용 — L9 게이트가 둘의 일치를 고정.

## 6. 텍스트/라벨 (text.rs)

- 요청별 플랜: tbvh + 배치 BVH 워크(컷/헤어라인 프루닝; tbvh 노드는
  크기 주석 u32::MAX = 프루닝 금지), declutter 예산 선택(결정적),
  블록명은 r==0 && !below_cut 프레임에서만.
- 라벨은 per-gen 파일로 응답, 다음 요청 때 삭제. 오리엔테이션(0/1)
  포함 — 뷰어 회전 렌더는 #55(오버레이) 예정.

## 7. stats (plan JSON/응답에 노출)

wc_cells, wc_variants, inst_edges, frame_rects, visited_bvh,
cull_size/cull_layer/cull_page_size, culled_page_*, page_candidates,
pts_*, grid_fallback_full, kbox_merges, lod_swapped, washed_pages,
culled_bvh_size(rev 43), thin_frames(rev 45).

## 8. 불변식/함정

- **누락은 버그**: 보수 폴백(K-box 병합, skew bbox, pts 청크, i128
  포화)은 항상 추가 방향. 브루트포스 오라클(테스트 `brute`)은 페이지
  선택에 대해 "플래너 ⊇ 오라클"을 고정 — 오라클 술어는 `HierOpts`를 받아
  플래너와 같은 문턱(hairline × cut, `page_hairline`이면 페이지에도)을 쓴다
  (`brute_with(v, req, &opts)`; `brute`는 기본 옵션). 리뷰 2026-09-11: 기본
  정책이 바뀐 뒤에도 오라클이 옛 규칙(cut/2)으로 가는 페이지를 버려, 플래너가
  가는 페이지를 빠뜨려도 통과할 수 있었다. 테스트
  `page_hairline_option_on_the_pbvh_leaf_path_matches_the_oracle`이 선형 런과
  페이지 BVH 잎 경로에서 켜짐/꺼짐 각각 오라클과 **등식**으로 대조한다.
- cells() 등 카운트는 "슬롯 수" 함정 주의(M0 메모).
- 프레임 대표는 "화면 전역 최대"가 아니라 격자/그리드-공정 대표 —
  하강 가지치기(대표 최선성 붕괴)는 사용자 결정으로 금지.
- 좁힘 변환은 전부 checked("limit exceeded" 패닉).
