# floe VFS 아키텍처 (V4: 계층 보존 워킹셋)

2026-08-02 결정, rev 19 (rev 2 = 코드 대조 검토, rev 3/5/6/7/8/11
= 외부 검토, rev 4 = 포맷-유지 원칙 폐기, rev 9/10 = **캐시 무결성
단순화·책임 분리 지시**, rev 12 = **M0 실증**, rev 13~18 =
**M1/M2/M3/M3.5 구현 + 외부 검토 2회 반영(0.6.3)**, rev 19 =
**M4 실측 완료 + 스케일 결함 4건 수정(0.6.4→0.7.0, ovm v3)** —
변경 요지 §0). 상태: **M0~M5 + mmap + M7-A/B + 스켈레톤 폐기
(rev 24, 0.10.0)** — **다음 = M7-C 사무실 실측(fit-뷰 콜드 포함)**
(M4 결과: §0 rev 19). 배경: MAIN09(150M급)
실측에서 **뷰어 깊은 줌이
줌인할수록 느려지는** 병리 확인. 원인은 개별 증상이 아니라 하나의
근본 구조 — **워킹셋을 평탄화(flatten)해서 klayout에 넘긴다**는 것.
이 문서는 그 평탄화를 걷어내고, VFS/BVH를 정확히 활용해 klayout에
**계층 그대로** 붙이는 방향을 정의한다. 선행 문서: `VFS.md`(V1~V3),
세션 메모리 [[vfs-rewrite-direction]], [[vfs-build-perf]].

---

## 0. 개정 요지

rev 2 (코드 대조 검토):
- 리스크 2건 해소·삭제: authored+스플라이스 **모달 정합**은 구조적
  보장(§3.2), **cut 에지-단위 갈림**은 존재 불가(셀 단위 속성, §2.4).
- 클립영역 전파: 워크리스트 수렴 → **topo-rank 1패스**(§2.3).
- **klayout read 이름-바인딩**(§3.3)을 M0 스파이크로 승격(M2 게이트).
  WC는 상주가 아니라 **gen-ephemeral**(§3.1), eviction은 페이지 단위.
- pick의 design-이름 의존 대책(§3.4), frames 이행(§3.2), 프로토콜/
  지표 명세(§3.5), valmini 게이트 교정(§7).

rev 3 (외부 검토):
- depth: min-path-depth(의미 변경) 철회 → **WC variant
  `WsKey=(ci, remaining_depth)`**로 현행 의미 정확 유지(§2.5).
- skew Grid **닫힌형 수식 명문화**: 역행렬 4-코너 + det=0 분기 +
  i128, ⊖ 표기를 ⊕(−·)로 교정(§2.3).
- **page BVH를 M1 범위로**(§2.2).
- topo 전제 교정: back-edge "skip"은 보장이 아님 — **빌드에서 사이클
  명시 검출·거부**를 전제로 추가(§2.3).
- WC 이름 숫자화 `W<gen>_<r>_<ci>`(공백/유니코드 배제), 이전 gen 일괄
  `delete_cells`(§3.1). pick은 P-이름의 ci + **ci→design명 테이블
  1회 전달**(§3.4). M0 폴백의 전송 포맷(hier.tsv) 명세(§3.3).
- 검증 게이트 7건 추가(§7).

rev 4 (지시: **포맷-유지 원칙 없음, 최대/최선의 결과 목표**):
- rev 3에서 포맷 안정성 때문에 타협했던 3건을 **빌드-시 `.ovm v2`**로
  승격(§3.6): page BVH는 runtime lazy → **packed 섹션**(§2.2), topo
  rank는 기동 시 계산 → **셀 레코드 필드**(§2.3), 그리고
  [[vfs-build-perf]]의 이연 지뢰 **`seq: u16` → u32**(65535 페이지
  panic)를 같은 범프에 흡수.
- 확인된 사실: `.ovm`엔 이미 MAGIC+VERSION 게이트가 있고(에러 명확),
  **`height`는 v1부터 셀 레코드에 존재** → depth sentinel 접기는 셀
  단위로 즉시 가능(§2.5). **python은 .ovm을 파싱하지 않아**(경로
  참조뿐) 범프 영향 범위는 rust `floe-ovm` 크레이트 하나. `.ovp`/
  `.ovc`는 불변.
- localview **K-box를 M1 기본 탑재**로 격상(계측-후-적용 → 기본 K=4,
  A/B에서 K=1 대조)(§2.3, §6-2).

rev 5 (외부 검토 2차 — 구현 명세 확정):
- **Rep::Pts 경로 명세**(§2.3): 임계 이하 열거 / 초과 시 보수적
  전체-extent 폴백, 방출은 항상 rep 1개. hier.tsv 폴백에 rep 열 +
  pts pool 추가(§3.3).
- **세션 트랜잭션 신설**(§3.7): 응답-시점 resident 등록 + 클라이언트
  stale drop 조합의 **영구 공백 버그**(현행 flat 경로에도 잠복)를
  ack-gen 2단계 커밋으로 해소.
- **page BVH 강화**(§2.2): 레이어는 리프 필터 → **(cell,layer)별
  루트**(page-range 테이블), 노드에 subtree max_w/max_h 집계(리프
  이전 cut 컬링), 리프 = 페이지 디렉터리 연속 구간(빌드 시 run 내
  재배열). 계측 분리.
- **`.ovm` v2 wire 스키마 동결**(§3.6): 헤더 216B/9섹션, cell 128B·
  page 88B(오프셋표), prange 16B·pbvh 48B 신설, height **u32** +
  checked 깊이 오버플로 에러. 사실 정정: `tools/validate_vfs.py`가
  .ovm을 직접 파싱 → v2 갱신 범위에 포함(런타임 python은 비파싱).
- K-box는 **box별 질의 후 dedup**(중간 단일-bbox 병합 금지), 병합
  함수·tie-break 결정성 고정(§2.3). i128은 checked + 보수 폴백.
- M5 "버전 범프"는 CLI/크레이트 릴리스 범프로 명확화(캐시 포맷은
  M1에서 v2)(§5).

rev 6 (외부 검토 3차 — Pts 총비용 / cut 포화 / 장애 복구):
- **Pts 재설계**(§2.3): "폴백 = 비용 0"이 현 구조에선 거짓
  (place()가 pool을 매번 Vec 복사, rep_extent 전 점 순회; 실측
  testchip_1g5 = Pts 410개·오프셋 147만·최대 3,823 → 임계 4096이면
  전부 열거 경로). v2 pts pool에 **extent+chunk 인덱스**를 굽고
  zero-copy 접근자 추가, 방출은 **가시-부분집합 Pts**(klayout의
  type-10 전개 리스크를 임계로 바운드), 요청당 PTS_ENUM_BUDGET.
  M0에 **10만/100만-점 Pts 재료화 실증** 추가.
- **max_w/max_h u32 → u64**(§3.6): 빌더가 u32::MAX로 포화시키고
  (vfs.rs:1015) planner가 일반값으로 비교 → 극단 dbu/거대 지오메트리
  에서 오컷 가능. 페이지 96B/pbvh 56B로 재배치.
- **ACK 상태기계 완성 + 장애 복구**(§3.7): 초기값/중복/역행 규칙,
  resident_mb 의미, apply 부분 실패 시 **모자이크 전체 재생성 +
  `reset=`**(부분 적용 레이아웃으로 다음 gen 진행 금지),
  fault-injection 게이트.
- **빌드 atomic publish**(§3.6): 현행은 design.ovp를 제자리
  truncate 후 기록(vfs.rs:995) — 중단 시 구 ovm + 파괴된 ovp.
  tmp 파일 + fsync + rename(ovm이 커밋 마커) + 헤더에 ovp_len
  기록·오픈 시 정합 검증.

rev 7 (외부 검토 4차 — Pts rebase / 진짜 atomic publish):
- **Pts 부분집합 rebase 필수**(§2.3): `Rep::Pts` 불변조건 = 첫
  오프셋 (0,0)(doc.rs:28), writer는 첫 점을 기록하지 않음
  (write.rs:98) — 부분집합을 그대로 방출하면 **오배치**. 0/1/≥2
  케이스 규칙 + 원본-index dedup(좌표 dedup 금지 — 중복 좌표
  멤버는 별개) 명세. 선례: tiler hier.rs:869.
- **O_vis 사다리**(§2.3): "기여는 항상 extent" 철회 — 대형 Pts
  extent 일률 사용은 옛 `view=cwb` 병리의 재현. extent는 1차 교차
  판정 전용, 기여 bbox는 선택 결과 기준.
- **chunk는 Morton 재배열**(§2.3): 원본 순서 256개 묶음은 공간이
  섞인 파일에서 모든 chunk bbox ≈ extent가 되는 함정.
- **versioned OVP publish**(§3.6): rev 6 방식(ovp 먼저 rename)은
  "감지"일 뿐 — rename 사이 중단 시 구 캐시까지 파괴되고, ovp_len
  동일 크기 오조합은 통과. v2는 `design-<build_id>.ovp` + ovm에
  build_id 기록, **ovm rename이 유일한 커밋** → 언제 죽어도 구 또는
  신 캐시 하나는 항상 유효. 커밋 후 구 ovp GC + 디렉터리 fsync.
- **projected 예산**(§3.7): eviction 판단은 committed가 아니라
  committed+이번 플랜 new 기준(v1 동작 계승 명시), LRU touch도
  pending(롤백 가능). reset 후 실패 gen 재사용 금지.
- 오픈 검증을 섹션 유형별로 정정(고정 stride vs 가변), 페이지별
  checked `file_off+csize ≤ ovp_len`, zero-copy = borrowed bytes의
  LE 이터레이터(캐스트 금지)(§3.6). M0 Pts 케이스에 경계값
  1/2/1024/1025 + 좁은-뷰 부분집합 추가(§5).

rev 8 (외부 검토 5차 — Morton/GC/검증 범위 정합):
- **멤버 identity = Morton pool slot**(§2.3): rev 7의 "원본 index
  dedup"은 Morton 재배열 후 wire에 index가 없어 구현 불가 — slot
  index가 identity(동일 좌표 별개 slot 유지, 방출 순서 = pool
  순서). 정렬 tie-break의 "원본 index"는 빌드-시 규칙으로만 존속.
- **GC와 실행-중 viewer**(§3.6): `Vfs::delta`가 요청마다 ovp를
  reopen(lib.rs:394) → 커밋 직후 구 ovp GC가 구 viewer를 깨뜨림.
  `Vfs::open`이 핸들을 1회 열어 보존 + `read_at`, unlink-후에도
  열린 핸들 유효, GC 실패는 다음 빌드로 이월. 디렉터리 fsync는
  **2회**(ovp 이름 내구화 → 커밋 내구화).
- **M0 교정**: 1-점 type-10은 스펙상 불가(writer가 `len−2` 기록,
  write.rs:98) — 입력 2/1024/1025/10만/100만 × **선택 결과
  0/1/≥2** 매트릭스로 재구성. 소형 Pts도 O_vis는 정확 스캔(extent
  금지 — 두 점짜리도 extent면 부풀림)(§2.3).
- **build_id 검증 범위 정정**(§3.6/§7): ID는 파일명에만 있으므로
  "수동 내용 교체 검출" 게이트 삭제(정상 퍼블리시의 세대 혼합
  방지가 목적). build_id는 `create_new`로 충돌 검출·재생성.
- **영향 범위 추가**(§3.6): ovp **파일명** 변경은 python에도 미침
  — floe/cli.py:137(info), tools/validate_vfs.py:75. ".ovp 불변"
  → "payload 바이트 포맷 불변, 파일명/발견 경로 변경"으로 교정.
- **바이트 동일성 정책**(§7): build_id 때문에 독립 빌드 간 ovm
  바이트가 달라짐 — ovp는 그대로, ovm은 build_id를 0으로 정규화
  후 비교, Morton/page/pbvh 순서의 jobs-불변 게이트.

rev 9 (지시: **캐시 무결성 단순화 — 운영 원칙으로 대체**):
- 운영 원칙 3조 채택(§3.6): ① 원본 변경 = 캐시 삭제 후 재빌드,
  ② 뷰어가 보는 동안 원본 수정/삭제 금지, ③ 그래도 어긋나면
  운영으로 해결(삭제·재빌드).
- 이에 따라 rev 7/8의 **versioned OVP·build_id·GC·이중 디렉터리
  fsync·오픈 핸들 보존 전부 철회** — 그 장치들은 "사용 중 재빌드"
  를 코드로 감당하려던 것인데 ②가 시나리오 자체를 금지한다.
- 남기는 최소 장치(§3.6): **ovm-마지막 커밋**(빌드 시작 시 ovm
  삭제 → ovp 기록 → ovm 기록; 중단 = "no cache" 명확 에러, 조용한
  혼합 없음) + 오픈 구조 검증 + `ovp_len` 단일 정합 체크. 파일명은
  `design.ovp` 유지(파일명 변경에 따른 cli.py/validate_vfs.py 갱신
  불필요). ovm이 다시 결정적이 되어 **바이트 동일성 게이트도 정규화
  없이 원복**(§7).

rev 10 (지시: **인덱서/뷰어 책임 분리** + 이전 검토 잔여 1건):
- 인덱싱은 **제한된 경로(시스템)를 통해서만** 실행 — 시작 시 캐시
  전체 삭제(최소한 마커), 종료 시 **마커 생성**(= design.ovm).
  **인덱서는 동시 빌드 등 경합에 책임지지 않는다**(직렬화는 호출
  경로의 책임)(§3.6).
- 뷰어는 **인덱싱하지 않는다**(0.4.6부터 이미 그렇다) — 원본과
  캐시가 정상 생성되었는지 **확인만** 한다(마커 존재 + 오픈 구조
  검증 + ovp_len)(§3.6).
- **checked 변환 규칙**(§3.6, 외부 검토 잔여): 빌드의 모든
  usize/u64 → u32/u16 narrowing(na/nb, Pts count, page/place/BVH
  start·count, payload size 등)은 checked — silent truncation
  금지, 초과는 "limit exceeded: <필드>" 빌드 에러.

rev 28 (T1~T4 cell-local Text VFS — 2026-08-04, 0.11.0, **ovm v5 =
재인덱싱 필요**; 계획 = rust/VFS_TEXT_PLAN.md, 4 커밋):

- **T1 (포맷/빌드)**: ovm v5 — 신규 5섹션
  TEXTS(80B)/TRANGES(16B)/TBVH(40B)/TREPS(64B)/TCHUNKS(32B), 셀
  레코드 144B(trange 범위 + **재귀 텍스트-레이어 마스크
  tmask_rec** — 텍스트 없는 서브트리 가지치기용), 헤더 @80 =
  ovt_len. **design.ovt** 신규: 문자열 원문 + Morton 정렬 텍스트
  Pts 풀(청크 bbox는 TCHUNKS), 빌드 중 BufWriter 스트리밍.
  텍스트 인덱싱은 **셀-로컬만 읽음**(계층 미순회) — 비용/메모리가
  소스 레코드 수에 비례, 경로 전개 없음. run 내 순서 = bbox 중심
  Morton 프리소트(+seq 타이) → BVH 중앙 분할 저장 순서(결정적,
  jobs 무관 바이트 동일). 오픈 검증 2단: shallow = 테이블
  구조(run/root/rep 서술자/ovt 스팬/청크 수), deep(from_bytes) =
  레코드 스윕 + **정확-1회 소유권**(텍스트·trange claim) + TBVH
  공유/사이클 DFS — places/inst-BVH와 동일한 mmap-지연 절충(§3.6
  기존 원칙). Vfs::open은 ovp_len처럼 **ovt_len 쌍 검증** + ovt
  mmap. 마커: design.ovt 스크럽 목록 추가, FLOE_KILL_AT=
  ovt-written 킬포인트(마커 매트릭스 4점).
- **T2 (라벨 플래너)**: rust/vfs/text.rs — 기하 플래너와 동일
  규칙(vis 레이어·semantic depth·셀 컷)의 결정적 워크가 전체
  변환을 추적, 후보 = 뷰포트 안 앵커의 (경로, 멤버) 정확 집합
  (declutter 전, 오라클 게이트 대상). Grid 닫힌형 인덱스 사각 +
  Pts Morton 청크 사다리 재사용. 멤버/후보 예산은 truncated
  플래그(표시 전용 열화, 조용한 생략 아님). 최초 declutter =
  월드-고정 cell_px(48px) bin당 1승자, 우선순위 blk > txt >
  레이어 순 > 안정 해시, 예산 400(블록명은 rev 29에서 bin 제외,
  총 예산 4096으로 교정). **블록명은 런타임 합성**
  (r==0 depth 경계의 자식 프레임에서 셀명+배치 bbox 중심;
  초기 96px gate는 rev 29에서 실제 원문/`...` fit 판정으로 교체)
  — BLOCK_ROW/센티널/저장 라벨 전면 폐기.
  probe(px=0)는 구조적으로 라벨 없음.
- **T3 (프로토콜/뷰어)**: vfsd 응답 `labels=<gen별 tsv> nlabels=N
  text_plan_ms=F` — kind 명시 행(`txt\t<l>/<d>\t…` /
  `blk\t-\t…`), 뷰-generation 자산(클라이언트가 다음 요청에서
  파일 삭제; stale이면 미적용 폐기). 리파인 라운드는
  `nolabels=1`로 재계산 생략, 뷰어는 라운드1 파싱 행을 재적용.
  `FLOE_LABELS=0` = 데몬 킬스위치. **FLOE_LOD=0이 라벨까지 끄지
  않도록 label_px 분리**. blk 행은 뷰어 frame_layer로 매핑.
- **T4 (컷오버)**: emit_viewer_side에서 collect_all_texts 호출·
  labels.tsv·texts.tsv 생성 제거(스크럽 목록에는 유지 — 구 캐시
  청소), skel::label_rows/LabelRow/for_each_offset_capped 삭제
  (.ice 경로의 collect_all_texts/build_skeleton은 동결 유지),
  뷰어 _load_sidecar_labels/_view_labels/_live_labels 삭제. meta
  = "texts" 집계(records/members/cells/reps/ovt_bytes)로 교체.
  **§6 잔여 위험 0(경로 전개 상주) 해소** — 그 코드 경로 자체가
  VFS 빌드에서 사라짐.
- 게이트: ovm 텍스트 라운드트립+corrupt 6종, 플래너 유닛 4종
  (브루트포스 오라클 XOR: 회전/미러/배열/Pts 중복/depth/vis/cut,
  declutter 결정성/부분집합/예산, 예산 고갈 truncated), 신규
  tools/validate_vfs_text.py **X1~X6**(klayout 오라클 XOR
  full/depth0/vis 슬라이스, declutter, ovt 절단 corrupt, jobs
  바이트 동일(ovm+ovt), 데몬 라벨 수명), validate_vfs.py = v5
  구조 + 사이드카 부재 + meta 텍스트 합계 klayout 대조, 마커
  4점, 전체 스위트 + 서비스 스모크 green. 계획 대비 명시 편차:
  (a) 오픈 딥 검증은 from_bytes 전용(mmap 지연 원칙 유지), (b)
  publish는 기존 마커 규약 유지(.tmp 체인 미도입 — 동일 보장),
  (c) 라벨 행은 layer_idx 대신 l/d 표기(자기서술적).

rev 29 (Calibre식 hierarchy frontier — 2026-08-04):

- **표시 계약**: 유한 depth `d`는 실제 설계 레이어와 별개인 구조
  frontier 하나만 표시한다. 경로 깊이 `d`에 있는 각 셀의 직계 자식
  bbox와 읽을 수 있는 셀명이 대상이며, depth 0/1/2/... 결과는
  누적하지 않는다. 요청 깊이가 top height 이상으로 접히거나 full이면
  frontier가 없으므로 FRAME_LAYER와 블록명도 없다.
- frontier는 visible layer, geometry cut, merged LOD 선택과 무관하다.
  따라서 모든 설계 레이어를 꺼도 보이며 `layers=none`은 `all`과
  구별한다. 반대로 실제 페이지/텍스트는 계속 visible layer를
  정확히 따른다. size-cut된 셀의 all-layer rbbox를 대체 형상으로
  그리는 동작은 금지 상태를 유지한다.
- **합성/순서**: 반복 frontier는 화면상 2px 미만 피치 또는 기존
  재료화 상한에서만 footprint로 융합한다(cut 값과 무관). FRAME_LAYER
  는 KLayout 레이어 순서에서 먼저 등록해 모든 실제 설계 레이어의
  아래(underlay)에 그린다. pick/snap 대상에서는 계속 제외한다. 블록명은
  bbox 중앙 정렬, 세로 bbox는 90도 회전한다. 고정 화면 폰트의 가용
  폭을 넘으면 원문 일부를 남기지 않고 ASCII `...`만 표시하며,
  `...`도 못 들어가면 생략한다. 블록명은 자체 frontier bbox가 있으므로
  48px generic text bin 경쟁에서 제외한다. 총 표시 예산은 4096이며,
  초과 시 조용히 버리지 않고 `labels partial`로 알린다. KLayout
  headless 렌더에서는 1픽셀 anchor로 대체되지 않도록 label 표시 중
  `text-lazy-rendering=false`, `text-point-mode=false`를 전역 기본으로
  사용한다.
- **KLayout 고유 표시 제약**: `text-lazy-rendering=false`로 실제
  글리프를 그려도 `db.Text`는 정렬 기준점(origin)에 별도 1픽셀 anchor를
  함께 그린다. `...`에서는 가운데 점 위쪽, 일반 문자열에서는 중심
  부근의 1픽셀 돌기로 보일 수 있다. `markers-visible` 및
  `text-point-mode` 설정으로 제거되지 않는다. 이는 현재 KLayout text
  renderer의 고유 동작으로 기록하고 우회 raster 합성은 도입하지
  않는다.
- **운영 기본값/독립 토글**: viewer 시작 cut은 L2, merged LOD는 off다.
  LOD 토글은 page variant 선택만 바꾸며 `cut_px`와 coverage를 변경하지
  않는다. 따라서 `lod:off · cut:L2`가 정상 기본 상태다.

rev 27 (외부 검토 3차 반영 — 2026-08-04, 0.10.3; 2건):

- **(높음) 사이드카 메모리 — 부분 수정 + 잔여 위험 등재**: 전체
  TSV String 빌드를 **BufWriter 행 스트리밍**으로 교체(피크에서
  텍스트 전체 사본 1개 제거). 그러나 **collect_all_texts의 경로
  전개 상주(entries)는 미해결 잔여 위험** — §6에 등재, 판정
  기준 = 9.8G 재인덱싱의 단계별 계측(entries 수·rss). 초과 시
  후속 = 외부 정렬 스풀 + 스트리밍 수집.
- **(낮음) frame layer u32::MAX 경계 + 센티널 모호성**: ①
  frame_layer를 "max+1 포화, 충돌 시 미사용 번호로 하향" 규칙으로
  — 임의 레이어 집합에서 충돌 불가, rust/python 동일 구현(경계
  유닛 추가). ② labels.tsv 블록 행은 pseudo-layer(u32::MAX) 대신
  **명시적 row kind**(LabelRow.block → "blk")로 — 어떤 실제
  레이어 번호와도 혼동 불가.

rev 26 (외부 검토 2차 반영 — 2026-08-04, 0.10.2; 7건 확인·수정):

- **(높음) 라벨 rep 전개**: label_rows(와 take_members 경유 포함)가
  cap 검사 전에 rep_offsets로 na×nb 전체 Vec을 물질화 — 대형
  블록 배열에서 GB 할당. 수정 = **지연 캡 열거자**
  (for_each_offset_capped; 물질화 0).
- **(높음) 뷰어측 텍스트 RSS 무한**: collect_all_texts는 경로
  전개형이라 재사용 심한 칩에서 소스보다 커질 수 있음. 수정 =
  **단계별 계측**(entries 수/labels 행/사이드카 문자열 MB + rss를
  빌드 로그로) — 9.8G 수치가 커지면 후속 = 외부 정렬 스풀 +
  스트리밍 TSV(백로그로 명시).
- **(중간) 소형 DBU 페이지의 1px 충실도**: 방출 rect 최소 폭 1
  DBU라 px_per_dbu>1이면 128px 조건으로도 블록이 보임. 수정 =
  게이트가 **실제 방출 셀 폭 max(1, ceil(extent/G))×px ≤ 1px**을
  판정(양 축). tiny-DBU 유닛 추가.
- **(중간) 255/0 실설계 충돌**: 프레임 레이어 = **런타임 산출:
  최대 설계 레이어 + 1, dt 0** (데몬 frame_layer(ovm) ↔ 뷰어
  frame_layer(meta) 동일 규칙 — 캐시 저장 불필요, 델타 프레임은
  런타임 생성물). labels.tsv 블록 행은 레이어 번호 대신 **"blk"
  센티널**로 기록(뷰어가 런타임 프레임 레이어에 매핑; 구 캐시의
  255/0 행은 일반 행으로 강등 — 재인덱싱 시 해소). pick/snap
  제외·hollow·색상 전부 동적 값 사용.
- **(중간) prange 교차-소유권 검증**: 셀별 prange 런의 모든
  페이지가 (해당 cell, 해당 layer, LOD_EXACT)인지 open에서 검사 —
  손상 런이 다른 셀/LOD 페이지를 가리키면 조용히 오표시되던 구멍.
  부정 픽스처(런이 MERGED를 삼킴) 추가.
- **(낮음) \r unescape 누락** 수정, **(낮음) depth 기본값 문서**
  (cli --depth 도움말, README) 0으로 갱신.

rev 25 (M7/스켈레톤 외부 검토 반영 — 2026-08-04, 0.10.1; 7건
전부 실결함/공백으로 확인·수정):

- **(높음) 밀도 게이트 역전**: 전체-페이지 members를 부분-교차
  px²와 비교해 **줌인할수록 LOD가 발화**(격자 셀이 ~780px 블록).
  수정 = 전체-페이지 대 전체-페이지 + **충실도 전제조건: LOD 셀
  ≤ 1화소(양 축) ⟺ 페이지 ≤ 128px** — 병합 셀이 가시 블록이 될
  수 없음이 계약. 딥줌 코너-슬라이스 회귀 유닛.
- **(높음) Grid 전수 순회**: 10^6×10^6 grid 레코드는 바이트-소형
  (한 페이지)인데 LOD 생성이 10^12회 루프. 수정 = **직교 grid는
  셀별 존재판정으로 해석적 마킹 O(격자)**(임의 개수), skew는
  멤버 6.5만 초과 시 **verbatim 통과**(통계 계수 — 무음 캡 금지).
  게이트: 해석 경로 vs 브루트 래스터 상위집합, skew verbatim.
- **(높음) depth-0 빈 화면**: 순수 계층 top은 기본 오픈이 검은
  화면. 수정 = **r==0에서 모든 직계 자식을 아웃라인 프레임으로**
  (기존 컷-프레임 융합·캡 기제 재사용; §2.5 "no frames" 문구
  폐기). 순수-계층 픽스처 유닛.
- **(중간) poly/path bbox 스미어**: bbox 마킹은 **bbox가 1셀에
  들어갈 때만**(실윤곽 대비 ≤1셀 계약 유지), 그보다 크면
  verbatim. rect는 bbox=실기하라 종전 규칙(양축 ≥4셀만 verbatim).
- **(중간) lod 링크 검증 강화**: lod 바이트·codec 화이트리스트,
  타깃=정확히 MERGED+동일 seq, **이중 클레임 비트셋**. 부정
  픽스처 4종.
- **(낮음) 구 skeleton.oas 잔존**: 재빌드 삭제 목록에 추가(구
  캐시 위 재인덱싱 시 용량 회수).
- **(낮음) labels.tsv escape 미복원**: 뷰어 리더에 unesc 추가.

rev 24 (스켈레톤 폐기 + 오픈 depth 0 — 2026-08-04, 0.10.0):

- **VFS 캐시에서 skeleton.oas 폐기**: 광역 뷰는 워킹셋 + coverage
  + LOD 변종이 전담(뷰어 skel 모드·skel 렌더러·`_skel_text_fits`
  제거, gui는 항상 live). 남는 역할은 **라벨뿐** — 빌드가
  `labels.tsv`(display-ready 행: 큰 1단계 셀 블록명 255/0 +
  예산 라벨은 설계 레이어 그대로)를 방출하고 뷰어는 이를 로드해
  기존 `_view_labels` declutter로 그림. detail-twin dt 매핑 제거.
  meta `skeleton` 키 → `labels` 키. 마커 정리 목록에 labels.tsv.
- **`floe_tiler::skel`은 축소 존치**: `label_rows()`(지오메트리
  없는 블록명+선정) 추가; build_skeleton 본체는 레거시 .ice
  경로(main.rs `index`)가 계속 사용 — 그 게이트
  (validate_rust_skel)는 무변경. validate_vfs.py에 labels 게이트
  추가(메타 계수 일치 + 예산 행 ⊆ 사이드카 문자열; valmini 802행
  = .ice 스켈레톤 게이트의 802 튜플과 동수 = 선정 로직 이관 검증).
- 구 캐시(스켈레톤 있음)는 그대로 열리며 라벨만 비어 있음 —
  재인덱싱 시 labels.tsv 생성. 인덱싱 시간: 텍스트 수집 1패스는
  동일, 스켈레톤 지오메트리 빌드+인코딩이 사라져 emit 단계가
  단축(testchip 0.3s; 9.8G에서 수 초~수십 초 절감 예상).
- **뷰어 오픈 디폴트 depth = 0**(top 지오메트리 + 아웃라인 +
  coverage): 어떤 칩에서도 가장 빠른 정직한 첫 화면. --goto는
  종전대로 full.
- 리스크(명시): 광역 성능 폴백이 사라졌으므로 9.8G fit-뷰 콜드가
  live+LOD로 수 초 내인지 M7-C에서 확인 — 느리면 lod_k/트리거를
  광역 쪽으로 조정.

rev 23 (M7-B 플래너 밀도 선택 — 2026-08-03, 0.9.1):

- **밀도 게이트**: `ViewReq.px_per_dbu` 추가(px= 와이어 플럼빙).
  페이지 선택 후처리에서 `members > lod_k × 화면 px²`(가시 교차
  면적 합 기준 — 겹침은 면적을 과대평가해 **exact 쪽으로 편향**)
  이면 LOD 변종 인덱스로 스왑. lod_k 기본 4.0(HierOpts),
  히스테리시스는 M7-C 실측에서 플래핑 관찰 시 추가.
- **계측 = exact by construction**: probe 요청(pick/snap/clip)은
  데몬에서 px_per_dbu=0으로 강제 — 밀도 게이트 자체가 실행되지
  않음. `FLOE_LOD=0` = 전역 킬스위치(데몬 env), plan CLI는
  `--lod 0`. 렌더 XOR 게이트는 probe 경유라 exact 그대로.
- **이름 계약 수정**: `Vfs::page_name`이 LOD 페이지에 "…q" 명을
  반환 — WC 참조·evict 명이 payload 셀명과 일치해야 하며,
  불일치 시 klayout이 빈 exact-명 유령 셀을 만들고 다음 gen의
  exact 로드가 "$1" 리네임으로 밀려 레이어가 통째로 소실되는
  것을 L7 개발 중 실증(수정 완료).
- **전달 경로**: 응답 `lod=N` → 서비스 → 상태줄 ", lod N"(현재
  프레임이 표시 근사임을 상시 노출).
- **게이트**: 플래너 유닛(줌아웃=스왑/줌인=exact/probe=exact/
  킬스위치), **L7 신설** = 변종 사이클(광역 lod 발화 → 동일 뷰
  fine-px에서 exact 복귀 + XOR 0 + 원장 무결) — L1~L6은
  FLOE_LOD=0으로 실행(수명주기 계약은 변종 선택과 직교), S5에
  밀도 발화 + `--lod 0` 무발화 게이트 추가. 스위트 전체 green +
  testchip 서비스 스모크.

rev 22 (M7-A LOD 페이지 빌드 — 2026-08-03, 0.9.0, **ovm v4 =
재인덱싱 필요**):

- **밀도 기반 LOD 변종 생성**: members ≥ 4096인 페이지에
  coverage-grid 박스 융합 변종(LOD_MERGED)을 빌드 시 1회 생성.
  방식 = 페이지 bbox 위 128² 격자에 소형 member 풋프린트를 워드-OR
  마킹 → 행 RLE + 수직 결합으로 사각형 융합. **양 축 ≥4셀 레코드는
  verbatim 통과**(개별 가시 블롭은 exact 윤곽 유지, 서브셀 먼지만
  융합). 보수적 계약: 커버리지 ⊇ exact, 과잉 ≤ 1셀 — 유닛으로
  강제(상위집합 + 1셀 팽창 상한).
- **포맷 v4**: 페이지 lod 바이트 활성(LOD_MERGED=1) + @68 패드가
  exact→LOD 링크(lod_page, LOD_PAGE_NONE 센티널)로. 링크 검증
  (경계·타깃 lod·동일 cell/layer·체인 금지)은 open(shallow)에
  포함. LOD 셀명 = exact명 + "q" 접미(P 접두 유지 — 뷰어의 수집/
  축출/이름해석 경로 무변경으로 통과).
- **플래너는 아직 exact 전용**(M7-B에서 밀도 선택): pranges가
  exact 런만 커버하므로 구조적으로 배제됨 — S5 게이트가 "플랜
  페이지수 == exact 페이지수"로 고정.
- **파생 데이터 회계**: 레이어 테이블·G5 보존 합산은 exact만
  (검증기 lod-skip); LOD 페이지 자체는 G5a klayout 재계수를 동일
  통과(payload 파싱·자기 계수 일치).
- 실측(testchip 1.5G): lod 5,184개, **ovp +1.1%(+18MB), ovm
  +0.5MB**, 빌드 52.8s(0.8.2 파이프라인 위, LOD 비용 노이즈 수준).
  repfloor 합성: 5 exact 전부 변종 생성, 스위트 전체 green.
- 주의: 0.8.2 리팩터 후 rep-split fragments 카운터가 크게 찍힘
  (testchip 1.63억) — 파티션·바이트·게이트는 정상이라 통계 계수
  방식 문제로 보이며 encode-batch 검토(M7 후) 항목에 포함.

rev 21 (ovm mmap 오픈 — 2026-08-03, 0.8.1; M4 후속 1순위):

- **`Ovm::open` = read-only mmap** (memmap2+libc 벤더 추가, 닫힌망
  빌드 경로 유지): 9.8G 자산의 ovm 25G를 데몬 기동 시 전체 read
  하던 비용이 사라지고, 터치한 페이지만 상주(OS 페이지캐시 공유).
  실측: testchip 점 플랜 최대 RSS 2.5MB (ovm 24.5MB).
- **검증 2단화**: open(mmap) = shallow — 헤더·버전·섹션 경계와
  소형 테이블(layers/cells/pages/pranges/pbvh)의 "corrupt cache;
  rebuild" 검증 전부 유지. **places·pts pool·inst-BVH 전수
  루프는 open에서 제외**(남기면 다중 GB 섹션이 통째로 폴트-인 —
  mmap 무의미). `from_bytes`(Vec 경로) = deep 유지: 유닛·게이트
  픽스처는 종전 강도 그대로. 근거 = rev 9 운영 원칙(마커
  프로토콜이 빌드 완결성 보증; 그 너머의 심층 손상 = 빌더 버그 =
  빌드 게이트 소관). 트레이드: 심층 손상 데이터를 읽으면 데몬이
  clean error 대신 접근자 assert panic.
- **매핑 수명 안전성**: 재빌드는 마커 규약상 삭제→재생성(새
  inode)이라 라이브 매핑은 구 inode에 유효; in-place truncate
  경로 없음 → SIGBUS 시나리오 없음.
- 캐시 포맷 불변(ovm v3, 재인덱싱 불필요). 스위트 전체 green
  (모든 게이트가 mmap 오픈 경유).

rev 20 (M5 통합·폐기 완료 — 2026-08-03, 0.8.0):

- **flat 경로 전면 제거** (캐시 포맷 불변 — ovm v3 유지, 재인덱싱
  불필요; 뷰어·데몬 동시 배포라 프로토콜 단절은 원자적):
  - rust/vfs: flat 플래너(`plan`/`Plan`/`Mat`/`PlanStats`)·flat
    `Session`/`Update`·`Vfs::delta`(flat 전용 스플라이스)·
    `fold_array`/`rep_footprint`/`IRange`/`axis_range` 제거.
    hier가 쓰는 `ViewReq`/`layer_mask`/`read_page_payloads`/
    `delta_hier`는 유지.
  - vfsd: `mode=` 잠금(`mode_lock`)·flat 세션 제거 — 모든 요청이
    V4(hier)로 서빙되고, `mode=`는 세션리스 probe 표시로만 남음
    (`probe`/`hier_probe` 동의어). `mats_<gen>.tsv`·
    `frames_<gen>.oas` 부수 파일 생성 제거(컷 프레임은 M1부터
    델타 내장 FRAME_LAYER 사각형).
  - `plan` CLI: hier 출력 단일화(`--mode`는 스크립트 호환으로
    수용·무시).
  - 뷰어: `FLOE_VFS_MODE` 제거, `VfsMosaic.apply`(flat)·
    `frame_ci`·mats/frames 응답 파싱 제거, `vfsclient.request`의
    `hier=` 파라미터 제거(항상 V4).
  - 게이트: render 검증 hier 단일화(flat 패스 라인 삭제),
    lifecycle의 `hier=True` 인자 정리. §4 재확인 = coverage/skel/
    render 게이트 green으로 갈음.
- 스위트 green(31 rust 유닛 + G5/G6 + render 6뷰 + H1-H5 +
  L1-L6 + marker + S1-S4) + testchip 서비스 스모크(렌더 프레임 +
  pick 프로브, hier names 해석 확인).

rev 19 (M4 실측 완료 — 2026-08-03, 9.8G b3/사무실 호스트,
0.6.4→**0.7.0 (ovm v3)**):

- **실측이 드러낸 스케일 결함 4건, 전부 당일 수정**(완료 마일스톤
  코드의 결함이지 계획된 공백이 아님 — ①류):
  1. 0.6.4: emit_viewer_side가 build의 rbbox/lmems 패스를 중복
     재실행(9.8G 스켈레톤 단계 수 분 침묵) → 재사용 + 단계별
     하트비트. meta.json 바이트 동일 검증.
  2. 0.6.5: BVH 워크가 가시 인스턴스 전수를 후보 셋에 수집한 후
     cut 분류(184M-place 칩 넓은 뷰 첫 요청 무한) → 인라인 분류
     + 무할당 `cell_rbbox`/`cell_lmask_rec`.
  3. 0.6.6: 소형(≤8192) Pts가 요청당 예산(200k) 고갈 시
     **O_vis=extent 폴백 → 자식 localview가 셀 전체로 연쇄**
     (§2.3이 금지한 view=cwb 병리의 Pts 재림; depth 9 / 0.1µm
     뷰에서 29,180페이지/2.3G members/188s). 소형은 예산 무관
     정확 스캔(폴백 삭제) + 회귀 유닛.
  4. **0.7.0 (ovm v3)**: **rep 페이지-bbox 바닥** — 레코드 bbox가
     반복 전개 전체라 BSP 평면으로 좁혀지지 않고, 그런 레코드를
     담은 모든 페이지 bbox가 다이 폭 → 플래너가 (정직하게) 어떤
     뷰에도 전부 선택. 9.8G 점 뷰 depth 0 = 2,151페이지/97s
     (5µm 뷰와 동수 = 뷰 무관 상수 바닥). 합성 재현(klayout 폴딩
     Pts 홍수) 후 수정: **분할 평면에서 rep 조각화**(Pts=arena
     in-place 파티션+rebase(첫 offset (0,0), 1-member→One),
     Grid=인덱스 사각 분할(skew 안전, na≥2 정규화), 경계 소유 =
     center×2 < plane2 → left / ≥ → right 명시), **바이트 플로어
     없음**(2-member 산탄 레코드가 사분면 페이지를 도로 오염 —
     재현으로 확인), **wide One(다이 링·스파인)은 재귀 레벨별
     oversize 페이지 격리**, **페이지 게이트 = 바이트**(레코드
     수 아님 — 단일 초대형 rep도 단독 분할; testchip 2,282→5,680
     페이지로 1MB 타깃 준수 복원, ovp +11% = 소형 페이지 deflate
     손실), 레이어 테이블 = **저장** 레코드(G5c는 소스 scan 대비
     ≥ 하한, members 등식은 불변 앵커), `plan --inspect` 현장
     프로브(wide 페이지·rep 분해·visible-member 비율). 게이트:
     cli 유닛 7종(중복 offset 개수 보존·음수 좌표·축/음수/skew
     Grid·단일 거대 Pts·oversize 비오염·경계 소유) +
     `validate_vfs_split.py` S1~S4(플로어 붕괴·보존·klayout
     페이지 재계수·jobs 결정성) 스위트 편입.
  - 운영 대응(0.6.4~0.6.6 사이): 클라이언트 스트리밍 라운드 캡
    8(데몬 stateless라 잔여/N 바닥은 기하 꼬리 — 유닛으로 확인 후
    회귀), `FLOE_STREAM_KB`(0=off/고정), `FLOE_STREAM_TARGET_MS`,
    FLOE_DEBUG 라운드 로그.
- **M4 결과표** (9.8G b3, cold, 점진 로딩 on; v2 = 결함 수정 전
  0.6.5/0.6.6 빌드, v3 = 0.7.0 재인덱싱):

  | 뷰 | v2 빌드 | v3 빌드 |
  |---|---|---|
  | 5µm 구역 depth0 cutL2 | 2,158p / 97.1s | **50p / 1.7s / 11.3M** |
  | 5µm depth full cutL1 | (동류 ~100s) | **1,056p / 6.0s / 67M** (3회 점진) |
  | 102×84µm depth1 cutL1 (기준 뷰) | 3,182p / 100.6s / 509.5M | **531p / 9.6s (load 7.0+draw 2.5) / 25.3M** |
  | 0.138×0.113µm depth9 (딥줌) | 29,180p / 187.9s / 2.3G | **319p / 4.3s / 64.4M** |
  | 100µm 광역 depth full | — | 21,127p / 18.6s (load 7.6 + **draw 11.0**) |

  plan_ms도 350~1,100 → 88~192로 동반 하락. 검산: v2 기준 뷰의
  509M drawn = 59,000개/µm²(물리 불가) → ~99%가 뷰 밖 과선적의
  증명. 인덱싱 파싱은 회귀 아님(283.3s ≈ 0.4.7의 279s; 150M
  31s는 5,407셀 병렬 한계).
- **판정**:
  - LOD 기준(mid-zoom cold 3~5s) 대비 9.6s — 초과이나 초과분의
    성격이 바뀜: 만성 과선적이 아니라 **페이지 원자성 + draw**.
    M7은 유효하되 기대 이득은 자릿수가 아닌 2~3×급. 방향은 컷
    연장이 아니라 **밀도 기반**(cut L2 불감 실측: `(w<cut)AND
    (h<cut)`은 가늘고 긴 fill/배선을 어느 레벨에서도 못 걸러냄)
    + draw 절감 겸용.
  - 100µm 광역은 draw(11s) 지배 + refine이 load 종료 후에야
    시작되어 팬 시 검은 영역이 마지막에 채워짐 — **렌더 스케줄링
    튜닝 백로그**(지시: 잔여 항목 정리 후 착수).
  - ovm 25G(places 11.8G + pts pool 11.6G + inst-BVH 3.1G) →
    **mmap 전환 = M4 후속 1순위**(리더는 mmap 전제 설계).
  - 기본값 확정: `--stream-kb` 24MB(적응 2~32MB), K=4, PTS 임계
    8192 유지 — 이탈 사례 없음.

rev 18 (M3.5 외부 검토 반영 — 2026-08-03, 0.6.3; 5건 전부 실결함/
실개선 확인·수정):
- **① 우선순위 좌표계(높음)**: 페이지 bbox는 셀-로컬인데 스트리밍
  정렬이 world 뷰 중심과 직접 비교 — 배치/회전/공유 셀에서 "중심
  우선"이 무의미. **플래너가 WC-로컬 lv-box 중심→페이지 bbox
  거리²로 `HierPlan.page_prio` 산출**(중심이 bbox 안이면 0, 공유
  페이지는 min), 세션은 이를 받아 정렬만. 계층 회귀 유닛(자식이
  (100k,100k)에 배치, 로컬 메트릭만 prio 0을 줌).
- **② deferred 유령 셀(높음)**: partial 델타의 WC가 미재료화
  페이지를 이름 참조 → klayout이 빈 셀 생성, 중도 이탈 시 evict
  불가로 누적. **delta_hier가 (committed∩plan)∪이번 new만 참조**
  (avail 필터) — 유령 생성 자체가 없어짐. 게이트: check_ledger에
  "빈 P-셀 0" 전역 불변 + L6 round1 P-셀 수 == new + 중도-이탈
  wander 시나리오.
- **③ 인터랙션 블로킹(중간)**: 7초 refinement 동안 snap/pick/clip
  대기 → **라운드 사이에 요청 큐 드레인 서비스**(render는 재큐 후
  `latest`로 자연 중단, None은 전파). partial 상태 pick은 화면에
  보이는 것만 조회(WYSIWYG — 미재료화 지오메트리는 화면에도 없음)
  로 규정; 정밀 질의는 기존 probe 경로 그대로.
- **④ 예산 단위(중간)**: 빌드가 csize=usize=인코딩 크기를 기록해
  예산이 실제 파싱 비용과 무관 — **`write_tree_sized`로 usize =
  디코드(비압축 바디) 바이트 기록**(세션 상주 예산도 정확화),
  스트리밍 예산을 usize 기준으로 전환, **클라이언트 적응 예산**
  (요청별 `stream=` 오버라이드, 측정 라운드 시간→목표 ~0.35s로
  자가 튜닝, 2~128MB 클램프). usize 값 변경으로 **캐시 재빌드
  권장**(구조 불변, 구캐시는 예산 정밀도만 저하). 실측: 첫 페인트
  0.93s, 라운드 ~0.4s로 수렴(예산 24.5→~6MB), 총 7.46s.
- **⑤ UI/게이트(낮음)**: 프레임에 `refining=N`(gui 상태줄
  ", refining N" + "refining N pages..." — partial 동안
  rendering-done 미표시), draw_ms 누적화(load+draw ≈ 총시간),
  **L6 콜드 기준을 `stream_kb=0`으로 교정**(데몬 기본값 의존 제거).
- **⑤-b gui 입력 잠금 수정**(사용자 보고 "점진 드로잉 미동작"):
  ⑤의 첫 구현이 refinement 내내 `_pending`을 유지 → 마우스
  핸들러가 `_pending`에 게이트라 **입력이 7~9초 동결**(프레임은
  갱신되나 조작 불가 = 점진의 목적 상실). **첫 프레임에서
  `_clear_pending()`** — 입력 즉시 해제, refining 표시는 별도
  플래그, 조작 시 라운드 사이 중단→미ack 롤백→새 뷰 스트리밍
  (실서비스 검증: gen2 3라운드에서 팬 → gen3 0.7s 스트림 시작,
  기커밋 페이지 재사용으로 deferred 63으로 감소). 참고: 적응
  예산은 라운드를 잘게(~0.4s×15) 쪼개 0.6.2의 "3단계" 대비
  전환이 미세·연속적 — 진행 확인은 refining 카운트다운.
- **⑤-c 적응 예산 상방 폭주 수정**(사용자 재현 "'+' 19회 줌 후
  끝까지 무변화→한 번에 교체"): 작은 웜 라운드(1~2MB/50ms)를 파싱
  속도 표본으로 오판해 예산이 128MB 클램프까지 폭증 → 헤비 뷰가
  예산 안에 통째로 들어가 **partial 자체가 소멸**(FLOE_DEBUG로
  라운드 로그 실측: kb 24576→104637→131072, 최종 뷰 new=157
  partial=0 단일 7s 라운드). 수정: **유효 표본 조건**(그 라운드가
  실제로 예산의 ≥50%를 실었을 때만 적응 — `pending_new_mb` 사용)
  + **상한 32MB**(100MB급 뷰 ≥3 스테이지 보장) + 라운드 목표
  `FLOE_STREAM_TARGET_MS`(기본 500, 100~2000 클램프; 0.6.2식
  굵은 단계를 원하면 ~1300). `FLOE_DEBUG=1`이면 서비스가 라운드
  로그(gen/new/partial/kb)를 stderr로 출력(현장 진단용). 수정 후
  동일 재현: 최종 뷰 11라운드/0.7~0.9s 간격, 영역별(diff bbox
  상단/하단/코너 밴드) 가시 갱신 복구.

rev 17 (M3.5 점진 첫 페인트 + M4 규정 + M7 예약 — 2026-08-03,
0.6.2):
- **첫-방문 병목 실측 분해**(testchip 198×156µm·cut L1·189페이지·
  124MB·19.2M members): vfsd(플랜+ovp+스플라이스+델타 기록)
  **0.04s**, klayout `ly.read` **6.4s**, 나머지 0 — 병목은 오직
  klayout 단일-스레드 파싱(≈19MB/s). 데몬 병렬화 여지 없음,
  `Layout.read` 분할 병렬 불가.
- **M3.5 점진 로딩**: `HierSession.apply`에 라운드당 신규 payload
  **csize 예산**(vfsd `--stream-kb`, 기본 24576=24MB; 0=off) +
  **뷰-중심 거리 우선순위**(엄격 프리픽스, tie=페이지 인덱스,
  최소 1페이지 진행 보장). 응답 `partial=`/`deferred=`(§3.5).
  뷰어는 라운드마다 apply+렌더+프레임 송출 후 같은 뷰 재요청 —
  나머지 상태 없이 `plan − committed`가 자연 수렴. 중도 이탈은
  기존 stale 규약(ack 미전송=롤백) 그대로. probe/flat 무예산.
  실측: 위 뷰 **첫 페인트 6.4s → 1.08s**(41페이지 중심부), 6라운드
  총 7.2s(+12%는 화면 뒤 진행).
- **M4 규정 추가**(§5): M4는 "대표 표본"이 아니라 **알려진 최악
  사례 회귀 + 기본값 결정** — 자산별 역할, LOD 판정 기준, "기본값
  ≠ 한계"(튜너블은 노브 유지), 상태줄/지표 운영 루프 명문화.
- **M7 예약**(§5): LOD 페이지 — M5 이후, M4 판정 기준 충족 시만
  착수(v2에 lod/codec 예약돼 포맷 부채 0).
- 게이트: 세션 유닛(우선순위/부분 커밋/드롭 후 동일 청크 재전송/
  진행 보장/예산 0), lifecycle **L6**(4KB 예산 수렴·합집합=콜드
  new·중도 드롭 롤백·최종 XOR) 스위트 편입.

rev 16 (M2/M3 외부 검토 반영 — 2026-08-03, 0.6.1; 6건 중 5건 실결함
확인·수정, 검증 공백 게이트화):
- **① names= stale 유실(높음)**: 첫 hier 응답이 stale이면 런당
  1회뿐인 이름 테이블을 영구 유실 — service가 **stale 판정 전에
  names를 소비**하도록 이동(테이블은 뷰-무관). 게이트 L5.
- **② HierSession bytes 잔여(중간)**: new 페이지 크기를 plan 시점에
  장부에 기록 → 롤백이 못 지움 — **커밋 시점 기록**으로 이동
  (pending이 (page,bytes) 쌍을 나름; pending = 순수 diff 회복).
  회귀 assert 추가.
- **③ 손상 캐시 플래너 panic(중간)**: page/prange의 layer_idx·
  page.cell 미검증 — 오픈 검증에 추가(+`bs_width×8 ≥ n_layers`,
  layer name 경계). corrupt 유닛 3종 추가 — 손상은 항상 "corrupt
  cache; rebuild".
- **④ unchecked narrowing 잔존(중간)**: bitset count·bs_width·
  doc.top·bvh len/인덱스 합·pbvh 인덱스 합·**Builder 카운터 6종**
  (checked `bump`) 전부 checked로.
- **⑤ sparse Pts 프레임 재재료화(중간)**: below-cut sparse Pts는
  전 오프셋을 매 플랜 복사 — **count > PTS_FULL_REP도 footprint
  1박스로 강등**(§2.1 융합 규칙에 크기 상한 추가). 9,000점 유닛.
- **⑥ heartbeat(낮음)**: (a) stderr 출력은 **의도된 설계** —
  stdout은 vfsd 프로토콜/plan JSON 채널이라 모든 진행 로그는
  stderr(여기 명문화). (b) parse 단계 무소식은 실공백 — **parse
  heartbeat**(5s, elapsed+rss) 추가.
- **검증 공백 게이트화**: lifecycle L3를 apply **①~④ 전 단계**
  주입으로 확장(뷰어 gate-훅 `_fault_step`) + L5(names stale),
  `validate_vfs_marker.py` 신설 — `FLOE_KILL_AT` 훅으로 3지점
  **실제 강제 종료** 후 no-cache/corrupt 확인 + 재빌드 복구,
  스위트 편입. narrowing 경계는 구성 가능한 것만 유닛(카운터
  오버플로 등 실물 불가 경계는 checked 코드로 보장).

rev 15 (M3 구현 완료 — 2026-08-03, 파이썬 뷰어; 지시로 **기본 모드
= hier**):
- **뷰어 hier apply**(§3.7 ①~⑤): `VfsMosaic.apply_hier`(델타 read →
  W-top 연결 → 이전 gen WC 일괄 shallow `delete_cells` → evict
  prune + 인덱스 remap), stale 프레임은 apply 생략 = **ack 미전송이
  곧 롤백 신호**(service의 기존 `if newer(): return`이 규약이 됨),
  부분 실패는 `reset_all()`(Layout 제자리 재구성 — 렌더러 바인딩
  유지) + `reset=1` 재요청 1회. 데몬-gen은 GUI gen과 분리된 전용
  단조 카운터(`req_gen` — 실패 gen 재사용 금지 충족).
- **pick ci→이름**(§3.4): `names=` 1회 로드(캐시 공유 dict, 파일
  즉시 삭제) + `_WsNames` 리졸버(P/W 이름에서 ci 파싱) — service
  의 `mosaic.design.get()` 호출부 무수정 드롭인. probe(pick/snap/
  clip/cli region)는 `mode=hier_probe`.
- **프레임 융합 규칙 추가**(§2.1 — M3 실측이 잡은 시각 회귀):
  below-cut 배열의 멤버별 아웃라인은 피치 < 2×cut에서 서로 융합해
  **실지오메트리를 덮는 단색 워시**가 됨(발견: probe 프레임 전면
  #93a4ad). 멤버 피치(Grid) 또는 extent 밀도(Pts)가 2×cut 미만이면
  **footprint 1박스(Rep::One)로 강등** — 분해 가능하면 기존대로
  멤버별. 수정 후 hier/flat 서비스 렌더 **PNG 바이트 동일** 확인.
  이후 실칩에서 all-layer rbbox가 visible-layer와 무관한 stripe를
  만드는 것이 확인되어, 이 규칙은 현재 depth-boundary frame에만
  적용되고 size-cut frame은 표시하지 않는다.
- **게이트**: validate_vfs_render **6뷰**(배열 관통 마이크로 뷰 +
  경계 스트립 추가) × flat/hier 2회, `validate_vfs_lifecycle.py`
  신설 **L1~L4**(10-gen 팬 XOR+WC 잔존 0 / stale-drop 재전송 /
  부분실패→reset 복구 / 예산 0 evict churn) — 전부 스위트 편입.
  `floe probe`(실서비스) hier/flat 프레임 동일.

rev 14 (M2 구현 완료 — 2026-08-02, floe-index 0.6.0):
- **계층 델타**(§3.2): `Vfs::delta_hier` — 신규 페이지 스플라이스 +
  authored WC(`W<gen>_<r>_<ci>`, 페이지 identity + 자식
  CellInstArray + 프레임 rect(+rep) 255/0) 단일 OASIS, gen top WC가
  단일 top(parse_doc 게이트 통과, 증분 델타는 ovp 무-IO).
- **HierSession 2단계 커밋**(§3.7): pending = committed 대비 **순수
  diff**(적용은 커밋 시점에만) — 롤백 = diff 폐기라 undo 불필요,
  의미는 명세와 동일(new 취소/evict 복원/LRU touch 원복). ack
  상태기계(커밋/롤백/중복 no-op/미래 ack·gen 비단조 에러·에러 시
  pending 보존), projected 예산, reset(장부 초기화, gen 단조 유지).
- **vfsd**(§3.5): `mode=hier|hier_probe` + `ack=`/`reset=` 파싱,
  응답에 `top=`/`names=`(런당 1회, hier_probe 포함 첫 응답)/상주
  4지표/`wc_cells= inst_edges= frame_rects=`, hier에서
  `placements=/frames=` 미생성. 세션-모드는 런당 잠금
  (`error=mode_switch`), probe는 자유. flat 경로 무변경.
- **게이트**: 트랜잭션 유닛 3종(commit/stale-drop 재전송/프로토콜
  에러/projected/LRU-touch 롤백), delta 라운드트립 2종(full/증분·
  프레임·rebased Pts·결정성), `tools/validate_vfs_hier.py` H1~H5
  (실데몬: gen1 apply → 증분 이름-바인딩+shallow delete → stale
  롤백 재전송 → dup-gen 에러/reset 복구 → **hier_probe cut=0 델타
  = 소스 지오메트리 XOR 일치**) — validate_rust.sh에 편입.

rev 13 (M1 구현 완료 — 2026-08-02, floe-index 0.5.0):
- `.ovm` v2 + 계층 플래너 + `plan --mode hier|flat` 구현·게이트
  통과 — 결과·구현 노트는 §5 M1 결과 블록. flat 경로는 스펙대로
  병존(vfsd/뷰어는 M1에서 무변경 — 라이브 경로 영향 0).
- 스펙 대비 편차(전부 안전 방향): 프레임은 rep-extent footprint가
  localview에 닿을 때만 방출(스펙은 무조건 — 뷰-무관 프레임 억제
  강화), det=0 비정규 2-D Grid는 보수 full-range(허용), 빌더
  narrowing은 "limit exceeded: <필드>" 하드 에러(패닉) 방식.

rev 12 (M0 실증 완료 — 2026-08-02, klayout 0.30.9/pip, macOS):
- **바인딩 GREEN**(§3.3/§5): 옵션 없는 `Layout.read`가 파일-미정의
  상주 셀 참조를 기존 셀에 정확히 바인딩 — 30 gen 연속: 충돌 변형/
  유령 잔존 0, 지오메트리 XOR 0(회전·Grid·Pts rep 경유 포함),
  shallow `delete_cells` 후 페이지 전원 생존(고아 포함), RSS 평탄.
  hier.tsv 폴백은 **불사용 판정**(명세는 §3.3에 보험으로 보존).
  관찰: 리더가 미정의 참조당 임시 셀을 만들고 병합-해제해 **cell
  index 슬롯만 단조 증가**(`Layout::cells()` = 슬롯 수, 라이브 수는
  `each_cell` — §3.1 구현 노트).
- **Pts 비전개 확정**(§2.3/§6-6): klayout은 type-10을 iterated
  array로 유지 — 1M 점: 레코드 1, read 0.146s(≈0.15µs/점), RSS
  ≈26B/점, **좁은-뷰 draw는 N 무관 0.008s 상수**(뷰 클립 동작).
  사다리의 근거를 "전개 방지" → **델타 바이트·스캔 CPU 바운드**로
  교정, full-rep 임계 1024 → **8192 기본**(≈50KB wire;
  testchip_1g5 최대 3,823이 전부 무스캔 경로에 들어옴; M4 A/B
  확정). 리베이스 XOR(선택 0/1/≥2 × 5크기) 전부 green — §2.3
  rebase 규칙 실증. pya 폴백 단가: 개별 삽입 ~55만/s, 1M 점
  1.8s+47MB(네이티브 read 대비 12×) — §1.1 flat load 병목과 정합.
- 스파이크 재현: `rust/oasis/examples/m0_gen.rs`(생성기),
  `tools/m0/*.py`(스파이크), 산출물 `data/m0/` — 결과표 §5 M0.

rev 11 (검토 소항목 3건):
- 오픈 검증에서 `len % bs_width` **이전에 `bs_width > 0` 선검증**
  (corrupt 캐시가 0-나눗셈 panic이 되지 않게)(§3.6).
- checked 규칙을 **헤더 총계·섹션 off/len 산술에도 명시 적용**
  (v1의 `off+len > data.len()`은 u64 wrap 가능 — checked_add/
  checked_mul)(§3.6).
- **Morton 키 알고리즘 고정**(§2.3): extent min 기준 unsigned 변환
  (i128 경유) + u64×2 → u128 비트 인터리브, 동률은 정렬-전 index —
  jobs/플랫폼 무관 결정성.

---

## 1. 문제: 평탄화가 klayout의 스케일링을 버린다

9.8G / 150M급 OASIS가 애초에 klayout에서 열리는 이유는 klayout이
**계층적으로** 그리기 때문이다: 셀을 1회 그리고, 인스턴스/배열로
재사용하고, 뷰에 클립한다. 현재 VFS 뷰어 경로는 `plan/descend`가
계층을 걷으며 `(page, placement)` 쌍의 **평탄 리스트**를 만들어
단일 `FLOE_WS` 밑에 월드좌표로 전부 붙인다. 이 평탄화가 klayout의
계층 재사용을 통째로 버린다.

### 1.1 실측 (MAIN09, 뷰 0.7×0.6um, cut_um≈0.001, `floe-index plan`)

| 지표 | 값 | 해석 |
|---|---|---|
| placements | **6,343,345** | 워킹셋 배치 630만 → klayout 인스턴스 삽입이 load 50s |
| members | 1,712,332,519 | 17억 (draw 추정) |
| visited_cells | 521,187 | 0.7um 뷰가 52만 셀 방문 = **뷰 무시** |
| pages | 6,111 (17.6MB) | 유니크 페이지 |
| culled_subtrees_size / pages_size | 0 / 0 | 이 컷에선 컬링 전무 |
| plan_ms | 259 | 플랜 자체는 빠름 → 병목은 klayout load |

뷰어 상태줄(동일 병리):
```
live (752 tiles, 801ms=318 load+483 draw, cut<0.148um, ~348.7M) view 75.3x62.0um
live (839 tiles, 29563ms=29250+313, cut<0.0414um, ~677.9M)      view 7.4x6.1um
live (1790 tiles, 50559ms=50204+354, cut<0.001um, ~1.2G)        view 0.7x0.6um
```
줌인(뷰↓)인데 tiles·load·drawn이 **증가**한다. load 50s에는 vfsd의
6.3M행 TSV 기록/파싱과 파이썬 6.3M회 CellInstArray 삽입이 다 포함.

### 1.2 두 증상, 한 뿌리

- **뷰 확장** (`lib.rs descend`, Grid arm `r2.view = cwb`): 배열
  자식으로 내려갈 때 뷰를 배열 footprint 전체로 확장 → 배열 밑에서는
  사용자의 좁은 뷰가 무시됨. 줌인으로 size-cut이 멈추면(cut_um→0)
  배열 콘텐츠가 통째로 materialize (visited_cells 52만).
- **`fold_array` 중첩 평탄화** (`lib.rs` 424-453): 자식이 이미 rep를
  가진(배열의 배열) 경우 바깥 배열을 na×nb **개별 Mat로 전개**
  (placements 630만). 코드 주석의 "중첩은 얕고 개수는 적다" 가정이
  MAIN09에서 깨졌다.

둘 다 "평탄 모델은 배열/계층을 표현할 수 없다"의 증상이다. 증상별
패치(가시 멤버만 전개 등)로는 근본이 남는다.

---

## 2. 이상적 설계: 계층 보존 워킹셋

**원칙:** 워킹셋 = 소스 계층의 **가지치기된 복사본**. VFS는 BVH로
"이 뷰에 필요한 셀/인스턴스/페이지"만 고르고, klayout에 **그 계층을
그대로** 넘긴다. klayout이 인스턴스 변환 누적·배열·뷰 클립을 native로
처리한다. 평탄화·per-member 전개·뷰 확장 전부 사라진다.

### 2.1 워킹셋 레이아웃 구조 (1-레벨 → 다중 레벨)

현재: `FLOE_WS` → 페이지 셀(월드좌표로 직접, 배열 rep는 Mat에).
목표:
```
FLOE_WS
  └─ WC(top)                          (top 소스 셀의 워킹셋 셀, 1회 배치)
       ├─ 페이지 셀 (top 자기 지오메트리, localview 교차 & cut 이상만)
       ├─ CellInstArray → WC(childA)   (로컬 xf; 배열이면 na,nb,va,vb 보존)
       ├─ CellInstArray → WC(childB)   (중첩 배열이면 WC(childB) 내부에 또 배열)
       └─ 프레임 rect (cut 미만 자식; 배열이면 rect+rep로 배열 보존)
```
- 워킹셋 셀의 키는 **`WsKey = (ci, remaining_depth)`**(§2.5). full
  depth(뷰어 검사 모드 기본)에서는 remaining_depth가 단일 sentinel
  이라 **셀당 WC 1개** — dedup은 이 키 기준이다. C가 여러 경로/
  변환으로 쓰여도 (같은 키면) 셀은 한 번 로드, 인스턴스가 변환을
  나른다(klayout 방식).
- **WC는 상주 객체가 아니다**: 내용이 `localview`의 함수라 프레임마다
  달라진다. 세션에 상주하는 것은 페이지(바이트)뿐이고, WC 셀은 기존
  `FRAMES_{gen}`/`LABELS_{gen}`과 같은 **프레임-ephemeral**(§3.1).
- `WC(C, r)` 내용:
  - **페이지**: C 자기 지오메트리 페이지 중 localview에 닿고
    `max_w/max_h ≥ cut`인 것만 (identity 인스턴스). 페이지 셀은
    variant 간 **공유**된다(같은 P-이름을 여러 WC가 참조).
  - **자식 인스턴스** (r>0일 때): 서브트리가 뷰에 닿고 cut 이상인
    자식으로 가는 `CellInstArray` → `WC(child, r-1)`. **배열은
    배열로 보존**(na,nb,va,vb), **중첩 배열은 중첩 그대로** — 전개
    없음.
  - **프레임**: 사용자가 지정한 depth 경계 자식의 외곽선 rect,
    **WC 로컬 좌표**. visible `lmask_rec`을 먼저 통과한 자식만 만들며,
    배열 rect에는 rep를 실어 klayout이 배열로 처리한다. **융합 규칙**:
    멤버 피치
    (Grid 벡터) 또는 extent 밀도(Pts)가 **2×cut 미만**이면 멤버
    아웃라인들이 화면에서 융합해 실지오메트리를 덮는 단색 워시가
    되므로 **footprint 1박스(Rep::One)로 강등**한다(flat 동등).
    FRAME_CAP(200K)은 플랜 총량 상한으로
    유지하되 WC 단위 dedup 덕에 자연 감소. size-cut 자식은 all-layer
    recursive bbox가 선택 레이어의 올바른 proxy가 아니므로 프레임 없이
    생략한다. per-(cell,layer) proxy가 도입되기 전에는 false geometry를
    표시하지 않는 것이 정확성 계약이다.

### 2.2 BVH 정확 활용 (인스턴스 + 페이지)

- **인스턴스 컬링**: 셀마다 인스턴스 BVH를 localview로 프루닝 — 뷰에
  닿는 인스턴스만 방문(공간 컬링의 본래 목적). 현행 descend도 BVH를
  걷지만 flat mat 생성용이라 뷰 확장으로 무력화됨.
- **페이지 컬링**: 페이지는 빌드 때 1MB 목표(`PAGE_TARGET_BYTES`)로
  공간 분할되어 있으나, 이는 분할기일 뿐 **질의 인덱스가 아니다** —
  현행 코드는 materialize된 셀의 페이지 전체를 선형 순회한다
  (lib.rs:193). fill-heavy 톱셀(수십만 페이지)에서 프레임마다
  반복되면 병목이므로 **page BVH를 M1 범위에 포함**한다:
  - **빌드-시 packed 구축**(`.ovm v2`, §3.6), 레이어를 구조로 분리:
    페이지는 셀 안에서 (layer, seq)로 연속 저장되므로 **(cell,layer)
    별 page-range 레코드**(→ §3.6)를 두고, 그 run마다 페이지 BVH
    루트를 단다. 레이어 컬링은 range 테이블에서 루트째 skip —
    리프 비트 필터가 아니라서 다층 중첩 셀에서 단일-레이어 뷰가
    전 노드를 방문하는 병리가 없다.
  - **노드 집계**: bbox + **subtree max_w/max_h** — broad view의
    페이지 cut(`max_w/max_h < cut`)을 리프 이전에 서브트리째 컬링.
  - **리프 = 페이지 디렉터리의 연속 구간**(first/count는 디렉터리
    인덱스): 빌드가 각 (cell,layer) run **안에서** 페이지를 BVH 리프
    순서로 재배열한다(seq는 이름/식별자일 뿐 저장 순서 아님; delta는
    file_off 정렬 IO라 무관). 별도 page-index 간접 배열 불필요.
  - `page_count ≤ 임계(예: 8)`인 run은 루트 없이 선형 순회
    (pbvh_root = NONE).
  - 계측(§3.5): `culled_page_layer_roots`(레이어로 skip한 run),
    `culled_page_bvh_bbox`, `culled_page_bvh_cut`(집계로 자른
    서브트리), `visited_page_bvh`, `page_candidates`.

### 2.3 클립영역 전파 (뷰 확장·fold 평탄화를 대체)

각 워킹셋 노드의 로컬 프레임에서 "뷰가 닿는 영역" `localview`를
top-down 전파한다 (키는 WsKey, §2.5):

- `localview(top) = view` (top-local = world).
- 인스턴스 C→child (로컬 변환 T, `Rep::One`):
  `localview(child) ∪= T⁻¹(localview(C)) ∩ child.rbbox`.
- **배열** (`Rep::Grid{na,nb,va,vb}`): 멤버 열거 없이 닫힌형.
  멤버 오프셋 `o = i·va + j·vb`는 부모 프레임의 순수 병진이므로,
  1. **가시 오프셋 영역**: `R = localview(C) ⊕ (−B₀)`
     (⊕ = Minkowski 합; B₀ = offset-0 멤버의 부모-프레임 bbox.
     멤버 m 가시 ⟺ `(B₀+o_m) ∩ lv ≠ ∅` ⟺ `o_m ∈ R`).
  2. **가시 인덱스 범위**: `det([va vb]) ≠ 0`이면 R의 네 꼭짓점을
     2×2 역행렬로 인덱스 공간에 투영, 보수적 정수 bbox
     `(i_min..i_max)×(j_min..j_max)`를 만들고 `[0,na)×[0,nb)`로
     clamp. `det == 0`(nb=1의 vb=(0,0), 또는 공선 벡터)이면 1D 축
     투영(성분별 구간 나눗셈 교집합) 또는 보수적 full-range.
     곱셈·행렬식은 **i128 checked** 연산, 나눗셈은 `div_floor`/
     `div_ceil` 명시 사용(음수 벡터 포함). 오버플로 시 에러가 아니라
     **보수적 full-range 폴백**(누락 금지 — 과다 포함은 안전).
  3. **자식 localview 기여**: 가시 인덱스 bbox의 4-코너로 가시
     오프셋 bbox `O_vis`를 만들고
     `localview(child) ∪= T₀⁻¹(localview(C) ⊕ (−O_vis)) ∩ rbbox`.
  인스턴스는 **CellInstArray 1개**(full na×nb) — 오프스크린 멤버는
  klayout이 클립. **중첩 배열**은 `WC(child)`가 내부에 자기 배열
  인스턴스를 가지므로 자연히 중첩 — 전개 0. 전 단계 배열당 O(1),
  플래너 시간은 멤버 수와 무관해야 한다(§7 게이트).
  - 주의: 기존 `visible_offsets`의 Grid 분기(lib.rs:354-378)는
    descend가 호출하지 않는 **데드코드**이고 `for j in 0..nb` 전 열
    순회(O(nb))라 재사용 금지. 위 닫힌형으로 새로 쓰고 브루트포스
    대조 유닛테스트를 붙인다(§7).
- **불규칙 rep** (`Rep::Pts`): 현 구조로는 어떤 경로도 싸지 않다 —
  `Ovm::place()`가 pool을 **매번 Vec으로 복사**(lib.rs:579-589)하고
  `rep_extent`도 Pts는 전 점 순회이며, 실측(testchip_1g5)은
  placement 697 중 Pts 410, 오프셋 합계 147만/최대 3,823이라
  per-rep 임계만으로는 요청 총비용이 안 잡힌다. v2에서 구조로 해결:
  - **pool 엔트리에 extent + chunk 인덱스 굽기**(§3.6): extent
    4×i64와 256-오프셋 chunk별 bbox를 빌드가 미리 계산. 빌드는
    오프셋을 **Morton 순서로 재배열**한 뒤 256개씩 chunk로
    묶는다. 키 알고리즘 고정(jobs/플랫폼 무관 결정성):
    `zx = u64(x − extent.min_x)`, `zy = u64(y − extent.min_y)`
    (뺄셈은 i128 경유 — i64 범위 차는 u64에 항상 들어감),
    `key = interleave(zx, zy) → u128`(x = 짝수 비트, y = 홀수
    비트), 정렬 키 = `(key, 정렬-전 index)`. 정수 연산만 사용.
    이렇게 하는 이유: 원본 순서 그대로 묶으면 공간이 섞인
    파일에서 모든 chunk bbox가 extent에 수렴해 좁은 뷰도 전
    chunk를 스캔하게 된다. rep는 멤버 "집합"이 의미라(§7 멤버-수
    대조) 재배열은 rebase만 정확하면 안전.
  - **zero-copy 접근자**: 플래너는 `place()`의 Vec 복사 대신 pool
    의 borrowed byte slice를 **LE 이터레이터로 디코드**해 읽는다
    (`&[(i64,i64)]` 캐스트 금지 — 정렬/엔디언).
  - **방출 사다리**(M0 실증 완료: klayout은 type-10을 전개하지
    않고 iterated array로 유지 — 1M 점 = 레코드 1, 좁은-뷰 draw
    N 무관 상수(§5 M0). 임계가 바운드하는 것은 전개가 아니라
    **델타 바이트와 스캔 CPU**다):
    `|pts| ≤ PTS_FULL_REP`(기본 **8192** — M0로 1024에서 상향:
    full rep ≈6.1B/점 wire·0.15µs/점 read라 8192 ≈ 50KB/1.2ms,
    testchip_1g5 최대 3,823 → 실자산 전부 무스캔 경로; M4 A/B로
    최종 확정) → full rep 1개. 초과 → **가시-부분집합 Pts**:
    chunk bbox ∩ R인 chunk만 스캔해 `o ∈ R` 멤버 선택. 요청당
    `PTS_ENUM_BUDGET`(예: 20만 점, per-point 테스트 CPU 캡) 소진
    시 → chunk-단위 통짜 포함(초과포함 ≤ 256×chunk 수로 유계).
    최후 폴백 full rep + 경고 계측. 어느 단도 멤버 누락은 없다
    (부분집합은 항상 `⊇ 가시`). 방출량의 하한은 진짜 가시 멤버
    수 — 그건 flat도 마찬가지였고, 사다리는 그 위의 초과분만
    바운드한다.
  - **부분집합 rebase (정확성 필수)**: `Rep::Pts`의 불변조건은
    **첫 오프셋 (0,0)**(doc.rs:28)이고 writer는 첫 점을 기록하지
    않는다(write.rs:98). 선택된 오프셋 `[p0, p1, ...]`은 —
    0개: placement 생략 / 1개: `Rep::One` + 원점을 p0만큼 이동 /
    ≥2개: 원점 += p0, rep = `[0, p1−p0, p2−p0, ...]`.
    선례: tiler의 동일 구현(hier.rs:869 "rebase: Pts offsets are
    anchored at (0,0)"). **멤버 identity = pool slot index**(Morton
    재배열 후 원본 index는 wire에 없다): K-box 여러 개의 선택
    합집합은 slot index로 dedup, 방출 순서 = pool(Morton) 순서.
    좌표값 dedup은 금지 — 동일 좌표의 별개 slot은 별개 멤버(합법,
    멤버-수 검증의 대상)다. rep는 멤버 multiset이 의미라 이걸로
    정확성이 유지된다.
  - **O_vis(자식 localview 기여) 사다리**: extent를 일률 사용하면
    대형 Pts에서 자식 localview가 셀 전체로 부풀어 옛 `view=cwb`
    병리가 재현된다. extent는 **1차 교차 판정 전용**이고 기여는
    선택 결과의 bbox로 —
    **소형(≤1024)**: 방출은 full rep여도 기여는 **정확 스캔**한
    가시-선택 bbox(두 점짜리 대각 Pts도 extent를 쓰면 부푼다) /
    선택 성공: 선택 오프셋들의 bbox / chunk-통짜: 포함 chunk
    bbox들의 합 / full 폴백만: extent. (방출과 기여가 같은 스캔을
    공유 — 인덱스가 placement 방출뿐 아니라 하위 WC 페이지 선택도
    실제로 줄인다.)
  - 계측(§3.5): `pts_offsets_scanned` / `pts_selected` /
    `pts_offsets_emitted` / `pts_bytes_emitted` +
    `pts_enumerated`/`pts_fallback`(사다리 단별).
- **스케줄링 = topo-rank 1패스 (fixpoint 없음)**: 소스 계층이
  DAG라는 전제 하에, **빌드가 전역 topo rank를 계산해 `.ovm v2` 셀
  레코드에 굽고**(§3.6 — 기동 시 O(places) 패스 불필요), 플랜마다
  rank-순 min-heap 스윕 — 노드를 pop하는 시점에는 모든 부모의
  기여가 끝나 있으므로 localview 확정 → cut/layer 판정 → BVH 프루닝
  → 자식 기여 push. WsKey당 정확히 1회 확장, 수렴·재방문 계측
  불필요, 비용 O(가시 서브트리 × log). (variant 키의 rank는 ci의
  rank를 그대로 쓰면 된다 — 부모 셀의 rank가 항상 낮으므로.)
  - **DAG 전제는 빌드가 보장한다**: 현행 `topo_order`는 back-edge를
    조용히 건너뛸 뿐(cli/src/vfs.rs:443) 거부하지 않는다. v2
    빌드에서 rank 부여가 곧 검증이 된다 — **rank를 못 받는 셀 =
    사이클 → 하드 에러**(스펙상 OASIS 계층은 비순환; 깨진 파일은
    인덱싱 단계에서 거부). vfsd는 .ovm의 rank/DAG를 신뢰한다.
- 다경로 누적: localview는 **box K개(기본 4)의 집합**으로 유지.
  - **질의는 box별**: 인스턴스 BVH·page BVH·Grid/Pts 가시범위를
    각 box로 따로 질의한 뒤 결과를 dedup(페이지 set, 자식 기여는
    box 단위로 자식의 K-box 집합에 추가). 중간에 단일 bbox로 합치면
    K-box의 이점이 사라지므로 **금지**.
  - **병합 결정성**: K 초과 시 "낭비 최소" 쌍 병합 —
    `area(bbox(a∪b)) − area(a) − area(b)`가 최소인 쌍, 동률이면
    (x0, y0, 생성순) 사전순 tie-break. 같은 요청은 항상 같은 플랜을
    내야 한다(§7 결정성 게이트).
  - K=1이면 단순 bbox 합집합으로 퇴화(A/B 대조용). 상한 = rbbox.
    이산 병리 근거는 §6-2.
- **변형(variant, 기하학적) 재료화 불필요**: 회전/미러는 인스턴스
  변환으로 klayout이 처리. `.ice`의 variant 방식보다 단순.

근거(정확성): 페이지/멤버 B가 필요 ⟺ 어떤 보이는 멤버 o에 대해
`base(B) + o ∈ view`. 오프스크린은 klayout이 클립하므로 localview
합집합에 대해 셀을 1회 구성하면 누락 없음. 합집합·보수 bbox의 과다
포함은 "진짜 지오메트리를 더 싣는 것"이라 오류가 아니라 비용이다.

### 2.4 cut은 셀 단위 속성 (에지 단위 아님)

파서가 magnification/임의각 placement를 거부하므로(doc.rs:528) 변환은
quarter-turn+flip뿐이다. 인스턴스의 world 크기는 w/h 스왑까지 셀
고유이고, cut 판정 `(w<cut)&&(h<cut)`은 스왑 불변 → **above/below-cut
은 (cut이 주어지면) 셀 단위로 1회 분류**된다. 인스턴스별로 갈리는
경우는 구조적으로 불가능. full-depth geometry walk의 below-cut 셀은
WC를 만들지 않는다. all-layer recursive bbox는 선택 레이어의 정직한
대체 형상이 아니므로 size-cut 프레임도 방출하지 않는다. 단, 유한
depth의 구조 frontier를 찾는 walk는 cut과 별개다(§2.5).

### 2.5 depth 의미론: remaining-depth WC variant

depth는 실기능이다: gui 첫 페인트가 depth=1(gui.py:277-283),
스핀박스/단축키 존재. 현행 의미론(plan-side, descend의
`depth >= req.depth` 중단): 경로 깊이 p의 셀은 **잔여 예산
`r = d − p`**를 갖고, r=0이면 자기 페이지만, r>0이면 자식도 —
즉 절단은 **경로별**이다.

공유 WC 하나로는 경로별 절단이 불가능하므로(같은 셀이 r=0과 r=2로
동시 도달), **WC를 `WsKey = (ci, remaining_depth)`로 variant화**한다:

- `WC(C, r)`: 페이지(항상) + r>0이면 자식 인스턴스 →
  `WC(child, r−1)`. 페이지 셀은 variant 간 공유(지오메트리 복제 0,
  배열 전개 0) — 늘어나는 것은 경량 WC 정의뿐.
- **full depth는 단일 sentinel variant**(r=∞): 뷰어 검사 모드
  기본값에서는 셀당 WC 1개로 rev 2와 동일, 오버헤드 0.
- **variant 정규화**: `r ≥ height(ci)`이면 절단할 것이 없으므로
  sentinel(F)로 접는다 — `height`는 **v1 셀 레코드에 이미 존재**
  (CellV.height, 빌드 topo fold가 계산). 이 정규화로 유한 d에서도
  깊은 서브트리 밖의 variant 증식이 구조적으로 잘린다.
- 유한 d에서 variant 수는 셀당 최대 `min(d+1, height 분포)`(첫
  페인트 d=1이면 ≤2). localview는 WsKey별로 누적한다.
- min-path-depth 방식(rev 2)은 다경로 셀에서 사용자가 지정한 depth
  보다 깊은 디테일을 보여주는 **의미 변경**이라 철회.
- **비누적 frontier**: 유한 depth에서만 r=0 셀의 직계 자식을
  FRAME_LAYER의 hollow bbox(+rep)로 그린다. r>0/다른 depth의 bbox를
  함께 쌓지 않는다. full sentinel 또는 `r >= height(ci)` 정규화로
  full에 접힌 경로에는 frontier가 없다. 따라서 마지막 깊이/full은
  구조선이 없는 것이 정상이다.
- frontier walk와 합성 블록명은 visible layer 및 geometry cut을
  무시한다. 실제 페이지/설계 텍스트만 visible layer와 cut을 따른다.
  이 분리 덕분에 `layers=none`도 Calibre식 구조 탐색 뷰가 된다.
- FRAME_LAYER는 KLayout LayoutView의 실제 paint stack에서 모든 설계
  레이어보다 먼저 오는 underlay이며, pick/snap/clip의 설계 형상으로
  취급하지 않는다. Layout 등록 순서만으로는 부족하다. LayoutView가
  source layer 번호로 재정렬하므로 layer-properties 순서를 명시적으로
  고정한다. 반복 bbox는 화면
  피치 2px 미만 또는 재료화 상한일 때만 footprint로 융합한다.

### 2.6 결과

- **inst_edges**(구 placements) = 가시 서브트리의 인스턴스-에지 수
  (수백), 배열 멤버 수(630만)가 아님.
- **pages** = 뷰 교차분만(수백).
- 깊은 줌(거대 중첩 배열 포함) = WC 셀 몇 개 + CellInstArray 에지
  몇 개 + 페이지 몇 개 → **load 50s → 1초 미만** 목표 (6.3M행 TSV와
  6.3M회 파이썬 삽입도 함께 소멸).
- **9.8G 스케일**: klayout 자신의 계층 스케일링을 그대로 쓰므로,
  소스가 klayout에서 열리는 한 워킹셋도 열린다.

---

## 3. 델타 / 프로토콜: OASIS 계층 하나로 통합

현행: 델타 = 페이지 셀만 스플라이스한 OASIS + 배치(mats) TSV +
frames OASIS 별도. 목표: **WC 계층 전체를 OASIS 하나로** 방출.

### 3.1 WC 수명·이름 (gen-ephemeral)

- 페이지 `P<ci>_<li>_<seq>`(현행, 빌드 때 .ovp에 구움). WC는
  **`W<gen>_<r>_<ci>`** (r = remaining_depth, full은 `F`) — 순수
  숫자/고정 문자만. design명은 이름에 **넣지 않는다**: 공백이 있으면
  `top=W...` 공백 구분 라인 프로토콜이 깨지고, 유니코드/특수문자
  위생 문제도 생긴다(design명 전달은 §3.4의 테이블로).
- WC는 매 프레임 새 gen 이름으로 방출하는 ephemeral 셀. 이름 충돌이
  원천적으로 없으므로 klayout read의 셀 병합/재정의 의미론에 기대지
  않는다(현행 "이름 유일 → read 무병합" 전제(viewport.py 주석)를
  그대로 계승).
- apply는 이전 gen의 WC 셀들을 **`delete_cells`(일괄, shallow)**로
  제거 — 개별 delete_cell 반복의 cell-index 재배치 비용 회피.
  주의: viewport.py의 기존 `prune_cell(ci, -1)` 패턴을 WC에 쓰면 그
  gen 트리만 참조하던 **상주 페이지까지 삭제**된다 — 반드시 shallow.
- 구현 노트(M0 관찰): klayout 리더는 델타의 미정의 참조마다 임시
  셀을 만들고 이름 병합 후 해제한다 — **cell index 슬롯**이 gen마다
  (정의 셀 + 미정의 참조 이름 수)만큼 단조 증가하고 재사용되지
  않는다. `Layout::cells()`는 슬롯 수라 라이브 수는 `each_cell`로
  세야 한다(뷰포트의 이름-기반 레지스트리는 무영향). 30 gen 실측
  RSS 평탄 — 세션 수명 기준 무해, 지표로만 관찰.
- eviction은 현행대로 **페이지 단위**(Session bytes 예산, evict
  이름 목록). WC는 evict 대상이 아니다. (Session은 plan에 없는
  페이지만 evict하므로 현재 gen WC가 evict된 페이지를 참조하는 일은
  구조상 없다 — §7에서 게이트로 확인.) 신규/evict의 장부 반영은
  **ack-gen 트랜잭션**(§3.7)을 따른다 — stale drop이 레이아웃
  장부를 오염시키지 않는다.
- top WC 1개를 `FLOE_WS`에 배치(응답의 `top=` 필드, §3.5).

### 3.2 델타 구성 (authored + 스플라이스 혼합)

- 신규 페이지: `.ovp` 바디 **바이트 스플라이스**(현행 `splice_tree`,
  무해제).
- WC 셀: authored로 방출 — 자기 페이지 identity 인스턴스 + 자식 WC
  `CellInstArray` + 프레임 rect(+rep). `WCell.places`가
  (name,x,y,rot,flip,**rep**)를 이미 지원(write.rs:265,562-602)하므로
  writer 확장 불필요; `write_tree(&wcells)` → `tree_body` → 페이지
  바디들과 같은 splice에 합류.
- **frames**: 별도 `frames_{gen}.oas` 폐기, WC 내부 지오메트리로
  흡수. FRAME_LAYER(255/0) hollow 렌더 규약은 유지.
- **모달 정합은 구조적으로 보장**(리스크 아님): `tree_body`가 바디
  첫 레코드=CELL(13/14)을 assert하고, OASIS 모달 변수는 셀마다
  리셋되며, write_tree는 셀마다 XYRELATIVE를 재선언한다. 혼합
  라운드트립 테스트로 못박기만 한다(§7).
- 부수 이득: top이 WC(top) 하나가 되므로 델타가 **정규 단일-top
  OASIS** — `parse_doc` 가능(현행 multi-top splice는 불가) → 테스트
  용이.

### 3.3 상주 페이지 참조 바인딩 (성립 전제, 최대 리스크)

WC의 PLACEMENT는 델타 파일 안에 정의가 없는 **상주 페이지 이름**을
참조한다. klayout `Layout.read`가 이 참조를 기존 셀에 이름으로
바인딩해 줘야 §3 전체가 성립한다. **M0 확증 완료(GREEN)**: klayout
0.30.9, 옵션 없는 plain read로 30 gen 연속 — 충돌 변형(`$`)/유령
잔존 0, 지오메트리 XOR 0(회전·Grid·Pts rep 경유 포함), shallow
`delete_cells` 후 페이지 전원 생존(그 gen만 참조하던 고아 포함),
RSS 평탄. 절차·수치는 §5 M0. (관찰: 미정의 참조는 임시 셀 생성→
이름 병합→해제로 처리되어 cell index 슬롯만 남는다 — §3.1 구현
노트.)

폴백(**불사용 판정** — 다른 klayout 계열 대비 보험으로 명세만
보존): 페이지 스플라이스는 그대로 두고 **WC 구성만 pya API
로**(프레임당 에지 수백이라 저렴; 사실상 mats-TSV의 계층형 후계).
폴백의 전송 포맷도 지금 명세한다 — `hier_{gen}.tsv`, 행 단위:
```
parent_ws  child_kind  child_id  x  y  rot  flip  rep
```
`parent_ws` = WC 이름, `child_kind` = `page|wc|frame`, `child_id` =
P-이름 / WC 이름 / FRAME_LAYER rect(rect는 x,y에 코너, rep 열의
`r:w:h` 확장 사용). **rep 열**(Pts 표현 가능해야 함 — na/nb만으로는
불가): `-`(단일) | `g:na:nb:vax:vay:vbx:vby`(Grid) |
`p:<row>`(Pts — 동반 `pts_{gen}.tsv`의 행 참조, 행 = `x,y` 쌍
공백-구분 목록). 어느 쪽이든 §2의 플랜 산출물은 동일하다.

### 3.4 pick의 design 이름 (ci→이름 테이블)

mats-TSV 12열(design명)은 placements가 아니라 **pick이 소비**한다
(service.py `mosaic.design`). TSV 폐기 후에는:

- pick은 셰이프가 담긴 페이지 셀 이름 `P<ci>_<li>_<seq>`에서 **ci를
  파싱**하고, **ci→design명 테이블**로 이름을 얻는다. parent-WC
  역추적은 쓰지 않는다(페이지가 여러 depth variant WC에 공유되면
  모호하고, 공백/유니코드 이름 문제도 재발).
- 테이블은 vfsd가 기동(또는 첫 요청) 시 `names=<path>` TSV
  (`ci \t design명`)로 **1회 전달**. **수명 규칙**: 클라이언트는
  수신 즉시 전체를 메모리에 로드하고 파일을 지운다(이후 재전송
  없음 — 재요청 키도 없음). design명의 공백/유니코드는 TSV 값
  위치라 안전.

### 3.5 프로토콜 / 지표

- 요청의 layer 문법은 `layers=all|none|a/b,...`이다. `none`은 빈
  설계 레이어 마스크이며 `all`로 승격하지 않는다. 유한 depth에서는
  geometry가 0개여도 hierarchy frontier/블록명을 요청할 수 있다.
- 요청: `mode=hier|flat` 스위치 추가 (A/B; M5에서 flat 제거와 함께
  삭제), **`ack=<gen>`**(마지막으로 apply 완료한 gen — 세션
  트랜잭션의 커밋 신호, §3.7), **`reset=1`**(장부 전체 초기화 —
  apply 부분 실패 복구, §3.7).
- 응답: `placements=`(TSV 경로)·`frames=`/`nframes=` 폐기,
  `top=W<gen>_<r>_<ci>` 추가(숫자 이름이라 공백-구분 프로토콜 안전),
  기동/첫 응답에 `names=` 1회(§3.4). `pages/new/evict/bytes/members/
  plan_ms`는 유지, `resident_mb`는 `resident_committed_mb/
  resident_projected_mb/pending_new_mb/pending_evict_mb`로 대체
  (§3.7). **`partial=0|1`/`deferred=N`**(M3.5 점진 로딩 — 이번
  응답이 `--stream-kb` 예산에 걸렸는지와 이월 페이지 수; partial=1
  이면 클라이언트는 apply·ack 후 같은 뷰를 재요청).
- probe 모드(pick/snap/clip의 `_probe_layout`, cli `_vfs_region`)도
  같은 델타 포맷을 그대로 탄다(세션-무관이라 ack 불필요) — 별도
  구현 없음, 검증만(§7 clip XOR).
- `plan` 스텁 지표 추가: `wc_cells`(variant 포함), `wc_variants`
  (유한 depth에서만 >0), `inst_edges`, `frame_rects`,
  `culled_page_layer_roots`, `culled_page_bvh_bbox`,
  `culled_page_bvh_cut`, `visited_page_bvh`, `page_candidates`,
  `pts_enumerated`, `pts_fallback`, `pts_offsets_scanned`,
  `pts_selected`, `pts_offsets_emitted`, `pts_bytes_emitted`.
  기존 `placements`(=mats.len())·`members`는 의미가 변하므로 A/B
  비교표에는 **rev별 정의를 병기**한다(§7).
- 캐시 포맷은 **`.ovm` v2로 범프**(§3.6). `.ovp`/`.ovc`/meta.json은
  불변.

### 3.6 `.ovm` v2 — wire 스키마 동결

포맷-유지 원칙은 없다(2026-08-02 지시). rev 3에서 호환성 때문에
runtime으로 미뤘던 것을 빌드 시점에 굽는다. 아래가 **구현 기준
스키마**다(리틀엔디언, 오프셋 = 레코드 시작 기준 바이트).

- **버전 게이트**: MAGIC `FLOEOVM1` 유지, `version(u32 @8) = 2`.
  리더 게이트는 기존(`from_bytes`의 "ovm version N" 에러) — 문구에
  "run: floe-index vfs <src> (rebuild)"를 붙인다. 구 바이너리+새
  캐시 / 새 바이너리+구 캐시 모두 명확히 거부. **전방 호환 없음**:
  version ≠ 2는 무조건 거부(폐쇄 생태계, 재빌드가 정답).
- **헤더 232B** = 고정부 88B(v1 필드 배치 유지: magic 0, version 8,
  bs_width 12, unit 16, src_size 24, src_mtime 32, top 40,
  n_layers 44, n_cells 48, n_pages 52, n_places 56, n_bvh 64,
  reserved 68, **ovp_len u64 @72**(정합 검증용), reserved u64 @80)
  + 섹션 테이블 **9개** × (off u64, len u64) @88. 섹션 순서:
  0 strings, 1 layers, 2 cells, 3 places(+pts pool tail),
  4 bitsets, 5 inst-bvh, 6 pagedir, **7 pranges, 8 pbvh**(신설).
  신설 섹션의 레코드 수는 `len / stride`로 유도.
- **셀 레코드 128B** (v1 112B에서 재배치):

  | off | 필드 | | off | 필드 |
  |---|---|---|---|---|
  | 0 | name_off u32 | | 80 | place_start u32 |
  | 4 | name_len u16 | | 84 | place_count u32 |
  | 6 | reserved u16 | | 88 | page_start u32 |
  | 8 | **height u32** | | 92 | page_count u32 |
  | 12 | **topo_rank u32** | | 96 | bvh_start u32 |
  | 16 | dbbox 4×i64 | | 100 | bvh_count u32 |
  | 48 | rbbox 4×i64 | | 104 | lmask_direct u32 |
  | | | | 108 | lmask_rec u32 |
  | | | | 112 | rec_members u64 |
  | | | | 120 | **prange_start u32** |
  | | | | 124 | **prange_count u32** |

  `height`는 u16 → **u32**(remaining_depth/topo_rank와 일관). 빌드의
  `height[c] + 1`은 현재 unchecked u16(cli/src/vfs.rs:777) —
  **checked add + "hierarchy depth overflow" 명시 에러**로.
  `topo_rank` 부여 실패 = 사이클 → 빌드 하드 에러(§2.3).
- **페이지 레코드 96B** (max_w/max_h **u64** 승격으로 stride 변경):
  0 cell u32, 4 layer_idx u32, **8 seq u32**(v1 u16@8), 12 lod u8,
  13 codec u8, 14 reserved u16, 16 bbox 4×i64, 48 file_off u64,
  56 csize u32, 60 usize u32, 64 records u32, 68 reserved u32,
  72 members u64, **80 max_w u64, 88 max_h u64**.
  - seq u32는 [[vfs-build-perf]]의 이연 지뢰("(cell,layer) 65535
    페이지 초과 시 panic") 흡수. `page_cell_name`은 문자열이라 기존
    값 범위에서 이름 불변.
  - max u64는 포화 오컷 제거: 현행 빌더는 `clamp(0, u32::MAX)`
    (vfs.rs:1015)로 저장하고 planner가 일반값으로 비교 → 포화
    상황(극단 dbu·거대 지오메트리)에서 `max < cut` 오판으로 진짜
    페이지를 잘라낼 수 있다. v2는 clamp 제거, u64 그대로 저장,
    planner는 u64로 비교. (pbvh 집계도 동일 — 아래.)
- **prange 레코드 16B**(섹션 7, §2.2의 (cell,layer) run):
  0 layer_idx u32, 4 page_lo u32, 8 page_count u32,
  12 pbvh_root u32(`0xFFFF_FFFF` = 루트 없음 → 선형 순회).
  셀의 run들은 `cell.prange_start .. +prange_count`로 연속.
- **pbvh 노드 56B**(섹션 8):
  0 bbox 4×i64(32B), 32 first u32, 36 count u16,
  38 leaf u8, 39 reserved u8, **40 max_w u64, 48 max_h u64**
  (서브트리 집계 — u64, 페이지 레코드와 동일 사유). **리프의
  first/count = 페이지 디렉터리 인덱스 연속 구간**(빌드가
  (cell,layer) run 안에서 페이지를 리프 순서로 재배열 — §2.2);
  내부 노드의 first/count = 자식 노드 구간(인스턴스 BVH와 동일
  규약).
- **pts pool 엔트리 v2**(places 섹션 tail, §2.3의 구조 해결):
  `extent 4×i64(32B) | n_chunks u32 | reserved u32 |
  chunk bbox 4×i64 × n_chunks | 오프셋 (i64,i64) × count`.
  chunk = 256 오프셋, 오프셋은 **Morton 순 재배열**(§2.3 — 동률
  tie-break는 빌드-시 정렬에서만 원본 index 사용, 결정적). 저장
  후 **멤버 identity = slot index**(§2.3) — 원본 index는 wire에
  남기지 않는다(+4B/점의 공간 낭비, 용도 없음). place 레코드
  (kind=2, stride 불변)는 현행대로
  `na=count, va.0=pool off`가 이 엔트리를 가리킨다. 리더는
  `pts_extent(i)`/`pts_chunk(i,k)`/`pts_slice(i,range)` **zero-copy
  접근자**(borrowed bytes + LE 이터레이터, 캐스트 금지)를 제공
  (현행 place()의 Vec 복사 경로는 플래너에서 사용 금지).
- **캐시 무결성 = 운영 원칙 + 마커 규약**(2026-08-02 단순화·책임
  분리 지시). 운영 원칙 3조:
  1. 원본 파일이 변경되면 **캐시를 삭제하고 재빌드**한다.
  2. 캐시가 있고 뷰어가 보는 동안 **원본 수정/삭제 금지**(따라서
     "사용 중 재빌드" 시나리오는 정책 밖 — versioned OVP·GC·오픈
     핸들 보존 같은 코드 장치 불필요).
  3. 그래도 파일↔캐시 동기가 어긋나면 **운영으로 해결**(삭제·
     재빌드) — 코드는 어긋남을 명확한 에러로 보여주기만 한다.
  **책임 분리** — 인덱싱은 제한된 경로(시스템)를 통해서만 실행되고,
  역할은 이렇게 나뉜다:
  - **인덱서**: 시작 시 캐시 산출물 **전체 삭제**(최소한 마커 =
    `design.ovm`은 반드시 삭제) → 산출물 기록(`design.ovp` 등,
    파일명 현행 유지) → 종료 시 **마커 생성**(design.ovm을
    마지막에 기록 = 커밋, `ovp_len`@72 포함). **동시 빌드·경합에
    대한 책임은 없다** — 같은 캐시를 두 인덱서가 동시에 빌드하지
    않는 것은 호출 경로(시스템)의 책임이다.
  - **뷰어**: 인덱싱하지 않는다(0.4.6부터 — 캐시 없으면 "no VFS
    cache; run: floe-index vfs" 에러). 하는 일은 **정상 생성 확인
    뿐**: 마커 존재 + 버전/오픈 구조 검증 + `ovp_len` 정합. 실패는
    "no cache" 또는 "corrupt cache; rebuild".
  어느 지점에서 인덱서가 중단돼도: 마커 없음 → "no cache" / 마커
  부분 기록 → 구조 검증 실패 → "corrupt cache" — **중단된 빌드가
  유효한 캐시처럼 보이는 경우는 없다**. 전원 장애 등 극단 케이스의
  잔여 위험은 운영 원칙 3이 커버한다(fsync 강화·버전드 파일명은
  도입하지 않는다).
- **checked 변환 규칙**(빌드 전 구간): usize/u64/i64 → u32/u16
  narrowing은 전부 checked — `na`/`nb`, Pts count, page/place/bvh/
  prange의 start·count, 페이지 payload size(csize/usize), name
  off/len, seq 등. 초과 시 silent truncation이 아니라 **"limit
  exceeded: <필드>" 빌드 하드 에러**(리더 측 u64 산술은 §의 오픈
  검증처럼 checked). 근거: 현행 빌드에 `payload.len() as u32`,
  `jobs_list.len() as u32` 류의 무검사 캐스트가 산재 — 대형
  파일에서 조용한 잘림 가능.
- **오픈 검증**(v2 리더) — 섹션 유형별로:
  - 고정 stride 섹션(layers/cells/inst-bvh/pagedir/pranges/pbvh):
    길이 나누어떨어짐 + 레코드 수 정합.
  - places: `len ≥ n_places × PLACE_LEN`(잔여 = 가변 pts pool),
    pts pool off/count·chunk 수 경계.
  - bitsets: **`bs_width > 0` 선검증** 후 `len % bs_width == 0`
    (corrupt 헤더의 0-나눗셈 panic 방지). strings: 가변(오프셋
    경계만).
  - 인덱스 경계: top < n_cells, cell의 place/page/bvh/prange 구간,
    prange의 page 구간·pbvh_root, pbvh first/count(리프=페이지·
    내부=노드).
  - 페이지별 **checked** `file_off + csize ≤ ovp_len`(오버플로
    포함).
  - **헤더/섹션 산술도 checked**: 섹션 `off + len ≤ 파일 크기`
    (v1은 무검사 u64 덧셈이라 wrap 가능), `count × stride ≤ 섹션
    len` — 전부 checked_add/checked_mul.
  실패는 전부 "corrupt cache; rebuild" 에러.
- **reserved 규칙**: 쓰기는 0, 읽기는 무시. 향후 확장은 version
  범프로만(필드 재해석 금지).
- **영향 범위(정정)**: `.ovm` 파서는 rust `floe-ovm` 크레이트 +
  **`tools/validate_vfs.py`**(read_ovm이 version/스트라이드를 직접
  파싱 — v2로 갱신, M1 범위). 런타임 python 패키지는 .ovm을
  파싱하지 않는다(경로 참조뿐). `.ovp`(페이지 페이로드)·`.ovc`·
  meta.json 불변.
- **마이그레이션** = 캐시 재빌드 1회. 폐쇄망 절차는 원래 "새 바이너리
  반입 + 현장 인덱싱"이므로 추가 절차 없음(240G급 실측: parse 279s +
  병렬 build, 하트비트 있음 — [[vfs-build-perf]]).
- 검증: v1↔v2는 바이트 비교가 불가하므로 **의미 동등**으로 —
  validate_rust.sh 전체 스위트(scan/tiles/depth/meta/skel + vfs
  render XOR + coverage)를 v2 재빌드로 통과시키는 것을 게이트로.

### 3.7 세션 트랜잭션 (ack-gen 2단계 커밋)

**현행 잠복 버그**(flat 경로에도 이미 존재): 데몬은 응답을 만드는
순간 페이지를 resident로 등록하고(`Session::apply`, lib.rs:563-),
python은 요청이 stale이면 델타를 **적용하지 않고 버린다**
(service.py:362 `if newer(): return`). 그 결과 —
```
gen 1: 데몬 resident 등록 → 클라이언트 stale drop (레이아웃 미적용)
gen 2: 데몬 "이미 resident" → 바디 재전송 안 함
       → 레이아웃에 없는 페이지 셀 참조 = 조용한 지오메트리 공백
```
페이지는 예산 압박으로 evict될 때까지 재전송되지 않으므로 공백이
**영구**다. V4(WC가 이름으로 참조)에서는 ghost 셀로 더 표나게
깨진다. 해소:

- **2단계 커밋**: 응답의 `new`/`evict`는 **pending(gen)**으로만
  기록. 다음 요청의 `ack=<gen>`(§3.5)이 커밋 신호다 —
  `ack == pending.gen`이면 pending을 resident에 커밋,
  `ack < pending.gen`이면 **롤백**(new는 resident 취소 → 다음 플랜
  에서 다시 new로, evict는 resident 복원 → 레이아웃과 일치 유지).
  요청은 직렬이므로 pending은 항상 최대 1개.
- **상태기계 규칙**(모호 케이스 고정):
  - 최초 요청은 `ack=0`(gen은 1부터; 0 = "적용한 것 없음").
  - pending 없음 + ack 수신 = **no-op**(멱등 — 커밋 직후 재전송
    허용).
  - `ack > pending.gen` 또는 요청 `gen`의 역행/중복 = **프로토콜
    에러**(`error=` 응답; 클라이언트는 아래 복구 절차로).
  - **예산 판정은 projected 기준**: 요청 처리 순서가 "ack 해소 →
    플랜"이므로 플랜 시점의 pending은 이번 응답분뿐이다. eviction
    선택은 `projected = committed + 이번 new − 이번 evict`가 예산
    이하가 될 때까지(committed만 보면 예: committed 90MB/예산
    100MB/new 30MB에서 evict 없이 실레이아웃 120MB가 된다 —
    v1 apply의 "삽입 후 초과분 evict" 동작을 2단계에서도 계승).
    **LRU last-used 갱신도 pending** — 롤백 시 touch까지 되돌린다.
  - 보고 지표: `resident_committed_mb`, `resident_projected_mb`,
    `pending_new_mb`, `pending_evict_mb`(§3.5 응답 필드 —
    `resident_mb` 단일 필드 대체).
- **decoded 캐시와 layout-resident의 분리**: 델타 파일 생성(디스크
  IO)은 롤백과 무관 — 데몬은 같은 페이지를 다시 잘라 보내면 된다
  (페이지는 불변). 트랜잭션 대상은 "레이아웃에 무엇이 있는가"라는
  장부뿐이다.
- **apply 순서**(M3, 클라이언트): ① 새 델타 read → ② 새 top WC를
  FLOE_WS에 연결 → ③ 이전 gen WC들 shallow `delete_cells` →
  ④ evict 페이지 prune → ⑤ 다음 요청에 `ack=<gen>`. stale 판정 시
  ①~⑤ 전부 생략(레이아웃 불변, ack 미전송 → 데몬 롤백).
- **apply 부분 실패 복구**: ①~④는 원자적이지 않다 — 중간 예외 시
  레이아웃은 이미 부분 변경이라 "ack 안 보내고 데몬만 롤백"으로는
  불일치가 남는다. 규칙: **부분 적용된 레이아웃으로 다음 gen을
  진행하지 않는다** — 예외 즉시 (a) VfsMosaic 레이아웃 폐기·전체
  재생성, (b) 다음 요청에 **`reset=1`**(§3.5) — 데몬은 pending 폐기
  + resident 장부 전체 초기화(다음 플랜은 전 페이지 new), (c) 이후
  정상 사이클 재개. **실패한 gen 번호는 재사용하지 않는다** —
  reset 요청부터 새(더 큰) gen으로 계속(단조 증가 불변 유지).
  페이지는 불변이라 재전송이 곧 복구다. fault-injection으로 ①~④
  각 단계 실패를 게이트(§7).
- probe 모드는 세션-무관(fresh layout)이라 트랜잭션 밖.

---

## 4. 정확성: cut / coverage / labels

- **cut(디테일 컷)**: **셀 단위** 분류(§2.4). below-cut 셀은 WC와
  FRAME_LAYER proxy 없이 생략한다. all-layer `rbbox`를 단일 visible-layer
  proxy로 사용하면 무관한 다이 폭 outline/배열 stripe가 생기기 때문이다.
- **페이지 컷**: `max_w/max_h < cut` 페이지 skip, coverage가 채움
  (현행).
- **coverage 합성**: 서비스 측 numpy 팔레트 합성(현행 유지). 핸드오프
  게이트 동일.
- **skeleton 라벨**: 라이브 뷰 라벨은 skeleton에서 재사용·declutter
  (현행 유지). WC 계층과 독립(FLOE_WS 밑 별도 라벨 셀). 페이지는
  text-free 유지.

---

## 5. 마일스톤

0. **M0 — klayout 실증 스파이크** (M1과 병행, **M2 진입 게이트**):
   - 바인딩: editable=False Layout에서 (a) 파일-미정의 상주 셀 이름
     참조가 기존 셀에 바인딩되는지, (b) shallow `delete_cells`가
     자식(페이지)을 남기는지, (c) 여러 gen 연속 read/delete 후
     누수·잔존이 없는지 확증. 실패 시 §3.3 폴백(hier.tsv)으로 M2를
     재설계.
   - **Pts 재료화**(§2.3 방출 사다리의 임계 튜닝 입력): 입력
     type-10 크기 **2 / 1024 / 1025 / 10만 / 100만**(1-점 type-10
     은 스펙상 불가 — writer가 `len−2`를 기록, write.rs:98) ×
     viewport **선택 결과 0 / 1 / ≥2** 매트릭스. 선택 1개는
     `Rep::One` + 원점 이동으로 방출되는지, ≥2는 rebase(원점 이동
     + [0, pᵢ−p0])의 위치 정확성을 XOR로 확인. `Layout.read`
     시간/RSS, `each_inst()` 인스턴스 레코드 수(전개 여부 확정),
     draw 시간, pya 폴백의 확장량 측정. klayout이 전개한다면 full
     rep 임계(1024)를 낮추거나 가시-부분집합을 소형 Pts까지 확대.
   - **결과 (완료 2026-08-02, klayout 0.30.9/pip, macOS)** — 재현:
     `cargo build --release -p floe-oasis --example m0_gen`,
     `m0_gen pts data/m0` / `m0_gen gens data/m0 30`, 이후
     `tools/m0/binding_spike.py data/m0` /
     `tools/m0/pts_case.py data/m0 <N>` /
     `tools/m0/pts_fallback.py <k>` (.venv python).
     - **바인딩 GREEN**: plain read(LoadLayoutOptions 불요), 30 gen
       — 변형/유령 0, 지오메트리 XOR 0(rot·Grid·Pts rep 경유 포함),
       shallow delete 후 페이지 전원 생존(고아 포함), live-cell
       장부 일치, RSS 61.1→61.3MB 평탄. 이 규모에서 read
       0.1~0.5ms, `delete_cells` 0.01~0.03ms. cell index 슬롯 단조
       증가 관찰(§3.1 구현 노트). hier.tsv 폴백 불사용 판정.
     - **Pts 재료화** (전 케이스 리베이스 XOR green, 비전개 확정):

       | 입력 N | read s | ΔRSS MB | inst 레코드 | draw wide / narrow s |
       |---|---|---|---|---|
       | 2 | ~0 | ~0 | 1 | .008 / .008 |
       | 1,024 | ~0 | ~0 | 1 | .006 / .007 |
       | 1,025 | ~0 | ~0 | 1 | .006 / .007 |
       | 100,000 | .014 | 3.2 | 1 | .031 / .008 |
       | 1,000,000 | .146 | 25.3 | 1 | .078 / **.008** |

       좁은-뷰 draw가 N 무관 상수 = klayout이 iterated array를 뷰
       클립. per-점 단가: read ≈0.15µs, RSS ≈26B, wire ≈6.1B
       (deflate 후). 선택 0/1/≥2 리베이스(§2.3)가 모든 크기에서
       full 파일 지오메트리와 XOR 일치 — `Rep::One`+원점 이동(1개),
       원점+=p0/[0,pᵢ−p0](≥2), 생략(0개) 모두 실증. pya 폴백(개별
       삽입): ~55만 인스턴스/s, 100만 = 1.8s + 47MB — 네이티브
       read 대비 12×/1.9×(flat 경로 6.3M 삽입 ≈ 11.5s와 정합,
       §1.1 load 병목의 정량 재확인).
     - **판정**: M2 진입 게이트 통과. full-rep 임계 1024 →
       **8192**(§2.3), 사다리 구조는 유지(바운드 대상 = 델타
       바이트·스캔 CPU), hier.tsv 폴백 dormant. 주의: 실측은 pip
       klayout 0.30.9 — 배포 번들(conda-forge) 계열은 M4 스위트가
       재확인.
1. **M1 — `.ovm` v2 + 계층 플랜**:
   - v2(§3.6): 빌드에 topo_rank(사이클 = 하드 에러)·height u32
     checked·prange/pbvh 섹션·seq u32·max u64(clamp 제거)·pts pool
     extent/Morton-chunk·**마커 규약**(시작 시 전체 삭제, ovm-
     마지막 커밋)·**checked 변환 전면 적용**(silent truncation
     제거), 리더 필드 확장 + zero-copy pts 접근자 + 섹션-유형별
     오픈 검증, `tools/validate_vfs.py` v2 갱신(stride/필드 —
     파일명은 불변), validate_rust.sh 스위트 v2 재빌드 통과.
   - 플랜: `floe_vfs::plan/descend`를 flat `(pages, mats)` →
     **WC 계층** 산출로 재작성. 산출물: `Vec<WsCell{ key: (ci, r),
     pages:Vec<u32>, insts:Vec<WsInst{ child_key, local_xf, rep }>,
     frames:Vec<(BBox, Rep)> }>`. 내용: rank 1패스 전파 + K-box
     localview + skew-Grid 닫힌형 + Pts 임계/폴백(§2.3), 셀 단위
     cut 분류(§2.4), depth variant + height 접기(§2.5), 인스턴스/
     페이지 BVH 프루닝(§2.2). **flat 경로는 mode 플래그로 병존**
     (A/B).
   - **결과 (구현 완료 2026-08-02, 0.5.0)**: floe-ovm v2(빌더/
     리더/섹션-유형별 오픈 검증/zero-copy pts 접근자/checked
     narrow), 빌드(topo_rank + 사이클 하드 에러, height u32
     checked, prange + pbvh(리프 ≤8, run 내 리프-순 재배열), Pts
     Morton 전처리(병렬 단계), seq u32, max u64 clamp 제거, 마커
     규약, checked 전면), 계층 플래너 `rust/vfs/src/hier.rs`
     (rank min-heap 1패스·K-box·Grid 닫힌형·Pts 사다리·depth
     variant/height 접기·프레임 rect+rep), `plan --mode hier|flat`,
     `tools/validate_vfs.py` v2(+`ovp_len` 정합 게이트),
     `Vfs::open`의 ovm↔ovp 쌍 검증. **게이트**: 유닛 15종
     (브루트포스 참조 플래너 무누락/동일성, Grid 닫힌형 프로퍼티
     (음수 벡터·det=0·±1 타이트), Pts rebase 0/1/≥2·slot dedup·
     예산 폴백 무누락, pbvh=linear 동일성, K-merge 결정성, depth
     fixture, 회전/미러) green, `--jobs` 1 vs 8 **ovp/ovm 바이트
     동일**, 마커 3상태(no cache / corrupt / pair mismatch) + 구
     버전 문구, validate_rust.sh 전체 스위트 v2 통과. **valmini
     A/B**: 동일 페이지 집합 뷰에서 placements 54→11·27→9·78→12
     (inst_edges), **배열-관통 좁은 뷰는 flat 6페이지/26배치 →
     hier 1페이지/2에지**(`view=cwb` 병리 소거 실증). MAIN09 실측
     A/B는 M4(사무실 자산).
2. **M2 — 계층 델타 + 트랜잭션**: WC 계층을 OASIS 하나로(§3.2).
   `placements=`/`frames=` 폐기, `top=`/`names=`/`ack=` 추가(§3.5),
   `Session` 2단계 커밋(pending/commit/rollback, §3.7).
   - **결과 (구현 완료 2026-08-02, 0.6.0)**: `delta_hier`(단일-top,
     authored+스플라이스, 증분 ovp 무-IO), `HierSession`(pending =
     순수 diff — 롤백 = 폐기), vfsd `mode=hier|hier_probe` +
     `ack=/reset=/names=` + 상주 4지표 + 세션-모드 잠금. 게이트:
     트랜잭션 유닛 3(스테일-드롭 재전송 회귀 포함), 델타 라운드트립
     2(parse_doc 단일-top·프레임 rep·rebased Pts·바이트 결정성),
     `validate_vfs_hier.py` **H1~H5 실데몬**(증분 이름-바인딩 +
     shallow delete, 롤백 재전송, dup-gen/reset, **probe cut=0 =
     소스 XOR 일치**) — validate_rust.sh 편입. flat/파이썬 무변경.
3. **M3 — 계층 apply**: `VfsMosaic`에 gen-ephemeral WC 수명(§3.1:
   `delete_cells` 일괄 shallow, 페이지 잔존/evict 유지, cell-index
   remap 계승) + §3.7 apply 순서(①~⑤, stale 시 전부 생략 + ack
   미전송) + pick의 ci→이름 테이블(§3.4).
   - **결과 (구현 완료 2026-08-03, 기본 모드 = hier)**:
     `apply_hier`/`load_names`/`_WsNames`/`reset_all`(viewport.py),
     vfsclient `hier/ack/reset` + `top=/names=` 파싱, service hier
     분기(전용 `req_gen`, stale=ack 미전송, 예외→reset 재요청 1회),
     probe/clip/cli region은 `hier_probe`. `FLOE_VFS_MODE=flat`로
     A/B(M5에서 제거). §2.1 **프레임 융합 규칙**은 이 단계 실측이
     추가(전면 워시 회귀 → footprint 강등, 수정 후 hier/flat 서비스
     렌더 PNG 바이트 동일). 게이트: render 6뷰(관통 뷰 2종 추가) ×
     양 모드, lifecycle L1~L4(팬 XOR·WC 잔존 0·stale 재전송·부분
     실패 복구·evict churn), `floe probe` 실서비스 — 전부 green,
     스위트 편입.
3.5. **M3.5 — 점진 첫 페인트 (구현 완료 2026-08-03, 0.6.2)**:
   첫-방문 병목은 klayout 단일-스레드 파싱뿐임을 실측(§0 rev 17)
   — 총량 대신 체감을 쪼갠다. 데몬이 라운드당 `--stream-kb`(기본
   24MB)만큼만 신규 페이지를 **뷰 중심 거리순**으로 싣고
   `partial=1`로 응답, 뷰어는 라운드마다 렌더+프레임 송출 후 재요청
   (ack 트랜잭션 위 정상 사이클 — 중도 이탈=롤백, 상태 추가 0).
   testchip 189페이지/124MB 뷰: 첫 페인트 1.08s, 6라운드 총 7.2s.
4. **M4 — 검증**: §7 게이트 전부 + MAIN09 실측(inst_edges/pages/
   load_ms 자릿수 감소 확인) + flat vs 계층 A/B 수치.
   (**완료 — 결과·표·판정은 §0 rev 19**: 스케일 결함 4건 수정
   포함, 최대 발견 = rep 페이지-bbox 바닥 → ovm v3.)
   - **성격 규정**: M4는 "수많은 실칩의 대표 표본"이 아니라
     **알려진 최악 사례 회귀 + 기본값 결정**이다. 정확성·상한은
     구조(가시 서브트리 비례, 무누락 폴백, 예산 바운드, checked
     하드 에러)가 보증하고, 실측은 상수만 정한다 — 틀린 상수는
     느려질 뿐 깨지지 않는다.
   - **자산별 역할**: MAIN09 = 중첩-배열 딥줌 회귀(원병리),
     testchip_1g5 = Pts·mid-zoom 밀집 + 점진 로딩 체감, 240G =
     빌드 스케일, 9.8G b3 = fill 홍수, valmini = 합성 게이트.
   - **측정 항목**: 대역별(wide/mid/deep) cold 분해(델타 MB·
     ly.read s·첫 페인트·완성 시간)·RSS·K=4 vs K=1, flat 대조.
   - **LOD 판정 기준**: 점진 로딩 상태에서 mid-zoom cold **완성**
     시간이 실칩 기준 3~5s 초과가 상습이면 M7 착수, 아니면 보류.
   - **기본값 ≠ 한계**: M4가 정하는 것은 `--stream-kb`·K·PTS 임계
     등의 **기본값**이며 전부 노브로 남긴다(현장 이탈 사례는 플래그
     조정). 미지의 칩은 상태줄 한 줄(tiles/load/draw/cut/members)
     + §3.5 지표로 회수해 다음 fixture로 삼는다 — M4는 이 운영
     루프의 시작점이지 종점이 아니다.
5. **M5 — 통합·폐기**: coverage/skeleton 라벨 재확인(§4), flat 경로
   + mats-TSV + frames 파일 + `mode=` 스위치 제거. (여기의 "버전
   범프"는 push 관례에 따른 **CLI/크레이트 릴리스 버전** — 캐시
   포맷은 M1에서 이미 v2로 끝났고 M5는 포맷을 건드리지 않는다.)
   (**완료 — 0.8.0, §0 rev 20**.)
6. **M6 — 증분(팬 최적화, 후속)**: WC를 gen-ephemeral에서 "상주 +
   localview 차분 갱신"으로 승격. 팬은 작은 델타, 줌은 부분 재구성.
   (M1~M5 동안은 프레임당 WC 재구성 — 깊은 줌은 바운드되어 싸다.)
7. **M7 — LOD 페이지 (예약; M5 이후, M4 판정 기준 충족 시만)**:
   페이지 레코드의 `lod`/`codec` 예약 바이트를 사용한 간략화 LOD
   변종 — 남은 병목(파싱량 ∝ 델타 바이트)의 근본 개선. 착수 전
   결정 필요 사항: 간략화 방식(밴드식 데시메이션/fill 병합), LOD
   선택 규칙(줌 대역), 캐시 증분 비용, 손실-렌더 검증 기준(XOR
   게이트의 LOD-인지화). coverage(광역)와 점진 로딩(체감)이 이미
   흡수한 몫을 제외한 잔여 가치를 M4 수치로 판정한다.

---

## 6. 리스크 / 미해결

0. (해소 — rev 28, T4) **뷰어측 텍스트 수집의 경로 전개 상주**:
   collect_all_texts 기반 사이드카가 VFS 빌드에서 통째로
   제거되었다 — 텍스트는 셀-로컬로 1회만 인덱싱(ovm v5 +
   design.ovt)되고 라벨은 요청 시 플래너가 뽑는다. 빌드 텍스트
   단계의 비용/메모리는 이제 **소스 레코드 수에 비례**하며 경로
   전개 자체가 존재하지 않는다(9.8G 재인덱싱의 "[vfs] build:
   text index …" 라인으로 실측 확인 예정). 원 계획이던 외부 정렬
   스풀은 불필요해져 철회.

1. (해소 — M0 GREEN) **klayout read 이름-바인딩**(§3.3): 옵션 없는
   plain read로 확증(klayout 0.30.9, 30 gen 누수 0, §5 M0 결과).
   폴백(hier.tsv + pya API)은 불사용 — 명세만 §3.3에 보존.
2. **localview 합집합의 이산 병리**: 같은 셀이 화면 안 멀리 떨어진
   두 곳에 보이면 단일-bbox 합집합은 그 사이 전부를 덮어, 중간
   줌에서 페이지 과다 로드가 flat(경로별 정확한 lview)보다 나빠질
   수 있다. 대응: **K-box localview를 M1 기본 탑재**(K=4, §2.3) —
   잔여 리스크는 K 초과 병합 시의 과다 포함 정도로, M4 중간-줌
   A/B(K=1 대조)로 계측.
3. **부분 배열 정확성**: skew-Grid 역행렬/가시범위의 오프바이원 =
   배열 멤버 **누락**(정확성 회귀). §7의 브루트포스 유닛테스트 +
   배열 관통 소형 뷰 XOR 필수.
4. **depth variant 증식**: 유한 d에서 셀당 최대 d+1개 — 실사용 d는
   작고(첫 페인트 1), **`r ≥ height(ci)` → sentinel 접기**(§2.5,
   height는 v1 필드)로 깊은 서브트리 밖 증식이 구조적으로 잘린다.
5. **v2 마이그레이션**: 기존 `.floe` 캐시 전부 재빌드 필요(1회).
   버전 게이트 에러는 명확하지만, 사무실 대형 자산은 재빌드 시간
   (parse 279s + build)을 계획에 반영해야 한다.
6. (해소 — M0) **klayout의 type-10 Pts 전개**: 전개하지 않음을 실증
   — 1M 점 = 레코드 1, read 0.146s, RSS +25MB, 좁은-뷰 draw N 무관
   상수(§5 M0). 사다리는 델타 바이트·스캔 CPU 바운드로 존속,
   full-rep 임계는 8192로 상향(§2.3, M4 A/B 확정).
7. (해소 — 기록만) 모달 정합 → 구조 보장(§3.2). cut 에지-단위 →
   셀 단위 속성(§2.4). 전파 수렴·9.8G plan 시간 → rank 1패스 +
   빌드 사이클 거부(§2.3). min-depth 의미 변경 → depth variant로
   철회(§2.5). 포맷-유지 타협(runtime BVH·기동 시 rank 계산·seq
   u16) → `.ovm` v2로 승격(§3.6). **stale-drop 영구 공백**(현행
   flat 경로에도 잠복하던 실버그) → ack-gen 트랜잭션(§3.7).
   max_w/max_h u32 포화 오컷 → u64(§3.6). 빌드 중단 시 캐시 파괴
   (제자리 truncate) → **ovm-마지막 커밋 + 운영 원칙**(rev 9에서
   versioned OVP 철회, §3.6). Pts 부분집합 오배치 위험 → rebase
   규칙 + slot dedup(§2.3). 대형 Pts extent 일률 기여의 view=cwb
   재현 위험 → O_vis 사다리(§2.3). 사용-중 재빌드/원본 변경 동기화
   → 운영 원칙 3조(§3.6).

---

## 7. 검증 게이트

- **유닛(신규)**:
  - skew-Grid 가시 인덱스/닫힌형 기여 vs **브루트포스 대조** 프로퍼티
    테스트 — 경계 ±1, 음수 벡터, `det=0`(nb=1·공선), 큰 좌표
    (i128 오버플로 경로), na 또는 nb=1. (현재 vfs 크레이트
    유닛테스트는 splice_roundtrip 1개뿐.)
  - authored WC + 스플라이스 혼합 델타 라운드트립(parse_doc +
    klayout 로드).
  - **depth fixture**: 같은 셀이 서로 다른 경로 깊이로 도달하는
    구성에서 d별 스냅샷 = 현행 flat plan-side 절단과 일치.
  - 같은 페이지가 여러 WC variant에서 참조되는 케이스.
  - **Pts**: 방출 사다리 4단(full rep / 가시-부분집합 /
    chunk-통짜 / 예산 초과 full 폴백) 전부에서 멤버 누락 0
    (브루트포스 대조), 상위 단은 과다 포함만 함을 확인. **rebase
    위치 정확성**(0/1/≥2 케이스, 원점 이동 + [0, pᵢ−p0]) XOR.
    **slot-index dedup**(§2.3): 동일 좌표의 별개 slot 멤버가 K-box
    합집합 후에도 보존(멤버-수 대조 = validate_vfs G5와 동일 기준). **O_vis
    사다리**: 선택 bbox ⊆ 포함-chunk bbox 합 ⊆ extent 관계와 각
    단이 실제로 그 단의 bbox를 쓰는지 확인. extent/chunk 인덱스 vs
    전 점 스캔 대조. PTS_ENUM_BUDGET 소진 경로 강제 통과.
  - **결정성**: 동일 요청 2회 → 플랜/델타 산출 바이트 동일(K-box
    병합 tie-break 포함, §2.3).
- **수명주기**: 여러 gen 연속 apply 후 이전 WC 셀 잔존 0; 페이지
  eviction 후 현재 WC에 dangling 참조 없음(§3.1 구조 보장의 실측
  확인); **stale drop 케이스** — 응답 생성 후 apply 없이 취소(ack
  미전송) → 데몬 롤백 → 다음 프레임에서 해당 페이지 재전송, 공백
  0 (§3.7; 현행 잠복 버그의 회귀 테스트); **fault-injection** —
  apply ①~④ 각 단계에 예외 주입 → 모자이크 재생성 + `reset=1` →
  다음 프레임 정상 렌더(공백 0, §3.7 복구 절차).
- **마커 규약**(§3.6): 빌드를 마커 삭제 후 / ovp 기록 중 / 마커
  기록 중 각 지점에서 강제 종료 → 오픈이 항상 "no cache"(마커
  없음) 또는 "corrupt cache; rebuild"(마커 부분 기록 = 구조 검증
  실패) — **중단된 빌드가 유효한 캐시로 보이는 경우 없음**. (동시
  빌드·사용-중 재빌드·전원 장애는 운영/호출-경로 책임 — 게이트
  없음.)
- **checked 변환**(§3.6): Builder 단위 테스트로 각 narrowing
  경계에 초과값 주입(u32 count 초과, payload > u32 등 실물로 못
  만드는 경계 포함) → "limit exceeded: <필드>" 에러 확인 — silent
  truncation 경로 0. (seq u16 초과 → u32는 §3.6에서 흡수됨;
  스트레스 케이스는 기존 65535-페이지 게이트 유지.)
- **바이트 동일성(0.4.7 게이트 계승)**: 같은 입력에서 `--jobs`
  무관 동일 출력 — ovp/ovm 모두 **바이트 그대로 비교**(rev 9에서
  build_id 철회로 정규화 불필요). Morton/페이지 재배열/pbvh 노드
  순서가 jobs 수와 무관하게 결정적임을 이 게이트가 함께 증명.
- **valmini XOR**: 중첩 배열 케이스는 **이미 있음**(TOP→MID 배열→
  LEAF1 3×3이 fold_array 전개 경로를 통과) — 보강할 것은 **뷰**다:
  `validate_vfs_render.py`에 배열 내부를 가로지르는 소형 깊은-줌 뷰,
  배열 경계 걸침 뷰 추가(현행 whole/quarter/corner-half는 부분-배열
  을 못 잡음). 필요 시 큰 na·nb 케이스 추가.
- **스케일 불변**: Grid na·nb를 크게 키워도(멤버 수 ↑) planner
  시간이 멤버 수와 무관함을 계측(§2.3 닫힌형의 회귀 방지).
- **pick**: 공백·유니코드 design명 셀에서 ci→이름 테이블 경유 표시
  확인(§3.4).
- **M0 게이트**: §3.3 바인딩 + shallow `delete_cells` + 다중 gen
  누수 확증 스파이크 — **완료(GREEN)**, 결과표 §5 M0.
- **v2 포맷 게이트**(§3.6): v2 재빌드로 validate_rust.sh 전체
  스위트 통과(v1↔v2 의미 동등). 구 바이너리+새 캐시 / 새 바이너리+
  구 캐시 교차 실행 시 버전 에러 문구(재빌드 안내) 확인. 합성
  스트레스로 (cell,layer) 페이지 65535 초과 케이스의 seq u32 경로
  확인.
- **A/B(실자산)**: MAIN09 등에서 flat vs 계층 —
  pages/inst_edges/plan_ms/load_ms/draw_ms + page_candidates,
  그리고 K-box K=4 vs K=1(중간 줌 페이지 수). 지표 의미 변경(§3.5)
  을 표에 병기해 사과-사과 비교 유지.
- flat 경로 유지 기간: 계층이 valmini XOR green + MAIN09 자릿수 개선
  확인까지. 이후 M5에서 제거.
