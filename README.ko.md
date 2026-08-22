# floe — 사용자 가이드

대용량(수십~수백 GB) OASIS 설계 파일을 Calibre DESIGNrev 급 반응성으로
열람·계측·클립하는 뷰어/인덱서. 한 번 인덱싱(.floe 캐시)하면 이후에는
러스트 데몬(vfsd)이 뷰포트 단위로 필요한 페이지만 공급한다.

- 타겟: Linux(사무실), 개발/테스트: macOS
- 구성 문서: [ARCHITECTURE.ko.md](ARCHITECTURE.ko.md),
  [docs/SPEC-*.ko.md](docs/), DRC 기능 개요:
  [docs/DRC.ko.md](docs/DRC.ko.md), DRC 스크립팅 CLI:
  [docs/DRC-CLI.ko.md](docs/DRC-CLI.ko.md), 러스트 렌더러 대체
  테스트 계약: [docs/RENDERER-TESTS.ko.md](docs/RENDERER-TESTS.ko.md),
  플래너 정본 이력: `rust/VFS_HIER.md`

## 1. 설치

```sh
# 러스트 바이너리 (floe-index: 인덱서+데몬)
export PATH="$HOME/.cargo/bin:$PATH"
cd rust && cargo build --release        # → rust/target/release/floe-index

# 파이썬 (뷰어). gi(PyGObject)는 OS 패키지로!
python3 -m venv --system-site-packages .venv
.venv/bin/python -m pip install klayout numpy   # (pip 셔뱅 깨짐: python -m pip 사용)
```

- GTK3/PyGObject는 pip 금지, OS 패키지 사용. CLI 전용이면 GTK 불필요.
- Linux 배포는 `tools/make_portable.sh`(conda-forge GTK 포터블 번들,
  Linux에서 빌드). 참조 구현: `../flateyes/make_portable.sh`.
- 확인된 조합: Python 3.14 + klayout 0.30.9.

## 2. 인덱싱

```sh
rust/target/release/floe-index vfs <design.oas> [outdir] [옵션]
# outdir 기본값: <design.oas>.floe
```

| 옵션 | 의미 |
|---|---|
| `--jobs N` | 워커 수 (기본: CPU 코어) |
| `--plan-batch N` / `--encode-batch N` | 파이프라인 윈도/인코드 배치 |
| `--page-target-mb N` | 페이지 목표 크기 (기본 1MiB) |
| `--no-lod` | LOD 변종 생성 생략 (빌드 −5분@9.8G, 기본은 LOD 포함) |
| `--coverage` / `--coverage-only` | 밀도 커버리지(design.ovc) 생성/추가 |
| `--frontier-only` | **재인덱싱 없이** meta.json의 미니맵 프런티어만 재굽기(초 단위) |
| `--slow-cell-s S` | slow-cell 로그 임계 초 (기본 5.0, 0 = 전 셀 — 스레드/팬아웃 계측용) |
| `--p2-shard-limit-mb N` | P2 arena 샤딩 복사 상한. 미지정 시 Linux는 MemAvailable 여유의 절반, **macOS는 무제한** — Mac에서 대용량 인덱싱 시 지정 권장 |
| `--kill-at <지점>` | 게이트 전용 장애 주입 |

산출물(`<src>.floe/`): `design.ovm`(메타·인덱스), `design.ovp`(페이지
페이로드), `design.ovt`(텍스트), `meta.json`(뷰어 요약+미니맵 프런티어),
선택적 `design.ovc`(커버리지). 소스 크기/mtime이 캐시에 박히므로 소스가
바뀌면 재인덱싱 필요.

## 3. 뷰어 실행

```sh
.venv/bin/python -m floe view [design.oas] [옵션]   # 파일 생략 = 빈 뷰어
.venv/bin/python -m floe                            # 인자 없음 = view
```

| 옵션 | 의미 |
|---|---|
| `--goto X,Y[,W]` | (µm) 지점 센터링(+뷰 폭 W). 실행 중 인스턴스로 전달됨 |
| `--depth N` | 시작 깊이 (기본 0; 999=full; --goto 시 full) |
| `--detail low\|medium\|high` | 시작 디테일 (기본 medium) |
| `--lod on\|off`, `--frames on\|off`, `--labels on\|off` | 시작 토글 |
| `--drc FILE.db` | Calibre ASCII DRC 결과 브라우저 프리로드 (옆에 신선한 `FILE.db.ice` 인덱스가 있으면 자동 사용 — 수백 GB도 즉시 오픈) |
| `--stream-kb KB` | 점진 페인트 라운드 예산 고정 (0=비활성; 기본 적응형 24576) |
| `--multi` | 단일 인스턴스 소켓 무시하고 새 창 |

터미널에는 스트리밍 라운드마다 `[perf] gen=.. round=.. new=.. bytes=..
plan=..ms delta=..ms apply=..ms draw=..ms total=..ms lod=.. refining=..
settled=..` 한 줄이 상시 출력된다(라운드별 비누적 — 성능 실측용).

## 4. 키맵

상단 **메뉴 바**(File/View/Ruler/DRC/Help)가 아래 명령 전부와
fit·clip·open .db·rules 기능을 노출한다(구 패널 버튼들은 메뉴로
이전, 2026-08-22) — 라벨에
키가 병기되고, 토글류는 메뉴를 열 때 현재 상태가 체크로 표시된다.
**File > load layout…** = 인덱싱된 소스를 실행 중에 열기(빈 뷰어로
시작했을 때의 첫 로드 포함 — 미인덱스 파일은 floe-index 안내).

| 키 | 동작 |
|---|---|
| `0`~`9` | 해당 깊이로 설정. **`9` 두 번(1초 내) = full depth** |
| `<` / `>` | 깊이 −1 / +1 (0..max_depth 클램프) |
| `d` | 디테일 다이얼로그 (low/medium/high = 컷 5/3/1px) |
| `f`, `Shift+C` | 프레임(cell reference outline) on/off |
| `l` | LOD on/off · `v` | 커버리지 on/off · `a` | abstract 모드 |
| `b` | 레이어 흑백(그레이스케일) on/off — DRC 시인성용(독립 토글) |
| `+`/`=` / `-` | 중심 줌 ±(1.25×) |
| **Ctrl+Z** / **Shift+Z** | Calibre 줌인 50%(스팬 ×0.5) / 줌아웃 50%(×2) |
| **Ctrl+A** | Zoom All(fit) — 줌아웃은 fit의 **16배**까지 허용 |
| **Ctrl+C** | 현재 화면(오버레이 포함 그대로)을 이미지로 클립보드 복사 — 뷰어 실행 중에만 유지(flateyes와 동일) |
| **Tab** | 오버레이 3-상태 순환: 모두 표시 → **현재(점프된) 에러 외 에러 숨김**(마크·룰러 유지) → 전부 숨김(룰러·마크·마커·선택·스냅) → 모두 표시 |
| 커서키 / **Ctrl+커서키** | 팬 50% / 10% |
| 휠 | 커서 기준 줌 (이벤트당 4%) |
| 우클릭 드래그 | 러버밴드 줌 (전방=줌인, 후방=줌아웃; 흰 1px 실선) |
| `r` | 룰러 모드 (크로스헤어 커서; 클릭 2점, Shift=자유각, `m`=스냅 토글) |
| `k` / **Shift+K** | 마지막 룰러 삭제 / 전체 룰러 삭제 (룰러는 다중 누적) |
| `g`, **Ctrl+.** | goto 다이얼로그 |
| `e` | **에러 박스 선택 모드**(룰러처럼 **Esc까지 유지** — 박스 반복 가능): 클릭 2점으로 박스 → **화면에 보이는(현재 필터·페이지) 에러** 중 박스 안의 것 선택(gold). 두 번째 클릭에 **Shift = 추가**, **Ctrl = 토글**, 무수식 = 대체. 그리드도 동일(Ctrl 토글/Shift 범위) — 모든 선택은 보이는 에러만 대상 |
| `n` / `p` | **현재 보이는 목록**(selected/in view/waive 필터 적용분) 안에서 다음 / 이전 순환 (페이지 자동 이동) |
| `w` | **waive 토글** — gold 선택이 있으면 **선택 전체 일괄**(전부 waived면 해제, 아니면 전부 waive), 없으면 현재 에러(단클릭/n·p 포커스, 없으면 점프 에러). v2 pack 필요 |
| `Esc` | 단계별 해제: 에러선택 모드→찍던 점→룰러모드→룰러들→선택→에러 박스선택→격리 레이어 복원→DRC 마크 |
| `q` | 종료 확인 |

마우스: 좌클릭 = pick(겹침 순환), 좌/중 드래그 = 팬, 미니맵 클릭 = 센터링.

## 5. 패널 구성

- **왼쪽 pane**: **DRC 브라우저 전용**(열기는 DRC 메뉴 ·
  룰 검색(목록 상단)+룰 목록[이름 · 에러수/waived]|에러 번호 그리드 ·
  하단 에러 상세 — **상세는 최소 150px 상시 확보**; db 로드 시
  pane이 자동으로 넓어지고, 좁히면 이름은 "…" 줄임·상세는 줄바꿈·
  필터 행은 다단 래핑으로 대응). cell/object 브라우저는 추후 같은
  자리에 추가 예정.
- **미니맵**(depth별 구조 프런티어 — 인덱싱 때 실제 플래너로
  구워짐)은 우측 pane 하단 **탭**으로 이동(2026-08-22): minimap
  탭(기본) | palette 탭(색 7×7 + fill 5×4), 클릭 = 센터링.
- **중앙**: 캔버스. 상태줄에
  `live (N tiles, +N new, N ms = N load [plan+delta+apply] + N draw …)`
  와 `depth: */13`(full은 `*`) · `detail: medium` · cov/lod/frame 상태.
- **오른쪽 pane**: 레이어 목록(Calibre식 `l.d ■ NAME`, 숨김 = 취소선만,
  더블클릭 = 표시 토글, 클릭 = 선택(Ctrl/Shift 다중), 우클릭 = 메뉴),
  하단 탭 = **minimap(기본) | palette(색 7×7 + fill 5×4)**
  (fit/clip은 메뉴).

### 색/패턴 팔레트

- 레이어 행 선택 → 색 스와치 클릭 = 즉시 recolor (폴딩된 그룹 부모는
  멤버 전체 적용). 패턴 스와치 좌클릭 = fill 지정.
- 레이어 우클릭 메뉴: **load/save layer properties…**(임의 경로의
  Calibre layerprops 로드/내보내기). 디자인 디폴트 발행(save colors+
  fills as design default)은 개발용 — `FLOE_FILL_EDIT=1`에서만 노출.
  자세한 규칙은
  [docs/SPEC-PERSONALIZATION.ko.md](docs/SPEC-PERSONALIZATION.ko.md).

## 6. 환경변수 노브

| 변수 | 의미 |
|---|---|
| `FLOE_HAIRLINE` | rev 41 헤어라인 계수 (기본 0.5, 0=off) |
| `FLOE_THIN_UM` | rev 45 프레임 격자 피치 µm (기본 7.0, **0 = rev41+43 완전 복원**) |
| `FLOE_XQUARTZ` 계열 | XQuartz XRender 흑화 우회 (런처가 `CAIRO_DEBUG=xrender-version=-1` 설정) |
| `FLOE_CLICK_DEBUG` | 팔레트/미니맵 클릭 경로 stderr 트레이스 (`[click]` 줄) |
| `FLOE_PANEL_DEBUG`, `FLOE_MOSAIC`, `FLOE_WS`, `FLOE_CLIP`, `FLOE_REGION`, `FLOE_TESTCHIP` | 디버그/테스트용 |

## 7. 기타 CLI

`python -m floe <cmd>`: `index`(레거시 타일 캐시 .tiles), `info`,
`render --bbox … --out view.png`, `clip --bbox …`, `probe`, `profile`,
`drc`(요약; .ice 인덱스 자동 사용), `svrf`(SVRF 룰덱 서브셋 파스 →
`<deck>.rules.json`), `gtktest`. `floe-index`: `scan`,
`tile`, `index`, `vfs`, `plan`(플래너 계측 JSON), `vfsd`(데몬),
`drc`(DRC 인덱스 사이드카 굽기).

### DRC 결과 인덱스 (.ice)

수백 MB~수백 GB Calibre ASCII DRC 결과(.db)는 자기완결 **pack**으로
한 번 변환하면 뷰어/CLI가 mmap으로 즉시 연다:

```sh
rust/target/release/floe-index drc results.db [--jobs N]
.venv/bin/python -m floe view chip.oas --drc results.db
```

- 실측 1/4~1/5 크기, 변환 후 .db 불필요. 병렬 빌드(--jobs 무관 동일
  바이트).
- 위치 쿼리 내장 → DRC 브라우저의 **in view** 필터: 선택한 룰의
  에러 중 현재 화면 안에 있는 것만 나열(상한 1000, 뷰 추적).
- 에러당 1B 리뷰 상태 내장 — 그리드 **우클릭 → waive/unwaive**
  (gold 선택 시 일괄)로 제자리 기록, All/Not Waived/Waived 필터와
  룰 행의 `에러수/waived` 카운트에 연동(재-pack 시 초기화 주의).

구 **v1 오프셋 사이드카는 폐기**됐다(2026-08-19 — waive 저장·공간
쿼리 불가, 원본 .db 상시 필요): 남아 있는 v1 .ice는 안내 후 ASCII
폴백되니 `floe-index drc`로 재변환하면 된다. 소스가 바뀌어도
(size/mtime) 동일하게 폴백+재변환. **뷰어의 open .db… 다이얼로그는
.db만 보여주지만 로딩은 항상 pack(.ice)으로 한다** — pack이 없거나
오래되면 그 자리에서 인덱싱을 돌리고 로그를 모달 창에 보여준 뒤
연다. 참고: 구 레거시 타일 캐시가 쓰던 `.ice` 확장자는 `.tiles`로
개명되어 이제 `.ice`는 DRC 인덱스 전용이다.

### SVRF 룰 메타데이터 (.rules.json)

DRC 에러의 waive 판단을 돕기 위해 SVRF 룰덱을 **서브셋 파스**해서
룰별 제약(연산자·수치)·참조 레이어·원천 GDS 레이어를 뽑아 둔다
(지오메트리 연산은 구현하지 않음 — derivation은 피연산자 이름
그래프만):

```sh
.venv/bin/python -m floe svrf sfa14.drc.cal --scan     # 새 덱: 먼저 인벤토리
.venv/bin/python -m floe svrf sfa14.drc.cal -D FEOL    # 실런과 같은 -D 세트!
# -> sfa14.drc.cal.rules.json (뷰어가 로드하는 사이드카)
```

만든 `.rules.json`을 **.db 옆에 두면** 뷰어가 db를 열 때 자동으로
붙인다(DRC 메뉴 load SVRF rules…로 수동 로드도 가능, 정보줄 `svrf N/M` =
매칭 룰 수). 이후 에러 상세에 덱 원문 제약, **이 에러 자체의 측정
치수 vs 한계값(Δ·%)**, 참조/원천 레이어, derivation 체인이 붙고,
**에러 더블클릭 점프가 그 룰의 원천 레이어만 켜고 나머지를 끈다**
(레이어 격리 — Esc로 이전 가시성 복원, 상태줄에 켜진 수 표시).
nav 행의 **룰 유형 콤보**는 SVRF 측정문 기준 분류(width/space/
enclosure/area/density/…)로 룰 목록을 거른다 — 검색·waive 필터와
교집합, 측정문이 파스되지 않은 룰은 other.
TVF(Tcl) 덱은 Calibre가 생성한 SVRF 산출물을 입력으로 쓰고,
DMACRO/CMACRO는 전개하지 않으니 `--scan`으로 사용 여부부터 확인.

레이어 격리를 실제 디자인과 연동해 시험하려면 `gen_drcdb.py`의
`--layers`/`--svrf-gds`(디자인 실제 gds 번호로 덱 정렬)/
`--pathname`(db가 가리킬 덱 이름) 옵션으로 디자인 맞춤 db+덱을
만든다. testchip_1g5 예:

```sh
.venv/bin/python tools/gen_drcdb.py data/testchip_1g5.drc.db \
  --checks 96 --max-errors 400 --die 0,0,10500,10500 --seed 7 \
  --layers M1,M2,M3,M4,M5,M6,VIA1,VIA2,POLY,ACTIVE,CONT,NWELL \
  --svrf-gds "M1=5/0,M2=7/0,M3=9/0,M4=11/0,M5=13/0,M6=15/0,\
VIA1=6/0,VIA2=8/0,POLY=3/0,ACTIVE=2/0,CONT=4/0,NWELL=1/0,\
FILLA=0/0,FILLB=63/63" \
  --pathname testchip.drc.cal --svrf data/testchip.drc.cal
rust/target/release/floe-index drc data/testchip_1g5.drc.db --pack
.venv/bin/python -m floe svrf data/testchip.drc.cal
.venv/bin/python -m floe view data/testchip_1g5.oas \
  --drc data/testchip_1g5.drc.db
```

## 8. 검증

```sh
sh tools/validate_rust.sh    # 전체 게이트 (H/L/S/X/렌더/마커, ALL OK 필수)
cd rust && cargo test --release
```

## 9. 문제 해결

- **XQuartz에서 이미지가 검게 나옴**: XRender 합성 버그. 런처가 자동
  우회하며, 진단 사다리는 selfcheck→render→probe→gtktest (`FLOE_DUMP`).
- **`open: size/mtime mismatch`**: 소스가 바뀜 → 재인덱싱.
- **DRC가 stale index 경고 후 느리게 열림**: .db가 갱신됨 →
  `floe-index drc results.db` 재실행.
- **미니맵 누락**: 구 캐시 → `--frontier-only`로 초 단위 재굽기.
- **콤보 박스가 마우스만 살짝 움직여도 닫힘**: GTK 메뉴 그랩이
  XQuartz/원격 X에서 깨지는 증상 — 콤보 팝업을 리스트 모드로 전역
  전환해 완화(`-GtkComboBox-appears-as-list`). 그래도 재현되면
  XQuartz 환경설정에서 "Focus Follows Mouse"(포커스가 마우스를
  따름)를 꺼야 한다(`defaults write org.xquartz.X11 wm_ffm -bool
  false` 후 XQuartz 재시작).
- **pip 실행 오류**: venv의 pip 셔뱅 깨짐 → `.venv/bin/python -m pip`.
