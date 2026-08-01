# floe VFS 아키텍처 (V1: 포맷 + 인덱서 + plan 시뮬레이터)

2026-08-01 결정. 배경: 9.8G 실측에서 klayout을 런타임 저장소로 쓰는
구조의 바닥 확인 — 4타일 뷰 128s(load 94.3s = 밴드 파일 db::Layout
파스 ~6MB/s, draw 22.3s = 8M/s), 로드된 모자이크 걷기 바닥(1레이어
2,278도형에 draw 2.6s). floe 미출시이므로 .ice 밴드 타일 구조는
브리지 없이 이 구조로 대체한다. 기준 설계 문서:
`oasis_index_and_viewer.md` (사용자 제공), 결정 기록:
세션 메모리 `vfs-rewrite-direction`.

## 전체 그림

```
OASIS ──▶ floe-index vfs ──▶ <src>.floe/
                               design.ovm   mmap 메타 (셀/배치/BVH/페이지 디렉토리)
                               design.ovp   독립 압축 페이지 (셀-로컬 exact 지오메트리)
                               skeleton.oas, texts.tsv   (당분간 기존 그대로)

뷰어(V2+):  plan_view(뷰포트) ─▶ 교차 페이지만 디코드 ─▶ klayout working set
            (klayout은 bounded working set의 exact 렌더러로만)
```

- **V1**(이 문서의 범위): 포맷 + 인덱서 산출 + `plan` 시뮬레이터 —
  지오메트리를 로드하지 않고 "화면당 페이지 수/바이트"를 검증.
- V2: oasis-vfs 런타임 + PyO3(abi3) + 뷰어 working-set 렌더.
- V3: proxy(실루엣/헤어라인) + coverage 오버레이(GTK 픽스버프 합성).
- V4: 계측 기반 경계 조정. 스트리밍 2-pass 인덱서(파스 메모리)는
  별도 마일스톤.

## 1급 제약

- **결정론**: 수렴 후 화면 = (뷰포트, 배율, 레이어, depth, cut)의
  순수 함수. 예산/우선순위는 정련 "순서"에만 관여한다.
- **사용자 depth 의미 불변**: materialization 정책은 컷 레벨의
  일반화이며 depth 값을 자동으로 바꾸지 않는다.
- **rep 무확장**: 어떤 단계도 repetition을 펼쳐 저장하지 않는다.
- 순수 Rust / vendored deps / C 툴체인 불요 원칙 유지.

## 구조 원칙 (기존 .ice와의 차이)

1. **페이지는 셀-로컬 좌표, 클립 없음.** 타일 절단·변형 셀($n)·경계
   복제·subset-Pts 조각화(9.8G b3에서 소스 rep의 ~100배 레코드 증식
   실측)가 인덱스에서 사라진다. 캐시 크기는 원본 오더에 근접할 것.
   인스턴스 경로별 변형·클립은 렌더 시(working set) 담당.
2. 큰 셀×레이어만 공간 분할: pre-comp 바이트가 목표(1MB)를 넘으면
   레코드 extent 중앙값 기준 이분할(축 교대, 결정론). rep은 클립하지
   않고 extent로 소속 페이지만 결정 — 페이지 bbox는 내용의 실제 bbox.
3. 페이지 레이어 축 = **레이어 단위**(D1). 콘텐츠가 있는
   (셀, 레이어)에만 페이지가 생긴다.

## 페이지 payload (D2: OASIS 재사용)

페이지 = **완전한 단일 셀 OASIS 파일** (`write_tree` 재사용, 셀 이름
"P", 내부 CBLOCK deflate-6, 조건부 rep-그룹 정렬 포함). 디코드 =
`parse_doc` 재사용, 검증 = klayout이 페이지를 직접 읽어 대조.
파일 외피(MAGIC/START/END) ~290B는 1MB 페이지에서 0.03% — 재사용성과
검증성의 대가로 수용. 디렉토리의 codec 바이트(D4)로 향후 포맷 교체
여지 확보 (0 = plain OASIS+내부 CBLOCK).

## design.ovm 스키마

원칙: little-endian 고정, 파일 포인터 금지(상대 offset/인덱스),
고정폭 레코드, 리더는 unsafe transmute 없이 bounds-checked 필드 접근
(&[u8]에서 LE 읽기 — mmap이든 read든 동일 API).

```
header (고정 폭)
  magic "FLOEOVM1", version u32, flags u32
  unit f64 (grid steps per micron)
  src_size u64, src_mtime u64          ← 원본 지문
  top_cell u32, n_layers u32, n_cells u32, n_places u64,
  n_pages u32, n_bvh u32
  섹션별 offset/len u64 쌍: strings, layers, cells, places,
  bitsets, bvh, pagedir

strings   이름 blob (레코드가 off u32 + len u16로 참조)

LayerRec  layer u32, dt u32, name_off u32, name_len u16, pad u16,
          records u64, members u64
          (파일 등장 순서 = 기존 layer_order 규칙, 팔레트 재현용)

CellRec   name_off u32, name_len u16, height u16,
          dbbox i64×4, rbbox i64×4          (direct/recursive,
                                             텍스트 앵커 포함)
          place_start u32, place_count u32,
          page_start u32, page_count u32,
          bvh_start u32, bvh_count u32,
          lmask_direct u32, lmask_rec u32,   (bitset pool 인덱스)
          rec_members u64

PlaceRec  child u32, rot u8, flip u8, rep_kind u8, pad u8,
          x i64, y i64,
          na u32, nb u32, vax i64, vay i64, vbx i64, vby i64
          (rep_kind 0=단일 1=grid 2=pts: na=개수, vax=pts pool off;
           pts pool은 bitsets 섹션 뒤에 병합 저장)

bitsets   ceil(n_layers/8)B 단위, 중복 제거 pool

BvhNode   bbox i64×4, first u32, count u16, flags u16
          (leaf: place_start+first부터 count개; 내부: 자식 노드
           first..first+count; 셀별 서브배열)

PageDir   cell u32, layer_idx u32, seq u16, lod u8, codec u8,
          bbox i64×4, file_off u64, csize u32, usize u32,
          records u32, pad u32, members u64,
          max_w u32, max_h u32
          (max_w/h = 페이지 내 최대 도형 치수 — page-read 방지
           컷 판단의 근거. (cell, layer_idx, seq) 정렬)
```

## plan 시뮬레이터

입력: 뷰포트(dbu), px_per_um, 가시 레이어, cut_px, depth.
순회(§19 정책): 셀 rbbox∩뷰포트 → recursive layer bitset 교차 →
projected 크기 < cut_px면 서브트리 통째 컬 → 인스턴스 BVH로 자식
배치 선택(배치 rep은 visible (i,j) 범위 산술, 펼치지 않음) → 현재
셀 페이지 중 로컬 뷰포트 교차 + max_feature ≥ cut 조건 통과분 수집.
페이지는 HashSet으로 유일화(디코드 1회) + materialization 횟수 별도
집계. 출력: 페이지 수/압축·해제 바이트, 방문 노드 수, 컬 통계
(size-cut 페이지/서브트리, 레이어 컬, bbox 컬).

## 합격 게이트

- G1 94s 사건 재현 뷰(200×165µm, 전 레이어, cut 2px): planned
  bytes ≤ ~30MB (해제 <1s 상당)
- G2 같은 뷰 1레이어: ≤ ~2MB
- G3 광역 fit: exact 페이지 ~0 (coverage는 V3 자리)
- G4 .ovm ≤ 원본 ~5%, .ovp ≈ 원본 크기 오더
- G5 페이지 round-trip: 페이지별 records/members == 디렉토리,
  (셀×레이어) 합 == 소스 scan 합 (무클립이라 정확 일치)
- G6 rbbox/카운트 klayout 대조 (기존 검증 방법론)

## V2 뷰어 통합 (진행 중)

vfsd 프로토콜(라인 kv): 요청 `gen= view=x0,y0,x1,y1(um) px= cut=
depth=full|N layers=all|a/b,.. out=` → 응답 `gen= pages= new=
evict=이름,.. delta=경로|- placements=경로(tsv) bytes= plan_ms=
resident_mb=`. `mode=probe`(예정) = 세션 무관 정밀(cut=0) 델타 —
픽/스냅/클립용.

뷰어 교체 지점 (조사 결과, 파일:라인은 2026-08-01 기준):
- Cache.load(cache.py:247): .floe/meta.json(빌더가 신설 생성) 분기
- Mosaic.ensure/_band_file(viewport.py:124/113): vfsd 델타
  multi-read + 배치 목록 기반 WS_TOP 재구성으로 대체 (@t 태그 불필요
  — 페이지 셀명 전역 유일; 트윈/밴드 키 소멸)
- _svc_render의 밴드 선택/트윈/점진 단계(service.py:296-397):
  vfsd가 대체 — newer()/latest 중단·코얼레싱은 유지
- load_region(cache.py:1029)/snap/pick(service.py:126/176):
  mode=probe 정밀 델타로
- 스켈레톤 far view(scope="skel" 분기): 그대로 — .floe에도
  skeleton.oas/texts.tsv를 빌더가 생성
- 뷰어가 요구하는 meta 필드: dbu, bbox, grid(합성 — live/skel 분기
  휴리스틱용), layers+color, src{size,mtime}, skeleton, vfs:1
- vfsd 스폰: RenderWorker/_render_service(service.py:583/503)의
  spawn 패턴에 subprocess.Popen 파이프로 편입

## V1에서 하지 않는 것

스트리밍 인덱서(in-memory Doc 유지, D3), proxy/coverage, VFS
런타임/PyO3, 뷰어 연결, skeleton/texts 변경, 기존 index/tile
서브커맨드 제거(검증 기준선으로 당분간 유지).
