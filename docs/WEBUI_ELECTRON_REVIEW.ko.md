# Electron DRC 승인 응답 대기 중 표시 프로세스 장애

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
