# 웹 전환 M1a — Rust 애플리케이션 이관 기록

2026-09-13, `feature/webui`. [M0 대조표](WEBUI_M0.ko.md),
[서비스 초안](WEBUI_SERVICE_API.ko.md), [실행 방법](../rust/app/README.md).

## 1. 완료 범위

- M1a-1: `cf7fbe1`, worker client와 M0 문서. renderd의
  handshake/open/style/render/cancel/cleanup, PNG/raw 오라클.
- M1a-2a: **일반 OASIS `index` CLI 경로**. `rust/app`의 `floe2-web` 실행 파일,
  `rust/app-core`의 바이너리 조회·캐시 검증·옵션 정책·프로세스 수명주기.
  기존 `floe-index`를 직접 실행하며 Python에 위임하지 않는다.
- **M1a 전체 완료가 아니다.** info/render/probe, jobdeck/parser/catalog와
  서버 진행 이벤트·view lease는 아직 없다. HTTP/WS/브라우저 UI도 미구현이다.

Rust renderer, 인덱싱 알고리즘, LOD/occupancy 표현 정책, 캐시 포맷은 변경하지
않았다. `feature/jobdeck` 실측 브랜치와 기존 Python 제품/portable도 그대로다.
내부 개발 crate 버전은 0.1.0이며 기존 indexer/renderd 호환 버전 0.12.83과 다르다.

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

1. M1a-2b: 일반 레이아웃 metadata/info → headless render/probe 서비스와
   CLI 이관. worker-client를 연결하고 PNG/export 정책을 같은 오라클로 검사.
2. M1a-3: jobdeck parser/catalog/선택/ledger/spec 및 덱 index 이관.
3. M1b: 네트워크 의존성 게이트, controller/진행 이벤트/lease → gateway/Canvas.
