# floe / floe2 제품 경계

`floe2`는 별도 저장소나 Rust workspace 복제가 아니라 이 저장소 안의 Rust-only
제품 셸이다. renderer와 GUI 구현을 복사하지 않고 `floe/`의 공통 모듈과 하나의
`rust/` workspace를 사용한다. 제품별 차이는 진입점에서 고정한다.

| 계약 | `floe` | `floe2` |
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
.venv/bin/python -m floe view chip.oas
.venv/bin/python -m floe2 view chip.oas

# 어느 쪽에서 만들어도 두 제품이 같은 cache를 연다.
.venv/bin/python -m floe2 index chip.oas
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
- `floe/`: 공유 GUI/CLI/cache/adapter 구현과 안정판 KLayout worker를 소유한다.
- `rust/`: 유일한 parser/VFS/renderer 구현이다. 제품별 복사본을 만들지 않는다.
- `tools/validate_floe2.py`: backend, CLI 표면, KLayout-free import와 socket 분리를
  검증하며 `tools/validate_rust.sh`의 필수 gate다. valmini를 넘기면 실제 release
  index/info/probe/render/clip lifecycle까지 한 번에 검사한다.

공유 구현에 제품별 조건이 필요하면 `floe.product`의 identity를 사용한다. renderer
코드를 `floe2/`로 복사하거나 별도 cache suffix를 만들지 않는다. 제품이 충분히
안정된 뒤 저장소 자체를 나누는 결정은 별도 migration으로 다룬다.

## portable

```sh
tools/make_portable.sh
# floe2-portable-<version>-<date>.tar.gz

FLOE_PORTABLE_KLAYOUT=1 tools/make_portable.sh
# floe-portable-<version>-<date>-klayout.tar.gz
```

기본 bundle에는 두 Python 패키지가 함께 들어간다. `floe2`가 공통 구현인 `floe`를
import하기 때문이며, KLayout wheel은 포함하지 않는다. Python 패키지와
`floe-index`/`floe-renderd`는 항상 같은 checkout의 조합으로 배포한다.
빌드 스크립트는 조립된 runtime의 `selfcheck`가 성공해야만 tar를 생성한다.

## 운영 성능 gate

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
모든 trace는 coverage post-composite 없이 daemon PNG를 그대로 측정한다.
