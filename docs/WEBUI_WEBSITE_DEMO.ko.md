# 회사 홈페이지용 읽기 전용 데모

2026-09-29, `feature/webui`. **TeeBox는 Electron standalone을 유지한다.** 이 문서는
별도 공개 샘플을 로그인 없이 탐색하는 홈페이지 데모만 다룬다. TeeBox 위임 인증·개인
작업 파일 서비스·Electron 원격 연결은 재개하지 않는다. 실제 인터넷 공개, 인증서 설치,
방화벽·nginx/Caddy 변경은 이 구현 과정에서 하지 않았다.
HTTPS가 기본이며 내부망 HTTP 시험만 아래 명시적 opt-in으로 허용한다.

## 구성과 제공 범위

```text
회사 홈페이지의 데모 링크
  → https://demo.example.test/demo (공개 샘플 ID 목록)
  → 명시적 샘플 선택 → 방문자별 일회용 접속권
  → 독립 뷰 / renderd → 사전에 준비한 공용 immutable 인덱스 읽기
          HTTPS nginx → 127.0.0.1 Rust
```

지원: pan·zoom·fit·goto·depth·detail·thin·frames·labels·mono, PNG 화면 전송,
세션 재접속과 자기 세션 종료. 로그인/브라우저 설치 확장이 필요 없다.
캔버스에서 좌/중 버튼 드래그는 pan, 우 버튼 드래그는 zoom band다. 오른쪽 방향은 확대,
왼쪽 방향은 축소이며 release에서 한 번 제출한다. Escape는 취소하고 canvas의 브라우저
context menu는 막는다. 현재 프레임이 표시되기 전/이전 화면 대기 중에는 band를 받지 않는다.
줌·자유 pan처럼 이전 프레임의 정확한 16px 위상 배치가 불가능하거나 detail·thin·depth·
frames·labels·mono가 바뀌어도, 같은 고정 소스 세션·연결·데이터 revision·worker의
**이미 표시된 화면**은 새 프레임까지 유지한다. 표시 옵션은 렌더 정책이지 접근 권한이 아니다.
상태줄은 `Previous image · waiting for current frame`으로 구별하며 이를 현재 프레임으로
승인하거나 stale 수신 프레임을 새로 표시하지 않는다. 수신·디코드·실제 표시 단계의
render key/revision 일치 검사는 그대로 유지한다. 연결 해제·hidden·로그아웃·데이터/worker
변경에서는 비운다. pan release의 마지막 미리보기도 유지하되 no-op/거부 시 원래 뷰로 복원한다.
렌더 대기는 화면과 조작부 전체의 `wait` 커서로 표시한다. 요청 대기열·ACK·서버 상태 전파·
이미지 디코드·브라우저 표시까지 포함하며, ACK나 서버 idle만으로 해제하지 않는다.
현재 최종 프레임이 표시되면 복원하고 no-op·거부·실패·연결 종료에서도 대기가 남지 않게 한다.
최종 프레임이 incomplete인 경우에는 그 상태를 문구로 알리고 커서를 계속 기다리게 하지 않는다.
현재는 **기본 데모 UI**이며 전체 Electron UI의 레이어 패널·셀 트리·측정·pick/snap은
이 경로에 연결하지 않았다. 큰 DPR 화면은 내부 렌더 해상도를 제한해 표시한다.

파일 업로드·서버 탐색·인덱싱·DRC 읽기/쓰기·원본/clip 내보내기·공유 기본값·초대
API를 마운트하지 않는다. UI 숨김만으로 제한하지 않는다. 데모 모드에서는 TeeBox
위임 launch/revoke도 거부한다. 단, **표시한 픽셀·이름은 공개 정보**이며 브라우저
스크린샷까지 금지하는 DRM이 아니다. 공개 승인된 합성/샘플 데이터만 별도 root에 둔다.

## 1. 실행 파일과 데이터 준비

서버에는 Python·Electron이 필요 없다. 같은 빌드의 `floe2-web`, `floe-index`,
`floe-renderd`를 한 폴더에 둔다. Linux 배포판/GLIBC 호환성은 기존 빌드 절차를 따른다.

```sh
cd rust
cargo build --offline --locked --release -j 4 -p floe-app -p floe-index -p floe-renderd
./target/release/floe2-web selfcheck --adjacent
```

관리자가 [demo.example.json](../tools/web-service/demo.example.json)을 복사·편집한다.
예제의 `data_root`, `runtime_root`는 먼저 만들어져 있어야 하며 서로 포함되면 안 된다.
`public_origin`은 경로·끝 slash 없는 정확한 HTTPS origin, 예: `https://demo.company.com`.
아래 HTTP 시험 opt-in에서만 `http://`를 사용한다.
`max_sessions`는 **1~4**다. 샘플 ID는 ASCII 영문/숫자/`_`/`-` 1~64자이며 UI에 그대로
표시한다. `source`는 data_root 아래 상대경로다. jobdeck TC 의존 파일도 root 안이어야 한다.
TeeBox의 실칩 공용 root를 데모 root로 지정하지 않는다.

이하 `/etc/floe2-demo/demo.json` 등은 **관리자가 준비할 예시 경로**다.
비특권 전용 사용자로 실행하고 해당 사용자에게 승인 샘플/인덱스 외의 사내 파일 접근을
주지 않는 것이 배포 전제다. root 실행은 피하며, 같은 UID의 프로세스 분리는 sandbox가 아니다.

```sh
/opt/floe2/bin/floe2-web server --check-config /etc/floe2-demo/demo.json
/opt/floe2/bin/floe2-web server --prepare-demo /etc/floe2-demo/demo.json --jobs 4
# 필요할 때 명시적으로 LOD 포함 전체 revision 재빌드
/opt/floe2/bin/floe2-web server --prepare-demo /etc/floe2-demo/demo.json --jobs 4 --lod
```

`--check-config`는 읽기 전용이며 `runtime_ready:false`가 정상이다. TLS·인덱스가 준비됐다는
검사가 아니다. `--prepare-demo`는 **구성된 모든 샘플의 full immutable revision을 게시하는
명시적 쓰기**다. 일반 `index`의 mutable 캐시만으로는 데모가 열리지 않는다. 기존 current
revision을 즉석에서 덮어쓰거나 삭제하지 않으며 저장 공간이 추가로 필요하다. 반복 실행도
새 빌드다. 샘플은 순서대로 처리하므로 뒤 샘플이 실패해도 앞 샘플의 게시까지 롤백하지 않는다.
jobdeck은 전체 선택 소스가 대상이며 기존 기본값대로 occupancy도 만든다. 준비 중 중단은
SIGINT/SIGTERM/SIGHUP을 처리한다. 자동 삭제·회수·폴더 감시는 없다.

준비 후 공개 서비스는 인덱스 읽기만 한다. 초기 운영에서는 서비스 중지 → 명시적 재준비
→ 재시작을 권한다. 열린 세션은 pin한 revision을 계속 쓰고 자동 hot reload하지 않는다.
캐시 lock/소유권 규약 때문에 별도 계정이나 읽기 전용 mount는 실제 권한으로 확인해야 한다.

## 2. 프록시 전용 키와 Rust 실행

관리자가 암호학적 난수 32바이트를 소문자 hex 64자로 생성해 별도 파일로 보관한다.
예를 들어 **아직 없는 파일**에 `umask 077`을 적용한 `openssl rand -hex 32` 출력을 저장한다.
키 값은 명령 인수·JSON 설정·저장소·웹 root·채팅·로그에 넣지 않는다.
키 파일은 실행 UID 소유의 0600 또는 0400 일반 파일이어야 하며 symlink/hardlink는 거부한다.
마지막 LF 한 개는 허용한다. 아래는 파일 경로만 전달한다.

```sh
/opt/floe2/bin/floe2-web server --demo /etc/floe2-demo/demo.json \
  --proxy-key-file /etc/floe2-demo/proxy.key --port 58080
```

시작 전에 모든 샘플의 등록 범위·published revision을 확인한다. 준비되지 않았으면
리스너를 열지 않고 `--prepare-demo` 안내로 실패한다. **요청/열기에서는 인덱서를 실행하지 않는다.**
고정 `127.0.0.1`만 listen하며 `--listen 0.0.0.0` 옵션은 없다. ready 줄은 `/demo` URL만
출력한다. loopback 포트의 직접 브라우저 접속은 프록시 증명이 없어 403이 정상이다.
SIGINT/SIGTERM/SIGHUP에서 세션을 닫고 렌더러 회수를 기다린다. NFS가 무한 대기하면
filesystem 호출 종료까지 지연될 수 있어 종료 시간을 무조건 보장하지 않는다.

현재 `runtime_root`는 정책상 분리된 운영 폴더다. renderer scratch의 위치를 이 필드로
바꾸지는 않는다. 필요하면 서비스 전용 private `TMPDIR`을 별도로 제공한다.

## 3. HTTPS 프록시와 홈페이지 링크

[nginx 예제](../tools/web-service/demo.nginx.example.conf)는 별도 데모 origin의 `http {}`에
포함하는 템플릿이다. hostname·인증서·포트·include 경로를 현장에 맞춘다. 단순 prefix
rewrite나 홈페이지 하위 `/floe/` mount는 지원하지 않는다. 전용 subdomain을 사용한다.

[비공개 header include 예제](../tools/web-service/demo.proxy-headers.example.conf)의 placeholder를
Rust 키 파일과 같은 값으로 관리자가 채운다. nginx master/관리자 외에는 읽지 못하도록
보호한다. Rust 키 파일 소유권은 Rust 실행 UID를 유지한다. 외부에서 받은 proxy key는
이 설정으로 덮어쓰며 TeeBox 위임 header/Authorization은 제거한다. 새 `proxy_set_header`를
location에 추가하면 상위 설정 상속이 끊어질 수 있으므로 전체 header 규칙을 재검토한다.

`Host`/`Origin`은 그대로 전달해 Rust의 고정 origin 검사와 대조한다. 인증 cookie/CSRF,
일회용 bootstrap과 WebSocket subprotocol 검사는 익명 데모에서도 유지된다. 프록시 키는
로그인이 아니라 신뢰한 프록시 경유 증명이다. 브라우저에는 전달하지 않는다.

WebSocket의 Upgrade/Connection 전달과 heartbeat보다 긴 timeout은
[nginx WebSocket 공식 문서](https://nginx.org/en/docs/http/websocket.html)를 따른다.
per-IP 요청/launch 제한은 [limit_req 공식 문서](https://nginx.org/en/docs/http/ngx_http_limit_req_module.html),
header 상속·buffer/retry 설정은 [proxy 공식 문서](https://nginx.org/en/docs/http/ngx_http_proxy_module.html)를
확인했다. 이 예제는 실제 nginx/TLS 환경에서 실행 검증한 배포 인증서가 아니다.
운영자가 `nginx -t`와 실제 HTTPS/WS 연결을 확인해야 한다. CDN 앞단이 있다면 실제 client IP
신뢰 설정을 별도로 검토하며 임의 `X-Forwarded-For`를 신뢰하지 않는다.

회사 홈페이지에는 고정 진입점만 링크한다. 방문자 세션 URL이나 `#bootstrap`을 게시하지 않는다.

```html
<a href="https://demo.company.com/demo" target="_blank" rel="noopener noreferrer">
  Floe2 레이아웃 데모 열기
</a>
```

iframe은 CSP `frame-ancestors 'none'`/X-Frame-Options DENY로 차단한다. 새 탭 방식이다.
프록시 예제는 데모/세션 경로만 전달하고 나머지는 404로 닫는다. 쿠키/인증 header/응답 body를
access/debug log에 기록하지 않는다. 실제 공개·인증서·방화벽 설정은 별도 운영 작업이다.

## 내부망 HTTP 시험 (Caddy, 명시적 opt-in)

공인 서브도메인·인증서 없이도 **Caddy의 내부망 HTTP 포트 → loopback Rust**로
동일한 읽기 전용 데모를 시험할 수 있다. 일반 HTTPS와 TeeBox/standalone 인증 정책은
바뀌지 않는다. 기존 인덱스를 재준비할 필요는 없다.

**HTTP에는 암호화·전송 무결성이 없다.** 화면·샘플 이름·일회용 접속권·세션 cookie/CSRF를
내부망의 공격자가 도청하거나 변조할 수 있다. 공개 승인 샘플만 사용하고, 관리자 승인된
시험망으로 Caddy bind/방화벽/접속 대역을 제한한다. 설정의 hostname/IP가 실제 내부망인지
Floe2가 DNS 조회 등으로 판정하지 않는다. `allow_insecure_http`는 인터넷 공개 허가가 아니다.

1. [HTTP JSON 예제](../tools/web-service/demo-http-test.example.json)를 참고해 기존
   `demo.json`의 `public_origin`을 `http://실제서버내부IP:8080`으로 바꾸고,
   **`deployment` 안에** `"allow_insecure_http": true`를 추가한다. 생략/false는 HTTPS만
   허용한다. true인데 HTTPS origin이면 설정 오류로 거부한다. TeeBox mode에는 이 필드가 없다.
2. root/runtime/샘플 경로는 기존의 실제 경로를 유지한다. `--check-config` 결과에서
   `insecure_http_test:true`를 확인한다. `runtime_ready:false`는 여전히 정상이다.
3. Rust 실행은 기존 명령 그대로다. `--port 58080`을 유지하고 재시작한다.
   `WARNING: HTTP demo test mode is unencrypted`와 `HTTP test proxy required`를 출력한다.
4. [Caddy HTTP 예제](../tools/web-service/demo-http-test.Caddyfile)의 IP와 허용 client subnet을
   현장에 맞춰 바꾼다. **사이트 수신 포트는 8080, `reverse_proxy` 대상만 58080**이다.
   기존 `floe-demo.company.com:58080 { ... }` 같은 충돌 블록은 비활성화하고 다른 회사 사이트는
   그대로 둔다. Rust 포트를 외부에 노출하지 않는다.
5. [비공개 Caddy include 예제](../tools/web-service/demo.proxy-headers.example.caddy)는
   `/etc/caddy/floe-demo-proxy.caddy`로 준비한다. Rust 키와 같은 값을 사용하고, 관리자/Caddy
   실행 계정만 읽게 한다. Rust `proxy.key`는 계속 Rust 실행 UID 소유의 0600/0400이다.
6. 아래 검사에 성공한 경우만 기존 Caddy service를 reload한다. 회사 Caddy가 container/별도
   호스트라면 `127.0.0.1`의 의미가 달라지므로 예제를 그대로 사용하지 않는다.

```sh
/opt/floe2/bin/floe2-web server --check-config /etc/floe2-demo/demo.json
/opt/floe2/bin/floe2-web server --demo /etc/floe2-demo/demo.json \
  --proxy-key-file /etc/floe2-demo/proxy.key --port 58080
# 별도 터미널: validate 성공 후에만 reload
sudo caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
sudo systemctl reload caddy
```

브라우저에서 **`http://실제서버내부IP:8080/demo`**를 연다. 인증서 설치·SSH 터널은 필요
없으며 지정한 origin과 주소·포트가 정확히 같아야 한다. HSTS가 걸린 회사 도메인은 브라우저가
HTTPS로 올릴 수 있으므로 내부 IP 사용을 권한다. 원래 cookie를 Caddy에서 수정하거나 Origin을
덮어쓰지 않는다. `X-Floe-Proxy-Key`는 외부 값을 신뢰하지 않고 private include로 덮어쓴다.

HTTP 모드에서는 `floe_http_test_<id>`라는 별도 host-only cookie를 사용하고 Secure 속성만
뺀다. HttpOnly/SameSite=Strict·세션별 경로·수명·CSRF·일회용 교환·proxy proof·정확한
Host/Origin 검사는 그대로다. HTTPS는 기존 `__Secure-floe_server_<id>`/Secure를 유지한다.
UI는 서버가 응답에 넣은 HTTP 시험 표식을 확인하며 경고를 표시하고 `ws://`로 연결한다.
HTTPS 모드는 표식이 false이며 `wss://`다. 프록시 header/query로 transport를 변경할 수 없다.

검증 근거: [Caddy reverse_proxy](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy),
[Caddy bind](https://caddyserver.com/docs/caddyfile/directives/bind),
[Set-Cookie/Secure prefix](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Set-Cookie).
내부 HTTP opt-in은 전송 보안의 예외이지 읽기/쓰기 권한 확장이 아니다. 실제 사내망·방화벽·
현장 브라우저 수용은 별도다. HTTPS로 돌아갈 때는 flag를 제거하고 origin/Caddy를 함께 바꾼 뒤
새 세션을 연다. 서비스 재시작 시 이전 세션은 유효하지 않다.

## 자원·세션 제한

| 항목 | 기본/상한 |
|---|---|
| 동시 세션 | 설정 1~4; 닫히는 native가 회수되기 전 슬롯 재사용 금지 |
| 접속권 / 세션 | 교환 전 30초 / 교환 후 최대 15분 |
| native idle | 뷰 접근 없는 120초; 활성 스트림은 접근으로 간주 |
| launch rate | 프로세스 전체 burst 4, 이후 5초당 1개; 프록시 per-IP 제한 별도 |
| 화면 | 가로/세로 각각 2048 이하, 면적 2,097,152 px 이하; UI 자동 축소 |
| 세션별 render | decode 2 + raster 2, decoded 예산 256 MiB, refinement off, PNG |
| 공용 admission | CPU slot 16, renderer 4, decoded 예약 1024 MiB |

브라우저는 jobs/budget/binary/root를 변경할 수 없다. 일반 로컬 `FLOE_RUST_JOBS` 등으로
데모의 위 RenderOptions를 확대하지 않는다. 다만 renderer의 다른 진단 환경변수는 별도이므로
운영 서비스 환경을 깨끗하게 유지한다. 예약은 RSS/전체 스레드의 하드 상한이나 DoS 방어 완성이
아니다. scene/mmap/retained/PNG·제어 스레드 메모리는 별도다. 외부 공개에는 전용 OS 계정,
OS 자원 제한, 프록시 rate limit·모니터링, 공개 샘플 fit/exact 비용 실측이 필요하다.
페이지에 표시한 approx/partial을 정확한 마스크 검증 결과로 오해하지 않도록 한다.

## 검증과 잔여

정책/키 파일·HTTP 권한 목록·샘플 ID/경로·cross-session/Origin·읽기 전용 API·rate limit,
ES2017 UI의 명시적 launch/재전송 금지·고DPR 해상도 제한을 자동 검사한다.
`server_runtime` 합성 게이트는 실제 CLI로 revision 준비 → loopback 데모 → 일회용 교환 →
PNG WS 수신 → 과대 해상도 거부 → SIGHUP 종료를 실행하고 원본/인덱스 바이트 불변을 확인한다.
HTTPS 프록시 증명은 테스트 header로 모사하므로 실제 TLS/nginx/일반 방문자 브라우저 수용은
별도로 남는다. RHEL/ETX 및 실제 NFS 수용을 이 결과로 완료 처리하지 않는다.

2026-09-29 로컬 검증: server 정책/secret 12, app parser 1, broker HTTP 7,
권한 inventory 3, 정책 CLI 2, web 단위 149 통과(별도 ignored 3은 실행하지 않음).
`sh tools/validate_rust.sh --only server_runtime,app_cli,web_cli_inventory,web_ui`
ALL OK (`server_runtime` native 11건 포함).
관련 세 패키지 `cargo clippy --offline --locked -j 4 --all-targets --no-deps` 통과;
의존 tiler/vfs의 기존 unused/dead-code 경고는 남고 이 변경의 새 경고는 없었다.

2026-09-29 HTTP 시험 opt-in 추가 검증: server 정책/secret 14, web 단위 150,
broker HTTP 8, HTTPS proxy 3, 권한 inventory 3, 정책 CLI 3 통과. `server_runtime`은
HTTP/HTTPS 각각의 CLI 준비·실제 PNG WebSocket·상한 거부·SIGHUP 회수를 포함해 12건 통과.
`sh tools/validate_rust.sh --only server_runtime,app_cli,web_cli_inventory,web_ui` ALL OK.
관련 세 패키지 clippy 통과(기존 의존 경고만 유지). Caddy 2.11.4에서 새 HTTP 예제를
합성 proxy key로 `caddy validate`하여 통과했다. 실제 Caddy 프록시 경유 브라우저·사내망
접속은 아직 검증하지 않았고 운영 설정·인증서·방화벽을 변경하지 않았다.

2026-09-29 데모 입력/화면 유지 회귀: `server.test.cjs`에서 우 버튼 band 확대·축소,
Escape/blur/state 변경 취소, letterbox 좌표, 좌/중 버튼 13px pan release, no-op/거부 복원,
버튼/키/휠 줌 대기 화면 유지와 stale 패킷 폐기, worker/dataset 변경 및
hidden/연결 해제/로그아웃 화면 정리를 고정했다. 전체 `validate_web_ui.cjs` 통과.
`server_runtime` 12건도 통과했고 HTTP/HTTPS 데모 CLI 두 경로에 실제 band 명령 →
native PNG·bbox 확대/축소·render revision 변경·공용 인덱스 불변 검사를 추가했다.
이는 합성/loopback 검증이며 수정 후 회사 브라우저에서의 실제 마우스 수용 검사는 별도다.
UI가 실행 파일에 내장되므로 배포에는 `floe2-web` 재빌드·교체·재시작 후 `/demo`에서
새 세션 열기가 필요하다. 이 변경에 재인덱싱이나 프록시 설정 변경은 필요 없다.

2026-09-29 표시 옵션/대기 커서 회귀: 위 화면 유지 계약을 detail·thin·depth·frames·labels·
mono에도 확장했다. `server.test.cjs`는 옵션별 render key 변경 후 캔버스 초기화가 없는지,
이전 정책의 패킷을 버리는지, ACK/idle 뒤에도 디코드와 표시를 기다리는지 검사한다.
연속 변경 대기열·지연 디코드 중 새 revision·최종 incomplete·요청 거부/no-op·오류/연결 종료의
커서 복원도 고정했다. 좌/중/우 드래그의 grab/crosshair → wait → 기본 커서 복원도 검사한다.
`sh tools/validate_rust.sh --only server_runtime,web_ui` ALL OK
(native 12건, 전체 ES2017/UI 회귀). 회사 서버 배포 및 실제 브라우저에서의 깜빡임 수용
검사는 별도다.
