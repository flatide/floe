# floe cell-local Text VFS 구현 계획

2026-08-04 초안. 대상은 현재 `design.ovm` v4와 계층 VFS를 사용하는
`floe-index`/`vfsd`/Python viewer다. 이 계획은 `VFS_HIER.md` rev 27의
잔여 위험인 `collect_all_texts` 경로 전개와 `labels.tsv`/`texts.tsv`
상주를 제거한다.

상태: **T1~T4 구현 완료** (2026-08-04, 0.11.0, 커밋 b6110d7 /
7ae8529 / a7f17a8 / 744e3e4 — milestone별 커밋, 각 gate green 후
다음 단계 삭제 원칙 준수). 기록과 계획 대비 편차(오픈 딥 검증 =
from_bytes 전용, publish = 기존 마커 규약 유지, 라벨 행 l/d 표기)는
rust/VFS_HIER.md rev 28 참조. T5(전역 검색)는 미착수(요구 발생 시).

---

## 1. 결론과 범위

Text는 geometry와 논리적으로 분리하지만 hierarchy 밖으로 평탄화하지
않는다. 원본 cell의 local text record를 한 번만 저장하고, 기존 VFS
planner가 방문한 cell/placement/repetition에 대해서만 현재 viewport의
text를 top 좌표로 해석한다.

최종 데이터 흐름은 다음과 같다.

```text
OASIS cell-local texts
  -> cell/layer별 TextRecord + text BVH
  -> design.ovm/design.ovt에 기록
  -> vfsd hierarchy traversal 중 local viewport query
  -> visible repetition member만 좌표화
  -> Rust screen-space declutter/budget
  -> generation별 소형 labels 결과
  -> Python/KLayout working layout
```

이 계획의 범위:

- `collect_all_texts()`와 hierarchy 전체 text-path 전개 제거
- `labels.tsv` 전체 로드 제거
- VFS 캐시의 `texts.tsv` 생성 제거
- cell-local text metadata와 spatial index를 mmap으로 조회
- layer visibility, semantic depth, transforms, repetition을 반영한
  viewport text query
- cut frame의 block name을 OVM cell name에서 런타임 생성
- generation/stale-safe label 전달과 기존 GTK/KLayout 표시 유지
- 정확성, 결정성, 손상 캐시, 메모리와 실칩 성능 gate 추가

초기 범위에서 제외:

- substring/정규식용 전역 full-text 검색 엔진
- text glyph를 직접 rasterize하는 새 renderer
- geometry exact page 포맷 변경
- 실행 중 캐시 재빌드 또는 다중 writer 지원
- 기존 v4 캐시의 in-place 변환

전역 text 검색은 화면 표시와 다른 인덱스가 필요하므로 §7의 T5 후속
milestone로 분리한다.

---

## 2. 현재 문제와 불변조건

### 2.1 현재 문제

현 VFS build는 `collect_all_texts(doc)`에서 text를 가진 cell까지의 모든
hierarchy path를 `TextEntry`로 만들어 상주시킨다. 반복은 symbolic이어도
동일 cell로 가는 path 수가 많으면 entry 수가 source text record 수보다
훨씬 커진다. 그 뒤 label 선택, sidecar 정렬, TSV 기록이 같은 entry
집합을 소비한다.

따라서 peak RSS와 build 시간은 source text 수가 아니라 **전개 가능한
hierarchy path 수**의 영향을 받는다. mmap은 이 평탄화 결과를 그대로
저장하는 한 근본 해결이 아니다.

### 2.2 반드시 유지할 불변조건

- source text는 원래 cell의 local 좌표로 정확히 한 번 저장한다.
- placement hierarchy와 placement repetition은 기존 OVM을 유일한
  근거로 사용하며 text index에 복제하지 않는다.
- text 자체의 `One/Grid/Pts` repetition member identity와 중복 좌표를
  보존한다.
- geometry와 동일하게 visible layer와 semantic depth를 적용한다.
- 회전/반전/이동과 skew Grid를 포함해 후보 누락이 없어야 한다.
- declutter 전 candidate 집합은 source/KLayout oracle과 같아야 한다.
- declutter 후 결과는 budget 이하이며 같은 요청에 대해 결정적이어야
  한다.
- 이전 generation의 label은 화면에 반영하지 않는다.
- Rust 구조체 직접 캐스팅 없이 little-endian accessor와 bounds check를
  사용한다.
- build의 모든 count/offset/size narrowing은 checked 변환을 사용한다.
- cache build는 현재 운영 원칙대로 전체 cache 삭제 후 수행하고,
  `design.ovm`을 마지막에 publish한다.

---

## 3. 저장 포맷

### 3.1 파일 구성

```text
<source>.floe/
  design.ovp    geometry OASIS payload, 현행 유지
  design.ovt    text string/Pts 가변 payload, 신규
  design.ovm    geometry + text fixed metadata, mmap, 최종 commit marker
  meta.json     사용자/도구용 요약
```

`design.ovm`은 v4에서 **v5**로 올린다. v4를 조용히 해석하지 않고 기존
version gate로 명확한 rebuild 오류를 반환한다. 기존 캐시는 변환하지
않고 원본 OASIS에서 다시 만든다.

`design.ovt`를 별도로 두는 이유는 다음과 같다.

- 문자열과 대형 Pts는 가변 길이라 fixed OVM table과 수명이 다르다.
- build 중 `BufWriter`로 순차 기록해 전체 string pool 복사를 피한다.
- runtime에는 read-only mmap으로 열어 선택한 문자열/point chunk만
  page-in한다.
- geometry OASIS payload인 `design.ovp`의 byte 포맷을 바꾸지 않는다.

v5 header에는 `ovt_len`을 넣고 open 시 실제 `design.ovt` 길이와
대조한다. publish 순서는 다음으로 고정한다.

1. cache directory의 기존 산출물 삭제
2. `design.ovp.tmp` 작성 후 `design.ovp` 확정
3. `design.ovt.tmp` 작성 후 `design.ovt` 확정
4. 두 payload 길이를 담은 `design.ovm.tmp` 작성
5. `meta.json.tmp` 작성 후 `meta.json` 확정
6. `design.ovm` rename으로 유일한 최종 commit

중단 시 `design.ovm`이 없으므로 viewer는 불완전 cache를 열지 않는다.

### 3.2 OVM v5 신규 section

정확한 stride/offset은 T0 wire spike에서 동결하되 논리 section은 다음과
같다.

```text
TEXTS       fixed TextRecord 배열
TRANGES     (cell_id, layer_idx)별 연속 text run + TBVH root
TBVH        packed text BVH
TREPS       text 자체 repetition descriptor
TCHUNKS     대형 Pts repetition의 chunk bbox/OVT offset
```

Cell record에는 `trange_start/trange_count`를 추가한다. 별도 global
cell-to-range map을 두는 것보다 기존 cell 접근 시 한 번에 range를 찾을
수 있고 hierarchy traversal의 locality가 좋다.

개념적인 `TextRecord` 필드는 다음과 같다.

```text
cell_id       u32
layer_idx     u32
x, y          i64, i64       local anchor
string_off    u64            design.ovt byte offset
string_len    u32
rep_idx       u32            TREPS index, NONE 가능
bbox          4 * i64        local repetition extent 포함
source_seq    u32            결정적 tie-break
flags         u32            예약; display/search 분류
```

Text run은 `(cell_id, layer_idx)` 순서로 정렬한다. run 내부는 local bbox
중심 Morton key와 `source_seq` tie-break로 결정적으로 정렬한다. jobs 수가
달라도 section과 payload byte가 같아야 한다.

### 3.3 문자열

문자열은 UTF-8 원문 byte를 `design.ovt`에 저장한다.

- 1차 구현은 source 순서의 순차 저장으로 하며 build 중 전역
  `HashMap<String, ...>` dedup을 두지 않는다. 전역 dedup이 source보다
  큰 heap을 만들 수 있기 때문이다.
- 동일 cell 안의 작은 bounded map dedup은 T0 실측에서 이득이 확인될
  때만 허용한다.
- `string_off + string_len <= ovt_len`을 open 시 전 record에 대해
  checked 검증한다.
- runtime accessor는 `&[u8]`를 빌리고, 최종 선택된 label에 대해서만
  UTF-8 검증/`String` 생성을 한다.
- 유효하지 않은 UTF-8 정책은 기존 parser의 text 문자열 정책과
  동일하게 고정한다. 임의 lossy 변환을 추가하지 않는다.

### 3.4 Text repetition

- `One`: descriptor 없이 anchor 하나.
- `Grid`: `na/nb/va/vb`를 fixed descriptor에 저장한다. viewport query는
  기존 skew Grid 역행렬/보수 경계 계산을 공유한다.
- `Pts`: Morton 순서 point pool과 chunk bbox를 `design.ovt`/`TCHUNKS`에
  기록한다. 동일 좌표도 서로 다른 member로 유지한다.

Text Pts는 geometry placement Pts와 동일한 규칙을 재사용한다.

- 전체 extent는 1차 교차 판정에만 사용한다.
- 좁은 viewport에서는 교차 chunk만 scan한다.
- 결과 0/1/2+ member를 정확히 처리한다.
- count 또는 byte 범위가 format 한계를 넘으면 조용히 truncate하지
  않고 `limit exceeded: text ...`로 build를 실패시킨다.

### 3.5 Text BVH

TBVH는 `(cell, layer)`별 root를 갖는다. leaf는 같은 run 안의 연속
TextRecord 범위만 가리킨다.

넓은 Grid/Pts record 하나의 bbox가 cell 전체를 덮더라도 record 하나를
조회하는 비용만 발생하며, 실제 member 선택은 repetition query가 한다.
이를 여러 text page에 복제하지 않는다.

open 검증은 다음을 포함한다.

- 모든 trange의 cell/layer/text run 소유권
- TBVH root/child index 범위
- cycle 없음과 root별 도달 노드의 유일성 또는 명시된 공유 규칙
- leaf 범위가 해당 trange의 text run 내부
- leaf text의 cell/layer가 trange와 일치
- repetition/chunk/string offset과 길이가 `design.ovt` 내부
- bbox가 비어 있지 않고 integer 연산 overflow가 없음

손상은 panic/무한 순회가 아니라 `corrupt cache; rebuild`로 끝난다.

### 3.6 Build 메모리 규칙

OVM v5 text section을 기존 `Builder`의 거대한 `Vec`에 무조건 추가하지
않는다. build 중 section별 임시 spool과 bounded batch를 사용하고 최종
OVM을 section 순서로 복사한다.

- source `Doc`이 존재하는 첫 구현에서도 `doc.cells[ci].texts`만 읽고
  hierarchy path를 순회하지 않는다.
- 한 번에 유지하는 상태는 현재 cell 또는 설정된 batch의 TextRecord,
  BVH scratch와 bounded string buffer뿐이다.
- `design.ovt`는 순차 `BufWriter`로 기록한다.
- 최종 OVM 조립 시 section 전체의 추가 사본을 만들지 않는다.
- 향후 streaming parser/cell spool로 바뀌어도 wire format을 바꾸지
  않고 같은 writer를 사용할 수 있게 한다.

---

## 4. Planner와 표시 정책

### 4.1 Candidate query

Planner가 `WsKey=(cell_id, remaining_depth)`를 방문할 때 다음 순서로
text를 조회한다.

1. visible layer bitset으로 cell의 trange root를 자른다.
2. top viewport/localview K-box를 현재 cell local 좌표로 역변환한다.
3. 각 local box로 TBVH를 조회하고 TextRecord index를 dedup한다.
4. `One/Grid/Pts`에서 viewport와 교차하는 member만 열거한다.
5. 현재 placement transform을 적용해 top DBU 좌표를 얻는다.
6. semantic depth 규칙으로 방문하지 않은 subtree의 text는 조회하지
   않는다.
7. candidate budget을 적용하기 전 정확 candidate 수와 scan 지표를
   기록한다.

회전/반전된 box의 역변환은 네 모서리의 보수적 bbox를 사용한다.
후보 누락은 허용하지 않으며, 과다 후보는 최종 top-coordinate point
test에서 제거한다.

동일 cell이 여러 localview box 또는 placement path로 보이는 경우 text
identity는 `(WsKey, placement path/member identity, text_idx,
text-member)`다. 좌표가 같다는 이유로 source member를 정확성 단계에서
합치지 않는다. 화면 declutter 단계에서만 여러 candidate 중 하나를
선택할 수 있다.

### 4.2 Screen-space declutter

전체 candidate를 Python에 보내지 않고 Rust에서 표시 집합을 고른다.

- layer visibility를 먼저 적용한다.
- screen bin 크기와 총 `LABEL_VIEW_BUDGET`은 기존 viewer 상수에서
  시작하되 request 또는 daemon 설정으로 조정 가능하게 한다.
- 선택 우선순위는 `block-name > user text`, zoom 적합도, layer order,
  안정적인 identity hash 순으로 고정한다.
- 같은 viewport/request에서 jobs, hash seed, iteration order와 무관하게
  결과가 같아야 한다.
- 연속 pan에서 label이 불필요하게 깜빡이지 않도록 bin origin은 가능하면
  viewport origin이 아니라 world DBU의 screen-scale grid에 고정한다.
- 후보/선택/예산 탈락 수를 각각 계측한다.

### 4.3 Block name

큰 1단계 cell의 block-name row는 저장하지 않는다. Planner가 cut frame
또는 block outline을 생성할 때 이미 가진 `cell_id`, transformed bbox,
OVM cell name으로 label candidate를 만든다.

따라서 다음 항목이 제거된다.

- `BLOCK_ROW`
- `blk` TSV sentinel
- pseudo-layer와 실제 `u32::MAX` layer 충돌
- Rust/Python frame-layer 계산 차이에 의한 block label 분류 오류

Frame geometry의 runtime layer는 별도 문제로 유지할 수 있지만 block
label의 의미를 layer 번호로 인코딩하지 않는다.

### 4.4 점진 refinement

Label query는 geometry page decode보다 먼저 끝나는 metadata-only
작업이다. 첫 partial 응답에 표시할 label을 포함해 검은 영역이 남아
있는 동안에도 cell/text 문맥을 제공한다.

- 같은 generation의 후속 refinement는 기본적으로 label을 재전송하지
  않는다.
- viewport, depth, cut, visible layer가 바뀌면 새 generation에서 다시
  계산한다.
- label query budget을 넘으면 geometry refinement와 독립된 cursor로
  이어갈 수 있으나, T2 실측 전에는 한 요청에서 bounded selection을
  끝내는 단순 경로를 기본으로 한다.
- stale generation의 label file/response는 apply하지 않고 즉시 폐기한다.

---

## 5. daemon/Python 인터페이스

### 5.1 요청

기존 view request의 다음 값으로 text 결과가 완전히 결정되어야 한다.

```text
generation
viewport bbox
pixel width/height 또는 pixels-per-dbu
semantic depth
visible layer IDs
cut level
```

별도 Python-side 전역 label filtering은 하지 않는다.

### 5.2 응답

1차 구현은 현재 `VfsMosaic.apply_hier(..., labels=...)`를 재사용한다.
vfsd가 generation별 bounded label TSV를 임시 파일로 기록하고 응답에
`labels=<path>`와 `nlabels=N`을 추가한다.

행은 의미를 명시적으로 가진다.

```text
kind  layer_idx  x  y  escaped_utf8
```

- `kind=text`: 실제 design layer를 `layer_idx`로 참조
- `kind=block`: layer와 무관한 runtime annotation

raw `(layer, datatype)`나 sentinel 값으로 kind를 표현하지 않는다.
최종 label 수가 작으므로 이 임시 TSV는 mmap 대상이 아니다. 대형 영구
데이터만 mmap하고, request 결과는 단순하고 bounded한 전달 형식을
유지한다.

프로토콜 수명 규칙:

- label 결과는 names table과 달리 **view-generation 자산**이다.
- client는 generation stale 검사 후에만 읽는다.
- 성공 apply 또는 stale/drop 시 파일을 삭제한다.
- 부분 apply 실패/reset 시 geometry와 label을 함께 버린다.
- daemon session ledger의 resident page bytes에는 label 임시 파일을
  포함하지 않는다.

추후 IPC profiling에서 파일 생성이 병목일 때만 length-prefixed binary
응답 또는 delta OASIS text cell로 바꾼다.

### 5.3 Python 정리

전환 완료 후 다음 전역 상태를 제거한다.

- `_load_sidecar_labels()`
- `cache._live_labels`
- Python `_view_labels()`의 전체 리스트 선형 scan
- `meta.json.labels.file`

Python은 daemon이 고른 bounded label rows를 현재 generation의 ephemeral
label cell에 넣는 역할만 유지한다.

---

## 6. 계측

Build heartbeat에 다음 단계를 추가한다.

```text
[vfs] text-index: cells C/C, records R, strings MB,
      reps G/P, spool MB, elapsed S, rss X
[vfs] text-index: bvh nodes N, ovt MB, elapsed S, rss X
```

5초 heartbeat는 parsing/build/encoding과 동일하게 유지한다. 완료 시
다음을 `meta.json` 또는 build summary에 기록한다.

- source text records와 members
- text-bearing cells와 `(cell, layer)` ranges
- One/Grid/Pts record 수
- Pts offsets/chunks 수
- `TEXTS/TRANGES/TBVH/TREPS/TCHUNKS` byte 수
- `design.ovt` 문자열/point byte 수
- text index wall time과 단계별 peak RSS
- 제거된 flattened entry 수는 새 build에서 생성하지 않으므로 0이
  아니라 `not_materialized`로 표시한다

Planner 응답/상태줄에는 최소 다음을 추가한다.

```text
text_bvh_nodes
text_records_candidate
text_rep_chunks_scanned
text_members_tested
text_members_visible
labels_selected
labels_budget_dropped
text_plan_ms
```

geometry `plan_ms` 안에 포함하되 `text_plan_ms`를 별도로 제공한다.

---

## 7. 단계별 구현

### T0 — wire/query spike와 baseline

목표: 포맷을 동결하기 전에 text 규모와 query 정확성을 확인한다.

- `valmini`, text repetition 합성 fixture, `testchip_1g5`, 9.8G에서
  source text records, 기존 flattened entries, sidecar byte, text 단계
  wall/RSS를 수집한다.
- cell-local `TextRecord/Grid/Pts` wire round-trip을 unit test로 만든다.
- 기존 placement Grid/Pts query 코드를 text에서 공유할 수 있도록
  순수 iterator 경계를 확인한다. geometry 동작은 바꾸지 않는다.
- source/KLayout에서 고정 viewport의 raw text candidate oracle을 만든다.
- `TextRecord` stride, v5 header/section offset과 `design.ovt` layout을
  동결한다.

Gate:

- hierarchy path를 전개하지 않고 모든 source text/repetition을 round-trip
- skew Grid/Pts viewport candidate XOR 0
- 1억-member Grid에서 좁은 view query가 전체 member 수에 비례하지 않음
- 100만 Pts에서 intersecting chunk만 scan

### T1 — OVM v5 text index build/open

목표: viewer 동작을 바꾸지 않고 새 text index를 생성·검증한다.

- OVM v5 section/accessor/bounds validation 구현
- `design.ovt` streaming writer와 publish lifecycle 구현
- cell/layer trange, TBVH, text repetition encoding 구현
- build heartbeat/summary 구현
- `tools/validate_vfs.py`에 v5 구조 검증 추가
- 기존 `labels.tsv/texts.tsv`는 비교용으로 이 단계까지만 생성

Gate:

- source-local record/member/string count 일치
- 모든 corrupt fixture가 `corrupt cache; rebuild`로 종료
- jobs 수와 무관하게 `design.ovm/design.ovt` byte 동일
- kill-at `ovp-written/ovt-written/ovm-partial`에서 marker 부재 또는
  정상 cache만 관찰
- text section build peak가 flattened path 수에 비례하지 않음

### T2 — planner text query와 shadow 비교

목표: viewer에 표시하지 않고 새 planner 결과를 기존 label 결과 및
oracle과 비교한다.

- hierarchy traversal에 text range/TBVH query 추가
- One/Grid/Pts visible-member 계산 추가
- block name runtime candidate 추가
- Rust declutter와 지표 추가
- `plan` command에 raw/selected label dump를 test flag로 제공

Gate:

- declutter 전 candidate가 고정 viewport oracle과 XOR 0
- depth 0/1/full 및 layer toggle 정확
- rotate/flip/skew/중복 Pts/다중 path 정확
- candidate iteration 순서를 섞어도 selected 결과 동일
- 요청당 selected label이 budget을 넘지 않음
- generation 취소 후 stale label이 결과에 없음

### T3 — daemon/viewer 전환

목표: Python 전체 label 목록을 제거하고 request-scoped label만 표시한다.

- vfsd label 응답과 임시 파일 lifecycle 구현
- service generation/stale/apply/reset 연결
- `VfsMosaic`의 ephemeral label cell 재사용
- 기존 sidecar와 새 결과를 선택하는 임시 비교 flag 제공
- GUI layer visibility와 block annotation 동작 회귀 검증

Gate:

- 첫 frame부터 label 표시 가능
- pan/zoom/depth/layer toggle에서 이전 generation label 잔존 0
- label apply fault injection 후 reset 복구
- viewer start 시 text 전체 scan/heap load 0
- pick/snap/ruler/DRC goto/clip 회귀 없음

### T4 — 기존 sidecar 제거와 cutover

목표: 메모리 문제의 원인을 실제 production build에서 삭제한다.

- VFS build의 `collect_all_texts()` 호출 제거
- `labels.tsv`와 `texts.tsv` 생성/marker 정리 목록/meta 항목 제거
- `BLOCK_ROW`, `label_rows()`의 VFS 의존 제거
- `_load_sidecar_labels`, `_view_labels`, `_live_labels` 제거
- 비교 flag와 v4 cache 지원 제거, rebuild 메시지 확정
- portable bundle/selfcheck에 v5 open/text-plan smoke 추가

Gate:

- repository VFS production 경로에서 `collect_all_texts` 참조 0
- cache에 `labels.tsv/texts.tsv`가 생성되지 않음
- 전체 Rust/Python/portable validation green
- v4 cache는 명확한 rebuild 오류, v5 cache는 정상 open

### T5 — 선택 사항: 전역 text 검색

화면 표시 cutover 이후 실제 제품 요구가 있을 때만 진행한다.

- exact search: 정렬된 `(hash, string bytes, cell_id, text_idx)` mmap index
- prefix search: 문자열 정렬 index의 range query
- substring/regex: 실측 후 trigram/FST 또는 별도 SQLite/검색 파일 결정
- 검색 결과는 source-local identity로 반환하고 placement 좌표 resolve는
  viewport 한정 또는 pagination한다
- “모든 hierarchy occurrence를 한 번에 반환”하는 API는 금지한다

검색 index가 없어도 화면 text 표시와 VFS 정확성은 완전해야 한다.

---

## 8. 테스트 자산과 승인 기준

### 8.1 필수 합성 fixture

- 동일 text-bearing cell을 깊은 DAG의 여러 path에서 재사용
- 1억-member orthogonal/skew Grid text repetition
- 100만 Pts, 무작위 순서, 중복 좌표, viewport 안/밖 혼합
- 회전 0/90/180/270, flip, 음수 좌표와 i64 경계 근접
- 동일 layer의 조밀 text와 여러 layer visibility 조합
- 실제 design layer `255/0`과 `u32::MAX/0`
- text 없는 cache와 빈/매우 긴/escape 포함 문자열
- TBVH root/child/leaf/trange/OVT offset 손상 fixture

### 8.2 정확성

- raw candidate는 `valmini`, 합성 fixture와 실칩 표본 viewport에서
  source/KLayout oracle과 layer별 identity XOR 0
- semantic depth와 transform별 누락 0
- block name은 cut frame의 실제 cell name/placement와 일치
- selected label은 raw candidate의 부분집합
- jobs 및 반복 실행과 무관하게 cache와 selected 결과 결정적

### 8.3 메모리

- text build peak 추가분은 source-local text index + bounded batch에
  비례하고 hierarchy path occurrence 수에는 비례하지 않아야 함
- viewer open 시 text record 수와 무관한 작은 heap만 사용하고 전체
  문자열 page-in을 하지 않아야 함
- 9.8G에서 `viewer-side: N text entries` 단계와 entries 상주가 완전히
  사라져야 함
- text 단계 RSS는 heartbeat로 parsing/build cell plans/encoding과
  분리해 기록

초기 수치 gate는 T0 baseline 후 고정한다. 권장 시작값은 text index
단계의 no-text-build 대비 RSS 증가를 `max(256 MiB, bounded batch의 2배)`
이내로 두는 것이다. source `Doc` 자체의 RSS는 별도로 기록한다.

### 8.4 성능

- viewer open에서 `labels.tsv` 선형 load 시간 0
- 일반 interactive viewport의 `text_plan_ms` P95 20ms 이하를 목표
- text query가 전체 source text 수가 아니라 visited cell/TBVH node/
  visible repetition member 수에 비례
- first partial geometry 응답을 text query가 지연시키지 않음; 20ms
  budget을 넘는 자산은 bounded candidate/continuation 정책으로 전환
- 9.8G cold/warm viewport의 기존 geometry plan/load/draw 성능을 10%
  이상 악화시키지 않음

### 8.5 표준 검증

각 milestone에서 최소 다음을 수행한다.

```text
cd rust && cargo test --workspace
sh tools/validate_rust.sh
python tools/validate_vfs.py <fixture/cache>
```

포맷 변경 milestone은 jobs 1/기본/최대의 OVM/OVT byte 비교와 kill/corrupt
matrix를 추가한다. GUI cutover에는 고정 viewport screenshot 또는 label
tuple dump 비교를 남긴다.

---

## 9. 롤백과 운영

- T1/T2에서는 기존 sidecar를 유지하므로 flag로 즉시 기존 표시 경로로
  돌아갈 수 있다.
- T3 문제 발생 시 viewer flag만 되돌리고 cache v5의 sidecar를 사용한다.
- T4 이후 rollback은 이전 binary와 원본 재인덱싱으로 한다. v5 cache를
  v4 binary로 억지로 열지 않는다.
- production cutover 전 9.8G build 로그, cache 크기, 대표 viewport의
  raw/selected count와 text plan 시간을 보관한다. proprietary text와
  좌표는 반출하지 않고 집계 수치만 사용한다.
- 기존 운영 원칙을 유지한다: viewer가 cache를 사용하는 동안 원본이나
  cache를 수정하지 않고, 재인덱싱은 전체 cache 삭제 후 직렬 실행한다.

---

## 10. 예상 코드 영향

- `rust/ovm/src/lib.rs`: v5 section builder/accessor/open 검증
- `rust/cli/src/vfs.rs`: cell-local text build, OVT writer, publish/heartbeat,
  sidecar 제거
- `rust/vfs/src/hier.rs`: text BVH/repetition query, declutter, block labels,
  지표
- `rust/vfs/src/lib.rs` 및 vfsd command 경로: mmap과 label 응답/lifecycle
- `floe/vfsclient.py`: generation별 label 응답 parsing
- `floe/service.py`: 전역 sidecar 제거, stale-safe label apply
- `floe/viewport.py`: 명시적 text/block kind를 ephemeral cell에 적용
- `tools/validate_vfs.py`: v5/text section 구조 및 count 검증
- `tools/validate_rust.sh`: text fixture/oracle/corrupt/determinism gate
- `rust/VFS_HIER.md`: 구현 완료 시 rev 요약과 잔여 위험 상태 갱신

`rust/vendor/`는 수정하지 않는다. geometry page partition, OASIS parser와
KLayout exact page load는 이 작업에서 동작을 바꾸지 않는다.
