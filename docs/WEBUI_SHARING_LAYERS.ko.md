# M2b-4b2 — 공유 범위 안의 게스트 레이어 UI

후속 [M2b-4b3](WEBUI_SHARING_QUERY_UI.ko.md)는 query/룰러·pointer를 연결한다.
아래의 남은 UI 설명은 M2b-4b2 시점이다.

2026-09-17, `feature/webui`. [로컬 공유](WEBUI_SHARING_UI.ko.md)와
[개인 DRC 화면](WEBUI_SHARING_DRC_UI.ko.md)의 후속이다. 기본 off·loopback 한계를
그대로 유지하며 원격 공개·파일 탐색/쓰기·스타일/설정 편집 권한을 추가하지 않는다.

## 범위와 그룹

`POST /api/v1/guest/{id}/layers`는 자기 guest cookie/CSRF와 Origin, 고정 view ID,
state revision을 검사한다. Explore worker를 이 요청으로 만들거나 재개하지 않는다.
인증·view 검사 뒤 허용된 실제 plane으로 먼저 투영하고 **그 뒤** 이름·별칭·색·채움과
부모/자식·행 개수를 만든다. owner catalogue를 통째로 보내고 브라우저가 숨기지 않는다.
현재 source/모드가 바뀌면 기존 grant가 폐기되는 계약도 유지한다.

- 일반 레이어: 승인되지 않은 datatype은 이름·개수·부모로도 보내지 않는다. 원래
  최저 datatype이 미승인이면 승인된 최저 datatype을 표시 그룹의 첫 행으로 삼는다.
- 잡덱: 승인된 자식이 있는 level head와 승인된 chip 행만 보여준다. level mode의
  원래 hidden chip 이름은 계속 숨긴다. head의 체크 상태/그룹 변경은 hidden을 포함한
  **승인된 실제 자식만** 대상으로 한다. 미승인 형제의 존재나 개수를 반환하지 않는다.
- synthetic head는 승인 자식 그룹을 켜고 끈다. physical group은 펼치면 해당 행의
  datatype, 접으면 승인된 group 전체를 바꾼다. 접힘은 개인 표시 상태이며 geometry를
  끄지 않는다. 부분 선택은 indeterminate 체크박스로 표시한다.

Explore의 기존 `explore.set` 안에
`layer_visibility:{pair:[layer,datatype],group:bool,visible:bool}`를 추가한다.
서버는 요청한 행/그룹을 scoped catalogue에서 찾고 승인 멤버만 변경한다. 기존
`layers`와 동시 지정은 거부한다. 기존 직접 `layers:only`의 넓은 alias/미승인 plane
거부도 완화하지 않는다. `layers:all`은 계속 모든 **승인 plane**이며 `none`은 전체 숨김이다.
Follow는 목록/개인 접힘만 있고 가시성 변경은 서버에서도 거부한다.

## 수명·비용

한 응답은 최대64행/256KiB, 요청은128KiB, fold exception은4096쌍이다. 가시성 선택은
기존 native 4096 explicit-pair 한계와 All/None 표현을 사용한다. 예를 들어5000개
plane의 All에서 하나만 숨기려면 명시 선택이 상한을 넘으므로 오류이며 몰래 더 숨기거나
권한을 넓히지 않는다. 이 한계를 제거한 무제한 선택 모델은 이번 변경이 아니다.

목록 read는 기존 불변 catalogue와 스타일 snapshot을 사용하며 native decode/render나
파일 I/O를 하지 않는다. 공유 catalogue는 Arc로 보존하고, 투영 중에는 행 index/허용
member key를 임시로 만든다. 전체 행 수에 대한 CPU/임시 메모리 비용은 남으며 O(화면)
또는 상수 비용으로 주장하지 않는다. catalogue 순회/JSON 작성은 shares lock 밖이다.
상태/권한은 응답 생성 후와 실제 body poll 직전에 다시 검사한다. OS에 넘긴 byte를
폐기 시 회수할 수 있다는 보장은 아니다.

브라우저는 동시에 목록 read 하나만 유지하고 view/connection epoch가 바뀌면 취소·초기화한다.
카메라만 움직일 때 목록을 다시 읽지 않는다. 정책 render key 변화, 접힘/페이지 이동,
레이어 변경 완료 또는 명시 Reload에서 갱신한다. pan과 겹쳐 revision read가 거부되면
최신 camera에서 read만 다시 시도한다. 변경 WS 명령은 실패/연결 끊김/결과 불명 때
재전송하지 않는다. 이름은 textContent, 색은 검증된 hex만 사용한다.

## 검증

최종 집중 검증: web117(+외부 fixture3 ignored), 전체 JS/ES2017 회귀와 strict
web/app/app-core clippy가 통과했다. 단위 검사는 미승인 physical head 제거·그룹 재구성,
부분 승인 deck head/hidden 자식·큰 선택 상한·지연 body 폐기를 포함한다. UI 검사는
실제 HTML ID와 guest controller 연결, plain-text 이름, 개인 접힘, group/leaf 명령,
pan 무조회, follow 거부, late/revoke 및 변경 무재전송을 확인한다.

native HTTP/WS20개도 통과했다(`/private/tmp/floe-guest-layers-native.log`). 부분 승인
잡덱 head의 실제 off/on·원래 scope 복원, 단일 승인 layer의 목록/개수, 미승인 키·
다른 guest/owner credential·오래된 view/revision·폐기 후 read 거부를 포함한다.
기존 직접 parent 선택 거부와 All의 scope 제한도 유지한다. native 합성 cache의
bytes/mtime와 worker 수거도 검사했다. release 로컬 공유 CLI도 새 asset bytes,
follow 목록과 미접속 Explore의 worker 비생성, 별도 DRC 동의·권한/폐기를 통과했다.

`sh tools/validate_rust.sh` 전체 재실행이 exit0 / `RUST VALIDATION: ALL OK`로 끝났다.
workspace·owner 서비스21·native stream20·웹 UI/ES2017, occupancy27·jobdeck83·
renderer46과 KLayout jobs1/8 각각13 PX+2 phase-exact+14 style을 포함한다.
전체 로그는 `/private/tmp/floe-guest-layers-battery-recheck.log`, 최종 strict clippy는
`/private/tmp/floe-guest-layers-clippy-final.log`다. 검증 중 제품/테스트 파일 SHA-256을
고정·재확인했으며 renderer/VFS/vendor 코드는 바꾸지 않았다. 기존 의존성·Python·GLib/Gdk
경고는 남는다. 실제 브라우저/스크린샷·Linux·현장 수용을 수행한 것은 아니다.

최초 전체 실행은 기존 `validate_layer_defaults.py`의 native test binary가30초 제한을
넘어 exit1이었다(`/private/tmp/floe-guest-layers-battery.log`). 같은 코드·제한의 단독
재실행에서 테스트 본체0.23초로 통과했고(`/private/tmp/floe-guest-layers-defaults-recheck.log`),
이후 전체 재실행도 통과했다. 재현되지 않은 단발 실패이며 원인을 환경 지연으로
단정하지 않는다. timeout을 늘리거나 검사를 생략하지 않았다.

남은 공유 UI는 displayed receipt 기반 pick/snap·룰러·pointer, DRC 순회/marker/CD다.
SH-08 실제 owner/guest 탭·복원/storage/opener, Python-free Linux·G1/G4,
현장 TeeBox Firefox/ETX는 별도 수용이다. 원격 C/G3는 새 운영 권한/정책이 필요하고
M5는 실측 조건부다. 사용자가 유보한 index hot reload/revision은 건드리지 않는다.
