# 웹 전환: native worktree 빌드 스탬프

2026-09-17, M4g-33. **집중 검증·전체 회귀 통과. 첫 실행 지연과 현장 수용은 미해결.**

M4g-31/32에서 기록한 첫 실행 지연과 별개로, native 빌드의 반복 재생성을 수정한다.
원격 공유·실제 서버·캐시 revision/hot reload의 범위를 확장하지 않는다.

## 원인과 변경

`rust/{cli,renderd}/build.rs`는 `.git`을 디렉터리로 가정해 `.git/HEAD`를 무조건
감시했다. linked worktree의 `.git`은 파일이므로 이 대상은 존재하지 않는다.
Cargo는 없는 감시 대상을 매번 dirty로 판정해 변경 없는 빌드도 다시 실행했다.

두 wrapper는 `rust/build_support/native_revision.rs`를 공유한다. Git이 반환한
실제 HEAD/index 및 공통 branch/packed-refs 경로 중 존재하는 대상만 감시한다.
worktree의 `.git` 포인터 파일, native package src/manifest/build helper도 감시한다.
`status`에는 `GIT_OPTIONAL_LOCKS=0`을 적용해 감시 중인 index를 갱신하지 않는다.
호출자의 repository-routing 환경변수로 다른 저장소 revision을 찍지 않는다.

native의 기존 ZIP 계약은 유지한다: 비어 있지 않은 `FLOE_SRC_REV`(trim) 우선,
그 다음 현재 source root 자체의 Git, 아니면 `unknown`. 상위의 무관한 저장소
HEAD를 사용하지 않는다. 앱 build script의 별도 full-hash/입력 검증 계약은 바꾸지 않는다.
dirty `+`는 스탬프 갱신 시 관측한 상태이며 전체 소스의 암호학적 ID나 서명이 아니다.
native 0.12.101 / 비교 셸 0.12.146이며 재빌드가 필요하다. wire/cache 형식은 그대로다.

## 근거

실제 `feature/webui` worktree에서 수정 전 같은 native release build를 연속 실행했다.
첫 빌드 17.41초, 소스 변경 없는 반복 빌드 15.68초였고 두 bin artifact 모두
`fresh=false`였다. 수정 후 첫 빌드 17.26초, 반복은 0.05초 / 두 bin 모두
`fresh=true`였다. 이는 버전 범프 전 동일한 0.12.100에서 원인만 대조한 수치다.
로그는 `/private/tmp/floe-build-watch-{before,after}-{first,repeat}.{log,jsonl}`이다.
컴파일·링크 생략의 근거이지 cold executable start, runtime decode/raster,
브라우저 input-to-photon 속도의 개선 근거가 아니다.

`tools/validate_native_revision.py`는 사용자 저장소를 바꾸지 않고 private 합성 Git
repo/worktree와 dependency 없는 Cargo fixture를 만든다. 두 native wrapper가 같음을
확인하고 실제 빌드 스크립트를 사용해 다음을 검증한다.

- 변경 없는 Cargo 재실행 `fresh=true`, src 변경 및 HEAD만 바뀐 commit은 갱신.
- 일반 checkout/worktree의 자기 HEAD/index, 공통 ref 경로와 존재하는 감시 대상.
- packed refs 전환과 detached HEAD, 상속된 다른 repository-routing 변수 무시.
- 다른 repo 안의 ZIP은 `unknown`, Git 없는 환경의 명시 override, 비정상 `.git`.
- native wrapper 두 개를 `rustc -D warnings --emit=metadata`로 검사하고 fmt 확인.

집중 gate 로그 `/private/tmp/floe-build-watch-gate.log`와 버전 갱신 후
`floe-build-watch-gate-final.log`는 PASS다. 0.12.101 전체 release와 앱 3개 package의
strict clippy(`--no-deps --all-targets -- -D warnings`)도 통과했다. 기존 native
의존성 경고까지 0이라는 뜻은 아니다. 0.12.101에서 다시 한 no-op native 빌드는
0.12초 / 두 bin `fresh=true`였다(`floe-build-watch-version-repeat.{log,jsonl}`).
위 상대 로그 이름은 모두 `/private/tmp/` 아래다.

새 index/renderd/app의 직접 `--version` 실행은 각각 0.443/0.376/0.498초였고
native 기대 버전 일치를 확인했다. 이전 수십 초 지연이 이번에도 반복된 것은
아니지만, 단일 관측으로 원인이 해결됐다고 판단하지 않는다.
첫 전체 `sh tools/validate_rust.sh` 실행은 새 revision/개명/selfcheck/portable gate를
모두 통과한 뒤 `layer_defaults-bd03435aee2b8413 --ignored --nocapture`의 30초
제한에서 중단했다(`/private/tmp/floe-build-watch-battery.log`, exit1).
같은 코드·같은 제한의 레이어 기본값 집중 재실행은 통과했다
(`floe-build-watch-layer-defaults-retry.log`). 개별 package 구성의 core 288/web 118/
app 27 단위 검사와 일반 test harness 실행도 exit0으로 통과했다
(`floe-build-watch-{core,web,app}-tests.log`). fixture 전용 ignored 검사는
아래 전체 배터리의 각 드라이버로 별도 실행했다.

소스를 고정한 전체 재실행은 `BATTERY_EXIT=0` / `RUST VALIDATION: ALL OK`로
통과했다(`/private/tmp/floe-build-watch-battery-final.log`). 새 revision gate와
캐시 개명·portable·레이어 기본값을 포함하며, KLayout workers 1/8 각각
13 PX + 2 phase-exact + 14 style을 통과했다. 첫 실행 실패 이력은 삭제하지 않는다.

추가 진단으로 이미 빌드된 웹 표준 test harness 8개를 최대 4개씩 `--list`로
실행했고 각각 0.008~0.015초/exit0이었다(`floe-build-watch-startup-list.log`).
테스트 본문을 실행하지 않는 warm 관측이며, 앞선 지연의 원인을 입증하거나
cold-start 수용을 대신하지 않는다. 전체 재실행 성공이나 native no-op 개선 역시
첫 실행 지연의 해결로 세지 않는다.

## 남은 수용

macOS의 첫 실행 수십 초 지연은 여전히 미해결이다. 이 변경으로 해결했다고
보고하지 않으며 검증 timeout·보안 설정을 완화하지 않는다. 실제 브라우저
UI/다운로드/공유 SH-08, Python-free Linux 및 Linux 개명 syscall, 현장
Firefox/ETX·실칩 성능·G1/G2/G4가 남는다. 현재 호스트는 Darwin arm64이며 확인한
Docker/Podman/Lima/QEMU 실행 환경은 없다. 새 설치나 원격 서버 사용은 하지 않았다.
원격 SH-10은 사용자 결정대로 보류, M5는 실측 조건부다.
