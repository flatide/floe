# 캐시·pack 파일 이름 규칙 — 현행과 예정 개명 (2026-09-15)

이 문서는 **아직 적용되지 않은 개명 계획**의 정본이다. 개명 작업은 다른
워킹트리에서 진행되므로, 어느 브랜치를 보든 "지금 이름이 무엇이고 무엇으로
바뀌는지"를 여기서 확인한다. 개명 커밋이 올라가면 이 문서의 §2 상태를
"완료(커밋 SHA)"로 바꾸고 §4의 목록을 지운다.

## 1. 결정 (사용자 2026-09-15)

| 대상 | 현행 | 예정 | 예 |
|---|---|---|---|
| 레이아웃 인덱스 폴더 (VFS 캐시, `floe-index vfs` 산출) | `<src>.floe/` (보이는 폴더) | **`.<src>.ice/`** (숨김 폴더, postfix `.ice`) | `chipA.oas.floe/` → `.chipA.oas.ice/` |
| Calibre DRC 결과 pack (`floe-index drc` 산출) | `<db>.ice` (보이는 파일) | **`.<db>.tray`** (숨김 파일, postfix `.tray`) | `results.db.ice` → `.results.db.tray` |

- "숨김"은 같은 디렉터리에서 **기본 이름 앞에 `.`** 를 붙이는 것으로 본다
  (별도 폴더로 옮기지 않는다). 다른 형태로 확정되면 이 표를 고친다.
- 두 개명은 **한 커밋(한 브랜치)** 에서 코드·테스트·문서·`.gitignore`·
  로컬 픽스처를 함께 바꾼다. 절반만 바뀐 상태를 어느 브랜치에도 두지 않는다.
- 폴더 안의 파일 이름(`design.ovm/ovp/ovt/ovo`, `meta.json`)과 포맷
  (`SPEC-FORMATS.ko.md`)은 바뀌지 않는다. pack의 내부 포맷(FLOEICE 매직,
  레이아웃 버전 4)도 그대로다 — 바뀌는 것은 파일 이름뿐이다.

### 이력 (같은 확장자가 뜻을 바꿔 온 순서)

1. `<src>.ice/` — 최초 KLayout 시절 타일 캐시 폴더. 2026-08-13 `.tiles`로
   개명(`SPEC-FORMATS.ko.md`, `DRC.ko.md`). `.gitignore`의 `*.oas.ice/`와
   README "`.ice` 구조와 설계 노트" 절이 이때의 잔재다.
2. `<db>.ice` — 2026-08-14부터 DRC 결과 pack(v1 사이드카 → 2026-08-19 v2 pack).
3. `<src>.floe/` — Rust VFS 캐시(현행).
4. **이번 개명**: `.ice`가 다시 인덱스 폴더의 postfix가 되고(숨김), DRC pack은
   `.tray`(숨김)가 된다. 즉 개명 후 `.ice`를 보면 "레이아웃 인덱스"이지 DRC가
   아니다. 옛 문서·로그의 `.ice`는 날짜로 뜻을 구분한다.

## 2. 상태 — 계획 단계, 어느 브랜치에도 미적용

- 2026-09-15 기준 `main`(c10b7ef), `feature/jobdeck`(09be2ab), `feature/webui`
  (7c00e90) 모두 **현행 이름**이다. `.tray`는 어느 브랜치·워킹트리에도 없다.
- 개명 작업은 **다른 워킹트리**에서 한다. `feature/jobdeck`은 개명 전까지
  현행 이름을 유지하고, 개명이 `main`에 오르면 머지/리베이스 뒤 §4 목록을
  한 번에 처리한다(jobdeck 코드의 이름 의존은 모두 §4에 있다).
- 개명 전에는 이 문서 이외의 문서·도움말·메시지에 예정 이름을 쓰지 않는다.
  관련 문서 머리에는 이 문서를 가리키는 한 줄만 둔다.

## 3. 개명 담당이 정할 것 (결정 필요)

1. **구 이름 캐시 처리** — 실칩 덱은 소스 667개·인덱스 9.8 GB(요약 포함)이고
   재색인에 40분이 든다(`OCCUPANCY_PLAN.ko.md` §12). 재색인을 강제하지 말고
   `floe2 index`/뷰어 로드가 `<src>.floe/`를 발견하면 `.<src>.ice/`로 **개명
   (mv) 후 진행**하는 쪽을 권한다. DRC pack도 같다(`<db>.ice` → `.<db>.tray`,
   pack 매직으로 DRC pack임을 확인한 뒤). 구 이름을 계속 읽기만 하는 방식은
   두 이름이 공존해 혼란이 길어지므로 피한다.
2. **`.tiles` 레거시** — `floe/cache.py` `cache_dir_for`는 `.floe`가 없으면
   `.tiles`를 돌려준다. floe2는 VFS 전용이므로 개명 때 이 폴백을 유지할지
   함께 정한다(`floe index --legacy`는 동결 셸용).
3. **사이드카 파생 이름** — waive 자동저장 `.<db>.waive.<user>`, 노트
   `.<db>.notes.<user>.fe`, 저장 대화상자 기본 이름은 **pack 이름에서 `.ice`를
   벗겨** 만든다(`floe/drc.py` `waive_autosave_path`, `_waive_tmp_fallback`,
   `notes_autosave_path`, `_notes_tmp_fallback`; `floe/gui.py` notes/waive
   save-as). 숨김 pack `.results.db.tray`에서 postfix만 벗기면 `.results.db`가
   남아 `..results.db.waive.<user>`(점 두 개)가 된다. 파생 이름은 **.db 이름**
   (`IcePack.src_path` 또는 앞의 `.`도 벗긴 이름)에서 만들어야 한다.
4. **로드 대화상자 필터** — `floe/gui.py` `BROWSE_HIDDEN_SUFFIXES = (".floe",
   ".ice")`는 GTK 선택기가 폴더에 필터를 적용하지 않아 넣은 것이다. 새 이름은
   점 파일 규칙(`name.startswith(".")`)만으로 숨겨지므로 접미사 목록은 구 이름
   잔재를 숨기는 용도만 남는다. 1번을 "개명"으로 정하면 지워도 된다.
5. **`.gitignore`** — 레거시 `*.oas.ice/`를 `.*.ice/`, `.*.tray`로 바꾼다
   (`data/`는 이미 무시).
6. **현장 명령** — 숨김 이름은 `ls`·`find -name '*.ice'`에 안 잡힌다. 문서의
   집계 예시(`OCCUPANCY_PLAN.ko.md` §12의 `find`/`du` 줄)는 `-name '.*.ice'`로
   고친다. tcsh 사용자 안내도 같다.
7. **로컬 픽스처** — `data/m1/valmini.oas.floe`, `data/sample9.oas.floe`
   (gitignore, 각 개발 머신에 있음)와 `docs/RUST_RENDERER_HANDOFF.ko.md`의
   `FLOE_INTEGRATION_CACHE` 경로는 개명 커밋과 같은 날 각자 옮긴다.

## 4. 이름에 의존하는 지점 — `feature/jobdeck` 09be2ab 기준

머지 뒤 남은 곳은 다음으로 찾는다(§5의 "무관" 항목은 걸러 읽는다):

```sh
grep -rn -E '\.floe\b|\.ice\b' floe rust/cli/src rust/vfs/src rust/render-core/src rust/renderd/src tools docs README.md .gitignore
```

### 4.1 레이아웃 인덱스 폴더 `<src>.floe` (→ `.<src>.ice`)

경로를 **직접 계산**하는 곳(정본 후보; 개명 때 한 함수로 모으는 것이 좋다):

- `floe/cache.py` `cache_dir_for` — `Cache(src).dir`의 근원. jobdeck의
  `sources._cache_state`, `render._source_layers`, `viewer.deck_ready`가 모두
  여기를 거친다.
- `floe/cli.py` — `_cmd_index_legacy`(meta 존재 검사 + "move the .floe cache
  aside" 메시지), `_run_rust_index`(Rust에 넘기는 outdir), `cmd_index`
  (`outdir = src + ".floe"`, "cache up to date: {outdir}" 출력), `_cache_ready`
  (`src + ".floe/meta.json"`).
- `floe/gui.py` `_vfs_index_and_load` — `outdir = src + ".floe"`(File > load
  layout의 색인 argv; gate `ViewerIndexArgvTests`).
- `rust/cli/src/vfs.rs` `vfs_cmd` — outdir 생략 시 `format!("{}.floe", src)`.
  `floe-index vfs <src> [outdir]`의 기본값이므로 Rust 재빌드(RENDERD_VERSION
  bump)가 필요하다.

문구·주석(사용자에게 보이는 것부터):

- `floe/cli.py` — "[jobdeck] N source(s) have no .floe cache yet; run: …"
  (gate `validate_jobdeck` 733이 stdout을 검사), `--force` 도움말 "existing
  <src>.floe", `--profile-snapshot` 도움말, 주석 132/137.
- `floe/jobdeck/render.py` — 원장 사유 "no fresh <src>.floe cache (run
  --index)", 모듈 doc; `floe/jobdeck/plan.py` — 요약 줄 "indexed (.floe)";
  `floe/jobdeck/sources.py` — `SourceInfo.indexed` 주석, `unindexed` doc,
  report note "a fresh <src>.floe cache exists"; `floe/jobdeck/viewer.py`
  `deck_ready` doc; `floe/jobdeck/geom.py` `SKIP_NOT_INDEXED` 주석.
- `floe/gui.py` — 로드 대화상자 doc(5152, 5178), 2063 주석,
  `BROWSE_HIDDEN_SUFFIXES`(§3-4).
- `rust/render-core/src/deck.rs` — 스펙 doc "source path_hex=<hex utf-8 path
  of a .floe cache>"와 테스트 문자열(`/a/chipA.oas.floe`; 경로는 불투명해서
  포맷은 안 바뀐다). `rust/renderd/src/main.rs` — `OpenCommand.cache` doc,
  파서 테스트 `cache=/tmp/a.floe`. `rust/cli/src/vfs.rs` 모듈 doc.

테스트·스크립트(픽스처 이름·경로 조립):

- `tools/validate_jobdeck.py` — `chipA.oas.floe`/`chipB.oas.floe`/
  `mark.oas.floe` 존재 검사(1707–1825), `work / (name + ".floe")`(793),
  `CLI / "chipB.oas.floe"`(3276, 3318), stdout 문구 검사(733).
- `tools/validate_index_cli.py` — 가짜 바이너리 argv 기대값
  `str(src) + ".floe"`(59, 128–185). `tools/validate_floe2.py` — 정규 argv.
- `tools/validate_occupancy.py`(332, 453, 641, 728),
  `tools/validate_rust_renderer.py`(298), `tools/validate_klayout_oracle.py`
  (96), `tools/validate_vfs.py`, `validate_vfs_hier.py`, `validate_vfs_text.py`,
  `validate_vfs_split.py`, `validate_vfs_marker.py`, `validate_vfs_profile.py`,
  `tools/hairline_ab.py`, `tools/gen_thintest.py`, `tools/occupancy_experiment.py`.
- `tools/validate_rust.sh` — 스모크 정리 목록(37–38: `$SRC.floe`,
  `*_rust.floe`, `$SRC.ice`), `--legacy` 우회용 aside 이동(52–59),
  `VOUT="${SRC%.oas}_rust.floe"`(76).

문서(개명 커밋에서 함께 고친다; 지금은 머리 한 줄 안내만):
`README.md`(13곳; 779행 "`.ice` 구조와 설계 노트"는 레거시 타일 캐시 설명이라
개명 뒤 `.ice`와 충돌 — 삭제 또는 "레거시 `.tiles`"로 개명), `docs/FLOE2.md`,
`SPEC-FORMATS.ko.md`(제목), `SPEC-INDEXER.ko.md`(§0, §1 `[outdir=.floe]`),
`SPEC-VIEWER.ko.md`, `JOBDECK.ko.md`(§3, §5 스펙 예, §6, §10),
`OCCUPANCY_PLAN.ko.md`(§3), `RENDERER-TESTS.ko.md`, `RUST_RENDERER*.md`,
`RUST_RENDERER_HANDOFF.ko.md`, `FLOE2_OPTIMIZATION.ko.md`.

### 4.2 DRC 결과 pack `<db>.ice` (→ `.<db>.tray`)

- `rust/cli/src/drcice.rs` `drc_cmd` — usage "`floe-index drc <results.db>
  [out.ice]`", 기본 출력 `format!("{}.ice", src)`. `rust/cli/src/main.rs` usage,
  `rust/cli/src/drcpack.rs` 모듈 doc. (Rust 재빌드 필요.)
- `floe/drc.py` — `load_db`의 사이드 경로 `path + ".ice"`(v2 pack 자동 선택),
  §3-3의 파생 이름 네 곳, 오류 문구 "corrupt/truncated packed .ice"(D 게이트가
  문구를 검사하는지 확인), 모듈 doc.
- `floe/gui.py` — `_drc_open_db`의 `side = path + ".ice"`, notes/waive save-as
  기본 이름의 `.ice` 벗기기(7004, 7085), in-view 라벨 "(packed .ice v2 only)",
  주석 1275/9068.
- `floe/cli.py` — `floe2 drc` 도움말, `view --drc` 도움말("FILE.db.ice").
- 테스트 — `tools/validate_drc_ice.py`(사이드 경로 197, 픽스처 이름 310–640),
  `tools/validate_jobdeck.py` 1819(`chipA.db.ice` 자동 빌드 검사),
  `tools/validate_rust.sh` 38/113.
- 문서 — `DRC.ko.md`(§2 제목 ".ice pack"), `DRC-CLI.ko.md`(§0),
  `SPEC-FORMATS.ko.md`(§`<db>.ice`), `SPEC-VIEWER.ko.md`(167, 181, 291),
  `SPEC-VALIDATION.ko.md`(D 게이트 표), `README.md`(405).

## 5. 이름과 무관 — 바꾸지 않는다

`grep '\.floe\b'`에 같이 잡히지만 **제품 이름 'floe'** 이지 postfix가 아니다:

- 임시 파일 접두어: `.floe-jobdeck-`(`floe/jobdeck/render.py` 스펙 스테이징),
  `.<name>.floe-shot-`(`floe/shots.py`), `.<name>.floe-clip-`
  (`floe/rust_render.py`), renderd `.{name}.floe-renderd-…tmp`.
- GTK CSS 클래스 `.floe-loading`, `.floe-layers*`, `.floe-drc-*`, `.floe-note-panel`
  (`floe/gui.py` PANEL_CSS; `validate_rust_renderer` 683–698이 검사).
- `--profile-snapshot …/x.floe-profile` 예시 경로(사용자가 정하는 경로).
- 폴더 안의 `design.*`/`meta.json`, 덱 스펙 포맷, `meta.json`의 `src.path`
  (소스 경로이지 캐시 경로가 아니다), renderd `open cache=<path>`(경로는
  불투명).
