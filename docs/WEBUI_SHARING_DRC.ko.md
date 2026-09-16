# M2b-3b2 — 명시 DRC 공유와 독립 목록·선택 API

후속 [M2b-4a 로컬 공유 UI](WEBUI_SHARING_UI.ko.md)는 layout 초대·기본 프레임 탐색을
연결했다. 이 문서의 DRC 공개 승인/목록·overlay UI는 다음 M2b-4b에 남는다.

`feature/webui`, [공유 경계](WEBUI_M2_SHARING.ko.md)와
[독립 query/룰러](WEBUI_SHARING_QUERY.ko.md)의 후속 단계. 기본 off·합성/loopback만이며
**아직 사용자용 공유 UI/CLI나 원격 공유 제품 완료가 아니다.** `shares=false`를 유지한다.

## 공개 범위

owner가 초대에 `drc:{id,revision,approve:true}`를 명시해야 DRC 읽기가 허용된다.
생략/null은 DRC 권한 없음이다. 현재 등록된 reader·revision·열린 source가 일치하고
reader가 ready여야 발급된다. 다른 ID/revision/승인 false는 거부한다. 초대 결과,
owner 초대 목록, guest session 복원에는 허가된 DRC의 **ID/revision만** 덧붙인다.

이 허가는 **해당 DRC 결과 전체의 규칙 이름·설명, 오류 좌표, waive 상태**에 대한 읽기다.
geometry의 가시 레이어에서 DRC 공개를 추론하거나 DRC가 그 레이어만의 오류라고 가정하지
않는다. 다음 UI의 승인 화면은 이 차이와 전체 결과 공개를 명시해야 한다. source 경로/title,
reviewer·note 본문·리뷰 쓰기 허가·등록/빌드 카탈로그·별도 SVRF 파일은 공개하지 않는다.
규칙 설명은 결과 자체의 텍스트이며 review note와 다르다.

DRC reader 교체·review/waive 적용 등으로 고정 read revision이 바뀌면 기존 grant 전체를
폐기한다. 새 DRC/reviewer를 자동으로 따라가거나 살아 있는 waive 갱신을 구독하지 않는다.
라이브 갱신 정책이 정해지기 전의 보수적 동작이다. 일반 레이아웃 공유에 DRC를 붙이지
않았다면 DRC 변경만으로 읽기 권한이 생기지 않는다. index hot reload/revision 저장소
정책은 여전히 별도 유보이며 이 기능으로 해결했다고 표시하지 않는다.

## 실제 API

모두 `/api/v1/guest/{share_id}` 아래이며 별도 guest cookie와 `X-Floe-Guest-CSRF`,
기존 loopback Host/Origin 검사를 사용한다. owner 인증이나 owner handler로 폴백하지 않는다.

| 경로 | 의미 |
|---|---|
| `GET /drc` | 고정 결과의 count/precision/format·불완전 record 수만 반환 |
| `POST /drc/read` | 규칙/오류 목록, 좌표, CD, 선택·공간 필터, 순회, focus 좌표 준비 |
| `GET/POST /drc/panel` | 이 guest만의 목록 필터·현재 오류·표시 옵션, panel revision CAS |
| `GET/POST /drc/selection` | 이 guest만의 오류 선택 집합, selection revision CAS |

쓰기처럼 보이는 panel/selection POST도 메모리상 탐색 상태뿐이다. source/캐시/DRC/sidecar를
쓰지 않는다. request의 view ID·DRC revision이 현재 grant/독립 view와 달라지면 409다.
read의 focus/InView/현재 뷰 필터에는 `state_rev`가 필수이며, 지정했다면 응답까지 카메라
revision이 같아야 한다. selected-only 조회는 별도 선택 revision도 다시 검사한다.

응답은 `{view_id,revision,data}`다. DRC read 종류는 exhaustive match로 분류하고 출력
필드는 중첩 row까지 allowlist로 투영한다. owner DTO에 필드가 늘어도 note/reviewer/SVRF가
자동 공개되지 않는다. 이번 DRC 허가는 별도 SVRF 등록에 대한 허가가 아니므로 Types/
Comparison/metric 필터·layer isolate는 403이다. 노트·waive 쓰기·export/import·파일 경로·
새 결과 선택·pack-build 명령도 없다. 미래 게스트 SVRF 지원은 별도 공개 계약이 필요하다.

follow와 explore 모두 개인 패널을 가진다. follow의 focus 조회는 좌표를 반환할 뿐이며
owner 카메라/선택을 변경하는 권한은 없다. explore는 준비된 goto를 기존 `explore.set`으로
자기 view에 적용한다. DRC geometry를 위한 새 render worker는 만들지 않고 기존 고정
reader를 재사용한다. HTTP 요청만으로 explore worker를 새로 열지는 않으며 WS 연결에서
기존 자원 admission을 통과해야 한다.

패널/선택은 grant에 속한다. WS 재접속은 같은 패널을 복원하고, 기존 60초 idle 회수로
worker가 바뀌어도 허가가 살아 있다면 개인 목록 상태는 유지된다. 새 view ID는 이전 HTTP
대상을 대체하므로 옛 view의 늦은 응답은 거부한다. revoke/logout/TTL/owner 종료 시 패널도
폐기된다. owner Attachment의 패널이나 다른 guest와 공유하지 않는다.

## 비용·경합·게시

- gateway 전체 guest DRC actor 작업은 queued+active 합계 1개다. 기존 owner queue 4를
  guest만으로 채우지 않는다. 초과는 429이고 자동 품질 저하/부분 성공은 없다.
- reservation은 HTTP waiter가 아니라 실제 Work가 소유한다. 취소/timeout으로 waiter가
  사라져도 작업이 실제 끝나기 전에는 한도를 반환하지 않는다. native 파일 I/O는 기존
  actor에서 실행하며 HTTP reactor에서 하지 않는다.
- 입력은 기존 16KiB, 결과는 envelope 포함 1MiB, actor 대기는 최대 30초 및 외부 HTTP
  timeout을 따른다. 기존 read scan/cursor 상한, 선택 입력 64개·총 5000개를 재사용한다.
- 제출은 shares→registry 순서로 고정 허가와 직렬화하고, enqueue에서 expected revision을
  capture한다. 게시/패널 CAS는 shares→registry→read revision→panel/controller 순서다.
  panel guard를 잡은 채 enqueue하지 않는다. mutex를 await/native I/O 동안 유지하지 않는다.
- revoke/종료/주기적 유효성 검사는 대기 ticket을 취소한다. handler 완료 시뿐 아니라 실제
  HTTP body poll에서도 grant/view/revision을 다시 검사한다. 그 전에 폐기되면 body를
  보내지 않는다. 이미 hyper/OS에 넘긴 유계 chunk나 수신자 복사본을 회수하지는 못한다.

이 제한은 한 gateway의 admission이며 여러 프로세스 서버 전체 quota나 owner 지연/RSS
보장이 아니다. 장시간 NFS read의 즉시 종료도 보장하지 않는다. body response 상한과
기존 HTTP 연결/idle 제한은 별개다.

## 검증과 잔여

web 단위 113개(외부 fixture 3 ignored)와 web/app-core strict all-target/no-deps clippy,
합성 owner native 21개 및 기존 공유/탐색 stream 20개가 통과했다. native 21에는
owner+guest2가 서로 다른 DRC 오류를 선택·조회·goto하고 서로 다른 실제 PNG frame
카메라를 받는 시나리오가 있다. owner panel/state는 불변이며 follow는 별도 개인 패널이다.
명시 허가 부재·잘못된 ID/revision/승인·교차 guest·옛 panel/selection CAS·미허가 읽기를
거부하고, 한 guest 폐기가 다른 guest를 종료하지 않으며, 같은 reader의 waive revision
전환은 기존 DRC grant를 폐기한다. source/index/DRC 파일 bytes·mtime과 종료 자원 수거도
검사한다. 신설 body-poll gate는 handler 이후 폐기, projection gate는 중첩 note/미정의
필드, actor gate는 취소 후 실제 종료 전 permit 유지와 남은 owner queue 슬롯을 고정한다.

강화한 native 검사는 DRC 허가 없는 guest의 여섯 method/path 조합을 모두 거부하고,
WS 재접속에서 동일 view ID·새 connection epoch와 개인 panel/선택 복원을 확인했다.
로그: `/private/tmp/floe-share-drc-native-final.log`,
`/private/tmp/floe-share-drc-stream.log`.
전체 `sh tools/validate_rust.sh`도 exit 0 / `RUST VALIDATION: ALL OK`로 종료했다.
로그는 `/private/tmp/floe-share-drc-battery.log`이며 owner native 21개·공유 stream 20개,
occupancy 27개·jobdeck 83개·renderer 46개, KLayout jobs 1/8 각각
13 PX + 2 phase-exact + 14 style 검사가 통과했다. 검증 전후 구현/validator 파일의
SHA-256도 동일하다. 실제 브라우저에서 DRC overlay/목록을 조작한 것은 아니므로
SH-06의 native 경로와 SH-08 UI 수용을 구별한다.

다음 M2b-4는 CLI opt-in, 발급/승인/전달/만료/폐기 UI, guest 화면과 개인 DRC 목록·마커·
goto·레이어/룰러 연결이다. 테스트용 API를 일반 사용자가 쓸 수 있는 공유 기능으로 바꾸는
단계가 남아 있다. 실제 브라우저 cookie/storage/opener·화면/다운로드, Python-free Linux,
현장 G1~G4, 별도 승인/운영 정책이 필요한 원격 배포 C/G3, 조건부 M5는 별도다.
