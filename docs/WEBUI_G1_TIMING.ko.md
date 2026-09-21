# G1 — 브라우저 구간 계측

작성: 2026-09-21. 범위: owner 웹 뷰어와 같은 UI를 쓰는 WKWebView.
관련 정본: [웹 전환 계획 §6](WEBUI_PLAN.ko.md#6-성능-요구와-게이트),
[데스크톱 계획](WEBUI_DESKTOP.ko.md).

## 1. 목적과 수용 경계

기존 상태줄의 `plan/decode/draw/native wall/queue`는 Rust 서비스·렌더러의
시간이다. 이 값만으로 브라우저의 입력 대기·이미지 준비·Canvas 반영 비용을
판정할 수 없어, 기본 off인 로컬 진단을 추가했다. 렌더 정책·65ms 입력 throttle·
프레임 ACK 순서·release-only pan/band 동작은 변경하지 않는다.

이 진단은 **G1을 조사할 도구이며 G1 통과 증거가 아니다**. JS 콜백 구간과
Canvas/CSS 호출 반환까지를 측정한다. 운영체제 입력 발생→JS 전달 대기,
compositor/화면 scanout·ETX 전송·모니터 photon은 측정하지 않는다.
서로 다른 표본의 지연을 더해 input→photon으로 보고하지 않는다.

시간은 `performance.now()`의 상대 단조 시계를 쓴다. 해상도는 브라우저의
제한을 따르며 소수 셋째 자리 출력은 실제 1µs 정확도를 보장하지 않는다.
[MDN performance.now](https://developer.mozilla.org/en-US/docs/Web/API/Performance/now).
`requestAnimationFrame`은 repaint **이전** 콜백이고 숨김 상태에서 멈출 수 있다.
따라서 콜백 간격은 표시 FPS가 아니며, 추가 rAF를 기다린 값도 photon의
대체 증거로 쓰지 않는다.
[MDN requestAnimationFrame](https://developer.mozilla.org/en-US/docs/Web/API/Window/requestAnimationFrame).

## 2. 사용법과 보관 정책

1. 합성 레이아웃을 열고 **About → Browser timing → Record browser callback timings**를 켠다.
2. About을 닫고 정한 goto/커서 pan/드래그를 수행한다.
3. About을 다시 열고 **Show / refresh measurements**로 JSON을 확인한다.
   자동으로 갱신하지 않아 보고서 DOM 갱신이 매 프레임에 섞이지 않는다.
4. 체크를 끄면 미완료 표본을 `interrupted`로 끝내고 기록을 중단한다.
   **Clear measurements**는 표본과 미완료 계측을 지운다. 실제 입력·렌더 작업을
   취소하는 버튼은 아니다. 다시 켜면 새 기록을 시작한다.

최근 완료 표본 256개와 진행 중 계측 128개만 유지한다. 넘치면 오래된 완료
표본을 덮어쓰거나 새 계측만 생략한다(`overwritten`, `untracked`). 렌더링을
생략하거나 늦추기 위한 budget이 아니다. 꺼져 있으면 시계 호출·표본 할당을
하지 않지만 연결된 가벼운 조건 검사까지 없어지는 것은 아니다.

기록 내용은 상대 시간·고정 분류 문자열·final 여부뿐이다. 좌표·뷰/프레임 ID·
소스 경로·레이어명·픽셀·리뷰 본문·인증값은 보관하지 않는다. 파일 작성,
local/sessionStorage, 업로드/API, 자동 다운로드는 추가하지 않는다. 사용자
설정 저장과도 독립적이다. guest Follow/Explore·독립 displaytest에는 적용하지 않는다.

숨김 전환은 자동 off, pagehide는 off 후 삭제한다. 뷰 교체/닫기의 buffer reset도
표본을 삭제한다. 연결 종료는 진행 표본만 interrupted로 닫으며, 재접속 때
기록 설정이 켜져 있더라도 이전 연결의 늦은 콜백은 새 표본을 만들지 못한다.
WebKit이 실제로 보이는 창을 hidden으로 판단하는 현상도 우회하지 않는다.

## 3. 측정 필드

JSON은 `format=floe.browser-timing`, `version=1`이다. 서로 다른 종류를 한
히스토그램에 섞지 않는다. 없는 경계나 취소·실패로 끝나지 못한 성공 구간은
0이 아니라 `null`이다. 공통 `at_ms`는 기록 시작 기준 완료 시점,
`elapsed_ms`는 해당 표본 시작→종료(성공 또는 중단)다.

| kind | 필드 | 실제 측정 경계 |
|---|---|---|
| edit | queue_ms | 허용된 edit의 큐 진입 → WebSocket.send 반환. 65ms throttle·앞선 edit 대기 포함 |
| edit | send_to_ack_ms | send 반환 → 해당 edit의 accepted 응답 처리 |
| edit | ack_to_snapshot_ms | accepted 처리 → 일치하는 authoritative state snapshot에서 edit 확정 |
| edit | queue_to_snapshot_ms | 큐 진입 → 위 확정. 프레임 도착/표시 완료가 아님 |
| frame | packet_ms | 수신 binary frame의 JS 처리 진입 → 패킷·현재 상태/credit 검사 완료 |
| frame | decode_ms | 위 검사 완료 → raw 준비 또는 PNG onload/error/cancel 콜백. 순수 decoder CPU 시간이 아님 |
| frame | submit_ms | 준비 콜백 → Canvas 반영·상태 갱신·present 호출 완료. raw ImageData 구성/putImageData도 여기에 포함 |
| frame | receive_to_submit_ms | JS frame 처리 진입 → 위 반영 완료. 네트워크 수신 시작 시점이 아님 |
| preview | event_to_submit_ms | 마지막으로 반영할 move/up update 처리 진입 → pan CSS/밴드 overlay 갱신 호출 반환 |
| preview | submit_interval_ms | 같은 gesture·종류 안에서 연속 preview 호출 완료 간격. 실제 표시 간격이 아님 |

edit 시작은 입력 이벤트 진입이 아니라 validation·기존 margin 준비 **이후**의
큐 진입이다. ACK만으로 표본을 성공 종료하지 않는다. 성공은 `accepted`,
거부·다른 revision·취소는 `interrupted`다. action은 pan/zoom/goto/fit/band/
display로 제한하며 body는 기록하지 않는다.

frame은 foreground/margin, raw/png, final 여부를 구분한다. 유효성 검사에
통과해 decode에 진입한 프레임만 추적한다. 잘못된 패킷·현재 뷰와 불일치하는
프레임은 성공 표본에 들어가지 않으며 전체 수신/폐기 통계를 대신하지 않는다.
완료 outcome은 `submitted`, 미반영·decode/반영 실패는 `discarded`, 기록 중단은
`interrupted`다. `submitted`도 margin이 현재 foreground를 실제로 덮었거나
모니터에 보였다는 뜻은 아니다. 반영 중 예외는 일부 Canvas 변경이 있었어도
성공으로 세지 않는다. 기존 ACK disposition은 계측에서 변경하지 않는다.

preview는 기존 rAF에 실린 마지막 move를 기준으로 한다. 먼저 합쳐진 입력의
대기는 측정하지 않는다. 기존 mouseup의 pan preview는 `trigger=release`,
rAF preview는 `trigger=raf`다. 드래그 취소/완료에서 간격 기준을 초기화하므로
두 gesture 사이의 휴지 시간을 pacing으로 세지 않는다.

## 4. 검증과 남은 G1 수용

- 순수 계측: packet 3 + decode 12 + submit 4 = 19ms, 입력 queue 65/ACK 10/
  snapshot 5ms의 정확한 경계; disabled/비정상 시계, 256/128 상한, 삭제·재시작·
  늦은 콜백, 메타데이터 배제.
- 실제 app.js mock 통합: raw/PNG/margin/불일치, 패킷·draw 비용 주입,
  기존 65ms throttle 유지, ACK 단독 미확정, snapshot 이후 종료,
  present 실패·hidden·pagehide 및 설정 버튼의 네트워크 비발생.
- gestures: 기존 release-order 46개 사례와 coalesced move의 마지막 입력 시각,
  단일 rAF/단일 pan 제출 보존.
- Rust transport: 실제 loopback에서 버전 지정 자산 제공·MIME·HTML 참조와
  잘못된 bundle/임의 파일 경로 거부.

2026-09-21 실행 결과:

```sh
cd rust
cargo fmt -p floe-web -- --check
cargo clippy -p floe-web --offline --locked --lib --tests --no-deps -- -D warnings
cargo test -p floe-web --offline --locked --lib
cargo test -p floe-web --offline --locked --test transport
cd ..
sh tools/validate_rust.sh --only web_ui,web_selfcheck,embedded_host,validation_selector
```

fmt·대상 clippy 통과, lib 123 passed/3 ignored, transport 15 passed. 선택 배터리는 4개 모두
`RUST VALIDATION: ALL OK`로 끝났다. 임시 `.venv` 링크는 검사 뒤 제거했다.
기존 VFS dead-code 경고는 남아 있다. 전체 배터리 재실행이나 실제 GUI 통과를
뜻하지 않는다. Node mock·HTTP 통합 검사는 실제 Chrome/Firefox/WKWebView의
레이아웃/포커스/표시 성능 수용을 대체하지 않는다.

G1을 닫으려면 같은 합성 fixture·같은 renderd/options·같은 viewport 물리 px/
DPR·레이어·depth·detail·thin·refinement·margin 조건에서 GTK와 web/native를
대조해야 한다. cold와 warm, exact 재방문과 인근 pan, foreground와 margin,
raw와 PNG를 분리하고 같은 입력 trace를 여러 번 수행한다. 계측 on/off 비용도
확인한다. JS 진단은 병목 위치를 좁히는 보조 자료로만 사용하며 input→photon과
실제 표시 pacing ±10%, margin 안 새 strip 검정/라벨 지연 0의 화면 증거는 별도로
필요하다. 실제 장치·ETX 측정 없이 추정 수치나 합성 지연을 성능 결과로 쓰지 않는다.
