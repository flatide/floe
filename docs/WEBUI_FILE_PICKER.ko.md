# 서버 파일 선택기 (M4g-8)

`feature/webui`. 선택기·창 표시 설정 구현 기록은 [M4 §51–52](WEBUI_M4.ko.md), 전체 이관 잔여는
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
게시·출력 저장 승인은 아니다. M4g-8b부터 파일 메뉴의 새 열기는 서버가 확정한 마지막
depth·detail·thin(auto 포함)·frames·labels 선호·글꼴 크기를 유지한다. 새 잡덱은
full depth이며 실제 labels는 off지만 레이아웃으로 돌아오면 이전 선호를 복원한다.
다른 소스는 fit하고 단색·레이어 선택·스타일은 새 소스 기본값으로 시작한다. 같은
소스/모드/레벨 재선택은 depth·카메라·스타일·worker/캐시를 유지한다. Close 뒤에도
창 표시 선호는 남으며 새 세션에는 남지 않는다. 빈 창의 `--perf-baseline`도 첫 파일
선택에 적용된다(그 밖의 source 필수 표시 옵션은 기존 CLI 제약을 따른다).

CLI 전달은 파일 메뉴와 구분해 기존 명시 옵션/기본값을 적용한다. 잡덱의 labels=false
capability와 `--labels off` 선호도 구분해 다음 파일에서 라벨이 뜻밖에 켜지지 않는다.
표시 설정은 새 controller의 첫 렌더 전에 적용하며, 이전 revision이 바뀌면 전환을
거부한다. 색인 동의→자동 재열기 UX는 아직 별도다.

읽기가 진행 중이면 Cancel work를 누른다. 취소와 열기 제안 게시가 같은 잠금 아래
경합하므로, 취소가 먼저면 열지 않는다. 이미 게시됐으면 성공 receipt의 제안을
Dismiss해야 한다. metadata 등록만 완료된 소스는 목록에 남을 수 있지만 원본·캐시에
쓰지 않는다. 응답 유실 때에는 Check / retry same request로 **동일 요청**만 확인한다.
기록이 만료·손상됐거나 다른 요청이 같은 seq를 선점한 경우 자동 신규 선택은 하지
않는다. 현재는 새 세션으로 복구해야 한다.

## 열린 DRC와 SVRF metadata

M4g-24b/c·25는 같은 승인 폴더/파일 handle/요청 이력을 DRC에도 사용한다.
`Open DRC results…`는 현재 layout을 유지하며 읽기 전용으로 DRC를 교체한다.
DRC 전용 필터에서는 명시 ICE도 선택할 수 있다. 파일 선택만으로 reviewer 권한을
주지 않으며 `Reconnect launcher reviewer…`에서 런처가 고정한 범위만 별도 동의로
재연결한다. 원본 ASCII의 pack 생성은 별도 Build pack 승인 대상이다.

`Load SVRF metadata…`는 열린 DRC에 `floe-svrf-rules` JSON을 불러온다. All files에서
`.rules.json` 이름을 검색할 수 있다. 원본 deck/include parser가 아니며 다른 파일을
따라 읽거나 쓰지 않는다. 기존 geometry reader를 재사용하고 검증 성공 후 metadata와
query revision만 함께 바꾼다. 실패·취소는 이전 metadata/revision을 보존한다.
성공 시 type/filter/선택·미승인 preview는 초기화하되 카메라·레이어·reviewer 권한·
완료된 저장 이력은 유지한다. 매칭0건도 명시하며 자동 색인은 하지 않는다.
응답 유실은 위와 동일하게 GET/동일 요청 재확인으로 처리한다. 상세 자원·수명 계약은
[M4 §82](WEBUI_M4.ko.md)를 따른다.

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
