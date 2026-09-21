# Electron 독립 앱 비교

2026-09-21. 상위 [웹 전환](WEBUI_PLAN.ko.md), [데스크톱](WEBUI_DESKTOP.ko.md).
사용자 승인: **Electron 최소 호스트 비교 진행**. 기존 macOS WKWebView 앱 유지.
정식 호스트 채택·배포는 RHEL 8.6/8.10 + ETX/X11의 호환성/메모리/입력 지연을
확인한 뒤 결정한다. Chromium이 느리거나 빠르다고 미리 단정하지 않는다.

## 범위와 단계

| 단계 | 산출물 | 완료 근거/잔여 |
|---|---|---|
| E0 | 기존 Rust Session의 전용 파이프 sidecar, JS 클라이언트, 수명/인증 회귀 | 구현·실제 Node↔Rust 합성 검사; 아래 계약 |
| E1 | sandboxed Electron 독립 창, 같은 웹 번들, 시작/종료/실패 처리, 런타임 고정/검증 | macOS arm64 실제 Chromium 합성 창 검사 통과. 기능·성능 수용은 E2/E3 |
| E2 | 합성 레이아웃 입력/표시, 시작/RSS/CPU/input→표시 비교 도구, native 메뉴/입출력/복구 수용 | E2a pan·메모리, E2b 파일 내보내기 합성 검사 통과. WK/현장 대조·물리 입력·실제 Save 창·복구 수용은 남음 |
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

E0 검증 명령(개발용 Node만 필요, 이 명령에는 Electron 창 검사가 없음):

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

## E1: 실제 독립 창

`electron/main.cjs`는 같은 Rust 웹 번들을 sandboxed Chromium 창에 연다. Python과
외부 Chrome/Firefox는 실행에 필요 없다. 서비스·색인·raster는 기존 Rust 그대로다.
Node integration/preload/native IPC 없음, context isolation/sandbox on, 비영속 partition,
권한 요청/일반 새 창/webview 거부, 소유 origin 밖 HTTP/WS·탐색 거부. E2b의 정확한
POST 내보내기만 숨김 임시 창을 허용한다(아래 계약). 시작/실패 페이지의
고정 data URL만 별도 허용하며 임의 data URL 접두사를 허용하지 않는다. spellcheck는
끄고 기존 웹 CSP를 유지한다. 임시 profile은 새0700폴더이며 종료 정리는 best-effort다.
강제 crash 뒤의 안전 삭제/모든 디스크 흔적 제거를 보장하지 않는다.

메뉴의 Open layout/DRC/About는 기존 웹 동작을 호출한다. 창 닫기/Cmd-Q는 기존
기본취소 End session 확인을 열고, 응답이 없을 때도 기본취소 native 확인을 거쳐야
Rust EOF 취소를 보낸다. macOS 숨김/최소화 창은 먼저 복원한다. Rust가 정리/join한
뒤 앱이 종료된다. Recover View는 명시 확인 후 기존 origin root GET만 수행한다.
bootstrap·저장·index를 재전송하지 않는다. renderer 실패는 고정 오류 화면으로
알리고 자동 복구하지 않는다. 실제 renderer crash/복구 수용은 E2에 남긴다.

### 개발 실행

현재는 저장소 기반 비교판이며 `.app`/RPM/portable 완성 패키지가 아니다. 메뉴 코드도
`desktop/ui/menu-action.js`를 재사용하므로 `electron` 디렉터리만 떼어 배포하지 않는다.
공식 런타임 archive를 `runtime.json`의 SHA-256으로 검증하고 전체 내용(Helper,
resources, LICENSE/고지 포함)을 유지한다. 개발 스크립트는 다운로드/설치하지 않으며
실행 시 Electron 버전44.4.3과 sandbox 설정을 재검사한다(버전 검사는 해시 검증 대체 아님).

```sh
# feature/webui 작업 트리 루트에서
(cd rust && cargo build --release --offline --locked -p floe-index -p floe-renderd)
(cd electron/service && cargo build --release --offline --locked)

# macOS: 검증·압축해제한 공식 런타임
export FLOE_ELECTRON_BIN="/absolute/path/Electron.app/Contents/MacOS/Electron"
# Linux 후보: 같은 변수에 검증·압축해제한 런타임의 /absolute/path/electron 지정
sh tools/run_electron_dev.sh view "/absolute/path/design.oas"
# 소스 생략 시 native 폴더 선택; 임의 cwd/home 자동 허용 없음
sh tools/run_electron_dev.sh
```

개발 런처는 호출한 cwd와 공백/한글 인자를 유지한다. Rust CLI의 view 옵션을 그대로
넘기며 색인은 자동 생성하지 않는다. 서비스 바이너리는 기본 release 경로 또는 명시
`FLOE_ELECTRON_SERVICE_BIN`; 명시한 빈/잘못된 override는 fallback 없이 오류다.
`FLOE_INDEX_BIN`/`FLOE_RENDERD_BIN`도 기존 override를 보존한다.
E2b부터 같은 Cargo build가 `floe-electron-download`도 만든다. 기본은 선택한 service
바이너리와 같은 폴더이며 `FLOE_ELECTRON_DOWNLOAD_BIN`으로 명시할 수 있다.
두 바이너리를 함께 재빌드해야 한다. npm 패키지·추가 vendor는 필요 없다.

### 기능 한계와 검증

- E1의 다운로드 차단은 E2b에서 새 파일 내보내기로 확장했다. 기존 파일 덮어쓰기,
  자동 재시도/재개, 임의 원격 URL 다운로드는 계속 금지다. 아래 수용 범위를 따른다.
- 프로그램식 clipboard 권한은 차단한다. 표준 native Edit의 Copy/Paste 메뉴는
  있지만 이미지·좌표 복사, 붙여넣기/IME의 실제 수용은 별도다.
- `node --test electron/service-client.test.cjs electron/host.test.cjs`: **10/10**.
  실제 파이프5개까지 포함하면15개. 종료 timeout·중복 요청·stale 확인 취소,
  런처의 cwd·공백/한글 인자·명시 executable 오류를 포함한다.
- 공식44.4.3 darwin-arm64 archive SHA-256 검증 후 실제 Electron 실행 **exit0**.
  새 빈 root로 Rust 일회용 인증·목록 읽기, 보이는 문서·hash 제거·renderer Node 부재,
  실제 다른 loopback sentinel 탐색 차단(요청0)과 새 창 차단, 기본취소 focus→취소→
  정상 Quit 확인→서비스 종료를 검사했다. 인증값은 출력하지 않는다.
- `--smoke-test`는 추가 인자를 받지 않고 새 빈 root만 만든다. 합성 캡처는 새 임시
  폴더의0600 `window.png`이며, 문서 가시성을 먼저 검사하고 `stayHidden:true`로
  캡처한다. 캡처를 숨은 창의 가시성 통과 수단으로 쓰지 않는다. layout 픽셀·물리
  키 입력·input→photon 검증은 아니다.
- 반복 명령: `FLOE_ELECTRON_BIN="..." sh tools/validate_electron.sh`.
  이 gate는 Electron E0/E1/E2b(blob) 전용이다. POST clip은 layout driver에서 검사한다.
  전체 `validate_rust.sh` 통과를 뜻하지 않는다.
  최근 전체 Rust/web gate는 기존 GTK startup oracle30초 timeout으로 실패했고,
  selected gates/native host 통과와 분리해 기록한다.

현장 RHEL에서 실행하지 않았다. Linux sidecar `cargo check`와 아래 공식 런타임의
GLIBC 정적 점검만 통과했으며 ETX·전체 시스템 라이브러리·Chromium sandbox·서버
메모리 비용을 검증한 것은 아니다.

## E2 이후 고정할 비교 조건

### E2a: 합성 레이아웃·pan·계측 프로브

`tools/validate_electron_layout.cjs`는 인자를 받지 않고 새 임시 폴더에 기존 valmini
generator로 합성 OASIS를 만든 뒤 Rust index4 jobs로 인덱싱한다. 개발 fixture 생성에만
Python/KLayout을 사용하며 실행하는 Electron·Rust 제품 경로에는 Python이 없다.
실행 전후 합성 source/cache 파일 SHA-256 inventory가 같아야 통과한다.

```sh
export FLOE_ELECTRON_BIN="/absolute/path/Electron.app/Contents/MacOS/Electron"
export FLOE_QA_PYTHON_BIN="/absolute/path/venv/bin/python" # KLayout 있는 개발 oracle
# debug service 비교라면 명시; 생략은 release 경로
export FLOE_ELECTRON_SERVICE_BIN="$PWD/electron/service/target/debug/floe-electron-service"
node tools/validate_electron_layout.cjs
```

내부 `--smoke-layout-test ABS_SOURCE`는 기존 색인을 읽고 view 조작만 한다. 검증용
합성 소스에만 사용한다. DRC/default 저장·index·출력 경로 인자를 허용하지 않으며
화면 캡처/metrics는 별도 새0700폴더에0600으로 남긴다. 일반 실행에는 QA가 없다.
공통 메뉴와 QA protocol 수학을 기존 `desktop/ui`, `rust/web/ui`에서 읽으므로 현재
개발 비교판은 저장소 전체 레이아웃을 필요로 한다.

실제 macOS arm6444.4.3에서 **exit0**: goto200,200,300µm/full/high, decode4/raster4,
refinement off, DPR2,1640×1317 device px. 활성 Chromium 창에 Shift+Right/Left 및
Right/Left를 주입하고, 실제 가시 margin 착지·작업 완료·두 RAF를 기다린다. 10%/50%
각 pan에서는 캡처 픽셀이 변하고 원위치에서는 **BGRA bytes가 초기와 완전히 동일**.
소스/cache 불변, 기존 인증/탐색 차단/종료 gate도 통과했다.

2026-09-21 E2b 추가 전 단일 실행의 참고값(debug Rust service + release index/renderd):

| 관측 | 값 | 해석 한계 |
|---|---:|---|
| 10% 이동 / 복귀 | 42.7 / 34.6ms | 입력 주입→margin까지 안정된 DOM+2RAF,10ms polling 포함 |
| 50% 이동 / 복귀 | 99.7 / 38.1ms | 최초 반응 시간·input→photon·p95가 아님 |
| Chromium4 프로세스 working-set 합 | 약741MiB | 공유 페이지 중복 가능; 고유/피크 메모리 아님 |
| Rust service+renderd2 프로세스 RSS 합 | 약448MiB | 같은 시점의 참고값; 위 값과 합쳐 고유 메모리라 하지 않음 |

캡처/readback도 작업 사이 부하를 더한다. 메모리는 Chromium의 `getAppMetrics()`와
숫자만 읽는 `ps`의 **소유 Rust service 자손**을 구분한다. E2b부터 별도 download helper도
Rust RSS 합에 포함한다(이전 2개→3개). renderer Tab의 OS sandbox는
실측 true, Browser main의 false는 정상 호스트 권한 경계다. 모든 프로세스가 sandboxed라고
주장하지 않는다. [Electron 프로세스 지표](https://www.electronjs.org/docs/latest/api/structures/process-metric),
[메모리 단위](https://www.electronjs.org/docs/latest/api/structures/memory-info).

초기 실패도 보존한다. 첫 하네스는16px pan 스냅을 무시한 기대 좌표로 timeout이었다.
다음에는 foreground를 초기 캡처로 잡고 margin 복귀와 비교해848,168픽셀 차이가 났다.
최종 gate는 **실제 Live margin crop 착지끼리** 비교한다. 제품 cut/raster/표시 규칙은
바꾸지 않았다. 이는 foreground→margin 전환의 픽셀 불변을 증명하지 않으며 그 대조는
E2/G1에 별도로 남긴다. 허용 오차를 키워 통과시킨 것이 아니다.

최종 로그: `/private/tmp/floe-electron-layout-final.log`; 합성 캡처/metrics:
`/var/folders/1v/1wct59qn2dbc457m8msmb5c80000gn/T/floe-electron-layout-cDb1lU/`.
E2a 시점 Node unit12개 + 실제 pipe5개 =17개, Rust unit5·clippy·실제 빈 창 회귀는
`sh tools/validate_electron.sh`로 재실행한다. WKWebView와 같은 조건의 대조,
cold startup·반복 분포/peak·물리 입력·RHEL/ETX 수용은 아직 없다.

### E2b: Rust 소유 staging과 native 파일 내보내기

기존 웹의 PNG/설정 blob과 정확 clip·DRC review artifact POST 경로를 받는다. 정책은
`getInitiatorOrigin()`의 소유 origin, 소유 WebContents, 허용 URL/MIME, redirect 없음으로
검증한다. POST는 기존 웹의 `noopener` form을 숨김·동일 partition 창에서 **그대로**
보낸다. 원래 POST/200/허용 MIME의 첫 응답만 받으며 GET·오류 응답·redirect·추가
탐색을 차단하고 임시 창을 닫는다. 호스트가 cookie/CSRF/body를 읽어 재전송하지 않는다.
DRC notes/waives의 실제 Electron 내보내기 수용은 아직 별도다(경로 단위 검사만 있음).

흐름: Chromium 수신 → native Save 창 → Rust 목적지 측 복사·원자 게시.

- `setSavePath`가 동기 `will-download` 안에서만 가능하므로 Rust helper가 생성한
  새0700 staging 한 개를 미리 준비한다. **수신 완료 후** native Save 창을 띄운다.
  대화상자의 이름은 경로/제어문자를 제거한 제안일 뿐이다.
  종료 때 정리되는 앱 profile 내부/별칭 경로는 저장 대상으로 거부한다.
- 사용자 선택 뒤 목적지 폴더에 새 staging을 만들고64KiB씩 복사한다. 파일/디렉터리
  핸들·inode·regular/nlink·크기/시간 정보를 확인하고 기존 WK 호스트와 같은
  `linkat` no-clobber +0600+fsync를 쓴다. 기존 파일/링크를 덮어쓰지 않는다.
- 활성 수신 한 개, 파일512MiB, 수신300초 제한. 크기0(미상)는 수신 중 검사하며
  polling 전에 일시 초과할 수 있다. 복사 중 두 벌이 있어 최대 약1GiB 디스크가
  필요할 수 있다. 복사 시간은 파일 크기·저장장치에 비례하며 메모리는 파일 크기와
  함께 늘지 않는다. 느린 fsync/read/write syscall 자체의 종료시간은 보장하지 않는다.
- 중단/초과는 cancel, 자동 pause/resume·재요청 없음. 창 폐쇄만으로 다운로드가
  멈춘다고 가정하지 않는다. 실제 `DownloadItem.cancel()` 뒤 terminal `done`을
  기다리고 Rust EOF 정리를 한다.30초 내 producer 종료를 확인하지 못하면 staging을
  명시적으로 보존하고 cleanup 실패로 보고한다. 늦은 Save 선택은 게시하지 않는다.
- 게시 성공/실패와 cleanup 결과는 독립적이다. 모호한 결과는 목적지 확인 안내만
  하고 재시도하지 않는다. 알려진 payload와 빈 디렉터리만 지우며, 예상 밖 내용은
  보존한다. cleanup 미확인은 profile 보존·오류 종료다. 강제 crash의 정리/보안 삭제,
  모든 로컬 동일 사용자 경로 경쟁에 대한 방어를 보장한다고 주장하지 않는다.
- helper stdin/stdout은 전용 FIFO/Unix pipe다. 원자 게시·복사는 Rust에 남고
  renderer Node/preload/native IPC 권한은 추가하지 않는다. 파이프 read-ahead가
  `poll`에 다음 cancel을 숨기지 않게 unbuffered fd로 읽는다. E0 조기 취소도
  typed `Cancelled`만143으로 매핑하며 실제 시작 오류를 취소로 위장하지 않는다.

API 근거: [Electron DownloadItem](https://www.electronjs.org/docs/latest/api/download-item),
[창/POST 처리](https://www.electronjs.org/docs/latest/api/web-contents).

검증:

- Rust helper14+service5 단위·clippy, Node34개(가짜 controller + 실제 pipe를 구별).
  복사 중 취소/목적지 경합, symlink·기존 파일 불변, sparse512MiB 초과, 예상 밖 staging
  내용/producer 미확인 시 보존, 늦은 chooser 결과, bounded 진단 이력을 검사한다.
- 실제 Chromium blob: 새 합성 JSON 취소→0600 저장→동명 거부, 활성16MiB blob 수신 중
  종료·terminal 확인·cleanup. `--smoke-download-test`는 경로 인자를 받지 않는다.
- layout driver는 새 valmini의 pan 검사 뒤 별도 세션에서 정확 clip을 준비·승인하고
  원래 인증 POST 한 번의 다운로드를 확인한다. GET popup과 잘못된 CSRF POST는
  거부되며 원래 레이아웃/인증은 유지된다. clip 검사만 view budget256MiB로 설정해
  기존 managed clip 어드미션을 만족시킨다. source/cache SHA-256은 전후 불변이다.
- 위 Save 선택/취소는 **QA에서 destination callback을 주입**한 것이다. 실제 OS
  Save 창 클릭·IME·ETX 파일 선택 수용을 뜻하지 않는다. 입력 파일 chooser와
  프로그램식 clipboard, actual renderer crash/복구는 이 단계 완료에 포함하지 않는다.

로그: `/private/tmp/floe-electron-export-final.log`,
`/private/tmp/floe-electron-download-layout.log`, `floe-electron-download-post-final.log`.
E2b pan 재검사는 픽셀 복귀를 통과했고
Rust3 프로세스 합 약417MiB를 관찰했다. 동시 개발 부하·단일 실행이므로 이전 표와
성능 우열을 판정하지 않는다.

중간 실패도 남긴다: 재빌드 직후 pipe3개가10/30초 startup 제한을 넘었다
(`floe-electron-download-gate-final.log`). 같은 바이너리를 재빌드/제한 변경 없이
재검사하면11/11이 총0.43초에 끝났다(`floe-electron-download-warm-repeat.log`).
macOS 새 바이너리 검사/동시 빌드 부하와 분리하지 못했으므로 원인을 확정하지 않으며,
첫 기동 지연 수용은 여전히 남는다. 최종34개와 실제 blob gate는 제한 완화 없이 통과했다.

기존 WKWebView의 실제 저장/취소/cleanup 실패·DRC/인증 복구·renderd 종료/재열기
회귀(`sh tools/validate_desktop.sh`)는 exit0이다
(`/private/tmp/floe-desktop-electron-download-regression.log`); 최종 공유 코드의 desktop
38개·clippy도 통과했다. 두 Electron Rust 바이너리의 Linux musl `cargo check`도
통과했으나 Linux 링크/실행 수용은 아니다.

필수 전체 `sh tools/validate_rust.sh`는 **exit1**: GTK startup oracle30초 timeout
(oracle-build9.875초는 성공). Electron 회귀와 별개인 기존 실패이며 timeout/오라클을
완화하지 않았다. 로그 `/private/tmp/floe-webui-electron-download-full.log`.
전체 green이라고 표기하지 않는다. 아직 남은 전체 goal은 G1/G4 폭넓은 실제 UI·동일
조건 성능 대조, 현장 RHEL/ETX/Python-free Linux 수용, 배포 closure/고지·서명 및
별도 승인으로 보류한 원격 단계 등이다. E2b 완료가 웹 전환 전체 완료는 아니다.

### 비교·배포 시 유지할 조건

- `electron/runtime.json`은 공식 릴리스 **44.4.3 (2026-09-18)** 및 공식 SHA-256을
  기록한다. macOS arm64는 검증·실행했고, Linux x64는 archive/GLIBC 정적 점검만
  했다. macOS x64 검증과 Linux 실제 실행은 남았다.
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

## E3 사전 점검: 공식 Linux archive (실행 아님)

2026-09-21 공식 `electron-v44.4.3-linux-x64.zip`을 새 임시 폴더에 다운로드했다.
SHA-256은 `fe880a7e37160cfd4e00193bc4c713ead7a778abfe74860a2d36d86fd0be48a8`로
기록된 공식 해시와 일치한다. 압축 해제된 모든 일반 파일을 ELF magic으로 분류하고
Apple LLVM17 `objdump --private-headers`의 **Version References**를 읽었다.
임의 문자열 검색·ELF 실행·`ldd`를 사용하지 않았다.

| 동봉 ELF64 x86-64 | 최대 GLIBC requirement |
|---|---:|
| electron | 2.25 |
| chrome-sandbox | 2.4 |
| chrome_crashpad_handler | 2.17 |
| libffmpeg.so | 2.17 |
| libvk_swiftshader.so | 2.17 |
| libvulkan.so.1 | 2.16 |

6개 모두2.28 ceiling 이하다. 따라서 **이 공식 archive의 직접 GLIBC 심볼 요구가
2.28을 넘는 문제는 발견되지 않았다**. 이 결과를 RHEL 실행/전체 의존성 충족으로
확대 해석하지 않는다. artifact: `/private/tmp/floe-electron-linux.G3vz1Z/unpacked`.

주 실행 파일은 여전히 GTK3/GLib/GObject/GIO, NSS/NSPR, ATK/AT-SPI, Cairo/Pango,
X11/XCB/Xrandr 등, GBM, xkbcommon, udev, ALSA, CUPS, DBus, expat을 동적 요구한다.
WebKitGTK는 요구하지 않지만 **OS GUI 라이브러리가 전혀 필요 없는 앱은 아니다**.
`$ORIGIN` RPATH도 사용하는 공식 Chromium bundle이며 기존 Rust-only portable의
RPATH 금지 검사를 완화해 통과시킨 것이 아니다. 두 패키징 계약은 아직 별개다.

현장에 남은 검증: RHEL8.6/8.10의 실제 SONAME/심볼·동적 plugin 전체,
namespace/sandbox 설정과 일반 사용자 실행, ETX/X11 입력·DPI·합성/전송 부하,
동시 사용자별 메모리, Rust service/index/renderd Linux 바이너리의 실행·정리.
OS 패키지 설치/교체, setuid 권한 설정, user namespace/security 설정 변경,
`--no-sandbox`/`--disable-web-security` 우회는 하지 않았다. Chromium의 GPU compositor
프로세스는 UI 표시용이며 floe geometry rasterizer를 GPU로 전환한 것이 아니다.

현재 판단: Electron 최소 호스트는 **비교할 수 있는 구현 후보**가 됐다. 기존 macOS
WKWebView 앱을 대체하거나 RHEL 정식 배포로 채택할 근거는 아직 부족하다.
