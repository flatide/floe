# DRC-PUB-01 — 명시적 게시 복구 코어

2026-09-23. [프로세스 종료 분석](WEBUI_REVIEW_PROCESS_CRASH.ko.md)의 후속이다.
이번 단계는 **Rust DRC note/waive 저장의 표식과 복구 코어**다. HTTP/actor/UI 승인
연결은 아직 없으므로 사용자가 앱에서 복구할 수 있는 상태나 DRC-PUB-01 전체
완료로 세지 않는다. WKWebView는 동결 상태를 유지한다.

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

## 3. 로컬 검증

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

## 4. 다음 단계 / 남은 범위

1. actor의 admission·동시 저장 배제·pending opaque operation 수명과 owner 전용
   preview/approve/reconcile wire를 연결한다. autosave/read-only/guest에는 복구 권한을
   부여하지 않는다. 파일 목록이나 경로 입력을 추가하지 않는다.
2. 초기 review snapshot 실패 상태에서도 등록된 writer의 복구 미리보기로 진입할
   UI를 만들고, 결과 불명·서비스 종료 후 재조회·명시 승인·reader 재설치를 검증한다.
   특히 `web/src/drc/mod.rs`의 초기 guarded waive 로드 자체가 두 링크 때문에
   실패할 수 있다. 성공한 오류 목록/metadata만을 전제한 버튼으로는 부족하다.
   신뢰된 실패 등록의 identity와 기존 writer 권한을 유지하되 임의 pack 경로를
   받아 우회하는 API를 만들지 않는 것이 후속 wire의 필수 조건이다.
3. shared layer defaults에도 같은 link gap이 있다. 이번 DRC 표식은 **적용되지 않으며**
   해결됐다고 주장하지 않는다. 별도 source-bound 표식/권한 정책과 회귀가 필요하다.
4. 기존 일반 게시의 NFS link/rename 오류 및 결과 불명 receipt 경로 전체는 별도다.
   이번 `Uncertain` 처리의 범위는 명시적 복구 unlink다.
5. 실제 RHEL8.6/8.10·NFS lock/xattr/ACL·다중 client·장애/durability를 검증한다.
   로컬 SIGKILL은 원격 서버 장애나 전원 손실 수용을 대신하지 않는다.

전체 목표에는 이 게시 복구 연결 외에도 GTK 대비 G1/물리 입력·IME/DPI,
RHEL/ETX/Python-free 실행, 정식 offline 배포가 남는다. 원격 공유·유료 CI 보류는 유지한다.
