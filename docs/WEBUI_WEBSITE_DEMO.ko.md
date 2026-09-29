# 회사 홈페이지용 읽기 전용 데모

2026-09-29, `feature/webui`. **TeeBox는 Electron standalone을 유지한다.** 이 문서는
별도 공개 샘플을 로그인 없이 탐색하는 홈페이지 데모만 다룬다. TeeBox 위임 인증·개인
작업 파일 서비스·Electron 원격 연결은 재개하지 않는다. 실제 인터넷 공개, 인증서 설치,
방화벽·nginx 변경은 이 구현 과정에서 하지 않았다.

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
