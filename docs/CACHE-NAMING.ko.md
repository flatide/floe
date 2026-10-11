# 캐시·pack 파일 이름 규칙 (2026-09-16 개명 완료)

정본 코드: `floe/cachepath.py`(Python 쪽 유일한 이름 규칙), `rust/cli/src/vfs.rs`
`default_outdir`/`hidden_sibling`(Rust 기본값). 이 두 곳 외에는 어디에도
접미사를 쓰지 않는다. 이력·결정 과정은 §4.

## 1. 규칙

| 대상 | 이름 | 예 | 2026-09-16 이전 |
|---|---|---|---|
| 레이아웃 인덱스 폴더 (VFS 캐시, `floe-index vfs` 산출) | **`.<src>.ice/`** — 소스와 같은 디렉터리의 숨김 폴더 | `chipA.oas` → `.chipA.oas.ice/` | `chipA.oas.floe/` |
| Calibre DRC 결과 pack (`floe-index drc` 산출) | **`.<db>.tray`** — db와 같은 디렉터리의 숨김 파일 | `results.db` → `.results.db.tray` | `results.db.ice` |
| 색인 잠금 (2026-10-09, §5) | **`.floe-lock/`** — 대상과 같은 디렉터리의 숨김 폴더, 대상마다 `<키>.build`·`<키>.use` | `.chipA.oas.ice/` → `.floe-lock/chipA.oas.vfs.build` | — |
| DRC 리뷰 사이드카 (리뷰어별 waive·note) | `.<db>.waive.<user>`, `.<db>.notes.<user>.fe` — **db 이름** 기준 | `.results.db.waive.jkim` | 같음 (pack 이름에서 `.ice`를 벗겨 만들던 것을 db 이름 기준으로 바꿈; 결과는 동일) |

- "숨김"은 기본 이름 앞에 `.`를 붙인 것이다. 별도 폴더로 옮기지 않는다.
- 폴더 안의 파일(`design.ovm/ovp/ovt/ovo`, `meta.json`)과 pack 내부 포맷
  (`FLOEICE` 매직, 레이아웃 버전 4)은 그대로다. 바뀐 것은 이름뿐이다.
- 명시 outdir(`floe-index vfs <src> <outdir>`, `floe-index drc <db> <out>`)은 어떤
  이름이든 된다. 규칙은 outdir을 생략했을 때와 Python이 경로를 계산할 때 적용된다.

## 2. 구 이름의 자동 개명 (마이그레이션)

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

## 5. 잠금 `.floe-lock/` (2026-10-09, app 0.12.323 / RENDERD 0.12.297)

사용자(2026-10-09): "동일 파일을 먼저 인덱싱하고 있거나 사용하고 있는 경우에는 다른
사용자가 실행하더라도 리젝될 수 있어야 함."

- **정본 코드:** `rust/vfs/src/lock.rs`(floe-index·renderd)와 `floe/indexlock.py`
  (Python 쌍둥이). 두 곳은 같은 이름·형식·권한 규칙을 쓴다.
- **잠금 수단:** 커널 권고 잠금 flock이다. 기다리지 않는다. 쥔 프로세스가 끝나거나
  kill되면 커널이 푼다. Linux NFS는 flock을 서버의 POSIX 잠금으로 흉내 내므로
  호스트 사이에서도 맞물린다.
- **키:** 원본 이름과 종류로 정한다. 구 이름도 같은 키를 쓴다. 경로는 실제 경로다
  (심볼릭 링크를 풀고, 없는 대상은 폴더만 풀어서; Rust `real_path`, Python
  `_real_path`). 0.12.325부터, 링크로 가리킨 캐시도 같은 잠금이다.

  | 대상 | 키 | 같은 키를 쓰는 구 이름 |
  |---|---|---|
  | `.X.oas.ice/` | `X.oas.vfs` | `X.oas.floe/` |
  | `.X.db.tray` | `X.db.pack` | `X.db.ice` |
  | `deck.cal.rules.json` | `deck.cal.rules` | — |

  명시 outdir·out은 그 이름 뒤에 `.vfs`·`.pack`·`.rules`를 붙인다.
- **파일:**
  - `<키>.build`: 그 대상을 쓰는 모든 실행이 배타로 쥔다. 내용은 쥔 사람이다
    (`who`·`from`·`host`·`pid`·`since`·`what`).
    - `who`는 `FLOE_REVIEWER`, 없으면 계정이다. 공용 계정이라 계정만으로는 누구인지
      모른다.
    - `from`은 DISPLAY 호스트나 SSH 클라이언트다.
  - `<키>.use`: 읽는 쪽(renderd, vfsd, DRC 팩 리더)이 공유로, 통째로 다시 만드는
    실행이 배타로 쥔다. 배타는 커밋 표시까지다.
  - `use.<host>.<pid>.<n>`: 읽는 쪽의 등록이다(프로세스·폴더마다 하나, 쥔 키 목록).
    - 주인이 배타로 잠근다. 임시 이름으로 쓰고 잠근 뒤 rename한다.
    - 잠글 수 있는 등록은 죽은 주인의 것이다. 무시하고, 지울 수 있으면 지운다.
    - **이름을 대는 데만 쓴다**(0.12.325). 거절 여부는 커널 잠금만 정한다. NFS의
      디렉터리 캐시는 다른 호스트의 새 파일을 늦게 보인다.
    - 권한은 umask와 무관하게 0644로 명시한다. 이 계정이 읽을 수 없는 등록도
      사용자로 센다. 이름에서 host·pid를 읽어 "someone on <host>, pid N"으로 댄다.
- **지우지 않는다.** `.build`·`.use`를 새로 만들면 두 번째 잠금이 되어, 실행 중인
  쪽과 서로 못 본다. 운영 규칙: **`.floe-lock`을 지우지 말 것.**
- **권한:** 놓인 폴더의 group/other 쓰기 비트와 setgid를 따르고, 공유 폴더면 sticky
  비트를 둔다. 폴더보다 넓히지 않는다(0775 폴더 → 01775 / 0664, 0755 → 0755 /
  0644).
  - 배타 잠금은 읽기쓰기로 연다. NFS의 배타 잠금은 쓰기 권한이 필요하다.
  - 읽는 쪽은 읽기 전용으로 연다.
- **대비책:**
  - 읽는 쪽은 잠금을 만들 수 없는 폴더(읽기 전용)이거나 열린 파일 한도를 넘으면
    잠금 없이 연다.
  - 쓰는 쪽은 잠금 파일을 읽기쓰기로 열 수 없으면 종료 1과 고치는 법을 알린다.
  - flock 미지원(ENOLCK·EOPNOTSUPP·ENOSYS)이면 경고 한 줄을 남기고 잠금 없이
    진행한다.
  - NFS `nolock`·`local_lock=` 마운트면 처음 한 번 "이 호스트 안에서만"이라고
    경고한다(`/proc/mounts`).
  - 킬 스위치 `FLOE_LOCK=off`.
- 누가 무엇을 쥐는지(명령별)는 SPEC-INDEXER §4.5, 뷰어의 동작은 SPEC-VIEWER를 본다.
  게이트는 `index_lock`이다.
