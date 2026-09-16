# 웹 전환 G4 잔여 감사

갱신: 2026-09-16, M4g-25(실행 중 SVRF metadata 교체). 상위 [계획](WEBUI_PLAN.ko.md), 원래 범위
[M0 §2~3](WEBUI_M0.ko.md), 단계별 실행 기록 [M4](WEBUI_M4.ko.md).

이 문서는 **로컬 구현과 전체 수용을 분리하는 잔여 목록**이다. 표의 구현/게이트는
기능 경로의 존재와 로컬 회귀 근거이며, GTK 은퇴 또는 실제 브라우저 동작의 최종
합격표가 아니다. 커밋 개수를 완료율로 환산하지 않는다. 아래는 1차 대조이며 새
누락을 발견하면 추가한다. 고객 파일·현장 프로파일은 저장하지 않는다.

M4g-23의 [메뉴 원본 재대조](WEBUI_G4_MENU.ko.md)에서 **실행 중 DRC 파일 교체,
SVRF metadata 교체, 카메라 유지 jobdeck 레벨 재선택**3건의 구현 누락을 확인했다.
시작 시 CLI 등록/일반 Open/Mode 전환은 동등한 대체가 아니다. 로컬 구현의 잔여를
진단 옵션과 실제 브라우저 수용만으로 좁혀 보고한 이전 설명을 정정한다.
M4g-24b/c는 DRC 최초 등록/교체와 런처 권한에 한정한 reviewer 명시 재연결을
연결했다. 자동 저장은 새로 켜야 한다. G4-MENU-01의 로컬 근거는 연결됐으며
M4g-25는 SVRF 교체도 로컬 연결했다. 카메라 유지 레벨 재선택1건은 계속 OPEN이다
([M4 §81~82](WEBUI_M4.ko.md)).

## 1. 범위별 현재 근거와 남은 일

| 범위 | 로컬 구현/검증 근거 | 아직 닫지 않은 부분 |
|---|---|---|
| CLI 10종·보조 PNG 명령 | `rust/app`, `rust/app-core`; `validate_app_cli.py`, `validate_app_render.py`, `validate_app_jobdeck*.py`, `validate_fe_embed.py` 등. view 옵션/형식 대조 M4 §62; M4g-15a/20/21 reviewer 읽기; M4g-22 브라우저 dump | 무효/개발 옵션의 최종 제품 경계. GDS/gzip은 기존 native 한계이며 새 지원 아님 |
| UI-01 열기·조작 | `launcher.js`, `browse.js`, `index-open.js`, `gestures.js`, `minimap.js`, `app.js`; 실제 startup/IPC·DOM gate. M4g-19 GTK 휠137입력·현재 표시/대기/drop·margin/DPR gate | G4-MENU-03 카메라 유지 레벨 재선택 미구현. 실제 브라우저 포커스·키·wheel·resize·재접속 수용, G1 input→photon/pacing·장치별 감도 |
| UI-02 pan/margin | `view_controller`, `stream`, `app.js`의 착지 margin/16px 위상·crop/표시 base, controller/stream/client gate | GTK 대비 새 strip·라벨 지연 실제 화면 측정. 덱 margin은 기존에도 미지원 |
| UI-03 레이어·스타일 | Rust 선택/CAS, palette/presets/settings/defaults; GTK 원본 선택·접힘·상속 oracle, HTTP/DOM gate. M4g-16d 슬롯 모델/API·JSON v2·참조 프리셋/16×16 UI와324 GTK/native/web 대조 | 실제 선택·다중 스타일·기본값 게시와 슬롯 pointer/keyboard·포커스 화면 수용([계약/gate](WEBUI_BITMAP_SLOTS.ko.md)) |
| UI-04 pick/snap/측정 | query/inspect/measure/rulers + Rust query/export, 숫자·scene 유효성·stale gate | 실제 포인터/클립보드·시각적 측정 검증 |
| UI-05 입력·복사·종료 | snapshot/session-exit와 단축키 보호. **M4g-13 두벌식 fallback**: `hangul.js`/`drc-notes.js`, GTK 원본 조합 oracle | OS IME와 fallback의 실제 입력·스크롤·키보드/브라우저별 수용. DOM gate로 대체하지 않음 |
| DRC-01 조회·선택 | `app-core/drc`, `web/src/drc`, `drc*.js`; lazy paging/selection/CD/isolation/query gate; M4g-24b/c DRC 교체·런처 reviewer 재연결, M4g-25 같은 reader의 SVRF 원자 교체 | 현장 대형 결과와 실제 브라우저 조작 수용 |
| DRC-02 저장·전송 | reviewer 고정 sidecar, snapshot/prepare/approve·CAS·receipt, notes/waives/transfer HTTP와 UI gate. **M4g-14 확정 시 자동 저장 opt-in**, M4g-15a/20/21 읽기/쓰기 분리·legacy·ASCII/cache | 실제 브라우저 저장/충돌/복구 수용, 대형 sidecar 연속 저장 및 cache 선택 cold-open 비용 실측 |
| EXPORT-01 | Rust capture/mosaic/PNG metadata/clip + snapshot; raster/metadata/DRC-capture gate | 실제 브라우저 copy/download/승인 표시 수용 |
| SYS-01/02 | Rust worker 발견·수거·cache freshness·selfcheck·portable/ELF/고지; native/포장/전송 gate. M4g-17a~c 합성/독립/정적 PNG 진단; M4g-22 최근 수신·합성 bitmap/명시 PNG 다운로드 | 실제 dump/표시·다운로드 수용, GTK 진단/애니메이션 PNG 경계, Python-free **Linux에서 실행**, 현장 Firefox/ETX, G4 전체 end-to-end 판정 |

표의 `validate_*.py`와 Node는 개발 오라클/하네스다. 제품 실행 경로에 Python,
KLayout, Node를 다시 넣지 않는다. 브라우저의 입력 조합·표시 일시 상태는 계획대로
정적 JS에 두고, geometry/조회/파일 저장·권한·충돌 판정은 Rust에 둔다.

## 2. 자동 저장 — 사용자 결정과 M4g-14 구현

2026-09-16 사용자 선택: **reviewer별 자동 저장을 먼저 명시적으로 켜는 opt-in**.
M4g-14에서 연결했다([M4 §64](WEBUI_M4.ko.md)). 아래 동작은 집중/연결 검사에서
통과했으며 전체 배터리 결과는 해당 절에 기록한다. 실제 브라우저 수용은 별도다.

- 기본 off. 서버가 등록한 reviewer와 허용된 note/waive sidecar 범위 안에서만 동작.
  reviewer 선택 자체를 쓰기 동의로 간주하지 않는다.
- GTK처럼 노트는 **편집 확정** 시, waive는 **상태 변경 확정** 시 저장한다.
  글자 입력마다, IME 조합 중, blur/선택 변경/창 닫기만으로 저장하지 않는다.
- 기존 snapshot의 DRC/revision/선택·sidecar CAS, 충돌/잠금/원자 게시·receipt를
  보존한다. 새 대상·다른 DB로 넘어간 초안을 자동 적용하거나 외부 변경을 덮어쓰지 않는다.
- 실패/만료/결과 불명은 사용자에게 표시하고 초안을 보존한다. 자동 재시도·자동
  legacy sidecar 채택·in-pack fallback을 추가하지 않는다. import/export와 공유
  기본값 게시의 동의는 이 opt-in에 포함하지 않는다.
- 기존 note 파일에 파싱 경고가 있으면 수동 미리보기/승인을 요구한다. 자동 저장
  동의가 무효 행 정리까지 암묵 승인하지 않는다. 창/편집기를 닫으면 기존처럼
  메모리 초안이 지워지며, 원문을 storage에 저장하거나 재연결 때 재작성하지 않는다.
- 구현은 **현재 탭·등록된 reviewer·연결 epoch 범위**의 로컬 opt-in이다. notes/waives를
  각각 켤 수 있게 하며 서버의 기존 `--drc-reviewer`/`--drc-edit-waives` 권한이 없으면
  활성화하지 못한다. 설정 파일·sessionStorage로 opt-in을 자동 복원하지 않는다.
- 사용자 확정 시 opt-in 세대를 포착하고 read/prepare/승인 직전 다시 확인한다.
  도중 해제 또는 해제→재활성화는 이미 시작한 준비에 새 승인을 주지 않는다.
  이미 제출된 저장을 opt-out으로 되돌렸다고 표시하지 않는다. disconnect/종료는
  opt-in을 해제하며 미확인 receipt의 기존 명시 복구 경로를 유지한다.
  실제 WebSocket 단절은 `stop()`과 다르므로 DRC 패널의 연결 상태/epoch에 직접 결합한다.
  정상 waive 저장에 따른 reader revision 변경만으로는 opt-in을 해제하지 않는다.
- 서버의 read/prepare/submit API를 유지하고 UI 확정 동작만 기존 승인 요청에
  연결하는 범위다. 서버 배경 저장기·주기 저장·새 공유 권한을 추가하지 않는다.
  파일 게시와 waive reader refresh 결과는 계속 별도로 표시한다. 기존 수동 경로도
  유지한다. 지연 read/prepare/승인 직전 GET·해제/충돌 gate와 실제 Rust HTTP 저장으로
  검증했다. 카탈로그 `autosave:false`는 서버의 독자적 배경 저장이 없다는 의미로 유지한다.

성능 주의: 현재 sidecar CAS는 파일 전체 해시/재작성 비용이 있다. 단일 operation
진행 중 추가 저장을 쌓지 않으며 대형 waive 파일의 연속 클릭 성능은 실측 대상이다.
opt-in UI 연결만으로 GTK pwrite와 같은 비용이라고 주장하지 않는다(M4 §23~30).

GTK `--floe-reviewer`는 표시 이름뿐 아니라 기존 reviewer waive 파일의 읽기 선택에
영향을 준다(`floe/cli.py`, `floe/drc.py`). native `drc`/`render`의 읽기 선택은 있지만,
view에서 이를 `--drc-reviewer` 쓰기 등록으로 단순 치환하면 권한 의미가 달라진다.
M4g-15a는 `view --drc PACK.ice --floe-reviewer TAG`로 **명시 ICE와 인접 파일**만
연결한다. 메모 배지/본문·waive 조회는 가능하지만 edit/transfer/자동 저장은 허용하지
않는다. writer 옵션·별도 waive 파일과 혼용은 오류다. env 태그는 자동 선택하지 않는다.
M4g-20은 승인된 legacy 임시 이름을 인접 파일 다음 후보로 읽는다. 디렉터리 열거,
사용자 지정 경로, 임시 root 확장, 파일 생성/수정/이동/채택은 없다. symlink/FIFO/
hardlink·잘못된 내용은 명시 오류이고 임시 파일로 조용히 폴스루하지 않는다.
이를 “reviewer 전체 이관 완료”라고 세지 않는다([M4 §75](WEBUI_M4.ko.md)).
M4g-21은 명시 reviewer의 ASCII에서 현재 인접 ICE를 선택한다. GTK와 같은 size/
초 단위 mtime 기준이며 stale/corrupt/missing이면 sidecar 없는 ASCII로 돌아간다.
상태는 화면에 표시하고 암묵 색인·캐시 수리·root 확대는 하지 않는다. 선택 뒤 actor
open에서도 원본 일치를 재검사한다. hot reload/내용 해시 기반 revision 보장은 아니다.

## 3. 로컬 기능 완성과 구별할 목표 잔여

1. [G4-MENU-03](WEBUI_G4_MENU.ko.md)의 카메라 유지
   레벨 재선택 구현·검증, 진단/무효 CLI 경계와 G4 목록의 최종 재대조. reviewer 읽기 경로는
   M4g-20/21에서 연결했으며 ambient reviewer 자동 선택은 하지 않는다. 개발 bitmap 슬롯 UI는
   M4g-16d에서 로컬 연결했지만 실제 브라우저 수용은 아래2번에 남는다.
   M4g-17a~c [합성/독립/정적 PNG 표시 진단](WEBUI_DISPLAY_DIAGNOSTICS.ko.md)은 연결했다.
   `--dump`는 브라우저 최근 프레임/합성 화면 보관과 명시적 다운로드로 사용자 결정됐다.
   M4g-22에서 로컬 연결·회귀를 추가했다. GTK 진단/애니메이션 PNG 경계는 남으며
   기존 명령을 폐기하지 않는다. Canvas 검증을 실제 화면/다운로드 수용으로 세지 않는다.
2. 실제 브라우저 입력·저장·복구·화면 수용, Python-free Linux 실행, G1/G4 판정.
   2026-09-16 사용자가 합성 시작 파일·다운로드를 승인한 뒤 재시도했으나 브라우저
   자체 URL 정책이 `file://` 시작 파일을 다시 차단했다. 우회하지 않았고 합성
   서버와 인증 시작 파일을 정리했다. 실제 다운로드 성공으로 기록하지 않는다.
3. M2 공유/원격은 `shares=false`, loopback-only인 **미구현**이다. 허가 없는 원격
   노출/공유 API 확장을 로컬 구현의 자연스러운 연장으로 추론하지 않는다.
4. M0/M3 TeeBox Firefox/ETX는 현장 실행 불가로 보류. 사용자에게 같은 측정을
   반복 요청하거나 로컬 결과를 현장 PASS로 바꾸지 않는다.
5. M5 world-tile은 성능 전제·실측에 따른 조건부 단계. 미구현을 완료로 세지 않는다.
   index hot reload/revision은 사용자가 별도 설계로 유보한 범위다.

따라서 “이번 커밋으로 한 기능 경로를 닫음”과 “웹 전환 전체 완료”를 구별해 보고한다.

## 4. IPC 단발 실패와 별도로 재현한 잠금 잔류

2026-09-16 M4g-17c 최초 전체 배터리에서
`instance::tests::wire_lost_ack_and_replays_do_not_repeat_the_handler`가
`instance/tests.rs:32: already owned`로1회 실패했다(exit101).
같은 작업 트리에서 해당 검사 단독20회와 app-core 전체 병렬 검사3회는 모두 통과했다.
IPC 코드는 이번 PNG 변경에 포함되지 않지만, 이것만으로 환경 문제라고 확정할 수는 없다.
최초 실패와 반복 검증 로그는 M4 §72에 기록한다. 그 전체 재실행 PASS만으로 단발
실패의 원인 규명/수정으로 계산하지 않는다.

M4g-18에서 **소유자 종료와 다른 스레드의 fork→exec가 겹치는 별도 조건**을
재현했다. `Owner::drop`이 close에만 의존하여, 자식의 상속 descriptor가 exec 전까지
같은 flock을 유지했다. dup 단위 검사와 exec 직전 자식을 동기화해 멈춘 native 검사
모두 수정 전 exit101로 실패하고, 명시 `LOCK_UN` 추가 뒤 통과했다([M4 §73](WEBUI_M4.ko.md)).
기존 close-on-exec 검사는 exec 이후만 검사했으므로 이 구간을 포착하지 못했다.

수정은 자기 socket inode 정리 뒤 살아 있는 FD로 unlock하며 lock 파일은 남긴다.
fork된 복사본의 Drop은 creator PID가 다르면 자기 FD만 닫고 부모 socket/잠금을
해제하지 않는다. 저장 PID로 신호를 보내거나 timeout을 새 owner 실행 권한으로
취급하지 않는다. 단발 실패 당시 backtrace가 없어서 첫 claim/재시작 claim 중 어디서
실패했는지는 모른다. **재현된 결함은 수정했지만 최초 실패와의 인과는 미확정**이다.
호출 위치를 남기도록 테스트 helper에 `track_caller`를 추가했으며 재발 시 실제 실패
위치/조건을 기록한다. 반복 PASS로 최초 실패를 환경 탓으로 단정하지 않는다.
