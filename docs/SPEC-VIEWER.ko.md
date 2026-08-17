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
  마커(흰 십자+사각), DRC 마크(**상태색** — 2026-08-14: not waived
  = red(#FF5252), waived = green(#00E676, 2026-08-17 cyan에서 변경); 엣지=2px 단색 실선, 폴리곤=2px
  단색 외곽 + 내부 50% 스페클 — `_drc_fill_speckle` 짝홀 스캔라인
  + 색상별 캐시 2행 체커 스트립 composite, >256 꼭짓점은 외곽만),
  선택 하이라이트.
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
- prev/next(n/p)는 **현재 보이는 목록 안에서 순환**(2026-08-15):
  목록 = selected ∧ in-view ∧ waive 필터의 교집합, 페이지 경계는
  자동 이동. base 종류별 스텝(`_drc_step_ei`): None=전체 산술,
  리스트=index 순환, lazy 상태필터=status_rank→status_page(다음
  1개만) — 전수 리스트 없음.
- **open .db… 다이얼로그**(2026-08-14): 파일 타입은 `*.db`만.
  선택한 .db는 직접 파스하지 않고 **오직 `<db>.ice`(pack)만
  로딩** — 신선한 v3 pack이 없으면(부재/스테일/v1/구 레이아웃)
  `floe-index drc <db> --pack`을 실행하고 로그를 **모달
  다이얼로그**에 실시간 표시(cancel = terminate) 후 로딩
  (`_drc_open_db`/`_drc_pack_and_load`, 바이너리는
  vfsclient.find_binary).
- **룰 검색**(2026-08-15): nav 행의 검색 박스(구 prev/next 버튼
  자리 — n/p 키는 유지)가 룰 이름 부분일치(대소문자 무관)로 목록을
  실시간 필터. TreeView 내장 typeahead 검색은 rules/grid 모두
  비활성(set_enable_search False — 반쪽짜리 팝업 제거).
- **SVRF 룰 메타데이터**(2026-08-15, SPEC-FORMATS `<deck>.rules.json`
  참조): db 로드 시 자동 탐색(`_drc_rules_auto` — Rule File Pathname
  베이스네임의 **db 옆** 사이드카 우선, 기록 경로·`<db>.rules.json`
  차선) + 패널 `rules…` 버튼 수동 로드. 정보줄에 `svrf 매칭/전체`.
  상세 pane 하단(`_drc_meta_lines`): 덱 원문 제약(constraint:) ·
  **measured** = 이 에러 자체의 치수 vs 한계값과 Δ/%(waive 판단
  보조; `_drc_measured` — CD 룰러와 동일 판정: rect=min(w,h)·마주
  보는 엣지쌍=갭·단일 엣지=길이·area=신발끈, 복잡 도형은 생략) ·
  layers(직접 피연산자) · gds(원천 레이어 폐쇄) · derivation 체인
  (`derived` 맵을 `svrf.rhs_operands`로 워크, 6줄 캡). 메타 없는
  룰/사이드카 부재 시 상세는 기존 그대로(추가 줄 0).
- **필터/페이지네이션**(2026-08-14): 룰 목록은 **All = 전체 룰
  (에러 0개 포함)**, Not Waived/Waived = 매칭 0개 룰 숨김.
  All/Not Waived/Waived 콤보가 status 바이트 기준으로 룰 카운트·
  그리드·in-view 필터·캔버스 페인트를 일괄 필터링. **성능 계약**:
  카운트 = [wcount] O(1), 필터 그리드 베이스 = `('status', waived)`
  lazy 서술자 — 페이지는 `status_page`(청크 스캔+조기 종료),
  n/p 셀 위치는 `status_rank`(단일 카운트) — 어떤 크기의 룰도
  필터 리스트를 실체화하지 않음(실데이터의 waived
  표기법 미파악 상태라 현재는 전부 Not Waived; v1은 status 없음 →
  Waived 항상 빈 목록). 그리드는 **DRC_PAGE(1000)셀 페이지** —
  ◀ "start – stop / count" ▶ 바, n/p 스텝이 자동으로 페이지를
  넘김(`_drc_goto_cell`). 대형 룰 클릭 시 그리드가 계속 갱신되던
  루프는 vscrollbar 폭 진동이 원인 — 그리드 스크롤러 vscroll
  ALWAYS로 고정.
- **에러 번호 표기**(2026-08-14): 그리드·상태줄은 **룰-로컬 1부터**
  (`ei+1`), Calibre식 전역 번호는 상세 pane의 `#로컬(전역)`과 점프
  상태줄로만 노출. 셀 폭은 페이지의 최대 로컬 번호 자릿수.
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
- **더블클릭 레이어 격리**(2026-08-15, rules.json 필요): 점프 시
  `_drc_isolate_layers` — 해당 룰의 source_gds에 매칭되는 레이어만
  켜고 나머지는 끔(dt null = 그 gds 레이어의 전 datatype). 최초
  격리 때 이전 가시성을 1회 스냅샷(`_drc_lyr_saved`), **Esc가 전용
  단계로 복원**(에러 박스선택 뒤·DRC 마크 앞). goto **앞**에 배치
  적용(set_active 배치 + goto의 redraw 1회 — 이중 렌더 없음). 룰
  전환 더블클릭은 스냅샷을 덮지 않고 격리 세트만 교체. 사이드카
  없음/매칭 레이어 0 = 무동작(None, 상태줄 표기 생략). n/p 스텝
  점프는 격리하지 않음(더블클릭 전용, isolate= kwarg).
- **에러 번호 = 전역 파일순 순번**(Calibre RVE와 동일, 파일의 서수
  토큰 무시): 모든 백엔드(ASCII/v1/v2)가 동일 번호를 내며, 룰별
  트리 나열은 저장순=파일순이라 자동 오름차순.
- **in view**(체크박스, packed .ice v2 전용 — 구 filter errors in
  view/highlight): **순수 목록 필터**. **selected**(체크박스,
  2026-08-15): gold 선택 에러만 목록에 — 두 체크와 waive 콤보는
  교집합으로 합성. **선택은 룰별 보존**(`_drc_sels` dict: 룰 전환
  시 저장/복원, Esc·빈 토글은 그 룰 것만 삭제, db 리로드가 전체
  초기화); selected 체크 상태에서 선택 없는 룰은 **빈 목록**(전체
  표시 아님). 선택된 룰의
  뷰포트 내 위반(상한 DRC_HL_CAP 1000)만 그리드에 나열
  (`_drc_grid_map` = in-view ei 리스트; 오버레이 패스의
  `_drc_hl_list()` 호출이 뷰 키 캐시를 갱신하고 idle로 그리드를
  따라오게 함, 포커스 셀은 뷰에 남아 있으면 유지). **흑백 연동·
  더블클릭 자동 off는 폐지**. 뷰·db·선택 룰 키로 캐시, 룰 미선택
  시 상태줄 안내(상태줄 카운트는 필터 on일 때만). v1 사이드카에서는
  토글 거부+안내.
- **룰 에러 상시 표시**(2026-08-14→15 수정): 룰이 열려 있으면
  **현재 그리드 페이지의 에러들**(_drc_page_marks — 그리드 채움 시
  지오메트리 프리빌드, 페이지당 ~7ms)이 캔버스에 **상태색 실도형**
  (not waived=red, waived=green)으로 그려짐 — 페이지 넘김·필터·waive
  변경이 마커에 즉시 반영. `_drc_stamp_errs` 공용 페인터(마커
  붕괴 = 스팬 < DRC_MARK_PX·포커스 9×9·세그먼트 예산 20k). 선택(gold)은 그
  위에 같은 페인터로 덮임. in-view 필터의 뷰 추적 호출은 유지.
  v1 사이드카도 페이지 마커 표시(query_rect 불필요해짐).
  **그리드 숫자도 상태색**(waived green / not-waived red; gold 선택
  배경 위에도 상태색 유지, 현재 셀은 파랑 배경+흰 글자).
- **mono(그레이스케일)**: `b` 키 토글(독립 — 필터와 연동 없음),
  서비스 "mono" 잡 → Renderer.set_mono(디자인 레이어 색을
  luminance 회색으로; 프레임/스페클 구조 불변) + _color_epoch
  무효화.
- **에러 박스 선택**(`e`, 2026-08-14→15): 크로스헤어 모드는
  **Esc(또는 e 재입력)까지 유지** — 박스 완료 후에도 다음 박스
  대기(룰러와 동일 계열). 두 클릭이 박스를 정의 → **현재 그려진 마커(_drc_page_marks =
  필터·페이지 적용분) 중 박스 내 에러만** 선택 — 선택은 항상
  보이는 에러 대상(query_rect/v2 요구 제거, v1도 동작). 두 번째
  클릭 수식키: 무수식=대체(빈 박스는 해제), **Shift=추가,
  Ctrl=박스 내 토글** — 그리드 Ctrl/Shift와 동일 계열. 선택은 목록을 **대체하지 않는다**(2026-08-14 수정):
  그리드는 기존 내용(전체 또는 hl in-view)을 유지하고 선택된
  번호만 **gold 배경**으로 마킹(현재 셀 파랑이 우선). 캔버스는
  선택 에러를 **gold 원래 도형**(2px, 세그먼트 예산 20k)으로
  표시. **화면 스팬이 마커 크기(DRC_MARK_PX=5) 미만인 에러는 어느
  경로든 5×5 마커 사각형으로 붕괴**(점프 마크 포함; 포커스는 9×9)
  — 임계 = 마커 크기 자체(2026-08-16: 구 ≤2px 임계는 도형이
  마커보다 작게 그려지는 중간 줌 구간을 남겼음 — 재도입 금지).
  해제 = Esc(선택 해제 단계는 마크 해제보다 먼저) /
  룰 전환 / 새 박스.
- **단클릭 포커스**(`_drc_focus`): 에러 번호 단클릭 = 상세 갱신 +
  셀 마킹만(**뷰 이동 없음** — 2026-08-14 팬 제거, 도형 불변).
  포커스된 에러가 마커로 붕괴한 경우 9×9. 점프·룰 전환·Esc가
  해제.
- **waive/unwaive 메뉴**(2026-08-14): 그리드 **우클릭** → "waive/
  unwaive #로컬" 또는 gold 선택이 있으면 "N selected" 일괄 적용 —
  v2 status 바이트를 제자리 기록(`set_status`; STATUS_WAIVED=1 ↔
  NONE=0). 적용 후 자동 갱신: All에서는 그리드/상세만, waive
  필터에서는 룰 목록 재구축(+열린 룰 재선택, 매칭 0 룰 숨김
  반영)·hl 캐시 무효화·캔버스 재도장. v1은 메뉴 대신 pack 안내.
  waive 상태는 재-pack 시 초기화됨(포맷 계약).
- **그리드 선택 편집**(2026-08-14): **Ctrl+클릭** = 해당 에러를
  선택에 추가/해제(토글), **Shift+클릭** = 현재 셀부터 클릭 셀까지
  시각적 범위를 선택에 추가. 뷰 이동 없음, 박스 선택과 같은
  _drc_sel로 합쳐짐(gold 마킹/캔버스 gold 표시 동일), 전부
  해제되면 선택 자체가 사라짐.
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
