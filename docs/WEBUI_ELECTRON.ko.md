# Electron 독립 앱 비교

2026-09-21. 상위 [웹 전환](WEBUI_PLAN.ko.md), [데스크톱](WEBUI_DESKTOP.ko.md).
사용자 승인: **Electron 최소 호스트 비교 진행**. 기존 macOS WKWebView 앱 유지.
정식 호스트 채택·배포는 RHEL 8.6/8.10 + ETX/X11의 호환성/메모리/입력 지연을
확인한 뒤 결정한다. Chromium이 느리거나 빠르다고 미리 단정하지 않는다.

## 범위와 단계

| 단계 | 산출물 | 완료 근거/잔여 |
|---|---|---|
| E0 | 기존 Rust Session의 전용 파이프 sidecar, JS 클라이언트, 수명/인증 회귀 | 구현·실제 Node↔Rust 합성 검사; 아래 계약 |
| E1 | sandboxed Electron 독립 창, 같은 웹 번들, 시작/종료/실패 처리, 런타임 고정/검증 | 다음 구현. E0만으로 독립 앱 사용 가능이라고 하지 않음 |
| E2 | 합성 레이아웃 입력/표시, 시작/RSS/CPU/input→표시 비교 도구, native 메뉴/입출력/복구 수용 | 비교·측정 필요. 단순 HTTP 시간은 input→photon이 아님 |
| E3 | RHEL 전체 ELF/라이브러리 의존성, sandbox·ETX/다중 사용자 실측, 라이선스/업데이트/오프라인 배포 | 현장 대기, OS 패키지/보안 설정 변경 없음 |

Rust geometry·렌더러·색인·DRC·파일 권한/저장 API를 JS로 옮기지 않는다. Electron은
기존 정적 웹 UI와 작은 JS 호스트를 사용하므로 Python-free지만 호스트까지 Rust-only는
아니다. 웹/기존 macOS 호스트의 대체·삭제, Node를 Rust CLI 필수 의존성으로 추가,
원격 공유 공개, implicit indexing, `--no-sandbox` 우회는 포함하지 않는다.

## E0: 전용 파이프와 수명

`electron/service`는 별도 Cargo workspace다. 기존 `rust/vendor`만 사용하고 기존
Rust/macOS manifest·lock/vendor 원본은 바꾸지 않는다. `desktop/src/service.rs`를
직접 재사용해 signal/cancel/join 동작을 복제하지 않는다. 별도 Node 클라이언트도
표준 모듈만 사용하며 npm install/postinstall은 없다.

1. sidecar의 stdin/stdout은 상속한 FIFO 또는 Unix socket이어야 한다. 터미널,
   일반 파일, TCP socket을 거부한다. 이는 직접 리다이렉션 방어이며 신뢰된 부모가
   받은 값을 고의로 저장하는 것까지 막는 보안 경계라고 주장하지 않는다.
2. 부모는 `{"v":1,"args":[...]}` 한 줄을 보낸다. 인자는 Rust `Session::parse`가
   해석한다. 프레임64KiB, 인자256개/각8KiB 상한; UTF-8/공백/개행은 JSON으로 보존한다.
3. 소스/root가 없으면 `directory` 이벤트 뒤 명시 폴더 선택만 받는다. 기존 Rust
   initial-directory 정책을 사용하며 임의 cwd/home를 자동 browse root로 추가하지 않는다.
4. ready의 origin/일회용 URL은 원래 Rust 검증을 거쳐 전용 stdout으로만 전달한다.
   인증 파일·argv·stderr 로그에는 쓰지 않는다. Node도 malformed 프레임/자식 stderr를
   그대로 출력하지 않는다. 부모/자식 둘 다 고정 버전/키/상태를 검사한다.
5. 부모 stdin EOF 또는 `cancel\n`은 Rust 세션 취소다. Rust가 정리/join한 뒤 종료한다.
   잘못된 control도 세션을 닫고 오류다. 정상 웹 End session은 기존 인증·CSRF·DELETE를
   거쳐 exit0이다. 파일 승인/저장/인덱싱 권한은 파이프에 추가하지 않는다.

검증 명령(개발용 Node만 필요, Electron 런타임은 아직 실행하지 않음):

```sh
cd electron/service
cargo test --offline --locked
cargo clippy --offline --locked --all-targets --no-deps -- -D warnings
cargo build --offline --locked
FLOE_ELECTRON_SERVICE_BIN="$PWD/target/debug/floe-electron-service" \
FLOE_INDEX_BIN="$PWD/../../rust/target/release/floe-index" \
FLOE_RENDERD_BIN="$PWD/../../rust/target/release/floe-renderd" \
node --test ../service-client.test.cjs ../service-lifecycle.test.cjs
```

Rust5 unit·host clippy, Node4 프레임/상태 검사와 실제 파이프5개 검사 **9/9 통과**.
실제 새 빈 폴더의 Node↔Rust 검사에서 일회용 교환,
GET/DELETE의 기존 CSRF 검사, browse scope, 정상 종료/EOF 후 listener 소멸,
폴더에 파일0개, stdout 일반 파일이면0바이트/exit1, malformed control의 오류/정리와
초기 malformed 입력을 로그/credential로 내보내지 않음을 확인했다. 첫 검사에서는
GET용 CSRF 헤더를 빠뜨려401이었고 하네스만 수정했다. 제품 인증을 완화하지 않았다.
실제 Electron/레이아웃 픽셀/현장 Linux 수용과 구별한다.
로그: `/private/tmp/floe-electron-service-{unit,clippy,lifecycle}.log`.
기설치 target의 `cargo check --offline --locked --target x86_64-unknown-linux-musl`도
**exit0**이다(`floe-electron-service-linux-check.log`). 이는 sidecar의 Linux 타입/
컴파일 검사이며, 링크된 실행 파일·RHEL 런타임·Chromium/glibc 호환성 검증은 아니다.

## E1 이후 고정할 비교 조건

- `electron/runtime.json`은 공식 릴리스 **44.4.3 (2026-09-18)** 및 공식 SHA-256을
  기록한다. macOS arm64/x64·Linux x64 아카이브의 다운로드/검증/실행은 별도 단계다.
  실행 파일뿐 아니라 런타임 전체 의존성·고지/라이선스·업데이트 책임을 확인한다.
  [릴리스](https://github.com/electron/electron/releases/tag/v44.4.3),
  [공식 해시](https://github.com/electron/electron/releases/download/v44.4.3/SHASUMS256.txt).
- renderer Node integration off, context isolation/sandbox on, preload/native IPC 없음,
  소유 origin 외 요청/탐색·새 창·임의 권한 거부가 기본이다. 인증은 기존 웹 경로를
  사용한다. 복구는 기존 세션으로 명시 root GET만 하며 bootstrap/write를 재실행하지 않는다.
- 창·서비스 수명과 파일 대화상자/다운로드/클립보드 기능별 수용을 따로 표시한다.
  최소 shell 단계의 미구현 기능을 조용히 통과시키거나 완성품으로 배포하지 않는다.
- Rust decode/raster jobs·창 크기/DPR·동일 합성 source/view·refinement/margin 등의
  설정을 맞춰 비교한다. Chromium 프로세스 메모리와 Rust worker 메모리를 분리하고,
  서버 실행에서는 ETX 전송 비용이 여전히 남는다는 점을 포함한다.

공식 [플랫폼 지원](https://github.com/electron/electron#platform-support)은 RHEL/ETX의
개별 보장이 아니다. [VS Code의 RHEL8/glibc2.28 요구](https://code.visualstudio.com/docs/supporting/requirements)도
다른 Electron 번들 전체의 호환성을 증명하지 않는다. 오래된 Electron 고정 또는
보안 완화를 호환성 해법으로 채택하지 않는다.
