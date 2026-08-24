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
번만 올린다.

## 코드 소유권

- `floe2/`: 제품 identity, Rust-only backend 강제, CLI 진입점만 소유한다.
- `floe/`: 공유 GUI/CLI/cache/adapter 구현과 안정판 KLayout worker를 소유한다.
- `rust/`: 유일한 parser/VFS/renderer 구현이다. 제품별 복사본을 만들지 않는다.
- `tools/validate_floe2.py`: backend, CLI 표면, KLayout-free import와 socket 분리를
  검증하며 `tools/validate_rust.sh`의 필수 gate다.

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
