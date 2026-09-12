# floe2-web — Rust 애플리케이션 CLI (개발 중)

현재 **일반 레이아웃 `index/info/render/probe`와 잡덱 `index`·분석 `jobdeck`을 지원**한다. 이름에 web이 있지만 HTTP 서버나
브라우저 UI는 아직 없다. 기존 Python `floe2`/GTK와 병행 개발하는 별도 실행 파일이다.

```sh
cd rust
cargo build --offline --locked --release -p floe-app -p floe-index -p floe-renderd
./target/release/floe2-web index --help
./target/release/floe2-web index /path/to/design.oas --jobs 12
./target/release/floe2-web index /path/to/deck.jb --level 1,3 --lod --jobs 12
./target/release/floe2-web jobdeck /path/to/deck.jb --level 1,3 --mode chip \
  --report /path/to/analysis.json --spec /path/to/composite.spec
./target/release/floe2-web index /path/to/design.oas --occupancy-only --occupancy-um 4
./target/release/floe2-web index /path/to/design.oas --profile-cell TOP \
  --profile-jobs 8,12,16 --profile-snapshot /path/to/scratch/profile.bin
./target/release/floe2-web info /path/to/design.oas
./target/release/floe2-web info /path/to/design.oas --json
./target/release/floe2-web render /path/to/design.oas --at 100,200 --size 50,50 \
  --px 800x800 --thin keep --out /path/to/shot.png
./target/release/floe2-web probe /path/to/design.oas
```

`cargo`는 `rust/`에서 실행해야 그 안의 `.cargo/config.toml`(vendor, musl linker)이
적용된다. 저장소 루트에서 `--manifest-path rust/Cargo.toml`만 주는 것과 다르다.

- `--force` 없이 current 캐시는 재사용, stale/incomplete 캐시는 거부한다.
  LOD는 기본 off; current 캐시의 생성 옵션은 `--lod`만으로 바뀌지 않는다.
- occupancy 추가/교체는 기존 OVM/OVP/OVT를 보존한다. 기존 요약을 바꾸려면
  `--occupancy-only`를 명시한다. 프로파일 stdout은 native JSON 그대로다.
- `FLOE_INDEX_BIN` 명시 override → 개발 tree release → 실행 파일 옆 → PATH.
  잘못된 override/버전 불일치는 hard error. 실행 시 Python은 필요 없다.
- 두 새 CLI 사이의 동시 색인을 막는 `<source>.floe.index.lock` 파일이 남는다.
  0바이트 lock inode이며 완료 후 잠금은 풀린다. 실행 중 파일을 지우면 안 된다.
  기존 Python/native CLI와 열린 뷰어까지 보호하는 서버 lease는 아니다.
- render 기본은 full depth/cut=0, solid fill이다. `--detail`/`--thin`/라벨·프레임은
  명시 선택하며 좌표 suffix와 단일 JSON report를 지원한다. 전체 옵션은 `render --help`.
  final partial/glyph truncation은 exit 3으로 PNG를 보존한다.
- `FLOE_RENDERD_BIN` 조회도 명시 override 실패를 거부한다. 기존
  `FLOE_RUST_JOBS`/`FLOE_RUST_RASTER_JOBS` 등 렌더 환경 옵션을 지원한다.
- 덱 index는 선택된 레벨의 plain OASIS 소스를 하나씩 실행한다. `--jobs`는
  파일 내부 worker 수이며 파일 간 곱해지지 않는다. LOD/occupancy를 전달하고,
  missing/unsupported source는 명시 후 skip, 실행 실패가 있으면 exit 2다.
  Ctrl+C/SIGTERM은 현재 child를 수거하고 다음 소스를 시작하지 않는다.
  덱 cell profile은 거부하므로 해당 source OASIS를 직접 지정한다.
- `jobdeck`은 source header·좌표·색상/skip ledger를 분석하며 색인/렌더를 실행하지 않는다.
  `--sources DIR`, `--colors FILE`, `--ly-dt cross|zip`, `--on-missing skip|fail`,
  `--lenient`, `--placements`를 지원한다. spec은 current 캐시만 참조하며 누락은 exit 3,
  그릴 배치가 전혀 없으면 오류다. report/spec은 각각 원자적으로 저장하지만 쌍의 트랜잭션은 아니다.
  분석 CHIP-block 색상과 뷰어용 level/source-chip 색상은 기존처럼 구분한다.
- 덱 `info/render/probe`, `view/clip/...`와 batch/mosaic/DRC export는 미이관 오류를 낸다.
  자동 Python fallback이나 기존 launcher/portable 교체는 없다.

범위·차이·검증·다음 단계는 [M1a 기록](../../docs/WEBUI_M1A.ko.md)에 있다.
