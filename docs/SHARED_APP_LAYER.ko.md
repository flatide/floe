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
| app-core `drc::desktop` | 데스크톱 뷰어의 DRC 리뷰(P2b): GTK가 써 온 Python IcePack의 리뷰 동작 그대로 — 클릭마다 상태 바이트·규칙 카운터를 제자리에 씀, 다른 실행의 사이드카는 `.stale-<epoch>`로 옮기고 팩의 내장 섹션으로 새로 만듦, 쓸 수 없는 결과 폴더는 임시 폴더(경로 해시 이름), 그것도 안 되면 팩 안, 노트는 고칠 때마다 통째로 쓰고 마지막 노트가 없어지면 파일을 지움, Python이 쓴 줄을 너그럽게 읽음. 팩·ASCII 묶음 `Opened`(load_db 규칙, 검사 행, 오류 페이지, CD 선분) | 공유 |
| app-core `desktop` | 데스크톱 뷰어가 그리기 전에 묻는 것: 열 수 있는지(`ready`: 준비·현재성·누가 재색인 중인지), 레이아웃 meta(모든 키 + 색), 빈 레이어 목록 규칙(`list_layers`·안내 줄), layerprops 행(P2a) | 공유 |
| `app-cli` | 웹이 없는 CLI 명령: index·info·render·probe·clip·jobdeck·drc·svrf·fe-embed·selfcheck. webui `rust/app/src`에서 웹 명령을 뺀 것. 프로그램 이름과 입력 오류의 종료 코드는 실행 파일이 정한다(`Host`의 `name`·`input_error_exit`, `floe_app_core::set_program`; 기본 floe2-web·2) | 공유(P1a; webui의 `floe2-web`이 이 크레이트를 쓰도록 바꾸는 일은 webui 쪽에서) |
| `floe2` | jobdeck의 제품 명령줄(P1c부터 유일한 floe2 CLI): 공유 CLI 명령 + `view`(그리고 인자 없음·소스만)·`gtktest`는 GTK 뷰어(`python -m floe.gtkview`). 버전 = `floe/__init__.py`의 `__version__`. 입력 오류 exit 1(Python CLI와 같음) | jobdeck만 |
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
| P2 | `floe2 gtk-service`(stdio JSON-lines). GTK 뷰어의 비-UI 판단을 Rust로 옮기고 Python 모듈을 걷어낸다. 푸시 단위(§5): P2a 열기·준비·레벨 행·덱 스펙·레이어 속성 행, P2b DRC·svrf, P2c 레이어 속성 편집·fill, P2d(P3와 함께) 오라클을 떼어 낸 뒤 Python 모듈 삭제 | P2a 0.12.329, P2b 0.12.330, P2c 0.12.331, P2d 0.12.332 |
| P3 | KLayout 레거시를 제품 경로에서 빼고, 동결 `floe` 셸은 개발 전용 오라클(`tools/oracle/floe_oracle`)로 둔다(§6) | 0.12.333 |
| P4 | GTK 렌더 루프를 공유 Rust `ViewController`로(§7; 사용자 승인 2026-10-10). P4a 공유 컨트롤러의 데스크톱 정책, P4b perf 줄, P4c 서비스의 뷰 채널, P4d GTK 전환, P4e 질의·미니맵, P4f 정리 | P4a 0.12.334, P4c 0.12.335, P4b 0.12.336, P4d 0.12.337, P4e 0.12.338, P4f 0.12.342 |

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

## 5. P2 — GTK 뷰어는 UI만 (`floe2 gtk-service`)

- **서비스:** `floe2 gtk-service`(rust/floe2/src/service.rs)는 stdin에서 요청 한 줄(`{"id":N,"op":…}`)을 받아 stdout에 답 한 줄(`{"id":N,"result":…}` 또는 `{"id":N,"error":{"kind","message"}}`)을 순서대로 쓴다. stdin이 닫히면 끝나며, 그때 덱 스펙 폴더를 지운다. 판단은 app-core(`desktop`·`dataset`·`jobdeck`)가 하고, 서비스는 그 결과를 전달하는 통로다.
- **클라이언트:** floe/gtkservice.py가 프로세스당 서비스 하나를 띄운다(`find_floe2`, `FLOE2_BIN`). `ServiceCache`는 gui.py·rust_render.py가 읽던 Cache/DeckCache 속성을 그대로 준다: src, dir, meta, is_jobdeck, ids, mode, props_src, exists·load·is_stale·close, layer_props, set_mode·set_levels, 덱 뷰별 보이기 기억, catalog.infos[tc].cache_dir. 어떤 레이어가 켜져 있는지 같은 세션 상태는 GTK에 남는다.
- **P2a(0.12.329) 요청:**
  - `version`
  - `ready {source, levels?}`: 레이아웃은 캐시가 있는지·현재인지·재색인 중이면 누구인지, 덱은 그릴 수 있는 소스가 모두 색인됐는지(deck_ready).
  - `open {source, levels?, mode?}`
    - 레이아웃: meta.json 그대로에 색을 반영하고 빈 레이어를 뺀 것, 안내 줄, 캐시 폴더, props 행, stale.
    - 덱: DeckSnapshot의 meta, 서비스가 쓴 스펙 파일 경로와 handle, props 소스·행, 소스별 캐시 폴더.
  - `close {handle}`
  - `level_rows {source}`
- **floe2 뷰어 쪽:**
  - gui.py는 floe2일 때 `_open_file_load`·`_index_ready`·`_index_busy`·레벨 대화상자·레이어 속성 행을 서비스로 받는다. 동결 floe(APP floe)는 예전 Python 경로 그대로다.
  - cli.py `cmd_view`의 시작 열기(`_service_open`)와 `_cache_ready`도 서비스를 쓴다.
  - `DeckRenderWorker`는 floe/rust_render.py로 옮겼고, rust_render의 스타일은 `cache.layer_props()`를 쓴다.
  - 레벨 행 문구(`_level_row_text`)는 UI 문구라 gui.py에 있다.
- **같은 답인지:** jobdeck 게이트 `GtkServiceTests`가 서비스와 Python 구현을 맞대어 본다.
  - 덱 4개 × 뷰 3개, 레벨 선택 2개: meta, 스펙(숫자는 값으로 비교 — Rust는 `2.5e-5`, Python은 `2.5e-05`), props 소스, 소스 폴더.
  - `ready` = deck_ready(색인 안 된 복사본, 빠진 소스 포함), `level_rows`.
  - 레이아웃 meta = `Cache.load()`(layerprops로 색이 바뀌는 경우 포함), props 행 = `load_layer_props`, 소스가 바뀌면 current가 false.
  - 뷰어가 floe2로 열 때 ServiceCache, 색인 없는 덱은 "run: floe2 index".
  - 실제 GTK 창(`GuiSmokeTests`)은 Rust `floe2 view` → gtkview → 서비스로 연다.
- **현장 영향:** floe2 뷰어도 심볼릭 링크인 캐시 폴더를 열지 않는다(§4의 차이가 뷰어까지 옴). 열기가 Rust 쪽에서 더 엄격하다(버전·vfs·design.ovm 확인 — 예전 뷰어는 meta.json만 봤다).
- **다음:** P2b DRC(팩·상태·노트·waive·가져오기/내보내기 — 마커마다 부르던 상태·노트는 페이지 단위로 받아 GTK가 들고 있는다)와 svrf 사이드카, P2c 레이어 속성 저장·게시와 fill 패턴, P2d는 P3(오라클 분리)와 함께 Python 모듈 삭제.

### P2b (0.12.330) — DRC 리뷰와 svrf 사이드카

- **결정:** 웹의 리뷰 저장소(`drc::review::store`: snapshot → draft → publish, 팩 inode에 묶임, 처음 쓸 때 확인, 0600, 편집마다 파일 전체 재작성)는 GTK가 써 온 동작과 다르다(조사 2026-10-09).
  - 파일 형식은 같다.
  - 그러나 이 저장소를 그대로 쓰면 현장에서 보이는 것이 바뀐다: 1억 오류 팩의 클릭이 수백 MB 재작성이 되고, 같은 .db를 재팩하면 리뷰를 잃고, 읽기 전용 결과 폴더에서 쓸 수 없다.
  - 그래서 GTK용으로 Python IcePack의 동작을 app-core `drc::desktop`로 옮겼다. 웹 저장소는 webui용으로 그대로다.
  - 두 방식은 같은 파일을 같은 형식으로 읽고 쓴다. 같은 리뷰를 웹과 GTK가 동시에 고치는 경우의 조정은 webui와 함께 정할 일이다(웹 저장소 주석: GTK 같은 비협조 작성자와 CAS가 아니다).
- **서비스 요청:** `drc_busy`, `drc_find`, `drc_open`(mode pack|load), `drc_errors`, `drc_status`(hex), `drc_set_status`(선택 한 번에), `drc_status_page`, `drc_status_rank`, `drc_query`, `drc_set_note`·`drc_clear_note`, `drc_note_export`·`drc_note_import`·`drc_waive_export`·`drc_waive_import`, `drc_cd`, `drc_close`, `svrf_rules`, `svrf_operands`.
- **GTK 쪽:** floe/gtkservice.py `PackDrc`·`AsciiDrc`가 gui.py가 쓰던 IcePack·DrcDb 속성을 그대로 준다.
  - 오류는 256개씩 받아 두고, 상태는 4096개씩 받아 들고 있다가 쓰면 함께 고친다. 마커마다 묻던 상태와 노트는 왕복 없이 답한다.
  - waive 선택은 요청 한 번(`set_statuses`)이다.
  - gui.py는 floe/drc.py·floe/svrf.py를 더 이상 부르지 않는다.
  - `offset_screen_segment`(화면 기하)와 상태 상수는 gui.py에 있다.
  - 동결 floe의 GUI도 같은 서비스를 쓴다. DRC는 KLayout과 무관하기 때문이다.
- **같은 답인지:** DRC 게이트 D12가 같은 팩을 IcePack과 서비스로 열어 대조한다.
  - 작은 픽스처: 검사·오류·CD 선분, 서비스가 쓴 상태를 Python이 읽고 카운터가 맞음, 페이지·순위, 무작위 사각형 질의(필터 포함).
  - gen_drcdb 자산(검사 60, 오류 수천): 전체 오류, 무작위 waive 뒤의 카운터·페이지·순위, 상한·필터·검사 부분집합별 질의.
  - 파일: 노트 파일 바이트 = IcePack 직렬화, 다시 열면 같은 노트, 지우면 파일 없음. waive 내보내기 바이트가 같고 남의 파일은 거절. 리뷰 중 재팩은 75. 남의 사이드카는 옮기고 새로 만듦.
  - svrf: 피연산자와 사이드카.
- **열린 항목:** Rust 색인(app-core index·cell_index·drc build)은 소스 옆에 `X.oas.floe.index.lock`(숨김 아님)과 `.X.oas.ice.index.lock`을 남긴다. webui의 앱 수준 잠금(리비전·등록 세트·산출물 보호가 이 이름을 안다)이며, 0.12.328부터 `floe2 index`가 Rust라 jobdeck 현장에서도 생긴다. floe_vfs 잠금(`.floe-lock/`)으로 옮기는 일은 webui와 함께 정한다.

### P2c (0.12.331) — 레이어 속성 파일

- **옮긴 것:** layerprops 파일 형식이다. Layer 메뉴의 속성 불러오기(`layerprops_read`), 저장(`layerprops_save`), 디자인 기본값 게시(`layerprops_publish`, `<props_src>.layerprops`)를 app-core `desktop::{read_props, write_props, publish_props}`(layerprops::parse/format)가 한다. 쓰기는 임시 이름을 거쳐 rename으로 한다.
- **남긴 것:** 팔레트 표(7×7 색 이름, 5×4 fill 비트맵)는 floe/fillpat.py가 읽는 UI 표시 데이터로 GTK에 둔다. Rust(`styles`)와 Python 모두 같은 `floe/colornames.def`·`floe/fillpatterns.def`를 읽는다(Rust는 `include_str!`). 그래서 이 두 파일은 P2d에서 Python 모듈을 지울 때도 남긴다.
- **같은 답인지:** `GtkServiceTests`가 확인한다.
  - 읽기: 주석, 빈 줄, `7.20.9` 같은 점 키, 남는 열, 빠진 열, 잘못된 줄 = `fillpat.parse_layerprops`.
  - 저장: 빈 이름은 `l_d`, 공백 이름은 `_` = `fillpat.format_layerprops` 바이트.
  - 게시: 경로와 바이트가 `cache.save_shared_props`와 같다.
- **차이:** 탭이나 제어 문자가 든 레이어 이름은 Python이 깨진 줄로 썼다. Rust는 쓰기를 거절하고 이유를 상태줄에 보인다.

### P2d (0.12.332) — 제품 경로에 Python 비-UI 코드가 없다

- **뷰어의 import:**
  - `floe2 view` → `python -m floe.gtkview` → floe/viewcli.py(뷰 인자, 단일 인스턴스 전달, 시작 열기; 동결 셸의 floe/cli.py가 아님) → floe/gui.py.
  - gui.py가 맨 위에서 import하는 것은 gtkservice, fillpat, hangul, product, rust_render뿐이다. 렌더 워커 생성기와 DETAIL 표는 rust_render.py로 옮겼고, service.py는 그 이름을 다시 내보낸다.
  - 동결 floe(APP floe)의 경로만 그 분기 안에서 cache·cachepath·indexlock·jobdeck·service(KLayout)를 늦게 import한다.
  - 쓰이지 않던 `live_caps`는 지웠다.
- **레이아웃 색인:** 뷰어에서 레이아웃을 색인할 때 floe2는 `floe2 index SRC --jobs 12`(Rust)를 부른다. 캐시 폴더, 옵션, 잠금은 Rust가 정한다. 동결 floe는 floe-index vfs를 직접 부른다.
- **번들:** 기본 floe2 번들은 뷰어 파일만 싣는다(`FLOE2_PRODUCT_FILES`: `__init__`, gtkview, viewcli, gui, gtkservice, rust_render, vfsclient, instance, product, hangul, fillpat, `.def` 두 개). KLayout 번들(FLOE_PORTABLE_KLAYOUT=1)은 동결 셸까지 모두 싣는다.
- **강제:**
  - jobdeck `GuiSmokeTests`: 실제 창(덱 세 번, 레이아웃 + `--drc` 한 번)을 import 차단기 아래에서 연다. 차단기는 cache, cachepath, indexlock, drc, svrf, jobdeck, shots, fe_embed, render, viewport, coverage, view_policy, cli, service, klayout을 import할 수 없게 한다. 리뷰 사이드카는 서비스가 쓴다.
  - floe2 게이트: 번들 목록이 뷰어가 불러오는 floe 모듈을 모두 담고 오라클 모듈은 하나도 담지 않는다.
- **남은 것(P3):** 오라클 모듈은 아직 floe/ 안에 있다. 게이트가 KLayout 오라클과 대조 기준으로 쓰기 때문이다. → P3(§6)에서 개발 전용 패키지로 옮겼다.

## 6. P3 (0.12.333) — 동결 floe 셸은 개발 전용 오라클 패키지로

- **옮긴 것:** `floe/`에 남아 있던 동결 셸과 Python 기준 구현을 `tools/oracle/floe_oracle/`로 옮겼다(`git mv`라 이력이 이어진다).
  - 대상: `__main__`, cli, cache, cachepath, indexlock, drc, svrf, shots, fe_embed, render, viewport, coverage, view_policy, service, `jobdeck/*`
  - vfsd 클라이언트(`VfsClient`)는 `floe_oracle/vfsclient.py`로 갔다. 실행 파일 찾기(`find_binary`, `find_floe2`)는 제품의 `floe/vfsclient.py`에 남는다.
- **`floe/`에 남은 것:** 뷰어 파일 13개다.
  - `__init__`, gtkview, viewcli, gui, gtkservice, rust_render, vfsclient, instance, product, hangul, fillpat, `colornames.def`, `fillpatterns.def`
  - 번들의 `FLOE2_PRODUCT_FILES`와 같다. floe2 게이트가 `git ls-files floe`와 맞춰 본다.
- **제품 코드에서 지운 것:**
  - gui.py의 동결 floe 분기: Python 캐시·덱 열기, 색인 잠금 확인, 레벨 행, `floe-index vfs` 직접 색인, KLayout 밀도 커버리지(`v`, `cov:`)
  - rust_render.py의 KLayout 워커 선택: `make_render_worker`는 Rust만 쓰고, `FLOE_RENDERER`가 rust가 아니면 거절한다.
  - `floe.cache`로 layerprops를 읽던 대체 경로: 캐시가 `layer_props()`를 준다. 오라클의 `Cache`·`DeckCache`도 같은 메서드를 갖는다.
  - product.py는 floe2만 안다. `FLOE_PRODUCT`는 제품에서 아무것도 고르지 않는다(값이 floe여도 창·소켓은 floe2).
- **오라클 실행:** `python -m floe_oracle CMD`는 옛 `python -m floe CMD`다. `PYTHONPATH`에 `tools/oracle`과 체크아웃을 둔다.
  - 명령: index(`--legacy` `.tiles` 포함), info, render, clip, probe, profile, drc, svrf, jobdeck
  - KLayout과 Rust 중 무엇으로 그릴지는 `floe_oracle.service.make_render_worker`가 정한다. 동결 셸의 기본은 KLayout이고, Rust를 고르면 제품의 워커를 쓴다.
  - 버전은 제품의 것을 쓴다.
- **오라클에서 뺀 것 — 뷰어(`view`)와 `gtktest`:**
  - 뷰어는 제품의 것(`floe2 view`) 하나다. 계획에 적었던 "KLayout 비교용 gui 사본"은 만들지 않았다.
  - 이유: 어떤 게이트도 그 사본을 돌리지 않는다. 그러면 10,600줄 사본이 제품 gui·rust_render와 따로 놀다 소리 없이 깨진다.
  - KLayout 대조는 다음으로 계속한다: `python -m floe_oracle render`(KLayout) 대 `floe2 render`(Rust)의 PNG, 그리고 게이트의 KLayout 픽셀 오라클(render_goldens, render_speckle, render_frames, klayout, rust_renderer).
  - 동결 KLayout 뷰어는 floe-legacy 브랜치에 있다.
- **`floe2 gtktest [PNG]`:** 동결 셸의 GTK 표시 점검(검은 화면 진단)을 뷰어 진입점(floe/viewcli.py)으로 옮겼다.
  - P1c 뒤로 Rust floe2는 이 단어를 소스 이름으로 뷰어에 넘겼다. README가 안내하던 명령이 사라져 있었던 것을 되살린 것이다.
  - `floe2 --help`에도 적었다.
- **게이트:**
  - 오라클을 쓰는 게이트 24개(validate_*)와 벤치·실험 도구 8개가 `floe_oracle`을 import한다(`tools/oracle`을 경로에 더함).
  - `-m floe`를 쓰던 곳은 `-m floe_oracle`로 바꿨다: index_cli, drc_ice의 `--floe-reviewer`, bench_warm, validate_rust.sh의 레거시 `.tiles` 빌드.
  - rust_renderer의 view 테스트는 제품의 floe/viewcli.py를 부른다.
  - jobdeck `GuiSmokeTests`: 오라클을 `PYTHONPATH`에 둔 채 차단기가 `floe_oracle`과 `klayout` 전체를 막는다.
  - floe2 게이트가 새로 확인하는 것:
    - 제품 패키지가 `python -m floe`로 실행되지 않는다.
    - 뷰어의 정체성이 `FLOE_PRODUCT`가 없거나 floe·floe2여도 floe2다.
    - floe/ 파일이 번들 목록과 같다.
    - floe/의 모든 import(함수 안 지연 import 포함, AST)가 뷰어 모듈·오라클·KLayout 밖을 가리키지 않는다.
    - `floe2 gtktest`가 뷰어 진입점에 닿는다.
  - validate_rust.sh `--changed` 매핑은 오라클 경로를 따른다. 오라클의 cache·cachepath를 바꾸면 전체 배터리가 돈다.
- **번들:**
  - 기본 floe2 번들은 그대로다(뷰어 파일만). 기본 번들에 `floe_oracle`이 들어 있으면 검증이 실패한다.
  - 개발용 KLayout 번들(`FLOE_PORTABLE_KLAYOUT=1`)은 `floe_oracle`을 함께 싣는다. `floe` 런처는 `python -m floe_oracle`을 부르고(명령만), selfcheck가 이를 확인한다.
- **다른 문서의 경로:** `floe/cache.py`, `floe/drc.py`, `floe/jobdeck/…` 같은 경로는 쓰인 때의 위치다. 지금은 `tools/oracle/floe_oracle/` 아래에 있다. 뷰어 파일(gui.py, rust_render.py 등)은 그대로 floe/에 있다.
- **현장 영향:**
  - 제품(`floe2`·`floe-index`·`floe-renderd`와 GTK 뷰어)의 명령·출력·종료 코드는 바뀌지 않았다.
  - `floe2 gtktest`가 다시 동작한다.
  - 개발 체크아웃의 `python -m floe`는 `python -m floe_oracle`(뷰어 없음)이 됐다.
- **공유 크레이트 변경(webui 병합 때 참고):** app-core `Error::opening(e, what, path)` — 열 수 없는 파일의 I/O 오류가 경로를 말한다. 없는 파일이면 `source not found: PATH`(`cache::fingerprint`), `jobdeck not found: PATH`(`JobDeck::read`)다. 종류는 그대로 Io다.
  - 그전에는 `No such file or directory (os error 2)`만 나왔다. `floe2 info`·`render`·`view`, 그리고 floe2 뷰어의 열기(서비스)가 모두 그랬다(P1c·P2a 이후).
  - jobdeck 게이트 `LoadingBannerTests`가 P3에서 floe2 경로로 돌면서 드러났다(전에는 동결 floe 경로가 "no VFS cache"를 말했다).

## 7. P4 — GTK 렌더 루프를 공유 `ViewController`로

사용자 승인(2026-10-10): "P4 진행해줘".

- **출발점:** 공유 app-core에 웹 뷰어의 `view` 모듈이 이미 있다(webui에서 P0로 들어옴, 약 8,800줄).
  - `ViewController`: 제어 스레드 하나가 renderd 워커 하나를 맡는다. 편집 CAS(`state_rev`), 이전 렌더를 취소하고 비운 뒤 다시 제출, 여백 미리 그리기와 잘라 쓰기, 질의(snap·pick), 셀 트리, 눈금자, 클립 준비를 한다.
  - `ViewState`·`Patch`·`Navigation`, 미니맵 투영, 팔레트·layerprops·fill 슬롯.
  - 웹이 GTK 동작을 옮겨 만든 것이라 깊이 단계, 미니맵 6 px 여백, 휠 같은 규칙이 이미 같다.
- **GTK와의 차이(조사 2026-10-10):** 데스크톱에 필요한데 없는 것, 또는 다르게 동작하는 것.
  - 컷 아래 밀도 켜고 끄기(`density=`)와 여백 프레임의 `vw`/`vh`가 worker-client 요청에 없었다.
  - 렌더 오류 하나가 뷰를 닫는다(웹은 다시 연다). GTK는 상태줄에 말하고 계속 쓴다.
  - 비우기 5초, 렌더 300초, snap·pick 5초를 넘기면 뷰가 실패한다. GTK에는 이런 마감이 없다.
  - Esc(진행 중인 렌더만 멈추기)가 없다.
  - 상태줄 perf 줄(`perf_status`, 현장 로그의 형식)이 없다. 컨트롤러는 라운드별 `fields`만 준다.
  - 프레임 형상: 웹은 뷰포트 그대로 그리고 16 px 단위로 잘라 쓴다. GTK는 프레임을 2 px 격자에 맞추고 2 px 여유를 둔다. 스페클 위상이 프레임 기준이라 연이은 프레임이 짝수 px만큼 움직여야 깜빡이지 않는다. → P4d에서 GTK 끌기를 2 px 단위로 맞춘다(키 이동은 이미 16 px).
  - GTK 쪽에 남는 것(UI): 프레임 표시와 확대 중 이전 프레임, 덧그림(DRC 마커, 선택, 고무줄, 눈금자, 셀 강조), 위젯.
- **단계(푸시 단위):**

| 단계 | 내용 |
|---|---|
| P4a | 공유 컨트롤러의 데스크톱 정책(아래). GTK는 그대로 |
| P4b | 라운드를 누적한 프레임 보고와 perf 줄(`perf_status`)의 Rust 이식. Python과 같은 문자열인지 대조 게이트로 확인 |
| P4c | `floe2 gtk-service`의 뷰 채널(열기·Patch·Esc·프레임 이벤트·raw 파일)과 Python 클라이언트. 같은 프레임인지 게이트로 확인 |
| P4d | `gui.py`가 그 채널을 쓴다. 입력은 Patch로 보내고 받은 프레임만 표시한다. 예전 Python 루프는 `FLOE_GTK_LOOP=legacy`로 남겨 두고, 현장 확인 뒤 P4f에서 지운다. **현장 확인 필요** |
| P4e | 내비게이션(확대·이동·밴드·미니맵 클릭·goto·fit)과 미니맵 바탕을 컨트롤러에서 받는다. snap·pick·셀 트리·루트·클립은 P4d에서 이미 채널을 탄다. 눈금자와 미니맵의 뷰 상자 그리기는 UI로 남긴다 |
| P4f | 예전 루프(여백·`_covered`·정착 로직)를 지우고, `rust_render.py`를 제품에서 빼서 게이트용 개발 클라이언트로 옮기고, 번들·문서를 정리한다 |

### P4a (0.12.334) — 공유 컨트롤러의 데스크톱 정책

웹 동작은 바뀌지 않는다. 바뀌는 것은 여백 요청의 `vw`/`vh` 하나이고, renderd가 원래 받는 값이다.

- **worker-client `RenderRequest`:**
  - `viewport: Option<(u32, u32)>`는 `vw=`/`vh=`다. 프레임이 뷰포트가 아닐 때(여백) 그 뷰포트를 말한다. renderd는 밀도 점을 뷰포트의 fit 보기에 맞춰 솎는다(2026-10-04).
  - `density: Option<bool>`는 `density=on|off`다.
  - 둘 다 None이면 줄에 나가지 않는다.
  - `WorkerClient::set_timeouts(render, query)`와 `RenderSession::set_timeouts`를 더했다.
- **`ViewState`·`Patch`:** `density: Option<bool>`를 더했다. None이면 renderd 기본값(`FLOE_RUST_DENSITY_STACK`)을 쓴다. 렌더 정책이므로 렌더 키에 들어간다.
- **`ViewController`:**
  - **여백 요청:** 뷰포트 크기를 `vw`/`vh`로 함께 보낸다. 웹도 마찬가지다. 여백과 그 뷰포트의 밀도 점이 같아진다.
  - **`DesktopPolicy`:** 기본값은 웹의 것이다(비우기 5초, worker-client의 마감, 렌더 오류면 뷰 실패). `DesktopPolicy::desktop()`과 `start_desktop(.., ControllerOptions, DesktopPolicy)`가 데스크톱 정책을 쓴다.
    - 비우기는 걸리는 만큼 기다린다.
    - 렌더 마감 30일, snap·pick 마감 10분이다.
    - 전경 렌더 오류는 `Snapshot::render_failure = (render_rev, 이유)`로 남기고 뷰는 계속 쓸 수 있다.
    - 교체(prepare_replacement)와 fork는 정책을 물려받는다. 웹이 리터럴로 만드는 `ControllerOptions`는 그대로 두었다.
  - **`cancel_render()`(Esc):** 진행 중인 전경 렌더를 멈추고, 같은 상태는 다시 그리지 않는다. 다음 변경이 다시 그린다. `Snapshot::cancelled_rev`가 그것을 말하고, 이미 보인 프레임은 남는다. 여백만 돌고 있으면 아무것도 하지 않는다.
- **테스트:**
  - 단위 테스트 `desktop_esc_stops_the_frame_and_renders_again_on_the_next_change`, `desktop_render_error_is_said_and_the_view_goes_on`(웹 정책은 같은 오류로 뷰가 실패), `desktop_drain_waits_as_long_as_the_frame_takes`, `density_is_a_render_policy_and_goes_on_the_request`
  - 여백 요청의 `vw`/`vh`(기존 margin 테스트에 더함)
  - worker-client `viewport_and_density_go_on_the_wire_only_when_given`

### P4c (0.12.335) — `floe2 gtk-service`의 뷰 채널

P4b(perf 줄)는 따로 진행 중이다. 이 단계에서 GTK는 아직 그대로다.

- **요청**(`rust/floe2/src/view.rs`):
  - `view_open {source, levels?, mode?, width, height, patch?, margin?, frame_cache?}`는 `{view, snapshot, model}`을 돌려준다.
    - `ManagedDataset::open`, `ViewState::initial`, 첫 패치를 거쳐 `ViewController::start_desktop(.., DesktopPolicy::desktop())`로 연다.
    - 자원 한도는 `RenderOptions::local()`(`FLOE_RUST_*`)이 요구하는 만큼이다. 데스크톱에는 함께 쓰는 사용자가 없다.
  - `view_edit {view, patch, base?}`: base가 없으면 현재 `state_rev`를 쓴다. 뷰를 고치는 쪽은 뷰어 하나뿐이다.
  - `view_cancel`(Esc), `view_snapshot`, `view_close`.
  - 패치 JSON은 웹 `PatchDto`의 이름을 그대로 쓰고 `density`를 더했다. 항목은 navigation(fit·pan·zoom·goto·minimap·band), pixels, depth·depth_step, detail, thin, layers, layer_change, frames, labels, font_px, mono, density, style_deltas, root이고, 모르는 필드는 거절한다.
- **이벤트**(응답 사이에 한 줄씩, stdout은 줄 단위로 잠근다):
  - 뷰마다 펌프 스레드가 5 ms 간격으로 컨트롤러를 본다.
  - `{"event":"frame"}`: 새 전경·여백 프레임이다. 바이트(FLOERAW1 헤더 + RGBA, 또는 PNG)를 뷰 폴더(`$TMP/floe2-view-<pid>-<n>`, 0700)에 임시 이름으로 쓰고 이름을 바꾼 뒤 그 경로를 보낸다. 뷰어가 읽고 지운다. 넘기지 못한 파일은 4개까지만 남긴다.
  - `{"event":"view"}`: 스냅숏이 바뀌었다. 상태, 단계, 카운터, `render_failure`, `cancelled_rev`가 든다.
  - `{"event":"closed"}`.
- **Python**(`floe/gtkservice.py`):
  - `Service`에 읽기 스레드를 두었다. 응답은 기다리는 요청에게, 이벤트는 `events()`로 간다.
  - `ViewSession(source, w, h, ids?, mode?, patch?, margin?, frame_cache?)`는 `edit(**patch)`, `cancel()`, `close()`, `events()`를 준다. `events()`는 다른(닫힌) 뷰의 프레임 파일을 읽지 않고 지운다.
  - `read_frame(frame)`은 (RGBA 또는 PNG 바이트, w, h, format)을 돌려주고 파일을 지운다.
- **renderd 수정**(공유 크레이트, 0.12.308):
  - 컷 아래 밀도의 첫 라운드(pass 1을 먼저 보이는 `density_round=1`)의 줄에 `style_epoch`, `labels_truncated`, 그 라운드 자신의 미완료 질의 장면(`scene_gen=gen scene_round=n scene_complete=0`)을 더했다. 그전에는 프레임 줄 형식이 아니어서 공유 worker-client가 그 줄을 거절했다(`missing style_epoch`, 뷰 실패).
  - 뒤따르는 라운드 번호도 하나씩 올라간다(전에는 두 줄 모두 `round=1`이었다).
  - 웹은 밀도를 보낸 적이 없어 이 경로를 지나지 않았다. Python 어댑터는 라운드 번호를 읽지 않는다.
- **게이트 `gtk_view`**(`tools/validate_gtk_view.py`, 약 3초): valmini를 `floe2 index`한 뒤 `ViewSession`으로 확인한다.
  - 처음 열기
  - 16 px 단위로 맞춘 이동
  - 밀도 켜기(첫 라운드가 최종 아님 + `density_round=1`, 이어서 최종 프레임, 라운드 증가, 픽셀 6,611개가 바뀜)
  - 여백 프레임(`viewport`=[400, 300], 더 넓음)과 그 안의 이동(새 프레임 없음, `crop_hits`)
  - 각 프레임이 같은 상자·크기·정책의 `RustRenderWorker` 프레임과 바이트까지 같다(여백은 `bg`와 뷰포트를 넘겨).
  - 닫으면 폴더가 사라지고, 다음 뷰가 열린다.

### P4b (0.12.336) — 프레임 보고와 perf 줄을 Rust로

현장에서 붙여 주시는 perf 줄(상태줄의 짧은 줄과 툴팁·터미널의 긴 줄)은 형식이 계약이다. 그래서 Python과 바이트까지 같게 옮겼다. 따로 진행해 P4c 다음에 들어왔다.

- **`app-core view::perf`(새 모듈, 공유):**
  - `FrameReport`: 세대 하나의 누적이다. `floe/rust_render.py`의 `_submit_render` 상태와 `_emit_frame`의 합·최댓값·결과 사전을 그대로 옮겼다.
    - `round(&fields, probe, &PerfAdapter, PerfTiming)`, `frame(&Frame, ..)`, `is_final`
    - `PerfJob`(bbox, w, h, scope, bg, cut_px), `PerfAdapter`(raster_jobs, max_depth, dbu), `PerfTiming`(어댑터가 잰 adapter_read_us, elapsed_ms)
  - `perf_status` / `perf_status_with(.., density_only)`는 (긴 줄, 짧은 줄)을 준다. `fmt_count`, `occ_note`, `load_note`(Viewer._load_note)도 옮겼다.
  - Python 규칙을 따랐다. `round()`는 짝수 쪽으로 반올림하고, `int / int`는 정확히 반올림하며, `%d`는 버리고, `%g`와 `str(float)`, 같은 값 중 처음 것을 고르는 `max`, 줄 글자의 `int()`(`1_000`, `+5`, ` 7`)도 같게 했다.
  - 같을 수 없는 경우는 renderd가 내지 않는 값뿐이다. 무한·NaN(JSON null), u64나 2^96을 넘는 정수, 비ASCII 숫자, Python `perf_status` 자신이 거절하는 값이다.
- **`floe/rust_render.py`:** `FLOE_RUST_RECORD=<경로>`를 주면 내보낸 프레임마다 JSON 한 줄을 남긴다. 받은 필드, 작업·어댑터 값, 잰 시간, 픽셀을 뺀 결과다. 주지 않으면 아무것도 바뀌지 않는다.
- **게이트 `perf_parity`**(`tools/validate_perf_parity.py`와 무시된 통합 테스트 `rust/app-core/tests/perf_parity.rs`, view::perf 단위 테스트; 따뜻할 때 약 35초):
  - 합성 perf 줄 17,422개. 전체 프레임을 키마다 경곗값으로 바꾸고, 시드를 고정한 무작위 사전 4,000개를 더했다. `perf_status`·`occ_note`·`fmt_count`의 모든 줄을 지나는지 줄 추적으로 강제한다.
  - `fmt_count` 1,074개, `occ_note` 9,690개, `_load_note` 108개
  - 어댑터로 그린 실제 렌더 37가지: MAIN01 비슷한 칩과 잡덱. detail, depth, 라벨·프레임, 타일 재사용, 여백, 밀도(점유와 계획), 48 MB 예산 맞춤, 16페이지 라운드, probe, 덱 합성을 덮는다.
  - 어댑터 자신의 `_emit_frame`에 넣은 퍼즈 프레임 줄 400세대
  - 합계 903라운드를 Rust로 다시 돌렸다. 결과 사전은 키·타입·값이 같고, 두 perf 줄이 같다.
- P4d에서 서비스의 프레임 이벤트가 이 보고와 perf 줄을 싣는다. GTK는 그것을 표시만 한다.

### P4d (0.12.337) — GTK 뷰어가 컨트롤러의 렌더 루프를 쓴다

**현장 확인이 필요했다.** 이상하면 `setenv FLOE_GTK_LOOP legacy`로 예전 Python 루프(`floe/rust_render.py`)로 돌아갔다. 회사 Linux 확인(2026-10-10, 0.12.341) 뒤 P4f(0.12.342)에서 예전 루프와 이 스위치를 지웠다.

- **뷰어(`floe/gui.py`, `controller_loop()`):**
  - 렌더 워커 자리에 `gtkservice.ViewWorker`가 들어간다. 뷰어는 정책(크기, depth, detail, thin, 레이어, frames, labels, 글꼴, 회색조, 밀도, 루트)과 뷰를 편집으로 보내고, 받은 프레임을 그린다. 바뀐 항목만 보낸다.
  - 뷰 계산(휠·키 이동·끌기·밴드·미니맵·goto·fit, `_clamp_view`)은 뷰어에 남고, 뷰는 `goto`(중심·폭 µm)로 간다. 컨트롤러의 뷰가 다르면(데스크톱 범위로 자름) 뷰어가 그것을 받는다. 끌기 중에는 보내지 않고 놓을 때 보낸다.
  - 컨트롤러가 정한다: 뷰가 120 ms 멈춘 뒤 그리기(정착), 이전 렌더 취소, 여백과 잘라 쓰기, Esc, 렌더 오류를 상태줄에.
  - 끌기를 놓으면 화면의 프레임에서 짝수 px 떨어지게 맞춘다. 스페클 위상이 프레임 기준이라 홀수면 새 프레임이 왔을 때 무늬가 뒤집힌다. 예전 루프는 프레임마다 2 px 격자에 맞췄다.
  - 상태줄과 터미널의 perf 줄은 서비스가 프레임에 실어 보낸 것이다(P4b의 Rust `perf_status`).
  - snap, pick, 셀 트리, 클립, 색·무늬 바꾸기, 회색조는 `ViewWorker.submit`가 뷰 채널 요청이나 편집으로 바꾼다. 답은 예전 어댑터와 같은 사전으로 온다.
  - 덱의 색 모드 전환은 새 뷰를 지금 자리에서 연다. 컨트롤러가 fit을 먼저 그렸다 버리지 않는다.
  - GUI 스모크가 실패하면 워커가 죽은 이유를 함께 말한다.
- **공유 컨트롤러(app-core):**
  - `DesktopPolicy::clamp`: 편집마다 뷰를 `Viewport::clamped`(GTK `_clamp_view`: 확대 0.01 dbu/px~fit의 16배, 다이와 그 10% 바깥)로 자른다.
  - `DesktopPolicy::settle`: 뷰만 바뀐 편집(이동·확대·크기)은 120 ms 동안 멈춘 뒤 그린다. 휠이나 끌기 한 번이 렌더 한 번이다. 정책 편집은 바로 그린다.
  - `DesktopPolicy::report`: 세대마다 `FrameReport`를 두고, 받아들인 프레임마다 보고를 남긴다(`ViewController::frame_report(id)`). `ms`는 제출부터 잰다.
  - `DesktopPolicy::incomplete_queries`: 그린 장면이 완전하지 않아도(컷 아래 밀도, 예산 맞춤의 부분 프레임, 미뤄진 페이지) snap·pick이 그린 것으로 답한다. GTK 어댑터가 그렇게 했다. 웹은 그대로 거절한다.
  - 넷 다 웹 기본값에서는 꺼져 있다. 웹 동작은 바뀌지 않는다.
- **worker-client:** `WorkerClient::set_incomplete_queries`(`RenderSession`도)가 snap·pick 줄에 `incomplete=ok`를 붙이고, 그 답(`scene_complete=0`인 ok)을 받는다. 웹이 리터럴로 만드는 `QueryRequest`는 그대로 두었다.
- **renderd(0.12.310):** snap·pick이 `incomplete=ok`를 받는다. 없으면 예전처럼 `scene_incomplete`로 거절한다.
- **`floe2 gtk-service` 뷰 채널:**
  - `view_query {view, frame, kind: snap|pick, x, y, r_px, nth?, layers?}`: x·y는 뷰포트 px, frame은 화면의 프레임 id다. 답은 `query` 이벤트.
  - `view_cells {view, seq, kind, src?, cell?, pattern?, limit?, box?, cap?, root?}`: 답은 `cells` 이벤트(rust_render의 사전). 뷰의 워커가 아직 열리는 중이면(큰 캐시의 renderd 열기) 열릴 때까지 기다렸다가 묻고, 컨트롤러의 셀 대기열이 차 있으면 60초까지 다시 낸다. 다른 거절도 같은 seq의 `cells` 이벤트로 온다(0.12.340: 전에는 요청 오류로 돌아와 셀 트리가 `loading…`에 멈췄다 — main01, 2026-10-10).
  - `view_clip {view, seq, bbox, layers?, cell_name?, out}`: 쓰고 나면 `clip` 이벤트.
  - 프레임 이벤트에 `report`(Python 결과 사전 모양), `perf`([긴 줄, 짧은 줄]), `depth`가 붙는다.
  - 처음 상태도 데스크톱 범위로 자른다.
- **예전 루프와 다른 점(현장에서 보일 수 있는 것):**
  - 여백 안에서 끝난 끌기가 16 px 단위가 아니면 새 프레임을 그린다(renderd 타일 재사용으로 빠르다). 예전 루프는 잘라 썼다. 키 이동은 16 px 단위라 그대로 잘라 쓴다.
  - 화면의 프레임이 지금 상태의 것이 아니면(이동 직후 그리는 중) snap·pick은 답하지 않는다.
- **게이트:**
  - `gtk_view`(약 7초): 채널의 snap·pick·셀 트리·클립이 어댑터와 같다. 실제 창의 `Viewer`가 컨트롤러 루프로 열고 확대·이동·밀도·회색조마다 어댑터와 바이트가 같은 프레임을 보이며, perf 줄은 Python `perf_status`와 같다. 밀도를 켠 채 snap이 같은 답을 준다. 여백 안 이동은 잘라 쓴다. `FLOE_GTK_LOOP=legacy`는 예전 루프로 그린다.
  - jobdeck: `GuiSmokeTests`가 컨트롤러 루프로 창을 열고(덱 세 번, 레이아웃 + DRC) 덱 한 번은 legacy로 연다. 실제 뷰어의 모드 전환 테스트는 두 루프에서 돈다(legacy는 전환 전 debounce, 컨트롤러는 레이어가 바로 편집으로 가고 새 뷰가 그 레이어와 자리를 가진다). `DeckViewChannelTests`는 덱의 레벨 헤드 색·무늬·선 굵기와 레벨 일부를 어댑터와 바이트로 비교한다.
  - 단위: `desktop_frames_carry_their_report_and_the_web_s_do_not`, `desktop_queries_answer_from_an_incomplete_frame`, `desktop_clamp_keeps_the_gtk_zoom_range_and_the_die_in_reach`, `desktop_settle_renders_a_pan_burst_once_and_a_policy_edit_at_once`, worker-client `an_incomplete_scene_answers_only_when_the_query_allowed_it`, renderd `incomplete=ok` 파싱·거절.

### P4e (0.12.338) — 내비게이션과 미니맵을 컨트롤러에서

P4d처럼 컨트롤러 루프에서만 바뀐다. `FLOE_GTK_LOOP=legacy`는 예전 Python 계산 그대로다.

- **내비게이션:** 뷰어의 확대·이동이 컨트롤러의 `Navigation` 편집이 된다. 계산은 공유 `Viewport::navigate`(웹과 같은 것)가 하고, 뷰어는 결과 뷰를 받아 그린다.
  - 휠과 Ctrl+Z / Shift+Z, +/−: `Zoom {factor, anchor}`. anchor는 커서(또는 가운데)의 뷰 비율이다.
  - 화살표(Shift = 1/10): `Pan {x, y, snap}`. 걸음은 뷰어가 예전처럼 16 px 단위로 맞춘 값이다(최소 16 px).
  - 오른쪽 버튼 밴드: `Band {start, end, axes, outward}`. 방향(전체 움직임에서 우세한 쪽)과 5 px 넘게 움직인 축은 뷰어가 정한다(입력 판정).
  - 미니맵 클릭: `Minimap {point}`(미니맵 px). 다이 밖 테두리 처리와 16 px 단위 반올림은 컨트롤러가 한다.
  - goto, fit: `Goto {center_um, width_um}`, `Fit`.
  - 바뀐 정책(크기 등)과 같은 편집으로 보낸다. 뷰어 자신의 뷰는 보내지 않는다(내비게이션이 컨트롤러의 뷰를 옮긴다).
  - 끌기는 그대로다. 움직이는 동안 뷰어가 옮기고, 놓을 때 짝수 px로 맞춘 뷰를 `goto`로 보낸다.
  - 뷰가 아직 열리는 중이거나 편집이 거절되면 뷰어의 계산으로 옮긴다.
- **미니맵:** `view_minimap {view, depth?, bbox?}`가 바탕 이미지(다이, 테두리, 깊이별 경계 상자; 팔레트 숫자 180×180)와 다이의 자리(`placement`: 픽셀 상자와 배율)를 준다. app-core `view::minimap`의 굽기(웹과 같은 것)다. 루트 아래에서는 그 다이의 기본 바탕이다(구운 경계는 맨 위 셀의 것). 뷰어는 깊이마다 한 번 받아 두고, 움직이는 뷰 상자는 매 프레임 직접 그린다(서비스 왕복을 그리기마다 하지 않는다).
- **UI로 남는 것:** 눈금자(점, 거리 글자, 겹침 배치), 미니맵의 뷰 상자, 끌기 중 미리보기. 웹의 `ruler` 모듈은 HTTP 문자열 좌표용이라 GTK에 맞지 않는다.
- **app-core:** `minimap::placement(bbox)`를 더했다(공개 함수 추가, 웹 영향 없음).
- **게이트 `gtk_view`:** 실제 창의 뷰어에서 내비게이션 13가지(Ctrl+Z, 커서 위치 휠 확대·축소, 화살표, 1/10 화살표, 밴드 확대·가는 밴드·밴드 축소, 미니맵 클릭, 창 크기를 준 goto와 확대 유지 goto, fit)가 컨트롤러로 가고, 뷰어의 Python 계산과 같은 뷰가 된다. 미니맵의 자리와 바탕(전체 + 깊이별)이 뷰어의 굽기와 바이트까지 같다. 단위 `placement_is_the_projection_s_die_and_its_scale`.

### P4f (0.12.342) — 예전 Python 렌더 루프를 지우고 `rust_render.py`를 제품에서 뺀다

사용자 확인(2026-10-10, 회사 Linux, 0.12.341): 셀 트리, 숨긴 레이어 취소선, 색·fill 팔레트가 동작한다. 사용자: "P4f 진행해줘".

- **뷰어(`floe/gui.py`):**
  - 렌더 루프는 컨트롤러의 것뿐이다. `FLOE_GTK_LOOP`와 `controller_loop()`를 지웠다.
  - 지운 것: `_submit_render`·`_submit_margin`·`_schedule_margin`·`_covered`·`_frame_holds_view`·`_settle_after_frame`·`_margin_enabled`·`_preview_tick`, 세대 번호(`gen`, `_job_keys`)와 debounce, `dropped`·`cancelled`·프레임 결과 처리, `FLOE_MARGIN_MAX_MPIX`(컨트롤러의 여백도 16 Mpx로 묶인다: `margin::grow`의 `MAX_PIXELS`).
  - `redraw`는 뷰를 보이고 바뀐 것을 컨트롤러에 보낸다. Esc는 `ViewWorker.cancel()`이다.
  - 뷰어 자신의 뷰 계산(확대·이동·밴드·미니맵·goto·fit)은 뷰가 아직 열리는 동안의 대체로 남는다. 열린 뒤에는 컨트롤러의 계산이다(P4e).
  - `--stream-kb`·`--stream-target-ms`·`--render-debug`는 명령줄 호환으로 받기만 한다.
  - detail 표(`DETAIL_LEVELS`·`DETAIL_PX`·`DEFAULT_DETAIL`)는 gui.py로, 셀 질문 종류(`CELL_QUERY_KINDS`)는 gtkservice.py로, About 창의 renderd 찾기는 `vfsclient.find_renderd()`로 옮겼다.
- **`rust_render.py`는 개발 전용 오라클로:** `git mv floe/rust_render.py tools/oracle/floe_oracle/rust_render.py`. 게이트 20여 개와 벤치가 기준 어댑터(`RustRenderWorker`, `DeckRenderWorker`, `make_render_worker`)로 쓴다. 예전 Python perf 줄(`perf_status`, `occ_note`)도 `floe_oracle/perf_line.py`로 옮겼다. perf_parity의 Python 기준이다.
- **`floe/`는 12개 파일이다:** `__init__`, gtkview, viewcli, gui, gtkservice, vfsclient, instance, product, hangul, fillpat, `colornames.def`, `fillpatterns.def`. 번들 `FLOE2_PRODUCT_FILES`와 같다(floe2 게이트가 `git ls-files floe`와 맞춰 본다). floe2 게이트는 `rust_render.py`·`perf_line.py`를 오라클 모듈로 보고 번들과 뷰어 import에서 막는다.
- **번들:** 기본 floe2 번들에는 pip 휠이 없다. 뷰어가 NumPy·Pillow를 쓰지 않는다. KLayout 개발 번들만 NumPy·Pillow·KLayout을 넣는다. selfcheck도 그에 맞췄다. `make_portable.sh`는 x86_64 Linux에서만 돌아서 이 Mac에서는 `bash -n`까지만 확인했다. **회사 Linux에서 번들을 한 번 만들어 확인해야 한다.**
- **게이트:**
  - `rust_renderer`: 예전 루프 전용 테스트(정착 판단, 마진 미리 그리기와 그 픽셀 상한, 세대별 오류, 여백 안 이동의 즉시 제출)를 지웠다. 이 동작은 컨트롤러 단위 테스트(마진, 정착, Esc, render_failure)와 `gtk_view`가 맡는다. 새 루프에 맞게 고친 것: thin·밀도·루트는 컨트롤러 패치(`_ctl_policy`)에, `--margin`·`--frame-cache`는 `ViewWorker`의 margin·frame_cache로 간다; 거절된 프레임(render_failure)과 Esc 취소는 한 번 말하고 대기 상태를 푼다; Esc는 `worker.cancel()`을 부른다. perf 줄 문자열 테스트는 `floe_oracle.perf_line`을 쓴다.
  - `gtk_view`: legacy 확인을 지웠다. 기준 어댑터는 오라클의 것이다.
  - jobdeck: GuiSmoke의 legacy 덱 실행을 지웠다. 모드 전환 테스트는 컨트롤러 루프만 본다.
