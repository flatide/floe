# 웹 전환 M2 — DRC 읽기·공유 이관 기록

2026-09-13, `feature/webui`. [상위 계획](WEBUI_PLAN.ko.md),
[기능 대조표](WEBUI_M0.ko.md), [M1b](WEBUI_M1B.ko.md).

## 1. M2a-1: 기존 ICE pack 읽기 서비스와 CLI

`rust/app-core/src/drc/`는 native `floe-index drc`가 이미 만드는 **layout 4**
pack을 읽는다. 포맷 변경/재색인 필요 없음. 좌표는 i64 DBU로 보관하고 표시/질의
경계에서 µm로 변환한다. global 번호는 source ordinal이 아닌 전체 파일 순서의
1-based 번호이고, rule-local index는 core에서 0-based, 기존 CLI JSON에서 1-based다.

- header/footer·section 연속성·산술·check/error/block 범위·string ref·block bbox를
  검사한다. 좌표는 조회한 블록만 decode하고 varint 길이/overflow, delta overflow,
  points 수와 잔여 바이트, bbox를 검사한다. 임의 손상이 반드시 검출된다는
  checksum 계약은 아니다. 이전 버전 pack이나 구조 손상은 명시 오류다.
- 좌표와 block table을 mmap하지 않는다. 192 KiB slab으로 block metadata를
  순회해 rule bbox를 만들고, block별 pread + 최대 16개/32 MiB LRU로 좌표를 읽는다.
  open은 O(checks + blocks) metadata 비용이며 O(errors) 좌표 decode가 아니다.
  mmap 부재는 RSS 상한 보장이 아니라 외부 truncate의 SIGBUS 방지다.
- `errors()`는 단일 규칙의 유계 페이지, `query()`는 check bbox → block bbox →
  qbox → 실제 bbox 순으로 검색한다. waive 필터는 결과 cap **전에** 적용한다.
  검색당 최대 262,144 error slot / 4,096 순회 단계 후 `next` cursor를 돌려준다.
  빈 결과여도 `next`가 있으면 미완료다. 동일 cursor를 이어 읽으면 파일 순서와
  multiset이 유지된다. 생략을 완료로 위장하지 않는다.
- µm→DBU broad phase는 ULP/DBU 바깥 반올림으로 후보를 보수적으로 잡고 실제
  포함 판정은 µm에서 한다. `123456 / 40000` 같은 점의 경계 검색이 곱셈 반올림으로
  누락되는 사례를 오라클에서 확인해 고정했다.
- 취소는 metadata slab/블록/좌표 4,096개마다 검사한다. pack/선택한 waive 파일의
  inode·크기·mtime/ctime 변화는 reopen 오류다. 원자 교체/외부 색인 revision 관리의
  일반 해법을 추가한 것이 아니며 열린 파일 descriptor에 대한 일관성 확인이다.

읽기의 명시 한계(표시 정확도를 낮추는 budget이 아님): encoded metadata 64 MiB,
decoded metadata 64 MiB, 문자열 1 MiB, encoded coordinate block 16 MiB,
record당 262,144점/블록당 1,048,576점. 초과는 geometry를 자르지 않고 오류다.
페이지당 최대 2,000 errors/1,048,576점은 **continuation**으로 나눈다. metadata
raw/decoded 복사, LRU와 결과 복사는 별개이므로 32 MiB RSS cap으로 해석하지 않는다.
현재 실제 큰 DRC pack의 cold-open/pread 비용은 미측정이다.

### 1.1 읽기 CLI·waive

```sh
rust/target/release/floe2-web drc results.db.ice --rules
rust/target/release/floe2-web drc results.db --errs M1.WIDTH --floe-reviewer reviewer1
rust/target/release/floe2-web drc results.db.ice --list
```

기존 `--list`, `--rules`, `--errs`, `--floe-reviewer`를 지원한다. JSON 필드와 좌표
반올림, duplicate rule 첫 항목+경고, rules 우선순위를 유지한다. 오류 출력은
64개 단위로 읽고 stream하며 전체 오류 JSON을 메모리에 모으지 않는다.

직접 `.ice`는 원본 없이 열 수 있고 `.db`는 size/mtime가 같은 인접 `.ice`를 선택한다.
**이 단계에서 ASCII 직접 fallback은 아직 미이관**이다. missing/stale/corrupt side
pack이면 `floe-index drc <db>` 안내와 오류를 내며 자동 색인/덮어쓰기하지 않는다.

reviewer 순서: 명시값/FLOE_REVIEWER → 원격 DISPLAY host → SSH client → 계정명.
기존 `.<db>.waive.<reviewer>`와 temp fallback의 동일 SHA-1 path tag를 쓴다.
SHA-1은 기존 파일 이름 매칭용일 뿐 인증/무결성 보장이 아니다. 기존 sidecar가
있으면 원본 size/mtime/count fingerprint와 크기를 검증해 읽고, 없으면 embedded
status를 읽는다. **읽기만으로 autosave를 생성하거나 stale 파일을 rename하지 않는다.**
불일치 sidecar는 오류이며 조용히 다른 리뷰 상태로 바꾸지 않는다. notes와 쓰기는 M4다.

### 1.2 게이트

- 단위: block LRU/번호, 7개 페이지 연결, 잘못된 footer/varint/precision/truncate,
  취소, waive 필터, empty-query scan continuation, reviewer path 검증.
- `validate_app_drc.py`: adversarial ASCII(CRLF/빈 규칙/중복/unknown kind/부분 레코드)
  + 생성 DB를 기존 native로 색인. **1,931개 좌표/번호/status** 전수 대조,
  **168개 공간 검색**(점 경계·1/7/2000 cap·rule/waive 필터) + error paging 대조.
- 실제 CLI `--rules`/`--errs`는 Python JSON과 일치하고 PATH-empty 실행한다.
  전후 source/pack/waive 바이트·mtime 및 파일 목록 불변을 단언한다. stale refusal도
  source/pack을 변경하지 않는지 확인한다. full battery에 필수 게이트로 연결했다.
- 2026-09-13 검증: DRC 단위 6개, core/app 전체 테스트, strict clippy,
  Rust 1.89/빈 registry/offline 테스트 + Linux musl release link 통과.
  `sh tools/validate_rust.sh`는 `RUST VALIDATION: ALL OK`로 완료했다.
  기존 native 경고는 남아 있다. Linux에서 실제 실행하거나 DRC 웹/공유를
  완료했다는 뜻은 아니다.

## 2. M2a-2: 등록된 DRC actor와 인증 읽기 API

`view SOURCE --drc results.db.ice [--drc-waives EXISTING_FILE]`는 첫 번째
등록 소스에 DRC를 묶는다. 경로는 trusted CLI에서만 입력하며 pack/sidecar 부모를
**DRC 전용** 등록 root에 추가한다(잡덱 TC 허용 root를 넓히지 않는다).
브라우저는 경로·reviewer를 지정할 수 없다.
sidecar 옵션을 생략하면 embedded status만 읽는다. CLI `drc`의 ambient reviewer
조회와 의도적으로 구분한다. 이 단계에는 아직 DRC 화면이 없고 API만 연결했다.

- `floe-drc-read` **전용 1스레드**, active 1 + pending 4. open/metadata/좌표
  읽기/JSON 직렬화는 모두 이 스레드에서 한다. HTTP reactor나 renderd stdin에서
  기다리지 않는다. timeout/핸들러 drop은 해당 ticket만 취소하고, logout·만료·종료는
  active 취소 + 큐를 비운다. idle thread는 condition variable에서 쉰다.
- 기존 Resources에 **1 CPU slot + 256 MiB read-memory**를 수명 전체 예약한다.
  렌더 worker 숫자는 증가시키지 않는다. `decoded_mb`라는 기존 admission 필드는
  DRC 예약도 포함하며 RSS 상한은 아니다. decode+raster+DRC가 16 CPU/2048 MiB를
  넘는 CLI 설정은 URL 게시 전에 거부한다. 실제 빈 CPU/스케줄링 지연 보장은 아니다.
- 열린 pack과 선택 waive는 fd에 고정한다. 외부 교체·truncate의 검출 한계는 §1과
  동일하며 자동 reload/reindex하지 않는다. scope는 OS sandbox가 아니고 외부
  프로세스가 root/symlink를 동시 교체하는 것은 지원하지 않는다.
- 종료는 `serve`에서 4초 동안 actor 종료를 확인한다. 파일 시스템의 blocking
  pread/NFS 자체를 강제 interrupt할 수 있다는 계약은 아니다. deadline 초과는
  명시 오류이며 실제 thread 종료까지 자원 예약을 유지한다.

| endpoint | 요청/응답 |
|---|---|
| `GET /api/v1/drc` | 등록 ID/revision/source_id, opening/ready/error, 유계 metadata. 미등록은 drc=null |
| `POST /api/v1/drc/{id}/read` | `{view_id, revision, body}`. `body.kind`: rules/rule/errors/geometry/query |

기존 Host/Origin/cookie/CSRF 제한을 그대로 적용한다. read는 **요청 전후** 살아 있는
동일 owner 인증 + 현재 view_id/source_id/DRC revision을 검사한다. 같은 소스를
재open해도 이전 view_id 응답은 409다. 결과 header에 view ID와 DRC revision을
반복한다. error는 경로를 담지 않는 안정적인 code다. 원격 공유 권한을 추가한 것은 아니다.

- `rules`: start(0-based 문자열)/search/limit(1..64). 이름 case-insensitive
  substring 검색, 4,096 rules 또는 약 1 MiB name scan 후 continuation.
  목록 이름은 256문자와 `name_truncated`; `rule`로 전체 설명을 조회한다.
- `errors`: check/start/waived(null|bool)/limit(1..64). 체크 내 file order,
  waive 필터가 cap보다 먼저이며 global은 1-based 문자열, local/check는 0-based다.
- `query`: bbox_um(문자열 4개)/checks(null 또는 최대128개)/waived/cursor/limit.
  core의 보수적 broad phase와 exact bbox 판정·scan continuation을 유지한다.
- `geometry`: check/error/start/limit(1..2048). 점은 **i64 DBU 문자열**과
  precision을 반환하고 next/total을 표시한다. 페이지 일부를 닫힌 polygon 전체로
  그리면 안 된다. 이 API는 큰 오류 좌표를 조용히 잘라서 완료로 반환하지 않는다.
- 응답 JSON은 1 MiB까지 bounded writer로 만든다. 단일 rule 설명/escape 확장 등이
  넘으면 413 `drc_read_limit`이며 일부 JSON을 성공으로 보내지 않는다. errors/query의
  빈 rows라도 next가 있으면 미완료다. 클라이언트의 무제한 자동 페이지 수집은 금지한다.

`validate_web_drc.py`가 adversarial DB + 130개 반복 오류 + 5,000점 polygon을
native로 색인하고 실제 Rust CLI/HTTP에 PATH-empty로 붙는다. 규칙·waive 세 모드·
7개 오류 페이지·511점 좌표 페이지·9개 공간 검색을 Python과 대조한다. 무인증/
경로 주입/과대 요청/오래된 view/revision/외부 truncate, 원본 바이트·mtime 불변,
초기 render state 불변, 열기 중 취소·100 ticket 취소·자원 회수도 검증한다.
2026-09-13 검증: core 47/app 6/web 16 단위 + transport 8 테스트,
strict clippy, Rust 1.89/빈 registry/offline 테스트·Linux musl release link 통과.
`sh tools/validate_rust.sh`는 `RUST VALIDATION: ALL OK`로 완료했다. 이후 번들 hash
입력/DRC 전용 root 분리·추가 거부 게이트는 재빌드한 CLI와 HTTP 테스트로 재확인했다.
현장 Firefox/ETX는 현재 실행 불가라는 사용자 확인에 따라 보류다(로컬 PASS로 대체하지 않음).

## 3. M2a-3: 읽기 전용 DRC 패널과 표시 좌표

`view SOURCE --drc PACK.ice`가 오른쪽 DRC 패널을 연다. 별도 Python/브라우저
확장 없이 로컬 번들의 ES2017 JS/Canvas를 사용한다. 기존 pack/waive는 읽기만 한다.

- 규칙 이름 검색(32개/페이지), 전체 설명, 상태 필터(all/not waived/waived),
  오류 64개/페이지와 이전/다음·첫 페이지. 뒤로 이동 cursor 기록은 128개다.
  이름·설명은 `textContent`만 사용하며 HTML로 해석하지 않는다. 규칙 검색은
  목록을 필터링하고 현재 선택은 새 규칙을 클릭할 때 바뀐다.
- 오류 click은 선택+goto, double-click/Frame error는 오류 bbox를 뷰의 30%에
  맞춘다. 점 오류의 기본 폭은 0.1 µm. 사용자가 zoom을 바꾼 뒤 다른 오류로
  이동하면 그 배율을 유지하며 Frame error/Clear로 고정을 해제한다.
  `n`/`p`와 오류 행 위/아래는 **현재 페이지 안에서만** 이동한다. 경계는 안내하고
  페이지 버튼을 사용한다. 기존 GTK의 **현재 규칙/필터 결과 안에서 페이지를 넘어
  순환**하는 동작은 아직 미이관이다(규칙을 전부 가로지르는 순회가 아님).
- In view는 모든 규칙을 현재 viewport에서 검색한다. 후속 페이지는 최초 bbox에
  고정하며 pan/zoom 뒤에는 이전 뷰 결과임을 표시한다. 자동 전체 페이지 수집이나
  매 pan마다 DRC 재검색은 하지 않는다. Markers는 현재 페이지의 최대64개 표시다.
- 선택 geometry는 2,048점/응답, 최대128페이지·262,144점(좌표 배열 4 MiB)이다.
  읽기 중에는 **점선 bbox preview**, 모든 좌표가 온 뒤에만 polygon을 닫는다.
  edge는 두 점씩 선분으로 그린다. 상태1은 waived 색, 작은 오류는 최소 표시점을
  사용한다. 큰 단일 polygon의 브라우저 paint 시간 상한을 보장하는 것은 아니다.
- 선택/검색/context 변경은 이전 XHR를 취소하고 늦은 응답을 token + view/revision
  검사로 버린다. u64 ID/cursor는 JS Number로 바꾸지 않는다. geometry의 좌표 변환은
  화면 표시용이고 goto 뷰포트 계산은 Rust에서 한다.

읽기 API에 두 명령을 추가했다. `focus(check,error,fit)`와
`in_view(waived,cursor,limit)`는 envelope의 `state_rev`가 필수이고 요청 전후
현재 view revision을 검사한다. 서버가 DBU/viewport/pixels를 actor에 전달하므로
브라우저가 임의 DBU 또는 종횡비를 지정하지 않는다. focus는 `navigation`을
반환할 뿐 view를 변경하지 않는다. UI가 이를 기존 revision-checked WS edit로
보내며 중간의 입력 경합은 폐기한다. in_view는 실제 사용한 bbox도 반환한다.

### 3.1 pan·margin·resize

DRC overlay는 **현재 표시된 native 프레임의 bbox + 실제 blit/crop 원점**에 맞춘다.
요청한 새 viewport를 먼저 적용하지 않는다. 완전한 margin의 crop, 이전 label
foreground, mouse pan에서 동결한 composite, resize의 중앙 padding에 같은 변환을
사용한다. 따라서 native 응답을 기다리는 동안에도 DRC가 이전 화면과 함께 움직인다.
패널 숨김은 marker 토글과 별개이며 숨긴 뒤에도 선택 overlay는 유지한다.

실제 Chrome QA에서 기존 안내문이 나타났다 사라지면 viewport 높이가 바뀌지만
서버가 이전 크기의 margin crop을 유지하는 결함을 재현했다. 안내문을 viewport
안의 absolute overlay로 옮겨 표시만으로 해상도가 바뀌지 않게 했다. 선택적
ResizeObserver + 기존 window/panel resize 경로가 실제 요소 치수를 동기화한다.
같은 크기/이미 제출한 크기는 재제출·frame 동결하지 않는다. 이전 Firefox에서
ResizeObserver가 없어도 기본 경로는 유지하며 현장 지원 판정은 별도다.

### 3.2 검증과 미완료 범위

- JS gate: ES2017 구문, u64/text 안전성, 취소/늦은 좌표·focus 폐기,
  5,000점 geometry 3페이지의 완료 시점, zoom 고정/재프레임, 빈 페이지 continuation,
  in_view 고정 bbox, DRC의 margin/free-pan/resize 투영과 중복 resize 억제.
- native HTTP gate: focus fit/배율 유지·필수/오래된 state_rev, in_view bbox·결과를
  Python bbox 기준과 대조. 조회만으로 render state가 변하지 않는지 단언한다.
- 로컬 Chrome: synthetic valmini + 2규칙/4오류로 polygon/edge 선택,
  Shift+cursor 10% margin crop, 119×33 CSS px drag, 패널 접기/펴기, 규칙 검색,
  waive 빈 결과·In view, 입력 오류 중 viewport 크기 유지 확인. 콘솔 warn/error 없음.
  스크린샷은 세션 내 육안 확인이며 현장 Firefox/ETX·input-to-photon 측정이 아니다.
- 2026-09-13: 전체 `sh tools/validate_rust.sh` ALL OK, 대상 crate fmt/strict clippy,
  Rust 1.89 빈 registry/offline 테스트와 Linux musl release link 통과. 기존 native
  경고는 남아 있으며 Linux 실행/현장 브라우저 성능 PASS를 의미하지 않는다.

M2a-3 시점에는 새로고침 때 DRC 선택/필터가 초기화되었다. 아래 M2a-4b에서
같은 살아 있는 view에 대한 서버 상태 복원을 연결했다.
화면 오류 hit-test/box selection, 현재 규칙의 페이지 횡단 순회, SVRF 연동/자동 CD·측정,
선택 상태의 서버 보존/공유·waive/note 쓰기는 이 단계의 완료 범위가 아니다.

## 4. M2a-4a: view별 DRC 패널 상태 API

`GET/POST /api/v1/drc/{id}/views/{view_id}/panel`을 추가했다. 이 단계는 **서버
API 기반**이며 브라우저의 자동 복원 연결은 M2a-4b다. 기존 read-only 표현은
pack/waive/notes 파일에 대한 것이고, 이 API는 열린 view의 메모리만 변경한다.

- 저장 항목: 규칙 검색·현재 규칙 페이지/선택, 오류 페이지·waive 필터,
  선택 check/local ID, 고정된 In view query bbox/cursor·당시 state_rev,
  marker/패널 가시성, 오류 이동의 zoom 고정 상태. 번호는 u64 문자열이며
  이름/좌표 수와 길이를 구조적으로 제한한다. 과거 페이지 전체/좌표 목록은 저장하지 않는다.
- view attachment당 한 상태 + `panel_rev`가 있고 초기값은 1/body=null이다.
  pack revision과 view ID가 반드시 일치해야 한다. view를 닫고 새로 열면 새 상태다.
  서버 재시작·view 만료 뒤까지 유지되는 영속 저장이 아니다. HTTP 연결을 새로
  맺어도 같은 열린 view이면 GET으로 복원할 수 있다.
- POST는 `{revision, base_panel_rev, body}`. 다른 값으로 변경할 때만 revision을
  올린다. 같은 요청의 결과 불명확 재시도는 상태가 동일할 때만 idempotent 성공,
  오래된 base의 다른 상태는 409 `drc_panel_conflict`다. 병렬 요청도 commit 락에서
  다시 검사한다. 따라서 늦은 탭을 무조건 최종 writer로 취급하지 않는다.
- 구조/유한 좌표/양수 배율/길이는 HTTP에서 유계 검사하고, 실제 rule/error/cursor
  범위는 기존 actor의 metadata로 검사한다(좌표 decode 없음). 검증 중 취소·파일
  변경·닫힌 view는 기존 취소/오류 계약을 따르며 성공 전에 인증·view를 재확인한다.
  검증 실패나 revision overflow에는 이전 상태를 보존한다.
- native render state/worker generation은 바뀌지 않으며 implicit goto·waive/note
  쓰기·색인도 하지 않는다. 공유 권한과 충돌 조정 UI를 구현했다는 뜻은 아니다.

게이트: 동시 같은 base의 서로 다른 상태는 하나만 commit, retry/conflict/overflow,
문자열/좌표 한계 단위 테스트. 실제 HTTP는 무인증/잘못된 CSRF·revision·참조·cursor,
새 연결 GET/retry/conflict/새 view 초기화, 원본 파일·render revision 비변경을 단언한다.
2026-09-13: 전체 `sh tools/validate_rust.sh` ALL OK, web/app strict clippy와
Rust 1.89/offline 테스트·Linux musl release link 통과. 브라우저 저장 큐 모듈은
별도로 선행 테스트했지만 이 API 단계의 번들에는 아직 연결하지 않았다.

## 5. M2a-4b: 브라우저 저장 큐와 재접속 복원

§4의 패널 상태 API를 실제 UI에 연결했다. 규칙 검색/선택·현재 페이지·waive
필터·선택 오류·marker/패널 가시성·오류 goto 배율 상태를 보관하고, 새로고침과
page lifecycle 복귀 때 같은 열린 view에서 복원한다. 선택은 bbox/좌표를 다시
읽어 overlay만 만들며 **자동 focus/goto를 하지 않는다**. 사용자가 선택 뒤에
zoom/pan한 native view는 독립적으로 복원된다. 이전 페이지 history는 메모리
128개 한도이고 새로고침 시 초기화하며, 현재 cursor는 유지한다.

- 저장 큐는 진행 1 + 최신 대기 1, 변경을 120 ms 묶는다. 가변 JS 객체를 그대로
  잡지 않고 4 KiB 이하 snapshot으로 고정한다. 저장 상태가 같으면 새 HTTP 요청을
  만들지 않는다. Rust JSON map의 key 순서와 JS 삽입 순서 차이는 정규화한다.
- `panel_rev`를 증가 방향으로 확인하고 같은 state/view/DRC revision 응답만
  받아들인다. view 변경/종료 때 timer·XHR·최신 대기값을 모두 취소한다.
- 저장 중/서버 세션 동기화 완료/저장 미확정 상태를 별도로 표시한다. 오류나
  충돌 뒤에는 pending을 재전송하지 않고 `Reload review`로 서버 상태를 다시
  읽도록 한다. 이 버튼은 미동기화 로컬 패널 변경을 버린다는 tooltip을 둔다.
  서버 메모리 저장이며 pack/waive 파일의 영속 save가 아니다.
- 즉시 reload/닫기 전의 120 ms 변경 또는 결과를 못 받은 요청은 저장을 보장하지
  않는다. 종료 시 sync XHR/beacon으로 권한·revision 검사를 우회하지 않는다.
  다음 연결은 실제 서버의 확정 상태만 읽고, 이전 입력을 임의 재생하지 않는다.
- 복원 동안 DRC 목록 버튼만 잠시 비활성화하고 native 렌더는 독립적으로 계속한다.
  In view는 최초 bbox/cursor를 복원하며 그 사이 뷰가 바뀌었으면 과거 viewport
  결과임을 표시한다. 빈 결과+next는 복원 뒤에도 미완료다.

게이트: 저장 중 연속200회 변경→최신1개, 객체 복사/u64, 동일 상태 무전송,
잘못된 응답·revision 역행·과대 state·읽기 실패·저장 충돌/취소·이전 view 늦은 응답.
DRC 패널은 선택/waive/페이지/zoom 상태 복원이 goto나 새로운 저장을 만들지 않는지,
고정 bbox/cursor와 이전 뷰 안내를 유지하는지 검사한다. ES2017 구문 게이트에 포함한다.

실제 Chrome에서 browser timer를 DTO method로 호출하면 `Illegal invocation`으로
시작/종료가 실패하는 차이를 발견했다. Window timer를 closure로 감싸 receiver를
보존했고, Node 대역에도 같은 receiver 제약을 넣어 고정했다. 수정 번들에서
검색어 M2·not-waived·global4 선택 후 zoom/pan→reload, marker off/패널 숨김→reload,
명시적 Reload review를 확인했다. native 뷰 폭/배율과 선택 후 pan 위치를 유지했고
콘솔 warn/error는 없었다. 현장 Firefox/ETX·대형 pack 성능 검증은 여전히 별도다.
2026-09-13: 전체 배터리 `RUST VALIDATION: ALL OK`; 이후 응답 타입/과거 query
안내 보완까지 JS gate·재빌드 CLI의 native HTTP gate·fmt/strict clippy·Rust 1.89
offline 테스트·Linux musl release link로 재확인했다. QA 세션은 End session으로
정상 종료했다. 기존 native 경고는 남아 있다.

## 6. 다음 경계

1. 현재 규칙/필터 안의 페이지 횡단 순회는 별도 작은 단계로 연결한다.
   GTK의 단일 click=선택/초점, double-click=이동, Escape 뒤 n/p=뷰 이동 없이
   초점 변경도 조작 parity로 남아 있다(현 M2a-3 click은 선택+goto).
2. ASCII/index 흐름·기존 notes·상세 측정/룰 매핑은 각각 parity gate와 함께 확장.
3. 공유는 설계/DRC에 묶인 읽기 capability, 발급/만료/폐기·follow/independent
   state를 별도 구현·검증. 아직 shares=false, loopback-only다.
4. 외부 HTTPS/WSS·TeeBox 접근/인증 정책은 §10 미결 사항이며 로컬 기반 구현과
   실제 외부 공개를 구별한다. 브라우저 주소만 외부 IP로 바꿔 노출하지 않는다.
