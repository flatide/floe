# M2b-1 — 로컬 공유의 초대와 인증 경계

후속 M2b-2는 [읽기 전용 follow 전송](WEBUI_SHARING_FOLLOW.ko.md)을 연결한다.
이 문서 아래의 `not_connected`/전송 미구현은 M2b-1 시점 기록이며, 현재 follow는
`follow_frames`다. M2b-3a의 [독립 렌더 기반](WEBUI_SHARING_EXPLORE.ko.md)에서 explore는
`explore_frames`로 연결했다. CLI/공유 UI·독립 DRC/scoped query는 여전히 남는다.

2026-09-16, `feature/webui`. [설계/수용 기준](WEBUI_M2_SHARING.ko.md)의 첫 구현 단계.
사용자가 승인한 **기본 off, 합성·loopback 우선** 범위다. 원격 공개·노트 본문·
서버 export·게스트 파일 탐색/쓰기는 열지 않는다.

## 구현 범위

이 단계는 초대·인증·폐기 API이며 **아직 화면 공유 기능이 아니다**. 서버의 trusted
설정 `Gateway::enable_local_sharing`으로만 켜며 기존 CLI/GUI는 아직 켜지 않는다.
`shares=false`를 유지하고 별도 `share_grants`로 권한 코어의 opt-in 여부만 표시한다.
`follow`와 `explore` 초대를 구별하지만 두 모드 모두 `delivery=not_connected`다.
화면 스트림·독립 controller/worker·DRC·웹 공유 UI는 후속 단계다.

- owner의 현재 attachment와 dataset revision, 현재 layer selection을 서버에서
  고정한다. 발급 요청은 view ID·state revision·명시 `approve=true`를 요구한다.
  경로, 다른 source ID, 권한 목록, 임의 만료시간은 받지 않는다.
- 초대·접속 합계 최대4개. 초대120초/일회 교환, 접속1800초, owner 만료가 우선이다.
  메모리만 사용하며 파일·native worker·데이터 lease를 새로 만들지 않는다.
- 게스트마다 별도 `Auth`를 두며 owner의 `Auth`에는 넣지 않는다. 공개 share ID,
  초대, cookie, CSRF, session ID는 각각 독립 난수다. owner와 guest의 proof를
  서로 바꿔 넣어도 인증되지 않는다. 비밀값은 최초 발급/교환 응답에만 제공한다.
- 게스트 쿠키와 `X-Floe-Guest-CSRF`는 owner 것과 분리한다. 쿠키 Path/name 구분만을
  권한 경계로 삼지 않는다. owner cookie가 자동 첨부돼도 guest CSRF는 owner proof가
  아니다. 목록에는 비밀값을 다시 내보내지 않는다.
- 모든 guest 요청에서 owner 생존과 현재 scope를 다시 확인한다. 현재 버전은
  layer selection이 조금이라도 달라지면 기존 grant를 폐기한다. owner 파일 교체는
  새 attachment ID여서 자동 따라가지 않는다. 종료/실패한 뷰도 거부한다.
  게스트 로그아웃은 자기 grant만 제거하며 owner 종료는 전부 제거한다.
- DRC는 아직 어떤 읽기도 허용하지 않는다. 따라서 reviewer·waive 변경에 대한
  live 정책/DRC scope는 구현됐다고 주장하지 않는다. 향후 데이터 응답의 전송 직전
  scope 검사, revoke 중 encoder/worker 수명도 별도 게이트다.

## API

모든 요청에 기존 literal-loopback Host/Origin, body 크기/시간 제한, no-store,
no-referrer 정책을 유지한다. URL query에 비밀값을 받지 않는다.

| 경로 | 허용 신원 | 기능 |
|---|---|---|
| `GET/POST /api/v1/shares` | owner | 목록/명시 초대 발급 |
| `DELETE /api/v1/shares/{id}` | 발급 owner | 초대 또는 세션 폐기 |
| `POST /api/v1/guest/{id}/exchange` | 해당 일회 초대 | 별도 cookie+CSRF 교환 |
| `GET/DELETE /api/v1/guest/{id}/session` | 해당 guest | 자신의 최소 상태/로그아웃 |

기존 owner HTTP와 WebSocket은 guest proof를 받아들이지 않는다. guest의 GET도
owner catalog, minimap, note 본문, artifact를 읽는 권한이 아니다. 일반 웹 앱으로
연결하거나 owner sessionStorage를 복구하지 않으며 guest 페이지는 아직 없다.

## 검증과 잔여

`rust/`의 vendored/offline 설정에서 web 단위101개(외부 fixture3 ignored),
transport HTTP15개, strict all-target/no-deps clippy가 통과했다. 공유 단위4개와
진단 세션의 opt-in 거부1개를 포함한다. 합성 native HTTP/WS 스트림9개 중 새 공유
검사는 opt-in·scope·동일 브라우저의 owner cookie 동반을 HTTP로 모사·owner HTTP/WS
거부·guest logout·owner 종료를 검증한다. 기존 owner 서비스20개, 웹 UI/ES2017,
release 빌드와 빈 PATH의 웹 CLI 시작/종료도 통과했다.

`validate_view_stream.py`에 새 결과 marker를 필수로 연결했고 캐시 bytes/mtime
불변과 worker 임시파일 수거도 확인했다. 로그는
`/private/tmp/floe-share-grants-{unit-final,http,clippy-final,stream,owner,ui,build,cli}.log`다.
기존 의존성 경고는 남아 있다. 이 단계는 집중 검증이며 마지막 전체 배터리는
직전 `87f5703`의 [M4 §88](WEBUI_M4.ko.md) 기록이다. 실제 브라우저의 cookie/storage/
opener와 UI 동작을 확인한 것은 아니며, SH-01~10 전체를 통과로 계산하지 않는다.

다음은 follow의 고정 attachment/실제 프레임·개별 전송 credit, explore의 독립
controller/worker·공통 자원 admission, 범위 제한 DRC 읽기, 명시 공유 UI다.
실제 브라우저와 원격 TLS/G3, Linux/현장 수용도 남는다. 기존 index hot reload/
revision 저장소 유보는 유지하며 이 단계에서 해결했다고 표시하지 않는다.
