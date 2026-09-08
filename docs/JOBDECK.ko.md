# Jobdeck (Calibre MDPView `.jb`) — floe2 구현 노트

2026-09-08 착수. Calibre MDPView가 여는 MEBES jobdeck을 floe2에서 열기 위한
설계·진행 상태·미결 사항을 한곳에 모은다. 포맷은 비공개이므로 **실측으로
확정한 것**과 **아직 확인 못 한 것**을 구분해 적는다. 출처는 사용자가
KLayout으로 만든 역분석 툴(`jobdecks_klayout/`, 로컬 전용·gitignore)이며,
그 툴의 문서(jobdeck-format, coordinate-model, load-plan, cli-spec)와
손계산 기대값(`expected.json`)이 이 문서와 gate의 근거다.

> 실제 jobdeck과 마스크 데이터는 저장소에 절대 넣지 않는다. gate의 fixture는
> KLayout(개발 전용)으로 임시 디렉터리에 생성한다.

## 1. 포맷 (확정 / 미확정)

```
SLICE 1,17
RETICLE
* <deckname>.jb
OPTION PA, AA=0.0200, BA=0.002000, SA=80
MTITLE 1,<name>
*PLACE-INFO
CHIP ID001, * MAIN 1.0000
$ (1, NAME, AD=0.00020, SF=1, TC=file.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0)
ROWS 85120.0/45020.0
*END-PLACE
END
```

확정(실덱 관찰):
- `CHIP <id>[, 자유 텍스트]`: id는 첫 콤마/공백 앞 텍스트(콤마 제거), 나머지는
  `Chip.tail`에 보존만 한다. id는 덱 안에서 유일(중복은 warning).
- `$ ( idx, name, AD, SF, TC, LY={..}, DT={..}, BX,BY,UX,UY )`: 한 줄에 닫혀야
  한다. 줄바꿈된 항목은 이어 붙이지 않고 **구조 오류로 거부**한다(이어
  붙이는 규칙을 모르며, BX..UY가 0으로 남으면 배치가 틀어진다).
- `ROWS y/x` — **Y가 먼저**. CHIP 블록 소속이며 블록의 모든 `$`가 공유.
- 같은 identifier라도 CHIP마다 AD/SF/TC/bbox가 다를 수 있다 → 항목은 독립
  저장, `variation_report()`로 노출.
- 배치식: `ratio = AD / source_dbu`, `mag = SF·ratio`,
  `dx = jx − cx·ratio`, `dy = jy − cy·ratio`, `cx = (BX+UX)/2`.
- MTITLE 없는 identifier, 어떤 CHIP에 없는 identifier는 정상(coverage로 보고).
- 알 수 없는 `$` 키(예 `ZZ=7`), 헤더 항목, 파싱 안 된 줄은 보존·보고만 한다.
  SLICE/RETICLE/OPTION은 배치에 쓰이지 않는다.

미확정(코드에 플래그로 남김):
- `LY={a,b}` × `DT={c,d}`가 cross인지 zip인지 (`--ly-dt`, 기본 cross).
- 회전/미러 필드의 존재 여부 (실덱에서 본 적 없음).
- OPTION의 AA/BA/SA 의미.

## 2. 색 규칙 (MDPView 실측 2026-09-07)

열 가지 색이 순환한다(Tk 색 이름 그대로: blue, yellow, red, pink, orange,
white, purple, cyan, magenta, green). 대상의 **순서 목록**을 만들고 i번째에
`palette[i % 10]`을 준다.
- identifier 모드: identifier 오름차순. 선택(`--id`)이 색을 바꾸지 않는다.
- layer 모드: LY 오름차순, LY/DT 핀 가능.
- chip 모드: CHIP은 덱 순서, 선택된 identifier k는 k−1개의 CHIP 뒤에 끼워
  넣는다(`--id 2` → CHIP1, $2, CHIP2, …). 따라서 chip 모드의 CHIP 색은 선택에
  따라 움직인다. 여러 identifier 동시 선택은 추정(MDPView는 하나씩 펼침).

## 3. floe2 구현 방향 (결정)

KLayout 툴은 "소스 → 하나의 flat-ish layout 재작성" 구조였다. floe2에서는
**런타임 합성**으로 간다:
- 각 TC 소스는 기존 `<src>.floe` 캐시 그대로 사용(Calibre의 `.fvi` 유사).
  재작성·복사 없음. 소스가 1500개여도 인덱스는 한 번씩만.
- 배율(mag)·오프셋은 **씬 루트의 배치(placement)에서만** 적용한다. floe2의
  Rust 스택은 OASIS 배율 PLACEMENT(record 18)를 거부하고 `Xf`는 정수
  행렬이므로, 캐시 내부는 손대지 않고 renderd가 "캐시 N개 + 루트 배치"를
  받아 그린다. 좌표는 i64라 KLayout 툴의 int32 dbu 굵히기 문제가 사라진다
  (`choose_dbu`의 doublings는 잔차 통계용으로만 남긴다).
- 페인터 순서는 덱 레이어 테이블(`(idx, ly, dt)` 또는 chip 모드 `(chip, idx,
  ly, dt)`) 순.

### 단계
| 단계 | 내용 | 상태 |
|---|---|---|
| M1 | Python 포팅(`floe/jobdeck/`), `floe2 jobdeck` CLI, 일괄 인덱싱, 손계산 gate | ✅ 2026-09-08 (main) |
| M2 | renderd 다중 캐시 씬 + 루트 배율/오프셋, 합성 프레임 | 예정, `feature/jobdeck` |
| M3 | GUI: 덱 열기, identifier/chip/layer 색 모드, 선택 | 예정 |
| M4 | headless shot/mosaic (KLayout 툴 cli-spec 대응) | 예정 |
| M5 | KLayout 툴을 oracle로: 같은 뷰포트 byte/픽셀 비교 gate | 예정 |

M2 전에 확인할 것: 실덱의 `mag` 분포(1이 아닌 값이 흔한지), 소스 자체에
배율 배치가 들어 있는지(있으면 인덱스 단계에서 거부되므로 정책 필요).

## 4. M1 상세

패키지 `floe/jobdeck/` (KLayout·렌더러 의존 없음):
- `parser.py` — `parse_jobdeck(path, strict=True)` → `JobDeck`(chips, header,
  options, mtitles, comments, unknown, errors, warnings). `report()`,
  `coverage()`, `title_check()`, `variation_report()`, `extras()`.
- `geom.py` — `choose_dbu`, `to_grid`, `layer_table`, `plan(...)` →
  `(placements, stats)`; skip ledger(`missing / unreadable / unknown_format /
  empty_layer / not_indexed`)를 선택 안(`skipped`)/밖(`deck_issues`)으로 분리.
- `color.py` — `JOBDECK_PALETTE`, `chip_order`, `color_order`, `ColorScheme`
  (`build`, `order_table`, `color_of`, JSON save/load). `.lyp` writer는
  KLayout 전용이라 이식하지 않음.
- `sources.py` — OASIS START / GDS UNITS 헤더 probe(gzip 포함, 4KB 1회 읽기),
  `SourceCatalog`(status, dbu, `indexed` = 최신 VFS 캐시 존재).
- `plan.py` — `plan_deck` (파싱→probe→배치→색), 요약 텍스트, JSON 리포트.

CLI:
```
floe2 jobdeck deck.jb [--sources DIR] [--id N,N] [--mode identifier|layer|chip]
                      [--colors FILE] [--ly-dt cross|zip] [--on-missing skip|fail]
                      [--lenient] [--placements] [--report FILE]
                      [--index [--force] [--jobs N]]
```
종료 코드: 0 정상, 1 구조 오류/인자, 2 `--on-missing fail`에서 dbu 없는
소스 또는 인덱싱 실패, 3 선택 안에 배치 못 한 항목이 있음(ledger 출력).
`--index`는 probe가 ok인 소스마다 `floe2 index <src> --jobs N`을 순차
실행하고(각 실행이 내부 병렬), 최신 캐시는 건너뛴다.

Gate `tools/validate_jobdeck.py` (배터리 편입, 20 tests):
- fixture: chipA/chipB(dbu 5e-5), mark(1e-3), chipA.gds, .oas.gz, .gds.gz,
  junk.bin, absent.oas, `test.jb`(3 CHIP, id 1/2/3/5, 17 instance, AD 변동,
  TC/SF 변동, ZZ=7, MTITLE 4 미사용), `test_formats.jb`, `broken.jb`.
- 17개 배치 `mag/dx/dy`를 **손계산** `tools/jobdeck_expected.json`과 1e-9로
  비교(코드가 코드를 검증하지 않도록), 배치 폭, bbox, 레이어 테이블, dbu
  2.5e-5·doubling 0·잔차 0, 선택이 grid를 옮기지 않음, ledger 분리, 네 컨테이너
  (oas/gds/gz)가 같은 수치, 헤더 probe dbu, 색 순서(identifier/chip splice/
  layer 핀), 스킴 JSON 왕복, CLI 종료 코드, `--index` 후 `Cache.exists()` +
  `meta.vfs` + 비-stale.

## 5. 미결·후속
- LY/DT cross vs zip, 회전/미러: 실덱 사례가 나오면 확정.
- 실덱 `mag` 분포 확인 후 M2 설계 확정(배율 1이 대부분이면 정수 `Xf` 경로로
  충분, 아니면 renderd에 실수 배율 배치 도입).
- M4의 mosaic/anchor/unit 규칙은 KLayout 툴 cli-spec을 그대로 따른다.
