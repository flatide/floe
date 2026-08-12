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

## 3. 컷/생략 사다리 (정확한 술어)

- 페이지: `(max_w<cut && max_h<cut) || max_min<hair` (v6 필드; 선형·
  pbvh 리프 동일).
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
  선택에 대해 "플래너 ⊇ 오라클"을 고정 — 오라클 술어는 hairline 0.5
  기본을 미러링(cut/2)하므로 규칙 변경 시 함께 갱신.
- cells() 등 카운트는 "슬롯 수" 함정 주의(M0 메모).
- 프레임 대표는 "화면 전역 최대"가 아니라 격자/그리드-공정 대표 —
  하강 가지치기(대표 최선성 붕괴)는 사용자 결정으로 금지.
- 좁힘 변환은 전부 checked("limit exceeded" 패닉).
