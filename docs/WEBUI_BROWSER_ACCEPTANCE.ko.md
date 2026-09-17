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

## 7. 잔여

현재 근거는 owner의 합성 layout 표시·일부 조작·dump 다운로드, §4의 layout-only
공유, §5의 일부 레이어/스타일·스냅 없는 수동 측정, §6의 두 설정 다운로드와
§6.1의 flat9레이어 Native/Calibre 불러오기·복원이다. 각 절의 미검사 범위 및
파일/DRC 열기·저장/충돌/복구·슬롯 편집·clipboard·auth/BFCache/종료는 남는다.
이를 UI-03/04나 owner/guest SH-08 전체 수용으로 확대하지 않는다.

Python-free Linux 실행, G1/G4 전체, 현장 Firefox/ETX G2는 남는다. 원격 SH-10은
사용자 보류이며 world-tile M5는 실측 조건부다. 로컬 Chrome 성공으로 닫지 않는다.
