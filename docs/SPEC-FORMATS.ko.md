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

## <db>.ice — Calibre DRC 결과 인덱스 사이드카 (v1)

정본: `rust/cli/src/drcice.rs`(빌더 `floe-index drc results.db`),
`floe/drc.py` IceDb(리더). 수백 GB ASCII .db는 **변환하지 않는다** —
.db가 정본으로 남고, 사이드카는 mmap 랜덤 액세스용 오프셋 인덱스만
담는다(원본의 2~4%). 좌표는 조회 시 해당 레코드 슬라이스만 ASCII
파스(load_ascii와 동일한 관용 파서 — 동치성은 D1 게이트가 고정).

레이아웃(전부 LE, 헤더 40B/푸터 80B):

```
[헤더]   magic "FLOEICE\0" · u32 version=1 · u32 flags ·
         f64 precision · u64 src_size · u64 src_mtime(초)
[에러 인덱스]  에러당 16B: u64 src_off · u32 src_len ·
         u8 kind(0=p,1=e) · 3B pad — src_off..+len = .db의 레코드
         헤더 줄부터 마지막 좌표 줄까지
[체크 디렉토리] 체크당 48B: u32 name_ref · u32 desc_start ·
         u32 desc_cnt · u32 pad · u64 err_start · u64 err_cnt ·
         u64 declared · u64 original
[desc refs]    설명 **줄 단위** u32 문자열 ref — 체크마다 반복되는
         "Rule File Pathname:/Title:" 줄이 여기서 1회로 dedup
[문자열 테이블] u32 len + 원시 바이트, ref = 섹션 내 오프셋
[푸터]   8×u64 섹션 오프셋/개수 · u32 cell_ref · u32 · magic
```

- stale 규칙: src_size/src_mtime 불일치 시 리더가 거부, 뷰어는 ASCII
  전체 파스로 폴백(+stderr 안내). 재굽기는 `floe-index drc` 재실행.
- 오픈은 푸터(EOF 역방향) 기준 — 빌더는 단일 순방향 패스로 쓴다.
- 파스 관용 규칙(빈 줄/CRLF/선언 개수 무시/미지 레코드 스킵/절단
  허용)은 drc.py와 러스트 빌더가 **동일 상태기계**를 공유한다.
- **관리 섹션 필터**: 파일 끝의 `DENSITY_RDBS`/`NET_AREA_RATIO_RDBS`/
  `DFM_RDBS`/`LAYOUT_INPUT_EXCEPTION_RDBS` 블록은 rdb 파일 목록이지
  룰체크가 아님 — 이름이 `_RDBS`로 끝나고 **에러 0건**인 블록은 양쪽
  파서가 동일하게 드롭한다(에러를 가진 체크는 이름과 무관하게 유지 —
  누락은 버그 원칙). 설명 줄 수는 카운트 줄의 세 번째 정수를 그대로
  따르므로 `Waiver Criteria:` 등 추가 줄도 자동 수용.

주의: 구 `.ice`(레거시 타일 캐시 디렉토리)는 2026-08-13에 `.tiles`로
개명되어 이 확장자는 DRC 인덱스 전용이다.

### .ice v2 — 완전 변환(pack, `--pack [--jobs N]`)

자기완결 포맷(.db 불필요): 룰 테이블이 파일 앞에 나열되고 에러는
룰(체크)에 그룹으로만 귀속된다. 에러당 고정 로케이터 없음.

```
[헤더 40B]  version=2, flags=1 (+precision/src size·mtime 정보성)
[좌표 블롭] 64에러 블록 단위 varint 스트림: 레코드 =
            uv((점수<<1)|종류) · zz(서수 델타) · zz(첫점 델타,
            직전 에러 기준) · 이후 점들은 직전 점 델타.
            블록 시작에서 델타 리셋. 좌표는 무손실 dbu 정수
            (비정수 좌표 파일은 pack이 거부 — v1 사용).
[qbox]      에러당 4B: 소속 체크 bbox 위 256×256 u8 격자로
            바깥쪽 라운딩한 bbox — 디코드 없이 레코드 단위
            후보 필터(항상 superset).
[블록 테이블] 블록당 48B: 블롭 오프셋·개수·i64 bbox(dbu)
[체크 디렉토리 64B/체크][desc refs][문자열 테이블][푸터 120B]
```

- **공간 정렬**: 체크 내부 에러를 첫점 Morton(Z-order)으로 재배열 —
  블록 bbox가 조밀해져 공간 인덱스가 되고 첫점 델타도 작아진다.
  부작용: 브라우저의 체크 내 나열 순서가 기록순이 아님(#서수는 보존).
- **위치 쿼리** `IcePack.query_rect(µm rect, cap)`: 체크 bbox →
  qbox(레코드 단위) → 해당 블록만 디코드 + 정확 bbox 확정.
  실측(95MB/50만 에러, 전 체크 다이 전역 산포의 최악 합성):
  10µm 42ms · 100µm 179ms · full-die cap800 5ms.
- **병렬 빌드**: 파일을 --jobs 바이트 구간으로 분할, 워커가 체크
  헤더 패턴에 투기적 동기화 후 체크 단위로 파스 → 코디네이터가
  구간 이음새를 체크 오프셋 완전 일치로 검증(불일치 = 오동기 →
  신뢰 경계부터 순차 재파스). **산출 바이트는 --jobs 무관 동일**
  (D5). 실측 95MB 8코어 0.18s (~530MB/s).
- 크기 실측: 합성 95MB → 22MB(1/4.3; qbox 4B/에러 포함).
- 리더 디스패치: 헤더 version 필드(1=사이드카 IceDb, 2=IcePack).
