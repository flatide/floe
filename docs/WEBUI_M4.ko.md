# 웹 전환 M4 — 조작 parity 구현 기록

`feature/webui`. [상위 계획](WEBUI_PLAN.ko.md), [기능 대조표](WEBUI_M0.ko.md).
M2 공유 권한 추가와 실제 브라우저 pack-build 승인 클릭은 승인 대기이며,
M0/G2·M3 현장 Firefox/ETX는 사용자 요청대로 보류다. 이 경계를 우회하지 않고
독립적인 로컬 native 이관을 진행한다. M4 전체 완료나 GTK 은퇴를 뜻하지 않는다.
현재는 §10의 **DRC 오류별 PNG 캡처 CLI**까지 연결했다.
§1~9의 미연결 표기는 각 선행 단계 당시의 범위다. 웹 clip/나머지 내보내기와 전체 조작
수용은 남아 있다.

## 1. M4a-1: 표시 scene에 고정한 native pick/snap

M0-D4의 선행 조건을 `renderd`와 `floe-worker-client`에 구현했다.
HTTP/WS endpoint·인증/공유 권한·브라우저 UI는 추가하지 않았다.
**웹 query capability는 여전히 false**이며 CLI/UI의 pick/snap·수동 ruler/clip은
다음 연결 단계다. 도형 픽셀·인덱스 포맷·기본 cut/LOD 정책은 바꾸지 않는다.

### scene과 프레임의 구별

- 모든 frame에 `scene_gen`, `scene_round`, `scene_complete`, `scene_summary`를
  붙인다. 일반 렌더는 게시된 geometry scene의 generation/round이며, 라벨만
  다시 그린 foreground는 geometry를 제공한 이전 margin scene의 ID를 유지한다.
  따라서 frame `gen/round`를 query의 scene으로 추정하면 안 된다.
- 각 scene의 identity·완료 여부·요약 레이어 집합은 그 immutable `FrameScene`과
  함께 게시된다. query thread는 **한 번 캡처한 Arc**에서 판정·질의·응답한다.
  질의 도중 다른 frame이 게시돼도 응답의 actual identity가 뒤바뀌지 않는다.
  추가 scene history나 geometry decode/재렌더는 하지 않는다.
- `scene_complete`는 선택된 geometry의 partial/deferred 여부다. draw-only label
  잘림과 구별하며, cut/LOD/wash를 제거한 원본 전체의 정확성을 주장하지 않는다.
  요약이 없는 빈 plan도 유효한 scene이다. 덱은 query 미지원이므로 네 필드가0이다.
- identity는 **해당 native worker 수명 안에서만 유효**하다. 캐시/데이터셋 revision,
  view ID·worker epoch·실제 표시 frame ID·상태 revision은 상위 controller가 별도로
  고정해야 한다. 다른 worker의 동일 숫자는 같은 scene이 아니다.

### 요청과 응답

```text
snap seq=20 x=1000 y=2000 r=10 layers=7/0 scene_gen=8 scene_round=1
pick seq=21 x=1000 y=2000 r=3 nth=0 layers=7/0 scene_gen=8 scene_round=1
```

`scene_gen/scene_round`는 둘 다 지정한 양의 canonical u64다. 하나만 지정하거나
0·overflow·비정규 표기면 command 오류다. 기존 GTK처럼 둘 다 생략하는 로컬
client는 기존 bounded query 의미를 유지한다. 새로운 Rust client는 항상 지정한다.

응답은 seq·found·기존 결과에 actual scene 네 필드, `query_summary`(요청 레이어 중
요약 레이어 수), `query_status`를 포함한다.

| status | 의미 |
|---|---|
| `ok` | 요청 scene 일치, 선택 geometry 완료, 요청 레이어에 요약 없음. 이때만 found=0이 정상적인 빈 결과 |
| `scene_unavailable` | 아직 게시 scene 없음(덱은 client에서 제출 자체를 거부) |
| `scene_mismatch` | 현재 게시 scene의 generation 또는 round가 기대값과 다름 |
| `scene_incomplete` | 해당 scene에 partial/deferred geometry가 있음 |
| `scene_summary` | 요청이 원본 geometry를 대체한 요약 레이어를 포함함 |
| `superseded` | 같은 종류의 새 질의 또는 명시 취소가 기존 질의를 중단함 |
| `error` | member 안전 한계·geometry 산술·레이어 등 native 질의 오류 |

거부는 found=0과 `err_hex`를 함께 내고 정상 빈 결과와 구별한다. 요약+exact 혼합
화면의 `layers=all`은 summary 거부지만 exact 레이어만 명시하면 질의할 수 있다.
`Frame::query_scene().queryable()`은 **전체 scene**의 요약 없음 판정이며, 필터한
질의의 허용 여부는 실제 응답의 status/query_summary로 판정한다.

native의 기존 pick 후보64개·정수 면적/레이어 정렬·음수 nth 순환은 유지한다.
512점 초과 윤곽은 기존처럼 prefix만 반환하되 `points_truncated=1`을 명시한다.
이 prefix를 닫아 완전한 polygon으로 그리거나 측정하면 안 되며 bbox/면적은 전체
도형에서 구한 값이다. pinned snap은 조용히 최선 후보를 반환하던 shape cap 대신
기존 전역 member 안전 한계4,194,304의 명시 오류에 의존한다. legacy snap의
shape cap1,048,576은 그대로다. nearest 보장 범위는 선택된 게시 geometry다.

### Rust process client의 수명과 비용

- `WorkerClient::query(QueryRequest)`는 비동기·try-only 송신이며 최대8개 미완료
  질의만 허용한다. 성공한 송신 뒤에만 양의 seq(i64 범위)를 올린다. render 세대와
  query seq/대기열은 독립이고, 응답은 같은 `poll()`의 `Event::Query`로 받는다.
- `QueryReply`는 원래 요청·actual scene·status·typed snap/pick·진단을 보존한다.
  미발급/중복 seq·종류 불일치·ok인데 scene 불일치·모순된 상태·레이어/순환 index·
  숫자 범위·잘못된 UTF-8 hex·512점 초과 등을 protocol 오류로 거부한다.
  HTTP에 native 진단 문자열/경로를 그대로 노출하는 API가 아니다.
- render/cancel은 query 대기 중에도 제출·소비할 수 있다. render cancel ACK는
  query credit을 해제하지 않는다. 동기 style ACK는 query를 먼저 drain해야 한다.
  M4a-2 controller는 이를 drain한 뒤 style을 변경한다(§2).
- query 기본 deadline은5초(`Config.query_timeout`, 허용0초 초과~24h), 제출 때부터
  절대시간이다. 다른 frame/응답으로 연장하지 않는다. timeout·프로토콜 오류는
  기존 client 방식대로 worker를 close/kill/reap하고 명시 오류를 반환한다.
  shutdown flag·bounded IO·파일 회수와도 독립적으로 연동된다.
- 임의 renderd를 노출하는 서비스가 아니며, 추가 native worker나 HTTP 자원 예약은
  없다. 기존 renderer의 query thread·kind별 supersede를 사용한다.

### 검증과 다음 연결

- native context 단위: scene 두 counter·round·overflow, 미게시/불일치/partial과
  요청 요약 레이어 판정. client 단위: strict 결과/좌표/outline/상태 조합.
- self-hosted fake daemon: 렌더와 query 교차, 독립 credit,8개 상한, 잘못된/중복
  seq·종류·scene, timeout/close와 private 파일 회수. Python은 필요 없다.
- `tools/validate_worker_queries.py`: KLayout로 작은 합성 OASIS와 면적 오라클을
  만들고 native로 색인·점유 요약을 생성한다. **PATH-empty 실제 Rust test/daemon**이
  vertex snap,2개 overlap/음수 순환,512점 초과 윤곽과 전체 면적, margin→라벨-only
  scene 유지, A→B→A 가시성 전환·stale 거부, query/render 동시 진행, 요약+exact
  혼합의 선택별 질의와 partial scene 거부를 단언한다. partial은 기존 native 진단
  한계 `decode_pages=0`을 typed request의 선택 필드로 전달해 실제로 생성한다
  (기본값/HTTP·UI 옵션은 변경 없음). cache/summary bytes·mtime 불변과 임시파일 정리를
  검사하며 full battery에 배선했다. UTF-8 경로는 native, UTF-8 결과 codec은 fake/단위로 검사한다.
- 기존 valmini Python/KLayout query·PNG/raw 픽셀 오라클도 계속 실행한다.

검증 결과:

- native renderd 단위19개, worker-client 단위7개·수명주기11개 통과. 실제 native
  질의 gate는 요약+exact 혼합/partial/가시성 전환을 모두 실행하고 통과했다.
  기존 export용 fake daemon도 새 frame schema로 갱신했으며 partial export를
  성공으로 취급하는 등 기존 판정 기준을 완화하지 않았다.
- worker-client/app-core/web/app의 `cargo test --offline --locked`와
  `cargo clippy --all-targets --no-deps -- -D warnings` 통과. 해당 패키지와
  변경 native 파일의 포맷 검사 통과. native clippy는 기존 dead-code/unused
  경고를 남기므로 workspace 전체 경고0이라는 뜻은 아니다.
- Rust 1.89.0의 같은 앱 패키지 테스트 및 새 worker 회귀 통과. registry 접근 없이
  `--offline --locked`로 실행했다. Linux musl release 앱/index/renderd 교차 빌드
  성공, 세 실행 파일이 x86-64 ELF static-pie임을 확인했다. Linux 실행은 미검증이다.
- 전체 `sh tools/validate_rust.sh`가 `RUST VALIDATION: ALL OK`로 통과했다.
  KLayout 13 PX + 2 phase-exact + 14 style을 jobs1/8에서 각각 통과했고,
  기존 jobdeck80·renderer46, cancel/clip/PNG/raw 및 Python 비교도 유지했다.

실제 Firefox/ETX·실칩 query 지연/메모리를 이 합성 검사로 대체하지 않는다.

위 선행 단계 이후 ViewController 연결은 §2에서 구현했다. owner API·pick/snap
UI·ruler 연결과 공유/원격 공개·현장 수용은 여전히 별도다.

## 2. M4a-2: 표시 frame에 고정한 로컬 controller query

`app-core/view`의 `ViewController`까지 연결했다. **HTTP/WS schema·endpoint·
인증/공유 권한·브라우저 UI는 변경하지 않으며 웹 query capability는 false다.**
새 geometry decode, query 전용 render, 추가 worker나 frame history는 만들지 않는다.
native 호환 버전은0.12.87로 index/renderd를 함께 재빌드해야 한다.

### 표시 anchor와 geometry scene

- `QueryAnchor`는 dataset revision·worker epoch·frame ID·state revision·render
  revision·render key다. `query_anchor(frame_id)`는 현재 보관한 foreground 또는
  margin이 그 상태/배율에서 viewport 전체를 덮는지 확인한다. caller가 **실제로
  표시한** frame을 지정해야 하며, 최근 수신 frame을 대신 사용하면 안 된다.
  controller는 DOM을 보지 못하므로 이 API만으로 실제 표시 여부를 증명하지 않는다.
- `query(ViewQuery)`는 이 anchor를 원자적으로 검사한 뒤 mailbox에 넣고 local ID를
  반환한다. native 제출 직전 다시 검사하며, 결과 소비 시에도 latest ID와 anchor가
  살아 있어야 한다. view edit·대상 frame 교체·close는 입력/결과를 무효화한다.
  다른 worker나 재open에서 같은 숫자를 재사용해도 이전 결과를 채택하지 않는다.
- background margin이 native의 최신 scene이 된 뒤에도 foreground를 계속 표시할
  수 있다. 동일 render key·배율·16px 위상에서 viewport를 덮는 **알려진 scene**에
  질의를 고정하고 원래 표시 anchor도 함께 보관한다. crop pan은 current viewport
  좌표를 쓰며 geometry를 다시 렌더하지 않는다. 제출 후 동등한 새 margin이 와도
  응답은 native가 캡처한 immutable scene에서 왔고 표시 anchor가 유효한 경우에만 쓴다.
- 라벨/폰트 변경은 UI render key를 올리지만 native geometry scene은 재사용할 수
  있다. depth/cut/layers/frames/mono/decode ceiling/style epoch/thin으로 별도 geometry
  key를 검사하고, 같은 scene을 참조하는 작은 label-only frame이 기존의 넓은
  coverage를 축소하지 않게 한다. 보지 못한 scene 범위를 추정하거나 여러 bbox의
  합집합을 새 coverage로 주장하지 않는다. 같은 scene ID의 geometry/완료/요약
  메타데이터가 바뀌면 worker 오류다.

### 좌표·레이어·거부 의미

- `ViewQuery.position`은 **현재 viewport 좌상단 기준0..1 비율**이다(margin 이미지
  비율 아님). `radius_px`는0..64 **device px**로 한정한다. 좌표와 반경은 finite여야
  하며, 서버에서 GTK의 `int(world)`와 같은 0 방향 절삭·최소 DBU 반경1로 변환한다.
  overflow는 입력 오류다. 미래 웹 경계의 CSS/DPR 변환은 별도 구현/검증 대상이다.
- `Layers::All`은 현재 가시 선택을 뜻한다. 명시 pair는 알려진 가시 레이어여야
  하며 숨긴 레이어를 섞으면 오류다. None은 정상적인 빈 선택이다.
- 표시 geometry나 현재 게시 geometry가 미완료면 질의를 거부한다. 라벨만 잘린
  frame을 geometry 미완료로 오인하지 않는다. 요약+exact 혼합은 native가 **요청한
  레이어** 기준으로 판단하므로 exact subset은 허용, 요청 요약은 명시 Summary다.
  jobdeck은 Unsupported다. 모든 결과의 status를 확인해야 하며 이 단계가 cut/LOD/
  wash로 근사한 geometry를 원본 정밀 geometry로 바꾸는 것은 아니다.
- `ViewQueryResult.reply`의 native 진단은 로컬용이다. 경로/문자열을 웹에 그대로
  전달하는 DTO로 쓰지 않는다. 다른 도형/파일을 자동으로 탐색하는 fallback도 없다.

### 최신 입력·취소·스타일 수명

- snap/pick 각 pending1개·결과1개, native in-flight 각2개(합4개)다. 새 입력은 같은
  종류의 pending/result를 교체한다. **superseded ID마다 terminal 결과를 보장하지
  않는 latest-only 계약**이다. native 요청이 유계여도 caller가 받은 Arc를 외부에서
  무한 보관하는 것은 controller의 보관 상한에 포함하지 않는다.
- `query_snapshot()`은 종류별 최신 ID/결과와 accepted/submitted/consumed/discarded,
  queued/in-flight를 제공한다. query 결과는 frame bytes나 새 dataset lease를
  보관하지 않는다. 대기 입력도 현재 metadata/레이어 수 한계 안이다.
- `cancel_query(kind)`는 local 슬롯을 즉시 비우고 실제 미완료 질의가 있으면 native
  `cancel_query kind=snap|pick before_seq=N`을 보낸다. 종류별 취소 frontier는 단조이고
  `< N`만 중단한다. query와 같은 seq 시계의 새 번호를 쓰지만 render generation 및
  다른 query 종류에는 영향이 없다. 이미 완료된 질의는 정상 응답할 수도 있다.
- client는 종류별 취소 ACK 최대1개를 추가 보관한다. ACK와 각 query terminal은
  독립적이며 `pending_queries()`는 둘을 합산한다(직접 worker client는8 requests+
  2 ACKs, controller는4 requests+2 ACKs). 취소 ACK만 받고 request credit을 해제하지
  않는다. ACK의 kind/frontier/미발급/중복을 검증하고 query와 같은 제출 기준5초
  절대 deadline을 적용한다. 다른 frame으로 연장하지 않는다.
- 스타일 변경은 query와 취소 ACK를 모두 drain한 뒤 동기 호출한다. 그동안 계속
  poll하고, 스타일이 그대로인 pan render는 stale query를 기다리지 않는다.
  `RenderSession::capture`는 query/ACK 미소비 중이면 Busy로 거부하되 worker를
  닫지 않는다. 따라서 frame만 소비하는 동기 capture가 query 응답을 버리지 않는다.
- timeout/통신 오류면 기존 방식으로 view를 Failed로 만들고 worker·permit·파일을
  정리한다. 정상 close도 결과/입력/scene metadata를 비운다. native query 취소는
  협조적이며 진단용 `FLOE_RUST_QUERY_INLINE=1`은 stdin 실행 중 중단을 보장하지 않는다.

### 검증과 남은 단계

- controller 단위8개: 모든 anchor 필드·좌표/반경·숨긴 레이어·미완료 거부,
  음수 절삭,1000개 입력 coalesce·종류별 상한, 취소 독립, ACK 뒤에도 style drain,
  pan과 stale reply·오류 회수, margin crop, label/font key와 thin/style/context 검증,
  deck Unsupported를 고정한다.
- process client fake gate: kinds/ACK/request credit·seq·render 교차와 잘못된 ACK/
  중복·timeout/close를 추가한다. native 단위는 파서 canonical 범위와 실제 종류별
  frontier의 단조·strict 판정을 검사한다.
- `tools/validate_worker_queries.py`는 기존 native gate에 종류별 실제 취소 교차,
  실제 `ViewController`의 foreground→margin→crop·label/font·가시성 A→B→A·worker
  격리·summary/exact subset·close/lease, 동기 capture Busy 후 같은 worker에서 query
  drain과 재capture 성공을 추가했다. PATH-empty 실행, 합성 fixture만 사용하며
  source cache/summary bytes·mtime 불변과 private 파일 회수를 확인한다.

검증 결과:

- core88개(새 controller8개 포함), worker-client 단위7개·수명주기12개, native
  renderd21개 통과. 실제 native worker/query와 controller/query gate도 통과했다.
  마지막에 보강한 label/font/thin 메타데이터 단위와 실제 capture guard까지 다시 실행했다.
- app/core/worker/web의 offline·locked 테스트, 해당 패키지 strict scoped clippy와
  포맷 검사 통과. Rust 1.89.0에서도 앱 패키지 테스트를 통과했고, 기존 native/
  dependency warning은 남아 있으므로 workspace 전체 경고0을 주장하지 않는다.
  Cargo 검증은 `rust/`에서 실행해 vendored source 설정을 적용했다.
- 전체 `sh tools/validate_rust.sh`가 `RUST VALIDATION: ALL OK`로 통과했다.
  기존 jobdeck80·renderer46 및 KLayout 13 PX + 2 phase-exact + 14 style을
  jobs1/8에서 통과했다. Linux musl release 앱/index/renderd 교차 빌드와 x86-64
  static-pie 형식도 확인했다. Linux 실제 실행 및 현장 수용은 미검증이다.

owner/view/connection epoch·표시 packet 연결과 query DTO는 다음 §3에서 구현했다.
UI pick/snap·수동 ruler/clip은 남아 있다. `.ovo` 외부 교체/hot reload·현장
Firefox/ETX·실칩 지연/메모리·공유 권한과 실제 pack-build 승인 클릭은 별도다.

## 3. M4a-3: owner WebSocket query

기존 인증된 `/api/v1/events`에 질의·취소와 결과 전송을 연결했다. bootstrap/
HttpOnly cookie/CSRF/Origin/bundle 검사·loopback 제한·owner 세션 수명은 그대로다.
새 공유 권한, 외부 listener, REST query, 경로/argv 입력 또는 파일 쓰기는 없다.
브라우저 pick/snap·수동 ruler 조작은 아직 연결하지 않았다. native wire 변경은
없어 버전0.12.87을 유지하며, 앱/웹 bundle은 다시 빌드해야 한다.

### 요청과 응답

기존 연결별 단조 `seq`를 다른 제어 메시지와 함께 쓴다. 다음은 shape 질의다.

```json
{
  "type": "view.query", "seq": "20", "view_id": "<view>",
  "connection_epoch": "<current connection>",
  "body": {
    "anchor": {
      "dataset_revision": "1", "worker_epoch": "2", "frame_id": "4",
      "state_rev": "3", "render_rev": "3", "render_key": "1"
    },
    "operation": {"kind": "pick", "nth": "0"},
    "position": [0.5, 0.5], "radius_px": 1,
    "layers": {"mode": "all"}
  }
}
```

- snap은 `operation:{"kind":"snap"}`(nth 없음), pick의 nth는 signed-i64 canonical
  문자열이다. ID/seq는 양의 canonical u64 문자열이다. 중복/미정의 필드, null,
  객체로 쓴 문자열 enum 등을 거부한다. 메시지는 기존8KiB·60개/s 한계 안이다.
- `position`은 현재 viewport 기준 비율이고 `radius_px`는0..64 device px다.
  화면에 안 보이는 점·비유한 값은 오류이며 DBU 변환은 controller가 수행한다.
  가시 레이어 all/none/only 의미와4096 pair 상한은 §2와 같다. 8KiB 메시지 한계도
  별도로 적용되므로 큰 명시 목록은 모두 전달할 수 있다는 뜻은 아니다.
- 접수되면 `query.accepted`에 원래 seq·view_id·connection_epoch·query_id가 온다.
  잘못된 body는 `error`/`invalid_request`, 표시 receipt가 없으면
  `frame_not_displayed`, 현재 frame/state와 맞지 않으면 `stale_frame`이다.
  미완료/unsupported 등 controller 거부도 safe code다. 잘못된 view/connection,
  역행 seq·잘못된 JSON schema는 기존 제어 계약처럼 socket을 닫는다.
- 비동기 `query.result`는 원래 seq·query_id·view_id·connection_epoch·전체 anchor,
  status와 hit를 가진다. 정상 `ok`+hit=null만 빈 결과다. native refusal은 §1의
  상태명을 유지하고, 일반 native 오류는 `query_failed`로 바꾼다. native 처리 전
  anchor가 바뀌거나 다른 최신 질의가 생기면 `stale_frame`/`superseded`이며 hit는 null이다.
- native 응답에는 `scene:{generation,round,complete,summary_layers}`와
  `requested_summary_layers`도 있다. native 응답 없이 transport에서 종료한 결과에는
  이 native metadata가 없다. scene counter/summary 수는 문자열이며 미게시 ID는 null이다.
- snap hit는 `kind:snap`, `point_dbu:[x,y]`, `snap:vertex|edge`다. pick은
  `kind:pick`, count/index, pair, layer_name/cell_name, area_dbu2, bbox_dbu,
  points_dbu, points_truncated다. 좌표·면적·counter는 문자열, u32 pair만 JSON 숫자다.
  면적은 native f64 값을 더 손실 없이 옮긴 것이지 임의 정밀도 정수 면적은 아니다.
  `points_truncated=true`의512점 prefix를 완전한 윤곽/측정값으로 해석하면 안 된다.
  이름은 도형 metadata이며 future UI에서 text로 표시해야 한다. native 오류 문자열,
  파일 경로·native 명령·전체 request/진단 필드는 전송하지 않는다.

취소는 다음과 같다. 자신의 연결에서 마지막으로 제출한 해당 종류만 취소한다.

```json
{"type":"view.query.cancel","seq":"21","view_id":"<view>","connection_epoch":"<current connection>","kind":"snap"}
```

응답 `query.cancelled`는 원래 seq·view/connection·kind를 확인한다. 이것은 local
latest 슬롯 무효화 확인이며 native가 중단됐다는 ACK/terminal과 구별한다.

### 표시·연결·전송 상한

- frame packet에 `query_scene`를 추가한다. frame의 `query`는 게시 geometry ID가
  있고 geometry가 완료됐을 때 true다. 라벨만 잘림·mixed summary 자체는 false
  조건이 아니며, 요청 요약 레이어는 native가 따로 거부한다. snapshot capability
  `query`는 layout=true, deck=false다. 프레임 없거나 stale일 때의 제출 허가는 아니다.
- socket마다 **displayed ACK와 writer 완료를 모두 확인한** foreground/margin
  receipt 각1개만 보관한다. discarded ACK, 새 연결, 다른 frame/worker/revision에는
  receipt를 빌려주지 않는다. receipt는 ID metadata뿐이며 frame Arc/bytes는 추가로
  보관하지 않는다. margin receipt의 원래 state/render revision은 crop 후 오래될
  수 있으므로 요청은 현재 snapshot revision을 쓰고 controller에서 다시 확인한다.
- ACK/전송 완료는 클라이언트가 표시했다고 신고한 근거이며 DOM 표시를 증명하지는
  못한다. 미래 UI는 실제 사용할 foreground/margin을 골라야 하고, drag preview·
  미확정 입력·새 상태/연결에서 이전 결과를 표시하지 않도록 별도 검증해야 한다.
  writer 완료가 아직 확인되지 않은 순간의 질의도 `frame_not_displayed`로 거부한다.
- 연결별 ticket은 snap/pick 각1개이며 최신 결과만 한 번 전송한다. superseded
  입력마다 terminal을 보장하지 않는다. 같은 view의 여러 owner 소켓은 같은
  controller의 종류별 latest 질의를 공유하므로 서로 supersede할 수 있지만,
  결과는 발급한 소켓의 ID에만 반환한다. 독립 view를 같은 view로 취급하지 않는다.
- 연결 종료/명시 취소는 `cancel_query_if_current(kind,id)`로 비교 후 무효화한다.
  오래된 소켓이 닫히며 다른 소켓의 새 질의를 취소하거나 새 결과를 지우지 않는다.
  재접속은 receipt와 질의를 복원·재전송하지 않는다. native drain/timeout은 §2를 유지한다.
- query 결과는 이미지 ACK·encoder/byte admission을 기다리지 않는다. 공통 유계
  writer queue4개에 공간이 없으면 latest ticket에서 재시도하며 결과 큐를 늘리지
  않는다. view edit ACK+snapshot용2슬롯을 남긴다. 개별 응답은256KiB 상한이다.
  완료된 이미지 packet도 기존 byte reservation 안에서 최대1개만 대기하며 output
  queue가 잠시 찼다고 복사/렌더를 다시 하지 않는다. queue에 여유가 있으면 encoder
  완료 즉시 전송하여 정상 프레임에 추가 tick 지연을 넣지 않는다. 이미지의 기존 ACK/write
  deadline과 native controller의 독립 poll/reap은 그대로다.

### 검증 범위

- 단위: DTO의 각 counter·signed nth·미정의 필드·범위, 큰 좌표 문자열과 native
  오류 redaction,2개 receipt 상한/worker·revision 구별, geometry 완료와 라벨/요약
  capability 구별, 오래된 consumer의 조건부 취소를 고정한다.
- `tools/validate_worker_queries.py`는 기존 fixture로 세 실제 owner WS gate를
  추가 실행한다.2개 겹침/음수 nth·vertex·긴 윤곽, visible layer·요약/exact subset·
  정상 빈 결과, margin ACK 보류 중 foreground query·crop·재접속·다른 연결 종료,
  discarded/위조 frame/worker·stale revision·화면 밖 점·큰 반경·다른 view/epoch·
  미정의/중복 필드 거부를 검사한다. cache bytes/mtime 불변과 worker 파일 회수도 유지한다.
- 기존 실제 PNG/raw view-stream gate는 같은 test harness를 재사용하며
  100-input/slow subscriber/credit/재접속/로그아웃과 margin parity 기준을 유지한다.

검증 결과:

- core89개, web34개와 transport8개 통과. 전송 queue 포화 시 packet/byte reservation을
  복사 없이 유지·반환하는 단위 테스트도 포함한다. 실제 native query1개·controller
  query/capture2개·owner WS3개와 기존 PNG/raw stream2개 모두 통과했다.
- app/core/worker/web의 offline·locked 테스트와 scoped strict clippy·포맷 검사,
  기존 JS/UI 회귀 검사를 통과했다. Rust 1.89.0 테스트 및 Linux musl release
  app/index/renderd 교차 빌드도 통과했으며 세 실행 파일의 x86-64 static-pie 형식을
  확인했다. 기존 native/dependency warning은 남아 있다. 전체 workspace 포맷 검사는
  변경하지 않은 native 파일의 기존 차이로 실패하며 이번 범위에 일괄 포맷을 섞지
  않았다. Linux 실제 실행은 미검증이다.
- 전체 `sh tools/validate_rust.sh`가 `RUST VALIDATION: ALL OK`로 통과했다.
  jobdeck80·renderer46 및 KLayout 13 PX + 2 phase-exact + 14 style을 jobs1/8에서
  통과했다. 전체 검사 시작 후 추가한 정상 packet 즉시 전송 보정은 영향받는 단위·
  clippy·실제 query/stream 및 MSRV/교차 빌드를 다시 실행해 확인했다.

브라우저 pick/snap 연결은 다음 §4로 이어진다. 수동 ruler/clip, 현장 Firefox/ETX,
외부 공유 권한과 pack-build 승인 클릭 수용은 owner API 검증으로 대체하지 않는다.

## 4. M4a-4: 브라우저 도형 선택·스냅 프로브

`query.js`는 §3 API의 유계 클라이언트, `inspect.js`는 Canvas 주석/Inspector다.
기존 ES2017·로컬 자산·인증·pan/DRC 경로를 확장하며 새 라이브러리, 외부 호스팅,
공유 권한 또는 원본/인덱스/리뷰 파일 쓰기는 없다. native wire/버전은0.12.87을
유지하고 UI bundle을 다시 빌드한다. 기존 GTK launcher는 유지한다.

### 사용자 동작

- 일반 클릭은 도형 선택, 같은 위치8 CSS px 안의 반복 클릭은 overlap 순환이다.
  Inspector의 ←/→는 현재 지점의 이전/다음 후보를 조회한다. native 후보64개 계약을
  유지하며 Shift는 추가, Ctrl/Cmd는 toggle이다. modifier 클릭은 최상위 후보를
  고르고 순환을 초기화한다. 최대64개 선택에 도달하면 기존 선택을 보존하고 표시한다.
- DRC box 모드가 입력을 소유하며, 일반 클릭의 실제 DRC marker hit는 기존 동작을
  우선한다. modifier 클릭은 도형에 전달한다. drag/반환 drag/chord/버튼 불일치/
  취소는 클릭이 아니다. Ctrl/Cmd drag는 이전처럼 pan을 만들지 않는다.
- 도형의 layer/cell·native 면적(DBU²)·bounds(DBU)를 text로 표시하고, 현재 layer
  페이지의 해당 행과 도형 윤곽을 노란색으로 강조한다. 숨은 layer page를 자동으로
  찾아 이동하는 기능은 아직 없다. Clear/Escape로 선택·미완료 질의를 지운다.
  DRC의 기존 Escape 처리 순서를 먼저 유지한다.
- `Snap probe (m)`는 기본 off다. 켜면 hover의 vertex/edge를 십자로 표시하고
  native 정수 DBU 좌표를 읽는다. **수동 ruler나 거리 측정의 대체가 아니다.**
  이후 ruler가 같은 snap 경로를 사용한다. 반경은 pick3/snap10 CSS px를 DPR로
  변환하되 기존 API 한계64 device px로 제한한다.
- 요약/미완료/scene 교체/timeout/질의 오류를 "도형 없음"과 구별한다. UI는 현재
  가시 레이어 all로 조회하므로 mixed summary 거부 시 확대하거나 summary 레이어를
  숨겨 exact subset을 고른다. deck은 기존 미지원 이유를 보여준다.

### 좌표·수명·상한

- 실제로 blit하고 displayed ACK를 전송한 foreground/margin만 요청 대상이다.
  전체 margin crop을 우선하고, 라벨 partial margin이 이전 라벨 프레임 아래에서
  새 strip을 제공하는 경우에도 geometry-complete이면 그 margin을 선택한다.
  margin 원래 revision이 아니라 **현재** snapshot revision과 retained frame ID를 보낸다.
- 입력은 viewport의 정렬된 device 영역(CSS left/top의 소수 여백 포함)에서 구한
  0..1 비율이다. margin origin을 입력에 이중 가산하지 않는다. DBU 변환/반올림은
  Rust만 수행한다. pending edit/drag/frozen preview, 크기/DPR 불일치, 숨김/끊긴
  연결, ACK 전에는 보내지 않는다. 응답 사용 직전 같은 표시 anchor·DPR·CSS 배치를
  재검사한다. 새 프레임·pan·편집·재접속·모드 전환에서 이전 응답을 되살리지 않는다.
- snap/pick 각 최신 입력1개, 공통 전송 간격80ms(최대12.5 query/s), 단일 송신
  타이머와 종류별8s timeout이다. 500개 hover도 최신1개로 합치고 중간 입력을
  재생하지 않는다. 아직 전송하지 않은 최신 hover가 취소될 때도 이전 native
  in-flight 요청을 취소한다. 로컬 취소는 native drain 완료를 뜻하지 않는다.
- 이미 접수한 도형은 같은 dataset/worker/render key의 pan에서 world 주석으로
  유지한다. 미완료 요청은 폐기한다. render key(표시 정책/스타일), dataset/worker,
  connection 교체에서는 선택도 초기화한다. query 결과만으로 새 render/HTTP
  조회를 만들지 않는다. opt-in snap이 off이면 hover 질의도 없다.
- wire ID/좌표는 u64/i64 canonical 문자열로 검증하고 text에 보존한다. Canvas
  투영은 기존 렌더/DRC와 같은 f64 표시 연산이며 임의 정밀도 측정기가 아니다.
  면적은 native f64 DBU² 값이다. native 윤곽512점 상한을 검증하며 truncated
  prefix를 닫힌 polygon으로 잇지 않는다(열린 선 + 점선 bbox + 명시 문구).
- 선택64개·윤곽512점과 viewport 픽셀 상한을 적용하는 주석 Canvas1개를 쓴다.
  pan/hover paint는 rAF로 합치고 종료 시 주석 버퍼·타이머를 정리한다. 도형 재스캔/
  raster·파일/scene history를 브라우저에 추가하지 않는다.

### 검증과 남은 범위

- `query.test.cjs`: 소수 CSS origin/DPR/margin crop, 모든 표시 anchor 경계,
  큰 정수·잘못된 DTO·긴 윤곽·scene/summary/empty 구별,500 hover coalescing,
  전송/대기 교체 후 취소·종류 격리·timeout·stop/resume/회수.
- `inspect.test.cjs`: 실제 UI 모듈에 클릭·순환·추가/toggle·cap·취소·snap을 입력한다.
  plain text, 열린 truncated 윤곽, DPR2 십자 크기, pan 주석 유지와 style/연결
  초기화, 오류와 빈 결과 구별, 수동 ruler를 꾸며내지 않는 범위를 고정한다.
- `client.test.cjs`: 실제 app.js/gesture/Inspector/WS ACK 경로를 함께 실행한다.
  재렌더/HTTP 없이 selection/snap, margin crop의 새 revision, label partial의
  geometry, late response·error 무효화를 단언한다. 기존 전체 DRC/UI 테스트와
  object modifier/release/drag, frame query metadata 검사를 함께 실행한다.
- 위 테스트는 결정적 DOM/Canvas 하네스이며 실제 브라우저 시각·조작 수용을
  대신하지 않는다. 이 단계에서 사용자 브라우저 탭을 바꾸거나 pack-build 승인
  클릭을 실행하지 않았다. 현장 Firefox/ETX는 보류이고 공유 권한도 대기 중이다.

검증 결과:

- ES2017 파싱과 전체 JS/UI 회귀가 `WEB UI: ALL OK`다. 새 query/Inspector 및
  실제 app.js 연결 테스트와 기존 DRC·gesture·protocol 검사를 함께 통과했다.
- app6개·web34개·transport8개 offline·locked 테스트, scoped 포맷·strict clippy,
  release 자산 빌드가 통과했다. Rust 1.89.0에서도 같은 테스트를 통과했으며
  Linux musl release 앱 교차 빌드와 x86-64 static-pie 형식을 확인했다.
  기존 native/dependency warning은 남고 Linux 실제 실행은 미검증이다.
- 전체 `sh tools/validate_rust.sh`가 `RUST VALIDATION: ALL OK`다. jobdeck80·
  renderer46, KLayout 13 PX + 2 phase-exact + 14 style의 jobs1/8 게이트도 통과했다.
  전체 검사 후 추가한 JS-only hover repaint/입력 보정은 최종 JS/UI·app/web 테스트,
  clippy·release 자산·MSRV/musl 빌드를 다시 실행해 확인했다. native geometry는
  이번 단계에서 변경하지 않았다. 현장 수용이나 전체 M4 완료를 뜻하지 않는다.

수동 ruler(시작/끝·snap·축 고정·삭제·clear·측정), clip/내보내기, 선택 layer의
다른 페이지 추적, 나머지 GTK 조작 parity는 다음 단계다. M4 전체 완료가 아니다.

## 5. M4a-5: Rust 좌표·거리 계산과 수동 ruler

일반 두 점 ruler를 연결했다. GTK의 `_cursor_snapped`/`_ruler_end_preview`/
`_ruler_click`을 기준으로 **기본 dominant-axis, 동률 수평, Shift 자유 각도**다.
거리와 Δx/Δy는 Rust에서 DBU→µm로 계산하며, browser는 수치 검증·표시 투영과
문자열 formatting만 한다. native renderer/인덱스 포맷·버전0.12.87은 유지하고
Rust 앱과 embedded UI bundle을 재빌드한다.

### 읽기 전용 계산 계약

- 기존 owner WebSocket에 `view.measure`를 추가했다. `seq`, view ID, connection
  epoch와 §3의 표시 ACK+write receipt 검증을 그대로 사용한다. 새로운 listener,
  공유 권한, REST route, 파일 읽기/쓰기, 원본 geometry 열거는 없다.
- body는 `anchor`, 현재 viewport top-left 기준 `position:[x,y]`(0..1),
  `start_dbu:null|[x문자열,y문자열]`, `free_angle:bool`,
  `snap_query:null|query_id문자열`이다. 첫 점은 start=null, 두 번째 점/미리보기는
  이미 접수한 첫 점을 보낸다. start는 사용자 주석 좌표이며 geometry 권한 토큰이 아니다.
- `snap_query`는 **이 연결에서 결과를 전송한** snap ticket만 참조한다. controller에서
  여전히 latest이고 동일한 표시 anchor·질의 위치(기존 GTK int 변환 기준)이며
  성공한 결과인지 다시 확인한다. 다른 연결·변경된 위치·취소/교체/오류 결과는 거부한다.
  성공한 빈 snap만 커서 좌표로 측정하며, 오류/summary 거부는 빈 결과로 바꾸지 않는다.
- snap이 없으면 표시 viewport와 입력 비율의 고정 개수 좌표만 계산한다(도형 열거 없음).
  exact geometry가 없어도 가능하므로 잡덱/summary/미완료 geometry에서도 **snap off**
  좌표 측정은 지원한다. 이는 해당 도형을 검증하거나 실제 경계를 찾아준다는 뜻이 아니다.
- `measure.result`는 seq/view/epoch/anchor, 원래 cursor/snap의 `point_dbu`,
  `snap:null|vertex|edge`, `segment:null|{endpoints_dbu,delta_um,distance_um}`을
  반환한다. endpoints는 축 고정 후 좌표다. 좌표/거리/ID는 문자열이며 native 오류나
  로컬 경로는 응답에 넣지 않는다. 현재 view 상태·render revision을 바꾸지 않는다.
- unsnapped 좌표는 GTK와 같은 f64이고 ±2^62 DBU 범위를 따른다. snapped i64는
  문자열로 보존하고 두 정수의 차이를 i128에서 먼저 계산한다. 따라서 2^53 위의
  인접 정수 간격을 절대 좌표 반올림으로 잃지 않는다. 비정수 좌표와 최종 길이는
  f64이므로 임의 정밀도 측정기라고 주장하지 않는다. 비유한 수/overflow는 명시 오류다.

### 조작·수명·상한

- `r`/Ruler 버튼으로 시작, 두 클릭으로 저장하며 mode는 다음 측정을 위해 유지한다.
  Shift는 자유 각도, `m`은 ruler mode의 edge/vertex snap toggle이다. layout은
  snap 기본 on이며 mode 밖의 기존 Inspector snap probe(m, 기본 off)와 구별한다.
  잡덱은 snap을 비활성화하고 cursor-only 안내를 표시한다. summary는 사용자가
  snap을 끄거나 exact 레이어/배율을 선택해야 한다.
- 수동 ruler mode의 클릭은 geometry pick/DRC marker보다 우선한다. DRC box는
  서로 배타적인 mode다. 기존 drag pan을 유지하며 drag를 두 번째 점으로 간주하지 않는다.
- hover는 snap·계산 각각 최신1개와80ms 송신 간격을 사용한다. 중간 입력은 replay하지
  않고 종료/화면 교체에서 폐기한다. 클릭 확정 중 hover는 확정 요청을 덮어쓰지 않는다.
  항상 클릭 위치를 새로 해결하므로 오래된 hover snap/preview를 클릭 결과로 쓰지 않는다.
- 첫 점 marker와 마지막 검증된 preview는 새 응답을 기다리는 동안 유지해 매 mousemove에
  깜빡이지 않게 한다. 상태는 계산 중임을 표시한다. stale/오류/화면 교체에서는 preview를
  지운다. 완료 ruler와 이미 접수한 첫 점은 같은 view/dataset/worker의 pan·zoom·표시 정책
  변경에서 world 주석으로 남는다. 미완료 요청은 표시 anchor/연결/DPR/크기 변경 시 폐기한다.
- `k`는 수동 ruler의 마지막 항목, `Shift+K`는 수동/DRC ruler 전체를 지운다.
  Escape는 pending point → ruler mode → 완료 rulers 순서이고 그 뒤 기존 selection/DRC
  처리를 따른다. mode 밖에서 완료 수동 ruler를 Escape로 지우면 CD ruler도 같이 지운다.
  DRC box가 활성화된 때는 기존 box Escape 우선순위를 유지한다.
- 수동 ruler는256개까지이며 초과 시 기존 ruler를 버리지 않고 명시 안내한다. 서버에는
  ruler history를 추가하지 않고 화면당 Canvas1개(기존16 Mpx 한계)와 유계 배열만 사용한다.
  이전 DRC ruler의 clipping·label 배치 코드를 재사용한다. 값은 별도 panel에서도 읽을 수 있다.
  query/measure timeout은 각각8s이며 다시 선택하도록 안내한다. 페이지 종료 시 타이머·
  주석 Canvas를 회수한다. ruler 영구 저장이나 페이지 새로고침 복원은 아직 없다.

### 검증 범위와 남은 parity

- Rust 단위: canonical 좌표, fractional cursor, 양/음수·동률 축·free angle·0 길이,
  i64 극단의 checked 차이·1 DBU 간격, frame/margin·pan·닫힘, snap 성공/빈 결과/
  summary 오류·다른 위치·취소. viewport 좌표 측정은 추가 query/render가0인지 단언한다.
- 실제 renderer/owner WS: 두 도형 vertex snap, axis/free 거리, 다른 socket의 snap
  대여 거부, 이전 snap/frame 거부, pan 후 소수 좌표, 렌더 횟수 불변을 고정한다.
  기존 `validate_worker_queries.py`에 필수 `WEB MANUAL RULERS: ALL OK` 마커를 배선했다.
  인덱스/summary bytes·mtime 불변과 worker 임시 파일 회수도 같은 gate가 검사한다.
- 결정적 UI: 실제 measure 모듈의 두 점/preview·Shift·snap→measure 연결, 큰 ID,
  summary와 정상 빈 결과 구별,500 hover 최신 슬롯, 클릭 확정 보호·timeout·stale,
  marker 유지·256개 상한·삭제/Escape·회수. 실제 app.js에서도 DRC marker보다 ruler
  입력 우선, HTTP/이미지 재렌더 없이 native 응답을 표시하는 경로를 검사한다.
- **아직 남음:** 선택2개 이상에서 `r` 진입 시 nearest-neighbor bbox gap 자동 측정,
  수동/auto/CD를 통합한 정확한 생성 순서의 k 삭제, 서로 다른 ruler군의 label 충돌
  회피, overlay/export·새로고침 복원. 현재는 k가 수동 항목부터 지우고 없을 때 CD를
  처리한다. 이를 GTK와 완전 동일하다고 보지 않는다. clip/다른 layer page 추적도 남는다.
- 실제 browser 시각·조작 수용, 현장 Firefox/ETX, pack-build 승인 클릭, 공유 권한은
  여전히 별도 미검증/대기다. 로컬 하네스와 교차 빌드로 대체하지 않는다.

검증 결과(2026-09-14):

- 최종 app6·app-core94·web35·transport8 테스트, scoped 포맷/strict clippy와
  ES2017·전체 JS/UI 회귀가 통과했다. 실제 query gate는 native1·controller2·
  owner WS4개(새 ruler 포함)를 통과했다. 새 marker를 빼거나 ignored 테스트를
  건너뛰면 통합 스크립트가 실패한다.
- 전체 `sh tools/validate_rust.sh`가 `RUST VALIDATION: ALL OK`다.
  jobdeck80·renderer46, KLayout 13 PX + 2 phase-exact + 14 style의 jobs1/8
  검증을 포함한다. 전체 검사 시작 후 보강한 극단 수치의64자 표기 및 추가 단위
  케이스는 최종 관련 Rust/UI·실제 query·clippy·release 빌드에서 재검증했다.
- Rust 1.89.0에서도 app6·core94·web35·transport8을 통과했고 Linux musl
  release 앱의 x86-64 static-pie 교차 빌드를 확인했다. 기존 dependency warning은
  남는다. Linux 실제 실행·브라우저 시각/조작 및 TeeBox 수용은 이 결과에 포함하지 않는다.

## 6. M4a-6: 선택 bbox 자동 gap과 통합 ruler

GTK `_measure_selection`의 nearest-neighbour 규칙을 Rust로 이관했다. 선택한
두 개 이상의 도형에서 `r`/Ruler로 진입하면 각 bbox의 가장 가까운 이웃을 고르고,
중복 쌍을 합친 뒤 선택 index 순서·수평→수직 순서로 떨어진 축의 간격만 측정한다.
동률은 먼저 선택된 이웃, 닿거나 겹친 축은0개다. 겹치는 다른 축은 그 교집합의
중간점, 겹치지 않으면 두 bbox 중심의 중간점에 치수선을 놓는다. **다각형 윤곽 간
최단 거리나 union span이 아니다.** UI에 bbox 측정임을 명시한다.

### 계산·표시 경계

- 기존 owner WebSocket `view.measure_selection`의 body는 `anchor`와
  `boxes_dbu:[[x0,y0,x1,y1],…]`다. 최대64개, 좌표는 canonical i64 문자열이고
  뒤집힌 bbox·추가 필드·잘못된 타입을 거부한다. 기존 seq/view/connection 검증과
  표시 ACK+writer receipt·controller current anchor 검증을 동일하게 적용한다.
- bbox는 **이미 접수한 선택 주석**이다. 서버는 해당 bbox가 원본 도형인지 다시
  검색하지 않으며 새 geometry 권한을 부여하지 않는다. 브라우저는 Inspector의
  유효 선택 snapshot만 전송한다. layout pick은 그대로 exact scene 제약을 따른다.
  jobdeck geometry pick을 새로 지원한다는 뜻이 아니다.
- `measure_selection.result`는 seq/view/epoch/anchor와 `segments`를 반환한다.
  각 segment의 endpoints_dbu/Δx·Δy µm/distance_um 문자열 형식은 §5와 같다.
  최대4032 bbox 비교·128개 segment로 유계이며 새 worker query/render·파일 I/O·
  서버 주석 history는 없다. owner 인증·listener·공유 권한·native wire도 그대로다.
- i64 bbox 합과 차이는 i128로 먼저 계산한다. 중간점은 정확한1/4 DBU 정수로
  저장하고 `.25/.5/.75` 문자열을 보존한다. 따라서2^53 위의1 DBU 간격이나
  i64 양끝 평균을 절대좌표의 f64 반올림으로 잃지 않는다. 최종 µm 거리와 browser
  Canvas 투영은 여전히 f64다. 임의 정밀도 화면 투영·contour 측정을 주장하지 않는다.

### 조작과 비동기 순서

- mode에 다시 들어가 선택이2개 이상이면 이전 auto set을 제거하고 새 측정을
  요청한다. 수동 ruler는 유지한다. 선택이0~1개이면 기존 auto set도 유지한다(GTK).
  매 hover/pan에서 자동 gap을 다시 계산하지 않는다.
- 요청은 최신1개·8초 timeout이다. 선택 내용/순서, 표시 anchor, view/connection,
  모드 종료·취소가 바뀌면 늦은 결과를 폐기한다. 오류/timeout은 명시 안내하며
  이전 결과를 새 측정처럼 되살리지 않는다. 완료 auto는 수동과 같이 world 주석으로
  pan/zoom에 남고 view/dataset/worker 변경·종료에서 해제된다.
- 수동(최대256), auto(최대128), CD(최대3)를 하나의 생성 순서에 둔다. `k`/Undo는
  가장 나중 항목을 삭제한다. `Shift+K`/Clear와 mode 밖 ruler Escape는 전체를
  지운다. pending point → ruler mode → 완료 rulers의 Escape 순서는 유지한다.
  DRC box mode의 Escape와 CD 전용 삭제 버튼은 각각 기존 목적을 유지한다.
- auto 요청은1개 pending 슬롯, CD jump는 최대3개 pending 슬롯으로 순서를
  예약한다(상태줄 ruler 수는 이 예약도 포함). 결과가 도착하면 **같은 위치**에서
  실제 개수로 교체한다. 대기 중 더 나중에 만든 수동 ruler보다 뒤로 옮기지 않는다.
  pending auto/CD를 삭제하면 해당 요청을 무효화한다. CD 미완료 상태의 `k`는
  기존 의미대로 그 pending CD 묶음 전체를 취소한다. 복원 중 CD는 기존 보호를
  유지하고, 복원 완료 후 삭제하도록 명시 안내한다.
- 수동/auto/CD 및 미리보기를 기존 ruler Canvas의 **한 번의 label 배치**로 그린다.
  CD의 µm→DBU 투영은 표시 프레임의 DBU를 쓰며14 CSS px offset·DPR/crop·clipping은
  공통 renderer가 맡는다. DRC canvas는 marker/윤곽/box만 그린다. CD 숨김은 유지한다.
  작은 창에서 충돌 없는 라벨 자리가 없을 수 있으므로 panel의 텍스트 값도 남긴다.
- 기존 local UI/theme·인증·정적 자산 구조를 재사용했다. 새 npm/vendor 의존성,
  hosting, public endpoint, 원본/캐시 변경은 없다. native renderer 버전0.12.87은
  유지하고 앱/embedded bundle을 재빌드한다.

### 검증과 남은 범위

- 단위: 이웃 동률·중복·순서, 겹침/접촉/축퇴 bbox, 대각선 교차 중간점, 음수,
  i64 극단의1 DBU 간격·반 DBU, canonical/bounds/count 오류를 고정했다.
- `validate_worker_queries.py`가 GTK 파일에서 **순수 `_measure_selection` 함수만
  AST로 추출**해23개 고정·seeded 입력(0~64개 bbox)의 오라클을 생성한다. 실제
  owner socket/Rust 결과의 모든 치수선 끝점·순서·거리를 비교한다. GTK/Python은
  개발 gate에서만 사용하고 앱 실행 경로에는 추가하지 않았다.
- `WEB SELECTION RULERS: ALL OK`를 필수 marker로 추가했다. 표시 ACK 이전·pan
  이후 stale 요청 거부, 추가 query/render0, 입력 오류, 기존 index/summary bytes·mtime
  불변과 worker 임시 파일 회수를 함께 검사한다.
- ES2017/DOM·Canvas 하네스는 auto 교체/보존, 취소·timeout·위조/지연 응답,
  혼합 생성 순서, CD 비동기/복원 순서·숨김, 한 번의 label pass·단위 투영을 확인한다.
  실제 DRC 모듈의 독립/공통 history 경로를 모두 실행한다.
- clip·overlay/export·수동/auto 주석의 새로고침 복원·전체 조작 parity는 아직 남는다.
  실제 browser 시각/조작, 현장 Firefox/ETX, pack-build 승인 클릭, 공유 권한은
  별도 보류/승인 대기다. M4 전체 완료나 GTK 기본값 교체를 선언하지 않는다.

검증 결과(2026-09-14): app6·app-core96·web36·transport8, scoped fmt/strict
clippy, ES2017와 전체 UI 테스트, 실제 native1/controller2/owner WS5개가 통과했다.
전체 `sh tools/validate_rust.sh`도 `RUST VALIDATION: ALL OK`이며 jobdeck80·renderer46,
KLayout13 PX +2 phase-exact +14 style의 jobs1/8 검증을 포함한다. 이후 추가한
실제 app.js의 Inspector→auto gap→공통 history 연결 검사도 전체 UI 재실행으로 통과했다.
Rust1.89.0에서 app6/core96/web36/transport8을 재검증했고 macOS release와
Linux x86-64 musl static-pie 교차 빌드도 통과했다. Linux 실행과 실제 브라우저·
TeeBox 수용은 이 결과에 포함하지 않는다. 기존 의존성 경고는 남는다.

다음 독립 단계는 일반 layout의 exact clip CLI다. 기존 native clip은 full depth·
cut0이며 jobdeck 미지원이다. Rust process client의 private artifact/timeout·종료,
앱의 source/cache 경로 보호·원자적 게시를 연결하고 Region XOR·jobs 바이트 게이트로
검증한다. 이 단계에 웹 download·공유 권한·GTK 기본값 교체를 섞지 않는다.

## 7. M4b-1: 일반 layout exact clip CLI

### 범위와 사용

```sh
rust/target/release/floe2-web clip design.oas \
  --bbox=100,200,150,260 --layers 7/0,8/3 \
  --cell-name FLOE_CLIP --out '선택 영역.oas'
```

- 기존 VFS cache에서 **full depth·cut0·exact** geometry를 내보낸다. 현재 화면,
  detail/thin·LOD/occupancy·frames·labels·raster jobs 설정과 무관하다.
  `--exact`는 기존 CLI와 같은 호환 플래그다. 인덱스를 자동 생성하지 않는다.
- `--bbox`는 µm이며 nearest/ties-even DBU, 역방향 꼭짓점 정규화, DBU 반올림 후
  0면적·비유한 수·i64 overflow 거부. clip 경계와 geometry 교차의 native 반올림
  규칙은 이 입력 좌표 변환과 별개이며 변경하지 않았다.
- 레이어 이름/alias와 `layer/datatype`을 지원한다. 생략/`all`/빈 문자열은 전체다.
  `--layers ','` 같은 빈 토큰 목록도 **기존 clip처럼 전체**다. Python adapter가
  pick/snap의 `_query_layers`를 공유하는 규칙을 유지하며 render의 `none`과 구별한다.
  낮은 수준의 Rust `ClipRequest`는 명시 `Layers::None`도 표현할 수 있다.
- `--out` 기본 `clip.oas`, `--cell-name` 기본 `FLOE_CLIP`. 이름은 공백·한글 허용,
  UTF-8 1~4096 bytes/control 없음. protocol에는 hex로 보내 shell 해석이 없다.
- jobdeck clip은 기존처럼 미지원이며 `.jb` 입력에 명시 오류를 낸다.
  소스 마스크 OASIS를 직접 clip할 수 있다. 웹 download/API/UI는 추가하지 않았다.

### 자원·수명·게시

- `floe-worker-client::clip`은 opened layout·idle worker를 요구한다. render generation과
  query가 남아 있으면 Busy다. CLI는 별도 worker를 쓰고 style/render 명령을 보내지 않는다.
- `FLOE_RUST_JOBS`(기본 min(CPUs,8), 1~256)·기존 decode LRU budget을 사용한다.
  `FLOE_RUST_OPEN_TIMEOUT_S`와 `FLOE_RUST_CLIP_TIMEOUT_S`는 각각 기본 300초,
  범위 1~86400이다. 이 제한은 native 응답 대기 시간이며 큰 파일의 후속 복사 I/O
  시간까지 보장하는 deadline은 아니다. SIGINT/SIGTERM을 open/clip 대기와 복사에서 확인한다.
- 워커가 쓸 수 있는 목적지는 앱이 발급한 private `clip-N.oas`뿐이다. 성공 응답의
  seq·크기·records=rects+polys·시간 필드를 검증한다. EOF/timeout/손상 응답이면 worker를
  quit→grace→kill/wait로 종료하고 owned workspace를 정리한다. ENOSPC는 성공/빈 clip으로
  위장하지 않는다. source alias는 unlink할 뿐 원래 cache를 지우지 않는다.
- `O_NOFOLLOW|O_NONBLOCK`으로 연 private regular file(nlink=1)의 실제 길이,
  **버전 고정 native writer의 START/unit/CELL-name/END envelope**를 확인한다.
  이는 임의 OASIS의 전체 문법/기하 검증이 아니다. 실제 기하는 독립 KLayout XOR gate가 맡는다.
  descriptor를 소유한 채 이름을 unlink하고 worker를 reaping한 다음 스트리밍 게시한다.
- 원본·cache 내부·cache lock·symlink 출력·부모 alias를 거부한다. 긴 clip 뒤에도
  목적지를 재검사하고 같은 디렉터리의 create-new/0600 임시 파일에 최대 1MiB씩 복사한다.
  선언 길이보다 짧거나 길면 실패하며 이전 출력은 보존한다. sync→취소 확인→rename이
  commit point이며 rename 성공 뒤 도착한 취소를 미게시 오류로 돌리지 않는다.
- source가 stale이면 기존 read CLI처럼 경고하고 cache geometry를 사용한다.
  **현재 소스의 재해석이 아님**을 경고문에 적으며 marker/version 손상은 오류다.
  인덱스 갱신/외부 summary 교체 수명 문제는 이 단계에서 바꾸지 않는다.

클라이언트는 OASIS 전체를 두 번째 Vec으로 읽지 않지만 native `ClipGeometry`와
OASIS writer는 여전히 전체 산출 geometry/encoded bytes를 보유한다. 이 구현을
native clip의 총 RSS 상한/완전 스트리밍 개선으로 해석하면 안 된다. 출력 디스크 여유도
private 파일+목적지 staging(+교체 전 파일)을 포함해 필요하다.

### 게이트

- worker fake-daemon: style 없이 연속 clip, UTF-8/issued slot, worker 종료 뒤 descriptor,
  잘못된 seq/count/크기/magic/unit/cell/END, short/symlink/hardlink/directory,
  오류/EOF/timeout/중단, 원본/외부 sentinel과 workspace 정리.
- 앱 unit: bbox DBU ties-even/역방향/overflow, 화면 옵션 거부; stream unit:
  짧음/늘어남/read 오류/복사 중 취소에서 이전 출력·임시 파일 보호와 1MiB 유계 복사.
- `tools/validate_app_clip.py`: private valmini와 concave/비대칭 path/반복·회전·반사
  fixture의 Python adapter·jobs1/8 **OASIS 바이트 일치 + 원본 KLayout Region XOR**,
  half-DBU·긴 UTF-8 셀명·레이어 이름·기본 출력·빈 뷰, source/cache alias 보호,
  실 process SIGINT/SIGTERM(open/ready/clip)·timeout·ENOSPC·손상 출력·cache·stale 경고.
  Rust runtime의 PATH는 비우며 Python/KLayout은 개발 오라클에서만 쓴다.
- 이 gate는 `tools/validate_rust.sh`에 필수 배선했다. 실제 브라우저/현장 TeeBox,
  Linux 실행을 이 로컬 검증으로 대체하지 않는다. 최종 실행 결과는 아래에 기록한다.

실행 결과(로컬): 전체 `sh tools/validate_rust.sh`가 `RUST VALIDATION: ALL OK`다.
새 clip 게이트 외 기존 jobdeck80·renderer46, query/owner/웹 UI, KLayout13 PX +
2 phase-exact +14 style의 jobs1/8 검증도 통과했다. 마지막 스트림 `Interrupted`
재시도/취소 보강과 기존 PNG 게시의 무복사 경로 유지 후 app7/core99/web36/transport8,
worker-client unit7/lifecycle14를 재실행했고, 실제 clip·기존 PNG/report 오라클,
scoped 포맷·strict clippy, Rust1.89.0 테스트·macOS release·Linux x86-64 musl
static-pie 교차 빌드도 다시 통과했다. native geometry와 renderd wire/version은
바꾸지 않았다. 기존 의존성/개발 오라클의 warning은 남으며 실 Linux 실행이나
현장 Firefox 수용을 완료한 것으로 보지 않는다.

다음 독립 단계는 render CLI의 batch/mosaic와 관련 report·PNG metadata 이관이다.
웹 clip·공유 권한·현장 수용과 구분해 진행한다.

## 8. M4b-2: batch·mosaic 캡처 CLI

### 범위와 사용

```sh
rust/target/release/floe2-web render design.oas \
  --batch shots.txt --out captures --report captures/report.json
rust/target/release/floe2-web render design.oas \
  --corners=0,0,100,100 --size=10,10 --px=600x400 \
  --line=0.5 --line-color='#ffffff' --keep-tiles --out corners.png
```

batch 예시(좌표는 µm, 기존 단위 suffix도 허용):

```text
# 한 줄에 이름과 key=value. shell을 실행하지 않는다.
'영역 A' bbox=0,0,100,80 px=800x640 layers=7/0 depth=full
four mosaic='10,90;90,90;90,10;10,10' size=10,10 px=400 keep_tiles=yes
```

- 일반 layout과 jobdeck 모두 기존 `render` 경로를 공통 Rust capture runner로
  옮겼다. `--batch -`는 stdin, 한 줄짜리 batch도 `--out`은 디렉터리다.
  단일 이미지의 기본값·레이어/depth/detail/thin·frames/labels와 JSON 의미는 유지한다.
  덱 labels는 기존처럼 명시 미지원이며, 인덱스 자동 생성이나 원본 변경은 없다.
- batch는 POSIX `shlex.split(comments=False)` 상당의 인용/escape를 직접 파싱한다.
  전체 줄 `#` 주석만 허용하고 `#rrggbb`를 inline 주석으로 오인하지 않는다.
  CLI 기본값을 상속하되 줄에 영역 키가 있으면 기존 bbox/at/mosaic/corners를 모두
  비운 뒤 그 줄 값을 적용한다. 같은 키는 마지막 값이 이기며 알 수 없는 키는 오류다.
  전역 detail/thin·frames/labels/font는 batch 키가 아니다.
- `--mosaic-at`은 TL,TR,BR,BL 시계방향 입력이고, 결과와 kept tile 이름은
  TL,TR,BL,BR 행 우선이다. `--corners`는 지정 bbox의 **안쪽** 네 모서리를 캡처한다.
  tile마다 기존 aspect/anchor 규칙을 적용하고 첫 tile의 픽셀 크기를 공통 사용한다.
- 구분선은 기존 세로→가로 순서, 소수 px coverage와 ties-even 색상 혼합을 유지한다.
  폭이 이미지보다 훨씬 커도 작업은 이미지 행/열 수에 비례한다.
  report의 bbox/tiles·순서·pixel·line 정보와 complete/over-budget/skipped 의미도 유지한다.
- 일반 shots에는 원래 PNG annotation metadata가 없었다. `fe_embed` iTXt 보조 CLI와
  DRC marker/legend/ruler 캡처는 **아직 남아 있다**. 웹 내보내기/API는 추가하지 않았다.

### 비용·실패·파일 계약

- batch 전체가 한 native worker와 style을 재사용한다. 일반 shot은 native PNG를,
  mosaic은 네 raw tile을 순서대로 받아 canvas에 복사한다. raw tile 한 장을 처리한
  뒤 버리며 네 tile 전체를 별도 Vec으로 유지하지 않는다. kept tile은 즉시 staging한다.
- tile은 기존 최대16M pixels, 최종2×2 mosaic은 최대64M pixels(256MiB RGBA canvas)다.
  canvas 외 raw tile 한 장과 worker/cache 메모리는 별도다. 전체 프로세스 RSS의
  256MiB 상한을 주장하지 않는다. PNG는 RGBA8/filter0/single IDAT를 staging 파일로
  스트리밍하고 IDAT 길이·CRC를 마감한다. 전체 filtered/compressed 복사본은 만들지 않는다.
  이미 vendored된 `flate2`/`crc32fast`를 사용하며 새 다운로드나 vendor 수정은 없다.
- batch 입력 최대16MiB·4096 shots, 이름 최대247 UTF-8 bytes다. 안전한 파일명·
  고유 이름을 요구하며 상한 초과는 오류이지 prefix 캡처/조용한 잘림이 아니다.
  `--batch -`의 입력 생산자가 EOF를 보내지 않아도 SIGINT/SIGTERM으로 CLI를 끝낸다.
  stdin reader의 비-join 종료는 CLI process 전용이며 HTTP 작업에 사용하지 않는다.
- 모든 shot/레이어·목적지를 worker 시작 전에 검사한다. batch 디렉터리만 명시 생성하며,
  별도 report 부모는 미리 존재해야 한다. 원본·색상 입력·cache/lock·batch 파일,
  symlink 출력/부모 alias를 보호한다. shot/report/kept tile의 동일·대소문자만 다른
  이름과 파일/부모 경로 충돌도 거부한다. 대소문자 규칙은 Linux에서도 보수적으로 적용한다.
- 한 mosaic의 네 렌더와 모든 PNG staging을 성공시킨 뒤 main/kept tile을 게시한다.
  그 전 ENOSPC·손상/unknown partial·중단은 이전 main/report/kept 파일을 보존하고
  owned staging·worker 파일을 회수한다. 각 파일은 create-new/0600→sync→취소 확인→
  rename이 commit point다. **여러 파일/shot 전체가 하나의 트랜잭션은 아니다.**
  이후 shot이나 후속 rename 실패는 이미 저장한 artifact 수를 명시한다.
  report는 마지막에 원자 게시하며 실패하면 PNG가 저장됐음을 오류에 적는다.
- 일반 layout의 미완료 frame은 게시하지 않는다. 덱의 알려진 deferred/skipped는
  기존처럼 INCOMPLETE 이미지/report와 exit3으로 보존하되 각 tile의 deferred를 합산한다.
  원인 없는 partial/라벨 잘림은 성공으로 내보내지 않는다.

### 검증

- `validate_app_captures.py`를 필수 배터리에 추가했다. PATH-empty Rust runtime으로
  layout/jobdeck·단일/다중 batch·stdin·한글/공백·단위·역방향 corners·half phase·
  labels/frames·분수/큰 구분선·kept tiles를 검증한다. Python 오라클은 개발에만 쓴다.
  PNG decoded RGBA와 report는 Python과 일치하고 Rust jobs1/8의 PNG bytes도 일치한다.
  Python과 Rust PNG의 압축 bytes 동일성은 계약이 아니다.
- fake worker는 ready/open/style 각1회·gen1..5의 단일 worker 재사용, 네 tile의
  deferred 합산, 두 번째 tile 실패와 unknown partial, ready/open/style/render 중
  SIGINT/SIGTERM·reap·기존 출력 보존을 검사한다. EOF 없는 stdin 취소도 검사한다.
  잘못된 옵션/파일명/레이어·경로 충돌은 새 디렉터리/worker 없이 거부하는지 확인한다.
- batch 문법/입력 상한과 mosaic blend/PNG CRC·inflate는 단위로 고정한다.
  기존 단일 PNG·덱 render·exact clip/공통 게시 회귀와 cache bytes/mtime 불변도 유지한다.
- GTK 기본값, native geometry/wire/renderd 버전0.12.87은 바꾸지 않는다.
  현장 Firefox/ETX·Linux 실행·공유 권한·실제 pack-build 승인 클릭은 별도 보류다.

실행 결과(2026-09-14): 전체 `sh tools/validate_rust.sh`가 `RUST VALIDATION: ALL OK`다.
새 capture 게이트와 기존 jobdeck80·renderer46·웹/owner/query, KLayout13 PX +
2 phase-exact +14 style의 jobs1/8 검증을 포함한다. 마지막 구분선 무할당/PNG CRC·
staging/후속 shot 실패 보강 후 app7/core103/web36/transport8 및 worker-client
unit7/lifecycle14, 새 capture·기존 단일 PNG·exact clip 오라클을 다시 통과했다.
scoped fmt/strict clippy, Rust1.89.0의 같은 테스트, macOS release와 Linux x86-64
musl static-pie 교차 빌드도 통과했다. 기존 의존성/개발 오라클의 warning은 남는다.
실 Linux 실행이나 현장 Firefox 수용을 이 결과로 대체하지 않는다.

다음 독립 단계는 픽셀 chunk를 유지하는 PNG 주석 metadata codec과 `fe_embed`
보조 CLI다. 이후 DRC overlay/legend/ruler 캡처를 연결하며, 웹 download·공유 권한과
일반 캡처의 Python 제거 완료를 혼동하지 않는다.

## 9. M4b-3: PNG 주석 metadata와 fe-embed CLI

### 기존 명령의 Rust 대응

```sh
# python -m floe.fe_embed 대신 사용한다. 이미지 파일은 명시적으로 in-place 편집한다.
rust/target/release/floe2-web fe-embed --json annotations.json shot.png
rust/target/release/floe2-web fe-embed --append \
  --ruler=0,0,100,0 --ppu=8 --unit=um --note='확인\n두 번째 줄' shot.png
rust/target/release/floe2-web fe-embed --dump shot.png
rust/target/release/floe2-web fe-embed --strip shot.png
rust/target/release/floe2-web fe-embed --selftest
```

- `--box/--ellipse/--line/--path/--polygon/--ruler/--text`, `--json FILE|-`,
  `--legend`, `--note`, `--ppu/--unit`, `--append/--dump/--strip/--selftest`,
  여러 위치 인자 PNG와 `--`를 연결했다. 주석 옵션 순서 뒤에 JSON 주석을 붙이며,
  명시 ppu/unit/note/legend가 JSON metadata보다 우선한다. dump가 strip보다 우선하고
  둘은 사용하지 않는 JSON/legend 파일을 열지 않는다. shell/glob 확장은 shell의 몫이다.
- 좌표는 **이미지 중심 원점의 pixels, x 오른쪽/y 아래**다. PNG pixels나 OASIS
  DBU 좌표를 자동 변환하지 않는다. `ppu`가 ruler scale이며 unit 기본값은 `um`이다.
  `%.10g` 좌표, 팔레트/hex 대소문자 계약, polygon fill alpha·pattern,
  casing/dash·text backdrop와 escape, legend 별칭/순서를 Python과 대조했다.
- `iTXt` keyword `flateyes`와 기존 key=value 형식을 유지한다. 압축/비압축 기존
  metadata를 읽고 새 것은 비압축 UTF-8로 쓴다. append는 첫 flateyes chunk의
  알려지지 않은 줄도 보존하되 새 ppu는 기존 ppu/unit, 새 note/legend는 해당 필드만
  대체한다. 기존 flateyes chunk는 모두 제거하고 IEND 바로 앞에 하나를 넣는다.
- 픽셀 IDAT·다른 text/ancillary chunk·IEND 뒤 bytes는 그대로 복사한다.
  strip은 metadata만 제거한다. IDAT를 이미지로 decode하거나 pixels를 재압축하지 않는다.
  `--selftest`는 일곱 종류·한글·PNG round-trip/strip의 native in-memory 검사이며
  Python/flateyes import를 시도하지 않는다. 실제 상호 운용 검증은 개발 오라클이 맡는다.
- `Annotation`은 검증된 metadata 줄, `Document`는 주석/ppu/unit/note/legend다.
  웹 overlay의 geometry 모델이나 편집 상태 저장 기능이 아니다. 기존 DRC 캡처의
  marker·CD ruler·레이어 legend 조립과 `render --drc*` 연결은 다음 단계다.

### 파일 수명과 명시적인 한계

- PNG 최대1GiB·65536 chunks, 주석 입력/출력 및 한 PNG의 flateyes **총 해제된 text**
  최대16MiB,100k annotations, 한 path/polygon 최대1M points, CLI 최대4096 PNG다.
  메모리 상한을 뜻하는 숫자와 파일/레코드 상한을 혼동하지 않는다(JSON 모델과
  metadata 문자열은 별도 보관). pixels를 위한 frame-size Vec은 없고 PNG I/O는
  최대1MiB 블록이다. 추가 후의 PNG도 같은 파일/chunk 상한 안이어야 한다.
- PNG signature/IHDR 필드·chunk 경계·CRC·IEND·flateyes iTXt/UTF-8를 검사한다.
  **전체 PNG 이미지 유효성 검사기는 아니다**: IDAT DEFLATE와 실제 image samples는
  해석하지 않는다. 다른 소유자의 iTXt를 압축 해제하거나 다시 직렬화하지 않는다.
- Python의 일부 관대한 입력은 의도적으로 거부한다: nonfinite/0 이하 ppu,
  잘못된 CRC/iTXt, metadata 한계를 넘는 압축/중복 chunk, 조용히 필드를 잘라야 하는
  잘못된 point 배열, 줄 형식을 깨는 control/U+2028/U+2029. JSON의 text/unit/note/
  legend 문자는 문자열이어야 한다. 정상 형식의 bytes 계약과 손상 입력 관용을 구별한다.
- 쓰기 목적지의 symlink/FIFO/directory, hardlink 및 중복/alias 목적지는 거부한다.
  읽기 전용 dump는 명시한 symlink/중복 경로를 읽을 수 있다.
  `O_NOFOLLOW|O_NONBLOCK` regular descriptor를 사용하며 기존 PNG의 dev/inode·길이·
  mtime/ctime·mode/link 수를 읽기 후와 최종 교체 전에 확인한다. 변경되면 현재 파일을
  덮어쓰지 않는다. 이는 **cross-process compare-and-swap/편집 lock이 아니다**.
  동시 외부 편집·부모 디렉터리 교체를 조정하는 서버/클라이언트 저장 계약은 남는다.
- 같은 디렉터리의 create-new staging→스트리밍 복사→sync→취소 확인→rename이다.
  SIGINT/SIGTERM·입출력 오류·형식 오류는 해당 PNG를 보존하고 owned 임시 파일을
  제거한다. 기존 Unix rwx mode만 유지하고 setuid/setgid·ACL/xattr는 복제하지 않는다.
  여러 PNG 전체는 트랜잭션이 아니며 후속 실패 시 이미 갱신한 PNG 수를 명시한다.
  일반적인 파일 I/O 지연 자체의 절대 deadline을 보장하지는 않는다.
- EOF 없는 `--json -`도 공유 bounded stdin reader로 취소한다. reader의 process 종료
  방식은 CLI 전용이다. 명령행 입력 오류는2, 파일/JSON 실행 오류는1,
  취소는128+signal이다. 기존 `floe2`/GTK·renderd wire/version·인덱스는 변경하지 않는다.
  새 HTTP/다운로드 endpoint·공유 권한·브라우저 실행 경로는 없다.

### 검증

- 필수 `tools/validate_fe_embed.py`: unchanged Python `fe_embed`가 만든 기대 PNG와
  **전체 bytes·추출 metadata·Pillow pixels**를 대조한다. 일곱 종류·표시 스타일·
  250개 seeded 극단 좌표/10자리 반올림·한글·공백·JSON stdin·legend/명시 우선순위·
  append/strip·압축/중복 metadata·알 수 없는 줄과 ancillary·복수 IDAT·trailer를 검사한다.
  1/L/P/RGB/RGBA/LA/I;16 PNG도 비교한다. Rust runtime은 PATH-empty이며 native
  binary override도 무효값으로 두어 renderd/indexer/Python 실행이 필요 없음을 확인한다.
- 손상 signature/header/chunk/CRC/iTXt/UTF-8·압축 bomb/중복 합산·파일/chunk 상한,
  option/JSON 오류, symlink/hardlink/FIFO/alias, 후속 PNG 실패를 검사한다.
  자식의 RLIMIT_FSIZE로 staging 쓰기 실패를 주입하고 원본·임시파일 보호를 확인한다.
  EOF 없는 stdin 및64MiB ancillary 복사 중 실제 SIGINT/SIGTERM도 검사한다.
- native unit은 출력 한계 유지, append 순서·metadata 상한, 원본 inode 교체 감지,
  취소, CRC/round-trip을 고정한다. 기존 batch stdin과 PNG/clip 게시 회귀도 유지한다.
- 압축 metadata는 zlib StreamEnd(Adler trailer 포함)까지 확인하며 모든 잘린 prefix를
  거부한다. 일반 Read의 EOF만으로 정상 압축 종료를 판정하지 않는다.

실행 결과(2026-09-14): 전체 `sh tools/validate_rust.sh`가 `RUST VALIDATION: ALL OK`다.
새 fe-embed·기존 capture/clip, jobdeck80·renderer46, KLayout13 PX +2 phase-exact
+14 style의 jobs1/8 검증을 포함한다. 최종 PNG bytes 오라클과 batch capture 오라클도
재실행해 통과했다. app8/core108/web36/transport8 및 worker-client unit7/lifecycle14,
scoped fmt/strict clippy, Rust1.89.0의 같은 테스트, macOS release와 Linux x86-64
musl static-pie 교차 빌드가 통과했다. 기존 의존성 warning은 남는다.
현장 Firefox/ETX와 실 Linux 실행은 미검증이며 M4 전체 완료가 아니다.

다음 독립 단계는 DRC marker·CD ruler·레이어 legend metadata 조립과
`render --drc*` 연결이다. 웹 다운로드/공유 권한이나 review 쓰기까지 완료한 것은 아니다.

## 10. M4b-4: DRC 오류별 PNG 캡처

```sh
rust/target/release/floe2-web render design.oas \
  --drc results.db --drc-rule 'M1.WIDTH' --drc-err 1-20 \
  --px 1200 --out width.png
# 결과: local<TAB>global<TAB>path. 여러 장은 width_1.png … width_20.png.
# --layers가 없으면 기존 SVRF sidecar로 레이어를 격리한다.
rust/target/release/floe2-web render design.oas \
  --drc results.db.ice --drc-rule 'M1.WIDTH' --drc-err 3 \
  --layers all --floe-reviewer reviewerA --out one.png
```

### 표시와 CLI 계약

- `--drc/--drc-rule`, `--drc-err N|A-B|all`, `--drc-cap`(200),
  `--drc-frac`(.3), `--drc-rules`, `--floe-reviewer`를 연결했다.
  중복 룰 이름은 경고 후 첫 블록을 쓴다. all만 cap을 적용하고 명시 범위는
  끝 번호를 실제 개수에 맞추되 cap으로 자르지 않는다. 빈 선택/0 cap은 오류다.
  인덱스 범위를 Vec으로 펼치지 않고 한 오류씩 읽는다.
- 정사각형은 오류 bbox의 긴 변/fraction(유한 값만, .02..1 clamp), 축퇴 오류는
  0.1 µm다. `--px WxH`도 기존처럼 W×W로 쓴다. worker bbox는 DBU half-even
  정수 반올림, metadata는 반올림 전 µm bbox에서 중심 원점/y-down pixels로
  변환한다. round 후 축퇴/overflow는 오류이며 임의의 확대·좌표 clamp를 하지 않는다.
- **기존 Rust DRC 경로의 실제 기본값:** full depth, cut=0, live speckle/사용자
  패턴·outline 스타일, frames/labels on(덱 labels off). 일반 shot의 archival
  solid/1px·frames/labels off와 다르다. Python docstring의 "viewer detail"만 보고
  cut=3 같은 값을 추정하지 않았다. `--depth`는 기존대로, 명시 `--detail/--thin/`
  `--label-font-px`는 새 Rust CLI에서 실제로 반영한다(기존 DRC CLI는 후자들을 무시했다).
- 픽셀에 마커를 굽지 않는다. 기존 flateyes iTXt에 active `#FF5252`/waived
  `#00E676`, 2px casing 없는 선/다각형을 넣는다. 다각형 ≤256점만 solid alpha128
  내부 채움, 그 이상은 윤곽만이다. 단순 도형의 기존 CD 계산을 재사용하고 한 edge의
  ruler는 endpoint 순서와 무관하게 위쪽/오른쪽 법선으로14px 옮긴다.
  note는 `RULE #local(global)` 및 waived 표식, ruler scale은 ppu/unit=um이다.
- 명시 `--layers all`은 자동 격리를 우회한다. 생략 시 CLI에서만
  `--drc-rules` → 기록된 deck basename의 인접 `.rules.json` → 기록된 deck 경로의
  `.rules.json` → `<drc 인자>.rules.json` 순서를 사용한다(명시 경로는 자동 후보와
  합치지 않는다). 실패/룰 없음/가시 레이어 미일치는 경고 후 all이다.
  all/none에는 범례가 없고 선택 레이어는 디자인 색·fill 이름·이름·L/D 순으로 쓴다.
  원격 요청에 임의 경로나 이 자동 발견을 노출하지 않는다.
- layout과 complete jobdeck 공통 runner다. DRC와 region/batch/mosaic/report
  옵션의 혼용은 조용히 무시하지 않고 거부한다. 기본 파일명은 룰 이름을 안전한
  문자로 바꾼 이름, 여러 장은 룰-local 번호 suffix다. 부모 디렉터리는 자동 생성하지 않는다.

### 수명·안전·비용

- 모든 목적지를 native worker 시작 전에 검사한다. 원본/cache/lock·layerprops·
  DRC 인자/선택 pack·reviewer waive·rules 후보와 출력의 충돌, symlink/hardlink
  목적지를 거부한다. 이미지16M pixels, 기존 DRC 읽기·metadata16MiB/100k 주석
  한계는 오류로 끝내며 prefix 마커나 unannotated PNG로 성공하지 않는다.
- 하나의 live worker/style를 재사용하고 오류 하나의 geometry/PNG만 처리한다.
  worker-client의 요청별300초 render deadline을 사용한다(기존 Python의 첫300초/
  이후120초 idle 대기와 다름). ready/open/style deadline과 종료/reap도 공통 client가 맡는다.
  문법 오류·미완료/라벨 잘림·손상 PNG·잘린 ASCII 입력은 정상 캡처로 게시하지 않는다.
  skip ledger가 있는 덱도 DRC 캡처에서는 미완료 오류다(일반 shot의 flagged PNG와 구별).
- 완전한 PNG를 `annotations::png::stage`로 원본 chunk를 유지하며 주석과 함께
  staging→sync한 뒤 입력 DRC identity/취소를 확인해 한 번 rename한다.
  PNG를 먼저 덮어쓴 뒤 metadata를 추가하는 창은 없다. 실패/중단은 현재 출력과
  owned 임시 파일을 보호한다. 여러 PNG 전체의 트랜잭션은 아니므로 후속 실패는
  이미 저장한 장수를 오류에 포함한다. 외부 파일 편집과의 CAS/lock은 별도 계약이다.
- read-only `open_current`를 재사용한다. fresh pack/기존 waive는 읽기만 하고,
  stale/corrupt 인접 pack은 경고 후 ASCII fallback한다. pack/review 자동 생성이나
  review/notes 쓰기, 새 HTTP/download 경로는 추가하지 않았다.

### 검증

- 필수 `validate_drc_captures.py`: private valmini/DRC/pack/waive·fractional ASCII·
  thin jobdeck의 Python PNG 전체 bytes·metadata·Rust jobs1/8 불변을 대조한다.
  rectangle/회전·single/pair edges·교차/축퇴·256/257점 fill 경계·상태 색·범례·
  명시 all/none·자동 sidecar 우선순위·cap/범위·fraction clamp·depth/파일명을 검사한다.
- fake worker로 open/style 각1회·연속 generation, 기본 cut/frames/labels 및
  명시 detail/thin/font 전달, partial/deferred/손상 PNG/후속 렌더 실패를 고정한다.
  SIGINT/SIGTERM을 ready/open/style/render에 주입하고 실제 reap/이전 출력 보존을
  확인한다. staging write 실패·주석 상한·잘린 ASCII·출력 alias/입력 보호도 검사한다.
  native 임시 디렉터리와 기존 Python 덱 오라클의 임시 파일은 분리한다.

실행 결과(2026-09-14): 전체 `sh tools/validate_rust.sh`가 `RUST VALIDATION: ALL OK`다.
새 DRC 캡처·fe-embed·일반/batch 캡처와 기존 jobdeck80·renderer46, KLayout13 PX
+2 phase-exact +14 style jobs1/8 검증을 포함한다. 이후 점으로만 된 룰의 기본
파일명 코너를 보강했고, 최종 실행 파일로 DRC PNG/metadata·fe-embed·batch/mosaic
오라클을 모두 다시 통과했다. 새 DRC 비교는401×401px(384px 타일 경계 통과)에서
decode/raster jobs1/8을 각각 요청한다.

최종 app9/core110/web36/transport8·worker-client unit7/lifecycle14, scoped fmt/strict
clippy, Rust1.89.0의 같은 테스트, macOS release와 Linux x86-64 musl static-pie
교차 빌드도 통과했다. 기존 의존성/개발 오라클 warning은 남고 native geometry/wire/
renderd 버전0.12.87은 변경하지 않았다. 실 Linux 실행·현장 Firefox 수용을 주장하지 않는다.

다음 CLI 이관은 SVRF subset parser/scan이다. 웹 내보내기 UI·review 저장·나머지 조작·
패키징/현장 수용은 남으며 M4 전체 완료가 아니다. GTK 기본값과 공유/승인 경계는 유지한다.
