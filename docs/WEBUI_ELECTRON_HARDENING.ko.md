# Electron SIGHUP·배포 fuse 강화 — 2026-09-28

`feature/webui`. SIGHUP 누락과 개발 runtime 기본 fuse를 확인한 뒤 수정했다.
기존 macOS WK 전용 기능은 동결을 유지한다. Electron이 재사용하는 Rust 서비스 수명
코드만 공유 수정하며 렌더 규칙·리뷰 저장 권한·원본 파일 형식은 변경하지 않는다.

## 종료 신호

- Electron은 INT/TERM/HUP을 같은 명시 취소 경로로 연결하고 ready에서 자기 listener만
  재등록한다. 메뉴 종료의 확인 절차와 달리 OS 종료 신호에서는 승인창을 기다리지 않는다.
  기존 failure가 있었다면 취소 완료를 성공으로 바꾸지 않는다.
- Rust pipe host는 첫 입력/폴더 선택 **전**부터 신호를 등록한다. 대기 중인 읽기도
  50ms poll·취소 flag로 깨우고, 같은 flag를 서비스에 전달해 cleanup/join까지 유지한다.
  EOF와 신호가 경합해도 원래 취소 이유를 TERM으로 덮지 않는다.
- CLI와 인덱스 자식 취소, 다운로드 helper, 패키저에도 HUP을 연결한다. CLI/서비스의
  SIGHUP 취소 코드는129다. CLI의 기존 `max(2)` 보정이 HUP=1을 INT로 바꾸던 부분도
  수정했다. download helper는 기존처럼 discard 결과/cleanup ACK로 성공 여부를 알린다.
- 부모에만 HUP을 보내는 경우와 격리된 새 host+service process group에 HUP을 보내는
  경우를 구분해 검사한다. 후자에서 취소된 pipe 읽기를 protocol error(exit1)로
  오인하던 경합을 재현하고 typed cancellation으로 수정했다.
- SIGKILL/전원 장애의 cleanup, NFS blocking I/O의 즉시 종료를 보장하지 않는다.
  주기적으로 신호를 재전송하거나 타 사용자의 PID/프로세스 그룹을 종료하지 않는다.

## 번들 fuse 계약

공식 [Electron fuse 문서](https://www.electronjs.org/docs/latest/tutorial/fuses)와
[v1 schema](https://github.com/electron/fuses/blob/main/src/config.ts)에 맞춰
오프라인 Rust 패키저가 **checksum 검증 후 해제한 private runtime 복사본만** 변경한다.
npm 패키지 설치나 런타임 다운로드를 패키저에 추가하지 않는다.

| 항목 | 번들 정책 |
|---|---|
| RunAsNode | off |
| EnableNodeOptionsEnvironmentVariable | off |
| EnableNodeCliInspectArguments | off (`--inspect`/SIGUSR1 inspector 비활성) |
| GrantFileProtocolExtraPrivileges | off |
| cookie encryption·ASAR integrity·ASAR-only·browser-specific snapshot | 기존 off 유지 |
| WASM trap handlers | 기존 on 유지 |

고정 Electron44.4.3의 native 바이너리 한 개에서 v1/9-fuse wire만 받는다. 기존 wire는
`101100011`, 강화 후는 `000000001`이다. 모호한 sentinel, 잘린 wire, 알 수 없는 schema/
상태/default는 쓰기 전에 거부한다. universal 런타임은 기존 native-only 입력 pin의
지원 대상이 아니며 두 sentinel을 임의로 하나만 바꾸지 않는다. 고정 크기 buffer로
스캔하고 경계에 걸린 sentinel도 검사하며 수정 후 실제 bytes를 다시 읽어 단언한다.

macOS는 binary 변경으로 upstream 서명이 무효화되므로 staging `.app`에만 `codesign`
ad-hoc 서명을 재생성하고 deep/strict 검증한다. helper의 기존 entitlements/flags 등을
보존한다. 설치된 원본 앱·input ZIP·Keychain·인증서·Gatekeeper 설정은 바꾸지 않는다.
이 서명은 개발 실행용이며 배포자 인증/공증을 제공하지 않는다.

`BUNDLE.json`에 고정 fuse 정책을 기록하고 `BUILD.txt`에는 fuse 정책·서명 종류·ASAR
미적용을 구분한다. Rust `verify.sh`와 독립 Python 검사는 파일 목록/해시 외에 실제
wire도 검사한다. 약한 runtime의 해시를 manifest에 다시 기록해도 정책 검사를 통과하지
못한다. 구 fuse 정책 없는 번들은 새 검증기로 거부하며 재빌드한다. 구 번들의 자체
검증기를 새 정책 검사로 오해하면 안 된다.

개발/번들 런처는 `ELECTRON_RUN_AS_NODE`, `NODE_OPTIONS`, `NODE_PATH`,
`NODE_EXTRA_CA_CERTS`도 거부한다. 이 검사는 fuse의 대체가 아니다. 명시 개발용
runtime override를 쓰면 번들 runtime 보장에서 벗어나므로 번들 수용 때는 unset한다.

## 제외 범위

앱 소스는 runtime 옆 디렉터리에 있고 launcher가 그 경로를 전달하는 구조다.
`app.asar` 전환, OnlyLoadAppFromAsar, ASAR 무결성은 이번에 켜지 않는다. 런처·소스·
검증기를 모두 바꿀 수 있는 사용자에 대한 변조 방지/권한 격리도 아니다.
정식 Developer ID 서명·공증·업데이트 운영과 Linux/ETX GUI 수용은 별도다.

## 검증 기록

- Node 종료 신호/terminal exit 단위 검사 통과.
- 실제 Rust service/download 합성17개 통과. 폴더 대기·live SIGHUP129, 격리된
  process-group HUP+EOF, partial download discard·기존 실패 경로를 포함한다.
- CLI·packager 단위 및 strict Clippy 통과. 서비스/download 단위·strict Clippy 통과.
  기존 tiler/VFS 등 의존 crate 경고는 별도로 남아 있다.
- `sh tools/validate_rust.sh --only app_cli,web_portable,embedded_host,app_render,app_clip`:
  `ALL OK`. CLI INT/TERM/HUP 자식 종료·임시파일 정리, 패키저 TERM/HUP 회수와
  기존 native render/clip 오라클을 포함한다. 전체 배터리를 새로 통과한 것은 아니다.
- 공식 macOS arm64 ZIP의 저장소 SHA-256 pin 일치 후 실제 private 번들을 조립했다.
  fuse 변경, ad-hoc 재서명/deep strict 검증과 bundle wire/inventory 검증을 통과했다.
  이는 창 실행/물리 입력/사용자 설계 검사와 별개다.
- 최종 archive를 다른 새 폴더에 풀어 독립 Python fuse 검사·Rust 검증기988항목을
  통과했다. 그 번들의 release service/download로 위17개 회귀도 다시 통과했다.
  단위 검사 수는 app35(별도 fixture2 ignored), packager13, service/download19,
  Node signal/terminal4, launcher/closure4다.

회귀 로그: `/private/tmp/floe-hup-native-final.log`,
`/private/tmp/floe-hup-fuses-units-final.log`, `/private/tmp/floe-hup-fuses-clippy-final.log`,
`/private/tmp/floe-hup-electron-units-final.log`, `/private/tmp/floe-hup-electron-clippy-final.log`,
`/private/tmp/floe-hup-battery.j8nvrf/battery.log`,
`/private/tmp/floe-hup-fuse-package-final.log`, `/private/tmp/floe-hup-fuse-independent.log`.
최초 process-group 실패는 `/private/tmp/floe-hup-native.log`에 보존한다.
실제 Electron 창의 HUP smoke는 `tools/validate_electron_signals.cjs`에 추가했지만,
이번에는 GUI를 실행하지 않았고 RHEL/ETX 실측도 하지 않았다.

개발 검증 산출물: `/private/tmp/floe-fuse-runtime.oPIEoX/floe2-hardened-final.tar.gz`,
SHA-256 `34f6c9280af07e6d8707887dbe1befb62ad81e33a3bedb18bc70c8b2e35bec8e`.
원본 ZIP이나 설치된 앱을 덮지 않았으며 기존 번들에 적용하려면 재빌드해야 한다.
