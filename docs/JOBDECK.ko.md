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
| M2 | renderd 다중 캐시 합성(`open deck=`), 루트 배율/오프셋, headless 렌더, 오라클 gate | ✅ 2026-09-08 (`feature/jobdeck`) |
| M3 | 뷰어에서 덱 열기(`floe2 view deck.jb`), 색 모드 메뉴, `view/info/index/render`가 .jb를 직접 받음 | ✅ 2026-09-09 (`feature/jobdeck`) |
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

## 5. M2 상세 — 합성 렌더 (renderd)

### 구조
- `rust/render-core/src/deck.rs` — `DeckSpec`(파서), `Deck`(열린 덱),
  `Deck::render`. **배치 하나 = 단일 캐시 렌더 한 번**: 덱 뷰포트를 그 소스의
  dbu로 사상한 뷰 `(v − d) / scale`로 보통의 plan → decode → raster를 돌리되
  배경을 alpha 0으로 칠하고, 결과를 덱 레이어 순서(`order` = out)로
  opaque-over 합성한다. 배율은 이 사상에만 존재하므로 캐시·플랜·라스터는
  정수 그대로다. world→device 사상 `(x − x0)·W·2³² / span`이 배율 전후에 같은
  식이라 픽셀은 평탄화한 단일 레이아웃 렌더와 **바이트 동일**하다(gate 4).
- 컬링: 소스 top cell bbox를 덱 좌표로 변환해 뷰와 교차하지 않는 배치는
  건너뛴다(`passes_skipped`). 가시 레이어(`layers=`)는 out 단위. 플래너가
  소스 전체를 cut 아래로 판정해 top cell을 버린 경우(전체 뷰에서 0.2× 마크
  같은 것; 단일 캐시에서는 top이 칩이라 없던 상황)도 오류가 아니라 빈 패스로
  건너뛴다 — 2026-09-09 뷰어 기본값(depth 0, detail medium = cut 3px)의 첫
  프레임이 이 오류로 비어 있던 것을 gate에 고정(`test_3b`).
- 페이지 예산: 덱 전체에 **하나의 예산**(서버 고정 1024MB). 소스마다 LRU를
  두되, 디코드 직전 `share_budget`: 다른 소스들의 상주량이 예산의 절반을
  넘으면 비례 축소하고, 현재 소스에 나머지를 준다 → 합계 ≤ 예산, 현재 소스
  ≥ 절반 보장.
- 스펙 파일(줄 단위, Python이 씀; 경로/이름은 hex):
  ```
  deck unit=2.5e-05
  source path_hex=<.floe 디렉터리>
  layer out=0 name_hex=<"$1 METAL1"> color=#0000ff fill=solid width=1
  placement source=0 layer=123/43 out=0 scale=8.0 dx=1640800000 dy=3200800000 order=0
  ```
  `scale = mag · source_dbu / deck_dbu`, `dx/dy`는 덱 그리드 정수(M1의 ix/iy).
- renderd 와이어: `open deck=<spec> budget_mb= jobs=` (cache와 배타),
  `style`은 `<out>/0 COLOR FILL WIDTH` 행으로 덱 레이어 색을 바꾼다, `render`는
  좌표가 덱 dbu·`layers=`가 `<out>/0`인 것 외에 동일하며 `frame …` 응답은
  단일 캐시 경로의 모든 필드를 유지(없는 단계는 0)하고 `passes=`,
  `passes_skipped=`를 덧붙인다. `info`는 `deck=1 sources= placements=`.
- Python: `floe/jobdeck/render.py` — `write_deck_spec`(인덱스 없는 소스
  `not_indexed`, 캐시에 없는 LY/DT `empty_layer`는 ledger), `DeckRenderWorker`
  (`RustRenderWorker`의 `_open_command`만 바꿈), `render_deck_png`,
  `fit_bbox_to_pixels`(늘리지 않고 확장). CLI: `floe2 jobdeck deck.jb --render
  out.png --bbox X0,Y0,X1,Y1 [--pixel WxH] [--spec FILE]`.

### M2에서 제외(M3~)
라벨, 계층 프레임, refinement 라운드, pan/margin 재사용(retained frame),
pick/snap/clip(덱에서는 오류 응답), 배치별 회전/미러(포맷 미확정).

### gate (`tools/validate_jobdeck.py` CompositeTests)
1. 스펙: 3 source · 4 layer · 17 placement, `scale=8.0 dx=… dy=…` 손계산값.
2. 합성 프레임 vs **손으로 배치한 박스**: 1024×800 전 픽셀(모서리 1px 제외)의
   색이 painter 순서대로 일치.
3. `layers=` 컬링: $2만 켜면 노랑·검정만 남음.
4. 합성 vs **KLayout 평탄화 단일 레이아웃**을 단일 캐시 경로로 렌더한 프레임:
   바이트 동일. (KLayout은 int32라 오라클 레이아웃은 1e-4 um 그리드로 만든다;
   뷰포트는 소수점 오프셋을 줘서 경계 정합 반올림 차이를 배제.)
5. `--render` PNG 크기, 인덱스 없는 소스의 exit 3 + ledger.

## 6. M3 상세 — 뷰어 (사용자 결정 2026-09-09: 출력용 CLI가 아니라 뷰어에서 봐야 한다)

덱은 **레이아웃과 같은 명령으로** 다룬다. 전용 플래그를 두지 않고 기존 floe2
규칙을 그대로 쓴다:

```sh
floe2 index  deck.jb                 # 덱이 참조하는 모든 TC 소스를 인덱싱(최신 캐시는 유지)
floe2 view   deck.jb                 # 뷰어에서 열기 (단일 인스턴스 포워딩·File > load layout… 동일)
floe2 info   deck.jb                 # 덱 요약 + 뷰 레이어 표
floe2 render deck.jb --bbox X0,Y0,X1,Y1 --px 1200 --out d.png [--layers "$1 METAL1,$3 ALIGN"]
floe2 jobdeck deck.jb [--report r.json] [--spec s.spec]   # 분석·보고만
```

- `floe/jobdeck/viewer.py` `DeckCache` — 뷰어·CLI가 `Cache`에서 읽는 것을 그대로
  제공한다: `src`(덱 경로), `dir`(renderd가 여는 스펙), `meta`(`dbu`, `bbox`,
  `layers`, `src`, `grid`(1×1), `vfs`, `jobdeck{mode, chips, placements,
  skipped, colour_order}`), `exists/load/is_stale/resolve_layers`. "캐시"는
  소스들의 `<src>.floe` 전부이며 `deck_ready()`가 그 존재를 답한다.
  `service.make_render_worker`는 `is_jobdeck`을 보고 `DeckRenderWorker`를 만든다.
- **뷰 레이어 = 색 대상**(`render.view_layers`): identifier 모드는 identifier당
  한 줄(`$1 METAL1`), layer 모드는 (LY,DT)당 한 줄(`LY123.DT43`), chip 모드는
  CHIP당 한 줄(`CHIP ID001`). 레이어 패널의 토글·색 변경·layerprops 저장이 그
  단위로 동작하고, 스펙의 `out`·painter 순서도 이 표를 따른다. (M1 리포트의
  `layer_table`은 분석용 (idx,ly,dt) 세분을 유지.)
- 메뉴 **Jobdeck > colour by identifier / layer / CHIP block**: `DeckCache.
  set_mode()`로 재플랜·스펙 재작성 후 레이어 패널을 다시 만들고 워커를 새
  스펙으로 재시작한다(현재 뷰 유지). 창 제목은 `deck.jb · jobdeck N CHIPs · M
  placements · colours by …`.
- File > load layout… 에 `jobdecks (*.jb)` 필터. 소스 중 인덱스 없는 것이
  있으면 "지금 인덱싱할까요?" → `floe2 index deck.jb`를 모달 로그로 실행 후 연다.
  `floe2 view deck.jb`는 인덱스가 없으면 터미널에서 exit 1로 알린다.
- 덱에서 동작하지 않는 것(오류 상태 메시지로 답함): pick/snap, clip, 라벨,
  계층 프레임, margin prefetch. depth/detail/fit/goto/ruler/DRC 좌표 점프는
  덱 좌표(um) 그대로 동작한다.

gate: `ViewerCacheTests`(meta·세 모드의 레이어 표와 색·resolve_layers·close),
`GuiSmokeTests`(`FLOE_GUI_SMOKE_MS`로 `floe2 view --multi deck.jb` 실제 GTK
기동 → 워커 open → 첫 합성 프레임 표시), `test_5_ordinary_commands_take_a_deck`
(`render/info/index/view`의 .jb 처리와 종료 코드).

## 7. 미결·후속
- LY/DT cross vs zip, 회전/미러: 실덱 사례가 나오면 확정.
- 실덱에서 M2 성능 확인: 배치 수 × 패스 비용(플랜+디코드+라스터 각 1회).
  전체 뷰에서 수천 패스가 되면 (a) 같은 소스·같은 scale의 배치를 한 패스로
  묶기, (b) 뷰 밖 컬링 외에 픽셀 미만 배치 스킵, (c) 패스 병렬화 순으로 검토.
- 실덱의 `mag` 분포·소스 내부 배율 배치 유무는 M3 전에 확인(인덱서는
  OASIS 배율 PLACEMENT를 거부하므로 정책 필요).
- M4의 mosaic/anchor/unit 규칙은 KLayout 툴 cli-spec을 그대로 따른다.
