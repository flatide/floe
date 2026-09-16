# M2b-4b4 — 게스트 DRC 순회·마커·박스 선택

2026-09-17, `feature/webui`. [게스트 DRC 패널](WEBUI_SHARING_DRC_UI.ko.md)과
[조회/룰러 입력](WEBUI_SHARING_QUERY_UI.ko.md)에 개인 오류 순회·캔버스 선택을
연결한다. 별도 승인한 DRC 결과만 사용하고, 기본 off·loopback-only는 그대로다.
CD와 자동 이동 모드, 실제 브라우저 및 전체 공유 수용을 완료한 단계는 아니다.
후속 [M2b-4b5](WEBUI_SHARING_DRC_CD.ko.md)에서 CD/자동 이동을 연결했다.
아래 “카메라를 이동시키지 않는다”는 M2b-4b4 시점 기록이며, 현재는 확인된 이동
이후에만 순회가 자동 이동을 따른다. 단순 마커 선택은 계속 이동하지 않는다.

## 순회와 입력

- Previous/Next error, `,`/`.`와 캔버스 Tab/Shift+Tab은 현재 규칙의 페이지를
  넘어 순회한다. 오류 목록의 위/아래 키도 같은 유계 순회를 사용한다.
  active/waived, Current view, Selected only 필터를 native `filtered_step`에
  전달한다. 순회는 개인 focused record만 바꾸고 선택 집합은 바꾸지 않는다.
- native 262,144-slot 검색 한도에 닿으면 **Continue search**를 표시한다.
  다음 요청은 명시 클릭으로만 전송하며 background drain/retry하지 않는다.
  u64 cursor·감소하는 remaining·선택 revision·응답 bbox/필터를 검증한다.
  규칙/필터/선택 변경, Current view의 카메라 변경은 이전 continuation을 폐기한다.
  Escape는 진행 중인 순회 read 또는 continuation을 취소한다.
- 현재 페이지와 focused record의 실제 표시 marker만 hit-test한다.
  marker는 device 좌표에 그리며 hit 반경은 6 CSS px다. 클릭 시 현재 overlay
  DOM rectangle로 변환하므로 letterbox·분수 CSS origin·DPR를 반영한다.
  전체 pack 공간 검색이나 숨은 오류의 자동 스캔은 추가하지 않는다.
- 단순 클릭은 기존 replace/Shift-add/Ctrl·Command-toggle만 수행한다.
  Explore의 더블클릭은 `isolate:false`의 native Frame error를 한 번 요청한다.
  첫 클릭 HTTP가 빠르거나 늦어도 선택 토글을 두 번 적용하지 않는다.
  첫 선택이 실패/불명확하면 이어진 이동도 실행하지 않는다.
- Follow는 자기 목록/선택만 변경한다. 카메라 gesture controller를 활성화하지
  않으며 marker 더블클릭도 goto를 보내지 않는다. 수동 ruler는 기존대로 Explore만이다.

현재 단계의 순회는 **카메라를 이동시키지 않는다**. Go · same scale / Frame error와
명시 더블클릭으로만 이동한다. 기존 owner의 jump-mode 자동 순회·zoom lock·CD 대상
복원은 다음 M2b-4b5에서 ACK + 동일 revision snapshot에 묶어 연결한다. 단순히
이동을 큐에 넣었다는 boolean만으로 CD의 소유 오류를 정하지 않는다.

## 현재 페이지 박스 선택

`e` 또는 Box select를 켜고 두 모서리를 클릭한다. Shift는 add, Ctrl/Command는
toggle이며 Escape는 첫 모서리 취소 → 모드 종료 순이다. 드래그는 계속 pan이다.
manual ruler와 box 모드는 동시에 활성화하지 않는다. 단순 mouse-down을 pan으로
오인해 박스를 취소하지 않으며 실제 이동/새 accepted view는 이전 박스를 버린다.

클라이언트는 **현재 규칙/페이지의 최대 64개 ID**와 µm bbox, waive 필터를 기존
`POST /api/v1/guest/{id}/drc/selection`에 보낸다. 기존 Apply DTO에 선택적인
`bbox_um:[decimal;4]`, `waived:boolean`을 추가했다. bbox가 있으면 envelope의
`state_rev`가 필수다. Rust는 숫자/순서/개수와 view/review/selection revision을
검증하고 `SelectionCandidates`에서 실제 교차/상태를 필터링한다.

서버는 admission 전, 선택 적용 시, 응답 body poll 시 권한/뷰 유효성을 확인한다.
중간에 카메라가 바뀌거나 응답이 유실된 선택 변경은 **재시도하지 않고 Reload review**를
요구한다. Current view의 읽기 재조회 예외를 selection mutation에 적용하지 않는다.
native 선택 집합 5,000개 상한·guest 한 작업·공유 DRC 큐 상한은 그대로다.
owner/다른 guest의 선택, source/cache/DRC 파일, 리뷰 쓰기·노트/SVRF·export 권한을
변경하지 않는다.

## 검증

집중 검사에서 web 단위 118개(+외부 fixture 3 ignored), native owner 21개,
전체 ES2017/JS gate, web/app/app-core all-target strict clippy `--no-deps`를 통과했다.
기존 미변경 dependency 경고는 남으며 workspace 전체 strict clippy 합격 주장이 아니다.

새 gate는 다음을 검사한다.

- u64 순회/감소 cursor/명시 continuation·취소·필터 검증, 선택 집합 불변.
- ICE/ASCII outline과 기존 개인 패널 복원/불명확 변경 무재전송 회귀.
- DPR/letterbox marker·두 모서리 좌표, 실제 guest 입력 controller의 Follow/Explore
  선택·Tab·box와 pan 동작, ruler와 box 모드 배선.
- 느린/빠른 첫 클릭의 double-toggle 방지, 늦은 응답·camera/revoke 초기화.
- 실제 native bbox/waive 필터, state fence 누락/오래된 revision 거부, selection CAS,
  두 guest/owner 독립성, source/index/DRC bytes·mtime 불변.
- 새 script의 bundle hash/HTML 배선 및 release CLI byte-exact asset 검증.

최종 `sh tools/validate_rust.sh`는 exit0 / `RUST VALIDATION: ALL OK`로 완료됐다.
web118(+외부 fixture3 ignored), native owner21·stream20, release 공유 CLI의
byte-exact 자산·권한/수명 검사, 전체 JS/ES2017, occupancy27·jobdeck83·renderer46,
KLayout jobs1/8 각각13 PX+2 phase-exact+14 style을 포함한다.
전체 로그는 `/private/tmp/floe-guest-drc-nav-battery.log`다. 검증 전후 제품/테스트
15개 파일의 SHA-256이 동일하며, 임시 venv 링크는 검증 종료 후 제거했다.
renderer/VFS/vendor·Cargo.lock과 main의 사용자 변경은 건드리지 않았다.
최종 fmt 및 web/app/app-core strict clippy `--no-deps`도 다시 통과했다.
집중 로그: `/private/tmp/floe-guest-drc-nav-{js,unit,native,clippy}.log`.
DOM/WS harness와 native HTTP는 실제 브라우저의 포커스·클릭·화면 수용이 아니다.
이전 시작 파일의 브라우저 접근 차단은 재시도하거나 다른 경로로 우회하지 않는다.

## goal까지 남은 범위

다음 구현은 게스트의 **실제 승인된 이동 → 자동 CD**, jump-mode/zoom lock·hover와
선택/CD의 독립 복원, 수동/auto/CD 생성 순서 및 k/K/Escape 수명 대조다.
그 뒤 SH-08 실제 owner/guest 탭·storage/opener/복원과 시각 수용,
Python-free Linux 실행·G1 지연/pacing·G4 전체 대조, TeeBox Firefox/ETX는 여전히 남는다.
원격 C/G3은 별도 운영 정책/승인이 필요하고 M5 world-tile은 실측 조건부다.
사용자 유보 사항인 index hot reload/revision은 이번 범위가 아니다.
이번 단계로 `shares=false`를 전체 완료로 바꾸거나 GTK를 은퇴시키지 않는다.
