# 웹 전환 M4 — 조작 parity 구현 기록

`feature/webui`. [상위 계획](WEBUI_PLAN.ko.md), [기능 대조표](WEBUI_M0.ko.md).
M2 공유 권한 추가와 실제 브라우저 pack-build 승인 클릭은 승인 대기이며,
M0/G2·M3 현장 Firefox/ETX는 사용자 요청대로 보류다. 이 경계를 우회하지 않고
독립적인 로컬 native 이관을 진행한다. M4 전체 완료나 GTK 은퇴를 뜻하지 않는다.

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

다음은 owner/view/connection epoch·실제 표시 packet과 이 local anchor를 연결하는
bounded transport, 안전한 query DTO와 UI pick/snap·수동 ruler/clip이다. 현재 false인
웹 capability를 구현보다 먼저 켜지 않는다. `.ovo` 외부 교체/hot reload·현장
Firefox/ETX·실칩 지연/메모리·공유 권한과 실제 pack-build 승인 클릭은 별도다.
