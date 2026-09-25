# xattr 없는 NFS의 리뷰·공유 기본값 저장

2026-09-25. 사용자가 **xattr 없는 NFS에서도 동작**하도록 요구하여, 앞선 NFS
보류 중 기본 읽기·저장 호환성은 재개했다. 실제 NFS mount/서버 장애·다중 client
검증은 수행하지 않는다. Electron과 외부 브라우저의 공통 Rust 저장 코어 변경이며
WKWebView 전용 호스트는 계속 동결한다.

## 동작

- `flistxattr` 최초 조회가 `ENOTSUP`/`EOPNOTSUPP`이면 OS xattr 목록 없음으로
  처리한다. 이미 열거된 속성을 읽지 못하면 여전히 오류다.
- Floe 자체 pack binding/recovery 표식의 `fsetxattr`가 위 오류를 반환하면 **숨김
  보조 파일**로 전환한다. `EACCES`, `EPERM`, `ENOSPC`, `EDQUOT`, `EIO` 등은
  미지원으로 위장하지 않는다. 기존 OS 권한·소유자·읽을 수 있는 ACL/xattr 보존
  실패도 계속 오류다. 관련 syscall 계약: [listxattr](https://man7.org/linux/man-pages/man2/listxattr.2.html),
  [setxattr](https://man7.org/linux/man-pages/man2/setxattr.2.html).
- 기존 note JSON, waive binary, `.layerprops` 내용·파일명은 바뀌지 않는다.
  기본 저장/자동 저장 opt-in/별도 복구 승인 및 읽기 전용 권한도 그대로다.
- 보조 파일이 있으면 xattr 지원 여부와 무관하게 확인한다. 두 방식의 같은
  속성이 다르면 거부한다. 이후 xattr가 가능한 곳에서 명시적으로 저장하면
  xattr 경로로 돌아갈 수 있다. 읽기만으로 생성·개명·마이그레이션하지 않는다.

## 두 파일의 일관성

보조 파일은 대상과 같은 디렉터리의
`.floe-meta-<target-leaf SHA-1>-<payload inode hex>.json`이다. caller가 경로나
이름을 지정하지 못한다. **현재 대상 inode에서 정확한 이름 하나만 유도**하며
디렉터리 검색이나 임의 metadata 선택은 하지 않는다.

내용은 v1, 정확한 target leaf, payload inode/길이/내용 SHA-1, pack binding과
recovery marker다. 최대64KiB, 고정 속성 조합, unknown field 거부, regular-file/
single-link/NOFOLLOW 제한과 읽기 전후 metadata 비교를 적용한다. SHA-1은 기존
코어와 동일한 변경 감지 수단이지 서명·인증이 아니다. 같은 UID의 악의적 수정에
대한 새 보안 경계로 주장하지 않는다.

게시 순서:

1. stable lock을 유지하며 private payload stage를 완성하고 sync한다.
2. 새 inode의 보조 파일을 보호 경로 검사 후 `O_EXCL`로 생성한다. payload와
   같은 접근 권한/읽을 수 있는 ACL을 적용하고 파일과 디렉터리를 sync한다.
3. 이전 target·pack·lock·stage·보조 정보를 다시 검사한 뒤 본 파일을 게시한다.
4. 게시 디렉터리 sync 성공 뒤에만 이전 snapshot의 정확한 보조 파일을 정리한다.
   sync 경고면 두 record를 모두 남긴다. 정리 실패는 완료를 실패로 뒤집지 않고
   작은 orphan을 남길 수 있으며, 정리 삭제의 별도 crash durability는 보장하지 않는다.

기존 target은 이전 inode의 record만 읽으므로 2번 뒤 종료해도 새 정보와 섞이지
않는다. 게시 전 확정 실패/취소는 현재 시도의 stage와 보조 파일만 정리한다.
게시 syscall 시도 뒤 오류/중단은 양쪽 증거를 유지하고 기존 결과 불명 fence를
적용한다. 성공이 증명되지 않은 쓰기를 자동 재시도하지 않는다.

신규 `link(stage,target)`와 `unlink(stage)` 사이 중단으로 두 링크가 남으면 기존
별도 승인 복구가 보조 record에서도 같은 marker를 읽는다. target/내용을 다시
쓰지 않고, 정확한 stage 링크만 제거한다. 복구 준비 후 보조 정보 교체/삭제도
충돌이다. 무표식·target 없는 stage의 자동 청소는 추가하지 않는다.

## 운영상 제한

- **보조 파일은 삭제 가능한 cache가 아니다.** 현재 target의 record가 사라지면
  신규 reader는 기존과 동일하게 unverified legacy 리뷰로 취급하고 채택에는
  별도 확인이 필요하다. 진행 중 snapshot은 변경을 거부하며 무표식 복구는 불가다.
  손상된 record가 *있으면* legacy로 무시하지 않고 오류를 낸다.
- pack binding 및 recovery identity는 기존 `(dev,ino,mtime,ctime)` 기반을 유지한다.
  다른 NFS client/재마운트에서 identity가 달라지면 거부할 수 있다. 이 변경은
  다중 host 간 리뷰 이식 설계가 아니다. 복사/이동/백업 복원은 기존 explicit
  import 경로로 재바인딩해야 한다. record만 복사해 강제 채택하지 않는다.
- 본 파일의 원자적 rename/link, stable flock, 권한 및 sync 지원은 여전히 필요하다.
  NFSv3/v4 ACL, root squash, client attribute cache, 서버 전원 손실 및 비협조
  writer와의 원자적 CAS 보장은 별도다. mount 옵션·서버 설정을 변경하지 않았다.
- xattr fallback 저장은 작은 보조 파일 write+sync와 payload hash read를 추가한다.
  waive의 기존 게시 증명용 hash는 재사용하여 동일 파일의 중복 read를 피한다.
  매번 정상 교체한 이전 record는 정리하므로 자동 저장 때 계속 누적하지 않는다.
  중단·sync 경고·정리 오류의 orphan은 디렉터리 검색으로 자동 삭제하지 않는다.
- 기존 브라우저 권한·임의 경로 금지·동적 입력 보호를 유지한다. 보조 파일 충돌은
  덮어쓰지 않고 오류다. 사용자 설계/리뷰의 실제 저장으로 시험하지 않았다.

## 자동 검증

Rust test 전용 thread-local syscall 모델로 list=ENOTSUP 및 list=빈 목록 +
set=EOPNOTSUPP를 각각 주입한다. 제품 환경변수/권한 우회 스위치는 추가하지 않는다.
note/waive/default 신규·교체·재열기, pack 불일치, waive 적용, mode 보존,
취소/이전 증거 보존, 손상·symlink·hardlink·준비 후 교체 거부, 동적 입력 보호,
sync 실패 및 별도 복구를 검사한다. 실제 Stage link/unlink 경계의 unwind는
서비스 kill/NFS 장애가 아닌 합성 fault 모델이다.

고정된 최종 코드에서 `cargo test --offline --locked -p floe-app-core --lib`는
322 passed/8 ignored, `-p floe-web --lib`는127 passed/3 ignored다. ignored에는
개발 오라클과 부모가 명시 실행하는 private subprocess entry가 포함된다.
app-core/web scoped fmt 및 clippy `--all-targets --no-deps -D warnings`도 통과했다.
tiler의 기존 경고1개, VFS의 기존 경고2개는 수정하지 않았다.

초기 전체 코어 실행에서 기존 합성 `actual_link_gap_and_repair_death...`가
`prepare_recovery`의 Busy로1회 실패했다. 단독 및 후속 전체 재실행은 통과했지만
그 일회성 실패의 원인을 확정했다고 주장하지 않는다. 초기 clippy에서 발견한
새 record의 enum 크기 증가와 테스트 표현은 수정하고 최종 검사에 포함했다.

최종 로그는 `/private/tmp/floe-no-xattr.kKhmUr/`의 `*-accepted.log`다.

고정 후보의 `sh tools/validate_rust.sh --only
layer_defaults,drc_review,web_drc_notes,web_drc_waives,web_read_reviewer,web_review_recovery,web_ui`
도 **exit0 / ALL OK**다. GTK와 동일한20개 shared-default target/bytes,
34개 managed 리뷰 게시·28개 export, 웹 승인/재열기/별도 복구 및 Node UI 회귀를
포함한다. 선택 배터리이지 전체 렌더러 배터리 통과 주장은 아니다. 임시 `.venv`
링크는 제거했으며 원본 개발 venv와 사용자 설계 파일은 변경하지 않았다.

Electron 서비스의 `cargo check --offline --locked` 및
`cargo build --offline --locked --release`도 exit0이며 기본 경로의 실행 파일을
갱신했다. Electron GUI는 실행하지 않았고 `.app`/portable 번들 재포장은 하지 않았다.
실제 RHEL/ETX 및 xattr 없는 NFS의 저장·다중 client·장애 수용은 남는다.
