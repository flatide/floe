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
  0.12.271(2패스 예산 맞춤은 셀의 컷을 두고 한 번에):
  - 게이트 `density_stack` `ladder_checks`:
    - 픽스처: VIA 2만 개(셀 점)와, TOP이 직접 가진 0.3~0.9 µm 상자 6만 개(한 페이지, 추정 10.6 MB). 예산 4 MB(고정
      예약 0.5 MB).
    - 1·2 스레드 모두 한 번에 맞추고, 그 페이지를 빼며(thinned), 같은 프레임을 그린다.
    - 사다리(`FLOE_RUST_DENSITY_FIT_LADDER=on`)는 6번 걸어 같은 프레임에 이른다.
    - 기본 예산은 그 페이지를 디코드한다.
  - 단위 `a_dots_plan_over_its_budget_keeps_the_cells_cut_and_thins_its_pages_in_one_pass`.
  0.12.281(2패스 영역을 빈 칸으로, 위층과 아래층은 따로; 리스트 단위 건너뛰기):
  - 게이트 `density_stack` `cells_checks`: LOW의 7.7 µm 상자가 x 0~32 µm를 3 px 틈만 남기고 덮는다. MID의 0.1 µm 비아는
    0.5 µm마다 점이고, TOP의 반점은 위층 점이다.
    - 타일 bbox(`FLOE_RUST_DENSITY_FREE_CELLS=off`)는 joint로 맞춤 1번이다. 칸 방식은 분리 계획으로 2번이고 항목이 더
      적다(880 / 3,360).
    - 틈만 지나는 칸(빈 픽셀 96 px, 1/8 미만)은 하위 밀도를 생략한다. 아래층 점이 더 적고(652 / 721 px) 위층은 같다.
      다른 픽셀(69)은 모두 틈에 있다.
    - `FLOE_RUST_DENSITY_OTHERS_MIN=0`은 타일 bbox의 프레임과 바이트까지 같다.
    - 0.12.282: 이름만 있는 빈 레이어 ALONE을 맨 위로 보이면 위층 계획이 비지만 프레임이 그려진다(위층 0 px, 아래층 > 0;
      0.12.281은 `invalid plan: top … is missing`). 루트 칩의 BOUNDARY 100/0과 같은 경우다.
    - 0.12.283(도형이 있는 켜진 레이어 중 최상위):
      - ALONE을 맨 위로 켜면 MID가 위층이 되고, 프레임과 밀도 수가 LOW·MID만 켠 것과 같다(위층 > 0). 킬 스위치
        `FLOE_RUST_DENSITY_TOP_HELD=off`에서는 위의 0.12.282 경우다.
      - DEEP(TOP > NEST > DEEP_CELL, 두 단계 아래의 0.1 µm 정사각형 240개)을 켠 depth 1 프레임은 LOW·MID 프레임과
        같다(DEEP이 빠짐). depth 2에서는 DEEP이 위층이다. 킬 스위치에서는 depth 1의 위층이 0 px다.
  - 0.12.284 `shift_checks`(세계에 고정한 칸, 칸의 실제 면적):
    - LOW 위의 통로 14개(0.6~1.4 µm, 3.7 µm 간격)에 MID 비아 점, 맨 위는 TOP 반점이다. 0/3/7/11 px 팬에서 하위
      밀도(4,570 px)와 남은 칸의 빈 픽셀(17,200 px)이 매번 같고, 타일 bbox와도 같다. 16ea8fc는 흔들렸다.
    - 오른쪽 끝(x ≥ 38.4 µm)의 빈 띠를 폭 386 / 400 px로 보면 하위 밀도(47 / 740 px)가 타일 bbox와 같다. 16ea8fc는
      0 / 725 px였다.
    - 0.12.298(뷰어의 밀도 켜기/끄기) `toggle_checks`:
      - `gate_layout`의 MID 사각형을 0.4 µm/px에서 본다.
      - 환경 변수 없는 워커의 `density=on` 프레임은 `FLOE_RUST_DENSITY_STACK=top` + `FLOE_RUST_DENSITY_DOTS=on` 워커의
        필드 없는 프레임과 바이트까지 같다(706 px). 그 워커의 `density=off`는 환경 변수 없는 프레임과 같다(0 px).
      - 한 뷰에서 끔 → 켬 → 끔: 켬은 밀도를 그린다(재사용 0타일). 다시 끔은 처음 프레임과 같다(보관 프레임에서 28타일).
        `RetainedKey::density`를 빼면 켬이 밀도 없는 보관 프레임으로 통째로 그려져 실패한다(확인함).
      - 단위:
        - renderd `retained_key_tracks_the_density_toggle`: `density=on|off`와 없음의 파싱, 켬·끔의 키가 다름, 없음은
          환경 기본, `density=maybe`는 오류.
        - rust_renderer 어댑터 계약: 작업의 `density`가 있을 때만 명령 끝에 ` density=on|off`가 붙는다.
        - rust_renderer 뷰어 계약 `test_gui_density_toggle_follows_backend_capability`: 밀도가 없는 렌더러에서는 토글이
          아무것도 바꾸지 않고 메뉴가 비활성이다. Rust 렌더러에서는 토글이 렌더 키를 바꾸고 다시 그린다. `v`, 메뉴,
          상태줄, 두 렌더 작업에 실리고, 넘어온 `density=`가 상태를 바꾼다.
    - 0.12.297(밝기: 픽셀에 모인 도형 면적만큼, 원본 색을 넘지 않게) `bright_checks`:
      - `gate_layout`의 MID 0.05 µm 사각형(0.016 px²)을 0.4 µm/px에서 본다. 모두 하한 아래라 점유 격자로 퍼뜨린다.
      - 켜진 픽셀은 모두 MID 색의 배수이고, 어느 채널도 색을 넘지 않는다.
      - 0.6 % 반쪽의 알파 합이 g × 사각형 면적(62.5 px²)의 15 % 안이다: g 1·2·4(low·medium·high)에서 61.5·123.1·246.2.
        문턱으로 빠지는 블록이 없다(`dot_gated` 0).
      - 25 % 마당은 색의 0.25·0.49·0.98이다.
      - 두 단계 축소(0.8 µm/px)하면 반쪽의 알파 합이 fit의 4분의 1이다(30.4). 줌아웃 이득처럼 더 줄지 않는다.
      - 킬 스위치(`FLOE_RUST_DENSITY_BRIGHT=off`): 점 모드다. 반쪽은 문턱으로 빠지고, 켜진 픽셀은 색 그대로다.
      - `shapes_first_layout`에서 LOW 원본 14,910 px는 밝기를 켜도 끄고 혼자일 때와 같다. TOP의 밀도는 그 둘레 10,100 px에
        보인다.
      - 이 게이트의 다른 검사는 `FLOE_RUST_DENSITY_BRIGHT=off`로 고정한다. 점을 켜진 픽셀로 센 검사들이다.
      - 어댑터 계약(`rust_renderer`): `density_plan2`는 35개 값(…/dot_gate_min/bright_milli)이다. 상태줄 테스트: 밝기
        g 2면 `reserve 896 MB, bright x2, pass 2 plan`(줌아웃 이득·문턱 없음).
      - 단위:
        - vfs `under_the_brightness_a_block_counts_the_area_its_content_covers`(64 px 블록 상한 포함).
        - render-core `under_the_brightness_a_pixel_shows_its_covered_area_never_past_the_colour`: 0.3 px²는 색의 0.6,
          한 픽셀의 1.28 px²는 색 그대로, 1×1 px가 네 픽셀에 걸치면 각각 0.5. 타일 5·8·16·128 px, 작업자 1~4, bin 켬·끔이
          같다.
        - render-core `under_the_brightness_the_planes_compose_over_one_another_the_top_first`: 초록·빨강·흰색이 0.25 px²씩
          한 픽셀에 있으면 (96, 159, 32). 최상위 평면 둘이든, 하나에 아래 평면 한 번 걷기든 같다.
        - render-core `under_the_brightness_a_lattice_array_covers_as_its_members_one_by_one`: 격자 배열을 한 번에 더한 것
          (`bright_lattice`)이 같은 멤버를 점 리스트로 하나씩 더한 것과 채널 1 안에서 같다.
        - render-core `a_lattice_of_many_members_to_a_pixel_spreads_them_over_its_hull`: 0.2 px 간격 0.1 px 멤버는 픽셀마다
          5. 한 축에 타일당 65,536개를 넘는 20만 개는 헐에 고르게 퍼져 합이 20만이다.
    - 0.12.295(하한 탐색 스레드, 점 하나어치 리스트 표본, 정확한 빠른 연산):
      - `probe_threads_checks`: `lists_layout`의 비아를 하한 0, 9타일(1,000 px)로 본다. 탐색이 맞고, 프레임이 계획 하나의
        것과 픽셀까지 같다. 스레드는 4이고, 킬 스위치(`FLOE_RUST_DENSITY_PROBE_THREADS=off`)면 1이다.
      - `lists_checks`는 두 번 돈다.
        - 모든 멤버를 읽을 때(`FLOE_RUST_DENSITY_LIST_BY_DOT=off`): 지금까지와 같다(4,000개).
        - 점 하나어치에 하나를 읽을 때(0.11 px² 비아, 8개에 하나): 1·2·4 스레드와 bounds의 프레임이 하나이고, 멤버를
          8분의 1 이하로 읽는다(500개).
      - `dense_lists_checks`는 `FLOE_RUST_DENSITY_LIST_BY_DOT=off`로 고정한다. 모든 멤버를 읽은 프레임과 바이트까지
        같은지가 그 검사의 기준이다.
      - 단위:
        - vfs `a_list_member_under_a_dot_is_read_one_in_the_members_that_make_a_dot`.
        - render-core `the_fast_arithmetic_is_the_i128_one`: 2의 거듭제곱(2^0~2^59)과 아닌 제수로 나눈 floor·ceil,
          i128→f64, 장치 좌표(i64 밖 포함)가 i128 연산과 같다.
        - render-core `the_i64_rows_are_the_i128_sums`: 8방향, i64 양 끝 좌표에서 변환이 i128 합과 같다(넘침은 오류로
          같다).
    - 0.12.294(1패스 도형이 먼저: 모든 평면의 밀도는 원본이 없는 곳에만) `shapes_first_checks`:
      - 장면:
        - LOW의 18×16 µm 원본(x 2~20 µm).
        - TOP(4/0)의 0.05 µm 사각형 40,000개. 띠(x 10~30 µm)가 LOW의 오른쪽 가장자리를 가로지른다.
        - TOP1(4/1)의 점(띠 밖).
      - TOP이 맨 위 평면(LOW, TOP)이든 TOP1 아래의 최상위 평면(LOW, TOP, TOP1)이든 다음이 성립한다.
        - LOW 원본 위(x 10~20 µm)에 TOP이 없고, 그 칸은 LOW 단독과 같다.
        - 오른쪽(x 20~30 µm)은 LOW 단독이 비워 둔 픽셀에서 TOP 단독과 같다(4,625 px). LOW 외곽선이 가장자리 다음 열을
          칠한다.
        - 2패스 최상위 면이 계획한 빈 픽셀(`free_top`)은 50,859 px로, 킬 스위치를 켰을 때(80,000)보다 적다.
      - 킬 스위치(`FLOE_RUST_DENSITY_SHAPES_FIRST=off`)면 LOW 원본 위에도 TOP 단독처럼 켠다(4,664 px).
      - 이 게이트의 다른 검사는 `main`이 `FLOE_RUST_DENSITY_SHAPES_FIRST=off`로 고정한다. "최상위 평면은 아래 원본 위에도"
        규칙(§10.10) 위에서 만든 검사들이다.
      - 단위 `with_the_shapes_first_no_plane_draws_its_density_over_an_original`:
        - 최상위 평면 1개 / 2개: 원본 위에 없고 빈 곳은 단독과 같다. 2패스 최상위 면이 받는 영역은 아래 평면의 영역과
          같다.
        - 끄면 원본 위에도 그리고, 받는 영역도 더 넓다. 타일·워커와 무관하다.
      - vfs 단위 `a_page_decoded_under_the_floor_keeps_aside_the_blocks_a_box_holds_whole`(`HierOpts::dot_occ_boxes`).
    - 0.12.293(최상위 평면은 그리기 순서상 위에서 8개, 평면마다 따로 계획; 0.12.292는 맨 위 레이어 번호) `top_group_checks`:
      - LOW의 36×16 µm 원본, TOP(4/0)의 0.05 µm 사각형 40,000개(띠), TOP1(4/1)의 점 몇 개(띠 밖, 맨 위 평면).
      - TOP의 띠 안 픽셀(9,330 px)이 LOW를 켜도 같다. 킬 스위치(`FLOE_RUST_DENSITY_TOP_GROUP=off`)면 TOP은 아래 평면이라
        LOW 원본에 가려 0 px다. LOW는 TOP이 켜지 않은 곳에 보인다(5,383 px).
      - 컷 아래 표준 셀 6,000개(셀마다 LOW 상자 + MID): 둘 다 최상위 평면일 때 LOW 1,062 px로, 아래 평면일 때(킬 스위치)와
        같다(±10 %). 한 계획으로 묶으면 셀이 맨 위 레이어로만 세어져 LOW가 없었다.
      - 이 게이트의 다른 검사는 최상위 평면 하나로 고정한다(`main`이 `FLOE_RUST_DENSITY_TOP_GROUP=off`). 레이어 3~4개짜리
        장면이라 기본값(위에서 8개)이면 모두 최상위 평면이 되어, 아래 평면 걷기를 전제로 한 검사(`lower` > 0 등)가 맞지 않는다.
      - 단위 `every_top_plane_draws_its_density_over_the_originals_below_it`(최상위 평면 1개 / 2개, 위의 최상위 평면 원본).
    - 0.12.291(밀도만, 진단) `density_only_checks`:
      - LOW의 36×16 µm 원본 아래, MID의 0.05 µm 사각형 40,000개(띠). 예산 64 MB.
      - `FLOE_RUST_DENSITY_ONLY=on`이면 LOW의 원본이 0 px(끄면 24,911 px), MID의 밀도는 같은 픽셀(9,443 px), 1패스가 든
        페이지 0(끄면 1), 2패스 예약 64 MB(예산 전체).
      - 상태줄 테스트: 환경 변수를 켜면 `[density: dots, density only, lit …`.
    - 0.12.290(위층 페이지 먼저) `top_first_checks`:
      - 루트 칩을 1/4 배율로 만들어(`tools/gen_route_chip.py --scale 0.25`, 약 4 s) 색인하고, 예산 64 MB
        (`FLOE_RUST_BUDGET_MB`), fit(1350×971), depth 0에서 M8(38/0)과 M1(31/0)을 본다.
      - M8만 켠 픽셀(1,877 px)이 M1을 켠 프레임에서 모두 같다. 킬 스위치(`FLOE_RUST_DENSITY_TOP_FIRST=off`)는 페이지가
        예약을 넘고(`over_budget` > 0) M8을 95 % 미만(66 %) 남긴다.
    - 0.12.288(detail 밀도 문턱) `gate_checks`:
      - MID의 0.05 µm 사각형: 왼쪽 40×40 µm에 무작위 4,000개(0.6 %), 오른쪽 20×20 µm에 0.1 µm 간격(25 %).
      - 0.4 µm/px에서 모두 하한 아래라 점유로 퍼뜨린다(`occ_pages` ≥ 1, 디코드 0). medium 컷(3 px)은 블록에 2점이 필요하다
        (`dot_gate_min` 2, `dot_gated` > 0).
      - 성긴 반쪽은 0 px이고, 킬 스위치(`FLOE_RUST_DENSITY_GATE=off`)와 high 컷(1 px, 문턱 없음)은 80 px다. 25 % 영역은
        601 px로 끈 것과 같다(≥ 영역의 15 %, ≥ 끈 것의 90 %).
      - 이 게이트의 다른 검사는 `main`이 문턱을 끈다(점 수를 문턱 없이 센다).
      - 단위 `a_dot_block_too_sparse_for_the_detail_is_left_out`: 1점 블록 100개는 빠지고 4점 블록은 남는다. 영역 넷으로
        나눠도 같고, 킬 스위치는 모두, 몫 1/4은 4점 블록, 0.3은 없음이다. 미룬 항목(`settle_occ_fallback`)도 1점은 빠진다.
        점 수 규칙을 보는 기존 단위 9개는 `dot_gate: false`로 고정했다.
      - 어댑터 계약: `density_plan2`는 34개 값(…/dot_gain_milli/dot_gated/dot_gate_min)이다. 상태줄 테스트:
        `dots x0.64, gate 2/16 px (12k out)`(한 번만).
    - 0.12.287(축소할수록 성기게) `zoom_out_checks`:
      - 40×20 µm 다이에 무작위 0.1 µm 비아 3,000개(MID)와 TOP 반점을 둔다.
      - fit 뷰에서 `dot_gain_milli`는 1000이다. 2배 축소(0.2 µm/px)에서는 round(1000 × fit / 0.2) = 524이고, 점(lit)은
        킬 스위치(`FLOE_RUST_DENSITY_ZOOM_OUT=off`)의 그만큼(±12 %, 760 → 393 px)이다.
      - 그 뷰의 여백(bg, 2W×2H, 어댑터가 `vw`/`vh`를 보냄)은 같은 비율이고, 가운데가 뷰포트 프레임과 바이트까지 같다.
      - 게이트의 다른 검사는 감쇠를 끈다(`main`이 `FLOE_RUST_DENSITY_ZOOM_OUT=off`, `worker`는 이전 값을 되돌림). 고정
        뷰(400×200 px, 0.1 µm/px)가 대부분의 다이보다 넓어서, 켜 두면 fit 기준의 점 개수(예: `dots_checks`의 성긴
        배열 100 px → 96)가 줄어든다.
      - 어댑터 계약: 여백 작업의 명령 줄이 ` bg=on vw=20 vh=10`으로 끝나고, 뷰포트 자신(view = bbox)이나 view가 없으면
        `vw`가 없다. `density_plan2`는 32개 값(…/free_others/dot_gain_milli)이다. 상태줄 테스트: `reserve 896 MB, dots x0.64`.
    - 0.12.286(1점보다 작은 멤버는 면적만큼): 단위 `a_member_under_a_pixel_stands_for_its_area_in_dots`.
      - 0.2 px LEAF 6,000개(0.04 px²)가 약 240점으로 면적 ±15 % 안이다. 끄면 5,000점을 넘는다.
      - 빠른 길을 끈 것과 항목이 같고, 영역 넷으로 나눠도 점 수가 같다.
      - 게이트 `lists_checks`는 424 px(0.12.285 4,000), `ladder_checks`의 비아 점은 2,239 px(19,954)다.
    - 0.12.285: 세계 원점을 가로지르는 1 px 팬(x0 −0.05 / +0.05 µm)에서, 칸 경계에 걸친 통로 하나(x 2.88~3.58 µm)의 하위
      밀도가 308 / 308 px로 타일 bbox와 같다. fd4fdcb는 0 / 308 px였다(오프셋의 round()).
  - `ladder_checks`: 맞춤은 계획마다 한 번이다(칸 방식은 위층·아래층 두 계획: passes 2).
  - 단위 `a_small_point_list_in_full_blocks_is_passed_over_whole`.
  - 어댑터 계약: `density_plan2` 31개 값(…/free_top/free_others). 상태줄 테스트: `24 regions (free top 1.3M, others 210k
    px)`.
  0.12.280(점 리스트: 꽉 찬 블록의 청크는 건너뛰고, 조밀한 청크는 표본으로):
  - 게이트 `density_stack` `dense_lists_checks`: 비아 셀 두 개의 0.1 µm 비아를 20 µm에 무작위 6만 개씩 둔다(KLayout
    압축, 셀마다 점 리스트).
    - 100·200 px에서 기본, 1 스레드, `FLOE_RUST_DENSITY_LIST_FULL=off`, `FLOE_RUST_DENSITY_LIST_SAMPLE=off`가 모두 셋을
      끈 프레임(0.12.279의 걷기, 12만 개 모두 읽음)과 바이트까지 같다.
    - 두 번째 리스트의 청크는 첫 리스트의 꽉 찬 블록에서 건너뛴다(`full_chunks` 약 200).
    - 100 px에서는 조밀한 청크를 표본으로 읽어 멤버를 5분의 1 미만 읽는다(20,128개). 200 px에서는 표본이 없다.
  - 단위 `a_point_list_chunk_in_full_blocks_is_passed_over_and_a_dense_one_read_at_a_step`. 어댑터 계약: `density_plan2`
    29개 값(…/occ_decoded/full_chunks/full_members/sampled_chunks/sampled_members). 상태줄 테스트: `list chunks in full
    blocks 2.9M of 742.0M members, list chunks sampled 60k of 15.4M members`.
  0.12.279(2패스 디코드·래스터 시간을 로그 줄에): 어댑터 계약 `density_us` 6개 값(…/decode2_us/raster2_us), 상태줄 테스트
  `pass 2 decode 812 ms, raster 24310 ms`(로그 줄에만).
  0.12.278(칸이 큰 페이지는 디코드):
  - 게이트 `density_stack` `coarse_checks`: `occ_checks`의 두 정사각형 페이지를 1 µm/px로 본다(600 px, 칸 9.4 px, 0.3 µm
    상자는 하한 아래).
    - 기본은 디코드해 컷 없는 프레임과 같은 픽셀(3,311 px)을 켜고, `occ_decoded`가 1 이상이다.
    - `FLOE_RUST_DENSITY_OCC_DECODE=off`는 퍼뜨린다(3,933 px).
    - 고정 예약 128 KB(`FLOE_RUST_BUDGET_MB=1`, `RESERVE_LEFT=off`)로 예산 맞춤이 페이지를 빼면, 예비 점이 퍼뜨리기와 같은
      수(2 % 안)로 켜진다.
  - `occ_checks`는 칸 15.6 px 뷰라 `OCC_DECODE=off`로 퍼뜨리기만 검사한다.
  - 단위 `a_page_under_the_floor_too_coarse_for_its_grid_is_decoded_or_its_dots_stand_in`. 어댑터 계약: `density_plan2` 25개
    값(…/occ_pages/occ_decoded). 상태줄 테스트: `pages decoded under the floor 6`.
  0.12.277(페이지 퍼뜨리기 기본 켬, design.ovb가 있을 때):
  - 게이트 점 절: 1 px 하한 아래 TOP의 0.5 px 점은 기본에서 컷 없는 프레임의 4분의 1 안(48 / 50 px)으로 켜지고,
    `occ_pages`가 1 이상이다. `FLOE_RUST_DENSITY_PAGE_SPREAD=off`는 0 px이고 나머지 픽셀은 같으며, 점 항목은 셀의
    것뿐이다. 0.6 px 하한 검사는 퍼뜨리기를 끈 채로 한다.
  - `left_checks`: 고정 예약 4 MB(탐침 초과)도 기본에서는 TOP의 1 px 미만 상자를 색인으로 컷 없는 프레임의
    4분의 1 안(20,541 / 19,982 px)으로 켠다. 퍼뜨리기를 끄면 0이다.
  - `occ_checks`·`mixed_checks`는 기본 설정으로 돈다. design.ovb 없는 캐시는 기본에서 아무것도 켜지 않고,
    `=on`에서 상자 퍼뜨리기다.
  - 단위: 색인이 없는 픽스처는 기본에서 그리지 않고, `dot_page_spread_boxes`일 때 상자에 퍼뜨린다(퍼뜨리기·점유
    격자·노드 테스트).
  0.12.276(페이지 BVH 노드 항목도 색인 면적으로): 단위 `a_page_bvh_node_under_the_floor_counts_the_area_its_pages_cover`
  (SPEC-PLANNER §3), ovm 테스트에 `page_occ_area`(격자 페이지의 칸 넓이 × 단계, 두 번째 호출도 같은 값, 기록 없음은
  None). 라우팅 합성 칩의 fit·4배 넓은 뷰는 이 노드가 생기지 않아 0.12.275와 같다.
  0.12.275(점유 격자 v2: 칸마다 덮인 면적, 디코드된 페이지의 하한 아래 도형):
  - 게이트 `density_stack` `occ_checks`: design.ovb가 v2(머리말 version 2, 페이지 수가 맞음)이고 페이지마다
    2 KB보다 작다(두 정사각형 픽스처 185 B). 나머지 기대는 0.12.274와 같다.
  - 게이트 `density_stack` `mixed_checks`:
    - 픽스처: 300 µm 레이아웃. 아래 절반에 0.2 µm 상자 2만 개(2 % 덮임), 위 절반에 1.2 µm 상자 3만 개(거의 다
      덮임)를 둔다. 한 페이지다. 1000 px, 깊이 0, 페이지 퍼뜨리기 켬.
    - 300 µm 뷰(0.3 µm/px)에서는 1.2 µm 상자가 1패스의 것이고 페이지가 디코드된다. 아래 절반의 0.2 µm 상자가
      컷 없는 프레임과 같은 7,821 px를 켠다. `FLOE_RUST_DENSITY_UNDER_FLOOR=drop`은 0이다.
    - 1,600 µm 뷰(1.6 µm/px)에서는 모두 하한 아래라 퍼뜨려진다. 아래 절반이 298 px로 컷 없는 275 px의 3분의 1
      안이다. `FLOE_RUST_DENSITY_OCC_COVER=off`(같은 몫)는 7,823 px로 3배 이상이다.
  - 단위:
    - ovm `a_page_occupancy_file_attaches_to_its_own_index`: 단계(2배 간격, 0·1·15 경계), 격자 압축 왕복, 합계
      기록(NaN·음수 거부), `occ_wants_grid`. v1 머리말, 다른 출처·ovp·페이지 수, 잘린 표, 순서가 어긋난 표,
      magic 불일치는 붙지 않는다.
    - 인덱서 `page_occupancy_holds_the_area_of_every_member_in_its_cells`: 점 리스트 둘, 직교 Grid 둘(폭 > 간격,
      음의 간격 포함), 비스듬한 Grid, 단일 상자의 페이지마다, 칸 넓이가 payload 멤버로 잰 것과 1e−9 안에서 같고
      단계도 같다. 큰 상자가 있는 페이지는 멤버 넓이의 합(8 B)만 남긴다.
    - 플래너 `a_page_spread_over_its_occupancy_grid_puts_the_area_its_cells_cover`,
      `a_page_without_a_grid_spreads_the_area_its_shapes_cover_over_its_box`(SPEC-PLANNER §3).
  0.12.274(페이지 점유 비트, design.ovb):
  - 게이트 `density_stack` `occ_checks`:
    - 픽스처: TOP이 0.3 µm 상자 4만 개를 직접 가진 600 µm 레이아웃(100 µm 정사각형 둘을 양 끝 모서리에 둠, 점 리스트로
      색인, 페이지 하나). 1000×1000 px, 깊이 0, 페이지 퍼뜨리기 켬.
    - design.ovb가 `64 + 512 × 페이지`(576 B)다.
    - 두 정사각형 사이(200~800 px)에는 아무것도 켜지 않는다(컷 없는 프레임도 0). `occ_pages`는 1 이상이다.
    - `FLOE_RUST_DENSITY_PAGE_OCC=off`는 상자 전체에 퍼뜨려 사이에 3,627 px를 켠다.
    - `--no-page-occupancy`로 색인한 캐시, 그리고 그 캐시에 다른 색인(한 시간 이른 mtime)의 design.ovb를 둔 경우도
      `PAGE_OCC=off`와 바이트까지 같고 `occ_pages`는 0이다.
  - 단위:
    - ovm `a_page_occupancy_file_attaches_to_its_own_index`: `occ_cell`이 `occ_edge`의 칸에 넣는다(길이 1~4,097).
      출처·페이지·격자·길이가 다르거나 magic이 아니면 붙지 않는다. 기록 없는 페이지는 None이다.
    - 인덱서 `page_occupancy_marks_the_cells_of_every_member_and_no_other`: 점 리스트 둘(떨어진 무리 포함), 직교 Grid,
      비스듬한 Grid, 단일 상자의 페이지마다, payload를 펼친 멤버로 칠한 격자와 같다. 반 넘게 빈 페이지가 있다.
    - 플래너 `a_page_spread_over_its_occupancy_grid_leaves_its_empty_cells_empty`(SPEC-PLANNER §3).
  - 어댑터 계약: `density_plan2` 24개 값(…/reserve_mb/occ_pages). 상태줄 테스트: 로그 줄의 `pages 257 (250 by
    occupancy)`.
  - 실측 비교(정답 | 상자 전체 | 점유 비트)와 색인 비용은 CUT_DENSITY_DESIGN §10.12와 SPEC-FORMATS에 있다.
  0.12.273(라우팅 합성 칩): `tools/gen_route_chip.py`. 탑 자신의 1 px 미만 배선·비아, 칩 전체 비아 점 리스트,
  블록과 표준 셀을 만든다. 조건별 재현은 CUT_DENSITY_DESIGN §10.12에 있다. 작업 프로세스 수와 무관하게 바이트까지
  같은 파일을 만든다(규모 0.02로 확인). 게이트에는 넣지 않았다(데이터 생성 도구).
  0.12.272(하한 아래 페이지 퍼뜨리기, 선택): 단위 `a_page_under_the_floor_spreads_its_shapes_dots_over_its_box`.
  비교 이미지(정답 | 퍼뜨리기 | 지금)는 CUT_DENSITY_DESIGN §10.12에 있다(기본이 꺼져 있어 게이트에는 넣지 않았다).
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
| C1~C9 | `tools/validate_cell_tree.py` (`cell_tree`, `render`·`indexer` 별칭, 약 20초) | 셀 트리(SPEC-VIEWER §8c, design.ovh): 계층 픽스처(블록 3배치 = 평·90°·미러, 블록 안 3×2 격자, 탑 직접+깊은 배치 셀, 도형 없는 셀, 미배치 셀)를 KLayout으로 대조 — C1 `floe2 index`가 design.ovh를 쓰고 `floe-index hier --check` identity=ok·타 캐시 파일 거부(rc 1), C2 모든 셀의 서로 다른 자식과 멤버 수 = KLayout 인스턴스 배열 size 합·leaf 표기, C3 탑 아래 인스턴스 수 = 탑다운 곱셈 합(탑 1·orphan 0), C4 cell_find 부분일치/글롭 대소문자 무시·이름순·total+limit, C5 cell_bbox 탑 직계 = 인스턴스 박스 정확 합집합·깊은 셀 = 블록 범위 상위집합(approx=1)·orphan/도형 없음 None, C6 cell_insts 뷰 안 박스 = KLayout 전개 탐색(회전·미러·격자·불규칙 반복)·cap → more=1·탑 = 자기 박스, C7 파일 삭제 시 소형 캐시 메모리 요약(파일 안 씀)·`FLOE_RUST_HIER_INLINE_PLACES=0`이면 code=nohier·`--hier-only` 뒤 **같은 데몬**이 집어 듦·캐시 없는 소스 거부, C8 뷰 루트(`root=BLK`) 프레임 == BLK를 탑으로 한 별도 레이아웃(KLayout copy_tree) 프레임 바이트 동일(3뷰)·탑 뷰와는 다름·루트 아래 cell_bbox(직계 정확·6)·cell_insts(KLayout BLK 탐색 12)·루트 위 셀 0·테이블 밖 루트 거부·보이는 레이어가 없는 루트(VIA에 2/0만) = 검은 프레임(오류 아님)·1/0이면 그려짐·density stack 켬 == 끔, C9(0.12.296) 파일이 이름만 둔 빈 레이어(3/0 NOTHING)만 켬 = full depth(프레임 켬·끔)·depth 0 프레임 끔은 검은 프레임, depth 0 프레임 켬은 1/0을 켰을 때와 같은 depth 밖 외곽선 픽셀만(오류 아님, 0.12.295는 `invalid plan: top … is missing`)·density stack 점 켬도 같음·1/0이면 그려짐 |
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
