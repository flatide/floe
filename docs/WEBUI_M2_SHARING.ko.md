# M2b-0 — 읽기 전용 공유의 구현 전 경계 감사

2026-09-16, `feature/webui`의 `f95f890` 기준.
[상위 계획 §4/§8/§9](WEBUI_PLAN.ko.md), [M2 기록](WEBUI_M2.ko.md),
[현재 API와 제안의 구분](WEBUI_SERVICE_API.ko.md).

**이 문서는 초기 코드 대조와 구현 제안이다. 공유 기능 구현/수용 완료가 아니다.**
M2b-0 시점 제품은 `shares=false`, loopback-only이며 owner 인증만 있다. 감사 단계는
런타임·listener·파일 권한을 바꾸지 않는다. 따라보기만 만들고 독립 탐색/원격 수용을
제외한 채 M2를 완료로 재정의하지 않는다. 원래 목표인 배포 C는 그대로 남는다.

이후 사용자 결정(2026-09-16): **기본 off인 opt-in 로컬 공유 구현 진행 승인**.
follow와 explore를 모두 유지하며 먼저 합성 데이터·loopback에서 검증한다. 실제
원격 공개·노트 본문 공개·서버 export·게스트 파일 탐색/쓰기는 열지 않는다.
아래 초기 감사의 승인 대기는 이 범위에 한해 해소됐지만 기능/수용 완료는 아니다.

M2b-1의 초대·인증은 [구현 기록](WEBUI_SHARING_GRANTS.ko.md), M2b-2의 읽기 전용
follow 전송은 [후속 기록](WEBUI_SHARING_FOLLOW.ko.md)을 따른다.
초대/접속 합계4개·초대120초·접속1800초·owner 종료 시 폐기를 초기 로컬
한계로 선택했다. 아래 전체 SH 기준은 권한 코어의 일부 검사만으로 닫지 않는다.

## 1. 현재 코드에서 확인한 선행 변경

| 확인 지점 | 현재 사실 | 공유를 붙이기 전에 필요한 변경 |
|---|---|---|
| [auth.rs](../rust/web/src/auth.rs), `Auth` | bootstrap1개, 활성 `Grant`1개, 인증 결과는 역할 없는 `SessionId` | owner와 guest를 구분하는 principal·scope, 유계 초대/세션 저장소. owner bootstrap을 게스트에게 주거나 새 exchange로 owner를 대체하지 않기 |
| [transport.rs](../rust/web/src/transport.rs), `http_session`·`upgrade` | 정상 인증을 모든 owner route·WS 조작이 공통 사용 | 신원 확인과 명령별 권한 확인 분리. 기존 owner handler가 guest를 정상 owner로 받아들이지 않게 기본 거부 |
| 같은 파일, `active_view`·`Attachment` | gateway의 현재 owner view1개; DRC panel·prepared edit·clip draft도 attachment에 저장 | follow는 지정된 attachment를 고정, explore는 별도 view/controller/worker. 요청마다 현재 owner view로 따라 바인딩하지 않기 |
| 같은 파일, `logout`·`stop_services` | 인증된 로그아웃이 picker/DRC/owner service/worker 전체를 종료 | guest 로그아웃은 자기 세션·연결·독립 worker만 종료. owner 종료와 grant 폐기의 범위를 분리 |
| [stream.rs](../rust/web/src/stream.rs), `Control` | 인증 후 `view.set/apply/fill_slot/query/measure/clip.prepare`를 dispatch | WS handshake뿐 아니라 각 메시지에서 역할·대상·만료·폐기 검증. follow에는 렌더 상태 변경 금지 |
| [owner.rs](../rust/web/src/owner.rs), [DRC HTTP](../rust/web/src/drc/http.rs) | owner catalog는 등록 source 전체; DRC 조회·팔레트 조회 일부는 POST | GET=허용/POST=거부 정책 불가. 공유 dataset/DRC별 투영 DTO와 명시 명령 allowlist 필요 |
| [origin.rs](../rust/web/src/origin.rs) | literal loopback Host/Origin만 허용, 전달 헤더로 origin을 바꾸지 않음 | 원격용 고정 public origin·TLS 종료/프록시 신뢰 정책을 별도로 승인·구현. 현재 검사 완화로 원격을 켜지 않기 |
| [managed.rs](../rust/app-core/src/managed.rs), `Resources` | process-local admission. 기본 CPU16/foreground reserve4/worker2/decoded2048MiB | 한 gateway의 모든 guest가 같은 자원 관리자를 사용. gateway 여러 개의 서버 전체 상한은 별도 운영 설계 필요 |
| [app.js](../rust/web/ui/app.js), `start` | 인증 storage가 origin 기준이며 초기화 때 owner catalog를 읽음 | 역할·세션별 복원, owner와 guest의 쿠키/CSRF/storage 충돌 방지. guest용 catalog/controls·쓰기 receipt 복구 차단 |

현재의 단일 owner 제품에서 권한 우회가 재현됐다는 보고가 아니다. **guest를 같은
인증 성공 경로에 추가했을 때 생길 확장 위험**이다. 기존 8개 socket 상한은 guest
8명·worker8개를 지원한다는 뜻도 아니다.

## 2. 제안하는 권한/데이터 범위

역할 이름과 실제 URI는 아직 wire 계약으로 확정하지 않는다. 아래는 테스트 가능한
최소 권한 구분이다. 권한은 UI 버튼 유무가 아니라 서버 principal에서 검사한다.

| 행위 | owner | follow guest | explore guest |
|---|---|---|---|
| 공유된 view의 이미지·범위 내 DRC 결과/waive 상태 읽기 | 기존 권한 | 허용 제안 | 허용 제안 |
| ACK/ping/자기 재접속 | 허용 | 허용 | 허용 |
| viewport/depth/detail/thin/가시성 변경 | 자기 view | 금지 | 자기 독립 view만 |
| DRC 목록 필터/선택·오류 위치로 이동 | 자기 view | 별도 개인 목록 상태만; owner 선택/goto 금지 | 자기 panel/선택/goto만 |
| pick/snap/룰러 | 기존 capability | 서버 질의는 우선 금지 제안 | 자기 scene의 지원 capability만 |
| 임의 source 열기, 파일 탐색·root 추가, index/force/pack-build | 기존 명시 승인 | 금지 | 금지 |
| note/waive 쓰기·자동 저장·reviewer 재등록·import/transfer | 기존 명시 권한 | 금지 | 금지 |
| defaults 게시·서버 clip/export·artifact 다운로드 | 기존 명시 권한 | 금지 제안 | 금지 제안 |
| grant 발급·권한 확대·타인 연결/worker 취소 | owner 관리 | 금지 | 금지 |
| 자기 guest 로그아웃 | 해당 없음 | 자기 연결만 | 자기 연결/worker만 |

follow의 개인 DRC 목록 상태는 owner attachment의 `drc_panel`을 수정하지 않는다.
explore는 read-only 데이터 권한이지만 자기 표시 상태 변경은 허용한다. 기존
`PatchDto` 전체를 그대로 허용하지 않고 허용 필드/레이어·레벨 범위를 제한한다.
화면의 off 레이어/미선택 레벨도 읽어도 되는지는 권한 문제다. 기본 제안은 발급 시
명시한 레이어/레벨 범위 이내이며 전체 dataset 권한을 자동 추정하지 않는다.
독립 view는 승인된 source binding으로만 생성하고 누락 인덱스를 암묵 생성하지 않는다.

follow는 이미 합성된 owner 프레임에서 금지 레이어를 제거할 수 없다. 발급 범위 밖
레이어/레벨을 owner가 켜면 프레임을 보내기 전에 follow를 중단해야 한다. 원본
프레임을 보낸 뒤 CSS로 감추는 방식은 허용하지 않는다. margin은 viewport 밖 픽셀도
포함하므로 공개 범위가 현재 화면 사각형뿐인 권한으로 해석해서는 안 된다. minimap·
레이어 목록·snapshot·query 응답도 같은 scope에 맞게 투영하거나 명시 거부한다.

노트에는 결과 형상과 별개의 민감한 리뷰 내용이 있을 수 있다. **waive 상태 표시와
note 본문 공개는 분리**한다. 노트 공개·서버 내보내기·게스트 수·TTL은 운영 결정이
필요하다. 이미 전송한 픽셀의 캡처/복사를 막거나 revoke로 회수할 수 있다는 보장은
하지 않는다. 다운로드 버튼을 숨기는 것과 데이터 유출 방지는 같지 않다.

### 수명과 식별자

제안: 초대는 owner가 명시 발급하는 별도 임의 비밀값이며 owner bootstrap/cookie/
CSRF를 복사하지 않는다. 단기·한 번 교환 후 별도 guest cookie+CSRF로 바꾼다.
재접속은 살아 있는 guest 세션만 복원하고 초대 토큰을 재사용하지 않는다. 여러
수신자가 필요하면 독립 초대를 발급한다. TTL·활성 초대/세션 수는 유계로 검증하며
수치와 owner 종료 후 유지 여부는 아직 확정하지 않았다.

grant는 owner ID, 역할, source binding, dataset의 현재 revision 식별, DRC ID/
revision, 허용 레이어/레벨, 만료를 묶는다. index의 불변 revision 저장소가 이미
있다는 가정은 하지 않는다. 현재의 캐시 lease·변경 감지 한계를 그대로 명시한다.
owner가 다른 파일/DRC/reviewer를 열었을 때 기존 guest가 새 대상을 자동으로 보지
않게 한다. 초기 제안은 해당 binding이 바뀌면 grant를 중단하고 재발급을 요구한다.
waive 갱신을 live로 받을지 snapshot으로 고정할지는 별도 선택이며, 그 선택 전에는
일반 hot reload를 구현하거나 지원했다고 표시하지 않는다.

인증 후 실행 대기 중 scope가 바뀌는 경합도 다룬다. admission/작업 시작/응답·프레임
게시 경계에서 grant를 다시 확인하고 revoke는 active read/encoder/WS를 취소한다.
큰 업로드/preparation body를 읽거나 슬롯을 점유하기 전에도 해당 owner 권한을 검사한다.
이미 OS socket에 넘긴 바이트와 수신자 저장물을 되돌릴 수는 없다. 취소 요청만으로
worker·파일 lease·메모리 credit을 먼저 반환하지 않고 실제 작업 종료까지 유지한다.

쿠키 이름·Path 구분만으로 권한 격리를 증명하지 않는다. 쿠키는 포트 격리를 제공하지
않으며 Path도 완전한 보안 경계가 아니다([RFC6265 §8.5](https://www.rfc-editor.org/rfc/rfc6265.html#section-8.5)).
owner/guest를 같은 브라우저에서 여는 경우까지 cookie·CSRF·storage·opener/새로고침
격리를 검증한다. Origin 검사는 유지하지만 비브라우저 클라이언트의 신원 증명으로
쓰지 않는다([RFC6455 §10.1–10.2](https://www.rfc-editor.org/rfc/rfc6455.html#section-10.1)).
브라우저 reload가 owner의 미확인 쓰기 receipt를 guest 권한으로 복구하는 경로도 막는다.

## 3. 성능·자원과 따라보기/독립 탐색

follow는 기존 controller의 프레임을 배포하며 새 renderd를 만들지 않는다. 다만
게스트별 전송 buffer·encoder·socket·ACK/pending credit 비용은 남는다. 느린 guest가
owner의 프레임 진행을 막지 않아야 하며 guest admission에 별도 공정성/owner 여유가
필요하다. 한 개의 전역 semaphore를 공유하는 것만으로 owner 응답성을 보장하지 않는다.

explore는 별도 controller/worker/DRC panel/선택/query receipt가 필요하다. 같은
renderd를 두 view가 공유하면 generation frontier·게시 scene을 서로 바꾸게 된다.
공통 데이터의 안전한 읽기·캐시 lease는 재사용할 수 있으나 view 상태는 공유하지 않는다.

현재 [RenderOptions::local](../rust/app-core/src/render.rs)은 CPU가 충분하면
decode8+raster4, cache1024MiB를 기본으로 잡는다. 두 view를 그대로 복제하면 CPU
예약24가 기본16을 초과한다. DRC/picker 예약도 별도여서 `workers=2`만으로 둘째
view가 들어간다고 보장할 수 없다. guest를 늘리며 매번 새 `Resources`를 만드는
방법은 상한을 우회하므로 금지한다. admission 실패는 명시 busy로 반환하고 품질이나
정확도를 몰래 낮추지 않는다. guest jobs/cache 및 동시 수는 실측과 운영 승인으로 정한다.

여러 gateway/사용자가 같은 서버를 쓰는 경우 process-local 상한의 합은 서버 상한이
아니다. 공통 supervisor/운영 quota 중 무엇으로 통제할지 배포 C 전에 확정한다.
인덱싱의 기존16-thread 목표와 foreground reserve는 보존하되 실제 물리 CPU 수·RSS·
응답 지연의 하드 보장이라고 부르지 않는다. NFS의 blocking I/O 취소 한계도 남는다.

## 4. 구현 순서와 반드시 실패를 주입할 게이트

1. **M2b-1 권한 코어/HTTP·WS 분리**: 승인된 범위에서만 principal/grant·role checks,
   owner-only wrapper와 scoped read를 연결한다. owner 회귀를 먼저 고정한다.
2. **M2b-2 follow**: 고정 attachment·개별 전송 credit·게스트 로그아웃/폐기.
   이것만으로 DRC 독립 탐색이나 M2 전체를 완료 처리하지 않는다.
3. **M2b-3 explore**: dataset-bound 신규 view/worker admission·독립 panel/조회·재접속.
   owner와 guest2개가 서로 다른 DRC 오류를 보는 실제 합성 시나리오로 검증한다.
4. **M2b-4 UI·로컬 통합**: 명시 발급/만료/폐기와 읽기 전용 표시, 위조 메시지 거부.
   loopback 합성 검증은 원격 기능 완료나 실제 브라우저 수용이 아니다.
5. **M2b-5 배포 C**: 승인된 HTTPS/WSS origin/TLS·프록시 신뢰·사용자 접근/URL 전달·
   서버 전체 자원 정책을 구현하고 실제 LAN G3를 측정한다. A/B는 loopback을 유지한다.

아래는 **M2b-0에서 정한 전체 필수 수용 기준**이다. M2b-1의 부분 구현/검증은
별도 구현 기록을 따르며 기존 green 배터리나 권한 코어만으로 전체를 대체하지 않는다.

| ID | 증명할 것 |
|---|---|
| SH-01 | owner/guest 교차 cookie·CSRF, 잘못된 role/범위, 중복 교환, 만료·revoke·권한 확대가 실패; 유효 owner 회귀 불변 |
| SH-02 | HTTP·WS 모든 조작의 권한표 전수 대조. route/Control variant를 새로 추가하면 미분류로 실패. GET catalog/POST read를 각각 검증 |
| SH-03 | source/DRC/view/레이어·레벨 ID 바꿔치기, prepared token·receipt·cursor·artifact ID 재사용이 누출·쓰기·다른 view 변경을 일으키지 않음 |
| SH-04 | owner의 다른 파일/DRC/reviewer·허가 밖 레이어/레벨 전환 중 guest read/렌더 경합; frame/margin/minimap/목록을 포함해 범위 밖 데이터가 기존 grant로 전송되지 않음 |
| SH-05 | follow가 owner viewport/DRC 선택을 변경하지 않음; 느린 ACK/미수신/대형 frame/동시 guest에서도 owner 프레임과 자원 credit 유지 |
| SH-06 | explore 두 개와 owner의 pan/goto/query/선택·cancel·reconnect가 독립적; worker admission 거부/종료/child reap/linger 후 회계 대칭 |
| SH-07 | guest logout/revoke가 owner/다른 guest를 종료하지 않음; owner 종료 시 승인된 cascade 정책; 늦은 HTTP/WS 완료는 권한을 되살리지 않음 |
| SH-08 | 실제 브라우저 동시 owner/guest·새로고침/뒤로가기/복사한 URL·storage·opener; URL/로그/Referrer/에러에 비밀값과 비공유 데이터가 남지 않음 |
| SH-09 | 읽기 전용 DRC 목록/오류 이동/waive 표시·jobdeck 선택 범위·summary 제한을 실제 native 결과와 대조; 기존 파일 bytes·mtime 불변 |
| SH-10 | 승인된 TLS/Origin/proxy 입력 위조·원격 auth/revoke/부하·G3. 인증서/운영 준비 없는 로컬 모형 PASS로 대체하지 않음 |

route 목록 존재만으로 권한 구현을 증명하지 않는다. 미분류를 잡는 inventory와 실제
401/403/대상 불변·파일 불변/프레임 검사를 함께 수행해야 한다. 모든 이미지 read는
권한 있는 데이터 공개이므로 “읽기 전용이라 권한 확대가 아니다”라고 추론하지 않는다.

## 5. 이번 감사 증거와 진행 결정

현행 owner 인증4개·Origin2개 unit을 `cargo test --offline --locked -j2 -p floe-web
--lib auth::tests` 및 `origin::tests`로 재실행해 모두 통과했다. 로그는
`/private/tmp/floe-share-{auth,origin}-audit.log`. **guest 테스트가 아니다.**
제품 코드는 `f95f890`에서 바뀌지 않았고 해당 revision의 전체 배터리 기록은
[M4 §87](WEBUI_M4.ko.md)에 있다. 이 문서 변경을 위해 전체 제품 배터리를 재실행한
것으로 보고하지 않는다.

현재 host는 Darwin arm64이며 PATH에서 docker/podman/colima/limactl/
qemu-x86_64/qemu-system-x86_64를 찾지 못했다. 다른 경로에 설치가 없다는 증명은
아니지만 지금 Linux 실행 수용의 근거도 없다. 도구 설치·외부 서버 접근은 하지 않았다.
승인 뒤에도 거부됐던 브라우저 시작 경로는 재시도/우회하지 않았다. TeeBox도 사용자가
아직 실행할 수 없다고 했으므로 같은 현장 측정을 재요청하지 않는다.

**읽기 공유 권한 추가 범위**는 위 사용자 결정으로 승인됐다. 기본 off인 opt-in
공유를 합성/loopback에서 먼저 구현하되 follow와 explore를 모두 계획에 유지하고,
노트 본문·서버 export·파일 탐색/쓰기·실제 원격 공개는 열지 않는다. 활성 토큰
전달 방식·TTL/동시 수·owner 종료/review 갱신 정책은 구현 전에 확정한다. 원격 서비스의
TLS/접근·전체 서버 자원 정책은 별도 단계 승인이다. 승인 근거는 문서 자체가 아니라
사용자의 명시 선택이며 전체 goal은 아직 완료가 아니다.
