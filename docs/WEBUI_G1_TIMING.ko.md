# G1 — 브라우저 구간 계측

작성: 2026-09-21. 현재 범위: owner 웹 뷰어와 같은 UI를 쓰는 Electron.
관련 정본: [웹 전환 계획 §6](WEBUI_PLAN.ko.md#6-성능-요구와-게이트),
[데스크톱 계획](WEBUI_DESKTOP.ko.md).

2026-09-23: WKWebView 동결에 따라 아래 WK 비교 이력은 기준선으로만 보존한다.
이후 성능 수용은 GTK 대비 Electron/웹을 대상으로 하며 WK 성능 개선이나 교차
대조 완료를 기다리지 않는다. G1 input→photon/pacing 기준은 완화하지 않는다.

2026-09-22 후속: [실제 활성 창의 WK/Electron 대조](WEBUI_DESKTOP.ko.md#25-활성-창에서의-전체-native-검사와-호스트-대조-2026-09-22)에서
같은1600×1200/DPR2/world bbox·workers·새 합성 소스/캐시로 pan reuse on/off ×
세 단계 RGBA가 모두 일치했다. 이는 §8~9의 숨김 실패 뒤 얻은 픽셀 동등성 근거다.
input→photon/pacing 측정은 하지 않았으므로 **G1 성능 게이트는 계속 미완료**다.

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

## 7. WKWebView의 실제 Canvas parity 기준선

2026-09-22. 제품 렌더 정책/기본값/버전0.12.185는 그대로다. 앞선 Electron
전용 gate에 대응하는 macOS 네이티브 QA를 추가했다. **G1 속도 비교 통과가 아니라
각 호스트 안의 foreground/margin 정확도 검사**다.

`tools/validate_native_frame_parity.cjs`는 인자를 받지 않는다. 새0700 임시 폴더에
valmini를 생성·색인한 뒤 같은 소스·캐시·release index/renderd를 WKWebView와
Electron에 순서대로 전달한다. decode/raster4, detail high, full depth,
goto200,200,300µm, refinement off, raw, pan reuse on/off를 고정한다.
각 창은 닫기 취소→명시적 종료→service join을 마쳐야 하며, 마지막에 소스·캐시
SHA 목록이 시작 전과 같은지 확인한다. 클립보드·리뷰 저장·기존 기본값은 사용하지
않는다. Python/KLayout은 **개발 fixture 생성에만** 쓰이고 두 앱 런타임에는 없다.

네이티브 `--smoke-frame-parity-test ABS_SOURCE`는 이 드라이버용 명시적 read-only
QA다. 기존 view/session 파서를 사용하며 새 쓰기 권한·JS→native IPC는 없다.
일반 시작 시에는 실행되지 않는다. 새 JS wrapper는 visible/인증 완료/Live margin
crop/같은 render revision을 기다리고 두 rAF 뒤 `frame-parity-probe.js`를 호출한다.
labels off, frames off 전환은 **새 revision**의 margin이 도착해야 비교한다.
geometry+frames, geometry-only는 모든 RGBA의 차이0과 nonempty를 요구한다.
labels 포함은 §6의 수용 규약대로 차이만 기록한다. 실패/불완전/hidden은 통과할 수
없으며, 단계30초·기존 native 전체120초 deadline을 늘리거나 재시도하지 않는다.
호스트 밖에는 순서·크기·DPR·lit/changed의 검증된 숫자와 고정 verdict만 출력한다.

실행 예(먼저 desktop 및 Electron helper를 offline build하고 검증된 runtime을 준비):

```sh
FLOE_QA_PYTHON_BIN=/absolute/dev-venv/bin/python \
FLOE_ELECTRON_BIN=/absolute/Electron.app/Contents/MacOS/Electron \
FLOE_ELECTRON_SERVICE_BIN="$PWD/electron/service/target/debug/floe-electron-service" \
FLOE_ELECTRON_DOWNLOAD_BIN="$PWD/electron/service/target/debug/floe-electron-download" \
    node tools/validate_native_frame_parity.cjs
```

실제 macOS arm64/DPR2 실행(`floe-native-parity-first.log`, exit0):

| 호스트 | 실제 Canvas | labels 포함 차이 | geometry+frames / geometry 차이 |
|---|---|---|---|
| WKWebView | 1840×1382 | 0 | 0 / 0 |
| Electron44.4.3 | 1640×1317 | 28 | 0 / 0 |

두 pan-reuse 설정 각각 위 결과이며, 합성4세션이 정상 종료했다. WK의 도형 비교는
각2,542,880픽셀, Electron은 각2,159,880픽셀이다. 입력 소스·캐시는 바뀌지 않았다.
WK의 기본 window content와 Electron의 outer window 크기, UI font/layout이 달라
viewport가 같지 않다. 따라서 lit 수나 label 차이를 **엔진 차이로 해석하지 않는다**.
공유 픽셀 비교 코드를 사용했지만 두 호스트의 이미지끼리 비교한 것도 아니다.

검증: native40단위, JS7검사, desktop strict clippy/fmt 통과. geometry mismatch,
숨김/미준비/오래된 revision, zero-lit, 비정상·초과 숫자, 잘못된 QA 인자를 거부한다.
이 단계에서 전체 Rust/web battery를 다시 돌린 것은 아니다. 기존 전체 실행의
`layer_defaults`30초 timeout 기록은 남아 있다. 다음 G1 단계는 **동일 물리 viewport·
DPR·화면 옵션을 고정한 cross-host 비교**, 실제 입력→첫 반응/완료·pacing·메모리,
GTK 기준선과 RHEL/ETX 현장 수용이다. 화면 제어 연동의
`CUA_REPL_ENABLED_SURFACES is required`도 해소되지 않았으므로 물리 입력 검증으로
계산하지 않는다.

## 8. 같은 viewport의 WKWebView/Electron RGBA 대조와 초기 가시성 실패

2026-09-22. §7 드라이버를 **cross-host 원시 픽셀 대조**로 확장했다. 제품 버전,
일반 창 크기, resize/cut/라벨 정책은 바꾸지 않았다. 명시적 parity QA와
`FLOE_QA_CROSS_HOST=1`을 함께 사용할 때만 공유 script가 viewport를800×600 CSS px로
고정한다. DPR은 조작하지 않고, 실제 Canvas 크기가800×DPR/600×DPR가 되어야 한다.
`validate_native_frame_parity.cjs`는 이 모드를 설정하며 사용 명령은 §7과 같다.
환경변수 없는 기존 Electron/WK 단독 parity QA는 기존 창 크기로 동작한다.

두 호스트가 같은 `layout-parity-probe.js`와 `frame-fingerprint-probe.js`를 실행한다.
foreground 전체와 margin의 정렬된 crop을 `getImageData`로 읽어 SHA-256을 계산한다.
기존 per-host 도형 RGBA 차이0 조건도 유지한다. digest 중 frame ID/revision/bbox가
바뀌거나 숨김으로 전환되면 실패다. PNG 디코딩·CSS 리샘플링·창 screenshot/도구바는
비교에 섞지 않는다. 픽셀 배열·해시는 **명시적 합성 검사에만** 읽으며 새 API,
JS→native IPC, 파일/클립보드/리뷰 권한은 만들지 않는다.

비교기는 세 phase가 정확히 한 번씩 순서대로 있어야 한다. 물리 크기·DPR·DBU bbox와
각 counterpart의 해시가 호스트 사이에서 같아야 한다. valmini의1nm DBU,
goto200,200,300µm와4:3으로 유도되는 `[50000,87500,350000,312500]`도 단언한다.
즉 두 호스트가 *같이 틀린* 카메라를 반환해도 통과하지 않는다. 라벨 포함에서
foreground와 margin이 서로 다른 기존 수용 규약은 유지하되, **같은 foreground끼리,
같은 margin crop끼리의 cross-host 해시**는 라벨 포함에서도 일치해야 한다.

### QA 순서 수정과 실제 관측

처음에는 CSS resize와 초기 goto를 동시에 진행했다. 하지만 제품
`ViewState::resize`는 **배율을 유지**하므로 픽셀 폭 변경 후에도 월드 폭이300µm라는
가정이 틀렸다. 최종 순서는 초기 Live margin 착지→CSS resize→실제 크기의 새 프레임
착지→필요할 때 기존 Go 버튼으로200,200,300을 한 번 적용→세 phase 대조다.
가시성/시간 제한을 완화하거나 제품 resize를 변경하지 않았다. 최초 렌더 비용을
측정하는 cold-start benchmark가 아니며, 이 준비 비용을 성능 개선으로 세지 않는다.

- `floe-native-cross-first.log`: 첫 WK 검사 실패. 단계 진단 전이므로 세부 원인 미확정.
- `floe-native-cross-phase.log`: reuse on에서 양쪽 해시 일치, off에서는 Electron
  `layout-failed-ready0`, exit1. 전체 통과로 세지 않는다.
- `floe-native-cross-ordered.log`: 순서 수정 뒤 **4세션/두 reuse 설정 exit0**.
  두 호스트는 각각1600×1200px/DPR2/위 고정 bbox였고, labels+frames+geometry,
  frames+geometry, geometry 세 phase 모두 counterpart 해시가 일치했다.
  라벨 포함 해시 접두부 `69220836d4ba298c`, 도형 두 phase `fb6b7b9bb3189208`이다.
  foreground/margin 간에도 이 뷰에서는 차이0이었다. 소스·캐시는 불변이고 종료/join 통과.
- 카메라·margin 검사를 추가한 **최종 코드의 실제 재검사는 아직 green이 아니다**.
  `floe-native-cross-final.log`와 추가 진단 `floe-native-cross-state.log`는 WK의
  `layout-failed-initial`, exit1이다. 후자의 고정 상태는
  `mask=3118, canvas=1×1, DPR=2, goto=200,200,300`이었다. 인증은 끝났지만
  `document.hidden=true`, frame ID가 없고 Live margin도 없었다. **CSS resize나
  digest 실행 이전**이다. OS 창이 실제로 가려졌는지/WebKit 가시성 전달 문제인지는
  화면 제어 연결 없이 확정하지 않는다. hidden을 강제로 false로 만들거나 재로드,
  시간 연장, 자동 재시도를 추가하지 않았다. 모든 테스트 프로세스는 종료됐다.
- 최종 비교기로 `ordered`의 on/off 기록을 다시 검사해 고정 카메라·해시 계약 통과를
  확인했다. 이는 **기록 재검사**이며 마지막 실제 native 실행 실패를 대체하지 않는다.

실패 진단은12bit mask와 canvas 크기/DPR/goto 숫자만 내보낸다. bit0부터 순서대로
visible, hash 제거, logout enabled, fit enabled, empty hidden, rendering hidden,
Live margin crop, margin visible, foreground ID, margin ID, 두 revision 같음,
prefetch 아님이다. `revision 같음`만으로는 두 ID/유효 revision의 존재를 증명하지
못한다. 단계명(initial/configure/resize/goto/ready/compare/fingerprint)과 함께 읽는다.
원문 notice·경로·레이어명·인증값·리뷰 본문은 출력하지 않는다.

최종 pure 검증은 desktop42단위, 선택 JS17, strict desktop clippy/fmt 통과다.
새 단위는 양쪽이 같은 잘못된 bbox, 다른 DPR, 누락/중복 phase, 해시 변경,
resize 후 월드 폭 변경, digest 도중 frame 교체를 거부한다. 기존 의존성 경고는
유지했다. 전체 Rust/web battery는 이번 단계에서 재실행하지 않았으며 앞선
native oracle30초 timeout도 해결되지 않았다.

남은 G1: 초기 가시성 실패를 재현·해결한 최종 matrix 재통과, 같은 조건의 입력→첫
반응/완료·pacing/RSS/CPU 비교, 실제 물리 입력·compositor/ETX 표시, GTK 기준선이다.
이번 해시 일치로 성능 동등성이나 RHEL 호환성, 전체 goal 완료를 주장하지 않는다.

## 9. 초기 숨김 실패의 AppKit 대조

2026-09-22, 로컬 macOS26.5.2. 합성 QA에만 `window_visibility::snapshot`을 추가해
WebView 부착 직후, 첫 유효 frame, 실패 시점의 **소유 앱/창/view** 상태를 읽는다.
타 앱의 창 목록·제목·화면 내용·PID·경로는 조회하지 않는다. 창 활성화/전면 배치,
hidden override, reload, timeout 연장은 추가하지 않았다. 일반 실행에는 이 출력이 없다.

새 합성 실행 `/private/tmp/floe-native-window-state.log`는 초기 대기에서 **exit1**이다.
WebView 부착 직후와30초 단계 실패 직후가 모두 다음 상태였다:

| 관측 | 값 |
|---|---|
| AppKit app active / hidden / unoccluded | false / false / false |
| window visible / key / main / miniaturized / unoccluded | true / false / false / false / false |
| WebView attached to owned window / is current content view | true / true |
| view or ancestor hidden | false |
| view bounds / visibleRect 크기 | 1200×850 / 1200×878 |
| WebKit document.hidden / Canvas / 실제 frame ID | true / 1×1 / 없음 |

`NSWindow.isVisible`은 다른 창에 가려져도 true일 수 있다. `occlusionState`의 Visible
bit가 없으면 AppKit은 창 전체가 가려졌다고 판정한다.
[Apple occlusionState](https://developer.apple.com/documentation/appkit/nswindow/occlusionstate-swift.property).
따라서 **이번 실행**은 WebKit만 잘못 hidden을 반환한 경우가 아니다. WebView의
부착 실패·명시적 NSView 숨김·최소화도 위 값과 맞지 않는다. `visibleRect`가 비어 있지
않다는 사실을 실제 모니터 노출 증거로 사용하지 않는다. 어떤 창/Space/환경이 가렸는지,
모든 과거 실패가 같은 원인인지는 이 진단으로 확정하지 않았다.

현재 제품은 시작 시 활성화를 요청하지만 macOS14부터 활성화는 사용자 의도에 따른
요청이며 항상 허용되는 것은 아니다. deprecated `activateIgnoringOtherApps`를
반복 호출하거나 보안/포커스 정책을 우회하는 수정을 하지 않았다.
[Apple AppKit14 release notes](https://developer.apple.com/documentation/macos-release-notes/appkit-release-notes-for-macos-14).
이 공식 정책이 **이번 비활성 상태의 구체적인 시스템 원인**이라고 단정하지도 않는다.

기존 `app.js`는 hidden 문서에 온 frame을 decode/paint하지 않고 discarded ACK한다.
다시 보이면 기존 visibilitychange 경로가 연결을 새로 수립한다. 이번 작업에서는
그 계약을 변경하지 않았다. 다음 실제 검사는 사용자가 해당 합성 Floe2 창을 직접
선택하고 가리지 않는 조건, 또는 화면 제어 연결 복구 후 진행한다. 무인 반복 실행으로
우연한 성공을 모으지 않으며 기존 실패는 matrix 미통과로 남긴다.

검증: desktop42단위와 `native-confirmation-qa`를 포함한 all-target strict clippy/fmt
통과. 실제 빈 AppKit 창의 음성/양성 대조도 **exit0**였다
(`/private/tmp/floe-native-visibility-properties.log`): 진단 읽기가 숨겨진 창을
드러내지 않음, 제품의 명시적 reveal 이후 isVisible 변화, 부착/content 동일성,
기존 최소화/시트 보존 및 Return/Enter 기본취소를 확인했다. 이 테스트는 WebView,
서비스, 설계/리뷰 파일을 사용하지 않으며 실제 사용자 키 입력의 수용은 아니다.

이번 화면 제어 재확인도 `CUA_REPL_ENABLED_SURFACES is required`였다. G1 픽셀
매트릭스 최종 재통과·성능/물리 입력 수용, 전체 Rust/web gate, RHEL/ETX 및 저장
복구/배포 검증은 여전히 남는다. 제품 버전은0.12.185, 변경은 QA/진단뿐이다.
