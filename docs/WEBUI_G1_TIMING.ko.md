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

## 5. foreground/margin 도형 픽셀 대조 (0.12.185)

Electron 비교 중 같은 valmini 뷰의 두 Canvas 원본을 직접 대조했다. 캡처 시점·
CSS/브라우저 chrome 차이를 배제하고, 동일 render revision·배율·16px 정수 이동·
완전 포함을 확인한 뒤 foreground 전체 RGBA와 margin의 해당 영역을 비교한다.
픽셀을 재샘플하지 않으며 빈 검정 화면의 일치도 성공으로 인정하지 않는다.

goto200,200,300µm/full/high, raw, DPR2, 1640×1317px, decode4/raster4,
refinement off인 새 합성 세션에서 다음을 확인했다. 프레임/라벨 토글은 이 세션에만
적용되고 원본·캐시 SHA-256은 전후 불변이다.

| 비교 | 수정 전 변경 픽셀 | 수정 후 |
|---|---:|---:|
| 도형만, native pan reuse on | 31 | 0 |
| 도형만, native pan reuse off | 1,817 | 0 |
| 도형+프레임, reuse off | 1,817 | 0 |
| 도형+프레임+라벨, reuse on/off | 59 / 1,844 | 28 / 28 |

원인: stroke 변환이 lower-origin f64에서 `height - 1 - floor(y + 0.5)`를
계산했다. 동일한 0.3µm 상자의 y=305000 DBU 경계가 foreground에서84행,
margin에서는656px 이동을 제외하고83행으로 반올림됐다. fill은 이미 Q32.32라
영향받지 않았다. 재사용은 interior tile의 기존 픽셀을 복사해 차이 대부분을 숨겼다.
대표 도형 경로를 꺼도 같은 차이가 있어 대표화/cut을 변경하지 않았다.

stroke도 기존 top-origin Q32.32를 공유하고 동치 정수식을 쓰도록 수정했다.
새 Rust 단위 테스트는 수정 전 `(13,84) != (13,83)`으로 실패했고, 수정 후
정점과 실제 rectangle foreground/margin 전체 픽셀을 비교해 통과한다.
render-core 단위123개도 통과했다. **공유 CPU renderer 수정**이며 Electron/CSS
보간 설정을 바꾼 것이 아니다. renderd와 기대 버전은0.12.185로 동기화했다.

재현/회귀 명령(런타임·개발 Python 설정은 Electron 문서 참조):

```sh
node tools/validate_electron_layout.cjs --frame-parity
```

항상 새 valmini를 만들어 reuse on/off 두 세션을 실행한다. 라벨 off에서 프레임
on/off 각각 **2,159,880픽셀 RGBA 완전 일치**가 필수다. probe 단위3개는 불일치
집계·alpha·잘못된 배율/범위/revision/숨김 및 예제8개 상한을 검사한다. 예제 개수는
진단만 제한하며 전체 픽셀 비교에는 상한/허용 밴드/skip이 없다.

라벨 on의28픽셀은 화면 하단 두 행에 남으며, 도형 gate로 라벨 parity까지
통과했다고 해석하지 않는다. 후속 §6에서 해당 차이를 기존 사용자 수용 규약의
margin-only label 꼬리로 확인했다. 예전 screenshot848,168픽셀 차이와 이번 원본 Canvas 비교는
범위·시점이 다르므로 동일 원인이라고 단정하지 않는다. WK 동일 조건 대조 및
실제 input→photon/G1 성능 수용도 여전히 별도다.

로그: `/private/tmp/floe-stroke-half-before.log`, `floe-stroke-half-after.log`,
`floe-electron-frame-parity-final.log`. 좌표/RGBA 예제는 명시적 합성 QA 산출물에만
남으며 제품 진단·일반 사용자 세션에는 수집 기능을 추가하지 않았다.

추가 회귀: KLayout 오라클은 jobs1/8 각각 **13 PX + 2 phase-exact + 14 style**를
통과했다(`floe-stroke-half-klayout-j1.log`, `floe-stroke-half-klayout-j8.log`). 기존
Electron pan·clip·복구·storage/cookie 상실 검사도 `floe-electron-layout-185.log`에서
통과했다. 필수 전체 `validate_rust.sh`는 **exit1**로 종료했다. workspace unit·CLI·
캐시 이동·버전/portable·embedded·app read 뒤 기존 `layerprops` native oracle의
30초 timeout이다(`floe-stroke-half-full.log`). 제한 완화/생략은 하지 않았고 전체
green으로 간주하지 않는다. 이번 실행의 timeout을 이전 dyld sample과 동일 원인이라고
확정하지 않는다. strict render-core/renderd clippy는 기존 private-interface/dead-code/
style 경고로 **37건 실패**다(`floe-stroke-half-final-clippy.log`). 새 테스트의 불필요한
clone 경고만 수정했고 재검사에서 사라졌으며 기존 경고를 숨기거나 일괄 수정하지 않았다.

## 6. 라벨 28픽셀의 원인과 기존 수용 규약 재대조

2026-09-21, 현재 제품0.12.185. **새 renderer 결함으로 분류했던 판단을 정정한다.**
[F2R-21의 사용자 결정](FLOE2_OPTIMIZATION.ko.md#326-리뷰-medium-2건--margin-crop-라벨-정확도-게이트-질의-스레드-2026-09-05)
(2026-09-05,0.12.53)은 빠른 라벨 포함 pan을 위해 다음 차이를 명시적으로 수용했다:
margin 착지 때 viewport 밖·margin 안의 label anchor에서 뻗은 글리프 꼬리가 보이고
기존 라벨을 덮을 수도 있다. `labels_truncated` margin은 여전히 crop에서 제외한다.
도형 픽셀 차이까지 수용한 것이 아니며, §5의 stroke 수정/엄격 도형 gate는 유효하다.

합성 valmini의 CLI label selection을 같은 배율에서 직접 비교했다:

- foreground DBU box `[50000,79542.6829…,350000,320457.3171…]`,1640×1317px.
- declutter bin은 `ceil(48/(1640/300000)) = 8781 DBU`다. 아래쪽 정렬 경계는
  `9×8781 = 79029 DBU`다.
- `M496`(layer63/63)의 anchor는 `(70088,78925)`다. foreground의 정렬 경계보다
  104DBU 아래여서 선택되지 않고, margin 계획에는 들어간다. 글리프는 화면 하단에
  도달한다. 실제 차이의 범위는 x93..124,y1315..1316이며 총28픽셀이다.
- CLI 선택은 foreground260행/margin530행, 양쪽 `truncated=false`였다.
  로그 `/private/tmp/floe-label-{foreground,margin}.tsv`의 `M496` 행으로 확인했다.
- 엄격한 *라벨 포함* 동일성을 임시 하네스에서 요구하면 기존 바이너리는 예상대로
  exit1이다(`floe-label-halo-before.log`). 도형-only 두 비교는 같은 실행에서0픽셀이다.
  이는 이번에 새로 만든 결함이 아니라 기존 규약과 새로 요구한 oracle이 서로 다름을 보인다.

글꼴 최대 ink 범위와 source text 길이로 조회 여백을 확대하는 실험도 만들었다.
이는 새 형식/재인덱싱 없이 가능하지만, viewport 외곽 라벨을 더 선택해 plan/raster
비용과 기존 cap 소진 가능성을 늘리고 직접 렌더의 label 선택 규약도 바꾼다. 따라서
웹 이관의 필수 수정으로 편입하지 않았다. **제품 코드·버전·기존 label oracle은
원래대로 유지**하며, 실험 패치만 `/private/tmp/floe-label-halo-trial.patch`에 보관했다.
이 임시 파일은 배포 산출물이나 지원 옵션이 아니다. 실험의 font/VFS 단위 통과를
현재 제품 또는 실제 픽셀/성능 수용 통과로 계산하지 않는다. 재채택에는 긴 라벨·
다층 overlap·dense/cap·여러 font 크기의 표시/성능 비교와 정책 결정이 필요하다.

복원 후 release index/renderd/render-cli와 Electron Rust helper를 다시 빌드하고,
ready 응답의0.12.185를 확인했다. 새 valmini의 `--frame-parity`는 reuse on/off에서
각각2,159,880 RGBA픽셀의 도형·도형+프레임 차이0, 라벨 포함 차이28로 **exit0**이다
(`floe-label-policy-restored-parity.log`). source/cache 불변과 정상 종료도 통과했다.
이 실행은 기존 표시 규약 재검증이며 전체 Rust/web battery 재실행은 아니다.

G1의 미완료 범위는 동일 조건 GTK/WK/Electron의 실제 input→photon·pacing,
margin 안 검은 strip/라벨 지연, 현장 수용이다. **모든 라벨 포함 foreground/margin
픽셀 동일**을 기존 목표에 새 필수 조건으로 추가하지 않는다. 초기 프레임 대기 실패는
이 28픽셀 현상과 별개이며 Electron 시작 대기 기록에서 계속 추적한다.
