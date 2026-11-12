# 캡처 CLI 레퍼런스 (`floe2 render` 외)

셸 스크립트와 자동화에서 뷰어 없이 그림을 뽑는 명령 모음이다. 한 장 찍기, 여러 장 한 번에 찍기,
네 모서리를 한 장으로 합치기, jobdeck의 레벨·칩 지정, 보고서 JSON, 인덱스가 없는 소스의 처리,
종료 코드와 환경 변수를 한곳에 모았다. 옵션의 의미는 `floe2 render --help`가 정본이고, 이 문서는
그것을 쓰임새별로 묶고 규칙을 보탠 것이다. DRC 에러 스냅샷은 [DRC-CLI.ko.md](DRC-CLI.ko.md) §3,
결정의 이력과 측정은 §11의 문서 지도에 있다.

실행 형태는 환경에 따라:

```sh
rust/target/release/floe2 render ...     # 소스 체크아웃 (cd rust && cargo build --release)
floe2 render ...                         # floe2-portable 번들 (bin/ 이 PATH에 있을 때)
```

웹 호스트 `floe2-web`도 같은 `render`·`info`·`clip`·`fe-embed`를 갖는다(공유 app-cli;
SHARED_APP_LAYER.ko.md). 도움말과 메시지의 프로그램 이름만 다르다.

**공통 규약**

- 입력은 레이아웃(`.oas`, `.gds`)이나 jobdeck(`.jb`)이다. 덱 전용 옵션은 표에 "덱"으로 표시했다.
  레이아웃에 주면 거부한다(`--level`, `--chipid`/`--chipnumber`는 명령행에서 2, `--chip`/`--chip-off`/`--fit-chip`은
  찍을 때 1).
- stdout은 진행 줄(`[floe2] rendered …`, `[jobdeck] …`, `WARNING`, `note`)이고, 데이터는 `--report`
  JSON 파일로 받는다. 자동 색인의 출력과 오류(`floe2: …`)는 stderr다. DRC 캡처만 stdout에 TSV를 쓴다.
- 길이는 모두 µm가 기본이고 `nm`·`um`·`µm`·`μm`·`mm`·`cm`·`m` 접미사를 받는다(`53.02mm`). 소수 DBU는
  그대로 유지된다. 좌표의 두 꼭짓점은 뒤집혀 있어도 된다.
- 출력은 원자적으로 게시된다. 임시 파일에 다 쓴 뒤 rename하므로 실패하거나 취소된 샷은 이전 파일을
  남긴다. 권한은 `rw-rw----`(`FLOE_ARTIFACT_MODE`). 입력 파일과 캐시는 출력 대상이 될 수 없고, 한 실행의
  출력 이름은 서로 겹칠 수 없다.
- 그림은 뷰어가 같은 영역을 연 화면과 같다(`floe2 view --goto`, depth full). 보관용 모양은 `--archival`.

**종료 코드**

| 코드 | 뜻 |
|---|---|
| 0 | 모든 샷이 나왔다. 빠진 것이 있어도 그림이 나왔으면 0이다(0.12.437): 켠 칩의 건너뛴 배치, 디코드 예산을 넘긴 페이지, 점유 요약을 기다린 pass(`summary_building`)는 stdout의 `WARNING: N jobdeck placement(s) not drawn`, stderr 마지막 줄 `floe2: WARNING: rendered incomplete - …`, 보고서 `complete=false`로 알린다 |
| 3 | `FLOE_RENDER_INCOMPLETE_EXIT=3`(§7)을 두었을 때만: 그림은 나왔지만 빠진 것이 있다(0.12.436까지의 기본). stderr `rendered incomplete - … [exit 3]` |
| 2 | 명령행 오류: 모르는 옵션, 잘못된 값, 함께 쓸 수 없는 옵션, 레이아웃에 준 `--level`·`--chipid`·`--layer-colors`·`--density on`, 덱에 준 `--labels on` |
| 1 | 실행 중 입력 오류와 실패: 없는 파일, 읽을 수 없는 `--layerprops` 파일, 덱에 없는 레벨·레이어 번호·칩 이름, 레이아웃에 준 `--chip`/`--fit-chip`, 아무 배치에도 닿지 않는 영역, 자동 색인 실패(`indexing NAME first failed (exit N …)`), 렌더러 오류. `floe2-web`은 2 |
| 75 | 다른 실행이 그 소스를 색인 중이다(`--wait`로 기다릴 수 있다) |
| 128+signal | 취소됐다(SIGINT 130, SIGTERM 143). 쓰던 출력은 남기지 않는다 |

---

## 1. 한 장 찍기: `floe2 render SOURCE [OPTIONS]`

```sh
floe2 render chip.oas --bbox 0,0,2000,2000 --px 800 --out a.png            # 영역, 폭 800 px, 높이는 종횡비
floe2 render chip.oas --px 1600 --out whole.png                            # 소스 전체
floe2 render chip.oas --at 1000,1000 --size 200,150 --px 1200x900 --out b.png   # 중심과 크기
floe2 render chip.oas --bbox 0,0,2000,2000 --layer METAL1,VIA1 --legend --out m.png
floe2 render chip.oas --bbox 0,0,2000,2000 --archival --out export.png     # 보관용: cut 0, solid, 프레임·라벨 없음
floe2 render deck.jb  --level 2 --bbox 40000,80000,60000,95000 --px 1200 --out deck.png
floe2 render deck.jb  --level 2 --chip MAIN.oas --out main.png             # 그 칩만 켜고 그 범위를
```

### 1.1 영역과 크기

| 옵션 | 기본 | 의미 |
|---|---|---|
| `--bbox X0,Y0,X1,Y1` | 소스 전체 | 영역. 덱에서 `--chip`/`--chip-off`를 줬으면 기본은 켠 칩들의 배치 범위 |
| `--at X,Y --size W,H` | | 같은 영역을 점과 크기로. `--anchor center`(기본)면 X,Y가 중심, `lb`면 왼쪽 아래 꼭짓점 |
| `--px W` | max | 가로 px. 높이는 영역의 종횡비에서 나온다 |
| `--px max` | | 영역의 종횡비로 16 Mpx(4096×4096) 안에 드는 가장 큰 크기. 생략과 같다(0.12.433) |
| `--px WxH` | | 가로·세로 px. 영역을 그 종횡비로 **넓힌다**(center는 중심 고정, lb는 꼭짓점 고정) |
| `--stretch` | | `--px WxH`일 때 영역을 넓히지 않고 그대로 늘여 그린다 |
| `--depth N` | full | 계층 깊이. 0은 top만, 999나 생략은 전체 |
| `--fit-chip NAME[#K]` | | 덱. 영역을 그 칩의 배치 범위로(§4.2). 좌표 영역과 함께 쓸 수 없다 |
| `--in-chip NAME[#K]` | | 덱. 영역 좌표를 그 배치의 **칩 좌표**(소스의 µm)로 읽어 덱이 그 칩을 놓는 대로 놓는다(`X = dx + mag·x`, 길이는 mag배). `--bbox`, `--at`, `--size`, `--mosaic-at`, 좌표 `--corners`에 모두 적용. 영역 없이 주면 그 배치 자체(§4.2). `--fit-chip`과 함께 쓸 수 없다 (0.12.435) |

- 한 프레임은 16 Mpx(4096×4096)까지다. 넘으면 같은 종횡비로 그 안에 드는 가장 큰 크기로 그리고(0.12.431),
  stdout의 `WARNING: a.png: 1280x13440 px for 40.0000x420.0000 um (0.0000,0.0000,40.0000,420.0000 um)
  exceeds 16 Mpx - drawn at 1264x13272`로 요청한 크기와 영역을 말하며 보고서에 `pixel_requested`를 남긴다.
  `FLOE_CAPTURE_REDUCE=off`면 줄이지 않고 거부하되 같은 수치를 댄다(`floe2: frame … exceeds 16 Mpx; --px 1264x13272
  fits …`, exit 1). `--px W`만 주면 높이가 영역의 종횡비에서 나오므로 가로의 10.24배보다 긴 영역은 1280으로도 넘는다.
- `--px`를 생략하면 영역의 종횡비로 상한 안에 드는 가장 큰 크기로 그린다(0.12.433; 그 전은 가로 1200). 정사각
  영역은 4096×4096, 403×446 um의 소스 전체는 3895×4307, 40×420 um 띠는 1264×13272다. 전과 같은 1200을
  쓰려면 `--px 1200`을 주거나 `FLOE_CAPTURE_PX=1200`(§7)을 둔다. `--drc` 스냅샷의 기본은 그대로 1200 정사각이다
  ([DRC-CLI.ko.md](DRC-CLI.ko.md) §3).
- `--bbox`, `--at`, `--mosaic-at`, `--corners`는 서로 배타다.

### 1.2 무엇을 그릴지

| 옵션 | 기본 | 의미 |
|---|---|---|
| `--layers NAME,L/D,…` / `--layer` | all | 레이어 이름·별칭이나 `L/D` 쌍. 덱은 레벨 이름(MTITLE, 없으면 `LEVELn`)이나 칩의 파일 이름. 말하지 않으면 전부이되 `--layerprops` 파일이 숨긴 레이어는 빠진다. `all`은 숨긴 것까지 전부다(0.12.452; §1.3) |
| `--layerid N,N.D,…` | | 레이어 패널의 번호. `N`은 datatype 0, `N.D`(`N/D`도). 덱의 `N`은 레벨 N 전체. `--layers`와 함께 주면 둘 다 |
| `--level N,N,…` | 전체 | 덱. 이 마스크 레벨만 로드한다. 로드하지 않은 레벨의 소스는 열지도 색인하지도 않는다 |
| `--chip NAME,…` | | 덱. 이 칩들만 켠다(§4.2). `--layers`와 함께 쓸 수 없다 |
| `--chip-off NAME,…` | | 덱. 이 칩들만 끄고 나머지를 켠다 |
| `--chipid ID,…` `--chipnumber N,N-M,…` | | 덱. 이 CHIP 블록의 소스와 배치만 열고 색인하고 그린다(§4.3) |

- 숫자가 아닌 `--layerid`는 명령행에서(2), 덱에 없는 번호는 찍을 때(1) 거부한다.
- 이름에 `level 2`처럼 주면 `--layerid 2`를 쓰라고 알린다. 예전 `--layers '$2'` 표기도 그대로 받는다.

### 1.3 그림 모양

기본은 뷰어의 첫 화면과 같다: detail medium, thin keep, 레이아웃은 컷 아래 밀도 켬, 뷰어의 채움과
layerprops, 셀 프레임과 라벨(덱은 라벨 없음, 밀도 없음).

| 옵션 | 기본 | 의미 |
|---|---|---|
| `--detail exact\|low\|medium\|high` | medium | 크기 컷 0/5/3/1 px. `exact`는 모든 도형을 그리거나 예산 초과로 실패한다 |
| `--thin auto\|keep\|cull` | auto(=keep) | 광역에서 가는 도형을 1 px 헤어라인으로 남길지(keep) 버릴지(cull) |
| `--density on\|off` | 레이아웃 on | 컷이 버린 도형을 덮는 면적만큼의 밝기로. 컷이 있어야 하고(`--detail low\|medium\|high`), 캐시에 `design.ovs`가 있어야 한다(전체 인덱스; 경량 캐시에는 없어 `on`을 거부, `floe-index ovs CACHE`가 더한다). 덱은 거부. `--archival`은 off. `FLOE_RUST_DENSITY_STACK=off`면 기본 off |
| `--occupancy on\|off` | 말하지 않음 | 광역 뷰를 점유 요약(`design.ovo`)으로. `on`은 요약 없는 소스의 요약을 먼저 만들고(자동 색인), `off`는 덱도 요약 없이 페이지로. 말하지 않으면 덱은 있는 요약을 쓰고 만들지는 않으며(경량 소스의 광역 pass는 비고 불완전으로 알린다), 레이아웃은 off. 컷이 있어야 한다 |
| `--frames[=on\|off]` `--labels[=on\|off]` | on | 셀 프레임, 설계 텍스트. 덱의 라벨은 지원하지 않는다 |
| `--layer-colors row\|group` | row | 레이아웃의 기본 레이어 색. row는 패널 줄마다, group은 layer 번호마다 한 색. `FLOE_LAYER_COLORS`가 기본값. 덱은 거부 |
| `--layerprops FILE` | | 이 실행의 레이어 속성 파일(0.12.452). Calibre `.layerprops`를 소스 옆 디자인 기본값 위에 덮는다: 파일에 있는 레이어의 색·채움·선폭, 그리고 숨김. 뷰어의 `floe2 view SOURCE --layerprops FILE`과 같다. 아래 설명 |
| `--mode level\|chip\|layer` | level | 덱의 뷰. 뷰어의 Jobdeck 메뉴와 같다. level은 마스크 레벨마다 한 색, chip은 레벨 아래 소스 칩마다 한 색(chip view), layer는 소스 레이어 LY/DT마다 한 색. 그 뷰에서 저장한 layerprops(`<deck>.jb.layerprops`, `<deck>.chip-by-level.jb.layerprops`, `<deck>.layer.jb.layerprops`)가 적용된다(§4.6). 레이아웃은 거부 (0.12.436) |
| `--legend` | | 그린 레이어의 범례(색, 채움, 이름 L/D)를 이미지마다 flateyes 메타데이터로, 배율 `ppu`(가로 px/um)와 `unit=um`과 함께(0.12.433; DRC 스냅샷과 같다). 픽셀은 그대로다. `floe2 fe-embed --dump`로 읽는다. 덱에서 `--chip`으로 고르면 켠 칩의 줄만(0.12.436), chip view(`--mode chip`)에서는 칩 줄, 어느 뷰에서나 그림에 실제로 보이는 줄만(0.12.439; `FLOE_LEGEND_IN_VIEW=placed`면 틀 안에 배치가 있는 줄, `off`면 로드한 레벨 전부)(§4.6) |
| `--archival` | | 보관용 모양: 컷 0, solid 채움, 밀도·프레임·라벨 없음(0.12.349까지의 기본). 함께 준 옵션이 이긴다. `FLOE_RENDER_DEFAULTS=archival`이면 이것이 기본 |
| `--label-font-px N` | 14 | 라벨 글자 크기 6..96 |

- 레이아웃의 디코드 예산(`FLOE_RUST_BUDGET_MB`, 기본 1024)은 뷰어와 같다. 컷이 있으면 뷰어처럼
  예산에 맞춰 성기게 그리고 `WARNING: … thinned to fit the render budget: …`과 보고서의 `budget_fit`으로
  알린다. `exact`는 실패하며 필요한 양과 길(`--detail high`, 좁은 영역, `--layers`, 예산 올리기)을 말한다.

**레이어 속성 파일 지정: `--layerprops FILE`** (0.12.452; 사용자 "floe2 render에도 --layerprops 추가해줘")

```sh
floe2 render layout.oas --layerprops /user/props/review.layerprops --bbox 0,0,2000,1500 --out a.png
floe2 render layout.oas --workdir /user/tmp --layerprops review.layerprops --legend --report a.json --out a.png
floe2 render deck.jb --mode chip --layerprops chips.layerprops --out chips.png
```

- 쓸 수 없는 폴더의 소스에는 디자인 기본값(`SOURCE.layerprops`)을 둘 수 없다. 이 옵션은 그 파일을 실행에서 지정한다.
  뷰어의 `floe2 view SOURCE --layerprops FILE`(0.12.451)과 같은 파일을 받는다.
- 형식은 Calibre `.layerprops`다(`L.D 색 fill 이름 표시 선폭`). 뷰어의 Layer > load layer properties가 올리는 것과 같은
  규칙으로 디자인 기본값 위에 덮는다.
  - 파일에 있는 레이어: 색(이름이나 `#rrggbb`), 채움(fill 이름), 선폭(숫자. 1 이하면 기본값의 선폭을 지운다), 표시
    여부(`0` 숨김, `1` 표시). 뜻이 없는 칸(모르는 색 이름, 모르는 fill, 숫자가 아닌 선폭)은 기본값을 그대로 둔다.
  - 파일에 없는 레이어는 기본값 그대로다. 소스에 없는 레이어의 줄은 건너뛴다.
  - `--archival`에서는 색만 쓰인다(채움은 solid, 선폭 1). `--layer-colors`는 파일이 색을 정하지 않은 레이어의 기본 색을
    정한다.
- 파일이 숨긴 레이어는 무엇을 그릴지 말하지 않은 샷에서 빠진다. 범례에도 없다.
  - 샷이 스스로 말하면 그대로 그린다: `--layers`/`--layerid`로 준 레이어, `--chip`/`--chip-off`로 고른 칩은 파일이
    숨긴 것이라도 그린다. 색과 채움은 파일의 것이다. `--chip-off`의 나머지는 로드한 칩 전부다.
  - `--layers all`은 숨긴 것까지 전부다. batch 줄의 `layers=all`도 같고, `layers=`(빈 값)은 기본으로 되돌린다.
  - 모든 레이어를 숨기는 파일이면 그림이 비고 `WARNING: … hides every layer of this source`가 난다.
  - 소스 옆 디자인 기본값의 숨김은 전처럼 캡처에 적용하지 않는다. 숨김이 적용되는 것은 이 옵션으로 준 파일뿐이다.
- jobdeck은 찍는 뷰(`--mode`)의 줄로 읽는다(§4.6). level view의 `N.0`이 레벨 N이다.
- 파일은 소스를 열기 전에 읽는다. 없거나 읽을 수 없으면(텍스트가 아님, 4 MiB 초과, 폴더) 색인도 캡처도 하지 않고 1로
  끝난다(`floe2: layer properties file not found: PATH`, `floe2: layer properties file PATH: REASON`). 값이 비면 2다.
  링크는 따라가 읽는다. 상대 경로는 실행한 디렉터리 기준이다.
- 소스의 레이어를 하나도 가리키지 않는 파일은 오류가 아니다. 기본값으로 그리고
  `WARNING: … names no layer of SOURCE`로 알린다.
- 실행에 하나다(batch 키가 아니다). `--drc` 스냅샷에도 쓴다([DRC-CLI.ko.md](DRC-CLI.ko.md) §3). `--out`과 `--report`는
  이 파일을 덮어쓸 수 없다.
- stdout에 `[floe2] layer properties: FILE - M of its N row(s) for layers of this source, K hidden`이 한 번 나오고,
  보고서 맨 위 `layerprops`가 파일·줄 수·숨긴 줄을 말한다(§5).
- `info`와 `probe`에는 없다. 뷰어 안에서 바꾼 색을 저장한 파일(Layer > save layer properties)을 그대로 줄 수 있다.

### 1.4 출력

| 옵션 | 기본 | 의미 |
|---|---|---|
| `--out FILE` | view.png | PNG. `--batch`에서는 디렉터리(샷마다 `<이름>.png`; 샷이 하나여도) |
| `--report FILE` | | JSON 보고서(§5). 단일 샷도 받는다 |

### 1.5 인덱스가 없는 소스

`info`·`render`·`probe`·`clip`은 인덱스가 없는 소스를 먼저 색인한다(`floe2 index`의 규칙으로: 덱의
소스는 경량 캐시, 레이아웃은 전체 인덱스; 출력은 stderr). 바뀐 소스는 있는 캐시를 그대로 쓰고 경고한다
(`index --force`로 다시 만든다).

| 옵션 | 의미 |
|---|---|
| `--no-index` | 인덱스 없는 소스를 거부한다(`FLOE_AUTO_INDEX=off`와 같다) |
| `--temp-index[=DIR]` | 이 실행만의 폴더(DIR, 없으면 `$TMPDIR`)에 색인해 읽고 끝나면 폴더째 지운다. 소스 옆에는 아무것도 남지 않는다. 덱 캡처의 광역 pass가 요약을 기다리면 거기 요약을 만들고 다시 그린다(`--occupancy off`면 요약 없이). `--no-index`와 함께 쓸 수 없다 |
| `--light` / `--full` | 자동 색인이 만드는 캐시의 종류. 말하지 않으면 덱의 소스는 light, 레이아웃은 full(`FLOE_INDEX_LIGHT=on\|off`). 이미 있는 캐시는 그대로 연다 |
| `--wait[=SECONDS]` / `--no-wait` | 다른 실행이 색인 중인 소스를 75로 거부하지 않고 기다린다(`FLOE_LOCK_WAIT=on\|SECONDS`) |
| `--workdir DIR` | 캐시(덱이면 모든 소스의)를 소스 옆이 아니라 DIR 아래 소스의 절대 경로 자리에서 읽고 쓴다(`/proj/a/x.oas` → `DIR/proj/a/.x.oas.ice/`). 소스 옆은 읽지도 쓰지도 않는다. DIR은 없으면 만든다. 모든 floe2 명령이 받고 줄 어디에 있어도 된다(`FLOE_WORKDIR`이 기본; [CACHE-NAMING.ko.md](CACHE-NAMING.ko.md) §6) |

- 자동 색인은 `floe2 index`의 기본값(스트리밍, `--mem`, 요약 상한)으로 돈다. 다르게 만들려면 미리
  `floe2 index SOURCE [--light|--full] [--occupancy] [--level N] …`을 돌린다.
- 같은 덱을 다시 찍을 것이면 `floe2 index deck.jb`가 낫다. 임시 인덱스는 쓸 수 없는 폴더나 정말 한 번뿐일
  때를 위한 것이고, 비용은 영구 색인과 같다(DIRECT_READ_PLAN §8.29).
- 영역이 있는 덱 캡처는 그 영역이 닿는 배치의 소스만 열고 색인한다(§4.4).
- 소스 옆에 쓸 수 없는 폴더면 자동 색인은 `indexing x.oas first: cannot write beside x.oas, in /proj/a:
  Permission denied (os error 13) - … --workdir DIR (or FLOE_WORKDIR) puts them under a directory this
  account can write, at the source's absolute path: DIR/proj/a/`로 1로 끝난다(0.12.430; `floe2 index`와
  `drc --build`도 같은 말). `--workdir DIR`이나 `--temp-index`를 준다(CACHE-NAMING.ko.md §6).

---

## 2. 여러 장 한 번에: `--batch FILE|-`

```sh
floe2 render deck.jb --level 2 --px 1200 --batch shots.txt --out shots/ --report shots/report.json
cat shots.txt | floe2 render chip.oas --batch - --out shots/
```

```
# shots.txt: 줄마다 NAME key=value …  (전체 줄 주석만, 빈 줄 무시)
top_left   bbox=0,0,2000,1500
pad_a      at=53.02mm,92.61mm size=32mm,25.6mm px=1200x900 anchor=lb
metal_only bbox=0,0,2000,1500 layer=METAL1,VIA1
level_two  layerid=2
main_four  fit_chip=MAIN.oas corners=fit size=300um,300um px=800 line=3 linecolor=#ff0000
quad       mosaic="40000,95000;60000,95000;60000,80000;40000,80000" size=4mm,3mm keep_tiles=1
```

- 소스를 한 번 열고 모두 찍는다. `--out`은 디렉터리이고 샷은 `<NAME>.png`(mosaic의 타일은
  `<NAME>_tl/_tr/_bl/_br.png`)로 나온다. `-`는 stdin이다.
- NAME은 파일 이름으로 안전해야 한다: 비어 있지 않고 247 바이트까지, `=`·`/`·제어 문자 없음, `.`·`..`
  아님, 한 파일 안에서 유일.
- 값은 셸처럼 따옴표로 묶을 수 있다(`'…'`, `"…"`, `\` 이스케이프). `#`은 줄 첫머리에서만 주석이라
  `linecolor=#ff0000`은 값이다.
- 키: `bbox` `at` `size` `anchor` `px` `stretch` `layers`(=`layer`) `layerid` `depth` `mosaic` `corners`
  `line` `linecolor` `keep_tiles` `chip` `chip_off` `fit_chip` `in_chip`. 없는 키는 명령행 값을 쓴다. 켜고 끄는 값은
  `1`/`on`/`true`/`yes`. `corners=fit`은 기본 영역의 네 모서리다. `px=max`는 영역 종횡비의 가장 큰 크기(§1.1)라
  명령행의 `--px 1200`을 줄 하나에서 되돌릴 수 있다.
- 영역 대체 규칙: 줄에 자기 좌표 영역(`bbox`, `at`, `mosaic`, 좌표 `corners`)이 있으면 명령행의 영역
  키를 `fit_chip`까지 모두 버린다. 줄에 `fit_chip`이나 `corners=fit`만 있으면 명령행의 좌표 영역만
  버린다.
- 칩 좌표 틀(`in_chip`, 0.12.435): 명령행의 `--in-chip`은 줄의 좌표(`bbox`, `at`, `size`, `mosaic`, `corners`)에도
  적용된다. 줄의 `in_chip=NAME#K`는 명령행의 틀을 바꾸고, `in_chip=`(빈 값)은 덱 좌표로 되돌린다. 줄의 `fit_chip`은
  명령행의 `in_chip`을, 줄의 `in_chip`은 명령행의 `fit_chip`을 버린다.
- 켤 것의 대체 규칙: 줄의 `layers`/`layerid`는 명령행의 `chip`/`chip_off`를, 줄의 `chip`/`chip_off`는
  명령행의 `layers`를 버린다. `layers=`(빈 값)은 기본으로 되돌리고(전부, `--layerprops` 파일이 숨긴 레이어는 뺀다),
  `layers=all`은 숨긴 것까지 전부다(0.12.452).
- 한도: 파일 16 MiB, 4096 샷. 셸 확장은 없다.
- DRC 옵션과는 함께 쓸 수 없다. 보고서는 샷마다 한 행이다.

---

## 3. 네 모서리를 한 장으로: mosaic

```sh
floe2 render deck.jb --corners 40000,80000,60000,95000 --size 4mm,3mm --px 600x450 --out quad.png
floe2 render deck.jb --level 2 --chip MAIN.oas --corners --size 300um,300um --px 800 --out main4.png
floe2 render deck.jb --mosaic-at "X,Y;X,Y;X,Y;X,Y" --size W,H --line 3 --keep-tiles --out quad.png
```

| 옵션 | 기본 | 의미 |
|---|---|---|
| `--corners X0,Y0,X1,Y1` | | 그 영역 **안쪽**의 네 꼭짓점에 `--size` 크기의 타일 넷(tl 타일의 왼쪽 위 = 영역의 왼쪽 위) |
| `--corners` (값 없이, 또는 `fit`) | | 기본 영역의 네 모서리. 기본 영역은 켠 칩의 범위, 칩을 고르지 않았으면 소스 전체, `--fit-chip`이 있으면 그 칩 |
| `--mosaic-at "X,Y;X,Y;X,Y;X,Y"` | | 네 점을 시계 방향으로 tl, tr, br, bl. 각 점은 `--at`처럼 `--size`·`--anchor`로 읽는다 |
| `--size W,H` | 필수 | 타일 하나의 영역 크기 |
| `--line W` | 2 | 구분선 폭 px. 0이면 없음. 이음매 중심으로 `floor(W/2)` px는 양쪽 불투명, 나머지는 양쪽 1 px 혼합 |
| `--line-color RGB` | #ffffff | 구분선 색 |
| `--keep-tiles` | | 네 타일 PNG(`<out>_tl/_tr/_bl/_br.png`)도 남긴다. 모든 렌더가 성공했을 때만 |

- 타일마다 `--px`이고 결과는 가로세로 두 배(tl,tr / bl,br), 최종 이미지는 64 Mpx까지다. 타일이 16 Mpx를 넘으면
  네 타일이 같이 줄어든다(§1.1; WARNING은 `quad.png each tile: …`, 보고서의 `pixel_requested`는 합친 크기).
  `--px`를 생략하면 타일마다 그 종횡비의 가장 큰 크기라 정사각 타일의 최종 이미지는 8192×8192, 64 Mpx다(0.12.433).
- `--legend`는 합친 최종 이미지에 들어간다. 덱이면 네 타일 중 하나에라도 실제로 보이는 줄이다(0.12.439). 보고서에는 `mosaic`과 `tiles`(타일별 bbox)가 남는다.

---

## 4. jobdeck 캡처

케이스별(덱 전체, 레벨, 칩, 배치, CHIP 블록, 위치, 네 모서리, 여러 장, 해상도, 요약, 인덱스 없는 덱)로 필요한
옵션과 확인 결과, 미비한 것을 묶은 정리는 [JOBDECK-CAPTURE.ko.md](JOBDECK-CAPTURE.ko.md)다.

```sh
floe2 info   deck.jb --level 2 --chips                       # 칩 이름, CHIP, 배치 #K와 범위(--bbox/--corners 형식)
floe2 render deck.jb --level 2 --chip MAIN.oas,FRAME.oas --fit-chip MAIN.oas --corners --size 300um,300um --out main4.png
floe2 render deck.jb --level 2 --chip-off 'TEST*' --out level2.png
floe2 render deck.jb --chip '2:*' --fit-chip 'MARK.oas#3' --px 600 --out mark3.png
floe2 render deck.jb --chipnumber 2 --level 1 --bbox 40000,80000,70000,96000 --out block2.png
floe2 render deck.jb --temp-index --level 9 --at 53.02mm,92.61mm --size 2mm,2mm --out spot.png
floe2 render deck.jb --level 2 --mode chip --legend --out level2_chips.png             # chip view: 칩마다 다른 색
```

### 4.1 레벨

`--level N,N`은 그 마스크 레벨만 로드한다. 로드하지 않은 레벨의 소스는 열지 않고 색인도 하지
않는다. 레벨은 `--layers`에서 이름(MTITLE, 없으면 `LEVELn`)으로, `--layerid N`에서 번호로 고른다.
색은 기본이 뷰어의 level view다(레벨마다 한 색). `--mode chip`이면 chip view로 칩마다 다른 색이다(§4.6).

### 4.2 칩 on/off와 칩으로 정하는 영역

- 칩은 칩뷰의 행, 곧 레벨 아래의 소스 하나다(JOBDECK.ko.md §1a). 이름은 셋 중 무엇이든 받는다: 칩뷰가
  보이는 소스 파일 이름, TC 경로, CHIP id(그 블록이 배치하는 칩들). `N:이름`은 레벨 N의 칩만, `*` `?`
  `[..]` 와일드카드(셸에서는 따옴표), 쉼표로 여럿.
- `--chip`은 그 칩들만 켜고, `--chip-off`는 그 칩들만 끈다. 함께 주면 켠 칩에서 끈 칩을 뺀다.
  `--layers`와는 함께 쓸 수 없다.
- 영역을 주지 않으면 켠 칩들의 배치 범위다(덱의 BX..UY를 배치한 값이라 소스를 읽지 않는다).
  `--fit-chip NAME[#K]`는 영역만 그 칩의 배치로 정한다(켜져 있지 않아도 된다). `#K`는 덱 순서(CHIP 블록
  순서, 그 안에서 ROWS 순서)의 K번째 배치 하나다. `floe2 info deck.jb --chips`가 번호와 범위를 보여 준다.
- **칩 좌표로 영역 주기** `--in-chip NAME[#K]`(0.12.435): 영역의 좌표와 길이를 그 배치의 칩 좌표, 곧 소스
  레이아웃의 µm로 읽고, 덱이 그 칩을 놓는 대로 놓는다(`X = dx + mag·x`, `Y = dy + mag·y`, 길이는 mag배;
  미러·회전은 없다). `--bbox`, `--at`, `--size`, `--mosaic-at`, 좌표 `--corners`에 모두 적용되고 `--px`는 픽셀이라
  그대로다. 영역 없이 주면 그 배치 자체가 영역이라 `--fit-chip NAME#K`와 같고, 값 없는 `--corners --size W,H`면
  그 배치의 네 모서리를 칩 µm의 타일 크기로 찍는다. 이름은 `--fit-chip`과 같은 것을 받되 **배치 하나**여야 한다.
  여러 번 놓인 칩은 `#K`를, 레벨마다 다르게(다른 mag나 원점) 놓인 칩은 레벨을 `N:NAME#K`로 준다(안 주면
  `its levels place it differently - level 2 chipB.oas mag 2 at 43020,88000; level 5 … (um); give the level: N:NAME#K`,
  exit 1). 그릴 칩은 바꾸지 않는다. 그 자리의 그 칩만 보려면 `--chip`을 함께 준다. stdout에
  `[floe2] NAME: in chip 2:chipB.oas#1 (CHIP ID002 ROWS 1, level 2, mag 2, origin 43020,88000 um): 100,200,300,400 chip um -> 43220,88400,43620,88800 um`,
  보고서에 `in_chip`(§6)이 남는다. `floe2 info deck.jb --chips --json`의 `chips.rows[].placements[]`가 배치마다
  mag, dx_um, dy_um를 준다.

```sh
floe2 render deck.jb --in-chip 'MAIN.oas#2' --at 1234.5,678.9 --size 100,100 --out spot.png   # 칩 (1234.5, 678.9) 주위 100 µm
floe2 render deck.jb --in-chip '2:MAIN.oas#2' --chip '2:MAIN.oas' --bbox 0,0,1000,1000 --out ll.png
floe2 render deck.jb --in-chip 'ID002#1' --corners --size 50,50 --px 400x400 --out corners.png
```
- 캡처는 **켠 칩**의 건너뛴 배치만 부족한 것으로 센다(WARNING, stderr `rendered incomplete`, `complete=false`; exit는
  0, §0 종료 코드). 꺼진 칩의 skip은 `note:` 한 줄과 보고서의 `jobdeck.skipped_off`에만 남는다.

### 4.3 CHIP 블록으로 자르기: `--chipid` / `--chipnumber`

`floe2 index`와 같은 선택이다. `--chipid ID,…`(정확한 id나 `*` `?` `[..]` 패턴)와 `--chipnumber N,N-M,…`
(덱 순서 1부터, 범위 `N-M`; 둘을 함께 주면 어느 쪽에든 든 블록)이 고른 CHIP 블록의 소스와 배치만 열고
색인하고 그린다. 다른 블록은 로드하지 않은 레벨처럼 격자와 extent에만 참여한다. 덱 전용이고(레이아웃은
2), DRC 캡처와는 함께 쓸 수 없다. 그림은 그 블록만 든 덱을 찍은 것과 같다. 보고서 `jobdeck.chip_blocks`,
stdout `[jobdeck] blocks    : N of M CHIP block(s) placed …`.

### 4.4 캡처 창: 위치 캡처는 그 자리의 소스만 읽는다

영역이 있는 덱 캡처(`--bbox`, `--at`, `--mosaic-at`, 좌표 `--corners`, 또는 `--chip`/`--fit-chip`/`--in-chip`이
정하는 칩 범위)는 샷들의 영역을 덱 µm의 창으로 모아(`--in-chip`의 좌표는 덱 좌표로 놓은 뒤), 그 창에 닿는 배치(덱의 BX..UY를 배치한 extent,
닿기만 해도 포함, BX..UY가 없는 항목은 항상 포함)의 소스만 열고, 자동 색인과 `--temp-index`의 색인·요약도
그 소스만 만든다. 로드한 레벨의 나머지 소스는 그대로 둔다(stderr 색인 줄의 `; K source(s) the capture's
region does not reach are left as they are`). 칩 표·건너뛴 배치·색은 덱 전체를 연 것과 같고 그림은 바이트
동일하다. 영역이 아무 배치에도 닿지 않으면 거절한다(exit 1, `no placement of the levels loaded is in the
capture's region`). 영역 없는 전체 샷과 `info`·`probe`·`clip`·DRC 캡처·뷰어는 창 없이 연다.
`FLOE_DECK_WINDOW=off`면 예전처럼 로드한 레벨의 모든 소스를 연다. 출력은 stdout
`[jobdeck] window    : N of M source(s) read - P of Q placement(s) in the capture's region(s) …`와
보고서 `jobdeck.window`.

### 4.5 광역 캡처와 점유 요약

덱의 광역 pass는 경량 소스의 점유 요약(`design.ovo`)에서 그린다. 요약이 없는 소스의 광역 pass는 비고
`summary_building`으로 세어 불완전으로 알린다(exit 0; 캡처는 한 번 그리고 만다). 길은 셋이다: 미리 `floe2 index deck.jb`
(요약 포함), `render --occupancy on`(요약을 먼저 만든다), `--temp-index`(임시 폴더에 요약을 만들고 다시
그린다). `--occupancy off`면 요약 없이 페이지로 그린다. 근접 캡처는 요약을 쓰지 않는다.

### 4.6 뷰: level / chip / layer 색 (`--mode`)

```sh
floe2 render deck.jb --level 2 --mode chip --legend --out level2_chips.png      # 칩마다 다른 색
floe2 render deck.jb --mode chip --chip '2:*' --fit-chip 'MAIN.oas#1' --out main_chipview.png
floe2 render deck.jb --mode layer --out layers.png                               # 소스 레이어 LY/DT마다 한 색
```

- 기본은 뷰어의 level view다. 마스크 레벨마다 한 색이고 그 레벨의 칩은 모두 그 색이다.
- `--mode chip`은 뷰어의 chip view다(0.12.436). 레벨 아래 소스 칩마다 한 색이다. 팔레트는 뷰어와 같다. 레벨들이
  앞 색을 차지하고 그 뒤를 소스가 덱에 처음 나오는 순서로 받으므로, 어떤 레벨을 로드하든 같은 소스는 같은 색이다
  (한 그림에서 색이 겹쳐 캡처가 바꿔 주는 줄은 예외다: 아래).
- `--mode layer`는 source layer view다. 소스 레이어 LY/DT마다 한 색이다.
- **그림에 칠해지는 줄끼리는 색이 겹치지 않게 한다**(0.12.449; 사용자 "가능하면 칠해진 레이어의 색이 겹치지 않아야
  하는데 현재는 겹치기도 함 … 실제로 칠할때 색을 결정하고 이전에 그린 색과 최대한 겹치지 않게 선택"). 뷰의 팔레트는
  열 색을 차례로 돌려 쓰므로 열한 번째 줄은 첫 줄과 색이 같고, 그 둘이 한 그림에 나오면 범례에서 구별할 수 없었다.
  캡처는 줄의 색을 그 줄이 처음 픽셀을 칠하는 순간에 정한다(renderd `distinct=`).
  - 먼저 칠한 줄 가운데 같은 색이 없으면 뷰의 색 그대로다. 있으면 팔레트에서 아직 안 쓴 색 가운데, 이미 칠한 색들과
    검은 바탕에서 가장 먼 색을 준다(OKLab 거리). 열 색을 다 쓴 뒤에는 예비 색 43개에서 같은 식으로 고른다.
  - 그래서 겹치는 줄이 없는 그림은 0.12.448까지와 픽셀이 같고, 겹치던 줄만 색이 바뀐다. 칠하는 순서는 덱의 배치
    순서(낮은 레벨부터)라 먼저 칠하는 줄이 제 색을 지킨다. 도형이 영역에 없어 칠하지 않는 줄은 색을 차지하지 않는다.
  - 뷰가 일부러 같은 색으로 그리는 줄은 그대로 한 색이다: level view의 레벨과 그 칩들, chip view에서 여러 레벨에
    놓인 같은 소스, layer view에서 같은 LY의 DT들. 갈라 주는 것은 팔레트가 한 바퀴 돌아 우연히 같아진 색이다.
  - layerprops로 뷰의 색과 다르게 준 색은 그 줄의 것으로 두고, 다른 줄에는 주지 않는다. 뷰의 색 그대로 저장된 줄은
    여느 줄과 같다.
  - mosaic의 네 타일은 한 그림이다. 뒤 타일은 앞 타일이 정한 색을 이어받는다.
  - `--legend`와 무관하다: 범례가 있든 없든 픽셀이 같고, 범례의 색은 칠한 색이다. 보고서의 샷 행 `colors`가 칠한
    줄과 색을, `recolored`가 뷰의 색과 달라진 줄을 말하고(§5), 달라진 줄이 있으면 `note:` 한 줄이 난다(§6).
  - 뷰어의 화면은 그대로다(뷰의 색). 같은 영역이라도 캡처와 뷰어의 색이 다를 수 있고, 영역이 다르면 같은 줄이 다른
    색일 수 있다. 줄과 색의 대응은 그 그림의 범례나 보고서로 본다. `FLOE_DECK_DISTINCT_COLORS=off`면 뷰의 색 그대로
    찍는다(0.12.448까지의 동작).
  - 한 그림에 줄이 2천 개쯤을 넘는 mosaic은 뒤 타일에 넘길 색 목록이 명령 한 줄에 들어가지 않아 뷰의 색으로 찍고
    `note: … too many rows are placed in the picture …`로 알린다.
- 그 뷰에서 저장한 layerprops가 적용된다. level view는 `<deck>.jb.layerprops`, chip view는
  `<deck>.chip-by-level.jb.layerprops`, layer view는 `<deck>.layer.jb.layerprops`다. 키 공간이 다르므로 서로 섞이지
  않는다.
- `--layerprops FILE`(0.12.452, §1.3)의 파일은 찍는 뷰의 줄로 읽어 그 뷰의 저장 파일 위에 덮는다.
  - level view: `N.0`이 레벨 N이다. 레벨의 칩은 레벨의 색을 따르고, 레벨을 숨기면 그 칩들이 빠진다.
  - chip view: `N.D`가 칩 줄이다. `N.0`은 그 레벨의 묶음이라 채움·선폭·표시 여부가 자기 줄이 없는 칩에 내려가고,
    색은 어느 칩의 것도 아니다. 칩 줄의 표시 여부가 묶음의 것보다 먼저다.
  - 뷰어에서 저장한 파일이 그 뷰의 줄을 그대로 담고 있으므로, level view에서 저장한 파일은 `--mode level`에, chip
    view에서 저장한 파일은 `--mode chip`에 준다.
- `--legend`는 그 뷰가 보여 주는 줄을 적는다. level view는 레벨 줄, chip view는 칩 줄(색, 채움, 파일 이름, `L/D`)이고
  레벨 머리 줄은 적지 않는다(그 색으로 그려진 것이 없다). `--chip`으로 고르면 켠 칩의 줄만 적는다(0.12.436; 전에는
  로드한 레벨 전부).
- 그림에 실제로 보이는 줄만 적는다(0.12.439; 사용자 "실제 그려진 것만 레전드를 추가하도록"). 렌더러가 프레임을
  합성하면서 픽셀마다 마지막으로 칠한 줄을 기억했다가, 픽셀이 하나라도 남은 줄을 프레임과 함께 알려 준다(renderd
  `drawn=`). 그래서 배치는 영역에 닿지만 그 자리에 그 레이어의 도형이 없는 칩, 위에 그린 줄에 전부 덮인 줄, 요약을
  기다리느라 비워 둔 pass의 줄은 적지 않는다. 셀 프레임 선은 줄의 색이 아니므로 세지 않는다. 레벨 줄은 그 레벨의 칩
  가운데 하나라도 보일 때 적는다. mosaic은 네 타일을 합쳐 본다. 보이는 줄이 없으면 범례가 없고
  `note: … no layer to list - no legend`가 난다. 이 판정에 렌더러가 프레임을 그리는 동안 픽셀당 4바이트를 더 쓴다
  (16 Mpx 프레임이면 64 MB). `--legend`를 준 덱 캡처에서만이다.
- `FLOE_LEGEND_IN_VIEW=placed`면 0.12.438의 규칙이다: 틀 안에 배치가 있는 줄(덱의 BX..UY 범위가 영역에 닿으면 포함,
  모서리만 닿아도, 범위를 모르는 배치는 늘; 캡처 창 §4.4의 판정). `off`면 로드한 레벨의 줄 전부다(0.12.437까지).
  레이아웃 캡처의 범례는 이 규칙 밖이다: 전처럼 무언가 든 레이어를 모두 적는다.
- `--chip`, `--chip-off`, `--fit-chip`, `--in-chip`, `--layerid`, `--layer`는 뷰와 무관하게 같다. 켜고 끄는 단위는
  어느 뷰에서나 레벨 아래의 소스 칩이다.
- 뷰는 한 실행에 하나다. batch 키가 아니므로 level view와 chip view를 함께 찍으려면 두 번 돌린다.
- 보고서 `jobdeck.view`가 `level|chip|layer`를 말한다. 레이아웃에는 `--mode`가 없다(인수 오류, exit 2).

---

## 5. 보고서 JSON (`--report FILE`)

맨 위:

| 키 | 뜻 |
|---|---|
| `source`, `dbu` | 소스 경로와 DBU |
| `shots[]` | 샷마다 한 행(아래) |
| `cut_px`, `thin`, `density`, `occupancy`, `layer_colors`, `legend`, `archival` | 그림 모양. `occupancy`는 말하지 않았으면 null, `layer_colors`는 덱이면 null |
| `distinct_colors` | 덱의 칠해지는 줄끼리 색이 겹치지 않게 했는가(0.12.449; §4.6). 레이아웃과 `FLOE_DECK_DISTINCT_COLORS=off`는 false |
| `layerprops` | `--layerprops`의 파일(0.12.452; 주지 않았으면 null): `file`(읽은 파일의 실제 경로), `rows`(파일의 줄 수), `matched`(그 가운데 소스에 있는 레이어의 줄 수), `hidden[]`(파일이 숨긴 줄의 `L/D`; 덱의 level view는 숨긴 레벨의 칩 줄). 샷 행의 `layers`가 null인 샷에서 이 줄들이 빠졌다 |
| `complete` | 모든 샷이 완전한가 |
| `jobdeck` | 덱만: `complete`, `skipped[]`(캡처가 빠뜨린 배치), `skipped_off[]`(꺼진 칩의 skip), `over_budget_pages`, `summary_building`, `view`(`--mode`의 뷰: level|chip|layer), `levels`, `chip_blocks`(`--chipid`/`--chipnumber`의 `n:ID` 목록, 없으면 null), `window`(`rects_um`, `sources`, `sources_total`, `placements`, `placements_total`; 창이 없으면 null) |

샷 행:

| 키 | 뜻 |
|---|---|
| `name`, `out`, `pixel` | 샷 이름, 출력 경로, [W,H](mosaic은 합친 크기) |
| `ppu`, `ppu_y` | 배율 px/um, 가로와 세로(0.12.433). 렌더러는 축마다 따로 맞추므로 `--stretch`가 아니면 둘은 한 픽셀 안에서 같다. mosaic은 타일의 배율이라 구분선을 건너는 거리에는 `line_pixels`만큼 어긋난다. `--legend`의 메타데이터 `ppu`는 가로 값이다 |
| `pixel_requested` | 16 Mpx 상한에 걸려 줄였을 때만: 요청한 [W,H](mosaic은 합친 크기; §1.1) |
| `bbox_um` | 단일 샷의 영역. mosaic은 대신 `mosaic`(width, height, tile, line, line_pixels, line_color)과 `tiles`(tl/tr/bl/br의 bbox) |
| `layers`, `depth` | 그린 레이어 지정과 깊이. `layers`는 말하지 않았으면 null, `--layers all`이면 `"all"`(0.12.452; 전에는 null) |
| `ms` | 걸린 시간 ms |
| `complete`, `skipped_placements`, `over_budget_pages`, `summary_building` | 빠진 것의 수 |
| `budget_fit` | 예산에 맞춰 성기게 그렸을 때만(renderd의 `fit_*`; mosaic은 타일별) |
| `chips` | 덱에서 칩을 골랐을 때: `on`(`N:이름`), `chip`, `chip_off`, `fit_chip`, `region_um`(칩으로 정한 영역), `placements` |
| `in_chip` | `--in-chip`을 줬을 때(0.12.435): `chip`(준 이름), `chip_id`, `rows`(ROWS 번호), `n`(#K), `tc`, `levels`, `mag`, `dx_um`, `dy_um`, `extent_um`(그 배치의 덱 범위), 그린 영역을 칩 좌표로 되돌린 `bbox_chip_um`(mosaic은 `tiles_chip_um`) |
| `legend[]` | `--legend`의 줄들 |
| `colors` | 덱(`distinct_colors`가 true일 때): 픽셀을 칠한 줄과 그 색, `{"L/D": "#rrggbb"}`(0.12.449). 키는 범례의 `L/D`와 같다(level view는 레벨 `N/0`, chip view는 칩 줄). 위에 그린 줄에 전부 덮여 범례에서 빠진 줄도 들어 있다 |
| `recolored[]` | 덱: `colors` 가운데 뷰의 색과 다른 색으로 칠한 줄의 `L/D`. 겹치는 색이 없었으면 빈 배열 |

보고서도 출력이라 실패하면 `PNG was saved; report failed: …`로 끝난다(PNG는 남는다).

---

## 6. stdout과 stderr의 줄

stdout:

```
[floe2] rendered a.png (800x600, 0.0000,0.0000,2000.0000,1500.0000 um) in 0.42s
[floe2] rendered quad.png (1200x900 mosaic of 4 x 600x450, line 1 solid on each side) in 1.10s
[floe2] WARNING: a.png: 1280x13440 px for 40.0000x420.0000 um (0.0000,0.0000,40.0000,420.0000 um) exceeds 16 Mpx - drawn at 1264x13272 (FLOE_CAPTURE_REDUCE=off refuses it)
[floe2] WARNING: DRC frame 5000x5000 px exceeds 16 Mpx - drawn at 4096x4096 (FLOE_CAPTURE_REDUCE=off refuses it)
[floe2] WARNING: a.png thinned to fit the render budget: …; FLOE_RUST_BUDGET_MB raises it
[floe2] WARNING: a.png stopped at the page budget: N page(s) not drawn
[floe2] WARNING: N jobdeck placement(s) not drawn: …
[floe2] note: N placement(s) the load skips are in chips no shot had on: …
[floe2] note: a.png: no layer to list - no legend
[floe2] note: a.png: 2 of the 12 row(s) painted drawn in another colour than the view's - a row painted before had theirs (FLOE_DECK_DISTINCT_COLORS=off keeps the view's)
[floe2] note: a.png: too many rows are placed in the picture to tell them apart by colour - drawn in the view's colours
[floe2] layer properties: /p/a.layerprops - 3 of its 4 row(s) for layers of this source, 1 hidden (left out where a shot names no layers; --layers all draws every layer)
[floe2] WARNING: layer properties file /p/b.layerprops names no layer of /d/layout.oas (1 row(s)) - drawn by the defaults
[floe2] WARNING: layer properties file /p/c.layerprops hides every layer of this source - a shot that names no layers draws none (--layers / --layerid name them, --layers all is every layer)
[floe2] report shots/report.json (3 shot(s), INCOMPLETE)
[jobdeck] blocks    : 1 of 3 CHIP block(s) placed (--chipid / --chipnumber): 2:ID002
[jobdeck] window    : 1 of 3 source(s) read - 2 of 17 placement(s) in the capture's region(s) (FLOE_DECK_WINDOW=off reads them all)
[jobdeck] skipped   : CHIP ID001 level 2 chipA.oas: reason (detail)
```

stderr:

```
[jobdeck] 2 source(s) have no index yet - indexing them first …; 1 source(s) the capture's region does not reach are left as they are
[floe2][warn] source changed; displaying cached geometry. Rebuild with index --force, then reopen.
[jobdeck] 1 pass(es) of light source(s) wait for their occupancy summary - building the missing summaries in the temporary index and drawing again (--occupancy off: the pages instead)
floe2: WARNING: rendered incomplete - 0 jobdeck placement(s) missing, 0 page(s) over the decode budget, summary_building=1 … (see above; exit 0 - FLOE_RENDER_INCOMPLETE_EXIT=3 exits 3 as before)
floe2: indexing SOURCE first failed (exit 71 …; see above)
floe2: layer properties file not found: /p/nope.layerprops
floe2: layer properties file /p/bad.layerprops: not a text file (UTF-8)
floe2: frame 1280x13440 px for 40.0000x420.0000 um (0.0000,0.0000,40.0000,420.0000 um) exceeds 16 Mpx; --px 1264x13272 fits (FLOE_CAPTURE_REDUCE=off: refused, not reduced) [FLOE_CAPTURE_REDUCE=off, exit 1]
```

---

## 7. 환경 변수

| 변수 | 기본 | 뜻 |
|---|---|---|
| `FLOE_AUTO_INDEX` | on | `off`면 인덱스 없는 소스를 거부(`--no-index`) |
| `FLOE_RENDER_INCOMPLETE_EXIT` | 0 | 그림은 나왔지만 빠진 것이 있는 캡처(켠 칩의 건너뛴 배치, 예산 초과 페이지, `summary_building`)의 종료 코드. 기본 0이고 메시지와 보고서 `complete=false`로 알린다(0.12.437). `3`이면 0.12.436까지처럼 3으로 끝난다(1..125) |
| `FLOE_WORKDIR` | 없음(소스 옆) | 캐시·pack·사이드카를 읽고 쓰는 작업 디렉터리(`--workdir DIR`; §1.5). floe2가 자식 프로세스에 내보내는 것이기도 하다 |
| `FLOE_INDEX_LIGHT` | 덱 on, 레이아웃 off | 자동 색인이 만드는 캐시의 종류(`--light`/`--full`) |
| `FLOE_LOCK_WAIT` | off | `on`이나 초: 색인 중인 소스를 기다린다(`--wait`) |
| `FLOE_DECK_WINDOW` | on | `off`면 덱 캡처가 로드한 레벨의 모든 소스를 연다(§4.4) |
| `FLOE_LEGEND_IN_VIEW` | drawn | 덱 캡처의 `--legend`가 적는 줄. 기본은 그림에 실제로 보이는 줄(0.12.439). `placed`면 틀 안에 배치가 있는 줄(0.12.438의 규칙), `off`면 로드한 레벨의 줄 전부(0.12.437까지의 동작)(§4.6) |
| `FLOE_DECK_DISTINCT_COLORS` | on | 덱 캡처가 칠해지는 줄끼리 색이 겹치지 않게 색을 정한다(0.12.449; §4.6). `off`면 뷰의 색 그대로(0.12.448까지의 동작) |
| `FLOE_CAPTURE_PX` | max | `--px`를 생략했을 때의 크기. `--px`와 같은 `W`·`WxH`·`max`(§1.1). `1200`이면 0.12.432까지의 기본. DRC 스냅샷의 기본 1200에는 적용되지 않는다 |
| `FLOE_CAPTURE_REDUCE` | on | `off`면 16 Mpx를 넘는 프레임을 줄이지 않고 거부한다(§1.1; DRC 스냅샷도) |
| `FLOE_RENDER_DEFAULTS` | | `archival`이면 보관용 모양이 기본(0.12.349까지의 동작) |
| `FLOE_LAYER_COLORS` | row | `--layer-colors`의 기본 |
| `FLOE_RUST_DENSITY_STACK` | | `off`면 뷰어와 캡처의 밀도 기본이 off |
| `FLOE_RUST_OCCUPANCY` | | `--occupancy`를 말하지 않았을 때의 렌더러 규칙을 바꾼다: `on`이면 레이아웃도 요약으로 그리고, `off`면 덱도 요약을 쓰지 않는다 |
| `FLOE_RUST_BUDGET_MB` | 1024 | 디코드 예산 MiB(1..1048576). 뷰어와 같다 |
| `FLOE_RUST_JOBS` | min(CPU, 8) | 렌더러의 디코드 워커 수 1..256 |
| `FLOE_RUST_OPEN_TIMEOUT_S` | 300 | 소스 열기 제한 초(1..86400) |
| `FLOE_RENDER_TIMEOUT_S` | 없음(일주일) | 한 프레임을 기다리는 상한 초 |
| `FLOE_ARTIFACT_MODE` | 660 | 출력 파일 권한(8진수, 소유자 rw 필수). `600`은 비공개, `664`는 그 외에 읽기 |
| `FLOE_RSS_CEILING` | 90 | 자동 색인의 메모리 상한(MemTotal의 %, 또는 `300G`, `off`). 넘으면 색인이 71로 멈추고 캡처는 1로 끝난다 |
| `FLOE_INDEX_BIN`, `FLOE_RENDERD_BIN` | 옆의 바이너리 | `floe-index`·`floe-renderd`의 위치를 바꿀 때만 |
| `FLOE_REVIEWER` | | DRC 캡처의 리뷰어 태그(`--floe-reviewer`) |
| `FLOE_REVIEWER_REQUIRED` | off | `1`·`on`·`yes`·`true`면 리뷰어 ID를 명시해야 한다(0.12.440). `--floe-reviewer`나 `FLOE_REVIEWER` 없이 리뷰어의 waive를 읽는 `render --drc`·`floe2 drc`는 1로 끝난다(DRC-CLI.ko.md) |

---

## 8. 관련 명령

- **`floe2 info SOURCE [--level N] [--chips] [--json]`**: 소스 요약. 덱의 `--chips`는 로드한 레벨의 칩마다
  CHIP 블록, 배치 `#K`, 범위를 `--bbox`/`--corners` 형식으로 보여 준다(`--chip`/`--fit-chip`/`--in-chip`이 받는 이름).
  `--json`은 메타데이터와 `source_stale`. `--json --chips`는 JSON 문서 하나로 `chips`를 함께 낸다(0.12.435; 전에는
  표가 JSON 뒤에 텍스트로 붙었다): `levels`와 `rows[]`(`level`, `level_name`, `key`, `name`, `source`, `chips`(CHIP id),
  `extent_um`, `placements[]`: `n`, `chip`, `rows`, `at_um`, `bbox_um`, `mag`, `dx_um`, `dy_um`). 배치마다 한 장 찍는
  batch 줄을 만들 때 `jq`로 읽는다. `--no-index`·`--temp-index`·`--light|--full`·`--wait`는 render와 같다.
- **`floe2 index SOURCE [--light|--full] [--occupancy] [--level N] [--chipid|--chipnumber] [--force]`**: 캡처
  전에 캐시를 만들어 두는 길. 덱은 레벨이나 CHIP 블록 단위로 나눠 돌릴 수 있다.
- **`floe2 fe-embed [OPTIONS] PNG…`**: 찍은 PNG에 주석을 넣거나 읽는다. 픽셀은 건드리지 않고 flateyes
  메타데이터(iTXt)만 바꾼다. `--dump`는 들어 있는 메타데이터(범례 포함)를 찍고, `--strip`은 지운다.
  `--box`/`--ellipse`/`--line`/`--path`/`--polygon`/`--ruler`/`--text`/`--note`/`--legend FILE`/`--json FILE`은
  주석을 더하고(`--append`면 있는 것 뒤에), `--ppu N --unit U`는 축척이다. 좌표는 이미지 **중심** 원점의
  픽셀(x 오른쪽, y 아래)이다. 파일마다 원자적으로 바꾼다.
- **`floe2 clip SOURCE --bbox X0,Y0,X1,Y1 [--layers SPEC] [--out clip.oas] [--cell-name NAME]`**: 영역의
  도형을 OASIS로 정확히(cut 0, 전체 깊이) 잘라 낸다. 레이아웃 전용이다(덱은 소스 레이아웃을 직접 자른다).
  전체 인덱스를 쓴다(경량 캐시는 거부). `--no-index`·`--temp-index`·`--wait`는 render와 같고,
  `FLOE_RUST_CLIP_TIMEOUT_S`(300)가 한 clip의 상한이다.
- **DRC 에러 스냅샷**: `floe2 render SRC --drc results.db --drc-rule RULE [--drc-err N|A-B|all] …`.
  영역·batch·mosaic·report·`--chipid`와는 함께 쓸 수 없다. [DRC-CLI.ko.md](DRC-CLI.ko.md) §3.
- **뷰어**: `floe2 view SOURCE --goto X,Y[,W] [--level N] [--detail …] [--depth N]`은 그 점을 중심으로
  (W는 뷰 폭 µm) 연다. 캡처의 기본 모양은 이 화면이다. 뷰어 안에서는 Ctrl+C가 화면을 클립보드에
  복사한다(SPEC-VIEWER.ko.md §1).

---

## 9. 예시: 한 레벨의 위치들을 스크립트로

```sh
#!/bin/sh
# 레벨 9의 세 위치를 1200 px로. 인덱스가 없으면 임시 폴더에 만들고 지운다.
DECK=deck.jb
cat > shots.txt <<'SHOTS'
left    at=20mm,40mm size=2mm,2mm
center  at=50mm,40mm size=2mm,2mm
right   at=80mm,40mm size=2mm,2mm
SHOTS
floe2 render "$DECK" --level 9 --temp-index --px 1200 --batch shots.txt --out shots/ --report shots/report.json
case $? in
  0) echo complete ;;
  3) echo "incomplete - see shots/report.json" ;;
  *) echo "failed ($?)" ;;
esac
```

csh/tcsh 호스트에서는 `(floe2 render … > render.out) >& render.err`처럼 stdout과 stderr를 나누고
`$status`로 종료 코드를 본다.

---

## 10. 함께 쓸 수 없는 것

- `--bbox` / `--at` / `--mosaic-at` / `--corners`는 하나만. `--fit-chip`은 좌표 영역과 함께 쓸 수 없다
  (값 없는 `--corners`와는 된다). `--in-chip`은 `--fit-chip`과 함께 쓸 수 없다(둘 다 기본 영역을 정한다).
- `--layers`(`--layer`, `--layerid`)와 `--chip`/`--chip-off`는 하나만.
- `--density on`은 `--detail exact`·덱과, `--occupancy on`은 `--detail exact`와 함께 쓸 수 없다.
- `--layer-colors`, `--density`, `--labels on`은 덱에서 거부한다. `--level`, `--mode`, `--chip*`, `--fit-chip`, `--in-chip`,
  `--chipid`, `--chipnumber`는 덱에서만 받는다.
- `--temp-index`는 `--no-index`와, DRC 옵션은 영역·batch·mosaic·report·`--chipid`/`--chipnumber`와 함께 쓸 수 없다.
- mosaic에는 `--size`가 필요하다.

---

## 11. 문서 지도

| 문서 | 내용 |
|---|---|
| README.md "floe2 사용법" | 명령 예시 묶음, 캡처 기본값 = 뷰어, 디코드 예산, `--density`, `--legend`의 결정 |
| [JOBDECK.ko.md](JOBDECK.ko.md) §7 | `floe2 render`의 M4 이력: 영역·mosaic·batch 규칙, 칩 on/off, 임시 인덱스, 캡처 창 |
| [JOBDECK-CAPTURE.ko.md](JOBDECK-CAPTURE.ko.md) | 덱 캡처를 케이스별로: 필요한 옵션, 확인 결과, 미비와 주의 (2026-11-09) |
| [DRC-CLI.ko.md](DRC-CLI.ko.md) §3 | DRC 에러 스냅샷 |
| [DIRECT_READ_PLAN.ko.md](DIRECT_READ_PLAN.ko.md) §8.11, §8.29, §8.30 | 캡처는 요약을 만들지 않는다, 임시 인덱스, 캡처 창의 측정과 판정 |
| [CACHE-NAMING.ko.md](CACHE-NAMING.ko.md) | 캐시·pack·사이드카의 이름과 잠금, `--workdir`의 작업 디렉터리(§6) |
| [SHARED_APP_LAYER.ko.md](SHARED_APP_LAYER.ko.md) §12 | 읽기 명령의 자동 색인과 대기, 캡처 창의 배관 |
| [SPEC-VIEWER.ko.md](SPEC-VIEWER.ko.md) | 뷰어의 키(Ctrl+C 클립보드 복사)와 렌더 파이프라인 |
| [OCCUPANCY_PLAN.ko.md](OCCUPANCY_PLAN.ko.md), [CUT_DENSITY_DESIGN.ko.md](CUT_DENSITY_DESIGN.ko.md) | 점유 요약과 컷 아래 밀도의 설계 |
