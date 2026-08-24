# Rust renderer 작업 인계

갱신일: 2026-08-24
canonical 저장소: `/Users/journey/Flatide/floe`
현재 구현 기준: P1/HIGH와 이 문서의 runtime hardening까지 반영된 부모 저장소 HEAD

이 문서는 대화 메모리에 의존하지 않고 부모 `floe` 저장소에서 바로 후속 작업을
재개하기 위한 짧은 인계서다. 상세 설계와 완료 gate는
`docs/RUST_RENDERER_PLAN.ko.md`, 실행 계약은 `docs/RUST_RENDERER.md`, 정확도
fixture는 `docs/RENDERER-TESTS.ko.md`가 canonical source다.

## 1. 저장소 소유권

- 이후 코드·계획·검증은 모두 부모 `floe`에서 수행한다.
- 이전 `/Users/journey/Flatide/rust-floe`는 2026-08-23 편입 직전 prototype
  snapshot이며 Git 저장소도 아니다. 여기의 `crates/`, `python/`,
  `IMPLEMENTATION_PLAN.ko.md`, `README.md`를 부모로 다시 복사하지 않는다.
- prototype의 유효 계약은 부모 문서와 `rust/render-*`, `floe/rust_render.py`,
  `tools/validate_rust_renderer.py`에 이미 포함됐고 부모 구현이 더 최신이다.
- `target/`, 실제/합성 `.oas`, `.floe` cache, 고객 layout/profile은 인계물이
  아니며 커밋하지 않는다.

## 2. 고정된 제품 결정

- 목표 화면은 Calibre와 실질적으로 동일한 CPU 화면이다. GPU는 현재 범위가
  아니고 속도보다 geometry·표시 정확도가 우선이다.
- Rust renderer 제품은 같은 저장소의 `floe2`이며 KLayout backend를 거부한다.
  안정판 `floe`는 KLayout을 기본으로 유지하고 `FLOE_RENDERER=rust` 명시 A/B만
  허용한다. 두 제품은 같은 `rust/`와 `<src>.floe` cache를 공유한다.
- abstract는 KLayout 고유 기능이라 Rust 범위에서 제외한다. Rust worker는
  `supports_abstract = False`이고 GUI 메뉴/`a` 동작도 비활성화된다.
- OASIS CIRCLE(record 27)은 인덱싱 시 결정적 내접 64각형 `PolyRec`으로 바뀐다.
  renderer에 원 primitive를 추가하거나 OVM/OVP 포맷을 올리지 않는다.
- OASIS TEXT 자체에는 각도 필드가 없다. record-17 계층 변환에서 합성된
  0/90/180/270도 회전만 보존한다. arbitrary-angle/magnified record 18은 text만
  수평 대체하지 않고 전체 parser/indexer 범위 오류로 유지한다.
- 글꼴은 SIL OFL Noto Sans Mono bytes와 pure-Rust `fontdue`를 binary에 번들한다.
  OS/KLayout font lookup은 하지 않는다. 요청별 6..96 정수 screen px 크기가 glyph,
  declutter, block-name/ellipsis fit, padding에 같은 비율로 적용된다.
- OVM/OVP/design.ovc는 기존 mmap 기반을 유지한다. renderer가 VFS planner 결과와
  page OASIS payload를 직접 소비하며 KLayout delta/Layout 등록을 거치지 않는다.
- 병렬 단위는 독립 page decode와 2D image tile raster다. worker 수가 달라도 PNG
  bytes가 같아야 하며, stale generation은 게시되지 않아야 한다.

## 3. 현재 완료 상태

- persistent `floe-renderd`, decoded-page budget LRU와 generation 상주 상한,
  progressive page rounds, render/clip generation 취소, jobs/tile/page budget,
  phase telemetry
- rectangle/polygon/path, hierarchy, One/Grid/Pts repetition, frame/wash,
  speckle/custom fill/outline/mono의 결정적 CPU raster
- rectangle fast path, polygon, PATH outline 내부가 같은 Q32.32
  `PixelCenter ∪ LowerBoundary` scan-conversion 규칙을 사용. half-phase에서
  KLayout과 Rust 각각 RECT/POLYGON/PATH 표현 간 exact 동일성 gate 통과
- 번들 글꼴 label, live GUI/CLI font size, 합성 quarter-turn label rotation. glyph 상한
  초과는 geometry frame을 실패시키지 않고 결정적 label 접두부만 표시
- 게시 `FrameScene` 기반 bounded pick/snap; page bbox를 먼저 자르고 query당
  repetition member 400개/포함 pick 후보 64개를 상한으로 둔다. query는 다음
  decode/raster와 병행
- 축퇴 2-D Grid는 렌더/query/clip 공통 전개 전에 명시 오류로 거부하고, 손상
  OASIS의 point-list 선언 개수는 남은 payload byte보다 클 때 할당 전에 거부
- OVC density coverage의 KLayout-free PNG post-composite
- cut=0/full-depth exact clip, rational 교점, KLayout half-tie 규칙, concave component
  분리, single-cell OASIS writer, 공백 경로/UTF-8 cell name atomic publish
- `view`, `render`, `probe`, `info`, `clip`의 KLayout-free 시작 경로
- GTK를 막지 않는 비동기 cold open, SIGINT 격리, open/clip timeout, 소비한
  progressive PNG/style TSV의 즉시 정리
- headless `floe2 render`의 solid archival fill, settled frame 대기,
  `--labels --label-font-px`, `--frames --depth`, fsync + atomic PNG replace
- `floe2`가 Rust-only backend를 소유하고 `floe`는 KLayout 기본 안정판. 공통
  backend factory의 legacy 모듈은 계속 lazy import

최근 연속 커밋:

```text
2975d74 renderer: reject unsafe repetition workloads
6efefda renderer: complete KLayout-free 0.12 cutover
62d866f renderer: make Rust the default backend
f628270 renderer: route headless exports through Rust
95555a0 renderer: decouple Rust startup from KLayout
3b774c4 renderer: add exact Rust OASIS clip
250ccd3 renderer: scale label planning with Rust font
d73f6df renderer: enable Rust density coverage
1e58f2d renderer: add concurrent Rust pick and snap
dfa71a9 renderer: preserve hierarchy label rotation
6b4c221 renderer: make Rust label size live
f357b3f renderer: add deterministic Rust labels
```

## 4. 바로 이어갈 작업

### P1 — portable의 필수 KLayout 제거 (완료: 2026-08-24)

기본 `tools/make_portable.sh`에서 KLayout wheel과 selfcheck import를 제거하고
NumPy/Pillow와 GTK/PyGObject를 유지했다. `FLOE_PORTABLE_KLAYOUT=1`은 이름에
`-klayout` suffix가 붙는 별도 rollback/oracle bundle을 만든다.

이 작업은 `floe index`를 함께 정리해야 한다. 현재 Python `cmd_index`는 legacy
KLayout indexer이므로 KLayout-free portable에서 그대로 호출할 수 없다.

권장 경계:

1. 완료: 기본 `floe index SRC`를 함께 배포되는 `floe-index vfs SRC SRC.floe`
   subprocess로 위임.
2. 완료: `--jobs`, 명시적 `--force`를 Rust 경로에 연결.
3. 완료: coverage/LOD/page target/slow-cell/P2 shard ceiling을 Rust 의미의
   옵션으로 노출. density overview는 GUI 기본과 맞춰 opt-in으로 유지.
4. 완료: `--skeleton-only`, `--texts-only`, `--merge-only`, `--merge`, `--tile-mb`,
   `--mem`, `--mem-floor`, `--no-gov`, text cap, `--bands`, KLayout read/edit mode는
   Python legacy 전용이므로 `--legacy` 또는 별도 legacy command에서만 허용한다.
5. 완료: binary 검색은 `FLOE_INDEX_BIN`, 개발 tree의
   `rust/target/release/floe-index`, Python executable 인접 `floe-index`, PATH 순으로
   두고 누락 시 설치 지침이 포함된 hard error를 낸다.
6. 완료: `--force`만 기존 `SRC.floe` 교체 권한으로 간주. 평상시에는 source
   fingerprint/cache version이 맞고 Rust `Vfs::open` 구조·pair 검증을 통과한
   cache만 재사용하며, 임의 cache 삭제를 추론하지 않는다. 명시한
   `FLOE_INDEX_BIN`이 무효면 다른 후보로 폴스루하지 않는다.

### P2 — KLayout-free portable gate (완료: 2026-08-24)

- 완료: `tools/validate_floe2.py`가 모든 `klayout` import를 차단한 subprocess에서
  실제 release Rust index → `info` → `probe` → labels/frame 포함 `render` → UTF-8
  cell-name `clip`을 공백/UTF-8 source 경로 하나로 연속 실행한다.
- 완료: clip은 Rust scan 재파싱, render는 PNG magic, 각 단계는 `floe2` product
  prefix까지 단언한다. 전체 gate는 대형 milestone source가 주어져도 이 lifecycle에
  복사하지 않고 항상 작은 valmini를 사용한다.
- 완료: portable selfcheck가 GTK pixbuf PNG, NumPy/Pillow, `floe`/`floe2`,
  `floe-index`, `floe-renderd`, KLayout 부재를 검사하고 실패를 exit status로
  집계한다. `make_portable.sh`는 tar 생성 전에 조립된 runtime의 selfcheck를 반드시
  실행한다. 실제 Linux tarball은 매 build에서 이 gate를 통과해야만 생성된다.
- 유지: 안정판 `floe` bundle은 KLayout wheel을 넣은 별도 환경에서 oracle로 검사한다.

### P3 — 운영 gate

- 완료: `tools/bench_floe2.py`가 실제 GUI와 같은 persistent session에서 fit,
  full-depth mid zoom 첫 방문, hotspot, single-layer near, 5회 warm pan을 고정 순서로
  실행한다. jobs 1/4/8/16, 반복 횟수, viewport, budget을 한 명령으로 통제한다.
- 완료: adapter가 정확한 `plan/read/decode/scene/raster/png`, cache hit/miss,
  decoded resident, decode worker/tile 수를 결과 schema에 보존한다. 하네스는 daemon
  peak RSS와 jobs=1 대비 total/raster speedup까지 privacy-safe JSON으로 기록한다.
- 완료(milestone calibration): 이미 index된 1.4GB 합성 fixture에서 1200x800 전체
  trace와 jobs 1/4/8/16을 완주했다. 가장 무거운 hotspot의 raster는
  36.0/10.2/7.3/6.8ms(1 대비 1.00/3.54/4.95/5.31배), total은
  49/20/17/17ms였다. 같은 cache의 native GTK smoke도 Rust worker open과 첫 depth-0
  frame(24ms)을 표시한 뒤 정상 종료했다. 이 fixture는 analytic repetition/frame
  비중이 높아 decoded resident가 0MB였으므로 실칩 메모리 gate를 대신하지 않는다.
- 남음: 사용자 실칩에서 대표 hotspot/layer를 지정한 `--runs 3` 측정과 native GTK
  장기 `floe`/`floe2` A/B. 기존 mid-zoom 첫 방문 9~10초가 어느 단계인지 판정하고,
  Rust 경로에 KLayout 단일 Layout 등록 단계가 없음을 운영 기록으로 확정한다.

## 5. 검증 명령

일상 전체 gate:

```sh
cd /Users/journey/Flatide/floe
sh tools/validate_rust.sh
```

renderer만 빠르게:

```sh
env -u FLOE_RENDERER PYTHONDONTWRITEBYTECODE=1 \
  FLOE_RENDERD_BIN="$PWD/rust/target/release/floe-renderd" \
  FLOE_RUST_ROUND_PAGES=4 \
  FLOE_INTEGRATION_SOURCE="$PWD/data/m1/valmini.oas" \
  FLOE_INTEGRATION_CACHE="$PWD/data/m1/valmini.oas.floe" \
  .venv/bin/python -B tools/validate_rust_renderer.py
```

Rust 정적/단위 검사:

```sh
cd /Users/journey/Flatide/floe/rust
cargo fmt -p floe-render-core -p floe-renderd -- --check
cargo clippy -p floe-render-core -p floe-renderd \
  --all-targets --no-deps -- -D warnings
cargo test --workspace
```

마지막 `sh tools/validate_rust.sh`는 2026-08-24에 `RUST VALIDATION: ALL OK`로
끝났다. Rust workspace 단위 테스트는 158 passed + 3 ignored, renderer Python
검사는 13 pure + 1 real integration이며 KLayout jobs 1/8 pixel/style oracle도
모두 통과했다. 기존 workspace의 일부 unrelated compiler warning은 남아 있지만
renderer-core/renderd 대상 clippy hard-error는 없다.

## 6. 주요 파일

| 책임 | 위치 |
|---|---|
| 실행 계획/완료 gate | `docs/RUST_RENDERER_PLAN.ko.md` |
| renderer 운영 계약 | `docs/RUST_RENDERER.md` |
| 정확도 fixture 계약 | `docs/RENDERER-TESTS.ko.md` |
| Python backend 선택/legacy worker | `floe/service.py` |
| Rust queue adapter/coverage/atomic export | `floe/rust_render.py` |
| GUI font/abstract capability | `floe/gui.py` |
| headless render/clip/index CLI | `floe/cli.py` |
| scene/raster/font/query/clip | `rust/render-core/src/` |
| persistent protocol/parallel orchestration | `rust/renderd/src/main.rs` |
| VFS planner/text contracts | `rust/vfs/src/` |
| renderer integration | `tools/validate_rust_renderer.py` |
| 전체 gate | `tools/validate_rust.sh` |
| portable 제작 | `tools/make_portable.sh` |
