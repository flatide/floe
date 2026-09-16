# G4 — GTK 메뉴 원본 재대조

2026-09-16, M4g-23. 기준 구현 `e48e91f`와 현재 `floe/gui.py`.
상위 [잔여 감사](WEBUI_G4_AUDIT.ko.md), [M0 목록](WEBUI_M0.ko.md).
이번 감사는 **새 기능 구현 완료가 아니라 기존 완료 근거의 범위를 정정**한다.

## 1. 방법과 결과

`tools/validate_web_menu_inventory.py`는 GTK를 import하거나 실행하지 않고
`_build_menubar` AST에서 실제 item/check callback을 추출한다.42개 호출 지점,
39개 handler family를 분류한다. thin/mode의 각각3항목 반복문은 호출 지점1개씩이다.
최초 감사 당시36개가 연결됐고, M4g-26b 이후 현재39개 호출 지점은 웹 control·JS
참조·테스트 파일에 연결된다.2개는 제품 범위 밖,1개는 기존 Rust 경로에서 무효이며
이 목록의 OPEN은0개다. 이는 알려진 메뉴 연결 누락의 해소이지 전체 수용 완료가 아니다.

`linked`는 **연결 근거가 존재함**이지 기능 parity PASS가 아니다. 소스·테스트 파일의
존재만으로 픽셀/행동/권한/실브라우저 수용을 증명하지 않는다. 실제 동작 검사는
기존 native/HTTP/UI gate와 G1~G4 수용 목록을 함께 봐야 한다. 새 callback·제거된
callback·웹 control/테스트 링크 소실은 실패하며, 이 결함 주입도 자체 검사한다.

```sh
python3 -B tools/validate_web_menu_inventory.py
# inventory 확인: exit0, linked39/open0.
python3 -B tools/validate_web_menu_inventory.py --require-complete
# M4g-26b부터 exit0, 전체 배터리에 배선. 전체 G4 PASS가 아니다.
```

| 원본 메뉴 묶음 | 웹 연결 근거 / 제외 이유 |
|---|---|
| layout/jobdeck 열기 | browse/launcher/index-open, 선택 레벨 확인·색인 동의. DRC 파일 열기와 다른 경로 |
| clip / copy / 종료 | clip/snapshot/session-exit의 명시 조작과 승인·취소·수명 검사 |
| fit / zoom / goto / detail / label size / depth | app.js의 controls·viewport 입력과 client/GTK 정책 oracle. zoom 버튼의1.25배와 Ctrl+Z/Shift+Z의2배를 구분 |
| frames / thin / mono / overlay | 요청 정책·키·표시 및 client gate. 물리 화면 수용 별도 |
| ruler / snap / undo / clear | measure/rulers, native geometry와 JS 입력/표시 gate |
| DRC 이동 / box 선택 / waive / note | drc/drc-notes/drc-waives 및 관련 HTTP/UI gate. note 삭제는 빈 텍스트 확정으로 제공 |
| review import / export | drc-transfer: 등록된 reviewer의 전체 review 교체·별도 승인. 임의 서버 경로 쓰기 아님 |
| jobdeck mode / Ctrl+, | live-mode + 서비스 Mode: 현재 카메라·로드 레벨 유지. 로드 레벨 재선택과 다름 |
| jobdeck 로드 레벨 재선택 | reselect-levels + 서버 camera/revision 고정 요청·명시 index 재시도. 첫 프레임 카메라/실패 보존 HTTP와 UI gate |
| About / licenses | about/notices + 배포 고지 gate |
| abstract / 옛 coverage | Rust abstract 미지원, density coverage 폐기. 새 occupancy는 제외 대상이 아님 |
| LOD 토글 | 기존 GTK→Rust wire에 전달되지 않음. 웹은 무효로 수용하지 않고 설명과 함께 거부. index --lod와 다름 |

## 2. 확인된3건의 진행 상태

### G4-MENU-01 — 열린 창에서 DRC 파일 열기/교체 (24c 로컬 연결)

GTK `_drc_open_dialog`→`_drc_open_db`는 사용자가 파일을 선택하고 현재 인접 ICE를
읽거나 색인 동의를 받는다. `load_drc`는 현재 레이아웃을 유지하면서 DRC를 교체한다.
M4g-23 감사 당시 웹은 `view --drc`로 시작할 때만 `Gateway::attach_drc_registry`를 호출했다.
이 함수는 publish 전 `Arc::get_mut`을 요구한다. browse actor의 Select는 레이아웃
등록/launch proposal만 만들고, DRC 패널의 Reload review는 **같은 등록**을 다시
읽는다. Build pack도 같은 등록 ASCII→ICE이지 다른 파일 열기가 아니다.

따라서 CLI로 새 workspace를 띄우는 우회는 이 기능의 완료 근거가 아니다.
필요한 구현은 승인 폴더 내 opaque 파일 선택→원본/cache 확인→필요시 별도 색인
동의→현재 review 교체다. DRC 없이 시작한 workspace의 최초 등록도 다뤄야 한다.

교체는 새 read identity, 선택/그룹/CD·prepared focus 폐기, 오래된 응답 차단을
같은 경계로 묶어야 한다. 진행 중 게시/결과 불명 receipt를 버리거나 다른 DB에
재전송하면 안 된다. reviewer opt-in·legacy 읽기·write target·defaults/index의
보호 경로를 새 등록에 맞게 갱신하되 **파일 선택을 새로운 쓰기 승인으로 취급하지
않는다**. 원본/sidecar를 암묵 수정하지 않으며 느린 open·취소에도 자원 예약과
worker 수거가 유지돼야 한다. 레이아웃 카메라/레이어/캐시는 불필요하게 바꾸지 않는다.

M4g-24a 선행 구현: 공유 `SourceSet`에 DB/SVRF/원본·pack tree의 게시 금지 경로를
등록하고, 기본값/리뷰 게시와 동일한 reservation으로 직렬화한다. 등록 전에 준비된
초안도 게시 직전에 재검사한다. reviewer sidecar/lock은 기본값 게시만 막으며 기존
리뷰 writer의 별도 권한을 새로 주거나 없애지 않는다. 현재 연결 지점은 시작 시 DRC
등록/리뷰 등록이었다([M4 §79](WEBUI_M4.ko.md)).

M4g-24b: `Open DRC results…`→동일 승인 폴더 picker→`open_drc`를 연결했다.
DRC 없는 세션의 최초 등록, 기존 DRC 유지 상태에서 후보 준비, 원자 교체와 이전
reader 수거, 이전 저장 receipt 유지/새 편집 차단, layout cache/index 충돌을
다룬다. **교체된 DRC의 명시적 reviewer 재등록/저장 opt-in은 아직 없다.** 새 입력은
읽기 전용이고 예전 reviewer·SVRF·legacy sidecar 권한을 자동 승계하지 않는다.
전체 동등 기능으로 세지 않고 이 항목은 계속 OPEN이다([M4 §80](WEBUI_M4.ko.md)).

M4g-24c는 위의 후속을 연결했다. 사용자 결정은 **런처에서 허용한 reviewer와 권한만
새 DRC에 명시적으로 재연결**하는 것이다. 파일 열기는 계속 읽기 전용이며 별도
재연결 동의가 필요하다. reviewer 미지정 실행은 쓰기 권한을 만들 수 없고, 읽기
전용/notes 전용/notes+waives grant는 그대로다. 준비된 ICE가 필요하며 색인은 별도
승인한다. 같은 review worker/저장·전송 ledger를 유지해 순번을 리셋하지 않고,
receipt의 원래 binding epoch를 보존한다. draft/transfer artifact/자동 저장 동의는
넘기지 않는다. 합성 실제 HTTP 및 UI gate는 [M4 §81](WEBUI_M4.ko.md)에 기록한다.
inventory는 linked로 바꾸되 실제 브라우저/현장 수용은 별도로 남긴다.

### G4-MENU-02 — 열린 DRC에 SVRF metadata 불러오기/교체 (25 로컬 연결)

GTK `_drc_rules_dialog`→`_drc_rules_load`는 JSON을 읽고 rule match/type census/
선택 오류의 상세 metadata를 갱신한다. 웹 `--drc-rules`는 최초 등록 경로이며
등록의 `rules`는 고정이다. type 필터/constraint 표시가 구현됐다는 사실로 실행 중
metadata 교체까지 완료로 세면 안 된다.

승인 폴더·opaque 선택을 재사용하되 JSON 제한·취소·보호 입력을 적용한다. geometry
reader 재사용 여부와 별개로 metadata/query revision 및 filter/선택의 유효성을
원자적으로 바꿔야 한다. 이전 type/page 응답을 새 metadata 결과로 보이지 않게 하고,
다른 DB로 매칭하거나 실패한 입력 때문에 기존 유효 metadata를 조용히 지우지 않는다.

M4g-25는 `Load SVRF metadata…`→opaque picker의 `load_drc_rules`로 구현했다.
큰 ASCII DRC를 다시 읽지 않도록 동일 actor의 열린 Database에서 새 metadata만
준비한다. 성공 시 snapshot/query revision을 원자 교체하며 실패·취소는 이전
metadata와 revision을 그대로 둔다. layout/reviewer/저장 receipt는 유지하고
type/filter/selection·미승인 preview는 초기화한다. 입력 JSON은16MiB 제한,
원본 deck/include를 읽거나 파일을 쓰지 않는다. 나중의 명시 pack build/reviewer
재연결에도 별도 승인 입력 scope와 metadata 선택을 보존한다. 합성 HTTP/DOM
회귀는 [M4 §82](WEBUI_M4.ko.md); 실제 브라우저 수용과는 별개다.

### G4-MENU-03 — 카메라를 유지한 jobdeck 로드 레벨 재선택 (26b 로컬 연결)

GTK `_jobdeck_reselect_levels`는 현재 `cx,cy,spp`를 보관한 뒤 새 레벨을 열고
`_fit_after_worker_start=False`로 복원한다. 웹의 Levels to load + Open은 일반
open이며, `service/open.rs`의 새 레벨 집합은 `ViewState::initial`에서 시작한다.
window display 정책도 의도적으로 camera를 포함하지 않는다. 현재 Mode 조작은
기존 로드 레벨만 유지하므로 이 기능을 대체하지 못한다.

감사 당시 요구조건: 현재 view/revision과 서버의 카메라 snapshot에 묶인 명령이어야 한다.
metadata/cached source를 먼저 확인하고 실패·취소 시 기존 뷰를 유지하며, 새
레벨에 색인이 필요하면 원래 선택/카메라를 보존한 명시 동의 경로로 이어져야 한다.
그 사이 pan/resize/다른 open이 발생하면 오래된 카메라로 덮어쓰지 않는다. 성공 시
첫 프레임부터 카메라가 맞아야 하며, fit 이후 별도 goto로 겉보기만 맞추지 않는다.

M4g-26a 선행 구현은 managed deck index의 잠금 분류다. 현재 레벨이 읽는 캐시를
재사용하고 새 선택 소스만 만드는 작업은 열린 뷰를 유지한 채 가능하다. force나
occupancy-only가 열린 캐시를 수정하려 하면 전체 계획을 첫 쓰기 전에 거부한다.
26a 시점에는 카메라 고정 재선택 명령/브라우저/색인 재시도를 연결하지 않았으므로
이 항목은 계속 OPEN이었다([M4 §83](WEBUI_M4.ko.md)).

M4g-26b에서 `Levels to load` 안의 `Apply levels · keep view`를 연결했다.
현재 view/revision과 서버 camera에 묶인 선택만 받으며 source/mode/좌표/쓰기 권한을
브라우저에서 바꾸지 않는다. 첫 프레임부터 같은 bbox/pixels이고 새 layer defaults를
적용한다. 같은 선택은 no-op이다. 새 선택의 미색인 소스는 이전 화면을 유지하며
별도 index 승인으로 이어지고, 승인/replay에도 원래 view/revision/pixels를 고정한다.
그 사이 pan/resize/open은 오래된 교체를 거부한다. metadata 실패·취소·완료된 캐시와
실패한 화면 교체의 구분은 실제 HTTP/native gate로 확인한다([M4 §84](WEBUI_M4.ko.md)).
이 항목을 linked로 바꾸지만 브라우저·현장 수용은 완료로 세지 않는다.

## 3. 다음 구현과 판정

다음 우선순위는 진단/무효 CLI 경계와 전체 G4 목록의 최종 재대조다.
세 항목은 모두 실제 런타임 연결까지 진행했으며 CLI 시작만으로 대체하지 않았다.
세 항목을 모두 닫아도 actual browser·Python-free Linux·G1/G2/G3/G4, 공유/원격,
조건부 M5가 자동 완료되지는 않는다. APNG 정적 기본 이미지는 M4g-29에서 연결했으며
GTK 위젯 진단/명령 폐기 및 무효 CLI 제품 경계는 기존 별도 목록대로 남는다.
현장 실행 불가·이전 도구 제한은 우회하지 않는다.
