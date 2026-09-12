# floe2-web — Rust 애플리케이션 CLI (개발 중)

현재 **일반 레이아웃/잡덱 `index/info/render/probe`, 분석 `jobdeck`, 기본 웹 `view`를 지원**한다.
기존 Python `floe2`/GTK와 병행 개발하는 별도 실행 파일이며 제품 전환은 아직 완료되지 않았다.

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
./target/release/floe2-web info /path/to/deck.jb --level 1,3 --json
./target/release/floe2-web render /path/to/deck.jb --level 1,3 --px 800x800 \
  --detail high --out /path/to/deck.png --report /path/to/deck.json
./target/release/floe2-web probe /path/to/deck.jb
./target/release/floe2-web view /path/to/design.oas --goto 13600,8600,700 \
  --depth full --detail high --jobs 8 --raster-jobs 4
./target/release/floe2-web view /path/to/deck.jb --level 1,3 --mode chip
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
  원인 없는 final partial/glyph truncation은 exit 3으로 기존 PNG를 보존한다.
  덱 source skip/예산 deferral은 표시된 부분 PNG와 complete=false report를 저장하고
  exit 3을 반환한다. `probe`도 덱 skip이 있으면 OK 대신 exit 3이다.
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
- `view`는 127.0.0.1만 열고 전용 임시 Firefox 프로필을 실행한다. 기존 사용자
  프로필을 건드리지 않으며 창/프로세스 종료·End session·Ctrl+C에 worker와 세션을 수거한다.
  Firefox가 없으면 `--firefox PATH`를 지정하거나 `--no-open`으로 private session
  JSON의 일회용 URL을 사용한다. 이 파일은 0600이며 링크를 공유/로그에 남기면 안 된다.
  소스 경로는 CLI에서만 등록한다(최대 32개). 웹의 open은 자동으로 색인하지 않는다.
  GTK launcher/portable은 그대로다. refinement는 off, 일반 layout margin은 on이다.
  `--frame-cache off`로 native retained frame 재사용과 margin을 함께 끄고 측정할 수 있다.
  잡덱 margin은 미지원이다.
- 웹 canvas는 왼쪽/가운데 drag(놓을 때 한 번 제출), 화살표 50%/Shift 10% pan,
  ± zoom, `Ctrl+A` fit, `f` frames를 지원한다. 레이어 `⋯`는 fill/pattern/선폭 편집,
  Label px는 6..96 device px다. `--labels`, `--no-frames`, `--label-font-px`는
  초기 표시 옵션이다. 전체 GTK 단축키·query/ruler parity는 아직 개발 중이다.
- `clip/...`와 batch/mosaic/DRC export는 미이관 오류를 낸다.
  자동 Python fallback이나 기존 launcher/portable 교체는 없다.

범위·차이·검증·다음 단계는 [M1a 기록](../../docs/WEBUI_M1A.ko.md)에 있다.
웹 실행·브라우저 검증/남은 범위는 [M1b §9](../../docs/WEBUI_M1B.ko.md#9-m1b-3--기본-canvas-뷰어와-rust-실행-명령)를 따른다.
