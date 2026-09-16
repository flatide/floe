# M2b-4a — 로컬 초대 UI와 전용 게스트 프레임 화면

현재 query/룰러·pointer 연결은 [M2b-4b3](WEBUI_SHARING_QUERY_UI.ko.md)를 따른다.
아래 구현/미완료 범위는 M2b-4a 시점이며 SH-08 실제 수용은 계속 남는다.

후속 [M2b-4b1 게스트 DRC UI](WEBUI_SHARING_DRC_UI.ko.md)는 별도 whole-result 승인과
개인 목록/선택·윤곽·오류 이동을 연결한다. 아래의 layout-only 설명은 4a 단계 당시
범위이며, 현재 DRC 공유도 자동이 아니라 별도 체크를 요구한다.

`feature/webui`, [공유 경계](WEBUI_M2_SHARING.ko.md)의 M2b-4 첫 UI 단계다.
앞선 follow/explore transport와 DRC/query API를 사용자용 화면에 연결하기 시작한다.
**전체 공유 UI·SH-08 또는 원격 배포 완료가 아니다.** 뒤의 잔여 기능을 전체 목표에서
빼지 않는다. 기존 GTK 기본 실행기와 `feature/jobdeck` 실측 작업은 변경하지 않는다.

## 사용과 명시 공개 범위

```sh
rust/target/release/floe2-web view synthetic.oas --local-sharing
# 자동 브라우저 실행을 원하지 않으면 --no-open을 추가한다.
```

`--local-sharing`은 기본 off다. 다른 창으로 forward하지 않고 독립 workspace를
시작하며 listener는 계속 `127.0.0.1`이다. 옵션만으로 초대가 발급되지 않는다.
owner의 기존 비공개 시작 링크로 workspace를 열고 layout을 연 뒤 **Share locally…**를
선택한다. 현재 view revision을 기준으로 공개 설명을 확인하고 체크해야 초대를 만든다.
owner 시작 링크를 guest에게 주는 것이 아니다.

이 화면이 승인하는 것은 **현재 가시 레이어·이미 로드한 jobdeck 레벨의 layout scope**다.
현재 viewport 사각형에 한정하지 않으며, margin·다른 위치의 geometry와 layout label도
포함한다. 기존 허용 범위의 query API 권한 역시 그대로다. 이번 UI는 DRC opt-in을
보내지 않는다. DRC 결과 전체 공개는 [별도 API 계약](WEBUI_SHARING_DRC.ko.md)에 따라
별도 승인 UI를 붙여야 하며 layout 공유로 자동 허용하지 않는다. note 본문·SVRF·파일
탐색/색인·review 쓰기·서버 export 권한은 추가하지 않는다.

- follow: owner의 카메라/표시 프레임을 따른다. guest 조작은 owner로 보내지 않는다.
- explore: 기존 admission을 통과한 별도 worker/view로 fit·goto·줌·pan·depth/detail/
  thin·frames/labels/mono를 바꾼다. 일반 옵션 기본 자원 상한을 늘리지 않는다.
  explore 슬롯이 없으면 연결이 거부될 수 있으며 무한 worker 대기열은 없다.
- 초대 한 번 교환에 최대 120초, 접속 최대 1800초, 초대/접속 합계 4개라는 기존
  한계를 유지한다. UI는 이 최대 수명을 안내하며 새 탭에서 재사용 가능한 영구 링크로
  표현하지 않는다. 잔여 초 단위 countdown이나 여러 사용자 계정 체계는 아니다.
- 목록에서 owner가 개별 Revoke를 실행한다. 생성/폐기의 응답이 불명확하면 목록을
  다시 확인한다. POST를 자동 재전송하지 않으며 초대 secret은 서버 목록에서 복구하지
  않는다. owner가 초대 창을 닫는 동안 발급이 완료됐을 수 있어도 다음 목록에서 폐기할
  수 있다. UI를 닫았다는 것만으로 서버 취소 성공을 주장하지 않는다.
- source/view/layer scope 변경·owner 종료 등 기존 폐기 조건을 유지한다. 이미 받은
  픽셀이나 사용자가 복사한 링크/화면은 회수하지 못한다.

현재 capability `share_grants=true`가 이 실험적 UI를 연다. `shares=false`는 남은
DRC/layer/query UI와 전체 수용을 완료로 광고하지 않기 위해 유지한다. flag 이름을
원격 공유 지원이나 전체 M2 완료로 해석하지 않는다.

## 게스트 앱의 분리

정적 `/guest/{64hex share_id}` 페이지는 공유가 켜졌을 때만 제공한다. 정적 shell에
인증/설계 데이터는 없고, 임의 ID의 shell을 받아도 API에 접근할 수 없다.
초대 secret은 `#invite=…` fragment에만 두고 첫 HTTP 요청 전에 주소에서 제거한다.
owner의 `#bootstrap=…`는 거부한다. 초대 창의 새 탭 링크는 `noopener noreferrer`이며
서버의 no-store·no-referrer·CSP·Host/Origin 경계를 유지한다.

`guest.js`는 owner `app.js`를 로드하지 않는다. owner catalog·source 선택·쓰기 복구
기록·note controller를 초기화하지 않고, `floe-guest-session:<origin>:<share_id>`라는
별도 tab storage만 사용한다. HTTP는 guest 경로와 `X-Floe-Guest-CSRF`, WS는 guest
CSRF subprotocol만 쓴다. 별도 cookie 이름/Path도 기존 계약 그대로다. 이 코드 경로의
분리가 동일-origin 브라우저 자체의 별도 보안 sandbox라는 뜻은 아니다.

초대 교환 실패/불명확한 응답은 자동 재시도하지 않는다. 인증 후 WS 재접속은 먼저
현재 guest session을 다시 확인하고 이전 편집을 재전송하지 않는다. revoke/expiry/
scope 변경이면 bitmap·저장 자격증명을 지우고 새 초대를 요청하도록 한다. Leave share는
자기 guest session만 닫는다. 실패 시 서버 종료 미확인이라고 표시하고 owner/다른 guest를
종료하지 않는다. 숨김 탭/pagehide에서는 연결과 미표시 프레임을 정리하며 visible/bfcache
복귀는 현재 세션을 확인한다. idle worker 회수와 view 상태 복원은 기존 서버 계약이다.

## 프레임과 입력

공통 `protocol.js`/`image-decode.js`의 유계 raw/PNG 검증·decode를 재사용한다. view ID·
connection epoch·dataset/worker/render revision·bbox/크기가 맞는 프레임만 받아서
별도 Canvas buffer에 decode하고 animation frame에서 표시한 뒤 ACK한다. 표시 전에
상태가 바뀌면 discarded ACK이고, 옛 연결의 callback은 현재 연결에 ACK하지 않는다.

foreground와 margin 두 bitmap을 보관해 같은 render key·배율·정수 16px 위상의 pan이면
현재 중심에 margin을 먼저, 이전 label 프레임을 그 위에 그린다. 새로운 strip에는 margin
geometry가 즉시 보인다. decode·수신 packet·표시 canvas 등 추가 메모리는 있으므로
서버의 전송 credit을 브라우저 RSS 상한이라고 부르지 않는다.

follow는 owner bitmap을 비율을 유지해 창에 맞춰 보여주며 owner 해상도를 바꾸지 않는다.
explore는 자기 viewport의 DPR/device 크기를 서버에 요청한다. 기존 8192px/축·16Mpx 한계를
넘으면 오류를 표시하며 몰래 품질을 낮추지 않는다. 현재 키는 화살표 50%/Shift 10%,
`+`/`-`, Home이며 goto는 µm 문자열을 Rust에 전달한다. 편집은 최대 16개 대기,
65ms cadence와 accepted state revision 확인을 적용해 frame ACK/heartbeat 여유를 남긴다.
충돌/거부된 명령은 재실행하지 않는다. partial/approximate 프레임과 rendering 상태를
표시하며 background/pan 자체를 새 geometry 요약 정책으로 바꾸지 않는다.
WS 제어 응답은 owner와 같은 256KiB 상한이다. 정상 4,096개 u32 레이어 쌍이
64KiB를 넘을 수 있으므로 frame header의 별도 64KiB 상한과 혼동하지 않는다.

## 검증과 잔여

현재 집중 검증: web unit 113개, `--local-sharing` 기본 off/독립 실행/잘못된 값 거부,
web/app/app-core strict clippy, ES2017 parse·기존 owner UI 전체 회귀가 통과했다.
`guest.test.cjs`는 owner storage 미접근, follow 제어 금지, explore 자기 ID·직렬화,
raw pixel 표시 후 ACK, margin 새 strip, stale frame discard, 재접속/폐기/개별 로그아웃,
숨김/복귀를 검증한다. `sharing.test.cjs`는 승인 부재·변경된 문맥·명시 발급/폐기·
fragment 링크·불명확 응답의 무재시도·modal 수명을 확인한다.

`validate_web_local_sharing.py`는 합성 source의 실제 Rust CLI에서 default-off와 opt-in,
정적 guest shell/asset, native open, 두 모드 초대/교환/폐기, owner/DRC 거부, source/cache
불변과 종료 수거를 확인했다. Node DOM/Canvas와 native HTTP/WS 증거는 **실제 브라우저
SH-08**의 대체가 아니다.

전체 `sh tools/validate_rust.sh`는 exit 0 / `RUST VALIDATION: ALL OK`로 끝났다
(`/private/tmp/floe-share-ui-battery.log`). web 단위 113개(외부 fixture 3 ignored),
owner native 21개·공유 stream 20개, occupancy 27개·jobdeck 83개·renderer 46개,
KLayout jobs 1/8 각각 13 PX + 2 phase-exact + 14 style 검사가 통과했다.
이후 최종 점검에서 위 WS 수신 상한만 64→256KiB로 정정했다. 최종 소스의 전체 JS
gate에 정상 4,096개 레이어 허용과 256KiB 초과 거부를 추가해 재통과했고
(`/private/tmp/floe-share-ui-js-final.log`), release 실행기를 재빌드한 뒤 실제 CLI
gate도 재통과했다(`/private/tmp/floe-share-ui-native-final.log`). 이 마지막 검사는
서버가 제공하는 guest/sharing 자산이 현재 소스 bytes와 같은지도 단언한다.
전체 배터리를 이 마지막 경계값 변경 후 다시 실행한 것으로 표기하지 않는다.

다음 M2b-4b는 명시 DRC 공개 승인·개인 목록/선택·overlay/goto, scope로 제한한 레이어
목록/선택, displayed receipt에 묶인 pick/snap/룰러, 남은 pointer/키 조작이다. backend
존재를 guest UI 완료로 계산하지 않는다. 실제 동시 owner/guest 탭·cookie/storage/opener/
뒤로가기·복사 URL의 브라우저 수용, Python-free Linux·G1/G4·현장 Firefox/ETX, 원격 C/G3는
계속 별도다. 브라우저 시작 파일의 기존 정책 거부를 재시도/우회하지 않았고 현장 검사를
재요청하지 않았다. 원격 노출과 전체 서버 quota 정책은 새 승인이 필요하다. M5는 실측
조건부이며 index hot reload/revision 저장소는 사용자가 유보한 범위다.
