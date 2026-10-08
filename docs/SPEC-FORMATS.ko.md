# SPEC: 캐시 포맷 (.<src>.ice 디렉토리)

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
| bvh | 인스턴스 BVH | **BVH_LEN=56**(v8): bbox, first/count/leaf + v7 크기 주석 `max_dim`@40, `max_min`@44 (u32 포화; 서브트리 내 자식 rbbox 최대 변/최소 변) + **v8 레이어 마스크** `lmask_rec`@48(아래에 놓인 모든 셀의 재귀 레이어 마스크 합집합), `lmask_direct`@52(그 셀들 자신의 도형 마스크 합집합) — 비트셋 인덱스, `LMASK_UNKNOWN`(u32::MAX) = 기록 없음. 아래 배치가 64개(`BVH_MASK_MIN_PLACES`) 미만인 노드는 기록하지 않는다(작은 노드의 합집합은 거의 다 달라 비트셋 풀이 커진다: 전 노드 기록 시 합성 MAIN01 1/10에서 비트셋 1,800만 개·1.2 GB, 임계 64에서 84만 개·48 MB). `Builder::annotate_bvh_masks(jobs)`가 마지막 셀 뒤·finish 전에 채우며 결과 바이트는 jobs와 무관하다. v7 캐시는 열 때 "ovm version 7 (this build reads v8)" 오류 — `floe2 index <src> --force`로 다시 만든다. |
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

## design.ovo — 점유 피라미드 (선택, FLOEOVO2)

마스크 정책 광역뷰의 요약(docs/OCCUPANCY_PLAN.ko.md). `floe-index vfs
--occupancy`(`floe2 index --occupancy`, opt-in)가 만들고 `--occupancy-only`가
기존 캐시에 추가·교체한다. `design.ovo.tmp`에 쓰고 rename으로 게시하므로 이름
아래에 부분 파일이 놓이지 않는다. 자체 버전(`FLOEOVO2`, 2026-09-16; 리더는 v1
`FLOEOVO1`도 읽는다)의 sidecar이며 CACHE_VERSION과 무관하다(없거나 무효하면
"요약 없음").

```
header  magic "FLOEOVO2" | version u32 (2) | unit f64 | src_size u64 | src_mtime u64
        | cell_dbu i64 | bbox x0 y0 x1 y1 i64 | n_levels u32 | n_layers u32
        | top_len u16 | top utf8
layer k layer u32 | dt u32 | status u8 (0 ok, 1 none:cells, 2 none:work,
        3 none:size, 4 none:unsupported, 5 empty) | work u64 | n_planes u8
        (ok가 아니면 0) | n_planes × ( depth u8 | n_levels × (w u32 | h u32
        | off u64 | len u64) )
body    레벨 비트맵: row-major, 행은 바이트 패딩, 행의 i번째 셀 = byte i/8 의
        bit i%8. level L 셀 = cell_dbu × 2^L, 원점 = bbox x0/y0, grid =
        ceil(span / cell). 격자가 64 × 64 이하가 될 때까지 2배 레벨.
```

**평면(plane)** = 배치 깊이 하나의 피라미드. depth 0은 top 셀 자신의 레코드,
d는 top에서 배치 d단계 아래 셀의 레코드이며, 도형이 있는 깊이에만 평면이 있다
(오름차순). 깊이 15 이상은 평면 15에 접힌다(`DEPTH_CAP`; 요청 depth ≥ 15는
무제한과 같다). 요청 depth N은 depth ≤ N인 평면들의 OR을 그리고, 무제한은 전부,
장래의 depth 구간 [s, e]는 s..e의 OR이다 — 페이지 경로가 그 depth에서 그리는 도형
집합과 정확히 같다. **v1**(`FLOEOVO1`, version 1; 레이어 항목에 n_planes가 없고
n_levels 항목이 바로 이어짐)은 전 깊이를 평탄화한 평면 하나(`depth=all`)로 읽히며
무제한(또는 레이어별 full) depth에서만 쓰인다. 리더는 평면 depth의 오름차순·상한,
ok ↔ n_planes ≥ 1을 추가로 검사한다.

비트 = "셀의 열린 상자가 도형 내부와 양의 면적으로 만남"(KLayout `Region & box`
판정). 도형 교차로만 만들며 bbox 대체가 없다(리뷰 2026-09-11 P1-1). hull이
거부되는 path(퇴화 spine·U-turn)가 있는 레이어는 `none:unsupported`(비트맵
없음)다. 양의 면적 도형이 하나도 없는 레이어(레이어 테이블에만 있는 레이어, 폭 0
rect·path뿐인 레이어)는 `empty`(비트맵 없음, 레벨 항목은 0)로 기록되고 렌더러는
"그릴 것 없음"으로 요약한다(페이지 경로로 돌리지 않으며 상태줄의 요약 레이어
수에 든다; 2026-09-14 실측: 덱 하나에 9.8 GB, 절반이 빈 레이어의 0 피라미드).
`none:*`·`empty` 레이어는 파일 크기 상한(`none:size`)의 자리를 차지하지 않는다.
로더는 magic·버전·레벨 격자·`(w+7)/8 × h == len`·오프셋 범위(잘린
파일 거부)·비트맵이 테이블 뒤에서 테이블 순서대로 겹침 없이 이어지는지
(2차 리뷰 P2-4)를 검사하고, `identity`(src_size·src_mtime·top·레이어 테이블)가 design.ovm과
다르면 파일 전체를 거부한다. `floe-index occupancy <cache>`가 헤더·identity·
레이어 status·레벨별 set 수를 줄 단위로 출력한다(SPEC-INDEXER §6.5).

## design.ovh — 계층 요약 (FLOEOVH1, 2026-09-29)

정본: `rust/vfs/src/hiersum.rs`. 뷰어 셀 트리(SPEC-VIEWER §8c)의 색인.
0.12.300부터 밀도의 셀 덮임(`rust/vfs/src/cover.rs`, SPEC-PLANNER)도 이 파일의
엣지(자식과 멤버 수)를 읽는다. 파일 형식은 그대로다.
design.ovm의 배치 레코드는 부모별 BVH 순서(자식별 아님)라 "셀의 서로
다른 자식과 멤버 수"는 그 셀의 레코드 전부, "셀의 부모"는 레코드 전부를
읽어야 하므로 한 번 훑어 요약한다. 인덱서가 빌드 끝에 쓰고(`--no-hier`로
생략), `floe-index hier <cache> [--check]`(= `floe2 index --hier-only`)가
이전 캐시에 덧붙이거나 identity를 보고한다. tmp + rename 공개, 마커
프로토콜 밖(design.ovo와 같음): 재빌드 때 삭제 목록에 포함.

- 헤더 64 B: magic `FLOEOVH1`, version u32(1), n_cells u32, n_places u64,
  src_size u64, src_mtime u64, top u32, 예약 u32, n_edges u64, 예약 u64.
- `kids_start` (n_cells+1)×u64 — 부모별 엣지 범위.
- `edges` n_edges×48 B — child u32, 예약 u32, members u64(그 부모 안의
  그 자식 배치 멤버 수, 반복 전개, 포화), extent 4×i64(자식 재귀 bbox를
  모든 배치로 옮긴 합집합, 부모 좌표; 도형 없는 자식은 EMPTY). 한
  부모의 엣지는 child 오름차순(이진 탐색 `edge_to`).
- `parents_start` (n_cells+1)×u64, `parents` n_edges×u32 — 자식별 부모
  오름차순.
- `insts` n_cells×u64 — 탑 아래 인스턴스 수(탑 1, 미배치 0; topo_rank
  순 DP, 포화).
- identity 검사 `validate_against`: n_cells·n_places·src_size·src_mtime·
  top이 design.ovm과 같아야 하며 아니면 "요약 없음"으로 읽힌다(뷰어가
  빌드를 제안). 실측: MAIN01 1/10 합성(35,183셀·8,260만 레코드)
  11.4 s → 188,065 엣지·10.6 MB. 레코드 400만 이하 캐시는 데몬이 메모리
  요약(`HIER_INLINE_PLACES`).

## design.ovb — 페이지 점유 격자 (FLOEOVB1 v2, 2026-10-03)

정본: `rust/ovm/src/lib.rs`(`ovb_header`, `ovb_image`, `occ_cell`/`occ_edge`,
`occ_level`/`occ_coverage`, `occ_encode`/`occ_decode`, `Ovm::attach_page_occ`), 생성
`rust/cli/src/vfs.rs`(`page_occupancy`, `page_occupancy_areas`, `occ_axis_lengths`,
`OvbWriter`). 페이지마다 bbox를 64×64칸(`OCC_GRID`)으로 나눠, 칸마다 도형이 덮는 면적의
비율을 단계로 남긴다. 2패스 점이 하한 아래 페이지의 점을 그 칸에, 덮인 면적만큼 놓는다
(SPEC-PLANNER §3, `HierOpts::dot_page_occ`·`dot_occ_cover`).

- v1(0.12.274)은 칸마다 비트 하나(도형 유무)였다. v2 리더는 v1 파일을 붙이지 않는다
  (stderr 한 줄, 상자 전체 퍼뜨리기). 다시 색인하면 v2가 된다.
- 인덱서가 페이지를 쓰는 순서대로 `design.ovb.tmp`에 쓰고, 끝에 페이지 표와 머리말을 채워
  rename한다. design.ovm(마커)보다 먼저 공개하며 재빌드 삭제 목록에 들어 있다.
  `--no-page-occupancy`(= `floe2 index --no-page-occupancy`)면 만들지 않는다.
- 머리말 64 B: magic `FLOEOVB1`, version u32(2), grid u32(64), n_pages u32, 단계 수 u32(15),
  src_size u64, src_mtime u64, ovp_len u64, 페이지 표 위치 u64, 나머지 0.
- 본문: 페이지 순서대로 각 페이지의 기록(가변 길이), 그다음 페이지 표. 기록은 셋 중 하나다(`occ_record`).
  - 격자: 칸마다 단계 4비트(행 우선, 짝수 x가 아래 니블)를 raw deflate로 압축한 것이다(`occ_encode`).
    페이지 범위(긴 변)가 가장 큰 도형(긴 변)의 16배(`OCC_GRID_SHAPE`)보다 넓을 때만 둔다
    (`occ_wants_grid`).
  - 합계 8 B: 그보다 큰 도형이 있는 페이지는 도형들이 덮는 면적(dbu², f64)만 둔다(`occ_total`). 이런
    페이지는 모든 도형이 하한 아래일 때 화면에서 16 px(하한 1 px 기준) 미만이라 격자가 쓸모없다.
    작은 셀의 레이어 페이지가 대부분 여기 속한다(sample9는 9,760페이지 전부).
  - 길이 0: 기록 없음(LOD 변종 페이지, 또는 덮인 칸이 없는 페이지)이다.
  - 페이지 표: (n_pages + 1)개의 u64 시작 위치다. 페이지 i의 격자는 [표[i], 표[i+1])이고, 마지막
    값은 표 자신의 위치다.
- 칸과 단계:
  - 칸 k는 `[lo + ⌊k·ext/64⌋, lo + ⌊(k+1)·ext/64⌋)`(`occ_edge`)이고, 좌표는 그 경계로 칸에
    넣는다(`occ_cell`).
  - 단계 0은 도형이 닿지 않은 칸이다. 단계 k(1~15)는 덮인 비율 약 2^(k−15)를 뜻한다(로그 반올림,
    2배 간격). 2^−14.5보다 작은 것은 1, 1을 넘는 것(겹침)은 15다.
- 면적: 멤버의 도형 상자가 칸과 겹친 넓이를 더한다. polygon은 넓이(shoelace) / 상자 넓이,
  path는 (길이 + 연장) × 폭 / 상자 넓이의 비율로 상자에 퍼뜨린다.
  - 점 리스트는 멤버마다 더한다.
  - 직교 Grid는 x 구간들과 y 구간들의 곱이므로, 칸의 넓이 = (그 열에 든 x 길이) × (그 행에 든
    y 길이)다. 축마다 한 모서리 왼쪽에 든 길이를 등차급수 닫힌 식으로 구한다(`occ_axis_lengths`).
    멤버 수, 겹침(폭 > 간격), 음의 간격과 무관하게 O(64)다.
  - 비스듬한 Grid는 2^16 멤버까지 멤버마다, 그 이상은 멤버 넓이 합을 레코드 상자에 고르게
    퍼뜨린다.
- 출처 검사: `Vfs::open`이 붙인다. 다음 경우에는 붙이지 않는다(stderr 한 줄).
  - 머리말의 version·grid·n_pages·단계 수·src_size·src_mtime·ovp_len이 design.ovm과 다르다.
  - 페이지 표가 파일 끝에 맞지 않거나 순서가 어긋난다.
  - 풀리지 않는 격자는 그 페이지만 기록 없음으로 본다.
- 크기와 시간: CUT_DENSITY_DESIGN §10.12의 표.

## design.ovs — 점유 밀도 (FLOEOVS1 v3/v4, 2026-10-06)

정본: `rust/vfs/src/occ_density.rs`(`build`, `encode_file`, `OvsFile`), 생성 `rust/cli/src/vfs.rs`(`write_ovs`,
`write_ovs_after_build`, `remove_cell_ovs`), 그리기 `rust/render-core/src/occ.rs`·`cache.rs`(`occ_density`).
밀도 스택 2패스를 계획 없이 그리는 레이어 × 배치 depth 평면이다(0.12.317부터 기본, `FLOE_RUST_DENSITY_OCC=off`면
쓰지 않음; CUT_DENSITY_DESIGN §10.16).

- 파일:
  - `design.ovs`: 탑의 파일, version 3.
  - `design.ovs.<셀 번호>`: 탑 바로 아래에서 박스가 탑 박스의 `--roots`(기본 0.25) 이상인 셀의 파일, version 4.
    그 셀의 root 뷰가 쓴다.
- 생성:
  - 색인 끝에 기본으로 만든다(0.12.316). design.ovm(마커) 뒤에 만들며 마커 프로토콜 밖이다(design.ovh와 같음).
  - `--no-ovs`(= `floe2 index --no-ovs`)면 만들지 않는다. 잡덱 소스도 만들지 않는다.
  - `floe-index ovs <cache> [--um F] [--jobs N] [--roots F]`가 이전 캐시에 덧붙이거나 다시 만든다.
    이번에 만들지 않은 셀의 이전 파일은 지운다.
  - 파일마다 tmp + rename으로 공개한다. 재빌드 때 삭제 목록에 들어 있다(셀 파일 포함).
- 머리말 80 B:
  - magic `FLOEOVS1`, version u32(3 또는 4), group u32(8)
  - src_size u64, src_mtime u64
  - unit f64(dbu/µm), cell_dbu i64, x0 i64, y0 i64
  - w u32, h u32(레벨 0 셀 수), n_levels u32, n_layers u32
- version 4는 머리말 뒤에 24 B를 더 둔다.
  - 셀 u32, rot u8, flip u8, 예약 2 B, x i64, y i64
  - 이 값은 그 셀의 (첫) 배치다. 셀 좌표를 탑 좌표로 옮기는 `Xf::place(x, y, rot, flip)`다.
  - 격자는 탑 좌표다. 탑 격자 중 그 배치 박스를 덮는 부분이고, 시작 셀은 8의 배수다.
- 표: 레이어마다 다음을 둔다.
  - 평면 수 u8
  - 평면마다 depth u8(0 = 그 파일 셀 자신의 도형, 15 = 그 depth와 그 아래 전부)
  - 레벨마다 (비트 위치, 길이, 평균 위치, 길이) 4×u64
- 본문: raw deflate로 압축한다. 레벨 L은 셀이 2^L배이고, 셀 수가 64 이하가 될 때까지 둔다.
  - 비트: 행마다 ⌈w/8⌉ B. 셀 i는 바이트 i/8의 비트 i%8이다.
  - 평균: 8×8 셀 묶음마다 u8 하나. 비트가 켜진 셀을 덮는 평균 비율이고, 255가 전부다.
  - 셀이 하나도 없는 레벨은 길이 0이다.
- 출처 검사(`validate_against`): src_size·src_mtime·n_layers·group이 design.ovm과 같아야 한다. 셀 파일은 셀 번호가
  그 캐시 안에 있어야 하고, 요청한 root와 같아야 한다. 아니면 "파일 없음"으로 읽혀 계획 경로로 그린다(stderr 한 줄).
  version 1·2는 거절한다.

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

## .<db>.tray — Calibre DRC 결과 pack (v2, 레이아웃 버전 4)

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
이름: 2026-09-16부터 `.<db>.tray`(db 옆 숨김 파일; `floe/cachepath.py`).
그 전의 `<db>.ice`는 발견 시 자동 개명된다(`.ice`는 지금 VFS 인덱스
폴더 `.<src>.ice/`의 접미사; 2026-08-13까지는 레거시 타일 캐시 이름).

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
  단일 스토리 `ValueError("corrupt pack - 재-pack 안내")`로
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

## <db>.clusters — 사용자 DRC 클러스터

2026-10-05. 사용자/외부 스크립트가 작성하는 UTF-8 텍스트 사이드카.
`results.db` 옆 `results.db.clusters`를 뷰어가 자동 탐색한다. pack을
직접 열어도 원래 DB 이름을 기준으로 찾는다. 수동 `load clusters…`는
임의 경로의 파일을 읽으며, 검증 성공 후 기존 클러스터 전체를 대체한다.
pack 및 waive/note 파일은 변경하지 않는다.

```ini
[M1.SPACE.1]
반복 패턴 = 1-1000000, 2000001
경계부 = 1000001-2000000
반복 패턴 = 2000002-3000000

[VIA.ENC.2]
모서리 = 1, 5, 9-12
```

- `[룰 이름]`: DB의 정확한 룰 이름(대소문자 구분). 앞뒤 공백은 제거한다.
  같은 이름의 룰 블록이 여럿이면 모호하므로 지정할 수 없다.
- `클러스터 이름 = 번호, 시작-끝, …`: **1부터 시작하는 룰 내부 번호**.
  화면 에러 그리드 및 CLI `--errs`의 `local`과 같다. 전역 번호나
  원본 ASCII 레코드의 ordinal이 아니다. 구간은 양 끝을 포함하며,
  순서는 자유다. 읽은 뒤 원래 번호 순서로 조회한다.
- 빈 줄과 `#`로 시작하는 주석 줄은 무시한다. UTF-8 BOM을 허용한다.
  번호 사이에 쉼표 또는 공백을 쓸 수 있다. 이름에는 `=`를 쓰지 않는다.
- 같은 룰 섹션/클러스터 이름을 반복하면 소속을 이어 붙인다. 클러스터
  표시 순서는 첫 등장 순서다. 수백만 개의 흩어진 번호는 여러 줄에 나눠 쓴다.
- 중복/겹치는 범위(같은 클러스터 내 중복 포함), 0/음수/역방향/룰 에러 수
  초과, 없는 룰, 문법 오류는 **파일 전체를 거부**한다. 오류는 파일과 줄을
  표시하고, 수동 로드 실패 시 기존 클러스터를 유지한다.
- 누락된 번호는 자동 `Unclustered`에 속한다. 이 이름은 예약어다.
  `[룰]` 헤더만 있고 클러스터가 없는 섹션은 무시한다.
- 이 파일에는 DB 지문이 없다. DRC 재실행으로 에러 순서가 바뀌었는데
  룰 이름과 번호 범위가 그대로면 이를 감지할 수 없으므로, 해당 결과와
  함께 생성·보관하고 재실행 시 다시 작성해야 한다.

저장/조회: 정규화한 불연속 구간과 누적 개수를 보관한다. 에러마다
Python 객체나 소속 비트맵을 만들지 않으며, 일반 페이지와 rank는 구간
누적합을 이진 탐색한다. 메모리는 에러 수가 아닌 **구간 수**에 비례한다.
status 카운트 초기화만 해당 상태 바이트를 읽고, 이후 카운트는 캐시한다.
한 룰의 여러 클러스터는 청크 누적합을 공유하여 같은 상태 바이트를 반복해서
읽지 않는다. 룰 전체가 waived 또는 미waived면 상태 스캔도 생략한다.
waive 페이지/rank/변경은 최대 1M 에러 단위 청크를 사용하며 도형을 읽지
않는다. 공간 조회의 소속 마스크는 bounded 쿼리 청크에만 만들고,
지오메트리 디코딩 및 cap **이전**에 적용한다.

검증: `tools/validate_drc_clusters.py`,
`tools/validate_drc_cluster_spatial.py`
(`sh tools/validate_rust.sh --only drc_clusters`).

## <deck>.rules.json — SVRF 룰 메타데이터 사이드카 (v1)

정본: `rust/cli/src/svrf.rs`(`floe-index svrf deck.cal [-D SW]…`; 2026-09-29
`floe/svrf.py`의 `floe svrf`에서 이식 — 파이썬 파서와 사이드카 바이트·scan
출력·파스 상태가 게이트 덱 31회 + 무작위 덱 3,000개에서 동일함을 확인한 뒤
옮겼다. `generated_by`만 `floe-index <버전>`), 읽기 쪽 `floe/svrf.py`
(`load_rules`·`rhs_operands` — 연산자 단어 목록은 빌더와 같아야 하며
게이트가 고정), 게이트 `tools/validate_svrf.py` R1~R5. 파일은 파이썬
`json.dump(indent=1, sort_keys=True)`와 같은 바이트(ASCII 전용 `\uXXXX`,
파이썬 float repr). Calibre SVRF 룰덱의 **서브셋 파스**
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
  `source sourceme.* && floe-index svrf ...`; 전체 env 벌크 임포트
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
- **치수 분류/연속 옵션**(R3c, 2026-10-06): 단일 레이어
  `EXTERNAL … NOTCH`는 `notch`, 명확한 두 레이어 `INTERNAL`은
  `overlap`으로 분류한다. `ENCLOSURE`는 extension 검사도 포함하므로
  `enclosure`를 유지한다. `OVERLAP` 옵션 자체로 metric을 바꾸지 않는다.
  줄바꿈된 옵션은 같은 측정문의 모든 제약에 동일한 `text`/metric으로
  보존하고, 옵션 뒤의 비교값은 주 CD 한계로 추가하지 않는다.
  구형 사이드카도 보존된 원문 근거가 있으면 로더가 분류를 보정한다.
  구형 파일에서 누락된 연속 옵션은 사이드카를 재생성해야 복구된다.
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

## 선택 대표 파일 design.ovr (OVR1)

0.12.154부터 일반 레이아웃용 네이티브 대표 점을 별도 파일로 저장한다.
OVM CRC32와 소스 식별자, 레이어·깊이 그룹, 128점 공간 디렉터리 및
40-byte 점 레코드, 파일 CRC32로 구성한다. OVM/OVP 버전 변경 없음.
[형식·상한·폴백 계약](REPRESENTATIVES.ko.md)을 따른다.
