# 웹 표시 진단 이관

2026-09-16, M4g-30. [M0 §2.8~2.9](WEBUI_M0.ko.md)의 `gtktest`/`--dump`
미이관을 실제 코드로 분리한 계약과 현재 구현이다. **합성·정적 입력 PNG 진단은
연결하고 승인된 브라우저 dump도 추가했지만 실제 브라우저 수용·GTK 진단 폐기는 아니다.**

## 1. GTK의 실제 동작

| 경로 | 원래 동작 | 웹 현재 상태 |
|---|---|---|
| `gtktest [png]` | 선택 PNG를360×160으로 bilinear 축소해 표시 | `displaytest [PNG]`: 정적 PNG/APNG 기본 이미지 snapshot을 브라우저 smoothing으로360×160 표시. GTK 보간 픽셀 동일성은 보장하지 않음 |
| `gtktest` 합성 | 검은360×160 RGB pixbuf에 빨강/초록/파랑/노랑70×100 막대4개 | 같은 픽셀의 Rust PNG/raw, 공통 디코더와 Canvas로 대조 |
| `gtktest` 배치 | 같은 pixbuf를 Overlay/ScrolledWindow 안에 표시 | 웹 `.viewport`의 crop/별도 투명 overlay를 표시; GTK 위젯 구조를 복제하지 않음 |
| `view --dump` 수신 | 수신 raw/PNG를 pixbuf로 만든 뒤 `/tmp/<APP>_frame.png`에 덮어씀 | M4g-22: 승인된 decoded raw/PNG 한 장을 브라우저 bitmap으로 보관; 명시 PNG 다운로드 |
| `view --dump` 합성 | overlays 후 `/tmp/<APP>_disp.png`와 widget alloc/mapped/visible 진단 | M4g-22: 최근 viewport crop/합성과 canvas 주석 한 장 보관·명시 다운로드. GTK 위젯/OS 진단을 복제하는 기능은 아님 |

`_display`의 기존 dump는 `_update_labels/_update_note_labels` **전**에 실행된다.
GTK dump가 항상 화면의 모든 주석을 포함한다는 가정도 맞지 않는다.
사용자는 브라우저 최근 이미지 보관/명시 다운로드를 선택했다. 서버 파일 연속
덮어쓰기는 추가하지 않으며 기존 GTK 명령의 동작은 보존한다.
M4g-30 사용자 확정: `gtktest`는 기존 GTK 비교 패키지에만 유지하고 Rust 제품은
`displaytest [PNG]`로 대체한다. Rust의 `gtktest`는 대체 명령을 안내하는 exit2이며
alias/암묵 실행/Python fallback을 추가하지 않는다. GTK 위젯 자체 진단은 이관 대상이
아니지만 실제 브라우저·현장 수용과 GTK 뷰어 전체의 은퇴 조건은 그대로 남는다.

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
- 원본 파일80MiB, 한 축8192px, 총16Mpx, 원본65536 chunks 상한이다. 검증 후 정적
  snapshot을 세션 하나에 보관한다. IDAT의 실제 샘플 디코딩은 브라우저가 하며 손상 샘플은 UI 오류다.
  네이티브 구조 검사 통과를 이미지 디코딩/화면 수용 PASS로 부르지 않는다.
- 정적 PNG의 RGB/RGBA/회색/알파/팔레트/16-bit/Adam7 envelope를 수용한다.
  M4g-29는 APNG의 **IDAT 정적 기본 이미지**도 수용한다. 기본 이미지는 animation의
  첫 프레임일 수도, animation 밖의 별도 이미지일 수도 있다. 이는 애니메이션 재생이
  아니다([W3C PNG3 §4.9](https://www.w3.org/TR/png-3/#apng-frame-based-animation)).
  원본 전체의 길이/CRC/chunk 수 검사를 마친 뒤 메모리에서만 `acTL/fcTL/fdAT`를
  제거한다. 버릴 chunk의 CRC 오류·길이 초과와 IEND 뒤 trailing data도 거부한다.
  animation 순서·제어 본문의 의미는 검증하지 않는다. 해당 의미가 무효여도 정적
  PNG가 유효하면 표시할 수 있다. 취소 가능한1MiB 단위 in-place 이동으로 두 번째
  대형 이미지 버퍼를 만들지 않지만, Vec의 원본 할당 용량은 세션 동안 남을 수 있다.
- capabilities의 `display_input`은 없으면null, 있으면 `{width,height,bytes}`다.
  `bytes`는 원본 파일 크기가 아니라 제공하는 정적 snapshot 길이다.
  **Show static PNG**를 눌러야 인증된 `GET /api/v1/display-test/input`을 읽는다.
  HTTP는 경로·파일명·업로드·source/view ID를 받지 않는다. 일반 view/About에는 입력
  파일이 등록되지 않으며 endpoint는404다. 애니메이션 외의 **원본 chunk와 embedded
  metadata**는 그대로 인증된 session owner에게 전송한다. 정적 PNG는 바이트 불변이며
  익명화/metadata 삭제 기능이 아니다. 주석 편집 `fe-embed`는 APNG chunk도 계속 보존한다.
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
`validate_display_input.py`는11개의 static PNG 형식/팔레트/진짜 Adam7, RGBA/팔레트 ×
기본 이미지 포함/분리의 유효 APNG4개와 opaque animation 본문1개를 대조한다.
실제 CLI/HTTP의 보존 chunk bytes와 Pillow 정적 기본 픽셀을 검사한다. 실행 후 파일을
지워도 같은 bytes가 오는지, 인증·삭제 chunk의 CRC·원본 chunk 수/크기·trailing data/
FIFO 거부도 검사한다. 선택 `--gtk-oracle`은 로컬 GdkPixbuf의 정적 loader로 APNG4개
픽셀을 추가 대조한다(개발용 GI 필요, 없으면 실패). GTK 위젯/축소/브라우저는 실행하지 않는다.
DOM gate는 실제 공통 decoder와 대역 Image/Canvas로 배율·alpha readback 연결,
취소/오류/미재실행을 검사한다. 브라우저의 실제 보간 픽셀 대조는 아니다.
전체 배터리 실행 기록은 [M4 §70~72, §87](WEBUI_M4.ko.md)에 둔다. 이를 실제 브라우저 실행으로
대체 보고하지 않는다. 기존 브라우저 시작 경로의 도구 거부도 우회하지 않았다.

M4g-27 재대조로 GTK 원본이 animation player가 아니라는 점을 확인했고,
M4g-29에서 **정적 기본 이미지** 호환을 연결했다([CLI 재대조 §4](WEBUI_G4_CLI.ko.md)).

GTK 진단의 제품 경계는 M4g-30에서 위와 같이 확정했다. 다음 잔여는
실제 Firefox/ETX의 표시/입력 수용이다. 기존 GTK 구현은 보존한다.

## 4. M4g-22 — 최근 수신/합성 화면의 브라우저 dump

`floe2-web view SOURCE --dump`는 독립 workspace로 시작하고 브라우저의 보관을
초기 활성화한다. 기존 창으로 forward하지 않는다. 일반 실행은 off이며 **About →
Display diagnostics → Keep recent received frame and composed display**로 켤 수 있다.
About를 닫아도 활성화는 유지하고, opt-out·페이지 종료/로그아웃·세션 만료는 해제한다.
BFCache 복귀는 off여서 명시적으로 다시 켜야 한다. 새 페이지 reload는 원래 CLI
옵션을 다시 읽는다. 기능은 숫자 전용 `--render-debug`나 일반 Save view PNG와 별개다.

- 수신: 유효한 현재 WS frame이 정상 디코드되어 canvas에 적용될 때 복사한다. stale/
  hidden/discarded/디코드 실패는 보관하지 않는다. 마지막 **foreground 또는 margin**
  하나이며 목적·원형식·generation·픽셀 크기·incomplete 여부를 표시한다. 원 PNG의
  압축 바이트를 보관하는 것이 아니라 디코딩 결과를 PNG로 내보낸다.
- 합성: `present()`의 현재 margin/foreground 배치와 보이는 query/DRC/ruler canvas를
  viewport device-pixel 크기로 복사한다. pan preview, incomplete/frozen base도 보이는
  그대로 대상이다. 주석의 비동기 paint도 알림을 보내며 한 animation-frame 작업으로
  병합한다. 캡처 전에 기존 overlay flush를 사용하되 네이티브 재렌더·query는 하지 않는다.
  **Capture display now**는 현재 합성만 새로 보관한다.
- 두 이미지는 독립적인 최근 캡처다. 로컬 번호와 합성 시 마지막 수신 번호를 보여
  주지만 동일 시점의 한 쌍이나 동일 generation임을 보장하지 않는다. 여백 수신 이미지와
  화면 crop의 크기도 다르다. 패널·status·CSS 선택 box·OS/ETX 합성 화면은 제외한다.
- 각각 최대16Mpx/한 축8192px(기존 frame contract), 총32Mpx ≈128MiB RGBA bitmap을
  보관한다. 교체 전에 이전 bitmap을1×1로 해제한다. off에는 복사가 없지만 on에는
  수신 시 동기 복사와 합성 복사/overlay flush 비용이 생긴다. 정상 성능 측정에서 끈다.
  브라우저 내부 메모리·기존 표시 canvas까지128MiB로 제한한다는 뜻은 아니다.
- PNG 인코딩은 다운로드 클릭에서만 시작한다. 동시 encoder1개와 retry PNG1개(80MiB
  이하)만 허용한다. 인코더 내부 snapshot은 추가 최대16Mpx이며 기존 화면/브라우저
  자체 할당은 별도다. 이후 프레임으로 canvas가 바뀌어도 클릭 시 bitmap을 인코딩한다.
  다운로드를 요청한 뒤에도 같은 frozen PNG의 명시 재다운로드가 가능하다. 브라우저가
  저장을 완료했는지는 알 수 없으므로 “requested”로 표시한다.
- clear/opt-out/다른 view·close/pagehide/종료가 오면 이미지·PNG·object URL을 해제하고
  진행 중 encoder의 delivery를 취소한다. 실제 `toBlob` 작업은 취소할 수 없으므로
  callback까지 credit을 유지하며 다시 켜도 encoder가 중첩되지 않는다. 이전 source의
  늦은 callback은 다운로드하지 않는다. 이미 사용자에게 내려간 파일은 지우지 않는다.
- WS 단절만으로 이미 보관한 픽셀을 지우지는 않아 단절 당시 상태를 다운로드할 수
  있다. 인증 만료가 확인되거나 페이지/세션을 종료하면 지운다. 브라우저 storage·서버
  파일·업로드·clipboard read는 없다. PNG에는 화면의 설계/주석이 들어가므로 민감한
  파일일 수 있다. 경로·credential·원본 metadata를 이미지 이름에 넣지 않는다.

카탈로그의 `display_dump`는 viewer 지원 여부, `dump_on_start`는 trusted CLI의 초기
선택이다. 새 서버 이미지 저장/다운로드 endpoint나 path DTO는 만들지 않는다.
독립 displaytest 세션에서는 둘 다 false다. 실제 브라우저 PNG/다운로드·물리 화면 수용은
기존 도구 제약을 우회하지 않고 별도 항목으로 유지한다.
