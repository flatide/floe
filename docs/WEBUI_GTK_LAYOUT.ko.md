# 웹 UI의 GTK 배치 전환과 셀 트리

2026-09-30, `feature/webui`. 사용자 요청: "웹버전 UI가 기존 버전과 많이 다름.
레이어 목록, 컬러 팔레트, 미니맵이 없고 DRC pane도 없음. 상단 메뉴 구조도 아님.
상단의 FLOE2 타이틀과 세션 관련 부분을 남기고 나머지는 기존 UI와 동일하게 구성.
회사 데모 페이지는 load layout 같은 부분을 예외 처리. 필요하면 feature/jobdeck을
병합(cell tree 추가됨)." 이 문서는 그 구현 범위와 검증, 잔여를 기록한다.

## 1. 병합

`feature/jobdeck`(cb634dd, 9커밋)을 `feature/webui`에 병합했다(12d77df).
지난 병합과 같은 규칙으로 rustfmt 충돌은 jobdeck 내용을 채택한 뒤 다시 rustfmt했고,
renderd의 webui 전용 scene-pinned 질의 바인딩(`query_summary_layers`)과
`render-core/cache.rs`의 빈 plan 정규화(`pages.is_empty()`와 새 view root 경우)를
모두 유지했다. `validate_rust.sh`는 `cell_tree` 게이트를 `WEB_APP_GATES` 옆에 둔다.
renderd 0.12.231, `design.ovh` 색인 단계, `floe-index svrf`가 함께 들어왔다.

## 2. 소유자 화면(`index.html`)의 배치

GTK `floe/gui.py`(feature/jobdeck)와 같은 구성이다. 모든 기존 컨트롤 id는 그대로이며
옮겨졌을 뿐이라 기존 플러그인(`palette.js`, `drc.js`, `clip.js`, …)과 Electron 호스트의
id 참조, `client.test.cjs`의 id 스캔은 바뀌지 않았다.

```text
header  floe2 [WEB PREVIEW] · 문서 제목 · 연결 상태 · Share locally… · End session · About   (유지)
nav     File | View | Cell | Ruler | DRC | Jobdeck | Help                                  (menubar.js)
+------------------+--------------------------------------+---------------------------+
| cells | DRC |    | Fit − +   X Y View µm Go             | expand all | collapse all   |
| inspect          |                                      | 1     [swatch] 1          |
| find cell…       |            canvas                    | 2     [swatch] 2          |
| tree             |                                      | 63.63 [swatch] MARKER     |
| ☑highlight zoom  |                                      | 9 layers · hint           |
| root top         |                                      | minimap | palette         |
+------------------+--------------------------------------+---------------------------+
status  x … y … um | Live · … | view W × H µm | depth: 0/2 · detail · thin · frame
perf    Rust foreground · plan … (기존 두 번째 행)
```

- **왼쪽 pane**(`#left-pane`, 기본 260px, 최소 156px, 드래그/더블클릭/화살표 조절):
  `cells`(새 `cells.js`), `DRC`(기존 `#drc-panel` 전체가 이 페이지 안에 있음; DRC를
  읽으면 GTK처럼 DRC 탭을 올리고 pane을 420px로 한 번 넓힘), `inspect`(기존 pick/ruler
  섹션 — GTK는 상태줄에 쓰지만 웹의 pick/snap/ruler 조작부가 있어 탭으로 둠).
- **오른쪽 pane**(`#right-pane`, 기본 250px, 최소 210px): expand/collapse all, 레이어
  목록, 아래 `minimap | palette` notebook. 레이어 행은 Calibre 순서(그룹 표식 `+/-`,
  `L.D`(datatype 0은 `L`), 31×14 swatch(레이어 색 테두리 + 채움 1:1), 이름)로 그리며
  숨긴 행은 색을 유지한 채 흰 취소선, 선택 행은 `#31566d`다. 체크박스와 색 입력은
  키보드/스크린리더/테스트용으로 남기고 시각적으로 숨겼다(`.layer-check`, `.layer-edit`).
  swatch 클릭은 행 스타일 편집기(색·채움·선폭)를 연다. 소유자 화면은 서버 페이지를
  64행씩 이어 읽어 **한 스크롤 목록**으로 보인다(`wholeList`, 1024행 초과 시 기존 페이징).
  `#palette-page`의 49색/20채움 grid는 팔레트 탭이 보일 때만 읽는다(기존 `<details>`
  open 계약을 탭이 대신 유지).
- **메뉴**(`menubar.js`): 선언적 모델이며 상태를 소유하지 않는다. 열릴 때마다 체크/
  라디오 항목은 대리 컨트롤(`#detail`, `#frames`, `#live-mode` …)을 다시 읽고, 활성화는
  그 컨트롤을 click/set+change한다. 대리 컨트롤이 hidden/disabled면 항목도 그렇다.
  GTK 항목과 대응: File(load layout/jobdeck → 기존 browse 대화상자, registered sources·
  index·clip·settings·shared default는 도구 대화상자, copy/save view, End session),
  View(fit/zoom/goto/detail/label size/depth/frames/labels/mono/thin/overlays/Layers/
  Display options…), Cell(tree·zoom·highlight·clear·root·top·build), Ruler, DRC(open/
  rules/reconnect/next/prev/waive/note/box/build pack/restore/Show DRC pane), Jobdeck
  (level/chip/source 라디오·toggle·levels), Help(About/licenses/Share locally…).
  `tools/validate_web_menu_inventory.py`는 Cell 메뉴 7개 handler를 포함해 48 call sites /
  45 handlers 모두 linked다.
- **도구 대화상자**(`panes.js`): 이전 사이드바 섹션은 `#source-dialog`(browse 버튼·등록
  소스·모드·레벨·open/close·CLI launch proposal·Index and open), `#display-dialog`
  (공통 `floe-view-controls` 블록·overlays·snapshot), `#clip-dialog`, `#settings-dialog`
  (layerprops 저장/불러오기·공유 기본값), `#index-dialog`로 옮겼다. CLI launch proposal이나
  색인 승인 버튼이 나타나면 source 대화상자를 자동으로 연다. 공통 controls/toolbar 블록은
  그대로여서 데모 페이지 주입도 유지된다.
- **상태줄**: GTK처럼 두 행이다. 첫 행 왼쪽에 커서 좌표 `x … y … um`(현재 viewport의
  표시용 선형 투영; 질의/이동 좌표는 계속 Rust), 기존 `#status`, 오른쪽에 view 크기와
  `depth: d/max · detail · thin · frame`(`#dstatus`). 둘째 행은 기존 `#perf`.

## 3. 셀 트리(`cells.js` + 백엔드)

GTK 40eb80b/92a100f/5151052의 동작을 따른다. 검색 150ms 디바운스(`*`/`?` 와일드카드,
2000건 한도), 확장 시 자식 지연 로드(placeholder 뒤 삽입), `… N more (find by name)`,
선택 시 `bbox`(정보줄: 인스턴스 수·크기·approx)와 현재 view의 `insts`(캔버스에 cyan
2px 외곽선, 7px 미만은 7px 사각형; view가 바뀌면 다시 질의), zoom/Enter는 셀 범위를
view의 80%로 goto, `t`는 탭을 올리고 검색창 포커스, root/top은 view root 설정/해제,
Escape는 강조 해제. 색인 요약이 없으면(`nohier`) 안내와 `build cell index…` 버튼을 보인다.
트리는 뷰가 `idle/rendering`이 된 뒤에 읽고(열리는 중의 busy 회피), 실패하면 다음 안정 상태에서
한 번씩 재시도한다. 강조 오버레이는 다른 오버레이처럼 DPR 크기의 비트맵을 CSS 크기로 놓는다.

**백엔드**(worker-client → app-core → floe-web):

- `rust/worker-client/src/cells.rs`: `CellRequest`(sources/children/find/bbox/insts) →
  renderd `cell_*` 명령, `CellReply`/`CellFailure{nohier|superseded|state|query|oversize}`
  파싱(`-` 빈 목록, hex 이름, n/total/cap 일관성). 회신 줄 한도는 명령 64 KiB와 별도로
  **4 MiB**이며 초과 줄은 그 seq의 `oversize` 실패로 강등된다(worker 장애 아님).
  in-flight 4, 기한 60초. `RenderRequest`/`ClipRequest`에 `root` 필드(` root=N`).
- `rust/app-core`: `ViewState.root: Option<Root{cell,name,bbox}>`, `Patch.root: RootEdit
  {Clear|Cell{src,cell}|Resolved}`; root 변경은 `same_policy`(render key)에 포함되고
  die(`ViewState::die`)가 root bbox가 되어 fit/minimap/goto 경계가 바뀐다. jobdeck은 root를
  거부(`root_unsupported`), 도형 없는 셀은 `cell_without_shapes`. `ViewController::cell_query`
  는 ticket 큐(4)로 run loop가 엔진에 제출하고 mpsc로 회신을 돌려준다(lock 밖에서 대기,
  타임아웃 시 ticket 회수). `resolve_root`는 `cells src cell`로 이름·bbox를 얻은 뒤 CAS 편집.
  같은 소스의 revision 전환은 root를 해제하고 top으로 다시 fit한다.
- `rust/web`: `POST /api/v1/views/{id}/cells`(owner, `spawn_blocking`, 15초 → 504),
  409 `{"error":"nohier"|…,"message"}`; WS `view.set`의 `root: null | {src,cell}`
  (해결은 `spawn_blocking`); snapshot에 `capabilities.cells/cell_root`, `root`, `root_name`;
  minimap 투영은 root 아래에서 root bbox를 die로 쓰고 frontier 없음(`base:"full"`).
  데모 브로커는 `cells:false, cell_root:false`이며 root patch를 거부한다.
  `tools/web_permissions.json`에 경로 등록.
- UI: root 아래에서는 제목에 `· root NAME`, minimap은 baked base 대신 die만 그린다.

검증: `cargo check --workspace --all-targets` 통과; `floe-worker-client` 15 + lifecycle 14,
`floe-app-core --lib` 372, `floe-web --lib` 154, `permissions_inventory` 3,
`server_broker` 8 통과. 실제 daemon(`tests/real_cells.rs`, ignored, env로 경로 지정)에서
valmini의 `cell_sources`/`cells`/`cell_find`/`cell_bbox`/`cell_insts`/`render root=3`을
확인했다(`design.ovh` cells=7). 색인 시 `design.ovh`가 함께 생성된다.

## 4. 회사 데모 페이지(`server.html`)

같은 배치를 쓰되 파일 열기·색인·DRC·측정·clip·설정 저장·공유 기본값은 **마운트하지도
표시하지도 않는다.** `build.rs`가 `index.html`의 canonical 블록 4개(`floe-view-controls`,
`floe-view-toolbar`, 새 `floe-right-pane`, `floe-layer-menu`, `floe-layer-tools`)를
`server-` 접두어(id/for/aria-controls/aria-labelledby/aria-describedby)로 주입한다.

```text
header  floe2 [DEMO] · 세션 상태 · Reconnect · End my session · Samples
nav     File(Samples… · Reconnect · End my session) | View(소유자와 동일, overlays 제외)
canvas + 오른쪽 layers/minimap/palette pane (왼쪽 cells/DRC pane 없음)
status  frame status | view 크기 · depth/detail 행 · 아래 세션 주체 행
```

데모 클라이언트(`server.js`)는 공통 모듈(`palette.js`, `presets.js`, `fill-editor.js`,
`minimap.js`, `menubar.js`, `panes.js`)을 `server-` 접두 `el`로 바인딩하고, 이들이
부르는 소유자 경로를 **자기 세션의 읽기 전용 경로로만** 매핑한다:

| 소유자 경로 | 데모 세션 경로 | 응답 |
|---|---|---|
| `POST /api/v1/views/{id}/palette` | `POST /api/v1/server/sessions/{id}/palette` | 같은 page/range DTO (`LayerCatalog::model`) |
| `GET /api/v1/views/{id}/minimap/{base}` | `GET …/minimap/{base}` | 같은 180×180 palette 문자열 |
| `GET /api/v1/views/{id}/fill-slots/{key}` | `GET …/fill-slots/{key}` | `editable:false`의 세션 슬롯 표 |
| `GET /api/v1/palette/presets` | `GET …/presets` | 번들 49색/20채움 |

레이어 표시/스타일 편집은 이미 허용된 WS `view.set`(`PatchDto`)로만 간다. 비트맵 슬롯
편집·소유자 파일/쓰기 경로는 계속 없다. `tools/web_permissions.json`에 위 경로를
`server_session` 권한으로 등록했다.

## 5. 검증

- `node tools/validate_web_ui.cjs`: ES2017 파싱(menubar/panes/cells 추가), 새
  `cells.test.cjs`, `server.test.cjs`의 shared-panels 환경(메뉴 구성, 세션 경로만 사용,
  Detail 라디오 → `view.set`, 팔레트 탭이 presets를 읽음), 기존 client/palette/minimap 회귀.
- `python3 -B tools/validate_web_menu_inventory.py --require-complete`: exit 0.
- 브라우저(Chrome, valmini.oas, 최종 실행 파일): 메뉴·서브메뉴·라디오(detail high) 적용, 레이어 행
  선택, minimap/palette 탭 전환, 상태줄; 셀 트리 로드(VALMINI_TOP/DEEPA/MEGAFILL/MID)·확장(LEAF1/2)·
  선택 강조(1 in view, 인스턴스 위치에 cyan 상자)·zoom·view root(제목 `· root DEEPA`, minimap
  `Root DEEPA`, fit)·top 복귀·검색 `LEAF*`(2 matches)·Escape 복원.
- 데모(Caddy HTTP 시험 프록시 127.0.0.1:8080 → 58080, `--prepare-demo`한 valmini): `/demo`
  샘플 목록 → 세션 페이지에 File/View 메뉴, 9개 레이어 행, minimap, 팔레트 49색/20채움 로드,
  더블클릭 숨김이 `view.set layer_batch`로 반영(`layers.mode=only`), 색 프리셋 `style_batch`
  accepted. 세션이 소유자 경로(`/api/v1/views/…`)를 부르지 않음을 요청 로그로 확인.
- Rust: `cargo check --workspace --all-targets`, `floe-web --lib` 154 · `server_broker` 8 ·
  `permissions_inventory` 3, `floe-app-core --lib` 372, `floe-worker-client` 15+14 통과.

## 5b. 검토 후속 수정(2026-09-30)

외부 검토 5건을 같은 날 반영했다.

- **view root와 DRC 좌표계(P1)**: DRC 오류 좌표는 top 셀 좌표다. DRC 이동(`prepare`
  patch)은 같은 편집에서 `root: Clear`를 함께 적용해 top으로 복귀한 뒤 이동한다
  (`drc/focus.rs`). root 상태에서 `In view` 필터는 서버가 `drc_view_root`(409)로 거부하고
  (`drc/http.rs`), 패널은 마커·In view·box 선택을 멈추고 안내를 표시한다(`drc.js`).
- **root 전환 뒤 stale bbox(P2)**: `cells.js`가 root(cell) 변경을 감지해 배치 extent·강조를
  버리고 재조회한다. extent(bbox)와 강조(insts)의 flight를 분리해 view 변경이 진행 중인
  extent 응답을 버리지 않게 했다.
- **잡덱 DBU 혼용(P2)**: `cell_bbox`/`cell_insts` 결과는 뷰(덱/root) 좌표이므로 뷰의
  `dbu_um`만 곱한다. `cells` 응답의 자기 좌표 bbox는 `localBbox`로 분리했다.
- **대형 트리 응답 한도(P2)**: 소유자 HTTP 도우미의 회신 한도가 요청별 옵션이 되어 셀
  조회는 8 MiB를 허용한다(renderd 20 000행 상한 내).
- **서브메뉴 키보드(P2)**: 서브메뉴를 부모·소유 항목 참조를 가진 독립 메뉴로 다뤄
  →/Enter 진입, ←/Escape 복귀, ↑↓/Home/End 탐색을 고쳤다. `menubar.test.cjs`가 고정한다.

## 6. 잔여

- 색인 요약 빌드(`floe-index hier`)를 웹 승인 경로로 제공하지 않았다. `build cell index…`
  버튼은 안내만 하며 비활성이다.
- GTK와 다른 점: inspect 탭(웹 전용 pick/snap/ruler 조작부), Goto/zoom toolbar 행 유지,
  Registered sources/Index 대화상자(웹 전용 등록 소스), Overlays 항목. 데모에는 왼쪽
  pane이 없다.
- 실제 RHEL/ETX/Electron 화면 수용, 회사 서버 배포는 별도다. UI가 실행 파일에 내장되므로
  `floe2-web` 재빌드·교체·재시작이 필요하다.
