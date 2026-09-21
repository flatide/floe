# Rust 서비스 종료·재시작 후 리뷰 재조회

2026-09-22. [G4 잔여 감사](WEBUI_G4_AUDIT.ko.md)의 서비스 프로세스 장애에 대한
최소 실행 근거다. [Chromium만 종료하는 검사](WEBUI_ELECTRON_REVIEW.ko.md)와
[저장소 내부 게시 훅 검사](WEBUI_REVIEW_PROCESS_CRASH.ko.md) 사이를 연결한다.
제품 동작·게시 방식·인증·복구 정책·버전은 변경하지 않았다.

## 무엇을 종료하는가

`tools/validate_electron_service_review.cjs`는 Node 개발 하네스이며 인자를 받지 않는다.
새0700 폴더마다 기존과 같은 TOP40×30µm OASIS와2오류/1규칙 DRC를 만들고,
실제 Rust indexer로 `.ice`와 `.tray`를 생성한다. 기존 고객/합성 파일을 입력받거나
이전 인증을 디스크에서 읽지 않는다. 테스트 산출물은 새 폴더에 남긴다.

생산 `ServiceClient`를 통해 실제 `floe-electron-service`를 실행한다. 서비스/worker는
빈 환경·빈 PATH와 명시 index/renderd·private TMPDIR만 받고, Python/Node/shell을
런타임에서 호출하지 않는다. 하네스의 Node/`ps`는 제품 의존성이 아니다. Rust 서비스의
private pipe에서 받은 일회용 인증만 메모리에서 교환하며, URL/쿠키/CSRF/승인 토큰·
요청/응답 전문을 진단 로그나 별도 인증 파일에 남기지 않는다. 승인한 합성 메모의
sidecar 쓰기는 검증 대상이다. GUI·OS 클립보드·외부 서버는 사용하지 않는다.

note/waive 각각 다음2경계를 검사한다(총4회 SIGKILL +4회 명시 재시작).

| 경계 | 종료 전 상태 | 재시작 후 기대값 |
|---|---|---|
| prepared | snapshot→prepare만 수행, approve POST 없음 | sidecar 부재, 초안은 자동 저장되지 않음 |
| published | approve POST202/queued 수신 후 실제 sidecar의 완전한 바이트·0600·nlink1 확인. terminal receipt GET 없음 | 게시된 값의 디스크 재조회 가능; 이전 프로세스의 receipt는 복원되지 않음 |

서비스가 ready이고 실제 뷰가 idle일 때 직접 자식 PID를 숫자로만 수집한다.
**하네스가 spawn한 서비스의 child handle에만 SIGKILL**을 보낸다. 종료 결과는
`code=null, signal=SIGKILL`이어야 하며, 이전에 확인한 자식 worker들도15초 안에
사라져야 한다. 다른 프로세스의 명령줄/환경을 수집하거나 임의 PID를 종료하지 않는다.

## 고정한 복구 계약

1. 종료 전후 sidecar 및 합성 입력의 바이트/크기/권한/mtime가 변하지 않는다.
2. 새 서비스는 새 인증과 reader로 시작한다. 이전 인증은401이다.
3. review ledger의 `last_seq=0`, 서버 autosave=false. 이전 `#1` 조회는
   **410/operation_expired**이고, 현재 context에 이전 승인 토큰을 넣어도
   **410/review_expired**다. 거부한 요청이 seq를 소비하거나 디스크를 바꾸지 않는다.
4. fresh snapshot은 게시 전이면 없음, 게시 후이면 한글 메모 또는 waive를 읽는다.
   조회 snapshot을 revoke하고 **새로 prepare/approve**해야 다음 변경이 저장된다.
   메모는 다른 본문, 이미 waive된 항목은 unwaive하여 실제 변경을 확인한다.
5. 후속 저장은 terminal succeeded/published/outcome_unknown=false와 독립 디스크
   바이트 검사로 대조한다. 합성 원본·cache·pack은 불변이고 입력 디렉터리의 추가
   산출물은 해당 sidecar와 lock뿐이다. EOF 종료143 및 그 서비스의 worker 종료도 확인한다.

초기 receipt 상태에 대한 하네스의404 가정은 실패했다. Rust의
`drc/review/http.rs::operation`과 오류 매핑을 확인해 기존 **410 만료 계약**으로
수정했으며 제품 응답을 바꾸지 않았다. 실패 기록은
`/private/tmp/floe-service-review-first.log`, `floe-service-review-phase.log`다.

## 실행과 검증

```sh
# release index/renderd + 같은 소스의 Electron Rust service가 선행한다.
(cd electron/service && cargo build --offline --locked)
node tools/validate_electron_service_review.cjs
node --test tools/electron-review-fixture.test.cjs tools/validate_electron_review.test.cjs \
  electron/service-client.test.cjs
```

기존 `FLOE_INDEX_BIN`, `FLOE_RENDERD_BIN`, `FLOE_ELECTRON_SERVICE_BIN`의 명시 override는
허용하되 무효 값은 fallback하지 않는다. 입력 파일/리뷰어/PID override는 없다.
index 명령120초, 서비스/UI가 아닌 HTTP 상태 준비30초, HTTP8초, 종료/worker15초
상한이다. timeout을 늘리거나 실패한 저장을 재승인해서 통과시키지 않는다.

- macOS 실제4조합 통과: `floe-service-review-matrix.log`.
- live worker 종료 검사를 포함한 최종본 통과: `floe-service-review-final.log`.
- terminal receipt 미소비 단언을 포함한 소스 재검사: `floe-service-review-receipt.log`.
- pure fixture/관찰기/ServiceClient **15개 통과**: `floe-service-review-unit.log`.
- 기존 `sh tools/validate_rust.sh --only web_drc_notes,web_drc_waives`의 실제 HTTP/
  Python 비교 오라클 통과: `floe-service-review-http-gates.log`. 임시 venv 링크는
  종료 후 제거했다. 전체 배터리 재실행/통과를 뜻하지 않는다.
- 공유 fixture를 사용하는 기존 실제 Chromium2회 crash/reload/resolve도 통과:
  `floe-review-shared-fixture-regression.log`. 모두 `/private/tmp/`의 개발 로그다.
  공통화 전후 fixture의 전체 바이트가 같음을 기존 HEAD와 직접 비교했고 두 SHA-256을
  pure gate에 고정했다. 일반 Electron gate에는 **pure 검사만** 추가했다.
  서비스 강제 종료·리뷰 쓰기를 하는 실제 검사는 위 명령으로 별도 실행한다.

## 완료로 세지 않는 것

이 검사는 완전한 게시 바이트와 nlink1을 본 **뒤** 종료하므로 DRC-PUB-01의
link/unlink 사이 창을 시험하거나 해결하지 않는다. fsync/전원 손실·ENOSPC·NFS 장애,
실제 Electron 서비스 오류창/브라우저의 재시작 입력·미저장 초안 안내도 별도다.
server의 `autosave=false`를 브라우저 opt-in 복구 검증으로 확대하지 않는다.
프로세스 SIGKILL 뒤 모든 private 임시 **디렉터리**가 회수된다는 보장도 추가하지 않는다.

전체 goal에는 DRC-PUB-01 파일시스템/복구 정책, 더 넓은 UI·저장소 장애 및 G1/G4,
RHEL8.6/8.10+ETX·Python-free Linux 실행, 정식 배포 수용이 남는다. 전체 배터리의
기존 native oracle30초 기동 timeout은 미해결이며 이번 선택 검사들을 전체 green으로
합산하지 않는다. 원격 공유·유료 CI·캐시 수명주기·조건부 M5의 보류도 유지한다.
