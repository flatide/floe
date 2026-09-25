# DRC-PUB-01 — 명시적 게시 복구 코어·승인 연결

2026-09-25 갱신. [프로세스 종료 분석](WEBUI_REVIEW_PROCESS_CRASH.ko.md)의 후속이다.
`075a1b3`의 **Rust DRC note/waive 표식·복구 코어**에 owner actor/HTTP/공통 웹 UI를
연결했다. Electron도 같은 UI를 사용한다. shared defaults·일반 게시의 결과 불명·실제
NFS 수용은 별도이므로 DRC-PUB-01 전체 완료로 세지 않는다. WKWebView는 동결한다.

## 1. 복구 대상과 저장 변경

신규 게시의 `linkat(stage,target)` 성공 후 `unlinkat(stage)` 전 종료는 같은
완성 파일에 두 이름을 남긴다. 일반 snapshot/import/reader의 `nlink == 1`
검사는 그대로 두고, 별도의 writer capability에서만 두 링크를 검사한다.

별도 journal 파일 대신 게시 inode의 xattr에 유계 JSON 표식을 기록한다.
DRC는 이미 pack binding xattr를 필수로 쓰므로 새 의존 라이브러리나 파일시스템
기능 종류를 추가하지 않는다. 그렇다고 실제 NFS의 xattr 지원을 확인했다는 뜻은
아니다. 추가 속성 저장/읽기 검증에 실패하면 **게시 전 실패**하며 무표식으로
조용히 저장하지 않는다.

- macOS `com.floe.review-stage-v1`, Linux `user.floe.review-stage-v1`.
- v1, 정확한 target leaf(리뷰어/kind 포함), `.floe-review-<pid>-<serial>.tmp`,
  directory·payload·stable lock의 `(dev, ino)`만 기록한다. 최대4KiB이며 unknown
  field/버전/경로 구문은 거부한다. pack identity는 기존 binding으로 따로 검증한다.
- 표식은 payload와 함께 sync된 뒤 게시된다. 기존 파일 교체 때도 새 inode에 맞게
  갱신한다. 기존 mode·uid/gid·ACL·다른 xattr는 유지한다. 파일 내용 형식은 그대로다.
- 표식은 인증 토큰/서명이 아니다. 서버의 등록된 writer 권한, scope, 원본·동적
  등록 보호를 대신하지 않으며 임의 요청 경로나 이름을 받지 않는다.
- 성공 뒤 표식은 남는다. read에서 지우거나 다음 실행 때 디렉터리를 순회하지 않는다.
  dev/ino는 client/mount-local 증거다. 다른 NFS client나 재마운트에서 값이 달라지면
  자동 이식하지 않고 거부한다. 다중 client 복구는 후속 현장 설계·수용 대상이다.

## 2. 코어 계약

`app-core/src/drc/review/store/recovery.rs`:

| 호출 | 결과/허용 범위 |
|---|---|
| `Store::prepare_recovery` | 읽기 전용 미리보기. 기존 lock만 열고 잠금; 생성하지 않음. 두 링크, 정확한 표식/binding·payload 구문·보호 범위 검사 후120초 opaque capability |
| `Recovery::recover` | 별도 명시 승인이 전제. 동일 capability/내용/metadata/lock 재검증 후 **표식의 stage 이름 하나만** unlink |
| `Recovery::reconcile` | 같은 작업의 읽기 전용 확인. `Pending` 또는 `Completed`; 충돌/확인 실패를 완료나 새 저장 허용으로 해석하지 않음 |

미리보기는 directory fd 기준으로 target과 표식의 정확한 sibling만 확인한다.
symlink, third link, 다른 inode, 바뀐 내용/권한/binding, 교체된 lock, 취소·만료·동적
입력 등록은 삭제 전에 거부한다. marker 없는 구버전 orphan은 이름만 보고 채택하지
않는다. 게시 전 종료로 target 없이 남은 stage도 이 API의 복구 대상이 아니다.

복구는 payload를 다시 쓰거나 target을 교체/삭제하지 않는다. 이미 동일 target이
single-link이고 stage가 없으면 추가 unlink 없이 완료를 확인한다. unlink 호출 뒤에는
취소나 syscall 오류만으로 실패를 단정하지 않고 정확한 inode·내용·security·stage 부재를
재검증한다. 확인 불가는 `Uncertain`이며 자동 재시도하지 않는다. directory sync 실패는
완료 결과의 durability 경고다. 관련 syscall 계약은
[link(2)](https://man7.org/linux/man-pages/man2/link.2.html),
[unlink(2)](https://man7.org/linux/man-pages/man2/unlink.2.html)를 참고한다.

stable flock은 협조하는 Floe writer끼리만 배제한다. 마지막 검증과 unlink 사이의
비협조적 디렉터리 변경에 대해 원자적 CAS를 제공하지 않는다. GTK/다른 프로그램 및
NFS의 client cache·locking·ACL·전원 손실을 로컬 성공으로 수용 처리하지 않는다.

## 3. 코어 단계 로컬 검증 (075a1b3)

- DRC review65 passed, private subprocess entry1 ignored. ignored entry는 부모
  테스트가 별도 프로세스로 명시 실행한다. fmt와 app-core clippy `--no-deps -D warnings`
  통과. 의존 VFS의 기존 경고2개는 그대로다.
- 기존 게시 전/후/dir-sync 뒤 note/waive × 신규/교체12조합 유지.
- 새6조합: 실제 Stage 내부 link/unlink 사이 SIGKILL2개, 표식 있는 gap fixture에서
  복구 unlink 전/후 SIGKILL4개. 새 프로세스에서 승인 전 두 링크 거부, 명시 복구 후
  원본 bytes/pack binding/일반 snapshot/새 저장을 확인한다. 복구 fixture의 시작 gap은
  단위 모델로 재구성했으며 실제 게시 중단2개와 구분한다.
- 구버전 무표식 gap 재구성2개는 여전히 거부되고 삭제되지 않는다.
- 단위 fault 모델: unlink 오류·변경 없이 결과 불명, unlink 완료 후 EIO와 늦은 취소,
  directory sync 실패, 완료 후 동일 작업 재확인, syscall 성공 뒤 대상 변경.
- 보안 경계: traversal/잘못된 marker·binding, 동일 bytes+attrs 다른 inode, 임의 stage,
  symlink, 세 번째 hard link, pack/권한 변경, 내용 있는 lock 및 **빈 다른 lock inode**,
  read-only store·동적 입력 보호·취소·만료. target/pack bytes와 기존 ACL/속성을 보존한다.

초기 검사에서는 새 owned marker 갱신으로 기존 전체-속성 동등 검사가 실패했다.
owned marker가 실제로 달라지는지 별도로 단언하고, 그것을 제외한 모든 속성·ACL·권한
동등 비교를 유지한 뒤 재실행했다. 초기 실패를 통과 기록으로 합치지 않는다.

검사 로그:

- `/private/tmp/floe-review-recovery-initial.log` — 초기1실패.
- `/private/tmp/floe-review-recovery-core-final.log` — review65개 통과.
- `/private/tmp/floe-review-recovery-clippy.log` — app-core clippy 통과.
- 선택 없는 전체 Rust/web 배터리: `/private/tmp/floe-recovery-battery.9KIaa9/full.log`,
  **exit0 / RUST VALIDATION: ALL OK**. 마지막 KLayout 대조는 각 worker 설정에서
  13 PX + 2 phase-exact + 14 style 통과. 앞의 부분 통과를 합산한 결과가 아니다.

전체 실행 명령은 새 합성 TMPDIR에서
`CARGO_BUILD_JOBS=4 sh tools/validate_rust.sh`였다. 제품 코드는 실행 전에 고정했고,
이후 변경은 문서 기록뿐이다. 개발 KLayout용 임시 `.venv` 링크는 종료 시 제거했다.
이번 단계에 새 Electron GUI/실제 NFS 검사는 없으며 코어와 기존 Rust/web 경로의
회귀만 검증했다.

로그 SHA-256:

```text
full:   93ea5888814831a0406db92fcad2f707791f9f3cd3bf763904a3cebbf2b20aa3
review: 165da12bd73f0d030abd543f8b80fd6dc4e129ff739a8b8f567a982ca3682a1f
clippy: de6a45676c1b9e4b111450db065611474d8b932909b2fd2dd776083bc93869e6
```

## 4. 명시적 승인 연결 (2026-09-25)

DRC 패널의 접힌 `Recover interrupted review file…`을 펼쳐 Notes/Waives를 선택한다.
등록된 writer만 `Preview file recovery` → 대상 basename·bytes·reviewer 확인 →
체크박스 → `Approve file recovery`를 사용할 수 있다. 미리보기 TTL은30초다.
현재 edit를 완료하거나 버려야 미리보기로 진입하며, 자동 저장 opt-in은 복구에
적용되지 않는다. 경로 입력·파일 목록·임의 reviewer·stage 이름은 받거나 노출하지 않는다.
notes-only 등록에서는 Waives 선택을 비활성화한다. `review_grant.available`은 현재
쓰기 권한이 아니라 **분리된 권한의 재연결 가능 여부**다. 따라서 복구 표시에는
현재 `notes/waives.available`, `editable`, `detached`, binding을 사용한다. 이 구분을
처음에는 잘못 연결했고 실제 Electron 검사에서 발견해 UI/통합 fixture를 수정했다.

| owner 전용 wire (`/api/v1/drc/review/{notes,waives}` 기준) | 동작 |
|---|---|
| `GET /recovery` | 별도 유계 receipt ledger 조회. 파일 복구 없음 |
| `POST /recovery/prepare` | 등록된 DRC/view/revision에서 읽기 전용 proof 준비. bounded body/native admission 유지 |
| `POST /recovery` | `approve_recovery:true`와 해당 token의 명시 승인. 일반 저장 token·autosave 승인과 호환되지 않음 |
| `POST /recovery/reconcile` | 결과 불명인 **같은 작업**만 읽기 전용 재확인. unlink 재시도 없음 |
| `POST /revoke` | 기존 경로로 미승인 preview 폐기 |

승인 시 기존 owner worker로 작업을 넘긴다. HTTP 요청 취소·응답 유실이 proof나
자원 lease를 회수하지 않는다. 결과 불명은 ledger를 active로 유지하고 proof를 보관해
다른 저장·전송·pack build·등록 교체를 막는다. 동일 seq/signature 재요청은 최초 receipt만
반환하며 old context도 재실행으로 해석하지 않는다. 명시적 read-only 확인은 별도 취소
flag를 사용하고 원래 write의 취소 flag를 초기화하지 않는다. 세션 종료는 모든 native
작업에 취소를 전달하고 worker가 해제할 때까지 admission을 유지한다.

브라우저는 승인 직전 tab/session에 동일 요청을 기록한다. 기록 실패면 승인 POST를
보내지 않는다. 응답이 끊기거나 UI를 재시작해도 GET만 자동 실행한다. `Resolve same
approval`은 사용자 클릭으로만 최초 요청을 그대로 보낸다. 다른 탭의 같은 seq 결과를
자기 승인으로 간주하지 않는다. 세션이 바뀐 기록은 자동 채택하지 않으며, 독립 확인
체크박스 뒤 `Clear tab receipt only`로 로컬 기록만 지울 수 있다. 미승인/불명/진행 중
복구는 CLI 창 재사용 전달이 기존 작업을 덮어쓰지 않도록 보류한다.

### 실패한 reader와 재열기

미리보기의 context는 성공한 오류 목록/selection이 아니라 신뢰된 **등록 ID**에서
유도한다. `phase:error`와 metadata 부재에서도 writer 권한이 있으면 진입할 수 있다.
편집기에도 별도 `recoveryReady`를 둔다. waive reader 실패/미적용 receipt는 일반
저장·import를 계속 막지만 파일 복구는 막지 않는다. 선택 없음은 허용하되 열린
편집·전송 잠금·진행 중 저장·로컬 미확인 승인은 먼저 해결해야 한다. 해당 편집기
모듈이 없을 때도 복구 승인을 진행하지 않는다.
승인된 복구 시 reader revision을 폐기하고 `drc_reopen_required`를 설정한다. 새 revision
조회로도 과거 waive 상태를 재사용할 수 없다. 복구 성공/실패와 reader 재설치는 별도다.
결과 확인 후 **Open DRC로 원본 DB/pack을 다시 선택**하고, 필요하면 기존 런처의
reviewer 권한을 명시적으로 재연결한다. `Reload review`/receipt GET만으로 재설치하지
않는다. 불명 작업은 먼저 같은 작업의 읽기 전용 확인을 끝내야 한다.

현재 CLI 기본 writer 시작은 explicit-waive reader이고, 두 링크라는 이유만으로 항상
초기 metadata 로드가 실패하지는 않는다. 반면 선택된 reviewer의 guarded reader는
두 링크를 거부한다. 이번 HTTP gate의 **초기 실패**는 별도 합성 rules 파싱 실패로
유도했으며, 이를 guarded-link 실패로 주장하지 않는다. 실제 공개된 CLI 권한 조합은
변경하지 않았다. 재연결 뒤에는 정상 guarded reader로 복구한 waive가 반영됨을 검사한다.

### 이번 단계 검증

- app-core review66 passed/private subprocess entry1 ignored(부모가 명시 실행), web
  review18 passed. managed proof가 note/waive·취소/완료에서 pack lease와 단일 borrow를
  유지하고, 원래 write 취소와 새 read-only 확인을 분리하는 회귀 추가.
- `validate_web_review_recovery.py`: 실제 Rust 게시 표식 → 별도 hard link로 gap 재구성
  → 승인 전 보존 → 명시 복구 → 동일 receipt → 과거 reader 거부 → picker 재열기와
  권한 재연결. note/waive bytes·inode·mtime·xattr·source를 보존하며 decoy stage는
  건드리지 않는다. 권한 없음/read-only/인증 없음, caller 경로 주입, 다른 token,
  일반 저장 token 혼용, 승인 전 mode 변경도 검사한다. 실제 SIGKILL은 §3과 구분한다.
- ES2017/UI 회귀: 미체크·만료·중복 클릭·context/권한/연결 변경·늦은 응답·저장소
  실패·ACK 유실·다른 세션·새로고침·명시 read-only 확인. 실제 DRC 컨트롤러에
  실제 Notes 편집기와 metadata 실패 등록을 붙인 panel 테스트, Notes/Waives의
  recoveryReady·failed-reader/transfer/unknown 분리 및 CLI 전달 모달 게이트도 포함한다.
- app-core/web clippy `--no-deps -D warnings` 통과. VFS 기존 경고2개 유지.
- 새 HTTP gate는 `validate_rust.sh`의 전체 실행과 `web` alias에 포함한다.

실제 Electron44.4.3 합성 회귀도 추가 실행했다. 기존 note/waive 승인 뒤 Chromium
표시 프로세스를 각각 강제 종료하고, 명시적 Recover View→동일 요청 resolve→파일
읽기 확인→Rust 종료까지 통과했다. 각 파일의 POST 수는1→1→2(최초 승인→crash→
명시 resolve)다. 새 복구 패널의 표시/펼치기도 확인하고 스크린샷을 직접 검토했다.
이 검사는 DOM 기반 조작/확인창 응답 주입이며 물리 입력 수용이 아니다. native
테스트는 `--multi`로 사용자 창을 재사용하지 않고, 실패 시에도 Rust 종료 뒤 QA 창을
종료한다. 첫 실패는 위의 권한 표시 오류, 초기 캡처는 직전 compositor 프레임이었다.
실제 펼침 위치와 두 RAF를 기다린 뒤 새 이미지를 확인했다.

최종 Electron 로그: `/private/tmp/floe-recovery-accepted-electron.log`(exit0).
합성 화면: `/private/tmp/floe-electron-review-HNEmlc/recovery-panel.png`(직접 시각 확인).
새 **link repair 버튼의 native end-to-end**와 NFS 강제 장애는 아직 별도다. 결과 불명
unlink의 native fault 모델은 §3, UI/active ledger/lease는 이번 로컬 회귀로 확인했다.
HTTP에서 NFS syscall 결과 불명을 실제로 유발했다고 주장하지 않는다.

### 최종 검증 기록

첫 전체 실행 `/private/tmp/floe-recovery-wire-battery.2dGSBr/full.log`는 새 동적
route factory의 권한 inventory 누락으로 실패했다. 정확한 owner route10개와
factory mount만 명시적으로 등록하고 AST 감사·cross-principal 거부 검사를 유지했다.
후속 `/private/tmp/floe-recovery-wire-final.Rbv8zu/full.log`는 exit0/ALL OK였으나
실행 중 UI 수정이 있었으므로 최종 고정 후보의 전체 통과 기록으로 대신하지 않는다.

최종 선택 없는 전체 실행 `/private/tmp/floe-recovery-accepted.Ajo8Yh/full.log`는
**exit0 / RUST VALIDATION: ALL OK**다. 실행 전에 제품·테스트 코드를 고정했으며
이후 변경은 이 문서 기록뿐이다. `CARGO_BUILD_JOBS=4`, 새 합성 TMPDIR에서
`sh tools/validate_rust.sh`를 실행했고 개발용 `.venv` 링크는 종료 시 제거했다.
새 복구 HTTP/UI·편집기 게이트와492개 권한 거부 검사가 이 실행에도 포함됐다.
마지막 KLayout 대조는 jobs1/8 각각13 PX + 2 phase-exact + 14 style 통과다.

고정 후보의 별도 웹 UI 검사 `/private/tmp/floe-recovery-accepted-ui.log`는 exit0이다.
Electron 재빌드 후 위 합성 native 검사도 exit0이며, 이미지까지 직접 확인했다.
`cargo fmt -p floe-app-core -p floe-web --check`도 exit0이다. 추가로 실행한
`cargo fmt --all --check`는 dbg/oasis/tiler의 기존9파일에서 실패했다. 해당 파일들을
`HEAD` blob과 byte 비교해 이번 변경이 아님을 확인했고 일괄 재포맷하지 않았다.
실패 로그는 `/private/tmp/floe-recovery-workspace-fmt.log`에 별도로 보존한다.

```text
full:     e0bea1d3c9d7a622ce3edbb3fb4fdeb6d41673369c471dd6deb3975925e78178
ui:       ff220342a99cb8a90a5ffec4824daeb27ae17f4993a44cf13210e0ad04ae37b1
electron: 22d01fa0d67372ca92254ed9fec3c3b52a71af5eafd469b7d90825a3c2aa475d
image:    2a4d3b98d76ce696b38663268e05c55ffc98bd1a7e219d0e6b408fafdc4d29e3
```

## 5. 다음 단계 / 남은 범위

1. 실제 Electron 창에서 명시 복구 UI, 복구 중 서비스 종료·새 세션의 새 승인 흐름을
   추가 검사한다. 자동 재열기는 구현하지 않았으며 현재는 위의 명시적 picker 경로다.
2. NFS 결과 불명과 승인 proof의 장기 유지/서비스 재시작을 함께 검증한다. reader
   startup의 explicit-waive/guarded 경로 차이는 별도 감사 대상으로 유지한다.
3. shared layer defaults에도 같은 link gap이 있다. 이번 DRC 표식은 **적용되지 않으며**
   해결됐다고 주장하지 않는다. 별도 source-bound 표식/권한 정책과 회귀가 필요하다.
4. 기존 일반 게시의 NFS link/rename 오류 및 결과 불명 receipt 경로 전체는 별도다.
   이번 `Uncertain` 처리의 범위는 명시적 복구 unlink다.
5. 실제 RHEL8.6/8.10·NFS lock/xattr/ACL·다중 client·장애/durability를 검증한다.
   로컬 SIGKILL은 원격 서버 장애나 전원 손실 수용을 대신하지 않는다.

전체 목표에는 이 게시 복구 연결 외에도 GTK 대비 G1/물리 입력·IME/DPI,
RHEL/ETX/Python-free 실행, 정식 offline 배포가 남는다. 원격 공유·유료 CI 보류는 유지한다.
