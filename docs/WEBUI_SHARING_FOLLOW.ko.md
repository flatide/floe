# M2b-2 — 로컬 읽기 전용 화면 따라보기 전송

아래는 M2b-2 시점 기록이다. 후속 [M2b-3a](WEBUI_SHARING_EXPLORE.ko.md)는 explore의
별도 worker/표시 상태와 `explore.set`을 연결한다. follow의 입력/권한은 그대로다.

`feature/webui`, [공유 경계](WEBUI_M2_SHARING.ko.md) · [초대/인증 코어](WEBUI_SHARING_GRANTS.ko.md).
사용자가 승인한 기본 off·합성/loopback 범위다. **전송 기반 구현이며 사용자용 공유 UI나
독립 탐색 완료가 아니다.** CLI opt-in/초대 전달 UI는 M2b-4에 남긴다. 기존 제품의
`shares=false`는 유지하고 trusted `Gateway::enable_local_sharing`에서만 켠다.

## 구현과 권한

- `GET /api/v1/guest/{id}/events`는 해당 guest cookie와 `floe.v1`, `bundle.{hash}`,
  **`guest-csrf.{secret}`** WS subprotocol을 요구한다. literal-loopback Host와 정확한
  Origin도 필수다. owner의 `csrf.` proof/명령 dispatch는 재사용하지 않는다.
- follow 세션의 `delivery`는 `follow_frames`다. explore는 `not_connected`를 유지하며
  이 endpoint로 연결하면409다. 독립 탐색 요청을 몰래 follow로 바꾸지 않는다.
- 발급 당시 고정한 attachment ID·dataset revision·layer selection을 유지한다.
  owner의 파일 교체, layer selection 변경(축소 포함), view 실패/종료, owner 만료는
  grant를 폐기한다. 새 파일이나 넓어진 layer scope로 자동 연결하지 않는다.
- 현재 snapshot뿐 아니라 **실제 프레임의 dataset revision과 native request.layers**를
  검사한다. 이전 all-layer 프레임이 새 좁은 scope의 권한을 상속하지 않는다.
  프레임의 render key/worker epoch와 foreground revision 또는 margin ID도 확인한다.
- admission, encoder 완료, 실제 socket poll에서 권한을 다시 확인한다. explicit revoke는
  grant mutex 안에서 sink enqueue와 직렬화된다. controller scope는 매 poll/20ms tick에서
  재관찰한다. await 동안 mutex를 보유하지 않는다. 이미 socket에 넘긴 바이트나 수신자가
  저장한 픽셀을 사후 회수할 수 있다는 보장은 하지 않는다.
- `share.hello`와 `share.state`는 별도 allowlist DTO다. state는 카메라/픽셀 크기·revision·
  margin 배치 정보만 전달하며 owner catalog/title/path/minimap/styles/DRC/note를 전달하지 않는다.
  프레임 header도 allowlist이며 `perf`는 제외, `query=false`와 빈 query scene으로 고정한다.
- 입력은 `ping`과 `frame.ack`만 허용한다. owner의 view/set/apply/query/measure/clip 등은
  연결을 종료하며 owner 상태를 변경하지 않는다. seq·epoch·frame ID·disposition을 검증한다.

이 단계의 화면은 native renderer의 geometry·label·frame 픽셀이다. 브라우저가 별도로
얹은 DRC 마커/노트/룰러를 재현하거나 DRC 읽기 권한을 제공하지 않는다. margin은 현재
화면 밖 픽셀도 포함한다. grant는 고정 dataset의 선택 layer 범위이며 현재 viewport 사각형만
공개하는 권한이 아니다. 향후 발급 UI도 이 범위를 명시해야 한다.
native TEXT는 가시 레이어를 따르지만 계층 frame·블록 이름은 renderer의 layer-free
표시 정책을 따른다(`vfs/text.rs`). 따라서 선택 레이어는 계층 정보까지 익명화하는
경계가 아니다. 발급 UI는 이러한 native 주석도 따라보기 픽셀에 포함됨을 안내해야 한다.

## 성능과 수명

새 renderd/데이터 lease/파일 읽기를 만들지 않고 기존 `Arc<DisplayFrame>`의 PNG/raw
바이트를 그대로 전송한다. 카메라 상태를 이미지와 분리해 margin 안의 crop pan도 추가
렌더 없이 따라간다. 재접속은 새 connection epoch에서 현재 유효 foreground/margin을 받는다.

| 항목 | 로컬 follow 한계 |
|---|---|
| 동시 연결 | gateway4개, grant당1개 |
| 전송 예약 | guest 전용256MiB, native payload+packet+WS buffer를 보수적으로3배 회계 |
| packet 복사 worker | guest 전용1개, owner encoder2개와 별도 |
| 이미지 credit | 연결당 실제 write 완료+유효 ACK까지1장, pending은 controller의 최신 프레임 |
| write/ACK/idle | 5초/10초/30초, heartbeat10초 |
| 입력 | 메시지8KiB·초당60개, 현재 owner HTTP의 body/origin 한계도 유지 |

전송 budget은 프로세스 RSS나 서버 전체 quota가 아니다. owner의256MiB 예약·encoder·
socket credit을 게스트가 차지하지 않지만, 픽셀 복사·네트워크·CPU 비용이 0이라는 뜻도 아니다.
대기 중 이미지 큐를 쌓거나 품질을 낮추지 않는다. 자원이 없으면 최신 프레임을 나중에 다시 본다.

게스트는 owner의 subscriber 수를 늘리지 않아 owner 창이 사라진 뒤 기존 idle grace를
연장하지 않는다. revoke/logout/stop은 watch 신호로 전송·복사를 취소한다. blocking 복사는
1MiB마다 취소를 확인하며 실제 종료 전까지 encoder/byte 예약을 보유한다. shutdown은 guest
socket·encoder·byte 회계 수거도 기다린다. grant 폐기 후 대기 바이트를 close flush로 내보내지 않는다.

## 검증과 잔여

집중 검증은 web unit106개(외부 fixture3 ignored), strict all-target/no-deps clippy,
합성 native HTTP/WS13개가 통과했다. 기존9개와 새 follow4개이며 raw/PNG 원본 바이트
일치, owner control 거부, 미ACK guest와 owner 진행, margin-only pan/재접속,
layer scope 변경, owner 종료, 64MiB raw 프레임을 읽지 않는 실제 TCP 연결의 revoke와
자원 수거를 검증한다. blocking 복사 중 취소해도 실제 종료 전까지 credit을 보유하는
경합은 별도 결정적 unit으로 고정했다. 캐시 bytes/mtime과 worker 임시파일 수거도 단언한다.

초기 margin 검사는 카메라 revision 변경 직후 crop 완료 카운터를 읽어 실패했다.
기존 owner gate처럼 완료를 기다리도록 보완했다. 큰 프레임 검사는 테스트 클라이언트의
기본16MiB frame 한계에서 실패해 제품의80MiB packet 계약에 맞췄다. 서버 한계나
제품 timeout을 완화한 것이 아니다. 최초 실패 로그도 보존했다.
최종 집중 로그는 `/private/tmp/floe-share-follow-{unit-final,clippy-final,stream-verified}.log`다.
전체 `sh tools/validate_rust.sh`도 exit0 / `RUST VALIDATION: ALL OK`로 완료됐다.
workspace, owner 서비스20·native stream13·웹 UI/ES2017, occupancy27·jobdeck83·
renderer46, KLayout jobs1/8 각각13 PX+2 phase-exact+14 style을 포함한다.
전체 로그는 `/private/tmp/floe-share-follow-battery.log`다. 기존 native/Pillow/GLib 경고는
남아 있다. 실제 브라우저나 현장 수용을 수행한 것으로 기록하지 않는다.

다음 구현은 독립 explore controller/worker의 공통 admission, 범위 제한 DRC 읽기, 명시
발급·초대 전달·만료·폐기/guest 화면 UI다. 실제 브라우저 쿠키/storage/opener·화면 수용과
Linux/현장 G1~G4도 남는다. 원격 TLS/Origin/서버 전체 자원 정책·G3는 별도 승인/수용이며
이번 loopback 검증으로 대체하지 않는다. 노트 본문·서버 export·게스트 파일 탐색/쓰기는
열지 않는다. SH-01~10 전체나 M2를 완료로 표시하지 않는다.
