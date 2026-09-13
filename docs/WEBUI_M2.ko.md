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

## 3. 다음 경계

1. rule/error 목록·검색·waive 필터·goto·marker overlay를 브라우저에 연결한다.
2. ASCII/index 흐름·기존 notes·상세 측정/룰 매핑은 각각 parity gate와 함께 확장.
3. 공유는 설계/DRC에 묶인 읽기 capability, 발급/만료/폐기·follow/independent
   state를 별도 구현·검증. 아직 shares=false, loopback-only다.
4. 외부 HTTPS/WSS·TeeBox 접근/인증 정책은 §10 미결 사항이며 로컬 기반 구현과
   실제 외부 공개를 구별한다. 브라우저 주소만 외부 IP로 바꿔 노출하지 않는다.
