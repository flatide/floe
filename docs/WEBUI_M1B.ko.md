# 웹 전환 M1b — gateway/브라우저 구현 기록

2026-09-13, `feature/webui`. [상위 계획](WEBUI_PLAN.ko.md),
[서비스 설계](WEBUI_SERVICE_API.ko.md), [M1a 완료 범위](WEBUI_M1A.ko.md).

## 1. 현재 단계와 남은 일

**M1b-1은 인증된 loopback HTTP/WS transport 라이브러리 기반**이다.
`rust/web`의 `floe-web` crate를 추가했다. **M1b-2a에서 앱 코어의
managed read lease·admission과 latest-only view controller를 추가했고**(§5),
**M1b-2b에서 신뢰된 launcher가 등록한 view의 인증된 제어/프레임 스트림을
연결했다**(§6). M1b-2c1은 등록된 소스 범위와 관리형 색인 supervisor다(§7).
M1b-2c2에서 인증된 catalog·view 생성/재open·색인 작업 API를 연결했다(§8).
M1b-3에서 `floe2-web view`와 번들 HTML/Canvas 기본 뷰어를 연결했다(§9).
M1b-4a에서 일반 layout margin prefetch/착지/crop을 연결했다(§10).
M1/G1/G4 전체 완료는 아니며 drag·현장 Firefox/ETX 성능 검증은 남아 있다.

다음 단계:

1. M1b-4b: drag·추가 스타일 편집(fill/width/font)과 실제 UI 게이트, G1/읽기 G4.
   deck margin/labels/query는 capability=false를 유지한다.
2. 이후 DRC/query/export parity와 현장 Firefox/ETX 성능 검증.

기존 CLI·GTK/Python 제품, jobdeck 실측 브랜치·렌더링 정책은 변경하지 않았다.
임의 파일 경로나 renderd wire를 HTTP/WS로 직접 실행하는 통로도 없다.

## 2. 실제 제공하는 transport 계약

library `Gateway::new(bound_addr)`, `transport::serve(listener, gate, shutdown)`를
테스트 하네스와 Rust 실행 명령이 호출한다. loopback TCP만 허용한다.
listener의 실제 주소와 생성 시 주소가 달라도 거부한다. 한 Gateway 인스턴스는
한 번 실행/종료하는 용도이며 재시작할 때 새 인증 상태를 만든다.

| 경로 | 조건/결과 |
|---|---|
| `POST /api/v1/session/exchange` | 정확한 Origin, JSON `{bootstrap, protocol:1, bundle}`. 일회용 token → HttpOnly cookie + CSRF secret + opaque session ID |
| `GET /api/v1/capabilities` | cookie **및** X-Floe-CSRF. protocol/bundle, 등록된 view가 있으면 `render:true`; shares/uploads는 false |
| `GET /api/v1/view` | cookie **및** X-Floe-CSRF. 등록된 view의 현재 snapshot, 없으면 404 |
| `DELETE /api/v1/session` | 정확한 Origin + cookie/CSRF. grant 폐기·cookie 만료·열린 WS 종료 |
| `GET /api/v1/events` | 정확한 Origin + cookie + subprotocol `floe.v1`, `bundle.<bundle>`, `csrf.<secret>` |

view가 없는 transport 하네스는 `hello`와 `{"type":"ping","seq":"1"}` → `pong`을 지원한다.
seq는 양의 u64를 표현하는 정규 10진 문자열이며 연결 내 엄격 증가한다.
중복/역전/미정의 필드·타입/클라이언트 binary는 연결을 종료한다. view가 등록된
연결의 generation·connection epoch·view ID·frame credit는 §6을 따른다.
bundle ID는 M1b-3에서 자산·wire 코드 내용에 종속된 40자리 ID로 교체했다.
잘못된 번들 교환은 426, 이전 ID의 asset URL은 404다(§9).

### 2.1 인증과 노출 경계

- bootstrap 120초/1회, owner session 8시간. OS entropy의 독립 256-bit nonce,
  고정 길이 비교는 `subtle`. secret은 Serialize하지 않으며 Debug는 redacted다.
  entropy 실패는 서비스 오류이고 grant를 반쯤 소비하지 않는다.
- Host는 실제 listen 주소의 **리터럴 IP:port**와 일치해야 한다. Origin은 null,
  다른 scheme/host/port, localhost alias, 중복 필드도 거부한다. Forwarded 헤더는
  신뢰하지 않는다. absolute-form URI와 query string도 현재 API에서 거부한다.
- cookie는 `Path=/api/v1; HttpOnly; SameSite=Strict`, 서버 포트별 이름이다.
  cookie는 포트 간에도 전송될 수 있으므로 인증에 cookie만 쓰지 않는다.
  GET에도 별도의 X-Floe-CSRF, WS에도 별도 csrf subprotocol이 필요하다.
  Origin이 없는 같은-origin GET은 CSRF 검증을 거친 뒤 허용한다.
- cookie의 Secure 속성은 **loopback HTTP**이므로 아직 붙이지 않는다. remote/TLS,
  프록시 허용 Origin, guest/share 범위는 M2의 별도 배포 정책이다. bind 주소만
  바꿔 외부에 열 수 없으며 동일 UID 악성 프로세스를 격리하는 sandbox가 아니다.
- 모든 응답은 no-store/no-referrer/nosniff/frame 금지/CSP. CORS는 열지 않는다.
  요청 원문·cookie·CSRF·bootstrap을 로깅하지 않는다. UI는 bootstrap fragment를
  교환 전에 주소에서 제거한다. launcher의 secret 전달/격리는 §9를 따른다.

### 2.2 자원/종료

HTTP 연결 32개, header 16KiB/64개, JSON body 16KiB, header/handler 5초,
HTTP 연결 전체 10초(keep-alive/unused-body drain 포함). WS는 별도 8연결,
control frame/누적 message 8KiB, read buffer 8KiB다. control-only write buffer
상한은 16KiB이며 이미지가 있는 연결의 전송 상한은 §6을 따른다.
입력은 고정 1초 창당 60메시지이며 ping/pong도 센다. 이 값들은 현재 작은
control 하네스 기준이고 전체 프레임 바이트/큐 상한을 대신하지 않는다.

WS 인증은 메시지마다/250ms tick에 재검사한다. idle 30초, send 1초,
close 100ms. 서버 종료는 신규 accept를 닫고 HTTP task를 수거한 뒤 WS permit
회수를 최대 2초 기다린다. 실패는 명시 오류다. 브라우저가 메시지를 보내야만
로그아웃/종료를 감지하는 구조가 아니다. native worker의 별도 poll/watchdog과
view 연결 해제 수명은 §5·§6을 따른다.

## 3. HTTP/WS 의존성 게이트

Axum **0.8.9**, Tokio **1.53.1**, Hyper **1.11.1**/hyper-util **0.1.20**,
tokio-tungstenite/tungstenite **0.29.0**, getrandom **0.3.4**, subtle **2.6.1**.
Serde/JSON은 기존 1.0.228/1.0.151을 유지했다. 정확한 전이 버전/registry checksum은
`rust/Cargo.lock`, 기능 집합은 `rust/web/Cargo.toml`을 정본으로 한다.

- Axum default off + http1/json/tokio/ws, Tokio default off + rt/net/sync/time/macros.
  `full`, HTTP/2, TLS, multipart/form/query, compression, C TLS 라이브러리는
  추가하지 않았다. 기존 동기 app-core를 Tokio handler에서 직접 오래 실행하지 않는다.
- RFC6455 handshake/fragmentation/masking/UTF-8는 직접 구현하지 않고
  [Axum WS](https://docs.rs/axum/0.8.9/axum/extract/ws/struct.WebSocketUpgrade.html)와
  Tungstenite를 사용한다. library default만 믿지 않고 크기/시간을 명시 제한한다.
  동기 작업과 transport 분리는 [Tokio bridging](https://tokio.rs/tokio/topics/bridging) 방침을 따른다.
- 공식 `cargo vendor --locked --versioned-dirs` 산출에서 새 **59개** 디렉터리만
  편입했다. 기존 **28개/1,699파일의 SHA-256**은 전후 동일하다. 기존 syn 2와 새
  syn 3은 별도 버전으로 공존하며 기존 vendor 파일을 수정/덮어쓰지 않았다.
  전체 registry 패키지는 87개다. WASI/Windows용 전이 소스 포함은 해당 제품 지원 선언이 아니다.
- 새 패키지는 MIT/Apache/BSD 계열이다. `matchit`은 MIT **AND** BSD-3-Clause,
  `subtle`은 BSD-3-Clause이며 각 upstream LICENSE/NOTICE를 그대로 동봉한다.
  r-efi의 LGPL 대안은 선택하지 않고 MIT/Apache 선택지를 사용한다. 최종 배포물의
  라이선스 수집은 portable 단계에서도 유지해야 한다.
- 선언 MSRV는 앱과 같은 Rust 1.89. 새 의존성 metadata의 상한은 target별
  wasip2 1.87, 실제 Mac/Linux에 쓰는 Axum은 1.80이다. manifest 숫자 확인뿐 아니라
  Rust 1.89 + vendor-only 컴파일/테스트도 별도로 수행한다.

### 3.1 보안 감사 (2026-09-13)

공식 cargo-audit 0.22.2, RustSec DB
`b50980aad8b8f14f77e25a97b32dd94bf008b0af`(2026-09-09), advisory 1,243개.
registry yanked 검사도 포함했다. HTTP/WS 후보 lock은 취약점/경고 0.
실제 workspace lock은 **취약점 0, 기존 ttf-parser 0.25.1의 유지보수 중단
경고 1**이다. 경고까지 0이라고 보고하지 않는다.

[RUSTSEC-2026-0192](https://rustsec.org/advisories/RUSTSEC-2026-0192.html)는
기존 fontdue 경로의 ttf-parser가 더 이상 유지되지 않음을 알린다. 현재
`render-core/font.rs`는 번들 NotoSansMono만 `include_bytes!`로 파싱하며 외부
font upload/path API는 없다. 이번 네트워크 편입에서 font parser를 교체해
라벨 픽셀 계약을 바꾸지 않는다. **후속: fontdue/대체 parser 유지보수 검토와
glyph/PNG parity 고정 후 교체 여부 결정**. 새로운 외부 font 입력은 허용하지 않는다.

갱신 시 `cargo audit --file rust/Cargo.lock`을 다시 실행한다. 이번 결과는
감사 시점의 알려진 advisory만을 뜻하며 미래 취약점 부재의 보장이 아니다.

## 4. 검증과 한계

`cargo test --offline -p floe-web`은 Python 없이 6개 단위 + 7개 실제 TCP/WS
통합 테스트를 실행한다. bootstrap 재사용/만료, cookie 단독·CSRF/Origin/Host,
프로토콜 skew, fragmentation 중 ping, 중복 seq, malformed JSON/UTF-8/마스킹,
개별/누적 과대 frame, body/header 제한, 32 TCP/8 WS 상한, slow header/body,
로그아웃 후 살아 있는 WS 폐기와 서버 종료를 검증한다. 테스트에는 loopback
socket 권한이 필요하며 외부 네트워크 연결은 필요 없다.

`cargo fmt -p floe-web -- --check`/strict clippy, 위 테스트와
`sh tools/validate_rust.sh` 전체 배터리 통과
(`RUST VALIDATION: ALL OK`, 기존 KLayout 13 PX + 2 phase-exact + 14 style 포함).
Rust 1.89.0 + 빈 registry `CARGO_HOME`에서 `--offline --locked`로 같은 13개
테스트를 통과했고, `x86_64-unknown-linux-musl --release --no-run` 테스트
실행 파일을 만들었다(`file`: ELF x86-64 static-pie). 이는 Linux 실행 검증이
아니라 vendored-only/MSRV/정적 링크 검증이다. macOS legacy oracle은 이미
생성·검증된 동일 valmini 캐시를 사용했다.

workspace 전체 `cargo fmt --all -- --check`는 기존 native 코드의 포맷 차이로
실패한다. 이번 변경 밖 소스를 일괄 재포맷하지 않았으며 새 패키지는 clean이다.

실제 Linux 실행, Firefox/ETX, 프레임 지연/메모리, 다중 사용자 admission은
아직 미측정이다. 네트워크 보안 감사의 범위와 기존 font 경고는 §3.1을 따른다.

## 5. M1b-2a — 관리형 캐시/자원과 view controller

`app-core/managed.rs`와 `app-core/view/`는 HTTP를 모르는 앱 서비스다. 신뢰된
로컬 source 등록 → `ManagedDataset::open` → 초기 `ViewState`의 정책/goto 일괄
검증 → `ViewController::start` 순서다. 첫 제출은 그 초기 상태이며 숨은 fit
render를 먼저 실행하지 않는다. open 대기 중 입력도 최신 상태에 누적한다.

- source별 캐시 키를 실제 부모 디렉터리 기준으로 정규화해 read/write lease를
  한 번에 획득하거나 전부 거부한다. 잡덱은 선택된 모든 TC 캐시를 pin하며,
  누락 TC의 키도 포함한다. 실제 open 후 source 집합을 다시 대조한다.
  재open은 새 dataset revision이며 외부 `.ovo` 자동 감지/hot reload는 없다.
- `Resources` 기본 예약은 CPU 16 slot, index가 빌려 쓸 수 없는 foreground 4,
  worker 2, decoded budget 합 2048 MiB다. render는 **decode+raster 합**으로
  보수적으로 예약한다. index 요청은 1..16 jobs를 검증하되 기본 reserve 때문에
  한 번에 최대 12가 승인된다. 16을 쓰려면 관리자가 reserve=0 등 명시 설정해야
  한다. 초과는 Busy이고 조용히 jobs를 줄이지 않는다. 대기 queue는 아직 없다.
- 이 제한과 lease는 **동일 Resources/gateway의 관리형 작업에만** 유효하다.
  외부 GTK/CLI/다른 gateway의 재색인을 차단하지 않는다. 원래 `PreparedIndex`
  exclusive OS lock은 유지한다. 실제 index endpoint는 후속 단계에서 lease를
  native child reap까지 소유하도록 연결한다. 단순 permit API가 이미 index
  endpoint나 server-wide supervisor를 완성했다는 뜻이 아니다.
- decoded 합은 RSS hard cap이 아니다. generation/retained/mask/mmap 및
  전송 복사, native 제어/heartbeat 스레드는 별도다. 실행 중 index를 선점하거나
  다중 사용자 응답 시간을 보장하지 않는다. 예약값은 현장 측정 전 로컬 기본치다.

view별 제어 스레드는 native worker 하나를 소유하며 브라우저 구독/ack와
무관하게 20ms poll로 응답 파일·deadline을 처리한다. foreground active 최대 1,
pending은 최신 상태 1개다. 취소는 한 번만 보내고 `pending_generations()==0`
(terminal 응답/파일 소비)까지 다음 작업을 제출하지 않는다. stdin의 cancel ack는
작업 종료와 다르다. 취소 후에도 별도 5s drain deadline을 유지하고 초과 시
명시 실패/worker close·reap. startup/close는 공유 종료 flag로 중단 가능하다.

`state_rev`(복원 상태), `render_rev`(뷰포트 또는 유효 픽셀 정책), `render_key`
(pan과 무관한 정책), `worker_epoch`와 native generation/round는 별개다.
예를 들어 layout auto→cull은 state_rev만, pan은 render_rev만, keep/style 변경은
key도 바꾼다. stale base revision은 상태를 바꾸지 않고 conflict. 상대 pan 100개는
100개 모두 적용하며 렌더만 합친다. 최신 frame Arc 하나를 보관하고 정책/뷰포트
변경 즉시 무효화한다. 늦은 frame과 실제 IO 오류는 각각 discard/명시 오류이며
서로를 cancelled 또는 성공으로 바꾸지 않는다.

서버 좌표 계산은 기존 f64 DBU 경계를 유지한다. 전달 DTO는 §6의 10진 문자열을
쓴다. pan은 선택 시 16 device px 위상 스냅, zoom anchor는 화면 좌상단
기준, resize는 중심/배율 보존이다. 1..8192 px/축, 합 16 Mpx, 유한·양의 bbox,
native 스타일 폭 1..8·색/레이어/덱 라벨 capability를 상태 반영 전에 검증한다.
덱 head 선택은 자식 pair로 확장하고, head 스타일은 자식에 전파하되 같은 요청의
명시 child 스타일이 우선한다. 일반 layout/deck의 cut/occupancy 규칙은 그대로다.

검증: 단위 테스트의 100회 입력, startup 단일 transaction, cancel ack/terminal
순서, 취소 drain timeout, 무구독 진행/이전 Arc 회수, revision/key, 오류·종료·
자원 반납과 alias/atomic lease를 고정했다. `validate_view_controller.py`는
Unicode/공백 source의 private valmini를 색인하고 **PATH가 빈 환경**에서 Rust
controller의 13개 실제 PNG를 기존 동기 RenderSession과 바이트 대조한다.
cache 바이트/mtime 불변, worker 임시파일 0, lease 충돌/재open revision과
startup 실패 후 자원 회수도 필수 단언이다. 전체 배터리에 연결했다.

잡덱 dataset 게이트도 level/chip/layer × 전체/선택 로드 6개를 managed
controller로 다시 렌더해 Python 오라클 PNG와 대조한다. 첫 state_rev=1과
캐시 lease/단위 변환/초기 스타일을 포함한 경로다.

검증 완료: app-core 단위 **29개**, worker-client 전체, 위 **13+6개 PNG 대조**,
변경 패키지 fmt/strict clippy(`--no-deps`), 전체 `sh tools/validate_rust.sh`
(`RUST VALIDATION: ALL OK`, jobdeck 80·renderer 46·기존 KLayout 오라클 포함).
Rust 1.89.0/빈 registry `CARGO_HOME`/`--offline --locked` 단위 29개와 Linux
musl release 테스트 실행 파일의 정적 링크도 확인했다. Linux에서 실제 실행한
결과는 아니며, 기존 workspace 전역 fmt/clippy 차이는 §4와 동일하게 보존한다.
이 단계에는 웹 프레임 전송/GUI/ETX 측정이 없으며 G1/G4 또는 M1 전체 완료가 아니다.

## 6. M1b-2b — 인증된 제어/이미지 스트림

`Gateway::with_view(addr, controller, title)`에 신뢰된 로컬 호출자가 연 view를
등록한다. owner grant 하나/view 하나의 slice이며 HTTP에 source 경로·native 명령을
받지 않는다. catalog·재open·index endpoint는 다음 단계다. title은 256문자로
제한하고 오류는 허용된 code만 내보낸다. native 경로/원문 오류/통계 전체를 보내지 않는다.

- WS마다 독립 무작위 `connection_epoch`; `hello` 다음 authoritative `snapshot`.
  재접속은 새 epoch/seq와 현재 snapshot/최신 frame을 받고 끊긴 상대 입력을 재생하지 않는다.
  `view.set`은 seq/epoch/view_id/base_state_rev/body를 받는다. base 충돌은
  `stale_state` + 현재 snapshot, 잘못된 값은 `invalid_request`이고 상태 불변이다.
  알 수 없는 필드/JSON null/잘못된 타입은 거부한다. world DBU/µm 좌표와 모든 u64
  ID/revision은 10진 **문자열**, 픽셀 크기와 제한된 비율은 JSON number다.
- 하나의 binary message = `u32 LE header_bytes + UTF-8 JSON + native PNG/raw`.
  header에는 view/dataset/connection/worker/render identity, generation/round,
  bbox/해상도/row0=top/format/길이와 final/partial/deferred/skips/approximate를 싣는다.
  PNG/raw의 원본 바이트를 바꾸지 않는다. raw magic/stride·길이, PNG IHDR 크기와
  bbox/최대 해상도를 검증하며 쿼리는 아직 capability=false다.
- 이미지 credit는 구독자당 **1**이다. `frame.ack`(displayed/discarded)와 실제
  socket write 완료가 **모두** 있어야 해제한다. 추측한 조기 ACK로 복사/전송
  한도를 우회할 수 없다. 대기 중 최신 frame은 controller Arc 한 장으로 합친다.
  별도의 구독자별 이미지 backlog를 만들지 않는다. native poll/cancel은 구독/ACK와 독립이다.
- header 64KiB, native payload 80MiB, 이미지 write buffer는 전체 packet+16KiB.
  packet encoder 최대 2개, 전체 전송 예약 256MiB(원본·packet·socket 복사를
  보수적으로 3배 계산), 전부 try-admission이며 무제한 waiter가 없다.
  별도 reader와 writer를 두고 control queue 4개/응답 256KiB를 제한한다.
  한계 초과·5s write timeout·10s image ACK timeout은 **해당 연결**을 닫는다.
  이는 RSS hard cap 또는 느린 망에서 끊김이 없다는 보장이 아니다.
- server ping 10s, 수신 idle 30s. 모든 입력과 tick에서 auth를 재검증한다.
  logout/server shutdown은 native close를 요청하고 비동기로 최대 4s 수거를 기다린다.
  구독자가 없어도 backend는 drain한다. 한 번도 연결하지 않으면 120s,
  마지막 구독 해제 후에는 60s 동안 복원을 허용한 뒤 worker를 닫는다.
  현 slice는 closed view를 재open할 API가 없어 다시 실행해야 한다. ETX 현장 정책은 미확정이다.
- CPU reserve 계산도 보완했다. foreground 4 + index 12는 **어느 요청이 먼저 와도**
  승인된다. reserve는 index 단독 예약을 제한하며 이미 foreground가 쓰는 slot을
  이중 차감하지 않는다. 총 16/동시 index 1 제한은 그대로다. worker 수명 전체를
  예약하므로 유휴 worker도 slot을 점유한다(실제 순간 CPU 사용률 제한과 다름).

검증: app-core 30개, web 12개 단위 + 실제 HTTP/WS 7개. 새 필수 게이트
`validate_view_stream.py`는 private Unicode/공백 valmini를 색인한 뒤 **PATH가 빈
환경**에서 native PNG/raw를 실제 인증 WS로 보내 원본과 바이트 대조한다.
format별 100회 pan, ACK 보류 중 단일 credit/latest-only, 빠른 두 번째 수신자의
독립 진행, stale base/잘못된 depth, epoch 재사용/binary 입력 거부, 재접속,
10s ACK timeout, logout 후 worker·임시파일·전송 예약 회수를 검증한다.
캐시 byte/mtime 불변도 필수다. browser decode 완료 시 stale 필터·Canvas 픽셀·
숨긴 탭/ETX는 아직 이 테스트의 범위가 아니며 M1b-3/4에서 별도로 검증한다.

위 게이트, 변경 패키지 fmt/strict clippy(`--no-deps`) 및 전체 배터리
`RUST VALIDATION: ALL OK` 통과. 기존 KLayout 13 PX + 2 phase-exact + 14 style을
유지했다. Rust 1.89.0·빈 registry·`--offline --locked`의 core/web 테스트와
Linux musl release 테스트 실행 파일의 static-pie 링크도 확인했다(Linux 실행은 아님).

## 7. M1b-2c1 — 소스 등록 범위와 관리형 색인 supervisor

`app-core/registered.rs`는 로컬 launcher가 지정한 디렉터리(최대 32개)에 대해서만
`RegisteredSource`를 만든다. 파일 경로를 받는 browser API가 아니다. 레이아웃은
OASIS header만 읽고, 덱은 문법/level/TC 집합을 보존한다. 전체 TC 최대 65,536,
level 4,096이며 초과는 명시 오류다. 아직 directory 탐색/업로드는 없다.

- 선택하지 않은 TC도 DBU 조사에 쓰이므로 **모든** TC와 cache 목적지의 scope를
  검사한다. source/header의 원래 경로와 native의 lexical abspath를 모두 검사해
  `symlink/../file`의 의미 차이를 통한 탈출도 막는다. 없는 TC는 존재하는 가장
  가까운 부모까지 resolve한다. 루트 `/` 전체 허가는 거부한다.
- 작업 시작 전에 source size/정밀 mtime과 level/의존 집합·경로를 재검사한다.
  바뀌었으면 재등록 필요 오류이며 조용히 새 파일을 열거나 색인하지 않는다.
  이는 원본 전체 hash나 불변 디스크 revision이 아니다. 검사 후 외부 프로세스가
  source/symlink/cache를 동시 교체하는 경우는 여전히 미지원이고 OS sandbox를
  대신하지 않는다. 사용자가 미룬 OVO hot reload/보존 정책을 구현한 것도 아니다.

`managed_index.rs`는 전체 관련 cache write lease와 CPU permit을 먼저 받아
전용 control thread에서 prepare/native 검증/순차 TC 색인을 수행한다.
읽고 있는 view가 하나라도 겹치면 Busy, 동시 managed index 최대 1, jobs 1..16과
foreground reserve를 적용한다. 덱 선택 밖 캐시도 등록 의존 집합의 lease에
포함하는 보수적 초기 정책이다. 일반 CLI의 jobs/경로 호환은 그대로다.

- force/LOD/occupancy는 기존 `PreparedIndex`·`DeckIndexPlan`을 재사용한다.
  현재 캐시의 무변경 재사용과 occupancy-only의 기본 marker 불변을 유지한다.
  browser용 profile/snapshot 경로는 열지 않는다. profile은 기존 CLI로 실행한다.
- snapshot은 준비/실행/취소/성공/일부 누락/실패/취소 완료, 현재 source의 basename,
  전체·완료·재사용·누락·실패 수, elapsed와 **safe error kind**를 제공한다.
  덱의 missing/unsupported source가 있으면 Incomplete다. 원문 native 오류나
  절대경로를 웹 진행 상태로 보관하지 않는다. exact 성공은 native exit code 기준이며
  텔레메트리 한 줄을 완료 신호로 오인하지 않는다.
- managed 전용 stdout/stderr는 nonblocking pipe다. 전용 control thread가 각
  poll마다 pipe당 최대 64KiB를 drain하고 stderr에서 허용된 단계/정수만 추출한다.
  line 4KiB를 넘으면 그 줄은 버리고 카운터를 올린다. log backlog나 별도 reader
  thread는 없다. 구독자가 없거나 느려도 색인 진행/취소는 독립이다.
- SIGTERM→1초 grace→kill/reap까지 write lease를 유지한다. terminal snapshot은
  child와 permit을 수거한 **뒤**에 표시한다. 취소/실패가 native 색인의 부분 캐시를
  되돌리는 transaction/backup은 아니다. 다음 색인은 기존 freshness/force 정책을 따른다.

필수 `validate_managed_index.py`는 private Unicode/공백 valmini와 **빈 PATH**로
실제 build/reuse/occupancy 추가, 열린 Dataset과 index의 lease 충돌, 덱 level
선택과 missing source의 Incomplete를 검증한다. 제어용 가짜 native는 shell builtin만
사용해 큰 양방향 pipe 출력 후 SIGSTOP하며, 1초 kill fallback/실제 PID 소멸/자원 반납을
단언한다. native exit 7/버전 불일치/등록 소스 변경도 명시 실패다. 과거 CLI의
stdout/stderr 상속은 그대로이며 이 캡처 방식은 managed 호출에만 적용한다.

현재는 앱 코어 API와 테스트 하네스에 연결된 범위다. HTTP 작업 ID/idempotency,
로그아웃/서버 종료 연동, catalog/open DTO와 유계 operation queue는 M1b-2c2다.

검증 완료: app-core 단위 **32개**, 실제 managed index 필수 게이트, 변경 패키지
fmt/strict clippy와 전체 `sh tools/validate_rust.sh`의 `RUST VALIDATION: ALL OK`.
기존 KLayout 13 PX + 2 phase-exact + 14 style 포함. Rust 1.89.0/빈 registry의
`--offline --locked` 테스트와 Linux musl release 테스트 실행 파일도 빌드했다.
폐쇄망 Linux/동시 viewer 부하 측정이나 HTTP index API 검증을 대신하지 않는다.

## 8. M1b-2c2 — owner catalog/open/index API

`Service::start`에 로컬에서 허가/등록한 source 최대 32개와 고정 native 설정을
넣고 `Gateway::with_service`로 연결한다. browser에는 독립 무작위 source ID만
보낸다. 임의 경로/실행파일/env/upload/profile 옵션은 없다. 아직 owner 한 명의
현재 view **1개**, 동시 오래 걸리는 open/index 작업 **1개**다. M2의 따라보기/
독립 탐색/공유 권한과 공용 supervisor를 완성한 것은 아니다.

다음 API는 모두 cookie + X-Floe-CSRF, 변경 요청은 정확한 Origin까지 요구한다:

| 경로 | 동작 |
|---|---|
| `GET /api/v1/catalog` | source ID·basename title·deck 여부·level 수. 파일 경로 없음 |
| `GET /api/v1/catalog/{id}/levels/{start}` | level ID(10진 문자열)/title, 64행 페이지 |
| `POST /api/v1/operations` | kind=open/index, 엄격한 DTO, owner 작업 seq → 202 |
| `GET /api/v1/operations` | 마지막 수락 seq·active와 최근 32개 작업의 최신 상태 |
| `GET /api/v1/operations/{seq}` | 자기 작업의 진행/결과, 축출된 이력은 410 |
| `POST /api/v1/operations/{seq}/cancel` | 해당 작업만 취소. 이미 끝난 작업에는 재실행/다른 작업 취소 없음 |
| `GET /api/v1/view` | 현재 source ID·mode와 authoritative view snapshot |
| `GET /api/v1/views/{id}/layers/{start}` | 가시 UI 행 64개, pair/name/style/가시성/덱 head-parent. 원본 tooltip/path 없음 |
| `DELETE /api/v1/views/{id}` | 명시 view close. 이전 ID의 재전송으로 새 view를 닫지 않음 |

`open` body에는 source_id/mode/levels와 §6의 초기 view patch를 받는다.
depth/detail/thin/goto/pixels/style을 한 번에 적용한 뒤 controller를 시작한다.
open 성공은 dataset/worker controller 생성이지 첫 프레임 완료가 아니다.
실제 open/render 오류는 snapshot의 실패 상태를 따른다. 캐시가 없으면 명시
index_unavailable이며 자동 색인/force는 없다. 다른 view를 열려면 이전 view를
닫고 worker 종료를 기다린다. mode/선택을 바꾼 재open에는 새 dataset/worker/view
ID를 부여한다. 현 단계의 재open은 초기 patch를 적용하며 이전 스타일/이력의
자동 이관을 구현했다고 주장하지 않는다.

`index`는 jobs/force/LOD/occupancy/occupancy-only/occupancy-um만 허용한다.
등록 의존 집합의 managed lease/CPU 예약은 §7과 같고 열린 view와 충돌하면
작업은 Busy로 실패한다. core JSON/원문 로그가 아니라 허용된 진행 카운터만
snapshot으로 전달한다. index를 묵시적으로 다시 시도하지 않는다.

작업 seq는 owner 서비스 수명의 양의 u64 10진 문자열, 새 작업은 마지막+1이다.
동일 seq·동일 typed payload의 재전송은 보관된 상태를 반환하며 native를 다시
실행하지 않는다. payload가 다르면 409. 최근 32개 밖의 오래된 seq도 high-water
때문에 **410이고 재실행하지 않는다**. busy로 아직 수락하지 않은 요청은 seq를
소비하지 않는다. WS 편집 seq/connection epoch와 별개이며 재접속 후 먼저 작업
상태를 조회해야 한다. 취소와 완료가 경합하면 최종 실제 결과를 확인한다.

filesystem/prepare/native wait는 HTTP thread가 아니라 전용 owner thread에서
실행한다. pending+active 합 1, 무제한 thread/spawn_blocking/operation queue가
없다. HTTP 진행 조회가 없어도 native를 drain한다. close/cancel은 긴 index
뒤에 줄 서지 않고 종료 flag를 보낸다. logout·auth/bootstrap 만료·서버 종료도
service/index/view를 중단한다. shutdown의 비동기 대기 실패는 오류이며 느린
외부 filesystem syscall 자체에 hard deadline/OS 격리를 보장하지 않는다.

WS upgrade는 그 시점의 view Arc와 write-buffer 한도를 함께 고정한다. open
전 control-only 연결이 나중에 큰 이미지 스트림으로 변하는 경합은 없다.
명시 종료/실패한 view의 WS는 마지막 snapshot을 보낼 짧은 유예 뒤 닫아 이전
model/목록을 계속 붙들지 않는다. 일반 연결 해제의 60s linger는 그대로다.

레이어 목록은 128문자 name/최대 4개 alias로 표시 메타데이터를 제한한다.
원래 pair/서버 렌더·선택 의미는 유지한다. level 모드의 숨긴 렌더 자식이 선택되어
있으면 표시된 부모도 켜진 것으로 보고, chip 모드는 level-parent/source-child다.
목록 페이지의 state_rev로 비동기 응답이 최신 상태인지 판정할 수 있다.

필수 `validate_owner_service.py`: private valmini, 빈 PATH, 실제 HTTP/WS/native로
무인증/미정의 옵션/경로 거부, 자동 색인 금지, index 중복 요청/다른 payload 충돌,
열린 cache의 재색인 거부, 초기 generation=1, layout→chip→level 재open과 새 revision,
가시 head/숨김 자식, 이전 close ID 거부, 살아 있는 WS 및 index의 logout/서버 종료
후 PID/lease/전송 예약 회수를 검증한다. 기존 PNG/raw 100-input 스트림 gate도 유지한다.
browser 자체의 입력·decode·Canvas 검증과 실행 명령은 다음 단계다.

검증 결과: web 단위 13 + 실제 TCP/WS 7, 위 실제 native owner gate와 PNG/raw
스트림 gate, strict clippy/해당 crate fmt, `sh tools/validate_rust.sh` 전체가
통과했다(`RUST VALIDATION: ALL OK`, 기존 KLayout oracle 포함). Rust 1.89.0의
빈 registry + `--offline --locked` 테스트와 Linux musl release 테스트 실행 파일
빌드도 통과했다. Linux/Firefox/ETX 실행 검증으로 대신 세지는 않는다.

## 9. M1b-3 — 기본 Canvas 뷰어와 Rust 실행 명령

`floe2-web view SOURCE [SOURCE ...]`는 loopback gateway와 기본 뷰어를 실행한다.
사용법은 [Rust CLI README](../rust/app/README.md), 전체 옵션은 `view --help`다.
일반/잡덱 source(최대 32개)는 로컬 CLI가 등록하고, 웹은 opaque ID만 사용한다.
source/level 선택, level/chip mode, 명시 index·진행·취소·close/reopen, goto/fit/
커서 pan·wheel/버튼 zoom, depth/detail/thin, label/frame/mono, layer checkbox/
색상, native perf·미완료 상태를 연결했다. close 뒤 mode/level을 바꿔 재open한다.
웹의 현재 explicit layer selection 상한은 4096이다. All-minus-one도 이 상한을
넘으면 명시 오류이며 큰 목록을 조용히 축약하지 않는다.

### 9.1 시작·번들·브라우저 수명

- CLI의 초기 goto/depth/detail/thin/level 등은 인증된 `GET /api/v1/startup`의
  한 open request다. UI가 실제 viewport device px를 추가하고 seq=1로 보낸다.
  고정 크기의 숨은 fit render를 먼저 실행하지 않는다. 재접속은 current view를
  복원하고 초기 상대 이동을 다시 적용하지 않는다. open은 자동 index를 하지 않는다.
- HTML/CSS/JS를 Rust 바이너리에 내장한다. build.rs의 source/관련 wire 내용 hash가
  bundle ID와 `/assets/<id>/...` URL을 결정한다. hash는 cache/skew 식별자이지
  보안 서명이 아니다. 이전/미정의 asset은 404, 번들 불일치 교환은 426이다.
  CSP에 inline/eval/CDN은 없고 connect-src는 실제 loopback HTTP/WS origin뿐이다.
- 기본 실행은 [Firefox의 전용 프로필/독립 인스턴스 옵션](https://firefox-source-docs.mozilla.org/browser/CommandLineParameters.html)을
  쓴다. 기존 프로필·보안 설정·인증서를 변경하지 않는다. bootstrap URL을 argv에
  넣지 않고 0700 전용 디렉터리의 0600 `launch.html`로 전달한다(프로세스 목록 노출 방지).
  session JSON도 0600/create_new이며 기존 파일/심볼릭 링크는 덮어쓰지 않는다.
  `--no-open`에서는 그 private JSON의 일회용 링크를 사용한다. 링크 공유는 금지다.
- 전용 process group만 TERM→KILL/reap 후 프로필을 제거한다. 자식의 종료 확인은
  waitid(WNOWAIT)로 PID를 보유해 재사용 race를 피한다. macOS의 zombie-only group
  EPERM은 libproc으로 같은 그룹의 살아 있는 프로세스가 없음을 확인한 경우만
  허용한다([XNU 구현](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_sig.c)).
  실제 cleanup 실패는 오류/프로필 보존이다. 기존 사용자 Firefox를 종료하지 않는다.
  Ctrl+C/SIGTERM, End session, 소유 브라우저 프로세스 종료가 native worker 수거로 이어진다.

### 9.2 표시·입력 경계

raw는 FLOERAW1 RGBA를 `putImageData`, PNG는 크기 검증 후 Canvas에 1:1로 blit한다.
canvas backing store는 device px, CSS 배치는 device-pixel 정렬이다. 서버가 좌표/
pan·zoom 계산을 담당하고, 브라우저는 그 좌표를 표시용 숫자로만 변환한다. resize는
새 프레임 전까지 이전 화면을 unscaled frozen base로 보관한다. margin crop은 아직 없다.

u64 seq/revision은 10진 문자열로 비교한다. header/payload/좌표/크기 검증 뒤에도
비동기 PNG decode 완료 시 dataset/view/connection/worker epoch·render revision/
key·bbox·pixel size를 다시 검사한다. 다음 상태가 accepted만 되고 snapshot이 아직
안 온 경우에도 이전 PNG를 폐기한다. hidden tab/늦은 callback은 credit를 반환하고
표시하지 않는다. Blob URL/timer/callback은 성공/오류/재접속 모두 정리한다.
입력은 bounded queue(64)·한 요청 in-flight·약 15회/s로 제한한다. 불확실한 상대
입력은 재접속 때 재생하지 않고 notice를 표시한다. 과대 좌표/치수도 명시 오류다.

### 9.3 검증·남은 범위

- app 5/app-core 34/web 13 단위 + 실제 HTTP/WS 8, crate fmt/strict clippy 통과.
  full `sh tools/validate_rust.sh`에서 기존 native/KLayout oracle와 모든 앱 게이트를
  포함해 `RUST VALIDATION: ALL OK`. 실제 native 통합의 ignored Rust 테스트는
  해당 Python driver로 필수 실행하며 단위 테스트 skip을 통과로 세지 않는다.
- `validate_web_cli.py`: PATH empty, 명시 색인 후 초기 gen=1/goto/치수, private
  Firefox argv/profile, logout/SIGINT, source/기존 session 보존, native child 수거.
  Firefox 프로세스 수명 테스트는 제어된 fake binary다. Firefox UI 검증이 아니다.
- `validate_web_ui.cjs`: production JS를 ES2017로 파싱하고 malformed packet/u64/
  늦은 PNG·hidden tab·epoch/credit/URL 정리를 테스트한다. **개발 검증에는 Node>=18**이
  필요하며 설치된 앱/`cargo build`에는 Node/npm/Python이 필요 없다. npm 설치 없이
  test-only Acorn 8.15.0(MIT)을 공식 tarball·integrity와 함께 고정했다
  (`tools/vendor/acorn-8.15.0/README.floe.md`). 이 VM 테스트는 browser pixel oracle가 아니다.
- 실제 macOS Chrome에서 합성 valmini 첫 화면(raw/PNG), zoom, layer toggle,
  재접속을 확인했다. raw/PNG 첫 화면은 gen=1, backing 2312×1651(DPR 2)이며
  native geometry가 표시되는 스크린샷을 작업 기록에 남겼다. GTK/native의 기존
  half-phase oracle를 Browser/Firefox G1이 완료된 것으로 대체하지 않는다.
- Rust 1.89 + 빈 registry `--offline --locked` 동일 단위/HTTP 테스트, Linux musl
  release link 성공. Linux 실행/현장 Firefox·ETX는 아직 미검증이다.

이 단계는 외부 Sites hosting·외부 로그인/CDN을 사용하지 않는다. 기존 폐쇄망
Rust 앱의 작업 화면을 우선 연결했으며 Python/GTK 제품·jobdeck 실측 브랜치는
유지한다. layout margin/라벨 crop, 추가 스타일(fill/width/font) 편집, drag, G1/G4
및 DRC/query/export/배포 전환은 다음 단계다. refinement 기본 off도 그대로다.

## 10. M1b-4a — 일반 레이아웃 margin/crop

`view`의 frame-cache는 기본 on이다. `--frame-cache off`는 native retained frame
재사용과 margin prefetch를 함께 끈다(decoded page LRU를 끄는 옵션은 아니다).
기존 `ViewController::start`/`Service::start`는 prefetch 없는 하네스 계약을 유지하고,
웹 launcher가 `start_configured(ControllerOptions)`로 명시 활성화한다. 잡덱은
요청이 on이어도 margin capability=false이며 추가 렌더를 하지 않는다.

### 10.1 생성·스케줄·수명

- 첫 foreground final 이후 idle 때 ±50% 커서 한 스텝만큼 확장한 프레임을
  같은 worker에 제출한다. 확장은 양축 16 device px 배수다. 8192 px/축·16 Mpx
  한도에 걸리면 같은 비율로 축소/16px 내림, 여유가 없거나 지원 좌표 경계를
  넘으면 prefetch를 생략한다. viewport 요청 자체를 확대하거나 숨은 fit을 만들지 않는다.
- margin은 라벨을 포함하고 foreground와 같은 depth/detail/thin/style/mono/frames
  정책을 쓴다. 라벨 잘림·partial/deferred가 있으면 crop 완료로 인정하지 않는다.
  실패한 optional prefetch는 기존 정상 foreground를 지우지 않고 `margin_failure`와
  상태줄에 표시한다. 같은 영역을 계속 재시도하지 않는다. worker 자체의 I/O/종료
  실패는 여전히 hard error이며 취소로 위장하지 않는다.
- foreground 요청은 진행 중 margin보다 우선한다. cancel ack가 아니라 terminal
  drain 후 다음 native 작업을 제출하며 기존 5s drain deadline을 유지한다. 이미
  완전한 margin이 새 viewport를 덮으면 foreground를 생략한다. 배율·정책이 맞는
  진행 중 margin은 가능한 완료시켜 반복 pan이 prefetch를 영원히 취소하지 않게 한다.
- 착지 프레임 양쪽 여유가 원래 extension의 70% 이상이면 그대로 사용한다.
  벗어나면 현 중심으로 보충한다. 상주 foreground 1 + margin 1, native active 1,
  pending은 계산된 최신 상태뿐이다. 전송은 두 슬롯 중 foreground 우선, 연결당
  encoding/write/ACK flight 1과 기존 byte/encoder admission을 그대로 적용한다.

### 10.2 표시·무효화

frame 봉투의 `purpose=foreground|margin`, snapshot의 `margin={frame_id,origin_px,
crop_safe}`로 용도를 구분한다. margin은 과거 render_rev여도 같은 dataset/view/
worker·render_key·배율과 16px 위상을 만족하면 유효하다. reconnect는 새 connection
epoch를 붙여 필요한 margin을 다시 전송한다. foreground에는 기존 최신 revision
검사를 그대로 적용한다. 늦은 margin decode도 설치 전에 다시 검사한다.

브라우저는 foreground Canvas와 margin Canvas 두 장만 유지한다. margin을
배경에 놓고 CSS 위치를 정수 device px로 이동시켜 viewport에서 clip하므로 pan마다
전체 RGBA를 복사하지 않는다. 완전한 margin 안에서는 foreground를 숨기고 라벨까지
같이 crop한다. 잘린 라벨이 있는 margin은 새 strip의 base로만 쓰고, 겹치는 기존
foreground를 위에 유지한 채 새 foreground를 받는다. 새 정책·zoom·resize는 현재
crop을 frozen viewport로 보관한 뒤 이전 margin을 재사용하지 않는다.

커서 입력의 임시 위치는 화면 px만으로 즉시 이동한다. 상태/세계 좌표는 Rust
응답이 정본이고, 불확실한 입력을 재접속 뒤 다시 실행하지 않는다. buffer→viewport
투영은 표시 전용 f64이며 재샘플하지 않는다. 양축 배율 1e-9, 정수 오차 1e-3 px
범위를 벗어나면 재사용하지 않는다. `displayed` ACK는 Canvas 설치/credit 완료를
뜻하며 input-to-photon 측정값은 아니다.
perf에는 마지막 foreground 비용과 background margin 비용을 분리해 표시한다.
crop hit를 새 foreground 렌더로 오인하거나 큰 background 치수를 viewport로
비교하지 않도록 `crop (no foreground render)`를 명시한다.

각 Canvas는 최대 RGBA 64 MiB, 두 장 합 128 MiB다. decode packet/임시 RGBA/브라우저
내부 surface와 native retained/decoded 메모리는 별도다. RSS 128 MiB 보장으로
해석하지 않는다. 캐시/배율/정책이 바뀌거나 close하면 불필요한 buffer를 폐기한다.

### 10.3 검증

- core: half-DBU/50%·10% 양방향 pan, 16px phase, 4K/16Mpx/좌표 끝, crop-hit,
  정책 변경, label truncation, prefetch 실패 반복 억제, cancel ack/terminal,
  deck 및 frame-cache off. app-core 단위 40개 통과.
- 실제 native `validate_view_controller.py`: 원본 foreground와 margin crop
  **18개 raw 픽셀 대조**(반 DBU 위상·speckle/16×16 pattern·양축 10/50%) 완전 일치.
  10% pan은 추가 foreground 제출 0. 기존 13 PNG/lease 게이트도 유지한다.
- 실제 HTTP/WS `validate_view_stream.py`: PNG/raw + 라벨, foreground 우선 credit,
  crop 뒤 foreground 없이 재접속, 과거 margin render_rev/새 epoch, thin 정책
  변경의 강제 무효화, 원래 2×100입력·slow subscriber·종료 게이트 유지.
- JS: 즉시 50% pan에 추가 draw 호출 0, 정수 위치·truncated base/foreground 겹침,
  느린 PNG decode 중 key 변경, URL/ACK 정리. CLI는 on/off 모두 PATH-empty 검증.
- 실제 macOS Chrome/DPR 2: 2312×1651 viewport에 4616×3315 raw margin이 착지하고
  라벨 on에서 Shift+Right 입력 후 같은 frame ID/generation 2를 유지하며 위치만
  224 device px 이동했다. 화면과 DOM 상태를 확인했다. Firefox/ETX G1/G2 성능
  ±10% 또는 모든 사용자 파일의 무결성을 이 실험만으로 주장하지 않는다.

`sh tools/validate_rust.sh` 전체가 `RUST VALIDATION: ALL OK`로 통과했다. 마지막
표시/버퍼 회수 수정 뒤 JS 게이트·release CLI·strict clippy를 다시 확인했다.
Rust 1.89/빈 registry/`--offline --locked` 단위·HTTP 테스트 및 최종 번들의 Linux
musl release link도 성공했다(Linux 실행 검증은 아님). 기존 native 경고는 보존했다.
query capability는 계속 false다.
M4의 expected/actual scene ID는 이제 foreground뿐 아니라 **실제 표시된 margin
generation**도 식별해야 한다. crop에서 render_rev만으로 native query scene을
추정해서는 안 된다.
