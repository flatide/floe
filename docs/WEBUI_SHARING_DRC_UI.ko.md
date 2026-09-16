# M2b-4b1 — 게스트 DRC 읽기 화면

2026-09-17, `feature/webui`. [로컬 초대/프레임 UI](WEBUI_SHARING_UI.ko.md)에
[고정 결과 공유 API](WEBUI_SHARING_DRC.ko.md)를 연결한다. 기본 off·loopback-only이며
원격 공개나 전체 공유 UI 완료가 아니다. 게스트 레이어·pick/snap·룰러·포인터 조작과
실제 브라우저/현장 수용은 계속 남는다.

## 초대의 별도 승인

`--local-sharing`으로 연 workspace의 **Share locally…**에서 현재 source에 연결된
ready DRC 결과만 추가로 보여준다. 결과 이름/규칙 수/오류 수를 owner에게 표시하며,
체크박스는 열 때마다 해제한다. layout 승인만 하면 초대에 `drc`를 보내지 않는다.

별도 체크는 **그 결과 전체**의 규칙 이름·설명, 오류 좌표와 waive 상태 공개를
승인한다. 현재 가시 레이어나 viewport 안 오류만이라는 뜻이 아니다. 발급에는
`drc:{id,revision,approve:true}`만 넣고 서버가 현재 source/reader/revision을 다시
검사한다. 준비 중이거나 다른 source의 결과에는 체크를 제공하지 않는다. 확인 실패
시 layout-only 초대는 가능하고, DRC 권한을 임의 추론하지 않는다.

노트 본문·별도 SVRF metadata·reviewer·파일 탐색·index·review 쓰기·서버 export는
이 화면에서도 공개하지 않는다. title은 owner 승인 설명일 뿐 guest 응답에 추가하지
않았다. DRC/review revision 변경은 기존 계약대로 grant 전체를 폐기하며 자동으로
새 결과/리뷰를 따라가지 않는다. 초대 실패/불명확 응답은 재전송하지 않는다.

## 게스트의 개인 패널

전용 `guest-drc.js`는 owner `drc.js` controller를 초기화하지 않는다. HTTP는 자기
grant 아래 `/drc`, `/drc/read`, `/drc/panel`, `/drc/selection`만 사용한다. UI helper도
method/path를 allowlist로 제한하고, 실제 권한 판단은 기존 서버가 수행한다. DRC
허가가 없거나 WS의 view admission/state가 아직 없으면 DRC 요청을 보내지 않는다.

연결한 조작:

- 규칙 이름 필터·규칙/오류 페이지, 규칙 설명, active/waived 필터.
- 현재 규칙의 Current view·Selected only 교집합, marker 표시 토글.
- 클릭 replace·Shift add·Ctrl/Command toggle과 전체 선택 해제. 기존 서버의
  규칙별 선택 의미/5,000개 상한을 사용하고 owner/다른 guest 선택은 바꾸지 않는다.
- 현재 페이지 오류 marker와 선택 오류의 실제 ICE/ASCII 윤곽. u64 ID는 문자열로
  유지하고 ASCII 소수 좌표를 DBU 정수로 반올림하지 않는다. p는 닫힌 polygon,
  e는 독립 edge 쌍이며 윤곽을 다 받기 전에는 bbox preview만 표시한다.
- explore의 **Go · same scale / Frame error**는 Rust가 만든 navigation을 사용한다.
  현재 view/epoch/state가 그대로이고 pending edit가 없을 때 자기 view에 한 번
  적용한다. `isolate:false`이며 레이어 공개 범위를 넓히거나 owner를 이동시키지 않는다.
  follow는 개인 목록/선택/marker만 있고 두 이동 버튼은 비활성이다.

panel과 selection은 grant 메모리에 보존한다. 재접속 때 서버 값을 복원하되 이전
이동이나 선택 변경을 다시 보내지 않는다. 접속 epoch, 독립 view ID 또는 고정 DRC
identity가 달라지면 진행 중 read를 취소하고 이전 rows/outline을 버린다.
노트/waive 자동 저장 opt-in과 무관하며, 여기의 panel POST는 파일 쓰기가 아니다.

선택/패널 변경 응답이 불명확하거나 CAS가 충돌하면 조작을 멈추고 **Reload review**로
서버 상태를 다시 읽게 한다. 변경 명령을 자동 재전송하지 않는다. 현재 뷰 필터만은
pan 때문에 read revision 검사가 거부된 경우 최신 accepted camera를 다시 조회한다.
이 규칙은 panel/selection 변경에는 적용하지 않는다. follow의 camera 변화도 자기
목록 조회에만 반영된다.

## 표시/비용 경계

좌표 투영과 형상 페이지 검증은 `drc-geometry.js`로 분리했다. owner DRC는 동일한
순수 함수를 재사용하며 helper에는 HTTP/storage/owner 초기화가 없다. 공유 state의
`dbu_um`을 두 모드에 제공하고 Rust camera 문자열도 좁은 geometry metadata로 제공한다.
경로·catalog·reviewer는 포함하지 않는다. camera 문자열 제공을 guest Goto 초안 복원
또는 전체 키/포인터 조작 완료로 계산하지 않는다.

marker/outline은 현재 표시 bitmap과 같은 viewport의 device 좌표에 합성한다.
새 geometry worker나 별도 native render를 요구하지 않으며, margin pan의 같은 배율
착지 합성 뒤에도 현재 카메라에 맞춰 그린다. 공유 이미지가 없는 동안 DRC만으로
새 geometry 프레임을 만들어 보냈다고 ACK하지 않는다.

guest UI당 한 번에 HTTP 작업 하나만 실행한다. gateway 전체 native DRC queued+active
1개라는 기존 상한은 그대로여서 다른 guest와 경합하면 busy가 가능하다. 응답 1MiB,
규칙 32/오류 64행, 형상 slice 2,048점·전체 262,144점, 이전 페이지 stack 64개를
적용한다. 큰 윤곽을 받는 동안 다른 패널 조작은 대기하며 Reload review로 취소할 수
있다. 현재 페이지 marker만 표시하므로 목록에 없는 모든 오류가 표시된다고 주장하지
않는다. 서버 quota/RSS·실제 브라우저 반응성 보장은 별도다.

## 검증과 남은 일

집중 검증은 web 단위 113개(외부 fixture 3 ignored), native owner 21개, strict
web/app/app-core clippy 및 전체 ES2017/JS 회귀를 통과했다. native 시나리오는
owner+explore 2개의 다른 오류/PNG 탐색·독립 panel/selection을 유지하고, follow와
explore에 전달하는 좌표 단위/camera도 검사한다. 실제 브라우저 검사는 아니다.

새 UI gate는 별도 whole-result 동의/다른 source 거부, HTML로 해석하지 않는 규칙/설명,
u64·ICE/ASCII 윤곽, follow 이동 금지, 개인 선택/패널 복원, 응답 유실 후 무재시도,
pan 중 늦은 focus·현재 뷰 read 거부·revoke와 늦은 geometry를 검사한다. 전체 guest
client gate도 DRC 허가 없는 무조회·WS admission 전 무조회·같은 guest 자격증명/경로·
실제 controller 연결과 이미지 표시 ACK를 검사한다.

CLI gate는 실제 Rust 실행기에서 내장 guest 자산 bytes 일치와 별도 DRC 허가로
합성 fractional ASCII 결과를 읽고, 미허가 SVRF 조회·implicit pack 생성·source/cache/
DRC 수정이 없음을 확인하도록 확장했고 실제 release CLI 검사도 통과했다.

전체 `sh tools/validate_rust.sh`는 2026-09-17 exit0 / `RUST VALIDATION: ALL OK`로
완료됐다. web113(+외부 fixture3 ignored), native owner21·stream20, 전체 JS/ES2017,
CLI 공유/DRC, occupancy27·jobdeck83·renderer46 및 KLayout jobs1/8 각각
13 PX+2 phase-exact+14 style 대조가 포함된다. 로그는
`/private/tmp/floe-share-drc-ui-battery.log`이며 배터리 시작/완료 시 변경 제품·테스트
파일의 SHA-256이 같음을 확인했다. `cargo fmt -p floe-web -- --check`와
`git diff --check`도 통과했다. 기존 dependency/Pillow/GLib 경고는 남아 있으며
실제 브라우저·Linux·현장 실행을 한 것으로 계산하지 않는다.

다음은 scope 제한 레이어 목록, displayed receipt 기반 pick/snap·룰러, pointer/키
조작과 DRC 순회·마커 클릭/CD 조작의 연결이다. SH-08 실제 owner/guest 탭·storage/
opener·복사 링크/복원 수용, Python-free Linux·G1/G4·TeeBox Firefox/ETX는 별도다.
기존 브라우저 시작 파일 정책 거부는 재시도/우회하지 않았고 현장 측정을 다시 요청하지
않았다. 원격 배포 C/G3는 새 권한·운영 정책이 필요하고 M5는 실측 조건부다.
사용자가 유보한 index hot reload/revision 저장소 정책은 건드리지 않는다.
