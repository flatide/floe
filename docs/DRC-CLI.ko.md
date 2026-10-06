# DRC CLI 스크립팅 레퍼런스

셸 스크립트/자동화에서 쓰는 DRC 명령 모음. 뷰어 없이 룰 목록·에러
목록을 뽑고 에러 스냅샷 PNG를 만드는 표면이다. (뷰어 연동·포맷
내부는 [DRC.ko.md](DRC.ko.md) 참고.)

실행 형태는 환경에 따라:

```bash
.venv/bin/python -m floe <cmd> ...     # 소스 체크아웃
floe <cmd> ... / floe-index drc|svrf ...    # floe-portable 번들
```

**공통 규약**

- stdout = 데이터(JSON/TSV)만. 진행·경고(`[floe] ...`)는 전부
  stderr — 스크립트는 stdout만 파싱하면 된다.
- 실패 = 비-0 exit + stderr `floe: ...` 한 줄.
- 에러 번호는 두 종류: **local** = 룰 안 1-based 순번(뷰어
  그리드와 동일), **global** = 파일 순서 전역 번호(Calibre RVE
  스타일, 뷰어 상세의 `#local(global)`).
- status: `0` = not waived, `1` = waived. waive 변경은 뷰어
  전용(우클릭 메뉴 / `w` 키) — CLI 쓰기 표면은 없고, 재-pack 시
  초기화된다.

---

## 0. 준비: pack 인덱스

```bash
floe-index drc results.db [out.tray] [--jobs N]
```

`.results.db.tray`(v2 pack, db 옆 숨김 파일; 2026-09-16까지의 `results.db.ice`는
발견 시 자동 개명)를 만든다. 이후 모든 명령이 신선한
pack을 자동 사용한다(소스 size/mtime 대조). pack이 없거나 낡으면
**ASCII 전체 파스로 폴백**(stderr 안내) — 수백 MB급부터 매우
느리므로 스크립트는 pack을 먼저 만들 것. `--pack`은 호환용
no-op. 좌표 토큰이 정수가 아닌 db는 명확한 메시지와 함께 거부.

### 대규모 룰의 공간·CD/델타 사전 준비

```bash
floe-index drc-prepare results.db --svrf deck.svrf.rules.json
# Rust CD 측정을 사용하고 한 룰 안의 좌표 블록을 작업자 4개로 병렬 처리
floe-index drc-prepare results.db --svrf deck.svrf.rules.json --backend rust --jobs 4
# 특정 룰만 준비 (--rule 반복 가능)
floe-index drc-prepare results.db --svrf deck.svrf.rules.json --rule M1.SPACE
# SVRF 없이 공간 인덱스만 준비
floe-index drc-prepare results.db --spatial-only
```

기존 `.tray`를 읽고 룰별 쿼드트리, 측정 CD, 절대/비율 델타 기본 그룹을
`.results.db.analysis/`에 저장한다. pack이나 개인 waive/notes는 변경하지
않는다. pack이 없거나 낡으면 중단하며 ASCII 전체 로딩으로 우회하지 않는다.
`--delta-only`로 CD/델타만 준비할 수 있다. 진행률은 stderr, 취소는 Ctrl+C이며
완료된 캐시는 재실행 때 사용한다. 진행 중인 룰은 다시 준비한다.
실제 에러가 0개인 룰은 전처리·캐시 생성·자식 프로세스 실행을 건너뛰고,
완료 메시지에서 준비한 룰 수와 건너뛴 룰 수를 따로 표시한다.

`floe-index drc-prepare`는 Rust CD 엔진과 기존 Python/NumPy 캐시 관리 계층을
함께 실행한다. Portable에서는 `runtime/bin/floe-index` 옆의 Python을 자동으로
사용하며 별도 활성화가 필요 없다. 개발 환경에서는 활성 가상환경 또는 소스의
`.venv`를 찾고, 특정 런타임은 `FLOE_PYTHON_BIN=/path/to/python`으로 지정할 수 있다.
바이너리만 단독 복사한 환경에는 `floe` Python 패키지와 NumPy도 필요하다.
내부 CD 작업에는 지금 실행한 `floe-index`를 사용한다. 기존
`python -m floe2 drc-prepare ...` 진입점과 옵션·캐시는 호환된다.

`--backend auto`가 기본이며 호환되는 `floe-index`가 있으면 Rust로 CD를 측정한다.
구형/미설치 바이너리에서는 Python으로 처리한다고 진행 메시지에 표시한다.
`--backend rust`는 Rust를 사용할 수 없으면 중단하고, `--backend python`은
기존 측정 경로를 지정한다. Rust 바이너리는 `cd rust && cargo build --release -p floe-index`로
빌드하거나 최신 portable 번들의 바이너리를 사용한다.
`--jobs N`은 Rust의 룰 내부 병렬 작업자 수(1~256), 기본은 CPU 수와 4 중 작은 값이다.
환경변수 `FLOE_DRC_JOBS=N`으로 뷰어의 대규모 CD 측정에도 같은 수를 적용할 수 있다.

Rust는 기존 `.tray`의 64개 에러 단위 블록들을 묶어 독립적으로 복호화·측정하고,
원래 에러 순서대로 동일한 측정 캐시를 기록한다. 원본 DB 재파싱이나 pack 재생성은
필요하지 않다. 수치 반올림/비교 경계는 해당 블록만 별도 Python 프로세스에서
재확인하여 기존 CD 선택·추정 표시·소수점 다섯 자리 결과를 보존한다.
공간 인덱스와 델타 그룹은 기존 NumPy 경로로 생성하며, CLI는 그룹의 영구 캐시만
준비한다. 뷰어용 리뷰 상태 복사본과 페이지 인덱스를 CLI에서 임시 생성하지 않는다.

읽기 전용/네트워크 결과 폴더라면 `FLOE_DRC_ANALYSIS_ROOT=/local/ssd/drc-cache`로
별도 저장소를 지정한다. 뷰어에도 같은 환경변수를 지정해야 같은 캐시를 사용한다.
전체 14억 건의 측정표와 두 모드 그룹은 약 40.6GB(공간 인덱스·임시 정렬 파일
제외)이므로 필요한 룰부터 준비할 수 있다. `--svrf`에는 원문 SVRF가 아니라
`floe-index svrf`로 생성한 `.rules.json`을 지정한다.

동일한 합성 사각형 데이터로 Python 측정과 Rust 1/4 작업자를 비교하려면:

```bash
python tools/bench_drc_prepare.py --counts 10000 1000000 --jobs 1 4
```

실행마다 별도 임시 분석 캐시를 사용하며, 각 단계 시간과 최초/재사용 시간을
JSON으로 출력한다. 결과값 검증과 DB 생성 시간은 처리 시간에서 제외한다.

2026-10-06 macOS arm64(논리 CPU 8개), Python 3.14.6에서 합성 사각형과
단일 width 조건으로 측정한 최초 전처리 시간이다. 프로그램 시작·공간 인덱스·
CD 측정·두 델타 그룹을 포함하며, 분석 캐시는 없고 OS 파일 캐시는 유지했다.

| 룰의 에러 수 | Python CD 백엔드 | Rust CD 작업자 4개 |
| ---: | ---: | ---: |
| 1만 | 0.74초 | 0.43초 |
| 100만 | 24.09초 | 1.05초 |
| 1000만 | 미측정 | 7.18초 |

1000만 개의 Rust CD 단계는 1개 작업자 1.61초, 4개 작업자 1.00초였다.
4개 작업자의 나머지 주요 단계는 공간 인덱스 2.92초, 두 그룹 합계 3.06초였다.
모든 측정값·조건 번호·추정 표시를 검증했으며 이 벤치의 정밀도 보정 대상은 0개였다.
실제 형상, 조건 수, 수치 경계 보정 비율과 저장장치에 따라 처리 시간은 달라진다.

## 1. 룰 목록: `floe drc <db> --rules`

```bash
floe drc results.db --rules > rules.json
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

## 2. 한 룰의 에러 목록: `floe drc <db> --errs RULE`

```bash
floe drc results.db --errs M1.SPACE.1 > errs.json
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

## 3. 에러 스냅샷 PNG: `floe render --drc`

```bash
floe render chip.oas --drc results.db --drc-rule M1.SPACE.1 \
    --drc-err 1-50 --px 800 --out snap.png > made.tsv
```

| 옵션 | 기본 | 의미 |
|---|---|---|
| `--drc FILE.db` | — | 결과 db (`--drc-rule`과 항상 함께) |
| `--drc-rule NAME` | — | 룰 이름 (`--rules`에서 얻은 것) |
| `--drc-err N\|A-B\|all` | all | local 번호(1-based). 명시 N/A-B는 **전량** 렌더 |
| `--drc-cap N` | 200 | `all`일 때만 적용되는 상한 (초과분 stderr 경고) |
| `--drc-frac F` | 0.3 | 프레임 대비 에러 스팬 비율(뷰어 점프 프레이밍 동일) |
| `--px N` | 1200 | 정사각 한 변 px |
| `--out PATH` | 룰명.png | 출력 스템: 1장 = 그대로, 여러 장 = `stem_<local>.png` |
| `--depth N` | full | 계층 깊이 (기본 = 전체 전개, 뷰어 "99") |
| `--layers ...` | 자동 | 명시하면 svrf 격리보다 우선 |
| `--drc-rules RULES.json` | 자동 탐색 | svrf 사이드카 직접 지정 |
| `--floe-reviewer NAME` | DISPLAY/SSH 유도 | waive 자동 저장의 리뷰어 태그 (`floe drc`/`view`에도 있음; FLOE_REVIEWER env 동치) |

- **픽셀 = 뷰어와 동일**: 뷰어의 렌더 서비스 경로(상주 vfsd
  세션 + detail medium 컷/헤어라인/LOD/커버리지) 그대로, depth만
  full. 첫 에러가 콜드 스타트(수 초, 타임아웃 300s)를 내고 이후
  에러당 ms급.
- **레이어 격리**: rules.json 사이드카가 발견되면(탐색 순서 =
  db 옆 덱 basename → 기록된 덱 경로 → `<db>.rules.json`, 또는
  `--drc-rules`) 그 룰의 원천 GDS 레이어만 켠다 — 뷰어 더블클릭
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
  스크립트에서 주석만 뽑으려면 `python fe_embed.py --dump x.png`.
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
floe drc "$DB" --rules > rules.json                # 2. 룰 목록

# 3. 에러 있는 룰마다 앞 20개 스냅샷
for RULE in $(python3 -c "
import json
for r in json.load(open('rules.json')):
    if r['errors']: print(r['name'])"); do
  floe drc "$DB" --errs "$RULE" > "errs_$RULE.json"
  floe render "$SRC" --drc "$DB" --drc-rule "$RULE" \
      --drc-err 1-20 --px 800 --out "snap/$RULE.png" \
      >> made.tsv
done
```

ProperTee에서는 각 단계를 파일 리다이렉션으로 받고
`json_parse`/TSV 파싱으로 읽는다(§2 캡처 상한).
