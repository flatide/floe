# 웹 클라이언트·공유·legacy review 후속 수정 — 2026-09-28

`feature/webui`의 네 건을 코드와 합성 회귀로 확인하고 수정했다. 기존 인덱스 revision
문구 정리와 별도인 런타임 변경이다. 실제 브라우저·RHEL/ETX·NFS 현장 수용은 수행하지 않았다.

## 1. 재접속 복원 실패 후 중단

WS close가 예약한 `restore()`의 GET이 한 번 실패하면 오류만 표시하고 다음 재시도를
예약하지 않았다. 이제 일시적인 HTTP503·네트워크 오류·타임아웃에도
500ms → 1s → 2s → 4s → 최대5s 간격으로 복원을 다시 시도한다. 성공한 WS hello에서
500ms로 초기화한다. pagehide/종료/인증 만료/종료된 view는 재시도를 중단하며
이전 page/socket의 늦은 콜백은 무효화한다. 재접속은 서버 상태를 다시 읽는 과정이며
사용자 입력이나 저장·게시 승인을 재전송하지 않는다.

`client.test.cjs`의 연속 오류·복원 성공·backoff 초기화·pagehide·늦은 콜백·401 회귀를
`validate_web_ui.cjs`에 연결했다.

## 2. 내장 한글 조합 중 Escape가 초안을 폐기

내장 조합기가 Escape를 소비하지 않아 notes editor의 폐기 핸들러까지 전파됐다.
첫 Escape는 현재 보이는 조합 글자를 유지하면서 조합만 끝내고 이벤트를 소비한다.
다음 Escape는 기존의 명시적 초안 폐기 동작이다. OS IME의 조합 처리와 서버 저장
권한은 바꾸지 않았다. 커서 이동 등으로 이미 끝난 조합은 Escape를 가로채지 않는다.

조합기 단위 검사와 실제 notes panel 합성 DOM 검사에서 텍스트·editor 유지, 두 번째
Escape의 폐기, 입력 중 HTTP 요청 없음, OS IME 경로 보존을 확인했다.

## 3. Explore가 owner clip의 마지막 자원을 점유

기존 기본 worker 한도2는 owner+Explore가 연결되면 clip의 별도 worker를 계속 막았다.
기본 한도를3으로 바꾸되 CPU16·decoded2048MiB 총한도는 유지한다. Explore 입장은
동일 `Resources` lock 안에서 게스트 비용과 최소 owner clip 여유
**1 worker·1 CPU·min(owner budget,256MiB)**를 함께 검사한다. 부족하면 게스트 입장을
busy로 거부한다. 실제 clip permit은 명시적 내보내기 때만 획득하며 guest/owner를
강제 중단하거나 CPU·메모리 상한을 우회하지 않는다.

이는 최소 clip을 위한 **게스트 입장 조건**이지 항상 유효한 clip 예약은 아니다.
이후 다른 owner/index 작업의 입장, 더 큰 jobs 요청은 여전히 busy가 될 수 있다.
UI는 현재 capacity를 바탕으로 jobs를 제안하고 최종 admission은 서버가 다시 검사한다.
예약 회계는 process-local이며 서버 전체 quota나 실제 RSS 상한은 아니다.

단위 검사는 CPU/worker/메모리 각각의 경계, 두 번째 게스트 거부, clip 입장 및 반환을
검사한다. native 공유 통합 검사에서는 실제 Explore 연결 중 owner export permit이
허용됨을 확인한다. 별도 owner export/managed clip 게이트는 준비·승인·렌더·다운로드·
취소/회수 경로를 검사한다. 두 게스트를 동시에 검사하는 fixture만 worker 한도4를
명시한다.

## 4. `/tmp` legacy sidecar 소유자 누락

pack/reviewer에서 유도한 임시 이름과 파일 형식 검증만으로는 다른 OS 사용자가 미리
만든 파일을 배제하지 못했다. legacy 임시 read target에 한해 서비스 effective UID를
기록하고, NOFOLLOW로 연 FD의 UID를 **hash/parse/binding 전에** 검사한다. 이름 선택
후 바뀐 파일도 같은 검사를 거치며 다른 UID면 명시 오류다. 자동 채택·이동·삭제는 없다.

인접 sidecar는 기존 공유 파일시스템 정책을 유지한다. reviewer 태그가 OS 인증이
되는 것도 아니고 같은 UID 안의 악의적 변경까지 차단하는 기능도 아니다. UID 검사
외의 bounded regular file·single link·Stamp·보안 속성·변경 감지 검사는 유지한다.

notes/waives 모두 같은 UID의 정상 임시 파일을 읽는 회귀를 유지했다. 다른 UID 거부는
테스트 Store의 기대 UID만 달리해 실제 FD 읽기 검사를 통과시키지 못하도록 검증했다.
`chown`·권한 상승·타 사용자 파일은 사용하지 않았으며 거부 후 파일 바이트가 보존된다.

## 검증

- `cargo test --offline --locked -p floe-app-core -p floe-web --lib`:
  app-core 356 passed/8 ignored, web 129 passed/3 ignored. 별도 fixture 게이트는 아래로 실행.
- `node tools/validate_web_ui.cjs`: ES2017 파싱·클라이언트/프로토콜/한글/notes 회귀 통과.
- `cargo clippy --offline --locked -p floe-app-core -p floe-web --all-targets --no-deps -- -D warnings`:
  통과. 수정 Rust 파일의 rustfmt 검사·`git diff --check`도 통과.
- `sh tools/validate_rust.sh --only view_stream,owner_service,managed_clip,drc_review,web_read_reviewer,web_hangul`:
  `RUST VALIDATION: ALL OK`. native stream20·owner service25, clip 오라클,
  read-only reviewer·한글 GTK 대조·DRC review 파일 계약 검사를 포함한다.

로컬 로그는 `/private/tmp/floe-review-four-units.log`,
`/private/tmp/floe-review-four-ui.log`, `/private/tmp/floe-review-four-clippy.log`,
`/private/tmp/floe-four-regressions.tRTUu8/battery.log`다.
기존 tiler/VFS 경고는 남아 있다. 선택 게이트 통과를 전체 배터리 또는 실제 UI/현장
수용의 새 통과 기록으로 해석하지 않는다.

## 2026-09-30 최신 병합 후 미커밋 작업 재검증

기준은 `b655282`와 기존 미커밋 변경이다. 셀 트리/root·잡덱 병합 뒤 위 네 수정,
Electron 종료 신호·fuse·TeeBox 실행기를 함께 검사했다. 공용 TeeBox 서버 재개,
WK 전용 기능, 현장 GUI 수용으로 범위를 넓히지 않았다.

추가 보완:

- owner 통합 검사에서 새 `design.ovh`가 revision 파일 목록에 없어 정상 회수 준비와
  중단 회수 복구 2건이 실패했다. OVH를 Stamp·sync·inventory·회수 목록에 포함하고,
  seal 전에 형식과 OVM binding을 검증한다. 미봉인 OVH를 가진 구 revision은 자동
  승인하지 않는다([계약/호환성](WEBUI_INDEX_REVISIONS.ko.md)). 삭제·내용 변경·동일
  바이트 inode 교체·symlink/hardlink·손상/다른 소스 요약을 거부하는 회귀를 추가했다.
- X11 opt-in 검사는 전체 Electron/portable gate뿐 아니라 `electron/package.json`의
  `npm test`에도 연결했다. 새 배포 진입점의 회귀가 기본 단위 검사에서 빠지지 않는다.
- 병렬 Rust 빌드 중 clipboard 단위 검사의 실제 10ms 시계가 즉시 성공 probe까지
  만료시켜 실패했다. 테스트에서만 monotonic 시계와 timer 전달을 제어한다.
  9ms에는 미완료, 10ms에는 거부, 늦은 응답/아직 시작하지 않은 probe는 권한을
  주지 않는 경계를 검사한다. 제품의 750ms 제한·권한·OS 클립보드는 바꾸지 않았다.
- Electron 종료 결과 처리의 들여쓰기를 정리했다. 기존 SIGHUP/INT/TERM 의미는 같다.

확인 결과:

- 웹 UI/프로토콜 회귀 통과. Electron `npm test` 53개는 최종 변경 후 5회 연속 통과.
- Rust app-core 375, web 156, packager 13 단위 검사 통과. fixture가 필요한 ignored
  검사는 이 숫자에 포함하지 않는다. 세 package의 일반 통합 검사도 통과했다.
- OVH 회귀를 포함한 revision 단위 32개와 실제 `floe-index`의 native revision 게시
  검사를 통과했다. 정상 OVH 봉인과 손상/다른 원본 header의 게시 거부를 포함한다.
- Electron Rust service/download 19개와 실제 합성 pipe 수명/다운로드 17개 통과.
  live·폴더 대기·격리 process-group의 SIGHUP 및 staging 정리를 포함한다.
- Electron portable 실행기/closure 합성 7개 통과. Linux X11은 모의 환경 검사이며
  실제 Linux/ETX 실행으로 세지 않는다.
- `sh tools/validate_rust.sh --only view_stream,owner_service,managed_clip,drc_review,web_read_reviewer,web_hangul,app_cli,web_portable,embedded_host,app_render,app_clip,cache_revision`:
  **12개 선택 게이트 ALL OK**. native stream20·owner HTTP25(회수/중단 복구 포함),
  실제 revision 게시·clip 바이트/Region XOR·DRC 저장/읽기·한글 대조를 포함한다.
  전체 배터리 또는 실제 Electron/RHEL/NFS 현장 수용 통과로 확대 해석하지 않는다.
- 해당 Rust package와 Electron service의 `clippy --all-targets --no-deps -D warnings`,
  미커밋 Rust 파일들의 rustfmt 검사, `git diff --check` 통과.
  workspace 전체 `cargo fmt --all --check`는 기존 `cli/src/svrf.rs`, `tiler`,
  `web/src/broker/runtime.rs` 등의 포맷 차이로 실패했다. 관련 없는 파일은 고치지 않았다.
  기존 tiler/VFS 의존 crate 경고도 유지한다.

로그는 `/private/tmp/floe-pending-{rust-final,clippy-final,electron-rust,electron-clippy}.log`,
`floe-pending-native-lifecycle.log`, `floe-pending-portable.log`,
`floe-pending-web-ui-final.log`, `floe-pending-electron-repeat-{1..5}.log`다.
OVH 집중 검사는 `floe-pending-hierarchy-{revision,native}.log`에 남겼다.
최종 선택 배터리는 `floe-pending-selected-gates-final.log`다.
최초 clipboard 시간 의존 실패는 `floe-pending-electron-node.log`에 보존했다.
최초 회수 통합 실패는 `floe-pending-selected-gates.log`에 보존했다.
기존 Electron archive는 이번 소스 변경으로 재패키징하지 않았다.
