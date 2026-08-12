# floe 아키텍처 개요

이 문서는 프로젝트를 이어받는 개발자(및 Claude Code 세션)의 진입점이다.
컴포넌트별 상세는 `docs/SPEC-*.ko.md`, 플래너의 설계 결정 이력 정본은
`rust/VFS_HIER.md`(rev 1~46b), 저장소 규약은 `AGENTS.md`.

현재 버전: floe-index **0.11.28**, ovm 포맷 **v7**, meta CACHE_VERSION 8.

## 1. 3-프로세스 구조

```
┌─ GUI (floe/gui.py, GTK3) ──────────────────────────────────────┐
│  키/마우스, 레이어·팔레트 패널, 미니맵, 상태줄, 오버레이(룰러) │
└──── mp.Queue(job/res) ─────────────────────────────────────────┘
┌─ RenderWorker 프로세스 (floe/service.py, spawn) ───────────────┐
│  klayout(Layout+LayoutView) 소유. 잡: render/pick/snap/clip/   │
│  recolor/repattern. VfsMosaic(floe/viewport.py)가 워킹셋 유지  │
└──── stdio 라인 프로토콜 (floe/vfsclient.py) ───────────────────┘
┌─ vfsd (rust/cli/src/vfs.rs, floe-index vfsd) ──────────────────┐
│  design.ovm(mmap) 위에서 hier 플랜(rust/vfs/src/hier.rs) 수행, │
│  델타 OASIS 저작, ack/gen 트랜잭션, 라벨/프런티어/프로브 응답  │
└────────────────────────────────────────────────────────────────┘
```

- GUI는 klayout을 직접 만지지 않는다(GIL/블로킹 회피).
- RenderWorker는 spawn으로 뜬다(잡 하니스 작성 시 `if __name__` 가드 필수).
- vfsd는 캐시당 1개, 라인 단위 요청/응답. 파일 핸드오프(델타/라벨/프런티어
  TSV)는 클라이언트 임시 디렉토리에 쓰고 응답에 경로를 싣는다.

## 2. 데이터 흐름 (뷰 한 번)

1. GUI `redraw()` → `_clamp_view()`(줌 클램프: MIN_SPP 0.01 ~ fit×16) →
   `_submit_render()`: 렌더 프레임 bbox를 **스페클 주기(2px) 격자에 스냅**
   (좌/상단 floor/ceil, 프레임 +2px — 팬 시 명멸 방지 계약) → render 잡.
2. service `_svc_render_vfs`: 스트리밍 라운드 루프(최대 8라운드, 예산
   적응 2048..32768KB, 마지막 라운드 stream=0). 라운드마다
   vfsd 요청 → 델타 파스·`VfsMosaic.apply_hier`(WC 재구축) →
   `Renderer.render_png` → 프레임 emit + `[perf]` stderr 한 줄.
3. vfsd `serve_hier`: `plan_hier`(뷰∩rbbox 시드, 컷 사다리) →
   `HierSession.apply`(ack 트랜잭션, 예산 내 new 페이지 선정) →
   `delta_hier`(WC 트리+프레임 rect+워시 rect를 OASIS로 저작) → 응답.
4. GUI가 프레임 픽스버프 표시. 뷰가 프레임에 덮이면(`_covered`) 재렌더
   생략, 아니면 재요청. 픽/스냅은 화면 워킹셋(WYSIWYG) 대상.

## 3. 캐시 산출물 (`<src>.floe/`)

| 파일 | 내용 | 스펙 |
|---|---|---|
| `design.ovm` | 셀/배치/BVH/페이지 디렉토리/텍스트 인덱스 헤더 (v7, mmap) | SPEC-FORMATS |
| `design.ovp` | 페이지 페이로드(압축 OASIS 조각) | SPEC-FORMATS |
| `design.ovt` | 텍스트 풀 | SPEC-FORMATS |
| `design.ovc` | (선택) 밀도 커버리지 비트플레인 | SPEC-FORMATS |
| `meta.json` | dbu/bbox/레이어(+색)/텍스트 통계/**미니맵 프런티어** | SPEC-FORMATS |

구(레거시) 경로: `.ice` 타일 캐시(`python -m floe index`) — 동결 유지.

## 4. 성능 사다리 (와이드 뷰가 사는 이유)

플래너가 화면에 안 보일 것을 단계적으로 제거한다. 순서대로:

1. **컷** (detail low/medium/high = 5/3/1px): 양변 < cut 레코드/페이지/
   서브트리 제거. 페이지는 v6 `max_min` 필드, 인스턴스 서브트리는 v7
   BVH 크기 주석(`max_dim`/`max_min`)으로 **노드 단위 프루닝**(rev 43;
   사무실 150M frames-only fit 4.5s → 수십 ms).
2. **헤어라인** (rev 41, `hairline×cut`, 기본 0.5): min변이 기준 미만인
   지오메트리 페이지·폴드 제거. **프레임은 예외** ↓
3. **프레임 7µm 격자** (rev 45): min<cut≤max 프레임은 컬 대신 셀-로컬
   7µm 격자 대표만 잔존(Grid 닫힌형 stride, 구간 경계 2/4개, 14px 미만
   강등). r==0 워크에서만 BVH max_min 프루닝 해제.
4. **프레임 4밴드** (rev 42): min변 px ≥25 흰 외곽(지오메트리 위),
   9~24 회색 외곽, 5~8 회색 채움, <5 회색 점선(모두 지오메트리 아래).
5. **LOD** (M7): members > lod_k(4.0)×화면px² 이고 LOD셀≤1px이면 병합
   변종 페이지로 스왑. rev 41/43/45 이후 체감 축소 — 디폴트 유지 확정,
   빌드 `--no-lod`로 A/B 가능.
6. **워시** (M7-C): 양축 ≤ wash_px(2.0)px 페이지는 bbox 1렉트로 붕괴.
7. **스트리밍**: 델타를 예산 단위로 나눠 첫 페인트 ~1s.

계약: **과포함은 비용, 누락은 버그**. 모든 보수적 폴백은 멤버를 더할 뿐.

## 5. 렌더 계약 (klayout, floe/render.py)

- 디자인 fill = 50% 스페클: 2×2 체커 스티플 **역상 쌍**을 페인트 플레인
  홀짝에 교대 배정(모든 레이어가 같은 구멍 공유 — klayout의 플레인별
  스티플 오프셋 상쇄). 레이어별 fill 오버라이드(`set_fill_patterns`)가
  있으면 그 비트맵, 단 speckle 지정은 쌍 경로 유지.
- 페인트 순서: [회색 프레임 언더레이(FRAME_GRAY/FILL/DOTS)] < [디자인,
  레이어 번호 오름차순 = 큰 번호 위] < [흰 프레임 FRAME_LAYER].
- 프레임 레이어 번호 = 설계 최대 레이어+1(포화 시 하향), dt = base+밴드.
- 렌더 프레임은 스페클 주기 스냅(§2) 덕에 팬/재렌더 간 픽셀 결정적.

## 6. 개인화/공유 (SPEC-PERSONALIZATION)

- Calibre `.layerprops`가 색/fill 소스: 개인(`~/.cache/floe/colors/
  <sha1(abs src)>.layerprops`) → 없으면 소스 옆(`<file>.layerprops` 또는
  `<stem>.layerprops`) 채택+개인 시드.
- 색 이름표: `floe/colornames.def`(7×7). fill 비트맵: `floe/
  fillpatterns.def`(20종, 실물 Calibre hex) — **비트맵은 개인화 없음**,
  fill *지정*만 layerprops로 개인화.

## 7. 검증 체계 (SPEC-VALIDATION)

`tools/validate_rust.sh` = 단일 진입 게이트(ALL OK 필수). valmini 자산
생성 → 러스트/파이썬 대조(XOR), vfs 오픈 검증, H1~H5(hier), L1~**L9**
(라이프사이클; L9 = 미니맵 굽기 == vfsd frontier 재생), S1~S5(분할),
X1~X6(텍스트), 마커 킬포인트, 렌더 스페클/프레임 픽셀 게이트.
러스트 유닛: `cargo test --release` (floe-vfs 41개 포함).
불변식: **빌드 결정성**(--jobs 무관 sha 동일), 브루트포스 오라클
(플래너는 절대 더 적게 선택하지 않음).

## 8. 저장소 규약 (요약; AGENTS.md·메모리와 일치)

- **plan-first**: 파일 수정 전 계획 제시→피드백→구현 (조사/계측은 예외).
- **러스트를 건드린 푸시마다 `rust/cli/Cargo.toml` 버전 범프**
  (현재 0.11.28). 파이썬 전용은 범프 없음.
- `git add`는 명시 경로만(**`add -A` 금지**, 병렬 세션 안전), 커밋 전
  branch==main 확인.
- 빌드 변경은 **바이트 동일성**이 하드 게이트.
- 트레일러: `Co-Authored-By: Claude ...` + `Claude-Session: ...`.
- 편집 스크립트로 일괄 치환 시 각 replace의 매치 여부를 assert로 검증
  (무음 no-op 사고 이력 있음).

## 9. 디렉토리 지도

```
floe/            파이썬 앱
  cli.py         CLI 디스패치 (view/index/render/clip/probe/drc/…)
  gui.py         GTK 셸 (키맵·패널·미니맵·팔레트·상태줄)  ~3.9k줄
  service.py     RenderWorker 프로세스 (잡 루프, 스트리밍 라운드, [perf])
  viewport.py    VfsMosaic (WC apply, 프레임 레이어 키, 원장)
  vfsclient.py   vfsd 라인 프로토콜 클라이언트
  render.py      klayout Renderer (스페클/프레임 플레인/fill 오버라이드)
  cache.py       캐시 로딩, layerprops 개인화, 레거시 .ice
  fillpat.py     색테이블/fill 로더, layerprops 파서, hex 변환
  colornames.def / fillpatterns.def   팔레트 단일 소스
  coverage.py drc.py instance.py …
rust/            Cargo 워크스페이스 (vendored deps, 오프라인 빌드)
  oasis/         OASIS 파서/라이터 (doc.rs: Rep{One,Grid,Pts})
  ovm/           .ovm v7 포맷 (lib.rs: Builder/Ovm/CellSink)
  vfs/           플래너 (hier.rs), 텍스트(text.rs), 세션(lib.rs)
  tiler/         Xf 변환, 레거시 타일러
  cli/           floe-index 바이너리 (vfs.rs: 빌드+vfsd+plan)
  VFS_HIER.md    플래너 설계 정본 (rev 1~46b)
tools/           gen_*(자산 생성) validate_*(게이트) make_portable.sh
docs/            SPEC-*.ko.md (본 문서 세트)
```

## 10. 진행 중/백로그 (2026-08-12 기준)

| # | 항목 | 상태 |
|---|---|---|
| #57 | refine 재설계 — 라운드당 WC 재저작 고정비 제거(구조 캐시) | 대기. `[perf]` 실측으로 판단 |
| #60 2-B | 몬스터 셀 split 재귀 병렬화 (PtsArena 재설계) | 9.8G slow-cell 내역 확인 후 |
| #61 후속 | 9.8G no-lod 저하 구간 A/B (lod 빌드 대조) | 사무실 진행 중 |
| #48 | 사무실 기본값 확정 (lod_k/격자/예산) | 부분 대체됨 |
| #55 | 라벨 Cairo GUI 오버레이 (90° 회전+폰트) | 독립, 후순위 |
| — | 지오메트리 7µm 격자 (rev 45의 지오메트리판, ovm v8 예상) | Calibre 관찰 대기 |
| — | 왼쪽 pane cell/object 브라우저 | 자리만 확보(`_left_stack`) |
| — | fill 비트맵 스탬핑 확정 시 fillpatterns.def 갱신 | 실물 반영 완료(2026-08-12), 오차 발견 시 수정 |
