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
| **Level view** | `--mode level` (구 `identifier`, 별칭 유지) / 메뉴 "level view" | mask level마다 한 줄·한 색. 이름은 MTITLE 그대로(`LEVEL-1` 등), 없으면 `LEVELn`. 내부의 칩별 가시성은 유지한다. |
| **Chip view** | `--mode chip` / 메뉴 "chip view" | **레벨 → 소스 칩** 트리. `LEVEL-1` 아래 `PATTERN01.TE`처럼 TC의 파일 이름을 표시한다. 부모 키 `level/0`, 칩 키 `level/source순번`. 부모는 자체 도형 없는 그룹이며 펼침 여부와 무관하게 하위 칩 전체를 토글한다. |
| (없음) | `--mode layer` / 메뉴 "source layer view" | 소스 LY/DT별(우리 확장, 매뉴얼 용어 아님). |

### 2026-09-10 추가 관찰 및 제품 결정

사용자의 Calibre 관찰: 칩뷰는 `0 JOBDECK`, 레벨 6행, 소스 칩 약 138행의
평면 목록이었다. 레벨뷰에서 레벨을 전부 끈 뒤 칩뷰로 가면 레벨 행은 꺼져
있지만 칩 행은 켜져 도형이 다시 나타났다. 이 평면 목록과 가시성 동작을
그대로 복제하지 않고, **표시 명칭은 맞추되 floe의 계층형 제어를 사용**한다.

- 같은 레벨에서 같은 TC의 반복 ROWS/CHIP 배치는 한 칩 행으로 묶는다.
  같은 TC가 여러 레벨에 쓰이면 각 레벨 아래 독립 행으로 둔다. 식별은
  정규화한 전체 TC 경로이며 basename이 같아도 경로가 다르면 합치지 않는다.
  툴팁에 TC 경로와 관련 CHIP 식별자를 표시한다.
- 부모 토글은 하위 전체에 적용된다. 일부만 켜져 있으면 `[partial]`을
  표시한다. 레벨뷰↔칩뷰 전환은 정확한 칩별 가시성을 보존하므로 숨긴
  도형이 다시 나타나지 않는다. 레벨뷰는 내부 칩 행을 패널에서 숨길 뿐,
  렌더 선택은 동일한 leaf 키를 사용한다. source layer view는 별도 상태다.
- 소스 순번·기본 색은 전체 덱 기준으로 고정한다. 로드 레벨을 줄여도
  달라지지 않는다. 칩뷰 팔레트는 레벨 다음 소스 등장 순서이며, 실제
  Calibre 색 배정과의 완전 일치는 아직 검증하지 않았다.
- 기존 `--layers '$1 TITLE'`/`'$1'`은 별칭으로 유지한다. `level/0`은 해당
  레벨의 칩 전체를 선택한다. 소스 이름으로 선택하면 모든 일치 행을
  선택한다. 기존 `CHIP C01` 선택은 새 목록의 제어 단위가 아니므로 오류다.
- 기존 칩뷰 layerprops는 다른 키 공간이므로 적용하지 않는다. 새 파일은
  `<deck>.chip-by-level.jb.layerprops`; 옛 파일을 삭제하거나 덮어쓰지 않는다.
  레벨뷰의 기존 부모 색·채움·선폭·숨김 설정은 하위 칩에 적용된다.
- `0 JOBDECK` 행과 Calibre depth 대응은 이번 변경에 포함하지 않는다.
  관찰상 TOP에서 depth 2는 박스, 3은 도형이지만 추가 관찰 후 결정한다.

§2의 CHIP-block 색 splice는 기존 분석 CLI `floe2 jobdeck --mode chip --level`
규칙으로 남으며, 뷰어의 소스 칩 트리/색 정책과 구별한다. 검증:
`JobdeckChipHierarchyTests`(반복·경로 충돌, 그룹 토글, 부분 선택, 모드 왕복,
동일 가시성의 렌더 픽셀 일치, 기존 레벨 속성).

실제 GUI 연속 전환 검사에서 발견한 경합도 함께 차단했다. 레이어 토글의
debounce가 모드 전환 뒤까지 남으면 새 워커의 open/style 준비 전에
`style_epoch=0` 렌더를 보내 시작이 실패했다. 캐시 재바인딩 때 이전
debounce를 취소하고, open 중에는 제출하지 않으며 준비 완료 콜백에서
현재 뷰를 그린다. 실제 Viewer 왕복·가시성·뷰포트·최종 프레임 게이트를 추가했다.

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
  줄(MTITLE 이름, 키 `n/0`), chip view는 level 줄 아래 소스 칩 줄들
  (`level/source순번`), source layer view는 (LY,DT)당 한 줄. 두 jobdeck 뷰는
  같은 내부 칩 행을 사용하고 level view에서만 패널의 칩 행을 숨긴다. 레이어
  패널의 토글·색 변경·layerprops 저장이 그 단위로 동작하고, 스펙의 `out`·painter
  순서도 이 표를 따른다. (M1 리포트의 `layer_table`은 분석용 (level,ly,dt)
  세분을 유지.)
- 메뉴 **Jobdeck > level view / chip view / source layer view**(§1a): `DeckCache.
  set_mode()`로 재플랜·스펙 재작성 후 레이어 패널을 다시 만들고 워커를 새
  스펙으로 재시작한다(현재 뷰 유지). 창 제목은 `deck.jb · jobdeck N CHIPs · M
  placements · level view`. **Ctrl+,** (메뉴 "toggle level view / chip view",
  사용자 요청 2026-09-10)는 level view ↔ chip view를 바로 바꾼다(source layer
  view에서는 level view로; 덱이 아니면 상태줄 안내만). gate
  `JobdeckShortcutTests`.
- **레벨 선택 로드**(사용자 요청 2026-09-10, Calibre MDPView처럼): 덱을 열 때
  먼저 "load jobdeck levels" 대화상자가 mask level마다 한 줄(`$n 이름`, CHIP 수,
  인스턴스 수, 소스)을 체크 목록으로 보여 주고(all/none), 고른 level만 플랜·
  인덱싱·표시한다. File > load jobdeck…, `floe2 view deck.jb`(항상 창을 거쳐
  연다), 실행 중인 창으로 forward된 열기 모두 같다. `floe2 view deck.jb --level
  1,3`은 묻지 않고 그 level로 열고, 스크립트·gate는 `FLOE_JOBDECK_LEVELS=all|
  N[,N...]`로 답한다(기본 `ask`; level이 하나뿐인 덱은 묻지 않음). 현장
  2026-09-10: File > load jobdeck…에서 고른 덱이 인덱스가 있으면 `open_file`로
  직행해 대화상자가 안 떴다 → `_load_picked`가 덱을 항상 `_open_or_index`로
  보낸다(gate `test_load_dialog_routes_a_deck_through_the_level_question`);
  대화상자는 `keep_above`+`present`로 부모 위에 머물고 닫힌 뒤 `_restore_keys`로
  키를 돌려준다(숨은 모달은 "대화상자 없음 + 키 먹통"으로 보인다). 선택은
  `DeckCache(ids=…)`로 들어가 뷰 레이어 표(level/chip/source layer view 모두)와
  스펙이 그 level만 담고, 색은 세 뷰 모두 전체 덱 기준으로 고정된다(로드
  선택이 색을 옮기지 않음 — `plan_deck(load_ids=)`; 분석 CLI의 `--level`
  splice 규칙과 별개). 인덱싱도 그 level의 소스만(`floe2 index deck.jb --level
  1,3`), 준비 판정(`deck_ready(ids=)`)도 같다. 준비 단계도 로드한 level의
  소스만 전체 probe하고, 나머지 소스는 그리드 계약을 위해 헤더의 dbu만 읽으며
  선택 밖 배치는 세기만 하고 만들지 않는다(9차 리뷰). 창 제목 `· levels 1,3 of 4`. Jobdeck 메뉴
  **select levels to load…**로 열린 덱의 선택을 바꾸면(필요한 소스는 인덱싱 후)
  뷰를 유지한 채 다시 연다. CLI `--level`은 index/info/render/view에 있다
  (`render`의 보고서 `jobdeck.levels`). gate `LevelSelectTests`,
  `IndexOnOpenTests.test_deck_asks_levels_then_opens_them`.
- File > **load jobdeck…**(2026-09-09; `.jb` 필터가 앞에 오는 같은 대화상자) 또는 File > load layout… 의 `jobdecks (*.jb)` 필터. 소스 중 인덱스 없는 것이
  있으면 "지금 인덱싱할까요?" → `floe2 index deck.jb`를 모달 로그로 실행 후 연다.
  `floe2 view deck.jb`도 같다(2026-09-09: 인덱스 없는 파일은 레이아웃·덱·DRC db
  모두 뷰어가 묻고 인덱싱한 뒤 연다 — `Viewer._open_or_index`,
  `FLOE_INDEX_ON_OPEN=yes|no`로 자동 응답).
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

### 4차 (4건, 인덱스 없는 파일 열기 흐름)

| # | 지적 | 조치 | gate |
|---|---|---|---|
| P2-1 | 자동 인덱싱 뒤 `--drc`가 사라짐: 레이아웃 인덱싱과 DRC 열기를 따로 예약해 DRC가 먼저 실리면 이후 `_apply_cache()`가 초기화(1건 → 0건) | `_open_or_index(..., then=)`: DRC 열기를 레이아웃 열기 **성공 콜백 뒤**에 연결. GUI smoke가 `--drc` 지정 시 `_drc_total > 0`을 요구 | `test_then_runs_after_a_successful_open_only`, `IndexOnOpenSmokeTests`(인덱스 없는 chipA + 생성한 chipA.db, 정책 yes → 인덱싱·열기·DRC 유지·pack 생성) |
| P2-2 | DRC에는 `FLOE_INDEX_ON_OPEN`이 적용되지 않음 | `_index_consent(question)` 하나로 레이아웃·덱·DRC pack이 같은 정책(yes/no/ask)을 쓴다 | `test_drc_pack_shares_the_consent_policy` |
| P2-3 | headless 검증(`test_5`)이 디스플레이 확인 없이 GUI를 실행 | 그 검사는 디스플레이 가드가 있는 `IndexOnOpenSmokeTests`로 이동 | 같은 클래스 |
| P3-4 | 검은 도형 회귀 테스트가 흰 프레임만 생기는 배율이라 회색을 검사하지 않음 | 자식 프레임이 9~25px(회색 outline 밴드)이 되는 배율로 계산(`_gray_scale_view(child_um)`): 도형 없음(hier.jb) → 회색 있음, 색 도형·검은 도형(frames.jb) → 회색 0 | `test_p2_1_black_design_covers_the_gray_frame` |

`__version__` 0.12.75 (Python·gate만; renderd 0.12.62 유지).

### 현장 2026-09-09: 확대 시 `decoded generation budget exceeded: 2040099526 > 1073741824`

리뷰 P1-2로 넣은 패스별 예산 검사가 실칩 중간 배율에서 프레임 자체를 거부했다.
이제 패스는 페이지를 **우선순위 순으로 32개씩 디코드하며 예산에 닿으면 멈추고**,
디코드된 페이지로 프레임을 그린다(`partial=1 deferred=N`, 메모리 상한 유지).
어댑터는 settled 프레임의 `deferred`를 `over_budget_pages`로 올리고 상태줄에
`N pages over budget (not drawn)`으로 표시한다. 단일 캐시 경로는 종전대로 거부한다.
RENDERD_VERSION 0.12.64. 근본 해결(뷰 수준 LOD/wash가 덱 패스에도 같은 강도로
걸리는지, 페이지 우선순위가 화면 기여도 순인지)은 실칩 측정 뒤 §10.

### 현장 2026-09-09: 누락 소스 3개 때문에 인덱싱 완료된 덱이 열리지 않음

`deck_ready`/`DeckCache.unindexed()`가 누락·읽기불가 소스도 "인덱스 없음"으로 세어
열기를 막았다. 이제 **그릴 수 있는(probe ok) 소스만** 인덱스를 요구하고, 누락·
읽기불가·미지원 소스는 skipped ledger(제목 `N NOT DRAWN`, 상태줄, `info`의
INCOMPLETE)로 보고하며 덱은 열린다. 인덱싱이 일부 실패해도 성공한 소스로 연다
(로그는 close 버튼으로 남음). 그릴 것이 하나도 없을 때만 거부한다.
`floe-index`는 **plain OASIS만** 읽으므로 GDS·gzip 소스는 probe에서 `unsupported`
로 분류돼(dbu는 알지만) 묻지 않고 skip된다 — 미리 OASIS로 변환해 둘 것.

### 현장 2026-09-09: 소스 인덱싱 실패 `TRAPEZOID: out of spike scope`

실제 덱의 소스(마스크 데이터)에 OASIS TRAPEZOID/CTRAPEZOID 레코드가 있어 인덱서가
거부했다. `floe-oasis` 파서가 23–26 레코드를 KLayout과 같은 꼭짓점 규칙으로
폴리곤화한다(README의 인덱서 지원 레코드, `tools/validate_oasis_shapes.py`).
같이 고친 것: 뷰어의 인덱싱 모달 로그가 실패·취소 시 바로 닫히지 않고
`close` 버튼으로 남아 오류 메시지를 읽을 수 있다. RENDERD_VERSION 0.12.63.

### 5차 (3건, 2단계 이후 리뷰 2026-09-09, RENDERD 0.12.69)

| # | 지적 | 판정 | 조치 |
|---|---|---|---|
| P1-1 | 병렬 묶음의 이미지 메모리가 예산 밖: 묶음은 새 decoded 바이트·64패스로만 끊는데 패스 결과(창 이미지)는 묶음이 끝날 때까지 보관되고, scene 재사용 패스는 새 바이트가 0이라 1024² 64패스가 1 MiB 예산에서 306 MB RSS | 사실 | 패스의 창 이미지(geometry, 프레임 패스가 있으면 ×2)를 `pass_image_bytes`로 묶음에 **페이지와 함께 청구**한다. 묶음은 다음 패스가 예산의 절반을 넘기기 **전에** 닫힌다(단독 패스는 항상 실행). 프레임 줄 `batches=`·`batch_bytes_max=`. gate `ReviewFixTests5.test_p1_1`: 1024² 전체 프레임 17패스가 1 MiB 예산에서 17묶음(최대 청구 = 한 패스), 기본 예산에서 1묶음, 픽셀 동일; 창 모드의 청구는 프레임보다 작다 |
| P2-2 | 서브윈도 좌표 정밀도: `dx`=10¹⁶·`scale`=0.001에서 OFF 20,800 px, ON 0 px | 사실 | 창을 **덱 좌표가 아니라 소스 좌표**(정수 dbu bbox + raster가 쓰는 `source_view`)에서 raster와 같은 식 `(x − view.x0)·width/span`으로 계산한다(`subwindow(&BBox, &RasterViewBox, w, h) → Window::{Part, Outside, Full}`). 덱 좌표 10¹⁶은 f64에서 2 단위로 반올림되지만 raster는 소스 dbu를 소스 뷰로 사상하므로 창이 배치 옆에 놓였던 것. 유한하지 않으면 `Full`(전체 프레임)로 복귀, 밖이면 `Outside`. 덱 좌표의 사전 컬링(`boxes_intersect`)도 제거(플랜 뷰의 소스 bbox 클리핑이 같은 판정을 정확히 한다). 단위 테스트 `subwindow_at_a_huge_deck_offset_follows_the_raster`(덱 좌표식은 열 1200, raster는 1140), gate `test_p2_2`: `dx`=10¹⁶·`scale`=7e‑7에서 OFF/ON 바이트 동일(2000² 뷰, 상자 왼쪽 변 열 1140·아래 변 행 859 확인) |
| P2-3 | 병렬 이후 `raster_us`는 패스 합이라 실제 경과(66 ms)보다 크게(130 ms) 보이고 `workers=1`은 패스 내부 타일 워커뿐 | 사실 | 묶음의 병렬 raster 구간 **벽시계** `raster_wall_us`(묶음은 직렬이므로 합 = 프레임의 실제 raster 시간), 동시 패스 수 `pass_workers`, `batches`를 프레임 줄에 추가. `raster_us`(합)·`workers`(타일 워커)는 의미를 유지. 상태줄: `scene S + frame sum F + composite C ms, raster wall W ms Pp x Tt, B batches, pass max MB, batch max MB`. gate `test_p2_3`: jobs 1 → `pass_workers` 1, jobs 4 → 4, wall ≤ 합 |

### 6차 (1건, 5차 반영 리뷰 2026-09-09, RENDERD 0.12.70)

| # | 지적 | 판정 | 조치 |
|---|---|---|---|
| P2 | 화면 밖 배치 때문에 전체 렌더 실패: 5차에서 덱 좌표 사전 컬링을 없애자 `dx`=10¹⁶·`scale`=0.001의 화면 밖 배치가 소스 뷰 −10¹⁹ dbu를 i64로 바꾸다 `coordinate overflow: source view x0`로 프레임 전체를 실패시킴(회귀) | 사실 | `source_plan_request`가 소스 bbox 클리핑을 **f64에서 정수 변환보다 먼저** 한다: 소스 뷰의 floor/ceil을 소스 bbox(i64→f64)와 비교해 엄격히 벗어나면 `None`(빈 패스, `passes_skipped`), 겹치면 클리핑한 값(항상 bbox 안, 즉 i64 안)만 `checked_bound`로 변환. 덱 좌표 검사는 복구하지 않음(정밀도 문제). 단위 테스트 `offscreen_placement_at_a_huge_offset_is_skipped_not_an_error`, gate `ReviewFixTests5.test_p2_4`: 정상 배치 + 화면 밖 10¹⁶ 배치가 정상 배치만 있는 덱과 바이트 동일, `passes 1 / skipped 1` |

### 7차 (1건, 6차 반영 리뷰 2026-09-09, RENDERD 0.12.71)

| # | 지적 | 판정 | 조치 |
|---|---|---|---|
| P2 | bbox 반올림으로 유효한 뷰가 뒤집힘: 6차의 클리핑이 정수 bbox를 f64로 바꿔 클리핑한 뒤 다시 정수로 보정해, 소스 bbox x = 2⁶⁰+1..2⁶⁰+2(양끝이 f64에서 모두 2⁶⁰)에서 x0 = 2⁶⁰+1 > x1 = 2⁶⁰가 되어 invalid view로 프레임 실패(합성 OASIS 재현; 실칩 좌표는 아님) | 사실 | 축별 `clip_low`/`clip_high`: 뷰 경계(f64)가 bbox 경계에 닿거나 넘으면 **원래 i64 bbox 값**을 그대로 쓰고, 엄격히 안쪽인 뷰 경계만 정수로 변환(변환 뒤 `max/min`으로 bbox가 반대로 반올림된 경우 보정). 뷰 경계가 i64 범위 밖 먼 쪽이면 미스(None, 빈 패스). bbox 값은 f64를 거치지 않는다. 단위 테스트 `plan_clip_keeps_exact_bounds_at_huge_coordinates`: 2⁶⁰+1..2⁶⁰+2 소스가 (2⁶⁰+1, 2⁶⁰+2)로 플랜되고, 반대편 i64 끝의 소스는 오류 없이 미스. gate 재현은 없음 — fixture 생성기(KLayout)가 32비트 좌표라 2⁶⁰ 소스를 만들 수 없다 |

### 8차 (2건, 3·4단계 리뷰 2026-09-10, RENDERD 0.12.73)

| # | 지적 | 판정 | 조치 |
|---|---|---|---|
| P2-1 | 스트리밍에서 `decode_pages` 제한으로 빠진 페이지가 정상 완료로 보고됨(4페이지 플랜, `decode_pages=2` → 2페이지만 그리고 `partial=0 deferred=0`, 전체 렌더와 15,984 px 차이). 기본 GUI 요청(제한 없음)에는 없음 | 사실 | 선택 단계에서 제외된 페이지 수를 **두 경로에 공통으로** `deferred`에 더하고 `partial`을 세운다(whole-scene 경로는 scene의 deferred로 partial만 잡고 수는 세지 않았고, 스트리밍 경로는 슬라이스 scene이 구조상 partial이라 아무것도 보고하지 않았다). gate `ReviewFixTests8.test_p2_1`: dense.jb 1 MiB·`decode_pages=2`(render 명령에 삽입)에서 `over_budget_pages`>0·전체 렌더와 다름, 제한 없으면 0·동일 |
| P2-2 | 스트리밍 경로의 `raster_wall_us`에 계층 프레임 raster 시간이 빠짐(직렬 구간) | 사실 | `stream_pass`의 프레임 raster도 벽시계에 더한다. gate `StreamTests` depth 1 케이스(dense.oas D0에 손자 셀 K 추가: D 페이지는 스트리밍, K는 프레임): `frame_passes` 1·스트리밍 1·픽셀 동일; 리뷰 보완(비차단)으로 직렬 fixture의 모든 케이스에서 `raster_wall_us` ≥ `raster_us`(geometry 슬라이스 + 프레임 합, deck 카운터에 노출)를 검사한다 — wall ≥ frame만으로는 이전 버그(11.97 < 13.84 ms)도 통과했다 |

실덱 계측을 읽을 때: 실제 raster 시간은 `raster wall`, 합계 `draw/raster ms`는
패스 수만큼 부풀어 있다.

### 9차 (4건, 레벨 선택 로드 리뷰 2026-09-10)

| # | 지적 | 판정 | 조치 |
|---|---|---|---|
| P2-1 | 전체 level 재선택이 다시 묻거나 다른 선택이 됨: `_open_or_index(ids=None)`이 "전체"와 "아직 안 물음"을 같이 뜻해, 일부만 연 뒤 메뉴에서 전체를 고르면 ask 정책은 대화상자가 두 번, `FLOE_JOBDECK_LEVELS=3`이면 [3]만 열림 | 사실 | `ask_levels` 인자 분리: `ids`가 주어지거나 `levels=` 필드가 있거나 `ask_levels=False`면 묻지 않는다. 메뉴 재선택은 대화상자의 답(None = 전체)을 `ask_levels=False`로 넘긴다. gate `IndexOnOpenTests`(ask_levels=False·env=3에서 None 그대로 열림; `test_reselecting_every_level_is_final`) |
| P2-2 | 소스가 많으면 대화상자가 화면보다 넓어짐(줄바꿈·말줄임 없는 Label, 250개 이름에서 26,741 px) | 사실 | `level_row_text`: 요약은 소스 이름 3개 + "+N more", 전체 목록은 툴팁; Label `ellipsize END`·`max_width_chars 56`, 대화상자 기본 폭 640. gate `test_load_dialog_rows_are_bounded` |
| P2-3 | 부분 로드여도 준비 단계가 전체 소스·전체 배치를 처리(전체 소스의 헤더+캐시 상태 조회, 17배치 생성 후 3개로 필터) | 사실 | 그리드 계약(전체 덱의 extent와 모든 소스 dbu)은 유지하되, 로드한 level의 소스만 전체 probe(캐시 상태 포함)하고 나머지는 `header_dbu`로 헤더의 dbu만 읽는다(`infos`에 등록 안 함); `plan()`은 선택 밖 엔트리의 extent·개수만 세고 배치는 만들지 않는다. gate: level 3 로드 시 `catalog.infos`는 mark.oas뿐, `instances_total` 17·`instances` 3·dbu는 전체 덱 값 |
| P2-4 | chip view 색이 선택에 따라 바뀜(ID001이 전체 로드에선 노랑, level 3만 로드하면 파랑): `scheme.build(deck, ids)`가 분석 CLI의 splice 규칙을 그대로 씀 | 사실 | `plan_deck(load_ids=)`를 분석 선택 `ids`와 분리: 로드 선택은 배치·행·probe 범위만 줄이고 색은 전체 덱 기준(세 뷰 모두), `floe2 jobdeck --level`의 splice 규칙은 그대로. Calibre의 실제 색 정책은 샘플 관찰 후 확정. gate `test_load_selection_moves_no_colour_in_any_view` |

### 10차 (3건, thin 정책 분리 리뷰 2026-09-11, RENDERD 0.12.78)

| # | 지적 | 판정 | 조치 |
|---|---|---|---|
| P1-1 | GUI 프레임 캐시 키(`_render_key`)에 thin 정책이 없어 메뉴 토글 뒤 `_covered()`가 옛 화면을 유효로 보고 렌더를 내지 않음(상태줄만 바뀜) | 사실 | 키에 `_effective_thin()` 포함(margin 프레임도 같은 키). gate `test_render_key_carries_the_thin_policy`(auto/cull/keep, 덱 auto = keep) |
| P1-2 | renderd `RetainedKey`에 `thin_keep`이 없어 다른 정책의 프레임을 재사용(cull→keep 0 px 유지, keep→cull 1,176 px 유지, 타일 16개 재사용); published query scene도 같은 키라 pick/snap도 옛 정책 | 사실 | `RetainedKey.thin_keep` 추가. renderd 단위 테스트 `retained_key_tracks_the_thin_policy`(keep≠cull, 생략=cull, 잘못된 값 거부) |
| P2-3 | `--thin auto`가 실행 중인 창에 전달되지 않아 keep 상태의 창을 기본으로 되돌릴 수 없음 | 사실 | `--thin` 기본을 None으로 두고 명시된 값은 auto 포함 전달(`thin=auto`). gate `test_single_instance_forwards_effective_detail_and_depth` 확장 |

### 11차 (7건, 점유 요약 구현 계획 리뷰 2026-09-11, 문서만)

`docs/OCCUPANCY_PLAN.ko.md` 1차 계획에 대한 리뷰. 판정과 반영은 그 문서 §11에
기록했다. 요지: bbox 기반 점유 생성 폐기(도형 교차 마킹을 M1 필수로), 적용 조건에
exact·depth full·top 일치 추가, 전용 마스크 경로와 레이어 순서 유지, 오차 계약을
2셀 규칙으로 정정하고 독립 도형 오라클·빈 공간·pan 위상 gate 추가, `.ovo`
유효성·게시·자체 버전(`CACHE_VERSION` 불변), jobdeck 래퍼 옵션 명시 전달, M5까지
opt-in과 자원 상한, pick/snap 제외.

## 11. 성능 분석 2026-09-09 — 판정과 계획

리뷰어의 분석(광역뷰 누락 = cut 정책, 다중 level 지연 = 배치별 전체 화면 반복,
budget = 패스별 디코드 보유)을 코드와 대조했다. 모두 사실이다.

| 주장 | 판정 | 근거 |
|---|---|---|
| detail high도 1px cut이며 no-cut이 아니다 | 사실 | `DETAIL_PX = (5, 3, 1)` |
| 페이지 크기 cull이 wash보다 앞서 적용돼 작은 도형의 넓은 반복이 통째로 사라진다 | 사실 | `hier.rs` 페이지 `max_w/max_h < cut` cull, BVH `max_dim < cut` prune; 1×1 dbu 500×500 반복이 high에서 0px |
| 배치마다 전체 화면 plan→scene→raster→합성을 반복하고, 프레임이 없어도 프레임 패스를 돈다 | 사실 | `deck.rs` 배치 루프; 프레임 패스는 무조건 실행이었음 |
| `10008tiles`는 누적 작업량이며 병목은 로딩이 아니라 반복 | 사실 | 합성 fixture: 6 level ON 385 ms / 1 level OFF 38 ms, 픽셀 동일 |
| 응답의 `scene_us=0`·합성 시간 미계측으로 ~457 ms가 설명되지 않는다 | 사실 | 프레임 줄이 단일 캐시 필드를 0으로 채움 |
| budget 상한은 한 배치 패스에서도 넘을 수 있고, 최신 변경은 오류 대신 partial을 남기지만 완전한 화면을 보장하지 않으며 스크린샷·보고서는 그 partial을 버린다 | 사실 | `deck.rs` 청크 디코드; `shots.py`는 `refining`만 봄 |

### 1단계 — 픽셀을 바꾸지 않는 것 (이 커밋, RENDERD 0.12.65)
- 프레임 패스는 그 배치의 plan에 계층 프레임이 있을 때만(`subtree_has_frames`).
- 계측: 프레임 줄에 `scene_us`(실측), `frame_passes`, `unique_pages`(합산 `pages`와
  별도), `pass_bytes_max`, `frame_raster_us`, `composite_us`(overlay+최종 layering);
  어댑터는 `result["deck"]`으로 올리고 상태줄에 `deck N passes (F frame, S
  skipped) U/P pages, scene+frame+composite ms, pass max MB`.
- 스크린샷: 캡처의 `over_budget_pages`를 shot 행과 보고서(`complete`,
  `jobdeck.over_budget_pages`)에 기록, `floe2 render`는 exit 3. shot 행의
  `complete`는 덱의 누락 배치(`skipped_placements`)와 예산 초과를 **둘 다**
  반영해 전체 보고서와 같은 판정을 낸다(리뷰 2026-09-09: shot만 읽는 배치
  처리가 누락 이미지를 완전한 것으로 봤음); 마지막 `INCOMPLETE` 로그도 같다.
- gate `PerfAnalysisTests`.

### 2단계 — 배치별 전체 화면 구조
- ✅ (RENDERD 0.12.66) **배치의 화면상 창만 raster·합성**: 배치의 덱 bbox를 장치
  픽셀로 옮겨 stroke 여유(9px)를 더한 창 `[c0,r0,c1,r1)`을 라스터에 **장치 창
  인자**로 넘긴다(`render_geometry_styled_cancellable_windowed`). 라스터는 전체
  프레임의 world→device 사상·타일 격자·채움 위상을 그대로 쓰고 창 밖 타일은
  배경으로 두며 work-bin 수집도 창으로 컬링한다 → 창 안 픽셀은 전체 렌더와
  바이트 동일. (부분 창 뷰를 f64로 다시 계산하는 첫 시도는 경계 픽셀의 반올림이
  달라 폐기.) 합성도 창 영역만. 창 밖 배치는 `passes_skipped`. 플랜은 여전히 소스
  뷰 전체(페이지 선택 불변). 킬 스위치 `FLOE_RUST_DECK_SUBWINDOW=off`. gate
  `SubwindowTests`: solid·speckle·16×16 패턴·선폭 3·프레임 on·홀수 크기 프레임·
  줌 뷰에서 off/on 바이트 동일.
- ✅ (RENDERD 0.12.67, 실측 없이 효과가 클 것으로 판단한 것부터) **프레임 내
  plan/scene 재사용**: 플랜 뷰를 소스 bbox로 클리핑하고 `px_per_dbu`를 덱 span을
  한 번 나눠 구하므로, 같은 소스·같은 배율로 전부 보이는 배치들은 플랜 요청이
  같아 하나의 plan·scene(디코드 포함)을 공유한다(`scene_reuses=`). 프레임 scene
  캐시는 예산을 넘으면 비운다. **배치 병렬 라스터**: 준비된 패스를 예산의 절반·
  64개 단위 묶음으로 워커 수만큼 병렬 라스터(묶음이 여럿이면 패스당 워커 1)하고,
  합성은 배치 순서대로 직렬 → 픽셀 불변. gate `ReuseAndBatchTests`: 재사용 수,
  worker 1 vs 4 vs 전체 프레임 경로 바이트 동일(speckle·패턴·프레임).
- ✅ (RENDERD 0.12.68) **버퍼를 작업량에 비례**: windowed 라스터가 창 밖 타일은
  밴드조차 만들지 않고 **창 크기 프레임**만 돌려준다(`assemble_window`: 타일의
  창 교집합만 복사 → 전체 렌더의 창과 바이트 동일, 라스터 단위 테스트
  `windowed_render_is_the_crop_of_the_full_render`가 타일 크기 2/3/10에서 확인).
  덱 합성은 창 크기 패스를 오프셋에 얹는다. 배치당 전체 프레임 할당·초기화가
  사라진다.
- 타일 소유 합성과 worker pool: 창·묶음 병렬·scene 재사용으로 목적(배치당 전체
  화면 반복 제거)은 달성했으므로 별도 구조는 두지 않는다. 남는 배치당 고정
  비용은 패스마다 한 번의 work-bin 수집(창으로 컬링)과 묶음당 스레드 spawn이며,
  실칩 계측(`passes`, `raster wall`, `composite ms`, `scene reuses`)에서 그
  비중이 크게 나오면 그때 다시 본다.
- (5차 리뷰, RENDERD 0.12.69) 묶음 예산에 창 이미지 청구, 소스 좌표 서브윈도,
  `raster wall`/`pass_workers`/`batches` 계측 — §9 5차 표.

### 3단계 — 페이지를 버리지 않는 유한 메모리 렌더 ✅ (2026-09-10, RENDERD 0.12.72, 실측 없이 사용자 결정)
- **슬라이스 스트리밍**(`stream_pass`): 한 패스의 decoded 합이 슬라이스 한도
  (예산의 1/2, `STREAM_SLICE_DIV`)에 이르고 아직 디코드할 청크가 남으면 그
  패스는 scene 전체를 보유하지 않는다. 지금까지의 슬라이스를 scene으로 만들어
  창에 raster하고 합성에 겹친 뒤 버리고, 다음 슬라이스를 디코드해 반복한다.
  "예산 초과 → partial(`N pages over budget`)"은 사라지고 프레임은 완전하다.
- **픽셀 불변의 근거**: 덱 패스는 한 레이어를 한 색으로만 칠하고(프레임은 별도
  패스), 합성은 opaque-over이므로 칠해진 픽셀의 **합집합**만 결과를 정한다 →
  슬라이스 순서·개수와 무관하게 whole-scene raster와 바이트 동일. 프레임 패스는
  플랜에서 나오므로 페이지 없는 scene으로 한 번만 raster.
- 순서: 스트리밍 패스 앞의 준비된 묶음을 먼저 합성(배치 순서 유지)하고, 스트리밍
  패스는 합성 버퍼에 직접 겹친다. 프레임 scene 캐시에는 넣지 않는다(같은 소스의
  다른 배치는 LRU 적중으로 다시 디코드).
- 상태줄 `N streamed in K slices`, 프레임 줄 `streamed_passes= slices=`. 킬 스위치
  `FLOE_RUST_DECK_STREAM=off`(예산에서 멈추는 partial 프레임으로 복귀).
- 왜 타일/레이어 마스크 누적이 아닌가: 단색 단일 레이어 패스에서는 창 크기 RGBA
  겹침이 곧 마스크 누적과 같고(같은 픽셀 집합), 별도 마스크 자료구조와 합성 규칙이
  필요 없다. 메모리 상한 = 슬라이스(예산/2) + LRU 잔류 + 창 이미지.
- gate `StreamTests`: dense.jb 1 MiB 예산(스트리밍, 2+ 슬라이스) vs 기본 예산
  (whole scene) 바이트 동일 — speckle·패턴·프레임 on·줌 뷰(서브윈도); 킬 스위치는
  partial을 되돌린다. `ReviewFixTests.test_p1_2`·`PerfAnalysisTests`도 새 계약으로
  재고정(1 MiB 캡처가 완전·exit 0).

### 4단계 — jobdeck 전용 광역 표시 정책 ✅ (2026-09-10, RENDERD 0.12.72, 실측 없이 사용자 결정)
- **sub-cut wash**(`ViewReq::sub_cut_wash`, 덱 플랜 요청에만 켬): 크기 cut이
  **버리던** 것을 자기 레이어의 footprint wash(렉트, 보통 채움 → 뷰어 speckle이
  얇게 함)로 남긴다.
  - 자체 페이지: cut(양축 < cut, 또는 hairline)에 걸린 페이지 → `(layer, page
    bbox)`; 페이지 BVH 노드가 통째로 잘리면 `(prange layer, node bbox)`(페이지
    BVH는 (cell, layer)별이라 레이어가 정확).
  - 하위 셀 배치(depth-full 생략·유한 깊이 fold): 배치 footprint(반복 extent
    한 렉트)를 자식 `lmask_rec ∩ vis`의 각 레이어에 → 반복 구조의 존재가 한
    렉트로 남는다.
  - 자식 BVH 노드가 크기로 잘리면: 플랜당 `sub_cut_walk_budget`(200k 노드) 안에서는
    잎까지 걸어 배치별 정확한 레이어로, 넘으면 노드 bbox를 현재 셀의 가시 레이어에
    **coarse** wash(`sub_cut_coarse`). rev 43의 prune이 막던 O(가시 배치) 폭주를
    예산으로 막는다. r == 0(깊이 소진, 자식은 outline뿐)에서는 wash 없음.
  - top 셀이 통째로 sub-cut이면(덱 뷰의 0.2× 마크) 플랜을 버리지 않고 위 규칙으로
    wash → 마크의 색이 남는다(`CompositeTests.test_3b` 재고정).
- 한계(문서화): wash는 페이지/배치 bbox이므로 30% 채움의 콘택 배열이 100%
  블록으로 보인다(speckle이 완화). 뷰어의 일반 레이아웃 경로는 바뀌지 않는다
  (`sub_cut_wash=false`). 킬 스위치 `FLOE_RUST_DECK_WIDE=off`. 상태줄 `N sub-cut
  washes`, 프레임 줄 `wide_washes=`.
- gate `WideViewTests`: tiny.jb(1 µm 점 4만 개의 자체 페이지 + 1 µm 자식 셀
  200×200 배열)를 200 px 전체 뷰·cut 3 px에서 — off면 두 level 모두 0 px, on이면
  네 모서리까지 칠해지고 색은 exact 렌더와 같다; test.jb(cut 위)는 wash 0·픽셀
  동일.

측정은 실덱의 같은 뷰·같은 level 조건에서 1단계 계측 값과 5차 리뷰의 `raster wall`·`pass_workers`·`batches`, 3·4단계의 `streamed/slices`·`sub-cut washes`로 한다. 3·4단계는 실측 없이 구현했으므로 실덱 확인 항목: 광역 뷰에서 wash 블록이 MDPView의 표시와 비슷한지, 스트리밍 패스의 슬라이스 수와 raster wall.

## 10. 미결·후속

- **현장 2026-09-10: 특정 줌부터 사라지는 반복 도형.** 35.8 × 34.6 mm 소스에서
  우하단 영역만 남고 연결된 동일 형태 반복이 약 229 µm 뷰부터 사라짐(덱 모드에선
  4단계 sub-cut wash 블록으로 남음 — `FLOE_RUST_DECK_WIDE=off` 실측으로 확정).
  우하단은 Calibre와 유사하게 동작하므로(뭉침이 빠른 건 cut 차이, Calibre는 0.5 px
  추정) 원인 확정이 먼저다. 후보는 페이지 크기 cut(`max_w/max_h < cut`), hairline
  (`max_min < 0.5 cut`), 자식 셀 생략/BVH 프루닝이며, 어느 것인지는 `floe-index
  plan <src>.floe --view … --px-per-um … --cut-px … --explain 1`(SPEC-INDEXER §6)로
  사라지는 뷰와 보이는 뷰를 각각 찍어 같은 셀·레이어의 판정을 비교해 정한다.
  설계 논의(밀도 사다리: 크기 cut 대신 exact/LOD 점유, hairline은 전체 길이의
  1 px 선, 자식 bbox+rep 방출; 격자 솎아내기는 프레임에만)는 판정이 나온 뒤
  착수한다. LOD 쌍은 `floe2 index --force --lod` 재인덱싱이 필요하다(기본 끔).
  사용자 실측 2: 안 보이는 뷰(210.7 × 203.5 µm, cut 0.256 µm)에서 선택 페이지 0·
  hier 1/1 → 플랜 단계에서 전부 제거; 보이는 뷰는 cut 0.747 µm에서도 1페이지가
  살아남음(판정 크기 역전 = 저장 구조가 다름). 사용자 실측 3: `floe2 render`
  캡처는 두 영역 모두 나옴 → render는 `cut_px 0`(exact)이라 원인이 플랜 cut임을
  확정. 이를 GUI 없이 재현하도록 `floe2 render --detail low|medium|high|exact`
  (기본 exact)를 두었다(gate `test_render_detail_reproduces_the_viewer_cut`).
  실측 4(perf 카운터): `cut pages 4/pbvh 0/cbvh 0/cells 0` → 페이지 판정.
  실측 5(`--explain`): 4페이지 모두 `cull_hair` — ICV_3641/ICV_3640, 레이어 3/0·
  3/300, max_w 11.4/118.6 µm, max_h 0.08/4.3 µm, max_min 0.081/0.124 µm, 멤버
  약 2만 9천. 모든 레코드가 hair 문턱 0.128 µm 아래인 가는 선 페이지(한 페이지는
  4 nm 차이). 리뷰어 정정: max_min/max_w는 최댓값이지 전 레코드 값이 아님, 3/0과
  3/300의 동일성은 미확정(930/931 지표가 다름), 이전 0.747 µm 뷰의 hair 문턱은
  0.374 µm라 "130 nm 레코드 하나로 통과" 설명은 성립하지 않음, 비용 추정 1,300만은
  930 한 페이지(합 2,910만)이며 32 × 페이지 면적은 픽셀당 겹침 상한이 아님. 조치:
  리뷰어 권고대로 **페이지 hairline 규칙만 해제**(FLOE2_OPTIMIZATION "결함 B
  1차 해제"), 기존 raster 그대로. LOD run 1 px 접기·밀도 사다리·크기 cut 제거·
  재인덱싱은 실칩의 `thin pages N kept` 비용 측정 뒤로 분리. **정책 분리
  (2026-09-11)**: 마스크는 hairline이 많으므로 덱은 keep이 기본, 일반 레이아웃은
  기존 성능 정책(cull)을 유지하고, 단독 마스크 OASIS는 `--thin keep`/View > keep
  thin shapes로 선택한다. 요청별 `thin=keep|cull`로 전달되며 공유 기본값이 아니다.
  gate `ThinPageTests`(덱 기본 keep, 레이아웃 기본 cull, `--thin`, 진단 override).
  **실측 6(2026-09-11, 덱 fit 뷰 165 × 160 mm, 1 px = 202 µm)**: keep 정책으로
  25,138페이지(그중 thin 25k)를 전부 디코드(합 30.9 s)하고 4,170만 hairline을
  칠해(draw 22.2 s) 프레임 32 s → 광역 뷰에서는 쓸 수 없음. 이 줌에서 선
  하나하나는 1 px 미만이라 페이지 점유 덩어리만 남는데 그 비용을 전부 치른다.
  리뷰어 방향(동의): 근접뷰는 exact keep, 광역뷰는 **빈 공간의 위치를 보존하는
  다중 해상도 점유 요약**이 원본 디코드·raster를 대체해야 한다. bbox 채움과 bbox당
  밀도 숫자 하나는 제외(빈 공간 위치 소실; 제가 낸 밀집 bbox wash 안도 철회).
  기존 LOD는 긴 path/polygon을 verbatim으로 남겨(`vfs.rs` 3265행) 처리량이 충분히
  줄지 않을 수 있음. 먼저 소스 하나·레이어 하나로 요약 생성 비용, 광역뷰 시간,
  빈 공간 보존을 재는 실험: `tools/occupancy_experiment.py SRC.oas --layer L/D
  --out DIR`(미세 exact 렌더로 만든 점유 피라미드의 생성 비용·저장량, keep/cull
  fit 렌더 시간, fit 해상도 exact를 기준으로 2/4/8 px 셀이 채우는 빈 영역을
  크기별로 집계). 전체 재인덱싱 여부는 그 결과로 결정.
  **실측 7(2026-09-11, 문제 영역 26 × 33 mm·레이어 하나, `occupancy_experiment`)**:
  미세 exact 렌더(4.06 µm/px, 2048 px 타일 16장) 4.3 s; level 0 점유 387만/5,200만
  셀(7.4 %, 6.5 MB), fit 레벨(32 µm) 9.3만 셀(101 KB), 130 µm 8천 셀(15.8 % —
  셀이 굵어질수록 과대). fit keep 렌더는 1.93 GB > 1 GiB로 거부(exact keep으로는
  이 영역의 광역뷰를 그릴 수 없음), fit cull 렌더는 0.21 s에 81 px(사실상 빈 화면,
  점유는 11 %). 굵은 셀의 과대 표시(기준 = 1 px 셀 요약, 점유 92,710 px): 2 px
  +15,046 px(+16 %, jaccard 0.860), 4 px +35,786(+39 %, 0.722), 8 px +69,914(+75 %,
  0.570). **표시 오차 기준: 요약 셀은 화면 1 px 이하** — 2배 피라미드에서 셀 ≤ 1 px
  인 가장 굵은 레벨(0.5~1 px)을 쓰면 과대는 1 px 미만 빈 공간에 한정. 결론: 요약
  방식 성립(생성 초 단위, 저장 레이어당 약 15 MB, 광역뷰 페인트 수만 셀). 다음
  단계(착수는 사용자 결정): 색인 시 도형 기준 점유 피라미드 `design.ovo`, keep
  정책에서 µm/px ≥ 기준 셀이면 요약으로 대체(페이지 디코드·계층 순회 생략),
  renderd 요약 pass, 덱 합성 통과, gate·킬 스위치. 구현 계획(형식·생성기·선택
  규칙·덱 통합·gate·단계·위험)은 `docs/OCCUPANCY_PLAN.ko.md`. **M1(생성기·
  `design.ovo`·CLI·gate) 완료 2026-09-11**(RENDERD 0.12.79; 계획 §12): `floe2
  index <src> --occupancy-only`로 기존 캐시에 추가(opt-in), `floe2 index deck.jb
  --occupancy`로 덱 소스 전체. **M2(단일 소스 renderd 요약 경로) 완료
  2026-09-11**(RENDERD 0.12.80): 단독 마스크 OASIS의 keep 광역뷰가 요약으로
  그려진다. 덱 합성은 M4.

- **배율/임의각 PLACEMENT(OASIS 18)**: 2026-09-09 현재 실제 소스에서 아직 관측되지
  않아 보류(사용자 확인). 나타나면 계층 변환을 실수화하지 않고 **인덱싱 시
  평탄화**로 구현한다: 파서 뒤 단계에서 배율≠1 또는 각도가 90° 배수가 아닌
  배치의 자식 도형을 변환해 부모 셀로 복사(꼭짓점은 dbu 반올림 = KLayout
  flatten 규칙, PATH 반폭도 배율 적용, 텍스트는 위치만), 배치의 repetition은
  복사된 도형의 `rep`로 유지, 중첩은 재귀·순환은 오류, 평탄화 도형 수 상한
  초과는 명확한 오류. gate는 KLayout flatten과 꼭짓점 집합 대조. 현재 오류
  문구: `magnified/angled placement: out of spike scope`.
- LY/DT cross vs zip, 회전/미러: 실덱 사례가 나오면 확정.
- 실덱에서 M2 성능 확인: 배치 수 × 패스 비용(플랜+디코드+라스터 각 1회).
  전체 뷰에서 수천 패스가 되면 (a) 같은 소스·같은 scale의 배치를 한 패스로
  묶기, (b) 뷰 밖 컬링 외에 픽셀 미만 배치 스킵, (c) 패스 병렬화 순으로 검토.
- 실덱의 `mag` 분포·소스 내부 배율 배치 유무는 M3 전에 확인(인덱서는
  OASIS 배율 PLACEMENT를 거부하므로 정책 필요).
- M4의 mosaic/anchor/unit 규칙은 KLayout 툴 cli-spec을 그대로 따른다.
