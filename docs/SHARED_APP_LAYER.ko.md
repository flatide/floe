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
| `app-cli` | 웹이 없는 CLI 명령: index·info·render·probe·clip·jobdeck·drc·svrf·fe-embed·selfcheck. webui `rust/app/src`에서 웹 명령을 뺀 것. 프로그램 이름은 실행 파일이 정한다(`Host`, `floe_app_core::set_program`; 기본 floe2-web) | 공유(P1a; webui의 `floe2-web`이 이 크레이트를 쓰도록 바꾸는 일은 webui 쪽에서) |
| `floe2` | jobdeck의 실행 파일: 공유 CLI 명령 + `view`(그리고 인자 없음·소스만)는 GTK 뷰어(`python -m floe.gtkview`). 버전 = `floe/__init__.py`의 `__version__` | jobdeck만 |
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
| P1b | 빠진 기능 G1~G9를 Rust로: 칩 캡처, 밀도 요청, 색인 옵션, BUSY, 잠금 확인, 옛 이름 이전 잠금, DRC 팩 잠금, svrf 단일 파서, `phase=render` | 예정 |
| P1c | 게이트·배포·별칭을 Rust `floe2`로 바꾸고 Python CLI 경로를 지운다 | 예정 |
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
  - `floe2`(validate_floe2.py `validate_rust_cli`, P1a): `floe2 --version`이 앱 버전이고, index·info·probe·render·clip이 Python 없이 돈다(PATH 맨 앞의 `python`·`python3`가 호출을 기록하는데 비어 있어야 한다). 모르는 단어는 GTK 뷰어의 소스로 넘기고, `floe2 view --help`는 GTK 진입점에 닿는다.
  - `worker_client`(tools/validate_worker_client.sh): Rust 클라이언트와 Python 어댑터가 같은 프레임을 내는지 본다(valmini; raw·PNG, 라벨, 스타일, 취소 반복).
  - `cell_index`: app-core의 design.ovh 추가·유지·거절을 본다.
  - `unit`(`cargo test --workspace`)에는 app-core(380개)·worker-client·notices의 단위 테스트가 든다.
