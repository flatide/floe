# 캐시·pack 파일 이름 규칙 (2026-09-16 개명 완료)

정본 코드: app-core `rust/app-core/src/cache/names.rs`(floe2·floe2-web·GTK 서비스가
쓰는 모든 파생 이름; Python이 빠진 뒤 유일한 규칙), `rust/cli/src/vfs.rs`
`default_outdir`/`hidden_sibling`(floe-index를 단독으로 돌릴 때의 기본값), 두 곳이
함께 쓰는 작업 디렉터리 규칙 `rust/vfs/src/workdir.rs`(§6), 동결 오라클
`tools/oracle/floe_oracle/cachepath.py`(게이트의 참조). 이 밖에는 어디에도
접미사를 쓰지 않는다. 이력·결정 과정은 §4, 소스 옆이 아닌 작업 디렉터리에 두는
`--workdir`는 §6.

## 1. 규칙

| 대상 | 이름 | 예 | 2026-09-16 이전 |
|---|---|---|---|
| 레이아웃 인덱스 폴더 (VFS 캐시, `floe-index vfs` 산출) | **`.<src>.ice/`** — 소스와 같은 디렉터리의 숨김 폴더 | `chipA.oas` → `.chipA.oas.ice/` | `chipA.oas.floe/` |
| Calibre DRC 결과 pack (`floe-index drc` 산출) | **`.<db>.tray`** — db와 같은 디렉터리의 숨김 파일 | `results.db` → `.results.db.tray` | `results.db.ice` |
| 색인 잠금 (2026-10-09, §5) | **`.floe-lock/`** — 대상과 같은 디렉터리의 숨김 폴더, 대상마다 `<키>.build`·`<키>.use` | `.chipA.oas.ice/` → `.floe-lock/chipA.oas.vfs.build` | — |
| DRC 리뷰 사이드카 (리뷰어별 waive·note) | `.<db>.waive.<user>`, `.<db>.notes.<user>.fe` — **db 이름** 기준 | `.results.db.waive.jkim` | 같음 (pack 이름에서 `.ice`를 벗겨 만들던 것을 db 이름 기준으로 바꿈; 결과는 동일) |

- "숨김"은 기본 이름 앞에 `.`를 붙인 것이다. 별도 폴더로 옮기지 않는다(`--workdir`가 있으면
  이름은 같고 폴더만 작업 디렉터리 아래의 미러다, §6).
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
  숨기는 용도로만** 남는다(새 이름은 점 파일 규칙으로 숨겨진다). 같은 브라우저는
  심볼릭 링크를 따라가지 않고 자기 이름이 필터를 통과하는 링크만 `이름@` 행으로
  나열한다(2026-10-21, 2026-10-26; SPEC-VIEWER의 "파일 열기 대화상자" 항목).
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
  - 0.12.359부터 CLI의 `--wait[=SECONDS]`(`FLOE_LOCK_WAIT`)는 75로 끝내지 않는다.
    탐침으로 앞선 실행이 끝나기를 기다린 뒤 다시 시도한다. 잠금 자체는 그대로
    거절한다(SHARED_APP_LAYER §12).
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

## 6. 작업 디렉터리 `--workdir DIR` (2026-10-31, app 0.12.427)

사용자(2026-10-31): "--workdir로 .ice, .tray, .sqlite등의 파일을 읽고 쓰는 위치를
지정할 수 있게 해줘."

- **옵션:** `--workdir DIR`은 floe2의 모든 명령(`index`·`info`·`render`·`probe`·`clip`·
  `jobdeck`·`drc`·`drc-prepare`·`svrf`·`view`·`gtk-service`)이 받고, 명령 앞이든 뒤든 줄
  어디에 있어도 된다(`floe2 --workdir /ssd/w render x.oas …`, `floe2 index x.oas --workdir=/ssd/w`).
  기본값은 환경 변수 `FLOE_WORKDIR`(빈 값은 없는 것)이고 옵션이 환경보다 우선한다. `~/`는
  셸처럼 풀고, 상대 경로는 현재 디렉터리 기준이다. DIR이 없으면 만들고(`mkdir -p`), 파일이거나
  만들 수 없거나 `/`이면 명령행 오류로 2로 끝난다. floe-index도 같은 옵션과 `FLOE_WORKDIR`을
  받아(`floe-index [--workdir DIR] light x.oas`, `floe-index drc r.db`, 줄 어디서나) 기본
  `.<file>.ice`·`.<db>.tray`를 같은 자리에 둔다. 명시한 outdir·`--out DIR`·`[out.tray]`는
  옮기지 않는다. 구현은 한 곳 `rust/vfs/src/workdir.rs`(`workdir()`·`set`·`under`·
  `lexical_absolute`·`take_option`·`apply`)이고, floe-app-cli `workdir.rs`와 floe-index의
  main이 명령 파서 앞에서 옵션을 떼어내며, app-core `cache/names.rs`와 floe-index
  `hidden_sibling`이 그 규칙으로 이름을 놓는다.
- **규칙:** 파생 이름은 §1 그대로이고, 놓이는 폴더만 DIR 아래 **소스의 절대 경로 자리**다.
  소스 `/proj/a/x.oas`의 캐시는 `/proj/a/.x.oas.ice/`가 아니라 `DIR/proj/a/.x.oas.ice/`다.
  이름에서 만들어지는 것은 전부 따라간다.

  | 대상 | `--workdir DIR`일 때의 위치 |
  |---|---|
  | 레이아웃·덱 소스의 캐시 `.x.oas.ice/`(전체 인덱스, 경량 캐시, 요약), 구 이름 `x.oas.floe/` | `DIR/<소스 폴더>/.x.oas.ice/` |
  | 캐시의 revisions `.x.oas.ice.revisions/` | 캐시 옆, 즉 DIR 아래 |
  | DRC pack `.r.db.tray`, 구 이름 `r.db.ice` | `DIR/<db 폴더>/.r.db.tray` |
  | pack의 `.r.db.analysis/`(drc-prepare; `FLOE_DRC_ANALYSIS_ROOT`가 있으면 그쪽) | pack 옆, 즉 DIR 아래 |
  | 리뷰 `.r.db.review.<user>.sqlite3`(`FLOE_DRC_REVIEW_ROOT`가 있으면 그쪽), `.r.db.waive.<user>`, `.r.db.notes.<user>.fe` | pack 옆, 즉 DIR 아래 |
  | 잠금 `<캐시>.index.lock`, `.floe-lock/` | 캐시·pack 옆, 즉 DIR 아래 |
  | 출력(`--out`의 PNG·보고서·clip OASIS), 입력(`<db>.clusters`, `--drc-rules`) | 바뀌지 않는다. `.clusters`는 db 옆에서 먼저 찾고 없으면 DIR의 미러에서 찾는다 |

- **소스 옆은 읽지도 쓰지도 않는다.** 작업 디렉터리에 캐시가 없으면 소스 옆에 캐시가 있어도 다시
  색인한다(옆의 캐시를 쓰려면 `--workdir` 없이 돌린다). 같은 캐시를 여럿이 공유하려면 모두가 같은
  DIR을 준다. 잠금 키는 경로 기준이라 DIR의 캐시와 옆의 캐시는 서로 다른 잠금이고, 같은 DIR의 같은
  소스는 예전처럼 서로 막는다(§5).
- **자식 프로세스:** floe2는 받은 DIR을 `FLOE_WORKDIR`로 내보내므로 floe-index·floe-renderd·GTK
  뷰어와 `gtk-service`가 같은 자리를 쓴다. floe-index도 덱이나 drc-prepare를 floe2에 넘길 때 같은
  변수를 넘긴다. floe2가 floe-index를 부를 때는 캐시 폴더를 명시해 넘기므로 floe-index 쪽 규칙은
  단독 실행에서만 쓰인다.
- **접근 범위:** 캐시·pack을 쓰는 명령의 AccessScope(`registered.rs`)에 작업 디렉터리가 자동으로
  들어간다. 웹 호스트가 등록 루트 밖의 DIR을 주어도 거기에 쓸 수 있다.
- **`--temp-index`와 함께:** 작업 디렉터리에 캐시가 있으면 그것을 열고, 없으면 임시 폴더에
  만들어 끝나면 지운다(임시 폴더의 해시 이름 규칙은 그대로).
- **왜 미러인가:** 임시 인덱스의 `<경로 해시>/.x.oas.ice`는 실행과 함께 사라지지만 작업
  디렉터리는 사람이 들여다보고 지운다. 미러는 같은 이름의 소스가 다른 폴더에 있어도 겹치지 않고,
  `ls DIR/proj/a`로 무엇의 캐시인지 보이며, 프로젝트 단위로 `rm -rf DIR/proj/a`가 된다.
  캐시의 `meta.json`이 소스를 적으므로 캐시가 소스 옆에 없어도 읽는 쪽은 소스를 찾는다
  (임시 인덱스 0.12.416과 같은 성질).
- **검증:** 게이트 `floe2`(tools/validate_floe2.py의 workdir 블록: 미러 위치, 소스 옆 무변화, 잠금
  위치, `--workdir=`·앞자리·`FLOE_WORKDIR`, `--no-index` 거절, render, 잘못된 값의 2, `view`의
  전달, 새 작업 디렉터리에의 첫 pack 빌드와 `--rules` 읽기, 새 작업 디렉터리로의 캡처 자동 색인,
  floe-index 단독의 `light --workdir`·`--workdir vfs`·`FLOE_WORKDIR`의 `drc`·명시 `--out` 유지·값 없는
  `--workdir`의 2)와 단위 테스트 floe-vfs workdir.rs, app-core names.rs
  `a_work_directory_mirrors_the_absolute_path_under_itself`, app-cli workdir.rs.
  잠금은 2026-10-31 손으로 확인했다: 미러의 `.floe-lock/<키>.build`를 쥔 작성자가 `--workdir` 재색인을
  75로 막고(`FLOE_WORKDIR`로도 같다), 통째 재색인 중이면 미러의 캐시를 여는 render도 75, 미러의 캐시를
  읽는 등록(`use.<host>.<pid>.<n>`)이 `--force`를 75로 막고, `--wait`는 미러의 작성자를 기다린 뒤
  다시 만들며, 미러의 `.index.lock`을 쥔 쪽은 "another Rust application" 75다. 소스 옆에 쥔 잠금은
  `--workdir` 실행에 아무 영향이 없고(다른 키), 옆으로 돌린 같은 실행은 막힌다. pack의
  `.floe-lock` 작성자 잠금은 floe-index가 스테이징 파일 경로로 잡는 기존 동작이라 작업 디렉터리와
  무관하게 `.tray` 이름의 키를 쓰지 않는다(lease `.tray.index.lock`은 미러에 놓인다).
