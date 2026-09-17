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
  **대표(page frontier)**(`ViewReq::page_reps`, 사용자 설계 2026-09-17, 같은 날
  리뷰 조건으로 단계화, 같은 날 현장으로 레벨을 항목 예산으로 결정; 단일 레이아웃
  요청에 켬, 덱 pass·exact·probe는 off): 컷이 버리던 것을 없애지 않고 **2^L개 중
  하나**를 남긴다. L은 프레임의 레벨: 먼저 **세는 pass**가 뷰 안의 컷 항목 수를 구하고
  (페이지는 `members`, 배치는 멤버 수 × 자식의 `rec_members`; 컷 자식 BVH 노드는
  인덱스 리더의 게으른 노드별 표 `Ovm::cbvh_member_log2`의 log2 합으로, `rep_items`),
  `HierOpts::rep_items`(기본 100만, `FLOE_RUST_REP_ITEMS_M`) 안에 드는 최소 L을 잡는다
  (`level_for`, `rep_level`; 예산 안이면 0). 한 옥타브 축소하면 뷰의 컷 항목이 4배라
  L이 2 오르고 1/4이 남는다("4개 중 1개"); 레이아웃 전체가 뷰에 들어오면 수가 더
  늘지 않으므로 L도 멈춘다 — fit 뷰는 예산만큼의 항목이 칩 전체에 퍼진 모습이다
  (앞서 run마다 "뷰에 맞은 줌"으로 상한을 두었더니 작은 run이 모두 레벨 0이 되어
  블록이 통째로 채워지고 디코드가 폭주했다). 솎기는 경로 전체에서 **한 번만** 적용하고
  단계가 나눠 맡는다.
  - 배치 단계(플래너): 컷 배치의 반복 멤버가 lm = min(L, ⌊log2 멤버 수⌋)를 맡아
    2^lm개 중 하나(Grid는 축 균형 stride `thin_grid`, Pts는 2^lm번째 slot; 멤버 0은
    남음)를 남기고, 셀 안 배치 index가 남은 L − lm을 맡는다(`place_rep`). 남긴 멤버는
    각각 **자식 bbox 한 개**를 자식의 보이는 레이어에 그린다(`rep_dots`, explain
    `rep_dots`): 컷 아래라 화면에서 cut px 이하의 점(hairline 셀은 가는 띠)이고, 그
    줌에서 인스턴스의 그림 그 자체다. 자식 페이지를 디코드하지 않고 자식 아래를 걷지도
    않는다(현장 2026-09-17: 자식을 통째로 그리면 픽셀 하나를 위해 자식의 모든 페이지를
    디코드했고 블록이 박스로 채워졌다). 배열 footprint 하나를 wash하지도 않는다(같은 날의
    fit 뷰 박스 하나). 세는 pass도 인스턴스 하나 = 항목 하나로 센다.
  - 페이지 단계(플래너): 페이지는 그릇이므로 뷰 안의 컷 페이지를 모두 남기되, 디코드
    예산(`HierOpts::rep_decode_bytes` 256 MiB, `rep_decode_bytes` 합)을 넘으면 플랜을
    다시 해 페이지를 run 안 index로 2^Lp개 중 하나만 남긴다(`rep_page_level`,
    `rep_replans`; 진단 `FLOE_RUST_REP_DECODE_MB`). 남긴 페이지는 L − Lp를
    `WsCell::page_levels`로 래스터에 넘긴다.
  - 레코드 단계(래스터, `thin_record`): 레코드의 반복 멤버가 min(level, ⌊log2 멤버 수⌋)를
    맡고 페이지 안 index가 나머지를 맡는다(2^lr의 배수만, index로 바로 건너뜀).
  집합은 frontier 격자 대표처럼 **아래로 포함**된다(S(L+1) ⊆ S(L): 넓은 뷰에 보이는
  것은 가까운 모든 뷰에도 있었고 축소 중 새로 나타나는 것은 없다). 화면 이동으로 수가
  2의 거듭제곱을 넘나들면 L이 1 움직인다(히스테리시스는 아직 없음). run의 첫 항목
  (index 0)은 어느 줌에서든 후보다. BVH 프루닝: 페이지 BVH는 run 구간에 2^Lp의 배수가
  없으면 건너뛰고(Lp = 0이면 모두 내려간다), 자식 BVH는 L에서 노드 아래 가장 무거운
  반복의 ⌊log2 멤버⌋를 뺀 모듈러스 하한으로 구간을 건너뛴다(`rep_pruned`). 세는
  pass는 컷 페이지 메타데이터를 모두 읽고(O(뷰 안 컷 페이지)) 자식 BVH는 표로 O(노드)다.
  대표는 sub-cut 예산을 타지 않는다. 진단용 sub-cut 규칙의 wash 판정에 쓰는 ink
  추정은 멤버 수 × 최소변 × 긴변(px). 대표는 부분만 보이는 무늬이지 요약처럼 채워진
  면이 아니다. 킬 스위치 `FLOE_RUST_PAGE_REPS=off`(뷰어), `floe-index plan
  --page-reps 1`; 상태줄·perf 줄 `reps K pages/C children [L n] [P n]`.
  gate `PageFrontierTests`(121만 hairline이 32 페이지: 모든 줌에서 32 페이지 모두 남고
  L = 1(100만 예산), 절반 뷰는 L 0; 12,100선의 희소 레이어는 L 0으로 전부, 네 사분면,
  200 px 밀도 > 800 px 밀도; `FLOE_RUST_REP_ITEMS_M=0.001`이면 L 4로 1/16 픽셀·포함
  관계; `FLOE_RUST_REP_DECODE_MB=1`이면 re-plan해 16번째 페이지만·구간 프루닝; L 두
  선은 wash 없이 선 두 개; 킬 스위치는 0 px), `SubCutTests`(예산 안 = L 0, 진단 규칙과
  같은 픽셀), `ThinPageTests`·`test_render_detail…`.
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
