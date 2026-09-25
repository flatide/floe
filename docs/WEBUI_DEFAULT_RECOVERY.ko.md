# 공유 기본값 복구 및 게시 결과 불명 처리

2026-09-25. Electron/브라우저가 공유하는 Rust·웹 구현이다. WKWebView 전용
코드는 변경하지 않는다. 이번 검증은 **단위·모의 오류·정적 검사** 범위이며 실제
Electron 조작, 서비스 강제 종료, RHEL/ETX/NFS 장애 수용과 구별한다.

최신 사용자 요청으로 [xattr 없는 NFS의 기본 저장 호환성](WEBUI_NO_XATTR.ko.md)을
추가했다. 아래 xattr 필수 설명은 `374452b` 시점 기록이며, 현재는 ENOTSUP에만
inode별 보조 record를 사용한다. 기존 식별·복구·결과 불명 방어는 유지한다.
NFS 실제 장애/다중 client 수용은 별도로 남는다.

## 1. 공유 기본값의 명시적 복구

Shared design default의 **Review interrupted-file repair…**를 선택한다.
현재 소스·jobdeck mode가 유도하는 정확한 `.layerprops`만 검사한다. 일반
Review publication과 승인 token 종류를 분리하며 `approve_recovery:true`만
복구를 승인한다. 현재 화면의 레이어/스타일을 저장하는 기능이 아니다.

- 미리보기: 등록된 소스·동적 보호 목록, 완성된 layerprops 구문, 정확한 두 링크,
  기존 stable lock, 자체 표식을 검사한다. 파일/lock 생성이나 디렉터리 순회 없음.
- 승인: 같은 파일·내용·mtime·권한·ACL/xattr·lock을 재검증하고 **표식에 적힌
  여분의 stage 링크 하나만 제거**한다. target/payload/권한/다른 임시파일은 보존한다.
- 취소·만료·다른 source/mode·symlink·세 번째 링크·다른 inode·변경된 lock/표식은 거부한다.
- UI preview는30초, core proof는120초. 이미 제출한 결과 불명의 proof는 같은
  작업의 읽기 전용 확인을 위해 유지한다. 만료된 승인을 재사용해 unlink하지 않는다.
- `Check repair result (read only)`는 추가 unlink 없이 같은 작업의 상태만 확인한다.
  완료면 복구 성공(새 directory sync를 했다고 주장하지 않음), 그대로면 미완료로
  종결하고 새 미리보기/승인이 필요하다. 확인 실패는 계속 불명이며 새 쓰기를 막는다.
- browser session journal을 기록하지 못하면 승인 POST를 보내지 않는다. 응답 유실/
  reload 뒤 자동 POST 없음. Resolve는 명시 클릭으로 원래 seq/body만 재전송한다.

신규 기본값 게시 inode에 macOS `com.floe.default-stage-v1`, Linux
`user.floe.default-stage-v1` 표식을 기록한다(최대16KiB). 정확한 source 경로·inode·
size/mtime, target leaf, stage leaf, directory/file/lock identity에 묶인다.
자체 표식은 인증 수단이 아니며 등록 권한·scope·보호 검사를 대체하지 않는다.
일반 read/import의 single-link 검사는 완화하지 않았다.

기존 기본값의 내용 형식과 다른 속성은 유지하며 교체 시 자체 표식만 갱신한다.
**374452b 당시 공유 기본값 게시에는 xattr 저장/읽기 지원이 필요**했다. 지원하지
않으면 게시 전에 실패한다. DRC의 기존 xattr 요구와 별개로, 기본값에 추가된
호환성 조건이다. 해당 NFS에서 지원한다고 가정하지 않는다. 일반 Save settings
다운로드는 이 shared-default 게시 경로와 다르다.

새 owner HTTP 경로는 `POST /api/v1/defaults/{seq}/reconcile` 하나이며,
기존 `/prepare`의 `recover:true`와 기존 제출의 별도 승인 필드를 사용한다.
권한 opt-in은 기존 shared-default publisher 그대로다. guest나 파일 선택만으로
권한이 생기지 않고 임의 path/reviewer 입력도 추가하지 않는다.

## 2. 일반 게시의 결과 불명

DRC note/waive와 shared defaults가 사용하는 `Stage`를 보완했다.
`linkat/renameat`를 **호출하기 전에** Drop 자동 정리를 해제한다. syscall 오류나
worker panic 뒤 임시 링크를 조용히 지워 복구 증거를 없애지 않는다.

| 관측 | 보고/후속 동작 |
|---|---|
| rename 결과 오류라도 정확한 target inode·single-link·stage 부재 확인 | 성공; directory sync 실패는 durability 경고 |
| link 성공 ACK 뒤 정확한 stage 정리와 target single-link 확인 | 성공 |
| link 오류지만 target이 게시됐을 가능성, unlink 실패/검증 실패, post-commit panic | `PublicationUnknown`; 취소/미게시로 표시하지 않음 |
| 게시 syscall 전 검증·권한·취소 실패 | 기존 확정 실패/취소, 소유 stage 정리 |

불명인 일반 게시는 같은 서버 세션에서 새 게시를 차단한다. DRC receipt는
`published:null,outcome_unknown:true`, defaults는 active `uncertain`이다.
확정 receipt 재조회/동일 요청 replay는 재게시가 아니다. DRC detach/reconnect도
서버의 불명 latch를 해제하지 않는다. 일반 게시의 proof를 영구 journal로 저장하거나
실패한 syscall을 자동 재시도하는 구현은 아니다.

운영 절차: receipt와 파일을 확인한 뒤 새 서버 세션을 사용한다. 정확한 신규 표식과
두 링크가 남았다면 별도 복구 승인을 한다. 정상 single-link 파일은 복구하지 않는다.
무표식 구버전 orphan, target 없이 남은 stage, 증거가 충돌하는 파일은 이름을
추측하여 삭제하지 않으며 관리자의 독립 확인 대상이다. 새 세션은 이전 저장 승인이나
자동 저장 opt-in을 재생하지 않는다.

stable flock과 identity 확인은 협조하는 writer 사이의 방어다. 비협조 writer에 대한
원자적 CAS나 NFS 서버의 선형화 보장은 아니다. dev/ino가 달라지는 재마운트·다른
클라이언트에서는 복구를 거부할 수 있다. 해당 수용은 NFS 지원 재개 시 다룬다.

## 3. 자동 검증과 남은 실제 검사

- app-core layer defaults23개: 권한/ACL·속성 보존, 별도 복구, 취소·만료·표식/lock
  교체·동적 입력 보호, 모의 unlink 오류/완료 후 오류, rename/link 오류 후 증거 보존.
- managed DRC15개: 기존 저장/취소/lease 회귀와 `PublicationUnknown` 전달.
- web lib127 passed /3 ignored: 기본값 actor 복구 및 DRC 서버 쓰기 latch 포함.
  기본값 actor의 gap 재구성 테스트는 macOS의 자체 합성 파일 xattr만 읽는다.
  Linux에서 이 macOS 전용 테스트를 통과했다고 주장하지 않는다.
- 권한 AST inventory3 passed /1 diagnostic ignored; 새 endpoint도 owner-only 표에 포함.
- Node ES2017/UI 회귀: 일반 승인과 복구 승인 혼용 거부, no-consent/expired/stale,
  journal 실패, ACK 유실·reload·동일 요청 resolve, read-only check, active 불명 차단.
- app-core/web scoped fmt 및 clippy `--all-targets --no-deps -D warnings`.
  기존 tiler/VFS dependency 경고는 이번 범위에서 변경하지 않는다.
- `sh tools/validate_rust.sh --only layer_defaults,web_ui`: exit0. GTK와 동일한
 20개 target/bytes 게시 오라클 및 Node 전체 UI 회귀이며 GUI를 열지 않았다.
  전체 배터리 실행이 아니고 렌더/잡덱 정확도 전체를 재검사했다고 주장하지 않는다.
- Electron `service`의 `cargo check --offline --locked`와 debug/release 빌드:
  exit0. 기본 실행 경로의 release 헬퍼도 갱신했으며 앱 실행·배포 번들 재포장은 별도다.

선택 배터리 로그: `/private/tmp/floe-default-final.9CwuyT/selected.log`.
SHA-256: `dc481191572dc386d6d90a925ae5364dcfc7f925353c9b1144efbbbf7cf8ed01`.

초기 단위 검사에서 `/var`와 `/private/var` canonical path 비교를 바로잡았고,
clippy의 큰 enum은 Box로 수정했다. 웹 crate의 unsafe 금지 계약을 유지하여 fixture
속성 검사를 macOS 테스트의 읽기 전용 시스템 명령으로 한정했다. 최초 실패를 통과로
계수하지 않는다. 제품에는 명령 실행 의존성을 추가하지 않았다.

실제 GUI/서비스 강제 종료·새 세션, NFS 오류/locking/xattr/ACL/durability, 다중
클라이언트 수용은 사용자 요청대로 이번에 실행하지 않는다. 이전 DRC Electron
통과 기록을 이번 공유 기본값 UI 통과 기록으로 전용하지 않는다.
