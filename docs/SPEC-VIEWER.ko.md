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
    아래층).
  - **하단 줄**에는 요약 줄만 나온다. 항목은 ` · `로 잇는다.
    - 시간: `N ms = L load + D draw`. `+ T text`·`+ O other`·`+ W wait`는 전체 줄과 같은 문턱에서만
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
    - 컷: `cut<X um`과 예산 맞춤(`… to fit budget`, `STILL OVER`). `(larger side)`는 빠진다.
    - 그림이 모자란 것: `N pages over budget (not drawn)`, `labels partial`, `evict N`,
      `summary N layers (not pickable)`.
    - 덱: `deck N passes`.
  - 로드 직후 첫 프레임은 `loaded in X s · `만 앞에 붙는다. 내역은 로그에 있다.
  - depth는 위 줄(`depth: d/max · detail: …`)에 있다.
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
  open .db…·rules… 버튼은 2026-08-22 메뉴 바로 이전(패널 정보줄만 잔류).
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
- **메뉴 바**(2026-08-22, `_build_menubar`): File(**load layout**·
  clip·copy·quit) /
  View(fit·줌·goto·detail·depth·토글 체크 5종·오버레이 순환) /
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
  LOD 토글(메뉴 항목·`l` 키·깊이 라벨의 `lod:on/off`·`floe2 view --lod`)은 제거됐다(2026-09-22
  사용자 결정: LOD는 쓰지 않는다 — 토글은 Rust 경로의 renderd에 닿지도 않았다). 실행 중인 창으로
  넘어오는 `lod=` 필드는 받아서 무시하고, renderd의 LOD 교체는 기본 끔이다(SPEC-PLANNER §3).

## 8. 색/패턴 적용 경로

recolor: 행 스와치 재생성(set_color) + meta 갱신 + 개인 layerprops
스냅샷(_save_props_state) + `recolor` 잡(renderer colors 갱신+refresh) +
epoch↑ + 즉시 재렌더. 안정판 floe의 KLayout worker만 coverage 틴트를 함께
갱신한다. repattern 동일 구조(_push_fills:
지정 전체를 resolved rows로 전송). 폴딩된 그룹 부모 선택 시 멤버 전체
적용. 시작 시 service `_apply_personal_fills`가 layerprops로 복원.

## 8b. DRC 브라우저 (대용량 규약)

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
  필터 컨트롤 행 = **FlowBox**(좁으면 여러 줄로 래핑, 1~4열).
  lpaned pack1 shrink=True + 검색 박스 width_chars 8이 pane 폭
  하한을 낮게 유지(실질 하한 = 미니맵 196px).
- **룰|그리드 분할 = 비율 유지**(2026-08-18): hsplit 위치는 고정
  px가 아니라 **pane 폭의 분수**(초기 0.45) — 드래그가 분수를
  재고정하고 size-allocate가 재적용(guard 플래그로 루프 차단).
  구 고정 220px는 시작 pane(196px)을 통째로 삼켜 그리드가 0폭으로
  숨고 핸들이 가장자리에 붙던 원인.
- **룰 행 카운트 = 에러수/waived**(2026-08-18, "30/20" 형태):
  둘 다 O(1)([wcount]) — waive 필터의 룰 숨김 판정은 기존
  `_drc_wf_count` 그대로. waive/unwaive는 all 필터에서도 해당
  행 텍스트를 제자리 갱신.
- **SVRF 룰 메타데이터**(2026-08-15, SPEC-FORMATS `<deck>.rules.json`
  참조): db 로드 시 자동 탐색(`_drc_rules_auto` — Rule File Pathname
  베이스네임의 **db 옆** 사이드카 우선, 기록 경로·`<db>.rules.json`
  차선) + DRC 메뉴 load SVRF rules… 수동 로드. 정보줄에 `svrf 매칭/전체`.
  상세 pane 하단(`_drc_meta_lines`): 덱 원문 제약(constraint:) ·
  **measured** = 이 에러 자체의 치수 vs 한계값과 Δ/%(waive 판단
  보조; `_drc_measured` — CD 룰러와 동일 판정: rect=min(w,h)·마주
  보는 엣지쌍=갭·단일 엣지=길이·area=신발끈, 복잡 도형은 생략;
  제약이 여럿이면 **상한(<,<=,==) 우선** — `> 0 < v` 체인의 하한은
  무의미한 +Δ라 표시하지 않음, 2026-08-17) ·
  layers(직접 피연산자) · gds(원천 레이어 폐쇄) · derivation 체인
  (`derived` 맵을 `svrf.rhs_operands`로 워크, 6줄 캡). 메타 없는
  룰/사이드카 부재 시 상세는 기존 그대로(추가 줄 0).
- **룰 유형 필터**(2026-08-17, rules.json 필요): nav 행 콤보 —
  "all types" + 사이드카 constraint **metric별 룰 수**(width/space/
  enclosure/area/density/… DRC_METRICS 정순, 그 외 알파벳순;
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
  `_drc_open`). **db 로드 직후 = 무선택 상태**(2026-08-18 사용자
  결정 — 자동 열기 금지): 모델 재부착 중 GTK가 행 0을 유령
  선택하면 busy 가드가 이벤트를 삼켜 "선택돼 보이는데 그리드는 빈"
  상태가 되므로, load_drc가 unselect_all로 유령 선택을 걷어낸다.
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
- **룰 에러 상시 표시**(2026-08-14→15, 2026-08-18 마커 단일화):
  룰이 열려 있으면 **현재 그리드 페이지의 에러들**(_drc_page_marks
  — 그리드 채움 시 지오메트리 프리빌드, 페이지당 ~7ms)이 캔버스에
  **상태색**(not waived=red, waived=green)으로 그려짐 — 페이지
  넘김·필터·waive 변경 즉시 반영. **실도형은 점프된 에러
  (더블클릭/n·p — drc_mark 대상) 하나만**: 그 에러도 화면 스팬 <
  DRC_MARK_PX면 마커로 붕괴, **나머지 에러는 줌과 무관하게 항상
  DRC_MARK_PX 마커**(2026-08-18 사용자 요청 — 광역 뷰 도형 뒤엉킴
  제거; 포커스 셀은 9×9). `_drc_stamp_errs` 공용 페인터(세그먼트
  예산 20k). 선택(gold)은 그 위에 같은 페인터로 덮임(동일 규칙 —
  gold도 마커). in-view 필터의 뷰 추적 호출은 유지.
  **마커 hover/pick**(2026-08-18): 페인터가 프레임마다 화면좌표
  히트 리스트(_drc_hits)를 굽고 — ① hover 툴팁 = "룰명
  #로컬(전역) · waived"(변경 시에만 set_tooltip_text), ② 정지
  좌클릭이 마커 6px 안이면 `_drc_pick` = **번호 단일클릭과 동일**
  (셀 마크+포커스+디테일, 뷰 불변), ③ **더블클릭 = 번호 더블클릭과
  동일**(goto_cell + _drc_jump(isolate=True) — 프레이밍·CD 룰러·
  레이어 격리; _on_press에서 drag 가드 **앞**에 처리 — 짝 단일
  press가 재장전한 pan drag를 해제해야 이벤트가 살았음, 2026-08-18
  rev 2); Ctrl/Shift 클릭은 디자인 선택 제스처 유지, 마커 미히트
  시 기존 디자인 pick 폴스루. **룰 전환 시 점프 잔재 정리**
  (2026-08-18): 다른 룰 선택은 이전 룰의 drc_mark·_drc_pos·자동
  CD 룰러를 제거(수동 룰러 유지) — 새 룰의 마커 위에 이전 점프
  도형이 남지 않음. **Esc 격리 복원 = 점프 마크 동시 해제**
  (2026-08-18): _drc_lyr_saved 복원 스테이지가 drc_mark·자동 CD
  룰러도 함께 제거 — _drc_pos·포커스는 유지(클릭-형 n/p가 같은
  위치에서 이어짐).
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
  경우에는 그 대각선 룰러 뒤에 같은 두 점을 잇는 **수평·수직 성분
  룰러 2개**도 붙인다. 축정렬 사각형=**폭·높이 2개**(둘 다 유의미,
  중앙 관통·중앙 교차). 복잡한 폴리곤/엣지셋은 룰러 생략(사용자 규정 2026-08-13).
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
  `zoom`, `root`, `top`; 좁으면 여러 줄로 감김) · 인덱스가 없을 때만
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
- **선택**(트리·결과 공통, 단클릭): `cell_bbox`로 정보 줄(`NAME: N
  instances · W × H um`, 근사면 `(the blocks holding it)`), highlight가
  켜져 있으면 `cell_insts`로 **현재 뷰 안 인스턴스 박스**를 받아 캔버스에
  **CELL_HL(#40E0FF) 2 px 외곽**으로 그린다(화면에서 7 px보다 작은
  인스턴스는 7 px 마커 사각). 상한 4,096박스·탐색 예산 200만 방문 —
  넘치면 `more`, 상태줄 `(more - zoom in)`. 프레임이 착지할 때마다
  뷰 키(src, ci, 뷰 박스 반올림)가 바뀌었으면 다시 묻는다
  (`_cell_hl_follow`; 데몬은 대기 중인 더 새 insts 질의가 있으면 옛것을
  `superseded`로 즉시 답한다). Tab 오버레이 숨김 상태에서는 그리지 않음.
- **줌**(더블클릭·Enter·`zoom`·메뉴): `cell_bbox`의 범위를 DRC 점프와
  같은 규칙으로 프레이밍(양 축 0.8, `CELL_VIEW_FRACTION`). 범위 =
  탑이 **직접 배치한 셀은 그 인스턴스 박스들의 정확한 합집합**, 더 깊은
  셀은 **그 셀을 품은 탑 직계 블록들의 범위**(요약이 엣지별 합집합만
  갖고 레코드별 변환은 갖지 않으므로; `approx=1`, 상태줄
  `(zoomed to the blocks holding it)`) — 이후 하이라이트가 정확한
  인스턴스를 보여 준다. 탑 아래에 없는 셀(orphan)·도형 없는 셀은
  줌하지 않고 상태줄로 이유를 말한다.
- **키/Esc**: `t` = cells 페이지 올리고 검색 박스 포커스(pane 폭 하한
  260 px); Esc 체인에서 선택 해제 **다음** 단계가 셀 하이라이트 해제
  (`_cell_hl_clear`: 하이라이트와 트리 선택을 함께 지움; highlight
  체크는 유지). 검색 박스에 포커스가 있으면 캔버스 키는 오지 않는다
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
  트리의 "선택한 셀이 표시되는 탑이 된다"): Cell 메뉴 · 패널 `root`
  버튼이 **선택한 셀을 뷰 루트**로 삼고(Ctrl+T 키는 사용자 요청으로
  2026-09-30 삭제, 0.12.254 — 눌러도 아무 일 없음, 평문 `t`의 트리 포커스로
  흘러가지도 않음; 계약 `test_ctrl_t_is_no_shortcut`), `Ctrl+Shift+T` ·
  메뉴 · `top` 버튼(루트일 때만 활성)이 탑 셀로 돌아온다. 뷰 루트가
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

- pick: 화면 워킹셋에서 점 포함 도형 최소면적 순(_PICK_CAP 64),
  nth 순환. 프레임/라벨 셀(FRAMES/LABELS 프리픽스)은 픽 제외.
- Rust pick/snap 공통: page bbox 선행 prune, 비가시 Pts까지 포함해 query당
  repetition member 400개 상한.
- snap: 반경 내 vertex 우선, edge 수선(투영) 차선(_SNAP_CAP 400).
- clip: probe(exact) 경로로 영역 저장. 계측은 절대 LOD/컷/격자를
  거치지 않는다(프로브 강제 0).

## 10. 상수 모음 (gui.py 상단)

MIN_SPP 0.01 · FIT_ZOOM_OUT 16 · WHEEL_ZOOM_STEP 0.96 ·
KEY_PAN_FRACTION 0.5 / _FINE 0.1 · CAL_ZOOM_IN 0.5 · MINIMAP_PX 180 · MINIMAP_PAD 6 ·
DETAIL_PX (5,3,1)/기본 medium · COV_MAX_TEXEL_PX 160 ·
DEBOUNCE_MS(gui.py) · 스트림 상수(_MAX_STREAM_ROUNDS 8, 예산 클램프 2048..32768KB)는 service.py.
