# 웹 전환 M4 — 조작 parity 구현 기록

`feature/webui`. [상위 계획](WEBUI_PLAN.ko.md), [기능 대조표](WEBUI_M0.ko.md).
M2 공유 권한 추가와 실제 브라우저 pack-build 승인 클릭은 승인 대기이며,
M0/G2·M3 현장 Firefox/ETX는 사용자 요청대로 보류다. 이 경계를 우회하지 않고
독립적인 로컬 native 이관을 진행한다. M4 전체 완료나 GTK 은퇴를 뜻하지 않는다.
현재는 §14의 **owner viewport clip UI**, §15의 **표시 픽셀 PNG 복사/저장과
overlay 전환**, §16의 **Rust layerprops 포맷·초기 가시성**, §17의
**열린 세션 설정 Load/Save·필드별 스타일 적용**, §18/19의
**공유 설계 기본값 게시 코어·owner 승인 API**, §20의 **게시 preview·승인·결과 UI**까지 연결했다.
§21은 DRC waive·주석 이관의 포맷/메모리 모델 단계이며 저장 API/UI 연결은 아니다.
§22에서 명시적 로컬 review 저장과 pack binding을 추가했다. §25에서 고정 reviewer를
명시한 owner의 주석 read/prepare/승인 게시 API를 연결했다. §26은 그 API의
선택 주석 편집·미리보기·명시 승인 UI다. geometry reader는 읽기 전용이다.
§27은 native waive snapshot을 기존 reader에 적용하는 내부 갱신 경로다.
§28은 그 갱신과 HTTP/선택/준비된 focus의 조회 revision 장벽을 연결한다.
§29/30에서 waive 저장의 owner 승인 API·UI와 디스크 게시/읽기 반영 receipt를 연결했다.
§31은 저장 주석의 목록 badge/이동 대상 본문을 위한 읽기 전용 projection API다.
§32에서 목록 badge와 마지막으로 이동한 오류의 주석 overlay·서버 상태 복원을 연결했다.
§33~35에서 native 전체 review import/export, owner 분할 전송 API와 파일 선택·전체 교체
미리보기/별도 승인·내보내기 패널을 연결했다.
§36은 Python-free 로컬 배포 진단과 빌드 식별이며 실제 패키지 조립은 다음 단계다.
§37에서 기존 GTK portable과 독립적인 Rust 웹 패키지 조립을 연결했다.
각 절의 미연결 표기는 해당 선행 단계 당시의 범위다.
전체 조작 parity와 실제 브라우저/현장 수용은 남아 있다.

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

## 11. M4b-5: SVRF subset parser/scan CLI

```sh
rust/target/release/floe2-web svrf deck.cal --scan -DSTACK=6LM -I rules
rust/target/release/floe2-web svrf deck.cal -o deck.rules.json \
  -DSTACK=6LM --follow-verbatim --no-env-switches
```

### 호환 범위

- 기존 `svrf`의 모든 옵션을 연결했다. `-D/-I` 반복·붙여쓰기, `--out=FILE`,
  `-oFILE`, `--` 이후 dash 파일명도 지원한다. 기본 출력은 `<deck>.rules.json`이고
  `--scan`은 양쪽 IFDEF 분기를 조사하며 `-o`가 있어도 파일을 쓰지 않는다.
- `INCLUDE`, DEFINE/UNDEFINE, IFDEF/IFNDEF의 값 비교, ELSE/ENDIF, VARIABLE,
  LAYER/MAP, derivation 연결 그래프, check의 @ 설명·측정·여러 줄 bound/연산자
  continuation을 기존 Python 상태 머신과 대조한다. 그래프는 연산을 실행하지 않고
  source GDS와 unresolved 이름만 구한다. 순환은 유한하게 처리한다.
- `-D`/DEFINE이 환경보다 우선한다. **덱이 실제 테스트한 switch만** 환경에서 읽고
  `env_switches` provenance에 남긴다. `$NAME` switch도 지원한다.
  INCLUDE의 `$VAR/${VAR}/~[/~user]` 확장은 별도이므로 `--no-env-switches`로
  꺼지지 않는다. INCLUDE 검색은 포함한 파일의 디렉터리 → `-I` 순서다.
- DMACRO/CMACRO는 확장하지 않으며 Tcl·shell·SVRF geometry를 실행하지 않는다.
  VERBATIM 안 INCLUDE는 inventory만 하고 `--scan/--follow-verbatim`일 때만 읽는다.
  unknown histogram, 중복 check의 last-wins, 괄호/comment/IFDEF drift 경고,
  동일 빈도 histogram의 최초 출현 순서까지 보존한다. OS I/O 실패의 상세 문구는
  Rust/운영체제 표현을 쓰며 Python exception 문자열과 같지는 않다. signoff 판정기가 아니다.
- `floe-svrf-rules` version1 JSON의 필드/값을 유지한다. `generated_by`는
  `floe2-web <native package version>`으로 구별하고, JSON 공백·키 배치·Unicode escape는
  Python과 바이트 동일 계약이 아니다. CR/LF/CRLF, 마지막 줄, UTF-8 replacement,
  Unicode 설명·경로·십진 숫자를 처리한다. 원래 측정 RHS의 ASCII operand 문법은 그대로다.

### 자원·파일 계약

- 입력은 regular file만 허용하며 O_NONBLOCK으로 FIFO를 기다리지 않는다.
  INCLUDE cycle은 canonical 경로로 찾되 진단에는 원래 상대/`..` 경로를 유지한다.
  빠진 root는 Python의 빈 sidecar 성공 대신 오류다. 빠진/열 수 없는 INCLUDE는
  기존처럼 경고이며, 읽는 도중 실패·변경·상한 초과는 오류다.
- 누적 입력256MiB, file visits4096, INCLUDE 깊이64, 조건부 깊이4096,
  physical/치환 line·text64KiB, metadata/치환 작업 회계64MiB, mapping/closure 확장
  회계64MiB, 그래프 작업16M, sidecar16MiB다. map65536, check별 constraints1024·
  operands/closure4096, 기존 reader의 총 rule text64KiB도 검사한다.
  정수는 signed64 파싱 후 check의 source GDS가 reader의 u32 영역인지 확인한다.
  무한대가 되는 decimal은 오류이고, 미해석 변수 bound는 기존처럼 null/raw다.
  **상한은 잘린 정상 결과가 아니라 미완료 오류다. 회계값은 프로세스 RSS 한도가 아니다.**
- DEFINE 치환은 longest-name/word-boundary/단일 pass를 유지하는 증분 trie다.
  DEFINE마다 전체 정규식을 다시 만들지 않는다. trie262144 nodes와 line당8M match
  steps를 제한하며, 조건부 활성 여부는 깊이와 무관하게 O(1)로 검사한다.
- 출력은 source·읽은 INCLUDE·검색한 missing INCLUDE 후보, symlink/hardlink,
  cache/lock과 충돌할 수 없다. 부모는 자동 생성하지 않는다. 완성된 JSON을 기존
  reader로 검증한 뒤 같은 디렉터리의 create-new0600 임시 파일에 쓰고 sync한다.
  모든 입력 identity/dev/ino/size/mtime/ctime·취소를 재확인한 뒤 rename한다.
  실패 시 이전 출력과 원본을 보존하고 자신이 만든 임시 파일만 정리한다.
  SIGINT/SIGTERM은130/143이며, 원본 외부 편집과의 CAS/lock은 제공하지 않는다.
- 새 HTTP endpoint나 browser source-path 입력은 없다. sidecar reader/등록 API는
  계속 recorded deck/include 경로를 따라가지 않는다. indexer/renderd/Python 실행도 없다.

### 검증

필수 `validate_svrf_native.py`는 **기존 `validate_svrf.py` R1–R4의 모든 parse 호출**에
실제 Python parser를 oracle로 쓰고, JSON 전체 내용·CLI 요약·scan 전문을 비교한다.
추가로 고정 seed의100개 상태 혼합, Unicode/손상 UTF-8·개행·symlink include cycle·
include-dir 순서·숫자/치환 한계·FIFO·missing root·깊은 INCLUDE·출력 충돌·
원자 저장/취소/임시 파일 정리를 확인한다. native PATH는 비워 fallback을 배제한다.
Rust 단위 테스트는 작은 상한을 주입한 오류 경로, 그래프/치환, env lazy lookup,
재귀 없는 inline block 종료, 변한 입력의 게시 거부, 기존 reader round-trip을 검사한다.

실행 결과(2026-09-14): 전체 `sh tools/validate_rust.sh`가 `RUST VALIDATION: ALL OK`다.
새 SVRF135개 JSON/scan 비교+faults, 기존 sidecar2032개 비교, jobdeck80·renderer46,
KLayout13 PX +2 phase-exact +14 style jobs1/8 검증을 포함한다. 과거 CLI 게이트의
"svrf 미지원" 단언도 실제 native scan/변환 검증으로 갱신했다.

최종 app11/core119/web36/transport8·worker-client unit7 및 lifecycle 검사,
scoped fmt/strict clippy, Rust1.89.0의 같은 패키지 테스트, macOS release와 Linux
x86-64 musl static-pie 교차 빌드가 통과했다. 기존 의존성·개발 오라클 warning은
남는다. native geometry/wire/renderd 버전0.12.87은 바꾸지 않았다.
실 Linux 실행·현장 Firefox/ETX 수용을 주장하지 않는다.

다음 독립 작업은 owner 웹 clip/내보내기 연결이다. review/주석 저장·나머지 조작·
GTK 진단 대체·패키징/현장 수용은 남는다. 공유 권한·실제 pack-build 승인 클릭은
보류 경계를 유지하며 M4 전체 완료나 GTK launcher 교체를 뜻하지 않는다.

## 12. M4c-1: 관리형 exact clip·만료 파일 소유 코어

owner 웹 내보내기의 선행 코어를 `floe-app-core::exports`에 추가했다.
**이번 단계는 HTTP/UI 연결 완료가 아니다.** 기존 CLI clip은 같은 native 수집 코어를
사용하되 출력 경로 보호·원자 저장·빈 토큰을 all로 해석하는 호환 동작을 유지한다.

### 작업 수명과 정확성

- `exports::clip::Job`은 등록된 source와 `Arc<ManagedDataset>`, canonical i64 DBU
  bbox, 명시 all/none/실제 layer pair, trusted `ClipOptions`만 받는다. browser 경로나
  출력 파일명·native argv를 받는 API가 아니다. 요청 jobs는1..16이고 옵션과 같아야 한다.
  알 수 없는 layer·등록/source 불일치·jobdeck은 native 작업 전에 거부한다.
- 화면 render와 별도 dedicated worker를 열며 style·viewport·cut·thin·LOD·occupancy와
  무관한 기존 exact/full-depth clip을 호출한다. 정수 DBU를 다시 반올림하지 않고,
  명시 `Layers::None`을 전체 레이어로 바꾸지 않는다. stale source의 캐시 export는
  outcome에 `source_stale`을 남기고 암묵적으로 재인덱싱하지 않는다.
- CPU jobs·worker1·decoded budget을 공유 `Resources`에 예약한다. pinned dataset의
  read lease와 예약은 native 종료/reap까지 잡는다. 새 export를 위해 기존 화면이나
  다른 작업을 취소하지 않고, 부족하면 Busy다. 이 회계는 전체 RSS/OS thread 상한이나
  viewer latency 보장이 아니며, native ClipGeometry/인코딩 버퍼는 §7의 기존 한계다.
- preparing→opening→clipping→finishing→ready/failed/cancelled를 보고한다.
  terminal은 worker reap과 자원 반환 뒤다. cancel과 결과 commit을 같은 mutex로
  직렬화하므로 cancel이 이기면 artifact가 없고, commit이 이기면 늦은 cancel은 false다.
  이미 관찰한 native/IO 오류를 나중의 cancel로 바꾸지 않는다. Job Drop도 cancel→join한다.
- 등록 scope/source를 작업 전후 확인하고, source/meta/OVM/OVP/선택적 OVT의
  dev/ino/size/mtime/ctime을 전후 대조하며 마지막에 marker/version을 재검증한다.
  같은 서비스의 index는 read lease로 배제한다. **외부 프로세스가 열린 view의 cache를
  교체하는 문제 전체를 해결하거나 immutable revision을 제공하는 것은 아니다.**
  기존 보류 사항인 cache 세대 관리와 외부 writer CAS/lock은 별도다.

### 파일 보관·만료·다운로드 준비

- worker client가 header/END·unit/name·크기를 확인하고 unlink한 regular-file
  descriptor를 그대로 소유한다. 두 번째 전체 `Vec`, named artifact 디렉터리나
  최종 파일 복사를 만들지 않는다. 아직 웹 다운로드 라우트는 없다.
- 기본 보관 한도는4건·건당512MiB·합계2GiB, 동시 reader2, ready 후 TTL600초다.
  작업 시작 전에 건당 최대량을 예약하고 성공하면 실제 파일 크기로 줄인다.
  실패하면 반환하며, 용량 확보를 위해 다른 결과를 자동 축출하지 않는다.
  **건당 한도는 native 생성 후 검사한다. 생성 중 임시 디스크나 ClipGeometry의
  메모리를 이 한도로 제한하는 것은 아니다.** 초과 결과는 Incomplete 오류이며 게시하지 않는다.
- 별도 만료 thread가 browser 트래픽 없이도 만료를 검사한다(최대1초 간격).
  만료/release/owner close 뒤에는 기존 reader도 더 읽을 수 없다. reader가 붙은
  retired descriptor는 reader Drop까지 계속 보관량·건수에 산입한다. pending 작업의
  예약도 close만으로 미리 반환하지 않고 native 정리 뒤 Reservation Drop에서 반환한다.
- `Download::read_chunk`는 최대1MiB, 독립 `read_at` offset으로 읽고 전후에
  만료/철회·파일 크기를 검사한다. 같은 artifact의 두 download가 seek offset을 공유하지
  않는다. HTTP 연결 시 reactor 밖에서 읽고 transport credit을 기다린 뒤 다음 chunk를
  읽어야 하며, 연결 종료/timeout에 reader를 Drop해야 한다.
- `Store::close`는 결과와 reader를 철회하고 pending 작업에 취소를 보낸다.
  owner 종료 처리에서는 별도로 Job들을 join해야 native reap이 완료된다.
  artifact ID는 Resources의 단조 ID를 쓰지만 **이 모듈은 인증/권한/재요청 ledger가 아니다.**

### 검증과 다음 연결

단위 테스트는 pending 예약·실제 bytes 정산, 한도/잘못된 descriptor,
독립 offset·reader cap, 트래픽 없는 TTL, pin 상태 회계, owner close,
cancel/commit 경합과 실제 오류 보존을 검사한다.

필수 `validate_managed_clip.py`는 private concave/path/회전·반사·배열 fixture를
Python/j1/j8 바이트 및 KLayout Region XOR로 대조한 뒤, managed 결과도17-byte
chunk로 같은 바이트인지 검사한다. explicit none, stale flag, unknown layer/등록
불일치/deck 거부, admission/읽기 lease, ready/open/clip/quit의 cancel/Drop/close,
timeout·ENOSPC·손상 출력·작업 중 source/cache 변경·oversize 결과 폐기를 고정한다.
기존 valmini 포함 `validate_app_clip.py`도 공통화 회귀 게이트로 유지한다.

실행 결과(2026-09-14): 전체 `sh tools/validate_rust.sh`가 `RUST VALIDATION: ALL OK`다.
새 managed clip과 기존 clip·캡처·query/DRC/웹 UI, jobdeck80·renderer46,
KLayout13 PX +2 phase-exact +14 style jobs1/8 검증을 포함한다. 만료 단위 테스트의
30ms 스케줄링 의존도 제거한 뒤 app11/core128/web36·transport8·worker-client
unit7/lifecycle14를 다시 통과했다. Rust1.89.0 테스트와 최종 export9개 재검증,
scoped fmt/strict clippy, macOS release·Linux x86-64 musl static-pie 교차 빌드도 통과했다.
워크스페이스 전체 `cargo fmt --all -- --check`는 기존 미변경 CLI/VFS 등의 포맷
차이로 실패하며 변경 패키지의 통과와 구분한다. 해당 파일은 포맷 일괄 수정하지 않았다.
기존 dependency/Pillow/GDK warning은 남고, native geometry/wire/renderd 버전0.12.87과
vendor는 변경하지 않았다. 실제 Linux 실행·현장 Firefox/ETX 수용은 별도다.

다음 M4c 연결은 owner HTTP 작업 ledger·표시/revision receipt·취소·artifact download와
clip UI다. read-only 공유를 export 허가로 해석하지 않으며 guest endpoint는 추가하지
않았다. 전체 M4·GTK 기본 launcher 교체·현장 Firefox/ETX 수용은 여전히 미완료다.

## 13. M4c-2: owner clip 승인·취소·파일 다운로드 API

§12의 관리형 코어를 기존 인증된 **owner** 서비스에 연결했다. 별도 guest/export
권한을 만들지 않았고 임의 경로·명령·업로드를 받지 않는다. 일반 layout만 지원한다.
이 단계 시점에는 clip 브라우저 버튼/선택 도구가 없었다. `capabilities.exports`는 owner
서비스의 API 존재를 뜻하며, 이후 §14에서 view의 `capabilities.clip`과 UI를 연결했다.

### 준비와 명시 승인

1. 표시 ACK와 실제 write receipt를 가진 WS에서 `view.clip.prepare`를 보낸다.
   공통 `seq/view_id/connection_epoch`에 `body:{anchor,bounds,layers,jobs,cell_name}`을
   더한다. anchor는 query와 같은 dataset/worker/frame/state/render revision 식별자다.
   `bounds`는 `{kind:"viewport"}` 또는 `{kind:"dbu",bbox:[i64문자열4개]}`다.
   좌표는 canonical 정수만 받고 2^53 이상도 f64로 왕복하지 않는다. viewport는
   서버의 현재 DBU 범위를 ties-even으로 변환하며 0면적/overflow는 거부한다.
2. `layers`는 `visible|all|none`. visible은 현재 서버 선택을 복제하므로 빈 선택이
   전체 레이어로 바뀌지 않는다. jobs는1~16, 이름은 native 검증을 그대로 적용한다.
   응답 `clip.prepared`는 범위·선택 모드/개수·이름·jobs·revision과 30초 준비 토큰이다.
   준비 자체로 worker/read lease를 늘리지 않는다. 새 준비가 이전 것을 교체하고,
   원래 연결 종료 시 미승인 토큰을 폐기한다.
3. `POST /api/v1/exports`의 `{seq,view_id,token,approve:true}`만 작업을 승인한다.
   승인 시 view가 열려 있고 receipt가 여전히 현재 상태인지 다시 검사한 뒤 dataset을
   pin한다. 이후 pan/zoom·view close와 독립적으로 exact/full-depth 작업을 수행한다.
   display cut/depth/summary는 clip 내용에 영향을 주지 않으며 deck은 명시 거부한다.

준비 토큰은 연결에 묶이지만 승인된 작업은 owner 수명이다. 별도 `/exports` ledger는
1부터 시작하는 canonical u64 문자열 seq, 이력32개, active1개다. 동일 seq/요청은
완료·파일 폐기·view close 뒤에도 재실행하지 않는다. 옵션 변경409, 이력 만료410,
작업 중 새 seq429이며 실패한 사전 검증은 seq를 소비하지 않는다. terminal 게시와
이전 취소 핸들 해제를 같은 잠금에서 끝낸 뒤 다음 작업을 허용한다.

### 현재 라우트와 수명

| 라우트 | 동작 |
|---|---|
| `GET /api/v1/exports` | ledger·한계·사용량·별도 ready `artifacts` 목록(§14) |
| `POST /api/v1/exports` | 위 준비 토큰의 명시 승인, 202 |
| `GET /api/v1/exports/{seq}` | queued/preparing/opening/clipping/finishing/cancelling/ready/failed/cancelled |
| `POST /api/v1/exports/{seq}/cancel` | 취소 요청, 현재 상태와202 |
| `GET /api/v1/artifacts/{id}` | 크기·남은 TTL·고정 파일명, 만료/미존재410 |
| `DELETE /api/v1/artifacts/{id}` | 즉시 접근 폐기, 멱등204; 실제 descriptor 회수는 비동기 |
| `GET /api/v1/artifacts/{id}/download` | cookie+header CSRF로 파일 스트림 |
| `POST /api/v1/artifacts/{id}/download` | native browser form 다운로드용 고정 body CSRF |

모든 API는 기존 Host/Origin/session 검사를 통과해야 한다. GET도 cookie만으로 읽지
못한다. download POST만 `Content-Type: application/x-www-form-urlencoded`와
정확히 `csrf=<소문자 hex64>` 한 필드를 받는다. 중복·인코딩 변형·다른 필드는 거부한다.
비밀을 URL에 넣지 않고 JS의 전체 Blob 버퍼 없이 브라우저에 attachment를 전달하기
위한 계약이다. CSP는 `form-action 'self'`로 한정한다. Range/resume는416 미지원이다.

결과명은 서버 고정 `floe-clip-{id}.oas`, 경로·native PID/진단은 응답하지 않는다.
operation은 파일이 만료돼도 ready 이력을 유지하고 `artifact.available=false`로
구분한다. 최대4개/개별512MiB/총2GiB/reader2개/ready 후600초는 §12와 같다.
취소202는 완료 확인이 아니다. 코어에서 게시가 먼저 확정되면 ready가 남으며,
cancelled/failed는 native 종료·reap와 자원 해제 뒤에만 보인다. logout/context
종료는 작업을 취소하고 준비/결과 접근을 폐기한다. 이전 accepted 요청을 새 seq로
자동 재전송하면 안 된다. 불명확한 응답은 GET 조회 후 **같은 요청**의 명시 재확인이다.

### 전송·자원 경계

- native 수집은 별도 actor/worker, 파일 읽기는 blocking executor에서 한다.
  64KiB chunk·채널1개·reader2개이며 모든 live chunk는 `Bytes::from_owner`로 기존
  전송 byte credit을 보유한다. Hyper의 slice/clone 뒤에도 credit이 먼저 반환되지 않는다.
- 각 read/queue 대기 전후 session·만료·폐기를 검사한다. 10초 전송 정체를 중단한다.
  폐기 전에 전달한 chunk나 OS 네트워크 버퍼는 회수할 수 없으며 이후 읽기부터 막는다.
  Content-Length와 실제 bytes가 불일치한 전송은 완료된 다운로드로 취급할 수 없다.
- HTTP 전체10초 제한을 idle10초로 바꿔 활동 중인 긴 전송을 허용한다. 대신 **모든**
  요청 body를16KiB/5초 안에서 수집해, handler가 무시하는 GET body도 무한 drain하지
  못한다. 실패 응답은 connection close, header5초/HTTP32개/WS8개 상한은 유지한다.
  WS upgrade 뒤에는 HTTP 타이머를 해제하고 기존 WS heartbeat/ACK/send 기한을 쓴다.
- metadata/open/revoke는 메모리 작업이며, 만료/폐기 descriptor close는 reaper 또는
  마지막 reader의 blocking 스레드가 공유 잠금 밖에서 수행한다. 서버 종료는 native
  actor·reader·reaper 상태를 확인한다.
  kernel의 파일 I/O 자체를 강제로 중단하는 deadline이나 native 출력 생성 중 disk/RSS
  상한은 아니다. §12의 생성 후 크기 검사 한계와 외부 cache hot-reload 보류는 그대로다.

### 검증

`validate_owner_service.py`에 기존3개와 독립 export2개 통합 검증을 연결했다.
private valmini에서 all/visible/none의 HTTP GET과 visible의 form POST bytes를 기존
Python/Rust j1/j8 clip + KLayout XOR 오라클과 비교한다. receipt 위조·오래된 view·
토큰 재사용/연결 종료·중복 승인·인증/Origin/CSRF·Range·파일 폐기/replay를 검증한다.
주입 워커로 summary 표시와 exact export 독립, clip 대기 중 HTTP/WS 응답성,
view close 뒤 읽기 lease 유지, cancel/logout의 PID reap·자원 회수·진단 비노출도 확인한다.
core에는 정수/viewport·선택 스냅샷·stale/deck 거부·비동기 폐기를, transport에는
긴 Hyper body·11초 idle 뒤 WS·무시되는 request body의 크기/시간 상한을 고정했다.

실행 결과(2026-09-14): 필수 `sh tools/validate_rust.sh`가 `RUST VALIDATION: ALL OK`다.
새 owner export2개와 기존 owner/DRC3개, managed clip·CLI 기하/바이트 오라클,
jobdeck80·renderer46·KLayout13 PX +2 phase-exact +14 style jobs1/8을 포함한다.
최종 app11/core130/web41·transport10·worker-client unit7/lifecycle14,
scoped fmt/strict clippy, Rust1.89.0의 같은 테스트, macOS release와 Linux x86-64
musl static-pie 교차 빌드도 통과했다. 기존 dependency/Pillow/GDK warning은 남는다.
native geometry/wire/renderd 버전0.12.87·vendor·GTK 기본 launcher는 변경하지 않았다.
실제 Linux 실행이나 현장 Firefox/ETX, 브라우저 다운로드 수용을 대신하는 결과는 아니다.

이 계약을 사용하는 clip UI는 다음 §14에 기록했다. 다운로드 UI 실브라우저 수용,
나머지 export/주석 저장/전체 M4·GTK 은퇴·현장 Firefox/ETX 완료와 구분한다.

## 14. M4c-3: 현재 viewport clip UI

일반 layout의 **현재 화면 범위**를 exact OASIS로 내보내는 owner UI다. GTK의
현재 viewport clip 범위와 같으며 별도 사각형 선택 도구는 이번 범위가 아니다.
`capabilities.exports`는 인증된 owner API의 존재, view의 `capabilities.clip`은
layout 지원 여부다. 둘을 함께 검사하므로 서비스 없는 preattached viewer에 export
권한을 주거나 jobdeck을 암묵적으로 layout으로 처리하지 않는다.

### 준비·승인·수명

- `Clip current viewport…` → 레이어 visible/all/none·jobs1~16·셀 이름 입력 →
  `Review clip` → 서버가 동결한 DBU 정수 범위/레이어 모드·개수/옵션을 확인 →
  `Approve export` 순서다. Enter는 준비만 하고, 별도 승인 버튼이 있어야 worker를 쓴다.
  None은 빈 OASIS라는 경고를 표시한다. 이름은 일반 textContent로 출력한다.
- 현재 표시 ACK·연결·view/render revision·DPR/viewport 정합성을 요구한다. 옵션
  편집, pan/zoom/visibility/프레임 변경, 연결 종료, 30초 만료는 준비 결과를 버린다.
  화면의 summary/query 불가 여부는 좌표 준비를 막지 않으며 export는 항상 별도
  full-depth exact geometry다. 브라우저가 Canvas 좌표에서 world 정수를 재계산하지 않는다.
- 승인 직전에 ledger를 다시 조회하고 새 seq를 계산한다. 중복 클릭을 막고, 응답이
  불명확하면 원래 승인 요청을 보관한다. `Resolve request`는 **동일 seq/token/body**만
  명시 재전송한다. GET에 그 seq가 보이는 것만으로 다른 탭의 요청과 동일하다고
  가정하지 않는다. pagehide/bfcache 복원·재접속은 조회만 하고 새 export를 시작하지 않는다.
- 승인 이후 pan/view close와 독립적으로 상태를 조회/취소한다. Cancel 응답에서
  ready가 이긴 경우에도 GET과 같은 available/TTL 스키마를 준다. 창을 닫는 것은
  취소가 아니며 owner session 종료는 §13처럼 작업·파일 접근을 폐기한다.

### 파일과 다운로드

`GET /exports`에 별도 `artifacts:[{id,bytes,expires_in_ms,name}]`를 추가했다.
operation history32개가 만료 파일의 수명은 아니므로, ready 파일은 이력에서 밀려도
TTL 안에 계속 발견할 수 있다. 최대4개이며 pending/만료/폐기 파일은 목록에 없다.
usage와 목록은 reaper와 별도 잠금으로 읽으므로 순간 entries 수와 목록 길이의 일치를
클라이언트가 단언하지 않는다. 파일 ID와 이름은 서버 지정이며 임의 path를 받지 않는다.

Download는 같은 출처의 일회성 native form POST다. CSRF는 body에만 있고 URL에는
없다. JS Blob으로 파일 전체를 복제하지 않는다. 오류 페이지가 layout을 대체하지
않도록 별도 다운로드 context(`noopener noreferrer`)를 사용한다. 서버/클라이언트는
클릭을 다운로드 완료로 표시하지 않는다. 실제 완료는 브라우저 다운로드 UI에서 확인한다.
Release는 서버 파일 접근만 폐기하며 이미 받은 사용자의 복사본은 지우지 않는다.
TTL 조회는 버튼 DOM을 교체하지 않아 키보드 focus를 보존한다.

### 검증과 남은 수용

`clip.test.cjs`는 summary 표시·display receipt, i64/u64 극값, 준비 만료/변경/timeout,
명시 승인·중복 클릭, 불명확 응답의 동일 요청 replay, pagehide/resume, ready/cancel 경합,
독립 파일 목록/TTL focus, body CSRF form을 고정한다. `client.test.cjs`의 별도 clip 실행은
실제 app.js의 표시 ACK→WS 준비→HTTP 승인→Download/Release 연결까지 실행한다.
Node 테스트는 브라우저 Window 타이머의 잘못된 receiver도 거부한다.

로컬 Chrome 합성 valmini 점검에서 unbound Window timer의 `Illegal invocation`을
발견했다. clip뿐 아니라 같은 주입 방식을 쓰던 inspect/measure 타이머도 Window에
bind했다. 수정 후 초기 연결·layout 표시·clip 활성화와 margin crop 상태를 확인했다.
브라우저에서 승인→실제 저장 파일까지의 수용과 현장 Firefox/ETX는 **미확인**이다.
네이티브 HTTP 게이트는 viewport 준비의 ties-even 범위, ready cancel 응답 스키마,
목록의 게시/폐기를 검증하며, 기존 all/visible/none OASIS 바이트/XOR 게이트도 유지한다.

실행 결과(2026-09-14): 필수 `sh tools/validate_rust.sh`가 `RUST VALIDATION: ALL OK`다.
owner export2 + 기존 owner/DRC3, managed/CLI clip 오라클, jobdeck80·renderer46,
KLayout13 PX +2 phase-exact +14 style jobs1/8을 포함한다. 마지막 UI 수명주기 보강 후
ES2017/전체 JS 게이트도 재실행해 통과했다. app11/core130/web41·transport10·
worker-client unit7/lifecycle14, strict scoped clippy/fmt, Rust1.89.0 테스트와
macOS release/Linux x86-64 musl static-pie 빌드도 통과했다. Linux 실행은 미확인이다.
renderd0.12.87·geometry/wire·vendor·GTK 기본 launcher는 바꾸지 않았다.

전체 M4 완료, GTK 기본 전환/제거, 공유 권한 추가, 현장 수용을 뜻하지 않는다.
다음 독립 구현은 남은 웹 내보내기와 review/주석 저장 등 parity 항목이며,
기존 현장 실측 보류는 유지한다.

## 15. M4d-1: 표시 픽셀 PNG와 overlay 전환

UI-05 중 GTK `_copy_view`의 **보이는 canvas 복사**, `_toggle_overlays`의
3상태를 웹에 연결한다. 새 native render/export나 source 파일 접근 없이 이미 표시한
픽셀만 사용한다. exact OASIS clip(§14), batch/DRC 재렌더 캡처(§8/10), 주석 metadata
편집(§9)과 별개다. 일반 layout과 jobdeck/summary/preview 모두 대상이며, exact scene
query가 가능해야 한다는 조건은 없다. GTK launcher·geometry/cut·vendor는 바꾸지 않는다.

### 조작과 화면 계약

- Display의 **Copy view** 또는 canvas focus의 Ctrl/Cmd+C가 이미지 복사를 요청한다.
  선택된 텍스트, 자식/input target, Alt/Shift/IME 조합은 가로채지 않는다.
- **Save view PNG**는 같은 합성 결과를 브라우저 다운로드로 요청한다.
  clipboard 기능이 없으면 Copy만 비활성화한다. 권한 거부 시 자동 파일 쓰기로
  바꾸지 않고, **Download captured PNG**로 방금 고정한 같은 이미지를 받을 수 있다.
- 화면 크기의 opaque black canvas에 현재 margin → foreground → 도형 선택/snap →
  DRC → ruler 순서로 합성한다. 실제 표시 중인 정수 device offset을 그대로 쓰며
  보간/재스케일하지 않는다. pan 중 새 strip은 margin, 겹친 부분은 이전 labeled
  foreground인 상태도 그대로다. native design labels는 이미 원본 pixels에 있다.
- annotation의 예약된 rAF만 동기적으로 마무리하고 클릭 시점의 합성을 고정한다.
  이후 pan/frame 교체가 진행돼도 그 PNG는 바뀌지 않는다. UI sidebar/status/오류 배너/
  viewport hint는 제외한다. 출력 크기는 CSS 크기가 아니라 viewport **device pixels**다.
  이것은 원본 도형 전체·최신 final만의 캡처 또는 flateyes metadata 파일이 아니다.
- Overlays는 `All → Hide other errors → Hide all`로 순환한다. canvas의 Tab으로도
  바꾸며 Shift+Tab은 정상적인 focus 이탈에 남긴다. 중간 단계는 선택/snap/룰러와
  focused DRC를 유지하고 page/group 오류 marker만 숨긴다. 숨긴 DRC는 hit/tooltip
  대상에서도 제거한다. 마지막 단계는 모두 숨기되 완료된 선택·룰러·group은 지우지
  않는다. 진행 중 DRC box 선택만 취소한다. native labels/frames는 별도 토글 그대로다.
  모드는 현재 페이지 수명 안에서만 보관하며 사용자/설계 설정 저장은 후속 범위다.

### 브라우저·권한·비용 경계

인증된 owner capability `snapshot_png`만 추가했다. 새 HTTP/WS route, guest 권한,
upload, clipboard 읽기는 없다. PNG와 blob URL은 로컬 browser 메모리에서만 만든다.
파일명은 `floe-view-WxH.png`이며 경로/원본 이름/토큰을 넣지 않는다.

Async Clipboard는 secure context 및 실제 `ClipboardItem`/`clipboard.write` 지원을
검사한다. 지원돼도 브라우저 권한에 따라 실패할 수 있다. PNG Promise를 항목에 넣어
**원래 클릭/keydown 안에서** write를 시작하고, 인코딩 완료 뒤 새 권한 동작을
가정하지 않는다. 브라우저 하한은 이를 지원한다고 가정해 올리지 않으며 PNG 저장이
대안이다([Clipboard API 명세](https://www.w3.org/TR/clipboard-apis/)).
PNG 인코딩은 표준 canvas `toBlob`을 사용한다
([HTML canvas 명세](https://html.spec.whatwg.org/multipage/canvas.html#dom-canvas-toblob-dev)).

- 출력·입력 canvas는 기존 pixel 한계(축8192, 면적16Mi pixels)로 검증한다.
  입력 canvas 최대5개를 출력 canvas1개에 합성하며 encoder1개,
  retained PNG1개(80MiB 이하)로 제한한다.
  이는 browser 내부 인코딩 임시 메모리까지의 하드 한계를 의미하지 않는다.
- native `toBlob`은 취소 API가 없으므로 pagehide/stop이 전달 promise를 거부해도
  encoder credit은 callback까지 유지한다. 빠른 권한 거부나 bfcache resume로
  encoder가 중첩되지 않는다. 15초가 지나면 대기 안내만 표시하며 성공/취소로 가장하지
  않는다. OS clipboard 요청 자체를 취소하거나 이미 전달된 복사본을 되돌릴 수는 없다.
- 이전 retained PNG/URL은 다음 캡처와 stop에서 폐기하며, 임시 합성 canvas는
  callback/인코딩 오류 때 1×1로 줄인다. 실패 후 재다운로드는 새 렌더/인코딩이 아니다.
  download 요청을 저장 완료로 표시하지 않는다. 실제 파일 완료는 브라우저가 관리한다.
- GTK의 selection-owner clipboard와 달리 세션 종료 시 시스템 clipboard를 비우지
  않는다. 이를 UI에 표시한다. 사용자 clipboard를 읽거나 덮어써 복구하려 하지 않는다.

### 검증과 미완료

`snapshot.test.cjs`의 독립 pixel source-over 모델은 crop/alpha/stack/검정 여백,
후속 frame과 독립적인 캡처, 동기 clipboard 요청, 기능 부재/거부/인코딩 실패,
크기/개수 한계, uncancellable encoder와 stop/resume, URL/canvas 회수를 고정한다.
실제 app.js 통합 실행은 margin+옛 foreground 위치, Ctrl/Cmd+C/선택 텍스트/IME와
Tab/Shift+Tab, 복사/저장/fallback, HTTP·native 명령 없음과 종료 정리를 확인한다.
inspect/measure/DRC 테스트는 숨김 중 데이터 보존과 숨긴 marker의 hit 제거를 검사한다.
Node canvas/clipboard mock은 실제 OS clipboard·PNG codec·다운로드 수용의 대체가 아니다.

로컬 Chrome에서는 합성 valmini 초기 연결, margin crop 상태와 Copy/Save 버튼
활성화를 accessibility tree로 확인했다. 실제 버튼 저장/OS clipboard 조작과 화면
픽셀 screenshot 대조는 하지 않았다. owner capability false/true와 새 embedded asset은
transport/owner native 게이트로 확인했다.

Rust1.89.0 테스트, scoped strict clippy/fmt와 macOS release/Linux x86-64 musl
static-pie 빌드는 통과했다. Linux 실행은 미확인이다. 초기 scoped 테스트는
worker-client `query_errors`에서 `worker command queue full`로 1회 실패했다.
8개 query 직후 render를 unwrap하는 기존 테스트의 writer scheduling 가정이 있으며,
관련 제품/테스트 코드는 이번에 바꾸지 않았다. 동일 버전의 단독 lifecycle14개와
scoped 전체 재실행, 전체 배터리의 해당 테스트 및 Rust1.89 실행은 통과했다.
최초 실패를 지우거나 이 원인을 이번 UI 수정으로 해결했다고 간주하지 않는다.

최종 실행(2026-09-14): `sh tools/validate_rust.sh`는 `RUST VALIDATION: ALL OK`다.
owner export2 + owner/DRC3, managed clip, jobdeck80·renderer46와
KLayout13 PX +2 phase-exact +14 style jobs1/8을 포함한다. ES2017/전체 JS,
app11/core130/web41·transport10·worker-client7/lifecycle14도 최종 통과했다.
renderd 버전은0.12.87로 유지했다(이번 단계는 worker 구현 변경 없음).

현장 Firefox/ETX와 실제 OS clipboard·다운로드 수용은 보류/미확인이다.
전체 M4 완료나 GTK 기본 전환은 아니다. 남은 review/주석·설정 저장·기타 export와
공유 승인/패키징·현장 게이트를 계속 별도로 추적한다.

## 16. M4d-2: Rust layerprops 포맷과 초기 가시성

UI-03의 설정 불러오기/저장 이관을 위한 공통 Rust 포맷과, 웹 첫 화면의 실제 누락을
해결한다. 이전 `styles::load_props`는 색·fill·width만 보존하고 다섯 번째 visibility
열을 버렸으며 `ViewState::initial`은 무조건 All이었다. GTK는 `_apply_props_visibility`를
첫 렌더 전에 적용하므로 숨김 설정이 있는 같은 설계의 첫 화면이 달랐다.

### 현재 연결된 동작

- source 옆의 `<file>.layerprops`, 없으면 `<stem>.layerprops`라는 기존 우선순위를
  유지한다. 덱은 기존 mode별 `props_source` namespace를 사용한다. `cache.py`의 실제
  정책은 **개인 palette cache 없음**이며, 오래된 GUI 소개 주석을 근거로 새 개인
  파일 조회/자동 저장을 도입하지 않는다.
- 공통 `layerprops::parse`는 여섯 열, 생략된 datatype=0/name=""/visibility=1/width=1,
  주석/빈 행 생략, malformed 행 수, 중복 행 순서와 미정의 color/fill/flag 토큰을
  보존한다. extra column/dot suffix 무시는 기존 Python 형식과 같다. u32 밖의 pair는
  native layer가 될 수 없으므로 malformed로 센다. unknown color/fill을 다른 값으로
  바꿔 저장하지 않는다. 파일 포맷 해석과 현재 스타일에 적용하는 정책을 분리했다.
- startup visibility는 실제 `0`/`1`만 적용한다. 모르는 pair·다른 flag는 무시하며
  unlisted layer는 기본 표시를 유지한다. 중복은 마지막 유효 flag가 이긴다. 덱 헤드는
  **명시적인 유효 flag가 없는 자식**에만 영향을 준다. 자식 행이 헤드보다 먼저 있어도
  같은 결과다. 부분 선택에는 헤드 key를 포함하지 않는다. 중복 헤드를 먼저 합쳐
  큰 자식 목록을 중복 행마다 순회하지 않으며, 모두 표시이면 기존 All 경로를 유지한다.
- `Model`의 초기 가시성은 native worker 첫 요청 전 `ViewState`에 들어간다.
  `--layers`/startup body의 명시 All/None/Only가 있으면 그 편집이 우선한다.
  headless render/capture의 기본 All은 바꾸지 않는다. 열린 view가 변경된 sidecar를
  자동 감지하는 기능은 추가하지 않았다. 기존 source/cache revision 결정 보류도 유지한다.
- `layerprops::format`과 `Row::from_style`은 순서·이름·색 palette의 첫 일치 이름과
  표준 bitmap 이름을 보존해 six-column text를 만든다. 빈 이름은 L_D, ASCII space는
  underscore로 바꾸는 기존 포맷이다. 현재는 **라이브러리 API**이며 브라우저 Load/Save
  버튼이나 새 filesystem/HTTP endpoint를 광고하지 않는다.

### 데이터 손실·자원 경계

문서4MiB, 최대65,536행·필드4,096bytes다. 크기 초과/필드 내 control character는
문서 전체의 명시 오류이며 prefix만 적용/저장하지 않는다. 출력 token에 whitespace를
넣어 새 행/열을 만드는 것도 거부한다. 선폭 정수는 큰 수 allocation/overflow 없이
native 범위1..8로 clamp하며 invalid token은 별도로 유지한다. 기존 explicit selection
한계4,096은 그대로다. 더 큰 부분 선택이면 명시 오류이며 임의의 일부만 표시하지 않는다.
All/None으로 정규화되는 대형 표에는 이 부분 선택 한계를 잘못 적용하지 않는다.

six-column Calibre 파일에는 임의16×16 bitmap 본문을 저장할 수 없다. 알려진 이름과
맞지 않는 bitmap은 `Unsupported`이며 speckle/solid로 대체하지 않는다. GTK의 세션 내
bitmap 편집 후 슬롯 이름만 저장하는 경로도 재오픈 시 편집 내용이 보존되지 않으므로
그 손실을 정답으로 복제하지 않는다. 완전한 설정 snapshot/브라우저 저장 연결에서는
이 경우를 별도 lossless 형식/명시 안내로 처리해야 한다.

### 검증과 다음 연결

`validate_layerprops.py`는 실제 `floe.fillpat` parse/format/color 이름과 GTK의
`_apply_props_visibility`/group sync를 추출해 오라클로 쓴다. GTK import/창은 필요 없다.
72문서·980표준 스타일·4개 private valmini 모델의 초기 선택/명시 All, 입력/cache의
byte·mtime 불변을 검사한다. Rust 실행 PATH는 비워 Python fallback을 금지한다.
단위 테스트는 malformed/unknown/큰 정수·control/한계, 부모-자식 역순/중복, 대형
All/None과 부분 선택 거부를 추가로 고정한다.

기존 `validate_app_deck_render.py`의6 native managed controller에 GTK 기본 가시성의
첫 PNG와 명시 All 복원 PNG를 모두 대조한다. 부모0/자식1과 헤드 후순위 파일을 포함한다.
기존 metadata/styles/archival PNG gate를 삭제하거나 조건부로 바꾸지 않았다.
새 layerprops 게이트도 `validate_rust.sh`에 연결했다.

최종 검증(2026-09-14): 전체 `sh tools/validate_rust.sh`가 종료 코드0과
`RUST VALIDATION: ALL OK`로 완료됐다. jobdeck80·renderer46, KLayout13 PX
+2 phase-exact +14 style jobs1/8을 포함한다. 마지막 중복 헤드 정규화 뒤에는
codec72문서/980스타일/4모델과 덱6 controller의 초기·All 총12 PNG를 다시 대조했고,
app11/core134/web41·transport10·worker-client7/lifecycle14 및 scoped strict
clippy/fmt를 재확인했다. Rust1.89 테스트와 macOS release/Linux x86-64 musl
static-pie 빌드도 통과했다. 기존 vfs dead-code 경고는 남아 있다.
renderd는0.12.87로 유지한다(worker 변경 없음). 이번 단계에는 새 브라우저 UI가 없고,
실제 Linux 실행·현장 Firefox/ETX 검증을 수행한 것으로 간주하지 않는다.

다음은 열린 세션의 원자적 속성 적용/브라우저 Load·Save와 명시적인 기본값 게시다.
live import에서 invalid 색/fill/width를 각기 무시하고 unlisted 상태·기존 명시적
child fill/width를 보존해야 한다. 이를 행마다 완전한 Style로 덮어쓰는 변환으로
대체하면 GTK의 sparse assignment 상속과 달라진다. 저장은 source 경로 임의 쓰기나
자동 기본값 게시가 아니어야 한다. GTK 기본값 게시 메뉴는 `FLOE_FILL_EDIT` 개발용임도
유지한다. 이 단계는 Load/Save UI나 전체 UI-03/M4 완료가 아니다.

## 17. M4d-3: 열린 세션의 설정 Load/Save

UI-03의 Load/Save를 Rust의 세션 상태와 기존 owner HTTP/WS에 연결했다. 레이어 패널의
**Load settings**로 사용자가 직접 파일을 고르고, **Save settings**는 선택한 형식의
브라우저 다운로드를 요청한다. source 옆의 `.layerprops`나 색인 파일을 쓰지 않으며,
자동 저장·설계 기본값 게시·임의 서버 경로 입출력은 추가하지 않는다.

### 두 형식과 적용 의미

- **Calibre layerprops**: §16의 six-column 텍스트를 현재 상태에 부분 적용한다.
  모르는 pair와 invalid color/fill/visibility/width는 해당 항목만 무시한다. unlisted
  레이어와 관련 없는 view 상태는 유지한다. 중복은 마지막 유효 항목, 명시 자식은
  같은 파일의 헤드보다 우선한다. visibility는 현재 선택을 기반으로 적용한다.
- 색은 **이번 파일에 명시된** 헤드 recolor가 자식에 전파된다. 반면 fill/width는 GTK처럼
  세션의 **희소 assignment map**을 보존하므로 이전에 명시한 자식 값이 새 헤드 값보다
  우선한다. invalid 항목을 기본값으로 채운 완전한 Style로 변환하지 않는다. width≤1은
  override 제거이고, 자식이면 헤드 선폭을 상속한다. 초기 sidecar의 width>1만 반영하는
  정책과 live import의 제거 정책은 실제 GTK와 마찬가지로 별개다.
- GTK의 width-only 파일은 유효 color/fill이 없으면 repattern 전에 return하는 결함이
  있다. Rust는 width-only도 즉시 반영한다. 이 차이는 의도된 수정이며 단위 테스트로
  고정한다. GTK 연속 로드 오라클은 각 파일에 유효 fill 하나를 두어 이 결함을 분리한다.
- Calibre Save는 현재 **유효 색/fill/width/가시성**과 metadata 순서·전체 이름을
  내보내므로 상속 관계를 평탄화한다. 알려진 이름이 없는 custom bitmap은 명시 오류다.
  손실을 숨기거나 다른 패턴 이름으로 대체하지 않고 Native JSON 사용을 안내한다.
- **Native JSON (기본 저장 형식)**: `format:"floe.layers", version:1`과 전체 레이어
  row·group 표를 저장한다. 색은 유효 값, fill/width는 희소 assignment다. 두 필드는
  필수이며 `null`은 상속/기본값, bitmap은 `solid`/`clear`/`speckle` 또는16행 u16을 저장한다.
  같은 pair/group 표의 view에만 전체 복원한다. 누락/중복 pair, 다른 group, 미지원 버전,
  unknown field·invalid 값은 문서 전체 오류다. custom bitmap과 이후 헤드 변경 의미를
  모두 보존한다. viewport·mono·DRC·주석·격리 전 visibility 백업은 저장 대상이 아니다.

설정 파일뿐 아니라 웹 스타일 편집도 `style_deltas`로 **변경한 필드만** 보낸다.
색상 변경이 fill/width를 새 자식 override로 고정하지 않는다. fill/width dialog도
변하지 않은 값은 보내지 않고 no-op 제출은 요청 자체가 없다. GUI의 헤드 직접 편집은
해당 필드를 자식 전체에 적용하고, 같은 요청의 명시 자식 필드는 개별적으로 우선한다.
이 동작은 위의 Calibre partial import와 구별한다. 기존 완전한 `styles` API의 의미는
유지하고 두 수정 형식의 혼용·null·unknown/중복 pair는 거부한다.

### 트랜잭션·자원·수명

`POST /api/v1/views/{id}/settings/{state_rev}/{native|calibre}`는 읽기 전용 준비다.
파일 이름/경로 대신 선택한 UTF-8 본문만 보내며 cookie·CSRF·Origin 검사를 통과해야 한다.
canonical settings POST만 인증 후 최대4MiB를 버퍼링한다. 일반 요청16KiB,
WebSocket control8KiB는 그대로다. semaphore1이 import body/파싱과 export 작업을
유계화하고 문서 파싱·모델 적용 준비는 blocking pool에서 수행한다. HTTP 수명은 기존
5초 한계를 유지하며 timeout 뒤 남는 blocking 작업도 완료 전까지 permit을 보유한다.
완성된 HTTP 응답 전송까지 semaphore를 보유하는 것은 아니며, 그 잔류량은 기존
HTTP 동시 연결32개와 응답별4MiB·idle deadline으로 제한한다.

준비 결과는 해당 view의 기존 single-use slot에 Arc-backed 레이어 상태로 보관한다.
`view.apply`는 token과 base revision만 받아 기존 CAS로 원자 반영한다. 새 준비는 이전
token을 대체하고, 알려진 token은 충돌 시에도 한 번만 소비된다. view 교체/로그아웃/
revision 변경 뒤 준비/다운로드 결과는 버린다. token의 서버 바인딩은 **view+base revision**이며,
브라우저는 추가로 connection epoch가 바뀌면 진행 중 작업을 중단하고 자동 재전송하지 않는다.
이미 적용을 전송한 뒤 Cancel이면 committed 가능성을 명시하고 현재 snapshot을 권위로 삼는다.

`GET`는 같은 id/revision의 설정 텍스트만 반환한다. Native JSON과 Calibre 모두4MiB/
65,536행 한계며 상한 초과는 prefix 성공이 아니다. 기존4,096 partial selection 한계도
유지한다. 입력은 FileReader 비동기 읽기 후 fatal UTF-8 검사, 다운로드는 Blob URL과
고정된 안전한 파일명이다. 파일 접근은 브라우저에서 사용자가 선택한 File에 한정한다.
이 API 선택의 근거는 [W3C File API](https://www.w3.org/TR/FileAPI/)이며 현장 Firefox
호환성을 이 문서나 Node 실행만으로 확인한 것으로 간주하지 않는다.

브라우저는 한 작업만 허용하고, 다운로드 URL은 최대4개/60초 후 또는 종료 시 회수한다.
복구·epoch/revision 변경·pagehide/stop에서 reader/XHR/edit callback을 정리한다.
GET 저장도 현재 revision과 맞지 않으면 다운로드하지 않는다. 실제 저장 성공 여부는
브라우저 소관이므로 UI는 “Download requested”와 다운로드 목록 확인을 안내한다.
이번 통합 테스트에서 발견한 **no-op edit 후 pick receipt 소실**도 수정했다. margin을
실제로 재합성한 경우에만 foreground receipt를 폐기하고, pixels가 바뀌지 않았다면
그 receipt를 유지한다. 최신 입력/정책/ACK 검사 자체를 우회하지 않는다.

### 검증과 남은 범위

`validate_layerprops.py`는 기존72문서/980표준 스타일/4모델 외에 실제 GTK
`_load_props_dialog`/희소 map 처리와 RustRenderWorker/DeckRenderWorker의
recolor/repattern을 오라클로 사용한다. 일반4모델+덱6모델에 각16회, 총160회 연속
로드의 가시성·색/fill/width를 Rust 상태와 비교한다. 기존 덱6 controller의 초기/All
12 PNG와 archival PNG 게이트도 유지한다. 입력/cache byte·mtime 불변을 검사한다.
GTK widget·chooser는 mock이며 실제 GTK 창을 조작한 검증은 아니다.

core 단위 테스트는 희소 상속·중복/invalid 항목·width-only·custom bitmap/native 왕복,
불완전 snapshot·충돌 거부와 색상만/헤드·자식 필드별 편집을 고정한다. owner HTTP native
테스트는32KiB 준비가 무변경임을 확인하고 CAS/replay/stale·형식/4MiB 거부·custom bitmap
왕복과 source/cache bytes 불변을 검사한다. runtime PATH를 비워 Python fallback을 막는다.
ES2017 모듈/실제 app.js 실행은 file/UTF8·상한·취소/timeout·재접속·ACK 뒤 효과·native
다운로드와 no-op 뒤 pick 사용을 확인한다. Node FileReader/download mock은 실제 브라우저
chooser·저장 결과의 대체가 아니다.

로컬 Chrome의 합성 valmini에서 첫 frame·margin crop의 착지와 Load/Save 버튼 활성화,
Native JSON/Calibre 선택 및 “설계 기본값을 쓰지 않음” 안내를 accessibility tree로
확인했다. 파일 선택/다운로드 버튼은 실제로 누르지 않았고 pixel screenshot 대조도 없다.
검증용 서버는 SIGINT로 정상 종료했다.

최종 검증(2026-09-14): 마지막 필드별 편집과 owner 테스트를 포함한
`sh tools/validate_rust.sh`가 종료 코드0·`RUST VALIDATION: ALL OK`로 완료됐다.
owner6개, jobdeck80·renderer46, KLayout13 PX+2 phase-exact+14 style, GTK160회 연속
설정 오라클과 기존12 초기/All PNG를 포함한다. app11/core140/web41와 ES2017/전체 JS,
scoped strict clippy/fmt도 통과했다. Rust1.89 테스트와 macOS release/Linux x86-64 musl
static-pie 빌드는 통과했으며 기존 tiler/vfs 등의 dependency 경고는 남아 있다.
worker 구현은 바꾸지 않아 renderd0.12.87은 유지한다. 실제 브라우저 파일 선택/저장과
Linux 실행·현장 Firefox/ETX 수용은 미확인이다.
설계 기본값 게시와 나머지 주석/review 쓰기·내보내기·배포 작업은 후속 단계다.

## 18. M4d-4a — 공유 설계 기본값 게시 코어 (2026-09-14)

UI-03의 남은 설계 기본값 게시를 구현하기 위한 **Rust 코어 단계**다. 일반 설정 Save와
구분하며, 아직 launcher opt-in·HTTP/WS 게시 endpoint·브라우저 승인 버튼을 추가하지 않았다.
`FLOE_FILL_EDIT`가 비어 있지 않을 때만 보이는 GTK 개발 메뉴의 공유 파일 쓰기를
일반 다운로드나 자동 저장으로 확대하지 않는다. 기존 owner 설정 API에는 파일 쓰기 권한이 없다.

### 대상·형식

`layer_defaults::Publisher`는 trusted launcher가 등록한 전체 source 집합(1..32)을 받아
만드는 capability다. `prepare`는 그 집합에 속한 source, mode, 현재 Calibre 설정 text로
읽기 전용 draft를 만들고, **draft를 소비하는 `publish`만** 파일을 쓴다. 브라우저 경로 입력이나
임의 출력 경로는 없으며, 후속 서버에서는 `ViewState::layerprops`의 현재 revision 출력만
사용해야 한다. 이 코어 자체는 view/owner/승인 토큰을 알지 못한다.

- 일반 layout: `<source>.layerprops`.
- 덱 level: `<deck>.jb.layerprops`, chip: `<stem>.chip-by-level.jb.layerprops`,
  source-layer: `<stem>.layer.jb.layerprops`. 확장자·숨김 파일·공백·한글 처리는 기존
  `props_source`/GTK와 같다. 읽기의 `<stem>.layerprops` fallback에 쓰지는 않는다.
- 입력은 기존 six-column codec의4MiB/65,536행 한계, malformed/빈 문서는 전체 거부다.
  게시 코어가 새 설정 형식이나 개인 palette 파일을 만들지 않는다.
- custom bitmap의 Calibre 이름 부재 외에 **폭이 넓은 head 아래 명시 child width1**도
  Calibre export 오류로 알린다. startup/live import에서1은 상속이므로 그대로 저장하면
  다음 로드에 폭이 달라진다. Native JSON은 이 상태를 손실 없이 보존한다. 공유 default의
  형식을 JSON으로 몰래 바꾸거나 근사 선폭을 쓰지 않는다.

현재 source뿐 아니라 **모든 등록 source·덱 dependency·cache tree·index lock**을
출력 금지 대상으로 검사한다. 다른 source가 `.layerprops` 형태의 파일명을 갖거나
missing dependency가 게시 대상과 같은 이름인 경우도 거부한다. source/dependency 범위와
directory identity를 준비 및 게시 직전에 다시 확인한다. source/cache 원본은 쓰지 않는다.

### 원자성·충돌·메타데이터

draft는120초 유효하며 기존 파일의 bytes, dev/inode, size, mtime/ctime ns, mode/uid/gid,
link 수, 읽을 수 있는 확장 속성과 ACL을 캡처한다. 기존 파일은4MiB 이하 regular single-link만
허용하고 symlink/FIFO/directory/hardlink를 거부한다. 교체 시 `O_RDWR`로 truncate 없이
열어 기존 파일의 쓰기 권한도 확인한다. directory 쓰기 권한만으로 read-only 파일을 우회하지 않는다.

게시 시 `<target>.lock`의 nonblocking exclusive `flock`을 얻는다. lock은0-byte regular
single-link 파일이고 inode가 유지되어야 하므로 **게시 후에도 삭제하지 않는다**. 같은 protocol을
쓰는 프로세스끼리 최종 재검사와 commit을 직렬화한다. 이미 있는 빈 lock 파일은 실패나
이전 프로세스 종료의 증거가 아니며, PID stale-lock 삭제 로직을 두지 않는다. 잠금 수명과
NFS/SMB 동작은 [flock 매뉴얼](https://man7.org/linux/man-pages/man2/flock.2.html)을 따른다.

같은 directory FD 안에 임시파일을 만들고 `openat`/`fstatat`의 no-follow로 다룬다.
새 파일의 umask/default ACL 정책을 **내용이 비어 있을 때** 캡처한 뒤 private로 만들고,
완성한 text에 원래 target의 mode/uid/gid/확장 속성/ACL 또는 새 파일의 생성 정책을 적용한다.
복원 후 동일성을 확인하고 파일 sync → 최종 충돌/취소 확인 → 기존 대상은 `renameat`,
없던 대상은 create-if-absent `linkat`으로 게시한다. 기존 열린 descriptor는 이전 inode를
계속 읽을 수 있고 pathname 독자는 완성된 old/new 파일을 본다. 실패 시 소유한 임시 entry만
정리한다. commit 뒤 디렉터리 sync 실패는 `Published {directory_synced:false}` 경고이며,
게시 완료를 오류/취소로 뒤집거나 자동 재시도하지 않는다.

Linux POSIX ACL은 FD xattr로, macOS extended ACL은 Darwin ACL API로 보존한다.
새 파일의 ACL·mode 관계는 [Linux ACL 매뉴얼](https://man7.org/linux/man-pages/man5/acl.5.html),
Darwin descriptor API는 [Apple ACL 문서](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/acl_get_fd.3.html)
및 SDK `sys/acl.h`를 기준으로 했다. 추가 libacl·Python·외부 chmod 실행은 **제품에 없다**.
macOS 단위 테스트의 chmod는 전용 합성 파일에 실제 ACL을 주는 오라클로만 사용한다.
확장 속성 이름 합64KiB/값 합1MiB, 특수 permission bits·실행 capability·macOS BSD flags는
명시 거부하고, 읽기/복원 불가 속성도 기존 파일을 바꾸지 않고 실패한다.
원래 소유자가 다른 UID이면 group 쓰기 권한이 있더라도 새 inode에 그 owner를 복원할
권한이 없어 실패할 수 있다. 같은 서비스 UID의 게시를 우선 대상으로 하며, GTK의
in-place group-write와 여러 OS 계정 간 게시의 완전한 동작 일치는 아직 수용 범위가 아니다.
지속 lock 파일의 접근 권한도 해당 공유 계정/디렉터리 정책에 맞아야 한다. 이를 해결하려고
owner/ACL을 몰래 바꾸거나 lock을 삭제하는 fallback은 두지 않는다.

이는 모든 filesystem metadata의 보존이나 hostile filesystem sandbox를 뜻하지 않는다.
권한 때문에 열거되지 않는 privileged xattr, 파일 시스템 고유 속성은 보장 범위 밖이다.
외부 writer가 lock을 무시하고 최종 검사 뒤 바꾸는 경쟁은 filesystem CAS가 아니다.
concurrent source/symlink/directory 이동도 기존 등록 계약대로 미지원이다. NFS/SMB의
lock·ACL·durability는 현장 검증 전 수용으로 표시하지 않는다. 강제 종료/파일 시스템 장애의
임시 entry 회수도 후속 운영 정책이며, 임의 `.tmp`/lock 청소를 자동으로 실행하지 않는다.
열려 있는 다른 GUI의 default 변경 감지·캐시 교체는 앞서 보류한 파일 관리 정책 범위다.

### 검증·후속

집중 단위 테스트14개는 read-only prepare, 생성/교체, old FD 보존, 실제 ACL/xattr·mode
보존과 생성 시 directory ACL 상속, bytes/속성/삭제/새 생성 충돌, 취소/만료/주입 실패,
nonblocking lock·단회 경쟁 승자, 특수 파일·전체 source/cache 보호, directory/lock/temp
치환 시 무관 파일 보존, commit 후 늦은 취소/dir-sync 실패를 고정한다. Calibre 선폭
손실 거부와 Native JSON 왕복1개를 더해 app-core155개가 로컬에서 통과했다.

`tools/validate_layer_defaults.py`는 실제 GTK `props_src`/`shared_props_paths`/
`save_shared_props`를 오라클로 layout2개와 덱6개×3mode, **20개 경로/바이트**를 비교한다.
모든 쓰기는 독립 임시 fixture이며 Rust runtime PATH를 비운다. source·stem fallback의
bytes/mtime 불변과 허용된 target/0-byte lock 외 잔류 없음도 단언한다. 이 게이트는
`tools/validate_rust.sh`에 필수 연결했다. 실제 웹 게시/사용자 default를 바꾼 검증은 아니다.

Rust1.89의 app-core155개와 Linux x86-64 musl release 테스트 static-pie 링크를 확인했다.
Linux에서 실행한 ACL 검증은 아니며 현재 macOS의 결과로 대신하지 않는다.
최종 `sh tools/validate_rust.sh`는 종료 코드0·`RUST VALIDATION: ALL OK`로 완료됐다.
새20개 게시 오라클과 app-core155·web41, 기존 owner6·jobdeck80·renderer46 및
KLayout13 PX+2 phase-exact+14 style을 포함한다. ES2017/전체 JS, scoped fmt와
app-core/web all-target strict clippy도 통과했고, 기존 dependency/deprecation 경고는 남는다.
worker 구현은 바꾸지 않아 renderd0.12.87을 유지한다.
다음 단계는 별도 launcher opt-in, owner/view/revision에 고정한 준비/명시 승인/결과 receipt,
취소·연결 단절 시 미확정 결과 조회, 유계 작업 admission과 UI다. guest 공유 범위는 추가하지 않는다.

## 19. M4d-4b — owner 기본값 승인·게시 API (2026-09-14)

§18의 파일 게시 코어를 별도 owner capability에 연결했다. **브라우저 버튼은 아직 없다.**
기존 Settings Save는 계속 다운로드이며, 이 단계는 개인 palette 자동 저장이나
guest 공유 권한을 추가하지 않는다. native renderer·인덱스 포맷·cut/LOD 정책도 바꾸지 않는다.

### 실행 권한과 준비

`floe2-web view` 실행 환경의 `FLOE_FILL_EDIT`가 비어 있지 않으면
`capabilities.design_defaults:true`다. 기존 GTK 개발 메뉴와 같이 문자열 `0`도 **켜짐**이다.
변수가 없거나 빈 문자열이면 false이고 모든 defaults endpoint는 인증 후403이다.
trusted Rust embedding은 모든 source/DRC를 등록한 뒤 `Gateway::enable_design_defaults`를
한 번 호출할 수 있다. gateway를 공개하거나 capability를 만든 뒤 DRC를 추가하지 못한다.

등록된 layout/deck/dependency/cache/index-lock 외에 DRC 입력·waive·SVRF·현재/향후 ICE
tree와 launcher의 세션 자격증명 파일·private directory도 보호한다. 별도 보호 경로는
출력 경로를 늘리지 않고 제한만 추가한다. target뿐 아니라 지속 lock도 검사하며,
대소문자 무시 파일 시스템에서 다른 이름·nlink1로 같은 입력을 가리키는 경우도 inode로 거부한다.

모든 HTTP 요청은 기존 exact Origin/Host·owner cookie·CSRF·16KiB body 제한을 따른다.
입력 DTO는 알 수 없는 필드를 거부한다. 브라우저는 path, source path, mode, text를
지정하지 못하며 서버의 **현재 view와 state revision**에서 source/mode/설정 text를 얻는다.

| API | 역할 |
|---|---|
| `POST /api/v1/defaults/prepare` | `{view_id,state_rev}`를 읽기 전용 draft로 준비 |
| `POST /api/v1/defaults` | `{seq,view_id,state_rev,token,approve:true}` 명시 승인,202 |
| `GET /api/v1/defaults` | capability 상태와 전용 operation ledger |
| `GET /api/v1/defaults/{seq}` | 진행/최종 receipt 조회 |
| `POST /api/v1/defaults/{seq}/cancel` | 실제 작업에 취소 요청,202; 즉시 취소 완료를 뜻하지 않음 |
| `POST /api/v1/defaults/revoke` | `{token}`에 해당하는 미승인 draft 폐기,204 |

준비는 파일을 만들거나 lock을 잡지 않는다. 응답에는 basename, title, mode, 선택 levels,
rows/bytes, `replaces_existing`, `scope:shared_design_default`, `affects:future_opens`와
30초짜리 opaque token만 있다. 전체 서버 경로·native 오류·설정 text는 응답하지 않는다.
최신 draft 하나만 보관하고 새 prepare는 이전 draft를 폐기한다. token은 owner/view/revision에
고정되며 같은 숫자의 revision을 가진 다른 view·덱 mode 재open에 사용할 수 없다.

**게시 내용은 현재 모델의 전체 설정 파일이며 다른 파일과 병합하지 않는다.** 일부 덱 level만
열었다면 그 선택 모델의 rows로 해당 mode 기본값 파일을 교체한다. 후속 UI는 선택 level,
파일 교체 여부와 다른 사용자의 향후 open에 영향을 줄 수 있음을 승인 전에 표시해야 한다.
전체 덱 설정을 보존하는 자동 병합으로 설명하면 안 된다.

### 수명·응답과 비용

prepare의 파일 읽기/직렬화는 semaphore1의 blocking task에서 수행한다. draft1·게시 작업1,
기존 text4MiB/65,536행, 최근 receipt32개로 유계다. 게시 전용 native thread가 코어를 호출하며
HTTP/event loop나 view/controller lock을 파일 I/O 중 잡지 않는다. 활성 게시 중 새 준비는429다.

승인 시 현재 view/revision을 다시 확인한 시점이 acceptance point다. 이미 인정한 같은
seq/body는 현재 view가 바뀌어도 receipt를 replay하고 새 게시를 실행하지 않는다. 다른 body는
409, 오래된/소비된 token은410이다. 알아본 token의 stale view/revision은 폐기한다.
유효한 승인 뒤 navigation/style 변경은 **이미 승인한 immutable 설정 bytes**를 바꾸지 않는다.
게시 commit까지 view revision을 잠그는 계약은 아니다.

상태는 queued → publishing → succeeded/failed/cancelled다. `published:true`가 commit 결과이고
늦은 cancel을 받아도 취소로 다시 쓰지 않는다. `directory_synced:false`는 게시 완료·durability
경고다. 게시 전 오류는 `published:false`와 안전한 오류 코드로 알리며 native path/로그는 숨긴다.
파일/속성 충돌이나 비협조 writer 문제의 코어 계약은 §18 그대로다.

POST 전송 뒤 연결이 끊기면 결과는 미확정일 수 있다. 같은 서버 세션에서 GET/원래 seq의
명시 확인으로 해결하고 **새 seq로 자동 재게시하지 않는다**. ledger는 서버 메모리이며
서버 종료/재시작에 걸친 durable idempotency를 제공하지 않는다. logout/서버 종료는 미승인
draft를 버리고 승인 작업에 stop을 요청한다. 실제 commit 결과를 우선하며 다른 열린 GUI의
default 교체 감지/실시간 전파는 여전히 보류다.

기존 HTTP5초 deadline에서 취소된 prepare는 stop을 전달하고 결과를 버린다. 서버는 종료 시
작업 drain에4초 관찰 한계를 둔다. 그러나 NFS/SMB 등의 **블로킹 syscall을 강제 중단하는
시간 상한은 아니다**. task/thread 수는 제한되지만 커널 I/O가 멈추면 최종 join도 지연될 수 있다.
이를 현장 파일 시스템의 lock/ACL/종료 수용을 통과한 것으로 표시하지 않는다.

### 검증·후속

로컬 단위 테스트는 app11·app-core156·web44가 통과했다. 추가 항목은 owner별 draft 폐기,
최신/만료 token·semaphore1·종료 후 준비 거부, 승인 작업 취소 후 ledger drain·미게시,
임의 path/text DTO 거부와 추가 등록 파일/inode alias 보호다.

owner 실제 HTTP/native 통합은9개다. 기본 off/auth/명시 승인, read-only prepare,
same-seq replay·서로 다른 body, revision 변경·revoke·외부 파일 충돌, DRC 입력 보존,
덱 level→chip→layer 닫기/재열기의 token 무효화·선택 level preview·mode별 게시를 확인한다.
`validate_web_cli.py`는 PATH를 비운 Rust 실행에서 env 빈 값과 nonempty `0` 구별,
세션 credential이 게시 lock 경로와 충돌하면 준비를 거부하는 것, logout/SIGINT 정리를 단언한다.
모든 쓰기는 전용 합성 임시 fixture이며 실제 사용자 default 파일은 바꾸지 않는다.

Rust1.89에서도 app11·app-core156·web44를 실행했고, Linux x86-64 musl의 release
`floe2-web` static-pie 링크를 확인했다(Linux 실행 검증은 아님). scoped fmt와 app/core/web
all-target strict clippy도 통과했다. 기존 dependency/deprecation 경고는 별도다.
최종 `sh tools/validate_rust.sh`는 종료 코드0·`RUST VALIDATION: ALL OK`로 완료됐다.
owner9·CLI opt-in/credential 보호·GTK 게시20개 오라클, 기존 jobdeck80·renderer46,
KLayout13 PX+2 phase-exact+14 style(jobs1/8), ES2017/전체 JS를 포함한다.
renderd0.12.87은 유지하며, 실제 브라우저 게시 클릭·현장 공유 파일 시스템 수용은 아직 아니다.
후속 M4d-4c는 별도 Publish design default 버튼, preview와 공유 영향 명시 승인,
진행/취소·미확정 결과 확인·재접속 receipt 복원이다. 일반 Save의 의미를 바꾸지 않는다.

## 20. M4d-4c — 공유 기본값 게시 UI

`FLOE_FILL_EDIT` opt-in의 `design_defaults:true`에서만 **Shared design default** 패널을
보인다. 기존 Load/Save와 시각·동작을 구분하며 기본 실행에서는 새 API polling도 없다.
일반 Save는 파일 다운로드이고, 게시 UI에 파일 업로드·서버 경로 입력·자동 저장은 없다.

### 확인과 승인

1. 현재 연결·view ID·state revision과 미완료 편집이 없는 상태에서 **Review publication**을
   누르면 읽기 전용 prepare만 호출한다. layout뿐 아니라 덱 mode·선택 levels도 서버 preview를
   사용한다. basename, title, mode, rows/bytes, Create/Replace를 textContent로 표시한다.
2. **전체 공유 파일 교체, 다른 사용자의 향후 open에도 영향, 열린 창에는 즉시 전파하지 않음**을
   표시한다. 선택 level만 연 덱이면 그 행만 저장하고 나머지를 병합/보존하지 않음을 명시한다.
   설명을 읽고 승인 체크를 켜야 **Publish shared default**가 활성화된다.
3. 클릭 후 최신 ledger를 GET으로 재확인하고 같은 view/connection/revision·유효한30초
   draft인지 다시 검사한다. 중간에 다른 게시가 있거나 local edit가 대기하면 다시 검토해야 한다.
   승인 token을 revoke하지 않고 원래 seq/body로 POST한다.
4. Dismiss/Escape, 만료, view/epoch/revision 변경은 미승인 draft를 폐기한다. 알려진 token은
   best-effort revoke하며, 전송 중 끊겨 token을 모르면 서버 만료에 맡긴다. Escape는 시작
   버튼으로 포커스를 돌린다. 이미 승인한 작업을 닫기/이동으로 취소한 것처럼 표시하지 않는다.

custom bitmap 또는 Calibre에서 보존되지 않는 child 선폭 override는 Native JSON Save를
안내한다. 파일 ACL/권한 등 코어의 다른 Unsupported도 근사 저장하지 않고 오류로 남긴다.
시각 검증에서 확인한 긴 패널과 후속 Index 영역의 겹침은 layers section의 non-shrinking
높이와 별도120..360px layer list로 수정했다. 전체 sidebar는 스크롤하며 canvas 폭은 유지한다.

### 미확정 요청·종료

승인 POST 직전에 원래 `{seq,view_id,state_rev,token,approve:true}`와 owner session ID를
현재 origin의 `sessionStorage` 한 entry에 보관한다. full 설정 내용이나 서버 경로는 저장하지
않고, 복구 입력은2KiB/schema/ID/u64를 검증한다. 다른 session 또는 손상된 entry는 전송하지
않고 명시 확인을 요구한다. 이 entry는 서버 receipt의 영구 저장소나 새로운 권한이 아니다.

응답 유실·408·5xx·잘못된 receipt는 **Outcome unknown**이다. 자동 GET/reload/bfcache 복원은
새 게시를 하지 않는다. **Resolve original request**를 누른 경우에만 동일하게 승인했던 body를
재전송한다. GET의 같은 seq 기록만으로 token/signature 일치를 단정하지 않는다. 확인된 승인
응답은 entry를 지우고 서버 진행을 추적한다. 세션 내 frame/viewport가 바뀌어도 기존 요청의
결과는 조회 가능하다. `operation_expired`는 과거 commit 가능성이 있어 미게시로 단정하지 않는다.

history가 소실되거나 요청 record가 손상되어 확인할 수 없으면 사용자가 공유 파일을 확인했다는
별도 체크 후 **Clear local record**를 선택할 수 있다. 이는 로컬 record만 버리며 서버 파일,
history, 작업을 삭제하거나 새 게시를 보내지 않는다. 게시가 활성 상태일 때는 허용하지 않는다.
storage 사용 불가/저장 실패 시 복구 제한을 표시한다. GET/reload만으로 자동 재시도하지 않는
원칙은 같지만, storage가 없으면 페이지를 닫은 뒤 미확정 body를 복원할 수 없다.

승인 결과는 queued/publishing/succeeded/failed/cancelled receipt로 표시한다. 취소 버튼은
요청만 보내며, 이미 `published:true`면 성공을 유지한다. `directory_synced:false`는
**게시 완료·내구성 미확인**이며 재게시를 권하지 않는다. 일반poll2.5초/활성poll0.5초,
draft1·GET/prepare/write/cancel 각각1·revoke1+최신 대기token1로 유계다.
페이지 종료는 XHR/timer를 정리하고 미확정 승인 record를 보존한다. 명시 End session은
record를 지우고 기존 서버 logout/stop 정책을 따른다. actual commit 확인 없이 파일이
롤백됐다고 주장하지 않는다. NFS/SMB·다른 GUI 캐시 교체의 §18/19 한계는 그대로다.

### 검증

`defaults.test.cjs`는 opt-in off·read-only preview·unchecked 승인 거부·만료/epoch/view/revision,
늦은 prepare·revoke·Escape, 승인 뒤 응답 유실/페이지 종료·새로고침 시 같은 요청만 복구,
409/408/expired history 구분·명시 local record clear·cancel/commit·dir-sync 경고,
다른 tab의 seq 경합, u64>2^53·overflow, storage/schema·plain-text 표시를 고정한다.
`client.test.cjs`의 별도 defaults 실행은 실제 app.js의 auth/capability·checkbox/승인·저장소
정리·Escape를 연결하고, frame pixels/WS view.set을 새로 만들지 않는 것을 단언한다.
기존 settings의 Unsupported 안내도 bitmap뿐 아니라 inherited-width override를 포함한다.
새 JS는 ES2017 gate, 실제 bundle route와 content hash에 들어가며 defaults API 소스도
bundle fingerprint에 포함한다. 전체 JS 게이트와 Rust app11·app-core156·web44,
scoped fmt/all-target strict clippy, Rust1.89 Linux musl static-pie 링크가 통과했다.

로컬 Chrome의 합성 valmini 사본에서 미리보기·체크 전 게시 비활성·30초 만료·Escape 폐기와
포커스 복귀를 직접 확인하고 screenshot으로 패널 겹침 수정을 재확인했다. 미승인 상태에서는
`.layerprops`와 `.lock`이 생성되지 않았다. 당시 브라우저의 **실제 게시 클릭은 별도 승인 대기**였으며,
이 시각 검증을 성공 게시/취소 클릭의 실제 browser acceptance나 현장 Firefox/ETX 수용으로
대체하지 않는다. 서버의 실제 파일 쓰기는 §19의 독립 합성 HTTP/native 테스트 범위다.
전체 `sh tools/validate_rust.sh`는 `RUST VALIDATION: ALL OK`로 완료됐다.
owner9·jobdeck80·renderer46, KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 포함한다.
배터리 실행 뒤 미확정 결과 확인 체크의 초기화를 보강했고, 최종 소스에서 전체 JS와
launcher CLI 게이트·release bundle·Rust1.89 Linux musl 링크를 다시 통과했다.
그 검증의 브라우저와 서버는 종료했으며 공유 기본값 파일은 생성하지 않았다.

2026-09-14 추가 승인 후, 로컬 Chrome에서 같은 전용 합성 `synthetic.oas`에 대해
**실제 Publish shared default 버튼**을 실행했다. 첫 preview는 조작 중30초 만료로 폐기되어
게시되지 않았으며, 새 preview의 Create·9 rows·262 bytes와 승인 체크를 거쳐
`Queued #1` → `Published #1 synthetic.oas.layerprops` 및 향후 open에 적용된다는 안내를 확인했다.
대상 `/private/tmp/floe-default-ui.AeRhyd/synthetic.oas.layerprops`는262바이트·mode0600·9개
레이어로 생성됐고, stable `.lock`은0바이트였다. 합성 OASIS 및 OVM/OVP/OVT/meta.json의
SHA-256이 게시 전후 동일했다. End session의 종료 화면과 서버 exit0을 확인하고 탭을 닫았다.
이 결과는 합성 layout의 **신규 파일 게시 성공**에 한정한다. 실제 browser의 기존 파일 교체,
게시 취소·응답 유실 복구·덱별 게시와 현장 Firefox/ETX·NFS/SMB 수용을 대신하지 않는다.
생성한 합성 기본값은 관찰 결과로 남겼으며 사용자 설계/기존 기본값은 변경하지 않았다.
M4 전체와 GTK 은퇴는 아직 완료가 아니다.

## 21. M4e-1 — DRC review sidecar codec과 주석 그룹 모델

`app-core::drc::review`에 Python `IcePack`의 waive import/export와 shared note
편집·`.fe` 직렬화를 이관했다. **순수 library 단계**다. 파일 경로/계정/reviewer 탐색,
자동 저장·삭제, 기존 pack 수정, HTTP/WS 권한·쓰기 endpoint를 추가하지 않는다.
현재 웹 DRC actor는 여전히 읽기 전용이다. DRC-02의 저장/충돌/API/UI 전체가 완료된 것은 아니다.

### waive: 64KiB 스트리밍

- `Layout`은 pack의 source size·mtime·total과 파일 순서의 rule counts를 검증한다.
  빈 rule도 counter 자리를 유지한다. 합계 불일치·u64 산술 overflow·rule별 u32 초과는
  거부하며 FLOEWAIV v1의40-byte header, status byte 배열, rule별 little-endian u32를 유지한다.
- `rewrite_waives`는 header와 정확한 전체 입력 길이를 확인하고, status를64KiB씩 읽어
  요청된 전역0-based gid 변경을 적용한다. `status == 1`만 waived로 세며2..255도 보존한다.
  입력 counter는 신뢰하지 않고 재계산한다(Python `waive_import`와 동일).
  중복 변경 gid는 마지막 값이 우선하며 한 호출 최대5000개다.
- 메모리는 오류 총수에 비례하는 status 벡터가 아니라 **rule counts + 변경 집합 +64KiB**다.
  300만 오류 가상 reader/계측 sink 테스트가 전체 status 배열 없이 이 경로를 실행한다.
  rule 수 자체도 기존 metadata 한도와 연계해 최대4,194,304개로 제한한다.
- 출력 인자는 **미게시 staging sink**여야 한다. 손상된 말미/추가 바이트·I/O 실패·취소 시
  sink에는 prefix가 남을 수 있다. 성공 반환은 파일 commit/fsync 완료가 아니다.
  실제 저장 계층이 전체 검증·동시 수정 확인 후 교체해야 한다. chunk 사이 취소는 확인하지만
  blocking Read/Write 자체를 즉시 중단하거나 NFS deadline을 보장하지 않는다.

### 주석: 그룹과 오류 ID 보존

- 하나의 주석이 여러 전역 gid를 공유한다. 새 선택에 note를 붙이면 기존 그룹에서 그 멤버만
  분리하고, 남은 멤버는 기존 문구를 유지한다. 빈/공백 문구는 선택 멤버만 지운다.
  그룹은 생성 순서, 각 그룹의 members는 숫자 순서로 직렬화한다.
- 잘못된 gid·과대 text/membership·취소·sequence overflow는 **메모리 변경 전에** 검사한다.
  입력을 조용히 일부 적용하지 않는다. 유효한 반복 gid는 집합으로 취급한다.
- `.fe`의 authoritative 데이터는 `floe_pack=`과 `floe_note=`다. `text=` mirror로
  멤버를 재구성하지 않는다. export는 기존 escape(`\\`, `\n`)·색`#FFD819`·16px/검정
  translucent 배경과 error bbox 중심의 text mirror를 같은 annotation codec으로 만든다.
  한글·탭·여러 줄·literal backslash·pipe를 그대로 round-trip한다.
- 중심 callback 실패/NaN은 export 오류다. Python의 예외 fallback `(0,0)`을 복제하지 않는다.
  빈 모델은 `None`을 반환할 뿐, 기존 파일을 삭제하지 않는다.
- 한 edit 최대5000 gid, 모델100,000 membership, 한 문구64KiB, 모델 문구 합과 import/export
  각각16MiB다. mirror가 많으면 export 한도가 먼저 찰 수 있다. 오류로 표시하며 잘린 정상
  파일을 내보내지 않는다. 이 수치는 RSS 상한이나 서버의 총 admission 예산이 아니다.

### 의도적으로 보강한 호환 경계

1. note import는 **정확히 한 개의 matching fingerprint**를 요구한다. 기존 Python의
   fingerprint 없는 파일 허용·마지막 tag 우선은 다른 run의 gid 적용 위험 때문에 따르지 않는다.
2. malformed note 행/빈 내용/무효 gid는 기존처럼 건너뛰되 `ImportReport`에 행/멤버 수를
   반환한다. gid 표기는 ASCII decimal만 받으며 Unicode digit·overflow도 무효 멤버로 센다.
   지원하지 않는 control/line separator 문자와 자원 한도는 전체 import 오류다.
3. 서로 겹친 import 그룹은 **마지막 할당 우선 + 이전 그룹에서 detach**다. Python은 lookup은
   마지막을 읽어도 옛 그룹 membership을 남겨, clear/export 뒤 과거 note가 되살아날 수 있다.
   Rust는 그 모순을 남기지 않으며 재할당 수를 별도 반환한다. 정상 GTK 산출물의 bytes는 같다.
4. 이 fingerprint는 size/초 단위mtime/total이라는 **레거시 호환 표식**이다. 같은 값의 별도
   DRC run을 식별하거나 동일 권한을 증명하지 못한다. 저장 계층은 별도의 현재 pack identity,
   인증된 reviewer scope, 기대 review revision을 결합해야 하며 tag 일치만으로 autosave를
   덮어쓰거나 기존 review를 새 pack에 자동 승계해서는 안 된다.

### 검증과 다음 단계

`tools/validate_drc_review.py`가 작은 adversarial DB와 생성 DB를 native pack으로 만들고,
Python 편집/export/import 결과를 Rust와 대조한다. **22개 주석 상태와5793 status bytes**의
FE/waive 전체 bytes, note lookup·그룹/멤버 순서, reserved status·counter 재계산을 확인한다.
Rust 테스트 실행은 `PATH`가 비어 있고 source/pack/sidecar 전체의 hash·mtime가 불변이다.
이 게이트는 `tools/validate_rust.sh`에 연결했다. native unit은 foreign/truncated/extra 입력,
취소·write 실패·bounded streaming, 그룹 원자성·재import·상한·정확한u64 gid를 검증한다.
최종 `sh tools/validate_rust.sh`는 종료 코드0·`RUST VALIDATION: ALL OK`로 완료됐다.
app11·app-core165·web44, 새 review 대조, jobdeck80·renderer46,
KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 포함한다. review unit9개는 Rust1.89에서도
통과했고, 최종 scoped fmt·app/core/web all-target strict clippy와 Linux x86-64 musl
release static-pie 링크를 확인했다. Linux 실행/현장 Firefox 수용은 아니며 기존 dependency/
GTK/Pillow 경고는 별도다. renderd0.12.87은 유지하며 실제 서버 쓰기 경로는 바꾸지 않았다.

다음은 이 codec을 사용하는 **명시적 로컬 review 저장 계층**이다. pack/파일 identity와
revision 충돌, atomic staging/commit, reviewer 구분, 실패 시 기존 파일 보존을 먼저 고정한 뒤
owner/API/UI에 연결한다. GTK의 read-only 폴더→전역 temp→pack 내부 pwrite fallback이나
stale 파일의 자동 이동/삭제를 그대로 이식하지 않는다. 공유 권한/현장 Firefox 수용은 별도다.

## 22. M4e-2a — pack에 고정한 로컬 review 저장

`app-core::drc::review::store`는 승인된 root 안의 **pack·reviewer·종류별 한 파일**에 대한
로컬 쓰기 capability다. `Store::open`/`snapshot`/`prepare_*`는 파일을 만들지 않는다.
실제 쓰기는 준비 결과를 소비하는 `Draft::publish`뿐이다. reviewer 문자열은 인증이 아니며,
원격 요청이 경로/reviewer를 골라 이 constructor를 직접 부를 수 있는 API는 없다.
기존 웹 DRC actor·공유 권한·GUI 자동 저장·CLI 옵션은 변경하지 않았다.

### 읽기·준비·게시

- 이름은 기존 GTK와 같은 `.<db>.waive.<reviewer>`, `.<db>.notes.<reviewer>.fe`다.
  원본 pack, caller가 등록한 다른 입력/캐시/credential 경로와 alias는 보호한다.
  심볼릭 링크·여러 hardlink·비정규 파일을 거부하며 전역 temp fallback이나 pack pwrite는 없다.
- `Snapshot`은 외부에서 조립/역직렬화할 수 없는 기대 review 버전이다. 파일의 inode·size·
  mtime/ctime(ns)·owner/group/mode/nlink·ACL/xattr와 streaming content digest를 캡처한다.
  digest는 변경 검출용 SHA-1이며 인증 서명이나 악의적 same-UID 프로세스 방어가 아니다.
  gateway의 `review_rev`와 승인 receipt는 이후 actor가 별도로 매핑해야 한다.
- waiver가 없으면 pack 내부의 기존 status/counter를 read-only 가상 스트림으로 읽어
  첫 sidecar를 만든다. 기존 waiver는 header/전체 길이를 검사하고 counters를 재계산한다.
  상태 변경은 최대5000 gid의 한 batch이며 reserved byte도 보존한다.
- 주석은 실제 pack의 전역 gid→rule/local ID→bbox 중심을 사용한다. 기존 codec의 그룹 편집,
  note import의 replace 의미·경고 report를 유지한다. 준비/승인 전에 import report를 보여야 한다.
  중심 읽기 오류를 원점 주석으로 대체하지 않는다.
- publish는 안정된 `.lock` inode의 nonblocking exclusive flock을 얻고 snapshot을 다시
  확인한다. 같은 디렉터리의 전용 private staging에 전체 결과를 쓴 뒤 권한/속성 적용,
  file sync, pack/대상/lock/staging 재검사 후 rename으로 교체한다. 기존 파일이 없으면
  link-at로 생성해 그 사이 다른 파일이 생겼을 때 덮어쓰지 않는다.
- `layer_defaults`의 descriptor-relative Directory/Stage와 OS Security 보존 코드를
  crate 내부에서 재사용했다. 기존 기본값의 temp 이름·권한·게시 정책은 유지한다.
  단, macOS 동시 최초 게시 테스트에서 lock의 `O_CREAT` open이 간헐적 ENOENT를
  반환해 lock 열기를 `O_CREAT|O_EXCL` 생성 → EEXIST일 때 기존 inode 열기로 분리했다.
  두 저장 경로에 공통 적용하며, 다른 오류는 숨기지 않고 lock은 지우거나 truncate하지 않는다.
  review 신규 파일은0600이며, 기존 파일은 원래 ACL/xattr/권한을 유지한다(아래 owned binding만
  추가/확인). read-only 파일은 디렉터리 쓰기 권한만으로 우회 교체하지 않는다.
- 주석을 모두 지울 때 파일을 삭제하지 않고 matching fingerprint가 있는 빈 FE를 게시한다.
  GTK도 빈 주석으로 읽으며, 파일 부재가 다른 writer의 신규 생성으로 오인되는 일을 줄인다.
  이 empty tombstone은 Python의 빈 note 파일 삭제와 다른 저장 정책이다.
- 검사에서 발견된 동시 변경/삭제/동일 bytes의 inode 교체·120초 draft 만료는 실패다.
  실패/취소는 기존 대상 파일을 보존하고 소유한 staging entry만 정리한다. 다른 writer가
  대상 자체를 지웠다면 그것을 되살리는 기능은 아니다. commit 뒤 취소나 directory sync
  실패는 성공을 취소로 바꾸지 않으며 `Published { directory_synced:false }`로 경고한다.

### 재시작과 legacy binding

FLOEWAIV/FE의 size·초mtime·total은 유일한 DRC run ID가 아니다. 새 review에는 다음 owned
확장 속성을 **같은 staging inode에 설정한 뒤 함께 commit**한다. 별도 manifest 두 파일을
순차 교체하는 방식이 아니다. waive/FE의 기존 내용 형식은 바꾸지 않는다.

- macOS: `com.floe.review-pack-v1`, Linux: `user.floe.review-pack-v1`.
- 값: 현재 open pack의 dev/inode/size/mtime(ns)/ctime(ns). 경로·reviewer credential은 없다.
- matching binding이면 재등록/재시작 뒤 읽기·수정이 가능하다. binding이 다른 pack이면
  legacy header가 같아도 Cache 오류다. 복사/교체/touch된 pack도 보수적으로 별도 run으로
  판단할 수 있다. **내용 해시 기반의 이동 가능한 run ID나 완전한 index revision 관리가 아니다.**
- binding 없는 기존 파일은 `legacy_unverified()`로 표시한다. 읽기/준비는 가능하지만
  게시 전에 caller의 명시 `accept_legacy_run()`이 필요하다. 이는 해당 파일이 이 run의
  review라는 별도 확인이며, 서로 다른 binding을 강제로 덮는 옵션은 아니다.
- GTK 등 다른 도구가 파일을 새 inode로 저장하며 xattr를 버리면 다시 legacy 확인이 필요하다.
  xattr를 보존한 복사본이 다른 pack을 가리키면 자동 승계하지 않는다. foreign review의
  안전한 보관/초기화·전체 waive import/export UI는 후속 범위이며 자동 aside/delete는 없다.
- xattr를 지원하지 않거나 binding을 쓰지 못하면 **게시 전 실패**한다. NFS/SMB의 xattr,
  inode/시간 정밀도·flock·rename·fsync 의미는 현장 수용 항목이다. 로컬 macOS 검사나 Linux
  링크만으로 이 보장을 해당 파일 시스템에 일반화하지 않는다.

### 비용·동시성 한계와 다음 단계

현재 waiver 편집은 header/status/counter 전체를 streaming 재작성하고 검증 시 전체 digest를
읽는다. 메모리는 오류 총수에 비례하지 않지만 **I/O는 O(waive 파일 크기)**이며, 과거 GTK의
status/counter pwrite O(수정 수)보다 비용이 크다. 작은 개별 클릭마다 즉시 호출하는 autosave
UI를 붙였다고 주장하지 않는다. 실제 actor 연결에서 batch/coalesce·작업 admission·취소/receipt를
묶고 대형 review 실측을 해야 한다. status journal/증분 트랜잭션은 필요 시 별도 설계 대상이다.
주석 준비는 현재 모델 전체의 FE mirror를 직렬화하며 기존16MiB export 한도를 따른다.

각 파일은 원자적이지만 waive+notes 두 파일을 합친 하나의 트랜잭션은 아니다. 동시 writer는
같은 stable lock 규약을 따라야 한다. GTK 등 비협력 writer가 최종 검사 이후 쓰는 경우까지
막는 filesystem CAS는 아니며, 악의적인 같은 UID/관리자 격리도 아니다. pack은 게시 후
immutable하게 취급해야 한다. 서비스 연결 시 현재 DRC/read lease를 작업 수명까지 유지해
같은 서버의 pack rebuild를 직렬화하고, source/pack 교체 뒤 오래된 주석/선택을 승계하지 않는다.
blocking 파일 I/O는 즉시 중단할 수 없으며 lock은 의도적으로 unlink하지 않는다.

### 검증

native 저장 unit15개는 read-only 준비·초기 seed·reserved bytes·실제 중심·empty tombstone,
서로 다른 reviewer, 독립/동시 writer, 파일/pack/parent 교체·mtime 복원 후 in-place 변경,
symlink/hardlink·protected 경로·lock 경합/교체, 권한/xattr·read-only 대상,
pre-commit 오류/취소·late commit+directory-sync 실패, legacy 승인·새 pack binding 거부를 검증한다.
DRC 두 writer 중 정확히 하나만 성공하는 테스트와 공유 lock의 동일 inode·비절단 테스트를
각64회씩10번(각640회 경합) 반복 통과했다. ENOENT를 Busy로 치환하거나 테스트 기대값을
완화한 것이 아니라 위 lock 생성 경로를 바꿨다.
`validate_drc_review.py`의 별도 Rust 실행은 합성 파일에서 **28회 실제 게시**를 수행하고
Python 전체 bytes와 재로드/빈 주석을 대조한다. runtime PATH는 비어 있으며 원래 입력과
Python review의 bytes·mtime/ctime·mode·xattrs가 불변이고, 허용한8개 target/lock 외의
새 파일이나 남은 staging이 없는지 검사한다. macOS의 Python xattr API 부재는 개발 gate에서만
읽기 전용 `/usr/bin/xattr -lx`로 보완한다; 제품 runtime은 libc를 쓴다.
`cargo fmt -p floe-app-core --check`와 app-core/app/web의 strict all-target clippy가
통과했다. Rust1.89에서 app-core unit181개와 Linux `x86_64-unknown-linux-musl`
release static-pie 빌드도 통과했다(이 Mac에서 Linux 바이너리를 실행한 것은 아니다).
최종 `sh tools/validate_rust.sh` exit0: workspace unit(app11/core181/web44 포함),
위28회 저장 oracle, jobdeck80, renderer46, KLayout13 PX+2 phase-exact+14 style,
`RUST VALIDATION: ALL OK`를 확인했다. 이 단계는 renderer/worker protocol 변경이 없어
renderd 버전은0.12.87을 유지한다. TeeBox/Firefox 및 공유 파일 시스템 현장 수용은 미실시다.

다음 단계는 이 로컬 코어의 관리형 writer/수명·승인/review revision·명시 import/export와
owner API/UI 연결이다. M4 전체·DRC-02 전체·GTK 은퇴·현장 수용 완료는 아니다.

## 23. M4e-2b — 관리형 review 수명과 게시 작업

`app-core::drc::review::managed`가 §22 저장 코어를 서비스 자원 관리에 연결한다.
아직 HTTP 쓰기 endpoint나 자동 저장 UI는 아니며, 승인된 경로·reviewer를 제공하는
로컬 caller용 API다. `ManagedStore` → `Snapshot` → `Prepared` → `Publication`으로
실행권을 넘기며, 원래 expected snapshot·pack binding·원자 게시 계약을 그대로 사용한다.

### admission과 lease

- 등록은 off-reactor에서 수행한다. pack을 열기 **전** 기존 DRC admission pool에서
  CPU1 slot·256MiB를 예약하고 pack 및 caller가 지정한 protected files의 read lease를
  함께 잡는다. 등록 실패/취소는 예약을 반환한다. protected trees는 출력 제외 영역이며
  재귀적 read lease 집합으로 확장하지 않는다. 서비스는 실제 ASCII source/rules 등
  의존 파일을 등록해야 한다.
- 종류별 등록이다. notes와 waives를 둘 다 열면 각각의 pack/model에 대해 합계CPU2·512MiB를
  예약한다. 기존 reader/renderer의 예약에 추가된다. 이 수치는 admission이며 hard RSS,
  디스크 I/O 대역폭·지연시간 또는 다른 프로세스의 소비 상한이 아니다.
- 등록 하나당 살아 있는 snapshot/draft/게시 작업은1개다. 추가 호출은 Busy이며 숨은
  큐나 한 CPU 예약 뒤 무제한 model 보관을 하지 않는다. 읽기/준비는 동기 파일 I/O라
  서비스가 blocking worker에서 호출해야 한다. caller는 호출 전에 취소 flag를 소유하며,
  해당 작업이 끝나기 전에 flag를 reset/reuse하지 않는다.
- snapshot/draft/worker가 등록을 소유한다. 밖의 등록 핸들을 먼저 닫아도 실행 중인
  작업의 CPU/memory 예약과 pack/source lease는 남는다. 같은 Resources를 쓰는 pack
  rebuild와 충돌하며, 무관한 입력의 인덱싱은 기존 admission 범위 안에서 가능하다.

### 승인·취소·결과

`Prepared::publish`만 실제 저장 작업을 시작한다. draft는 한 번 소비되며 legacy 미확인은
별도 `confirm_legacy`가 필요하다. 이는 로컬 caller의 명시 승인이지 HTTP 인증이 아니다.
FE import의 경고 report도 승인 전에 caller가 보여야 한다. 웹의 owner·DRC identity·
review_rev·예상 파일 버전 token과 승인 seq/signature를 결합하는 것은 다음 owner actor의 책임이다.

`Publication`은 별도 join 가능한 thread에서 게시하고, 조회자가 없어도 typed status를
유지한다. process-local ID·kind·phase·elapsed·안전한 오류 종류·게시 outcome만 포함하며
원시 경로/오류 메시지를 wire용 status에 넣지 않는다. durable HTTP retry ledger가 아니므로
응답 유실 후 새 `publish`를 자동 호출하면 안 된다. owner가 이 핸들을 보관하고 동일 작업을 조회해야 한다.

- `request_stop`은 등록을 retire하고 진행 중 flag를 취소한다. 새 읽기/준비/게시를
  허용하지 않는다. 강제로 lease를 반환하거나 작업이 끝난 것처럼 표시하지 않는다.
- `cancel`은 best-effort 요청이다. 실제 commit이 끝났으면 Succeeded를 유지하며
  directory sync 실패는 게시 outcome의 내구성 경고다. 이미 게시한 것을 롤백됐다고 하지 않는다.
- worker panic은 `Failed / Worker / outcome_unknown:true`다. commit 뒤 panic 가능성이
  있으므로 미게시로 단정하거나 자동 재시도하지 않는다. 이 경우 결과 파일과 등록을 다시 확인해야 한다.
- `close`/Drop은 취소하고 join한다. HTTP reactor에서 호출하지 않는다. blocking OS I/O는
  즉시 중단할 수 없으며, 파일 시스템이 멈추면 정리도 기다린다. 프로세스 격리·강제 I/O deadline을
  구현한 것으로 간주하지 않는다.
- terminal은 이 게시 작업의 borrow 해제를 뜻한다. 여전히 등록 핸들이 살아 있으면 idle
  예약/lease도 살아 있다. rebuild 전에 owner가 registration·미승인 draft를 retire/drop하고
  진행 작업의 실제 종료를 기다려야 한다.

### 검증과 남은 연결

집중 unit10개가 admission 선행·실패/취소 반환·한 개 작업 제한·준비 시 무쓰기,
원본/pack rebuild 충돌·무관 입력 허용·핸들 drop 이후 lease 유지, legacy/import/revision,
native note/waive 게시·재로드, 취소 중 lease 보존·join, late commit/내구성 경고와
post-commit panic의 미확정 결과를 고정한다. 기존 Python oracle의28회 실제 게시도
이 managed 경로로 올려 전체 bytes·실제 bbox 중심·재로드·모든 예약 반환을 함께 단언한다.
PATH-empty 실행이며, 기존 입력/sidecar 불변성과 staging 정리는 같은 harness가 검사한다.
core unit191·scoped fmt·app/core/web strict all-target clippy를 통과했다. Rust1.89에서도
core191 및 Linux x86-64 musl release static-pie 빌드를 통과했다(Linux 실행 검증은 아님).
최종 `sh tools/validate_rust.sh` exit0·`RUST VALIDATION: ALL OK`: workspace unit
(app11/core191/web44), managed 저장28회 oracle, jobdeck80·renderer46,
KLayout13 PX+2 phase-exact+14 style(jobs1/8)를 포함한다. renderer/worker protocol은
바꾸지 않아 renderd0.12.87을 유지한다.

다음은 owner actor의 인증 reviewer/seq receipt·review_rev, 읽기 actor의 waive 상태 갱신,
명시 note/waive import/export 및 편집 UI다. 큰 waive의 O(파일 크기) I/O와 batch/coalesce
실측, 현장 Firefox/NFS/SMB는 여전히 남는다. M4 전체 완료나 GTK 은퇴를 뜻하지 않는다.

## 24. M4e-2c — reader/store pack 일치와 선택 waive 조회

관리형 store가 자기 pack을 검증하는 것과, 웹의 기존 읽기 actor가 **같은 pack**을
보고 있는지는 별개다. reader가 먼저 열린 뒤 외부에서 파일을 교체하면 새 store가
새 pack을 정상적으로 열어도 화면의 gid 의미와 달라질 수 있다. 저장 HTTP를 붙이기 전에
이 두 등록을 연결하는 검증을 추가했다.

- store/managed 등록에서 얻는 `review::Identity`는 비직렬화 opaque Rust 값이다.
  `Database::validate_review_identity`는 이미 열린 reader의 descriptor·등록 경로와
  store의 보수적 pack binding을 대조한다. 동일 legacy header나 동일 파일 bytes라도
  다른 inode는 거부한다. 같은 inode의 touch/변경·외부 교체도 명시 reopen 전까지 거부한다.
  ASCII reader는 이 검토 저장 경로의 대상이 아니며 먼저 pack이 필요하다.
- 웹 DRC `Service::validate_review_identity`는 기존 bounded read actor/Ticket으로
  검사를 수행한다. HTTP DTO에는 이 명령이나 filesystem identity 필드가 없다.
  취소·닫힌 actor·오류 코드는 기존 읽기 계약을 따른다. 인증이나 atomic commit 그 자체가
  아니므로 다음 owner coordinator는 이 검사와 registry의 현재 id/revision 확인을 모두
  사용하고, 승인 시점과 실제 저장까지 managed lease/expected snapshot을 유지해야 한다.
- `Snapshot::selected_statuses`는 최대5000개의 gid를 받아 입력 순서·중복과0/1 외의
  reserved status bytes도 그대로 돌려준다. sidecar가 없으면 embedded pack 상태를 쓴다.
  정렬 후 인접 gid만 묶어 positional read하며 드문드문 떨어진 gid 사이를 읽지 않는다.
  결과·임시 메모리는 O(선택 수)다. 실패/범위 초과/취소 시 부분 결과를 성공으로 반환하지 않는다.
- 조회 전후 expected snapshot을 검사한다. 새 파일 생성/원자 교체를 현재 상태로 몰래
  받아들이지 않으며 managed 등록이 retire되면 읽기도 중단한다. **기존 sidecar digest
  검증은 그대로여서 전체 조회 I/O가 O(선택 수)가 되는 것은 아니다.** 현재 경로는 전후
  전체 sidecar hash를 읽는다. 큰 waive 파일의 batch/지연·I/O 실측 과제는 남아 있다.

### 검증

새 core unit4개가 reader/store 동일성, 같은 bytes의 다른 pack·교체·touch·ASCII 거부,
선택 상태의 순서/중복/reserved bytes·범위/개수 상한·취소·stale snapshot과 managed
lease 수명을 검사한다. positional helper는5000개 순서 섞인 선택,16GiB sparse gap,
offset overflow와EOF도 확인한다. 기존 unit과 함께 core195·web44 및 strict all-target
clippy를 통과했다. 기존28회 native 게시/Python oracle에도 선택 상태 byte 대조를 넣었다.
`validate_web_drc.py`는 실제 actor에 올바른/다른/교체된 pack identity를 보내 확인하며
읽기·검증이 sidecar/lock을 만들지 않는 것도 단언한다. pack/ASCII HTTP 회귀는 통과했다.
Rust1.89 core195와 Linux x86-64 musl release static-pie 빌드도 통과했다(Linux 실행은 아님).
최종 `sh tools/validate_rust.sh` exit0·`RUST VALIDATION: ALL OK`: workspace unit
(app11/core195/web44), 위28회 native 게시 oracle, jobdeck80·renderer46,
KLayout13 PX+2 phase-exact+14 style(jobs1/8)를 포함한다. 검증용 임시 venv 링크는 제거하고
기존 venv 및 승인된 합성 shared-default 게시 결과물은 보존했다.

다음은 고정 reviewer를 owner 인증에 결합한 편집·승인/receipt API와 UI다. 이 단계에는
새 HTTP 쓰기 endpoint·자동 저장·reviewer CLI 옵션을 추가하지 않았다. 기존 launcher와
renderd0.12.87은 그대로이며, TeeBox/Firefox·NFS 수용 및 M4 전체 완료는 아니다.

## 25. M4e-3a — owner 주석 편집·승인 API

`floe2-web view layout.oas --drc results.db.ice --drc-reviewer alice --no-open`으로
등록된 pack의 **주석** 편집 API를 명시적으로 켤 수 있다. 기본은 꺼져 있다.
reviewer는 trusted launcher의 고정 설정이며 owner bootstrap 세션에 연결한다.
환경의 `FLOE_REVIEWER`나 브라우저의 reviewer/path/gid 필드는 채택하지 않는다.
공유 TeeBox 계정에서 실제 사람을 식별하는 인증/RBAC 구현은 아니며, trusted launcher
자체를 조작할 수 있는 운영자로부터 다른 tag를 격리한다는 뜻도 아니다. guest 권한은 없다.
`--drc-waives`는 계속 명시된 기존 waive의 읽기 등록이며, 이 옵션이 waive 쓰기를 켜지는 않는다.
ASCII 등록은 owner pack-build 후 주석을 읽을 수 있다. 옆 `.ice`를 몰래 선택하지 않는다.

### 요청과 승인

공통 prefix는 `/api/v1/drc/review/notes`다. 모든 요청은 기존 cookie+CSRF·Host/Origin
검사를 받는다. `context`는 현재 `drc_id`, `revision`, `view_id` 세 문자열이다.

| 메서드 / suffix | 동작 |
|---|---|
| GET (prefix 자체) | 고정 reviewer·진행 상태·process-local `review_rev`·receipt 목록 |
| POST `/read` | `context`, `errors:[{check,error}]`로 snapshot 준비. 파일 무쓰기 |
| POST `/prepare` | `context`, snapshot `token`, `text`로 수정 준비. 파일 무쓰기 |
| POST (prefix 자체) | `seq`, `context`, prepared `token`, `approve:true`, `confirm_legacy`로 게시 승인 |
| GET `/{seq}` | 응답 유실/재접속 후 같은 작업 결과 조회 |
| POST `/{seq}/cancel` | 승인된 작업의 best-effort 취소. 이미 게시한 파일의 undo는 아님 |
| POST `/revoke` | `token`으로 미승인 snapshot/draft 폐기 |

- check/local은 기존 읽기 API와 같은 **0-based** 정규 십진 문자열이다. 화면의 global
  번호는1부터 시작하므로 저장 gid로 쓰지 않는다. read actor가 store의 opaque pack
  identity를 확인한 뒤 check/local을0-based gid로 변환한다. 최대5000개이며 중복은 합친다.
- snapshot은 현재 파일의 expected revision을 소유하고120초 유효하다. 같은 owner에서
  새 read를 하면 이전 미승인 상태는 폐기한다. 선택 멤버의 공통 주석 또는 mixed 표시,
  기존 주석 개수·파일 존재·legacy 미확인·파싱 report를 반환한다. 모든 note 그룹을
  JSON으로 복제하지 않는다. 브라우저는 snapshot 조회를 배경 polling으로 반복하면 안 된다.
- prepare는 snapshot을 한 번 소비하고30초짜리 새 token을 발급한다. selection은 read에서
  고정되며 prepare/submit으로 다른 errors를 끼워 넣지 못한다. 실제 저장될 trim 결과,
  clear 여부, 대상 이름, 기존 파일 교체 여부와 파싱 report를 돌려준다. 빈/공백 주석은
  선택 멤버만 지우며 전체가 비어도 fingerprinted FE tombstone을 남긴다.
- 승인 시 registry의 현재 reader와 등록된 view를 확인한다. 그 뒤의 이동은 이미 승인한
  선택·텍스트를 바꾸지 않는다. 같은 seq+승인 본문은 기존 receipt를 돌려주며 새 게시를
  만들지 않는다. 다른 본문은 conflict, history32개에서 밀린 과거 seq는 expired다.
  process 재시작까지 지속되는 durable 작업 DB는 아니다. 미확인 legacy는 별도
  `confirm_legacy:true`가 필요하고, 다른 pack에 기록된 binding은 이 flag로 우회하지 못한다.

### 수명·출력 보호

주석 owner 작업은 bounded native thread에서 실행하며 HTTP 구독자 수명과 분리된다.
read/prepare의 취소는 작업 flag로 전달하고 native 작업이 끝날 때까지 admission/lease와
body slot을 보존한다. 게시가 실제로 끝난 경우 늦은 취소로 cancelled로 바꾸지 않는다.
directory sync 실패는 `published:true`인 내구성 경고다. worker panic 등의 결과 불명은
`outcome_unknown:true, published:null`로 표시하고 자동 재시도/롤백을 주장하지 않는다.

read/prepare는 최대1MiB body를 허용하며 settings import와 공통 large-body slot 하나를
공유한다. 주석 한 개는 trim 후64KiB, 응답은 공통 주석 한 개라 JSON escaping 후에도
1MiB 안이다. 일반 요청은 기존16KiB다. native 등록은 CPU1·256MiB와 pack/DRC source/rules
read lease를 추가로 예약한다. hard RSS/I/O deadline은 아니며 큰 sidecar의 전후 digest와
전체 FE 재작성 비용은 남는다. 이 단계는 per-click autosave 성능 검증이 아니다.

저장 코어의 `open_guarded`는 등록된 layout/jobdeck과 그 의존 파일·캐시·index lock도
출력 제외 대상으로 검사한다. DRC/rules/명시 waive/세션 credential 및 private directory는
별도 보호한다. notes 등록은 design-default writer보다 먼저 하므로 그 writer도 미래의
review target/lock을 보호한다. native target은 고정된 adjacent name뿐이며 temp fallback,
pack pwrite, 기존 파일 삭제, 임의 경로 다운로드/게시를 추가하지 않는다.

pack-build는 진행 중인 read/prepare/게시가 있으면 Busy다. 올바른 build 승인 시 미승인
preview만 폐기하고 그 lease를 놓은 뒤 기존 rebuild 경로로 간다. accepted note receipt는
새 DRC id가 등록돼도 보존한다. force rebuild로 같은 bytes를 만들었더라도 pack identity가
달라지므로 이전 sidecar는 자동 승계하지 않는다. 명시 import 연결은 다음 단계다.

### 검증과 남은 작업

`tools/validate_web_drc_notes.py`를 필수 배터리에 배선했다. private synthetic pack·valmini와
PATH-empty native runtime으로 기본 비활성/인증, forged reviewer/path/gid, check/local 범위,
미승인 무쓰기, 실제 주석 게시/Python gid 대조, 중복 승인·변경 본문 거부·늦은 취소,
외부 수정/lock 충돌,64KiB 주석과 금지 문자, clear/tombstone, legacy 별도 승인,
credential 모양 target 보호, 외부 pack 교체와 rebuild 뒤 receipt/old binding을 검증한다.
core의 추가 보호 gate는 등록 jobdeck 의존 파일과 그 캐시를 검사한다. owner unit은
준비 수명·body slot·build 배제·취소 flag/late commit·owner-bound receipt·wire와
미확정 게시/null 및 directory-sync 경고를 검사한다.

최종 `sh tools/validate_rust.sh`는 exit0·`RUST VALIDATION: ALL OK`다. workspace unit
(app11/core196/web49), 새 owner HTTP 게시 gate, 기존28회 native 게시/Python oracle,
jobdeck80·renderer46, KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 포함한다.
Rust1.89 core196/web49와 Linux x86-64 musl release static-pie 교차 빌드도 통과했다.
변경한 app/core/web의 fmt check와 strict all-target clippy도 통과했다. 기존 의존성
warning은 남아 있다. 검증용 임시 venv 링크만 제거하고 기존 venv 및 승인된 합성
shared-default 게시 결과물은 보존했다.
이는 Linux 실제 실행이나 실제 브라우저의 주석 게시 클릭을 검증한 것은 아니다.

브라우저 편집 패널·자동 저장, waive 쓰기와 reader 상태 갱신, 명시 import/export는 아직
남아 있다. 실제 브라우저의 주석 게시 클릭 수용도 미실시다. 기존 GTK launcher와
renderd0.12.87은 유지하며, M4 전체·현장 Firefox/NFS 수용 완료를 뜻하지 않는다.

## 26. M4e-3b — owner 선택 주석 편집·승인 패널

§25의 `--drc-reviewer TAG`를 명시한 세션에만 Notes 패널을 표시한다. reviewer와
출력 경로를 브라우저에서 고르지 않는다. Read selected notes → 편집 → Preview save →
별도 동의 → Approve note save 순서다. 읽기·미리보기·선택 이동·패널 복원은 무쓰기다.
파일 이름, 생성/교체 여부, 정규화된 실제 저장 텍스트(빈 문구는 선택 주석 지우기),
파싱 경고를 승인 전에 표시하고 legacy 미확인은 별도 체크가 필요하다.

### 선택·수명과 비용

- 여러 룰의 그룹 선택(최대5000개)이 클릭한 행보다 우선한다. 없으면 현재 행 하나다.
  대상 설명에 이 차이를 표시한다. 화면 global 번호 대신0-based check/local 문자열을
  보내며 u64를 JS Number로 바꾸지 않는다. refs 전체 복사는 명시적 Read 때만 한다.
- pan/zoom·viewport revision 변경은 snapshot을 다시 읽지 않는다. 선택 revision·
  DRC id/revision·view id·connection epoch 변경/연결 해제는 미승인 capability를 폐기하고
  새 저장을 막되 로컬 문구는 남긴다. 이미 승인된 작업의 선택을 바꾸지는 않는다.
- snapshot120초·preview30초 만료 시 새 승인을 막는다. 같은 선택의 Reload snapshot,
  keep text로 파일의 새 버전을 명시적으로 읽고 다시 준비한다. 다른 선택 편집은
  Discard 후 시작하며 문구를 다른 선택에 몰래 적용하지 않는다.
- 초안은 메모리에만 둔다. reload/pagehide/logout 때 지우며 자동 저장이 아니다.
  Ctrl/Cmd+Enter는 미리보기만, Escape는 초안만 버린다. IME 조합 중에는 실행하지 않는다.
  GTK 한글 조합기는 이관하지 않고 browser IME를 사용한다.
- 상태는 기존 DRC catalog를 재사용한다. 승인 작업 중500ms, 응답 유실 요청이 남으면
  2500ms로 결과만 조회한다. 큰 FE snapshot을 배경에서 반복 읽지 않는다. 서버의
  sidecar hash·전체 재작성 I/O 비용과 autosave 성능 검증은 여전히 남는다.

### 결과·복구

원래 승인 본문과 session id만 sessionStorage에 보관하고 주석 문구는 저장하지 않는다.
reload/Refresh는 POST를 재전송하지 않는다. Resolve approved request만 원래 승인한
동일 seq/body를 보내 receipt를 확인한다. 다른 세션/손상된 기록은 새 승인을 막고,
사용자가 파일을 확인했다는 별도 확인 뒤 로컬 기록만 지울 수 있다. durable 서버
ledger나 자동 재시작 복구는 아니다.

내 요청의 수락 응답과 `published:true`를 모두 확인해야 로컬 문구를 지운다. 다른 탭의
같은 seq receipt만으로 내 문구가 저장됐다고 판단하지 않는다. 실패·commit 전 취소·
결과 불명에서는 문구를 남긴다. `published:null`은 미확정이고, commit 뒤 취소는 undo가
아니다. directory sync 실패는 저장된 파일의 내구성 경고다. latest receipt는 세션 전체
결과라는 제목이며, 승인 후 선택 이동이 이미 승인한 내용을 바꾸거나 취소하지 않는다.

### 검증과 남은 작업

`drc-notes.test.cjs`가 동의/legacy/clear, UTF-8/u64 경계, 만료/연결 변경, 실패·취소·unknown
문구 보존, 다른 탭 충돌, 동일 승인만 재전송, 저장소 실패, 지연 응답 폐기/정리를 검사한다.
`drc-notes-panel.test.cjs`는 실제 DRC/selection/notes 모듈의 룰 간 선택과0-based refs,
pan 무조회, 선택/epoch/DRC 변경 무효화를 묶어 검사한다. 두 gate와 ES2017 파싱·전체 UI
회귀를 `tools/validate_web_ui.cjs`와 필수 Rust 배터리에 배선했다. 자산 HTTP gate와
bundle identity에 새 JS 및 native review API 소스를 포함했다.

집중 UI gate, web/app strict all-target clippy, Rust1.89 web49 unit+transport10,
Linux x86-64 musl release static-pie 교차 빌드가 통과했다.
최종 `sh tools/validate_rust.sh` exit0·`RUST VALIDATION: ALL OK`: workspace unit
(app11/core196/web49), owner 주석 HTTP gate, 기존28회 native 게시/Python oracle,
jobdeck80·renderer46, KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 포함한다.
초기 실제 브라우저 시도는 탭 생성 전 승인 서비스 용량 오류로 차단됐다. 후속 §27 작업 중
정상 승인된 Chrome 세션에서 합성 DRC 오류 선택→읽기→한글/여러 줄/`<script>` 텍스트
미리보기, 미동의 저장 버튼 비활성,30초 만료 뒤 문구 보존, snapshot 재조회,
선택 변경 시 저장 차단과 문구 보존, Discard 및 End session 정상 종료를 확인했다.
스크린샷으로 편집 패널의 줄바꿈/배치를 확인했다. 게시·clipboard 버튼은 누르지 않았다.
서버 exit0, 입력 OASIS·DRC·cache SHA-256 불변 및 주석/lock 미생성을 확인했다.
앞서 별도 승인받은 shared-default 합성 게시 결과물은 보존했다.

주석 badge/overlay·명시 import/export, waive 쓰기와 reader 갱신, 자동 저장 정책/성능,
실제 브라우저 게시와 현장 Firefox/IME/NFS 수용은 남아 있다. native API·worker·
raster 변경이 아니므로 renderd0.12.87과 GTK 기본 경로를 유지한다. M4 전체 완료나
Linux 실제 실행 검증을 뜻하지 않는다.

## 27. M4e-4a — geometry 캐시를 보존하는 waive 조회 갱신

waive writer를 연결하기 전, 저장으로 파일 inode가 바뀌었을 때 이미 열린 reader를
갱신하는 경로를 추가했다. 기존 `attach_waives`는 이전 입력의 `unchanged()`를 먼저
확인하므로 원자 교체 후 그 reader에 다시 적용할 수 없었다. 새 경로는 **같은 geometry
pack**을 별도로 확인하고, 검증한 waive 상태만 교체한다. 일반 조회의 기존 변경 검사는
완화하지 않는다. 이 단계에서 새 HTTP 쓰기 endpoint·waive UI·자동 저장을 켜지는 않는다.

### native 계약

- `store::Snapshot::apply_waives`가 expected snapshot을 한 번 소비한다. Notes/ASCII,
  다른 pack inode/변경된 pack, 취소, snapshot 이후 sidecar 생성/변경/교체/삭제를 거부한다.
  legacy header가 같은 다른 pack도 허용하지 않는다. 새 파일/lock/임시 산출물은 없다.
- snapshot의 captured descriptor를 복제하고 전후 expected revision·security·digest를
  검사한다. 검사 뒤 경로를 새로 열어 다른 파일을 몰래 채택하지 않는다. 마지막 pack/file
  metadata·취소 검사까지 성공해야 status 입력과 counter 벡터를 함께 교체한다.
- counter는 snapshot의 실제 status 스트림 recount 결과를 사용한다. legacy 파일의
  낡은 counter를 UI에 다시 노출하지 않으며 status2..255는 그대로 보존한다. 읽기 갱신이
  legacy 파일을 재작성하거나 해당 run의 소유권을 승인한 것으로 보지는 않는다.
- 명시적으로 읽은 snapshot이 absent이면 pack embedded status로 전환한다. 기존
  snapshot 이후 파일이 사라지면 오류이며, 자동 fallback/sidecar 삭제 기능이 아니다.
- geometry block LRU·qbox·check/global identity는 그대로 둔다. 오류 수만큼의 status
  벡터는 만들지 않고, 추가 상주 데이터는 rule당 u32 counter와 열린 descriptor다.
  snapshot 검증의 **전체 waive digest I/O는 여전히 O(파일 크기)**다. 이관 후 모든 저장이
  빠르다거나 per-click autosave에 적합하다는 성능 판정은 아니다.

`managed::Snapshot::apply_waives`는 적용 완료까지 admission·pack/source lease를 유지한다.
caller 취소와 store retire를 적용 전에 검사한다. retire/cancel은 best-effort이며 이미
반영한 읽기 상태를 undo하는 작업이 아니다. disk 게시 outcome과 reader 적용 결과는
다른 값이다. reader 적용 실패를 이미 성공한 파일 게시의 실패/롤백으로 보고하면 안 된다.

### actor와 후속 owner 연결

`drc::Service::apply_waives`는 이 opaque managed snapshot만 받아 기존 유계 reader queue에
넣는다. HTTP `Request`로 snapshot·경로를 만들 수 없다. 해당 명령만 오래된 waive 입력
precheck를 우회하며, 내부 snapshot 검증은 그대로 수행한다. 뒤에 큐잉된 조회는 새
status/counter를 보고 catalog의 `metadata.waives`도 갱신된다. 실패 시 partial install은 없다.
snapshot·lease는 큐/실행/취소 수명에 묶이고 response subscriber가 추가 복제를 만들지 않는다.

이는 **native actor 순서 보장**이지 브라우저의 revision 장벽 전체가 아니다. 현재 pack
id/revision을 바꾸지 않는다. 다음 owner 단계에서 status 변경 전 HTTP 결과·waived filter
cursor·준비된 focus/selection 응답을 fence하고 review revision/receipt에 적용 결과를
연결해야 한다. 파일은 저장됐지만 읽기 갱신이 실패한 경우와 응답 유실 후 재조회도 별도로
표시해야 한다. 이 연결 전에는 일반 웹 실행에서 새 적용 명령을 호출하지 않는다.

### 검증

새 core unit5개는 두 차례 native 게시 뒤 상태/카운터/필터, geometry/좌표 불변과
decoded block 수 불변, legacy recount·reserved bytes, wrong kind/pack/ASCII,
생성/교체/삭제·적용 직전 취소/변조, absent 명시 복원, managed lease/retire를 고정한다.
실제 actor 테스트는 native 저장→갱신→뒤에 큐잉한 조회의 새 상태, 동일 geometry
id/revision, 외부 writer로 stale snapshot 거부/새 snapshot 복구, wire 위조 거부,
종료·예약 반환과 원본 pack bytes 불변을 검사한다. `validate_web_drc.py`가 그 테스트의
성공 마커를 필수로 단언하므로 기본 cargo의 ignored 상태만으로 통과시키지 않는다.

core201·web49 unit, 실제 합성 actor 검사, app/core/web strict all-target clippy,
Rust1.89 core201/web49 및 Linux x86-64 musl release static-pie 교차 빌드가 통과했다.
최종 `sh tools/validate_rust.sh` exit0·`RUST VALIDATION: ALL OK`: workspace unit
(app11/core201/web49), 새 native actor 갱신을 포함한 pack/ASCII 웹 DRC gate,
owner notes gate·28회 managed 게시/Python oracle, jobdeck80·renderer46,
KLayout13 PX+2 phase-exact+14 style(jobs1/8)이 통과했다. 검증용 venv 링크만 제거했다.
native DRC 경로만 바꾸므로 renderd0.12.87과 GTK 기본
실행 경로는 유지한다. owner waive 쓰기/API/UI·전체 M4·현장 Firefox/NFS 수용 완료는 아니다.

## 28. M4e-4b — waive 갱신의 조회 revision 장벽

§27의 geometry 보존 갱신을 HTTP/WS에 연결하기 위한 선행 단계다. **새 waive 쓰기
endpoint·자동 저장·임의 파일 재부착 권한은 추가하지 않는다.** `--drc-reviewer`는
여전히 주석 opt-in이고 `--drc-waives`는 읽기 등록이다. 이 둘을 waive 쓰기 권한으로
해석하지 않는다. native 내부 `apply_waives`를 호출하는 owner writer는 다음 단계다.

### identity와 전환

- 같은 reader/geometry의 `id`는 유지한다. catalog의 `revision`은 이제 **해당 조회
  상태**의 opaque64hex 토큰이다. waive apply가 유계 actor queue에 수용되면 새 토큰으로
  전환하고 `phase: updating`을 노출한다. queue 포화·닫힌 reader 등 admission 거부는
  토큰을 바꾸지 않는다. metadata/좌표/qbox/LRU를 새로 열거나 레이아웃을 렌더하지 않는다.
- 적용 중 새 조회는 기다리는 HTTP 요청을 쌓지 않고 `drc_context_changed`로 거부한다.
  coordinator는 apply ACK 또는 catalog의 `ready`를 확인한 뒤 새 revision으로 읽는다.
  적용이 취소·실패하거나 ACK가 유실돼도 이전 토큰은 되살리지 않는다. 새 토큰은 저장 성공
  증명이 아니라 오래된 결과를 폐기하기 위한 장벽이다. 실패 원인은 apply 응답에 남는다.
- query ticket은 제출 시 revision을 캡처한다. actor 실행 전후와 ticket 결과 소비 시점에
  확인해, 이미 계산됐지만 소비되지 않은 이전 status 응답도 거부한다. HTTP는 요청의
  revision을 시작/await 이후/메모리 상태 commit에서 확인하고 응답 header에도 그대로 싣는다.
- registry identity → read revision → panel/prepared/controller 순서의 짧은 메모리 락이다.
  파일 해시·decode·native I/O나 await 동안 revision 락을 잡지 않는다. 기존 bounded actor와
  작업별 취소/lease 수명은 유지한다. geometry 인덱스 hot reload 정책과는 별개다.

### UI·선택·준비된 focus

- 같은 DRC id라도 revision이 달라지면 기존 panel/그룹/필터 커서를 다시 쓴다고 보지 않는다.
  서버는 새 revision의 첫 panel/selection 접근에서 저장 상태를 비우며, 오래된 요청이 이를
  다시 채울 수 없다. idle catalog poll은 revision이 같으면 상태와 진행 중 geometry 읽기를
  보존한다. 기존 layout의 viewport·레이어 isolation/복원 snapshot·geometry는 건드리지 않는다.
- 준비된 DRC focus에는 원 revision fence가 붙는다. HTTP 응답 뒤에 갱신이 발생해도
  WebSocket `apply`가 fence를 확인한 상태로 controller CAS를 수행하므로 낡은 focus는
  뷰를 이동시키지 못한다. 일반 layer settings의 prepared edit는 기존 경로를 유지한다.
- 브라우저 task는 요청 당시 revision을 고정한다. 새 catalog를 채택하면 옛 결과·focus를
  취소/무시하고 `ready`에서 새 목록을 읽는다. 이미 클라이언트에 도착한 표시를 서버가
  소급 취소하는 것은 아니다. 다른 탭의 변경을 아는 시점은 catalog poll이며, 다음 writer UI는
  자기 저장을 시작할 때 조회를 일시 중지하고 receipt/새 catalog를 연결해야 한다.
- 주석 snapshot/preview/승인도 기존 context.revision 검증을 이 장벽에 연결한다. 이전
  context의 새로운 승인은 거부하되 이미 수용된 동일 operation의 receipt 조회/replay는
  별도 기존 ledger 계약을 따른다. 디스크 게시 성공을 조회 갱신 실패로 뒤집으면 안 된다.

### 검증 및 남은 연결

web unit52·transport10·전체 ES2017/JS gate와 strict app/core/web clippy가 통과했다.
revision 변경/취소/닫힘·짧은 commit 락·동일 reader의 stale callback·panel 초기화와 idle
유지를 unit으로 고정했다. 실제 native actor gate는 게시를 반복하며 이전 ticket 거부,
새 status/counter·동일 geometry를 검사한다. 실제 owner HTTP/WS gate는 합성 pack에만
native waive를 저장한 뒤 이전 filter cursor/panel/selection409, stale focus 무이동,
새 status/focus 성공·pack bytes 불변을 검사한다. `validate_owner_service.py`에
`RUST DRC WAIVE REVISION: ALL OK`를 필수 단언해 ignored 기본 실행으로 대체할 수 없다.
UI gate는 같은 id·새 revision과 `updating→ready`, 지연된 geometry/focus 폐기·새 조회 복귀,
stale panel 자동 저장 없음·layout 무편집을 검사한다. 실제 browser waive 게시 수용은 아니다.

Rust1.89 core201/web52와 Linux x86-64 musl release 교차 빌드가 통과했다.
전체 `sh tools/validate_rust.sh`는 exit0·`RUST VALIDATION: ALL OK`로 완료됐다.
workspace unit·owner10(새 HTTP/WS revision gate 포함)·pack/ASCII 웹 DRC·주석/API·
28회 managed 게시/Python oracle·jobdeck80·renderer46·KLayout13 PX+2 phase-exact+
14 style(jobs1/8)이 통과했다. 검증용 venv 링크만 제거했으며 원래 venv와 승인된
shared-default 합성 게시 결과물은 보존했다. Linux 실제 실행·현장 수용을 뜻하지 않는다.
owner waive의 명시 opt-in·read/prepare/승인/receipt·UI와 전체 M4는 남아 있다.
다음 단계는 `published`와 `reader_applied`를
분리하고 외부 교체/취소/응답 유실 시 이를 정확히 보존하는 owner 연결이다.
renderd0.12.87·GTK 기본 실행·현장 보류 범위는 바꾸지 않는다.

## 29. M4e-4c — owner waive 승인 게시와 동일 reader 반영

§25의 owner 승인 모델을 waive에도 연결했다. **`--drc-edit-waives`를 추가로 명시해야**
활성화되며 `--drc-reviewer TAG`와 `--drc`가 필요하다. 기존 reviewer 옵션은 주석만,
`--drc-waives FILE`은 읽기만 허용한다. 기본 실행에서 새 쓰기 권한을 추론하지 않는다.
이 단계는 API/CLI이며 waive 편집 패널은 다음 단계다.

### 등록과 요청

- `/api/v1/drc/review/waives`의 GET 상태·POST 승인, `/read`, `/prepare`, `/revoke`,
  `/{seq}`, `/{seq}/cancel`이 주석과 같은 owner cookie/CSRF·Host/Origin·body admission을
  따른다. `capabilities.drc_waives`와 DRC catalog의 `waives`가 활성 여부를 표시한다.
- reviewer와 대상은 launcher가 고정한 pack 인접 sidecar다. native kind는 route의
  서버 설정으로 고정하고 JSON의 path/reviewer/kind/global ID는 거부한다. 선택은 기존
  opaque DRC/view/revision과0-based check/local 문자열로 검증·중복 제거하며 최대5000개다.
- 읽기는 선택 수·waived 수·reserved 수·파일 이름·legacy 여부를 반환한다. 준비는
  `waived: true|false`만 받으며 선택 수·실제 변경 수·reserved 수·생성/교체 여부를 보여준다.
  임의 status byte는 입력할 수 없다. 선택하지 않은 reserved status는 그대로 보존한다.
  선택한 reserved status를0/1로 바꾸는 경우에도 준비 결과에 그 수를 표시한다.
- Read/Prepare/Revoke는 target·lock을 만들지 않는다. snapshot120초·preview30초,
  별도 `approve:true`와 legacy 확인, 동일 owner/seq/body만 receipt replay하는 계약은
  주석과 같다. notes/waives ledger는 각각 유계이며 autosave는 둘 다 false다.
- write opt-in으로 ICE를 다시 열 때만 해당 고정 reviewer 파일이 이미 있으면 읽기에
  연결한다. 다른 reviewer나 인접 ICE를 탐색하지 않는다. 명시한 read sidecar가 고정
  write target과 다르면 시작을 거부한다. 소스·캐시·pack·rules·세션/credential 보호는
  유지하고 자기 write target만 별도 읽기 입력 보호 목록에 중복 추가하지 않는다.
  CLI의8바이트 pack header 확인은 시작 전 동기 I/O이며 NFS open 지연을 없애지는 않는다.

### 게시와 조회 결과는 별개

waive worker가 승인 context를 확인한 뒤 **디스크 게시 전에** 조회 revision을 전환한다.
파일 게시·새 snapshot 검증·actor 반영까지 `updating`을 유지하므로 이전 조회 결과나
prepared focus가 이 구간에 commit될 수 없다. 실패·취소도 이전 revision을 되살리지 않는다.
geometry id/LRU는 그대로이며 status/counter만 교체한다. notes와 waives의 준비/게시
어느 쪽이든 진행 중이면 pack rebuild를 거부하고, 종료 시 두 writer 모두 취소/join한다.

native waive 게시 결과에는 staged inode·전체 digest·pack binding의 **비직렬화 proof**가
붙는다. 게시 뒤 경로에서 읽은 snapshot이 바로 그 게시 파일인지 검증한 다음 §27 경로로
적용한다. 같은 bytes의 다른 inode나 외부 수정, 다른 target/kind는 거부한다. path만
다시 열어 외부 writer의 파일을 자기 게시 결과로 채택하지 않는다. staged 파일에 대한
추가 해시는 고정64KiB 메모리이지만 **O(sidecar bytes)의 추가 읽기**다. 기존 전체 digest/
재작성 비용도 남으므로 대형 파일 per-click autosave 성능을 보장하는 변경이 아니다.
SHA-1은 기존 변경 감지용이며 인증 서명은 아니다.

| 결과 필드 | 의미 |
|---|---|
| `published: true/false/null` | 디스크 commit 성공/미게시/결과 불명 |
| `directory_synced: false` | 게시된 파일의 내구성 경고. 재게시 지시가 아님 |
| `reader_applied: true/false/null` | reader 반영 성공/미반영/ACK 결과 불명 |
| `reader_error`, `reader_revision` | 조회 반영 문제와 새 조회 토큰. 저장 성공 증명이 아님 |

게시 성공 뒤에는 중간 `refreshing_reader`를 노출한다. reader 검증 실패·취소·응답 유실이
이미 성공한 디스크 게시를 `published:false`로 바꾸지 않는다. 전용 owner thread가
actor ACK를 최대30초 기다리며 기한 초과/채널 유실은 `reader_applied:null`이다. ticket
drop은 best-effort 취소이며 이미 반영된 상태를 undo하지 않는다. HTTP 연결 단절도
승인 작업을 자동 재제출하거나 결과를 성공으로 추정하지 않는다.

자동 갱신은 **이 owner가 승인한 게시**에 한정된다. 외부 파일 교체 뒤 원래 inode를
되돌려도 ctime이 달라져 일반 조회는 계속 거부될 수 있다. 명시적으로 다시 열어야 하며,
외부 변경 hot reload나 서버/클라이언트 인덱스 교체 정책을 이 단계에서 추가하지 않는다.

### 검증과 남은 작업

`tools/validate_web_drc_waives.py`를 필수 배터리에 추가했다. PATH-empty release의
합성 owner HTTP로 기본 비활성·별도 opt-in·위조 필드·check/local·중복 refs·미승인 무쓰기,
실제 승인/clear와 Python status oracle, 같은 reader의 새 revision·이전 조회 거부,
동일 승인 receipt replay·변경 본문 거부, lock 충돌·legacy 별도 승인·reserved 보존,
snapshot 뒤 외부 교체의 무덮어쓰기·명시 reopen 복구·고정 target 자동 읽기를 검사한다.
원본 OASIS/DRC/pack/cache의 bytes·mtime·ctime 불변도 단언한다. core unit은 게시 proof의
동일 파일/다른 inode·변조·다른 reviewer/kind를, web unit은 kind 고정과 reader 실패/ACK
불명이 디스크 성공·directory sync 경고를 바꾸지 않음을 검사한다. worker 예외에서도
waive 전용 `reader_applied`가 주석 응답에 섞이지 않아 기존 strict UI 스키마를 보존한다.

집중 HTTP gate, app11/core202/web54 unit, fmt·strict app/core/web all-target clippy,
Rust1.89 core202/web54와 Linux x86-64 musl release 교차 빌드가 통과했다.
최종 소스의 전체 `sh tools/validate_rust.sh`는 exit0·`RUST VALIDATION: ALL OK`다.
workspace unit(app11/core202/web54), owner HTTP/WS·notes/waives 승인 API·전체 ES2017/JS,
native 관리형 게시/Python oracle, jobdeck80·renderer46,
KLayout13 PX+2 phase-exact+14 style(jobs1/8)이 통과했다. 검증용 venv 링크만 정리했다.
waive UI·실제 브라우저 게시·현장 Firefox/NFS와 Linux 실행 수용은 아직 남아 있다.
승인된 shared-default 합성 게시 결과물은 보존한다. renderd0.12.87·GTK 기본 경로,
공유 endpoint 보류와 사용자 jobdeck 실측 브랜치는 바꾸지 않는다. 전체 M4 완료가 아니다.

## 30. M4e-4d — owner waive 편집·승인 패널

§29의 별도 `--drc-edit-waives` opt-in에만 Waives 패널을 표시한다. Read selected
statuses → Waive/Clear waive 선택 → Preview save → 별도 동의 → Approve waive save
순서다. 초기 동작은 미선택이며 선택/읽기/미리보기/pan/재접속은 저장하지 않는다.
reviewer·파일 경로·임의 status byte는 브라우저에서 고르지 않는다.

### 대상과 표시

- 주석과 같은 선택 공급자를 사용한다. 최대5000개의 룰 간 그룹 선택이 현재 행보다
  우선하며, 대상을 고정한 뒤0-based check/local 문자열을 보낸다. 표시 global 번호를
  wire ID로 변환하지 않는다. refs 복사는 명시적 읽기에만 수행한다.
- snapshot은 이미 waived인 수·reserved 수·파일 이름을, preview는 선택 수·실제 변경 수·
  생성/교체와 selected reserved 상태의0/1 교체를 표시한다. legacy 파일은 별도 확인이
  필요하며 미동의 저장 버튼은 비활성이다. 빈 선택이나 미선택 action은 준비할 수 없다.
- snapshot120초·preview30초, 선택/DRC/view/connection epoch 변경은 미승인 token을
  폐기한다. 다른 선택에 같은 동작을 몰래 적용하지 않으며, 같은 선택의 Reload snapshot은
  로컬 action만 유지한다. Ctrl/Cmd+Enter는 preview, Escape는 local discard다.
  UI 문구는 textContent로 출력한다. pan/zoom만으로 snapshot을 다시 읽지 않는다.

### 조회 중지와 복귀

이 탭의 승인 POST **전부터** DRC 조회·선택·윤곽·prepared focus를 비활성화한다.
레이아웃 viewport/레이어/렌더 상태는 바꾸지 않는다. 승인 중 도착한 이전 geometry 응답은
버리고, 이미 준비했던 주석의 승인은 막되 주석 초안 문구는 보존한다. 다른 탭의 작업은
기존 catalog/operation poll로 감지한 시점부터 같은 중지 규칙을 적용한다.

`published:true`만으로 복귀하지 않는다. terminal receipt, `reader_applied:true`, 같은
geometry의 **receipt reader_revision과 일치하는 ready catalog**를 확인해야 새 목록과
선택을 읽는다. 파일 저장은 성공했지만 reader 반영 실패/불명이면 두 결과를 나눠 보여주고
기존 상태를 계속 표시하지 않는다. 파일 확인 후 명시적으로 reopen해야 한다. 새 geometry
id의 catalog까지 옛 receipt로 막지는 않는다. 게시 전 실패/취소도 갱신된 조회 revision을
확인한 뒤 복귀하며, 상태 조회 실패/스키마 오류는 정상 catalog가 올 때까지 중지한다.

파일 게시·reader 적용·directory sync 경고는 각각 표시한다. `refreshing_reader`는
이미 저장됐지만 조회 반영을 기다리는 중이다. 늦은 취소는 저장/반영의 undo가 아니다.
active 작업은500ms, 유실된 승인 기록이 남으면2500ms로 가벼운 ledger만 조회한다.
terminal receipt마다 전체 catalog를 한 번 갱신하고, 수동 Refresh로 재조회할 수 있다.
대형 sidecar snapshot/geometry를 이 주기로 읽는 것은 아니다. §29의 O(파일 크기)
저장/검증 비용은 남으며 자동 저장 성능 수용을 주장하지 않는다.

### 응답 유실과 종료

원래 승인한 body와 session ID만 독립된 `floe-waive-pending` sessionStorage에 남긴다.
선택 refs·편집 action을 따로 보관하거나 reload 때 새 저장을 보내지 않는다. Resolve만
원래 seq/body를 재전송한다. 다른 탭의 같은 seq receipt는 내 선택이 저장됐다는 증거가
아니므로 내 승인 ACK와 context도 일치해야 local choice를 정리한다. 결과 불명·실패·
취소는 새 동작으로 자동 재시도하지 않는다. 손상/다른 세션 기록은 사용자가 파일을 확인한
뒤 로컬 기록만 지울 수 있다. 서버 durable ledger나 외부 파일 자동 재부착은 아니다.
disconnect/pagehide/end 시 진행 중 읽기와 타이머를 정리하고 미승인 초안을 지운다.
재접속 시 receipt를 먼저 확인하며, 종료된 화면에 저장 진행 중 안내를 남기지 않는다.

### 검증

`drc-waives.test.cjs`는 선택·action/동의·legacy/reserved·만료·epoch·고정 refs/u64,
파일/reader의 성공·실패·불명·취소·directory sync, old/new catalog 복귀, stale schema,
동일 승인만 복구·다른 탭 충돌·storage 실패·타이머 정리를 검사한다. 실제 DRC/notes/group
모듈을 사용하는 `drc-waives-panel.test.cjs`는 룰 간 대상, pan 무조회, 승인 시 이전 geometry
취소, 늦은 callback 무시, 같은 revision에서만 재개, 주석 보존과 layout 무편집을 검사한다.
두 gate를 ES2017/전체 UI와 필수 Rust 배터리에 배선하고 새 JS를 bundle identity와
native HTTP asset gate에 포함했다. Chrome 실제 DOM 검사에서 발견한 option 닫기 태그
오타를 수정하고 placeholder/Waive/Clear 세 option의 HTML 구조 단언을 추가했다.

전체 `sh tools/validate_rust.sh`는 exit0·`RUST VALIDATION: ALL OK`다. workspace
unit(app11/core202/web54), owner HTTP/WS·notes/waives API, 전체 ES2017/JS,
native 관리형 게시/Python oracle, jobdeck80·renderer46, KLayout13 PX+2 phase-exact+
14 style(jobs1/8)이 통과했다. 배터리 시작 뒤 수정한 HTML option 오타는 최종 소스로
전체 JS, web54/transport10, app/web strict clippy, release 빌드, Rust1.89 web54/
transport10과 Linux x86-64 musl release 교차 빌드를 다시 통과했다.
`cargo fmt -p floe-web -p floe-app -- --check` 범위도 통과했다. 전체 workspace
fmt 검사는 이번에 변경하지 않은 CLI/VFS 등의 기존 편차로 실패하므로 전체 fmt clean을
주장하지 않는다. 관련 없는 포맷 일괄 변경은 하지 않았다.

Chrome의 최종 bundle에서 합성 입력만 사용해 Error1 선택→Read selected statuses→
Waive 미리보기(선택1·변경1), Discard→Clear waive 미리보기(선택1·변경0)를 실제
조작했다. 두 경우 동의 전 Approve 버튼은 비활성이며, Error2로 변경하면 미리보기가
사라지고 승인할 수 없다. 화면 캡처로 패널·레이아웃 표시도 확인했다. 승인 버튼은 누르지
않았다. End session 뒤 프로세스 exit0, session 파일 제거, 입력3개·인덱스4개의 SHA256
불변과 note/waive sidecar·lock 미생성을 확인했고 합성 입력과 로그는 보존했다.
검증용 venv 심볼릭 링크만 정리했다.

실제 브라우저 waive 게시·현장 Firefox/NFS·Linux 실제 실행 수용은 별도다. 새 API나
공유 권한은 추가하지 않았으며 GTK 기본·renderd0.12.87·기존 승인된 shared-default
합성 게시 결과물은 유지한다. 전체 M4 완료를 뜻하지 않는다.

## 31. M4e-5a — 저장 주석 표시용 읽기 API

GTK의 오류 목록 `*` 표시와 마지막으로 이동한 오류의 좌상단 주석 패널에 필요한
서버 읽기 경로다. 편집용 `/notes/read`를 목록 표시 때 호출하면 미승인 snapshot/preview가
폐기되고 파일 전체를 다시 파싱하므로, 표시 조회와 편집 준비를 분리했다. **이 절은
API/캐시 단계이며 웹 badge·본문 overlay UI는 아직 연결하지 않았다.**

### 요청·응답과 권한

`POST /api/v1/drc/review/notes/display`는 기존 `--drc-reviewer TAG`의 owner 등록만
사용한다. 일반 read-only 등록에서는 비활성이고 guest/새 파일 경로/추가 reviewer를
허용하지 않는다. cookie+CSRF·Host/Origin·기존 large-body admission을 그대로 적용한다.

- 입력: 기존 `{drc_id, revision, view_id}` context, `errors:[{check,error}]` 최대512개,
  선택적인 `focus:{check,error}` 하나. 인덱스는 canonical u64 문자열의0-based 값이며
  표시 global 번호나 임의 gid를 받지 않는다. errors가 비어 있어도 focus 하나는 허용하지만
  둘 다 없으면 오류다. 입력 순서·중복을 보존하며,5000개 편집 선택 전체를 보내는 API가 아니다.
- 응답: `kind:drc_note_display`, 요청 context, 문자열 `review_rev`, 고정 reviewer·파일
  **basename**, `rows:[{check,error,noted}]`, `focus:{check,error,text}` 또는 null.
  본문은 선택된 한 오류의 전체 주석(최대64KiB) 또는 null이다. `exists`,
  `legacy_unverified`, `import_report`도 함께 반환하므로 파싱 손실·미확인 legacy를
  숨기면 안 된다. `cache_hit`은 서버 snapshot 재사용 여부다. 편집/게시 토큰은 발급하지 않는다.
- 각 check/local은 기존 DRC actor가 opaque pack identity와 함께 검증한다. 요청 시작과
  응답 전의 registry·view·조회 revision을 재검사한다. 도중 waive 갱신/pack rebuild/close가
  있으면 이전 결과를 채택하지 않는다. 진행 중 저장이나 마지막 저장 결과 불명은 표시 조회를
  거부한다. 표시 읽기가 저장 결과 불명을 해소하거나 새 저장을 자동 승인하지 않는다.

### 캐시·비용·수명

- review service당 immutable note snapshot 하나다. key는 전체 context와 notes
  `review_rev`이며 viewport pan/zoom state_rev는 넣지 않는다. 첫 조회는 기존 managed
  store로 pack metadata와 note sidecar를 읽는다. warm 조회는 note map lookup과
  bounded check/local 매핑만 수행하고 sidecar hash/파싱을 다시 하지 않는다.
- cache도 기존 admission/pack read lease를 유지한다: CPU1·256MiB 예약이며 실제 RSS의
  하드 상한은 아니다. 이미 편집 preview가 있으면 독립 표시 cache와 함께 최대CPU2·512MiB를
  예약할 수 있다(DRC reader·waive store 등과 별도). 자원 부족은 명시 오류다. 캐시가 공짜거나
  전체 프로세스 메모리가512MiB 이내라는 뜻은 아니다.
- 명시 edit read 시작은 display cache를 먼저 내려 편집 admission을 우선한다. 승인 submit,
  pack rebuild, session stop도 cache를 해제한다. context/revision 변경은 다음 표시 조회에서
  교체한다. 다른 표시 조회 때문에 기존 edit snapshot/preview를 폐기하지 않는다.
- 표시 읽기는 편집 준비와 같은 try-only 슬롯 하나를 사용한다. 실행 중인 읽기의 취소/timeout은
  native 작업이 실제로 풀릴 때까지 body/CPU/lease를 유지하며 rebuild·submit과 겹치지 않는다.
  큰 파일 I/O·파싱은 HTTP reactor 밖에서 수행한다. 표시 캐시는 세션 메모리이며 durable
  파일이나 다른 사용자 데이터베이스를 만들지 않는다.
- warm 조회 전후에는 directory/pack·열린 파일과 경로의 inode·size·mtime/ctime·소유권/
  mode·link count·보안 attribute가 캡처와 같은지 확인한다. 변경·삭제·대체·처음 없던 파일
  생성은 명시 오류이며 새 외부 파일을 자동 파싱하지 않는다. 이는 표시 snapshot의 저비용
  guard이지, 전체 digest를 검사하는 게시 CAS나 보안 서명을 대체하지 않는다. 명시적 새
  edit read/reopen 전까지 해당 외부 변경을 자동 채택하지 않는 정책을 유지한다.

### 검증과 다음 연결

core 테스트는 없음→생성·동일 bytes 쓰기·다른 inode 교체·삭제·취소·종류 불일치를 검사한다.
web 단위는 표시 읽기 중 build/edit admission 거부·종료 시 native unwind까지 permit 유지와
결과 불명·클라이언트 path/reviewer/gid/승인 필드 거부를 검사한다.
기존 필수 `validate_web_drc_notes.py`에 실제 native HTTP 검증을 추가했다. 빈 PATH에서
512개 목록+focus, 순서·본문/배지·64KiB escaping, warm cache hit, edit snapshot와 preview
보존, 저장/clear 후 revision·cache 교체, 외부 변경 거부, stale context/pack binding,
cache와 draft를 함께 보유한 pack rebuild, 미등록/ASCII/보호된 session-file 거부를 검사한다.
합성 OASIS·DB·pack·index의 bytes/metadata 불변과 서버 종료/임시파일 정리도 유지한다.

전체 `sh tools/validate_rust.sh`는 exit0·`RUST VALIDATION: ALL OK`다. core203/web56,
기존 owner HTTP/WS·notes/waives·전체 ES2017/JS, jobdeck80·renderer46,
KLayout13 PX+2 phase-exact+14 style(jobs1/8)이 통과했다. 마지막 HTTP body 한도 수정은
최종 release로 실제 주석 HTTP gate와 web56/transport10을 재실행했다. 일반16KiB를
넘는 유효32KiB JSON은 성공하고1MiB 초과는 거부하며, 룰 횡단 역순·중복 목록도
Python oracle과 대조했다. app/core/web scoped fmt·strict all-target clippy가 통과했다.
Rust1.89 core203 및 최종 web56/transport10, Linux x86-64 musl release 교차 빌드도
통과했다(실제 Linux 실행 수용은 아님). 검증용 venv 링크만 정리하고 로그는 보존했다.

다음 연결은 읽기 결과를 사용한
목록 badge와 **선택 행이 아닌 마지막으로 승인된 오류 이동**의 주석 overlay다. pan 무조회,
overlay all/focus/none, 주석 저장 revision·늦은 응답·재접속, 전체 본문/긴 주석 잘림 표시,
표시 PNG 캡처와 상태 복원까지 함께 검증한다. 실제 브라우저 주석/waive 게시 승인과
현장 Firefox/NFS/ETX 수용은 별도이며 GTK 기본과 renderd0.12.87은 유지한다.

## 32. M4e-5b — 저장 주석 배지·이동 대상 overlay

§31의 표시 읽기 API를 웹 오류 목록과 캔버스에 연결했다. 편집 snapshot/preview는
목록 조회에 쓰지 않는다. 주석 게시·waive 승인·공유 기본값의 권한은 확장하지 않았다.

### 표시·선택 계약

- 현재 유계 오류 페이지의 주석 유무를 `*` 접두부와 tooltip으로 표시한다. 응답 실패는
  `Saved notes unavailable`과 조회 오류로 표시하며 **주석 없음으로 해석하지 않는다**.
  legacy binding 미확인·import 손실 카운터도 패널에서 명시한다.
- 본문은 현재 선택 행이 아니라 **성공 ACK를 받은 마지막 오류 이동**의 대상이다.
  준비된 focus가 실패하거나 아직 ACK되지 않았으면 바꾸지 않는다. 캔버스 단일 클릭으로
  다른 오류를 선택해도 마지막 이동의 주석은 유지한다. 기존 jump mode에서 실제 이동하는
  목록/step 조작은 ACK 후 갱신된다.
- Markers 체크와 독립적이며 overlay `all`/`focus`에서 보이고 `none`에서만 숨긴다.
  `k`/`K`로 ruler만 지우는 동작은 주석을 지우지 않는다. Clear/실제 focus 종료·source/DRC
  변경은 대상을 해제한다. viewport 교차 여부로 본문을 숨기지 않는 GTK의 현재 구현을 따른다.
- 12 CSS px 본문·반투명 검정 배경을 기존 `drc-canvas` 좌상단에 그린다. DPR을 반영하며
  텍스트는 코드로 실행하지 않는다. viewport보다 긴 본문은 표시 prefix만 측정하고
  `note clipped; full text in panel`로 알린다. 패널의 접힌 **full saved note**에는 최대64KiB
  전문을 `textContent`로 제공한다. wrap 결과를 재사용하므로 pan마다 글자를 재측정하지 않는다.
  0폭/결합문자가 길게 반복돼도 측정 문자열이 무한히 자라지 않도록 한 줄128 codepoint에서
  강제로 줄바꿈한다. 본문 저장값은 바꾸지 않는다.
- 표시 PNG의 기존 flush/합성 경로가 같은 `drc-canvas`를 포함한다. 추가 canvas·서버 geometry
  재렌더·주석 본문 localStorage 저장은 없다. overlay none이면 캡처에도 주석이 없다.

### 요청·복원·경합

- 표시 identity는 DRC/view/read revision·connection epoch·notes `review_rev`·목록 refs·
  마지막 이동 target이다. viewport `state_rev`는 제외한다. 따라서 일반 pan/zoom·현재 행
  선택·catalog의 같은 상태 poll은 조회하지 않는다. In view 필터로 실제 목록이 바뀌면
  새 페이지 배지 조회는 필요하다. 화면당64행을 쓰며 API 자체 한도는512행이다.
- 요청은 최대1개 진행+최신 desired 상태만 보관한다. 같은 turn의 상태 변경을 합치고
  오래된 응답은 폐기한다. 실패 시 자동 반복하지 않고 `Refresh saved notes`로 재시도한다.
  서버가 거부하는 외부 sidecar 교체는 표시 Refresh만으로 채택되지 않는다. 기존 명시적
  Reload snapshot/reopen 경계를 유지한다.
- 편집 UI는 승인 제출 전·진행/결과 불명·상태 조회 실패·snapshot 준비 중임을 표시 모델에
  전달한다. 그동안 기존 배지/본문을 내리고 새 표시 읽기를 시작하지 않는다. 확인한 게시
  revision 이후에 새 조회를 한다. 명시 edit read는 외부 변경을 같은 review_rev로 다시
  읽을 수 있어 별도 read-turn으로 표시도 갱신한다. 표시 읽기는 편집 초안/preview를 지우지 않는다.
- 서버 panel 상태에 선택/CD와 독립된 선택적 `note_target:{check,error}`를 저장한다.
  canonical u64·pack 범위와 live jump 조건을 검증하며, 본문/경로는 저장하지 않는다.
  이전 상태에 필드가 없으면 대상 없음으로 복원한다. Reload review는 화면을 다시 이동하지
  않고 그 target으로 본문을 읽는다. 미접속·복원 중·waive reader 갱신 중에는 표시하지 않는다.

### 검증

전용 JS gate는 strict 응답 schema·역순/누락/잘못된 target·큰 u64·UTF-8 제한, 한 요청
직렬화·늦은 revision·오류/결과 불명·재접속·명시 read 갱신·pan 무조회·긴 주석의 측정 상한을
검사한다. 실제 DRC/notes/selection/navigation 모듈 통합 gate는 ACK 대상/선택 분리,
서버 panel 복원·Markers/overlay/k·snapshot canvas flush·편집 preview 보존·저장 revision
장벽·종료 정리를 확인한다. 기존 필수 ES2017 gate에 모두 배선했다.

native panel 테스트와 실제 HTTP gate는 독립 target round-trip과 invalid/범위 초과/body
확장을 거부한다. web57/transport10, scoped fmt와 app/core/web `clippy --no-deps --all-targets
-- -D warnings`, 전체 JS·DRC ASCII/ICE HTTP·주석 read/edit/display HTTP gate가 통과했다.
의존성까지 검사한 clippy는 변경하지 않은 oasis의 기존 lint13건 때문에 실패하므로 전체
workspace clippy green을 주장하지 않는다. Rust1.89 web57/transport10과 Linux x86-64 musl
release 교차 빌드도 통과했다(실제 Linux 실행 수용은 아님).

Chrome에서는 새 `/private/tmp/floe-note-ui-browser.rnYzbr`의 합성 OASIS/DRC pack과
사전 작성 주석으로 `*`·HTML처럼 보이는 문자/한글·ACK 후 좌상단 본문·Markers off·overlay
focus/none/all·Reload review 복원을 확인했다. End session은 exit0으로 끝났고 session JSON을
정리했다. 합성 source/DB/pack/주석 SHA256은 시작 전과 동일하다. 이 확인에서 주석/waive
게시나 clipboard 버튼은 누르지 않았다. shared-default 합성 게시의 별도 승인/결과는 §20 그대로다.

전체 배터리는 exit0·`RUST VALIDATION: ALL OK`다: app11/core203/web57,
jobdeck80·renderer46, KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 통과했다.
최초 실행은 기존 macOS legacy oracle의 fork pool 대기에서 중단했다. 미완성 합성 cache를
옆에 보존하고 동일 source를 `index --legacy --jobs 1`로4초에 생성한 뒤, 전체 배터리를
처음부터 재실행했다. 비교 geometry와 게이트를 생략하지 않았다. 마지막 결합문자 측정 상한은
최종 JS/ES2017·web57/transport10·주석 HTTP gate와 release/musl 빌드로 별도 재검증했다.
로그는 `/private/tmp/floe-note-ui-battery-retry.log` 및 `floe-note-ui-*-final.log`에 보존했다.
검증용 venv 링크만 제거하며 실제 venv·합성 게시 결과물·다른 worktree 변경은 보존한다.
주석 import/export·실제 브라우저 게시·현장 Firefox/NFS/ETX 수용은 남아 있고,
GTK 기본과 renderd0.12.87은 바꾸지 않는다.

## 33. M4e-6a — 주석·waive native 가져오기/내보내기

GTK의 `note_import`/`waive_import`는 선택 오류 편집이나 병합이 아니라 **전체 review
교체**다. 기존 notes import draft에 더해 두 종류의 snapshot export와 전체 waive import를
`app-core/drc/review/store/transfer.rs`에 구현하고 managed borrow에 연결했다.
이 단계에는 새 CLI/HTTP/upload/download/UI가 없다. 브라우저의 게시·다운로드 수용이나
전체 Python 이관 완료를 의미하지 않는다.

### 전송·메모리 계약

- `Snapshot::export`는 이미 읽고 검증한 snapshot만 직렬화한다. 현재 경로의 외부 파일을
  조용히 새 review로 채택하지 않는다. 출력은 caller가 소유한 **미게시 staging sink**이며
  경로를 열거나 파일·lock을 만들지 않는다. 오류 시 sink에 prefix가 남을 수 있으므로
  `Ok`와 caller의 최종 context/cancel 검증 전에는 다운로드/게시 대상으로 노출하지 않는다.
  flush/fsync/원자 게시·artifact TTL/다운로드 권한은 다음 연결의 책임이다.
- Notes는 기존 FE serializer와 native pack의 실제 error bbox 중심을 재사용한다.
  정규화 손실 카운터·그룹/멤버 수를 반환한다. 빈 주석도 유효한 fingerprinted tombstone을
  내보낸다. GTK의 빈 export는 목적지를 지우지만, native export는 삭제 권한을 추론하지 않는다.
- Waive는 64KiB positional read로 처리하고 cursor를 공유/변경하지 않는다. 입력의 reserved
  status 값(2/255 등)은 그대로 보존하고 `status == 1`의 룰별 counter를 다시 센다.
  존재하지 않는 sidecar는 원본 pack의 status를 읽어 export하며 sidecar를 생성하지 않는다.
- `prepare_waives_import(File)`은 trusted caller가 선택·허용한 regular descriptor만 받는다.
  expected header/정확한 길이/EOF를 확인하고 전체를 한 번 세어 preview를 만든다. approved
  publication에서 다시 스트리밍해 입력 digest와 metadata/security가 같은지 검증하며
  commit 직전에도 재검사한다. 전체 status Vec나 5,000개 선택 편집 목록을 만들지 않는다.
  비용은 입력 길이에 선형이며 재검사로 여러 번 읽는다. 메모리는 O(룰 수)+고정 I/O chunk다.
  큰 파일의 blocking I/O는 chunk 사이에서만 취소할 수 있고 즉시 선점/고정 지연을 보장하지 않는다.
- 경로 문자열 재조회가 아닌 열린 descriptor를 보유한다. upload가 연결될 때 caller는
  비공개 임시파일을 **capture 전에** unlink한 FD도 전달할 수 있다. capture 후 inode/link 수/
  내용/속성이 바뀌면 보수적으로 충돌 처리한다. 업로드 권한이나 경로 접근을 이 함수에서
  새로 허용하지 않는다.

### 확인·게시 경계

FE/waive의 size/mtime/count 헤더는 호환성 표식이지 고유 run의 인증이 아니다.
따라서 명시 import draft는 대상이 이미 native binding을 갖더라도 `legacy_unverified`로
표시하고 별도 `accept_legacy_run`/`publish(true)` 확인을 요구한다. 기존의 잘못된 **대상**
binding을 우회하는 옵션은 아니다. portable input의 속성/binding을 대상에 복사하지 않고,
기존 대상 권한을 보존하면서 승인된 pack binding을 새 inode에 기록한다.

기존 stable lock·expected revision·동일 디렉터리 stage·fsync/commit·waive reader proof를
공유한다. 취소/입력 변경/대상 교체/입출력 실패는 commit 전 기존 파일을 보존한다.
commit 이후 취소와 directory-sync 실패는 기존대로 게시 성공/내구성 경고이며 rollback으로
보고하지 않는다. noncooperating writer와의 완전한 filesystem CAS는 보장하지 않는다.
managed snapshot/draft/publication은 동일 CPU1/256MiB admission 및 pack/source read lease를
유지한다. export의 sink가 막혀 있어도 retire만으로 permit을 반환하지 않고 native unwind까지
보유한다. 새 무제한 worker/queue는 만들지 않았다.

### 검증·다음 단계

전송 회귀는 빈 export 무쓰기, 실제 주석 중심, 이미 bound인 대상의 import 확인,
131,075 status(선택/64KiB 경계 초과), reserved 값/빈 룰/잘못된 counter, header/길이/종류/
취소 거부, unlink된 FD와 cursor 독립성, preview 이후/scan 도중/commit 직전 변경,
대상 충돌, sink I/O 실패 및 managed retire/admission/reader proof를 검사한다.
`validate_drc_review.py`는 실제 Python `note_export`/`note_import`와 `waive_export`로
만든 두 synthetic pack을 PATH 없는 Rust에서 대조한다. 22 주석 상태·5,793 status bytes의
codec 비교, 전체 waive import6회를 포함한 native 게시34회·export28회가 통과했다.
기존 입력·Python review·transfer 파일의 bytes/mtime/ctime/mode/xattrs 불변과 허용한
native target/lock 외 출력·잔류 stage 없음도 단언한다.

전체 `sh tools/validate_rust.sh`는 exit0·`RUST VALIDATION: ALL OK`다. app11/core214/web57,
기존 owner HTTP/WS·notes/waives·전체 ES2017/JS, jobdeck80·renderer46,
KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 통과했다. 새 합성 source의 legacy oracle은
기존 macOS fork 대기를 피하기 위해 먼저 `index --legacy --jobs 1`로 생성했다.
비교 geometry와 gate를 생략하지 않았다. 마지막 전체 교체 단언은 core214 전체 테스트를
현재 Rust와1.89에서 재실행했다. app/core/web scoped fmt 및 vendored
`clippy --no-deps --all-targets -- -D warnings`도 통과했다. 변경하지 않은 의존성의 기존
경고를 전체 workspace clippy clean으로 표현하지 않는다.
Rust1.89 core214/web57/transport10과 Linux x86-64 musl release 교차 빌드도 통과했다.
이는 실제 Linux/Firefox 실행 수용이 아니다. 로그는
`/private/tmp/floe-review-transfer-battery.log`와 `floe-review-transfer-*-final.log`,
`floe-review-transfer-msrv.log`/`floe-review-transfer-musl.log`에 보존했다.
검증용 venv 링크만 제거하며 실제 venv·승인된 합성 shared-default 게시 결과·다른 worktree는 보존한다.

다음은 owner에 한정한 bounded upload/artifact lifetime, 전체 교체 preview·별도 승인,
context/revision·TTL·취소/재접속·다운로드와 UI 연결이다. 큰 waive를 JSON 배열로 올리거나
선택 편집 한도로 조용히 자르는 경로는 허용하지 않는다. 실제 브라우저 주석/waive 게시 및
현장 Firefox/NFS/ETX 수용은 여전히 별도다. GTK 기본·renderd0.12.87은 유지한다.

## 34. M4e-6b — owner 전체 review 전송 API

§33의 native 전송을 현재 owner 서비스에 연결했다. 이 단계는 **API와 합성 회귀**이며
웹 패널의 파일 선택·가져오기/내보내기 버튼은 아직 없다. 실제 브라우저 review 게시,
브라우저 다운로드와 현장 Firefox/ETX/NFS 수용을 완료로 간주하지 않는다.

### 범위·입력

기존 `--drc-reviewer TAG`의 notes와 `--drc-edit-waives`의 waive opt-in을 각각 따른다.
owner cookie·CSRF·origin 검사를 재사용하고, 등록한 ICE reader와 열린 view의
`{drc_id,revision,view_id}`를 고정한다. ASCII review, guest/share, 임의 서버 파일
업로드·경로/reviewer 변경은 허용하지 않는다. `/capabilities uploads:false`의 일반 파일
업로드 금지 계약은 그대로이며 새 권한이나 자동 게시를 추가하지 않는다.

다음 경로에서 `ROOT = /api/v1/drc/review/{notes|waives}`다.

| 경로 | 의미 |
|---|---|
| `GET ROOT/transfer` | 전송 ledger32개, 독립 artifact 목록, 현재 upload, 한도/사용량 |
| `POST ROOT/transfer` | `seq,context,action`으로 import 시작/export/prepare |
| `POST ROOT/transfer/chunk` | raw octet-stream을 현재 offset부터 추가 |
| `GET ROOT/transfer/{seq}` | accepted 작업의 확정/미확정 결과 확인 |
| `POST ROOT/transfer/{seq}/cancel` | 실행 중 작업 취소 요청, 이미 완료된 결과 보존 |
| `GET/DELETE ROOT/artifacts/{id}` | 준비한 다운로드 metadata/명시 폐기 |
| `GET/POST ROOT/artifacts/{id}/download` | bounded 첨부 파일 다운로드 |

import 시작에는 `bytes`(정규화한 십진 문자열), prepare에는 `token`만 추가한다.
export에는 둘 다 없다. JSON `chunk` action이나 알 수 없는 필드는 거부한다.
raw chunk는 `Content-Type: application/octet-stream` 및
`X-Floe-Transfer-{Token,Seq,Offset}`, `X-Floe-{DRC,Revision,View}` 고정 헤더를 사용한다.
전체 파일 JSON/base64/선택 오류 배열을 받지 않고, 서버가 파일명을 정한다.

### 수명·예산·재시도

- 기존 공유 large-body semaphore 하나/1MiB body 한도를 transfer와 chunk에도 적용한다.
  chunk의 body permit은 HTTP 응답이 아니라 native 작업의 실제 unwind까지 보유한다.
  파일 최대512MiB, notes 입력 최대16MiB, waive 입력은 등록 pack의 header/status/counter
  전체 길이와 정확히 같아야 한다. 종류별 notes/waives 각각2슬롯·최대1GiB 예약·동시
  다운로드1개다. clip 저장소와는 별도이며 서버 전체 disk/RSS 상한이라고 주장하지 않는다.
- import는 `O_EXCL`·0600 private stage를 생성하고 사용자 bytes를 쓰기 **전에**
  descriptor-relative unlink한다. 경로나 원래 클라이언트 파일명을 서버에 보존하지 않는다.
  snapshot/read lease와 최댓값 예약을 upload가 소유한다. 순서에 맞는 offset만 받아
  64KiB씩 기록하며 업로드 종료까지 한 메모리 버퍼로 합치지 않는다.
- upload TTL은 시작부터600초이며 chunk마다 연장하지 않는다. 완성된 prepare token은
  기존30초 정책이다. waive의 입력 FD/예약은 전체 교체 preview와 실제 게시 unwind까지
  살아 있다. notes는 bounded FE 파싱 후 FD/예약을 반환하고 기존 admitted draft를 유지한다.
- native 준비/export는 기존 종류별 review owner worker에서 실행한다. 별도 무제한
  worker/queue를 만들지 않는다. 큰 unlinked FD의 revoke/expiry/logout 폐기는 같은
  worker에 넘겨 HTTP reactor/registry mutex 밖에서 수행한다. blocking fsync/read/write를
  선점하거나 취소 지연의 절대 상한을 보장하지는 않는다.
- 전송 seq는 기존 게시 seq와 분리한다. 같은 owner/context/action/offset/길이/bytes의
  재시도만 기존 결과를 돌려주고, 다른 body는409다. SHA-1은 chunk 변경/재전송 검출용이며
  인증 수단이 아니다. ledger에는 raw body가 없다. HTTP ACK가 끊겨도 accepted 작업은
  계속되므로 poll/replay로 확인한다. prepare 후 소비된 upload의 이전 chunk 재시도도
  재기록하지 않는다. 오래된 이력은 expired이지 새 작업이 아니다.
- 미완성 upload·waive import preview가 남아 있으면 DRC pack 재빌드는 busy다.
  기존 `POST ROOT/revoke {token}`으로 폐기하고 native cleanup 후 재빌드한다.
  preview를 지운 즉시 파일/lease가 해제됐다고 간주하지 않는다.

### 준비와 실제 게시·다운로드

prepare는 `action:replace_all`, 대상 이름/기존 여부, 입력 크기, 전체 주석 그룹/멤버 수와
손실 카운터 또는 waived count를 반환한다. 빈 notes는 `clears:true`의 fingerprinted
tombstone이다. 현재 선택 오류만 교체하거나 조용히 병합하지 않는다.
명시 import는 portable run을 인증할 수 없으므로 이미 bound인 대상도
`legacy_unverified:true`다. 기존 `POST ROOT`에 `approve:true,confirm_legacy:true`를
별도로 제출해야 실제 파일이 바뀐다. 기존 revision/입력 충돌, stable lock, atomic commit,
waive reader 적용·ACK 분리 계약은 그대로다. 전송 worker 자체는 review를 게시하지 않는다.

export는 기존 편집 snapshot/preview를 소비하지 않는 읽기 작업이다. 현재 native snapshot을
private spool에 직렬화·fsync한 뒤 context/cancel을 다시 검사하고만 artifact를 노출한다.
실패·중간 파일은 다운로드할 수 없다. sidecar/lock이 없는 경우 새 review 파일을 만들지 않는다.
artifact는 owner/reader/view/review revision과600초 TTL에 고정된다. review 게시 성공 또는
결과 불명으로 revision이 바뀌면 이전 artifact를 즉시 revoke하고 off-reactor 회수한다.
재빌드/logout/명시 폐기도 같은 경로다. active reader가 남으면 실제 drop까지 회계가 유지된다.

다운로드는 기존 clip의64KiB reader/전송 credit/idle10초 경로를 공유한다. 매 chunk에서
owner/context/review revision/TTL을 확인한다. GET은 CSRF 헤더, 브라우저 form POST는
정확한 `csrf=<token>` body와 origin/cookie를 요구한다. CSRF query·Range는 거부하며
`no-store,nosniff,attachment`와 고정 서버 파일명을 사용한다. 만료/새 revision에서 과거
성공 ledger를 replay해도 파일을 재생성하거나 다시 다운로드 가능하게 만들지 않는다.
artifact 목록은32개 작업 이력과 독립적이다. 많은 upload chunk가 과거 export의 이력을
밀어내도 새로고침 후 파일 조회/폐기는600초 수명 안에서 가능하다.

### 검증

`validate_web_drc_transfer.py`를 전체 battery에 추가했다. private valmini/ICE만 사용하며
런타임 PATH는 비운다. 합성 FE 입력>1MiB와 정규화 다운로드>2MiB, 전체6007개 status
(선택 edit cap 초과), reserved2/255·counter 재계산을 Python oracle bytes와 비교한다.
opt-in/auth/알 수 없는 필드/크기·offset/동일·변조 replay/미완성 prepare, 승인 전 무쓰기,
전체 교체/빈 tombstone/별도 legacy 확인, 외부 target 충돌, 슬롯 포화/폐기, GET/form
다운로드·Range/CSRF 거부, 이전 review artifact 폐기, 취소/logout·입력 fingerprint
불변·named stage 잔류 없음이 gate다. 순수 Rust는 private spool의 unlink-before-write·0600,
wire 경계값·body 없는 replay signature, 취소 후 실제 unwind까지 admission을 검증한다.

전체 `sh tools/validate_rust.sh`는 exit0·`RUST VALIDATION: ALL OK`로 완료했다.
app11/core215/web60과 기존 HTTP/WS·ES2017 UI, jobdeck80·renderer46,
KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 통과했다. 새 private valmini의 legacy
oracle은 macOS의 기존 fork 대기를 피하도록 먼저 `--legacy --jobs 1`로 생성했다.
최종 artifact 목록/이력 분리 보강은 release 재빌드 후 HTTP gate를 재실행했고,
Rust1.89 core215/web60/transport10, scoped fmt·vendored clippy `--no-deps --all-targets
-- -D warnings`, Linux x86-64 musl release 교차 빌드도 재검증했다. 의존성의 기존
경고를 workspace 전체 clippy clean으로 표현하지 않으며, 교차 빌드는 Linux 실기 수용이 아니다.
로그는 `/private/tmp/floe-review-transfer-api-battery.log` 및
`floe-review-transfer-api-final-{http,msrv,musl,clippy}.log`에 보존한다.

GTK 기본·renderd0.12.87을 바꾸지 않는다. UI 연결과 실제 브라우저 review 게시 권한,
현장 수용은 후속이다. shared-default 합성 게시 승인을 review 게시 승인으로 확대하지 않는다.

## 35. M4e-6c — 전체 review 가져오기·내보내기 UI

§34의 owner API를 `Import / export review` 패널에 연결했다. 기존 notes reviewer와
waives opt-in을 따르며, ICE와 연결된 열린 layout/view가 필요하다. 선택 오류는 없어도
된다. import는 **현재 reviewer의 전체 review 교체**이지 선택 편집이나 병합이 아니다.
기존 선택 editor가 있으면 먼저 종료해야 import할 수 있고, export는 그 editor/preview를
보존한다. trusted reviewer·경로/대상·인증 권한을 브라우저에서 바꾸는 기능은 없다.

### 파일·승인·경합

- 파일 chooser의 File handle에서 최대1MiB `slice` 하나씩 raw 전송한다. 전체 파일의
  `arrayBuffer`/text/JSON/base64나 오류 ID 배열로 변환하지 않는다. 주석16MiB·waives512MiB와
  서버의 exact pack length/내용 검증을 유지한다. 선택한 클라이언트 파일명/경로는 보내지 않는다.
- 전송 seq와 게시 seq는 독립적이다. ACK 소실/timeout은 현재 요청과 같은 Blob을 보관하고
  `Resolve identical request`로만 다시 확인한다. offset/body/seq를 바꾸거나 새 승인으로
  재시도하지 않는다. 새로고침/재접속은 서버의 남은 임시 상태를 조회할 뿐 파일을 다시
  읽거나 업로드·게시를 자동 재개하지 않는다.
- 완성된 prepare의 개수/정규화 손실·대상 이름을 표시하고, portable 파일이 같은 DRC run인지
  확인하는 체크와 전체 교체 체크를 각각 요구한다. size/time/count header만으로 run을
  인증할 수 없음을 표시한다. prepare 최초 제출 시각을 포함한 보수적30초 기한과
  DRC id/revision·view id·연결 epoch를 다시 검증한다. pan/zoom은 같은 review를 무효화하지 않는다.
- 실제 승인은 기존 Notes/Waives 컨트롤러에 넘긴다. 그 컨트롤러가 다시 최신 reviewer/
  review_rev를 확인하고 기존 승인 ledger·session recovery·receipt·cancel을 소유한다.
  waive는 실제 게시 전부터 기존 조회를 중지하고 적용 ACK와 같은 ready reader revision을
  확인한 뒤 재개한다. transfer 패널이 별도 저장 경로나 재연결 autosave를 만들지 않는다.
- 전송/preview 중 두 선택 편집 컨트롤러를 잠근다. 읽기 전용 export를 진행하는 것을
  게시 중이라고 표시하지 않는다. 취소는 accepted 작업 취소+알려진 token revoke이며
  이미 게시된 review를 되돌리지 않는다. ACK 소실로 남은 서버 upload는 조회 후 명시 폐기한다.
  연결 종료/늦은 응답은 추가 chunk·승인·poll을 만들지 않고 File/Blob 참조를 놓는다.
  최종 세션 종료에서는 오래된 다운로드 목록/사용량도 화면에서 제거한다.
- 내보내기 목록은 작업 이력과 독립적인 artifact 목록을 사용한다. refresh가 같은 파일의
  버튼을 다시 만들어 키보드 focus를 잃지 않는다. download는 CSRF hidden form의 POST이며
  token을 URL에 넣지 않는다. 다운로드 요청 메시지와 실제 브라우저 파일 저장 성공은 구별한다.
  파일 만료/슬롯/예약 bytes/진행 상태·명시 release를 노출하며 세션 종료가 임시 자원을 회수한다.

### 검증·남은 수용

ES2017 gate에 `drc-transfer.test.cjs`와 실제 DRC+Notes+Waives 컨트롤러를 조합하는
`drc-transfer-panel.test.cjs`를 추가했다. 전자는 1MiB+10byte slicing·같은 Blob replay,
queued poll/취소 wake-up·focus 보존·늦은 응답/종료·기한/범위·이중 확인과 파일 한도를
검증한다. 후자는 오류 선택이 없는 전체 교체가 기존 게시 패널만 사용하고 waive reader
장벽을 지키며 layout을 변경하지 않는지 검증한다. 기존 편집 gate는 전체 교체 승인
ACK 소실→동일 승인 복구와 승인 직전 context 무효화를 추가했다. app XHR gate는 raw Blob,
CSRF/context/offset/64bit seq 헤더와 크기 거부를 검증한다. 새 자산은 Rust bundle hash와
transport 테스트에 포함한다. CLI help의 오래된 'notes 미이관' 문구도 read-only 검사와
web view opt-in의 차이를 설명하도록 바로잡았다.

Chrome 실제 UI에서 private 합성 `synthetic.oas`/2-error ICE로 정상 layout/DRC 연결,
무선택 export 준비(`floe-notes-4.fe`,63bytes), 다운로드 요청 메시지와 End session을 확인했다.
실제 다운로드 파일의 저장 완료는 확인하지 않았으므로 브라우저 다운로드 수용 완료로
표시하지 않는다. source/입력은 보존됐고 review target/lock은 생기지 않았으며 server exit0,
private runtime 디렉터리 빈 상태를 확인했다. 파일 input 줄바꿈/preview word-wrap와
전송 중 문구를 실제 화면 점검 후 보강했다.
합성 shared-default 게시 승인은 **이 파일 업로드·review 게시의 승인으로 확대하지 않았다**.
browser chooser/upload·최종 approve 클릭, 기존 review 교체·Firefox/ETX/NFS 수용은 남는다.
로컬 Firefox가 설치되지 않아 자동 실행 시도는 실패했고 Chrome만 검사했다.

전체 `sh tools/validate_rust.sh` 재실행은 exit0·`RUST VALIDATION: ALL OK`로 완료했다.
app11/core215/web60, 새 전송 API/ES2017 UI와 기존 HTTP/WS, jobdeck80·renderer46,
KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 통과했다. Rust1.89 core215/web60/
transport10, Linux x86-64 musl release 교차 빌드, scoped fmt와 vendored
`clippy --no-deps --all-targets -- -D warnings`도 통과했다. 의존성의 기존 경고는 남는다.
새 private valmini의 legacy oracle은 macOS fork 대기를 피하도록 `--legacy --jobs 1`로
먼저 만들었으며 대조 geometry나 gate를 생략하지 않았다. 마지막 테스트 보강은
비동기 미완료 상태의 조용한 성공 종료를 거부하며 전체 battery에도 포함됐다.

첫 전체 실행은 기존 worker-client lifecycle의 `query_errors`에서 8개 query 직후
render 제출이 `Busy: worker command queue full`을 반환해 실패했다. 해당 test는
8-slot 전송 큐가 즉시 비워진다고 가정한다. 제품/테스트 코드를 바꾸거나 실패를 숨기지 않고
단독3회(각14개) 통과와 다른 빌드가 끝난 뒤 전체 재실행 통과를 확인했다. 부하/스케줄링
의존의 테스트 취약점은 별도 남기며, 단독 통과만으로 근본 해결됐다고 주장하지 않는다.
첫 실패 로그는 `/private/tmp/floe-review-transfer-ui-battery.log`, 최종 전체 로그는
`/private/tmp/floe-review-transfer-ui-battery2.log`다. 별도 `floe-review-transfer-ui-`
접두사의 `tests/clippy/msrv/musl/lifecycle-{1,2,3}.log`도 보존한다.
GTK 기본·renderd0.12.87은 유지하며 이 단계로 전체 웹 전환/현장 수용 완료를 선언하지 않는다.

## 36. M4f-1 — Python-free 배포 진단과 빌드 식별

SYS-01/02의 전용 웹 portable 조립에 앞서 `floe2-web selfcheck`를 추가했다.
기존 Python/GTK `make_portable.sh`, 기본 실행기, 인증/공유 범위나 source/cache 포맷은
바꾸지 않는다. 새 HTTP endpoint·리스너·브라우저 실행/파일 게시 동작도 없다.

### 검사 범위와 종료 의미

- `--version`과 JSON metadata에 앱 버전, source revision, target triple, 내장 웹 bundle,
  index/renderd 호환 버전을 표시한다. linked worktree의 `.git` 파일을 지원하며, 소스 ZIP이
  무관한 상위 저장소 revision을 상속하지 않는다. `FLOE_SRC_REV`는 빌드 시 명시할 수 있고
  빈 값/공백/제어 문자/128byte 초과를 거부한다. `+`는 빌드 스크립트가 관찰한 dirty 상태이며
  신뢰 서명이나 전체 binary checksum이 아니다. GTK/renderd0.12.87은 그대로다.
- `--metadata-only`는 도구 검색/실행을 하지 않고 `runtime_checked:false`, 빈 checks를
  반환한다. 실행하지 않은 검사를 `ok:true`로 주장하지 않는다.
- 기본 selfcheck는 정상 명령과 같은 override→dev→adjacent→PATH 검색을 사용한다.
  `--adjacent`는 앱 실행 파일 옆의 index/renderd만 검사하고 그 도구의 override와 fallback을
  무시한다. 두 모드의 scope와 실제 검사 경로를 JSON에 노출한다. 잘못된 명시 override는
  일반 모드에서 여전히 hard error다.
- 실제 index 버전 확인과 renderd ready handshake·종료/자기 임시 디렉터리 정리를 수행한다.
  첫 검사 실패 뒤에도 다른 도구 결과를 수집하되 취소 뒤에는 다음 검사를 시작하지 않는다.
  필수 검사 성공 exit0, 검사 실패 exit1, CLI 오류 exit2이며 SIGINT/SIGTERM은 기존 코드다.
- Firefox는 **경로만** 찾고 실행하지 않는다. `--no-open`이 유효하므로 없는 것은 참고 정보다.
  `desktop_acceptance:unverified`를 명시하며 브라우저 버전/기능·ELF/GLIBC·렌더 pixels·
  Firefox/ETX/NFS 실기 수용을 이 검사로 대체하지 않는다.
- 공백/한글 설치 경로는 지원한다. 반면 현재 worker wire는 공백/제어 문자가 있는 TMPDIR을
  거부한다. 이 경우 selfcheck도 명시 실패하며 임시 자원을 남기거나 다른 경로로 조용히
  fallback하지 않는다. 설계 파일은 열거나 색인/렌더하지 않는다.

### 버전 검사의 deadline 보강

기존 index 검사는 자식 종료를 5초 기다린 뒤 stdout reader thread를 무제한 join했다.
wrapper가 종료됐어도 자손이 stdout을 상속하면 끝나지 않을 수 있었다. 기존 nonblocking
pipe helper를 재사용해 상태와 **EOF 모두**를 같은 5초 기한/취소 검사 안에서 기다린다.
4096byte 응답 상한·UTF-8/버전 검증을 유지하고 오류 시 직접 실행한 자식을 수거한다.
자손 전체 sandbox나 OS의 uninterruptible I/O까지 강제 중단한다는 보장은 아니다.

### 검증

`validate_web_selfcheck.py`를 전체 battery에 연결했다. PATH 없는 실제 Rust 3개 재배치,
공백/한글 설치 경로·인접 모드의 override 무시와 일반 모드의 hard error, browser 무실행,
metadata-only 무실행, 불일치/UTF-8/oversize/잘못된 ready, TMPDIR 오류·정리, wrapper 종료 후
남은 stdout의 5초 종료, SIGTERM143·자식 수거를 고정한다. 전용 사설 Git fixture로 clean/
dirty/worktree/ZIP/명시 revision을 검사한다. Python은 개발용 gate일 뿐 배포 의존성이 아니다.
실제 shared-default 합성 게시 승인은 DRC review 게시/업로드 승인으로 확대하지 않는다.

첫 통합 실행은 테스트 TMPDIR에 공백이 있어 기존 wire 거부를 만났다. 정상 fixture는
공백 없는 TMPDIR을 쓰고, 공백 오류는 별도 회귀로 남긴 뒤 통합 gate를 통과했다.
scoped clippy의 불필요한 `()` 경고도 수정 후 통과했다. Rust1.89 app13/core215/web60/
transport10과 Linux x86-64 musl release 교차 빌드는 통과했다.

전체 `sh tools/validate_rust.sh`는 exit0·`RUST VALIDATION: ALL OK`로 완료했다.
workspace unit, 새 selfcheck와 기존 CLI/HTTP/WS·ES2017 UI, jobdeck80·renderer46,
KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 통과했다. 새 private valmini의 legacy
oracle은 기존 macOS fork 대기를 피하도록 `--legacy --jobs 1`로 먼저 생성했다.
scoped fmt·vendored clippy `--no-deps --all-targets -- -D warnings`도 통과했으며
의존성의 기존 경고를 workspace 전체 무경고로 표현하지 않는다. 로그는
`/private/tmp/floe-web-selfcheck-battery.log`와 같은 접두사의
`integration2/clippy2/msrv/musl.log`에 보존한다. 검증용 임시 venv 링크만 제거하며
원래 venv·승인된 합성 shared-default 게시 결과는 보존한다.

다음 M4f-2는 **별도** Rust+내장 자산 portable 조립이다. 기존 GTK 패키지와 이름/출력을
분리하고, offline build·대상 ELF/GLIBC 확인·무결성 manifest·원본 dependency/font notice를
보존해야 한다. vendor crate 목록만으로 Rust 표준 라이브러리·정적 musl까지의 고지가
완료됐다고 가정하지 않으며, 실제 toolchain/runtime 고지 출처도 확인한다.
Linux 실행 검사를 할 수 없는 교차 조립은 미실행으로 표시하고 현장 검사를
남긴다. 현재 단계에서는 tarball/GTK 은퇴/현장 수용 완료를 선언하지 않는다.

## 37. M4f-2 — 별도 Rust 웹 portable

`tools/make_web_portable.sh`와 개발용 `floe-web-packager`가 Rust 실행 파일3개·내장 UI를
오프라인 조립한다. 기존 `make_portable.sh`/GTK 실행기·권한 endpoint·renderd0.12.87은
변경하지 않는다. 자세한 명령·배포/고지 범위는 [WEBUI_PORTABLE.ko.md](WEBUI_PORTABLE.ko.md).

설치된 cargo/rustc/대상 std만 사용하며 다운로드나 target 설치를 자동 수행하지 않는다.
native/build dependency closure의 원본 고지·manifest, font·toolchain 자료를 수집하고
각 파일 SHA-256을 검사한다. GTK/Python/Node/브라우저와 개발 packager는 배포하지 않는다.
GNU는 ELF 로더·허용 공유 라이브러리·실제 동적 version requirement의 GLIBC 상한을,
musl은 interpreter·DT_NEEDED·버전 요구 부재를 검사한다. section header나 파일 안의
임의 GLIBC 문자열만으로 호환성을 판정하지 않는다. tar의 외부 TAR_OPTIONS는 무시한다.
고지 수집은 배포 승인이나 새 사용권을 의미하지 않는다.

새 private stage만 사용하고 같은 부모의 최종 archive에 hard-link로 비덮어쓰기
게시한다. 파일/디렉터리/깨진 symlink와 게시 경쟁은 모두 기존 대상을 보존한다.
취소 시 직접 소유한 자식 그룹을 종료하고 leader를 수거한 뒤 stage를 정리한다.
metadata/EOF는10초·스트림당4MiB, notice 합128MiB·개별 입력128MiB 상한이다.
SIGKILL/시스템 장애 잔류는 별도이며 다음 실행이 미지의 stage를 자동 삭제하지 않는다.
게시 완료 뒤 늦은 취소나 stdout 파이프 종료가 파일을 되돌리지는 않는다.

Linux x86_64 조립은 실제 `selfcheck --adjacent`를 반드시 실행한다. macOS 교차 조립은
Linux 실행을 생략한 것이 아니라 **실행할 수 없음**을 `runtime_checked=false`로 기록한다.
어느 경우에도 `desktop_acceptance=unverified`다. GNU/실제 Linux·Firefox/ETX/NFS 및
About 고지 UI·남은 조작 parity·GTK 은퇴는 후속으로 남는다.

### 검증과 수정 기록

합성 ELF의 GLIBC 상한/임의 문자열 오탐/잘못된 offset·부족한 payload·동적 테이블,
옵션/불변 output·stage 정리의 Rust unit6개와 scoped clippy를 통과했다. Rust1.89에서도
6개를 통과했다. 최초 테스트의 문자열 길이와 macOS `/var`→`/private/var` 기대값 오류는
fixture를 수정했고 판정 기준을 약화하지 않았다. 고지 첫 실행은 미사용 UEFI r-efi의
자료까지 요구해 게시 전에 실패했다. 고지 생략으로 우회하지 않고 target-filtered
dependency closure를 사용해 실제 선택되지 않은 패키지를 제외했다.

실제 musl archive를 조립해 세 ELF, 전체 목록/hash, 공백/한글 경로 재배치, 손상 사본
거부를 검사했다. 약6.4MiB이며 Linux 실행 수용으로 보고하지 않는다.
`validate_web_portable.py`는 합성 도구로 기존 output/깨진 symlink·잘못된 옵션,
notice/build/ELF 실패와 SIGTERM143·직접 자식 수거·stage 제거를 검사하고 전체 battery에
연결했다. 이 합성 경로와 실제 archive 검증을 구분해 보고한다.

전체 `sh tools/validate_rust.sh`는 exit0·`RUST VALIDATION: ALL OK`로 완료했다.
packager6·app13/core215/web60과 기존 CLI/HTTP/WS·ES2017 UI, jobdeck80·renderer46,
KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 통과했다. 새 private valmini의 legacy
oracle은 `--legacy --jobs 1`로 먼저 생성했고 원본 geometry 대조를 생략하지 않았다.
scoped fmt/clippy `--no-deps --all-targets -- -D warnings`, Rust1.89 packager 검증과
실제 musl 조립을 별도로 통과했다. 기존 dependency/개발 oracle 경고는 남는다.
로그는 `/private/tmp/floe-web-portable-battery.log` 및 같은 접두사의
`real3/integration2/clippy-final/msrv-final.log`다. 실제 archive는 private 임시 산출물이며
Git에는 코드/문서/검증만 포함한다. 검증용 venv 링크만 제거하고 원본 venv·합성 게시
결과와 main/feature-jobdeck 작업은 보존한다. 다음은 About/빌드 정보 연결이며 전체
웹 전환·Linux 실기·현장 수용 완료를 선언하지 않는다.

## 38. M4f-3a — 읽기 전용 About / 빌드 식별

상단 About은 앱 버전·소스 revision·target·웹 bundle·expected native compatibility를
표시한다. launcher와 `selfcheck --metadata-only`가 같은 build-info를 사용한다.
index/renderd 값은 **호환 요구값**이지 실행 중 도구의 source hash가 아니다.
`unknown`/dirty `+`와 desktop acceptance `unverified`의 의미도 화면에 명시한다.
인증된 `GET /api/v1/about`만 사용하며 source/index/renderer/selfcheck를 실행하지 않는다.
뷰가 없어도 읽을 수 있고, endpoint에 파일 경로나 게시/업로드 기능은 없다.

웹 bundle fingerprint에 About JS/DTO와 원본 Noto Sans Mono OFL을 포함했다.
고지 본문은 `textContent`로만 표시하며 HTML/링크/외부 자산을 실행하지 않는다.
모달은 배경 입력/접근성 노출을 차단하고 Tab·Escape·포커스 복원·pagehide 정리를
소유한다. 닫기/새로 열기/종료 뒤 늦게 도착한 응답은 표시하지 않고 자동 재시도하지 않는다.
기존 렌더 상태·픽셀·index/renderd0.12.87 및 GTK 기본값은 그대로다.

### 고지 범위와 후속

이번 UI의 `notice_scope=embedded_font_only`는 글꼴 원문만을 뜻한다.
portable의 전체 원본 고지는 `NOTICES/INVENTORY.txt`부터 별도로 읽고 `verify.sh`로
무결성을 검사한다. 전체 고지 브라우저를 완료했다고 표현하지 않는다.
실제 musl 조립본의 고지 파일 합은 약13MiB이고 단일 Rust 저작권 문서는 약12MiB여서,
전체 원문을 초기 About JSON/JS/DOM에 무제한 적재하지 않는다.

**M4f-3b 후속:** 설치된 portable의 고지 목록/본문을 유계 페이지로 읽는 UI.
고정된 배포 출처·무결성/교체 처리·경로/심볼릭 링크 차단·읽기 자원 상한을 먼저
정의해야 한다. 개발 빌드에 인접 자료가 없을 때도 기능/고지 범위를 명시하며,
브라우저에서 서버 임의 경로를 요청하는 API는 만들지 않는다.
이 구분은 SYS-02 전체, Linux 실행·Firefox/ETX/NFS 현장 수용·GTK 은퇴를 닫지 않는다.

### 검증

ES2017 UI 회귀와 About의 schema·read-only GET·원문 일치·늦은 응답/종료·Tab/Escape/
포커스 복원 테스트를 통과했다. 실제 HTTP 테스트는 인증/CSRF·no-store·POST 거부,
경로 query 거부·로그아웃 후 거부·뷰 부재에서도 조회 가능함을 검사한다.
실제 launcher HTTP와 selfcheck의 build/bundle/compatibility 일치 및 open/index
operation이 발생하지 않음은 `validate_web_cli.py`에 연결했다.
Chrome 실제 화면에서 build/OFL 본문과 레이아웃, 배경 AX 숨김, Shift+Tab/Tab 순환,
Escape 포커스 복원, End session 뒤 About 비활성화를 확인했다. 이 합성 소스는 현재
index가 없어 뷰 open이 거부된 상태였으며, 자동 인덱싱 없이 About이 동작하는 경우를
확인한 것이다. 실칩 렌더 성능/Firefox 수용으로 확대 해석하지 않는다.
세션은 exit0으로 종료했고 임시 credential은 제거됐다. 공유 기본값·DRC 게시/업로드는
실행하지 않았다.

전체 `sh tools/validate_rust.sh`는 exit0·`RUST VALIDATION: ALL OK`다.
app13/core215/web60, HTTP11, 새 About을 포함한 ES2017 UI, jobdeck80·renderer46 및
KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 통과했다. 변경 파일 fmt/check,
app/web scoped clippy `--no-deps --all-targets -- -D warnings`와 Rust1.89 HTTP11/app13도
통과했다. 기존 dependency/PyGI/Gdk 경고는 전체 무경고로 표현하지 않는다.
로그는 `/private/tmp/floe-web-about-battery.log` 및 같은 접두사의
`http/ui2/clippy/msrv.log`다. 검증용 venv 링크만 정리하고 원본 venv·사용자 작업은
보존한다. 전체 웹 전환 및 SYS-02 완료가 아니라 M4f-3a 구현/로컬 검증 완료다.

검증 준비 중 `cargo fmt --all`이 만든 범위 밖25개 파일은 모두
`rustfmt(HEAD)`와 바이트 단위 동일함을 재확인하고 복구용 patch를 보관한 뒤
그 포맷 변경만 되돌렸다. source 변경/사용자 작업은 섞지 않았으며 이후 fmt는
변경 파일에만 한정한다.

## 39. M4f-3b — portable 원본 고지의 유계 열람

새 portable의 `NOTICES/` 원본 자료를 About에서 읽는다. 원문을 별도 JS나 바이너리에
중복 내장하지 않는다. packager가 `NOTICE-INDEX.json`을 먼저 만들고 그 SHA-1 content ID를
`FLOE_NOTICE_INDEX_SHA1`으로 앱에 고정한다. 목록에는 source revision/target, 원본 경로와
크기, UTF-8/hex 구분, 연속 chunk offset/길이/digest가 있다. `BUILD.txt`와 metadata에도
ID를 남긴다. 새 crate `floe-notices`는 기존 vendor만 사용하며 index/renderd0.12.87,
렌더 픽셀·wire·기본 GTK 실행기는 변경하지 않는다.

### 범위와 자원 계약

- 목록≤2MiB·4096파일, 원본 합≤128MiB. 목록64개와 원본 chunk≤64KiB를 한 번에 읽는다.
  UTF-8 경계는 보존하며 빈 파일도 한 페이지다. 비UTF-8 원본은 파일 전체를 hex 형식으로
  분류해 원래 byte를 잃지 않는다. packager 생성 시에는 원본 하나를 상한 안에서 읽지만
  runtime이 전체 원문을 초기 JSON/JS/DOM에 적재하지는 않는다.
- 서버는 실행 파일 인접 목록의 compiled ID/source/target과 구조를 검증하고 해당
  catalogue를 고정한다. 경로는 `NOTICES/` 하위 상대 경로만 허용하며 각 성분을 열린
  디렉터리 fd에 대해 `openat(O_NOFOLLOW)`로 연다. symlink/FIFO/비regular leaf와
  `..`·제어문자·과도한 경로 깊이를 거부한다. 임의 파일을 발견하거나 열람하지 않는다.
- 인증·CSRF가 필요한 `GET /api/v1/about/notices/{start}`와 `/{id}/{page}`만 제공한다.
  브라우저는 숫자만 보내고 서버 경로를 요청하지 않는다. 기존 Host/Origin·query 거부,
  no-store·HTTP5초 기한을 사용한다. 요청 chunk는 원본 크기·읽기 전후 identity/mtime/ctime
  및 digest 검사를 모두 통과해야 한다. 실패하면 본문을 표시하지 않는다.
- Gateway당 읽기 slot은1개다. timeout/요청 취소 시 취소 flag를 설정하지만 slot은
  syscall이 돌아올 때까지 reader가 보유해 thread가 쌓이지 않는다. 파일 읽기는 detached
  OS thread로 분리해 Tokio runtime 종료가 blocking pool을 무제한 기다리지 않는다.
  kernel의 uninterruptible I/O 자체를 강제로 중단하거나 NFS 지연이 없다고 보장하지 않는다.
- 목록 검증 실패는 `unavailable`로 고지 기능만 닫고 viewer는 유지한다. compiled ID가
  없는 개발 실행 파일/구 portable은 `not_packaged`·내장 글꼴만 가능함을 표시한다.
  인접 파일 복사만으로 활성화되지 않으며 새 packager 재빌드가 필요하다. 설치 복구/
  교체는 재시작 후 다시 검사하며 열린 앱에 목록 hot reload를 추가하지 않는다.

SHA-1은 **content/change 식별자이지 게시자 인증이 아니다**. `verify.sh`의 전체 패키지
SHA-256과 신뢰하는 배포 경로 확인은 별도로 유지한다. `selfcheck --metadata-only`는
파일을 읽지 않고 compiled ID만 출력한다. 일반 selfcheck는 `portable_notice_index`를
`index_only_not_all_file_chunks` 범위로 검사하며 원본 전체/전체 배포 검증이라고 부르지
않는다. About을 열어도 selfcheck·설계 open/index/render·게시가 시작되지 않는다.

### UI와 로컬 확인

목록/본문은 replace하며 누적하지 않는다. 이전/다음·1-based 페이지 이동·명시 재시도를
제공한다. 파일명과 HTML을 `textContent`로만 표시한다. 닫기·다른 선택·종료 후 늦은 응답은
버리고 오류 시 이전 본문을 새 제목 밑에 남기지 않는다. 자동 재시도/다운로드/쓰기 기능은
없다. About의 포커스 순환은 현재 표시되고 활성인 입력만 포함한다.

Rust catalogue5개는 UTF-8 경계·빈 파일·hex·HTML 원문, 목록 페이징·미등록 파일 제외,
identity/손상/취소, symlink/부모 symlink/FIFO/경로, 손상 목록/크기 상한을 검사한다.
HTTP12개(기존11+고지1)와 ES2017 UI 회귀는 인증·범위·손상 거부·no-store·POST 거부·
로그아웃·늦은 응답·페이지 이동·원문 보존을 검사한다. 실제 kernel-hung NFS fault를
주입한 검사는 아니며 그 경로는 읽기 slot 소유권과 종료 구조의 검토로 구분한다.

`tools/validate_web_notices.py`는 **개발용** 별도 compiled-fixture gate다. 기본 battery는
Rust/HTTP/UI 및 packager 거부 테스트를 실행하며 이 별도 바이너리 빌드를 자동 만들지는
않는다. 재현할 때 `prepare NEW_DIR` 출력의 digest/source_revision을 각각
`FLOE_NOTICE_INDEX_SHA1`/`FLOE_SRC_REV`로 설정하고 `cargo build --offline --locked
-p floe-app --bin floe2-web`로 빌드한 뒤 `run TEST_APP NEW_DIR SYNTHETIC_OAS`를 실행한다.
NEW_DIR는 새 private 위치여야 한다. 해당 빌드는 테스트 전용이며 일반 배포에 쓰지 않는다.

Rust1.89로 만든 macOS compiled fixture에서 원본70개·모든 UTF-8/hex chunk 복원,
목록/원본 손상 거부, metadata-only 무읽기와 index-only selfcheck, 인증·종료·credential
정리를 통과했다. 실제 Chrome에서도 긴 HTML의 비실행 원문 표시,64KiB 다음 페이지,
마지막4/4 페이지·비활성 Next, 목록65–70/70, hex, Escape 포커스 복원, End session 후
About 비활성화를 확인했다. 서버 exit0·credential 제거를 확인하고 생성한 탭만 닫았다.
이 소스는 index가 유효하지 않아 뷰가 열리지 않은 상태였으며, 자동 재인덱싱하지 않았다.
실칩 렌더 성능이나 Firefox/ETX 수용 결과로 확대 해석하지 않는다.

원본70개 검증 로그는 `/private/tmp/floe-notices-native-test.log`, 브라우저용 fixture는
`/private/tmp/floe-notices-native.WkLc8Y/install`이다. 승인된 합성 shared-default 결과는
유지했고 이번 고지 검증에서 공유 기본값·DRC 게시/업로드·clipboard는 사용하지 않았다.
전체 웹 전환·SYS-02·현장 수용·GTK 은퇴 완료를 뜻하지 않는다.

전체 `sh tools/validate_rust.sh`는 exit0·`RUST VALIDATION: ALL OK`다. notices5·packager6·
app13/core215/web60·HTTP12, CLI/WS·ES2017 UI, jobdeck80·renderer46과 KLayout13 PX+
2 phase-exact+14 style(jobs1/8)을 통과했다. 별도로 Rust1.89의 notices5·packager6·app13·
HTTP12도 통과했다. 해당 툴체인에는 clippy가 설치되지 않아 추가 다운로드 없이 기존
설치 툴체인의 scoped clippy `--no-deps --all-targets -- -D warnings`로 검사했고 통과했다.
변경 파일만 fmt/check했으며 기존 dependency/PyGI/Gdk 경고를 전체 무경고로 표현하지 않는다.

실제 musl archive를 Rust1.89로 다시 조립하고 compiled notice ID·원본 모든 chunk,
전체 파일/hash·공백/한글 재배치·손상 사본 거부를 검사했다. 이 archive는 검증용 dirty
pre-commit 산출물이고 `runtime_checked=false`·`desktop_acceptance=unverified`다.
Linux 실기 실행이나 릴리스 승인으로 보고하지 않는다. 경로는
`/private/tmp/floe-notices-portable.Vs6zqx/floe2-web-musl.tar.gz`이며 Git에 포함하지 않는다.

전체 로그는 `/private/tmp/floe-notices-battery.log`, 별도 검증은 같은 접두사의
`msrv-final/clippy-installed/archive-check/native-test.log`다. 실제 venv는 보존하고
검증용 `.venv` symlink만 제거했다. main의 기존 변경과 feature/jobdeck는 건드리지 않았다.

## 40. M4g-1 — 오른쪽 드래그 박스 확대/축소

입력 parity 대조에서 빠져 있던 GTK `_on_release`의 오른쪽 드래그를 이관했다.
왼쪽/가운데 pan·도형/DRC 클릭·룰러·선택 박스의 기존 동작을 바꾸지 않는다.
`Navigation::Band`와 strict `navigation.kind=band`가 시작/끝 화면 비율·참여 축·축소
여부만 받는다. world bbox/중심·배율은 Rust가 계산하고 기존 state revision CAS와
렌더 latest-only 경로를 탄다. 새 renderer 명령/worker/권한은 추가하지 않는다.

- 오른쪽 버튼을 누른 뒤 **전체 수평 excursion**의 우측이 좌측 이상이면 확대,
  좌측이 더 크면 축소다. 마지막 위치 부호로 방향을 뒤집지 않는다. 되돌아온 sliver를
  과도한 확대 박스로 해석하지 않는다. 동률은 GTK처럼 확대다.
- 방향에 맞는 수평 이동과 수직 절댓값 각각5 CSS px 이상인 축만 배율에 참여한다.
  가늘고 긴 박스는 긴 축으로 동작하며 두 축 모두 미달이면 no-op이다. 보였던 박스가
  되접혀 취소되면 안내한다. DPR은 제스처 좌표 변환에만 쓰고 world 좌표를 반올림하지 않는다.
- 흰1 device px 밴드와 방향 안내만 rAF로 표시한다. 이동 중 네트워크/geometry 렌더와
  큰 bitmap 복사는 없다. Overlay none에서도 밴드는 남으며 release에 한 번만 제출한다.
  band zoom 대기 중 기존 픽셀은 기존 frozen viewport 경로로 유지한다.
- 실제 표시 프레임/착지 crop이 현재 bbox·배율과 맞아야 시작한다. 과거 frozen frame에서
  새 상태를 기준으로 박스를 계산하지 않는다. view/revision/epoch·resize(패널 포함),
  blur/hidden/pagehide·버튼 상실·chord와 Escape는 밴드/예약 paint를 취소하고 늦은 release를
  실행하지 않는다. wheel은 제스처 중 무시하고 단순 오른쪽 클릭은 inert다.
- 기존 mouse pan과 마찬가지로 드래그 delta는 축마다 한 viewport까지다. 브라우저
  바깥 무제한 drag나 GTK의 전역 `_clamp_view`(최소/fit 배율·다이 경계) 전체 이관은 아니다.
  Rust의 기존 finite/좌표±2^62·16Mpx 유효성 검사를 유지하며 거부 시 현재 상태를 보존한다.

`validate_zoom_band.py`는 GTK의 **실제** `_track_band`/`_on_release`를 AST로 읽어 실행하고,
같은 입력을 production `gestures.js`에 전달한 뒤 Rust DTO/Viewport 결과를 비교한다.
352개 고정/seeded 케이스, DPR1/1.25/2/2.5, 음수·half-DBU 위치, 양방향/동률/되돌림,
가로·세로/얇은 박스/no-op을 포함한다. 비교 범위는 GTK의 **clamp 전** 좌표/배율이다.
개발 gate만 Python/Node를 쓰며 Rust 앱의 실행 의존성은 아니다. 전체 battery에 연결했다.

JS 단위/통합은 rAF·release1회,5px 임계·취소·stale/과거 화면 거부·숨김 overlay에서도
밴드 표시·기존 클릭/pan 보존을 검사한다. native WS gate는 PNG/raw의 확대→축소
바이트 복원, 잘못된 범위/낡은 revision 거부와 재접속을 검사한다.
이 단계는 M4 전체나 현장 Firefox/ETX 입력 수용 완료를 뜻하지 않는다.

로컬 Chrome에서 별도 합성 valmini 사본의 첫 화면·margin crop과 조작 안내를
실제 표시해 확인했다. End session 뒤 서버 exit0·접속 파일 제거와 테스트 탭 닫기를
확인했다. 이 세션은 공유 기본값/DRC review 게시·파일 다운로드를 하지 않았다.
브라우저 제어 지연으로 **실제 오른쪽 버튼 드래그의 시각/입력 수용은 미검증**이며,
이를 production JS/GTK/Rust 대조352건과 실제 native WS gate 통과로 대체하지 않는다.
테스트의 fit bbox 왕복에는 부동소수점 반올림 허용 오차를 적용하지만,
PNG/raw payload 왕복 비교는 계속 바이트 완전 일치다.

검증: 새 합성 valmini 환경에서 전체 `sh tools/validate_rust.sh`가 exit0 /
`RUST VALIDATION: ALL OK`로 완료됐다. app-core216·web60·HTTP12,
GTK/JS/Rust 입력352건·native band/margin/연속 입력 스트림, ES2017/전체 UI,
jobdeck80·renderer46와 KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 포함한다.
변경 Rust 파일의 scoped rustfmt 및 app-core/web/app의 `--no-deps --all-targets`
clippy `-D warnings`도 통과했다. 기존 의존 패키지의 tiler/VFS 경고는 별도다.
최소 지원 Rust1.89에서도 app-core216·web60, GTK/JS/Rust352건과 실제 native
band/margin/연속 입력 스트림3건을 통과했다. 새 의존성·native 호환 버전 변경은 없다.
검증용 `.venv` symlink만 제거했으며 기존 가상환경, main의 수정 및 feature/jobdeck은 보존했다.

## 41. M4g-2 — 색인 구조 미니맵과 동일 배율 이동

GTK의 고정 **180×180** 미니맵을 복원했다. 이것은 렌더 thumbnail이나 occupancy/
coverage가 아니다. 일반 layout 색인의 기존 `meta.json.frontier.depths`에 저장된
DBU bbox를 사용하며, 메인 화면의 frames off와 무관하게 구조를 보여 준다. full depth,
저장 depth 범위 밖, frontier 없는 구 캐시와 jobdeck은 다이 외곽선/현재 위치만 보인다.
색인 형식·페이지 선택·cut·LOD·renderer wire와 native 호환 요구0.12.87은 변경하지 않는다.

### 데이터·입력·비용 경계

- Rust가 view 최초 준비 시 metadata를 한 번 더 읽고 source size/mtime·bbox·dbu가
  이미 연 cache와 일치하는지 검사한다. regular/no-follow 파일만 받으며256MiB 기존
  metadata 상한, depth32개·depth당6000행·행당4~5숫자를 제한하고 잘못된 좌표를 거부한다.
  잘못된 미니맵 metadata는 view open 오류다. index/info/render 자체는 이 읽기를 하지 않는다.
- raw world 배열은 변환 후 버린다. 다이 베이스+최대32 depth의 ASCII palette 인덱스
  32400bytes씩, 최대 약1.02MiB를 Layout별 OnceLock/Arc로 공유한다. 프레임을 생성하거나
  별도 worker를 시작하지 않는다. 동시 최초 준비의 임시 변환 중복까지 RSS 상한으로
  보장하는 것은 아니다. 기존 bake의6000행/멤버 예산에 따른 구조 요약 한계도 유지한다.
- 인증된 `GET /api/v1/views/{id}/minimap/{base}`는 등록 view의 메모리 베이스만 읽는다.
  원본 경로/geometry 배열을 전달하지 않고 임의 depth/경로는404다. snapshot은 현재
  depth 키·다이·최대6개 palette 사각형만 보내며 위치 투영·클릭 world 계산은 Rust가 맡는다.
- 브라우저는180px Canvas 베이스 최대3개를 보관하고 요청은1개만 진행한다. 빠른 depth
  변경은 최신 값으로 합치며 pan은 베이스 조회 없이 위치 표시만 갱신한다. 접속 epoch/
  view 교체·숨김·종료에서 과거 응답을 버린다. 읽기 실패는 명시 Retry까지 재시도하지 않는다.
- 다이 내부 왼쪽 클릭은 현재 배율을 유지하고 이동량을 현재 **16 device px** 간격에
  ties-even으로 맞춘다. letterbox는 no-op이다. Enter/Space는 현재 배율로 다이 중심 이동,
  IME/modifier/진행 중 제스처는 보호한다. 기존 state CAS·latest-only·margin 경로를 재사용한다.
  전역 camera clamp/단일 인스턴스·초기 CLI 옵션 parity를 함께 변경하지 않는다.
- GTK와 동일한 배경/외곽/구조/현재 뷰 색,0.7px 구조 생략,6px 현재 뷰 상자↔점 기준을
  적용한다. 다이와 완전히 겹치지 않는 뷰에서는 GTK의 stray border를 그리지 않도록
  보정했다. 미니맵은 정밀 geometry 조회나 빠진 도형을 복구하는 표시가 아니다.

`tools/validate_minimap.py`는 GTK의 실제 helper/미니맵/클릭 함수를 AST로 실행해
Rust와180개 사례의 베이스·표시 픽셀 완전 일치와 배율/16px 위상 이동을 대조한다.
음수·큰 offset·얇은 다이·여러 depth/줌·letterbox를 포함한다. 좌표만 부동소수점 허용
오차를 적용하며 픽셀은 exact다. Python/Node는 개발 gate일 뿐 앱 runtime 의존성이 아니다.
전체 battery에 연결했고, unit은 손상/초과 metadata·잘못된 키·off-die를 추가 검사한다.
native HTTP/WS gate는 인증·404·조회 무렌더/무상태변경·동일 배율/위상 클릭과 재접속을,
JS/client gate는3개 캐시·depth 합치기·pan 무조회·응답 검증·retry·suspend/resume·종료를 검사한다.

로컬 Chrome에서 별도 합성 valmini 사본을 열어 frames off에서도 depth0/1 구조가
표시되고 full은 다이 외곽선으로 전환되는 것을 확인했다. 미니맵의 다른 위치를 클릭한
전후 뷰 크기는521.172×356.841µm로 유지되고 현재 위치 박스와 메인 화면이 이동했다.
End session 뒤 미니맵 숨김·서버 exit0·접속 파일 제거를 확인하고 생성한 탭만 닫았다.
검증 사본은 `/private/tmp/floe-minimap-ui.uyD4A3`이며 공유 기본값/DRC 게시·업로드·
clipboard/다운로드는 사용하지 않았다. 현장 Firefox/ETX, 실제 오른쪽 drag 입력,
남은 CLI startup/single-instance와 공유 권한 수용은 별도로 남는다.

전체 `sh tools/validate_rust.sh`는 exit0·`RUST VALIDATION: ALL OK`다. core218·web60·
HTTP12, GTK 미니맵180건·band352건·native 스트림4건·전체 ES2017 UI,
jobdeck80·renderer46와 KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 통과했다.
Rust1.89에서도 core218·web60·미니맵180건·native 스트림4건을 통과했다.
변경 파일 scoped rustfmt/check, app-core/web/app clippy `--no-deps --all-targets`
`-D warnings`와 최신 자산 release 빌드를 통과했다. 기존 dependency 경고는 별도다.
로그는 `/private/tmp/floe-minimap-battery.log`, `floe-minimap-msrv.log`,
`floe-minimap-clippy.log`, `floe-minimap-ui-lifecycle.log`다.
검증용 `.venv` symlink만 제거했으며 실제 가상환경·main의 기존 수정·feature/jobdeck은 보존했다.

## 42. M4g-3 — depth 상대 입력과 DRC 단축키 정정

현재 `floe/gui.py._on_key/_depth_step` 대조에서 누락된 `<`/`>`와 잘못 배정된
DRC 순회 키를 이관했다. 범위는 UI-01/05의 이 입력들이다. q 종료 확인·Ctrl+,
잡덱 모드 전환·전체 startup/single-instance·현장 입력 수용은 다음 작업으로 남긴다.
native index/renderd 호환 버전0.12.87과 렌더 픽셀 정책은 바꾸지 않는다.

- `<`/`>`는 절대값을 브라우저에서 추측하지 않고 `depth_step:-1|1`을 보낸다.
  기존64개 입력 큐·ACK/snapshot 장벽을 사용하며 controller가 revision CAS 락 안에서
  현재 depth와 native 최대 depth를 읽는다. 경계 no-op은 revision/렌더를 증가시키지 않는다.
  stale revision·0/기타 delta·절대 depth와 동시 지정·null/중복 키는 거부한다.
- GTK처럼 full(999 이상)은 실제 최대 depth에서 시작한다. 최대값 미확인 시999를
  사용하며 결과는0..최대값, setter의999 한계에 맞춘다. 최대값이999 이상일 때도
  saturating 산술로 처리한다. 초기 open은 relative depth를 허용하지 않는다.
  기존 절대 depth 입력·9 9→full·view 배율/위치는 유지한다.
- DRC 다음/이전은 **period/comma**다. 초기 웹 M2의 n/p는 제거한다. 현재 규칙·
  Selected/In view·pagination·wrap·클릭/이동 모드·Escape 복원은 바꾸지 않는다.
  오류 행의 위/아래도 그대로이고 입력칸·IME/modifier는 보호한다.
- `n`은 기존 owner 주석 snapshot/editor, `w`는 기존 owner waive snapshot/editor를
  연다. gold 선택 우선·없으면 현재 오류라는 기존 유계 선택을 재사용한다. 패널이
  닫혀 있으면 열고 해당 입력으로 포커스를 옮긴다. 권한 없는 세션은 안내만 보인다.
- `w`의 신규 snapshot에서 모두 waived이면 Clear waive, 그 외/혼합이면 Waive를
  미리 선택한다. reserved 상태는 기존 경고/명시 승인 계약을 유지한다. **키 자체는
  prepare/게시를 하지 않는다.** preview·체크 승인·이전 파일/legacy 검증이 계속 필요하다.
  이미 열린 편집기는 초안을 보존하고 재포커스만 한다. 반복 키는 읽기/게시를 늘리지 않는다.

`validate_depth_keys.py`는 실제 GTK 함수 AST를 실행해200개 현재/최대/depth 방향
조합을 Rust와 대조한다(미확인·0·999 경계·u32/u64 최대 포함). 개발 gate만 Python을
사용한다. controller 단위 테스트는 CAS/no-op/거부 시 상태 보존을, native HTTP/WS는
full→1→2→경계→1→0→경계의 프레임·camera 보존을 검사한다. 실제 UI 회귀는
연속 depth 입력의 ACK 대기·modifier/IME, DRC 순회/필터/복원, real editor 배선,
반복 읽기 억제·선택 ID 보존·혼합/all-waived 토글 제안·자동 게시 없음을 검사한다.

로컬 Chrome에서 full→1→2 depth 이동의471.129×446.051µm camera 보존,
period로Global1 선택·comma로Global3 wrap, n/w의 실제 편집기 진입과 Escape 폐기를
확인했다. 첫 시각 검증에서 기존 snapshot/preview 코드가 아직 hidden/disabled인
입력에 focus하던 문제를 발견했다. IO 해제와 render 이후에만 focus하도록 notes와
waives를 고치고, disabled/hidden이면 focus되지 않는 회귀 모델에서 실패→통과를
확인했다. 재빌드한 Chrome에서 주석 입력칸 및 Waive 선택칸 focus를 재확인했다.
이 브라우저 검증은 preview/게시·업로드·공유 기본값을 사용하지 않았다. 두 번의
End session 모두 서버 exit0과 접속 파일 제거를 확인했고 합성 디렉터리
`/private/tmp/floe-keys-ui.RnHbRW`에 notes/waive sidecar가 생기지 않았다.

전체 `sh tools/validate_rust.sh`는 exit0·`RUST VALIDATION: ALL OK`다. core219·web60·
HTTP12, GTK depth200·미니맵180·band352, native 스트림5건, jobdeck80·renderer46,
KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 통과했다. 별도 드라이버로 실행하는
core oracle2건은 일반 cargo test에서는 ignored이지만 배터리에서 필수 실행한다.
Rust1.89에서도 core219·web60·depth200·native 스트림5건을 통과했다.
브라우저에서 보강한 focus 수정 후 전체 ES2017 UI gate·release 자산 빌드,
app-core/web/app clippy `--no-deps --all-targets -D warnings`와 변경 Rust 파일
scoped rustfmt/check를 다시 통과했다. 로그는 `/private/tmp/floe-keys-battery.log`,
`floe-keys-msrv.log`, `floe-keys-ui-focus.log`, `floe-keys-clippy.log`다.
기존 dependency 경고·현장 Firefox/ETX·남은 입력/운영 수용은 별도다.

## 43. M4g-4 — q 종료 확인과 미확정 종료 표시

GTK `_confirm_quit()`처럼 취소를 기본으로 하는 종료 확인창을 연결했다. live canvas의
`q`와 상단 End session은 같은 modal을 연다. 입력칸·modifier·IME·진행 중 drag의
키는 가로채지 않는다. 확인창을 여는 것만으로 파일을 저장하거나 초안/선택/연결을
폐기하지 않는다. 기존 세션 API·worker 종료 계약·native 호환0.12.87은 그대로다.

- Cancel에 기본 포커스를 두므로 바로 Enter하면 취소한다. Escape·배경 클릭도 취소,
  Tab/Shift+Tab은 두 버튼 안에서 순회하고 닫으면 원래 포커스와 aria-hidden을 복원한다.
  브라우저 탭 자체를 닫는 동작을 가로채거나 자동 저장하지 않는다.
- 확인 버튼만 기존 `DELETE /api/v1/session`을 한 번 호출한다. 반복 키/중복 클릭은
  종료를 재전송하지 않는다. pagehide는 열린 확인창을 닫으며 복원 시 재승인을 요구한다.
  기존에 승인한 게시의 커밋은 종료로 되돌려지지 않는다고 명시한다.
- 기존 client가 DELETE 실패를 덮고 `Session ended`로 표시하던 부분도 보완했다.
  실패 시 로컬 뷰는 중단되지만 **Server shutdown unconfirmed**로 알린다. session 및
  기존 게시 복구 기록은 성공 응답까지 남긴다. 자동 재시도·다른 세션의 종료는 없다.
  사용자가 로컬 launcher를 확인해야 하며 성공/실패 네트워크 수신이 파일 게시 결과를
  판정하지 않는다. 이미 열린 파일과 서버 자원의 실제 종료는 기존 endpoint가 담당한다.

ES2017 독립 modal gate는 취소 기본값·focus trap·Escape/IME·ARIA 복구·pagehide/복원·
확인 전 무동작·한 번만 호출을 검사한다. 실제 app.js client gate는 q와 버튼 배선,
취소 전후 HTTP/WS 불변, 확인 시 단일 DELETE, 실패 시 미확정 표시/복구 기록 보존을
검사한다. 새 자산은 고정 embedded route와 bundle hash에 포함된다.

로컬 Chrome의 합성 valmini에서 q→Cancel 기본 포커스·Enter 취소·Escape 취소·
Tab 순환과 상단 버튼의 동일 확인창을 확인했다. 취소 전후 live gen2와 camera는
유지됐다. 확인 버튼 이후 Session ended·버튼 비활성·서버 exit0·접속 파일 제거를
확인하고 생성한 탭만 닫았다. 기본값/DRC 파일 게시·다운로드·업로드는 사용하지 않았다.
실제 네트워크 응답 유실은 Chrome에서 주입하지 않았으며 그 경로는 client mock gate다.

전체 ES2017 UI·web60·HTTP12·실제 native CLI 수명주기·변경 파일 scoped rustfmt와
web/app clippy `--no-deps --all-targets -D warnings`를 통과했다. Rust1.89에서도
web60·HTTP12를 통과했다. 최종 자산으로 release 재빌드와 Chrome 확인을 수행했다.
렌더러/플래너/인덱스 코드는 이 단계에서 변경하지 않았다. 전체 native/오라클 배터리의
가장 최근 실행은 직전 M4g-3의 exit0이며, 이 단계는 위 UI/HTTP/CLI 게이트를 재실행했다.
로그는 `/private/tmp/floe-exit-ui-final.log`, `floe-exit-rust-final.log`,
`floe-exit-cli.log`, `floe-exit-msrv.log`, `floe-exit-clippy-final.log`다.
main의 기존 수정과 feature/jobdeck worktree는 보존했다. Firefox/ETX 현장 수용과
잡덱 모드 전환·CLI startup/single-instance·공유 정책의 나머지는 여전히 미완료다.

## 44. M4g-5a — 잡덱 모드 전환의 상태 준비와 GTK 오라클

열린 jobdeck의 level/chip/source-layer 전환에 앞서 `view::deck_mode`를 추가했다.
**상태 준비만** 담당한다. 아직 HTTP/API·worker 교체·Ctrl+, 또는 모드 선택 UI에
연결하지 않았으므로 현 사용자는 새 open에서만 mode를 고를 수 있다. 렌더러·색인
형식과 native 호환0.12.87은 바꾸지 않는다.

- `DeckModeMemory`는 로드한 덱/선택 레벨의 수명에 귀속된다. GTK `save_visibility`/
  `restore_visibility`처럼 level/chip은 같은 leaf namespace를 공유하고 raw source
  layer는 독립적으로 기억한다. 최초 raw 진입은 그 모드의 기본 가시성을 읽는다.
  부모 행을 전송해 일부 숨긴 칩이 다시 켜지지 않도록 effective leaf 집합을 이관한다.
- 새 모드의 `Model`·`ViewState`를 준비하면서 DBU bbox와 pixel dimensions를 그대로
  유지한다. depth/detail/thin/frames/labels/font도 유지하며 카메라를 fit으로 바꾸지 않는다.
  모드별 layerprops 색·채움·폭/상속은 새로 읽는다. 이전 raw 숫자 key의 스타일을
  다른 의미의 덱 key에 복사하지 않는다. 이전 isolate의 restore handle도 제거한다.
  GTK의 generic `_apply_cache`가 mono를 끄는 것과 달리 **mono는 유지**한다.
- 같은 덱 경로/선택 레벨/source size·mtime, DBU/bbox와 현재 model revision을
  확인한다. 다른 selection·좌표계·현재 model, invalid view는 거부하고 같은 mode는
  호출자가 no-op으로 처리하도록 한다. 이 검사는 캐시 hot-reload 불변성/권한 검사를
  대신하지 않는다. 후속 service는 등록 source 검증과 revision CAS가 별도로 필요하다.
- 준비는 기존 view/memory를 수정하지 않고 렌더 permit을 예약하거나 worker를 시작하지
  않는다. 후속 service가 성공한 전환에서만 반환된 state/memory를 함께 설치해야 한다.
  기본 decode8+raster4 worker 둘을 겹쳐 시작하면16 CPU admission을 넘으므로, 안전한
  순차 교체·실패/취소 처리·이전 view/query/게시 토큰 무효화가 다음 구현 범위다.

`validate_app_deck_render.py`에 실제 GTK 가시성 메서드와 Python `DeckCache.set_mode`
오라클을 추가했다. 전체/선택[2,3] 로드 각각12단계, 총24단계의 모드 왕복에서 일부
칩만 표시·전체 숨김·전체 표시·독립 raw 가시성을 비교한다. 매번 off-center camera의
103×91 native PNG도 Python `ShotRunner` 결과와 바이트 대조한다. 이는 전환 직후
archival exact 출력 오라클이며 HTTP 연결/worker handoff·브라우저 수용 테스트는 아니다.
뷰 옵션·기본 스타일 복원·isolate 초기화·prepare 무변경·오류 시 상태 보존도 단언한다.
다른 selection으로 memory를 재사용하는 경우를 거부하며, 새 ignored integration test는
기존 전체 배터리 드라이버가 실행 횟수24와 성공 marker를 필수 확인한다.

집중 GTK/native 오라클은23 PNG/report 쌍+6 API+24 mode PNG+8 signal 단계를 통과했다.
픽셀 비교가 모두 빈 화면에서 통과하지 않도록 각 selection의 PNG가 최소3종류이고
숨김/표시 전환이 모두 존재함도 확인한다. 전체 `sh tools/validate_rust.sh`는 exit0,
`RUST VALIDATION: ALL OK`다. core219·web60·HTTP12, GTK depth200·미니맵180·band352,
jobdeck80·renderer46와 KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 통과했다.
Rust1.89에서도 core219와 GTK/native24전환 포함 드라이버를 통과했다. 변경 Rust 파일의
scoped rustfmt와 app-core clippy `--no-deps --all-targets -D warnings`도 통과했다.
로그는 `/private/tmp/floe-deck-mode-battery.log`, `floe-deck-mode-oracle.log`,
`floe-deck-mode-msrv.log`, `floe-deck-mode-clippy-final.log`다. 기존 dependency/GTK/Pillow
경고는 별도다. 실제 브라우저의 모드 전환·현장 Firefox/ETX와 실칩 jobdeck 수용을 완료한
단계는 아니며, main의 기존 수정과 feature/jobdeck 작업 트리는 보존했다.

## 45. M4g-5b — 열린 잡덱 모드 전환과 단일 예약 worker 교체

§44의 상태 준비를 owner service와 웹 UI에 연결했다. 열린 잡덱에서 Display의
**Jobdeck mode**로 level/chip/source-layer를 선택하며, canvas의 Ctrl+,는
level→chip, chip/source-layer→level로 전환한다. Source의 Open mode는 다음 open용
설정으로 분리한다. 현재 덱의 source와 로드한 level 집합은 바뀌지 않는다.
새 렌더링 알고리즘·인덱스 포맷·자동 재색인은 없고 native 호환0.12.87을 유지한다.

### 상태와 작업 계약

- 기존 `POST /api/v1/operations`에 `kind:mode`를 추가했다. 입력은 seq, 현재 view_id,
  base_state_rev와 mode뿐이다. 임의 source/path/levels/body는 거부한다. 등록 source의
  재검증 후 같은 선택 집합의 새 모드 dataset/model을 준비한다.
- viewport의 DBU bbox/pixel dimensions, depth/detail/thin/frames/labels/font/mono는
  유지한다. level/chip은 exact leaf 가시성을 공유하므로 level 부모가 켜져 있어도 숨긴
  칩을 다시 켜지 않는다. source-layer 가시성은 독립적으로 기억한다.
  모드별 기본 색/채움/폭을 다시 읽고 이전 isolate restore handle은 제거한다.
- 원래 attachment에서 읽은 revision과 현재 attachment를 commit 직전에 다시 검사한다.
  다른 WS가 준비 중 뷰를 편집했다면 stale로 실패하고 그 최신 원래 뷰를 보존한다.
  동일 모드 요청은 worker/epoch/state_rev를 바꾸지 않는 no-op이다.
- 같은 seq/body 재전송은 원래 receipt만 반환한다. 전환으로 원래 view_id가 폐기된
  뒤에도 replay를 먼저 처리한다. 같은 seq의 다른 body는 충돌이다.
  새 view_id/worker epoch로 이전 frame·query·layer/settings/prepared scope가 섞이지
  않으며, 이전 WS는 종료되고 새 연결이 새 snapshot을 읽는다.

### 자원·취소·실패 경계

`PreparedReplacement`는 기존 CPU/decoded/worker **동일 예약**을 공유한다. 준비 중
추가 control thread는 대기만 하며 두 번째 native engine을 열지 않는다. 초기 state
검증·새 attachment entropy·행 metadata 등 실패 가능한 준비를 cutover 전에 끝낸다.
취소 또는 stale CAS는 대기 thread를 종료하고 원래 worker를 계속 사용한다.

commit은 기존 controller의 CAS 락 아래 stop/query 무효화와 활성화 gate를 연결한다.
새 controller는 기존 worker의 close/drop/reap와 control thread 종료를 확인한 뒤에만
native engine을 연다. 이 대기는 commit 후 새 뷰가 취소돼도 생략하지 않는다.
Opening/Cancelling controller에는 재교체를 허용하지 않아 종료 확인을 건너뛰는
연쇄 교체를 막는다. controller handle은 예약을 weak로만 참조하므로 닫힌 handle이나
옛 WS가 남아 있어도 native 종료 후 quota를 계속 붙잡지 않는다.

`succeeded`는 attachment cutover 완료이지 첫 렌더 완료가 아니다. cutover 후 native
open/render 실패는 새 view의 failed 상태로 표시하며 이전 worker 자동 복구를 약속하지
않는다. cutover 이후 작업 cancel은 이미 완료한 전환을 되돌리지 않는다. End session은
새 controller를 닫으며 기존 worker까지 reap한 후 예약을 해제한다.

### 브라우저

모드 변경은 연결된 ready view에서만 가능하며 pending WS 입력/gesture/owner 작업 중에는
새 전환을 막는다. 변경 중 일반 뷰 입력도 제출하지 않는다. 새 snapshot에서 카메라와
level 선택기를 복원하고 이전 프레임/레이어 목록/스타일 편집 선택을 정리한다.
POST 응답 유실, 작업 조회 실패 또는 terminal 이후 view 조회 실패는 **읽기만 재조회**한다.
새 seq로 mutation을 자동 재전송하지 않는다. source 선택 메뉴가 다른 항목을 가리켜도
live mode 명령은 현재 view_id/revision만 보낸다. Ctrl+,는 canvas에만 적용하고
repeat/Shift/Alt/Meta/IME 입력은 받지 않는다.

### 검증과 남은 범위

- controller 집중 테스트4개: 준비/drop/stale·중복 준비, 종료 지연 시 비중첩,
  commit 직후 취소, 새 open 실패를 강제한다. workers1/CPU2/decoded1 제한에서
  추가 quota 없이 동작하고 종료 후 usage0을 확인한다.
- §44의 GTK 24전환 PNG 오라클을 실제 연속 controller 교체로 확장했다. workers1에서
  매번 이전 controller 종료·새 epoch·동일 quota와 GTK PNG 바이트를 함께 확인한다.
- owner HTTP native 테스트는 선택[1,2]/chip 한 개 숨김→level→layer→chip→layer→level
  5회 교체, raw의 독립 전체 숨김, 카메라/제어 보존, no-op/stale/replay/conflict,
  old layer URI 거부와 old WS 종료, 새 epoch PNG, logout/quota0을 확인한다.
  private 합성 source 두 개의 캐시 digest도 전후 동일하다.
- JS 게이트는 dropdown/Ctrl+,·다른 source picker·중복/대기 입력 차단·응답 유실·
  작업/view 조회 실패 후 read-only 복구·stale 전환을 전체 UI 드라이버에서 필수 실행한다.

CLI startup/single-instance 전체 이관·실칩 전환 지연/메모리·현장 Firefox/ETX 수용은
이 단계의 완료 조건에 포함하지 않는다. GTK 기본 launcher는 변경하지 않는다.

로컬 Chrome에서는 전용 합성 덱의 선택[1,2], 중심(103,117)/폭311µm에서 B 칩 하나만
숨기고 chip→level→chip→layer→chip→layer→chip으로 왕복했다. Ctrl+,와 선택기를 모두
사용했고 chip 숨김·raw 전체 숨김의 독립 복원, 카메라 유지, 정상 geometry 표시를
확인했다. 모드 선택기와 레이어 목록 screenshot도 확인했다. End session 후 Session ended와
서버 exit0을 확인하고 전용 탭을 닫았다. 기본값 게시·업로드·다운로드는 실행하지 않았다.

전체 `sh tools/validate_rust.sh`는 exit0, `RUST VALIDATION: ALL OK`다. core223·web60,
owner HTTP native11(새 모드5회 포함), GTK/native 모드24회, jobdeck80·renderer46,
KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 포함한다. Rust1.89에서도 core223·web60과
owner11·GTK/native24회가 통과했다. scoped rustfmt 및 app-core/web/app all-target
clippy `--no-deps -D warnings`도 통과했다. 마지막 안내 문구 정리 뒤 release bundle을
다시 빌드하고 전체 UI/launcher CLI smoke를 재실행했다. 기존 dependency·GTK/Pillow
경고는 별도이며 main의 기존 수정과 feature/jobdeck worktree는 보존했다.
로그는 `/private/tmp/floe-live-mode-battery.log`, `floe-live-mode-msrv.log`,
`floe-live-mode-msrv-owner.log`, `floe-live-mode-msrv-deck.log`,
`floe-live-mode-clippy.log`, `floe-live-mode-final-ui.log`, `floe-live-mode-final-cli.log`다.

## 46. M4g-6 — CLI 초기 표시·카메라와 잡덱 레벨 선택

기존 GTK `cmd_view`와 초기 `_after_allocate` 경로를 대조했다. 이 단계 전 웹 CLI는
일반 레이아웃도 full depth/frames off로 시작하고 goto의 폭이 필수였다. 초기 표시를
맞추되 GTK 기본 실행기·저수준 owner API의 빈 body 기본값은 변경하지 않는다.

- 일반 CLI open은 depth0, goto/DRC/잡덱은 full이다. 명시한 정수 depth는 GTK처럼
  0..999로 제한하며999 이상은 full이다. 정규 십진 문자열만 받는 wire와 달리 CLI는
  부호/선행0도 정규화한다. 매우 큰 입력은 정수 overflow 없이 처리한다.
- frames/labels 기본은 on이며 초기 frames off는 labels도 억제한다. 잡덱 labels는
  계속 미지원/false다. 기존 웹의 bare `--frames`/`--labels` 및 `--no-*` 별칭과
  GTK의 `--frames on|off`/`--labels on|off`를 모두 받는다. 이후 live 토글은 독립이다.
- `--goto X,Y[,W]`는 폭 생략 시 초기 fit 배율을 보존한다. live goto DTO에서도
  생략을 허용하고 null/비유한 값/0/음수 폭은 거부한다. interactive Fit은 GTK와
  같은5% 여백과 산술 순서를 사용한다. archival render/capture bbox는 바꾸지 않는다.
- `--refinement off`는 명시적으로 native round 환경값보다 우선한다.
  `--perf-baseline`은 argv 순서와 무관하게 frames/labels/refinement/frame-cache를 끈다.
  decoded cache/geometry cut은 유지한다. 웹의 live LOD 토글과 progressive on은
  미이관이므로 `--refinement on`을 조용히 무시하지 않고 거부한다.
- `--mode layer`를 초기 잡덱에도 허용한다. 잡덱 레벨을 명시하지 않으면 다중 레벨은
  선택 대기, 한 레벨은 바로 연다. `--level`이 환경보다 우선하고
  `FLOE_JOBDECK_LEVELS=all|ask|N,N...`을 지원한다. 잘못된 환경 목록은 GTK의 경고 후
  all로 폴스루하는 동작 대신 hard error다. 실수로 전체 덱을 여는 것을 피한다.
- startup API의 `confirm_levels`는 선택 대기 안내일 뿐 권한이 아니다. UI는 선택
  전 렌더/색인 요청을 보내지 않는다. 초기 body를 successful open까지 보관하여
  index-required 실패→사용자의 명시적 색인→Open 재시도에도 goto/depth 등을 유지한다.
  다른 source 선택 또는 성공한 open에서 폐기하며 다른 소스로 누출하지 않는다.
  실패 뒤 브라우저 새로고침까지 이 미완료 초기 제안을 보존하는 것은 아직 후속이다.

### 검증과 남은 범위

`validate_web_startup.py`는 GTK `cmd_view`의 초기 정책 prefix와 Viewer의 실제
frames/labels 대입·fit/goto/view_bbox AST를 실행한다. 빈 합성 경로만 읽고 GTK 창/
socket/index 실행 전에 멈춘다.144조합을 Rust CLI·DTO·Viewport 결과와 비교하며
일반/덱, goto2/3좌표, depth 경계, frames/labels, baseline, DRC, 종횡비를 교차한다.

별도 native8개 실행은 PATH가 빈 환경에서 전용 valmini 복사본·2레벨 덱을 쓴다.
선택 대기1건은 작업0, 나머지7건은 고정 viewport에서 submitted1/consumed1과 초기
옵션을 확인한다. round 환경을1로 설정해도 explicit off/baseline이 direct-final을
유지한다. 종료 후 session/temp 제거와 source/cache 바이트 불변도 확인한다.
전체 게이트에 필수 배선했다. JS 드라이버도 선택 대기·레벨 부분 선택·색인 실패 후
재시도·성공 뒤 다른 source로 옵션 비누출을 단언한다. 좌표 고정 query/pan fixture는
fit 정책과 독립적인 explicit viewport를 사용하며 기존 정확한 좌표 단언은 유지했다.

로컬 Chrome에서는3레벨 합성 덱을 CLI `(103,117,311)`, depth99/detail high,
frames/labels off로 실행했다. 자동 open 없이 선택 대기를 확인한 뒤 MASK-B만 열어
레이어1개·노란 geometry와 카메라/옵션 보존을 DOM·screenshot으로 확인했다.
End session 후 Session ended와 서버 exit0을 확인하고 전용 탭을 닫았다.
이 단계에서는 파일 게시/업로드/다운로드/clipboard를 실행하지 않았다.

현재 검증: core223·web61·app14, GTK144/native8, 전체 ES2017/UI, scoped rustfmt와
app-core/web/app all-target clippy 통과. Rust1.89에서도 관련 단위와 GTK144를 통과했다.
첫 전체 배터리는 기존 worker lifecycle의 query8개 직후 render가 송신 큐 Busy를
받아 실패했다. 별도 lifecycle 재실행14건은 통과했으며 첫 실패를 숨기지 않는다.
재실행에서는 native 수동 좌표 테스트가 fit 여백에서 생기는
`1.000000000003638` DBU를 정수 문자열 `1`로 요구하는 의존성을 발견했다.
해당 테스트만 이진수 정확 viewport로 분리해 raw 좌표와 snapped 좌표의 기존
정확 일치 단언을 유지했다. 제품 좌표의 반올림/허용 오차는 변경하지 않았다.
분리 뒤 native query/측정5건은 통과했다.
최종 전체 `sh tools/validate_rust.sh`는 exit0, `RUST VALIDATION: ALL OK`다.
owner HTTP native11, GTK/native 모드24회, jobdeck80·renderer46, KLayout13 PX+
2 phase-exact+14 style(jobs1/8)을 포함한다. 기존 dependency·GTK/Pillow 경고는
별도이며 main의 기존 수정과 feature/jobdeck worktree를 보존했다.
로그는 `/private/tmp/floe-startup-battery-final.log`, `floe-startup-msrv.log`,
`floe-startup-msrv-oracle.log`, `floe-startup-msrv-query.log`,
`floe-startup-clippy.log`다. 앞선 실패 로그 `floe-startup-battery.log`와
`floe-startup-battery-recheck.log`도 남겨 원인과 조치를 추적할 수 있다.

CLI 전체 완료는 아니다: single-instance/`--multi`/인자 없는 빈 창, 나머지
hairline/thin-um/debug/dump/stream/LOD 옵션 정책, 수동 source open 기본값 통합,
현장 Firefox/ETX와 Linux portable 실행 수용은 후속이다. M5 공유 권한·실칩 jobdeck
실측도 그대로 남는다. native 프로토콜/RENDERD_VERSION0.12.87은 변경하지 않는다.

## 47. M4g-7a — 로컬 단일 인스턴스 통신 기반

`app-core::instance`에 trusted launcher용 Unix socket rendezvous를 추가했다.
이 단계는 **제품 CLI/브라우저와 연결하지 않은 기반**이다. 기존 GTK와 웹 실행 동작은
그대로이며 forwarding/`--multi`/인자 없는 빈 창이 완성된 것으로 보지 않는다.

### 소유권·전달 계약

- 키는 product/effective UID/정규화 DISPLAY다. GTK 실제 함수처럼 screen suffix를
  제외하고 구분하며, DISPLAY 없는 macOS는 aqua/Linux는 headless 이름을 쓴다.
  SHA-1은 파일 이름용일 뿐 인증이 아니다. TeeBox의 공유 UID를 실사용자 ID로 취급하지 않는다.
- launcher가 선택한 로컬 부모 아래 owner 전용0700 디렉터리와0600 socket/lock을
  사용한다. 부적절한 소유권·권한·symlink·hard-linked lock은 거부하며 권한을 고치지 않는다.
  경로가 Unix socket 길이 제한을 넘으면 명시 오류다. 기본 registry 위치 선택은 CLI 통합 때 정한다.
- 실제 owner는 커널 flock을 유지한다. lock inode는 종료 시에도 남겨 동시 실행의
  분리 소유를 피한다. 소켓은 보관한 inode와 일치할 때만 정리한다. crash 복구도 lock을
  얻고 connect가 connection-refused일 때만 stale socket을 제거한다. timeout/권한 오류/
  응답 불명은 새 owner 실행이나 소켓 삭제의 허가가 아니다. 저장된 PID로 신호를 보내지 않는다.
- 연결 양쪽이 Linux SO_PEERCRED/macOS getpeereid로 같은 UID를 확인한다.
  protocol/build 불일치를 거부한다. 이는 협력하는 로컬 launcher와 다른 UID에 대한
  경계이지 악의적인 동일 UID/root의 파일 교체를 방어하는 sandbox가 아니다.
- 길이 prefix와 엄격한 JSON schema, 본문64KiB/응답16KiB, 직렬화 중 크기 제한을 둔다.
  I/O는 절대3초 기한과 취소 확인을 사용하므로 한 byte씩 보내도 기한이 연장되지 않는다.
  잘못된 framing은 handler 실행 없이 연결을 닫는다. 모든 오류에 구조화 응답을 보장하지 않는다.
  callback은 유계 작업 접수만 해야 하며 취소에 협력해야 한다. 동기 색인/렌더를 수행하면 안 된다.
- owner 시작마다 무작위 epoch를 새로 만들고, **연결 handshake마다** 고유 ticket을
  예약한다. 연결만 끊긴 요청과 뒤이어 내용이 같은 새 요청은 서로 다른 ticket이다.
  최근32개 **발급 ticket**의 본문/결과를 보관한다(성공한 작업32개가 아님).
  같은 intent의 재전송은 결과만 돌려주고, 다른 본문은 conflict, 보관 범위 밖은 expired다.
  재접속도 새 미사용 ticket을 소비한다. 이전 미처리 ticket은 유효 범위 안에서 늦게 처리될
  수 있으므로 실제 open 요청은 향후 source/context/revision도 검증해야 한다.
- 처리 결과를 ACK 전 보관한다. submit 도중 I/O 실패·잘못된 응답·취소는 결과 불명
  `Incomplete`다. 복구하려면 보관한 동일 intent를 사용하며 새 번호로 자동 재실행하지 않는다.
  재시작한 owner는 epoch가 달라 과거 intent를 거부한다. 영속적인 exactly-once 보장은 아니다.
  `Handled`는 callback 결과/접수 receipt이며 첫 프레임 완료 ACK가 아니다. callback 오류는
  고정 code만 전달하고 내부 경로가 포함될 수 있는 Error.message를 보내지 않는다.

### 실제 연결에 앞서 필요한 작업

현재 web service의 source 목록과 공유 기본값/DRC 게시 보호 source 목록은 시작 시 고정된다.
목록만 동적으로 바꾸면 이미 준비·승인한 게시 대상이 새 source/cache와 충돌할 수 있다.
따라서 다음 단계는 trusted launcher의 등록과 게시 보호 검사를 같은 수명/동기화 경계에
넣는 것이다. 브라우저가 임의 서버 경로를 등록하는 API로 대체하지 않는다. 그 뒤 CLI 전달
옵션의 원자적 적용·open receipt/실패·`--multi`·빈 창을 실제 UI에 연결한다. 새 등록만으로
색인/force/공유 게시를 승인하지 않으며 원래 사용자의 단계별 승인 의미를 유지한다.

### 검증

새 단위10개는 lost ACK/replay·동일 내용의 별도 요청·ticket 만료·충돌·restart epoch,
소유권/권한/교체 inode, 잘못된 길이/UTF-8/JSON/응답, 응답 상한·비공개 오류,
trickle 절대 기한·부분 입력 중 종료를 검사한다. ignored oracle는 기본 단위 결과와
분리하며 `validate_instance_key.py`가 GTK 실제 normalize_display의199입력을 비교한다.
이 gate를 전체 배터리에 필수 배선했다.

별도 native integration 실행 파일은 자신을 PATH 없는 자식으로 실행한다. 실제 두
프로세스의 동시 claim·동일 요청 replay·build 거부·테스트 소유 자식 SIGKILL 후 복구·
epoch 변경·정상 종료와 lock inode 유지, exec한 자식에 flock이 상속되지 않음을 검사한다.
합성 임시 파일만 사용하고 layout/cache/브라우저 게시를 하지 않는다. 다른 UID 계정 간
실행과 Linux 현장 runtime 수용은 이 macOS 검사로 대체하지 않는다.

검증: app-core233·web61·app14 단위, scoped rustfmt, app-core/web/app all-target
clippy를 통과했다. Rust1.89에서도 새 단위10개·native lifecycle·GTK199를 통과했고
x86_64-unknown-linux-musl all-target check가 성공했다(Linux 실행 확인은 아님).
전체 `sh tools/validate_rust.sh`는 첫 실행에서 exit0,
`RUST VALIDATION: ALL OK`다. 새 GTK199/native IPC와 기존 GTK startup144/native8,
owner/DRC/UI·잡덱80·렌더러46, KLayout13 PX+2 phase-exact+14 style(jobs1/8)을
포함한다. 기존 dependency/GTK/Pillow 경고는 별도이며 main 변경·feature/jobdeck은
보존했다. 검증 전용 `.venv` symlink만 종료 후 제거했다.
로그는 `/private/tmp/floe-instance-battery.log`, `floe-instance-msrv.log`,
`floe-instance-msrv-oracle.log`, `floe-instance-linux-check.log`,
`floe-instance-clippy.log`, `floe-instance-native.log`다.
native renderd wire/RENDERD_VERSION0.12.87은 불변이다. getrandom0.3.4/socket2 0.6.5는
이미 vendored된 의존성을 app-core에도 명시한 것이며 vendor 원본은 변경하지 않는다.

## 48. M4g-7b — 동적 소스 등록과 공유 게시 보호

`registered::SourceSet`과 trusted `Service::register_source`를 추가하고, 기존
기본값 publisher·DRC notes/waives의 managed store까지 같은 SourceSet을 연결했다.
시작 시 Vec를 복사해 보호 대상을 고정하던 경로를 없앴다. 기존 정적 native 생성 API는
독립 SourceSet을 감싸 호환하며, 공유 service 경로는 항상 같은 Arc를 전달한다.

### 등록·게시·카탈로그의 원자성

- 등록은 append-only, 최대32개다. 빈 service도 만들 수 있다. trusted caller의
  AccessScope로 원본 경로와 전체 덱 dependency를 검사한다. 같은 경로는 기존 source의
  변경 여부와 새 scope 내 dependency 포함을 확인하고 같은 Arc/opaque ID를 돌려준다.
  외부 변경은 자동 채택하지 않고 기존 계약대로 재등록 필요 오류다. 현재 살아 있는
  service에서 기존 source 교체/삭제·scope 확장을 구현한 것은 아니다.
- 등록 reservation은 prepare한 추가 목록을 숨긴다. 실패·취소·drop이면 버리고,
  commit 때만 보호 목록에 추가한다. service는 짧은 state/catalog lock 안에서 보호
  목록을 먼저 commit하고 handle을 공개하므로 새 ID가 보호 없이 보이는 순간이 없다.
  source header/dependency I/O는 그 mutex 밖의 trusted 호출 스레드에서 한다.
- 기본값·DRC 게시가 시작되면 SourceSet publication lease를 얻는다. lock/temp 생성
  전부터 최종 검사·commit·directory sync·실패 cleanup까지 유지하므로 그 사이 새
  등록은 Busy다. 등록 검사/commit 중 새 게시도 Busy다. 서로 다른 게시끼리는 기존
  sidecar flock/managed admission 정책을 유지한다. 짧은 mutex는 lease 수만 세고
  파일 I/O 중에는 잡지 않으므로 catalog 조회·기존 렌더를 막지 않는다.
- 등록이 먼저 끝나면 과거 draft도 최신 보호 목록을 검사한다. 새 source/TC/cache/
  index lock과 충돌하는 기본값·note·waive target 또는 게시 lock은 파일 생성 전에
  거부한다. 게시가 먼저 시작됐다면 등록자는 완료 뒤 **새로 검사**해야 한다. 이 단계는
  Busy를 자동 재시도하거나 사용자의 승인 의도를 새 요청으로 바꾸지 않는다.
- service 등록 중 새 open/index/mode 명령도 Busy로 거부해 색인 쓰기가 등록 검사를
  가로지르지 않게 한다. 이미 끝난 operation의 replay receipt는 유지한다. 등록 중
  source 목록 조회와 현재 view의 표시·pan은 계속 가능하며, 종료/취소는 commit 전에
  재확인한다. 등록만으로 source/cache/sidecar에 쓰거나 open/index를 실행하지 않는다.
- 이것은 같은 service의 협력하는 writer/registration 경계다. 다른 프로세스의
  비협력 파일 수정, 분산 cache revision 관리, 동일 UID의 악의적 교체에 대한 filesystem
  CAS나 sandbox를 새로 제공하지 않는다. 공유 실사용자 인증/M5 권한 결정도 별도다.

### 범위와 검증

새 HTTP endpoint는 없다. 기존 owner GET catalog가 추가된 ID를 반환할 뿐이며
브라우저는 path를 전송해 등록할 수 없다. IPC callback에서 동기 register를 호출하면
안 된다. 다음 단계에서 유계 launcher 작업으로 접수하고 off-reactor/off-socket
스레드에서 실행한 뒤 CLI 초기 옵션과 UI를 연결한다. `--multi`·인자 없는 CLI와
브라우저 빈 창의 파일 선택/대기 흐름은 아직 미완료다. GTK 기본 launcher도 유지한다.

새 단위5개는 등록 롤백·취소·재사용·상한·scope·동시 publication lease,32회 실제
thread race의 단일 승자, 기존 default/note/waive draft의 새 target/lock 보호,
staging 중 등록 거부와 작업 종료 후 lease 해제를 검사한다. 별도 native owner HTTP
gate는 빈 service→trusted 등록→native 첫 frame, 같은 ID 재사용·무암묵 색인,
미리 준비한 default의 승인 후 새 보호 대상 충돌, scope/cancel/종료 후 거부와
`POST catalog`405를 검사한다. source/cache 바이트와 자원 회수도 단언한다.

로컬 focused 결과는 app-core238·web61 단위와 owner HTTP12건, all-target clippy
통과다. Rust1.89에서도 app-core238·web61이 통과했고 관련3패키지의 Linux musl
all-target check가 성공했다(실제 Linux 실행 확인은 아님).1.89 clippy는 설치되지
않아 해당 시도는 도구 부재로 종료됐으며, 최종 clippy는 설치된 기본 도구체인으로
통과했다. 전체 `sh tools/validate_rust.sh`는 첫 실행에서 exit0,
`RUST VALIDATION: ALL OK`다. owner HTTP12건·GTK startup144/native8·IPC GTK199,
UI/DRC·잡덱80·렌더러46과 KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 포함한다.
기존 dependency/GTK/Pillow 경고는 별도이며 검증 전용 `.venv` symlink만 종료 후
제거했다. main의 기존 변경과 별도 feature/jobdeck 작업은 건드리지 않았다.
로그: `/private/tmp/floe-registration-battery.log`, `floe-registration-owner.log`,
`floe-registration-unit.log`, `floe-registration-clippy.log`,
`floe-registration-msrv.log`, `floe-registration-linux-check.log`.
1.89 clippy의 도구 부재 기록은 `floe-registration-msrv-clippy.log`다.
native renderd protocol/RENDERD_VERSION0.12.87과 vendor는 불변이다.

## 49. M4g-7c — 유계 전달 제안과 안전한 view 교체

기존 IPC와 source registration 사이에 놓일 `launch::Launches` 및 owner API를
추가했다. 이 커밋은 서버 경계까지다. 제품 CLI의 IPC callback/준비 작업자와
브라우저의 빈 창·레벨 선택/자동 open 소비자는 아직 연결하지 않는다. 기본 launcher의
동작, Python/GTK 경로 및 현장 수용 상태는 불변이다.

### 접수와 완료의 구분

- trusted launcher만 `reserve`로 임의 ID를 만들고, off-reactor 작업에서 등록을 마친
  뒤 `ready`/`failed`를 보낸다. 진행 중 제안은1개, 보관 이력은16개, 본문은16KiB다.
  접수한 제안 자체는 파일 열기/색인/첫 frame 완료가 아니다. 미처리 제안이 있으면
  새 reserve는 Busy다. 실패도 owner가 dismiss하기 전까지 표시할 대상으로 남긴다.
- trusted gateway에 명시 attach했을 때만 `launcher` capability가 켜진다.
  `GET /api/v1/launch`는 현재 제안, `GET /api/v1/launch/poll/{revision}`은 최대4초
  대기 후 최신 snapshot을 돌려준다. 대기2개 제한, 초과429, 종료/인증 만료 시 취소다.
  전역5초 handler deadline, query-string 금지, exact origin/host와 cookie+CSRF를
  완화하지 않는다. poll 반환 직전에도 인증을 검사한다.
- `POST /api/v1/launch/{id}`는 `open`/`present`/`dismiss`만 받는다. 브라우저는
  open의 실제 viewport pixels·레벨 선택·operation seq와 선택적으로 현재 view ID/
  state revision만 더한다. 임의 path/등록/색인/operation body는 받지 않는다.
  body·모드·source ID는 trusted 제안에서 온다. 오류는 안전한 코드만 반환한다.
- 접수 성공은 기존 owner operation ledger에 **한 번** 넣었다는 뜻이다. 같은 action
  재전송은 같은 receipt, 다른 seq/본문으로 재전송은 conflict, 이력 밖 ID는 expired다.
  원래 operation의 성공/실패와 frame 준비 여부는 기존 operation/view 채널로 읽는다.
  불명확한 HTTP 결과를 새 seq로 자동 재실행하면 안 된다. 실패한 open의 재시도도
  자동 신규 요청이 아니라 UI의 명시 동작으로 구분해야 한다.
- dismiss는 진행 중 등록 작업에 전달할 취소 flag를 올린다. 늦은 ready는 거부된다.
  이미 commit된 등록이나 이미 접수된 open을 되돌린다고 주장하지 않는다. 접수 후
  취소는 해당 operation의 기존 cancel 경로를 사용한다. `present`는 operation/
  render revision을 만들지 않는다. OS 창 focus는 이후 browser 연결/현장 수용 범위다.

### 여러 파일 등록과 view cutover

`Service::register_sources`는1..32개 파일을 한 SourceSet transaction으로 검사하고
commit한다. 뒤쪽 파일이 실패/취소되면 앞쪽 추가도 노출하지 않는다. 기존 파일 및
같은 배치 안의 중복 파일은 같은 Arc/ID다. source/cache/sidecar 쓰기나 암묵 색인은
없고, 앞 단계의 공유 게시 exclusion과 최신 보호 목록 계약을 유지한다.

전달 open은 일반 HTTP open과 달리 선택적인 기존 `(view_id,state_rev)`를 바인딩한다.
이 값도 operation replay signature에 포함한다. 현재 attachment/revision을 먼저
확인하며, 실제 적용 직전에도 다시 확인한다.

- 같은 source·모드·선택 레벨이면 기존 controller를 edit한다. 명시 patch만 적용하고
  renderer/decoded/retained cache를 소유한 worker와 epoch를 유지한다. 요청 body에서
  생략한 필드를 서버가 임의의 초기 기본값으로 덮지 않는다. 이후 CLI producer는
  GTK가 포워딩에 싣는 정규화된 detail/depth/frames/labels/font 기본값도 명시적으로
  구성해야 한다. `thin`의 미지정/명시 auto 구분과 파일 없는 present-only는 별도다.
- 다른 source/모드/레벨이면 metadata·초기 상태·attachment를 준비한 뒤 기존
  `PreparedReplacement`로 교체한다. 같은 worker reservation을 공유하는 dormant
  controller이며, 이전 native worker의 완전 close/drop/reap 뒤에만 다음 것을 연다.
  metadata/상태 검증 실패·stale·commit 전 취소는 기존 화면을 보존한다. commit 뒤
  새 native open 실패까지 기존 화면으로 롤백하는 계약은 아니다.
- revision이 바뀌거나 다른 창으로 전환됐다면 오래된 요청은 실패하고 새 화면을
  닫지 않는다. 기존 일반 `POST operations` open에는 강제 교체 권한을 추가하지 않았다.
  성공 receipt는 controller 접수/cutover이며, 첫 PNG가 도착했다는 ACK가 아니다.

### 검증 및 이어갈 범위

단위 gate는 pending1개·동시 reserve 단일 승자·취소/종료·bounded history/revision과
제안/동작 스키마를 검사한다. native owner gate는 빈 service의 배치 등록 rollback/
ID 재사용, 인증/경로 비노출, long-poll 제한과 조회 비차단, 같은 파일의 worker epoch
보존, 중복 action의 한 번 적용, stale/미색인 실패 시 기존 화면 보존, 단일 worker
예약으로 다른 파일 cutover, 과거 ID/receipt·dismiss·logout을 검사한다. 실제 합성
source와 cache의 바이트 불변/worker 임시 파일 회수는 기존 하네스가 함께 확인한다.

다음 단계는 CLI `--multi`/빈 실행·비동기 registration queue와 브라우저 소비자다.
기존 작업·pan이 진행 중일 때의 대기, 레벨 질문, ACK 불명확 시 읽기 전용 확인,
측정한 viewport를 합친 최초1회 open을 연결해야 한다. UI가 아직 소비하지 않는
기반 API를 single-instance 사용자 기능 완료로 보고하지 않는다.
이번 단계에는 브라우저 게시/다운로드/clipboard를 추가로 실행하지 않는다. 이전
합성 shared-default 게시 승인·결과는 §20 그대로 보존한다. 현장 Firefox/ETX 확인은
계속 보류이며 native renderd0.12.87/vendor도 불변이다.

검증 결과: web 단위66(신규5)·owner HTTP13(신규1) 통과, 비활성 launcher/API 인증
transport gate1개 추가. 설치된 기본 Rust의 web all-target clippy도 통과했다.
Rust1.89에서는 app-core238·web66 단위 및 관련3패키지 Linux musl all-target check가
통과했다(실제 Linux 실행은 아님). 전체 `sh tools/validate_rust.sh`는 exit0,
`RUST VALIDATION: ALL OK`로 완료됐으며 GTK startup144/native8, owner13,
VFS lifecycle/split·occupancy·DRC/SVRF·잡덱80·렌더러46과
KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 포함한다. 초기 개발 중 schema
테스트 기대값과 테스트 하네스 필드 누락을 고친 뒤 focused/전체 검증을 모두 통과했다.
기존 dependency/GTK/Pillow 경고는 별도다. 검증용 `.venv` symlink만 제거하며 원본
venv·합성 shared-default 게시 결과와 main/feature/jobdeck 작업은 보존한다.
로그: `/private/tmp/floe-launch-battery.log`, `floe-launch-owner.log`,
`floe-launch-unit.log`, `floe-launch-clippy.log`, `floe-launch-msrv.log`,
`floe-launch-linux-check.log`.

## 50. M4g-7d — 실제 CLI 전달과 빈 웹 창

§47–49의 소유권·등록·제안 경계를 제품 CLI와 브라우저에 연결한다. GTK `floe2`의
기본 실행기나 실측용 `feature/jobdeck`는 바꾸지 않는다. 웹 이관 전체 완료도 아니다.

### 실행과 접수

```sh
floe2-web                              # 빈 창 / 기존 창 present 요청
floe2-web view mask.oas --goto 10,20,700 --thin keep
floe2-web view other.oas --multi        # 기본 instance와 독립된 세션
floe2-web view --no-open                # 브라우저 없이 기본 owner, private session file 사용
```

기본 owner 키는 별도 제품명·UID·DISPLAY이며 DISPLAY 미지정도 지원한다. Firefox와
native binary를 찾기 **전에** 기존 owner를 확인한다. busy/버전 불일치/불명확한
ACK는 임의 새 창을 만들 권한이 아니다. CLI exit0은 `queued` 접수이며 소스 열기·
첫 frame 완료가 아니다. 같은 실행에서 등록하는 파일은 최대32개이며 세션 source
상한도 기존32개다. 가득 차면 새 독립 세션을 사용한다.

`--multi`, jobs/raster/budget/전송형식/frame-cache/refinement/baseline,
port/session-file/Firefox, DRC 관련 옵션을 명시하면 독립 세션으로 시작한다.
이미 열린 worker의 프로세스 설정을 조용히 무시하거나 바꾸지 않는다. `--no-open`
단독은 기본 owner가 될 수 있으며 이후 전달에서 기존 창 설정을 바꾸지는 않는다.
환경변수의 worker 설정도 기존 owner에 소급하지 않는다.

IPC는 절대 경로와 검증한 display argv, sender의 레벨 환경정책만 전달한다.
경로·정책 준비는 bounded 작업자에서 수행하며 socket callback/HTTP reactor에서
파일을 파싱하지 않는다. 등록 중에도 기존 렌더/취소는 독립적으로 동작한다.
브라우저에는 불투명한 source ID와 표시 옵션만 보낸다. --root 밖 의존성을 임의
승인하거나 source/cache/sidecar를 자동 생성하지 않는다. 종료 시 취소·join한다.

같은 소스의 전달은 GTK와 같이 depth/detail/frames/labels/font의 정규화 기본값도
명시한다. `thin`은 미지정과 명시 `auto`를 구분한다. 잡덱은 명시 `--level`이 sender의
`FLOE_JOBDECK_LEVELS`보다 우선하고, 기본 다중 레벨은 질문 후에만 연다. 동일
source/mode/levels는 worker/캐시를 보존하고 다른 요청은 §49의 안전 교체를 사용한다.

### 브라우저와 불명확한 결과

빈 창은 등록 소스가 없다는 안내와 비활성 Open/Index를 보인다. 파일 선택기는 아직
없으며 별도 터미널의 `floe2-web view FILE`로 추가한다. 독립 빈 창은 forwarding 대상이
아니므로 FILE을 지정해 다시 시작하도록 안내한다. bare FILE shorthand도 아직 없다.

제안 소비자는 기존 operation·pan/입력 ACK·초기 레벨 질문이 끝날 때까지 기다린다.
실제 device viewport와 현재 view ID/revision을 합쳐 한 번 제출한다. 준비된 레벨
선택은 reconnect/BFCache의 live view 복원으로 덮지 않는다. 앞 요청 완료와 다음
제안 도착의 경합, 늦은 poll snapshot의 이미 처리한 ID 재생도 차단한다.

HTTP mutation 전에 sessionStorage에 ID와 전체 action을 보관한다.
`GET /api/v1/launch/{id}`는 owner 인증 아래 receipt만 조회한다. 네트워크 오류·reload는
읽기만 재개하며 자동으로 새 seq를 만들지 않는다. 명시 Check 버튼만 원래 action을
재전송할 수 있고, 결과가 불명확한 동안 Dismiss로 원 요청 기록을 덮어쓰지 않는다.
receipt 이력이 만료/손상되면 operation 상태를 확인하고 세션을 다시 시작해야 한다.
자동 신규 open이나 성공으로의 추정은 없다. 정상 종료/인증 만료/pagehide는 poll을
중단한다. present는 렌더를 요청하지 않으며 OS foreground 성공을 ACK로 주장하지 않는다.

로컬 IPC의 dirty-build 장벽은 web bundle에 launcher/app-core Rust 소스와 Cargo
manifest/lock을 포함해 만든다. 콘텐츠 불일치 검출이지 실행파일 서명/사용자 인증은 아니다.
native renderd protocol/RENDERD_VERSION0.12.87과 vendor는 불변이다.

### 검증

- Rust app17·web66 단위 및 all-target clippy 통과. Rust1.89 app-core238·web66 단위와
  관련3패키지 Linux musl all-target check 통과(실제 Linux 실행 수용은 아님).
- `validate_web_handoff.py`: 빈 owner→다중 CLI→native frame, queue Busy, 첫 표시
  viewport·옵션, 같은 view/worker epoch 유지, present 무렌더, 미색인 실패 시 기존 화면
  유지, 다른 파일 교체, 별도 세션, receipt 재조회/재전송, source/cache 불변·종료 회수.
  잡덱의 질문·sender 정책·명시 --level 우선순위도 합성2레벨로 검사한다.
- ES2017 gate와 실제 app.js/launcher 통합: 초기 빈 창, 카탈로그 갱신, 한 번 측정한
  open, 선택/재연결 보존, 저장 후 전송, ACK 유실·같은 요청 복구, 늦은 poll/연속 요청.
- Chrome 합성 확인: 빈 창→CLI 소스 등록→미색인 오류(암묵 색인 없음)→별도 합성
  cache를 명시 생성→CLI goto/high/keep/full 실제 그림→정상 종료. 이전 소스 오류
  문구 잔류를 발견해 수정·회귀 고정했다. 재빌드 CLI의 구버전 owner 거부도 확인했다.

전체 배터리 첫 실행은 기존 `worker-client/tests/lifecycle.rs`의 query 포화 직후
render에서 queue Busy로 중단됐다. 해당 lifecycle14개는 단독 재검증에서 모두 통과했다.
경쟁 빌드를 끝낸 상태의 전체 재실행은 exit0, `RUST VALIDATION: ALL OK`로 완료했다.
workspace 단위·owner13·GTK startup144/native8·CLI handoff·ES2017/UI·잡덱80·
렌더러46 및 KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 포함한다. 기존 테스트의
타이밍 조건을 완화하거나 worker 정책을 바꾸지는 않았다. 기존 dependency/GTK/Pillow
경고는 별도다. 검증용 `.venv` 링크만 제거했고 main/feature/jobdeck 변경은 보존했다.
최종 release의 Chrome에서도 미색인 오류→정상 소스 전달 시 오류 해제와 실제 그림,
동일 파일 재방문의 좌표/배율·기본 medium·명시 thin auto 적용·정상 종료를 확인했다.
중간 재확인용 세션1개는 연결 전에 bootstrap 기한이 끝나 정상 자동 종료됐으며,
새 세션에서 다시 확인했다. 실제 게시/다운로드/clipboard는 추가 실행하지 않았다.
로그: `/private/tmp/floe-forward-{unit,clippy,msrv,linux-check,ui,native}.log`,
`floe-forward-battery.log`, `floe-forward-worker-recheck.log`,
`floe-forward-battery-retry.log`.

## 51. M4g-8 — 허가된 서버 폴더 파일 선택기

[사용법/제약](WEBUI_FILE_PICKER.ko.md). 빈 창에서 Browse server files로 파일을
선택할 수 있다. GTK 실행기나 실칩 `feature/jobdeck`를 변경하지 않았다. 별도
`--multi` 창도 파일 선택을 지원하되 CLI single-instance owner가 되지는 않는다.
capabilities의 `file_picker`와 `launcher`는 그 차이를 따로 표시한다.

### 탐색과 선택 경계

- `app-core::browse`: trusted root를 고정하고 불투명 handle만 발급한다. root에서
  각 ancestor를 descriptor-relative/non-following으로 다시 열어 inode를 검사한다.
  디렉터리/파일 교체, symlink 탈출, 손상/만료 handle을 정상 빈 결과로 처리하지 않는다.
  선택한 regular file의 inode·size·mtime·ctime를 등록 전후 검사한다. 이후 native 경로
  접근 전체를 불변 revision으로 고정하는 OS sandbox라는 뜻은 아니다.
- 폴더 우선·대소문자 무시 정렬, dot/cache/ICE 숨김, OASIS/jobdeck/All files 필터,
  부분 문자열 검색과 128행 페이지다. 디렉터리를 한 번 조사/정렬하고 재사용한다.
  최대100k matches/1M examined, handle2,048, 깊이64/경로4,096 bytes다. 한도 초과는
  명시 오류이며 잘린 목록을 완전한 목록으로 표시하지 않는다. symlink/special과
  비UTF8/metadata 실패 항목은 각 건수를 표시한다. APFS의 비UTF8 이름 생성 거부와
  Linux의 허용 차이를 합성 gate가 구분한다.
- roots는 초기 소스 부모 + `--root`, 둘 다 없으면 실행 폴더다. home/`/` 전체로
  확장하거나 CLI 전달 때 새 탐색 root를 부여하지 않는다. 추가 scope는 새 세션의
  명시 `--root`로만 정한다. TC 의존성 등록도 같은 허가 범위 안이어야 한다.

### 비동기 API와 기존 열기 재사용

전용 단일 actor/active1개가 파일 읽기를 담당한다. HTTP는 seq/요청을 유계 ledger에
접수하고 즉시202를 반환한다. `GET /browse`는 roots와 cursor만, `GET /browse/{seq}`는
최대32개 이력 중 하나만 조회하므로 매 poll에서 전체 디렉터리 이력을 복제하지 않는다.
응답은 요청 본문도 포함해 연결 간 seq 충돌을 판별한다. 경로·파일 내용·argv를 응답하지
않으며 브라우저 파일 upload endpoint를 추가하지 않았다.

`select`는 §48의 원자적/게시 보호 등록을 거쳐 §49의 열기 제안 ID를 만든다.
그 단계에서 색인·native worker·설정 파일 쓰기를 실행하지 않는다. 제안 consumer가
실제 viewport와 현재 view/revision으로 열기를 접수한다. 같은 source는 worker/캐시를
재사용하며, 다른 source는 기존 교체/취소 경계를 따른다. 미색인 파일은 기존 화면을
보존하고 별도 Index 조작을 안내한다. 덱2레벨 이상은 선택 확인 후 연다.

취소와 제안 게시를 같은 actor state 잠금으로 직렬화한다. 취소가 늦으면 cancelled로
꾸미지 않고 성공 receipt/launch ID를 반환한다. 그 이후의 취소는 제안 Dismiss다.
등록 metadata만 카탈로그에 남을 수 있지만 소스·캐시에 쓰지는 않는다. 네트워크
유실 전 sessionStorage에 요청을 저장하며 재개는 조회만, 명시 Check만 동일 요청을
재전송한다. pagehide 늦은 응답·만료 이력·다른 요청의 같은 seq를 새 선택으로 적용하지
않는다. record가 손상/충돌하면 새 세션이 필요한 현재 복구 한계를 표시한다.

자원 admission은 1 CPU slot + 192 MiB다(정렬 임시 키/handle/응답 이력 포함 추정;
강제 RSS 상한 아님). 다른 작업과 총16 slots 안에서 나눈다. NFS blocked syscall은
취소 flag로 중단할 수 없으며 이 부분의 현장 지연/종료 보장은 아직 없다.

### 검증과 남은 parity

- core10개: 정렬/필터/숨김·pagination·reader offset 독립·handle/breadcrumb 만료,
  root/ancestor symlink 교체·변경 파일·FIFO·취소/한도·깊이·경로 없는 DTO.
- actor4개: 동일 요청 재접수/seq 충돌/이력 만료, 등록 후 무색인·무렌더, 변경 소스,
  취소·종료·자원 회수·HTTP용 strict DTO.
- `validate_web_browse.py`: private 합성 OASIS/덱으로 auth/두 root/paging·숨김,
  원본 변경 거부, 최초 열기·동일 worker 재사용·다른 파일 교체, 미색인 실패 시 기존
  화면 보존, 덱 레벨 질문, source/cache digest 불변·정상 종료를 검사한다.
- ES2017 및 전체 UI gate 통과. 별도 picker gate는 literal filename, 페이지 이동,
  저장 후 전송·읽기 전용 복구·동일 요청 재전송·seq 충돌·취소·늦은 응답을 고정한다.
- Rust1.89 app-core248(web와 무관한 기존 ignored3개)·web70 통과, 3패키지 Linux
  musl all-target check 및 호스트 all-target clippy 통과. renderer ABI/버전0.12.87 불변.
- Chrome 실제 확인: 빈 창 목록, 미색인 선택의 명시 오류/Index 안내, 정상 파일로
  전환 시 오류 해제·실제 full-depth 픽셀, 하위 폴더 빈 목록과 기존 렌더 유지.
  이름 검색으로 정확히 한 파일을 반환하고 End session 뒤 exit0·session file 제거·
  worker 디렉터리 비움을 확인했다. 테스트 탭도 닫았다.
  설정 게시/다운로드/clipboard는 추가 수행하지 않았다.

현재 새 선택은 CLI 기본 표시값을 사용한다. GTK의 마지막 표시 옵션 유지, GDS/gzip,
임의 home/path 탐색, 색인 동의→자동 재열기 UX·bare FILE/나머지 CLI 옵션과 현장
Firefox/ETX 수용은 남아 있다. 파일 선택기를 전체 웹 이관 완료로 계산하지 않는다.
전체 배터리는 exit0, `RUST VALIDATION: ALL OK`로 완료했다. workspace 단위·새 picker
gate·owner/CLI 전달·GTK startup144/native8·ES2017/UI·잡덱80·렌더러46 및
KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 포함한다. 기존 dependency/GTK/Pillow
경고는 별도다. 검증용 `.venv` 링크만 제거했고 main/feature/jobdeck 변경은 보존했다.
전체 로그: `/private/tmp/floe-browse-battery.log`.
집중 gate 로그: `floe-browse-{unit,api-unit,clippy,ui,msrv,linux-check,native}.log`.

## 52. M4g-8b — 파일 선택 시 창 표시 설정 유지

GTK `open_file`/`_apply_cache`를 기준으로 파일 메뉴의 표시 정책을 복원했다.
기존 chooser가 각 선택을 CLI 기본값으로 바꾸던 차이를 제거하며 자동 색인은 추가하지 않는다.

- open DTO `display_policy`: 생략/explicit은 기존 API·CLI 의미 그대로, window는
  파일 메뉴/수동 재열기다. picker 목록 조사 시점이나 DOM의 낡은 값을 복사하지 않는다.
  owner 작업에서 controller state와 CAS revision을 **한 snapshot**으로 읽고, 새 모델의
  초기 상태에 depth/detail/thin/frames/labels/font를 적용한 뒤 첫 render를 시작한다.
- 다른 소스는 fit 및 mono/layer/style 초기화, 같은 소스/모드/레벨은 기존 controller의
  edit 경로로 worker/decoded/retained cache·카메라·depth·mono·레이어 상태를 보존한다.
  같은 잡덱 재선택도 사용자가 고른 shallow depth를 full로 되돌리지 않는다.
- 새 잡덱은 full depth, 실제 labels=false다. window의 원래 레이아웃 라벨 선호는
  따로 기억하므로 layout→deck→layout에서 on/off 모두 보존한다. thin은 resolved
  keep/cull이 아니라 auto까지 원래 정책을 복사한다. Close는 세션 선호를 지우지 않는다.
- CLI가 보내는 `label_preference`는 capability로 가리기 전 frames∩labels 값이다.
  기본 덱 labels=false와 명시 `--labels off`를 혼동하지 않는다. 재접수/색인 후 수동
  재열기에서도 이 값과 display_policy를 유지한다. 빈 창 `--perf-baseline`은 trusted
  초기 설정으로 seed된다. 기존에는 빈 창 baseline도 거부했지만 이제 독립 빈 창에
  한해 허용한다(process 옵션이므로 기존 owner에 present만 전달하지 않는다). 나머지
  source 필수 표시/DRC/레벨 옵션 제약은 그대로다.
- remembered preference는 전환 commit/동일 뷰 edit 성공 후만 갱신한다. metadata 준비
  실패·취소·stale CAS는 이전 view와 선호를 유지한다. 신규 세션에 영구 저장하지 않으며
  layerprops나 공유 기본값 파일을 읽는 기존 정책/쓰기 권한도 바꾸지 않는다.

검증 범위:

1. `validate_web_file_display.py`: 실제 GTK open 함수와 depth setter, cache 교체의
   mono reset을 실행한다(cache I/O만 fake). cache 함수가 표시 선호를 직접 덮어쓰는지
   별도로 단언하고 Rust 정책과 1,296개 조합을 비교한다. 전체 배터리에 필수 배선했다.
2. owner native gate: 첫 generation1/state_rev1에서 설정 확인, 동일 파일 worker/카메라/
   mono/레이어 유지, 다른 파일 fit/reset, 잡덱 왕복 labels on/off·auto thin, Close 뒤
   재열기, CLI label 선호와 동일 덱 shallow depth를 검사한다. 이전 stale/cancel/실패/
   한번만 접수 gate도 그대로 실행한다.
3. 실제 CLI chooser gate는 빈 창 baseline의 첫 파일 frames/labels off를 확인한다.
   CLI startup/handoff 및 실제 app.js harness는 전달 기본값·원래 요청 재열기·다른
   소스로 설정이 누출되지 않는 기존 조건에 display_policy/label_preference를 더한다.
4. Rust1.89에서 app 17/web 72 단위 테스트(각 oracle 1개는 전용 gate에서 실행),
   Linux musl all-target check, 최종 host clippy와 ES2017/UI를 통과했다.
5. 실제 Chrome에서 합성 first.oas의 depth7/high/auto·frames on·labels off·font23을
   확인하고 Mono를 켠 뒤 picker로 second.oas를 열었다. 표시 선호는 유지되고 Mono는
   해제되며 두 번째 파일의 실제 컬러 geometry가 표시됨을 확인했다. End session 뒤
   exit0·session file 제거·worker 디렉터리 비움과 테스트 탭 종료도 확인했다.
   첫 실행은 브라우저 연결 전에 bootstrap이 만료되어 정상 종료됐으며 새 private
   세션으로 검증했다. 공유 파일 게시·업로드·다운로드·clipboard는 수행하지 않았다.

전체 배터리 첫 실행은 초기 legacy 다중 worker oracle에서 진행 없이 대기했다.
해당 검증 프로세스만 SIGINT로 종료하고 자식 프로세스 정리를 확인했다. 같은 private
valmini에 `floe index --legacy --jobs 1`로 oracle을 5초 만에 생성한 뒤 배터리를
재실행했다. 제품/legacy 코드는 이 우회를 위해 변경하지 않았다. 전체 배터리는
exit0, `RUST VALIDATION: ALL OK`로 완료했다. workspace 단위·owner 14·GTK file
display 1,296·startup144/native8·picker/handoff·ES2017/UI·잡덱80·렌더러46과
KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 포함한다. 기존 dependency/GTK/Pillow
경고는 별도다. 로그: `/private/tmp/floe-window-battery-retry.log`,
집중 검사: `/private/tmp/floe-window-*.log`.

다음은 미색인 파일의 단일 동의→색인→자동 재열기다. 파일 선택 자체로 색인·force·
공유 게시를 승인한 것으로 해석하지 않는다. 현장 Firefox/ETX 수용은 여전히 미검증이며
이 단계로 GTK 폐기나 전체 웹 이관 완료를 선언하지 않는다.

## 53. M4g-9a — 승인된 색인→재열기 서버 작업

미색인 파일을 선택한 뒤 별도 index와 open을 브라우저가 두 번 제출하는 대신,
원래 실패 open을 서버가 보존하고 **명시 승인된 하나의 owner 작업**으로 처리한다.
이번 단계는 서버/API와 native gate다. 브라우저 승인 창·복구·새 뷰 연결은 후속이며
현재 파일 선택이나 reload가 이 작업을 암묵 제출하지 않는다.

### 원래 요청과 승인 범위

- 열기 실패 중 선택한 소스의 실제 OASIS header와 캐시를 읽기 전용으로 재검사한다.
  누락된 meta는 기존 reader에서 I/O, 전부 미색인인 덱은 invalid input으로도 반환돼
  오류 종류만으로 판정하지 않는다. 현재 등록을 재검증하고 선택 TC의 read lease를
  취한 뒤 fresh cache가 없는 정상 OASIS에만 `index_unavailable`과 `index_open`
  제안을 준다. changed source, 없는 원본, 선택하지 않은 TC의 미색인, 현재 캐시의
  없는 레이어는 색인으로 고칠 수 있다고 제안하지 않는다. HTTP reactor에서 I/O하지
  않으며, 성공한 열기는 이 error-only 검사를 추가로 하지 않는다.
- 서버는 source ID/Arc, 레벨 집합, mode, patch, display_policy와 raw label 선호를
  최대32개 보관한다. 원래 open의 ledger가 만료되면 참조도 거부한다. 공개 이력에는
  source/title/mode·선택 방식/개수의 유계 요약만 실으며 전체 레벨 목록을 매번 복제하지
  않는다. 향후 UI의 승인 preview는 **원래 선택**을 보여야 하며 현재 DOM 선택으로
  대체하면 안 된다.
- 새 `index_open` DTO는 원래 `open_seq`, 승인 `approved:true`, 현재 target, viewport
  pixels, options를 필수로 받는다. target은 empty 또는 view ID+state_rev다. source,
  레벨/모드/표시 옵션은 서버 보관본을 사용한다. 기본 jobs12·상한16, LOD/occupancy/
  force 의미는 기존 index와 같다. `force` 기본false이며 일반 승인만으로 기존 캐시를
  덮어쓰지 않는다. native 경로나 출력 경로를 브라우저에서 받지 않는다.
- caller-generated `request_id`64자리 소문자 hex를 queued/진행/완료 응답에 보존한다.
  seq만 같은 다른 작업의 결과를 브라우저가 자기 승인 결과로 채택하지 않도록 한다.
  기존 typed signature+high-water ledger로 동일 승인 재접수는 재실행하지 않고 다른
  본문은 충돌, 만료된 seq는410이다. 이 기반만으로 브라우저 저장/복구가 구현된 것은 아니다.

### 두 commit 지점과 자원

- 색인 전 target revision을 검사한 뒤 기존 ManagedIndex를 사용한다. 선택 레벨만
  실제 색인하지만 writer/CPU 예약 범위는 기존 all-source 정책을 유지한다. 활성
  reader나 CPU 예약과 충돌하면 busy이며 기존 view를 먼저 닫거나 jobs를 몰래 줄이지
  않는다. index jobs와 viewer의 총 부하는 여전히 기존 Resources admission을 따른다.
- 취소는 native 작업 취소/회수를 기다린 뒤 terminal을 게시한다. incomplete/failed/
  cancelled 색인은 자동 열기를 하지 않는다. 덱의 부분 색인 결과도 전체 성공으로
  바꾸지 않는다. 이미 만들어진 캐시를 롤백/삭제한다고 주장하지 않는다.
- 색인 전체 성공 후 같은 open 경로를 호출한다. 그 시점의 target revision과 최종
  교체 commit을 다시 확인하므로 색인 중 live pan/옵션 변경·Close는 낡은 승인으로
  덮어쓰지 않는다. 색인이 성공했어도 open이 실패/취소되면 `stage:open`, top-level
  실패/취소와 **index.phase:succeeded**가 함께 남는다.
- 원래 CLI의 goto/depth/detail/thin/frames/labels/font는 첫 프레임 전에 적용한다.
  window 정책은 정상 open과 같은 단일 snapshot에서 선호와 CAS를 읽는다. 성공은
  attachment 교체 완료이지 실제 표시 완료가 아니며 이후 native 렌더 실패는 view의
  별도 상태다. renderer ABI/버전0.12.87·인덱스 형식·GTK 경로는 변경하지 않았다.

### 검증 및 남은 작업

owner native gate17개가 통과했다. 새3개는 private valmini 복사본으로 다음을 검사한다.

1. 무승인/잘못된 pixels의 무쓰기, 실제 색인, 첫 generation1/state_rev1에서 초기
   표시·goto 보존, 동일 요청 재접수/다른 force 충돌, 손상 캐시 sentinel의 비파괴 거부와
   별도 force 성공, 원본 변경 시 제안 없음, 오래된 open 참조 만료.
2. 덱 선택 레벨의 LOD 색인과 미선택 파일 무쓰기, chip 모드/레벨·첫 generation 보존,
   missing TC 포함 partial의 incomplete/무열기, 없는 레이어/원본의 잘못된 색인 제안 방지.
3. 실제 native 호출 직전 test-owned wrapper를 일시 정지해 색인 전 stale CAS 무쓰기,
   색인 후 stale CAS의 캐시 성공/이전 뷰 보존, 명시 재시도의 cache 재사용, 취소와
   자식 reap·request ID/replay, 활성 캐시 reader와 force writer 충돌을 확인한다.

`tools/validate_owner_service.py`의 필수 marker로 배선했으며 native 실행의 PATH는
비워 Python/KLayout을 런타임으로 호출하지 않는다. Python은 gate/oracle 역할뿐이다.
후속 M4g-9b는 승인 preview·승인 요청의 전송 전 저장·읽기 전용 복구·동일 요청만
명시 재시도·작업 성공 후 새 뷰 채택·부분 결과 표시와 실제 브라우저 검증이다.
그 뒤 bare FILE/잔여 CLI·형식 parity와 G1/G4 감사가 남으며 공유 권한/원격 배포,
Firefox/ETX 현장 수용, 조건부 world-tile을 이 단계의 완료에 포함하지 않는다.

대상 app/web all-target clippy와 Rust1.89 단위(app17/web73), 관련3패키지의 Linux
musl all-target check를 통과했다. Linux 검사는 컴파일 확인이지 현장 실행 수용이
아니다. 전체 `sh tools/validate_rust.sh`는 exit0, `RUST VALIDATION: ALL OK`로
완료했다. owner17·GTK file display1,296·CLI/picker/handoff·ES2017/UI·occupancy25·
잡덱80·렌더러46과 KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 포함한다.
기존 dependency/GTK/Pillow 경고는 별도다. UI는 이번 단계에서 변경하지 않았고 실제
브라우저 승인 조작을 검증했다고 주장하지 않는다. 이전 private valmini/legacy oracle을
재사용했으며 검증용 `.venv` 링크만 제거했다. main/feature/jobdeck 작업은 수정하지 않았다.
전체 로그: `/private/tmp/floe-index-open-battery.log`. 집중 검사 로그는
`floe-index-open-owner-v6.log`, `floe-index-open-clippy-final.log`,
`floe-index-open-msrv.log`, `floe-index-open-linux-check.log`다.

## 54. M4g-9b — 승인된 색인→재열기 UI와 동일 요청 복구

이 단계는 §53의 서버 작업에 실제 브라우저 동의를 연결한다. 기존 독립 Index 조작도
유지한다. 파일 선택·preview·Close·새로고침은 색인 승인이 아니다.

- `Index and open…`은 재시도 가능한 실패 open에만 나타난다. 인증된 단일 preview는
  원래 source ID/이름·모드·전체 선택 레벨·표시 정책을 보여 준다. 현재 DOM의 다른
  소스/레벨로 바꿔 보내지 않으며 서버가 저장한 원래 요청이 권위다.
- jobs는 현재 자원 잔여의 읽기 전용 안내로 제안하고 최종 입력 그대로 제출한다.
  자동 축소/확대하지 않는다. cache reader 충돌은 별개이며 창을 암묵 닫지 않는다.
  LOD·occupancy·force는 매 새 preview에서 off, force는 별도 체크다. Close가 기본
  포커스이며 키보드 focus trap/Escape/IME를 처리한다. 문자열은 textContent다.
- 클릭 후 seq·crypto request ID·원래 open_seq·현재 view/revision target·pixels·
  옵션 전체를 세션별 sessionStorage에 **POST보다 먼저** 저장한다. 저장 실패이면
  제출하지 않는다. 조회 결과의 identity 불일치/만료/손상은 fail closed다.
- reload·visibility 복귀·BFCache는 GET만으로 복구한다. 결과 불명확 시 Check / retry는
  같은 승인만 전송하며 새 seq를 만들지 않는다. 취소도 원래 요청 identity 확인 후만
  수행한다. 아직 접수 여부가 불명확하면 late POST를 취소했다고 주장하지 않으며
  Close→End session 경로를 안내한다. 일반 Cancel은 승인 복구 중 차단한다.
- Close는 pending 승인 기록을 잊지 않는다. 작업 중 화면/source/설정 변경은 잠그되
  End session은 접근할 수 있다. hide/pagehide는 XHR 관찰만 중단하고 네이티브 작업을
  취소하지 않는다. 승인 기록은 terminal 결과의 UI 반영까지 성공한 뒤 지운다.
- 색인 실패/부분 덱은 자동 열기를 하지 않는다. 색인 성공 후 open 실패/취소는 완성된
  캐시가 남는다고 구별해 표시한다. 성공은 새 view 부착이며 픽셀 렌더 상태와 별개다.
  성공 채택 후 기존 미색인 경고를 지운다. reload가 force/게시 승인으로 확대되지 않는다.

집중 검증:

1. ES2017/독립 UI: 원래 선택·별도 force·자원 안내·전송 전 저장·저장 실패·read-only
   reload·동일 재시도·identity 충돌·불명확한 취소·만료·부분 결과·UI 채택 실패·late
   응답·키보드. 실제 app.js harness는 startup 실패→승인→응답 유실→BFCache 복구→
   새 픽셀 표시, POST 중복 없음, 일반 취소/뷰 변경 차단, 오래된 경고 해제를 검사한다.
2. owner native17: preview의 인증/no-write/선택 레벨·open_seq echo를 더했으며
   첫 generation의 원래 설정·선택 덱 LOD·미선택 소스 불변·부분 덱·force·CAS·취소/
   reap·이력 만료를 계속 검사한다. Rust1.89 app-core249/web73 단위, 대상 clippy와
   Linux musl all-target check도 통과했다. Linux 컴파일은 현장 실행 수용이 아니다.
3. 실제 Chrome에서 새 private valmini 복사본의 실패 open→preview→Close 후 cache
   미생성을 확인했다. jobs2/LOD on/force off로 승인하고 depth7/high/keep/frames on/
   labels off/font23/goto(5,6,300)와 실제 컬러 geometry를 확인했다. End session 뒤
   서버 exit0·session file 제거·worker 디렉터리 비움을 확인했다. 공유 기본값 게시,
   사용자 파일 upload·download·clipboard·실칩 데이터는 사용하지 않았다.

전체 `sh tools/validate_rust.sh`는 exit0, `RUST VALIDATION: ALL OK`로 완료했다.
owner17·GTK file display1,296·CLI/picker/handoff·ES2017/UI·occupancy25·잡덱80·
렌더러46과 KLayout13 PX+2 phase-exact+14 style(jobs1/8)을 포함한다. 배터리 시작 뒤
추가한 JS의 이전 경고 해제/일반 취소 가드는 별도 전체 UI 회귀를 재실행했고, 최종
release를 재빌드해 두 번째 합성 파일로 실제 Chrome 승인→geometry/원래 표시 옵션→
경고 해제→reload 복원을 확인했다. reload 전후 OVM/OVP SHA256도 같았다. 두 번째
세션 역시 End session 후 exit0·session 파일/worker 임시 파일 정리와 테스트 탭 종료를
확인했다. 브라우저의 **진행 중** 응답 유실/취소는 결정적 harness/native gate로
검사한 것이며 실제 장시간 브라우저 장애 주입 수용으로 확대하지 않는다.

이번 reload 관찰에서는 실제 viewport와 depth/detail은 복원됐지만 goto 입력칸은
HTML 초기값(0,0,700)으로 돌아왔다(실제 뷰 폭300 유지). 입력 초안/현재 뷰 동기화는
후속 CLI·입력 parity 감사의 열린 항목으로 남긴다. 전체 GTK 동일 복원 완료는 아니다.
전체 로그: `/private/tmp/floe-index-open-ui-battery.log`. 집중 로그는
`/private/tmp/floe-index-open-ui-{owner-v2,msrv,linux,clippy,final}.log`다.
Firefox/ETX 현장·장시간 실칩·공유 서버 수용은 여전히 별도다. 다음 구현은 bare FILE와
M0의 잔여 CLI/파일 형식/조작 parity이며 G1/G4 전체 감사와 GTK 은퇴 판정도 남아 있다.

## 55. M4g-10 — view 생략 실행과 현재 뷰의 goto 입력 복원

§54에서 남긴 입력 복원과 M0의 bare FILE를 처리한다. 기존 GTK launcher를 바꾸거나
웹 전환 완료를 선언하는 단계가 아니다.

- `floe2-web FILE [OPTIONS]`와 `floe2-web [OPTIONS] FILE`는 명시 `view`와 같은
  파서·등록·단일 인스턴스 전달·시작 정책을 사용한다. parser에서 파일 존재나
  확장자를 보고 분기하지 않는다. OASIS/잡덱의 실제 형식/범위 검증은 기존 단계다.
- 알려진 명령(index/render/selfcheck 등)이 우선한다. 이름이 겹치는 파일은
  `./index` 또는 `-- index`, 대시로 시작하는 파일은 옵션 뒤 `-- -mask`로 지정한다.
  글로벌 help/version은 단독이어야 한다. 미이관/잘못된 옵션은 계속 오류이며,
  누락 소스의 등록 실패가 자동 색인·변환이나 다른 프로그램 실행으로 바뀌지 않는다.
- 서버 snapshot은 현재 viewport의 µm 중심·폭을 `camera_um` 문자열로 제공한다.
  브라우저는 현재 view/epoch/revision이 맞는 값을 추가 반올림 없이 표시한다.
  매우 작은/큰 유한값은 필요한 경우 scientific notation을 사용한다. µm 값이
  표현 불가이면 null→빈 입력이며 NaN/Infinity·임의 기본 카메라를 만들지 않는다.
- 최초 열기/reload는 카메라로 goto를 채운다. 깨끗한 입력은 pan/zoom을 따라가되
  포커스 중인 좌표나 편집된 세 값은 하나의 초안으로 보존한다. Go가 실제 승인된
  snapshot으로 완료되어야 초안을 해제하고, 거절·연결 끊김·이후의 새 입력은 보존한다.
- 입력칸 Escape는 현재 카메라를 복원할 뿐 navigation을 전송하지 않는다. IME 입력
  중 Escape는 가로채지 않는다. 다른 view_id로 바뀌면 이전 파일 초안은 버리고,
  대기 중인 CLI 파일/레벨/goto 제안은 기존 뷰 snapshot으로 덮어쓰지 않는다.
  초안은 서버나 브라우저 저장소에 영구 저장하지 않으므로 전체 reload에서는 현재
  서버 카메라를 우선한다. 이는 일반 pan의 renderer/cut/캐시 정책 변경이 아니다.

검증:

1. Rust parser는 Unicode/공백·확장자 없는 마스크 이름·잡덱 레벨/모드·옵션 선행·
   `--`·독립 인스턴스의 전체 Command가 명시 view와 같은지 확인한다. CLI native
   lifecycle는 실제 bare SOURCE 시작, IPC gate는 같은 worker의 bare 재방문과 bare
   jobdeck 전달을 포함한다. 자동 색인/캐시 변경 금지와 첫 generation 옵션도 유지한다.
2. 카메라 단위 테스트는 half-DBU 위상·분수 단위·극소 값·표현 불가 값을 확인한다.
   실제 HTTP 첫 뷰의 중심/폭은 authoritative bbox와 정확히 왕복하고 CLI 입력과는
   기존 1e-9 µm 허용차로 비교한다. aspect-ratio f64 계산의 마지막 비트 차이
   (`20`→`20.00000000000003`)를 숨기려고 표시를 반올림하지 않는다.
   UI gate는 최초 복원·focus·초안·
   재접속·Escape/IME·잘못된/거절/성공 입력·늦은 ACK·stale snapshot·포커스한 채
   다른 파일로 전환·픽셀 복원을 검사한다. 추가 per-pan HTTP 조회는 없다.
3. 집중 Rust app18/web74 단위와 transport13, 전체 ES2017/UI 검사가 통과했다.
   Rust 1.89.0 app/web 단위, Linux musl all-targets check, clippy `-D warnings`도
   통과했다. 초기 회귀에서 CLI 요청값과 f64 카메라의 strict equality, 포커스를 둔
   채 일반 snapshot이 입력을 갱신한다고 가정한 테스트를 수정했다. 각각 정확한
   viewport 왕복과 포커스 보호 계약을 검사하며 기능을 완화하지 않는다.
4. 실제 Chrome에서 기존 합성 `second.oas`를 bare FILE로 열어 컬러 geometry와
   goto(5,6,300), depth7/high/keep/frames on/labels off/font23을 확인했다. reload 뒤
   goto와 표시 옵션을 유지했고, 오른쪽 pan 뒤 X=154.48096885813146으로 갱신됐다.
   X 초안 `123.`을 남기고 위로 pan해도 세 입력은 보존됐다. Escape 뒤 현재
   X/Y=(154.48096885813146,107.73010380622839)로 복원되며 gen4가 유지됐다.
   End session 뒤 exit0·session 파일 제거·worker 디렉터리 비움·테스트 탭 종료와
   OVM/OVP SHA256 불변을 확인했다. 첫 브라우저 연결 도구 시간 초과는 탭 생성 전
   unused bootstrap 만료/exit0로 끝났고, 준비한 빈 탭과 새 세션에서 위 검증을 했다.
   실칩·Firefox/ETX 수용, 진행 중 브라우저 장애 주입 결과로 확대하지 않는다.

전체 `sh tools/validate_rust.sh`는 exit0, `RUST VALIDATION: ALL OK`로 완료했다.
수정한 실제 bare CLI 시작·같은 창 전달과 GTK 표시 설정 대조, 전체 ES2017/UI,
owner17, occupancy25, 잡덱80, 렌더러46, KLayout13 PX+2 phase-exact+14 style을
포함한다. 기존 Python gate의 deprecation/resource 경고는 남아 있지만 실패는 없다.
전체 로그: `/private/tmp/floe-open-parity-battery-final.log`.
집중 로그: `/private/tmp/floe-open-parity-{rust,ui-final,msrv,linux,clippy}.log`.

남음: 미이관 view 옵션(GDS/gzip/stream/진단 등의 명시 처리 포함)과 조작 parity,
G1 지연/pacing·G4 전체 수용, 공유/원격·Firefox/ETX 현장. 이 단계의 완료를 전체
마이그레이션 완료율로 환산하지 않는다.

## 56. M4g-11a — 레이어 다중 선택의 원자적 가시성 변경

UI-03 대조에서 웹의 개별 체크박스만으로 GTK의 다중 선택 show/hide/toggle을
대체하지 못함을 확인했다. 이번 단계는 Rust 상태/API이고, 브라우저의 선택·접힘
UI와 다중 스타일 편집은 다음 단계다. 이를 전체 레이어 조작 parity 완료로 세지 않는다.

- `view.set.layer_batch`는 선택 pair와 action, 선택된 접힌 부모를 받는다.
  실제 자식/순서는 현재 model에서 해석한다. 일반 레이어 그룹은 같은 layer의
  최소 datatype이 부모이며 펼쳤으면 개별 행, 접었으면 자식을 포함한다.
- 잡덱의 합성 부모는 펼침 여부와 무관하게 모든 자식을 포함한다. 부분적으로
  보이는 부모의 toggle은 전체 hide다. 부모·자식이 함께 선택되면 palette 순서대로
  부모를 먼저 처리하고 포함된 자식을 건너뛰어 두 번 toggle하지 않는다.
- 요청 전체는 기존 view/epoch/revision 검사를 거쳐 한 상태로 교체한다. 대상 검사
  실패·다른 가시성/설정 조작과 충돌하면 원래 뷰를 보존한다. 실제 변경이 없는
  show/hide는 렌더하지 않으며, explicit-all 목록을 단지 All로 정규화하는 것도
  새 렌더를 일으키지 않는다. 카메라·스타일·색인·raster 정책은 바꾸지 않는다.
- 입력 선택/접힘 목록과 펼친 대상은 각 4096개 이하, 기존 explicit 가시성 목록과
  transport body 상한도 유지한다. 상한을 넘으면 전체 오류다. 큰 그룹을 먼저 전량
  복사하지 않고 4097개까지만 읽어 초과를 판정한다. 일반 all/none은 기존 별도 조작이다.

검증:

1. `validate_layer_palette.py`는 GTK의 실제 `_set_selected_layers`,
   `_on_layer_toggled`, `_sync_jobdeck_groups`, `_is_jobdeck_head`를 AST로 읽어
   inert row에 실행한다. 일반/잡덱·63개 선택 조합·8개 가시성·4개 접힘 상태·
   3개 action, 총 12,096개를 Rust와 비교해 통과했다. 도형/파일/GTK 런타임은
   필요 없고 개발용 Python oracle이며 제품 런타임 의존성이 아니다.
2. core/wire gate는 중복/뒤집힌 선택 순서, 부분 잡덱 부모, 존재하지 않는 행,
   자식·독립 행을 접힌 부모로 위조, 빈/초과/충돌 요청과 no-op 보존을 고정한다.
3. 실제 native HTTP/WS에서 여러 레이어를 한 번에 숨기고 원복하여 raw 픽셀이
   원본과 같은지, 변경당 revision/렌더가 한 번인지, no-op/stale/무효/충돌에 렌더가
   없는지 확인했다. 처음 고른 두 레이어는 겹침 때문에 픽셀 변화가 보장되지 않아
   전체 fixture 레이어 숨김/원복으로 강화했다. 나머지 레이어 보존은 1번에서 대조한다.
   stream6 통과, cache bytes/mtime 불변·worker 정리도 확인했다.

최종 app-core253/web75 단위, clippy `-D warnings`, Rust 1.89.0 단위 및 Linux
musl all-targets check가 통과했다. 이번에는 화면 코드를 변경하지 않았으므로 실제
브라우저의 다중 선택 클릭을 검사했다고 주장하지 않는다.
집중 로그: `/private/tmp/floe-palette-{oracle,stream-final,clippy-final,msrv-final,linux-final}.log`.
전체 `sh tools/validate_rust.sh`는 exit 0 / `RUST VALIDATION: ALL OK`로 끝났다
(`/private/tmp/floe-palette-battery.log`). 새 palette oracle/stream과 함께 owner17,
occupancy25, 잡덱80, 렌더러46, KLayout13 PX+2 phase-exact+14 style 대조를 통과했다.
다음 우선순위는 `feature/jobdeck`의 로컬 `09be2ab`까지 미합류 16개 커밋을 §11에
따라 합류하고 Rust 서비스의 occupancy 기본 생성·depth/희소 표시 대응을 검증하는
것이다. 이 단계에서는 합류하지 않았다. 그 뒤 브라우저 다중 선택·접기/펼치기·
페이지 간 선택과 multi-style 조작을 잇는다.
잔여 CLI/형식·G1/G4·공유/원격·현장 수용은 계속 열려 있다.

## 57. M4g-11b — 실측 잡덱 합류와 occupancy 기본 생성

`feature/jobdeck`의 로컬 `09be2ab`까지 16개 커밋을 §11의 정방향 merge로 가져왔다.
실측 브랜치와 main의 작업 트리를 수정하거나 역머지하지 않았다. 버전 충돌만
수동 해결했으며 결합된 renderd/index는 `0.12.89`, Python 표시 버전은 `0.12.133`이다.
양쪽 분기의 이전 실행 파일을 잘못 호환으로 받아들이지 않도록 함께 재빌드해야 한다.

합류한 핵심은 빈 occupancy 레이어의 무비트맵 표현·마킹 병렬화, 레이어별 전체
내용을 포함하는 depth의 요약 사용, 희소 sub-cut 페이지 유지/배치 확장과 wash 억제,
알 수 없는 index 옵션 거부, GTK 로딩 안내와 기본 요약 생성이다. 기하/요약 알고리즘은
실측 브랜치의 계약을 보존한다. 그 브랜치에 기록된 실칩 150ms는 웹 실측값이 아니다.
occupancy의 M5 완료와 웹 M5(world-tile)를 혼동하지 않는다.

Python 변경의 Rust 대응:

- 일반/잡덱 CLI, managed index, 웹 index/index-open은 occupancy 기본 on이다.
  fresh current 캐시에 요약만 없으면 추가하고 base marker·page·text·metadata를
  보존한다. stale/incomplete 캐시는 여전히 별도 force가 필요하다.
- `--no-occupancy` / `options.occupancy:false`는 생성·추가 생략이며 기존 요약 삭제가
  아니다. `occupancy-only`는 요약만 다시 만들고, `occupancy-um`은 생성을 요청한다.
  명시 CLI 모드 플래그끼리는 배타적이다. 셀 profile에는 기본/명시 summary 옵션을
  전달하지 않아 normal cache/lock을 만들지 않는다.
- 신규 웹 승인 dialog와 일반 index checkbox는 기본 체크한다. 저장된 false 승인,
  응답 유실/reload 후 동일 승인 재시도는 false 그대로이며 비활성 checkbox/jobs도
  저장된 승인 값으로 표시한다. 선택/preview는 쓰지 않고
  force 체크는 계속 기본 off다. 기존 작업/모드 전환 상태 안내와 thin 3상태 선택은
  이미 웹에 있어 GTK widget을 복사하지 않는다.
- 잡덱 CLI 완료 줄에 `(n/N)`, 경과 및 평균 기반 남은 시간(추정)을 표시한다.
  기존 Rust 서비스는 순차 source 실행과 내부 jobs, 취소/lease 계약을 유지한다.
- 합류 중 Python 잡덱 wrapper의 no-occupancy 누락을 발견했다. 자식 CLI 기본값도
  on이므로 새/force 소스에 flag를 보내지 않으면 해제가 유실된다. 웹 작업 트리에서
  `--no-occupancy` 전달을 보완하고 Rust/Python 양쪽의 실제 소스 생성으로 검사한다.
  이 추가 수정은 아직 실측 브랜치로 역반영하지 않았다.
- Python CLI의 암묵 occupancy 기본값은 backend 확정 뒤 적용한다. 그렇지 않으면
  `--legacy`도 Rust 옵션을 지정한 것으로 오인되어 새 `.tiles` oracle을 만들 수 없다.
  Rust 기본 on/legacy 기본 off를 구분하고 legacy에 명시한 summary 옵션은 계속 거부한다.

검증 결과:

- app-core254/web76/app20 단위, 이 세 package 대상 strict clippy, Rust 1.89.0 단위, Linux musl
  all-targets check 통과. CLI의 default OVO bytes를 Python과 대조하고 opt-out→기존
  캐시 추가 시 base bytes/mtime 보존, profile JSON/snapshot·SIGINT/SIGTERM을 통과했다.
- 잡덱 source/index gate에서 선택된 source의 기본 생성, opt-out→additive, LOD와
  occupancy-only, 중간 실패·비선택 소스 보존, 완료 줄 진행 정보를 대조했다.
- 첫 전체 배터리는 GTK `open_file`이 `_open_file_load`로 분리된 데 따른 대조 도구의
  AttributeError에서 멈췄다. 새 helper와 로딩 callback을 포함해 실제 GTK 정책을
  다시 실행하고 1,296개 대조를 통과했다. legacy 기본값은 stub writer gate와 별도로
  새 합성 소스에 실제 `index --legacy --jobs 1`을 실행해 `.tiles` 생성/`.floe` 미생성을
  확인했다. 재실행 로그는 `floe-deck-sync-battery-final.log`다.
- index-open/client JS gate는 기본 체크, 명시 해제, 승인 journal/재시도 불변을 확인한다.
  native owner gate는 생략 옵션의 실제 OVO 생성과 첫 프레임을 검사한다.
- 실제 Chrome 시도는 브라우저 도구의 `file://` 인증 시작 파일 정책으로 차단됐다.
  우회하지 않았고 합성 서버는 종료하여 임시 인증 파일을 정리했다. 이번 기본 체크의
  실제 브라우저 클릭 수용은 미검증이며, native/JS 검증을 그 수용으로 대신하지 않는다.
- 집중 로그: `/private/tmp/floe-deck-sync-{units,build,cli,sources,clippy,msrv,linux}.log`.
  추가 renderer 전체 strict clippy(`render-clippy.log`)는 통과하지 않았다. lib test
  기준 31개 진단이며 미사용 repetition 함수, 기존 deck/raster/summary 스타일과
  합류한 layer-depth 루프의 `needless_range_loop`가 포함된다. deck/raster/repetition은
  이번 HEAD 대비 변경이 없음을 확인했다. 이 lint 부채를 실행·픽셀 회귀 PASS나
  위의 app/core/web clippy PASS와 혼동하지 않는다.
  추가 로그: `floe-deck-sync-{file-display,python-index,legacy}.log`.
  전체 `sh tools/validate_rust.sh`는 exit 0 / `RUST VALIDATION: ALL OK`로 끝났다
  (`/private/tmp/floe-deck-sync-battery-final.log`). owner17, GTK 파일 정책1296,
  palette12096, native stream6, occupancy27, 잡덱83, 렌더러46, VFS H1-H5/L1-L9,
  KLayout13 PX+2 phase-exact+14 style을 통과했다.

목표 잔여: 레이어 다중 선택/접힘/페이지 간 UI와 스타일 조작, 잔여 CLI·입력 형식,
G1 지연/pacing·G4 전체 수용, Python-free Linux 실행, 공유/원격 및 Firefox/ETX 현장.
이번 합류로 실측 코드와의 차이는 줄었지만 전체 목표 완료 직전으로 판정하지 않는다.

## 58. M4g-11c — 접기·페이지 간 범위 선택의 읽기 전용 Rust API

UI-03의 다중 선택을 연결하기 전, 현재64행 카탈로그가 표현하지 못하던 일반 datatype
그룹과 페이지 간 범위 선택을 서버에 추가한다. 기존 `layer_batch`는 가시성을 한 번에
바꾸는 쓰기 경로이고, 이번 API는 **패널 순서만 조회하는 별도 경로**다. 브라우저의
선택/접기 버튼은 아직 추가하지 않았다. 다중 style 조작은 가시성의 항상-자식-포함
잡덱 부모 규칙과 다르므로 후속으로 남긴다.

구현:

- 일반 layout과 잡덱 source-layer 모드는 layer별 최저 datatype을 부모로 묶는다.
  datatype0이 없어도 동작한다. jobdeck level/chip의 synthetic head는 기존 의미를
  유지하며, level 모드의 숨긴 chip은 패널 목록에 넣지 않는다.
- `POST .../palette`는 `page`/`range`와 `fold:{closed,exceptions}`만 받는다.
  전체 접기/펼치기에 모든 부모 ID를 실을 필요가 없으며 개별 예외는 최대4096개다.
  기존16KiB 요청 상한은 별도로 유지한다. 잘못된 부모·중복·null/추가 필드는 오류다.
- 접힌 자식 span을 먼저 건너뛰고 최대64행으로 페이지를 나눈다. 원래 페이지에서
  자식을 사후 숨기는 방식의 빈 페이지가 없다. 범위는 두 anchor를 포함하며 역방향도
  같은 순서이고 페이지 경계를 넘을 수 있다. 최대4096개; 초과/숨긴 anchor는 명시 오류다.
- immutable catalogue의 그룹 경계를 한 번 만들고, 페이지 조회에는 실제 geometry
  순회·decode가 없다. 펼친 목록의 총수 계산은 표시 행 수에 비례하지만 반환/임시
  페이지 목록은64개이며 접힌 대형 그룹은 경계로 건너뛴다. 범위 열거는4097개에서
  오류로 중단한다. 서버에 사용자 선택/접기 상태를 저장하지 않는다.
- row의 부모/자식 수·접힘과 범위의 부모 목록을 반환해, 브라우저가 전 카탈로그를
  미리 내려받거나 그룹 자식을 확장하지 않아도 `layer_batch`를 구성할 수 있게 한다.
  응답 revision/render key로 지연 결과를 구분한다. 현재 UI의 구 GET schema는 보존한다.
- renderer/VFS/색인 포맷·정확도 정책·렌더러 프로토콜 버전은 바꾸지 않는다.

검증은 GTK 실제 `_selectable_layer_order`로 만든32개 일반/잡덱/숨긴 chip/450행
목록·접기 조합과 12,096개 가시성 batch를 사용한다. Rust 단위는 비영 datatype 부모,
150개 그룹의64행 페이지, 양방향·숨긴 anchor·빈 목록·4096 경계,10만 행 그룹 접기,
엄격 DTO를 다룬다. native HTTP/WS는 인증·Origin/CSRF·16KiB 상한·old view 거부,
구 GET 보존과 조회 전후 revision/render key/제출 수 불변을 단언한다. 실제 잡덱
level/chip/layer 왕복에서도 숨긴 chip 비노출과 카메라/가시성 불변을 검사한다.

집중 검증은 web82 단위(외부 oracle2개는 별도 실행), transport13, native stream6,
owner17과 GTK32/12,096 대조를 통과했다. 최종 web all-targets strict clippy,
Rust1.89 web 단위, Linux musl all-targets check도 통과했다. 이는 실제 Linux/Firefox
실행 수용을 뜻하지 않는다. 기존 renderer 전체 lint 부채(§57)는 수정하지 않았다.
전체 `sh tools/validate_rust.sh`도 exit0 / `RUST VALIDATION: ALL OK`로 끝났다
(`/private/tmp/floe-palette-read-battery.log`). app20/core254/web82 단위,
GTK32/12,096·native stream6·owner17, occupancy27·잡덱83·렌더러46,
VFS H1-H5/L1-L9와 KLayout jobs1/8 각각13 PX+2 phase-exact+14 style을 통과했다.
집중 로그는 `floe-palette-read-{tests,gtk,stream,owner,clippy-final,msrv,linux}.log`다.

목표 잔여: 이번 커밋으로 UI-03의 **서버 선택·접기 조회**를 닫지만 브라우저 Ctrl/Shift
선택·접기/펼치기·일괄 가시성 연결과 다중 스타일 조작은 남는다. 그 밖의 CLI/형식 차이,
G1 지연/pacing·G4 최종 수용, Python-free Linux 실행, 공유/원격과 현장 Firefox/ETX도
아직 남아 있다. 로컬 구현 후반부와 전체 목표 완료를 구분하며 백분율로 환산하지 않는다.

## 59. M4g-11d — 웹 레이어 다중 선택·접기와 일괄 가시성

§56의 원자적 가시성 쓰기와 §58의 목록/범위 조회를 실제 웹 패널에 연결했다.
`palette.js`는 선택·접힘·메뉴만 소유하고, 레이어 순서/그룹/스타일/가시성은 계속
Rust가 결정한다. 전체 카탈로그를 브라우저로 내려받거나 geometry를 순회하지 않는다.

- 클릭 선택, Ctrl/⌘ 추가·해제, Shift 범위와 modifier+Shift 합집합, 단독 재클릭
  해제·double-click 가시성 toggle을 지원한다. 우클릭은 이미 준비한 선택을 바꾸지
  않고 Show/Hide/Toggle selected·전체 on/off 메뉴를 연다. Shift+F10/ContextMenu,
  메뉴의 방향키/Home/End/Escape와 버튼 focus를 연결한다.
- 개별 그룹과 전체 접기/펼치기를 지원한다. 부모는 datatype0이 없으면 최저
  datatype이며 jobdeck level 모드의 숨긴 chip은 노출하지 않는다. 접기는 page0으로
  돌아가고 숨긴 자식의 기존 선택은 유지한다. 숨겨진 anchor의 Shift는 GTK처럼
  단일/modifier 선택으로 처리한다. 선택과 접기는 같은 view의 재접속에서는 유지하고
  새 view에서는 초기화한다. 파일·sessionStorage에 새로 저장하지 않는다.
- 같은64행 페이지의 범위는 이미 받은 서버 순서로 즉시 선택하며 페이지를 넘은
  범위만 읽기 전용 API를 호출한다. selection/fold 예외는 각각4096개 상한이다.
  기존 HTTP16KiB·WebSocket8KiB 상한도 적용되어 pair 값/개수에 따라 더 일찍 전체
  오류가 될 수 있다. 자동 분할 쓰기·부분 선택/적용·조용한 truncation은 없다.
- 선택 행의 Show/Hide/Toggle은 `layer_batch` 한 번이다. 체크박스와 double-click도
  같은 경로로 일반 접힌 부모의 자식 포함·jobdeck 부모 규칙을 따른다. 전체 All/None은
  기존 전체 가시성 명령이다. 선택/접기는 렌더하지 않고, 가시성이 실제로 바뀔 때만
  서버 revision과 렌더가 바뀐다. 개별 행의 색/fill/width 컨트롤은 보존한다.
- 체크박스는 먼저 확정 표시하지 않는다. 쓰기 ACK만으로 다시 활성화하지 않고
  authoritative snapshot 또는 오류를 기다린다. 거부된 쓰기는 자동 재전송하지 않는다.
  view/key/접힘/offset과 입력 revision이 달라진 읽기는 취소·폐기한다. 새 클릭 이후
  옛 범위 응답이 선택이나 오류 안내를 덮지 않으며, pagehide/연결 단절로 중단한
  다음 페이지는 재접속 때 그 offset으로 다시 읽는다. 읽기 실패는 명시 Retry다.
- 선택 행 표시는 파란 배경, 도형 pick의 레이어 강조는 기존 별도 표시를 유지한다.
  이름과 alias는 textContent/속성으로만 표시한다. 렌더러/VFS/색인·wire 버전 변경은
  없으며 JS/CSS/HTML은 Rust binary의 새 content-identified asset bundle에 포함한다.

로컬 검증:

- GTK 실제 `_on_layer_clicked`에서 추출한7,776개 조합(선택·anchor·접힘·modifier·
  클릭 종류)이 웹 순수 선택 함수와 일치한다. 기존 목록/접기32개·가시성12,096개
  GTK-source 대조도 유지한다. 테스트에만 Python/Node를 사용하며 제품은 호출하지 않는다.
- 유계 DOM harness는 페이지 간92행 선택, 숨긴 선택 보존, 접힌 부모 포함, 메뉴/focus,
  no-op·거부·늦은 응답·4096 초과·잘못된 DTO·재접속/새 view·정리를 검사한다.
  실제 app.js 연결 gate는 선택/접기에 `view.set`/canvas draw가 없고 일괄 가시성은
  한 CAS로 제출되며 ACK만으로 완료 처리하지 않는지, stale DOM/거부 요청의 미재전송을
  단언한다. 기존 JS 회귀와 ES2017 파서 검사도 통과했다.
- HTTP transport gate는 HTML의 새 모듈 참조와 실제 asset MIME/내용 식별을 검사한다.
  실제 브라우저 클릭·키보드·화면 스크린샷은 이번 단계에서 확인하지 않았다. §57의
  인증 시작 파일에 대한 브라우저 도구 제한을 우회하지 않았으며, 위의 모의 DOM/native
  검증을 실제 브라우저 수용으로 세지 않는다.
- web all-targets strict clippy, Rust1.89 web 단위82개, Linux musl all-targets check를
  통과했다. 기존 renderer 전체 lint 부채(§57)는 그대로이며, Linux 컴파일은 실행
  수용이 아니다. 집중 로그는 `/private/tmp/floe-palette-ui-{js-final,gtk,clippy,msrv,linux}.log`다.
- 전체 `sh tools/validate_rust.sh`는 exit0 / `RUST VALIDATION: ALL OK`로 끝났다
  (`/private/tmp/floe-palette-ui-battery.log`). app20/core254/web82 단위,
  transport13·native stream6·owner17, GTK 선택7,776/목록32/가시성12,096,
  occupancy27·잡덱83·렌더러46, VFS H1-H5/L1-L9와 KLayout jobs1/8 각각
  13 PX+2 phase-exact+14 style을 통과했다. 검증용 `.venv` 링크만 제거하고
  원래 환경과 main/실측 브랜치의 별도 작업은 보존했다.

목표 잔여: 이번 단계에서 레이어 **다중 선택·접기·일괄 가시성의 로컬 연결**을 닫는다.
다음은 다중 style 편집이다. expanded jobdeck 부모의 스타일 대상은 가시성의 항상
자식 포함 규칙과 다르므로 그 동작을 별도로 이관해야 한다. 잔여 CLI/view 옵션·
입력 형식 차이, G1 지연/pacing·G4 전체 수용, 실제 브라우저 저장/복구/입력과
Python-free Linux 실행이 남는다. 공유 게스트/읽기 전용 권한·원격 배포는 미구현,
Firefox/ETX 현장 검증은 보류, world-tile M5는 조건부 보류다. 전체 완료 직전으로
표시하거나 세부 커밋 개수를 완료율로 환산하지 않는다.

## 60. M4g-11e — 선택 행의 다중 스타일과 GTK sparse 상속

선택된 레이어의 색상·채움·선폭 및 상대 증감을 하나의 `style_batch`로 적용한다.
`styles`/`style_deltas`는 이전 API 의미를 유지하며 새 팔레트 명령과 혼합하지 않는다.
색인·renderer wire/버전·정확도 정책이나 공유 파일을 변경하지 않는다.

§59에서 예상했던 “펼친 부모의 스타일 대상은 가시성과 다르다”는 문장은 위젯의
선택 확장만으로는 충분하지 않았다. **실제 GTK→DeckRenderWorker 경로**에서 색은
해당 요청의 부모를 자식에 전파하고, 채움/선폭은 매번 전체 sparse map을 전송하여
자식 override가 부모보다 우선한다. 새 명령은 이 차이를 보존한다.

- 일반 그룹: 접힌 부모는 같은 layer의 최저 datatype을 포함해 자식까지 직접 지정한다.
  펼친 부모는 자기 행만 지정한다. 선택한 부모/자식은 중복 적용하거나 두 번 증감하지 않는다.
- 잡덱 그룹: 부모 색은 펼침과 관계없이 전파한다. 부모 fill/width는 자식 지정값을
  지우지 않고 지정이 없는 자식에 상속된다. 접힌 부모는 자식도 직접 지정한다.
  level 뷰의 숨긴 chip은 Model에서 영구 접힘으로 판정한다. 숨긴 행을 웹으로 보내거나
  UI에서 임의 layer ID 규칙으로 추론하지 않는다.
- width1은 sparse override 제거다. +/-1은 현재 지정값(없으면1)을 기준으로 하고
  1..8에 clamp한다. 예를 들어 부모6을 상속한 자식의 Increase는2, Decrease/1은
  지정값을 제거해 다시6으로 보일 수 있다. GTK와 같으며 편집기에 설명한다.
- color-only는 채움·선폭 상속을 구체 값으로 고정하지 않는다. fill/width 변경은
  전체 sparse map에서 표시를 다시 해석한다. Native JSON Load/Save 뒤에도 상속을 유지한다.
- 선택·접기 부모·하위 영향 집합에4096개 상한을 적용하고 확장 전에/중에 검사한다.
  기존8KiB WS 상한도 유지한다. 클라이언트의 영구 크기 초과는 “기다리라”가 아니라
  선택/입력을 줄이라고 안내하고 wire에 보내지 않는다. 부분 적용·자동 분할은 없다.

웹의 Style selected 버튼/우클릭 항목은 현재 선택과 접힘을 고정한 편집기를 연다.
RGB checkbox와 fill/width의 Unchanged를 분리해 지정하지 않은 필드를 건드리지 않는다.
clear/solid/speckle 또는16행 custom bitmap, width1..8 및 Increase/Decrease를 제공한다.
편집기 열기·입력은 렌더하지 않고 Apply 한 번만 CAS를 제출한다. 선택·접기·view/key·
연결이 바뀌면 이전 편집 대상을 버리고, 지연 ACK만으로 재활성화하거나 거부를 재전송하지
않는다. 기존 단일 행 색/스타일 컨트롤은 남겨 두었다.

검증:

- `validate_palette_styles.py`는 실제 GTK 메서드와 실제 layout/deck submit 확장을
  실행한다. 생성자의 파일 조회와 pipe publish만 inert하게 두고, 일반 비영 datatype
  부모·chip·숨긴 level,63개 선택 조합·4개 접힘·2개 초기 상속·7개 색/fill/width 액션
  **10,584개**의 표시 값과 sparse assignment를 Rust와 대조한다. solid/clear/checker의
  enum/동등 bitmap 표현만 정규화한다. GUI/브라우저 자체를 실행한 검증은 아니다.
- Rust 단위는 필드별 상속·접힌 덮어쓰기·증감·Native JSON 왕복·원자 오류/확장 상한을
  검사한다. DTO는 null/추가·중복 필드·무효 RGB/bitmap/width·빈 변경을 거부한다.
- JS는 frozen selection, 선택/접힘/연결 변경 폐기, 잘못된 bitmap·빈 변경, 단일 필드와
  합친 batch·거부 미재전송을 검사한다. 실제 app.js 연결에서는 선택 유지, 하나의 CAS,
  ACK/snapshot 순서와 응답 뒤 갱신된 행의 색을 검사한다.
- native stream에 단일 revision/frame, no-op, stale/무효 입력의 무변경, 원래 raw 픽셀로
  복원하는 검사를 추가했다. 실제 잡덱 Model에서도 hidden level과 expanded chip의
  child override 결과를 검사한다. 전체 배터리 결과는 아래에 별도 기록한다.
- §57의 브라우저 인증 시작 파일 제한을 우회하지 않았다. 이 편집기의 실제 브라우저
  클릭·입력·스크린샷 수용과 현장 Firefox/ETX는 미검증이다.
- 첫 전체 배터리는 기존 worker-client 수명주기 테스트의 `worker command queue full`
  로 실패했다. 질의8개 직후 writer가 이미 큐를 비웠다고 가정한 테스트였다.
  제품의 큐/질의 상한과 코드는 바꾸지 않고, 테스트에서만 최대250ms 동안 `Busy`를
  재시도한다. Busy에서 generation/질의 credit 무변경,9번째 질의의 credit 오류,
  렌더·취소 독립성과 질의 timeout/정리를 계속 단언한다. held-query deadline은
  writer·렌더·취소 검사와 경합하지 않게2초로 분리했다. 첫 실패 기록은
  `/private/tmp/floe-palette-style-battery.log`에 보존한다.
- 집중 검증은 core257/web83 단위, GTK+adapter10,584개, native stream7개와
  실제 잡덱23 PNG/report·6 API/controller·24 mode transition 검사를 통과했다.
  worker-client lifecycle14개는 수정 후5회 반복도 통과했다. core/web/worker-client
  all-targets strict clippy, Rust1.89 단위/수명주기와 Linux musl all-targets check도
  통과했다. 기존 renderer 전체 lint 부채(§57)는 그대로이며 Linux 컴파일을 실제 실행
  수용으로 계산하지 않는다. 집중 로그는 `/private/tmp/floe-palette-style-*.log`다.
- 최종 전체 `sh tools/validate_rust.sh`는 exit0 / `RUST VALIDATION: ALL OK`로
  끝났다(`/private/tmp/floe-palette-style-battery-final.log`). app20/core257/web83 단위,
  native stream7, GTK 선택7,776/스타일10,584, occupancy27·잡덱83·렌더러46,
  VFS H1-H5/L1-L9와 KLayout jobs1/8 각각13 PX+2 phase-exact+14 style을 통과했다.
  검증용 `.venv` 심볼릭 링크만 제거했으며 원래 환경과 main/실측 작업은 보존했다.

목표 잔여: 다중 스타일의 **로컬 기능 연결**을 닫는 단계이지 UI-03 전체 완료가 아니다.
다음은 이름 있는 색/채움 프리셋·bitmap 직접 편집과 행 단위 스타일 UX 대조다.
GTK bitmap 편집은 `FLOE_FILL_EDIT` 뒤의 개발용 기능이므로 이 경계도 함께 확인하며,
일반 프리셋 선택과 개발용 슬롯 변경을 같은 제품 기본 기능으로 취급하지 않는다.
미이관 CLI/view 옵션·GDS/gzip 차이, G1 지연/pacing·G4 전체 감사, 실제 브라우저 저장/
복구/입력·Python-free Linux 실행도 남는다. 공유/원격은 미구현, 현장 Firefox/ETX는
보류, world-tile M5는 조건부 보류다. 커밋마다 이 잔여를 보고하며 임의 완료율로 바꾸지 않는다.

## 61. M4g-11f — 이름 있는 프리셋과 단일 행 스타일 경로 통일

GTK의 `colornames.def`49색(7×7), `fillpatterns.def`20채움(5×4)을 같은 순서로
웹에 제공한다. yellow/yellow1처럼 RGB가 같은 별칭도 별도 버튼으로 유지한다.
새 Rust 목록은 공유 `.def`를 compile-time에 포함하고 런타임 Python·파일 읽기를
호출하지 않는다. 각 표는 최대256항목, 이름64 ASCII자, 색6 hex/패턴16×u16로
제한한다. 잘못된 내장 표를 일부만 표시하지 않고 오류로 처리한다.

- 인증된 `GET /api/v1/palette/presets`는 열린 view·worker 없이 읽을 수 있다.
  색 이름/RGB와 패턴 이름/16행/정규화된 fill DTO만 반환한다. 현재 응답16KiB 이내를
  gate로 고정하며 source 경로·사용자 설정은 노출하지 않는다. `.def`와 새 모듈도
  web bundle 식별에 포함해 데이터만 바뀐 빌드가 예전 UI와 섞이지 않게 한다.
- Color and fill presets를 처음 펼칠 때만 읽는다. 같은 bundle 안에서는 view/policy
  변경으로 다시 읽지 않는다. 연결 단절·pagehide는 진행 중 읽기를 취소하고 늦은
  결과를 폐기한다. 읽기 실패는 명시 Retry이며 불완전한 팔레트는 표시하지 않는다.
- 이름은 title/접근성 이름으로, 색은 검증된 RGB로 표시한다. 채움은 흰 바탕의 검은
  16×16 미리보기이며 MSB가 왼쪽, 위 행부터다. 고정16 CSS px로 표시해 패턴을
  위젯 폭으로 늘리지 않는다. 스와치와 키보드 focus는 일반 button을 사용한다.
- 클릭은 **그 시점의 선택/접힘**을 캡처해 `style_batch` 한 번으로 해당 필드만
  지정한다. 읽기/선택 자체는 렌더하지 않는다. ACK만으로 버튼을 풀지 않고 현재
  snapshot을 기다리며 거부된 쓰기를 자동 재전송하지 않는다. 기존4096행/8KiB 쓰기
  상한과 Rust의 sparse 상속 규칙을 그대로 사용한다.
- 웹 단일 행 색 picker와 style form도 `style_batch`로 전환했다. 접힌 부모는
  자식을 포함하고, 펼친 잡덱 부모의 자식 fill/width override는 보존한다. 현재 값과
  다른 필드만 제출하여 변경 없는 Apply가 상속을 명시값으로 바꾸지 않는다.
  페이지/접힘/연결/정책이 바뀐 뒤 옛 행 편집기는 제출할 수 없다. 단일/다중 편집기는
  서로 닫으며 color picker도 서버 확인 전 새 색을 확정하지 않는다. 기존 외부
  `styles`/`style_deltas` API의 의미는 바꾸지 않는다.

검증:

- core259/web84 단위, HTTP transport14와 strict clippy를 통과했다.
  실제 GTK 표49색·20 bitmap은 이름·순서·모든16행을 Rust와 비교하며, 기존
  GTK/adapter 스타일10,584개 대조도 유지한다. 이 오라클의 Rust 검사는 PATH를 비워 실행한다.
- ES2017/DOM gate는 전49색·20패턴의 클릭 필드와 MSB-left 모든 미리보기 픽셀,
  별칭 보존·무효 DTO·읽기 실패/취소/늦은 응답·한 번 읽기를 단언한다. 실제 app.js
  연결 gate는 접힌 선택 CAS, ACK/snapshot 순서, 거부 미재전송, 옛 행 form 거부와
  편집기 전환을 검사한다. native stream에는 프리셋 GET의 state/render 무변경
  검사를 추가했다. 최종 배터리·최소 버전/Linux 결과는 아래에 별도 기록한다.
- 실제 브라우저 클릭/키보드/스크린샷 수용은 미검증이다. §57의 인증 시작 파일
  브라우저 도구 제한을 우회하지 않았으며 위 검증을 실제 화면 수용으로 세지 않는다.
- Rust1.89 core259/web84 단위와 Linux musl all-targets check도 통과했다.
  기존 renderer 전체 lint 부채(§57)는 건드리지 않았으며, Linux 컴파일은 실제
  Python-free Linux 실행 수용이 아니다. 집중 로그는 `/private/tmp/floe-presets-*.log`다.
- 전체 `sh tools/validate_rust.sh`는 exit0 / `RUST VALIDATION: ALL OK`로 끝났다
  (`/private/tmp/floe-presets-battery.log`). app20/core259/web84 단위, transport14·
  native stream7, GTK 프리셋49색/20패턴·스타일10,584, occupancy27·잡덱83·렌더러46,
  VFS H1-H5/L1-L9와 KLayout jobs1/8 각각13 PX+2 phase-exact+14 style을 통과했다.
  검증용 `.venv` 심볼릭 링크만 정리했으며 원래 환경과 main/실측 작업은 보존했다.

목표 잔여: 프리셋·행 스타일의 로컬 연결을 닫지만 UI-03 전체 수용은 아니다.
GTK의 `FLOE_FILL_EDIT` bitmap 편집은 슬롯 하나를 바꾸면 그 슬롯을 참조하는 모든
레이어를 바꾸는 개발용 기능이다. 현재 Rust는 레이어별 fill 값을 보존하므로 선택
레이어 hex 입력과 동등하지 않다. 이 개발 도구의 슬롯 상태/저장 의미 이관은 남긴다.
다음 로컬 우선순위는 M0의 미이관 CLI/view 옵션·GDS/gzip 등 입력 형식 대조다.
G1 지연/pacing·G4 전체 수용, 실제 브라우저 저장/복구/입력과 Python-free Linux 실행,
공유/원격 미구현, 현장 Firefox/ETX 보류, 조건부 world-tile M5도 여전히 남아 있다.

## 62. M4g-12 — direct-final 호환 CLI와 안전한 렌더 진단

M0 잔여 옵션을 이름이 아니라 실제 Python→worker 경로로 대조했다. `--hairline`과
`--thin-um`은 환경을 설정하지만 이를 읽는 곳은 KLayout용 `vfsclient.py`뿐이다.
`rust_render.py`/renderd는 두 환경이나 동등 필드를 소비하지 않는다. 이전 M0 표의
“프레임 정책 override” 표현을 정정했다. 웹 이관에서 새 hairline/격자 정책을 켜지 않는다.

- `view --stream-kb 0`은 기존 `--refinement off`와 동일하게 round_pages=2^30으로
  설정한다. 환경값보다 우선하며 명시 process 옵션이므로 독립 workspace를 시작한다.
  source 없이 빈 창도 가능하고, 뒤에 열리는 파일에 동일 설정이 적용된다. 이 옵션만으로
  frame/decoded cache나 frames/labels를 끄지 않는다. nonzero·음수·실수는 명시 오류다.
- `view --render-debug`는 workspace의 `RenderOptions.debug`에만 저장한다. 기본 off,
  환경/전역 mutable 상태/HTTP 옵션 없이 trusted CLI만 켠다. 기존 창으로 포워딩하지
  않으며 receiver도 위 두 process 옵션을 거부한다. 소스/모드 교체로 열린 새 render
  worker에도 같은 설정이 전달된다. 별도 clip worker의 진단까지 추가한 것은 아니다.
  native render wire·cache key·페이지/픽셀 정책은 바꾸지 않는다.
- `RenderSession.poll`에서 소비한 frame마다 `[render-perf]` 한 줄을 stderr로 쓴다.
  PID·세대·round·final/partial/deferred/labels_truncated·출력 w/h/bytes와 고정 allowlist의
  숫자 phase/cache/paint/deck/summary 지표만 담는다. 미래의 알 수 없는 필드와 숫자가 아닌
  값은 제외하고 u64로 정규화해 4KiB 미만으로 제한한다. 경로·월드 좌표·원본 텍스트·raw
  wire/stderr는 출력하지 않는다. 예전 debug의 raw dump와 의도적으로 다르다.
- 이 로그는 worker frame 수신 기준이며 화면에 실제 표시된 프레임·input-to-photon·
  API 완료 ACK가 아니다. prefetch/나중에 버릴 frame도 포함될 수 있다. stderr는 동기
  출력이므로 느린 소비자가 있으면 측정에 영향을 준다. 기본 benchmark는 off로 하고,
  진단 출력 실패만으로 정상 렌더를 실패시키지 않는다. 오류·query의 원문 중계도 하지 않는다.
- `--stream-target-ms`, view `--lod`, `--hairline`, `--thin-um`, GTK `--dump`,
  `--floe-reviewer`는 이유를 포함해 시작 전에 거부한다. 특히 reviewer 표시 태그를
  `--drc-reviewer`의 게시 권한 opt-in으로 자동 변환하지 않는다. 안내가 생긴 것이지
  progressive/표시 진단/태그 기능을 이관 완료한 것은 아니다.
- 형식 감사: 기존/신규 source catalog 모두 GDS/gzip은 제한된 header/DBU 조회만 하고
  네이티브 색인 불가로 분류한다. plain OASIS·jobdeck parity와 추가 형식 지원을 분리한다.
  자동 변환·재색인·Python fallback·cache format 변경은 없다([M0 §2.9](WEBUI_M0.ko.md)).

검증: app22/core261/web84 단위 검사는 통과했다. 진단 gate는 민감한 문자열·좌표·
미래 필드·줄바꿈·u64 초과·긴 zero-padding이 로그에 새지 않는지 단언한다.

- 실제 GTK startup 정책144개와 native 실행8조건/첫 frame7개를 대조했다.
  `FLOE_RUST_ROUND_PAGES=1`을 둔 뒤 off/stream-kb 별칭을 번갈아 사용해 layout/덱
  모두 gen1/round1 한 번으로 끝나는지 검사한다. debug on인3조건만 stderr에 한 줄이
  나오며 모든 값이 ASCII 숫자, geometry/source 경로 미포함, stdout 미출력임을 단언한다.
- CLI의 미지원 옵션은 파일/worker를 열기 전에 이유를 포함해 실패한다. 기존 cache
  bytes·재사용·force·LOD/occupancy·profile·snapshot·signal/lock gate도 통과했다.
  GDS/gzip header/DBU·unsupported source 상태는 기존 Python/Rust 대조를 통과했다.
- app/core/web all-targets strict clippy와 Rust1.89 Linux musl all-targets check를
  통과했다. Linux 컴파일을 실제 Python-free Linux 실행으로 세지 않는다. 기존 renderer
  전체 lint 부채(§57)는 그대로다. 집중 로그는 `/private/tmp/floe-cli-audit-*.log`다.
- 실제 브라우저 입력/표시 수용은 미검증이다. 이전 브라우저 인증 시작 파일 도구 제한을
  우회하지 않았으며 HTTP/WS/native/DOM gate를 화면 수용으로 바꾸지 않는다.
- 전체 `sh tools/validate_rust.sh`는 exit0 / `RUST VALIDATION: ALL OK`로 끝났다
  (`/private/tmp/floe-cli-audit-battery.log`). app22/core261/web84 단위, native stream7,
  실제 startup8조건/7 frame·GTK 정책144개, occupancy27·잡덱83·렌더러46,
  VFS H1-H5/L1-L9와 KLayout jobs1/8 각각13 PX+2 phase-exact+14 style을 통과했다.
  검증용 `.venv` 심볼릭 링크만 제거했으며 원래 환경과 main/실측 작업은 보존했다.

목표 잔여: 이 단계는 **잔여 CLI/형식 감사와 지원 가능한 두 옵션**을 닫는다.
다음은 G4 기능별 최신 구현/검증 대조다. GTK 개발 bitmap 슬롯·표시 진단/태그와
progressive 정책 결정, 실제 브라우저 입력/저장/복구 수용, Python-free Linux 실행,
G1 지연/pacing·G4 전체 수용은 남는다. 공유/원격은 미구현, 현장 Firefox/ETX와
M3는 보류, world-tile M5는 조건부 보류다. 전체 완료에 가깝다고 과장하거나 임의
완료율로 환산하지 않고 커밋마다 이 잔여를 함께 보고한다.

## 63. M4g-13 — 폐쇄망 두벌식 한글 입력기와 G4 잔여 감사

2026-09-16. GTK `floe/hangul.py`와 `gui.py::_note_entry_key`에는 OS IME가 없는
폐쇄망용 두벌식 조합기가 있다. 이전 웹의 `isComposing` 보호만으로는 이 기능이
이관된 것이 아니었다. `rust/web/ui/hangul.js`의 ES2017 상태 머신으로 옮겨 note
편집기에 연결했다. 입력은 브라우저 로컬 일시 상태이며 서버/Python 왕복은 없다.

- 편집기를 열면 기본 off. `Built-in Hangul` 또는 Shift+Space로 켠다.
  HangulMode/HanjaMode와 기존 GTK 명칭 키도 받으며 Shift는 쌍자음 조합을 끊지 않는다.
  초성·중성·종성/복합 모음·겹받침·받침 이동과 단계별 Backspace를 보존한다.
- OS IME가 우선이다. composition lifecycle/`isComposing`/229를 확인하고,
  조합 중 Ctrl/Cmd+Enter·Escape가 preview/discard로 넘어가지 않게 한다.
  Ctrl/Meta/Alt 편집 단축키는 가로채지 않는다.
- UTF-16 offset으로 **현재 preedit 구간만** `setRangeText`로 교체한다. 전체
  textarea를 다시 쓰지 않는다. 커서/선택·다른 입력·붙여넣기/잘라내기/drop·blur·
  편집 잠금/재읽기에서 이전 조합을 확정하고, 닫기/종료는 모드도 초기화한다.
  programmatic insertion의 maxlength 우회를 별도로 막고 초과 키는 텍스트와
  조합 상태를 보존한 채 거부한다. 기존 UTF-8 64KiB preview 한도는 유지한다.
- setRangeText가 없으면 fallback만 비활성화하고 OS IME 경로를 남긴다. 새 runtime
  라이브러리/CDN/저장은 없다. 자산 hash와 고정 embedded asset 경로에 포함한다.

API 근거: [WHATWG text-control 선택/치환](https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-setrangetext),
[W3C IME key 값](https://www.w3.org/TR/uievents-key/#keys-ime). 지원 버전을 추정해
현장 호환으로 선언하지 않는다. 실제 브라우저의 caret 스크롤·OS IME 공존·native
undo grouping은 별도 입력 수용이며 deterministic fake textarea로 증명하지 않는다.

집중 검증:

- `validate_web_hangul.py`: 실제 Python `HangulComposer`와 현대 한글 **11,172자**,
  **22,744개 시퀀스·237,352번 전이**의 commit/preedit/pending을 대조. 조립 후
  단계별 삭제, 받침 이동·모음 연속·reset·고정 난수 입력, KEYMAP 전체가 일치한다.
- `hangul.test.cjs`: UTF-16 emoji prefix, 선택 치환/커서 이동/외부 수정, clipboard,
  modifiers, OS composition/229, 길이 상한 롤백, 비활성/닫기·지원 API 부재.
- 실제 DRC selection+notes 모듈 통합에서 `한글` 입력/6 UTF-8 bytes, **입력 중
  HTTP 0건**, IME 중 preview/discard 0건, 종료 시 조합/모드 정리를 단언한다.
- `node tools/validate_web_ui.cjs`: ES2017 구문과 전체 UI/상태 회귀 통과.
- `cargo clippy --offline --locked -p floe-web --all-targets --no-deps -j2 -- -D warnings`
  통과. 의존 VFS의 기존 dead-code 경고는 남으며 workspace 전체 lint 통과 주장은 아니다.
- 전체 `sh tools/validate_rust.sh`는 exit0 / `RUST VALIDATION: ALL OK`로 완료했다
  (`/private/tmp/floe-hangul-battery.log`). app22/core261/web84 단위,
  새 조합 대조와 DRC 저장/복구·전체 UI, occupancy27·잡덱83·렌더러46,
  VFS H1-H5/L1-L9 및 KLayout jobs1/8 각각13 PX+2 phase-exact+14 style을 통과했다.
- 마지막 동일 live-region 안내 중복 갱신 방지 보강 후에도 전체 UI/조합 oracle과
  native HTTP embedded-asset gate를 다시 통과했다
  (`/private/tmp/floe-hangul-ui-final.log`, `floe-hangul-assets-final.log`).
  실제 브라우저 수용은 미검증이다. 검증용 `.venv` 링크만 정리하며 원래 환경은 보존한다.

[G4 잔여 감사](WEBUI_G4_AUDIT.ko.md)를 추가했다. 사용자 결정은 자동 저장 opt-in
추가이며 이 커밋은 저장 동작을 바꾸지 않는다. 다음은 note 확정/waive 변경의
reviewer별 opt-in, reviewer 읽기 선택과 개발 도구 경계 정리다. 그 뒤에도 브라우저/
Linux 실제 실행·G1/G4 수용, 미구현 공유/원격, 보류 M0/M3, 조건부 M5가 남는다.
이 입력기 한 건의 완료를 전체 목표의 완료 또는 임의 퍼센트로 보고하지 않는다.

## 64. M4g-14 — reviewer별 확정 시 자동 저장 opt-in

2026-09-16. 사용자 선택에 따라 GTK의 확정 시 저장을 웹에 연결한다. 기본은 여전히
off이며 **편집 중 주기 저장**이 아니다. 서버의 기존 snapshot/prepare/승인 API,
reviewer·target·DRC/revision·CAS·원자 게시·receipt와 자원 admission은 변경하지 않는다.

```sh
floe2-web view layout.oas --drc checks.db.ice --drc-reviewer kim --drc-edit-waives
```

`--drc-reviewer`는 기존 note 쓰기 등록이고 `--drc-edit-waives`가 waive 쓰기를
추가한다. 등록/CLI 실행만으로 자동 저장을 켜지 않는다. Notes/Waives 패널의
`Automatically save confirmed … (this tab)`을 종류별로 명시적으로 켜야 한다.

### 동작과 동의 범위

- note: 선택 snapshot을 읽고 내용을 편집한 다음 `Save note` 또는 Ctrl/Cmd+Enter.
  prepare가 정규화한 정확한 문구에 대한 승인 요청을 이어 보낸다. 빈 문구 확정은
  선택 note를 지우는 기존 의미다. 입력/OS IME 조합/blur/단순 조회는 저장하지 않는다.
- waive: snapshot에서 `Waive`/`Clear waive`를 선택하면 저장한다. 새 편집을 여는
  canvas `w`는 현재 선택의 상태를 읽어 toggle→prepare→승인으로 연결한다.
  이미 열린 선택 편집기는 `w`로 덮어쓰거나 제출하지 않고 포커스만 옮긴다.
- 켜기 자체는 HTTP 요청을 만들지 않고, 이미 만들어 둔 미리보기를 승인하지 않는다.
  off에서는 기존 미리보기·별도 체크/승인 경로를 유지한다.
- 동의는 **탭·고정 reviewer·인증 세션·실제 연결 epoch**에 결합한 메모리 객체다.
  확정 시 포착한 객체와 승인 직전 객체가 같아야 한다. 해제→재활성화로 옛 준비가
  되살아나지 않는다. 바쁜 동안도 opt-out은 가능하나 새 opt-in은 막는다.
  정상 waive reader revision 변경은 동의를 해제하지 않는다.
- WebSocket 단절은 editor stop과 별개다. 실제 `drc.js` 연결 callback과 epoch를
  연결해 단절/재접속/다른 창 상태에서 새 opt-in을 요구한다. checkbox/설정/storage
  자동 복원은 없다. 기존 pending storage에도 문구나 opt-in은 넣지 않는다.
- legacy binding 확인과 note 파싱 경고 검토는 자동 승인하지 않는다. import/export와
  설계 공유 기본값 게시는 별도 동의다. reserved waive status는 명시한 Waive/Clear
  동작에 따라 기존처럼1/0으로 바뀌며 preview/receipt의 기존 계수 의미를 유지한다.

### 경합·실패·비용

snapshot 이후 선택/DRC/revision/connection/만료가 달라지면 제출하지 않는다.
사용자가 opt-out해도 이미 제출된 파일을 되돌렸다고 표시하지 않는다. 실패/충돌/
결과 불명은 기존 receipt로 표시하며 열린 편집기의 원문/선택을 보존한다. 종료/
pagehide의 기존 초안 삭제 계약은 그대로다. 단절에 의한 선택 무효화는 원문을
유지하지만 새 동의와 snapshot 없이 재저장하지 않는다. 읽기·refresh·재연결은
새 저장을 재시도하지 않으며 명시 Resolve만 동일 요청을 재전송한다.

카탈로그 `autosave:false`는 서버의 독자적 배경 저장 없음이다. UI가 사용자 확정에
대해 기존 `approve:true` 요청을 보내는 것이며 서버권한·RBAC를 UI boolean으로
대체하지 않는다. 세션의 등록된 reviewer 이외 이름/경로는 요청으로 받지 않는다.
한 저장 진행 중 추가 큐를 만들지 않는다. sidecar 전체 해시/재작성 비용은 그대로이며
GTK pwrite와 같은 성능 또는 대형 파일 연속 클릭 성능을 보장하지 않는다.

### 검증

- `review-save-mode.test.cjs`: 기본 off, reviewer/세션 분리, 객체 동일성,
  준비 중 opt-out, 해제→재활성화의 옛 동의 거부, checkbox 자동 복원이 동의가
  되지 않음, 초기화/저장 부재. 연결 상태 제공이 빠진 호출자도 opt-in 불가다.
- `review-autosave.test.cjs`: 실제 notes/waives 모듈에서 확정·단축키·IME,
  read/prepare/승인 전 GET 지연, 해제·선택/revision/만료/종료, legacy/파싱 경고,
  CAS 실패·이미 commit된 opt-out, 응답 유실/동일 요청 복구, 탭 분리.
- 기존 실제 DRC/notes/waives 통합은 reader fence/이전 callback 취소·일치하는
  revision까지 조회 정지·note 보존을 계속 검사한다. 여기에 실제 연결 false와
  새 epoch에서 두 opt-in이 모두 꺼지고 재연결로 복원되지 않는 단언을 추가했다.
- `validate_web_autosave.py`: private 합성 OASIS/DRC/ICE로 실제 Rust 서버와 실제
  JS 편집 모듈을 연결한다. 한글 note 파일 저장/재조회, waive→clear와 reader ACK,
  opt-out 후 수동 미리보기, 원본 source/pack/cache hash·mtime 불변, 종료를 검증한다.
  credential은 자식 stdin으로만 전달하고 출력에 없는지 검사한다. Node/가짜 text
  control은 개발 하네스이며 실제 브라우저 클릭/표시 수용이 아니다.
- 전체 UI ES2017/회귀와 위 집중 검사는 통과했다. `floe-web` all-targets
  `cargo clippy --offline --locked --no-deps -j2 -- -D warnings`도 통과했다.
  의존 VFS의 기존 dead-code 경고는 남으며 workspace 전체 lint 통과 주장은 아니다.
- 전체 `sh tools/validate_rust.sh`는 exit0 / `RUST VALIDATION: ALL OK`로 완료했다
  (`/private/tmp/floe-autosave-battery.log`). app22/core261/web84 단위, 실제
  note/waive 저장·복구 및 새 자동 저장 연결, 전체 UI/GTK 한글 대조, occupancy27·
  잡덱83·렌더러46, VFS H1-H5/L1-L9 및 KLayout jobs1/8 각각13 PX+2 phase-exact+
  14 style을 통과했다. 최종 연결 기본 거부/안내 보강 후에도 전체 UI·실제 Rust
  저장 연결과 embedded-asset HTTP 검사를 재실행해 통과했다
  (`/private/tmp/floe-autosave-ui-final.log`, `floe-autosave-assets-final.log`).
  checkbox 복원 불변식도 별도로 재검증했다. 실제 Linux/브라우저/현장 수용은
  별도이며 브라우저 도구 제한을 우회하지 않았다.

목표 잔여: 자동 저장 **로컬 실행 경로**를 연결한 단계다. 다음은 reviewer 읽기
선택의 별도 이관, 개발 bitmap 슬롯/CLI 경계와 G4 최종 대조다. 실제 브라우저의
입력·저장·복구/시각 수용, Python-free Linux 실행과 G1/G4 판정은 남는다. 공유/원격은
미구현, M0/M3 현장은 보류, M5 world-tile은 조건부다. 커밋 보고도 이 전체 잔여를
유지하며 이 opt-in 한 건을 전체 완료나 임의 완료율로 바꾸지 않는다.

## 65. M4g-15a — 명시 ICE reviewer의 읽기 전용 선택

2026-09-16. G4 잔여 중 reviewer 읽기와 쓰기 opt-in을 분리하는 첫 단계다.
GTK의 전체 자동 탐색을 이관 완료한 것으로 세지 않는다.

```sh
floe2-web view layout.oas --drc results.db.ice --floe-reviewer alice
```

- trusted CLI가 명시한 ICE와 인접 `.results.db.waive.alice`,
  `.results.db.notes.alice.fe`만 읽는다. 명시 reviewer는 엄격히 검증하며 환경변수나
  browser 요청의 tag/path로 바꾸지 않는다. 파일 부재는 빈 review이며 생성·수리·
  lock 생성·in-pack fallback을 하지 않는다. 기존 pack/sidecar 검증과 saved-note
  snapshot/바인딩·외부 변경 검사를 유지한다. readonly 파일의 atime 갱신은 OS 정책이다.
- 메모 배지·마지막 점프 오류 본문과 waive 상태/필터는 기존 reader UI로 표시한다.
  notes 서비스의 `editable:false`를 UI가 검사해 편집·자동 저장을 숨기고, 서버는
  편집 snapshot부터 prepare/submit/recovery/transfer/artifact까지 거부한다.
  GET notes status와 POST notes/display만 허용한다. 기존 로컬 저장 복구 기록을
  읽거나 replay/삭제하지 않는다. 자동 저장 checkbox 복원으로 권한을 얻지 못한다.
- `--drc-reviewer`·`--drc-edit-waives`·`--drc-waives`와 혼용은 명시 오류다.
  기존 writer 등록/자동 저장은 그대로이며 새 계정 인증·공유 권한은 아니다.
  독립 창으로 열고 기존 single-instance에 reviewer/write 설정을 전달하지 않는다.
- 시작 시 8-byte nonblocking regular-file probe로 명시 ICE만 허용한다. FIFO는
  대기하지 않고 거부한다. ASCII가 주어지면 아직 미이관인 cache 자동 선택을
  조용히 흉내 내지 않고 명시 ICE를 요구한다. 기존 CLI `open_current`와 안전 probe를
  공유하며 파서/renderer/캐시 포맷은 바꾸지 않는다.
- read-only notes는 처음 등록한 reader ID에 고정된다. 명시 rebuild로 reader가
  교체돼도 기존 review를 새 pack에 자동 연결하지 않는다. 재열기가 필요하다.
  index hot reload/revision 운영 정책은 여전히 사용자 유보 범위다.

범위 제한: legacy 임시 디렉터리의 waive/note fallback, `--drc ASCII`에서 fresh
ICE 자동 선택, env/host 기반 reviewer 기본 선택은 남아 있다. 이번 모드는 인접
파일만 읽는다는 점을 help·시작 안내에 표시한다. 임시 경로를 지원하려고 파일
picker/덱 source scope를 `/tmp` 전체로 넓히지 않는다. 과거 GTK의 자동 파일 생성·
손상 파일 덮어쓰기·in-pack 쓰기를 read-only 호환성으로 이관하지 않는다.

검증:

- app23/core261/web85 단위, strict clippy(app/web all-targets), ES2017/전체 UI 통과.
  실제 DRC+editor+display 모듈의 읽기 모드에서도 배지/본문/캔버스·점프 ACK·복원·
  pan 무조회·재연결을 검사하고 편집/자동 저장·transfer·복구 호출이 없음을 고정했다.
- `validate_web_read_reviewer.py`: 실제 Rust HTTP와 private 합성 자료. 기존 native
  승인 API로 만든 메모/waive, xattr 없는 **0444 legacy 파일**, 파일 없는 reviewer를
  각각 읽었다. 두 review API의 모든 editor/transfer/artifact 경로(정상 CSRF form
  download 포함)는 403, 미인증은 401이다. 원본/pack/cache·sidecar 내용/mtime/ctime/
  mode 불변, 새 sidecar/lock 없음, ASCII 명시 오류와 FIFO 비차단, 정상 종료를 확인했다.
- `sh tools/validate_rust.sh` **exit 0 / ALL OK**
  (`/private/tmp/floe-read-review-battery.log`). 기존 수동/자동 저장·DRC 전송, occupancy27,
  jobdeck83, renderer46, VFS H1-H5/L1-L9, KLayout jobs1/8 각각13 PX+2 phase-exact+
  14 style 통과. 최종 안내 문구 변경은 전체 UI와 embedded-asset HTTP 검사 및 release
  재빌드로 별도 검증했다(`floe-read-review-ui-final.log`, `floe-read-review-assets-final.log`).
- 추가 재검사에서 기존 native 자동 저장 하네스의 `!waives.suspended()` 단언이 한 번
  실패했다. 하네스는 요청 번호 일치 없이 마지막 terminal receipt를 썼고, UI refresh
  이후 별도 GET이 더 최신인 경우도 처리하지 않았다. 저장 제품 코드는 바꾸지 않고
  하네스에 **요청 seq 일치 + 실제 controller/reader 반영**을 넣었다. 두 번째 POST를
  첫 번째 성공 receipt 관측까지 보류하는 재현을 강제하고 5회 연속 통과했다.
  timeout 상향/재시도로 실패를 숨기거나 이전 성공을 새 성공으로 세지 않는다.

제품 runtime에 Python/Node를 추가하지 않는다. 브라우저 click/IME/화면 또는 현장
acceptance를 DOM/native HTTP 검사로 대체하지 않는다. renderer/index 버전과
공유 캐시 형식은 그대로다. main/feature/jobdeck 작업 트리는 수정하지 않았다.

목표 잔여: 명시 ICE 읽기 경로 하나를 닫는 단계이며 reviewer legacy 탐색은 남는다.
개발 bitmap 슬롯/CLI 제품 경계·G4 최종 대조, 실제 브라우저 입력/저장/복구/화면,
Python-free Linux 실행과 G1/G4 수용은 미완료다. M2 공유/원격은 미구현, M0/M3 현장은
보류, M5 world-tile은 조건부다. 전체 goal은 계속 active이며 임의 완료율을 보고하지 않는다.

## 66. M4g-16a — 개발 bitmap 슬롯의 source-side 계약 고정

2026-09-16. reviewer legacy 읽기 연결과 독립적으로 UI-03의 남은 슬롯 의미를
감사했다. 제품 모델/API/UI는 아직 미구현이며, 이번 커밋은 문서와 개발 오라클뿐이다.
세부 근거와 후속 수용 조건은 [슬롯 계약](WEBUI_BITMAP_SLOTS.ko.md)에 고정했다.

- GTK의 슬롯 참조는 bitmap 값과 다르다. 같은 값인 별도 슬롯, 미사용 슬롯 편집 후
  나중의 할당, 슬롯을 참조하는 모든 행의 갱신을 구분해야 한다. 선택 행 custom hex
  입력으로 이관 완료 판정을 내리지 않는다.
- `solid`/`clear`는 이름으로 고정하며 현재 슬롯18/19다. GTK 메뉴의 비활성화만
  복제하지 않고 후속 Rust 진입점에서도 검증해야 한다. `FLOE_FILL_EDIT`의 현재 웹
  공유 기본값 게시 opt-in을 슬롯 편집 자체의 동의와 혼동하지 않는다.
- 실제 GTK 편집 메서드의 Cancel/Apply·clear/solid/invert/reset·드래그 방향과 release,
  동일 bitmap 슬롯 격리·다음 할당, worker 전송을 inert widget과 합성 행으로 실행했다.
  `_props_rows`가 편집 bitmap 대신 이름만 내보내는 저장 손실도 명시했다. 웹의
  Native JSON/Calibre text 손실 거부 정책은 유지한다.

검증: `validate_palette_styles.py` exit0. 새 원본 계약324편집/80메뉴, 기존
GTK+adapter→Rust 스타일10,584개,49색/20bitmap 대조가 모두 통과했다
(`/private/tmp/floe-bitmap-contract-palette.log`). `node tools/validate_web_ui.cjs`
exit0/ALL OK로 자동 저장 opt-in/read-prepare-approve 경합과 전체 ES2017/DOM 회귀도
통과했다(`/private/tmp/floe-bitmap-contract-ui.log`). 원본 계약은 기존 palette 검사에
연결해 전체 배터리에 포함했지만 **이번 문서/테스트 전용 단계에서 전체 배터리를
재실행하지는 않았다**. 제품 변경·Rust 슬롯과의 parity·실제 browser 조작 PASS가 아니다.

목표 잔여: 슬롯 Rust 모델/API/UI·lossless 설정, reviewer legacy 읽기 연결과 G4 최종
대조, 실제 브라우저 입력/저장/복구/화면 및 Python-free Linux 실행은 남는다. M2 공유/
원격은 미구현, M0/M3 현장 검증은 보류, M5 world-tile은 조건부다. 단계를 세분화한
커밋 수를 전체 완료율로 바꾸지 않는다. main/feature/jobdeck은 수정하지 않았다.

## 67. M4g-16b — Rust 슬롯 참조와 lossless 설정 v2

2026-09-16. [슬롯 계약 §4](WEBUI_BITMAP_SLOTS.ko.md)의 native 모델/설정 단계다.
슬롯 편집 API·launcher 개발 opt-in·브라우저 편집기는 다음 단계이며, 이 커밋을
UI-03 전체 완료로 세지 않는다. reviewer legacy 읽기 권한 연결과는 독립적이다.

- `Assignments`가 직접 Fill과 이름 있는 슬롯 참조를 구별한다. 기존 complete style/
  delta/값 batch는 직접 값으로 남고 참조를 해제하며, color/width만 바꾸면 참조는
  유지한다. 시작/라이브 layerprops의 유효 fill 이름은 참조다. 내장 bitmap은 한 번
  파싱해 재사용하고 sparse 슬롯 override만 저장한다.
- native `StyleBatch.fill_slot`과 `Patch.fill_slot_edit`이 슬롯 할당·편집을 수행한다.
  같은 bitmap인 다른 슬롯/직접 값은 함께 바뀌지 않는다. 상속 자식을 포함해4096행을
  넘으면 전체 거부, 고정 solid/clear·무효 이름·설정/다른 style 명령과의 혼합도 거부한다.
  stale CAS는 기존 view controller에서 거부한다. unused 슬롯/동일 pixels의 참조만
  바뀌면 state revision만 바뀌며, 해석된 style이 바뀌면 render key와 margin이 무효화된다.
- Native JSON v2는 전체20슬롯 bitmap과 행별 `fill_slot`을 저장한다. 필수 `fill`과
  `width`의 기존 null/상속 의미는 유지하고 참조와 직접 값을 동시에 지정할 수 없다.
  unused 슬롯 편집도 보존한다. v1은 계속 읽고 직접 값으로 복원하며 slot override를
  초기화한다. 참조/override가 없으면 v1으로 내보내므로 기존 값 문서는 같은 형식이다.
- v2 누락/중복/고정 슬롯 변경/미정의 필드/잘못된 bitmap은 원자 거부한다. 기존 설정
  owner 인증·4MiB/65,536행·read-only prepare→승인/CAS 경로만 사용한다. 파일 경로/
  게시 권한은 추가하지 않는다. Calibre text가 custom bitmap을 손실 없이 표현하지
  못하면 계속 오류와 Native JSON 경로를 제공한다.
- renderer wire/cache 형식·Raster 규칙·renderer/index 버전은 바꾸지 않는다.
  현재 웹 프리셋은 여전히 값 기반이고 `view.set`의 새 slot 필드는 명시 거부한다.
  shared default 게시의 `FLOE_FILL_EDIT`와 별도 승인은 그대로다. 이후 API/UI가
  연결되기 전에 개발 editor가 이미 동작한다고 표시하지 않는다.

집중 검증:

- 실제 GTK 슬롯324사례의 전체 표/참조/해석 bitmap·나중의 접힌 그룹 할당과
  Native JSON 왕복을 Rust와 대조했다. 기존 GTK+adapter 스타일10,584개와49색/20패턴
  오라클도 통과했다(`/private/tmp/floe-fill-slots-palette.log`).
- workspace 단위 검사에서 새 core6검사(상속/직접 값·unused·v1·v2·4096/4097·
  controller CAS/margin)를 포함해 통과했다. 기존 reviewer native 미커밋 검사도
  실행된 작업 트리 기준이며 그 별도 변경은 슬롯 커밋에 포함하지 않는다.
- 실제 renderer15 PNG 쌍(기존13 + 슬롯 할당/편집2), owner HTTP의 v2 준비/승인/
  다운로드·Calibre 손실 거부·v1 복원 및 원본/cache 무변경이 통과했다.
- core/app/web all-targets strict clippy 통과
  (`/private/tmp/floe-fill-slots-clippy-core.log`; app/web 선행 검사는 `floe-fill-slots-clippy.log`).
  기존 tiler unused-mut/VFS dead-code warning은 범위 밖이며 숨기지 않았다.

최종 `sh tools/validate_rust.sh`는 **exit0 / RUST VALIDATION: ALL OK**로 끝났다
(`/private/tmp/floe-fill-slots-battery.log`). 위 오라클/실제 renderer·HTTP 검사와
전체 ES2017/UI·자동 저장/DRC 전송·occupancy27·잡덱83·렌더러46·VFS H1-H5/L1-L9,
KLayout jobs1/8 각각13 PX+2 phase-exact+14 style을 통과했다. 검증용 `.venv` 링크만
정리하며 원래 환경과 main/feature/jobdeck은 보존한다. 실제 브라우저 slot drag/
키보드·화면 수용이나 Python-free Linux 실행을 위 검사로 대신하지 않는다.

목표 잔여: 슬롯 API/UI·개발 모드 연결, reviewer legacy 읽기와 G4 최종 대조가 로컬
구현에 남는다. 실제 브라우저 입력/저장/복구/화면, Python-free Linux와 G1/G4 수용도
남는다. M2 공유/원격은 미구현, M0/M3 현장은 보류, M5 world-tile은 조건부다.
index hot reload/revision은 사용자 유보 범위다. 전체 목표는 아직 완료가 아니다.

## 68. M4g-16c — 슬롯 표 조회와 개발 opt-in/CAS API

2026-09-16. [슬롯 계약 §5](WEBUI_BITMAP_SLOTS.ko.md)의 transport 단계다.
Rust 모델을 임의 경로/파일 게시 없이 웹에서 호출할 수 있게 했다. 이 단계에서는
브라우저 프리셋과 편집기 DOM을 바꾸지 않았으며 **슬롯 UI 완료가 아니다**.

- snapshot에 sparse 슬롯 override의40hex 캐시 힌트를 추가했다. pan·색·참조 할당은
  키를 바꾸지 않고, 해시 비용은 최대18개 override에만 비례한다. 이 값은 인증/
  편집 revision이 아니다. 표가 예전 값으로 돌아와도 오래된 state CAS는 거부한다.
- owner 인증된 `GET /api/v1/views/{id}/fill-slots/{key}`는 전체20슬롯을 반환한다.
  잘못된 key/view는400/409/404, 미인증/cross-origin은401/403으로 거부한다.
  기존 compiled presets GET은 바뀌지 않으며, 조회로 render·state·파일을 수정하지 않는다.
- `style_batch.fill_slot`은 정상 스타일 할당으로 허용한다. 값 기반 `fill`과 혼합,
  null/미정의 이름은 거부한다. 다른 필드 변경이 참조를 의도 없이 해제하지 않는다.
- 전용 `view.fill_slot`만 슬롯 bitmap을 편집한다. launcher의 nonempty
  `FLOE_FILL_EDIT` opt-in이 없으면 `fill_edit_disabled`. 고정 슬롯·잘못된 bitmap·
  fan-out 초과는 기존 모델의 원자 거부, 연결/view/seq/state CAS는 기존 WS 경계다.
  일반 `view.set`/startup/open Patch에는 이 편집 필드를 넣지 않는다.
- 메모리 편집 capability는 파일 게시와 독립적이다. 공유 기본값 publish의 별도
  preview/승인은 유지하고, 기존 명시 Native JSON v2 load/save도 그대로다.
  서버 오류/거부를 UI가 자동 재전송하는 경로는 추가하지 않는다.
- renderer/index protocol·cache 형식·버전은 바꾸지 않는다. transport/schema는
  web bundle identity에 반영되며 새 모듈도 build hash에 포함했다.

집중 검증:

- core 슬롯 검사6개 통과(추가 cache hint 분리/기본 Reset/설정 왕복 포함), GTK 대조는
  독립 full battery에서 실행한다. core 로그 `/private/tmp/floe-slot-api-core.log`.
- web 단위86개/실제 transport14개 통과(`/private/tmp/floe-slot-api-web.log`).
- 실제 HTTP/WS+native renderer8개 통과(`/private/tmp/floe-slot-api-stream.log`).
  새 슬롯 gate는 opt-in on/off, 일반 참조 할당, 미사용 슬롯 무렌더, 직접 bitmap
  분리, 사용 슬롯 render key 변경, byte-exact 픽셀 복원, 고정/stale/다른 view/연결
  거부, read-only 기본 표와 원본/cache 무변경을 단언한다.
- core/app/web all-targets strict clippy 통과(`/private/tmp/floe-slot-api-clippy.log`).
  기존 tiler unused-mut/VFS dead-code 의존 경고는 범위 밖이며 숨기지 않았다.
- 기존 actual launcher gate에 env 빈 값과 nonempty `0`의 capability 차이를 추가했다.
  전체 배터리의 실제 CLI 실행에서 둘 다 통과했다.

첫 전체 배터리(`/private/tmp/floe-slot-api-battery.log`)는 기존 native palette batch
검사의 서버 종료에서 `view shutdown deadline exceeded`로 실패했다. 새 슬롯 검사와
해당 palette의 픽셀/원자성 검사는 통과했고 종료에서만 실패했다. 원인을 확정하거나
해결했다고 주장하지 않는다. 하네스에 실패 시 phase/완료 여부/자원·transport 사용량
진단을 추가했으며 production의4초/하네스6초 제한과 테스트 병렬도는 그대로 유지했다.

같은 native HTTP/WS8개 묶음을 같은 병렬 조건으로 연속3회 더 실행해 모두 통과했다
(`/private/tmp/floe-slot-api-stream-repeat1.log`~`repeat3.log`). 진단 변경 후 clippy도
통과했다(`/private/tmp/floe-slot-api-clippy-final.log`). 최종 전체
`sh tools/validate_rust.sh`는 **exit0 / RUST VALIDATION: ALL OK**로 완료했다
(`/private/tmp/floe-slot-api-battery-final.log`). app23/core270/web86 단위, GTK 슬롯324/
스타일10,584, 실제 controller15 PNG 쌍과 새 슬롯 HTTP/WS, launcher opt-in, DRC 수동/
자동 저장 및 전체 ES2017/UI, occupancy27/잡덱83/렌더러46, VFS H1-H5/L1-L9,
KLayout jobs1/8 각각13 PX+2 phase-exact+14 style을 통과했다. 단위 검사 수는 기존
reviewer 미커밋 검사도 포함된 작업 트리 기준이며 그 변경은 이 커밋에 포함하지 않는다.
첫 종료 실패의 원인은 아직 미확정이며 재발 시 위 진단으로 추적한다. 검사 삭제,
deadline 완화나 반복 실행 결과 중 실패를 숨기는 방식으로 통과 처리하지 않았다.

목표 잔여: 웹 프리셋을 슬롯 참조로 연결하고 세션 표·16×16 draft 편집·Apply/Cancel/
Reset·키보드/드래그와 stale 입력 거부를 이관해야 한다. reviewer legacy 읽기 연결은
별도 승인 대기 상태이며 그8개 native 미커밋 파일은 이번 단계에서 건드리지 않았다.
CLI 경계/G4 재감사, 실제 브라우저 입력·저장·복구/화면과 Python-free Linux/G1/G4
수용도 남는다. M2 공유/원격은 미구현, M0/M3 현장은 보류, M5 world-tile은 조건부,
index hot reload/revision은 사용자 유보다. 이 API 단계로 전체 목표를 완료 처리하지 않는다.

## 69. M4g-16d — 슬롯 참조 프리셋과 웹 bitmap 초안 편집

2026-09-16. [슬롯 계약 §6](WEBUI_BITMAP_SLOTS.ko.md)의 정적 프론트를 연결했다.
기존 Rust 슬롯 모델/API/설정 v2와 renderer의 resolved bitmap 계약은 바꾸지 않는다.
슬롯 UI의 로컬 기능 경로를 닫는 단계이며 **실제 브라우저 수용 완료가 아니다**.

- 채움 프리셋은 선택/접힘/상속을 Rust가 처리하는 `style_batch.fill_slot`을 보낸다.
  내장 기본 표와 현재 슬롯 표의 캐시를 분리했다. 현재 표의 키는 view/epoch/override
  힌트이며 pan revision만으로 재조회하지 않는다. 표가 아직 없거나 무효면 fill 미리보기와
  할당을 잠그고 수동 Retry를 제공한다. 실패한 GET을 자동 반복하지 않는다.
- launcher opt-in과 현재 표의 editable 응답 뒤에18개 개발 슬롯 선택기를 둔다.
  미사용 슬롯·레이어 무선택에서도 열 수 있고 swatch 우클릭도 제공한다. solid/clear는
  선택/편집하지 않으며 서버의 고정 슬롯·권한·fan-out 거부는 그대로다.
- 16×16 복사 초안에서 MSB-left 클릭/드래그, Clear/Solid/Invert/내장 Reset,
  Cancel/Apply를 제공한다. 방향키/Home/End·Space/Enter·grid Escape, roving focus와
  pointer/mouse fallback을 넣었다. capture 실패/cancel/blur 이후 stroke를 계속하지 않는다.
- Apply만 전용 WS 명령을 한 번 보낸다. 초안을 연 시점의 view/epoch/state CAS를
  전송 큐에서도 유지한다. ACK 뒤 authoritative snapshot이 완료 기준이며 stale를
  새 revision으로 바꾸거나 disconnect 후 자동 재전송하지 않는다. 미사용 슬롯의
  상태 변경은 기존 native no-render 의미를 유지한다.
- source/revision/연결 변경과 palette 닫기/pagehide 때 초안을 비운다. 이미 전송한
  요청은 커밋됐을 가능성을 안내하며 취소를 서버 rollback처럼 표시하지 않는다.
  입력 원문/bitmap을 storage에 저장하지 않는다. Apply는 파일을 쓰지 않으며 Native
  JSON 저장·공유 기본값 게시·DRC 자동 저장 opt-in은 계속 별도 경로다.
- 새 모듈은 binary에 포함하고 bundle identity에 넣었다. 새 외부 의존성·서버 파일
  경로·renderer protocol/cache 형식·제품 runtime의 Python/Node 의존성을 추가하지 않았다.

집중 검증:

- GTK 원본에서 실행한324편집 event trace를 웹 초안에 재생해 결과를 대조했다.
  같은324사례의 Rust 슬롯/Native JSON 대조, 기존10,584스타일과49색/20패턴도 통과했다
  (`/private/tmp/floe-slot-ui-palette.log`). AST 이벤트를 자체 JS 예상값으로 바꿔
  정답을 정의하지 않는다. GTK 메서드의 실제 결과가 오라클이다.
- ES2017 parse와 전체 DOM/client gate 통과(`/private/tmp/floe-slot-ui-dom-final.log`).
  실제 `app.js` 연결 검사는 선택 없이 열기, 로컬 Cancel의 무전송, 단일 Apply,
  ACK/snapshot 대기, unused 결과의 슬롯 표 재조회, pan 시 재조회 없음,65ms 전송 큐
  중 revision 변경에도 원래 CAS 보존, stale 거부/재연결 시 무재전송과 정리를 단언한다.
  pointer capture 실패의 별도 회귀 검사도 통과했다.
- 웹 all-targets strict clippy는 기존 단계와 같은 `-- --no-deps -D warnings` 범위로
  통과했다(`/private/tmp/floe-slot-ui-clippy-web.log`). 의존성까지 clippy를 적용한
  첫 실행은 변경하지 않은 Oasis의 기존13경고로 실패했다
  (`/private/tmp/floe-slot-ui-clippy.log`). 이를 workspace 전체 clean으로 보고하지 않는다.
  tiler unused-mut/VFS dead-code 경고도 유지하며 이번 UI 범위에서 수정하지 않았다.
- scoped rustfmt와 diff-check 통과. 처음 발견한 자산 테스트의 줄바꿈은 rustfmt로
  정리했다. 기존 reviewer native 미커밋8파일은 수정하거나 이 단계에 포함하지 않는다.

최종 `sh tools/validate_rust.sh`는 **실제 exit0 / RUST VALIDATION: ALL OK**로 완료했다
(`/private/tmp/floe-slot-ui-battery.log`). app23/core270/web86 단위, GTK324편집/
10,584스타일, HTTP/WS+native renderer8개, 명시 opt-in의 실제 note/waive 저장과
재조회, 전체 ES2017/DOM/client, VFS H1-H5/L1-L9, occupancy27/잡덱83/렌더러46,
KLayout jobs1/8 각각13 PX+2 phase-exact+14 style을 통과했다. 이전 단계의 native
palette 서버 종료 실패는 이번 실행에서는 재현되지 않았으며 원인 해결로 주장하지 않는다.
단위 수는 별도 reviewer 미커밋 검사를 포함한 작업 트리 기준이다. 보호한8파일의 diff는
검증 전후 동일하며 이번 커밋에는 포함하지 않는다. 검증용 `.venv` 링크만 제거하고
main의 기존5항목 변경과 feature/jobdeck 작업 트리는 보존했다.

목표 잔여: 슬롯 UI는 로컬 연결했으나 실제 브라우저 pointer/키보드·포커스/화면을
검증해야 한다. reviewer legacy 읽기 연결은 별도 승인 대기이며 CLI 진단/제품 경계와
G4 최종 재감사도 남는다. 실제 브라우저 저장/복구·G1/G4 수용, Python-free Linux 실행은
로컬 DOM/native gate로 대체하지 않는다. M2 공유/원격은 미구현, M0/M3 현장은 보류,
M5 world-tile은 조건부, index hot reload/revision은 사용자 유보다.

## 70. M4g-17a — GTK 원본 대조와 합성 표시 진단

2026-09-16. [표시 진단 계약](WEBUI_DISPLAY_DIAGNOSTICS.ko.md)을 추가했다.
`gtktest`의 합성 이미지/위젯 비교와 `--dump`의 수신/합성 파일 저장은 다른 기능이다.
합성 진단의 실제 UI 경로부터 연결했으며, 입력 PNG·독립 명령·자동 dump까지
완료했다고 세지 않는다. 기존 GTK와 사용자 데이터/경로 접근 범위는 그대로다.

- About의 **Run display test**에서만 고정 색 막대 PNG/raw를 인증 GET한다.
  view/worker/file을 열지 않고, GET에는 파일명/좌표/사용자 옵션이 없다.
  raw230,416바이트와 PNG16KiB 미만을 한 번 생성해 재사용한다. 기존 HTTP 제한과
  owner cookie/CSRF·host/origin 정책을 그대로 적용한다.
- `protocol.imagePayload`와 `image-decode.js`를 실제 viewer/진단이 공유한다.
  raw `ImageData`, PNG `Image+Blob`·dimension 검사·5초 제한·취소/URL 정리가
  공통이다. live frame envelope와 stale/CAS/ACK·credit 소유는 기존 app.js에 남긴다.
  진단용 가짜 frame/query identity를 발행하거나 renderer protocol을 바꾸지 않는다.
- 세 panel은 PNG/raw와 `.viewport`의320×128 crop/투명 십자 overlay다.
  결과는 readback의 다른 픽셀 수이며 DPR에 맞춰 CSS 표시 크기를 조정한다.
  `desktop_acceptance:unverified`를 유지하고 사용자의 화면 관찰은 별도 필드다.
  OS/ETX 화면, 전체 PNG 변종·실칩/native raster, 입력 지연을 검사한 것으로 확대하지 않는다.
- About 열기/재접속/재열기로는 실행하지 않는다. 한 실행의 read→decode 경합,
  Cancel/닫기/pagehide에 요청·image URL·canvas·보고서를 정리한다. 자동 재시도,
  보고서 업로드/storage 저장/다운로드와 view 변경을 추가하지 않았다.
- CLI의 `gtktest` 명시 오류는 합성 대안 위치와 남은 입력 PNG/GTK 범위를 안내한다.
  새 자동 dump나 GTK 명령 폐기/alias는 없다. `--dump`의 서버 임시 파일 연속 쓰기와
  브라우저 최근 이미지 보관/명시 다운로드 중 선택은 사용자 응답 대기다.

집중 검증:

- `validate_display_test.py`: 실제 GTK `synth`/`fill_rect`의57600픽셀을 native
  raw·Pillow로 디코드한 native PNG와 정확히 비교했다. 같은 native fixture를 Node
  pixel-canvas에 전달해 PNG/raw/crop 결과를 확인했다(`/private/tmp/floe-display-oracle.log`).
  Python/Node는 개발 검사이며 제품 런타임에 포함되지 않는다.
- 공통 image decoder의 explicit start·한 번 완료·invalid/mismatch/timeout/cancel/
  늦은 callback과 URL 회수, 전체 ES2017/UI/client gate 통과
  (`/private/tmp/floe-display-ui.log`). 실제 app.js/About 연결에서 명시 인증 GET과
  view 명령 무발행·닫기/pagehide도 검사했다. 실제 브라우저 실행/스크린샷 검사는 아니다.
- 실제 HTTP15개 통과(`/private/tmp/floe-display-http-final2.log`). 처음 새 검사에서
  CSRF 누락을403으로 기대했으나 기존 `http_session` 계약은401이므로 기대값을
  바로잡았다. 인증 정책을 완화한 것이 아니다. 별도 초기 컴파일의 하네스 종료 메서드
  오기도 기존 `shutdown` 호출로 고쳤다. 최종 검사는 같은 전체 transport 묶음이다.
- app/web all-targets scoped strict clippy(`-- --no-deps -D warnings`) 통과
  (`/private/tmp/floe-display-clippy.log`). 기존 tiler unused-mut/VFS dead-code 경고는
  유지한다. 새 크레이트/외부 의존성을 추가하지 않았다. scoped rustfmt와 diff-check 통과.

최종 `sh tools/validate_rust.sh`는 **실제 exit0 / RUST VALIDATION: ALL OK**로 끝났다
(`/private/tmp/floe-display-battery.log`). app23/core270/web87 단위, HTTP15개,
GTK57600픽셀/native PNG/raw/웹 crop oracle, GTK 슬롯324/스타일10,584, 기존
native HTTP/WS8개·취소/재접속·owner 설정과 자동 저장, 전체 ES2017/UI/client,
VFS H1-H5/L1-L9·마커 복구, occupancy27/잡덱83/렌더러46, KLayout jobs1/8 각각
13 PX+2 phase-exact+14 style을 통과했다. 단위 수는 별도 reviewer 미커밋 검사도
포함한 작업 트리 기준이다. 그8파일의 diff는 전후 동일하며 커밋에 포함하지 않는다.
검증용 `.venv` 링크만 정리하고 main과 feature/jobdeck의 기존 상태를 보존했다.
실제 브라우저 수용은 실행하지 않았으며 위 단위/DOM 결과로 대체하지 않는다.

목표 잔여: 입력 PNG와 독립 표시 진단 명령·dump 정책, reviewer legacy 읽기 연결
(별도 승인 대기), CLI/G4 최종 재대조가 로컬에 남는다. 실제 브라우저 입력/저장/복구/
화면, Python-free Linux와 G1/G4 수용은 위 합성/DOM/native 검사로 대체하지 않는다.
M2 공유/원격 미구현, M0/M3 현장 보류, M5 world-tile 조건부, index hot reload/revision
사용자 유보 범위를 유지한다. 한 진단 기능 추가를 전체 목표 완료로 세지 않는다.

## 71. M4g-17b — index/renderd 없는 독립 표시 진단 명령

2026-09-16. `floe2-web displaytest`와 전용 정적 페이지를 추가했다. 기존 empty
workspace도 설계 없이 열 수 있지만 owner service/worker 발견과 자원 설정은 거치므로,
표시 자체의 진단에는 별도의 worker 없는 실행 경로를 둔다. 입력 PNG/`--dump`는
아직 미이관이며 이 명령을 기존 `gtktest`의 완전한 대체나 GTK 은퇴로 세지 않는다.

- `--no-open`, `--port`, `--firefox`, `--session-file`만 받는다. private session JSON의
  `mode:display-test` 외에 사용자 파일 경로를 HTTP로 등록하지 않는다. layout/index/
  renderd/DRC/기본 workspace IPC에 접근하지 않는다. `--no-open`이면 Firefox 발견도 없다.
- 기존 SessionFile의 create-new0600/자신이 만든 inode만 정리하는 규칙과 Browser의
  독립0700 Firefox 프로필/0600 시작 파일/별도 process group 수거를 재사용한다.
  인증 URL은 기존처럼 private 파일에만 있고 argv/stderr에는 없다.
- 전용 root HTML과 ES2017 bootstrap은 기존 cookie+CSRF/one-use fragment 교환을
  사용한다. fragment를 주소에서 먼저 제거하고, bundle/자격/`display_only` capability를
  확인한다. 같은 탭 재로드는 sessionStorage 자격만 사용한다. 저장소 거부 시 재로드
  한계를 표시하며 다른 저장소로 우회하지 않는다. 자동 test·WS·view 명령은 없다.
- 명시 Run은 기존 합성 검사와 공통 image decoder를 사용한다. Quit는 기존 확인
  대화상자와 DELETE session이다. 실패/불확실한 종료는 성공으로 표시하거나 재전송하지
  않는다. pagehide 취소/BFCache 정지와 report 폐기, bootstrap120초/session8시간 만료,
  Ctrl+C/소유 Firefox 종료 수거를 구분한다. 수동 `--no-open` 탭 종료는 서버 종료가 아니다.
- Gateway의 만료 관리는 display-only일 때 owner worker 없이도 종료한다. 일반 뷰어의
  service 수명주기·API·native 프레임 경로는 변경하지 않는다. 두 HTML과 새 JS를
  같은 content bundle에 포함한다. 새 외부 의존성은 없다.

검증 진행 기록: app24/web88 단위(별도 ignored 제외), 새 만료 검사, ES2017/전체 DOM
및 기존 client gate, `validate_display_cli.py`의 실제 Rust HTTP/empty-PATH/없는 native
worker/로그아웃/SIGINT/파일 보존과 가짜 Firefox 자발적 종료·프로필 정리 통과.
실제 브라우저는 실행하지 않았으며 Node DOM/가짜 Firefox를 화면 수용으로 세지 않는다.
최종 `sh tools/validate_rust.sh`는 **실제 exit0 / RUST VALIDATION: ALL OK**로 끝났다
(`/private/tmp/floe-display-standalone-battery.log`). app24/core270/web88 단위,
HTTP15·native HTTP/WS8, GTK57600픽셀 PNG/raw/crop 대조, 새 독립 CLI, reviewer
자동 저장의 실제 파일 저장/재조회, ES2017/전체 DOM, VFS H1-H5/L1-L9/중단 후 마커
복구, occupancy27/jobdeck83/renderer46, KLayout jobs1/8 각각13 PX+2 phase-exact+
14 style을 통과했다. 실제 브라우저/현장 수용은 하지 않았다.

app/web all-targets scoped strict clippy(`-- --no-deps -D warnings`), scoped rustfmt,
diff-check도 통과했다. 기존 의존 크레이트와 GTK 개발 게이트의 경고는 유지한다.
검증용 `.venv` 링크만 정리했으며 main/feature/jobdeck와 별도 reviewer 수정8파일의
diff를 보존했다. 위 core 단위 수는 그 별도 미커밋 검사도 포함한 작업 트리 기준이다.
그8파일은 이번 커밋에 포함하지 않는다.

커밋 시 목표 잔여: 입력 PNG 진단·`--dump` 방식 결정, reviewer legacy 읽기 연결의
별도 승인, CLI/G4 최종 재대조. 실제 브라우저 입력/저장/복구/화면과 Python-free Linux,
G1/G4 수용은 별도다. M2 공유/원격 미구현, M0/M3 현장 보류, M5 world-tile 조건부와
index hot reload/revision 사용자 유보도 남는다. 독립 진단 하나로 전체 완료를 선언하지 않는다.

## 72. M4g-17c — CLI가 선택한 정적 PNG의 불변 표시 진단

2026-09-16. `floe2-web displaytest [PNG]`에 선택 PNG 한 개를 연결했다. 기존 GTK의
선택 PNG→360×160 BILINEAR 기능을 웹의 명시 Show/공통 decoder/Canvas smoothing으로
옮기되 보간 픽셀 동일성과 GTK 위젯·애니메이션 재생을 약속하지 않는다.

- CLI 인자로만 파일을 고른다. no-follow/nonblocking regular-file open, 읽기 전후
  identity/metadata 검사와80MiB/8192축/16Mpx/65536 chunks 한계, PNG envelope/CRC
  검사를 거쳐 Vec를 immutable Bytes로 넘긴다. 원본 변경/삭제 후에도 이미 열린
  세션의 응답은 같으며 파일을 다시 읽거나 수정하지 않는다. FIFO·APNG·trailing data와
  malformed envelope는 명시 오류다. IDAT 샘플의 실제 디코딩 실패는 웹이 표시한다.
- 기존 `annotations/png.rs`의 chunk scanner를 공유한다. 기존 read/edit 경로는
  annotation 해석/원래 trailing-data 보존 정책을 유지한다. display만 annotation을
  해석하지 않는다. scanner 분기 회귀 검사와 기존 fe-embed oracle로 확인한다.
- 새 API는 인증된 고정 `/api/v1/display-test/input`, metadata는 width/height/bytes뿐이다.
  파일 경로나 이름을 받지 않고 새 업로드/쓰기 권한도 없다. 원본 PNG metadata는 함께
  전송하므로 익명화 기능이 아니다. 일반 view/About는 input이 없어404를 유지한다.
- 별도 **Show input PNG** 버튼 전에는 bytes를 읽지 않는다. 공통 image decoder로
  dimensions/timeout/blob lifecycle을 검사하고360×160으로 smoothing/alpha 표시한다.
  alpha readback 수와 사용자 관찰을 별도 보고하되 desktop_acceptance는unverified다.
  파일명/경로/픽셀/annotation 텍스트는 보고서에 담지 않는다. Cancel/Quit/pagehide와
  read→decode 경계에서 취소하며 실패·재열기 때 자동 요청을 재생하지 않는다.
- 입력 없는 합성 진단과 일반 layout/native frame 경로는 유지한다. 추가 의존성이나
  Python 런타임을 넣지 않았다. 테스트의 Pillow/Node는 개발 오라클/대역이다.

초기 검증:11개 PNG 형식(팔레트1/2/4/8, gray/alpha/RGB/RGBA/16-bit/진짜 Adam7)의
actual CLI/HTTP 원본 byte와 Pillow 픽셀 대조, 인증·파일 삭제 후 snapshot 유지·손상 CRC/
크기/APNG/trailing/FIFO 거부, 기존 독립 CLI, shared decoder/Canvas 연결 DOM과
전체 ES2017/UI gate를 통과했다. 실제 브라우저 디코딩/보간/화면 수용을 한 것은 아니다.
최종 전체 재실행(`/private/tmp/floe-display-input-battery-rerun.log`)은 **실제 exit0 /
RUST VALIDATION: ALL OK**였다. app24/core271/web88 단위, HTTP15/native HTTP·WS8,
입력 PNG11형식·합성57600픽셀·기존 fe-embed byte/metadata, 전체 ES2017/DOM/저장/복구,
VFS H1-H5/L1-L9/마커 복구, occupancy27/jobdeck83/renderer46, KLayout jobs1/8 각각
13 PX+2 phase-exact+14 style을 통과했다. core/app/web all-targets scoped strict clippy
(`-- --no-deps -D warnings`)와 scoped rustfmt·diff-check도 통과했다. 기존 dependency/
GTK 개발 게이트 경고는 유지한다. 실제 브라우저/현장 수용을 뜻하지 않는다.

검증 관찰: 최초 전체 배터리(`/private/tmp/floe-display-input-battery.log`)는 기존
`instance::tests::wire_lost_ack_and_replays_do_not_repeat_the_handler`의 `already owned`
1회 실패로 **exit101**이었다. 해당 검사20회(`/private/tmp/floe-display-input-instance-repeats.log`)
와 app-core 전체 병렬 검사3회(`/private/tmp/floe-display-input-core-repeats.log`)는
같은 작업 트리에서 모두 통과했다. 원인은 미확정이며 IPC 코드/테스트는 바꾸지 않았다.
단발 실패를 환경 문제로 단정하지 않고 [G4 §4](WEBUI_G4_AUDIT.ko.md)에 추적한다.
최종 재실행 통과는 이 잠금 실패 원인의 규명/수정으로 계산하지 않는다.

검증용 `.venv` 링크만 정리했다. main/feature/jobdeck 상태와 별도 reviewer8파일의
기존 diff는 보존하며 이번 커밋에 포함하지 않는다. 위 core 검사 수는 해당 미커밋
검사도 포함한 작업 트리 기준이다.

목표 잔여: `--dump` 정책, GTK 진단/애니메이션 PNG의 제품 경계, reviewer legacy 읽기
승인·연결, CLI/G4 최종 대조. 실제 브라우저 입력/저장/복구/화면·Python-free Linux·G1/G4,
M2 공유/원격, M0/M3 현장, M5 world-tile 조건부, hot reload/revision 사용자 유보를
이 입력 기능 하나로 완료 처리하지 않는다.

## 73. M4g-18 — IPC 소유자 종료와 fork→exec 잠금 수명

2026-09-16. §72의 단발 `already owned` 실패 후 소유자 종료 경계를 검사했다.
기존 `Owner::drop`은 자기 socket을 정리하고 File의 close에만 의존했다. 같은 open
description을 상속한 자식이 fork→exec 구간에 있으면 close-on-exec가 아직 실행되지
않아 flock이 남는다. macOS SDK의 `flock(2)` 명세와 두 독립 재현에서 확인했다.

- dup 단위 검사: owner가 살아 있을 때 중복 claim은 Running, owner를 drop한 직후
  복제 FD가 남아 있어도 새 owner를 획득해야 한다. 이전 FD를 나중에 닫아도 새 owner의
  잠금은 유지돼야 한다. 수정 전 `already owned`로 exit101, 수정 뒤 통과했다.
- native 검사: 실제 자식의 pre-exec hook을 미리 만든 Unix socket 두 쌍으로 동기화한다.
  child ready→부모 owner drop→재획득 시도→child exec 허용 순서를 고정한다. fork 뒤
  hook은 timeout이 설정된 FD의 read/write와 errno 처리만 수행하고 allocation/formatting/
  Rust mutex를 쓰지 않는다. CLOEXEC는 끄지 않으며 assertion 전에 자식을 수거한다.
  수정 전 잠금 잔류로 exit101, 수정 뒤 기존 lifecycle 전체와 함께 통과했다.
- owner Drop은 자기 socket inode 정리 뒤 살아 있는 FD로 `LOCK_UN`을 수행한다.
  EINTR만 재시도하고, persistent lock inode는 지우지 않는다. timeout/불명확한 socket을
  재획득 허가로 바꾸거나 replay ledger·epoch·build/UID 규칙을 완화하지 않는다.
- fork된 Owner 복사본은 creator PID가 다르면 자기 FD만 닫는다. 부모 socket 삭제/
  shared flock unlock은 금지하며 PID를 파일에 저장하거나 신호 대상으로 사용하지 않는다.
  단위 검사는 이 destructor 분기를 모사해 부모 측 복제 FD와 socket이 보존됨을 확인한다.
- SIGKILL 등 Drop 자체를 실행하지 못하는 종료에서, 아직 exec하지 않은 자식의 참조까지
  즉시 없애는 보장은 아니다. claim 도중 Owner 생성 전에 실패하는 경로의 File도 기존
  close 수명을 유지한다. 기존 fail-closed 동작을 임의 대기로 숨기지 않는다.

최초 배터리 단발 실패에는 backtrace가 없어서 첫 claim과 재시작 claim 중 어느 쪽인지
확인할 수 없다. **위 결함 재현/수정과 최초 실패의 인과 확정을 구분**한다.
테스트 owner helper에 `track_caller`를 추가해 재발 시 호출 위치를 기록한다.
별도 reviewer 수정8파일은 보존하며 이번 단계에 포함하지 않는다.

집중 검증: instance 단위12개(별도 GTK oracle ignored1), native IPC lifecycle 전체,
동기화한 pre-exec 검사10회와 app-core 전체 병렬 검사3회(각273 passed/7 ignored),
app-core/web/app all-target scoped strict clippy(`-- --no-deps -D warnings`) 통과.
Rust1.89에서도 instance 단위/native lifecycle을 실행해 통과했으며 app-core all-target
Linux x86-64 musl check도 통과했다. Linux에서 실행한 결과는 아니다.
로그는 `/private/tmp/floe-ipc-lease-clippy.log`, `floe-ipc-lease-msrv.log`,
`floe-ipc-lease-linux-check.log`다.

전체 `sh tools/validate_rust.sh`는 **실제 exit0 / RUST VALIDATION: ALL OK**로 끝났다
(`/private/tmp/floe-ipc-lease-battery.log`). app24/core273/web88 단위, 새 native IPC
회귀와 기존 HTTP/WS·CLI 수명주기, reviewer 자동 저장의 실제 note/waive 게시·재조회와
opt-out, ES2017/UI·표시 진단, VFS H1-H5/L1-L9·마커 복구, occupancy27/jobdeck83/
renderer46, KLayout jobs1/8 각각13 PX+2 phase-exact+14 style을 통과했다.
scoped rustfmt·diff-check도 통과했으며 기존 dependency/GTK 개발 게이트 경고는 남는다.
실제 브라우저 또는 Linux/현장 실행을 검증한 것은 아니다.

검증용 `.venv` 링크만 정리했다. main의 기존5항목 변경과 feature/jobdeck worktree,
reviewer8파일의 diff를 보존했다. core 검사 수는 별도 reviewer 미커밋 검사도 포함한
작업 트리 기준이며 그 파일들은 이 커밋에 넣지 않는다. renderer/protocol은 바꾸지 않는다.

커밋 시 목표 잔여: `--dump`/GTK 진단의 제품 경계, reviewer legacy 읽기 승인·연결과
CLI/G4 최종 재대조. 실제 브라우저 입력/저장/복구/화면·Python-free Linux·G1/G4,
M2 공유/원격 미구현, M0/M3 현장 보류, M5 world-tile 조건부와 hot reload/revision
사용자 유보는 별도다. 이번 잠금 수정으로 전체 전환 완료를 선언하지 않는다.

## 74. M4g-19 — 휠 확대율·대기 중 입력·표시 앵커

2026-09-16. G4 재대조에서 실제 GTK `_on_scroll`은 `0.96 ** delta`에 delta를
[-1,1]로 제한하고 `_pending`/pan/band/button-held 동안 휠을 버리는 반면, 웹은
매번0.8 또는1.25를 적용하며 deltaY=0도 축소하고 렌더 중64개 용량의 큐에 입력을 쌓는
차이를 확인했다. 기존 단위/DOM gate에 wheel 호출 자체가 없어 이 차이를 못 봤다.

- 휠은 GTK와 같은 이벤트당 배율 상한0.96(확대)/1÷0.96(축소)을 적용한다.
  보고된 Y가1 미만이면 분수를 유지한다. 0/가로 전용 이벤트, 비유한 값·잘못된 mode/
  pointer 좌표·버튼 동시 입력은 명령을 만들지 않는다. 키보드/버튼의0.8/1.25와
  Ctrl+Z/Shift+Z는 변경하지 않는다. GPU/새 renderer·worker protocol 변경은 없다.
- 연결·idle·현재 크기에 맞는 final 표시 frame/ACK가 있고 입력 큐가 비었을 때만
  휠을 받는다. 초기 화면 전, edit ACK만 도착한 상태, foreground 렌더/PNG decode,
  오래된 frame, resize frozen base, drag, hidden/disconnect·owner/index 작업 중에는
  버린다. 버린 입력을 재연결/렌더 완료 후 누적 실행하거나 재시도하지 않는다.
- 착지한 margin crop도 현재 frame으로 인정한다. background margin prefetch는
  foreground phase를 바꾸지 않으므로 휠을 막지 않는다. geometry가 불완전하다는
  이유만으로 입력을 영구 차단하지 않으며 query capability와 navigation을 구별한다.
- 앵커는 CSS bounding box 전체가 아니라 실제 canvas가 시작하는 device-aligned
  inset·DPR·render pixels로 정규화한다. world 좌표 확대는 기존 Rust `Viewport`가
  수행한다. 이벤트가 viewport 밖으로 이어지면 앵커를 가장자리로 제한한다.

**이벤트 정책과 물리 감도는 다르다.** [W3C Pointer Events §12 Wheel Events](https://www.w3.org/TR/pointerevents4/)는
pixel/line/page delta의 실제 크기를 장치·OS·앱 설정에 맡긴다. 여기서는 mode를
검증하되 각 mode의 보고 단위를 동일한 물리 회전각이라고 가정하지 않고 이벤트
상한과 단위 미만 분수만 적용한다. pixel↔line 변환 계수·가속 보정·관성 감지 같은
미측정 보정을 추가하지 않는다. GTK smooth delta와 숫자가 같을 때 정책을 대조한
것이지, 같은 손동작이 모든 Firefox/Chrome에서 같은 줌 거리를 만든다는 보장이 아니다.
그 감도·이벤트 빈도와 실제 input→photon/pacing은 G1/G2 실측에 남는다.

집중 검증: `validate_web_wheel.py`는 GTK 실제 `_on_scroll`과 `WHEEL_ZOOM_STEP`을
AST로 읽어137개 finite smooth/discrete/button 조합을 웹과 비교한다. pending/drag
무호출도 GTK 원본으로 고정한다. Python/Node는 개발 오라클일 뿐 제품 의존성이 아니다.
추가 단위는 DOM3모드·0/NaN/큰 delta·DPR1/1.25/1.5/2/3·fractional inset을 확인한다.
실제 app.js 하네스는 수정 전 첫 frame 전 wheel에서 실패했으며, 수정 후 first-frame/
ACK/렌더/idle-old-frame/decode/resize/drag/hidden/disconnect,100회 burst의 비재생,
margin crop·background 비차단과 키보드 줌 불변을 통과했다. 전체 ES2017/UI도 통과했다
(`/private/tmp/floe-wheel-ui.log`). app-core/web/app all-target strict clippy도 exit0
(`/private/tmp/floe-wheel-clippy.log`). 전체 `sh tools/validate_rust.sh`는 실제 exit0,
`RUST VALIDATION: ALL OK`로 끝났다(`/private/tmp/floe-wheel-battery.log`).
자동 저장 native/HTTP, jobdeck83, KLayout13 PX+2 phase-exact+14 style도 통과했다.
기존 reviewer8파일의 미커밋 diff는 그대로 보존하고 이번 커밋에 포함하지 않는다.

같은 검증 진행 중 사용자 결정 두 건을 받았다. 유도된 legacy note/waive sidecar만
읽기는 승인됐고, dump는 브라우저 최근 프레임/합성 화면 보관·명시적 다운로드로
확정됐다. 이번 휠 변경에 그 구현을 섞지 않으며 새 쓰기/임의 경로 권한을 뜻하지 않는다.

커밋 시 목표 잔여: CLI/dump 구현·GTK 진단 정책과 승인된 reviewer legacy 읽기 연결,
G4 최종 대조. 실제 브라우저/Linux/G1/G4 수용, 공유/원격 권한 경로의 승인·구현,
M0/M3 현장 보류와 M5 world-tile 조건부는 계속 남는다. hot reload/revision은 사용자
유보 범위다. 휠 정책 대조를 전체 UI parity나 현장 성능 합격으로 확대하지 않는다.

## 75. M4g-20 — 승인된 legacy 임시 sidecar 읽기

2026-09-16. 사용자는 선택한 DRC pack과 reviewer에서 정확히 유도되는 임시
note/waive 파일의 읽기만 승인했다. M4g-15a의 `view --drc PACK.ice --floe-reviewer TAG`
읽기에 이 후보를 연결한다. 기존 `--drc-reviewer` 쓰기 모드와 자동 저장 opt-in은
그대로이며 이 변경이 쓰기 동의나 reviewer 자동 발견을 뜻하지 않는다.

- note와 waive 각각 **인접 파일 우선, 없을 때 유도된 임시 파일**, 둘 다 없으면
  기존 인접 대상의 빈 메모/pack 내 waive 상태를 사용한다. 경로를 시작 시 고정하고
  열린 탭에서 새 후보를 계속 검색하지 않는다. 잘못된 인접 파일을 임시 파일로
  대체하거나 stale 파일을 이동/삭제/수정하지 않는다.
- 이름은 GTK `notes_autosave_path`/`_notes_tmp_fallback`,
  `waive_autosave_path`/`_waive_tmp_fallback`과 같다. hash는 pack의 lexical abspath를
  SHA-1한 앞12자리다. source 경로나 canonicalized symlink 이름으로 바꾸지 않는다.
- `ReadTargets`와 읽기 전용 store는 pack/reviewer에서 유도되는 두 이름을 다시
  검증한다. 일반 AccessScope/browse root를 확장하지 않는다. 임시 디렉터리 전체를
  허용하거나 HTTP 요청에 경로/reviewer 선택 필드를 만들지 않는다.
- read-only store는 초안 생성·import·게시를 native 층에서도 거부한다. 파일은
  디렉터리 FD에 대한 `O_NOFOLLOW` 읽기로 열며 단일 hardlink·regular file·크기·pack
  binding을 검증한다. 기존 미확인 legacy 표시는 유지한다. lock도 만들지 않는다.
- waive는 이 검증으로 포착한 FD를 이미 열린 DRC에 설치한다. 확인 뒤 pathname을
  다시 열지 않는다. 초기 조회도 기존 자원 예약과 취소 경로를 사용하고 읽기 lease는
  DRC 수명 동안 유지한다. note display의 외부 변경·pack 교체 시 재open 요구는 유지한다.
- 초기 waive snapshot은 전체 파일 hash와 상태 검증을 수행한다. 큰 legacy 파일의
  cold-open 비용은 실측 과제이며 기존 explicit-waive header 읽기와 동일 비용이라고
  주장하지 않는다. 웹 요청당 다시 전량 로드하는 구조는 아니다.
- 기존 미커밋 native 준비 중 read-only store/managed store/거부 gate4파일을 검토해
  필요한 기반으로 포함했다. 별도 ASCII/cache 선택의4파일은 수정·커밋하지 않는다.
  이번 경로는 여전히 명시 ICE만 지원하며 fresh ICE 자동 선택은 다음 과제다.

집중 검증: `validate_web_read_reviewer.py`는 별도 임시 디렉터리(소스 root 밖)의
합성 파일과 GTK 실제 경로 함수를 사용한다. 수정 전 release는 `temporary-read`의
메모가 없다고 반환해 exit1(`floe-legacy-read-red.log`), 수정 후 동일 HTTP 검사는
통과(`floe-legacy-read-http.log`)했다. 인접/임시/mixed/없는 reviewer·다른 hash,
손상 임시 파일보다 인접 파일 우선, symlink/끊어진 symlink/FIFO/hardlink 거부,
외부 메모 변경409, 경로 DTO와 모든 쓰기/전송 거부, 파일/pack/cache 불변을 검증한다.
단위는 임시 read-only store가 초안·import·위조 draft 게시를 거부함과 임의 이름
거부를 고정한다. 웹 단위89통과/3오라클별도, scoped strict clippy exit0.
추가 browse 단언은 기존 루트 이름의 번호 접두사를 반영한 뒤 통과했고 임시 root가
등록되지 않음과 경로로 탐색할 수 없음을 확인했다. 전체 `sh tools/validate_rust.sh`는
실제 exit0, `RUST VALIDATION: ALL OK`로 끝났다
(`/private/tmp/floe-legacy-read-battery.log`). 새 읽기 gate·기존 자동 저장·jobdeck83·
KLayout13 PX+2 phase-exact+14 style이 함께 통과했다. 선택 파일 rustfmt와 diff 검사도
통과했다. 별도 ASCII/cache4파일·main·feature/jobdeck 상태는 보존했다.

커밋 시 목표 잔여: reviewer ASCII/cache 선택, 승인된 브라우저 dump 구현과 CLI/G4
마감. 실제 브라우저 입력/저장/복구·Linux 실행·G1/G4, 공유/원격 승인·구현,
M0/M3 현장 보류·M5 world-tile 조건부는 별도이며 hot reload/revision은 사용자 유보다.
이번 읽기 연결을 전체 웹 전환 완료로 세지 않는다.

## 76. M4g-21 — 명시 reviewer의 ASCII/현재 ICE 선택

2026-09-16. `view --drc RESULTS.db --floe-reviewer TAG`에서 GTK `load_db`와
같이 현재 인접 `RESULTS.db.ice`를 선택한다. ICE를 직접 주는 기존 경로도 유지한다.

- native `select_review`는 기존 `open_current`와 캐시 판정을 공유한다. 원본 size와
  **초 단위 mtime**이 pack header와 같아야 한다. 내용 전체 hash나 source revision
  수명주기를 새로 도입하지 않는다. 같아도 같은 초·같은 크기의 외부 교체까지 검출하는
  보장은 없다. hot reload/revision 설계는 계속 사용자 유보 범위다.
- 캐시가 없거나 stale/corrupt/retired/nonregular이면 원본 ASCII로 돌아간다. 유도된
  note/waive는 붙이지 않고 화면 summary에 미적용 이유를 표시한다. 캐시/sidecar를
  생성·수리·이동·삭제하지 않는다. FIFO probe는 nonblocking regular-file 검사다.
- launcher는 원본/정확한 인접 후보의 scope와 metadata reservation을 먼저 검사한다.
  scope 밖 symlink는 fallback으로 은폐하지 않고 명시 거부한다. 임시 sidecar 읽기는
  M4g-20의 고정 유도 대상만 유지하며 임의 경로·browse/쓰기 root를 추가하지 않는다.
- actor가 선택된 파일을 실제 열 때 format 및 원본/cache 일치를 재검사하고, sidecar/
  rules 준비가 끝난 뒤 한 번 더 검사한다. 원본과 선택 cache의 managed read lease와
  다른 export/기본값 쓰기 경로의 보호를 reader 수명 동안 유지한다. HTTP는 내부 경로나
  cache 오류 원문 대신 `review_cache` 상태값만 받는다.
- 명시 reviewer가 없는 실행과 `--drc-reviewer` writer는 기존 explicit-file 의미다.
  ambient `FLOE_REVIEWER` 태그를 자동 선택하거나 읽기를 쓰기 승인으로 바꾸지 않는다.
  ASCII fallback 뒤 명시 build가 만든 새 reader에 reviewer를 자동 승계하지 않는다.
- selection과 actor open이 각각 pack metadata를 검증한다. 대형 cache/legacy waive의
  cold-open 비용은 실측 과제이며 테스트 fixture 시간을 현장 성능으로 환산하지 않는다.

검증: native source 변경/취소/유도 경로 테스트, 실제 HTTP의 fresh/mtime stale/
size stale/corrupt/retired/missing/FIFO/directory/scope escape와 fractional ASCII,
기존 legacy 읽기·쓰기 차단·입력/파일 목록 불변을 확대해 통과했다. UI simulated DOM은
ICE 선택과 ASCII fallback 안내를 확인한다. scoped release 단위(app24/core273/web89),
strict all-target clippy와 rustfmt/diff 검사도 통과했다. 초기 빌드에서 crate-private
UTF-8 helper 사용 오류를 수정하고 재검증했다. HTTP/UI/정적 검사 로그는
`/private/tmp/floe-read-cache-{http,ui,clippy,release}.log`다.

전체 `sh tools/validate_rust.sh`는 실제 exit0, `RUST VALIDATION: ALL OK`로 완료됐다
(`/private/tmp/floe-read-cache-battery.log`). 확장된 캐시 읽기와 기존 자동 저장·
jobdeck83·renderer46·KLayout13 PX+2 phase-exact+14 style을 모두 통과했다.
실제 브라우저/현장 수용은 이 결과에 포함하지 않는다. main의 기존 수정과
feature/jobdeck 작업 트리는 건드리지 않았으며 검증용 `.venv` 임시 링크만 제거했다.

커밋 시 목표 잔여: 승인된 브라우저 dump 구현과 진단/무효 CLI 경계, G4 최종 대조.
실제 브라우저 입력/저장/복구·Python-free Linux·G1/G4 수용, 공유/원격 승인·구현,
M0/M3 현장 보류·M5 world-tile 조건부는 별도다. 이 단계로 전체 goal을 완료 처리하지 않는다.

## 77. M4g-22 — 승인된 브라우저 display dump

2026-09-16. 사용자 결정대로 `view --dump`를 서버 `/tmp` overwrite가 아닌
브라우저의 최근 수신 프레임/합성 화면 보관·명시 다운로드로 연결했다.
정확한 사용·수명·비용 계약은 [표시 진단 §4](WEBUI_DISPLAY_DIAGNOSTICS.ko.md)다.

- CLI는 독립 workspace를 열고 보관의 초기값만 capability로 전달한다. About의
  명시 토글로도 켤 수 있으며 off에는 복사하지 않는다. 숫자 `--render-debug`,
  Save view PNG, 공유 파일 게시 권한과 구별한다. IPC 요청으로 켤 수 없다.
- 정상 디코드되어 수용된 foreground/margin frame 한 장과 현재 viewport 합성 한 장을
  각각 frozen canvas로 보관한다. crop·이전 foreground·보이는 query/DRC/ruler 포함,
  비동기 주석 paint 알림은 rAF1개로 합친다. flush는 paint 알림을 재발행하지 않는다.
  동일 시점의 한 쌍·완료 프레임·OS/ETX 화면의 screenshot이라고 부르지 않는다.
- bitmap2장(각 최대16Mpx), 동시 PNG encoder1개·retry blob1개(80MiB)를 제한한다.
  PNG는 다운로드 클릭 때만 인코드한다. clear/opt-out/view close/change/pagehide/
  종료 후 늦은 callback은 저장하지 않으며 실제 encoder callback까지 credit을 유지한다.
  수신 시 복사와 overlay flush는 비용이 있으므로 정상 benchmark에서는 끈다.
- HTTP 쓰기/이미지 경로/API·서버 파일·storage·clipboard read·자동 다운로드가 없다.
  다운로드 파일 자체는 설계와 주석을 포함할 수 있음을 UI에 명시한다. 실브라우저
  저장 완료 여부는 알 수 없으며 retry 버튼은 같은 frozen PNG를 다시 요청한다.

집중 검증: 독립 pixel-canvas 하네스가 received의 원크기와 margin/foreground/overlay
합성, 후속 canvas 변경에 대한 snapshot 불변,100회 교체의 보관 상한, off 무복사,
자동 인코드/다운로드 없음, 취소·늦은 callback·단일 encoder·실패/80MiB 거부·URL
정리를 확인했다. 실제 app.js 하네스는 CLI 초기 보관, raw/PNG·margin, stale/실패 제외,
비동기 overlay 알림, 네트워크 명령 없음과 view close/pagehide 정리를 통과했다.
초기 close 단언 실패는 테스트 대역이 dump 시나리오의 DELETE view를 지원하지 않은
것으로 확인해 해당204 응답을 보완했다. 제품의 실패 응답을 성공으로 치환한 것이 아니다.
전체 ES2017/UI(`floe-dump-ui.log`), app24/web89 단위·release build
(`floe-dump-native.log`), strict clippy(`floe-dump-clippy.log`) 및 실제 CLI/HTTP의
초기 capability/default off·자산·기존 렌더/종료(`floe-dump-http.log`)가 통과했다.
로그는 모두 `/private/tmp/`에 있다. 첫 전체 배터리는 `validate_app_cli.py`에 남은
이전 `--dump` 미이관 오류 단언에서 exit1이었다. 승인된 새 동작에 맞춰 잘못된 값
거부/서버 비저장 help 검사를 유지하고, 활성화의 실제 CLI/HTTP gate는 그대로 둔다.
최종 전체 `sh tools/validate_rust.sh`는 `RUST VALIDATION: ALL OK`까지 완료됐다
(`/private/tmp/floe-dump-battery.log`). 새 dump/CLI/HTTP 게이트와 기존 jobdeck83·
renderer46·KLayout13 PX+2 phase-exact+14 style(j1/j8)을 모두 통과했다.
최종 strict all-target clippy도 exit0(`floe-dump-final-clippy.log`)이며 scoped
rustfmt/diff 검사를 통과했다. 실제 브라우저/현장 수용은 이 결과에 포함하지 않는다.
main의 기존 수정과 feature/jobdeck 작업 트리는 보존하고 검증용 `.venv` 임시 링크만
제거했다. 기존 가상환경이나 설계 파일은 삭제하지 않는다.

커밋 시 목표 잔여: GTK 진단/애니메이션 PNG·무효 CLI 경계와 G4 목록 최종 대조.
실제 브라우저 입력/저장/복구·dump/화면·Python-free Linux·G1/G4 수용은 별도다.
공유/원격 승인·구현, M0/M3 현장 보류, M5 world-tile 조건부도 남아 있으며 index
hot reload/revision은 사용자 유보다. 이번 단계로 전체 goal을 완료 처리하지 않는다.

## 78. M4g-23 — GTK 메뉴 기반 G4 완료 감사 정정

2026-09-16. `e48e91f` 이후 원래 메뉴와 실행 경로를 다시 대조했다. 공개 CLI/전체
회귀 green만으로는 실행 중 조작의 누락을 잡지 못했다. `_drc_open_dialog`,
`_drc_rules_dialog`, `_jobdeck_reselect_levels`에 해당하는 웹 경로가 없다.
초기 CLI 등록·현재 등록의 Reload/Build·일반 Open·Mode는 이 세 경로를 대체하지
못한다. 구체적 근거와 다음 구현 경계는 [G4 메뉴 감사](WEBUI_G4_MENU.ko.md)다.

`validate_web_menu_inventory.py`는 원본 AST의42개 호출/39개 handler를 분류하고
웹 control/JS/test 링크와 신규/제거 callback을 검사한다.36개 연결·2개 제외·1개
기존 무효·3개 미구현이다. 링크/새 handler/누락 control/누락 test 결함 주입도
검증한다. 기본 exit0는 inventory의 정합성뿐이며 실제 동작 수용이 아니다.
`--require-complete`는 현재3건을 출력하고 **의도대로 exit1**이다. 배터리에는
inventory 모드만 연결하며 원래 미완료를 숨기거나 전체 G4 PASS로 바꾸지 않는다.

검증: inventory와 결함 주입, 완료 모드의3건 거부, `sh -n tools/validate_rust.sh`,
`git diff --check`를 실행했다. 이번 변경은 감사 도구/문서/배터리 호출만이며 제품
Rust/JS 코드는 변경하지 않았다. 전체 배터리는 이전 §77의 `e48e91f` 구현 검사
기록을 보존하고 이번 감사 변경에서 재실행했다고 보고하지 않는다. 실제 브라우저
합성 세션 열기/다운로드 검증의 추가 승인은 요청 상태이며 이전 도구 제한을
우회하지 않았다. main과 feature/jobdeck 작업은 그대로다.

커밋 시 목표 잔여 정정: 우선 실행 중 DRC 등록/교체·SVRF metadata 교체, 카메라
유지 jobdeck 레벨 재선택의 구현·gate·UI 연결이다. 진단/무효 CLI 경계 최종 결정,
실제 브라우저·Python-free Linux·G1/G4·현장 수용, 공유/원격 승인·구현과 조건부
M5도 남는다. 감사표 작성이나 연결 검사만으로 이 기능들이 완료됐다고 세지 않는다.

## 79. M4g-24a — 실행 중 DRC 교체의 선행 게시 보호

2026-09-16. 새 DRC/SVRF를 등록하기 전에, 이미 살아 있는 기본값/리뷰 writer와
그 writer가 준비한 초안도 새 입력을 덮어쓰지 못하게 해야 한다. 기존 시작 시 고정
보호 목록은 이 요구를 충족하지 않아 공유 `SourceSet`의 registration에 게시 금지
metadata를 추가했다. **이번 단계에는 실행 중 파일 선택·교체 API/UI가 없다.**

- immutable DB/SVRF/원본 파일·현재/미래 pack tree는 기본값과 리뷰 게시를 모두
  금지한다. 정확히 유도된 reviewer sidecar/lock은 기본값 게시만 금지한다. 기존
  리뷰 writer의 고정 scope/target/reviewer·승인/CAS 계약이 계속 쓰기 권한을 결정한다.
  이 목록은 deny-only이며 임의 파일 읽기·탐색·색인·쓰기 권한을 추가하지 않는다.
- 등록은 기존 sidecar publication reservation과 상호 배제한다. 성공 commit에만
  목록이 설치되고 취소/drop은 rollback한다. 파일 I/O는 state mutex 밖에서 한다.
  기본값/리뷰 초안은 publication lease를 잡은 뒤 새 목록을 다시 검사하므로 등록 전
  준비된 초안도 예외가 아니다. 읽기 전용 legacy snapshot은 이 쓰기 금지로 막지 않는다.
- 정규화 경로·기존 symlink prefix·파일 identity alias를 검사한다. 아직 존재하지
  않는 pack 하위 경로도 보호하며 실제 publisher의 부모 디렉터리 검사는 유지한다.
  외부 프로세스의 모든 filesystem mutation을 막는 OS sandbox라고 주장하지 않는다.
- 목록은 종류별로 중복 제거한 세 목록의 합계1024개로 제한하고 세션 동안
  append-only로 보존한다.
  예전 입력 보호를 임의로 풀지 않으며 상한 초과는 명시 오류다. 계속 다른 파일을
  등록하는 장기 세션은 이 상한에 도달할 수 있다. reader retirement와 연계한 회수는
  이번 단계 범위가 아니다. 개별 실패 batch는 앞서 유효했던 metadata를 지우지 않는다.
- `Gateway::attach_drc_registry`와 초기 reviewer 등록에 연결했다. 기존 fixed 보호
  목록도 유지한다. 런타임 coordinator는 향후 실제 취소 토큰과 승인 폴더 검사,
  새 reader 공개 경계에서 이 primitive를 사용해야 한다. 현재 helper의 초기 등록
  호출을 런타임 교체 구현으로 세지 않는다. managed index/export와의 충돌·수명,
  in-flight 저장/불명 receipt, 오래된 query/selection 폐기도 후속에 남아 있다.

집중 검증: core278통과/7오라클별도, web90통과/3오라클별도 및 기존 integration 통과. 추가 registry
검사는 fixed Publisher를 교체하지 않고 새 DB/rules/waives 입력이 기존 기본값 초안을
차단함을 확인했다. core gate는 미게시 등록 비노출·취소 rollback·source 추가 뒤
보호 보존·1024 상한/중복·hardlink/symlink·미래 pack 경로·옛 기본값/note/waive
초안의 target/lock 충돌·정상 허용 writer/읽기 유지·파일 생성 없음이다. 첫 회귀에서
미생성 pack 부모 때문에 Io가 먼저 반환되는 것을 찾아 deny-only 경로 검사를
planned-prefix 방식으로 고친 뒤 통과했다. 실제 게시 부모 검증은 느슨하게 하지 않았다.
로그는 `/private/tmp/floe-drc-rebind-protection-unit.log`와
`/private/tmp/floe-drc-rebind-registry-unit.log`다. strict all-target clippy와 선택 파일
rustfmt/diff 검사도 통과했다. 전체 `sh tools/validate_rust.sh`는 실제 exit0,
`RUST VALIDATION: ALL OK`로 끝났다
(`/private/tmp/floe-drc-rebind-protection-battery.log`). 기존 DRC note/waive·자동 저장·
읽기 reviewer·SVRF·웹 HTTP/UI, jobdeck83·renderer46·KLayout13 PX+2 phase-exact+
14 style이 함께 통과했다. 메뉴 inventory 기본 검사는 통과하지만 `--require-complete`는
미구현3건으로 의도대로 exit1이다. 실제 브라우저/현장 수용을 이 결과에 포함하지 않는다.
main의 기존 변경과 feature/jobdeck 작업은 보존했고 검증용 `.venv` 임시 링크만
제거했다. 기존 가상환경과 설계 파일은 삭제하지 않았다.

커밋 시 목표 잔여: 우선 실행 중 DRC 최초 등록/교체의 수명·권한·API/UI·실제 HTTP
gate, 다음 SVRF 교체와 카메라 유지 레벨 재선택이다. 진단/무효 CLI 경계 최종 결정,
실제 브라우저·Python-free Linux·G1/G4·현장 수용, 공유/원격 승인·구현과 조건부
M5도 남는다. 이 선행 보호 단계로 G4 미구현3건이나 전체 goal을 완료 처리하지 않는다.

## 80. M4g-24b — 현재 레이아웃을 유지한 읽기 전용 DRC 열기

2026-09-16. Source의 `Open DRC results…`는 레이아웃을 다시 열지 않고 현재
workspace에 DRC를 처음 등록하거나 교체한다. 승인 폴더의 opaque 파일 handle만
받는 기존 picker를 재사용한다. DRC 모드에는 일반 모드가 숨기는 ICE 파일도
표시하되 디렉터리 캐시·숨김 파일·symlink는 계속 제외한다. raw path, reviewer,
write target은 요청 필드가 아니다.

- `open_drc`는 선택 시작 시의 view ID·DRC ID/revision을 고정한다. 최초 등록은
  DRC identity가 null이다. 오래된 context는 거부하고 새 reader가 준비되는 동안
  기존 reader/레이아웃 카메라·레이어·renderer를 유지한다. 초기/기존 reader가
  오류 또는 종료 상태여도 같은 identity로 다른 파일을 선택할 수 있다.
- 현재 인접 ICE를 검증해 선택하고 stale/corrupt/missing이면 원본 ASCII로 읽는다.
  직접 선택한 ICE는 그대로 검증한다. ambient reviewer/legacy sidecar는 조회하지
  않고 색인을 암묵 실행하지 않는다. 기존 별도 Build pack 승인 경로는 유지한다.
- 후보 준비의 파일 I/O는 picker actor에서 한다. 후보1개·draining reader 최대2개,
  open 대기300초로 제한한다. native/NFS blocking call의 강제 중단을 보장하지는
  않는다. 취소/실패 후보도 종료 요청 후 실제 actor가 끝날 때까지 예약을 보유한다.
  picker drop은 block된 actor를 무기한 join하지 않으며 workspace 종료 검사는
  기존 bounded shutdown에 포함한다.
- picker의 취소/receipt lock→registry→현재 view→note/waive admission의 경계에서
  입력 게시 보호를 설치하고 새 reader를 공개한다. 선택/그룹/CD/prepared focus는
  폐기한다. 진행 중 저장·preparation·transfer는 교체를 거부한다. 이미 끝난 저장의
  immutable receipt와 동일 요청 replay는 보존한다. 늦은 취소가 성공한 교체를
  cancelled로 바꾸지 않는다. 실패/취소가 기존 reader를 지우지 않는다.
- 이전 reviewer는 detached로 남아 결과 조회/미확인 승인 복구만 가능하다. 새
  snapshot·편집·transfer를 막고 UI의 자동 저장 opt-in도 해제한다. 미승인 local
  초안은 무효로 표시하며 다른 DB로 보내지 않는다. 새 DRC의 reviewer 재등록·
  읽기/저장 권한·자동 저장 opt-in 연결은 **후속 구현**이다. 이 한계 때문에
  G4-MENU-01을 완료로 바꾸지 않는다.
- 이전 기본값/리뷰 초안의 동적 보호뿐 아니라 DRC/SVRF/sidecar가 등록된 layout
  cache 또는 index lock과 충돌하는 경우도 양쪽 등록 순서에서 거부한다. 실패한
  batch는 보호 목록에 일부만 설치하지 않는다. cache 재생성 권한으로 입력을
  지우는 우회를 허용하지 않는다. append-only 보호1024개 한계는 §79 그대로다.
- 브라우저 journal은 submit 전 저장하고, 재접속은 receipt GET부터 시작한다.
  불명 요청은 명시적으로 같은 요청만 재시도한다. DRC 성공 receipt를 재생하더라도
  그 안의 옛 catalog를 설치하지 않고 현재 등록을 다시 조회한다.

집중 검증: core279/web92 단위 테스트와 전체 ES2017/UI가 통과했다. 실제 HTTP
`validate_web_drc_open.py`는 DRC 없는 시작, 현재 cache/ASCII/직접 ICE, raw path·
reviewer 거부, stale context·손상 pack·취소/성공 경계, 이전 ID 거부, 저장 receipt
유지·미승인 초안 거부·입력 fingerprint 불변·layout 상태 불변·종료 수거를 확인한다.
단위 테스트의 오류 reader 교체 검사는 표시용 phase보다 실제 revision 변경 flag를
봐야 함을 잡았고 수정 후 통과했다. 기존 UI의 엄격한 review DTO에 detached 상태와
불명 승인 복구를 추가했다. 입력이 없는 registry는 build:null로 내보내 기존
build capability의 string source_id 계약도 유지한다.

전체 `sh tools/validate_rust.sh`는 exit0, `RUST VALIDATION: ALL OK`로 끝났다
(`/private/tmp/floe-drc-runtime-battery.log`). jobdeck83·renderer46·KLayout13 PX+
2 phase-exact+14 style(j1/j8)을 포함한다. 그 뒤 마지막 DOM 구조 점검에서 detached
note의 부모 authoring 영역이 receipt/복구 버튼까지 숨기는 문제를 고쳤다. 쓰기는
차단한 채 이전 local text·receipt를 보이며 reload 후에도 부모가 보이는 gate를
추가했다. 이 보완 후 전체 UI gate·release build·실제 CLI/HTTP·DRC 교체 HTTP·
strict all-target clippy를 다시 통과했다. pack 생성은 별도 승인 후에만 실행되고
승인 replay가 재색인을 하지 않는 실제 HTTP 검사도 추가해 통과했다. 이 집중
재검증을 전체 배터리를 한 번 더 실행한 것으로 표현하지 않는다.
집중 로그는 `/private/tmp/floe-drc-runtime-{unit,ui-final,build-final,cli-final,http-final,clippy-final}.log`다.
메뉴 inventory와 diff 검사는 통과하며 `--require-complete`는 미완결3건으로
의도대로 exit1이다. 검증용 `.venv` 임시 링크만 제거했으며 대상 가상환경은 보존했다.

실제 브라우저는
사용자가 합성 세션 열기/다운로드를 승인한 후 Chrome에서 재시도했으나 시작 파일이
브라우저 URL 정책으로 다시 차단됐다. 다른 URL·CDP·대체 surface로 우회하지 않았다.
합성 전용 서버와 비공개 시작 파일을 정리했고 실제 PNG 다운로드는 **미검증**이다.
HTTP/DOM gate를 실제 브라우저 수용으로 세지 않는다. main과 feature/jobdeck은
변경하지 않았다.

커밋 시 목표 잔여: 우선 새 DRC의 명시적 reviewer 재등록/저장 opt-in, SVRF 교체,
카메라 유지 jobdeck 레벨 재선택이다. 진단/무효 CLI 경계 최종 결정, 실제 브라우저·
Python-free Linux·G1/G4·현장 수용, M2 공유/원격 승인·구현과 조건부 M5도 남는다.
index hot reload/revision 관리는 사용자 유보다. 전체 goal은 계속 진행 중이다.

## 81. M4g-24c — 런처 범위 reviewer 명시 재연결

2026-09-16. 사용자 결정: **브라우저에서 reviewer·저장 권한을 새로 선택하지 않고,
런처에서 허용한 reviewer와 권한만 새 DRC에 명시 재연결한다.** M4g-24b의 파일
선택은 여전히 읽기 전용이다. `Reconnect launcher reviewer…`의 별도 확인 checkbox와
버튼이 필요하다. `--floe-reviewer`는 읽기 전용, `--drc-reviewer`는 notes 편집,
`--drc-edit-waives`는 별도 waive 편집 권한을 유지한다. 런처 grant가 없는 실행에는
버튼/새 권한이 생기지 않는다. 새 DRC가 ASCII뿐이면 먼저 별도 Build pack을 승인해
ready ICE로 만든다. runtime 재연결에 reviewer 입력란이나 임의 쓰기 경로는 없다.

- 기존 picker actor/operation journal의 `reconnect_drc_review {seq,context,approve}`를
  사용한다. view·DRC ID·revision을 동결하고, path/reviewer/editable/autosave 같은
  추가 필드는 거부한다. 재접속은 receipt GET부터, 재전송은 동일 요청만 허용한다.
  성공 receipt를 다시 받아도 그 안의 옛 reader를 설치하지 않고 현재 catalog를
  조회한다. 취소와 commit은 같은 picker lock에서 순서가 정해진다.
- 입력·유도 sidecar/lock 보호 등록과 후보 reader의 준비를 먼저 끝낸다. 기존
  layout 카메라/레이어/renderer는 유지한다. 실패한 후보/잘못된 sidecar/오래된
  context가 현재 DRC나 detached review 상태를 바꾸지 않는다. 성공 시 새 read
  identity와 note/waive binding을 같은 commit 경계에서 공개한다. 기존 query/
  selection/group/CD/prepared focus는 무효화된다. 후보1개·retired reader2개와
  300초 준비 대기/실제 종료까지 자원 보유는 §80과 같다.
- reviewer/권한/보호 roots는 immutable launcher Config에 남긴다. 바뀌는 것은
  검증한 reader ID·읽기 전용 target·opaque binding ID뿐이다. notes-only는 다른
  waive sidecar를 읽지 않는다. waive writer의 기존 상태는 **인접 write target만**
  guarded descriptor로 읽으며, legacy 임시 파일은 자동 채택하지 않는다. 읽기
  전용은 이전에 승인된 정확한 pack/reviewer 유도 sidecar 범위만 읽는다.
- review worker/저장·transfer ledger를 교체하거나 순번을 초기화하지 않는다.
  완료 receipt·high-water·만료된 순번 거부가 모두 유지된다. 각 receipt는 최초
  admission의 `scope_id`를 보존해 새 binding의 결과로 다시 이름 붙이지 않는다.
  새 note/waive status는 새 `binding_id`와 증가한 `review_rev`를 내보낸다.
  이전 승인 재전송은 이전 receipt이며 새 파일에 저장하지 않는다.
- 준비 중 review read/write/transfer가 있으면 재연결을 거부한다. 이전 미승인 token과
  다운로드 capability는 폐기하고 저장/전송 영수증은 남긴다. 화면에서는 이전
  등록의 receipt임을 표시한다. 아직 확인되지 않은 승인 복구를 숨기지 않는다.
  미승인 local text는 무효로 남기며 다른 DRC로 옮겨 저장하지 않는다.
- 브라우저 자동 저장 opt-in은 새 binding에서 반드시 해제한다. 중간 detached
  catalog를 놓친 탭도 binding 변경으로 해제하고, 늦은 옛 status로 되돌아가지
  않는다. 재연결 자체는 note/waive 저장·전체 import·다운로드가 아니다.

집중 검증은 web 단위94개(별도 fixture3개 ignored), 전체 ES2017/UI, release build,
strict all-target clippy가 통과했다. 실제 `validate_web_drc_open.py`는 grant 없는
거부, 읽기 전용/notes-only/waives 세 정책, raw 권한 필드 거부, stale/실패 보존,
새 DRC 저장, 이전 저장/transfer receipt 재생, 순번·epoch 보존, 읽기 전용 note
표시, symlink sidecar 거부, 입력 fingerprint/레이아웃 불변과 종료 수거를 검증했다.
UI gate는 명시 동의·journal-before-send·잃어버린 ACK의 GET 복구, 이전 receipt
표시, opt-in 초기화(중간 detach 누락 포함), old catalog 거부를 고정했다.

전체 `sh tools/validate_rust.sh`는 exit0, `RUST VALIDATION: ALL OK`로 완료됐다.
core279/web94 단위 테스트, jobdeck83·renderer46·KLayout13 PX+2 phase-exact+
14 style(j1/j8)을 포함한다. 로그는 `/private/tmp/floe-review-reconnect-battery.log`다.
배터리 실행 중 추가 점검으로 이전 binding의 outcome-unknown receipt가 새 DRC의
saved-note 조회까지 막던 UI 조건을 보완했다. 이전 경고는 receipt로 보존하며 새
binding 조회는 허용한다. 마지막 UI 보완 후 전체 UI·release build·strict all-target
clippy·실제 CLI·DRC 교체/재연결 HTTP를 별도로 재검증했다. 이 집중 재검증을 전체
배터리를 두 번 실행한 것으로 세지 않는다. 집중 로그는
`/private/tmp/floe-review-reconnect-{ui-final,build-final,clippy-final,cli-final,http-final}.log`다.
메뉴 inventory37 linked/2 OPEN, `--require-complete`의 의도된 exit1, scoped fmt·
diff 검사도 확인했다. 실제 브라우저 다운로드는 §80의 도구
정책 차단 이후 재시도/우회하지 않았으며 여전히 **미검증**이다. main과 jobdeck의
작업을 이 변경에 포함하지 않는다.

이 단계의 목표 잔여: G4 메뉴 구현은 **SVRF metadata 교체·카메라 유지 jobdeck
레벨 재선택2건**이다. 진단/무효 CLI 경계 재대조, 실제 브라우저·Python-free Linux·
G1/G4·현장 수용, M2 공유/원격 승인·구현 및 조건부 M5가 별도로 남는다. 메뉴
inventory의 linked37/open2는 기능 전체 수용률이 아니다. 전체 goal은 진행 중이다.

## 82. M4g-25 — 실행 중 SVRF metadata 원자 교체

2026-09-16. `Load SVRF metadata…`는 현재 DRC에 `floe-svrf-rules` JSON을 불러오거나
교체한다. 승인 폴더의 opaque 파일 handle과 현재 view/DRC/revision만 전송하며
원본 SVRF deck/include 해석·임의 path 입력·파일 쓰기·reviewer 권한 부여는 없다.
실패한 파일 선택이 유효한 metadata를 지우지 않으며 매칭0건은 summary에 명시한다.

- 같은 DRC reader/Database를 재사용한다. picker actor가 bounded JSON을 읽고
  기존 DRC actor가 check 이름에 매칭한 불변 snapshot을 준비한다. 큰 ASCII DRC를
  다시 파싱하지 않는다. 준비 중 기존 조회는 유지되며 query fence가 결과를 검증한다.
- picker 취소/receipt lock→registry→view→review services→reader state→revision 순으로
  짧은 in-memory commit을 한다. 보호 입력 등록, metadata snapshot, 새 query revision이
  함께 공개된다. 실패하면 이전 revision도 유지한다. 새 revision에서는 이전 query/
  type/filter/selection/CD/prepared focus와 미승인 review/transfer preview가 무효다.
  진행 중 review preparation/publication/transfer와는 성공 commit하지 않는다.
- layout 카메라·레이어·렌더러, DRC reader ID·waive 상태, reviewer binding/권한은
  유지한다. 완료된 저장/전송 receipt와 high-water도 보존한다. 같은 reviewer의 기존
  자동 저장 opt-in을 새로 부여하지 않으며, 새 metadata가 이전 preview를 승인하지 않는다.
  다른 DRC 열기는 기존대로 metadata를 버리고 reviewer를 detach한다.
- startup의 reader512MiB(기존256+metadata256)를 reader256와 snapshot256 예약으로
  분리했다. 교체 후보·이전 snapshot·진행 중 조회 참조는 각자의 예약과 read lease를
  실제 사용 종료까지 가진다. 추가 CPU/worker를 만들지 않는다. JSON16MiB/구조 제한과
  별개인 공유 admission이며 RSS hard limit은 아니다. actor 대기300초 timeout은
  실제 NFS/worker 종료 확인을 뜻하지 않는다.
- metadata 입력은 별도 scope로 보관하며 DRC/layout root를 넓히지 않는다. 이후
  명시 pack build/reviewer 재연결에도 마지막 metadata 선택을 유지하고 기존
  defaults/index/review 게시의 보호 입력에 포함한다. index hot reload는 추가하지 않는다.
- 같은 picker ledger를 쓰므로 잃어버린 ACK는 GET부터 복구하고 동일 요청만 replay한다.
  UI는 성공 receipt의 옛 catalog를 설치하지 않고 현재 catalog를 다시 조회한다.

집중 검증: core280/web95 단위(외부 fixture 테스트 각각7/3개 ignored), 전체 ES2017/UI, release build,
실제 `validate_web_drc_rules.py`가 통과했다. 새 HTTP gate는 ASCII/ICE 양쪽의 같은 reader
교체, type/constraint 변경, malformed/version/16MiB 초과/교체된 inode/stale context의
실패 보존, 취소 race와 동일 replay, 권한 필드 거부, 이전 preview 거부/저장 receipt 유지,
build/reconnect metadata 유지, camera/input fingerprint와 종료 수거를 확인한다.
DOM gate는 metadata-only revision에서 늦은 type 응답 폐기, 매칭0건 표시,
journal-before-send·GET 복구·동일 재전송을 확인한다. 실제 브라우저 수용은 아니다.
로그: `/private/tmp/floe-live-svrf-{unit,ui,build,http}.log`.
strict all-target clippy도 exit0으로 통과했다(`/private/tmp/floe-live-svrf-clippy.log`).
전체 `sh tools/validate_rust.sh`도 exit0, `RUST VALIDATION: ALL OK`로 완료했다
(`/private/tmp/floe-live-svrf-battery.log`). 새 runtime SVRF HTTP gate와 기존 DRC
교체/reviewer 재연결 gate를 포함하고, jobdeck83·renderer46·KLayout13 PX+
2 phase-exact+14 style(j1/j8)을 통과했다. scoped fmt·diff 검사와 메뉴 inventory도
통과했다. `--require-complete`는 남은 메뉴1건 때문에 의도대로 exit1이다.
집중 검증 뒤 runtime 코드를 추가 변경하지 않았다. 검증용 `.venv` 링크만 제거했으며
대상 가상환경과 main/jobdeck의 별도 변경·새 문서를 보존했다.

커밋 시 목표 잔여: GTK 메뉴 구현 목록은 카메라 유지 jobdeck 레벨 재선택1건이다.
inventory linked38/open1은 전체 수용률이 아니다. 진단/무효 CLI 최종 재대조,
실제 브라우저·Python-free Linux·G1/G4·현장, M2 공유/원격 승인·구현, 조건부 M5는
별도로 남는다. 브라우저 URL 정책 차단을 우회하지 않았으며 다운로드는 미검증이다.
main/jobdeck 작업은 이 단계에 포함하지 않는다. 전체 goal은 진행 중이다.

## 83. M4g-26a — 열린 덱을 보존하는 선택 소스 인덱싱

2026-09-16. 카메라 유지 레벨 재선택을 준비하며 선행 충돌을 확인했다.
`ManagedIndex::start`가 선택 여부/재사용 여부와 무관하게 등록된 모든 TC 캐시에
쓰기 잠금을 잡았다. 열린 레벨A를 재사용하고 새 레벨B만 만들려는 작업도 A의
read lease와 충돌했다. 기존 뷰를 닫아 해제하는 것은 실패 시 기존 화면 보존
계약과 맞지 않으므로 이 잠금 분류를 별도 단계로 수정한다.

- 덱은 CPU·단일 index 예약과 전체 TC read lease를 먼저 얻고, 등록/선택/캐시를
  검증해 `DeckIndexPlan::todo`를 만든다. read lease를 놓지 않고 해당 목적지만
  하나의 자원 mutex 안에서 원자적으로 write로 전환한다. 모든 목적지의 충돌을
  확인한 뒤 전환하므로 마지막 목적지의 충돌도 첫 native 쓰기 전에 실패한다.
- 재사용·미선택 TC는 read로 남아 기존 renderer가 계속 사용할 수 있다. `force`,
  새 occupancy 생성, `occupancy_only`가 실제로 수정할 TC는 write가 필요하며,
  열린 캐시라면 `busy`다. 옵션을 무시하거나 현재 뷰를 닫아 우회하지 않는다.
- canonical 부모/캐시 alias를 동일 잠금으로 처리한다. 계획 외 목적지 승격은
  거부한다. 기존1개 index·jobs1..16·foreground reserve를 유지하며, 혼합 잠금과
  CPU 예약은 취소/실패와 native child reap을 포함한 전체 supervisor 수명을 따른다.
- 일반 레이아웃 managed index 및 CLI `PreparedIndex` 정책은 그대로다. 실제
  native 소스별 쓰기 직전의 재검증/OS lock도 그대로다. 읽기 분류는 gateway 내부
  불변만 보장하며 외부 indexer/캐시 교체에 대한 reader 보호나 hot reload는 아니다.
- API 권한·요청 필드·선택/옵션 의미·순번/replay는 변경하지 않는다. 성공한 index도
  열린 뷰의 로드 레벨/카메라/가시성이나 renderer를 자동 교체하지 않는다.

집중 검증: core282/web95 단위(외부 fixture7/3개 ignored), 실제 managed index,
실제 owner HTTP18개가 통과했다. 자원 단위는 atomic rollback, alias 충돌, 승격 후
reader 차단, 실패/해제 회계 및 foreground reserve를 검사한다. native gate는 열린
캐시 byte 보존, 선택된 새 소스 생성·미선택 미생성, force/occupancy 충돌 시 첫
쓰기 전 거부, 혼합 잠금 취소/정지 child kill·reap를 검사한다. HTTP는 살아 있는
뷰의 camera/revision/renderer·로드 레벨 보존, 새 cache 생성과 replay를 확인한다.
합성 덱 fixture에 빠진 필수 AD를 보완한 후 재실행했으며 이는 제품 결함 수정이 아니다.
로그는 `/private/tmp/floe-deck-index-leases-{unit,managed,http,clippy,build}.log`다.
strict all-target clippy와 release build도 exit0으로 통과했다. 전체
`sh tools/validate_rust.sh`는 exit0, `RUST VALIDATION: ALL OK`로 완료했다
(`/private/tmp/floe-deck-index-leases-battery.log`). 새 실제 managed/HTTP gate,
jobdeck83·renderer46·KLayout13 PX+2 phase-exact+14 style(j1/j8)을 포함한다.
집중 검증 뒤 runtime 코드를 추가 변경하지 않았다. scoped fmt·diff 검사와 메뉴
inventory도 통과했다. `--require-complete`는 남은 메뉴1건 때문에 의도대로 exit1이다.
검증용 `.venv` 링크만 제거했으며 대상 가상환경·main/jobdeck의 별도 작업은 보존했다.

커밋 시 목표 잔여: 카메라 유지 레벨 재선택1건은 **여전히 OPEN**이며 다음 단계에서
서버 camera/revision 고정, 실패·취소 보존, 색인 승인 재시도와 브라우저 조작을 연결한다.
이 선행 변경을 메뉴 완료로 세지 않는다. 진단/무효 CLI 최종 재대조, 실제 브라우저·
Python-free Linux·G1/G4·현장 수용, M2 공유/원격 승인·구현 및 조건부 M5도 남는다.
URL 정책 차단 이후 브라우저 검증을 우회하지 않았으며 다운로드는 미검증이다.
main/jobdeck의 별도 변경은 보존한다. 전체 goal은 진행 중이다.

## 84. M4g-26b — 카메라 유지 jobdeck 레벨 재선택

2026-09-16. `Levels to load`의 `Apply levels · keep view`는 현재 열려 있는 덱을
고른 경우에만 보인다. GTK `_jobdeck_reselect_levels`의 중심·배율 복원을 서버의
첫 generation 상태로 준비하며, fit 후 별도 goto를 보내는 방식은 아니다.

- owner `reselect_levels`는 seq/view_id/base_state_rev/필수 levels만 받는다. source,
  현재 mode, camera, pixels, DBU는 서버의 단일 controller snapshot에서 유도한다.
  브라우저의 bbox/source/mode/body/쓰기 옵션은 거부한다. queued 작업 시작·준비 후
  교체 commit에서 원래 view/revision을 검사하고 DBU가 달라지면 준비를 거부한다.
- 다른 레벨 집합은 기존 window display 정책과 새 mode별 layer defaults로 준비한다.
  GTK generic deck reopen처럼 depth는 full, mono는 초기값으로 시작하며, detail/thin/
  frames/font 등 창 정책은 유지한다. 옛 synthetic layer ID·visibility·isolation·style과
  mode visibility memory는 새 선택으로 복사하지 않는다. 같은 선택은 empty patch의
  CAS no-op이라 viewport의 부동소수점 resize 재계산도 하지 않는다.
- 가용하지만 미색인인 선택 TC가 있으면 준비를 중단하고 이전 뷰를 유지하며 별도
  index 승인을 제안한다. 일반 Open의 부분 덱 표시나 실제 없는 TC의 skip/incomplete
  정책은 바꾸지 않는다. 인덱싱은 M4g-26a의 혼합 잠금으로 열린 재사용 캐시를 유지한다.
  force/occupancy가 열린 캐시를 수정하려는 경우는 여전히 Busy이며 무시/자동 close하지 않는다.
- index preview의 `reselect.target/pixels`를 승인 journal에 넣고, 서버도 다른
  target/pixels로 갱신하는 승인을 거부한다. 원래 요청 뒤 pan/resize/open이 생기면
  오래된 카메라로 덮지 않는다. 색인이 성공한 뒤 교체가 stale이면 완료된 cache는
  보존하고 `index:succeeded`와 `open:failed`를 분리해 표시한다. 새 명시 요청으로
  재시도할 수 있지만 새로고침 자체가 새 권한/seq/소스를 만들지는 않는다.
- 준비 실패·commit 이전 취소는 기존 뷰를 유지하며 하나의 render 예약을 재사용한다.
  commit 뒤 늦은 취소는 성공을 cancelled로 바꾸지 않는다. 성공 receipt는 attachment
  교체 완료이고 첫 프레임 성공 보장은 아니다. 이후 native worker/렌더 오류는 기존
  view failure 계약을 따르며 이 단계가 worker 실패 후 자동 rollback을 추가하지 않는다.
- UI는 pending 입력·제스처·launch/index 동의·owner 작업 중 재선택을 막는다. 승인
  modal 중 DOM resize도 승인 전에 거부한다. 잃은 응답은 GET부터 복구하고 같은 요청만
  재전송하며, 완료 receipt의 퇴역한 view ID 때문에 replay를 재실행하지 않는다.

집중 검증: web96 단위(외부 fixture3개 ignored), 전체 ES2017/DOM UI, 실제 owner HTTP20개,
strict all-target clippy와 release build가 통과했다. native gate는 첫 프레임 gen1의
bbox/pixels, chip/layer 모드·새 defaults, 동일 선택 no-op, stale/잘못된 레벨/metadata
실패, 일반 교체 취소 race, 퇴역 view replay와 단일 worker를 검사한다. 별도의 정지
native writer로 색인 전 pan 거부, 색인 중 resize→완료 cache 보존/교체 거부,
취소→kill/reap→같은 anchor의 명시 재시도 성공을 확인한다. UI는 현재 소스 제한,
빈 선택/중복/대기/상태 조회 실패, ACK 유실의 읽기 복구, 선택 표시 복원과 고정
anchor journal/변조 거부/resize 거부를 검사한다. 최초 native 테스트의 stale HTTP
기댓값409는 기존 Busy 계약429/`busy`로 정정했으며 제품 오류로 계산하지 않는다.
로그: `/private/tmp/floe-levels-{unit,ui,http-final,clippy-final,build}.log`.
전체 `sh tools/validate_rust.sh`도 exit0 / `RUST VALIDATION: ALL OK`로 완료했다.
core282/web96 단위, owner HTTP20, 전체 UI·메뉴 linked39/open0, jobdeck83·renderer46,
KLayout j1/j8 각각13 PX+2 phase-exact+14 style이 통과했다. 외부 fixture를 요구하는
기존 ignored 단위는 별도로 유지하며 실제 native 통합 게이트와 혼동하지 않는다.
전체 로그는 `/private/tmp/floe-levels-battery.log`다. 검증용 `.venv` symlink만 제거하고
연결 대상 환경은 보존했다. 실제 브라우저 수용/스크린샷은 도구 URL 정책 차단을
우회하지 않아 미검증이며 합성 HTTP/DOM으로 대신 합격시키지 않는다.

커밋 시 목표 잔여: 감사된 GTK 메뉴의 알려진 연결 누락은 **0건**(linked39, excluded2,
inactive1)이며 `--require-complete`를 전체 배터리에 넣었다. 이것은 전체 기능/수용
완료율이 아니다. 다음은 진단/무효 CLI 경계와 전체 G4 최종 재대조다. 실제 브라우저·
Python-free Linux·G1/G4·현장, M2 공유/원격 승인·구현, 조건부 M5도 별도로 남는다.
main/jobdeck의 별도 변경·현장 검증 대기는 유지하며 전체 goal은 진행 중이다.
