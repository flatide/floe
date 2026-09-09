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

## 1a. MDPView 용어 — level view / chip view (사용자 지적 2026-09-09)

MDPView 매뉴얼은 jobdeck을 **level view**와 **chip view**로 본다. 공개된 매뉴얼
원문은 이 세션에서 확보하지 못했다: manualzz / manualzilla의 "Calibre DESIGNrev
Layout Viewer User's Manual" 사본은 봇 차단(사람 확인 요구, 자동 통과시키지 않음)
또는 비공개(403)이고, Siemens 문서 포털은 로그인 뒤에 있다. 대신 공개된 MEBES
jobdeck 문법 자료로 용어의 실체를 확정했다:

- Artwork Conversion의 "MEBES Job Deck Syntax Summary"(artwork.com/gdsii/
  job_array/page6.htm): `CHIP 1-1,(1,SCNX01Y-01-MB,AD=0.5)` — 괄호 안 첫 숫자가
  **level**, 이어서 pattern file과 AD. 즉 우리 파서의 `$ (idx, …)`의 `idx`는
  **mask level 번호**이고 `MTITLE n,name`은 그 level의 이름이다.
- KLayout 포럼(Matthias, 2015/2021): jobdeck은 여러 MEBES pattern을 x/y로
  배치하는 합성 계층이며 포맷은 비공개.

따라서 매뉴얼의 두 뷰는 다음과 대응하며, 코드·CLI·GUI 용어를 이에 맞췄다:

| MDPView | floe2 | 내용 |
|---|---|---|
| **Level view** | `--mode level` (구 `identifier`, 별칭 유지) / 메뉴 "level view" | mask level(`$n`, MTITLE 이름)마다 한 줄·한 색. 모든 CHIP의 해당 level 배치가 그 색으로 그려진다. 키 `n/0`. |
| **Chip view** | `--mode chip` / 메뉴 "chip view" | CHIP 블록을 덱 순서대로 나열하고 각 CHIP은 자기가 배치하는 level들로 **펼쳐진다**: 패널의 `+CHIP ID001` 아래 `$1 METAL1`, `$2 VIA1`…. 키 `<CHIP 순번>/<level>`; CHIP 줄(`/0`)은 자체 배치 없이 그룹 머리이며 접힌 채 토글하면 하위 level이 함께 토글된다. 색은 CHIP 색. |
| (없음) | `--mode layer` / 메뉴 "source layer view" | 소스 LY/DT별(우리 확장, 매뉴얼 용어 아님). |

chip view에서 level을 "선택"하면 그 level이 `k−1`개의 CHIP 뒤 색 슬롯을 차지한다는
2026-09-07 실측(§2)은 CLI `floe2 jobdeck --mode chip --level k`의 색 순서에
반영돼 있다. 뷰어에서 그 "선택"이 어떤 조작(하이라이트/펼침)인지는 매뉴얼로
확인한 뒤 붙인다 — 현재 뷰어 chip view는 CHIP 색만 쓴다.

렌더 쪽 변화: 덱 스펙의 `layer` 줄이 `key=L/D`(뷰가 쓰는 키 쌍)를 가진다
(`DeckLayer.layer/datatype`; 없으면 `out/0`). renderd의 `style`·`layers=`는 그
쌍으로 해석한다. RENDERD_VERSION 0.12.59.

## 2. 색 규칙 (MDPView 실측 2026-09-07)

열 가지 색이 순환한다(Tk 색 이름 그대로: blue, yellow, red, pink, orange,
white, purple, cyan, magenta, green). 대상의 **순서 목록**을 만들고 i번째에
`palette[i % 10]`을 준다.
- level 모드(level view): level 오름차순. 선택(`--level`)이 색을 바꾸지 않는다.
- layer 모드: LY 오름차순, LY/DT 핀 가능.
- chip 모드(chip view): CHIP은 덱 순서, 선택된 level k는 k−1개의 CHIP 뒤에 끼워
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
| M4 | `floe2 render`의 헤드리스 shot: `--at/--size/--anchor`, 단위 접미사, `--px WxH`(확장/`--stretch`), mosaic, `--batch`(한 번 열고 여러 장), `--report` | ✅ 2026-09-09 (`feature/jobdeck`) |
| M5 | KLayout LayoutView를 독립 오라클로: 배율 인스턴스로 만든 덱을 KLayout이 직접 그린 그림과 합성 프레임을 배터리 픽셀 정책으로 비교하는 gate | ✅ 2026-09-09 (`feature/jobdeck`) |

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
- **뷰 레이어 = 색 대상**(`render.view_layers`, §1a): level view는 level당 한
  줄(`$1 METAL1`, 키 `n/0`), chip view는 CHIP 줄(`CHIP ID001`, `pos/0`) 아래
  그 CHIP의 level 줄들(`pos/level`), source layer view는 (LY,DT)당 한 줄. 레이어
  패널의 토글·색 변경·layerprops 저장이 그 단위로 동작하고, 스펙의 `out`·painter
  순서도 이 표를 따른다. (M1 리포트의 `layer_table`은 분석용 (level,ly,dt)
  세분을 유지.)
- 메뉴 **Jobdeck > level view / chip view / source layer view**(§1a): `DeckCache.
  set_mode()`로 재플랜·스펙 재작성 후 레이어 패널을 다시 만들고 워커를 새
  스펙으로 재시작한다(현재 뷰 유지). 창 제목은 `deck.jb · jobdeck N CHIPs · M
  placements · level view`.
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

## 7. M4 상세 — 헤드리스 shot (`floe2 render`, 소스 종류 무관)

참조 툴의 cli-spec에서 결정된 규칙을 floe2의 기존 명령에 얹었다. 덱 전용
플래그가 아니라 `floe2 render`의 일반 옵션이며 레이아웃에도 그대로 쓰인다
(`floe/shots.py`).

```sh
floe2 render deck.jb --at 53.02mm,92.61mm --size 32mm,25.6mm --px 1200x900 --out a.png
floe2 render deck.jb --at 40000,82000 --size 32000,24000 --anchor lb --px 1200x900 --out b.png
floe2 render deck.jb --corners 40000,80000,60000,95000 --size 4mm,3mm --px 600x450 \
             --line 3 --keep-tiles --out quad.png          # 4장 mosaic, 1200x900
floe2 render deck.jb --mosaic-at "X,Y;X,Y;X,Y;X,Y" --size W,H --out quad.png
floe2 render deck.jb --batch shots.txt --out shots/ --report shots/report.json
```

- 영역: `--bbox` | `--at X,Y --size W,H [--anchor center|lb]` | (없음) 소스 전체.
  모든 길이에 `nm`/`um`/`µm`/`μm`/`mm`/`cm`/`m` 접미사 허용(없으면 um).
- 픽셀: `--px W`는 기존 floe 규칙(높이는 영역 종횡비에서), `--px WxH`는 영역을
  그 종횡비로 **확장**(center는 중심 고정, lb는 꼭짓점 고정; `--stretch`면
  확장 없이 그대로).
- mosaic: `--mosaic-at`는 네 점(시계방향, 좌상부터: tl, tr, br, bl; 각 점을
  `--at`처럼 `--size`/`--anchor`로 읽음), `--corners`는 영역의 네 W×H 꼭짓점
  직사각형(영역 **안쪽**: tl 타일의 좌상 꼭짓점 = 영역의 좌상 꼭짓점). 각 타일이
  `--px`, 결과는 가로세로 2배(tl,tr / bl,br). 구분선은 타일 위에 그린다: 이음매
  중심으로 `floor(W/2)`px는 양쪽 불투명, 나머지는 양쪽 1px 혼합(`--line 0`은
  없음). `--keep-tiles`로 `<out>_tl/_tr/_bl/_br.png`도 남긴다.
- `--batch FILE`(`-`=stdin): 줄마다 `NAME key=value …`(bbox at size anchor px
  stretch layers depth mosaic corners line linecolor keep_tiles; 빈 키는 명령행
  값). 워커를 한 번만 열고 모두 찍는다(`--out`은 디렉터리). `--report`는 shot별
  영역·픽셀·시간(mosaic은 타일 영역과 선 규칙) JSON.
- 출력은 보관용이므로 solid 채움(뷰어의 speckle은 표시용). 단일 shot은 renderd의
  PNG 그대로, mosaic은 raw 타일을 stdlib zlib으로 PNG 인코딩(Pillow 불필요).

## 8. M5 상세 — KLayout 독립 오라클

gate `KLayoutOracleTests`: 참조 툴과 같은 방식으로 덱을 KLayout 레이아웃으로
만든다(int32 안전 1e-4 um 그리드, 소스 셀을 자기 dbu 정수 좌표 그대로 복사,
`ICplxTrans(scale = mag·source_dbu/oracle_dbu, dx, dy)` 인스턴스). 그것을
KLayout `LayoutView`가 동결 셸의 `Renderer`(solid, 선폭 1)로 직접 그린 그림과
floe2 합성 프레임(같은 뷰포트 1024×800)을 배터리 픽셀 정책
(`validate_render_goldens` P-a/b/c: 색마다 1px 경계 밴드 밖 차이 0, 성분 소실
없음, 면적 드리프트 한도)으로 비교하고, 전체 RGB는 색별 경계 밴드 합집합 밖에서
차이 0이어야 한다. 참조 툴 자체를 gate에서 실행하지는 않는다(저장소 밖,
사용자 결정: 구현 참고용).

## 9. 리뷰 2026-09-09 반영 (6건)

| # | 지적 | 조치 | gate |
|---|---|---|---|
| P1-1 | 없는 LY/DT를 지정한 배치가 빠진 PNG를 정상(exit 0)으로 내보내고 `--report`에도 기록이 없음 | ledger가 렌더까지 흐른다: `open_cache`가 덱을 열 때 `skipped` 줄을 출력, `run_shots`가 `WARNING: N placement(s) not drawn`을 찍고 report에 `jobdeck{complete, skipped, view}`를 넣으며, `floe2 render`는 **exit 3**(`floe2 jobdeck`과 같은 뜻: 결과가 불완전). `floe2 info`는 `INCOMPLETE` 줄, 뷰어는 제목에 `· N NOT DRAWN`과 상태 메시지. 스펙은 뷰 레이어를 모두 나열한다(그려지지 않는 level도 레이어; 스타일 파일이 그 이름을 쓴다). | `test_p1_1` |
| P1-2 | 덱 렌더가 단일 캐시의 generation 예산 검사를 우회(LRU에서 축출돼도 scene의 Arc가 페이지를 붙들고 있음) | `Deck::render`가 패스마다 디코드된 페이지 바이트를 합산해 예산을 넘으면 단일 캐시와 같은 오류(`decoded generation budget exceeded: N > B bytes`)로 거부. 프레임 줄에 `pass_bytes_max=`. | `test_p1_2`: 무작위 6만 사각형 소스(반복 압축 불가), 예산 1MiB에서 단일 캐시·덱 모두 거부, 기본 예산에서 둘 다 렌더 |
| P1-3 | 자식 셀에만 도형이 있는 소스가 기본 depth 0에서 검게 나옴(덱은 계층 프레임도 안 그림) | 덱은 **full depth로 연다**(`floe2 view deck.jb` 기본, 뷰어 `open_file`의 .jb도 `_set_depth(999)`); 그리고 `frames=`를 덱 패스에 전달해 depth를 줄이면 소스의 계층 프레임이 그려진다(`frame_paints=`). | `test_p1_3`: full depth 그림 있음, depth 0·frames off는 검정(재현), depth 0·frames on은 프레임, `floe2 render hier.jb` 비검정 |
| P2-4 | `--mosaic-at`의 아래 두 타일이 뒤바뀜(시계방향 입력을 행 우선으로 그대로 넘김) | tl, tr, br, bl 입력을 tl, tr, bl, br로 재배열해 합성·`_bl/_br` 파일·report 태그가 맞음 | `test_units_regions_and_aspect`, `test_cli_region_forms_on_a_deck` |
| P2-5 | `--corners`가 꼭짓점을 중심으로 잡아 기준 도구와 다른 영역(절반이 영역 밖) | 타일은 영역 **안쪽** 꼭짓점 직사각형: `corners=0,0,100,100 size=20,10`의 tl = (0,90,20,100) | 같은 테스트(기대값 교정) |
| P2-6 | 한 줄짜리 batch가 새 `--out` 디렉터리 대신 그 이름의 PNG 파일을 만듦(shot 수·디렉터리 존재로 추론) | `run_shots(batch=…)`를 명시적으로 받는다: batch면 항상 디렉터리, 아니면 단일 PNG(shot 1개 강제) | `test_p2_6` |

RENDERD_VERSION 0.12.60, `__version__` 0.12.71.

### 2차 (5건)

| # | 지적 | 조치 | gate |
|---|---|---|---|
| P1-1 | 계층 프레임 합성 순서: 배치마다 프레임+도형을 완성해 덮으니 앞 배치의 흰 테두리가 뒤 배치의 도형에 가려짐(일반 렌더 240px 유지, 덱 0px) | 단일 캐시 라스터의 순서(회색 밴드 → 도형 → 흰 밴드)를 **덱 전체**에서 지킨다: frames가 켜지면 배치마다 같은 scene을 두 번 라스터(프레임만 / 도형만)하고, 프레임 패스를 구조색으로 갈라(회색 = under 평면, 흰색 = over 평면) 끝에서 under → 모든 도형 → over 순으로 얹는다(`split_frame_planes`). 라스터·플랜·디코드는 그대로. | `test_p1_1_frame_order_is_kept_across_placements`: 겹치는 두 배치, A만 켰을 때의 흰 픽셀이 둘 다 켰을 때도 모두 남음 |
| P2-2 | chip view 이름 선택: `resolve_layers("CHIP ID002")`가 배치 없는 그룹 머리(2,0)만 돌려 검은 화면; 여러 CHIP의 `$1 METAL1`은 마지막 하나만 | `Cache.resolve_layers`처럼 이름은 **모든** 일치 행을, 그룹 머리(CHIP 행, LY의 datatype 0)는 그룹 전체로 확장. `L/D`는 정확히 그 키(+머리면 그룹). 없는 키는 오류. | `test_p2_2` |
| P2-3 | 저장한 덱 색상 미복원(선폭만 복원) | `DeckCache.load()`가 `apply_personal_colors(meta, props_src)`를 적용. 뷰마다 키 공간이 다르므로 `props_src`는 level view = `<deck>.jb`, chip/layer view = `<deck>.chip.jb` / `<deck>.layer.jb`(stem 폴백이 다른 뷰 파일에 닿지 않는 이름); GUI·워커의 layerprops 로드/저장은 `props_src`를 쓴다. | `test_p2_3` |
| P2-4 | 검은 PNG 방지 테스트가 alpha 바이트까지 세어 검은 PNG도 통과 | PNG를 복원(Pillow)해 RGB만 검사 | `_png_lit_pixels` |
| P3-5 | `render_deck_png()`의 solid 채움이 `(layer, 0)` 키라 chip/source-layer view의 datatype≠0 행은 speckle | `(layer, datatype)` 키 | `test_p3_5`: 헬퍼 PNG == 보관용 raw 렌더 |

RENDERD_VERSION 0.12.61, `__version__` 0.12.72.

### 3차 (2건)

| # | 지적 | 조치 | gate |
|---|---|---|---|
| P2-1 | 검은 도형 위로 회색 계층 프레임이 비침: 도형 합성 버퍼의 검은 픽셀을 모두 투명으로 바꿔서 실제 검은 도형도 배경 취급(일반 렌더 회색 0px, 덱 48px) | 도형 합성 버퍼는 처음부터 alpha 0(칠한 픽셀만 불투명)으로 두고, 마지막에 검은 바탕 → under 평면 → 도형 → over 평면 순으로 얹는다. `composite_geometry_only()` 제거. frames가 꺼진 경우도 같은 경로(바탕 + 도형)라 픽셀 동일. | `test_p2_1`: 두 level을 검정으로 recolor해도 회색 0px, 흰 프레임은 그대로 |
| P2-2 | source-layer view에서 DT0 선택에 다른 datatype이 섞임(모든 datatype 0을 그룹 머리로 간주) | 그룹 머리는 chip view의 가상 CHIP 행뿐: 뷰 행에 `head` 플래그(meta `jobdeck_head`)를 두고 그 행만 확장. `7/0`·`LY7.DT0`은 (7,0) 하나. | `test_p2_2`: dt.oas(7/0, 7/1) 덱에서 DT0 단독 선택의 픽셀 수가 둘 다 켠 것과 같음(DT1은 DT0 안쪽) |

RENDERD_VERSION 0.12.62, `__version__` 0.12.73.

## 10. 미결·후속
- LY/DT cross vs zip, 회전/미러: 실덱 사례가 나오면 확정.
- 실덱에서 M2 성능 확인: 배치 수 × 패스 비용(플랜+디코드+라스터 각 1회).
  전체 뷰에서 수천 패스가 되면 (a) 같은 소스·같은 scale의 배치를 한 패스로
  묶기, (b) 뷰 밖 컬링 외에 픽셀 미만 배치 스킵, (c) 패스 병렬화 순으로 검토.
- 실덱의 `mag` 분포·소스 내부 배율 배치 유무는 M3 전에 확인(인덱서는
  OASIS 배율 PLACEMENT를 거부하므로 정책 필요).
- M4의 mosaic/anchor/unit 규칙은 KLayout 툴 cli-spec을 그대로 따른다.
