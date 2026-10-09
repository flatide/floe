# floe-worker-client (웹 M1a-1)

Rust-only `floe-renderd` process client. 기존 GTK/Python 진입점을 바꾸지 않으며
HTTP 서버/CLI 전체 이관은 아직 아니다. Linux/macOS 대상, unsafe Rust 없음.

## 구현 범위

- 명시 executable + `ready` 버전 검증 → layout/deck `open` → typed style ack.
  호환 버전은 build 시 `renderd/Cargo.toml`에서 읽는다. 새 통신 필드나
  M1a 당시 daemon 변경은 없었다. M4a-1/2의 scene query·종류별 취소 추가로 native 호환
  버전은0.12.89이며 renderd/index를 함께 재빌드해야 한다.
- `RenderRequest`: DBU bbox, 치수, depth/cut/exact, all/none/layers,
  frames/labels/font, style/mono, thin, raster/decode jobs, PNG/raw.
  기본 refinement off(2^30 round pages). query는 M4a-1에서 아래와 같이 추가했고 clip은 미구현.
- 단조 generation, 취소 frontier, stale frame 폐기, partial/final 구별.
  이전 세대의 실제 오류도 `Event::Failed`로 전달해 cancelled로 위장하지 않는다.
- private 0700 디렉터리의 source alias로 공백/한글 경로 지원. style은 ack 후,
  frame은 소비 후 삭제. 출력 slot과 정확히 일치하는 경로만 읽고 회수한다.
  symlink/FIFO/hardlink를 frame으로 읽지 않는다. source 캐시는 수정하지 않는다.
- raw magic/치수/정확한 길이, native PNG IHDR/IDAT/IEND·CRC·치수 검증.
  PNG의 deflate를 이중 decode하지는 않는다. 최종 decoder 오류 처리도 소비자의
  책임이다. `Frame.bytes`는 원본 payload, `Frame.fields`는 경로를 뺀 native
  telemetry다. sum/max/wall 집계는 상위 서비스에서 의미별로 수행한다.
- I/O queue 각 8개 line(최대 64KiB), 미완료 render 최대 32개, stderr tail
  8KiB. 기본 frame 최대 16Mpx/80MiB. Unix 별도 process group으로 터미널
  foreground SIGINT와 분리. 종료 유예 뒤 필요 시 소유 child kill/reap.

## 호출 계약 / 아직 보장하지 않는 것

`spawn/open/set_styles`는 deadline이 있는 동기 메서드다. UI/HTTP loop에서 직접
호출하지 말고 전용 controller thread에서 실행한다. `render/cancel`은 bounded
송신 큐가 차면 `Busy`이며 **실패한 요청은 제출된 것이 아니다**. 이벤트를
drain하고 최신 목표 상태를 다시 제출한다. 성공한 cancel은 그 시점 이후
poll에서 이전 frame을 내보내지 않는다.

M1a-2b의 선택적 `Config.shutdown_requested`(공유 AtomicUsize, nonzero)는
ready/open/style 및 poll 대기를 약 20ms 간격으로 확인해 `Cancelled`로 종료한다.
종료 유예/kill/reap 시간은 별도다. 이 flag는 프로세스 전체 종료용이며 세대별
supersede는 계속 `cancel()`을 사용한다. flag 미지정인 기존 호출자는 그대로다.

controller는 연결된 브라우저 속도와 무관하게 **계속 `poll()`**해야 한다.
poll은 frame 파일 소비와 render deadline 판정을 수행한다. 브라우저를 기다리며
poll을 멈추면 내부 line queue가 유계여도 기존 renderd의 `round_paths=1`
게시 파일/daemon 내부 response queue까지 유계가 되는 것은 아니다. M1b의
latest-frame mailbox/WS credit은 이 controller의 downstream에 둔다.
`app-core/view`의 M1b-2a controller는 이 독립 poll과 취소 후 drain deadline을
구현했다. WS credit 및 서버 전체 admission이 구현되었다고 보지는 않는다.

`pending_generations()`는 취소한 세대도 terminal 응답/파일 소비까지 센다.
`CancelAcknowledged`만 받고 0이라고 가정하지 않는다. M1b controller는 이 값이
0일 때만 다음 렌더/스타일 변경을 제출한다.

style 변경은 idle 또는 cancel 이후에 가능하고 ack까지 기다린다. 남아 있는
stale frame은 이 대기 중에도 회수한다. `poll(None 결과)`는 대기 시간 만료일
뿐 final이 아니다. `Frame::complete()`는 native frame의 partial/deferred/
label 잘림 검사이고, 별도 jobdeck source skip ledger까지 판정하지 않는다.

직접 실행하는 신뢰된 renderd가 대상이며 임의 프로세스 sandbox가 아니다.
실행 파일이 자손 프로세스에 pipe를 넘기는 wrapper는 범위 밖이다. close/Drop은
daemon 자체를 수거한다. 브라우저에서 binary/argv/path/env를 받는 API는 없다.
새 인덱스 revision은 기존 worker 재open 대신 close→새 worker로 전환한다.
M0-D4의 scene ID 확장은 M4a-1, 표시 anchor·query 취소 연결은 M4a-2에서 추가했다.
native wire는 외부에 직접 공개하지 않는다. M4a-3의 owner WebSocket은 상위
controller/표시 receipt와 allowlisted DTO를 거치는 별도 경계다.

## M4a-1: native pick/snap

`Frame::query_scene()`으로 실제 게시 geometry의 generation/round·완료·요약 수를
읽는다. label-only foreground는 이전 margin scene을 재사용하므로 frame의
generation을 대신 넣으면 안 된다. `WorkerClient::query(QueryRequest)`는 기대 scene,
DBU x/y/radius, layers, `Snap|Pick{nth}`를 받고 새 seq를 반환한다.
`poll()`의 `Event::Query(QueryReply)`에 원래 요청·actual scene·typed 결과가 온다.
ID는 worker-local이며 상위 controller가 dataset/view/worker epoch와 표시 상태를
검사해야 한다. 자세한 wire·제한은 [M4 기록](../../docs/WEBUI_M4.ko.md)을 따른다.

최대8개 outstanding query, 추가로 종류별 취소 ACK 각1개, IO line64KiB, outline512점이다. `points_truncated`이면
prefix를 완전한 polygon으로 해석하지 않는다. query seq는 양의 i64 범위에서
성공한 제출만 증가하고 render frontier와 독립이다. query 중 render/cancel/poll은
가능하며 style ACK 전에는 query를 drain한다. default query deadline5초는 frame이나
다른 질의 응답으로 연장되지 않고 timeout/protocol 오류는 close/reap한다.
summary/미완료/mismatch를 빈 hit와 구별한다. 혼합 scene의 exact 레이어만 지정하면
질의할 수 있지만 전체 scene의 `queryable()`은 false다. 모든 render/query 응답을
독립적으로 소비해야 하며 frame만 골라 다른 이벤트를 버리는 capture loop에 query를
동시에 제출하지 않는다. `RenderSession::capture`는 미소비 query/취소 ACK가 있으면
worker를 닫지 않고 Busy로 거부한다. deck query와 브라우저 조작 UI는 여전히 미지원이다.

`cancel_queries(QueryKind::Snap|Pick)`는 그 종류의 frontier만 올린다. 성공한 송신은
query와 같은 seq 시계를 한 칸 사용한다. render generation/다른 종류에는 영향이 없다.
취소 ACK(`Event::QueryCancelAcknowledged`)는 실제 query terminal을 대신하지 않는다.
`pending_queries()`는 **query+취소 ACK**를 합산하고 동기 style 전에는 모두 drain해야 한다.
취소 ACK에도 제출 기준5초 절대 deadline과 미발급/중복/잘못된 frontier 검사를 적용한다.
종류별 기존 ACK 미소비나 bounded 송신 큐 포화는 Busy이며 seq를 소비하지 않는다.
실행이 이미 끝난 질의는 취소와 경합해 정상 응답할 수 있으므로 상위 anchor 검사는 필수다.

M4a-2의 `app-core/view`는 표시 frame/worker epoch·revision을 검증하고 종류별 latest-only
입력·결과와 최대4개 native 미완료 질의를 보관한다. margin crop/라벨-only의 geometry
scene을 재사용하며 style 변경 때 query·취소 ACK를 먼저 소비한다. 이것은 로컬 도메인
연결이다. M4a-3의 `web/query`가 owner/view/connection·표시 ACK를 추가 검증하고
`cancel_query_if_current(kind,id)`로 오래된 연결이 다른 연결의 새 질의를 취소하지
않게 한다. query 자체가 새 worker나 별도 geometry decode를 만들지는 않는다.

`tools/validate_worker_queries.py`는 합성 OASIS/점유 혼합·KLayout 면적, margin/라벨
재사용·가시성 전환·overlap/긴 outline·query/render/종류별 취소 교차와 실제 controller의
crop·worker 격리를 PATH-empty native로 검사한다. M4a-3의 실제 owner WebSocket
pick/snap·표시/연결 거부·요약 subset·재접속도 함께 검사한다. full battery가
실행하며 fixture 누락은 skip이 아니다.

## 검증

```sh
cd rust
cargo test --offline -p floe-worker-client
cargo clippy --offline -p floe-worker-client --all-targets --no-deps -- -D warnings
cargo fmt -p floe-worker-client -- --check
```

`tests/lifecycle.rs`는 테스트 실행 파일 자체를 fake daemon으로 쓴다. 잘못된
버전/UTF-8/중복·과대 line/EOF/ready·open·render timeout, style/open 실패,
손상 raw/PNG·경로 탈출·symlink·과대 frame, stale/취소, stderr 폭주/막힌 stdin,
cleanup과 덱 capability를 검사한다. 이 테스트에는 Python이 필요 없다.

실제 daemon/기존 Python adapter 오라클 대조(개발 환경에만 Python 필요):

```sh
# 저장소 루트, release renderd와 이미 색인한 다중 페이지 synthetic source 사용
sh tools/validate_worker_client.sh /path/to/valmini.oas /path/to/valmini.oas.floe
```

단일 PNG 바이트 동일성, raw/PNG 픽셀, nonempty 단언, depth/thin/라벨/폰트,
style/revisit, cold worker의 다중 refinement round, 20세대 취소를 검사한다.
valmini처럼 여러 페이지를 선택하는 fixture가 필요하다. real_worker 테스트는
기본 `cargo test`에서 ignored지만 이 스크립트에서는 필수 실행이며 missing
fixture/oracle은 hard fail이다. 전체 `tools/validate_rust.sh`에도 연결되어 있다.
다른 Python 경로는 개발용 `FLOE_TEST_PYTHON`으로 지정한다.
