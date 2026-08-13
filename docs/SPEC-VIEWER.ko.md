# SPEC: 뷰어 (floe/gui.py · service.py · viewport.py · render.py)

## 1. 프로세스/스레드 모델

- GUI(GTK 메인루프)와 RenderWorker(spawn 프로세스, klayout 소유)가
  mp.Queue(job/res) + `latest` 공유값(최신 렌더 gen — 낡은 잡 즉시
  포기)으로 통신. GUI 폴링 루프가 res를 디스패치.
- 잡 종류: `render`, `pick`, `snap`, `clip`, `recolor`, `repattern`,
  (`frontier`는 46b에서 제거 — 미니맵은 meta 소비).
- refine(스트리밍 라운드) 중에도 내부 드레인 루프가 pick/snap/clip/
  recolor/repattern을 서빙(WYSIWYG), 새 render 잡은 라운드를 중단시킴.

## 2. 렌더 파이프라인 (GUI 쪽)

- `redraw()` → `_clamp_view()`: spp ∈ [MIN_SPP=0.01, fit_spp×FIT_ZOOM_OUT
  (16)]; fit 너머는 다이 중앙 고정. → `_covered()`: 같은 렌더키 + 뷰가
  프레임 안(여유 패드)이면 재렌더 생략.
- `_submit_render()`: **스페클 주기 스냅** — 프레임 좌변 floor,
  상변 ceil을 2×spp 격자에 맞추고 w/h에 +2px. 2×2 체커 위상이
  레이아웃 고정이 되어 재렌더/팬 간 픽셀 결정적(홀수px 원점 이동 시
  전 레이어 명멸하던 버그의 해결책 — 커밋 dbd9111).
- **렌더키** `_render_key`: (scope, visible, depth, cut_px, lod, frames,
  labels, **color_epoch**) — 팔레트 recolor/repattern이 epoch를 올려
  캐시 프레임 재사용을 무효화.
- bbox는 끝까지 float dbu(딥줌 스케일 왜곡 방지 — int 라운딩 금지).

## 3. 스트리밍 라운드 (service._svc_render_vfs)

- 최대 _MAX_STREAM_ROUNDS(8), 마지막 라운드 stream=0(잔여 전부).
- 예산 적응: 유효 샘플(배송 ≥ 예산 절반)만으로 ~stream_target_ms(500)
  수렴, 클램프 [2048, 32768]KB.
- 라운드마다: vfsd 요청 → names/labels 소화 → `apply_hier`(실패 시
  reset_all + reset=1 재요청 1회) → emit(렌더+프레임 큐잉) →
  **`[perf]` stderr 한 줄**(라운드별 비누적: gen/round/new/bytes/tiles/
  plan/delta/apply/draw/total/lod/refining/settled) → partial이면 반복.
- 상태줄 누적치: load=plan+delta+apply(plan_ms는 라벨 포함, 재가산 금지).

## 4. VfsMosaic (viewport.py)

- klayout Layout에 WC 셀 트리를 apply(델타 파스→splice), 페이지 셀은
  이름 바인딩으로 상주/축출(evict). 원장 = applied_gen/req_gen.
- 프레임 플레인 키: FRAME_LAYER(흰 외곽)/FRAME_GRAY/FRAME_FILL/
  FRAME_DOTS — 레이어 번호 = max설계+1 포화 규칙(러스트와 동일).
- 부분 적용 실패는 반드시 reset_all(다음 gen 오염 금지, par.3.7).

## 5. Renderer (render.py)

- LayoutView 설정: 텍스트 lazy 해제(라벨 실글리프), cell-box off.
- 스페클: `_DESIGN_SPECKLE_STIPPLES = ("*.\n.*", ".*\n*.")` 역상 쌍을
  paint_plane 홀짝으로 — 전 레이어 공통 구멍(klayout 플레인 오프셋
  상쇄). **레이어별 fill 오버라이드** `set_fill_patterns({(l,d): rows})`:
  add_stipple 캐시, all-set=솔리드/all-clear=클리어 자연 처리, 단
  16×16 speckle(_SPECKLE16)과 동일하면 쌍 경로 유지(구멍 공유 계약).
- 페인트 순서 `_place_hollow_underlays_first`: [회색 프레임 언더레이
  (hollow 순서 역순 우선)] < [디자인(레이어 오름차순, 큰 번호 위)] <
  [above 셋(흰 프레임)]. hollow=외곽 1px, dotted="*." 라인스타일,
  solid=불투명.
- `render_png(path, bbox_dbu, w, h, visible, depth)` — zoom_box+
  save_image. klayout이 창 원점을 1px로 스냅함(실측).

## 6. 패널/오버레이

- 3-pane: `lpaned[ left | paned[ canvas | side ] ]`. left:
  `_left_stack`(DRC 브라우저 상시 내장 §8b; cell/object 브라우저
  추후 동거) + 하단 미니맵(180px, `_frontier_depths` =
  meta.frontier.depths, 클릭 센터링, 0.7px 미만 도트 생략).
- side(우측, margin_end 6): 제목/소스, 토글 버튼행, 레이어 목록
  (LayerRow: `l.d ■ NAME` 모노스페이스, 숨김=행 전체 취소선만·색 유지,
  선택 하이라이트 d9f2ff/픽 fff2a8), **색 팔레트 7×7**(colornames.def
  순서, DrawingArea·pane 폭 연동·1px 외곽), **fill 팔레트 5×4**
  (fillpatterns.def 순서, 1:1 타일 미리보기·흰 바탕 검은 도트,
  좌클릭=지정; 비트맵 에디터는 FLOE_FILL_EDIT=1 개발용), fit/clip 버튼.
- 오버레이(픽스버프 직접 스탬프, gui.py 상단 헬퍼): 룰러(흰 1px 실선
  + 화살촉 + 거리 칩 흰 텍스트 + 점선 리더), 러버밴드(흰 1px), 스냅
  마커(흰 십자+사각), DRC 마크(빨강: 엣지=2px 단색 실선, 폴리곤=2px
  단색 외곽 + 내부 50% 스페클 — `_drc_fill_speckle` 짝홀 스캔라인 +
  2행 체커 스트립 composite, >256 꼭짓점은 외곽만), 선택 하이라이트.
  stamp_segment/rect_outline은 px 파라미터(기본 2, 룰러/밴드는 1).
- 룰러: 다중 누적, k/Shift+K 삭제, 스냅(m, vertex/edge, _SNAP_CAP 400),
  자유각 Shift. 커서: 기본 default, 룰러 모드만 crosshair(_idle_cursor).

## 7. 키/입력 정본

`_on_key`의 체인이 정본(README 표와 일치). `_command_key`가 한글 IME
상태에서도 하드웨어 키코드로 라틴 키를 복원. 숫자는 `_depth_digit`
(9-9 1초 내 = full). depth 라벨은 full을 `*`로 표기(`depth: */13`).

## 8. 색/패턴 적용 경로

recolor: 행 스와치 재생성(set_color) + meta 갱신 + 개인 layerprops
스냅샷(_save_props_state) + `recolor` 잡(renderer.colors+coverage 틴트
갱신+refresh) + epoch↑ + 즉시 재렌더. repattern 동일 구조(_push_fills:
지정 전체를 resolved rows로 전송). 폴딩된 그룹 부모 선택 시 멤버 전체
적용. 시작 시 service `_apply_personal_fills`가 layerprops로 복원.

## 8b. DRC 브라우저 (대용량 규약)

- `drc.load_db`가 소스: .ice 사이드카(신선)면 IceDb(mmap, 레코드
  단위 lazy 디코드), 아니면 ASCII 전체 파스. 인터페이스 동일
  (checks[].errors는 시퀀스 프로토콜).
- prev/next 워크는 에러 전수 리스트를 **절대 만들지 않는다** —
  `_drc_cum` 누적 카운트 + bisect 산술(수억 에러 안전).
- **뷰어 왼쪽 pane에 상시 내장**(2026-08-13; 별도 윈도 폐지 —
  `_build_drc_panel`, `_DrcPanel` 위젯 홀더, 'e' = 미로드 시 db
  열기/이후 포커스, db 로드 시 lpaned ≥420px 자동 확장). 내부는
  위 = [룰 목록 | 에러 번호 그리드] 가로 분할, 아래 = 상세
  TextView. 룰 목록은 **이름 +
  에러 개수만**(부제 없음 — 이름은 ellipsize로 잘려도 개수 열은
  항상 보임; 설명은 상세 pane 전담). **룰 선택 = 열기**: 선택한
  룰의 그리드만 표시(한 번에 한 룰 — 아코디언 요구 충족,
  `_drc_open`).
- 그리드 = 전용 TreeView, **pane 폭에 맞춰 열 수 재배치**(가로
  스크롤 없음 — hscroll NEVER, size-allocate에서 열 수 = 폭 ÷ 셀
  폭[**현재 룰의 최대 번호 자릿수** 프로브 — 룰 전환/그리드 갱신마다
  재계산]; `_drc_grid_set_cols`가 모델/열을 재구축하고 마킹 셀
  유지). 셀 = 전역 에러 번호(레코드 디코드 없음
  — 번호 = cum[ci]+i+1), 행 선택 모드 NONE — 클릭한 **셀 하나만**
  마킹(markup 배경, `_drc_cell_mark`).
  셀 단클릭 = 하단 상세(`#번호 [상태]`·룰 설명 전문·`edge (um):`/
  `polygon (um):` 좌표 목록[상한 64~128줄]; bbox·종류 표기는 없음 —
  waive 상태는 v2 get_status, v1은 '-'), 더블클릭 = 점프(점프도
  상세 갱신). 룰 행 클릭 = 룰 정보 표시. 상한
  DRC_LIST_MAX 셀, 초과분 "… more" 행; n/p 스텝은 룰 선택·셀
  마킹·스크롤을 동기화한다.
- **에러 번호 = 전역 파일순 순번**(Calibre RVE와 동일, 파일의 서수
  토큰 무시): 모든 백엔드(ASCII/v1/v2)가 동일 번호를 내며, 룰별
  트리 나열은 저장순=파일순이라 자동 오름차순.
- **highlight in view**(체크박스, packed .ice v2 전용): **선택된 룰
  하나만** 적용 — 뷰포트 내 그 룰의 위반(상한 DRC_HL_CAP 1000)을
  `query_rect(checks=(ci,))`로 얻어 **줌 무관 3×3 cyan 사각형**
  (DRC_CYAN)으로 표시(단클릭으로 포커스된 에러는 5×5). 켜지면
  레이어가 자동 그레이스케일(mono)로, 꺼지면 이전 상태 복원.
  **그리드도 화면 내 에러만 나열**(`_drc_grid_map` = in-view ei
  리스트; 팬/줌마다 hl 재계산이 idle로 그리드를 갱신, 포커스 셀은
  뷰에 남아 있으면 유지). 에러 번호 **더블클릭(점프) 시 highlight
  자동 off**. 뷰·db·선택 룰 키로 캐시, 룰 미선택 시 상태줄 안내.
  v1 사이드카에서는 토글 거부+안내.
- **mono(그레이스케일)**: `b` 키 토글, 서비스 "mono" 잡 →
  Renderer.set_mono(디자인 레이어 색을 luminance 회색으로;
  프레임/스페클 구조 불변) + _color_epoch 무효화.
- **에러 박스 선택**(`e`, 2026-08-14): 크로스헤어 모드에서 두
  클릭이 박스를 정의 → 열린 룰의 박스 내 에러 전부 선택
  (`query_rect`, 상한 DRC_SEL_CAP 5000, v2 pack 필요; highlight
  여부 무관). 선택은 그리드 목록을 대체(우선순위: 선택 > hl
  in-view > 전체)하고 캔버스에 cyan 3×3 사각형(포커스 5×5)으로
  표시 — 선택 활성 중 hl 사각형은 대체됨. 해제 = Esc(선택 해제
  단계는 마크 해제보다 먼저) / 룰 전환 / 새 박스.
- **단클릭 포커스**(`_drc_focus`): 에러 번호 단클릭 = 줌 유지, 상세
  갱신 + 강조 — highlight 모드면 5×5 사각형(뷰 이동 없음), 아니면
  **해당 에러를 화면 중앙으로 팬**(goto 창폭 None = 줌 불변) +
  외곽선 cyan halo. 점프·룰 전환·Esc가 해제.
- **점프 규약**(`_drc_jump`): 줌 = 에러 전체 extent가 양축 모두 뷰의
  ~30%(DRC_VIEW_FRACTION, 세로축은 캔버스 종횡비로 환산; 퇴화 시
  0.1µm 창). **CD 자동 룰러**(`_drc_cd_ruler`, 리스트)를 점프마다
  교체 부착: 단일 엣지=길이 1개, 엣지 쌍=최근접 갭 1개(평행 대면이면
  중점 앵커; 교차/접촉은 없음), 축정렬 사각형=**폭·높이 2개**(둘 다
  유의미, 중앙 관통·중앙 교차). 복잡한 폴리곤/엣지셋은 룰러 생략
  (사용자 규정 2026-08-13). 수동 룰러는 보존, k/Esc에는 일반
  룰러처럼 반응.

## 9. 픽/스냅/클립

- pick: 화면 워킹셋에서 점 포함 도형 최소면적 순(_PICK_CAP 64),
  nth 순환. 프레임/라벨 셀(FRAMES/LABELS 프리픽스)은 픽 제외.
- snap: 반경 내 vertex 우선, edge 수선(투영) 차선(_SNAP_CAP 400).
- clip: probe(exact) 경로로 영역 저장. 계측은 절대 LOD/컷/격자를
  거치지 않는다(프로브 강제 0).

## 10. 상수 모음 (gui.py 상단)

MIN_SPP 0.01 · FIT_ZOOM_OUT 16 · WHEEL_ZOOM_STEP 0.96 ·
KEY_PAN_FRACTION 0.5 / _FINE 0.1 · CAL_ZOOM_IN 0.5 · MINIMAP_PX 180 ·
DETAIL_PX (5,3,1)/기본 medium · COV_MAX_TEXEL_PX 160 ·
DEBOUNCE_MS(gui.py) · 스트림 상수(_MAX_STREAM_ROUNDS 8, 예산 클램프 2048..32768KB)는 service.py.
