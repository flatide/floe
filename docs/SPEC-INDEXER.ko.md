# SPEC: 인덱서 (floe-index vfs)

정본 코드: `rust/cli/src/vfs.rs` (`vfs_cmd`, `build`,
`build_cell_plan`, `frontier_json_planned`). 이력: `rust/VFS_HIER.md`
"빌드 병렬화 2차/3차", "phase 2-A", rev 46b.

## 1. 명령

```
floe-index vfs <src.oas> [outdir=.floe] [--jobs N] [--plan-batch N]
    [--encode-batch N] [--page-target-mb N] [--no-lod]
    [--coverage | --coverage-only] [--frontier-only] [--kill-at P]
    [--slow-cell-s S] [--p2-shard-limit-mb N]
```

`--slow-cell-s S` = slow-cell 로그 임계 초(기본 5.0, 0 = 전 셀 —
게이트/계측용), `--p2-shard-limit-mb N` = P2 arena 샤딩 복사 명시
상한(미지정: Linux = MemAvailable 여유 절반, off-Linux = 무제한).

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
       Grid 인덱스 분할/Pts rebase 분할, oversize 격리, v6 max_min 계산).
       **셀별 분할 모드**(#60): 지배 레이어(최대 레이어 ≥60% AND
       ≥8MiB) → **P2 서브트리 팬아웃**, 아니고 레이어≥2·입력≥4MiB →
       **P1 레이어 팬아웃**, 그 외/헬퍼 0/MemAvailable<4GB → 직렬.
       셀당 한 모드만 — 중첩하면 P1이 전 레이어 종료까지 헬퍼를
       쥐고 있어 지배 레이어의 P2가 항상 예산 0을 본다. 예산 슬롯은
       종료한 플래너만 반환하므로 두 팬아웃 모두 몬스터-셀 꼬리
       최적화다(윈도 대기 플래너는 슬롯 보유; 일반 위치까지 가속 =
       통합 워커 풀, 후속).
       - **P1**: 비어있지 않은 레이어가 태스크 단위, ≤8 스레드
         (무거운 레이어 우선), 결과는 li 순서 병합(pages/pbvh
         리베이스 + compact **arena shard** slot) → 바이트 불변.
       - **P2**: 지배 레이어 내부 — 직렬 프리픽스 전개(노드 1스텝
         `split_node`를 직렬 재귀와 공유)로 frontier 태스크 생성
         (target = 스레드×4, cap 32, cutoff = max(입력/target,
         2MiB)), 세그먼트 리스트가 DFS 방출 순서(노드 oversize →
         left → right)를 기록. 태스크 생성 시 **arena sharding**:
         Frag::Pts 참조 범위를 태스크-로컬 shard로 복사(태스크만
         참조하는 원본 엔트리는 즉시 해제 — 일시 오버헤드 = 엔트리
         1개분), 실행은 무거운 태스크 우선, 병합은 세그먼트 순서 +
         **seq 재부여** + shard slot(프리픽스 → 태스크 순) 지정,
         pbvh는 병합 완료 후 레이어당 1회 → 바이트 불변. 헬퍼 0이면
         frontier/sharding 자체를 만들지 않는 완전 직렬 폴백.
         **shard-복사 한도**: 예상 복사량(레코드 범위 합×4B, 복사
         전에 정확히 계산됨)을 **결정 시점**(프리픽스·타 레이어·타
         플래너 할당 이후)의 MemAvailable 여유 절반과 대조하고,
         동시 P2 셀 간 경쟁은 **전역 reservation**(SHARD_RESERVED,
         결정→태스크 실행 종료까지 유지)으로 중재 — 초과 시
         frontier는 유지하되 태스크를 프리픽스 arena로 **직렬**
         실행(복사 0, **helper lease는 결정 즉시 반환**되어 꼬리가
         LOD/타 셀에 재대여, 로그 `p2_mem_fallback=1`·split 1t
         보고). off-Linux는 MemAvailable이 없어 기본 무제한 —
         `--p2-shard-limit-mb`로 명시 상한. 수 GB 단일 Pts
         엔트리가 OOM을 만들지 않는다. **기아 계측**: P2 대상인데
         셀 시작 시점 여유 슬롯이 0이면(jobs>1·usable CPU>1 조건 —
         풀로 해결 가능한 사례만) `p2_eligible=1 helpers=0` 표시 +
         빌드 말미 집계 한 줄 — 통합 워커 풀(#76)의 실측 근거.
         남은 지렛대 = 프리픽스 파티션 병렬화(P2-ext, 미착수).
     - **LOD 변종 생성**: 후보 members≥256; 후보 ≥64(LOD_PAR_MIN)면
       셀 내부 스레드 팬아웃(≤16, 같은 공유 예산), cand 순서 병합 →
       바이트 불변. `--no-lod`면 후보 목록 자체를 비움(전 페이지
       LOD_PAGE_NONE).
     - CellSink 프리인코딩.
   - 커미터(메인): 순서대로 `append_cell_sink` 리베이스, lod_page
     전역화, 텍스트/비트셋/cell 레코드 커밋. 윈도 채워지면 청크 인코드
     + 아레나 해제. 메모리 거버너: MemAvailable<4GB면 윈도 반감.
   - **slow-cell 로그**: plan이 임계(기본 5s, `--slow-cell-s`,
     0=전 셀; 환경변수 아님 — --kill-at과 같은 CLI-상태 규칙) 초과인 셀을 stderr로:
     `slow cell NAME (ci N/total): plan Xs (places, pages, frag
     splits; bvh asm split/Nt[ p2_tasks=K shard=MMiB] lod/Nt pts
     sink; layers A, top L<layer>.<dt> Xs ...)` — split/lod 스레드
     수·P2 태스크/샤딩량·레이어별 split 상위 3개가 팬아웃 효과의
     실측 근거(150M 실측: ESD dummy 164.3→42.5s).
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
