# floe

대용량 OASIS 설계 파일을 빠르게 조회/클립하는 경량 유틸리티.

수십~수백 GB의 원본 설계 파일을 매번 전체 로딩하지 않고,

- 특정 **레이어**만 추출하거나
- 특정 **영역(bbox)** 만 클립해서 별도 OASIS 파일로 저장
- Calibre DRC 결과 DB(RDB)를 참조해 **에러 영역을 조회/분석**

하는 것을 목표로 한다. 타겟 환경은 Linux, 개발/테스트는 macOS.

## 환경 설정

엔진은 KLayout pip 모듈(`klayout.db`/`klayout.lay` 헤드리스), GUI 셸은
시스템 PyGObject/GTK3. **`gi`(PyGObject)는 pip으로 설치하지 않는다** —
OS 패키지로 설치하고, venv를 그 gi가 보이는 파이썬으로
`--system-site-packages` 옵션과 함께 만든다. CLI 전용
(`index/info/render/clip`)이면 GTK 없이 klayout+numpy만으로 동작한다.

Python 3.14 + klayout 0.30.9 확인됨.

### macOS (개발/테스트)

```sh
brew install pygobject3 gtk+3 librsvg adwaita-icon-theme

# pygobject3의 gi가 어느 brew 파이썬 버전에 들어갔는지 확인 (예: python3.14)
ls /opt/homebrew/lib | grep python

# 반드시 그 버전의 Homebrew 파이썬으로 venv 생성
/opt/homebrew/bin/python3.14 -m venv --system-site-packages .venv
.venv/bin/pip install klayout numpy
.venv/bin/python -c "import gi; gi.require_version('Gtk', '3.0'); print(gi.__file__)"
```

- Xcode CLT의 `/usr/bin/python3`, python.org 설치본, pyenv 등 다른 파이썬으로
  venv를 만들면 `--system-site-packages`여도 brew의 gi가 보이지 않아
  `No module named 'gi'` 가 난다. venv의 `pyvenv.cfg`에서
  `home = /opt/homebrew/opt/python@3.XX/bin` 인지 확인할 것.
- Intel 맥은 Homebrew 경로가 `/opt/homebrew` 대신 `/usr/local`.
- `librsvg`가 없으면 GTK 심볼릭 아이콘(check-symbolic.svg) 로드 경고가 뜬다.

### Linux (인터넷 가능 호스트)

```sh
# PyGObject/GTK3는 OS 패키지로 (RHEL 계열 GNOME 데스크톱엔 기본 탑재)
sudo dnf install python3-gobject gtk3      # Debian/Ubuntu: python3-gi gir1.2-gtk-3.0

python3 -c 'import gi; gi.require_version("Gtk", "3.0")'   # 사전 확인
python3 -m venv --system-site-packages .venv
.venv/bin/pip install klayout numpy
```

폐쇄망 호스트는 아래 [폐쇄망 리눅스 배포](#폐쇄망-리눅스-배포) 절차를 따른다.

## 테스트 데이터 생성

실제 설계 파일이 없으므로, 실제와 유사한 구조의 대용량 OASIS를 합성 생성한다.

```sh
.venv/bin/python tools/gen_test_oasis.py --out data/testchip_1g5.oas --target-gb 1.5
```

생성되는 레이아웃 구조:

- `FLOE_TESTCHIP` (top) → 6×6 블록 그리드 (블록 1.5mm, die ~10.1mm)
- 로직 블록: 스탠다드셀 라이브러리 12종 인스턴스 row 배치 + M2~M6 랜덤 라우팅/비아
- SRAM 블록 2개 (모서리): 1024×2048 비트셀 정규 어레이 (OASIS repetition 압축 테스트용)
- **더미 메탈필** (M1~M6, datatype 1): 랜덤 사각형 — 파일 용량의 대부분을 차지
- 마커 (layer 63/63): 알려진 좌표의 십자 + 텍스트 라벨 → clip 결과 검증용

`<out>.manifest.json`에 die/블록/마커 좌표, 레이어별 shape 수 등 ground truth가
기록되므로 이후 clip/추출 툴의 자동 테스트 기준으로 사용한다.

생성 파라미터: `--target-gb`(목표 크기, 실측 bytes/shape로 캘리브레이션),
`--seed`, `--grid`(블록 그리드), `--jobs`(병렬 워커 수).

### 반복(repetition) 폭발 스트레스 파일 — 운영 호스트 로딩 병리 재현

운영 파일의 병리(작은 파일 → editable 읽기에서 수억 shape로 구체화)를
재현하는 합성 파일. 파일 크기와 펼침 크기를 독립적으로 지정한다.

```sh
# 파일 ~30MB, editable 읽기 시 ~15GB (도형당 ~46B 기준 약 3.5억 개)
.venv/bin/python tools/gen_stress.py data/stress30.oas --file-mb 30 --expand-gb 15

# 1/10 축소판으로 빠른 확인
.venv/bin/python tools/gen_stress.py midi.oas --scale 0.1
```

- `--file-mb`: 진짜 난수 이산 사각형(개당 ~9B, deflate 무력화)으로 파일 부피 구성
- `--expand-gb`: atom 그리드 flatten → OASIS writer가 repetition 레코드로
  압축 → editable 읽기에서 전부 구체화(개당 ~46B RAM)
- 생성 자체는 블록별 임시 파일 + viewer 모드 병합이라 RAM ~2GB로 동작
- 결과 파일은 shapes/byte가 높아 floe가 자동으로 viewer 모드를 선택함.
  병리 자체를 보려면 `--layout-mode editable`로 강제.
  (`floe index`도 v0.4.3부터 기본 viewer read — `--read-mode editable`이
  펼침 RAM을 요구하는 옛 경로.)

## 레이어 맵

| GDS layer/dt | 이름 | 비고 |
|---|---|---|
| 0/0 | BOUNDARY | die 경계 |
| 1/0~4/0 | NWELL, ACTIVE, POLY, CONT | FEOL |
| 5/0~15/0 (홀수 진행) | M1~M6, VIA1~VIA5 | BEOL |
| 5/1, 7/1, 9/1, 11/1, 13/1, 15/1 | M*_FILL | 더미필 (용량 대부분) |
| 63/63 | MARKER | 검증용 마커/라벨 |

## floe 사용법

OASIS에는 공간 인덱스가 없어 어떤 조회든 파일 전체 파싱이 필요하다.
floe는 **최초 1회 인덱싱**으로 이 비용을 지불하고, 이후 모든 조회는
관심 영역과 교차하는 타일만 로딩해 ms~초 단위로 응답한다.

```sh
alias floe=".venv/bin/python -m floe"

floe index data/testchip_1g5.oas          # 1회: <src>.ice/ 생성 (전 코어 사용)
floe index data/testchip_1g5.oas --jobs 1 # 병렬 끄기 (기본: 코어 수만큼 fork)
# 대용량 실칩 인덱싱은 Rust 인덱서 권장 - 아래 [Rust 인덱서] 섹션
# (python 인덱서는 동결 상태, 같은 .ice를 훨씬 빠르게 만든다)
floe info  data/testchip_1g5.oas          # 레이어/그리드/통계 요약
floe view  data/testchip_1g5.oas          # 네이티브 데스크톱 뷰어 (기본)
floe view  data/testchip_1g5.oas --goto 5240,5260,50   # 시작 위치+뷰 폭(um)
floe render data/testchip_1g5.oas --bbox 5000,5000,5200,5200 \
          --layers M2,M3,VIA2 --out view.png
floe clip  data/testchip_1g5.oas --bbox 5000,5000,5100,5100 --out region.oas
```

- `--bbox`는 µm 단위 `X0,Y0,X1,Y1`. `--layers`는 이름 또는 `layer/datatype` 목록.
- **인덱스 read는 기본 viewer 모드** (v0.4.3): klayout editable 모드는
  반복(repetition) 배열을 읽으면서 멤버 전부를 개별 객체로 펼친다
  (~46B/멤버 — 운영 호스트에서 9.83GB 파일이 **400GB RSS**까지 관측).
  viewer 모드는 배열을 압축 상태로 유지해 stress30 실측 기준 read
  **27배 빠르고 RSS 3.9배 절약**, `clip_into` 결과·밴드 파일 내용·렌더
  픽셀 완전 동일 검증. `--read-mode editable`로 이전 동작 복원 가능
  (flat 소스는 editable read가 ~3배 빠름).
  read 중에는 1분마다 `[index] reading... N GB RSS (Ss)` 하트비트가
  나온다 (별도 프로세스라 C++ read가 GIL을 잡고 있어도 동작).
  read 이후의 무음 구간도 진행이 보인다: 레이어 카운트와 텍스트 레이어
  탐지는 셀 단일 스캔으로 합쳐져 `scanning layers... cell N/M`,
  텍스트 수집은 `collecting texts... N found` + RSS 하트비트를 낸다.
- **텍스트 = 광역 스켈레톤 라벨 전용** (v0.5.4): 뷰어의 라이브 렌더는
  텍스트를 그리지 않으므로(`text-visible=false` — floe 자체의 klayout
  렌더 설정) 텍스트의 유일한 소비처는 far view 스켈레톤 라벨이다.
  타일(밴드 파일)에는 텍스트가 아예 실리지 않는다 — clip_into가 실어
  나르는 원본 텍스트도 viewer-safe strip으로 제거된다 (pick 대상에서
  텍스트 제외, clip 반출물에도 미포함; 원본 텍스트가 필요하면
  `clip --exact`).
- **수집은 무인·유계, 예산은 스켈레톤 캡에서 역산** (v0.6.2): 양산
  파일은 도형마다 붙는 마커 텍스트가 수십억 개일 수 있다 (호스트
  실측 17억 개/754GB RSS). 텍스트의 유일한 소비자가 스켈레톤 라벨
  (`--skel-texts`, 기본 5만)이므로 수집 예산도 거기서 나온다:
  레이어당 몫 = 4×캡 ÷ 텍스트 레이어 수, 초과 레이어는 **영역별
  (≤16×16) bbox 질의**로 몫÷영역 수 개씩 **공간 균일하게** 표본화
  (질의는 캡에서 조기 중단되어 작업량이 텍스트 수와 무관). 카운트와
  수집 모두 **fork 워커로 병렬**(레이어·영역 단위, 소스는 COW 공유,
  결과는 튜플 직렬화 후 정렬해 결정적). 타일(1,600개) 단위 순차
  질의였을 때는 질의당 계층 하강 비용만으로 텍스트 단계가 3,583초
  실측 — 영역 축소 + 병렬화로 분 단위 이하가 목표. 예산 도입 전에는
  타일당 1만 고정이라 레이어당 1,600만 개(~10GB, ~5분)를 모았다가
  5만으로 버리는 낭비도 있었다. `--text-cap` / `--text-tile-cap`을
  명시하면 그 값이 우선. 솎임은 로그 + meta(`texts_thinned`) +
  `floe info`에 표시.
- **텍스트 정책 변경은 재인덱싱 없이**: `floe index <src> --texts-only`
  — 구버전 캐시의 b0 잔존 텍스트 제거 + 스켈레톤 라벨 재생성만 수행
  (b1~b3 무변경). 캡 옵션과 조합해 수십 분짜리 조정으로 끝난다.
- **밴드 파티션 v2** (v0.5.0): 타일 클립도 viewer 모드로 수행 —
  어레이 소스의 `clip_into`가 ~100배 빨라진다 (stress30 실측 20.9→0.2s;
  `--tile-tgt editable`로 이전 동작 복원). 셀×레이어 컨테이너마다
  최대 256멤버를 샘플해 **단일 크기(uniform) 컨테이너**(필/어레이 —
  실제 양산 데이터의 대부분)를 µs에 판별하고, uniform이면 Region 전개
  없이 `Shapes.insert(Shapes)` 레코드째 복사한다 (배열은 배열로, 박스는
  박스로 보존 — 시간·메모리 거의 0, 균일 필 벤치 RAM 절반). 혼합
  컨테이너만 기존 Region bbox 필터 경로(폴리곤 변환, 지오메트리 동일)를
  탄다. uniform 멤버가 다수면 밴드 레이아웃도 viewer 모드로 만들어
  레코드 보존을 유지하고, 혼합 다수면 editable로 만들어 flat 폴리곤
  누적 페널티(적대적 타일 실측 RAM +45%)를 피한다 (타일별 자동 선택).
  산출물은 검증 완료: midi 80/80 밴드·LOD 파일 지오메트리 완전 일치,
  전체 배터리(픽셀/XOR/snap/pick/clip) 통과. 캐시 포맷 불변.
- **폐쇄망 성능 재현** (`profile` + `tools/gen_from_profile.py`): 원본
  OAS를 반출할 수 없을 때, 폐쇄망에서 `floe profile chip.oas --out
  prof.json` 으로 **구조 프로파일**(타일·레이어·depth별 도형 수, 계층/
  어레이 통계, 파일 크기 — 좌표·지오메트리 없음, `--anon`이면 레이어
  이름도 익명화)을 뽑아 나온 JSON만 갖고 나온다. 외부에서
  `python tools/gen_from_profile.py prof.json --out sample.oas` 로
  렌더 성능이 유사한 가짜 레이아웃을 합성해 인덱싱·뷰어 벤치마크를
  재현한다 (`--scale 0.1` = 축소판). auto depth 선택과 타일 로딩·렌더
  비용을 결정하는 density 테이블을 그대로 복제하는 방식.
- `clip --exact`: 원본 파일을 다시 파싱하는 느린 경로 (타일 경계에서 잘리지 않은
  원본 geometry가 필요할 때).
- 네이티브 뷰어 조작 (Calibre DESIGNrev 방식):
  **좌드래그 = 영역 줌** (정방향 →: 박스 영역으로 줌인, 역방향 ←: 박스/화면
  비율만큼 줌아웃 — 역방향일 때 밴드가 주황색으로 표시),
  **좌클릭 = object picking**, **중/우클릭 드래그 = 팬**,
  휠·트랙패드 = 커서 기준 줌(보조), `f` fit.

### Ruler (거리 측정, flateyes 기반 UX + 벡터 스냅)

- `r` 측정 모드 시작/종료. 두 점을 클릭하면 측정선이 확정되어 남는다.
  flateyes와 달리 **측정선은 하나만 유지** — 새 측정을 시작하면 이전
  선은 사라진다. 기본은 수평/수직 축 스냅(우세한 축 자동 선택),
  `Shift` = 자유 각도.
- **벡터 스냅** (`m` 토글, 기본 켜짐): flateyes의 이미지 밝기 스냅 대신
  OASIS geometry의 **꼭짓점/엣지에 정확히 붙는다** — 꼭짓점 우선, 없으면
  엣지 위 최근접점. 커서 주변 스냅 위치가 십자 마커로 미리 표시된다
  (꼭짓점=녹색, 엣지=파랑). 확대(live) 상태에서 동작.
- 측정 중 상태바에 실시간 길이/dx/dy 표시. `Esc` 는 단계적으로 해제:
  진행 중 점 → 선택 → 측정 모드 → 확정된 측정선 → goto 마커.

### Object picking (Calibre 방식)

- 확대(live) 상태에서 **좌클릭**: 클릭 지점을 포함하는 도형 중 **가장 작은
  것**이 선택되어 흰 외곽선으로 하이라이트되고, 상태바에 레이어 이름
  layer/datatype · 셀 이름 · 크기 · 좌표 · (순번/전체) 가 표시된다.
- **같은 지점을 다시 클릭하면 겹친 도형들을 차례로 순환** (fill 아래의
  배선, 그 아래의 boundary 순). `Esc` 해제. (텍스트는 타일에 실리지
  않아 선택 대상이 아니다 — v0.5.4, 텍스트 절 참조.)
- 보이는(켜진) 레이어만 대상. 타일 경계에서 잘린 도형은 잘린 조각
  기준으로 선택된다 (셀 이름에 `$1` 변형 표기 가능).
  줌아웃 상태에서는 인덱싱 때 만든 **skeleton**(블록 아웃라인+이름, 대형
  배선/스트랩, 라벨만 담은 수 MB짜리 구조 모델)을 라이브 렌더링해 어떤
  배율에서도 선명한 플로어플랜을 보여주고, 확대하면 뷰포트와 교차하는
  타일만 lazy 로딩해 실시간 렌더링한다 (LRU 캐시로 메모리 상한 유지 →
  원본 크기와 무관하게 동작). 새 프레임 도착 전에는 이전 프레임이
  원래 배율 그대로 고정 표시된다.
- **딱 뷰포트만 렌더** (v0.6.3): 예전에는 각 방향 50% 마진을 더해
  그려뒀지만(마진 내 팬을 재렌더 없이 처리할 목적), 실사용에서 팬은
  어차피 재렌더를 유발하므로 마진은 프레임당 픽셀 4배 + 걸리는 타일
  수만 늘리는 비용이었다 — 제거. 배율·depth·레이어가 그대로인 동일
  뷰는 여전히 다시 그리지 않고, 팬 중에는 이전 프레임이 고정 표시된다.
- 구버전 캐시에는 `floe index --skeleton-only` 로 skeleton만 추가할 수
  있다 (원본 1회 읽기, 재타일링 없음).

### Goto (좌표 이동, Calibre 방식)

- `g` → goto 다이얼로그 (모달, 부모 창 중앙). **x / y / window** 를 um 단위로
  입력하고 Enter 또는 ok — 해당 좌표로 이동하고 다이얼로그는 닫힌다. 그
  지점에 **X 마커**가 표시된다 (렌더 도착 전에도 고정 프레임 위에 유지).
  `Esc`(또는 close)로 적용 없이 닫고, 뷰의 `Esc` 로 마커 삭제.
- x/y는 현재 화면 중심으로 프리필. DRC 리포트의 `"x, y"` 쌍을 한 필드에
  통째로 붙여넣어도 된다 — 값은 x, y, window 순으로 채워지고, 쌍을
  붙여넣으면 남아 있는 y 프리필은 무시된다.
- **window** = 이동 후 뷰 폭(um). 비우면 현재 배율 유지. die 밖 좌표는
  뷰가 경계에 클램프되지만 마커는 요청한 지점에 남는다.
- **CLI**: `floe view <src> --goto X,Y[,W]` (um) — 시작과 동시에 해당
  위치를 보여준다 (W 생략 시 fit 배율). 이미 창이 떠 있으면 forward 되어
  실행 중인 창이 그 위치로 점프한다. DRC 리포트 좌표를 셸에서 바로
  넘길 때 사용.

### DRC 결과 브라우저 (Calibre RVE 방식)

Calibre가 `DRC RESULTS DATABASE "out.db" ASCII`로 쓰는 **ASCII 결과
데이터베이스(.db)** 를 읽어 룰별 에러 목록을 보여주고 클릭으로 점프한다.

- `e` → **DRC results** 창 (비모달 — 열어둔 채 뷰 조작 가능).
  `open .db…` 로 파일 선택, 또는 시작 시 `floe view <src> --drc out.db`.
- 트리: 룰 체크(이름 + 룰 텍스트 + 에러 수) → 에러(#번호, poly/edge,
  중심 좌표). 룰 행 더블클릭 = 펼침/접힘, **에러 행 더블클릭 = 점프**.
- 점프: 에러 bbox 중심으로 이동, 뷰 폭 = 에러 크기의 8배(최소 2µm).
  에러 도형이 **빨간 외곽선**으로 표시된다 (poly = 닫힌 폴리곤,
  edge = 선분 + 끝점 표시). `Esc` 로 마커 삭제.
- **`n` / `p`** = 다음/이전 에러로 순차 이동 (창의 prev/next 버튼과 동일,
  전체 에러를 룰 순서로 순회하며 트리 선택도 따라온다).
- 파서(`floe/drc.py`)는 순수 파이썬으로 포맷 오차에 관대하다: CRLF/빈 줄
  무시, 선언 카운트는 참고만, 모르는 레코드 문자는 건너뜀, 잘린 파일은
  파싱된 부분까지 사용. 포맷: `p <n> <꼭짓점수>` + 줄당 `x y`,
  `e <n> <엣지수>` + 줄당 `x1 y1 x2 y2` (정수, um = 값/precision).
- **CLI**: `floe drc out.db [--list]` — 룰별 요약(–list 시 에러 좌표까지).
  GUI 없이 호스트에서 db 파일 sanity 확인용.
- **테스트 db 생성**: `python tools/gen_drc_db.py <layout.oas> out.db
  --checks 4 --per 6 --seed 7` — 레이아웃 bbox 안에 합성 에러를 배치하고
  ground truth를 `out.db.manifest.json` 으로 남긴다. 생성 포맷은
  klayout 자체 Calibre ASCII 리더(`klayout.rdb`)가 그대로 읽는 것으로
  교차검증했다 (아이템/카테고리 수 일치).

### 단일 인스턴스 동작 (flateyes와 동일)

floe는 이미지 뷰어 flateyes의 OASIS 버전으로, 인스턴스 모델을 그대로 따른다:

- **(uid, DISPLAY)당 뷰어 창 1개.** 첫 실행이 창을 열고, 같은 DISPLAY에서의
  이후 `floe view 다른파일.oas` 는 실행 중인 창에 경로를 넘기고 즉시 종료한다
  (exit 0, ~0.1초 — forward 경로는 klayout/GTK를 import하지 않음).
  기존 창이 해당 파일로 전환되고 앞으로 올라온다. `--goto` 옵션도 함께
  전달된다 (`경로\tgoto=X,Y[,W]` 요청 라인).
- **DISPLAY 값이 다르면 독립 창** — 한 리눅스 호스트에서
  `DISPLAY=:1 floe view a.oas` 처럼 여러 사용자 DISPLAY로 각각 실행 가능.
- `--multi`: 단일 인스턴스를 끄고 항상 독립 창을 연다 (소켓 미사용).
- 소켓: 리눅스는 abstract namespace(`\0floe-<uid>-<display>`, stale 불가),
  그 외는 파일 소켓(+probe 후 unlink로 stale 처리). macOS 개발 환경은
  DISPLAY 없이 'aqua' 키로 동일하게 동작.
- 종료 코드: 0 = 정상/전달됨, 1 = DISPLAY 미설정·소켓 실패,
  **3 = X 디스플레이 접속 불가** (죽은 세션, xauth 불일치 등).
- 전달 대상 파일의 인덱스가 없으면 **forward 전에 이 터미널에서** 먼저
  인덱싱한다 (GUI 프로세스가 몇 분씩 멈추는 것 방지).

### 네이티브 뷰어 (`floe view`) — GTK3/PyGObject (flateyes와 동일 제약)

GUI는 **GTK3/PyGObject** 셸이다. flateyes와 같은 폐쇄망 호스트
(RHEL 계열 GNOME, 아무것도 설치 불가, PyGObject/GTK3만 스톡)를 그대로
따른다:

- **pycairo 없음**: GTK draw 시그널을 쓰지 않는다. 프레임·오버뷰·러버밴드·
  측정선·스냅 마커·선택 하이라이트 전부 **GdkPixbuf 합성**(`fill_rect` =
  subpixbuf.fill, 대각선은 점묘)으로 하나의 pixbuf에 그려 `Gtk.Image` 한 장으로
  표시하고, 텍스트(측정 라벨)는 `Gtk.Overlay` 위 `Gtk.Label`을 margin으로
  배치한다 (flateyes와 동일 기법).
- GTK import는 lazy(`import_gtk`) — 모듈 import는 GTK 없이도 되고(헤드리스
  테스트), PyGObject 부재/디스플레이 접속 불가는 **exit 3** + 명확한 메시지.
- 무거운 geometry 렌더링은 klayout C++ 엔진(`klayout.lay` 헤드리스)이
  **별도 렌더 프로세스**(`service.py`)에서 PNG 프레임으로 수행. GUI는 pixbuf
  표시 + 이벤트만 담당한다. 스레드가 아닌 프로세스인 이유: klayout 렌더 루프가
  GIL을 잡아 스레드로는 긴 렌더 동안 메인 루프가 얼어붙는다
  (spawn 방식이므로 `__main__` 가드 필수).
- 실제 설계 레이어는 Calibre식 단일 패스로 표시한다. 모든 레이어가 같은
  화면 픽셀 위상을 공유하는 1px 50% 체크보드 speckle 채움과 동일한 레이어
  색의 연속 1px 외곽선을 사용한다. KLayout의 paint-plane별 stipple 위상
  이동은 두 역상 패턴을 교대로 지정해 상쇄하며 알파 색상 혼합은 하지 않는다.
  hierarchy FRAME_LAYER는 별도의 hollow underlay 규칙을 유지한다.
- live 상태줄은 `live (25 tiles, 233 ms = 0 load + 230 draw, cut<5.9um,
  ~3.4M drawn)` 형태다. `~N drawn` = 이 프레임이 그린 도형 멤버 수 추정치
  (타일·밴드 로드 시 서브트리 멤버 수를 캐시해 두고 뷰와 겹친 면적 비율로
  합산 — 전수 카운트 대비 비용 0, full-view에서 전수와 일치 검증).
  draw 시간이 큰데 drawn이 작으면 증폭 문제(중복/배열), 둘 다 크면 순수
  물량 문제로 상태줄만으로 구분할 수 있다.
- 렌더 요청(줌/팬/레이어/depth 변경) 제출 시 하단 상태바 우측에 "rendering…"
  인디케이터가 즉시 표시되고, 1.5초를 넘기면 경과 초가 붙는다. 렌더 중에도
  팬/줌 가능하며, 밀린 요청은 최신 것만 처리된다.
- 상태 영역은 2단이다. 윗단에는 커서 좌표, ruler/selection/DRC/clip 메시지와
  view 크기, depth/cut/coverage/LOD 상태를 표시한다. 아랫단에는 마지막 렌더
  성능과 `rendering…`/refinement 진행을 표시하므로 마우스 이동으로 렌더
  정보가 사라지지 않는다.
- 운영 튜닝은 shell 환경변수를 사용하지 않고 `view` 옵션으로 명시한다:
  `--lod on|off`, `--frames on|off`, `--labels on|off`, `--stream-kb`,
  `--stream-target-ms`, `--render-debug`. FRAME_LAYER는 사이드 버튼 또는
  `h`로도 즉시 켜고 끌 수 있으며, off 요청은 daemon의 frontier와
  블록명 생성을 중단한다. `lod/frames/labels`는 이미 실행 중인 단일
  인스턴스에도 전달된다. render-process 생성 옵션(`--stream-kb`, 기본과
  다른 `--stream-target-ms`, `--render-debug`)은 독립 인스턴스를 연다.
  OS 세션의 `DISPLAY`/`XDG_RUNTIME_DIR`과 Cargo
  빌드 식별값은 애플리케이션 튜닝값이 아니므로 환경에서 계속 받는다.
- 인스턴스 소켓은 `GLib.io_add_watch`로, 결과 큐는 `GLib.timeout_add`(25ms)로
  서비스한다. UI 라벨은 English only (XQuartz 한글 글리프 부재 — flateyes 규칙).
- 키: `f` fit, `+`/`-`(`=`) 줌, `r` ruler, `m` 스냅, `d` depth 다이얼로그,
  `g` goto 다이얼로그, `c` detail cut 다이얼로그, `a` abstract 모드,
  `v` coverage 밀도 채움 토글(VFS), `e` DRC 브라우저,
  `n`/`p` 다음/이전 DRC 에러, `0`-`9` depth, `Esc` 단계 해제,
  `q` 종료 (확인 다이얼로그). cut/coverage 상태는 윗단 상태바 우측
  (`cut: L1 · cov: off` 등).
- 마우스: 왼쪽 드래그는 패닝(짧은 클릭은 객체 선택), 오른쪽 드래그는
  영역 줌(오른쪽 방향은 확대, 왼쪽 방향은 축소)이다. 휠은 커서 위치를
  기준으로 이벤트당 최대 한 단계(4%)만 줌한다. 가운데 드래그도 패닝을
  유지한다.
- 레이어 패널은 텍스트 목록이다 (체크박스 없음): 이름 **더블클릭**으로
  온/오프하며 꺼진 레이어는 **취소선**으로 표시된다. layer 번호로
  그룹핑되며, 같은 번호에 datatype이 여럿이면 (`11/0 M1`, `11/1 M1.1`)
  최소 datatype이 부모 행이 된다. 부모 앞 `+`를 클릭하면 자식 datatype들이
  아래로 펼쳐지고 마커가 `-`로 바뀐다 (다시 클릭하면 접힘; 기본은 접힘).
  그룹이 접혀 있을 때 부모를 온/오프하면 자식도 함께 바뀐다. 그룹을
  펼치면 부모와 각 자식 datatype이 모두 개별적으로 토글된다.
  자식 없는 레이어는 마커 자리가 공백이라 모든 이름이 좌측 정렬된다.
  레이어 행은 기본 UI보다 약 20% 큰 글꼴을 사용하며 layer/datatype 열은
  전체 목록의 최대 길이로 고정되어 모든 색상 마커가 같은 열에 정렬된다.
  폴리곤을 pick하면 해당 행을 배경/글자색으로 강조한다. 접힌 datatype은
  자동으로 펼치고 그 행이 보이도록 스크롤하며, 빈 공간 pick 또는 `Esc`로
  선택을 해제하면 강조도 사라진다.
  패널 하단 `expand all` / `collapse all` 버튼으로 모든 그룹을 한 번에
  펼치거나 접는다 (visibility에는 영향 없음).

### depth (계층 표시 깊이)

Calibre DESIGNrev의 depth와 동일한 개념. 0 = 설계 top 셀의 shape만 표시
(하위 셀은 외곽 프레임 + 셀 이름), N = N 단계 아래까지 전개, 999 = 전체.
KLayout `LayoutView.max_hier_levels`로 구현하며, 타일 모자이크가 만드는
내부 2단계(FLOE_MOSAIC→TILE)는 오프셋으로 숨겨서 사용자에게는 원본 설계
계층 기준으로 보인다. `d` 키 → 다이얼로그(모달, 부모 창 중앙; 프리셋
0/1/2/3/full + 스핀박스로 실시간 조정, ok/Enter/Esc 로 닫기),
`render --depth` 지원.
현재 depth와 cut은 하단 상태바 우측에 항상 표시된다
(`depth: full · cut: L1` 형태).
**단축키: `d` = 다이얼로그 (Calibre와 동일), 숫자 `0`~`9` = 해당 depth.**
full은 다이얼로그의 full 프리셋/스핀박스(999)로 지정.

- **시작 depth** (v0.10): 일반 오픈은 **depth 0**으로 시작한다 —
  top 지오메트리 + 직계 자식 아웃라인 프레임으로 어떤 칩에서도 가장
  빠른 정직한 첫 화면을 띄운다. `--goto`나 `--drc`로 특정 지점을
  검사하러 들어갈 때는 **full**로 시작하고, `view --depth N`이 두
  경우 모두를 덮어쓴다. 런타임 변경은 기존과 동일(숫자/`d`).
  depth 값 자체는 사용자가 정한 값에서 자동으로 바뀌지 않는다.
- **abstract 모드** (v0.6.1): `a` 키 토글. klayout abstract 모드로
  화면 10px 미만 셀을 내용 없는 프레임으로만 그린다 — 손실 있는
  **탐색 가속 모드**다 (광역 실측 4.5s→0.18s, 25×; 화면 정보의 큰
  부분이 생략되므로 상태바에 `· abstract`가 떠 있는 동안은 판독용으로
  쓰지 말 것). 셀 단위라 flat 필에는 효과가 없다 — 그건 컷 레벨과
  병합 트윈의 몫.
- 광역 뷰 비용은 depth가 아니라 크기 밴드 컷이 제한한다 (아래 크기
  밴드 섹션). 예전의 밀도 기반 auto depth는 밴드 도입으로 제거했다 —
  depth는 순수하게 계층 탐색 도구다.
- 뷰에 걸린 모든 타일이 지원하는 얕은 명시 depth의 렌더는 풀 타일 대신
  `tiles_lod/` 축약 타일을 로딩하므로 첫 방문 영역도 빠르게 뜬다
  (풀 타일 로딩은 깊은 줌에서만).
- **점진적 렌더 (LOD 프리뷰)**: 어레이 밀집 설계는 풀 타일 하나가 수천만
  도형으로 전개되어 첫 파싱에 수십 초가 걸릴 수 있다 (예: repetition
  압축된 150MB 파일 → 저장환산 20억 도형, 타일당 ~44초). 이런 첫 방문
  렌더는 LOD 타일로 만든 **프리뷰 프레임을 즉시(수십 ms) 표시**하고
  ("preview - loading tiles…" 표시), 풀 타일 파싱이 끝나면 같은 뷰가
  조용히 최종 프레임으로 교체된다. 한 번 로드된 타일은 캐시되어
  재방문은 빠르다.
- **점진 밴드 프레임** (v0.7.0): 첫 방문에서 파싱할 밴드 바이트가
  문턱(8MB ≈ 파스 0.5초)을 넘으면 단일 대기 대신 **굵은 것부터 쌓는
  시퀀스**로 그린다 — b0(작은 파일)+아직 안 온 밴드의 병합 트윈으로
  수십 ms에 근완성 프레임 → 미세 밴드가 파스되는 대로 재렌더(트윈
  과대 슬래브가 정밀 지오메트리로 교체) → 최종 프레임. 매 단계가
  보이는 부분집합의 **완전 재렌더**라 레이어 z-순서가 항상 정확하고,
  최종 프레임은 단일 패스와 **픽셀 동일**(검증). Calibre의 층별
  차오름과 같은 문법의, 크기 축 버전이다.
- 부수 수정 (v0.7.0): 컷이 숨긴 밴드 셀이 klayout hide_cell의 흰
  bbox 프레임으로 그려져 광역 뷰 타일 경계에 흰 선이 남던 문제 제거 —
  가시성은 이제 모자이크 top의 인스턴스 목록 재구성으로 처리한다
  (스냅/픽은 측정 전 전 밴드 복원).
- far view(스켈레톤)에도 반영된다: 기본(auto)은 depth 0 모습(블록
  아웃라인 + 이름 + 탑 대형 도형)만 보여주고 — 최초 로딩 화면 포함 —
  명시적 depth k(1/2)면 레벨 k까지의 스켈레톤 디테일(파워 스트랩,
  라벨 등)을, full이면 전부 그린다. 라이브 depth 의미와 일관됨.
- 타일 경계에서 잘린 셀은 변형본 이름(`BLK_0_0$1` 등)으로 표시될 수 있다.
- 주의: 대형 어레이 영역에서 어중간한 depth(비트셀 프레임 수백만 개를
  외곽선으로 그리는 경우)는 full depth보다 느릴 수 있다.

### 크기 밴드 (size bands) — full depth 광역 뷰의 근본 최적화

klayout은 서브픽셀 도형도 전부 순회하며 그리므로(멤버당 비용, ~8M개/s)
광역 full-depth 뷰가 어레이/필 밀집 영역에서 수십 초씩 걸렸다
(`0 load + 12600 draw` 류). depth를 낮추는 것은 회피책이므로, 인덱스가
타일을 **도형 크기 밴드**로 분할해 두고 렌더 시 화면에서 안 보일 밴드를
로드/드로우에서 통째로 제외한다.

- 인덱싱 때 각 타일을 `tiles_b0/`(≥2µm) `b1`(0.5–2) `b2`(0.125–0.5)
  `b3`(<0.125µm) 파일로 분할 (`floe index --bands 0.125,0.5,2` 기본,
  `--bands none` = 구형 단일 타일). 셀 트리·인스턴스는 밴드마다 보존
  (셀명에 `__b<k>` 접미사)되어 depth 의미는 동일하고, 그 밴드에 도형이
  없는 서브트리는 가지치기된다. 밴드 합 = 원본 타일 전체 (손실 없음).
- 렌더 시 뷰 스케일로 컷 크기를 계산해 큰 밴드만 로드하고, 이미
  로드된 미세 밴드는 klayout hidden-cell로 드로우에서 제외한다.
  컷이 발동하면 상태줄에 `cut<0.35um` 형태로 표시.
- **컷 레벨** (v0.6.0): 사용자에게 노출되는 단위는 픽셀이 아니라
  **레벨**이다 — off(0) / L1 / L2(기본) / L3, 높을수록 광역 뷰가
  가벼워진다. 레벨 뒤의 화면-px 문턱값(현재 2/4/8px)은 구현 세부라
  나중에 조정돼도 "L1"의 의미는 유지된다. `c` 키 다이얼로그로 실시간
  변경, 시작값은 `view --cut-level`(기본 2). 상태바 우측에
  `depth: full · cut: L2 · cov:off · lod:off`처럼 각 상태를 상시 표시.
  VFS의 LOD는 기본 off이며 merged page 선택만 제어한다. cut과
  coverage는 LOD 상태와 독립적으로 동작한다.
- **병합 트윈 (merged twin)** (v0.6.0): 컷으로 빠지는 밴드는 화면에서
  사라지는 대신 **병합 트윈**으로 그려진다 — 인덱싱 때 밴드별 지오메트리를
  flatten하고 닫힘(closing: `sized(+d)→merged→sized(−d)`, d = 밴드
  상한×0.5)으로 서브픽셀 틈을 메워 필 필드를 큰 슬래브 몇 개로 융합한
  것 (`tiles_m<k>/`). klayout merge는 닿는 도형만 합치므로 간격을 둔
  더미 필에는 닫힘이 필수다. 트윈은 **벡터**라 레이어 색·토글이 살아
  있고(미리 렌더한 이미지 방식은 색 변경 때문에 탈락), 스냅/픽에서는
  제외된다(드로우 전용 대역). 닫힘 후에도 폴리곤이 4096개를 넘는
  희소 레이어는 트윈을 만들지 않는다(어차피 압축이 안 되는 산발
  콘텐츠). **트윈은 항상 타일링 후 별도 패스로 빌드된다**(v0.6.20) —
  인라인 빌드는 제거됐다. 후처리가 구조적으로 우월한 이유: ① 소스
  레이아웃(수십 GB)이 RAM에 필요 없어 워커가 사실상 공짜, ② 쓰인
  밴드 파일은 writer가 정리한 최소본(콘텐츠 레이어만)이라 걷기가
  싸다, ③ 실패·재시도가 인덱싱과 분리된다. 기본 off(사용자 결정:
  인덱싱 완주 우선 — 트윈 없으면 컷된 밴드가 사라질 뿐), `--merge`
  = 인덱스 끝에 자동 후처리, `--merge-only` = 기존 캐시에 언제든
  후添(밴드 파일만 읽음; stress30 실측 7초, midi 1초).
- 효과 (midi 스트레스 캐시, 16타일 full depth 광역 뷰):
  **5,082ms → 17ms** (draw 4,528→16ms). 근접 줌은 전 밴드 로드 = 기존과
  동일. auto depth 기능은 제거됨 - full depth가 기본이고 비용은 컷이
  제한한다.
- 정확도: 스냅/픽킹/클립은 항상 전 밴드를 사용하므로 측정은 벡터
  정확도 그대로. CD 측정 줌(도형이 수십 px)에서는 픽셀 단위로 동일함을
  검증했다. 밴드 파일은 Region 경유로 box가 폴리곤으로 저장되어 1–2px
  크기 도형의 래스터가 ±1px 다를 수 있다 (기하 XOR은 완전 일치).
- 클립 출력의 셀명에는 `__b<k>` 접미사가 남는다. 원본 그대로의 셀명이
  필요하면 `floe clip --exact` (소스 직접 클립).
- 메모리: 미세 밴드는 방문한 타일만 로드되고 LRU로 방출되므로
  사용자당 상주량은 "국소영역 + 가벼운 전역 밴드"로 제한된다.

## 폐쇄망 리눅스 배포

floe는 순수 파이썬. 의존성: `klayout`, `numpy` pip 휠 + GUI는
PyGObject/GTK3 (**RHEL 계열 GNOME 호스트에 기본 탑재** — flateyes와 동일하게
추가 설치 없음). 인덱싱만이라면 Rust 바이너리 한 개 반입이 가장 단순하다
(아래 [Rust 인덱서](#rust-인덱서-floe-index) 섹션).

```sh
# 1) 인터넷 PC에서 휠 수집 (타겟 파이썬 버전에 맞춰)
pip download klayout numpy -d wheels/ \
    --platform manylinux2014_x86_64 --only-binary=:all: \
    --python-version 311        # 예: 타겟이 python3.11

# 2) 폐쇄망 호스트에서 - 반드시 시스템 PyGObject가 보이는 파이썬으로
python3 -c 'import gi; gi.require_version("Gtk", "3.0")'   # GUI 사전 확인
python3 -m venv --system-site-packages .venv               # gi가 보이게
.venv/bin/pip install --no-index --find-links wheels/ klayout numpy
# floe/ 디렉토리 복사 후: .venv/bin/python -m floe ...
```

**주의 — PyGObject/pycairo를 pip으로 설치하지 말 것.** pip은 meson 소스
빌드를 시도하고 폐쇄망에는 pkg-config/cairo-devel이 없어
`Dependency lookup for cairo ... failed` 로 실패한다. PyGObject는 OS RPM
(`python3-gobject`, GNOME 호스트 기본 탑재)을 쓰고, venv는
`--system-site-packages` 로 만들어 그것을 보이게 하는 것이 정답이다
(pycairo는 아예 불필요 - floe는 cairo를 쓰지 않는다). venv를 SCL/conda
등 다른 파이썬으로 만들면 시스템 gi와 맞지 않으니 위 사전 확인이 성공한
바로 그 python3를 사용할 것.

- CLI 전용(`index/info/render/clip`)이면 GTK 없이도 동작한다.
- 원격에서는 Exceed TurboX/XQuartz 등 X 서버로 `floe view` 실행 (flateyes와
  동일한 접속 형태). macOS 개발 환경 설정은 위 [환경 설정](#환경-설정) 참고.

### floe-portable: 완전 자립 번들 (PyGObject 없는 호스트용) — 권장

호스트에 `python3-gobject`(gi)조차 없거나 파이썬 버전이 안 맞는 경우,
flateyes-portable과 동일하게 필요한 걸 전부 싸서 가져간다. Python +
PyGObject + GTK3 (conda-forge, 재배치 가능) + klayout + numpy + floe가
한 tar에 들어가고, 호스트엔 아무것도 설치·변경하지 않는다 (쓰는 것은 X
디스플레이와 시스템 폰트뿐).

flateyes와 달리 floe는 **klayout/numpy가 PyPI 휠**이라 런타임의 pip으로
설치해야 하므로, 번들은 **x86_64 Linux 빌드 머신**에서 만든다. 그 휠들이
**glibc 2.27+**를 요구하므로 타깃은 **RHEL8+**여야 한다 (RHEL7은 klayout
휠 자체가 안 돌아 불가). 빌드 시 verify가 실제 glibc floor를 출력한다.
`tools/make_portable.sh`가 flateyes의 레시피를 그대로 따른다:

```sh
tools/make_portable.sh                     # -> floe-portable-<ver>-<date>.tar.gz
# 폐쇄망 미러만 되는 빌드 머신이면 klayout/numpy 휠을 미리 받아두고:
WHEELS=./wheels tools/make_portable.sh
```

호스트에서는 **풀고 실행만** (설치·권한 불필요):

```sh
tar xzf floe-portable-*.tar.gz -C /opt     # 위치 자유
/opt/floe-portable/selfcheck               # 창 없이 스택 검증 (모두 OK 확인)
/opt/floe-portable/floe view /path/chip.oas
/opt/floe-portable/floe index /path/chip.oas   # CLI도 동일
```

conda-forge가 GTK 스택 전체를 재배치 가능하게 묶어주므로 (GIR 타입립·
gdk-pixbuf 로더·스키마·폰트 포함) gi를 시스템에 얹을 필요가 없다. 런처가
첫 실행 시 호스트별 캐시(GSettings 스키마, pixbuf 로더 목록)를 자동
생성한다. floe 코드만 갱신하려면 새 `floe/` 패키지를 번들의
`runtime/lib/python*/site-packages/floe`에 덮어쓰면 된다 (재빌드 불필요).

**뷰어 문제 진단 순서** (창이 검게 나오는 등):
1. `selfcheck` — 스택(gi/GTK/pixbuf/klayout) 검증, 창 없이.
2. `floe render <src> --bbox ... --out t.png` — 캐시·렌더 엔진 검증, GUI 없이.
3. `floe probe <src>` — **뷰어의 렌더 서비스 경로**(spawn 자식 + 큐 + 프레임)
   를 GUI 없이 그대로 실행. 여기가 실패하면 뷰어는 검게 뜬다; 통과하면
   디스플레이 쪽 문제다. 뷰어 자체도 서비스가 죽으면 상태바에
   "error: render service died"를 표시한다 (워치독).
4. `floe gtktest [png]` — 최소 픽스버프 표시 매트릭스 (표시 계층 문제 판별).

**XQuartz 주의 — 이미지가 검게 표시**: XQuartz(맥에서 ssh -X로 접속)의
XRender 구현은 텍스트만 그리고 **이미지 합성을 검게 표시**한다 (창을
초기보다 키우면 더 확실히 재현; 시스템 GTK venv에서는 미발생, 포터블
번들의 cairo 조합에서 확인). XQuartz로 볼 때는 **`FLOE_XQUARTZ=1`**로
실행하면 cairo 코어 프로토콜 폴백으로 우회되어 표시된다 — 단 프레임
전송이 느려지고, 화면 갱신이 다음 입력 이벤트까지 지연되는 잔여 문제가
있다 (마우스를 움직이면 갱신됨). **기본값은 정상 XRender**로, 운영 접속
경로인 Exceed TurboX 등 실제 X 서버에서는 그대로가 맞다. XQuartz는
개발 편의용 뷰어로만 간주하고 최종 검증은 실제 대상 호스트에서 할 것.

### 대안: venv 통째 복사 (호스트에서 pip 실행이 어려울 때)

호스트에서 `pip install`조차 어려우면(정책·권한) venv를 빌드 머신에서
완성해 통째로 복사한다. venv는 공식적으로 재배치 가능하지 않지만,
아래 조건을 지키면 실전에서 문제없이 동작한다:

```sh
# 1) 빌드 머신: 운영과 같은 RHEL major·x86_64·같은 python3 마이너 버전
#    (운영과 같은 RHEL 컨테이너/VM 권장). venv는 호스트에 놓일
#    "최종 절대 경로"에서 만든다 - 스크립트 shebang에 이 경로가 박힌다.
python3 -m venv --system-site-packages /opt/floe/venv
/opt/floe/venv/bin/pip install klayout numpy
cp -r floe /opt/floe/
tar -C / -czf floe-venv.tar.gz opt/floe

# 2) 폐쇄망 호스트: 같은 경로에 풀고 검증
tar -C / -xzf floe-venv.tar.gz
/opt/floe/venv/bin/python -c "import klayout.db, numpy, gi"  # 한 줄 검증
alias floe="/opt/floe/venv/bin/python -m floe"
```

- **같은 베이스 파이썬이 호스트에 있어야 한다**: venv는 표준 라이브러리를
  담지 않고 `pyvenv.cfg`의 `home=` 경로에 있는 python3를 쓴다. 빌드
  머신과 호스트가 같은 python3 RPM(모듈 스트림)이어야 함.
- **실행은 항상 `venv/bin/python -m floe` 형태로.** `venv/bin/pip` 등
  스크립트는 shebang에 빌드 경로가 박혀 있어 경로가 다르면
  "bad interpreter"로 죽는다 (`python -m` 은 shebang을 안 탐).
  부득이 다른 경로에 풀었다면 python 실행은 되지만 스크립트류는 못 쓴다.
- **gi/GTK는 venv에 실려 가지 않는다**: PyGObject는 시스템
  site-packages(`python3-gobject` RPM)에서 오므로 호스트 쪽 RPM은 여전히
  필요하다 (GNOME 호스트 기본 탑재). klayout·numpy는 manylinux 휠이라
  .so가 자체 완결적이어서 복사에 안전하다.

### .ice 구조와 설계 노트

```
<src>.ice/
  meta.json      원본 지문(size/mtime), 그리드, 레이어 테이블(+색),
                 타일별 depth-밀도 테이블(auto depth용), 통계
  tiles_b<k>/t_r_c.oas  타일별 OASIS를 도형 **크기 밴드**로 분할 저장
                   (절대좌표 유지, 전 레이어, 경계에서 절단; b0 = 최대
                   크기 밴드). 렌더는 뷰 스케일이 요구하는 밴드만 로딩 -
                   광역 뷰는 서브픽셀 필/어레이 밴드를 아예 읽지 않는다.
                   빈 밴드는 파일 생략. (`--bands none` 구형 캐시는
                   tiles/t_r_c.oas 단일 파일)
  tiles_lod/...    depth 제한 축약 타일 - 누적 5만 도형 캡까지 레벨을
                   유지하고 그 아래는 고스트 bbox(254/0)로 대체, 타일별
                   지원 depth는 meta.lod.tiles에 기록. 얕은 depth 렌더는
                   풀 타일 대신 이것만 로딩 (밴드 캐시에서는 캡 이하
                   타일도 파일 생성 - 단일 풀 타일 폴백이 없으므로)
  skeleton.oas   줌아웃용 구조 모델 - 블록 아웃라인(255/0) + 탑 대형 도형
                 (= depth 0 뷰), 레벨 k 셀의 스트랩·라벨은 트윈 레이어
                 (dt + k*30000)에 분리 저장 (depth >= k 명시 시 표시)
```

- **geometry는 타일 경계에서 잘린다** — 뷰어/영역분석 용도로는 무해하나
  경계에 걸친 원본 도형이 그대로 필요하면 `clip --exact` 사용.
- **뷰어 모자이크는 타일별 셀 격리** (v0.4.1): klayout은 multi-read 시
  같은 이름의 셀을 병합하는데, 이대로 두면 (a) k개 타일에 걸친 셀이 같은
  위치에 k번 인스턴스되어 **draw가 k배**가 되고 (b) evict(prune) 후
  재로드할 때 공유 셀에 남아 있던 도형이 **또 병합돼 세션이 길수록
  무한 누적**된다 (호스트 234s draw 사례의 원인, 로컬 재현·확인).
  그래서 로드 직후 새로 생긴 셀 전부에 `@t<r>_<c>_<k>` 태그를 붙여
  병합을 원천 차단한다 — 캐시 포맷 불변(재인덱싱 불필요), 표시 이름은
  `_strip_band`가 태그를 벗긴다. clip 경로(`load_region`)는 매번 새
  Layout이라 병합(셀 재완성)을 그대로 유지한다.
- **텍스트는 타일에 저장하지 않는다** (v0.5.4) — far view 스켈레톤
  라벨로만 존재한다 (사용법의 텍스트 절 참조).
- **어레이 재압축**: `Layout.clip`은 타일 경계에 걸린 셀 어레이(SRAM 등)를
  개별 인스턴스 수백만 개로 풀어버린다. 인덱서가 격자 패턴을 감지해 정규
  CellInstArray로 재구성한다 (정규 어레이는 OASIS 라운드트립에서 보존됨
  → 타일 로딩 50배 가속).
- `clip_into` 사용 시 타겟 레이아웃에 레이어를 미리 생성해야 한다.
  안 그러면 anonymous 레이어로 복사되어 OASIS writer가 통째로 버린다.
- **타일 빌드는 fork 병렬** (`--jobs`, 기본 = 코어 수 상한). klayout
  바인딩은 C++ 실행 중 GIL을 잡고 있어 파이썬 스레드로는 병렬화가 안
  된다(실측 1.0배). 대신 소스 read 후 fork하면 로드된 레이아웃이
  copy-on-write로 공유되어 재읽기 없이 워커들이 타일을 나눠 만든다.
  결과는 순차 빌드와 바이트 단위로 동일 (fork 없는 플랫폼은 순차 폴백).
- **메모리 거버너 (v0.5.5, 램프는 v0.6.6)**: 워커별 타일 빌드
  메모리(Region 전개 등)는 COW와 무관한 사유 메모리라, 코어 수만큼
  워커를 띄우면 작은 기계에서 OOM이 난다(16GB 맥 + stress30: 10워커
  × 4~8GB 실측). `허용 워커 수 = (가용 RAM + 워커 사유분 − 바닥 여유)
  / (추정 × 1.25)`를 0.5초마다 재계산해 디스패치를 조이고, 추정치는
  라이브 워커 RSS로 계속 상향 학습한다. **첫 타일이 완료되기 전에는**
  보수적 사전값(12GB/워커)으로 예산을 나눈 만큼 즉시 가동한다 —
  예전에는 중앙 타일 1개를 단독 프로브해서 96코어 호스트가 최악
  타일이 끝나는 ~20분 동안 놀았다(실측). 16GB 랩톱은 사전값 기준으로도
  1워커라 안전성 불변. 워커는 타일 1개마다 재fork되어
  (`maxtasksperchild=1`) 끝난 타일의 메모리가 OS로 실제 반환된다.
  로그: `governor: N workers (~X GB/worker, Y GB free)`. `--jobs`는
  상한으로 유지되고, `--mem GB`가 실행 전체(로드된 소스 + 워커)의
  메모리 상한 — 다른 프로그램·사용자 몫을 남겨야 하는 공유 호스트용.
  `--no-gov`가 예전 동작(무조건 N워커), `--mem-floor GB`가 시스템
  가용 메모리 바닥 여유(기본 max(2, RAM의 5%)).
- **메모리**: 로드된 레이아웃 RSS ≈ 파일 크기의 ~3.6배 (1.5GB → 5.4GB
  실측, flat 기준 — viewer read는 어레이 소스에서 그보다 훨씬 작음).
  부족하면 거버너가 워커를 줄여 완주는 하지만 그만큼 느려진다. 뷰어는
  타일만 로딩하므로 영향 없음.
- **공유 호스트 주의 — RSS는 허수**: fork COW 때문에 `top`/`ps`에는
  워커 각각이 부모의 로드된 소스(수십 GB)를 통째로 쓰는 것처럼
  보인다. 40워커면 겉보기 수 TB지만 실사용은 "소스 1부 + 워커 사유분
  합"이고 `--mem`이 그 합을 강제한다. 관리자에게는 시스템 가용
  메모리(free)나 PSS(`/proc/<pid>/smaps_rollup`) 기준으로 설명할 것 —
  RSS 합산은 이 구조에서 항상 과대계상이다.
- **중단 내성 (v0.6.13)**: 완료된 타일마다 진행 저널
  (`<cache>/progress.jsonl`)에 기록되므로, 런이 중간에 죽어도(호스트
  OOM 킬, 관리자 정리 등) **같은 명령을 다시 실행하면 이어서
  빌드**한다 (read·텍스트 단계는 다시 하지만 완료 타일은 스킵; 타일
  빌드는 결정적 덮어쓰기라 반쯤 쓰다 만 타일은 그냥 재생성).
  소스·버전·옵션이 달라지면 자동으로 처음부터. `--force` = 저널
  무시하고 전체 재빌드. 타일링 중 워커 전원이 외부에서 죽으면(3분간
  busy 0) 잃어버린 타일을 자동 재디스패치한다.

## Rust 인덱서 (floe-index)

`rust/` 워크스페이스의 네이티브 인덱서. **Python `floe index`를 대체**하며
(python 인덱서는 동결 — 버그 수정 없음), 동일 포맷의 `.ice` 캐시(밴드
타일 b0~b3 + tiles_lod + density + meta.json + skeleton.oas + texts.tsv
사이드카)를 만들고 뷰어가 그대로 연다 (호환 확인 완료).

- **순수 Rust, klayout 무관** — floe-oasis(파서/라이터), floe-tiler,
  floe-index CLI. 의존 크레이트는 `vendor/` 동봉이라 빌드 중 crates.io
  접속이 없고 C 툴체인도 불필요하다.
- **성능**: MAIN09(150MB, 25타일) 실측 **60초** (glibc, `--jobs 12`).
  파스·타일링 모두 병렬. 권장 jobs는 **12~16** — 테스트 서버 실측에서
  12가 24보다 빨랐다 (메모리 대역폭 무릎).
- **캐시 크기**: klayout급 모달 인코딩 (XYRELATIVE, 레코드 정렬로 델타
  생략, W/H·halfwidth·repetition 모달 재사용) + CBLOCK 압축.

### 사용법

```sh
floe-index index chip.oas --jobs 12          # outdir 생략 = <src>.ice
floe-index index chip.oas out.ice --mem 200 --mem-floor 8   # 공유 호스트
floe-index scan chip.oas 16                  # JSON 인벤토리 (진단용)
floe-index --version
```

- `--jobs N` 워커 수, `--tile-bytes N` 타일 목표 크기,
  `--bands um,um,um` 크기 밴드 문턱 (기본 0.125,0.5,2 — python과 동일).
- **메모리 거버너**: `--mem GB` = 프로세스 RSS 상한, `--mem-floor GB` =
  시스템 가용 메모리 바닥 여유 (기본 max(4GB, RAM 5%)). 한도에 닿으면
  새 타일 착수를 보류하되 최소 1워커는 항상 진행하고, 하트비트에
  `K waiting (mem)`으로 표시된다. /proc 기반이라 리눅스 전용
  (macOS에서는 비활성).
- **진행 로그**: 시작 시 버전 스탬프 `[floe-index <버전> <git> (gnu)]`
  (반입 바이너리가 여럿 돌아다니므로 어떤 빌드인지 매 실행 명시 —
  버전은 rust 변경이 포함된 푸시마다 올라가서 zip 반입 빌드도
  버전 번호만으로 식별된다),
  5초마다 `tiles N done, M building / total (Ns)` 하트비트, 타일이
  64개 이하면 타일별 완료 라인(멤버 수·소요 시간)도 나온다.
- **종료 요약**: 끝나면 캐시 크기 요약이 자동 출력된다 — 밴드별
  (`tiles_b0..b3`, `tiles_lod`) 크기·파일 수, skeleton/texts.tsv/
  meta.json 크기, 합계와 원본 대비 배율, 피크 RSS(`VmHWM`)와 거버너
  대기 횟수(리눅스). `du` 스윕 없이 이 블록만 옮겨 적으면 된다.
  stdout JSON에도 `src_bytes`/`cache_bytes`가 들어간다.
- `scan`은 셀/레이어별 레코드·멤버 수, 텍스트, placement, repetition
  타입 히스토그램(`rep_types`)을 JSON으로 출력한다 — 파일 구성 진단용
  (175GB 캐시 사건의 원인 확정도 이 히스토그램으로 했다).

### 빌드와 배포

```sh
# macOS/개발 (Apple Silicon 포함 - 네이티브 빌드, 특별 절차 없음)
cd rust && cargo build --release      # -> target/release/floe-index

# 리눅스 서버 반입용 (상세·오프라인 툴체인은 rust/BUILD.md)
sh rust/build-linux.sh
#  -> dist/floe-index-linux-gnu     glibc 동적 - 권장 (병렬 인덱싱 ~40%
#                                   빠름; 타깃 glibc >= 빌드 머신이면 OK)
#  -> dist/floe-index-linux-x86_64  musl 완전 정적 - 어떤 x86_64 리눅스든
#                                   실행되는 이식성 폴백 (+ .sha256)
```

- 배포는 **바이너리 한 개 반입 + `chmod +x`** 로 끝난다 (venv/휠 불필요).
- musl 정적 빌드에는 musl malloc의 전역 락(전 스레드 수가 ~190초로
  수렴하는 병목)을 우회하는 스레드 캐시 allocator가 내장된다. glibc/
  macOS 빌드는 순수 시스템 allocator를 쓴다 (자체 per-thread 캐시 보유).
- **검증**: `sh tools/validate_rust.sh` (~20초) — scan 카운트, 밴드 파일
  지오메트리 XOR, depth별 멤버 수, meta/density(밴드 파일 기반 정밀
  오라클), skeleton/사이드카를 klayout 빌드 캐시와 대조하는 5단계
  스위트. rust 쪽 커밋 전 필수.

## 로드맵

1. ✅ 테스트용 대용량 OASIS 생성기 (`tools/gen_test_oasis.py`)
2. ✅ 공간 인덱스(.ice) + CLI (index/info/render/clip)
3. ✅ 네이티브 뷰어 (view): 영역 줌/팬/레이어 토글/depth/clip 저장
4. Calibre DRC 결과(.db) 파서/조회 + 에러 점프: ✅ 1차 (ASCII db 파서
   `floe/drc.py`, `e` 브라우저 + `n`/`p` 점프, `floe drc`, 합성 db 생성기
   `tools/gen_drc_db.py`; klayout.rdb 교차검증). 남은 것: 실제 Calibre
   출력으로 호스트 검증, 에러 영역 자동 clip
5. ~~대용량 스케일링: 인덱싱 시 레이어 그룹별 다중 패스(RAM 상한)~~
   — 운영 호스트 RAM이 1.5TB라 불필요 판정 (2026-07-28; 로드 RSS는
   파일 크기의 ~3.6배 → 100GB급 파일까지도 여유). 단, 이 ~3.6배는
   flat 소스 기준 — 어레이 중심 소스는 editable read가 40배까지
   폭발해(9.83GB → 400GB 관측) v0.4.3부터 인덱스 read 기본을 viewer
   모드로 변경 (위 인덱스 노트). 다중 패스는 여전히 불필요.
   단일 패스가 더 빠르므로 하지 않는다. 타일 병렬 빌드는 ✅ `--jobs`
6. 뷰어 개선: 셀/텍스트 검색, 마커 점프 (좌표 이동(goto)은 ✅;
   중간 줌은 컷 레벨 + 병합 트윈(v0.6.0)이 담당 — 픽셀 피라미드는
   레이어 색 실시간 변경과 충돌해 탈락)
7. 렌더링 취소 키: 긴 드로우를 중단하고 이전 프레임 유지 (사용자
   결정 2026-07-30: 최적화가 충분히 된 뒤 최후 수단으로 구현)
8. **Rust 인덱서** (`rust/`, 위 [Rust 인덱서] 섹션): python 인덱서
   대체. 캐시 완전성 ✅ 스켈레톤+사이드카 ✅ 병렬화+CBLOCK ✅ PATH ✅
   모달 인코딩 ✅ — 5단계 검증 스위트 통과, 뷰어 호환 확인. 남은 것:
   9.8G 실칩 모달 라이터 재측정(서버), 사이드카 스트리밍 쓰기+진행
   하트비트, 컷오버 전 대형 자산 일회 게이트 후 python 인덱서 은퇴.
9. **Calibre 팔레트 임포트** (사용자 결정 2026-08-02, 안정화 후):
   실무자 전원이 Calibre 사용자이므로 뷰어 경험을 동일하게 —
   Calibre layer properties(색·채움 패턴·선 스타일)를 읽어 floe
   레이어 속성으로 매핑. 치밀한 채움 팔레트에서는 페인터 순서가
   실제 가림으로 작동하므로 드로우 순서(스택 순서)도 함께 맞출 것.
