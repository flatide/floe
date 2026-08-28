# DRC CLI 스크립팅 레퍼런스

셸 스크립트/자동화에서 쓰는 DRC 명령 모음. 뷰어 없이 룰 목록·에러
목록을 뽑고 에러 스냅샷 PNG를 만드는 표면이다. (뷰어 연동·포맷
내부는 [DRC.ko.md](DRC.ko.md) 참고.)

실행 형태는 환경에 따라:

```bash
.venv/bin/python -m floe <cmd> ...     # 소스 체크아웃
floe <cmd> ... / floe-index drc ...    # floe-portable 번들
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
floe-index drc results.db [out.ice] [--jobs N]
```

`results.db.ice`(v2 pack)를 만든다. 이후 모든 명령이 신선한
pack을 자동 사용한다(소스 size/mtime 대조). pack이 없거나 낡으면
**ASCII 전체 파스로 폴백**(stderr 안내) — 수백 MB급부터 매우
느리므로 스크립트는 pack을 먼저 만들 것. `--pack`은 호환용
no-op. 좌표 토큰이 정수가 아닌 db는 명확한 메시지와 함께 거부.

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

## 4. SVRF 사이드카 생성: `floe svrf`

스냅샷 레이어 격리와 뷰어 디테일(제약/원천 레이어)을 살리려면
룰덱에서 rules.json을 한 번 만들어 둔다:

```bash
floe svrf deck.cal --scan                  # 새 덱 인벤토리(파스 없이 확인)
source sourceme.sfa14_ALL && \
floe svrf deck.cal --follow-verbatim       # 실런과 같은 환경으로 생성
```

- 출력 기본 `<deck>.rules.json` (`-o`로 변경). db 옆에 두면 자동
  탐색된다.
- `-D NAME[=V]` 반복 지정 = 실런 스위치 재현(-D가 환경변수보다
  우선). 환경변수 폴백이 기본이므로 sourceme를 source 한 셸에서
  돌리면 -D 나열이 거의 필요 없다(`--no-env-switches`로 끔).
- `-I DIR` = INCLUDE 탐색 경로 추가, `--follow-verbatim` =
  VERBATIM/Tcl 블록 안 INCLUDE도 추적.

## 5. 엔드-투-엔드 예시

```bash
set -e
DB=results.db; SRC=chip.oas; DECK=deck.cal
mkdir -p snap

floe-index drc "$DB" --jobs 8                      # 0. pack
floe svrf "$DECK" -o "$DB.rules.json"              # 1. 사이드카(1회)
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
