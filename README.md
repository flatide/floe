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

floe index data/testchip_1g5.oas          # 1회: <src>.ice/ 생성
floe info  data/testchip_1g5.oas          # 레이어/그리드/통계 요약
floe view  data/testchip_1g5.oas          # 네이티브 데스크톱 뷰어 (기본)
floe view  data/testchip_1g5.oas --goto 5240,5260,50   # 시작 위치+뷰 폭(um)
floe render data/testchip_1g5.oas --bbox 5000,5000,5200,5200 \
          --layers M2,M3,VIA2 --out view.png
floe clip  data/testchip_1g5.oas --bbox 5000,5000,5100,5100 --out region.oas
```

- `--bbox`는 µm 단위 `X0,Y0,X1,Y1`. `--layers`는 이름 또는 `layer/datatype` 목록.
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
  배선, 그 아래의 boundary 순). 텍스트 라벨도 선택 대상. `Esc` 해제.
- 보이는(체크된) 레이어만 대상. 타일 경계에서 잘린 도형은 잘린 조각
  기준으로 선택된다 (셀 이름에 `$1` 변형 표기 가능).
  줌아웃 상태에서는 인덱싱 때 만든 **skeleton**(블록 아웃라인+이름, 대형
  배선/스트랩, 라벨만 담은 수 MB짜리 구조 모델)을 라이브 렌더링해 어떤
  배율에서도 선명한 플로어플랜을 보여주고, 확대하면 뷰포트와 교차하는
  타일만 lazy 로딩해 실시간 렌더링한다 (LRU 캐시로 메모리 상한 유지 →
  원본 크기와 무관하게 동작). 새 프레임 도착 전에는 이전 프레임이
  원래 배율 그대로 고정 표시된다.
- **마진 렌더링**: 프레임을 뷰포트보다 각 방향 50% 넓게 렌더해 두므로
  (Calibre 방식), 마진 안에서의 팬은 재렌더 없이 즉시 표시되고 경계에
  가까워지면 백그라운드로 재중심 렌더가 돈다. 배율·depth·레이어가 그대로면
  같은 화면을 다시 그리지 않는다. 첫 방문 영역에서 마진이 미로딩 타일을
  크게 늘리면 **2단계 렌더**: 뷰 영역만 먼저 렌더해 즉시 표시하고,
  마진 확장 프레임은 뒤에서 조용히 갈아끼운다 (새 요청이 오면 생략).
- 구버전 캐시에는 `floe index --skeleton-only` 로 skeleton만 추가할 수
  있다 (원본 1회 읽기, 재타일링 없음).

### Goto (좌표 이동, Calibre 방식)

- `g` → goto 다이얼로그 (비모달, Esc 닫기). **x / y / window** 를 um 단위로
  입력하고 Enter 또는 go — 해당 좌표로 이동하고 그 지점에 **X 마커**가
  표시된다 (렌더 도착 전에도 고정 프레임 위에 유지). `Esc` 로 마커 삭제.
- x/y는 현재 화면 중심으로 프리필. DRC 리포트의 `"x, y"` 쌍을 한 필드에
  통째로 붙여넣어도 된다 — 값은 x, y, window 순으로 채워지고, 쌍을
  붙여넣으면 남아 있는 y 프리필은 무시된다.
- **window** = 이동 후 뷰 폭(um). 비우면 현재 배율 유지. die 밖 좌표는
  뷰가 경계에 클램프되지만 마커는 요청한 지점에 남는다.
- **CLI**: `floe view <src> --goto X,Y[,W]` (um) — 시작과 동시에 해당
  위치를 보여준다 (W 생략 시 fit 배율). 이미 창이 떠 있으면 forward 되어
  실행 중인 창이 그 위치로 점프한다. DRC 리포트 좌표를 셸에서 바로
  넘길 때 사용.

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
- 렌더 요청(줌/팬/레이어/depth 변경) 제출 시 하단 상태바 우측에 "rendering…"
  인디케이터가 즉시 표시되고, 1.5초를 넘기면 경과 초가 붙는다. 렌더 중에도
  팬/줌 가능하며, 밀린 요청은 최신 것만 처리된다.
- 인스턴스 소켓은 `GLib.io_add_watch`로, 결과 큐는 `GLib.timeout_add`(25ms)로
  서비스한다. UI 라벨은 English only (XQuartz 한글 글리프 부재 — flateyes 규칙).
- 키: `f` fit, `+`/`-`(`=`) 줌, `r` ruler, `m` 스냅, `d` depth 다이얼로그,
  `g` goto 다이얼로그, `0`-`9` depth, `a` depth auto, `Esc` 단계 해제.

### depth (계층 표시 깊이)

Calibre DESIGNrev의 depth와 동일한 개념. 0 = 설계 top 셀의 shape만 표시
(하위 셀은 외곽 프레임 + 셀 이름), N = N 단계 아래까지 전개, 999 = 전체.
KLayout `LayoutView.max_hier_levels`로 구현하며, 타일 모자이크가 만드는
내부 2단계(FLOE_MOSAIC→TILE)는 오프셋으로 숨겨서 사용자에게는 원본 설계
계층 기준으로 보인다. `d` 키 → 다이얼로그(프리셋 0/1/2/3/full/auto +
스핀박스, 비모달이라 열어둔 채 실시간 조정), `render --depth` 지원.
현재 depth는 하단 상태바 우측에 항상 표시된다 (`depth: auto(2)` 형태).
**단축키: `d` = 다이얼로그 (Calibre와 동일), 숫자 `0`~`9` = 해당 depth,
`a` = auto 복귀.** full은 다이얼로그의 full 프리셋/스핀박스(999)로 지정.

- 기본은 **auto**: 인덱싱 때 저장한 타일별 depth-밀도 테이블로 "그 depth의
  도형 수 + 컷 레벨의 셀 프레임 수"를 추정해, 예산(약 12만 개) 안에 드는
  가장 깊은 depth를 렌더마다 자동 선택한다 (상태바에 `depth auto:N` 표시).
  프레임 비용 덕에 비트셀 어레이를 프레임 수백만 개로 자르는 어중간한
  depth는 자동으로 건너뛴다. 국소 영역 조회는 대부분 예산 아래라 full로
  렌더되고, 넓은 뷰에서만 자동으로 얕아진다. 숫자키/스핀박스/full 버튼으로
  명시하면 그 값으로 고정되고, `a` 키나 auto 버튼으로 복귀.
- 뷰에 걸린 모든 타일이 지원하는 depth의 렌더는 풀 타일 대신
  `tiles_lod/` 축약 타일을 로딩하므로 첫 방문 영역도 빠르게 뜬다
  (풀 타일 로딩은 깊은 줌에서만).
- far view(스켈레톤)에도 반영된다: 기본(auto)은 depth 0 모습(블록
  아웃라인 + 이름 + 탑 대형 도형)만 보여주고 — 최초 로딩 화면 포함 —
  명시적 depth k(1/2)면 레벨 k까지의 스켈레톤 디테일(파워 스트랩,
  라벨 등)을, full이면 전부 그린다. 라이브 depth 의미와 일관됨.
- 타일 경계에서 잘린 셀은 변형본 이름(`BLK_0_0$1` 등)으로 표시될 수 있다.
- 주의: 대형 어레이 영역에서 어중간한 depth(비트셀 프레임 수백만 개를
  외곽선으로 그리는 경우)는 full depth보다 느릴 수 있다.

## 폐쇄망 리눅스 배포

floe는 순수 파이썬. 의존성: `klayout`, `numpy` pip 휠 + GUI는
PyGObject/GTK3 (**RHEL 계열 GNOME 호스트에 기본 탑재** — flateyes와 동일하게
추가 설치 없음).

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

### .ice 구조와 설계 노트

```
<src>.ice/
  meta.json      원본 지문(size/mtime), 그리드, 레이어 테이블(+색),
                 타일별 depth-밀도 테이블(auto depth용), 통계
  tiles/t_r_c.oas  타일별 OASIS (절대좌표 유지, 전 레이어, 경계에서 절단)
  tiles_lod/...    depth 제한 축약 타일 - 누적 5만 도형 캡까지 레벨을
                   유지하고 그 아래는 고스트 bbox(254/0)로 대체, 타일별
                   지원 depth는 meta.lod.tiles에 기록. 얕은 depth 렌더는
                   풀 타일 대신 이것만 로딩 (캡 이하 타일은 파일 생략)
  skeleton.oas   줌아웃용 구조 모델 - 블록 아웃라인(255/0) + 탑 대형 도형
                 (= depth 0 뷰), 레벨 k 셀의 스트랩·라벨은 트윈 레이어
                 (dt + k*30000)에 분리 저장 (depth >= k 명시 시 표시)
```

- **geometry는 타일 경계에서 잘린다** — 뷰어/영역분석 용도로는 무해하나
  경계에 걸친 원본 도형이 그대로 필요하면 `clip --exact` 사용.
- **텍스트 라벨**은 half-open 규칙으로 정확히 한 타일에만 저장 (clip의
  경계 중복 문제 회피).
- **어레이 재압축**: `Layout.clip`은 타일 경계에 걸린 셀 어레이(SRAM 등)를
  개별 인스턴스 수백만 개로 풀어버린다. 인덱서가 격자 패턴을 감지해 정규
  CellInstArray로 재구성한다 (정규 어레이는 OASIS 라운드트립에서 보존됨
  → 타일 로딩 50배 가속).
- `clip_into` 사용 시 타겟 레이아웃에 레이어를 미리 생성해야 한다.
  안 그러면 anonymous 레이어로 복사되어 OASIS writer가 통째로 버린다.

## 로드맵

1. ✅ 테스트용 대용량 OASIS 생성기 (`tools/gen_test_oasis.py`)
2. ✅ 공간 인덱스(.ice) + CLI (index/info/render/clip)
3. ✅ 네이티브 뷰어 (view): 영역 줌/팬/레이어 토글/depth/clip 저장
4. Calibre DRC RDB 파서/조회 (KLayout `rdb` 모듈) + 에러 영역 자동 clip/뷰
5. 대용량 스케일링: 인덱싱 시 레이어 그룹별 다중 패스(RAM 상한), 타일 병렬 빌드
6. 뷰어 개선: 중간 줌 레벨 피라미드, 셀/텍스트 검색, 마커 점프
   (좌표 이동(goto)은 ✅)
