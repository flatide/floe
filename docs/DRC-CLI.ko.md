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
  1로 끝난다(0.12.440). 리뷰어 전원의 수치는 §2b의 `--review-stats`가 내고, 바로 답해야 하는
  조회에는 §2c의 `--review-counts`를 쓴다(0.12.464). 전원의 합집합(팀 합계)도 §2c가 바로
  답한다(0.12.465).
- status: `0` = not waived, `1` = waived. waive 변경은 뷰어
  전용(우클릭 메뉴 / `w` 키) — CLI 쓰기 표면은 없고, 재-pack 시
  초기화된다.

---

## 0. 준비: pack 인덱스

```bash
floe-index drc results.db [out.tray] [--force] [--jobs N]
```

**현재 pack은 그대로 둔다(0.12.459; 사용자: "floe-index drc도 .tray가 있어도 다시 생성하는 것 같음").**
0.12.458까지 이 명령은 실행할 때마다 .db 전체를 다시 파싱해 pack 전체를 임시 파일로 다시 썼고,
끝에서 바이트가 같을 때만 기존 파일을 남겼다. 이제 잠금을 잡기 전에 `floe2 drc DB --build`가 묻는
것을 스스로 묻는다(`rust/cli/src/drccurrent.rs`).
- **현재의 뜻:** pack이 리더가 여는 방식으로 온전히 읽히고(app-core `Pack::open`의 구조 검사와 같은
  검사), .db가 색인한 그 파일이다. 크기와 시각이 pack에 적힌 것과 같으면 읽지 않고 현재다. 시각만
  다르면 .db를 한 번 읽어 CRC-32를 견주고, 같으면 `.results.db.tray.source`에 적어 둔다.
- **현재이면:** 아무것도 쓰지 않고 잠금도 잡지 않으며 `… is current … - kept; --force packs it
  again`을 말하고 0으로 끝난다. 리뷰어가 pack을 열고 있어도 0이다(전에는 75).
- **다시 만드는 경우:** `--force`(리뷰어가 열고 있으면 전처럼 75), .db가 바뀌었을 때, pack이 온전히
  읽히지 않거나 다른 레이아웃 버전일 때. 뒤의 둘은 이유를 `… is not current - <이유> - packing it
  again`으로 말한다.
- **알아둘 점:** 크기와 시각이 색인 때와 같으면 내용을 읽지 않는다. 같은 초 안에 같은 크기로 내용만
  바뀐 .db는 `--force`가 필요하다. `floe2 drc --build`와 뷰어도 같은 기준이다.
- 다른 실행이 pack을 만드는 중이면 묻지 않고 전처럼 75로 거절된다.

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

**`--svrf`로 준 사이드카는 뷰어가 찾는다(0.12.460; 사용자: ".ice, .db.tray, .db.analysis가 모두 준비된
상태에서 floe2 view layout.oas --drc error.db --workdir /user/tmp 로 실행하면 svrf 적용도 되어 있어야
할 것 같은데 되어 있지 않음").** 뷰어는 사이드카를 db에 기록된 덱 경로와 db 이름으로 찾는데,
`floe-index svrf DECK --workdir DIR`은 지금 넘긴 덱의 실제 경로 아래에 쓴다. 두 경로가 다르면(Calibre를
돌린 곳의 마운트 경로, 복사해 온 덱, 상대 경로) 뷰어가 찾지 못했고, `drc-prepare`는 받은 파일을 어디에도
남기지 않았다. 이제 `drc-prepare`가 받은 사이드카의 절대 경로를 분석 폴더의 `rules.path`에 한 줄로
적어 두고(`Rules sidecar noted for the viewer: …`), 뷰어의 db 로드와 DRC 스냅샷이 그 자리를 맨 먼저
본다(app-core `svrf::rules_candidates`).
- 순서: `--rules FILE`(지정하면 그것만), 적어 둔 사이드카, db 옆의 `<덱 이름>.rules.json`, 기록된 덱
  경로의 `<deck>.rules.json`, `<db>.rules.json`, 작업 디렉터리가 있으면 그 셋의 미러.
- 적힌 파일이 없으면 빈 자리로 지나가고 다음 자리를 본다. `--spatial-only`는 메모를 건드리지 않고, 다른
  사이드카로 다시 준비하면 그것으로 바뀐다. 같은 경로면 파일을 다시 쓰지 않는다.
- 메모는 분석 폴더와 함께 있다: db 옆(`--workdir`이면 DIR 아래)이나 `FLOE_DRC_ANALYSIS_ROOT`. 절대
  경로라서 결과 폴더를 다른 곳으로 복사하면 그쪽에서 `drc-prepare`를 다시 돌려야 메모가 그곳의
  사이드카를 가리킨다(캐시는 그대로 쓰이므로 금방 끝난다).

**분석 캐시는 pack의 내용에 묶인다(0.12.459).** 0.12.458까지는 pack **파일**의 스탬프(크기·시각·inode)에
묶여, pack을 다시 만들거나 결과 폴더를 복사하거나 touch하면 캐시 전부를 다시 계산했고 옛 캐시는
남았다. 이제 pack의 내용 ID(에러가 무엇인가; DRC.ko.md §2 '결과 식별 = 내용')에 묶인다(app-core
`store::content_identity`).
- 같은 결과로 다시 만든 pack, 다른 폴더로 복사한 결과, 시각이 바뀐 파일은 캐시를 그대로 쓴다.
  에러가 다르면 다른 식별값이다.
- `drc-prepare`는 시작할 때 다른 식별값의 캐시(0.12.458까지의 것, 같은 이름으로 교체된 예전 결과의
  것)를 지우고 `Removed N outdated cache entries (… MB)`로 말한다. 그 pack을 연 창이나 실행이 있으면
  지우지 않고 `N outdated cache entries left …`로 말한다.
- 지우는 것은 이 캐시의 것이 확실한 항목뿐이다: 다른 식별값을 이름에 가진 중심 폴더와 그 `.lock`,
  다른 pack을 key에 적은 완성된 측정표 폴더. 아무것도 말하지 않는 항목(측정 중인 표), 다른 도구의
  파일, 링크는 둔다. 뷰어는 아무것도 지우지 않는다.
- 제약으로 측정할 것이 없는 룰은 측정 배열을 잡지 않고 바로 `CD skipped`로 건너뛴다(전에는 실행마다
  배열을 잡고 `CD measurement 0 / N`을 찍은 뒤 건너뛰었다).

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
- **사유**(0.12.463): 리뷰어의 `.<db>.reasons.<ID>`(DRC.ko.md §2)가 있으면 `reasons`에 사유별로
  낸다. 종류(`waive`, `not_required`)와 문구가 같은 것을 하나로 모으고, 그 사유를 갖고 **지금
  waive(검토 제외)인** 에러를 상태 바이트와 플래그에서 같은 패스에 센다. 그런 에러가 없는 사유는
  내지 않는다. `waived_with_reason`·`not_required_with_reason`은 그 합이라서 `waived`에서 빼면
  사유 없이 waive된 수다. 임시 폴더의 사유 파일도 본다(`reasons_location: temp`). 다른 결과의
  사유 파일은 `skipped`에 적는다.
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
     "reasons_file": "/r/.results.db.reasons.kim", "reasons_location": "results",
     "reasons_modified": "2026-11-13T07:01:08Z",
     "waived_with_reason": 10, "not_required_with_reason": 0,
     "reasons": [{"kind": "waive", "text": "dummy metal fill", "errors": 10, "actions": 1,
                  "first": "2026-11-13T07:01:08Z", "last": "2026-11-13T07:01:08Z",
                  "rules": [{"index": 1, "name": "A.WIDTH", "errors": 10}]}],
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
| `reasons[]` (0.12.463) | 사유별 한 항목: `kind`(`waive`·`not_required`), `text`, `errors`(그 사유를 갖고 지금 waive·검토 제외인 에러 수), `actions`(그 사유를 준 작업 수), `first`·`last`(처음·마지막으로 준 시각, UTC), `rules[]`(룰별 `errors`). waive 사유가 먼저, 에러가 많은 순. 사유 파일이 없는 리뷰어는 키가 없다 |
| `waived_with_reason`, `not_required_with_reason` | 사유가 있는 waive·검토 제외 에러 수 |
| `reasons_file`, `reasons_location`, `reasons_modified` | 사유 파일, 그 자리(`results`·`temp`), 마지막 수정 시각 |
| `confirmed` (0.12.466) | 리뷰어 줄에서는 그 리뷰어가 confirm해 둔 에러 수다(confirm 파일의 수라서 지금 아무도 waive하지 않는 에러도 들어 있다). confirm 파일이 없는 리뷰어는 키가 없다. `team`과 `team.rules[]`에서는 누군가 waive했고 확인자가 confirm한 에러 수다 |
| `unconfirmed` | `team`과 `team.rules[]`: 누군가 waive했고 아직 confirm되지 않은 에러 수. conflict인 waive도 센다(`waived`는 conflict를 뺀 수다) |
| `waived_confirmed` | 리뷰어 줄(waive 파일이 있을 때): 그 리뷰어의 waive 중 확인자가 confirm한 수 |
| `confirm_file`, `confirm_location`, `confirm_modified` | confirm 파일(`.<db>.confirm.<리뷰어>`), 그 자리(`results`·`temp`), 마지막 수정 시각 |
| `rejected` | `team`과 `team.rules[]`: 확인자가 waive를 반려한 에러 수(0.12.468). 이 에러들의 waive는 `waived`·`confirmed`·`unconfirmed`·`conflicts` 어디에도 세지 않는다. 리뷰어 줄(반려 기록이 있을 때): 그 확인자가 반려한 에러 수 |
| `waived_rejected` | 리뷰어 줄(waive 파일이 있을 때): 그 리뷰어의 파일에 아직 남아 있는 반려된 waive 수. 그 리뷰어의 뷰어가 되돌리면 0이 된다. 리뷰어 줄의 `waived`는 파일의 수라서 이만큼을 포함한다 |
| `rejects_file`, `rejects_location`, `rejects_modified` | 반려 기록(`.<db>.rejects.<리뷰어>`), 그 자리(`results`·`temp`), 마지막 수정 시각 |
| `team.rules` | 모든 룰. `team.reviewers`는 센 리뷰어 수 |

키는 사전순으로 나온다. ProperTee처럼 캡처 상한이 있는 호출자는 파일로 받아 `json_parse`한다(§2).
같은 ID로 두 세션을 열 수 없게 된 것(세션 잠금)과 ID 요구 스위치는 DRC.ko.md §2에 있다.

## 2c. 실시간 수치: `floe2 drc <db> --review-counts` (0.12.464, 팀 합계 0.12.465)

리뷰어별 에러 수·waived·방문·검토 제외와 **팀 합계**(전원의 리뷰를 합친 수치)를 **바로** 답한다(사용자
2026-11-13: "이 통계는 실시간 요청으로 들어올 거라서 사용자가 waive 할때마다 통계를 만들어두는게 좋을 것
같아", ".db를 여러 사람이 검토하고 그 합집합을 실시간으로 받아야 해"). 만들어 둔 것을 읽는다: waive할
때마다 waive 사이드카의 룰별 카운터가, 방문과 검토 제외를 할 때마다 방문 기록(`.sqlite3`)의 룰별
카운터가 그 작업과 함께 갱신된다(SPEC-FORMATS). 리뷰어별 수치는 그 카운터만 읽고 에러는 하나도 읽지
않으므로 걸리는 시간이 에러 수와 무관하다. pack도 룰 목록(이름, 에러 수)과 식별값만 읽는다. 팀 합계는
pack 옆의 `.<db>.team`에 두고, 리뷰어들이 바꾼 구간만 다시 세어 고친다(아래 '팀 합계').

```bash
floe2 drc results.db --review-counts                        # 전체 룰 합계
floe2 drc results.db --review-counts --rule M1.S.1          # 그 룰의 수치도
floe2 drc results.db --review-counts --all-rules            # 모든 룰의 수치도
floe2 drc results.db --review-counts --floe-reviewer kim    # 이 리뷰어만
```

- **합계**(옵션 없이): `reviewers[]`의 각 항목이 그 리뷰어의 `errors`(결과의 전체 에러 수), `waived`,
  `visited`, `not_required`다. `visited_not_required`·`pending`도 §2b와 같은 뜻으로 함께 낸다.
  confirm 파일이 있는 리뷰어(확인자)는 `confirmed`도 낸다(0.12.466. 그 파일의 룰별 카운터다).
- **한 룰**(`--rule NAME`): 최상위 `rules[]`에 그 룰의 `index`·`name`·`errors`가, 각 리뷰어의
  `rules[]`에 그 룰의 `errors`·`waived`·`visited`·`not_required`가 붙는다. 아무도 검토하지 않은
  결과라도 최상위 `rules[]`는 나오므로 에러 수는 항상 답한다. 같은 이름의 룰이 둘이면 둘 다 나온다
  (`index`로 구별한다). 결과에 없는 이름은 `no such rule`로 1이다.
- **모든 룰**(`--all-rules`): 손대지 않은 룰도 한 줄씩 나온다(§2b는 작업이 있는 룰만 낸다).
- **리뷰어**: §2b처럼 작업 파일로 찾는다. `--floe-reviewer ID[,ID…]`면 그들만이고 파일이 없는 ID는
  `present: false`다. 아무도 검토하지 않았으면 `reviewers`가 빈 목록이다.
- **바로 반영**: 뷰어가 그 리뷰를 열고 있는 동안에도 읽는다. waive·방문·검토 제외 하나하나가 그 작업의
  응답 전에 파일에 있으므로 다음 호출의 답에 들어 있다.
- **§2b와의 차이**: 리뷰어별 수치는 세지 않고 저장된 카운터를 읽는다(§2b는 상태 바이트를 다시 세고
  `counters_match`로 알린다. 0.12.440부터 한 리뷰어의 세션은 하나라서 카운터는 상태와 같다). 사유별
  수는 §2b에만 있다. 리뷰어별 수치를 더한 값은 팀 합계가 아니다: 같은 에러를 두 사람이 waive하면
  합집합에서는 하나다. 합집합은 `team`이 낸다.
- **pack**: §2b와 같은 규칙이다(없거나 `.db`가 바뀌었으면 1, 만드는 중이면 75). 잠금에 리더로
  등록하지 않고 게시된 pack을 읽으며, 읽는 동안 파일이 바뀌면 1로 끝난다. 룰 수를 넘는 카운터처럼
  읽을 수 없는 파일은 §2b처럼 `skipped`에 이유와 함께 적는다.

**팀 합계 `team`** (0.12.465)

- **수치**: `errors`·`visited`·`waived`·`not_required`·`conflicts`·`pending`·`reviewers`. §2b의 `team`과
  같은 뜻이고 같은 값이다. 방문은 합집합이고, 두 사람이 waive한 에러는 하나다. 같은 에러를 방문한 두
  사람의 판단(waive, 검토 제외)이 다르면 conflict이고 그 에러는 팀의 waive·검토 제외에 넣지 않는다.
  `pending`은 아무도 방문하지 않았고 검토 제외도 아닌 에러다. `--rule`·`--all-rules`를 주면
  `team.rules[]`에 그 룰들의 같은 수치가 붙는다.
- **confirm**(0.12.466): `confirmed`는 누군가 waive했고 확인자가 confirm한 에러 수, `unconfirmed`는
  누군가 waive했고 아직 confirm되지 않은 에러 수다. 둘의 합이 팀에서 waive된 에러 전부이고,
  conflict인 waive도 여기에 든다(`waived`는 conflict를 뺀 수라서 둘의 합이 `waived`보다 클 수
  있다). confirm은 뷰어의 DRC 메뉴 `team waives…`에서 확인자(`floe2 view --confirmer`)가 하고
  (DRC.ko.md §2), confirm 하나는 구간 하나만 다시 세게 한다.
- **반려**(0.12.468): `rejected`는 확인자가 waive를 반려한 에러 수다. 반려된 에러의 waive는
  리뷰어의 파일에 아직 남아 있어도 팀 수치에 들지 않는다: 확인자가 반려하는 순간 `waived`·
  `confirmed`·`unconfirmed`가 줄고 `rejected`가 는다. 리뷰어 줄의 `waived`는 그 리뷰어 파일의
  카운터라서 그 리뷰어의 뷰어가 되돌릴 때까지는 반려된 waive를 포함한다(`--review-stats`의
  `waived_rejected`가 그 수다). 확인자 줄의 `rejected`는 반려 기록을 읽어서 낸다(작업 한 번에 한 줄인
  작은 파일이다).
- **누구의 합계인가**: 언제나 리뷰어 전원이다. `--floe-reviewer`는 `reviewers[]`의 줄만 줄인다(§2b는
  지정한 리뷰어들만의 합계를 낸다. 일부의 합집합이 필요하면 §2b를 쓴다).
- **바로 답하는 방법**: 합집합은 에러 단위로 봐야 해서 카운터를 더해서는 나오지 않는다. 그래서 만들어
  둔다.
  1. 리뷰 세션은 자기 리뷰를 바꿀 때마다 어느 룰의 어느 구간(에러 4,096개)이 바뀌었는지를 자기 변경
     기록 `.<db>.changes.<리뷰어>`에 덧붙인다. 자기 파일에만 쓰고, 클릭 하나에 48바이트다.
  2. 팀 합계는 `.<db>.team`에 룰별·구간별로 들어 있다. 이 명령은 답하기 전에 각 리뷰어의 기록에서
     지난번 이후의 줄만 읽고, 거기 적힌 구간만 전원의 파일로 다시 세어 합계를 고친 뒤 파일을 통째로
     바꿔 쓴다(임시 파일, rename). 바뀐 것이 없으면 그 파일과 리뷰어 파일들의 시각만 읽는다. 쓰지도
     잠그지도 않는다.
  3. 에러를 많이 바꾸는 작업(65,536개 이상의 waive·검토 제외 job, 동료 결정 적용, 이전 run에서
     가져오기, waive 파일 불러오기)은 뷰어가 작업을 마치면서 직접 고쳐 둔다. 그 뒤의 조회는 셀 것이
     없다.
- **`current`**: 수치가 지금의 파일 전부를 반영하면 `true`다. `false`면 `waiting[]`에 이유가 한 줄씩
  있고 수치는 반영된 데까지다. 세 경우가 있다. 긴 작업이 진행 중이라 기록된 데까지만 센 경우(작업은
  약 1초마다 기록한다), 파일은 바뀌었는데 그 기록이 아직 없는 경우(아래 '기록 없이 바뀐 파일'), 다른
  프로세스가 지금 합계를 고치고 있는 경우(3초까지 기다린 뒤 가진 값으로 답한다). 모두 다음 조회에서
  풀린다.
- **기록 없이 바뀐 파일**: 0.12.465 이전 빌드의 뷰어가 쓴 리뷰, 비트를 쓰고 기록하기 전에 끝난 실행,
  복사해 온 폴더가 그렇다. 기록은 답을 싸게 할 뿐이고 맞게 하는 것은 파일의 시각이다: 세션은 기록마다
  그때의 파일 시각을 함께 적고, 이 명령은 리뷰어의 파일이 그 시각과 다르면 기록되지 않은 변경으로
  본다. 변경 기록이 아예 없는 리뷰어는 파일이 바뀔 때마다 그 리뷰어가 손댄 룰을 통째로 다시 센다(바로
  반영된다). 기록이 있는데 맞지 않으면 기록을 `FLOE_TEAM_GRACE_S`초(기본 10) 기다린 뒤 다시 센다.
  그동안은 `current: false`이고 수치는 직전 값이다. `0`이면 기다리지 않는다. 그 리뷰어가 뷰어를 다시
  열면 세션이 먼저 알리므로 바로 반영된다.
- **`kept`·`counted`·`skipped`**: `kept`는 합계를 `.<db>.team`에 적어 두었는지다. 결과 폴더에 쓸 수
  없으면 `false`이고 그 답만을 위해 센다(수치는 같다. 조회할 때마다 그동안의 변경만큼 다시 센다).
  `counted`는 이번 답을 위해 다시 센 구간 수다(바뀐 것이 없으면 0, 클릭 하나면 1). `skipped[]`는
  합계에서 뺀 파일이다(다른 결과의 것, 읽을 수 없는 것. §2b가 빼는 것과 같다).
- **쓰는 것**: 이 명령은 `.<db>.team` 하나를 pack 옆에 둔다(고치는 동안 `.floe-lock/<db>.team.build`를
  잡는다). 리뷰는 건드리지 않는다. `FLOE_TEAM_KEEP=off`면 쓰지 않고 답할 때마다 센다(`kept: false`).
  같은 변수를 뷰어 쪽에 두면 세션이 변경 기록을 쓰지 않고(있던 기록은 지운다) 작업 뒤에 합계를
  고치지 않는다. 어느 쪽이든 수치는 같다.
- **한계**: 한 리뷰어의 룰별 waive 카운터가 0이면 그 룰의 상태 바이트는 읽지 않는다(카운터는 상태와
  함께 쓰인다. 어긋난 파일은 §2b가 `counters_match`로 알린다). 변경 기록이 없는 리뷰어의 파일이 시각의
  같은 눈금 안에서 두 번 바뀌면 두 번째는 다음 변경 때 반영된다. NFS에서 다른 호스트가 조회하는
  경우는 설계로만 대비했고 게이트는 로컬 파일 시스템에서 돈다: 기록이 비트보다 먼저 보이면 받아들이지
  않고 기다리며, 새 리뷰어의 첫 파일은 마운트의 디렉터리 캐시 시간만큼 늦게 보일 수 있다.
- **비용**(에러 2,030만·룰 2,000개·리뷰어 3명, 캐시가 데워진 상태에서 호출 한 번, 0.12.465): 바뀐 것이
  없을 때 17.7 ms(팀 합계가 없던 0.12.464 바이너리는 같은 자리에서 17.1 ms), `--all-rules` 31 ms(2 MB),
  클릭이나 방문 하나 뒤 24~25 ms(구간 1개), 1,000만 개를 waive한 job 뒤 23 ms(뷰어가 job이 끝나고
  66 ms 안에 고쳐 두므로 센 구간 0), 뷰어가 고쳐 두지 않는 6만 개 job(룰 전체에 흩어짐) 뒤 69 ms
  (4,883구간), `.team`이 없을 때의 첫 답 75 ms, `--review-stats` 560 ms. `.team`은 168 KB, 변경 기록은
  리뷰어마다 2.5 KB였다(0.12.464 때 다른 날 잰 값은 12 ms·20 ms·433 ms다). `--review-counts`는 에러
  수에 따라 늘지 않고 `--review-stats`는 에러 수와 리뷰어 수에 비례한다.

```json
{
  "type": "floe-drc-review-counts", "version": 1,
  "generated": "2026-11-13T09:42:26Z",
  "database": "/r/results.db", "pack": "/r/.results.db.tray",
  "fingerprint": "471a…", "cell": "TOP", "errors": 20299635, "rule_count": 2000,
  "rules": [{"index": 2, "name": "M2.OVERLAP.2", "errors": 20000000}],
  "reviewers": [
    {"id": "kim", "present": true, "errors": 20299635,
     "waived": 13333344, "visited": 40, "not_required": 250000,
     "visited_not_required": 40, "pending": 20049635,
     "waive_known": true, "visited_known": true, "not_required_known": true,
     "waive_file": "/r/.results.db.waive.kim", "waive_location": "results",
     "waive_modified": "2026-11-13T09:41:58Z",
     "review_file": "/r/.results.db.review.kim.sqlite3", "review_location": "results",
     "review_modified": "2026-11-13T09:41:50Z",
     "rules": [{"index": 2, "name": "M2.OVERLAP.2", "errors": 20000000, "waived": 13333338,
                "visited": 40, "not_required": 250000, "visited_not_required": 40,
                "pending": 19750000}]}
  ],
  "team": {"errors": 20299635, "visited": 41, "waived": 15455919, "not_required": 249966,
           "conflicts": 34, "pending": 20049634, "confirmed": 0, "unconfirmed": 15455953,
           "reviewers": 3, "current": true, "kept": true, "counted": 0,
           "rules": [{"index": 2, "name": "M2.OVERLAP.2", "errors": 20000000, "visited": 41,
                      "waived": 15455913, "not_required": 249966, "conflicts": 34,
                      "pending": 19749999, "confirmed": 0, "unconfirmed": 15455947}]},
  "skipped": []
}
```

`rules`(최상위, 리뷰어, `team`의)는 `--rule`·`--all-rules`를 줄 때만 있다. `rule_count`는 결과의 룰 수다.
`team.waiting`과 `team.skipped`는 있을 때만 나온다. 그 밖의 키는 §2b와 같은 뜻이다. 위 예는
`--floe-reviewer kim --rule M2.OVERLAP.2`의 답이다: 리뷰어 줄은 하나이고 팀은 세 사람의 것이다.
confirm은 아직 없어서 `unconfirmed`가 waive된 에러 전부다(conflict 34개의 waive를 더한 수).

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
| `--detail`, `--density`, `--thin`, `--occupancy`, `--label-font-px`, `--layer-colors`, `--layerprops`, `--legend`, `--archival`, `--no-index`, `--temp-index`, `--light`/`--full`, `--wait` | 일반 캡처와 같음 | [CAPTURE-CLI.ko.md](CAPTURE-CLI.ko.md) §1.3·§1.5. 영역(`--bbox`/`--at`/mosaic)·`--batch`·`--report`·`--chipid`/`--chipnumber`와는 함께 쓸 수 없다 |
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
- **`--layerprops FILE`**(0.12.452): 이 실행의 레이어 속성 파일. 일반 캡처와 같이 디자인 기본값 위에 덮는다
  ([CAPTURE-CLI.ko.md](CAPTURE-CLI.ko.md) §1.3). 색·채움·선폭이 스냅샷과 범례에 쓰인다. 레이어 격리가 고른 레이어와
  `--layers`로 준 레이어는 파일이 숨긴 것이라도 그린다. 격리할 레이어를 모르는 룰(all layers)은 파일이 숨긴 레이어를
  빼고 그린다.
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
  스냅샷은 db 옆과 덱 옆에서 못 찾으면 DIR의 같은 자리에서 찾는다(`drc-prepare --svrf`로 적어 둔 사이드카가
  있으면 그것을 맨 먼저 본다, 0.12.460). `-o FILE`은 준 자리에 그대로 쓴다.
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
