# floe2 독립 창 + 내장 WebView

작성: 2026-09-18. 정본 상위 계획: [WEBUI_PLAN.ko.md](WEBUI_PLAN.ko.md).

## 1. 확정된 요구와 현재 상태

사용자는 외부 Chrome/Firefox 없이 독립 창으로 실행하고, **현재 웹 UI를
내장 WebView에 표시**하는 형태를 선택했다. HTML을 버리고 별도 Rust 위젯 UI로
재작성하지 않는다. Python 런타임은 추가하지 않는다.

필수 현장 환경은 **RHEL 8.6 또는 8.10, 서버 실행 + ETX/X11 표시**다.
macOS에서 창이 뜨는 것, SSH XQuartz 실험, Firefox에서 성공한 검사는 이 현장
수용을 대신하지 않는다. 외부 브라우저 실행 경로도 별도로 유지한다.

현재는 **D2-mac 입출력·명시적 복구 및 시작/메뉴 사용성 보완, D3-mac 개발 패키지 고지 조립**까지 진행했다. macOS 12+에서 `floe2-desktop`과
로컬 개발용 `.app`을 빌드할 수 있다. 시스템 AppKit/WKWebView를 Rust 바인딩으로
호출하며 외부 브라우저나 Python 런타임을 사용하지 않는다. **RHEL 호스트와
배포용 패키지는 아직 미제공**이다. `floe2-web` 외부 브라우저 경로도 유지한다.

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
  순수 함수로 둔다. macOS 호스트는 실제 WKNavigationDelegate에서 이 정책을
  적용하고 main-frame 이외 탐색, 새 창, 미디어 권한을 거부한다. D2 다운로드는
  소유 origin의 blob 및 기존 clip/review POST 다운로드 경로만 별도로 허용한다.
  JS→네이티브 파일/명령 IPC는 추가하지 않는다.
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

Linux는 다음 후보를 비교한다. 아직 어느 것도 채택·완료되지 않았다.
macOS는 시스템 WKWebView 직접 바인딩을 선택했으며 이 선택으로 Linux ABI를
고정하지 않는다. Wry/Tao 의존성은 추가하지 않았다.

| 후보 | 확인할 핵심 |
|---|---|
| 시스템 API 4.0을 쓰는 얇은 Linux 호스트 | Rust 바인딩/FFI 안전성·유지보수, 시스템 WebKit 보안 업데이트, ETX 성능 |
| 지원 가능한 API 4.1 런타임 별도 동봉 | RHEL 8.6/glibc 2.28 기준 빌드, 하위 라이브러리 closure·라이선스·업데이트 책임, 용량 |
| 별도 Chromium 계열 내장 런타임 | RHEL 8/X11 지원 하한, 배포 크기·메모리·ETX 합성 비용 및 보안 업데이트 |

2026-09-21 사용자가 Electron 대안을 문의했다. 기존 웹 UI와 Rust 서비스/렌더러를
유지하고 작은 JS/TS 호스트에 Chromium을 동봉하는 구성이 가능하다. Python-free와
호스트까지 Rust-only는 다른 조건이다. Electron 기반 VS Code의 요구사항에는
RHEL 8·glibc 2.28이 있지만, 임의의 최신 Electron 번들 또는 ETX를 보장하지 않는다.
선정 버전/전체 ELF 의존성·sandbox·다중 사용자 메모리·ETX 입력/합성을 검증한다.
기존 후순위 결정은 Chromium이 ETX에서 느리다는 실측 결론이 아니다. 최소 호스트
사용자는 **Electron 최소 호스트 비교 진행**을 승인했다. 비교 후보를 구현하되
정식 채택은 RHEL/ETX 실측 뒤 결정한다. macOS 호스트는 유지한다.
별도 경계와 단계는 [Electron 비교 계획](WEBUI_ELECTRON.ko.md)에 기록한다.
[Electron 플랫폼 지원](https://github.com/electron/electron#platform-support),
[VS Code 요구사항](https://code.visualstudio.com/docs/supporting/requirements),
[동봉 엔진 보안 업데이트 책임](https://www.electronjs.org/docs/latest/tutorial/security).

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
| D1 | 플랫폼 호스트 선택, lock/vendor/고지, 독립 창·인증·표시·종료/실패 정리 | macOS 개발 호스트·인증/확인 종료 검사 구현; Linux·실패 복구 수용 잔여 |
| D2 | 메뉴·단축키·IME·DPR/resize/pan, 파일 선택·다운로드·클립보드, 저장·복구·창 닫기 | macOS 파일/편집 메뉴·PNG/텍스트 복사·명시적 복구 구현; 아래 잔여 수용 별도 |
| D3 | macOS 패키지, RHEL 8.6/8.10 ELF/런타임, 실제 ETX 입력·픽셀·지연·사용량 비교 | macOS 개발판 고지 조립/검사 구현; 서명·공증·배포/현장 수용 미완료 |

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

## 6. D1-mac 개발 호스트 (2026-09-18)

구현 경계:

- `desktop/`은 별도 Cargo workspace. 기존 `rust/Cargo.toml`, `Cargo.lock`,
  `vendor/`는 불변이며 headless/musl 기본 빌드에 GUI를 넣지 않는다.
  별도 lock의 registry 92개 중 기존 83개는 같은 version/checksum의 상대
  symlink로 재사용, 새 바인딩 9개는 Cargo vendor 원본(약 14 MiB)이다.
  `tools/validate_desktop_vendor.py`는 identity/checksum과 새 crate 모든 파일을
  검증한다. 원본 고지·upstream 보충 문서는 `desktop/NOTICES.md`에 연결했다.
- AppKit 메인 스레드가 WKWebView와 weak delegate의 수명을 소유한다.
  원래 Rust 서비스는 별도 스레드이며, 창 종료/실패/시그널 때 취소 후 join한다.
  SIGINT/SIGTERM은 명시적인 강제 종료 경로이고 저장 확정을 대신하지 않는다.
- 매 창은 비영속 WKWebsiteDataStore와 새 일회용 인증을 사용한다. 인증 URL은
  메모리로만 전달하며 로그에는 기존 비인증 loopback origin만 나온다.
- 닫기 버튼·메뉴/Dock Quit은 기존 웹 End session 확인창을 연다. Cancel이
  기본 포커스다. 확인 DELETE 이후 서비스 종료를 기다려 창을 닫는다.
  네이티브 창 닫기가 기존 파일 선택 모달의 포커스와 경쟁하는 결함을 실측해,
  종료 확인의 키/포커스 처리를 window capture로 올렸다. 기존 모달의 작업·
  초안은 폐기하지 않으며 종료 취소 시 그대로 돌아간다.
- 정책 밖 탐색·popup·download를 허용하거나 OS 외부 브라우저로 전달하지
  않는다. 파일 upload, 다운로드 save dialog, clipboard, 메뉴 편집/IME는 D2
  잔여다. 서버 내부의 Browse server files는 기존 scope와 읽기 계약을 유지한다.
- WebKit 실패에서 인증/저장 요청을 자동 재전송하지 않는다. 현재 JS가 동작하지
  않는 실패 화면은 제목으로 알리며 Ctrl+C로 정리한다. 재접속/장애 종료 UX는
  D2 수용 잔여다. macOS 결과는 RHEL/ETX 결과가 아니다.

개발 실행(리포 루트 기준):

```sh
(cd rust && cargo build --release --offline --locked -p floe-index -p floe-renderd)
(cd desktop && cargo build --offline --locked)
FLOE_INDEX_BIN="$PWD/rust/target/release/floe-index" \
FLOE_RENDERD_BIN="$PWD/rust/target/release/floe-renderd" \
  desktop/target/debug/floe2-desktop view /path/to/design.oas --depth full --detail high
```

`view`는 생략 가능하다. 캐시가 없거나 오래됐으면 화면에 안내하며 자동 색인하지
않는다. 색인은 기존 CLI 또는 웹의 명시적인 Index 승인 경로로 수행한다.
`--firefox`, `--no-open`, `--session-file`은 내장 호스트에서 거부한다.

```sh
sh tools/build_desktop_macos_dev.sh
sh tools/validate_desktop.sh
```

첫 명령은 `desktop/target/macos-dev.XXXXXX/Floe2.app`을 매번 새로 만들고 경로를
출력한다. 기존 묶음을 덮어쓰지 않는다. 인접 index/renderd를 포함하지만 **debug
개발용이며 서명·공증·release 성능 검증을 마친 패키지가 아니다**.
2026-09-21부터 의존 고지는 아래 §8처럼 조립하지만 배포 법적 검토 완료를 뜻하지 않는다.
두 번째는 실제 창을 띄우는 명시적 macOS 게이트다. 일반 headless 배터리에서는
GUI를 자동 실행하지 않고 source/vendor 감사와 공통 embedded lifecycle만 한다.

검증:

- 오프라인 build, host clippy `-D warnings`, 수명 unit 2개 통과.
- 실제 WKWebView `--smoke-test`: 빈 워크스페이스 인증 → 네이티브 닫기 → Cancel
  → application Quit → 확인 → 서버 join 통과. 초기 3회 대기는 성공으로 세지
  않는다. 계측 후 발견한 중첩 모달 포커스 충돌을 수정해 통과했다.
  테스트는 설계·reviewer·쓰기 scope 인자를 받지 않고 고정된 marker/boolean만
  출력한다. 120초 UI 전체 deadline은 기존 30초 native oracle과 별개다.
- 기존 `node tools/validate_web_ui.cjs`: ES2017 및 전체 결정적 UI 회귀 통과.
- 전체 `sh tools/validate_rust.sh`는 unit/CLI/runtime/embedded/layerprops 이후
  기존 `layer_defaults` native oracle의 30초 timeout으로 exit 1. 전체 green
  아님. 로그: `/private/tmp/floe-desktop-gates.6CjZyt/full.log`.
  `embedded_host,layer_defaults,web_ui,web_cli,web_startup` 선택 재검증에서는
  embedded/vendor·layer_defaults·web_cli는 통과했지만 GTK startup oracle이
  다시 30초 timeout이었다(`focused.log`). 이후 소스/기한 변경 없이
  `--only web_startup,web_ui`를 실행해 **exit 0 / ALL OK**를 확인했다
  (`startup-rerun.log`). 전체 통과로 합산하지 않는다.
- 최종 `sh tools/validate_desktop.sh`: **exit 0**, unit 2개 및 실제 WKWebView
  인증/닫기 취소/application Quit 확인/server join 통과(`native.log`).
- 실제 개발 `.app`에서 새 임시 합성 valmini를 full depth/high로 열어 화면의
  도형·색·라벨과 9개 레이어, 연결/final frame 표시를 확인했다. 오른쪽/왼쪽
  커서 pan으로 중심 이동·복원 및 margin crop generation 3/4도 확인했다.
  테스트 후 실제 닫기 버튼과 End session으로 종료, 프로세스 exit 0 확인.
  화면 캡처로 직접 확인했으나 정량 픽셀 oracle·input-to-photon 측정은 아니다.
  `data/m1` 및 공통 검증의 별도 이름 캐시는 뷰어 기본 경로와 달라 거부됐으며
  기존 캐시를 개명/덮어쓰지 않았다. 새 `/private/tmp/floe-desktop-layout.p9Mjp2`
  복사본에서만 2-worker 색인했다. 원본/고객 설계/DRC sidecar는 건드리지 않았다.

전체 목표 잔여: Linux 호스트/런타임 선택·RHEL 8.6/8.10 ETX 실측, D2 입출력·
복구 수용, D3 배포, 웹 잔여 브라우저 수용과 G1/G4 및 Python-free Linux 검증.
원격 공유/CI 실행 보류는 바꾸지 않았다.

## 7. D2-mac 파일·클립보드·복구 (2026-09-18)

§6은 당시 D1 기록이다. 이후 추가한 현재 동작:

- 웹의 `input[type=file]`은 NSOpenPanel로 **선택한 단일 파일**만 넘긴다.
  폴더 선택/일괄 재귀 업로드는 거부한다. 파일 크기·형식·reviewer scope·저장 승인
  검사는 기존 JS/서버에서 그대로 수행한다. 이것은 settings/review import용이며,
  로컬 OASIS를 임의로 서버 scope에 등록하는 새 기능은 아니다.
- WKDownloadDelegate와 NSSavePanel로 PNG·설정 blob, clip/review artifact의
  기존 POST 다운로드를 연결했다. URL은 소유 loopback origin의 정확한 경로만
  허용하고, 외부 URL/file URL/임의 popup은 거부한다. POST만 다운로드 전용
  비표시 WebView 1개에 원래 요청을 맡기고, 정확한 URL·HTTP 200·binary MIME의
  **응답 단계**에서 다운로드로 전환한다. HTML 응답은 표시하지 않는다. 최초
  응답 대기는 30초, 후속 탐색과 다운로드 redirect는 거부한다. POST body와
  cookie/CSRF를 호스트가 읽거나 요청을 복제하지 않는다. 실패/완료 시 전용 뷰를 정리한다.
  action 단계에서 바로 Download로 전환하면 실제 WebKit에서 POST body가 0바이트가
  되는 문제를 합성 clip으로 확인해 이 구조로 바꿨다.
- 웹 공통 다운로드 폼은 `noopener`를 유지하되 `noreferrer`를 제거하고 응답의
  Referrer-Policy를 `same-origin`으로 바꿨다. `no-referrer`가 non-CORS POST의
  Origin까지 `null`로 만드는 표준 동작과 서버의 exact Origin 검사가 충돌하기
  때문이다. 외부 referrer는 계속 차단되고, 인증값은 URL query에 두지 않으며
  bootstrap fragment는 HTTP referrer에 실리지 않는다. 서버의 Origin/CSRF/cookie
  검사는 완화하지 않았다. [Fetch Origin 규칙](https://fetch.spec.whatwg.org/#append-a-request-origin-header).
- 동시 다운로드 1개, 게시 파일 최대 512 MiB. 알려진 길이는 대화상자 전에,
  길이 미상은 전송 중 및 게시 전에 검사한다. 폴링 상한은 순간 디스크 사용량의
  엄밀한 상한이 아니다. 실패/취소는 자동 재시작하거나 resume data를 사용하지 않는다.
- 선택한 목적지와 같은 파일시스템의 무작위 0700 임시 폴더에 내려받고,
  완료 후 0600·sync 및 **no-clobber hard link**로 게시한다. 기존 파일과 dangling
  symlink, 선택 이후 생긴 동명 파일 모두 보존한다. **덮어쓰기는 아직 지원하지
  않으므로 새 이름을 선택**해야 한다. hard link를 지원하지 않는 파일시스템에서는
  명시적으로 실패하며 copy/overwrite로 조용히 대체하지 않는다. sync 결과 불명은
  목적지를 확인하라고 알리고 재시도하지 않는다. 정상 실패/종료는 소유 임시 파일만
  정리한다. 호스트 kill/OS crash 후 잔존 임시 폴더의 자동 청소는 미구현이다.
- Edit 메뉴의 Undo/Cut/Copy/Paste/Select All은 표준 responder chain을 사용한다.
  PNG는 기존 ClipboardItem promise + 사용자 클릭/키 입력 경로를 그대로 사용한다.
  클립보드를 읽는 네이티브 IPC나 주기적 읽기는 없다.
  [WebKit의 사용자 동작 기반 clipboard 계약](https://webkit.org/blog/10855/async-clipboard-api/).
- 앱 메뉴 `Recover View…`는 Cancel/Return 기본의 네이티브 확인 후 **같은 WebView**의
  credential-free root에 GET한다. 일회용 bootstrap, 저장 POST, 색인/clip 승인을
  다시 보내지 않는다. cookie/sessionStorage가 남으면 기존 웹의 receipt 조회와
  view 복원을 사용한다. 새 문서 로드 후 고정 marker만 제한적으로 확인하며,
  인증 페이지 재연결과 렌더/저장 결과 확인을 구분한다. 미저장 편집 초안·캡처는
  사라질 수 있고, sessionStorage까지 소실된 경우에는 새 앱 세션이 필요하다.
- `Force End Session…`은 JS가 응답하지 않아도 사용할 수 있는 별도 네이티브
  확인이다. 기본은 취소이며 승인 시 다운로드 취소·서비스 취소·worker join을
  한다. 기존 SIGTERM 취소 경로를 써 exit 143을 반환한다. 이미 승인된 서버 파일
  쓰기가 완료됐을 가능성을 안내하고, 저장을 대신 승인하거나 롤백하지 않는다.

검증 기록:

- host unit 5개: 서비스 수명, URL/method allowlist, 이름 정규화, 미완료 정리,
  게시 경쟁/no-clobber/0600/초과 크기/dangling symlink 검증. 오프라인 build·fmt·
  host clippy `-D warnings` 통과. registry crate 추가 없음(기존 getrandom 직접 사용과
  AppKit block2 feature만 lock에 추가), 기존 Rust workspace/vendor 불변.
- `sh tools/validate_desktop.sh`: 실제 빈 WKWebView 인증, 창 닫기→취소,
  application Quit→확인, 서버 join 통과. 최종 다운로드 방어 추가 후 재실행도
  exit 0(`/private/tmp/floe-d2-native-final.log`). 실제 valmini 앱도 다운로드·
  취소·복구 이후 End session으로 exit 0 확인했다.
- 실제 개발 `.app` + 임시 합성 valmini에서 1840×1382 PNG 및 native settings JSON
  저장, 파일 헤더/0600 확인, NSOpenPanel로 동일 JSON 재입력→9 rows 적용 통과.
  산출물 `/private/tmp/floe-desktop-io.tReHXO/`. 설계 기본값/실제 리뷰는 쓰지 않았다.
- 실제 clip POST→저장창→새 OASIS 게시 통과: `floe-clip-5.oas` 71,197 bytes,
  0600, OASIS 헤더 및 `floe-index scan`으로 단일 셀·3,585,415 도형 확인.
  이어 같은 다운로드의 Save 취소와 Recover View 후 프레임/조작 복원도 확인했다.
  DRC artifact는 같은 정책/스트림을 쓰지만 네이티브에서의 실제 검사는 아직 별도 잔여다.
- 웹 transport 15검사와 전체 ES2017/UI 회귀 통과. 공통 referrer 헤더와 두 다운로드
  폼의 `noopener`를 테스트로 고정했다(`/private/tmp/floe-d2-transport.log`,
  `/private/tmp/floe-d2-ui-final.log`). 임시 진단은 형식 boolean/길이만 사용했고 제거했다.
- 실제 Copy view 버튼 및 canvas Cmd+C는 PNG 복사 성공 상태 확인. 좌표 입력에서
  Cmd+C/Cmd+V로 **방금 복사한 합성 값** 복원 확인. 기존 클립보드 내용은 읽지 않았다.
- Recover View 확인창 Return은 미적용 `901.234` 초안을 유지했다. 명시적 Reload만
  실제 서버 좌표 `201.712…`로 복원, 같은 세션·레이어·프레임·조작 재활성화 확인.
  한 검사에서 표시 프레임 대기가 남았으나 후속 새 세션·재연결에서는 활성화됐다.
  이를 강제 WebContent crash/occlusion 전 조합 통과로 세지 않는다. 강제 종료 확인과
  서비스 exit 143도 확인했다. 실제 DRC 결과 불명 저장·WebContent process kill·
  cookie/storage 소실·IME/다중 화면 DPI 수용은 잔여다.
- 전체 `sh tools/validate_rust.sh`는 workspace unit, CLI/runtime, embedded,
  layerprops/layer_defaults 이후 **기존 GTK palette oracle의 30초 timeout**으로
  exit 1. 전체 green 아님(`/private/tmp/floe-d2-battery.log`). 같은 제한으로
  `--only embedded_host,layer_palette,web_ui,web_cli`는 **ALL OK**
  (`/private/tmp/floe-d2-focused.log`). 제한 완화·OS 보안 변경은 하지 않았다.
- POST 수정 후 전체 재실행은 앞서 실패한 palette 및 clip/capture/질의/stream,
  owner service 21검사, HTTP 권한 462 probes, 파일 선택/표시까지 통과한 뒤
  기존 **gtk_startup_oracle의 30초 timeout**으로 exit 1이었다
  (`/private/tmp/floe-d2-battery-final.log`). 전체 green으로 합산하지 않는다.
  같은 제한의 `--only web_startup,web_ui` 재확인도 startup에서 timeout/exit 1
  (`/private/tmp/floe-d2-startup-final.log`)이므로 그 실행의 web_ui는 미실행이다.
  위 별도 Node UI 회귀 통과와 구분한다. OS 앱 검사라는 원인은 확정하지 않았다.

실측 중 발견한 기존 자원 예약 이슈: 기본 뷰 1024 MiB + Browse 192 MiB +
clip 1024 MiB가 managed decoded 2048 MiB를 초과해 clip은 jobs 1이어도 `busy`다.
기본 decode 8 + raster 4 + Browse 1 + clip 4도 CPU 16을 넘는다. 이 단계에서는
공유 서비스의 자원 정책을 완화하지 않았다. 작은 합성 다운로드 검사는 기존 옵션
`--budget-mb 512 --jobs 2 --raster-jobs 1`로 별도 수행해 통과했다. 기본값에서의 clip
admission/실제 가용량 안내 개선은 이후 §10에서 처리했다. 위 결과는 수정 전 실측이다.

전체 목표 잔여: D2 장애·DRC/IME/DPI 확대 수용, Linux 호스트 및 RHEL 8.6/8.10
ETX, D3 배포/서명/고지, G1 성능·G4 대조·Python-free Linux 실행. 원격 공유·CI·
열린 색인 hot-reload 보류는 그대로이며 이 커밋을 전체 목표 완료로 세지 않는다.

## 2026-09-19 — 움직이는 중 버튼 놓기

macOS 독립 창에서 왼쪽 pan·오른쪽 박스 줌을 움직이는 중 놓으면 취소되고,
멈춘 뒤 놓으면 성공한다는 현장 보고가 있었다. 공통 `gestures.js`는
`mousemove(buttons=0)`을 즉시 취소로 처리하므로 뒤의 정상 `mouseup`도 버렸다.
그 순서는 합성 이벤트로 재현했다. 실제 보고된 WebView의 이벤트 순서를 수집해
확정한 것은 아니므로 현장 원인 확정과 회귀 테스트를 구분한다.

버튼 상태 0인 이동에서는 마지막으로 눌림이 확인된 미리보기를 유지하고,
최대 100ms 동안 원래 버튼의 `mouseup`을 기다린다. 정상 놓기는 유예가 끝날
때까지 기다리지 않고 즉시 적용한다. 버튼 상태 0만으로 이동·선택을 확정하지
않으며 반복된 이동은 기한을 연장하지 않는다. 놓기가 유실되면 타이머가 취소하고,
늦은 놓기는 무시한다. Escape·blur·숨김·resize·뷰 변경 취소는 유지한다.
기다리는 중 새 press는 이전 드래그를 취소하고 새 동작을 시작한다.

검증 범위: 왼쪽/중간 pan·오른쪽 band, 첫 rAF 전/후 놓기, 종료 좌표 반영,
누락·다른 버튼·늦은 놓기, 취소/재시작을 가짜 시계로 고정하며 owner와 guest의
실제 UI 핸들러에서도 0-button move → release 및 Escape를 검사한다.
UI는 실행 파일에 내장되므로 기존 `.app`을 재실행하는 것만으로는 갱신되지 않는다.
`sh tools/build_desktop_macos_dev.sh`로 새 앱을 만들고 출력된 경로를 실행한다.
실제 마우스의 빠른 드래그 재확인은 별도다.

검증 결과: release-order 46건과 owner/guest UI 통과,
`node tools/validate_web_ui.cjs` 전체 ES2017/결정적 UI gate 통과.
동일한 새 테스트를 수정 전 HEAD의 `gestures.js`에 적용하면 zero-button move
직후의 active 단언이 실패하는 것도 확인했다. 새 개발 `.app` 빌드와 실제
`--smoke-test`의 WebKit 인증 → 닫기 취소 → 종료 확인 → service join도 통과했다.
이 네이티브 smoke는 마우스 이벤트 순서 실측이나 pan/박스 줌 수용을 대신하지 않는다.

## 2026-09-19 — 시작·열기·메뉴 보완

- LaunchServices의 작업 디렉터리는 터미널과 다르다. `open -n … --args view
  quick.oas`에서 앱이 닫히던 것은 같은 상대경로를 찾지 못했기 때문이다.
  `open --args`에는 **소스 및 다른 경로 옵션을 절대경로로** 넘긴다. 원래 터미널
  디렉터리를 앱에서 추측하거나 홈 폴더로 바꾸지 않는다.
- 인자 파싱/서비스 시작·종료 오류는 stderr와 네이티브 오류창에 표시한다.
  bootstrap 링크는 오류 문구에서도 생략하며, 글자 수/제어 문자를 제한한다.
  오류 확인은 실패 작업을 다시 실행하지 않는다. `--help`는 GUI를 열지 않고,
  자동 `--smoke-test` 실패도 오류창에서 멈추지 않는다.
- SOURCE/`--root` 없이 시작하면 **작업 폴더를 먼저 명시적으로 선택**한다.
  Finder의 기본 cwd `/`를 권한 범위로 삼지 않는다. 취소는 서비스·worker를
  만들지 않고 exit 0. 선택한 기존 로컬 디렉터리만 `--root`와 같은 검사를 거쳐
  이번 세션의 초기 browse 범위가 되며 `/` 자체는 거부한다. 선택 자체는
  색인·설계/리뷰 저장을 실행하지 않는다. 기존 SOURCE/`--root` 지정은 유지되고,
  시작 후 범위를 추가하는 IPC나 Finder 문서 열기 이벤트는 추가하지 않았다.
- File → Open Layout… (`Cmd+O`), Open DRC Results…는 기존 승인 범위의 웹
  파일 선택기를 연다. DRC는 먼저 열린 레이아웃이 필요하다. 다른 모달/승인창을
  덮지 않고, 색인/저장 확인을 자동 승인하지 않는다. About은 기존 읽기 전용
  정보 화면, Close Window (`Cmd+W`)는 기존 End session 확인을 사용한다.
  Window → Minimize (`Cmd+M`) 및 Dock 재활성화는 **같은 세션** 창만 복원한다.
  데스크톱 프로세스 사이 단일 인스턴스 전달은 여전히 미구현이다.

상대경로를 쓰는 터미널 실행(호출자의 cwd 유지, 명시 worker 환경변수도 보존):

```sh
# 저장소에서 최초/변경 후 빌드
sh tools/build_desktop_macos_dev.sh
# 같은 저장소 디렉터리 안에 quick.oas가 있을 때
sh tools/run_desktop_macos_dev.sh view quick.oas
# 다른 디렉터리에서도 실행기 경로를 지정하면 그 디렉터리 기준으로 해석
sh /path/to/floe2_webui/tools/run_desktop_macos_dev.sh view quick.oas
```

최적화된 호스트 미리보기도 만들 수 있다. 기존 기본은 debug이며, worker는
두 경우 모두 release다. 매 빌드는 **새 `.app` 경로**를 출력하여 이전 앱을
덮어쓰지 않는다. `--release`도 unsigned/unnotarized 개발판이지 D3 배포 완료가 아니다.

```sh
floe_app=$(sh tools/build_desktop_macos_dev.sh --release)
open -n "$floe_app"                          # 초기 작업 폴더 선택
open -n "$floe_app" --args view /absolute/path/quick.oas
# 또는 최적화 바이너리를 호출자의 cwd에서 직접 실행
sh tools/run_desktop_macos_dev.sh --release view quick.oas
```

회귀 검사: `validate_desktop_launcher.py`는 격리된 합성 경로/가짜 실행 파일로
cwd·공백/한글 인자·명시 override·종료 코드·미빌드 안내·debug/release 패키징 및
새 출력 경로를 검사한다. `desktop/ui/menu-action.test.cjs`는 허용 버튼 외 실행,
비활성/숨김/로딩 및 모달 중 열기를 차단한다. 둘은 `embedded_host` gate에 배선했다.
시작 폴더 API는 유효성/최초 1회/기존 SOURCE·root 보존을 Rust unit으로 고정한다.
실제 Finder/오류창/메뉴 검사는 아래 실행 기록과 구분한다.

실행 기록:

- `floe-app --lib`: 32 통과, 기존 GTK oracle 2개는 이 명령에서 ignored.
  호스트 unit 6개, 실행기/패키징 합성 7개, 고정 메뉴 스크립트 검사 및
  호스트 `cargo clippy --all-targets --no-deps -- -D warnings` 통과.
  의존 프로젝트의 기존 경고와 호스트 검사 결과를 구분한다.
- `sh tools/validate_rust.sh --only embedded_host,web_ui`: ALL OK.
  새 초기 폴더가 실제 browse 범위로 연결되고 그 안에 파일을 만들지 않는
  lifecycle 단언 추가 후 `--only embedded_host`도 ALL OK. 전체 배터리
  재실행을 의미하지 않는다.
- 실제 debug/release WebView: 인증 → 초기 파일 선택기 닫기 → 네이티브
  About → 다른 메뉴의 모달 덮어쓰기 차단 → 닫기 취소 → 종료 확인/join 통과.
  처음 확장한 smoke는 초기 파일 목록 요청 중 Close를 눌러 timeout이 났다.
  목록 읽기 완료와 모달 닫힘을 각각 확인하도록 테스트를 수정했으며, 제한
  시간이나 모달 보호를 완화하지 않았다.
- 시작 모달을 `run()`보다 먼저 띄우므로 AppKit 시작 완료를 명시적으로
  알린 뒤 모달 루프에 들어간다. 통상 `run()`이 `finishLaunching()`을
  호출한다는 [Apple 계약](https://developer.apple.com/documentation/appkit/nsapplication/finishlaunching%28%29)에 따른다.
  이 보완 전 화면 제어 연결은 timeout, 보완 후 동일한 Finder 시작창을
  읽고 조작할 수 있었다.
- 실제 `.app`에서 새 빈 합성 폴더 선택 → 그 폴더 하나의 파일 선택기,
  Cmd+O 재열기, Cmd+W 종료 확인/종료를 확인했다. 합성 잘못된 옵션의
  오류창 및 Close 뒤 exit 1, 초기 폴더 선택 Cancel 뒤 앱 종료도 확인했다.
  실제 설계/리뷰 파일은 수정하지 않았다. Dock 복원·다중 화면/IME·실제 DRC
  저장 장애 수용까지 확대한 검사는 아니다.

남은 큰 작업은 RHEL 8.6/8.10 호스트·ETX 검증, 배포 서명/고지 수용,
DRC 저장 결과 불명·WebContent crash/저장소 소실 및 IME/DPI 수용이다.
원격 공유·CI·열린 색인 hot-reload 보류는 변경하지 않는다.

## 8. D3-mac 개발 패키지 고지·검증된 앱 실행 (2026-09-21)

`build_desktop_macos_dev.sh`는 기존 Rust portable packager의 오프라인 의존성
조회·원문 복사·유계 notice index를 재사용한다. macOS host/index/renderd의
native/build 의존성(빌드 전용 포함), 양쪽 Cargo lock/manifest, Rust toolchain
저작권/라이선스, 렌더러 글꼴 OFL을 `Contents/Resources/NOTICES`에 조립한다.
objc2 계열의 standalone license 부재는 기존 `desktop/NOTICES.md`의 출처 표시된
upstream MIT 보충문과 원본 README로 명확히 구분한다. vendor를 바꾸지 않으며
그 외 미정의 고지 누락은 오류다. 시스템 AppKit/WebKit/SDK는 동봉하지 않는다.

호스트에 notice index 식별자·소스 revision·target을 고정한다. About의 기존
인증된 읽기 전용 목록/본문 UI가 이를 사용한다. macOS 앱은 고정된
`Contents/Resources`에서만 찾고 cwd나 임의 환경변수 경로로 대체하지 않는다.
기존 portable은 실행 파일 옆 경로 그대로다. 파일 단위 크기·총량·페이징·
심볼릭 링크 거부 및 본문 chunk 검사는 공통 코어가 담당한다.
SHA-1은 여기서 **콘텐츠 식별**이며 발행자 인증/코드 서명이 아니다.

```sh
floe_app=$(sh tools/build_desktop_macos_dev.sh --release)
"$floe_app/Contents/MacOS/floe2-desktop" --check-notices
sh tools/run_desktop_macos_dev.sh --release view quick.oas
```

`--check-notices`는 모든 고지 chunk를 읽고 확인한 뒤 종료한다. GUI·listener·
worker·소스 파일을 열지 않고, 실패는 stderr/exit 1이다. 앱을 다른 디렉터리에
옮겨도 동작한다. 빌드는 이 검사까지 통과해야 성공 경로를 출력한다.

성공 후에만 `desktop/target/macos-preview-{debug,release}.path`를 원자적으로
교체한다. 실행기는 호출자 cwd와 명시 worker override를 유지하면서 해당 앱과
그 안의 worker를 기본 선택한다. 잘못된/사라진 앱 경로·symlink receipt는 오류로
알리며 조용히 다른 빌드로 넘어가지 않는다. receipt가 아예 없으면 기존 직접
`cargo build` 바이너리 실행은 유지한다(이 빌드는 고지 패키지를 제공하지 않음).
이전 `.app`을 지우거나 덮어쓰지 않고, 빌드 실패 때 이전 receipt도 유지한다.
자동 다운로드·서명·공증·외부 게시·사용자 파일 권한 확대는 없다.

회귀 검사는 `validate_desktop_launcher.py`의 합성 실행기/패키징과 Rust packager
unit에 더해, 실제 `.app`을 받아 새 임시 복사본만 손상시키는 별도 게이트다:

```sh
.venv/bin/python -B tools/validate_desktop_notices.py "$floe_app"
```

새 경로/한글·공백 이동, worker/Python 없는 실행 환경, 본문 변조·누락·같은 내용의
외부 symlink·index 변경 거부를 검사한다. 원본 앱과 설계 파일은 변경하지 않는다.
macOS 전용 실제 앱 검사는 headless/Linux 배터리에 자동 실행시키지 않는다.

실행 기록:

- `floe-app --lib`: 33 통과, 기존 GTK oracle 2개 ignored. packager unit 8개,
  host unit 6개, 합성 실행기/패키징 9개 통과. host/packager clippy
  `--all-targets --no-deps -- -D warnings` 및 Rust fmt 통과. 의존 VFS의 기존
  dead-code 경고 3개는 이 변경 범위에서 수정하지 않았다.
- 실제 debug·release `.app`: 고지 279파일 전 chunk 검사 통과. release 앱의
  별도 복사본에서 worker/Python 없이 경로 이동·누락·본문/index 변조·외부
  symlink 거부 통과. 실행기가 검증된 release 앱을 선택하는 것도 확인했다.
- 패키지 전용 `--smoke-test-notices`: 실제 WKWebView 인증 → About 목록
  → 본문 읽기 → 다음 64개 목록 → 닫기 취소 → application Quit 확인 →
  service join 통과. 고지 검사가 없는 기존 `--smoke-test`와 구분한다.
  두 QA 명령은 source/reviewer/write scope 인자를 받지 않는다. 디자인 픽셀·
  DRC 쓰기·물리 입력·IME/DPI 수용을 증명하는 테스트는 아니다.
- 합성 실행기 검사의 최초 실행은 `/var`·`/private/var` 경로 별칭 비교 1건이
  실패했다. 빌드 진입 경로를 물리 경로로 통일한 뒤 9개 모두 통과했다.
- `sh tools/validate_rust.sh --only embedded_host,web_portable,web_selfcheck,web_ui`:
  **exit 0 / ALL OK**. 공통 portable 실패/취소·자기 진단·embedded 수명·UI
  회귀가 통과했다. 전체 배터리나 Linux/ETX 수용을 뜻하지 않는다.
- 상세 로그: `/private/tmp/floe-desktop-notices-{unit,packager,host}.log`,
  `floe-desktop-notices-release.log`, `floe-desktop-notices-native.log`,
  `floe-desktop-notices-battery.log`.

전체 목표 잔여는 D2 확대 장애·DRC·IME/DPI/물리 드래그 수용, RHEL 호스트 및
8.6/8.10 ETX, 배포 서명/공증·고지 최종 수용, G1 성능/G4 대조와 Python-free
Linux 실행이다. 이 단계는 D3 선행 작업일 뿐 전체 목표 완료가 아니다.

## 9. D2-mac 조합 입력과 앱 단축키 분리 (2026-09-21)

코드 검사와 새 회귀로 파일 선택기의 `Enter`가 `isComposing=true`인 경우에도
검색을 제출하고 기본 동작을 취소하는 것을 재현했다. About·clip·DRC build·
공유 기본값 확인의 Escape에도 조합 판정이 빠져 있었다. 그 밖의 상당수
단축키는 `isComposing`만 확인하고 기존 메모/waive 입력기가 쓰는 229 호환
신호를 확인하지 않았다.

현재 규칙은 `isComposing || keyCode === 229`이면 **앱의 keydown 명령을 실행하지
않는 것**이다. native IME의 기본 처리에 `preventDefault`를 걸지 않는다.
파일 검색/대화상자 닫기·포커스 순환, 뷰/미니맵 이동·복사·깊이·드래그 취소,
레이어/채움 편집과 owner/guest DRC 순회를 같은 규칙으로 맞췄다. 기존 메모의
composition 상태 추적 및 내장 두벌식 입력은 그대로다. 입력 확정 후 일반 키의
동작은 유지하며, 타이머 기반 입력 지연이나 키 재실행은 추가하지 않았다.

229는 새로운 단축키 규약이 아니라 기존 브라우저 경계 이벤트에 대한 호환
처리다. [UI Events legacy keyCode 규칙](https://w3c.github.io/uievents/#determine-keydown-keyup-keyCode)과
[MDN 조합 중 keydown 설명](https://developer.mozilla.org/en-US/docs/Web/API/Element/keydown_event#keydown_events_with_ime)을
참고했다. 이 근거와 합성 이벤트 통과를 macOS/RHEL 입력기 전체 수용으로
확대하지 않는다.

검증 범위:

- 각 UI의 실제 handler에 `isComposing=true` 및 `isComposing=false, keyCode=229`
  이벤트를 넣고 요청 수·초안/대화상자·포커스·뷰/선택/드래그 상태 불변을 검사한다.
  일반 Enter의 검색은 한 번 제출되고, 일반 Escape는 기존처럼 취소/닫기한다.
- 실제 WKWebView `--smoke-test`에 빈 초기 파일 선택기의 조합 Enter/Escape/Tab
  합성 검사를 추가했다. 성공/실패 고정 marker만 반환하며 원래 입력값을 복원한다.
  `--smoke-test-notices`도 같은 검사 후 기존 About 고지·종료 수명을 검사한다.
  probe 자체도 이벤트 미지원·기본 취소·포커스/텍스트/대화상자 변경 시 실패하는지
  테스트한다. 이 JS는 명시적 네이티브 QA에서만 실행되며 제품 UI에는 추가하지 않는다.
- 합성 DOM 이벤트는 OS 후보창, 실제 물리 키, 엔진의 기본 form submit 및
  composition 이벤트 순서를 재현하지 않는다. macOS 한글/일본어 입력기와 현장
  RHEL/ETX의 실제 입력 수용은 여전히 별도로 남긴다.

실행 결과:

- 수정 전 `browse.test.cjs`의 새 검사는 조합 Enter의 `preventDefault`에서 실패했고,
  수정 후 확대된 전체 ES2017/UI 회귀가 통과했다. 테스트 개발 중 검색의 사전
  GET을 포함한 총 요청 수를 제출 수로 비교한 단언과 중복 test 변수명도 고쳤다.
- `sh tools/validate_rust.sh --only embedded_host,web_ui,web_hangul,validation_selector`:
  **exit 0 / ALL OK**. 내장 두벌식은 기존 GTK composer와 11,172음절,
  22,744시퀀스, 237,352전이 대조 통과. 이것은 OS IME 검사가 아니다.
- host unit 6개, fmt 및 `clippy --all-targets --no-deps -- -D warnings` 통과.
  release `.app`의 실제 WKWebView에서 `ime-ok` → About 고지 본문/페이징 →
  닫기 취소 → 종료 확인 → service join 통과. 새 앱 고지 279파일 무결성 및
  별도 복사본의 이동/누락/변조/symlink 거부 검사도 통과했다.
- 로그: `/private/tmp/floe-desktop-ime-before.log`(의도한 수정 전 실패),
  `floe-desktop-ime-ui.log`, `floe-desktop-ime-host.log`,
  `floe-desktop-ime-native.log`, `floe-desktop-ime-battery.log`.

이번 단계는 공통 UI 입력 결함을 없애는 D2 보완이다. DRC 저장 장애/프로세스
crash·storage 소실 복구, DPI/물리 입력, RHEL 호스트 및 전체 배포 게이트를
완료로 처리하지 않는다. 기존 원격 공유·CI·열린 색인 hot-reload 보류도 유지한다.

## 10. D2 기본 clip 예약 충돌 보완 (2026-09-21)

§7의 실패는 clip이 뷰의 1,024 MiB decoded LRU를 그대로 예약하고 기본 4 jobs를
요청한 데서 나왔다. 기본 뷰 decode 8 + raster 4, Browse 1을 유지하면 CPU가
17/16, decoded 예약은 2,240/2,048 MiB가 된다. jobs만 줄여도 메모리는 넘는다.

관리형 exact clip의 전용 LRU를 **min(뷰 cache, 256 MiB)**로 분리했다.
clip은 exact/full-depth 계획의 페이지를 배치로 순회하므로 작은 LRU는 이미
처리한 페이지의 보관량을 줄일 뿐 도형·레이어·반복을 생략하지 않는다. CLI의
`floe2 clip` 옵션이나 raster/cut은 바꾸지 않았다. 이 캐시는 active batch,
누적 clip geometry, 출력 OASIS 버퍼를 합친 RSS 상한이 **아니다**.

- 기본 jobs는 현재 미예약 CPU 슬롯에 맞춰 1~2를 제안한다. 사용자가 선택한
  1~16 jobs는 승인 후 바꾸지 않는다. CPU가 0이면 필드에는 1을 제안하되
  준비/승인은 비활성이다. 실행 중인 뷰/DRC/색인을 대신 중지하지 않는다.
- 기본 뷰 + Browse + DRC/rules + clip(2 jobs/256 MiB)의 총 예약은 CPU 16,
  worker 2, decoded 1,984 MiB다. 기존 상한 16/2/2,048을 늘리지 않았다.
- 인증된 export catalog에 미예약 CPU/worker/decoded 및 clip cache를 표시한다.
  OS의 실제 여유 메모리가 아닌 **단일 시점의 관리형 예약 가용량**이다.
  UI는 준비와 승인 전 확인하고, 실제 worker 시작은 같은 원자적 admission으로
  다시 검사한다. 경합 시 `busy`가 날 수 있으며 자동 재시도/축소는 없다.
- 자원이 없더라도 기존 artifact 다운로드/해제 및 결과 불명 요청의 동일 receipt
  재확인은 유지한다. 가용량 안내가 추가 파일 접근·저장 권한을 주지는 않는다.

검증:

- core 292개·web 122개 unit 통과(각각 ignored 7/3은 합산하지 않음).
  변경 crate fmt와 `clippy --all-targets --no-deps -- -D warnings` 통과.
- 실제 native clip gate에 기본 뷰/Browse/DRC-rules **예약을 보유한** j2/256 MiB
  실행을 추가했다. all/selected/none 출력이 기존 CLI golden과 바이트 일치하며
  기존 Python/j1/j8·KLayout XOR, 취소/reap·source/cache 보호·오류 주입도 통과했다.
  실제 네이티브 뷰·DRC 창을 동시에 조작한 부하 실측이라는 뜻은 아니다.
- owner HTTP/WS 21검사 통과. catalog 가용량과 작은 뷰 cache 유지(64 MiB)를
  검사한다. 전체 ES2017/UI 회귀에서 준비 차단·승인 전 가용량 감소·복구 후
  자동 제출 없음·사용자 jobs 보존·동일 요청 재확인·다운로드 유지도 통과했다.
- release 개발 `.app` 재빌드, 279개 고지 파일 검사, 실제 WKWebView의 인증·
  합성 조합 키·About·닫기 취소·종료 확인 및 service join 통과. 이 빈 작업공간
  smoke는 새 기본값의 native 저장창 수동 검사를 대신하지 않는다.
- 첫 선택 배터리는 기존 CLI 오류 주입 기대 문구 단언에서 실패했고, 진단을
  보강한 재실행은 `clip --help` 시작 자체가 50초 timeout이었다. 같은 바이너리의
  help가 정상 시작된 뒤 제한을 바꾸지 않고 재실행해 위 clip/owner/UI 검사를
  통과했다. 최초 실패의 원인을 macOS 보안 검사라고 확정하지 않는다.
- 로그: `/private/tmp/floe-desktop-clip-{unit,clippy,fmt,ui}.log`,
  `floe-desktop-clip-battery.log`(첫 실패), `floe-desktop-clip-battery-retry.log`
  (help timeout), `floe-desktop-clip-battery-final.log`,
  `floe-desktop-clip-{release,native}.log`. 첫 실행의 `embedded_host`는 별도로 통과했다.

이번 단계가 전체 D2 또는 현장 수용 완료를 뜻하지 않는다. 실제 OS IME/DPI/물리
입력, DRC·crash/storage 확대 장애 수용, RHEL 8.6/8.10 ETX 호스트, 서명/공증,
G1/G4 및 Python-free Linux 검증은 남는다. 원격 공유·CI·열린 색인 hot-reload
보류를 변경하지 않았다.

## 11. D2-mac 복구 deadline·오래된 콜백 격리 (2026-09-21)

§7 복구 코드에서 확인한 결함은 세 가지다. 로딩 실패는 `recovering`을 해제하지
않았고, 60회 probe 제한은 navigation 완료 이후에만 시작했다. JS completion이
아예 오지 않으면 다음 probe도 시작되지 않아 제한이 작동하지 않았다. 또한
동일 delegate를 사용하는 다운로드용 WebView의 종료를 주 화면 실패로 취급했고,
이전 navigation/전송의 늦은 callback이 새 상태를 변경할 여지가 있었다.

현재는 다음 규칙을 사용한다.

- 명시적 확인 후 시작한 **한 번의 복구 전체**(GET 로딩 + 인증 페이지 확인)를
  30초로 제한한다. 동시에 하나만 실행하며 JS 응답이 없어도 timer가 종료한다.
  OS가 앱을 정지시키면 실행 자체를 보장하지는 않지만, 재개 후 늦은 성공 응답도
  deadline 검사로 거절한다. 실패/만료는 busy 상태를 해제해 명시적 재시도 또는
  Force End Session을 사용할 수 있게 한다. 자동 reload/retry는 없다.
- 주 화면 WebView와 현재 `WKNavigation`의 identity를 확인한다. navigation은
  보관해 포인터 재사용과 혼동하지 않으며, JS completion은 별도 복구 epoch로
  묶는다. 이전 성공/실패는 새 복구를 완료시키거나 취소시키지 않는다. 실패는
  provisional과 committed 두 callback에서 처리한다.
  [Apple의 navigation 오류·content process 종료 구분](https://developer.apple.com/documentation/webkit/wknavigationdelegate).
- 인증된 페이지가 확인될 때만 이전 오류를 해제한다. 숨긴 창에서도 인증이
  확인되면 복구 대기는 끝내되, 창 활성화 후 **프레임·저장 receipt를 별도로 확인**
  하라고 표시한다. 페이지 인증을 렌더/DRC 저장 성공으로 확대하지 않는다.
- 다운로드 WebView 오류는 전송에만 적용한다. 주 화면 process 종료는 소유 중인
  전송을 취소하고 오류를 표시한다. 취소된 전송의 늦은 완료/실패는 새 전송을
  지우지 않는다. 복구 중 새 파일 대화상자·다운로드도 시작하지 않는다.
- 확인창 Cancel/Return 기본, 같은 WebView의 credential-free root GET,
  cookie/sessionStorage 재사용, bootstrap/저장 승인 비재생 계약은 유지한다.
  storage 소실을 새 인증 권한으로 대체하지 않으며 미저장 초안·캡처는 복원하지 않는다.

검증 범위:

- 순수 Rust 시간 주입 회귀: 로딩 미완료/JS 무응답 deadline, 실패 후 재시도,
  이전 epoch callback, 만료 직후 성공 응답, hidden/인증 대기, 중복 probe·완료,
  identity 소진을 검사한다. 호스트 기존 수명·전송 검사와 합쳐 13 unit 통과.
- `--smoke-test-recovery`는 별도 **빈 작업공간**에서만 동작한다. 인증 완료 문서에
  고정 합성 sessionStorage 값과 document marker를 넣고, 제품의 GET 복구 함수로
  재로딩한다. 이전 navigation의 완료/실패 callback을 주입해 새 요청에 영향이
  없는지 확인하고, 새 문서·저장소 보존·인증 및 확인 종료를 검사한다.
  JS는 고정 결과 marker만 반환하며 인증값·경로·메모·클립보드를 읽어내지 않는다.
- 이 검사는 사용자 확인창을 우회하는 일반 기능이 아니다. QA 옵션은 파일·DRC·
  reviewer 인자를 받지 않으며 새 세션만 사용한다. 정상 Recover 메뉴는 확인창을
  계속 거친다. Node 검사도 합성 값만 변경하고 타 storage 값은 보존한다.
- `sh tools/validate_desktop.sh`에 이 검사를 추가하고 `embedded_host` 선택 게이트에
  probe의 Node 회귀를 연결했다. 실제 native 실행 결과는 아래 기록으로 구분한다.

실행 결과:

- `sh tools/validate_desktop.sh`: **exit 0**. 13 unit, 기존 빈 창 smoke와 새
  `armed → recovered` 검사, 주입한 retired navigation 무시, 새 문서 인증,
  닫기 취소·종료 확인·service join 통과. host fmt/clippy `-D warnings`도 통과했다.
- `sh tools/validate_rust.sh --only embedded_host,web_ui,validation_selector`:
  **exit 0 / ALL OK**. 전체 배터리나 실제 OS crash 수용으로 합산하지 않는다.
- release `.app` 재빌드 및 279개 고지 파일 검사 통과. 그 앱에서도
  `--smoke-test-recovery`, `--smoke-test-notices`를 각각 실행해 **exit 0**을 확인했다.
- 로그: `/private/tmp/floe-desktop-recovery-{unit,clippy,native,battery}.log`,
  `floe-desktop-recovery-release.log`, `floe-desktop-recovery-release-native.log`.

이는 실제 WebContent process kill, cookie/storage 강제 소실, DRC 저장 중 결과
불명 복구의 대체 검사가 아니다. 그 확대 수용과 OS IME/DPI/물리 입력, RHEL 호스트,
서명·공증, G1/G4 및 Python-free Linux 검증은 여전히 남는다.

## 12. D2-mac 실제 DRC 저장·복구·다운로드 수용 (2026-09-21)

`209ff78`의 release 개발 `.app`을 새 합성 valmini + DRC pack으로 실행했다.
DRC는 `SYNTHETIC.SPACE` 규칙의 사각형 2개와 edge-pair 1개, 총 3오류다.
reviewer는 `native-d2` 하나만 등록하고 waive 편집을 명시적으로 허용했다.
실제 설계·기존 리뷰·다른 세션·공유 기본값은 사용하지 않았다.

이번 검사는 실제 AppKit/WKWebView UI를 조작했다. 전용 GUI 연결은 환경 오류로
사용할 수 없어, 이미 허용된 macOS 접근성과 **소유한 합성 앱 PID/창만** 대상으로
검사했다. 인증 URL/cookie/sessionStorage를 읽어내거나 HTTP API로 UI를 대신하지
않았다. 최종 종료 확인은 화면에서 버튼 위치를 확인해 클릭을 요청했지만 그
자동화 호출은 접근성 권한 오류로 끝났다. 접근성 API에서 그 웹 모달의 자식이
조회되지 않은 관측과 함께 별도 잔여이며, 최종 버튼 조작/VoiceOver 수용을
통과했다고 주장하지 않는다. 시스템 설정·권한 변경도 하지 않았다.

| 실제 검사 | 결과 |
|---|---|
| 오류 1 선택 → 메모 읽기 → 한글 포함 입력 → 미리보기 → 명시 승인 | `Saved · #1`, 새 note sidecar 생성 |
| 같은 오류의 Waive 선택 → 미리보기 → 명시 승인 | waive sidecar 생성, 복구 후 `Save completed · #1` 및 reader revision 일치 |
| 메모·waive 자동 저장 opt-in을 각각 켠 뒤 Recover View → Reload View | 인증된 같은 세션으로 복귀, 두 opt-in 모두 off로 초기화 |
| 복구 후 오류 1을 다시 읽기 | 입력한 한글 메모 완전 일치, `1 already waived · 0 reserved statuses` |
| Notes Prepare export → Download → 실제 NSSavePanel에서 새 경로 승인 | 228바이트 `.fe`, note sidecar와 `cmp` 일치, 파일 권한 0600 |
| 복구·export·종료 전후 보호 검사 | note/waive SHA-256 불변, 소스 OASIS·ASCII DRC·pack·VFS 파일의 크기/해시 불변 |
| 세션 수명 정리 | native 실행 exit 0, 소유 앱·renderd PID 모두 종료; 최종 확인 버튼 자동화 성공으로는 합산하지 않음 |

첫 메모 미리보기는 접근성 화면 조회 중 30초가 지나 만료됐고 저장되지 않았다.
새 스냅샷/미리보기에서만 승인해 저장했으며, 유효기간을 늘리거나 실패한 승인을
자동 재전송하지 않았다. 자동화 도중 waive 선택 스냅샷도 만료/무효화되어
폐기·재읽기했다. 이 과정을 정상 저장 성공으로 세지 않는다.
다운로드도 Save 버튼 직후 파일이 아직 없었으므로, 이후 호스트의
`Download saved (new file; existing files unchanged)`와 실제 파일을 모두 확인한
시점만 성공으로 기록한다.

합성 메모는 `Synthetic native save — 한글 복구 확인 2026-09-21`이다.
접근성 텍스트 입력이므로 OS 한글 IME의 조합/후보창 수용과는 다르다.
정상 저장 **후** 복구 검사이며, 저장 중 프로세스 종료·결과 불명·cookie/storage
소실·충돌 복구 검사를 대체하지 않는다. opt-in 이후 새 자동 저장 요청을 보내지
않았으므로 이번 기록의 자동 저장 수용 범위는 **복구 시 동의 초기화**까지다.
다운로드는 Notes만 확인했으며 Waives 다운로드/재import는 별도다.

실행 자료는 `/private/tmp/floe-desktop-drc.6Umo3s/`에 남겼다. 입력 fingerprint,
저장/복구 접근성 상태, 앱 창 캡처, 다운로드 및 테스트 sidecar를 포함한다.
별도 회귀도 host unit 13개 및 `node tools/validate_web_ui.cjs` 전체 검사를
통과했다(`host-tests.log`, `web-ui-tests.log`). 문서만 갱신했으며 전체 Rust/GTK
배터리 재통과로 확대하지 않는다.
소스/캐시/리뷰 산출물은 저장소에 커밋하지 않는다. 임시 자료는 OS 정리로
사라질 수 있으며 제품의 영구 테스트 입력으로 의존하지 않는다.

이번 단계는 제품 코드 변경이 아닌 실제 native 수용 범위 확대다. 남은 macOS
항목은 저장 중 장애/결과 불명·storage 소실, OS IME·DPI·물리 입력/접근성,
서명·공증·배포 수용이다. RHEL 호스트·ETX/Python-free Linux 및 G1/G4는 그대로
남고, 원격 공유·CI·열린 색인 hot-reload 보류도 유지한다.

## 13. D2-mac 합성 저장 응답 유실·재로딩 자동 검사 (2026-09-21)

§12의 수동 정상 저장 후 복구와 별도로, 실제 WKWebView + Rust 저장 경로의
반복 가능한 응답 유실 게이트를 추가했다. 명령은 **단독 옵션만** 받는다.

```sh
FLOE_INDEX_BIN="$PWD/rust/target/release/floe-index" \
FLOE_RENDERD_BIN="$PWD/rust/target/release/floe-renderd" \
  desktop/target/debug/floe2-desktop --smoke-test-review-recovery
```

`view`, SOURCE, `--root`, DRC/reviewer 등 다른 인자를 함께 주면 이 QA 모드로
진입하지 않는다. 기존 세션·인증 파일도 받지 않는다. Rust OASIS writer로 작은
사각형을 만들고, 고정 합성 ASCII DRC 2오류를 새 0700 임시 폴더에 쓴다.
기존 Rust 인덱서/DRC builder를 각각 2 jobs로 실행하며 Python은 사용하지 않는다.
준비 단계도 SIGINT/SIGTERM과 작업별 120초 deadline에 취소·join한다.
reviewer는 새 합성 자료의 `native-recovery-test`로 고정한다. 같은 이름의 다른
폴더 리뷰에 접근하지 않는다. 실행 폴더를 출력하고 작은 합성 산출물을 보존한다.

검사 순서:

1. 실제 UI에서 첫 오류 선택, 메모 입력/미리보기/명시 승인을 실행한다.
   QA 전용 document-start 스크립트가 그 요청을 **정상 전송**한 뒤 성공 HTTP
   응답 한 번만 UI의 network-error 경로로 바꾼다. 서버/디스크 장애 주입은 아니다.
2. 결과 불명 UI와 서버의 저장 receipt가 확인된 뒤 같은 WebView를 제품의
   credential-free GET 복구 함수로 재로딩한다. 시작 시점부터 요청 수를 세어
   자동 재전송이 없었는지 확인한다. QA의 Reload는 테스트 명령의 고정 동작이며,
   정상 Recover 메뉴의 확인창은 바꾸지 않는다.
3. 제품 UI의 **Resolve approved request**로만 원래 승인을 재확인한다.
   요청 수는 1→2지만 receipt는 계속 `#1`이어야 한다. 새 편집 승인/seq를 만들지
   않고, 재로딩 후 자동 저장 opt-in도 off여야 한다.
4. Waive에도 같은 유실·재로딩·명시 재확인을 적용한다. 디스크 저장뿐 아니라
   reader metadata가 receipt와 일치하고 read barrier가 풀렸는지 확인한다.
5. UI에서 메모·waive를 각각 다시 읽고, snapshot을 순서대로 해제한다.
   마지막에는 native 닫기→Cancel, application Quit→확인을 거쳐 service를 join한다.
   별도 Rust 검증이 원본 OASIS/ASCII/pack/색인 바이트 불변, note 내용·waived count,
   두 sidecar 권한 0600을 확인한다.

주입 스크립트는 이 합성 QA 호스트의 **주 문서에만** 설치한다. 일반 실행과
다운로드 WebView에는 설치하지 않는다. 정확한 두 API 경로의 POST 수/상태 코드만
보며 request body, 승인 token, 인증 header/cookie, response body를 읽지 않는다.
호스트로 나오는 값도 고정된 단계 marker뿐이다. document-start 설치 덕분에
페이지 초기화 중 자동 POST가 생겨도 검사에서 빠지지 않는다. 4xx/5xx를 성공한
응답 유실처럼 취급하지 않고 검사 실패로 처리한다. 새 registry 의존성은 없으며
기존 Rust OASIS writer와 이미 vendored된 WebKit 바인딩 feature만 사용한다.

개발 중 첫 실행은 `about:blank` 시점에 아직 없는 QA 스크립트를 실패로 판단했다.
인증된 페이지 준비 이후에 검사하도록 고쳤다. 다음 실행은 read-back에서 두
snapshot을 동시에 요청해 하나가 429로 거절되어 timeout이었다. 자원 제한은
바꾸지 않고 첫 snapshot을 해제한 뒤 다음 것을 읽도록 검사 절차를 수정했다.
자동 검사 창은 다른 창 뒤에 두어 사용자 키 입력 포커스를 가져오지 않는다.
이 결과를 실제 물리 키/IME 수용으로 확대하지 않는다.

검증 범위의 한계: **성공 응답 유실**과 실제 엔진 재로딩 수용이다. 실제
WebContent process kill, 저장 중 worker kill/디스크 장애, cookie/storage 강제
소실 또는 crash 뒤 OS 복구를 통과했다는 뜻이 아니다. 이 확대 장애 수용과
OS IME/DPI/접근성, 서명/공증, RHEL/ETX·G1/G4는 여전히 별도다.

실행 결과:

- host unit **15 passed**, fmt 및 clippy `--all-targets --no-deps -- -D warnings`
  통과. QA 단독 인자, 신규 파일 비덮어쓰기, 전송 경로/오류 구분, 문서 초기화 전
  대기 및 snapshot 순차 해제 회귀를 포함한다.
- `sh tools/validate_desktop.sh`: **exit 0**. 기존 빈 창·GET 복구 smoke와 새
  note/waive 응답 유실→재로딩→명시 재확인→UI/file read-back 검사 모두 통과했다.
- `sh tools/validate_rust.sh --only embedded_host,web_ui,validation_selector`:
  **exit 0 / ALL OK**. 전체 Rust/GTK 배터리 통과로 합산하지 않는다.
- release 개발 `.app`을 새로 빌드하고 고지 279파일을 검증했다. index/renderd
  환경 override를 제외한 **동봉 실행 파일만으로도** 새 native QA가 exit 0이며
  두 UI receipt `#1`, 파일 read-back/0600/입력 불변과 확인 종료를 통과했다.
- 로그: `/private/tmp/floe-native-review-ui.log`(초기 문서 준비 실패),
  `floe-native-review-ui-retry.log`(동시 snapshot read-back timeout),
  `floe-native-review-ui-final.log`, `floe-native-review-native-gate.log`,
  `floe-native-review-battery.log`, `floe-native-review-{unit,clippy,release}.log`,
  `floe-native-review-release-ui.log`.

## 14. D2-mac 응답 없는 닫기 요청 (2026-09-21)

네이티브 닫기/Cmd+W/Quit은 웹의 End session 확인창을 요청하지만, 기존에는
`evaluateJavaScript`의 완료 콜백이 오지 않으면 그 요청이 무기한 대기했다.
Force End Session 메뉴는 사용할 수 있었으나 닫기 버튼 자체에는 후속 안내가
없었다. 정상 완료/JS 오류와 달리 **콜백 자체가 없는 경우**를 보완했다.

- 닫기 확인 요청의 응답 대기는 **5초**다. 기한이 지나거나 확인창을 열지 못했다는
  응답이면 기존 native Force End Session 확인을 제공한다. **5초 뒤 자동 종료가
  아니다.** Cancel/Return 기본값과 명시적 End Session 승인, 저장 완료 가능성
  경고, 취소·worker join 계약을 유지한다. 다른 native 파일/확인창은 덮지 않는다.
- 응답을 기다리는 중 반복 닫기/Cmd+Q는 새 JS 요청을 쌓거나 기한을 연장하지
  않는다. 정상 웹 확인창의 `opened` 응답이 오면 타이머를 해제하므로 사용자가
  확인창을 오래 읽는 것은 제한하지 않는다.
- 요청별 ticket과 복구 epoch로 늦은 콜백을 버린다. 시간 만료·강제 종료 확인
  취소 이후의 새 닫기, Recover View, WebView 실패가 이전 콜백에 영향받지 않는다.
  종료 watchdog은 저장/색인/clip 승인이나 bootstrap을 재전송하지 않는다.

순수 상태 회귀는 응답 유실·중복 요청·정상/실패 응답·기한 직후 응답 경합·복구 및
무효화·취소 뒤 재요청·ticket 고갈을 검사한다. 기존 빈 워크스페이스 명령
`--smoke-test-recovery`도 확장했다. 복구 성공 뒤 닫기 JS 실행/콜백 하나를 의도적으로
생략하고 중복 native 닫기를 보낸다. 실제 5초 타이머와 NSAlert를 거쳐 **Cancel
컨트롤의 action**을 실행한 뒤 같은 세션 유지와 정상 닫기/취소/Quit을 확인한다.
테스트에 소스/reviewer/쓰기 경로를 지정할 수 없고 강제 종료를 자동 승인하지
않는다. 이는 실제 WebContent process hang/kill이나 물리 Return 입력의 수용을
뜻하지 않는다. 해당 장애 및 OS 입력·접근성, 서명/공증, RHEL/ETX·G1/G4는 남는다.

실행 결과: host unit **21 passed**(새 닫기 상태 6개), fmt 및 host clippy
`--all-targets --no-deps -- -D warnings` 통과. `sh tools/validate_desktop.sh`는
**exit 0**이며 실제 native Cancel 뒤 같은 인증 문서/메뉴/일반 종료가 동작했고
서비스가 join됐다. 이전 note/waive 응답 유실 복구와 입력 파일 불변 검사도 그대로
통과했다. 로그는 `/private/tmp/floe-close-timeout-{unit,clippy,native}.log`이다.
새 release 개발 `.app`에서도 worker 환경 override 없이 같은 복구/타임아웃/
취소/정상 종료 검사를 **exit 0**으로 통과했다(`floe-close-timeout-release-ui.log`).
개발 고지 279파일 검사는 통과했지만 서명·공증 완료 패키지는 아니다.

추가 선택 배터리의 첫 실행은 작업 트리에 `.venv`가 없어 selector 진입에서
exit 1이었다(`floe-close-timeout-battery.log`). 제품 실패와 구분한다. 정본의 기존
개발 interpreter와 KLayout/numpy를 읽기 확인하고 임시 `.venv` 링크로 재검증했다.
새 패키지 설치나 제품의 Python 런타임 추가는 하지 않았다.
같은 `--only embedded_host,web_ui,validation_selector` 재실행은 **exit 0 / ALL OK**
(`floe-close-timeout-battery-retry.log`)이며 검사 후 이번에 만든 링크만 제거했다.
기존 정본 가상환경은 변경하지 않았다. 선택 배터리 통과를 전체 Rust/GTK 및 현장
수용으로 합산하지 않는다.

## 15. D2-mac 세션 정보 소실과 재시작 안내 (2026-09-21)

인증을 복구할 수 없는 상태를 일반 네트워크 대기와 구분한다. 웹은 현재 문서의
인증 정보가 없거나 sessionStorage 읽기가 실패했을 때, 또는 현재 요청이 HTTP
401로 거절됐을 때 `restart-required` 고정 상태만 표시한다. 호스트는 비밀값이나
페이지 오류문을 읽지 않고 이 상태를 확인해 30초 기한을 기다리지 않고 **새 앱
세션 필요**를 알린다. 503/연결 실패를 세션 소실로 단정하지 않는다. 이전 문서의
늦은 401/복구 응답은 현재 문서에 적용하지 않는다.

기존 창은 자동 종료·재인증하지 않는다. 인증/저장 POST 및 일회용 bootstrap을
재생하지 않으며 메모/waive 등 pending journal을 지우지 않는다. 복구 불가 창의
종료는 기존 Cancel 기본 native Force End Session 확인을 따른다. 세션이 살아 있는
일반 GET 복구·숨김 창 처리·30초 네트워크 deadline은 유지한다.

새 **단독 인자 전용** 실제 native QA:

| 명령 | 실제로 변경하는 대상 | 확인 범위 |
|---|---|---|
| `--smoke-test-storage-loss` | 새 빈 WebView의 `floe-session:<자기 origin>` 키 하나 삭제 | 재로딩 후 인증정보 부재 인식, 재인증/쓰기 비재생 |
| `--smoke-test-cookie-loss` | 새 비영속 WKWebsiteDataStore의 cookie 타입만 공개 WebKit API로 제거 | sessionStorage는 유지하되 실제 HTTP 인증 실패 인식, 재인증/쓰기 비재생 |

두 명령은 source/root/reviewer/세션 경로를 받지 않는다. 기존 창·브라우저 프로필을
찾거나 재사용하지 않으며 쿠키 값·인증 header·request body를 읽지 않는다. QA 전용
주 문서 시작 스크립트는 HTTP method/path 카운터만 기록한다. 첫 문서에서 exchange
1회와 기존 읽기 전용 초기 목록의 browse POST 1회, 다른 쓰기 0회를 요구한다.
새 문서에서는 exchange/browse/쓰기 모두 0회를 확인한다. 다운로드 WebView에는 주입을
복제하지 않는다. 완료 후 **QA 소유 빈 서비스만** 취소·join하며 이것은 제품의 자동
재시작/자동 종료 동작이 아니다. 기존 자료 파일을 새로 쓰는 검사도 아니다.

이는 WebContent process kill/OS crash, 전체 sessionStorage 또는 저장 중 pending
journal 소실, DRC 저장 중 디스크 장애를 통과했다는 뜻이 아니다. 이번 실제 소실
범위와 별도로 남기며, OS IME/DPI/접근성·서명/공증·RHEL/ETX·G1/G4도 계속 남는다.

개발 중 기존 닫기 timeout smoke가 NSAlert의 Cancel 기본 상태 검사에서 중단됐다.
모든 버튼 추가 및 명시적 `layout()` 이후 Cancel key/initial responder를 지정하도록
구성 순서를 보완했지만 release의 속성 단언은 다시 실패했으므로 이것만으로 해결됐다고
보지 않는다. 이후 debug 진단 4회는 Return/initial 속성이 true여도 현재 포커스는
Cancel이 아닌 상태였고, release 실패 시 어느 속성이 달랐는지는 확정하지 못했다.

따라서 수용 판정을 실제 사용자 계약으로 바꿨다. Apple은 첫 버튼을 Return의
기본 버튼으로 설명하며 Cancel 이름에는 Escape 키가 붙을 수 있다고 명시한다.
`initialFirstResponder`도 현재 키 전달 결과를 뜻하지 않는다.
[NSAlert buttons](https://developer.apple.com/documentation/appkit/nsalert/buttons?language=objc),
[AppKit responder 설명](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/EventOverview/EventHandlingBasics/EventHandlingBasics.html).
QA는 초기 속성을 로그로 남기되 그 조합으로 기본 동작을 추정하거나 Cancel 버튼을
직접 클릭하지 않는다. 표시된 합성 sheet의 **자기 windowNumber로 만든 Return
NSEvent를 그 NSWindow에만 전달**하고, completion이 Cancel일 때만 성공이다.
다른 결과면 Force End action에 도달하기 전에 실패한다. 전역 키 이벤트, 실제 OS
키보드/IME/접근성 수용이 아니며 기존 사용자 창에는 이벤트를 보내지 않는다.

웹 UI 최초 회귀의 실패는 새 안내 문구의 단어 순서와 기존 `private session link` 기대의 차이였고,
사용자에게 동일한 지시를 주면서 기존 문구를 유지하도록 정리했다.
새 QA의 초기 실패는 읽기 전용 목록 조회도 POST 큐를 사용한다는 점을 카운터가
구분하지 못했기 때문이다. 기존 목록 프로토콜을 변경하지 않고 첫 조회 1회만
별도 집계하며, 조회가 아직 제출되기 전의 문서 준비 상태는 대기하도록 고쳤다.

검증 결과: host unit **23 passed**, fmt 및 최종 host clippy
`--all-targets --no-deps -- -D warnings` 통과. `sh tools/validate_desktop.sh` 최종
실행은 **exit 0**으로 기존 일반/복구/닫기 timeout/합성 DRC 검사를 통과했고,
실제 세션 키 삭제와 비영속 cookie 제거 각각에서 새 문서의 명시적 재시작 상태,
exchange/browse/다른 쓰기 0회 및 QA 서비스 join을 확인했다. 전체 Node 웹 UI
회귀도 문구 정리 후 **exit 0**이었다. 로그:
`/private/tmp/floe-session-loss-{unit,clippy-complete,native-complete,web-ui-retry}.log`.
초기 실패 로그(`native`, `native-retry`, `storage-diag`, `web-ui`)도 같은 접두사로
보존하며 성공 실행과 섞어 계산하지 않는다.
`sh tools/validate_rust.sh --only embedded_host,web_ui,validation_selector`도
**exit 0 / ALL OK**였다(`floe-session-loss-battery.log`). 검사에만 사용한 정본 개발
환경의 임시 `.venv` 링크는 제거했으며 새 Python 설치/제품 런타임 추가는 없다.
기존 lock/vendor는 불변이고 WebKit의 이미 vendored된 data-record 바인딩 feature만
추가했다. release 개발 `.app` 조립 및 고지 279파일 검사도 통과했다.
첫 release 실행에서는 세션 키/쿠키 소실 두 검사는 통과했지만 앞서 설명한 닫기
속성 단언이 실패했으므로 전체 release QA는 exit 1이었다
(`floe-session-loss-release-ui.log`). 이를 최종 Return 전달 검사와 구분한다.

최종 release 개발 `.app`에서도 **세션 키 소실 → cookie 소실 → 복구/닫기 timeout**을
각각 새 빈 세션으로 순차 실행해 **exit 0**이었다. cookie 검사는 실제 401 경로의
고정 UI 상태 `Session expired`, 세션 키 검사는 인증 요청 전 `Not connected` 상태를
각각 요구해 두 소실 경로를 혼동하지 않는다. Return 검사는 표시된 sheet에 전달된
이벤트의 실제 Cancel completion 및 세션 유지로 통과했고, 이후 정상 닫기/취소/종료도
성공했다. 실행 파일은 `desktop/target/macos-dev.gb7oX8/Floe2.app`의 내부 바이너리이며
환경변수 worker override 없이 번들된 worker로 실행했다. 로그:
`/private/tmp/floe-session-loss-release-complete{.log,-ui.log}`.

이 단계의 로컬 구현·검증은 완료했다. 목표 전체에서는 여전히 WebContent/worker
강제 종료와 저장 중 디스크 장애의 복구 검증, 실제 키보드/IME·DPI·접근성 수용,
서명·공증 및 RHEL 8.6/8.10 ETX/X11 호스트·현장 G1/G4 검증이 남는다. 개발용
release 조립/실행 성공을 배포 승인이나 전체 목표 완료로 간주하지 않는다.

## 16. D2-mac 개별 다운로드 중단 (2026-09-21)

응답을 받은 뒤 다운로드가 멈추면 기존에는 Recover View를 막는 활성 전송을
개별 취소할 방법이 없었다. 세션 전체를 끝내지 않고 **File → Stop Download…**로
현재 전송만 중단할 수 있게 한다. 확인창의 기본값은 Cancel이며, Save/Open 등
이미 열린 네이티브 모달을 덮지 않는다. Save 대화상자에서는 그 창의 Cancel을 쓴다.

승인하면 현재 비표시 전송 WebView/활성 WKDownload를 중단하고 소유한 private
staging만 기존 정리 경로로 제거 시도한다. 기존 Drop 정리는 파일시스템 오류를
별도 보고하지 않으므로 새 완료 문구는 임시 파일 제거 성공까지 단정하지 않는다.
열린 레이아웃·서버 artifact·완료된 다운로드는 유지한다.
POST/내보내기를 다시 제출하거나 중단 파일을 자동 재개하지 않는다. 전송이
확인창에서 기다리는 사이 완료됐다면 게시된 파일을 되돌리지 않고 완료됐음을
안내한다. 전송이 없을 때 메뉴를 다시 눌러도 세션/완료 파일은 그대로다.
Recover View의 차단 안내에도 이 메뉴를 표시한다.

새 단독 인자 `--smoke-test-download-cancel`은 caller source/root/reviewer/출력
경로를 받지 않는다. 새 빈 WebView에서 고정된 합성 텍스트 blob 하나를 만들고,
**실제 WKDownload destination delegate**까지 도달한 뒤 목적지 응답을 보류한다.
새 0700 합성 폴더에 만든 private staging에는 테스트가 직접 partial bytes를 쓴다.
이것은 실제 WebKit 수신 byte 진행률/네트워크 stall/디스크 장애 실측이 아니다.
사용자 NSSavePanel 조작·다운로드 publication과도 구분한다.

이 상태에서 실제 File 메뉴 selector와 native sheet로 다음을 확인한다.

1. sheet-local Return → Cancel: 전송과 partial bytes 및 완료 파일 sentinel 유지.
2. Stop Download 버튼 → 명시 중단: delegate 해제/cancel, 임시 payload/디렉터리
   제거, 새 목적지 파일 미생성, 완료 sentinel 동일성 및 서비스 생존.
3. 활성 전송 없는 중복 Stop은 새 확인창/세션 종료를 만들지 않음.
4. 이후 웹 준비·메뉴·정상 닫기/취소/종료와 service join.

보류한 destination callback은 정상 취소 및 실패/호스트 종료 때에도 정확히 한 번
null 목적지로 해제한다. 원래 WebView를 교체하거나 인증을 재발급하지 않는다.
검사 후 소유한 합성 파일만 정리하며 기존 파일/고객 자료는 사용하지 않는다.
단위 검사는 caller 인자 거부와 partial/완료 파일 분리를, JS 검사는 준비 이후
합성 blob 1회 생성만을 확인한다. headless `embedded_host`에는 JS 검사만 추가하고
실제 GUI는 명시적 `validate_desktop.sh`에서만 실행한다.

검증 결과:

- host unit **25 passed**, fmt 및 host clippy `--all-targets --no-deps -- -D warnings`
  통과. 의존 vfs의 기존 경고는 이번 host 검사와 별개다.
- `sh tools/validate_desktop.sh` **exit 0**: 기존 일반/복구/닫기 timeout/합성 DRC/
  세션 키·cookie 소실과 새 다운로드 취소의 실제 native 검사 모두 통과했다.
- `sh tools/validate_rust.sh --only embedded_host,validation_selector` **exit 0 / ALL OK**.
  정본 개발 interpreter를 검사에만 연결한 임시 `.venv` 링크는 제거했다. 새 패키지
  설치/제품 Python 의존성 또는 전체 Rust/GTK 배터리 통과를 뜻하지 않는다.
- 최종 release 개발 앱 `desktop/target/macos-dev.9K8nEq/Floe2.app`도 조립·고지
  279파일 검사를 통과했다. worker override 없이 번들된 worker로 다운로드 취소와
  복구/닫기 timeout을 각각 새 빈 세션에서 실행해 **exit 0**이었다. 자동 테스트의
  sheet-local Return/버튼 호출은 실제 물리 키보드/마우스 수용과 구분한다.
- 로그: `/private/tmp/floe-download-cancel-{native-complete,clippy-complete,battery}.log`,
  `floe-download-cancel-release-complete{.log,-ui.log}`.

이 단계의 macOS 개별 다운로드 중단은 완료했다. 전체 목표에는 crash/디스크 장애와
정리 실패의 구체적 오류 보고, 실제 IME/DPI/접근성, 서명/공증, RHEL/ETX와 G1/G4가
계속 남는다. 기존 사용자 소개·브로셔 변경과 현장 jobdeck 작업은 포함하지 않는다.

## 17. D2-mac 다운로드 임시 파일 정리 실패 보고 (2026-09-21)

§16에서 남겼던 조용한 Drop 정리 오류를 명시적인 결과로 옮겼다. `PendingFile`의
`publish()`는 **게시 결과와 cleanup 결과를 따로 반환**하고, `discard()`는 cleanup
실패를 반환한다. 성공한 게시 뒤 정리만 실패해도 이미 저장한 파일을 되돌리거나
저장 실패로 바꾸지 않는다. 게시가 불명확하면 기존처럼 목적지 확인을 요청하며
재전송하지 않는다. 명시적 결과를 낸 뒤 Drop에서 몰래 재시도하지 않는다.
예외적 unwinding/setup 이탈의 RAII fallback도 실패 시 오류 종류만 stderr에 남긴다.

삭제 대상은 소유한 `payload`와 그 빈 임시 디렉터리 두 개뿐이다. payload 대신
디렉터리가 있거나 다른 파일 때문에 디렉터리가 비지 않으면 오류로 남기며, 재귀
삭제·목적지 삭제·알 수 없는 파일 제거를 하지 않는다. 이미 없는 경로는 정리된
상태로 인정한다. 정상 게시, 전송 실패/취소, 크기 제한, Save 취소/경합, WebView
장애와 종료의 native 경로는 이 결과를 받아 처리한다.

처음 실패한 OS 오류 종류를 보관하고, **Download cleanup warning**을 창 제목의
앞에 붙인다. 이후 메뉴/복구/새 상태가 이 경고를 지우지 않으며 세션은 계속 사용할
수 있다. 실제 경로·설계명·원시 오류문을 로그에 보내지 않는다. 사용자가 종료하면
완료된 service join 뒤 일반 오류 반환으로 알리므로 Finder 실행도 기존 native
오류창 경로를 타며 exit 1이다. 자동 재삭제/재저장/새 인증은 없다. 사용자가 이미
수동으로 정리했는지는 재탐색하지 않으므로 문구는 **정리가 확인되지 않았음**이다.

검증 범위:

- 실제 파일시스템 단위 검사: 성공한 게시 + cleanup 실패, 목적지 경합 + cleanup
  실패의 두 결과 분리; payload가 디렉터리인 경우 내부 파일 보존; 없는 경로의
  성공 처리. 기존 파일·symlink·크기 제한/원자 게시 검사도 유지한다.
- 새 단독 인자 `--smoke-test-download-cleanup-failure`는 §16과 같은 새 빈
  WebView/합성 blob/새 private 폴더만 사용한다. QA가 만든 알려지지 않은 파일을
  임시 디렉터리에 추가해 실제 `DirectoryNotEmpty`를 발생시킨다. 첫 확인 취소는
  모든 파일을 유지하고, 명시적 중단은 payload만 제거한다. 추가 파일·완료 sentinel
  보존, 중복 Stop 뒤 경고, 후속 메뉴·닫기·종료 뒤에도 경고/오류 종류 유지를 확인한다.
- native 검사는 정상 세션 종료/join까지 확인한 뒤 제품과 같은 cleanup 오류를
  반환한다. runner는 **exit 1 + 고정 QA 성공 표식 + 정확한 cleanup 종료 안내**를
  모두 요구한다. 관계없는 panic/비정상 종료가 통과할 수 없으며, 이 예상 실패
  케이스만 일반 오류창을 생략한다. 따라서 실제 종료 오류창의 버튼 조작은 이
  자동 검사가 대신하지 않는다.
- QA teardown은 자신이 만든 추가 파일 한 개와 알려진 임시 경로만 정리한다.
  사용자 파일이나 기존 브라우저 프로필은 사용하지 않는다. 이 결과는 ENOSPC,
  NFS 단절, 저장 중 전원/프로세스 종료 또는 물리 디스크 고장 수용이 아니다.

최종 검증 결과:

- host unit **29 passed**, fmt와 host clippy `--all-targets --no-deps -- -D warnings`
  통과. 게시·cleanup 결과 분리, 알 수 없는 파일/디렉터리 보존 검사를 포함한다.
- `sh tools/validate_desktop.sh` **exit 0**. 기존 일반/복구/DRC/세션 소실/정상
  다운로드 취소 및 새 정리 실패 native 검사를 모두 통과했다. 정리 실패 케이스의
  프로그램 자체는 예상대로 exit 1이며, runner가 고정 표식·정확한 오류 문구까지
  검사한 뒤 성공 처리한다. 첫 정리 실패부터 후속 메뉴/종료까지 경고를 유지한다.
- `sh tools/validate_rust.sh --only embedded_host,validation_selector` **exit 0 / ALL OK**.
  출력 없는 대기 중에도 같은 살아 있는 프로세스를 유지해 완료를 확인했으며
  재시작하지 않았다. 검사에만 연결한 정본 개발 `.venv` 링크는 제거했다.
- release 개발 앱 `desktop/target/macos-dev.MmC9qz/Floe2.app` 조립 및 고지 279파일
  검사를 통과했다. worker override 없이 번들 worker로 정상 다운로드 취소 exit 0,
  합성 정리 실패의 정확한 오류/exit 1을 확인했고 전체 검사 wrapper는 exit 0이었다.
- native QA는 Objective-C 객체 해제 시점에 의존하지 않고 `run()` 반환 전에
  자신이 만든 알려진 테스트 파일/디렉터리의 teardown을 명시적으로 확인한다.
- 로그: `/private/tmp/floe-cleanup-report-{native-complete,clippy-complete,battery}.log`,
  `floe-cleanup-report-release-complete.log`. release 실제 실행의 고정 표식과 종료
  코드는 실행 도구 결과에도 남겼다. 전체 Rust/GTK 배터리, 실제 오류창 버튼 조작,
  서명/공증 또는 현장 수용의 완료를 뜻하지 않는다.

이 단계의 정리 실패 보고는 완료했다. 전체 목표에는 확대 장애 복구·실제 IME/DPI/
접근성, 서명/공증, RHEL 8.6/8.10 ETX 호스트와 현장 G1/G4 검증이 남는다.

## 18. D2-mac 다운로드 경로 교체 보호와 실제 게시 (2026-09-21)

§17의 path 기반 정리에 두 재현을 추가했다. private staging을 이동하고 옛 이름을
다른 합성 폴더의 symlink로 바꾸면 취소가 다른 폴더의 payload를 지웠다. 선택한
저장 폴더를 이동·교체하면 publication이 다른 폴더의 payload를 새 목적지로
게시했다. 기존 코드에서 두 검사 모두 실패했고, 고객 자료 없이 새 합성 경로만
사용했다. 단순한 저장 실패 문구가 아니라 파일 작업의 기준을 수정해야 하는 문제다.

`PendingFile`은 선택 시 canonical parent와 private staging 디렉터리의 핸들을
보관한다. payload 검사·진행 크기·게시·정리는 `openat/fstatat/linkat/unlinkat`의
핸들 상대 단일 이름으로 실행한다. symlink를 따라 payload를 열지 않고, regular
file·512MiB·단일 hardlink를 검사한 뒤 같은 열린 파일에 0600과 sync를 적용한다.
다른 파일의 hardlink를 0600으로 바꾸는 것도 거부한다. 목적지의 기존 파일을
덮어쓰지 않는 link publication과 게시/정리 결과 분리는 유지한다.
디렉터리를 열고 보관하므로 선택 폴더에는 읽기·쓰기·탐색 권한이 필요하다.
권한/핸들 확보 실패 때 안전하지 않은 path 방식으로 우회하지 않는다.

WebKit에 staging URL을 넘기기 전과 게시 전후에 parent/staging의 device·inode를
검사한다. 수신 중 경로 변경 또는 크기 조회 오류도 명시적으로 중단하고 별도
transfer WebView를 정리한다. 정리는 원래 열린 staging의 payload만 unlink하고,
parent에 남은 임시 이름의 identity가 같을 때만 빈 디렉터리를 제거한다. 이동된
폴더를 찾아 재귀 삭제하거나 새 위치에 저장을 재시도하지 않는다. 변경/불명확한
정리는 §17의 경고로 남으며, 성공한 게시를 cleanup 실패 때문에 재생하지 않는다.

macOS/APFS 합성 검사에서는 `rmdir` 뒤에도 열린 디렉터리의 link count가 2로
남았다. 링크 수만으로 정리 완료를 판단하지 않는다. 기존 임시 이름이 없을 때에는
공개 `F_GETPATH`로 핸들의 위치를 읽고 그 위치가 없는지 확인한다. 위치가 살아
있거나 조회가 불명확하면 정리 오류이며, 반환 경로는 삭제에 사용하지 않는다.
API와 버퍼 크기는 [Apple fcntl 문서](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html),
디렉터리 이동 추적 용도는 [Apple WatchRoot 문서](https://developer.apple.com/documentation/coreservices/kfseventstreamcreateflagwatchroot)를 참고했다.

보호 범위에는 한계가 있다. WebKit 공개 destination API는 fd가 아닌 **파일 URL**을
받으므로 마지막 확인과 WebKit 파일 열기 사이를 원자화하지 못한다. POSIX의
mkdir→open 또는 identity 검사→link/unlink 역시 비협조적인 same-UID/root의
이름 변경에 대한 filesystem CAS가 아니다. 이 단계는 확인된 경로 교체·symlink/
hardlink 사례를 막는 보완이며, 적대적으로 계속 바뀌는 파일시스템의 완전한 격리나
NFS/디스크 장애 수용을 주장하지 않는다. 실제 서버 저장 정책은 별도 검증 대상이다.

직접 의존성으로 선언한 `libc = 0.2.189`는 이미 desktop lock/vendor에 있던 동일
패키지다. lock에는 host의 의존 연결 한 줄만 추가했고 새 registry 패키지·버전·
checksum 또는 vendor source는 변경하지 않았다. Linux native host 구현을 뜻하지 않는다.

새 `--smoke-test-download-publish`는 caller 파일/출력 경로를 받지 않는 단독 QA
모드다. 기존 취소 QA와 달리 합성 blob의 실제 WKDownload에 새 private staging
URL을 넘긴다. 테스트가 payload를 대신 쓰지 않으며 WebKit 완료 callback에서
정상 publication을 실행한 뒤 원본 blob bytes·0600·임시 파일 제거·완료 sentinel
보존을 확인한다. 이후 일반 메뉴/닫기/service join과 알려진 합성 파일만의 명시적
teardown을 요구한다. 사용자 SavePanel 클릭·대용량 전송·실물 디스크 고장은 별도다.

검증 결과:

- host unit **35 passed**, fmt·host clippy `--all-targets --no-deps -- -D warnings`
  통과. 경로 교체 두 건, 교체 없이 이동된 staging의 경고, 삭제된 staging의 정상
  처리, symlink/hardlink 대상의 내용·권한 보존을 포함한다.
- `sh tools/validate_desktop.sh` **exit 0**. 실제 WebKit blob의 새 파일 저장뿐 아니라
  기존 일반 메뉴/닫기·복구·합성 DRC·인증 소실·다운로드 취소·정리 실패도 통과했다.
- release 개발 앱 `desktop/target/macos-dev.8XmH5K/Floe2.app` 조립과 고지 검사를
  통과했다. 번들 worker만으로 실제 게시·취소 exit 0과 정리 실패의 정확한
  QA 표식/오류 exit 1을 검증했다. 전체 wrapper는 exit 0이다.
- vendor 검사 **92개 잠금 패키지, 기존 공통 source 불변**. 고지 279파일은 별도
  합성 복사본에서 이동·변조·누락·symlink 거부까지 통과했다. 서명/공증은 아니다.
- `sh tools/validate_rust.sh --only embedded_host,validation_selector` **exit 0 / ALL OK**.
  환경 점검 2개, 런처 9개, JS·vendor 및 실제 내장 서비스 수명 검사까지 완료했다.
  이 선택 결과를 전체 배터리 통과로 대체하지 않는다. 로그는
  `/private/tmp/floe-download-directory-battery-selected.log`다. 두 검사 종료 후
  검사 전용 `.venv` 링크만 제거했으며 정본 interpreter는 변경하지 않았다.
- 로그: `/private/tmp/floe-download-directory-{tests-final,clippy-final,native,release}.log`.
  이전 구현의 두 실패는 `floe-download-directory-before.log`, release 실제 실행의
  표식·종료 상태와 고지 변조 검사는 실행 도구 결과에 남겼다.

- `sh tools/validate_rust.sh` 전체 재시도는 약 14분 실행 후 **명시 중단(exit 143)**했다.
  새 test executable 실행마다 긴 대기가 반복됐고, 이 시점까지 실패한 assertion은
  없었지만 전체 완료도 아니다. 실행 중단을 통과나 코드 오류로 바꾸어 기록하지
  않는다. 중단 직전 notices 5개까지 통과하고 oasis 검사 시작 단계였다. 로그는
  `/private/tmp/floe-download-directory-battery-full.log`다. 전역 배터리 완료는 후속에
  남긴다. OS 보안 설정을 바꾸거나 검사 skip을 늘려 우회하지 않았다.

이번 macOS 다운로드 경로 보호·실제 게시 단계는 완료했다. 전체 목표에는 실제
process/worker·디스크 장애 복구, 물리 IME/DPI/접근성, 서명/공증, RHEL/ETX 및
현장 G1/G4와 전역 배터리 완료가 계속 남는다.

## 19. D2-mac 실제 렌더러 종료와 마지막 화면 표시 (2026-09-21)

장애 수용 중 공통 UI에서 두 재현을 고정했다. worker failure snapshot 뒤에도
retained margin crop이 상태줄을 `Live`로 덮었고, 같은 revision의 늦은 PNG decode/
raw packet이 실패 뒤 다시 착지할 수 있었다. 수정 전 Node 검사에서 각각 실패했다.
픽셀 계산/렌더러를 바꾸는 문제가 아니라 **마지막 화면과 살아 있는 결과의 구분**이다.

실패한 view의 마지막 화면은 참조용으로 유지하되 `failed · last displayed image
(not live)`로 표시한다. 첫 프레임 전 실패라면 화면이 없음을 표시한다. 남은 perf도
previous frame으로 표시하고 margin 준비 안내는 지운다. 진행 중 PNG decode를
취소·해제하고, terminal view의 늦은 raw/PNG는 revision이 같아도 discarded ACK로
거부한다. 실패 상태에서 탐색은 차단하고, 사용자가 **Close layout → Open layout**으로
다시 열 수 있다고 안내한다. 자동 재시작·이전 입력/저장 재생·새 인증 발급은 없다.

검증은 세 층으로 나눈다.

- 공통 UI Node: 빈 화면/foreground/margin 세 상태 각각에서 실패 표시·남은 픽셀·
  decode 취소·늦은 PNG/raw 거부·URL 해제·조작 차단·명시적 닫기/재열기를 단언한다.
- native QA DOM probe: 실제 착지한 margin과 실패/닫기 상태를 고정 표식으로 읽는다.
  인증/저장소/요청 본문을 읽지 않는다. probe 자체의 상태 판정도 별도로 검사한다.
- **실제 macOS WKWebView + renderd SIGKILL**: 개발 driver가 새 앱을 실행하고
  `ps`의 pid/ppid/comm만 읽어 그 앱의 유일한 직접 자식과 정확한 renderd 실행 경로를
  두 번 확인한다. 확인된 PID만 한 번 종료한다. 실패 표시, 닫기로 두 buffer 제거,
  명시적 다시 열기, 이전 자식 회수와 다른 PID, 새 margin 프레임, 닫기 취소와 정상
  service join/exit 0을 모두 요구한다. 기존 사용자 앱을 이름으로 찾아 종료하지 않는다.

새 단독 인자 `--smoke-test-renderer-failure`는 외부 driver용이며 source/reviewer/
추가 인자를 받지 않는다. 기존 합성 fixture 생성기를 재사용하되 새 layout만 열고
DRC·reviewer는 등록하지 않는다. source/pack/cache read-back 동일성을 확인한다.
Python과 `ps`는 **개발 검사에만** 쓰며 설치 제품의 의존성/자동 재시작 기능이 아니다.
private WebKit PID API는 추가하지 않았다. headless `embedded_host`에는 probe와
PID 선택 순수 단위 검사만 들어가며, 실제 앱 실행은 `validate_desktop.sh`에 있다.

직접 실행 예시(합성 파일만 새로 생성):

```sh
FLOE_INDEX_BIN="$PWD/rust/target/release/floe-index" \
FLOE_RENDERD_BIN="$PWD/rust/target/release/floe-renderd" \
python3 -B tools/validate_desktop_renderer_failure.py desktop/target/debug/floe2-desktop
```

검증 결과:

- host unit **36 passed**, fmt와 host clippy `--all-targets --no-deps -- -D warnings`
  통과. 최종 진단 probe 보완 후에도 재검사했다.
- 최초 debug host의 실제 SIGKILL/명시적 재열기 검사는 **exit 0**이었다. 이전
  자식 회수·다른 PID·새 margin·정상 종료와 합성 입력 동일성까지 확인했다.
  로그: `/private/tmp/floe-renderer-failure-native.log`.
- `sh tools/validate_rust.sh --only embedded_host,web_ui,validation_selector` 최종
  **exit 0 / ALL OK**. 빈/foreground/margin 세 실패 회귀, ES2017 및 전체 결정적
  UI 검사, probe·PID 선택 검사, 실제 내장 서비스 수명을 포함한다. 로그는
  `/private/tmp/floe-renderer-failure-battery-final.log`; 검사 전용 `.venv` 링크는
  제거했다. 수정 전 두 실패는 `floe-worker-failure-before-{margin,foreground}.log`다.
- 최종 release 개발 앱 `desktop/target/macos-dev.3YRqNP/Floe2.app` 조립과 고지
  279파일 검사는 **exit 0**. 로그는 `floe-renderer-failure-release-final.log`다.
  이 번들의 실제 GUI 장애 검사는 아직 완료하지 않았다.
- **전체 native 재검증은 미통과**다. 첫 suite의 새 케이스가 exit 1이었고, 후속
  진단 실행은 `renderer-document-hidden`으로 첫 margin을 기다리다 exit 1이었다.
  고정 표식이 없던 첫 실패의 원인까지 동일하다고 단정하지 않는다. 최종 suite는
  기존 일반 메뉴 검사에서 `menu=unavailable`, step 2 대기 후 120초 deadline으로
  exit 1이었다. 로그는 `floe-renderer-failure-{native-suite,diagnostic,native-final}.log`.
  따라서 최초 단독 성공을 반복 실행·최종 번들·전체 native 회귀 성공으로 합산하지 않는다.

이 native gate는 실제 창이 보이는 상태에서 실행해야 한다. 숨겨지거나 완전히
가려진 WebView가 프레임 수신/메뉴 작업을 중지하는 정책을 우회하거나, Node 결과로
실제 프레임 조건을 대체하지 않았다. 최종 창 가시성 환경의 반복 검증은 잔여다.

이 검사는 빈 합성 작업의 렌더러 프로세스 장애다. 실제 WebContent process 종료,
저장 도중 crash/디스크 고장, 물리 키보드·IME/DPI/접근성, 고객 대형 설계/현장
수용을 대신하지 않는다. 이번 화면 제어 연동은 `CUA_REPL_ENABLED_SURFACES is
required`로 사용할 수 없어 수동 화면 조작·스크린샷은 별도로 남긴다.

## 20. D2-mac 숨겨진 메뉴의 진단 분리 (2026-09-21)

jobdeck 정방향 통합 `2445068` 뒤 새 release 개발 앱을 조립했다. 고지279파일의
전체 chunk 검사 및 실제 앱의 격리 복사본에 대한 경로 이동·누락·변조·symlink
거부는 통과했다. 최초 번들은 `desktop/target/macos-dev.g02LO5/Floe2.app`이며
로그는 `/private/tmp/floe-webui-sync4-desktop-{release,notices}.log`다.
이는 서명/공증·GUI 수용을 대신하지 않는다.

해당 번들의 `--smoke-test-notices`는 인증·합성 조합키·초기 Browse 닫기까지
진행했으나 `menu=unavailable`에서120초 deadline으로 **exit1**이었다
(`floe-webui-sync4-desktop-native.log`). 이 값은 숨겨진 문서와 없는/비활성
버튼을 구별하지 않았다. 새 진단은 `document.hidden`일 때만 `hidden`을 반환한다.
제품은 **창을 활성화한 후 메뉴를 다시 선택하라**고 안내하며 불필요한 Recover
View/재로딩을 권하지 않는다. 버튼 부재·비활성·모달 guard와 허용된 세 메뉴만
실행하는 정책은 유지한다. 활성화 시 메뉴를 자동 재실행하지도 않는다.

수정된 debug 호스트의 실제 빈 합성 세션은 같은 지점에서 `menu=hidden`을
보고하고 service를 취소·join하여 **exit1**로 끝났다
(`/private/tmp/floe-desktop-visibility-native.log`). 이 실행의 실패가 WebKit의
문서 가시성 판정 때문이라는 근거이며, 앞선 모든 실패 원인이나 OS/연동 설정의
근본 원인을 확정하는 것은 아니다. native QA는 거부된 메뉴 뒤에 진행할 수
없으므로 이 경우 명시적 가시성 실패로 종료한다. 숨김 guard 제거·가짜 표시·
timeout 연장으로 게이트를 통과시키지 않는다.

- Node 메뉴 회귀: 숨김/모달/비활성/허용 목록을 구별하고 숨김 중 클릭0회,
  다시 보인 뒤에도 모달이 있으면 기존 guard가 유지됨을 확인했다.
- host fmt·unit **36 passed**·host clippy `--all-targets --no-deps -- -D warnings`
  통과. dependency warning이 없는 전체 workspace라는 주장은 아니다.
  로그: `/private/tmp/floe-desktop-visibility-{unit,clippy}.log`.
- `sh tools/validate_rust.sh --only embedded_host,web_ui,validation_selector`:
  **exit0 / ALL OK**. 실제 내장 서비스 수명·launcher/vendor·메뉴/복구 probe와
  전체 ES2017/UI 회귀를 포함한다. 로그: `floe-desktop-visibility-battery.log`.
  검사 전용 `.venv` 링크는 제거했다. 이것을 native GUI 통과로 집계하지 않는다.
- CUA 재확인도 여전히 `CUA_REPL_ENABLED_SURFACES is required`였다. 실제 표시
  화면/물리 입력과 최종 native suite는 통과하지 않았다. 현장 RHEL/ETX,
  WebContent/저장 중 crash, IME/DPI/접근성, G1/G4·배포 수용은 계속 남는다.

## 21. D2-mac 확인창의 Return 기본 취소 (2026-09-21)

`72e28e7` 개발 앱의 새 빈 `--smoke-test-recovery` 실행은 인증·명시 GET 복구·
새 문서/기존 저장소 확인까지 진행했다. 이후 강제 종료 **제안** 확인창에 보낸
sheet-local Return이 처리되지 않아120초 후 **exit1**이었다. 최종 성공으로
집계하지 않는다(`/private/tmp/floe-desktop-recovery-current.log`).

동일 NSAlert 설정의 별도 AppKit 대조에서 비활성 창은 `initial responder=Cancel`,
`default cell=Cancel`이어도 표시 후 키 equivalent가 빈 문자열이었다. 기존의
`layout → Cancel Return 지정 → beginSheet`만으로 이 경로를 보장하지 못했다.
`beginSheet` **이후** `NSWindow.defaultButtonCell`을 Cancel 셀로 다시 지정한 경우
같은 Return·Enter 이벤트가 Cancel로 처리됐다. 이는 로컬 비활성 합성 창의 근거이며
모든 macOS/활성 창에서 기존 물리 Return이 실패했다고 일반화하지 않는다.

제품은 `confirmation::set_cancel_default`를 공통 확인창에 연결한다. AppKit의
공개 default-button API만 쓰고 전역 키 후킹·자동 클릭·확인 승인·가시성 우회는
없다. Cancel 초기 포커스와 승인 버튼의 Return equivalent 제거는 유지한다.
예상 button cell/default를 설정할 수 없으면 승인 버튼을 비활성화하고 취소를
안내한다. 정상 승인 동작은 계속 명시적인 사용자 선택만으로 실행한다.
이미 vendored된 AppKit 바인딩의 NSCell/NSActionCell/NSButtonCell feature만 켰으며
새 패키지·lock 변경·vendor 원본 수정은 없다.

Apple은 defaultButtonCell을 창이 Return/Enter를 받았을 때 동작하는 대상으로
설명한다. `NSAlert`의 버튼 순서·Cancel 이름에 따른 초기 key equivalent와
창의 실제 키 전달을 구별한다.
[defaultButtonCell](https://developer.apple.com/documentation/appkit/nswindow/defaultbuttoncell),
[NSAlert 버튼](https://developer.apple.com/documentation/appkit/nsalert/addbutton(withtitle:)).

검증은 다음처럼 분리한다.

- `cargo test --offline --locked`: **36 unit 통과**. 일반 실행은 새 GUI 테스트를
  빌드·실행하지 않는다. fmt 및 feature 포함 host clippy도 통과했다.
  커밋 직전 같은 코드의 재검증도 exit0이다. 재검증 로그는
  `/private/tmp/floe-cancel-final-{unit,native,clippy}.log`에 둔다.
- 새 opt-in 실제 AppKit 테스트는 제품 helper를 그대로 사용한다. 새 빈 창만
  만들고 WebView·서비스·파일·인증을 사용하지 않는다. **키 미입력 유지, Return
  취소, Enter 취소** 3대조를 통과했다. 시간 초과 정리로 Cancel completion이
  발생해도 그 전에 결과를 판정하므로 키 성공으로 둔갑하지 않는다.
  로그: `/private/tmp/floe-desktop-cancel-native.log`.
- 수정된 전체 호스트의 복구 QA는 `recovered → Return Cancel → session preserved`
  를 확인했다. 이후 메뉴 단계는 `document.hidden`으로 **exit1**이다.
  `/private/tmp/floe-desktop-cancel-recovery.log`. 이 부분 관측을 전체 복구 suite
  통과로 보고하지 않는다. 숨김 메뉴 guard와 deadline은 변경하지 않았다.
- Escape는 이 비활성 AppKit 실험에서 기존·수정 설정 모두 직접 sheet 이벤트로
  처리되지 않았다. Escape/물리 키·포커스/접근성 수용은 미검증으로 남긴다.
  이를 Return 테스트의 성공에 포함하거나 실제 사용자 환경의 Escape 회귀로
  단정하지 않는다. 실제 화면 제어 연결도 아직 사용할 수 없다.
- `--smoke-test-download-cancel`도 같은 제품 helper에서 Return 취소 후 전송/
  staging 보존, 명시 Stop 후 payload 제거·완료 파일/세션 보존까지 확인했다.
  이후 숨김 메뉴 실패로 **exit1**이므로 전체 다운로드 수용 통과가 아니다.
  로그: `/private/tmp/floe-desktop-cancel-download.log`.
- `sh tools/validate_rust.sh --only embedded_host,web_ui,validation_selector`는
  **exit0 / ALL OK**. 기존 vendor 92개·원본 checksum 검사도 통과하고 검사 전용
  `.venv` 링크는 제거했다. `floe-desktop-cancel-battery.log`에 기록하며 전체
  배터리·native GUI 통과와 구별한다. dependency 경고는 계속 남는다.

명시 네이티브 게이트(`tools/validate_desktop.sh`)에만 다음 실행을 배선했다.
headless 배터리는 GUI를 암묵적으로 실행하지 않는다.

```sh
cd desktop
cargo test --offline --locked --features native-confirmation-qa --test native-confirmation
cargo clippy --offline --locked --all-targets --features native-confirmation-qa --no-deps -- -D warnings
```

나머지 G1/G4·WebContent/저장 중 crash·OS IME/DPI·RHEL/ETX·서명/공증 수용은
그대로 남는다. 위 테스트는 실제 표시·물리 키보드 수용의 대체가 아니다.

## 22. D2-mac 종료 요청 시 기존 창 복원 (2026-09-21)

코드 검토에서 Dock Quit의 `applicationShouldTerminate → request_close`는
최소화 상태에서 기존 웹 종료 확인을 요청하면서 창을 복원하지 않는 것을 확인했다.
실제 Dock 클릭으로 재현했다고 보고하지 않는다. 종료 요청 시작에서 소유 창의
`deminiaturize → makeKeyAndOrderFront`를 호출하고, 기존 Dock reopen도 같은
`window_visibility::reveal` helper를 사용한다. 이미 native sheet가 열렸을 때에도
그 부모 창은 먼저 복원하되 `panel_open` guard는 그대로 반환한다.

sheet 해제·확인 승인·서비스 종료·파일 저장·reload·메뉴 재실행·다른 앱 활성화는
helper에 없다. 기존 취소 기본값, 저장/복구 guard와 종료 응답 검사는 유지한다.
`orderOut`으로 숨긴 창/최소화한 창의 복원과 앱 전체 Hide·Spaces·다른 앱에 가려진
화면의 실제 포커스/가시성을 구분한다. 후자는 이번 자동 검사로 증명하지 않는다.

- `cargo fmt -- --check`, 일반 unit **36 passed**, native QA feature를 포함한
  host clippy `--all-targets --no-deps -- -D warnings` 통과. 처음 QA 컴파일의
  `Message` trait import 누락을 수정하고 다시 통과했다. 의존성 경고는 남는다.
- 명시 `native-confirmation` AppKit 검사 **exit0**: 새 창이 숨김/최소화 상태에
  실제 진입한 것을 먼저 단언하고 제품 helper가 이를 복원하는지 확인한다.
  열린 sheet의 부모를 숨겼다가 복원해 같은 sheet가 부착되어 있고 completion이
  호출되지 않았음을 단언한다. 기존 무입력/Return/Enter 취소 검사도 통과했다.
  앱 run loop에서 최대3초 관측하며 timeout을 성공/강제 복원으로 바꾸지 않는다.
  설계·WebView·서비스·파일을 사용하지 않고 다른 앱에 입력을 보내지 않는다.
  로그: `/private/tmp/floe-desktop-reveal-{unit,clippy,native}.log`.
- 화면 제어 재확인은 `CUA_REPL_ENABLED_SURFACES is required`로 실패했다.
  실제 Dock Quit·물리 입력·전체 native UI 수용을 위 helper 검사로 대체하지 않는다.
- 수정된 debug 제품의 새 빈 `--smoke-test`와 `--smoke-test-recovery`는 각각
  **exit0**였다. WebKit 인증·합성 조합키 guard·About/모달 guard·native close 취소·
  application quit 확인·서비스 join과, 명시 GET 복구/기존 sessionStorage/인증
  재실행 없음·실제5초 종료 확인 timeout/중복 요청 제한/Return 기본 취소를 확인했다.
  로그: `/private/tmp/floe-desktop-reveal-{smoke,recovery}.log`.
  최소화한 **제품**에서 Dock을 누르는 물리 검사는 아니며, 이전 간헐적 hidden
  실패 원인을 이 창 복원 수정으로 모두 해결했다고 일반화하지 않는다.

제품 바이너리/전체 게이트의 결과와 미완료 항목은 별도로 기록한다. RHEL 호스트
선택·ETX, G1/G4, OS 입력/접근성, 장애 복구 및 서명/공증의 전체 목표는 유지한다.

## 23. 명시 native suite 완주 및 개발 앱 (2026-09-21)

`34a4139`의 `sh tools/validate_desktop.sh`를 생략 없이 실행해 **exit0**을 확인했다.
`/private/tmp/floe-desktop-34a4139-full.log`. 실제 AppKit 취소/복원, 빈 WebView
인증/메뉴/종료와 복구뿐 아니라 다음 합성 경로를 포함한다.

- 메모/waive 저장 ACK 유실 뒤 인증된 reload·동일 receipt 명시 확인, 파일 내용과
  0600 read-back. 자동 POST 재실행이 없고 원래 입력은 그대로다.
- sessionStorage/cookie 소실 후 종료 상태와 새 시작 안내. bootstrap을 재사용하지 않는다.
- 실제 WKDownload blob 게시/read-back, 취소 시 staging만 제거, 알 수 없는 파일을
  둔 정리 실패의 sticky 경고와 **예상 exit1**을 외부 harness에서 성공적으로 구별했다.
- driver가 자신이 생성한 host의 유일한 직계 renderd를 두 번 재검사한 후 한 번
  종료했다. 이전 픽셀을 Live로 표시하지 않음, 명시 close/reopen, 새 frame,
  이전 자식 수거·다른 PID의 새 자식, 정상 종료를 관측했다. 원본 source/pack/cache는
  변경되지 않았다. WebKit WebContent 프로세스의 실제 crash 검사는 아니다.

별도 release 개발 앱은 `desktop/target/macos-dev.5SWzV9/Floe2.app`에 만들었다.
고지279개 검사 및 복사본에서 재배치/누락/변조/symlink 거부 검사 **exit0**.
기존 앱 덮어쓰기·설치·서명/공증은 없고 source revision은 `34a4139` 기준이다
(사용자 문서 변경 때문에 dirty 표시가 붙는다). 로그는
`/private/tmp/floe-desktop-reveal-app-{build,notices}.log`에 있다.

이번 명시 suite의 성공은 이전 실패 기록을 삭제하지 않는다. CUA 실제 화면·
물리 Dock/IME/DPI/접근성, 화면/입력 성능 G1, 전체 G4와 RHEL/ETX 및 배포 수용은
남는다. 정규 Rust/웹 전체 배터리와 이 native suite도 서로 다른 게이트다.
