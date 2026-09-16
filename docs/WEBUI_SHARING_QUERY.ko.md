# M2b-3b1 — 독립 게스트의 scoped query와 룰러

`feature/webui`, [독립 렌더 기반](WEBUI_SHARING_EXPLORE.ko.md) 다음 단계.
사용자가 승인한 **기본 off·합성/loopback** 범위다. 독립 DRC panel/선택/읽기(M2b-3b2),
CLI opt-in·발급/guest UI(M2b-4)는 아직 남는다. `shares=false`를 유지한다.

## 구현 계약

- 기존 guest WS에서 explore만 `explore.query`, `explore.query.cancel`,
  `explore.measure`, `explore.measure_selection`을 허용한다. own view ID·연결 epoch·
  증가하는 seq가 필요하다. follow는 유효한 DTO로 보내도 연결을 닫고 owner에 dispatch하지
  않는다. owner용 명령/clip/export/filesystem/DRC/note 권한을 추가하지 않는다.
- query는 독립 native worker의 pick/snap이다. 결과 DTO는 기존 유계 변환을 재사용하며
  raw 진단/경로를 보내지 않는다. owner나 다른 guest의 controller/query ticket을 쓰거나
  취소하지 않는다. 종류별 최신 ticket 두 개만 보관한다.
- **자기 연결에 전송 완료되고 `displayed` ACK된 프레임만 조회 권한**이 된다.
  foreground/margin별 최신 receipt 하나씩이며 픽셀/scene 이력을 더 보관하지 않는다.
  `discarded`, ACK 전, 다른 view의 receipt, 재접속 전 epoch는 조회 권한이 아니다.
  기존 native scene 및 dataset/worker/frame/state/render revision 검증도 모두 통과해야 한다.
- 유효한 margin receipt는 16px 위상을 보존하는 crop pan 뒤에도 현재 카메라/revision으로
  조회할 수 있다. 이전 foreground anchor는 stale이다. 새 foreground ACK는 이전 receipt를
  대체한다. 60초 재접속 grace가 view/state를 보존해도 연결의 receipt·snap 결과는 복원하지 않는다.
- query의 `layers:all`은 **현재 보이는 허용 subset**이다(표시 편집의 all=granted와 구분).
  jobdeck parent alias를 먼저 정규화하고 grant subset을 검사한다. 좁은 grant에서 허가 밖
  기존 레이어와 존재하지 않는 레이어는 모두 `forbidden`으로 응답해 코드로 존재를 구분하지 않는다.
  결과 request layer와 explicit pick layer도 grant/current visible/request subset인지 검사한다.
  snap에는 layer 필드가 없으므로 native scene/request 검증에 의존한다.
- 결과가 범위를 벗어나면 hit 없는 `query_failed`만 보내고 해당 ticket을 취소한다.
  실패 envelope 전송이 원래 native snap을 룰러 권한으로 바꾸면 안 된다.
- 수동 룰러·최대64개 선택 bbox 측정은 현재 표시 receipt에 고정된 Rust 산술이다.
  snap을 지정하면 같은 연결에서 이미 보낸 성공 결과만 허용하고, stale/cancelled snap을
  조용히 자유 좌표로 대체하지 않는다. 선택 bbox는 클라이언트 annotation 좌표이며 새로운
  source 읽기를 하지 않는다. 저장·공유 annotation·서버 export 기능이 아니다.

## capability와 비용

Explore header는 실제 native `query`/`query_scene`을 보낸다. follow는 계속 query=false와
빈 scene이다. hello의 query는 일반 레이아웃 여부를 알릴 뿐, 개별 프레임의 summary/partial
또는 stale scene 제약을 완화하지 않는다. jobdeck geometry query는 현재 native 계약대로
지원하지 않으며 수동 룰러 같은 geometry-independent 산술은 허용한다.

query 결과는 이미지 ACK credit 대기 중에도 전송한다. 한 프레임 credit·별도 guest 복사/
전송 admission은 그대로다. 입력8KiB/초당60개, 출력 control256KiB를 유지하며 종류별 최신
결과만 전달한다. 이는 공정한 운영 응답 시간 보장이 아니다. 모든 실제 socket write에서
grant 유효성을 재검사하고, revoke/logout/scope 변경/연결 종료 후 늦은 결과를 보내지 않는다.
이미 OS에 전달된 바이트의 회수를 보장하지 않는다.

## 검증

web 단위109개(외부 fixture3 ignored), web/app-core strict all-target/no-deps clippy,
웹 crate의 fmt check, 합성 native HTTP/WS20개가 통과했다. 기존17개와 query/룰러3개다.
native gate는 private valmini 복사본만 사용하며 cache bytes/mtime 불변과 worker 임시파일
수거를 단언한다. 전체 `sh tools/validate_rust.sh`도 exit0 / `RUST VALIDATION: ALL OK`로
완료됐다. workspace·owner 서비스20·native stream20·웹 UI/ES2017, occupancy27·jobdeck83·
renderer46, KLayout jobs1/8 각각13 PX+2 phase-exact+14 style 검사를 포함한다.
전체 로그: `/private/tmp/floe-share-query-battery.log`. 실제 브라우저·Linux·현장 수용을
실행한 것은 아니며 기존 native/Python/GLib 경고는 남아 있다.

- raw/PNG owner+두 독립 guest의 실제 pick/snap·18µm 수동 룰러·2µm bbox 간격.
- ACK 전·owner/다른 guest receipt 위조·범위 밖/unknown layer·None에서 All 조회 거부/무hit.
- 이미지 ACK 대기 중 query·margin crop·취소·discard·재접속/epoch·폐기/회수.
- follow에 네 명령 모두 유효한 DTO로 보내도 owner query 수/상태가 바뀌지 않음.
- 결과 subset 단위 검사와 기존 owner query/룰러 회귀. 실패 시 대형 binary payload를
  테스트 로그에 덤프하지 않고 길이만 기록한다.

초기 테스트의 두 계약 가정을 수정했다. 새 foreground ACK 후 옛 프레임은 stale 검사 전에
`frame_not_displayed`이고, margin 재사용은 실제 커서키와 같은 `snap:true` pan이 필요하다.
비정렬 pan은 올바르게 새 렌더를 발생시킨다. 제품의 권한/상한을 완화하지 않았다.
초기 실패 로그: `/private/tmp/floe-share-query-stream{,2}.log`.
집중 최종 로그: `/private/tmp/floe-share-query-stream3.log`,
`/private/tmp/floe-share-query-{unit-verified,clippy-verified,fmt}.log`.
`cargo fmt --all --check`에는 기존 cli/render-core/vfs 등의 포맷 차이가 있어 전체 green으로
기록하지 않는다. 관련 없는 파일을 포맷 변경하지 않고 `cargo fmt -p floe-web -- --check`로
이번 crate를 검증했다. 기존 의존성 경고도 남아 있다.

## goal까지 남은 범위

다음은 M2b-3b2의 명시 DRC scope·독립 panel/필터/선택·읽기와 SH-06의 **owner+guest2가
서로 다른 DRC 오류를 보는 시나리오**다. 이어 M2b-4의 CLI opt-in·발급/전달/만료/폐기·
guest UI가 필요하다. geometry query 통과로 DRC/공유 전체를 완료 처리하지 않는다.
실제 브라우저 cookie/storage/opener·화면 수용, Linux/현장 G1~G4, 별도 원격 배포 C/G3,
유보된 index hot reload/revision 정책 및 조건부 M5는 별도로 남는다.
