# SPEC: 인덱서 (floe-index vfs)

정본 코드: `rust/cli/src/vfs.rs` (`vfs_cmd`, `build`,
`build_cell_plan`, `frontier_json_planned`). 이력: `rust/VFS_HIER.md`
"빌드 병렬화 2차/3차", "phase 2-A", rev 46b.

## 1. 명령

```
floe-index vfs <src.oas> [outdir=.floe] [--jobs N] [--plan-batch N]
    [--encode-batch N] [--page-target-mb N] [--no-lod]
    [--coverage | --coverage-only] [--frontier-only] [--kill-at P]
```

## 2. 파이프라인 (스트리밍, rev 44)

1. **파스**: `std::fs::read` 전체 → `parse_doc` (진행 하트비트 스레드가
   경과/RSS를 stderr로 틱). 150M급 실측 ~155s — 단일 스레드, 수용됨.
2. **rbbox**: `cell_bboxes_full` (직렬 1패스; 알려진 미결).
3. **본 빌드** (`build`): 셀들을 topo 순서로 **지속 워커 풀 + 순서
   커미터** 파이프라인 처리.
   - 워커: 입장 제어(`ci >= commit_base + window` 시 1ms 슬립) 후
     `build_cell_plan` — phase 타이머 6칸 [bvh/asm/split/lod/pts/sink]:
     - 인스턴스 BVH(리프 순서=방출 순서, v7 max_dim/max_min 주석),
     - (cell,layer) 런 조립 + **rep-split**(목표 페이지 크기 초과 시
       Grid 인덱스 분할/Pts rebase 분할, oversize 격리, v6 max_min 계산),
     - **LOD 변종 생성**: 후보 members≥256; 후보 ≥64(LOD_PAR_MIN)면
       셀 내부 스레드 팬아웃(≤16), cand 순서 병합 → 바이트 불변.
       `--no-lod`면 후보 목록 자체를 비움(전 페이지 LOD_PAGE_NONE).
     - CellSink 프리인코딩.
   - 커미터(메인): 순서대로 `append_cell_sink` 리베이스, lod_page
     전역화, 텍스트/비트셋/cell 레코드 커밋. 윈도 채워지면 청크 인코드
     + 아레나 해제. 메모리 거버너: MemAvailable<4GB면 윈도 반감.
   - **slow-cell 로그**: plan 5s 초과 셀을 이름/ci/phase 내역과 함께
     stderr 출력 (몬스터 셀 표적 목록; 150M 실측: ESD dummy 164.3→42.5s).
4. **ovt/ovp 쓰기** → **meta.json**(`emit_viewer_side`) →
   **design.ovm 커밋**(빌드 바이트를 `Ovm::from_bytes` 딥 검증 통과
   후 파일로 — 검증이 공짜 게이트).
5. **커버리지**(--coverage): design.ovc 생성.

로그 라인: `[vfs] build: streaming pipeline N cells (J workers, plan
window W, encode batch E, page target M MiB[, lod off])...` →
`pipeline complete (wall/commit/encode)`.

## 3. 결정성 (하드 게이트)

산출 바이트는 `--jobs`/윈도와 **무관하게 동일**해야 한다. 커미터가
ci 순서를 강제하고, 셀 내부 병렬(LOD)도 cand 순서로 병합. 게이트:
jobs 1 vs N sha256 비교(스위트), `cell_sink_append_is_byte_identical`.

## 4. 마커 프로토콜 / --kill-at

완성 마커 = design.ovm 존재(마지막에 커밋). `--kill-at` 지점:
`marker-deleted`, `ovp-written`, `ovt-written`, `ovm-partial` — 각
지점에서 강제 종료해도 뷰어가 미완성 캐시를 완성으로 오인하지 않음을
`tools/validate_vfs_marker.py`가 고정.

## 5. 미니맵 프런티어 굽기 (rev 46b)

design.ovm 커밋 직전, **실제 플래너로** depth별 프레임 집합을 굽는다:

- 정준 파라미터: `FRONTIER_CANVAS_PX=1200`(×1.05 fit 마진),
  `FRONTIER_CUT_PX=3.0`(medium), hair/thin 기본값.
- depth 0..min(height, FRONTIER_DEPTH_CAP=32) 각각 `plan_hier` →
  `floe_vfs::hier::frontier_boxes`(WS 월드 전개: xf 합성 + 프레임/
  인스턴스 rep 멤버, 워크 가드 8M) → 64×64 그리드 셀당 4개 스트리밍
  keep → 최대-우선 라운드로빈 ≤ FRONTIER_KEEP(6000).
- meta.json `frontier`에 [x0,y0,x1,y1,band] 행으로 저장. 빈 꼬리 깊이
  제거. **결정적**(순회 순서+strict-greater 교체).
- 구 DFS 굽기(rev 30: min_div/scan budget)는 삭제됨 — 지역 누락 원인.

`--frontier-only`: 소스 파스 없이 기존 캐시의 design.ovm만 열어
frontier를 재계산하고 meta.json의 해당 객체만 brace-balanced splice로
패치(`patch_meta_frontier`). 9.8G도 초 단위.

## 6. plan 명령 (계측)

```
floe-index plan <cachedir> --view x0,y0,x1,y1(µm) [--px-per-um F]
    [--cut-px F] [--depth N|full] [--layers ...] [--lod 0]
    [--wash-px F] [--hairline-f F] [--thin-um F] [--inspect]
```
JSON 출력: pages/bytes/records/members + 플래너 stats 전체
(frame_rects, culled_*, lod_pages, washed_pages, culled_bvh_size,
thin_frames, plan_ms …). 실칩 병목 확정용(“플랜 vs 파스 vs 드로우”).

## 7. 알려진 미결

- 파스·rbbox 직렬(수용), split 재귀 병렬화(#60 2-B: PtsArena order
  in-place 분할이 형제 병렬을 막음 — 아레나 선생성+분리 재설계 필요).
- split_pages seq u16 오버플로 가능성(이론), fragments 카운터 과대
  표시(0.8.2 이후, 계수만 문제).
