# 로컬 실제 브라우저 수용 기록

2026-09-17, `feature/webui`. [전체 G4 잔여](WEBUI_G4_AUDIT.ko.md),
[합성 표시 진단](WEBUI_DISPLAY_DIAGNOSTICS.ko.md),
[공유 SH 수용 범위](WEBUI_SHARING_ACCEPTANCE.ko.md).

이 문서는 실제 브라우저 조작·화면·저장 결과의 근거다. Node/HTTP 모형 게이트와
구분하며, 아래 일부 통과를 G1/G4 전체나 Firefox/ETX·Linux 수용으로 바꾸지 않는다.
문서만 갱신한 관측 단계와 실제 제품 수정·회귀 실행을 구별한다. §6.1은 실제
브라우저에서 드러난 설정 업로드 결함의 수정이며, 로컬 회귀와 화면 수용은 별도다.

## 1. 세션과 범위

- 사용자가 정상 CLI 시작 절차로 연 합성 `valmini.oas` owner 탭을 인계받았다.
  별도 임시 폴더의 source/cache이며 원본은 `tools/gen_valmini.py`의 기존 fixture다.
  7 cells, 11 pages로 jobs 2 색인했고 source는 저장소의 합성 원본과 `cmp`가 일치한다.
- 실행 옵션: `--depth 99 --detail high --jobs 2 --raster-jobs 1 --dump --local-sharing
  --no-open`. UI에서도 depth99/실제 최대2, High1px, Frames/Labels on을 확인했다.
- macOS arm64, 실제 Chrome extension 연결. About의 앱 revision은 `1a16379`,
  web bundle은 `9ff0b84011c8978ed46fa1106c4814189736e8b8`다.
  About의 index/renderd0.12.101은 **기대 호환 버전**이며 실행 도구의 source hash
  증거로 사용하지 않는다. `21a0c22`까지 후속 커밋은 문서만 바꿨다.
- private 시작 파일·session JSON·cookie/storage credential은 읽지 않았다.
  기존 `file://` 접근 차단을 우회하지 않았고, 원격 서버·실제 설계·기존 기본값은
  사용하지 않았다. 공유 flag가 켜졌다는 것만으로 초대를 발급하지 않는다.

## 2. owner 화면과 입력

| 검사 | 실제 관측 |
|---|---|
| 첫 화면 | Local connected, geometry·반복 배열·여러 레이어·라벨 표시. raw/final frame과 margin ready |
| Goto | X200, Y220, view500µm 입력 뒤 표시 상태/좌표 반영 |
| 중앙 pick | `1 selected · overlap 1/2`, PNG에 선택 윤곽 포함. 전체 후보 정확도 오라클은 아님 |
| Right50% pan | X200 → 449.1349480968858µm, Y220/view500 유지. margin crop 표시 |
| Shift+Left10% pan | X449.1349480968858 → 400.69204152249137µm. margin crop 표시 |
| Zoom buttons | view500 → 400 → 500µm, final round1 프레임. 이후 X200/Y220으로 복원 |
| Layers None | 9개 체크 모두 off, 0 pages. screenshot에서 geometry/label/선택 잔상이 없는 검은 뷰 |
| Layers All | 9개 체크 모두 on, 8 pages. screenshot에서 geometry와 라벨 복원 |

pan의 관측 이동량249.134948µm와48.442907µm는 viewport2312device px에서 각각
1152px와224px이며16px 정렬 계약과 맞는다. 입력당 정확히250/50µm를 요구하지 않는다.
Shift+Left 첫 도구 호출은 승인 검토 시간 초과였다. 좌표가 바뀌지 않았음을 다시
읽고 한 번 재시도했다. 이를 제품 입력 지연이나 실패로 집계하지 않는다.

상태줄에 margin crop/no foreground render가 표시됐다는 것은 최종 표시 경로의
근거다. **정지 screenshot과 도구 왕복 시간으로 순간 검은 strip, input→photon,
연속 frame pacing, GTK 대비 성능을 판정하지 않는다.** 이 단계에서는 기록 영상이나
동일 조건 GTK 지연 대조를 하지 않았다. `--dump`의 bitmap 복사도 켜진 검사이므로
성능 baseline이 아니다. 제품 cut·근사 정책의 화질 오라클도 아니다.

## 3. 실제 dump 다운로드

About의 `--dump` 보관이 on임을 확인하고 두 다운로드 버튼을 명시적으로 눌렀다.
UI의 requested 표시만으로 성공을 판단하지 않고 Downloads의 정확한 두 파일을
확인했다. 다른 다운로드 파일을 열거하지 않았고 파일은 사용자에게 남겨두었다.

| 파일 | 픽셀 크기 | 파일 크기 |
|---|---|---:|
| `floe-dump-received-203-4616x2943.png` | 4616×2943 | 1,164,309 bytes |
| `floe-dump-display-235-2312x1471.png` | 2312×1471 | 748,177 bytes |

둘 다 PNG/RGB8/non-interlaced로 식별됐으며 합성 PNG를 실제 디코드해 geometry·
라벨·선택 윤곽을 확인했다. 수신 정보는 margin/raw/generation18, 합성은2개 canvas
layer였다. margin과 합성은 **독립 캡처**이며 같은 순간의 exact-pixel 쌍으로 대조하지
않았다. Download again 재시도, opt-out 도중 인코딩 취소, clipboard, Save view PNG,
정확 clip/settings 저장의 실제 브라우저 수용은 이 두 성공으로 대체하지 않는다.

PNG·시작 파일·source/cache는 커밋하지 않는다. 이 검사로 서버 측 자동 PNG 쓰기나
설계 기본값 게시 권한이 생기지 않는다.

## 4. 실제 Follow·Explore — 합성 layout-only

2026-09-17 사용자가 **현재 가시 레이어의 Follow·Explore 초대 발급, 같은 Chrome의
테스트 탭 사용, 검사 후 폐기**를 승인했다. 위 owner 세션의 9개 레이어만 공유했고
DRC는 포함하지 않았다. 초대는 각각 UI에서 동의 후 한 번 발급하고 `Open guest tab`으로
열었다. 외부로 링크를 보내지 않았으며 초대 credential·cookie/storage 값은 읽거나
문서에 저장하지 않았다. URL의 초대 fragment는 guest 부팅 후 제거된 것을 확인했다.

| 검사 | 실제 관측 |
|---|---|
| Follow 표시·제어 | geometry·라벨과 raw/margin 프레임 표시. 승인 레이어 9개는 읽기 전용, 독립 이동 컨트롤 없음 |
| owner 추적 | owner view500 → 400µm 변경에 Follow 화면도 확대. Follow canvas의 Right 입력 뒤 owner는 X200/Y220/view400 유지 |
| Follow 새로고침 | 새 초대 발급 없이 `Following owner`와 프레임 복원 |
| Explore 이동 분리 | X1000/Y1000/view200µm 요청에 도형 밖 검은 화면, X200/Y220/view400 요청에 도형 복원. owner는 X200/Y220/view400 유지 |
| Explore 레이어 분리 | Hide all: 9개 체크 off·geometry/라벨 없는 검은 화면. 동시에 Follow의 9개 체크와 도형 유지. Show all approved로 복원 |
| Explore 새로고침 | 독립 view200µm로 확대한 화면이 reload 뒤에도 유지. owner는 view400µm 유지. 화면 대조이며 exact-pixel diff 검사는 아님 |
| 두 세션 폐기 | owner의 각 Revoke 후 해당 guest가 종료 안내·`No displayed frame`으로 전환. screenshot에서 이전 geometry가 지워지고 Explore 조작 비활성화 |
| 폐기 후 reload | 두 탭 모두 새 초대를 요구하고 프레임을 복원하지 않음. owner 목록의 활성 초대/게스트 0개 확인 |

Explore의 Goto 입력은 현재 카메라 readback이 아니라 초안이다. reload 뒤 입력이
0/0/빈 폭으로 초기화돼도 독립 화면은 복원된다. 입력값만으로 복원 판정을 하지 않았다.
이 UX 차이는 기록하되 이번 검사를 위해 카메라/입력 동작을 바꾸지 않았다.

보안 관측의 범위도 구분한다. 실제 초대 링크의 `rel="noopener noreferrer"`와
`target="_blank"`는 확인했다. 그러나 브라우저 도구의 제한된 읽기 평가에는
`window.opener`가 노출되지 않아 **실제 opener 단절을 판정할 수 없었다**.
`document.referrer`는 빈 문자열로 관측했지만 네트워크 Referrer 헤더 검사는 아니다.
credential 저장소·교차 인증 경계의 native 게이트를 이 관측으로 대체하지 않는다.
Back/BFCache, 자연 만료, owner 종료·공개 범위 변경에 따른 폐기, 느린 수신자,
게스트 DRC·pick/snap/룰러는 이번 실제 브라우저 검사 범위 밖이다.

검사 후 두 테스트 탭을 닫고 사용자 owner 탭은 X200/Y220/view500µm·9개 레이어 on으로
남겼다. source와 `design.ovm/ovp/ovt` 네 파일의 SHA-256·크기·mtime가 검사 전후
모두 일치했다. 초대 폐기는 이후 접근을 막으며 이미 수신·복사한 픽셀을 회수하는
기능은 아니다. 제품 코드 수정이나 전체 배터리 재실행은 없었다.

## 5. owner 레이어·스타일·수동 측정

2026-09-17 사용자가 새로 연 동일 합성 valmini 세션에서 검사했다. 이전 프로세스는
사용자가 종료했다고 확인했으며, 별도 재시작 시도는 인증 교환 없이120초가 지나
exit0으로 종료됐다. 이 종료들을 제품의 자발적 크래시 증거로 집계하지 않는다.
새 세션의 About은 §1과 같은 앱 revision `1a16379`·web bundle을 표시한다.
`cb82bb4`까지 후속 변경은 문서뿐이다. 비공개 시작 파일이나 credential은 읽지 않았다.

| 검사 | 실제 관측 |
|---|---|
| 범위 선택 | 1/0 클릭 → Shift+3/0 클릭으로 1/0·2/0·3/0, `3 selected` |
| 개별 선택 해제 | Cmd+2/0 클릭으로 1/0·3/0만 남고 `2 selected` |
| 선택 가시성 | Hide selected는 1/0·3/0만 off, 나머지7행은 on. Show selected로9행 모두 복원 |
| 다중 색상 | Style selected에서 Color만 체크해 cyan 적용. 1/0·3/0만 `#00ffff`, 나머지7행 색상 불변. 화면에서 청록 도형 표시 |
| 단일 스타일 취소 | 7/0의 Speckle/1px를 Outline/3px 초안으로 바꾼 뒤 Cancel. 다시 열면 Speckle/1px 유지 |
| 단일 스타일 적용 | 7/0에 Outline/3px 적용 후 final frame screenshot에서 파란 채움 사각형이 윤곽선으로 변경 |
| 새로고침 | 1/0·3/0의 cyan과 7/0의 Outline/3px 유지. 레이어 행 선택은0개로 초기화. 선폭의 픽셀 수를 별도 계수한 검사는 아님 |
| 뷰 닫기·재열기 | 같은 등록 소스를 Open layout으로 다시 열면 기본 색상과7/0 Speckle/1px 복원. depth99·High 유지. fit 이후 X200/Y220/view500으로 다시 맞춤 |
| 키보드 편집 진입 | 7/0 스타일 버튼의 Enter로 편집기가 열리며 복원된 Speckle/1px 확인; Cancel로 닫음 |

수동 ruler는 스냅을 **끈 상태**에서 actual pointer 두 번으로 측정했다. viewport는
1156×735.5 CSS px, canvas2312×1471 device px이고 view 폭은500µm다.

| 클릭 좌표(CSS px) | 계산한 거리 | 실제 UI |
|---|---:|---|
| (500,450) → (900,450) | 400×500/1156 = 173.0103806µm | Length/Δx173.0104, Δy0.0000µm; 수평 치수선 screenshot |
| (650,250) → (650,600) | 350×500/1156 = 151.3840830µm | Length151.3841, Δx0.0000, Δy−151.3841µm; `2 rulers` |

canvas에 포커스가 있는 상태의 `k`는 마지막 수직 ruler만 삭제하고 수평 ruler와
173.0104µm 결과를 남겼다. `Shift+K`는0 rulers, `Escape`는 측정 모드 off로 전환했다.
스냅은 원래 on으로 복원했고, 선택0개·ruler0개·9레이어 on·기본 색상·X200/Y220/view500
상태로 사용자 탭을 유지했다. source/OVM/OVP/OVT의 SHA-256·크기·mtime도 전후 일치한다.
저장/다운로드/공유 기본값 게시, 새 초대 발급, 제품 코드 변경은 없었다.

이는 평면9레이어의 일부 UI-03/04 수용 근거다. 그룹/잡덱의 접힘·상속, 페이지 간 선택,
bitmap 슬롯, 스냅 정확도·Shift 자유각·선택 bbox gap, DRC CD, clipboard와 지연/부하
수용은 별도다. 모형 게이트나 이 두 거리 일치만으로 전체 측정 기능을 완료 처리하지 않는다.

### 5.1 bitmap 슬롯과 오버레이 스크롤바 클릭

2026-09-18. 기존 DRC 세션58385/62804는 보존하고, 에이전트가 합성 valmini를 읽는
별도 loopback 서버64803을 직접 시작했다. DRC/sharing/dump 없이 `FLOE_FILL_EDIT=1`,
jobs2/raster1/budget1024·full/high/goto200,220,500/refinement off다. 이 env로
공유 기본값 패널도 표시되지만 preview/게시/설정 저장·다운로드는 실행하지 않았다.
bitmap Apply는 이 서버의 메모리 상태만 변경한다.

| 검사 | 실제 관측 |
| --- | --- |
| 기본 편집기 | 레이어 선택0개여도 미사용 `diagonal_right_wide` 편집 가능.256셀/32on, roving tab stop1개. 고정 solid/clear는18개 편집 목록에 없음 |
| 키보드/초안 | Space·Right·Enter로 첫 두 셀 on, Ctrl+End로16행16열 포커스. Invert222on, Reset은 처음32셀과 전부 일치 |
| 포인터 | Clear 후 첫 행 드래그는9개 수신 위치만 on. 중간 셀은 비어 있음. GTK 원본 `motion`도 이벤트 위치만 칠하며 보간하지 않음. 연속선 페인트 성공으로 세지 않음 |
| 취소 | Escape 후 재열면 원래32셀. 상태 gen2 유지 |
| 미사용 Apply | Solid 적용 후 성공 안내, 기존 final/margin gen2와 timing 유지. 파일 저장 없음 |
| 참조 | 1/0·3/0에 해당 슬롯 할당. 단일 스타일 읽기에1/0은16행 모두ffff. 슬롯 Clear 후1/0·3/0 둘 다0000, 비참조2/0은Speckle 유지 |
| reload/Reset | reload 뒤 편집 슬롯은0on을 보존. Reset은 직전의0on이 아니라 내장32셀로 돌아가고 Apply 가능 |
| 뷰 변경 | 수정 실행 파일의 별도50880에서 미전송 초안 중 + zoom은 초안 폐기/not replayed 안내. 재열면 내장 기본값 그대로 |

참조 검사 중 단일 스타일 버튼의 일반 click이 열리지 않았다. 키보드 Enter와
버튼 왼쪽 좌표 클릭은 열렸다. screenshot의 우측 오버레이 스크롤바가 버튼과
겹쳤으며 버튼 오른쪽은 목록 끝과 같았다. DOM `elementFromPoint`만으로는 이
native scrollbar 겹침을 검출하지 못했다.

M4g-43에서 `.layers`에16px 우측 content inset을 넣었다. `scrollbar-gutter`에
의존하지 않으므로 구형 브라우저에도 기존 padding 규칙으로 적용된다. 재빌드 후
에이전트가 별도 서버만64803→50880으로 재시작하고 새 비공개 링크로 열었다.
목록이 실제 overflow 중일 때1/0·3/0 버튼의 일반 click이 각각 편집기를 열었고,
DOM 치수로 목록 오른쪽과 버튼 사이16px, screenshot으로 스크롤바 분리를 확인했다.
이것은 macOS Chrome의 수용 근거이며 Firefox·모든 OS/확대율 검증은 아니다.

UI 전체 ES2017/DOM gate와 release app 빌드가 통과했다. 추가 CSS source guard는
여백 규칙의 존재만 고정하며 실제 hit-test 대체가 아니다. source/OVM/OVP/OVT와
기존 note/waive6개 파일 SHA-256은 기존 값과 같고 `valmini.oas.layerprops`는 없다.
새 파일 게시·DRC 저장·v2 파일 왕복은 하지 않았다. 슬롯/그룹 상속·pointer cancel/
Firefox·pixel-exact 렌더 결과 전체 수용은 별도로 남는다.

### 5.2 명시 종료 — 서버는 종료되지만 이전 픽셀이 남음

50880에서 End session 확인창은 Cancel에 기본 포커스를 줬다. Cancel 뒤
Local connected/Live gen4가 유지됐고 다시 열어 명시 End session을 선택했다.
UI는 Session ended/Close this tab으로 바뀌었고 실행 handle은 **exit0**으로 종료됐다.
기존 DRC 서버는 종료하지 않았다.

그러나 screenshot에는 마지막 레이아웃이 그대로 남고 하단에도 Live/margin/직전
perf가 남았다. `app.js::endSession`은 모듈/연결을 멈추지만 `clearBuffers()`와
viewport/status 초기화가 빠져 있다. 기존 `client.test.cjs` 종료 검사는 요청1회/
receipt 보존/버튼 상태만 단언하므로 픽셀 잔류를 잡지 못한다. **열린 제품 결함**으로
추적하며 종료 화면 정리와 regression/실제 재검증이 필요하다. 종료 실패·불명확한
게시의 recovery record 보존 계약은 이 수정과 별개로 유지해야 한다.

M4g-44 수정 후55105에서 에이전트가 합성 서버를 직접 실행했다. Live gen2의
foreground2312×1471 / margin4616×2943 표시를 확인한 뒤 Cancel은 두 버퍼와
연결을 유지했다. 다시 명시 종료하자 screenshot에서 레이아웃이 지워지고
`Session ended` 안내로 바뀌었다. DOM에서도 foreground/margin1×1, query/ruler/DRC
overlay 숨김, perf/margin/viewport 문구 비움, source/Open/Index 비활성화를 확인했다.
실행 handle은exit0이다. 이 **잔류 표시 결함은 수정·실제 재검증 완료**로 갱신한다.
실제 네트워크 장애/종료 응답 불명은 이번 Chrome에서 유발하지 않았으며, 해당 경로의
즉시 정리·recovery 보존 및 늦은 callback 차단은 결정적 회귀 근거로 구분한다.
기존 DRC 서버나 sidecar 저장은 건드리지 않았다.

### 5.3 도형 선택·스냅 좌표 대조와 룰러 버튼 이동

2026-09-18. 기존62804 합성 세션에서 DRC markers off/패널 닫기, 3/0만 표시,
Frames/Labels off, X49/Y31.5/view50µm로 근접뷰를 만들었다. KLayout 읽기 전용
조회에서 DBU0.001µm, LEAF1의18×3µm box와 MID의20µm 간격 배열 및 TOP의
(30,30)µm 배치를 확인했다. 실제 viewport는1156×735.5 CSS px, DPR2다.

실제 클릭 `(627,470)` 뒤 Inspect는 `1 selected · overlap 1/1`, LEAF1/3/0,
`Area 54000000 DBU²`, `Bounds 30000,30000,48000,33000`으로 원본과 일치했다.
레이아웃은 gen14 margin crop이고 screenshot에서도 해당 사각형/위 PATH가 보였다.
이는 **사각형1개의 좌표·면적 대조**다. 모든 후보·비정형·스냅/룰러 수용이 아니다.

다음 단계에서 브라우저 제어가 `Debugger unattached`로 끊겼다. 동일 Chrome의
공식 get/claim 경로도 실패했고 네이티브 창 제어도 창을 얻지 못했다. 서버 handle은
계속 실행 중이고 소스/cache/DRC/기존 sidecar10파일의 SHA-256은 직전 값과 같다.
새 `browser-preview` 파일은 없다. 이 기술적 연결 실패를 제품 선택 결함으로 세지 않는다.

**복원 완료:** 같은 날 연결이 복구됐다. 먼저9레이어/Frames/Labels on,
DRC 패널·Markers on, X200/Y220/view500µm, 선택/룰러0으로 복원하고 다음 검사를 했다.
서버 재시작을 사용자에게 요청하지 않았다.

동일한 근접뷰/배율에서 실제 pointer와 Rust 응답을 대조했다. 원본 box는
(30,30)–(48,33)µm, 다음 배열 멤버는(50,30)–(68,33)µm다.

| 검사 | 실제 결과 |
|---|---|
| 꼭짓점 근처 CSS(420,503) | `vertex · DBU 30000, 30000` |
| 변 근처 CSS(650,436) | `edge · DBU 40003, 33000`; X는 포인터 투영 후 DBU 반올림, Y는 원본 상단 변과 일치 |
| 스냅 ruler (420,503)→(834,503) | DBU(30000,30000)→(48000,30000), Length/Δx18.0000µm, Δy0; 화면 치수선18.0000µm |
| 스냅 ruler (834,503)→(883,503) | DBU(48000,30000)→(50000,30000), Length/Δx2.0000µm, Δy0; 합계2 rulers |

이2µm 검사는 **수동 두 점 스냅**이며 다중 선택의 자동 bbox gap 검사가 아니다.
Shift 자유각·모든 형상/후보·대규모/현장 성능 수용으로 확대하지 않는다.

추가로 probe 결과가 있는 상태에서 Ruler 버튼을 클릭하면 모드가 켜지지 않는 현상을
재현했다. 포인터가 canvas를 떠나면서 `snap-status`가 빈 문자열로 바뀌어 버튼의
Y가486.78125→431.1875 CSS px로 움직였다. Enter로는 정상 진입해 위 측정을 마쳤다.
hover 결과의 출현/소거가 아래 컨트롤을 움직이지 않도록2줄 높이를 예약하고,
긴 좌표/오류는 초점 가능한 영역 안에서 스크롤하도록 고쳤다.

수정본 검증을 위해 에이전트가 별도 합성 서버51199를 직접 시작했다(DRC 등록/쓰기
권한 없음). 브라우저 도구가 `file://` 비공개 시작 파일 접근을 URL 정책으로 거부하여
다른 도구/경로로 인증을 우회하지 않았다. 인증 없는 공개 시작 화면에서 빈 readout의
높이39.1953125 CSS px와 초점/배치를 확인했다. 이는 **수정 후 인증된 hover→버튼 클릭
재검증을 대신하지 않는다**. 해당 실제 재검증은 남기고 테스트 서버는 종료했다.
CSS/접근성 source guard, 전체 JS UI 게이트와 offline/locked release app 빌드는 통과했다.

기존62804는9레이어/Frames/Labels/Markers on, DRC 패널 open, X200/Y220/view500,
선택/룰러0·probe off·ruler snap on으로 다시 복원했다(gen44 margin crop).
원본/cache/DRC/기존 sidecar10파일의 SHA-256은 불변이고 새 `browser-preview` 파일은 없다.
메모·waive 저장/자동 저장 활성화, 기본값 게시, 설정 내보내기는 하지 않았다.

### 5.4 실제 다중 선택·자동 bbox gap·Shift 자유각

2026-09-18. 연결 상태를 새로 확인한 기존62804 합성 탭에서 검사했다. 새 인증 링크,
파일 업로드/저장, 공유 또는 reviewer 권한은 사용하지 않았다. About의 실제 값은
source revision `aa1f0a3f48cd018684ef754af09f7f8d3ad4b09d+`, web bundle
`2a10c3d9254aa79ff5c37d44a08e0d4b1e10bf55`, target `aarch64-apple-darwin`이다.
dirty 표시가 있는 이전 실행 파일의 관측이며 최신 초기화/BFCache 수정이나
§5.3의 CSS 수정 후 수용으로 계산하지 않는다. 기대 native 버전은 실행 worker
source hash의 증거가 아니다.

DRC markers off/패널 닫기, 3/0만 표시, Frames/Labels off, full/High에서 검사했다.
viewport1156×735.5 CSS px, DPR2. §5.3에서 원본과 대조한 두 box는
(30,30)–(48,33)µm와(50,30)–(68,33)µm다.

| 조작 | 실제 UI·화면 결과 |
|---|---|
| X59/Y31.5/view70µm에서 CSS(528,470) 클릭 | LEAF1/3/0,1 selected, bounds30000,30000,48000,33000, area54000000DBU² |
| viewport 중앙을 Shift-click | 2 selected, 마지막 box bounds50000,30000,68000,33000 |
| 같은 중앙을 Command-click | 두 번째만 해제,1 selected와 첫 box bounds 복원 |
| Shift로 다시 추가한 뒤 r | `1 selected bbox gaps`, DBU(48000,31500)→(50000,31500), Length/Δx2.0000µm, Δy0; 실제 화면에 두 box 사이 치수선 |
| X50/Y33/view70µm에서 snap on, CSS(528,519)→viewport 중앙 | 기본 축 정렬: DBU(30000,30000)→(50000,30000), Length20.0000µm |
| 동일 첫 점, Shift를 누른 중앙 클릭 | 자유각: DBU(30000,30000)→(50000,33000), Δx20/Δy3µm, Length20.2237µm; `sqrt(20²+3²)`의4자리 표시와 일치 |
| k, 이어 Shift+K | 마지막 자유각만 제거해20µm/1 ruler, 이후0 rulers |

자동2µm 치수선과 수평20µm/대각20.2237µm 동시 표시는 screenshot으로도 확인했다.
이미지를 읽어 길이를 추정한 것이 아니라 원본 좌표와 Rust가 반환한 DBU/거리 표시를
대조한 것이다. bbox gap을 contour 최단 거리로 부르지 않는다. 두 도형/한 자유각의
검사이며 다중 후보 전체, 비정형 도형, clipboard, 모든
DPR/Firefox/원격 입력과 성능 수용은 남는다.
CD/수동/자동 혼합 Undo는 후속 §5.5에서 별도로 확인했다.

검사 후9레이어, Frames/Labels/Markers on, DRC 패널 open, X200/Y220/view500µm,
full/High/thin auto, 선택/룰러0·probe off·ruler snap on으로 복원했다(gen66 margin crop).
note/waive 자동 저장은 모두 off이며 새 저장 receipt는 없다. 원본/cache/DRC/기존
sidecar10파일 SHA-256은 기존 기준과 모두 같고 `browser-preview` sidecar/lock은 없다.
서버를 재시작하지 않았으며 테스트 탭을 후속 수용을 위해 보존했다.

제품 코드 변경은 없다. 현재 HEAD의 `inspect.test.cjs`와 `measure.test.cjs`도
각각 ALL OK로 재검증했지만, 이 모형 게이트를 실제 브라우저 관측과 혼합하지 않는다.

### 5.5 실제 CD·수동·bbox 간격 혼합 Undo

2026-09-18. §5.4와 같은 기존62804 합성 Chrome 세션에서 파일 저장 없이 검증했다.
실행 파일은 §5.4의 이전 빌드이며 최신 CSS/초기화/BFCache 수용으로 세지 않는다.

두 box의 bbox gap2µm를 먼저 만들고 수동 룰러19.9830µm를 추가했다
(DBU39077,30000→59060,30000). 이어 M2.OVERLAP.2의 global1을 선택해
`Frame error`로 이동하자 이전 룰러2개가 유지되고 `Gap 0.0116 µm` CD가 추가됐다.
형상 선택은 격리된 레이어 상태에 맞게 해제됐다. snap off로 새 수동 룰러
0.5449µm를 추가해 총4개를 만든 뒤 실제 canvas 키 입력으로 확인했다.

| 입력 후 | 남은 기록 | UI 확인 |
|---|---|---|
| 최초 | bbox → 수동19.9830 → CD → 수동0.5449 | `4 rulers` |
| k 한 번 | bbox → 수동19.9830 → CD | `3 rulers`, CD 값 유지 |
| k 두 번 | bbox → 수동19.9830 | `2 rulers`, `CD rulers cleared.` |
| k 세 번 | bbox | `1 rulers`, bbox gap2.0000µm |
| k 네 번 | 없음 | `0 rulers`, 치수·bbox·CD 목록 비움 |

CD를 다시 만든 뒤 수동0.5449µm를 추가해 `Clear CD rulers`를 눌렀을 때도
수동 기록1개와 끝점/거리 표시는 그대로 남았다. Shift+K로 마지막 기록을 지웠다.
이는 완료된 CD 한 선분을 섞은 실제 수용이다. 지연/실패 응답, 여러 CD 선분의
실제 입력 경합 전체까지 확대하지 않는다. 현재 HEAD의 `rulers`, `measure`,
`drc-cd`, `drc-navigation` Node 게이트4개는 별도로 모두 ALL OK다.

검사 후 레이어 복원/DRC focus 해제, 9레이어·Frames/Labels/Markers on,
X200/Y220/view500µm, DRC 패널 open, 선택/룰러0, ruler mode/probe off,
ruler snap on으로 복원했다(gen88 margin crop). note/waive 자동 저장은 off다.
원본·캐시·기존 리뷰10파일 SHA-256 불변, 새 `browser-preview` sidecar/lock 없음.
제품 코드 변경과 새 저장/인증/권한 부여, 사용자에게 서버 재시작 요청은 없었다.

## 6. 설정 다운로드 — 최초 불러오기 권한 차단

2026-09-17 사용자가 새 합성 valmini 세션을 열고, 다운로드한 두 설정 파일을 같은
loopback 세션으로 다시 불러오는 검사를 명시 승인했다. 실제 설계·설계 기본값·외부
서버는 범위 밖이다. `9f8544a`까지 후속 변경은 문서뿐이다.

실제 Save settings 버튼으로 두 형식을 각각 다운로드했다. requested 상태만으로
완료를 판단하지 않고 Downloads의 해당 파일 두 개만 확인했다.

| 파일 | 크기 | 확인한 내용 |
|---|---:|---|
| `floe-layers.json` | 716 bytes | `floe.layers`, version1, groups0, rows9. 색상은 초기 UI와 일치하고 모두 visible, fill/width는 null |
| `floe-layers.layerprops` | 262 bytes | 주석 외9행. 같은 layer/datatype·색상·이름과 speckle/가시성1/선폭1 |

이 평면 fixture의 native 출력은 version1이었다. bitmap 슬롯·참조·그룹 상속이나
version2 직렬화 수용을 이 결과로 대신하지 않는다. 두 파일은 사용자 Downloads에
남겼으며 저장소에는 추가하지 않았다.

1/0을 숨기고7/0에 Outline/3px를 적용한 뒤 편집기를 다시 열어 변경값을 확인했다.
이후 Load settings의 실제 filechooser로 JSON을 선택하려 했으나 브라우저 도구가
`Not allowed`를 반환했다. 앱의 Settings applied 응답은 관측하지 못했다. 확장
프로그램의 파일 URL 접근 설정을 확인하도록 안내했으며 우회 업로드나 권한 자동
변경은 하지 않았다. **이 최초 시도의 JSON/Calibre 불러오기·roundtrip은 미검증**이고,
앱 importer의 실패 증거로도 집계하지 않는다. 이때 Calibre 업로드는 시도하지 않았다.

뷰를 닫고 같은 등록 소스를 다시 열어9레이어 on·초기 색상·7/0 Speckle/1px를
복원했다. X200/Y220/view500µm, depth99·High·Frames/Labels on·선택/룰러0·snap on,
Local connected/final frame 상태를 확인하고 사용자 탭과 서버는 유지했다.
이는 파일 import를 통한 복원이 아니다. source/OVM/OVP/OVT의 SHA-256·크기·mtime는
전후 일치한다. 제품 코드 변경이나 전체 배터리 재실행은 없었다.

### 6.1. 권한 변경 뒤 발견한 XHR charset 호환성 결함

사용자가 확장 프로그램의 파일 URL 접근을 허용한 뒤 같은 합성 세션에서 다시
검사했다. filechooser가 두 파일을 모두 전달했지만 Native JSON과 Calibre 각각
`The requested value or selection is not supported.`로 실패했다. JSON 시도 뒤
1/0은 off인 채로 남았고 설정 버튼은 다시 활성화됐다. 다운로드 파일의 SHA-256은
§6에서 저장한 것과 같았다. 파일 선택 거부와 앱의 HTTP 거부를 구별한다.

서버 `settings::prepare`가 Content-Type을 `text/plain; charset=utf-8`과 대소문자까지
완전히 일치시켰다. [Chromium XHR 구현](https://chromium.googlesource.com/chromium/src/third_party/+/f4bee3533965ef933f7b75f0a365c90b5f75d922/blink/renderer/core/xmlhttprequest/xml_http_request.cc)은
문자열 body를 보낼 때 charset 값을 `UTF-8`로 교체한다. 기존 raw HTTP gate와
JS 모형은 이 브라우저 변환을 재현하지 않았다. 새 native 회귀에서 uppercase 헤더의
JSON 요청은 수정 전 **400 invalid_request / exit101**로 실패했다.

수정은 단일 헤더 값의 ASCII 대소문자 비교만 완화한다. text/plain·UTF-8 인코딩,
단일 헤더·기존 파라미터 형식, 인증/Origin/CSRF·4MiB cap·준비 토큰·CAS는 유지한다.
다른 인코딩/미디어 타입, charset 누락·중복·접미사, 복수 헤더를 허용하지 않는다.
Native/Calibre 각각 uppercase·mixed-case 허용과7종 부적합 값 거부를 추가했다.
Chromium형 헤더로 Calibre 적용과 Native 복원도 실행하며 기존 lowercase 경로를
유지한다. 수정 후 집중 owner settings gate는 exit0으로 통과했다(재전송/stale·
custom bitmap/v2·파일 불변 포함). `validate_owner_service.py`의21개 native 검사와
`node tools/validate_web_ui.cjs`도 통과했다.

`sh tools/validate_rust.sh`는 macOS arm64·jobs2·offline·전용 합성 TMPDIR에서
실행했다. 아래 native 테스트 실행이 각각30초 제한으로 중단됐다. 제한을 늘리거나
게이트를 생략하지 않았고, 같은 제한의 재실행은 모두 통과했다.

| 중단된 검사 | 동일 제한 재실행 결과 |
| --- | --- |
| `validate_layerprops.py` | 72 documents·980 styles·4 native view models 통과(native test0.11s) |
| `validate_layer_defaults.py` | 20 GTK targets/bytes·native publication 통과(native test0.27s) |
| `validate_layer_palette.py` | 12,096 batch·32 order·7,776 click cases 통과 |
| `validate_web_startup.py` | 144 startup·380 stream cases,22 native launch cases 통과 |

세 번째 전체 실행의 전반부, palette부터 원래 스크립트의 재개 구간, startup부터
마지막까지의 재개 구간을 합쳐 **원래 배터리 전 항목을 통과**했다. 마지막 구간은
exit0과 `RUST VALIDATION: ALL OK`로 종료했으며 jobdeck83·renderer46검사와
KLayout jobs1/8 각각13 PX·2 phase-exact·14 style 검사를 포함한다. 이것은
**분할 재실행 결과이지 단일 전체 실행 PASS가 아니다**. 최초 시간 초과의 원인은
미확정이며 재실행 성공으로 해결됐다고 판단하지 않는다. 수정한 두 Rust 파일의
`rustfmt --check`와 `git diff --check`도 통과했다.

사용자가 수정 실행 파일로 서버를 재시작하고 Chrome에서 연 새 loopback 세션
(`127.0.0.1:57643`, revision `7467c59+`, bundle
`18deb131059086ea60eee0ea8031a52e638cad78`)에서 실제 filechooser로 재검사했다.
§6의 다운로드 파일을 그대로 사용했고 각 변경·복원을 UI 상태와 화면으로 확인했다.

| 형식 | 적용 전 의도적인 변경 | 실제 불러오기 결과 |
| --- | --- | --- |
| Native JSON | 1/0 off, 7/0 Outline·3px | `Settings applied · 9 rows read`;9레이어 on,7/0 Speckle·1px, geometry 복원 |
| Calibre layerprops | 2/0 off, 7/0 Outline·5px | 같은9행 적용 응답;9레이어 on,7/0 1px·`aaaa/5555` 교대16×16 패턴, geometry 복원 |

Calibre의 named `speckle`은 기존 importer에서 named fill slot의 bitmap으로
해석되므로 편집기에는16×16 pattern으로 보인다. 원래 Native의 builtin Speckle과
표현 방식이 다른 기존 계약이며, 화면 확인을 비트 단위 픽셀 오라클로 간주하지 않는다.
마지막에는 Native JSON을 다시 import해 초기 상속 상태와 Speckle·1px를 복원했다.
재열기가 아니라 파일 import로 복원한 결과다. 세 번 모두 카메라는
X200/Y220/view500µm를 유지했고 Local connected·final frame, depth99·High,
Frames/Labels on·선택/룰러0 상태로 탭과 서버를 열어 두었다.

소스·OVM/OVP/OVT의 SHA-256·크기·mtime와 다운로드 두 파일의 SHA-256은 전후
일치한다. 권한 자동 변경·우회 업로드·설계 기본값 게시·실제 설계 사용은 없었다.
이 결과는 flat9레이어·기본 패턴의 실제 다운로드→import 수용이다. 그룹 상속,
custom bitmap/v2·슬롯 편집은 집중 HTTP 게이트와 구별하며 실제 브라우저 수용으로
확대하지 않는다.

## 7. 실제 읽기 전용 DRC 열기·SVRF 교체·CD/레이어 복원

2026-09-17, §6.1과 같은 Chrome/57643 세션에서 후속 검사했다. 제품 코드는
`cb4a698`이며 열린 바이너리는 §6.1의 수정 빌드다(커밋 뒤 서버 재시작은 없음).
파일 선택기가 허용한 기존 합성 폴더 안에 새 `drc-ui.Z8ykGA` 하위 폴더를 만들고
아래 작은 fixture를 생성했다. 실제 설계·기존 reviewer 파일은 사용하지 않았다.

```sh
python tools/gen_drcdb.py TEST_DIR/synthetic.db \
  --checks 8 --max-errors 7 --zeros 1 --precision 40000 \
  --die 120,160,280,280 --seed 42 \
  --svrf TEST_DIR/synthetic.svrf --pathname synthetic.svrf \
  --layers M1,M2,M3,M4,V1,V2,CT,GT \
  --svrf-gds M1=1/0,M2=2/0,M3=3/0,M4=4/0,V1=5/0,V2=6/0,CT=7/0,GT=8/0,FILLA=63/63,FILLB=8/0
rust/target/release/floe2-web svrf TEST_DIR/synthetic.svrf \
  --no-env-switches --out TEST_DIR/synthetic.rules.json
```

`TEST_DIR`는 새 합성 디렉터리로 치환한다. generator는 검증 도구이고, 제품의
DRC 읽기·SVRF 변환·브라우저 서비스는 Rust다. ASCII는3,616bytes,8규칙·23오류·
4 admin section이다. 첫 규칙과 마지막 규칙은0오류이며 admin section은 규칙 목록에
나오지 않았다. DRC source hash는
`3e9c9015089d8647fb1031640385314884857a87cd9b0459b423f884da829988`이다.

| 검사 | 실제 Chrome 관측 |
| --- | --- |
| 최초 DRC 등록 | Open DRC results → 승인 폴더 → synthetic.db → Open selected DRC (read-only). `8 rules · 23 errors · ASCII`, `NO REVIEW WRITES`, no ICE cache 표시 |
| 열 때 레이아웃 | 9레이어·depth99·High 유지. 중심 X200/Y220 유지. 오른쪽 패널 때문에 화면 폭2312→1672device px, view500→361.5916955µm: 같은 배율의 crop이며 동일 view 폭 유지라는 뜻은 아님 |
| 빈 규칙·목록 | M1.SPACE.1은 No matching errors. M2.OVERLAP.2는 global1/2 두 오류. 단일 클릭은 `Global 1 · 4/4 vertices`를 표시하고 카메라는 유지 |
| Frame error·Next | global1 중심254.1032875/183.4981µm, global2 중심141.5042375/178.294575µm. 원본 edge 좌표의 bbox 중심과 일치하며 CD 각각0.0116/0.0189µm 표시 |
| SVRF 연결 | Load SVRF metadata → synthetic.rules.json. `SVRF 8/8 matched`,5분류 표시. 카메라·레이어는 유지하고 기존 focus/CD 초기화 |
| 규칙 비교·격리 | M2 첫 오류로 이동: `measured 0.011575 vs < 0.025µm`, Δ≈−0.013425µm. source_gds인2/0·3/0·8/0·63/63만 on, 나머지5레이어 off |
| 잘못된 교체 | JSON 대신 생성한 원본 synthetic.svrf를 선택하면 명시 거부. 모달을 닫은 뒤8/8 metadata, global1 focus/CD, 동일 카메라·4레이어 격리가 유지됨 |
| Restore layers | 최초9레이어 on으로 복원, 오류 focus와 CD 해제. 이 작업은 카메라를 원위치로 되돌리는 명령은 아님 |

CD 비교는 source의 정수 좌표 차이 `463/40000=0.011575µm`,
`757/40000=0.018925µm`와 직접 대조했다. 부동소수점 상세 표시에는 미세한 잔차가
있고 화면의 짧은 라벨은 반올림된다. 이 두 edge-pair의 일치를 모든 CD 종류·스냅·
sign-off 정확도의 수용으로 확대하지 않는다. 분류 목록은 확인했지만 모든 필터
조합·대형 페이지 순회·키보드/box 그룹 선택은 이번 검사 범위가 아니다.

오류 메시지는 범용 파일 선택 안내여서 SVRF JSON 설명은 아래 고정 도움말을 함께
읽어야 했다. 파일 선택기의 AX checkbox는 Playwright role 조회와 맞지 않아 한 번
`no_matches`였고, 실제 AX 행을 다시 읽어 선택했다. 이를 앱의 파일 읽기 실패로
집계하지 않는다. 자동화 왕복 시간으로 제품 지연을 측정하지 않았다.

마지막에 DRC markers off·focus/CD 정리·패널 접기 후 X200/Y220/view500µm,
9레이어 on·0 rulers·Local connected/final frame으로 복원했다. **합성 DRC와
SVRF는 읽기 전용으로 연결된 채 남아 있다.** 이전에 없던 DRC 등록까지 없앴다고
표현하지 않는다. `Build pack`·reviewer 재등록·메모/waive·자동 저장은 실행하지
않았다. `Saving review state`는 세션의 패널 상태 동기화이며 review sidecar 게시가
아니다.

검사 전후 source·OVM/OVP/OVT의 SHA-256·크기·mtime, 합성 DRC의 SHA-256·크기·
mtime, metadata JSON의 SHA-256이 같다. 새 테스트 폴더는 generator/변환기가 만든
DB·SVRF·INCLUDE·JSON4파일뿐이며 ICE/리뷰/lock 파일이 생성되지 않았다. 제품 수정이
없는 실제 UI 수용 기록이므로 §6.1의 배터리를 다시 실행한 것으로 기록하지 않는다.

### 7.1 후속 DRC 교체: 실제 실패 발견과 자원 예약 수정

같은 열린 바이너리에서 별도 합성 `replacement.db`(3규칙·5오류·1,328bytes)를
선택했다. 선택 뒤 Close는 기존8규칙·23오류와 SVRF8/8을 보존했다. 하지만 실제
Open selected DRC는 두 번 모두 `Catalogue or owner is busy…`로 실패했다.
모달을 닫으면 기존 DRC/SVRF·카메라·9레이어가 그대로였다. **새 DRC 교체 성공을
실제 Chrome 수용으로 기록하지 않는다.** 잘못된 DRC 파일을 여는 검사는 아직 안 했다.

native 회귀로 같은 실패를 재현했다. `prepare_open`이 캐시 선택용 예약을 유지한
채 실제 reader를 시작해 새 reader용256MiB/CPU1을 두 번 잡았다. 기본 렌더1024 +
picker192 + 기존 reader256 + SVRF256 + 선택256 + 새 reader256 =2240MiB로
기본2048MiB 풀을 넘었다. 기존 HTTP 검사는 렌더64MiB여서 이 조합을 놓쳤다.

수정은 cache/source 읽기 lease를 단계 전체에 유지하고, 임시 pack metadata가
해제되는 캐시 선택 끝에서 그 단계의 CPU/메모리 예약만 해제한다. 실제 reader는
기존대로 별도 예약을 받아 시작한다. 이때 피크는1984MiB이며 예산 상향이나 무예약
parse가 아니다. 이전 reader는 새 reader 준비·원자 교체 전까지 유지한다.
테스트/실행 기록은 [M4 §89](WEBUI_M4.ko.md#89-m4g-34--drc-교체-준비의-중복-예약-제거)에 둔다.

열린 Chrome은 재시작하지 않았으므로 수정 후 브라우저 재검증은 남는다. 검사 후
DRC 패널을 접고 X200/Y220/view500µm·9레이어·0룰러·gen32 final crop 상태로 복원했다.
원본/OVM/OVP/OVT·기존 합성 DRC/SVRF JSON·교체 후보의 hash는 불변이며,
새 후보 폴더에도 pack/review sidecar는 생성되지 않았다.

### 7.2 합성 reviewer 저장 수용 준비(미실행)

사용자는 별도 임시 합성 DRC 세션에서 테스트 reviewer만 등록하여 메모·waive의
수동 저장, 자동 저장 opt-in, 재불러오기를 실제 Chrome으로 확인하도록 승인했다.
새 임시 폴더에 valmini·8규칙/23오류 DRC·native pack·SVRF metadata와 실행 스크립트를
준비했다. 실행 파일은 `595aba6` 수정 후 빌드이며, 기존 읽기 전용 탭과 분리한다.
새 시작 파일의 인증값은 읽지 않고 사용자가 연 탭의 포트만 인계받는다.

- 런처: `--drc .synthetic.db.tray --drc-rules synthetic.rules.json`
  `--drc-reviewer browser-test --drc-edit-waives`, jobs2/raster-jobs1/budget1024,
  refinement off. 공유·기본값 게시 권한은 추가하지 않는다.
- 쓰기는 새 합성 폴더에서 pack/reviewer로 유도되는 note/waive sidecar와 해당
  lock만 대상으로 한다. 실제 설계·기존 리뷰·공유 기본값·외부 서버는 제외한다.
- 입력 OASIS/cache/DB/pack/metadata의 fingerprint를 전후 대조하고, 저장 결과는
  UI receipt와 해당 sidecar의 내용 모두로 확인한다. requested/preview 표시만으로
  저장 성공이나 reader 갱신을 판정하지 않는다.

다음 표는 **아직 실행하지 않은 수용 체크리스트**다.

| 검사 | 확인할 조건 |
|---|---|
| 초기 상태 | reviewer가 browser-test이며 notes/waives 자동 저장 모두 off, 새 sidecar 없음 |
| 수동 메모 | 선택 오류에 테스트 문구 입력·미리보기만으로 파일 미생성; 명시 승인 뒤 해당 선택에만 저장 |
| 수동 waive | 선택·미리보기는 저장하지 않음; 승인 후 sidecar 상태와 reader 적용 결과를 따로 확인 |
| 취소 | 미리보기 취소/초안 폐기가 기존 저장 내용·비선택 오류를 바꾸지 않음 |
| 자동 저장 opt-in | 토글을 켜기만 해서는 저장하지 않음; 메모 확정/waive 변경 확정 때만 저장 |
| 재불러오기 | 새로고침 후 저장 메모·waive는 재조회되지만 두 자동 저장 opt-in은 off로 복귀 |
| 종료 상태 | 남은 초안·선택 정리, 자동 저장 off; 합성 파일은 증거로 남기되 저장소에 커밋하지 않음 |

연결 확인에서 기존 합성 탭은 열려 있지만 제어 도구가 `Debugger unattached`로
응답했다. 재연결 시도도 같아 UI 검사/저장을 진행하지 않았고 사용자에게 새 세션
포트와 브라우저 연동 재활성화를 요청했다. 이는 제품 저장 결함이나 수용 PASS가
아니다. 브라우저 인증 파일을 다른 도구로 읽어 우회하지 않는다. 충돌·불확실 응답
복구·프로세스 재시작 및 OS IME는 위 기본 roundtrip과 별도 수용 항목으로 남긴다.

### 7.3 읽기 전용 DRC 필터·순환·선택 복원

다음 확인에서는 브라우저 제어 연결이 복구됐다. 새 reviewer 세션이 아니라 기존
읽기 전용 탭을 사용했다. About은 앱 `7467c59+`, web bundle
`18deb131059086ea60eee0ea8031a52e638cad78`을 표시하므로 `595aba6`의 DRC 교체 수정
수용으로 세지 않는다. 시작 시 `Disconnected / renderer failed`였지만 해당 포트의
gateway가 LISTEN 중인 것을 확인했다. 화면 안내대로 Close → Open layout을 실행하자
Local connected·새 generation1·도형이 복구됐고 기존8규칙/23오류·SVRF8/8 연결도
유지됐다. **최초 worker 실패의 원인은 미확정**이며 제어 도구의 연결 실패와 같은
원인이라고 단정하지 않는다. 서버 종료·재시작·새 인증값 읽기는 하지 않았다.

X200/Y220/view500µm로 맞춘 뒤 DRC 패널을 열었다. 같은 배율을 유지하면서 실제
폭은361.5916955µm가 된다(§7과 같은 viewport 축소). 아래는 이 작은 fixture에서
실제 UI를 조작한 결과이며 대형 페이지/연속 부하 검사는 아니다.

| 검사 | 관측 |
|---|---|
| 규칙 검색 | `M2.OVERLAP` 제출 → M2.OVERLAP.2 한 행; 선택하면 오류2건 |
| 상태 필터 | Waived → 규칙/오류0건; Not waived → 같은 규칙/오류2건. 파일 쓰기 없음 |
| 버튼 순환 | Global1에서 Next →2 →1, Previous →2. 현재 규칙의 양끝 wrap |
| 키보드 순환 | 오류 행에 포커스를 둔 `.` →1, `,` →2. 순회는 카메라를 움직이지 않음 |
| 타입 필터 | 검색/상태 초기화 후 density → M4.DENSITY.W.4 한 행, 선택하면 오류4건 |
| 빈 Selected | M2 규칙에서 선택0개·Selected on → 오류0건 |
| 박스 선택 | Selected off, Box select on 뒤 두 canvas 모서리 클릭 → 이 규칙2개/전체2개 선택 |
| Selected 복원 | Selected on → 오류2건; 브라우저 reload 후 M2·Selected on·선택2개·오류2건 유지 |
| 선택 지우기 | Clear rule selection → 선택0개, Selected 결과0건 |
| In view 추종 | Selected off·In view on에서2건; X1000/Y1000/view200으로 이동하면0건; 기준 뷰로 복귀하면2건 |

박스의 두 클릭은 screenshot 좌표(CSS px) `(470,440)`·`(930,650)`이다. 양 오류의
위치는 §7의 source 좌표와 일치한다. 이 검사는 Shift/Cmd 그룹 연산이나 페이지를
넘는 선택·hover·스크롤 성능을 증명하지 않는다. 팝업의 End/Return 첫 시도는 옵션을
바꾸지 않아 관측된 Waived 메뉴 항목을 직접 선택했다. 이를 제품 실패로 세지 않는다.

끝에 In view/Selected/Box select/Markers off, 검색 없음·All types/All statuses·
첫 규칙으로 정리하고 패널을 접었다. X200/Y220/view500µm·9레이어 on·기본 색상·
depth99/High·Frames/Labels on·선택/룰러0·Local connected·gen12 margin crop를
AX와 screenshot으로 확인했다. 원본/OVM/OVP/OVT/DRC DB/SVRF JSON의 SHA-256은
검사 전후 일치하며 DRC 폴더는 기존4파일만 있다(pack/review/lock 미생성).
제품 수정이나 전체 배터리 재실행은 없었다. §7.2의 저장 검사는 여전히 새 합성
reviewer 세션 인계를 기다린다. 브라우저 제어 연결 자체는 더 이상 차단 조건이 아니다.

## 8. 실제 Chrome 표시 진단

§7.3과 같은 세션의 About → Run display test를 명시 실행했다. fixture는
`gtk-four-bars-v1`, DPR2다. Canvas readback 결과는 PNG57,600픽셀·raw57,600픽셀·
crop/overlay40,960픽셀 각각 `different_pixels=0`이었다.

screenshot에서도 A/B의 검은 바탕 위 빨강·초록·파랑·노랑 막대, C의 잘린 막대와
가운데 흰 십자를 확인했다. 실제 관측 뒤 `All three panels look correct`를 선택해
`screen_observation=all_visible`이 됐으며 `desktop_acceptance=unverified`는 유지됐다.
원격 화면·Firefox/ETX·색 관리·input→photon/pacing이나 native renderer/WS 정확도의
검증으로 확대하지 않는다. 파일 다운로드·서버 게시·설계 변경은 없고 About을 닫았다.

## 9. 실제 키 입력·초안 보호 — 일부 완료

§7.3의 기존 합성 탭에서 source·카메라·레이어 설정을 유지하며 실제 키 입력을
전달했다. source 최대 depth는2다. 다음은 DOM/AX의 적용 값으로 확인한 결과이며,
단순 요청 직후의 이전 값/Rendering 상태를 완료로 집계하지 않았다.

| 검사 | 관측 |
|---|---|
| 숫자 depth | canvas `1` → depth1·Baked depth1·최종3pages |
| 상대 depth | `>`로1→2; 다시 `>`는2 유지; `<`는2→1 |
| 하한 | `0` 적용 뒤 `<`도0 유지·Baked depth0 |
| full 입력 | 개별 실제 `9` 두 번 후 depth가 `full`로 변경 |
| 좌표 편집 보호 | X 초안에 `987654`, Left, `fb1` 입력 → `98765fb14`; 카메라 이동·frames/mono 토글·depth 변경 없음(gen29 유지) |
| 초안 취소 | X 입력의 Escape → 현재 중심200µm로 복원; Y220/view500 유지 |
| Tab 순환 | canvas에서 Tab 세 번 → Hide other errors → Hide all → All |
| 포커스 이탈 | canvas Shift+Tab → Go 버튼(`goto`)으로 이동 |
| 종료 확인 진입 | canvas `q` → 확인 dialog, Cancel에 초기 포커스. 즉시 종료하지 않음 |

캔버스의 generic role 조회와 비입력 요소의 `pressSequentially`는 도구가 거부했다.
화면의 실제 `viewport` 요소를 재확인한 뒤 개별 key press로 검사했으며, 도구 실패를
앱 키 입력 실패로 계산하지 않는다. `99`의 정확한 이벤트 간격·경계1초를 측정한
검사가 아니라 그 단축키의 실제 full 적용을 확인한 것이다. Tab 검사는 모드 선택
값의 순환이며 이때 룰러/선택이0개여서 모든 overlay 종류의 픽셀 숨김을 증명하지 않는다.
오른쪽 버튼 drag는 제공된 자동화 API로 전달할 수 없어 박스 줌의 실제 수용은
계속 미검증이다. DOM에서 이벤트를 합성해 물리 조작 수용을 대신하지 않았다.

X200/Y220/view500·depth99·Frames/Labels on·Mono off·Overlays All로 복원했다.
이후 종료 확인의 Cancel에 Enter를 보냈고 dialog는 닫혔지만 화면에 `Local service
is unavailable`가 나왔다. 이때 기존57643과 새 테스트56087 모두 LISTEN 소켓이
없었다. **Cancel이 서버를 종료시켰다고도, 취소 후 정상 연결이 유지됐다고도
판정하지 않는다.** 종료 원인/시점의 인과는 미확정이고 취소 경로의 실제 재검증을
남긴다. 서버 재시작이나 다른 프로세스 종료는 이 검사에서 수행하지 않았다.

사용자가 새 저장 세션의56087 포트를 인계했으나 해당 탭에는 `Launch with
floe2-web view and use its private session link`가 표시돼 아직 인증되지 않았다.
비공개 JSON의 전체 URL을 사용자가 직접 열도록 요청했으며 인증값은 읽지 않았다.
이후 그 탭도 사라져 §7.2 저장 roundtrip은 시작하지 못했다. 새 세션 입력7파일의
기준 SHA-256을 수집했으며 예정된 두 reviewer sidecar는 없는 상태였다.

### 9.1 내장 두벌식 — 실제 키·커서·초안 폐기

2026-09-18. §10.9의 보존된62804 합성 세션/`browser-preview` reviewer에서 검사했다.
서버를 교체하지 않았으며 notes/waives 자동 저장은 모두off다. Global1만 그룹 선택하고
Read selected notes로 빈 초안을 연 뒤 실제 키를 전달했다. textarea 값 주입만으로
한글 조합 성공을 판정하지 않았다.

| 조작 | 실제 결과 |
|---|---|
| Shift+Space 뒤 `g k s r m f` | `한글`, caret2; screenshot에서도 한글 표시 |
| Backspace3회 | `한그` → `한ㄱ` → `한` |
| `글` 재입력, Left, `r k` | `한가글`, caret2 |
| Shift+Left로 가 선택, `s k` | `한나글` |
| Shift+Space로off, 끝에서 ` qfb1.,` | `한나글 qfb1.,`; 종료창 안 열림, full/Frames on/Mono off/gen4 유지 |
| emoji 초안 `🙂` 입력 후 모드on, `r k` | `🙂가`, UTF-16 caret3 |
| Escape·재열기 | 초안 폐기, 빈 텍스트와 내장 모드off. 체크박스로on도 재확인 |

끝에는 Discard draft·Clear rule selection으로0선택/초안없음, saved-note revision0,
자동 저장off·Local connected/Live gen4로 복원했다. 미리보기·파일 저장/다운로드·
클립보드·자동 저장 opt-in은 실행하지 않았다. 소스1개·cache4개·DB/pack/metadata·기존
note/waive 등 총10파일의 SHA-256은 전후 일치하고 `browser-preview` sidecar/lock은 없다.
이번 검사는 내장 fallback의 부분 수용이며 OS IME composition/후보창·Firefox/ETX나
장문·붙여넣기·원격 키 지연의 검증은 아니다. 제품 코드 변경/전체 배터리 재실행은 없다.

## 10. 실제 Chrome reviewer 메모 저장·복원과 waive admission 결함

사용자가 전체 비공개 링크로 연 새56444 세션에서 Local connected·OWNER REVIEW를
확인했다. About은 앱 `595aba6`을 표시했다. 승인된 별도 합성 valmini/ICE/SVRF,
reviewer `browser-test`, renderer1024MiB, decode2/raster1 조건이다. 인증 JSON이나
bootstrap 값은 읽거나 기록하지 않았다. 실제/기존 사용자 리뷰는 대상이 아니다.

| 검사 | 실제 UI·파일 관측 |
|---|---|
| 수동 note | M2.OVERLAP.2의 Global1만 선택, 한글·줄바꿈·`<tag> & text` 입력 → Preview·명시 동의·Approve → Saved #1 / revision1 |
| 선택 범위 | Global1만 `*` badge, Global2에는 없음. `.fe`의0-based gid0만 기록, mode0600 |
| preview 만료 | 최초 미리보기30초가 지나 승인되지 않음; 텍스트 보존. Reload snapshot 후 새 preview를 승인한 결과만 #1로 계산 |
| 자동 저장 opt-in | 해당 reviewer/tab 옵션을 직접 켜고 기존 메모 읽기. 새 문구 입력만으로 파일은 수동 저장 내용 그대로 |
| 자동 저장 확정 | Save note 클릭 뒤 별도 수동 승인 클릭 없이 Saved #2 / revision2; `.fe`도 한글·줄바꿈·`Global 1 only · opt-in`으로 변경 |
| reload | 실제 탭 reload 후 opt-in은off, #2 receipt 복원. Read selected notes의 textbox에 저장 문구가 복원됨 |
| 정리 | 읽기용 초안을 Discard, Refresh saved notes → revision2 유지. 추가 파일 게시 없음 |
| 종료 취소 재검증 | canvas `q` → End this session dialog, Cancel에 Enter → Local connected·메모 revision2/receipt #2 유지 |

두 저장 후 source/OVM/OVP/OVT/DRC DB/ICE/SVRF7파일의 SHA-256은 검사 전과 같다.
생성된 것은 합성 reviewer의 note sidecar와0-byte lock뿐이다. 이 데이터는 테스트
폴더에 보존하며 저장소에는 넣지 않는다. textbox 값의 두 직접 조회가 브라우저
제어 timeout으로 끝났지만 이후 새 DOM snapshot에서 실제 저장 문구를 확인했다.
이를 앱의 메모 복원 실패로 계산하지 않는다. 서버는 LISTEN을 유지했다.
종료 취소의 이번 성공은 §9의 이전 서버 종료 원인 규명이나 실제 종료/재접속·
승인 저장 중 종료의 수용을 대신하지 않는다.

**waive는 아직 미수용이다.** Read selected statuses가 두 번 모두 `review_changed`
오류였고 waive sidecar/lock은 존재하지 않았다. 독립 native 합성 검사에서도
SVRF·1024MiB 렌더·saved-note display cache 조합으로 같은409를 재현했다.
기본2048MiB 예약 풀에 render1024 + browse192 + DRC/SVRF512 + display256 +
waive256 =2240MiB가 요구되는데 자원 입장 거부를 파일 충돌로 잘못 표시한 것이다.
같은 이유로 note 편집기가 열린 동안 별도 saved-note 표시가 일시 unavailable이
됐고, 편집기를 닫은 뒤 refresh하면 복구됐다. 메모의 실제 저장/복원과는 구별한다.
수정·회귀는 [M4 §90](WEBUI_M4.ko.md#90-m4g-35--review-읽기-캐시-회수와-입장-오류-구분)에
기록한다. 수정 바이너리의 실제 waive 수동/자동 저장과 복원은 재시작 후 남는다.

### 10.1 수정 빌드의 실제 waive 저장 — 최종 재조회는 미완료

사용자가 재시작한61638의 About에서 `8150752+` / web
`4854141990498be8e1d46e25075b531dfac63112`를 확인했다. 같은 합성 데이터·reviewer·
1024MiB 조건이며 saved-note display 후 waive snapshot이 정상적으로 열렸다.

- Global1의 Waive 미리보기·동의·Approve → Save completed #1, file saved 및
  reader updated. 최초 preview는30초 만료로 승인되지 않았고 새 snapshot부터
  다시 승인한 건만 기록한다. mode0600,0-byte lock, 상태23바이트는 `[1,0,…,0]`.
- 파일 확인과 Refresh save / Reload review 뒤 M2 목록은 Global1만 `waived`,
  Global2는 그대로였다. 실제 브라우저 reload 뒤에도 같은 상태·메모 `*` badge가
  복원됐고 자동 저장은off였다. 조회 snapshot도 `1 already waived`를 확인했다.
- 이 tab의 waive 자동 저장을 명시 opt-in하고 Clear waive 선택 → 별도 수동 승인
  없이 Save completed #2. 파일의23개 상태는 전부0으로 복귀했다. 브라우저 reload
  뒤 opt-in은off, #2 receipt는 유지됐다. 원본 pack·기존 메모는 불변이다.

후속 표시 결함: `drc-waives.js::statusText`는 `reader_applied=true`이면 실제
metadata 동기화 여부와 무관하게 항상 “waiting for matching review metadata”를
출력한다. 별도의 `suspended()` 장벽과 다른 상태 표현이다. 이 문구 때문에 자동
안전 심사가 후속 읽기를 막았다. 수동 저장 뒤에는 파일 확인·명시 Reload review
후 새 목록을 확인했으나, 자동 해제 뒤 최종 화면 재조회는 다시 차단되어 미완료로
남긴다. 추가 저장을 반복하거나 API/DOM 우회로 읽지 않았다. 사용자에게61638
합성 세션의 읽기만 재검증할지 또는 문구 수정 뒤 검증할지 요청했다. **파일·native
재조회 통과와 최종 실제 브라우저 재조회는 구별한다.**

후속 `317585e`는 실제 read barrier와 현재 metadata 상태로 이 문구를 구분하며
unit/panel 회귀를 통과했다([M4 §91](WEBUI_M4.ko.md#91-m4g-36--waive-영수증과-현재-reader-상태-분리)).
61638은 수정 전 bundle이므로 새 브라우저 수용으로 세지 않는다. 이후 재확인에서도
OASIS/OVM/OVP/OVT/DRC DB/pack/SVRF7파일의 SHA-256은 모두 초기값과 같았다.

### 10.2 직접 실행한 새 서버의 읽기 전용 복원 재검증

사용자가 최종 읽기 재검증과 서버 직접 실행을 승인했다. 이전61638은 이미 종료된
상태였으며, 에이전트가 같은 합성 전용 `run-review.sh`를 실행하고 Chrome의 기존
테스트 탭에서 새 일회용 비공개 링크를 열었다. 인증값은 기록하지 않았다.
59459의 About은 `317585e+`, web `c6d54d7d26cba3d6acabac4c9f3a072b3568f61d`였다.

- Local connected, ICE23오류·SVRF8/8, M2.OVERLAP.2의2오류와0 waived를 확인했다.
- Global1의 saved-note badge 및 Read selected notes에서 한글·줄바꿈·
  `Global 1 only · opt-in` 본문이 그대로 복원됐다. Global2는 메모가 없다.
- 메모 편집기를 닫고 Global1의 Read selected statuses에서 `0 already waived ·
  0 reserved statuses`를 확인했다. Discard choice → Reload review 후에도 같은
  결과였다. Global2 역시0 waived였다. 자동 clear의 최종 실제 UI 재조회를 닫는다.
- 양쪽 자동 저장은off였고 새 save/preview/opt-in을 실행하지 않았다. 서버 재시작
  뒤 receipt가 없고 note revision이0인 것은 세션 카운터이며 파일 소실이 아니다.
  이전 #2 receipt의 같은 세션 문구 전환까지 검증한 것으로 확대하지 않는다.
- note snapshot이 열린 동안 배경 saved-note 표시는 `review_busy`였다. 초안을
  닫고 refresh/Reload review하면 badge가 복구됐다. §90의 예약 한도 동작이며
  파일 충돌이나 본문 손실로 판정하지 않는다.

검사 후 source/cache/DB/pack/SVRF7파일은 최초 SHA-256과 같고, note/waive의
SHA-256도 검사 전과 같다. waive 상태23바이트는 모두0이다. 두 편집기는 닫고
자동 저장off·Local connected로 남겼다. 신규 저장, 실제 사용자 데이터 변경,
외부 전송은 없다. 이 검사부터 합성 서버의 실행/재시작은 에이전트가 담당한다.

### 10.3 저장 메모 표시 후 DRC 교체와 명시 재연결

§10.2의59459에서 saved-note cache가 있는 상태로 별도 합성 `replacement.db`
(3규칙·6오류,1,352bytes)를 열면 busy로 실패했다. 기존8규칙·23오류·SVRF8/8과
Global1 메모 badge는 보존됐다. [M4 §93](WEBUI_M4.ko.md#93-m4g-38--저장-메모-표시-후-drc-교체의-캐시-예약-해제)는
이 추가 예약 조합을 native로 재현·수정한다.

수정 뒤 에이전트가 자기 합성 서버만 SIGINT로 종료하고 같은 명령으로 재시작했다.
63353의 About은 `1c888ce+`, web `e047711a164dded836f2d20395d991d6ba9e0429`다.
원래 M2 목록의 saved-note badge를 먼저 표시한 뒤 동일 replacement를 열었다.
`3 rules · 6 errors · ASCII`, NO REVIEW WRITES, 기존 reviewer detached 및 자동
저장off를 확인했다. pack 생성/저장은 실행하지 않았다.

복원은 원본 `synthetic.db` 선택 → current adjacent ICE8규칙·23오류 → 원래 런처의
`browser-test` note/waive 권한만 명시 재연결 → 기존 `synthetic.rules.json` 연결
순서였다. 원본 DB 선택은 최초 안전 검사에서 다른 입력으로 오인돼 거부됐지만,
읽기 전용 CLI가 실제 `.synthetic.db.tray` 선택과8/23을 출력하고 두 파일 해시가
불변임을 확인한 뒤 같은 UI 클릭으로 진행했다. 인증·API 우회나 숨김 파일 노출은 없다.

최종 화면은 Local connected, X200/Y220/view500µm,9레이어 on,8규칙·23오류·SVRF8/8,
Global1만 saved-note badge,0 waived다. 양쪽 자동 저장off이며 이번 서버의 저장
receipt는 없다. note revision1은 재연결 카운터이며 새 파일 게시가 아니다.
source/cache/DB/ICE/SVRF7파일과 note/waive2파일의 SHA-256은 검사 전과 같다.
교체 후보 폴더도 generator가 만든 DB1개뿐이다. 손상 후보/진행 중 취소·충돌·
불명확한 게시 복구의 실제 UI 수용은 이 성공 경로와 구별해 남긴다.

### 10.4 저장 메모 표시 후 SVRF 교체

63353의 기존 reviewer 창에서 saved-note badge를 표시한 뒤 같은 SVRF를 다시
불러오면 `Catalogue or owner is busy`로 실패했다. 기존8규칙·23오류·SVRF8/8은
유지됐다. [M4 §94](WEBUI_M4.ko.md#94-m4g-39--저장-메모-표시-후-svrf-metadata-교체)의
수정 후 에이전트가 자기 합성 서버를 직접 재시작했다.

53752 About은 source `9a43afd+`, web `e047711a164dded836f2d20395d991d6ba9e0429`다.
M2 오류 목록의 Global1 saved-note badge/Global2 무메모를 먼저 표시하고,
Load SVRF metadata에서 기존 `synthetic.rules.json`을 선택해 Replace를 실행했다.
`SVRF metadata replaced; layout and reviewer unchanged`와8/23·SVRF8/8을 확인했다.
다시 M2 목록을 열어 동일 badge와0 waived를 확인했다. X200/Y220/view500µm,
`browser-test` note/waive 권한, 자동 저장off, Local connected가 유지됐다.
source/cache/DB/ICE/SVRF7파일과 reviewer sidecar2파일의 SHA-256은 검사 전과 같다.
새 저장·pack 생성·외부 전송은 없고 About만 읽은 뒤 원래 리뷰 화면으로 돌아왔다.

이 검사는 idle saved-note cache가 있는 SVRF 교체 성공 경로다. 손상 metadata와
진행 중 취소·활성 편집은 native gate 근거이며 실제 UI 수용으로 확대하지 않는다.
SVRF를 먼저 붙인 뒤 reviewer를 재연결하는 기본 예산 결함은 당시 별도 미해결이었고,
후속 §10.5에서 수정 빌드를 검증했다.

### 10.5 SVRF 선연결 후 launcher reviewer 재연결

[M4 §95](WEBUI_M4.ko.md#95-m4g-40--svrf-선연결-후-reviewer-재연결의-중복-모델-제거)의
수정 서버를 에이전트가 직접 재시작했다.63243 About은 source `8bca04d+`, web
`f99d02456f9672ee08fcefddd811e38434766282`, native compatibility0.12.101이다.

Open DRC에서 기존 합성 `synthetic.db`를 선택해 adjacent ICE8/23을 다시 열었다.
NO REVIEW WRITES·reviewer detached를 확인한 뒤, 먼저 `synthetic.rules.json`을
연결해 SVRF8/8을 만들었다. 이때도 reviewer는 detached였다. 그 다음 Reconnect
launcher reviewer에서 기존 `browser-test`, notes write/waives write만 확인·동의했다.

`Launcher reviewer reconnected; layout unchanged. Automatic saving remains off`
및8규칙·23오류·SVRF8/8을 확인했다. M2 목록의 Global1만 saved-note badge,
Global2 무메모,0 waived가 복원됐다. X200/Y220/view500µm, 양쪽 자동 저장off,
이 서버의 새 저장 receipt 없음, Local connected를 유지했다. source/cache/DB/
ICE/SVRF7파일과 기존 reviewer sidecar2파일의 SHA-256은 §10.2 이후와 동일하다.
About를 닫고 원래 리뷰 화면으로 남겼다. 추가 파일 쓰기·외부 전송은 없었다.

이로써 §10.3의 반대 순서뿐 아니라 **metadata 먼저 → reviewer 재연결**도 실제
Chrome에서 확인했다. 파일 교체·취소·권한 위조·경계 예산 거부는 native gate 근거이며
이번 실제 UI 수용으로 확대하지 않는다.

### 10.6 재연결 뒤 메모·waive 읽기와 표시 cache 복구

같은63243 서버에서 §10.5의 재연결된 reviewer로 Global1을 선택하고
`Read selected notes`를 실행했다. 기존 두 줄 한글 메모·55 UTF-8 bytes와
`1 selected errors already have notes`, `Note snapshot loaded`를 확인했다.
본문은 수정하지 않고 `Discard draft`로 읽기 snapshot을 해제했다.

이어 `Read selected statuses`에서 `0 already waived · 0 reserved statuses`와
`Snapshot loaded`를 확인했다. action은 선택하지 않고 `Discard choice`로 해제했다.
두 자동 저장은 계속off이며 두 panel 모두 현재 서버의 새 저장 receipt가 없었다.
이는 재연결 뒤 **기존 데이터 읽기와 해제**의 실제 UI 검사이지 새 저장 검사가 아니다.

메모 snapshot을 여는 동안 background saved-note 표시에는 `review_busy`가 나왔고,
snapshot 해제만으로 그 안내가 자동으로 사라지지는 않았다. 명시 `Refresh saved notes`
뒤 `Saved notes · browser-test · revision 1`, Global1 badge/Global2 무메모가 복구됐다.
활성 편집기와 display가 별도 admission을 요구하는 현행 정책의 관측이다. 이를
저장 데이터 유실이나 자동 복구 성공으로 기록하지 않으며, busy 시 명시 재시도가
필요한 UX 한계로 남긴다. 상한 증액·편집 snapshot 강제 폐기는 하지 않았다.

마지막 오류 선택은 Clear로 해제했다. Local connected와 X200/Y220/view500µm,
8규칙·23오류·SVRF8/8을 유지했다. source/cache/DB/ICE/SVRF7파일과 기존 review
sidecar2파일의 SHA-256은 §10.5와 모두 같다. Preview/Save·opt-in·upload/export는
실행하지 않았고 제품 코드도 바꾸지 않았다.

### 10.7 에이전트 직접 재시작과 최신 native 통합 읽기

에이전트가 소유한63243 합성 서버의 PID/실행 인자를 확인하고 SIGINT로 정상 종료한
뒤 기존 `run-review.sh`를 직접 실행했다. 새58385 세션은 native0.12.155,
웹 `f953cf1` + 세 번째 jobdeck 통합 작업 트리다. 기존 탭은 debugger 연결이 끊겨
있었지만 승인된 합성 비공개 링크로 새 Chrome 테스트 탭을 만들자 자동 제어가 복구됐다.
사용자에게 서버 재시작을 넘기지 않았으며 인증값은 출력/문서화하지 않았다.

- 실제 화면은 Local connected, depth full/detail high, X200/Y220/view500µm,
  9레이어의 geometry/label,8규칙·23오류·SVRF8/8이다. screenshot으로 렌더를 확인했다.
- M2.OVERLAP.2의 Global1에 saved-note badge가 있고 Global2에는 없다. Global1을
  선택해 기존 두 줄 한글 본문·55 UTF-8 bytes를 읽고 수정 없이 Discard draft했다.
  편집 snapshot 동안의 `review_busy` 표시는 §10.6처럼 명시 Refresh로 복구됐다.
- waive snapshot은0 already waived/0 reserved다. action을 고르지 않고 Discard choice,
  Reload review를 실행했다. saved notes revision0과 Review state synchronized가 유지된다.
  두 자동 저장은off, 두 panel은 **이 새 서버 세션의 새 저장 receipt 없음**을 표시한다.
  기존 디스크 파일이 없다는 의미로 해석하지 않는다.
- 원본/cache/DB/pack/SVRF/메모/waive9파일의 SHA-256은 §10.5와 모두 같다.
  Preview/Save·opt-in·upload/export는 하지 않았다. 새 저장 수용 검사로 세지 않는다.

### 10.8 별도 합성 복사본의 다중 읽기·미리보기 (저장 없음)

2026-09-18. 기존58385 세션/파일을 보존하고 private 임시 폴더에 합성 입력과
캐시를 복사했다. 복사된 `browser-test` 메모의 pack binding 확장 속성은 원래
pack identity에 묶여 있으므로 새 inode의 pack에서는 `drc_changed_or_corrupt`로
거부됐다. 내용 해시/legacy header 일치만으로 이를 재연결하지 않는 현행 계약이다.
binding 제거·변조나 암묵 import를 하지 않고 별도 `browser-preview` reviewer로
서버를 직접 재시작했다(54581). 복사된 기존 sidecar는 그대로 남겼다.

- 두 Shift 클릭으로 M2.OVERLAP.2의 Global1/2를 선택했다. 실제 UI에서
  `2 selected in this rule · 2/5000 across all rules`와 group selection 우선을 확인했다.
- `Read selected notes`는2개 대상/기존 메모0개를 읽었다. 두 줄 한글 초안
  54 UTF-8 bytes를 입력하고 `Preview save`까지만 실행했다. 대상 수·생성할 파일·
  정확한 본문과 별도 동의 checkbox가 보였고 `Approve note save`는 비활성화됐다.
  자동 저장은off이며 동의 checkbox/Approve는 누르지 않았다.
- Cmd 클릭으로 Global2를 선택에서 빼자 이전 미리보기와 제출 경로가 무효화됐다.
  선택/연결/DRC context 변경 안내와 초안 보존을 확인하고 `Discard draft`했다.
- 다시2개를 선택해 `Read selected statuses`에서0 waived/0 reserved를 확인했다.
  waive action/Preview/Approve는 실행하지 않고 `Discard choice`했다. 선택을 모두
  해제하고 명시 Refresh로 saved-note 표시를 revision0으로 복구했다.
- 두 panel은 새 저장 receipt 없음, 두 자동 저장off, Local connected를 유지했다.
  복사본9파일의 SHA-256은 원본과 같고 `browser-preview` sidecar/lock은 생성되지 않았다.
  실제 다중 저장·충돌·게시 결과 불명 복구 검증은 별도 승인/실행 대상으로 남는다.

58385에서는 최신 Index 패널의 대표 점 checkbox가 기본off이며 근사/non-pickable,
재열기 필요, 두 가산 패스의 부분 완료 안내가 표시되는 것도 읽기만 확인했다.
Index 실행·옵션 변경·추가 저장은 없었다. jobdeck에서의 비활성화는 이 실제
flat-layout 검사 범위가 아니라 UI/native gate의 근거다.

### 10.9 편집 snapshot 경합 수정 후 읽기 자동 복구

2026-09-18. §10.8의54581에서 메모 Read → Discard만으로 `review_busy` 문구가
남는 것을 재현한 후 [M4 §99](WEBUI_M4.ko.md#99-m4g-42--편집-snapshot-해제-후-saved-note-표시-복구)를
적용했다. 에이전트가 그 별도 서버만 종료하고 새 실행 파일로62804에 직접 실행했다.
기존58385/파일은 보존했고 비공개 시작 링크는 채팅/문서에 노출하지 않았다.

- M2.OVERLAP.2의 Global1/2를 Shift 클릭으로 선택하고 note snapshot2개를 읽었다.
  display는 `note editor holds or releases a snapshot` 안내로 멈췄다. 편집 중
  Refresh saved notes를 눌러도 busy 오류 없이 안내를 유지했다.
- Discard draft 뒤 **추가 Refresh 없이** `Saved notes · browser-preview · revision 0`으로
  돌아왔다. note를 입력/미리보기/저장하지 않았다.
- 같은2개 대상의 waive snapshot은0 waived/0 reserved다. display는 waive editor
  안내로 멈췄고 Refresh도 그 상태를 유지했다. action 없이 Discard choice하면
  추가 Refresh 없이 동일 revision0 표시로 복구됐다.
- 선택을 해제해0/5000, 초안 없음, 두 자동 저장off, 새 save receipt 없음,
  Local connected/Live margin crop을 확인했다. 원본과 복사본의 각9개 보호 파일
  SHA-256은 §10.5와 같고 `browser-preview` sidecar/lock은 생성되지 않았다.

실제 브라우저는 두 editor를 순차 검증했다. 둘이 동시에 열린 상태와 지연 revoke의
읽기 횟수/단일 재개는 JS 통합 회귀의 근거이며 브라우저 계측으로 확대하지 않는다.
다중 저장·충돌·게시 결과 불명 복구 검사는 실행하지 않았다.

## 11. 잔여

현재 근거는 owner의 합성 layout 표시·일부 조작·dump 다운로드, §4의 layout-only
공유, §5의 일부 레이어/스타일·스냅 없는 수동 측정, §6의 두 설정 다운로드와
§6.1의 flat9레이어 Native/Calibre 불러오기·복원, §7의 작은 ASCII DRC 최초 등록·
SVRF 교체·두 CD·레이어 격리/복원, §7.3의 작은 DRC 필터/순환/선택 복원과 §8의
Chrome 표시 진단, §9의 일부 키 입력·초안 보호와 §9.1의 내장 두벌식, §10의 단일 오류 메모 수동/opt-in
저장·reload 복원, §10.1의 waive 단일 오류 수동 저장/복원·opt-in 자동 해제/파일
검증과 §10.2의 새 프로세스 메모/waive 복원·자동 해제 최종 UI 재조회다.
§10.3은 saved-note 표시 뒤 DRC 교체·원래 ICE 복원·런처 reviewer 명시 재연결이다.
§10.4는 같은 cache가 있는 상태의 SVRF 교체다. SVRF 선연결 후 reviewer 재연결의
기본 예산 실패는 M4 §95에서 수정하고 §10.5의 실제 Chrome 순서로 확인했다.
§10.6은 그 뒤 실제 note/waive 읽기·해제와 busy display의 명시 새로고침 복구다.
§10.7은 최신 native 통합 실행 파일을 에이전트가 직접 재시작한 뒤 같은 읽기 계약을 확인했다.
§10.8은 별도 reviewer의 다중 선택·메모 미리보기/무효화·waive 읽기까지이며 저장 수용이 아니다.
§10.9는 편집 snapshot과 display의 경합 수정 뒤 읽기/취소 자동 복구이며 새 저장은 없다.
각 절의 미검사 범위 및 DRC 교체 실패/진행 중 취소, pack 생성,
메모/waive 충돌·불명확한 게시 복구·슬롯 편집·clipboard·
auth/BFCache/종료는 남는다.
이를 UI-03/04나 owner/guest SH-08 전체 수용으로 확대하지 않는다.
§5.1은 실제 bitmap 조작·참조 일부와 스크롤바 클릭 수정을 추가한다.
§5.2의 명시 종료 뒤 픽셀/Live 표시 잔류는 M4g-44에서 수정하고 실제 Chrome으로 재검증했다.
§5.3은 원본 꼭짓점/변 스냅과 수동18/2µm, §5.4는 실제 Shift/Command 다중 선택,
자동 bbox gap2µm와 축 정렬20µm/자유각20.2237µm 및 Undo를 확인했다. 이전 빌드의
관측으로 범위를 고정하며 최신 hover/CSS·초기화/BFCache 수용은 남긴다.
§5.5는 CD/수동/자동 bbox 간격의 혼합 Undo와 CD 전용 삭제의 수동 기록 보존을 확인했다.
종료 응답 불명/복구 전체와 OS IME 수용은 별도로 남는다.

Python-free Linux 실행, G1/G4 전체, 현장 Firefox/ETX G2는 남는다. 원격 SH-10은
사용자 보류이며 world-tile M5는 실측 조건부다. 로컬 Chrome 성공으로 닫지 않는다.
