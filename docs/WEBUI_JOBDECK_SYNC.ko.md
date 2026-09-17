# 웹 전환: jobdeck 정방향 통합 기록

2026-09-17, M4g-31. **정방향 통합·로컬 회귀 통과. 전체 목표/현장 수용 완료는 아니다.**

## 세 번째 통합 — OVR 대표 점

2026-09-18, M4g-41. **정방향 통합·로컬 전체 회귀 통과. 현장 수용 완료는 아니다.**

웹 `f953cf1`에서 실측 기준 `c8171175931eaa5e980233eea6121851b6d04946`까지
14개 커밋을 정방향 병합한다. main/실측 worktree는 변경하지 않는다. 아래 기존
M4g-31 기록의 통과 결과는 이번 통합의 검증 결과가 아니다.

- native/index/renderd는 `0.12.155`. page frontier는 기본 off, `FLOE_RUST_PAGE_REPS=on`만
  진단용이다. OVR의 선택 조건·메모리 상한·실제 도형 위 점·query 제외·한 캐시 수명에
  고정된 mmap 계약은 [대표 점 설계](REPRESENTATIVES.ko.md)를 따른다. hot reload는 추가하지 않는다.
- Rust CLI/관리형 API에 `--representatives`, `--representatives-only`,
  `--representatives-points 1..4194304`를 이관한다. 기본 off이고 jobdeck 자체는 거부한다.
  웹의 수동 Index에는 plain layout용 opt-in만 노출한다. 자동 열기/읽기에 쓰기를 추가하지 않는다.
- 최신 캐시에서 대표와 occupancy가 둘 다 없고 둘 다 요청하면 대표 → occupancy의 두
  가산 패스를 실행한다. 기존 파일은 강제 재색인하지 않으며 두 패스 동안 같은 새/구
  캐시 writer lock과 관리형 lease를 유지한다. 첫 패스 실패/취소 뒤 두 번째는 시작하지
  않는다. 둘째 실패 시 이미 게시한 첫 요약은 남는다(다중 파일 원자 트랜잭션이 아니다).
  대표 단독 생성은 기존 OVM/OVP/OVT/meta/OVO를 보존한다. 취소는 소유한 tmp만 정리한다.
- 대표 점 카운터는 숫자 allowlist로 전달하고 프레임은 approximate로 표시한다. 이 표시로
  query 권한/장면 identity를 만들거나 바꾸지 않는다. 기본 캐시는 완성됐지만 선택 대표
  생성이 실패했다는 native 경고도 경로/원문 없이 boolean 상태로 노출한다.
- 선택 검증 `--only`/`--list`를 받으면서 이전 83개 검증 호출을 모두 유지한다.
  `--only web`는 Rust 앱/웹 집중 게이트이고, 기본 전체 배터리는 줄이지 않는다.
  빈 `--only=`/쉼표·공백뿐인 선택은 exit2로 거부한다. 이를 허용해 검사0개로
  성공하던 경로를 재현한 뒤 고쳤다. 선택 파서 자체의20사례 gate도 추가했다.

추가 게이트: 결합/가산 OVR bytes 일치, 명시 상한 재생성, 기존 캐시 bytes/mtime 불변,
활성 reader 쓰기 거부, 패스 순서/첫 실패/경계 취소/SIGINT/SIGTERM, API 불법 조합,
근사 표시와 query-scene 분리.

집중 검증 통과: workspace all-target check, core291/web121단위, ES2017/UI 전체,
Rust CLI 실파일 대조·관리형 reader 잠금/연속 추가 생성, 공개 옵션114개/native probe180회,
worker-client의 Python/raw 대조, native OVR의 무디코드·depth·pixel replay·손상 폴백·
결합 생성 실패 시 base cache 보존. 선택 실행의 `RUST VALIDATION: ALL OK`는
명시된5게이트의 결과이며 전체 배터리 PASS가 아니다. 로그는
`/private/tmp/floe-web-ovr-sync.yNot41/`의 `unit.log`, `ui.log`, `focused.log`다.
실제 새 서버58385/Chrome 읽기 복원은 [브라우저 §10.7](WEBUI_BROWSER_ACCEPTANCE.ko.md#107-에이전트-직접-재시작과-최신-native-통합-읽기)에 기록했다.

고정된 제품 소스로 전체 `sh tools/validate_rust.sh`가 exit0/
`RUST VALIDATION: ALL OK`로 끝났다(`full.log`). occupancy41, jobdeck83,
renderer46, OVR 가산·실패·픽셀 대조, KLayout workers1/8 각각13 PX+2 phase-exact+
14 style을 통과했다. 별도 fixture 하네스가 ignored oracle도 실제로 실행했다.
strict app/core/web clippy도 통과했다(`clippy.log`; 기존 native 의존성 warning은 별개).
그 뒤 제품 소스는 바꾸지 않고 위 선택 스크립트만 보완했다.90개 gate 목록/기본 전체
선택/alias/중복/무효 인자20사례와 실제 `--only validation_selector` 진입은 각각
통과했다(`selector-gate.log`). 선택 PASS와 전체 PASS의 로그/범위를 구분한다.

첫 executable 시작 지연은 [M4 §98](WEBUI_M4.ko.md#98-gtk-실행-준비와-실제-다중-미리보기의-검증-경계)에
추가 근거를 남겼으며 전체 회귀 PASS를 원인 해결로 세지 않는다.
전체 목표의 남은 범위는 실제 브라우저 수용, Python-free Linux
실행, G1/G4 및 현장 Firefox/ETX다. 원격 SH-10/index hot reload는 사용자 보류,
M5 world-tile은 성능 조건부로 유지한다.

## 기준과 범위

- 웹 기준 `8a45175`, 직전 공통 기준 `09be2ab`, 이번 실측 기준
  `45c99341d995d79bed5f62f99a518e6dfdfc818a` (14개 커밋).
- `feature/jobdeck` → `feature/webui` merge. 실측 worktree와 main은 변경하지 않는다.
- native/index/renderd 버전 `0.12.100`, Python 비교 셸 `0.12.145`.
  웹의 query-scene identity·공유 권한·원자 교체 계약은 유지한다.
- FLOEOVO2 depth별 점유, 자동 기준 셀, repetition 구간별 점유 병렬화,
  layout page frontier 대표, sub-cut wash 기본 off를 최신 native 구현으로 받는다.
  실칩 화질·속도 판정은 이번 합성 검증으로 대신하지 않는다.

## Rust 앱 이관

1. occupancy 미지정은 layout off / jobdeck on. 명시 opt-out과
   `--occupancy-um`의 opt-in 의미, `--occupancy-only`의 가산/재생성을 보존한다.
   `--occupancy-balance 0|1`을 native 인자와 관리형 API에 전달한다.
2. 색인 승인 미리보기는 source별 `occupancy_default`를 내려준다.
   새 승인은 이 기본값을 표시하되 이미 저장된 승인/재시도 옵션은 바꾸지 않는다.
3. 새 기본 이름은 레이아웃 `.<src>.ice/`, DRC `.<db>.tray`.
   순수 이름 계산과 읽기 전용 기존 경로 선택을 `app-core/cache/names.rs`에 모은다.
   기존 `<src>.floe/`와 `<db>.ice`는 재색인 없이 읽을 수 있다.
   잘못된 새 대상이 있으면 구 대상으로 몰래 우회하지 않는다.
4. 관리형 read/index lease, source 등록 범위와 출력/프로파일 보호는
   새·구 경로를 모두 포함한다. 활성 뷰가 읽는 캐시를 다른 이름으로 쓰지 않는다.
5. DRC note/waive 이름과 임시 폴백 해시는 pack 이름이 아니라 논리 DB 경로 기준.
   새·구 pack 이름의 인접 sidecar 및 새 규약의 temp 경로는 일치한다.
   예전 pack-path 해시 temp 파일을 추가 검색하지 않는다.
6. planner의 sparse/wash/대표 카운터는 숫자 allowlist를 통해 진단에 전달한다.

## 후속 구현: 명시 Index에서만 개명 (M4g-31 시점 기록)

GTK 최신 구현은 열기에서도 구 이름을 자동 개명한다. 웹의 읽기 범위는
그것만으로 파일시스템 변경 권한을 얻지 않는다. M4g-31 구현은 읽기/재사용 시
**개명하지 않으며**, 명시 index도 선택된 기존 경로를 재사용한다.
2026-09-17 사용자는 **명시 Index에서만 개명**을 승인했다. 이 통합의 회귀를
마친 뒤 별도 단계에서 구현한다. 읽기/조회에는 개명을 추가하지 않으며,
이 후속 구현을 이번 통합에서 이미 완료한 것으로 간주하지 않는다.
후속 구현은 활성 lease, 새 대상 충돌, 읽기 전용 디렉터리, 실패 시 원본 보존을
별도 검증해야 한다. 인덱스 revision/hot reload는 기존 유보 사항이다.

이후 구현·검증은 [M4g-32 기록](WEBUI_CACHE_MIGRATION.ko.md)에서 추적한다.
후속 단계의 검증 기준:

- layout Index / 선택 레벨의 deck Index / 명시 DRC pack build만 개명할 수 있다.
  info/open/probe/미리보기/파일 선택과 비쓰기 cell profile은 구 이름을 유지한다.
- 새·구 이름의 writer lock을 함께 보유한다. 관리형 세션의 활성 reader와
  충돌하면 개명 전에 거부한다. 이 lease를 별도 프로세스의 뷰어 잠금으로
  확대 해석하지 않는다.
- 새 이름이 있으면 그것이 우선이며 구 대상을 삭제하지 않는다. 목적지가
  나중에 생기는 경쟁도 atomic no-replace rename으로 거부한다. 원본의 종류·
  identity를 검사하고 symlink/특수 파일을 개명하지 않는다.
- 개명 전 오류/취소는 원본을 보존한다. 개명 후 재색인이 실패하거나 취소될
  수 있으므로 이름 변경을 별도로 보고한다. 성공한 개명을 자동 rollback하거나
  전체 작업이 무변경이었다고 보고하지 않는다. pack 이름과 무관한 리뷰
  sidecar는 이동하지 않는다.
- 새 이름과 재사용 파일의 bytes/inode/mtime, 충돌·읽기 잠금·실패·취소·profile
  비쓰기를 합성 fixture로 확인한다. 파일 형식 변경/재색인을 개명 자체의
  조건으로 추가하지 않는다.

## 검증 기록

- 합성 웹 UI 전체: 통과(새 source별 기본값, 승인 재연결 포함).
- 최신 코드 workspace 검사: app 27, core 285, web 118, HTTP transport 15,
  권한 inventory 3, render-core 118, renderd 21, vfs 67 통과.
  fixture 전용 ignored 항목은 별도 하네스의 대조 결과로만 판정한다.
- 실제 CLI 대조: layout 기본 off, opt-in/재생성, source·marker 손상,
  LOD, profile 비쓰기 대조 통과 후 fake worker 버전 조회 타임아웃 발생.
  별도 합성 진단과 원래 하네스의 단독 재실행 모두 통과했다.
  소스·시간 제한은 변경하지 않았으며 이전 실패도 기록으로 보존한다.
- CLI inventory: 공개 옵션 111개·숨김 거부 17개·native 파서 177회 통과.
- 최종 fmt(app/core/web/index/renderd/render-core/vfs)와 app/core/web all-target
  strict clippy(`--no-deps -- -D warnings`) 통과. 기존 native 의존 crate 경고는 남는다.
- 최종 고정 소스 전체 배터리: **BATTERY_EXIT=0 / RUST VALIDATION: ALL OK**.
  `floe-jobdeck-sync-battery-stable.log`. KLayout 오라클은 workers 1/8 각각
  13 PX + 2 phase-exact + 14 style 통과. 점유 37개, jobdeck/native/CLI/web
  집중 게이트와 실제 fixture 전용 ignored 항목도 원래 배터리에서 실행했다.
  앞선 실패·집중 재실행 이력은 아래에 보존한다.

### 첫 실행 지연과 재검증

첫 전체 실행은 `layerprops-b6213c275f863062`, 두 번째는
`layer_defaults-bd03435aee2b8413`의 30초 subprocess 제한에서 각각 멈췄다.
대조 불일치를 기록한 실패가 아니다. 코드/제한을 바꾸지 않은 단독 재실행은 각각
72문서·980스타일·4뷰(테스트 본문 0.12초), 공유 기본값 20사례(0.30초)를 통과했다.

원인 분리를 위해 새 단독 crate 구성의 `floe_app_core-c158d48fa83e4156`을
테스트 본문이 없는 `--list`로 두 번 실행했다. 첫 실행 **32,890ms**, 두 번째
**3ms**, 두 번 모두 292개 테스트 목록·exit0이다. 첫 실행 지연은 테스트 본문
없이도 재현됐지만, OS/보안 검사/캐시 중 무엇 때문인지는 확정하지 않는다.
패키지별 단위 검사 뒤 전체 배터리를 재실행하며 기존 timeout/단언은 유지한다.
재실행 PASS가 cold-start 수용이나 이 지연 원인 수정이라는 뜻은 아니다.

로그: `/private/tmp/floe-jobdeck-sync-battery.log`,
`floe-jobdeck-sync-battery-retry.log`(각 최초 중단),
`floe-jobdeck-sync-layerprops-retry.log`,
`floe-jobdeck-sync-layer-defaults-retry.log`(변경 없는 집중 PASS).

세 번째 전체 실행(`floe-jobdeck-sync-battery-retry2.log`)은 managed clip의
`cache-change` 주입에서 Worker/Cache 오류 분류 불일치를 검출했다. 새 경로 함수가
별도로 실행되는 fake worker 문자열 안에 잘못 남은 **하네스 이관 결함**이었다.
생성 시 실제 경로 리터럴을 넣고 marker 첫 바이트의 손상 확인도 추가했다.
오류 분류 단언은 그대로 유지했으며 집중 재실행은 native bytes·취소/수거·잠금·
손상 검사를 모두 통과했다(`floe-jobdeck-sync-managed-clip-retry.log`, 25.89초).

남은 게이트의 부분 점검(`floe-jobdeck-sync-tail-diagnostic.log`; 전체 PASS 아님)은
DRC read-reviewer의 FIFO 사례에서 중단했다. `.synthetic.db.*` 사이드카 glob이
새 `.synthetic.db.tray` pack까지 포함하여, 하네스가 의도적으로 만든 FIFO를
해시하며 대기했다. note/waive 접두사로 목록을 좁혔으며 FIFO/디렉터리/별칭 거부
사례 자체는 유지했다. 집중 재실행은 읽기·불변·ASCII fallback 전부 통과했다
(`floe-jobdeck-sync-read-reviewer-retry.log`). 중단한 실행의 전용 로그를 잡은
프로세스가 없음을 확인하고 소유가 확인된 임시 venv 링크만 정리했다.

DRC open 집중 검사는 기본 pack을 파일 선택 목록에서 찾던 가정도 검출했다.
새 `.tray` 기본 이름은 숨김 파일이므로 목록에서 감추는 것이 맞다. 명시 pack
선택용 비숨김 복사본을 만들고, 숨김 기본 pack은 여전히 노출되지 않는 단언을
추가했다. 제품의 탐색 권한/필터는 넓히지 않았다. 수정 후 집중 검사 통과
(`floe-jobdeck-sync-drc-open-retry.log`). 후반부 부분 실행은 전체 배터리 PASS로
계산하지 않으며, 모든 수정 이후 원래 배터리를 처음부터 실행한다.

전체 재실행 `floe-jobdeck-sync-battery-final.log`는 zoom-band의 20초 제한에서
중단했다. 같은 게이트의 무수정 재실행은 352사례·본문 0.01초로 통과했다
(`floe-jobdeck-sync-zoom-band-retry.log`). 실행 중 만든 다음 단계의 미배선 Rust
초안도 `web/build.rs`의 전체 app-core 소스 해시에 들어가 재빌드를 유발한 것을
확인했다. 초안은 검증 트리 밖으로 옮겼다. OS 차원의 첫 실행 지연 원인은
여전히 미확정이며, 소스를 고정한 후 전체를 다시 실행한다.

고정 소스의 `floe-jobdeck-sync-battery-fixed.log`는 별도 앱 구성의
`floe2_web-8da155cbd51789fb gtk_startup_oracle` 실행 30초 제한에서 중단했다.
웹 패키지 118개/HTTP 15개/권한 3개, 앱 패키지 27개 단위 검사는 각각 다시
통과했다. 무수정 시작 대조 재실행도 22 launch/21 first-generation·380 stream
정책 사례를 통과했다(`floe-jobdeck-sync-startup-retry.log`). 이후 동일 소스·기존
제한의 전체 실행이 통과했다. 이러한 재실행을 cold-start 지연 해결로 간주하지 않는다.

추가 관측: native cli/renderd `build.rs`는 worktree에서 파일인 `.git` 아래의
존재하지 않는 `.git/HEAD`를 watch한다. 반복 재빌드 비용의 후속 점검 대상이며,
이 사실만으로 모든 프로세스 첫 실행 지연의 OS 원인을 설명하지 않는다.
후속 [M4g-33](WEBUI_BUILD_REVISION.ko.md)에서 실제 no-op 재빌드 재현과
worktree-aware 감시 경로 수정을 추적한다. 첫 실행 지연과는 별개의 근거다.

## 전체 목표에서 남은 범위

이번 동기화와 회귀는 닫았고, 승인된 명시 Index 개명은 M4g-32에서 진행한다.
그 밖에 실제 브라우저 UI/다운로드/공유 SH-08,
Python 없는 Linux 실행, 현장 Firefox/ETX·실칩 성능 수용이 남아 있다.
원격 공유 SH-10은 사용자 결정으로 설계·코드·합성 테스트까지 보류하며 실제 공개 권한은 없다.
M5 world-tile 전송은 측정 조건부이며, 구현 커밋 수를 완료율로 환산하지 않는다.
