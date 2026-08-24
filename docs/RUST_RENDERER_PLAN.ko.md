# floe Rust renderer 구현 계획

상태: M1 완료, M2 정확도·결정성 vertical slice 완료, M3 paint/frame vertical slice 완료,
M4 text/label vertical slice 완료, M5 daemon/session vertical slice 완료,
M5 density coverage 완료, M7 pick/snap 완료, M8 exact clip vertical slice 완료
목표: 기존 `floe`의 Rust 인덱서·VFS 플래너는 유지하고, KLayout의 런타임
렌더 책임을 결정적 멀티코어 CPU 렌더러로 대체한다.

### 현재 구현 상태 (2026-08-24)

통합 기준선은 현재 부모 `floe` main, `floe-index 0.12.0`, OVM v7이다.

- 완료: Cargo workspace와 `render-core`, `render-cli`, `renderd` scaffold
- 완료: 폐쇄망 vendor 설정과 동일 Cargo workspace 내 직접 path dependency
- 완료: cache pair validation, `ViewReq -> HierPlan`, plan 통계
- 완료: OVP file-order batch read, page OASIS decode, phase telemetry
- 완료: OVP read는 file-order 일괄 I/O로 유지하고 독립 OASIS page parse를
  `jobs` worker로 병렬화. 결과·오류·LRU insert는 요청 page 순서로 결정
- 완료: CLI 실캐시 smoke test와 부모 `floe-index plan` 결과 대조
- 완료: daemon `open/style/render/cancel/info/quit` line protocol, 영속 cache/LRU와
  style epoch, 요청별 jobs/page budget, DBU viewport, exact-mode 충돌 검증
- 완료: 부모 Python의 GUI/DRC snapshot/headless probe가 공통
  `make_render_worker` factory를 사용. 기본은 in-tree Rust adapter이고
  `FLOE_RENDERER=klayout`일 때만 rollback worker를 선택.
  `FLOE_RUST_WORKER=MODULE:TYPE`은 개발용 override로만 유지
- 완료: in-tree `RustRenderWorker` vertical slice. 부모 job/result queue 계약을
  persistent `floe-renderd` protocol로 변환하고 progressive frame,
  generation 취소, recolor/repattern/mono, layerprops fill/width, telemetry,
  프로세스/임시 파일 lifecycle을 처리
- 완료: adapter 전용 `round_paths=1`. 각 partial PNG를 고유 경로로 게시해
  다음 round의 atomic replace가 Python의 이전 응답 read와 경합하지 않으며,
  읽은 partial은 즉시 제거하고 final/전체 임시 디렉터리는 종료 시 정리
- 완료: 부모 `Vfs::read_page_batch` 전환과 임시 OVP reader 제거
- 완료: 결정적 budget LRU와 불변 partial/complete `FrameScene`
- 완료: checked orthogonal transform과 One/Grid/Pts 가시 범위 순회
- 완료: 단일 스레드 rectangle occupancy와 결정적 RGBA PNG vertical slice
- 완료: 부분 DBU viewport 위상을 보존하는 device Q32.32 even-odd polygon
  active-edge fill과 checked 산술 gate
- 완료: 부모 `floe-tiler`와 동일한 Manhattan PATH square-miter/extension fill,
  KLayout `Path.polygon()`과 동일한 복수 segment 비맨해튼 PATH miter 및 급각 외곽
  clip. 퇴화 spine/U-turn은 조용히 누락하지 않고 page ID를 포함한 명시적 render
  error로 중단
- 완료: 병렬·bounded `path-inventory`로 운영 cache의 PATH record/member, 꼭짓점,
  Manhattan 여부, U-turn/퇴화, half-width, start/end extension 분포를 감사
- 완료: 부모 importer의 OASIS CIRCLE(record 27) → 결정적 내접 64각형 `PolyRec`
  정규화 소비(렌더러 primitive 및 OVM/OVP 포맷 변경 없음)
- 완료(기준선): `--jobs` scoped worker와 수평 band 단독 소유로 worker 수 독립
  RGBA 합성을 먼저 고정
- 완료: 2D square tile 독립 scratch, atomic work index의 bounded worker pool,
  `(row,col)` 고정 위치 합성, CLI/daemon `tile_px`(기본 128, 1..4096)
- 완료: KLayout solid geometry의 1 device-pixel frame, 전역 Bresenham 위상,
  1~8px outline의 KLayout 짝수 폭 device 편향, tile/viewport 경계의
  stroke 폭 기반 tile/viewport cull footprint
- 완료: layer-index paint plane, visibility/color/불투명 적층, 전 레이어 공통 위상
  2x2 speckle, KLayout의 `(device row + framebuffer height - 1) mod 16`
  위상을 따르는 16x16 pattern, mono 변환
- 완료: PATH의 fill/외곽 outline과 별도로 원래 spine에 style 중심선을 렌더.
  begin/end extension은 hull에만 적용하며 중심선 끝점은 확장하지 않음
- 완료: planner wash와 hierarchy frame 4밴드(gray fill/dotted/outline < design <
  white outline), frame repetition/transform 직접 순회
- 완료: 기존 `Vfs::plan_labels_with`를 generation당 한 번 호출해 선택된 design
  text/block label을 모든 progressive round와 tile worker가 불변 `Arc`로 공유.
  TSV 생성, KLayout layer 등록, round별 재계획 없음
- 완료: SIL OFL 1.1 Noto Sans Mono를 binary에 번들하고 pure-Rust `fontdue`와
  의존성을 offline vendor. OS font lookup 없이 6..96 정수 px 크기, center
  alignment, 0/90/180/270도 회전, 결정적 정수 alpha 합성을 구현. ASCII
  glyph는 bounded process cache로 progressive round/generation 사이 재사용
- 완료: gray block text < design layer geometry+text < white block text paint stack,
  layer visibility/mono 연동, label glyph 262,144개 명시적 요청 상한
- 완료: 최신 게시 `FrameScene`을 공유하는 현재 화면 기준 pick/snap. 제품 경로에서
  KLayout layout/delta 등록 없이 page/hierarchy/transform/repetition/path/wash를 직접
  순회하고 frame/live label은 제외. snap 400 shape, pick 64 candidate 상한과 vertex
  우선, edge Python ties-even 반올림, boundary-inclusive inside, integer area 및
  `(area, layer, datatype)`/`nth` 순환 계약을 보존
- 완료: query는 render worker queue 밖의 stdin 제어 경로에서 게시 scene `Arc`를
  clone하므로 이후 refinement round의 page decode/raster와 병행 가능. 새 round PNG가
  원자 게시될 때만 query scene도 교체되어 미게시 partial을 노출하지 않음
- 완료: GUI `v`의 OVC density coverage를 Rust progressive PNG에 직접 post-composite.
  KLayout 없이 기존 NumPy/Pillow의 neighborhood-aware blank mask, live palette/visibility,
  `cut_px > 0`, finest texel 160px handoff 계약을 그대로 사용. binary geometry의 8-bit
  edge antialias coverage와는 별도 기능
- 완료: GUI `clip`을 cut=0/full-depth exact `HierPlan`으로 실행하고 daemon jobs로
  page를 병렬 decode/LRU 재사용한 뒤 계층·repetition·PATH outline을 평탄화하여
  단일 `FLOE_CLIP` OASIS cell로 기록. rectangle type은 유지하고 PATH는 KLayout
  clip처럼 polygon으로 기록
- 완료: clip 교점을 rational로 끝까지 유지한 뒤 KLayout의 nearest DBU
  (정확한 half tie는 +무한대 방향)로 한 번만 반올림. concave clip의 역방향
  경계 bridge를 분할·상쇄해 분리된 component를 각각 단순 polygon으로 방출
- 완료: daemon은 공백 없는 전용 임시 경로만 받고, Python adapter가 사용자 선택
  경로와 같은 디렉터리에 write/fsync 후 atomic replace. 공백 포함 출력 경로 지원
- 완료: CLI 반복 `--style L/D,#RRGGBB,FILL[,WIDTH]`, `--frames`, `--mono` 경로와
  OVM layer key→index 공개 매핑
- 완료: strict monotonic generation cancellation. plan/decode/scene/raster/PNG 경계,
  paint plane과 repetition 1024-member batch에서 취소를 확인하고 stale frame은
  응답·파일 모두 미게시
- 완료: PNG same-directory 임시 파일 write/sync/rename과 cancellation frontier의
  직렬 commit; 실패·취소 시 임시 파일 정리
- 완료: `page_prio` 기반 progressive refinement. daemon 기본 128 page씩 decode해
  같은 generation의 `round/final/partial` frame을 원자 게시하고 새 generation이
  남은 round를 취소. `decode_pages`는 전체 상한, `round_pages`는 round 상한
- 완료: 부모 정확도 계약 P-a(골든 edge Chebyshev 1px band), P-b(4-connected
  component 보존), P-c(area drift 제한) 반영
- 검증: 2D tile 전환 후 PX1~PX5 13개 실제 Rust 후보를 재생성해 모두
  P-a/P-b/P-c 통과, 각 후보의 1/4/8 worker PNG byte 동일. PX5의 16개 PATH도
  보류 없이 렌더
- 검증: 독립 `tools/validate_klayout_oracle.py`가 OASIS를 KLayout과 Rust cache
  경로로 각각 렌더해 PX 13개와 style 14개를 대조. half-pixel view, speckle,
  custom 16x16 pattern, 1/2/4px outline, PATH 중심선, paint order, visibility,
  mono를 포함하며 jobs 1/8 모두 통과
- 검증: valmini 1200×800도 1/4/8 worker PNG SHA-256 동일
- 검증: 실제 valmini 2-layer speckle + 4px outline styled 화면과 depth-0 frame
  화면이 1/4 worker에서 각각 PNG SHA-256 동일
- 검증: 실제 valmini daemon에서 진행 중 generation 40을 generation 41로 취소,
  gen40 PNG/임시 파일 0개, gen41 frame만 게시. 동일 styled 요청 gen21/31/41의
  PNG SHA-256 일치, 재전송한 gen40은 stale drop
- 검증: 부모 `make_render_worker`가 실제 in-tree adapter를 선택한 headless
  `floe probe`에서 valmini 2개 화면 PNG magic/queue/process lifecycle 통과
  (4 workers, 각각 약 54ms/39ms). 4-page progressive + recolor/repattern/mono
  실제 통합 1개와 Python 순수 계약 테스트 13개 통과
- 검증: 실제 adapter/daemon/부모 Cache 조합에서 100세대 pan/zoom burst를
  valmini(4-page round)와 sample9(506 pages, 64-page round)에 각각 실행. 이전
  99세대 frame publish 0, 최신 세대 settled frame 수신, 작업 상태와 partial 파일
  잔류 0
- 검증: exact clip Rust unit 4개가 KLayout half-up 대각 교점, boundary-touch
  drop, 분리 concave component, OASIS 왕복을 고정. 실제 valmini 선택 3-layer
  export는 jobs 1/8 OASIS byte 동일이고 부모 `_svc_clip` 결과와 Region XOR 0;
  공백 포함 사용자 경로 atomic publish와 `floe clip` custom UTF-8 cell-name
  실제 CLI smoke 통과
- 검증: `valmini`, `thintest`, `stress30`, `sample9`, `testchip_1g5`의 총 30,456
  page를 inventory. PATH는 valmini의 record 6개/member 8개뿐이고 모두 현재
  renderer 범위이며 U-turn/퇴화/음수 extension/zero half-width는 0
- 검증: 이 Mac의 native Python/GTK floe를 XQuartz 없이 Rust backend로 직접 실행.
  sample9 밀집 영역 full-depth mid-zoom에서 cold 25ms(1 page, load 4ms + draw
  13ms), 같은 GUI/daemon/LRU로 전달한 인접 pan 16ms(+0 page), 2배 zoom 11ms,
  frames 전환 12ms까지 정상
- 검증: 번들 글꼴 로드/제어문자 정규화/크기 상한, anchor center, 90도 회전,
  layer visibility, gray/white block tone, 1/8 worker와 100/13px tile의 RGBA byte
  동일성 Rust 회귀 통과. 실제 valmini 부모 adapter labels-on은 136개를 0.065ms에
  계획하고 28,377 antialiased pixel을 게시, `labels_truncated=0`
- 검증: 실제 styled valmini에서 tile 64/128/256 × worker 1/4/8의 9개 PNG가
  기존 band renderer PNG와 SHA-256 일치. 합성 primitive seam 회귀는 pattern,
  4px dotted stroke, rectangle/polygon/path를 3px 타일 경계에 교차시켜 byte 동일
- 실측: 128px tile, styled valmini 1200×800 release 5회 중앙값 `raster_us`가
  94.733/27.511/23.083ms로 4-worker 3.44배, 8-worker 4.10배. 잠정 2.5배/4배
  gate 모두 통과. 4-worker tile 크기 중앙 경향도 128px가 64/256px보다 우수
- 실측: sample9 full-depth mid-zoom 506 page(1.84M record) release 디코드 중앙값이
  1/4/8 worker에서 131.8/41.9/31.7ms(4 worker 3.15배, 8 worker 4.16배).
  `round_pages=64`, 600×600 daemon은 약 10ms 첫 partial frame을 게시하고 8번째
  final PNG가 single-shot PNG와 SHA-256 동일
- 실측: 별도 cold-daemon sample9 밀집 mid-zoom 303-page gate에서 600×600,
  8-worker의 `round_pages=32/64/128/256` 첫/final latency는 각각
  17/290, 17/159, 21/122, 31/99ms. 기본 128은 64 대비 첫 화면 +4ms 대신 final
  -37ms여서 유지. 같은 영역 100세대 1000×700 pan burst는 stale frame 0,
  최신 세대 3 frame, pending job 0
- 다음: 실제 GTK GUI A/B 장기 운용

부모의 `data/m1/valmini*.floe`는 현행 OVM v7로 재생성됐으며 M1 smoke/golden
fixture로 사용한다. `v_j1.floe`, `v_j8.floe`는 과거 M1 비교 산출물이라 v2 상태다.

### 부모 `floe` 연동 상태

정확도 정책과 PX1~PX5 fixture는 `8a10742`에서 확정됐고, `030faf2`에서 OASIS
CIRCLE(record 27)이 source parse 시 결정적 내접 64각형 `PolyRec`으로
정규화됐다. 모달·repetition·xy-relative를 보존하고, 작은 반지름의 연속 중복
꼭짓점을 접으며, `r=0`은 지오메트리를 방출하지 않는다. 따라서 기존 polygon의
rep-split·fragment·LOD·encode 경로와 renderer polygon 경로를 그대로 사용하고
OVM/OVP 포맷은 바뀌지 않는다.

이 fallback은 호환성과 PX5 실행을 위한 것이며 Calibre 원형 픽셀 동치 gate로
해석하지 않는다. 현 단계에서 렌더러 착수를 위해 필요한 추가 부모 변경은 없다.

## 1. 목표와 성공 조건

`floe` Rust renderer의 1차 제품 목표는 다음 한 문장으로 고정한다.

> `floe-vfs`의 `HierPlan`과 선택된 OVP 페이지를 직접 소비하여, 현재
> KLayout 렌더 경로와 동등한 도형 정확도와 표시 계약을 제공하는 취소 가능
> 멀티코어 CPU 렌더러를 구현한다.

성공 조건은 다음과 같다.

1. `cut=0`, full depth, LOD/wash/coverage/abstract off에서 소스 geometry가
   누락 없이 전달되고, binary raster는 부모의 P-a/P-b/P-c 정확도 정책을
   모든 승인 fixture와 대표 실칩 view에서 통과한다.
2. 동일 요청은 worker 수, 실행 순서와 무관하게 동일한 RGBA/PNG를 만든다.
3. rectangle, polygon, path, orthogonal transform, One/Grid/Pts repetition을
   저장 형태를 유지한 채 렌더한다. 전체 repetition의 선행 전개는 금지한다.
4. 현재 speckle 위상, 레이어 paint 순서, frame 4밴드, visibility, color,
   fill pattern, line width 계약을 보존한다.
5. page read/decode와 PNG encode를 제외한 `raster_us` 기준 render-bound 대표
   화면에서 4 worker가 1 worker 대비 최소 2.5배, 8 worker가 최소 4배
   빠르다는 것을 목표로 한다. 이 수치는 M0 실측 후 자산별 최종 gate로
   확정한다.
6. Python GUI의 기존 `RenderWorker` 호출부를 유지한 채 opt-in A/B가 가능하고,
   최종 cutover 후 `view`, `render`, `probe`는 `klayout` 없이 동작한다.

## 2. 범위

### 2.1 1차 필수 범위

- `design.ovm`, `design.ovp`, `design.ovt` open/validation
- 기존 `ViewReq -> HierPlan` 호출
- OVP page decode LRU
- `HierPlan`의 `WsCell`, `WsInst`, page, frame, wash 직접 순회
- rectangle/polygon/path software rasterization
- 외부 OASIS CIRCLE의 결정적 64각형 compatibility normalization
- translate, 90도 단위 rotate, mirror transform
- One/Grid/Pts repetition의 tile-local 가시 범위 계산
- 레이어 visibility/color/paint order
- 2x2 공통 위상 speckle와 16x16 fill pattern
- frame white/gray/fill/dotted 스타일
- 1..8px outline width, mono mode
- image-tile 병렬 렌더, generation 취소, progressive frame
- RGBA 출력과 결정적 PNG encoding
- Python `RenderWorker` 호환 adapter
- 현재 화면 `FrameScene` 기반 pick/snap
- GUI/CLI clip의 Rust exact OASIS 출력
- OVC density coverage의 KLayout-free PNG post-composite
- 성능/메모리/취소 telemetry

### 2.2 제외 범위

- 인덱서, OVM 포맷, VFS planner 재설계
- 범용 mutable layout database와 KLayout API 호환층
- GDS, arbitrary-angle/magnified placement, trapezoid/XGEOMETRY
- 퇴화 spine 및 정확한 join 계약이 확정되지 않은 U-turn PATH
- 범용 Boolean/Region 엔진, DRC 엔진, 편집 기능
- KLayout 고유 abstract mode 재현
- GTK GUI 재작성
- GPU/OpenGL/Vulkan/Metal
- 레거시 Python `.tiles` 인덱서 이식

미지원 OASIS record는 근사하지 않고 명시적으로 실패한다. 운영 자산 inventory에
등장할 때만 별도 범위 변경으로 추가한다.

## 3. 기존 코드와의 경계

재사용 기준은 `rust`의 현재 운영 코드다.

- `floe-oasis`: page OASIS decode, geometry record, `Rep`
- `floe-tiler`: `Xf`, bbox/path helper
- `floe-ovm`: mmap metadata, page/layer/cell accessor
- `floe-vfs`: `ViewReq`, `HierPlan`, `WsCell`, `WsInst`, `grid_ranges`

renderer crate는 `rust/` workspace에 편입되어 위 공용 crate를 직접 path dependency로
사용한다. parser/planner를 복사한 별도 구현은 두지 않으며 Cargo.lock, vendor 정책,
Linux/portable build를 인덱서와 함께 관리한다.

기존 코드에 필요한 최소 공개 API 변경은 다음으로 제한한다.

```rust
impl Vfs {
    pub fn read_page_bytes(&self, page: u32) -> Result<Vec<u8>, String>;
    pub fn read_page_batch(&self, pages: &[u32]) -> Result<Vec<(u32, Vec<u8>)>, String>;
}
```

`read_page_payloads`의 기존 정렬/일괄 읽기 경로는 유지하고, renderer가 페이지
하나 또는 정렬된 batch를 읽을 수 있는 공개 wrapper만 추가한다. `DecodedPage`는
renderer가 `floe-oasis::doc::parse_doc`로 생성한다. 따라서 `floe-vfs`가 renderer를
역참조하는 의존성은 만들지 않는다. 인덱서 산출 포맷은 변경하지 않는다.

## 4. 목표 아키텍처

```text
Python GTK GUI
    |
    | render/style/pick/snap/clip request
    v
Python RenderWorker adapter                 초기 호환층
    |
    | stdin/stdout control + PNG file handoff
    v
floe-renderd (Rust)
    +-- Vfs + HierPlan
    +-- RenderSession / generation cancellation
    +-- decoded page LRU
    +-- immutable FrameScene
    +-- CPU tile worker pool
    +-- deterministic RGBA + PNG
```

`floe-renderd`는 planner와 renderer를 같은 프로세스에서 소유한다. 따라서 KLayout을
위해 수행하던 delta OASIS 저작, `Layout.read`, 이름 바인딩, WC cell 삭제/재생성은
제품 렌더 경로에서 사라진다. 기존 `vfsd`와 delta 경로는 A/B oracle 및 rollback용으로
cutover가 끝날 때까지 유지한다.

게시된 각 progressive round의 불변 `FrameScene`은 query snapshot으로도 공유한다.
stdin 제어 경로가 이를 짧게 `Arc` clone한 뒤 lock 없이 pick/snap을 수행하므로,
render worker는 다음 round의 병렬 decode/raster를 계속할 수 있다. 이는 KLayout 단일
인스턴스용 부모 `rust vfsd -> delta OASIS -> Layout.read` 직렬 병목을 query 경로에도
되살리지 않기 위한 명시적 경계다.

### 4.1 프레임 생성 흐름

1. 요청의 부분 DBU viewport를 반올림하지 않고 보존하고, device 교차점을
   Q32.32로 정규화한다.
2. 기존 planner로 `HierPlan`을 만든다.
3. `page_prio` 순으로 미상주 page를 round budget만큼 읽고, file-order I/O 뒤
   독립 OASIS payload를 `jobs` worker로 decode한다.
4. 현재 상주 page 집합과 `HierPlan`으로 immutable `FrameScene`을 만든다.
5. 화면을 128x128 pixel tile로 분할한다.
6. worker가 tile마다 모든 paint plane을 정해진 순서로 렌더한다.
7. tile 결과를 고정 위치에 복사한다. 완료 순서는 결과에 영향을 주지 않는다.
8. generation이 최신일 때만 PNG를 publish한다.
9. deferred page가 있으면 다음 refinement round를 수행한다.

기본 tile 크기는 실측으로 확정한 128x128이며 실제 worker 수는
`min(request.jobs, tile_count)`다. CLI/daemon의 `--tile-px`/`tile_px`와
`--jobs`/`jobs`로 고정할 수 있다. 64/128/256 결과는 byte-identical이고 현재
styled valmini 4-worker 중앙 경향에서는 128px가 가장 빨랐다.

## 5. workspace 구조

```text
floe/
  floe/
    rust_render.py       기존 RenderWorker queue 호환 adapter
  rust/
    Cargo.toml
    .cargo/config.toml
    render-core/
      src/
        lib.rs
        request.rs       viewport/style/exact-mode 계약
        page_cache.rs    decoded-byte budget LRU
        scene.rs         HierPlan -> immutable FrameScene
        traverse.rs      hierarchy/rep/tile-local 순회
        transform.rs     checked DBU/fixed-point 변환
        raster/
          mod.rs
          rect.rs
          polygon.rs
          path.rs
          stroke.rs
          pattern.rs
          text.rs
        tile_pool.rs
        framebuffer.rs
        stats.rs
    renderd/
      src/
        main.rs
        protocol.rs
        session.rs
        png.rs
    render-cli/
      src/main.rs        fixture/benchmark/A-B용 headless CLI
      src/bin/path-inventory.rs
  tools/
    validate_rust_renderer.py
    validate_klayout_oracle.py
  docs/
    RUST_RENDERER.md
    RUST_RENDERER_PLAN.ko.md
  fixtures/
  benches/
  docs/
```

초기 dependency는 std + 기존 vendored `flate2`, `crc32fast`만 허용한다.
새 raster/PNG/font crate 도입은 정확도·offline vendor·라이선스 gate를 통과한
경우에만 허용한다. M1 rectangle/polygon 경로는 외부 raster dependency 없이
구현해 기준을 먼저 확보한다.

## 6. 핵심 타입과 API

```rust
pub struct RenderRequest {
    pub generation: u64,
    pub view: RasterViewBox,      // 부분 DBU를 보존하는 f64 target box
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub cut_px: f64,
    pub visible_layers: LayerMask,
    pub styles: StyleEpoch,
    pub lod: bool,
    pub frames: bool,
    pub labels: bool,
    pub abstract_mode: bool,     // 반드시 false; KLayout 고유 기능이라 거부
    pub exact: bool,
}

pub struct RenderedFrame {
    pub generation: u64,
    pub rgba: Vec<u8>,
    pub stats: RenderStats,
    pub partial: bool,
    pub deferred_pages: u32,
}

pub struct RenderStats {
    pub plan_us: u64,
    pub page_read_us: u64,
    pub page_decode_us: u64,
    pub scene_us: u64,
    pub raster_us: u64,
    pub png_us: u64,
    pub tiles: u32,
    pub workers_used: u16,
    pub primitives_tested: u64,
    pub primitives_drawn: u64,
    pub rep_members_tested: u64,
    pub rep_members_drawn: u64,
    pub decoded_cache_hit: u32,
    pub decoded_cache_miss: u32,
    pub decoded_cache_bytes: u64,
    pub cancelled: bool,
}
```

`exact=true`는 `cut=0`, full depth, LOD/wash/coverage/abstract off를 검증하고
상충하는 요청을 거부한다. 조용히 옵션을 덮어쓰지 않는다.

## 7. scene과 page cache

### 7.1 DecodedPage

OVP page는 기존 `parse_doc`로 decode하고 단일 page cell의 geometry를 다음
불변 구조로 정규화한다.

```rust
pub struct DecodedPage {
    pub page_id: u32,
    pub layer_idx: u32,
    pub bbox: BBox,
    pub rects: Arc<[RectRec]>,
    pub polys: Arc<[PolyRec]>,
    pub paths: Arc<[PathRec]>,
    pub decoded_bytes: u64,
}
```

페이지는 `(cache identity 또는 mmap된 OVM generation, page_id, file_off, csize)`로
식별하고 immutable하게 공유한다. 현재 OVM page record에는 payload checksum이
없으므로 parse 성공, record limit, 예상 길이 검증으로 손상을 검출한다. LRU budget은
decoded bytes 기준이며 기본값은 현재 vfsd의 1024 MiB를 따른다. stale generation에서
끝난 decode도 같은 cache identity이면 LRU에 남길 수 있지만 frame에는 commit하지
않는다.

### 7.2 FrameScene

`FrameScene`은 `HierPlan`을 복사 전개하지 않고 다음 참조만 가진다.

- `WsKey -> WsCell` index
- 이번 round에서 사용 가능한 page id bitset
- frame/wash records
- paint plane 순서와 style snapshot
- top `WsKey`

tile worker는 world tile bbox를 inverse transform하여 WC/page local bbox로
내려 보낸다. `Grid`는 기존 `grid_ranges`, `Pts`는 선택된 subset 또는 chunk bbox를
이용한다. 하나의 대형 repetition을 frame 전체 command vector로 펼치지 않는다.

## 8. CPU raster 계약

### 8.1 좌표

- layout geometry: i64 DBU
- raster viewport: 요청의 f64 DBU target box를 그대로 보존
- edge/intersection 산술: i128 checked arithmetic
- device subpixel/intersection: i128 Q32.32 pixel
- Y축은 `view.y1 -> image row 0`으로 변환
- overflow 또는 limit 초과는 명시적 render error

계획용 `ViewBox`만 보수적으로 정수 DBU로 반올림한다. raster target box에는 원래
float bbox를 전달하여 `-10.9375 µm` 같은 half-DBU 위상을 잃지 않는다. device
교차점은 frame 전체 원점에서 Q32.32로 한 번만 계산하며 band마다 다른 반올림을
하지 않는다.

### 8.2 도형

- rectangle: tile clip 후 scanline span 직접 기록
- polygon: active-edge scan conversion, fill rule은 M0 KLayout probe로 고정
- path: fully visible path도 동일 stroker를 사용한다. 경계 path만 다른 표현으로
  바꾸지 않는다. start/end extension과 join의 KLayout rounding을 golden으로 고정.
  퇴화 spine/U-turn은 성공 응답에서 geometry를 생략하지 않고 명시적으로 실패
- outline: device-pixel width, bbox cull 시 `width + 1px` 확장
- text: M4 전에는 별도 plane으로 비활성 가능하되 최종 cutover 전 deterministic
  bundled font와 0/90/180/270도, center alignment를 구현

M1은 binary occupancy로 geometry 누락을 먼저 검증한다. M3에서 8-bit coverage
antialias를 추가한다. antialias 변경이 geometry culling이나 pick 결과에 영향을
주어서는 안 된다.

### 8.3 paint와 pattern

각 image tile은 한 worker가 아래 전 순서를 처리한다.

```text
gray frame geometry + gray block text
design layers: layer number 오름차순, 각 layer의 geometry + design text
white frame geometry + white block text
```

레이어 병렬 합성은 하지 않는다. 서로 겹치는 레이어의 결과를 worker 완료 순서로
합성하지 않기 위함이다. text는 별도의 최상위 plane이 아니라 현재 KLayout 경로와
같이 자신이 속한 design/frame paint plane 안에서 그린다.

- design fill은 불투명 overwrite
- speckle off pixel은 기존 하위 pixel을 유지
- 2x2 speckle mask는 absolute device coordinate에 적용
- visibility가 바뀌어도 다른 레이어의 pattern phase는 바뀌지 않음
- 16x16 사용자 pattern은 column을 device x에 고정하고 source row를
  `(device row + framebuffer height - 1) mod 16`으로 선택하는 KLayout 위상 사용
- tile 경계에서 pattern/AA/outline seam이 없어야 함

## 9. 병렬화와 취소

초기 수평 band 구현은 2D tile worker로 교체됐다. render마다 `min(jobs, tile_count)`의
bounded worker를 만들고 tile index를 atomic counter로 배분한다. 각 worker는 tile
크기의 scratch RGBA에 그린 뒤 `(tile_index, scratch)`를 반환하고, coordinator가
tile의 `(row,col)` 고정 위치에 한 번 복사한다. 완료 순서는 pixel 결과에 영향을
주지 않는다. worker별 전체 framebuffer와 공유 framebuffer에 대한 unsafe 동시
쓰기는 없다. 기본 tile은 128px이며 CLI/daemon의 `tile_px`로 1..4096 범위에서
고정할 수 있다.

구현된 `RenderCancellation`은 `generation < before_generation`일 때만 취소하는
strict monotonic frontier다. 새 render 명령 `N`을 읽는 stdin thread가 즉시 frontier를
`N`까지 올리므로 단일 render worker가 plan/decode/PNG encode 중이어도 별도
`cancel` 명령을 받을 수 있다. frontier update와 최종 rename은 짧은 mutex 임계구역으로
직렬화하고, 나머지 hot-path 확인은 atomic load만 사용한다.

취소 확인 지점:

- page read/decode 전후
- scene 생성의 WC batch 사이
- Grid/Pts member batch 1024개마다
- paint plane 사이
- 현재 worker band 시작/끝 및 향후 image tile 시작 전
- PNG encode 전

취소된 generation은 frame을 publish하지 않는다. 이미 decode된 immutable page는
LRU에 남겨 다음 요청이 재사용할 수 있다. PNG는 encode 뒤에도 다시 확인하며,
same-directory 임시 파일의 최종 rename은 frontier와 원자적으로 결정한다.

## 10. daemon 프로토콜

구현된 초기 프로토콜은 기존 vfsd와 같은 line protocol을 사용해 새 dependency를
피한다. cache는 프로세스당 한 번만 열며 모든 field와 path에는 whitespace를
허용하지 않는다. CLI의 micron 좌표와 달리 daemon `view`는 raw DBU다.

```text
open cache=/abs/design.oas.floe budget_mb=1024 jobs=8
style epoch=3 path=/tmp/floe-style-3.tsv
render gen=42 view=x0,y0,x1,y1 w=1922 h=1082 depth=full cut=3 exact=0 layers=all frames=on mono=off jobs=8 tile_px=128 decode_pages=512 round_pages=128 round_paths=0 style_epoch=3 out=/tmp/floe-frame-42.png
cancel before_gen=43
info
quit
```

style TSV는 주석/빈 줄을 허용하며 아래 네 field를 bottom-to-top 순서로 쓴다.
layer key는 이름, `L/D`, `idx:N` 중 하나이고 중복 layer는 오류다.

```text
L/D  #RRGGBB[AA]  solid|speckle|clear|pat:HEX64  1..8
```

generation은 strict 증가해야 한다. 같거나 작은 render는 `dropped ... reason=stale`,
진행 중 또는 queue에 있던 이전 render는 `cancelled gen=N phase=...`로 응답한다.
`exact=1`은 `cut=0 depth=full frames=off`와 함께만 허용하며 상충 옵션을 조용히
덮어쓰지 않는다. `style_epoch`을 보낸 render는 현재 style과 일치해야 한다.

응답:

```text
frame gen=42 round=1 final=0 png=/tmp/floe-frame-42.png partial=1 deferred=31 ...stats
cancelled gen=41 phase=render
dropped gen=40 reason=stale
error gen=42 code=limit message=...
```

`round_pages` 기본값은 128이며 한 round에서 새로 읽을 최대 page 수다.
`decode_pages`는 generation 전체 page 상한이고 생략하면 plan 전체를 refinement한다.
기본 `round_paths=0`에서는 같은 generation이 `final=1`까지 같은 출력 경로를 원자
교체한다. Python adapter는 `round_paths=1`을 사용해 intermediate round마다 고유
경로를 받고, 읽은 partial을 즉시 지운다. 이 방식은 응답 N을 읽는 중 daemon이
같은 경로를 N+1로 교체하는 consumer race를 없앤다. final은 요청의 원래 출력
경로에 게시하며 종료 시 private 임시 디렉터리와 함께 정리한다. PNG는 동일
디렉터리의 임시 파일에 완전히 기록한 뒤 rename한다. protocol path에는 whitespace를
허용하지 않는 임시 디렉터리를 사용한다. cancel frontier와 rename이 직렬화되므로
stale 확인과 publish 사이의 경쟁도 없다. 안정화 후에만 length-prefixed pipe/socket
전환을 검토한다.

## 11. 단계별 구현

### M0 — 계약 동결과 scaffold

작업:

- Cargo workspace와 세 crate 생성
- 동일 `rust/` workspace path dependency 연결 및 offline build 설정
- KLayout 0.30.9 baseline fixture runner 작성
- rectangle, polygon, path, hierarchy, Grid/Pts, frame, pattern별 golden 생성
- 현재 RenderWorker의 6개 대표 request를 익명 JSON trace로 저장하는 collector 작성
- 1/4/8 worker benchmark harness 형식 확정

종료 gate:

- 빈 renderer CLI가 cache open, plan summary, selected page decode까지 수행
- valmini 6뷰의 plan/pages가 기존 `floe-index plan`과 일치
- golden/benchmark 출력에 source 이름·좌표를 넣지 않는 규칙 확정

### M1 — 단일 스레드 exact geometry renderer

작업:

- page cache와 FrameScene
- hierarchy transform 누적
- One/Grid/Pts tile-local traversal
- rectangle/polygon binary raster
- solid color와 visibility
- raw RGBA/PNG 출력

종료 gate:

- cut=0/full-depth fixture에서 source geometry record/member 누락 0
- rectangle/polygon PX1/PX2/PX4가 P-a/P-b/P-c 통과
- rotate/mirror/negative/skew repetition fixture 누락 0
- worker=1 반복 실행 SHA-256 동일

### M2 — image-tile 병렬화

초기 수평 band는 2D tile 독립 소유 방식으로 교체됐다. atomic work distribution,
결정적 tile commit, 64/128/256 크기 A/B, generation 취소의 daemon 연결까지
완료했다. render마다 thread를 생성하는 현재 bounded pool을 daemon 수명 고정 pool로
재사용하는 최적화는 thread 생성 비용이 실측 병목일 때만 수행한다.

작업:

- 완료: bounded worker pool과 atomic tile 분배
- 완료: 64/128/256 tile 크기 A/B
- 완료: tile-local traversal과 tile scratch의 결정적 합성
- 완료: cancellation token의 tile 시작점 연결과 tile telemetry

종료 gate:

- worker 1/2/4/8 RGBA SHA-256 동일
- tile 크기 64/128/256 결과 동일
- tile seam fixture pixel 차이 0
- render-bound 화면에서 provisional scaling gate 충족
- 완료: 실제 Python adapter의 연속 100 generation에서 stale publish 0

### M3 — KLayout 표시 계약

현재 color/visibility/paint order, 공통 위상 speckle, 16x16 pattern, mono,
1~8px outline, PATH 원 spine 중심선, wash와 frame 4밴드 vertical slice 및 독립
Rust 회귀 테스트를 완료했다. 실제 KLayout style PNG 자동 대조도 jobs 1/8에서
통과했다. 현재 oracle/GUI의 oversampling=1 binary 계약에는 별도 8-bit edge
coverage를 적용하지 않으며, Calibre/KLayout 목표 화면이 이를 요구한다는 실측이
나올 때만 범위를 다시 연다.

여기서 제외한 edge coverage는 OVC density overview와 다르다. `design.ovc` density
post-composite는 M5 adapter에서 구현되어 GUI의 `v` 토글로 동작한다.

작업:

- 완료: path stroker와 extension/join rounding, 원 spine 중심선
- 현재 binary 화면 계약에서 제외: 8-bit edge coverage
- 완료: 2x2 speckle inverse pair와 absolute phase
- 완료: 16x16 fill, outline width, mono
- 완료: frame 4밴드와 paint stack

종료 gate:

- 기존 `validate_render_speckle.py` 계약을 Rust test로 이식
- 기존 `validate_render_frames.py` 계약을 Rust test로 이식
- visibility 변경 전후 surviving layer pattern phase 동일
- non-Manhattan polygon/path와 tile 경계 golden 통과

### M4 — text/label

완료:

- Noto Sans Mono asset/source/SHA-256/OFL 기록·번들
- generation label plan 공유, frame-local glyph cache, center alignment, 90도 단위 회전
- design text와 gray/white block label paint plane
- daemon `labels`, `font_px` 및 Python `FLOE_RUST_LABEL_PX` 연결
- 요청별 6..96px 크기, Rust backend 전용 GUI/CLI live 제어
- 요청 크기를 글리프뿐 아니라 declutter 간격·블록명/말줄임 fit·블록명 padding에도
  동일 비율로 적용(14px 기본 선택/픽셀 계약 유지)
- design text의 합성 계층 회전(0/90/180/270) 보존; reflection은 anchor와
  baseline 방향에 반영하되 glyph는 가독성을 위해 mirror하지 않음
- OASIS TEXT 자체에는 각도 필드가 없으며 record 17 합성 회전만 적용한다.
  임의각/배율 record 18은 전체 geometry parser/indexer의 명시적 범위 밖이고,
  텍스트만 수평으로 대체 렌더링하지 않음

종료 gate 현황:

- label anchor/rotation/visibility 및 worker/tile byte 동일 회귀 통과
- 번들 글꼴·회전·alpha를 포함한 RGBA CRC32 golden 고정
- VFS 4096 label budget과 raster 262,144 glyph 상한으로 작업량 제한
- OS font lookup이 코드 경로에 없고 번들 bytes만 사용

### M5 — render daemon과 Python A/B

현재 `floe-renderd`의 단일-cache session, style epoch, 영속 decoded-page LRU,
전체 render protocol, generation 취소, deterministic PNG handoff와 외부 Python
adapter vertical slice는 완료했다. headless parent probe/통합 계약과 native GTK
Rust 실행·goto pan/zoom·frames/labels 상태 검증은 통과했다. 독립 KLayout/Rust
자동 oracle도 통과했으며 실제 실칩의 장기 GUI A/B 운용이 남아 있다.

작업:

- 완료: `floe-renderd` protocol/session/PNG handoff
- 완료: 기존 `RenderWorker` API를 구현하는 in-tree Rust adapter
- 완료: 부모의 최소 `FLOE_RENDERER=klayout|rust` opt-in hook
- 완료: coverage.py OVC density post-composite 유지(Rust progressive frame 연결)
- 완료: 기존 GUI status schema로 Rust phase stats 전달

종료 gate:

- GUI 코드의 render job/result schema 변경 없음
- view/pan/zoom/depth/layer/color/fill/mono/frame/label 조작 회귀 없음
- KLayout/Rust 동시 trace에서 geometry 누락 0
- daemon crash 시 오류 표시와 재시작 가능

### M6 — 실칩 성능 gate와 기본 전환

작업:

- fit, hotspot, single-layer near, depth full, warm pan trace
- 1/4/8/16 worker scaling과 CPU utilization
- page decode vs scene vs raster vs PNG 병목 분리
- decoded LRU와 render tile 기본값 확정

종료 gate:

- 사용자 승인된 실칩 화면에서 정확도 gate 전부 통과
- 대표 render-bound 화면에서 `raster_us` 기준 확정 scaling gate 통과
- peak RSS가 설정 budget + framebuffer/tile scratch 상한 이내
- 완료: Rust를 기본으로 전환하고 `FLOE_RENDERER=klayout` rollback 유지

### M7 — 상호작용과 KLayout 런타임 제거

작업:

- 완료: 현재 게시 FrameScene 기반 bounded pick/snap
- 제외: exact-page 우선 decode 재시도. 부모의 계약이 “what you see”이므로 미게시
  페이지를 query만을 위해 추가 decode하지 않음
- 완료: headless `floe render`를 Rust worker로 전환. 기존 solid archival fill,
  settled progressive frame 대기, PNG magic 검증, fsync+atomic replace를 유지하고
  `--labels --label-font-px`, `--frames`, finite/full depth를 연결
- 완료: exact plan/병렬 decode/평탄화/정수 polygon clip/OASIS writer
- 완료: `cache.py`의 `klayout.db`를 실제 legacy 함수 호출까지 지연하고,
  `service.py`의 KLayout renderer/viewport import도 legacy worker 내부로 격리.
  frame layer/live-cap 계산은 KLayout-free `view_policy.py`로 분리
- 완료: 기본 portable bundle에서 KLayout wheel/import를 제거하고 NumPy/Pillow,
  GTK/PyGObject와 두 Rust binary를 유지. `FLOE_PORTABLE_KLAYOUT=1`만 별도
  `-klayout` rollback bundle을 생성
- 완료: `floe index`를 Rust VFS subprocess로 전환하고 cache reuse/명시적
  `--force`, Rust 옵션, `--legacy` 경계를 고정

종료 gate:

- 완료(vertical slice): Rust scene/unit fixture와 부모 adapter wire schema
- 완료(integration): 동일 VFS 작업 집합의 KLayout pick/snap oracle 결과 동치
- 완료: `view`, `render`, `probe`, `info`, Rust `clip` import/worker 선택
  clean-environment selfcheck. 실제 portable bundle 검증은 후속
- 완료: Rust worker는 abstract capability=false, GUI 메뉴/`a` 동작 비활성화.
  명시적 KLayout rollback worker에서만 abstract capability=true
- `rg '^import klayout' floe/` 결과가 dev/legacy 경로에만 존재
- KLayout은 개발 oracle로만 남음

## 12. 다음 착수 작업 목록

현재 vertical slice 이후 작업을 아래 순서로 나눈다.

1. 완료: `session: partial-page progressive refinement rounds`
2. 완료(vertical slice): `adapter: parent backend hook + in-tree Python RenderWorker`
3. 완료: `test: 100-generation cancellation soak + KLayout style oracle automation`
4. 완료: `path: operating-input inventory + unsupported-path hard failure`
5. 완료: `query: published FrameScene pick/snap + KLayout oracle parity`
6. 완료: `coverage: OVC density post-composite + off/on integration gate`
7. 완료: `clip: exact scene flatten + KLayout Region XOR oracle`
8. `parallel: consider daemon-lifetime pool only if thread startup is measurable`

progressive/adapter vertical slice의 완료 정의였던 `page_prio` refinement, 100회
pan/zoom burst의 stale publish 0, 기존 GUI job/result schema를 유지한 opt-in Rust
frame 반환은 모두 충족했다. native GTK Rust 세션의 cold/warm goto와 frames/labels
상태도 확인했다. 다음 운용 gate는 실제 실칩의 장기 KLayout/Rust GUI A/B다.

PATH inventory 결과 OASIS page의 `PathRec`에는 start/end extension만 있고 별도
round-cap flag는 없다. 운영 5개 cache에서는 U-turn도 없었다. 따라서 검증되지 않은
U-turn을 추측 구현하지 않고 hard error로 고정했으며, 원형 source geometry는 indexer의
결정적 polygon 정규화 경로를 사용한다. 향후 운영 자산에서 U-turn이 발견될 때만
source/OASIS/KLayout oracle을 함께 추가해 범위를 다시 연다.

## 13. 검증 matrix

| 계약 | fixture/gate | 판정 |
|---|---|---|
| page decode | valmini 전 page | record/member/layer count 동일 |
| hierarchy | 6 viewport + micro/array boundary | layer geometry XOR 0 |
| transform | rot/mirror/negative/skew fixture | bbox/occupancy 동일 |
| repetition | One/Grid/Pts, duplicate Pts | 누락·중복 0 |
| binary raster 정확도 | PX1~PX5 | P-a edge band, P-b component, P-c area drift 통과 |
| determinism | jobs 1/2/4/8/16 | RGBA와 PNG SHA 동일 |
| tile seam | edge가 모든 tile 경계를 통과 | XOR 0 |
| pattern | speckle/fill + visibility toggle | phase·색·적층 동일 |
| KLayout style oracle | PX 13 + style 14, jobs 1/8 | 허용 edge 외 RGB 완전 일치 |
| frame | 4밴드 collision fixture | gray < design < white |
| pick/snap | 동일 VFS scene + KLayout service oracle | found/좌표/도형/순환 dict 동일 |
| exact clip | rectangle/diagonal/분리 concave + valmini | KLayout Region XOR 0 |
| OVC density | 동일 cut view coverage off/on | vector 보존, blank density 합성 |
| cancellation | 100 zoom/pan generation | stale publish 0 |
| corrupt input | ovm/ovp offset·length 오류와 page OASIS 손상 | panic 없이 명시 오류 |
| memory | 큰 page/repetition | configured budget 준수 |
| scaling | render-bound field trace | M0에서 확정한 4/8 worker gate |

KLayout oracle 비교는 renderer 개발 기간에만 사용한다. Rust 내부 test는 가능한
모든 계약을 독립 fixture/golden으로 옮겨 KLayout 설치 없이도 일상 회귀를 잡는다.

## 14. 주요 위험과 대응

| 위험 | 대응 |
|---|---|
| polygon/path rounding 차이 | M0에 경계·음수·비스듬한 edge probe, i128 고정소수점 |
| tile 경계 seam | absolute subpixel 좌표, tile-local 원점 반올림 금지 |
| hierarchy를 tile마다 반복 순회하는 비용 | 먼저 측정; 필요할 때만 WC별 tile bin cache 추가 |
| 대형 Grid/Pts 전개 폭발 | `grid_ranges`, Pts chunk/subset, member batch cancel |
| worker 수에 따른 pixel 차이 | tile 단독 소유, 레이어 직렬 paint, 공유 blend 금지 |
| PNG가 새 병목 | raster/PNG 시간을 분리하고 frame publish용 raw/shared-memory는 후속 검토 |
| text/font host 차이 | 라이선스 확인된 font를 bundle, OS font lookup 금지 |
| pick tie/source 순서 drift | canonical hull, OVM layer 순회, stable source ordinal 회귀 고정 |
| parent Rust API drift | renderer adapter 한 crate로 격리, parser/planner 복사 금지 |
| Python integration rollback 어려움 | `FLOE_RENDERER` A/B와 기존 KLayout 경로 유지 후 cutover |

## 15. 완료 정의

다음을 모두 만족할 때 KLayout renderer 대체가 완료된 것으로 본다.

- Rust renderer가 기본 경로다.
- exact geometry, pattern/frame, determinism, cancellation gate가 모두 통과한다.
- 실칩 1/4/8 worker scaling과 RSS gate가 통과한다.
- GUI의 render/depth/layer/style 조작에 기능 회귀가 없다.
- `view`, headless `render`, `probe`가 KLayout 미설치 번들에서 동작한다.
- pick/snap을 Rust scene이 제공한다.
- GUI clip이 Rust exact OASIS writer로 동작한다.
- KLayout은 개발 oracle과 레거시 도구에서만 사용한다.
