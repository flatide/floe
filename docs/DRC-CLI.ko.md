# DRC CLI 스크립팅 레퍼런스

셸 스크립트/자동화에서 쓰는 DRC 명령 모음. 뷰어 없이 룰 목록·에러
목록을 뽑고 에러 스냅샷 PNG를 만드는 표면이다. (뷰어 연동·포맷
내부는 [DRC.ko.md](DRC.ko.md) 참고.)

실행 형태는 환경에 따라:

```bash
rust/target/release/floe2 <cmd> ...          # 소스 체크아웃
floe2 <cmd> ... / floe-index drc|svrf ...    # floe2-portable 번들
```

0.12.333(P3)부터 제품 명령은 `floe2`다. 예전 문서의 `floe drc`·`floe render`는 아래의 `floe2 drc`·`floe2 render`와
같은 명령이고, Python 셸 `floe`는 개발용 오라클로만 남았다. 캡처의 일반 옵션(영역·크기·레이어·모양·인덱스)과
종료 코드·환경 변수는 [CAPTURE-CLI.ko.md](CAPTURE-CLI.ko.md)에 있다.

**공통 규약**

- stdout = 데이터(JSON/TSV)만. 진행·경고(`[floe2] ...`, `[drc] ...`)는 전부
  stderr — 스크립트는 stdout만 파싱하면 된다.
- 실패 = 비-0 exit + stderr `floe2: ...` 한 줄(명령행 오류 2, 실행 중 입력 오류 1).
- 에러 번호는 두 종류: **local** = 룰 안 1-based 순번(뷰어
  그리드와 동일), **global** = 파일 순서 전역 번호(Calibre RVE
  스타일, 뷰어 상세의 `#local(global)`).
- 리뷰어: waive 상태(`waived`, `status`)는 `--floe-reviewer ID`(또는 `FLOE_REVIEWER`)의 것을
  읽는다. 주지 않으면 DISPLAY 호스트·SSH 접속원·계정명 순으로 얻은 ID인데, 스크립트에서는 대개
  아무의 리뷰도 아니므로 0이 나온다. `FLOE_REVIEWER_REQUIRED=1`인 환경에서는 ID 없이 읽는 명령이
  1로 끝난다(0.12.440). 리뷰어 전원의 수치는 §2b의 `--review-stats`가 낸다.
- status: `0` = not waived, `1` = waived. waive 변경은 뷰어
  전용(우클릭 메뉴 / `w` 키) — CLI 쓰기 표면은 없고, 재-pack 시
  초기화된다.

---

## 0. 준비: pack 인덱스

```bash
floe-index drc results.db [out.tray] [--jobs N]
```

`.results.db.tray`(v2 pack, db 옆 숨김 파일; 2026-09-16까지의 `results.db.ice`는
발견 시 자동 개명)를 만든다. `floe2 … --workdir DIR`(또는 `FLOE_WORKDIR`)이면 pack과
그 뒤를 잇는 `.results.db.analysis/`·리뷰 `.sqlite3`·waive·notes 사이드카가 db 옆이 아니라
DIR 아래 db의 절대 경로 자리에 놓인다(CACHE-NAMING.ko.md §6). `floe-index drc`를 단독으로
돌릴 때도 같은 `--workdir`과 `FLOE_WORKDIR`을 받고, 명시한 `[out.tray]`는 그대로다. `floe2 drc DB --build`
(명시 빌드·재사용·구 이름 개명)는 0.12.428부터 pack의 `.floe-lock` 잠금을 `floe-index drc`와 같은 키로
쥔다. 다른 빌드가 쥐고 있으면 75로 누가 쥐었는지 대고, 리뷰어가 연 pack은 `--force` 교체를 "in use"로
거절하며, 그대로 재사용하는 `--build`는 지나간다(CACHE-NAMING.ko.md §5). db 옆에 쓸 수 없는 폴더면
`--build`는 `cannot write beside results.db, in /r: Permission denied (os error 13) - … --workdir DIR (or
FLOE_WORKDIR) puts them under a directory this account can write, at the source's absolute path: DIR/r/`로
1로 끝나 `--workdir`을 안내한다(0.12.430; CACHE-NAMING.ko.md §6). 이후 모든 명령이 신선한
pack을 자동 사용한다. 신선함은 .db가 색인한 그 파일인지다(0.12.441). 크기와 시각이 색인 때와 같으면
그대로 쓰고, 크기는 같고 시각만 다르면 .db를 한 번 읽어 pack에 적힌 CRC-32와 견준다(복사, 복원,
touch는 같은 파일이다). 이 문서의 명령들은 .db의 크기와 무관하게 견준다. 견준 결과는 pack 옆
`.results.db.tray.source`에 남아 다음부터는 읽지 않는다. `floe2 drc DB --build`도 같은 확인을 하고
같으면 pack을 그대로 둔다(`reused: true`). `FLOE_DRC_VERIFY_MB`(MiB, 기본 1024)는 뷰어가 메뉴에서
.db를 골라 pack을 열 때 읽을 크기의 상한일 뿐이다(DRC.ko.md §2). 내용이 다른 .db에는
`--build`가 `existing DRC pack is stale (the source is not the file that was indexed); explicit
--force is required`로 1로 끝나고, `--build --force`나 `floe-index drc`가 다시 색인한다. 다시 색인한
pack의 에러가 같으면 리뷰는 그대로 붙는다(내용 ID; DRC.ko.md §2 '결과 식별 = 내용'). 레이아웃 4
pack(0.12.440까지)은 읽지 않으므로 다시 색인한다. pack이 없거나 낡으면
**ASCII 전체 파스로 폴백**(stderr 안내) — 수백 MB급부터 매우
느리므로 스크립트는 pack을 먼저 만들 것. `--pack`은 호환용
no-op. 좌표 토큰이 정수가 아닌 db는 명확한 메시지와 함께 거부.

### 대규모 룰의 마커 중심·CD/델타 사전 준비 (`drc-prepare`)

```bash
floe2 drc-prepare results.db --svrf deck.svrf.rules.json
# 한 룰 안의 좌표 블록을 작업자 4개로 병렬 처리
floe2 drc-prepare results.db --svrf deck.svrf.rules.json --jobs 4
# 특정 룰만 준비 (--rule 반복 가능)
floe2 drc-prepare results.db --svrf deck.svrf.rules.json --rule M1.SPACE
# SVRF 없이 마커 중심만 준비
floe2 drc-prepare results.db --spatial-only
# 브랜치의 명령도 그대로: floe-index가 옆의 floe2에 넘긴다
floe-index drc-prepare results.db --svrf deck.svrf.rules.json
```

기존 `.tray`를 읽고 룰별로 캔버스 마커가 읽는 정확한 중심(에러당 16B)과
CD 측정표, 절대/비율 델타의 기본(auto) 그룹을 `.results.db.analysis/`에
저장한다(app-core `drc::analysis`, 2026-10-11 Rust 이식 - 브랜치의 Python/NumPy
계층과 floe-index drc-measure/drc-centers 프로토콜을 대신한다). pack이나 개인
waive/notes는 변경하지 않는다. pack이 없거나 낡으면 중단하며 ASCII 전체 로딩으로
우회하지 않는다. `--delta-only`로 CD/델타만 준비할 수 있다. 진행률은 stderr,
취소는 Ctrl+C(종료 코드 130)이며 완료된 룰의 캐시는 재실행 때 사용한다. 진행 중인
룰은 다시 준비한다. 실제 에러가 0개인 룰은 건너뛰고, 완료 메시지에서 준비한 룰 수와
건너뛴 룰 수를 따로 표시한다. `--svrf`에는 원문 SVRF가 아니라 `floe-index svrf`로
생성한 `.rules.json`을 지정한다(없으면 `--spatial-only`만 가능).

`--jobs N`은 룰 내부 병렬 작업자 수(1~256), 기본은 CPU 수와 4 중 작은 값이다.
환경변수 `FLOE_DRC_JOBS=N`으로 뷰어의 측정·마커 작업에도 같은 수를 적용할 수 있다.
`--backend auto|rust`는 받아들이고 무시하며(`python`은 거부), 수치 경계를 Python
프로세스로 재확인하던 경로도 없다 - Rust 산술이 CPython의 `math.hypot`·`math.fsum`과
`Decimal(str(value))`를 그대로 따른다(게이트 `drc_analysis`가 기준 구현과 대조).

읽기 전용/네트워크 결과 폴더라면 `FLOE_DRC_ANALYSIS_ROOT=/local/ssd/drc-cache`로
별도 저장소를 지정한다. 뷰어에도 같은 환경변수를 지정해야 같은 캐시를 사용한다.
전체 14억 건의 측정표와 두 모드 그룹은 약 40.6GB, 중심은 약 22.4GB이므로 필요한
룰부터 준비할 수 있다.

2026-10-11 Apple M4(8코어), 작업자 4개, `gen_drcdb --heavy 10000000`의 1천만 에러
룰: 뷰어 서비스에서 첫 측정과 절대 그룹 2.36초, 같은 측정표의 비율 그룹 0.25초,
다시 연 뒤 0.02초(캐시), 중심 캐시를 만들며 그린 첫 마커 0.33초. OS 파일 캐시는
따뜻한 상태였다. 실제 형상과 조건 수, 저장장치에 따라 달라진다.

### 이전 실행과의 도형 대응 (`drc-match`)

```bash
floe2 drc-match OLD.tray NEW.tray --plan pairs.json --states flags.u8 \
    --out records.bin --work-dir /tmp/match [--jobs 4] [--memory-mib 64] \
    [--progress progress.json] [--report report.json]
floe2 drc-match --protocol        # 2
```

뷰어의 `review from previous run…`은 app-core `drc::matcher`를 직접 부른다. 이
명령은 같은 매처를 스크립트와 기준 구현(`tools/oracle/floe_oracle/drc_migrate.py`)에
내준다(2026-10-11; 브랜치의 `floe-index drc-match`를 app-core로 옮겼다).
`pairs.json`은 `[[이전 룰, 새 룰], ...]`(양쪽에서 유일한 같은 이름),
`flags.u8`은 이전 pack 전역 에러당 1바이트(V=1, W=2, NR=4)다. `records.bin`은
16바이트 레코드를 (룰, 에러) 순으로 쓴다(SPEC-FORMATS `.floereview` 절). 출력은 모두
새 파일이어야 하고 입력과 달라야 한다. 해시 그룹의 바이트 비교는 pack을 읽기 전용으로
매핑해 대상 에러까지만 디코딩하고 작업자들이 나눠 한다. Apple M4에서 작업 상태가 모두
있는 2백만 에러 룰이 4개 작업자로 2.7초(1개 6.6초)였다.

## 1. 룰 목록: `floe2 drc <db> --rules`

```bash
floe2 drc results.db --rules > rules.json
```

JSON 배열, 룰당 한 객체:

```json
[
 {"name": "M1.SPACE.1", "errors": 1523, "waived": 12},
 ...
]
```

플래그 없이 실행하면 사람용 요약(셀·precision·룰별 카운트),
`--list`는 에러별 중심/크기(um)까지 출력한다.

## 2. 한 룰의 에러 목록: `floe2 drc <db> --errs RULE`

```bash
floe2 drc results.db --errs M1.SPACE.1 > errs.json
```

JSON 배열(객체 단위 스트리밍 — 수백만 에러 룰도 상주 메모리
없이 흘러나옴):

```json
[
 {"local": 1, "global": 8841, "kind": "p", "status": 0,
  "bbox": [123.45, 67.89, 123.61, 68.02]},
 ...
]
```

- `kind`: `"p"` = polygon, `"e"` = edge.
- `bbox`: um, 소수 4자리 반올림.
- 룰 이름이 중복이면 첫 매치 사용 + stderr 경고.

**캡처 상한 주의(ProperTee 등)**: 셸 출력 직캡처가 잘리는
환경(ProperTee는 ~1MiB)에서는 반드시 **파일로 리다이렉션한 뒤
json_parse** 할 것. 파이프 직캡처는 지원 경로가 아니다.

## 2b. 리뷰어 통계: `floe2 drc <db> --review-stats` (0.12.440)

```bash
floe2 drc results.db --review-stats > review_stats.json
floe2 drc results.db --review-stats --floe-reviewer kim,lee > two.json   # 이 리뷰어들만
floe2 --workdir /work/floe drc results.db --review-stats > stats.json     # 리뷰가 workdir에 있을 때
```

뷰어 없이 리뷰어 전원의 검토 수치와 팀 합계를 JSON 한 문서로 낸다(사용자 2026-11-10: TeeBox가
같은 레이아웃·db를 검토하는 사람들의 통계를 수집). 쓰는 것은 없다. 뷰어가 그 결과를 열고 있는
동안에도 읽는다. 0.12.446부터 뷰어의 waive는 요청마다 디스크에 동기화되므로(`FLOE_WAIVE_SYNC`,
DRC.ko.md §2) 다른 호스트에서 읽어도 방금 한 것까지 들어 있다. 같은 버전부터 한 창이 결과를 바꿔
가며 여므로(SPEC-VIEWER.ko.md §7 "창 하나") 뷰어의 종료는 검토의 끝이 아니다. 런처는 필요할 때 이
명령을 부른다(사용자 2026-11-11: "통계는 별도의 cli 명령으로").

- **리뷰어 찾기**: pack 옆(결과 폴더, `--workdir`이면 그 미러)의 작업 파일에서 찾는다. waive
  사이드카 `.<db>.waive.<ID>`와 방문·검토 제외 `.<db>.review.<ID>.sqlite3`다. 결과 폴더에 쓸 수
  없어 이 계정이 다른 곳에 둔 것도 본다: 임시 폴더의 waive 사이드카(`waive_location: temp`),
  `FLOE_DRC_REVIEW_ROOT`나 사용자 데이터 폴더의 방문 기록(`review-root`, `user-data`). 임시 폴더는
  호스트마다 다르므로 그런 리뷰는 그 호스트에서 돌려야 보인다. 치워 둔 `.stale-<시각>`과 만드는
  중인 `.tmp-<pid>`는 리뷰어가 아니다.
- **다시 센다**: waived는 사이드카의 상태 바이트를 읽어 센 값이다. 저장된 카운터는 비교만 해서
  `counters_match`로 알린다. 0.12.439까지 같은 ID의 두 세션이 카운터를 어긋나게 둘 수 있었고,
  `--rules`의 `waived`는 그 카운터다. 방문·검토 제외는 비트맵에서 센다.
- **팀 합계**: 뷰어 공동 검토 창(`reviewers…`)에서 전원을 불러와 본 수치와 같다. 방문은 합집합,
  waive와 검토 제외는 누구든 한 것이되, **둘 다 방문한 리뷰어의 판단이 서로 다른 에러는 conflict**
  로 따로 세고 waived·not_required에서 뺀다. waive 파일만 있는 리뷰어(방문 기록 없음)는 conflict를
  만들지 않는다. `pending` = 전체 − (방문했거나 검토 제외인 에러)다. 방문 없이 일괄 waive한 에러는
  pending에 남는다(방문과 waive는 독립).
- **pack이 현재 것이어야 한다**: pack이 없거나 `.db`가 인덱싱 뒤 바뀌었으면(크기나 내용; 시각만
  바뀐 `.db`는 현재 것이다, 0.12.441, §0) 0을 내지 않고 1로 끝나며 무엇을 할지 말한다. 다른 실행이 pack을 만드는 중이면 75다. `--rules`·`--errs`·
  `--list`·`--build`와 함께 쓸 수 없다(종료 2).
- **다른 결과의 파일**: pack 지문(내용 ID)이 다른 사이드카는 세지 않고 `skipped`에 이유와 함께
  적는다. 에러가 달라진 재실행 전의 파일이 그렇다. 그 리뷰어가 새 결과를 열면 파일은
  `.stale-<시각>`으로 치워지고(0.12.441) 그 뒤로는 새 리뷰가 세어진다.
- **비용**: 리뷰어마다 에러당 1바이트를 읽는다. 14억 에러 결과에 열 명이면 14 GB를 읽는다.

```json
{
  "type": "floe-drc-review-statistics", "version": 1,
  "generated": "2026-11-10T09:30:09Z",
  "database": "/r/results.db", "pack": "/r/.results.db.tray",
  "fingerprint": "b701…", "cell": "TOP", "errors": 9100,
  "rules": [{"index": 1, "name": "A.WIDTH", "errors": 6000}, …],
  "reviewers": [
    {"id": "kim", "present": true,
     "waive_known": true, "visited_known": true, "not_required_known": true,
     "waived": 10, "visited": 5, "not_required": 10, "visited_not_required": 0, "pending": 9085,
     "counters_match": true,
     "waive_file": "/r/.results.db.waive.kim", "waive_location": "results",
     "waive_modified": "2026-11-10T08:42:20Z",
     "review_file": "/r/.results.db.review.kim.sqlite3", "review_location": "results",
     "review_modified": "2026-11-10T08:42:21Z",
     "rules": [{"index": 1, "name": "A.WIDTH", "waived": 10, "visited": 5,
                "not_required": 10, "visited_not_required": 0, "pending": 5985}]},
    {"id": "park", "present": true, "waive_known": true, "visited_known": false,
     "not_required_known": false, "waived": 0, "counters_match": true, …, "rules": []}
  ],
  "team": {"reviewers": 3, "errors": 9100, "visited": 7, "waived": 15, "not_required": 10,
           "conflicts": 1, "pending": 9083,
           "rules": [{"index": 1, "name": "A.WIDTH", "errors": 6000, "visited": 7, "waived": 14,
                      "not_required": 10, "conflicts": 1, "pending": 5983}, …]},
  "skipped": [{"reviewer": "ghost", "file": "/r/.results.db.waive.ghost",
               "reason": "waive file of another DRC result (its header or size is not this pack's)"}]
}
```

| 필드 | 의미 |
|---|---|
| `fingerprint` | pack의 내용 ID(hex 64자, 0.12.441): 에러가 무엇인지의 SHA-256. 에러가 같은 결과는 값이 같다(복사본, 다시 만든 pack, 같은 에러가 나온 재실행). 수집하는 쪽에서 두 보고서가 같은 결과의 것인지 견주는 키다 |
| `reviewers[].rules` | 그 리뷰어가 작업한 룰만(waived·visited·not_required 중 하나라도 0이 아닌 룰). 없는 룰은 0 |
| `reviewers[].present` | `--floe-reviewer`로 지정했는데 작업 파일이 없는 ID는 `false`이고 수치가 없다 |
| `*_known` | 그 차원의 파일이 있는지. waive 파일만 있으면 방문·검토 제외는 0이 아니라 모름이라 키가 없다 |
| `counters_match`, `counter_mismatch[]` | 저장된 waived 카운터가 센 값과 같은지, 다르면 룰별 `stored`·`counted` |
| `waive_modified`, `review_modified` | 파일의 마지막 수정 시각(UTC). 마지막 작업 시각의 근사 |
| `team.rules` | 모든 룰. `team.reviewers`는 센 리뷰어 수 |

키는 사전순으로 나온다. ProperTee처럼 캡처 상한이 있는 호출자는 파일로 받아 `json_parse`한다(§2).
같은 ID로 두 세션을 열 수 없게 된 것(세션 잠금)과 ID 요구 스위치는 DRC.ko.md §2에 있다.

## 3. 에러 스냅샷 PNG: `floe2 render --drc`

```bash
floe2 render chip.oas --drc results.db --drc-rule M1.SPACE.1 \
    --drc-err 1-50 --px 800 --out snap.png > made.tsv
```

| 옵션 | 기본 | 의미 |
|---|---|---|
| `--drc FILE.db` | — | 결과 db (`--drc-rule`과 항상 함께) |
| `--drc-rule NAME` | — | 룰 이름 (`--rules`에서 얻은 것) |
| `--drc-err N\|A-B\|all` | all | local 번호(1-based). 명시 N/A-B는 **전량** 렌더 |
| `--drc-cap N` | 200 | `all`일 때만 적용되는 상한 (초과분 stderr 경고) |
| `--drc-frac F` | 0.3 | 프레임 대비 에러 스팬 비율(뷰어 점프 프레이밍 동일) |
| `--px N` | 1200 | 정사각 한 변 px. 4096까지이고 넘으면 4096으로 줄여 그리며 stdout에 WARNING을 낸다(0.12.431; `FLOE_CAPTURE_REDUCE=off`면 거부). `max`는 4096. 영역 캡처의 생략값이 0.12.433부터 가장 큰 크기로 바뀐 뒤에도 스냅샷의 생략값은 1200이다(`FLOE_CAPTURE_PX`도 스냅샷에는 적용되지 않는다) |
| `--out PATH` | 룰명.png | 출력 스템: 1장 = 그대로, 여러 장 = `stem_<local>.png` |
| `--depth N` | full | 계층 깊이 (기본 = 전체 전개, 뷰어 "99") |
| `--layers ...` | 자동 | 명시하면 svrf 격리보다 우선 |
| `--drc-rules RULES.json` | 자동 탐색 | svrf 사이드카 직접 지정 |
| `--detail`, `--density`, `--thin`, `--occupancy`, `--label-font-px`, `--layer-colors`, `--legend`, `--archival`, `--no-index`, `--temp-index`, `--light`/`--full`, `--wait` | 일반 캡처와 같음 | [CAPTURE-CLI.ko.md](CAPTURE-CLI.ko.md) §1.3·§1.5. 영역(`--bbox`/`--at`/mosaic)·`--batch`·`--report`·`--chipid`/`--chipnumber`와는 함께 쓸 수 없다 |
| `--floe-reviewer NAME` | DISPLAY/SSH 유도 | 기존 waive 상태를 읽는 리뷰어 태그; 캡처는 리뷰를 쓰지 않는다 (`floe2 drc`/`view`에도 있음; FLOE_REVIEWER env 동치). `FLOE_REVIEWER_REQUIRED=1`이면 필수(0.12.440) |

- **픽셀 = 뷰어와 동일**: 뷰어의 렌더 서비스 경로(상주 vfsd
  세션 + detail medium 컷/헤어라인/LOD/커버리지) 그대로, depth만
  full. 첫 에러가 콜드 스타트(수 초, 타임아웃 300s)를 내고 이후
  에러당 ms급.
- **디코드 예산**(`FLOE_RUST_BUDGET_MB`, 2026-10-12): 레이아웃 캡처와 같다(README Jobdeck 절 아래).
  - 컷 0 프레임이 예산을 넘으면 필요한 양과 길(`--detail high`, `--layers`, 예산 올리기)을 말하며 실패한다.
  - `--detail`로 컷을 줬을 때 예산 때문에 성기게 그렸으면 `[drc][warn] … thinned to fit the render budget: …`으로 알린다.
- **`--detail`·`--density`**(2026-10-12): detail 기본은 뷰어와 같은 medium이다(0.12.349까지는 컷 0; `--archival`이면 컷 0).
  - 밀도는 뷰어처럼 기본으로 켜져 컷 아래 도형을 밀도로 그린다(0.12.351). `--density off`나 `FLOE_RUST_DENSITY_STACK=off`로 끈다.
- **`--layer-colors row|group`**(2026-10-13): 레이어 기본 색. row(기본)는 패널 줄마다 다른 색, group은 layer 번호마다 한 색이다.
  범례(fe-embed) 색도 같다. `FLOE_LAYER_COLORS`가 기본값이다.
- **레이어 격리**: rules.json 사이드카가 발견되면(탐색 순서 =
  db 옆 덱 basename → 기록된 덱 경로 → `<db>.rules.json`, 작업 디렉터리가 있으면 이어서 그 셋의
  DIR 아래 자리(0.12.450), 또는 `--drc-rules`) 그 룰의 원천 GDS 레이어만 켠다 — 뷰어 더블클릭
  격리와 동일. 없으면 전 레이어 + stderr 안내.
- **stdout = TSV**, 저장 파일당 한 줄:

  ```
  local#<TAB>global#<TAB>path
  ```

- PNG에는 에러 도형(상태색 red/green), CD 룰러(µm 길이 자동
  라벨), note(`룰 #local(global)`), 그리고 격리된 캡처면 켜진
  레이어의 legend(색 + fill 패턴 **이름**, 예 `speckle`)가
  **flateyes embed**(iTXt 청크)로 실린다 — flateyes(1.16+)로
  열면 주석·범례 표시/편집, 다른 도구에선 평범한 PNG.
  모든 레이어를 그린 캡처는 legend가 없고, `--legend`(2026-10-13)를 주면 레이어 패널 줄 순서로 모든 레이어의
  legend가 실린다. legend는 그림을 그린 스타일에서 만든다(app-core `legend.rs`, 일반 캡처의 `--legend`와 같은 코드).
  스크립트에서 주석만 뽑으려면 `floe2 fe-embed --dump x.png`.
- 사각형 룰러는 뷰어와 같은 SVRF 오류 방향 선택을 사용한다. 한 방향만
  해당하면 하나, 양쪽 해당/판단 불가이면 둘을 임베드한다. `--layers`를
  지정해도 룰러 판단용 사이드카를 읽는다. 룰러의 표시 자릿수는 외부
  flateyes에서 정하며 PNG에는 좌표와 ppu를 저장한다.
- area 전용 룰은 길이 룰러 대신 `area 0.12345 µm²` 면적 텍스트를
  임베드한다. 폴리곤의 실제 면적을 사용하고 혼합 검사면 길이 룰러와
  면적 텍스트를 함께 넣는다.

## 4. SVRF 사이드카 생성: `floe-index svrf`

스냅샷 레이어 격리와 뷰어 디테일(제약/원천 레이어)을 살리려면
룰덱에서 rules.json을 한 번 만들어 둔다:

```bash
floe-index svrf deck.cal --scan            # 새 덱 인벤토리(파스 없이 확인)
source sourceme.sfa14_ALL && \
floe-index svrf deck.cal --follow-verbatim # 실런과 같은 환경으로 생성
```

- 출력 기본 `<deck>.rules.json` (`-o`로 변경). db 옆에 두면 자동
  탐색된다.
- `--workdir DIR`(또는 `FLOE_WORKDIR`)이면 기본 출력이 덱 옆이 아니라 `DIR/<덱의 절대 경로>.rules.json`으로
  가고 잠금도 그 옆에 생긴다(0.12.450). 덱 폴더에 쓸 수 없는 곳에서는 `-o` 없이 이렇게 돌린다. 뷰어와
  스냅샷은 db 옆과 덱 옆에서 못 찾으면 DIR의 같은 자리에서 찾는다. `-o FILE`은 준 자리에 그대로 쓴다.
  폴더 없이 이름만 주면 현재 디렉터리이고 `--workdir`이 옮겨 주지 않는다(CACHE-NAMING.ko.md §6).
- `-D NAME[=V]` 반복 지정 = 실런 스위치 재현(-D가 환경변수보다
  우선). 환경변수 폴백이 기본이므로 sourceme를 source 한 셸에서
  돌리면 -D 나열이 거의 필요 없다(`--no-env-switches`로 끔).
- `-I DIR` = INCLUDE 탐색 경로 추가, `--follow-verbatim` =
  VERBATIM/Tcl 블록 안 INCLUDE도 추적.
- 2026-09-29 `floe svrf`에서 **floe-index로 이동**(DRC pack `floe-index drc`,
  레이아웃 캐시 `floe-index vfs`와 같은 자리). 옵션·출력(`.rules.json`
  바이트까지)은 그대로이고, `floe svrf …`는 같은 옵션의 floe-index
  명령줄을 알려 주고 exit 2로 끝난다(스크립트는 명령 이름만 바꾸면 된다).
  `-DNAME`·`-D=NAME`·`--define=NAME`도 `-D NAME`과 같다.

## 5. 엔드-투-엔드 예시

```bash
set -e
DB=results.db; SRC=chip.oas; DECK=deck.cal
mkdir -p snap

floe-index drc "$DB" --jobs 8                      # 0. pack
floe-index svrf "$DECK" -o "$DB.rules.json"        # 1. 사이드카(1회)
floe2 drc "$DB" --rules > rules.json                # 2. 룰 목록

# 3. 에러 있는 룰마다 앞 20개 스냅샷
for RULE in $(python3 -c "
import json
for r in json.load(open('rules.json')):
    if r['errors']: print(r['name'])"); do
  floe2 drc "$DB" --errs "$RULE" > "errs_$RULE.json"
  floe2 render "$SRC" --drc "$DB" --drc-rule "$RULE" \
      --drc-err 1-20 --px 800 --out "snap/$RULE.png" \
      >> made.tsv
done
```

ProperTee에서는 각 단계를 파일 리다이렉션으로 받고
`json_parse`/TSV 파싱으로 읽는다(§2 캡처 상한).
