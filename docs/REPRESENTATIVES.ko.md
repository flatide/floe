# 인덱싱 시 생성하는 대표 점 — OVR1 (0.12.154)

실칩 테스트 자산: [MAIN09.oas / MAIN01.oas 통계](REALCHIP_TEST_NOTES.ko.md).

후속 설계안: [OVR2 — 형상 대표와 오차에 따른 사전 병합](OVR2_DESIGN.ko.md).
2026-09-18 사용자 요청에 따른 구현 전 설계이며, 아래 OVR1의 현재 동작과 구분한다.

일반 레이아웃의 `thin:cull`에서 컷 아래의 존재를 보여 주는 선택 기능이다.
페이지 frontier와 sub-cut wash를 켜지 않는다. 원본 플랜은 종전 cull 그대로이며,
`design.ovr`의 실제 도형 위 점을 레이어 색으로 보충한다. 박스 영역을 채우지 않는다.

## 사용

```sh
# 새 인덱스와 함께 생성: 공통 파싱 결과 Doc을 공유
floe2 index source.oas --representatives --jobs 12

# 기존 .ice에 추가/교체: OVM/OVP/OVT/meta를 보존
floe2 index source.oas --representatives-only --jobs 12

# 레이어·깊이 그룹당 저장 샘플 상한 조정 (기본 262144)
floe2 index source.oas --representatives-only --representatives-points 524288 --jobs 12
```

추가 생성도 **소스 OASIS를 한 번 파싱한다**. 기존 OVP를 전부 재디코드해서
만들지 않는다. 정상 인덱싱과 함께 실행하면 추가 파싱은 없다. `--jobs`는 기존
공통 파싱/인덱싱에 적용되고, 대표 생성 자체는 결정적인 단일 스레드 알고리즘이다.
반복 멤버 전개 대신 선택된 멤버만 찾으므로 jobs 수만큼의 생성 가속을 약속하지 않는다.

파일 생성 뒤 뷰어를 다시 연다. 파일은 해당 캐시의 첫 대표 요청에서 mmap하고 검증하며,
열린 캐시의 수명 동안 같은 스냅샷을 사용한다. `FLOE_RUST_REPRESENTATIVES=off`로 끈다.
`--representatives` 없이 인덱싱한 캐시의 기본 동작은 종전 그대로다. 이미 최신인 캐시에
`--representatives`를 주면 파일이 없을 때 추가하고, `--representatives-points`를 주면
대표 파일을 재생성한다. 덱에는 적용하지 않는다.

### 생성 실패 시 (0.12.155)

`--representatives`를 새 인덱싱이나 `--force` 재색인과 함께 준 결합 실행에서 OVR
생성이 실패하면(멤버 디렉터리·그룹 상한, 샘플링 작업 상한 등) 경고를 남기고 캐시를
`design.ovr` 없이 완성한다. design.ovm과 마커는 그대로 쓰이고, 파일은 나중에
`--representatives-only`로 추가한다. 0.12.154에서는 이 실패가 `exit(1)`이라 한 시간
넘게 만든 인덱스가 마커 없이 남았고 `--force`였다면 기존 캐시도 이미 지워진 뒤였다.
`--representatives-only` 자체의 실패는 종전대로 exit 1이며 기존 캐시를 건드리지 않는다.
게이트는 바이너리의 `--kill-at representatives-fail`(게이트 전용 모의 실패)로 두 경로를
확인한다.

## 생성 비용

1. count(아래에서 위로): 명시 레코드·배치를 읽고 `(cell, layer, datatype, 상대
   depth)`별 논리 멤버 수를 계산한다. Grid는 `na × nb`, Pts는 배열 길이만 센다.
   배치는 자식 셀별로 먼저 합산하므로(합은 순서와 무관) 비용은 레코드 수 + Σ(서로
   다른 자식 × 자식 그룹 수)이고, 메모리는 셀 × 그룹의 디렉터리뿐이다. 배치 레코드마다
   저장하는 것은 없다(0.12.156; 0.12.154의 배치당 누적 구간 엔트리는 MAIN01의 배치
   수만으로 2 GiB 상한을 넘어 실패했다).
2. resolve(위에서 아래로, postorder 역순): top의 `(layer, datatype, depth)`별로 멤버
   순위를 결정적으로 샘플링하고, 각 샘플을 (최종 top 그룹, 원래 샘플 순번, 현재 셀의
   논리 순위, 누적 변환) 요청으로 만들어 계층을 따라 내려보낸다. 셀은 모든 부모의
   요청을 모은 뒤 한 번만 훑는다. count와 같은 순서(사각형, 다각형, 경로, 배치)로
   레코드를 한 번 순회하며 그룹별 커서를 전진시키고, 배치에 떨어진 순위는 `r / n`
   (반복 멤버)·`r % n`(자식 내부 순위)으로 자식에 넘기며, 도형에 떨어진 순위는 반복
   오프셋을 산술적으로 구해 점으로 확정한다. 요청은 이동하고 처리한 셀의 버퍼는
   해제된다. Grid/Pts 전체 멤버를 순회하거나 occupancy 셀을 마킹하지 않는다.
   비용은 O(레코드)가 아니라 훑는 셀의 레코드 + Σ(서로 다른 자식 × 자식 그룹) +
   샘플 × 깊이 + 정렬이며, 총 시간은 실측 대상이다.
3. 직사각형 중심, 다각형 꼭짓점, 경로 중심선 위 점을 계층 변환한다. 다각형 bbox
   중심처럼 도형 밖일 수 있는 위치는 사용하지 않는다. 형상 크기 정보도 저장한다.
4. 점들을 Morton 순서로 정렬하고 128점씩 공간 디렉터리를 만든다.

전 파일 최대 4,194,304점(보통 최대 약 162 MiB; reader 상한 192 MiB), 그룹별 기본
262,144점이다. 전역 예산을 나눌 때 작은 그룹의 몫을 먼저 확보하고 큰 그룹에
논리 멤버 수 비례로 배분한다. 인덱싱 중 메모리는 그룹 디렉터리(셀 × 그룹, 24 B씩
최대 67,108,864개 ≈ 1.5 GiB)와 이동 중인 샘플 요청(64 B씩, 최대치는 샘플 수 4,194,304
≈ 256 MiB; 로그의 `peak_requests`)이며 파싱 Doc·좌표 캐시·결과 점·직렬화 버퍼는
별도다. "샘플 수로 제한된다"이지 "작다"는 뜻이 아니다. top 그룹 수는 최대 65,536개다
(449 쌍 × 깊이 16 = 7,184). MAIN01의 실제 디렉터리 크기는 로그 `count directory=`로
계측해야 한다. 한계·좌표/멤버 수 overflow·
계층 cycle은 오류로 종료하며, 불완전한 대표 파일을 정상 파일로 게시하지 않는다.
기존 파일은 tmp 작성·sync·rename이 완료될 때 교체된다.

생성 비용은 명시 레코드 수, 배치가 참조하는 그룹 수, 샘플 수와 계층 깊이에 좌우된다.
1조 멤버의 단일 Grid도 1개 구간과 지정된 샘플 수만 처리한다. 기존 occupancy의
논리 멤버 전수 순회를 다른 이름으로 옮긴 방식이 아니다.

## 프레임 비용과 의미

- 표시 레이어와 요청 depth 이하 그룹만 읽는다. depth 0은 top 자체의 점만 포함한다.
- 도형의 max/min 치수가 기존 size/hairline 컷 아래일 때만 점을 보충한다.
- 컷에 걸린 뒤 밀도는 두께가 아닌 그룹 분포의 **화면 면적**으로 결정한다.
  목표는 약 4 화면 픽셀당 1점이며, `4^k`의 배수인 샘플 순위만 남긴다.
  두 배 축소하면 면적이 1/4이 되어 다음 단계의 집합은 이전 집합의 부분집합이다.
  이미 컷 아래였던 점들의 성질이며, 새로 컷에 걸리는 도형은 확대 시 원본으로 보인다.
- 전 프레임 최대 262,144점이 되도록 저장 점 수에서 공통 최소 솎기 단계를 정한다.
  줌과 무관한 하한이므로 예산 완화 때문에 축소 중 점이 되살아나지 않는다.
  이 제한 때문에 원본 페이지를 더 선택하거나 디코드 예산 초과 오류를 내지 않는다.
  공간 디렉터리와 화면 밖 점은 건너뛴다. 래스터도 128점 묶음의 bbox로 타일을 거른다.
- 좌표가 같은 zero-area 점을 기존 래스터의 hairline 픽셀 경로로 그린다.
  기본 1px outline에서는 1px 점이며 footprint wash는 없다. query/pick 도형에는 넣지 않는다.
- exact/probe/덱, `thin:keep`, 기존 frontier 또는 sub-cut 진단에는 보충하지 않는다.
  점유 요약이 담당한 레이어도 중복 보충하지 않는다. medium 3px 등 기존 컷 기본값은 유지한다.
- 최초 mmap 검증 뒤에는 OVR의 점/디렉터리만 읽는다. 대표 표시를 위한 OASIS 파싱,
  OVP 디코드, 원본 반복/계층 전개는 프레임에 없다. 원본 cull 플랜이 선택한 정상
  페이지의 디코드 비용과 예산은 종전대로 존재한다.

이 버전은 **top 공간에 미리 뽑은 유한 점 샘플**이다. 정확한 occupancy, 면적/밀도 측정,
형상 복원 데이터가 아니다. 좁은 영역을 확대하면 저장 샘플 부족으로 성길 수 있고,
가늘고 긴 선은 그 길이 전체가 아니라 한 대표 위치만 남는다. 화면 밀도 목표는 최대치로,
비균일 분포·중복 픽셀·컷 필터·저장 상한 때문에 실제 밀도는 더 낮을 수 있다.
Calibre와 동일한 분포/밀도나 실칩 가속률을 보장하지 않는다. 이 한계는 필요하면
셀 단위 로컬 샘플 등 후속 형식으로 확장해야 하며, 현재 파일을 exact로 취급하면 안 된다.

## 형식·진단·검증

`design.ovr`: little endian, `FLOEOVR1` + version 1, OVM 전체 CRC32,
source size/mtime, unit, top, 그룹 수. 그룹은 layer/datatype/depth/count/members,
chunk 수를 가진다. chunk는 bbox, 점 수, 최소 rank, 점 배열이다. 점은
`x:i64, y:i64, max_dim:u64, min_dim:u64, rank:u32, flags:u32(0)` (40 bytes).
끝에 파일 전체의 CRC32를 둔다. 구조·범위·rank 유일성·checksum·OVM 일치를 검증한다.
파일 없음/손상/불일치는 stderr에 사유를 남기고 정상 cull로 폴백한다.
OVM/OVP 형식과 버전은 바꾸지 않는다. 정상 재색인은 낡은 OVR/tmp도 제거한다.

생성 로그는 `count directory=<그룹 수> (<MiB>)`, `resolve cells=<훑은 셀> peak
requests=<최대 요청 수> (<MiB>)`, 그룹별 members/points, 전체 groups/directory/
peak_requests/points/bytes/총 경과시간을 찍는다(10초마다 진행 heartbeat).
프레임 wire/perf: `stored_rep_points`, `stored_rep_tested`, `stored_rep_limited`.
뷰어 상세 상태에는 `stored reps .../tested ... (capped)`가 표시된다.

짧은 검증:

```sh
cargo test --manifest-path rust/Cargo.toml -p floe-vfs representatives::tests --lib
.venv/bin/python tools/validate_representatives.py
```

단위 검증은 거대 반복의 한정 비용·공간 분산, 회전/반사/Pts/계층 depth, 포함 관계,
파일 손상/불일치 거절, cycle/overflow, 결정성을 다룬다. 통합 검증은 기존 인덱스
파일 보존, 정상/추가 생성 일치, 원본 페이지 디코드 없이 full/depth0 표시,
점 묶음/기존 래스터 픽셀 일치, 킬 스위치와 손상 파일 폴백을 확인한다.
긴 배터리 및 실칩 성능 측정은 3c4bed6 전에는 실행하지 않았다. 실칩 기록은 아래에 둔다.

## OVR2 1단계 — 같은 샘플을 형상으로 (0.12.160, opt-in)

설계는 [OVR2 설계안](OVR2_DESIGN.ko.md). 1단계는 최초 설계의 형상 복원만 구현했다: **샘플의
선정·순번·솎기는 OVR1과 같고, 샘플 하나가 점이 아니라 형상이다.** 마스크 피라미드,
전역 프레임 솎기 해제, 공급량 증가는 포함하지 않는다(2단계 이후).

```sh
floe2 index source.oas --representatives-only --representatives-format 2 --jobs 12
```

- `--representatives-format 1|2`(기본 1 = OVR1 점). 2를 주면 현재 캐시에 OVR1이 있어도
  추가 생성으로 다시 만든다(파일 존재만으로 생략하지 않음). reader는 두 형식을 읽고,
  킬 스위치 `FLOE_RUST_REPRESENTATIVES=off`와 적용 범위(thin:cull, exact/probe/keep/덱
  제외)는 공통이다.
- 형상: Rectangle은 변환된 실제 모서리(회전 시 가로·세로가 바뀜), Polygon은 가장 긴
  비퇴화 경계 선분(동률은 원본 순서), Path는 래스터가 칠하는 외곽선
  (`path_outline_any`)의 가장 긴 선분이며 둘 다 partial 플래그를 단다. 쓸 선분이 없으면
  도형 위 점으로 퇴화하고 로그에 센다. `gate_dim`은 원본 도형 bbox의
  `min(max_dim, 2 × min_dim)`이고 `gate_dim < cut`인 샘플만 보충한다.
- 파일: magic `FLOEOVR2`, 헤더·그룹표는 OVR1과 같고 레코드는 64 B(uid = group << 32 |
  순번, x0 y0 x1 y1, gate_dim, thickness, kind, flags). 청크(128개)는 형상 중심의 Morton
  순서이고 청크 bbox는 **형상 범위의 합**이라 중심이 화면 밖인 긴 선도 조회된다. 최대
  4,194,304개 = 256 MiB, reader 상한 384 MiB.
- 표시: 형상은 `WsCell.reps`로 top 셀에 실려 wash와 분리되고, 래스터는 Rect를 실제
  사각형의 경로(`paint_world_rect`: 서브픽셀 폭은 기존 hairline 규칙)로, 선분을 경계선
  stroke로 그린다. 따라서 가는 선은 투영 길이가 4 → 3 → 2 → 1 px로 줄어든다. 비닝/
  비비닝 래스터 모두 같은 픽셀이다.
- 검증: 단위(형상의 회전·반사·경로 외곽선·점 퇴화, v2 왕복·손상 거절, 범위로 조회,
  OVR1과 같은 중첩 솎기), 게이트 `validate_representatives.py`의 OVR2 절(40개 헤어라인이
  점 42개 → 선 5,627 px, 단일 선의 그림이 exact와 동일하며 길이 4·3·2·1 px, 회전 배치,
  모든 픽셀이 실제 도형 1 px 이내, 추가 생성이 색인 보존, OVR1 옆에서 format 2 요청 시
  교체).
- 1단계만으로는 fit 밀도(그룹당 표본 수)와 인계 지점의 밀도 절벽이 그대로였다.
  후속은 마스크 피라미드 대신 아래의 사전 병합 트리로 변경했다.

## OVR2 2단계 — 사전 병합 공간 트리 (0.12.161, opt-in)

생성 명령은 동일하다. 1단계 OVR2가 있어도 이 명령으로 다시 만들어야 트리가 추가된다.

```sh
floe2 index source.oas --representatives-only --representatives-format 2 --jobs 12
```

- 형식: `FLOEOVR2`, 내부 revision 3. reader는 OVR1·OVR2 revision 2·3을 읽는다.
  기본 생성은 계속 format 1이며, 형상 실험에 format 2를 명시한다.
- 생성: 기존 count/resolve 뒤 대표 형상만 공간 정렬해 128개 잎, 최대 8분기 트리를
  만든다. 노드마다 최대 8개의 Rect 프록시와 누적 기하 오차를 저장한다. 반복 멤버
  전수 전개나 세계좌표 비트맵 생성은 하지 않는다. 형상·프록시는 파일로 스트리밍하며
  완성 파일 크기의 추가 Vec을 만들지 않는다. 파일 상한은 512 MiB다.
- 병합: 일치/연속 구간부터 병합하며, 같은 축 범위의 평행 구간은 빈 간격 절반을
  국소 오차로 쓴다. 나머지 Rect는 보수적인 거리 상한을 쓴다. 부모에는 자식 오차도
  누적한다. Segment/Point는 1단계 형상을 유지하며 트리로 공간 검색한다.
- 조회: 전역 sample count에 따른 솎기를 쓰지 않는다. bbox로 공간을 제외하고,
  오차 ≤ 0.5 px이며 노드 전체가 컷 아래일 때 프록시를 쓴다. 컷 경계는 자식으로
  내려간다. 모든 가시 레이어를 occupancy가 대체하면 OVR 열기/검증도 미룬다.
- 스타일: 1 px outline의 solid 채움, 또는 자손 모두 서브픽셀인 hairline에만 Rect
  병합을 사용한다. 후자는 병합 후에도 solid로 칠한다. 다른 스타일은 잎으로 내려가
  개별 경계를 보존한다. 같은 타일/레이어의 hairline 행 구간은 합쳐 한 번 칠한다.
- IO: 헤더/노드 디렉터리 checksum은 열 때, 형상/프록시 블록 checksum은 처음 읽을 때
  검증한다. 형상 전체를 열 때 파싱하지 않는다. OVM identity checksum은 여전히 첫
  열기의 비용이며 cold 성능 측정에 포함한다. payload는 mmap이고 별도의 전체 decoded
  캐시는 만들지 않는다. 워커 타일의 행 구간 scratch는 64K 구간을 넘기기 전에 비운다.
- 정제: 노드·후보·읽기 바이트·출력 수·예상 페인트 픽셀 작업량 및 경과 시간으로 조회를 나눈다.
  저장한 커서부터 이어가며 앞쪽 N개만 표시하고 완료하지 않는다. 중간 프레임은
  partial/final=0이고, 끝까지 조회한 결과만 final=1이다. 원본 전수 디코드로 폴백하지
  않는다. 대표 조회 때문에 추가하는 중간 래스터는 한 번으로 제한하고 남은 조회
  묶음을 모아 최종 화면을 그린다(원본 페이지 정제는 기존 정책). 이 예산은 **작업
  묶음 기준**이며, 전체 프레임 시간 상한을 보장하지 않는다. 시간 검사는 128번의
  조회 작업마다 하며 mmap page fault 등의 단일 지연을 중단시키는 hard timeout은 아니다.
- 진단: `FLOE_RUST_REPRESENTATIVES_DIRECT=on` 또는
  `FLOE_RUST_REPRESENTATIVES_MERGE=off`는 같은 저장 형상을 병합 없이 조회한다.
  `FLOE_RUST_REPRESENTATIVES_BATCH=N`은 정제 실험용 출력 묶음 크기(1–262144)다.
  모두 워커 시작 전에 설정하며 revision 3 경로에 적용된다.
- perf/어댑터: 기존 stored_rep_points/tested/limited 외에 stored_rep_nodes/proxies/bytes,
  stored_rep_pixels(조회 시 페인트 추정), stored_rep_spans/painted_pixels(실제 hairline
  행 구간 병합 뒤 페인트; 일반 Rect/Segment의 전체 픽셀 수는 아님)를 기록한다.
- 게이트: 떨어진 두 군집의 4,096개 평행선 → 8개 프록시, 1개 노드·512 B payload
  조회. 확대 시 265개 형상으로 정제하고 direct 픽셀과 일치한다. 원래 빈 군집 사이를
  채우지 않으며, 출력 묶음을 3개로 제한해도 최종 그림은 같고 중간+최종 2회만 그린다.
  별도 단위 검증으로 작은 ROI 프루닝·depth·비등방 배율·넓은 outline halo·오차 상한·
  손상 파일·취소·스타일별 행 구간 병합의 픽셀 일치를 확인한다.

샘플 공급량은 그대로다. 전역 솎기와 점 표현의 손실은 제거하지만, 원래 뽑지 않은
도형까지 생기지는 않는다. MAIN01의 실제 밀도·속도는 새 파일로 별도 측정해야 한다.

## 실칩 기록

- 2026-09-18, MAIN01(9.8 GB, 배치 6.4억 + 배열 1.6억 레코드), 0.12.155
  `--representatives-only`: `representatives: member directory exceeds 2 GiB limit`
  (배치당 누적 구간 엔트리 상한 134,217,728개를 count 단계에서 초과). 추가 생성이라
  기본 캐시는 보존됐다. 이 실패가 884f3c4(배치당 엔트리 없는 count + 스트리밍
  resolve)의 계기다.
- 2026-09-18, MAIN09(142 MB, 337 레이어, 깊이 11), 884f3c4 `floe2 index --representatives`:
  인덱싱 시간이 평소보다 약 10초 늘어난 채로 완료했고, 사용자 확인으로 실제 시간이
  줄어드는 효과가 있었다(첫 실칩 성공). 아직 기록되지 않은 값: 로그의 `count
  directory=`, `resolve … peak requests=`, groups/points/파일 크기, 그리고 fit view
  perf 라인의 `stored_rep_points/tested/limited`와 plan_ms(depth 0·full depth).
- 2026-09-18, MAIN01, 884f3c4 `--representatives-only`: 595.3초에 완료.
  `groups=2146 directory=1191003 peak_requests=4194304 points=4194304 161M (147.8s)`.
  디렉터리 119만 그룹은 상한 67,108,864의 1.8 %, OVR 생성 148초, 나머지 447초는
  원본 재파싱. 뷰어: fit view 밀도가 너무 낮고, 확대해도 점은 점으로 남다가 어느
  줌에서 갑자기 도형이 되며 그 차이가 크다(사용자). 원인은 구조적이다: 전 파일 상한
  4,194,304점을 top 그룹 2,146개가 나누어 그룹당 평균 약 1,950점이 칩 전체를 대표하고,
  프레임 상한 262,144점은 2000² px에서 15 px당 1점이며, 저장 표본은 줌에 따라 늘지
  않아 한 옥타브 확대마다 화면 안의 점이 1/4로 줄다가 `min_dim ≥ cut/2`가 되는 줌에서
  실제 도형으로 바뀐다. 표본 점은 인계 줌 근처의 실제 밀도(픽셀당 선 하나 수준)에
  도달할 수 없다(필요 공급량이 프레임 상한 × 4^옥타브). 상한을 올리면 fit view
  밀도는 오르지만 절벽은 남는다. 절벽을 없애는 것은 래스터 커버리지 피라미드
  (design.ovo)나 도형 인계를 앞당기는 컷 정책뿐이다(2026-09-18 판단, 결정 대기).
