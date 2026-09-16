# 캐시·pack 파일 이름 규칙 (2026-09-16 개명 완료)

정본 코드: `floe/cachepath.py`(Python 쪽 유일한 이름 규칙), `rust/cli/src/vfs.rs`
`default_outdir`/`hidden_sibling`(Rust indexer 기본값),
`rust/app-core/src/cache/names.rs`(Rust 앱의 이름·읽기 호환)로 모은다.
native 앱 게이트는 Python의 순수 이름 함수를 오라클로 대조한다. 이력·결정 과정은 §4.

## 1. 규칙

| 대상 | 이름 | 예 | 2026-09-16 이전 |
|---|---|---|---|
| 레이아웃 인덱스 폴더 (VFS 캐시, `floe-index vfs` 산출) | **`.<src>.ice/`** — 소스와 같은 디렉터리의 숨김 폴더 | `chipA.oas` → `.chipA.oas.ice/` | `chipA.oas.floe/` |
| Calibre DRC 결과 pack (`floe-index drc` 산출) | **`.<db>.tray`** — db와 같은 디렉터리의 숨김 파일 | `results.db` → `.results.db.tray` | `results.db.ice` |
| DRC 리뷰 사이드카 (리뷰어별 waive·note) | `.<db>.waive.<user>`, `.<db>.notes.<user>.fe` — **db 이름** 기준 | `.results.db.waive.jkim` | 같음 (pack 이름에서 `.ice`를 벗겨 만들던 것을 db 이름 기준으로 바꿈; 결과는 동일) |

- "숨김"은 기본 이름 앞에 `.`를 붙인 것이다. 별도 폴더로 옮기지 않는다.
- 폴더 안의 파일(`design.ovm/ovp/ovt/ovo`, `meta.json`)과 pack 내부 포맷
  (`FLOEICE` 매직, 레이아웃 버전 4)은 그대로다. 바뀐 것은 이름뿐이다.
- 명시 outdir(`floe-index vfs <src> <outdir>`, `floe-index drc <db> <out>`)은 어떤
  이름이든 된다. 규칙은 outdir을 생략했을 때와 Python이 경로를 계산할 때 적용된다.

## 2. 구 이름의 자동 개명 (마이그레이션)

아래 자동 개명은 GTK/Python 비교 셸의 동작이다. `feature/webui`의 Rust 앱은
새 기본 이름과 구 이름 읽기를 지원하지만, 읽기 권한으로 개명하지 않는다.
2026-09-17 사용자는 명시 Index에서만 개명을 승인했다. 후속 구현·검증 전까지
Index도 구 이름을 재사용하며, 열기/조회에서는 계속 개명하지 않는다.
[웹 통합 차이와 검증 상태](WEBUI_JOBDECK_SYNC.ko.md)를 함께 본다.

- `cachepath.find_vfs_cache(src)`: `.<src>.ice/meta.json`이 있으면 그것, 없고
  `<src>.floe/meta.json`이 있으면 **그 폴더를 `.<src>.ice/`로 `os.rename`** 한 뒤
  돌려준다. `Cache(src)`(뷰어 열기, jobdeck 소스 probe, `floe2 info` 등),
  `floe2 index`, `_cache_ready`(`floe2 view`의 사전 검사)가 모두 여기를 거치므로
  구 캐시는 처음 만지는 순간 개명되고 재색인은 없다(실덱 667 소스 약 40분 절약).
- `cachepath.find_pack(db)`: `.<db>.tray`가 없고 `<db>.ice`가 v2 pack(매직·버전)이면
  개명. 폐기된 v1 사이드카는 개명하지 않고 `drc.load_db`가 예전처럼 안내한다.
- 개명은 캐시·pack 안에 자기 이름이 없기 때문에 안전하다: `meta.json`은 소스의
  경로·size·mtime, `design.ovm`·`design.ovo`·pack 헤더는 size·mtime만 담는다.
  덱 스펙은 열 때마다 임시 폴더에 새로 쓴다.
- 개명이 불가능하면(읽기 전용 폴더, 다른 프로세스와의 경쟁) 구 이름을 그대로
  읽고 stderr에 프로세스당 한 번 알린다. 킬 스위치 `FLOE_CACHE_MIGRATE=off`는
  개명을 막고 구 이름을 그대로 읽는다(진단용, 기본이 아님).
- 읽기 전용 결과 폴더의 waive/note 임시 폴백 파일(시스템 temp, 경로 해시)은
  이제 db 경로로 해시한다. `<db>.ice` 시절의 temp 폴백은 이름이 달라져 다시
  쓰이지 않는다(결과 폴더 옆의 정식 사이드카는 이름이 같아 그대로 이어진다).

## 3. 의존 지점 (개명 커밋에서 정리한 곳)

- 경로 계산: `floe/cache.py` `cache_dir_for`, `floe/cli.py` `_cmd_index_legacy`·
  `_run_rust_index`·`cmd_index`·`_cache_ready`, `floe/gui.py` `_vfs_index_and_load`·
  `_drc_open_db`·notes/waive save-as 기본 이름, `floe/drc.py` `load_db`·사이드카
  네 함수 — 전부 `floe/cachepath.py`를 부른다. `rust/cli/src/vfs.rs`(기본 outdir),
  `rust/cli/src/drcice.rs`(기본 out), usage 문자열.
- 로드 대화상자 `BROWSE_HIDDEN_SUFFIXES = (".floe", ".ice")`는 **구 이름 잔재를
  숨기는 용도로만** 남는다(새 이름은 점 파일 규칙으로 숨겨진다).
- `.gitignore`: `.*.ice/`, `.*.tray`.
- 게이트: `validate_index_cli`(기본 경로 = 숨김 형제, 구 이름 자동 개명·재색인
  없음·stderr 알림), `validate_drc_ice`(`validate_names`: 기본 pack 이름, 구 이름
  개명, 사이드카 이름이 pack 이름과 무관, `FLOE_CACHE_MIGRATE=off`), Rust
  `vfs::naming_tests`, `validate_jobdeck`·`validate_occupancy`·`validate_floe2`·
  `validate_rust.sh`는 `cachepath`로 경로를 얻는다. 테스트가 명시 outdir로 쓰는
  `*_rust.ice`, `textmini.oas.floe` 같은 이름은 규칙과 무관하다.
- 로컬 픽스처(`data/m1/valmini.oas.floe` 등, gitignore)는 처음 열 때 자동 개명된다.
- 현장 명령: 숨김 이름은 `ls`·`find -name '*.ice'`에 안 잡힌다. `find . -name
  '.*.ice' -type d`, `du -sh .*.ice`처럼 점을 붙인다(tcsh도 같다).

## 4. 이력과 결정

1. `<src>.ice/` — 최초 KLayout 시절 타일 캐시 폴더. 2026-08-13 `.tiles`로 개명.
2. `<db>.ice` — 2026-08-14부터 DRC 결과 pack(v1 사이드카 → 2026-08-19 v2 pack).
3. `<src>.floe/` — Rust VFS 캐시(2026-09-16까지).
4. **2026-09-16 개명**(사용자 결정 2026-09-15, 구현 feature/jobdeck): `.ice`가 다시
   인덱스 폴더의 postfix(숨김), DRC pack은 `.tray`(숨김). 옛 문서·로그의 `.ice`는
   날짜로 뜻을 구분한다. 같은 커밋에서 점유 요약의 색인 기본값도 바꿨다: jobdeck의
   소스는 기본 on, 레이아웃은 `--occupancy` opt-in(OCCUPANCY_PLAN §12).
   결정 항목의 결론: 구 캐시는 발견 시 개명(§2), `.tiles` 폴백은 동결 셸용으로
   유지, 사이드카는 db 이름 기준, 브라우저 접미사 목록은 잔재용으로 유지.

`grep -rn -E '"\.floe|\.floe/|"\.ice"' floe rust/cli/src tools`로 남은 접미사
사용을 찾을 수 있다(제품명 'floe'가 든 임시 파일 접두어 `.floe-jobdeck-`,
`.<name>.floe-shot-`, CSS 클래스 `.floe-*`는 무관).
