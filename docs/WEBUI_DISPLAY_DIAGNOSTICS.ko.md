# 웹 표시 진단 이관

2026-09-16, M4g-17c. [M0 §2.8~2.9](WEBUI_M0.ko.md)의 `gtktest`/`--dump`
미이관을 실제 코드로 분리한 계약과 현재 구현이다. **합성·정적 입력 PNG 진단은
연결했지만 실제 브라우저 수용·GTK 진단 폐기·자동 dump의 이관 완료는 아니다.**

## 1. GTK의 실제 동작

| 경로 | 원래 동작 | 웹 현재 상태 |
|---|---|---|
| `gtktest [png]` | 선택 PNG를360×160으로 bilinear 축소해 표시 | `displaytest [PNG]`: 정적 PNG snapshot을 브라우저 smoothing으로360×160 표시. GTK 보간 픽셀 동일성은 보장하지 않음 |
| `gtktest` 합성 | 검은360×160 RGB pixbuf에 빨강/초록/파랑/노랑70×100 막대4개 | 같은 픽셀의 Rust PNG/raw, 공통 디코더와 Canvas로 대조 |
| `gtktest` 배치 | 같은 pixbuf를 Overlay/ScrolledWindow 안에 표시 | 웹 `.viewport`의 crop/별도 투명 overlay를 표시; GTK 위젯 구조를 복제하지 않음 |
| `view --dump` 수신 | 수신 raw/PNG를 pixbuf로 만든 뒤 `/tmp/<APP>_frame.png`에 덮어씀 | 아직 변경하지 않음; 숫자 `--render-debug`와 다른 기능 |
| `view --dump` 합성 | overlays 후 `/tmp/<APP>_disp.png`와 widget alloc/mapped/visible 진단 | 2026-09-16 브라우저 최근 프레임/합성 화면 보관·명시적 다운로드로 결정; 구현은 남으며 기존 Save view PNG만으로 대체 완료라고 세지 않음 |

`_display`의 기존 dump는 `_update_labels/_update_note_labels` **전**에 실행된다.
GTK dump가 항상 화면의 모든 주석을 포함한다는 가정도 맞지 않는다.
서버 파일 연속 덮어쓰기와 브라우저 최근 이미지 보관/명시 다운로드 중 어느 경계를
택할지 사용자에게 물었으며, 응답 없이 기존 `--dump` 의미를 바꾸지 않는다.
GTK 명령의 폐기·alias 승인도 이 합성 진단 구현에 포함하지 않는다.

## 2. 사용과 보안 경계

기존 `floe2-web view`의 **About → Run display test**에서 명시적으로 실행한다.
About를 열거나 view가 바뀐 것만으로 자동 실행하지 않는다. layout 없이도 가능하다.
`gtktest` CLI는 계속 명시 오류이며 `displaytest [PNG]` 대안과 GTK 전용 경계를 안내한다.

M4g-17b는 인덱서·renderd도 없는 환경에서 실행할 독립 명령을 추가한다:

```sh
rust/target/release/floe2-web displaytest
rust/target/release/floe2-web displaytest /path/to/frame.png
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
M4g-17c는 선택적 PNG 인자 하나를 받는다. `--` 뒤는 옵션으로 해석하지 않으므로
대시로 시작하는 이름·공백·한글 경로도 지정할 수 있다. GTK 명령의 alias/폐기나
`--dump` 저장 방식 변경은 아니다.

### 선택 PNG의 고정 읽기

- CLI에서 명시한 일반 파일 하나만 시작 시 읽는다. 읽기 전후 inode·길이·mtime/ctime
  등을 비교하고, 메모리의 PNG envelope/각 chunk CRC를 검증한다. 기존 annotation
  scanner를 공유하지만 **flateyes/iTXt 내용을 해석·압축 해제하지 않는다**.
- 원본을 수정하거나 다시 읽지 않는다. 파일을 교체·삭제해도 이미 열린 세션의 bytes는
  같고, 변경한 파일을 진단하려면 새로 실행한다. symlink는 명시한 CLI 경로에서만
  정규화하며 regular-file 확인은 nonblocking/no-follow open으로 한다. FIFO는 거부한다.
- 한 파일80MiB, 한 축8192px, 총16Mpx,65536 chunks 상한이다. 원본 encoded bytes를
  세션 하나에 보관한다. IDAT의 실제 샘플 디코딩은 브라우저가 하며 손상 샘플은 UI 오류다.
  네이티브 구조 검사 통과를 이미지 디코딩/화면 수용 PASS로 부르지 않는다.
- 정적 PNG의 RGB/RGBA/회색/알파/팔레트/16-bit/Adam7 envelope를 수용한다.
  애니메이션 APNG와 IEND 뒤 trailing data는 명시 오류다. 애니메이션 재생이나 GTK의
  fallback-frame 동작은 이 진단에서 이관했다고 세지 않는다.
- capabilities의 `display_input`은 없으면null, 있으면 `{width,height,bytes}`다.
  **Show input PNG**를 눌러야 인증된 `GET /api/v1/display-test/input`을 읽는다.
  HTTP는 경로·파일명·업로드·source/view ID를 받지 않는다. 일반 view/About에는 입력
  파일이 등록되지 않으며 endpoint는404다. PNG **원본 바이트와 embedded metadata**는
  인증된 session owner에게 전송한다. 익명화/metadata 삭제 기능이라고 오해하면 안 된다.
- 웹은 공통 PNG decoder의 source dimensions 검사·5초 decode 제한을 사용한다.
  Canvas에360×160으로 늘이거나 줄이며 smoothing을 켠다. alpha는 보존하고 화면의
  바탕은 어두운 회색이다. GTK BILINEAR와 브라우저의 보간/색 관리가 같은 픽셀을
  만든다고 단언하지 않는다. device pixel 수와 DPR을 보고한다.
- 입력 검사와 합성 검사는 독립적이다. 입력 하나당 동시 요청1개, 고정 크기/5초 GET
  제한, read→decode 사이 취소, blob URL 회수·pagehide/Quit 정리, 실패 후 미재시도를
  적용한다. 보고서는 dimensions/bytes/출력 alpha가0이 아닌 픽셀 수와 사용자의 관찰을
  포함한다. 숫자 readback만으로 화면이 올바르다고 판정하지 않는다. 파일명/경로/픽셀/
  annotation 텍스트를 보고서에 넣거나 저장/업로드하지 않는다.

### 합성 검사

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
`validate_display_input.py`는11개의 static PNG 형식/팔레트/진짜 Adam7 fixture에서
실제 CLI/HTTP의 원본 bytes와 Pillow 디코딩 픽셀을 대조한다. 실행 후 파일을 지워도
같은 bytes가 오는지, 인증·CRC·큰 크기·trailing data/APNG/FIFO 거부를 검사한다.
DOM gate는 실제 공통 decoder와 대역 Image/Canvas로 배율·alpha readback 연결,
취소/오류/미재실행을 검사한다. 브라우저의 실제 보간 픽셀 대조는 아니다.
전체 배터리 실행 기록은 [M4 §70~72](WEBUI_M4.ko.md)에 둔다. 이를 실제 브라우저 실행으로
대체 보고하지 않는다. 기존 브라우저 시작 경로의 도구 거부도 우회하지 않았다.

다음 잔여는 기존 GTK 진단/애니메이션 PNG의 제품 경계, `--dump` 저장 방식,
실제 Firefox/ETX의 표시/입력 수용이다. 기존 GTK 구현은 보존한다.
