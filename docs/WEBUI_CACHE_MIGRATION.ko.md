# 웹 전환: 명시 Index의 캐시 이름 개명

2026-09-17, M4g-32. **명시 Index 개명 구현·로컬 전체 회귀 통과. 전체 웹 전환 수용 완료는 아니다.**

사용자 결정: 열기/조회는 비쓰기, 개명은 명시 Index에서만 수행한다.
M4g-31 통합의 구 이름 읽기 호환 위에 추가하며 Python/GTK 셸의 열기 시 자동
개명 정책은 변경하지 않는다.

## 경계

- layout/deck의 명시 `floe2-web index`와 웹 Index가 `<src>.floe/`를
  `.<src>.ice/`로 개명한다. 재사용 가능한 캐시는 개명만 하며 native 재색인을
  하지 않는다. 새 색인은 처음부터 숨김 이름을 쓴다.
- DRC의 명시 `--build`/웹 Build pack은 `<db>.ice`를 `.<db>.tray`로 개명한다.
  note/waive 파일의 논리 DB 이름은 같으므로 이동하거나 다시 쓰지 않는다.
- info/open/probe/파일 선택/승인 미리보기/cell profile은 개명하지 않는다.
  profile snapshot의 명시 출력 권한은 기존 계약이며 캐시 개명 권한이 아니다.
- 구 캐시가 stale/corrupt면 기존처럼 force 없이는 교체·개명을 거부한다.
  새 이름이 이미 있으면 새 이름을 우선하고, 구 대상은 삭제하지 않는다.
  잘못된 새 대상을 숨기기 위해 구 대상으로 우회하지 않는다.

## 잠금과 실패

두 이름의 안정적인 `.index.lock` inode를 일정 순서로 잠그고 child 수거까지
유지한다. 구 이름으로 실행 중인 협조적 Rust index writer와도 충돌한다.
lock 파일은 해제 시 삭제하지 않는다. 새 캐시에도 구 이름의 writer-lock 파일이
생길 수 있으며, 이것은 geometry 캐시 복제본이 아니다.

관리형 Index는 양쪽 별칭의 쓰기 lease를 요구한다. deck 계획에서도 개명 대상은
write 목록에 포함되므로 활성 reader와의 충돌을 첫 source 변경 전에 거부한다.
정본 이름의 무변경 재사용은 기존처럼 read-only kept 항목이다.

이 reader lease는 **같은 관리형 Resources 범위**에 한정된다. 별도 CLI/다른
프로세스·다른 서버 사용자의 열린 뷰어를 OS 차원에서 잠그는 기능은 추가하지
않는다. Index 전에 다른 뷰어를 닫아야 한다. 공유 캐시의 revision/hot reload,
비협조 외부 writer·부모 경로 교체에 대한 파일시스템 격리는 별도 과제다.

개명은 검증한 형제 이름에 대해 descriptor-relative no-replace syscall을 사용한다.
Linux는 renameat2(RENAME_NOREPLACE), macOS는 renameatx_np(RENAME_EXCL)이다.
지원하지 않는 OS/filesystem에서는 오류를 내며 copy/delete/덮어쓰기로 우회하지
않는다. destination 충돌·symlink·잘못된 종류를 거부하고 원본을 보존한다.

개명 완료와 재색인 완료는 별개다. rename 후 native 실패/취소가 생기면 새 이름과
기존 데이터는 남는다. rollback하지 않으며 CLI 진단/관리형 상태에 완료된 개명을
따로 기록한다. directory fsync 실패도 이미 완료한 개명의 취소로 보고하지 않고
내구성 경고를 남긴다. force는 기존처럼 재생성 허용이며 backup은 아니다.

## 집중 검증 (macOS arm64)

- inode/bytes/mtime 보존, 충돌/교체/별칭/잘못된 파일 종류/개명 전 취소.
- legacy info/probe/profile 비쓰기, explicit layout/deck 재사용 개명.
- 활성 reader가 있는 deck batch의 전체 사전 거부, 양쪽 writer lock.
  비-root 계정(uid 501)의 읽기 전용 부모에서도 조회 가능·개명 거부·원본 보존을
  확인했다. root/ACL/filesystem별 권한 검증을 대신하지 않는다.
- DRC 구 이름 읽기, reuse 개명, sidecar 불변, 취소/실패 뒤 개명 receipt 유지.
- UI의 성공/실패/취소 상태에서 개명 수/내구성 경고, 잘못된 receipt 거부.
- 위 항목의 단위·CLI·관리형 Index·DRC Build·UI 집중 gate는 통과했다.
  core 288, web 118/HTTP 15/inventory 3, app 27 단위 검사와 release build,
  app/core/web fmt 및 strict clippy도 통과했다. fixture 전용 ignored 테스트는
  해당 드라이버로 실행한 결과를 별도로 확인했다.
- 전체 `sh tools/validate_rust.sh`는 `BATTERY_EXIT=0` / `RUST VALIDATION: ALL OK`로
  통과했다. 이 순서 안에서 새 개명 gate·관리형 Index·DRC Build도 다시 통과했고,
  KLayout workers 1/8 각각 13 PX + 2 phase-exact + 14 style을 통과했다.
  전체 로그는 `/private/tmp/floe-cache-migration-battery.log`이다.
  기존 native 의존성의 unused 경고와 개발 GTK deprecation 경고는 남아 있다.
  최종 strict clippy는 앱 3개 package에 `--no-deps --all-targets -- -D warnings`를
  적용한 결과이며 workspace 전체 경고 0이라는 뜻이 아니다.
  실제 Linux syscall 실행은
  Linux 환경에서 별도로 확인한다; macOS 합성 통과로 대신하지 않는다.

집중 로그: `/private/tmp/floe-cache-migration-acceptance-final.log`,
`floe-cache-migration-managed-final.log`, `floe-cache-migration-drc.log`,
`floe-cache-migration-ui-final.log`(뒤 세 파일도 `/private/tmp/`).

실패 이력도 보존한다. 첫 CLI 실행은 40초, native 버전 확인은 5초 제한에 걸렸다.
이후 직접 읽기 전용 버전 확인에서 index/app은 각각 5ms, 처음 확인한 renderd는
37.867초였다. 기존 shell 제어 스크립트의 버전 확인도 한 차례 5초 제한에 걸렸다.
같은 코드·같은 제한의 집중 재실행은 통과했지만 OS 원인은 확정하지 않았으며,
첫 실행 지연 문제를 해결한 것으로 세지 않는다. timeout/보안 설정은 바꾸지 않았다.
추가한 jobdeck 테스트의 초기 fixture에는 실제 레이어·배치 크기가 없어 빈 선택으로
실패했다. valmini의 LY=1/DT=0과 UX/UY를 명시해 수정했으며 제품 판정은 변경하지 않았다.

## 전체 목표의 잔여

실제 브라우저 UI/다운로드/공유 SH-08, Python-free Linux 실행과 새 개명 syscall,
TeeBox Firefox/ETX·실칩 성능·G1/G2/G4 수용은 여전히 남아 있다. 원격 단계 SH-10은
사용자 결정으로 설계·코드·합성 검증까지 보류한다. M5는 측정 조건부다.
이번 개명 변경을 전체 웹 전환 완료 또는 캐시 revision/hot reload 구현으로 세지 않는다.
