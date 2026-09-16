# M2b-4b5 — 게스트 DRC 이동 확정·CD·자동 순회

2026-09-17, `feature/webui`. [유계 순회/마커 선택](WEBUI_SHARING_DRC_NAV.ko.md)의
다음 단계다. 별도로 승인한 DRC 결과와 기존 guest read/panel API만 사용한다.
기본 off·loopback-only이며 노트/SVRF, 파일 탐색·쓰기·export, 원격 공개를 추가하지 않는다.

## 이동과 측정 대상

- Go · same scale은 현재 배율을 유지하고 이후 오류 순회 배율을 잠근다.
  Frame error/수식키 없는 더블클릭은 오류에 맞추고 auto-fit을 다시 켠다.
  **서버가 확인한 이동 뒤에만** 목록 단순 클릭·Previous/Next·`,`/`.`·Tab 순회가
  같은 이동 모드를 따른다. 중간에 수동 줌으로 배율이 바뀌면 다음 순회는 배율을 잠근다.
- 캔버스의 단순 마커 클릭은 계속 선택만 한다. Ctrl/Command-toggle·Shift-add도
  이동하지 않는다. modifier 더블클릭은 이동/두 번째 선택 변경을 하지 않는다.
  Follow는 카메라 이동·새 CD 대상 생성을 할 수 없다.
- 선택된 오류와 CD 대상은 별개다. CD는 **마지막으로 이동이 확인된 오류**를 유지한다.
  현재 선택이 달라져도 CD 대상은 바뀌지 않는다. 새 오류로 실제 이동한 경우에만
  그룹을 교체하고, 일반 pan은 CD를 다시 읽지 않는다.

`guest-focus.js`는 명령을 보내는 서비스가 아니라 기존 `explore.set`의 receipt
관찰자다. 해당 seq의 `accepted`와 **정확히 같은 state_rev의 share.state**를 모두
확인해야 성공한다. 둘의 도착 순서는 무관하며 u64를 JS Number로 바꾸지 않는다.
관찰자 한 개, 기존 편집 직렬화/65ms cadence/최대16개 대기는 그대로다.

확인 대기는8초로 유계다. 거부·timeout·새 입력·건너뛴 revision·연결 변경·End navigation은
CD 후속 효과를 폐기한다. 아직 보내지 않은 해당 요청은 큐에서 제거한다.
**이미 보낸 이동을 되돌리는 취소는 아니다.** 이동이 뒤늦게 완료될 수 있지만
CD를 만들거나 명령을 재전송하지 않는다. native focus의 `isolate:false`도 유지한다.
Current view 필터는 이동이 확인될 때만 해제한다.

## 룰러·복원·취소

- Rust `measurements` 응답의 최대3개 CD를 기존 소수 좌표 decode/치수선 painter로
  표시한다. 임의 선분·거리 근사 계산을 추가하지 않는다. 다른 오류 identity나
  유효하지 않은 응답은 `CD unavailable`이며 무한 재조회하지 않는다.
- 수동/auto/CD가 같은 생성 순서 history를 사용한다. 비동기 CD는 빈 슬롯으로
  순서를 예약하고 응답을 그 자리에 넣으므로 나중 수동 룰러를 앞지르지 않는다.
  `k`는 가장 나중 항목, `K`는 전체 룰러를 지운다. 미완료 CD 그룹 삭제는 그룹 전체를
  폐기한다. 복원 중인 CD 삭제는 복원 완료 후 명시적으로 다시 요청하도록 안내한다.
- Escape는 활성 gesture/수동 ruler와 기존 박스/검색 취소 우선순위를 유지한 뒤
  CD 그룹 삭제 → 오류 자동 이동 종료 순이다. End navigation은 바로 이동 모드를
  끝내고 CD/선택 윤곽을 숨긴다. 마커 숨김은 CD를 숨길 뿐 삭제하지 않는다.
- Reload review는 개인 선택·이동 모드·CD 대상/남은 개수를 서버에서 다시 읽는다.
  **goto를 재생하지 않는다.** 수동 룰러는 같은 연결의 review reload에서 보존하며,
  연결 종료/revoke는 모든 주석을 비운다.
- CD 삭제가 개인 panel CAS 저장과 겹치면, 이전 저장의 확정 응답 뒤에 **새 삭제 의도**를
  저장한다. 이전 응답이 유실되거나 잘못됐으면 자동 재시도/후속 저장을 중단하고
  Reload review를 요구한다. read/이동 observer 취소와 파일 변경 없는 개인 panel
  mutation을 구별하며, 취소로 mutation 실패를 숨기지 않는다.

마커 hover는 현재 페이지의 표시 후보만 조회해 규칙/오류 번호/waived를 평문으로
보여준다. 포인터 이동·박스 preview·CD 변경은 RAF에서 overlay만 다시 그리며 base
이미지를 재합성하거나 frame ACK를 재발급하지 않는다. 기존 gesture release/pan의
합성 경로는 바꾸지 않는다. marker off/뷰 변경/연결 종료 시 이전 tooltip을 비운다.

## 검증

집중 JS/ES2017 gate에서 다음을 확인했다.

- receipt 순수 상태 테스트: ACK/snapshot 양 순서, >2^53 revision, timeout/거부/
  다른 입력/epoch/Follow/건너뛴 상태/취소·늦은 응답, 중복 송신 방지.
- 실제 guest controller의 DOM/WS harness: 두 receipt 절반 중 하나만 받았을 때
  CD/저장 없음, 승인 뒤 CD, rate-limit 대기 중 취소한 명령 제거, 새 입력·거부·
  timeout·disconnect 이후 CD/자동 재전송 없음, timer/RAF cleanup.
- DRC controller: 선택과 CD 독립 복원, auto-fit/수동 줌 lock, 미완료 CD 삭제,
  느린 대체 이동 취소, panel의 게시 후 응답 유실 시 재시도 없음,
  확정 CAS 뒤 새 삭제 저장, Current view 승인 경계, Follow/revoke.
- 공통 ruler controller의 수동/CD 삭제 순서, guest marker toggle에 따른 치수선
  표시, box hover의 base blit·ACK 불변. 기존 owner 룰러 경로는 기본 설정을 유지한다.
- 새 자산의 bundle hash/HTML 배선과 release CLI byte-exact 검사도 전체 배터리에 있다.

최종 `sh tools/validate_rust.sh`는 exit0 / `RUST VALIDATION: ALL OK`로 완료됐다.
web 단위118(+외부 fixture3 ignored)·HTTP 경계15, native owner21·공유 stream20,
release CLI의 byte-exact guest 자산, 전체 JS/ES2017, occupancy27·jobdeck83·renderer46,
KLayout jobs1/8 각각13 PX+2 phase-exact+14 style을 포함한다.
`cargo fmt -p floe-web --check`와 web/app/app-core all-target strict clippy
`--no-deps -- -D warnings`도 통과했다. 기존 미변경 dependency·Pillow/GLib 경고는
남아 있으며 workspace 전체 strict clippy 합격을 주장하지 않는다.

로그는 `/private/tmp/floe-guest-drc-cd-{battery,unit,clippy,js}.log`다.
검증 시작/종료의 제품·테스트15개 파일 SHA-256이 같고, 임시 venv 링크는 종료 후
제거했다. renderer/VFS/vendor·Cargo.lock과 main의 사용자 변경은 건드리지 않았다.
실제 브라우저 SH-08 수용은 DOM/WS harness로 대체하지 않는다. 이전 정책상 거부된
시작 파일 접근은 재시도/우회하지 않으며 TeeBox 검사를 다시 요청하지 않았다.

## goal까지 남은 범위

이번 단계는 예정된 guest DRC CD/jump-mode 구현을 연결한다. 다음은 공유 요구표
SH-01~10와 실제 구현의 **마지막 범위 대조**, 가능한 로컬 회귀와 현장 수용 체크리스트
정리다. SH-08 실제 owner/guest 탭·storage/opener/복원·시각/키보드 수용은 남는다.
웹 전환 전체로는 Python-free Linux 실행·G1 지연/pacing·최종 G4, TeeBox Firefox/ETX,
원격 C/G3의 운영 정책/승인이 별도다. M5 world-tile은 실측 조건부이고 사용자 유보인
index hot reload/revision은 착수하지 않는다. `shares=false`와 GTK 비교 경로를 유지하며
이 단계를 전체 goal 완료로 계산하지 않는다.
