# 공통 뷰어 — standalone / 일반 웹 / 회사 데모

2026-09-30, `feature/webui`.

## 원칙

Electron standalone과 `floe2-web view`는 같은 `index.html` / `app.js`를 실행한다.
회사 데모는 제한된 server session을 사용하지만, **공통으로 제공하는 뷰어 기능을 별도
구현하지 않는다.** 표시·입력 변경은 `viewer.js`와 공통 조작부 정의 한 곳에서 한다.
인증 방식이나 허용 API를 합치거나, owner UI 전체를 띄운 뒤 버튼만 숨기는 방식은 아니다.

| 책임 | 단일 구현 / 연결 |
|---|---|
| detail/thin/depth, frames/labels/mono, label px | `viewer.js::bindControls` |
| fit/zoom/pan, 키보드·IME 단축키, 깊이 단계/99 | 같은 `bindControls` |
| Goto 서버 좌표 동기화·편집 중 보존·Escape·승인 후 해제 | 같은 `bindControls` |
| 드래그·우 버튼 zoom band | 기존 공통 `gestures.js`, band 표시는 `viewer.js` |
| 휠 확대·축소의 현재 프레임/ACK/렌더 중 입력 억제 | `viewer.js::wheel`, 기존 `gestures.js::wheelNavigation` |
| DPR 정렬, 제한 해상도, letterbox | `viewer.js::dimensions/screen` |
| 픽셀 게시·foreground/margin 배치·이전 합성 화면 보존 | `viewer.js::paint/compose/freeze` |
| 렌더 대기 커서와 최종/부분 프레임 문구 | `viewer.js::busy/cursor/frameStatus` |
| 표시 옵션·Goto/zoom toolbar HTML | `index.html`의 `floe-view-controls/toolbar` 구간 |
| 뷰포트 프레임·band·대기 커서 CSS | `viewer.css` |

빌드 시 `build.rs`가 위 HTML 구간을 `server.html`에 삽입한다. 서버용 DOM ID 접두어만
붙이며, 표시 이름·옵션 값·기본 조작부를 수동 복제하지 않는다. 두 HTML은 같은
`viewer.js` / `viewer.css` 바이트를 각자의 제한된 자산 경로로 받는다. 변경 파일은
bundle 식별자에 포함되어 구 UI와 새 서버가 혼합되는 것을 막는다.

## 어댑터에 남는 것

`app.js`와 `server.js`는 진입점/어댑터로 남는다. 다음 차이는 제품 제약과 권한에
속하며 공통 모듈에 넣지 않는다.

- owner 로컬 인증 vs 데모의 one-use 교환·세션별 CSRF·WebSocket 프로토콜.
- 세션 재접속, 요청 승인·실패·ACK 순서, 제한된 요청 대기열. 데모의 8개 입력 대기열과
  제출 간격, 세션 수명, 서버의 해상도·CPU/메모리 제한은 유지한다.
- server는 connection/dataset/worker 교체·hidden·로그아웃 때 화면을 지운다.
  owner는 기존 로컬 복구 계약대로 연결 실패 시 이전 이미지를 참고용으로 남길 수 있다.
- owner의 레이어 패널·파일 선택·인덱싱·DRC·측정·clip/export·설정 저장 등 기능 플러그인.
  현재 데모의 미지원 범위는 그대로이며, 관련 owner API/자산은 데모 경로에 마운트하지 않는다.
- owner의 margin/overlay/query receipt·성능 상세·진단 훅, 데모의 프레임별 PNG 수신/게시 ACK.
  공통 합성기는 margin이 없으면 foreground만 다룬다. 프레임 수신·표시 직전의
  epoch/render key/revision 검증은 각 전송 어댑터와 공통 `protocol.js`에 유지한다.

`viewer.js`는 HTTP/WebSocket/storage/인증값을 소유하지 않는다. 전달받은 상태를 표시하고
편집 의도를 콜백으로 전달할 뿐이다. 파일 쓰기 권한을 UI capability만으로 부여하지 않는다.
공유 초대의 `guest.js`는 별도 권한 모델의 클라이언트이며 이번 owner↔회사 데모 통합의
대상이 아니다. 기존 공통 protocol/decoder/gestures는 계속 공유한다.

## 사용자가 보는 변화

- 데모의 Display와 Goto/zoom 조작부가 standalone과 같은 정의·배치·옵션 이름을 쓴다.
- 데모에도 같은 Ctrl+A/Ctrl+Z, F/C, B, G/D, </>, 숫자·99 단축키와 라벨 크기가 연결된다.
  허용하지 않은 owner 기능의 단축키는 연결하지 않는다.
- Goto 입력은 양쪽 모두 서버가 보낸 소수 문자열을 쓰고, 편집 중에는 pan/zoom이나
  새 상태 응답이 초안을 덮어쓰지 않는다. Go는 입력한 중심과 양수 view 폭을 적용한다.
- detail/thin 등 정책 변경 중에는 양쪽 모두 이전 화면을 참고용으로 유지한다.
  새 정책과 맞지 않는 프레임을 현재 프레임/질의 대상으로 승인하지 않는다.
- 양쪽 모두 대기 커서를 사용한다. ACK나 idle만으로는 완료가 아니며 해당 프레임의
  표시까지 기다린다. 최종 incomplete/실패/세션 종료에서 무한 대기가 남지 않아야 한다.
- owner의 native-pixel 배치·margin crop 최적화는 유지하고, 데모도 같은 합성 함수를 쓴다.
  모든 이동/옵션 변경에서 full-frame 복사를 추가하지 않는다. 실제 합성 위치를 고정해야
  하는 경우에만 freeze 복사가 발생한다.

## 회귀 게이트

- `viewer.test.cjs`: 동일 조작 입력/키/옵션 결과, Goto 초안/IME, DPR/상한/letterbox,
  current/stale·final/incomplete·대기 커서, 두 진입점의 공통 모듈 사용 및 권한 없는 모듈.
- `client.test.cjs`의 `FLOE_TEST_VIEWER=1`: 실제 owner 연결에서 모든 표시 옵션 변경,
  이전 frame receipt 보존·stale 거부·PNG 디코드 대기·실패 시 커서 복원.
- `server.test.cjs`: 같은 정책 변경·queued/ACK/snapshot/decode/rAF 순서, 실제 픽셀 버퍼를
  이용한 pan release/no-op/거부의 화면 보존, session 경계와 입력 제한.
- `server_broker`: 실제 내장 HTML이 canonical 두 구간을 포함하는지, 공통 자산 제공,
  owner 자산·API 차단과 인증/CSRF/Origin/HTTP opt-in 경계.
- 전체 UI / native `server_runtime` / 메뉴 inventory를 함께 검사한다.

2026-09-30 로컬 검증: `sh tools/validate_rust.sh --only
server_runtime,web_ui,web_menu_inventory` **ALL OK**. native 12건, ES2017 구문 검사와
전체 UI 회귀, 메뉴 inventory(41 call sites / 38 handlers)가 통과했다.
`cargo test --release --offline --locked -p floe-web --test server_broker`도 8건 통과했다.
빌드 중 UI를 바꾸면 테스트와 실행 파일의 bundle ID가 엇갈려 데모 WebSocket 연결이
거부되므로, 최종 검증은 소스를 고정한 뒤 실행 파일과 테스트를 다시 빌드했다.

실제 회사 서버·브라우저 및 Electron/RHEL/ETX 화면 수용은 별도다. UI는 실행 파일에
내장되므로 적용 시 `floe2-web` 재빌드·교체·재시작, Electron 배포물에는 새 실행 파일을
포함한 재패키징이 필요하다. 재인덱싱·프록시/인증 설정 변경은 필요 없다.
