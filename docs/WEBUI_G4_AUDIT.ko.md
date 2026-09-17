# 웹 전환 G4 잔여 감사

갱신: 2026-09-17, 합성 메모/waive 저장·복원과 admission/영수증/경로 수정. 상위 [계획](WEBUI_PLAN.ko.md), 원래 범위
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
M4g-25는 SVRF 교체, M4g-26a/b는 열린 캐시를 유지하는 인덱싱과 카메라 유지 레벨
재선택을 연결했다([M4 §81~84](WEBUI_M4.ko.md)). 메뉴 inventory는 linked39/open0이며
실제 브라우저/현장 수용 및 아래 진단/CLI 최종 대조와는 별개다.

M4g-27의 [CLI 원본 재대조](WEBUI_G4_CLI.ko.md)는10개 명령94개·보조 PNG16개,
숨김 거부17개를 실제 argparse/native parser로 고정했다. 사용자 선택대로 명시
`--refinement on`의 환경 page-round 호환을 복원하고 고정 off 상태 표시를 수정했다.
M4g-28은 양수 stream 인자의 기존 환경 round/독립 창/최종 옵션 충돌도 연결했다.
M4g-29는 APNG **정적 기본 이미지**, M4g-30은 사용자 결정에 따른 `displaytest`
대체 경계를 확정했다. GTK 위젯 진단은 비교 패키지에만 남기며 Rust alias는 없다.
새 byte-stream 알고리즘이나 animation player를 필수 이관으로 잘못 확대하지 않는다.

M2b-4c는 [공유 SH-01~10의 근거/열린 범위](WEBUI_SHARING_ACCEPTANCE.ko.md)를
대조하고 HTTP/wire inventory와 실제 교차 인증 거부 gate를 연결한다. 이는 로컬
권한 회귀 근거이며 실제 브라우저 SH-08·원격 SH-10과 G4 수용을 대체하지 않는다.

M4g-31은 `feature/jobdeck`의 `45c9934`까지 합류하고 source별 occupancy 기본값
(layout off / jobdeck on), `--occupancy-balance`, 새 캐시 이름과 구 이름 읽기,
depth 점유·page frontier를 웹에 맞춘다. CLI 목록은 공개111개·숨김 거부17개,
native parser177회다. [동기화 기록](WEBUI_JOBDECK_SYNC.ko.md)에 검증 근거와
자동 개명의 정책 차이를 남긴다. M4g-32는 사용자 승인에 따라 명시 Index/DRC
Build에서만 개명을 추가한다([계약·검증](WEBUI_CACHE_MIGRATION.ko.md)). 읽기 권한으로
파일 이름을 바꾸지 않는다. 별도 프로세스 reader 조정, 캐시 revision/hot reload,
실칩·실제 브라우저 수용은 추가하지 않는다. 원격 단계(SH-10)는 사용자 보류다.

## 1. 범위별 현재 근거와 남은 일

| 범위 | 로컬 구현/검증 근거 | 아직 닫지 않은 부분 |
|---|---|---|
| CLI 10종·보조 PNG 명령 | `rust/app`, `rust/app-core`; `validate_app_cli.py`, `validate_app_render.py`, `validate_app_jobdeck*.py`, `validate_fe_embed.py` 등. view 옵션/형식 대조 M4 §62; M4g-15a/20/21 reviewer 읽기; M4g-22 브라우저 dump | 무효/개발 옵션의 최종 제품 경계. GDS/gzip은 기존 native 한계이며 새 지원 아님 |
| UI-01 열기·조작 | `launcher.js`, `browse.js`, `index-open.js`, `gestures.js`, `minimap.js`, `app.js`; 실제 startup/IPC·DOM gate. M4g-19 GTK 휠137입력·현재 표시/대기/drop·margin/DPR gate. M4g-26b camera/revision 고정 레벨 재선택·색인 재시도 HTTP/UI | 실제 브라우저 포커스·키·wheel·resize·재접속 수용, G1 input→photon/pacing·장치별 감도 |
| UI-02 pan/margin | `view_controller`, `stream`, `app.js`의 착지 margin/16px 위상·crop/표시 base, controller/stream/client gate | GTK 대비 새 strip·라벨 지연 실제 화면 측정. 덱 margin은 기존에도 미지원 |
| UI-03 레이어·스타일 | Rust 선택/CAS, palette/presets/settings/defaults; GTK 원본 선택·접힘·상속 oracle, HTTP/DOM gate. M4g-16d 슬롯 모델/API·JSON v2·참조 프리셋/16×16 UI와324 GTK/native/web 대조 | 실제 선택·다중 스타일·기본값 게시와 슬롯 pointer/keyboard·포커스 화면 수용([계약/gate](WEBUI_BITMAP_SLOTS.ko.md)) |
| UI-04 pick/snap/측정 | query/inspect/measure/rulers + Rust query/export, 숫자·scene 유효성·stale gate | 실제 포인터/클립보드·시각적 측정 검증 |
| UI-05 입력·복사·종료 | snapshot/session-exit와 단축키 보호. **M4g-13 두벌식 fallback**: `hangul.js`/`drc-notes.js`, GTK 원본 조합 oracle | OS IME와 fallback의 실제 입력·스크롤·키보드/브라우저별 수용. DOM gate로 대체하지 않음 |
| DRC-01 조회·선택 | `app-core/drc`, `web/src/drc`, `drc*.js`; lazy paging/selection/CD/isolation/query gate; M4g-24b/c DRC 교체·런처 reviewer 재연결, M4g-25 같은 reader의 SVRF 원자 교체 | 현장 대형 결과와 실제 브라우저 조작 수용 |
| DRC-02 저장·전송 | reviewer 고정 sidecar, snapshot/prepare/approve·CAS·receipt, notes/waives/transfer HTTP와 UI gate. **M4g-14 확정 시 자동 저장 opt-in**, M4g-15a/20/21 읽기/쓰기 분리·legacy·ASCII/cache | 실제 브라우저 저장/충돌/복구 수용, 대형 sidecar 연속 저장 및 cache 선택 cold-open 비용 실측 |
| EXPORT-01 | Rust capture/mosaic/PNG metadata/clip + snapshot; raster/metadata/DRC-capture gate | 실제 브라우저 copy/download/승인 표시 수용 |
| SYS-01/02 | Rust worker 발견·수거·cache freshness·selfcheck·portable/ELF/고지; native/포장/전송 gate. M4g-17a~c 합성/독립/정적 PNG 진단; M4g-22 최근 수신·합성 bitmap/명시 PNG 다운로드; M4g-29 APNG 정적 기본 이미지; M4g-30 displaytest 대체 확정 | 실제 dump/표시·다운로드 수용, Python-free **Linux에서 실행**, 현장 Firefox/ETX, G4 전체 end-to-end 판정 |

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

1. [CLI 재대조](WEBUI_G4_CLI.ko.md)의 알려진 진단 대체 경계는 M4g-30 사용자 결정으로
   확정했으며 G4 전체 수용은 남는다. [메뉴 목록](WEBUI_G4_MENU.ko.md)은
   M4g-26b부터 모든 범위 내 handler가 로컬 연결됐지만 전체 수용은 아니다. reviewer 읽기 경로는
   M4g-20/21에서 연결했으며 ambient reviewer 자동 선택은 하지 않는다. 개발 bitmap 슬롯 UI는
   M4g-16d에서 로컬 연결했지만 실제 브라우저 수용은 아래2번에 남는다.
   M4g-17a~c [합성/독립/정적 PNG 표시 진단](WEBUI_DISPLAY_DIAGNOSTICS.ko.md)은 연결했다.
   `--dump`는 브라우저 최근 프레임/합성 화면 보관과 명시적 다운로드로 사용자 결정됐다.
   M4g-22에서 로컬 연결·회귀를 추가했고 M4g-29는 APNG 정적 기본 이미지를 연결했다.
   GTK 위젯 진단은 기존 비교 패키지에만 유지하고 Rust는 displaytest로 대체한다.
   Canvas 검증을 실제 화면/다운로드 수용으로 세지 않는다.
2. 실제 브라우저 입력·저장·복구·화면 수용, Python-free Linux 실행, G1/G4 판정.
   2026-09-16 사용자가 합성 시작 파일·다운로드를 승인한 뒤 재시도했으나 브라우저
   자체 URL 정책이 `file://` 시작 파일을 다시 차단했다. 우회하지 않았고 합성
   서버와 인증 시작 파일을 정리했다. 실제 다운로드 성공으로 기록하지 않는다.
   2026-09-17 브라우저 연결 복구 후 실제 Chrome에서 공개 진단 shell 표시와 미인증
   버튼 비활성화를 확인했다([표시 진단 §5](WEBUI_DISPLAY_DIAGNOSTICS.ko.md)). 이어 사용자가
   연 인증된 합성 탭에서 PNG/raw/crop 각각0차이와 화면 표시, reload·Back 후 명시 실행,
   종료 취소를 확인했다(같은 문서 §6). renderer/WS·다운로드·공유·현장 수용은 아직 남는다.
   사용자의 정상 세션 인계를 기존 시작 파일 접근 차단 해제로 간주하지 않는다.
   후속 [실제 owner 브라우저 기록](WEBUI_BROWSER_ACCEPTANCE.ko.md)은 합성 valmini의
   표시·goto·정렬 pan·줌·pick·레이어 전체 숨김/복원과 두 dump PNG의 실제 저장을
   확인한다. input→photon/순간 깜빡임, clipboard·저장/복구·공유 전체 수용은 별도다.
   같은 기록 §4에는 별도 승인한 실제 Follow·Explore의 추적/독립 이동·레이어,
   reload와 revoke 후 화면 지움·reload 차단을 추가했다. 두 초대는 폐기했고
   source/cache는 불변이다. SH-08의 일부 근거이며 DRC 공유·Back/BFCache·실제
   opener/Referrer 헤더 등을 포함한 전체 수용은 남는다.
   §5는 새 합성 owner 세션의 Shift/Cmd 레이어 선택·선택 가시성/색상, 단일 스타일
   취소/적용·reload 유지·재열기 기본값 복원과 두 수동 거리/Undo/Clear를 확인한다.
   그룹 상속·bitmap 슬롯·스냅/자유각·DRC CD·저장/복구는 이 검사에 포함되지 않는다.
   §6은 Native JSON/Calibre layerprops의 실제 다운로드 파일을 확인했다. 최초
   재불러오기는 filechooser의 `Not allowed`로 차단됐다(앱 실패와 구분).
   권한 변경 뒤 §6.1에서는 실제 Native/Calibre 업로드의 앱 거부를 관측했다.
   서버의 charset 대소문자 완전 일치 결함을 native 회귀로 재현하고 수정했다.
   수정 바이너리로 재시작한 Chrome에서 두 파일 각각9레이어 표시·7/0 스타일
   복원을 확인했고 마지막 Native import로 초기 상태를 되돌렸다. 카메라와
   소스/캐시 파일은 불변이다. flat9레이어 기본 패턴의 수용이며 그룹 상속·custom
   bitmap/v2·슬롯 편집의 실제 브라우저 수용으로 확대하지 않는다.
   §7은 승인된 합성 폴더에서8규칙/23오류 ASCII DRC 최초 등록, SVRF8/8 연결,
   두 edge-pair의 좌표/CD, metadata 기반4레이어 격리·9레이어 복원, 잘못된 SVRF
   교체 거부 후 기존 상태 보존을 실제 Chrome에서 확인했다. 기존 DRC의 교체·
   pack 생성·reviewer 재등록·메모/waive 저장·자동 저장 수용은 아직 아니다.
   합성 DRC는 읽기 전용 연결을 유지하고 패널/markers만 닫았으며 입력과 캐시는 불변이다.
   §7.1의 실제 교체는 기본 예산에서 실패했다. 캐시 선택/새 reader의 중복 예약을
   native 회귀로 재현·수정했지만, 수정 바이너리의 브라우저 교체 수용은 남는다
   ([M4 §89](WEBUI_M4.ko.md#89-m4g-34--drc-교체-준비의-중복-예약-제거)).
   별도 합성 reviewer의 실제 저장은 승인됐으며 후속 결과는 §10~10.1에 기록한다.
   [브라우저 기록 §7.2](WEBUI_BROWSER_ACCEPTANCE.ko.md#72-합성-reviewer-저장-수용-준비미실행)의
   준비 체크리스트 자체는 실행 결과가 아니다. §7.3에서 제어 연결이 복구된 기존 읽기 전용
   탭의 Close/Open 복구, 검색/상태/타입/In view 필터, 양방향 오류 wrap와 단축키,
   박스 선택·reload 복원을 확인했다. 첫 worker 실패 원인은 미확정이며 새 교체 수정
   바이너리/저장 수용은 아니다. §8은 실제 Chrome의 PNG/raw/crop 픽셀 차이0과
   screenshot 색 막대/흰 십자 관측이다. 원격 화면·G1/G2 수용으로 확대하지 않는다.
   §9는 depth 숫자/상대/하한/full, 좌표 초안의 단축키 보호·Escape, Tab 모드 순환과
   Shift+Tab 포커스 이탈을 확인한다. q의 Cancel 초기 포커스는 확인했으나 취소 뒤
   서버 리스너 부재로 연결 유지 여부·종료 원인은 미확정이다. 오른쪽 drag와 실제
   저장 세션 인증은 당시 남았고, 입력 전체 수용이나 종료/복구 PASS로 집계하지 않는다.
   §10의 새 인증 세션에서는 단일 오류 메모 수동·opt-in 확정 저장 및 reload 복원을
   확인했다. 입력7파일 불변, 새 합성 note/lock만 생성됐다. waive 조회는 기본 예산에서
   실패하여 native 회귀로 재현했다. M4 §90 수정 후 후속 실제 결과는 §10.1이며,
   다중 선택·충돌/불명확한 결과 복구 등 전체 저장 수용은 여전히 남는다.
   §10.1에서 수정 빌드의 단일 오류 waive 수동 저장·reload 복원과 opt-in 자동 해제,
   파일 상태 복귀도 확인했다. 자동 해제 후 최종 UI 재조회는 고정된 metadata 대기
   문구로 자동 안전 심사가 차단해 남아 있다. 별도 추가 저장이나 읽기 우회는 하지 않았다.
   `317585e`는 현재 read barrier에 맞게 영수증 문구를 구분하며 UI 회귀를 통과했다.
   §10.2에서는 에이전트가 직접 실행한 새 서버에서 메모와0 waived 복원·Reload review,
   입력/sidecar 불변을 확인해 자동 해제 최종 UI 재조회를 닫았다. 새 세션에 이전
   receipt는 없으므로 영수증 문구 전환의 실제 게시 왕복까지 검증한 것은 아니다.
   M4 §92는 별도 native 회귀에서 발견한
   DRC build 부모 경로 alias 오인을 수정하며, 브라우저 pack 생성 수용은 아니다.
3. M2 공유는 `shares=false`, loopback-only다. M2b-1의
   [초대·인증 코어](WEBUI_SHARING_GRANTS.ko.md)와 M2b-2의
   [follow 프레임 전송](WEBUI_SHARING_FOLLOW.ko.md)을 기본 off로 연결한다. 허가 없는 원격
   노출/공유 API 확장을 로컬 구현의 자연스러운 연장으로 추론하지 않는다.
   M2b-0의 [코드 기반 경계 감사](WEBUI_M2_SHARING.ko.md)는 권한표·follow/explore 분리·
   필수 수용 기준 SH-01~10을 추가했다. owner 인증6단위의 재통과는 guest 수용이 아니다.
   전체 서버 상한과 지금의 process-local 자원 관리도 구별한다. 사용자가 기본 off의
   opt-in 로컬 구현을 승인했다. 합성/loopback에서 follow와 explore를 구현하되 원격
   공개·노트 본문·서버 export·게스트 파일 탐색/쓰기는 허용하지 않는다.
   M2b-3a의 [독립 explore 렌더/표시 상태](WEBUI_SHARING_EXPLORE.ko.md)를 연결했다.
   M2b-3b1의 [scoped query/룰러](WEBUI_SHARING_QUERY.ko.md)를 연결했다.
   M2b-3b2의 [명시 DRC 읽기·독립 패널/선택](WEBUI_SHARING_DRC.ko.md)도 native로 검증했다.
   M2b-4a의 [로컬 CLI/발급·기본 guest 화면](WEBUI_SHARING_UI.ko.md)을 연결했다.
   M2b-4b1의 [게스트 DRC 승인/개인 패널·윤곽/이동](WEBUI_SHARING_DRC_UI.ko.md)을 연결했다.
   M2b-4b2의 [범위 제한 레이어 UI](WEBUI_SHARING_LAYERS.ko.md)는 native/CLI·JS·전체
   배터리를 통과했다. 실제 브라우저 수용을 대신하지 않는다.
   M2b-4b3의 [표시 receipt query/룰러·pointer UI](WEBUI_SHARING_QUERY_UI.ko.md)를 연결했다.
   M2b-4b4의 [유계 DRC 순회·마커/box](WEBUI_SHARING_DRC_NAV.ko.md)를 연결했다.
   M2b-4b5의 [이동 확정·CD/자동 순회](WEBUI_SHARING_DRC_CD.ko.md)도 연결했다.
   공유 요구표 최종 대조, 실제 브라우저/원격 수용은 남는다.
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
