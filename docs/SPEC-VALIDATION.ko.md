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
  (release `--lib`; release 프로필의 doctest는 LTO와 어긋나 제외), `unit_renderd`(release, renderd의 유닛;
  0.12.304 — `render` 별칭에 포함).
- **바뀐 파일로 고르기(`--changed`, 2026-10-06; 사용자: "매번 배터리 통과를 기다리는 것은 비효율적 — 관련 있는 검사만").**

  ```sh
  sh tools/validate_rust.sh --changed              # HEAD와 다른 파일(작업 트리·인덱스·새 파일)이 부르는 게이트
  sh tools/validate_rust.sh --changed=origin/feature/jobdeck   # 그 리비전 이후 바뀐 파일
  sh tools/validate_rust.sh --changed --dry-run    # 무엇을 돌릴지만 말한다
  sh tools/validate_rust.sh --files=rust/vfs/src/hier.rs,floe/gui.py --dry-run   # 그 경로들이 부르는 게이트
  ```

  - 경로마다 게이트가 정해져 있다(스크립트의 `gates_for`; 먼저 맞는 규칙이 이긴다).
    - 문서(`docs/`, `*.md`), 게이트가 쓰지 않는 도구(벤치·실험·생성기): 없음. 전부 그런 파일이면 아무것도 돌리지
      않고 `RUST VALIDATION: ALL OK (--changed; gates: none)`으로 끝난다.
    - `tools/validate_<게이트>.py`: 그 게이트.
    - 버전 줄만 바뀐 `floe/__init__.py`·`Cargo.toml`·`Cargo.lock`(푸시마다 바뀐다): `rust_renderer`, `floe2`,
      `index_cli`. 다른 줄도 바뀌었으면 전체.
    - `floe/gui.py` 등 뷰어: `rust_renderer`, `floe2`, `jobdeck`, `density_stack`, `index_lock`, `perf_parity`, `gtk_view`(P4d부터 렌더 루프가 컨트롤러의 것).
    - 뷰어 perf 줄의 Rust 이식(`rust/app-core/src/view/perf.rs`, `rust/app-core/tests/perf_parity.rs`, P4b): `perf_parity`.
    - 개발 전용 오라클 `tools/oracle/floe_oracle/`(동결 floe 셸과 Python 기준 구현, P3 2026-10-10):
      - `indexlock.py`(색인 잠금의 Python 쪽): `index_lock`, `jobdeck`, `occupancy`, `cell_tree`, `drc_ice`,
        `rust_renderer`, `index_cli`
      - `cli.py`·`jobdeck/*`: `index_lock` 등
      - KLayout 쪽(`service`·`render`·`viewport`·`coverage`·`view_policy`·`vfsclient`): 픽셀 오라클 게이트
      - `cache.py`·`cachepath.py`: 전체 배터리
    - 제품 명령줄인 Rust `floe2`(`rust/app-cli`·`rust/floe2`, 2026-10-09 P1c): 그것을 부르는 게이트 전부 — `unit`,
      `floe2`, `jobdeck`, `occupancy`, `cell_tree`, `index_lock`, `svrf`, `oasis_shapes`, `rust_renderer`,
      `representatives`, `fit_budget`, `sub_cut_box`, `shape_cut`, `write_once`, `layer_decode`, `area_true`,
      `density_stack`. `rust/app-core`는 여기에 `cell_index`를 더한다. `floe/gtkview.py`(GTK 진입점): `floe2`, `rust_renderer`,
      `jobdeck`.
    - renderd·render-core·render-cli·`floe/rust_render.py`(그리는 경로): `unit_render`, `unit_renderd`,
      `rust_renderer`, `jobdeck`, `occupancy`, `fit_budget`, `sub_cut_box`, `shape_cut`, `write_once`,
      `layer_decode`, `area_true`, `density_stack`, `cell_tree`, `representatives`, `oasis_shapes`, `floe2`,
      `klayout`, `perf_parity`. `deck.rs`·`cells.rs`는 더 좁다.
    - 계획기(`rust/vfs/src/hier.rs`·`cover.rs`): 위에 `unit_vfs`, `vfs_hier`, `vfs_lifecycle`, `vfs_marker`,
      `vfs_split`, `vfs_text`, `vfs_profile`. `hiersum.rs`·`occupancy.rs`·`representatives`는 더 좁다.
    - 모든 것이 걸린 파일 — 파서(`rust/oasis`), 인덱스 형식(`rust/ovm`), tiler, VFS의 나머지, floe-index
      (`rust/cli`), vendor, 오라클의 `cache.py`·`cachepath.py`, 이 스크립트, `gen_valmini.py` — 과 표에 없는 경로:
      **전체 배터리**.
  - 시작할 때 `== gates to run:`과 `== gates left out:`을 찍는다. 빠진 게이트가 무엇인지가 그 실행의 한계다.
  - 실행마다 끝에 게이트별 시간이 나온다: `== gate seconds (513s in all): setup=0s unit=38s …`.
    2026-10-06 전체 배터리 513초 가운데 jobdeck 122, vfs_lifecycle 117, unit 38, occupancy 36, vfs_render 32,
    density_stack 28, fit_budget 26, rust_renderer 14초이고 나머지는 11초 아래다(아래 빌드 수정 전에는 607초:
    준비 39, unit 80초).
  - 고르는 것으로 줄어드는 양(지난 커밋들의 파일이 부르는 게이트의 위 시간을 더한 어림): 문서만 0초, 게이트
    스크립트 하나 몇 초~30초, 뷰어만 약 3분, 그리는 경로 약 5분, 계획기 약 7분 반, 형식·인덱서 8분 반(전체).
    계획기·렌더러 변경은 느린 두 게이트(jobdeck, vfs_lifecycle)가 관련 게이트라 절반 넘게 남는다.
  - 운영: 커밋·푸시는 `--changed`가 통과하면 한다. 전체 배터리는 표가 전체를 부를 때와 사용자가 청할 때 돌린다.
- **linked work tree의 빌드(0.12.304).** renderd와 floe-index의 `build.rs`가 `.git/HEAD`를 지켜보는데, `git worktree
  add`로 만든 트리는 `.git`이 파일이라 그 경로가 없어 **빌드마다 두 바이너리를 다시 컴파일했다**(리뷰 트리와 게이트
  실행마다 16초). 이제 `.git` 파일이 가리키는 디렉터리의 HEAD와 `commondir`의 ref를 지켜본다: 바뀐 것이 없으면
  `cargo build` 0.01초.
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
- 컷 아래 공통 위상 패턴(2026-10-06, `CUT_DENSITY_DESIGN` §10.13):
  - `unit_render`의 `density_pattern_*`: 3×3의 두 위상(4/5점), 2×2(2점), 3×2(3점), 1×1의 빈칸,
    도형의 전체 차단 영역과 요약의 점만 차단하는 영역, 중복·하위 레이어 순회 순서, 배열/멤버 저장,
    정수 패닝 및 소수 위상의 타일·워커 일치, 1×1 사각형/다각형/경로 일치를 확인한다.
  - `density_stack`의 `pattern_checks`: 기본값=명시적 켬, `FLOE_RUST_DENSITY_PATTERN=off`의 밝기 복원,
    원본·스페클 구멍 보존, 불투명한 레이어 색, 타일64/워커1과 타일127/워커4의 바이트 일치.
    `full_shapes_first_checks`는 기본 패턴에서도 완전 가림이면 추가 계획·디코드가 0임을 확인한다.
    `nearly_full_pattern_checks`는 99.9%가 원본인 화면에서 남은 칸이 공통 위상의 금지 자리이면 계획·디코드가 0이고,
    허용 자리로 옮기면 밀도가 유지되며 타일64/127에서 같은지 검사한다.
  - `unit_render`의 `occupancy_summary_claims_speckle_holes_for_density`: occupancy의 실제 점유 내부는
    Solid/Speckle/Pattern/Clear 모두 차단하고 실제 빈 점유 셀은 밀도에 남긴다. 공통 패턴의 금지 자리 생략은
    `density_pattern_forbidden_holes_need_no_regions_and_keep_real_free_counts`로 정수 pan과 실제 빈 공간 비율을 확인한다.
  - `original_union_mask_matches_solid_fill_*` 두 단위 검사는 동일한 계획을 Solid로 그린 픽셀과
    Speckle/Pattern/Clear의 원본 차단 마스크를 직접 대조한다. 사각형·다각형·경로, Grid/Pts,
    회전·반사 계층, 가득 찬 타일 조기 종료와 deferred 배치를 포함한다.
  - 직접 마스크 탐색·레이어 단계(`CUT_DENSITY_DESIGN` §10.15): `unit_vfs`는 가려진 하위 트리 생략,
    살아 있는 합성 블록의 모든 기여 유지, 공유 셀의 회전·반사, 변환 상한과 예산의 보수적 복귀를 검사한다.
    `unit_render`는 프레임 비트마스크 내보내기의 타일/워드 경계, 위 밀도 뒤의 마스크 갱신, 완전 가림의
    남은 단계 생략, 두 번째 단계 중 취소와 워커 종료를 검사한다.
    `density_stack`의 `planner_mask_checks`는 마스크만 끈 대조군과 픽셀 일치 및 탐색 감소,
    `staged_density_checks`는 기본값의 공동 계획 유지와 명시적 `FLOE_RUST_DENSITY_STAGES=on`에서
    상위 8개 밖의 레이어까지 순서대로 그린 뒤 아래 계획을 생략하는지 확인한다.
  - `rust_renderer`: `density_plan2`는 `pattern` 뒤의 `mask_tests/mask_pruned/mask_fallbacks/stages`까지 44개 값.
    이전 39/40개 응답은 생략된 필드를 0으로 읽는다.
    상태줄은 백엔드 값으로 `pattern, cover x2` 또는 `bright x2`를 표시한다.
    아래의 기존 밝기·면적 보존 검사는 패턴 스위치를 끈 대조군으로 유지한다.
- 점유 밀도로 그리는 2패스(0.12.308 design.ovo 비트, 0.12.309 design.ovs 자체 비트; 0.12.317부터 기본,
  `FLOE_RUST_DENSITY_OCC=off`가 킬 스위치, `CUT_DENSITY_DESIGN` §10.16). `density_stack`의 `main()`은 계획 경로로 만든
  검사들을 위해 `off`로 고정하고, 점유 검사가 켠다:
  - `density_stack`의 `occ_density_checks`:
    - 픽스처: 1/0 DOT 셀(0.5 µm 정사각형)을 3 µm 간격 66×33 배열로 둔다. TOP 자신의 3/0에는 80 µm 상자와
      그 옆 0.4 µm 정사각형 26×26(3 µm 간격)을 둔다. 컷을 넘는 도형과 컷 아래 도형이 한 페이지에 있다.
      간격이 2 µm이면 1 µm/px에서 모든 멤버가 점 체커의 같은 반대 자리에 놓여 walk·점유 모두 0이 된다.
    - `floe2 index`가 design.ovs를 기본으로 만든다(0.12.316, 로그 `[vfs] ovs design.ovs: `).
      `--no-page-occupancy` 캐시는 만들지 않고 `[vfs] ovs: none`을 낸다.
      `--force --no-ovs`로 다시 색인하면 이전 design.ovs가 지워지고 새로 만들지 않는다.
      `jobdeck`의 `JobdeckIndexLodTests`: `floe2 index deck.jb`로 만든 소스 캐시에는 design.ovs가 없다.
    - 보통 색인(design.ovo 없음)에 `floe-index ovs --um 1`로 design.ovs를 만든다. 큰 페이지 1개를 디코드한다.
      표준 오류에 단계 줄(`index open`, `grid 1 um`, `decoded`, `walked in`, `settled in`, `written in`)이 나온다.
      `FLOE_OVS_PROGRESS_S=0`으로 다시 만들면 긴 단계 줄(`listing the pages to decode: `, `settle: 0/`,
      `write: layer 0/`)도 나온다(0.12.314).
      `--jobs 1`로 다시 만들어도 바이트 동일하다. `--no-page-occupancy` 캐시는 design.ovb가 없어 exit 1로
      거절하고 design.ovs를 만들지 않는다.
    - 스위치를 지운 워커(기본)의 프레임이 켠 워커와 바이트 동일하고 `occ_layers` 2다(0.12.317).
    - 400×200 px, 1 µm/px, full depth(셀 1 px): `density_plan2`가 `occ_layers` 2, `occ_cell_nm` 1000,
      regions 0, nodes 0이다. 1/0 점은 배열 범위 안 1/0 색으로만, 3/0 점은 정사각형 범위 안 3/0 색으로만
      켜진다. 그 밖에는 1패스 상자만 있다. 점 수는 DOT 556 px(walk 471), 정사각형 118 px(walk 169)이다.
      타일 64/워커 1과 타일 127/워커 4가 바이트 동일하다. 레이어를 하나씩 결합한
      `FLOE_RUST_DENSITY_OCC_THREADS=1`도 바이트 동일하다(0.12.310).
    - depth 0: TOP 자신의 정사각형만 같은 수로 남고 한 단계 아래 DOT은 없다(`occ_layers` 1).
    - 0.25 µm/px(셀 4 px > `FLOE_RUST_DENSITY_OCC_PX` 2), `FLOE_RUST_DENSITY_OCC_MB=0.0001`,
      design.ovs가 없는 캐시는 계획 경로로 그리며, 스위치를 끈 프레임과 바이트 동일하다.
  - `density_stack`의 `occ_cost_checks`(0.12.312, 비용 리뷰):
    - 3/0 상자로 다 덮인 70×70 px 뷰: `occ_made` 0, `occ_layers` 0, 프레임이 walk와 바이트 동일하다.
    - `FLOE_RUST_DENSITY_OCC_MB`를 레벨 0 레이어 하나의 1.5배로 둔다. 같은 뷰에서 1/0, 3/0, 1/0을 차례로 켠다.
      - 기대: `occ_made`가 [1, 1, 1](첫 레이어가 둘째 때 밀려나 다시 만들어짐), `occ_cache_kb`가 매번 상한 안이다.
      - 프레임은 기본 상한의 프레임과 바이트 동일하다.
  - 단위: render-core `the_layers_stop_where_a_newer_frame_asks`(1·3스레드, 멈춤 신호가 있으면 아무것도 만들지 않음).
  - `rust_renderer`: `density_plan2` 48개 값(…/occ_layers/occ_cell_nm/occ_made/occ_cache_kb). 46개 응답은 뒤의 둘을
    0으로 읽고, 47개는 거절한다. 상태줄은 `…, 12 made)`이고 로그 줄은 `…, 12 made; cache 84.0 MB)`이다.
  - `density_stack`의 `occ_review_checks`(0.12.311, b4ca42e 리뷰 재현):
    - 픽스처: 1/0에 3.5 µm 사각형 40×20(8 µm 간격), 2/0에 0.4 × 32 µm 선 32개(2 µm 간격). 따로 0.4 µm 정사각형
      하나뿐인 TOP. `--um 1`(작은 TOP은 자동)로 만든다.
    - 사각형, 1.5 µm/px(컷 4.5 µm): `occ_layers` 1, `occ_cell_nm` 2000(6 µm 컷의 1단계). 2,549 px로 walk(3,839)의
      0.25~4배 안이고, 밀도 끔은 0 px다. 이전 바이너리는 1.25~1.9 µm/px에서 0 px였다.
    - 선, 1 µm/px: `occ_layers` 0, 프레임이 walk·밀도 끔과 바이트 동일하다(429 px). 이전 바이너리는 673 px였다.
    - 작은 TOP: version 3 파일에 깊이 0 평면이 있고 셀이 켜져 있다. 이전 바이너리는 평면이 없었다.
    - design.ovs의 사각형 1단계 비트를 0xff로 덮으면 프레임이 walk와 바이트 동일하다(`occ_layers` 0).
    - design.ovp를 16 B로 자른 사본은 `floe-index ovs`가 exit 1(`page`)로 끝나고, 사본의 design.ovs가 그대로다.
  - 단위: vfs `a_page_counts_its_shapes_by_their_larger_side_and_a_path_with_its_ends`:
    - 3.5 µm 사각형은 class 1, 0.4 × 32 µm 선은 어디에도 없다.
    - 확장 1.5 µm의 경로는 외곽 4 µm(class 1)로 셀 3~7을 덮는다(중심선 + 반폭이면 4~6).
  - 단위: render-core `the_level_is_the_finest_whose_cut_covers_the_frames`:
    - 1 µm 셀, 1.25/1.5/1.9 µm/px에 3 px 컷이면 1단계다.
    - 예산으로 올린 컷(10 µm/px, 60 µm)은 32 µm 셀(3.2 px)을 허용한다.
  - 단위: render-core `a_plane_that_will_not_read_is_an_error_not_an_empty_layer`.
  - `density_stack`의 `occ_root_checks`(0.12.315, root 뷰):
    - 픽스처: 215 × 200 µm TOP 아래 BLK(DOT 배열 40×20과 자기 3/0 정사각형)를 R90으로 1번, 같은 내용의 BLK2를
      대칭으로 2번 배치한다. 둘 다 TOP 박스의 25 % 이상이다. 작은 SML(DOT 5×5)도 하나 둔다.
    - `floe2 index`가 셀 파일 2개를 기본으로 만든다. `--force --no-ovs`로 다시 색인하면 design.ovs와 셀 파일이 모두 지워진다.
    - `floe-index ovs --um 1`: `roots=2`이고, design.ovs.<셀>이 BLK·BLK2용으로 생긴다(version 4, 배치 R90 (100, 0)과
      대칭). SML용 파일은 없다. design.ovs는 `--roots 0`과 바이트 같다.
    - BLK·BLK2 root 뷰(120×100 px, 1 µm/px): `occ_layers` 2, `occ_cell_nm` 1000, regions 0, nodes 0이다.
      DOT 점은 배열 범위 안에만, 정사각형 점은 그 범위 안에만 켜진다(셀 좌표, 그 밖 0).
      점 수는 BLK 196/44, BLK2 200/40 px이고, walk는 174/37, 174/38 px다.
    - depth 0: 자기 정사각형만 같은 수로 남는다.
    - SML root 뷰와, `--roots 0`으로 다시 만든 뒤(셀 파일 삭제, `removed`)의 BLK root 뷰는 walk와 바이트 동일하다.
  - 단위: vfs `occ_density::tests` 8개:
    - 크기 등급 `class_of`
    - 멤버 순회와 4,096 초과 퍼뜨리기
    - 격자: 자동 셀, 레벨 크기, 범위 밖 셀, 역수 셀과 정확한 셀의 일치
    - 셀 찍기, 2×2 OR 풀링, 묶음 점유 수, 저장 바이트
    - 희소 타일과 직접 찍기의 일치
    - 파일 쓰기·읽기와 magic·버전(version 1 포함)·잘림 거절. version 4(셀 파일)의 셀·배치 읽기도 본다.
    - 셀 몫 비트를 탑 격자 제자리에 밀어 넣기(`a_cells_bits_go_into_the_tops_at_its_place`, 단어 경계 걸침 포함)
  - 단위: render-core `occ::tests` 3개:
    - 요청 depth의 평면 OR과 점유 셀 수 가중 평균
    - 평균이 없는 레이어는 None
    - 레벨 선택: 1 px 이하 가장 거친 레벨, 상한 초과 시 2 px까지 더 거친 레벨
  - `rust_renderer`: `density_plan2` 46개 값(…/stages/occ_layers/occ_cell_nm). 이전 39/40/44개 응답은
    새 필드를 0으로 읽고 45개는 거절한다. 상태줄과 로그 줄은 계획 내역 대신
    `pass 2 by occupancy 12 ms (16 um cells, 449 layers)`를 표시한다. 밝기 계수가 있어도
    `cells by box`/`cell cover`는 표시하지 않는다(0.12.310).
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
    - 0.12.300(셀의 덮임, 노드 표본, 항목 나눔) `hier_checks`: `hier_layouts`를 1 µm/px, 2/0만, g = 2로 본다.
      - 노드: 0.05 µm 사각형 160×160개(0.25 µm 간격, 4 px 노드에 배치 256개).
        - TOP의 도형으로 두면 알파 합이 g × 면적의 12 % 안이다(129 / 128).
        - 사각형마다 셀을 만들어 한 번씩 놓으면 20 % 안이고(128) `by_nodes`와 `node_sampled`가 0보다 크다.
        - `FLOE_RUST_DENSITY_NODE_SAMPLE=off`: 5배가 넘는다(1,579). `node_sampled`는 0이다.
      - 덮임: 1 µm 셀(1/0 상자) 안의 0.2 µm 2/0 사각형.
        - 60×60 배열, 2,000개 점 리스트, 한 번씩 놓은 셀 500개가 각각 g × 멤버 × 0.04의 15 % 안이다
          (290 / 288, 161 / 160, 40 / 40). `cell_cover`는 1, `cover_cells`는 1 이상이다.
        - `FLOE_RUST_DENSITY_CELL_COVER=off`: 각각 4배가 넘는다(3,600 / 3,725 / 954). `cell_cover`는 0이다.
      - 나눔: 0.5 µm 사각형 셀 48×48개(1 µm 간격, 1/4 덮임).
        - 안쪽 픽셀의 알파 평균이 0.5의 0.06 안, 편차가 평균의 0.35 미만, 최소 0.05 초과, 최대 0.8 미만이다
          (0.48, 0.23, 0.13~0.55).
        - `FLOE_RUST_DENSITY_ITEM_SHARE=off`: 편차 0.5 초과, 최소 0, 최대 0.99 이상이다(0.75).
      - 배열: 1.2 µm 셀(0.6 µm 사각형)을 1.5 µm 간격으로 40×40.
        - 4 px 블록 평균이 0.32의 0.03 안, 블록 편차 0.12 미만이다(0.06). 스위치를 끄면 0.18 초과다(0.26).
      - 어댑터 계약: `density_plan2`는 39개 값(…/stood_in/cell_cover/cover_cells/node_sampled)이다. 상태줄 테스트:
        `bright x2, cells by box`와 `bright x2, cell cover`.
      - 단위는 SPEC-PLANNER의 여섯 개다. 세 스위치를 모두 끈 프레임이 0.12.299와 바이트 단위로 같은 것, 기본 프레임이
        실행·스레드 수·미리 계산 여부와 무관하게 같은 것은 게이트가 아니라 측정으로 확인했다(11개 뷰, 6개 뷰).
    - 0.12.299(점유 격자 먼저, 빠진 페이지의 대체, 항목의 소수부):
      - `first_checks`: `first_layout`의 0.6 µm 사각형(0.4 µm/px에서 1.5 px)을 밝기로 본다.
        - 기본: 2패스가 디코드한 페이지 0, 격자로 놓은 페이지 1 이상. 성긴 구역의 알파 합이 g × 면적의 15 % 안이다
          (4,643 / 4,500). 마당은 색의 0.5다.
        - `FLOE_RUST_DENSITY_OVB_FIRST=off`: 디코드 1쪽 이상. 사각형이 덮은 픽셀이 색에서 멈춰 합이 0.62~0.85배다
          (3,305). 마당은 0.30~0.45다. 16 px 칸으로 둘의 평균 차가 평균의 0.4 미만이다.
        - 예산 1 MB에 고정 예약 128 KB(위 스위치는 끔): `stood_in` 1 이상, 디코드 0, 알파 합이 다시 g × 면적이다.
        - 거기에 `FLOE_RUST_DENSITY_STAND_IN=off`: 켜진 픽셀이 없다.
      - `sums_checks`: 0.32 px 셀 2,000개(1.64단위)를 1 µm/px로 본다.
        - 셀마다 다른 셀로 한 번씩 놓으면(단일 배치) 알파 합이 g × 면적의 6 % 안이다(407 / 410. 0.12.300부터 항목 나눔으로 408).
          `FLOE_RUST_DENSITY_BRIGHT_SUMS=off`면 1/1.64배다(250).
        - 한 셀을 2,000번 놓으면(점 리스트) 합이 같고 어느 픽셀도 색의 0.5를 넘지 않는다(0.20). 끄면 합이 0.85배
          미만이고(296) 색 그대로인 픽셀이 있다.
      - 어댑터 계약(`rust_renderer`): `density_plan2`는 36개 값(…/bright_milli/stood_in)이다. 상태줄 테스트:
        `3 pages left out, 7 pages by occupancy instead`.
      - 단위는 SPEC-PLANNER의 다섯 개다. 마지막 예산 검사에서 빠지는 경로는 게이트로 만들지 못했다. 단위의
        `stand_in_left_out(Some)`과 라우팅 칩 2배 확대 측정(172쪽)이 확인이다.
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
      - 0.12.303(2026-10-06) `full_shapes_first_checks`: 원본으로 가득 찬 타일이 plane 0을 건너뛰어도 빈 영역은 0이다.
        채움 사각형·맞닿은 1 px 선(스페클 레이어)·큰 스페클 사각형, 타일 64/512에서 스택 끔과 바이트 동일하고
        2패스 계획·장면 준비·추가 디코드·`cell_cover` 준비가 0, `covered`는 화면 전체다. 절반만 덮으면 그 절반은
        그대로이고 빈 절반에는 밀도가 나온다. 단위 `shapes_first_full_tiles_leave_no_density_demand`는 최상위 평면
        1/2/3개, 밝기 끔/켬, bin/walk, 타일·워커 조합도 확인한다. 수정 전에는 채워진 타일 전체가 최상위 밀도 영역으로
        나와 실패한다.
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
  0.12.301 `refit_checks`(실칩 2026-10-05: 두 레이어만 켜고 밀도를 끄면 `decoded generation budget exceeded`).
  먼저 레코드 벡터의 여유다. 0.6~1.0 µm 상자 60만 개만 있는 레이아웃(`layout_plain`: 페이지당 레코드 3.3만 개 —
  파서가 읽은 그대로는 벡터가 절반쯤 차서 부과가 추정의 1.12배)의 가운데 400 µm를 26 MB 예산으로 본다(네 페이지:
  추정 25.7 MB, 읽은 그대로 28.9 MB).
  - 읽은 그대로(`FLOE_RUST_DECODE_SHRINK=off`) + `FLOE_RUST_BUDGET_REFIT=off`: 0.12.300의 오류
    (`28883786 > 27262976`). 오류가 안 나면 레이아웃이 더는 재현하지 못하는 것이라 게이트가 실패한다.
  - 기본: 맞춘 계획이 통째로 든다(`fit_refits` 0, `fit_scale` 0, `fit_over` 0, 예산을 넘긴 페이지 없음, 4쪽,
    상주 15.8 MB).
  - 읽은 그대로(`FLOE_RUST_DECODE_SHRINK=off`): 프레임을 한 번 다시 계획해 그린다(`fit_refits` 1 이상, `fit_scale`
    1000 초과 — 실측 1,158, 3쪽, 상주 20.7 MB ≤ 예산). 같은 뷰를 다시 청하면 `fit_refits` 0, 같은 픽셀이다.
    기본보다 페이지·켜진 픽셀이 적고 상주는 크다.
  - 그 워커에서 밀도 스택을 켠 프레임은 줄여 계획하지 않는다(`fit_scale` 0, `fit_refits` 0). 다시 밀도를 끄면
    처음과 같은 픽셀, 같은 `fit_scale`이다.
  - 밀도 스택을 켠 프레임은 벡터를 줄여도 같은 그림이다(기본 워커의 프레임 = 읽은 그대로 워커의 프레임, 바이트
    단위 — `density_as_read`).
  - 여백(bg): 한 페이지 안의 200 µm 뷰포트와 네 페이지에 걸친 그 여백(같은 배율, 변마다 두 배 픽셀).
    - 읽은 그대로 + `FLOE_RUST_BUDGET_REFIT=off`: 뷰포트가 그려지고 여백이 오류다(0.12.300).
    - 기본: 뷰포트는 같은 픽셀, 여백도 그려진다(4쪽, `fit_scale` 0).
    - 읽은 그대로: 여백이 `dropped`(reason `budget`)이고, 다음 뷰포트 프레임이 `fit_scale` 1000 초과·`fit_refits`
      0·같은 픽셀이며, 그 뒤의 여백은 프레임이거나 `dropped`(reason `fit`)다 — 오류가 아니다(실측: 뷰포트가 통째로
      드는 곳에서 여백이 솎아야 해서 reason `fit`).

  다음은 공유 목록이다. 상자 크기 40가지를 같은 30,000곳에 둔 레이아웃(`layout_shared`: 페이지의 40개 레코드가 반복
  목록 하나를 공유)을 4 MB 예산으로 본다.
  - 기본: 맞춘 프레임이 그대로 든다(`fit_refits` 0, `fit_scale` 0, 상주 1.7 MB, 켜진 픽셀 있음).
  - 부과를 0.12.300대로(`FLOE_RUST_CHARGE_SHARED=off` + `FLOE_RUST_DECODE_SHRINK=off`): 프레임을 다시 계획해 그린다
    (`fit_refits` 1 이상 — 실측 2번, `fit_scale` 1000 초과 — 4,463, 상주가 예산 이하). 같은 뷰를 다시 청하면
    `fit_refits` 0, 같은 `fit_scale`, 같은 픽셀이다.
  - 거기에 `FLOE_RUST_BUDGET_REFIT=off`: 종전 오류다(`7265394 > 4194304`).
  - 0.12.300의 부과로 1 MB(페이지 하나보다 작음): 오류가 아니라 예산이 담는 만큼 그린 프레임이고
    `over_budget_pages`가 1 이상, `fit_over` 1이다.
  - 어댑터 계약: 프레임 줄의 `fit_scale`·`fit_refits`, 상태줄 `x2 to fit budget, STILL OVER, pages x1.18 their estimate`.
  0.12.302 `probe_checks`(실칩 2026-10-05: 새 배율의 첫 프레임이 느리고 원인이 other): 합성 칩(48 MB)의 첫 프레임
  21개 — 칩 전체, 그리고 다섯 곳에서 1/2·1/4·1/8·1/16 크기의 창, 저마다 다른 배율 — 를 기본 워커와
  `FLOE_RUST_FIT_PROBE_SUMMARY=off` 워커로 그린다.
  - 프레임마다 픽셀, 계획한 페이지 수, 맞춤 통계(`fit_pct`·`fit_thin`·`fit_full_pct`·`fit_none_pct`·`fit_fixed`·
    `fit_redecided`·`fit_over`)가 같다. 맞춤이 솎은 프레임이 3개 이상이어야 한다(실측 9개).
  - 둘 다 `fit_probe_ms` > 0이고 `fit_probe_walk`는 기본 False, 스위치를 끈 쪽 True다.
  - 이미 결정한 배율을 다시 청하면 `fit_probe_ms` 0, `fit_fixed` 1이다.
  - 어댑터 계약: 프레임 줄의 `fit_probe_us`·`fit_probe_walk`, 단계 합에 든 사전 계획(`other_ms` 15 → 12), 상태줄
    `… + 3916 draw + 1054 fit probe + 639 other`(100 ms 미만은 로그 줄에만), 로그 줄 `, fit probe 28.0ms (walk)`.
  0.12.318 `top_first_checks`(실칩 2026-10-07: 7.59와 14.367을 함께 켜면 7.59만, `none below x28.2`). 1패스의 예산은
  위 plane부터다(SPEC-PLANNER). 위의 검사들은 크기 순으로 만든 것이라 `main()`이 `FLOE_RUST_FIT_TOP_FIRST=off`로
  고정하고, 이 검사만 켠다(기본).
  - 두 레이어(`layout_pair`: 7/59 4 µm 사각형 4,000개, 14/367 1.5 µm 4,000개, 레코드 하나씩 — 레이어마다 약 0.8 MB의
    한 페이지)의 400 µm 뷰, cut 3 px, 예산 1 MB(1패스 약 0.9 MB).
    - 각 레이어만 켜면 통째로 그려진다(색으로 센 픽셀: 7/59 97,216, 14/367 21,060).
    - 함께 켜면 맨 위 14/367이 혼자일 때와 같고 7/59는 0이다. `fit_ranked` 1, `fit_layers_whole` 1, 끝난 레이어 없음,
      `fit_layers_out` 1, 상태줄 `top 1 whole, 1 left out to fit budget`.
    - 다음 프레임은 기억한 결정으로 같은 픽셀이다(`fit_fixed` 1, `fit_redecided` 0).
    - `FLOE_RUST_FIT_TOP_FIRST=off`는 종전 맞춤이다: 7/59가 혼자일 때와 같고 14/367은 0.
  - 스페클 구멍(`layout_over`: 7/59의 20 µm 사각형마다 14/367의 30 µm 사각형이 덮음, 1 GB): 둘을 함께 켜면 7/59만
    켰을 때 칠한 픽셀 가운데 일부가 7/59 색으로 남는다(구멍; 실측 108,484 px 중 6,172), 나머지는 14/367 색이다.
  - 합성 칩 전 레이어(48 MB): 광역뷰가 위 plane부터 맞춰진다(`fit_ranked` 1; 온전한 레이어 + 끝난 레이어 + 빠진 레이어
    = 전 레이어, 실측 168 + 0 + 281). 가운데 → 여백(전체) → 가운데 순서로 결정이 기억·적용되고 두 가운데가 여백의
    가운데와 같다.
  - 단위 vfs `the_budget_fit_keeps_the_top_plane_first`(예산별로 위 plane부터 완전·솎음·빠짐, 걷기가 닿지 않는 plane을
    수집하지 않음, 기억한 결정의 재적용·통째로 듦·다시 결정, 맨 위 plane만 홀로 사다리, 순위 없으면 종전 순서),
    `a_plane_past_the_ladders_reach_is_left_out_and_the_planes_above_kept_whole`(사다리 끝에서도 넘치는 plane은 빠지고
    결정은 그 위 plane들까지, 다시 적용해도 같음), 0.12.319(777ee08 리뷰)
    `a_page_two_working_cells_hold_counts_once_against_the_planes_above`(두 깊이에 놓인 셀의 페이지를 한 번만 세어 예산이
    담는 프레임은 네 페이지 전부, 세 페이지 예산은 위 plane만이고 결정은 "전부"가 아님 — 두 번 세면 실패),
    `a_decision_applied_again_says_what_its_plane_lacks`(아래 plane의 바닥 하한, 맨 위 plane의 올린 컷으로 내린 결정을
    다시 적용해도 페이지와 상태 — 온전·잘림·빠짐 레이어 수, `fit_none_pct`, `fit_pct` — 가 같음; `lacks`를 빼면 실패;
    0.12.320(15c464d 리뷰): 남긴 1600(등급 10)과 처음 뺀 200(등급 7) 사이 등급이 빈 위 plane도 다시 적용해 `none below
    x5.12` 그대로 — `below` 대신 결정의 등급 − 1을 쓰면 `x20.5`로 실패; 0.12.321(7801d4c 리뷰): 다시 적용하는 계획이
    그 plane을 결정의 등급부터만 모음 — 1600 한 쪽, `below`부터 모으면 200까지 두 쪽으로 실패).
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
| C1~C10 | `tools/validate_cell_tree.py` (`cell_tree`, `render`·`indexer` 별칭, 약 20초) | 셀 트리(SPEC-VIEWER §8c, design.ovh): 계층 픽스처(블록 3배치 = 평·90°·미러, 블록 안 3×2 격자, 탑 직접+깊은 배치 셀, 도형 없는 셀, 미배치 셀)를 KLayout으로 대조 — C1 `floe2 index`가 design.ovh를 쓰고 `floe-index hier --check` identity=ok·타 캐시 파일 거부(rc 1), C2 모든 셀의 서로 다른 자식과 멤버 수 = KLayout 인스턴스 배열 size 합·leaf 표기, C3 탑 아래 인스턴스 수 = 탑다운 곱셈 합(탑 1·orphan 0), C4 cell_find 부분일치/글롭 대소문자 무시·이름순·total+limit, C5 cell_bbox 탑 직계 = 인스턴스 박스 정확 합집합·깊은 셀 = 블록 범위 상위집합(approx=1)·orphan/도형 없음 None, C6 cell_insts 뷰 안 박스 = KLayout 전개 탐색(회전·미러·격자·불규칙 반복)·cap → more=1·탑 = 자기 박스, C7 파일 삭제 시 소형 캐시 메모리 요약(파일 안 씀)·`FLOE_RUST_HIER_INLINE_PLACES=0`이면 code=nohier·`--hier-only` 뒤 **같은 데몬**이 집어 듦·캐시 없는 소스 거부, C8 뷰 루트(`root=BLK`) 프레임 == BLK를 탑으로 한 별도 레이아웃(KLayout copy_tree) 프레임 바이트 동일(3뷰)·탑 뷰와는 다름·루트 아래 cell_bbox(직계 정확·6)·cell_insts(KLayout BLK 탐색 12)·루트 위 셀 0·테이블 밖 루트 거부·보이는 레이어가 없는 루트(VIA에 2/0만) = 검은 프레임(오류 아님)·1/0이면 그려짐·density stack 켬 == 끔, C9(0.12.296) 파일이 이름만 둔 빈 레이어(3/0 NOTHING)만 켬 = full depth(프레임 켬·끔)·depth 0 프레임 끔은 검은 프레임, depth 0 프레임 켬은 1/0을 켰을 때와 같은 depth 밖 외곽선 픽셀만(오류 아님, 0.12.295는 `invalid plan: top … is missing`)·density stack 점 켬도 같음·1/0이면 그려짐, C10(0.12.322; 현장 EBEAM 파일: Calibre는 3.0·3.300, floe는 3.1·3.2까지 목록) 색인은 LAYERNAME만 있는 3/1·3/2를 그대로 두고 stored_shapes 0(텍스트만 있는 5/0은 1), 뷰어 목록(`gui.listed_meta`)은 3/0·3/300·5/0만·`FLOE_EMPTY_LAYERS=show`면 전부 — 패널 행·그룹(3.0 아래 3.300만)·visible 집합·`_layers_arg`(모두 켜면 None)·잡덱 표 그대로·안내 줄·`_apply_cache`가 이 목록을 씀은 `rust_renderer`의 `LayerListTests` |
| K1~K7 | `tools/validate_index_lock.py` (`index_lock`, `indexer` 별칭, 약 30초; 2026-10-09, app 0.12.323) | 색인 잠금(CACHE-NAMING §5, SPEC-INDEXER §4.5): 게이트가 `floe/indexlock.py`로 다른 사용자처럼 잠금을 쥔다(`FLOE_REVIEWER=ws_kim_01`) — K1 build를 쥐면 `floe-index vfs`(본 빌드·`--occupancy-only`·`--coverage-only`·`--representatives-only`·`--frontier-only`)·`hier`·`ovs`·`floe2 index --force`·`--occupancy-only`가 3초 안에 75, 쥔 쪽 이름·pid·what 그대로, 캐시 파일 바이트·mtime 그대로, 래퍼의 임시 파일 정리가 남의 `design.ovo.tmp`를 남김(풀면 지움), K2 `--hold-at locked`로 실제 두 빌드 — 둘째 75(첫째 pid), 첫째 완료·vfsd 통과, K3 통째 재색인 중 renderd open = `code=locked` 원문(`_` 보존)·`floe2 info` 75 "being indexed"("no VFS cache" 아님)·뷰어 `_index_busy`, 바쁜 소스가 있는 잡덱은 그 소스를 ledger("being indexed by ws_kim_01")로 빼고 열림, K4 renderd가 연 동안 본 빌드·`floe2 index --force` 75(renderd의 등록 pid), 닫으면 빌드, `--hold-at committed`(커밋 뒤 ovs 단계)에는 renderd가 열림, K5(0.12.325, 9378c6d7 리뷰) renderd가 연 채 없는 design.ovh·design.ovo를 만드는 `hier`·`--occupancy-only`는 통과하고 있는 것을 바꾸면 75(쥔 쪽 이름), 바꾸는 실행이 `--hold-at locked`로 멈춘 동안 들어오는 renderd·Python 독자는 "being indexed"로 거절, 다른 덧붙임이 build를 쥐면 75, 캐시나 그 폴더를 심볼릭 링크로 가리킨 재빌드·`hier`도 75(키가 같음), umask 077에서도 등록 0644, 이 계정이 읽을 수 없는 등록(0000)도 사용자로 셈(`someone on other.host, pid 4242`) — 셋 다 9378c6d7 코드로는 실패, K6 래퍼가 "--force" 대신 "being indexed", 잡덱 색인이 바쁜 소스를 `BUSY`로 세고(`1 built, 0 failed, 1 busy, 0 kept`, 종료 75) 다른 소스는 빌드, 잠금을 쥔 동안 구 이름 `<src>.floe` 이전을 건너뜀(풀면 이전), K7 SIGKILL된 renderd의 등록은 사용자가 아님(다음 빌드가 지움), `FLOE_LOCK=off`(`.floe-lock` 없음·쥔 잠금 무시), 0555 폴더는 잠금 없이 열림, 잠금 폴더 권한(0775 → 01775/0664, 0755 → 0755/0644), 확인 3스레드가 빌드를 막지 않음, `ulimit -n 32`에서 24소스 덱이 열림(`without their locks`), `_index_modal`이 자기 프로세스 그룹(`start_new_session`·`killpg`). 잠금을 빼면 16개 중 11개 실패(나머지 다섯은 잠금 없이도 성립해야 하는 것). 0.12.324 `PackLockTests`: K8 DRC 팩 — 리뷰(`drc.load_db`)가 쥔 팩의 재팩 75(쥔 pid), 닫으면 팩, 팩의 build를 쥐면 `floe-index drc` 75·팩 바이트 그대로·`drc.load_db` `Busy`·`floe2 drc` 75, 읽을 수 없는 .db의 실패(rc 1)가 게시된 팩·임시 파일을 남기지 않음, K9 룰 사이드카의 build를 쥐면 `floe-index svrf` 75·rules.json 바이트 그대로, 풀면 새로 씀 — 넷 다 수정 없이는 실패. drc_ice D2는 재팩 전에 열어 둔 `ice`·`auto`를 닫는다(리뷰가 쥔 팩은 재팩되지 않으므로) |
| gtk_view | `tools/validate_gtk_view.py` (P4c 2026-10-10, 0.12.335; P4d 0.12.337; P4e 0.12.338, 약 8초) | `floe2 gtk-service`의 뷰 채널(공유 `ViewController`, `floe/gtkservice.py` `ViewSession`): valmini를 열고 이동(16 px)·밀도(첫 라운드 `density_round=1` 최종 아님, 이어서 최종, 라운드 증가, 픽셀이 바뀜)·여백(`viewport`, 덮인 이동은 새 프레임 없이 `crop_hits`)을 확인한다. 프레임마다 같은 상자·크기·정책의 `RustRenderWorker` 프레임과 바이트가 같아야 한다. 닫힌 뷰의 폴더는 사라진다 P4d: 채널의 snap·pick·셀 트리·클립 = 어댑터의 답(클립 OASIS 바이트 일치). 이어서 `floe/gui.py` `Viewer`를 프로세스 안에서 실제 창으로 띄워 컨트롤러 루프(기본)를 확인한다: 첫 프레임이 창 할당 뒤 뷰어의 뷰와 같고, 확대·이동·밀도·회색조 프레임이 그 뷰·정책(뷰어의 depth 포함)의 어댑터 프레임과 바이트가 같다. 서비스가 보낸 perf 줄 = 프레임 보고에 대한 Python `perf_status`, 보고에 어댑터 결과의 키가 다 있고 아래 막대가 그 짧은 줄이다. 밀도가 켜진(불완전한 장면) 프레임에서도 snap이 어댑터와 같은 답을 준다(`incomplete=ok`). 여백을 켠 뷰어의 여백 안 이동은 잘라 쓰기(`crop_hits`, 새 제출 없음). `FLOE_GTK_LOOP=legacy`이면 Python 루프가 그린다 P4e: 뷰어의 내비게이션 13가지(확대, 휠, 화살표, 밴드, 미니맵 클릭, goto, fit)가 컨트롤러로 가서 뷰어의 Python 계산과 같은 뷰가 되고, 미니맵의 자리와 바탕(전체 + 깊이별)이 서비스(`view_minimap`)에서 와서 뷰어의 굽기와 바이트가 같다 뷰어는 pycairo를 import할 수 없게 막은 채(대상 호스트와 같게) 돌며, 숨긴 레이어 행에 취소선(행 전체 한 줄, 행 크기 이미지)이 보이고 색·fill 팔레트에 이미지가 있다(0.12.339) renderd 시작을 1초 늦춘 서비스에서 뷰가 열리는 중에 보낸 셀 질문(`cell_sources`, `cells`)도 뷰가 열린 뒤 답을 받는다(0.12.340; 전에는 `view is still opening`으로 거절되어 큰 칩의 셀 트리가 `loading…`에 멈췄다) |
| floe2 제품 경계 | `tools/validate_floe2.py` (P3 2026-10-10, 0.12.333) | 제품 패키지 `floe/`는 뷰어뿐이다: `python -m floe`로 실행되지 않음, 뷰어 정체성이 `FLOE_PRODUCT`(없음·floe·floe2)와 무관하게 floe2, KLayout 밀도 커버리지 상태 없음, `git ls-files floe` = 번들의 `FLOE2_PRODUCT_FILES`, floe/의 모든 import(함수 안 지연 import 포함, AST)가 뷰어 모듈 밖·`floe_oracle`·`klayout`을 가리키지 않음, `floe2 gtktest`가 뷰어 진입점에 닿고 `--help`에 있음, 동결 셸은 `python -m floe_oracle`(KLayout/floe 정체성), KLayout 개발 번들이 `floe_oracle`을 싣고 `floe` 런처가 `-m floe_oracle` |
| floe2 Rust CLI | `tools/validate_floe2.py` `validate_rust_cli` (2026-10-09, 0.12.327, P1a) | Rust `floe2`(rust/floe2 + 공유 floe-app-cli): `--version` = 앱 버전·floe-index·renderd, index·info·probe·render·clip이 Python 없이(PATH 맨 앞의 기록용 `python`·`python3`가 비어 있음), 모르는 단어는 GTK 뷰어로(소스), `floe2 view --help`가 GTK 진입점(`python -m floe.gtkview`)에 닿음 |
| 제품 경로 | jobdeck `GuiSmokeTests`, `validate_floe2.py` (0.12.332, P2d) | floe2 뷰어의 실제 창(덱 세 번, 레이아웃 + `--drc`; P4d부터 컨트롤러 루프, 덱 한 번은 `FLOE_GTK_LOOP=legacy`)을 import 차단기(sitecustomize; 인터프리터의 원래 sitecustomize를 먼저 실행) 아래에서 연다. 차단 대상은 cache·cachepath·indexlock·drc·svrf·jobdeck·shots·fe_embed·render·viewport·coverage·view_policy·cli·service·klayout이다. 리뷰 사이드카를 서비스가 쓴다. floe2 게이트는 번들 목록(`FLOE2_PRODUCT_FILES`)이 뷰어가 불러오는 floe 모듈을 모두 담고 오라클 모듈은 담지 않는지 본다. 또 `floe/`의 어떤 코드도 GTK `draw` 핸들러를 연결하거나 cairo를 import하지 않는지 AST로 본다(대상 호스트에 pycairo가 없어 그런 위젯은 비어 보인다; 0.12.339). jobdeck `ViewerIndexArgvTests`: floe2 뷰어의 레이아웃 색인은 `floe2 index SRC --jobs 12`. jobdeck `DeckViewChannelTests`(P4d): 덱을 `ViewWorker`(컨트롤러)로 열어 처음 프레임, 레벨 헤드 색 바꾸기, 헤드의 fill과 다른 헤드의 선 굵기(repattern), 레벨 일부만 켜기가 제품 어댑터(헤드를 그 레벨의 datatype들로 펼친다)의 프레임과 바이트가 같다 |
| D12 | `tools/validate_drc_ice.py` (0.12.330, P2b) | GTK 뷰어의 DRC 리뷰(`floe2 gtk-service`, app-core `drc::desktop`) = IcePack: 작은 픽스처(검사·오류·CD 선분, 서비스가 쓴 상태를 Python이 읽음·카운터, 페이지·순위, 무작위 사각형 질의와 필터)와 gen_drcdb 자산(검사 60: 전체 오류, 무작위 waive 뒤 카운터·페이지·순위, 상한·필터·검사 부분집합 질의), 노트 파일 바이트 = IcePack 직렬화·다시 열기·지우면 파일 없음, waive 내보내기 바이트 같음·남의 파일 거절, 리뷰 중 재팩 75, 남의 사이드카 옮김, svrf 피연산자·사이드카. `rust/app-core`·`rust/app-cli`·`rust/floe2`·`floe/gtkservice.py`가 drc_ice를 부른다 |
| gtk-service | `tools/validate_jobdeck.py` `GtkServiceTests` (0.12.329, P2a) | `floe2 gtk-service`가 Python 구현과 같은 답: 덱 4개 × 뷰 3개 + 레벨 선택 2개의 열기(meta, 스펙을 값으로, props 소스, 소스 폴더, close가 스펙 폴더를 지움), `ready` = deck_ready(색인 안 된 복사본·빠진 소스), `level_rows`, 레이아웃 meta = `Cache.load()`(layerprops 색 포함)·props 행 = `load_layer_props`·소스가 바뀌면 current false, 뷰어(APP floe2)의 `_open_file_load`가 ServiceCache·덱 깊이 999·색인 없는 덱 "run: floe2 index", 그 캐시로 만든 워커가 `open deck=`. P2c(0.12.331): layerprops 읽기(주석·점 키·남는/빠진 열·잘못된 줄) = `fillpat.parse_layerprops`, 저장 = `format_layerprops` 바이트, 게시 경로·바이트 = `cache.save_shared_props`. `floe2` 게이트는 `gtk-service`의 version 응답을 본다 |
| CLI 게이트의 floe2 | 0.12.328 (P1c) | Python floe2 CLI(`floe2/` 패키지)를 지웠다. CLI를 부르는 게이트(`floe2`, `jobdeck`, `occupancy`, `cell_tree`, `index_lock`, `svrf`, `oasis_shapes`, `rust_renderer`, `representatives`, `fit_budget`, `sub_cut_box`, `shape_cut`, `write_once`, `layer_decode`, `area_true`, `density_stack`)는 `FLOE2_BIN`(기본 `rust/target/release/floe2`)을 부르고 출력 문구·종료 코드 계약은 그대로다. 게이트 쪽 변경은 셋: occupancy의 가짜 floe-index가 `--version`에 진짜처럼 답한다(Rust CLI는 실행 전에 색인기 버전을 확인한다), rust_renderer의 CLI 소스 쌍은 캐시를 링크가 아니라 복사로 둔다(공유 앱 계층은 심볼릭 링크 캐시 폴더를 열지 않는다), jobdeck `ViewerIndexArgvTests`는 뷰어의 잡덱 색인이 Rust floe2(`find_floe2`, `FLOE2_BIN`)를 부르는지 본다. `floe2` 게이트는 Rust 바이너리 기준으로 다시 썼다: 도움말(뷰어·공유 명령, legacy/coverage 옵션 없음), `floe2 view --help` = GTK 진입점(`FLOE_RENDERER=klayout`이어도 Rust), `--legacy`·`--coverage` exit 2 "unsupported index option", GTK 진입점이 floe2 정체성을 고름, 번들 런처가 floe2는 `runtime/bin/floe2`(`FLOE_GTK_PYTHON` = 런타임 python)·floe는 python으로 실행, 번들이 `bin/floe2`를 싣고 musl로 함께 빌드, 가짜 floe-index에 넘기는 기본 argv(`vfs SRC .SRC.ice --jobs 2 --no-lod`) |
| worker_client·cell_index | `tools/validate_worker_client.sh`, `cargo test -p floe-app-core cell_index` (2026-10-09, 0.12.326; docs/SHARED_APP_LAYER.ko.md) | 공유 Rust 앱 계층(feature/webui의 크레이트): worker_client = Rust renderd 클라이언트(worker-client)와 Python 어댑터의 프레임이 같음(valmini; raw·PNG·라벨·스타일·취소 반복), cell_index = app-core가 살아 있는 독자 옆에 design.ovh를 더하고 재실행에 유지하며 낡은·없는 캐시·busy·취소를 거절. `unit`(`cargo test --workspace`)에 app-core·worker-client·notices 단위 테스트가 든다 |
| perf_parity | `tools/validate_perf_parity.py` + `cargo test -p floe-app-core --test perf_parity -- --ignored`, `--lib view::perf` (P4b 2026-10-10, app 0.12.334; docs/SHARED_APP_LAYER.ko.md §7, 약 60초) | 뷰어 perf 줄(현장이 붙여 넣는 형식 — 사용자 계약)의 Rust 이식 `floe_app_core::view::perf`가 Python과 바이트 단위로 같다. `FrameReport` = rust_render.py `_emit_frame`의 라운드 누적과 결과 dict, `perf_status`·`fmt_count`·`occ_note`·`load_note` = gui.py. ① 합성: 손으로 만든 전체 프레임을 키 하나씩 값별(없음·None·0·fmt_count 단위 경계·round() 반올림 경계·거대값·소수)로 바꾼 것과 고정 시드 무작위 dict(정수·실수·bool 섞음, 밀도·fit·덱·라벨·축출·요약 변형, `FLOE_RUST_DENSITY_ONLY` 양쪽) 약 1.7만 줄, fmt_count 경계, 고정 시계의 `_load_note`. Python이 스스로 거부한(TypeError 등) dict는 세고 뺀다. perf_status·occ_note·fmt_count의 모든 줄에 닿는지 줄 추적으로 확인한다. ② 실제 렌더: MAIN01급 합성 칩(gen_main01_like 0.003)과 작은 잡덱을 어댑터로 `FLOE_RUST_RECORD`를 켜고 그린다 — detail 낮음·중간·높음·exact, depth full/0/2/5, 라벨·프레임 켬/끔, 같은 뷰·팬의 타일 재사용, 뷰포트를 둘러싼 여백(bg), 밀도 켬/끔(occupancy 밀도, `FLOE_RUST_DENSITY_OCC=off`의 plan), 48 MB 예산 fit, `FLOE_RUST_ROUND_PAGES=16` 다중 라운드, render probe, 덱 합성. ③ 어댑터의 `_submit_render`/`_emit_frame`에 꾸며 낸 frame 줄 400 세대(renderd 없이; 이상한 정수·`1_000`·`+5`, 밀도 값 39/40/44/46/48개와 틀린 개수, 깨진 place_walks; 필드 이름은 `_emit_frame` 소스에서 읽는다). ②③의 기록을 Rust `FrameReport`로 기록 순서대로 재생해 결과의 키·값 타입·값(float은 repr)과 perf 줄 둘이 같아야 한다. 기록이 큐에 나간 결과와 같은지도 본다(place_walks 제외: Python 결과는 세대 합계의 리스트를 공유한다). `FLOE_RUST_RECORD`가 없으면 어댑터 동작은 그대로다 |
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
