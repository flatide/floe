# M2b-4c — 공유 권한 목록 게이트와 수용 근거 대조

2026-09-17, `feature/webui`, M2b-4b5(`cead949`) 이후.
[공유 요구 SH-01~10](WEBUI_M2_SHARING.ko.md), [전체 잔여](WEBUI_G4_AUDIT.ko.md).

제품의 endpoint·권한·렌더 동작은 변경하지 않는다. **로컬 회귀 검증 강화이며 공유
전체 수용 완료가 아니다.** 기본 off, `--local-sharing` opt-in, loopback-only를 유지한다.
노트 본문/SVRF·게스트 파일 탐색/쓰기·서버 export·실제 원격 공개를 추가하지 않는다.

## 1. SH-02: 구현과 함께 변해야 하는 권한 목록

기존 native 테스트는 대표 요청과 수명 경합을 검증했지만 새 route/명령이 추가될 때
전체 권한 대조표에서 빠지는 것을 자동으로 막지 못했다. 다음 두 검사를 결합한다.

- [검토된 목록](../tools/web_permissions.json): HTTP method/path **162개**, wire 명령
  **32개**(owner/guest WS와 DRC Request), mount/factory **19개**. handler/variant의
  소스 위치·이름과 authority를 명시하며 CI가 새 항목을 자동 승인하지 않는다.
- [Rust AST 검사](../rust/web/tests/permissions_inventory.rs): web/src의 route와 Control,
  DRC Request를 `syn`으로 읽어 목록과 정확히 대조한다. 주석/문자열을 route로 세지
  않고 다중 행·메서드 체인·GET의 암묵 HEAD를 다룬다. 현재 notes/waives factory의
  두 root·merge/fallback 연결도 고정한다. 새 route/명령/handler/root 변경과
  지원하지 않는 동적 method/nest/일부 routing macro/UFCS 형식은 검토를 요구한다.
- [실제 HTTP 거부 검사](../tools/validate_web_permissions.py): 합성 native 서버에서
  보호된 method/path **154개 × 3 = 462건**을 요청한다. 무인증, 반대 역할의
  cookie+CSRF, 반대 역할의 cookie 값을 대상 역할의 cookie 이름으로 바꾼 경우다.
  두 CSRF 헤더에 증명을 넣고 Cookie Path도 무시해 보내므로, 이름/Path/헤더 누락
  때문에 우연히 거부되는 것을 인증 분리의 증거로 세지 않는다.

HTTP 분류는 owner139, guest6, guest DRC9, public static6, owner/guest bootstrap 각1이다.
static shell은 설계/인증 정보가 없는 공개 UI이며 일회용 bootstrap은 별도 capability다.
이8개를 401 행렬에 넣지 않는다. 기존 grants/CLI gate가 교환·재사용·유효 경로를 검증한다.
guest DRC의 명령별/필드별 공개 범위는 `drc/shared.rs`의 exhaustive match와 native
DRC gate가 별도로 검증한다. 목록에 guest DRC로 분류했다고 모든 body를 허용하지 않는다.

WS GET은 정상 Upgrade·protocol envelope를 갖춘 요청으로 401을 요구한다.
HEAD `/events`는 Axum WS extractor가 인증 handler 전에 거부하므로 405가 기대값이다.
다른 보호 HTTP 요청은 401을 요구한다. 응답 body/credential은 실패 로그에 남기지 않는다.
검사 뒤 유효 owner/guest가 살아 있는지와 기존 source/cache/review 파일의 bytes·mtime
불변을 기존 CLI harness에서 확인한다. 합성 private 서버만 사용하며 임의 서버 탐색은 없다.

WS owner 명령 거부 unit은 같은 검토 목록을 사용한다. 누락 body의 역직렬화 실패로
통과시키지 않고 **unknown variant** 거부인지 단언한다. 실제 socket의 owner 명령·
Follow의 Explore 조작 거부 및 유효 Explore 동작은 기존 native gate도 함께 실행한다.

`syn`/`quote`는 이미 vendored된 정확한 버전을 **dev-dependency**로만 추가했다.
새 vendor 패키지/파일은 없고 Cargo.lock은 floe-web의 두 테스트 의존 edge만 바뀐다.
normal dependency tree에는 두 edge가 없으며 제품에 Python/Node나 AST parser를 추가하지 않는다.

### 이 게이트가 증명하지 않는 것

AST 검사는 현재 지원한 라우팅 표현의 변경 감지다. Rust 컴파일러의 모든 macro 확장,
임의 alias/factory의 의미나 handler 내부 권한 검사를 증명하는 도구가 아니다.
새 표현은 리뷰와 파서 테스트를 함께 확장해야 한다. HTTP 거부 검사는 인증 경계를
전수 대조하지만, 유효 principal의 모든 body/상태 조합을 전수 탐색한 것은 아니다.
허용 경로·동일 역할의 대상 바꿔치기·late completion·파일 불변은 아래 별도 검증이 필요하다.

## 2. SH-01~10 근거와 열린 범위

경로 약어: `support/`는 `rust/web/tests/support/`, `sharing/`는 `rust/web/src/sharing/`.
native gate는 `tools/validate_view_stream.py`, `validate_owner_service.py`,
`validate_web_local_sharing.py`가 private valmini/합성 DRC로 실행한다.

| 요구 | 로컬 근거 | 완료로 간주하지 않는 범위 |
|---|---|---|
| SH-01 인증/수명 | `sharing/mod.rs`의 교차 증명·만료/슬롯/폐기 unit, `support/sharing.rs`의 opt-in·scope·교환·revoke, 이번 462 HTTP 거부 | 실제 브라우저 cookie/storage는 SH-08 |
| SH-02 조작 권한 | 이번 AST 목록·mutation tests·HTTP 행렬, guest Control의 owner namespace 거부, `drc/shared.rs` Request allowlist·native body 검증 | 모든 언어 표현/입력에 대한 보안 증명 아님; 새 surface는 재리뷰 |
| SH-03 대상 바꿔치기 | `support/exploration.rs`, `guest_queries.rs`, `guest_drc.rs`의 view/epoch/scope·receipt/DRC revision/selection CAS 검증; owner 전용 token/artifact 경로 교차 인증 거부 | 모든 가능한 token/body 조합의 전수 탐색 아님 |
| SH-04 공개 범위/경합 | scope 변경 폐기 unit, native follow margin·layer scope, DRC 교체/late read 및 공유 body 전송의 폐기 검사 | 실제 브라우저 잔상·후진/복구는 SH-08; 무중단 index hot-reload는 사용자 보류 |
| SH-05 Follow 독립성 | `local_follow_reuses_pixels_has_private_credit_and_rejects_owner_commands`, 대형 미수신 socket 폐기·private credit·owner 진행, margin crop/reconnect native 검사 | 실제 느린 원격망의 화면 지연/처리량은 SH-10/G3 |
| SH-06 Explore 독립성 | owner+두 Explore의 별도 state/worker/DRC, query/룰러, admission 거부·재접속 재사용·연속 단절 후 reap/회계 | 서버 전체 다중 gateway 부하·실칩 latency는 별도 |
| SH-07 종료/late 완료 | guest별 logout/revoke, owner/scope 종료 cascade unit/native, 취소된 copy/body·lease/permit 수명 검사 | 실제 브라우저 복구 시나리오는 SH-08 |
| SH-08 실제 브라우저 | client/DOM 모형의 storage·revocation·stale 응답 gate는 보조 근거 | **미수용**: 동시 owner/guest·refresh/back/URL/opener/Referrer·실제 화면. 접근 차단을 우회하지 않음 |
| SH-09 읽기 결과 | native 합성 DRC/waive 표시·독립 선택/CD·fractional ASCII, 덱 child 범위·summary 조회 제한; CLI source/cache/review bytes·mtime | 실제 guest UI 시각/입력 및 실칩 수용 별도 |
| SH-10 배포 C | loopback Host/Origin 위조 거부는 로컬 경계 근거 | **미구현/미수용**: 승인된 TLS/auth/proxy·원격 공개·G3. 로컬 PASS로 대체 불가 |

따라서 SH-01~07/09에는 로컬 근거가 연결됐지만 SH-08/10을 포함한 공유 전체를
완료로 닫지 않는다. 항목별 근거를 추가하는 것과 실제 운영 수용 판정은 분리한다.

## 3. 재실행과 이번 결과

```sh
cd rust
cargo test --offline --locked -j2 -p floe-web --test permissions_inventory
cargo test --offline --locked -j2 -p floe-web follow_control_allowlist_is_not_owner_dispatch
# 저장소 루트에서 private 합성 fixture로 전체 배터리 실행
cd ..
sh tools/validate_rust.sh
```

AST 검사의 3개 테스트(진단 출력 1개 ignored)와 HTTP 462건은 집중 실행 및 전체
배터리에서 통과했다. ignored 진단은 목록을 stdout에 출력할 뿐 policy 파일을
생성/승인하지 않는다. 최종 `cargo fmt --check -p floe-web`와 app/core/web의
all-target strict clippy(`--no-deps -- -D warnings`)도 통과했다.

첫 전체 실행은 `gtk_startup_oracle` 바이너리의 30초 실행 제한으로 중단됐다.
소스·제한을 바꾸지 않은 집중 재실행에서 startup144·stream380·native 시작22건이
통과했고, 이어 전체를 처음부터 다시 실행해 **`BATTERY_EXIT=0`,
`RUST VALIDATION: ALL OK`**를 얻었다. 최초 시간 초과의 OS/환경 원인은 확정하지
않았으며, 제한을 늘리거나 실패 항목을 건너뛰지 않았다.

최종 배터리는 web118·HTTP transport15·새 inventory3, native stream20/owner21,
전체 UI, occupancy27·jobdeck83·renderer46과 KLayout jobs1/8 각각13 PX+
2 phase-exact+14 style을 포함한다. fixture 전용 ignored 테스트는 배터리의 전용
드라이버가 별도로 실행한다. 기존 native/Pillow/GLib 경고는 남아 있다.

로컬 로그: `/private/tmp/floe-permissions-battery.log`(첫 중단),
`floe-permissions-startup-retry.log`, `floe-permissions-battery-retry.log`(전체 PASS),
`floe-permissions-clippy-final.log`. 검증 전후 변경 코드/테스트7개 파일의 SHA-256이
동일했다(`floe-permissions-input.sha256`). 임시 `.venv` 링크는 종료 후 제거했다.
실제 브라우저·Linux 실행·실칩 성능·원격 서비스 수용을 검사한 것은 아니다.

## 4. 전체 goal에서 남은 것

로컬 Rust/web 기능과 공유 UI는 연결됐고 이번 단계는 권한 목록 공백을 막는다.
남은 큰 범위는 실제 브라우저 SH-08·G1/GTK 대비 표시/입력/성능과 G4 최종 수용,
Python-free Linux **실행** 수용, 현장 TeeBox Firefox/ETX G2, 원격 배포 C의 운영
정책/승인·구현·G3다. TeeBox 불가와 브라우저 접근 차단을 로컬 모형으로 대신하지 않는다.
world-tile M5는 선행 성능 조건/실측에 따른 조건부 작업이며 완료로 세지 않는다.
인덱스 교체를 열린 GUI가 자동 감지하는 문제는 사용자 요청대로 후속 서버/캐시 수명
설계로 보류한다. 커밋 수나 로컬 green을 전체 완료 백분율로 환산하지 않는다.
