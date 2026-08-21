# SPEC: 캐시 포맷 (.floe 디렉토리)

정본 코드: `rust/ovm/src/lib.rs` (Builder/Ovm), 검증:
`tools/validate_vfs.py`(오픈 검증), `rust/VFS_HIER.md` par.1~2.

## design.ovm — 메타·인덱스 (VERSION = 7)

단일 파일, 섹션 연속 배치. `Ovm::open`(mmap, 2단 검증: 얕은 헤더 +
샘플링)과 `Ovm::from_bytes`(딥 검증 — 인덱서가 커밋 직전 1회 통과)로
읽는다. 헤더에 `unit`(dbu/µm), `src_size`/`src_mtime`(소스 동일성),
`top`, 섹션 오프셋/카운트, `ovp_len`/`ovt_len`이 박힌다.

### 섹션

| 섹션 | 레코드 | 핵심 필드 |
|---|---|---|
| names | 셀명 풀 | |
| layers | 레이어 테이블 | (layer, dt, name), 레이어 비트마스크용 인덱스 |
| cells | 셀 디렉토리 | name, height(트리 높이), topo_rank(부모<자식 보장), rbbox(재귀 bbox), bbox, place_start/count, page_start/count, bvh_start/count, prange_start/count, lmask_rec(재귀 레이어 마스크 비트셋), 텍스트 필드(v5) |
| places | 배치 | child ci, x, y, rot(0..3), flip, rep kind(0=One/1=Grid/2=Pts), Grid: na/nb/va/vb, Pts: pool 참조(오프셋+count) |
| pts pool | Pts 오프셋 풀 | Morton 정렬, 배치가 (오프셋,개수)로 참조. **1M 멤버 = 레코드 1개**(비전개) |
| bvh | 인스턴스 BVH | **BVH_LEN=48**: bbox, first/count/leaf + v7 크기 주석 `max_dim`@40, `max_min`@44 (u32 포화; 서브트리 내 자식 rbbox 최대 변/최소 변) |
| pages | 페이지 디렉토리 | **PAGE_LEN=104**: cell, layer_idx, seq, bbox, ovp 오프셋/csize/usize, records, members, max_w, max_h, lod_kind(LOD_EXACT/…), lod_page(u32, LOD_PAGE_NONE=없음), v6 `max_min`@96 (레코드별 min변의 최대 — 헤어라인 페이지 판정) |
| pranges | (cell,layer) 런 | layer_idx, page_lo, page_count, pbvh_root(PBVH_NONE=선형) |
| pbvh | 페이지 BVH | max_w/max_h 주석 (컷 프루닝) |
| bitsets | 레이어 마스크 풀 | |
| text (v5) | tbvh/텍스트 배치 인덱스 | 셀-로컬 텍스트, 라벨은 요청별 데몬 응답. tbvh 노드의 v7 크기 주석은 u32::MAX(프루닝 금지) |

### 페이지/LOD 계약

- 페이지 = (cell, layer)의 레코드 묶음(목표 1MiB, rep-split로 분할 —
  Grid는 인덱스 분할, Pts는 rebase 분할, oversize 격리).
- LOD 변종 페이지는 셀 페이지 리스트 꼬리에 붙고 `lod_page`로 링크.
  LOD_GRID=128 커버리지 격자 병합, **LOD ⊇ exact@격자, 과잉 ≤1셀** 계약.
  후보 조건 members ≥ LOD_MIN_MEMBERS(256). 셀명 "…q".
- prange의 런은 자기 (cell,layer)의 **EXACT 페이지만** 커버(오픈 검증).

### CellSink 병렬 append (#58)

빌드 워커가 셀 단위로 places/pts/bvh/pbvh/pranges를 **로컬 인덱스로
프리인코딩**(CellSink)하고, 커미터가 `Builder::append_cell_sink`로
memcpy 후 리베이스한다: places kind==2의 pool 오프셋(+pool_base),
bvh 자식(leaf→+place_start, else +bvh_start), pbvh(+page_base/
+pbvh_start), prange(page_lo+page_base, root+pbvh_start). 유닛
`cell_sink_append_is_byte_identical`이 바이트 동일성을 고정.

## design.ovp — 페이지 페이로드

페이지별 압축 OASIS 조각. 뷰어는 파스하지 않고 vfsd가 델타 저작 시
바이트 splice. 페이지 헤더의 오프셋/csize로 랜덤 액세스.

## design.ovt — 텍스트 풀

v5 텍스트 인덱스의 문자열/좌표 풀. 빈 파일 허용(mmap 0 예외 처리).

## design.ovc — 커버리지 (선택)

레이어별 밀도 비트플레인. 뷰어 `floe/coverage.py`가 컷 활성+텍셀
≤COV_MAX_TEXEL_PX(160) 시 빈 픽셀에만 팔레트 틴트 합성.

## meta.json (CACHE_VERSION = 8)

```json
{
 "version": 8, "vfs": 1,
 "src": {"path": abs, "size": N, "mtime": N},
 "dbu": 0.001, "top_cell": "TOP",
 "bbox": [x0,y0,x1,y1],            // dbu
 "grid": {...},                    // 레거시 타일 그리드 파라미터
 "layers": [{"layer","datatype","name","aliases","color","stored_shapes"}...],
 "texts": {"records","members","cells","grid_reps","pts_reps","ovt_bytes"},
 "frontier": {                     // rev 46b 미니맵 (SPEC-INDEXER §5)
   "keep": 6000, "px_per_um": F, "cut_px": 3,
   "depths": [[[x0,y0,x1,y1,band],...], ...]   // depth d = 요청깊이 d의 프레임 집합
 }
}
```

- `layers[].color`는 로딩 시 `normalize_layer_colors`(레이어 번호 팔레트)
  후 layerprops 오버레이(`apply_personal_colors`)를 거친다 — meta 파일
  자체는 불변.
- frontier의 정준 파라미터(px_per_um/cut_px)는 L9 게이트가 vfsd
  `mode=frontier`로 재생해 굽기와 박스 단위 일치를 검증하는 키다.

## 소스 동일성

ovm 헤더와 meta.src 모두 소스 절대경로/size/mtime을 기록. `Vfs::open`이
불일치 시 거부("read src"/stale). 자산 재생성 후엔 반드시 재인덱싱.

## <db>.ice — Calibre DRC 결과 pack (v2, 레이아웃 버전 4)

정본: `rust/cli/src/drcpack.rs`(빌더 `floe-index drc results.db
[--jobs N]` — pack이 유일한 출력), `rust/cli/src/drcice.rs`(공유
라인 파서), `floe/drc.py` IcePack(리더).

> **v1 오프셋 사이드카는 폐기**(2026-08-19 사용자 확정): waive
> 상태 저장 불가([status] 없음), 공간 쿼리 불가(qbox 없음), 원본
> .db를 상시 동반해야 했다(139G 실측: 사이드카 20G + 원본 139G).
> 리더도 제거 — v1 파일은 stderr 안내 후 ASCII 폴백(D2 게이트),
> `floe-index drc` 재실행으로 pack 전환. v1 레이아웃 기록은 git
> 히스토리(0.11.35 이전) 참조.

자기완결 포맷(.db 불필요): 룰 테이블이 파일 앞에 나열되고 에러는
룰(체크)에 그룹으로만 귀속된다. 에러당 고정 로케이터 없음.
파스 관용 규칙(빈 줄/CRLF/선언 개수 무시/미지 레코드 스킵/절단
허용)은 drc.py load_ascii와 러스트 빌더가 **동일 상태기계**를
공유하고, **관리 섹션 필터**도 양쪽 동일: ① `*_RDBS`로 끝나고
에러 0건인 블록만 드롭(에러를 가진 체크는 이름과 무관하게 유지),
② `__RVE_*__`(던더, `__RVE_ERROR_TAG2__` 등 RVE 내부 태그
북키핑)는 **레코드가 있어도 항상 드롭**(실덱 2026-08-20) —
그 레코드는 위반이 아니므로 전역 파일순 번호도 소비하지 않는다
(파이썬은 gnum 롤백, pack은 저장 체크 누적으로 유도 = 자동 일치).
구 `.ice`(레거시 타일 캐시 디렉토리)는 2026-08-13에 `.tiles`로
개명되어 이 확장자는 DRC 인덱스 전용이다.

```
[헤더 40B]  version=4(레이아웃 개정 카운터), flags=1
            (+precision/src size·mtime 정보성)
[좌표 블롭] 64에러 블록 단위 varint 스트림, **파일 기록순**:
            레코드 = uv((점수<<1)|종류) · zz(첫점 델타, 직전 에러
            기준) · 이후 점들은 직전 점 델타. 블록 시작에서 델타
            리셋. 좌표는 무손실 dbu 정수(비정수 좌표 파일은
            pack이 거부 — v1 사용). **레코드에 서수 없음**.
[qbox]      에러당 4B: 소속 체크 bbox 위 256×256 u8 격자로
            바깥쪽 라운딩한 bbox — 디코드 없이 레코드 단위
            후보 필터(항상 superset). 공간 쿼리의 주 필터.
[status]    에러당 1B 리뷰 상태(빌드 시 0): 0=none, 1=waived,
            2=reserved, 이후 앱 정의. 고정 오프셋이라 파일
            재작성 없이 제자리 수정(IcePack.set_status → pwrite,
            읽기 매핑과 일관). **재-pack 시 초기화됨** 주의.
[wcount]    체크당 u32 waived 카운터(빌드 시 0) — set_status가
            증분 유지, 필터 카운트는 [status] 재스캔 없이 O(1)
            (2026-08-14: 필터 전환마다 GB 스캔하던 지연 제거).
[블록 테이블] 블록당 48B: 블롭 오프셋·개수·i64 bbox(dbu)
[체크 디렉토리 64B/체크][desc refs][문자열 테이블][푸터 136B]
```

- **에러 번호 = 전역 파일순 순번**(Calibre RVE 방식, 2026-08-13
  사용자 규정): 저장이 파일순이므로 번호는 `err_start + 블록 내
  위치 + 1`로 파생되고(레코드 필드 없음), 브라우저의 룰별 나열도
  자동으로 오름차순이다. ASCII 파서/v1 리더도 동일 규칙(파일의
  `p <서수>` 토큰은 전 백엔드에서 무시).
- **위치 쿼리** `IcePack.query_rect(µm rect, cap, checks=)`:
  **스트리밍 + cap 조기 종료**(2026-08-14) — 체크 bbox → 블록
  테이블 청크(48B/블록) → 히트 희소면 블록별 qbox 행, 밀집이면
  청크 행-스팬 벡터화 → 후보 블록만 디코드 + 정확 bbox 확정.
  룰 크기에 비례하는 전량 스캔 없음(구현이었던 full-rule qbox
  스캔은 1M당 ~8ms + 콜드 페이지-인으로 필터 전환 수초의 원인).
  실측(250k 룰): full-die cap1000 6ms · 100µm 104ms(결과 비례) ·
  10µm 20ms. `waived=None/True/False`는 status 필터를 **쿼리 내부
  cap 이전**에 적용(2026-08-17: 호출측 후필터는 cap 뒤에 숨은 매칭을
  조용히 누락 — D6b) — waived=True는 [wcount]=0 룰을 O(1) 스킵.
- **병렬 빌드**: 파일을 --jobs 바이트 구간으로 분할, 워커가 체크
  헤더 패턴에 투기적 동기화 후 체크 단위로 파스 → 코디네이터가
  구간 이음새를 체크 오프셋 완전 일치로 검증(불일치 = 오동기 →
  신뢰 경계부터 순차 재파스). **산출 바이트는 --jobs 무관 동일**
  (D5). 실측 95MB 8코어 0.18s (~530MB/s).
- **원자적 기록**(2026-08-17): 인코더는 `<out>.tmpw`에 쓰고 완성
  후 rename — 재-pack이 기존 pack(열린 뷰어의 mmap 포함)을 즉시
  truncate하던 경로 제거, 실패/중단 시 기존 pack 무손상 + 임시
  파일 정리. 주의: [status]/[wcount]는 여전히 pack 안에만 있어
  재-pack이 waive 검토 상태를 초기화함(저널 사이드카는 S4에서
  결정).
- **인코더 메모리 계약**(2026-08-18): RSS가 최대 룰 크기에
  비례하지 않는다 — 룰당 per-error bbox(ebb 32B/에러)는
  상주 임계(기본 4M 에러 ≈ 128MB, `FLOE_DRC_QBOX_RESIDENT`)까지만
  유지하고, 초과 룰은 **2-pass**(bbox 프리패스로 체크 bbox 확정 →
  인코딩하며 qbox 행을 블록 단위 스트리밍). 블록 bbox 테이블도
  `.tmpb`로 스풀 후 제자리 복사(1.25G 에러 ≈ 20M 블록 = 940MB
  상주 제거). 두 경로는 **바이트 동일**(D5b가 강제 스트리밍
  vs 기본을 비교). 잔여 상주 = dir(64B/체크)+strtab.
- 크기 실측: 합성 95MB → 21MB(1/4.5; qbox 4B/에러 포함).
- **손상 방어**(2026-08-18): 리더는 헤더/푸터 길이·매직에 더해
  **전 섹션 경계**(파일 내부)와 체크 dir 범위(estart+ecnt ≤
  err_total 등)를 검증하고, 파싱 중 어떤 예외(struct.error 등)든
  단일 스토리 `ValueError("corrupt packed .ice - 재-pack 안내")`로
  정규화 — 열기 경로(ValueError/OSError 캐치)가 항상 ASCII 폴백/
  재빌드로 이어진다(D2 corrupt 픽스처 3종). `close()`가 pwrite
  fd·mmap을 해제(__del__ 연동; fd 누수 수정). 인코더는 시작 시
  잔존 `<out>.tmp*`(취소/kill 잔재)를 청소.
- 리더 디스패치: 헤더 version 필드(1=폐기된 v1 오프셋 사이드카 →
  직접 오픈 거부·사이드는 ASCII 폴백, ≥2=IcePack).
  **레이아웃 개정 규율**: pack 섹션 배치가 바뀌면 version을 올린다.
  리더는 현재 값(4)만 수용하고 옛 pack은 재-pack 안내와 함께 거부 —
  구(2) pack을 새 리더가 읽으면 푸터 크기 차이로 qbox가 40B 밀려
  **작은 사각형 쿼리만 조용히 빗나가는** 사고가 실제로 있었음
  (2026-08-13, #5/#8 하이라이트 실종).

## <deck>.rules.json — SVRF 룰 메타데이터 사이드카 (v1)

정본: `floe/svrf.py`(`python -m floe svrf deck.cal [-D SW]…`), 게이트
`tools/validate_svrf.py` R1~R4. Calibre SVRF 룰덱의 **서브셋 파스**
결과를 JSON으로 굽고 뷰어는 이 파일만 로드한다(덱 직접 파스 없음).
목적은 waive 판단 보조: 룰별 제약(연산자·수치)·참조 레이어·원천 GDS
레이어를 에러 디테일에 붙인다.

- **스코프 컷(핵심)**: 지오메트리 연산 의미는 구현하지 않는다 —
  derivation(`name = expr`)은 우변의 **피연산자 이름만** 방향
  그래프 엣지로 넣고(연산자 전부 무시), 체크의 `source_gds`는 이
  그래프를 LAYER/LAYER MAP 테이블까지 폐쇄(전이)해서 얻는다.
  **줄바꿈 derivation 지원**(2026-08-18, sfa14 실덱 ~1.5k줄):
  직전 assign의 우변이 연산자로 끝났거나 다음 줄이 연산자로
  시작하면 연속 줄로 이어 피연산자를 추가(중간에 다른 문장이
  오면 즉시 종료 — 오결합 방지). **하이브리드 VERBATIM/Tcl 덱**:
  VERBATIM·Tcl 제어 블록(`if {...}` 등)은 체크가 아니라 중괄호
  스킵, 내부 INCLUDE는 항상 인벤토리(`--scan`은 전부 추적,
  일반 파스는 `--follow-verbatim`으로 선택). DFM/RDB/DVPARAMS/
  OFFGRID와 `[`/`~`/`(` 시작 property 수식 줄은 조용히 분류
  (unknown 히스토그램 오염 방지).
- 파싱 대상: 전처리(INCLUDE 병합 — 경로 `$VAR`/`${VAR}`/`~` 환경
  확장 · `#DEFINE`/`#UNDEFINE`/`#IFDEF`/`#IFNDEF`/`#ELSE`/`#ENDIF`
  — **2-인자 값 검사** `#IFDEF STACK 6LM` = 정의됨∧값일치, 지시자
  줄 `//` 주석 제거, 따옴표 값 · `#DEFINE` 값 치환 · VARIABLE 수치
  해석 — **실런과 동일한 -D 세트 필수**, 아니면 체크 목록이
  달라진다; --scan이 스위치별 검사된 값 후보를 `NAME(v1|v2)`로
  보고. **환경 폴백**(2026-08-18): 덱이 검사하는 스위치 이름은
  -D에 없으면 os.environ을 **지연 조회**(sourceme 워크플로 —
  `source sourceme.* && floe svrf ...`; 전체 env 벌크 임포트
  아님, -D 우선, 히트는 defines로 승격되어 값 치환까지 동작).
  사용된 이름은 scan 리포트와 사이드카 stats.env_switches에
  provenance로 기록, `--no-env-switches`로 비활성),
  `LAYER`/
  `LAYER MAP`, 할당문, 체크 블록(`@` 설명 + 측정문 INTERNAL/INT·
  EXTERNAL/EXT·ENCLOSURE/ENC·AREA·DENSITY·LENGTH·ANGLE·PERIMETER·
  VERTEX에서 (metric, op, value) 추출).
- **제약 추출 규칙**(R3b, 2026-08-17): 제약 = **첫 비교연산자에서
  시작하는 연속 체인만**(`>= a <= b`, `> 0 < v` = 각 제약; 체인은
  첫 비-비교 토큰에서 종료). 그래서 옵션 토큰의 비교값(`ABUT<90`,
  `ABUT>0<90`, `OPPOSITE EXTENDED < x`)은 절대 제약으로 읽히지
  않는다. **비교연산자로 시작하는 다음 줄 = 직전 측정문의 연속**
  (SVRF 자유 서식 — 한계값이 제 줄로 감싸인 실덱 대응, 다중 줄
  가능, 블록 닫힘을 넘어 누출 금지). 한계: 피연산자가 줄바꿈으로
  갈라진 경우는 미지 히스토그램행(--scan으로 가시).
- 미인식 문장은 히스토그램 카운트 후 스킵(치명 아님). **의도적
  공백**: DMACRO/CMACRO 비전개(바디는 브레이스 깊이로 통스킵,
  CMACRO 호출 수를 경고로 노출), TVF(Tcl) 덱은 Calibre가 생성한
  SVRF 산출물을 입력으로, 멀티라인 문장은 미지원(미지 히스토그램에
  잡힘). 새 덱은 `--scan`(양쪽 #IFDEF 분기 모두 워크, 인벤토리만
  출력)을 먼저 돌려 스코프 구멍을 확인한다.
- JSON 구조(`format: "floe-svrf-rules"`, `version: 1`): `defines`/
  `variables`/`layers`(이름→[[gds,dt|null]]…)/`derived`(이름→우변
  원문; 뷰어가 `svrf.rhs_operands()`로 체인 워크)/`checks`(이름→
  desc·constraints[{metric,op,value,text}]·layers·source_gds·
  unresolved)/`stats`(스킵·CMACRO·경고 — 침묵 절단 금지).
- 뷰어 연동: .db 로드 시 자동 탐색(체크 desc의 Rule File Pathname
  베이스네임 기준 **db 옆** `<deck>.rules.json` 우선 — 덱 절대
  경로는 Calibre 런 머신 기준이라 뷰잉 머신에 없기 일쑤 — 그다음
  기록된 경로·`<db>.rules.json`) + DRC 패널 `rules…` 수동 로드.
  정보줄 `svrf N/M` = 매칭된 룰 수.
