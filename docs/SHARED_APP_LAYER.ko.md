# 공유 Rust 앱 계층 (feature/jobdeck ↔ feature/webui)

사용자(2026-10-09): "웹버전과 현재 feature/jobdeck이 함께 개발되고 있어서 feature/jobdeck에서 추가되는
기능들이 웹버전에도 포팅되어야 하므로 ui를 제외한 주요기능들은 모두 rust로 개발되어야 함 … feature/jobdeck에서도
python을 사용하지 않도록 변경해줘."

결정(같은 날):
- GTK 뷰어(`floe/gui.py`)는 UI로만 Python에 남긴다.
- 그 밖의 기능은 feature/webui의 Rust 앱 크레이트를 가져와 두 브랜치가 같은 코드로 쓴다.
- 웹 서버(`rust/web`)·Electron(`electron/`, `desktop/`)은 webui에만 둔다.

## 1. 크레이트

| 크레이트 | 하는 일 | 두 브랜치 |
|---|---|---|
| oasis·ovm·tiler·vfs·cli(floe-index)·render-core·renderd·render-cli·dbg·build_support | 파서, 색인, 계획기, 렌더러 | 같은 판(P0에서 webui 판을 가져와 rustfmt까지 맞춤) |
| `app-core` | 앱 정책: 데이터셋·잡덱(파서·소스·계획·스펙·뷰)·DRC(팩·ASCII·필터·리뷰 사이드카)·레이어 속성·캡처·clip·색인 판단·셀 인덱스. HTTP·GTK·Python과 무관 | 공유 |
| `worker-client` | renderd의 Rust 클라이언트(`rust_render.py`와 같은 프로토콜, `scene_gen` 고정 질의) | 공유 |
| `notices` | 배포 고지 목록 | 공유 |
| `app-cli` | 웹이 없는 CLI 명령: index·info·render·probe·clip·jobdeck·drc·svrf·fe-embed·selfcheck. webui `rust/app/src`에서 웹 명령을 뺀 것. 프로그램 이름과 입력 오류의 종료 코드는 실행 파일이 정한다(`Host`의 `name`·`input_error_exit`, `floe_app_core::set_program`; 기본 floe2-web·2) | 공유(P1a; webui의 `floe2-web`이 이 크레이트를 쓰도록 바꾸는 일은 webui 쪽에서) |
| `floe2` | jobdeck의 제품 명령줄(P1c부터 유일한 floe2 CLI): 공유 CLI 명령 + `view`(그리고 인자 없음·소스만)는 GTK 뷰어(`python -m floe.gtkview`). 버전 = `floe/__init__.py`의 `__version__`. 입력 오류 exit 1(Python CLI와 같음) | jobdeck만 |
| `app`(`floe2-web`), `web`, `packager`, `electron/`, `desktop/` | 웹 CLI·서버·데스크톱 | webui만 |

- **공용 크레이트의 webui 변경 가운데 jobdeck이 새로 받은 것(P0):**
  - oasis: `header.rs`(시작 헤더 probe — 잡덱 소스)와 varint 넘침 수정
  - vfs: `lock::opening_refusal`
  - render-core
    - `revision_lease.rs`: 봉인된 revision의 읽기 임대
    - 질의 `points_truncated`
    - 여백이 커질 때 선의 반 픽셀 위상 유지(0ab074a9 — 픽셀이 바뀐다)
  - renderd
    - `query_context.rs`, `cancel_query kind= before_seq=`
    - snap/pick의 `scene_gen`·`scene_round`, 응답의 `scene_*`·`query_summary`·`query_status`·`points_truncated`
    - `scene_gen`이 없는 옛 클라이언트(`rust_render.py`)도 그대로 받는다.
- **vendor:** 38개 크레이트, 약 40 MB다. 새로 든 것은 serde·serde_json·sha1·getrandom·socket2와 그 의존성이며 모두 순수 Rust라 musl 빌드는 그대로다. 웹 서버용(tokio·axum·hyper 등)은 넣지 않았다. `.gitignore`는 `data/`를 `/data/`로 바꿨다(vendor의 sha1·windows-sys 테스트 데이터가 빠지지 않게).

## 2. 단계

| 단계 | 내용 | 상태 |
|---|---|---|
| P0 | 공용 크레이트 맞추기, app-core·worker-client·notices 도입, 게이트 worker_client·cell_index | 0.12.326 |
| P1a | `app-cli`와 `floe2` 실행 파일. Python CLI는 아직 그대로 둔다 | 0.12.327 |
| P1b | 빠진 기능을 Rust로(G1, G3~G8; §4). G2 밀도 요청·G9 `phase=render`는 `view`만 쓰므로 P2·P4로 미룸 | 0.12.328 |
| P1c | 게이트·배포·별칭을 Rust `floe2`로 바꾸고 Python `floe2/` 패키지를 지운다(§4) | 0.12.328 |
| P2 | `floe2 gtk-service`(stdio JSON-lines). GTK 뷰어의 비-UI 판단을 Rust로 옮기고 Python 모듈을 걷어낸다 | 예정 |
| P3 | KLayout 레거시를 제품 경로에서 빼고, 동결 `floe` 셸은 개발 전용 오라클로 둔다 | 예정 |
| P4 | (선택) GTK 렌더 루프를 Rust `ViewController`로 | 별도 승인 |

## 3. 동기 규칙

- **jobdeck → webui:** webui가 jobdeck을 병합하면 공유 크레이트의 jobdeck 변경이 그대로 들어간다. 두 판이 같으므로 충돌은 실제로 바뀐 곳에서만 난다.
- **webui → jobdeck:** webui가 공유 크레이트를 바꾸면, jobdeck은 필요할 때 그 판을 `git checkout feature/webui -- <crate>`로 가져오고 jobdeck 쪽 변경이 빠지지 않았는지 diff로 확인한다.
- 공유 크레이트를 바꾸는 jobdeck 커밋은 메시지에 "shared app layer"를 적는다.
- **GTK 뷰어 실행(`floe2 view`, P1a):**
  - 루트(`floe/gui.py`가 있는 폴더): `FLOE_GTK_ROOT`, 없으면 실행 파일 위쪽에서 처음 만나는 폴더(체크아웃의 `rust/target/release/floe2`, 번들의 `bin/floe2`)
  - 인터프리터: `FLOE_GTK_PYTHON`, 없으면 루트의 `.venv`, 없으면 PATH의 `python3`
  - `python -B -m floe.gtkview ARGS`를 이 프로세스 자리에 exec한다.
- **게이트:**
  - CLI를 부르는 게이트는 모두 Rust `floe2`를 부른다(`FLOE2_BIN`, 기본 `rust/target/release/floe2`; P1c). `rust/app-core`·`rust/app-cli`·`rust/floe2`를 바꾸면 `--changed`가 그 게이트들을 고른다.
  - `floe2`(validate_floe2.py `validate_rust_cli`, P1a): `floe2 --version`이 앱 버전이고, index·info·probe·render·clip이 Python 없이 돈다(PATH 맨 앞의 `python`·`python3`가 호출을 기록하는데 비어 있어야 한다). 모르는 단어는 GTK 뷰어의 소스로 넘기고, `floe2 view --help`는 GTK 진입점에 닿는다.
  - `worker_client`(tools/validate_worker_client.sh): Rust 클라이언트와 Python 어댑터가 같은 프레임을 내는지 본다(valmini; raw·PNG, 라벨, 스타일, 취소 반복).
  - `cell_index`: app-core의 design.ovh 추가·유지·거절을 본다.
  - `unit`(`cargo test --workspace`)에는 app-core(380개)·worker-client·notices의 단위 테스트가 든다.

## 4. P1b·P1c (0.12.328)

- **옮긴 기능(jobdeck Python → 공유 Rust):**
  - G1 칩 캡처: app-core `jobdeck/chips.rs`(floe/jobdeck/chips.py의 ChipTable — 이름·TC 경로·CHIP id, `N:`, 와일드카드, `#K`, 영역, 설명 줄, `info --chips` 목록). `Capture`에 `chip`·`chip_off`·`fit_chip`·`corners_fit`(`--corners` 단독·`corners=fit`), 배치 키와 덮어쓰기 규칙(shots.parse_batch 그대로), 보고서의 `chips` 행, Python과 같은 로그 문구(`rendered … um) in …s`, `WARNING: N jobdeck placement(s) not drawn`, `note: … in chips no shot had on`, `report … (N shot(s), INCOMPLETE)`, `rendered incomplete - …  [exit 3]`).
  - G3 색인 옵션: `--hier-only`(Action::HierOnly → `floe-index hier`), `--no-page-occupancy`, `--no-ovs`; 덱 소스는 늘 `--no-ovs`. 실행할 floe-index 명령줄을 `[floe2] …`로 보이고(프로파일이면 stderr), occupancy 인자는 Python 순서·표기(`--occupancy-um 2.0`).
  - G4 덱 색인의 BUSY: 75로 끝난 소스는 `BUSY … (see the [lock] line above …)`, 요약 `N built, N failed, N busy, N kept`(busy가 있을 때만), 종료 75(실패가 있으면 2).
  - G5 잠금 확인: `floe_vfs::lock::writing_refusal`(새 probe) — 다른 실행이 쓰는 캐시는 색인 전에 거절; 레이아웃을 열기 전 `opening_refusal`(통째 재색인 중이면 "being indexed"); Busy 오류는 `[lock] …` 한 줄과 exit 75; `FLOE_LOCK_WHAT` = `floe2 <명령>`; worker-client가 renderd의 `[lock]` 줄을 그대로 내보내고 `error code=locked`의 `text_hex`를 풀어 Busy로 받는다.
  - G6 옛 이름 이전(`cache::rename_legacy`)은 대상의 쓰기 잠금(Full) 아래에서만 — 쓰는 실행이나 독자가 있으면 Busy, 옛 이름 그대로.
  - G7 DRC 팩: `open_current`가 팩을 여는 동안 리더 잠금(kind Pack)을 쥔다(`Pack::hold`); 다시 팩을 만드는 중이면 Busy(75), 팩이 없을 때 통째로 만드는 중이면 "being indexed".
  - G8 `floe2 svrf`는 floe-index를 가리키고 exit 2(floe/cli.py cmd_svrf와 같은 문구·순서). SVRF 파서는 `floe-index svrf` 하나다.
  - 그 밖의 계약: 없는 소스 `source not found: …`, `jobdeck --on-missing fail`은 안내 줄과 exit 2, 덱 계획 `N source(s) have no index yet; run: floe2 index …`, open 때 `[jobdeck] skipped   : …` 줄.
- **Python 쪽:** `floe2/` 패키지(`python -m floe2`)를 지웠다. `floe2 view`가 `python -B -m floe.gtkview`를 실행하고(`FLOE_PRODUCT=floe2`, `FLOE_RENDERER=rust`, `FLOE2_BIN` = 자기 경로), 뷰어의 잡덱 색인은 `vfsclient.find_floe2()`로 Rust floe2를 부른다. 체크아웃이 없으면(번들) 인터프리터 자신의 site-packages를 쓴다.
- **배포:** 포터블 번들은 `runtime/bin/floe2`를 싣고(musl로 floe-index·floe-renderd와 함께 빌드, 덮어쓰기는 `FLOE_INDEX_BIN`·`FLOE_RENDERD_BIN`·`FLOE2_BIN` 셋 함께), floe2 런처는 그것을 `FLOE_GTK_PYTHON=runtime/bin/python3`로 실행한다. `rust/build-linux.sh`의 BINS에 floe2.
- **결정·차이(현장 영향):**
  - 종료 코드: 인자 해석 오류 2, 실행 중 입력 오류 1, 불완전 3, 잠금 75 — Python CLI와 같다(floe2-web은 입력 오류 2를 유지).
  - 심볼릭 링크인 캐시 폴더는 Rust CLI가 열지 않는다(webui의 보안 규칙, `catalog.rs` `Layout::open_directory`). Python 뷰어는 아직 연다. 현장에서 캐시를 링크로 옮겨 쓰는 경우가 있으면 정책을 다시 정해야 한다.
  - 웹의 DRC 등록 경로(`select_current_source`)에는 팩 리더 잠금을 아직 걸지 않았다(webui 몫).
  - app-core `svrf/parse.rs`는 webui의 `rust/app`(floe2-web svrf)이 app-cli로 옮길 때까지 남긴다.
  - floe/cli.py의 floe2 전용 분기(잡덱 색인·칩 캡처 등)는 동결 floe와 같은 파일이라 지금은 남겼다. 어떤 진입점도 부르지 않으며, P2(서비스)·P3(레거시 정리)에서 모듈째 걷어낸다.
