# 네이티브 호스트별 창 재사용

2026-09-22 사용자 결정. 상위 [데스크톱 계획](WEBUI_DESKTOP.ko.md),
[웹 전환 계획](WEBUI_PLAN.ko.md). **정책 확정 / 구현 전**이다.

## 확정된 계약

- 같은 사용자·DISPLAY의 기본 창을 재사용한다. macOS DISPLAY 부재와 X11
  screen suffix 정규화는 기존 Rust `instance::Key` 규칙을 재사용한다.
- WKWebView, Electron, 외부 브라우저는 서로 다른 instance namespace다.
  GTK의 기존 endpoint도 공유하지 않는다. 한 호스트의 인증 URL을 다른 호스트나
  새 CLI 프로세스에 전달하지 않는다.
- `--multi`는 기본 instance를 소유하거나 전달하지 않는 독립 창이다.
- trusted CLI가 명시한 새 파일·dependency root는 기존 창의 등록/보호 목록에
  추가할 수 있다. 웹 요청/guest가 임의 경로를 등록하는 권한은 추가하지 않는다.
  추가 등록이 자동 index·DRC 쓰기권한·기본값 게시 승인을 뜻하지 않는다.
- 빈 CLI 호출은 기존 창 표시 요청이다. 최초 빈 실행의 명시 폴더 선택은 유지한다.
  이미 실행 중인 창으로 전달될 호출에는 새 폴더 선택/새 인증 세션이 필요 없다.
- busy·시작 중·종료 중·버전 불일치·불명확한 전달 실패를 새 기본 창 생성으로
  대체하지 않는다. 기존 epoch/seq/receipt와 같은 요청의 확인 규칙을 유지한다.

## 구현 분할과 완료 근거

1. **공통 Rust launch 경계**: 호스트 식별 enum, native 창/폴더 선택 전 claim 또는
   forward, owner 수명 보존, 기존 `handoff`와 source registration 사용. 브라우저
   namespace와 기본 동작은 불변. 명시 process/DRC 옵션은 기존 parser의 독립
   workspace 의미를 보존하고, 기존 worker 옵션을 조용히 변경하지 않는다.
2. **두 호스트 연결**: WK 메인 스레드와 Electron의 private pipe에 고정 present/
   forwarded 결과를 연결한다. secondary는 빈 창을 남기지 않고 종료한다. 기존
   owner의 숨김/최소화를 복원하되 열린 승인/편집/복구를 자동 확정하지 않는다.
   새로운 bootstrap은 발급/재생하지 않는다. 합성 QA는 기본 instance에 붙지 않는다.
3. **게이트**: 같은 호스트 재사용, 서로 다른 host/DISPLAY 격리, `--multi`, 빈 호출,
   새 파일/공백 경로·고토 옵션, 같은 파일 캐시 유지, pending 모달, 시작/종료 경합,
   소유자 사망 후 새 epoch와 전달 불명 중복 방지. 소켓 단위뿐 아니라 실제 두
   프로세스와 native 창/worker 개수를 검증한다. queued ACK는 표시 완료가 아니다.

현재 `parse_embedded`는 항상 independent로 바꾸며, Electron pipe는 directory/
ready 두 이벤트만 지원한다. 이 문서만으로 재사용 기능이 생긴 것으로 세지 않는다.
기존 `handoff`의 source 준비는 socket callback 밖에서 수행되며, 새 구현도 파일
I/O나 색인을 UI 스레드/socket callback에 넣지 않는다. instance rendezvous는
기존 로컬 private 디렉터리에 두며, DRC 저장용 NFS와 섞지 않는다.

완료에는 위 세 단계와 두 호스트의 실제 수용이 모두 필요하다. 사용자의 이번
승인은 저장소 프로토콜 변경이나 원격 공유·RHEL 배포 승인을 대신하지 않는다.
