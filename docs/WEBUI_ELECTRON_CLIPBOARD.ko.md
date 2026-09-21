# Electron 클립보드 수용 — E2d

2026-09-21. [Electron 비교판](WEBUI_ELECTRON.ko.md)의 추가 기능이며 기존 WKWebView
호스트/웹 snapshot 계약을 바꾸지 않는다. 사용자 승인: **별도 합성 세션에서 테스트
문자열·PNG로 OS 클립보드를 덮어쓰고, 방금 쓴 테스트 값만 확인**. 기존 내용은
읽거나 백업/복원하지 않는다. 테스트 PNG가 종료 후에도 클립보드에 남는다.

## 제품 경계

- Session permission check는 항상 false다. 영속/주변 권한을 부여하지 않는다.
  요청에서는 `clipboard-sanitized-write`만 검토하고 읽기·그 밖의 권한은 거부한다.
- 소유 WebContents의 정확한 root URL/main frame, 보이며 focus된 창,
  대화상자·종료·실패·복구 중이 아님을 확인한다. 별도 download 창에는 허용하지 않는다.
- renderer와 분리된 isolated world 1001에서 Chromium의 현재 transient activation,
  document visibility/focus **세 boolean 조건만** 조회한다. `userGesture:false`로
  실행하며 activation을 만들어 주지 않는다. 페이지 JS의 같은 이름 override로 이
  판정을 대신하지 않는다. clipboard 내용·인증값·경로를 조회하지 않는다.
- 응답은 750ms 안에서만 승인하고 완료 시 소유/상태를 다시 검사한다. blur·navigation·
  복구·종료가 pending 응답을 무효화한다. timeout 후 늦은 응답은 승인하지 않으며
  실제 probe가 끝날 때까지 그 한 개의 실행 슬롯을 유지한다. 자동 재시도하지 않는다.
- transient activation은 사용자의 최근 입력을 뜻하지, 호출이 반드시 특정 Copy 버튼에서
  왔다는 보장은 아니다. 이 정책은 같은 소유 페이지의 활성 입력에 대한 쓰기만 허용하며,
  해당 JS 자체의 악성 변경을 방어하는 별도 origin 격리라고 주장하지 않는다.
- PNG 합성·80MiB 한도·취소·실패 시 frozen PNG 다운로드 안내는 기존 웹 snapshot이
  담당한다. native host에 별도 이미지 합성/clipboard payload 전달 IPC를 추가하지 않는다.
  Node integration/preload/native bridge는 계속 없다. 표준 Edit Copy/Paste는 유지한다.

Electron의 [permission 요청/검사](https://www.electronjs.org/docs/latest/api/session),
[요청 정보](https://www.electronjs.org/docs/latest/api/structures/permission-request),
[isolated world 실행](https://www.electronjs.org/docs/latest/api/web-contents#contentsexecutejavascriptinisolatedworldworldid-scripts-usergesture)을
기준으로 구현했다. permission request에 JS userGesture 필드가 있다고 가정하지 않는다.

## 명시적 검사

일반 smoke/CI 명령은 클립보드를 덮어쓰지 않는다. 아래 옵션을 명시해야 새 valmini와
새 Electron 세션을 만들고 테스트 문자열·PNG를 쓴다. 실행 전에 기존 클립보드가
보존되지 않음을 확인할 것. 실제 설계/DRC/default/외부 서버에는 접근하지 않는다.

```sh
# runtime·Rust helper·개발 Python 환경은 Electron 문서의 개발 검사와 동일
node tools/validate_electron_layout.cjs --clipboard
node --test electron/clipboard-controller.test.cjs
```

macOS arm64 / Electron 44.4.3 합성 검사:

- 사용자 활성화 없는 쓰기 거부 → 주입된 Chromium mouse input으로 한글 포함
  테스트 문자열 쓰기 성공 → 같은 테스트 문자열만 native API로 읽어 일치 확인.
- 프로그램식 `readText`/`read`는 모두 거부. 이 음성 검사조차 원래 클립보드가 아닌
  위 테스트 값을 쓴 이후에만 수행한다. 읽기 성공 회귀가 있어도 내용을 출력하지 않는다.
- 테스트 textarea의 native Copy/Paste가 다른 테스트 문자열을 정확히 전달한다.
- 실제 Copy view PNG 버튼 및 캔버스 Cmd+C 주입으로 각각 1640×1317px PNG를 복사한다.
  하네스는 Linux에서는 Ctrl+C를 선택하되 Linux 실측은 아직 없다. QA가 encoder callback의
  blob을 변형 없이 관찰하고, 실제 OS PNG를 같은 decoder로 bitmap 변환해 **모든 픽셀을
  완전 비교**한다. 이후 새 그림/화면 캡처와 대조하지 않으며 빈 화면도 통과하지 않는다.
- activation 만료 후 쓰기가 다시 거부됨을 확인한다. source/cache SHA-256 inventory
  불변, 탐색·새 창 차단, 정상 취소/종료/서비스 join도 함께 확인한다.
- controller 6개는 잘못된 권한/URL/소유/하위 frame·비활성 상태, throw/무응답,
  중복 요청, 늦은 결과, event-loop 지연, 종료/상태 변경을 검사한다.

검증 로그: `/private/tmp/floe-electron-clipboard-final2.log`, 단축키 추가 검증
`/private/tmp/floe-electron-clipboard-keyboard.log` (각각 exit0).
기존 Electron 회귀도 `/private/tmp/floe-electron-clipboard-regression.log`에서 통과했다.
Rust helper14/service5·host clippy, 기존 Node41 + 프레임 probe3 + clipboard6,
실제 빈 창의 탐색/종료·blob 취소/새 파일 게시/경합/cleanup을 포함한다. 단위·전체
합성 창 검증과 실제 OS 대화상자 조작을 구별한다. 전체 Rust/web battery의 green을
뜻하지 않으며, 해당 실행 결과는 G1 기록과 별도로 확인한다.
전체 `validate_rust.sh`는 workspace unit/CLI/embedded/app-read 뒤 기존 layerprops
native oracle30초 timeout으로 exit1이었다(`floe-stroke-half-full.log`). 전체 green 아님.
초기 하네스는 Electron 44의 `readText()` Promise를 문자열과 직접 비교해 실패했다.
공식 [비동기 clipboard API](https://www.electronjs.org/docs/latest/api/clipboard)에 맞춰
await와 PNG `read/getType`을 사용했다. 제품 권한을 완화한 수정은 아니다.
다른 실행에서는 PNG 복사 완료 기다림이 실패했다. 임시 입력 제거 후에도 다시
Live/버튼 enabled를 기다리는 조건을 보강한 다음 통과했지만, 해당 실패의 정확한
원인까지 확정하지는 않았다. 실패 로그도 남긴다.

이 결과는 합성 Chromium 입력과 native API 경로의 검증이다. 실제 OS 메뉴 클릭·
물리 Cmd/Ctrl+C, IME·다른 앱과의 복사/붙여넣기·X11 PRIMARY selection·RHEL/ETX,
잠금/원격 세션 사이 클립보드 전달은 별도 수용 항목이다. OS clipboard manager의
보관/다른 앱으로의 전파를 제어하거나 종료 시 지운다고 보장하지 않는다.

화면 제어 재확인은 `CUA_REPL_ENABLED_SURFACES is required`로 실패했다. 별도 합성
일반 창을 정리하면서 SIGTERM/SIGINT 뒤 호스트가 남는 현상도 관측했다. Rust 서비스에
SIGTERM을 보내 worker/helper 정리를 확인한 뒤 소유 호스트만 강제 종료했다. 해당
종료 경로는 이번 clipboard 통과에 포함하지 않는다. 후속 E2e에서 별도 재현·수정과
실제 외부 신호 검사를 완료했으며 [종료 신호 수용 기록](WEBUI_ELECTRON_SIGNALS.ko.md)을 따른다.
