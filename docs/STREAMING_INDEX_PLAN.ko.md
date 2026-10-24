# 스트리밍 인덱서 설계 — 셀 단위 2-패스 빌드 (2026-10-19)

상태: S1(1차 재고 패스) 구현됨(0.12.363, §3.6), S2(CellStore) 구현됨
(0.12.365, §4.2), S3(셀 파서) 구현됨(0.12.366, §4.1), S4(드라이버,
`--stream`) 구현됨(0.12.367, §4.3 구현), S5(스트리밍 design.ovo) 구현됨
(0.12.368, §4.6 구현), S6(가드) 구현됨(0.12.369, §5 구현), 전환(스트리밍
기본) 구현됨(0.12.370, §11). S7 후속은 설계. 근거는
rust/FLOE_INDEX_MEMORY_PLAN.md §5의 현장 수치와 `floe-index scan`
0.12.328의 결과. 코드 줄 번호 참조는 0.12.360(8b59f3a5) 기준.

## 0. 결론

- 파싱 결과(Doc)를 통째로 들고 있는 지금 구조에서는 50 GB급 덱
  소스를 어떤 서버에서도 인덱싱할 수 없다. 현장 파일의 Doc은 927 GiB
  (번들 할당자 기준 1.05 TiB)에 원본 49 GiB가 더해지고, 서버는 376 GiB다.
  표현을 2.3배 줄여도 400 GiB 위다.
- 그러나 플랜·커밋·인코딩은 이미 셀 단위다. 셀 플래너는 자기 셀의
  레코드와 레이어 순서, 자식 셀의 재귀 bbox만 읽고(vfs.rs 5476·5491·5761),
  인코더는 자기 셀의 레코드만 인덱스로 참조하며(3892), 커미터는 파일
  순서로 쓴다. Doc 전체가 필요한 것은 빌드 앞의 요약 전처리와 덱 소스의
  점유 격자(design.ovo)뿐이다.
- 따라서 **2-패스**로 간다. 1차 패스(재고)는 scan 확장 위에 요약을 얹어
  도형을 버리면서 전처리를 끝내고, 2차 패스(빌드)는 파일 순서로 셀을
  파싱해 지금의 플래너·커미터·인코더에 넘기고 인코딩이 끝나면 버린다.
  상주 메모리는 창 안의 셀들(예상 바이트 합 ≤ 예산)과 메타데이터
  Builder, 1차 패스 표뿐이다.
- 셀 인덱스(CELL 레코드 순서), 정규화(셀 단위), 커밋 순서(파일 순서)가
  지금과 같으므로 **출력 바이트를 지금의 인메모리 빌드와 동일하게** 맞출
  수 있다. 그 동일성이 전환 게이트다.
- 현장 파일 기준 상주 8~20 GiB, 시간은 지금의 빌드가 돌 수 있었다면
  걸렸을 시간의 1.15~1.3배로 본다(§7).

## 1. 배경 수치 (현장 50 GB 덱 소스, 이름은 기록하지 않음)

| 항목 | 값 |
| --- | --- |
| 레코드 | 19.7억 (POLYGON 16.8억 = 85%, RECTANGLE 2.9억, 사다리꼴 9) |
| 셀 | 5,461개, 각각 한 번씩 배치 (PLACEMENT 5,460) |
| CBLOCK | 781,767개, 평균 해제 121 KB |
| 폴리곤당 정점 | 28.9 (points 485억) → 정점 힙 723 GiB |
| 점 목록 반복 | 2.38억개, 멤버 25.1억 → 41 GiB |
| doc_bytes_est | 927 GiB (pow2 1,070 GiB) + 원본 49 GiB |
| 가장 큰 셀 | 118만 레코드, 415 MiB (평균 셀의 3.3배) |
| 사망 | 12스레드 파싱 160 s에 361 GiB, 이후 스왑, 1,425 s에 OOM kill |

셀 하나가 415 MiB면 12스레드가 동시에 셀을 잡아도 5 GiB 안쪽이다.
평면 파일(셀 하나가 파일 전체)에 필요한 셀 내부 공간 분할은 이 파일에
필요 없고, 이 설계의 범위 밖이다(§8 S7).

## 2. 지금 파이프라인에서 Doc 전체를 읽는 곳

`floe-index vfs`(vfs.rs 54~861)와 `build`(6277~7077)를 기준으로
분류한다. (A) 요약으로 대체 가능, (B) 이미 셀 단위, (C) 재설계,
(D) 스트리밍 모드에서 보류.

| 단계 | 코드 | 읽는 것 | 분류 |
| --- | --- | --- | --- |
| 원본 읽기 + 구문 파싱 | vfs.rs 552·586, doc.rs 1680~1824 | 파일 전체 → Doc. finish_inner가 CELLNAME·TEXTSTRING 참조, 배치 대상 인덱스, top, 레이어 이름을 해결 | A (1차 패스 표 + 셀 파서) |
| 반복 정규화 | doc.rs 1511~1590 | 셀 자신(8셀 묶음은 작업 단위일 뿐, 결과는 셀별) | B |
| 재귀 bbox | tiler/hier.rs 39~126 | 자기 레코드(텍스트 포함) + 자식 rbb를 배치 변환 | A (§3.3) |
| 레이어 인덱스 | 6306~6312 | layer_order 전체 | A |
| 직접 요약 | 6314~6467 | 셀 자신: dmask, dtmask, dmembers, dbox, 레이어별 합 | A |
| topo·rank·fold | 4434~4468, 6469~6511 | 배치 그래프 전체 + 자식 요약 | A (§3.3) |
| 셀 플랜 | 5462~6015 | 자기 rects/polys/paths/places, 자식 rbb, lidx/nl, layer_order | B |
| 순서 커밋 + 텍스트 | 6787~6934, 6064~6273 | 플랜 + 자기 텍스트 + 요약 행 | B |
| 페이지 인코딩 | 4275~4389, 3891~3949 | 자기 셀 레코드(인덱스), 아레나, unit | B (셀을 인코딩까지 상주) |
| 레이어 표·BVH 마스크·finish | 7005~7014, 7060, 7068 | Builder 누적값만 | B |
| design.ovh·frontier·design.ovs | 2251, 2842, 2397 | 커밋된 OVM/파일만 | B |
| meta.json | 2964~3087 | rbb[top], 레이어 표, 텍스트 합계(커밋 중 누적 가능) | A |
| design.ovo (덱 기본 on) | 2036~2121, vfs/occupancy.rs 2137~2307 | Doc 전체: top-down 전개로 월드 비트맵 | C (§4.6) |
| design.ovc, design.ovr (opt-in) | 2814, 1973 | Doc 전체 | D |

이미 유리한 점: 커밋 순서 = CELL 레코드 순서(doc.rs 621~629·727; 미정의
배치 대상은 finish에서 뒤에 덧붙임), 정규화·플랜·인코딩·텍스트·ovb는
셀 단위, ovh·frontier·ovs는 커밋 파일만 읽는다.

## 3. 1차 패스: 재고 (inventory)

### 3.1 입력과 틀

파일 전체를 한 번 읽는다. CBLOCK은 해제한다(레이어 순서와 bbox가 그
안의 레코드에 있다). `scan_parallel`의 틀을 그대로 쓴다: `cell_cuts`
스킴으로 CELL 경계를 찾고 스레드별로 연속 셀 묶음을 처리한 뒤 파일
순서로 합친다. 2026-10-19의 scan 확장(lib.rs ScanStats, CellStat)이
이미 이 틀 위에 셀별 통계를 모은다. 거기에 좌표를 더한다.

### 3.2 셀별 산출

| 값 | 쓰는 곳 |
| --- | --- |
| 이름 또는 참조번호, 파일 순서 인덱스 | 셀 인덱스 = 지금의 Doc 인덱스 |
| 바이트 범위 [a, b)와 그 앞의 암시적 CELLNAME·TEXTSTRING 개수 | 2차 패스의 셀 파서 시드(지금 `cell_cuts`가 주는 것) |
| 레코드 수, doc_bytes_est | 입장 예산(§4.3), 사전 거절(§5) |
| 자기 bbox 두 가지(텍스트 포함 = build용, 제외 = ovo/ovc용) | dbox, rbb |
| 레이어 마스크, 텍스트 마스크, 멤버 수, 레이어별 레코드·멤버 합 | dmask, dtmask, dmembers, lrecs, lmems |
| 자식 간선: 자식(첫 등장 순서), 간선별 멤버 합 Σ members(place) | topo, rank, height, rmembers |
| 간선별·방향별(rot 4 × flip 2) 오프셋 박스 | rbb (§3.3) |
| 자식별 배치 레코드 원본(임계 이하일 때만, §4.6) | design.ovo 인스턴스 변환 |

메모리는 O(셀 + 간선 × 8)이다. 배치가 6.4억인 MAIN01급도 간선 수만큼만
든다. 셀 이름은 CELLNAME 표를 합친 뒤 해결한다(scan 확장의 `finish`와
같다).

### 3.3 fold: Doc 없이 지금과 같은 값을 내는 근거

- **topo·rank·height·rmembers·rmask·rtmask**는 지금도 셀별 요약과
  배치 그래프만 쓴다(6469~6511). 그래프의 DFS는 배치 레코드 순서로
  자식을 따라가므로(4434~4468) 1차 패스가 자식을 **첫 등장 순서**로
  기록하면 rank가 같다.
- **재귀 bbox**는 지금 배치 하나마다 자식 박스를 변환해 합친다
  (tiler/hier.rs 94~117): `Xf::place(x, y, rot, flip)`로 자식 rbb의
  두 모서리를 옮기고 반복 extent를 더한다. 같은 자식·같은 방향(rot,
  flip)의 배치들은 변환된 박스 R(B)가 같고 오프셋만 다르다. 축 정렬
  박스에서는 ∪ₚ(R(B) + oₚ)의 bbox = R(B) ⊕ bbox{oₚ}이므로, 간선·방향별로
  오프셋(반복 extent를 더한 박스)의 합집합 박스 하나만 두면 자식 rbb가
  나중에 정해져도 결과가 **정확히 같다**. 빈 자식(rbb None)은 기여하지
  않는다. 텍스트 포함·제외 두 변종을 같은 간선 집계로 만든다.
- 검증: 기존 Doc 기반 전처리(cell_bboxes_full, topo_order, fold)의
  결과와 1차 패스 결과가 셀마다 같아야 한다(§6 G1).

### 3.4 전역 산출

CELLNAME·TEXTSTRING·PROPNAME·LAYERNAME 표(파일 뒤에 올 수 있다),
layer_order(첫 등장 순서, CBLOCK 안 포함, LAYERNAME exact pair 포함 —
doc.rs 118~122·631~635·703~707), 레이어 이름·별칭, unit, top(배치되지
않은 유일 셀, doc.rs 1801~1812), 셀 수 n, 레이어 수 nl(비트셋 폭).

### 3.5 비용

scan과 같은 자릿수다. 현장 파일의 12스레드 구문 파싱이 초당 2.2 GiB의
Doc을 만들었으니 레코드를 저장하지 않는 1차 패스는 5~7분으로 본다.
실측은 현장 scan의 `scan_s`로 확인한다. 결과는 `.ice/inventory.bin`
같은 사이드카로 남겨 덱 사전 점검(§5)과 재시작에 쓸 수 있다(열린
질문 §10).

### 3.6 구현 (S1, 0.12.363 / floe-index 0.12.329)

- `floe_oasis::inventory`(rust/oasis/src/inventory.rs). scan의
  `record()`가 doc.rs와 같은 좌표 모달(XY 모드, geometry·placement·text
  각각의 x·y, W/H, 점 목록, 경로 확장, 배치 대상, CTRAPEZOID가 모달에
  되돌려 쓰는 암시 치수)을 따라가고, 수집기 `Part`가 CELL 레코드마다
  바이트 범위와 암시 이름 시드, 두 가지 자기 bbox, 레이어·텍스트
  마스크, 멤버 수, 자식별 간선(첫 등장 순서; 배치 수, 멤버 합, 방향
  8가지별 오프셋 박스)을, 전역으로 레이어 첫 등장 순서와 LAYERNAME
  규칙을 모은다. 레코드는 하나도 남기지 않는다. `finish`가 이름을
  해결하고(CELL 레코드 순서, 참조번호는 표로, 미정의 배치 대상은 뒤에
  첫 참조 순서로 — doc.rs finish_inner와 같다), top을 정하고, 파트의
  레이어 순서를 파일 순서로 합쳐 마스크를 재배치한 뒤 fold(topo·rank·
  height·rmembers·rmask·rtmask·rbb 두 변종)를 만든다. 진입점은
  `floe_oasis::inventory(data, jobs)`; 병렬 분할은 `cell_cuts`로 Doc
  파스와 같은 묶음을 쓴다.
- `floe_oasis::geom`(rust/oasis/src/geom.rs). tiler의 `rep_extent`·
  `path_outline`·`path_bbox`를 옮겨 왔고(tiler는 재export, 호출부
  불변) 방향 박스 변환 `orient_box`(Xf::place의 선형부)를 더했다. 재고와
  빌드가 같은 함수를 쓰므로 박스가 바이트 단위로 같다.
- `floe-index`(rust/cli/src/prepass.rs). build()의 사전 패스(재귀 bbox,
  직접 마스크·멤버·bbox와 레이어별 합, topo fold)를 `PrePass::from_doc`
  으로 떼어냈고(동작 불변, 출력 바이트 동일), `PrePass::from_inventory`
  가 같은 값을 재고에서 만든다. `check`가 G1: 셀 표와 순서, top, unit,
  레이어 순서·이름·별칭, 셀별 dbox 두 변종·dmask·dtmask·dmembers, 간선
  (자식 순서·멤버 합·오프셋 박스·배치 수), rbb 두 변종(tiler
  `cell_bboxes_full`·`cell_bboxes` 그대로), topo·rank·height·rmembers·
  rmask·rtmask, 레이어별 레코드·멤버 합. 명령은
  `floe-index inventory FILE [--jobs N] [--check] [--cells]`
  (JSON 요약 + stderr 한 줄; `--check`는 불일치 목록과 종료 코드 1).
- 게이트. 단위 테스트: 손으로 쓴 적대적 스트림(표 앞/뒤 × CBLOCK
  유무, 참조·이름 혼용 셀, 미정의 대상, 반복 12종, XYRELATIVE, 사다리꼴
  퇴화, CTRAPEZOID 암시 치수, 반지름 0 원, 텍스트만 있는 셀)을 jobs
  1·2·3으로 Doc과 대조(floe_oasis 안에서는 레코드 기반 참조값, cli에서는
  실제 `PrePass::from_doc`·tiler 함수), write_tree 계층 하나. 배터리:
  `rust_inventory`(tools/validate_rust_inventory.py) = valmini, KLayout이
  CBLOCK 없이/strict 모드로 다시 쓴 valmini, route_chip 0.05, main01_like
  0.001을 jobs 1·3으로 `--check`.
- 알려진 차이(유효하지 않거나 특이한 스트림에서만). (1) 같은 셀을 CELL
  레코드 둘이 정의하고 하나가 참조번호인데 CELLNAME 표가 뒤에 오면
  재고는 병렬 Doc 파스처럼 한 셀로 합치고 순차 파스(jobs 1)는 둘로
  둔다. (2) rmembers의 포화 연산이 배치 단위가 아니라 간선 단위다(곱이
  u64를 넘지 않는 한 같다). (3) CBLOCK 안의 CELL 레코드는 자기 바이트
  범위가 없고(`cblock_defs`로 센다) 둘러싼 보이는 셀의 범위에 속한다;
  암시 이름 시드는 cell_cuts처럼 CBLOCK 밖의 레코드만 센다. (4) 참조번호가
  중복된 CELLNAME은 첫 항목이 이긴다(병렬 파스와 같고, 순차 파스는
  마지막).
- 비용은 §3.5의 추정대로 scan과 같다(macOS, jobs 4; Doc 열은 `--check`가
  같은 프로세스에서 잰 파싱 + 정규화 + 사전 패스 시간).

| 파일 | scan | inventory | Doc 파스 + 사전 패스 |
| --- | --- | --- | --- |
| 합성 라우팅 칩 281 MB, 2,760만 레코드, 29셀 | 12.1 s | 12.0 s | 19.0 s |
| 합성 평면 CTRAPEZOID 1,600만 (반복 있음) | — | 8.4 s | 11.5 s |
| main01_like 0.001 (17.6 MB, 3,521셀, 449 레이어, 깊이 16) | — | 0.37 s | 1.24 s |

  현장 50 GB 파일의 1차 패스는 scan의 실측(`scan_s`)과 같은 시간으로
  본다.

## 4. 2차 패스: 빌드

### 4.1 리더와 셀 파서

- 리더는 파일 순서로 셀 범위를 `pread`한다. 범위 단위는 `cell_cuts`
  단위(보통 셀 하나; CBLOCK 안의 CELL 레코드는 스킴이 보지 못하므로
  이전 셀과 한 단위로 묶인다 — 파서는 단위에서 셀이 여럿 나오는 것을
  허용한다). 12개 워커가 다음 범위를 차례로 잡으므로 읽기는 앞으로만
  움직이는 창 안의 순차 접근이다. NFS에 mmap이 필요 없다. 버퍼는 셀의
  압축 바이트(현장 평균 9 MB)와 CBLOCK 하나의 해제 버퍼다.
- 셀 파서는 지금의 청크 파서를 재사용한다: `parse_records` + 청크
  Builder에 암시적 참조번호 베이스를 시드(doc.rs 1703~1740이 하는 것)
  → 1차 패스 표로 셀 이름·텍스트 문자열·배치 대상 인덱스를 해결
  (finish_inner 1761~1800의 셀별 부분) → `Cell`. 정규화는 그 셀에만
  건다. 지금의 8셀 묶음은 작업 단위일 뿐 결과는 셀별이므로 동일하다.
- 단위 테스트: 전체 파싱 Doc의 cells[ci]와 셀 파서의 결과가 레코드
  단위로 같다(§6 G2).

**구현(S3, 0.12.366 / floe-index 0.12.331).** `floe_oasis::doc::
parse_cell_range(data, &CellRange, &CellTables) -> Vec<CellPiece>`:
범위 하나를 `parse_records`(청크 빌더, 범위의 암시 참조번호 시드)로
읽고, 셀 이름은 재고의 CELLNAME 표로(자리표시자 포함), 배치 대상은
재고의 셀 표로 Doc 인덱스로, 미해결 텍스트는 범위 안의 표 또는 재고의
TEXTSTRING 표로 해결한다. 범위는 보통 셀 하나지만 CBLOCK 안의 CELL이
더 나올 수 있고(`CellPiece.index`로 구분), 셀 하나가 CELL 레코드 여럿
이면 조각이 여러 범위에서 나오므로 조각을 파일 순서로 이어 붙인 뒤
`normalize_cells(&mut [cell], jobs)`를 건다 — 전체 패스의 8셀 묶음
본체를 그대로 떼어낸 함수라 결과가 레코드 단위로 같다. 재고는 이를
위해 `head`(첫 보이는 CELL 앞 구간; 표와, CBLOCK이 거기 정의한 셀),
`cellnames`, `textstrings`, `cell_tables()`를 더 들고 있다. G2는
`floe-index inventory FILE --check-cells`(전체 Doc 파스의 cells[ci]와
레코드 단위 비교; `rust_inventory` 게이트가 모든 fixture에 jobs 1·3으로
돈다)와 단위 테스트(적대적 스트림 4변종, 헤드 CBLOCK 셀, 정규화가
쪼개는 반복을 가진 write_tree 계층)다.

검토(S3, 2026-10-20) 반영: (1) 미해결 텍스트는 재고의 TEXTSTRING 표를
먼저 본다(전체 Doc 파스가 보류 텍스트를 해결하는 표가 그것이다);
(2) 참조번호가 두 번 정의된 CELLNAME·TEXTSTRING은 재고가 거절한다 —
Doc은 그 참조를 레코드 위치에 따라(정의가 앞서면 그 자리에서, 아니면
표에서) 해결하므로 범위 단위 파서가 재현할 수 없고, 실제 라이터는 그런
스트림을 내지 않는다; (3) `InvCell.def_ranges` = 그 셀의 CELL 레코드를
담은 범위(헤드 포함)의 시작 오프셋들 — 범위가 셀 하나의 CELL 레코드
둘을 접으면 조각은 하나이므로 "조각 수 = 정의 수"는 성립하지 않고,
S4의 완료 판정은 def_ranges의 범위가 모두 파싱됐는지로 한다; (4)
`parse_cell_bytes(bytes, base, seeds, tables)` — pread 리더가 범위의
바이트만 들고 부른다; (5) `CellStore::failed()` — 리더 스레드의 파싱
오류를 커미터가 대기 중에 보고 빌드를 중단한다(Doc에서는 None).

**S3 셀 파서의 계약(S2 검토에서 정리).** 변환된 함수들이 셀 밖에서
읽는 것은 자식의 rbb(PrePass)뿐이고, 셀 안에서는 (1) `PlaceRec.cell`이
재고의 Doc 인덱스와 같아야 하며(플래너의 `rbb[pl.cell]`, sink의
place), (2) 모든 rect/poly/path/text의 (layer, dt)가 재고의 layer_order에
있어야 하고(lidx 조회는 panic), (3) 셀 단위 정규화가 전체 Doc 패스와
레코드 단위로 같아야 한다(페이지의 PRec.idx가 레코드 인덱스다).
G2가 이 셋을 본다.

### 4.2 플랜·커밋·인코딩: 그대로

- `build_cell_plan`, `build_cell_texts`, `encode_write_pages`,
  `annotate_bvh_masks`, `Builder::finish`는 바꾸지 않는다. `&Doc`을
  **CellStore**로 바꾼다: `cell(ci) -> &Cell`(상주 셀만), `layer_order`,
  `unit`. 인메모리 경로는 Doc을 감싼 store로 같은 코드를 돈다. 이
  리팩터만으로는 바이트가 바뀌지 않는다(§6 G0).
- 커미터는 지금처럼 ci 순서(= 파일 순서)로 커밋한다. 자식 rbb가 1차
  패스에서 나오므로 부모가 자식보다 먼저 와도 플랜할 수 있다.
- 인코딩 청크가 design.ovp에 쓰인 뒤 그 청크의 셀을 **해제**한다.
  지금 아레나와 PtsPrepared가 떨어지는 지점(6958~6961)과 같다.

**구현(S2, 0.12.365 / floe-index 0.12.330).** rust/cli/src/cellstore.rs의
`trait CellStore: Sync { n_cells, unit, top, layer_order, layer_names,
layer_aliases, cell_name(ci), cell(ci) -> CellRef }`와 `impl CellStore
for Doc`. `CellRef`는 상주 Doc에서 빌린 `&Cell` 또는 (S4의 스트리밍
저장소가 줄) `Arc<Cell>`이며 Deref로 같은 코드가 돈다. `build()`는
`(&dyn CellStore, PrePass, ...)`를 받고(사전 패스는 호출자가 만든다:
인메모리는 `PrePass::from_doc`, 스트리밍은 `from_inventory`),
`build_cell_plan`·`gen_lod_job`·`build_cell_texts`·`encode_write_pages`·
`encode_job`·커미터·레이어 표·`emit_viewer_side`가 `&Doc` 대신 저장소를
읽는다. meta.json의 레이어별 stored_shapes에 들어가던 텍스트 멤버 합은
Doc 전체를 다시 걷지 않고 `build_cell_texts`가 셀을 커밋하면서 레이어별로
누적한다(`TextIndexStats::layer_text_members`; 값은 같다). 그대로 `&Doc`을
받는 것은 전체 Doc이 필요한 보류·후처리 경로뿐이다: design.ovo(§4.6에서
S5), design.ovc/ovr(§4.7 보류), `vfs --profile-*`. G0: S2 직전 바이너리
(0.12.329)와 S2 바이너리가 같은 소스(valmini, route_chip 0.05, main01_like
0.001, 평면 CTRAPEZOID 400만 반복 유/무)를 `--jobs 1`·`4`, `--coverage
--occupancy`로 빌드한 캐시의 모든 파일(design.ovm·ovp·ovt·ovb·ovo·ovc·
ovh·ovs, meta.json)이 바이트 동일함을 확인했다: 10회 빌드 × 9파일 모두
`cmp` 일치(2026-10-19, macOS). 단위 테스트 35개(플래너·인코더·텍스트
인덱스의 보존 오라클 포함)도 그대로 통과한다.

### 4.3 입장 예산

지금의 창은 셀 **개수**(`plan_batch`, 바닥 jobs)다. 1차 패스가 셀마다
doc_bytes_est를 알므로 **바이트**로 입장시킨다: 상주 셀(파싱 중 + 플랜
중 + 커밋됐으나 미인코딩)의 예상 바이트 합이 예산을 넘지 않을 때만
다음 셀을 읽는다. 예산은 `--mem GiB`, 기본은 시작 시 MemAvailable의
절반과 32 GiB 중 작은 값. 개수 창과 MemAvailable 거버너(6793~6807)는
그대로 둔다. 셀 하나가 예산보다 크면 §5의 거절이다.

**교착 방지(S2 검토, 2026-10-19).** 커미터의 플러시 조건은 개수 창
(`ci + 1 >= chunk_base + window`)뿐이라, 바이트 예산이 커밋됐으나
미인코딩인 셀들에 다 쓰인 채 청크가 덜 찼으면 리더는 다음 셀을 읽지
않고 커미터는 그 셀의 플랜을 기다려 멈춘다. 그래서 저장소가
`flush_wanted()`로 역압을 주고 커미터는 이를 플러시 조건에 OR한다
(S2에서 훅을 넣었다; Doc 저장소는 항상 false). 인코딩이 끝난 청크는
`release(chunk_base..ci+1)`로 저장소에 돌려준다 — 아레나가 떨어지는
바로 그 지점이다.

**구현(S4, 0.12.367 / floe-index 0.12.332).** `floe-index vfs SRC OUT
--stream [--mem GiB]`(rust/cli/src/vfs.rs `vfs_stream`, rust/cli/src/
stream.rs `StreamStore`), `floe2 index SRC --stream [--mem GiB]`가
그대로 넘긴다. 1차 패스는 파일을 읽기 전용으로 mmap해 한 번 훑는
`floe_oasis::inventory`(매핑은 2차 패스 전에 내려놓는다; NFS에서 순차
한 번의 스윕이라 §4.1의 pread 선호와 충돌하지 않는다). 2차 패스의
`StreamStore`는 재고와 파일 핸들을 들고, `jobs`개의 리더가 범위(헤드,
그리고 셀 범위들)를 파일 순서로 잡아 `read_exact_at`으로 읽고
`parse_cell_bytes`로 파싱해 조각을 모으며, `def_ranges`의 범위가 모두
파싱된 셀을 조각 순서로 이어 붙여 `normalize_cells`를 건 뒤
`Arc<Cell>`로 둔다. 입장은 바이트 예산: 범위를 처음 정의하는 셀들의
doc_bytes_est를 그 범위에 물리고, 상주(물렸으나 아직 `release`되지
않은) 합 + 범위 비용 ≤ 예산일 때만 다음 범위를 잡는다(빈 저장소에는
항상 하나 들어간다; 가장 큰 셀이 예산보다 크면 출력에 손대기 전에
거절, 종료 1). 커미터는 청크 인코딩 뒤 `release(chunk_base..end)`로
셀을 돌려주고, 플랜을 기다리는 동안 `flush_wanted()`(리더가 예산에
막힘)이면 커밋된 청크를 즉시 인코딩해 되돌려 준다(교착 방지, 바이트
무영향 — 페이지는 청크 경계와 무관하게 페이지 순서로 쓰인다). 리더의
오류는 `failed()`로 커미터에, 대기 중인 플래너에는 panic으로 전해져
빌드가 멈추지 않고 중단된다. 예산 기본값 = min(MemAvailable/2, 32 GiB)
(MemAvailable을 모르면 MemTotal의 절반, 그것도 모르면 32 GiB — 로그가
어느 쪽인지 말한다). 후처리는 같다(ovh, frontier, meta.json, ovs);
design.ovo는 S5까지 건너뛰며 그렇다고 알린다(덱 소스의 기본값이라 안내
한 줄; `floe2 index --stream`은 ovo 없는 현재 캐시를 다시 쓰지, 전체
파스의 occupancy-only로 메우지 않는다); `--coverage`·`--representatives`·
프로파일·`--*-only`는 `--stream`과 함께 쓰면 종료 2.

검토(S4, 2026-10-20) 반영: (1) 빌드 안의 panic이 스트리밍 경로를 멈춰
세우던 두 구멍 — 하트비트 스코프의 정지 플래그를 드롭 가드로, 플래너·
커미터의 실패를 `CellStore::abort`로 저장소에 알려 `cell()`에서 자는
플래너를 깨운다; 리더의 소스 오류는 한 줄과 종료 1로 끝난다(인메모리
파스의 오류와 같은 모양; 마커가 먼저 지워져 캐시는 "없음"으로 읽힌다).
(2) 출력 디렉터리를 스트리밍 경로도 만든다. (3) 즉시 플러시 조건은
"커밋됐으나 미반환 셀이 있음"(페이지 유무와 무관: 텍스트만 있는 셀도
예산을 쥔다). (4) 여러 범위에 걸쳐 정의된 셀은 첫 정의와 마지막 정의
사이에 처음 정의되는 셀들이 함께 상주해야 하므로 그 스팬이 예산을 넘으면
사전 거절한다(가장 큰 셀만 보던 검사의 구멍). (5) 빈 자식(배치만 있는
셀)의 인스턴스 BVH 주석이 디버그에서 오버플로하던 것을 0으로 — 릴리스
바이트는 그런 배치가 있는 파일에서만 1/1에서 0/0으로 바뀐다. (6) 예산은
정규화 전 추정치라 정규화 중인 셀 하나만큼 넘을 수 있다(§4.4). 게이트 `vfs_stream`
(tools/validate_vfs_stream.py): valmini·route_chip 0.05·main01_like
0.001에서 인메모리와 `--stream`(jobs 1·4, 예산 = 가장 큰 셀 × 1.5)의
design.ovm·ovp·ovt·ovb·ovh·ovs·meta.json 바이트 동일(G3), `floe2 index
--stream`과 거절들(G4), 스트리밍 피크 RSS ≤ 인메모리 피크 + 여유(G5,
수치 출력); 단위 테스트 `streaming_build_matches_in_memory`는 적대적
스트림들을 예산 = 가장 큰 셀(모든 입장이 반환을 기다려 즉시 플러시
경로가 돈다)과 무제한으로 jobs 1·2·4에서 비교한다.

실측(macOS, jobs 4, 2026-10-20). (a) 합성 라우팅 칩 281 MB(Doc 2.7 GB,
top 셀 하나가 2.3 GB): `--stream --mem 3`이 바이트 동일, 13 s로 같고,
피크 RSS는 스트리밍 12.4 GB 대 인메모리 11.0 GB — 이 파일은 거대 셀 하나의
플랜 스크래치가 피크를 정하므로 Doc 상주를 줄여도 피크가 내려가지 않는다.
(b) main01_like 0.01(110 MB, 11,127셀, Doc 1.1 GB, 가장 큰 셀 138 MB,
`--no-ovs`): `--stream --mem 0.3`이 바이트 동일, 8.2 s 대 7.7 s, 피크
2.4 GB 대 3.9 GB. 스트리밍이 묶는 것은 Doc의 상주이지 플래너의 스크래치나
후처리가 아니며, 현장 50 GB 파일(Doc 927 GiB, 가장 큰 셀 415 MiB)은
(b)쪽 프로필이다.

### 4.4 상주 메모리

상주 = 창 안 셀들의 Doc 바이트 + 플랜 스크래치(아레나·샤드, 지금과 같음)
+ 인코딩 중 페이로드(encode_batch) + Builder 메타데이터 + 1차 패스 표
+ 리더 버퍼. 현장 파일: 창 48(12×4)에 평균 174 MiB, 최대 415 MiB →
8~20 GiB. 메타데이터는 페이지 수(약 100만)에 비례해 수십 MB다.
정정(S2 검토): 플러시 시점에는 **인코딩 중인 청크(창 하나)와 그 뒤의
플랜 창**이 함께 상주하므로 셀 수로는 최대 2×창(96)이고, 인코더가
레코드를 인덱스로 다시 읽기 때문에 커밋 시점에 셀을 버릴 수 없다.
바이트 입장(§4.3)은 이 2×창을 예산 안에 넣는 기준이어야 하며,
현장 파일 기준 16 GiB(평균)~39 GiB(큰 셀이 몰릴 때)다 — §10의 예산
기본값 질문에 이 수치를 쓴다.

### 4.5 후처리

레이어 표, BVH 마스크, OVM finish, design.ovh, frontier, meta.json,
design.ovm 커밋, design.ovs(덱 제외)는 변경 없음. meta.json의 텍스트
합계는 커밋 중 누적한다(지금 2990~2994가 Doc을 다시 훑는 부분).

### 4.6 design.ovo (덱 소스 기본 on)

지금은 Doc 전체를 top-down으로 전개해 레이어별·깊이별 월드 비트맵을
만든다(vfs/occupancy.rs 1272~1353). 비트맵은 OR 누적이라 **순서가 바이트를
바꾸지 않는다**. 스트리밍에서는 셀이 상주해 있는 동안 그 셀의 모든
인스턴스 변환(top에서 그 셀까지의 배치 경로를 전개한 것)으로 같은
비트맵에 즉시 래스터화한다. 인스턴스 변환은 1차 패스가 보관한 배치
레코드로 만든다. 작은 셀을 통째로 칠하는 규칙(2209~2212)은 rbb만
쓰므로 그대로다. 결과 바이트는 지금과 같아야 한다(§6 G3).

배치 레코드는 0.12.397까지 메모리에 보관했고(전체 빌드 5,000만 상한, 경량
캐시의 요약 패스는 예산의 4분의 1 ÷ 96 B; 넘으면 ovo 없이 완성 — DIRECT_READ_PLAN
§8.8), **0.12.398부터 배치 저장소**(`floe_vfs::places`)에 둔다: 메모리 또는 캐시
디렉터리의 임시 파일, 상한도 건너뜀도 없다(아래 0.12.398 항목, DIRECT_READ_PLAN §8.21).

**구현(S5, 0.12.368 / floe-index 0.12.333).** 바이트 동일성의 근거:
비트맵은 OR, charge는 연산 단위라 순서는 무관하고, top-down 걷기가
수행하는 **연산의 집합**만 같으면 된다. 그 집합은 (1) 셀의 인스턴스
= top에서 그 셀까지, 가지치기된(작은) 셀을 지나지 않고 도형이 아래에
하나도 없는 자식으로 내려가지 않는 경로들(가지치기 가능한 격자 배치의
멤버는 방문하지 않음), (2) 인스턴스마다: 작은 셀은 서브트리 bbox를
레이어별 깊이 마스크의 깊이에 마킹, 그 외는 자기 레코드 마킹 + 배치의
멤버 charge(작은 자식의 가지치기 가능한 격자는 charge 1 + 발자국 채움)
— 자식의 마킹은 자식의 인스턴스에서. 이것은 레이어와 무관한 인스턴스
집합이므로 셀을 한 번 상주시켜 모든 레이어를 함께 마킹할 수 있다.
구현은 세 조각이다. 재고(S1)에 도형 전용 레이어 마스크
(`InvCell.dsmask`, `Inventory.rsmask`)를 더해 걷기의 `has`와 레이어별
깊이 마스크를 Doc 없이 만든다. 스트리밍 저장소(S4)는 메인 빌드 중
커미터가 셀을 돌려줄 때 그 셀의 배치 레코드를 보관한다(`keep_places`,
상한 5,000만 레코드; Pts 멤버 목록은 Arc 공유). 빌드 뒤 `floe_vfs::
occupancy::StreamOcc`가 격자·레이어별 마스크를 만들고
`Instances`가 보관된 배치에서 셀의 인스턴스를 부모의 인스턴스를 거슬러
그때그때 열거하고(미리 전개하지 않는다 — valmini의 2.25 M 격자 멤버만
전개하면 270 MB; 인스턴스가 적은 셀만 합계 256 K개(14 MB)까지 실체화),
저장소를 `reset`해 파일을 다시 스트리밍하면서 `jobs`개의 마커가 상주
셀을 그 인스턴스마다 모든 레이어의 공유 평면에 마킹한다(`mark_cell`:
기존 `Marker`의 도형 프리미티브를 레이어별 마커로 끼워 쓴다). 패스의
메모리는 스트리밍 예산(`--mem`)의 1/4씩이다: 레이어 평면이 그것을
넘으면 레이어 묶음별로 패스를 반복하고(게이트처럼 예산이 작으면
레이어마다 한 패스), 상주 셀의 창도 1/4(최소 가장 큰 셀/스팬 — 마커는
한 번에 한 셀만 필요하므로 리더가 그만큼만 앞서 읽는다)이다. 메모리는
배치 레코드·평면·창의 것이고 걷기의 방문 수와는 무관하다; 시간은
인메모리 걷기의 방문 수에 조상 열거의 반복(깊은 사슬에서 깊이 배)을
더한 것이다. 실측(route_chip 0.05, jobs 1, macOS): 메인 빌드 뒤 RSS는
내려가지 않고(할당자가 쥔 힙 ~750 MB) 패스가 그 위에 창(가장 큰 셀
116 MB)·평면·인스턴스를 더해 1,054 MB(인메모리 --occupancy 737 MB,
스트리밍 ovo 없이 756 MB); Linux의 glibc/musl은 큰 벡터를 OS에
돌려주므로 보통 합산되지 않는다. 게이트 G5는 그래서 둘로 나뉜다: 메인
빌드만(ovo 없음, jobs 1)은 인메모리 피크 × 1.25 + 128 MB, 패스까지는
거기에 예산의 1/2을 더한 상한(jobs 1에서 강제, jobs 4는 보고 — 병렬
메인 빌드의 피크는 실행마다 100 MB쯤 흔들린다). main01_like 0.001(격자
7,744 × 7,790, ovo 1.0 GB, 12개 레이어가 12패스): 스트리밍 3,790 MB vs
인메모리 4,115 MB(jobs 1), jobs 4는 5,056 MB — 패스마다 평면을 새로 받고
돌려준 뒤 스레드별로 쥐고 있는 매킨토시 할당자의 보유분이며, 두 빌드
모두 피크는 `write_ovo`가 피라미드 전체(1.0 GB)를 한 버퍼로 직렬화하는
순간이다(후속: 조각내어 쓰는 기록기로 양쪽 다 1 GB를 덜 수 있다). 파일 크기 예산(none:size)은 레이어 순서로 사후 적용해
걷기가 멈췄을 자리와 같다. work 예산 초과(none:work)의 부분 카운트는
양쪽 다 스케줄에 좌우되므로 유일하게 보장하지 않는 바이트다. 게이트
`vfs_stream`이 세 fixture를 `--occupancy`로(그리고 valmini는
`--occupancy-prune 0 --occupancy-um 1`의 정확한 걷기로) 바이트 비교하고,
단위 테스트 `streaming_occupancy_matches_in_memory`는 격자·점목록·
경로·다각형·텍스트와 가지치기 가능/불가능 격자를 가진 계층을 prune
on/off × 셀 1/0.25 um × 평면 예산(1패스/여러 패스)으로 비교한다.
배치가 상한(5,000만 레코드)을 넘으면 한 줄 알리고 ovo 없이 캐시를 완성한다.
(0.12.370부터 `--occupancy-only`도 스트리밍하므로 ovo 없는 현재 캐시에는 모든
모드에서 occupancy-only 실행이 붙고, 상한을 넘는 캐시는 한 줄 알리고 그대로
둔다 — §11.)

**0.12.387 (DIRECT_READ_PLAN §8.10, L7b).** 마킹 패스의 리더 풀과 상주 창이 없어졌다:
마커 스레드 `jobs`개가 셀을 파일 순서로 맡아 그 자리에서 파싱(`StreamStore::parse_cell`:
pread + `parse_cell_bytes` + `normalize_cells`, 리더와 같은 레코드)하고 모든 인스턴스에
마킹한 뒤 버린다(cli `markpass.rs`). 입장은 "진행 중인 셀들의 Doc 추정 + 이 셀 ≤ 예산의
4분의 1(최소 가장 큰 셀)"이라 `--mem`은 그대로 상한이고, 기본 예산에서는 `jobs`개가 끝까지
돈다(현장 `--mem 0.5`의 2셀 병목은 그 예산에서만 남는다). 묶음이 안 쓰는 셀은 읽지 않는다
(방문하지 않는 셀, 작은 셀, 자기 레코드에 묶음의 레이어가 없는 셀은 보관된 배치만으로).
`begin_group`은 파일 크기 예산이 아직 받을 평면 수로도 묶음을 잘라 none:size가 될
레이어를 마킹하지 않는다(p10: 449 → 23 레이어 마킹). 다각형 스캔 변환은 버퍼를 재사용한다.
바이트는 같다(게이트 `vfs_stream`·`light_cache`, 단위 테스트
`streaming_occupancy_matches_in_memory`·`a_cell_parsed_alone_is_the_readers_cell`).
측정은 DIRECT_READ_PLAN §8.10.

**0.12.391 (DIRECT_READ_PLAN §8.14).** 여러 셀을 정의하는 범위 — 다른 셀 범위의 CBLOCK 안에 CELL
레코드가 든 스트림 — 는 마커가 한 번만 파싱한다(`StreamStore::parse_range`, markpass `Task::Range`):
그 범위의 첫 셀을 집는 마커가 아직 안 집힌 범위의 셀을 모두 함께 집고 범위 cost(정의된 셀들의 Doc
추정 합, 파싱이 실제로 만드는 양)로 입장한 뒤 셀 순서로 마킹한다. 전에는 셀마다 범위를 다시 읽고
파싱하며 그 셀의 추정만 과금했다(리뷰 2026-10-23: 24셀의 663 KiB 블록을 24번). 바이트 동일.

**0.12.392 (DIRECT_READ_PLAN §8.15).** 단위가 범위 하나에서 정의를 공유하는 셀 집합으로 넓어졌다
(`StreamStore::new`의 `SharedDef`: 범위가 정의하는 셀들로 이은 연결 집합 — 다른 셀 범위의 CBLOCK 안 CELL,
또는 각자 정의된 뒤 그런 CBLOCK에서 다시 정의된 셀). 마커는 집합의 범위를 각각 한 번씩 파싱하고
(`parse_ranges`) 집합 셀들의 Doc 추정 합으로 과금한다(markpass `Task::Shared`). 0.12.391은 셀의 첫 범위만
봐서 각자 정의된 뒤 CBLOCK에서 재정의된 셀은 여전히 셀마다 블록을 다시 읽었다(0.12.391 리뷰). 바이트 동일.

**0.12.393 (DIRECT_READ_PLAN §8.16).** 집합을 한꺼번에 파싱해 집합 전체를 과금하던 것을 리더 풀의 규칙으로
바꿨다: 마커들이 집합의 범위를 파일 순서로 하나씩 집어(`Task::Range`, `StreamStore::parse_range`) 범위가 처음
정의하는 셀들을 과금하고, 마지막 정의 범위가 읽힌 셀부터 join·마킹·해제한다(`joined`). 상주는 정의 사이에
열린 셀들 = 다중 정의 span이라 `min_budget` 이내이고, `min_budget`은 여러 셀을 정의하는 범위의 과금도 포함한다
(거절 문구에 그 범위). 0.12.392 리뷰: 셀마다 CBLOCK이 다음 셀을 정의하는 32셀 체인(Doc 58 MB)이 마킹 예산
4.6 MB에서 통째로 올라갔다(occupancy-only 패스 RSS 72 → 22.5 MB). 그룹 줄에 `resident peak`. 바이트 동일.

**0.12.394 (DIRECT_READ_PLAN §8.17).** 집합의 범위를 파싱할 때 그룹이 쓰지 않는 셀의 레코드는 만들지 않는다
(`floe_oasis::doc::parse_cell_bytes_keeping`: 파싱만 하고 보관하지 않음; 그룹 줄에 `records of other cells not
kept`). 0.12.393 리뷰: 레이어 1의 작은 셀들과 레이어 2의 큰 셀들이 범위를 나눠 쓰는 파일에서 12 마커가 큰 셀들을
만들었다 버려 예산 4.8 MB에 26.9 MB(occupancy-only RSS 12 jobs 54 → 29 MB). 집합들은 `shared_ranges`(모든 집합의
범위, 파일 순서)의 한 커서로 함께 진행된다(0.12.393은 한 번에 하나). 바이트 동일.

**0.12.398 (DIRECT_READ_PLAN §8.21).** 배치의 상주가 없어졌다. 요약 패스가 쓰는 배치는 모두
**배치 저장소**(`floe_vfs::places`)에 있다: 셀이 해제될 때(전체 빌드: `StreamStore::keep_places_into`,
커미터의 release), 세그먼트 디렉터리에서 셀 단위로 읽을 때(`floe-index light`: `LightIndex::
placements_each`, 배치 묶음마다 완성된 셀을 정규화해 넘김), 배치 수집 패스에서(occupancy-only)
`PlaceWriter::add_cell`이 셀의 정규화된 배치를 자식별 run — (자식, 부모) 간선 하나에 부모의 그
자식 레코드들, 레코드 순서, 태그 1 B + 지그재그 바리언트(평범한 배치 약 12 B, 메모리의 96 B 대비)
— 으로 덧붙이고, `finish`가 run 표를 (자식, 부모)로 정렬해 자식별·부모별 색인을 만든다. 저장
위치는 예산이 정한다: 재고의 배치 수 × 16 B ≤ 예산/4면 메모리(32 MiB 블록), 아니면 `design.places.tmp`
(캐시 디렉터리; pread로 run 단위로 읽고 저장소를 버릴 때 지운다; `FLOE_OCCUPANCY_PLACES=file|mem`이
강제). `Instances`는 자식의 incoming run으로 인스턴스를 열거한다(부모의 run을 한 번 읽어 부모의
인스턴스마다 레코드를 적용 — 전에는 레코드마다 부모를 열거했다; 작은 자식의 가지치기 가능한 격자는
건너뜀; 방문 수는 run의 `visits` 합으로 topo 순서에 셈), `mark_instance`의 charge·발자국은 부모의
outgoing run으로 본다(레코드 2^18개 이하는 목록으로 실체화, 그 위는 인스턴스마다 저장소 읽기).
상주는 run 표(간선당 32 B; MAIN01급 칩 60만 간선 = 19 MB)와 열거 중인 run뿐이다. 바이트는 같다
(게이트 `vfs_stream`·`light_cache`, 단위 테스트 `streaming_occupancy_matches_in_memory`가 메모리/파일
저장소를 번갈아 쓴다). 같이: 파일 단위 깊이 상한(`occupancy::depth_cap_for`, §8.21)으로 깊은
레이아웃이 레이어를 none:size로 잃기 전에 깊이 평면을 접는다. 측정은 DIRECT_READ_PLAN §8.21.

### 4.7 보류

`--coverage`(design.ovc)와 `--representatives`(design.ovr)는 Doc 전체를
읽는 opt-in이다. 스트리밍 모드에서는 "인메모리 빌드에서만 지원"으로
거절하고 후속으로 셀 단위 포팅한다(S7). 덱 소스는 둘 다 쓰지 않는다.
**결정(2026-10-19, 사용자): 미룬다.** design.ovc는 floe2가 읽지 않는
레거시이고, design.ovr은 덱 소스에 금지된 명시적 옵션이라 현재 쓰지
않는다.

## 5. 사전 거절과 운영 가드

- 1차 패스 뒤, 출력을 지우기 전에(마커 프로토콜 728~754보다 앞) 판정한다:
  가장 큰 셀의 doc_bytes_est가 예산을 넘으면 "largest cell N GiB, budget
  M GiB: this file needs intra-cell splitting (not implemented)"로 끝낸다.
  덱은 그 소스만 건너뛰고 계속 돈다. 기존 캐시는 남는다.
- 실행 중 RSS 상한: 하트비트가 읽는 RSS가 물리 메모리의 90%를 넘으면
  메시지와 함께 끝낸다(스왑 서버에서 24분 저속 구간을 막는다).
- 원장: 시그널 137은 "killed (out of memory?)"로 적는다.
- 이 셋은 스트리밍과 독립이라 먼저 나갈 수 있다(S6).

**구현(S6, 0.12.369 / floe-index 0.12.334; 사용자 2026-10-20 "S6
진행해줘").** `rust/cli/src/guard.rs`.
- 사전 거절. 스트리밍: 가장 큰 셀(또는 여러 곳에 정의된 셀의 스팬)이
  예산을 넘으면 `StreamStore::new`가 출력을 건드리기 전에 거절한다(S4부터;
  이제 종료 71과 "a cell larger than the budget needs intra-cell splitting,
  which is not implemented (§4.7)"). 인메모리: 파일을 읽기 전에 크기를
  MemAvailable과 견준다 — 파일이 MemAvailable보다 크면 읽지도 못하므로
  종료 71로 거절하고(캐시 그대로, "index with --stream"), 1/5을 넘으면
  "parsed form is 5-20x the file"이라고 한 줄 적는다. MemAvailable이 없는
  곳(Linux 밖)에서는 꺼져 있고, `FLOE_MEM_AVAILABLE_GB`가 그 값을 대신한다
  (게이트가 쓴다; 거버너와 `--mem` 기본값도 따른다).
- RSS 상한. 프로세스의 **익명** 상주 크기(Linux `RssAnon`, macOS
  phys_footprint — 스트리밍 재고 패스가 맵한 파일 페이지는 세지 않는다)가
  상한을 넘으면 한 줄과 종료 71로 끝낸다. 상한은 `FLOE_RSS_CEILING`: 비우면
  MemTotal의 90 %, 퍼센트(`80`), 바이트(`300G`), `off`. 검사 지점: 빌드
  시작, 캐시를 지우기 직전(파싱 뒤 / 재고 뒤 — 여기까지는 "the cache is
  unchanged"), 그리고 모든 하트비트(파싱·파이프라인·재고·리더·점유 패스,
  200 ms마다 — 이때는 "no commit marker, rebuilt next time"). 조언은 모드를
  따른다: 인메모리면 `--stream`, 스트리밍이면 `--mem`을 낮추라고, 기본 모드의
  폴백(스트리밍이 거절한 뒤의 인메모리, §11)이면 호스트 메모리를 비우거나
  `--stream --mem`으로 고집하라고.
- 원장 문구. `floe2 index DECK.jb`의 `FAILED … (exit N - …)`와 자동 색인의
  "indexing X first failed (exit N - …)"가 코드를 말로 푼다
  (`app-core index::exit_words`): 71 "stopped at the memory ceiling or
  refused for its size: index with --stream", 137 "killed (out of memory?):
  the host's OOM killer or a kill -9; index with --stream", 75 busy,
  130/143 cancelled, 134 aborted, 139 segmentation fault.
- 덱 건너뛰기. 덱 색인은 소스 하나의 실패(71 포함)를 세고 다음 소스로
  간다(기존 동작; 요약 `N built, N failed, …`, 종료 2). 거절은 출력을
  건드리기 전이라 기존 캐시가 남는다.
- 게이트 `vfs_stream`: `FLOE_RSS_CEILING=1M`의 인메모리·스트리밍 빌드와
  `FLOE_MEM_AVAILABLE_GB=0.000001`의 인메모리 빌드가 종료 71, 캐시 바이트·
  mtime 그대로, 안내 한 줄; `off`와 잘못된 값(`12x`, 기본값 적용 안내)은
  빌드. 단위 테스트 `guard::tests`(철자·호스트 수치), `index::tests::
  exit_words_name_the_memory_kill_and_the_guard`.

## 6. 결정성과 게이트

- G0 CellStore 리팩터: 인메모리 경로의 ovm/ovp/ovt/ovb/ovo/meta.json
  바이트가 리팩터 전후 동일.
- G1 1차 패스: rbb(두 변종)·topo·rank·height·rmembers·rmask·rtmask·
  layer_order·top이 Doc 기반 전처리와 셀마다 동일(valmini, sample9,
  route_chip, flat16m, 그리고 KLayout 생성 fixture).
- G2 셀 파서: 전체 파싱 Doc의 셀과 레코드 단위 동일, 정규화 포함.
- G3 패리티: 스트리밍 빌드 == 인메모리 빌드, 바이트 동일(ovm/ovp/ovt/
  ovb/ovo/meta.json). 전환의 유일한 승인 기준.
- G4 기존 게이트: jobs 1 vs N 동일(validate_vfs_split S4/S7, vfs_text X5,
  validate_vfs), KLayout XOR·보존, 마커 fault-injection.
- G5 메모리: 9.8 GB 현장 파일에서 피크 ≤ 예산 + 메타데이터 + 상수.
  합성: `tools/gen_route_chip.py --scale`로 큰 파일을 만들어 `--mem`을
  작게 주고 피크를 잰다.
- 시간 게이트: 같은 jobs에서 벽시계 회귀 ≤ 30%(메모리 플랜의 25%를
  2차 패스의 재읽기만큼 완화).

## 7. 시간 추정 (현장 파일)

| 구간 | 추정 | 근거 | 실측 (2026-10-20, `--mem 0.5`, 12 jobs) |
| --- | --- | --- | --- |
| 1차 패스 | 5~7분 | 12스레드 파싱 속도 2.2 GiB/s 환산, 레코드 저장 없음 | 131.6 s (rss 148M) |
| 2차 파싱 | 7분 | 같은 속도, 플랜과 겹쳐 돈다 | 본 빌드 5,992 s에 포함 (heartbeat: 5,740 s에 ranges 5,427/5,462, 390M/512M charged, rss 8.2G) |
| 플랜 + 인코딩 | 지금과 같음 | 9.8 GB 파일 195 s + 175 s(메모리 플랜 §1, 16워커), 레코드 수 비례 추정 20~30분 | 본 빌드 5,992 s ≈ 1.1 s/셀 — 리더 2~3개로도 대는 속도라 소비 쪽(플랜·인코딩) |
| 요약 패스 (design.ovo, S5) | — | 레이어 묶음당 1패스 | 1,765.8 s, 1패스 (plane budget 128M, resident budget 415M = 가장 큰 셀 → 상주 ≈ 2셀, ≈ 0.32 s/셀). 0.12.387부터 마커가 직접 파싱해 예산과 무관하게 `jobs`개가 돈다(재고의 1~1.5배 기대, DIRECT_READ_PLAN §8.10) |
| I/O | 50 GB × 2 | NFS 1 GB/s면 100 s | 패스에 포함 |
| 합 | | | 7,890 s |

지금의 빌드가 메모리에 들었다면 걸렸을 시간 대비 1.15~1.3배로 봤다. 실측은
`--mem 0.5`에서 7,890 s(§11 '현장 실측'): 예산은 셀의 Doc 추정치로 과금되고 평균
셀이 174 MiB이므로 512M에는 2~3셀, 요약 패스의 415M에는 2셀만 상주했다. 요약
패스는 그래서 거의 직렬이었고 기본 예산(32 GiB)이면 4~6 min으로 본다; 본 빌드는
리더 2~3개로도 소비보다 빨리 대므로 예산과 무관하게 ≈ 100 min이고, 줄이려면
소비 쪽(셀 간 플랜·인코딩 병렬)이 다음이다. 기본 예산 재실행으로 확인한다.

## 8. 단계

| 단계 | 내용 | 게이트 |
| --- | --- | --- |
| S1 재고 | scan 확장에 bbox·마스크·간선·표·layer_order·fold 추가, `floe-index inventory FILE` | G1 |
| S2 CellStore | 플래너·텍스트·인코더·meta가 `&Doc` 대신 CellStore | G0 |
| S3 셀 파서 | 바이트 범위 → Cell(s), 셀 단위 정규화 | G2 |
| S4 드라이버 | 리더 + 바이트 입장 예산 + 기존 커미터/인코더, `--stream` opt-in | G3, G4, G5 |
| S5 ovo | 인스턴스 래스터화, 배치 보관 임계 | G3 |
| S6 가드 | 사전 거절, RSS 상한, 원장 문구, 덱 건너뛰기 | 기존 게이트 |
| 전환 | 스트리밍이 기본, `--in-memory` 폴백 | G3 두 번 이상 녹색 후 |
| S7 후속 | 셀 내부 분할(평면 마스크), ovc/ovr 포팅, CBLOCK 창 단위 inflate, TCache 클래스 조정, 표현 축소 | 각각 |

S6은 S1~S5와 독립이라 먼저 번들로 낼 수 있다. S1은 scan 확장의
연장이라 작다. S2는 기계적 리팩터다. S3·S4가 본체다.

진행: S1 완료(0.12.363, §3.6; 사용자 2026-10-19 "S1부터 진행해줘").
S2 완료(0.12.365, §4.2 구현; 사용자 2026-10-19 "S2 진행해줘"): build()의
입력이 `(&dyn CellStore, PrePass)`가 되어 S4의 드라이버는 저장소 하나와
재고의 PrePass를 끼우면 된다. S3 완료(0.12.366, §4.1 구현; 사용자
2026-10-19 "S3 진행해줘"): `parse_cell_range` + `normalize_cells`, G2
녹색. S4 완료(0.12.367, §4.3 구현; 사용자 2026-10-20 "S4 진행해줘"):
`floe-index vfs --stream [--mem GiB]`, `floe2 index --stream`, 게이트
`vfs_stream`(G3·G4·G5). S5 완료(0.12.368, §4.6 구현; 사용자 2026-10-20
"S5 진행해줘"): 스트리밍 design.ovo, 바이트 동일. S6 완료(0.12.369, §5
구현; 사용자 2026-10-20 "S6 진행해줘"): 메모리 상한·크기 사전 거절(종료
71)·원장 문구. 전환 완료(0.12.370, §11; 사용자 2026-10-20 "전환해줘"):
`floe-index vfs`와 `floe2 index`의 기본이 스트리밍, `--in-memory`가 이전의
전체 파싱 빌드, 기본 모드는 스트리밍이 거절한 파일을 인메모리로 넘기고,
`--occupancy-only`도 스트리밍한다. 남은 것: S7 후속, 현장 50 GB 파일의
실측(기본 명령으로)과 9.8 GB 파일의 바이트 비교(§11의 절차).

## 9. 조사 중 발견한 문서 불일치 (구현 커밋에서 바로잡는다)

- SPEC-INDEXER §2는 셀을 "topo 순서"로 처리한다고 하나 코드는 Doc
  인덱스(파일) 순서로 커밋한다(6666·6792). topo는 fold와 rank에만 쓴다.
- 같은 절의 "파스 단일 스레드"는 `parse_doc_parallel_phased(&data, jobs)`다.
- §7의 seq u16은 u32다(3577, ovm/src/lib.rs 1209).
- SPEC-FORMATS의 "VERSION = 7"은 8이다(ovm/src/lib.rs 45).
- 메모리 플랜 §1의 `--plan-batch 1/2` 권고: 창의 바닥이 jobs라 jobs
  미만 값은 효과가 없다(6614). 플랜의 skeleton 항목은 0.10.0에서
  은퇴한 기능이다(810b4919).

## 10. 열린 질문

1. 예산 기본값: MemAvailable 절반과 32 GiB 중 작은 값으로 제안. 운영
   서버에서 다른 서비스가 쓰는 양을 고려한 값이 필요하면 알려 달라.
   (S4는 이 제안대로 구현했고 `--mem`으로 바꾼다; §4.4의 2×창 수치가
   현장 파일에서 16~39 GiB이므로 32 GiB면 평균적으로 맞고 큰 셀이 몰린
   구간에서는 리더가 기다린다.) **전환(0.12.370):** 기본값은 그대로이고,
   파일의 가장 큰 셀(또는 다중 정의 스팬)이 그보다 크면 호스트가 허락하는
   한(MemAvailable, 없으면 MemTotal의 90 %) 그만큼 올린다(§11). 다른 값이
   필요하면 `--mem`.
2. 1차 패스 결과를 사이드카로 남길지: 덱 사전 점검과 재시작에 쓸 수
   있으나 캐시 디렉터리에 파일이 하나 는다. **보류(2026-10-20):** 전환에
   넣지 않았다. 재고 패스는 scan 비용(현장 파일에서 수 분)이고, 사이드카는
   캐시 디렉터리에 무효화 규칙이 필요한 파일을 하나 더한다. 재시작이나 덱
   사전 점검 수요가 생기면 S7에서. **구현(2026-10-21, 0.12.379):**
   DIRECT_READ_PLAN L1(§8.2)이 `design.inv`(SPEC-FORMATS FLOEINV1)로 만들었다 —
   스트리밍 빌드가 design.ovm 커밋 전에 쓰고, `floe-index inventory --out/--load`가
   쓰고 읽는다. 빌드·재시작·덱 사전 점검이 그 파일을 읽어 재고 패스를 생략하는
   재사용은 아직 없다(L5의 열기 규칙과 함께).
3. ~~스트리밍 모드에서 `--coverage`·`--representatives` 거절을 받아들일지.~~
   결정됨(2026-10-19): 둘 다 현재 쓰지 않는 옵션이므로 미룬다(§4.7).
4. 전환 시점: G3가 fixture와 9.8 GB 현장 파일에서 녹색이면 기본값으로
   돌린다. 그전까지 `floe2 index`는 1차 패스의 추정이 예산을 넘는
   소스에만 `--stream`을 자동으로 켠다. **결정(2026-10-20, 사용자
   "전환해줘"):** fixture의 G3가 S4~S6의 배터리(5회)와 전환 배터리에서
   녹색이라 기본값으로 돌렸다(§11). 9.8 GB 현장 파일의 비교는 §11의 절차로
   현장에서 하고, 그때까지 `--in-memory`(또는 `FLOE_INDEX_STREAM=off`)가
   즉시 폴백이다. "추정이 예산을 넘는 소스에만 자동"이라는 중간 단계는
   두지 않았다: 재고 패스를 치르고도 인메모리로 가면 그 비용이 헛되고,
   스트리밍이 같은 바이트를 내므로 작은 파일에서도 피할 이유가 없다.

## 11. 전환 구현 (0.12.370 / floe-index 0.12.335; 사용자 2026-10-20 "전환해줘")

- **기본값.** `floe-index vfs SRC OUT`과 `floe2 index`(덱 소스, 뷰어의 덱
  색인, `info`·`render`·`probe`의 자동 색인 포함)는 스트리밍으로 빌드한다.
  모드는 셋이다(`vfs.rs resolve_build_mode`, app-core `IndexMode`): 기본
  (Auto) — 플래그 없음, floe-index가 정한다; `--stream` — 고집(스트리밍이
  안 되면 종료); `--in-memory` — 이전의 전체 파싱 빌드. 킬 스위치
  `FLOE_INDEX_STREAM=off`는 기본을 인메모리로 돌린다(모르는 값은 한 줄
  알리고 기본). `--stream`과 `--in-memory`는 배타(종료 2).
- **전체 파싱이 필요한 실행**(`--coverage`/`--coverage-only`,
  `--representatives`/`--representatives-only`, 프로파일, `--frontier-only`)은
  기본 모드에서 인메모리로 간다; `--stream`과 함께면 종료 2(그대로).
  `--mem`은 스트리밍 빌드의 것이라 인메모리로 가는 실행과 함께면 종료 2
  (`--in-memory --mem`도).
- **`--occupancy-only`도 스트리밍한다**(`vfs_stream_occupancy_only`):
  design.ovm의 식별자(소스 크기/mtime, top) 확인 → 재고 패스 → (재고의
  PLACEMENT 수가 상한 5,000만을 넘으면 패스 없이 판정) → 배치 수집 패스(모든
  셀을 한 번 읽고 바로 돌려줌, `keep_places`; `jobs`개 소비자) → S5의 레이어
  묶음 패스 → 같은 게시. 덱의 "캐시는 있는데 design.ovo만 없는 소스"가
  `--stream`에서 실패하던 결함(0.12.369)이 이것으로 없어졌고, app-core의
  `decide()`는 모든 모드에서 OccupancyOnly를 고른다(스트리밍 캐시 재사용
  규칙 삭제). 패스 중 가드의 상태 문구는 "the cache is unchanged"(캐시는
  design.ovo가 rename될 때까지 그대로).
- **기본 모드의 폴백**(출력을 건드리기 전에만): 재고가 거절한 파일(§4.1의
  참조 번호 중복 등, 또는 재고가 읽지 못한 파일)과 `StreamStore::new`의
  Inventory 오류는 "the in-memory build takes over" 한 줄과 함께 인메모리
  빌드로 넘어간다; `--occupancy-only`에서 배치가 상한을 넘으면 인메모리
  occupancy-only로 넘어갔다(`--stream`이면 한 줄 알리고 ovo 없이 종료 0) —
  0.12.398부터 배치 상한이 없어 이 폴백은 없다(§4.6).
  손상 파일은 재고가 거절 → 인메모리 파스가 같은 자리에서 거절(종료 1).
  폴백 뒤의 메모리 상한·사전 거절의 조언은 두 플래그 대신 "호스트 메모리를
  비우거나 `--stream --mem`으로 고집"(guard `Mode::Fallback`).
- **예산.** 기본은 그대로 min(MemAvailable/2, 32 GiB)(MemAvailable이 없으면
  MemTotal/2 — macOS는 hw.memsize로, 이제 Linux 밖에서도 읽는다). `--mem`
  없이 파일의 가장 큰 셀(또는 다중 정의 스팬)이 기본 예산을 넘으면 예산을
  그만큼 올린다(한 줄: "cell X needs about N resident, over the default
  budget M: the budget is raised to N (MemAvailable G)"; 상한은
  MemAvailable, 없으면 MemTotal의 90 %). 상한도 넘으면 종료 71("the budget
  cannot rise to N past …: … intra-cell splitting"). `--mem`을 준 경우는
  그대로 거절(71). 근거: 올린 예산의 스트리밍(상주 ≤ 2×창 + 플랜 스크래치)
  도 전체 파싱(Doc 전체 + 스크래치)보다 적게 든다. `NewError::Budget`이
  재고와 필요량을 돌려주므로 재고 패스는 한 번이다.
- **floe2.** `IndexOptions.mode: IndexMode {Auto, Stream, InMemory}`
  (`stream: bool` 대체), `--in-memory` 플래그, `--mem`은 Auto에서도 전달
  (`--in-memory`와는 거절), Build와 OccupancyOnly 실행에 모드를 전달.
  `exit_words(71/137)`의 조언은 모드를 가리지 않는 문구("a lower --mem for
  the streaming build, --stream for an in-memory one"). 뷰어·자동 색인은
  플래그를 주지 않으므로 그대로 스트리밍이 된다.
- **게이트.** `vfs_stream`: 기준 빌드가 `--in-memory`; 기본 명령줄(플래그
  없음)이 스트리밍하고 바이트 동일(세 fixture), `--occupancy-only`가
  스트리밍하고 design.ovo 동일(세 fixture), `floe2 index` 기본/`--stream
  --mem`/`--in-memory`, 거절 네 가지, `--coverage`의 인메모리,
  `FLOE_INDEX_STREAM=off`(동일)/모르는 값, 예산 상향(route_chip:
  MemAvailable 1.5×가장 큰 셀 → 상향·동일, 0.5× → 71), 가드(인메모리·
  스트리밍·기본), 재고가 거절하는 파일(CELLNAME 참조 번호 중복을 손으로 쓴
  OASIS: `--in-memory` 빌드, `--stream` 종료 1, 기본은 폴백·동일), 손상
  파일(`--stream`·기본). `jobdeck`: design.ovo가 없는 소스를 `--stream`과
  기본이 복구(바이트 동일), `--in-memory --force`와 기본 `--force`. 전체
  배터리는 이제 모든 색인을 스트리밍으로 돈다(이전 게이트들이 그대로 녹색
  = G4).
- **현장 검증 절차(§10.4의 9.8 GB 파일).** `python3
  tools/validate_vfs_stream.py --compare FILE.oas [--jobs N]`이 `--in-memory`
  와 기본 빌드를 임시 폴더에 만들고 파일별로 비교한다(시간·피크 RSS도
  적는다). 도구 없이: `floe-index vfs FILE A --in-memory --occupancy`,
  `floe-index vfs FILE B --occupancy`, 파일별 `cmp`. 50 GB 덱 소스는
  `floe2 index DECK.jb`를 그대로 돌리면 된다(기본 예산 32 GiB; 재고 패스는
  scan 시간; 가장 큰 셀 415 MiB).
- **현장 실측(2026-10-20/21, 50 GB 덱 소스, `floe2 index DECK.jb --mem 0.5`,
  12 jobs; 줄은 2026-10-21에 받음).** `[vfs] stream: inventory 5461 cells, 5
  layers, 1969681801 records in 131.6s (Doc estimate 927G; budget 512M from
  --mem; rss 148M)`; 예산 상향 줄 없음(가장 큰 셀 415M ≤ 512M); 본 빌드
  heartbeat `ranges 5427/5462 read, 390M of 512M charged (5740s, rss 8.2G)` →
  `build complete; 5462/5462 ranges read, 0B still charged`(5,992 s);
  `occupancy streaming: 5461 instances over 5461 cells (5461 cells
  materialized); plane budget 128M, resident budget 415M` → heartbeat `group 1
  cells 53/5461 (10s, rss 8.4G)` … `5411/5461 (1760s, rss 8.8G)` = 패스 내내
  3.1셀/s → `group 1 marked in 1765.7s (rss 8.1G)` → `1 pass(es) over the file
  (1765.8s)`; 합 7,890 s. 읽기는 §7(일정한 3셀/s가 상주 2셀 파이프라인의 속도다). 남은 것: 기본 예산 재실행의 시간(요약 패스 4~6 min 예상), 9.8 GB 파일의
  `--compare`, 뷰어 확인.
- **남은 것.** S7(§8), 사이드카의 재사용(§10.2: L1 0.12.379가 design.inv로, L2 0.12.380이
  design.seg로 만들었다, L3 0.12.381이 세그먼트를 혼자 파싱한다, L4 0.12.382가 그 둘과
  소스로 경량 캐시를 연다, L5 0.12.383이 `floe-index light`/`floe2 index --light`와 덱
  소스 경량 기본을 만들었다 — DIRECT_READ_PLAN §8.5·§8.6; 빌드가 design.inv를 읽어 재고
  패스를 생략하는 일은 아직 없다), 기본 예산 재실행.
- 0.12.385 (2026-10-22): 경량 요약 패스(`floe-index light --occupancy`)의 배치 임계가
  `--mem` 기반이 됐다(§4.6; 전체 빌드의 자기 패스는 5,000만 그대로). CBLOCK 안에 정의된
  셀이 있는 스트림은 `floe-index light`가 79로 거절하고 `floe2 index`가 전체 색인으로
  넘어간다 — DIRECT_READ_PLAN §8.8.
- 0.12.386 (2026-10-22): 경량 이미지 증분 합성(제자리 성장, 채움 시점 노드 마스크, 채움
  바이트 해제, 세그먼트 pread 묶음) — DIRECT_READ_PLAN §8.9(L7a).
- 0.12.387 (2026-10-22): 요약 패스의 마커가 셀을 직접 파싱한다(리더 풀·상주 창 없음, 묶음이
  안 쓰는 셀은 읽지 않음, 크기 예산 밖 레이어 마킹 생략, 다각형 버퍼 재사용) — §4.6,
  DIRECT_READ_PLAN §8.10(L7b). 바이트 동일.
- 0.12.391 (2026-10-23): 여러 셀을 정의하는 범위(CBLOCK 안 CELL)는 마커가 한 번만 파싱하고 범위
  cost로 과금한다 — §4.6, DIRECT_READ_PLAN §8.14. 바이트 동일.
- 0.12.392 (2026-10-23): 정의를 공유하는 셀 집합(`SharedDef`)을 마커가 한 번에 파싱·과금한다(각자
  정의된 뒤 CBLOCK에서 재정의된 셀도) — §4.6, DIRECT_READ_PLAN §8.15. 바이트 동일.
- 0.12.393 (2026-10-23): 집합의 범위를 하나씩 걷고 완성된 셀부터 해제한다(상주 = 다중 정의 span, 집합
  전체가 아님); 여러 셀을 정의하는 범위의 과금도 `min_budget`에 든다 — §4.6, DIRECT_READ_PLAN §8.16.
  바이트 동일.
- 0.12.394 (2026-10-23): 집합 범위의 파싱이 그룹이 쓰는 셀의 레코드만 보관하고(비대상 셀은 파싱만), 집합들이
  파일 순서의 한 커서로 함께 진행된다 — §4.6, DIRECT_READ_PLAN §8.17. 바이트 동일.
