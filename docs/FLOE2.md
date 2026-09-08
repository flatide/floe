# floe2 제품 경계 (floe는 동결)

**2026-09-08부터 `floe2`가 유일한 제품 라인이다.** KLayout 기반 셸 `floe`는
시장에 릴리즈된 적 없이 `floe-legacy` 브랜치(`floe-frozen-2026-09-08` 태그)에
동결됐고, 이 저장소의 `main`은 floe2다. KLayout은 **개발용**으로만 남는다:
정확도 oracle, 테스트 데이터 생성기, 그리고 새 기능을 Rust에 넣기 전에
KLayout 경로에서 선행 검증하는 용도. 그 용도의 KLayout 셸은 여전히
`FLOE_PRODUCT=floe`로 실행할 수 있으며 아래 표의 `floe` 열은 그 개발 셸을
가리킨다. `floe2`는 renderer와 GUI 구현을 복사하지 않고 `floe/`의 공통 모듈과
하나의 `rust/` workspace를 사용하며, 제품별 차이는 진입점에서 고정한다.

| 계약 | `floe` (동결·개발 전용) | `floe2` (제품) |
|---|---|---|
| 기본 renderer | KLayout | Rust |
| backend override | `FLOE_RENDERER=rust` A/B 허용 | Rust 외 전부 하드 에러 |
| Python/KLayout legacy index | `index --legacy` | 도움말에서 제거, 실행 거부 |
| legacy `.tiles` profile | 제공 | 명령 제거 |
| density coverage (`design.ovc`) | KLayout에서 선택 제공 | 생성 옵션·UI·로딩·합성 제거 |
| VFS cache | `<src>.floe` v8 | 같은 cache를 비파괴 공유 |
| native binaries | `floe-index`, 선택적 `floe-renderd` | 같은 `floe-index`, `floe-renderd` |
| GUI instance | `floe-<uid>-<display>` | `floe2-<uid>-<display>` |
| portable | KLayout 별도 번들 | KLayout 없는 기본 번들 |

## 실행

```sh
.venv/bin/python -m floe2 view chip.oas      # 제품
.venv/bin/python -m floe2 index chip.oas     # 1회: 공유 <src>.floe/ 생성

# 개발 전용: 동결된 KLayout 셸로 같은 cache를 열어 선행 검증/비교
.venv/bin/python -m floe view chip.oas
```

두 GUI는 socket identity가 달라 같은 DISPLAY에서 동시에 실행할 수 있다. 캐시는
source 옆의 동일한 `<src>.floe`를 읽으므로 복사하거나 변환하지 않는다. cache와
Rust wire/OVM/OVP 버전은 계속 `floe/__init__.py`, `floe/cache.py`, `rust/`에서 한
번만 올린다. 공유 cache에 과거 `design.ovc`가 있어도 floe2 `info`와 Rust worker는
이를 무시한다. sample09 detail-high refinement 실측에서 화면 변화 없이
350ms→980ms로 느려져 제품 경로에서 제거했다.

실제 GTK 시작부터 Rust worker open과 첫 frame 표시까지 자동 확인할 때는 개발용
종료 타이머를 사용한다. 값은 100..60000ms이며 일반 실행에서는 설정하지 않는다.

```sh
FLOE_GUI_SMOKE_MS=8000 .venv/bin/python -m floe2 view --multi chip.oas
```

## 코드 소유권

- `floe2/`: 제품 identity, Rust-only backend 강제, CLI 진입점만 소유한다.
- `floe/`: 공유 GUI/CLI/cache/adapter 구현과 KLayout worker(동결된 개발
  셸·선행 검증용)를 소유한다.
- `rust/`: 유일한 parser/VFS/renderer 구현이다. 제품별 복사본을 만들지 않는다.
- `tools/validate_floe2.py`: backend, CLI 표면, KLayout-free import와 socket 분리를
  검증하며 `tools/validate_rust.sh`의 필수 gate다. valmini를 넘기면 실제 release
  index/info/probe/render/clip lifecycle까지 한 번에 검사한다.

공유 구현에 제품별 조건이 필요하면 `floe.product`의 identity를 사용한다. renderer
코드를 `floe2/`로 복사하거나 별도 cache suffix를 만들지 않는다. 저장소 분리는
2026-09-08에 **브랜치 승격**으로 갈음했다: 저장소를 나누지 않고 floe2 라인을
`main`으로 올리고 floe를 동결했다. 동결된 셸을 지우지 않는 이유는 oracle·생성기·
선행 검증이 그 코드 경로를 쓰기 때문이며, 새 기능은 floe2에만 넣는다.

## portable

```sh
tools/make_portable.sh
# floe2-portable-<version>-<date>.tar.gz   <- 릴리즈 산출물

FLOE_PORTABLE_KLAYOUT=1 tools/make_portable.sh
# floe-portable-<version>-<date>-klayout.tar.gz
# 개발·oracle 전용 번들(릴리즈 아님): floe(KLayout)와 floe2 launcher 동봉
```

기본 bundle에는 두 Python 패키지가 함께 들어간다. `floe2`가 공통 구현인 `floe`를
import하기 때문이며, KLayout wheel은 포함하지 않는다. Python 패키지와
`floe-index`/`floe-renderd`는 항상 같은 checkout의 조합으로 배포한다.
Python 패키지, `floe-index`, `floe-renderd`의 버전도 모두 동일해야 한다.
adapter는 daemon의 `ready version=...` 응답을 `floe.RENDERD_VERSION`과
비교하고, 다르면 첫 open/render 전에 선택된 바이너리 경로와 함께 명시 오류를
낸다. 따라서 이전에 설치한 `0.1.0` renderd가 탐색 순서에서 잡혀 새 repetition
처리 코드를 우회하는 상태는 첫 화면의 도형 오류로 위장하지 않는다.
`__version__`은 매 push마다 오르고 `RENDERD_VERSION`은 Rust 재빌드 push에만
오른다; `validate_floe2.py`가 `RENDERD_VERSION == 두 Cargo 버전`과
`__version__ >= RENDERD_VERSION`을 고정한다.
Rust 바이너리는 portable의 glibc 하한이 빌드 호스트 버전으로 상승하지 않도록
기본적으로 `x86_64-unknown-linux-musl` 정적 타깃으로 빌드한다. 별도 바이너리를
쓸 때는 `FLOE_INDEX_BIN`과 `FLOE_RENDERD_BIN`을 반드시 함께 지정한다.
빌드 스크립트는 조립된 runtime의 `selfcheck`가 성공해야만 tar를 생성한다.
KLayout을 포함한 `floe-portable`은 두 renderer가 모두 있으므로 `./floe`와
`./floe2`를 함께 제공한다. Rust-only 기본 `floe2-portable`은 `./floe2`만 제공한다.

## 운영 성능 gate

열린 renderer 최적화의 우선순위, sample9 기준선과 수용 조건은
`docs/FLOE2_OPTIMIZATION.ko.md`에서 추적한다. 성능 변경은 해당 이슈 ID와
before/after 수치 및 gate를 함께 갱신한다.

기존 VFS cache에 대해 실제 GUI와 같은 persistent Rust session 흐름을 재현한다.
fit(depth 0), full-depth 중간 줌 첫 방문, hotspot, single-layer near, 겹치는 5회
warm pan을 순서대로 실행하며 jobs 1/4/8/16의 단계별 시간과 daemon peak RSS를
기록한다. 최종 판정에는 대표 hotspot과 레이어를 명시하고 3회 반복한다.

```sh
.venv/bin/python -B tools/bench_floe2.py chip.oas \
  --hotspot X_UM,Y_UM,SPAN_UM --layer M2 --runs 3 \
  --out floe2-field-benchmark.json
```

JSON에는 source/layer/cell 이름과 좌표를 쓰지 않는다. 설정 좌표는 프로세스 안에서만
DBU로 변환되고 결과에는 `hotspot_supplied` 여부만 남는다. `plan/read/decode/scene/
raster/png`, cache hit/miss, decoded resident peak, 프로세스 RSS peak와 jobs=1 대비
total/raster speedup을 기록한다. `--hotspot`을 생략한 개발 fixture 실행은 design
중앙을 사용하지만, 실칩 운영 gate에서는 대표 좌표를 반드시 지정한다.
모든 trace는 coverage post-composite 없이 daemon PNG를 그대로 측정한다. 제품
기본은 page decode 8 workers, raster 4 workers, 384px tile이며 각각
`FLOE_RUST_JOBS`, `FLOE_RUST_RASTER_JOBS`, `FLOE_RUST_TILE_PX`로 재현한다.
cache hit page는 첫 frame에 전부 포함되어 warm 재방문에서 page 수만으로 refine하지
않는다. 제품 기본은 중간 refinement frame 없음(miss 전체를 한 batch)이며
`FLOE_RUST_ROUND_PAGES`로 스트리밍 round를 복원한다. 재방문·pan·margin은
daemon이 보관하는 label-free geometry frame(상태·배율당 1장, 최대 3장,
`FLOE_RUST_RETAINED_MB` 256MiB)의 tile 재사용으로 처리한다(F2R-16/17/18/20);
전량 재사용이면 page plan·decode를 생략하고 라벨만 다시 계획한다(F2R-21).

동결된 KLayout 셸(oracle)과 floe2의 기본 end-to-end renderer를 같은 조건으로
비교할 때는 두 명령 모두 같은 `--perf-baseline`을 사용한다. 이 preset은 refinement, exact final
frame cache, LOD, hierarchy frames와 labels를 끄되 page/working-set cache와 최종
PNG publish는 유지한다. detail/depth/좌표/window는 workload이므로 명시적으로 맞춘다.

```sh
.venv/bin/python -m floe view chip.oas --multi --goto X,Y,W \
  --detail high --depth 999 --perf-baseline
.venv/bin/python -m floe2 view chip.oas --multi --goto X,Y,W \
  --detail high --depth 999 --perf-baseline
```

개별 제어는 `--refinement on|off`, `--frame-cache on|off`, `--lod on|off`,
`--frames on|off`, `--labels on|off`다. `--refinement off`는 floe의 VFS stream을
0으로, floe2의 miss round를 single all-page batch로 바꾼다.
