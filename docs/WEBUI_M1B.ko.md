# 웹 전환 M1b — gateway/브라우저 구현 기록

2026-09-13, `feature/webui`. [상위 계획](WEBUI_PLAN.ko.md),
[서비스 설계](WEBUI_SERVICE_API.ko.md), [M1a 완료 범위](WEBUI_M1A.ko.md).

## 1. 현재 단계와 남은 일

**M1b-1은 인증된 loopback HTTP/WS transport 라이브러리 기반**이다.
`rust/web`의 `floe-web` crate를 추가했다. 아직 웹 뷰어 실행 명령, 정적 UI,
렌더/색인 endpoint, 프레임 스트리밍, 세션 controller가 없다.
`floe2-web view`가 동작한다거나 M1/G1/G4가 완료됐다는 뜻이 아니다.

다음 단계:

1. M1b-2: managed catalog/read-write lease, 작업/worker admission, 상태/epoch,
   latest-only render controller·프레임 봉투·credit와 독립 watchdog.
2. M1b-3: 번들 HTML/Canvas와 실행 명령, open/goto/pan/zoom, layer/level/chip,
   depth/detail/thin·스타일·상태줄, DPR/y축/half-DBU/늦은 decode 필터.
3. M1b-4: layout margin prefetch/착지 base/crop/16px 위상, 실제 UI 게이트와
   G1/읽기 G4. deck margin/labels/query는 capability=false를 유지한다.

기존 CLI·GTK/Python 제품, jobdeck 실측 브랜치·렌더링 정책은 변경하지 않았다.
임의 파일 경로나 renderd wire를 HTTP/WS로 직접 실행하는 통로도 없다.

## 2. 실제 제공하는 transport 계약

현재는 library `Gateway::new(bound_addr)`, `transport::serve(listener, gate,
shutdown)`를 테스트 하네스가 호출한다. 랜덤 포트의 loopback TCP만 허용한다.
listener의 실제 주소와 생성 시 주소가 달라도 거부한다. 한 Gateway 인스턴스는
한 번 실행/종료하는 용도이며 재시작할 때 새 인증 상태를 만든다.

| 경로 | 조건/결과 |
|---|---|
| `POST /api/v1/session/exchange` | 정확한 Origin, JSON `{bootstrap, protocol:1, bundle}`. 일회용 token → HttpOnly cookie + CSRF secret + opaque session ID |
| `GET /api/v1/capabilities` | cookie **및** X-Floe-CSRF. protocol/bundle과 현재 `render:false, shares:false, uploads:false` 반환 |
| `DELETE /api/v1/session` | 정확한 Origin + cookie/CSRF. grant 폐기·cookie 만료·열린 WS 종료 |
| `GET /api/v1/events` | 정확한 Origin + cookie + subprotocol `floe.v1`, `bundle.<bundle>`, `csrf.<secret>` |

WS는 현재 `hello`와 `{"type":"ping","seq":"1"}` → `pong`만 지원한다.
seq는 양의 u64를 표현하는 정규 10진 문자열이며 연결 내 엄격 증가한다.
중복/역전/미정의 필드·타입/클라이언트 binary는 연결을 종료한다. 렌더 generation,
connection_epoch, view_id와 frame credit는 다음 controller 단계에서 추가한다.
지금의 bundle ID `m1b-transport-1`은 transport 하네스용이다. UI를 편입할 때
자산 내용에 종속된 bundle ID로 바꾸고 실제 asset skew 게이트를 추가해야 한다.

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
  요청 원문·cookie·CSRF·bootstrap을 로깅하지 않는다. URL fragment 수신·교환 후
  주소 제거는 아직 launcher/UI 미구현 범위이며 완료로 표시하지 않는다.

### 2.2 자원/종료

HTTP 연결 32개, header 16KiB/64개, JSON body 16KiB, header/handler 5초,
HTTP 연결 전체 10초(keep-alive/unused-body drain 포함). WS는 별도 8연결,
control frame/누적 message 8KiB, read buffer 8KiB, write buffer 상한 16KiB다.
입력은 고정 1초 창당 60메시지이며 ping/pong도 센다. 이 값들은 현재 작은
control 하네스 기준이고 전체 프레임 바이트/큐 상한을 대신하지 않는다.

WS 인증은 메시지마다/250ms tick에 재검사한다. idle 30초, send 1초,
close 100ms. 서버 종료는 신규 accept를 닫고 HTTP task를 수거한 뒤 WS permit
회수를 최대 2초 기다린다. 실패는 명시 오류다. 브라우저가 메시지를 보내야만
로그아웃/종료를 감지하는 구조가 아니다. 아직 native worker watchdog은 없다.

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
