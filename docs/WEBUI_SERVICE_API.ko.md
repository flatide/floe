# 웹 전환 — Rust 서비스/API 설계 초안

작성 2026-09-12, 기준 `feature/webui@6c33a48`.
[상위 계획](WEBUI_PLAN.ko.md) · [M0 기능 대조표](WEBUI_M0.ko.md).

**아래 endpoint/message는 전체 서비스 설계안이다.** M1b-1의 일부 transport API
(exchange/capabilities/logout/WS ping)는 [M1b 기록](WEBUI_M1B.ko.md)에 명세/구현했다.
아래 전체 URI가 그대로 구현된 것은 아니며 공유 API는 아직 없다.
M4g-2의 실제 `GET /api/v1/views/{id}/minimap/{base}`는 owner 인증+CSRF로 현재 view의
메모리 palette 베이스(180×180 ASCII 인덱스)만 반환한다. `base`는 `full` 또는 실제
저장된 depth의 정규 십진수다. 임의 파일/geometry 조회나 새 렌더를 하지 않는다.
snapshot의 `minimap`은 base 키·die·유계 화면 사각형만 포함하고,
`view.set`의 `navigation:{kind:"minimap",point:[x,y]}`는180px 미니맵 좌표로
동일 배율/16px 위상 이동을 요청한다. world 계산·범위 검증은 Rust에 있다
([M4 §41](WEBUI_M4.ko.md)). 아래 공유/카탈로그 API 초안을 구현한 것으로 보지 않는다.

M4g-10의 snapshot `camera_um`은 Rust가 현재 viewport에서 계산한
`[center_x,center_y,width]` 세 십진 문자열이다. HTTP view 복원과 WS snapshot에
동일하게 포함하고, framebuffer/margin bbox나 renderer ABI는 바꾸지 않는다.
문자열은 추가 UI 반올림 없이 입력에 사용하며 64자 초과 평문은 scientific notation으로
표시한다. 극단적인 단위로 µm의 유한값/양의 폭을 표현하지 못하면 null이고 입력칸은
빈 값으로 둔다(기존 geometry 상태는 유지). 이 표시는 새 navigation 요청이 아니다.
별도 해상도·좌표 변환을 브라우저가 다시 결정하지 않는다([M4 §55](WEBUI_M4.ko.md)).

M4g-11a는 live `view.set`의 `layer_batch`를 추가한다:
`{action:"show"|"hide"|"toggle",pairs:[[layer,datatype],...],collapsed:[[layer,datatype],...]}`.
`collapsed`는 생략하면 빈 목록이며 **선택한 일반 그룹 부모 중 접힌 것**만 전달한다.
잡덱 부모는 항상 자식 전체를 대상으로 하므로 펼침 여부를 보내지 않아도 된다.
Rust가 현재 model의 정렬/그룹과 가시성을 사용하며 브라우저가 자식을 전개하지 않는다.
기존 view/epoch/base_state_rev 검사 한 번으로 전체를 원자 적용한다. 부모와 자식이
같이 선택되면 포함된 자식을 두 번 toggle하지 않는다. 입력·펼친 대상은 각 4096개
이하이고 기존 explicit 가시 레이어 4096개 한계/transport body 상한도 유지한다.
무효/미지정 부모·빈 선택·초과는 전체 오류이지 부분 적용이 아니다. `layers`,
`layer_change`, isolation/restore, properties/settings와 혼합하지 않는다. 실제 변경이
없으면 revision/렌더도 늘지 않는다. 파일 쓰기·렌더러 wire 변경은 없다.
Rust 기반은 M4 §56, 브라우저 다중 선택/접힘·일괄 가시성 연결은 M4 §59에 기록한다.

M4g-11c는 인증된 `POST /api/v1/views/{id}/palette` **읽기 전용** 조회를 추가한다.
요청은 다음 둘 중 하나다:

```json
{"kind":"page","start":0,"fold":{"closed":false,"exceptions":[]}}
{"kind":"range","first":[3,1],"last":[7,9],"fold":{"closed":true,"exceptions":[[3,1]]}}
```

`fold` 생략은 모두 펼침이다. `closed`는 전체 그룹의 기본 접힘 상태이고 `exceptions`는
그 반대로 표시할 실제 그룹 부모의 중복 없는 목록이다(최대4096, 기존16KiB HTTP body
상한도 적용). 접기는 화면 레이어 가시성과 별개이며 ViewState/파일에 저장하지 않는다.
일반 레이아웃과 잡덱 source-layer 모드는 같은 layer의 최저 datatype을 부모로 삼는다.
잡덱 level 모드의 숨긴 chip 행은 조회로 노출하거나 펼칠 수 없다.

- `page`: 접힌 자식을 먼저 건너뛴 뒤 표시 순서의 `start`부터 최대64행. 응답의
  `total`은 접힘 반영 행 수, `all_total`은 접기 전 패널 행 수(숨긴 chip 제외)다.
  기존 row 필드에 `children`·`closed`를 추가하고 `head`·`parent`로 일반 그룹도 표현한다.
  `next`는 다음 표시 순서 offset 또는 null. `start == total`은 빈 끝 페이지이며
  그보다 큰 offset은 `invalid_palette` 오류다.
- `range`: 표시 순서에서 두 pair를 포함하는 구간을 방향과 무관하게 오름차순으로
  반환한다. `pairs` 최대4096, `groups`는 그중 펼칠 수 있는 부모들이다. 접힌 자식은
  제외하고 페이지 경계는 제한하지 않는다. 없는 pair는 `invalid_palette`, 접힘에
  가려진 anchor는 `palette_anchor_hidden`, 4096개 초과는 `palette_range_too_large`다.
  조용한 prefix 반환/부분 선택이나 자동 펼침은 하지 않는다.

두 응답 모두 `state_rev`·`render_key` 문자열을 포함한다. 브라우저는 view/접기 상태와
키가 바뀐 지연 응답을 버려야 한다. 소스 경로·geometry를 읽거나 native query/render를
제출하지 않는다. 기존 host/origin/cookie/CSRF/body 방어를 그대로 적용하고, `view.set`
또는 승인 operation으로 취급하지 않는다. 이전 `GET .../layers/{start}`는 펼친 목록과
기존 schema를 유지한다([M4 §58](WEBUI_M4.ko.md)). M4g-11d 웹 UI는 새 API를 사용하며
같은 페이지의 Shift 범위는 이미 받은 서버 순서로 처리하고, 페이지 간 범위만 추가로
읽는다. 선택/접기는 렌더를 제출하지 않고 선택 행 가시성은 `layer_batch` 한 번으로
보낸다. 선택4096개 한계 외에 기존 WebSocket8KiB/HTTP16KiB 상한도 적용되므로 큰
숫자 pair가 많은 요청은 더 일찍 전체 오류가 될 수 있다. 자동 분할/부분 적용은 없다.

M4g-11e는 live `view.set.body.style_batch`를 추가한다:

```json
{"pairs":[[3,1],[3,300]],"collapsed":[[3,1]],"color":"#22aa88","fill":{"kind":"pattern","rows":[4660,4660,4660,4660,4660,4660,4660,4660,4660,4660,4660,4660,4660,4660,4660,4660]},"width_step":1}
```

`color`/`fill`/`width`/`width_step` 중 적어도 하나가 필요하며, 생략 필드는 유지한다.
width는1..8, width_step은-1/+1이며 둘의 동시 지정은 오류다. null/추가·중복 필드는
거부한다. `collapsed`는 선택한 실제 그룹 부모만 허용한다. level 뷰에서 숨긴 chip
그룹은 Model이 영구 접힘으로 처리하므로 브라우저가 숨긴 자식을 열거할 필요가 없다.
선택/확장 및 부모가 영향을 줄 수 있는 자식까지4096개 상한이고 기존 wire 크기 상한도
유지한다. 초과는 전체 오류이며 분할 쓰기·재전송은 하지 않는다.

- 일반 접힌 부모는 자식까지 같은 값을 직접 지정하고, 펼친 부모는 자기 행만 지정한다.
- 잡덱 부모 **색**은 펼쳐져 있어도 자식에 전파한다. **채움·선폭**은 전체 sparse
  assignment를 해석하며 기존 자식 지정값이 우선한다. 접힌 부모는 자식도 직접 지정한다.
- 선폭1은 override 제거/상속이다. 증감은 행의 sparse 지정값(없으면1)에 적용하고
  1..8로 제한한다. 화면에 보이는 상속 선폭에 더하는 명령이 아니다.
- 기존 `styles`/`style_deltas`·settings/properties와 섞으면 오류다. 가시성/카메라를
  변경하지 않으며 공유 파일에 쓰지 않는다. 기존 CAS·no-op·렌더 합치기 계약을 따른다.

구형 행 단위 `style_deltas`의 전체 그룹 덮어쓰기 의미는 유지한다. 새 팔레트 선택
명령의 sparse 상속 의미와 구분한다. M4g-11f부터 웹 단일 행 편집기도 `style_batch`를
사용하여 현재 접힘과 sparse 상속 규칙을 따른다. 기존 외부 API 의미는 바꾸지 않는다.

M4g-11f는 인증된 읽기 전용 `GET /api/v1/palette/presets`를 추가한다. 본문과 view ID가
없고 열린 설계/worker 없이도 동작한다. cookie/CSRF·host/origin 검사를 적용하며 응답은
`version:1`, 순서 있는 `colors:[{name,color}]`, `fills:[{name,rows,fill}]`이다.
`rows`는 MSB-left16×u16 미리보기, `fill`은 기존 solid/clear/speckle/pattern DTO다.
색 이름 별칭은 RGB가 같아도 합치지 않는다. 현재49색/20채움과16KiB 이내 응답을 gate로
고정한다. 런타임에서 표를 읽거나 수정하는 endpoint가 아니며 compile-time `.def`만
노출한다. 표와 전송 코드도 bundle 식별에 포함된다. 팔레트 읽기는 state/render revision을
변경하지 않고 클릭 시점의 선택을 단일 `style_batch`로 별도 제출한다.

M4g-17a의 `GET /api/v1/display-test/{png|raw}`는 owner 인증 뒤 고정 합성 색 막대만
반환한다. PNG는 `image/png`(<16KiB), raw는 `application/octet-stream`의230,416바이트
`FLOERAW1`이다(360×160 RGBA). 임의 파일/view/worker 접근이나 frame/query ID는 없다.
About 또는 M4g-17b 독립 `displaytest` 페이지의 명시 실행만 이 API를 부르며 UI는
live viewer의 payload 검사/디코더를 공유한다. 독립 Gateway의 capabilities에는
`display_only:true`가 있으며 render/catalog/index_open/file_picker/launcher는 false다.
일반 Gateway는 `display_only:false`다. 경로/파일/worker 등록이나 새 쓰기 API 없이
기존 exchange와 `DELETE /api/v1/session`을 사용한다. 독립 세션의 logout/expiry는
owner worker 없이도 리스너를 종료한다. 루트 HTML만 역할에 맞게 선택하며 파일 URL이나
요청 파라미터를 디스크 파일에 매핑하지 않는다.
Canvas readback 결과와 원격 화면 수용은 구분한다([표시 진단 계약](WEBUI_DISPLAY_DIAGNOSTICS.ko.md)).

M4g-17c의 선택 `GET /api/v1/display-test/input`은 **CLI가 미리 읽고 검증해 고정한
정적 PNG 하나**만 반환한다. 웹 경로/업로드/재읽기/파일명 endpoint가 아니다.
capabilities의 `display_input`은null 또는 `{width,height,bytes}`이며 일반 view에는null이다.
원본 PNG 바이트(metadata 포함)를 불변 `Bytes`로 owner에게만 제공하며 input 부재는404,
인증 누락은401, 쿼리/다른 origin은403이다.80MiB/8192px축/16Mpx/65536chunks 제한과
CRC·trailing data/APNG 거부를 CLI에서 적용하고 실제 픽셀 디코딩은 브라우저에 맡긴다.
UI는 명시 Show·5초 read/decode·공통 PNG decoder·360×160 smoothing/alpha 표시를
사용한다. 결과는 보간/원격 화면의 정확성 인증이 아니다. 기존 합성 API/일반 뷰는 불변이다.

M4g-16c는 세션 슬롯 표를 별도로 읽는 owner API
`GET /api/v1/views/{id}/fill-slots/{key}`를 추가한다. snapshot의 `fill_slots_key`
(40 lowercase hex)가 현재 표와 같아야 하며 다르면409, 열린 view가 아니면404다.
응답은 `{version:1,view_id,fill_slots_key,editable,fills:[{name,rows:[u16;16]},…]}`이며
전체20슬롯/16KiB 미만이다. 기본 프리셋 GET은 계속 불변이다. key는 override만 해시한
비권한 캐시 힌트라 pan/색/할당마다 바뀌지 않는다. 별도 파일 I/O나 render를 하지 않는다.

`style_batch.fill_slot:"brick"`는 현재 슬롯 참조를 할당한다(`fill`과 동시 지정 불가).
슬롯의 내용 편집은 일반 Patch가 아니라 전용 WS 명령 `view.fill_slot`이다:

```json
{"type":"view.fill_slot","seq":"7","connection_epoch":"<current>","view_id":"<current>",
 "base_state_rev":"12","body":{"name":"brick","rows":[1,2,4,8,16,32,64,128,256,512,1024,2048,4096,8192,16384,32768]}}
```

capability `fill_slot_edit`는 기본 false다. trusted launcher의 nonempty `FLOE_FILL_EDIT`
opt-in 없이는 `fill_edit_disabled`, 고정 solid/clear/무효 슬롯은 `invalid_request`,
오래된 state는 `stale_state`다. 연결/view/seq 검사는 `view.set`과 같다. 결과는 기존
`accepted` 또는 `error` 뒤 authoritative snapshot이며 renderer는 resolved bitmap만 받는다.
새 edit 필드를 `view.set`/startup/open Patch에 넣지 않으므로 opt-in 우회 경로가 없다.
기존 명시 Native JSON import는 계속 허용한다. 슬롯 Apply는 파일 게시 승인이 아니며
공유 기본값 게시의 별도 preview/승인을 자동 수행하지 않는다.

M4g-16d 웹 프리셋은 `fill_slot` 참조를 할당하며16×16 편집기는 이 전용 명령을 쓴다.
현재 표는 view/연결 epoch/cache hint로 캐시하고, pan revision만으로는 재조회하지 않는다.
초안은 시작 시 state revision에 묶는다. 전송 대기 중 최신 revision으로 재작성하지 않고,
stale/disconnect 요청을 자동 재전송하지 않는다. 개발 도구는 capability와 현재 표의
`editable`을 모두 확인하지만 서버 권한/CAS 검사를 대신하지 않는다. Apply는 명시적
세션 편집이고 파일 저장이 아니며, 실제 브라우저 수용은 별도다([슬롯 계약 §6](WEBUI_BITMAP_SLOTS.ko.md)).

M4g-11b는 `index`/`index_open`의 `options.occupancy` 생략 기본값을 true로
맞춘다. false는 명시 해제이며 기존 요약을 지우지 않는다. `occupancy_only:true`는
일반 생성 기본값보다 우선하고, `occupancy_um` 지정도 요약 생성을 요청한다.
UI는 신규 승인에서 기본 체크하지만 저장된 승인/재시도의 false는 바꾸지 않는다.
파일 선택/읽기 전용 preview는 여전히 색인을 실행하지 않고 승인 후에만 쓴다.

M4g-3은 live `view.set` body에 `depth_step:-1|1`을 추가한다. 절대 `depth`와
동시 지정은 거부하고, controller가 CAS 락 안의 현재 depth와 native max_depth로
해석한다. 초기 open body에서는 상대 depth를 거부한다. DRC 단축키는 기존 유계
순회/owner editor API만 사용하며 새 게시 endpoint나 autosave를 추가하지 않는다.
M4g-5b는 기존 owner `POST /api/v1/operations`에
`{kind:"mode",seq,view_id,base_state_rev,mode:"level"|"chip"|"layer"}`를 추가한다.
seq/revision은 정규 십진 문자열이고 source/levels/path/body 필드는 받지 않는다.
전역 `jobdeck_modes`와 snapshot `capabilities.mode`를 함께 확인한다. 현재 덱의
선택 레벨·카메라를 보존하고 새 모드 기본 스타일을 읽어 순차 worker 교체를 CAS로
commit한다. 성공 receipt는 새 view_id와 unchanged:false(동일 모드는 true)를 반환한다.
이는 첫 프레임 완료가 아닌 attachment 교체 완료이며 이후 worker 실패는 view에 표시된다.
기존 작업 조회/cancel/replay 계약을 사용한다. old view_id의 원래 seq 재전송도 원래
receipt만 반환하며 다시 전환하지 않는다. 파일 게시/재색인 권한은 아니다([M4 §45](WEBUI_M4.ko.md)).
M4g-6의 인증된 `GET /api/v1/startup`은 `{request,confirm_levels}`를 반환한다.
`request`는 기존의 등록 source ID 기반 open 제안이고 `confirm_levels:true`는 UI에서
레벨 선택 후 명시적으로 open하도록 하는 안내다. 서버의 source 범위나 색인/게시 권한을
확장하지 않는다. goto의 문자열 `width_um`은 생략할 수 있으며 현재 폭(초기 fit 폭)을
유지한다. JSON null·비유한/0/음수 폭은 거부한다. CLI 초기 정책은 launcher에서
하나의 open body로 확정하며 저수준 owner API의 빈 body 기본값을 변경하지 않는다
([M4 §46](WEBUI_M4.ko.md)).
M4g-7a의 `app-core::instance`는 trusted local launcher끼리의 Unix socket 코어다.
HTTP/WS endpoint 또는 브라우저 임의 경로 등록 API를 추가하지 않는다. 동일 UID와
DISPLAY 구분은 공유 서비스의 실사용자 인증이 아니다. 통신 `Handled`는 callback의
처리/작업 접수 결과이며 view 부착·첫 프레임 완료를 뜻하지 않는다. 제품 CLI와의 연결,
동적 catalog 및 진행 중 게시 작업의 보호 경계는 후속이다([M4 §47](WEBUI_M4.ko.md)).
M4g-7b는 trusted Rust API `Service::register_source(scope,path,stop)`를 추가한다.
기존 인증 `GET /api/v1/catalog`에 새 opaque source ID가 보이지만 브라우저 path 등록
endpoint는 없다(`POST /api/v1/catalog`는405). service는0..32개 등록을 허용하며
기존 CLI는 여전히1..32개 소스를 요구한다. 같은 미변경 source는 같은 ID이고 등록만으로
open/index/게시를 하지 않는다. SourceSet이 기본값·DRC writer와 공유되어 새 등록 후
예전 draft도 현재 보호 목록으로 검사한다. 등록/게시와 새 open/index의 Busy·수명 계약은
[M4 §48](WEBUI_M4.ko.md)를 따른다. IPC handler/제품 CLI 연결이나 빈 창 UI의 완료는 아니다.
M4g-9a는 기존 owner `POST /api/v1/operations`에 다음 명시 승인 작업을 추가한다.
`{kind:"index_open",seq,request_id,open_seq,approved:true,target,pixels,options}`.
`request_id`는 caller가 생성한64자리 소문자 hex이며 모든 진행/완료 응답에 반영된다.
`open_seq`는 해당 owner의 아직 남아 있는 실패 open이고, `index_open` 제안이 있는
캐시 실패여야 한다. `target`은 `{kind:"empty"}` 또는
`{kind:"replace",view_id,state_rev}`다. 현재 뷰/revision을 색인 시작 전·이후·교체
commit에서 확인한다. source/모드/선택 레벨/표시 patch는 서버가 보관한 원래 요청이며
새 경로·다른 소스/선택·교체 표시 옵션을 받지 않는다. `options`는 기존 index 옵션이다.
`approved`와 `force`는 별개이며 생략/null 승인은 거부한다.
진행은 `stage:index|open`과 `index` snapshot으로 구별한다. 색인 성공 뒤 열기 실패/
취소는 `index.phase:succeeded`를 보존하며 디스크 결과를 취소한 것으로 표시하지 않는다.
top-level succeeded는 기존 open과 같은 attachment 교체 완료이며 첫 프레임의 완료는
아니다. 기존 작업 조회/cancel/replay/32개 이력 만료 계약을 따른다.
M4g-9a는 서버 기반([M4 §53](WEBUI_M4.ko.md))이며 M4g-9b에서 브라우저 승인/복구를
연결했다([M4 §54](WEBUI_M4.ko.md)). `capabilities.index_open`은 owner service가
있을 때만 true다. 인증+CSRF를 요구하는
`GET /api/v1/operations/{seq}/index-open`은 메모리에 보존한 원래 source/title/mode/
display_policy/open_seq, 전체 선택 레벨(`all` 또는 `only`+정규 i64 문자열 ids),
현재 `jobs_available`(0..16)을 반환한다. cache probe·색인·자원 예약을 하지 않으며
실제 admission은 승인 후 다시 검사한다. 만료 이력은410, 제안 없는 seq는400,
잘못된 CSRF는 기존 인증 규약대로401이다. 짧은 history 제안은 계속 레벨 개수만
보내고 전체 ids는 단일 preview에만 담는다(최대4096). 모든 queued/progress/terminal
`index_open` 응답은 request_id와 **open_seq**를 echo해 새로고침 후 원래 실패와
묶는다. 브라우저는 승인 내용을 sessionStorage에 먼저 저장하고 결과를 seq+kind+
request_id+open_seq로 검사한다. 조회/새로고침은 읽기 전용이며 불명확한 접수의
재시도도 동일 payload만 사용한다. 같은 seq의 다른 요청을 채택/취소하지 않는다.
M2a의 현재 DRC 등록/읽기 URI·페이지·취소·focus/in_view·패널 상태·필터·순회·마커·
CD·선택 집합·SVRF metadata/비교·타입 패널·격리/원자적 focus·ASCII API 계약은 [M2 기록](WEBUI_M2.ko.md) §2~21이
기준이다. geometry reader는 read-only이며, 명시 opt-in한 owner의 주석 저장 endpoint는
M4e-3a에 추가했다. waive 승인 쓰기는 별도 opt-in의 M4e-4c이며 공유 endpoint는 아직 없다.
M4e-1의 `app-core::drc::review`는 waive/FE codec과 메모리 note 그룹만 제공한다.
레거시 fingerprint는 인증/유일 run identity가 아니며, 향후 writer의 pack identity·
review revision 검증을 대체하지 않는다([M4 §21](WEBUI_M4.ko.md)).
M4e-2a의 `review::store`는 로컬 expected snapshot·pack binding·명시 게시 코어다
([M4 §22](WEBUI_M4.ko.md)). 이 코어 자체에 HTTP/WS endpoint는 없으며, owner 설정/lease/
review_rev·승인 receipt는 아래 M4e-3a/4c에서 연결한다. legacy 파일의 수정은 별도 확인이 필요하다.
M4e-2b의 `review::managed`가 준비/게시의 admission·read lease 수명과 취소/join·typed 결과를
제공한다([M4 §23](WEBUI_M4.ko.md)). 원격 endpoint를 추가한 것은 아니며, 이 process-local
작업 ID는 HTTP 재시도 승인 ledger를 대신하지 않는다.
M4e-2c는 store의 opaque pack identity를 기존 DRC 읽기 actor와 대조하는 내부 Ticket과
선택 waive 상태 조회를 제공한다([M4 §24](WEBUI_M4.ko.md)). HTTP로 filesystem identity를
받지 않으며, owner는 승인 시 registry id/revision도 확인해야 한다. 선택 상태 메모리는
최대5000개지만 expected sidecar digest 검증의 O(파일 크기) I/O는 남아 있다.
M4e-3a의 실제 주석 URI·snapshot/승인 token·owner receipt·rebuild와 자원 수명·출력 보호는
[M4 §25](WEBUI_M4.ko.md)를 따른다. `--drc-reviewer`는 trusted launcher 설정이며 단일
owner bootstrap에 결합한다. 공유 계정의 실사용자 인증/RBAC를 구현한 것으로 보지 않는다.
M4e-3b의 owner 주석 UI는 이 API를 그대로 사용한다([M4 §26](WEBUI_M4.ko.md)).
그룹 선택 우선·명시 snapshot 조회·정규화 preview·별도 승인·동일 승인만 복구하는 UI다.
pan/restore/reload가 자동 저장하지 않으며 새 endpoint는 없다.
M4e-4a의 waive snapshot 적용은 내부 actor 명령이다([M4 §27](WEBUI_M4.ko.md)).
HTTP로 경로/상태 파일을 받아 재부착하지 않는다. M4e-4b는 이 적용 admission에서
같은 geometry id의 조회 `revision`을 바꾸고 `phase: updating` 동안 새 조회를 거부한다
([M4 §28](WEBUI_M4.ko.md)). 이전 ticket/HTTP commit·필터/선택 상태·준비된 focus는
새 조회에 섞이지 않는다. 실패·취소여도 이전 revision은 되살리지 않는다.
M4e-4c의 `/api/v1/drc/review/waives` read/prepare/승인/receipt/cancel은
`--drc-reviewer TAG --drc-edit-waives`의 별도 trusted opt-in으로만 활성화한다
([M4 §29](WEBUI_M4.ko.md)). 읽기 sidecar 등록이나 notes 권한만으로 켜지지 않는다.
prepare는 검증한 check/local 선택에 `waived:bool`만 적용한다. native kind/path/reviewer는
서버가 고정한다. 승인 worker가 파일 게시 전부터 조회 revision을 fence하고 게시한 동일
파일의 snapshot만 reader에 반영한다. `published`와 `reader_applied`는 각각 true/false/null로
분리되며, reader 실패/ACK 불명이 성공한 디스크 commit을 미게시로 바꾸지 않는다.
외부 변경 자동 재부착이나 autosave는 없다. M4e-4d UI는 이 API로 action/preview/별도
승인과 동일 요청 복구를 제공하고, 게시 성공뿐 아니라 reader ACK와 일치하는 ready
revision을 확인한 뒤 DRC 조회를 재개한다([M4 §30](WEBUI_M4.ko.md)). 새 endpoint는
없으며 실제 브라우저 게시·현장 수용은 후속이다.

M4g-14는 **웹의 확정 시 자동 저장 opt-in**을 추가한다([M4 §64](WEBUI_M4.ko.md)).
서버 API/권한/sidecar CAS는 그대로이며 카탈로그 `autosave:false`는 서버가 독자적으로
배경 저장하지 않는다는 뜻이다. 탭에서 등록된 reviewer·현재 연결에 대해 켠 경우,
사용자 확정 동작이 기존 read/prepare/승인 요청을 연결한다. opt-in 자체·단순 조회·
입력·blur·pan·restore는 저장하지 않는다. 해제/연결 변경은 미제출 승인 자격을
폐기하고, 이미 제출된 요청의 취소/receipt는 기존 의미를 유지한다. legacy 파일과
note 파싱 경고는 별도 확인이며 import/기본값 게시 승인도 자동화하지 않는다.
이 UI opt-in은 실사용자 인증/RBAC 또는 새로운 파일 쓰기 권한이 아니다.

M4g-15a의 `--floe-reviewer TAG`는 **명시 ICE/인접 파일의 읽기 전용 선택**이다
([M4 §65](WEBUI_M4.ko.md)). notes 카탈로그에 필수 `editable:bool`을 추가한다.
`false`일 때 GET notes status와 POST notes/display만 열고, edit snapshot/prepare/
submit/recovery와 전체 transfer/artifact API는 `review_disabled`로 거부한다. waive는
등록 reader의 기존 read API에 반영하며 waive writer 자체는 만들지 않는다. 이
모드의 reviewer는 initial reader에 고정하고 새 pack reader로 자동 승계하지 않는다.
`--drc-reviewer`는 기존대로 editable=true이며, waive 추가 권한은 여전히 별도다.
두 CLI 옵션을 동시에 써서 읽기 대상을 다른 쓰기 대상으로 해석하지 않는다.
`drc_notes` capability는 조회 기능의 존재도 포함하므로 쓰기 허용은 notes의
`editable`로 판단한다. 공유/게스트/RBAC 구현을 뜻하지 않는다.
M4g-20은 승인된 유도 legacy 임시 이름을 인접 파일 다음 읽기 후보로 연결한다
([M4 §75](WEBUI_M4.ko.md)). 경로는 launch에서 고정하며 HTTP 필드를 추가하지 않는다.
임시 디렉터리는 browse/쓰기 root가 아니고 read-only store는 native 초안/게시도
거부한다. waive는 검증된 FD를 reader에 설치하고, note는 기존 display 경로로만
읽는다. 다른 reviewer/pack으로의 자동 승계는 없다.
M4g-21은 명시 `--floe-reviewer`의 ASCII에서 현재 인접 ICE를 선택한다. 카탈로그
`drc.metadata.review_cache`는 `explicit`(직접 ICE), `cache`(현재 인접 ICE),
`missing`(캐시 없음, ASCII), `ignored`(오래되거나 무효인 캐시, ASCII), 그 외 등록은
null이다. UI는 ASCII fallback에 notes/waives 미적용 안내를 유지한다. 전체 경로나
원문 오류는 wire에 보내지 않는다. 새 path 입력/API·implicit build·browse root가
없고, fallback에는 note/waive 서비스도 만들지 않는다. 최초 선택과 actor open은
size/초 단위 mtime 일치를 검사하며 내용 hash/hot reload 보장은 아니다. writer와
reviewer 미지정 실행의 기존 explicit-file 선택은 변경하지 않는다.

M4g-24b/c의 런타임 조작은 owner의 `/api/v1/browse` operation을 재사용한다.
`open_drc {seq,handle,context}`는 선택 파일을 읽기 전용으로 교체한다.
`reconnect_drc_review {seq,context,approve:true}`는 런처 grant만 현재의 ready ICE에
명시 재연결한다. context는 `{view_id,drc_id,revision}`이며 raw path/reviewer/
editable/autosave 필드는 거부한다. `/api/v1/drc`의 `review_grant`는 null 또는 고정
reviewer, notes_editable, waives_editable, available을 내보낸다. grant 없는 실행에
권한을 추가할 수 없다. 읽기 전용은 유도된 sidecar 읽기만, writer는 인접 write
target만 사용하고 legacy-temp를 자동 채택하지 않는다. 선택·재연결은 파일 저장이
아니며 별도 save/transfer 승인과 브라우저 자동 저장 opt-in은 그대로 필요하다.

note/waive status의 `binding_id`는 재연결마다 바뀌고 `review_rev`도 증가한다.
저장/전송 ledger의 순번·high-water·terminal 결과는 유지한다. 각 receipt의
`scope_id`는 최초 admission의 binding_id이며 진행 응답/새 재연결로 바뀌지 않는다.
재시도는 이전 context를 수정하지 않은 동일 요청이어야 한다. 재연결 후에도 이전
terminal receipt는 조회/동일 replay할 수 있지만 새 DRC의 저장 결과로 해석하면
안 된다. prepared token·다운로드 capability·자동 저장 opt-in은 승계하지 않는다.

M4g-22의 capabilities `display_dump:bool`은 viewer의 브라우저-local dump 지원,
`dump_on_start:bool`은 trusted CLI `view --dump`의 초기 보관 선택이다. 일반 viewer는
true/false, 명시 `--dump`는 true/true, 독립 displaytest는 false/false다. 브라우저
토글은 로컬 메모리만 변경한다. 새 이미지 보관/다운로드 HTTP endpoint, path DTO,
권한 확대는 없다. 승인된 decoded frame과 합성 canvas를 각각1장 보관하며 이미지
다운로드는 사용자의 명시 동작이다. [표시 진단 §4](WEBUI_DISPLAY_DIAGNOSTICS.ko.md)의
상한·취소·민감 정보·성능/수용 경계를 따른다. frame ACK/credit이나 worker protocol을
새로 정의하지 않는다.
M4e-5a는 기존 주석 owner 등록에 `POST /api/v1/drc/review/notes/display`를 추가한다.
최대512개의 check/local 배지와 focus 하나의 본문만 반환하고 편집/게시 token을 만들지
않는다. context+notes review_rev에 고정한 admitted snapshot 하나를 재사용하며,
저장/재빌드·외부 변경 시 의미와 비용은 [M4 §31](WEBUI_M4.ko.md)을 따른다.
M4e-5b UI는 같은 endpoint만 사용하며 편집 snapshot/token을 소비하지 않는다.
기존 view panel body에 선택적 `note_target:{check,error}`를 추가했다. 마지막 ACK 이동
대상을 현재 선택/CD와 독립적으로 복원하며, 필드 생략/null은 대상 없음이다. 직렬화에서는
대상 없음의 필드를 생략한다. live jump와 pack 범위를 검증하고 본문/경로는 저장하지 않는다.
화면/경합·긴 본문·캡처 계약은 [M4 §32](WEBUI_M4.ko.md)을 따른다.
M4e-6a는 native review snapshot export/전체 waive import만 추가하며 endpoint 변경은 없다.
export는 미게시 sink, import는 trusted descriptor를 요구한다. portable run 확인·스트리밍·
expected revision/admission 계약은 [M4 §33](WEBUI_M4.ko.md)을 따른다.
M4e-6b는 `/api/v1/drc/review/{notes|waives}` 아래 `/transfer`와 `/artifacts/{id}`의
owner 전용 분할 업로드/비동기 작업/다운로드를 추가한다([M4 §34](WEBUI_M4.ko.md)).
1MiB chunk·파일512MiB(주석 입력16MiB, waive는 pack의 정확한 길이)·종류별2슬롯/1reader·
600초 TTL이며 브라우저가 path/reviewer/target을 지정하지 않는다. import prepare의
전체 교체 preview/token은 기존 root 승인 게시로만 소비한다. 전송 seq는 기존 게시 seq와
분리하고 HTTP ACK 소실 후 같은 seq/body로 재확인한다.
M4e-6c 패널은 같은 API와 기존 notes/waives 승인 경로만 사용한다
([M4 §35](WEBUI_M4.ko.md)). 파일 전체를 JSON/base64로 바꾸지 않고 1MiB Blob을
순서대로 보내며, UI의 전체 교체/run 확인이 승인 요청과 분리된다. 전송 중 선택 편집은
잠그고 기존 선택 editor가 있으면 import를 금지한다(export는 보존). 새 권한/자동 게시/
재접속 업로드 재개는 없다. 실제 브라우저 업로드·게시와 현장 수용은 별도다.
M1b-2a의 `app-core/managed`·`view`에는 process-local lease/admission과
독립 worker controller를 구현했다. M1b-2b의 사전 등록 view용 제어/이미지
스트림은 [M1b 기록 §6](WEBUI_M1B.ko.md#6-m1b-2b--인증된-제어이미지-스트림)을 따른다.
M1b-2c1의 등록 scope/index supervisor는 M1b 기록 §7, M1b-2c2의 owner
catalog/open/index 작업 API는 §8을 따른다(통합 `/operations` seq 계약).
M1b-3의 실행/정적 자산·startup과 브라우저 계약은 §9를 따른다. 단일 layer
checkbox는 `view.set`의 `layer_change:{pair:[layer_u32,datatype_u32],visible:bool}`로
처리하고 서버에서 head/child 의미를 적용한다. `layers` 전체 선택과 같은 요청이면
전체 선택을 먼저 적용한다. 잘못된 pair나 선택 수 상한은 오류이며 조용히 자르지 않는다.
M2a-10d1의 `focus(isolate=true)` 준비 토큰과 `view.apply`, `view.set.restore_layers`,
snapshot `layers_isolated`는 [M2 §18](WEBUI_M2.ko.md#18-m2a-10d1-레이어-격리복원-코어와-원자적-focus-api)을 따른다.
M2a-10d2에서 double-click/Frame error/이동 순회와 Restore/Escape를 연결했다.
CD·live In view 등 종속 UI는 승인+같은 state_rev의 snapshot 후 반영하며,
불확실한 입력은 재접속 시 재전송하지 않는다. jobdeck 물리 plane 격리는 아직 미지원이다.
M2a-11a의 CLI는 fresh 인접 ICE 우선/ASCII fallback이다(§20). M2a-11b의 웹 actor는
명시 등록한 ICE/ASCII 파일만 읽으며 인접 pack/reviewer를 탐색하지 않는다(§21).
catalog의 format과 geometry의 points_dbu 또는 points_um으로 좌표 단위를 구별한다.
브라우저의 임의 경로 입력이나 암묵적 index 실행은 허용하지 않는다.
M2a-12a의 `drc/build::Build`는 명시 pack 생성·자원/쓰기 lease·진행/취소·검증/게시
코어이며 CLI `drc --build`에서 호출한다(M2 §22). M2a-12b1은 owner 전용
`POST /api/v1/drc/builds`(seq/drc_id/revision/view_id/approve/force/jobs),
`GET /api/v1/drc/builds[/{seq}]`, `POST /api/v1/drc/builds/{seq}/cancel`를 연결했다.
명시 동의 → 현재 DRC 응답/token 무효화·actor 종료/lease 해제 → 생성 → 새 리뷰
identity 등록 순서다. native 게시 결과와 새 reader의 opening/ready/error를 구분한다.
`GET /api/v1/drc`의 별도 build capability/ledger는 owner 작업이며 읽기 공유 권한이
아니다. M2a-12b2는 이 capability를 읽는 별도 브라우저 controller다. 생성 POST가
전송된 뒤 결과가 불명확하면 새 seq를 자동 발급하지 않고, 사용자 명시 확인만
원래 seq/옵션/identity를 재전송한다. GET/재접속은 생성 동의가 아니다.
실제 브라우저 승인 클릭 수용은 별도다(서버 M2 §23, UI §24).
M4a-1/2의 scene-pinned query는 **로컬 native process client와 ViewController**까지 구현했다.
frame/scene 구분·거부 코드·표시 anchor·종류별 취소·수명/상한은 [M4 기록](WEBUI_M4.ko.md)을 따른다.
M4a-3은 기존 인증된 owner WebSocket의 `view.query`/`view.query.cancel`에 연결했다.
layout snapshot query capability는 true, deck은 false다. 표시 ACK+write receipt,
연결별 결과·정수 문자열 DTO는 M4 §3이 기준이다. REST query·공유 권한은 추가하지 않았다.
브라우저 pick/snap은 M4a-4, Rust 좌표 계산 기반 수동 ruler와 owner `view.measure`는
M4a-5에서 연결했다. M4a-6의 owner `view.measure_selection`은 같은 표시 receipt로
최대64개 bbox 주석의 자동 gap을 계산한다(geometry 재검색/새 권한 없음).
clip과 실제 브라우저 수용은 별도이며, 표시/DPR/최신 입력·응답 검사는 M4 §4~6을 따른다.
M4c-1의 `exports::clip::Job`/`exports::artifacts::Store`는 관리형 exact clip·만료
파일 소유 코어다(M4 §12). M4c-2는 owner `view.clip.prepare`와 별도 `/exports`
ledger, 승인/취소·artifact metadata/폐기·chunk 다운로드를 연결했다(M4 §13).
다운로드 GET는 cookie+header CSRF, native browser form POST는 cookie+고정 body
CSRF로 인증한다. M4c-3은 현재 viewport 준비/승인·취소·파일 목록/다운로드 UI다.
`GET /exports`의 `artifacts` 목록은 작업 이력32개와 독립적이므로, 이력에서 사라진
ready 파일도 TTL 안에는 발견/다운로드/폐기할 수 있다. 전역 `capabilities.exports`와
view의 `capabilities.clip`(layout만 true)을 함께 요구한다. 임의 파일·guest 권한·자동
export는 없고 native form은 같은 출처 POST body에만 CSRF를 넣는다.
M4d-1의 owner `capabilities.snapshot_png`는 브라우저 표시 pixels의 복사/PNG 저장
UI만 광고한다. layout/jobdeck 공통이며 `capabilities.clip`·query 지원과 독립적이다.
browser 기능 검사를 추가로 요구하고 새 HTTP/WS·clipboard 읽기·upload는 없다.
표시/overlay·수명·fallback 계약과 미검증 범위는 [M4 §15](WEBUI_M4.ko.md)를 따른다.
M4d-2는 layerprops library codec·초기 view visibility를 연결했다. 명시적인 startup
selection이 design default보다 우선하며, 기존 RenderSession의 archival 기본 All은
유지한다([M4 §16](WEBUI_M4.ko.md)). M4d-3은 owner `capabilities.layer_settings`와
`GET/POST /api/v1/views/{id}/settings/{state_rev}/{native|calibre}`를 추가한다.
GET는 현 revision의 설정 text 다운로드, POST는 UTF-8 text의 읽기 전용 준비다.
POST 결과의 `prepared_token`만 기존 `view.apply`로 보내 같은 revision CAS에서 적용한다.
`rows`는 읽은 행 수이지 유효 pair/변경 행 수가 아니다. 세부 상속/한계는 [M4 §17](WEBUI_M4.ko.md).
cookie+CSRF·Origin 정책은 기존 owner 규칙이다. 등록된 view의 설정 text만 최대4MiB,
한 준비/내보내기를 별도 semaphore로 허용하며, 일반16KiB body·8KiB control 한계는
유지한다. 임의 경로·레이아웃 upload·source/default 쓰기 권한은 아니다(`uploads:false`).
M4g-16b에서 native 설정은 기존 `floe.layers` v1과 슬롯 보존 v2를 읽는다. v2는
`fill_slots:[{name,rows:[u16;16]},…]` 전체20항목과 행의 선택적 `fill_slot` 참조를
포함한다. 참조 행은 필수 `fill`이 null이며 직접 값과 동시에 지정하지 않는다.
v1은 값만 복원하고 v2는 미사용 슬롯 편집까지 보존한다. 누락/중복/고정 슬롯 변경은
문서 전체 오류다. M4g-16c의 별도 슬롯 API는 위에 기술한다. M4g-16d 웹 프리셋은
참조 기반이며 슬롯 편집 UI의 Apply 후에도 이 명시적 설정 저장 경로만 파일을 만든다.
기존 prepare/승인·한계·권한을 유지한다([슬롯 계약 §4](WEBUI_BITMAP_SLOTS.ko.md)).
`view.set.body.style_deltas`는 `{pair,color?,fill?,width?}`의 필드별 수정이다.
omitted는 유지, null은 오류이며 기존 완전한 `styles`와 한 요청에서 혼용하지 않는다.
M4d-4a의 `layer_defaults::Publisher`에 M4d-4b의 owner API를 연결했다.
`FLOE_FILL_EDIT`가 비어 있지 않은 launcher만 `design_defaults:true`이며 기본은 false다.
`layer_settings`는 공유 default 쓰기 권한이 아니다. 인증/CSRF 후
`POST /api/v1/defaults/prepare {view_id,state_rev}`는 경로/내용 입력 없는 읽기 전용 준비,
`POST /api/v1/defaults {seq,view_id,state_rev,token,approve:true}`는 별도 명시 승인이다.
preview는 basename/mode/levels/title/rows/bytes/교체 여부·30초 토큰을 반환한다.
`GET /api/v1/defaults[/{seq}]`, `POST /api/v1/defaults/{seq}/cancel`,
`POST /api/v1/defaults/revoke {token}`로 유계 receipt 조회·취소·미승인 draft 폐기가 가능하다.
seq는 owner open/index와 다른 namespace이며 같은 body 재전송은 한 번만 실행된다.
게시 API의 상태·파일 보호·예산은 [M4 §19](WEBUI_M4.ko.md)를 따른다.
M4d-4c의 UI(§20)는 preview와 체크 승인을 분리하며, 승인 POST 직전의 원래 요청을
owner session에 고정해 sessionStorage에 하나만 보관한다. 새로고침/GET은 게시 동의가 아니다.
미확정 상태의 Resolve 클릭만 같은 seq/body를 재전송하고, 게시 여부를 알 수 없는
`operation_expired`나408을 미게시로 단정하지 않는다. token/seq는 URL에 넣지 않는다.
게시 후 디렉터리 sync 실패는 committed 경고이지 재시도 가능한 미게시가 아니다.
receipt는 서버 메모리 수명만 보장하며 프로세스 재시작 뒤 자동 재시도 근거가 아니다.
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
| 표시 pixels PNG 복사/저장 | 브라우저 기능 검사, 실제 수용 미확인 | 동일(원본 geometry export 아님) |
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
혼동하지 않는다. 구현된 owner 경계(M4g-8)는 [M4 §51](WEBUI_M4.ko.md)을 따른다:
`GET /api/v1/browse`(허가된 roots와 seq cursor), `POST /api/v1/browse`
(list/page/select), `GET /api/v1/browse/{seq}`(최대32개 이력),
`POST /api/v1/browse/{seq}/cancel`. source 경로·argv·추가 root는 HTTP에 없다.
목록/선택은 전용 단일 작업자가 수행하고 접수는 즉시202를 반환한다. seq와 요청
내용을 결과에 함께 실어 다른 연결의 같은 seq 응답을 내 선택 결과로 채택하지 않는다.
선택 결과의 `launch_id`는 기존 owner 열기 제안이며 파일 읽기 capability가 아니다.
M4g-8b의 open `display_policy=explicit|window`는 생략 시 explicit이다. picker는
window를 발급하고 실행 시점의 revision-검증된 이전 view에서 표시 선호를 계승한다.
CLI는 기존 body를 유지하며 `label_preference` bool로 잡덱 capability에 가리기 전
라벨 선호를 전달한다. 두 필드는 경로·색인·출력 쓰기 권한이 아니다. null/미정의 값은
거부하며 receipt 서명과 재시도 identity에 포함한다. 세부 계약은 [M4 §52](WEBUI_M4.ko.md).

업로드 지원을 M1에 암묵적으로 넣지 않는다.
DRC import는 등록된 입력/artifact로 처리하고 include 탈출을 막는다. M4d-3 설정만은
사용자가 선택한 UTF-8 text의 유계 import를 명시적으로 추가했다. 설정 문서에는
서버 경로/include 명령이 없으며 source나 sidecar를 자동으로 쓰지 않는다.

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

초기 snap/pick wire에는 `seq`만 있어 조회 scene의 gen/round를 증명할 수 없었다.
M4a-1에서 expected/actual scene ID와 캡처한 immutable Arc의 상태 검증을 추가했다.
render thread와 margin이 scene을 교체할 수 있으므로 gateway에서 "지금 표시한
gen"을 응답에 붙이는 것으로 대체하지 않는다. 상태별 코드는 [M4 §1](WEBUI_M4.ko.md)을 따른다.

M4a-2의 로컬 controller는 dataset revision·worker epoch·실제 표시 frame ID·
state/render revision·render key를 고정한다. 현재 viewport의 좌표를 서버에서
DBU로 변환하고, 가시 레이어·scene 완료 여부와 알려진 margin coverage를 검사한다.
종류별 latest-only 질의와 명시 취소는 render와 독립이며 늦은 응답은 폐기한다.
요약-only/지원하지 않는 deck을 "찾지 못함"으로 위장하지 않는다.

M4a-3은 인증된 owner·view ID·connection epoch와 **표시 완료로 ACK한 packet**의
anchor를 연결한다. matching ACK와 writer 완료를 모두 확인한 foreground/margin
각1개만 연결별로 보관한다. native 오류는 safe code로 바꾸고, 다른 연결의 결과는
반환하지 않는다. 새 연결은 예전 receipt/질의를 재사용하지 않는다.
서버는 DOM을 확인하지 못한다. M4a-4에서 CSS/DPR 좌표·실제 표시 대상 선택·응답 사용 직전
stale 검사를 UI에 연결했으며 결정적 하네스와 실제 브라우저 수용은 구별한다.
로컬 anchor는 접근 권한 토큰이 아니다.

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
