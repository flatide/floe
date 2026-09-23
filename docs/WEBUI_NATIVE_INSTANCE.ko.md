# Electron 네이티브 창 재사용

2026-09-22 사용자 결정. 상위 [데스크톱 계획](WEBUI_DESKTOP.ko.md),
[웹 전환 계획](WEBUI_PLAN.ko.md). **2026-09-23 구현 / macOS 합성 창 검증 통과**.
RHEL/ETX 현장 수용은 별도다.

2026-09-23 변경: Electron을 단일 네이티브 호스트로 선택하고 WKWebView를 동결했다.
따라서 아래 구현/수용은 **Electron만** 대상으로 한다. 전날 승인된 같은 사용자·
DISPLAY의 창 재사용, `--multi`, trusted CLI의 새 파일 등록 정책은 유지한다.

## 확정된 계약

- 같은 사용자·DISPLAY의 기본 창을 재사용한다. macOS DISPLAY 부재와 X11
  screen suffix 정규화는 기존 Rust `instance::Key` 규칙을 재사용한다.
- Electron은 외부 브라우저/GTK와 다른 instance namespace를 사용한다. 동결된
  WK 호스트에는 새 endpoint나 창 재사용을 구현하지 않는다. 인증 URL을 다른
  호스트나 새 CLI 프로세스에 전달하지 않는다.
- `--multi`는 기본 instance를 소유하거나 전달하지 않는 독립 창이다.
- trusted CLI가 명시한 새 파일·dependency root는 기존 창의 등록/보호 목록에
  추가할 수 있다. 웹 요청/guest가 임의 경로를 등록하는 권한은 추가하지 않는다.
  추가 등록이 자동 index·DRC 쓰기권한·기본값 게시 승인을 뜻하지 않는다.
- 빈 CLI 호출은 기존 창 표시 요청이다. 최초 빈 실행의 명시 폴더 선택은 유지한다.
  이미 실행 중인 창으로 전달될 호출에는 새 폴더 선택/새 인증 세션이 필요 없다.
- busy·시작 중·종료 중·버전 불일치·불명확한 전달 실패를 새 기본 창 생성으로
  대체하지 않는다. 기존 epoch/seq/receipt와 같은 요청의 확인 규칙을 유지한다.

## 구현 분할과 완료 근거

1. **공통 Rust launch 경계**: Electron 전용 namespace, native 창 표시/폴더 선택 전 claim 또는
   forward, owner 수명 보존, 기존 `handoff`와 source registration 사용. 브라우저
   namespace와 기본 동작은 불변. 명시 process/DRC 옵션은 기존 parser의 독립
   workspace 의미를 보존하고, 기존 worker 옵션을 조용히 변경하지 않는다.
2. **Electron 연결**: 기존 private pipe에 고정 present/
   forwarded 결과를 연결한다. secondary는 빈 창을 남기지 않고 종료한다. 기존
   owner의 숨김/최소화를 복원하되 열린 승인/편집/복구를 자동 확정하지 않는다.
   새로운 bootstrap은 발급/재생하지 않는다. 합성 QA는 기본 instance에 붙지 않는다.
3. **게이트**: Electron 재사용, 브라우저/GTK namespace·DISPLAY 격리, `--multi`, 빈 호출,
   새 파일/공백 경로·고토 옵션, 같은 파일 캐시 유지, pending 모달, 시작/종료 경합,
   소유자 사망 후 새 epoch와 전달 불명 중복 방지. 소켓 단위뿐 아니라 실제 두
   프로세스와 native 창/worker 개수를 검증한다. queued ACK는 표시 완료가 아니다.

`Session::electron`은 parser의 독립 실행 여부를 보존하고 `floe2-electron` key를
claim/forward한다. `Session::parse`는 동결 WK/기존 테스트용 독립 실행 경계로 남는다.
별도 `FLOE_ELECTRON_INSTANCE_DIR`(기본 `/tmp`) 아래 기존 UID별0700 rendezvous를
쓴다. 외부 브라우저의 `FLOE_WEB_INSTANCE_DIR`/`floe2-web` key와 분리하며, 이 로컬
IPC 디렉터리는 DRC 저장용 NFS에 놓지 않는다.

private pipe의 `starting` 뒤에만 창 표시/download helper/폴더 선택을 시작한다.
전달 성공은 고정 `forwarded` 한 프레임과 exit0이고 `starting`, `directory`, `ready`,
인증 URL을 보내지 않는다. owner의 `present`는 ready 뒤에만 허용하며, 같은 사용자
IPC가 수락한 요청만 coalesced atomic bit로 알린다. 중복·순서 오류·추가 필드는
fail-closed다. 실패/불명확한 전달은 새 기본 service로 대체하지 않는다.

secondary도 Electron runtime·임시 profile·**숨긴 BrowserWindow 한 개**는 잠시
생성한다. 표시(show)는0회이고 새 Rust workspace/renderd/download helper는 없다.
따라서 "별도 Chromium 프로세스도 전혀 생성하지 않는다"는 최적화는 아니다.
명시 `--jobs`, `--raster-jobs`, refinement off 등 기존 process 옵션은 독립 실행으로
남는다. 작업 수만 지정해도 창이 따로 열릴 수 있으므로 기본 창을 재사용하려면
view/source/goto 같은 전달 가능 옵션만 사용한다.

기존 `handoff`의 source 준비는 socket callback 밖에서 수행하며 색인을 자동으로
실행하지 않는다. UI는 열린 모달, 미제출 goto, note/waive 편집·미확인 저장,
공유 기본값 승인/미확인 저장이 끝날 때까지 source open을 미룬다. 빈 present는
확인창을 승인하지 않고 창만 복원한다. QA의 일반 smoke는 `--multi`, instance QA는
새 짧은 private 디렉터리/합성 DISPLAY를 사용해 사용자의 기본 창에 붙지 않는다.

## 검증 — 2026-09-23

- Node 프레임/state 검사5개, 실제 private pipe lifecycle8개: 전달 시 invalid native
  override여도 성공(새 discovery 없음), screen suffix 동일 owner, busy 거부,
  `--multi`/명시 jobs/다른 DISPLAY 격리, 폴더 선택 중 lock 보유, 그 owner를
  SIGKILL한 뒤 새 owner, EOF/cancel·일회용 인증·일반 stdout 거부.
- 실제 pinned Electron44.4.3 macOS arm64 두 프로세스 검사:
  `tools/validate_electron_instance.cjs`. 생성한 valmini와 별도 합성 복사본만 사용.
  숨김/최소화 복원, 상대 경로·한글·공백 파일의 새 등록, 같은 파일 goto에서
  Rust 자식 PID 집합 불변, 빈 호출의 frame/revision 불변, End session 모달이
  열린 동안 source 불변/Cancel 뒤 새 source로 교체를 단언했다. 각 secondary는
  `windows=1, shows=0, starting=0, ready=0, forwarded=1, present=0`이고 owner는
  service/auth1회만 시작했다. fixture/cache의 전 파일 SHA-256 불변.
  성공 화면: `/tmp/feq.HGAhM6/owner.png`(직접 확인), exit0.
- 첫 GUI 시도들은 초기 파일 목록 조회 중 disabled Close를 누른 하네스 때문에
  열린 picker가 요청을 정상 보류했다. 활성 Close·닫힘 확인을 추가한 뒤 통과했다.
  timeout/권한/모달 보호를 완화하지 않았다.
- 기존 `web_handoff`는 실제 브라우저 namespace 경로의 queue/receipt, 새 파일,
  동일 view/worker epoch 재사용, 실패 보존을 계속 검증한다. 기존 `instance` 단위는
  UID/product/DISPLAY·소켓 경합·same-intent 재시도/불명확한 ACK를 담당한다.
  UI client 검사는 모달/리뷰 상태14종과 goto 초안의 준비 조건을 고정한다.
- 최종 **선택 없는 전체** `sh tools/validate_rust.sh` exit0 / `ALL OK`.
  KLayout 오라클은 각 worker 경로에서13 PX +2 phase-exact +14 style 통과.
  로그 `/private/tmp/floe-instance-full-final.c0sMId`, SHA-256
  `6501de51e06158cd883c5c04e268ee4041efdee59dd96ff8853ab82145b050da`.
  앞선 실행은 추가 중인 test가 다른 모듈의 private 필드에 접근해 E0616으로
  실패했다(`/private/tmp/floe-instance-battery.b7JpaW/full.log`). test를 필드 소유
  모듈로 옮긴 뒤 소스를 고정하고 전체를 다시 실행한 결과이며, 부분 통과의 합이 아니다.
- `sh tools/validate_electron.sh` 전체 exit0. 실제 notices/empty/export/signal
  창과 Rust19 unit, Node protocol/lifecycle/권한·복구·종료 회귀를 포함한다.
  의도적인 service-error signal case의 `SMOKE: FAIL`은 예상 exit1을 검사한 기록이다.
  로그 `/private/tmp/floe-electron-instance-host-full.log`, SHA-256
  `7246226c906bb1deab4dfe222fddbb4543b27ec855e2a4bdbe1a1e4e3c655f6f`.
  Rust launcher34 pass/2 oracle-only ignored, 전용 host clippy(`--no-deps -D warnings`),
  변경 Rust fmt 및 JS syntax도 통과했다. 의존 tiler/vfs의 기존 경고는 남으며,
  workspace 전체 clippy가 경고0이라는 주장은 아니다.

재현(개발용 fixture generator만 Python/KLayout; 제품 실행에는 불필요):

```sh
(cd electron/service && cargo build --offline --locked)
export FLOE_ELECTRON_SERVICE_BIN="$PWD/electron/service/target/debug/floe-electron-service"
export FLOE_ELECTRON_DOWNLOAD_BIN="$PWD/electron/service/target/debug/floe-electron-download"
export FLOE_INDEX_BIN="$PWD/rust/target/release/floe-index"
export FLOE_RENDERD_BIN="$PWD/rust/target/release/floe-renderd"
# FLOE_ELECTRON_BIN: 검증된 pinned runtime; Python은 기본 .venv/bin/python
"$FLOE_ELECTRON_BIN" tools/validate_electron_instance.cjs
```

helper와 JS/UI를 함께 재빌드/배포해야 한다(Electron shell0.1.3). 캐시 재색인은
필요 없다. WK 전용 코드·패키지·활성 수용 게이트는 추가하지 않았다.

이 단계가 NFS 게시 복구·물리 입력/성능·RHEL/ETX·정식 배포의 전체 목표 완료를
뜻하지 않는다. 저장소 프로토콜이나 원격 공유 권한도 확장하지 않았다.
