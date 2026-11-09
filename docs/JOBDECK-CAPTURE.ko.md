# jobdeck 캡처: 케이스별 옵션 정리

`floe2 render DECK.jb`로 jobdeck을 찍을 때 쓰임새(케이스)마다 어떤 옵션을 주면 되는지, 그 옵션이 지금
준비되어 있는지 정리한 문서다. 2026-11-09에 release 0.12.433 바이너리(floe-index/renderd 0.12.370)와
jobdeck 게이트의 합성 덱(`tools/validate_jobdeck.py`의 test.jb: 소스 3개, CHIP 블록 3개, 레벨 4개, 배치 17개)으로
케이스마다 명령을 실행해 종료 코드·그림 크기·보고서를 확인했다. 옵션 하나하나의 뜻과 문법은
[CAPTURE-CLI.ko.md](CAPTURE-CLI.ko.md), 결정의 이력은 [JOBDECK.ko.md](JOBDECK.ko.md) §7이다.

## 0. 한눈에

| 케이스 | 옵션 | 상태 |
|---|---|---|
| 덱 전체 한 장 (§2) | 영역 없이. 요약은 `floe2 index DECK` 미리, 또는 `--occupancy on`, 또는 `--temp-index` | 준비 |
| 16 Mpx보다 크게 (§2) | `--corners --size W/2,H/2 --px max` (2×2 mosaic, 64 Mpx까지) | 준비, 2×2까지 |
| 레벨 하나·여럿 (§3) | `--level N[,N]` + `--layerid N[,N]` 또는 `--layer NAME` | 준비 |
| 칩 하나·여럿, 칩 on/off (§4) | `--chip NAME[,…]`, `--chip-off NAME`, `N:NAME`, 와일드카드 | 준비 |
| 배치 하나, 반복 중 K번째 (§5) | `--fit-chip NAME#K`, 번호는 `info --chips` | 준비 |
| CHIP 블록 단위 (§6) | `--chipid ID`, `--chipnumber N,N-M` (`index`와 같은 선택) | 준비 |
| 마스크 좌표의 위치 (§7) | `--at X,Y --size W,H [--anchor]`, `--bbox`, 단위 접미사 | 준비 |
| 칩(소스) 좌표의 위치 (§8) | `--in-chip NAME[#K]` + `--bbox`/`--at --size`/`--corners`/`--mosaic-at` (0.12.435) | 준비 |
| 칩의 네 모서리 (§9) | `--fit-chip NAME --corners --size W,H`, `--mosaic-at`, `--line`, `--keep-tiles` | 준비 |
| 여러 장 한 번에: 칩마다·레벨마다·위치마다 (§10) | `--batch FILE --out DIR --report FILE`, 줄은 `info --chips --json`에서 만든다 | 준비, 줄 생성은 스크립트 |
| 해상도 (§11) | `--px W\|WxH\|max` (기본 max), `--stretch`, `FLOE_CAPTURE_PX` | 준비 |
| 그림 모양 (§12) | `--detail`, `--thin`, `--archival`, `--depth` + `--frames`, `--legend`, `<deck>.jb.layerprops` | 준비, 덱 제한 있음 |
| 광역과 점유 요약 (§13) | `--occupancy on\|off` | 준비, `--workdir`는 주의 |
| 인덱스 없는 덱, 쓸 수 없는 폴더 (§14) | `--temp-index[=DIR]`, `--workdir DIR`, `--light\|--full`, `--wait`, `--no-index` | 준비 |
| 누락·불완전 처리 (§15) | exit 3, WARNING, 보고서 `jobdeck.skipped` | 준비 |
| 보고서·메타데이터 (§16) | `--report FILE` (ppu, chips, window …), `--legend` (ppu, unit) | 준비 |

미비와 주의 목록은 §17.

## 1. 전제

- **좌표**는 덱 µm다(ROWS와 BX..UY를 배치한 값, `floe2 info DECK`의 `bbox um`). 길이에 `nm`/`um`/`mm`
  접미사를 붙일 수 있고 없으면 µm다.
- **소스**는 평문 OASIS만 읽는다. `.gds`, `.oas.gz` 같은 소스의 배치는 `unsupported`로 건너뛰고 캡처는 exit 3이다(§15).
- **인덱스**: 소스마다 경량 캐시(`.<src>.ice`: design.inv + design.seg)와 점유 요약(design.ovo)이 있어야 광역이
  그려진다. `floe2 index DECK.jb`가 모두 만든다(`--level`, `--chipid`, `--chipnumber`로 나눠 돌릴 수 있다). 없는 채로
  찍으면 자동 색인하되 요약은 `--occupancy on`일 때만 만든다(§13).
- **색**은 뷰어의 level view다: 레벨마다 한 색(MTITLE 순). 칩마다 다른 색(chip view)으로는 찍을 수 없다(§17).
- **종료 코드**: 0 완료. 1 입력 오류(없는 칩·레벨, 닿는 배치 없음, 색인 안 된 소스). 2 인수 오류(덱이 받지 않는
  옵션 등). 3 불완전(건너뛴 배치, 요약 대기, 예산 초과: 그림은 나오되 불완전으로 표시된다). 75 다른 실행이 색인 중
  (`--wait`로 기다린다). 71 색인의 메모리 상한.

## 2. 덱 전체 한 장

```sh
floe2 index  deck.jb                          # 소스 경량 캐시 + 점유 요약 (한 번)
floe2 render deck.jb --out deck.png           # 전 레벨, 덱 extent, 16 Mpx 안의 가장 큰 크기
floe2 render deck.jb --px 1200 --out small.png
```

- 영역을 주지 않으면 덱의 extent(모든 배치의 BX..UY)다. `--px`를 주지 않으면 그 종횡비로 16 Mpx 안에 드는
  가장 큰 크기다(32×25.2 mm 덱이면 4617×3633).
- 전체는 광역 pass라 소스의 점유 요약에서 그린다. 요약이 없는 소스의 pass는 `summary_building`으로 세어
  exit 3이다(§13).
- 16 Mpx보다 크게 찍으려면 영역을 넷으로 나눈 2×2 mosaic이다. 타일마다 16 Mpx, 합쳐 64 Mpx까지.

```sh
# 32000×25180 µm 덱을 빈틈없이 넷으로: 타일 4617×3633, 결과 9234×7266
floe2 render deck.jb --corners --size 16000,12590 --px max --line 0 --out deck_big.png
```

extent는 `floe2 info deck.jb`의 `bbox um`에서 읽는다. 더 잘게(3×3 이상) 나누는 옵션은 없다. 필요하면 `--batch`에
타일의 bbox를 줄로 쓴다(§10).

## 3. 레벨 하나·여럿

```sh
floe2 render deck.jb --level 2 --out level2.png                  # 레벨 2만 로드하고 그린다
floe2 render deck.jb --level 1,3 --layerid 1,3 --out l13.png     # 두 레벨
floe2 render deck.jb --layer METAL1 --out metal1.png             # 이름(MTITLE)으로. 전 레벨을 로드한다
```

- `--level N`은 **로드**를 그 레벨로 좁힌다. 다른 레벨의 소스는 열지도 색인하지도 않고, 영역도 그 레벨의 배치
  범위다. 합성 덱에서 `--level 2`는 소스 2개를, `--layerid 2`만은 3개를 열었다. 한 레벨만 필요하면 `--level`을 같이 준다.
- `--layerid N`은 레벨 번호, `--layer NAME`은 MTITLE(없으면 `LEVELn`)이다. `--layers 'level 2'`는 거절하고
  `--layerid 2`를 알린다(exit 1). 로드하지 않은 레벨을 `--layerid`로 고르면 `unknown deck layer: "N/0"`(exit 1)이다.
- 덱에 없는 레벨은 `level(s) 9 not in the deck (it places 1,2,3,5)`로 거절한다(exit 1).

## 4. 칩 하나·여럿, 칩 on/off

```sh
floe2 info   deck.jb --chips                                     # 칩 이름, CHIP, 배치 #K, 범위
floe2 render deck.jb --chip MAIN.oas --out main.png              # 그 칩만 켜고, 영역은 그 배치들의 범위
floe2 render deck.jb --level 2 --chip MAIN.oas --out main2.png   # 레벨 2의 그 칩만
floe2 render deck.jb --chip '2:MAIN.oas,FRAME.oas' --out two.png
floe2 render deck.jb --chip-off 'TEST*' --out no_test.png        # 그 칩들만 끄고 나머지
floe2 render deck.jb --chip ID002 --out block2_chips.png         # CHIP id: 그 블록이 배치하는 칩들
```

- 칩은 칩뷰의 행, 곧 레벨 아래의 소스 하나다. 이름은 소스 파일 이름, TC 경로, CHIP id 중 무엇이든 되고,
  `N:이름`은 레벨 N의 것만, `*` `?` `[..]`는 와일드카드다(셸에서 따옴표).
- 같은 소스가 여러 레벨에 있으면 `--chip MAIN.oas`는 그 레벨들을 모두 켠다(보고서 `chips.on`에 `1:MAIN.oas`,
  `2:MAIN.oas`). 한 레벨만이면 `--level N`이나 `N:`이다.
- 영역을 주지 않으면 켠 칩들의 배치 범위다(소스를 읽지 않고 덱의 BX..UY에서). 영역을 따로 주면 그 안의 켠 칩만 그린다.
- `--layers`/`--layerid`와 `--chip`/`--chip-off`는 함께 쓸 수 없다(exit 1). 없는 이름은 로드한 레벨의 칩 목록과
  CHIP id를 들어 거절한다.
- 색은 레벨 색 그대로다. 칩마다 다른 색이 아니다.

## 5. 배치 하나, 반복 중 K번째

```sh
floe2 info   deck.jb --chips          # "#3 CHIP ID002 ROWS 1 at 45020,90000: 41020,84900,49020,95100 um"
floe2 render deck.jb --fit-chip 'MAIN.oas#3' --out main_3.png
floe2 render deck.jb --fit-chip 'ID002#2' --out id002_2.png      # CHIP id면 그 블록의 배치 중 2번째
floe2 render deck.jb --fit-chip 'ID001#1' --chip '1:MAIN.oas' --out only.png   # 그 자리의 그 칩만
```

- `--fit-chip`은 **영역만** 정한다(그 칩이 꺼져 있어도 된다). `#K`는 덱 순서(CHIP 블록 순, 그 안의 ROWS 순)의
  K번째 배치 하나다.
- 같은 자리에 겹친 다른 칩(같은 레벨의 다른 소스, 다른 레벨)도 그려진다. 그 칩만 보려면 `--chip`을 같이 준다.

## 6. CHIP 블록 단위

```sh
floe2 index  deck.jb --chipnumber 1-4                # 블록 1~4의 소스만 색인 (분할·병렬 실행)
floe2 render deck.jb --chipid ID001 --out id001.png
floe2 render deck.jb --chipnumber 2-3 --level 1 --out b23.png
floe2 render deck.jb --chipid ID002 --chip FRAME.oas --out b2_frame.png   # 블록을 열고, 그 중 한 칩만 켠다
```

- 그 블록의 소스와 배치만 열고 색인하고 그린다. 다른 블록은 격자와 extent에만 참여한다. 그림은 그 블록만 든 덱과 같다.
- `--chipid`는 정확한 id나 `*` `?` `[..]` 패턴, `--chipnumber`는 덱 순서 1부터(범위 `N-M`). 둘을 주면 어느 쪽에든
  든 블록이다. stdout `[jobdeck] blocks    : 1 of 3 CHIP block(s) placed (--chipid / --chipnumber): 1:ID001`,
  보고서 `jobdeck.chip_blocks`.
- `--chip`과 다르다. `--chipid`/`--chipnumber`는 무엇을 **여는지**, `--chip`은 연 것 중 무엇을 **켜는지**다.
  함께 쓸 수 있다.

## 7. 마스크 좌표의 위치

```sh
floe2 render deck.jb --at 53.02mm,92.61mm --size 2mm,2mm --out spot.png
floe2 render deck.jb --at 40000,82000 --size 32000,24000 --anchor lb --px 1200x900 --out b.png
floe2 render deck.jb --bbox 41020,80020,49020,90220 --out box.png
floe2 render deck.jb --level 9 --at 53.02mm,92.61mm --size 2mm,2mm --out spot9.png   # 그 레벨만
```

- 좌표는 덱 µm다. `--anchor center`(기본)는 `--at`이 중심, `lb`는 좌하 꼭짓점이다.
- 영역이 있는 캡처는 **캡처 창**이다. 그 창에 닿는 배치의 소스만 열고 색인한다(stdout
  `[jobdeck] window    : 2 of 3 source(s) read - 5 of 17 placement(s) in the capture's region(s)`, 보고서
  `jobdeck.window`). 닿는 배치가 없으면 exit 1 `no placement of the levels loaded is in the capture's region`이고
  빈 그림을 내지 않는다.
- `--px WxH`는 영역을 그 종횡비로 넓힌다. `--stretch`면 넓히지 않고 눌러 그린다.

## 8. 칩(소스) 좌표의 위치: `--in-chip`

칩 안의 설계 좌표 (x, y)를 바로 받는다(0.12.435). `--in-chip NAME[#K]`를 주면 영역의 좌표와 길이를 그 배치의 칩
좌표, 곧 소스 레이아웃의 µm로 읽고, 덱이 그 칩을 놓는 대로 놓는다.

```
X_deck = dx + mag · x_chip        Y_deck = dy + mag · y_chip        W_deck = mag · w_chip
```

```sh
floe2 render deck.jb --in-chip 'MAIN.oas#2' --at 1234.5,678.9 --size 100,100 --out spot.png    # 칩 (1234.5, 678.9) 주위 100 µm
floe2 render deck.jb --in-chip '2:MAIN.oas#2' --bbox 0,0,1000,1000 --chip '2:MAIN.oas' --out ll.png   # 그 자리의 그 칩만
floe2 render deck.jb --in-chip 'ID002#1' --corners --size 50,50 --px 400x400 --out corners.png  # 배치의 네 모서리, 타일 50 칩 µm
floe2 render deck.jb --in-chip 'ID002#1' --out whole.png                                       # 영역 없이: 그 배치 자체 (= --fit-chip)
```

- `--bbox`, `--at`, `--size`, `--mosaic-at`, 좌표 `--corners`에 모두 적용된다. `--px`, `--line`은 픽셀이라 그대로다.
- 이름은 `--fit-chip`과 같은 것(소스 파일 이름, TC 경로, CHIP id, `N:` 접두, 와일드카드)을 받되 **배치 하나**여야
  한다. 여러 번 놓인 칩은 `#K`(덱 순서, `info --chips`의 번호)를 주지 않으면 `--in-chip takes one placement - it has 3
  (#1..#3 …): NAME#K`로 거절한다(exit 1). 같은 칩을 레벨마다 다른 mag나 원점으로 놓은 덱이면 레벨을 `N:NAME#K`로
  준다. 안 주면 `its levels place it differently - level 2 chipB.oas mag 2 at 43020,88000; level 5 chipB.oas mag 4 at
  43020,88000 (um); give the level: N:NAME#K`로 거절한다. 레벨들이 같게 놓으면(필드 덱은 mag 4 하나) 레벨 없이 된다.
- 그릴 칩은 바꾸지 않는다(그 자리에 겹친 다른 레벨도 그려진다). 그 칩만 보려면 `--chip`을 함께 준다. `--fit-chip`과는
  함께 쓸 수 없다.
- 캡처 창(§7)은 덱 좌표로 놓은 영역에 적용된다. 영역이 아무 배치에도 닿지 않으면 §7과 같이 거절한다.
- stdout `[floe2] NAME: in chip 2:chipB.oas#1 (CHIP ID002 ROWS 1, level 2, mag 2, origin 43020,88000 um):
  100,200,300,400 chip um -> 43220,88400,43620,88800 um`. 보고서 샷 항목 `in_chip`: `chip`, `chip_id`, `rows`, `n`, `tc`,
  `levels`, `mag`, `dx_um`, `dy_um`, `extent_um`, 그린 영역을 칩 좌표로 되돌린 `bbox_chip_um`(mosaic은 `tiles_chip_um`).
- batch(§10): 명령행의 `--in-chip`은 모든 줄의 좌표에 적용되고, 줄의 `in_chip=NAME#K`가 틀을 바꾸며 `in_chip=`은 덱
  좌표로 되돌린다.
- 미러·회전 필드는 덱 포맷에서 본 적이 없어 식에 없다(JOBDECK.ko.md §1). 배치마다의 mag, dx, dy는
  `floe2 info deck.jb --chips --json`의 `chips.rows[].placements[]`나 `floe2 jobdeck deck.jb --placements`가 준다.
- 확인(합성 덱, 2026-11-09): `--in-chip 2:chipB.oas#1 --bbox 100,200,300,400`이 `--bbox 43220,88400,43620,88800`과
  바이트 동일, `--at 250,250 --size 100,100`이 덱 `--at 43520,88500 --size 200,200`과 동일, 영역 없는 `--in-chip
  2:chipB.oas#2`가 `--fit-chip 2:chipB.oas#2`와 동일. 게이트 `jobdeck`의 ChipShotTests가 같은 것을 지킨다.

## 9. 칩의 네 모서리

```sh
floe2 render deck.jb --fit-chip MAIN.oas --corners --size 300um,300um --px 800 --out main4.png
floe2 render deck.jb --level 2 --chip MAIN.oas --corners --size 300um,300um --line 3 --keep-tiles --out main4.png
floe2 render deck.jb --corners 40000,80020,60000,95000 --size 4mm,3mm --px 600x450 --out quad.png
floe2 render deck.jb --mosaic-at "X,Y;X,Y;X,Y;X,Y" --size W,H --out quad.png   # 네 점 직접 (TL,TR,BR,BL)
```

- 값 없는 `--corners`(또는 `fit`)는 기본 영역(켠 칩의 범위, 칩을 고르지 않았으면 덱 전체)의 네 모서리 안쪽에
  `--size` 사각형 넷을 놓는다. 결과는 가로세로 2배의 한 장(tl,tr / bl,br)이고, `--line W`가 구분선(`0`은 없음),
  `--keep-tiles`가 `_tl/_tr/_bl/_br.png`를 남긴다. 보고서 `shots[].tiles`에 타일 넷의 bbox가 든다.
- `--size`는 필수다. 좌표를 준 `--corners`는 `--fit-chip`과 함께 쓸 수 없다(값 없는 것은 된다).

## 10. 여러 장 한 번에

```sh
cat > shots.txt <<'SHOTS'
# NAME key=value ...  키: bbox at size anchor px stretch layer layerid depth mosaic corners line linecolor keep_tiles chip chip_off fit_chip in_chip
main        chip=MAIN.oas px=1600
main_3      fit_chip=MAIN.oas#3 px=max
spot        in_chip=MAIN.oas#3 at=1234.5,678.9 size=100,100 px=800
align_box   layerid=3 bbox=41020,80020,49020,90220 px=600
corners     layerid=1 corners=fit size=1000,1000 px=400
SHOTS
floe2 render deck.jb --batch shots.txt --out shots/ --report shots/report.json
```

- 덱을 한 번 열고 모두 찍는다. 줄의 영역 키는 명령행의 영역을, 줄의 `layer`/`layerid`는 명령행의 `--chip`을(그
  반대도) 대신한다. `level`은 줄의 키가 아니다. 로드는 명령행 `--level`로 한 번 정한다.
- 이름은 유일해야 한다(`batch shot names must be unique`, exit 1).
- **칩마다·배치마다 한 장**은 `info --chips --json`에서 줄을 만든다(자동으로 만드는 옵션은 없다, §17). 0.12.435부터
  `--json --chips`가 JSON 문서 하나로 `chips.rows[].placements[]`(`n`, `chip`, `rows`, `bbox_um`, `mag`, `dx_um`,
  `dy_um`)를 주므로 `jq`로 읽는다.

```sh
floe2 info deck.jb --chips --json | jq -r '.chips.rows[] as $r | $r.placements[]
  | "L\($r.level)_\($r.name)_\(.chip)_\(.n) bbox=\(.bbox_um | map(tostring) | join(",")) px=1200"' > perchip.txt
floe2 render deck.jb --batch perchip.txt --out perchip/ --report perchip/report.json
```

합성 덱의 17 배치가 17장이 됐다. 레벨마다 한 장은 `layerid=N` 줄 하나씩이다. `jq`가 없으면 `--chips`의 텍스트를
`awk`로 읽을 수 있다. 배치 줄은 `#K CHIP ID ROWS n at X,Y: X0,Y0,X1,Y1 um` 형식이다(배치가 둘 이상인 칩에만 나온다).

## 11. 해상도

- `--px W`: 폭, 높이는 영역 종횡비에서. `--px WxH`: 둘 다(영역을 그 종횡비로 넓힌다. `--stretch`면 넓히지 않는다).
  `--px max`와 생략: 영역 종횡비로 16 Mpx 안의 가장 큰 크기(정사각이면 4096×4096). `FLOE_CAPTURE_PX=1200`이면
  생략값이 0.12.432까지의 1200이다.
- 16 Mpx를 넘는 요청은 가장 큰 크기로 줄이고 WARNING과 보고서 `pixel_requested`로 알린다. 더 크게는 2×2 mosaic
  64 Mpx다(§2).
- 축척은 보고서의 `ppu`(x)와 `ppu_y`(y, `--stretch`가 아니면 같다), `--legend` 메타데이터의 `ppu=`와 `unit=um`이다.

## 12. 그림 모양

- 기본은 뷰어가 그 영역에서 보여 주는 그림이다(detail medium, thin keep). `--detail exact|low|medium|high`,
  `--thin auto|keep|cull`, `--archival`(컷 0, solid 채움, 프레임 없음)은 덱에서도 그대로다.
- `--frames`는 덱에서 `--depth N` 컷 아래의 셀에만 나타난다. 기본(full depth)에서는 컷 아래가 없어 프레임도 없다.
  `--labels on`은 거절한다(`jobdeck labels are not supported`, exit 2).
- `--density`와 `--layer-colors`는 레이아웃용이라 덱에서 거절한다(exit 2).
- 뷰어의 level view에서 저장한 `<deck>.jb.layerprops`의 색·채움·선폭은 캡처에도 적용된다. 숨김(visibility 0)은
  적용되지 않으니 숨길 레벨은 `--layerid`나 `--chip-off`로 뺀다.
- `--legend`는 그린 레벨의 범례(색, 채움, 이름, N/0)와 축척(`ppu`, `unit`)을 flateyes 메타데이터로 넣는다.
  `floe2 fe-embed --dump a.png`로 읽는다. 픽셀은 그대로다.

## 13. 광역과 점유 요약

덱의 광역 pass(컷 아래 도형이 많은 넓은 영역)는 소스의 점유 요약(design.ovo)에서 그린다. 근접 캡처는 요약을 쓰지
않는다.

| 상태 | 결과 |
|---|---|
| `floe2 index deck.jb`를 미리 했다 | 광역 완료, exit 0 |
| 색인 없이 `render` (자동 색인) | 경량 캐시는 만들되 요약은 만들지 않아 광역은 exit 3, `summary_building=N pass(es)` |
| `render --occupancy on` | 요약을 먼저 만들고 그린다, exit 0 |
| `render --temp-index` | 임시 폴더에 요약까지 만들고 다시 그린다, exit 0 (끝나면 지운다) |
| `render --workdir DIR`만 | 자동 색인과 같아 광역은 exit 3. `--occupancy on`을 함께 주거나 `index deck.jb --workdir DIR`을 먼저 |
| `render --occupancy off` | 요약 없이 페이지로 그린다(느리지만 완전), exit 0 |

합성 덱 측정: `--workdir`만의 광역은 exit 3, `--workdir --occupancy on`은 exit 0이고 DIR 아래에 design.ovo 셋이
생겼고, 그 뒤 `--workdir`만으로도 exit 0이었다.

## 14. 인덱스 없는 덱, 쓸 수 없는 소스 폴더

```sh
floe2 render deck.jb --temp-index --at 53.02mm,92.61mm --size 2mm,2mm --out spot.png   # 소스 옆에 아무것도 남기지 않음
floe2 render deck.jb --workdir /work/floe --occupancy on --out deck.png                 # 캐시를 /work/floe 아래에 남김
setenv FLOE_WORKDIR /work/floe                                                           # csh: 모든 명령에
floe2 render deck.jb --no-index --out x.png                                              # 색인 안 된 소스가 있으면 거절
```

- `--temp-index[=DIR]`: 이 실행만의 폴더에 캐시(광역이면 요약도)를 만들고 끝나면 지운다. 한 번뿐일 때. 캡처 창 덕에
  영역에 닿는 소스만 색인한다.
- `--workdir DIR`(`FLOE_WORKDIR`): 소스의 절대 경로 자리(DIR/a/b/.x.oas.ice)에 캐시를 두고 다음 실행이 다시 쓴다.
  읽기 전용 소스 폴더의 정석이다. 광역은 §13.
- 소스 폴더에 쓸 수 없으면 `cannot write beside x.oas, in /proj/a: ... --workdir DIR (or FLOE_WORKDIR) ...`로
  거절한다(exit 1).
- `--light`/`--full`: 자동 색인이 만드는 캐시 종류(덱 소스는 기본 경량). `--wait[=S]`: 다른 실행이 색인 중인 소스를
  기다린다(없으면 exit 75).
- `--no-index`로 거절되는 덱의 메시지는 `no placement can be drawn (empty selection or no fresh source/layer cache)`로,
  어느 소스가 색인되지 않았는지 말하지 않는다(레이아웃은 `run floe2 index X first`라고 한다). §17.

## 15. 누락·불완전

- 켠 칩의 배치가 그려지지 않으면(소스 없음, 지원하지 않는 포맷, 레이어 없음) WARNING에 배치마다 사유를 적고 그림은
  불완전으로 표시해 내며 exit 3이다. 보고서 `complete=false`, `jobdeck.skipped[]`(chip, idx, tc, reason, detail,
  anchors). 꺼진 칩의 누락은 `note:` 한 줄과 `jobdeck.skipped_off`에만 남고 exit 0이다.
- 디코드 예산을 넘긴 페이지는 `over_budget_pages`, 요약 대기는 `summary_building`으로 센다(exit 3).
- 지원 포맷: 소스는 평문 OASIS다. `.gds`, `.gz`는 `unsupported`(detail `floe-index reads plain OASIS only`)다.

## 16. 보고서와 stdout

`--report FILE`의 샷 항목: `bbox_um`, `pixel`, `ppu`, `ppu_y`, `layers`, `depth`, `chips`(chip, chip_off, fit_chip, on,
placements, region_um), `in_chip`(§8), `tiles`(mosaic), `skipped_placements`, `summary_building`, `over_budget_pages`, `complete`,
`ms`, `out`. 덱 항목 `jobdeck`: `levels`, `view`(레벨 색), `window`, `chip_blocks`, `skipped`, `skipped_off`,
`complete`. 상위: `source`, `dbu`, `cut_px`, `thin`, `density`, `occupancy`, `archival`, `legend`, `layer_colors`,
`complete`.

stdout 줄: `[jobdeck] window    : …`, `[jobdeck] blocks    : …`, `[floe2] NAME: region X0,Y0,X1,Y1 um (--fit-chip …)`,
`[floe2] NAME: in chip NAME#K (CHIP …, level N, mag M, origin X,Y um): … chip um -> … um`,
`[floe2] rendered FILE (WxH, bbox um) in T s`, `[floe2] WARNING: N jobdeck placement(s) not drawn: …`. 불완전이면
마지막 줄이 `floe2: rendered incomplete - …  [exit 3]`이다.

## 17. 미비와 주의 (2026-11-09)

없는 것(우회해야 한다):

1. ~~**칩 좌표 입력**~~: 0.12.435의 `--in-chip NAME[#K]`(§8)로 해결.
2. **칩마다·레벨마다 자동으로 한 장씩**: §10의 batch 줄 생성(`info --chips --json` + `jq`).
3. ~~**`info --json --chips`**~~: 0.12.435부터 JSON 문서 하나에 `chips`가 든다(§10). 전에는 JSON 뒤에 표가 텍스트로
   붙었고 JSON 안에 칩이 없었다.
4. **칩뷰 색**(칩마다 다른 색): 캡처는 레벨 색만이다.
5. **미러·회전·톤 반전**: 옵션이 없다. 덱 포맷에서 해당 필드를 본 적이 없다(JOBDECK.ko.md §1 미확정).
6. **눈에 보이는 스케일바·격자**: 없다. 축척은 메타데이터(`ppu`)이고, `fe-embed --ruler`도 메타데이터 주석이다.
7. **2×2보다 잘게**: mosaic은 네 타일까지다(§2).

주의(동작은 하되 알아 둘 것):

8. `--chip`으로 고른 캡처의 `--legend`는 켠 칩의 레벨만이 아니라 **로드한 레벨 전부**를 적는다(`--layerid`로
   고르면 그 레벨만).
9. `<deck>.jb.layerprops`의 숨김(visibility 0)은 캡처에 적용되지 않는다(§12).
10. `--frames`는 `--depth N` 컷 아래에만 나타나고, `--labels on`, `--density`, `--layer-colors`는 덱에서 거절한다(exit 2).
11. `--layerid N`만 주면 전 레벨을 로드한다. `--level N`을 같이 준다(§3).
12. `--workdir`만으로 광역을 찍으면 exit 3이다(§13).
13. `--no-index` 덱의 거절 메시지가 원인을 말하지 않는다(§14).
14. `--layers`와 `--chip`의 충돌은 인수 충돌이지만 exit 1이다.

## 18. 확인 기록

2026-11-09, floe2 0.12.433(floe-index/renderd 0.12.370), 합성 덱 test.jb(`tools/validate_jobdeck.py`가 만드는 것)에
위 케이스의 명령 70개 남짓을 release 바이너리로 실행해 종료 코드·그림 크기·보고서·메타데이터를 확인했다.
같은 규칙을 게이트 `jobdeck`(ShotTests, ChipShotTests, LightSummaryTests)과 `floe2`(validate_floe2.py)가 지킨다.
2026-11-09 0.12.435: §8의 `--in-chip`과 §10의 `info --chips --json`을 더해 같은 덱으로 확인했다(ChipShotTests의
`test_in_chip_reads_the_region_in_the_placements_coordinates`, `test_info_json_carries_the_chips`).
