# 서버 파일 선택기 (M4g-8)

`feature/webui`. 구현 기록은 [M4 §51](WEBUI_M4.ko.md), 전체 이관 잔여는
[M0 §2.9](WEBUI_M0.ko.md)를 따른다. GTK 기본 실행기·실칩 브랜치는 바꾸지 않는다.

```sh
floe2-web                         # 실행 폴더를 탐색하는 빈 창
floe2-web view design.oas         # 소스 부모 폴더에서 추가 파일 선택
floe2-web view --multi --root /approved/designs --root /approved/masks
```

Browse server files는 **서버**의 허가된 폴더만 보여 준다. 브라우저가 실행 중인 Mac의
파일을 업로드하는 기능이 아니다. source 부모와 명시 `--root`가 대상이며 둘 다 없으면
실행 폴더를 쓴다. 전체 `/`는 거부하고 home을 자동 추가하지 않는다. roots는 세션
시작 때 고정된다. 다른 폴더가 필요하면 `--root`로 새 독립 세션을 시작한다. CLI 전달로
새 소스를 등록해도 파일 선택기의 탐색 권한을 자동 확장하지 않는다.

폴더 클릭/상단 breadcrumb로 이동하고 이름 부분 문자열로 검색한다. OASIS, jobdeck,
All files 필터를 제공한다. `PATTERN01.TE`처럼 확장자 없는 마스크 소스는 All files에서
선택한다. metadata 검사에서 OASIS/.jb 여부를 확인하며 GDS/gzip은 아직 거부한다.
폴더 우선·대소문자 무시 정렬, 128행 페이지다. dotfile, `.floe`, `.ice`, symlink와
특수 파일은 탐색하지 않는다. `.floe.lock` 같은 일반 파일은 All files에서 보일 수
있지만 레이아웃으로 열 수는 없다. symlink와 읽지 못한 이름은 개수를 표시한다.

파일 선택은 읽기 전용 등록 후 기존 열기 제안을 만든다. 다중 레벨 덱은 선택을
확인하고, 미색인 파일은 오류와 별도 Index 조작을 제공한다. 그 자체가 색인·설정
게시·출력 저장 승인은 아니다. 현재 새 열기는 CLI 기본값(layout depth0 / deck full,
medium, frames on, layout labels on / deck off, thin auto)을 사용한다. 마지막 표시
설정을 보존하는 GTK 파일 선택 동작 및 색인 동의→자동 재열기 UX는 후속 parity다.

읽기가 진행 중이면 Cancel work를 누른다. 취소와 열기 제안 게시가 같은 잠금 아래
경합하므로, 취소가 먼저면 열지 않는다. 이미 게시됐으면 성공 receipt의 제안을
Dismiss해야 한다. metadata 등록만 완료된 소스는 목록에 남을 수 있지만 원본·캐시에
쓰지 않는다. 응답 유실 때에는 Check / retry same request로 **동일 요청**만 확인한다.
기록이 만료·손상됐거나 다른 요청이 같은 seq를 선점한 경우 자동 신규 선택은 하지
않는다. 현재는 새 세션으로 복구해야 한다.

## 자원과 파일 변경

한 작업자, 한 active 요청, 최대100,000개 검색 결과/1,000,000개 조사 항목,
2,048개 entry handle, 깊이64/경로4,096 bytes, 응답 이력32개다. 한도 초과는
부분 목록을 완전한 것처럼 보여 주지 않고 오류로 반환한다. 검색 결과 한도는 이름
필터로, 조사 항목 한도는 더 작은 승인 root로 줄인다. 큰 목록은 한 번 스캔·정렬하고
페이지마다 전체를 다시 읽지 않는다. 자원 admission은 1 CPU slot + 192 MiB이며
프로세스 RSS의 강제 상한은 아니다. render/DRC/index와 총16 slots 안에서 나눈다.

descriptor-relative `openat`/`fstatat`, `O_NOFOLLOW`, root/ancestor inode 확인으로
디렉터리 교체를 통한 symlink 탈출을 거부한다. 선택한 inode·크기·mtime·ctime는 등록
전후 다시 검사한다. 읽은 목록이 바뀌면 Refresh가 필요하다. 이는 이후 모든 native
경로 I/O를 불변 dataset revision으로 만드는 OS sandbox가 아니다. 동시 외부 변경의
전체 수명주기 정책은 기존 별도 과제다. NFS의 커널 내부 blocked syscall은 flag로
중단할 수 없으므로 현장 지연·종료 수용을 로컬 테스트로 대신하지 않는다.
