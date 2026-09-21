# Electron 종료 신호 수용 — E2e

2026-09-21. [Electron 비교판](WEBUI_ELECTRON.ko.md)의 native 종료 경로 수정.
클립보드 실조작을 위해 별도 합성 창을 열었다가, 화면 제어 연결이 없어 SIGTERM으로
정리하는 과정에서 발견했다. CUA 실패와 호스트 종료 실패는 별개의 문제다.

## 재현과 원인

실제 외부 SIGTERM 뒤 `before-quit`은 발생했지만 우리 `process.on('SIGTERM')`
콜백은 호출되지 않았다. 일반 Quit 확인은 취소 기본값이라 Rust는 살아 있고 창은
사용자 확인을 기다렸다. SIGINT도 처음 관측한 일반 합성 창에서는 종료하지 못했다.
그 창은 소유 Rust service에 SIGTERM → worker/download helper 정리 확인 → 소유
Electron 호스트만 SIGKILL로 정리했다. 정상 종료 통과로 계산하지 않는다.

새 빈 root의 고정 재현 하네스에서도 SIGTERM 후 `before-quit`만 출력되고 10초 내
정리가 끝나지 않아 **exit1**이었다. 실패 정리에서만 소유 호스트를 강제 종료했으며
Rust EOF 취소로 남은 자식이 사라지는지 별도 확인했다.
로그: `/private/tmp/floe-electron-signals-before.log`.

고정 runtime44.4.3의
[ElectronBrowserMainParts](https://github.com/electron/electron/blob/v44.4.3/shell/browser/electron_browser_main_parts.cc#L634)는
메인 루프 생성 단계에 native shutdown handler를 설치해 `Browser::Quit`을 호출한다.
초기 main script의 libuv handler 등록만으로는 이 경로를 대체하지 못했다. `ready`에서
우리 listener를 제거·재등록한 뒤 실제 OS 신호가 해당 콜백으로 들어오고 정상 정리가
완료됨을 확인했다. 단순히 JS listener를 하나 더 붙이거나 Quit 확인을 생략하는 수정이 아니다.

## 수정 경계

- 시작 시 등록은 유지하고 ready에서 **우리 SIGINT/SIGTERM listener만** 재등록한다.
  다른 listener를 제거하지 않는다. OS 정책·Chromium sandbox·runtime 파일은 바꾸지 않는다.
- 신호는 기존 명시 강제 종료 경로인 Rust EOF 취소/정리를 호출한다. pending clipboard
  검사·복구는 무효화하고, 이미 기록된 오류를 false로 덮어쓰지 않는다. 정상 상태는
  exit0, 기존 실패가 있으면 exit1이다. 저장/index/bootstrap을 재전송하지 않는다.
- 열린 native message box는 부모 창을 가진 기존 dialog에 AbortSignal을 연결해
  **Cancel**로 닫는다. 신호 뒤 새 오류 확인창을 열어 종료를 막지 않는다. 승인 버튼을
  누르거나 미승인 파일 작업을 실행하지 않는다.
  [Electron dialog AbortSignal](https://www.electronjs.org/docs/latest/api/dialog#dialogshowmessageboxwindow-options).
- window/close controller 생성 전의 Quit 요청도 시작 취소로 처리하고, 취소 뒤 ready
  callback이 새 창/서비스를 만들지 않게 한다. 이는 코드 경계 보강이며 cold-start
  타이밍의 물리 Quit 전범위를 이 검사로 증명했다고 하지는 않는다.
- 일반 메뉴/Dock/window Quit은 기존 기본취소 확인을 유지한다. `before-quit`를 모두
  강제 종료로 바꿔 이 테스트를 통과시키지 않는다.

## 게이트와 한계

```sh
# 검증·압축해제한 FLOE_ELECTRON_BIN, Rust helper override는 Electron 문서 참조
node tools/validate_electron_signals.cjs
node --test electron/termination-signals.test.cjs
```

경로 인자를 받지 않으며 새 빈 root만 쓴다. 실제 설계·DRC·클립보드·원격 서버는 없다.
Rust 인증/읽기·탐색/새 창 차단 후 준비 marker를 출력하면 **외부 Node driver**가
소유 Electron PID에 실제 OS 신호를 보낸다. 앱 내부 `process.emit`/자체 signal/직접
cancel callback으로 대체하지 않는다. 초기 준비30초, 신호 뒤 정리10초이며 실패 정리의
강제 종료를 성공으로 세지 않는다.

macOS arm64 /44.4.3 결과(`floe-electron-signals-dialog.log`):

| 상태 | 신호 | 결과 |
|---|---|---|
| 인증된 빈 창 | SIGTERM | handler 실행, Rust join, 소유 자식 전부 소멸, exit0 |
| 인증된 빈 창 | SIGINT | 동일, exit0 |
| 실제 native message box pending | SIGTERM | Cancel 응답 후 정상 정리, exit0 |
| QA로 기존 failure를 설정한 native message box | SIGTERM | Cancel·정리, **예상 exit1** 유지 |

마지막 행은 실제 DRC 저장 실패 주입을 뜻하지 않는다. 하네스가 주입한 기존 error
상태를 종료 신호가 덮어쓰지 않는 대조다. native dialog 선택을 자동 승인하지 않는다.
일반 `validate_electron.sh`에도 이 네 실행과 listener 중복/보존 단위 검사를 연결했다.

전체 Electron host 회귀(`floe-electron-signals-regression.log`)도 Rust helper unit/
clippy, Node 계약, 실제 빈 창·blob 내보내기와 네 signal 실행까지 **exit0**이다.
이어서 실행한 layout 회귀(`floe-electron-signals-layout.log`)는 10%/50% pan·원위치
픽셀 일치까지 통과했으나, 별도 exact clip 창의 **authentication 단계에서 exit1**이다.
후속 recovery 검사는 실행되지 않았다. 해당 로그만으로 인증·문서 가시성 중 어느
조건이 실패했는지는 분리되지 않으므로 원인을 단정하지 않으며, 이전 layout 통과로
이번 실패를 대체하지 않는다.

RHEL8/ETX의 signal·파일 chooser·busy I/O·저장 진행 중 중단, OS 종료/로그아웃,
실제 물리 Ctrl+C·Dock Quit·접근성은 별도 수용이다. 느린 fsync 같은 syscall의 종료
시간을 이 빈 root 결과로 보장하지 않는다. 전체 Rust/web battery의 기존 layerprops
30초 timeout과 렌더러 lint 실패도 이 수정의 통과로 해소됐다고 표기하지 않는다.
