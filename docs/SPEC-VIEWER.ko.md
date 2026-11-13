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

- **Ctrl+C = 화면 클립보드 복사**(2026-08-19, flateyes 패리티):
  `_copy_view` — **윈도우 그랩**(`Gdk.pixbuf_get_from_window`,
  flateyes capture_view 방식; 원점은 `translate_coordinates`로
  톱레벨 기준 변환 — allocation x/y를 그대로 쓰면 중첩 paned/
  scroller 레이아웃에서 앱 창 전체가 잡히는 실사고 2026-08-19):
  룰러 거리 칩과
  디자인 라벨은 Gtk.Overlay 위 **위젯**이라 합성 픽스버프에 없음
  (현장 보고: 길이 텍스트 미캡처) — 윈도우에서 떠야 함께 실림.
  창 미매핑 시 `image.get_pixbuf()` 폴백. **store() 호출 금지** —
  클립보드 매니저(Exceed TurboX 동기화)가 이미지 타깃을 떨어뜨려
  붙여넣기가 비는 실사고(flateyes에서 확정); 셀렉션은 뷰어가 직접
  서빙, 종료 시 비워짐. 상태줄 "copied WxH".
- **Tab = 오버레이 3-상태 순환**(2026-08-21, 구 2-상태 토글에서
  변경): `overlay_mode` 0→1→2→0. **0 = 모두 표시**. **1 = 현재
  (점프된) 에러 외 에러 숨김** — 페이지 마커·gold 선택 스탬프만
  스킵(점프 마크·그 **note 패널**·CD/수동 룰러·칩·디자인 선택·스냅
  유지), `_drc_hits`가 비어 숨은 마커는 pick/툴팁 불가. **2 = 전부
  숨김**(2026-08-19 flateyes 패리티 상태) — `_draw_overlays` 조기
  종료(**줌 밴드만 유지**), `_drc_hits` 비움, **룰러 거리 칩·note
  패널도 숨김**(`_update_labels`/`_update_note_labels` — 위젯이라
  페인트 게이트와 별도, 잔존 실사고). note 패널은 `overlay_mode!=2`
  이고 점프 마크가 살아있는 동안만 표시. 룰러는 **Esc로 제거**(Tab
  아님). 상태줄로 현 상태 표기. ISO_Left_Tab 포함.
- `redraw()` → `_clamp_view()`: spp ∈ [MIN_SPP=0.01, fit_spp×FIT_ZOOM_OUT
  (16)]; 팬 한계 = 다이 bbox를 **변당 10% 확장**한 박스(2026-08-18:
  다이 가장자리에 붙박이면 외곽 피처 밴드 줌 드래그 공간이 없음);
  fit 너머는 다이 중앙 고정. → `_covered()`: 같은 렌더키 + 뷰가
  프레임 안(여유 패드)이면 재렌더 생략.
- `_submit_render()`: **스페클 주기 스냅** — 프레임 좌변 floor,
  상변 ceil을 2×spp 격자에 맞추고 w/h에 +2px. 2×2 체커 위상이
  레이아웃 고정이 되어 재렌더/팬 간 픽셀 결정적(홀수px 원점 이동 시
  전 레이어 명멸하던 버그의 해결책 — 커밋 dbd9111).
- **렌더키** `_render_key`: (scope, visible, depth, cut_px, lod, frames,
  labels, **color_epoch**) — 팔레트 recolor/repattern이 epoch를 올려
  캐시 프레임 재사용을 무효화.
- bbox는 끝까지 float dbu(딥줌 스케일 왜곡 방지 — int 라운딩 금지).
- **하단 상태줄 = 지금 확인할 것만**(0.12.263, `lit`·`cell dots` 0.12.264; 사용자 2026-10-01: "어차피 로그로 나오고
  있으니 상태줄에는 현재 필요한 부분만 나와야 생략없이 확인이 가능함"). `perf_status(res)`가
  (전체 줄, 요약 줄)을 만든다.
  - **전체 줄**은 터미널 로그(`live [density: …] (N tiles, …)  view W x H um`)와 하단 줄의 툴팁에 나온다.
    0.12.262의 줄과 같은데, 0.12.264부터 밀도 태그에 `lit L px`가 붙고 `dot items`가 `cell dots`로 바뀌었다.
    0.12.268부터는 로그 줄의 `cell dots N` 뒤에 출처가 붙는다. 형식은 `[nodes …, placements …, arrays …, list
    members …, list chunks … of … members, array members …, pages …; hash map …]`이고, 0이 아닌 것만 보인다
    (사용자 2026-10-02, 실칩 `cell dots 87.2M`). 하단 줄에는 붙지 않는다. 0.12.268은 `of … members`를 맨 끝(배열
    멤버 뒤)에 붙였고, 0.12.269부터 청크 바로 뒤에 붙는다. 0.12.270부터 밀도 태그의 하한 뒤에 2패스의 예약
    `reserve R MB`가 붙는다(고정 예약 또는 1패스가 남긴 만큼). 0.12.274부터 페이지 점 중 점유 비트(design.ovb)로
    놓은 것이 있으면 `pages N (M by occupancy)`로 보인다. 캐시에 그 파일이 있는지 이것으로 안다. 0.12.278부터 칸이 너무 커서
디코드한 하한 아래 페이지가 있으면 `pages decoded under the floor K`가 붙는다. 0.12.279부터 밀도 태그 끝에 2패스의 디코드와
    래스터 벽시계 시간 `pass 2 decode D ms, raster R ms`가 붙는다(사용자 2026-10-03, 449개 레이어 뷰가 35초 이상 걸림).
    0.12.280부터 출처의 `list chunks … of … members` 뒤에, 있으면 `list chunks in full blocks C of M members`(읽지 않고
    건너뛴 청크)와 `list chunks sampled C of M members`(표본으로 읽은 청크)가 붙는다. `list members`는 하나씩 읽은 멤버
    수다. 0.12.281부터 `N regions` 뒤에, 칸으로 계획했으면 그 칸들의 빈 픽셀 `(free top T, others O px)`가 붙는다(위층,
    아래층). 0.12.287부터 fit view보다 축소해 점이 성겨지면 예약 뒤에 `dots xG`(그 비율)가 붙는다. 여백 렌더의 명령에는
    뷰포트 크기 `vw=`/`vh=`가 붙는다(렌더러가 fit view를 뷰포트 기준으로 구한다). 0.12.288부터 detail의 밀도 문턱이 있으면
    (medium, low) 그 뒤에 `gate D/B px (N out)`이 붙는다: 블록 B px 중 D점 이상이어야 그리고, N은 빠진 블록 수다.
    0.12.291부터 진단 `FLOE_RUST_DENSITY_ONLY=on`(1패스의 도형을 건너뛰고 밀도만)이면 태그 맨 앞 `dots` 뒤에
    `density only`가 붙는다(뷰어가 같은 환경 변수를 읽는다). 0.12.297부터 밀도가 밝기로 그려지면(기본; 픽셀이 덮인 면적
    × g만큼, 원본 색을 넘지 않게) 예약 뒤에 `bright xG`(g: low 1, medium 2, high 4)가 붙고, `dots xG`와 `gate …`는
    붙지 않는다(밝기가 둘을 대신한다). 킬 스위치 `FLOE_RUST_DENSITY_BRIGHT=off`면 예전처럼 붙는다. 0.12.299부터 예산에서
    빠진 페이지를 점유 격자가 대신 그렸으면 `pass 2 over budget: …` 안에 `N pages by occupancy instead`가 붙는다
    (하단 요약 줄에도). 0.12.300부터 `bright xG` 뒤에 컷 아래 셀이 무엇을 대표하는지가 붙는다: `cell cover`(셀의 도형이
    덮는 면적. design.ovb와 계층 요약이 있을 때)나 `cells by box`(셀 상자. 둘 중 하나가 없거나
    `FLOE_RUST_DENSITY_CELL_COVER=off`).
  - **하단 줄**에는 요약 줄만 나온다. 항목은 ` · `로 잇는다.
    - 시간: `N ms = L load + D draw`. `+ T text`·`+ P fit probe`·`+ O other`·`+ W wait`는 전체 줄과 같은 문턱에서만
      붙는다. load의 `[plan+delta+apply]`는 빠진다.
    - 밀도 스택: `density: lit L px, pass 2 plan N ms (nodes …, reads …, cell dots …)`.
      - `lit`은 2패스가 켠 픽셀이다. 컷 아래 셀을 대신하는 점과, 2패스가 페이지에서 꺼내 면적대로
        그리는 컷 아래 도형이 모두 들어간다. 둘 다 화면에서는 점처럼 보인다.
      - `cell dots`는 그중 셀(셀·BVH 노드·배열)을 대신한 점 항목 수다. 0.12.263까지는 `dot items`였다.
      - 사용자 2026-10-01: "두번 그리고 점들이 찍히는데 dot items가 0". 합성 칩의 694 µm 뷰를 medium으로
        보면 컷 아래 셀이 없어 cell dots는 0이지만, 1~3 px 도형을 54페이지에서 그려 84k px를 켠다.
      - 괄호 안에 있을 때만 붙는 것: 탐침(`probe M ms xP`), 두 번 이상의 맞춤과 그것이 올린
        하한(`Q passes, floor F px`), 스레드.
      - 디코드한 페이지가 있으면 `P pages decoded`가 따로 붙는다.
      - 2패스의 예약이 무엇을 막았으면 `pass 2 over budget: floor probe, thinned, N pages left out`이 붙는다(0.12.266).
        `floor probe`는 하한 탐침이 예약을 넘은 것, `thinned`는 예산 맞춤이 페이지를 솎은 것,
        `N pages left out`은 디코드가 예약에서 뺀 페이지다. 로그 줄의 태그에도 같은 문구가 붙는다.
      - 블록·하한·구역 수는 전체 줄에만 있다.
    - work bin: `bin N items` 또는 `bin off(cap@N), hier V/P pruned`. 넘치면 타일마다 걷는 양을
      함께 보인다.
    - 컷: `cut<X um`과 예산 맞춤(`… to fit budget`, `STILL OVER`). `(larger side)`는 빠진다. 0.12.301부터 그 레이어들의
      페이지가 추정보다 크게 디코드돼 예산을 줄여 계획했으면 `, pages xN their estimate`가 붙는다. 0.12.318부터 1패스의
      맞춤은 위 plane부터라 `top N whole, L/D (1/M below xF, none below xG), K left out to fit budget`이다(위에서 온전한
      레이어 수, 예산이 끝난 레이어와 그 등급, 빠진 레이어 수; SPEC-PLANNER). `FLOE_RUST_FIT_TOP_FIRST=off`면 종전
      `1/M below xF, none below xG to fit budget`이다.
    - 그림이 모자란 것: `N pages over budget (not drawn)`, `labels partial`, `evict N`,
      `summary N layers (not pickable)`. `N pages over budget`은 0.12.301부터 단일 레이아웃에서도 나온다(어떤 계획도
      예산에 들지 않을 때 오류 대신 예산이 담는 만큼 그린다). 여백(margin) 프레임이 예산을 넘으면 오류 대신
      `dropped`(reason `budget`)이라 상태줄에 아무것도 뜨지 않는다(0.12.300은 `error: decoded generation budget
      exceeded`를 보였다).
    - 덱: `deck N passes`.
  - 로드 직후 첫 프레임은 `loaded in X s · `만 앞에 붙는다. 내역은 로그에 있다.
  - depth는 위 줄(`depth: d/max · detail: …`)에 있다.
  - detail은 low/medium/high(cut 5/3/1 px) 말고도 `d` 대화상자의 `cut px` 입력으로 임의의 cut을
    줄 수 있다(사용자 요청 2026-10-28: 덱의 근접 구간을 0.1 px까지 시험): (0, 100] px, Enter나 `set`으로
    적용하고 그동안 레벨 버튼은 모두 꺼지며 위 줄은 `detail: 0.1px`; 숫자가 아니거나 범위 밖이면 입력란 아래
    줄에 이유를 쓰고 값을 바꾸지 않는다; 레벨 버튼을 누르면 끝난다. 컨트롤러 패치에는 가장 가까운 레벨과 함께
    `cut_px`로 간다(`_ctl_policy`, SHARED_APP_LAYER §패치 JSON). gate rust_renderer
    `test_a_cut_of_ones_own_rides_on_the_nearest_level`, gtk_view(서비스의 `cut_px` 0.1 = 기준 렌더 cut 0.1,
    1 px fit과 다른 픽셀, 0·-1·101·문자열 거절, detail로 해제; 실제 대화상자 `own_cut`: 0.1 입력 → 상태줄·
    컨트롤러 상태, `abc`·`200` 거절 문구, high 버튼으로 해제).
  - 팬·캐시 프레임(`live (N tiles)`), `no layers visible` 같은 짧은 상태는 그대로 보인다.

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
  `_left_stack` = **Notebook 두 페이지**(2026-09-29): `cells` = 셀 트리
  (§8c, 기본 페이지) | `DRC` = DRC 브라우저(§8b; db 로드·`_drc_window`가
  이 페이지를 올림) — 미니맵은 2026-08-22 우측 pane 노트북으로 이전;
  DRC 상세(TextView)는 pack2 shrink=False + 높이 하한 150px로 **상시
  노출**(페이지 안에서).
- side(우측, margin_end 6): 토글 버튼행부터 시작(구 제목/소스
  줄은 2026-08-22 창 타이틀로 이전 — "floe - 파일명 · N GB ·
  grid NxN", 빈 시작은 "no layout"), 레이어 목록
  (LayerRow: `l.d ■ NAME` 모노스페이스, 숨김=행 전체 취소선만·색 유지,
  선택 하이라이트 d9f2ff 배경, 지오메트리 픽 = **흰 1px 외곽
  박스만**(배경 채움 없음 — 2026-08-22: 채움 픽이 레이어 선택과
  혼동)), **색 팔레트 7×7**(colornames.def
  순서, DrawingArea·pane 폭 연동·1px 외곽), **fill 팔레트 5×4**
  (fillpatterns.def 순서, 1:1 타일 미리보기·흰 바탕 검은 도트,
  좌클릭=지정; 비트맵 에디터는 FLOE_FILL_EDIT=1 개발용; **입력은
  그리드당 EventBox 1개 + `_palette_grid_pick` 좌표 해석**(미니맵과
  동일 패턴, 입력 윈도우 69→2 단순화). 2026-08-22 "팔레트 무반응"
  보고의 실원인은 **레이어 행 미선택**(피드백이 상태줄뿐) —
  배달·좌표·핸들러는 계측으로 전 구간 정상 확인, FLOE_CLICK_DEBUG=1
  트레이스 존치) — 두 팔레트는
  **Notebook의 palette 탭**, **minimap 탭이 기본**(180px,
  `_frontier_depths` = meta.frontier.depths, 클릭 센터링, 0.7px 미만
  도트 생략; 2026-08-22 왼쪽 pane 하단에서 이전). 다이는 **MINIMAP_PAD
  (6 px) 테두리 안**에 맞춘다(사용자 요청 2026-09-29, 0.12.244: 긴 축에서
  다이 외곽선이 이미지 첫/마지막 픽셀에 놓여 위젯 가장자리에 묻히고, 다이에
  잘라 붙이던 fit 뷰 박스가 그 위를 덮었다) — 뷰 박스는 테두리까지 나가
  이미지 안쪽 1 px에서 잘리고, 테두리 클릭은 가장 가까운 다이 가장자리로
  센터링(`_minimap_world_point`). 테스트
  `test_minimap_die_outline_keeps_a_margin_from_the_edge_and_the_view_box`. fit/clip·
  open .db…·rules… 버튼은 2026-08-22 메뉴 바로 이전(패널 정보줄만 잔류; 그 정보줄도
  2026-11-10에 뺐다, §8b).
- **레이어 목록에 넣는 것(2026-10-08, app 0.12.322; 사용자: 현장 EBEAM 파일을 레이아웃 모드로 열면
  Calibre는 3.0·3.300만, floe는 3.0·3.1·3.2·3.300을 보이고 3.1·3.2에는 도형이 없음).**
  - 원인: 파일의 LAYERNAME 표가 3/1·3/2를 이름으로 두고 그 위의 도형·텍스트는 없다. 색인은 KLayout처럼 그런 쌍도
    레이어 표에 두고(`stored_shapes` 0), 뷰어는 표를 그대로 목록에 올렸다. Calibre는 무엇이 놓인 레이어만 보인다.
  - 이제 레이아웃의 목록은 `gui.listed_meta`가 정한다. `stored_shapes`가 0인 쌍(도형도 텍스트도 없음)은 패널·visible
    집합·레이어 속성에서 빠진다. 열 때 터미널에 한 줄로 알린다(`[floe2] 2 layers not listed - the file's layer table
    names them, no shape or text is on them: 3/1, 3/2 (FLOE_EMPTY_LAYERS=show lists them)`).
  - 그대로인 것: 색인·`floe2 info`(모든 쌍과 stored shapes; 0 = 이름만)·렌더러의 스타일, 텍스트만 있는 레이어(라벨로
    그려지므로 목록에 있음), 잡덱의 표(level 머리 행은 원래 아무것도 갖지 않음). 모든 레이어를 켜면 요청은 레이어
    목록 없이(전부) 가므로 그림도 같다.
  - 킬 스위치 `FLOE_EMPTY_LAYERS=show`: 모든 쌍을 예전처럼 보인다. 게이트: cell_tree C10, rust_renderer
    `LayerListTests`.
- **레이어 기본 색: 패널의 줄 순서(2026-10-13, app 0.12.352; 사용자: "각 레이어에 대해 별도의 지정이 없을 경우
  같은 그룹은 같은 색으로 표시되는데(예를들어 2.0 2.1 2.2 2.32... 등) 연속되는 레이어는 다른 색을 가지도록
  변경해줘. 뷰어와 캡쳐 모두 적용해줘.").**
  - 예전(2026-08-04 b3ce5dd6부터): 색상이 layer 번호의 황금각 배수여서 한 번호의 datatype이 모두 같은 색이었다.
  - 이제 app-core `styles::palette_ranks`가 순번을 정한다. 도형·텍스트가 있는 쌍을 (layer, datatype) 순(패널의 줄)으로
    세우고, 빈 쌍(LAYERNAME만, `FLOE_EMPTY_LAYERS=show`일 때만 목록에 나옴)은 그 뒤에 둔다. 순번 n의 색은
    `styles::layer_color(n)`이다. 색상(hue)은 n × 137.508°, 채도 0.75, 명도 1이고 계산은 예전과 같다.
  - 패널에서 이웃한 줄은 색상이 137.5° 떨어진다. 둘 건너는 85°, 셋 건너는 52.5°다.
  - layerprops의 색이 여전히 먼저다. 캐시를 열 때(catalog) 정하므로 뷰어(gtk-service `open`의 meta)·렌더 스타일·
    `floe2 render`·DRC 캡처가 같은 색이고, 기존 캐시는 다시 색인할 필요가 없다. floe-index도 meta.json에 같은 색을
    쓰고, 오라클(`floe_oracle.cache.normalize_layer_colors`)도 같은 규칙이다.
  - 같은 쌍도 레이아웃마다 다른 색일 수 있다(순번이 그 파일의 레이어 집합에 달림). KLayout의 기본 배색과 같은 방식이다.
  - 잡덱은 자기 색 규칙(색 체계·레벨 머리 색)을 그대로 쓴다.
  - **실행 옵션(같은 날 사용자: "그룹내에서는 색이 같아야 하기도 하니 실행 옵션으로 색을 다르게 할지 기존처럼 할지
    정할 수 있게 해줘").** `floe2 view --layer-colors row|group`, `floe2 render --layer-colors row|group`이다.
    - `row`가 기본(패널 줄 순서)이고, `group`은 예전처럼 layer 번호마다 한 색이다(app-core `styles::LayerColors`).
    - 환경 `FLOE_LAYER_COLORS=row|group`이 두 쪽의 기본값이다. catalog가 열 때 읽으므로 웹도 같다. 다른 값은 한 번
      경고하고 row를 쓴다.
    - 뷰어: 옵션은 그 환경을 자기가 띄우는 gtk-service에 물려주고, 독립 창으로 연다(`process_options`). 레이아웃을
      열 때 정하는 값이라 이미 떠 있는 창에 넘기지 않는다. 그 창은 제 색을 유지한다.
    - 캡처: 옵션이 환경보다 먼저다(`Dataset::set_layer_colors`). report에 `layer_colors`가 남는다. jobdeck은 거부한다.
    - 오라클(`normalize_layer_colors`)도 `FLOE_LAYER_COLORS=group`을 따른다.
  - 테스트: app-core `neighbouring_rows_take_neighbouring_places`·`a_group_shares_its_layer_numbers_colour_when_asked`,
    app-cli `layer_colors_are_asked_for_a_layout`, 게이트 rust_renderer `PaletteTests`·
    `test_layer_colors_open_a_window_of_their_own`.
- **눈금 띠(scale border)**(2026-10-13, app 0.12.355; 사용자: "viewer의 뷰포트 외곽으로 calibre와 같이 길이를 확인할 수
  있는 띠를 두르고 기준 길이도 표시해줘. 그리고 좌상단, 좌하단, 우상단, 우하단에 좌표를 표시해줘").
  - 프레임 네 가장자리에 `BORDER_PX`(12 px, 안쪽 1 px 선 포함) 띠를 두른다. 두 색조(`BORDER_EVEN` dcdcdc · `BORDER_ODD`
    3c3c3c)가 기준 길이마다 번갈아 든다. 기준 길이는 1·2·5 × 10ⁿ µm 중 화면에서 60 px 이상인 가장 짧은 것이고, 구간은
    원점부터 센다 — 구간 경계가 세계 좌표의 어림수이고 띠는 레이아웃과 함께 움직인다(Calibre DESIGNrev의 띠와 같은
    쓰임). 띠와 레이아웃 사이에 1 px 선(`BORDER_RULE` 8c8c8c), 네 모서리 정사각형은 어두운 색조.
  - 글자는 위젯이다(pycairo 없음, 룰러 칩과 같은 방식·`_border_labels`): 네 모서리에 그 모서리의 세계 좌표 `x, y`(µm;
    소수 자리는 픽셀 크기에 맞춰 3자리 이상 5자리 이하, 0으로 반올림되면 부호 없음), 위 띠 가운데에 `scale 10 um`(기준
    길이). 클릭은 통과한다(pass-through). DRC 노트 패널은 띠가 보일 때 왼쪽 위 칩 아래로 내려간다.
  - 계산은 app-core `view::border`(`Border::new(viewport, dbu)`)가 하고 gtk-service `view_border {view, bbox?, width?,
    height?}`로 받는다(SHARED_APP_LAYER §10). 뷰어는 보이는 프레임의 상자(`obox`/`ospp`: 확대 중 얼어 있는 프레임이면 그
    상자)와 뷰포트 크기로 묻고, 같은 뷰에는 다시 묻지 않는다(`_border`). 컨트롤러 루프가 아직 없으면(뷰 여는 중) 띠가
    없다.
  - View > **scale border**(체크, `s`)로 켜고 끈다. 시작은 켬, `FLOE_SCALE_BORDER=off`면 끈 채 시작. Tab 2단계(오버레이
    전부 숨김)에서는 띠와 칩도 숨는다. 프레임 픽셀과 캡처(`floe2 render`)에는 들어가지 않는다(화면 합성에만; Ctrl+C
    복사는 창을 찍으므로 포함).
  - 게이트 `gtk_view` `border_parity`: 구간이 서비스의 것(1-2-5, 60~150 px, 경계 = 기준 길이의 배수와 그 짝홀, 색조 교대,
    프레임 전체 폭), 띠·선·모서리 픽셀, 모서리 칩 = 서비스 글자 = 뷰포트 모서리 µm, 같은 뷰는 한 번만 묻기, 이동하면 경계가
    이동 px만큼 옮겨짐, 끄면 프레임 픽셀, Tab 2단계에서 숨김; 서비스 `view_border`의 상자 지정과 거절. 단위 `view::border`
    5개.
- 오버레이(픽스버프 직접 스탬프, gui.py 상단 헬퍼): 룰러(흰 1px 실선
  + 화살촉 + 거리 칩 흰 텍스트 + 점선 리더), 러버밴드(흰 1px), 스냅
  마커(흰 십자+사각), DRC 마크(**상태색** — 2026-08-14: not waived
  = red(#FF5252), waived = green(#00E676, 2026-08-17 cyan에서 변경); 엣지=2px 단색 실선, 폴리곤=2px
  단색 외곽 + 내부 **solid 50% 알파 워시** — `_drc_fill_translucent`
  짝홀 스캔라인 + 색상별 캐시 1행 스트립 composite(블렌드), >256
  꼭짓점은 외곽만. 2026-08-22: 구 불투명 50% 체커는 디자인 스페클과
  주기가 같아 특정 줌/팬 패리티에서 동상 정렬 → 아래 레이어가 완전히
  가려지는 실사고 — 알파 블렌드는 위상 무관, 캡처 임베드와도 일치.
  재제안 금지), 선택 하이라이트.
  stamp_segment/rect_outline은 px 파라미터(기본 2, 룰러/밴드는 1).
- 룰러: 다중 누적, k/Shift+K 삭제, 스냅(m, vertex/edge, _SNAP_CAP 400),
  자유각 Shift. 커서: 기본 default, 룰러 모드만 crosshair(_idle_cursor).
  스냅은 화면에 도형으로 그려진 점프 에러에도 붙고(0.12.454), 다른 레이어에 가려 보이지 않는
  꼭짓점과 변에는 붙지 않는다(0.12.457, §9).

## 7. 키/입력 정본

`_on_key`의 체인이 정본(README 표와 일치). `_command_key`가 한글 IME
상태에서도 하드웨어 키코드로 라틴 키를 복원. 숫자는 `_depth_digit`
(9-9 1초 내 = full). depth 라벨은 full을 `*`로 표기(`depth: */13`).

- **렌더 중 입력(2026-09-30, 0.12.251; 현장: "rendering… 중에 Esc로 취소가
  안 되고 새 줌도 안 됨").** 렌더가 진행 중이어도(`_pending`) 마우스는
  더 이상 기다리지 않는다 — 휠 줌·클릭·드래그가 그대로 동작하고, 새 뷰의
  렌더가 진행 중인 것을 데몬의 세대 frontier에서 대체한다(키는 원래
  그랬다; 2c8766c의 "프레임까지 마우스 무시" 관례를 걷음). 커서는 wait 대신
  progress. **Esc의 첫 단계**는 진행 중인 렌더(또는 그려지는 중인 밀도
  라운드)의 취소: `_cancel_render` — 디바운스 해제, `gen += 1`(제출 없음:
  취소된 세대의 늦은 프레임은 이 세대 것이 아니라 버려짐), 어댑터
  `worker.cancel(gen)` = `cancel before_gen=N`(데몬 입력 스레드가 즉시
  frontier를 옮기고 진행 중 렌더는 다음 검사에서 멈춤; KLayout 서비스는
  `latest`로 다음 단계에서 포기), `_refining`·`_pending` 해제, 상태줄
  "render cancelled", 그림은 얼어 있던 프레임 그대로이고 뷰는 덮이지
  않은 채라 다음 팬·줌·redraw가 렌더한다. 다음 Esc부터 기존 체인. 늦게
  도착한 프레임(`_settle_after_frame`): 새 뷰의 렌더가 이미 예약돼 있거나
  (디바운스) 드래그 중이면 아무것도 안 하고, 프레임이 지금 뷰를 담지 않으면
  (`_frame_holds_view`: 같은 렌더 키·배율, 뷰가 프레임 상자 안 — 여백 판정
  `_covered`와 다르다) `redraw()`, 담으면 여백을 채운다. 0.12.251은 여기서
  `_covered`를 물었는데, margin을 끈 기본 설정에서 `_covered`는 뷰 둘레의
  여유를 요구해 방금 그린 뷰포트 프레임(스냅 여유 ≤ 2 px)을 한 번도 받지
  않았다 — 확정 프레임마다 다시 렌더하는 무한 반복(현장 2026-09-30
  "rendering이 계속 반복", 가상 디스플레이에서 가만히 둔 25초에 157번 →
  0.12.252에서 1번; 계약 `test_a_settled_frame_of_this_view_does_not_render_again`). 이전 세대의 `error`는
  지금 세대의 대기를 풀지 않는다(세대 없는 어댑터 오류는 푼다); 데몬의
  `cancelled gen=N`은 결과로 올라와 대기 중이던 세대면 대기를 푼다.
  뷰어 계약(validate_rust_renderer): `test_esc_cancels_the_render_in_flight_before_the_chain`,
  `test_a_wheel_zoom_during_a_render_supersedes_it`,
  `test_an_older_generations_error_leaves_the_pending_render`, 어댑터
  `test_cancel_sends_the_frontier_and_a_cancelled_render_is_told`.

- **빈 시작**(2026-08-22): `floe view`(src 생략)와 인자 없는
  `floe`(= view)는 레이아웃 없이 뜬다 — `_apply_cache(None)` =
  meta None·worker None·레이어 패널 빈 상태·타이틀 APP·상태줄
  "no layout"; redraw/fit/_clamp_view/미니맵/캔버스·미니맵 입력
  핸들러가 cache None에서 조기 반환. **File > load layout…**: 픽에
  **VFS 캐시가 없으면 "Build it now?" Yes/No** → Yes면
  `_vfs_index_and_load`가 `floe-index vfs <src> .<src>.ice --jobs 12 --no-lod`
  (레이아웃은 점유 요약 없이, `floe2 index`와 같음; 2026-09-16)를 모달
  로그(`_index_modal`, cancel=terminate)로 돌린 뒤 `open_file()`로
  로드(2026-08-28); 캐시가 있으면 곧장 `open_file()`(인스턴스
  포워딩과 동일 경로) → `_apply_cache(c)`가 워커 기동·패널 재구축,
  창이 이미 실현돼 있으면(_did_fit) 즉시 fit. 인스턴스 포워딩 경로
  자체는 여전히 인덱스 필수(다이얼로그만 인덱싱 제안). 실행 중
  인스턴스에 빈 요청("")이 포워딩되면 창만 present(옵션 무시).
- **파일 열기 대화상자 = 자체 브라우저**(`_browse_file_dialog`; 사용자 요청
  2026-09-10 — GTK chooser는 필터와 무관하게 모든 폴더를 보여 캐시 폴더와 DRC 팩이
  레이아웃 옆에 늘어섰다): 폴더 먼저, 그다음 현재 필터가 허용하는 파일
  (`list_browse_entries`; 점 파일·`*.floe`·`*.ice`는 숨김), 위/홈 버튼, 경로 입력란
  (Enter = 폴더 이동 또는 파일 선택), 이름 열 type-ahead, 필터 콤보. **심볼릭 링크는
  따라가지 않는다**(사용자 요청 2026-10-21, app 0.12.372 — 다른 볼륨을 가리키는
  링크가 많은 데이터 폴더에서 링크마다 대상을 stat 하느라(automount, 응답 없는 NFS)
  대화상자가 늦게 떴다): 링크는 파일처럼 자기 이름으로 현재 필터를 거친다(사용자 요청
  2026-10-26, app 0.12.407 — .jb 필터에서도 폴더의 링크가 모두 `이름 -> 대상`으로
  나열됐다). 필터를 통과한 링크는 폴더 뒤에 파일과 한 이름순으로 `이름@` 행(`ls -F`;
  크기·수정 시각 없음)이 되고, 툴팁이 링크 자체의 텍스트 `이름 -> 대상`(readlink)이다.
  사용자가 그 행을 활성화할 때만 그 하나를 따라가 폴더면 들어가고 파일이면 선택하며
  끊긴 링크면 아무 일도 없다. 대상을 보지 않으므로 폴더를 가리키는 링크도 이름으로
  걸러진다: all files 필터에서 보이고, 경로 입력란에 쳐서 들어갈 수도 있다. 나열에 드는
  호출은 항목마다 d_type/lstat와 필터를 통과한 링크의 readlink뿐.
  이 브라우저를 쓰는 대화상자: load layout, load jobdeck, DRC open .db, load DRC
  clusters, open rule metadata, load notes, load layer properties. 저장 대화상자
  (save notes/waives, save layer properties, save clip)와
  Reviewers 창의 다중 선택은 GTK chooser 그대로다(GIO가 항목마다 링크 대상을 stat 하며
  바꿀 수 없다; 이름을 치는 저장 모드는 미구현). 웹의 picker(app-core `browse`,
  `fstatat(AT_SYMLINK_NOFOLLOW)`)는 링크를 아예 건너뛴다(`skipped_links`). 게이트:
  rust_renderer `test_load_browser_hides_caches_and_sidecars`(폴더 링크가 폴더 행이
  아니고 레이아웃 링크가 파일 행이 아니며 링크는 자기 이름이 통과하는 필터에서만
  이름순·대상 포함, 행은 파일과 한 이름순의 `이름@`, 열기 대화상자마다
  `_browse_file_dialog`), gtk_view `browse_dialog`(GTK 대화상자 그대로: .jb 필터의 행,
  all files의 행, 툴팁의 마크업 이스케이프, 폴더 링크로 들어가기, 덱 링크 선택).
- **색인 잠금**(2026-10-09, app 0.12.323 / RENDERD 0.12.297; CACHE-NAMING §5,
  SPEC-INDEXER §4.5).
  - **열기 전 확인:** 다른 실행이 레이아웃 캐시를 통째로 색인 중이면 상태줄에
    `X.oas is being indexed by … - open it when the index is done`이 뜬다. 동의 창을
    띄우거나 경쟁 빌드를 시작하지 않는다(`_index_busy`; 그동안 `_index_ready`는
    False, CLI `_cache_ready`도 같다). 예전에는 빌드 중 meta.json이 없어 "No VFS
    index"로 보고, raw `floe-index vfs`로 그 실행의 파일을 지웠다.
  - **renderd의 거절:** renderd는 `Cache::open`에서 읽는 쪽 잠금을 잡는다(잡덱은
    소스 전부를 한 번에, 폴더마다 등록 하나). 거절하면 `error code=locked
    message=… text_hex=<UTF-8 hex>`를 보내고, 어댑터는 hex 원문을 쓴다
    (`message`의 공백↔`_` 변환이 `ws_kim` 같은 이름을 깨므로).
    - 상태줄은 `render service open failed: <원문>`이다.
    - renderd stderr의 `[lock]` 경고 줄(NFS nolock, 열린 파일 한도)은 어댑터가
      그대로 stderr로 넘긴다.
    - renderd는 시작할 때 열린 파일 soft 한도를 hard로 올린다.
  - **자기 잠금:** 같은 소스를 연 자기 worker가 있는데 다시 색인해야 하면(캐시 폴더를
    손으로 지운 경우) 먼저 worker를 내린다(`_apply_cache(None)`). 자기 독자 잠금이
    그 빌드를 거절하기 때문이다.
  - **`_index_modal`:** 자기 프로세스 그룹으로 띄우고(`start_new_session`), 취소는
    `killpg`다. 잡덱 색인이 띄운 소스별 floe-index까지 끝나야 잠금이 풀린다.
    종료 코드가 75면 로그의 `[lock]` 줄을 상태줄에 띄운다.
  - **잡덱:** 통째로 색인 중인 소스는 ledger에 `not_indexed`와 "being indexed by …"로
    빠지고, 나머지 소스로 열린다(`jobdeck/sources._cache_state`).
  - **잡덱 열기는 모달 색인 한 번이다(2026-10-22, app 0.12.388; DIRECT_READ_PLAN §8.11):**
    `_jobdeck_index_and_load`가 모달로 `floe2 index deck.jb --jobs 12 [--level …]`를 돌려 경량 캐시와
    요약을 함께 만들고 연다. `_index_ready`는 덱의 소스가 모두 색인되고 요약(`summary_missing` 0)까지 있어야
    참이라, 캡처가 만든 요약 없는 캐시만 있는 덱은 "N sources have no occupancy summary … Build it now?"로
    묻고 같은 모달 색인(플랜이 빠진 요약만 더하고 나머지는 kept)을 거쳐 연다. 뷰어가 여는 파일은 다시
    열리는 전제라 요약을 먼저 만들고, 0.12.384~0.12.387의 두 단계(`--no-occupancy` 재고 → 열기 → 뒤의
    `floe2 index deck.jb`, `_summary_job_start`)는 뺐다 — 수백 GB 덱에서 묻지 않은 수 시간의 작업이었다.
- **메뉴 바**(2026-08-22, `_build_menubar`): File(**load layout**·
  clip·copy·quit) /
  View(fit·줌·goto·detail·depth·토글 체크 6종(눈금 띠 `s` 포함, 2026-10-13)·오버레이 순환) /
  Cell(셀 트리·검색 `t`, 선택 셀로 줌, 인스턴스 하이라이트 체크,
  하이라이트 해제, 선택 셀을 뷰 루트로(키 없음 — Ctrl+T는 2026-09-30 삭제)·탑으로 `Ctrl+Shift+T`,
  셀 인덱스 빌드 — §8c) /
  Ruler(모드·스냅 체크, 삭제/전체 삭제) / DRC(open .db·SVRF rules·
  n/p·waive·박스선택 체크). 항목은 키와 **같은 핸들러**를 호출하고
  라벨에 키를 병기(AccelGroup 미등록 — 키는 `_on_key` 단일 경로,
  이중 발화 방지). CheckMenuItem은 메뉴 `show` 시 `_menu_sync`가
  실상태(frames_on/abstract/coverage_on/_mono/snap_on/mode)를
  반영하며 `_menu_guard`로 set_active의 핸들러 역발화를 차단.
  단, floe2는 density coverage 상태와 메뉴/키 입력 자체를 노출하지 않는다.
- **파일 로드 끄기**(2026-11-11, app 0.12.445; 사용자: "TeeBox에서 floe를 열 경우 사용자가 임의의 파일을 로드할 수
  없게 하려고 함. 메뉴에서 레이아웃, db등 파일 로드 관련을 disable 상태로 만들기 위한 옵션을 추가해줘"):
  `floe2 view --no-file-load`, 또는 런처 환경의 `FLOE_NO_FILE_LOAD=1`(`on`·`yes`·`true`; `viewcli.file_load_off`)로
  띄운 뷰어는 시작이 가리킨 것만 읽고 뷰어 안에서 고른 파일은 읽지 않는다(`Viewer(file_load=False)`, `_file_load`).
  한 계정으로 여러 사람의 뷰어를 띄우는 런처에서는 그 계정이 읽을 수 있는 모든 파일을 사용자가 열 수 있었다.
  - 꺼지는 명령은 메뉴에서 비활성이고 툴팁이 이유(`loading files is off in this viewer (--no-file-load)`)다.
    File의 `load layout…`·`load jobdeck…`, DRC의 `open results .db…`·`load SVRF rules…`·`load clusters…`·
    `load waives…`·`review from previous run…`·`load notes…`(`_build_menubar`의 `load()`), 레이어 우클릭 메뉴의
    `load layer properties…`, 공동 검토 창의 `Load peer work…` 버튼이다. 그 버튼은 작업이 끝나 다른 컨트롤이 다시
    켜져도 꺼진 채다(창의 `controls`에 넣지 않는다).
  - 핸들러도 거절한다(`_load_refused`). 상태 줄에 `<명령>: loading files is off in this viewer (--no-file-load)`를
    쓰고 대화상자를 만들지 않는다. 뷰어의 로드는 모두 `_browse_file_dialog`를 지나므로 거기서 한 번 막는다(누가
    부르든 None). 파일을 고르기 전에 창부터 여는 둘은 따로 막는다: `review from previous run…`은 창을 열지 않고(그
    창은 읽을 경로 둘을 받는 입력란이 전부다), `load waives…`는 공동 검토 창을 열지 않는다. 공동 검토 창의 선택기
    (`_DrcTeamWindow._choose`)는 읽을 파일을 고를 때만 거절한다. 로드에 묶인 키는 없다.
  - 그대로인 것: 명령줄의 소스(색인이 없으면 묻고 만든 뒤 연다), `--drc`(pack이 없으면 묻고 만든다), `--rules`,
    `--layerprops`(0.12.451, 아래 "레이어 속성 파일 지정").
    이름으로 따라오는 것도 그대로다 - jobdeck의 level 선택(Jobdeck 메뉴 `select levels to load…` 포함), .db 옆의
    rules.json·clusters 자동 부착, 디자인 기본 layerprops, 셀 인덱스 빌드.
  - 대상이 아닌 것: 저장과 내보내기. `clip region…`, `save waives as…`, `save notes as…`, 공동 검토 창의
    `Export my review…`·`Statistics CSV…`, 레이어 메뉴의 `save layer properties…`는 GTK 파일 선택기를 그대로 연다.
    그 선택기에서는 폴더를 둘러보고 임의의 경로에 쓸 수 있다.
  - 요청과 함께 간다(0.12.446, 아래 "창 하나"). 로드를 끈 실행의 요청(`noload=1`)을 받은 창은 자기 로드를 끄고
    다시 켜지 않는다(`_file_load_latch`: 메뉴 항목, 공동 검토 창의 버튼, 승계 창). 나중에 옵션 없이 온 요청도 켜지
    못한다. 0.12.445에서는 독립 창으로 떴고 인스턴스 소켓을 갖지 않았다.
  - 테스트: 게이트 gtk_view `file_loads_off`(실제 GTK에서 일반 뷰어와 나란히: 메뉴, 핸들러, 레이어 메뉴, 공동 검토
    창), rust_renderer `test_no_file_load_goes_with_the_request_and_the_window_keeps_it`(옵션과 환경값, 요청의
    `noload=1`, 첫 실행이 되는 창, `Viewer`까지의 전달, 브라우저의 거절, 창의 로드가 꺼져 유지되는 것).
- **창 하나(요청 전달)**(2026-11-11, app 0.12.446; 사용자: "사용자가 TeeBox를 이용해서 floe를 열 경우, 이미 떠있는
  floe 창을 이용하는게 좋을 것 같음 … 자동저장 방식이라면 전환 확인을 물을 필요는 없음. DISPLAY가 동일할 경우
  전환하는 flateyes 방식을 따르면 될 것 같음"): 같은 (uid, DISPLAY)의 나중 실행은 요청을 실행 중인 창에 넘기고
  끝나며, 창은 묻지 않고 그 요청으로 바뀐다. 0.12.445까지 `--drc`·`--userid`·`--no-file-load` 실행은 늘 독립
  프로세스였다(뷰어·gtk-service·renderd 한 벌씩, 작은 샘플에서 234 MB).
  - 요청 한 줄(`viewcli.cmd_view`): `경로\tkey=value…`. 뷰 옵션(`goto`·`detail`·`depth`·`frames`·`labels`·`labelpx`·
    `levels`·`thin`·`density`·`occupancy`)에 더해 `drc=`(결과 .db의 절대 경로), `rules=`, `userid=`(그 실행이 명시한
    리뷰어: `--userid`나 `FLOE_REVIEWER`), `noload=1`, `confirmer=1`(0.12.466: `--confirmer`나 `FLOE_CONFIRMER`. 그 요청의 결과를 확인자로 연다. 같은 결과가 다른 역할로 열려 있으면 다시 연다), `layerprops=`(0.12.451: 레이어 속성 파일의 실제 경로), `drcrule=`와 `drcerr=`(0.12.461: 결과에서 선택할 룰 이름과 그 룰의 에러 번호; 아래 "룰과 에러 지정")가 간다. `--drc`가 없는 파일이면 보내기 전에 1로 끝난다
    (`no such DRC results file`). 넘긴 요청은 창이 결과를 열어 보기 전에 답을 받기 때문이다. 탭이나 줄바꿈이 든
    경로·ID는 한 줄에 실을 수 없어 독립 창으로 뜬다. `--multi`와 렌더 프로세스 옵션(`--stream-kb`·
    `--stream-target-ms`·`--render-debug`·`--frame-cache off`·`--margin on`·`--layer-colors`)은 전과 같이 독립 창이다.
  - 룰과 에러 지정(0.12.461, `--drc-rule NAME` / `--drc-err N`; `Viewer._drc_launch_pick`): 결과가 열린 뒤 룰
    행을 선택하고(`_drc_select_row` = 클릭), 번호가 있으면 그 에러로 `_drc_goto_cell` + `_drc_jump(isolate=True)`
    (= 번호 더블클릭)한다. 룰은 이름이 같은 첫 check, 번호는 룰 안의 1부터의 순번이다(스냅샷의 `--drc-err`와
    같다). 새 창이면 `run_viewer(drc_pick=)`가 `_drc_open_db(pick=)`으로 넘기고(색인을 물어 만드는 경우 포함),
    `load_drc(pick=)`가 로드 끝에 적용한다. 넘어온 요청이면 `_request_pick`이 필드를 읽어 `_drc_request(pick=)`로
    가고, 이미 열린 결과에는 다시 열지 않고 바로 적용한다. 필터: 행이 목록에 없으면 검색과 종류 필터를 지우고,
    그래도 없으면 waive 필터를 All로 돌린다. 에러가 지금의 waive 필터에 안 걸리면 All로 돌린다. 없는 룰과
    없는 번호는 상태줄과 stderr에 말하고 False다. 에러로 이동한 로드의 상태줄에는 점프의 줄 뒤에 리뷰어
    부분(`· reviewer ID …`)을 덧붙인다: 로드의 줄이 읽히기 전에 점프의 줄로 바뀌기 때문이다. 0.12.460까지의
    창은 두 필드를 모르므로 결과만 열고 선택하지 않는다(소켓 이름은 그대로 두었다).
  - 소켓 이름은 `floe2-2-<uid>-<DISPLAY>`다(`instance.socket_address`). 이전 버전의 창은 레이아웃만 열고 나머지를
    버리므로 이 요청을 받지 않게 이름을 갈랐다.
  - 창의 처리(`_on_incoming`): 레이아웃도 결과도 없는 빈 실행은 창만 앞으로 가져온다. 색인된 일반 레이아웃은 답하기
    전에 열어 실패를 보낸 쪽에 알린다(전과 같다). 결과가 있거나 덱이거나 색인이 없으면 `OK`로 답하고 창 안에서
    한다(`_request_run`): 요청의 리뷰어를 창의 리뷰어로 삼고(`_reviewer`. 이후의 DRC 열기가 서비스에 그 ID를
    준다. ID를 말하지 않은 요청은 창이 처음 뜰 때의 ID 규칙으로 돌아간다), 보이는 레이아웃이 아니면 연 뒤(색인이
    없으면 묻고 만든다) 뷰 옵션을 적용하고, 결과를 연다(`_drc_request`). 레이아웃을 열지 못하면 창은 결과까지
    그대로다.
  - 결과 전환은 묻지 않는다. waive·방문·검토 제외·note는 할 때마다 디스크에 있고(DRC.ko.md §2), 서비스는 이전
    결과를 놓기 전에 진행 중인 리뷰 쓰기를 끝내고 방문 큐를 비운다(`Session::close`). 이미 열린 결과(같은 파일,
    pack이 현재 것, 같은 리뷰어)는 다시 열지 않고 상태 줄에 `DRC <파일>: already open`이라 하며 함께 온 `rules=`만
    붙인다.
  - 기다리는 요청(`_request_busy`·`_request_hold`): 대화상자나 메뉴가 열려 있으면(GTK grab) 닫힐 때까지, 리뷰
    쓰기(`_drc_review_job`, 공동 검토의 쓰기 작업)가 돌면 끝날 때까지 요청을 쥐고 250 ms마다 본다. 보낸 쪽은 바로
    `OK`를 받고 상태 줄이 `open request waiting (a dialog or a menu is open): <파일>`이라 한다. 가장 나중 요청
    하나만 남는다. 그러지 않으면 note 대화상자가 열린 채 결과가 바뀌어, 입력한 note가 버려지거나 새 결과의 같은
    번호 에러에 쓰일 수 있었다.
  - 창 제목이 검토 중인 결과를 말한다: `… · DRC <폴더>/<파일>`(`_title_show`. 실행마다 결과 파일 이름이 같으므로
    폴더까지 적는다). 한 창이 결과를 바꿔 가며 보이므로 무엇을 검토 중인지가 늘 보여야 하고, 패널에는 그 줄이
    없다. jobdeck의 제목도 `_title_base`에 둔다(뷰 루트를 바꾸면 이전 레이아웃의 제목으로 돌아가던 것이 고쳐진다).
  - 레이아웃과 jobdeck도 폴더와 함께 적는다(2026-11-11, app 0.12.451; 사용자: "창 제목에 폴더도 함께 표시해줘").
    `/path/1/layout.oas`를 보던 창이 두 번째 실행의 `/path/2/layout.oas`로 바뀌어도 제목은 둘 다 `layout.oas`였다.
    이제 `floe2 - 2/layout.oas · 0.02 GB · grid 4x4`, `floe2 - run3/deck.jb · jobdeck 3 CHIPs · …`다
    (`Viewer._title_name`: 파일이 있는 폴더의 이름과 파일 이름. 결과 부분과 같은 규칙이다. 경로는 창이 연 절대
    경로이고, 링크로 연 소스는 링크의 폴더다).
  - 고친 것: 소스 없는 실행(`floe2`만)의 요청은 첫 필드가 비어 있는데, 받는 쪽이 줄 전체를 strip()해서 첫 옵션을
    경로로 읽었다. 실행 중인 창이 `No VFS index for detail=medium. Build it now?`를 물었다. 줄 끝만 자른다.
  - 테스트: 게이트 gtk_view `forwarded_requests`(실제 창과 실제 소켓), rust_renderer
    `test_a_running_window_takes_the_whole_request`·`test_userid_goes_with_the_request`·
    `test_results_and_rules_reach_the_window_running_or_new`·`test_rules_need_a_drc_target_and_the_files_must_exist`·
    `test_the_title_names_a_file_with_its_folder`.
- **레이어 속성 파일 지정**(2026-11-11, app 0.12.451; 사용자: "layer property 파일도 실행 옵션으로 지정할 수 있게
  해줘"): `floe2 view SRC --layerprops FILE`. 런처가 띄운 뷰어는 파일 로드가 꺼져 있어 레이어 메뉴의 load를 쓸 수
  없고, 쓰기 권한이 없는 폴더의 소스 옆에는 디자인 기본값(`SRC.layerprops`)을 둘 수 없었다.
  - 파일은 Calibre `.layerprops`다(`L.D 색 fill 이름 표시 굵기`). 적용은 레이어 메뉴의 `load layer properties…`와
    같다(`_props_tables`): 레이아웃에 있는 레이어의 색·fill·선 굵기·표시 여부를 디자인 기본값 위에 덮고, 파일에
    없는 레이어는 그대로 두며, 레이아웃에 없는 행은 건너뛴다. 파일은 서비스가 읽는다(`layerprops_read`).
  - 명령줄(`viewcli.cmd_view`): 소스가 있어야 하고(`--layerprops requires a layout or a jobdeck`), 파일이 있어야
    한다(`no such layer properties file`). 링크면 대상의 실제 경로로 바꿔 넘긴다. 서비스는 링크를 따라가지 않지만
    이 파일은 명령줄이 직접 가리킨 것이다. 독립 창을 만드는 옵션이 아니다: 요청의 `layerprops=`로 떠 있는 창에
    간다. 이 실행이 창이 되는 경우에는 창을 만들기 전에 읽어 보고, 읽을 수 없으면 `floe2: layer properties file
    <경로>: <이유>`로 끝난다(1). 색인이 없는 소스(창이 빈 채로 떠서 묻고 만든 뒤 연다)는 대기 중인 열기의 필드로
    싣는다.
  - 떠 있는 창(`_on_incoming`): 읽을 수 없는 파일이면 아무것도 열지 않고 `ERR layer properties file <경로>:
    <이유>`로 답하며 보낸 쪽은 1로 끝난다. 읽히면 요청의 레이아웃을 여는 데 함께 준다(`open_file(path, props=)`.
    색인을 만든 뒤 여는 경로와 덱도 같다: `_open_or_index`가 `layerprops=`를 뷰 옵션에서 떼어 넘긴다).
  - 그 파일은 그 레이아웃의 것이 된다(`_props_bound`: 소스 경로, 파일 경로, 파일의 mtime·크기). 레이어 표를 만들
    때마다 올린다(`_apply_cache`의 `_props_bound_apply`): 처음 열 때, 같은 레이아웃을 다시 읽을 때, 덱의 level을
    다시 고를 때, 덱이 level view로 돌아올 때. 표에는 뷰를 만들기 전에 올린다. 뷰를 여는 패치의 `layers`가 파일의
    표시 여부를 따르므로 첫 프레임부터 숨긴 레이어를 그리지 않는다. 색·fill·굵기는 뷰가 만들어지면 `recolor`·
    `repattern` 잡으로 간다(열리는 중이면 `ViewWorker`가 쥐고 있다가 보낸다). 덱에서는 사용자가 그 뷰에서 고른
    표시 여부가 파일보다 뒤에 적용된다.
  - 덱은 level view의 파일이다. 실행이 여는 뷰가 level view이고, chip view와 source layer view는 행의 키가 달라
    각자의 파일을 쓴다(JOBDECK.ko.md). 다른 뷰에 있는 동안 온 요청은 묶어 두기만 하고 상태 줄이
    `layer properties of the level view: <경로>`라 한다.
  - 다른 레이아웃을 열면 그 파일은 따라가지 않는다. `--layerprops` 없는 요청으로 연 레이아웃은 기본값이다.
  - 같은 파일을 다시 지정한 요청(`_props_request`): 창이 이미 그 레이아웃을 그 파일로 보여 주고 파일이 바뀌지
    않았으면 다시 적용하지 않는다. 런처는 실행할 때마다 같은 명령줄을 반복하므로, 다시 적용하면 사용자가 그동안
    바꾼 표시 여부와 색이 매번 되돌아간다. 파일이 바뀌었거나 다른 파일이면 보이는 레이아웃에 바로 적용한다
    (`_props_load`: 메뉴의 load와 같은 경로). 이때는 지금 표 위에 덮으므로 앞 파일에만 있던 레이어의 값은 남아
    있다가, 그 레이아웃의 표를 다시 만들 때 새 파일만 적용된다. 결과·자·뷰를 그대로 두려고 다시 열지 않는다.
  - 무엇이 적용됐는지는 뷰가 뜬 뒤 상태 줄이 말한다: `layer properties loaded: <경로> (N colors, M fills)`, 맞는
    레이어가 없으면 `no matching layers in <경로>`(`_props_note`, `_worker_start_finished`). 창이 뜬 뒤에 파일이
    없어졌거나 깨졌으면 다시 읽을 때 `layer properties load failed: <이유>`를 상태 줄과 stderr에 쓰고 기본값으로
    연다.
  - `--no-file-load`와 함께 쓸 수 있다. 꺼지는 것은 뷰어 안에서 고르는 로드이고, 이 파일은 명령줄의 것이다.
  - 캡처는 `floe2 render SOURCE --layerprops FILE`(0.12.452)이 같은 파일을 받아 같은 규칙으로 덮는다
    ([CAPTURE-CLI.ko.md](CAPTURE-CLI.ko.md) §1.3). 다른 점: 캡처는 디자인 기본값의 숨김을 적용하지 않고(이 파일의
    숨김만), 덱은 찍는 뷰(`--mode`)의 줄로 읽는다.
  - 테스트: 게이트 gtk_view `layer_properties_option`(실제 창과 실제 명령줄: 표와 프레임의 색, 뷰를 연 레이어,
    다시 읽기, 같은 파일·바뀐 파일·다른 레이아웃의 파일, 링크, 거절 셋), rust_renderer
    `test_layer_properties_go_with_the_launch`(명령줄, 요청, 창의 처리, 묶임의 규칙).
- **밀도 켜기/끄기**(2026-10-05, 0.12.298; 사용자: "뷰어에서 밀도 켜기/끄기 옵션을 추가해줘"): floe2(Rust 렌더러)의
  View > **density under the cut**(체크, `v`). 컷 아래 도형을 덮인 면적의 밀도로 그리는 renderd의 밀도 스택
  (CUT_DENSITY_DESIGN §10.12)을 켜고 끈다. `_toggle_density`가 `density_on`을 뒤집고 `_on_depth`(상태줄과 다시
  그리기)를 부른다.
  - 시작값은 `floe2 view --density on|off`다. 실행 중인 창에는 `density=`로 넘어가고, 인덱스가 없어 열기를 미룬 경우는
    pending_fields로 간다. 옵션이 없으면 켜진다(0.12.351, 사용자 2026-10-12: "밀도는 기본으로 켜지게 해줘"; 그전에는
    뷰어와 renderd가 함께 보는 환경 `FLOE_RUST_DENSITY_STACK=top`일 때만). 그 환경이 `FLOE_RUST_DENSITY_STACK=off`면 꺼진
    채 시작한다(되돌리는 스위치). `floe2 render`의 기본도 같은 규칙이다(app-core `shots::density_default`).
  - 컷이 없는 프레임(cut 0)과 jobdeck 프레임에는 켜 두어도 `density`를 끔으로 보낸다. 컷 0에서 켜면 그릴 것은 없고
    2패스 예약만큼 1패스 예산만 줄어든다. 덱 패스는 밀도를 그리지 않는다. `floe2 render`도 같다.
  - 렌더 작업(뷰포트와 여백)에 `density`가 실리고, 어댑터는 렌더 명령에 `density=on|off`를 붙인다.
    - renderd는 켬이면 스택과 점을 그린다(`FLOE_RUST_DENSITY_DOTS=off`가 아니면 점). 끔이면 2패스가 없다.
    - 필드가 없는 요청(CLI 내보내기, 게이트)은 예전처럼 환경 변수가 정한다.
  - 렌더 키(`_render_key`)에 `_density_key()`가 들어간다. 토글하면 화면 프레임과 여백 프레임이 지금 상태를 담지 않은
    것이 되어 다시 그린다.
  - renderd도 보관 프레임의 키(`RetainedKey::density`)에 이 설정을 넣는다. 키에 없으면 밀도 없이 그려 보관한 프레임이
    같은 뷰의 켬 요청을 통째로(라벨만 다시 그리는 경로로) 대신해, 토글한 뷰에 밀도가 나오지 않는다.
  - 상태줄(깊이 라벨)에 ` · density:on/off`가 붙는다. 캐시에 design.ovs가 없어 renderd가 밀도 없이 그린 프레임
    (perf 필드 `density_none=nofile`)이면 ` · density:none (no design.ovs)`다.
  - **파일이 없으면 거부(0.12.401 / RENDERD 0.12.359, 사용자 2026-10-25 "ovs가 없으면 density on을 거부").** 밀도
    스택의 2패스는 캐시의 점유 밀도 design.ovs로 그린다. 캐시에 그 파일이 없으면 renderd(`run_render_attempt`,
    `Cache::has_density_file`)는 요청의 `density=on`을 끔으로 바꿔 그린다 — 1패스의 2패스 예약, fit·보관 프레임 키,
    2패스까지 끔 프레임과 바이트 동일 — 그리고 프레임 줄에 `density_none=nofile`을 적는다(페이지 플랜으로 2패스를
    대신 그리던 폴백은 없다; `FLOE_RUST_DENSITY_OCC=off`는 진단용으로 플랜을 유지한다. 탑의 파일이 있는 캐시에서
    자기 design.ovs.<셀>이 없는 셀의 root 뷰는 종전대로 플랜이다 — CUT_DENSITY_DESIGN §10.16). 뷰어는
    `_density_available()`(레이아웃 캐시의 design.ovs; 덱은 거짓)로 토글 켬을 거절하고 하단 줄에 "density needs
    design.ovs in the cache (floe-index ovs CACHE adds it to a full index; a light cache has none)"를 쓴다;
    `--density on`과 넘어온 `density=on`도 같은 사유로 거절해 stderr `[floe2] --density on refused: …`와 하단 줄에
    적고 끔으로 시작한다; 렌더 요청의 `density`는 파일이 있을 때만 켬이다. `floe2 render --density on`은 app-core
    `render::density_for`가 입력 오류로 거절한다(floe2는 exit 1, floe2-web은 2; 경량 캐시 "a light cache draws no density; the full index has it", 전체
    색인 "`floe-index ovs CACHE` adds it to a full index"). 말하지 않은 기본(켬)은 파일이 있을 때만 켬이다
    (DIRECT_READ_PLAN §8.24, CUT_DENSITY_DESIGN §10.17).
  - stable floe(KLayout)는 `v`가 coverage 오버레이라 이 항목이 없다. 렌더러가 밀도 스택을 갖지 않으면
    (`supports_density` 없음) 메뉴 항목이 비활성이고 키도 아무 일을 하지 않는다.
  LOD 토글(메뉴 항목·`l` 키·깊이 라벨의 `lod:on/off`·`floe2 view --lod`)은 제거됐다(2026-09-22
  사용자 결정: LOD는 쓰지 않는다 — 토글은 Rust 경로의 renderd에 닿지도 않았다). 실행 중인 창으로
  넘어오는 `lod=` 필드는 받아서 무시하고, renderd의 LOD 교체는 기본 끔이다(SPEC-PLANNER §3).
- **점유 요약 켜기/끄기**(2026-10-20, 0.12.371 / RENDERD 0.12.336; 사용자: "jobdeck으로 인덱싱된 레이아웃 파일을
  일반 레이아웃 모드로 열었을 때도 occupancy를 볼 수 있도록 옵션이 있으면 좋겠음"): floe2(Rust 렌더러)의 View >
  **occupancy summary at wide views**(체크, `o`). 캐시의 design.ovo로 광역 뷰를 그리는 점유 요약(OCCUPANCY_PLAN
  §6)을 이 뷰에서 켜고 끈다. renderd의 규칙은 잡덱의 pass는 켬, 일반 레이아웃의 프레임은 끔(2026-09-24 사용자
  결정)이라, 덱이 색인한 소스를 단독으로 열면 파일이 있어도 `summary: none (layout)`이었다. `_toggle_occupancy`가
  `occupancy_on`을 지금 유효한 상태의 반대로 두고 `_on_depth`(상태줄과 다시 그리기)를 부른다.
  - 체크 상태는 유효 상태다: 사용자가 정하지 않았으면(`occupancy_on` None) renderd의 규칙을 같은 환경에서 계산한
    값 - 덱 켬·레이아웃 끔, `FLOE_RUST_OCCUPANCY=on`이면 레이아웃도 켬, `=off`면 덱도 끔(`_occupancy_effective`).
  - 시작값은 `floe2 view --occupancy on|off`다. 실행 중인 창에는 `occupancy=`로 넘어가고, 인덱스가 없어 열기를 미룬
    경우는 pending_fields로 간다. 옵션이 없으면 아무것도 보내지 않는다(= renderd의 규칙).
  - 렌더 작업(뷰포트와 여백)에는 사용자가 정한 뒤로만 `occupancy`가 실리고(컨트롤러 `ViewState::occupancy`,
    `Patch::occupancy`; gtk-service 패치 필드 `occupancy`, 상태 JSON `occupancy`), worker-client는 렌더 명령에
    `occupancy=on|off`를 붙인다. renderd(`summary_switches`): 필드가 있으면 그 프레임은 그대로 따르고(켬이면
    레이아웃 프레임도 요약 - 킬 스위치 `FLOE_RUST_OCCUPANCY=off`보다 먼저; 끔이면 덱 pass도 페이지 경로, 사유
    `off`), 없으면 종전 규칙이다. 뷰 루트 아래서는 어느 쪽이든 끔이다(design.ovo는 탑을 평탄화한 것). 덱은
    `DeckRenderRequest::occupancy`로 pass마다 같은 규칙이다. 덱의 level/chip 전환은 토글을 유지한다.
  - 보관 프레임의 키는 요약 선택(`RetainedKey::summary`)을 이미 담고 있어 토글한 뷰가 이전 프레임으로 대신되지
    않는다. 예산 맞춤 기억(`fit_memory_key`)에는 요약이 플랜에서 뺀 레이어(`PlanRequest::summary_layers`)를
    더했다: 같은 배율에서 요약 켬과 끔은 서로 다른 페이지 집합을 맞춘 것이라 서로의 결정을 쓰지 않는다.
  - 상태줄(깊이 라벨)에 ` · occupancy:on/off`가 붙는다.
  - **파일이 없으면 거부(0.12.401 / RENDERD 0.12.359, 사용자 2026-10-25 "ovo가 없으면 occupancy on을 거부").**
    레이아웃 캐시에 design.ovo가 없거나(renderd 선택 사유 `nofile`) 이 캐시의 것이 아니면(`invalid`) 명시적
    `occupancy=on`은 renderd가 오류로 거절한다 — "occupancy=on asked but the cache's design.ovo is absent: `floe2
    index SRC --occupancy-only` makes it"(0.12.400까지는 perf 줄에 `summary: none (nofile)`을 적고 페이지 경로로
    그렸다). 뷰어는 `_occupancy_available()`(레이아웃 캐시의 design.ovo; 덱은 renderd의 pass 규칙이라 참)로 토글
    켬을 거절하고 하단 줄에 "occupancy summary needs design.ovo in the cache (floe2 index SRC --occupancy-only adds
    it)"를 쓴다; `--occupancy on`과 넘어온 `occupancy=on`도 같은 사유로 거절해 stderr와 하단 줄에 적고 정하지 않은
    상태(renderd의 규칙)로 둔다. 말하지 않은 요청(renderd의 규칙; `FLOE_RUST_OCCUPANCY=on`의 레이아웃)은 종전처럼
    `nofile`을 적고 페이지 경로로 그린다. 캡처의 명시적 `--occupancy on`은 요약을 먼저 만들고 그린다(0.12.388의
    자동 색인; 사용자 2026-10-25 "캡쳐에서 명시적으로 --occupancy on이라면 요약을 먼저 만들고 그리는게 맞음") —
    `--no-index`로 만들 수 없을 때만 renderd의 거절이 그대로 오류다(DIRECT_READ_PLAN §8.24).
  - **경량 캐시(DIRECT_READ_PLAN L6, 0.12.384).** 깊이 라벨에 레이아웃이면 ` · cache:light`(meta.json
    `cache: light`), 덱이면 ` · light:N/M`(경량 소스 수/소스 수, meta `jobdeck.light_sources`)이 붙는다. 덱의
    경량 소스에 design.ovo가 아직 없으면 요약이 그릴 광역 뷰의 그 pass는 비어 있고(renderd `summary_building=N`;
    근접 뷰는 그린다) 캔버스 위 배너가 뜬다 — "occupancy summary missing for N passes - wide views of those
    sources are empty (floe2 index deck.jb adds it)". 배너가 보이는 동안 3 s마다 다시 그려(`_summary_poll_tick`) 다른 실행이 더한
    요약이 생기면 걷힌다. 0.12.388부터 뷰어는 열 때의 모달 색인에서 요약을 만들므로 이 배너는 모달이 더하지
    못한 소스에만 남는다(0.12.397까지는 배치가 요약 패스의 예산을 넘은 소스가 그랬다; 0.12.398부터 요약 패스는
    배치 상한이 없어 색인이 실패한 소스뿐이다 — DIRECT_READ_PLAN §8.21)(0.12.384~0.12.387의 "building"/"unavailable"
    배너와 백그라운드 색인은 뺐다). perf 줄에는 `, N passes wait for their occupancy summary (light sources: empty until it
    is built)`, 요약 줄에는 `, N wait for the summary (building)`. 경량 캐시의 프레임이 채우기를 끝내기 전에 오면
    (`final=0`, `light_pending=1`) 하단 줄은 "reading the source... (fill round N)"이다(SPEC §3의 refining과 같은
    흐름; FLOE_LIGHT_FIRST_MS).
  - `floe2 render --occupancy on|off`가 같은 필드다(app-core `Shot::occupancy`, DRC 캡처도; `--detail exact`와
    `--archival`은 `on`을 거절한다 - 컷 0은 요약하지 않는다). 보고서 JSON에 `occupancy`(null = 규칙)가 실린다.
  - 렌더러가 요약을 모르면(`supports_occupancy` 없음) 메뉴 항목이 비활성이고 키도 아무 일을 하지 않는다.

## 8. 색/패턴 적용 경로

recolor: 행 스와치 재생성(set_color) + meta 갱신 + 개인 layerprops
스냅샷(_save_props_state) + `recolor` 잡(renderer colors 갱신+refresh) +
epoch↑ + 즉시 재렌더. 안정판 floe의 KLayout worker만 coverage 틴트를 함께
갱신한다. repattern 동일 구조(_push_fills:
지정 전체를 resolved rows로 전송). 폴딩된 그룹 부모 선택 시 멤버 전체
적용. 시작 시 service `_apply_personal_fills`가 layerprops로 복원.

## 8b. DRC 브라우저 (대용량 규약)

- **검토 통계**(2026-10-09 feature/jobdeck-drc, 2026-10-11 서비스 이식 —
  floe2 gtk-service `drc_team.rs`, app-core `drc::team`): 개별 에러를 열어 그
  도형이 실제로 캔버스에 그려졌을 때 `visited`를 기록한다(`_drc_visit_drawn`:
  선분·채움 스캔라인이 화면 안에 찍혔는지). 숨긴 오버레이(Tab), 화면 밖의 도형,
  원형 마커로 축약된 도형, 마커 표시와 다중 선택은 방문이 아니다. 숨겼던 오버레이를
  다시 보이면 그때 그려진 현재 에러가 방문이다. paint에서는 서비스 요청을 하지 않고
  다음 idle에 `drc_visit`를 보낸다. 서비스는 세션의 방문 스레드에서 순서대로 쓰고
  `{"kind": "visit"}` 이벤트로 알린다. 같은 마크의 반복 프레임은 한 번만 보낸다.
  `검토대상 아님`은 에러 번호 우클릭(현재/선택 전체)과 DRC 메뉴
  `mark/clear not for review`로 지정·해제한다(서비스 review 작업, 마커·그룹·선택은
  그대로). 목록은 번호 앞에 `V`/`X`(읽기 실패는 `?`)를 붙이고 waive·선택 색은 그대로다.
  상세에 `visited: … · not for review: …` 줄을 둔다. 패널의 전체 DB·현재 룰 통계 줄과
  `export review statistics…`(패널 버튼, DRC 메뉴 항목)는 2026-11-10(app 0.12.442)에 뺐다:
  수치는 `floe2 drc <db> --review-stats`와 공동 검토 창으로 본다. 서비스의 `drc_review_stats`
  작업(CSV, 접수한 방문을 쓴 뒤 작성)은 남아 있다.
- **패널 위쪽에는 아무 줄도 없다**(2026-11-10, app 0.12.442; 사용자: DRC UI 정리).
  `_build_drc_panel`의 첫 자식이 룰 목록·에러 그리드의 Paned다. 정보줄, `reviewer:` 줄, 범례·통계
  줄, 통계 내보내기 버튼, `Conflict only` 상태 줄이 있던 자리다. 로드가 말할 것은 상태 줄에
  낸다(`_drc_loaded_text`: 파일·룰 수·에러 수, `· ASCII read, no index`, `· svrf N/M`, `· reviewer <ID>`와 명시하지
  않은 ID의 안내, read-only, 치운 리뷰의 파일 수). 상태 줄은 커서가 움직이면 좌표로 바뀌므로,
  열리지 않은 로드(`_drc_not_loaded`: 실패·거절, 로드된 결과가 없을 때)와 치운 리뷰의 안내
  (`_drc_moved_aside_text`: `notices`의 줄)는 에러 상세 영역에도 적는다. 상세 영역은 룰을 고르기
  전까지 비어 있는 곳이라 그때까지 남는다. `Conflict only`의 상태(`_on_drc_conflict`,
  `_drc_conflict_poll`)와 오래 걸리는 마커 계산의 진행·완료(`_drc_marker_poll`)도 상태 줄이다.
  표 형태의 대응은 DRC.ko.md §3.
- **팀 waive와 confirm**(DRC 메뉴 `team waives…`, 0.12.466; `_DrcWaivesWindow`, DRC.ko.md §2): 리뷰어
  전원의 waive를 그들의 작업 파일에서 바로 읽어(`drc_team_waives`: 발견은 통계 명령과 같은
  `stats::discover`) 룰별로 보인다. 룰 목록은 누군가 waive한 룰만이고, 목록은 `To confirm`(기본)·
  `Confirmed`·`All team waives`, 행은 에러·waive한 사람들·사유·confirm한 사람들, 1,000개씩이다.
  더블클릭은 그 에러를 연다(`_drc_jump`). 확인자로 연 결과(`floe2 view --confirmer`, 서비스의
  `drc_open {confirmer: true}`)에서만 `Confirm selected`·`Unconfirm selected`·`Confirm all of rule`이
  켜져 있다(`drc_team_confirm`, job `confirm`: 쓰기 작업이라 창을 닫아도 끝까지 간다). 룰 전체는 묻고
  나서 한다. 그 밖의 창에서는 버튼이 꺼져 있고 툴팁이 이유를 말한다. 목록은 마지막 `Refresh`가 읽은
  상태이고 자기 confirm 뒤에는 다시 읽는다. 에러 상세에는 `waived by: …`(나 말고 waive한 사람과 그
  사유)와 `waive confirmed by: … · locked`가 붙는다(`drc_team_waive_get`, `_drc_team_lines`).
  **confirm된 waive는 잠긴다**(0.12.467, DRC.ko.md §2): 확인자가 아닌 뷰어의 unwaive는 그 에러를
  남기고, 사유를 주며 다시 waive해도 원래 사유가 남는다. 뷰어는 상태 줄로 알린다
  (`_drc_waived_refresh`의 `locked`, `_drc_locked_said`): 일부면
  `unwaived N error(s) in RULE · K locked: confirmed by hong - only a confirmer turns them back`,
  전부면 `locked: K waive(s) in RULE confirmed by hong - only a confirmer turns them back`, 사유를 주며
  다시 waive했으면 `… · K locked: confirmed by hong, reason unchanged`. 서비스의 답이 잠긴 에러를
  말하고(`drc_set_status`의 `locked`·`locked_by`, review job의 `locked`·`locked_by`), `PackDrc`는
  잠긴 에러의 캐시 상태를 바꾸지 않으며 `locked_take()`로 마지막 쓰기의 (수, confirm한 사람들)을 한 번
  내준다. 확인자의 창에서는 `Confirm …` 버튼의 툴팁이 잠금을, `Unconfirm selected`의 툴팁이 풀림을
  말한다.
  **반려**(0.12.468, DRC.ko.md §2): 창의 목록에 `Rejected`가, 확인자의 창에 `Reject selected`(사유를
  묻는다: `_drc_reason_dialog("reject", …)`, 창 제목 `Reject reason`)와 `Unreject selected`가 있다
  (`_DrcWaivesWindow._reject`, 서비스의 `drc_team_reject`, job `confirm`). 열에 `Rejected by`(확인자와
  사유)가 늘었고 수치 줄에 `· rejected N`이 붙는다. 결과를 연 뷰어는 열 때와 그 뒤 2초마다
  (`RETURNED_MS`) 서비스에 자기 waive 중 반려된 것을 되돌리게 한다(`_drc_returned_start`·`_look`·
  `_poll`, 서비스의 `drc_team_returned`, job `returned`: 폴더 목록을 읽으므로 작업이다). 되돌린 것이
  있으면 목록·마커·수치를 고치고 상태 줄에 `N waive(s) in RULE rejected by hong (사유): turned back`을
  쓴다. 반려된 에러를 waive하려 하면 서비스의 답(`rejected`·`rejected_by`)대로
  `… · K rejected by hong (사유): not waived`, 전부면
  `rejected: K error(s) in RULE by hong (사유) - not waived until the rejection is taken back`이다.
  `PackDrc.rejected_take()`가 마지막 쓰기의 (수, ((누가, 왜), …))를 한 번 내준다. 에러 상세에는
  `waive rejected by: hong (사유)`가 붙는다. 이 조회는 에러를
  고를 때마다 일어나고 리뷰어들의 파일을 찾느라 폴더 목록을 읽으므로(임시 폴더의 이름 19,000개에서 8 ms)
  서비스의 작업(job `team-row`)으로 한다: 상세는 바로 뜨고 팀 줄은 답이 오면 붙으며, 같은 에러를 다시
  보면 지난 답의 줄이 바로 나온 뒤 새 답으로 고쳐진다(`waive_ask`·`waive_answer`, `_drc_team_poll`). 화면
  스레드가 그 목록 읽기를 기다리게 두지 않는다. (그 지연이 드러낸 것: 서비스는 프레임 파일을 최근 4장만
  두는데, 뷰 편집 사이에 화면 스레드가 늦으면 이미 교체된 프레임을 읽다가 `error: frame event failed`가
  났다. `_ctl_events`는 파일이 없고 뒤에 새 프레임이 있는 프레임을 건너뛴다. 0.12.466.) 결과를 확인자로
  열면 로드의 상태 줄 끝에 ` · confirmer`가 붙는다. 메인 목록의 waive 필터와 캔버스의 색은 전처럼
  내 waive의 것이다.
- **공동 검토**(DRC 메뉴 `reviewers…`; `load waives…`도 이 창의 불러오기로 간다 —
  내 상태를 갈아끼우지 않는다): 다른 리뷰어의 `.floereview`, 작업 review SQLite,
  waive 파일을 읽기 전용으로 불러와(`drc_team_load`) 내 상태와 비교한다. 리뷰어마다
  최신 스냅숏 하나만 남는다. `Refresh`는 내 최신 작업과 불러온 자료를 집계한다
  (`drc_team_aggregate`, 서비스가 접수한 방문을 쓴 뒤). 목록은 선택한 동료의 작업,
  방문한 리뷰어 사이의 conflict, 선택한 동료와의 import conflict를 1,000개씩 보이며
  각 행에 내·동료의 V/W/NR과 conflict 이유를 쓴다. 더블클릭은 내 도형을 연다.
  `Apply all peer work safely`는 내 방문 에러의 다른 판단을 건너뛰고,
  `Use peer judgement on selected rows`는 선택한 행에 강제로 적용한다. 수동
  버튼(Waive/Unwaive/Exclude/Require selected)은 내 상태를 바꾼다(Waive·Exclude는 사유 입력창을
  먼저 띄운다, 0.12.463, DRC.ko.md §2 사유). 상대 visited는
  복사하지 않는다. `Export my review…`, `Statistics CSV…`(전체 DB·룰별, conflicts·
  reviewers 열). 모든 불러오기·집계·페이지·저장·적용은 창의 작업 스레드에서 서비스
  작업을 기다리며, 창을 닫으면 읽기 작업은 취소하고 쓰기는 끝까지 간다.
  메인 목록의 `Conflict only`는 마지막 집계의 conflict를 클러스터·CD 그룹·waive
  필터와 교집합으로 서비스 선택(`drc_team_conflicts`)으로 만들고, 목록·마커·박스
  선택·in view의 조건으로 쓴다. 준비 중에는 전체를 대신 보이지 않으며 룰·조건·DB가
  바뀐 뒤 도착한 결과는 버린다. 룰/그룹 제목의 개수는 기존 범위의 개수다. 내 판단을
  바꾸면 다음 `Refresh`까지 conflict 집합은 그대로이고 waive 필터만 다시 계산한다.
  Reviewers 창을 닫아도 마지막 집계와 필터는 남고, 다른 DB를 열면 초기화한다.
- **이전 DRC 실행의 검토 승계**(DRC 메뉴 `review from previous run…`): 이전 `.db`나
  `.tray`와(선택) 검토 파일을 고르고 `Compare geometry`(`drc_migrate_prepare`,
  진행률·단계·취소)로 정확한 도형 대응을 본 뒤 `Apply exact matches`
  (`drc_migrate_apply`)로 적용한다. 목록은 일치, conflict(모호·보호), 모호한 이전
  상태를 1,000개씩 보이고, 각 행에 현재와 이전의 V/W/NR(불일치 항목 `—`), 결과
  (exact match / partial consensus / current judgement protected / geometry match
  unverified)를 쓴다. 통계 줄은 일치(부분)·미대응·모호·conflict와 제외된 도형·
  이름이 없거나 중복인 룰의 이전 작업 수, 적용 결과를 보인다. 입력을 바꾸면 준비
  결과를 버리고, DB가 바뀌면 창을 닫는다.
- `drc.load_db`가 소스: 신선한 pack(`.<db>.tray`)이면 IcePack(mmap, 블록
  단위 lazy 디코드), 아니면 ASCII 전체 파스(v1 오프셋 사이드카는
  2026-08-19 폐기 — 잔존 파일은 stderr 안내 후 ASCII 폴백).
  인터페이스 동일(checks[].errors는 시퀀스 프로토콜).
- prev/next(n/p)는 **현재 보이는 목록 안에서 순환**(2026-08-15):
  목록 = selected ∧ in-view ∧ waive 필터의 교집합, 페이지 경계는
  자동 이동. base 종류별 스텝(`_drc_step_ei`): None=전체 산술,
  리스트=index 순환, lazy 상태필터=status_rank→status_page(다음
  1개만) — 전수 리스트 없음. **스텝 동작은 점프 상태로 갈림**
  (2026-08-18): drc_mark 활성(더블클릭/마커 클릭 후) = 기존 그대로
  프레이밍 점프+CD 룰러; **Esc 완전 복원 후(또는 점프 전) =
  번호 단일클릭과 동일** — 셀 마크+디테일+포커스만, 뷰 불변,
  `_drc_pos`는 계속 전진(연속 스텝 유지).
- **open .db… 다이얼로그**(2026-08-14): 파일 타입은 `*.db`만.
  선택한 .db는 직접 파스하지 않고 **오직 `.<db>.tray`(pack)만
  로딩** — 신선한 현-레이아웃 pack이 없으면(부재/스테일/v1/구
  레이아웃) **"Build it now?" Yes/No로 물은 뒤**(2026-08-28,
  `_ask_yes_no`) Yes면 `floe-index drc <db>`(--pack은 no-op이라
  생략)를 실행하고
  로그를 공용 **모달 다이얼로그**(`_index_modal`, cancel =
  terminate)에 실시간 표시 후 로딩(`_drc_open_db`/
  `_drc_pack_and_load`, 바이너리는 vfsclient.find_binary). load
  layout의 VFS 인덱싱과 같은 헬퍼를 공유.
- **룰 검색**(2026-08-15/18): 검색 박스는 **룰 목록 상단**(구
  prev/next 버튼 자리 nav 행에서 이동 — n/p 키는 유지). 룰 이름
  부분일치(대소문자 무관) 실시간 필터. TreeView 내장 typeahead
  검색은 rules/grid 모두 비활성(set_enable_search False).
- **좁은 pane 열화 규약**(2026-08-18): 내용이 옆으로 잘리거나
  패닝되면 안 됨 — 룰 목록 hscroll NEVER(이름 열 ellipsize END로
  "…" 줄임), 상세 TextView WORD_CHAR 줄바꿈 + hscroll NEVER,
  필터 컨트롤 행 = **FlowBox**(좁으면 여러 줄로 래핑). 0.12.444(사용자 2026-11-11)부터 이
  행은 `in view`·`selected`·유형 콤보·waive 콤보 넷이 **한 줄**이다: FlowBox가 넷을 한 줄에
  놓는 폭을 청하고 그만큼만 받는다(min 2 / max 4 children per line, halign START). 채우게 두면
  남는 폭을 열마다 나눠 벌어졌고, 다섯째 컨트롤이 둘째 줄을 만들면 그 줄의 콤보가 `in view`와
  `selected`가 선 열을 넓혔다. 넷의 폭보다 좁은 pane에서는 둘씩 접힌다. `Conflict only` 체크는
  만들고 연결하되 어느 행에도 넣지 않는다(기능을 열 때까지; `_conflict`).
  lpaned pack1 shrink=True + 검색 박스 width_chars 8이 pane 폭
  하한을 낮게 유지(실질 하한 = 미니맵 196px).
- **룰|그리드 분할 = 비율 유지**(2026-08-18): hsplit 위치는 고정
  px가 아니라 **pane 폭의 분수**(초기 0.44) — 드래그가 분수를
  재고정하고 size-allocate가 재적용(guard 플래그로 루프 차단).
  분수는 끌기 전까지 이 값이다(0.12.444): GTK가 양쪽의 청한 폭으로 정하는 첫 위치를 끌기로
  받아들여, 그리드 쪽 내용이 바뀔 때마다 분할이 따라 움직였다(초기값 0.45는 쓰인 적이 없고
  실제 위치가 0.44였다).
- **그리드 쪽의 배치**(0.12.444, 사용자 2026-11-11): 위에서부터 CD 컨트롤 줄, 세로 Paned
  (`_vsplit`: 위 = 그룹 목록과 그 페이지 줄 `_delta_list`, 아래 = 에러 그리드와 **그 아래의**
  에러 페이지 줄). 구분선을 끌어 두 목록의 높이를 나눈다(처음 `DRC_GROUPS_PX` 108, 그룹 목록의
  하한 두 줄; CD 부분이 숨으면 그리드가 전부). 그룹 페이지 줄(`_delta_bar`)은 두 페이지 이상일
  때만 보이고 `1/3`만 적는다. 에러 페이지 줄도 `1/3` 꼴이다. 패널의 버튼은 낮다
  (PANEL_CSS `.floe-drc-low button`: 페이지 화살표, CD 컨트롤, 필터 콤보).
  구 고정 220px는 시작 pane(196px)을 통째로 삼켜 그리드가 0폭으로
  숨고 핸들이 가장자리에 붙던 원인.
- **룰 행 카운트 = 에러수/waived**(2026-08-18, "30/20" 형태):
  둘 다 O(1)([wcount]) — waive 필터의 룰 숨김 판정은 기존
  `_drc_wf_count` 그대로. waive/unwaive는 all 필터에서도 해당
  행 텍스트를 제자리 갱신.
- **SVRF 룰 메타데이터**(2026-08-15, SPEC-FORMATS `<deck>.rules.json`
  참조): db 로드 시 자동 탐색(`_drc_rules_auto` — `floe2 drc-prepare --svrf`가 분석 폴더에 적어 둔
  사이드카가 맨 먼저다(0.12.460); 그다음 Rule File Pathname
  베이스네임의 **db 옆** 사이드카 우선, 기록 경로·`<db>.rules.json`
  차선; 작업 디렉터리가 있으면 이어서 그 셋의 DIR 아래 자리, 0.12.450 — 자리는 서비스의
  `svrf_rules_candidates`가 대고(app-core `svrf::rules_candidates`, DRC 캡처와 같은 목록) 뷰어는 그 가운데
  첫 파일을 읽는다) + DRC 메뉴 load SVRF rules… 수동 로드. 매칭 수는 자동 탐색이면 로드의 상태 줄
  `· svrf 매칭/전체`, 수동 로드면 그때의 상태 줄 `rules metadata <파일>: N/M rules matched`
  (패널 정보줄은 2026-11-10에 뺐다).
  이 블록은 0.12.444부터 상세 pane에 붙지 않고 **SVRF 정보 창**에 나온다
  (`_drc_svrf_dialog`: `i`, DRC 메뉴 `SVRF info…`, 에러 그리드 우클릭 메뉴; 막지 않는 창으로
  상세 pane의 에러 `_drc_detail_at`을 따라 바뀌고, 뷰어의 키를 넘겨받으며, Esc/`i`로 닫는다;
  에러 없이 룰만 고르면 측정 줄을 뺀 블록, 메타데이터에 없는 룰이면 그 사실).
  블록(`_drc_meta_lines`): 덱 원문 제약(constraint:) ·
  **measured / estimated CD** = 마커 치수 vs 한계값과 Δ/%(waive 판단
  보조; 에지쌍=실제 최소 갭·단일 에지=길이·area=신발끈,
  직사각형=가로/세로 후보 중 룰 조건으로 선택, 복잡 도형은 생략).
  추정 CD는 후보와 선택 이유도 상세에 표시한다. CD를 특정할 수 없으면
  `CD unavailable`로 표시한다. eqDRC 데이터는 사용하지 않는다.
  layers(직접 피연산자) · gds(원천 레이어 폐쇄) · derivation 체인
  (`derived` 맵을 `svrf.rhs_operands`로 워크, 6줄 캡). 메타 없는
  룰/사이드카 부재 시 상세는 기존 그대로(추가 줄 0). 적용 상태 줄(`SVRF: applied …`,
  `SVRF: not loaded` 등, `_drc_svrf_status`)도 0.12.447부터 상세 pane에 없다: 룰 이름의 `✓`,
  룰 행 툴팁, SVRF 정보 창이 말한다.
- **측정 차이 그룹**(2026-10-05): SVRF 메타데이터가 있는 룰에서
  룰을 선택하면 에러 그리드 위의 `absolute`가 자동 활성화된다.
  `ratio`로 기준을 바꾸고 `off`로 해제한다.
  그리드 위의 CD 부분(0.12.443, 사용자 2026-11-10)은 컨트롤 한 줄
  (`_delta_controls`: 기준 콤보 `absolute`·`ratio`·`off`와 `step` 버튼, 가로 Box에
  제 폭으로; 좁은 패널에서는 두 글자가 `…`로 줄어 줄이 잘리지 않는다)과 그룹
  목록·페이지 줄(`_delta_list`)이며, **CD 그룹이 없는 룰에서는 둘 다
  숨긴다**(`_drc_delta_shown` → `_drc_delta_refresh`): 룰 미선택, SVRF 조건 없음,
  측정 불가, 측정된 에러 없음(`Unmeasurable`뿐), 에러 0개. CD 그룹이나 에지 길이 그룹이
  있으면 보이고, 룰 전체에서는 서비스가 `has_cd`(어느 에러든 CD가 측정됨)라고 하면
  비율 기준에 그룹이 없어도 보인다(한계값 0). 측정 중에는 그 룰에서 직전에 본 결과
  (`_drc_delta_seen`), 없으면 룰 전체 개수(`_drc_group_count`: 0이면 숨김, 모르면 보임).
  `off`에서는 목록을 숨기고, 그룹이 없다고 확인된 적 없는 룰에 한해 컨트롤 줄만 둔다.
  간격은 `step` 버튼의 모달 `CD group step`(`_drc_step_dialog`: 지금 간격에서 시작,
  단위 표시, Enter/`ok` 적용, 거절 사유는 모달 안에, Esc/`cancel`은 그대로)에서 받는다.
  패널의 입력란과 `set step` 버튼, 숨긴 상태의 안내 문구(`select a rule for CD groups`,
  `grouping off`, `load matching SVRF metadata for CD groups`)는 없다. 지금 간격은 버튼
  툴팁과 `All errors` 행의 툴팁(`_drc_delta_summary`: 그룹 수 · `auto step N` / `step N` · 에지
  길이 그룹 수 · 추정 CD 수; 0.12.443에는 목록 아래 줄)에 나온다. 0.7초(`DRC_SAY_AFTER_S`)를
  넘기는 측정은 상태 줄에 `CD groups: …`로 진행과 완료를 말한다.
  **그룹 목록의 우클릭 메뉴**(0.12.444, `_on_drc_delta_menu`): 행이 범위가 되고 select all /
  deselect all / waive all / unwaive all / mark not for review all / clear not for review all
  (`_drc_scope_all`). select all은 나열된 것(waive 필터 적용), 나머지 넷은 필터와 무관하게
  범위 전체이며 그리드의 선택을 건드리지 않는다: select 작업으로 범위를 모으고
  (`_drc_selection_submit(then=)`) 그 결과에 review 작업을 한다(`_drc_scope_apply`).
  기본 간격은 룰 전체의 델타 범위에서 약 10개 그룹이 되도록 자동 계산한다.
  제약별 범위를 사용해 서로 다른 단위를 섞지 않고, 가장 넓은 범위를
  기준으로 공통 간격을 정해 각 제약당 최대 10개 구간으로 나눈다.
  다섯 자리 정밀도와 빈 구간 제외로 실제 그룹 수는 더 적을 수 있고,
  서로 다른 제약과 Unmeasurable 행 때문에 총합은 10개를 넘을 수 있다.
  클러스터/waive 필터와 독립적이며 절대 차이·비율을 따로 계산한다.
  직접 입력한 간격은 룰별·모드별로 기억하며 `auto` 또는 빈칸으로
  자동 간격을 복원한다. DRC/SVRF 메타데이터를 다시 로드하면 초기화한다.
  절대 차이는
  `abs(CD - bound)`, 비율 차이는 `abs(CD - bound) / abs(bound) × 100`이며
  둘 다 0 이상의 위반 크기를 표시한다(2026-10-06).
  상세의 델타·비율과 그룹 구간에는 양수의 `+` 부호를 붙이지 않는다.
  공통 측정/제약 선택(app-core `drc::analysis::measure` Selector, 2026-10-11
  Rust 이식 - 기준 구현은 `tools/oracle/floe_oracle/drc_delta.py`)을 상세 창
  (gtk-service `drc_cd_info`)과 그룹 계산이 함께 사용한다. 산술은 CPython의
  `math.hypot`·`math.fsum`과 같은 방식이라 비교·다섯 자리 경계에서도 기준과 같다.
  CD와 bound를 `Decimal(str(value))`(float는 가장 짧은 왕복 자릿수)로 받아 소수 다섯 자리
  `ROUND_HALF_UP`으로 먼저 정수화한다. 비율은 이 정수값의 정확한
  비로 계산한 뒤 다섯 자리로 반올림한다. CD 룰러 라벨도 같은 다섯
  자리 정밀도로 표시한다. 간격은 다섯 자리보다 유효
  소수 자릿수가 많으면 거부하며, 끝의 0은 허용한다.
  그룹 키는 `(제약 인덱스, floor(차이 정수 / 간격 정수))`이고,
  0 기준 반개구간 `[하한, 상한)`이다. 예: 간격 `0.10000`에서
  `0.10001`은 `[0.10000, 0.20000)`에 속한다.
  후보 선택(2026-10-06)은 원래 정밀도의 CD 후보에 오류 출력 조건을
  먼저 적용한다. 하나만 맞으면 그 값, 여럿이면 `<`/`<=`는 작은 값,
  `>`/`>=`는 큰 값을 사용한다. 직사각형 후보의 방향을 룰에서 추론한
  값은 `estimated CD`다. 같은 문장의 범위 조건은 AND로 평가하며
  `>L<U`는 상한 기준 작은 후보를 선택한다. 서로 무관한 제약을
  같은 구간으로 합치지 않는다. 후보가 없거나 서로 다른 측정문이
  맞아 어느 기준인지 특정할 수 없는 경우는 측정 불가로 유지한다.
  경계에서는 좌표·측정값의 수 ULP 규모 연산 오차만 허용하고,
  이 보정으로 판정이 달라지거나 해석 불가 대안 제약이 남으면 추정 표시한다.
  제약 번호와 단위를 그룹 이름에 포함하며 길이 `um`, 면적 `um2`,
  비율 `%`를 구별한다. 그룹의 추정 CD 개수는 waive 필터와 독립적인
  전체 소속의 개수이고, 추정 여부도 에러 수에 비례하는 수치 배열로 유지한다.
  측정 불가능 에러는 마지막 `Unmeasurable` 그룹에 보존하고,
  비율에서 반올림된 bound가 0이면
  `Unmeasurable / ratio unavailable`에 포함한다.
  그룹 목록은 `All errors`와 50개 단위 페이지, 선택 그룹의 에러 목록은
  기존 `DRC_PAGE(1000)` 단위 페이지이다. 파일 클러스터 ∧ 측정 그룹
  ∧ selected ∧ in view ∧ waive를 교집합으로 적용하며, 캔버스는
  에러 페이지와 독립적으로 선택 그룹 전체의 개별 마커를 표시한다. 그룹 카운트는
  waive 필터를 반영하되 0개 그룹도 행을 유지하여 활성 범위를 보존한다.
  최초 전체 룰 측정은 취소 가능한 백그라운드 작업이다. 현재 룰의
  측정 정수 배열을 재사용하여 기준/간격/파일 클러스터 변경 시 재측정을
  피하고, 그룹 디렉터리와 소속도 수치 배열로 유지한다. 그룹 행은
  요청한 페이지에 대해서만 생성하며, 구형 결과는 새 범위에 설치하지
  않는다. waive 갱신은 그룹 카운트와 상태별 page/rank를 함께 갱신한다.
  측정·그룹·마커·in-view(큰 룰)·박스 선택·대량 waive/note는 floe2 gtk-service의
  작업 스레드(종류·DB마다 하나, 새 요청이 이전 것을 취소)에서 실행하고
  `{"event": "drc", …}` 줄로 진행·중간·완료를 알린다(`rust/floe2/src/drc.rs`).
  10만 건 이상 룰의 측정표와 그룹은 `.analysis` 디렉터리에 저장해 룰 왕복과
  재시작에서 재사용한다. 그리드 채우기는
  사용되지 않는 페이지 geometry를 디코딩하지 않는다. 대규모 전처리,
  공간 인덱스와 중심·측정 캐시 저장 비용은 [DRC.ko.md §5](DRC.ko.md#5-성능-경로와-검증) 참고.
- **룰 유형 필터**(2026-08-17, rules.json 필요): nav 행 콤보 —
  "all types" + 사이드카 constraint **metric별 룰 수**(width/space/notch/
  enclosure/overlap/extension/area/density/… DRC_METRICS 정순, 그 외 알파벳순;
  측정문 없는 매칭 룰·미매칭 룰 = other). `_drc_types_rebuild`가
  로드/사이드카 부착 시 룰당 metric frozenset(`_drc_rtypes`)을
  굽고, `_drc_fill`이 검색·waive 필터와 **교집합**으로 룰 목록을
  거름(다중 측정문 룰은 각 유형에 모두 매칭). 유형 전환은 룰만
  숨기므로 룰별 선택은 유지(waive 필터와 달리 초기화 없음), 열린
  룰이 살아남으면 재선택. 사이드카 없음 = 콤보 비활성·all 강제.
- **필터/페이지네이션**(2026-08-14): 룰 목록은 **All = 전체 룰
  (에러 0개 포함)**, Not Waived/Waived = 매칭 0개 룰 숨김.
  All/Not Waived/Waived 콤보가 status 바이트 기준으로 룰 카운트·
  그리드·in-view 필터·캔버스 페인트를 일괄 필터링. **성능 계약**:
  카운트 = [wcount] O(1), 필터 그리드 베이스 = `('status', waived)`
  lazy 서술자 — `status_page`/`status_rank`는 룰당 4M-청크
  waived 카운트 캐시(첫 필터 접근 시 1패스 생성, set_status가
  증분 유지)로 시작 청크까지 status 접근 없이 점프하고 **최대 한
  청크만 스캔**(2026-08-18: 구현은 rank가 위치까지 전량 합산이라
  100M 룰 n/p가 O(룰 크기)였음 — 계약 위반 수정). 어떤 크기의
  룰도 필터 리스트를 실체화하지 않음. in-view 목록(`_drc_hl_list`)의
  waive 판정은 `query_rect(waived=)`로 **쿼리 내부 cap 이전**에
  적용(2026-08-17: cap 결과에 대한 후필터는 cap 뒤의 매칭을 누락
  — capped 표기도 필터 후 개수 기준이라 누락을 숨겼음)(실데이터의 waived
  표기법 미파악 상태라 현재는 전부 Not Waived; v1은 status 없음 →
  Waived 항상 빈 목록). 그리드는 **DRC_PAGE(1000)셀 페이지** —
  ◀ `페이지/전체` ▶ 바(0.12.444부터 그리드 아래), n/p 스텝이 자동으로 페이지를
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
  `_drc_open`). **db 로드 직후 = 무선택 상태**(2026-08-18 사용자
  결정 — 자동 열기 금지): 모델 재부착 중 GTK가 행 0을 유령
  선택하면 busy 가드가 이벤트를 삼켜 "선택돼 보이는데 그리드는 빈"
  상태가 되므로, load_drc가 unselect_all로 유령 선택을 걷어낸다.
- **사용자 클러스터**(2026-10-05): `<db>.clusters`가 정의한 룰은
  TreeStore의 부모 행, 클러스터는 하위 행이다. 부모는 전체 에러,
  하위 행은 그 소속만 표시한다. 미지정 에러는 `Unclustered`로 보존하고
  룰 내부 번호를 다시 매기지 않는다. 파일이 없는 룰은 단일 행이다.
  검색/유형 필터는 부모 룰에 적용하고, waive 필터는 빈 하위 행도 숨긴다.
  모델 재구축 후 펼침 상태 및 선택한 클러스터를 유지한다(필터에 가려진
  선택은 해제). 클러스터 ∧ selected ∧ in view ∧ waive를 교집합으로 적용한다.
  `./,` 및 페이지 점프는 클러스터의 lazy count/page/rank를 사용하고,
  in-view 소속은 `query_rect(members=)`의 cap 이전에 판정한다.
  선택은 `(룰, 클러스터)`별로 보존하여 다른 클러스터의 에러가 일괄
  waive/note 작업에 섞이지 않게 한다. 로딩/문법은 SPEC-FORMATS의
  `<db>.clusters` 절을 따른다.
- 그리드 = 전용 TreeView, **pane 폭에 맞춰 열 수 재배치**(가로
  스크롤 없음 — hscroll NEVER, size-allocate에서 열 수 = 폭 ÷ 셀
  폭[**현재 룰의 최대 번호 자릿수** 프로브 — 룰 전환/그리드 갱신마다
  재계산]; `_drc_grid_set_cols`가 모델/열을 재구축하고 마킹 셀
  유지). 셀 = 전역 에러 번호(레코드 디코드 없음
  — 번호 = cum[ci]+i+1), 행 선택 모드 NONE — 클릭한 **셀 하나만**
  마킹(markup 배경, `_drc_cell_mark`).
  셀 단클릭 = 하단 상세(`#번호 [상태]`·`rule:`·리뷰 줄·note·룰 설명·`edge (um):`/
  `polygon (um):` 좌표 목록[상한 64~128줄]; bbox·종류 표기는 없음 —
  waive 상태는 v2 get_status, v1은 '-'), 더블클릭 = 점프(점프도
  상세 갱신). 룰 행 클릭 = 룰 이름(`rule:`)·룰 설명(`_drc_show_rule`; 클러스터 행을
  골라도 같다). 룰 설명은 양쪽 모두 덱의 `Rule File Pathname:`·`Rule File Title:` 줄을
  뺀 것이고(`_drc_desc_lines`), 그 둘뿐이면 이름 줄만 남는다. 0.12.447에 룰 상세의
  `errors: N (declared N)`·`delta group: …`·덱 경로와 제목 줄, 그리고 양쪽의 SVRF 상태 줄을,
  0.12.448에 `cluster: 이름 (N errors)` 줄을 뺐다(사용자 2026-11-11; 수는 룰 목록의 열, 상태는
  `✓`·툴팁·SVRF 정보 창, 클러스터는 룰 아래 고른 행, 그룹은 그룹 목록의 행; 결과 파일이 적은
  `declared` 수와 덱 제목은 뷰어에 나오지 않는다). 상한
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
- **in view**(체크박스, pack 전용 — 구 filter errors in
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
- **룰/클러스터 전체 마커**(2026-10-08): 선택한 룰·클러스터·CD 델타
  그룹의 **전체 에러** 중 중심이 화면에 포함된 에러를 개별 원으로 표시한다.
  그리드 페이지는 마커 조회 키에 포함하지 않는다. waive/selected 필터는
  적용하며, `in view`는 목록만 좁힌다. gtk-service `drc_markers` 작업이
  app-core `drc::analysis::points::query`로 그림을 만들고 GUI는
  `gtkservice.PointMarkers`로 받는다(기준: `floe_oracle/drc_points.py`).
  **에러를 개수 상한으로 잘라내지 않는다**. 같은 중심은 한 번만 그리며,
  서로 다른 중심도 현재 화면의 같은 픽셀에 투영되면 중복 표시를 생략한다.
  같은 픽셀에서는 첫 not waived 에러(없으면 첫 waived)를 선택 대상으로
  남긴다. 원본 좌표를 보존하므로 확대하면 다시 구분된다.
  개별 마커는 불투명 5px AA 원(포커스 9px)이다. not waived=red,
  waived=green이며 같은 픽셀의 혼합 상태는 red를 우선한다.
  포커스/선택(gold)은 그 위에 개별 마커로 강조한다.
  **실도형은 점프된 에러(drc_mark 대상) 하나만** 표시하며, 화면 스팬이
  DRC_MARK_PX보다 작으면 원형 마커로 표시한다.
  `.tray`의 정확한 bbox 극값의 합을 원본 에러 순서의 little-endian int64
  Nx2(`centers-r1-<identity>-<rule>/centers.bin`, 단위 1/(2×precision) µm)로
  저장한다. 음수·반 DBU 중심을 유지하며 qbox 근사 위치를 사용하지 않는다.
  룰별 16B/에러 파일은 pack identity에 연결해 원자적으로 공개하고 mmap한다.
  처음에는 블록을 작업자들이 병렬로 복원하면서 바로 그림에 쓰고, 끝나면
  캐시를 공개한다(폴더에 쓸 수 없으면 캐시 없이 그린다). 취소는 작업 단위마다 확인한다.
  조회는 65536개 단위로 정확한 중심의 화면 포함 여부·소속·상태를
  확인한다. 중심이 화면 밖이면 가장자리로 끌어오지 않는다. 단, 닫힌 경계
  위의 중심은 마지막 픽셀에 포함한다. ASCII DB는 정확한 bbox 중심을
  청크 단위로 순회하여 동일한 원형 마커를 만든다.
  출력은 RGBA 이미지와 H×W 에러 ID 배열을 담은 임시 파일(`FLOEMRK1`)이다.
  마커 리스트나 에러당 GTK 호출을 생성하지 않는다. GUI는 마커
  레이어를 캐시하고 프레임마다 한 번 합성한다. 포인터 주변 13×13 픽셀만
  확인해 선택하며 같은 거리에서는 포커스/gold를 우선한다. Tab 숨김은
  픽셀 hit map도 비운다. 중간 결과를 내보였던 계산은 끝날 때 상태 줄에 원본 에러 수와 실제
  표시 중심 수를 구분해 말한다(`DRC markers: N errors / M markers in view`; 패널 정보줄은
  2026-11-10에 뺐다).
  마커 작업은 DB마다 하나이며 새 요청이 앞선 작업을 취소한다(GUI의
  `gtkservice.MarkerWorker`가 마지막 요청만 기다린다). 뷰/캔버스 크기/DB/룰/클러스터/CD 그룹/
  status revision/selected 소속이 키이며, 현재 키와 다른 결과는 버린다.
  좌표는 실제 표시한 obox/ospp를 사용하며, 박스 선택도 렌더 대기 중
  동결된 표시 좌표와 일치시킨다. in-view 목록의 뷰 추적 호출은 유지한다.
  **마커 hover/pick**: hover 툴팁은 룰명 #로컬(전역)·waived·note 여부를
  표시한다. 좌클릭이 마커 6px 안이면 `_drc_pick`은 **번호 단일클릭과 동일**
  하게 셀 마크·포커스·상세만 변경하고 뷰를 유지한다. **더블클릭은 번호
  더블클릭과 동일**하게 프레이밍·CD 룰러·레이어 격리를 적용한다.
  Ctrl/Shift 클릭은 디자인 선택 제스처를 유지하고 마커가 없으면 디자인
  pick으로 이어진다. 룰을 전환하면 이전 점프 마크·자동 CD 룰러를 지우고
  수동 룰러는 유지한다. Esc로 격리 레이어를 복원할 때 점프 마크와 자동
  CD 룰러도 지우며 현재 위치·포커스는 유지한다.
  **0.5초 진행 표시**: 마커 작업은 약 0.5초마다 `partial` 그림을 보내고
  GUI는 `MarkerProgress(markers)`로 받는다. 그림마다 processed/total
  카운트와 독립된 ID 배열을 가지며, 매 프레임 마커 레이어를 교체한다.
  GUI는 중간 결과에도 busy 상태를 유지하고 `(partial)`과 처리 건수를 표시한다.
  최종 응답은 즉시 busy를 해제하며 오류 응답은 미완성 결과/픽 정보를 지운다.
  GUI 쪽 결과 슬롯은 최신 그림 하나만 보관하고(밀린 파일은 지운다) 다른
  작업의 응답은 버린다. 첫 캐시 생성 중에도 완료된 작업 단위가 바로 그림에
  들어가며, 이후 전체 파일을 재스캔하지 않는다.
  **그리드 숫자도 상태색**(waived green / not-waived red; gold 선택
  배경 위에도 상태색 유지, 현재 셀은 파랑 배경+흰 글자).
- **mono(그레이스케일)**: `b` 키 토글(독립 — 필터와 연동 없음),
  서비스 "mono" 잡 → Renderer.set_mono(디자인 레이어 색을
  luminance 회색으로; 프레임/스페클 구조 불변) + _color_epoch
  무효화.
- **에러 박스 선택**(`e`, 2026-10-08): 크로스헤어 모드는
  **Esc(또는 e 재입력)까지 유지** — 박스 완료 후에도 다음 박스
  대기(룰러와 동일 계열). 두 클릭이 박스를 정의 → **페이지와 독립적인
  전체 룰/클러스터 중 박스와 화면에 걸치는 실제 에러**를 선택한다.
  gtk-service `drc_select` 작업(app-core `drc::analysis::select`)이 status/소속을
  교차하여 개수 상한 없이 조회한다. pack의 블록이 박스 밖이면 건너뛰고 박스 안이면
  일괄 선택하며, 나머지만 정확한 bbox를 복원한다. 선택은 서비스에 남는
  비트셋(에러당 1비트)과 65536개 단위 prefix로 저장해 count·contains·1000개 page를
  전체 열거 없이 처리하고, GUI는 그 번호(`gtkservice.DrcSelection`)만 가진다.
  완료된 결과만 적용하며 취소·실패·룰 전환 뒤의 오래된 결과는 이전 선택을
  덮어쓰지 않는다. 조회 중 Esc는 작업만 취소한다. 마커 숨김 상태에서는 선택하지 않는다. 두 번째
  클릭 수식키: 무수식=대체(빈 박스는 해제), **Shift=추가,
  Ctrl=박스 내 토글** — 그리드 Ctrl/Shift와 동일 계열. 선택은 목록을 **대체하지 않는다**(2026-08-14 수정):
  그리드는 기존 내용(전체 또는 hl in-view)을 유지하고 선택된
  번호만 **gold 배경**으로 마킹(현재 셀 파랑이 우선). 캔버스는
  선택 에러를 **gold 원형 마커**로 표시한다. 서비스의 마커 그림에
  직접 합성하므로 GTK가 선택된 모든 도형을 보관하거나 순회하지 않는다.
  동일 픽셀은 선택된 에러를 우선하고 그 안에서 not waived·원본 순서를 따른다.
  **화면 스팬이 마커 크기(DRC_MARK_PX=5) 미만인 에러는 어느
  경로든 지름 5px 원형 마커로 붕괴**(점프 마크 포함; 포커스는 9px)
  — 임계 = 마커 크기 자체(2026-08-16: 구 ≤2px 임계는 도형이
  마커보다 작게 그려지는 중간 줌 구간을 남겼음 — 재도입 금지).
  해제 = Esc(선택 해제 단계는 마크 해제보다 먼저) / 새 박스.
  룰·클러스터·CD 그룹마다 선택을 보관하며, 리뷰 필터 재검사가 필요한
  큰 선택은 재검사가 끝나기 전에는 활성 선택으로 사용하지 않는다.
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
- **`w` = waive 토글**(2026-08-20, 선택 우선 2026-08-22): ① gold
  선택(`_drc_sel`)이 있으면 **선택 전체를 한 배치로** — 전부
  waived면 unwaive all, 그 외(혼합 포함)는 waive all(w-w가
  왕복하도록 혼합은 waived로 수렴). ② 선택이 없으면 현재 에러 —
  `_drc_focus`(단클릭·클릭형 n/p) 우선, 없으면 `_drc_pos`에서
  역산(점프 중엔 포커스가 None이고, Esc 격리 복원 후에도 pos가
  남아 상세 패널에 보이는 에러가 그대로 토글됨), 새 상태 =
  `not _drc_waived()` 단건. 기록·갱신은 모두 우클릭 메뉴와 같은
  `_drc_set_waived` 경로(점프 마크 색 재유도·필터 연동 포함).
  아무 대상도 없으면 상태줄 안내, v1(비pack)은 pack 안내.
- **그리드 선택 편집**(2026-08-14): **Ctrl+클릭** = 해당 에러를
  선택에 추가/해제(토글), **Shift+클릭** = 현재 셀부터 클릭 셀까지
  시각적 범위를 선택에 추가. 뷰 이동 없음, 박스 선택과 같은
  _drc_sel로 합쳐짐(gold 마킹/캔버스 gold 표시 동일), 전부
  해제되면 선택 자체가 사라짐.
- **점프 규약**(`_drc_jump`): 줌 = 에러 전체 extent가 양축 모두 뷰의
  ~30%(DRC_VIEW_FRACTION, 세로축은 캔버스 종횡비로 환산; 퇴화 시
  0.1µm 창). **CD 자동 룰러**(`_drc_cd_ruler`, 리스트)를 점프마다
  교체 부착: 단일 엣지=길이 1개. 이때 에러선을 가리지 않도록 룰러를
  화면 법선 방향으로 14px 평행 이동하고 원래 양 끝과 룰러 양 끝을
  흰 점선으로 연결한다(줌과 무관한 화면 거리, 측정값은 원래 엣지
  길이). 엣지 쌍=최근접 갭 1개(평행 대면이면 중점 앵커; 교차/접촉은
  없음). 평행하지만 투영 구간이 겹치지 않아 최근접 양 끝이 대각선인
  경우에도 룰러는 **그 대각선 하나**다(2026-11-12, 0.12.453; 사용자: "유클리드 거리로 CD값이 정해지는
  경우(주로 대각선) 가로와 세로 룰러는 표시하지 않게"). CD가 그 유클리드 거리이므로 같은 두 점을 잇는
  수평·수직 성분 룰러 2개(0.12.452까지 대각선 뒤에 붙였다)는 그리지 않는다. 성분 길이가 필요하면 룰러
  모드(`r`)로 두 끝을 찍는다: 상태 줄이 `measure D um (dx …, dy …)`를 보여 준다(0.12.454부터 룰러의 스냅이
  에러 도형의 끝점과 변에 붙는다, §9). 축정렬 사각형은 폭·높이에 SVRF 오류 조건을
  각각 적용하여 해당 방향만 표시한다(2026-10-06). 양쪽이 조건에 맞거나
  미해석 옵션/비치수 제약·메타 부재·조건 불일치로 방향을 정할 수 없으면 두 개를
  유지한다. 구간은 CD 선택과 같은 AND 조건·부동소수점 경계 비교를 쓴다.
  Selector의 `ruler_segments`를 CLI 스냅샷(app-core drc::capture)과 공유하고, 그룹용 CD를 하나
  고르는 것만으로 다른 유효한 방향의 룰러를 숨기지 않는다.
  notch/overlap/extension metric도 거리 측정 대상으로 처리한다.
  EXTERNAL의 `NOTCH`/`SPACE` 분류 옵션은 방향 판정을 허용하며,
  그 외 평가하지 않은 옵션이 있으면 두 방향을 유지한다.
  **area 전용 룰은 길이 룰러 없이 면적 라벨만 표시한다.**
  `area_label`(app-core drc::analysis::measure)이 상세·그룹과 같은 폴리곤 면적을 다섯 자리
  `area … µm²`로 만든다. bbox는 위치 앵커에만 사용한다. 혼합 검사면
  면적 라벨과 치수 룰러를 병행하며 에지 레코드는 면적 라벨이 없다.
  `_DrcAreaLabel`은 선·화살표 없이 기존 거리 칩으로 표시하고 자동
  룰러의 Tab/Esc/k/새 점프 수명주기를 그대로 따른다. CLI PNG에도 같은
  문자열을 텍스트 주석으로 저장한다.
  거리 칩과 실시간 수동 측정 상태줄은 소수점 다섯 자리 HALF_UP이다.
  복잡한 폴리곤/엣지셋은 룰러 생략(사용자 규정 2026-08-13).
  수동 룰러는 보존, k/Esc에는 일반 룰러처럼 반응.

## 8c. 셀 트리 (Calibre cell tree, 2026-09-29)

- **자리**: 왼쪽 pane Notebook의 `cells` 페이지(§6). 위에서 아래로
  검색 박스(`find cell… (* ? wildcards)`) · 트리/결과 목록(TreeView,
  이름 열은 **이름 전체** + 개수 열 우정렬, 검은 배경 `.floe-drc-list`,
  **hscroll AUTOMATIC** — 사용자 요청 2026-09-29, 0.12.243: 깊이 펼치면
  들여쓰기만으로 30단계 570 px가 되고, hscroll NEVER에서는 그 폭이
  페이지 최소 폭이 되어 pane이 왼쪽을 잘라 내며 되돌아올 길이 없었다;
  DRC 페이지의 "ellipsize + 가로 스크롤 금지" 규약은 그 페이지의 것으로
  유지) · 컨트롤 행(**FlowBox** — `highlight` 체크 = 기본 켬,
  `zoom`; 좁으면 여러 줄로 감김. `root`·`top` 버튼은 0.12.289에서 뺐다 —
  뷰 루트는 행 더블클릭, 아래 **뷰 루트**) · 인덱스가 없을 때만
  보이는 `build index…` 행 · 정보 줄(줄바꿈 라벨). `_build_cell_panel`,
  위젯 홀더 `_CellPanel`. **페이지 최소 폭 ≤ 156 px**(테스트
  `test_cell_page_fits_the_left_pane_at_its_start_width`): 현장
  2026-09-29 "cells 탭이 안 보이고 DRC 탭도 반쯤 가림" = 버튼 4개
  HBox(276 px)가 pane 시작 폭(196 px)을 넘자 GtkPaned가 첫 자식의
  **왼쪽**을 잘라 낸 것. pane 시작 폭은 `LEFT_PANE_PX` = 260(0.12.242;
  옛 196은 미니맵의 하한이었고 미니맵은 오른쪽으로 갔다).
- **데이터 원천 = design.ovh**(SPEC-FORMATS): 셀별 **서로 다른 자식**과
  자식별 **배치 멤버 수**(반복 전개), 부모 목록, **탑 아래 인스턴스 수**,
  엣지별 자식 배치의 합집합 범위. design.ovm의 배치 레코드는 부모별
  BVH 순서라 "이 셀의 자식들"조차 그 셀의 레코드 전부를 읽어야 하고
  (MAIN01 1/10 합성: 8,260만 레코드·5.3 GB, MAIN01은 그 10배) "이 셀의
  부모"는 전부를 읽어야 하므로, 뷰어가 열 때 훑는 방식은 성립하지
  않는다. 인덱서가 빌드 끝에 한 번 훑어 파일로 두고(`floe-index vfs`,
  `--no-hier`로 생략; 1/10 합성 11.4 s → 10.6 MB), 이전 캐시에는
  `floe2 index --hier-only <src>`(= `floe-index hier <cache>`)로
  덧붙인다. 레코드 400만 개 이하의 작은 캐시는 파일이 없어도 데몬이
  메모리에서 요약한다(`FLOE_RUST_HIER_INLINE_PLACES`, 진단).
- **질의는 renderd의 hier 스레드**(RUST_RENDERER.md: `cell_sources` ·
  `cells` · `cell_find` · `cell_bbox` · `cell_insts`, 응답은 같은
  kind + `seq`; 실패는 `found=0 code=nohier|superseded|state|query
  err_hex=`). 렌더·픽 스레드와 독립이라 첫 질의의 요약 열기/빌드나
  넓은 뷰의 인스턴스 탐색이 렌더를 늦추지 않는다. GUI는 kind별
  **최신 seq만** 받는다(`_cell_pending`, `_on_cell_result`).
- **트리**: 레이아웃은 탑 셀이 루트이며 열자마자 펼쳐진다(자식 행 =
  `NAME  ×members`, 멤버 1이면 개수 생략); 자식 있는 행은 `…` 자리
  행을 달고 있다가 **처음 펼칠 때 한 번** `cells`로 채운다
  (`test-expand-row`, `Gtk.TreeRowReference`). 채울 때는 **새 자식을
  먼저 넣고 자리 행을 뒤에 지운다** — 마지막 자식이 사라진 행은 GTK가
  접어 버려 첫 펼침이 곧장 닫혔다(현장 2026-09-29, 0.12.240; 펼침
  상태를 기억해 되살림). 한 부모의 자식이
  20,000을 넘으면 `… N more (find by name)` 행. 정렬은 이름(대소문자
  무시). 잡덱은 **소스(TC)마다 루트**(라벨 = 카탈로그의 TC 식별자, 배치가
  여럿이면 `×N`), 펼치면 그 소스 탑의 자식들.
- **검색**: 박스가 비면 트리, 채우면 결과 목록이 같은 자리에 온다
  (모델 교체). 규칙은 대소문자 무시 **부분일치**, `*`/`?`가 있으면
  전체 이름 **글롭**. 결과 행 = `NAME  insts`(탑 아래 인스턴스 수),
  상한 2,000행 + 전체 개수 정보 줄. 키 입력 뒤 150 ms(SearchEntry 자체
  지연 포함 약 300 ms) 후 질의.
- **선택**(트리·결과 공통, 단클릭): `cell_info`로 **상태줄**과 패널 정보
  줄(`NAME: N instances · W × H um`, 근사면 `(the blocks holding it)`)을
  채운다(아래 "상태줄의 셀 정보"), highlight가
  켜져 있으면 `cell_insts`로 **현재 뷰 안 인스턴스 박스**를 받아 캔버스에
  **CELL_HL(#40E0FF) 2 px 외곽**으로 그린다(화면에서 7 px보다 작은
  인스턴스는 7 px 마커 사각). 상한 4,096박스·탐색 예산 200만 방문 —
  넘치면 `more`, 상태줄 `(more - zoom in)`. 프레임이 착지할 때마다
  뷰 키(src, ci, 뷰 박스 반올림)가 바뀌었으면 다시 묻는다
  (`_cell_hl_follow`; 데몬은 대기 중인 더 새 insts 질의가 있으면 옛것을
  `superseded`로 즉시 답한다). Tab 오버레이 숨김 상태에서는 그리지 않음.
- **상태줄의 셀 정보**(2026-10-15, app 0.12.358 / RENDERD 0.12.326; 사용자:
  "셀 트리에서 셀을 선택하면 셀의 관련정보(좌표등)을 상태줄에 표시해줘"):
  행을 고르면 renderd `cell_info`(hier 스레드, RUST_RENDERER.md)를 뷰 루트
  기준으로 묻고, 답의 `text` — app-core `view::cell_info::status_line`이
  쓴 문장(웹도 같은 것을 쓴다) — 를 상태줄 윗줄에 둔다. 답이 오기 전에는
  셀 이름, highlight가 켜져 있으면 뒤에 `· N in view`(넘치면 `(more -
  zoom in)`). 커서가 움직이면 `x … y … um   |   [선택한 도형]   |   셀
  정보` 순으로 이어 쓰므로 셀 정보가 사라지지 않는다(눈금자·오류 선택
  모드는 제 안내만). Esc 체인에서 선택 해제 다음 단계(highlight가 꺼져
  있어도)가 셀 선택과 이 줄을 지우고 `cell selection cleared`; 뷰 루트가
  바뀌면 새 루트 기준으로 다시 묻되 루트 안내 문구를 덮지 않는다. 문장:
  - 한 번 배치: `B · 1 placement at (6.700, 3.500) R270 MX · bbox (6.600,
    3.100) – (6.700, 3.500) um · 0.100 × 0.400 um · leaf` — 원점과 방향은
    루트까지 합성한 배치(`R0`~`R270` = 반시계 90° 단위, `MX` = 회전 전
    x축 대칭, GDS/OASIS 순서), bbox는 그 인스턴스의 상자.
  - 여러 번: `VIA · 39 placements · extent (1.300, 1.300) – (15.400,
    10.700) um · cell 0.100 × 0.100 um · leaf` — extent는 인스턴스 상자의
    **정확한 합집합**: 루트가 직접만 배치한 셀은 요약(design.ovh)의 범위가
    곧 그것이라 걷지 않고, 더 깊은 셀은 render-core `cells::info`가 요약의
    도달 범위로 가지친 걷기로 전부 세어 얻는다(`cell_bbox`의 블록 범위보다
    정확). 예산(`CELL_INFO_BUDGET` 200만 방문)을 넘으면 요약의 범위로
    `within … (the blocks holding it)`. `cell`은 셀 자체 크기.
  - 루트: `TOP · top cell · bbox … · 20.000 × 12.000 um · 2 children · 3
    levels below`(뷰 루트면 `view root`), 루트 아래에 없으면 `not placed
    under the top cell`/`the view root`, 도형 없는 셀은 `no shapes`(상자가
    없어 걷기로 찾을 수 없으므로 한 번 배치여도 자리는 없다).
  - 잡덱: 소스 배치를 곱한 개수 `6,000 placements (1,500 × 4 source
    placements)`, 소스 탑은 `source top cell`, 좌표는 덱 dbu(소스 배치로
    옮긴 것), 한 번 배치는 덱 전체에서 하나일 때만.
  - 좌표는 뷰의 좌표(탑·뷰 루트·덱)를 µm로, 소수 자리는 dbu가 필요한
    만큼(3~6자리; 셋째 자리 뒤의 끝 0은 뗀다 — 0.25 nm 격자의
    15960.40400은 `15960.404`), 개수는 천 단위 쉼표. 창이 좁으면 줄 끝이
    `…`로 잘리고(앞쪽이 좌표) 전체는 툴팁에 있다.
  실측(macOS): valmini의 한 번 배치된 DEEPA 0.01~0.06 s, 탑이 직접 225만 번
  배치한 MEGAFILL 0.05~0.06 s(요약의 범위; 걷던 첫 구현은 예산까지 0.08 s); 합성 MAIN01 1/10(배치 레코드 8,260만)의 TOP·한 번
  배치된 H01_00003·6배치 H01_00001·3,876만 배치 LF00_00004 모두 0.04~0.07 s
  (직접 배치 셀을 걷던 첫 구현은 H01_00001에 0.26 s). 게이트: `cell_tree` C11(KLayout 대조),
  `rust_renderer` `test_a_selected_cell_says_its_facts_on_the_status_line`,
  `gtk_view` `cell_status`(실제 뷰어), 단위 render-core `cells::info`·
  `transform::parts`, renderd `cell_info_answers_in_view_coordinates`,
  worker-client, app-core `view::cell_info`.
- **줌**(`zoom`·검색 박스 Enter·메뉴; 잡덱은 행 더블클릭·Enter도 — 레이아웃의
  행 더블클릭은 0.12.289부터 뷰 루트): `cell_bbox`의 범위를 DRC 점프와
  같은 규칙으로 프레이밍(양 축 0.8, `CELL_VIEW_FRACTION`). 범위 =
  탑이 **직접 배치한 셀은 그 인스턴스 박스들의 정확한 합집합**, 더 깊은
  셀은 **그 셀을 품은 탑 직계 블록들의 범위**(요약이 엣지별 합집합만
  갖고 레코드별 변환은 갖지 않으므로; `approx=1`, 상태줄
  `(zoomed to the blocks holding it)`) — 이후 하이라이트가 정확한
  인스턴스를 보여 준다. 탑 아래에 없는 셀(orphan)·도형 없는 셀은
  줌하지 않고 상태줄로 이유를 말한다.
- **키/Esc**: `t` = cells 페이지 올리고 검색 박스 포커스(pane 폭 하한
  260 px); Esc 체인에서 선택 해제 **다음** 단계가 셀 하이라이트 해제
  (`_cell_hl_clear`: 하이라이트와 트리 선택, 상태줄의 셀 정보를 함께
  지움; highlight 체크는 유지; 0.12.358부터 highlight가 꺼져 있어도 고른
  셀이 있으면 이 단계가 온다). 검색 박스에 포커스가 있으면 캔버스 키는 오지 않는다
  (§7의 Entry 가드).
- **인덱스 부재**: 어떤 소스의 답이 `code=nohier`면 정보 줄 안내 +
  `build index…` 버튼, 그리고 **소스당 로드마다 한 번** "지금 만들까요?"
  (FLOE_INDEX_ON_OPEN 정책, VFS 인덱스·DRC pack과 동일) → `floe-index
  hier <cache>`를 모달 로그(`_index_modal`)로 돌린 뒤 트리를 다시 읽는다.
  버튼·메뉴는 다시 묻는다.
- **잡덱 좌표**: 소스 좌표의 박스는 덱 배치(`scale·p + (dx, dy)`, 소스당
  기하 배치 중복 제거)로 덱 dbu로 바뀌어 오고, `cell_insts`의 뷰는 반대로
  소스 좌표로 들어간다(render-core `DeckXf`). 소스가 여러 곳에 놓이면
  박스도 그만큼.
- **뷰 루트**(2026-09-29, 0.12.241 / RENDERD 0.12.229 — Calibre 셀
  트리의 "선택한 셀이 표시되는 탑이 된다"): Cell 메뉴 · **트리 행 더블클릭**
  (Enter도 같다; 찾기 결과 목록의 행도)이 **그 셀을 뷰 루트**로 삼고(Ctrl+T 키는
  사용자 요청으로 2026-09-30 삭제, 0.12.254 — 눌러도 아무 일 없음, 평문 `t`의
  트리 포커스로 흘러가지도 않음; 계약 `test_ctrl_t_is_no_shortcut`),
  `Ctrl+Shift+T` · 메뉴 · **탑 셀 행 더블클릭**이 탑 셀로 돌아온다. 0.12.289
  (사용자 2026-10-04: "셀 트리에서 셀을 더블클릭하면 해당 cell을 root로
  변경하도록 해줘(root 버튼 기능). root 버튼과 top 버튼은 제거해줘."): 패널의
  `root`·`top` 버튼을 뺐고, 더블클릭은 `_on_cell_activate`가 받는다 — 자리표시·
  `more` 행은 무시, 잡덱의 행은 뷰 루트가 없으므로 전처럼 줌(계약
  `test_a_double_click_in_the_cell_tree_sets_the_view_root`). 뷰 루트가
  서면 그 셀의 **자기 좌표**가 세계가 된다: 플래너(`ViewReq::root`,
  `PlanRequest::root`, 와이어 `render … root=CI`)가 그 셀에서 출발하고
  depth도 그 셀부터 센다; 라벨 플래너도 같은 루트에서 걷는다; 픽/스냅은
  발행 씬을 그대로 읽으므로 루트 좌표; 클립(`clip … root=`)도 루트에서
  잘라 낸다. GUI는 다이 bbox를 루트의 재귀 bbox로 바꾼다
  (`_die_bbox`: fit·clamp·미니맵; 미니맵의 구운 프런티어는 탑 것이라
  루트에서는 그리지 않음), 렌더 상태 키(`_render_key`)에 루트를 넣어
  옛 프레임·마진이 새 좌표계에 나타나지 않게 하고(전환 때 화면과
  in-flight 프레임 키를 비운 뒤 fit), 창 제목 뒤에 `· root NAME`을 붙인다.
  renderd도 fit 메모리 키와 RetainedKey에 루트를 넣어 팬 재사용·결정
  기억이 루트별로 갈린다. **루트가 서면 꺼지는 것**: 점유 요약(design.ovo는
  탑을 평탄화한 것 — `summary: none`), 대표 파일(design.ovr, 탑 기준
  표본)은 plan 조건상 루트에서는 쓰이지 않음(page_reps 무관). 트리는 계속
  전체 계층을 보이며, 선택 셀의 정보·하이라이트·줌은 **루트 아래**에서
  센다(`cell_bbox`/`cell_insts … root=CI`: 루트 직계면 정확한 범위,
  루트 밖 셀은 "not placed under the view root"). 잡덱은 소스 탑들이
  덱의 셀이므로 뷰 루트가 없다(renderd가 `render root=`를 거부, GUI는
  상태줄 안내). 게이트 `cell_tree` C8: 루트=BLK 프레임 == BLK를 탑으로
  한 별도 레이아웃(KLayout copy_tree)의 프레임, 바이트 동일.
  **빈 루트(2026-09-30, RENDERD 0.12.231):** 파일의 탑은 모든 레이어를
  가지지만 루트는 아닐 수 있다. 보이는 레이어를 하나도 갖지 않은 루트
  (또는 통째로 컷 아래인 루트)는 계획에 작업 셀이 없고, 씬이 탑을 찾지
  못해 `invalid plan: top … is missing` 오류로 프레임이 없었다(합성
  칩에서 발견; density stack의 2패스는 최상위 평면의 레이어만 따로
  계획하므로 대부분의 루트에서 같은 오류). 이제 루트 요청의 빈 계획은
  빈 루트 작업 셀을 받아 **빈 그림**이 된다(`Cache::plan`, summary
  레이어의 같은 처리를 일반화; 잡덱의 "컷 아래 소스 = 건너뛴 패스"는
  루트가 없으므로 그대로). C8: VIA(1/0만)를 루트로 2/0만 켜면 검은
  프레임, 1/0이면 그려짐, density stack 켬 == 끔.
  **파일이 이름만 둔 빈 레이어(2026-10-04, app 0.12.296 / RENDERD 0.12.273;
  사용자: "빈 레이어 오류도 고쳐줘"):** 레이어 표에는 있지만 어느 셀에도 없는
  레이어(라우팅 칩의 BOUNDARY 100/0, LAYERNAME만 있는 레이어)를 그것만 켜면
  루트가 없어도 계획이 비어 같은 오류가 났다(full depth, 또는 depth 0에서
  프레임 끔. depth 0에서 프레임을 켜면 depth 밖 셀의 외곽선이 작업 셀을
  만들어 오류가 없었다).
  - 이제 뷰어의 프레임 요청은 `PlanRequest::empty_top`을 켠다. 빈 계획은
    빈 탑 작업 셀을 받아 **빈 그림**이 된다(depth 0·프레임 켬은 그
    외곽선만).
  - 잡덱 소스의 요청은 끈다. 빈 계획은 지금처럼 건너뛴 패스다.
  - C9: 1/0 도형과 이름만 있는 3/0(NOTHING)을 둔 레이아웃에서 3/0만 켠다.
    - full depth(프레임 켬·끔)와 depth 0·프레임 끔은 검은 프레임이다.
    - depth 0·프레임 켬은 1/0을 켰을 때와 같은 외곽선 픽셀만 있다.
    - density stack 점을 켜도 같다. 1/0을 켜면 그려진다.
- **남은 것**: 깊은 셀의 줌 범위가 블록 범위인 것은 요약에 레코드별
  변환을 두면 정확해진다. 인스턴스 탐색은 뷰를 요약 엣지 범위로 잘라
  BVH를 걷지만 넓은 뷰에서 큰 블록 안의 드문 셀은 예산에 걸릴 수 있다
  (`more`, 줌인).
- **루트의 depth(2026-09-30, 0.12.255; 사용자: "cell이 root가 되면 depth가 달라질
  수 있는데 고려가 안 된 것 같음").** 플래너는 처음부터 루트에서 depth를 셌지만
  뷰어는 파일 top의 높이(데몬이 매 프레임 알려 주는 `max_depth`)를 그대로 썼다 —
  라벨이 `d/16`, `<`/`>`는 루트 높이를 넘는, 그림이 바뀌지 않는 단계를 걸었다.
  이제 `_max_depth()` = 루트가 서면 루트 셀의 높이(`cells` 답의 `height`), 아니면
  top의 높이: 라벨 `depth: d/h`(d ≥ h면 `*` — 그 루트 전체), 단계 이동은 [0, h]로
  제한(전체·h 초과는 h에서 한 단계 내려감), depth 값 자체는 보존해 `top`으로 돌아오면
  원래 뷰; 루트가 바뀌면 라벨을 바로 갱신, depth 대화상자는 "0 = the view root only".
  계약 `test_under_a_view_root_the_depth_counts_to_the_roots_height`(이전 코드는
  `depth: 3/16`으로 실패).
- **게이트**: `cell_tree`(tools/validate_cell_tree.py — SPEC-VALIDATION),
  렌더 코어 유닛(cells.rs 9종), renderd 파스/워커 유닛, vfs hiersum 유닛,
  워커 와이어 계약(validate_rust_renderer).

## 9. 픽/스냅/클립

- pick: 화면 워킹셋에서 점을 포함하는 도형을 면적이 작은 순으로(같으면 레이어·데이터타입 번호,
  그다음 만난 순서) 고르고 nth로 순환한다. 프레임/라벨 셀(FRAMES/LABELS 프리픽스)은 픽 제외.
  - 후보(2026-11-12, 0.12.455; 사용자: "pick한 위치에 여러 도형이 겹쳐있을 경우 가장 작은 도형부터
    순환하도록 할 수 있을까? 그리고 도형이 어느 방향이라도 화면보다 클 경우에는 pick 대상에서
    제외해줘."): 점을 포함하는 도형을 **전부** 재고 그 가운데 가장 작은 64개(`PICK_CANDIDATE_CAP`)가
    후보다. 0.12.454까지는 순회가 레이어 순서로 만난 처음 64개에서 멈추고 그 안에서만 정렬했다. 한 점
    위에 64개보다 많은 도형이 겹친 자리(수백 레이어의 넓은 판)에서는 클릭한 작은 도형이 후보에 들지
    못했고 "가장 작은 것"이 앞쪽 레이어의 판이었다.
  - 보이는 도형만 후보다(2026-11-12, 0.12.456; 사용자: "화면 밖에 걸쳐있는 도형을 제외해 달라는
    얘기였는데 생각해보니 이러면 명백하게 보이는 도형도 선택이 안되는 문제가 있을 것 같음. 그리고
    가능하면 현재 눈에 보이는 도형을 선택하도록 해야 함. 레이어가 켜져 있더라도 가려진 레이어의
    경우에는 선택되지 않는게 좋을 것 같음."). 점을 포함하는 도형 가운데, 화면의 프레임에서 그 점
    주변에 **자기 레이어의 픽셀이 실제로 남아 있는** 도형만 고른다. 레이어가 켜져 있어도 그 자리에서
    위 레이어에 가려 한 픽셀도 보이지 않으면 후보가 아니다.
    - 판정은 그림 그대로다. 프레임은 위 레이어부터 칠하고 픽셀마다 한 번만 쓰므로(§래스터의
      write-once), 어떤 레이어가 쓴 픽셀은 위 레이어들이 남겨 둔 픽셀이다. renderd가 클릭 주변
      16 × 16 px 창(양쪽 8 px, 16 × 16 무늬의 한 주기)을 그 프레임의 요청으로 다시 칠하면서 레이어마다
      쓴 픽셀을 읽는다(render-core `seen_planes_around`). 도형은 자기 레이어가 쓴 픽셀이 자기 bbox
      (외곽선 굵기만큼 넓힌) 안에 하나라도 있으면 보이는 것이다.
    - 그래서 결과는 fill을 따른다. solid 아래의 도형은 그 안쪽에서 고를 수 없다. 기본 speckle은 모든
      레이어가 같은 칸을 칠하므로 위 레이어의 안쪽에서는 아래 레이어의 안쪽이 보이지 않고, 아래 도형은
      외곽선이 지나가는 자리(8 px 안)에서 고를 수 있다. 무늬가 다른 레이어들은 서로의 빈 칸으로 함께
      보이므로 함께 후보다. clear(외곽선만)인 도형은 외곽선 근처에서만 고를 수 있다.
    - 가려진 도형을 고르려면 위 레이어를 끈다. 가려서 뺀 수는 답에 실리고(`hidden`), 후보가 하나도
      없는데 가려진 도형이 있으면 상태 줄이 `no visible object here (N hidden)`으로 말한다(빈 자리의
      `no object here`와 구분). 스냅도 가려진 것에는 붙지 않는다(0.12.457, 아래 snap).
    - 0.12.455의 크기 제한(가로나 세로가 뷰보다 큰 도형 제외, `FLOE_PICK_VIEW_LIMIT`)은 없앴다. 화면을
      가로지르는 배선처럼 분명히 보이는 도형이 빠졌기 때문이다. 크기로는 아무것도 빼지 않는다.
  - 순서와 64개, 보이는지의 적용은 render-core `pick_scene`(`seen`)이고, 보이는 것만 묻는 것은
    컨트롤러가 정한다(app-core `view::query`의 `seen`; 서비스의 답에 `hidden`과 `request.seen`).
    `FLOE_PICK_VISIBLE=off`면 가려진 도형까지 모두 후보다(순서는 그대로 작은 것부터).
  - 그대로인 것: 컷 아래라 그려지지 않은 도형은 pick 대상이 아니다. 같은 자리의 클릭은 다음 후보로
    가고 후보를 다 돌면 처음으로 돌아온다. Ctrl/Shift 클릭은 항상 첫 후보다.
- Rust pick/snap 공통: page bbox 선행 prune, 비가시 Pts까지 포함해 query당
  repetition member 400개 상한.
- snap: 반경 내 vertex 우선, edge 수선(투영) 차선(_SNAP_CAP 400).
  - 보이는 것에만 붙는다(2026-11-12, 0.12.457; 사용자: "스냅도 가려진 도형에는 붙지 않도록 할 수
    있을까?"). pick과 같은 판정이다. renderd가 스냅 반경을 덮는 창(반경 + 3 px)을 그 프레임의 요청으로
    다시 칠해 레이어마다 쓴 픽셀을 읽고(`seen_planes_around`), 붙을 자리에서 그 레이어의 픽셀이
    2.5 px 안에 하나도 없으면 그 자리는 후보가 아니다(`SeenPlanes::shows_at`, `SEEN_NEAR_PX`).
    - 판정은 도형이 아니라 **붙을 점** 단위다. 꼭짓점은 그 꼭짓점에서, 변은 커서에서 내린 수선의
      발에서 본다. 도형의 일부가 보여도 가려진 쪽의 꼭짓점과 변에는 붙지 않는다.
    - 반경 안에서 보이는 것 가운데 가장 가까운 것을 잡는다(가장 가까운 것이 가려졌다고 스냅이
      비지 않는다). vertex 우선은 그대로다.
    - solid 아래의 꼭짓점과 변에는 붙지 않는다. 위가 기본 speckle이면 아래 도형의 외곽선이 한 칸
      건너 보이므로 그대로 붙는다. 위 레이어를 끄면 그 아래에 다시 붙는다.
    - 점프한 에러 도형(오버레이)의 스냅은 따로 판정하므로 그대로다. 레이아웃 쪽에 보이는 것이 없으면
      에러 도형의 답이 그대로 나온다.
    - `FLOE_SNAP_VISIBLE=off`면 가려진 것까지 모두 대상이다(0.12.456까지의 동작).
- 에러 도형의 스냅(2026-11-12, 0.12.454; 사용자: "룰러의 스냅이 에러 표시 도형에는 적용되지 않는데
  적용되게 할 수 있을까?"): 점프한 에러의 도형은 뷰어가 프레임 위에 덧그리는 것이라 snap이 답하는
  장면에 없었다. 그래서 에러 에지의 끝이 레이아웃 변의 중간이거나 그 자리에 레이아웃 도형이 없으면
  스냅이 그 끝을 지나쳤다. 이제 뷰어가 snap 질의에 그 도형의 선분을 함께 보내고(`view_query`의
  `overlay`), 공유 Rust(app-core `view::overlay`)가 레이아웃의 답과 하나로 합친다.
  - 규칙은 장면의 것과 같다. 반경 안의 끝점이 먼저이고(vertex), 없으면 선분 위의 가장 가까운
    점이다(edge). 레이아웃의 답과는 vertex가 edge보다 먼저이고, 같은 종류면 가까운 쪽이며, 거리가
    같으면 레이아웃의 답이다. 그래서 에러 도형이 레이아웃 도형 위에 그대로 놓인 곳의 답은 전과 같다.
  - 끝점은 에러의 좌표 그대로다(결과 파일이 레이아웃보다 촘촘하면 dbu 격자 밖일 수 있다). 선분 위의
    점은 장면의 edge 스냅처럼 선분을 따라 정수 dbu이고, 축에 평행한 선분에서는 선분 자신의 좌표를
    써서 정확히 그 위에 놓인다.
  - 대상은 **도형으로 그려진** 점프 에러뿐이다: 폴리곤의 외곽선, 에지 레코드의 에지. 마커 원으로
    접힌 에러(화면 폭이 `DRC_MARK_PX` 미만), 다른 에러의 마커 원, Tab으로 오버레이를 끈 상태는
    대상이 아니다.
  - 뷰어는 커서 주변(반경 + 2 dbu)에 걸친 선분만 보낸다(`_snap_shapes`, 한 번에 256개까지). 레이아웃
    스냅을 묻지 않는 넓은 배율(`tiles_spanned > 4`)에서도 도형이 그려져 있으면 도형만 묻는다
    (`layout: false`). 그 배율에서 물을 것이 없으면 남아 있던 스냅 표시를 지운다.
  - 대각선 간격의 가로·세로 성분은 이것으로 잰다: 룰러 모드에서 한 끝을 찍고 Shift를 누른 채 다른
    끝에 커서를 두면 상태 줄이 `measure D um (dx …, dy …)`를 보여 준다. Shift 없이는 긴 쪽 성분의
    룰러가 된다.
- clip: probe(exact) 경로로 영역 저장. 계측은 절대 LOD/컷/격자를
  거치지 않는다(프로브 강제 0).

## 10. 상수 모음 (gui.py 상단)

MIN_SPP 0.01 · FIT_ZOOM_OUT 16 · WHEEL_ZOOM_STEP 0.96 ·
KEY_PAN_FRACTION 0.5 / _FINE 0.1 · CAL_ZOOM_IN 0.5 · MINIMAP_PX 180 · MINIMAP_PAD 6 ·
BORDER_PX 12(눈금 띠; 기준 길이의 하한 60 px은 app-core `view::border::MIN_PX`) ·
DETAIL_PX (5,3,1)/기본 medium · COV_MAX_TEXEL_PX 160 ·
DEBOUNCE_MS(gui.py) · 스트림 상수(_MAX_STREAM_ROUNDS 8, 예산 클램프 2048..32768KB)는 service.py.
