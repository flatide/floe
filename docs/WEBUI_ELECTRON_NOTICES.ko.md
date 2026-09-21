# Electron 고지 창 — 로컬 구현·검증

2026-09-22. [Electron 비교판](WEBUI_ELECTRON.ko.md)의 **Help → Open Source
Licenses…**를 연결했다. [계획 §8](WEBUI_PLAN.ko.md)의 runtime 고지 표시 항목이다.
Electron 비교 shell 버전은0.1.1이며, Rust worker/제품0.12.185는 변경하지 않았다.
WKWebView 앱·렌더 규칙·클립보드 정책은 변경하지 않는다.

## 범위와 경계

- 실행 중인 Electron의 실제 executable 위치로 runtime root를 유도한다.
  macOS는 공식 archive의 `.app` 바깥, Linux는 executable 인접 위치다.
  **`LICENSE`와 `LICENSES.chromium.html` 두 이름만** 읽는다. URL에서 파일 경로를
  만들거나 저장소/사용자 디렉터리로 fallback하지 않는다.
- 고지마다32MiB 상한, 일반 파일·단일 link·UTF-8·읽기 전후 크기/수정 시각 검사를
  적용하고 symlink/빈 파일/특수 파일을 거부한다. 누락·변경은 창 안에서 명시 오류다.
  실제 pinned macOS Chromium 고지는20,111,209bytes이며 잘라서 표시하지 않는다.
- 고지는 메뉴를 열고 문서를 선택했을 때 비동기적으로 읽는다. 레이아웃 로드·pan·
  매 프레임 경로에는 파일 조회/HTML 파싱을 넣지 않는다. 큰 고지 문서의 파싱/메모리
  비용이 없는 것은 아니며, 이는 명시 고지 창에서만 발생한다.
- 새 non-persistent session을 사용하며 소유 뷰의 쿠키·인증·권한을 공유하지 않는다.
  `https://floe-notices.invalid`는 **그 session 내부 handler로만** 응답하고 서버나
  DNS에 연결하지 않는다. 고정된3개 GET/main-frame 경로 외 요청·탐색, 모든 새 창,
  download·permission·webview를 차단한다. 외부 고지 링크도 자동 브라우저로 열지 않는다.
- Node/preload 없음, sandbox/context isolation 유지, JavaScript 자체 비활성화와
  응답 CSP를 함께 적용한다. Chromium 고지의 원문은 유지하되 `chrome://` stylesheet를
  읽지 않으므로 줄바꿈/본문 표시용 로컬 CSS와 목차 링크만 더한다.
- modal 고지 창을 닫으면 원래 세션으로 돌아간다. 그동안 다른 native 명령의 중복
  진입·clipboard grant를 막고, 소유 세션 종료 시 고지 창도 정리한다.
  OS 클립보드 읽기/쓰기, 설계·리뷰·기본값 파일 접근은 추가하지 않았다.

고정 session의 [protocol handler](https://www.electronjs.org/docs/latest/api/protocol),
[WebPreferences](https://www.electronjs.org/docs/latest/api/structures/web-preferences),
[보안 경계](https://www.electronjs.org/docs/latest/tutorial/security)를 기준으로 한다.
Rust/font/toolchain 고지는 비교 번들의 `NOTICES/`에 별도로 있으며, 이번 창이 그
전체를 통합하거나 라이선스·코덱·대응 소스·법적 배포 조건을 판정한 것은 아니다.

## 검증

```sh
node --test electron/notices.test.cjs
# 명시적인 실제 창 검사. 고정 runtime + 새 private QA 폴더만 사용한다.
"$FLOE_ELECTRON_BIN" tools/validate_electron_notices.cjs
sh tools/validate_rust.sh --only web_portable
```

- 고지 unit5개: root/route 경계, 원문 보존/escaping, 잘못된 파일/상한,
  session 권한·요청/창 차단과 닫힘 정리. 기존 host 회귀 포함 **Node46개 통과**.
- packager **unit11개**, fmt, all-targets/no-deps clippy 통과. 명시 APP_FILES에
  고지 모듈을 추가했고 독립 번들 closure/launcher2개와 `web_portable` gate 통과.
- macOS arm64/Electron44.4.3 실제 창: Electron 고지 표시, Chromium 첫 항목과
  다수 Apache 본문의 native 검색, 프로그램식 JS 실행 거부, 외부 HTTPS/file 요청의
  `ERR_BLOCKED_BY_CLIENT`, script 포함 합성 고지의 비실행, 누락 고지 오류·닫힘 확인.
  텍스트 준비 후 캡처한 Electron 라이선스 화면도 직접 확인했다.
- 실제 제품의 빈 합성 owner 세션에서 Help 메뉴 handler를 호출해 고지 창의 별도
  session과 닫힘을 확인했다. 이어 원래 인증된 뷰의 탐색 차단·기본 취소·정상 종료와
  Rust service join 통과. 물리 메뉴 클릭·현장 OS 입력 수용과는 구별한다.

로그: `/private/tmp/floe-notices-host-unit.log`, `floe-notices-packager-unit.log`,
`floe-notices-packager-clippy.log`, `floe-notices-portable-gate.log`,
`floe-electron-notices-text-ready.log`, `floe-notices-owner-final.log`,
`floe-notices-owner-bounded.log` (메뉴 load 대기도30초로 고정한 최종본).
고지 캡처: `/var/folders/1v/1wct59qn2dbc457m8msmb5c80000gn/T/floe-notices-qa-vKLPgQ/electron.png`.
클립보드 재검사는 하지 않았다. 이전 [클립보드 검사](WEBUI_ELECTRON_CLIPBOARD.ko.md)의
성공 기록을 확인했으며, 이전 클립보드를 읽거나 다시 덮어쓰지 않았다.

초기 QA 실패도 구분한다. JavaScript-off는 QA의 DOM 실행까지 막았고, download
거부는 QA의 savePage 출력도 막았다. 권한을 풀지 않고 native title/findInPage로
검사했다. 즉시 capture는 UnknownVizError, compositor 첫 캡처는 이전 목차였다.
**동일30초 상한 안에서 실제 문서의 native 검색 준비**를 기다린 후 통과했다.
메뉴 click wrapper는 비동기 완료를 반환하지 않아 닫힘 직후 panel 검사도 먼저
실패했다. 실제 panel 해제 완료를 기다려 재검증했으며 제품 승인/시간 제한은 바꾸지
않았다. 앞선 실패 로그는 `floe-electron-notices-{first,phase,final,native,visible,ready,present}.log`,
`floe-notices-owner-smoke.log`에 남아 있다.

## 남은 범위

이번 결과는 고지 메뉴 구현과 로컬 회귀 근거이며 정식 Electron 채택이나 전체 goal
완료가 아니다. 전체 배터리는 마지막 `layer_defaults`30초 기동 timeout 실패를
유지한다. 이번에는 영향 범위 `web_portable`만 실행했으며 전체 green으로 세지 않는다.
G1 동일 조건 성능/실제 입력·표시, G4 추가 UI·저장 복구와 DRC-PUB-01 파일시스템
정책, RHEL8.6/8.10+ETX/Python-free Linux, 고지·코덱·대응 소스/업데이트·서명·배포
수용은 남아 있다. M5 조건부·원격 공유·캐시 수명주기의 사용자 보류도 유지한다.
