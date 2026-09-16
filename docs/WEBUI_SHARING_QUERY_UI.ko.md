# M2b-4b3 — 게스트 조회·룰러와 pointer 탐색

2026-09-17, `feature/webui`. [범위 제한 레이어](WEBUI_SHARING_LAYERS.ko.md)의
후속이며 [M2b-3b1의 native query 계약](WEBUI_SHARING_QUERY.ko.md)을 guest 화면에
연결한다. 기본 off·loopback·별도 guest 인증을 유지한다. 원격 공개, 파일 선택/쓰기,
서버 export, owner 설정/스타일 권한을 추가하지 않는다.

## 표시와 조회의 경계

Explore만 클릭 조회·겹침 순환·Shift 추가/Ctrl 또는 Cmd 토글 선택·snap probe와
개인 룰러를 제공한다. 같은 `query/inspect/measure/rulers` 모듈을 재사용하지만
owner application/인증/dispatcher는 초기화하지 않는다. 별도 adapter가
`view.query`, `view.query.cancel`, `view.measure`, `view.measure_selection`만 해당 `explore.*` 명령으로
매핑한다. 조회가 반환한 레이어 강조는 이미 받은 승인 목록에만 적용하며 새 이름을
조회하거나 가시성을 바꾸지 않는다.

조회 context는 현재 view/연결 epoch·정책·카메라에 맞고 실제 표시 후 ACK한
foreground 또는 **viewport 전체를 포함하는 유효 margin** 하나다. 예전 foreground가
겹친 영역을 그려 준다는 사실만으로 새 strip의 조회 권한이 생기지 않는다. margin은
기존 위상/crop 검사를 통과해야 하며 current state revision으로 조회한다. native도
기존 receipt·scene·승인 가시 레이어 검사를 다시 수행한다.

실제 `object-fit: contain` 이미지 크기/중앙 여백으로 좌표와 overlay를 맞춘다.
`devicePixelRatio`를 그대로 가정하지 않는다. 렌더 이미지, 선택/snap, 명시 공유된
DRC, 룰러 순서로 독립 canvas를 겹친다. DRC 미공유 시 그 버퍼를 할당하지 않는다.
partial/summary 프레임의 geometry 조회 가능 여부는 기존 scene capability에 따른다.
jobdeck의 geometry query는 계속 미지원이고 cursor-only 수동 측정은 가능하다.

## 입력·수명

- 왼쪽/가운데 drag는 화면 preview 후 release에 한 번 pan을 제출한다. 실제 이동과
  단순 mouse-down을 구별하여 클릭 순환을 보존한다. 원위치 복귀 drag는 클릭이 아니다.
- 오른쪽 drag는 box zoom이며 Escape는 제출 없이 취소한다. wheel은 기존 GTK 대조
  정책(이벤트당 최대4%·커서 anchor)을 재사용하고 pending/rendering/final 전 입력은
  쌓지 않는다. 키보드 50%/10% pan과 기존 표시 옵션은 유지한다.
- `r` 수동 룰러, `m` snap, `k/K` 삭제, Escape 단계별 취소와 선택 bbox 간격 측정은
  기존 native 산술을 사용한다. bbox 간격은 실제 contour 최단거리라고 표시하지 않는다.
- ruler 활성화 전에 inspector의 이전 snap을 취소하고, 활성 ruler가 snap slot을
  소유한다. inspector checkbox/leave가 ruler의 요청을 취소하지 않는다. 응답은 현재
  선택된 도구가 아니라 실제 전송 seq 소유자에게만 전달한다.
- 최신 조회/취소·지연 응답 검사와 기존 throttle을 유지한다. JSON이8KiB를 넘거나
  이미 대기 중인 socket byte가16KiB를 넘으면 전송하지 않고 재입력을 요구한다. native 초당60개
  제한도 유지하며 실패 명령/조회는 자동 재생하지 않는다.
- Follow에는 geometry 조회/수동 측정 context 자체가 없다. 끊김·숨김·폐기 시 개인
  선택/룰러와 표시된 상세 문자열, timer/RAF와 overlay buffer를 초기화한다. 별도
  파일·storage에 annotation을 저장하지 않으며 재접속 후 복원하지 않는다.

## 검증

새 pure context/wire 및 실제 inspector/ruler 결합 테스트와 guest DOM/WS 입력 검사를
추가했다. ACK 전 거부, margin의 새 strip, overlap-only 거부, DPR/letterbox, 겹침
순환/다중 선택/bbox 측정, ruler snap 소유권, jobdeck 수동 측정, pan/box/wheel 및
Escape/backpressure, 늦은 응답·disconnect 정리를 검증한다. 레이어 강조는 승인된
기존 행에서만 일어나며 추가 HTTP 요청/가시성 변경이 없음을 단언한다.

실제 guest HTML의 element ID와 script 배선을 사용하며 새 asset은 bundle hash와
release CLI의 byte-exact 응답 검사에 포함한다. 전체 JS/ES2017 회귀는 통과했다.
다중 선택 bbox 측정 테스트에서 reset의 mode callback이 도구를 다시 활성화할 수
있는 재진입을 발견해 막았다. 초기화 도중 콜백이 들어와도 timer/RAF·내용이
되살아나지 않는 회귀를 포함한다.

최종 `sh tools/validate_rust.sh`는 exit0 / `RUST VALIDATION: ALL OK`다.
web117(+외부 fixture3 ignored)·transport15, native owner21·stream20,
release 로컬 공유 CLI와 전체 JS/ES2017, occupancy27·jobdeck83·renderer46,
KLayout jobs1/8 각각13 PX+2 phase-exact+14 style을 포함한다.
로그는 `/private/tmp/floe-guest-query-battery.log`이며 검증 전후 변경 제품/테스트
17개 파일의 SHA-256을 대조했다. 임시 venv 링크는 검증 종료 후 제거했다.
renderer/VFS/vendor·Cargo.lock과 main의 사용자 변경은 건드리지 않았다.

`cargo fmt -p floe-web -- --check`와 web/app/app-core all-target strict clippy
(`--offline --locked -j2 --no-deps -- -D warnings`)도 통과했다.
로그는 `/private/tmp/floe-guest-query-clippy-scoped.log`다. 초기 의존성 포함 clippy는
미변경 `floe-oasis`의 기존 lint13건으로 실패했다
(`/private/tmp/floe-guest-query-clippy.log`). 관련 없는 코드를 고치거나 lint를
허용 처리하지 않았으며 전체 workspace clippy green이라고 주장하지 않는다.
기존 native·Python·GLib/Gdk 경고는 남는다.

DOM/WS harness와 native HTTP는 **실제 브라우저 입력·화면 수용이 아니다**.
기존 시작 파일의 브라우저 접근 차단을 재시도하거나 우회하지 않는다.

## goal까지 남은 범위

공유 UI의 다음 구현은 DRC 순회·marker/box 조작·CD다. SH-08 실제 owner/guest 탭,
복원/storage/opener·시각적 수용, Python-free Linux 및 G1/G4 전체 대조,
TeeBox Firefox/ETX는 남는다. 원격 C/G3에는 별도 운영 정책/권한이 필요하고
M5 world-tile은 실측 조건부다. 사용자 유보 사항인 index hot reload/revision은
이번 범위가 아니다. 이번 UI 연결만으로 `shares=false`를 전체 완료로 바꾸지 않는다.
