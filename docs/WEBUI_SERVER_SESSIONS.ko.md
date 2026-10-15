# S1a/S1b — TeeBox 위임 세션·네이티브 실행·화면 전송

**2026-09-29 최신 결정: 서버 도입 보류.** 아래 코드는 보존하고
[TeeBox standalone](WEBUI_TEEBOX_STANDALONE.ko.md)을 우선한다.
후속 요청의 [홈페이지 공개 데모](WEBUI_WEBSITE_DEMO.ko.md)는 이 세션 렌더 경로를
별도 anonymous 정책/CLI로 재사용한다. 아래 TeeBox 위임 운영 배선 보류와 구분한다.

2026-09-29 갱신, `feature/webui`. [공용 서비스 계획](WEBUI_PUBLIC_SERVICE.ko.md)의 S1 중
**발급·교환·회수**, **세션별 native renderer**, **HTTP/WS 화면 전송·재접속·회수**와
**서버 전용 기본 뷰 UI**를 구현했다. 운영 CLI·TeeBox 실제 연결은 아직 아니다.
`server --check-config`는 계속 `runtime_ready: false`다.
기존 standalone `Gateway`·Electron 로컬 실행·bootstrap 인증은 바꾸지 않았다.

## 1. 인증 흐름

```text
TeeBox ─ 계정/비밀번호 → HTTPS 프록시
       ─ 위임 키 + 실제 사용자/원본 → POST /api/v1/server/launches
                         └─ 사용자/원본에 고정된 일회용 접속권
Electron/브라우저 ─ 접속권 → /sessions/{id}/exchange
                         └─ 세션 전용 HttpOnly cookie + CSRF
```

서비스 계정 인증은 **TeeBox 발급/회수 backchannel**에 적용한다. 브라우저/Electron에
TeeBox 비밀번호를 넣지 않는다. 향후 프록시 배포에서는 해당 backchannel의 Basic 인증과
브라우저의 bootstrap/session 인증을 구분해야 한다. UI가 있다는 이유로 API 인증을 생략하는
것도 아니고, 모든 브라우저에게 TeeBox 계정 로그인을 요구하는 것도 아니다.

발급/회수에 필요한 세 값은 서로 역할이 다르다.

- `X-Floe-Proxy-Key`: 고정 HTTPS 프록시 경유 증명. 모든 브로커 요청에 필요하다.
- `X-Floe-Service-Client`: 프록시가 검증한 계정 ID. 설정의 `client_id`와 정확히 일치한다.
- `X-Floe-Delegation-Key`: TeeBox가 보유한 별도 위임 비밀. 브라우저/쿠키로 대체할 수 없다.

proxy key와 delegation key는 256-bit 고정 길이 secret이며 동일한 값은 설정 오류다.
상수 시간 비교를 사용하고 중복 header를 거부한다. 계정/비밀번호의 `Authorization`과
충돌하지 않도록 위임 키는 별도 header다. 프록시는 외부의 proxy/client header를 제거하고
검증한 값으로 다시 설정해야 한다. **위임 키를 프록시에서 모든 사용자 요청에 자동
주입해서는 안 된다.** TeeBox의 인증된 요청에서만 전달한다. forwarded host/proto/user는
어떤 권한도 만들지 않는다. 키 파일·회전·실제 Basic 검증 설정은 아직 배선하지 않았다.

## 2. HTTP 표면과 권한

| 경로 | method | 요구 권한 |
|---|---|---|
| `/server/{id}` | GET/HEAD | proxy 경계; 데이터/인증값 없는 정적 shell만 반환 |
| `/server-assets/{bundle}/{name}` | GET/HEAD | proxy 경계; 고정 bundle/자산 allowlist (후속 demo.js 포함 7개) |
| `/api/v1/server/launches` | POST | proxy + 인증된 service client + delegation key |
| `/api/v1/server/launches/{id}` | DELETE | 동일한 TeeBox backchannel(미교환 접속권도 회수 가능) |
| `/api/v1/server/sessions/{id}/exchange` | POST | proxy + 정확한 Origin + 해당 id의 일회용 bootstrap |
| `/api/v1/server/sessions/{id}` | GET/HEAD | proxy + 해당 세션 cookie/CSRF |
| 동일 세션 경로 | DELETE | proxy + 정확한 Origin + 해당 세션 cookie/CSRF |
| `/api/v1/server/sessions/{id}/view` | POST | proxy + 정확한 Origin + 해당 세션 cookie/CSRF; 화면 크기만 받음 |
| 동일 view 경로 | GET/HEAD | proxy + 해당 세션 cookie/CSRF; 상태 조회 |
| `/api/v1/server/sessions/{id}/stream` | GET(WS upgrade) | proxy + 정확한 Origin + 해당 세션 cookie + protocol/bundle/CSRF subprotocol |

발급 본문은 `{user_id, source}`만 허용한다. `user_id`는 **위임 인증 후** 받는 실제 사용자
ID이고 `source`는 설정된 공용 root 아래 상대경로다. 브라우저가 임의 reviewer/root/권한을
추가할 수 없고, unknown field·과대/잘못된 JSON은 오류다. API는 세션 생성만 한다.
OASIS 파싱/인덱싱/쓰기를 하지 않으며 발급 요청을 자동 재시도하지 않는다.
응답 유실 시 해당 접속권은 만료로 정리되며, 운영 재시도/발급 receipt UX는 후속이다.

실제 사용자 ID의 대소문자/Unicode를 서버가 임의 정규화하지 않는다. TeeBox가 불변의
canonical user ID를 공급해야 한다. 같은 OS UID로 임의 코드를 실행하거나 서비스 비밀을
읽을 수 있는 공격자까지 이 세션 정책으로 격리하는 것은 아니다. OS 권한/키 보관 검증은
운영 연결의 별도 조건이다.

발급 응답은 비공개 TeeBox 채널에만 `launch_id, bootstrap, expires_in`을 준다.
`launch_id`는 별도의 난수 식별자이며 비밀값이 아니다. bootstrap은 쿠키/CSRF와 다른
난수이고 세션마다 독립적이다. 다른 id에 사용하거나 2회 교환할 수 없다.
미승인 요청은 유효한 bootstrap을 소진하지 않는다. 새 사용자의 발급이 기존 사용자를
로그아웃시키지 않는다. `viewer_ready`는 native runtime을 붙인 경우에만 true다.
이는 §7의 기본 뷰 UI 준비 상태이며 전체 서비스 배포·기능 이관 완료를 뜻하지 않는다.

쿠키는 세션별 `__Secure-floe_server_<id>`, `Secure; HttpOnly; SameSite=Strict`다.
Domain을 지정하지 않아 host-only이며 Path를 `/api/v1/server/sessions/<id>`로 한정한다.
여러 창/만료된 세션의 쿠키가 모든 요청에 함께 실려 header 상한을 소진하지 않게 한다.
cookie 이름/경로 자체를 권한으로 보지 않고 실제 cookie와 CSRF를 함께 검증한다.
기본 접속권 수명 120초, 세션 8시간이며 1초 단위로 각각 최대 600초/24시간이다.
GET은 Origin이 없을 수 있으나 CSRF가 필요하고, Origin이 있으면 정확히 검사한다.
교환·로그아웃은 Origin 필수다. query string은 받지 않는다.

세션 조회는 자기 namespace/subject만 반환하며 서버 파일 경로나 비밀값을 반환하지 않는다.
모든 응답에 no-store/nosniff/no-referrer/frame 차단 정책을 붙인다. CORS,
파일 탐색·저장·원본 export 경로는 없다. WS는 §6의 뷰 편집/프레임 ACK만 받는다.
이 router는 standalone Gateway에 mount하지 않았다.
공개 데모 설정으로 TeeBox broker를 만들 수 없다.

## 3. 수명·자원·파일 경계

세션 수는 설정의 `max_sessions`로 원자적으로 제한한다. 로그인 전 미교환 접속권과,
만료/회수됐지만 native worker가 아직 회수되지 않은 세션도 수에 포함한다.
worker 소유권은 trusted adapter만 `claim_worker`로 얻으며 세션당 한 번이다.
이는 세션의 renderer용이다. 특정 사용자의 logout 이후에도 계속돼야 하는 공용 인덱싱
작업은 이 lease에 붙이지 않는다.

- 사용자·원본 binding은 변경 불가능하다. in-root symlink도 canonical source로 고정한다.
- claim 전에 세션 권한을 검사하고, mutex 밖에서 root 및 Stamp를 재확인한 뒤 다시
  세션 권한을 검사한다. 원본 교체/범위 변경이면 claim을 거부한다.
- logout/만료/stop/조기 drop은 해당 worker의 cancellation을 알린다.
  runner는 watch 채널 종료도 취소로 취급해야 한다.
- worker receipt drop만으로 슬롯을 반환하지 않는다. 실제 child reap 뒤
  `worker_reaped`를 호출해야 한다. 예상치 못한 worker 종료도 기존 인증을 폐기하며
  새 worker에 옛 쿠키를 재사용하지 않는다.
- 유지보수는 250ms 주기다. 프로세스 재시작 후 옛 쿠키·접속권·receipt는 무효다.

control-only `serve`는 native worker를 실행하지 않는다. `serve_runtime`이 §5의
프로세스 소유·reap·취소·공용 admission을 §6의 HTTP/WS에 연결한다.
최대 세션 수를 OS RSS 상한으로 간주하지 않는다.
원본 Stamp는 메타데이터이며 내용 해시가 아니다. source/TC 등록과 revision reader lease는
작업자 연결 시 추가로 필요하다. 개인 review store 쓰기 권한을 발급한 것도 아니다.

HTTP는 생성 시 고정된 loopback socket에서만 serve한다. 동시 연결 16, body 4KiB,
header 32개/16KiB, header 수신 5초, 한 요청 10초다. 일반 응답은 연결을 닫고
WS 101 Upgrade만 유지한다. 업그레이드 이후의 별도 상한은 §6을 따른다.
파일 metadata 확인은 동시 2개 blocking 작업으로 제한한다. HTTP 취소 뒤에도 해당
작업이 끝날 때까지 permit을 유지한다. NFS syscall 자체를 강제 취소할 수 있다는 뜻은
아니며, NFS hang 시 최대 두 lookup이 남을 수 있다. stop 이후 완료된 lookup은 신규
세션을 발급하지 않는다. native workers가 남은 채 종료하면 성공으로 보고하지 않는다.

## 4. 검증과 다음 단계

```sh
cd rust
cargo test --offline --locked -j 4 -p floe-app-core --lib server::
cargo test --offline --locked -j 4 -p floe-web --lib broker::
cargo test --offline --locked -j 4 -p floe-web \
  --test server_broker --test https_proxy --test transport --test permissions_inventory
```

합성 임시 파일·loopback HTTP로 검사한다. 실제 TLS/Basic 계정·TeeBox·Electron 접속은 아니다.
권한 inventory에는 `server_delegate`, `server_bootstrap`, `server_session`을 별도로 추가했다.
이 분류는 기존 owner/guest 권한을 확대하지 않는다.

후속 S1b: 운영 key/런처 연결, 서버 UI의 나머지 패널 연결.
세션별 renderer와 공용 자원 예약·회수, HTTP/WS 전송, 기본 UI는 아래 §5~7까지 구현했다.
S2는 공유 인덱싱 실행 및 개인 review store,
S3는 Electron 서버 연결과 사용자별 창/복구 namespace, S4는 공유/익명 데모다.
TeeBox 접속권을 Electron에 전달하는 실제 비공개 IPC와 운영 key 설정도 연결해야 한다.

2026-09-28 로컬 결과: 새 broker 단위 9건, 합성 HTTP 5건 통과. 기존 HTTPS/transport/
권한 inventory와 server 정책 검사 통과. `cargo test --offline --locked -j 4 --workspace`
exit 0(기존 ignored 유지), `sh tools/validate_rust.sh --only app_cli,web_cli_inventory,web_ui`
합성 valmini ALL OK. 변경 파일 rustfmt/`git diff --check` 통과, web clippy 통과
(새 경고 없음; 기존 tiler/vfs 경고 유지). 실제 브라우저·TeeBox·TLS·RHEL 수용은 하지 않았다.

## 5. S1b-1 — 세션별 renderd 실행 코어 (2026-09-29)

`broker::runtime::Runtime`은 브로커당 하나다. standalone owner `Service` 전체를
붙이지 않고, 인증된 `Access`에 바인딩된 열기·상태·프레임·뷰 편집만 Rust API로 제공한다.
별도 사용자 ID·파일 경로·실행 파일·jobs를 뷰 요청에서 받지 않는다. 매 조작 시 원본
binding/세션 만료를 다시 확인하며 logout과 dispatch를 같은 브로커 락으로 직렬화한다.

- 모든 세션은 같은 `Resources`를 사용한다. 호스트가 제공한 렌더 옵션의
  `decode_jobs + raster_jobs`, worker 1개, decoded MB를 **파일 준비 전에** 예약한다.
  `ReservedView`가 그 permit을 worker 제어기로 넘겨 이중 예약이나 예약 공백이 없다.
  포화 시 즉시 Busy이며 숨은 대기열을 만들지 않는다. admission 실패는 접속권을 없애지 않는다.
- 원본 Stamp와 공용 root를 재검사하고 `RegisteredSource`로 jobdeck의 숨긴 레벨까지
  TC/cache 범위를 검사한다. 이번 연결은 **이미 게시된 immutable revision set**을 연다.
  기존 mutable 캐시로의 자동 폴백·재인덱싱·개명·파일 복사는 하지 않는다. 호환 기존
  캐시의 재사용/선택과 최초 요청의 준비 절차는 S2 연결 항목이지, 전 파일 재색인 결정이 아니다.
- 같은 소스는 같은 게시 파일을 읽되 사용자별 `ManagedDataset`/`ViewController`/renderd와
  뷰·generation·캐시를 가진다. 파일 reader pin은 child 종료/회수까지 유지한다.
- logout/revoke는 준비 작업에도 전달되는 atomic 취소 flag를 즉시 올린다. 렌더 제어기는
  최대 약 20ms polling 뒤 `request_close`/join하고, **완전 회수 뒤에만** broker claim을
  반환한다. 준비/worker 실패는 세션을 종료하며 옛 쿠키를 교체 worker에 재사용하지 않는다.
  런타임 Drop/close는 모든 세션을 취소하고 join한다. panic/join 실패는 claim을 남겨 오류로
  보고한다. NFS syscall 중단·20ms 종료 완료나 하드 RSS 제한을 보장하는 것은 아니다.
- 별도 서버 소유 `ManagedIndex`와 취소 flag를 공유하지 않는다. 같은 `Resources`를
  인덱싱/내보내기에도 전달해야 전역 admission이 된다. 각 기능이 새 manager를 만들면 안 된다.
  아직 S0 indexing 원장 → native 실행의 자동 연결은 없다.

기존 owner/guest API, 파일 선택·review·export·인덱싱 권한을 노출하지 않았다.
S1b-1 당시 `viewer_ready: false`였으며 현재 값의 의미는 §7을 따른다.
호스트는 `close()`를 HTTP reactor 밖에서 실행하고
worker join 뒤 서버 종료를 완료해야 한다. control-only `serve`는 미회수 worker가
있으면 오류를 낸다. §6의 `serve_runtime`은 전송 종료와 worker join을 연결한다.
실제 운영 통합 launcher는 아직 없다.

검증 게이트:

```sh
sh tools/validate_rust.sh --only server_runtime
# private synthetic OASIS/revision + 실제 native 두 worker; 외부 서버/GUI 없음
```

합성 검사에서 같은 파일의 두 사용자 프레임 일치, worker epoch/뷰 상태 분리,
한 사용자 pan/logout 후 다른 화면 유지, Busy와 회수 후 재예약,
별도 인덱싱 작업의 완료, open/pan 중 인덱스 바이트 불변을 확인한다.
누락 revision의 무쓰기 실패, 잘못된 native 경로의 자원 반환, runtime Drop,
숨긴 jobdeck TC의 root 이탈 거부도 포함한다. 일반 단위 검사에는 사전 예약·Drop,
다른 broker/stale Access 차단, 사용자별 준비 취소, Busy의 세션 유지가 추가됐다.
정상 2-source jobdeck의 새 revision 게시 중에도 기존 native reader 잠금이 유지되며,
세션 회수 뒤 잠금만 풀리고 파일은 남는 것도 검사한다.

2026-09-29 로컬 결과: broker 단위 **14건**, 실제 native 통합 **4건(debug/release)** 통과.
`cargo test --offline --locked -j 4 --workspace` exit 0(기존 ignored 유지), 최종 브로커/
HTTPS/권한 inventory 재검사 통과. `validate_rust.sh --only
validation_selector,server_runtime,view_controller,view_stream,app_cli,web_ui` ALL OK.
마지막 잡덱 reader 검사는 별도 release `server_runtime --ignored`로 4건 재실행했다.
web lib/tests clippy에 새 경고 없음(기존 tiler/vfs 경고 유지), rustfmt·diff/shell 검사 통과.
기존 미커밋 변경을 보존했으며 실제 HTTP 렌더 접속·브라우저·TeeBox·TLS·NFS·RHEL 수용이나
공용 서비스 완성을 뜻하지 않는다. 이번 단계는 커밋/푸시하지 않았다.

## 6. S1b-2 — 세션 한정 HTTP/WS 렌더 전송 (2026-09-29)

`broker::serve_runtime`은 고정 loopback listener와 trusted runtime을 받는 Rust API다.
교환 응답의 `render_transport: true`는 이 전송 구현만 뜻한다. S1b-2 당시
`viewer_ready: false`였고 UI는 없었다. 현재 정적 기본 UI는 §7에 추가했지만 기존
standalone 웹 client를 이 router에 붙이지 않는다. 운영 서버 실행 명령은 아직 없으며
`server --check-config`의 `runtime_ready: false`는 유지한다.

### 요청·격리

- `POST .../view`는 `{width,height}`만 받는다. 인증된 세션의 고정 원본으로 비동기 열기를
  시작해 202를 반환한다. 경로·사용자·jobs·실행 파일 필드는 거부한다. 중복 열기/자원 포화는
  429이며 숨은 대기열이 없다. `GET .../view`는 opening 또는 안전한 snapshot이다.
- WS subprotocol은 `floe-server-v1`, `bundle.<현재 bundle>`, `csrf.<세션 CSRF>` 세 개를
  정확히 보낸다. 서버는 `floe-server-v1`만 응답에 선택한다. CSRF를 URL query나 로그에
  넣지 않는다. 세션 cookie·Origin·고정 proxy/Host proof가 함께 필요하다.
- 입력은 `ping`, `view.set`, `frame.ack`뿐이다. view.set은 현재 연결 epoch·자기 view ID·
  base_state_rev와 기존 bounded 뷰 patch를 요구한다. sequence는 연결 내 단조 증가하는
  양의 정수 문자열이다. 소스 교체·query/pick·clip·review·index·파일 API는 노출하지 않는다.
- 기존 snapshot/frame DTO를 쓰되 query/clip/mode capability는 끈다. frame query scene도
  무효화한다. 일반 native 실패는 경로/상세 stderr 대신 안전한 오류 상태만 전달한다.
  신원/권한은 WebSocket upgrade 한 번이 아니라 입력·상태 조회·출력 때 다시 검사한다.

### 프레임·재접속·수명

- 세션당 WS 한 개, 서버당 최대 32개다. binary frame은 한 장만 전송 중이며 **socket write
  완료와 client ACK 둘 다** 확인한 뒤 다음 credit을 준다. ACK 대기 중에도 뷰 편집은 처리한다.
  중간 프레임을 쌓지 않고 controller의 최신 프레임만 선택한다.
- 출력 채널 4개, JSON 응답 256KiB, 입력 message/frame 8KiB, 초당 입력 60개다.
  frame payload 상한은 기존 80MiB, header 상한 64KiB다. 서버 공용 전송 예약은 256MiB이고
  packet의 두 배를 복사 전에 예약해 write/ACK까지 보유한다. blocking copy는 동시 2개다.
  이 예약은 native decoded budget이나 OS RSS 상한을 대체하지 않는다.
- write 5초·frame ACK 10초·수신 무응답 30초 timeout, heartbeat 10초다. 끊긴 연결의 worker는
  즉시 없애지 않는다. 인증된 뷰 활동이 120초 없으면 정리하며 그 안에 재접속하면 같은
  worker/현재 뷰를 읽되 **새 connection epoch**를 발급한다. 이전 epoch의 입력은 재생하지
  않는다. 응답 유실 뒤에는 snapshot과 조정하고 상대 pan 등을 자동 재전송하면 안 된다.
- heartbeat/재접속은 인증 세션 수명을 연장하지 않는다. logout/revoke/TTL은 열린 socket과
  native 작업을 취소한다. 정상 서버 종료는 WS slot·copy·packet 예약을 drain하고 blocking
  thread에서 worker join한다. drain 실패는 성공으로 보고하지 않는다. serving future 취소도
  native cleanup을 예약한다. NFS에 묶인 준비 syscall의 강제 중단/하드 종료 시한은 보장하지 않는다.

### 합성 검증

`server_runtime` 게이트가 기존 native 4건에 HTTP/WS 5건을 더해 총 9건을 실행한다.
합성 OASIS/게시 revision·실제 renderd·실제 loopback HTTP/WS를 사용하지만 HTTPS proof는
테스트 header로 모의한다. 브라우저·실제 TLS 프록시·TeeBox·Electron·RHEL 수용은 아니다.

- 두 사용자 같은 초기 pixels, 독립 worker/view, 한 사용자 편집/logout 뒤 다른 세션 유지.
- reconnect 시 같은 worker/current state + 새 epoch, 구 epoch/다른 사용자 view ID 거부.
- cookie/CSRF/Origin/proxy/bundle 거부, 중복 WS·알 수 없는 open field·owner API 차단.
- 미ACK 동안 다음 frame 보류 + 편집 응답 허용, ACK 후 재개, logout의 열린 WS 회수.
- 살아 있는 WS의 세션 만료, 정상 종료/serving future abort 후 native 자원 완전 회수.

배포 후 자원 설정·장시간 부하·실칩 메모리/지연 검증은 별도로 남는다.

2026-09-29 S1b-2 로컬 결과: broker 단위 **19건**, 실제 native 통합 **9건(debug/release)**,
브로커 HTTP 5건·HTTPS 경계 3건·transport 15건·권한 inventory 3건 통과(기존 ignored 유지).
`cargo test --offline --locked -j 4 --workspace` exit 0. `validate_rust.sh --only
validation_selector,server_runtime,view_controller,view_stream,app_cli,web_cli_inventory,web_ui`
합성 valmini ALL OK. web lib/tests clippy `--no-deps`에 새 경고 없음(기존 tiler/vfs 경고 유지),
변경 파일 rustfmt·diff/shell 검사 통과. 검사 전용 venv 링크는 제거했다. 기존 미커밋 변경을
보존했으며 커밋/푸시는 하지 않았다. 전체 배터리 재실행이나 실제 브라우저/현장 수용은 아니다.

## 7. S1b-3 — 서버 전용 기본 뷰 UI (2026-09-29)

`/server/{launch_id}`는 별도 `server.html/server.js`를 로드한다. standalone `app.js`나
guest 앱을 재설정해 쓰지 않는다. 재사용하는 코드는 정적 CSS·프레임 검증·이미지 디코드·
마우스 제스처뿐이다. 원본 경로/실제 사용자/쿠키/접속권을 HTML에 삽입하지 않으며,
shell 조회가 session을 발급하거나 renderer를 시작하지 않는다.

UI가 제공하는 범위:

- 자기 고정 원본 열기, raw/PNG frame 표시, Fit·키보드/드래그 pan·커서 기준 wheel zoom·Goto.
- depth/detail/thin, frames/labels/mono 토글. 덱의 지원하지 않는 labels는 비활성화한다.
- 세션 재접속과 **자기 세션만** 끝내기(확인창). 창을 숨기거나 연결이 끊기면 canvas와
  미전송 입력을 비우고, 돌아올 때 현재 세션과 뷰 상태부터 다시 읽는다.

레이어/palette 패널, 우클릭 zoom band, margin prefetch, query/측정·DRC·개인 저장·
파일 탐색·인덱싱·export·초대/공개 데모를 연결한 것은 아니다. 현재 UI는 S1 전송 확인용
기본 뷰이며, 기존 standalone의 전체 기능과 성능이 이관됐다고 해석하지 않는다.

### 접속권·복구 계약

비공개 런처 연결 시 사용할 경로 형식은 `https://<fixed origin>/server/<id>#bootstrap=<one-use>`다.
UI는 어떤 HTTP 요청보다 먼저 fragment를 주소/현재 history 항목에서 지우고 정확한
세션의 `/exchange`에 한 번만 보낸다. 교환 실패/응답 유실은 자동 재시도하지 않는다.
이 링크를 로그·argv·공유 초대에 넣어서는 안 되며, 실제 TeeBox→Electron 비공개 전달은 S3다.
브라우저 시작 페이지/확장 기능 등의 fragment 노출 가능성까지 지워 주는 계약은 아니다.

교환 뒤 CSRF는 tab `sessionStorage`의 **origin + launch ID** 전용 namespace에만 저장한다.
standalone/guest/다른 세션의 storage는 읽거나 바꾸지 않는다. 실제 사용자는 인증된 inspect
응답으로만 표시하고 `textContent`를 쓴다. 쿠키는 기존 HttpOnly/API path 범위를 유지한다.
tab storage가 막히면 현재 연결은 진행하되 reload에는 새 launch가 필요하다.

`GET .../view`의 `unopened`는 아직 native 예약이 없는 상태다. 새 교환 직후에만 자동 open을
한 번 시도한다. 포화 또는 응답 유실 뒤에는 GET으로 결과를 확인하며, 여전히 unopened이면
**Open assigned layout** 버튼의 명시 재시도만 받는다. 이미 opening/열린 뷰를 다시 POST하지
않는다. 새 worker 준비 실패/인증 만료 후에는 새 launch가 필요하다.

WS의 한 프레임 credit은 decode 및 RAF 표시/폐기 뒤 반환한다. state/epoch가 바뀐 frame은
ACK만 하고 그리지 않는다. 뷰 편집은 한 요청과 그 응답 snapshot을 확인한 뒤 다음 요청을
보내며, 100ms 간격·대기 입력 8개로 한정한다. handshake/편집 응답은 10초, 수신 무응답은
30초에 재접속한다. transient 실패는 0.5~5초 backoff로 재조회하되 401/403/404는 종료한다.
연결을 바꿀 때 전송 중/대기 pan·zoom을 재생하지 않는다. 로그아웃 실패 시에도 화면과 tab
credential은 지우고 서버 종료 미확인을 표시한다. DELETE 자동 재전송은 없다.

정적 자산은 bundle에 고정한 6개만 제공한다. 다른 bundle, `app.js`, `guest.js`, defaults 등은
404다. HTML도 고정 Host/proxy 경계를 통과해야 한다. CSP는 같은 origin의 script와 지정 wss만
허용하며 eval·iframe·외부 script·form 제출·base 변경을 허용하지 않는다. 모든 응답에
no-store/no-referrer/nosniff를 유지한다. 새 server UI와 broker 소스도 bundle hash에 넣었다.

검증: 새 DOM 회귀는 주소 제거·단일 교환·storage 분리·픽셀/ACK·마우스·Goto·직렬 편집·
재접속 실패 반복·열기 응답 불명·숨긴 tab의 교환·로그아웃 불명을 검사한다. HTTP 회귀는
shell의 무권한 데이터 부재와 CSP/자산 allowlist를 확인한다. `server_runtime`의 10번째 검사는
**실제 native 프레임/상태를 Node의 서버 UI에 전달해 canvas와 ACK까지** 확인한다.
DOM/RAF는 합성이므로 실제 Chrome/Electron/Firefox 수용이나 스크린샷 검증은 아니다.
Node 18+는 개발 검사에만 필요하며 배포 런타임 의존성이 아니다.

2026-09-29 S1b-3 로컬 결과: broker 단위 19건, HTTP 6건, native 통합 10건(debug/release)
통과. 전체 Rust workspace exit 0(기존 ignored 유지), `validate_rust.sh --only
server_runtime,view_controller,view_stream,app_cli,web_cli_inventory,web_ui` 합성 valmini ALL OK.
마지막 이전 이미지/렌더 실패 문구 보완 뒤 새 DOM 회귀·최종 자산 재빌드·broker/HTTP/native
재검사도 통과했다. web lib/tests clippy `--no-deps` 새 경고 없음(기존 tiler/vfs 경고 유지),
ES2017 구문·변경 파일 rustfmt·diff/shell 검사 통과. 임시 venv 연결은 제거했고 기존 변경은
보존했다. 실제 GUI/현장 수용·공용 서버 공개·커밋/푸시는 하지 않았다.
