# 웹 표시 진단 이관

2026-09-16, M4g-17b. [M0 §2.8~2.9](WEBUI_M0.ko.md)의 `gtktest`/`--dump`
미이관을 실제 코드로 분리한 계약과 현재 구현이다. **합성 진단은 연결했지만
실제 브라우저 수용·입력 PNG·자동 dump의 이관 완료는 아니다.**

## 1. GTK의 실제 동작

| 경로 | 원래 동작 | 웹 현재 상태 |
|---|---|---|
| `gtktest [png]` | 선택 PNG를360×160으로 bilinear 축소해 표시 | 입력 PNG는 아직 미이관; 독립 합성 진단은 `displaytest` |
| `gtktest` 합성 | 검은360×160 RGB pixbuf에 빨강/초록/파랑/노랑70×100 막대4개 | 같은 픽셀의 Rust PNG/raw, 공통 디코더와 Canvas로 대조 |
| `gtktest` 배치 | 같은 pixbuf를 Overlay/ScrolledWindow 안에 표시 | 웹 `.viewport`의 crop/별도 투명 overlay를 표시; GTK 위젯 구조를 복제하지 않음 |
| `view --dump` 수신 | 수신 raw/PNG를 pixbuf로 만든 뒤 `/tmp/<APP>_frame.png`에 덮어씀 | 아직 변경하지 않음; 숫자 `--render-debug`와 다른 기능 |
| `view --dump` 합성 | overlays 후 `/tmp/<APP>_disp.png`와 widget alloc/mapped/visible 진단 | 저장 방식 결정 대기; 기존 Save view PNG만으로 대체 완료라고 세지 않음 |

`_display`의 기존 dump는 `_update_labels/_update_note_labels` **전**에 실행된다.
GTK dump가 항상 화면의 모든 주석을 포함한다는 가정도 맞지 않는다.
서버 파일 연속 덮어쓰기와 브라우저 최근 이미지 보관/명시 다운로드 중 어느 경계를
택할지 사용자에게 물었으며, 응답 없이 기존 `--dump` 의미를 바꾸지 않는다.
GTK 명령의 폐기·alias 승인도 이 합성 진단 구현에 포함하지 않는다.

## 2. 사용과 보안 경계

기존 `floe2-web view`의 **About → Run display test**에서 명시적으로 실행한다.
About를 열거나 view가 바뀐 것만으로 자동 실행하지 않는다. layout 없이도 가능하다.
`gtktest` CLI는 계속 명시 오류이며 위 합성 대안과 입력 PNG가 남았음을 안내한다.

M4g-17b는 인덱서·renderd도 없는 환경에서 실행할 독립 명령을 추가한다:

```sh
rust/target/release/floe2-web displaytest
rust/target/release/floe2-web displaytest --no-open --session-file /absolute/new-session.json
```

`--port N`(0=임의), `--firefox PATH` 또는 `FLOE_FIREFOX_BIN`도 기존 viewer와 같다.
독립 세션이므로 기본 workspace의 IPC 소유·forward·종료에 관여하지 않는다.
새 Firefox0700 프로필·0600 launch.html과 create-new0600 session JSON, 일회용120초
bootstrap/8시간 session을 그대로 재사용한다. 인증 URL은 argv/stderr에 넣지 않는다.
`--no-open`은 Firefox 발견도 생략하며 private session 파일을 사용한다.

전용 HTML은 같은 bundle의 고정 자산이며 레이아웃 UI/WS를 로드하지 않는다.
인증 후에도 **Run display test** 전에는 합성 API를 읽지 않는다. Quit는 기존 확인
대화상자와 `DELETE /api/v1/session`을 사용한다. 로그아웃·기한 만료·Ctrl+C·소유한
Firefox 종료 시 리스너와 생성한 파일을 정리한다. 기존 파일/심볼릭 링크는 덮어쓰지 않는다.
`--no-open`으로 수동 연결한 브라우저의 **탭 닫기만으로 서버를 종료하지는 않는다**.
Quit 또는 터미널 Ctrl+C를 사용한다. pagehide는 클라이언트 작업을 취소하고,
BFCache 복귀는 정지 상태를 알리며 재실행하지 않는다. 재로드는 같은 탭의 sessionStorage
자격으로 재인증한다. report는 storage에 저장하지 않는다.
이 단계는 입력 PNG 인자를 받지 않으며 GTK 명령의 alias/폐기나 `--dump` 변경이 아니다.

- owner 인증된 `GET /api/v1/display-test/png`와 `/raw`만 사용한다. 임의 파일 경로,
  source/view/query identity, 옵션 본문은 받지 않는다. host/origin·cookie/CSRF 경계를
  유지하며 증명 누락은401, 다른 origin은403, 무효 format은404다.
- Rust가 고정 픽셀을 한 번 생성해 불변 `Bytes`로 보관한다. raw는 기존 `FLOERAW1`
  헤더16바이트 +360×160 RGBA =230,416바이트, PNG는16KiB 미만이다. 파일·worker·
  index/cache·DRC·프레임 credit을 건드리지 않는다. 응답은 `no-store`다.
- 웹은 기존 뷰어와 **같은** `image-decode.js`의 raw `ImageData`/PNG `Image+Blob`
  경로를 사용한다. `protocol.js` 이미지 payload 검사를 공유한다. frame envelope,
  stale view/CAS·ACK/credit은 계속 실제 뷰어가 소유한다. 진단에 가짜 frame ID를
  만들어 live WebSocket에 끼워 넣지 않는다.
- 한 번에 한 실행만 허용한다. 고정 payload 상한·5초 XHR/PNG decode 제한을 두고,
  read 완료와 decode 시작 사이의 cancel도 검사한다. Cancel/About 닫기/pagehide는
  요청·decode·object URL·canvas·보고서를 정리한다. 실패/재접속/재열기 때 재시도하지 않는다.
- 결과는 PNG57600픽셀·raw57600픽셀·crop/overlay40960픽셀의 다른 픽셀 수다.
  panel C는(20,16)에서320×128로 자른 base와 흰 십자 투명 overlay를 분리 표시한다.
  보고된 DPR에 맞춰 source device pixel과 CSS 크기를 맞춘다. 일반 뷰를 움직이지 않는다.
- 보고서는 현재 UI에만 둔다. 경로/설계명/인증 정보/UA/월드 좌표를 수집하지 않으며
  서버 전송·파일 다운로드·storage 저장도 하지 않는다. 텍스트를 직접 선택해 복사할 수 있다.
  사용자의 `all_visible/display_problem` 관찰과 `desktop_acceptance:unverified`는 별도다.

Canvas readback은 **브라우저가 가진 bitmap**을 검사한다. OS compositor, 원격 ETX
화면, 실제 모니터 색, input-to-photon, live worker/WS까지 모두 통과했다는 의미가 아니다.
픽셀 검사0차이여도 실제 화면에 검게 보일 수 있어 세 panel의 관찰 결과가 필요하다.
이 기능을 만든 것과 이 환경의 실제 브라우저로 실행해 수용한 것을 구분한다.

## 3. 검증과 남은 범위

`tools/validate_display_test.py`는 GTK `cmd_gtktest` 안의 실제 `synth`와 GUI의
`fill_rect`를 AST로 추출한다. pixbuf 메모리 동작만 대역으로 제공하고 실제 함수가
만든57600픽셀을 Rust raw 및 Pillow로 디코드한 Rust PNG와 비교한다. 같은 native
fixture를 Node의 독립 pixel-canvas 하네스에 전달해 웹 PNG/raw/crop 결과도 검사한다.
Python/Node/Pillow는 개발 오라클이며 제품 런타임 의존성을 추가하지 않는다.

별도 단위/DOM/client 검사는 decode 중복 완료·늦은 callback·dimension 오류·timeout/
URL 회수, 명시 GET·취소·DPR·About 연결과 view 명령 무발행을 확인한다. 실제 transport
검사는 인증/바이너리 형식/반복 동일성/없는 view와 독립적인 응답을 확인한다.
독립 명령의 `validate_display_cli.py`는 빈 PATH와 존재하지 않는 index/renderd를
지정해도 실제 Rust CLI/HTTP 인증·PNG/raw 수신·logout/SIGINT·파일 보존이 동작하는지
검사한다. Firefox는 명시적인 테스트 대역으로만 실행하며 private argv/profile과
대역의 자발적 종료·정리를 확인한다. bootstrap/session 만료 시 owner worker 없이
리스너가 종료되는 Rust 검사와 인증/재로드/취소/종료의 DOM 검사를 추가했다.
전체 배터리 실행 기록은 [M4 §70~71](WEBUI_M4.ko.md)에 둔다. 이를 실제 브라우저 실행으로
대체 보고하지 않는다. 기존 브라우저 시작 경로의 도구 거부도 우회하지 않았다.

다음 잔여는 선택 PNG의 표시 진단·기존 GTK 명령의 제품 경계, `--dump` 저장 방식,
실제 Firefox/ETX의 표시/입력 수용이다. 기존 GTK 구현은 보존한다.
