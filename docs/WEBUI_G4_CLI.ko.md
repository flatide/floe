# 웹 전환 CLI 재대조

2026-09-16, M4g-27. 기준 `d7b81aa` 이후의 `feature/webui`.
[M0 원래 범위](WEBUI_M0.ko.md), [전체 잔여](WEBUI_G4_AUDIT.ko.md),
[단계 기록](WEBUI_M4.ko.md). **파서 목록의 완성과 기능·현장 수용을 구별한다.**

## 1. 범위와 검사 방식

`floe.cli.main(rust_only=True)`의 실제 argparse 생성 직후, dispatch 전에 멈춰
공개 명령10종의94개 옵션 action·위치 인자·별칭·기본값·choices·필수 여부를 읽는다.
`floe.fe_embed.build_parser()`의 보조 명령16개도 포함해 총110개다. 숨겨진 legacy
옵션17개는 별도로 거부를 확인한다. M0의93개/`index`17개는 이전 조사 시점 수치이며,
현재는 `--no-occupancy`를 포함해 `index`18개다. 원래 범위를 줄인 것이 아니다.

`tools/validate_web_cli_inventory.py`는 새/누락 명령·옵션·별칭·기본값 변경을 실패시킨다.
네이티브 release의 각 옵션을 **`--help` 앞에** 놓아 실제 parser가 소비/거부해야만
통과한다. choices/별칭과 숨김 옵션을 포함해175회 실행한다. 빈 PATH와 없는 worker/
browser 경로, 비공개 임시 cwd를 사용하며 source 파일·설정·세션을 만들지 않는다.
옵션 추가/명령 삭제/별칭·기본값 변조를 주입해 목록 검사의 실패도 확인한다.

이 검사는 **파서 진입·목록 누락/변경 감지**다. `--help`는 최종 옵션 조합 검사 전에
돌아갈 수 있으므로 성공을 정상 파일 출력이나 의미 동일성으로 계산하지 않는다.
숫자·좌표·파일/픽셀·수명·저장 권한은 아래 실제 통합 게이트의 별도 책임이다.
테스트 파일 존재 자체도 의미 검증 통과를 뜻하지 않는다. 전체 배터리의 해당 결과와
테스트 assertion을 함께 확인한다. Python/Node는 개발 오라클이며 제품 의존성이 아니다.

## 2. 명령별 구현·증거와 차이

모든 공개 옵션/별칭과 샘플 값의 명시 목록은 위 검사 파일의 `SURFACE`다.
아래는 그 목록의 의미별 근거다. 공통 help/version, bare source, 빈 창도 누락하지 않는다.

| 명령 / 공개 옵션 수 | 실제 구현·의미 근거 | 남는 차이/경계 |
|---|---|---|
| index /18 | `app/main.rs`, `app-core/index.rs`, `jobdeck/index.rs`; `validate_app_cli.py`의 실제 OVM/OVP/OVO bytes·force/reuse/corrupt·profile JSON/snapshot·native argv·signals, `validate_app_jobdeck_sources.py`의 선택/중복 소스·LOD/occupancy | 덱 profile은 개별 TC로 안내하며 거부. occupancy 기본 on, LOD 기본 off. legacy15개 거부 |
| info /1 | `app/read.rs`, `validate_app_render.py`의 top/DBU/bbox/레이어·stale, `validate_app_deck_render.py`의 덱 info/ledger | Rust `--json` 추가. cache 손상과 source stale 경고를 구별 |
| render /29 | `app/read.rs/capture.rs`, `validate_app_render.py`, `validate_app_captures.py`, `validate_drc_captures.py`; Python PNG/report·batch/mosaic·metadata·DRC CD/legend·j1/j8·취소/불완료 | 파일별 원자 게시, batch 전체 트랜잭션 아님. 알려진 덱 skip은 incomplete/exit3. 화면 요약과 archival exact를 구별 |
| clip /5 | `app/clip.rs`, `validate_app_clip.py`; Python/j1/j8 bytes·KLayout XOR·원본/기존 출력 보호·timeout/신호 | 덱 clip은 기존에도 미지원. `--exact`는 이미 exact인 호환 플래그 |
| probe /0 | `app/read.rs`, `validate_app_render.py/validate_app_deck_render.py`; 실제 ready/open/style/frame/실패·신호 | 덱 skip을 성공으로 숨기던 동작은 incomplete/exit3으로 정정. 숨김 `--layout-mode` 거부 |
| drc /4 | `app/drc.rs`, `validate_app_drc.py`; ASCII/현재 ICE·소수 좌표·waive·JSON/list·빈 PATH·stale/corrupt/신호 | 명시 build/SVRF 기능은 추가 옵션. 검사 명령이 reviewer 쓰기 권한을 만들지 않음 |
| svrf /6 | `app/svrf.rs`, `validate_app_svrf.py`; Python 전체 parser/scan·환경/include·short/long·fault/원자 게시 | subset parser이지 Tcl/Calibre 실행 아님. 검사하는 환경 이름과 include 접근은 CLI/웹에서 별개 |
| gtktest /0 | `app/main.rs`의 명시 안내, Rust `displaytest [PNG]`와 About 합성 진단; `validate_display_test.py`, `validate_display_cli.py`, `validate_display_input.py` | GTK 위젯 진단·명령 폐기 승인과 APNG 정적 기본 프레임은 아래§4. 실제 화면 합격 아님 |
| view /21 | `app/web_view.rs`, service/controller/UI; `validate_web_cli.py`, `validate_web_startup.py`, `validate_web_handoff.py`, reviewer/UI/HTTP 게이트 | 옵션별 경계는§3. 시작 등록과 실행 중 메뉴 교체는 서로 다른 게이트. 실제 브라우저/현장 별도 |
| jobdeck /10 | `app/deck_analysis.rs`, `validate_app_jobdeck.py/validate_app_jobdeck_plan.py`; parser·placements·report/spec·모드·선택·missing exit | 분석 명령과 live chip/level 행 모델을 구별. 비공개 포맷을 일반화하거나 signoff로 주장하지 않음 |
| fe-embed /16 | `app/fe_embed.rs`, `validate_fe_embed.py`; Python 전체 PNG bytes·픽셀·JSON/7종 annotation·append/dump/strip·fault | selftest는 native 메모리 검사. 전체 파일 일괄 트랜잭션/외부 프로세스 편집 잠금 아님 |

추가 entry: `selfcheck`는 설치/의존성 진단, `displaytest`는 표시 진단이며 GTK 화면 수용
대신이 아니다. `floe2-web FILE`, 인자 없는 실행, `--multi`/forward, goto/depth/detail
첫 프레임 원자 적용은 startup/handoff의 실제 CLI·HTTP/프로세스 게이트를 사용한다.
`FLOE_RENDERER`로 Python/KLayout fallback을 추가하지 않는다.

## 3. view 옵션의 실제 의미

### 승인된 refinement 호환 복원

사용자 결정: **기존 동작 유지**, 기본은 실질 off이며 새 적응형 정책을 만들지 않는다.

- 생략 또는 `--refinement on`: `FLOE_RUST_ROUND_PAGES`를 따름. 환경이 없으면
  기존처럼2^30 페이지이므로 현실적인 miss set은 한 라운드다. `on`만으로 새 round
  크기·시간/바이트 예산을 정하지 않는다.
- `--refinement off` 또는 `--stream-kb 0`: 환경보다 우선해2^30을 사용한다.
  `--stream-kb 0 --refinement on`도 off다. 중복 `--refinement`는 마지막 값이 우선한다.
- `--perf-baseline`: 순서와 무관하게 refinement·frame reuse·frames·labels off.
  decoded cache·geometry cut은 유지한다. 원래 무효인 live LOD를 새로 제어하지 않는다.
- 다른 독립 실행 옵션이 없고 유효한 마지막 refinement가 on이면 생략처럼 기존
  workspace를 사용한다. 이미 열린
  worker의 환경은 송신자의 값으로 바꾸지 않는다. off/stream0/baseline은 별도
  workspace다. 프로세스 옵션을 화면 patch로 forward하지 않는다.
- 상태줄의 고정 `refinement off` 문구를 제거했다. 실제 프레임의 `round`/`final`을
  표시하고, 중간 프레임은 `Refining`, 최종 불완전 프레임은 `INCOMPLETE`다.
  최종1라운드만 보였다는 이유로 프로세스 설정까지 off라고 추론하지 않는다.
  margin은 foreground timing을 바꾸지 않으며 u64 라운드를 JS Number로 변환하지 않는다.

`validate_web_startup.py`는 원본 `cmd_view`의 서버 생성 전 prefix와
`RustRenderWorker.__init__`의 실제 `_round_pages` 대입만 실행한다(생성자/worker/GUI 미실행).
이를 네이티브 controller가 실제 소비한 첫 generation의 프레임 수와 대조한다.
환경1·환경 생략, on/off·중복·stream0·baseline과 기본 표시 설정을 검사한다.

### 그대로 유지하는 별도 경계

| 옵션 | 현재 동작·판정 |
|---|---|
| nonzero `--stream-kb` | 웹은 명시 거부. **기존 Rust도 KB를 배치 크기로 쓰지 않았다**. 0이 아니면 환경 page round를 따르던 허용 인자다. 양수 호환 수용과 새 byte-budget 기능은 다른 결정이며 후자를 필수 이관으로 부풀리지 않는다 |
| `--stream-target-ms` | 기존 Rust에서 쓰지 않던 값. 웹은 명시 거부. 시간 적응형 정책을 새로 만들지 않음 |
| view `--lod` | 기존 Rust wire에 전달되지 않음. on/off 모두 명시 거부. `index --lod`는 생성 옵션으로 지원 |
| `--hairline`/`--thin-um` | 기존 KLayout planner에만 연결. 웹은 명시 거부. `--thin keep/cull`로 같은 의미인 척 대체하지 않음 |
| `--render-debug` | 전체 wire/경로·좌표 출력 대신 숫자 전용 진단. 독립 workspace, stderr 지연 가능성을 명시 |
| `--dump` | 사용자가 선택한 브라우저 최근 수신/합성 bitmap 보관·명시 다운로드. 고정 서버 `/tmp` 파일 덮어쓰기 아님 |
| `--floe-reviewer` | 명시 reviewer 읽기만. 인접/승인된 유도 legacy 경로, ASCII의 현재 ICE 선택. ambient 이름을 자동 채택하거나 쓰기 권한으로 alias하지 않음 |

## 4. GTK 표시 진단을 애니메이션 기능과 혼동하지 않기

`floe/cli.py:cmd_gtktest`는 `GdkPixbuf.Pixbuf.new_from_file(...).scale_simple(...)`를
`Gtk.Image.set_from_pixbuf`로 표시한다. 애니메이션 player가 아니다. 2026-09-16 로컬
GdkPixbuf2.44.7 PNG loader에 합성3×2 APNG를 넣은 읽기 검사에서, 기본 이미지가
animation의 첫 frame인 경우/별도 fallback인 경우 모두 IDAT의 빨강 정적 픽셀을 얻었다.
이 검사는 위젯·원격 화면·다른 loader 버전의 보장이 아니며 브라우저를 실행하지 않았다.

따라서 남은 APNG 차이는 **정적 기본 프레임 진단**이다. 현재 `displaytest`는 APNG
control/data chunk를 명시 거부한다. 이를 해소하는 데 animation 재생기가 필수라고
주장하지 않는다. 정적 fallback 추출/검증을 후속으로 검토하며, 아직 구현됐다고 세지 않는다.
GTK 위젯 자체의 진단/기존 명령 폐기 승인도 이 PNG 호환성과 별개의 제품 경계다.

## 5. 완료 판단

이번 파서 목록 검사는 인자 표면의 알려진 누락·drift를 감지하고, 사용자 결정에 따른
refinement 호환·잘못된 상태 표시를 수정한다. 양수 stream 호환, APNG 정적 fallback/
GTK 진단 경계가 남는다. 실제 브라우저·Python-free Linux·G1/G4·현장, M2 공유/원격
승인·구현, 조건부 M5는 그대로 별도다. 메뉴 목록0 OPEN과110개 파서 대조를 전체
goal 완료율로 계산하지 않는다. 전체 배터리/집중 검증 결과는 M4의 해당 단계에 기록한다.
