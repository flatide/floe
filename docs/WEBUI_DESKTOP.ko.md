# floe2 독립 창 + 내장 WebView

작성: 2026-09-18. 정본 상위 계획: [WEBUI_PLAN.ko.md](WEBUI_PLAN.ko.md).

## 1. 확정된 요구와 현재 상태

사용자는 외부 Chrome/Firefox 없이 독립 창으로 실행하고, **현재 웹 UI를
내장 WebView에 표시**하는 형태를 선택했다. HTML을 버리고 별도 Rust 위젯 UI로
재작성하지 않는다. Python 런타임은 추가하지 않는다.

필수 현장 환경은 **RHEL 8.6 또는 8.10, 서버 실행 + ETX/X11 표시**다.
macOS에서 창이 뜨는 것, SSH XQuartz 실험, Firefox에서 성공한 검사는 이 현장
수용을 대신하지 않는다. 외부 브라우저 실행 경로도 별도로 유지한다.

현재는 **D0 공통 실행 경계와 환경 진단 단계**다. 실행 가능한 `floe2-desktop`,
`.app`, Linux desktop portable은 아직 제공하지 않는다. `floe2-web`은 계속 외부
브라우저용 CLI다. “standalone Rust backend”와 “브라우저 없는 독립 앱”을 구분한다.

## 2. 공통 구조

```text
desktop main thread: native window + WebView (UI는 기존 번들)
                 │ 메모리로만 전달하는 일회용 loopback URL
service thread:  floe_app::embedded::Session
                 └ 기존 Rust gateway / 인증 / DRC·색인 / renderd client
```

- 기존 CLI 모듈을 `floe-app` 라이브러리로 옮겼으며 CLI 바이너리는 같은 진입점을
  호출한다. 웹 UI, 렌더·플랜, 파일 권한, 자원 예약, 저장 확인 API를 복제하지 않는다.
- `embedded::Session::parse/run`은 기존 view 옵션을 사용하되 Firefox 실행,
  `--no-open`, 외부 `--session-file`을 거부한다. 임베디드 세션은 독립 소유하며
  이미 열린 **브라우저** 세션으로 forward하지 않는다. 데스크톱끼리의 단일
  인스턴스/재열기는 D2에서 결정·검증한다.
- 임베디드 시작은 URL을 callback으로 전달한다. 인증 파일/argv/로그에 URL을
  남기지 않는다. 현재 세션이 소유하는 임시 디렉터리만 0700으로 만들며 기존
  scope 보호 목록에 넣는다. URL 구조 검증 및 동일 root 문서 탐색 판정은 공통
  순수 함수로 둔다. 이 함수가 존재한다고 **미구현 WebView의 탐색 차단까지
  완료된 것은 아니다**.
- 세션 종료·worker 정리는 기존 서비스가 맡는다. 호스트는 메인 스레드에서 창을
  운영하고 서비스 스레드 종료를 기다려야 한다. 창 닫기는 기존 End session의
  취소 기본값 확인을 열고, 승인되지 않은 저장이나 종료를 대신 실행하지 않는다.
- 인증 교환/HttpOnly cookie/CSRF/Origin 검사와 등록된 파일 scope를 유지한다.
  `file://` UI나 광범위한 Rust IPC로 인증·파일 접근 범위를 우회하지 않는다.
- WebView 의존성은 기존 headless Rust workspace의 기본 빌드 및 musl CLI/worker
  패키지와 분리한다. 별도 의존성은 lock/vendor/고지까지 검증한 뒤 추가한다.

## 3. RHEL 8에서 선행할 호환성 판정

공식 자료에서 확인한 내용(2026-09-18):

1. RHEL 8의 `webkit2gtk3`는 **WebKitGTK API 4.0** 패키지 이름이다.
   API 4.0은 GTK 3 + libsoup 2, API 4.1은 GTK 3 + libsoup 3이다. 4.1이라는
   이름이 GTK 4를 뜻하지 않는다. [WebKitGTK 프로젝트 설명](https://planet.webkitgtk.org/)
2. Wry는 0.25부터 Linux 의존성을 API 4.1로 바꿨다. 따라서 최신 Wry를 그대로
   붙이고 RHEL 8 기본 런타임 지원이라고 할 수 없다.
   [Wry 변경 기록](https://github.com/tauri-apps/wry/blob/dev/CHANGELOG.md)
3. upstream WebKitGTK는 2.52부터 libsoup 2 지원을 제거한다고 공지했다.
   API 4.0 고정은 Red Hat의 실제 패치·지원 상태와 함께 판단해야 한다.
   배포판의 backport 여부를 upstream 버전 숫자만으로 단정하지 않는다.
   [WebKitGTK libsoup 2 종료 공지](https://webkitgtk.org/2025/10/07/webkitgtk-soup2-deprecation.html)
4. 구형 Wry 0.24.x는 대안 후보지만 의존성/보안 고지를 별도로 검토해야 한다.
   최신 0.24 계열 릴리스 기록에도 glib 등의 advisory가 나타난다. “구형으로
   내리면 해결”을 제품 결정으로 삼지 않는다.
   [Wry 릴리스 기록](https://github.com/tauri-apps/wry/releases)

이번 조사에서 만든 Wry 0.57/Tao 0.37 후보 manifest/호스트 초안은 현장 기준
부적합을 확인해 제거했다. 후보 빌드/다운로드는 제품 검증으로 세지 않으며 기존
`rust/Cargo.lock`과 `rust/vendor/`를 바꾸지 않았다.

다음 후보를 비교한다. 아직 어느 것도 채택·완료되지 않았다.

| 후보 | 확인할 핵심 |
|---|---|
| 시스템 API 4.0을 쓰는 얇은 Linux 호스트 | Rust 바인딩/FFI 안전성·유지보수, 시스템 WebKit 보안 업데이트, ETX 성능 |
| 지원 가능한 API 4.1 런타임 별도 동봉 | RHEL 8.6/glibc 2.28 기준 빌드, 하위 라이브러리 closure·라이선스·업데이트 책임, 용량 |
| 별도 Chromium 계열 내장 런타임 | RHEL 8/X11 지원 하한, 배포 크기·메모리·ETX 합성 비용 및 보안 업데이트 |

Firefox kiosk/app 모드를 내장 WebView 구현 완료로 대체하지 않는다. 전역 OS
패키지 교체, sandbox 비활성화, 서버 보안 정책 변경도 자동 해결책으로 삼지 않는다.

현장에서 실행 가능해지면 다음 **읽기 전용** 결과를 받는다. 설치/GUI 실행/네트워크
접속/호스트명·DISPLAY 값 출력은 하지 않는다. devel metadata가 없다고 런타임이
없다고 판정하지 않는다.

```sh
sh tools/audit_desktop_env.sh
```

RPM 설치 버전과 pkg-config ABI는 서로 구분한다. 라이브러리 존재 여부만으로
실제 로드·sandbox·ETX 동작이 증명되지는 않는다.

## 4. 단계와 수용 게이트

| 단계 | 범위 | 상태 |
|---|---|---|
| D0 | 확정 요구, RHEL ABI 조사, 공유 Rust 실행 경계, 읽기 전용 환경 감사 | 공통 기반·선택 검증 완료; 현장 inventory와 전체 회귀 잔여 |
| D1 | 플랫폼 호스트 선택, lock/vendor/고지, 독립 창·인증·표시·종료/실패 정리 | 미완료 |
| D2 | 메뉴·단축키·IME·DPR/resize/pan, 파일 선택·다운로드·클립보드, 저장·복구·창 닫기 | 미완료 |
| D3 | macOS 패키지, RHEL 8.6/8.10 ELF/런타임, 실제 ETX 입력·픽셀·지연·사용량 비교 | 미완료 |

웹에서 이미 검증한 경로도 WebView 엔진 차이는 다시 검사한다. 특히 blob
다운로드/파일 덮어쓰기 확인, PNG 클립보드, DRC 결과 불명·저장 중 종료,
새로고침/재접속, sessionStorage, CSP, 쿠키·WS Origin, 한글 IME가 포함된다.
원격 새 창은 자동으로 외부 브라우저를 열지 않고 정책을 명시한다.

기존 전체 목표의 잔여(실제 브라우저 잔여 수용·G1 성능/pacing·G4 전범위 대조·
Python-free Linux 실행·G2 ETX)는 계속 남는다. D0를 이 목표 완료로 세지 않는다.
원격 공유 SH-10/열린 인덱스 hot-reload 보류와 M5 조건부 상태도 변경하지 않는다.

## 5. D0 검증 기록 (2026-09-18)

- `cargo test -p floe-app --offline --locked`: 31 passed, 2 oracle ignored.
  별도 `embedded_lifecycle`은 일반 unit 실행에서는 ignored이며 아래 배터리에서
  실제 실행한다. CLI 본문 이동은 `embedded` 모듈 추가와 `cli_main` 이름 변경
  외에는 기존 코드와 동일하다.
- `validate_desktop_env.py`: 합성 RPM/pkg-config 환경·인자·민감한 DISPLAY 값
  비출력 2검사 통과. 로컬 macOS inventory 실행도 확인했다. **실제 RHEL 결과 아님.**
- 신규 `embedded_host` gate: 빈 워크스페이스의 in-process ready 전달 → 인증
  교환 → cookie/CSRF 종료 → listener 종료를 실제 loopback으로 검사. 1 passed.
  이 검사는 WebView, 설계 픽셀, 네이티브 창 닫기 수용을 증명하지 않는다.
- 전체 `sh tools/validate_rust.sh`: workspace unit, 기존 CLI/버전/캐시/portable,
  macOS Python-free runtime smoke, embedded host, app read까지 통과한 뒤
  기존 `layerprops` 오라클 실행의 **30초 timeout으로 exit 1**. 전체 green 아님.
  더 앞선 선택 실행에서도 GTK 시작 오라클이 동일한 30초 제한에 걸렸다.
  기존 macOS 시작 지연과 같은 형태이나 원인은 이번에 확정하지 않았다.
- 이후 같은 코드·제한으로
  `sh tools/validate_rust.sh --only embedded_host,web_startup,layerprops,web_cli`는
  **exit 0 / ALL OK**. layerprops 72문서·980스타일·4모델, GTK 시작 144사례,
  stream 정책 380사례, 실제 시작 22사례(첫 generation 21건), 웹 CLI 수명 통과.
  재실행의 GTK 오라클 프로세스 wall은 15.664초, 테스트 본문은 0.01초였다.
  timeout 완화·보안 설정 변경·자동 재시도·검사 생략은 하지 않았다.
- 로그: `/private/tmp/floe-embedded-gates.RgMGtA/full.log`, `focused.log`.
  전체 배터리 성공과 macOS 시작 지연 원인 규명은 남겨 둔다.
- 기존 `rust/Cargo.toml`, `rust/Cargo.lock`, `rust/vendor/`는 불변.
  새 GUI 의존성·다운로드 후보는 제품에 편입하지 않았다.
