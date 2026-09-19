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
| wash_px | 2.0 | 워시 문턱 px (0=off) |
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
  화면px² → lod_page로 교체. 프로브는 px=0이라 구조적으로 exact.
- 워시: 페이지 화면상 양축 ≤ wash_px → (layer, bbox) 렉트로 붕괴.

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

#### 예산에 맞춘 밀도 (0.12.166, 선택 규칙은 0.12.169 — 기본; 위 사다리는 `FLOE_RUST_FIT_THIN=off`)

컷을 올려 크기 등급을 통째로 버리는 대신 **밀도를 낮춘다**(`plan_hier_thinned`,
`thin_to_budget`). 예산을 넘는 플랜은 자기 페이지를 **하나의 고정 우선순위**로 늘어놓고, 예산에
들어가는 **가장 긴 접두사**만 남긴다(`fit_priority`).
- 1순위 크기 등급, 큰 것 먼저. 등급 = 그 페이지가 아직 선택되는 가장 큰 컷(`fit_key`: keep은
  긴 변의 최대, cull은 min(긴 변, 짧은 변 ÷ hairline))의 **옥타브**(절대 dbu — 등급이 뷰에 따라
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
- 한계: 솎는 단위가 페이지라 밀집 영역이 페이지 크기의 조각으로 빈다. 인스턴스가 공유하는
  페이지는 모든 인스턴스에서 같이 빠진다. 접두사가 끝난 등급 아래는 표본도 남지 않는다
  (0.12.166은 모든 등급에 표본을 남겼지만 확대 시 포함 관계를 지킬 수 없었다). 추정이 실측보다
  작으면 decode 뒤의 검사가 여전히 오류를 낸다(안전망). 게이트 `tools/validate_fit_budget.py`,
  단위 `a_plan_over_its_decode_budget_keeps_its_cut_and_lowers_the_density`,
  `narrowing_the_view_never_removes_a_budget_fitted_page_that_stays_in_view`(밀도),
  `a_plan_over_its_decode_budget_is_planned_at_the_finest_cut_that_fits`(사다리).

### sub-cut 박스 (0.12.168, 정확성 수정 0.12.169)

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
- **보이는 레이어가 `sub_cut_box_layers`(4)개 이하일 때만**: 박스는 그러지 않으면 빈 화면일
  뷰를 위한 것이고, 레이어가 많은 뷰는 이미 차 있으며 박스는 레이어마다 걷기와 paint가 든다.
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
  계층 프레임은 레이어와 무관하게 그리므로 full depth이거나 프레임이 꺼진 요청에서만). 같은
  측정: via ×2 0.57 → 0.22 s, 4 레이어 fit 1.18 → 0.41 s, ×4 1.62 → 1.12 s; via fit의 plan
  545 → 170 ms, 읽기 2,900만 → 290만. 박스 수와 켜진 픽셀은 그대로다.
- 게이트 `sub_cut_box`(tools/validate_sub_cut_box.py: 합성 칩 + 리뷰 재현 레이아웃 4개), 단위
  `what_the_size_cut_drops_stays_as_a_box_under_the_sub_cut_boxes`,
  `a_sub_cut_box_shows_only_what_the_requested_depth_holds`(마스크 있는 인덱스와 없는 인덱스가
  같은 박스), `a_subtree_without_a_visible_layer_is_pruned_by_its_node_mask`.

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
