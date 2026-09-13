# floe2-web — Rust 애플리케이션 CLI (개발 중)

현재 **일반 레이아웃/잡덱 `index/info/render/probe`, 분석 `jobdeck`, 기본 웹 `view`, ICE/ASCII `drc` 조회를 지원**한다.
기존 Python `floe2`/GTK와 병행 개발하는 별도 실행 파일이며 제품 전환은 아직 완료되지 않았다.
M4a-1에서 native scene-pinned pick/snap client를 추가했지만 앱 controller/웹 query는
아직 미연결이다([M4 기록](../../docs/WEBUI_M4.ko.md)). native 호환 버전0.12.86으로
`floe-index`와 `floe-renderd`를 함께 재빌드한다. 공유 기능과 현장 Firefox 수용은 별도다.

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
./target/release/floe2-web drc /path/to/results.db.ice --rules
./target/release/floe2-web drc /path/to/results.db --errs M1.WIDTH --floe-reviewer reviewer1
./target/release/floe2-web drc /path/to/results.db --errs M1.WIDTH --svrf-rules /path/to/deck.rules.json
./target/release/floe2-web drc /path/to/results.db --build --jobs 12
./target/release/floe2-web drc /path/to/results.db --build --force --jobs 12
./target/release/floe2-web view /path/to/design.oas --drc /path/to/results.db.ice
./target/release/floe2-web view /path/to/design.oas --drc /path/to/results.db.ice --drc-rules /path/to/deck.rules.json
./target/release/floe2-web view /path/to/design.oas --drc /path/to/results.db --drc-rules /path/to/deck.rules.json
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
- `drc` 읽기는 기존 pack(또는 .db 옆의 fresh pack)을 우선하며, 없거나 stale/corrupt이면
  읽기 전용 ASCII로 fallback한다. `--rules`/`--errs`/`--list`·소수 좌표를 지원한다.
  pack의 per-reviewer waive는 읽되 ASCII fallback에는 적용하지 않는다.
  source/pack/autosave를 생성·수정하지 않으며 자동 pack-build는 없다.
  웹 `view --drc RESULTS.db|PACK.ice [--drc-waives FILE]`는 첫 소스에 묶인 읽기 전용 API를
  등록한다(별도 1 CPU + 256 MiB admission). DRC 패널은 읽기·선택·live In view/
  Selected 필터·순회·CD·복원을 ICE/ASCII 모두 지원한다. ASCII 소수 좌표는 µm로
  유지하고 잘린 레코드 개수를 표시한다. 최초 ASCII open은 전체 입력 스캔이다.
  웹은 CLI fallback과 달리 명시 파일만 읽으며 인접 ICE/ambient reviewer를
  탐색하지 않는다. ASCII에 `--drc-waives`를 함께 지정하면 오류다.
  웹 pack-build 서버는 명시 승인된 HTTP 작업으로만 생성/취소/새 identity 등록을
  수행한다. 웹 DRC 패널의 **Build pack… → Approve build**에서 명시 승인하며,
  jobs1..16(웹 기본4), 기존 pack 교체는 별도 unchecked 선택이다. 진행/취소 및
  결과 불명확 시 동일 요청 확인을 지원한다. 공유·편집/notes는 미이관이며 실제
  브라우저 승인 클릭 수용도 남아 있다([M2 기록](../../docs/WEBUI_M2.ko.md)).
  생성 승인은 현재 DRC 선택/준비된 이동을 초기화하지만 레이아웃을 재오픈하지 않는다.
  native 생성 terminal과 새 reader의 metadata open 완료는 별도 상태다.
- `drc RESULTS.db --build`는 명시적인 쓰기 작업이다. 기존 fresh `RESULTS.db.ice`는
  재사용하고 stale/corrupt/기존 pack 교체에는 `--force`가 필요하다. `--jobs`는
  1..16, 기본12다. 원본·review sidecar는 수정하지 않는다. 읽기 옵션과 병용할 수 없다.
  전용 임시 디렉터리에서 기존 native pack을 만든 뒤 metadata/fingerprint 검사와
  fsync를 거쳐 게시한다. 게시 전 실패·취소는 기존 pack을 보존한다. 소수 DBU는
  반올림하지 않고 build를 거부하므로 ASCII 읽기를 사용한다. stdout은 결과 JSON,
  stderr는 단계/유계 진행값이다. `<pack>.index.lock` inode는 종료 후에도 남는다.
  새 pack은0600, 교체 시 기존 일반 permission bits를 유지한다. `directory_synced`
  false 또는 `cleanup_warning` true는 게시 성공과 별도로 확인해야 한다.
  자원 lease는 process-local이므로 다른 실행 중인 뷰어를 자동 종료/갱신하지 않는다.
- CLI `drc --rules`/`--errs`에 `--svrf-rules FILE`을 명시하면 기존 version1
  sidecar의 규칙 정보/참고 측정값을 JSON에 추가한다. 원본 SVRF를 해석하거나
  경로를 자동 탐색하지 않는다. 일반 polygon width/signoff 판정기가 아니며,
  원본 parser는 미이관이다([M2 §15](../../docs/WEBUI_M2.ko.md#15-m2a-10a-svrf-sidecar규칙-메타데이터측정-코어)).
- `view --drc-rules FILE`은 명시한 rules snapshot을 DRC actor에 연결한다. 타입/규칙
  필터·scalar 측정 비교 API와 웹 패널 표시/상태 복원을 지원한다. 추가256 MiB를
  공통 admission에 예약하지만 CPU/worker 수는 늘지 않는다. 원본/include 경로를
  따라가거나 jobdeck TC root를 넓히지 않는다([M2 §16](../../docs/WEBUI_M2.ko.md#16-m2a-10b-svrf-등록타입규칙-필터측정-비교-api)).
  격리+goto 준비/단일 적용 및 최초 가시성 복원 API와 웹 Restore/Escape를 연결했다.
  jobdeck 물리 plane 격리는 다음 단계다([M2 §18~19](../../docs/WEBUI_M2.ko.md#18-m2a-10d1-레이어-격리복원-코어와-원자적-focus-api)).
- `clip/...`와 batch/mosaic/DRC export는 미이관 오류를 낸다.
  자동 Python fallback이나 기존 launcher/portable 교체는 없다.

범위·차이·검증·다음 단계는 [M1a 기록](../../docs/WEBUI_M1A.ko.md)에 있다.
웹 실행·브라우저 검증/남은 범위는 [M1b §9](../../docs/WEBUI_M1B.ko.md#9-m1b-3--기본-canvas-뷰어와-rust-실행-명령)를 따른다.
