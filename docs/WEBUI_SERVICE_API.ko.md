# 웹 전환 — Rust 서비스/API 설계 초안

작성 2026-09-12, 기준 `feature/webui@6c33a48`.
[상위 계획](WEBUI_PLAN.ko.md) · [M0 기능 대조표](WEBUI_M0.ko.md).

**아래 endpoint/message는 전체 서비스 설계안이다.** M1b-1의 일부 transport API
(exchange/capabilities/logout/WS ping)는 [M1b 기록](WEBUI_M1B.ko.md)에 명세/구현했다.
아래 전체 URI가 그대로 구현된 것은 아니며 공유 API는 아직 없다.
M2a의 현재 DRC 등록/읽기 URI·페이지·취소·focus/in_view 계약은 [M2 기록](WEBUI_M2.ko.md) §2~3이
기준이다. read-only actor만 있고 review 저장/공유 endpoint는 아직 없다.
M1b-2a의 `app-core/managed`·`view`에는 process-local lease/admission과
독립 worker controller를 구현했다. M1b-2b의 사전 등록 view용 제어/이미지
스트림은 [M1b 기록 §6](WEBUI_M1B.ko.md#6-m1b-2b--인증된-제어이미지-스트림)을 따른다.
M1b-2c1의 등록 scope/index supervisor는 M1b 기록 §7, M1b-2c2의 owner
catalog/open/index 작업 API는 §8을 따른다(통합 `/operations` seq 계약).
M1b-3의 실행/정적 자산·startup과 브라우저 계약은 §9를 따른다. 단일 layer
checkbox는 `view.set`의 `layer_change:{pair:[layer_u32,datatype_u32],visible:bool}`로
처리하고 서버에서 head/child 의미를 적용한다. `layers` 전체 선택과 같은 요청이면
전체 선택을 먼저 적용한다. 잘못된 pair나 선택 수 상한은 오류이며 조용히 자르지 않는다.
2026-09-13 M1a-1 `rust/worker-client`와 M1a-2a/b `app/app-core`의 일반 index·info/단일 render/probe를
구현했다. 현재 호출 계약은 [worker README](../rust/worker-client/README.md),
[M1a 기록](WEBUI_M1A.ko.md)을 따르며 아래 서비스 전체가 존재하는 것은 아니다.
M1 구현 때 명세/테스트를 함께 고정한다. 렌더/인덱스 포맷을 새로 만들거나
Python gateway를 중간 단계로 두지 않는다. 현 jobdeck 실측과 별도 분기다.

## 1. 실행 경계

```text
Rust CLI (floe2-web: 이관 중 개발용 이름)
  └─ floe-app-core: CLI와 웹이 공유하는 기능/정책
       ├─ cache/index/jobdeck/DRC/export 도메인
       ├─ floe-worker-client ── 기존 floe-renderd 프로세스
       └─ 기존 floe-index (색인/프로파일/occupancy/DRC pack)

브라우저 HTML/Canvas
  └─ HTTP/WS ── floe-webd (인증/DTO/세션/전송)
                    └─ 같은 floe-app-core
```

제안 경로: `rust/app-core`, `rust/worker-client`, `rust/app`, `rust/web`.
jobdeck/DRC/export는 처음에는 app-core 모듈로 두고 독립 crate가 필요한 경우만
분리한다. 기존 `rust/cli`의 **floe-index 이름/기능은 유지**한다.
기존 VFS/기하/occupancy 알고리즘은 재사용하고 Python만 담당하던 orchestration과
reader/정책을 이관한다. 복제 구현이나 `python -m floe ...` fallback은 금지.

- CLI는 headless 명령에 HTTP 서버·브라우저가 필요하지 않다. app-core는
  HTTP Request/Response, DOM, GTK를 모른다. webd만 인증된 DTO를 변환한다.
- renderd의 stdin/stdout은 내부 프로세스 경계다. 이를 외부 WS로 그대로
  노출하지 않는다. daemon 경로/argv/env/출력 위치는 서비스만 지정한다.
- 렌더/색인 CPU 작업은 기존 native worker에 둔다. HTTP handler에서 긴 parse,
  OVP read, PNG 처리, blocking process wait를 직접 수행하지 않는다.
- M4 전까지 기존 Python `floe2` launcher/GTK를 유지. Rust CLI의 미구현 명령은
  명확히 오류를 내고 지원 범위를 출력한다. 배포 기본 이름 변경은 별도 수용 판정.

### 1.1 HTTP/WS 의존성 후보와 도입 게이트

선정은 **Axum + Tokio + Serde/serde_json**. Axum의 HTTP routing과
`ws` feature를 사용하고 RFC6455를 직접 구현하지 않는다. Tokio는 transport와
프로세스 I/O에 한정하고 기존 동기 도메인과 경계를 둔다.
근거: [Axum 문서](https://docs.rs/axum/latest/axum/),
[WS 모듈](https://docs.rs/axum/latest/axum/extract/ws/index.html),
[Tokio의 동기/비동기 연결](https://tokio.rs/tokio/topics/bridging).

M0의 vendor 12개에 M1a-2a에서 Serde/JSON/Unix signal용 16개,
M1b-1에서 네트워크 stack용 59개를 추가했다. 버전·feature·라이선스·보안·
offline/MSRV 결과는 [M1b 기록 §3](WEBUI_M1B.ko.md#3-httpws-의존성-게이트).
의존성 갱신 때 다음 게이트를 계속 적용한다:

1. 최소 feature 집합(HTTP/1, WS, JSON, 필요한 process/io/time/sync)을 시험하고
   정확한 버전·MSRV·전이 의존성을 lock에 고정. 무심코 `full`을 켜지 않는다.
2. MAC/Linux 실제 target, portable GLIBC 하한, offline release/test를 검사.
   TLS 종료를 내장할지 배포 proxy로 할지도 원격 배포 전에 결정한다.
3. 라이선스 고지·의존성 보안 점검·패키지 크기와 build 시간을 기록.
4. vendor는 수작업 수정하지 않고 승인된 의존성 갱신 절차로 생성/검토.
   Cargo의 [vendor/source replacement](https://doc.rust-lang.org/cargo/commands/cargo-vendor.html)
   방식과 기존 `.cargo/config.toml`을 정합하게 유지한다.

M1b-1의 Cargo.toml/lock/vendor와 transport 검증은 구현됐다. 원격 배포/TLS,
실제 Linux/Firefox/ETX 수용 검증까지 완료했다는 뜻은 아니다.

## 2. 서비스의 책임과 자료형

| 경계 | 입력 → 결과 | 소유하는 것 |
|---|---|---|
| CatalogService | 허가된 source/deck handle → metadata, level 목록 | 소스 경로 해석·등록, layer 별칭/DBU, 파일 접근 범위 |
| IndexService | IndexOptions + source handles → JobId/진행/결과 | freshness/force/LOD/occupancy/profile, source dedup, 쓰기 lease, 취소·임시파일 |
| DeckService | parsed deck + 선택 + source revisions → DeckSnapshot | strict/skip ledger, 좌표 변환, 전역 ordinal/color, level→TC 행, composite spec |
| ViewService | OpenOptions / ViewPatch → Snapshot/RenderTicket | viewport/정책/가시성/스타일, 변경 순서, worker lease, render generation |
| WorkerClient | validated Render/Query/Clip → typed Event | handshake·wire·process lifecycle·원자적 publish 파일 소비 |
| DrcService | pack/rule/error handle + reviewer scope → 조회/수정 | lazy 읽기·identity·waive/note 충돌/저장·import/export |
| ExportService | Shot/Clip/Metadata 요청 → ArtifactId | batch/mosaic/PNG/OASIS/metadata, 성공 시 게시·실패 시 정리 |

정책 값은 enum/검증된 타입으로 표현한다: `ThinPolicy{Auto,Keep,Cull}`와
`EffectiveThin{Keep,Cull}`, `Depth{Full,Levels(n)}`, detail, visibility의
`All/None/Explicit`, `FrameFormat{Png,RawV1}`. null/빈 배열/필드 생략을 같은
값으로 뭉개지 않는다. CLI `--thin` 미지정과 명시 auto도 구별한다.

좌표는 내부에서 기존 DBU/checked geometry 계약을 보존한다. wire/JSON에서는
큰 정수(DBU/i64, ID/u64)를 **10진 문자열**로 전달해 JS Number 반올림을 피한다.
서브-DBU view 경계는 유한 f64의 round-trip 문자열을 사용한다. 무한/NaN,
역전/0 면적 bbox, 음수 반경, 치수 곱 overflow를 자원 할당 전에 거부한다.
DBU와 µm를 같은 필드에 혼용하지 않고 source/deck 좌표계 ID를 함께 둔다.
브라우저는 화면 상대 좌표로 입력하고 서버가 권위 있는 world 좌표를 계산한다.

### 2.1 capability (현재 코드가 기준)

| 기능 | layout | jobdeck |
|---|---|---|
| thin auto | cull | keep |
| PNG/raw, style, depth/detail | 지원 | 지원(virtual layer 매핑) |
| 착지 margin/pan prefetch | 지원 | 미지원 |
| label size / labels | 지원 | 현재 미지원 |
| pick/snap/clip | layout 지원, scene 상태에 따른 제한 | 현재 미지원 |
| hierarchy frames | 지원 | 지원(source별 frames-only pass 합성) |
| abstract / 폐기된 OVC coverage | 미지원 | 미지원 |
| OVO occupancy | 조건 충족 시 summary | source/pass 조건 충족 시 summary |

덱의 frames 지원은 `DeckRenderRequest.frames`와 frames-only pass 구현을
기준으로 한다. `jobdeck/render.py`/`run_deck_render`의 오래된 소개 주석에는
아직 미지원으로 적혀 있으므로 주석만으로 capability를 결정하지 않는다.
capability는 모드뿐 아니라 현재 scene에 따라 좁아질 수 있다. occupancy summary를
원본 polygon의 질의 가능 geometry로 가장하지 않는다. UI를 비활성화하는 것과
서버가 unsupported 명령을 거부하는 것을 모두 구현한다. `--lod`/refinement의
현재 옵션 불일치는 [M0-D1/D2](WEBUI_M0.ko.md#4-조사에서-드러난-이관-결정-항목)
해결 전 새로운 유효 기능으로 광고하지 않는다.

## 3. 서로 다른 identity와 세션

| 값 | 수명/의미 |
|---|---|
| source_id / dataset_id | 등록된 소스/덱. 경로나 사용자 파일 이름을 인증키로 쓰지 않음 |
| index_revision / dataset_revision | 일관된 캐시 산출물 집합; 덱은 spec/선택/소스 revision vector까지 포함 |
| view_id | 독립 탐색 상태·worker의 단위 |
| connection_epoch + request_seq | 연결 재생성과 입력 순서를 구별. 재접속 후 이전 seq와 충돌 방지 |
| state_rev | 서버가 수락한 전체 복원 상태 변경 순번 |
| render_state_key | source revision, depth/detail/effective thin/visibility/style/font 등 픽셀 정책. pan 위치만 달라도 같은 key 가능 |
| worker_epoch + generation + round | daemon 재시작과 foreground/margin 렌더/라운드를 구별 |
| drc_revision + review_rev | geometry 캐시와 별개인 DRC 내용/waive·note 변경 |

frame의 원인 state_rev를 기록하지만 note/선택만 바뀌었다는 이유로 geometry를
무조건 다시 그리지 않는다. foreground 응답은 현재 입력과 render_state_key를
검사한다. margin은 이전 요청에서 온 것이어도 동일 key/배율/위상·포함 bbox가
확인된 경우만 임시 표시용으로 재사용한다. 최신 foreground 번호와 같아야 한다는
규칙을 margin에 그대로 적용하면 prefetch 이득이 사라진다.

- 따라보기: owner view의 동일 프레임/상태를 구독, viewport 변경 권한 없음.
- 독립 탐색: 데이터에 read 권한을 가진 별도 view/worker. 데이터 쓰기는
  허용하지 않되 자신의 viewport 조작은 가능. "read-only"를 모든 입력 금지와
  혼동하지 않는다. 세션·CPU·메모리 admission 후에만 생성.
- reconnect는 인증 재검사 후 snapshot+현재 상태에 맞는 최신 final을 보낸다.
  이전 상태의 final밖에 없으면 stale 표시용이라는 사실을 밝히거나 새 렌더를
  기다린다. 과거 프레임/명령 로그 전체를 재생하지 않는다.
- linger는 bounded TTL/LRU. 마지막 subscriber 종료와 명시 close를 구별하고,
  close/TTL/worker crash에서 query·frame·파일·lease를 회수한다.

## 4. HTTP/WS 계약 초안

URI와 payload 필드는 M1에서 schema로 고정할 제안이다. browser에 서버 절대경로,
renderd argv, 임의 환경변수, 임의 출력 경로를 받는 endpoint는 없다.

| API 후보 | 권한/결과 |
|---|---|
| `GET /api/v1/capabilities` | 인증 후 protocol/build/capability/치수·전송 상한 |
| `GET /api/v1/catalog/{handle}` | 허가된 디렉터리 handle의 제한된 목록; 게스트는 허가된 dataset만 |
| `POST /api/v1/views` | dataset handle + startup options를 한 transaction으로 적용, view snapshot |
| `GET /api/v1/views/{id}` | 현재 상태, worker/index 상태, 재동기화 snapshot |
| `DELETE /api/v1/views/{id}` | view owner가 명시 종료 |
| `POST /api/v1/index-jobs` | index 권한 + 명시 옵션/force, 비동기 JobId; open이 몰래 overwrite하지 않음 |
| `GET /api/v1/jobs/{id}`, `POST /api/v1/jobs/{id}/cancel` | 자기/허가된 job만 상태/취소 |
| `POST /api/v1/exports`, `GET /api/v1/artifacts/{id}` | export 권한과 결과 download 범위. 임의 파일 서빙 아님 |
| `POST /api/v1/shares` | owner가 view/dataset·역할·만료를 제한해 발급/폐기(M2) |
| `GET /api/v1/views/{id}/events` (WS upgrade) | 인증/Origin/프로토콜 확인 후 명령·이벤트·binary frame |

로컬 CLI의 자유로운 파일 경로는 **trusted local boundary**에만 허용한다.
웹의 파일 선택은 서버 catalog handle을 사용한다. 브라우저 local file upload와
서버 파일 열기는 다르며, 업로드 지원을 M1에 암묵적으로 넣지 않는다.
DRC/설정 import 역시 등록된 입력/artifact로 처리하고 include 탈출을 막는다.

WS control envelope 예(설계용):

```json
{
  "protocol": 1,
  "type": "view.set",
  "view_id": "v-example",
  "connection_epoch": "c-example",
  "request_seq": "17",
  "base_state_rev": "42",
  "body": {"thin": "keep", "depth": "full"}
}
```

- 서버가 검증/권한/상태 충돌을 먼저 판정하고 `accepted`와 정규화한 snapshot을
  응답한다. 여러 control을 한 번에 적용한 뒤 render 1회를 예약한다.
- 연속 pan/zoom은 누적 상대 delta를 무작정 drop하지 않는다. 클라이언트가
  최신 목표 viewport로 합치거나 서버가 순서대로 state를 반영한 뒤 **렌더만**
  합친다. 창 열기+goto+detail+depth도 이 원칙으로 초기 레이스 방지.
- view/style 등 최종 상태로 대체 가능한 작업만 coalesce. waive/note/import/
  index/export/close는 ack/operation ID가 있는 별도 bounded queue다.
  취소/종료가 프레임 전송 완료 뒤로 밀리지 않도록 read/control loop를 분리.
- 오래된 base_state_rev는 conflict+snapshot. 무한 재시도/묵시적 last-writer-wins
  금지. 지속적 파일 쓰기는 operation ID와 예상 revision을 검증하고, 재접속 후
  완료 여부를 먼저 조회해 불명확한 명령을 자동 재실행하지 않는다.

### 4.1 frame: 원자적 봉투, credit 기반 전송

binary WS message 하나를 `u32le header_length + UTF-8 JSON header + payload`로
구성한다. header는 최대 64KiB 제안, frame 전체 한도는 handshake에서 고정.
text 헤더와 별도 binary가 우연히 짝지어질 것이라고 가정하지 않는다.

header 필수: view/connection/request identity, dataset_revision,
render_state_key, worker_epoch/gen/round, bbox(DBU), width/height,
format/payload_length, foreground|margin, final/partial/deferred,
labels_truncated, summary 사용 상태, query capability, phase별 perf.
payload는 PNG 또는 기존 `FLOERAW1` 전체(magic+u32le w/h+RGBA)다.

- 길이/치수/format/overflow를 server와 client 모두 검사. raw는 정확히
  `16 + 4*w*h` 바이트, PNG도 선언 치수와 실제 치수/해제 메모리를 제한한다.
  서버 파일 경로는 봉투에 포함하지 않는다.
- final은 **해당 렌더가 끝났음**, complete와 동의어가 아니다.
  `final=1, partial=1`도 표시 가능하지만 export 성공/완전 결과로 보지 않는다.
  page deferred, labels truncated, deck skip ledger, summary approximation을
  구별한다. refinement 수치가 0이라는 이유로 완전하다고 판정하지 않는다.
- subscriber별 **미확인 frame 1개 + 대기 최신 frame 1개**, 바이트 cap도 적용.
  대기 slot은 최신으로 교체. client가 display 또는 discard를 완료하고
  `frame.ack(frame_id, disposition)`를 보내야 다음 credit이 생긴다.
  socket write 완료만 credit으로 쓰면 browser의 수신 큐를 제한하지 못한다.
- ack timeout/느린 guest는 그 연결만 정리. subscriber 수/총 송신량도 제한.
  shared payload Arc를 이용하되 TCP/JS/Canvas 복사 메모리는 별도로 계산한다.
- 수신 시와 PNG decode 완료 시 최신 여부를 재검사. stale decode가 늦게
  완료돼 최신 그림 위에 덮는 것을 금지. frame/event callback도 connection_epoch 검사.
- 새 요청을 받았을 때 이미 전송된 구 frame을 회수할 수는 없다.
  그 경우 client stale discard가 원자성을 완성한다. reconnect에는 제한된
  snapshot/최신 frame만 보내고 끊기기 전 큐를 복구하지 않는다.

### 4.2 프레임 표시와 pan

서버가 확정한 raster 크기와 CSS 크기/DPR mapping을 분리한다. 성능 비교는
동일 raster W×H로 하고 DPR 때문에 4배 픽셀을 그린 결과를 GTK와 비교하지 않는다.
resize/DPR 변경은 치수·배율 재협상, 오래된 decode/margin의 적합성을 재검사한다.

일반 레이아웃은 기존 `_render_key`, `_covered`, `_margin_frame`, fill phase
스냅의 동작을 이식한다. 착지 geometry base + 적합한 label frame을 합성하고,
margin crop 불가/labels_truncated에서는 이전 화면을 "현재 완성"으로 표시하지
않는다. 새 영역은 async fast path로 채우며 확대/위상 불일치 때 재사용하지 않는다.
덱은 capability가 false라 이 최적화를 가정하지 않는다. layer 색·fill·thin 변경
후 과거 margin이 잠깐 다른 픽셀 정책을 보여 주는 것도 stale 오류로 센다.

## 5. Rust worker client — M1a 우선 구현

1. private workspace 생성 → native 바이너리 검증 → `ready` → `open` →
   `style` ack → render 순서. 인덱서/daemon/제품/웹 protocol 버전을 구별한다.
   현재 daemon 호환 기준은 `RENDERD_VERSION`이며 Python 제품 버전 숫자와
   단순 동일 비교하지 않는다. 새 wire 변경 시 daemon 호환 버전도 갱신.
2. 공백 없는 내부 경로를 쓰는 현 wire 제약을 adapter에서 흡수. 사용자의
   공백/한글 원본·출력 경로를 지원하고 shell 문자열 실행은 사용하지 않는다.
3. stdout strict parser / stderr 별도 drain. UTF-8/과대 line/잘못된 hex/
   알 수 없는 응답/EOF/버전 불일치는 명시 오류. stderr 무제한 저장 금지.
4. gen 증가/cancel을 직렬화하되 I/O 대기 동안 전체 상태 락을 잡지 않는다.
   foreground가 우선, margin은 foreground idle 때만 제출하고 새 입력에 취소.
5. `round_paths=1`의 publish 파일을 검증→read→unlink. stale/cancel/실패/종료
   경로도 client가 소유한 파일만 정리. 예상 디렉터리 밖 경로·symlink를 따라
   임의 파일을 읽거나 지우지 않는다. style 파일도 ack 후 수명 관리.
6. open/render/clip에 deadline, cancellation, worker crash를 구별. 초과 시
   EOF/정상 quit 유예 후 필요할 때만 소유 프로세스 종료·reap. 터미널 SIGINT가
   무관한 뷰 worker를 죽이지 않게 프로세스 그룹/부모 종료 계약을 시험.
7. 동일한 typed event를 CLI와 gateway가 소비. backend budget 오류를 cancelled로
   바꾸거나 빈 frame 성공으로 바꾸지 않는다. perf 합/최대/wall 구분 유지.

### 5.1 query는 현재 wire를 그대로 중계하면 부족하다

현재 snap/pick request와 response에는 `seq`는 있지만 조회 scene의 gen/round는
없다. render thread는 query와 별개로 published scene을 교체할 수 있고 margin도
scene을 바꾼다. gateway에서 "지금 표시한 gen"을 응답에 붙이는 것만으로는
어느 scene을 실제 조회했는지 증명할 수 없다.

M4 query 개방 전 내부 wire 확장: 요청에 expected scene ID, scene Arc를 잡는
시점에 그 ID 확인, 응답에 actual ID. ID는 최소 worker epoch의 gen/round와
렌더 상태를 식별해야 한다. 다르면 `stale_scene`/`query_unavailable`, 재시도는
현재 화면과 합치될 때만 한다. 원본 geometry 없는 summary-only/지원하지 않는
deck에서 "찾지 못함"으로 위장하지 않는다. 확장 전 웹 capability는 false.

## 6. 인덱스 revision — 제안 비교, hot reload 미구현

mtime/size는 현재 freshness에 필요한 정보지만 일관된 다중 파일 transaction ID는
아니다. OVM/OVP/OVT/OVO를 다른 시점 파일과 혼합하지 않아야 한다.

| 안 | 장점 | 제약 |
|---|---|---|
| 현 위치 캐시 + source별 read/write lease | 초기 구현이 작고 현재 cache layout 재사용 | 열린 세션이 있으면 관리형 재색인을 대기/거부. 외부 legacy indexer에는 lock 강제 불가 |
| 불변 revision 디렉터리 + 원자적 manifest 전환 | 기존 세션 pin, 새 세션 새 revision, 참조 종료 후 회수 가능 | 추가 디스크·게시/회수/실패 복구 설계와 indexer 출력 경로 검증 필요 |
| mtime 감지 후 파일별 즉시 재열기 | 겉보기 단순 | 혼합 세대/열린 mmap/GUI frame 불일치. 채택하지 않음 |

M1 로컬 실험은 **색인 완료 후 열기, 열려 있는 동안 외부 교체 없음**을 전제로
한다. 서비스가 실행하는 index는 모든 관련 view/worker lease 확인 후 진행한다.
old GTK/외부 CLI가 같은 캐시를 교체하는 경우까지 보호했다고 주장하지 않는다.
관리형 서버의 무중단 재색인에는 불변 revision+manifest 안을 우선 제안하되,
저장량/보존/운영 정책 승인 전 구현 범위로 확정하지 않는다.

덱 snapshot은 각 source의 revision과 선택/좌표/spec identity를 묶는다.
`.ovo` 추가도 다른 표시 결과를 만들므로 표시 revision 변경에 포함한다.
명시 reopen/전환 때 frame/margin/retained/query를 함께 무효화, worker를 새로 열고
완성 snapshot을 교체한다. 사용 중 파일을 삭제하지 않는다. 원본 OASIS 전체 hash를
open마다 계산하는 방식으로 대형 파일 비용을 늘리지 않는다.

사용자가 미룬 열린 GUI `.ovo` 자동 감지는 이 설계만으로 해결 처리하지 않는다.
revision의 디스크 형식/보존량/무중단 업데이트는 별도 설계 판정 항목이다.

## 7. 공유 서버 자원 — 로컬 예약 구현과 운영 제안

M1b-2a는 같은 Resources 객체의 worker/index 예약과 cache lease를 구현했다.
실제 기본값·적용 범위는 [M1b 기록 §5](WEBUI_M1B.ko.md#5-m1b-2a--관리형-캐시자원과-view-controller).
아래의 server-wide 감독·현장 응답성 정책까지 구현/검증한 것은 아니다.

**gateway별 semaphore는 서버 전체 제한이 아니다.** launcher마다 gateway가
생기면 각각의 한도가 곱해진다. 배포 B/C는 한 공용 supervisor/admission service
또는 관리자가 설정한 OS 자원 제약으로 전체 합을 통제해야 한다. A의 로컬
gateway limiter는 그 프로세스 자식에만 효력이 있음을 표시한다.

- index 기본 12를 유지하고 **관리형 index jobs 상한 16**을 초기 제안으로
  삼는다. standalone `--jobs` 호환과 서버 admission은 분리; 상한 초과는
  대기/명시 거부/합의된 축소를 알리고 조용히 다른 jobs로 측정하지 않는다.
- index source 동시 수, parse/split/LOD/occupancy의 pool, deck pass 병렬 ×
  pass 내부 raster/decode 병렬, 여러 view를 모두 회계한다. `jobs=4`가
  server-wide 4 thread라는 뜻은 아니다. 제어/heartbeat 스레드와 CPU worker 수
  구별. 단계별 실제 사용량을 계측해 중첩 pool의 예약 규칙을 검증한다.
- foreground render에 예약분, margin/linger는 가장 낮은 우선순위.
  색인 시작 시 렌더 예약분을 침범하지 않는다. 실행 중 비취소 구간을 semaphore가
  즉시 선점한다고 가정하지 않는다. 다중 사용자 실측 전 응답성 보장 수치를 약속하지 않는다.
- decoded LRU(현재 1GiB/worker), generation scene, retained(별도 256MiB/worker),
  OVO mmap/resident, deck pass/batch, raw margin, WS/Canvas copies를 합산한다.
  LRU budget만으로 RSS hard cap을 보장하지 않는다. managed worker 수·전체
  메모리 예약과 필요 시 OS cap을 함께 사용하고 OOM 오류 경로도 시험한다.
- foreground latest pending 1, margin pending 최대 1, index/export/query/DRC
  각각 제한된 queue. 무제한 `spawn_blocking`/독립 스레드 생성 금지.
- linger 기본값/총 worker 상한/동시 index 수/전체 메모리 한도는 현장 결정.
  임시 기본치를 모든 서버 메모리의 일정 비율로 자동 확대하지 않는다.

성능 실험은 index jobs 1/4/8/12/16 + 활성 viewer를 조합하고, 입력→표시/settle
p50/p95, 실행/대기 시간, RSS peak, I/O, thread 사용을 측정한다. render의
decode_jobs와 raster_jobs도 별도로 1/4/8 비교. 새 구조가 더 빠르다는 결론은
고정 W×H·detail/depth/thin·요약 여부·cold/warm 조건의 실측 후에만 낸다.

## 8. 보안·DRC 쓰기 경계

- loopback도 인증과 Host/Origin 검증. 외부는 HTTPS/WSS. random 단기 bootstrap
  토큰을 세션 credential로 교환하고 URL/로그에서 제거; 만료/revoke가 열린 WS에도
  적용. 외부 쿠키는 Secure/HttpOnly/SameSite 및 CSRF 방어. loopback credential
  전달 방식과 Firefox 버전 적합성은 M1/현장 gate에서 검증한다.
- owner, follow, independent-read, DRC-edit, index/export 권한을 구별한다.
  같은 TeeBox OS 계정 내 프로세스 격리가 보안 경계가 된다고 가정하지 않는다.
  같은 UID의 악성 프로세스/관리자까지 격리하려면 별도 계정/배포 격리가 필요하다.
- 허가된 directory/dataset handle을 서버에서 resolve. path traversal·symlink
  탈출·TOCTOU·개별 source 권한을 검사. TC/SVRF include 경로도 예외가 아니다.
  공유 token은 원래 scope 밖 경로·DRC pack·artifact를 열 수 없다.
- reviewer는 인증 사용자에 묶인 표시/저장 scope. `--floe-reviewer` 문자열로
  다른 사람의 waive를 수정하지 못하게 한다. pack/source identity + review_rev
  비교 후 원자적 저장. 다중 editor 충돌은 409/conflict, 조용한 덮어쓰기 금지.
- CLI SVRF 환경 분기는 기존 호환을 시험하되 원격 parser에는 명시 define과
  승인된 include root만 제공. 서버 전체 환경변수를 deck에 노출하거나 Tcl 실행 금지.
- error/note/cell/layer 이름은 신뢰하지 않는 텍스트. HTML 삽입 대신 text 처리.
  요청·메시지·row·frame·artifact 크기/속도 제한. 로그는 토큰/원본 경로/도형을
  기본 숨기고 diagnostic ID로 연결한다. 다운로드에도 권한·만료 적용.

공통 오류 DTO: code, 안전한 message, operation/request ID, retryable,
partial/completeness 정보. 주요 code는 invalid_request, unsupported,
stale_state/scene, permission_denied, index_stale/incomplete, budget_exceeded,
worker_version/crash/timeout, cancelled, io_error. ENOSPC와 superseded 구별.

## 9. 검증과 다음 결정

M1a gate: 가짜 daemon으로 malformed/UTF-8/oversize/EOF/timeout/version,
out-of-root publish, stale/cancel/round/file cleanup 시험 → 실제 valmini의
PNG/raw·thin·style·재방문과 기존 Python adapter 결과 대조. Python 없는 실행
환경에서 같은 검사를 수행하되 개발 오라클은 별도 프로세스로 사용 가능.

M1b gate: 이탈/재접속·100세대 입력·느린 guest·decode 순서 역전·rev 충돌·과대
치수·Origin/token/권한·margin 검정 strip·labels_truncated. ack 미수신에도
메모리/큐가 유계이며 owner 렌더가 계속되는지 검사. input→photon G1은 GUI 실측.

M2/M4 gate: DRC 대형 synthetic pack paging/읽기와 동시 waive/note 쓰기, export
재접속 중복 실행 방지, query scene identity, jobdeck skip/capability. 최종
Python-free 판정은 M0의 전체 대조표와 portable 검사 통과 후다.

미결: 의존성 lock/feature/MSRV, 브라우저/프론트 빌드 하한, CLI 무효 옵션 정리,
query wire 확장 세부, shared supervisor 배포, resource 숫자, revision 게시·회수,
원격 TLS/launcher bootstrap, 보조 명령 이름. 미결인 정책을 구현 완료로 표시하지 않는다.
