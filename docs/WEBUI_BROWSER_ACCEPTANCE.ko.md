# 로컬 실제 브라우저 수용 기록

2026-09-17, `feature/webui`. [전체 G4 잔여](WEBUI_G4_AUDIT.ko.md),
[합성 표시 진단](WEBUI_DISPLAY_DIAGNOSTICS.ko.md),
[공유 SH 수용 범위](WEBUI_SHARING_ACCEPTANCE.ko.md).

이 문서는 실제 브라우저 조작·화면·저장 결과의 근거다. Node/HTTP 모형 게이트와
구분하며, 아래 일부 통과를 G1/G4 전체나 Firefox/ETX·Linux 수용으로 바꾸지 않는다.
제품 코드를 수정한 단계가 아니므로 전체 배터리를 다시 실행했다고 기록하지 않는다.

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

## 6. 잔여

현재 근거는 owner의 합성 layout 표시·일부 조작·dump 다운로드, §4의 layout-only
공유, §5의 일부 레이어/스타일·스냅 없는 수동 측정이다. 각 절의 미검사 범위와
파일/DRC 열기·저장/충돌/복구·슬롯 편집·clipboard·auth/BFCache/종료는 남는다.
이를 UI-03/04나 owner/guest SH-08 전체 수용으로 확대하지 않는다.

Python-free Linux 실행, G1/G4 전체, 현장 Firefox/ETX G2는 남는다. 원격 SH-10은
사용자 보류이며 world-tile M5는 실측 조건부다. 로컬 Chrome 성공으로 닫지 않는다.
