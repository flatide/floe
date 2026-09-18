# floe2 독립 창 + 내장 WebView

작성: 2026-09-18. 정본 상위 계획: [WEBUI_PLAN.ko.md](WEBUI_PLAN.ko.md).

## 1. 확정된 요구와 현재 상태

사용자는 외부 Chrome/Firefox 없이 독립 창으로 실행하고, **현재 웹 UI를
내장 WebView에 표시**하는 형태를 선택했다. HTML을 버리고 별도 Rust 위젯 UI로
재작성하지 않는다. Python 런타임은 추가하지 않는다.

필수 현장 환경은 **RHEL 8.6 또는 8.10, 서버 실행 + ETX/X11 표시**다.
macOS에서 창이 뜨는 것, SSH XQuartz 실험, Firefox에서 성공한 검사는 이 현장
수용을 대신하지 않는다. 외부 브라우저 실행 경로도 별도로 유지한다.

현재는 **D2-mac 입출력·명시적 복구의 첫 구현**까지 진행했다. macOS 12+에서 `floe2-desktop`과
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
개발용이며 서명·공증·배포 고지 조립·release 성능 검증을 마친 패키지가 아니다**.
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
admission/실제 가용량 안내 개선은 별도 추적한다.

전체 목표 잔여: D2 장애·DRC/IME/DPI 확대 수용, Linux 호스트 및 RHEL 8.6/8.10
ETX, D3 배포/서명/고지, G1 성능·G4 대조·Python-free Linux 실행. 원격 공유·CI·
열린 색인 hot-reload 보류는 그대로이며 이 커밋을 전체 목표 완료로 세지 않는다.
