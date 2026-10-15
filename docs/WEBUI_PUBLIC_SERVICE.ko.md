# TeeBox / Electron · 공용 인덱스 · 읽기 전용 웹 서비스

2026-09-29 갱신, `feature/webui`. **TeeBox 공용 서버 도입은 보류.**
당분간 [TeeBox standalone](WEBUI_TEEBOX_STANDALONE.ko.md)으로 실행한다.
후속 요청으로 **[회사 홈페이지 공개 데모](WEBUI_WEBSITE_DEMO.ko.md)만** 실행 CLI·
사전 인덱스 준비·익명 샘플 선택을 연결했다. 실제 인터넷 공개는 하지 않았다.
아래는 보존하는 TeeBox 서버 설계/구현 기록이며 그 운영 연결·후속 단계는 진행하지 않는다.
**S0 정책/preflight, S1a 세션 브로커, S1b-3 세션별 native·HTTP/WS와 기본 서버 UI까지 구현했다.
TeeBox용 공용 뷰어 서비스는 아직 실행할 수 없다.**
현재 브라우저 원격 실행 절차는 [Linux + SSH 터널](WEBUI_LINUX_USAGE.ko.md)이다.
실제 서버 공개, 계정 생성, 인증서·방화벽·SSO 설정 변경은 하지 않는다.

## 1. 확정한 제품 구조

| 모드 | 주 진입점 | 데이터·신원 | 쓰기/인덱싱 |
|---|---|---|---|
| standalone | 기존 Electron | 로컬 런처 허용 폴더·사용자 | 기존 명시적 승인 유지 |
| TeeBox | 사용자 DISPLAY의 Electron | 공용 데이터 root + 위임받은 실제 사용자 ID | 최초 요청의 공용 인덱싱, 개인 작업 파일 |
| 공유/홈페이지 데모 | 브라우저 링크 | 공유 초대 범위 / 승인된 공개 샘플 | 읽기 전용; 데모 인덱싱 금지 |

```text
TeeBox(서비스 계정 인증 + 실제 사용자 ID)
   └─ 세션 요청 → 사용자별 렌더 프로세스 → 공유 원본/인덱스 읽기
            └─ 1회 접속권 → 사용자 DISPLAY의 Electron
                                     └─ 허용된 공유 링크 → 브라우저
회사 홈페이지 → 별도 공개 데모 진입점 → 승인된 샘플 독립 탐색
```

TeeBox 주 작업자를 위해 Firefox 대신 기존 Electron에 **서버 연결 모드**를 추가한다.
standalone의 로컬 backend·파일 선택·인증은 유지한다. 서버 연결 창 종료는 자기 세션만
끝내며 공용 서버·타 사용자·공용 인덱싱을 죽이지 않는다. 원본/인덱스는 사용자마다
복사하지 않지만 renderer의 디코드 메모리·뷰 상태까지 프로세스 사이에 공유되는 것은 아니다.

## 2. 신원과 신뢰 경계

서비스 로그인(TeeBox), 실제 사용자 ID, OS 실행 계정, 뷰어 세션은 서로 다르다.
실제 사용자마다 OS 계정과 레이아웃 폴더를 따로 둔다는 초기 제안은 철회한다.
같은 UID의 프로세스 분리는 보안 격리나 OS 파일 권한 분리가 아니므로 서버 내부의
개인 작업 권한 검사가 필요하다. DISPLAY·표시 이름·reviewer 문자열은 신원 증명이 아니다.

- HTTPS 프록시 → loopback Rust를 유지한다. 프록시가 서비스 계정·비밀번호를 검증한다.
- 외부 identity/proxy header를 제거하고 인증 결과로 재설정한다. 고정 origin은 관리자
  설정이며 forwarded header에서 유도하지 않는다.
- **인증된 TeeBox**만 실제 사용자 ID를 위임한다. 브라우저 `user_id`로 사용자를 바꾸지 않는다.
- 서버가 사용자·데이터·권한·만료에 바인딩한 1회 접속권을 발급한다. 재사용 가능한 비밀이나
  서비스 비밀번호는 Electron argv·일반 로그에 넣지 않는다.
- Electron은 고정 origin만 연결한다. 임의 HTTPS URL 허용, 인증서 검증 해제, 로컬 파일
  선택기를 통한 서버 root 확대는 하지 않는다.
- 창 재사용 키와 브라우저 복구/저장 namespace에 서비스·principal을 포함해야 한다.
  기존 UID+DISPLAY만으로 다른 실제 사용자를 합치지 않는다.

**[S1a 세션 브로커](WEBUI_SERVER_SESSIONS.ko.md)의 위임 검사·일회용 접속권 교환을 구현했다.**
실제 프록시 로그인 설정/키 관리/TeeBox 런처에는 아직 배선하지 않았다.
`Principal::delegated`와 client 이름 비교만은 여전히 인증 함수가 아니다.
브로커는 별도 TeeBox 위임 키까지 검사한 뒤 principal을 만들며, 브라우저는 TeeBox
비밀번호 없이 일회용 접속권으로 연결한다. 발급/회수 backchannel과 브라우저 session의
인증을 프록시 배포에서도 구분해야 한다.

## 3. 공용 데이터와 개인 작업

`shared_root`는 TeeBox의 공용 최상위 폴더다. 요청은 root 아래 상대경로만 선택한다.
jobdeck TC, DRC, 설정 등 모든 의존 파일도 등록 때 별도로 범위 검증해야 한다.
로컬 `view SOURCE`의 원본 parent 자동 허용을 서버에 가져오지 않는다.

개인 작업은 데이터·캐시와 겹치지 않는 `work_root`에 둔다.
경로는 `users/<namespace hex>/<subject hex>/<dataset id>`다. 인코딩은 경로 이탈과
구분자 충돌을 막고 기존 하위 symlink는 거부한다. preflight는 폴더를 만들지 않는다.
실제 게시 시 재검증·잠금이 필요하며 preflight만으로 TOCTOU 안전한 쓰기 권한이 되지 않는다.

dataset ID는 후속 catalog의 안정적인 레이아웃 논리 ID이며 cache revision이나 세션 ID가
아니다. DRC note/waive는 pack identity에도 바인딩한다. 인덱스 삭제/재빌드가 개인 작업을
삭제하지 않아야 한다. 기존 reviewer별 인접 sidecar는 자동 이동/덮어쓰지 않는다.
새 개인 review store 연결, 다른 사용자 작업의 명시적 읽기 권한은 후속이다.
최초 서비스 정책에서 원본/clip export와 공용 기본값 게시도 닫는다.

## 4. 최초 요청의 공용 인덱싱

1. 등록·범위·Stamp·옵션·캐시 준비 상태를 검사한다. 현재 호환 인덱스는 재사용한다.
2. 없거나 stale이면 canonical source + Stamp + native 버전 + 정규화된 전체 빌드 옵션으로
   작업을 등록한다. 같은 키는 합류하고 개인 신원은 인덱스 키에 넣지 않는다.
3. 서버 소유 runner가 실행한다. 첫 요청자/대기자 종료가 작업을 취소하지 않는다.
4. 실행 전·게시 시 원본 변경을 다시 검사하고 exit 0뿐 아니라 완성·호환성을 확인한다.
   기존 immutable revision/read lease를 연결해 열린 뷰의 캐시를 덮어쓰지 않는다.
5. 각 세션은 완성 revision으로 명시적으로 전환한다. 자동 hot reload를 추가하지 않는다.

현재 `server::indexing::Coordinator`는 **프로세스 내 원장만 구현**했다.
동일 작업 join, bounded queue/history, 같은 원본의 다른 옵션 직렬화, 다른 원본 병렬
admission, foreign receipt 거부를 검사한다. Running receipt를 버려도 슬롯을 해제하지
않아 남은 native 프로세스와 새 작업의 중복을 막는다. runner의 reap/recovery는 남는다.

Stamp는 dev/ino/size/mtime/ns/ctime/ns이고 내용 해시가 아니다. 성공 이력을 freshness로
쓰지 않는다. 다른 서버/CLI의 중복은 source/revision lease로 별도 차단해야 한다.
jobdeck은 의존 OASIS별 작업을 합류시키고 덱 진행률을 합성하는 adapter가 필요하다.

예제는 빌드 1개 × 8 jobs, 세션 4개다. 검사 상한은 `running × jobs ≤ 12`, 원장 256개다.
이는 초기 16-slot에서 뷰 여유를 고려한 **admission 정책**일 뿐 OS CPU/RSS 제한이 아니다.
S1b renderer/decoder는 공용 `Resources`로 세션 admission을 구현했다. 인덱싱 원장의
native runner도 같은 manager에 연결해야 전체 제한이 된다. 아직 그 실행 연결은 없고,
사용자별로 따로 만든 process-local 제한을 합산해도 서버 전체 제한이 되지 않는다.

자동 폴더 감시·자동 삭제는 없다. 회수는 관리자 수동 정책이며 현재
[revision 회수](WEBUI_INDEX_REVISIONS.ko.md)의 로컬 FS 제한을 유지한다(NFS 회수 차단).
열린 인덱스의 임의 삭제는 권하지 않는다. 새 full revision은 기존 파일과 함께 존재하므로
완료 전 디스크 사용량이 추가된다. 원본/개인 작업 삭제는 인덱스 회수 범위가 아니다.

## 5. 로그인 없는 공개 데모

공개 전용 root의 **샘플 ID allowlist**만 받는다. TeeBox 내부 root·세션·권한과 분리한다.
요청에서 파일 경로를 받지 않으며 사전 인덱싱·검증한 샘플의 독립 탐색만 허용한다.
stale/미준비 캐시는 관리자 오류이며 방문자가 인덱싱을 시작하지 않는다.

파일 탐색·review 읽기/쓰기·원본/clip export·공유 기본값·추가 초대는 금지한다.
UI 숨김뿐 아니라 모든 API에 적용한다. 동시 세션·idle·CPU/메모리 admission이 필요하다.
그림 자체는 공개된 것으로 본다. 현행 owner 수명에 묶인 Follow/Explore 초대를 상시
홈페이지 데모로 간주하지 않고 데모 수명을 분리한다. 최초 홈페이지 연결은 새 창/탭
링크이며 기존 frame-ancestors/X-Frame-Options를 풀어 iframe을 허용하지 않는다.

## 6. 현재 가능한 설정 검사

예제: [TeeBox](../tools/web-service/teebox.example.json),
[공개 데모](../tools/web-service/demo.example.json). 복사·편집하고 root들을 사전에
준비해야 한다. 이 예제는 아직 배포 설정이 아니다.

공개 데모 예제의 실행/프록시 설정은 [별도 절차](WEBUI_WEBSITE_DEMO.ko.md)를 따른다.

```sh
cd rust
cargo build --offline --locked --release -j 4 -p floe-app
./target/release/floe2-web server --check-config /absolute/path/service.json
```

version 1 JSON schema, HTTPS origin 표기, 기존 절대경로·root 비중첩, 데모 파일 범위를
검사한다. unknown field/과대 설정 파일/설정 symlink를 거부한다. 출력은 경로·신원·비밀값
없는 요약이며 **`runtime_ready: false`**를 명시한다. 폴더 생성·OASIS 파싱·인덱싱·listener·
브라우저 실행은 없다. TLS 신뢰·소유자/OS 권한·캐시 준비 상태는 검사하지 않는다.
`server`만 실행하거나 `--listen`을 주면 실패한다. 아직 공용 서빙 명령은 없다.

## 7. 구현 순서와 현재 상태

| 단계 | 상태 | 범위 |
|---|---|---|
| S0 정책/전송 기반 | 로컬 구현 | 신원·root·모드 권한·설정 검사·인덱싱 원장·고정 HTTPS origin |
| S1a TeeBox 세션 broker | 로컬 구현 | 별도 위임 proof, 사용자별 1회 접속권/cookie/CSRF, 만료·회수·세션 상한·worker receipt |
| S1b 사용자별 실행/운영 연결 | 코어/전송/기본 UI 구현, 운영 잔여 | 공유 Resources/renderd·revision pin·HTTP/WS·재접속·기본 조작 UI 구현. 프록시/키/운영 CLI·나머지 UI 패널은 남음 |
| S2 인덱스/개인 작업 연결 | 미구현 | 최초 open/native 작업 합류, revision lease, 덱 fanout, 개인 review store |
| S3 Electron 서버 모드 | 미구현 | 안전한 접속권 전달, 고정 origin, 창/복구 namespace, 서버 비소유 lifecycle |
| S4 공유/데모 웹 | 미구현 | 인증 공유와 익명 allowlist demo 분리, API read-only, idle/admission |
| S5 배포/현장 | 미실시 | TLS 프록시·RHEL8/ETX·TeeBox 위임·NFS·다중 사용자/실칩 |

S0의 proxy key는 프록시 경유 증명이지 사용자 로그인/owner 권한이 아니다.
`Gateway::enable_https_proxy` 뒤에도 기존 bootstrap/cookie/CSRF는 필수다.
loopback 동작은 그대로이며 HTTPS cookie는 Secure, WebSocket/CSP는 wss를 쓴다.
키 수명주기·프록시 배포 설정은 아직 없다. 이 기반만으로 공용 서버를 열지 않는다.

배포 검증에는 UI/API/WS 인증, 위조 header 제거, 오배선 upstream의 principal 거부,
계정 전환 시 복구 분리, 만료 후 열린 WS 회수, 교차 cookie/CSRF 거부가 포함된다.
TLS 검증 해제 없이 frame buffering·쓰기 자동 재시도를 피한다.
참고: [Nginx 인증](https://nginx.org/en/docs/http/ngx_http_auth_basic_module.html),
[WebSocket](https://nginx.org/en/docs/http/websocket.html),
[proxy](https://nginx.org/en/docs/http/ngx_http_proxy_module.html).

## 8. 로컬 검증

```sh
cd rust
cargo test --offline --locked -j 4 -p floe-app-core --lib server::
cargo test --offline --locked -j 4 -p floe-app --lib server::
cargo test --offline --locked -j 4 -p floe-app --test server_policy
cargo test --offline --locked -j 4 -p floe-web \
  --lib --test server_broker --test https_proxy --test transport --test permissions_inventory
```

합성 파일로 root/symlink/신원·demo 권한·32 동시 요청 join·다른 옵션 직렬화·receipt
보존·설정 검사의 파일 무변경을 확인한다. `https_proxy`는 실제 TLS가 아닌 loopback
모의 upstream 검사다. 현장 인증·공용 인덱싱 실행·원격 Electron·RHEL/ETX/NFS의
완료 근거로 사용하지 않는다.

2026-09-28 로컬 결과: app-core 단위 367, app 단위 36, 새 `server_policy` CLI 통합 2,
web 단위/HTTPS/transport/권한 inventory 151 passed(기존 ignored 제외).
`sh tools/validate_rust.sh --only app_cli,web_cli_inventory,web_ui`는 합성 valmini로
ALL OK였다. 변경 파일 rustfmt와 `git diff --check` 통과, app-core/app clippy 통과
(기존 tiler/vfs 경고 3건 유지). 검사 중 임시 venv 링크는 종료 시 제거했다.
