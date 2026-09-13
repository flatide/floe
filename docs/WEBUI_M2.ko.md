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

## 6. M2a-5a: 현재 규칙의 유계 순회와 좌표 읽기 비용

`Pack::step`과 `body.kind=step` 읽기 API를 추가했다. 이 커밋은 코어/API 단계이며
UI 연결과 §3의 페이지 내 순회 제한 변경은 아래 M2a-5b에서 진행했다.

- 요청은 `check`, `backwards`, `after`(없으면 앞/뒤 첫 오류부터), `waived`,
  선택적인 `bbox_um`이다. **한 규칙 안에서** 동일 필터를 만족하는 다음 오류를
  찾으며 끝에서는 반대쪽으로 한 번 순환한다. 자기 자신만 일치하면 다시 자신이다.
- 응답은 최대 하나의 `hit`(기존 목록과 같은 번호/종류/status/bbox/점 수),
  검사 slot 수 `scanned`, 그리고 `next={next,remaining}`이다. 번호는 u64 문자열.
  hit가 없고 next가 있을 때는 미완료이며 동일 규칙·방향·필터로 이어 읽는다.
  이어읽기는 `after=null`로 보내고, 원래 기준과 cursor를 함께 보내면 거부한다.
  서버가 cursor 세션을 별도로 보관하지 않으므로 filter 동일성은 호출자 계약이다.
- 한 호출은 최대 262,144 slots / 4,096 block 단계다. 상태 bytes를 블록 단위로
  읽으며 오래된 waive count를 근거로 생략하지 않는다. 공간 필터는 check/block
  bbox로 가지치기하고 실제 오류 bbox를 µm에서 비교한다. 최악 시간/RSS를 숫자로
  보장하는 cap은 아니며 큰 coordinate block과 NFS I/O는 기존 취소/timeout 계약이다.
- `error_info()`는 bbox만 필요한 focus/step 소비자에 작은 metadata만 복사한다.
  `error_points()`는 최대 2,048점의 요청 구간만 복사하므로 한 큰 오류의 여러
  transport 페이지에서 **좌표 복사량**이 전체 좌표 수에 비례한다. containing block
  decode/검증은 여전히 필요하고 다른 요청의 LRU eviction으로 재decode할 수 있다.
  errors/query의 기존 전체 record 복사까지 제거한 것으로 해석하지 않는다.
- 인증·source/view/DRC revision·원본 파일 변경 검사와 actor queue는 동일하다.
  native render state, 캐시 포맷, renderd wire/버전, waive 파일은 변경하지 않는다.

게이트: 앞/뒤·처음/끝/63→64 경계·waive 세 상태·공간 필터·1/7/전체 scan limit
조합을 파일 순서 전수 오라클과 비교한다. 262k 초과 무일치 검색은 continuation을
내고 geometry를 decode하지 않는지 검사한다. 5,000점 좌표의 2,048/2,048/904
구간 합은 원본과 같고 bbox 소비자는 전체 좌표를 반환하지 않는다. 실제 HTTP도
130개 오류를 담은 규칙과 큰 polygon에서 기존 Python 결과와 순회/번호/bbox를 대조하며,
잘못된 cursor·타입·범위를 거부하고 원본 파일 및 render revision 비변경을 확인한다.

2026-09-13: core 50개 및 web/app/transport 테스트, strict clippy,
Rust 1.89/빈 registry 오프라인 테스트·Linux musl release link 통과.
실제 HTTP 오라클과 전체 `sh tools/validate_rust.sh`는 `RUST VALIDATION: ALL OK`.
field Firefox/ETX와 새 단축키 UI의 검증을 이 API 단계의 완료에 포함하지 않는다.
기존 native 경고는 남아 있다.

## 7. M2a-5b: 페이지 횡단 UI와 선택/이동 모드

§6의 `step`을 n/p, 오류 행의 위/아래 키, Previous/Next error 버튼에 연결했다.
기존 페이지 버튼과 오류 순회 버튼을 구별한다. 이 항목이 §3의 click/페이지 내
순회 한계를 대체하며, 전체 DRC 조작 parity 완료를 뜻하지는 않는다.

- 현재 규칙과 waive 필터 안에서 앞/뒤로 순회하고 끝에서는 처음으로 돌아간다.
  대상이 현재 페이지 밖이면 그 오류부터 최대64개를 다시 읽는다. 모든 이전
  페이지를 모아 rank를 계산하지 않으며, 정확히 GTK의 grid page 배치와 같지는 않다.
  In view 결과에서 다른 규칙의 오류를 선택하면 열린 규칙도 그 규칙으로 바꾸고,
  n/p는 그 규칙만 순회한다. 공간 조건은 최초 query bbox에 계속 고정된다.
- 최초 click은 선택/outline만 바꾸며 native view를 이동하지 않는다.
  double-click 또는 Frame error가 이동 모드를 시작한다. 그 뒤 click/n/p는
  오류로 이동하며 사용자가 바꾼 zoom은 기존 zoom-lock 규칙으로 유지한다.
  Escape는 이동 모드와 선택 outline을 끝내지만 순회 위치는 보존한다. 이후 n/p는
  현재 화면을 옮기지 않고 다음 오류를 선택한다. Clear는 위치까지 초기화한다.
- 단일 click 때 오류 button DOM을 교체하지 않아 브라우저가 뒤따르는 실제
  double-click을 같은 대상에 전달할 수 있다. 페이지 경계를 넘는 키 조작과
  순회 버튼은 결과 오류 행에 focus를 유지한다. viewport의 화살표 pan은 그대로다.
- 순회 요청은 하나만 진행하며 key repeat를 무제한 대기시키지 않는다.
  empty+next에는 **Search incomplete / Continue search**를 표시하고 사용자의
  명시적인 클릭으로만 이어 읽는다. cursor 진행/번호/범위를 검사한다. 이 미완료
  검색 cursor 자체는 panel state에 저장하지 않으며 reload 시 폐기한다.
- 필터/규칙/선택/새 query/Escape/종료는 이전 조회를 취소한다. 이동 모드라도
  검색 동안 더 새 pan/zoom이 적용되면 오류만 선택하고 늦은 goto로 덮어쓰지 않는다.
  DRC geometry/focus의 이전 응답도 기존 task/view/revision 검사로 버린다.
  Reload review는 저장 상태 GET을 기다리기 **전에** 진행 중인 요청을 취소한다.
- 서버의 per-view panel state에 `jump_active`, `focus_visible`을 추가한다.
  모드와 표시 해제 상태를 복원하지만 복원 자체는 goto하지 않는다.
  선택 없이 활성화된 모드나 표시 해제+이동 모드 같은 모순은 거부한다.
  pack/waive/원본 파일이나 native wire는 변경하지 않는다.

게이트: ES2017/대역 DOM에서 63→64·처음↔끝·waive·미완료 수동 재개,
100회 key repeat의 1요청 상한, 취소 후 늦은 응답, 더 새 pan 우선,
필터 변경/다른 query 규칙·고정 bbox, 복원의 native 비이동을 검사한다.
기존 2^53 초과 ID/좌표 paging/overlay 회귀도 유지한다.

실제 Chrome + valmini 기반 260개 합성 오류: 64번 단일 click과 65번 ArrowDown은
gen2/484.599µm를 유지했고 키보드 focus가 새 행에 남았다. 65번 double-click은
3.81695µm로 이동했으며 이동 모드의 n은 66번으로 이동했다. Escape 뒤 n은
67번을 선택하면서 gen6/6.89152µm를 유지했다. 페이지/선택 reload, 첫↔끝 순회,
waived 무결과, focus cleared 상태 reload도 확인했고 콘솔 warn/error는 없었다.
브라우저 QA는 현장 Firefox/ETX/G2나 실칩 대형 DRC 응답 시간 검증을 대신하지 않는다.

2026-09-13: 전체 `sh tools/validate_rust.sh`는 `RUST VALIDATION: ALL OK`.
이후 순회 버튼의 결과 행 focus와 복원 시작 시 즉시 취소 보완까지 ES2017/JS 게이트,
fmt/strict clippy·최종 native build·Rust 1.89 Linux musl release link로 확인했다.
Rust 1.89 오프라인 단위/transport 테스트도 통과했다. 최종 버튼 focus는 실제
Chrome에서 Previous error→130번 행 focus→n→1번 행 focus로 재확인했다.
각 QA 서버는 End session으로 exit0 종료했다. 기존 native 경고는 남아 있다.

## 8. M2a-6: 표시 마커 클릭과 pan 분리

캔버스에 **실제로 그린** 마커의 가까운 중심을 6 CSS px 안에서 고른다.
현재 페이지 최대64개와 페이지 밖 선택 오류 최대1개만 대상이다. 보이지 않는
오류를 전수 검색하거나 native geometry pick을 호출하지 않는다. In view의
전체 규칙 검색/continuation과 마커 클릭은 별개다.

- 마우스 왼쪽 단일 클릭은 선택/상세/outline만 바꾸고, 이미 이동 모드여도
  native view를 옮기지 않는다. 이는 GTK `_drc_pick`의 캔버스 클릭 계약이다.
  두 번 클릭하면 기존 Rust focus/goto를 요청한다. 목록 클릭의 이동 모드와
  다르며, n/p·Escape·zoom-lock 계약은 §7 그대로다.
- 선택 오류에도 9 device px 중심 표식을 남긴다. 큰 outline이 나타난 뒤에도
  같은 위치의 두 번째 클릭 대상이 **눈에 보이도록** 하기 위함이다. 큰 jump 오류의
  중심 표식을 생략하는 GTK와는 이 표현이 다르다. 설계 픽셀은 변경하지 않는다.
- 표시한 overlay의 실제 DOM 위치/크기로 CSS 좌표를 비교한다. devicePixelRatio,
  fractional origin, margin crop/free-pan 이동을 반영하며 새 요청의 bbox를 이용해
  역변환하지 않는다. 가까운 마커가 우선이고 동률은 그린 순서가 우선이다.
- marker off/화면 밖/소스·revision 변경/복원 중/연결 끊김/미처리 view edit일 때는
  선택하지 않는다. 목록·선택이 바뀌면 다음 paint까지 이전 hit 목록은 무효다.
  선택을 바꿀 때 이전 step/geometry/focus를 취소하고 늦은 응답을 버린다.
- release-only pan이 8 CSS px 문턱을 한 번 넘었으면 원점에 돌아와도 클릭하지
  않는다. 중간 버튼·복수 버튼·Ctrl/Shift/Alt/Meta 클릭과 blur/resize/취소도
  마커 선택으로 바꾸지 않는다. browser MouseEvent.detail을 사용하므로 별도의
  double-click timer는 없으며 드래그 중에는 네트워크 입력을 보내지 않는다.

ES2017/Node 게이트: 가까운 마커/반경 경계/완전히 잘린 마커, 2^53 초과 ID,
CSS origin·DPR·이동된 crop, 한 번/두 번 클릭과 이동 모드, 보이지 않거나 stale한
hit 무효화, 늦은 좌표 응답 취소, 공간 검색 0회, app→gesture→panel 연결을 고정한다.
Chrome/DPR2 실제 QA에서 마커 51 단일 click은 gen4/484.599µm를 유지했고,
같은 위치 double-click은 gen6/3.70000µm로 이동했다. 10% margin pan에서도
outline/중심 표식이 함께 이동했다. Fit 후 이동 모드에서 다른 마커를 클릭하면
선택만 51→39로 바뀌고 gen8/484.599µm를 유지했다. Markers off 후 이전 위치를
클릭해도 선택은 그대로였다. 콘솔 warn/error는 없었고 End session은 exit0이었다.
이 결과를 Firefox/ETX/G2 또는 큰 DRC 데이터의 성능 검증으로 대신하지 않는다.

2026-09-13: 전체 `sh tools/validate_rust.sh`는 `RUST VALIDATION: ALL OK`,
KLayout 13 PX + 2 phase-exact + 14 style도 통과했다. 마지막 복수 버튼 보완까지
ES2017/JS·전환 패키지 strict clippy(`--no-deps`)·fmt·최종 native build/인증 HTTP를
재확인했고, Rust 1.89 오프라인 테스트와 최종 Linux musl release link도 통과했다.
workspace 의존 parser/tiler의 기존 clippy 경고는 별개이며 전체 warning-free를
주장하지 않는다. pack/캐시 포맷과 renderd wire/버전은 변경하지 않았다.

## 9. M2a-7a: 단순 오류의 CD 측정 코어/API

`app-core::drc::cd_segments`는 기존 `floe/drc.py::cd_segments`의 의미를 옮긴다.
단일 edge는 길이, edge 두 개는 가장 가까운 간격을 첫 항목으로 돌려주며,
서로 평행하고 X/Y 범위가 떨어진 경우 그 뒤에 수평·수직 성분을 붙인다.
마주 보는 평행 edge는 가운데 gap을 택한다. 네 모서리가 축 정렬 사각형이면
폭 다음 높이를 돌려준다. 복잡한 polygon/edge 집합·접촉/교차·길이0에는
CD ruler를 발명하지 않고 빈 목록이다. SVRF width/space/area 판정기는 아니다.

읽기 body `{"kind":"measurements","check":"0","error":"0"}`는 기존
DRC/view/revision scope 및 actor queue·취소 경계를 쓴다. 응답은 check/local/global
문자열 ID와 최대3개 `segments`다. 각 항목은 두 µm endpoint의 문자열 배열
`endpoints_um`, 문자열 `distance_um`, 단일 edge의 평행 offset 표시용 `offset` bool이다.
viewport에 독립적인 측정이라 `state_rev`는 필요 없고, native view는 변경하지 않는다.
요청에 좌표/경로/확장 알고리즘을 넣을 수 없고 파일의 오류만 읽는다.

- containing block은 기존처럼 검사/디코드하지만 선택 오류에서 최대4점만 복사한다.
  5,000점 이상 polygon도 전체 geometry를 다시 복사하거나 매 transport 페이지마다
  측정하지 않는다. 이 API의 빈 segments는 임의의 4점 prefix 측정이 아니다.
- i64 DBU 차이를 i128에서 먼저 구하고 2의 거듭제곱으로 정규화한 작은 좌표계에서
  계산한다. 절대좌표가 큰 곳의 1 DBU edge나 precision이 극단인 곳에서 중간 제곱의
  overflow/underflow로 잘못된0·NaN을 내지 않도록 하기 위함이다. µm 길이 자체가
  f64로 표현 불가능하면 명시 오류다. 출력 endpoint는 기존 표시 좌표처럼 f64이고
  특히 큰 원점에서는 endpoint 차이보다 별도 distance 값이 더 정밀할 수 있다.
- Python과 같은 후보 순서·가운데 허용오차(1.0001)·평행 허용오차(1e-12)를 유지한다.
  정규화와 거리 계산의 반올림까지 byte 동일하다는 계약은 아니다. 일반 합성 파일은
  endpoint/거리 수치 오라클로 비교하고, 큰 원점/작은 길이는 별도 정확값으로 검사한다.
- UI 연결과 CD 글자 배치는 다음 단계다. native renderer·pack 포맷·캐시·waive 파일과
  renderd wire/버전은 바꾸지 않았다.

검증: 고정 CD 예와 endpoint 역순, 접촉/교차/축퇴/복잡 도형, i64 양 끝·1 DBU 길이·
precision 한계를 단위 테스트한다. 실제 HTTP 게이트에 일반/뒤집힌/이동된 rectangle,
edge pair 및 seed76 난수 쌍 88개를 더해 기존 Python CD와 endpoint 순서·거리·offset을
대조하며, 기존 5,000점 polygon은 빈 segments인지 확인한다.
기존 native pack 읽기 오라클도 **1,931개 오류의 CD**를 전수 대조한다(geometry/status와
168개 공간 페이지 비교 유지). default toolchain과 Rust 1.89에서 모두 통과했다.

2026-09-13: core 53개 및 app/web/transport 테스트·fmt·전환 패키지 strict clippy,
인증 HTTP CD 오라클·ES2017 UI 회귀, Rust 1.89 빈 registry 오프라인 테스트와
Linux musl release link 통과. 전체 `sh tools/validate_rust.sh`는
`RUST VALIDATION: ALL OK`이며 13 PX + 2 phase-exact + 14 style 오라클도 통과했다.
추가한 1,931개 CD 테스트는 전체 배터리 후 별도 native/MSRV 오라클로 재확인했다.
이 단계는 API까지이며 CD UI·현장 Firefox/ETX 수용은 완료로 세지 않는다.

## 10. M2a-7b: 자동 CD 치수선·값 표시

Rust 측정 API를 `rulers.js`와 읽기 전용 CD 패널에 연결했다. 화면 표시용 투영과
라벨 배치는 JS지만 거리/측정 geometry 계산은 §9의 Rust 코어가 담당한다.
native raster·renderd wire/버전·캐시 포맷을 변경하거나 재색인하지 않는다.

- CD의 소속은 **마지막으로 focus 응답을 받아 이동 요청한 오류**다. 단순 canvas
  click은 선택만 바꾸므로 선택 오류와 CD 대상이 다를 수 있다. 패널은 자신의
  `jumped global N`을 표시하며 두 상태를 혼동하지 않는다. 다른 규칙으로 바꾸거나
  Clear/source/view 변경·focus 종료 때 CD도 정리한다. 단순 목록 선택은 CD를
  생성하지 않고, 이동 모드에서 goto가 성립한 경우에만 교체한다.
- 동일한 **표시 프레임**의 bbox/DBU/blit 원점으로 투영한다. pan/zoom/margin crop은
  최대3개 선분만 재투영하며 측정 API를 다시 호출하지 않는다. 늦은 geometry와
  독립적인 CD read token을 사용하고, 응답의 check/local/global·endpoint·길이·개수를
  검증한다. Reload는 panel GET을 기다리기 **전에** 이전 CD/focus/step을 취소한다.
- 단일 edge 치수선은 endpoint 순서와 무관한 위쪽 normal(수직이면 오른쪽)으로
  **14 CSS px** 이동하고 점선 연장선을 그린다. rectangle은 폭→높이, edge pair는
  gap→가능한 X/Y 성분 순서다. 실제 edge는 이동하지 않는다. DPR1/1.25/2/3에서도
  이격/선 두께/라벨 크기는 CSS 기준이다. 브라우저 UI 글꼴이며 native PNG의
  결정적 글꼴·pixel oracle에 포함시키지 않는다.
- 라벨은 흰 글자/불투명 배경과 leader로 자기 선을 가리키며, 서로 겹치지 않는
  유계 후보를 찾는다. 너무 작은 화면/축소로 배치할 자리가 없으면 **화면 라벨만**
  생략하고 패널에는 남은 모든 측정값을 보인다. 일반 값은 소수4자리 µm,
  0.0001 미만의 양수나 1e9 이상은 과학 표기로 표시하고 원 응답 문자열은 tooltip에
  둔다. 복잡한 오류는 `no supported measurement`, 읽기 실패는 `CD unavailable`다.
  unsupported/실패를 가짜 길이0으로 표시하거나 오류 outline을 버리지 않는다.
- `k`/Remove ruler는 마지막 하나, `Shift+K`/Clear rulers와 첫 Escape는 전부 지운다.
  이어지는 Escape는 focus/이동 모드를 종료하며 순회 위치는 유지한다. CD가 없으면
  첫 Escape가 focus를 종료한다. 읽기 중 지우기는 요청을 취소하고 모두 지운다.
  pending focus/step도 취소하여 늦은 이동이 다시 ruler를 만들지 않게 한다.
  Markers off는 치수선도 숨기지만 CD 값/상태를 삭제하지 않는다.

view 패널 JSON에 nullable `cd:{target:{check,error},remaining:0..3}`를 추가했다.
target은 selected와 별개이며 서버에서 canonical u64 및 pack의 실제 error 범위를
검증한다. `cd`가 있으면 jump_active여야 한다. remaining=0은 지운 상태다.
복원은 최대3개 측정을 읽어 남아 있는 선만 표시하고 **focus/goto를 호출하지 않는다**.
remaining=0이면 CD를 다시 읽지 않는다. 기존 4 KiB
latest-only/CAS·서버 세션 메모리 수명은 같고 pack/waive/notes 파일은 쓰지 않는다.
콘텐츠 hash로 JS/HTML/스키마 버전을 함께 식별한다.

검증: ES2017 parse와 기존 UI gate, CD DTO/label/crop/DPR 테스트,
독립 대상 복원·pan 재조회 0회·k/K/2단계 Escape·지연/취소/잘못된 응답을 테스트한다.
선택 오류가 화면 밖이어도 다른 CD의 화면 표시를 막지 않는지 확인한다.
인증 HTTP 게이트는 selected와 다른 CD target을 저장/복원하고 잘못된 번호/개수/
불일치 이동 상태는 commit 전에 거부한다(원본 파일/렌더 상태 불변 유지).

로컬 Chrome 실제 QA(합성 valmini + 260오류, DPR2): global1 edge의
`Length 0.6650 µm`/14px 이격, global2의 `Width 0.7600 µm`와 `Height 0.1700 µm`를
확인했다. k 후 reload는 폭만 복원하며 gen8/2.53333µm 뷰를 유지했다.
Shift+Right 10% margin pan에서도 도형/치수선이 함께 이동했고 gen8 그대로였다.
Escape 두 번은 CD→focus 순으로 정리했다. Fit 후 다른 마커를 클릭하면 선택은
51→39, CD는51이며 gen17/451.725µm를 유지했고 새로고침 뒤에도 각각 복원됐다.
콘솔 warn/error는 없었고 End session은 exit0이다. 빠른 연속 focus+다른 view 입력은
이동을 거부하는 기존 경합 방어를 확인했으며, 다음 성공 때 이전 경고를 지운다.
이 결과는 현장 Firefox/ETX/G2 수용을 대신하지 않는다.

2026-09-13: 전체 `sh tools/validate_rust.sh` ALL OK, KLayout 13 PX + 2 phase-exact
및 14 style 통과. 최종 보완까지 ES2017/JS·전환 패키지 fmt/strict clippy(`--no-deps`),
core 53/app 6/web 19 및 transport 8 테스트, release 번들의 인증 HTTP 게이트를
재확인했다. Rust 1.89 빈 registry 오프라인 테스트와 Linux musl release link도
통과했다. 기존 native 의존성 경고는 남아 있으며 Linux 실제 실행 PASS는 아니다.

## 11. M2a-8a: 규칙별 오류 선택 집합 코어/API

`app-core::drc::Selections`에 규칙별 replace/add/toggle 집합을 두고, 열린 view의
서버 메모리에 보관한다. focus/마지막 CD/패널 설정과 독립적이며 pack·waive·notes와
native view를 변경하지 않는다. **이 단계는 API까지이며 박스 선택 UI는 다음 단계다.**

기존 GTK `_esel_apply`와 SPEC-VIEWER를 재확인했다. 박스 선택 대상은 **현재 필터·
페이지의 오류**이고, 중심점 포함이 아니라 오류 bbox와 선택 박스의 닫힌 교차다.
전체 pack을 검색하거나 모든 공간 마커를 표시하는 계약이 아니다. 새 코어도 요청에
명시한 최대64개 local ID만 검사하고 waive 필터→bbox 교차→집합 연산을 수행한다.
ID 중복은 한 번만 적용하며 toggle도 두 번 뒤집지 않는다. replace는 해당 규칙만
대체하고 빈 replace는 그 규칙만 비운다. 다른 규칙의 집합은 유지한다.

인증 URI `GET/POST /api/v1/drc/{id}/views/{view}/selection`:

- GET은 `revision`, `view_id`, `state`를 돌려준다. state는 문자열
  `selection_rev`/`total`, 상수 `limit:5000`, 숫자순 `rules:[{check,errors}]`다.
  check/local ID도 문자열이며 2^53 초과 ID를 JS number로 축소하지 않는다.
- POST는 `revision`, `base_selection_rev`, 선택적 `state_rev`, `body`다.
  body는 `{kind:"apply",check,errors,mode:"replace"|"add"|"toggle",bbox_um?,waived?}`
  또는 `{kind:"clear_all"}`이다. bbox는 유한하고 순서가 맞는 µm 문자열4개이며
  점/선 박스도 허용한다. bbox를 보내면 state_rev가 필수다. 경로/임의 필드는 거부한다.
- pack/view/source/revision 및 주어진 state_rev를 actor 읽기 전후 확인한다.
  기존 단일 DRC actor·취소·pending4를 쓰고 HTTP reactor에서 pack을 decode하지 않는다.
  ID/pack 손상/만료/인증 오류는 집합을 변경하지 않는다. GET은 메모리 상태만 읽는다.
- selection revision은 commit 락 안에서 다시 검사한다. 성공한 **no-op도** revision을
  소비한다. 같은 base의 동시 두 요청 중 하나만 성공하고 나머지는409다. panel의
  전체 스냅샷 저장과 달리 toggle 명령은 재전송하지 않는다. 응답 유실은 GET으로
  실제 상태를 확인해야 하며 네트워크 단절 후 commit 취소를 보장하지 않는다.
- 전체 규칙 합계5000개는 명시적인 서비스 자원 한계다. 넘으면413이며 이전 집합과
  revision을 그대로 보존한다. 조용히 prefix로 자르지 않는다. GTK의 선언된 선택
  상수가 모든 기존 경로에서 강제됐다는 뜻은 아니다. 빈 규칙 항목도 남기지 않아
  map과 최대 응답이 유계다(최악 ID/5000규칙 JSON도1MiB 미만 테스트).

기존 읽기 API에 `{kind:"records",check,errors:[...]}`를 추가했다. 명시한 최대64개
오류의 정렬/중복 제거된 metadata만 반환하므로 다음 UI의 선택 목록 복원에 쓴다.
containing block은 검사·decode하지만 vertex 배열을 복제/전송하지 않는다.
큰 5000점 polygon도 point_count/bbox 등만 반환한다. metadata-only를 무I/O나
전체 RSS 상한으로 해석하지 않는다. 캐시 포맷·native wire/renderd 버전은 그대로다.

검증: Rust 집합/상한 원자성·u64 ID·취소·손상·bbox 경계·동시 CAS 단위 테스트와
실제 인증 HTTP를 통과했다. Python 오라클은 모드3종×waive3종×bbox4종을 비교하고,
전체130개 규칙에서도 지정64개만 선택하는지, 규칙별 보존/중복 toggle/no-op retry,
실제 동시 HTTP의200/409, 새 view 초기화, truncate 시 변경 거부를 단언한다.
전후 panel/render 상태와 원본 파일 바이트·mtime는 불변이다.

2026-09-13: core56/app6/web23/transport8 테스트, fmt 및 전환 패키지 strict clippy
(`--no-deps`), release 인증 HTTP·ES2017 UI 회귀 통과. Rust1.89 빈 registry 오프라인
테스트와 Linux musl release link도 통과했다. 전체 `sh tools/validate_rust.sh`는
`RUST VALIDATION: ALL OK`이며 KLayout13 PX+2 phase-exact+14 style도 통과했다.
기존 native 경고는 남아 있다. 새 선택 UI의 브라우저 QA나 현장 G2 완료는 아니다.

## 12. M2a-8b: 박스 선택·규칙별 금색 마커·복원 UI

§11 API를 ES2017 `drc-groups.js`와 DRC 패널에 연결했다. 집합 연산과 bbox 판정은
Rust가 하고, JS는 **실제로 표시한 프레임**의 좌표 역변환·입력·마커 표시만 한다.
기존 Rust 내장 정적 자산/loopback 구조를 유지하며 외부 호스팅이나 의존성을 추가하지 않았다.

- `e`/Box select는 두 클릭 모드다. 첫 모서리는 world µm로 보관하고 금색 작은
  십자로 표시하며, 포인터까지 점선 박스를 그린다. rAF 사이 여러 mousemove도
  마지막 위치를 남긴다. 완료 뒤에도 다음 박스 대기이며 e로 끈다.
- 두 번째 클릭이 무수식이면 현재 규칙 집합 대체, Shift는 추가, Ctrl은 토글이다.
  Mac을 위해 **Cmd도 선택 도구/오류 목록에서만** 토글 별칭으로 허용한다.
  Ctrl/Cmd가 Shift보다 우선한다. 목록의 Shift/Ctrl/Cmd 클릭은 오류 하나를
  추가/토글하고 focus/goto/geometry 읽기를 일으키지 않는다. 수정키 더블클릭을
  별도 goto나 두 번째 명령으로 재해석하지 않는다.
- 항상 현재 규칙·필터·페이지의 최대64개 ID만 보낸다. 중심 마커가 박스 안에
  없어도 오류 bbox가 걸치면 선택하는 기존 GTK 의미다. 선택한 그룹을 목록으로
  대체하지 않으며 행 배경과 마커만 금색으로 바꾼다. 현재 focus의 파란 행 배경이
  금색보다 우선하고 기존 focus/CD는 별개다. **Selected 목록 필터는 아직 미이관**이다.
- 포인터 좌표는 overlay DOM rect/DPR·표시 bbox/DBU·margin 원점을 사용한다.
  첫 클릭 뒤 pan해도 첫 world 모서리는 유지한다. 8 CSS px를 넘긴 드래그는
  release-only pan이고 원위치로 돌아와도 박스 클릭이 아니다. 복수 버튼/Alt,
  입력 중 모드 변경·lost capture·stale frame·미처리 view 입력은 선택으로 만들지 않는다.
- 규칙/페이지/waive 필터 변경은 미완성 박스를 취소한다. Markers off·reload·새
  view·종료는 모드도 해제한다. Escape는 첫 모서리 취소→선택 모드 종료→CD
  제거→현재 규칙의 그룹 비우기→focus 종료 순서다(존재하는 단계만 소비).
  다른 규칙의 그룹은 유지하고 waive 필터 변경은 전체 그룹 clear를 요청한다.

규칙별 집합은 서버 세션에서 복원하고, 현재 규칙의 선택 마커는 다른 페이지로
가도 표시한다. 복원에 필요한 metadata만 기존 `records`로 최대64개씩 읽고
vertex 배열은 받지 않는다. 5000개 전부가 선택돼도 최대79개 유계 읽기이며 pan마다
다시 읽지 않는다. 읽기 실패는 `Selected markers incomplete`로 표시하고 기존
선택 번호/현재 페이지 강조는 유지한다. 단순 조회/선택으로 설계 프레임은 렌더하지 않는다.

선택 명령은 브라우저에서 **active1·대기열0**이다. 응답의 scope/revision/정렬·중복·
합계/5000 상한을 확인하며 오래된 view 응답은 버린다. 충돌/timeout/잘못된 응답이면
GET으로 한 번 확인할 뿐, 토글을 자동 재전송하지 않는다. GET도 실패하면
`Selection uncertain`과 Reload 안내를 내고 추가 변경을 막는다. 이 상태는 열려 있는
view 수명까지만 보존되며 pack/waive/notes 파일·native renderer·캐시 포맷은 바꾸지 않는다.
다중 브라우저의 실시간 push 동기화나 index hot-reload를 구현한 것은 아니다.

검증: 새 JS 게이트는 유실 응답 이후 GET만 수행하는지, 연속 명령 거부, 오래된
scope, 잘못된/중복/2^53 초과 ID, bbox 경계, DPR2/분수 CSS 원점·margin 역변환,
Shift/Ctrl/Cmd, 빈 박스/다른 규칙 보존, 페이지 변경·복원·Escape를 단언한다.
마커 복원에 전체 공간 query/focus/vertex API를 호출하지 않는지도 검사한다.
기존 pan·마커·CD·패널·전체 ES2017 회귀를 유지했다.

로컬 Chrome/DPR2 합성260오류 QA: 박스로 global **1/38/39/51/54** 다섯 개를 선택해
금색 마커/행을 확인했고 gen4는 그대로였다. Shift 행 추가 후 새로고침은 여섯 번호와
기존 뷰를 복원했다. 최종 Cmd 별칭으로 global2를 0→1→0 토글했고 focus/goto는
없었다. 첫 모서리 후 Shift+Right 10% margin pan 뒤에도 같은 다섯 오류가 선택됐다.
65번부터 시작하는 다음 페이지에서 reload해도 다섯 선택과 페이지가 복원됐으며
선택 모드는 안전하게 off였다. 마지막 번들에서 첫 모서리 십자와 Escape의
모서리 취소→모드 종료도 확인했다. 콘솔 warn/error는 없고 세 QA 세션 모두 exit0이었다.
이것은 현장 Linux Firefox/ETX 검증이 아니다. G2는 계속 보류다.

2026-09-13: 전체 `sh tools/validate_rust.sh` ALL OK 및 KLayout13 PX+2 phase-exact+
14 style 통과. core56/app6/web23/transport8 테스트·fmt·전환 패키지 strict clippy와
release 인증 HTTP 오라클을 재확인했다. Rust1.89 빈 registry 오프라인 테스트와
Linux musl release link도 통과했으며 마지막 UI 보완은 ES2017/JS 게이트로 재검증했다.
기존 native 경고는 남아 있고 Linux 실행 PASS로 해석하지 않는다.

## 13. M2a-9a: 현재 규칙의 목록 필터·순회 코어/API

§12의 선택 집합을 목록과 순회에도 적용한다. 이 단계는 **서버/API까지**이며
현재 UI의 전체 규칙 고정 In view 버튼은 다음 M2a-9b에서 현재 규칙의 live 필터로
바꾼다. 기존 `errors/query/in_view/step` 읽기 API는 호환/오라클용으로 유지한다.

- `app-core::Pack::filtered_errors`는 현재 규칙의 local ID 순서로 **Selected ∩
  waive ∩ viewport bbox**를 모두 적용한 뒤 최대64개를 반환한다. `next`는 선택
  배열의 첨자가 아니라 다음 rule-local ID다. 빈 선택 집합은 빈 목록이며 전체 오류로
  되돌아가지 않는다. 필터 없는 목록도 같은 좌표/bbox 판정을 사용한다.
- 선택 집합이 있으면 그 규칙의 최대5000개 ID만 검사한다. 없으면 기존 query의
  check/block/qbox/실제 bbox 가지치기와 요청당 262,144 slot/4096단계 continuation을
  유지한다. `next`가 있는 빈 페이지는 완료가 아니다. 필터를 바꾸면 처음부터 시작한다.
- 새 `query_info`는 기존 geometry query와 판정·순회 코드를 공유하지만
  **vertex 배열을 복사하지 않고 metadata만** 반환한다. containing block의 검사와
  decode는 여전히 필요하므로 무I/O/무decode나 RSS cap을 보장하는 것은 아니다.
  geometry query의 기존 복사점 cap/continuation은 바꾸지 않았다.
- `filtered_step`은 동일 교집합 안에서 앞/뒤 순환한다. Selected이면 최대5000개
  ID를64개씩 검사해 전체 한 바퀴 안에서 끝나며 별도 pack-range cursor를 받지 않는다.
  Selected가 아니면 기존 유계 step/continuation을 사용한다. 필터 판정에 요약된
  waived count를 사용하지 않아 stale counter가 결과를 숨기지 않는다.

인증된 `/api/v1/drc/{id}/read`에 두 body kind를 추가했다:

| kind | 필드 | 결과 |
|---|---|---|
| `list` | check/start/limit(1..64)/waived/in_view/selection_rev? | metadata rows, next local ID, scanned, bbox_um, selection_rev |
| `filtered_step` | check/backwards/after?/cursor?/waived/in_view/selection_rev? | metadata hit, next step cursor, scanned, bbox_um, selection_rev |

`in_view:true`는 envelope의 현재 `state_rev`가 필수다. actor에 전달할 bbox/DBU는
서버의 authoritative viewport에서 얻는다. `selection_rev`는 양의 canonical u64
문자열이며 해당 revision의 **서버 집합**을 복사해 쓴다. null/생략은 Selected off,
해당 규칙의 집합이 비어 있더라도 명시된 revision은 Selected on이다. 브라우저가
임의 bbox/선택 ID 목록/경로를 이 두 명령에 넣을 수 없다. context가 필요한데
내부 호출자가 이를 전달하지 않아도 전체 결과로 fallback하지 않고 오류다.

읽기 전후 owner/view/source/pack revision, 주어진 view/selection revision을 확인한다.
변경된 집합은409 `drc_selection_conflict`, 변경된 뷰는409 `drc_context_changed`다.
기존 단일 actor·pending4·취소 경로를 재사용하며 새로운 스레드/의존성/파일 쓰기나
native navigation은 없다. 여러 요청의 cursor를 서로 다른 필터로 재사용하지 않는
것은 클라이언트 책임이며 UI에서 token과 revision으로 방어한다.

검증: core의 Selected/waive/bbox/limit/방향/순환 오라클, 빈 집합/빈 규칙/잘못된
cursor·ID·bbox·상한/취소·truncate를 검사했다. 65개×16,384점 fixture로 기존
geometry query는 복사점 cap에서 이어지고 metadata query는 좌표 복제 없이 같은
오류 metadata를 이어 읽는지 확인했다. 실제 HTTP는 Python 오라클과 **384개**
목록·순회 조합을 비교하고 잘못된 context/revision 거부, 큰 polygon의 metadata-only,
빈 Selected, panel/render 상태 및 원본 바이트·mtime 불변을 단언한다.

2026-09-13: core59/app6/web24/transport8 테스트, fmt·전환 패키지 strict clippy,
Rust1.89 빈 registry 오프라인 테스트와 Linux musl release link를 통과했다.
전체 `sh tools/validate_rust.sh`는 `RUST VALIDATION: ALL OK`이며 기존 ES2017/JS,
KLayout13 PX+2 phase-exact+14 style도 통과했다. native 의존성 경고는 남아 있다.
새 필터 UI의 브라우저 QA는 M2a-9b이며 현장 Firefox/ETX·G2는 여전히 보류다.

## 14. M2a-9b: Selected·live In view 목록 UI와 hover

§13을 기본 오류 목록/순회에 연결했다. **In view는 현재 규칙의 뷰 추종 체크박스**,
Selected는 현재 규칙의 선택 집합 체크박스다. 두 필터와 waive가 함께 교차하며
선택이 비어 있으면 빈 목록이다. 선택 자체를 바꾸는 박스/Shift/Ctrl/Cmd 동작과
목록을 좁히는 Selected는 별개다. 목록64개/페이지·명시적 다음/이전·유계 n/p 순회는
유지하고, 선택에 없는 오류/다른 규칙의 결과를 뒤섞지 않는다.

- 뷰 revision이 바뀌면 이전 목록·순회 요청과 cursor history를 버리고 새 첫
  페이지만 읽는다. pan/zoom 입력 중·WS 단절 중에는 조회하지 않는다. 연속된
  변경은 **100ms latest-only**로 묶으며 timer는 하나다. 빈 continuation은 계속
  미완료로 표시하고 자동으로 pack 전체를 따라 읽지 않는다. 이100ms는 DRC 목록
  갱신 지연이며 native geometry pan/crop을 지연시키는 debounce가 아니다.
- Selected가 켜진 동안 집합 revision이 바뀌어도 같은 방식으로 목록을 갱신한다.
  꺼져 있으면 집합 변경으로 목록을 다시 읽지 않는다. 선택 목록/쿼리/레이어 변경이
  스스로 native goto를 호출하지 않는다. focus/CD와 규칙별 선택 집합은 독립적으로
  유지되며 박스 선택 대상은 계속 현재 필터·페이지의 최대64개다.
- 응답의 scope/view/selection revision·rule/waive/selected/bbox·오름차순 local ID·
  진행 cursor를 검사한다. 늦은 응답은 token과 필터 키로 폐기한다. 오류/대기 중인
  목록을 `No matching errors` 완료로 표시하지 않으며 박스 선택을 잠근다.
- `in_view`/`selected_only` 두 boolean을 서버 panel 상태에 저장한다. 이전 body의
  생략값은 false다. 옛 고정 query 상태는 **복원만** 지원하고 당시 viewport임을
  표시한다. 새 체크박스를 조작하면 그 snapshot을 끝내고 현재 규칙으로 전환한다.
  snapshot+새 필터를 동시에 지정한 저장 요청은 거부한다.

복원은 선택 집합 GET→패널 GET/복원 순서다. Selected를 먼저 전체 오류로 표시하는
단계는 없다. 새로고침 중 WebSocket 접속/resize가 아직 진행 중이거나 live 조회가
view revision 충돌을 받으면 **패널 설정의 복원은 완료하고 목록만 준비된 뷰로 미룬다**.
선택 집합 GET 실패는 여전히 명시적인 복원 오류다. 오래된 reload의 GET이 도착해도
후속 패널 복원을 시작하지 않으며 자동 toggle 재전송/파일 쓰기는 없다.

마커 hover는 click과 동일한 **실제로 그린 마커의 화면 hit-list**를 사용한다.
6 CSS px 최근접 판정, 표시 crop/분수 원점/DPR를 유지하며 툴팁은
`룰명 #로컬 (전역) · waived`다. 같은 내용이면 DOM을 다시 쓰지 않고 좌표/pack/API를
조회하지 않는다. pointer leave·pan·필터/표시 변경 시 tooltip을 지운다. 전체 DRC
객체를 순회하는 hover가 아니며 마커 정책을 전체 pack으로 확대하지 않았다.

로컬 Chrome/DPR2 합성260오류 QA에서 빈 Selected, global1/2 선택 후 두 체크박스
교집합, 새로고침 후 번호/필터 보존을 확인했다. 첫 최종 번들 reload는 gen2와 뷰를
유지했다. 레이어를 숨긴 뒤 Shift+Right와 Up/Down pan에서는 **2→1→2개**로 목록이
따라왔고 집합은 두 개로 유지됐다. 다른 규칙은0개/전체 선택2개를 표시했고 돌아오면
원래 선택을 복구했다. clear 후0개, Selected off 후 live 목록/n 순회(global1, gen6)
및 재복원도 확인했다. 앞선 QA에서 발견한 초기 WS/resize 복원 실패를 위의 지연
조회로 고치고 새 세션에서 재현되지 않음을 확인했다. 두 세션 콘솔 warn/error는
없고 End session은 모두 exit0이다. 이 결과는 현장 Firefox/ETX 수용이 아니다.

새 JS gate는 100회 revision 알림의 단일 조회, 대기/단절/오래된 view·selection
응답 폐기, 3종 필터·빈 집합·순환·명시적 페이지, group GET 지연과 초기 WS/resize·
409 복원 경합, 무조회 hover/DOM 중복 갱신 금지를 단언한다. 기존 ES2017·마커·
box·CD·geometry·수명 게이트와 인증 HTTP384조합·panel 저장/원본 불변도 통과했다.

2026-09-13: 전체 `sh tools/validate_rust.sh` ALL OK 및 KLayout13 PX+2 phase-exact+
14 style 통과. 최종 보완을 포함한 JS/ES2017·core59/app6/web24/transport8·fmt·전환
패키지 strict clippy를 재확인했다. Rust1.89 빈 registry 오프라인 테스트와 최종
Linux musl release link도 통과했다. 기존 native 경고는 남아 있으며 Linux 실행
PASS나 M2 전체 완료를 뜻하지 않는다. 기존 내장 정적 자산 구조를 유지했고 외부
호스팅·의존성·native wire/캐시 포맷·renderd 버전은 바꾸지 않았다.

## 15. M2a-10a: SVRF sidecar·규칙 메타데이터/측정 코어

`rust/app-core/src/svrf.rs`는 기존 **`floe-svrf-rules` version 1 `.rules.json`**을
읽는다. GTK도 원본 deck 대신 이 파일을 읽는다. **원본 SVRF subset parser/CLI의
Rust 이관은 아직 M4에 남아 있다.** 이번 단계는 메타데이터 코어와 명시적인 CLI
읽기이며 웹 actor/타입 UI/레이어 격리는 다음 연결 단계다.

```sh
rust/target/release/floe2-web drc results.db.ice --rules --svrf-rules deck.rules.json
rust/target/release/floe2-web drc results.db.ice --errs M1.WIDTH --svrf-rules deck.rules.json
```

개발 CLI의 추가 옵션 `--svrf-rules FILE`을 명시할 때만 `--rules` 행에 `svrf`,
`--errs` 행에 `comparison`을 추가한다. 없는 규칙·측정 불가능한 오류는 각각 null이다.
기존 옵션만 쓰면 기존 JSON이 그대로이고 첫 중복 규칙 선택/경고도 유지된다.
메타데이터 옵션은 `--rules`/`--errs` 중 하나와만 사용하며 `--list` 병용은 거부한다.
DB/pack/sidecar의 읽기이며 자동 생성·갱신·Python fallback은 없다.

- 규칙명으로 match한다. description, constraint의 metric/op/value/text 및
  **미해석 값의 raw 원문**, 직접 layer, source_gds, unresolved를 보존한다.
  이 match는 올바른 deck/스위치 조합을 증명하는 provenance 검증이 아니다.
- 타입 코어는 규칙마다 metric을 중복 제거하고 기존 `width/space/enclosure/area/
  density/length/angle/perimeter/vertex/other` 순서, 그 밖의 metric은 이름 순이다.
  미매칭·measurement 없는 규칙은 other, 빈 sidecar는 타입 목록 자체가 비어 있다.
  DRC 파일의 동명 규칙은 각각 센다. 아직 웹 규칙 목록에 적용하지 않았다.
- derivation은 기존 ASCII operand/keyword subset과 breadth-first 순서를 쓰며
  cycle을 막고 6개 뒤 `derivations_more`를 명시한다. `source_gds`의 null datatype은
  모든 datatype이라는 매칭 helper를 둔다. 현재 설계에서 0개 match일 때 hide-all로
  해석하지 않는 것은 후속 격리 호출자의 계약이다.
- 측정은 단일 에지 length, 단순 에지 쌍의 진짜 gap, 축정렬 4점 사각형의 min span,
  polygon의 shoelace area만 지원한다. 나머지는 null이다. `< / <= / ==` 중 **처음
  측정 가능한 제약**을 우선하고 없으면 첫 측정 가능한 제약을 선택한다. bound=0은
  percent=null이다. marker의 측정값·bound·차이를 보여주는 참고값이지 재검증이나
  signoff PASS/FAIL 판정이 아니다(일반 다각형의 최소 폭/면적 Boolean 연산 아님).
- 면적은 원점을 뺀 i128 좌표에서 checked shoelace를 계산한다. i64 극단의 작은
  도형을 절대 f64 곱셈으로 상쇄시키지 않는다. i128 산술 overflow, 표현 불가능한
  면적 underflow/비유한 값·비교값은 오류이며 조용히0이나 infinity를 반환하지 않는다.
  이는 Python 절대좌표 shoelace의 반올림 손실을 재현하지 않는 정밀도 보완이다.

입력 경계: 전체16 MiB, checks/derived 각각65,536개, 규칙별 constraint1,024개,
layers/source_gds/unresolved 각각4,096개, 규칙 텍스트 합/각 derivation64 KiB,
metric64 bytes. map/배열 상한은 deserialize 중 적용하고 중복 key, 잘못된 타입,
비유한 값·알 수 없는 version은 명시 거부한다. GTK의 미래 version 경고 후 계속
읽기와 달리 이 typed reader는 version1만 수용한다. 새 제한은 표시를 자르는 budget이
아니며 RSS cap 주장도 아니다. 전체 파일 파싱은 유계 worker 작업으로 쓰도록 하며
즉시 취소 가능한 JSON parser라는 계약은 아니다.

파일은 nonblocking open 후 regular-file을 확인해 FIFO 입력이 대기하지 않는다.
읽기 전후 fd/path의 inode·size·mtime/ctime을 비교하고 취소를 검사한다. 받은 snapshot만
보관하며 **기록된 deck/include 경로를 따라가지 않는다.** root 권한 검사는 후속 웹
등록 계층의 책임이고 자동 탐색/공유 범위 확대/외부 변경 hot-reload를 추가하지 않았다.

`validate_app_svrf.py`는 실제 Python parser 출력과 GUI의 순수 메서드를 AST로
가져와(no GTK import) 비교한다. 합성/generated DB의 **1,052개 도형·제약 조합**, raw
미해석 값·중복 규칙·타입·6단 derivation·null·첫 upper-bound 선택을 검사한다. Python
area 반올림 차이는 ULP 범위와 독립적인 정수 DBU shoelace로 검증한다. Rust 실행은
PATH-empty이고 전후 파일 목록·mtime·hash 불변, corrupt/미래 version/초과/FIFO/디렉터리/
missing 파일의 실패도 단언한다. 이 gate를 전체 배터리에 추가했다.

2026-09-13: 전체 `sh tools/validate_rust.sh` ALL OK, KLayout13 PX+2 phase-exact+
14 style 통과. core65/app6/web24/transport8 테스트, fmt·전환 패키지 strict clippy,
Rust1.89 빈 registry 오프라인 테스트와 Linux musl release link도 통과했다. 기존
native/Pillow 경고는 남아 있다. native wire/캐시·renderd 버전·GTK 기본값은 바꾸지
않았다. Linux 실행이나 브라우저 기능 추가·현장 Firefox/ETX PASS를 뜻하지 않는다.

## 16. 다음 경계

1. SVRF sidecar 코어는 §15까지 이관했다. actor/웹 metric/type 필터·layer isolate는
   아직 미연결이다. 기존 GTK의 jump 시 In view 해제도 함께 parity 검증해야 한다.
   selected/live In view 목록 필터·순회·hover는 §13~14까지 구현했다.
   손으로 그리는 ruler/격리와 Escape 우선순위도 M4에서 확장한다.
   현재 페이지 마커 정책 자체를 전체 pack 마커로 확대하지 않는다.
2. ASCII/index 흐름·기존 notes·상세 측정/룰 매핑은 각각 parity gate와 함께 확장.
3. 공유는 설계/DRC에 묶인 읽기 capability, 발급/만료/폐기·follow/independent
   state를 별도 구현·검증. 아직 shares=false, loopback-only다.
4. 외부 HTTPS/WSS·TeeBox 접근/인증 정책은 상위 계획 §10 미결 사항이며 로컬 기반 구현과
   실제 외부 공개를 구별한다. 브라우저 주소만 외부 IP로 바꿔 노출하지 않는다.
