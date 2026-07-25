# zenoas

대용량 OASIS 설계 파일을 빠르게 조회/클립하는 경량 유틸리티.

수십~수백 GB의 원본 설계 파일을 매번 전체 로딩하지 않고,

- 특정 **레이어**만 추출하거나
- 특정 **영역(bbox)** 만 클립해서 별도 OASIS 파일로 저장
- Calibre DRC 결과 DB(RDB)를 참조해 **에러 영역을 조회/분석**

하는 것을 목표로 한다. 타겟 환경은 Linux, 개발/테스트는 macOS.

## 환경 설정

```sh
python3 -m venv .venv
.venv/bin/pip install klayout numpy
```

- KLayout pip 모듈(GUI 없는 `klayout.db`)을 엔진으로 사용. Python 3.14 + klayout 0.30.9 확인됨.

## 테스트 데이터 생성

실제 설계 파일이 없으므로, 실제와 유사한 구조의 대용량 OASIS를 합성 생성한다.

```sh
.venv/bin/python tools/gen_test_oasis.py --out data/testchip_1g5.oas --target-gb 1.5
```

생성되는 레이아웃 구조:

- `ZENOAS_TESTCHIP` (top) → 6×6 블록 그리드 (블록 1.5mm, die ~10.1mm)
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

## zenoas 사용법

OASIS에는 공간 인덱스가 없어 어떤 조회든 파일 전체 파싱이 필요하다.
zenoas는 **최초 1회 인덱싱**으로 이 비용을 지불하고, 이후 모든 조회는
관심 영역과 교차하는 타일만 로딩해 ms~초 단위로 응답한다.

```sh
alias zn=".venv/bin/python -m zenoas"

zn index data/testchip_1g5.oas          # 1회: <src>.zncache/ 생성
zn info  data/testchip_1g5.oas          # 레이어/그리드/통계 요약
zn view  data/testchip_1g5.oas          # 네이티브 데스크톱 뷰어 (기본)
zn render data/testchip_1g5.oas --bbox 5000,5000,5200,5200 \
          --layers M2,M3,VIA2 --out view.png
zn clip  data/testchip_1g5.oas --bbox 5000,5000,5100,5100 --out region.oas
```

- `--bbox`는 µm 단위 `X0,Y0,X1,Y1`. `--layers`는 이름 또는 `layer/datatype` 목록.
- `clip --exact`: 원본 파일을 다시 파싱하는 느린 경로 (타일 경계에서 잘리지 않은
  원본 geometry가 필요할 때).
- 네이티브 뷰어 조작 (Calibre DESIGNrev 방식):
  **좌드래그 = 영역 줌** (정방향 →: 박스 영역으로 줌인, 역방향 ←: 박스/화면
  비율만큼 줌아웃 — 역방향일 때 밴드가 주황색으로 표시),
  **좌클릭 = object picking**, **중/우클릭 드래그 = 팬**,
  휠·트랙패드 = 커서 기준 줌(보조), `f` fit.

### Ruler (거리 측정, flateyes와 동일한 UX + 벡터 스냅)

- `r` 측정 모드 시작/종료. 두 점을 클릭하면 측정선이 확정되어 남는다
  (여러 개 유지). 기본은 수평/수직 축 스냅(우세한 축 자동 선택),
  `Shift` = 자유 각도.
- **벡터 스냅** (`m` 토글, 기본 켜짐): flateyes의 이미지 밝기 스냅 대신
  OASIS geometry의 **꼭짓점/엣지에 정확히 붙는다** — 꼭짓점 우선, 없으면
  엣지 위 최근접점. 커서 주변 스냅 위치가 십자 마커로 미리 표시된다
  (꼭짓점=녹색, 엣지=파랑). 확대(live) 상태에서 동작.
- 측정 중 상태바에 실시간 길이/dx/dy 표시. `Esc` 는 단계적으로 해제:
  진행 중 점 → 선택 → 측정 모드 → 확정된 측정선 전체 삭제.

### Object picking (Calibre 방식)

- 확대(live) 상태에서 **좌클릭**: 클릭 지점을 포함하는 도형 중 **가장 작은
  것**이 선택되어 흰 외곽선으로 하이라이트되고, 상태바에 레이어 이름
  layer/datatype · 셀 이름 · 크기 · 좌표 · (순번/전체) 가 표시된다.
- **같은 지점을 다시 클릭하면 겹친 도형들을 차례로 순환** (fill 아래의
  배선, 그 아래의 boundary 순). 텍스트 라벨도 선택 대상. `Esc` 해제.
- 보이는(체크된) 레이어만 대상. 타일 경계에서 잘린 도형은 잘린 조각
  기준으로 선택된다 (셀 이름에 `$1` 변형 표기 가능).
  줌아웃 상태에서는 인덱싱 때 미리 렌더한 오버뷰 PNG를 표시하고, 확대하면
  뷰포트와 교차하는 타일만 lazy 로딩해 실시간 렌더링한다 (LRU 캐시로
  메모리 상한 유지 → 원본 크기와 무관하게 동작).

### 단일 인스턴스 동작 (flateyes와 동일)

zenoas는 이미지 뷰어 flateyes의 OASIS 버전으로, 인스턴스 모델을 그대로 따른다:

- **(uid, DISPLAY)당 뷰어 창 1개.** 첫 실행이 창을 열고, 같은 DISPLAY에서의
  이후 `zn view 다른파일.oas` 는 실행 중인 창에 경로를 넘기고 즉시 종료한다
  (exit 0, ~0.1초 — forward 경로는 klayout/tkinter를 import하지 않음).
  기존 창이 해당 파일로 전환되고 앞으로 올라온다.
- **DISPLAY 값이 다르면 독립 창** — 한 리눅스 호스트에서
  `DISPLAY=:1 zn view a.oas` 처럼 여러 사용자 DISPLAY로 각각 실행 가능.
- `--multi`: 단일 인스턴스를 끄고 항상 독립 창을 연다 (소켓 미사용).
- 소켓: 리눅스는 abstract namespace(`\0zenoas-<uid>-<display>`, stale 불가),
  그 외는 파일 소켓(+probe 후 unlink로 stale 처리). macOS 개발 환경은
  DISPLAY 없이 'aqua' 키로 동일하게 동작.
- 종료 코드: 0 = 정상/전달됨, 1 = DISPLAY 미설정·소켓 실패,
  **3 = X 디스플레이 접속 불가** (죽은 세션, xauth 불일치 등).
- 전달 대상 파일의 인덱스가 없으면 **forward 전에 이 터미널에서** 먼저
  인덱싱한다 (GUI 프로세스가 몇 분씩 멈추는 것 방지).

### 네이티브 뷰어 (`zn view`)와 툴킷 선택

GUI는 **tkinter** 기반이다. 선택 이유 (GTK 대비):

- CPython 표준 라이브러리라 폐쇄망에서 OS 패키지 하나로 끝
  (RHEL: `python3-tkinter`, Debian: `python3-tk`) — PyGObject/GTK 버전 매칭,
  gobject-introspection 의존성 사슬이 없음.
- 무거운 geometry 렌더링은 어차피 klayout C++ 엔진(`klayout.lay` 헤드리스)이
  **별도 렌더 프로세스**에서 PNG 프레임으로 수행. GUI는 비트맵 표시 + 마우스
  이벤트만 담당하므로 툴킷 성능이 병목이 아님 (UI 응답성은 레이아웃 크기와 무관).
  스레드가 아닌 프로세스인 이유: klayout 렌더 루프가 GIL을 잡고 있어 스레드로는
  긴 렌더 동안 메인 루프가 얼어붙는다 (spawn 방식이므로 `__main__` 가드 필수).
- 렌더 요청(줌/팬/레이어/depth 변경)이 제출되면 하단 상태바 우측에
  "rendering…" 인디케이터가 즉시 표시되고, 1.5초를 넘기면 경과 초가 붙는다.
  렌더 중에도 팬/줌 가능하며, 밀린 요청은 최신 것만 처리된다.
- 뷰어 코어(`viewport.py`의 타일 모자이크, `render.py`)는 툴킷 독립적 —
  GTK/Qt 셸이 필요해지면 `gui.py`(~400줄)만 교체하면 된다.
- 키: `f` fit, `+`/`-`(`=`) 줌. 단축키 외 조작은 마우스.

### depth (계층 표시 깊이)

Calibre DESIGNrev의 depth와 동일한 개념. 0 = 설계 top 셀의 shape만 표시
(하위 셀은 외곽 프레임 + 셀 이름), N = N 단계 아래까지 전개, 999 = 전체.
KLayout `LayoutView.max_hier_levels`로 구현하며, 타일 모자이크가 만드는
내부 2단계(TV_MOSAIC→TILE)는 오프셋으로 숨겨서 사용자에게는 원본 설계
계층 기준으로 보인다. GUI "depth" 버튼 → 다이얼로그(프리셋 0/1/2/3/full +
스핀박스, 비모달이라 열어둔 채 실시간 조정), `render --depth` 지원.
**단축키 (Calibre와 동일): 숫자 `0`~`9` = 해당 depth, `a` = 전체(full).**

- live 렌더에만 적용된다 (줌아웃 오버뷰 PNG는 인덱싱 때 full-depth로 고정).
- 타일 경계에서 잘린 셀은 변형본 이름(`BLK_0_0$1` 등)으로 표시될 수 있다.
- 주의: 대형 어레이 영역에서 어중간한 depth(비트셀 프레임 수백만 개를
  외곽선으로 그리는 경우)는 full depth보다 느릴 수 있다.

## 폐쇄망 리눅스 배포

zenoas는 순수 파이썬 (의존성: `klayout`, `numpy` pip 휠 + tkinter).

```sh
# 1) 인터넷 PC에서 휠 수집 (타겟 파이썬 버전에 맞춰)
pip download klayout numpy -d wheels/ \
    --platform manylinux2014_x86_64 --only-binary=:all: \
    --python-version 311        # 예: 타겟이 python3.11

# 2) 폐쇄망 호스트에서
sudo dnf install python3-tkinter          # 내부 미러 RPM (GUI 사용 시)
pip install --no-index --find-links wheels/ klayout numpy
# zenoas/ 디렉토리 복사 후: python3 -m zenoas ...
```

- CLI 전용(`index/info/render/clip`)이면 tkinter 없이도 동작한다.
- 원격 서버에서는 X11 포워딩(`ssh -X`)으로 `zn view`를 실행할 수 있다.

### .zncache 구조와 설계 노트

```
<src>.zncache/
  meta.json      원본 지문(size/mtime), 그리드, 레이어 테이블(+색), 통계
  tiles/t_r_c.oas  타일별 OASIS (절대좌표 유지, 전 레이어, 경계에서 절단)
  overview/*.png   레이어별 full-die 렌더 (줌아웃용)
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
2. ✅ 공간 인덱스(.zncache) + CLI (index/info/render/clip)
3. ✅ 네이티브 뷰어 (view): 영역 줌/팬/레이어 토글/depth/clip 저장
4. Calibre DRC RDB 파서/조회 (KLayout `rdb` 모듈) + 에러 영역 자동 clip/뷰
5. 대용량 스케일링: 인덱싱 시 레이어 그룹별 다중 패스(RAM 상한), 타일 병렬 빌드
6. 뷰어 개선: 중간 줌 레벨 피라미드, 좌표 이동/검색, 마커 점프
