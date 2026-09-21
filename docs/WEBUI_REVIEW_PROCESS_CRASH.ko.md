# DRC 저장 프로세스 종료 — 게시 경계와 미해결 link gap

2026-09-22. [G4](WEBUI_G4_AUDIT.ko.md), [Electron 표시 프로세스 장애](WEBUI_ELECTRON_REVIEW.ko.md)의
후속이다. 이번 변경은 Rust test-only 모듈이며 제품 게시/보안/복구 정책은 그대로다.
**DRC-PUB-01이 열려 있으므로 저장 프로세스 장애 전체를 완료로 판정하지 않는다.**

후속 [전체 Rust 서비스 종료·재시작](WEBUI_REVIEW_SERVICE_CRASH.ko.md)은 승인 전과
완전한 게시 후의4조합에서 새 reader·인증/receipt 분리와 후속 저장을 확인한다.
아래 내부 link gap이나 NFS/전원 손실 수용을 대신하지 않는다.

## 실제 SIGKILL 12조합

`app-core/src/drc/review/store/process_crash_tests.rs`가 실제 `Draft::publish_using`의
기존 private 훅을 이용한다. 새 제품 환경변수/HTTP 장애 API는 만들지 않았다.
부모가 같은 테스트 실행 파일의 정확한 ignored child 하나만 실행한다. 자식이
자신의 새 합성 pack/고정 reviewer/임시 폴더를 만들고 훅 진입 표지를 flush한 뒤
stdin에서 멈추면 부모가 **그 소유한 자식만 SIGKILL하고 wait/reap**한다.
준비 표지 제한은30초이며 PID 검색·프로세스 그룹 종료·자동 재시도는 없다.

| 축 | 경우 |
|---|---|
| 저장 | note / waive |
| 대상 | 신규 생성 / 기존 sidecar 교체 |
| 종료 | stage 파일 sync 후·게시 전 / 게시 완료 후·directory sync 전 / directory sync 후·receipt 반환 전 |

2×2×3=12조합에서 다음을 확인했다.

- 게시 전이면 기존 note/waive 또는 대상 부재를 유지한다. 게시 후이면 완전한 새 값을 읽는다.
- 훅 진입 시 관측한 대상 바이트와 SIGKILL 후 바이트가 같다. 새 reader가 pack binding,
  note 2개 멤버 및 waive 상태를 다시 검증한다. 합성 원본 pack은 바이트 불변이다.
- 기본0600 sidecar 권한을 유지하고 flock은 프로세스 종료로 풀린다. 새 snapshot에서
  **새로 승인한** 후속 저장이 성공한다. 죽은 요청/receipt를 재생하는 검사는 아니다.
- 게시 전 강제 종료는 완성된 임시 stage를 남긴다. 이 fixture에서는0600이다.
  읽기/후속 저장은 그 stage를 채택하거나 지우지 않는다. 테스트 폴더만 검증 뒤 정리한다.
  임의 기존 대상의 ACL/모드를 보존하는 경우까지 항상0600이라고 일반화하지 않는다.

이 근거는 로컬 macOS 파일시스템에서의 프로세스 종료다. 전원 손실, 실제 디스크/NFS
고장, HTTP 서비스 재기동 후 receipt 복구 또는 Linux 실행을 통과했다는 뜻은 아니다.

## DRC-PUB-01 — 신규 게시의 두 hard link 창 (미해결)

`layer_defaults::Stage::commit`의 신규 대상 경로는 기존 파일을 덮어쓰지 않기 위해
`linkat(stage,target)` 다음에 `unlinkat(stage)`를 호출한다. 기존 대상 교체는
`renameat`이므로 이 두-syscall 창과 구분한다.

신규 링크는 성공했으나 임시 이름 제거 전에 프로세스가 죽으면 **완전한 새 파일이
두 이름/nlink=2로 남는다**. DRC `Capture::read`는 보안 계약상 단일 링크 파일만
허용하므로 이후 snapshot/편집이 거부된다. 파일 내용 손실을 재현한 것은 아니지만,
저장이 끝나 보이는 파일을 곧바로 다시 읽을 수 없는 복구 결함이다.
같은 Stage를 쓰는 shared layer defaults에도 코드상 같은 경계가 있다.

별도 두 테스트는 게시 전 훅에서 `hard_link` 한 번을 수행해 **그 중간 파일시스템
상태를 재구성**하고 실제 자식을 종료한다. note/waive 모두 같은 inode의 두 이름,
`nlink=2`,0600, fresh snapshot의 `InvalidInput`을 확인했다. 이는 Stage 내부
syscall 사이에 직접 breakpoint를 넣은 실험은 아니며, 12개의 제품 훅 검사와
분리한다. 테스트 이름/출력의 `unresolved`와 `REVIEW CRASH GAP`을 유지했다.
이 진단 테스트가 green인 것은 거부 상태를 재현했다는 뜻이지 결함을 고쳤다는 뜻이 아니다.

### 수정 후보와 현장 호환성 결정

1. **원자적 no-replace rename**: 두 이름을 남기는 사용자 공간 창을 제거하면서
   기존 대상 덮어쓰기 금지를 유지한다. Linux `RENAME_NOREPLACE`는 파일시스템
   지원이 필요하다([Linux man-pages](https://man7.org/linux/man-pages/man2/rename.2.html)).
   미지원이면 명시 실패시켜야 하며 같은 link/unlink로 조용히 돌아가면 결함은 남는다.
2. **명시적 orphan 복구**: 현재 방식의 호환성을 유지하려면 대상/pack/reviewer와
   정확한 sibling inode를 검증하는 별도 복구 승인·프로토콜이 필요하다. 읽기만으로
   디렉터리를 스캔·개명·삭제하거나 nlink 보안 제한을 없애는 방식은 채택하지 않는다.

실제 RHEL 저장 경로가 NFS인지 로컬 XFS/ext4인지 미확인이다. 예를 들어
[upstream Linux4.18 NFS `nfs_rename`](https://github.com/torvalds/linux/blob/v4.18/fs/nfs/dir.c#L1873-L1883)은
nonzero rename flags를 거부한다. 이것을 현장 Red Hat 패치 커널의 실행 결과로
간주하지는 않지만, 로컬 APFS 통과만으로 무조건 치환할 수 없다는 구체적 근거다.
파일시스템 확인/정책 선택 전에는 제품 기본 동작이나 실패 호환성을 변경하지 않는다.

## 실행과 증거

```sh
cd rust
cargo test --offline --locked -p floe-app-core --lib \
  drc::review::store::tests::process_crash:: -- --nocapture
cargo test --offline --locked -p floe-app-core --lib drc::review:: -- --nocapture
cargo fmt --check -p floe-app-core
cargo clippy --offline --locked -p floe-app-core --lib --tests --no-deps -- -D warnings
```

- 최초 제품 훅12조합 통과: `/private/tmp/floe-review-process-crash.log`.
- 관련 review 스위트57 passed/child entry 1 ignored:
  `/private/tmp/floe-review-process-crash-suite.log` (gap 진단 추가 전).
- 최종 선택 검사2 passed/child entry 1 ignored: `/private/tmp/floe-review-link-gap.log`.
  내부12조합 통과 + 별도2개 재구성 상태에서 미해결 거부 확인.
- fmt 및 최종 app-core clippy `--lib --tests --no-deps -- -D warnings` 통과:
  `/private/tmp/floe-review-link-gap-clippy.log`. 기존 의존 VFS 경고2개는 변경하지 않았다.
  전체 배터리의 기존 `layer_defaults`30초 timeout은 별도 미해결이다.
  선택 검사 합산으로 전체 green을 주장하지 않는다.

전체 목표의 잔여는 DRC-PUB-01 및 실제 서비스/저장소 장애 복구, G1/G4 확대
수용·물리 입력/IME/DPI, RHEL8.6/8.10 ETX/Python-free Linux 실행, 정식 배포다.
원격 공유·유료 CI 보류 및 열린 인덱스 hot-reload의 후속 정책 범위는 유지한다.
