# DRC 기능 개요 (Calibre 결과 리뷰 파이프라인)

개발 기간 2026-08-13 ~ 08-19. 목표: Calibre DRC 결과(.db, 사무실
실측 수백 MB~139GB)를 즉시 열고, Calibre RVE에 준하는 브라우징과
waive 리뷰를 floe 뷰어 안에서 끝내는 것.

이 문서는 **전체 그림과 사용법** 중심의 개요다. 규범(포맷 바이트
레이아웃·뷰어 동작 계약·게이트 정의)은 각 SPEC이 정본:
- 포맷: `SPEC-FORMATS.ko.md` (`<db>.ice` pack, `<deck>.rules.json`)
- 뷰어 계약: `SPEC-VIEWER.ko.md` §8b (DRC 브라우저 대용량 규약)
- 검증: `SPEC-VALIDATION.ko.md` (D1~D7, R1~R4+R3b)

## 1. 파이프라인 한눈에

```
results.db (Calibre ASCII, 정본)          deck.cal (SVRF 룰덱)
      │  floe-index drc (rust, 병렬)            │  python -m floe svrf
      ▼                                         ▼
results.db.ice (자기완결 pack)            deck.cal.rules.json (룰 메타)
      └───────────────┬─────────────────────────┘
                      ▼
        floe view chip.oas --drc results.db
        (브라우저 + 캔버스 오버레이 + waive 리뷰)
```

```sh
# 1회 변환 (뷰어 open .db…가 자동으로도 수행)
rust/target/release/floe-index drc results.db [--jobs N]
# 룰덱 메타 (새 덱은 --scan 먼저; 실런과 동일한 -D 세트 필수)
.venv/bin/python -m floe svrf deck.cal --scan
.venv/bin/python -m floe svrf deck.cal -D SWITCH...
# 실행
.venv/bin/python -m floe view chip.oas --drc results.db
```

## 2. .ice pack (v2, 레이아웃 버전 4) — 유일한 인덱스 포맷

- **자기완결**: 변환 후 .db 불필요. 크기 실측 원본의 1/3~1/4.5.
- **좌표**: 파일순 64에러 블록 varint 델타 스트림. 레코드에 서수
  없음 — **에러 번호 = 전역 파일순 순번**(Calibre RVE 동일)이
  `err_start + 블록 내 위치 + 1`로 파생되고 룰별 나열은 자동
  오름차순. 정수 dbu 좌표 무손실.
- **공간 쿼리**: 에러당 4B qbox(체크 bbox 위 256×256 격자) + 블록
  bbox 테이블 → `query_rect` 스트리밍(청크 → 희소/밀집 하이브리드
  → 후보 블록만 디코드, cap 조기 종료). 뷰포트 쿼리 0.1~3ms.
  `waived=` 필터는 쿼리 내부 cap 이전 적용(후필터는 cap 뒤 매칭
  누락 — D6b).
- **waive 리뷰**: 에러당 1B `[status]`(0=none, 1=waived,
  2=reserved) — 파일 재작성 없이 pwrite 제자리 수정. 룰당 u32
  `[wcount]` 카운터를 set_status가 증분 유지 → 필터 카운트 O(1).
  **재-pack 시 status 초기화** 주의.
- **병렬 빌드**: 바이트 구간 분할 + 체크 헤더 투기 동기화 + 이음새
  검증 — **--jobs 무관 동일 바이트**(D5). 95MB 8코어 0.2s.
  기록은 `<out>.tmpw` → 완성 후 rename(원자적) — 재-pack이 기존
  pack(열린 뷰어 mmap 포함)을 truncate하지 않고, 실패 시 부분
  파일을 남기지 않음.
- **레이아웃 규율**: 섹션 배치가 바뀌면 헤더 version을 올리고
  리더는 현재 값만 수용(구 pack을 새 리더가 조용히 오독해 소형
  쿼리만 빗나갔던 사고의 재발 방지 — 거부 시 재-pack 안내).
- **v1 오프셋 사이드카는 폐기**(08-19 확정): waive 저장·공간 쿼리
  불가, 원본 .db 상시 동반(139G 실측 = 20G 인덱스 + 원본 139G).
  잔존 v1 파일은 stderr 안내 후 ASCII 폴백, `floe-index drc`로
  재변환. 구 레거시 타일 캐시의 `.ice` 확장자는 `.tiles`로 개명 —
  `.ice`는 DRC 인덱스 전용.

## 3. 뷰어 DRC 브라우저 (왼쪽 pane 상시 내장)

**열기** — open .db… 다이얼로그(.db만 표시, 로딩은 pack만; 부재/
스테일/구 레이아웃이면 자동 인덱싱 + 모달 로그), `--drc` 시작
옵션. 로드 직후 = 무선택 상태. 정보줄:
`파일명 [pack v4] — cell · 표시/전체 룰 · 에러 · svrf N/M`.

**룰 목록** — 이름 + `에러수/waived`(예: 30/20, 둘 다 O(1)).
- 검색 박스(목록 상단): 이름 부분일치 실시간 필터.
- 유형 콤보: SVRF 측정문 metric별 분류(width/space/enclosure/
  area/density/…; rules.json 필요, 다중 측정문 룰은 각 유형 매칭).
- All/Not Waived/Waived 콤보: status 기준(All은 에러 0 룰도 표시).
- 세 필터는 **교집합**. 룰 선택 = 열기(한 번에 한 룰).

**에러 그리드** — 룰-로컬 번호(1부터; Calibre 전역 번호는 상세의
`#로컬(전역)`), 1000셀 페이지(◀ 페이지/전체 ▶), pane 폭에 맞춘
열 재배치, 숫자도 상태색(red=not waived / green=waived).

**에러 조작**
| 입력 | 동작 |
|---|---|
| 단클릭 | 상세 + 셀 마킹만 (뷰 불변) |
| 더블클릭 | 점프(에러가 화면 30% 프레이밍) + 자동 CD 룰러 + **레이어 격리**(rules.json의 원천 레이어만 켬, Esc 복원) |
| n / p | 보이는 목록(selected ∧ in view ∧ waive) 안에서 순환, 페이지 자동 이동 |
| `e` + 두 클릭 | 박스 선택(Esc까지 유지; Shift 추가, Ctrl 토글; 보이는 에러만; 룰별 보존; gold 표시) |
| Ctrl/Shift + 셀 클릭 | 선택 추가/토글 |
| 우클릭 | waive/unwaive 메뉴(gold 선택 시 일괄) — [status] 제자리 기록, 카운트·색·필터 즉시 연동 |
| 체크박스 | in view(공간 쿼리, 뷰 추적) · selected(선택만) |
| `b` | 레이어 흑백 토글(시인성) |
| Esc | 단계 해제(…에러 박스선택 → 격리 레이어 복원 → DRC 마크) |

**캔버스** — 열린 룰의 현재 그리드 페이지 에러를 상태색 실도형으로
상시 표시(엣지 2px 실선, 폴리곤 50% 스페클 채움). 화면 스팬이 마커
크기(5px) 미만이면 5×5 마커로 붕괴(포커스 9×9, 임계 = 마커 크기
자체 — 도형이 마커보다 작게 그려지는 구간 없음). 선택은 gold.

**상세 pane** — `#로컬(전역) [상태]` + 룰 텍스트 + edge/polygon
좌표(pathname/title 줄 제외). rules.json이 붙으면 §4의 메타 블록
추가. 좁은 pane에서는 줄바꿈(내용 잘림 금지 규약).

## 4. SVRF 룰 메타데이터 (.rules.json) — waive 판단 보조

**서브셋 파서**(`floe/svrf.py`): 지오메트리 연산 의미는 구현하지
않는다 — derivation은 우변 피연산자 이름만 그래프로 넣고, 체크의
원천 GDS 레이어는 그래프를 LAYER/LAYER MAP까지 폐쇄해서 얻는다.
- 전처리: INCLUDE 병합, #DEFINE/#IFDEF/#ELSE, VARIABLE·값 치환 —
  **실런과 동일한 -D 세트 필수**.
- 제약 추출 = **연속 체인 규칙**: 첫 비교연산자에서 시작하는 연속
  comparator+value 체인만(`> 0 < v` = 2건). 옵션 비교값(`ABUT<90`,
  `ABUT>0<90`, `OPPOSITE EXTENDED < x`)은 구조적으로 제외.
  비교연산자로 시작하는 다음 줄 = 직전 측정문의 연속(감싼 한계).
- 의도적 공백: DMACRO/CMACRO 비전개(카운트만), TVF는 생성된 SVRF를
  입력으로, 피연산자 줄바꿈 분리는 미지 히스토그램행. 새 덱은
  `--scan`(양쪽 #IFDEF 분기 워크, 인벤토리 출력)으로 구멍 확인.

**뷰어 연동**: db 옆 `<덱 베이스네임>.rules.json` 자동 부착(수동
`rules…` 버튼), 상세에 추가되는 블록:
```
constraint: ENC via1_drawn m6_drawn > 0 < 0.105
measured: 0.0399 um vs < 0.1050 · Δ -0.0651 (-62.0%)
layers: via1_drawn, m6_drawn
gds: 6/0, 15/0, 0/0, 63/63
derivation:
  via1_drawn = VIA1 NOT fill_excl
  fill_excl = FILLA OR FILLB
```
measured는 CD 룰러와 동일 판정(rect=min(w,h)·엣지쌍=갭·area=
신발끈; 복잡 도형 생략), 제약이 여럿이면 상한(<,<=,==) 우선.
그 외 더블클릭 레이어 격리와 룰 유형 필터의 분류 원천.

## 5. 성능 계약 (전부 실측으로 확정)

- 필터/목록 어디에도 **O(룰 크기) 경로 금지**: 카운트 = [wcount]
  O(1), 상태 필터 그리드 = lazy 서술자(`status_page`/`status_rank`
  — 전수 리스트 실체화 없음), 공간 필터 = query_rect 스트리밍.
- GTK 대량 재빌드 규칙: **모델 분리(set_model None) + 선택 핸들러
  busy 가드** — ListStore.clear()의 행별 자동 재선택이 머신에 따라
  핸들러를 행마다 실행(1000룰 필터 전환 9.7s 실측)했던 원인.
- 원격/타 머신 증상은 `FLOE_DRC_PROF=1`([drcprof] 단계별 시간)로
  계측을 보내 확진한다 — 추측 금지.

## 6. 테스트 자산과 게이트

```sh
# 실디자인 정렬 세트 (testchip_1g5.oas, 레이어 격리까지 검증)
.venv/bin/python tools/gen_drcdb.py data/testchip_1g5.drc.db \
  --checks 96 --max-errors 400 --die 0,0,10500,10500 --seed 7 \
  --layers M1,M2,M3,M4,M5,M6,VIA1,VIA2,POLY,ACTIVE,CONT,NWELL \
  --svrf-gds "M1=5/0,M2=7/0,M3=9/0,M4=11/0,M5=13/0,M6=15/0,\
VIA1=6/0,VIA2=8/0,POLY=3/0,ACTIVE=2/0,CONT=4/0,NWELL=1/0,\
FILLA=0/0,FILLB=63/63" \
  --pathname testchip.drc.cal --svrf data/testchip.drc.cal
# 부하 세트: drctest(1000룰) / drcbig(--zeros 20 --heavy 12만~25만)
```
`gen_drcdb.py`는 **룰 종류와 일치하는 에러 도형**(SPACE=엣지쌍 갭,
WIDTH=얇은 사각형, AREA=면적 미달, DENSITY=50×50 윈도)을 한계 미달
치수로 생성하고, `--svrf` 동반 덱은 한계 표기를 5가지 실덱 스타일
(공백/붙임/`> 0 <v`/VARIABLE/줄바꿈)로 순환한다.

게이트(validate_rust.sh 스위트): `validate_drc_ice.py` **D1~D7**
(pack==ASCII 적대 픽스처 · 디스패치/폐기 v1 폴백 · dedup+lazy ·
gen 왕복 · jobs 불변 · query_rect 브루트포스 대조 · status/wcount),
`validate_svrf.py` **R1~R4+R3b**(전처리 · 그래프 폐쇄 · 추출
엣지케이스 · 실덱 표현식 · 엔드투엔드 매칭).

## 7. 미결 (사무실/실데이터 대기)

- 139GB 실덱 `floe-index drc` 실측: 크기(목표 1/10~1/30) ·
  시간(구 16분 대비) — 부족하면 도형 템플릿 사전·블록 압축 검토.
- 실제 Calibre의 waived 표기법 파악 → 매핑(현재 floe [status]가
  유일한 waive 정본, 실데이터는 전부 Not Waived로 취급).
- 실덱 `--scan`으로 SVRF 파서 스코프 보정(CMACRO 사용 여부,
  미지 문장 히스토그램).
- S4: known/intended 에러 분류, 기준내(Δ 여유) 자동 필터, waive
  보조 워크플로 — 실덱 데이터 확보 후 별도 계획.
