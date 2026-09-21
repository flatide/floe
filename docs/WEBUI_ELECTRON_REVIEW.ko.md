# Electron DRC 승인 응답 대기 중 표시 프로세스 장애

후속 [실제 Rust 서비스 종료·재시작 검사](WEBUI_REVIEW_SERVICE_CRASH.ko.md)는
서비스도 사라진 뒤 새 인증/reader에서 게시 값과 이전 승인 만료를 확인한다.
아래와 달리 GUI 없이 HTTP/pipe 경로를 검사하며 실제 오류창 수용은 남는다.

후속 [Rust 저장 프로세스 종료 검사](WEBUI_REVIEW_PROCESS_CRASH.ko.md)는12개
게시 전후 조합을 확인했고, 그 내부 신규 link/unlink 창은 DRC-PUB-01로 열어 두었다.
아래 Chromium 복구 성공은 해당 파일시스템 경계의 해결 근거가 아니다.

2026-09-22. [Electron E2](WEBUI_ELECTRON.ko.md), [G4 감사](WEBUI_G4_AUDIT.ko.md)의
합성 저장·복구 수용 확대다. 제품 호스트/웹 UI/Rust 저장 코드는 변경하지 않았다.
기존 WK의 성공 응답 유실 검사는 [Desktop §13](WEBUI_DESKTOP.ko.md#13-d2-mac-합성-저장-응답-유실재로딩-자동-검사-2026-09-21)에 있다.

## 범위

실제 Rust가 note/waive POST를 **202로 수락한 응답을 Chromium에 전달하기 전**
보류하고, 실제 표시 프로세스를 종료한다. Rust 서비스는 계속 실행한다.
따라서 승인 요청의 UI 응답 대기 중 장애이며, 디스크 쓰기 중 Rust worker를
종료한 검사가 아니다. 작은 파일의 쓰기가 crash 전에 끝났을 가능성도 있다.
fsync/rename 중단·전원/디스크/NFS 장애의 원자성 수용으로 확대하지 않는다.

`tools/validate_electron_review.cjs`는 인자 없는 개발 전용 Electron entry다.
매번 새로운 0700 임시 폴더에 TOP 사각형 OASIS와 2오류/1규칙 ASCII DRC를
생성하고 실제 Rust로 VFS/`.tray`를 만든다(jobs=2). reviewer와 본문은 합성
상수이며 기존 설계/리뷰/인증 파일을 받지 않는다. 생성물을 삭제하지 않고 경로를
출력한다. OS 클립보드·외부 서버·공유 기본값·보안 설정은 접근하지 않는다.

## 실행

기존 release index/renderd와 같은 소스의 Electron service/download helper,
검증된 고정 Electron 런타임이 필요하다. 자동 다운로드·설치는 없다.

```sh
# feature/webui 작업트리 루트. 기존 Rust release worker 빌드는 선행한다.
(cd electron/service && cargo build --offline --locked)
FLOE_ELECTRON_SERVICE_BIN="$PWD/electron/service/target/debug/floe-electron-service" \
FLOE_ELECTRON_DOWNLOAD_BIN="$PWD/electron/service/target/debug/floe-electron-download" \
  "$FLOE_ELECTRON_BIN" tools/validate_electron_review.cjs

# OS/저장 변경 없는 순수 관찰기 검사
node --test tools/validate_electron_review.test.cjs
```

일반 `validate_electron.sh`에는 순수 검사8개만 연결한다. 저장·프로세스 종료를
수반하는 실제 검사는 위 명령으로 별도 실행하며 패키지에 QA entry를 추가하지 않는다.
QA entry는 저장소의 기존 `electron/main.cjs`를 그대로 실행한다. runtime 버전,
sandbox/context isolation, Node/preload 비노출, 비영속 프로필을 확인한다.

## 고정한 계약

1. 실제 UI에서 Error 1 선택 → snapshot → 고정 본문/waive 미리보기 → 명시 승인.
   행이 활성화되고 background 메모 배지 조회가 화면에 완료된 뒤 snapshot을 읽는다.
2. 원래 호스트의 origin/요청 차단 판단을 먼저 적용한 뒤 정확한 두 save endpoint의
   202만 보류한다. 다른 상태 코드/요청/창은 장애 주입 대상으로 삼지 않는다.
3. `forcefullyCrashRenderer()` 뒤 새 Chromium PID를 요구한다. Rust 생존,
   자동 root GET/bootstrap/save 재전송 없음, Recover 취소의 요청 불변을 확인한다.
4. 메뉴의 명시 Recover 승인으로 root GET 한 번만 허용한다. pending 승인 journal과
   Resolve 버튼이 복구되고 autosave는 off여야 한다. Waive 미확인 중에는 오류 목록
   read barrier를 유지하며, 그 목록의 준비를 먼저 요구하지 않는다.
5. Resolve를 명시적으로 누를 때만 POST가 1→2가 되고 receipt는 여전히 `#1`이다.
   메모와 waive 각각 **1 → crash/reload 후 1 → 명시 resolve 후 2**를 고정한다.
6. 실제 UI로 메모/waive를 순서대로 다시 읽고 snapshot을 해제한다. 독립 파일 검사로
   원본/pack/cache SHA-256 불변, note 한 건, waive 상태 `[1,0]`와 rule count=1,
   sidecar 0600 및 추가 산출물 두 sidecar/두 lock만 존재함을 단언한다.
7. 실제 제품 종료 확인의 Cancel은 Rust를 유지한다. 명시 Confirm 뒤 Rust join과
   앱 exit0까지 확인해야 최종 OK다. native Recover 선택은 QA callback 주입이므로
   실제 OS 확인창·물리 키보드/IME 수용은 아니다.

Electron의 [webRequest는 마지막 listener만 사용한다](https://www.electronjs.org/docs/latest/api/web-request).
따라서 QA는 제품 listener를 다른 handler로 덮어쓰지 않고 등록을 감싸 원래
callback의 거부/redirect 판단을 보존한다. 요청 횟수는 죽는 페이지가 아닌 host에서
센다. 인증 header/cookie, 승인 token, request/response 본문은 관찰하지 않는다.
실패 진단은 고정 단계·숫자 카운터/상태 코드·불리언뿐이다.

준비는 기존 index 명령당120초, UI 각 단계30초, 보류 응답은10초 안전 제한이다.
실패 시 보류 callback을 취소하고 소유한 Rust 서비스에 종료를 요청한다.
시험을 통과시키기 위한 제한 확대, 저장 재승인/자동 재시도, 예산 확대는 없다.

## 실행 기록과 미완료 구간

- 순수 관찰기8개 + 기존 호스트/복구/service-client 선택 검사 합계 **25 passed**.
  로그 `/private/tmp/floe-electron-review-unit.log`.
- macOS arm64 Electron44.4.3 실제 저장/복구: 순서 교정 후 서로 다른 새 fixture에서
  연속2회 exit0. `floe-electron-review-crash-idle-read.log`, `...-repeat.log`.
- 디스크 waive 바이트/추가 산출물 검사까지 포함한 최종 소스도 새 fixture에서 exit0.
  `/private/tmp/floe-electron-review-crash-final.log`. 순수8개를 다시 통과했다.
- 고정 OASIS는 별도 KLayout 읽기로 TOP, 사각형1개,40×30µm,DBU0.001을 확인했다.
  QA 실행 자체에는 Python/KLayout이 필요 없다.

중간 실패도 보존한다. 초안의 pack 이름 `.ice`를 현재 `.tray`로 수정했다.
미확인 waive의 오류 목록을 먼저 기다려30초가 지난 검사 순서는 보호 계약에 맞췄다.
선택 직후 badge/snapshot 읽기가 겹친 HTTP429도 계측했다. 화면의 badge 조회 완료를
기다린 뒤 읽도록 바꿨으며 제품 read barrier/자원 제한을 완화하지 않았다.

별도 전체 배터리(`cb1f2c9`, 소스 고정)는 이전 `layerprops`72문서/980스타일
오라클을 통과했지만 다음 `layer_defaults` native oracle에서30초 timeout으로
실패했다. `/private/tmp/floe-webui-after-startup-full.log`. 이 실패에 본문 진입
계측은 없으므로 같은 loader 원인이라고 확정하지 않는다. 새 QA의 선택 통과를
전체 배터리 성공으로 합산하지 않는다.

전체 목표에는 Rust 저장 worker/디스크 장애, 실제 물리 입력·OS 파일창·IME/DPI,
G1 동일 조건 성능/G4 확대 수용, RHEL8.6/8.10+ETX 및 Python-free Linux 실행,
정식 호스트 채택·서명/배포가 남는다. 원격 공유·유료 CI 보류는 그대로다.

## Rust 서비스 종료 UI — 2026-09-22

표시 프로세스 복구와 달리 Rust 서비스가 죽으면 같은 세션의 Recover는 불가능하다.
기존 `finish()`는 서비스 exit를 알리는 동안 기존 레이아웃 문서를 종료 안내로 바꾸지 않았고,
`panel`이 이미 열려 있으면 알림 자체를 생략하고 창을 파괴했다. 새로운 합성
미승인 note preview에서 실제 소유 서비스에 SIGKILL을 보내, 종료 화면으로
전환되지 않는 것을 재현했다(`floe-service-exit-ui-baseline.log`, exit1). 기존 문서
자체의 WebSocket 단절 표시와는 구별하며, 죽은 서비스가 Live 문구를 얼마나 오래
남겼는지까지 이 실패 로그만으로 측정했다고 주장하지 않는다.

Electron 비교 shell **0.1.2**에서 다음과 같이 바꾼다. Rust 서비스/renderer 버전과
저장 API/권한은 바꾸지 않는다.

- 오류가 난 세션은 서버 없는 고정 로컬 상태 페이지로 교체한다. 도형·Live·편집기는
  남기지 않으며 미저장 초안 소실과 **승인된 쓰기는 이미 완료됐을 가능성**을 안내한다.
  새 세션을 명시적으로 시작해 결과를 확인하도록 하며 자동 시작/저장/색인은 없다.
- 열린 native message box는 AbortSignal로 취소하고 확인창 유무에 기대지 않는다.
  실제 Reload 확인창도 Cancel 응답으로 닫히며 늦은 Reload가 실행되지 않는다.
  OS Save/Open chooser 전체의 자동 닫힘을 이 결과로 보장하지는 않는다.
- 종료 후 기존 origin/WS/blob 요청과 navigation, native Recover/파일 메뉴/프로그램식 clipboard
  권한은 더 이상 사용할 수 없다. 고정 로컬 안내 페이지만 허용한다.
- 다운로드 producer 취소·helper 정리는 안내창과 독립적으로 진행한다. 창 닫기·Quit·
  신호가 정리 중 들어와도 `TerminalExit`는 정리 결과가 나온 뒤에만 exit한다.
  오류 창은 닫기 전까지 유지하고 exit1을 보존한다. 정상 종료와 일반 smoke는
  추가 확인을 요구하지 않는다. Electron의 [Quit/exit 수명주기](https://www.electronjs.org/docs/latest/api/app)와
  [dialog의 취소 신호](https://www.electronjs.org/docs/latest/api/dialog)를 사용하며 런타임/OS 보안을 바꾸지 않는다.

검사는 새 TOP 사각형·2오류/1규칙 DRC만 생성한다. UI에서 note를 준비하되
동의 체크/Approve는 **실행하지 않는다**. source/cache/pack의 바이트·모드·mtime와
파일 목록 전체가 불변이어야 하며, 실제 설계/기존 메모/클립보드는 접근하지 않는다.
인증은 새 pipe→페이지 메모리 안에만 있고 로그/파일에 기록하지 않는다.

```sh
# 같은 pinned runtime/일치하는 Rust worker·helper를 사용. 자동 설치 없음.
"$FLOE_ELECTRON_BIN" tools/validate_electron_service_exit.cjs
"$FLOE_ELECTRON_BIN" tools/validate_electron_service_exit.cjs --pending-prompt
"$FLOE_ELECTRON_BIN" tools/validate_electron_service_exit.cjs --signal
node --test electron/terminal-exit.test.cjs
```

macOS arm64/44.4.3에서 일반 창 닫기, 실제 Reload 확인창 대기→취소→Quit,
종료 안내 상태의 실제 SIGTERM 세 경로는 각각 exit0(검사 성공)이다. 제품의 예상 종료 코드는 **1**이며 QA가 이를
확인하고 성공으로 변환한다. 서비스는 실제 SIGKILL/code=null이고, 사전에 관찰한
직계 Rust worker도 사라져야 한다. 메뉴의 Recover 호출은 새 서비스/페이지를
만들지 않고, terminal PNG의 도형/Live 제거와 안내 문구를 직접 확인했다.
`floe-service-exit-ui-final.log`, `floe-service-exit-ui-prompt.log`,
`floe-service-exit-ui-signal.log`에 기록했다. 마지막 신호는 QA가 자기 소유 호스트
PID에 보낸 실제 OS 신호이며 JS `emit`이나 제품 취소 함수 직접 호출이 아니다.
실제 native 창/API 경로이며 물리 키 입력·현장 ETX/다른 OS 수용은 아니다.

초기 QA의 미승인 상태에서 Approve 활성화를 기다린 오류와 MenuItem click의
반환값을 검사한 오류는 실제 UI 계약에 맞췄다. consent 기본값/저장 권한/시간
제한을 완화하지 않았다. 해당 중간 실패 로그도 유지한다. 순수 수명주기/호스트/
복구/신호 **17개**와 portable JS 파일 포함 검사2개는 통과했다. 실제 SIGKILL
검사는 opt-in 명령이며 일반 게이트에는 순수 TerminalExit 검사만 넣는다.

`floe-terminal-exit-electron.log`의 전체 Electron gate는 Rust helper unit19개/
clippy 이후 Node 통합 검사에서 **38 pass/3 timeout**으로 exit1이었다. download
slot 두 검사와 pipelined init/cancel 서비스 검사이며 timeout의 원인을 이 UI 수정이나
macOS loader로 확정하지 않는다. timeout 확대·실패 검사 재실행으로 성공 처리하지 않았다.
그 뒤 실행되지 못한 별도 GUI 검사(`floe-terminal-exit-gui-regression.log`)는 새 빈 창의
인증/메뉴/종료, Chromium blob의 취소/게시/경합/정리와 기존 외부 SIGINT/SIGTERM
네 경로까지 exit0이다. 이를 앞선 전체 gate 실패를 대체하는 것으로 합산하지 않는다.
packager fmt/clippy도 통과했다.

같이 시작한 전체 Rust/web 검사 로그는 `floe-terminal-exit-full.log`다. 진행 중
`query_stream` 테스트 실행 파일을 읽기 전용으로 1초 sample한 결과, 시작 후
27.6초 시점의 783개 샘플이 모두 `_dyld_start+0`이고 footprint는112KiB였다
(`floe-terminal-query-start.sample`). 이후 테스트 본문은5개 ignored로0.00초에
끝났다. **해당 프로세스의 본문 이전 지연** 근거이며, 앞의 helper timeout3건까지
같은 원인이라고 단정하거나 macOS 보안 설정을 바꾸지는 않는다.

전체 Rust/web 실행은 **exit1**이었다. workspace 단위, index/app CLI, portable,
자급식 runtime, embedded lifecycle, layerprops/layer defaults, palette/bitmap,
display/clip/capture, 조회/공유/owner lifecycle, handoff/file picker까지 통과했으나
`web_startup`의 `oracle-build`에서180.009초 timeout으로 멈췄다. 실패 명령은
`cargo test --offline --locked -p floe-app --lib --no-run --message-format=json`이다.
앞선 layerprops/기본값 통과로 과거 기동 실패를 지우거나 이번 build timeout을
같은 원인으로 확정하지 않는다. 이후 gate는 실행되지 않았고 **전체 green 아님**이다.
전체 실행을 재시작하지 않았으며 임시 `.venv` 링크는 종료 trap에서 제거됐다.

이 수정은 서비스 종료의 표시 누락을 닫는 것이며 저장소 write/fsync 장애,
DRC-PUB-01, NFS 호환성 또는 전체 G4 완료를 뜻하지 않는다.
