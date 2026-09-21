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
