# M2b-3a — 로컬 게스트의 독립 렌더와 표시 상태

후속 M2b-3b1은 [scoped query/룰러](WEBUI_SHARING_QUERY.ko.md)를 연결한다. 아래의
query=false/미구현 설명은 M2b-3a 시점 기록이다. 현재 explore는 실제 native query scene을
전달하고, follow는 계속 query=false다. 독립 DRC panel과 공유 UI는 여전히 남는다.

`feature/webui`, [공유 경계](WEBUI_M2_SHARING.ko.md) · [follow 전송](WEBUI_SHARING_FOLLOW.ko.md).
사용자가 승인한 기본 off·합성/loopback 범위다. **독립 렌더 기반이지 M2b-3 전체나
사용자용 공유 기능 완료가 아니다.** DRC panel/범위 제한 읽기·pick/snap/룰러는 후속
M2b-3b, CLI opt-in·발급/guest UI는 M2b-4에 남는다. `shares=false`는 유지한다.

## 구현과 권한

- 기존 guest 인증 endpoint에서 explore의 `delivery`가 `explore_frames`가 된다.
  WS 주소·별도 guest cookie/CSRF·Origin·protocol/bundle 검증은 follow와 동일하다.
  요청을 owner follow로 바꾸지 않는다.
- 처음 WS가 승인될 때 `ViewController::fork_view`로 별도 view ID·controller·native
  worker·generation·프레임 캐시를 만든다. 고정 dataset lease와 불변 Model은 재사용하며
  owner의 worker를 취소/교체하거나 query scene을 공유하지 않는다. 독립 query API는 아직 없다.
- **owner와 같은 `Resources`**에서 새 permit을 예약한다. 새 자원 관리자를 만들어 상한을
  우회하거나 replacement 예약을 재사용하지 않는다. opening/render/close 중에도 실제 native
  worker 종료까지 예약을 유지한다. 실패하면 HTTP429(busy) 또는503이며 품질을 낮추지 않는다.
- 초기 표시 상태는 해당 owner의 현재 상태다. 이후 navigation/pixels/depth/depth_step/
  detail/thin/layers/frames/labels/font_px/mono만 `explore.set`으로 변경한다. 별도 view ID,
  connection epoch, 증가하는 seq와 base_state_rev CAS가 필요하다. 상대 좌표 연산은 기존
  Rust view 경로를 쓴다. owner용 `view.set`이나 스타일/설정/쓰기 DTO를 재사용하지 않는다.
- `layers:all`은 **허용된 레이어 전체**다. jobdeck parent를 먼저 실제 자식 plane으로
  정규화한 후 subset을 검사해 상위 그룹 선택으로 범위가 넓어지지 않게 한다. frame도 실제
  native request.layers의 subset과 dataset revision을 검사한다. owner가 레이어 선택을
  바꾸면(축소 포함) 현재 계약대로 grant 전체를 폐기한다.
- `share.state`는 카메라/revision/margin에 독립 view의 표시 설정·DBU·안전한 오류 코드만
  더한다. owner 경로/title/catalog/minimap/styles/DRC/note 및 raw 오류 메시지는 보내지 않는다.
  header의 `query=false`/빈 query scene을 유지한다. geometry·TEXT·계층 frame/블록 이름은
  native 픽셀에 포함될 수 있다. 레이어 제한이 layer-free 계층 주석의 익명화는 아니다.
- 새 파일/미선택 레벨 열기, 인덱싱, 임의 파일 읽기/쓰기, reviewer 변경, 노트 본문,
  서버 export/clip, 다른 view 취소는 허용하지 않는다. DRC 허가는 이번 단계에 추가하지 않는다.

## 자원과 수명

로컬 실험값은 explore당 decode1+raster1, decoded128MiB다. owner의 tile/raw/round/font
등 trusted native 설정과 margin/cache 정책은 상속한다. 브라우저가 jobs/budget/native
경로를 지정할 수 없다. 기본 worker 한도2에서는 owner+explore1개가 가능하나 다른 서비스의
예약이 있으면 그보다 먼저 busy일 수 있다. 두 게스트 검증은 테스트에만 worker 한도3을
명시했다. 이 값은 운영 서버의 처리량/지연/RSS 보장이 아니며 실제 부하는 별도 측정한다.

기존 guest 전용 전송256MiB·복사 worker1·socket4/grant당1·한 프레임 write+ACK credit을
그대로 쓴다. follow와 explore가 이 전송 budget을 공유하되 owner의 전송 credit과는 별도다.
native worker와 retained bitmap 등은 전송 budget과 다른 회계이며 process-local 예약은
프로세스 RSS나 여러 사용자 서버 전체 quota가 아니다.

연결이 끊기면60초간 동일 view로 재접속할 수 있다(connection epoch만 새 값). 이후
250ms 유지관리에서 표시 상태만 저장하고 worker를 닫는다. 세션 TTL1800초 이내 다시
연결하면 저장한 카메라/표시 설정으로 새 view ID·worker를 예약한다. 그 사이 자원이 없으면
busy이며 기존 grant/저장 상태는 보존한다. 초대 만료120초·세션 만료·revoke·logout·owner
scope 변경/종료는60초 grace보다 우선한다. guest는 owner subscriber 수를 늘리지 않는다.

HTTP 처리 중 살아 있는 controller의 마지막 Arc를 버리면 Drop이 thread join을 하므로,
폐기된 controller는 별도 retired 목록에 보관하고 종료 확인 후 해제한다. shutdown은
socket/encoder/byte credit뿐 아니라 독립 worker 종료도 기다린다. 취소 요청을 예약 반환으로
간주하지 않는다. 이미 전송한 픽셀의 회수나 NFS blocking I/O 즉시 취소를 보장하지 않는다.

## 검증과 잔여

web 단위108개(외부 fixture3 ignored), web/app-core strict all-target/no-deps clippy와
합성 native HTTP/WS17개가 통과했다. 기존13개와 독립 탐색4개다.
게이트는 owner+두 독립 view의 raw/PNG,
서로 다른 카메라/worker/CAS·재접속, 공통 자원 busy/retry, 레이어 subset과 jobdeck parent
정규화, 위조 ID/epoch/owner 명령/null/미정의 필드 거부, 실제60초 idle 회수·상태 복원,
owner scope 폐기와 전체 종료를 검사한다. private valmini 복사본만 쓰며 cache bytes/mtime과
worker 임시파일 수거를 단언한다. 테스트를 위해 제품의60초 한계를 줄이지 않는다.

잡덱 fixture의 초기 가정 두 가지를 고쳤다. 같은 OASIS를 두 번 배치해도 source별 칩
항목은 하나이므로 독립 합성 소스 두 개를 사용한다. 또 native 잡덱은 query/scene.complete가
false이므로 일반 레이아웃의 query=true 헬퍼를 별도 인자로 구분했다. 제품의 권한/timeout을
완화하거나 assertion을 삭제하지 않았다. 초기 실패 로그는
`/private/tmp/floe-share-explore-stream-{final,verified,verified2}.log`에 보존했다.
최종17개 통과는 `/private/tmp/floe-share-explore-battery.log`의 native stream 구간에 있다.
정적/단위 로그는 `/private/tmp/floe-share-explore-{clippy-verified,unit-final}.log`다.
전체 `sh tools/validate_rust.sh`도 exit0 / `RUST VALIDATION: ALL OK`로 완료됐다.
workspace·owner 서비스20·native stream17·웹 UI/ES2017, occupancy27·jobdeck83·renderer46,
KLayout jobs1/8 각각13 PX+2 phase-exact+14 style이 포함된다.
전체 로그는 `/private/tmp/floe-share-explore-battery.log`다. 기존 native/Pillow/GLib 경고는
남아 있으며 실제 브라우저·Linux·현장 수용을 수행한 것으로 기록하지 않는다.

남음: M2b-3b의 독립 DRC panel/선택/읽기 및 scoped query/룰러, M2b-4의 명시 발급·전달·
만료·폐기/guest 화면 UI. SH-06의 **owner+guest2가 서로 다른 DRC 오류를 보는 시나리오**는
독립 geometry navigation만으로 닫지 않는다. 실제 브라우저 cookie/storage/opener·화면 수용,
Linux/현장 G1~G4, 별도 승인/수용이 필요한 원격 배포 C·G3도 남는다. index hot reload/revision
정책과 조건부 M5는 별도다.
