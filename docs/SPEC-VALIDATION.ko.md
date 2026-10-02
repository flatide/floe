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
  격자는 간격이 닫히지 않으며, 2 px 이상 막대는 **변의 선 사이 블록(선 포함, floor(x + ½))과 정확히
  일치**(변 정확, 0.12.236), 2 px 미만 막대는 floor(w) 또는 floor(w)+1 px이고 모든 pan에서 같은 폭. 막대가
  모두 2 px 이상인 격자는 pan마다 켜진 열 비율이 기하가 주는 값과 정확히 같고(정수 pitch면 모든 멤버가
  같은 폭 — KLayout과 같은 양자화), 그 밖의 1px 이상 격자는 네 pan 평균 켜진 열 비율이 기대값(넓은 막대는
  블록, 가는 막대는 덮임)의 ±0.08, 폭·간격이 하나인 격자(KLayout이 반복 레코드로 쓰는 배열 — 멤버 번호로
  분산)는 ±0.025. 소수 좌표에서 맞닿은 사각형 둘과 그 좌표에 변을 둔 다각형(레이어 16)은 네 pan에서 한
  열을 공유하는 한 블록으로 그려지고, 채움을 끄면 선 세 열(A의 왼쪽, 공유 변, B의 오른쪽)만 켜진다. 1px 미만 배열 격자는 켜진 비율이 덮임의
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
- `density_stack`(tools/validate_density_stack.py, 약 10초; `render` 별칭에 포함): 밀도 스택 진단
  (0.12.226, `FLOE_RUST_DENSITY_STACK=top`, CUT_DENSITY_DESIGN §10.10 — 컷 아래 도형을 2패스로 빈 공간에).
  klayout.db로 만든 0.1 µm/px 400×200 레이아웃(1/0: 뷰 전체의 0.15 µm(1.5 px) 사각형 격자와 오른쪽 위 사분면의
  사각형, 최상위 4/0: 왼쪽 절반의 사각형과 그 안·1/0 사각형 위의 1.5 px 사각형들, 3/0: 왼쪽 사각형만, 모두
  기본 스페클, 컷 3 px)에서: 끄면 1.5 px 사각형이 하나도 없고(컷), 켜면 오른쪽 아래 사분면에 1/0의 사각형이
  1/0만 컷 없이 그린 프레임과 픽셀까지 같게 나온다. 왼쪽 사각형 안은 사각형만일 때와 같고(구멍의 하위 사각형도
  자기 사각형도 없음), 오른쪽 위는 1/0 사각형 위에 최상위의 사각형이 최상위만 컷 없이 그린 픽셀과 같은 위치·색으로
  올라오며 나머지는 끈 프레임 그대로다. 1/0만 켜면 1/0의 컷 없는 프레임과 바이트 동일하고(2패스는 보이는 레이어만,
  0.12.229), 1/0+2/0을 켜면 오른쪽 위에 4/0의 사각형이 없다. 켠 프레임은 `density_stack`과 `density_pages`(계획 > 0,
  디코드 > 0) 계측을 보고하고, 끈 프레임·`FLOE_RUST_AREA_TRUE=off`·`FLOE_RUST_WRITE_ONCE=off`는 계측 없이 변수 없는 프레임과 같다.
  0.12.233: 뷰어의 여백 프레임(`bg`, 각 축 2배)이 뷰를 뷰포트 프레임과 픽셀까지 같게 그린다(2패스가 자기 예약으로
  계획하고 그 맞춤을 배율·면마다 기억; `over_budget` 0).
  0.12.247 컷 아래 점(`FLOE_RUST_DENSITY_DOTS=on`, CUT_DENSITY_DESIGN §10.12): 셀 DOT(0.15 µm = 1.5 px)을 6 px 간격
  10×10 배열, 맞닿은 40×40 배열, 하나로 놓은 둘째 레이아웃에서 — 성긴 배열은 정확히 100픽셀(각각 멤버 중심 2 px
  안), 하나는 1픽셀, 맞닿은 배열은 4×4 블록마다 min(8, 중심이 그 안인 멤버 수)의 합(게이트가 멤버 위치로 셈)만큼,
  그 밖은 켜지지 않는다; `density_dots` 항목 = 블록 수, 초과 0; 변수 없이 스택만이면 DOT 안으로 걸어 그리고
  `density_dots`가 없다; 여백 프레임의 가운데 = 뷰포트 프레임. 0.12.248(2단계): TOP 자신의 0.05 µm(0.5 px) 사각형
  20×10(3 px 간격)이 점 모드(페이지 하한 0)에서 컷 없는 프레임과 같은 픽셀로 그려지고, 스택만(1 px 하한)은 그리지 않는다.
  0.12.249(3단계): 점 모드 프레임은 refining 라운드(1패스만, 2/0 원본까지 스택 없는 프레임과 바이트 동일) 뒤에 최종
  프레임(한 라운드 `FLOE_RUST_DENSITY_PROGRESSIVE=off`와 바이트 동일)이 오고, 여백 프레임은 한 번만 온다.
  0.12.250: 첫 라운드에서 다른 뷰로 확대하면 새 세대가 답하고(그 최종 = 자기 한 라운드 프레임) 이전 세대는 그 뒤 최종
  프레임을 내지 않는다(계획 취소 `HierOpts::stop`). 렌더 코어 단위
  `a_cancellation_during_the_density_collect_ends_the_frame_not_the_thread`: 2패스 bin 수집 중 취소가 프레임을
  `render cancelled`로 끝낸다(이전엔 워커가 barrier에 묶여 영영 돌아오지 않았다; 감시 30 s). 뷰어 계약
  `test_a_dropped_foreground_render_clears_the_pending_state`.
  0.12.251(렌더 중 입력, SPEC-VIEWER §7): Esc가 진행 중 렌더를 취소(`cancel before_gen`)하고 체인은 다음 Esc,
  렌더 중 휠 줌이 `_zoom_at`에 닿음, 이전 세대의 error/cancelled는 대기를 풀지 않음; 어댑터 `cancel(7)` →
  `cancel before_gen=7`, `cancelled gen=6 phase=render` → 결과·job 제거, `before_gen` ack는 결과 아님.
  0.12.255(SPEC-VIEWER §8c): `test_under_a_view_root_the_depth_counts_to_the_roots_height` — 루트(높이 2)에서
  라벨 `*/2`·`1/2`, 단계 이동이 [0, 2]로 제한, 전체에서 한 단계 = 1, top으로 돌아오면 `7/16`, 루트 변경이 라벨 갱신.
  0.12.253: 하한 `FLOE_RUST_DENSITY_FLOOR_PX` — 0.25 px는 0.5 px 미세 사각형을 그리고 0.6 px는 뺀다,
  `density_floor` = 0 / 0.25 / 0.59; 어댑터 계약 `density_floor=0.250` → 0.25, 없으면 None.
  0.12.262(2패스 계획을 스레드로, 선택): 게이트 점 절 — `FLOE_RUST_DENSITY_PLAN_THREADS=2`의 프레임이 기본과 바이트 동일하고
  `density_plan2` threads 2; 어댑터 계약 `density_plan2`(…/threads/reads/items).
  0.12.265(2패스의 결정은 프레임마다, 사용자 2026-10-01): `uneven` 레이아웃(400 µm, 구석 1/4에 서로 다른 셀 4개·먼 절반에
  200개, 각 1~1.3 µm 사각형 100개 = 0.667 µm/px에서 1.5~2 px, medium 컷 아래·하한 위)과 2패스 예약 1 MB
  (`FLOE_RUST_DENSITY_BUDGET_MB=1`)에서 전체를 먼저 그리면 2패스가 솎이고(lit이 128 MB 워커보다 적다) 그 뒤 같은
  배율의 구석이 새 워커의 구석과 바이트까지 같다(0.12.264: 1,137 px 대신 858 px).
  0.12.266(1패스가 든 페이지는 2패스 예산에서 0): `held` 레이아웃을 쓴다.
  - 구성: BIG 셀(2.1~2.4 µm 사각형 3,000개, 위치·크기 난수 — 반복으로 묶이지 않게)과 그 옆의 서로 다른 셀 30개(각
    1~1.3 µm 사각형 100개).
  - 0.667 µm/px에서 BIG은 1패스의 것이고 셀들은 2패스의 것이다. 둘이 합쳐서만 1 MB를 넘는다(한 쪽이 혼자 넘으면
    맞춤이 포기하고 전부를 계획한다).
  - 확인: 2패스 예약 1 MB의 프레임이 128 MB의 프레임과 바이트까지 같고, `density_plan2`의 `probes_over`·`thinned`와
    `over_budget`가 0이다.
  - 0.12.265에서는 21페이지로 솎여 5,990 px였다(새 빌드는 31페이지, 8,955 px).
  - 어댑터 계약: `density_plan2` 13개 값(…/probes_over/thinned). 상태줄 테스트: `pass 2 over budget: floor probe,
    thinned, 3 pages left out`.
  0.12.267(솎아야 하는 2패스도 스레드로): `uneven` 레이아웃·예약 1 MB의 전체 뷰를 `FLOE_RUST_DENSITY_PLAN_THREADS=2`로 계획하면
  한 스레드의 솎인 프레임과 바이트까지 같고, `density_plan2`가 threads 2·thinned 1이다(0.12.266은 threads 1).
  0.12.268(2패스 스레드 기본 min(코어, 4), 점 블록 격자):
  - 게이트 점 절:
    - 기본 스레드·1 스레드·2 스레드의 프레임이 같고, `FLOE_RUST_DENSITY_DOT_GRID=off`(해시맵)의 프레임과도 같다.
    - `density_plan2`의 출처 합(청크 멤버 제외)이 항목 수와 같다.
    - `map_updates`는 격자에서 0, 해시맵에서 양수다.
  - `uneven` 기록 절: 기준 워커를 한 계획(`FLOE_RUST_DENSITY_PLAN_THREADS=1`)으로 고정했다. 기본 스레드도 솎인
    프레임이 바이트까지 같고, threads = min(4, 코어, 영역), thinned 1이다.
  - 단위 `the_dots_grid_counts_and_orders_the_blocks_as_the_hash_map_does`:
    - 픽스처: LEAF·LEAF@2, 한 블록씩 든 청크 넷, 흩어진 리스트, 배열, 영역 셋으로 나눈 경우.
    - 격자의 결과가 해시맵과 같다. 청크 4개(멤버 1,024)를 한 번에 세고, 해시맵 갱신은 격자 0 / 해시맵 양수다.
      항목 수는 출처 합과 같다.
  - 단위 `a_dot_grid_and_the_hash_map_beyond_it_drain_in_key_order`: 무작위 갱신 2,000개가 정렬 맵과 같다.
  - 어댑터 계약: `density_plan2` 22개 값. 상태줄 테스트: 로그 줄의 출처(요약 줄에는 없음).
  0.12.269(점 리스트는 스레드마다 자기 영역의 멤버만):
  - 게이트 `density_stack` `lists_checks`:
    - 픽스처: 0.1 µm VIA를 300 µm 정사각형에 무작위로 4,000개(KLayout 압축 10, 점 리스트로 색인), 1000×1000 px(타일 9개).
    - 1/2/4 스레드와 `FLOE_RUST_DENSITY_DOT_BOXES=off`의 프레임이 같다.
    - 센 멤버: 4 스레드는 1 스레드의 1.25배 이하, 끈 경우는 1.5배 이상(4,000 / 4,201 / 4,204 / 11,822).
  - 단위 `a_point_list_is_walked_by_each_box_and_a_block_no_box_holds_whole_is_left_out`.
  - 상태줄 테스트: 로그 줄의 `list chunks C of M members`가 청크 바로 뒤에 온다(0.12.268은 배열 멤버 뒤였다).
  0.12.270(2패스 예약 = 1패스가 남긴 만큼):
  - 게이트 `density_stack` `left_checks`:
    - 픽스처: TOP이 0.05~0.3 µm 상자 6만 개를 직접 가진 300 µm 레이아웃(레코드 38,554, 추정 7.7 MB). 예산 32 MB(고정
      예약 4 MB), 깊이 0, 하한 0.
    - 2패스가 컷 없는 프레임과 같은 픽셀을 켠다(19,982 px). 예약은 32 MB이고 탐침 초과는 없다.
    - `FLOE_RUST_DENSITY_RESERVE_LEFT=off`는 아무것도 켜지 않고 하한 탐침이 초과한다.
  - `history_checks`·`held_checks`는 예약 1 MB가 요점이라 고정 예약(`FLOE_RUST_DENSITY_RESERVE_LEFT=off`)으로 고정했다.
  - 어댑터 계약: `density_plan2` 23개 값(…/reserve_mb). 상태줄 테스트: 로그 줄의 `reserve R MB`.
  0.12.261(블록 4 px, 한 번 걷기는 선택, 하한 1 px, 페이지 점 끔): 게이트 점 절의 기본은 4 px·퍼뜨림·맞춤·하한 1 px —
  성긴 배열 100 px가 각 멤버에서 1.5 px 안, 맞닿은 배열 = Σ min(8, 4×4 블록의 멤버), 항목 = 성긴 배열 100 + 1 + 맞닿은
  블록, TOP의 0.5 px 점은 그리지도 점으로 서지도 않음, `density_floor` 1, `density_plan2` 탐침 0·맞춤 1; 하한 0·0.25 px는
  점을 컷 없는 프레임과 같게(하한 0은 탐침 1), 0.6 px는 뺌; 한 번 걷기(`FLOE_RUST_DENSITY_ONE_WALK=on`)는 탐침 0·맞춤 1에
  점 없음, 페이지 점(`FLOE_RUST_DENSITY_PAGE_DOTS=on`, 하한 0)과 함께면 TOP의 점 50; 이전 규칙
  (`SPREAD=off FLOOR_PX=0 PAGE_DOTS=on`)은 예전 기대. 단위 `the_dots_one_walk_takes_pass_1s_pages_and_dots_the_pages_under_the_cut`에
  페이지 점 끔(작은 페이지 0점, LEAF 1점). 실제(컷 없이 그린 프레임)와의 켜진 비율 대조는 CUT_DENSITY_DESIGN §10.12.
  0.12.260(16 px 넘는 블록): 단위 `a_block_past_16_px_counts_what_its_items_hold`(상한 2,048·32,768, 64 px 블록에서 성긴
  배열 900점·맞닿은 48 px 배열은 한 항목 1,152, 7 px 정사각형 모서리의 LEAF 셋은 마스크 유무·블록 8·64 px 모두 3점),
  렌더 코어 `a_large_counted_dot_item_lights_exactly_its_count`(30×30 px 상자 — 스택 배열 18×18을 넘는 900픽셀 — 에 200점·1점이
  정확히, 타일·워커·bin 무관).
  0.12.259(한 번 걷기, CUT_DENSITY_DESIGN §10.12): 게이트 점 절 — 기본(한 번 걷기)에서 TOP의 0.5 px 점 200개 페이지는
  정확히 50점(면적이 켜는 양)이고 하한 0.25·0.6 px에도 같으며 `density_floor` 0 / 0.25 / 0.59, `density_plan2`는 탐침 0·
  맞춤 1패스; 탐침과 맞춤(`FLOE_RUST_DENSITY_ONE_WALK=off`)은 예전 기대(점이 컷 없는 프레임과 같고 0.25는 그림·0.6은 뺌);
  이전 규칙(4 px, 퍼뜨림 끔, 한 번 걷기 끔)은 예전 기대 그대로. 단위
  `the_dots_one_walk_takes_pass_1s_pages_and_dots_the_pages_under_the_cut`(큰 페이지만 계획, 레코드 컷 0·75, 작은 페이지는
  도형 수만큼의 점, LEAF 1점, 예산이 있어도 맞춤 없음, 몫 모드는 작은 페이지를 계획), `a_wash_the_walk_pushes_itself_carries_no_dot_count`
  (M7-C 페이지 wash는 개수 0, 블록 항목은 제 개수 — 개수와 wash가 나란함); 어댑터 계약 `density_plan2`.
  0.12.258(타일 걷기의 인스턴스 색인, RUST_RENDERER.md): 렌더 코어 단위 `a_tile_walks_the_instances_that_meet_it` —
  인스턴스 303개(무작위 자리·방향의 단일 배치 300, 30×2 격자, 점 목록, 빈 자식)의 셀을 색인 켬·끔으로 그린 프레임과
  사각형 칠 수가 같다(타일 8·16·128, 워커 1~3, bin 켬·끔); 8 px 타일에서 색인은 검사한 멤버가 4분의 1 미만.
  `the_instance_index_keeps_every_instance_with_a_member_in_view` — 무작위 뷰 400개와 가장자리·바깥·빈 뷰에서 질의 답은
  오름차순이고, 멤버 상자가 뷰에 닿는 인스턴스를 하나도 빠뜨리지 않으며(멤버를 직접 나열해 대조), 빈 자식은 묻지 않는다.
  0.12.256(점 모드의 순수 비용): 새 검사 없음 — 기존 단위·게이트가 그대로 통과하고, 7개 표준 뷰(합성 1/10 칩)의 프레임
  해시가 0.12.255와 같다(CUT_DENSITY_DESIGN §10.12).
  0.12.257(블록 8 px, 퍼뜨림): 게이트 점 절이 둘이 된다 — 기본(8 px, spread)은 성긴 배열 정확히 100 px(각 점이 멤버
  중심에서 한 블록 안), 외톨이 1, 맞닿은 배열 = Σ min(32, 8×8 블록에 중심을 둔 멤버), `density_block` 8, 항목 = 성긴
  배열이 걸친 블록 + 1 + 맞닿은 블록; 이전 규칙(`FLOE_RUST_DENSITY_BLOCK_PX=4 FLOE_RUST_DENSITY_SPREAD=off`)은 예전
  기대(100 px가 멤버에서 2 px 안, Σ min(8, 4×4 블록), 항목 100 + 1 + 블록, `density_block` 4) 그대로. 하한·점진·확대
  검사는 기본에서. 단위 `the_dots_blocks_spread_what_they_count_and_count_what_is_there`(8 px: 상한 32, 성긴 배열
  900점, 맞닿은 36블록 모두 32, 2×2 배열은 상자의 8이 아니라 4, 영역을 나눠도 같은 항목·개수, spread 끔은 개수 없이
  상자 면적, 4 px보다 항목이 적음; 마스크 없는 노드의 세 LEAF는 3점, 끄면 상자의 24점),
  `a_spread_dot_item_lights_its_count_over_its_box`(8×8 px 상자에 5점, 16×16 px에 100점이 상자의 네 사분면에 퍼짐,
  최상위 면은 하위 원본 위에 10점, 개수 없이는 상자 절반 32; 타일·워커·bin 무관); 어댑터 계약 `density_block=8` →
  8.0, 없으면 None.
  0.12.252: `test_a_settled_frame_of_this_view_does_not_render_again` — margin 끔에서 `_covered`가 거부하는
  뷰포트 프레임(한쪽 2 px 스냅 여유)이 착지해도 다시 렌더하지 않고 여백만, 뷰가 떠났으면 redraw, 디바운스·드래그
  중이면 아무것도 안 함(0.12.251의 무한 재렌더 회귀).
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
- `fit_budget`(tools/validate_fit_budget.py; 0.12.231: 배율별 맞춤 고정 — 가운데(48 MB에서 솎임) → 같은 배율의
  전체 칩(각 축 2배, 여백) → 가운데 순서로 그려 마지막 프레임이 기억된 결정을 적용하고(`fit_fixed`) 여백의
  가운데와 픽셀까지 같음; 0.12.232: 첫 가운데가 여백 범위로 결정해 여백이 그 결정을 **적용**하고(`fit_redecided` 0)
  첫 가운데도 여백의 가운데와 같음, 픽셀 일부만큼 옮긴 가운데가 기억을 공유, 예산 안의 근접뷰도 결정(everything)을
  지님(`fit_fixed` 1, `fit_thin` 0); 구석 1/4에 서로 다른 셀 4개, 먼 절반에 200개인 전용 레이아웃(`layout_uneven`,
  예산 1 MB)에서 구석이 everything으로 결정한 뒤 전체를 여백(`bg`)으로 청하면 `dropped`(reason fit)로 떨어지고 구석은
  같은 그림, 전체를 뷰포트로 청하면 다시 결정(`fit_redecided` 1); 0.12.265: 그 뒤 같은 배율의 구석이 처음 구석과 바이트까지
  같고(`fit_thin` 0, 다시 결정 아님 — 예산이 통째로 담는 프레임은 결정과 무관; 0.12.264는 솎인 전체 프레임을 잘라
  썼다), 전체를 다시 청하면 그 결정 그대로, 약 35초; `planner`·`render` 별칭에 포함): 합성
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
| C1~C7 | `tools/validate_cell_tree.py` (`cell_tree`, `render`·`indexer` 별칭, 약 20초) | 셀 트리(SPEC-VIEWER §8c, design.ovh): 계층 픽스처(블록 3배치 = 평·90°·미러, 블록 안 3×2 격자, 탑 직접+깊은 배치 셀, 도형 없는 셀, 미배치 셀)를 KLayout으로 대조 — C1 `floe2 index`가 design.ovh를 쓰고 `floe-index hier --check` identity=ok·타 캐시 파일 거부(rc 1), C2 모든 셀의 서로 다른 자식과 멤버 수 = KLayout 인스턴스 배열 size 합·leaf 표기, C3 탑 아래 인스턴스 수 = 탑다운 곱셈 합(탑 1·orphan 0), C4 cell_find 부분일치/글롭 대소문자 무시·이름순·total+limit, C5 cell_bbox 탑 직계 = 인스턴스 박스 정확 합집합·깊은 셀 = 블록 범위 상위집합(approx=1)·orphan/도형 없음 None, C6 cell_insts 뷰 안 박스 = KLayout 전개 탐색(회전·미러·격자·불규칙 반복)·cap → more=1·탑 = 자기 박스, C7 파일 삭제 시 소형 캐시 메모리 요약(파일 안 씀)·`FLOE_RUST_HIER_INLINE_PLACES=0`이면 code=nohier·`--hier-only` 뒤 **같은 데몬**이 집어 듦·캐시 없는 소스 거부, C8 뷰 루트(`root=BLK`) 프레임 == BLK를 탑으로 한 별도 레이아웃(KLayout copy_tree) 프레임 바이트 동일(3뷰)·탑 뷰와는 다름·루트 아래 cell_bbox(직계 정확·6)·cell_insts(KLayout BLK 탐색 12)·루트 위 셀 0·테이블 밖 루트 거부·보이는 레이어가 없는 루트(VIA에 2/0만) = 검은 프레임(오류 아님)·1/0이면 그려짐·density stack 켬 == 끔 |
| R1~R5 | `tools/validate_svrf.py` | SVRF 서브셋 파서 `floe-index svrf`(.rules.json; 2026-09-29 floe/svrf.py에서 이식 — 게이트는 게이트 전용 `--dump-state`로 파스 상태를 읽는다): R1 전처리(INCLUDE 상대경로 병합·순환 경고·#IFDEF/#ELSE -D 분기·#DEFINE 값 치환→제약·VARIABLE 수치 해석·--scan 양분기), R2 derivation 그래프(다이아몬드 폐쇄→전 원천 LAYER+MAP dt·순환 종료·미정의→unresolved·연산자 비누출), R3 체크 추출(다중 @ 결합·이중 한계 2제약·붙은 op·`ABUT<90` 비제약·측정 우변 할당문·미지 문장 카운트·따옴표 체크명·DMACRO 통스킵), R4 gen_drcdb --svrf 엔드투엔드(db 체크명 100% 매칭·제약값 생성식 일치·전 체크 gds 도달·-D SYNTH_EXTRA 정확히 1룰 추가·JSON 왕복), R5 명령 계약(사이드카 = 파이썬 json.dump(indent=1, sort_keys=True) 바이트 — 비ASCII·따옴표·1e-05/1e+16·null dt, 기본 `<deck>.rules.json`, --scan은 파일 안 씀, `-DNAME`/`-D=NAME`/`--define=NAME`, 없는 덱 rc 1·모르는 옵션 rc 2(파일 없음), `floe2 svrf`는 floe-index 명령줄 안내 후 rc 2, 빌더 연산자 단어 == 뷰어 rhs_operands) |

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
