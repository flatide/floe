# Rust renderer 작업 인계

갱신일: 2026-08-24
canonical 저장소: `/Users/journey/Flatide/floe`
현재 기준 커밋: `62d866f renderer: make Rust the default backend`

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
- KLayout은 개발 oracle과 명시적 rollback에만 남긴다. 기본 renderer는 Rust이며
  rollback은 `FLOE_RENDERER=klayout`이다. 알 수 없는 backend는 hard error다.
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

- persistent `floe-renderd`, decoded-page budget LRU, progressive page rounds,
  generation 취소, jobs/tile/page budget, phase telemetry
- rectangle/polygon/path, hierarchy, One/Grid/Pts repetition, frame/wash,
  speckle/custom fill/outline/mono의 결정적 CPU raster
- 번들 글꼴 label, live GUI/CLI font size, 합성 quarter-turn label rotation
- 게시 `FrameScene` 기반 bounded pick/snap; query는 다음 decode/raster와 병행
- OVC density coverage의 KLayout-free PNG post-composite
- cut=0/full-depth exact clip, rational 교점, KLayout half-tie 규칙, concave component
  분리, single-cell OASIS writer, 공백 경로/UTF-8 cell name atomic publish
- `view`, `render`, `probe`, `info`, `clip`의 KLayout-free 시작 경로
- headless `floe render`의 solid archival fill, settled frame 대기,
  `--labels --label-font-px`, `--frames --depth`, fsync + atomic PNG replace
- Rust가 기본 backend. KLayout은 명시적 rollback이며 legacy 모듈은 lazy import

최근 연속 커밋:

```text
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

### P2 — KLayout-free portable gate

- clean environment에서 Rust index → `info` → `probe` → labels 포함 `render` →
  UTF-8 cell-name `clip` 순서로 실행한다.
- Python import hook이 모든 `klayout` import를 거부하는 현재 renderer 통합 검사를
  index command까지 확장한다.
- 실제 portable tarball의 selfcheck가 `floe-index`, `floe-renderd`, GTK pixbuf PNG,
  NumPy/Pillow, `floe`를 검사하되 KLayout을 요구하지 않는지 확인한다.
- legacy rollback은 KLayout wheel을 넣은 별도 환경에서 계속 oracle로 검사한다.

### P3 — 운영 gate

- 사용자 실칩에서 native GTK 장기 A/B: fit, full-depth mid zoom 첫 방문, hotspot,
  single-layer near, warm pan
- jobs 1/4/8/16의 decode/raster scaling과 peak RSS
- 기존 문제였던 mid-zoom 첫 방문 9~10초 구간을 `plan/read/decode/scene/raster/png`
  telemetry로 기록. Rust 경로는 KLayout 단일 Layout 등록 단계가 없어야 한다.

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
끝났다. Rust workspace 단위 테스트는 140 passed + 3 ignored, renderer Python
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
