# 웹 전환 M0 — 기능 대조표와 착수 게이트

작성 2026-09-12. 기준: `feature/webui`, `6c33a48` (`feature/jobdeck` 분기점).
상위 계획: [WEBUI_PLAN.ko.md](WEBUI_PLAN.ko.md).
서비스 계약 초안: [WEBUI_SERVICE_API.ko.md](WEBUI_SERVICE_API.ko.md).

**로컬 조사/설계 결과이며 이관 완료표가 아니다.** 아래 Rust 서비스와 웹 UI는
아직 전체 미구현이다. 2026-09-13에 M1a-1 worker client와 M1a-2a/b 일반 index·info/render/probe
경로를 추가했으며 §8~9에서 추적한다. 기존 Rust parser/VFS/raster/occupancy 구현과 새 서비스의
완료 상태를 구별한다. 실칩 jobdeck/occupancy 판정은 실측 브랜치에서 계속한다.

## 1. 기준과 조사 방법

- `floe2/cli.py` → `floe/cli.py:main(..., rust_only=True)`의 실제 argparse
  객체를 명령 실행 직전에 읽었다. 공개 subcommand **10개**, 공통 help를
  제외한 subcommand 옵션 action **93개**다. 별칭은 한 action으로 센다.
- 실제 기본값은 argparse만으로 결정하지 않는다. `cmd_view`,
  `RustRenderWorker`, `DeckRenderWorker`, `gui.py`까지 확인했다.
- 이전 `SPEC-VIEWER.ko.md`, `RUST_RENDERER.md`의 KLayout 시절 설명이나
  과거 refinement/쿼리 cap 수치는 현재 코드보다 우선하지 않는다. 덱의
  hierarchy frames도 오래된 소개 주석과 달리 현재 frames-only pass로 지원한다.
- 아래 표의 단계는 **목표 단계**다. 일반 index의 부분 구현만 §9에 기록했다.
  앞으로 행별로 구현 커밋·회귀 게이트·실행 결과를 붙여 완료로 바꾼다.
- CLI 이름/옵션/단위/기본값/오류/종료 코드와 JSON·파일 형식을 계약으로 삼는다.
  시간·PID·임시 경로만 정규화해 비교하고, 결정적 픽셀/clip은 기존 오라클을 쓴다.
  알려진 결함이나 무효 옵션까지 새 기능인 것처럼 복제하지 않고 §4에서 추적한다.

## 2. CLI 대조표

모든 명령의 `-h`/`--help`, 최상위 `--version`을 유지한다. 인자 없는 실행은
빈 뷰어 열기/기존 뷰어 앞으로 가져오기다. 개발 중 새 Rust 실행 파일 이름은
기존 Python `floe2`와 충돌하지 않게 분리한다(API 문서 §1).

### 2.1 index — M1a (17개 옵션)

필수 위치 인자 `src`: 레이아웃 또는 `.jb`. 담당: index/cache 서비스.

| 옵션 | 현재 의미와 보존할 조건 |
|---|---|
| `--level N[,N...]` | 덱 선택 레벨에 필요한 소스만 색인. 생략 시 전체. 이 명령에는 `--id` 별칭 없음 |
| `--force` | 기존 캐시 교체 허용. 생략 시 current 재사용, stale/incomplete는 거부. 자동 파괴적 재색인 금지 |
| `--jobs N` | 양수, 기본 12. 덱 전체의 동시 소스 수와 소스 내부 jobs를 곱하지 않기 |
| `--page-target-mb N` | 양수 MiB, 생략 시 native 기본(현재 1 MiB) |
| `--occupancy`, `--occupancy-only` | 상호 배타. 전자는 색인+요약/기존 캐시에 추가, 후자는 fresh 캐시에 요약만 생성·교체 |
| `--occupancy-um UM` | 양수, 요약 생성 요청을 포함. 생략 시 기준 셀 4 µm. occupancy-only와 병용 시 원본 캐시 보존 |
| `--lod`, `--no-lod` | 생성은 기본 off, `--lod` opt-in. 덱 소스 색인에도 전달. occupancy-only에서 OVM/OVP 재생성 금지 |
| `--slow-cell-s S` | 0 이상, native 기본 5초. 0은 모든 셀 기록 |
| `--p2-shard-limit-mb N` | 0 이상, shard 복사 한도 전달 |
| `--profile-cell NAME`, `--profile-cell-ci N` | 상호 배타. 특정 셀 계획 계측, 일반 인덱스 산출물 쓰기 없음 |
| `--profile-jobs LIST`, `--profile-repeat N` | 양수 jobs 목록/반복 수(기본 1); 결과 순서·계측 필드 보존 |
| `--profile-snapshot FILE`, `--profile-snapshot-refresh` | parse/prepare 재사용용 명시 파일. fingerprint/버전/옵션 검사, refresh만 재생성 허용 |

필수 회귀: 공백·한글 경로, 잘못된 바이너리 override, source별 중복 제거,
`--level`/`--lod --occupancy` 조합, occupancy-only 전후 OVM/OVP 불변,
불완전 marker/버전 불일치, Ctrl+C와 실패한 요약 임시파일 정리, progress 스트리밍.
덱 source 순차 처리와 각 source의 jobs는 별개다. 공유 서버 자원 정책은 API §7.

### 2.2 info — M1a (1개 옵션)

`src`, `--level`. top cell, DBU, bbox, 레이어 이름/별칭/개수, 캐시 정보를 유지.
덱에서는 선택·소스·배치·skip ledger와 INCOMPLETE 정보를 보존한다.
CLI의 사람용 출력과 HTTP용 구조화 DTO는 같은 서비스 결과에서 별도로 직렬화한다.

### 2.3 render — 기본 M1a, batch/DRC/export 전체 M4 (29개 옵션)

필수 위치 인자 `src`. headless PNG export는 브라우저나 GTK를 실행하지 않는다.

| 옵션 | 현재 기본값/조건 |
|---|---|
| `--level` | 덱 선택 레벨 |
| `--bbox`, `--at`, `--size`, `--anchor`, `--stretch` | 영역 지정/중심·좌하단 anchor(`center` 기본)/종횡비. 단위 suffix 처리와 조합 오류 보존 |
| `--layers` | 이름·별칭 또는 `L/D` 목록. all/none 구분과 순서 보존 |
| `--px`, `--out` | 기본 `1200`(W 또는 WxH), `view.png` |
| `--mosaic-at`, `--corners` | 다점 mosaic 및 코너 캡처 |
| `--line`, `--line-color`, `--keep-tiles` | mosaic 구분선 기본 2.0 / `#ffffff`, 중간 PNG 보관 opt-in |
| `--batch`, `--report` | batch 파일/`-` stdin, JSON 결과 보고서 |
| `--depth`, `--detail` | depth 생략/full, 0=top. detail 기본 `exact`, 그 외 low/medium/high |
| `--thin` | `auto/keep/cull`, 생략과 명시 auto는 구별. auto=layout cull / deck keep |
| `--frames`, `--labels`, `--label-font-px` | 두 표시 기본 off, 폰트 14px(6..96). 덱 capability에 따라 제한 |
| `--drc`, `--drc-rule`, `--drc-err` | overlay DB/룰/오류 선택. err 기본 `all` |
| `--drc-cap`, `--drc-frac`, `--drc-rules` | 기본 200 / 0.3 / 미지정. 명시 오류 번호·범위와 자동 all의 cap 의미 구별 |
| `--floe-reviewer` | 공백 제거 후 빈 이름 거부, 환경의 reviewer보다 우선 |

`floe/shots.py`의 좌표→캡처 영역, batch 우선순위, mosaic 조립, report,
`floe/fe_embed.py`의 PNG metadata 연계까지 포함한다. 한 장 render만 옮기고
CLI 전체가 Python-free라고 판정하지 않는다. 파일 교체는 최종 성공 시 원자적으로.

M4b-2: `--batch`/stdin, `--mosaic-at`/`--corners`, 구분선·kept tiles·JSON report를
layout/jobdeck 공통 Rust capture runner로 연결했다([M4 §8](WEBUI_M4.ko.md)).
Python 픽셀/report·jobs1/8 결정성과 단일 worker/실패·취소 보존을 검증한다.
파일별 원자 게시이며 batch/여러 kept tile의 일괄 트랜잭션은 아니다.
PNG metadata/보조 CLI는 M4b-3, DRC 오류별 marker/CD/legend 캡처는 M4b-4로
연결했다([M4 §10](WEBUI_M4.ko.md)). DRC 캡처는 live 스타일·frames/labels on·
기본 cut0이며 일반 archival shot과 구별한다. 웹 내보내기/UI는 아직 별도다.

### 2.4 clip — M4 (5개 옵션)

`src`, 필수 `--bbox`, 선택 `--layers`, `--out`(clip.oas),
`--cell-name`(FLOE_CLIP), `--exact`(Rust는 이미 exact인 호환 플래그).
현재 jobdeck worker는 clip 미지원. 레이아웃은 cut/full-depth 표시 정책과
독립된 exact geometry 출력이며 기존 Region XOR/바이트 결정성 게이트 유지.

M4b-1: `floe2-web clip`의 위 옵션은 Rust로 연결했다([M4 §7](WEBUI_M4.ko.md)).
µm→DBU는 기존 CLI와 같은 nearest/ties-even, 역방향 bbox 정규화다.
render와 달리 clip의 빈 레이어 토큰 목록은 기존처럼 전체 레이어를 선택한다.
`--cell-name`은 1~4096 UTF-8 bytes/control 없는 이름으로 제한하며,
NaN/무한/DBU overflow/반올림 후 0면적은 명시 오류다.
M4c-1/2에서 관리형 export와 owner 표시 receipt·승인/취소·다운로드 API를 연결했다
([M4 §12~13](WEBUI_M4.ko.md)). M4c-3은 현재 viewport의 준비/승인·취소·파일 목록과
다운로드 UI를 연결한다(§14). API의 명시 `none`은
CLI의 빈 토큰 목록과 달리 빈 레이어 선택을 유지한다.

### 2.5 probe — M1a (공개 옵션 없음)

`src`. GUI 없이 동일 worker를 열어 fit/근접 요청을 실행하는 진단.
Rust 전환 후 handshake/open/frame/error/timeout/종료까지 headless로 검사한다.
성공과 무관한 stderr 로그, 배경 margin/preview, 최종 프레임을 구별한다.

### 2.6 drc — 조회 M2, 쓰기 연계 M4 (4개 옵션)

`db`, `--list`, `--rules`, `--errs RULE`, `--floe-reviewer`.
ASCII DB와 fresh ICE pack 선택, rule/error JSON, declared/실제 count,
좌표 단위·waive 표시를 보존한다. 기존 `rust/cli/src/drcice.rs`, `drcpack.rs`의
Rust 구현은 재사용 대상이나 Python `IcePack`과 GUI의 읽기·쓰기 전체 대체는 아니다.
M2a-11a에서 CLI의 fresh ICE 우선/readonly ASCII fallback과 소수 좌표·목록·JSON을
이관했다([M2 §20](WEBUI_M2.ko.md#20-m2a-11a-읽기-전용-ascii-drc-코어cli-fallback)).
M2a-11b에서 명시 ASCII 웹 등록·조회·윤곽/CD를 연결했다([M2 §21](WEBUI_M2.ko.md#21-m2a-11b-웹-ascii-등록조회윤곽cd)).
M2a-12a는 명시 `drc --build [--force] [--jobs N]`와 공통 생성/취소 코어다.
M2a-12b1에서 owner HTTP 승인·생성/취소·새 identity 재등록 서버를 연결했다.
M2a-12b2에서 pack-build 승인/진행/취소·새 identity 조회 UI를 연결했다.
실제 브라우저 승인 클릭 수용과 review 쓰기는 별도로 남아 있다(M2 §24).
M4e-1은 waive 스트리밍 codec과 메모리 주석 그룹 편집/FE 포맷을 Rust로 이관했다
([M4 §21](WEBUI_M4.ko.md)). Python byte oracle과 손상/취소 게이트를 추가했으며,
실제 autosave·동시 수정 충돌·review 쓰기 API/UI는 아직 연결하지 않았다.
M4e-2a는 로컬 저장의 snapshot 충돌·원자 게시·pack binding과 legacy 확인 경계를 추가했다
([M4 §22](WEBUI_M4.ko.md)). 웹 actor/자동 저장·전체 import/export 연결은 여전히 남는다.
M4e-2b는 준비부터 게시 완료까지 admission/read lease를 유지하고 취소·결과 조회·join을
제공한다([M4 §23](WEBUI_M4.ko.md)). owner의 쓰기 API/UI는 아직 연결하지 않았다.
M4e-2c는 기존 reader와 store의 opaque pack identity 일치 검사와 bounded 선택 waive
조회다([M4 §24](WEBUI_M4.ko.md)). 같은 내용의 다른 pack도 구별하며 쓰기 API/UI는 후속이다.
M4e-3a는 `view --drc-reviewer TAG`의 명시 opt-in과 owner 주석 read/prepare/승인 게시·
receipt/취소 API다([M4 §25](WEBUI_M4.ko.md)). 기본 비활성이고 reviewer/path는 wire로
고르지 않는다. 주석 UI·자동 저장·waive 쓰기·import/export·현장 수용은 남아 있다.
M4e-3b는 위 API의 선택 주석 편집·미리보기·명시 승인 패널이다
([M4 §26](WEBUI_M4.ko.md)). 선택 변경 시 초안 문구 보존, 동일 승인만 재전송하는
복구 UI를 제공한다. 자동 저장·주석 badge/overlay·waive 쓰기·import/export와
실제 브라우저 게시/현장 수용은 남아 있다. Chrome 합성 읽기·미리보기·만료/선택 변경
문구 보존·폐기·정상 종료는 후속 M4e-4a 작업 중 확인했다(주석 파일 무쓰기).
M4e-4a는 waive snapshot을 같은 geometry reader에 적용하는 내부 경로다
([M4 §27](WEBUI_M4.ko.md)). geometry cache를 유지하고 status/counter를 함께 교체한다.
M4e-4b는 이 갱신의 HTTP/필터·선택/prepared focus revision 장벽이다
([M4 §28](WEBUI_M4.ko.md)). geometry id/cache를 유지하되 조회 revision을 바꾼다.
M4e-4c는 별도 `--drc-edit-waives` opt-in의 owner read/prepare/승인 API다
([M4 §29](WEBUI_M4.ko.md)). 디스크 `published`와 `reader_applied`를 분리하고 게시한
동일 파일만 기존 reader에 반영한다. 기본 비활성·자동 저장 없음이며, 외부 파일 교체는
명시 reopen이 필요하다. M4e-4d는 위 API의 선택·action/preview·별도 승인·receipt UI다
([M4 §30](WEBUI_M4.ko.md)). 승인 전부터 이전 조회를 중지하고 reader 적용과 같은
revision의 ready catalog를 확인한 뒤 재개한다. 실제 브라우저 게시·현장 수용은 후속이다.
M4e-5a는 저장 주석의 최대512개 목록 badge와 이동 대상 하나의 본문을 읽는 owner API다
([M4 §31](WEBUI_M4.ko.md)). 유계 admitted snapshot을 재사용하고 편집 preview를
소비하지 않는다. M4e-5b에서 배지와 마지막 ACK 이동 대상의 본문 overlay를 연결했다
([M4 §32](WEBUI_M4.ko.md)). Markers/Tab·상태 복원·저장 장벽과 Chrome 합성 표시를
확인했다. 전체 import/export·실제 브라우저 게시·현장 수용은 후속이다.
M4e-6a는 주석/waive snapshot export와 전체 waive import의 native/managed 기반이다
([M4 §33](WEBUI_M4.ko.md)). bounded streaming·전체 교체 확인·입력/대상 변경 보호와
Python byte oracle을 추가했다. M4e-6b는 owner 전용 분할 업로드·전체 교체 준비와
artifact 다운로드 API 연결이다([M4 §34](WEBUI_M4.ko.md)). 기존 notes/waives opt-in과
명시 게시 승인을 재사용한다. M4e-6c는 전체 review 가져오기/내보내기 패널과 기존 저장
receipt/복구 연결이다([M4 §35](WEBUI_M4.ko.md)). Chrome 합성 export 준비와 세션 종료는
확인했으며 브라우저 업로드·review 게시·다운로드 파일 수용/현장 검증은 아직 남아 있다.
DRC 검사 CLI는 계속 read-only이며 review 편집 opt-in은 `view`에만 있다.

### 2.7 svrf — M4 (6개 옵션)

`deck`, `-o/--out`(기본 `<deck>.rules.json`), `--scan`,
`-D/--define`(반복), `-I/--include-dir`(반복), `--follow-verbatim`,
`--no-env-switches`.
현 parser의 subset/진단을 보존하고 임의 Tcl 실행기를 만들지 않는다.
CLI 환경 분기 호환과 서버의 include/환경 접근 통제는 구별한다(API §8).
M4b-5에서 Rust-only `floe2-web svrf`에 연결했다. JSON/scan은 기존 parser의
R1–R4와 비교하며, 빠진 root·비정상 숫자·자원 상한은 명시 오류다.
sidecar 원자 저장/입력 보호와 구체적 차이는 [M4 §11](WEBUI_M4.ko.md)을 따른다.

### 2.8 gtktest — M4에서 진단 대체 (공개 옵션 없음)

선택 위치 인자 `png`. GTK 전용 진단을 웹 제품에 끌고 오지 않는다.
전환 중 기존 명령은 GTK 비교 패키지에 남기고, Rust 셸은 명시적인 안내/오류를
제공한다. 웹용 PNG/raw/Canvas 표시 진단과 대체 명령 이름을 정한 뒤 폐기 승인.
M4f-1의 `selfcheck`는 native 설치 진단만 담당하므로 위 표시 진단/GTK 폐기 승인을
대체하지 않는다. 브라우저를 실행하지 않고 항상 desktop 수용 미검증을 표시한다.

### 2.9 view — M1b/M4 (21개 옵션)

선택 위치 인자 `src`. 빈 창→파일 선택, 미색인 파일→동의→색인→열기도 범위다.

| 옵션 | 실제 경로에서 보존할 조건 |
|---|---|
| `--level` | 덱 레벨 선택. 미지정 GUI는 질문, 환경 `FLOE_JOBDECK_LEVELS` 반영 |
| `--multi` | 독립 창/뷰 생성. 웹은 동일 worker에 독립 viewport를 섞지 않기 |
| `--goto` | X,Y[,W] µm. detail/depth와 한 번에 적용 후 첫 렌더. 포워딩도 같은 순서 |
| `--drc` | 결과 함께 열기. 현재 새 인스턴스 경로와 포워딩 차이 확인 필요 |
| `--detail`, `--depth` | 기본 medium; 일반 파일 depth 0, goto/DRC/덱은 full, 명시 depth 우선(0..999 정규화) |
| `--thin` | 생략=None, 명시 auto 포함 포워딩. resolved keep/cull은 모든 frame/margin/query key에 포함 |
| `--lod` | parser/UI 기본 on. **Rust worker render wire에는 lod 필드가 전달되지 않음**(§4) |
| `--refinement` | parser 기본 on이나 Rust 기본 round_pages=2^30, 실질 중간 렌더 off. 명시 off는 환경 round 설정보다 우선 |
| `--frame-cache` | on 기본. off는 frame 재사용 우회이지 decoded LRU 비활성화가 아님 |
| `--perf-baseline` | lod/refinement/frame-cache/frames/labels off. cold/warm page cache 자체는 유지 |
| `--frames`, `--labels`, `--label-font-px` | 표시 기본 on, 14px(6..96); GTK의 frames/labels 연동과 덱 capability를 별도 시험 |
| `--stream-kb`, `--stream-target-ms` | 0=refinement off; off와 nonzero 충돌 오류. target 기본 500(100..2000), Rust는 target 값을 사용하지 않음 |
| `--render-debug` | worker 진단. 외부 공유 로그에는 경로·원본 문자열 비노출 |
| `--hairline`, `--thin-um` | 프레임 정책 환경 override. 현재 이미 실행 중인 프로세스로는 소급되지 않음 |
| `--dump` | 진단 옵션. 웹에서는 범위를 명시하고 무효 옵션으로 조용히 수용하지 않기 |
| `--floe-reviewer` | 표시 태그이지 인증 주체가 아님. 공유 계정에서 임의 reviewer 이름이 쓰기 권한을 만들면 안 됨 |

단일 인스턴스: 현재 `(product, uid, DISPLAY)`와 프로세스 생성 옵션에 따라
포워딩/독립 실행을 고른다. 웹은 launcher의 세션 레지스트리와 `--multi`로
의도를 보존하되 DISPLAY를 인터넷 인증키로 사용하지 않는다. 웹 경로는
DISPLAY가 없어도 동작해야 한다(기존 GTK 오류까지 이식하지 않는다).

### 2.10 jobdeck — M1a (10개 옵션)

`deck`, `--sources`, `--level/--id`, `--mode`(level/chip/layer/identifier,
기본 level), `--colors`, `--ly-dt`(cross 기본/zip),
`--on-missing`(skip 기본/fail), `--lenient`, `--placements`, `--report`, `--spec`.

명령은 분석·report/spec 생성이다. `.jb`를 view/index/render로 여는 기능과
혼동하지 않는다. CLI의 과거 CHIP-block color와 GUI의 **레벨→TC 소스 칩**
행 모델도 별개다. 다음 게이트를 고정한다.

- 파서: 미정의 필드/행 번호/structural error, strict/lenient, 누락 TC ledger.
  실제 Calibre 비공개 포맷을 임의로 일반화하거나 미지원 옵션을 성공 처리하지 않기.
- 기하: AD/SF/좌표 변환·deck DBU·LY/DT·row/repetition·overflow 검증.
- 소스: 경로 해석, TC 이름, 선택 레벨만 색인, 중복 소스 재사용, cache freshness.
- GUI 행: LEVEL 타이틀/TC basename, 부모-자식 visibility, mode 변경 시 숨긴 칩
  재출현 금지. 전체 덱 기준 ordinal/color 고정으로 부분 로드에도 색 유지.
- 실패 의미: 분석 명령 missing skip은 ledger와 exit 3, fail은 exit 2.
  다른 덱 명령의 현 종료 코드는 별도 golden으로 고정하고 전부 3으로 바꾸지 않기.
- occupancy 사용 여부/미지원 레이어/누락 소스/partial을 서로 구별해 노출.

## 3. CLI 밖 기능과 폐기 경계

| ID / 목표 | 현재 코드 | 이관·검증 단위 |
|---|---|---|
| UI-01 / M1b | gui, viewport | 열기/레벨 선택/색인 동의, fit/goto/history, resize·wheel·drag·키 pan·zoom·depth/detail |
| UI-02 / M1b | gui margin 경로 | 16px fill 위상 pan, 착지 margin, crop, 라벨 잘림 시 crop 제외, 새 strip 검정 방지. 덱은 현재 미지원 |
| UI-03 / M1b→M4 | gui, fillpat, `.def` | 레이어 접기/선택/격리/전체 on/off·색·fill·width·mono, `.layerprops` load/save/설계 기본값. 이름/순서/alias 유지 |
| UI-04 / M4 | gui query/ruler | pick overlap 순환, snap, 측정·삭제·clear, Escape, 선택 레이어 추적. summary/미완료 scene에서 질의 제한 표시 |
| UI-05 / M4 | gui | 단축키/포커스/IME 입력·한글 주석, copy view PNG, overlay all/errors/none, quit/닫기/복원 |
| DRC-01 / M2 | drc, gui DRC 패널 | 대형 결과 lazy paging, 검색·룰/타입/waive 필터·선택/box-select·다음/이전·goto·layer isolate·측정 overlay |
| DRC-02 / M4 | drc, gui 저장 핸들러 | waive/unwaive·note·import/export·자동 저장·reviewer 구분·동시 수정 충돌. DB 교체 시 잘못된 오류에 적용 금지 |
| EXPORT-01 / M4 | shots, fe_embed | batch/mosaic·DRC 캡처·PNG iTXt metadata/legend/ruler. raster pixels와 metadata 각각 round-trip |
| SYS-01 / M1a→M4 | instance, cli, cache | 바이너리 발견/버전, index freshness, 취소/타임아웃/자식 수거, 개인·설계 설정 구분 |
| SYS-02 / M4 | portable 도구 | Rust+정적 자산만으로 실행, offline build, GLIBC/아키텍처 확인, licenses/About, 브라우저 실행/프로필 격리 |

SYS-01/02의 M4f-1은 native `selfcheck`와 source/target/web bundle 식별이다
([M4 §36](WEBUI_M4.ko.md)). PATH 없는 재배치·인접 도구 제한·버전 기한/취소와
renderd 종료를 검사한다. 기존 CLI 10개/옵션 대조표를 소급 변경하는 parity 항목이 아니라
배포 진단용 확장이다. 전용 Rust portable 조립·ELF/GLIBC 감사·licenses/About·현장 브라우저
수용 전체가 끝난 것으로 간주하지 않는다.
M4f-2는 별도 `make_web_portable.sh`의 Rust 조립·ELF 감사·고지 수집·체크섬과
비덮어쓰기 archive 게시다([안내](WEBUI_PORTABLE.ko.md)). macOS에서 실제 musl archive의
조립/재배치/hash는 검사하지만 Linux 실행·현장 수용 및 About UI 완료와 구분한다.
M4f-3a는 About 빌드 식별과 내장 글꼴 원문 고지까지다([M4 §38](WEBUI_M4.ko.md)).
M4f-3b는 새 portable의 compiled 목록과 검증된 원본 chunk를 유계 UI로 읽는다
([M4 §39](WEBUI_M4.ko.md)). 실제 Linux/Firefox/ETX·남은 UI parity 수용은 별도이므로
SYS-02 전체를 닫지 않는다.

UI-05 일부는 M4d-1에서 연결했다: 표시된 layout/annotation PNG 복사·저장, overlay
3상태, canvas 단축키와 텍스트/IME 보호. 실제 clipboard/다운로드·현장 Firefox/ETX
수용과 주석/설정 저장은 남는다([M4 §15](WEBUI_M4.ko.md)). 이미 그린 pixels만 합성하며
exact clip이나 원본 재렌더 export로 간주하지 않는다.

UI-01의 오른쪽 드래그 박스 줌은 M4g-1에서 연결했다([M4 §40](WEBUI_M4.ko.md)).
GTK의 지배 excursion·5px 축 선택을 이관하고 world 계산은 Rust에 둔다. 현재 표시된
뷰와 배율이 맞아야 시작하며 resize/revision/접속 변경은 취소한다. 나머지 단축키/
CLI startup/single-instance·현장 입력 검증까지 완료한 의미는 아니다.
UI-01의 구조 미니맵은 M4g-2에서 연결했다([M4 §41](WEBUI_M4.ko.md)). 기존 색인의
180px depth 구조 베이스를 재사용하며 pan은 현재 뷰 표시만 갱신한다. 다이 안 클릭은
배율과 16px 위상을 유지한다. 잡덱/구 캐시에 없는 frontier를 새로 만들지는 않는다.
UI-01/05의 M4g-3은 `<`/`>` depth 상대 이동, DRC comma/period 순회와 n/w 편집
진입이다([M4 §42](WEBUI_M4.ko.md)). 주석·waive 파일은 여전히 명시 승인만으로 게시한다.
q 종료 확인은 M4g-4에서 복원했다([M4 §43](WEBUI_M4.ko.md)). Ctrl+, 잡덱 모드
전환 및 CLI startup/single-instance는 남는다.
M4g-5a는 UI-03 잡덱 모드 전환의 Rust 상태 준비만 추가한다([M4 §44](WEBUI_M4.ko.md)).
GTK 가시성/PNG 24경로를 비교하지만 worker 교체·모드 API·Ctrl+, 연결 완료는 아니다.

UI-03의 layerprops 공통 codec·초기 가시성은 M4d-2에서 연결했다([M4 §16](WEBUI_M4.ko.md)).
색/fill뿐 아니라 file/stem default의 visibility를 첫 frame 전에 적용하고, 명시된
startup selection이 우선한다. M4d-3에서 열린 세션 Load/Save와 custom bitmap·fill/width
상속을 보존하는 native JSON을 연결했다([M4 §17](WEBUI_M4.ko.md)). Calibre 형식은
기존 partial import와 현재 표시 스타일 export를 지원한다. 실제 Chrome의 합성 layout
기본값 신규 게시 클릭은 별도 승인 후 확인했다(§20). 교체/취소/복구·file chooser/다운로드와
현장 브라우저 수용은 남아 있다. M4d-4a는 기본값 게시 Rust 코어와 GTK 경로/내용
오라클, M4d-4b는 launcher opt-in·owner 승인/취소/결과 API를 추가했다
([M4 §18/19](WEBUI_M4.ko.md)). M4d-4c는 opt-in 게시 UI와 공유 영향 승인·미확정 요청의
새로고침 복구를 연결했다(§20). 일반 Save는 공유 기본값을 쓰지 않는다.
개인 palette cache는 현재 GTK에도 없으며
자동 저장을 새로 만들지 않는다.

`python -m floe.fe_embed` 보조 CLI도 범위에 포함한다: 위치 인자 PNG들,
`--box`, `--ellipse`, `--line`, `--path`, `--polygon`, `--ruler`, `--text`,
`--json`, `--legend`, `--note`, `--ppu`, `--unit`, `--append`, `--dump`,
`--strip`, `--selftest`. Rust 대응 명령은 M4b-3의 `floe2-web fe-embed`다.
`floe.fillpat`의 색/패턴·Calibre layerprops와 `floe.hangul`의 주석 입력 연계도
누락하지 않는다. GTK 위젯 구현을 Rust로 직역하지 않고 동작은 웹 입력으로 이관한다.

M4b-3에서 위 보조 명령을 `floe2-web fe-embed`로 연결했다([M4 §9](WEBUI_M4.ko.md)).
PNG metadata/옵션·JSON·append/dump/strip은 Rust runtime만 사용한다. 기존 형식의
전체 PNG bytes와 pixels를 Python/Pillow 개발 오라클로 대조한다. `--selftest`는
native smoke이며 외부 Python import를 하지 않는다. DRC capture 조립·웹 주석 편집은
별도 남아 있다. 형식/자원 한계·파일별 원자 게시·동시 편집 한계는 §9에 명시했다.

이관하지 않는 것:

- `floe` KLayout renderer, legacy `.tiles` 생성·skeleton/merge 재구축,
  `floe.coverage`/`design.ovc`, Rust에서 미지원인 abstract. **새 `.ovo`
  occupancy는 폐기한 coverage와 달리 이관 범위**다.
- floe2에 등록되지 않은 최상위 `profile`과 `--coverage/--coverage-only`.
  `index --profile-cell*`은 반대로 반드시 유지한다.
- 숨겨진 index legacy 옵션 `--legacy`, `--tile-mb`, `--skeleton-only`,
  `--texts-only`, `--merge-only`, `--merge`, `--mem`, `--mem-floor`, `--no-gov`,
  `--text-cap`, `--text-tile-cap`, `--skel-texts`, `--tile-tgt`, `--bands`,
  `--read-mode` 및 view/probe의 `--layout-mode`: 기존 명시 거부 경로 유지.
- 개발 fixture/오라클의 Python/KLayout은 제품 런타임과 구분. 개발 도구 전체의
  Rust 전환은 별도 범위 결정 전까지 완료 조건에 넣거나 임의로 삭제하지 않는다.

## 4. 조사에서 드러난 이관 결정 항목

| ID | 확인 사실 | 처리 기준 |
|---|---|---|
| M0-D1 | view `--lod` 값은 Python job/UI에는 있지만 `_submit_render` wire에 없음 | 현 동작을 기록한 회귀를 먼저 만들고, Rust CLI에서 실제 정책 연결/안내 중 결정. 이름만 보고 on/off 기능을 새로 활성화하지 않기 |
| M0-D2 | refinement parser on ≠ Rust 실제 기본 off, stream-target 무시 | 실제 off를 기본 기준으로 고정. 명시 on/페이지 라운드·시간 옵션의 새 의미는 별도 결정 |
| M0-D3 | deck pick/snap/clip/margin/label size 미지원 | capability=false + 이유. 웹 이관 완료와 신규 덱 기능 추가를 구별 |
| M0-D4 | M4a-1~4의 scene/표시 anchor·owner receipt·선택/스냅, M4a-5/6의 Rust 수동·bbox gap ruler와 CD 순서 통합; M4c-3의 viewport clip UI | [M4 기록](WEBUI_M4.ko.md). geometry query와 clip은 layout만, cursor-only ruler는 deck/summary도 지원. clip UI는 summary 표시에서도 별도 exact 소스를 사용. 브라우저 다운로드 수용·나머지 내보내기는 남음 |
| M0-D5 | `.ovo` 교체와 열린 GUI 캐시 수명주기 | 사용자 합의대로 현 실측 blocker 아님. 서버 revision 설계는 초안, hot reload 구현은 별도 승인 |
| M0-D6 | `FLOE_RENDERD_BIN`은 무효여도 다음 후보, `FLOE_INDEX_BIN`은 hard error | 차이를 재현한 뒤 새 Rust 셸에서는 명시 override 실패를 오류로 정규화하는 변경을 기록 |
| M0-D7 | 단일 render에서 visible layers가 비면 native plan에 top이 없어 오류(Python/Rust 양쪽 재현) | 수정: 실제로 비어 있는 plan만 빈 root로 정규화. all-off/바깥 뷰 blank, 구조 frame 유지, retained off→on 회귀 |
| M0-D8 | 기존 덱 index wrapper는 page-target/slow-cell/P2 tuning 및 profile 인자를 전달하지 않음 | Rust는 일반 index 서비스를 재사용해 tuning을 전달. 덱 profile은 source를 직접 지정하도록 명시 거부. 선택·LOD·occupancy는 동일 유지 |
| M0-D9 | 기존 덱 probe는 source skip이 있어도 두 렌더 완료만으로 OK/exit 0 | Rust probe는 skip ledger가 남으면 exit 3과 incomplete를 보고하고 OK를 출력하지 않음. info의 exit 0과 render의 부분 PNG/exit 3은 기존 유지 |

## 5. 로컬/현장 완료 게이트

| ID | 결과/산출물 | 상태 |
|---|---|---|
| M0-L1 | 10개 명령/93개 옵션과 보조 기능 대조표 | 로컬 조사 완료(이 문서), 이관 미구현 |
| M0-L2 | 서비스·wire 경계/자원·revision/API 초안 | 로컬 초안 작성, 상세 값/미결 항목은 API 문서 |
| M0-L3 | HTTP/WS 후보와 offline 의존성 | Axum/Tokio 후보 검토. vendor/lock/MSRV/라이선스/빌드 검증 전, 선정 게이트 미완료 |
| M0-F1 | TeeBox·대표 데스크톱 Firefox/OS/표시 환경 | 미측정. `sh tools/audit_webui_env.sh`로 기본 정보 수집 |
| M0-F2 | ETX 프로필 격리/인스턴스 분리/Canvas·WS 기능/키 입력 | 미측정. 버전 수집만으로 통과 불가 |
| M0-F3 | portal 전달·직접 접속/TLS·공유 서버 권한/자원 배분 | 현장 정책 확인 필요 |

감사 도구는 브라우저를 열거나 프로필을 만들지 않고, 접속/속도 테스트도 하지
않는다. hostname/사용자/설계 경로/환경 전체/토큰을 수집하지 않는다. 예:

```sh
sh tools/audit_webui_env.sh
sh tools/audit_webui_env.sh --firefox /opt/firefox/firefox
```

현장에서는 별도로 두 ETX 세션에서 창이 엉뚱한 세션으로 합쳐지지 않는지,
기존 Firefox 프로필을 건드리지 않는지, 키/IME/클립보드 제약을 기록한다.
스크립트의 `not_measured`를 버전만 보고 PASS로 바꾸지 않는다.
XQuartz 경유 Linux 실행은 GTK 기동·입력 진단에는 도움이 되지만 Firefox-in-ETX
지연/프로필 격리/네트워크 접근의 대체 실험은 아니다.

M1 회귀 기반은 기존 `tools/validate_rust.sh`, `validate_rust_renderer.py`,
`validate_jobdeck.py`, `validate_occupancy.py`, `validate_drc_ice.py`,
`validate_svrf.py`와 synthetic valmini/jobdeck/DRC fixture를 재사용한다.
실칩 데이터·파생 프로파일을 새 브랜치에 커밋하지 않는다.

## 6. 다음 구현 순서

1. M1a-1: worker client의 handshake/open/style/frame/cancel/cleanup과
   fake worker 오류 게이트 → valmini PNG/raw headless 비교. 구현됨(§8).
2. M1a-2: Rust CLI/cache/index/프로파일·occupancy orchestration, 기존 옵션
   golden. 미구현 명령은 명시 거부하고 Python으로 우회하지 않기.
3. M1a-3: jobdeck parser/source/좌표/선택/색·ledger와 composite spec 이관.
   실측 브랜치에서 들어오는 수정은 Rust 쪽 회귀에도 반영.
4. M1b: dependency gate 후 gateway+Canvas+margin+세대/큐 계약,
   loopback G1 및 읽기 범위 Python-free 검증. 이후 현장 M0/G2 측정.

현장 미측정이 1~3의 로컬 개발을 막지는 않지만, 웹 전환이나 GTK 은퇴 승인을
대신하지도 않는다. 먼저 렌더러를 다시 쓰거나 occupancy 정책을 바꾸지 않는다.

## 7. 이번 로컬 검증 기록 (2026-09-12)

- 실제 argparse 객체와 §2의 command별 옵션·별칭·위치 인자를 대조:
  10개 명령, 93개 공개 옵션 action 누락 없음. fe_embed 보조 옵션도 대조.
- 문서의 상대 링크/코드 fence/후행 공백 검사 통과, `git diff --check` 통과.
- `sh -n tools/audit_webui_env.sh` 및 help/무효 옵션·경로/버전 명령 실패
  8개 검사 통과. 기본 실행은 Darwin/arm64, Firefox not_found,
  나머지 현장 항목 not_measured를 출력했다. 현장 호환성 판정이 아니다.
- 제품 코드는 바꾸지 않았으므로 Rust/렌더러 전체 배터리를 새로 실행하지
  않았다. 분기점의 기존 green 결과는 웹 이관 검증과 별개다.

## 8. M1a-1 진행 (2026-09-13)

`rust/worker-client` library와 fake/실제 daemon 게이트를 추가했다.
CLI 10개 명령 이관은 아직 아니므로 §2의 완료 상태를 올리지 않는다.
기존 protocol/renderer·jobdeck/occupancy 정책과 실행 기본 제품은 변경하지 않았다.

- 구현/제약: [worker-client README](../rust/worker-client/README.md).
- 실제 검증: `tools/validate_worker_client.sh`가 기존 Python adapter와 Rust
  client의 PNG 바이트 및 raw 픽셀, 라벨·스타일·재방문·20세대 취소를 대조.
  `tools/validate_rust.sh`에서 필수 실행한다.
- macOS/arm64: `cargo fmt --check`, worker-client 엄격 clippy, protocol 3개와
  fake-worker lifecycle 8개 그룹, 실제 daemon 게이트 통과. cold worker에서
  `round_pages=1`이 실제 여러 라운드가 되는지 단언하고 최종 raw 일치도 확인.
- 전체 `sh tools/validate_rust.sh`: `RUST VALIDATION: ALL OK`, exit 0.
  새 작업 트리에 기존 index CLI 테스트용 `data/m1/valmini.oas`가 없어 첫 실행은
  중단되었고, 합성 fixture 생성 후 재실행했다. 기존 dependency 경고는 남아
  있지만 새 crate의 clippy 경고는 없다. 배터리의 큰 입력 옵션을 쓰더라도
  새 client 게이트는 작은 valmini만 렌더하도록 제한했다.
- Linux: `cargo check --offline -p floe-worker-client --lib
  --target x86_64-unknown-linux-musl` 통과. **교차 컴파일 검사이며 Linux 실행·
  동시 사용자 부하·실칩 성능 검증은 아니다.**
- 다음: M1a-2 CLI/cache/index 서비스(§9). M0 현장/HTTP 의존성 게이트는 별도 대기.

## 9. M1a-2a 진행 (2026-09-13)

`floe2-web index`의 일반 레이아웃 경로를 추가했다. 옵션/캐시 보호, LOD/
occupancy, 셀 프로파일과 snapshot을 Rust `app-core`에서 처리한다.
잡덱/`--level`은 이후 M1a-3b에서 추가했다. M1a-2a 자체의 범위는 일반 레이아웃이다.
[실행 방법](../rust/app/README.md),
[상세 계약·검증·제약](WEBUI_M1A.ko.md)을 따른다.

## 10. M1a-2b 진행 (2026-09-13)

일반 레이아웃 info(기존 text + opt-in JSON), 단일 PNG render/report, probe를
Rust 서비스로 연결했다. `validate_app_render.py`가 기존 Python과 12종 PNG/
report·메타데이터를 대조하고 오류·취소·파일보호를 검사한다. 상세는
[M1a §7](WEBUI_M1A.ko.md#7-m1a-2b--일반-레이아웃-읽기와-단일-캡처).
render의 batch/mosaic/DRC·주석 export는 M4, 덱은 M1a-3이므로 해당 명령 전체
완료 표시는 하지 않는다. M0-D7은 이 과정에서 드러난 별도 native 결함이다.

## 11. M1a-3a/b 진행 (2026-09-13)

잡덱 문법·좌표·ledger 모델과 bounded source header/catalog, 덱 index의 레벨 선택·
LOD·occupancy·순차 job 실행을 Rust로 이관했다. 문법/배치 모델의 Python+손계산
대조, real OASIS 캐시 바이트 대조, 취소/오류 게이트를 배터리에 연결했다.
M1a-3c에서 덱 색/레이어 행·spec·`jobdeck` 분석 CLI까지 이관하고 Python 대조와
실제 daemon 합성 PNG 검사를 추가했다. 덱 info/render/probe 연결은 다음 단계다.
