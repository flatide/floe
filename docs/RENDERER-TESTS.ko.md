# 러스트 렌더러 대체 — 테스트 계약 (klayout 치환 프로젝트용)

최신화: 2026-08-22 (floe 0.11.45). 별도 진행 중인 자체 rust 렌더러가
floe 뷰어의 klayout 경로를 대체하려면 무엇을 소비하고, 무엇을
재현하고, 어떤 오라클을 통과해야 하는지의 정본 목록. 렌더러 쪽
저장소가 아니라 **floe 쪽에서 보증하는 계약**을 적는다.

관련이지만 별개인 문서: `rust/VECTOR_EXPORT_PLAN.md`(FVX) — 뷰포트
지오메트리를 독립 아티팩트로 **내보내는** 포맷 설계. 본 문서는 뷰어
런타임의 파스+래스터 경로 자체의 대체를 다룬다.

## 1. 대체 범위 — 지금 klayout이 하는 일

| 위치 | 역할 | 대체 여부 |
|---|---|---|
| `floe/viewport.py` | vfsd 델타 OASIS 파스 → 워킹셋 Layout 상주, WC top 스위칭, 페이지 셀 add/drop/evict, pick/snap 지오메트리 쿼리 | **대체 대상** |
| `floe/service.py` | 전용 프로세스에서 klayout C++ 렌더(GIL 격리) + 커버리지 numpy 팔레트 합성 + 프레임/라벨 오버레이 준비 | **대체 대상** (프로세스 분리 요구 자체는 렌더러가 GIL-프리면 소멸) |
| `floe/render.py` | headless PNG (`klayout.lay`) — CLI `render`/`clip`/캡처 | **대체 대상** |
| `tools/validate_*.py` | 게이트 오라클(멤버 recount, 픽셀 XOR) | **유지** — klayout은 대체 후에도 기준자(oracle)로 남는다 |
| `floe/cache.py`, `floe/cli.py` | 레거시 .tiles 인덱서, DRC/SVRF 등 비렌더 경로 | 무관 |

## 2. 입력 계약 (렌더러가 소비하는 것)

- **.floe 캐시**: `design.ovm`(v7 메타·인덱스) + `design.ovp`(페이지
  페이로드) + `design.ovt`(텍스트 풀) + `meta.json`(뷰어 요약·미니맵
  프런티어). 구조 정본: `docs/SPEC-FORMATS.ko.md`. 오픈 검증 규칙
  (ovm이 커밋한 ovp/ovt 바이트 길이 일치)은 `floe-vfs::Vfs::open`이
  이미 구현 — 직접 열지 말고 이 크레이트를 쓰는 것을 권장.
- **페이지 페이로드**: 각각이 **독립된 완전한 OASIS 파일**(내부
  CBLOCK 압축). 셀 하나만 담고(`Vfs::page_name()`: exact는
  `P{ci}_{li}_{seq}`, LOD 변종은 `q…` 접두), 좌표는 **셀-로컬
  dbu**. 도형 = RECTANGLE/POLYGON/PATH + repetition
  (One/Grid/Pts — Pts는 비전개, 1M 멤버 = 레코드 1개). 소스의
  CIRCLE(record 27)은 인덱싱 파스에서 **내접 64각형 폴리곤**으로
  변환되므로(0.11.46, 상수 코사인 테이블 = 플랫폼 무관 바이트)
  페이지 페이로드에 CIRCLE은 나타나지 않는다 — 렌더러는 원
  프리미티브가 필요 없다. 배치 변환은
  페이지에 없고 델타/플랜의 placement(x,y,rot,flip,rep)가 준다.
- **데몬 프로토콜(운영 경로)**: plan → delta(ack-gen 트랜잭션) →
  apply. 델타 = WC top `W{gen}_{r}_{ci}` + 페이지 셀 스플라이스,
  names 테이블은 데몬 런당 1회, 라벨은 요청별 응답(페이지에 텍스트
  없음). 정본: `docs/SPEC-VFSD.ko.md`. 세션 의미(스테일 드롭, 부분
  적용 폴트 복구, 제로 예산 축출)는 L1~L9 게이트가 고정.
- **직접 접근(브링업/벤치 경로)**: `floe-vfs` 공개 API —
  `Vfs::open(dir)`, `hier::plan_hier(&ovm, &req, &opts)`(뷰 →
  페이지 목록), **`Vfs::read_page_batch(&[u32]) ->
  Vec<(u32, Vec<u8>)>`** (0.11.45 신설: 데몬 없이 페이지 바이트
  획득 — 호출 순서 보존·중복 허용·범위 밖 = Err, IO는 파일순).
  렌더러 단독 파서/래스터 벤치는 이 경로로 시작하면 된다.

## 3. 시각 계약 (재현해야 할 페인트 규칙)

정본: `docs/SPEC-VIEWER.ko.md`, `docs/SPEC-PLANNER.ko.md`,
`rust/VFS_HIER.md`. 요점만:

- **페인트 순서**: 회색(프레임 바닥) < 디자인 < 흰 1px 외곽 —
  `validate_render_frames`가 고정.
- **프레임(계층 프런티어)**: r==0 경계 셀 전용 + 멤버 컷, 40px
  2톤, 헤어라인은 7µm 격자 대표 샘플링(rev 45; `FLOE_THIN_UM`,
  `FLOE_HAIRLINE` 노브 의미 포함).
- **레이어 fill**: 스펙클 등 패턴은 **전 레이어 공통 위상**(구멍
  공유), 불투명 겹침 — `validate_render_speckle`이 고정. 패턴
  이름/비트맵은 flateyes와 공유하는 FILL_PATTERNS 테이블.
- **DRC 오버레이**: 에러 폴리곤 fill = **solid 50% alpha**(불투명
  체커 금지 — 재제안 금지 항목), 외곽 2px, halo 없음. 캡처
  (fe_embed)와 화면이 동일해야 한다.
- **LOD 변종**: 파생 데이터 — 커버리지 superset + 1셀 과피복 한도
  (`lod_coverage_superset_and_bounded` 유닛), 플래너가 밀도
  게이트로 스왑. 렌더러는 exact/LOD 페이지를 구분 없이 그린다.
- **커버리지(v)**: design.ovc 비트플레인 → 팔레트 합성, 스펙클
  구멍 안으로 침투 금지(`validate_render_speckle`의 포함관계).
- **라벨**: 90도 회전/폰트 크기 규약(#55에서 Cairo 오버레이로 이동
  예정 — 렌더러 대체와 합류 지점).

## 4. 오라클 게이트 — 렌더러가 통과해야 할 것

기존 게이트는 "klayout 대비" XOR이므로, 렌더러는 같은 자산·같은
뷰에서 klayout 오라클과 대조된다:

| 게이트 | 내용 | 렌더러 적용 |
|---|---|---|
| `validate_vfs_render` | hier 델타 → 6뷰 × 9레이어 렌더 XOR | 그대로 (렌더 함수만 치환) |
| `validate_render_speckle` | 공통 위상·가시성·불투명 겹침·커버리지 포함관계 | 그대로 |
| `validate_render_frames` | 회색<디자인<흰 페인트 순서, 1px 외곽 | 그대로 |
| L1~L9 (`validate_vfs_lifecycle`) | 세션 수명주기(apply 대체 시 필수) | apply를 렌더러 쪽으로 바꾸면 필수 |
| X1~X6 (`validate_vfs_text`) | 라벨 응답/declutter | 라벨 그리기 치환 시 |

### 픽셀 동일성 정책 (2026-08-22 확정)

klayout과의 완전 XOR=0은 요구하지 않는다(서브픽셀 규약 차이는
정당). 대신 **바이너리 커버리지**(레이어당 on/off, oversampling 1,
AA 없음) 기준으로 아래 세 규칙을 모두 만족해야 한다 — 구현·자기
검증은 `tools/validate_render_goldens.py`:

- **P-a (경계 밴드)**: diff 픽셀은 골든 경계로부터 Chebyshev ≤1px
  밴드 **안에서만** 허용. 내부 diff 1픽셀 = 실패. (1px 위상
  시프트는 통과, 2px 시프트는 실패 — 자기검사로 고정.)
- **P-b (소멸 금지)**: 골든의 4-연결 성분은 크기 무관 후보와
  겹침 ≥1px — 1px 피처가 밴드 속으로 사라지는 것을 잡는다.
- **P-c (면적 드리프트)**: |on(후보)−on(골든)| ≤ max(16,
  0.75×골든 경계픽셀수) — half-open 채움 규약 차이(≈0.5×둘레)는
  허용, 전변 1px 성장(≈1.3×둘레)은 거부.

색·패턴·합성(스펙클/프레임/DRC 워시)은 이 정책의 대상이 아니고
§3의 speckle/frames 게이트가 따로 고정한다.

### 래스터 골든 (PX1~PX5)

`tools/validate_render_goldens.py` — klayout 오라클로 마이크로
픽스처를 정확히 고정된 뷰포트에서 구워 두고, 렌더러 출력물을 위
정책으로 대조한다. **골든은 커밋하지 않는다**(klayout 버전·호스트
종속 — manifest에 버전 기록, 불일치 시 자동 재베이크):

| 케이스 | 내용 |
|---|---|
| PX1 | 반픽셀·¼픽셀·음수 원점·원점 교차 뷰포트 반올림 (정렬 박스 격자) |
| PX2 | 엣지 기울기: 수평/수직/45°/atan(1/3)/atan(2/7)/atan(3) 웨지 |
| PX3 | 1~8px 선폭, 수평/수직/45° |
| PX4 | concave 폴리곤: L/U/plus/comb/예각 노치 (vertex/join 픽셀) |
| PX5 | PATH flush/square/round/비대칭 extension + 90°/45°/135° 꺾임 |

각 케이스는 정렬 뷰와 반픽셀 오프셋 뷰 양쪽으로 렌더된다(총
13뷰). 하네스 자기검사: 재렌더 == 골든(결정성), 정책 판별력
(shift1 통과/shift2 실패/dilate 실패/성분 소멸 실패). 렌더러
프로젝트 사용법: 같은 픽스처(.oas는 workdir에 생성됨)·같은
뷰(manifest.json)로 렌더한 PNG를 `--candidate DIR`로 대조.

`tools/validate_klayout_oracle.py`는 별도 half-phase fixture에서 같은 월드
사각형을 RECTANGLE/POLYGON/PATH로 각각 기록한다. KLayout 세 mask의 exact
동일성과 Rust 세 mask의 exact 동일성을 각각 요구하고, 엔진 사이에는 위의
P-a/P-b/P-c를 적용한다. 이 gate는 primitive 종류에 따라 fill phase가 달라지는
회귀를 1픽셀 허용 밴드 뒤에 숨기지 않는다.

**대체 시점에 새로 정할 것** (잔여):

1. **pick/snap 동등성** — 현재 klayout Layout 쿼리로 구현
   (`service.py` `_SNAP_CAP=400`/`_PICK_CAP=64`). 렌더러가 지오메트리
   상주를 가져가면 같은 캡·같은 겹침 순환 의미를 보장해야 한다.
2. **[perf] A/B 벤치** — 아래 기준선 대비 회귀 금지.

## 5. 성능 기준선 (렌더러 대체의 성공 지표)

- 뷰어 병목은 **klayout OASIS 파스 단독**이다(M3.5 실측: vfsd
  0.04s vs 전체 첫 페인트 8.4s → 예산 스트리밍으로 0.93~1.08s).
  즉 파스+적재가 빨라지면 스트리밍 라운드 체계 자체를 단순화할
  여지가 생긴다.
- 9.8G 실칩(2026-08-22, 사무실): 대부분 Calibre 동급 이상, 첫 방문
  2–3s(콜드), 재방문 고속. **풀depth+mid zoom 첫 방문 9–10s 블랙**
  (#57 싼-첫-페인트와 동일 지점) — 렌더러 대체가 직접 개선해야 할
  1차 케이스.
- 측정 도구: 뷰어 터미널 `[perf] gen=.. round=.. new=.. bytes=..
  plan=..ms delta=..ms apply=..ms draw=..ms total=..ms lod=..
  refining=.. settled=..` (라운드별 비누적). 렌더러 A/B는 같은
  뷰 시퀀스에서 apply+draw 합으로 비교한다.

## 6. 픽스처

| 자산 | 위치/생성 | 용도 |
|---|---|---|
| valmini | `tools/gen_valmini.py` (스위트가 $TMPDIR에 생성; `data/m1/valmini*.floe`는 0.11.44로 최신화됨) | 작은 적대 자산: 회전/미러 배치, 3계층, 비맨해튼, 텍스트 |
| sample9 | `tools/gen_sample9.py` | 145MB depth-9, 성능/실측 |
| repfloor / p2floor | `tools/validate_vfs_split.py`가 생성 | rep-flood(Pts/Grid 대량), 몬스터 셀 |
| frametest / thintest | `tools/gen_frametest.py` / `gen_thintest.py` | 프레임 톤/스택, rev 45 격자 |
| bench_m6fill | `data/bench_m6fill.oas` (166MB) | 단일 레이어 fill 팜 |
| 실칩 | 사무실 (9.8G 등) | 최종 실측 |

## 7. 실행 요약

```sh
# 캐시 만들기 + 플랜 확인
rust/target/release/floe-index vfs design.oas
rust/target/release/floe-index plan design.oas.floe --mode hier \
    --view x0,y0,x1,y1 --px-per-um 5 --depth 0

# 렌더러 브링업: floe-vfs로 페이지 바이트 직접 획득
#   let v = floe_vfs::Vfs::open("design.oas.floe")?;
#   let plan = floe_vfs::hier::plan_hier(&v.ovm, &req, &opts);
#   let pages = v.read_page_batch(&plan.pages)?;  // (idx, OASIS bytes)

# 오라클 게이트 (klayout 필요: .venv)
.venv/bin/python tools/validate_vfs_render.py <src.oas> <cache.floe>
.venv/bin/python tools/validate_render_speckle.py
.venv/bin/python tools/validate_render_frames.py
.venv/bin/python tools/validate_render_goldens.py            # 베이크+자기검사
.venv/bin/python tools/validate_render_goldens.py . --candidate out/  # 렌더러 대조
sh tools/validate_rust.sh        # 전체 스위트 (RUST VALIDATION: ALL OK)
```
