# SPEC: 검증 게이트 체계

진입점: `sh tools/validate_rust.sh` — valmini 픽스처를 생성/갱신하고
아래 전부를 순서대로 실행, 마지막 줄 `RUST VALIDATION: ALL OK` 필수.
러스트 유닛: `cd rust && cargo test --release`(워크스페이스; floe-vfs
41개 포함, `monster_cell_split_bench`는 `--ignored` 벤치).

## 0. 선택 실행(`--only`, 2026-09-17)

전체 배터리는 약 10분(jobdeck 2분, occupancy 1분 남짓, KLayout 오라클과
준비 단계가 나머지)이라 수정마다 돌리기엔 무겁다. 게이트 이름을 골라 돌린다.

```sh
sh tools/validate_rust.sh --list                 # 게이트 이름과 별칭
sh tools/validate_rust.sh --only occupancy,jobdeck
sh tools/validate_rust.sh --only planner         # 별칭
sh tools/validate_rust.sh --only quick path/to.oas
```

- 별칭: `quick`(vfs·render 유닛, occupancy, rust_renderer — 약 2분),
  `planner`(hier.rs 변경: unit_vfs, occupancy, jobdeck, rust_renderer,
  vfs_hier, vfs_lifecycle), `occ`, `render`(render-core/renderd 변경: unit_render,
  rust_renderer, representatives, render_goldens/speckle/frames, klayout), `indexer`(cli·인덱서
  변경), `python`(cli.py·cachepath·drc), `deck`. 모르는 이름은 exit 2.
- 준비 단계 재사용: release 빌드는 cargo의 신선도 검사에 맡기고, 레거시 .tiles
  오라클은 rust_* 게이트를 골랐을 때만(그리고 파이썬 인덱서가 바뀌었을 때만)
  다시 만들며, VFS 캐시(`<src>_rust.ice`)는 `--only`일 때 floe-index 바이너리와
  소스보다 새로우면 그대로 쓴다(`== VFS cache reused` 줄). `--only` 없는 전체
  배터리는 캐시를 매번 다시 만들어 형식 변경이 묵은 캐시 뒤에 숨지 못하게 한다.
- 유닛 게이트: `unit`(워크스페이스 debug `cargo test`), `unit_vfs`·`unit_render`
  (release `--lib`; release 프로필의 doctest는 LTO와 어긋나 제외).
- `gen_main01`(tools/validate_gen_main01.py, 약 10초; `python` 별칭에 포함): 합성 MAIN01 생성기.
  `--geometry legacy`가 2026-09-17의 파일과 바이트 동일(sha256 고정)한지, `--geometry chip`이
  `--jobs`와 무관하게 결정적이고 KLayout·floe-index가 읽으며 칩의 모양(가늘고 긴 배선, 여러
  크기대, 다이에 비해 작은 라이브러리 셀)인지.
- `sub_cut_box`(tools/validate_sub_cut_box.py, 약 15초; `planner`·`render` 별칭에 포함): 칩 형태
  합성 MAIN01의 via 레이어 하나, keep, 칩 전체 — 박스 없이는 빈 프레임, `FLOE_RUST_SUB_CUT_BOX=on`
  (0.12.182부터 기본 꺼짐)이면 박스가 찍힌 프레임이고 디코드 페이지 수는 같다. cull 요청, 전 레이어 keep(레이어 상한 초과), 컷 아래가
  없는 근접 keep 뷰는 킬 스위치와 바이트 동일. 리뷰 재현 레이아웃 4개(klayout.db로 생성):
  depth 제한 아래 도형에는 박스가 없고, 0.5 px·3 px 간격 배열과 점 목록의 드문 레이어는 멤버를
  직접 그린 프레임과 켜진 픽셀 수가 같으며, 64배치 중 하나의 레이어는 클러스터마다 노드 박스 하나.
  0.12.172: 두 레이어의 150 px 박스에서 위 레이어 채움을 없애도 아래 레이어가 그대로 켜지는지,
  같은 워커에서 frames 끔/켬/끔으로 계획된 경계 프레임이 0/64/0인지.
- `shape_cut`(tools/validate_shape_cut.py, 약 5초; `planner`·`render` 별칭에 포함): klayout.db로 만든
  작은 레이아웃(20 µm 사각형 + 0.4 µm 배열 + 0.2 µm 배선이 한 페이지, 배선만 있는 레이어 하나).
  작은 변 기준(`FLOE_RUST_SHAPE_CUT=min`, 0.12.213까지의 기본):
  전부 컷 이상인 근접 keep 뷰는 킬 스위치 `FLOE_RUST_SHAPE_CUT=off`와 바이트 동일하고 프레임이 컷을
  보고한다. 넓은 keep 뷰(배열 2.6 px, 배선 1.3 px, 컷 3 px)는 켜진 픽셀이 전부 큰 사각형 안이고
  (킬 스위치는 배열·배선을 그대로 켠다) 큰 사각형 자체는 같다. 배선만 있는 레이어는 빈 프레임이고
  플래너가 페이지를 자른다. cull과 컷 0 프레임은 킬 스위치와 바이트 동일.
  긴 변 기준(0.12.214부터 기본, `FLOE_RUST_SHAPE_CUT=max`와 같음; CUT_DENSITY_DESIGN §10.6): 넓은 뷰에서 큰 사각형은
  작은 변 기준과 같고 0.4 µm 배열은 0 px, 0.2 µm 선은 그려지며 선만 있는 페이지는 남는다. 변수 없는 워커의 넓은
  뷰 둘과 근접 뷰 하나가 `max`와 바이트 동일하고 `shape_cut_max=1`을 보고한다(`min`은 0). 같은 선 12개를 자식 셀(선 하나짜리 셀 12배치)로 둔 레이아웃(리뷰
  2026-09-25): 선이 정확히 1 × 350 px인 뷰(0.2 µm/px, 컷 3 px)에서 max·컷 0·도형 컷 세 경우 모두 평면 레이아웃과
  바이트 동일(max·컷 0은 4,200 px, 도형 컷은 0 px). 수정 전 max에서는 가는 자식 셀이 헤어라인 규칙에 잘려 0 px였다.
  (선 폭이 1 px 정수가 아닌 뷰에서는 평면 배열의 격자 순위와 배치의 월드 박스 해시가 여분 픽셀을 다른 선에 주므로
  바이트 비교를 하지 않는다.)
- `area_true`(tools/validate_area_true.py, 약 15초; `render` 별칭에 포함): klayout.db로 만든 세로
  막대 격자(0.1 µm/px 뷰, 격자마다 폭/간격 px 목록을 순환 — 정수·비정수 pitch, 폭이 섞인 이웃, 1px
  미만)를 0/¼/½/¾ px pan에서: 켜진 열이 막대가 건드리는 열 밖에 없다. 간격이 모두 2 px 이상인
  격자는 막대마다 floor(w) 또는 floor(w)+1 px이고 모든 pan에서 같은 폭이며 간격이 닫히지 않는다.
  1px 이상 격자는 네 pan 평균 켜진 열 비율이 덮임의 ±0.08, 폭·간격이 하나인 격자(KLayout이 반복
  레코드로 쓰는 배열 — 멤버 번호로 분산)는 ±0.025. 1px 미만 배열 격자는 켜진 비율이 덮임의
  0.85~1.15배. 킬 스위치 `FLOE_RUST_AREA_TRUE=off`는 1.2·1.5 px 간격을 닫고 1px 미만 격자를 전부 켠다.
  같은 뷰 두 번과 37×23 px 정수 pan의 겹친 영역이 픽셀까지 같다. 채움을 끈 다각형·사각형이 뷰의
  사방 밖으로 걸칠 때 뷰 안에 켜진 픽셀이 0(정수·소수 pan 4가지). 0.8×0.8 px 삼각형 900개가 같은
  bbox 사각형 900개의 0.35~0.65배만 켠다. M7-C 페이지 워시가 기본 꺼짐인지: 2 µm 셀 10×10 배치의
  넓은 뷰(페이지가 1 px 미만)에서 워시 0·도형이 그려지고, `FLOE_RUST_PAGE_WASH=on`이면 워시가 생긴다.
  M7 LOD 교체가 기본 꺼짐인지: `floe2 index --lod`로 만든 조밀한 레이아웃의 넓은 뷰에서 교체 0,
  `FLOE_RUST_LOD=on`이면 교체가 생긴다. 같은 세계 격자 행(1.5 px 막대 64개)을 한 배열·32개
  셀 두 번 배치·90° 회전 열 셀로 저장한 세 레이어가 정수·소수 pan에서 같은 픽셀을 켠다(세계 격자
  번호). 인덱서의 실제 페이지 분할(2026-09-23 검토): 64×6 격자(1.5×4.5 px 막대)와 같은 레이어의
  단일 사각형 70,000개를 페이지 목표 16 MiB(1페이지)·1 MiB(격자 Grid가 둘로 잘린 2페이지)로
  인덱싱해 두 축의 정수·소수 pan 5곳에서 같은 픽셀, 켜진 픽셀이 덮인 면적의 ±2 %(레코드별 번호일 때
  258 px 차이); 1 MiB 빌드 로그의 rep-split 줄이 그 Grid 조각 2개(한 줄 0·1멤버 0)를 센다. 2×64
  격자를 행 사이로 자르는 세로 긴 레이아웃(사각형 절반이 격자 아래, 절반이 위 — 중앙값 레코드인
  격자에 분할면이 놓인다): 1 MiB 빌드 로그가 2차원 Grid의 한 줄 조각 2개를 세고, 그 조각들이 같은
  레이아웃에 따로 저장한 두 행(레이어 12·13)과 pan 5곳에서 같은 픽셀을 켠다(863 px, 덮임 864의
  ±3 %); 자르지 않은 격자(16 MiB)와의 차이(300 px, 보장 범위 밖)는 출력만 한다. 추가 성김 진단
  `FLOE_RUST_WIDTH_C=2`: 1.5 px 배열의 네 pan 열 비율이 덮임 × (1 + 1/3)/1.5의 ±0.025, 0.5 px 배열의
  켜진 비율이 덮임의 0.55~0.8, c = 1이 켜지 않은 열은 켜지 않으며, 범위 밖 값 0.5는 c = 1과 픽셀까지 같다.
  생존 멤버 목록(0.12.207, CUT_DENSITY_DESIGN §10.7): 기본 뷰가 킬 스위치 `FLOE_RUST_SURVIVOR_LIST=off`와
  바이트 동일하고 검사 멤버가 더 적다(59,043 px, 924 → 671 — 0.1·0.25 px 배열이 목록으로 걸린다).
  배치 격자 진단(0.12.209, `FLOE_RUST_PLACE_LATTICE=on`, CUT_DENSITY_DESIGN §10.8): 0.2 px 막대와 0.3 px
  삼각형 120×3을 TOP의 도형(OASIS 반복)과 셀 배치 배열로 저장한 두 레이어가 정수·소수 pan에서 같은
  픽셀(2,380 px)을 켜고, 둘을 함께 켠 프레임이 목록 끔과 바이트 동일하며 방문 셀이 더 적다(132 대 361).
  프레임 결과의 `place_walks`에 그 배열의 걷기(`walked2`)가 오고, 목록 끔에서는 비어 있다(0.12.211).
  occupancy·jobdeck·representatives·sub_cut_box 게이트는 KLayout 규칙에 대한 비교라 이 킬 스위치를
  모든 워커에 고정한다(sub_cut_box: 상자는 표시용 점, 그 기준인 멤버 직접 그리기도 KLayout 규칙).
- `density_stack`(tools/validate_density_stack.py, 약 5초; `render` 별칭에 포함): 밀도 스택 진단
  (0.12.224, `FLOE_RUST_DENSITY_STACK=top`, CUT_DENSITY_DESIGN §10.10). klayout.db로 만든 0.1 µm/px
  레이아웃(1/0: 뷰 전체의 0.2 px 선, 최상위 2/0: 왼쪽 절반의 사각형과 그 안의 0.5 px 점, 3/0: 같은 사각형만,
  모두 기본 스페클)에서, 원본이 없는 오른쪽 절반은 켬·끔이 바이트 동일하다(하위 선 7,410 px). 끄면 사각형
  안이 사각형만일 때보다 더 켜지고(23,286 대 19,208 px — 구멍의 선과 점), 켜면 사각형만일 때와 픽셀까지 같다.
  켠 프레임은 `density_stack` 계측(lit·top·lower·covered·claimed)을 보고하고, 끈 프레임·`FLOE_RUST_AREA_TRUE=off`·
  `FLOE_RUST_WRITE_ONCE=off`는 계측 없이 변수 없는 프레임과 같다.
- `layer_decode`(tools/validate_layer_decode.py, 약 20초; `render` 별칭에 포함): 레이어 순서 디코드
  검증(docs/LAYER_DECODE_PROBE_PLAN.ko.md 1단계)의 `render_probe`. klayout.db로 만든 5레이어
  레이아웃(불투명 블록, 그 아래 성긴 배열, 가로지르는 헤어라인, 두 번 놓인 셀)에서 `mode=baseline`과
  `mode=ordered`의 프레임이 컷·줌 4단계·keep/cull·depth 0/full·frames·labels에서 바이트 동일하고,
  일반 `render`와도 같다. 타일 64/128/384 px와 래스터 워커 1/4/12에서도 같은 프레임이며, 두 모드가
  write-once로 건너뛴 타일·pass·항목 수가 일치한다. 프레임 응답은 `probe_frame`이고 published
  scene을 갱신하지 않는다(probe 뒤의 snap이 그 전 렌더의 답을 그대로 준다). 블록 크기(`block=`,
  작업자가 한 타일에 연달아 그리는 pass 수) 2·4·1000도 같은 프레임이다. `mode=occlusion`은
  `mode=occlusion`(위 레이어가 덮은 페이지를 읽지 않는다)도 같은 프레임이고, 불투명한 위 레이어
  아래의 페이지를 실제로 읽지 않는다. write-once 마스크를 끄면 occlusion은 오류이고(가림을 증명할
  수 없다) 그 뒤에도 워커가 정상 동작한다.
- `bench_layer_decode.py`는 게이트가 아니라 측정 도구다(docs/LAYER_DECODE_PROBE_PLAN.ko.md §9).
  폐쇄망에서 손으로 옮겨 적는 경우를 위해 마지막에 `== type this ==` 블록(한 depth당 4줄)만 찍고,
  값이 없는 열은 머리글로 접는다.
- `write_once`(tools/validate_write_once.py, 약 20초; `render` 별칭에 포함): write-once 타일
  (F2R-28)의 프레임 18개가 킬 스위치 `FLOE_RUST_WRITE_ONCE=off`와 바이트 동일한지(area-true와
  KLayout 규칙 각각, 합 36개), KLayout 규칙의 밀집 뷰에서 타일이 차고 paint가 줄어드는지(area-true는
  1px 미만 도형을 면적만큼 켜서 이 칩의 타일이 차지 않는다), 킬 스위치가 write-once 작업을 전혀
  하지 않는지.
- `fit_budget`(tools/validate_fit_budget.py, 약 25초; `planner`·`render` 별칭에 포함): 합성
  MAIN01 칩의 keep + cut 1 px 광역뷰가 48 MB 예산에서 오류 대신 낮춘 밀도(`fit_thin` > 0)로
  그려지고 **빈 프레임이 아닌지**, `FLOE_RUST_FIT_THIN=off`는 컷을 올리고(`fit_pct` > 100)
  `FLOE_RUST_FIT_BUDGET=off`는 종전 오류인지, 예산 안의 프레임은 픽셀이 바뀌지 않는지.
- `representatives`(tools/validate_representatives.py, 약 10초; `render`·`indexer`
  별칭에 포함): design.ovr 추가 생성이 캐시를 보존하는지, depth 0 제외·kill switch·
  손상 파일 폴백, 그리고 결합 인덱스 실행에서 OVR 생성이 실패해도(`--kill-at
  representatives-fail`, 게이트 전용 모의 실패) design.ovm·마커가 완성되고 캐시가
  열리는지, `--representatives-only`의 같은 실패는 exit 1이며 캐시를 건드리지 않는지.
  OVR2의 형상 길이·회전·실제 경계와 revision 3의 두 군집 병합/빈 공간 보존,
  direct 대비 픽셀 오차·확대 정제·조회 묶음 제한 시 최종 픽셀 및 2회 래스터,
  비닝/비비닝 일치도 검사한다. 기하 오차 상한·공간/depth 프루닝·블록 checksum·
  취소는 `unit_vfs`, 스타일별 hairline 행 구간 병합은 `unit_render`가 담당한다.
- 마지막 줄은 같은 `RUST VALIDATION: ALL OK`이고 `--only`면 돌린 게이트 목록이
  붙는다. 커밋 규칙: 반복 커밋은 바꾼 영역의 별칭 통과, 실칩용 푸시는 전체 배터리.

## 1. 픽스처

- **valmini**: `tools/gen_valmini.py` — 작은 결정적 자산. 파이썬
  .tiles 캐시(구명 .ice, 2026-08-13 개명)가 메타 패리티 오라클
  (파이썬 인덱서가 바뀌면 재생성 필요 — mtime 불일치 함정 주의:
  캐시 디렉토리 삭제 후 재실행).
- **sample9**: `tools/gen_sample9.py` — 145MB depth-9, seed 42, 티어
  테이블. 성능/실측용.
- **frametest**: `tools/gen_frametest.py` — 프레임 톤/스택 검증용 소형.
- **thintest**: `tools/gen_thintest.py` — rev 45 격자 관찰용(성긴/밀집
  행·2D·클러스터·수직 열·SHORTBAR 소멸 대조·LONGBAR 프레임 행·L30
  지오메트리 행). 독스트링에 기준 plan 수치 내장.
- **drctest**: `tools/gen_drcdb.py` — DRC 부하 테스트용 합성 .db
  (기본 ~95MB, 체크 1000개, 체크당 0..1000 에러, rect/엣지쌍/단일
  엣지/계단 폴리곤 믹스 + Waiver 줄 + `*_RDBS` 꼬리 4종). 실측:
  인덱싱 0.34s, pack 오픈 수십 ms vs ASCII 전체 파스 2.7s.

## 2. 게이트 카탈로그

| 게이트 | 스크립트 | 고정하는 계약 |
|---|---|---|
| scan/tile/depth/meta XOR | validate_rust_scan.py 등 | 러스트 vs klayout 밴드 타일 완전 일치 |
| vfs 오픈 검증 | `tools/validate_vfs.py` | ovm v7 구조(PAGE_LEN 104, 텍스트 수), **frontier 스키마**(keep/px_per_um/cut_px/5원소 행/다이 내부/depth0 비지 않음) |
| 렌더 6뷰 | validate_vfs_render | hier 델타 → klayout 렌더 XOR |
| H1~H5 | validate_vfs_hier | 실데몬 hier: 프로브 cut=0 XOR 일치 등 |
| L1~L9 | `tools/validate_vfs_lifecycle.py` | 세션 수명주기: L1 팬 루프, L2 스테일 드롭/재전송, L3 부분적용 폴트 ①~④+bad-top 복구, L4 제로 예산 축출, L5 names 보존, L7 LOD 변종 사이클/킬스위치, L8 layers=none+프레임 컷/밴드, **L9 미니맵 굽기 == vfsd mode=frontier 재생(박스 단위)** |
| S1~S7 | validate_vfs_split | rep-split: multiset 보존·경계 소유·skew·oversize 비오염·플로어; S6(#60 P1) = 두 Pts-플러드 레이어 픽스처에서 split 팬아웃 관측(`--slow-cell-s 0` slow-cell 로그의 `split x/Nt`≥2·per-layer top 리스트) + 병렬 피크 RSS ≤ 직렬×1.5+512MB(**per-run 독립 측정**: 신선한 래퍼 프로세스의 RUSAGE_CHILDREN; 1CPU(affinity+cgroup quota 최솟값)/MemAvailable<4GB 호스트는 스레드 검사 스킵); jobs 1↔4 바이트 동일(S4)이 arena shard 격리를 함께 고정; S7(#60 P2/#76) = commit-head MONSTER 뒤의 소형 셀들이 plan window를 채우는 p2floor에서 대기 플래너의 `plan window lent` 관측, MONSTER의 `helpers=0` 부재·`p2_tasks`≥2·스레드≥2 확인 + **jobs 1/4/16 바이트 동일** + validate_vfs.py 전 페이지 recount + 병렬 RSS(샤딩 복사 포함) ≤ 직렬×1.5+512MB. 러스트 유닛: `plan_window_slot_lending_requires_reacquire`(대기 플래너 임대→helper 점유 중 재개 금지→반환 뒤 재획득→lease 균형), `lod_uses_lent_plan_slots_and_returns_them`(64+ 후보 LOD가 시작 시 임대 슬롯으로 팬아웃하고 전량 반환), `p2_borrows_slot_lent_after_frontier_start`·`lod_borrows_slot_lent_after_phase_start`(#76b: 단계 시작 뒤 늦은 임대가 실제 작업에 합류·반환), `p2_forced_frontier_is_byte_neutral`(강제 frontier·threads=1 — oversize→left→right 순서·음수 skew Grid·coincident pile·페이지 페이로드 바이트 대조), `p2_mode_engages_and_matches_serial`(jobs=1 직렬 기준 ↔ jobs>1 P2), `p2_shard_limit_serial_fallback`(shard 한도 초과 = frontier 유지·복사 0·직렬 실행·**lease 즉시 반환**, 바이트 동일), 소형 자산 가드는 스위트가 valmini 빌드 로그에서 `p2_tasks=`/`split /≥2t` 부재를 직접 검사 |
| X1~X6 | validate_vfs_text | v5 텍스트/라벨/declutter |
| 마커 | validate_vfs_marker | --kill-at 4지점 + 재빌드 |
| render-speckle | validate_render_speckle | 공통 위상(전 레이어 구멍 공유), 가시성, 불투명 겹침, 커버리지 합성 포함관계 |
| render-frames | `tools/validate_render_frames.py` | 페인트 순서 회색<디자인<흰, 1px 외곽, 흰-위/회-아래 |
| PX1~PX5 | `tools/validate_render_goldens.py` | 러스트 렌더러 픽셀 정책 골든(klayout 오라클, 커밋 안 함 — 버전 종속 자동 재베이크): PX1 반픽셀·¼픽셀·음수 원점·원점 교차 반올림, PX2 수평/수직/45°/임의 기울기 엣지, PX3 1~8px 선폭 H/V/45°, PX4 concave(L/U/plus/comb/예각 노치), PX5 PATH flush/square/round/비대칭 ext+90/45/135° 꺾임 — 각 정렬+반픽셀 뷰(13뷰). 정책 P-a(diff는 1px 밴드 안만)/P-b(성분 소멸 금지)/P-c(면적 드리프트 ≤0.75×경계픽셀). 자기검사 = 재렌더 결정성 + 판별력(shift1 통과·shift2/dilate/소멸 실패); 외부 렌더러는 `--candidate DIR`로 대조 |
| D1~D7 | `tools/validate_drc_ice.py` | DRC pack `.<db>.tray`(v1 오프셋 사이드카는 2026-08-19 폐기; 2026-09-16 개명 — 기본 이름·`<db>.ice` 자동 개명·db 이름 기준 사이드카·`FLOE_CACHE_MIGRATE=off`): D1 pack 경유 == ASCII 파스(적대 픽스처 — 결과0 체크·중복 체크명·카운트줄 없음·미지 레코드·절단·CRLF·Waiver Criteria·`*_RDBS` 빈 것 드롭/에러 보유 시 유지·`__RVE_ERROR_TAG2__` 중간 배치 = 레코드 포함 드롭+전역 번호 무공백·전역 파일순 번호), D2 디스패치(신선 pack 자동 선택 / 스테일 pack·폐기 v1 = ASCII 폴백, v1 직접 오픈 = 거부, corrupt 3종 = 12B 스텁·중간 절단·푸터 오프셋 오염 → 전부 ValueError+사이드 폴백), D3 줄 단위 dedup+lazy 슬라이싱, D4 gen_drcdb 자산 왕복 == ASCII, D5 pack 바이트 --jobs 무관(2KB 픽스처 5분할 = 체크 중간 이음새 강제), D5b 강제 스트리밍 인코더(FLOE_DRC_QBOX_RESIDENT=0) 바이트 동일, D6 query_rect == 브루트포스 bbox 스캔, D6b waived= 필터 = 쿼리 내부 cap 이전 적용(소형 cap에서 유일 waived 에러 발견·비필터 결과 불변·[wcount]=0 스킵), D7 status 바이트 제자리 set/get·재오픈 지속·이웃 무오염·[wcount] 동기·청크 카운트 캐시 토글 후 동기 |
| R1~R4 | `tools/validate_svrf.py` | SVRF 서브셋 파서(.rules.json): R1 전처리(INCLUDE 상대경로 병합·순환 경고·#IFDEF/#ELSE -D 분기·#DEFINE 값 치환→제약·VARIABLE 수치 해석·--scan 양분기), R2 derivation 그래프(다이아몬드 폐쇄→전 원천 LAYER+MAP dt·순환 종료·미정의→unresolved·연산자 비누출), R3 체크 추출(다중 @ 결합·이중 한계 2제약·붙은 op·`ABUT<90` 비제약·측정 우변 할당문·미지 문장 카운트·따옴표 체크명·DMACRO 통스킵), R4 gen_drcdb --svrf 엔드투엔드(db 체크명 100% 매칭·제약값 생성식 일치·전 체크 gds 도달·-D SYNTH_EXTRA 정확히 1룰 추가·JSON 왕복) |

## 3. 러스트 유닛 (핵심만)

hier.rs: `hairline_min_side_cut`(rev 41), `frames_split_into_size_bands`
(rev 42 4밴드), `boundary_frames_take_the_size_cut`/`depth_boundary_
frames_keep_rep`/`dense_frames_stay_per_member…`(rev 34/39),
`thin_frames_sample_on_lattice`/`thin_singles_and_pts_dedupe_per_bin`
(rev 45: 닫힌형 수치·모서리·강등·cut0·격자off 폴백·양변 소멸),
`frontier_boxes_expand_ws_world_space`(rev 46), `brute` 오라클(페이지
과소선택 금지 — 술어는 hairline 0.5를 미러: 규칙 변경 시 동기화),
`deterministic_plans`. ovm: `cell_sink_append_is_byte_identical`,
`split_max_min_detects_hairline_pages`. cli: 분할/파이프라인 13개.

## 4. 결정성 게이트

- 인덱서: --jobs 값과 무관하게 design.ovm sha256 동일(스위트).
- 플래너: 동일 요청 = 동일 플랜(HashSet 순회에 의존 금지 — thin_bins는
  insert 전용, frontier keep은 BTreeMap+strict-greater).
- 프런티어: 인덱싱 굽기 vs 데몬 재생 박스 일치(L9).

## 5. 게이트 작성 규칙

- 새 파이썬 게이트는 `tools/validate_<영역>.py`, 자산 생성기는
  `tools/gen_<이름>.py`.
- klayout Region 비교는 소스 레이아웃 `_destroy()` **이전**에(파괴된
  레이아웃의 region은 조용히 빈 값 — L7 함정 메모).
- 테스트 블록을 스크립트로 지울 땐 마커 범위를 좁게(rev 37 테스트
  유실 사고).
- 실측 하니스에서 RenderWorker는 spawn — `if __name__ == "__main__"`
  가드 필수, 뷰 좌표는 dbu, 경계 프레임은 depth 0에서 나온다는 것
  (r==0 확장 주체가 부모) 주의.
