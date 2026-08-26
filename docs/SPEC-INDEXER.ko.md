# SPEC: 인덱서 (floe-index vfs)

정본 코드: `rust/cli/src/vfs.rs` (`vfs_cmd`, `build`,
`build_cell_plan`, `frontier_json_planned`). 이력: `rust/VFS_HIER.md`
"빌드 병렬화 2차/3차", "phase 2-A", rev 46b.

## 0. Python 사용자 명령

기본 `floe index <src.oas>`는 `floe-index vfs <abs-src>
<abs-src>.floe`를 shell 없이 subprocess로 실행한다. `--jobs`,
`--page-target-mb`, `--coverage`/`--coverage-only`, `--no-lod`,
`--slow-cell-s`, `--p2-shard-limit-mb`, `--profile-cell`/
`--profile-cell-ci`, `--profile-jobs`, `--profile-repeat`,
`--profile-snapshot`/`--profile-snapshot-refresh`를 같은 이름의 Rust
옵션으로 전달한다.
coverage는 viewer 기본값과 맞춰 opt-in이다.

정상 VFS cache의 cache version과 source size/mtime fingerprint가 맞고,
`floe-index vfsd`의 `Vfs::open` 검증(OVM 구조 + OVP/OVT committed length)을
통과하면 재사용한다. 기존 cache가 stale/incomplete/non-VFS/corrupt이면
`--force` 없이 Rust를 실행하지 않는다. 즉 `--force`만 기존 `<src>.floe`를
교체할 권한이다. 현재 cache에 `--coverage`를 지정하면 `--coverage-only`로
`design.ovc`만 비파괴 추가한다.

binary 검색 순서는 `FLOE_INDEX_BIN`, 개발 트리
`rust/target/release/floe-index`, Python 실행 파일 인접 `floe-index`, PATH다.
명시한 `FLOE_INDEX_BIN`이 실행 불가하면 다른 후보로 폴스루하지 않고 hard
error다. 전체 누락도 빌드/설치 지침을 포함한 hard error다. 동결된 Python/KLayout `.tiles`
인덱서는 명시적 `--legacy`에서만 사용하며 그 전용 옵션을 기본 Rust 경로와
섞으면 hard error다.

## 1. 명령

```
floe-index vfs <src.oas> [outdir=.floe] [--jobs N] [--plan-batch N]
    [--encode-batch N] [--page-target-mb N] [--no-lod]
    [--coverage | --coverage-only] [--frontier-only] [--kill-at P]
    [--slow-cell-s S] [--p2-shard-limit-mb N]
    [--profile-cell NAME | --profile-cell-ci N]
    [--profile-jobs N,N,...] [--profile-repeat N]
    [--profile-snapshot PATH] [--profile-snapshot-refresh]
```

`--slow-cell-s S` = slow-cell 로그 임계 초(기본 5.0, 0 = 전 셀 —
게이트/계측용), `--p2-shard-limit-mb N` = P2 arena 샤딩 복사 명시
상한(미지정: Linux = MemAvailable 여유 절반, off-Linux = 무제한).

### 1.1 단일 셀 비게시 프로파일

`--profile-cell NAME`은 정확한 셀 이름, `--profile-cell-ci N`은 slow-cell
로그의 0-based `ci`로 셀 하나를 고른다. 소스 전체 파스와 recursive bbox
준비는 실제 빌드와 같이 수행하지만 `build_cell_plan`은 선택 셀 한 번만
호출한다. 출력 디렉터리를 생성·삭제하지 않고 OVM/OVP/OVT/meta/coverage를
전혀 쓰지 않는다. 기존 cache가 있어도 Python CLI는 freshness/`--force`
경로를 거치지 않고 프로파일을 실행한다.

stderr는 진행과 사람이 읽는 요약, stdout은 반복 측정용 JSON이다. 기본 단일
실행은 기존과 같은 객체 하나이고, jobs/반복 행렬은 같은 객체들의 JSON 배열이다.
JSON은 read/parse/prepare/common inventory/plan, 셀 6페이즈, 레이어별 serial split 및 P2
`prefix/shard/tasks wall/tasks sum/task peak/merge/pbvh`, 작업량, RSS/HWM과
retained Pts arena를 기록한다. `tasks sum`은 각 태스크 wall의 합이며 OS CPU
clock은 아니다. 프로파일러 호출 스레드가 한 실행 슬롯을 소유하고 나머지
`jobs-1` 슬롯은 즉시 P1/P2/LOD에 제공되므로, 전체 빌드의 다른 셀/commit
window 경합이 없는 **격리 셀 확장성**을 측정한다.

`--profile-jobs 8,12,16`은 `--jobs`로 소스를 한 번만 파스하고 recursive
bbox/layer map도 한 번만 준비한 뒤, 지정한 planner job 수를 순서대로 실행한다.
`--profile-repeat N`은 각 job 수를 N번 반복한다. 각 실행이 끝날 때 `CellPlan`과
arena를 먼저 drop하므로 결과는 동시에 겹치지 않으며, 첫 실행의 cold allocator
효과도 숨기지 않고 `settings.repeat`로 구분한다. 공유 서버의 운영 후보를 비교할
때는 source parse도 `--jobs 16` 이하로 묶는다. 배열 안의 `timing_s.plan`은 해당
실행만의 wall이고 `timing_s.total`은 명령 시작부터 해당 실행 종료까지의 누적 wall이다.

`--profile-snapshot PATH`는 선택 셀의 rect/poly/path/place, 공유 Pts repetition
pool, 전체 recursive bbox와 layer order를 일반 `.floe`와 별개의 프로파일 전용
바이너리에 저장한다. 파일이 없으면 parse/prepare 뒤 같은 디렉터리 임시 파일을
`0600`으로 쓰고 sync한 뒤 원자 rename하며, 있으면 mmap으로 읽어 소스 parse와
recursive bbox 계산을 생략한다. 스키마 버전, canonical source path, size,
나노초 mtime, 선택 cell, 전 파일 CRC32 checksum과 commit footer를 모두
검사한다. stale/corrupt/다른 셀은 자동 재사용하지 않으며 명시적인
`--profile-snapshot-refresh`로만 교체한다. snapshot은 planner 입력을 owned
record로 복원하므로 완전한 zero-copy 형식은 아니다. 큰 셀에서는 JSON의
`timing_s.snapshot_load`를 실제로 확인해 OASIS parse 대비 이득을 판정한다.
고정폭 레코드라 파일 크기는 선택 셀 record 수에 비례하므로 여유 있는 로컬
scratch를 명시해야 한다. 텍스트는 `build_cell_plan` 입력이 아니므로 snapshot에
넣지 않는다.

```sh
floe2 index chip.oas --jobs 16 --profile-cell-ci 32810 \
    --profile-jobs 8,12,16 --profile-repeat 2 \
    --profile-snapshot /fast-scratch/ltv_top_FEOL_dmy.floe-profile \
    > ltv_top_FEOL_dmy.profile.json
# 원본이 바뀌거나 스냅샷 스키마/선택 셀이 달라졌을 때만:
floe2 index chip.oas --jobs 16 --profile-cell-ci 32810 \
    --profile-snapshot /fast-scratch/ltv_top_FEOL_dmy.floe-profile \
    --profile-snapshot-refresh
```

## 1.5 파스 규약 (도형 변환)

- CIRCLE(record 27) = **내접 64각형 PolyRec**으로 파스 시 변환
  (0.11.46) — 축 꼭짓점 4개가 정확해 bbox는 원과 동일, 꼭짓점
  좌표는 리터럴 코사인 테이블(런타임 cos 금지 — 플랫폼별 1ulp
  차이가 빌드 바이트를 가르면 안 됨), 반지름이 작으면 연속 중복
  꼭짓점 접힘(r≥1이면 ≥4점), r=0은 지오메트리 없음. 기존 폴리곤
  페이지 경로를 그대로 타므로 OVM/OVP 포맷 범프 없음. 게이트:
  `circle_records_become_64gon_polys` 유닛 + klayout이 쓴 실제
  CIRCLE(round 제로길이 path) 자산의 G5 recount.
- TRAPEZOID/CTRAPEZOID(23~26)는 여전히 파스 에러(스코프 밖).

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
       쥐고 있어 지배 레이어의 P2가 항상 예산 0을 본다. **#76 통합
       실행 슬롯 예산**: 셀 플래너와 P1/P2/LOD 헬퍼가 `--jobs`개의
       슬롯을 공유한다. 순서 커밋 윈도 밖에서 기다리는 플래너는
       슬롯을 반납하고, 입장이 허용돼도 슬롯을 다시 얻은 뒤에만
       플래닝을 재개한다. **#76b 늦은 임대 흡수**: P2/LOD는 단계
       시작 시점의 한 번뿐인 슬롯 조회로 스레드 수를 고정하지 않는다.
       최대 16개의 전역 제한된 parked helper가 태스크가 남아 있을 때
       뒤늦게 반납된 슬롯을 빌려 합류하고 즉시 반환한다. 따라서 기본
       `plan-window=jobs×4`가 늦게 찬 실칩에서도 중간 위치 몬스터가
       유휴 CPU를 빌리며, parked helper는 실행 슬롯을 소유하지 않아
       CPU-active 플래닝 작업은 항상 `--jobs` 이하이다. 내부 fanout
       상한은 런타임 재탐지값이 아니라 명시 `--jobs`를 따른다.
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
         pbvh는 병합 완료 후 레이어당 1회 → 바이트 불변. jobs=1이면
         frontier/sharding 자체를 만들지 않는 완전 직렬 폴백;
         jobs>1이면 시작 슬롯이 0이어도 frontier를 만들어 늦은 슬롯이
         남은 태스크에 합류할 수 있다.
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
         엔트리가 OOM을 만들지 않는다. **기아 계측**: P2 frontier의
         유효 태스크가 끝날 때까지 helper가 하나도 합류하지 못하면
         `p2_tasks=... helpers=0` 표시 + 빌드 말미 집계 한 줄. #76b
         이후 이는 시작 순간 슬롯 0이 아니라 실행 중에도 빌릴 슬롯이
         끝내 생기지 않았다는 뜻이다. 윈도 대기 슬롯 임대 횟수도 빌드
         말미 `plan window lent ...`로 보고한다.
         남은 지렛대 = 프리픽스 파티션 병렬화(P2-ext, 미착수).
     - **LOD 변종 생성**: 후보 members≥256; 후보 ≥64(LOD_PAR_MIN)면
       셀 내부 스레드 팬아웃(≤16, 같은 공유 예산), 단계 시작 뒤 늦게
       생긴 슬롯도 태스크가 남아 있으면 합류, cand 순서 병합 → 바이트
       불변. `--no-lod`면 후보 목록 자체를 비움(전 페이지
       LOD_PAGE_NONE).
     - CellSink 프리인코딩.
   - 커미터(메인): 순서대로 `append_cell_sink` 리베이스, lod_page
     전역화, 텍스트/비트셋/cell 레코드 커밋. 윈도 채워지면 청크 인코드
     + 아레나 해제. 메모리 거버너: MemAvailable<4GB면 윈도 반감.
     page encode는 persistent planner와 별도의 최대 `jobs` scoped
     스레드를 사용하므로 청크 경계에서 OS thread 수가 잠시 약
     `2×jobs`로 보일 수 있다. 이는 cell-plan 병렬도와 다른 계측이며
     후속 고정 풀 통합 대상이다.
   - **동시성 계측**: 5초 heartbeat는 `active_cells`/`peak`, 완료 줄은
     `cell-plan peak`를 출력한다. 셀 로그가 순차로 보여도 실제 동시
     cell-plan 수는 이 값으로 판정한다.
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
