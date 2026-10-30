# 원본 직접 읽기 + 경량 캐시 — 설계 검토 (2026-10-21)

사용자 요청(2026-10-21): "현재 jobdeck을 구성하는 레이아웃 용량이 테라바이트에
육박하고 있어서 인덱싱 용량과 시간이 너무 큰 문제가 있음. 이 경우 인덱싱하지
않고 원본 파일을 바로 읽어서 캡쳐하거나 뷰어로 볼 수 있도록 하는 방법을
검토해야 함. 원본을 직접 읽되, 속도를 개선할 수 있는 작은 용량과 빠른 시간에
인덱싱 가능한 캐시를 도입할 필요가 있음.(예전에 시도한 원본 파일을 랜덤
억세스할 수 있도록 미리 offset을 저장하는 방식등). 검토해줘."

이 문서는 검토 결과다. 코드는 없다. 구현 단계(§8)는 사용자 결정 뒤에 착수한다.
현장 파일 이름과 셀 이름은 적지 않는다.

## 0. 결론

1. **지금 캐시의 부피는 거의 전부 `design.ovp`(도형을 다시 인코딩한 페이지)
   이고, 시간은 거의 전부 그 페이지를 만드는 플랜·인코딩이다.** 마스크 소스
   (도형 위주, 배치 적음)에서 캐시 ≈ 소스 크기(실측 1.03×), 현장 50 GB 소스의
   색인 7,890 s(≈ 158 s/GB). 1 TB 덱이면 캐시 ≈ 1 TB, 시간 ≈ 44 시간이다.
   그런데 **광역 뷰(덱 캡처의 대부분)는 그 페이지를 한 장도 읽지 않는다** —
   요약 `design.ovo`가 대신 그린다(기준 셀 4 µm가 화면 1 px 이하인 배율,
   즉 ≥ 4 µm/px; 32 mm 창을 1,200 px로 찍는 캡처는 26.7 µm/px). 근접 뷰만
   페이지를 읽고, 그때 읽는 것은 뷰 안의 셀 몇 개의 일부다.
2. **"원본을 직접 읽는" 길은 이미 반쯤 놓여 있다.** 스트리밍 색인(S1~S6,
   0.12.363~0.12.370)이 만든 재고(inventory)가 바로 "셀마다 파일 오프셋을 미리
   저장하는 표"다: 셀별 바이트 범위 + 암시 참조번호 시드 + bbox + 레이어
   마스크 + 자식 간선 + 계층 fold, 레코드는 보관하지 않음. 셀 파서
   (`parse_cell_bytes`)는 그 범위 하나를 pread로 읽어 셀 하나를 만든다. 50 GB
   현장 파일의 재고 패스는 131.6 s(= 2.6 s/GB)였다. 지금은 빌드가 끝나면 버리는
   메모리 구조이고(사이드카는 STREAMING_INDEX_PLAN §10의 2번에서 보류), 렌더러가
   읽을 수 있는 형태가 아니다 — 그 둘이 이번 일의 본체다.
3. **셀 단위 오프셋만으로는 마스크에 부족하다.** 현장 마스크 소스는 셀 5,461개에
   레코드 19.7억, 가장 큰 셀 하나가 레코드 118만 = 파일 26 MiB, 파싱하면 Doc
   415 MiB다(scan의 `largest cell … 415M`은 Doc 추정치다 — 첫 검토는 이를 파일
   바이트로 읽어 셀 전체 파싱을 16 s로 잡았으나, L0 실측 뒤의 값은 단일 스레드
   1~2 s, 12 스레드 0.2~0.3 s다; §8.1). 근접 뷰 하나를 위해 그 셀을 통째로
   파싱하면 뷰에 무관하게 그만큼을 든다. 그래서 셀 안을
   **세그먼트**(CBLOCK 경계, 평균 64 KB; CBLOCK이 없으면 고정 크기 바이트 구간)로
   나눈 디렉터리 — 오프셋·길이·**모달 변수 스냅샷**·bbox·레이어 마스크·레코드
   수·max_w/max_h — 가 경량 캐시의 핵심이다. 스냅샷이 있어야 세그먼트를 독립적으로
   (그리고 병렬로) 파싱할 수 있고, bbox·레이어 마스크가 있어야 뷰 밖·안 보이는
   레이어의 세그먼트를 건너뛴다. 이것이 "랜덤 액세스 오프셋 표"의 완성형이다.
4. **제안: 경량 캐시 = `design.inv`(재고) + `design.seg`(세그먼트 디렉터리) +
   `design.ovo`(점유 요약).** 원본 읽기 2회(재고+세그먼트 1회, 요약 1회)로
   만들고, 크기는 소스의 0.1~0.3 %다. 렌더러는 이 셋으로 지금의 Ovm 자료구조를
   메모리에 합성해(셀 디렉터리 ← 재고, 페이지 디렉터리 ← 세그먼트, 배치·BVH는
   첫 방문에 원본에서) **기존 플래너·래스터·요약 경로를 그대로** 쓰고, 페이지
   페이로드만 `design.ovp` 대신 원본 파일의 세그먼트를 pread + inflate + 시드
   파싱한다. 1 TB 덱 추정: 생성 2~4 시간(§6; 요약 패스의 현장 수치 확인 필요),
   캐시 1~3 GB. 광역 뷰는 지금과 같고, 근접 뷰는 첫 방문 때 뷰 안의 세그먼트만
   읽는다(최악 = 셀 하나를 병렬 파싱, 12 스레드 0.2~0.3 s; 현장 실측 뒤 값, §6.2).
5. **픽셀 계약**: `--detail exact`/`--archival`(모든 도형)에서는 전체 색인과
   픽셀 동일(같은 레코드 집합). cut이 있는 뷰에서는 페이지 경계가 다르므로
   cut/hairline/예산 맞춤 판정이 조금 달라질 수 있다 — 동일성이 필요하면 셀의
   정규 페이지를 메모리에서 만들어 쓰는 길(§5.8)이 있다. 사용자 결정 사항.
6. **먼저 잴 것(§7, 하루치)**: 현장 파일의 세그먼트 공간 지역성(라이터가 도형을
   어떤 순서로 쓰는가)과 모달 스냅샷의 체인 빈도. `floe-index scan` 확장으로
   잰다(구현 §8.1, 0.12.374). 지역성이 나빠도 설계는 성립하지만(세그먼트 병렬 파싱이 하한을 준다)
   근접 뷰 첫 방문 시간이 ms인지 초인지가 거기서 갈린다.
   **현장 결과(2026-10-21, §8.1)**: 라이터는 레이어마다 한 축으로 정렬한 띠를
   CBLOCK 하나씩 쓴다(by bytes band 99 %, 겹침 p50 1.74 = 띠들이 셀을 한 번씩
   분할, 레이어/세그먼트 1, 체인 0). 근접 뷰 첫 방문 = 창을 가로지르는 띠
   10~20개, 수십 ms; 스냅샷 설계 그대로. 설계대로 L1 → L2로 간다.

## 1. 문제의 수치 — 지금 캐시가 왜 크고 느린가

### 1.1 캐시 구성(실측, 2026-10-21, macOS 4 jobs; 현장은 Linux 12+ 스레드)

| 소스 | 크기 | 레코드 | 배치 | 캐시 | ovp | ovm | ovo | 비율 | 색인 시간 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| sample9 (도형 위주, 40 레이어, 1,188셀) | 145 MB | 1,460만 | 2,851 | 1.2 GB | 149 MB | 4.7 MB | 1,070 MB | 8.3× (ovo 제외 1.06×) | 8.4 s (재고 1.3, 빌드 7.1; 파이프라인 3.3 중 인코딩 3.1) |
| route_chip 0.05 (점 목록 배치) | 281 MB | 2,760만 | — | 413 MB | 171 MB | 262 MB | — | 1.47× | 재고 12 s |
| main01_chip_p10 (MAIN01 1/10, 배치 위주) | 953 MB | 1.03억 | 8,260만 (멤버 16억) | 7.5 GB | 253 MB | 7,208 MB | 583 MB | 7.9× | 재고 12.6 s |
| 현장 50 GB 마스크 소스 (2026-10-20) | 50 GB | 19.7억 (85 % POLYGON) | ≤ 5,000만 | 미보고 (ovp ≈ 소스로 추정) | — | — | — | ≈ 1× | 7,890 s (재고 131.6, 빌드 5,992, 요약 1,766; `--mem 0.5` — 요약 패스는 예산 병목, §6.1) |

- 도형 위주 소스(마스크): **ovp ≈ 소스 크기**. 페이지는 "셀 하나짜리 완전한
  OASIS 파일(CBLOCK 포함)"이라 원본과 같은 정보를 같은 압축으로 다시 쓴 것이다.
  ovm은 작다(배치가 적다).
- 배치 위주 소스(레이아웃): ovm(배치 + 인스턴스 BVH)이 소스의 7.6×. 이쪽은 덱의
  문제가 아니다(덱 소스는 마스크).
- ovo는 레이어 수 × 칩 면적에 비례하는 밀집 비트맵이다. sample9의 1.07 GB는
  34 레이어 × 깊이별 평면 때문이고, 레이어 2~4개인 마스크는 작다(2026-09-15 덱
  전체 667 소스 172 MB).
- 시간: 재고(= scan) 2.6 s/GB(현장), 빌드 120 s/GB(현장), 요약 35 s/GB(현장;
  패스 수 미확인 — 2026-10-20에 요청한 `budget`/`pass(es)` 줄 대기 중. sample9는
  요약 패스 1회 2.4 s로 재고의 1.9배). sample9의 빌드 파이프라인(벽시계 3.3 s)은
  커미터의 페이지 인코딩·쓰기 구간이 3.1 s를 차지한다(플랜은 그 아래에 겹쳐
  돈다) — **페이지를 다시 쓰는 비용이 파이프라인을 결정한다**.

### 1.2 1 TB 덱으로 외삽(현장 속도)

| | 전체 색인(지금) | 경량 캐시(제안) |
| --- | --- | --- |
| 생성 시간 | 158 s/GB → **≈ 44 h** (소스 순차) | 재고+세그먼트 2.6 s/GB ≈ 45 min + 요약 1패스 5~35 s/GB ≈ 1.5~10 h → **2~4 h** 기대(§6) |
| 캐시 크기 | ovp ≈ 소스 → **≈ 1 TB** (+ ovm, ovo) | 재고 수 MB/소스 + 세그먼트 0.1~0.25 %/소스 + ovo → **1~3 GB** |
| 광역 뷰·캡처(≥ 4 µm/px) | ovo | ovo (같음) |
| 근접 뷰 | 페이지 디코드 | 원본 세그먼트 디코드 (§5.3) |

두 시간 모두 소스 하나씩 순차 처리 기준이다. 재고·요약은 소스 간 병렬이 쉬워
(파일당 메모리가 작다) NFS 대역폭이 허락하는 만큼 더 줄어든다. 전체 색인은
소스당 메모리(예산 32 GiB)와 디스크 쓰기 때문에 병렬화 여지가 적다.

## 2. 뷰와 캡처가 실제로 읽는 것

렌더러(render-core)가 캐시에서 읽는 것을 요청 종류별로 나누면 경량 캐시의
경계가 보인다.

| 요청 | 읽는 것 | 경량 캐시에서 |
| --- | --- | --- |
| 열기: unit, top, bbox, 레이어 표, 셀 수 | design.ovm 헤더·cells·layers | 재고 (`unit`, `top`, `rbb`, `layer_order`/`layer_names`) |
| 셀 트리, 검색, 셀 extent, 부모 | design.ovh (자식·멤버 수·extent·부모·탑 아래 인스턴스 수) | 재고의 간선(`Edge{child, placements, members, obox[8]}`)에서 같은 값을 메모리에서 만든다(hiersum의 topo DP) |
| 광역 뷰(기준 셀 ≤ 1 px, `FLOE_RUST_OCCUPANCY_PX`) | design.ovo 레벨 하나 | design.ovo (같음) |
| 광역 뷰, 밀도 스택(레이아웃) | design.ovs, design.ovb, 셀 덮임 | 덱은 밀도 끔(JOBDECK §7). 레이아웃은 §5.6 |
| 근접 뷰 플랜 | cells(rbbox·height·rank·마스크), places + 인스턴스 BVH(v7/v8 주석), pages(bbox·records·members·max_w/h/min), pranges·pbvh | cells ← 재고(전부); places·BVH ← 첫 방문에 원본에서 셀의 배치 레코드만 파싱해 만듦; pages ← 세그먼트 디렉터리 |
| 근접 뷰 디코드 | design.ovp의 페이지 바이트 → 작은 OASIS 파일 파싱 → `DecodedPage{Doc, PageIndex}` | 원본의 세그먼트 바이트 → inflate → 스냅샷으로 시드한 `parse_records` → 같은 `DecodedPage` |
| 라벨 | design.ovt + tbvh | 세그먼트의 텍스트 레코드(디코드한 페이지 안) |
| 셀 정보·pick·DRC 캡처 | 위와 같음 | 같음 |
| 덱 합성(`open deck=`) | 소스별 Cache | 소스별 경량 Cache (Deck 코드 불변) |

광역 뷰가 요약만 쓰는 것은 OCCUPANCY_PLAN(M2~M6)의 설계 그대로다: keep 정책에서
기준 셀이 화면 1 px 이하면 페이지 디코드·계층 순회를 생략하고 요약 평면을
그린다(`summary::level_for`). 덱 캡처의 창 크기는 보통 mm 단위, 픽셀은
1,200~2,400이므로 4~30 µm/px — 모두 요약 구간이다. 기준 셀을 1 µm로 내리면
(`--occupancy-um 1`) 요약 구간이 1 µm/px까지 내려오고 ovo는 16배 커진다
(마스크 4 레이어 35 mm급 칩이면 수백 MB).

## 3. 이미 있는 부품

| 부품 | 위치 | 상태 | 이번 설계에서 |
| --- | --- | --- | --- |
| 재고 패스 (S1) | `floe_oasis::inventory`, `floe-index inventory` | 완료 0.12.363; 셀별 `ranges`/`def_ranges`(바이트 범위), 암시 참조 시드, dbox 2종, 마스크 3종, 간선(배치 수·멤버·방향별 박스), fold(topo·rank·height·rmembers·rmask·rbb) | 사이드카 `design.inv`로 직렬화(§5.1); 같은 패스에서 세그먼트 디렉터리 방출(§5.2) |
| 셀 파서 (S3) | `doc::parse_cell_range` / `parse_cell_bytes` / `normalize_cells` | 완료 0.12.366; 범위 하나 → 셀(들), G2 레코드 동일 | 세그먼트 파서로 확장: 모달 스냅샷 시드(§5.2) |
| 스트리밍 리더 (S4) | `cli::stream::StreamStore` (pread, 예산) | 완료 0.12.367 | 렌더러 쪽 세그먼트 리더는 작게 새로 둔다(파일 순서 묶음 읽기, 오프셋 정렬) |
| 요약 (S5) | `floe_vfs::occupancy::StreamOcc`, `--occupancy-only` 스트리밍 | 완료 0.12.368; 재고에서 인스턴스 열거 | 경량 캐시의 두 번째 패스. 전체 캐시 없이 돌 수 있게 identity 검사 대상을 ovm → inv로 |
| 페이지 = 독립 OASIS 파일 | `Vfs::read_page_batch`, `cache::decode_payload` | 페이지마다 CELL 하나짜리 OASIS 파일(CBLOCK 포함) | 세그먼트 페이지는 같은 `DecodedPage`로 끝난다 — 래스터·PageIndex 불변 |
| 플래너 | `floe_vfs::hier::plan_hier(&Ovm, …)` | Ovm 접근자(cells/places/bvh/pages/pranges/pbvh) 위에서 동작, 105개 pub fn | 메모리에서 합성한 Ovm(§5.3)을 그대로 받는다. `Ovm::from_bytes`, `Vfs::from_ovm_for_tests`가 이미 있다 |
| 부분 프레임·refine | `FrameScene::deferred_pages`/`is_partial`, 뷰어의 refining 경로 | 예산 밖 페이지는 미룬 채 그리고 뒤에 채운다 | "아직 안 읽은 셀" = 미룬 페이지와 같은 흐름(§5.4) |
| 자동 색인·잠금 | app-core `index::decide`, `.floe-lock/` | 인덱스 없는 소스를 먼저 색인 | 경량 모드의 자동 생성·잠금은 같은 틀 |
| scan | `floe-index scan` (셀 통계, Doc 추정) | 완료 0.12.360 | 세그먼트 지역성·스냅샷 체인 통계 추가(§7) |

예전의 "오프셋 저장" 시도는 이 저장소 안에는 없다(DRC 결과 db의 v1 오프셋
사이드카는 2026-08-19 폐기된 다른 것이다). 그 아이디어가 겨냥한 것 — 원본을 다시
쓰지 않고 위치만 적어 두기 — 는 S1 재고가 셀 단위로 이미 하고 있고, 이 설계는
그것을 셀 안(세그먼트)으로 내리고 파일로 남기고 렌더러가 읽게 하는 것이다.

## 4. 왜 셀 단위로는 부족한가 — 세그먼트가 필요한 세 가지 이유

현장 마스크 소스: 셀 5,461개, CBLOCK 78.2만(평균 67 KB, p99 89 KB, 최대 7.8 MiB;
L0 실측), 가장 큰 셀 26 MiB = CBLOCK 341개(레코드 118만, Doc 415 MiB),
폴리곤당 꼭짓점 28.9개. 평면에 가까운 구조라 근접 뷰는 거의 항상 큰 셀 하나의
작은 부분이다.

1. **공간·레이어 프루닝.** 셀 범위 하나를 읽으면 셀 전부를 inflate·파싱한다.
   가장 큰 셀 = CBLOCK 341개; 1 mm 창은 35 mm급 칩이면 면적의 1/1,225, 한 축 띠로는
   1/35다. 세그먼트마다
   bbox·레이어 마스크가 있으면 뷰와 만나고 보이는 레이어를 가진 세그먼트만
   읽는다. 라이터가 타일 순서로 썼다면 수십 개, 한 축 띠라면 창을 가로지르는
   띠 — 현장 실측(§8.1)이 이 경우로, 341개 중 10~20개(≈ 1.5 MB, 레코드 3.5~7만) —
   무작위면 전부다.
2. **셀 안 병렬 파싱.** OASIS 모달 변수는 CELL 시작에서만 초기화되므로 지금의
   병렬 파싱(`cell_cuts`)은 CELL 경계에서만 나눈다 — 가장 큰 셀(26 MiB)은 스레드
   하나가 1~2 s를 쓴다(현장 병렬 파싱이 파일 50 GB를 160 s에 읽었으니 ≈ 310 MB/s,
   12 스레드면 ≈ 26 MB/s/스레드). 세그먼트
   경계의 모달 스냅샷이 있으면 어느 세그먼트든 독립적으로 파싱한다 → 셀 전부가
   필요한 최악의 경우도 12 스레드 0.2~0.3 s. (스트리밍 색인의 "giant top cell의
   플랜 스크래치 피크"와 §4.7의 "셀 내부 분할"도 같은 열쇠로 풀린다.)
3. **페이지 디렉터리 대체.** 전체 색인의 페이지(셀·레이어별 ≤ 1 MiB, bbox·
   max_w/h/min·records·members)는 플래너의 cut·hairline·예산 판정 단위다.
   세그먼트에 같은 통계를 재고 패스에서 적어 두면 플랜·인코딩 없이 "페이지
   디렉터리"가 생긴다 — 전체 색인 시간의 대부분(sample9 빌드 7.1 s 중 인코딩
   3.1 s, 플랜·요약·기타 나머지)이 없어진다.

세그먼트 단위 선택: CBLOCK이 있으면 CBLOCK 하나(압축 블록은 통째로 inflate해야
하므로 더 잘게 나눌 수 없다; 현장 평균 64 KB가 적당하다). CBLOCK이 없는 소스
(KLayout 비압축 출력, 합성 fixture)는 레코드 경계에서 자른 고정 크기 구간
(예: 256 KB). 두 경우 모두 "세그먼트 = 바이트 범위 + 시작 시점의 모달 상태".

## 5. 설계

### 5.1 경량 캐시 파일 (`.<src>.ice/` 안, 전체 캐시와 같은 디렉터리·잠금·이름 규칙)

| 파일 | 내용 | 크기(현장 50 GB) |
| --- | --- | --- |
| `design.inv` (**구현 §8.2**, SPEC-FORMATS FLOEINV1) | 재고 직렬화: 헤더(magic, 버전, unit, src_size/mtime, top, n_cells, n_layers), 레이어 표(순서·이름·별칭), 셀 표(이름, ranges, def_ranges, 카운터, dbox 2종, dmembers, 마스크 3종), 간선(child, placements, members, obox 8 — 쓰인 방향만), fold(topo, rank, height, rmembers, rmask/rtmask/rsmask, rbb 2종), CELLNAME/TEXTSTRING 표 | 셀 5,461 → 수 MB. (p10: 셀 35,183·간선 188,065·레이어 449 → 약 60~80 MB; 마스크 상한 = obox 8 × 32 B를 쓰인 방향만으로) |
| `design.seg` (**구현 §8.3**, SPEC-FORMATS FLOESEG1) | 세그먼트 디렉터리: 세그먼트마다 cell u32, 파일 오프셋 u64, 압축 길이 u32, 해제 길이 u32, 종류(CBLOCK/구간) u8, 스냅샷 오프셋 u32(또는 "체인: 이전 독립 세그먼트에서 파싱"), 레코드 수 u32, 멤버 u64, bbox 4×i64, 레이어 마스크(비트셋 풀 참조), 레이어별 통계 블록(레이어 idx, records, members, bbox, max_w, max_h, max_min — 세그먼트가 가진 레이어만), 배치·텍스트 유무 플래그; 셀별 세그먼트 범위 표; 스냅샷 풀 | 78.1만 × (약 80 B + 레이어 통계) ≈ 60~120 MB (0.12~0.24 %) |
| `design.ovo` | 지금과 같은 FLOEOVO2 | 마스크 수 MB~수십 MB |
| `meta.json` (**구현 §8.5**) | 전체 색인의 키(version, vfs, src, dbu, top_cell, bbox, grid, layers, texts, frontier)에 `"cache": "light"`; `src.path`가 캐시가 소스 옆이 아닐 때 소스를 가리킨다 | — |

- 무효화: 전체 캐시와 같은 `src_size/src_mtime` identity. 세 파일은 각각
  tmp + rename 게시, `design.inv`·`design.seg`가 한 쌍(둘 다 재고 패스가 쓴다),
  `design.ovo`는 지금처럼 독립 사이드카.
- 잠금: 전체 캐시의 `.floe-lock/` 규칙 그대로(빌드 중 읽기 거부, 열린 캐시의
  재빌드 거부).
- 전체 캐시와 공존: 같은 디렉터리에 `design.ovm/ovp`가 있으면 전체 캐시가
  우선(선택 규칙 §5.6). 경량 캐시를 먼저 만들고 나중에 전체 색인을 더하는 것도,
  전체 색인에 `design.inv/seg`를 덧붙이는 것도 가능하다(재고 패스는 전체 빌드의
  1단계이므로 전체 빌드가 inv/seg를 함께 쓰게 하는 것이 자연스럽다).

### 5.2 세그먼트 디렉터리와 모달 스냅샷

재고 패스(`inventory::Part::record`)는 이미 doc.rs와 같은 모달 변수(XY 모드,
geometry/placement/text x·y, W/H, 점 목록, 경로 확장, 반복, 배치 대상, 텍스트
문자열, CTRAPEZOID 암시 치수)를 따라간다. 세그먼트 경계(CBLOCK 시작, 또는 구간
크기 도달 뒤 첫 레코드 경계)에서 그 상태를 적는다.

- **스냅샷 내용** = doc.rs `Modal`: rep(Option<Rep>), layer, datatype, textlayer,
  texttype, relative, geo/pl/tx x·y, geo_w/h, circle_r, ctrap_type, poly_pts,
  path_pts, path_hw/es/ee, place_cell, text_string. 스칼라만이면 ~120 B.
- **목록 모달**(rep의 Pts, poly_pts, path_pts): 경계 시점에 살아 있고 그 세그먼트
  안에서 재사용(rep type 0, 점 목록 생략)되면 필요하다. 작으면(≤ 64 점) 스냅샷에
  넣고, 크면 "체인" — 그 세그먼트는 앞의 독립 세그먼트부터 이어서 파싱한다.
  재고 패스가 세그먼트 안에서 실제 재사용이 일어나는지 알 수 있으므로(그 세그먼트
  안의 레코드가 모달 목록을 쓰기 전에 새로 정의하면 스냅샷에 넣지 않아도 된다)
  체인은 "큰 목록을 경계 너머로 재사용하는" 경우에만 생긴다. 빈도는 §7에서 잰다.
- **파서**: `parse_cell_bytes`에 스냅샷 시드를 더한 `parse_segment(bytes, seeds,
  snapshot, tables) -> Doc(셀 하나)`. CBLOCK은 inflate 뒤 같은 길. 결과는 지금의
  페이지 디코드와 같은 `Doc` → `DecodedPage`.
- **레이어별 통계**: 전체 색인 페이지의 max_w/max_h/max_min/records/members/bbox를
  세그먼트 × 레이어로 같은 함수(`geom::rep_extent`, `path_bbox`)로 계산한다. 재고
  패스가 이미 bbox 계산을 위해 같은 값을 만들고 있다.
- **정규화**: 전체 패스의 `normalize_cells`(반복 분할 등)는 셀 단위다. 세그먼트
  페이지에는 레코드 단위로 같은 정규화를 세그먼트 안에서 건다(정규화는 레코드
  하나를 보고 결정하므로 경계에 무관).

### 5.3 렌더러: 경량 캐시를 Ovm으로 합성 (`LightVfs`)

목표: `plan_hier(&Ovm, …)`·`FrameScene`·래스터·요약 경로·덱 합성을 **바꾸지 않는다**.
`Cache::open(dir)`이 `design.ovm`이 없고 `design.inv/seg`가 있으면 `LightVfs`를
만든다.

1. **셀 디렉터리** — 재고에서 전부: name, height, topo_rank, rbbox, bbox,
   lmask_rec(rmask), 텍스트 필드. 모든 셀이 처음부터 있다(G1이 보증하듯 전체
   빌드의 값과 같다).
2. **페이지 디렉터리** — 세그먼트 디렉터리에서: 세그먼트 × (가진 레이어) 하나당
   `PageV{cell, layer_idx, bbox(레이어별), records, members, max_w, max_h, max_min,
   codec = SEGMENT, file_off/csize/usize_ = 원본 세그먼트}`; pranges는 (셀, 레이어)별
   세그먼트 런; pbvh는 런의 bbox로 메모리에서 만든다(전체 빌드의 `finish_layer`와
   같은 함수). 페이지 ID는 (세그먼트, 레이어) 쌍에 하나씩 — 한 세그먼트가 여러
   레이어를 가지면 디코드는 세그먼트당 한 번 하고 레이어별 `DecodedPage`로 나눈다
   (디코드 캐시 키 = 세그먼트, 페이지 조회는 그 안의 레이어 슬라이스).
3. **배치 + 인스턴스 BVH** — 처음에는 비어 있다(place_count 0). 셀을 처음 방문할 때
   그 셀의 "배치 있음" 세그먼트만 읽어(플래그) 배치 레코드를 모으고
   `build_cell_plan`의 BVH 단계(v7 max_dim/max_min, v8 레이어 마스크 ← 재고
   rmask)로 만든다. 배치가 많은 셀(레이아웃의 top)도 지금 전체 빌드가 하는 일과
   같고 한 번이다.
4. **합성** — 방문한 셀의 BVH·배치가 늘면 Ovm 바이트를 다시 만든다(`Builder`에
   셀 순서로 "있는 것/빈 것"을 넣고 `finish` → `Ovm::from_bytes`). 비용은 방문한
   셀의 배치 바이트에 비례하고 프레임당 한 번이면 된다(마스크: 수 ms;
   MAIN01급 레이아웃: 방문한 서브트리만큼). 방문 셀의 LRU(바이트 예산)로 상주를
   묶고 밀려난 셀은 빈 것으로 돌아간다.
5. **페이로드** — `Vfs::read_page_batch`의 경량판: 원본 파일에서 세그먼트를
   오프셋 순으로 pread(묶어서), CBLOCK은 inflate, 스냅샷으로 시드한 파싱. NFS에서
   64 KB 읽기 수백 개는 오프셋 정렬 + 병렬 리더로 흡수한다. 디코드 워커는 지금의
   `decode_pages_parallel` 그대로.
6. **계층 요약·셀 트리** — `design.ovh`를 만들지 않고 재고 간선에서 같은 표를
   메모리에 만든다(hiersum의 DP는 간선 입력이면 된다; 현장 5,461 셀은 ms,
   p10 35,183 셀·188,065 간선은 수십 ms).
7. **요약** — `design.ovo`는 지금 그대로(`OccupancySlot`). identity 검사의 상대가
   ovm이 아니라 inv가 된다.

### 5.4 프레임 흐름 — "아직 안 읽은 셀"

- 플랜: 합성 Ovm 위에서 지금처럼. 미방문 셀은 배치가 없는 것으로 보이지만
  재고는 `placements > 0`이라고 한다 → 플랜 결과의 "방문했고 배치 미구축인 셀"
  목록을 모은다(cut 아래라 들어가지 않은 셀은 당연히 제외 — 플래너가 이미 그
  판정을 한다).
- 채우기: 그 셀들의 배치를 읽어 BVH를 만들고 합성 → 다시 플랜. 한 라운드가
  계층 한 단계를 내려가므로 라운드 수 ≤ 깊이(MAIN01 15, 마스크 2~3). 라운드는
  프레임 하나 안에서 돌고 각 라운드의 플랜은 ms 단위다.
- 페이지: 플랜이 고른 세그먼트 페이지를 디코드 캐시 → 없으면 원본에서 읽는다.
  지금의 세대 예산(기본 1 GiB)·예산 맞춤(budget fit)·미룬 페이지(partial →
  refining) 흐름이 그대로 적용된다. 뷰어는 첫 라운드의 부분 프레임을 먼저 보고
  채워지는 것을 본다(지금의 refining과 같은 경험); 캡처(`floe2 render`)는 완료까지
  기다린다(지금도 partial을 버린다).
- 메모리: Doc 2~3 GB짜리 셀 전체 파싱은 일어나지 않는다. 디코드 캐시가 세그먼트
  단위로 예산을 지킨다. 배치 BVH 상주는 LRU.
- **구현(0.12.384, §8.7).** 라운드는 `LightLink::run_within(work, deadline)`이 돈다.
  뷰어 프레임(캡처의 `progressive=0`·여백 프레임이 아닌 것)은 probe·플랜·라벨 걷기가
  `FLOE_LIGHT_FIRST_MS`(기본 300 ms; `0`/`off` = 끝까지, L4 동작) 안에서 채우고, 남은
  표시가 있으면 지금 이미지의 플랜으로 프레임을 `final=0`으로 내보낸 뒤(renderd
  `LIGHT_CONTINUE` 시도; 라운드 번호는 이어진다) 같은 프레임을 다시 시도한다 — 시도마다
  적어도 한 라운드는 채우므로 계층 깊이 안에 끝난다. 뷰어는 "reading the source…
  (fill round N)"을 보고 채워지는 것을 본다. 미완 시도의 fit 결정·보관 프레임·published
  scene은 남기지 않는다. 덱 프레임도 같다(`run_deck_render_attempt`,
  `DeckRenderRequest::light_deadline` → pass의 `Cache::plan_within`,
  `DeckRenderReport::light_pending`; 경량 소스가 하나라도 있을 때). 캡처(`floe2 render`, worker-client `RenderRequest::progressive =
  false`)는 끝까지 채운 뒤 찍는다. 프레임 줄 `light_rounds= light_pending=`.

### 5.5 광역 뷰 = 요약, 요약이 없을 때

- 경량 캐시의 생성 순서는 inv/seg(빠름) → ovo(느림). ovo가 아직 없는 동안의
  광역 뷰는 세 가지 중 하나다(사용자 결정): (a) "summary building…" 배너와 빈
  광역 뷰, 근접 뷰는 바로 동작; (b) 재고의 셀별 bbox 채움(위치 보존 없음 —
  2026-09-11 리뷰에서 광역 표시로는 거부된 방식; "임시 표시"로만); (c) 요약이 될
  때까지 열기를 막는다. 제안은 (a).
- 덱의 광역 캡처는 ovo 없이는 의미가 없으므로 `floe2 render`의 자동 생성은
  inv/seg + ovo까지 만든 뒤 찍는다.
- 기준 셀: 덱 캡처가 1~4 µm/px까지 내려가면 `--occupancy-um 1`(ovo 16배)을
  권한다. 요약 구간 밖(기준 셀 > 1 px)은 세그먼트 경로다.
- **구현(0.12.384, §8.7; 결정 (a)).** 덱 pass의 소스가 경량이고 design.ovo가 없거나
  (`nofile`/`invalid`) 읽히지 않을 때, 그 소스의 자동 기준 셀(`auto_base_um_for_span`, 탑의
  긴 변)로 판정해 요약이 그릴 뷰(`level_for`가 Some)이면 pass를 비워 두고
  `summary_building_passes`에 센다(render-core `DeckSource::summary_pending`). 근접 뷰는
  페이지 경로다. 프레임 줄 `summary_building=N` → 뷰어는 캔버스 위 배너("occupancy
  summary building…" / "missing …")를 띄우고 3 s마다 다시 그려 요약이 생기면 걷힌다;
  `floe2 render`는 그 pass를 비운 그림을 쓰고 미완(`summary_building`)으로 끝낸다(종료 3,
  덱 캡처의 다른 미완과 같다). 그리는 명령
  (`render`·`probe`)의 자동 색인(`ready.rs`)은 요약 없는 덱 소스도 "no occupancy summary
  yet"으로 먼저 색인한다(`--no-index`면 거절; `info`는 캐시를 있는 대로 읽는다). GTK 열기는 2단계였다: `floe2 index deck.jb --no-occupancy`(재고,
  모달) → 열기 → 뒤에서 `floe2 index deck.jb`(요약; 경량 캐시의 additive 잠금이라 열린
  뷰어 옆에서 붙는다; 뷰어가 닫히면 함께 끝나고 다음 열기가 이어 받는다). 열기 응답
  `jobdeck.light_sources`/`summary_missing`, 준비 질의 `summary_missing`. **0.12.388(§8.11)부터**
  뷰어는 모달 색인 한 번으로 요약까지 만들고 백그라운드 요약은 없으며, 캡처의 자동 색인은
  `--occupancy on`일 때만 요약을 더한다.

### 5.6 모드와 선택 규칙

- 명령: `floe-index light SRC OUT [--jobs N] [--seg-bytes 256K] [--no-occupancy]`
  (= 재고 패스 + 세그먼트 방출 + 요약 패스), `floe2 index --light` / 덱
  `floe2 index deck.jb --light`. 지금의 `floe-index vfs`는 그대로(전체).
- 자동 색인(`info`/`render`/`view`의 "인덱스 없는 소스"): 기본을 어느 쪽으로
  둘지는 사용자 결정. 제안: **덱 소스는 경량이 기본**, 단독 레이아웃은 전체가
  기본, `FLOE_INDEX_LIGHT=on|off`와 `--light`/`--full`로 바꾼다. 크기 기준 자동
  선택(예: 2 GB 이상은 경량)은 두 번째 안.
- 열기: 디렉터리에 ovm이 있으면 전체, 아니면 inv/seg가 있으면 경량, 둘 다 없으면
  자동 생성 규칙. 상태줄·`info --json`에 `cache: full|light`.
- 밀도 스택(레이아웃 근접-광역 사이, design.ovs/ovb): 경량 모드에서는 ovs 패스를
  요약 패스에 합칠 수 있다(둘 다 "셀을 모든 인스턴스에 마킹"하는 같은 패스;
  design.ovs v2는 자기 셀만 마킹). 1차 구현은 덱(밀도 끔)에 집중하고 레이아웃은
  전체 캐시 기본을 유지한다.
- 웹(feature/webui)의 revision store는 업로드한 소스를 색인한 결과를 봉인한다 —
  경량 캐시도 같은 디렉터리 규칙이므로 render-core가 열 수 있다; 웹 쪽 정책은
  별도.
- **구현(0.12.383, §8.6).** `floe-index light SRC [--out DIR] [--jobs N] [--seg-bytes N]
  [--occupancy …] [--occupancy-only] [--mem GiB] [--force]`(요약은 `vfs`처럼 opt-in;
  `floe2 index`가 덱 소스에 붙인다), `floe2 index SRC --light | --full`,
  `FLOE_INDEX_LIGHT=on|off`, 기본 = 덱 소스 경량·레이아웃 전체(전체 색인만의 옵션이
  있으면 전체). 열기는 위 규칙대로이고, `floe2 info`가 `light: the geometry is read
  from the source`와 inv/seg/ovo 크기를, `info --json`이 `cache_kind: full|light`를 낸다.
  상태줄(깊이 라벨)은 0.12.384부터 레이아웃이면 ` · cache:light`, 덱이면 ` · light:N/M`
  (경량 소스 수/소스 수)을 붙인다; renderd의 `opened` 줄은 `cache=light|full`
  (레이아웃) / `light_sources= summary_missing=`(덱)을 낸다.

### 5.7 운영: NFS, 메모리, 동시성

- 읽기: 재고·요약 패스는 지금의 mmap/pread. 렌더러의 세그먼트 읽기는 pread
  (오프셋 정렬, 리더 n개). 원본은 열어 두되 잠그지 않는다(지금도 원본은 읽기만).
- 원본이 바뀌면 identity 불일치 → 캐시 무효(지금 규칙).
- 메모리: 경량 생성은 재고(파일 매핑 + 셀 표) + 요약(평면 예산)뿐이라 32 GiB
  예산 없이도 돈다(요약의 `--mem`은 유지). 렌더러는 디코드 예산 + 배치 LRU.
- 여러 프로세스가 같은 소스를 경량 모드로 열어도 캐시는 읽기 전용이다.

### 5.8 픽셀 계약

- `--detail exact`·`--archival`: 모든 도형을 그리므로 전체 색인과 **픽셀 동일**
  (레코드 집합이 같다; 세그먼트 정규화가 셀 정규화와 레코드 단위로 같음을 G2급
  게이트로 고정).
- cut이 있는 뷰(뷰어 기본 medium, 캡처 기본): 페이지 경계가 다르므로 페이지
  단위 판정(크기 cut, hairline, 예산 맞춤의 우선순위, LOD 없음)이 달라질 수 있다.
  화면상 차이는 "어느 가는 선 묶음이 요약되느냐" 수준이고, 요약 구간(광역)은
  동일하다. **동일성이 필요하면**: 첫 방문 셀에 `build_cell_plan`을 돌려 정규
  페이지(rep-split·Morton·LOD)를 메모리에서 만들고 페이로드는 인코딩 없이
  `DecodedPage`로 보관하는 변종(§9 B안)을 그 셀에만 쓴다 — 비용은 셀의 플랜
  시간(sample9 기준 인코딩 없이 수백 ms/백만 레코드).

## 6. 비용 추정

### 6.1 생성(1 TB 덱, 현장 호스트 속도)

| 구간 | 속도 근거 | 1 TB |
| --- | --- | --- |
| 재고 + 세그먼트 | 현장 재고 131.6 s/50 GB = 2.6 s/GB (세그먼트 통계는 같은 걸음에 더해지며 bbox는 이미 계산 중; +10~20 %) | 45~55 min |
| 요약 패스 | 현장 1,766 s/50 GB = 35 s/GB는 `--mem 0.5`(상주 예산 415M ≈ 2셀)의 1패스로, 파싱·마킹이 거의 직렬이었다(아래). 기본 예산이면 4~6 min = 5~7 s/GB; sample9도 1패스에 재고의 1.9배. **0.12.387(§8.10)부터 마커가 셀을 직접 파싱해 예산과 무관하게 `jobs`개가 돈다**: sample9 요약 3.2 → 2.9 s(재고 1.5 s), `--mem 0.3`에서도 4.0 s(전에는 18.8 s) | 재고의 1~1.5배 = **1~1.5 h** 기대 (예산 무관) |
| 쓰기 | 수 GB | — |
| 합 | | **2~3 h** (vs 44 h; 0.12.387 뒤) |

요약 패스가 현장에서 재고의 13배였던 까닭은 2026-10-21에 받은 줄들이 답했다
(STREAMING_INDEX_PLAN §7·§11): 그 실행은 `--mem 0.5`였고(`budget 512M from --mem`),
패스는 하나였으며(`1 pass(es)`, plane budget 128M에 5 레이어가 들어감), 상주
예산이 415M = 가장 큰 셀의 Doc(`resident budget 415M`)이라 평균 174 MiB인 셀이
한 번에 2개(마킹 중인 셀 포함)만 상주 → 리더 12·마커 12 중 2개씩만 돌았다
(heartbeat 53셀/10 s → 5,411셀/1,760 s = 내내 3.1셀/s; ≈ 0.32 s/셀 × 5,461). 기본 예산(32 GiB → 상주 8 GiB ≈ 45셀)의 재실행을 요청했고
4~6 min으로 본다. 마킹 비용 자체(≤ 0.3 s/셀)는 2차이며 §8 L7은 그 뒤의 일이다.

### 6.2 근접 뷰 첫 방문(가장 큰 셀 — 26 MiB, CBLOCK 341개 — 안의 1 mm 창, 0.83 µm/px, 레이어 1개)

| 라이터 순서 | 읽는 세그먼트 | 시간(12 스레드) |
| --- | --- | --- |
| 타일/공간 정렬 | 수십 개 × 67 KB | 수십 ms |
| 한 축 정렬 띠(**현장 실측**, §8.1) | 창을 가로지르는 10~20개 ≈ 1~1.5 MB(inflate 2~4 MB, 레코드 3.5~7만) | 수십 ms |
| 무작위 | 전부 341개 = 26 MiB | 0.2~0.3 s (병렬 파싱; 스냅샷이 없으면 1~2 s) |

첫 검토(0.12.373)는 이 셀을 415 MiB(파일)로 보고 띠 0.1~0.3 s, 무작위 1~2 s(스냅샷
없이 16 s)로 잡았다 — scan의 415M은 Doc 추정치였다(L0 실측으로 정정). 세그먼트
하나의 상한은 현장 최대 CBLOCK(압축 7.8 MiB, inflate 17 MiB, 레코드 16,391)이다.

두 번째 방문부터는 디코드 캐시(세그먼트)와 배치 LRU에서 온다. 같은 소스의
여러 캡처(batch)는 워커 하나가 열어 두므로 공유된다.

### 6.3 광역 뷰

지금과 같다(ovo 레벨 하나). 요약이 없는 상태는 §5.5.

## 7. 위험과 먼저 잴 것

| 위험 | 영향 | 측정/대응 |
| --- | --- | --- |
| 세그먼트 공간 지역성이 나쁨(라이터가 도형을 무작위 순서로 씀) | 근접 뷰 첫 방문이 셀 전체 파싱(병렬 0.2~0.3 s)으로 떨어짐 | **L0**: `floe-index scan`에 세그먼트별 bbox를 더해 "세그먼트 bbox 면적 합 ÷ 셀 bbox 면적"(겹침 배수)과 세그먼트 bbox 크기 분포(셀 대비 분위수), 레이어 수 분포를 셀별로 낸다. 현장 파일에서 한 번(scan 시간 ≈ 2 min/50 GB). 겹침 배수 ≤ 10이면 좋음, ≥ 1,000이면 무작위. **구현 §8.1(0.12.374); 현장 결과 2026-10-21: 한 축 띠(band 99 %), 겹침 p50 1.74 — 근접 뷰 = 띠 10~20개** |
| 모달 목록 체인이 잦음 | 체인된 세그먼트는 앞에서부터 파싱 → 병렬성·프루닝 손실 | L0에서 "경계에서 살아 있는 목록 모달 × 세그먼트 안 재사용" 빈도. 잦으면 목록 상한을 올리거나(스냅샷 풀이 커짐) 세그먼트 경계를 목록 정의 직후로 옮긴다. **현장 결과: 체인 0** |
| CBLOCK 안의 CELL 레코드(드묾) | 그 셀은 둘러싼 범위의 세그먼트로만 접근 | 재고가 이미 `cblock_defs`로 센다; 세그먼트는 둘러싼 셀의 것 + 안의 CELL 경계 기록 |
| cut 뷰의 픽셀 차이 | 전체 색인과 비교 게이트가 "동일"이 아니라 "허용 오차" | §5.8; exact는 동일 게이트, cut은 B안 선택지 |
| 요약 패스가 현장에서 느림 | 생성 시간의 대부분 | §6.1의 현장 줄 확인 → 예산/마킹 개선. **0.12.387(§8.10)**: 마커 직접 파싱으로 예산 병목 제거 |
| NFS의 작은 pread 다수 | 근접 뷰 지연 | 오프셋 정렬·묶음 읽기·리더 병렬; L0 뒤 현장 한 뷰로 잰다 |
| 배치가 많은 소스(레이아웃 top 643 M 배치) | 첫 방문의 배치 수집이 전체 빌드의 BVH 비용과 같음(수십 s) | 덱 소스에는 없는 경우(≤ 5,000만). 레이아웃은 전체 캐시 기본 유지(§5.6) |
| gzip OASIS·GDS | 랜덤 액세스 불가/미지원 | 지금도 미지원(plain OASIS만); 변화 없음 |

## 8. 단계 (사용자 결정 뒤 착수)

| 단계 | 내용 | 게이트 |
| --- | --- | --- |
| L0 측정 | scan 확장: 세그먼트별 bbox·레이어 수·모달 체인 통계; 현장 파일 1회 | **완료 0.12.374(§8.1)**: 단위 테스트 + `rust_scan` 게이트; **현장 판정 2026-10-21: 한 축 띠, 체인 0 — L1로** |
| L1 사이드카 | `design.inv` 직렬화/로더(재고 ↔ 파일 왕복 동일), 전체 빌드도 함께 씀(선택) | **완료 0.12.379(§8.2)**: 왕복 바이트 동일(단위 테스트 + `rust_inventory`), 스트리밍 빌드가 씀(`vfs_stream`) |
| L2 세그먼트 | 재고 패스의 세그먼트 방출(스냅샷·레이어 통계), `design.seg` | **완료 0.12.380(§8.3)**: 세그먼트 통계 = Doc(`--check-seg`, `rust_inventory`), 왕복 동일, 스트리밍 빌드가 씀(`vfs_stream`) |
| L3 시드 파서 | `parse_segment`, CBLOCK inflate, 체인 | **완료 0.12.381(§8.4)**: 모든 fixture + 적대적 스트림에서 세그먼트 파싱의 레코드 합 = 셀 파싱(`--check-seg`, `rust_inventory`, 단위 테스트) |
| L4 LightVfs | 합성 Ovm, 세그먼트 페이지 디코드, 배치 LRU, 셀 트리 | **완료 0.12.382(§8.5)**: 게이트 `light_cache` — fixture 5종 × exact 뷰 5개(전체·근접·depth 1·레이어 부분)가 전체 색인과 PNG 동일 + 플랜의 셀·간선·프레임 수 동일, cut 3 px 뷰는 빛나는 픽셀의 0.5 % 안(실측 0), 거절 3종; 단위 테스트(재고 요약 = 이미지 요약, 페이지 = 레이어 도형, 셀 트리 질의); valmini·sample9·route_chip·p10에서 뷰 17개 PNG 동일 |
| L5 명령·모드 | `floe-index light`, `floe2 index --light`, 열기 규칙, 상태줄/info, 자동 생성, 잠금 | **완료 0.12.383(§8.6)**: `floe2`(경량 생명주기: index --light/--full, info, exact 렌더 = 전체, clip 거절, 덮어쓰기 규칙, FLOE_INDEX_LIGHT), `jobdeck`(덱 소스 경량 기본), `index_lock` K13, `light_cache`(design.ovo 바이트 동일, --occupancy-only, --force) |
| L6 덱·뷰어·캡처 | 덱 소스 경량 열기, 요약 없는 상태 표시, refine 흐름, `floe2 render` 완료 대기 | **완료 0.12.384(§8.7)**: `floe2`(exact 렌더 = 전체, 라벨 켠 채), `jobdeck`(LightSummaryTests: `--no-occupancy` 뒤 광역 캡처 미완(3)·근접 캡처 통과·자동 색인의 요약 추가·2단계 `index`), `gtk_view`(경량 레이아웃의 `cache:light`·`light_rounds`), 단위 테스트(텍스트 채우기 = Doc, 라벨 행, 시한 라운드) |
| 운영 규칙 | 캡처·뷰어의 캐시 종류와 요약 기본값 (사용자 시나리오 2026-10-22) | **완료 0.12.388(§8.11)**: `render`·`probe`·`info --light/--full`, `render --occupancy on`만 요약 추가, 뷰어 모달 색인 1회·백그라운드 요약 제거; 게이트 `jobdeck` LightSummaryTests, `floe2` |
| L8 띠 분할 캡처 | exact 캡처가 디코드 예산으로 실패하지 않게 장치 행 띠 단위로 플랜·디코드·래스터 | **완료 0.12.389(§8.12)**: renderd `render_banded_capture`, 띠 그림 = 통짜 그림 바이트 동일(게이트 `floe2`), 캡처 시한 해제 |
| 리뷰 수정 A | 2026-10-23 리뷰 5건 중 1·4·3·2 | **완료 0.12.390(§8.13)**: `pieces_of` 바이트 배치 읽기(요약 pass가 원본 전체를 올리던 회귀), 소스 fd 풀의 (dev, ino) 검증, 덱 `--full`이 경량 캐시를 전체로 재색인, 메타데이터 전용 light의 예산 검사 면제; 5(공유 CBLOCK 범위의 셀별 재파싱)는 §8.14 |
| 리뷰 수정 B | 리뷰 5건 중 5 | **완료 0.12.391(§8.14)**: 여러 셀을 정의하는 범위(다른 셀 범위의 CBLOCK 안 CELL)를 마킹 pass가 한 번만 파싱하고 범위 cost로 과금(`StreamStore::parse_range`, markpass `Task::Range`); 픽스처 `cells_in_one_cblock`, 게이트 `vfs_stream` |
| 리뷰 수정 C | 0.12.391 리뷰 4건(5의 변형 1, 회귀 3) | **완료 0.12.392(§8.15)**: 정의를 공유하는 셀 집합(`SharedDef`)을 마킹 pass가 한 번에 파싱·과금(각자 정의 뒤 CBLOCK 재정의도), 띠 상자를 전체 플랜의 뷰로 잘라 가장자리 픽셀 동일, 높이 1은 띠 하나, 렌더 중 패닉은 `error code=panic` 응답, 자동 색인의 덱 플랜은 캐시를 그대로 연다(`render --full --occupancy on`이 요약을 만듦) |
| 리뷰 수정 D | 0.12.392 리뷰 1건(연결된 집합을 통째로 적재) | **완료 0.12.393(§8.16)**: 집합의 범위를 파일 순서로 하나씩 내보내고 마지막 정의 범위가 읽힌 셀부터 join·마킹·해제(리더 풀의 규칙, 상주는 다중 정의 span 이내), 여러 셀을 정의하는 범위의 과금도 `min_budget`에 포함, 그룹 줄에 `resident peak`, 게이트가 peak ≤ 예산을 확인 |
| 리뷰 수정 E | 0.12.393 리뷰 1건(비대상 셀의 임시 파싱 메모리) | **완료 0.12.394(§8.17)**: 파서가 그룹이 쓰는 셀의 레코드만 보관(`parse_cell_bytes_keeping`, 나머지는 파싱만), 집합들을 파일 순서의 한 커서로 함께 진행, 게이트가 레이어별 그룹의 12 jobs RSS 증가(≤ 18 MB)를 확인 |
| 덱 첫 화면 | 현장 2026-10-23: 274 경량 소스 덱의 첫 프레임 8분(거대 평탄 소스 둘의 요약 없음, frames 걷기 27 s, 순차 open 34 s) | **완료 0.12.395(§8.18)**: 요약 마크 한도 2^36·`floe2 index`가 한도 옵션 전달·요약 없는 레이어의 로그 안내, 자동 셀이 셀/크기 상한을 보고 8/16/32 µm, 전체 깊이의 요약 서브트리 prune(픽셀 동일, fill 0 round), 덱 open 병렬(`jobs`) |
| 요약 상한 기본값 | 사용자 2026-10-24: 마스크 데이터는 얕고 레이어가 겹치지 않으며 pick을 안 쓰니 요약을 더 적극적으로 — 8/16 µm 대신 용량을 써서 4 µm를 지키고, Calibre처럼 fit의 4배까지 요약 | **완료 0.12.396(§8.19)**: 셀 상한 2^30 → 2^32, 크기 상한 1 → 4 GiB(152.4 mm 레티클 전체 bbox가 4 µm 그대로, 262 mm까지; 사다리는 그 너머), 합본 평면 캐시에 바이트 상한 512 MiB; 요약의 도달 범위(기준 셀 ≤ 1 px 규칙)는 그대로 — 758 px 창의 152 mm 덱에서 fit의 53배까지라 4배는 이미 안쪽, 확대 중 무거움은 요약 없는 pass의 몫 |
| 리뷰 수정 G | 0.12.396 리뷰 2건(자동 셀의 µm 판정 vs 정수 dbu 격자, u32::MAX 격자의 피라미드 산술 넘침) | **완료 0.12.397(§8.20)**: 자동 셀 후보를 빌더와 같은 정수 dbu 격자로 판정(`cell_dbu_of`; 3 nm 파일의 4 µm = 1333 dbu가 1,001²로 none:cells 나던 것이 8 µm 500² ok), 피라미드 halving `half_up`(div_ceil)로 u32::MAX 격자 27레벨, 로그의 셀 µm 표기 `um_text`(`8.001um`) |
| 경량 캐시 완성 | 사용자 2026-10-24: 16 GB 맥북에서 9.2 GB 합성 칩의 `--stream --mem 0.5`가 셀 예산에 거절 — "어떤 상황에서도 인덱싱" 검토 → "경량캐시를 완성해줘" | **완료 0.12.398(§8.21)**: 요약 패스의 배치 저장소(자식별 run, 메모리/임시 파일; 상한·건너뜀 없음, 매 open의 재시도도 없음), 파일 단위 깊이 상한(바이트 상한을 넘기면 셀을 굵히기 전에 깊이 평면을 접음: p10 93 → 449 레이어 ok), 게이트 light_cache·vfs_stream·occupancy |
| 경량 캐시 메모리 2단계 | §8.21의 측정: 파싱한 큰 셀의 실제 메모리가 Doc 추정의 약 3배, 피라미드가 끝까지 상주 → 전체 칩 `--mem 8`에서 14 GB | **완료 0.12.399(§8.22)**: 마킹 패스의 셀 파싱이 배치·텍스트를 만들지 않음(`parse_cell_bytes_shapes`, 도형 추정으로 과금), 묶음이 끝난 피라미드를 디스크에 내림(`Spill`, `design.planes.tmp`, 쓸 때 복사) — 바이트 동일; 측정은 §8.22 |
| 레이아웃의 요약 가지치기 | 사용자 2026-10-25: 맥북에서 경량 캐시의 fit이 오래 걸리고 `+` 8~9회부터 20초 넘게 걸림(밀도 off면 빠른 듯) | **완료 0.12.400(§8.23)**: 레이아웃 경로 일곱 곳의 `!frames`를 덱과 같은 `prune_summary_when`으로 — 요약이 그리는 뷰에서 걷기·probe·채움이 사라짐; 측정은 §8.23 |
| 파일이 없으면 거부 | 사용자 2026-10-25: ".ovs가 없으면 density on을 거부하고 ovo가 없으면 occupancy on을 거부하는게 맞는 동작"; 캡처의 명시적 `--occupancy on`은 요약을 먼저 만들고 그림 | **완료 0.12.401(§8.24)**: renderd가 design.ovs 없는 캐시의 밀도 요청을 끔으로 그리고(`density_none=nofile`, 플랜 폴백 없음) design.ovo 없는 명시적 `occupancy=on`을 오류로 거절; 캡처 `density_for`(입력 오류)와 뷰어 토글·시작 옵션의 거절; 게이트 density_stack·occupancy·light_cache |
| 조밀한 도형 격자의 외곽 채움 | 현장 2026-10-26: 레벨 5의 한 EBEAM 레이어 두 개가 `--occupancy-um 16`으로도 2^36 마크 상한(`work=68719508999`)에서 멈춤 — fill 배열의 다각형·경로가 멤버마다 charge | **완료 0.12.405(§8.25)**: prune에서 멤버 bbox ≤ 셀인 조밀 축 정렬 Grid의 다각형·경로를 외곽 한 번으로(prune 계약 안); `FLOE_OCCUPANCY_GRID_FILL=off` 킬 스위치; 단위 테스트 + occupancy 게이트 fixture |
| 작은 도형의 bbox 마킹, 크기 비례 상한 | 현장 2026-10-27: 290 GB EBEAM 파일의 레이어 3134/0이 0.12.405 뒤에도 `none:work`(`work 68719514762`, 빌드 3618 s) — 평탄한 다각형 레코드가 멤버마다 스캔 변환 | **완료 0.12.410(§8.26)**: prune에서 두 축 모두 셀 이하인 다각형·경로는 멤버마다 bbox의 셀(최대 2×2)을 칠함(`small_member_box`, 계약 안; `FLOE_OCCUPANCY_SMALL_BOX=off`), `--occupancy-max-work`를 주지 않으면 상한 = 소스 1바이트당 4, 최소 2^36(`default_max_work`); 단위 테스트 |
| 재고 매핑의 페이지, occupancy-only의 재고 재사용 | 현장 2026-10-27: 290 GB 파일의 `--occupancy-only`가 재고 패스에서 `rss 216G`(1920 s) — "stream이어도 어쩔 수 없는건지" → "진행해줘" | **완료 0.12.411(§8.27)**: `light --occupancy-only`는 캐시의 design.inv를 재고로 읽고 소스를 훑지 않음(`FLOE_LIGHT_INV_REUSE=off`), 재고 걷기가 지나간 페이지를 32 MiB마다 돌려줌(`floe_oasis::Release`, madvise DONTNEED; `FLOE_INVENTORY_RELEASE=off`), 진행 줄에 skim·parse 진행과 rss의 heap·file; 단위 테스트 + 게이트 light_cache |
| 덱 배율의 자동 셀 | 현장 2026-10-28: 모든 배치가 mag 4인 덱의 18147 × 12578 µm 뷰에서 `summary 4 passes ..., 21 passes without summary` — 27 mm 소스의 4 µm 셀이 덱에서 16 µm(`check_deck_summaries`: 135 소스 OK, base_um 0.5~4) | **완료 0.12.414(§8.28)**: 자동 셀이 배율을 본다(덱 셀 ≤ 4 µm: `placed_ceiling_um`, `--occupancy-mag`), `floe2 index DECK`가 소스의 최대 배율을 넘기고 그보다 굵은 요약을 다시 만듦, renderd의 대기 판정도 같은 셀, 점검 스크립트의 `mag`·`deck_cell_um`·`COARSE`; 단위 테스트 + 게이트 jobdeck |
| 인덱스 없는 캡처 | 사용자 2026-10-28: 인덱스 없는 레이아웃·잡덱도 `render`로 찍을 수 있게, 1회성이면 색인을 건너뛰는 것이 유리한지 | **완료 0.12.416(§8.29)**: `--temp-index[=DIR]`(이 실행만의 폴더에 색인·읽기·삭제, 옆에 아무것도 남기지 않음; 덱 광역 캡처는 기다리는 요약을 거기 만들고 다시 그림), 측정 - 캡처는 최소 경량 재고 pass 1회이고 색인을 "안 하는" 것은 디스크에 안 남기는 것일 뿐 시간은 같음; 게이트 `floe2`·`jobdeck` |
| 캡처 창 | 사용자 2026-10-29: 위치 캡처가 그 자리의 EBEAM 파일만 읽는지 - 그리기는 그랬지만 열기·자동 색인·임시 인덱스의 요약은 로드 레벨의 모든 소스(현장 레벨 9 = 135 소스 ~1 TB) | **완료 0.12.421(§8.30)**: 샷들의 영역을 덱 µm 창으로 모아(덱의 BX..UY extent, 소스 읽기 없음) 창이 닿는 배치의 소스만 스펙·색인 계획·요약 2단계에; `render --chipid/--chipnumber`; 빈 영역은 거절; `FLOE_DECK_WINDOW=off`; 3칩 덱 근접 9.3 → 4.7 s, 광역 15.4 → 6.9 s, 그림 바이트 동일; 게이트 `jobdeck` |
| L7 후속 | 요약 패스 1패스화/마킹 가속, ovs 합치기(레이아웃 밀도), B안(정규 페이지), 세그먼트 캐시 디스크 보존(방문한 셀의 디코드 결과를 남겨 다음 세션 가속), 1패스 경량 생성(셀 로컬 점유 격자 합성) | 각각 — **L7a 완료 0.12.386(§8.9)**: 이미지 증분 합성(제자리 성장, 채움 시점 노드 마스크, 채움 바이트 해제), 세그먼트 pread 묶음·병렬 인코딩; **L7b 완료 0.12.387(§8.10)**: 요약 패스의 마커가 셀을 직접 파싱(리더 풀·상주 창 없음, 묶음이 안 쓰는 셀은 읽지 않음), 다각형 스캔 변환 버퍼 재사용, 크기 예산 밖의 레이어는 마킹하지 않음; 나머지(1패스 경량 생성은 §8.10의 정확성 논거로 보류)는 사용자 지시에 따라 |

L0는 하루, L1~L3는 S1~S3와 비슷한 크기(재고 코드 위의 확장), L4가 본체다.
L4까지 끝나면 단독 소스를 경량으로 열어 볼 수 있고, L5·L6이 제품 경로다.

### 8.1 L0 구현 (0.12.374 / floe-index 0.12.337, 2026-10-21; 사용자 "L0 진행해줘")

- **명령.** `floe-index scan FILE [jobs] [--seg-bytes N]`(N은 K/M/G 접미사 허용,
  기본 256K). 0.12.360의 sizing 출력에 `segments` 블록이 더해지고 stderr에 한 줄이
  더 붙는다. `floe-index inventory`와 스트리밍 빌드의 재고 패스는 세그먼트를 모으지
  않는다(출력 바이트·시간 불변; `rust_inventory`·`vfs_stream` 게이트).
- **세그먼트**(`floe_oasis::segments`, lib.rs의 scan frame이 수집). 셀 안의 최상위
  CBLOCK 하나(압축 블록은 통째로 inflate하므로 더 잘게 못 읽는다; `bytes`는 header
  포함 레코드 전체, `inflated`는 uncomp-count), 또는 CBLOCK 사이와 CBLOCK 없는
  스트림의 구간(span: 누적 길이가 `--seg-bytes`를 넘은 첫 레코드 경계에서 닫히고,
  CELL 레코드·CBLOCK 시작·END에서도 닫힌다). 헤드(첫 보이는 CELL 앞)의 CBLOCK이
  정의한 셀은 세그먼트가 없다(재고의 `cblock_defs`와 같은 경우). 세그먼트마다 셀,
  종류, 오프셋, 바이트, 레코드 수, 도형 수·멤버, 배치 수, 텍스트 수, 도형 bbox(반복
  확장; 재고와 같은 `rep_box`·`path_bbox`·사다리꼴/CTRAPEZOID/원 꼭짓점), 도형의
  (layer, datatype) 집합, 체인 비트, 체인된 목록의 최대 크기를 적는다.
- **모달 체인.** 레코드가 앞 세그먼트에서 spelled-out된 목록 모달을 되쓰면 체인이다:
  반복 type 0이 점 목록 반복(4~7·10·11)을 되쓰면 `rep`(격자 반복은 스칼라라 제외),
  POLYGON의 P 비트 없음이 `poly`, PATH의 P 비트 없음이 `path`. 같은 세그먼트 안의
  재사용은 체인이 아니다. CELL 레코드에서 모달이 초기화되므로(스펙) 셀 경계를 넘는
  체인은 없고, 병렬 scan의 워커는 CELL 경계에서 시작하므로 결과가 jobs와 무관하다
  (단위 테스트가 jobs 1 = 3을 고정).
- **요약**(`segments` 블록, `floe_oasis::segments::summarize`). `count`·`cblocks`·
  `spans`·`span_bytes`; `bytes`·`inflated`·`records`(max/p99/p90/p50); `layers`(도형
  있는 세그먼트의 레이어 수 분위수); `with_shapes`/`with_placements`/`with_texts`;
  `class_by_count`·`class_by_bytes` — 세그먼트 bbox를 셀 bbox(그 셀의 도형
  세그먼트 bbox 합집합)와 견주어 **tile**(두 변 모두 ≤ 25 %), **band**(한 변만
  > 50 %), **full**(두 변 모두 > 50 %), **other**, **empty**(도형 없음);
  `overlap`(도형 세그먼트가 둘 이상인 셀의 "세그먼트 bbox 면적 합 ÷ 셀 bbox 면적"
  p50/p90/max와 그런 셀의 수); `chained`(세그먼트 수, rep/poly/path별, `list_max`);
  `cells`(바이트 상위 10셀: 세그먼트 수, 바이트, overlap, 분류, 레이어 p50, 체인
  수). stderr: `scan: segments N (C CBLOCK, S span of 256K): by bytes tile a% band b%
  full c% other d% empty e%; overlap p50 x p90 y (K cells with 2+ segments);
  layers/segment p50 u max v; chained C (p%), largest carried list L`.
- **판정.** (1) by bytes의 tile이 대부분이면 라이터가 공간 순서로 쓴 것 — 근접 뷰 =
  세그먼트 수십 개(§6.2 첫 줄). (2) band가 대부분이면 한 축 정렬 띠 — 근접 뷰 =
  창을 가로지르는 띠(셀의 띠 수 × 창/셀 변 비). (3) full이 대부분이면 세그먼트가
  셀 전체에 걸친 것 — 셀 전체 병렬 파싱이 하한. overlap은 분류와 따로 읽는다:
  레이어 수 근처(1~2)면 세그먼트들이 셀을 한 번씩 분할한 것이고, 세그먼트 수
  자릿수면 같은 자리를 여러 번 지나간 것이라 그만큼 더 읽는다(첫 판의 "띠 =
  overlap 10~1,000"은 겹쳐 지나가는 띠를 가정한 것이었다; 현장 결과로 정정).
  (4) `chained.segments / count`가 1 % 미만이면 §5.2의 스냅샷 설계 그대로, 그
  이상이면 경계를 목록 정의 직후로 옮기는 규칙과 `list_max`에 맞춘 스냅샷 상한을
  더한다. (5) `layers` p50이 1이면 세그먼트가 레이어별로 나뉘어 있어 레이어
  프루닝이 세그먼트 단위로 된다.
- **현장 절차.** 새 빌드로 `floe-index scan FILE 12 > scan.json 2> scan.err`
  (sh/bash; csh·tcsh에는 `2>`가 없어 "Ambiguous output redirect."가 나므로
  `(floe-index scan FILE 12 > scan.json) >& scan.err`)를 돌리고 `scan.err`의
  두 줄과 `scan.json`의 `segments` 블록을 붙여 주시면 된다. 시간은 scan과
  같다(현장 50 GB: scan 99.8 s + 파일 읽기 98.1 s; 아래 실측).
- **검증.** 단위 테스트 `floe_oasis::segments::tests`: CBLOCK 3개짜리 셀 + 평문 셀의
  손으로 쓴 스트림(박스·레이어·멤버·체인·바이트 경계·셀 bbox·요약값), span 길이
  1/60/무한(레코드 경계·CELL 경계·체인 유무), jobs 1 = 3, 평문 scan은 수집 없음,
  적대적 스트림 3종(도형 수 보존), 분류 경계. 게이트 `rust_scan`: `segments` 블록의
  일관성(count = cblocks + spans, 분류 합 = count, cblocks ≤ scan의 cblocks, 체인
  ≤ count, 상위 셀 ≤ 10, 셀별 분류 합 = 세그먼트 수)과 `--seg-bytes 4096`에서
  span이 줄지 않음·CBLOCK 수 불변.
- **실측(2026-10-21).** fixture 네 줄은 macOS 4 jobs, 모두 KLayout 또는 합성기가 쓴
  파일로, 라이터가 공간 순서로 쓰지 않을 때의 모습이다(KLayout은 셀의 도형을
  레이어 컨테이너 순서로 쓰므로 CBLOCK이 셀 전체에 걸친다 = full). 마지막 줄이
  현장 파일(Linux 12 jobs; 판정은 아래 '현장 결과').

| 파일 | scan | 세그먼트 | by bytes | overlap p50 (2+ 셀) | layers/seg p50 (max) | chained |
| --- | --- | --- | --- | --- | --- | --- |
| valmini 16 KB | 0.00 s | 8 (CBLOCK 3, span 5) | full 99 % | — (0셀) | 1 (3) | 0 |
| sample9 145 MB (CBLOCK, 40 레이어) | 1.14 s | 1,238 (CBLOCK 1,232) | full 99 %, band 1 % | 2.00 (49셀) | 6 (12) | 0 |
| route_chip 281 MB (CBLOCK 없음) | 2.85 s | 916 span | full 73 %, empty 16 %, tile 10 % | 9.9 (17셀; top 624) | 1 (4) | 0 |
| main01_chip_p10 953 MB (CBLOCK 없음, 배치 위주) | 9.6 s | 37,332 span | full 59 %, empty 41 % | 2.0 (301셀; top 74) | 5 (26) | 0 |
| **현장 50 GB 마스크 소스** (2026-10-21, Linux 12 jobs) | 99.8 s (파일 읽기 98.1 s 별도) | 785,523 (CBLOCK 781,767, span 3,756) | band 99 %, full 1 % | 1.74 (4,089셀; p90 2.14, max 24.65; top 0.91) | 1 (2) | 0 |

  `empty`는 도형 없는 세그먼트(배치·텍스트만)의 몫이라 배치 위주 레이아웃에서 크다.
  세 파일 모두 체인이 0인 것은 KLayout과 합성기가 점 목록을 레코드마다 다시 쓰기
  때문이고, 현장 라이터는 다를 수 있다. 시간은 0.12.360의 scan과 같은 자릿수다
  (sample9 1.14 s = 재고 패스 1.23 s와 같다).
- **현장 결과(2026-10-21, 50 GB 마스크 소스, Linux 12 jobs, floe-index 0.12.337).**
  레코드 19.70억, 셀 5,461; 세그먼트 785,523 = CBLOCK 781,767 + span 3,756(256K);
  세그먼트 bytes p99 89 KB·최대 7.8 MiB, inflated p99 207 KB·최대 17 MiB, records
  p99 4,389·최대 16,391(라이터가 CBLOCK을 약 16 K 레코드에서 끊는 듯); by bytes
  band 99 % full 1 %; overlap p50 1.74 p90 2.14 max 24.65(4,089셀); layers/segment
  p50 1 max 2; chained 0. 바이트 상위 셀 = 26 MiB(27,251,903 B), CBLOCK 341개 모두
  band(overlap 0.91, 레이어 1, 체인 0; 띠당 ≈ 80 KB, 레코드 ≈ 3,460). scan의
  sizing 줄이 말하는 `largest cell … 1,178,584 records, 415M`은 그 셀의 Doc
  추정치(`doc_bytes`)이지 파일 바이트가 아니다 — 첫 검토(§0.3·§4·§6.2·§9 B)는
  이를 파일 바이트로 읽었고, 위에서 정정했다. 시간: read_s 98.1 s(파일 전체
  읽기) + scan_s 99.8 s(524 MB/s) — 재고 패스 131.6 s와 같은 자릿수라 세그먼트
  수집이 더한 시간은 없다.
  **판정**: 라이터는 레이어마다 도형을 한 축으로 정렬해 CBLOCK 하나가 띠 하나가
  되게 쓰고(band 99 %), 띠들은 셀을 한 번씩 분할한다(overlap ≈ 레이어 수; 분류
  (2)의 가장 좋은 형태). 레이어 하나 = CBLOCK 하나이므로 레이어 프루닝이
  세그먼트 단위로 된다(5). 모달 목록이 CBLOCK을 넘지 않으므로 §5.2의 스칼라
  스냅샷 설계 그대로다(4). 근접 뷰 첫 방문(§6.2) = 창을 가로지르는 띠 10~20개
  ≈ 1~1.5 MB inflate·파싱, 수십 ms; 셀 전체도 341개 병렬 0.2~0.3 s. 세그먼트
  디렉터리는 78.6만 × 64~128 B ≈ 50~100 MB(§6.1의 1~3 GB/TB 안). 남은 미지수:
  overlap 최대 24.65인 셀 하나(크기 모름; p90 2.14이므로 드묾 — 그 셀은 세그먼트
  bbox로 걸러지는 몫이 적어 셀 전체를 읽는 쪽에 가깝다)와 띠의 정렬축·간격(셀의
  mm 크기가 있어야 창당 띠 수가 확정된다; L2는 bbox로 고르므로 정확성에는 무관).
  **결론: 설계대로 L1 → L2로 간다.** 셀이 파일에서 작다는 사실(최대 26 MiB)은
  §9 B(셀 단위 지연 색인)의 비용도 수 s로 내리므로, L1(재고 사이드카)만으로도
  첫 방문 수 s의 근접 뷰가 되고 L2(세그먼트)가 그것을 수십 ms로 줄인다 — 1차
  범위(§10 1번)를 정할 때 이 단계적 가치를 쓴다.

### 8.2 L1 구현 (0.12.379 / floe-index 0.12.338, 2026-10-21; 사용자 "제안대로 L1 진행해줘")

- **파일.** `design.inv` = 재고(`floe_oasis::inventory::Inventory`)의 직렬화, 형식
  FLOEINV1(SPEC-FORMATS "design.inv"): 72 B 헤더(magic, version, body_len, CRC-32,
  소스 identity src_size/src_mtime, n_cells, n_layers, top, unit) + 섹션 SCAN·LAYERS·
  HEAD·CELLS·NAMES·FOLD. 표는 키 오름차순으로 써서 한 재고의 바이트가 하나다(jobs
  1 = 3 = 4). 간선의 obox 8개는 쓰인 방향만(비트 u8). 정본 `rust/oasis/src/invfile.rs`
  (`encode`/`decode`/`peek`/`write`/`read`/`roundtrip`); 로더는 길이·개수·색인·마스크
  폭·CRC를 검사하고 어긋나면 위치와 함께 거절한다.
- **쓰는 곳.** 스트리밍 빌드(`floe-index vfs`/`floe2 index` 기본)가 뷰어 파일 뒤,
  design.ovm 커밋 전에 쓴다(`[vfs] stream: design.inv (12K, 7 cells, 9 layers)`);
  마커 프로토콜 밖(design.ovh처럼 없어도 캐시는 완전, 실패는 한 줄), 재빌드의 삭제
  목록(`clear_outputs`)에 있다. 인메모리 빌드는 재고가 없으므로 쓰지 않는다(그래서
  `vfs_stream`의 바이트 비교는 이 파일을 건너뛰고 따로 검사한다). `--occupancy-only`는
  아직 쓰지 않는다(L5의 열기 규칙과 함께).
- **명령.** `floe-index inventory FILE --out PATH`(쓰고 바로 읽어 같은 바이트인지:
  `"file": {"roundtrip": true}`), `floe-index inventory --load PATH [FILE] [--cells]
  [--check]`(읽어서 같은 JSON; FILE이 있으면 새 재고와 바이트 비교 `"load":
  {"identical": true}`; `--check`는 읽은 재고를 Doc에 댄다).
- **아직 아닌 것.** 재사용 — 빌드·재시작·덱 사전 점검이 design.inv를 읽어 재고 패스를
  생략하는 것(현장 131.6 s)은 identity 규칙·열기 규칙(L5)과 함께 한다; 렌더러가 읽는
  것은 L4.
- **검증.** 단위 테스트 `floe_oasis::invfile::tests`(적대적 스트림 4종 + head-CBLOCK 셀의
  왕복: 바이트 동일 + 필드 동일 + jobs 3 동일; 모든 길이의 잘림·본문 비트 반전·꼬리
  바이트·다른 magic·다른 version·top 범위·child 범위 거절; CRC 참조 벡터; 디스크
  왕복). 게이트 `rust_inventory`: fixture마다 `--out` 왕복, jobs 1 = 3 바이트, `--load
  --cells` = 새 `--cells`, `--load FILE` identical + `--check`, 잘린·손상 파일 거절.
  게이트 `vfs_stream`: 스트리밍 캐시의 design.inv가 `--out`과 같은 바이트·jobs 4와 같음,
  인메모리 캐시에는 없음, 인메모리 재빌드가 묵은 파일을 지움.
- **실측(macOS 4 jobs).** valmini 12 KB; sample9 859 KB(1,188셀·40 레이어; 전체 1.25 s
  중 쓰기 0.011 s); main01_chip_p10 64 MB(35,183셀·188,065 간선·449 레이어; 11.6 s 중
  쓰기 0.58 s) — §5.1의 추정 60~80 MB 안. 현장 50 GB(5,461셀·5 레이어)는 수 MB.

### 8.3 L2 구현 (0.12.380 / floe-index 0.12.339, 2026-10-21; 사용자 "L2 진행해줘")

- **수집.** 재고 패스가 같은 걸음에 세그먼트를 모은다(`floe_oasis::inventory_segments
  (data, jobs, span_bytes)` → 재고 + `SegCollector`; L0의 수집기 확장). 세그먼트 정의는
  §8.1 그대로(셀 안의 최상위 CBLOCK, 또는 구간). CBLOCK 안의 CELL 레코드는 그 블록을
  나누지 않고 `cells_inside`로 표시한다(독자가 블록 전체를 파싱해 셀별로 나눈다).
- **스냅샷(§5.2).** 세그먼트의 첫 레코드에서의 모달 변수: 스칼라(XY 모드, geo/pl/tx
  x·y, layer/datatype/textlayer/texttype, W/H, circle_r, ctrap_type, path hw/es/ee,
  배치 대상, 텍스트 참조)와 목록 필드(반복, 폴리곤 점 목록, 경로 점 목록, inline
  텍스트 문자열). 목록 필드는 레코드에 적힌 그대로의 바이트로 — 닫히는 세그먼트
  안에서 정의됐고 256 B 이하면 inline, 길면 참조(정의한 세그먼트·오프셋·길이), 더
  앞에서 정의됐으면 앞 스냅샷의 것을 그대로 — 복사는 경계마다 한 번뿐이라 비용이
  없다. 셀의 첫 세그먼트는 기본값(길이 0). 체인 통계(§8.1)는 그대로 남는다.
- **레이어별 통계.** 세그먼트 × (layer, datatype)마다 records, members, bbox(반복
  확장 합집합), max_w/max_h/max_min(단일 도형의 치수; 반복 제외) — 전체 색인의
  `emit_page`와 같은 식(RECT = w·h, POLYGON = 점 bbox, PATH = `path_bbox`, 사다리꼴·
  원 = 파서가 만드는 폴리곤의 bbox). 파서가 버리는 레코드(반지름 0인 원, 퇴화한
  사다리꼴)는 세지 않는다(재고와 같은 규칙). L0의 PATH 박스가 반복으로 넓혀지지 않던
  것을 함께 고쳤다.
- **파일.** `design.seg` FLOESEG1(SPEC-FORMATS): 80 B 헤더(CRC, 소스 identity,
  `inv_crc` = 짝인 design.inv의 본문 CRC, 개수) + CELLS(셀별 구간) + SEGS(112 B 행,
  (cell, offset) 순) + LAYERS(80 B 행) + SNAPS(varint 스냅샷 풀). 로더는 모든 범위·
  색인·짝·CRC를 검사한다. 정본 `rust/oasis/src/segfile.rs`.
- **쓰는 곳.** 스트리밍 빌드가 design.inv 뒤, design.ovm 커밋 전에 쓴다(`[vfs] stream:
  design.seg (…)`; `--no-seg`로 생략, `--seg-bytes N`); 재빌드의 삭제 목록. `floe-index
  inventory FILE --seg PATH [--seg-bytes N] [--check-seg]`, `--load-seg PATH`(짝·동일
  확인). design.inv는 세그먼트를 모았든 아니든 같은 바이트다(스캔의 셀 레코드 박스는
  파일에 넣지 않는다).
- **게이트(§8 "세그먼트 통계 = 전체 Doc에서 계산한 값").** `--check-seg`: Doc(정규화
  전)의 셀·레이어별 통계를 같은 식으로 계산해 세그먼트들의 합(records, members)·
  합집합(bbox)·최대(max_*)와 비교한다; `cells_inside`가 있는 셀은 건너뛴다(개수 보고).
  `rust_inventory`: fixture마다 `--seg` 왕복, jobs 1 = 3 바이트, `--check-seg` OK,
  `--load-seg` 짝·동일, design.inv를 design.seg로 읽기·손상 거절. `vfs_stream`:
  스트리밍 캐시의 design.seg = `--seg` 바이트·jobs 4와 같음·짝, 인메모리에 없음, 묵은
  파일 제거, `--no-seg`. 단위 테스트: `segments::tests`(CBLOCK 3개 셀의 스냅샷 내용 —
  레이어·크기·좌표·inline 목록 바이트·경로 파라미터·앞 세그먼트의 목록 계승 — 과
  레이어별 통계, 파서가 버리는 레코드), `segfile::tests`(적대적 스트림 5종의 왕복·셀
  구간·레이어 행·기본 스냅샷·jobs 3 동일, 모든 길이의 잘림·손상·소스 밖 거절, 스냅샷
  전 필드 왕복).
- **실측(macOS 4 jobs).** design.seg: valmini 2 KB; sample9 931 KB(1,238 세그먼트,
  레이어 행 9,756, 스냅샷 2.3 KB); route_chip 217 KB; main01_chip_p10 22.7 MB(37,332
  구간, 레이어 행 226,776, 스냅샷 77 KB). 재고 패스 시간: sample9 1.23 → 1.39 s, p10
  11.1 → 12.2 s(+10~13 %). 현장 50 GB(78.6만 세그먼트, 레이어/세그먼트 1, 체인 0)는
  약 150~180 MB, 재고 131.6 s의 +10~15 %로 본다 — 다음 `floe2 index`가 답한다.
- **다음.** L3 시드 파서(`parse_segment`: 스냅샷으로 모달을 채우고 CBLOCK을 inflate해
  세그먼트 하나를 Doc으로; 참조 필드는 정의한 세그먼트에서 가져온다), 게이트는
  세그먼트 파싱의 레코드 합 = 셀 파싱(G2급).

### 8.4 L3 구현 (0.12.381 / floe-index 0.12.340, 2026-10-21; 사용자 "L3 진행해줘")

- **시드 파서.** `floe_oasis::doc::parse_segment(bytes, base, cell, snapshot, fields,
  tables) -> Vec<CellPiece>`: 세그먼트의 바이트(구간의 레코드들, 또는 CBLOCK 레코드
  하나 — 파서가 inflate)를 그 스냅샷의 모달 상태에서 파싱한다. 스칼라는 그대로
  `Modal`에 넣고, 목록 필드(반복·폴리곤·경로 점 목록·inline 텍스트 문자열)는
  레코드 자신이 지나는 같은 판독기(`read_rep`, `read_points`, 문자열)로 바이트에서
  복원한다(남는 바이트가 있으면 거절). 셀은 재고의 이름으로 미리 열어 두고(CELL
  레코드 없이 시작) `parse_records`를 depth 1로 돌린 뒤 `parse_cell_bytes`와 같은
  끝처리(`pieces_of`: 배치 대상 → Doc 색인, 텍스트 참조 → 문자열)를 한다. 결과는
  정규화 전의 레코드이고 전체 파싱과 같은 형태다. 세그먼트 안의 CELL 레코드는 그
  셀의 조각을 더하고, 이름 표 레코드는 소비만 한다(참조는 재고의 표가 푼다).
- **참조 필드.** `Snapshot::seed_fields(fetch)`가 inline 필드는 그대로, 참조 필드는
  `fetch(seg, off, len)`으로 바이트를 채운다. 파일 전체를 가진 독자의 구현이
  `segments::segment_stream(data, offset, bytes, cblock)`(CBLOCK이면
  `inflate_cblock_record`)과 `fetch_from(data, segs, …)`이고, L4의 LightVfs는 pread +
  inflate로 같은 일을 한다.
- **게이트(§8 "세그먼트 파싱의 레코드 합 = 셀 파싱").** `floe-index inventory FILE
  --check-seg`가 통계 비교에 더해 모든 세그먼트를 스냅샷으로 파싱해 셀별로 이어
  붙이고, 같은 셀의 `parse_cell_range`(head + 모든 범위, 파일 순서) 결과와 레코드
  단위로 비교한다(`"check_seg": {"stats_ok", "parse_ok", "pieces", "parse_s"}`; head
  CBLOCK이 정의한 셀은 세그먼트가 없어 건너뛴다). `rust_inventory`가 fixture마다
  본다. 단위 테스트 `segments_parse_with_their_snapshots_to_the_cell_parse`: 스트림
  8종 × 구간 길이 1/60/기본(레코드마다 세그먼트인 경우까지), 130점 폴리곤 목록을
  참조로 잇는 fixture(Ref 경로: 둘째 CBLOCK의 스냅샷이 첫 CBLOCK의 바이트를 가리키고
  파싱 때 가져온다), inline 텍스트·이름 배치 대상·격자 반복을 구간마다 잇는
  fixture, 남는 바이트가 있는 시드의 거절.
- **실측(macOS 4 jobs, `--check-seg`의 parse 부분 = 세그먼트 전부를 스냅샷으로 파싱 +
  셀 파싱 + 비교, 단일 스레드).** valmini 8 조각 0.0 s; sample9 1,238 조각 2.5 s(전체
  4.7 s); route_chip 916 조각 3.0 s(전체 9.1 s); main01_chip_p10 37,332 조각 24.5 s(전체
  56 s). 모두 OK, 건너뛴 셀 0.
- **다음.** L4 LightVfs: design.inv/seg로 Ovm을 합성하고(셀 디렉터리 ← 재고, 페이지
  디렉터리 ← 세그먼트 × 레이어), 페이지 페이로드를 원본에서 pread + inflate +
  `parse_segment`로 디코드한다(§5.3).

### 8.5 L4 구현 (0.12.382 / floe-index·renderd 0.12.341, 2026-10-22; 사용자 "L4 진행해줘")

- **파일.** `floe-index light SRC [--out DIR] [--jobs N] [--seg-bytes N]`(cli light.rs)이
  재고 패스 한 번으로 design.inv·design.seg(짝)와 meta.json(전체 색인의 키 +
  `"cache": "light"`, §5.1)을 DIR(기본 `.<SRC>.ice`)에 쓴다. design.ovm이 있는
  디렉터리는 건드리지 않는다(종료 1; 스트리밍 빌드가 이미 inv/seg를 옆에 둔다).
  writer는 `floe_vfs::light::write_files`로 두 명령이 같은 바이트를 쓴다(게이트가 본다).
- **열기(`floe_vfs::light::LightIndex`).** render-core `Cache::open`은 design.ovm이 없고
  design.inv가 있으면 경량으로 연다: inv를 읽고(CRC), seg가 그 inv와 짝인지(`inv_crc`,
  identity, 셀·레이어 수), 소스(meta.json `src.path`, 없으면 `.NAME.ice` → `NAME`)가
  파일을 만든 소스인지(size, mtime) 확인하고 pread용으로 연다. 거절: CBLOCK 안에 정의된
  셀이 있는 스트림(세그먼트 한 개가 두 셀의 레코드를 가져 디렉터리 행이 가르지 못한다 —
  전체 색인이 읽는다), 바뀐 소스, 짝이 아닌 seg, 소스를 말하지 않는 디렉터리.
- **합성 Ovm(§5.3 1·2·4).** `LightIndex::ovm()`이 `floe_ovm::Builder`로 이미지를 만들어
  `Ovm::from_bytes_shallow`(open의 얕은 검증)로 연다: 셀 = 재고 전부(이름·height·rank·
  dbox·rbbox·dmask/rmask/rtmask·rmembers = PrePass::from_inventory의 값), 페이지 = 세그먼트
  × 레이어 행(cell, layer_idx, bbox, records, members, max_w/h/min은 행 그대로, `file_off` =
  소스 오프셋, csize/usize_ = 세그먼트 바이트를 레이어 레코드 비율로 나눈 몫 — 한 레이어
  세그먼트는 전부, 여러 레이어 세그먼트는 몫; 코덱 `CODEC_SEGMENT` = 1, 디스크에는 없는
  값; `ovp_len` = 소스 크기), (셀, 레이어) 런은 세그먼트 순서 + `split_pbvh`(전체
  빌드의 finish_layer와 같은 함수 — split_bvh/split_pbvh는 cli에서 floe_ovm으로 옮겼다),
  텍스트 표는 비어 있다(라벨은 L6). 배치·인스턴스 BVH는 처음엔 없다.
- **채움(§5.3 3, §5.4).** `floe_ovm::Ovm`에 `LightMarks`가 붙는다: 이미지에 배치가 없는
  셀(재고는 있다고 한다)과 그중 독자가 요구한 셀. 플래너의 `expand`(hier.rs), 라벨
  walk(text.rs), 접근자 `cell_places`/`cell_bvh`가 `light_touch(ci)`로 표시한다(전체
  색인에서는 아무것도 하지 않는 한 분기). render-core의 모든 플랜 진입점(`plan`, fit
  decision, fit_plan, labels, `layer_held`, 셀 트리의 instances/info walk)은
  `LightLink::run`으로 돈다: 플랜 → 표시된 셀이 있으면 채움 → 이미지 재합성 → 다시 플랜,
  표시가 없을 때까지(한 라운드가 계층 한 단계; 상한 64). 표시는 늘기만 하고 채움은
  멱등이라 두 스레드가 한 이미지에서 같이 플랜해도 안전하다. 채움(`LightIndex::fill`)은
  셀의 배치 플래그 세그먼트를 pread + inflate + `parse_segment`(병렬)하고 재고의 배치 수와
  맞는지 확인한 뒤 build_cell_plan과 같은 항목 박스·v7 주석·`split_bvh`·`CellSink`로
  인코딩해 둔다(v8 노드 마스크는 합성 때 `annotate_bvh_masks`). 선채움: 요구된 셀 아래의
  셀도 `FLOE_LIGHT_PREFILL_PLACES`(262,144)·`FLOE_LIGHT_PREFILL_MB`(4) 안에서 같이 읽는다
  (sample9 근접 뷰 3라운드 30 ms; 전부 읽으면 652 셀 178 ms, 안 읽으면 5라운드 17 ms).
  상주 예산 `FLOE_LIGHT_PLACES_MB`(1 GiB)를 넘으면 채움을 오래된 순서로 비우되, 지금 요구한
  셀과 진행 중인 요청이 플랜한 이미지에서 읽은 셀(`Ovm::light_used`)은 비우지 않는다 — 요청의
  작업 집합이 예산보다 크면 그대로 들고 끝내고(라운드 로그 `over the places budget, kept`), 다음
  요청의 채움이 쓰지 않는 셀을 비운다(0.12.385, §8.8; 그 전에는 앞 라운드가 채운 셀까지 비워
  64라운드까지 되채웠다). 인덱스 교체는 `VfsSlot`(RwLock<Arc<Vfs>>)로,
  Cache와 HierHandle이 같은 슬롯을 본다; 한 요청은 집은 스냅샷을 끝까지 쓴다.
- **디코드(§5.3 5).** `Cache::decode_pages*`와 DecodePool은 경량이면 render-core
  `light::decode_pages`로 간다: 요청 페이지를 세그먼트별로 묶고 파일 순서로 pread, 세그먼트
  하나를 스냅샷으로 한 번 파싱해(워커 N) 레이어별 한 셀 Doc(`page_cell_name`, 그
  레이어의 rects/polys/paths만, 배치·텍스트 없음)으로 나눠 `DecodedPage`를 만든다
  (PageIndex, decode_shrink 동일). 정규화는 하지 않는다 — 멤버 집합이 같으므로 픽셀이
  같다(게이트). `read_page_batch`는 CODEC_SEGMENT 페이지를 거절한다.
- **요약·셀 트리(§5.3 6).** `LightIndex::summary()`가 재고 간선(obox 8방향 ⊕ 자식 rbbox의
  방향 박스 = 간선 extent, members, rank 순서의 insts)으로 design.ovh 바이트를 만든다
  (`hiersum::build_from_edges`, build()와 공유); 단위 테스트가 전부 채운 이미지의
  `hiersum::build`와 바이트 동일임을 본다. `layer_depth`도 재고 자식 표로 센다.
- **게이트 `light_cache`(tools/validate_light_cache.py).** valmini·KLayout 재작성 2종·
  gen_route_chip 0.05·gen_main01_like 0.001: `floe-index light`의 inv/seg = 스트리밍 빌드의
  바이트, meta.json 키, floe-render-cli로 exact 뷰 5개(전체·근접·근접 depth 1·전체 depth
  1·두 레이어) PNG 동일 + 플랜 wc_cells/inst_edges/frame_rects 동일(페이지 수와 members는
  다르다: 경량 페이지는 세그먼트의 것이라 구간의 bbox가 도형 없는 뷰에 닿는다), cut 3 px
  전체 뷰 차이 ≤ 빛나는 픽셀의 0.5 %(실측 0), 거절(전체 색인 디렉터리, 바뀐 소스, 이름
  없는 디렉터리). 단위 테스트: floe_vfs `light::tests`(스트림 5종: 열기·이미지 검증·
  마크·선채움 집합·전부 채움 = 재고 배치 수·요약 바이트 동일·딥 검증; 페이지마다 레코드·
  멤버 = 세그먼트 파싱의 그 레이어, 셀·레이어별 멤버 합 = parse_doc; 거절 4종),
  render-core `light::tests`(경량 Cache 열기, exact 플랜 + 라운드, 재플랜에 채움 없음,
  전 페이지 디코드의 레이어별 멤버, 풀 경로 동일, read_pages 거절, 셀 트리 children·
  instances 17·info).
- **실측(macOS 4 jobs, floe-render-cli, 전체 색인 → 경량; `FLOE_LIGHT_LOG=on`이 라운드
  줄을 낸다).** valmini: 뷰 4개 동일, 1라운드. sample9(1,188 셀, 1,450만 레코드; light
  파일 1.2 s, inv 859 K + seg 909 K): 칩 전체 exact 9,760 → 9,756 페이지, 2.51 → 2.79 s;
  60 µm 근접 뷰 4 페이지 = 4 페이지, 0.02 → 0.06 s(3라운드). route_chip(29 셀, 916 구간,
  CBLOCK 없음): 100 µm 근접 뷰 동일하나 경량이 765 페이지 / 2,080만 레코드를 고른다(전체
  38 / 132만; 1.34 s 대 0.09 s) — 이 writer는 레코드를 칩 전체에 흩뿌려 구간마다 bbox가
  칩이다(L0의 "full" 등급, §4·§7의 지역성 위험이 그대로 보인다; KLayout·현장 writer는
  띠다). main01_chip_p10(35,183 셀, 1억 레코드; light 파일 9.4 s, inv 64 M + seg 22 M):
  40 µm 근접 뷰 동일, 168 대 157 페이지, 1.02 s 대 0.13 s — 1라운드가 top의 배치 280만
  개(구간 103개)를 481 ms에 읽고 265 MB 이미지를 156 ms에 합성한다; 2 mm 창 cut 3 px:
  4라운드, 채운 배치 1,040만 개, 이미지 901 MB, 3.4 s 대 0.33 s. 이미지 = 방문한
  셀의 배치(64 B/개 + BVH; 전체 색인은 같은 것을 디스크에 두고 mmap), 채움 = 전체 빌드가
  색인 때 하던 일. 현장 마스크(배치 5,461개)는 top 한 라운드, 수 ms다.
- **남은 것(L7로).** 이미지 증분 합성(지금은 라운드마다 정적 섹션을 다시 인코딩·복사:
  p10 150~300 ms/라운드), 세그먼트 pread 병렬화·묶음(NFS), 텍스트 색인(라벨, L6), 채운
  셀의 최근 사용 추적(지금은 채운 순서로 비운다), 세그먼트 지역성이 나쁜 writer의
  근접 뷰(route_chip류: B안 또는 세그먼트 재분할).

### 8.6 L5 구현 (0.12.383 / floe-index·renderd 0.12.342, 2026-10-22; 사용자 "L5 진행해줘")

- **`floe-index light`(cli light.rs).** `SRC [--out DIR] [--jobs N] [--seg-bytes N]
  [--occupancy [--occupancy-um UM] [--occupancy-balance 0|1] [--occupancy-prune 0|1]
  [--occupancy-max-*]] [--occupancy-only] [--mem GiB] [--force] [--hold-at …]`. 소스는
  스트리밍 빌드의 `stream_open`으로 매핑해 재고 패스 한 번(L4는 파일을 통째로 읽었다 —
  50 GB에서는 안 된다); design.inv → design.seg(짝) → meta.json(커밋 마커, 마지막) 순으로
  tmp + rename. `--occupancy`는 S5의 스트리밍 요약 패스(`stream_occupancy`)인데, 인스턴스의
  배치를 파일 전체를 다시 파싱하는 places 패스가 아니라 방금 쓴 세그먼트 디렉터리의
  배치 플래그 세그먼트에서 읽는다(`LightIndex::all_placements` → `StreamStore::set_places`).
  `--occupancy-only`는 경량 캐시에 design.ovo를 더한다(identity 확인; 전체 색인이나
  경량 캐시 없는 디렉터리는 종료 1). design.ovm이 있는 디렉터리는 `--force`에만 손댄다
  (전체 색인의 파일을 지우고 경량으로 교체; `clear_outputs`). 잠금은 `vfs`와 같다: 빌드는
  Full(독자는 meta.json까지 배타, 그 뒤 `release_use`로 요약 패스 동안 열 수 있다),
  `--occupancy-only`는 Additive에 기존 design.ovo를 바꿀 때 독자 배타. `--hold-at
  locked:F`/`committed:F`로 게이트가 경쟁시킨다. 덱(`floe-index light DECK.jb`)은 옆의
  floe2에 `index DECK --light`로 넘긴다(`--out` 거절). 배치 수가 요약 패스의 상한을 넘으면 한 줄
  알리고 요약을 건너뛴다 — 상한은 0.12.385부터 `--mem`(기본 예산)의 4분의 1을 배치당 약 96 B로
  나눈 수(`vfs::light_places_cap`; 현장 32 GiB 기본 ≈ 9,000만, 그 전에는 vfs와 같은 5,000만 고정;
  §8.8). CBLOCK 안에 정의된 셀이 있는 스트림은 아무것도 쓰지 않고 종료 79로 거절한다(§8.8).
- **`floe2 index`(app-core index.rs, app-cli).** `IndexOptions.light: Option<bool>` —
  `--light`/`--full`(배타); 없으면 `full_only()`(--lod, --page-target-mb, --representatives*,
  --hier-only, --profile-*, --stream, --in-memory, --slow-cell-s, --p2-shard-limit-mb 중
  하나가 있으면 전체; --no-ovs·--no-page-occupancy는 세지 않는다 — 덱 소스가 --no-ovs를
  달고 온다) → `FLOE_INDEX_LIGHT=on|off`
  → 규칙(덱 소스 경량, 레이아웃 전체; `light_for(deck_source)`). `--light`에 전체 색인만의
  옵션이 오면 거절. `decide()`가 캐시의 종류(`cache::cache_kind`: design.ovm이면 전체,
  아니면 design.inv면 경량)와 요청을 함께 본다: 경량 캐시 위에 전체 요청이면 `--force`
  없이 빌드(잃는 것이 없다 — 스트리밍 빌드가 inv/seg를 옆에 남긴다; 안내
  `the light cache of X becomes the full index`), 전체 색인 위에 경량 요청이면 유지(안내
  `X has the full index …; kept (--force --light replaces it)`), `--representatives-only`·
  `--hier-only`는 경량 캐시를 거절, `--occupancy-only`는 경량 캐시면 `light --occupancy-only`.
  `arguments()`가 `light SRC --out DIR --jobs N [--occupancy …] [--force] [--mem G]`를 만든다.
  덱(`jobdeck::index::DeckIndexPlan`)은 소스마다 `light = light_for(true)`를 정해 넘긴다;
  자동 색인(`ready.rs`)은 기본 옵션이라 규칙이 그대로 적용된다(레이아웃 전체, 덱 소스
  경량 + 요약). 웹의 revision 빌드는 전체 색인이다(design.ovm/ovp를 봉인한다).
- **원본 파일 핸들.** `LightIndex`는 원본의 fd를 들고 있지 않는다: 읽기마다 프로세스 공용
  풀(`floe_vfs::light` SOURCE_FILES, 최대 8개, LRU)에서 꺼내 pread한다. 덱은 소스마다 경량
  캐시 하나라 수백 fd가 되고, 전체 색인은 파일을 매핑하고 페이지 파일은 읽을 때마다 열어
  fd를 쥐지 않는다 — `ulimit -n 32`로 24개 소스 덱을 그리는 index_lock K7이 잡았다.
- **열기·정보(app-core cache.rs, catalog.rs, dataset.rs, clip.rs).** `cache::inspect`와
  `Layout::open_directory`가 `validated_identity()`로 종류별 identity를 본다: 전체는
  `validated_vfs`(ovm), 경량은 `floe_vfs::light::check()` — inv·seg의 **헤더만**(각 72 B·
  80 B; 길이·짝 `inv_crc` = inv의 body CRC·셀/레이어 수; 64 MB 재고를 열 때마다 읽지
  않는다). `Layout.kind`, `Metadata.cache`(meta.json의 `"cache": "light"`), `floe2 info`의
  `cache … (…, VFS v1, light: the geometry is read from the source)`와 design.inv/seg/ovo
  크기(ovo 없으면 `none (no wide-view summary yet: index --occupancy adds it)`),
  `info --json`의 `cache_kind: full|light`. `floe2 clip`은 경량 캐시를 거절한다(페이지
  파일이 없다: `clip needs the full index of X …; index --full`). renderd·덱 합성은 L4의
  `Cache::open` 그대로다.
- **게이트.** `floe2`(validate_floe2.py `validate_rust_cli`): `index --light`의 파일,
  `info` 문구·JSON, `render --detail exact --labels off`가 전체 색인과 PNG 동일(라벨은
  경량 캐시에 텍스트 색인이 없어 L6까지 빠진다; 레이어 색은 같다 — 경량 meta.json의
  `stored_shapes`가 텍스트만 있는 레이어도 센다: 그 레이어를 텍스트 마스크에 가진 셀들의
  텍스트 수, 셀의 텍스트가 한 레이어에 있으면 정확하고 아니면 상한), `clip` 거절, 경량
  유지·전체로 올리기·`--light` 유지·`--light --force` 교체, `--light --lod` 등 거절,
  `FLOE_INDEX_LIGHT=on`; `jobdeck`(test_3: 덱 소스 3개가 경량 + ovo, `floe2 info deck`;
  LOD 테스트: 평범한 `--force`는 경량, `--full`은 전체); `index_lock` K13(`light`끼리의
  build 잠금, 독자가 있는 `--force`와 `floe2 index --light --force` 75, `--occupancy-only`의
  추가는 되고 교체는 75); `light_cache`(design.ovo = 스트리밍 빌드의 바이트,
  `--occupancy-only` 동일, 전체 색인·빈 디렉터리 거절, `--force` 교체). 단위 테스트
  app-core `the_light_cache_decisions`, app-cli `the_light_cache_is_a_mode_of_its_own`.
- **실측(macOS 4 jobs).** `floe-index light --occupancy` 대 `vfs --occupancy`(스트리밍):
  valmini ovo 동일; sample9 4.3 s(재고 1.4 + 배치 0.3 + 요약 2.9) 대 6.5 s, ovo 1.07 GB
  동일; route_chip 6.8 s 대 17.5 s, 동일. 경량 생성 = 재고 패스 + 요약 패스이고 본 빌드
  (플랜·인코딩)가 없다 — 현장 50 GB에서는 §6.1의 131.6 s + 요약 패스(기본 예산 4~6 min
  예상)만 남는다.
- **남은 것.** 뷰어 상태줄의 `cache: light` 표기·요약 없는 동안의 배너·덱 소스 경량
  열기의 refine 흐름, 텍스트 색인(라벨) — L6(§8.7)에서 했다; 세그먼트 지역성이 나쁜
  writer, 이미지 증분 합성(L7).

### 8.7 L6 구현 (0.12.384 / floe-index·renderd 0.12.343, 2026-10-22; 사용자 "L6 진행해줘")

- **텍스트 색인(라벨).** 경량 캐시의 채우기가 배치와 함께 텍스트를 읽는다
  (`floe_vfs::light`: `pieces_of`가 F_PLACEMENTS|F_TEXTS 세그먼트를 파싱, 재고의 수와 대조;
  `CellFill.texts`; `needs_fill` = 배치 또는 텍스트가 있는 셀). `image()`가 채운 셀마다
  전체 빌드의 `build_cell_texts`와 같은 규칙으로 텍스트 표를 쓴다 — 레이어별 run, Morton
  순서(원본 순서 tie-break), TBVH_LEAF 넘으면 텍스트 BVH(`floe_ovm::split_tbvh`로 옮김),
  문자열과 Pts 반복의 점 목록은 이미지의 design.ovt 바이트(`Vfs::light(ovm, source,
  ovt)`; `ovt_len`은 헤더에). `LightMarks`는 배치 표시와 텍스트 표시를 나눠 든다: 배치
  리더(`cell_places`/`cell_bvh`, 플래너의 expand)는 배치가 빠진 셀만, 라벨 걷기
  (`cell_tranges`)는 텍스트가 빠진 셀도 요청하므로 텍스트만 있는 셀은 기하 플랜에 라운드를
  더하지 않는다. 결과: `floe2 render --detail exact`가 라벨을 켠 채 전체 색인과 픽셀 동일
  (floe2 게이트; 스모크 fixture의 텍스트 레이어). 단위 테스트 `labelled_tree`(floe_oasis
  fixtures): 셀·레이어별 텍스트 = Doc, raw 라벨 걷기 = 평탄화 33개, render-core의 라벨
  플랜이 텍스트만 있는 셀을 채운다.
- **refine 흐름.** §5.4의 구현 항목. `PlannedView::light_pending`, `FitProbe::pending`,
  `PlannedLabels::pending`, `Cache::plan_cancellable_within` / `fit_decision_cancellable_within`
  / `plan_labels_within` / `plan_with_representatives_within` / `plan_within`; renderd
  `run_render`의 `LightProgress`(발행한 라운드 수·시도 수, 256 시도 상한)와 덱의
  `run_deck_render_attempt`(라운드 = 시도 번호, 미완이면 `.partial.` 경로에 `final=0`). 캡처는
  `progressive=0`(worker-client `RenderRequest::progressive`, app-core `base_request`).
- **요약 없는 상태.** §5.5의 구현 항목. render-core `DeckSource::summary_pending`,
  `DeckRenderReport::summary_building_passes`, `DeckInfo::light_sources/summary_missing`,
  `Cache::has_occupancy_file`; worker-client `Frame::summary_building`(완료 판정에 포함),
  app-core `require_complete` 문구, perf 줄 `, N passes wait for their occupancy summary
  (light sources: empty until it is built)` / 요약 줄 `, N wait for the summary (building)`
  (오라클 perf_line 동일), `DeckInfo`(jobdeck/dataset.rs) `light_sources`/`summary_missing`,
  `desktop::Readiness::summary_missing`; app-cli `ready.rs`의 `deck_state`가 요약 없는 소스도
  색인 대상으로 보고 `index_deck`이 occupancy-only 항목을 돌린다. GTK(gui.py):
  `_summary_banner`·`_summary_banner_update`·`_summary_poll_tick`(3 s)·`_summary_check`(열기
  뒤 meta의 `summary_missing` > 0이면)·`_summary_job_start/_done/_stop`(뒤의 `floe2 index
  deck.jb [--level]`, 진행 `occupancy : (k/n)` 파싱, 종료 시 다시 그림, `_quit`에서 killpg),
  `_jobdeck_index_and_load`의 1단계 `--no-occupancy`, 깊이 라벨 ` · cache:light` /
  ` · light:N/M`, refining 문구 "reading the source… (fill round N)".
- **게이트.** floe2(라벨 켠 exact 비교 + 라벨 없는 그림과 다름), jobdeck
  `LightSummaryTests`(위 표), gtk_view `light_status`, 기존 light_cache·occupancy·index_lock
  은 그대로; 오라클 `rust_render.py`/`perf_line.py`에 `summary_building`·`light_rounds`.
- **실측(macOS 4 jobs).** 단위·게이트 fixture 수준; 현장 수치는 사용자 실행 뒤.
- **남은 것(L7).** 이미지 증분 합성(라운드마다 정적 섹션 재인코딩: p10 150~300 ms),
  세그먼트 pread 병렬화·묶음, 채운 섹션의 recency 예산, 지역성이 나쁜 writer의 B안, 웹
  (feature/webui)의 배너·상태 표기(PatchDto).

### 8.8 L6 후속 수정 (0.12.385 / floe-index·renderd 0.12.344, 2026-10-22; 사용자 "그 순서로 수정 진행해줘")

테라바이트급 현장 덱을 바로 렌더해도 되는지 보려고 p10(909 MB)에 경량 캐시를 만들어 재다 찾은
세 가지. 측정(macOS 4 jobs, `floe2 render` 뷰어 기본 설정, 수정 전): fit 뷰 전체 색인 3.7 s /
1.3 GB, 경량 25.4 s / 10.5 GB; 40 µm 근접 뷰 0.20 s / 0.33 GB, 경량 1.3 s / 1.4 GB; render-cli
exact fit은 경량에서 64라운드 228 s 11 GB 뒤 오류(전체 색인은 raster 55분 넘겨 중단 — 554 M
멤버를 1024 px에 그리는 병적인 경우). 수정 뒤: 같은 exact fit이 4라운드(계층 깊이만큼, 비움 0)에
플랜을 끝낸다 — 마지막 이미지 6.9 GB(예산 1 GiB를 5.8 GB 넘긴 채 들고), 플랜 한 번 46 s, 최대 RSS
10.2 GB; 뷰어 기본 fit은 21 s / 10.7 GB로 전과 같다(되채움이 없던 경우). 경량 fit의 시간·메모리
자체는 L7(증분 이미지 합성)의 몫이다.

- **채움 예산이 자기 요청을 밀어내던 것.** `LightIndex::fill`의 예산(FLOE_LIGHT_PLACES_MB
  1 GiB)은 이번 호출이 요구한 셀만 지켰다. 한 뷰가 펼치는 셀의 배치 섹션이 예산보다 크면(배치당
  약 68 B: 1,500만 개쯤) 앞 라운드가 채운 셀이 밀려나고 다음 라운드의 플랜이 다시 요구해 되채우는
  되돌이가 MAX_FILL_ROUNDS(64)까지 이어졌다 — 뷰어라면 "fill round N" 프레임이 몇 분 이어지다
  오류, 캡처는 종료 1. 수정: floe_ovm `LightMarks`에 `used` 비트(`light_touch`/`light_touch_texts`가
  이미지에서 배치·텍스트를 읽은 셀마다 켠다, 들고 있든 아니든; `Ovm::light_used`),
  `LightIndex::fill_keeping(cells, keep)`(render-core `fill_into`가 플랜한 이미지의 used 집합을
  넘긴다)은 요구한 셀과 keep을 비우지 않고, 그 둘만으로 예산을 넘으면 그대로 들고 끝낸다
  (`FillReport.over_budget`, 라운드 로그 `; N MB over the places budget, kept (the request uses
  them)`). 다음 요청의 채움이 쓰지 않는 셀을 비운다 — 예산은 요청 사이의 상주 한도다. 테스트
  render-core `budget_tests`(두 팔 트리, 예산 1 B: 요청이 2라운드에 끝나고 아무것도 비우지 않는다,
  다른 팔의 요청이 앞 팔을 비운다, 돌아오면 되읽는다, 기본 예산은 아무것도 비우지 않는다).
- **CBLOCK 안에 정의된 셀이 있는 소스.** `floe-index light`가 경고만 하고 캐시를 썼고 렌더러가
  그 캐시를 거부해(L4) 덱이 통째로 열리지 않았다. 수정: 재고 패스 뒤 `cells_inside` 세그먼트나
  `cblock_defs` 셀이 있으면 아무것도 쓰지 않고 종료 79(`floe_vfs::light::UNSUITABLE_EXIT`);
  `floe2 index`는 규칙이 경량을 고른 소스(덱 소스 — `IndexOptions.deck_source`, 덱 계획이 켠다 —
  또는 FLOE_INDEX_LIGHT=on; `light`는 None 그대로)면 그 자리에서 전체 색인을 짓는다(app-core
  `fallback_arguments`가 전체 빌드의 명령줄을 미리 만들고 `IndexJob`이 79 뒤 두 번째 명령으로 잇는다,
  `fell_back`; app-cli 한 줄 "the light cache cannot read it (a cell is defined inside a CBLOCK) -
  the full index was built in its place"; `exit_words(79)`), `--light` 명시면 79로 끝난다. 재고
  패스는 두 번 든다(50 GB에 2~3분; 전체 빌드 2시간 앞에서는 작다). 게이트 light_cache
  `check_unsuitable`(floe-dbg `fixture head-cblock`이 floe_oasis의 손으로 쓴 스트림을 파일로 낸다:
  light 79·빈 디렉터리, FLOE_INDEX_LIGHT=on `floe2 index` → 전체 색인 + 한 줄, `--light` → 79).
- **배치 5,000만 상한.** 경량 요약 패스는 모든 배치를 메모리에 들고 돌므로 상한이 있는데
  (`STREAM_PLACES_CAP`), 넘는 소스는 요약 없이 남고 덱 광역 pass는 영원히 "building"이었다(p10이
  그랬다: 8,260만). 수정: 상한 = `--mem`(기본 min(MemAvailable/2, 32 GiB))의 4분의 1 ÷ 배치당 약 96 B
  (`vfs::light_places_cap`, `PLACE_BYTES_EST`; 현장 32 GiB ≈ 9,000만, `--mem 64` ≈ 1.8억; 전체
  빌드의 자기 패스는 5,000만 그대로). 넘으면 한 줄("N placement records exceed the M the summary
  pass keeps within its budget (a quarter of G at about 96 B each) … --mem raises the budget")과
  함께 건너뛴다. GTK 배너: 뒤의 `floe2 index`가 끝났는데도 pass가 기다리면 "occupancy summary
  unavailable for N passes - the background index (rc R) did not add it; its [summary] lines say
  why (floe2 index deck.jb --mem G raises the summary pass's budget)"로 바꾸고 3 s 폴링을 멈춘다
  (`_summary_job_rc`). `floe2 render`의 미완 메시지도 그 경우를 말한다. 게이트 light_cache
  `check_summary_budget`(main01_like 0.001: 1.29 M 배치, top 62 MB; `--mem 0.1` = 107 MB는 셀보다
  크고 상한 ≈ 28만이라 건너뛰고 말한다; 기본 예산은 짓는다 — check_fixture).

### 8.9 L7a 구현 (0.12.386 / floe-index·renderd 0.12.345, 2026-10-22; 사용자 "L7 진행해줘")

§8.5·§8.7이 L7로 미룬 것 중 렌더러 쪽 둘 — 이미지 증분 합성과 세그먼트 pread 묶음 — 을 했다.
요약 패스 쪽(1패스화·1패스 경량 생성), B안, 웹 배너는 다음 차례다.

- **이미지 증분 합성.** 경량 이미지는 더 이상 라운드마다 새로 짓는 평평한 바이트 버퍼가
  아니다. floe_ovm `Builder`가 들고 있는 섹션 버퍼들(섹션당 Vec 하나, pts pool은 따로)이
  이미지다: `Builder::into_ovm`이 그 위에 `Ovm` 독자를 얹고(`Ovm::parts`; `sec()`가 평평한
  버퍼 대신 섹션 버퍼를, `pool()`이 pool을 읽는다 — 전체 색인의 mmap 경로는 그대로), 채움은
  `Ovm::light_edit`로 제자리에서 붙인다(`LightIndex::extend_image`): 비트셋 표의 꼬리
  (`extend_bitsets`), 비운 셀의 레코드를 0으로(`patch_cell`), 새로 채운 셀의 배치·pool·BVH를
  섹션 끝에(`append_cell_places`: 레코드의 pool 오프셋과 노드의 `first`를 다시 기준 잡는
  append_cell_sink의 길), 텍스트 색인(`build_cell_texts`)을 꼬리 섹션에, 셀 레코드 패치. 라운드마다
  O(이미지)이던 재합성(정적 섹션 재인코딩 + `annotate_bvh_masks` 전체 패스 + finish 복사 +
  얕은 검증)이 O(새 채움)이 됐다 — p10 exact fit의 라운드별 이미지 시간 177/573/3,143/7,400 ms
  → 25/179/408/445 ms.
- **스냅샷 규약.** 요청은 플랜 결과와 스냅샷을 채움 전에 놓는다(render-core `run_within`:
  `drop(out); drop(vfs)`). `fill_into`는 슬롯을 쓰기 잠금으로 잡고 `Arc::get_mut`가 되면(아무도
  들고 있지 않으면) 그 이미지를 제자리에서 늘리고, 다른 독자(동시 요청, 셀 트리 질의)가 들고
  있으면 `Vfs::clone_light`(섹션 memcpy, 재인코딩 없음) 위에 늘려 슬롯에 넣는다(로그 `in
  place` / `copied`). 그 독자의 스냅샷은 불변으로 남는다 — 제자리 성장은 섹션을 늘리고 셀
  레코드만 바꾸므로 기존 노드·레코드의 자리는 움직이지 않는다(밀도 마스크 캐시의 키는
  `Ovm::identity()` = parts 주소; 복제·재합성이면 새 키).
- **노드 마스크를 채움 시점에.** v8 BVH 노드 마스크는 셀 단위 계산이라
  (`floe_ovm::annotate_cell_bvh` — 전체 빌드의 `annotate_bvh_masks`도 이 함수를 셀마다 부른다)
  `encode_places`가 셀을 읽을 때 매긴다(`write_bvh_masks`). 비트셋 id는 인덱스의 공유 표
  (`BitsetTable`, append-only: 첫 이미지의 셀 마스크 세 개씩 → 채운 셀의 노드 마스크)에서
  받으므로 이미지마다·재합성마다 같다. 인코딩(`split_bvh` + 주석)은 셀 단위로 `jobs` 스레드.
- **채움 바이트는 이미지가 든다.** `extend_image`가 붙인 셀의 `CellFill` 바이트(배치·pool·BVH)는
  놓는다(메타데이터 `pool_start`/`pool_len`만 남는다) — 상주 메모리 ≈ 이미지 1×. 비움은 레코드만
  0으로 하고 섹션은 구멍으로 남긴다(`Fills.dead_bytes`, `pending_evict`); 구멍이 산 섹션의 절반과
  64 MB를 넘으면 `compact_image`가 현재 이미지에서 셀을 되꺼내(`Builder::cell_sink_from_image`:
  pool 오프셋·노드 first를 다시 로컬로) 새로 짓는다. 섹션 용량은 재고의 상한(배치 64 B, 노드
  28 B + 셀당 56 B)으로 미리 잡아(`reserve_sections`; 가상 주소, 쓴 곳만 상주) Vec 배가
  realloc의 2배 복사가 없다. 예산(§8.8)은 그대로 `CellFill.bytes`로 센다.
- **세그먼트 pread 묶음.** `LightIndex::read_segments(sis, jobs)`: 요청 세그먼트를 오프셋순으로
  정렬해 인접(또는 포함) 구간을 한 pread로(최대 32 MiB), 구간들을 `jobs` 스레드로 읽는다.
  채움(`pieces_of`: 읽기 → 병렬 파싱)과 페이지 디코드(render-core `decode_pages`)가 쓴다.
- **FLOE_LIGHT_LOG.** `[light]` 줄이 worker-client를 지나 `floe2 render`·뷰어의 stderr에 보인다
  (전에는 `[lock]`만 통과시켜 L6의 안내가 CLI·뷰어에서는 맞지 않았다). 줄: `image N MB in
  place|copied|rebuilt (+A cells, H holed[, D MB dead]) in T ms`.
- **테스트.** floe_vfs `the_image_grows_in_place_as_the_rebuilt_one_would`(tree·adv·labels:
  라운드마다 제자리 이미지 = 이미지에서 새로 지은 것 — 셀마다 배치 레코드·BVH 노드와 마스크
  바이트·텍스트 run·unfilled 표시 —, 평평한 배치는 open의 검증을 지난다(deep은 셀 순서 소유를
  요구하므로 디스크에 가지 않는 이미지에는 맞지 않다), 비움 → 구멍(레코드 0, dead_bytes) →
  compaction(n_places 줄어듦) → 되채움), `coalesced_reads_are_the_single_reads`; render-core의
  light 테스트(budget_tests·deadline_tests 포함)와 게이트 light_cache(픽셀 동일)는 그대로.
- **실측(macOS 4 jobs, p10 909 MB; `floe2 render` 뷰어 기본 설정 fit / 40 µm 근접; render-cli
  exact fit 256 px decode 1,000 페이지).**

  | 뷰 | 0.12.385 (§8.8 뒤) | L7a 0.12.386 | 전체 색인 |
  | --- | --- | --- | --- |
  | fit, 뷰어 기본 | 21.2~25.4 s, 10.5~10.7 GB | 9.7~10.5 s, 11.3~11.8 GB | 3.7 s, 1.3 GB |
  | 40 µm 근접 | 1.3 s, 1.4 GB | 1.0 s, 1.7 GB | 0.20 s, 0.33 GB |
  | exact fit: 플랜 1회 / 라운드별 이미지 / RSS | 45.9 s / 177·573·3,143·7,400 ms / 10.2 GB | 26.2 s / 25·181·429·297 ms / 11.8 GB | — |

  fit의 남은 10 s는 한 라운드다: 예산 fit의 플랜이 4,053셀을 요구하고 선채움이 5,531셀 =
  8,257만 배치(소스의 전부)를 7.1 s에 읽어 인코딩하고 6.9 GB 이미지를 1.0~1.2 s에 제자리로
  늘린다 — 전체 색인이 색인 때 한 일을 뷰 때 하는 비용이라 셀 단위 채움으로는 여기까지다
  (덱 소스의 광역 뷰는 요약으로 그리므로 채움이 없다; 레이아웃은 전체 색인이 기본). 메모리는
  이미지 6.9 GB + 플랜·디코드 상태(exact fit은 전체 색인에서도 6 GB를 넘겼다). 플랜 1회가
  46 → 26 s인 것은 제자리 이미지가 셀별 lazy 표(`bvh_member_log2`)를 라운드 사이에
  유지해서다. 근접 뷰의 PNG는 수정 전과 바이트 동일.
- **남은 것(L7b~).** 요약 패스 1패스화/마킹 가속과 1패스 경량 생성(§8 표; 현장 TB 덱의 요약 1.5~2
  시간), 지역성이 나쁜 writer의 B안, 웹(feature/webui)의 배너·`PatchDto`, 세그먼트 캐시 디스크
  보존.

### 8.10 L7b 구현 (0.12.387 / floe-index·renderd 0.12.346, 2026-10-22; 사용자 "진행해줘")

§8.9가 다음 차례로 둔 요약 패스 쪽이다. 1 TB 덱의 생성 시간에서 요약 패스가 재고 패스와
맞먹거나 더 컸고(§6.1: 현장 `--mem 0.5`에서 재고의 13배), 그 구조가 문제였다.

- **마커가 셀을 직접 파싱한다** (cli `markpass.rs`, `StreamStore::parse_cell`). 전에는 리더
  풀 `jobs`개가 파일 순서로 셀을 파싱해 상주 창(예산의 4분의 1, 최소 가장 큰 셀)에 넣고 마커
  `jobs`개가 꺼내 마킹했다 — 창이 작으면(현장 `--mem 0.5` = 2셀) 리더도 마커도 둘만 돌았다.
  이제 마커 스레드가 다음 셀을 파일 순서로 맡아 그 자리에서 pread + `parse_cell_bytes` +
  `normalize_cells`(리더와 같은 함수, 같은 레코드: 단위 테스트
  `a_cell_parsed_alone_is_the_readers_cell`)로 파싱하고 모든 인스턴스에 마킹한 뒤 버린다.
  상주 메모리는 "마킹 중인 셀들"뿐이고, 입장은 "진행 중인 셀들의 Doc 추정 바이트 + 이 셀 ≤
  예산"(혼자면 항상)이라 `--mem`은 여전히 상한이지만 기본 예산에서는 `jobs`개가 끝까지 돈다.
  소스가 재고 뒤에 바뀌어 파싱이 실패하면 한 줄과 종료 1(전처럼).
- **묶음이 안 쓰는 셀은 읽지 않는다.** 셀마다 넷 중 하나다: 걷기가 방문하지 않거나 묶음의
  레이어가 아래에 없는 셀(건너뜀), 작은 셀(서브트리 bbox만: 아무것도 읽지 않음), 자기
  레코드에 묶음의 레이어가 없는 셀(보관된 배치만으로 멤버 charge와 작은 자식의 격자
  발자국: `StreamOcc::group_own`), 나머지(파싱). 레이아웃처럼 묶음이 여럿이면 뒤 묶음은
  그 레이어에 레코드가 있는 셀만 파싱한다.
- **크기 예산 밖의 레이어는 마킹하지 않는다.** `begin_group`이 묶음을 평면 예산뿐 아니라
  파일 크기 예산(`--occupancy-max-bytes`, 1 GiB)이 아직 받을 평면 수(레이어당 깊이 수
  상한)로도 자른다. 전에는 평면 예산에 드는 레이어를 모두 마킹한 뒤 `end_group`이 예산을
  넘은 것을 none:size로 버렸다 — p10은 449 레이어를 마킹해 23개를 남겼다. 인메모리 걷기가
  레이어 순서로 멈추는 자리와 같으므로 바이트는 같다.
- **다각형 스캔 변환의 버퍼 재사용** (`PolyScratch`). 다각형 멤버마다 월드 꼭짓점·이동
  꼭짓점·간선 목록·활성 목록·교차점의 다섯 할당을 하던 것을 마커(스트리밍은 묶음 레이어별
  스크래치)가 들고 다시 쓴다. 현장 덱 소스는 레코드의 85 %가 다각형(17억 개)이다.
  `mark_cell`의 레코드별 레이어 조회(HashMap)는 마지막 키 캐시로 대부분 건너뛴다.
- 마킹 자체(비트 OR, 연산 단위 charge, 걷기의 연산 집합)는 그대로라 design.ovo 바이트는
  같다: 게이트 `vfs_stream`(전체 스트리밍 빌드)·`light_cache`(경량), 단위 테스트
  `streaming_occupancy_matches_in_memory`(넓은 예산과 가장 큰 셀 크기의 예산 = 한두 셀씩
  입장, 묶음 1개/여럿). `[vfs] occupancy streaming: group N marked in T s: K cells parsed by the
  markers (B), P from their placements, S small, U not visited; parse X s + mark Y s over J
  threads` 줄이 셀 분류와 시간 분해를 낸다.
- **측정** (macOS 4 jobs, `floe-index light --occupancy`, 0.12.386 → 0.12.387; design.ovo 바이트 동일):

  | 소스 | 요약 패스(전) | 요약 패스(후) | 재고 패스 | RSS 전 → 후 |
  | --- | --- | --- | --- | --- |
  | sample9 145 MB | 3.2 s | 2.9 s | 1.5 s | 4.8 GB → 3.9 GB |
  | sample9 `--mem 0.3` | 18.8 s | 4.0 s | | 3.8 GB → 3.5 GB |
  | route_chip 281 MB | 4.1 s | 3.4 s | 3.0 s | 8.6 GB → 7.9 GB |
  | p10 909 MB `--mem 32` | 173 s (449 레이어 마킹) | 28 (묶음 2개, 마킹 23 s) s | 12 s | 11.9 GB → 11.1 GB |

  sample9와 route_chip은 기본 예산에서 전에도 리더·마커가 다 돌던 경우라 10~20 %의 차이다(route_chip은 탑 하나가 레코드 대부분을 들어 마커 하나의 꼬리 3 s가 남는다 — 셀 안 분할은 후속). 예산이 작은 `--mem 0.3`(34 묶음 = 34 패스)에서 4.7배, p10에서 6배: p10은 마킹이 지배적(스레드 초로 parse 0.5 s, mark 109 s)이라 이득은 묶음 자르기(449 → 23 레이어만 마킹)에서 온다. 바이트: sample9·route_chip·valmini는 전후 동일; **p10은 경량 캐시의 design.ovo가 0.12.383~0.12.386에서 인메모리 빌드의 것과 달랐다** — 18개 레이어의 `work` 수가 2~60 어긋났고 비트는 같았다(경량 빌드가 요약 패스에 넘기던 배치가 정규화되지 않아, 작은 자식의 가지치기 가능한 격자를 걷기가 발자국 한 번으로 charge하는 자리를 멤버마다 bbox로 charge했다; 게이트 fixture에는 16멤버 이상의 점목록 배치가 없어 잡히지 않았다). 0.12.387은 `all_placements`가 `normalize_cells`를 거쳐 전체 빌드와 같은 배치를 넘기고, p10의 design.ovo는 인메모리 빌드(`vfs --occupancy-only --in-memory`)와 바이트 동일하다. 현장의 1 TB 덱 소스(셀 5,461개, 한 번씩 배치, 레코드 20억)는 셀별 parse + mark가 비용이라 재고 패스의 1~2배를 본다 — 예산과 무관하게 `jobs`개가 돈다.
- **1패스 경량 생성(셀 로컬 점유 격자 합성)은 보류.** 재고 패스 한 번에 요약까지 내려면 셀의
  레코드를 읽는 시점에 그 셀의 인스턴스 변환(부모의 배치, 파일 뒤쪽에 올 수 있다)과 격자(탑의
  recursive bbox, fold 뒤에야 안다)를 알아야 한다. 셀 로컬 격자에 래스터화해 두고 나중에
  인스턴스마다 월드 격자에 합성하는 방식은 격자 정렬이 인스턴스마다 달라 셀 경계에서 로컬 격자
  한 칸만큼 과마킹한다 — 지금의 ovo(전체 색인과 바이트 동일)와 다른 근사 요약이 된다. 재고
  뒤의 두 번째 읽기를 없애 1 TB당 40~50분을 더 줄이는 길이지만 그림이 달라지는 선택이라
  사용자 결정으로 남긴다. 남은 것: B안(지역성 나쁜 writer), 웹 배너·`PatchDto`, 세그먼트
  캐시 디스크 보존.

### 8.11 캡처·뷰어의 기본값 (0.12.388, 2026-10-22; 사용자 시나리오와 "그 순서로 진행해줘")

사용자가 정리한 세 시나리오 — 일반 레이아웃 뷰어·캡처·DRC 리뷰(10 GB 미만, 전체 색인), 잡덱
뷰어·캡처, 잡덱 구성 레이아웃(**EBEAM 파일**, 수백 GB) 뷰어·캡처 — 와 두 원칙에 맞췄다.
명시적 색인과 뷰어로 여는 파일은 다시 열리므로 요약을 만드는 것이 기본이고, 캡처는 다시
열거나 다시 찍지 않는 전제라 요약을 만들지 않는다(잡덱은 한 파일을 여러 번 캡처하는 일이
드물다).

- **`render`·`probe`·`info`의 `--light` / `--full`** (app-cli `Ready.light`): 자동 색인이 캐시 없는
  소스에 짓는 종류. 없으면 규칙(덱 소스 경량, 레이아웃 전체; FLOE_INDEX_LIGHT). 캐시가 있으면
  종류를 바꾸지 않고 그대로 연다. `floe2 render ebeam.oas --light <창>`이 EBEAM 단독 캡처다.
  `clip`은 페이지가 필요하므로 늘 전체.
- **`render --occupancy on`** 은 뷰 스위치에 더해 자동 색인에 요약을 시킨다(`read::dataset`의
  `summaries`): 덱은 플랜의 occupancy-only 항목(`index_deck`), 레이아웃은 종류를 유지한
  `index --occupancy`(`index_layout`; `decide`가 경량 캐시 위의 전체 요청을 전체로 짓기 때문에
  `light`를 캐시의 종류로 준다). 없으면(기본) 요약을 만들지 않는다: 덱 캡처는 요약이 있으면
  쓰고 없으면 광역 pass를 비운 채 종료 3에 `index deck.jb`와 `--occupancy on`을 안내한다(근접
  창은 완성). 0.12.384~0.12.387은 그리는 명령이 늘 요약부터 만들었다.
- **GTK 뷰어**: 덱을 열 때 모달 색인 한 번(`floe2 index deck.jb [--level …]`)으로 경량 캐시와
  요약을 함께 만든다. `_index_ready`가 `summary_missing`도 보므로 캡처가 만든 요약 없는 캐시만
  있는 덱은 "N sources have no occupancy summary … Build it now?"로 묻고 빠진 요약만 더한 뒤
  연다. 0.12.384의 두 단계(`--no-occupancy` → 열기 → 백그라운드 `floe2 index deck.jb`)는 뺐다 —
  수백 GB 덱에서 묻지 않은 수 시간의 CPU와 NFS 읽기였다. 배너는 모달이 더하지 못한 소스(배치가
  요약 패스의 예산을 넘은 것)에만 남고 3 s 폴링으로 다른 실행이 더한 요약을 받는다.
- 남은 구멍: GTK 파일 열기 대화상자에는 플래그가 없어 캐시 없는 EBEAM을 거기서 처음 열면
  레이아웃 규칙의 전체 색인이다 — FLOE_INDEX_LIGHT=on으로 두고, 필요해지면 대화상자의 선택이나
  크기 규칙을 더한다. `floe2 view`도 아직 `--light`를 받지 않는다.
- 게이트: `jobdeck` LightSummaryTests(기본 캡처는 요약 없이 종료 3 + 안내, `--occupancy on`이
  먼저 채움, `index deck.jb`가 빠진 요약 추가), `floe2`(`render --light`가 경량 캐시만, 기본은
  전체, `--occupancy on`이 종류를 유지한 채 요약 추가, `--light --full` 거절), `index_lock` k10/k12(자동
  색인을 거치는 덱 fit 캡처는 `--occupancy on`; 맨 캡처는 이제 미완), `gtk_view`(fixture를 요약과
  함께 색인: capture_parity의 `--occupancy on` 단계가 요약 그림 자체를 비교한다). 다음은 L8: exact
  캡처의 띠 분할(§8 표).

### 8.12 L8 띠 분할 exact 캡처 (0.12.389 / renderd 0.12.347, 2026-10-23; 사용자 "그 순서로 진행해줘")

exact 캡처(cut 0)는 "모든 페이지 아니면 없음"이라 디코드 페이지의 추정 바이트가 예산
(FLOE_RUST_BUDGET_MB, 기본 1 GiB)을 넘으면 `decoded generation budget exceeded`로 실패했다
(0.12.348의 문구). 덱의 pass는 이미 페이지 조각으로 스트리밍한다(render-core deck.rs
`stream_pass`: pass가 한 레이어 한 색이라 조각의 합이 통짜 래스터와 픽셀 동일). 레이아웃
캡처는 레이어·채움·프레임·라벨이 섞여 조각 합성이 같은 논거를 갖지 못하므로 **장치 행 띠**로
나눴다(renderd `render_banded_capture`).

- 조건: exact, 캡처(`progressive=0`, 여백 프레임 아님), 프로브·밀도 패스·`decode_pages` 제한
  없음, 플랜의 페이지 추정(`Cache::pages_memory`)이 예산의 절반을 넘을 때. 뷰어 프레임과
  여백은 다시 그려지므로 전처럼 거절한다. FLOE_RUST_BANDED_CAPTURE=off가 킬 스위치.
- 띠: 예산의 4분의 1씩이 되도록 추정으로 띠 수를 정하고(최소 2), 띠마다 그 행들의 월드 상자
  (뷰 너비 × 행 높이, 위아래 9 px의 stroke 여유)로 다시 플랜해 페이지를 예산 안에서 디코드하고
  (페이지 캐시의 LRU가 다음 띠와 겹치는 페이지를 쥔다), 창 단위 래스터
  (`render_geometry_styled_cancellable_windowed`: 월드-장치 매핑, 타일 격자, 채움 위상이
  통짜 프레임의 것이라 창의 픽셀은 통짜 렌더와 바이트 동일)로 그려 합성 버퍼의 행에 복사한다.
  어느 띠의 페이지가 그래도 예산의 절반(디코드 뒤에는 예산)을 넘으면 둘로 나눈다, 한 행까지.
- 라벨: 마지막에 합성 버퍼 위로 라벨만의 패스(pan의 라벨 전용 렌더와 같다: 빈 플랜, 뷰 전체의
  라벨, 버퍼 전체 재사용). 띠의 장면은 라벨을 들지 않으므로 띠 경계에서 잘리는 라벨이 없다.
- 발표된 질의 장면은 페이지가 없어 `scene_complete=0`이다(캡처는 pick/snap을 하지 않는다).
  프레임 줄에 `bands= band_pages=`, stderr에 `[capture] gen N: exact capture WxH px: P pages
  estimated at E bytes against the decode budget B - drawn in bands`(worker-client가 `[lock]`·
  `[light]`처럼 넘긴다).
- 캡처 시한: worker-client의 렌더 시한 300 s는 뷰어의 것이라 app-core `captures::run`이
  `RenderSession::set_timeouts`로 캡처를 기다리게 했다(FLOE_RENDER_TIMEOUT_S, 기본 일주일).
- **측정** (macOS 4 jobs, `floe2 render --detail exact --px 800`, 뷰어 기본의 프레임·라벨 켬):

  | 소스 | 기본 예산 | 예산 축소 → 띠 | PNG |
  | --- | --- | --- | --- |
  | route_chip 0.05 | 1.8 s, 643 MB (추정 265 MB < 예산 절반: 통짜) | 64 MB: 67띠 5.6 s · 16 MB: 252띠 21 s · 4 MB: 800띠(행마다) 66 s; 모든 띠가 예산 초과(칩 폭 배선 페이지가 모든 행에 걸림, 행당 132 MB) | 셋 다 동일, 라벨·프레임 끈 변형도 동일 |
  | sample9 145 MB | 8 GiB 예산: 3.6 s, 2.19 GB (기본 1 GiB에서는 전에 `needs at least 1.66 GiB`로 실패) | 기본 1 GiB: 추정 2.86 GB → 띠, 15 s, 1.70 GB · 64 MB: 428띠 99 s, 1.28 GB (335띠 예산 초과) | 둘 다 동일 |

  띠 분할의 비용은 띠마다의 플랜과 띠 경계·띠를 가로지르는 페이지의 재디코드(LRU가 쥐지 못한 것)다: sample9 기본 예산에서 통짜의 4배. 전에는 실패하던 캡처가 나온다는 것이 요점이고, 띠 수는 추정의 4분의 1 예산으로 잡아 과추정에 여유를 둔다(필요하면 상수 하나). 페이지가 띠 전체에 걸치는 소스(route_chip의 칩 폭 배선, sample9의 탑 페이지)는 나누어도 줄지 않으므로 그 띠는 예산을 넘겨서라도 그린다 — 한 행을 그리는 어떤 렌더도 그 페이지들을 들어야 한다; `[capture] … N of M bands drawn past the decode budget` 줄이 알린다. 현장 EBEAM 파일은 한 축 띠 writer라 페이지가 국소적이고(L0 실측) 예산 안에서 띠가 잡힌다.
- 게이트 `floe2`: route_chip 0.05의 exact 캡처를 기본 예산과 FLOE_RUST_BUDGET_MB=64로 찍어
  바이트 비교, 작은 예산 쪽 stderr의 `[capture] … drawn in bands`, 라벨·프레임 끈 변형도 동일; 게이트
  `fit_budget`의 cli_checks(48 MB 예산의 exact 캡처가 띠로 그려져 넉넉한 예산의 것과 바이트 동일, 킬 스위치에서는
  전의 거절 문구와 저장 없음).
  남은 것: 디코드 쪽 coverage 컬링(§8.11의 확인 3: 래스터는 write-once로 덮인 영역을 건너뛰지만
  디코드는 아니다), 덱 pass와 레이아웃 띠의 통합.

### 8.13 L8 리뷰 수정 A (0.12.390 / floe-index·renderd 0.12.348, 2026-10-23; 사용자 "그 순서로 진행해줘")

0.12.389까지를 본 리뷰가 다섯 건을 냈고 모두 사실이었다. 1·4·3·2를 한 묶음으로 고쳤고 5는 §8.14다.

- **1 (P1) 요약 pass가 대상 세그먼트를 모두 메모리에 올린다.** L7a(0.12.386)가 읽기를 합치면서
  `LightIndex::pieces_of`가 `read_segments`로 대상 세그먼트의 원본 바이트를 전부 읽은 뒤에야 파싱을
  시작했고, `all_placements`(`floe-index light --occupancy`, 곧 `floe2 index deck.jb`와 `render
  --occupancy on`의 요약)는 모든 셀을 넘긴다. 도형과 placement가 한 세그먼트에 있는 소스 — span
  세그먼트의 비압축 파일(p10: 35,183셀에 37,332 span), 셀당 CBLOCK 하나인 KLayout류 — 에서는 원본
  전체가 올라갔다(리뷰 실측: 46 MB 입력에서 추가 힙 최고 6.7 → 52.2 MB; L7a 전에는 작업자 수만큼만
  동시에 읽었다). 이제 `pieces_of`는 세그먼트 바이트 합 `PIECES_BATCH_BYTES`(256 MiB, 세그먼트 하나는
  늘 들어간다) 단위로 읽고 파싱하고 놓는다 — 한 배치의 원본 바이트와 모든 배치의 파싱 결과(요청한 것)만
  상주. 뷰어의 fill(4 M placement 배치)도 같은 함수를 지난다. 단위 시험
  `batched_pieces_are_the_whole_read`(1 바이트 배치 = 통째 읽기, 셀별·순서별 동일).
- **4 (P2) 같은 경로의 원본이 교체되면 풀의 옛 fd를 쓴다.** `source_file` 풀이 경로 문자열로만 찾았다:
  원본을 atomic rename으로 바꾸고 캐시를 다시 만든 뒤 같은 프로세스(뷰어 서비스, 웹 서비스)가
  `LightIndex::open`을 다시 하면 identity 검사(경로 stat의 크기·mtime)는 새 파일을 통과시키고 읽기는
  옛 inode로 갔다. 이제 `LightIndex`가 열 때 소스의 (dev, ino)를 `source_id`로 기록하고 풀은 (경로,
  id)로 찾는다 — 다른 id의 항목은 버리고 다시 열며, 경로의 파일이 id와 다르면 거절한다(fstat 한 번,
  읽기마다 경로 조회 없음). 단위 시험 `the_pool_follows_a_replaced_source`,
  `a_reopened_index_reads_the_replaced_source`.
- **3 (P2) 덱의 `--full`이 경량 캐시를 유지한다.** `DeckIndexPlan::prepare`가 `info.indexed`면 요약
  유무만 보고 건너뛰어 `floe2 index deck.jb --full`이 "1 kept"로 끝나고 캐시는 경량이었다(레이아웃
  `index src --full`은 decide()가 전체로 지었다). 이제 `options.light == Some(false)`이고 캐시 종류가
  경량이면 빌드 목록에 넣고(`full_over_light`), `[jobdeck] index : N light cache(s) rebuilt as the
  full index (--full)` 줄로 센다. 반대 방향(전체 위의 규칙상 경량)은 그대로 유지. 게이트 `jobdeck`
  LightSummaryTests 끝에 `--full` 단계.
- **2 (P3) 메타데이터만 만드는 light에도 셀 크기 예산 검사.** `stream_open`이 `StreamStore::new`를 거치며
  최대 셀이 예산을 넘으면 71로 거절했는데, `--occupancy` 없는 light는 inventory pass만 돌고 셀을
  디코딩하지 않는다(valmini `--mem 0.00005`: "cell VALMINI_TOP needs about 84K resident" 71, 실제
  산출물 2.3 KB). 기본 예산은 호스트 한도까지 최대 셀에 맞춰 오르므로 명시적 `--mem`이 작을 때만
  걸렸다. 이제 `StreamArgs.metadata_only`(light 명령이 `!(occupancy || occupancy_only)`로 세움)면
  예산 arm이 `[vfs] stream: … the budget check is waived`를 내고 파일의 need를 명목 예산으로 store를
  연다. `--occupancy`는 검사를 유지한다(마킹 pass는 단독 셀을 늘 받지만 호스트 메모리 보호로 둔다).
  게이트 `light_cache` check_metadata_budget.
- 영향 밖: render-core `decode_pages`의 `read_segments`는 디코드 선택(예산으로 묶임)의 세그먼트만 읽는다;
  전체 색인은 풀을 쓰지 않는다(파일 mmap, 페이지 파일은 읽기마다 open).

### 8.14 L8 리뷰 수정 B (0.12.391 / floe-index·renderd 0.12.349, 2026-10-23; 사용자 "그 순서로 진행해줘")

리뷰의 5번: 스트리밍 full 빌드의 design.ovo pass(마킹 pass, L7b)에서 `StreamStore::parse_cell`이 셀의
def_range 전체를 파싱한 뒤 그 셀의 조각만 골랐다. 범위는 셀의 CELL 레코드 구간이라 보통 셀 하나지만,
다른 셀의 범위 안 CBLOCK에 CELL 레코드가 든 스트림(리뷰 재현: 24셀이 663 KiB 블록 하나에)에서는 그
범위가 25셀을 정의하고 — 셀마다 24번 다시 읽고 파싱했으며, markpass의 admission은 그 셀의 Doc
추정(`cell_bytes`)만 과금해 나머지 24셀의 파싱 결과가 예산 밖에 있었다. 경량 캐시는 이런 스트림을 79로
거절하므로 light에서 full로 전환되는 입력과 명시적 full 빌드의 요약 pass에만 해당한다.

- `StreamStore::shared_range_of(ci)`: 셀의 첫 def_range가 다른 셀도 처음 정의하는 범위(`charges` ≥ 2)면
  (범위, 그 셀들, 범위 cost = charges의 Doc 추정 합). `parse_range(ri)`: 범위를 한 번 읽고 파싱해
  정의된 모든 셀을 셀별로 join·normalize(parse_cell과 같은 레코드). `def_range_count(ci)`.
- markpass: claim 단위가 `Task::Cell` 또는 `Task::Range`. 파일 순서로 다음 셀을 집을 때 그 셀이
  Shapes가 필요하고 공유 범위에 있으면, 아직 claim되지 않은 그 범위의 셀을 모두 함께 집고(`claimed`
  비트) 범위 cost로 입장한다. 한 번 파싱한 결과에서 셀 순서대로 마킹하며(Shapes가 아닌 셀은 전처럼
  bbox·배치만), 다른 범위에도 정의된 셀은 전처럼 혼자 파싱한다. 그룹 줄에 `N of them from R range(s)
  parsed once`가 붙는다(공유 범위가 있을 때만).
- 덤으로 찾은 기존 차이: 스트리밍 `StreamOcc`는 탑 셀도 small 판정(subtree bbox가 격자 한 칸에 들어가면)을
  받았는데 인메모리 마커는 탑에 그 규칙을 적용하지 않는다(탑의 단위는 자기 레코드와 배치, 자식마다 walk). 탑
  전체가 격자 한 칸에 드는 설계(6셀 픽스처의 1 um 격자)에서 비트는 같고 `work`만 달랐다(인메모리 3/2/2,
  스트리밍 1/1/1). `StreamOcc::new`가 탑의 small을 끈다 — 현장 파일에서는 탑이 격자 한 칸에 들지 않으므로
  바이트 변화 없음.
- 바이트 동일: `streaming_occupancy_matches_in_memory`에 픽스처 `cells_in_one_cblock(6)` 추가(한 pass에 읽은
  바이트가 파일 크기를 넘지 않음도 확인), `a_cell_parsed_alone_is_the_readers_cell`에 (5),
  `a_shared_range_is_parsed_once_for_its_cells`(24셀: 25셀 모두 같은 범위, parse_range = parse_cell);
  `floe-dbg fixture cblock-cells`로 게이트 `vfs_stream`이 in-memory와 스트리밍(jobs 1·4, --occupancy)의
  캐시 파일 전부를 바이트 비교한다.

### 8.15 L8 리뷰 수정 C (0.12.392 / floe-index·renderd 0.12.350, 2026-10-23; 사용자 "다음 리뷰 결과를 확인해줘")

0.12.391 리뷰: 5번(§8.14)의 변형 1건과 새 회귀 3건. 모두 재현했다.

- **1. 여러 번 정의된 셀의 공유 CBLOCK(§8.14의 변형).** `shared_range_of`는 셀의 첫 def_range만 봤다. 24셀을
  각자 정의한 뒤 다른 셀 범위의 CBLOCK에서 다시 정의하면 첫 범위는 혼자라 `Task::Cell`로 가고 `parse_cell`이
  def_range 전부(그 CBLOCK 범위 포함)를 셀마다 다시 파싱했으며, 과금은 그 셀의 Doc 추정뿐이었다(리뷰 측정:
  작업당 0.96 MB 과금, 실제 사각형 벡터 36 MiB). 이제 `StreamStore::new`가 범위마다 정의하는 셀 목록을 만들고
  union-find로 셀–범위 연결 집합을 잇는다(`SharedDef { ranges, cells, cost }`; 셀이 둘 이상인 집합만, 셀마다
  `shared_of`). 마커는 Shapes가 필요한 셀의 집합을 통째로 집어(`Task::Shared`) 집합의 범위를 각각 한 번씩 읽고
  파싱한 뒤(`parse_ranges`: 셀별로 모든 범위의 조각을 파일 순서로 join·normalize = `parse_cell`) 셀 순서로
  마킹한다. cost = 집합 셀들의 Doc 추정 합(집합의 범위가 가진 레코드 전부가 그 셀들의 것이므로 파싱이 만드는 양과
  같다). 집합이 셀의 def_range를 모두 담으므로 셀은 늘 완전하고 `def_range_count` 폴백은 없어졌다. 최악의 집합
  (CBLOCK 정의가 사슬처럼 이어진 파일)은 파일 전체가 한 작업이지만, 그런 파일은 리더 풀의 다중 정의 span 요구
  (`min_budget`)도 같은 크기다. 픽스처 `cells_defined_twice(k)`(C0..Ck-1 각자 정의 → HOST + CBLOCK 재정의 →
  TOP), `floe-dbg fixture cblock-cells-twice`, 게이트 `vfs_stream`의 비교 목록에 추가; 단위 시험
  `a_shared_definition_is_parsed_once_for_its_cells`(한 CBLOCK: 25셀/1범위, 두 번 정의: 25셀/25범위, cost =
  Doc 합, parse_ranges = parse_cell), parity 시험 두 곳에 "twice" 케이스. 측정(24셀 두 번 정의, `floe-index vfs
  --stream --occupancy --occupancy-prune 0`, 2 jobs): 마커 읽기 5.5 KB → 517 B(파일 1,057 B; `25 of them from 25 range(s) parsed once`), design.ovo는 수정 전후와 인메모리 빌드와 바이트 동일; 한 CBLOCK 픽스처는 194 B 그대로.
- **2. 띠 가장자리 픽셀.** 첫·마지막 띠에도 9 px 여유를 더해 ROI 밖 페이지(전체 렌더는 플랜에 넣지 않는)를
  띠의 플랜에 넣었고, 윤곽선은 변 중심이라 폭의 절반이 도형 밖으로 나오므로 그 페이지의 굵은 윤곽선이 가장자리
  행에 들어왔다. 띠 상자의 y를 전체 플랜의 뷰(`page_request.view`, 명령 뷰의 정수화)로 자른다(x는 원래 같다).
  재현: ROI 위 0.5~3 px에 폭 8 윤곽의 도형(자기 페이지) → 100×100 exact 캡처가 예산에 따라 144/48 픽셀
  달랐고, 수정 뒤 0. 게이트 `fit_budget` `band_edge_checks`(Python renderd 클라이언트 `RustRenderWorker`에
  `set_outline_width`·`progressive=0`·`stderr_tail` 추가).
- **3. 높이 1 캡처의 패닉.** `clamp(2, height)`가 height 1에서 패닉(Rust의 clamp는 min > max면 패닉).
  `clamp(min(2, h), h)`: 한 띠. 덤으로 렌더 스레드의 패닉은 응답 없이 사라져 클라이언트가 기한까지 기다렸다
  (캡처는 일주일; 재현: route_chip 800×1 `--detail exact`는 기본 예산에서도 588 페이지 5.3 GB 추정으로 띠
  분할 → 패닉 → 대기, `FLOE_RENDER_TIMEOUT_S=20`이면 20 s 뒤 "render deadline exceeded"). renderd
  `render_worker`가 렌더를 `catch_unwind`로 감싸 `error gen=N code=panic message=the renderer failed inside
  this render: …`를 보내고 워커를 끝낸다(패닉 뒤 상태는 믿지 않는다; 다음 명령에서 "render worker stopped"로
  프로세스가 끝나고 패닉 훅의 보고는 stderr에 남는다). 게이트 `fit_budget` cli_checks에 `--px 800x1 --stretch
  --detail exact` 48 MB(기한 120 s로 회귀가 게이트를 멈추지 않게).
- **4. `render --full --occupancy on`.** 자동 색인(app-cli ready.rs)이 `DeckIndexPlan::prepare`를 그대로 써서
  0.12.390의 규칙(명시적 `--full`은 경량 캐시를 전체로 재빌드)이 경량 캐시를 빌드 항목으로 만들었고,
  `index_deck`은 캐시가 있는 소스의 occupancy-only 항목만 받으므로 아무것도 실행하지 않았다(캡처 exit 3,
  `summary_building=14`; `--full`을 빼면 성공). `prepare_for_open`을 분리했다: 자동 색인은 있는 캐시를 종류와
  무관하게 그대로 열고(`--light`/`--full`은 캐시 없는 소스에 짓는 종류), 요약 없는 캐시는 occupancy-only 항목
  (실행은 캐시 종류의 명령: `light_run = kind == Light`). `floe2 index deck.jb --full`과 서비스의 색인은 전처럼
  `prepare`다. 게이트 `jobdeck` LightSummaryTests에 `render --full --occupancy on` 단계(요약이 생기고 캐시는
  경량 그대로).

### 8.16 L8 리뷰 수정 D (0.12.393 / floe-index·renderd 0.12.351, 2026-10-23; 0.12.392 리뷰 1건)

0.12.392 리뷰: [P2] 연결된 공유 정의 집합 전체가 메모리 예산을 넘어 적재된다. 재현했다.

- **지적.** `parse_ranges`는 (A,B) → (B,C) → (C,D)처럼 이어진 집합의 범위를 전부 파싱한 뒤 돌려주고, 마커의
  admission은 진행 중인 작업이 없으면(`resident == 0`) 예산 초과도 허용하므로, 셀마다 CBLOCK이 다음 셀을
  정의하는 32셀 체인(파일 3.9 MB, Doc 추정 58 MB)은 `--stream --mem 0.006`(마킹 예산 4.6 MB)에서 거절 없이
  집합 전체 58 MB가 한 번에 올라갔다(리뷰 측정: 추가 힙 최고 6.2 → 62.3 MB; 여기서는 occupancy-only 패스의
  최대 RSS 72 MB). `min_budget`은 가장 큰 셀과 다중 정의 span만 봤으므로 이런 파일도 거절되지 않았다. 집합의
  길이만큼 메모리가 늘어 큰 입력에서는 OOM이 될 수 있는 결함이다.
- **수정.** 집합을 리더 풀의 규칙으로 걷는다(markpass.rs `SetRun`/`Task::Range`, stream.rs `parse_range`·
  `joined`). 집합의 첫 셀에 닿은 마커가 집합을 "진행 중"으로 올리고(집합의 셀 전부 claim; 그룹이 파싱할 셀은
  pending, 나머지는 손에 있는 것으로 마킹), 마커들은 집합의 범위를 파일 순서로 하나씩, 예산이 허락하는 대로
  집어 각각 한 번 파싱한다. 범위의 과금은 그 범위가 처음 정의하는 셀들의 Doc 추정이고(리더 풀의 `charges`와
  같다), 셀은 마지막 정의 범위가 file되는 순간 완성되어 join·normalize·마킹·해제된다. 따라서 상주는 "정의
  사이에 열려 있는 셀들" = 파일의 다중 정의 span이며 `min_budget` 이내다(마킹 예산은 `max(--mem/4,
  min_budget)`). 집합이 혼자 상주하고 진행 중인 범위·마킹이 없을 때만 예산을 넘는 범위를 들이는데(교착
  방지용 탈출구), span ≤ 예산이면 그 경우는 오지 않는다. 진행 중인 집합은 한 번에 하나다(둘이면 서로의 조각을
  기다릴 수 있다). 범위가 다른 셀을 정의하지 않는 보통 파일은 전과 같이 셀 단위(`Task::Cell`)다.
- **덤: `min_budget`의 허점.** 한 범위가 여러 셀을 정의하면(다른 셀 범위의 CBLOCK 안 CELL) 리더는 그 범위를
  통째로 파싱해 셀들이 함께 상주하는데, `min_budget`은 가장 큰 셀과 span만 봤다(한 CBLOCK의 24셀 픽스처:
  범위 과금 1,696 B, 가장 큰 셀 763 B. 리더 풀도 `resident == 0`의 탈출구로 그 범위를 들였다). 이제 가장 큰
  범위 과금도 포함하고 거절 문구가 그 범위를 말한다("a range of the file (offset N) defines K cells together
  ... which need about X resident at once"). 기본 예산의 상향(`need`까지, 호스트가 허락하면)은 그대로다.
- **측정(체인 32셀, `--mem 0.006`, 4 jobs).** occupancy-only 패스의 최대 RSS 72 → 22.5 MB, 그룹 줄의 새 항목
  `resident peak` 4.6M(예산 4.6M; 1 job이면 3.7M), 전체 스트리밍 빌드의 최대 RSS 85 → 36 MB; design.ovo는
  인메모리 빌드와 바이트 동일(`--mem 0.02`, 1 job도). 한 CBLOCK 24셀·두 번 정의 픽스처는 읽기 194 B·517 B
  그대로(peak 5.9K·12K), ovo 동일.
- **게이트·시험.** 픽스처 `floe_oasis::inventory::fixtures::cells_chained(k, m)`, `floe-dbg fixture
  cblock-chain`(32셀 × 10,000 rect, 4.1 MB), 게이트 `vfs_stream`에 체인 비교(예산 = 가장 큰 셀의 3배 = span)
  추가 + 모든 픽스처의 스트리밍 로그에서 `resident peak ≤ resident budget`, 체인에서는 Doc 추정의 1/4 이하
  확인(0.12.392 바이너리라면 58M으로 실패); 단위 시험 `a_shared_definition_is_parsed_once_for_its_cells`
  (체인 케이스, `defines`·`range_charges`, 범위별 파싱 + `joined` = `parse_cell`), parity 시험에 체인 케이스와
  tight 예산에서 `peak ≤ min_budget`(체인은 파일 Doc의 절반 이하), `build_both`는 파일의 need로 재시도.
- **게이트 `fit_budget`의 시간 제한.** cli_checks의 `floe2 render` 단계(1920 px exact 캡처: 25,393 페이지를 48 MB
  예산에서 104 띠로)는 subprocess 제한이 600 s였는데, 다른 프로그램이 3코어를 쓰는 호스트에서 0.12.392
  바이너리로도 586 s가 걸려 배터리 428·429가 여기서 멈췄다(수정과 무관). 제한을 1800 s로 올렸다(멈춤을 잡는
  상한이지 속도 기준이 아니다; 800×1 한 줄 캡처의 `FLOE_RENDER_TIMEOUT_S=120`은 그대로).

### 8.17 L8 리뷰 수정 E (0.12.394 / floe-index·renderd 0.12.352, 2026-10-23; 0.12.393 리뷰 1건)

0.12.393 리뷰: [P2] 선택되지 않은 셀의 임시 파싱 메모리가 예산에서 빠진다. 재현했다.

- **지적.** 마커의 admission은 그룹이 파싱할 셀(pending)만 과금하지만 `parse_range`는 범위 안의 모든 셀을 만든 뒤
  비대상 셀을 버렸다. 두 레이어가 섞인 파일(레이어 1의 작은 셀 Ai 뒤에 레이어 2의 큰 셀 Bi·Bi+1을 정의하는
  CBLOCK이 오는 63 범위, B는 사슬이라 전체가 한 집합)의 레이어 1 그룹에서 12 마커가 각자 범위의 큰 B들을
  만들었다 버려, 내부 추정 2.9 KB에 실제 추가 힙 26.9 MB(리뷰 측정). 여기서 occupancy-only `--mem 0.006
  --occupancy-prune 0 --occupancy-um 0.001`(레이어별 두 그룹)의 최대 RSS: 1 job 23.7 MB, 8 jobs 45.5 MB,
  12 jobs 54.0 MB.
- **수정 1: 파서가 지정한 셀의 레코드만 보관.** `floe_oasis::doc::parse_cell_bytes_keeping(.., wanted)`: CELL
  레코드의 셀(이름, 또는 CELLNAME 참조 번호 → 재고의 표 → Doc 인덱스)이 wanted가 아니면 그 셀의 레코드는 파싱만
  하고(모달 상태를 다음 레코드가 읽으므로 건너뛸 수는 없다) 벡터에 넣지 않으며 그 수를 센다(`Builder.keep/skip/
  skipped`; 배치·텍스트·사각형·다각형·경로·사다리꼴·원 모두). 비대상 셀의 조각은 빈 채로 나온다.
  `StreamStore::read_range`·`parse_range`·`parse_cell`이 wanted를 받고(마커: 집합에서 그룹이 파싱할 셀들, 혼자인
  셀: 자신), 리더 풀은 전부 보관한다. 그룹 줄에 `N records of other cells not kept`, `MarkReport.dropped`.
- **수정 2: 집합들을 파일 순서의 한 커서로.** 0.12.393은 한 번에 한 집합만 진행했다(두 집합이 서로의 조각을
  기다리는 교착을 피하려고). 그러면 독립된 작은 집합이 많은 파일(셀마다 CBLOCK 안에 셀 하나)은 마커 하나로
  돈다. 이제 `StreamStore::shared_ranges`(모든 집합의 범위를 파일 순서로)를 마커들이 한 커서로 걷는다: 커서가
  집합의 첫 범위에 닿으면 집합을 활성화하고(셀 claim, Shapes 셀은 pending, 나머지는 그 자리에서 마킹) 범위는
  예산이 허락하는 대로 내보내 여러 집합이 함께 진행된다. 리더 풀이 파일을 읽는 순서 그대로라 span 상한이
  집합을 가로질러 성립하고(열린 셀들의 첫 범위는 모두 가장 먼저 열린 셀의 span 안), 탈출구(아무것도 진행 중이
  아닐 때만 예산을 넘는 범위를 들임)는 예산 ≥ min_budget이면 오지 않는다. 혼자인 셀은 전처럼 셀 순서로 간다.
- **측정(리뷰 파일, occupancy-only `--mem 0.006 --occupancy-prune 0 --occupancy-um 0.001`, 레이어별 두 그룹).**
  최대 RSS 1 job 23.7 → 23.9 MB, 8 jobs 45.5 → 29.9 MB, 12 jobs 54.0 → 29.1 MB(1 job 대비 +30.3 → +5.2 MB);
  그룹 1 줄 `1260000 records of other cells not kept`(B들의 사각형), 그룹 2 `63 records`(A들); design.ovo는 두
  바이너리 모두 인메모리 빌드와 바이트 동일. 새 픽스처(층 64셀, 쌍 64개)도 1·4 jobs에서 동일.
- **게이트·시험.** 픽스처 `cells_layered_chain(k, m)`·`cells_paired(k, m)`, `floe-dbg fixture cblock-chain-layered`
  (64셀 × 10,000 rect, 3.6 MB)·`cblock-pairs`(64 쌍, 2,000 rect), 게이트 `vfs_stream`: 두 픽스처의 비교(사슬의
  예산은 가장 큰 셀의 3배) + `check_layered_markers`(층 픽스처의 레이어별 두 그룹 occupancy-only를 1 job과
  12 jobs로: ovo 동일, 그룹 1에 `records of other cells not kept`, RSS(12 jobs) ≤ RSS(1 job) + 18 MB; 이 픽스처에서
  0.12.393은 +25~28 MB, 0.12.394는 +5~11 MB); 단위 시험 doc.rs `a_range_parsed_for_some_cells_keeps_their_records_alone`(픽스처 10종의 모든 범위에서
  셀 하나만 보관 = 전체 파싱의 그 조각, 나머지는 빈 조각, 건너뛴 레코드 수 = 나머지의 레코드 수), stream.rs 집합
  시험에 보관 필터 검사, parity 시험에 layered·pairs 케이스(레이어별 그룹이면 `dropped > 0`, 한 그룹이면 0),
  alone 시험에 두 케이스. 단위 시험 floe-oasis 31·floe-index 46 통과.
- **남은 것.** 레이어 1 그룹이 A들만 쓰면서도 범위 전체(3.4 MB)를 읽고 CBLOCK을 풀어 파싱한다. 읽기량을 줄이려면
  세그먼트 디렉터리(design.seg)로 원하는 셀의 세그먼트만 읽어야 하는데, 그것은 경량 캐시의 길이고 전체 빌드의
  마킹 pass에는 아직 없다.
- **게이트 `fit_budget`의 한 줄 캡처 기한.** `--px 800x1 --stretch --detail exact`(§8.15의 3번 회귀 검사)에 준
  `FLOE_RENDER_TIMEOUT_S=120`은 멈춤을 잡는 상한이었는데, 이 캡처도 칩의 25,393 페이지를 디코드하므로 다른
  프로그램이 3코어를 쓰는 호스트에서는 120 s를 넘겼다(배터리 433; 430에서는 통과). 900 s로 올리고 subprocess
  제한도 1800 s로 맞췄다. 멈춤이면 일주일 대신 15분에 드러난다.

### 8.18 덱 첫 화면 — 요약 생성 한도·자동 셀, 전체 깊이의 prune, 덱 open 병렬 (0.12.395 / floe-index·renderd 0.12.353, 2026-10-23; 사용자 "대용량 잡덱 첫 화면 10분 이상" 검토 → "진행해줘")

- **현장 수치(사용자 붙여넣기).** 레벨 2개 × EBEAM 137개 = 경량 소스 274개, 758 × 794 px 창, 약 211 µm/px,
  `cut<633um`, thin keep. frames on: `loaded in 491.8s (cache 6.9s + service 2.9s + first frame 482.02s)`,
  `live (494986 tiles, +494986 new, 448436 ms = 108404 load [15371 plan+20038 delta+72995 apply] + 308724 draw)`;
  frames off: 458.8 s, `418663 ms = 78866 load [1904 plan+6835 delta+70127 apply] + 308477 draw`. 상태줄
  `deck 268 passes, summary 199 passes (not pickable) … evict 494k`; 긴 줄 `paints 5.1G`, `thin pages 495k kept`,
  `raster wall 308398 ms 4p x 4t, 8 batches, pass max 801MB, batch max 22MB, 2 streamed in 503 slices`,
  `dec sum 366423/max 274/idx 45825ms`.
- **읽기.** 268 pass 중 69개가 요약 없이 49.5만 페이지를 그렸고, 그중 두 pass(거대 평탄 소스 둘, 가시 레이어
  도형 약 25억 개씩)가 503 slice로 순차 스트리밍되며 51억 도형을 칠했다 — draw 308 s, decode 70 s의 거의
  전부(나머지 266 pass는 22 MB 이하 batch 6개). frames on/off 차 27 s = plan 13.5 + read 13.2 s: 전체 깊이에서
  프레임이 나올 수 없는데도 요약된 서브트리를 걷고 경량 캐시를 채운 비용. 두 실행 모두 프레임 자체 ms와 GUI
  first frame 사이에 33.6 s가 같다 = renderd `Deck::open`(274개 순차, 소스마다 design.inv/seg 통째 읽기 +
  요약·첫 이미지). 거대 소스에 요약이 없는 이유로 가장 유력한 것은 마크 한도 2^31(21.5억; 평탄 파일은 도형당
  마크 하나) 초과의 `none:work`이고, 덤프 확인은 현장 복귀 뒤(미확인). 네 수정은 어느 한도가 걸렸든 유효하다.
- **수정 1: 마크 한도 기본 2^36, `floe2 index`가 한도를 전달.** `occupancy::DEFAULT_MAX_WORK` 2^31 → 2^36
  (측정 80 M marks/s에서 레이어 하나의 마킹 상한 약 15분; 25억 마크 ≈ 30 s). `floe2 index --occupancy-max-work N
  --occupancy-max-bytes N[K|M|G]`가 floe-index light/vfs에 그대로 간다(`IndexOptions::occupancy_max_work/_bytes`,
  `occupancy_args`; floe-index 쪽 `--occupancy-max-bytes`도 접미사를 받는다). 요약 없는 레이어의 빌드 로그 줄에
  올릴 옵션을 덧붙이고(`occupancy::status_hint`: `… none:work (work N) - over --occupancy-max-work M marks (a flat
  file marks once per shape: raise it)`), 파일 전체 none:cells도 한 줄 남긴다(`none_cells_note`; 전에는 무음이라
  빌드 로그 grep으로는 못 찾았다).
- **수정 2: 자동 기준 셀이 한도를 본다.** `occupancy::auto_base_um(span_x, span_y, layers, max_cells, max_bytes)`
  → (셀, 사유): 2026-09-16 규칙(`auto_base_um_for_span`)의 첫 선택이 셀 상한(레이어당 2^30)이나 레이어당 평면
  하나 × 레이어 수가 크기 상한을 넘으면 사다리(남은 4/2 µm 후보, 그다음 `AUTO_COARSE_UM` 8/16/32)에서 맞는
  첫 셀로 올라간다(`AutoWhy::Cells/Size`; 로그 `cell=8um (8000 dbu, auto for the cell cap)`); 32 µm도 넘으면
  종전대로 none:cells. 152.4 mm 레티클 전체 bbox: 4 µm에서 38,100² = 14.5억 > 2^30 → 8 µm 3.6억 셀(평면 45 MB);
  131 mm까지는 4 µm 그대로. 인메모리·스트리밍 빌드 모두, `DeckSource::summary_pending`도 같은 함수(기본 한도)로
  판정한다. 한계: 깊이 평면 수는 마킹 전에 모르므로 크기 상한은 레이어당 평면 하나로만 본다(sample9의 38~40
  레이어 none:size는 그대로다).
- **수정 3: 전체 깊이의 prune (P1).** `DeckSource::prune_summary_for` = `prune_summary_when(frames, depth,
  max_depth)`: frames off이거나 depth가 무제한 또는 소스의 계층 높이 이상이면 요약된 서브트리를 걷지 않는다
  (프레임은 hier.rs `frame_depth_boundary`의 r == 0에서만 나오므로 거기서는 나올 수 없다; 제한 깊이는 종전대로
  걷는다). 합성 덱 8소스(sample9 ×8 하드링크, 1200 px fit 캡처): frames on 2.37 s·fill 32 round → 0.05 s·0 round,
  frames off 0.04 s·0 round; 세 PNG md5 동일(`d98e4f72…`). 현장 27 s의 몫.
- **수정 4: 덱 open 병렬 (P4).** `Deck::open_with(spec, budget, jobs)`: 소스를 `jobs`개 워커가 연다
  (`open_sources`: 스펙 순서 보존, 순서상 첫 실패가 오류, 실패 뒤 새 open 없음); renderd가 `open … jobs=`를 넘기고
  `FLOE_RUST_DECK_OPEN_JOBS`가 덮어쓴다(1 = 직렬, 진단). 합성 64 경량 소스(요약 없음, `--px 100` 캡처 = 사실상
  open뿐, exit 3): 직렬 0.62 s → 4 jobs 0.28 → 8 jobs 0.24 s(프로세스 전체 벽시계, 로컬 SSD). 현장의 34 s는 NFS
  읽기와 큰 design.seg라 비율은 다를 수 있다. 병렬 open의 일시 디스크립터(소스마다 design.inv·seg 통째 읽기,
  lease)는 직렬의 workers 배라 열린 파일 한도가 작은 호스트에서는 거절된다(배터리 439: 게이트 index_lock K7,
  `ulimit -n 32`에 40 소스가 `Too many open files`): 그 오류(`is_too_many_open_files`)를 만나면 워커를 멈추고
  남은 소스를 종전처럼 하나씩 연다.
- **재현(한 소스).** `floe-index light SRC --out DIR --occupancy-only --occupancy-max-work 1000000`: 레이어 1/0
  (마크 3,043,564) → `none:work (work 1025923) - over --occupancy-max-work 1000000 marks …` → 덱 캡처 0.33 s,
  fill 4 round(페이지 경로); 기본 한도로 재생성 → ok → 0.03 s, 0 round.
- **게이트·시험.** 단위: occupancy.rs `the_automatic_base_cell_climbs_for_the_caps`(규칙 표 + build에서 셀 상한
  1 M/100 k/10 k → 8 µm/32 µm/none:cells, 명시 셀 불변), deck.rs `summarized_subtrees_are_pruned_whenever_no_frame_can_appear`,
  light.rs `a_deck_opens_its_light_sources_in_parallel_in_order`(7 소스, jobs 1/4/16에서 순서 동일, 빠진 소스
  둘 중 앞의 것이 오류), app-core `the_summary_caps_go_to_floe_index_as_given`, app-cli
  `the_summary_caps_are_options_of_index`. 게이트 `occupancy`: 자동 셀의 상한 사례(1 M → 4 µm, 100 k → 8 µm,
  크기 500 kB → 2 µm, 명시 1 µm + 100 k → none:cells와 그 로그 줄), limits 시험(명시 셀 + 10 셀 → none:cells,
  자동 → 16 µm ok, none:work/none:size 줄의 안내, `1K`, `4X` 거절), `test_the_app_forwards_the_summary_caps`;
  게이트 `jobdeck` FullDepthPruneTests: 네 단계 사슬 픽스처 `deep.oas`(단계마다 광역 cut 위의 1/0 상자 하나,
  `build_deep_oas`)를 경량 캐시 + 요약으로 색인하고 prefill을 끈 채(`FLOE_LIGHT_PREFILL_PLACES=0`: 펼친 셀마다
  round 하나) `floe2 render --detail medium --occupancy on`: 전체 깊이의 frames on/off 모두 `[light] round` 0
  (0.12.394는 frames on에서 단계마다 하나, 3 round)이고 두 PNG 바이트 동일; depth 1은 프레임이 그려지고(두 그림이
  다름) round 수는 on/off 같다. 게이트 픽스처가 작아 오라클 워커 경로의 `light_rounds`(열기의 첫 채움 1)로는
  구별되지 않아 CLI 캡처와 round 줄로 잡았다.
- **현장 확인 절차.** 복귀 뒤 두 거대 캐시의 `floe-index occupancy DIR` 덤프에서 `status=`·`work=`(경량 캐시의
  `identity=mismatch`/`identity_error=design.ovm …`는 표시 문제). 수정판으로 `floe2 index deck.jb --jobs 12`를
  돌리면 그 두 소스의 로그에 `[vfs] occupancy cell=… ok=…`와 ok 레이어가 찍히고(없어진 요약만 다시 만든다),
  상태줄은 `summary 268 passes`, 첫 화면은 open 수 초 + 요약 그리기가 된다. 여전히 `none:work`면 `floe2 index
  deck.jb --occupancy-only --occupancy-max-work 274877906944`처럼 올린다.
- **정정.** `floe2 view --render-debug`는 현재 아무것도 찍지 않는다(gui.py에서 미사용, 뷰 컨트롤러가 debug를 끔);
  긴 perf 줄의 `, K passes without summary`가 nofile/invalid/near/layers 거부 수다(depth/off/exact는 무음).
- **현장 확인(2026-10-25~26, 사용자 붙여넣기).** 레벨 4·5(각 134칩, 소스 = 서로 다른 EBEAM 파일)의 캐시를 그대로 둔 채
  바이너리만 올린 상태: `loaded in 535.0 s 522515 ms = 129198 load + 351646 draw deck 268 passes, summary 199 passes (not
  pickable) bin 243 items cut<495um evict 623k` — 69 pass가 요약 없이(옛 한도로 만든 design.ovo의 레이어가 `none:*`).
  레벨 4의 요약을 `--occupancy-only`로 다시 만든 뒤 레벨 4만: `loaded in 8.2s 224 ms = 0 load + 99 draw deck 134 passes,
  summary 134 passes` — 페이지 디코드 0, 8 s는 경량 소스 134개의 open. 줌 6~9회에서도 느려짐 없음. 10.6 × 7.4 mm 뷰
  (7.6 µm/px)부터는 22 pass 중 21개가 `near`로 페이지 경로(`41880 ms = 10036 load + 28282 draw`, 8,056 페이지, 49
  슬라이스 스트리밍): 50 GB EBEAM의 요약은 `base_um = 2, layers = 5`이고 배치 배율 `mag = SF × AD / unit`로 보이는 셀이
  커지는 것이라, 덱 색인이 소스마다 `--occupancy-um = 4 µm / mag`를 넘기는 것이 후속 후보(사용자: 덱은 캡처 우선, 보류).
  레벨 5는 요약을 모두 만든 뒤에도 `summary 133 passes`로 1 pass가 남아 745.5 s — 어느 소스인지는
  `tools/check_deck_summaries.py deck.jb 5`(0.12.404: 소스별 `floe-index occupancy`의 identity·레이어 status를 pass의 LY/DT와
  맞춰 OK/NOFILE/INVALID/LAYER 한 줄씩, 문제가 있으면 exit 1)로 가린다; 이 명령이 경량 캐시에서 design.ovm을 찾아
  늘 `identity=mismatch`를 내던 결함은 0.12.403에서 고쳤다(SPEC-INDEXER의 occupancy 명령 항목).

### 8.19 요약 상한의 기본값 2^32·4 GiB, 합본 평면 캐시의 바이트 상한 (0.12.396 / floe-index·renderd 0.12.354, 2026-10-24; 사용자 "요약을 더 적극적으로" 검토 → "진행해줘")

- **사용자 관찰.** 잡덱은 웨이퍼 마스크 데이터라 일반 레이아웃과 달리 depth가 깊지 않고 레이어별 위치가 겹치지
  않는 경우가 대부분이며, picking을 따로 하지 않으니 요약을 더 적극적으로 써도 된다. 4 µm를 8/16 µm으로 굵히면
  보기에 좋지 않을 것 같으니 색인 시간·용량을 조금 더 써서 4 µm를 지키는 쪽이 좋겠고, Calibre는 fit 뷰뿐 아니라
  약 4배 확대까지 요약을 쓰는 것 같으니 floe도 그 정도까지 쓰면 뷰어가 쾌적하겠다. 캡처(render)는 따로 본다.
- **규칙 확인(변경 없음).** 요약은 fit 전용이 아니다: 매 프레임 레이어마다 기준 셀 ≤ 화면 1 px이면 셀이 0.5~1 px인
  피라미드 레벨을 그리고(summary.rs `level_for`, OCCUPANCY_PLAN §3), 1 px를 넘는 뷰부터 페이지 경로(`near`)다.
  현장 창 758 px, 덱 fit 211 µm/px에서: 4 µm 셀은 4 µm/px(뷰 폭 3.0 mm, fit의 53배)까지, 8 µm는 26배, 16 µm는
  13배까지 요약이고 fit의 4배(53 µm/px)에서 4 µm 셀은 0.08 px다. 같은 배율의 그림은 4 µm 파일과 8 µm 파일이 같다
  (레벨 L은 아래 레벨 2×2의 OR이라 4 µm 피라미드의 레벨 1 = 8 µm 피라미드의 레벨 0: 같은 격자, 같은 비트). 굵은
  셀이 잃는 것은 그림이 아니라 요약이 유지되는 구간 하나(4~8 µm/px; 거대 평탄 파일에서는 페이지 경로가 수백만
  도형을 읽는 뷰)라 4 µm가 좋다는 판단은 맞다. 소스 하나 기준으로는 자동 셀이 긴 변 ≥ 2,048셀이라 그 소스가
  화면에서 2,048~4,096 px를 차지할 때까지(758 px 창에서 자체 fit의 2.7~5.4배, 1920 px 창에서 1.1~2.1배) 요약이다;
  덱에서는 소스가 덱보다 작으니 덱 fit 대비로는 훨씬 크다. 현장 덱이 확대 중에도 무거웠다면 원인은 규칙이 아니라
  요약 없는 69 pass(§8.18)이고, 복귀 뒤 상태줄 `summary N passes`가 fit과 4배에서 같은 수인지로 확인한다. 1 px
  규칙을 넘기는 확장(셀 2 px까지 박스로 칠하기)은 하지 않는다: 필요가 없고, 그림이 2 px 블록이 되며, §3의 1 px
  팽창 계약이 2 px로 느슨해진다. 캡처와의 분리는 요청 단위의 `occupancy=`와 exact/archival이 이미 가른다.
- **상한 기본값.** `occupancy::DEFAULT_MAX_CELLS` 2^30 → **2^32**(65,536², level 0 평면 512 MB),
  `DEFAULT_MAX_BYTES` 1 → **4 GiB**. 152.4 mm 레티클 전체 bbox가 4 µm 그대로(38,100² = 14.5억 셀, 평면 181 MB,
  피라미드 포함 242 MB; 8 µm는 60.5 MB), 9인치 228.6 mm(32.7억)도 4 µm, 262,144 µm 정방형까지. 사다리는 그 너머의
  보험으로 남는다(300 mm → 8 µm, 600 mm → 16 µm, 1.2 m → 32 µm, 3 m는 32 µm none:cells). 마킹 작업은 도형당 하나라
  셀 크기와 무관하고(25억 도형 ≈ 30 s, 측정 80 M marks/s), 비용은 사실상 용량 4배와 생성 중 메모리다. 실측(152.4 mm
  bbox에 10 µm rect 2,000개의 픽스처, `floe-index light --occupancy --jobs 4`, 로컬 SSD): 4 µm `cell=4um (4000 dbu,
  auto)` 0.37 s, design.ovo 242 MB, 최대 RSS 911 MB — level 0 평면 181 MB의 약 5배(마킹 평면, level 0 사본, 피라미드,
  `write_ovo`의 파일 이미지가 함께 상주; 스트리밍 쓰기는 후속); 옛 상한 2^30에서는 `cell=8um (8000 dbu, auto for the
  cell cap)` 0.12 s, 60.5 MB, RSS 231 MB. 뷰어는 design.ovo를 mmap으로 열어(`OvoFile::open`) 그리는 레벨만
  만진다(fit은 레벨 5~6 수 MB, 근접 끝 level 0 181 MB). `DeckSource::summary_pending`은 같은 상수를 쓰므로 함께 바뀐다. 한계는 그대로: 깊이 평면
  수는 마킹 전에 모르므로 크기 상한 판정은 레이어당 평면 하나다(레티클 4 µm 평면 17장이 4 GiB 경계).
- **합본 평면 캐시.** summary.rs `PlaneCache`: (레이어, 레벨, 깊이 키)별 OR 합본을 개수 512에 더해 바이트
  512 MiB(`PLANE_CACHE_BYTES`)로 묶는다 — 어느 쪽이든 넘길 항목이 들어오면 비운다(상한보다 큰 평면 하나는 혼자
  남는다). 전에는 개수만이라 2^32 셀 상한에서는 GB 단위로 자랄 수 있었다(평면 하나만 그리는 경우도 `into_owned`
  복사본을 캐시한다).
- **게이트·시험.** 단위: occupancy.rs 규칙 표(152.4/228.6 mm → 4 µm, 2^30 아래에서는 8 µm, 262,144/262,145 µm
  경계, 300 mm/600 mm/1.2 m/3 m → 8/16/32/32 µm, 레티클 평면 17장 = 4 GiB 경계 → 18장은 8 µm Size), summary.rs
  `the_plane_cache_is_emptied_at_either_bound`. 게이트 `occupancy`·`jobdeck`의 상한 사례는 명시 옵션이라 그대로다.
  전체 배터리.

### 8.20 0.12.396 리뷰 수정 G — 자동 셀의 dbu 격자 판정, 가장 넓은 격자의 피라미드 산술 (0.12.397 / floe-index·renderd 0.12.355, 2026-10-24; 0.12.396 리뷰 2건, 모두 재현)

- **P2: 자동 격자 선택이 dbu 반올림을 반영하지 않았다.** `auto_base_um`은 후보 셀의 격자를 µm로 셌지만 빌더는 셀을
  정수 dbu로 반올림한다(`((um × unit).round()).max(1)`). 3 nm dbu 파일의 3999.999 µm 정사각형(1,333,333 dbu)을 셀
  상한 100만으로 만들면: 목표 규칙의 첫 선택은 1 µm(3,999.999셀 ≥ 2,048)이고 상한에 걸려 2 → 4 µm로 올라가는데,
  µm 판정은 4 µm가 1,000 × 1,000 = 100만으로 꼭 맞는다고 보았고 빌더의 4 µm는 1333 dbu = 3.999 µm라 1,001 × 1,001
  = 1,002,001 > 100만 → `every layer none:cells: 1001x1001 cells at 4um …`, `cell=3.999um (1333 dbu, auto for the
  cell cap) grid=0x0`(인메모리·스트리밍 빌더 둘 다 재현). 같은 입력에 `--occupancy-um 8`은 2667 dbu, 500 × 500으로
  정상. 수정: `auto_base_um(span_x_dbu, span_y_dbu, unit, layers, max_cells, max_bytes)`가 후보마다
  `cell_dbu_of(um, unit)`(두 빌더·덱의 pending 판정과 공유하는 한 규칙)으로 셀을 정하고 `ceil_div(span_dbu,
  cell_dbu)`로 격자를 세어 셀 수와 바이트를 판정한다; 호출자 넷(두 빌더, CLI 로그 줄, `DeckSource::summary_pending`)이
  dbu span과 unit을 넘긴다. 결과: 같은 입력이 `cell=8.001um (2667 dbu, auto for the cell cap) grid=500x500`, status
  ok. 1 nm 파일은 모든 후보가 정수 dbu라 종전과 같다.
- **P3: u32::MAX 격자에서 `(n + 1) / 2` 넘침.** 2^32 셀 상한은 u32::MAX × 1 격자(4 µm로 약 17 km 폭)를 허용하는데
  `level_count`·`layer_bytes`·`Level::pool`의 `(w + 1) / 2`가 u32에서 넘친다(릴리스는 0으로 감겨 레벨 수 27 대신 2,
  디버그는 패닉). `half_up(n) = n.div_ceil(2)`로 바꿨다(로더의 레벨 걷기는 i128 산술이라 그대로). 단위 시험:
  `half_up(u32::MAX) = 2^31`, `level_count(u32::MAX, 1) = 27`, `layer_bytes(u32::MAX, 1)`과 `(1, u32::MAX)`(행을
  바이트로 채우므로 8배)가 u64 참조 합과 같음, 그 폭의 자동 판정이 넘침 없이 (4 µm, Target)/(8 µm, Cells). 전체
  파일 생성은 하지 않았다(level 0 평면 512 MB).
- **로그 표기.** 셀의 µm를 `cell_dbu / unit`로 찍던 두 줄(빌드 로그 `cell=…um`, 덤프 `base_um=`)이 3 nm에서
  `8.001000000000001um`이 되어 게이트의 `%g` 기대와 맞지 않았다 → `occupancy::um_text`(소수 6자리에서 0 제거:
  `4`, `0.25`, `3.999`, `8.001`). 정수 dbu 셀의 표기는 그대로다.
- **게이트·시험.** 단위: `the_automatic_base_cell_climbs_for_the_caps`(규칙 표는 1 nm 환산 closure로, 3 nm 사례 추가),
  `the_pyramid_arithmetic_holds_at_the_widest_grid`, `the_cell_text_is_the_microns_trimmed`; 게이트 `occupancy`
  `test_the_automatic_cell_is_judged_on_the_dbu_grid`(3 nm 픽스처: 자동 → 8.001 µm 2667 dbu 500 × 500 ok와 덤프
  머리글, 명시 4 µm → none:cells 1001 × 1001 줄과 `cell=3.999um (1333 dbu) grid=0x0`). 전체 배터리.

### 8.21 경량 캐시 완성 — 배치 저장소, 파일 단위 깊이 상한 (0.12.398 / floe-index·renderd 0.12.356, 2026-10-24; 사용자 "어떤 상황에서도 인덱싱할 수 있어야" 검토 → "경량캐시를 완성해줘")

**배경.** 16 GB 맥북에서 9.2 GB 합성 칩(tools/gen_main01_like.py 전체: 셀 111,255, 레코드 9.9억, 배치
7.96억, 깊이 16, 449 레이어, TOP의 Doc 1.59 GB)의 `floe-index vfs --stream --mem 0.5`가 재고 133 s 뒤
"cell TOP needs about 1.5G resident, the budget is 512M … intra-cell splitting"으로 끝났다. 24 GB 호스트의
검토(rv7): 거부 자체는 설계이지만 진짜 벽은 셋이다 — (1) 전체 색인의 design.ovm 메모리 조립(1/10 파일 7.2 GB,
places 구역 배치당 66 B; 전체 외삽 65~70 GB, 디스크도 70 GB), (2) 요약 패스가 모든 배치를 96 B씩 메모리에
들어 1/10 파일(8,260만)도 상한을 넘었고 — 경량은 건너뛰고 전체는 인메모리 빌드로 넘어가 226 s·큰 메모리 — 깊이
16 × 449 레이어의 평면이 4 GiB 상한에서 93 레이어만 ok(4.29 GB), (3) 건너뛴 요약을 열 때마다 다시 시도(재고
패스 12 s/열기, 전체 파일이면 130 s). 어떤 호스트에서도 되는 길은 경량 캐시이므로 그 길의 (2)·(3)을 닫는다;
(1)은 전체 색인의 후속(구역의 디스크 스트리밍)이다.

**배치 저장소 `floe_vfs::places`.** 걷기가 배치에서 쓰는 것은 둘뿐이다: 셀의 인스턴스 열거(부모의 인스턴스 ×
부모의 그 자식 배치)와, 셀 인스턴스마다 자기 배치의 멤버 charge·작은 자식 격자의 발자국. 그래서 레코드를
**(자식, 부모) 간선별 run**으로 묶는다 — 부모의 그 자식 레코드들을 레코드 순서로, 태그 1 B(회전·미러·종류) +
x·y·반복의 지그재그 바리언트(평범한 배치 약 12 B; 점 목록은 멤버 오프셋을 그대로). `PlaceWriter::add_cell(부모,
정규화된 배치)`가 자식별로 묶어 덧붙이고(셀 하나는 한 번), `finish`가 run 표를 (자식, 부모)로 정렬해 자식별
색인(`incoming`)과 부모별 색인(`outgoing`)을 만든다. **저장 위치는 예산이 정한다**(`Backing::choose`): 재고의
배치 수 × 16 B(`BYTES_EST`)가 스트리밍 예산의 4분의 1 이하면 메모리(32 MiB 블록의 아레나), 아니면 캐시
디렉터리의 임시 파일 `design.places.tmp`(덧붙여 쓰고, 읽기는 run 단위 pread; 저장소를 버릴 때 지우며
`clear_outputs`·`clear_light_outputs`·앱의 임시 파일 정리 목록에도 든다). `FLOE_OCCUPANCY_PLACES=file|mem`이
진단·게이트용으로 강제한다. 셀 수 `n`의 run 표는 간선당 32 B(MAIN01급 칩 602,217 간선 = 19 MB; 1/10 파일
188,065). 공급처 셋이 모두 이 writer로 간다: 전체 스트리밍 빌드는 커미터가 셀을 해제할 때
(`StreamStore::keep_places_into`; 저장소 락 밖에서 덧붙임), `floe-index light`는 세그먼트 디렉터리의 배치
세그먼트를 묶음(`PIECES_BATCH_BYTES`)으로 읽으며 마지막 세그먼트가 파싱된 셀을 그 자리에서 정규화해 넘기고
(`LightIndex::placements_each`; `all_placements`는 전부를 들던 옛 방식으로 테스트의 기준), `--occupancy-only`의
배치 수집 패스는 해제되는 셀을 넘긴다. **`Instances`**는 저장소 위에서 돈다: 방문 수는 topo 순서로 부모의 수 ×
run의 `visits`(멤버 합; 작은 자식의 가지치기 가능한 격자는 0 — 걷기가 내려가지 않는 곳) 합, `each(ci)`는 자식의
incoming run마다 부모의 인스턴스를 한 번 열거하며 run의 레코드를 적용한다(전에는 레코드마다 부모를 열거했다;
순회 집합은 같고 열거는 적다). `mark_instance`의 charge·발자국은 셀의 outgoing run에서 온다(`Instances::
outgoing`: 레코드 2^18개 이하는 `PlaceView` 목록으로 실체화 — 마커당 16 MB — 그 위는 인스턴스마다 저장소를 읽음;
그런 셀은 크고 드물게 배치된다: 칩의 TOP 800만 레코드가 한 번). `Marker::mark_grid_footprint`는 `PlaceView`
(Doc의 레코드와 저장소의 레코드가 같은 뷰)를 받는다. 상주 메모리는 run 표와 마커가 열거 중 읽은 run뿐이다 — 배치
수와 무관하다. **없어진 것**: `STREAM_PLACES_CAP`(5,000만), `PLACE_BYTES_EST`·`light_places_cap`(예산/4 ÷ 96 B),
"placement records exceed … design.ovo is skipped", `--occupancy-only`의 인메모리 폴백, 그리고 그 결과였던 매
open의 요약 재시도(요약은 항상 시도되어 ovo가 남으므로 열기는 재시도할 것이 없다; 남는 배너 사유는 색인 실패뿐
— capture.rs·gui.py·SPEC-VIEWER의 문구에서 "--mem raises it"을 뺐다).

**파일 단위 깊이 상한 `depth_cap_for`.** 두 빌더 모두 마킹 전에 레이어별 top의 상대 깊이 마스크를 안다(스트리밍은
재고의 간선 fold, 인메모리는 새 `top_depth_masks`: 셀의 서로 다른 자식 위를 자식 먼저 fold — 배치 레코드가 아니라
간선 수에 비례). 레이어가 지킬 평면 수 `planes_kept(mask, k)` = 깊이 k 미만의 비트 수 + (k 이상이 있으면 1).
모든 레이어의 합 × 레이어당 피라미드 바이트가 `--occupancy-max-bytes`를 넘기면 들어가는 가장 큰 k(15에서 0까지)를
그 파일의 상한으로 삼아 `Planes`가 깊이 k 이상을 한 평면에 모은다. 자동 셀의 판정(레이어당 평면 하나)은 그대로
k = 0의 바닥이고, 사다리(8/16/32 µm)는 그것도 안 들어갈 때만 오른다 — 사용자의 우선순위(용량을 써서 4 µm를
지킨다)에 맞다. 형식은 같다(평면 depth ≤ 15, 오름차순; 리더는 k를 모르고 알 필요가 없다): 요청 depth ≥ k는 상한
평면을 그려 더 깊은 도형까지 보인다 — 지금까지 15 너머에서 그랬던 의미가 k로 내려올 뿐이고, 무제한 depth의 그림은
같다. 로그 `depth planes capped at K (depth K and deeper share one plane): L layers' P planes at every depth
would be B bytes over --occupancy-max-bytes M; Q planes kept`. 1/10 파일(449 레이어, 깊이 16, 4 µm 격자 4,000²):
전에는 93 ok·356 none:size(design.ovo 4.29 GB), 이제 k = 0으로 449 ok — 그리고 평면이 레이어당 하나라
스트리밍 패스의 묶음 수도 준다(평면 예산 2 GB면 한 묶음: 셀을 한 번만 파싱).

**스트리밍 쓰기.** 측정 중에 보인 것: `write_ovo`가 파일 이미지를 Vec 하나로 만든 뒤 썼으므로 쓰기 순간의 피크가
피라미드 + 이미지 = 요약 파일의 2배였다(1/10 파일의 3.77 GB 요약에서 최대 RSS 10.2 GB). `occupancy::write_ovo_to`가
헤더·표(오프셋을 미리 합산)를 쓰고 평면의 레벨을 피라미드에서 바로 흘려 쓴다(`publish_ovo`가 BufWriter로 .tmp에);
`write_ovo`는 그 위의 Vec 변형이라 바이트가 같다. 피라미드 자체(요약 파일 크기)는 끝까지 상주한다 — 묶음마다
디스크에 내리는 것은 후속.

**측정(이 24 GB 호스트, 8 jobs; 1/10 파일 = main01_chip_p10.oas 909 MB, 셀 35,183, 배치 8,260만, 449 레이어, 깊이 16).**

| 실행 | 배치 저장소 | 깊이 상한 | 묶음 | 요약 시간 / 전체 | design.ovo | 피크 |
| --- | --- | --- | --- | --- | --- | --- |
| 0.12.397 `light --occupancy` | 상한 3,350만 초과 → 건너뜀 | — | — | 0 s / 12.9 s | 없음(매 open 재시도) | — |
| 0.12.397 전체 색인의 인메모리 폴백 | Doc 전체 | 15 | — | 226 s | 4.29 GB, 93 ok · 356 none:size | 8 GB대 |
| 0.12.398 `light --occupancy`(기본 예산 12 GB) | 메모리 832 MB(레코드당 10 B), 8.2 s | 3(2,603 → 1,059 평면) | 2 | 178 s / 190 s | 3.77 GB, 449 ok | 최대 RSS 10.1 GB(ps, 소스 매핑 포함; 가드 phys_footprint는 묶음 1 초반 7.0 GB = 평면 3 GB + 큰 셀들) |
| 0.12.398 `light --occupancy-only --mem 2`, `FLOE_OCCUPANCY_PLACES=file` | 임시 파일 773 MB, 6.5 s | 3 | 9 | 283 s / 292 s | 바이트 동일 | 상주 피크 ≤ 512 MB/묶음 |

인스턴스 4.84억(실체화 2,314셀); 묶음 1은 큰 셀부터라 처음 70 s에 60셀, 마커 8개의 마킹 1,041 스레드초가
시간의 대부분이다. `--mem 2`에서는 묶음마다 상주 피크가 예산의 4분의 1(512 MB) 안이고 rss는 피라미드가 쌓이며
0.8 → 4.9 GB로 오른다(피라미드 3.77 GB가 끝까지 상주하는 몫). 최대 RSS는 스트리밍 쓰기 전 10.2~10.5 GB, 후 10.05 GB — 피크는 쓰기(1.2 s)가 아니라 묶음 1 초반의 큰 셀 파싱(평면 3 GB +
셀의 실제 메모리)에 있었다; 스트리밍 쓰기는 끝의 파일 이미지 3.77 GB를 없앤 것이고, 피크를 내리는 것은 아래 두 후속이다.

**전체 칩(9.2 GB, `light --occupancy --mem 8` = 16 GB 노트북의 기본 예산, 8 jobs, 이 호스트).** 끝까지 돈다:
재고 96 s(design.inv 212 MB, design.seg 79 MB), 배치 7억 9,587만 레코드를 60만 2,217 run으로 임시 파일 7.3 GB(레코드당
9.2 B)에 64.8 s, 깊이 상한 2(2,931 평면 13.6 GB → 881 평면), 인스턴스 9억 7,237만(실체화 4,814셀), 평면 예산 2 GB에
묶음 3개(401 s + 250 s + 49 s; 마킹 2,046 + 816 + 78 스레드초, 파싱 175 + 159 + 65 s, 마커가 소스 20 GB를 읽음), 요약
768 s, **전체 865 s(14.4분)**, design.ovo 4.08 GB·449 레이어 모두 ok, 캐시 4.1 GB(임시 파일은 지워짐). 0.12.397은
요약을 건너뛰고(상한 3,350만) 열 때마다 재고 130 s를 되풀이했고, 전체 색인은 외삽 65~70 GB라 불가능했다.
메모리: 상주 예산(셀 Doc 추정 합) 피크 2.0 GB를 지켰는데도 가드의 phys_footprint는 큰 셀들을 파싱하는 구간에서
8.6~10 GB, 묶음 3(가장 큰 셀 217개)에서 14 GB까지 올랐다(`/usr/bin/time` 최대 RSS 12.2 GB) — 평면 2 GB + 쌓인
피라미드 3.8 GB + 파싱된 셀의 실제 메모리가 Doc 추정의 약 3배(배치 800만의 TOP 같은 배치 위주 셀: PlaceRec와
정규화 스크래치) + 임시 파일 read 버퍼. 16 GB 노트북에서는 `--mem 4`(평면 1 GB, 셀 1 GB 추정)로 줄이는 것이 안전하며
(묶음이 늘어 시간은 더 든다), 다음 단계의 둘이 이것을 반으로 줄인다: 묶음이 끝난 피라미드를 디스크에 내려 두는 것
(요약 파일 크기만큼의 상주 제거)과 마킹 패스의 셀 파싱에서 배치·텍스트를 만들지 않는 것(저장소가 배치를 가지므로;
Doc 추정과 실제의 차이의 대부분).

**게이트·테스트.** 단위: `places::tests`(모든 반복 종류의 왕복, 자식별·부모별 run, 가지치기 격자의 visits 0,
메모리/파일 저장소, 임시 파일 삭제, 두 번 더한 셀 거절, 예산의 저장 위치와 환경변수), occupancy.rs
`the_depth_planes_collapse_before_the_layers_are_lost`(`planes_kept`·`depth_cap_for` 표, 깊이 4 사슬: 평면 2장
자리 → k = 1(평면 0과 1..3), 1장 → k = 0, 0장 → none:size; 평탄화 동일; 왕복), `the_cell_and_the_small_cells_are
_the_builders_rule`, light.rs `placements_come_cell_by_cell_as_at_once`(세그먼트 1개 묶음과 전체 묶음이
`all_placements`와 같음, 셀마다 한 번), stream.rs `streaming_occupancy_matches_in_memory`(저장소를 메모리/파일로
번갈아 쓰며 바이트 동일; 바이트 상한 사례: 깊이 평면이 접히고 레이어는 잃지 않으며 양쪽 동일). 게이트
`light_cache`의 `check_summary_budget`(main01_like 0.001, `--mem 0.1`: 메모리 저장소와 `FLOE_OCCUPANCY_PLACES=file`
모두 기본 예산의 design.ovo와 바이트 동일, 로그가 저장 위치를 말하고 임시 파일이 남지 않음), `occupancy`
`test_the_depth_planes_are_capped_before_the_layers_are_lost`(deep.oas: 평면 4장 자리 → 상한 1에 3/0의 깊이 2가 상한
평면으로, 3장 → 상한 0, 2장 → 파일 순서의 마지막 레이어 none:size; 평탄화 동일). 전체 배터리 447(스트리밍 쓰기 전)·
448(포함) ALL OK.

**남은 것.** (a) 마킹 패스의 셀 파싱이 배치 레코드까지 만든다(TOP 800만 배치 = Doc의 3분의 1): 저장소가 있으니
`parse_cell_bytes_keeping`에 레코드 종류 필터를 더하면 상주와 시간이 준다. (b) 전체 색인의 벽 (1): ovm 구역의
디스크 스트리밍(바이트 동일)과 거대 셀의 레이어 묶음 분할(§4.7). (c) 깊은 칩의 마킹 시간은 인스턴스 수에
비례한다(전체 칩 9.7억 인스턴스에 마킹 2,940 스레드초, 8 스레드로 12분); 피라미드가 끝까지 상주하므로 요약 파일 크기만큼의 메모리는 남는다 — 묶음마다 디스크에
내리는 쓰기가 다음 단계다. (d) `rss()`가 macOS에서 `?`를 찍던 것은 가드의 phys_footprint로 대신한다.

### 8.22 경량 캐시 메모리 2단계 — 도형 전용 파싱, 피라미드 스필 (0.12.399 / floe-index·renderd 0.12.357, 2026-10-25; 사용자 "진행해줘")

§8.21의 측정이 남긴 둘을 닫는다. (1) **마킹 패스는 도형만 만든다.** `floe_oasis::doc::parse_cell_bytes_shapes`
(`parse_cell_bytes_keeping`의 변형: Builder의 `only_shapes` — PLACEMENT·TEXT 레코드를 읽어 모달 상태는 이어 가되
push하지 않는다)와 그 위의 `StreamStore::parse_cell_shapes`·`parse_range_shapes`를 markpass가 쓴다. 배치는 배치
저장소가 가지고(§8.21) 텍스트는 마킹에 쓰이지 않으므로 바이트는 같다(게이트 `vfs_stream`·`light_cache`, 단위 테스트
`streaming_occupancy_matches_in_memory`). 입장 과금도 도형 추정으로 바꿨다(`StreamStore::cell_shape_bytes` = Doc
추정 − 배치 수 × size_of PlaceRec − 텍스트 수 × size_of TextRec; 점 목록·문자열의 힙은 남겨 둔 상한). 배치 위주의 큰
셀(칩의 TOP: 레코드 1,609만 중 배치 805만)은 상주가 절반 아래로 준다. 리더 풀의 `parse_cell`·`cell_bytes`는 그대로다
(전체 빌드는 배치가 필요하다). (2) **피라미드 스필.** `StreamOcc::spill_to(design.planes.tmp)`가 켜지면 `end_group`이
완성한 평면의 레벨 비트를 그 파일에 덧붙이고(`Spill::put`) 메모리에는 크기만 남긴다(빈 bits). `finish_spilled`가 요약과
스필을 함께 돌려주고, `write_ovo_to(occ, spill, w)`가 표를 쓸 때 스필의 길이를, 본문을 쓸 때 스필에서 8 MiB씩 읽어
복사한다(`write_ovo`는 스필 없는 Vec 형태, `write_ovo_with`가 스필 있는 것). 스필 쓰기가 실패하면 그때부터의 레벨은
메모리에 남고 파일은 그래도 온전하다(한 줄 경고). 스필 파일은 Spill이 버려질 때 지워지고 `clear_outputs`·
`clear_light_outputs`·앱의 임시 파일 정리에도 든다. 디스크는 묶음 동안 임시 파일(배치) + 스필, 쓸 때 스필 + .tmp다.

**측정(이 24 GB 호스트, 8 jobs; 0.12.398 대비).**

| 실행 | 0.12.398 | 0.12.399 |
| --- | --- | --- |
| 1/10 칩 `light --occupancy`: 최대 RSS / 가드 피크 | 10.05 GB / 7.0 GB | 6.9 GB / 5.3 GB |
| 같은 실행: 상주 과금 피크 / 마커 파싱 / 묶음 뒤 rss / 전체 시간 | 2.0 GB / 15.1 s / 3.5 GB / 165 s | 667 MB / 4.8 s / 1.3 GB / 160 s |
| 전체 칩 `light --occupancy --mem 8`: 가드 피크 / 최대 RSS | 14 GB / 12.2 GB | 7.2 GB / 10.0 GB(소스 매핑 포함) |
| 같은 실행: 묶음별 파싱 / 요약 / 전체 시간 | 175·159·65 s / 768 s / 865 s | 49·58·40 s / 547 s / 643 s(10.7분) |

스필은 전체 칩에서 3.8 GB, 게시 때 복사 4.0 s(1/10 칩 2.8 s). design.ovo는 두 파일 모두 0.12.398과 바이트 동일이고 임시 파일
둘은 끝나며 지워진다. 16 GB 노트북의 기본 예산(`--mem 8`)이 이제 가드 상한 14.4 GB에 넉넉히 든다.

**게이트·테스트.** 단위: occupancy.rs `the_spilled_pyramids_write_the_same_file`(레벨을 번갈아 스필에 두고 쓴 파일이
메모리의 것과 같음, 스필 파일은 함께 사라짐), stream.rs `streaming_occupancy_matches_in_memory`(파일 저장소 실행에서
스필을 켜 바이트 동일, ok 평면이 모두 스필에, 메모리에 피라미드 없음, 스필 파일 삭제); 게이트 `light_cache`가 임시
파일 둘(배치·평면)이 남지 않음을 확인. 전체 배터리 449 ALL OK.

**남은 것.** 전체 색인의 벽(ovm 구역의 디스크 스트리밍, 거대 셀의 레이어 묶음 분할). 파싱한 셀의 실제 메모리가 도형
추정보다 큰 몫(폴리곤 점 목록·Vec 여유 용량)은 추정식의 일이다.

### 8.23 레이아웃의 요약 가지치기 — 요약이 그리는 뷰에서 걷지 않는다 (0.12.400 / floe-index·renderd 0.12.358, 2026-10-25; 사용자 보고 → 검토 → "진행해줘")

**보고.** 맥북에서 9.2 GB 합성 칩의 경량 캐시(요약 포함)를 뷰어로 열면 밀도 기본 on에서 fit이 오래 걸리고 off면 빠르다;
`+`로 8~9회 줌인하면 그때부터 프레임이 20초 넘게 걸린다.

**측정(이 호스트, 전체 칩의 경량 캐시, 뷰어와 같은 요청: 요약 on, frames on, 모든 레이어, thin cull, 800 px).**

| 뷰 | 밀도 on | 밀도 off | 프레임의 내용 |
| --- | --- | --- | --- |
| fit 첫 프레임 | 254 s | 246 s | 요약 449 레이어 전부(level 2), 예산 probe 240~248 s, 채움 1라운드 |
| fit 같은 배율 두 번째 | 0.4 s | 0.4 s | 배율별 예산 결정 기억(`fit_memory`) |
| 8 mm | 5.8 s | 5.7 s | 요약 level 1, 채움 2라운드 |
| 4 mm | 58 s | 51 s | 요약 level 0, probe 45~50 s, 채움 3라운드 |
| 3 mm · 2 mm · 1 mm · 500 µm | 0.1~0.5 s | 0.1~0.5 s | 요약 경계를 지난 페이지 경로(`near`), 채움은 끝난 뒤 |

밀도 on/off의 차이는 몇 초다. "off면 빠름"은 같은 배율의 두 번째 프레임이 0.4 s인 것이다. `+`는 1.25배라 8회면 6배이고,
4 µm 셀이 1 px가 되는 요약 경계가 그 근처다(폭 1,000 px 창에서 fit의 5~6배). 느린 구간은 요약 구간 안의 새 배율마다
하는 예산 probe와 큰 셀의 배치 채움이고, 경계를 지나면 빠르다.

**원인.** renderd의 레이아웃 경로는 요약이 그리는 레이어의 서브트리를 `!frames`일 때만 걷지 않았다(`page_plan_request(…,
!command.frames)` 일곱 곳: pre-key, pass 1, pass 2의 fine 플랜 다섯). 덱 경로는 0.12.395(§8.18)에서 "frames on이어도 전체
깊이면 가지치기"(`prune_summary_when`: 계층 프레임은 깊이 경계 r == 0에서만 나오고 전체 깊이에서는 닿지 않는다)로 바꿨지만
레이아웃은 옛 규칙이었다. 뷰어의 기본은 frames on이라 요약이 449 레이어를 모두 그리는 fit에서도 걷기가 끝까지 내려갔고,
경량 캐시에서는 그 걷기가 TOP(배치 805만)과 그 아래 셀들의 배치를 채우며 분 단위가 걸렸다. 새 배율의 예산 probe는 그 요청을
복제하므로 같은 걷기를 또 했다. 밀도 2패스(경량 캐시에 design.ovs가 없어 플랜 기반)도 같은 규칙으로 플랜한다.

**수정.** 프레임마다 `prune_summary = prune_summary_when(command.frames, command.depth, cache.max_depth())`를 한 번 구해
일곱 곳이 쓴다(`floe_render_core::prune_summary_when` 내보냄). 제한 depth(프레임이 보일 수 있음)는 전과 같이 걷는다.
게이트를 쓰는 중에 하나 더 드러났다: **라벨 플래너**(`floe_vfs::text::plan_labels`)는 요약과 무관하게 자기 걷기를 하고,
경량 캐시에서는 지나는 셀마다 배치 채움을 요청했다(`light_touch`) — 컷(3 px)보다 큰 셀은 모두 내려가므로 fit에서 다섯
단계, main01_like 0.001에서 5라운드, 칩에서는 TOP 800만부터 분 단위다. 뷰어와 캡처의 라벨 기본값은 on이다. 이제 경량
캐시의 라벨 걷기는 이미지가 가진 것만 쓰고 채움을 요청하지 않는다: 배치가 아직 없는 셀은 내려가지도 그 셀의 텍스트를 읽지도
않고(`Ovm::light_unfilled_places`; 텍스트는 배치와 함께 채워진다), 배치가 없는 셀의 텍스트는 전처럼 채운다
(`TextStats::light_skipped`). 그래서 요약이 그리는 광역 뷰에는 경량 캐시의 라벨이 없고, 줌인해 기하 플랜이 셀을 채우면
그 셀들의 라벨이 따라온다. 후속: 텍스트만 채우는 fill(TOP의 다이 이름을 fit에서도 보이게; 이미지의 셀 레코드를 두 번
덧대야 한다).

**경계의 첫 프레임.** 가지치기 뒤 요약 구간은 1~3 s인데(fit 254 → 1.1 s, 8 mm 1.9 s, 4 mm 3.1 s, 채움 0라운드), 요약
경계를 처음 지나는 3 mm 프레임이 1,500 s 안에 끝나지 않았다. 추적(FLOE_LIGHT_LOG): 시도마다 예산 probe가 처음부터 다시
돌고(16~25 s) 경량 시한(300 ms) 때문에 채움은 한 라운드씩만 되는데, probe가 끝나기 전의 플랜은 결정 없이 요청된 컷으로
전부 걷어 한 라운드에 3,238셀·배치 1억 1,500만(이미지 10 GB)을 채우고, 다음 시도의 probe 채움이 그것을 비우고 다시
채우는 순환(1,278셀·9,100만 배치가 20 s씩, 비움 3,039)이 70회 넘게 되풀이됐다. 전에는 요약 구간의 느린 probe들이 TOP·L1·L2
(4,700만 배치, 4.1 GB)를 미리 채워 두었기 때문에 3 mm 프레임이 0.5 s였던 것이다. 수정: 경량 캐시에서 probe의 채움이 끝나지
않은 시도는 플랜을 돌리지 않고 바로 다음 시도로 넘긴다(`LIGHT_CONTINUE`; 프레임 게시 없음). probe는 시도마다 한 단계씩
채우며(쓴 셀은 유지) 결정이 나면 그 결정으로 플랜하므로, 전과 같은 얕은 채움 집합으로 수렴한다.

**측정(수정 뒤, 전체 칩의 경량 캐시, 뷰어 요청, 라벨 off, 새 워커).**

| 뷰 | 0.12.399 | 0.12.400 | 비고 |
| --- | --- | --- | --- |
| fit 첫 프레임 | 246~254 s | 1.1 s | 요약 449 레이어, 채움 0, probe 수 ms |
| 8 mm | 5.7 s | 1.9 s | 채움 0 |
| 4 mm | 51~58 s | 3.1 s(새 워커 5.4 s) | 채움 0 |
| 3 mm(요약 경계 첫 프레임) | 0.5 s(앞 probe들이 채운 뒤) | 27.6 s | 채움 6라운드: TOP 2.2 s, 1,383셀 요청·3,238셀 채움(배치 1억 1,500만) 17.8 s |
| 2 mm | 0.1 s | 6.3 s | 채움 3라운드(300셀 요청·872 채움) |
| 1 mm | 0.2 s | 0.2 s | |

fit에서 경계까지의 합: 0.12.399 약 310 s + 0.5 s, 0.12.400 약 6 s + 28 s. 라벨 on은 요약 구간에서 더 이상 채우지
않는다(라벨은 기하가 채운 셀의 것만). **남는 벽**: 경계 프레임의 작업 집합이 이 칩에서는 배치 1억 1,500만(이미지 11 GB,
채움당 약 93 B)이라 1 GiB 배치 예산을 열 배 넘겨 "요청이 쓰므로 유지"된다 — 16 GB 노트북에서는 스왑이다. 경량 캐시의
채움은 힙이고 전체 색인은 design.ovm을 mmap하므로(파일 캐시) 생기는 차이다. 다음 라운드: 채움을 파일에 두고 mmap하는 것
(L7 후속 "세그먼트 캐시 디스크 보존")과, 배치 예산을 넘는 셀을 채우지 않고 bbox 워시로 그리는 **배치 예산 fit**(요약
pass의 `summary_building`처럼 "아직 안 채운 셀"을 그림에 남기고 예산 안에서만 채움). probe 걷기가 시도당 16~25 s인 것
(`reps`가 켜지면 `decide_by`가 꺼져 전부 걷는다)도 그때 본다.

**게이트.** `light_cache` `check_summary_prune`(main01_like 0.001의 경량 캐시+요약, prefill off: fit 캡처가 frames on/off
모두 `[light] round` 없이 바이트 동일, depth 1은 프레임을 그리고 라운드 수가 같음) — 덱의 `jobdeck` FullDepthPruneTests와
같은 형태; 라벨 on도 채움 0. 전체 배터리 452(가지치기·라벨)·453(probe 우선) ALL OK.

**남은 것.** 요약 경계를 처음 지나는 프레임의 큰 셀 채움(디코드 + BVH 인코딩; 한 번), design.ovs 없는 경량 캐시의 밀도
2패스(플랜 기반; 요약 뷰에서는 이제 가지치기로 가벼움, 근접 뷰에서는 측정 뒤 판단) → **§8.24(0.12.401)**: design.ovs가
없으면 밀도를 거부하기로(사용자 결정), 플랜 폴백은 없어졌다.

### 8.24 파일이 없으면 거부 — design.ovs 없는 밀도, design.ovo 없는 요약 (0.12.401 / floe-index·renderd 0.12.359, 2026-10-25; 사용자 결정)

**결정.** §8.23 뒤 사용자: ".ovs가 없으면 density on을 거부하고 ovo가 없으면 occupancy on을 거부하는게 맞는 동작 같음",
그리고 "캡쳐에서 명시적으로 --occupancy on이라면 요약을 먼저 만들고 그리는게 맞음". 지금까지는 둘 다 파일이 없으면
조용히 다른 경로로 그렸다 — 밀도는 페이지 플랜 기반 2패스(경량 캐시에는 design.ovs가 없어 §8.23의 칩에서 근접 뷰의
2패스가 분 단위), 요약은 perf 줄에 `summary: none (nofile)`을 적고 페이지 경로.

**규칙.**
- 밀도(캐시의 design.ovs). renderd `run_render_attempt`는 밀도 스택이 켜질 요청(exact 아님, area-true,
  `density_stack_on`)에서 캐시에 파일이 없으면(`Cache::has_density_file`) 요청의 `density`를 끔으로 바꿔 그린다 — 1패스의
  2패스 예약, fit·보관 프레임 키, 2패스까지 끔 프레임과 바이트 동일 — 프레임 줄에 `density_none=nofile`(있으면 `-`). 플랜
  기반 2패스 폴백은 없다; `FLOE_RUST_DENSITY_OCC=off`(킬 스위치)는 진단용으로 플랜을 유지한다. 탑의 파일이 있는 캐시에서
  자기 design.ovs.<셀>이 없는 셀의 root 뷰는 종전대로 플랜(CUT_DENSITY_DESIGN §10.16; 밀도 게이트 `occ_root_checks`의
  SML), 파일이 있으나 읽히지 않는 경우도 종전대로 `occ_density`의 줄과 플랜이다. 캡처(`floe2 render/capture`)는 app-core
  `render::density_for`: 덱은 끔, 명시적 `--density on`은 design.ovs가 없으면 입력 오류로 거절(floe2는 exit 1, floe2-web은 2; 경량 캐시 "a light cache draws
  no density; the full index has it", 전체 색인 "`floe-index ovs CACHE` adds it to a full index"), 말하지 않으면 기본
  켬은 파일이 있을 때만. 뷰어(gui.py `_density_available`): 토글 켬 거절 + 하단 줄, `--density on`/넘어온 `density=on`
  거절(stderr `[floe2] --density on refused: …`), 요청의 `density`는 파일이 있을 때만 켬, 라벨 ` · density:none (no
  design.ovs)`(renderd의 `density_none=nofile`도 같은 라벨). `floe2 render --help`의 `--density` 설명도 파일을 말한다.
- 요약(design.ovo). renderd는 명시적 `occupancy=on`에 선택 사유가 `nofile`/`invalid`이면 오류로 거절한다 — "occupancy=on
  asked but the cache's design.ovo is absent: `floe2 index SRC --occupancy-only` makes it"(덱 pass는 `run_deck_render`,
  `summary_building` 규칙 그대로; render-core가 `NONE_NOFILE`/`NONE_INVALID`를 내보낸다). 말하지 않은 요청
  (`FLOE_RUST_OCCUPANCY=on`의 레이아웃)은 종전처럼 `nofile`과 페이지 경로. 뷰어(`_occupancy_available`: 레이아웃은
  design.ovo, 덱은 참): 토글 켬 거절 + 하단 줄 "occupancy summary needs design.ovo in the cache (floe2 index SRC
  --occupancy-only adds it)", `--occupancy on`/넘어온 `occupancy=on` 거절(정하지 않은 상태로 둠). 캡처의 명시적
  `--occupancy on`은 종전(0.12.388)대로 요약을 먼저 만들고 그린다(자동 색인; `--no-index`면 renderd의 거절이 오류).
- perf 필드: 프레임 줄 끝의 `density_none`을 app-core `view/perf.rs`와 oracle `rust_render.py`가 넘긴다.

**게이트.** `density_stack` `occ_density_checks`의 design.ovs 없는 사례: 켬 프레임 = 끔 프레임(바이트 동일),
`density_none=nofile`, `density_plan2`의 occ_layers·regions 0; 킬 스위치 워커는 종전 플랜 프레임과 동일. `occupancy`
게이트의 파일 없는 사례: 명시적 occupancy=on이 "design.ovo … --occupancy-only"로 거절. `light_cache` `check_summary_prune`:
경량 캐시에 `floe2 render --density on`이 입력 오류 exit 1, stderr에 design.ovs·light cache; 같은 게이트의 depth 1 사례는 바뀌었다 —
frames on은 depth-1 셀을 채우는 한 라운드, frames off는 0(요약 가지치기; 0.12.400까지는 경량 캐시의 밀도 2패스 플랜이 양쪽을
똑같이 채워 "라운드 수가 같음"이었다). `rust_renderer`의 뷰어 토글 테스트 두 건도 파일 규칙으로 갱신(파일 없는 스텁 캐시는 토글과
넘어온 on이 거절되고 패치는 끔; 임시 디렉터리의 design.ovs/design.ovo로 종전처럼 켜짐). 검증: 유닛 테스트(render-core 440,
renderd 203, app-core 34, app-cli 25) 통과; 배터리 454(`--only occupancy,density_stack,light_cache`: 앞의 둘 통과, light_cache는
거절 종료 코드 기대값 2→1 수정), 455(전체: unit~vfs_stream 23개 통과, light_cache의 depth 1 기대값 갱신), 456(light_cache~gtk_view
25개 통과, rust_renderer의 토글 테스트 갱신), 457(rust_renderer·klayout 통과) — 같은 바이너리로 모든 게이트 통과(게이트 기대값 갱신
세 건은 모두 이 변경이 설계한 동작).

### 8.25 조밀한 도형 격자의 외곽 채움 — 멤버 수가 아니라 셀 수에 비례하는 마킹 (0.12.405 / floe-index·renderd 0.12.361, 2026-10-26; 현장 → "변경 진행해줘")

**현장.** 레벨 5의 요약을 모두 만든 뒤에도 1 pass가 요약 없이 남아 첫 화면 745.5 s(`summary 133 passes`).
`tools/check_deck_summaries.py`가 그 소스를 가렸고 레이어 45/0의 status가 `none:work`, `work=68719510400`(2^36 상한에서
멈춘 값)이었다. `--occupancy-um 16`으로 다시 만들어도 45/0과 45/300이 같은 자리에서 멈췄다 — 셀 크기와 무관하므로 작업량이
도형 수에 지배된다는 뜻이다. 요약 마킹의 작업량은 칠한 셀 + 멤버 + 변 행이고, 사각형의 조밀 축 정렬 격자만 외곽 한 번이었으며
다각형·경로의 격자는 멤버마다 charge와 스캔 변환을 했다(fill 배열 = 수십억 멤버).

**변경.** `--occupancy-prune 1`(기본)에서 다각형·경로의 축 정렬 Grid가 (1) 멤버 둘 이상, (2) 멤버의 world bbox가 두 축 모두
셀 이하, (3) 축마다 틈(피치 − 멤버 폭)이 셀보다 좁으면 `grid_fill_closed_form`이 외곽 사각형을 돌려주고 멤버 0을 쥔
범위가 그것을 한 번 칠한다(`mark_poly_pts_range`; 경로는 외곽선 hull로 같은 길). (3)은 사각형의 `rect_closed_form`과
같은 판정(`grid_closed_form`으로 일반화). unit 생성기(`poly_splits`/`path_splits`가 xf·셀·prune을 받음)와 Shapes 러너가 같은
판정이라 그런 레코드는 Members unit으로 쪼개지지 않는다. 그림은 prune 계약 안이다: 멤버가 2×2 셀을 넘지 않으니 bbox가
닿는 셀은 도형이 닿는 셀의 한 셀 이웃이고, 틈이 셀보다 좁으니 틈에 든 셀 옆에는 멤버 bbox가 있다. 멤버가 셀보다 큰 조밀
격자는 셀당 멤버가 최대 넷, 성긴 격자(틈 ≥ 셀)는 멤버 수 ≤ 셀 수라 둘 다 종전 멤버별 경로로 충분하다. 사각형 격자(정확한
닫힌형)와 `--occupancy-prune 0`(오라클 게이트)은 그대로다. 킬 스위치 `FLOE_OCCUPANCY_GRID_FILL=off`.

**검증.** 단위 테스트 `a_dense_grid_of_small_shapes_is_one_footprint_under_the_prune`: 4 dbu 삼각형 400×300(피치 9×7, 셀
10)과 회전 배치된 셀 안의 경로 200×100(외곽 6×2, 피치 8×5) — pruned는 외곽의 모든 셀, exact의 한 셀 안 superset, 작업량
pruned < 20,000 / exact ≥ 120,000(삼각형); 25 dbu 멤버의 조밀 격자와 피치 30의 성긴 격자는 pruned = exact 평면. 게이트
`occupancy` PruneContractTests: fixture에 4/0(삼각형 0.3 µm, 피치 0.5×0.7 µm)·5/0(경로 0.4×0.2 µm, 피치 0.6×0.5 µm) 40×30 격자를
OASIS 반복으로 더해 계약(exact ≤ pruned ≤ dilate 1셀)·깊이 평면·작업량(exact ≥ 1,200, pruned의 4배 미만) 검사(fixture는 `opt.format =
"OASIS"`로 써야 반복이 생긴다 — 옵션을 주면 KLayout이 확장자를 보지 않고 GDS로 쓴다). **측정**(0.3 µm 삼각형, 피치 0.5×0.7 µm,
2000×2000 = 4 M 멤버를 OASIS 반복 하나로; `floe-index light --occupancy --jobs 4`, 옛 경로는 킬 스위치): 자동 셀 0.5 µm에서 work
28,800,000 → 5,600,000(= 칠한 셀 수), 레이어 마킹 0.5 s → 0.0 s; `--occupancy-um 4`에서 work 24,775,000 → 87,500(283배), 0.7 s →
0.0 s; 레벨 0 평면은 둘 다 동일(5,600,000·87,500 셀 점등, 차이 0). 전체 배터리 464 ALL OK(463은 fixture 형식 수정 전 중단).

**남은 것.** 멤버가 셀보다 크고 bbox보다 훨씬 작은 도형(대각 띠 등)의 조밀 격자는 멤버별 그대로다(셀 수에 묶여 있어 비용은
유한). 현장 소스의 재생성 결과(45/0·45/300의 `work=`와 시간)는 사용자 보고를 기다린다.

### 8.26 작은 도형은 bbox로, 상한은 소스 크기에 비례 (0.12.410 / floe-index·renderd 0.12.362, 2026-10-27; 현장)

**현장.** 0.12.405 이후 빌드로 290 GB EBEAM 파일의 요약을 만들었는데 레이어 3134/0이 여전히 `none:work`였다:
`[vfs] occupancy cell=2um (40000 dbu, auto) grid=3000x3350 levels=7 layers=6 ok=3 empty=2 jobs=12 4.8M (3618.2s)`,
`layer 3134/0 none:work (work 68719514762) - over --occupancy-max-work 68719476736 marks`. §8.25는 반복 배열만 다뤘다.
평탄한 EBEAM 데이터는 대부분 반복 없는 POLYGON 레코드와 임의 오프셋 반복(타입 10/11)이고(2026-10-19 50 GB 소스의 scan:
POLYGON 85%, 점 목록 반복 2.38억), 그 멤버는 하나하나 스캔 변환을 했다 — 변마다 행 charge, 행마다 활성 변 + 1 —
셀 하나를 켜는 작은 도형에 수십 charge였다.

**변경.** (1) `--occupancy-prune 1`(기본)에서 다각형·경로 외곽선의 world bbox가 두 축 모두 셀 이하이면 멤버마다
그 bbox가 닿는 셀을 칠한다(`small_member_box` → `mark_world_rect`; 레코드 하나든 반복이든, 경로는 hull로 같은 길).
bbox는 최대 2×2 셀이고 도형이 닿는 셀을 모두 담으니 prune 계약(exact ≤ pruned ≤ dilate(exact, 1셀)) 안이다. 넓이 0
외곽선은 정확 경로와 같이 아무것도 칠하지 않고(`twice_area`), 셀보다 큰 도형과 `--occupancy-prune 0`(오라클 게이트)은
그대로 스캔 변환한다. 킬 스위치 `FLOE_OCCUPANCY_SMALL_BOX=off`. (2) `--occupancy-max-work`를 주지 않으면 floe-index(`light`·
`vfs`)가 상한을 소스 1바이트당 4, 최소 2^36으로 잡는다(`occupancy::default_max_work`, `WORK_PER_SOURCE_BYTE`) — 16 GiB 이하는
종전과 같고 290 GB는 1.16조. 이 레이어의 마크는 도형이라 파일을 읽는 시간에 비례하고, 작은 파일이 반복으로 수십억 멤버로
불어나는 경우는 고정 하한이 막는다. 요약 없는 레이어의 로그는 `- over --occupancy-max-work N marks (by default 4 per byte
of the file, at least 68719476736: raise it)`.

**측정**(합성: 20꼭짓점 OPC형 도형 0.3~1.5 µm, 4 mm 정사각형에 반복 없는 레코드 100만 + 2,000개 도형 × 500 오프셋의 점 목록
반복 = 멤버 200만, 57 MB, dbu 0.05 nm; `floe-index light --occupancy --occupancy-um 2 --jobs 4`, 옛 경로는 킬 스위치): work
64,989,991 → 4,951,225(13.1배; 멤버당 32.5 → 2.5), 마킹 1.5 s → 0.2 s; 레벨 0 점등 2,507,418 → 2,510,406 셀(+2,988, 0.12%) —
정확 표시의 셀은 하나도 빠지지 않고 늘어난 셀은 모두 한 셀 안. `--occupancy-prune 0`의 design.ovo는 바이트 동일. 같은
비율이면 현장 레이어는 상한에 닿을 때까지 멤버 약 21억을 처리했고, 이제 같은 멤버가 약 53억 charge다.

**검증.** 단위 테스트 `a_small_shape_is_its_bbox_under_the_prune`(12꼭짓점 도형 레코드 60개 + 40 오프셋 반복 + 셀 모서리의 삼각형
+ 8×2 경로: exact의 한 셀 안 superset, 삼각형의 네 번째 셀, pruned work ≤ 멤버 × 5, exact ≥ 4배; 25 dbu 삼각형은 exact와 평면
동일; 넓이 0은 둘 다 empty — 킬 스위치를 켜면 실패), `a_large_source_gets_a_work_cap_by_its_size`. §8.25 테스트의 계약 검사를
모듈 함수 `within_one_cell`로 옮겨 함께 쓴다.

**남은 것.** 현장 파일의 재생성 결과(3134/0의 status·`work=`와 빌드 시간)를 기다린다.

### 8.27 재고 매핑의 페이지와 occupancy-only의 재고 재사용 (0.12.411 / floe-index·renderd 0.12.363, 2026-10-27; 현장 질문 → "진행해줘")

**현장.** 290 GB EBEAM 파일에 0.12.410으로 `floe2 index <file> --occupancy-only`를 돌리니 재고 패스가
`[vfs] stream: inventory... (1920s, rss 216G)`를 보였다. 사용자 질문: 스트리밍인데도 rss가 200 GB를 넘는 게 어쩔 수 없는지.

**원인.** `stream_open`은 소스 전체를 읽기 전용으로 매핑해(memmap2, MAP_SHARED) `inventory_segments`로 훑는다. 훑은 페이지는
매핑이 풀릴 때까지 프로세스에 상주하고, 진행 줄의 rss(`/proc/self/statm`의 resident)는 이 파일 페이지(RssFile)를 포함한다. 힙
(RssAnon)은 셀 표와 세그먼트 목록뿐이라 몇 GB다(세그먼트당 0.5~1 KB × CBLOCK 수백만 개). 메모리 가드는 전부터 RssAnon만 본다.
매핑은 재고가 끝나면 풀리고 이후 패스는 pread로 읽는다(page cache, rss 아님). 맥(floe-index 0.12.362, `light --jobs 4`)에서 2.2 GB
비압축 합성 칩(gen_route_chip `--scale 8`)의 ps rss는 단일 스레드 skim 동안 초당 약 140 MB로 2.14 GB(파일 크기)까지 오르고 parse
내내 그대로였고, 힙(phys_footprint)은 끝에 14 MB였다. 그리고 `--occupancy-only`는 경량 캐시가 이미 design.inv로 들고 있는 재고를
얻으려고 소스를 처음부터 다시 훑고 있었다.

**변경.** (1) `light --occupancy-only`는 재고를 캐시의 design.inv에서 읽는다(`vfs::cached_inventory`: `invfile::read`에 지금
소스의 크기·mtime 대조; 디코드나 identity가 틀리면 아무것도 쓰지 않고 "rebuild the light cache"로 종료 1). 재고는 소스 바이트만으로
정해지고 design.seg는 design.inv의 CRC로 짝이 맞으니 훑은 것과 같은 재고다(소비자가 읽는 필드는
`the_round_trip_is_byte_identical_and_field_identical`가 고정). 로그는 `inventory ... records from design.inv in 0.0s`. 킬 스위치
`FLOE_LIGHT_INV_REUSE=off`. 전체 색인의 `vfs --occupancy-only`는 그대로 훑는다(그 design.inv는 design.ovm의 짝이 아닌 옆 파일).
(2) 재고 걷기가 지나간 바이트를 돌려준다: `inventory_with`·`inventory_segments_with`가 `floe_oasis::Release { step, f }`를 받아
걷기마다(단일 job의 걷기, 병렬의 skim·head·워커별 셀 묶음) 약 `step` 바이트마다와 끝에서 `f(phase, from, to)`를 부르고(`Behind`,
레코드마다 비교 한 번), floe-index는 그 구간을 페이지 경계로 madvise(DONTNEED)한다(`INVENTORY_RELEASE_STEP` 32 MiB;
`FLOE_INVENTORY_RELEASE=off`면 쥔다). 읽기 전용 공유 매핑이라 다시 닿은 페이지는 파일 내용 그대로 다시 매핑되고, page cache는
남으니 skim 뒤의 parse가 디스크를 다시 읽지도 않는다. 걷기 하나가 쥐는 매핑은 약 32 MiB(워커 12개면 약 400 MB). (3) 진행 줄은
`inventory... (1920s; skimmed 290G of 290G, parsed 120G of 290G; rss 2.3G: heap 2.1G, file 0.2G)` — 돌려준 양으로 진행을
보이고(전에는 rss가 사실상 진행 표시였다), Linux에서 rss를 RssAnon과 RssFile로 나눈다(그 필드가 없는 4.5 이전 커널은 statm의
resident·shared; macOS는 `rss X`).

**측정**(맥, release, `--jobs 4`). 280 MB 합성 칩(gen_route_chip `--scale 1`)의 경량 캐시에 `--occupancy-only`: 재고 3.4 s →
0.0 s, 전체 6.8 s → 3.6 s, design.ovo는 옛 바이너리·새 바이너리·킬 스위치 셋이 바이트 동일; KLayout CBLOCK 파일(57 MB)도 셋이
동일. 페이지 반납은 맥에서 보이지 않는다: 2.2 GB 칩의 ps rss가 반납을 켜도 꺼도 2.14 GB — macOS의 madvise(DONTNEED)는 파일 매핑의
페이지를 비활성 목록으로 옮길 뿐 프로세스에서 떼지 않는다. Linux는 호출하는 즉시 프로세스의 rss가 준다(madvise(2); memmap2 문서
"The resident set size (RSS) of the calling process will be immediately reduced") — 현장 진행 줄의 `file`로 확인한다.
design.inv·design.seg는 반납 유무로 바이트 동일.

**검증.** floe-oasis `a_walk_hands_back_what_it_passed`(fixture 5종 × jobs 1·3 × 세그먼트 유무: 재고·세그먼트가 반납 없는 걷기와
같고, phase마다 구간이 겹치지 않고 magic부터 이어지며, skim은 END의 id 뒤에서, parse는 병렬이면 END 앞·단일 job이면 END의 id
뒤에서 끝남; 7바이트 step이라 여러 번), floe-index `the_rss_splits_into_heap_and_file`; 게이트 `light_cache`: `--occupancy-only`가
`records from design.inv in`이고 소스를 훑지 않음, 킬 스위치로 훑은 것과 design.ovo 동일, 헤더 뒤가 깨진 design.inv는 종료 1에
design.ovo 없음, `FLOE_INVENTORY_RELEASE=off`로 만든 경량 캐시와 design.inv·design.seg 동일.

**남은 것.** 병렬 걷기는 셀 개수로 묶으므로(`per = n_units.div_ceil(jobs)`) 한 셀이 대부분인 파일은 skim 뒤 parse도 사실상 한
스레드다. 경량 캐시를 처음 만들 때의 재고 시간이 현장에서 문제가 되면 다음 순서다.

### 8.28 덱 배율의 자동 셀 (0.12.414 / floe-index·renderd 0.12.364, 2026-10-28; 현장 → 배율 확인)

**현장.** 덱의 18147 × 12578 µm 뷰(창 약 1300 px, 14 µm/px)에서 "요약이 하나만 쓰이면서 디테일이 없어져버림", 상태줄은
`summary 4 passes 3.1M cells, 21 passes without summary`. `tools/check_deck_summaries.py`로 level 9의 135 소스를 보니 문제는
원본 파일이 없는 EBEAM 하나(NOFILE, pass 없음)뿐이고 나머지는 OK, base_um은 0.5·1·2·4. `floe2 jobdeck`의 `mag : [4.0]`.

**원인.** 요약 구간의 끝은 덱 좌표의 셀이 1 px이 되는 곳이고, 덱 좌표의 셀 = 소스 셀 × 배치 배율(SF × AD / 소스 dbu)이다. 자동 셀
(`auto_base_um_for_span`, 2026-09-16)은 소스 자신의 긴 변만 보고 8 mm 넘는 소스에 4 µm를 고르므로, mag 4로 놓인 27 mm 소스의 셀은
덱에서 16 µm다. 14 µm/px 뷰에서 그런 pass는 `none (near)`로 페이지 경로에 가고, 3 px cut(42 µm)에 걸린 페이지는 빠진다(덱 wash는
2026-09-16부터 꺼짐). 같은 덱의 작은 소스(0.5~1 µm → 덱 2~4 µm)만 요약으로 남아 "요약이 하나만" 보였다.

**변경.** (1) floe-vfs `placed_ceiling_um(mag)`: 덱 셀이 `PLACED_CELL_UM`(4 µm, mag 1에서 8 mm 넘는 소스가 갖는 셀) 이하인 가장 굵은
후보(4/2/1/0.5/0.25, 없으면 0.25; mag ≤ 1이면 없음). `auto_base_um_placed`는 첫 선택을 그 값과 스팬 규칙의 작은 쪽으로 하고, 상한
사다리는 그대로 거기서 오른다(`AutoWhy::Mag`, 로그 `cell=1um (… dbu, auto for the placement mag)`). `Opts.mag`(기본 1), floe-index
`light`·`vfs`의 `--occupancy-mag M`; 명시 `--occupancy-um`이 이긴다. (2) app-core `IndexOptions.occupancy_mag`(floe2의 옵션은 아님),
`jobdeck::index::source_mags`: 소스마다 덱 전체(모든 level·CHIP) 항목의 최대 SF × AD / dbu — 선택에 따라 셀이 바뀌지 않게. 배율이 1보다
크면 그 소스의 빌드·요약 추가에 `--occupancy-mag`를 붙인다. `floe2 index DECK`(`prepare`)는 있는 design.ovo의 셀이 그 배율의 자동
셀(파일 자신의 top 상자·레이어 수, 기본 셀 상한, `--occupancy-max-bytes` 또는 기본)보다 굵으면 occupancy-only로 다시 만들고
`[jobdeck] occupancy : N summary(ies) built again - …`로 센다. 여는 쪽의 자동 색인(`prepare_for_open`)은 있는 요약을 그대로 쓴다 —
290 GB 소스의 마킹을 뷰어 열기가 시작하지 않게. (3) renderd `DeckSource.mag`(`placed_mags`: 소스 unit × scale × 덱 unit의 최대):
요약이 아직 없는 경량 소스의 대기 판정(§5.5)이 색인이 고를 셀로 본다. (4) `check_deck_summaries.py`: OK 줄에 `mag=… deck_cell_um=…`,
덱 셀이 4 µm를 넘으면 `COARSE`(exit 1).

**비용.** 27 × 34 mm 소스에 1 µm은 레이어당 9.2억 셀, level 0 평면 115 MB(피라미드 포함 153 MB) — 셀 상한 2^32 안, 레이어 26개까지
바이트 상한 4 GiB 안(넘으면 사다리가 2 µm로). 마킹은 도형당 표시라 셀 크기와 거의 무관하다.

**측정**(맥, release, 합성: 1 µm 상자 200 × 200개를 50 µm 피치로 10 mm에, dbu 0.05 nm; 같은 소스를 AD로 mag 1·4에 놓은 두 덱).
mag 1 덱의 색인은 `cell=4um (auto)`; mag 4 덱의 2400 µm 뷰(300 px, 8 µm/px — 상자는 덱에서 4 µm, 반 픽셀)는 그 요약으로 0 px
(페이지 경로, cut 아래), `floe2 index mag4.jb`가 그 요약만 0.1 s에 다시 만들어 `cell=1um (… auto for the placement mag)`, 같은
뷰가 144 px(상자 12 × 12개, 하나에 한 픽셀); 다시 돌리면 두 덱 모두 kept.

**검증.** floe-vfs `a_source_drawn_at_a_mag_takes_a_cell_for_it`(천장 값, 27 mm 소스 mag 1/2/3/4/0.25, 이미 고운 소스, 상한에서 오름 —
152 mm 판은 `Cells`로 4 µm, 40레이어는 `Size`로 2 µm —, 두 빌더의 셀, 명시 셀), render-core `a_source_takes_its_placements_largest_mag`,
app-core `a_source_takes_its_largest_mag_in_the_deck`와 `the_light_cache_decisions`(인자: `--occupancy-mag 4.0`, 명시 셀이 이김, 0·음수·
NaN·inf 거절); 게이트 jobdeck `test_a_deck_at_mag_4_builds_its_summaries_for_the_mag`(위 측정의 0 → 144 px, COARSE → OK, kept, 자동 색인은
굵은 요약 유지, 명시 `--occupancy-um 2`가 이기고 다음 `index`가 다시 1 µm).

**남은 것.** 현장의 굵은 요약은 `floe2 index deck.jb`로 다시 만들어야 한다(소스별 occupancy-only, 재고는 design.inv에서). 요약이 그리는
pass의 1비트 셀이 조밀한 마스크에서 한 면으로 차는 문제(농담)는 별개다.

### 8.29 인덱스 없는 캡처와 임시 인덱스 (0.12.416, 2026-10-28; 사용자 "인덱싱이 되어 있지 않더라도 캡쳐할 수 있게 수정해줘. 인덱싱이 없는 경우, 인덱싱을 하지 않고 캡쳐하는 것이 유리한지(1회성인경우)도 확인")

**확인.** `floe2 render`는 0.12.359부터 인덱스 없는 소스를 먼저 색인하고 그린다(§8.11: 레이아웃은 전체 색인, 덱 소스는
경량 캐시, `--no-index`·`FLOE_AUTO_INDEX=off`면 거절). 캐시 없이 그리는 경로는 없다 - renderd는 셀 표와 세그먼트
(design.inv/seg)를 전제하고, 그것을 만드는 것이 경량 캐시의 재고 pass다. 실제로 못 찍는 경우는 원본 옆 폴더에 쓸 수 없을
때였다(`indexing X first: Permission denied (its index is written beside it, in DIR)`; 덱이면 소스마다 FAILED 뒤 그릴 것이
없어 실패).

**수정 `--temp-index[=DIR]`** (`render`·`info`·`probe`·`clip`; SHARED_APP_LAYER §12): 인덱스 없는 소스를 이 실행만의
폴더(DIR, 없으면 `$TMPDIR`)에 색인해 읽고 명령이 끝나면 지운다. 종류는 규칙 또는 `--light`/`--full` 그대로, 옆에 캐시가
있으면 있는 대로 연다. 덱 캡처는 첫 그림에서 요약을 기다린 pass가 있으면(`summary_building`; renderd의 판정이라 근접
캡처는 해당 없음) 빠진 요약을 임시 인덱스에 만들고 다시 그린다 - 나중에 `floe2 index DECK.jb`가 더해 줄 수 없고, 첫
그림은 기다리는 pass를 건너뛰므로 싸다. `--occupancy off`면 요약 없이 페이지로 그린다(기다리는 pass가 없다).

**측정** (Mac 8코어 24 GB, release; 합성 라우팅 칩 tools/gen_route_chip.py scale 1 = 280 MB·27.6 M 레코드·29 셀, scale 8 =
2.24 GB·221 M 레코드; 1200 px 캡처, 뷰어 기본):

| 단계 | 280 MB | 2.24 GB |
| --- | --- | --- |
| 경량 색인(재고 pass) | 3.3 s, 캐시 0.24 MB | 29.6 s, 1.5 MB (훑기 1.6 GB/10 s, 파싱 ~75 MB/s) |
| 요약 추가(`light --occupancy-only`) | 3.3 s, +36 MB | 메모리 상한 71(단일 셀 18 GB 상주, 이 호스트) |
| 전체 색인 | 15.8 s, 438 MB, rss 10 GB | 메모리 상한 71 |
| fit 캡처, 경량 | 0.8 s - **검은 그림**(밀도 없음: 컷 아래 도형이 전부 빠짐) | 7.5 s, 같음 |
| fit 캡처, 경량 + 요약(`--occupancy on`) | 0.3 s - 1비트 점유 그림(톤 없음) | - |
| fit 캡처, 전체 | 0.07 s - 밀도 그림(뷰어와 같음) | - |
| 근접 200 µm 캡처, 경량 / 전체 | 1.0 s / 0.08 s (medium: 컷 아래 밀도만 다름; exact는 동일) | 8.5 s / - |
| 자동 색인 + fit 캡처 한 번에 | 전체 14.7 s, 경량(`--light`) 4.1 s | |
| 덱(이 칩을 mag 1로 두 level) fit 캡처 | 요약 없음 0.05 s 빈 그림·3 / `--occupancy on` 4.7 s(요약 4.4 + 그림 0.3) / 요약 있음 0.3 s | |

**판정.**
- 캡처에는 최소한 경량 재고 pass(원본 전체를 한 번 읽어 셀 표·세그먼트·상자)가 필요하고, 그림은 그 위의 두 번째 읽기다.
  "색인하지 않는 캡처"는 이 pass의 결과를 디스크에 남기지 않는 것일 뿐 시간은 같다. 단일 pass로 그리려면 범위를 미리 알고
  계층이 없어야 하는데(계층 파일은 셀 표가 먼저 필요) 그래도 fit 뷰에서 재고 29.6 s 대 그림 7.5 s - 최대 25 %의 이득이라
  새 경로를 만들 가치가 없다.
- **잡덱**: 1회성이면 `--temp-index`가 맞다. 비용 = 소스마다 경량 재고 1 pass + 광역 뷰면 요약 1 pass - 영구 색인과 같고
  옆에 남기지 않을 뿐이다. 같은 덱을 두 번 이상 찍으면 영구 색인이 낫다(두 번째부터 0.3 s). 덱 뷰는 밀도를 쓰지 않으므로
  경량 + 요약이 뷰어와 같은 그림이다.
- **레이아웃**: 광역(fit) 캡처는 경량 캐시만으로는 안 된다 - 밀도(design.ovs)가 없어 컷 아래 도형이 전부 빠진다(위
  검은 그림). 전체 색인(4~5배 시간, 1800배 디스크)이 뷰어의 밀도 그림을 주고, `--occupancy on`의 요약 그림은 1비트라
  톤이 없다. 근접·exact 캡처는 `--light`로 충분하다(exact는 전체와 픽셀 동일: 게이트 floe2). 그래서 1회성 레이아웃
  캡처는 광역이면 `--temp-index`(전체 색인을 임시 폴더에), 근접이나 exact면 `--temp-index --light`.
- 현장 290 GB EBEAM: 재고 pass가 30분 이상(§8.27), 요약 pass도 그만큼이라 1회성 캡처도 시간 단위다. 같은 파일을 다시 찍을
  가능성이 있으면 영구 색인(`floe2 index deck.jb`)이 맞고, 임시 인덱스는 쓸 수 없는 폴더나 정말 한 번뿐일 때다.
- 2.24 GB 칩의 요약·전체 색인이 이 24 GB 호스트에서 메모리 상한(71)에 걸린 것은 단일 셀 18 GB 상주의 스트리밍 빌드 한계로
  별건이다(STREAMING_INDEX_PLAN §5); 경량 캐시와 그 위의 캡처는 8.5 GB rss로 끝났다.

### 8.30 캡처 창 - 위치 캡처는 그 자리의 소스만 읽는다 (0.12.421, 2026-10-29; 사용자 "특정 위치를 캡쳐한다고 할때, jobdeck의 경우 해당하는 EBEAM 파일만 읽어서 처리하도록 되어 있는지 확인해줘" → "그렇게 수정해줘")

**확인.** 그리기는 이미 해당 소스만 읽었다: renderd가 배치마다 뷰를 소스 좌표로 옮겨 소스 경계와 자르고, 겹치지 않는 배치는
plan도 fill도 페이지 읽기도 없이 건너뛴다(`source_plan_request` None → `passes_skipped`). 3칩 덱(280 MB 라우팅 칩 세 벌,
경량 + 요약 색인, 1200 px)에서 칩 A 근접 0.67 s = 칩 하나뿐인 덱의 0.66 s, 칩 사이 빈 영역 0.03 s. 열기는 로드한 레벨의
모든 소스를 건드리지만(헤더 4 KB, 캐시 meta.json, renderd의 design.inv/seg 읽기와 소스 파일 open 1회) 도형은 읽지 않는다.
공백은 인덱스가 없을 때였다: 자동 색인과 `--temp-index`는 `DeckIndexPlan::prepare_for_open`이 레벨만 받아 캐시 없는 소스를
모두 색인했고(근접 캡처에 3 소스 × 2.8 s, 합계 9.3 s), 광역 캡처의 2단계도 renderd는 기다리는 pass를 하나로 바로 판정했는데
로드한 레벨의 경량 소스 전부의 요약을 만들었다(3 × 2.3 s 더, 합계 15.4 s). 범위를 줄이는 옵션도 없었다(`--chip`/`--chip-off`/
`--fit-chip`은 열기 뒤에 보이는 칩과 영역만, `--chipid`/`--chipnumber`는 `floe2 index`에만, `--level 9`는 그 레벨의 135개
소스 그대로). 현장 레벨 9(135개 소스 ~1 TB)의 한 위치를 `--temp-index`로 찍으면 레벨 전체를 한 번(광역은 두 번) 읽는
셈이었다.

**수정 - 캡처 창(app-core `jobdeck::window`).** 덱만으로 배치의 extent를 안다(`Placement.bbox_um` = BX/BY/UX/UY를 AD·SF·ROWS와
헤더 dbu로 배치한 값; 소스 도형은 읽지 않는다). `floe2 render`는 열기 전에 샷들의 영역(자기 영역, 또는 `--chip`/`--fit-chip`이
고른 칩의 extent - 덱과 헤더만 읽는 사전 분석 `Analysis::open`의 칩 표에서, `run_summary`와 같은 `shot_region`으로)을 타일
상자째 덱 µm 창으로 모으고(`captures::deck_window`), 열기·자동 색인·임시 인덱스의 요약 2단계가 그 창과 겹치는 배치의 소스만
다룬다: `AnalysisOptions.window` → 스펙에는 창 안의 배치와 그 소스만(`CompositeSpec.outside`; renderd가 여는 소스도 그만큼),
`DeckIndexPlan::prepare_for_open(.., reads)`의 todo도 그 소스만(`outside`는 그대로 둠), ready.rs `deck_state`도 그 소스만
본다. 모델과 장부는 로드 전체 그대로라 칩 표·skipped·색은 바뀌지 않고, 그림은 덱 전체를 연 것과 바이트 동일하다(창이 빼는
배치는 renderd의 cull이 건너뛰던 배치다). 겹침은 닫힌 구간(변이 닿으면 포함)이고, extent가 퇴화한 항목(BX..UY 없음 - 파서
경고)은 알 수 없는 범위로 보고 항상 읽는다. 창이 아무 배치에도 닿지 않으면 입력 오류로 거절한다(exit 1; 예전에는 레벨
전체를 색인한 뒤 빈 그림 - 현장에서는 bbox 오타 한 번이 1 TB였다). `FLOE_DECK_WINDOW=off`면 예전처럼 모두 읽는다.
`render --chipid ID,.. --chipnumber N,N-M,..`(FIX M의 `ChipSelection` 그대로, `geom::PlanOptions.chips`)은 그 CHIP 블록의
소스와 배치만 열고 그린다 - 다른 블록은 로드하지 않은 레벨처럼 격자·extent에만 참여한다(BX..UY가 없는 덱의 수동 제어). 출력:
stdout `[jobdeck] window    : N of M source(s) read - P of Q placement(s) in the capture's region(s)`와 `[jobdeck] blocks    :
..`, stderr의 색인 줄에 `; K source(s) the capture's region does not reach are left as they are`, 보고서 `jobdeck.window`
(rects_um, sources, sources_total, placements, placements_total)와 `jobdeck.chip_blocks`, `info --json`의
`metadata.jobdeck.window`(창이 있을 때만; 뷰어·info·probe·clip·DRC 캡처는 창 없음). 배관은 SHARED_APP_LAYER §12.

**측정** (Mac 8코어 24 GB, release; tools/gen_route_chip.py scale 1 = 280 MB 칩 세 벌을 가로로 놓은 3칩 덱, 1200 px, 인덱스
없는 상태에서 `--temp-index`; 그림은 모두 영구 색인 위의 덱 전체 캡처와 바이트 동일):

| 캡처 | 0.12.420 | 0.12.421 |
| --- | --- | --- |
| 칩 A 근접 2000 µm | 3 소스 색인, 9.3 s | 1 소스 색인, 4.7 s |
| 칩 A 전체 10000 µm (요약 2단계) | 3 색인 + 3 요약, 15.4 s | 1 색인 + 1 요약, 6.9 s |
| 칩 A·B에 걸친 영역 | 3 + 3 | 2 + 2, 13.9 s |
| `--fit-chip B` | 3 + 3 | 1 + 1, 6.9 s |
| `--chipnumber 2` (영역은 덱 전체) | - | 1 + 1, 7.1 s |
| 칩 사이 빈 영역 | 3 색인 뒤 빈 그림 | 거절(exit 1), 색인 없음 |

**판정.** 현장 레벨 9의 한 위치는 그 자리의 EBEAM 한두 개만 재고 pass(광역이면 요약 pass도)를 치른다 - 135개 중. 290 GB 파일이
그 자리에 있으면 그 파일은 여전히 30분 이상이고(§8.27), 그 자리에 없으면 읽지 않는다. 인덱스가 있는 덱은 전과 같다(그리기는
이미 해당 소스만; 열린 소스 수가 창 안으로 줄어 renderd의 design.inv/seg 읽기만 준다). 전제는 덱의 BX..UY가 소스의 실제
범위를 담는다는 것이다 - MDPView가 칩을 놓는 값이라 현장 덱에는 있고, 없는 항목은 항상 읽으며, 의심스러우면
`FLOE_DECK_WINDOW=off`나 `--chipid`다. 게이트 `jobdeck` LightSummaryTests `test_a_capture_reads_the_sources_its_region_reaches`
(근접·칩 B 위·`--fit-chip mark.oas#3`·`--chipnumber 2`의 소스 수와 보고서, 그림 바이트 동일, 빈 영역 거절, 킬 스위치, 레이아웃
거절); 기존 두 테스트의 "근접" 영역 38000,81000은 모든 칩 옆의 빈 자리였으므로 칩 안(42000,81000)으로 옮기고 옛 영역은
거절 사례로 쓴다. 단위 테스트 app-core `window`(닿음·퇴화 extent·소스 집합)·`geom`(블록 선택이 격자를 움직이지 않음), app-cli
`read`(`--chipid`/`--chipnumber` 해석, 레이아웃·DRC 거절).

## 9. 검토한 대안

- **A. 전체 색인만 빠르게/작게 만들기**(ovp 압축률, 인코딩 병렬, 소스 병렬):
  시간은 최소 "파일을 두 번 읽고 한 번 쓰기"라 1 TB에서 수 시간이 하한이고,
  크기는 ovp를 없애지 않는 한 ≈ 소스다. 세그먼트 페이지가 바로 "ovp 없는 전체
  색인"이므로 이 설계가 A를 포함한다.
- **B. 셀 단위 지연 색인**(첫 방문 셀에 `build_cell_plan` 전체 → 메모리 Ovm):
  플래너·페이지가 전체 색인과 완전히 같아 픽셀 동일이 쉽다. 그러나 평면 마스크의
  가장 큰 셀(26 MiB, 레코드 118만, Doc 415 MiB)은 첫 방문이 셀 전체 파싱 + 플랜
  (수 s)이고 뷰에 무관하게 셀 전부를 든다. 세그먼트 설계의 선택지(§5.8)로 두되,
  L0 실측으로 비용이 수 s로 내려왔으므로 L1 단계의 근접 뷰(세그먼트 없이 셀
  단위)로는 쓸 만하다.
- **C. 부분 캐시를 표준 포맷의 sparse 파일로**(재고에서 섹션 상한을 예약해
  방문 셀만 채움): 페이지 수 상한이 느슨해 디렉터리가 수백 GB 주소 공간이 되고
  결정성·검증 규칙이 깨진다. 기각.
- **D. KLayout식 지연 로드(파일을 열 때 셀 표만, 셀은 필요할 때)**: 셀 단위
  = §4의 한계. 세그먼트가 그 아래 단계다.
- **E. 요약만 만들고 근접 뷰는 포기**(캡처 전용): 덱 캡처가 요약 구간에만 있다면
  L0~L2 + ovo로 끝난다(세그먼트 없이도 ovo는 재고 + 요약 패스로 만들 수 있다).
  뷰어 근접 뷰가 요구이므로 세그먼트까지 간다 — 단, **E를 1차 납품으로 자르는
  선택지**는 있다(§10).

## 10. 열린 질문 (사용자 결정 — 2026-10-21 "제안대로 L1 진행해줘"로 1~5는 제안대로 결정)

1. 1차 범위: (i) 캡처 우선 — 경량 생성(inv + ovo)으로 광역 캡처부터(세그먼트
   없이, L0·L1·요약 identity 변경만), (ii) 뷰어 근접 뷰까지(L4). 제안: L0로 잰 뒤
   (ii)로 바로 간다 — (i)는 (ii)의 앞부분이다. L0 결과(§8.1): 띠·체인 0이라 (ii)의
   근접 뷰 첫 방문은 수십 ms, L1(셀 단위)만으로도 수 s다. **결정: (ii), L1부터.**
2. 자동 색인 기본(§5.6): 덱 소스 = 경량, 레이아웃 = 전체? 크기 문턱? **결정: 덱 소스
   경량, 단독 레이아웃 전체, 플래그로 뒤집음(크기 문턱 없음).**
3. cut 뷰의 픽셀 계약(§5.8): 허용 오차로 가는가, B안으로 동일성을 사는가. **결정:
   허용 오차로 시작; exact/archival은 동일 게이트; B안은 필요할 때.**
4. 요약 없는 동안의 광역 표시(§5.5): 배너(제안) / 임시 bbox 채움 / 대기. **결정: 배너.**
5. 덱 캡처의 최소 배율(µm/px): 기준 셀 1 µm(`--occupancy-um 1`)로 요약 구간을
   넓힐지. **결정: 4 µm 유지, 바꾸기 전에 측정.**
6. 현장 50 GB 요약 패스의 `budget`/`pass(es)`/`charged` 줄(2026-10-20 요청) —
   **답 2026-10-21**: `--mem 0.5`, 1패스, 상주 예산 415M ≈ 2셀 = 예산 병목(§6.1).
   남은 것은 기본 예산 재실행의 시간.
