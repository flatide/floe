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
| `superseded` | 같은 종류의 새 질의가 기존 질의를 중단함 |
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
  다음 controller 연결은 이를 무시하고 style 호출 실패로 view를 죽여서는 안 된다.
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

다음은 ViewController의 표시 frame/worker epoch·revision 고정, latest-only query
mailbox·취소·style drain과 늦은 응답 거부다. foreground뿐 아니라 **표시된 margin
scene**도 대상으로 해야 한다. 그 이후에만 owner API·pick/snap UI·ruler를 연결한다.
공유/원격 공개와 현장 수용은 별도 대기 상태로 유지한다.
