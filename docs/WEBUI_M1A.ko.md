# 웹 전환 M1a — Rust 애플리케이션 이관 기록

2026-09-13, `feature/webui`. [M0 대조표](WEBUI_M0.ko.md),
[서비스 초안](WEBUI_SERVICE_API.ko.md), [실행 방법](../rust/app/README.md).

## 1. 완료 범위

- M1a-1: `cf7fbe1`, worker client와 M0 문서. renderd의
  handshake/open/style/render/cancel/cleanup, PNG/raw 오라클.
- M1a-2a: `b3a95d7`, **일반 OASIS `index` CLI 경로**. `rust/app`의 `floe2-web` 실행 파일,
  `rust/app-core`의 바이너리 조회·캐시 검증·옵션 정책·프로세스 수명주기.
  기존 `floe-index`를 직접 실행하며 Python에 위임하지 않는다.
- M1a-2b: `d985f44`, 일반 레이아웃 info/단일 PNG render/probe. 상세 계약은 §7.
- M0-D7: `cc98ce5`, native 빈 plan 보완(§8).
- M1a-3a: `4ad0f28`, 잡덱 문법·순수 배치 모델(§9).
- M1a-3b: `75f1917`, source header/catalog와 덱 index(§10).
- M1a-3c: jobdeck 색/레이어/spec·분석 CLI(§11).
- **M1a 전체 완료가 아니다.** 덱 info/render/probe CLI와
  서버 진행 이벤트·view lease는 아직 없다. HTTP/WS/브라우저 UI도 미구현이다.

기하 raster·인덱싱 알고리즘, LOD/occupancy 표현 정책, 캐시 포맷은 변경하지
않았다. 빈 플랜 오류만 §8에서 별도 수정했다. `feature/jobdeck` 실측 브랜치와
기존 Python 제품/portable은 유지한다. 내부 개발 crate 버전은 0.1.0이며
indexer/renderd 호환 버전 0.12.85와 다르다.

## 2. index 계약

- `--jobs` 기본 12, page target/LOD/slow-cell/P2 shard 옵션과 selected-cell
  jobs/repeat/snapshot을 typed 값으로 전달한다. NaN/inf/음수/바이트 곱 overflow,
  누락 값/미지원 옵션은 native 실행 전에 거부한다.
- LOD 기본 off. Python CLI처럼 `--lod`가 명시되면 생성하고, `--no-lod`와
  둘 다 있을 때도 `--lod`가 우선한다. current 캐시의 build 옵션 변경은
  `--force`가 있어야 실제 재색인한다.
- 원본 size/초 단위 mtime, meta CACHE_VERSION/VFS type, **기존 `Vfs::open`의
  OVM/OVP/OVT 구조·쌍 검증**을 모두 통과해야 current다. OVM 자체의 source
  fingerprint도 함께 확인한다. meta JSON은 identity만 읽고 frontier는 건너뛴다.
  source hash/새 revision 포맷은 추가하지 않았다.
- `--force` 없는 기존 stale/incomplete 캐시는 거부한다. force는 native의
  기존 파일 교체를 허용하며 **이전 캐시 백업/전체 캐시 원자적 교체가 아니다**.
- `--occupancy`/`--occupancy-um`은 current 캐시에 요약이 없으면 additive,
  있으면 재사용한다. `--occupancy-only`는 current 캐시에 요약만 교체한다.
  이 경로의 `--lod/--no-lod`는 기존 페이지를 재생성하지 않는다.
- 프로파일은 normal cache와 lock을 만들지 않는다. stdout JSON object/array는
  native 그대로이며 진행 로그는 stderr다. snapshot은 명시한 별도 파일에만
  쓴다. normal `.floe` 안의 snapshot 경로는 refresh를 줘도 거부한다.
- source는 abspath 의미를 보존한다. source symlink로 호출하면 그 **별칭 옆**
  캐시를 사용하며 원본 target 옆으로 몰래 이동하지 않는다. 공백/한글을 지원,
  native가 지원하지 않는 비 UTF-8 경로는 손실 변환하지 않고 거부한다.
- 인덱서 override → 개발 release → CLI 인접 → PATH. 명시 invalid/empty
  override는 폴스루하지 않는다. `--version`은 5초/4KiB 이내의 응답을 검사하고
  현재 indexer manifest 버전과 비교한다(제품/renderer 버전과 혼동하지 않기).
- wrapper 진행 prefix는 `[floe2-web]`, 실행 진단은 stderr. 잘못된 인자는 exit 2,
  cache/IO/버전 실패는 1, native 실패 코드는 보존한다. SIGINT=130/SIGTERM=143.
  cleanup 실패는 따로 경고하며 원래 실패 코드를 덮지 않는다.

## 3. 동시성·취소와 남은 경계

- 새 CLI의 non-profile 요청은 `<src>.floe.index.lock`을 잡고 cache 검사부터
  native 종료까지 유지한다. create/open은 NOFOLLOW, file lock은 OS가 회수한다.
  unlock 후 inode를 삭제하지 않아 다른 inode를 동시에 잠그는 레이스를 피한다.
  최초 lock 생성에는 source 부모의 쓰기 권한, 이후에는 lock 접근 권한이 필요하다.
  이 권한이 없는 read-only 배포의 `index` 재사용은 현재 지원하지 않는다.
- cache destination symlink는 force로도 거부한다. meta/OVM/OVP/OVT의
  특수파일·symlink도 current 판정에서 거부한다. 동일 UID 악성 프로세스나
  외부 캐시 교체까지 sandbox로 막는 기능은 아니다.
- subprocess는 독립 process group. 부모가 받은 SIGINT/SIGTERM은 소유한
  child에 전달하고 1초 후 필요하면 kill/reap한다. 오류/취소/Drop에서 이 작업의
  `design.ovo.tmp`를 회수하고 기존 `design.ovo`는 보존한다.
- **새 CLI끼리만 잠금에 참여**한다. 기존 Python `floe2 index`, 직접 실행한
  `floe-index`, 열린 renderd/GTK는 아직 lease에 참여하지 않는다. 외부 재색인과
  동시 실행하지 않는 M0 전제가 그대로다. NFS lock 동작도 현장 확인 대상이다.
- 준비의 in-process VFS 구조 검증은 도중 취소되지 않고 진입/완료 시 취소를
  확인한다. 긴 인덱싱에 render의 300초 deadline을 적용하지 않았다.
- `IndexJob::poll/cancel`은 컨트롤러 호출용이다. native stdout/stderr는 현재
  CLI FD로 스트리밍한다. 웹용 bounded 진행 이벤트, server-wide jobs≤16
  admission, worker/read lease, snapshot 동시 작성 lease는 후속 작업이다.
- 임의 wrapper가 자손 프로세스에 pipe를 넘기는 경우는 지원 대상이 아니다.
  직접 실행하는 신뢰된 native indexer 프로세스/스레드 모델이 기준이다.

## 4. 의존성과 폐쇄망 빌드

이번 JSON/Unix signal 처리에만 필요한 의존성을 추가했다. Axum/Tokio/HTTP는
아직 도입하지 않았다. 주 의존성은 serde 1.0.228(derive), serde_json 1.0.151(std),
signal-hook 0.4.4(**default features off**). 기능/라이선스 근거는 upstream
[serde_json manifest](https://docs.rs/crate/serde_json/latest/source/Cargo.toml),
[signal-hook manifest](https://docs.rs/crate/signal-hook/latest/source/Cargo.toml)다.

Cargo가 registry checksum으로 받은 소스를 `cargo vendor`로 생성한 뒤 **새
디렉터리만** 편입했다. 기존 12개 vendor 파일/버전은 변경하지 않았다.
lock의 신규 registry package는 16개:

| 추가 | 버전 |
|---|---|
| serde / serde_core / serde_derive | 각각 1.0.228 |
| serde_json / itoa / memchr / zmij | 1.0.151 / 1.0.18 / 2.8.3 / 1.0.23 |
| proc-macro2 / quote / syn / unicode-ident | 1.0.107 / 1.0.47 / 2.0.119 / 1.0.24 |
| signal-hook / signal-hook-registry / errno | 0.4.4 / 1.4.8 / 0.3.14 |
| windows-sys / windows-link | 0.61.2 / 0.2.1 |

Windows 두 package는 errno의 target dependency를 offline resolution에 포함한
것이며 Windows 실행 지원을 뜻하지 않는다. 라이선스 원문은 vendor 안에 보존.
대부분 MIT OR Apache-2.0, memchr는 Unlicense OR MIT, zmij는 MIT,
unicode-ident는 (MIT OR Apache-2.0) AND Unicode-3.0이다.

새 app/core의 File lock API 때문에 Rust 1.89 이상을 선언한다. 실제 빌드는
저장소 기준 1.97.1에서 검증한다. 형식적인 최소 버전 전체 테스트와 advisory DB
보안 감사, Linux 배포 패키지 검증은 아직 완료 판정하지 않는다.
M1b의 네트워크 의존성·MSRV·portable 승인은 별도 게이트다.

## 5. 회귀 게이트

```sh
cd rust
cargo fmt -p floe-app -p floe-app-core -- --check
cargo clippy --offline --locked -p floe-app -p floe-app-core --all-targets --no-deps -- -D warnings
cargo test --offline --locked -p floe-app -p floe-app-core
cargo build --offline --locked --release -p floe-app --target x86_64-unknown-linux-musl
```

`tools/validate_app_cli.py valmini.oas`는 개인 임시 복사본으로 다음을 검사한다.
개발 오라클에만 Python을 쓰고 Rust runtime의 PATH는 비워서 실행한다.

- 기존 Python CLI와 default/LOD 캐시 OVM/OVP/OVT 바이트 일치.
- current 재사용의 바이트/mtime 불변, source/meta/1바이트 marker 손상 거부,
  FIFO 비차단 실패, force, source alias/cache symlink 구별.
- occupancy add/rebuild 시 normal cache 불변, existing summary 재사용.
- profile JSON, jobs 1/2×repeat 2, snapshot load/save, no normal-cache writes.
- invalid override/version/옵션, native argv/실패 코드, writer lock,
  부모에만 전달한 SIGINT/SIGTERM의 child 전파·reap·임시파일 정리.

새 gate는 `tools/validate_rust.sh`에 필수 연결했고 대형 인자를 주더라도 이
게이트는 valmini만 복사한다.

2026-09-13 실행 결과:

- macOS/arm64: fmt, 새 app/core strict clippy, 단위 5개 통과.
- `validate_app_cli.py`: `RUST APP CLI: ALL OK`. 마지막 cleanup 오류 코드
  보존 항목 추가 후 release를 재빌드하고 이 게이트를 다시 실행했다.
- 전체 `validate_rust.sh`: `RUST VALIDATION: ALL OK`, exit 0.
  jobdeck 80개, renderer 46개, occupancy 25개와 KLayout oracle도 통과했다.
  기존 tiler/vfs/render-core의 unrelated compiler warning은 그대로 남았다.
- **빈 CARGO_HOME + 빈 CARGO_TARGET_DIR**에서 `--offline --locked` musl
  release 빌드 통과. 생성 파일은 x86-64 ELF static-pie이며 신규 소스를
  Cargo registry cache에 의존하지 않고 vendor에서 빌드했다.
- Linux 실행·NFS file lock·공유 서버 admission·실칩 성능은 미측정이다.
  기존 indexer 알고리즘을 바꾸지 않았으므로 인덱싱 가속 성과로 해석하지 않는다.

## 6. 다음 단계

1. M1a-3c: jobdeck 색·UI 레이어, spec 및 덱 분석/읽기 CLI 이관.
2. M1b: 네트워크 의존성 게이트, controller/진행 이벤트/lease → gateway/Canvas.

## 7. M1a-2b — 일반 레이아웃 읽기와 단일 캡처

`floe2-web info SOURCE`, `render SOURCE`, `probe SOURCE`를 추가했다.
Rust `catalog/styles/shots/render/artifact` 모듈을 CLI가 공유하며 Python fallback,
HTTP 서버, GTK 실행은 없다. PNG는 renderd의 원본 바이트를 그대로 저장한다.

- info 사람용 출력은 Python과 동일. **새 opt-in `--json`**은 metadata와
  source_stale를 제공한다. 기존 캐시의 정수/DBU를 유지하는 로컬 JSON이며,
  큰 정수를 문자열로 보내야 하는 브라우저 wire DTO와는 아직 별개다.
- 메타의 frontier는 보관하지 않는다. meta≤256MiB, layers≤65,536,
  DBU/bbox/키 중복, canonical VFS 구조와 meta/OVM identity를 검사한다.
  Serde `float_roundtrip`을 켜 좌표의 1-ULP 손실을 피한다(신규 vendor 없음).
- 소스 size/mtime 변경은 기존 읽기 CLI처럼 **경고 후 캐시를 사용**한다.
  버전/DBU/OVM 쌍 불일치·손상은 hard error다. index의 stale 거부와 구분한다.
  이 단계의 읽기 경로는 아직 managed read lease가 아니므로 실행 중 외부
  재색인/summary 교체 금지 전제가 유지된다.
- `--bbox`, `--at --size`, 단위 suffix, center/lb anchor, WxH aspect 확장과
  stretch, width-only half-even 높이 계산, fractional DBU를 보존한다.
  default exact는 **cut=0, wire exact=0**인 기존 캡처 계약이다. headless depth는
  기본 full, low/medium/high는 5/3/1px, thin auto는 layout cull이다.
- 레이어 이름/별칭(동일 별칭의 여러 datatype), explicit pair, 중복 제거를
  보존한다. 메타는 소스 기록 순서, **렌더 스타일은 (L,D) 정렬 순서**다.
  첫 PNG 대조가 이 차이를 잡았고 수정 후 바이트 일치를 확인했다.
- 레이어 색을 layer-number palette로 정규화하고 source/stem `.layerprops`
  기본 색을 적용한다. 색/패턴 `.def`는 compile-time 포함이므로 Python 파일을
  runtime에 찾지 않는다. PNG export는 속성 파일과 관계없이 solid/width=1,
  probe는 기존 live 기본 speckle/속성 패턴·width를 사용한다.
- 기존 render 환경 jobs/raster_jobs/budget/tile/round/open timeout을 읽는다.
  `FLOE_RENDERD_BIN`은 명시 invalid/empty면 hard error(기존 폴스루 정규화).
  native manifest 버전 handshake, source alias, frame 파일 소비는 worker-client.
- `Config.shutdown_requested`가 ready/open/style와 긴 poll 대기를 중단한다.
  CLI SIGINT/SIGTERM은 이 flag로 연결하고 child 종료/수거 후 130/143을 반환한다.
  renderer의 generation cancel과 프로세스 전체 종료 요청은 별개다.
- export는 **final && !partial && deferred=0 && !labels_truncated**만 성공이다.
  불완전이면 exit 3, 기존 PNG는 보존한다. PNG 한 장≤16Mpx. same-directory
  create_new 임시파일→write/sync→rename이며 소스·캐시·index lock·symlink target을
  출력으로 허용하지 않는다. rename 이후 늦은 signal을 미게시로 보고하지 않는다.
- 단일 `--report` JSON은 기존 필드/단위(시간·출력명 제외)를 보존한다.
  PNG와 report 각각 원자적이지만 **두 파일 전체 transaction은 아니다**.
  report 실패 시 PNG가 이미 저장되었다고 명시한다. batch/mosaic/DRC/PNG metadata
  export와 jobdeck render/--level은 후속 범위이며 조용히 무시하지 않는다.
- probe는 같은 worker에서 600×600 fit/depth0 → 중앙 tile/full 두 요청의
  final/complete와 종료를 확인한다. 브라우저 표시·ETX 성능 판정이 아니다.

`tools/validate_app_render.py valmini.oas` 게이트:

- info 출력/typed metadata, **12가지 PNG 바이트 및 report** 대조: 전체/width/
  단위·anchor/stretch/half-phase/depth·detail·labels/선택/속성/별칭/다중 round.
- runtime PATH=""에서 실제 index→info/render/probe, 캐시 바이트/mtime 보존.
- NaN/과대 픽셀/0면적/옵션·경로 오류, stale 경고/손상 실패/override 실패.
- 가짜 daemon의 ENOSPC·final partial·glyph truncation에서 기존 PNG 보존.
  ready/open/style/render 각각 부모에만 SIGINT/SIGTERM→8회 취소·reap·temp 정리.
- 발견한 기존 native 결함 **M0-D7**: `layers=none`이면 빈 plan에 top이 없어
  오류. Python/Rust 모두 명시 실패하는 회귀로 먼저 고정했으며 빈 PNG 성공으로
  위장하지 않는다. 별도 native 수정 단계에서 전체 off 표시 gate로 바꾼다.

2026-09-13 실행 결과:

- app/core/worker-client fmt와 strict clippy 통과. 단위 13개와 fake worker
  lifecycle 9개 통과(real worker는 별도 필수 gate에서 실행).
- `RUST APP READ: ALL OK`, 12 PNG/report 대조와 signal 8회 포함.
- `validate_rust.sh`: `RUST VALIDATION: ALL OK`, exit 0. 기존 인덱싱·VFS·
  occupancy·jobdeck·renderer 및 KLayout oracle 회귀도 통과했다.
- vendor만 사용하는 `--offline --locked` Linux x86_64 musl release 빌드 통과.
  실제 Linux 실행/ETX 화면/실칩 성능은 이 결과에 포함하지 않는다.

## 8. M0-D7 — 전체 레이어 off의 빈 프레임

VFS는 가시 레이어가 없거나 뷰에 아무것도 없으면 빈 working set을 반환한다.
렌더 scene은 root가 필수인데 기존에는 occupancy summary만 이 경우를 보완했다.
`Cache::plan`에서 working cells와 pages가 **둘 다 빈 경우만** 빈 root를 추가한다.
잘못된 비어 있지 않은 plan의 missing-root 검증, 페이지/cut 정책, 기존 구조
프레임은 그대로다. 데이터/인덱스 포맷 변경이나 재인덱싱은 없다.

- CLI Python/Rust oracle: 모든 레이어 off와 뷰 바깥은 검은 PNG 성공,
  depth 0 + frames on은 레이어가 모두 off여도 구조 경계를 유지.
- 실제 persistent worker: retained cache on에서 on→off→on의 픽셀 복원 확인.
- native compatibility 버전 0.12.84. `floe-renderd` 재빌드가 필요하다.
- 2026-09-13: render-core 116개, worker protocol 3개·lifecycle 9개 및
  실제 worker gate 통과. CLI PNG/report 비교는 빈 뷰 3건을 추가해 15건이다.
  전체 `validate_rust.sh`도 `RUST VALIDATION: ALL OK`, exit 0.

## 9. M1a-3a — 잡덱 문법·좌표 모델

`app-core::jobdeck::{parser,geom}`에 순수 Rust 모델을 추가했다. 아직 새 CLI의
`.jb`/`--level` 거부는 풀지 않는다. 실제 source probe·색상/레이어 tree·native spec
생성·덱 index/render 연결은 다음 단계이며 이 단계에서 완료로 표시하지 않는다.

- 관찰된 한 줄 `$ (...)`, CHIP tail, Y/X 순서 ROWS, MTITLE/OPTION,
  독립 AD/SF/TC, LY/DT, 미정의 필드·위치 인자·unparsed/warning을 보존한다.
  명칭 변환 규칙이나 미확인 directive의 배치 효과를 새로 추론하지 않는다.
- strict 구조 오류 거부, lenient 오류 ledger와 잘린 항목 제외를 지원한다.
  UTF-8 손상 바이트는 기존 `errors="replace"`와 같다. 더 엄격해진 경계는
  괄호 불균형/짝 오류, NaN/Inf·i64 밖 식별자, 잘못된 ROWS 줄의 부분 적용 금지다.
  이 경우도 lenient에서 잘못된 배치를 성공으로 내보내지 않는다.
- `ratio=AD/source_dbu`, `mag=SF×ratio`, `dx=jx−cx×ratio`를 동일 연산 순서로
  계산한다. `SF`는 정렬 offset에 곱하지 않는다. 전체 덱 DBU/extent가 격자를
  정하고 선택된 배치만 생성한다. half-even 반올림·±2^62 범위·잔여 오차도 보존.
  양수가 아닌 AD/SF와 좌표 산술 overflow/underflow는 명시 오류다.
- LY×DT(default), zip(last datatype 반복), source 문제의 selected skip과
  outside-selection ledger, CHIP별/공유 layer table 및 보고서를 보존한다.
  `out_of`의 내부 CHIPs×모든 레이어 복제는 없애고 공유 키로 같은 출력 번호를
  조회한다. 보고서 형식이나 실제 배치 순서는 바뀌지 않는다.
- 신규 서비스 안전 경계: 입력 64MiB/한 줄 1MiB/괄호 64중첩, 보고서 CHIP×level
  4M 항목, 플랜 기본 visits 4M·배치 2M·LY/DT 및 layer table 65,536·출력 모델
  추정 메모리 256MiB. **초과는 전체 오류이지 부분 성공/픽셀 생략이 아니다.**
  긴 이름·반복 skip anchor도 할당 전에 회계한다. 모델 예산은 프로세스 전체 RSS
  상한이 아니며 parser 입력/임시 자료구조/JSON 보고서는 별도다. PlanOptions로
  조정 가능한 라이브러리 경계이고 서버 전체 admission은 M1b에서 관리한다.

게이트 `tools/validate_app_jobdeck.py`는 고객 파일 없이 기존 synthetic DECK와
seed 고정 100개 덱으로 Python oracle을 만들고 Rust 통합 테스트가 소비한다.
보고서/모든 entry/ROWS/source 순서와 630개 cross·zip·선택·색상 grouping용 플랜을
정수/float 값을 포함해 대조한다. `tools/jobdeck_expected.json`의 독립 손계산
17개 배치도 별도로 확인한다. 일반 Cargo test의 ignored test는 이 스크립트가
필수 실행하고 fixture 누락은 실패한다. 전체 배터리에 연결했다.

2026-09-13 결과: app-core 단위 13개와 strict clippy/fmt 통과. 모델 gate는
최종 123개 문법 사례와 630개 플랜에서 `RUST APP JOBDECK MODEL: ALL OK`.
전체 배터리도 `RUST VALIDATION: ALL OK`, exit 0이며 마지막 진단 숫자 표기
7건 추가 후에는 모델 gate와 clippy를 재실행했다. Offline/locked Linux musl
release 빌드도 통과했다. 실칩/Calibre의 새 관찰 결과를 검증한 것은 아니다.

## 10. M1a-3b — 소스 조회와 덱 인덱싱

`floe2-web index deck.jb --level 1,3 --lod --jobs 12`를 지원한다. 신규 registry
의존성은 없고 기존 floe-oasis/flate2를 app-core에 직접 연결했다.

- OASIS는 기존 `Cur`를 공유하는 `floe_oasis::header::probe_start`로 최초
  4KiB에서 START/unit만 읽는다. GDS는 최대 64KiB 안의 UNITS, gzip도 헤더만
  해제한다(압축 입력 1MiB 상한). geometry/CBLOCK을 순회하지 않는다.
- 공용 uint cursor가 10개 continuation byte 뒤 shift≥64에서 패닉하던 경계를
  명시 overflow로 수정했다. 유효 u64 최댓값과 5종 양수 real 인코딩은 유지.
  zero/NaN/Inf/reciprocal underflow DBU, 과대 문자열 길이, 잘린 헤더도 오류다.
  native 호환 버전 **0.12.85**, renderd/index 재빌드가 필요하다.
- plain OASIS만 indexable이다. GDS/gzip은 DBU를 알더라도 `unsupported`,
  미인식/손상/누락은 각각 `unknown_format`/`unreadable`/`missing`이다.
  unselected source는 DBU만 조회하며 캐시 상태를 읽거나 catalog에 등록하지 않는다.
- selected source의 indexed 판정은 canonical VFS 검증을 재사용한다. 옛 cache
  constructor의 미생성 `.tiles`/손상 시 빈 경로 표시는 `.floe` 대상으로 정규화했다. 오류의 상세
  문구는 native byte 위치를 포함할 수 있으며 status/필드 의미가 호환 경계다.
- 레벨에 포함된 source만 TC 순서로 처리한다. 같은 어휘 정규화 **cache destination**을
  가리키는 `a`/`./a`는 한 번만 실행하되 별도 source symlink 옆 캐시는 합치지 않는다.
  파일마다 기존 `PreparedIndex`가 lock/freshness/force를 다시 확인하고 동일 child
  실행기를 쓰므로 동시에 두 파일의 jobs가 곱해지지 않는다.
- current는 비파괴 재사용, `--force --lod`는 실제 재색인. occupancy-only는
  indexed source의 summary만, unindexed source는 index+summary로 처리하는 기존
  덱 의미를 유지한다. `.jb.floe` 합성 캐시는 만들지 않는다.
- 누락/unsupported source는 명시 skip이며 기존 index CLI처럼 exit 0도 가능하다.
  실행 실패는 다른 source를 계속 처리한 뒤 exit 2; 부모 SIGINT/SIGTERM은 현재
  child에 전파·수거·소유 임시파일 정리 후 130/143, 다음 source는 시작하지 않는다.
- 의도된 개선(M0-D8): 기존 덱 wrapper가 버리던 page-target/slow-cell/P2 tuning을
  이제 전달한다. 덱 profile은 조용히 일반 index가 되지 않고 명시 거부한다.
  catalog preflight/읽기 경로의 managed read lease는 아직 M1b 범위다.

`validate_app_jobdeck_sources.py`: plain OASIS/GDS/gzip, malformed/oversize varint,
FIFO/누락과 indexed 전후 catalog 대조, unselected FIFO meta 무접근; 실제 3-source
덱의 선택/default/LOD/force/occupancy를 Python 캐시 OVM/OVP/OVT 바이트로 대조한다.
캐시 재사용 mtime, stale 비파괴 거부, unknown level/profile 거부, alias dedup,
빈 skip batch, 실패 후 다음 source, 부모에만 보낸 SIGINT/SIGTERM을 검사한다.

검증: source oracle(indexed 전후), 실제 덱 인덱싱 gate 모두 `ALL OK`.
oasis 9 / app-core 14 단위 테스트, app/app-core strict clippy 및 fmt,
offline/locked Linux musl release 빌드 통과. 전체 `sh tools/validate_rust.sh`도
`RUST VALIDATION: ALL OK`, exit 0이다. 소스 디렉터리 alias를 `canonicalize`하지
않으므로 symlink별 캐시 경계를 보존하며, 실칩/동시 서버 부하 측정은 포함하지 않았다.

## 11. M1a-3c — 색상·뷰 목록·분석·합성 spec

`floe2-web jobdeck deck.jb --sources DIR --level 1,3 --mode chip --placements
--report report.json --spec composite.spec`가 Python 없이 분석한다. 기존 색상 JSON,
cross/zip, missing skip/fail, strict/lenient를 공유 `Analysis` 서비스로 옮겼다.

- 10색 회전·49색 reserve·핀 우선순위, 분석 CLI의 CHIP-block 색상 삽입 순서를
  보존한다. GUI용 목록은 별도로 level head → 정규화 TC leaf 구조다. 동일
  소스의 global ordinal과 색상은 전체 덱에서 정하므로 load selection으로 바뀌지 않는다.
- analysis `ids`는 배치와 CHIP 색상 삽입을 제한하지만 모든 source를 probe한다.
  load `load_ids`는 source/cache probe와 표시 행을 제한하고, 나머지 source는
  header DBU만 본다. 두 선택 모두 전체 덱 기준 좌표계를 유지한다.
- 레이어명/동명 소스/숫자 L/D/옛 `$n TITLE` 선택, level head 확장, load dialog
  행과 leaf별 placement 수를 이관했다. 아직 브라우저 패널을 만든 것은 아니다.
- spec은 현재 캐시의 LY/DT만 참조한다. not_indexed/empty_layer는 기존과 같은
  line=-1, stage=spec ledger; source/placement 순서·scale·정수 dx/dy·색상 동일.
  float의 지수 0 패딩은 달라도 f64 값은 같다. 캐시/header DBU 불일치나 probe 후
  source 변경은 오류로 거부한다(열린 뷰 hot reload/managed read lease는 아님).
- source metadata의 모든 layer를 소스마다 상주시켜 곱하지 않는다. 실제 참조
  LY/DT만 보관하며 lookup 추정 64MiB, view table/row text 각각 128MiB/65,536행,
  spec/JSON artifact 128MiB 상한은 명시 오류다. 합계 RSS 상한이나 truncation이 아니다.
- `--report`/`--spec`은 source/덱/색상 JSON/캐시/lock을 출력으로 쓰지 못한다.
  미선택·누락 source와 directory alias도 보호한다. 같은 디렉터리 임시파일+
  sync+rename을 재사용하며 두 artifact는 **개별 원자적**이다. 모든 사전 검사를
  마친 뒤 report부터 게시한다. 그 뒤 spec I/O가 실패하면 report 게시 사실을 알린다.
- 의도된 오류 정리: 문법·옵션/그릴 배치 없음은 exit 2, I/O는 1, skip 결과는 3.
  기존 Python의 일부 `SystemExit(string)`=1과 구분된다. lenient 구조 오류도
  console과 JSON에 노출한다. 분석 color token의 제어문자, native spec의 non-hex
  색상은 명시 거부한다. 런타임에 index/renderd나 Python을 실행하지 않는다.

`validate_app_jobdeck.py`는 123개 문법/630개 플랜에 3모드 palette·pins·선택,
load dialog·뷰 행·색상·selector oracle를 더했다. `validate_app_jobdeck_plan.py`는
24개 분석/load 모델, 20개 CLI report, 17개 spec 및 실제 daemon 3모드 PNG 쌍을
대조한다. header-only unselected 손상 cache, selected not_indexed, 빈 레이어,
skip/fail, 경로 alias 보호, 산출물 보존을 포함하며 배터리에서 필수 실행한다.

2026-09-13 검증: app 4/app-core 16 단위, strict clippy/fmt,
offline/locked Linux musl release 빌드 통과. 전체 배터리의 신규 oracle와
기존 jobdeck·renderer·KLayout 검사를 포함해 `RUST VALIDATION: ALL OK`, exit 0.
실칩·ETX·브라우저 수용 검증을 완료한 것은 아니다.
