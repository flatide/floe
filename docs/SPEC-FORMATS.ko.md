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
