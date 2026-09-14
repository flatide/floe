# floe2 웹 셸 / 서버-클라이언트 계획 (정본)

작성 2026-08-29, 갱신 2026-09-15(M4f-3b portable 고지 열람).
관련 정본: `FLOE2_OPTIMIZATION.ko.md`(F2R-10/11),
`RUST_RENDERER_PLAN.ko.md`, `SPEC-VIEWER.ko.md`, `rust/BUILD.md`.

현재 상태: **M0 로컬 조사/API 초안 + M1a worker client·공유 서비스 + 일반/잡덱 index·info/render/probe·분석/spec CLI**.
M1b-1에서 의존성 선정과 인증된 loopback HTTP/WS transport 기반을 추가했다.
M1b-2a에서 managed lease/admission과 native view controller를 추가했다.
M1b-2b에서 사전 등록한 view의 인증된 제어/프레임 스트림을 연결했다.
M1b-2c1에서 로컬 등록 scope·관리형 색인/진행·취소 수명을 연결했다.
M1b-2c2에서 인증된 catalog·view 생성/재open·색인 작업 API를 연결했다.
M1b-3에서 Rust `view` 실행 명령과 번들 Canvas 기본 뷰어를 연결했다.
M1b-4a에서 layout margin prefetch/crop과 라벨 포함 pan 재사용을 연결했다.
M1b-4b에서 drag·fill/width/font 편집·기본 GTK 단축키를 연결했다.
M2a-1에서 기존 DRC ICE pack 읽기·공간 페이지·waive 조회 CLI를 이관했다.
M2a-2에서 DRC 전용 actor·자원 예약과 등록 ID 기반 인증 읽기 API를 연결했다.
M2a-3에서 규칙/오류 페이지·waive 필터·goto·표시 프레임에 정렬된 DRC overlay를 연결했다.
M2a-4a에서 view별 DRC 패널 상태의 서버 메모리 보존·revision 충돌 API를 추가했다.
M2a-4b에서 유계 브라우저 저장 큐·새로고침/재접속 복원을 연결했다.
M2a-5a에서 현재 규칙/필터의 유계 순회 API와 좌표 slice 읽기를 추가했다.
M2a-5b에서 페이지 횡단 순회·click/이동 모드·Escape와 해당 상태 복원을 연결했다.
M2a-6에서 표시된 DRC 마커의 단일/이중 클릭을 release-only pan과 분리해 연결했다.
M2a-7a에서 단순 DRC 오류의 CD 측정 코어와 읽기 API를 추가했다.
M2a-7b에서 마지막 이동 오류의 CD 치수선·값 표시, k/K/Escape와 상태 복원을 연결했다.
M2a-8a에서 규칙별 DRC 선택 집합과 현재 페이지 후보 bbox 판정·revision 충돌 API를 추가했다(UI는 다음 단계).
M2a-8b에서 두 클릭 박스·Shift/Ctrl/Cmd 선택과 규칙별 금색 마커·세션 복원을 연결했다.
M2a-9a에서 현재 규칙의 Selected·waive·live viewport 교집합 목록/순회 코어와 인증 API를 추가했다(UI는 다음 단계).
M2a-9b에서 Selected·현재 규칙 live In view UI, 유계 추종/복원·마커 hover를 연결했다.
M2a-10a/b에서 SVRF sidecar 읽기·타입/규칙 필터·측정 비교 코어/CLI/API를 연결했다.
M2a-10c에서 웹 타입 선택/상세/비교와 성공한 오류 이동 시 In view 해제를 연결했다.
M2a-10d1에서 서버의 레이어 격리·한 번만 복원과 준비 토큰 기반 원자적 focus API를 추가했다.
M2a-10d2에서 승인 후 웹 상태 반영·Restore layers·Escape와 취소/재접속을 연결했다.
M2a-11a에서 읽기 전용 ASCII DRC 코어·CLI fallback과 소수 좌표 측정 parity를 추가했다.
M2a-11b에서 명시한 ASCII DRC의 웹 등록·유계 조회·소수 좌표 윤곽/CD를 연결했다.
M2a-12a에서 명시 DRC pack 생성 코어/CLI와 임시 생성·검증·원자 게시·취소를 추가했다.
M2a-12b1에서 owner HTTP 생성/취소와 이전 DRC 응답 무효화·새 identity 재등록을 연결했다.
M2a-12b2에서 pack 생성 승인·진행/취소·불명확한 요청 확인 UI와 새 리뷰 조회 복원을 연결했다.
M4a-1에서 공유와 독립적인 native pick/snap scene 식별·Rust process client를 연결했다.
M4a-2에서 표시 frame/worker·revision에 고정한 로컬 controller query와 종류별 취소·style drain을 연결했다.
M4a-3에서 기존 owner WebSocket에 표시 ACK/연결별 query와 안전한 결과 DTO를 연결했다.
M4a-4에서 브라우저 도형 선택/overlap 순환·modifier 선택·스냅 프로브와 stale 검사를 연결했다.
M4a-5에서 Rust 좌표·거리 계산과 두 점 수동 ruler/Shift·snap·미리보기·삭제를 연결했다.
M4a-6에서 Rust 선택 bbox 자동 gap과 수동/auto/CD 생성 순서·삭제·라벨 배치를 통합했다.
M4b-1에서 일반 layout의 exact clip CLI·private 출력 검증·스트리밍 원자 게시를 연결했다.
M4b-2에서 layout/jobdeck batch·mosaic 캡처와 report·kept tiles를 단일 Rust worker로 연결했다.
M4b-3에서 PNG pixels를 보존하는 flateyes metadata와 `fe-embed` Rust 보조 CLI를 연결했다.
M4b-4에서 기존 live 스타일·오류 marker/CD/legend를 유지하는 DRC PNG 캡처 CLI를 연결했다.
M4b-5에서 SVRF subset 전처리·규칙 그래프·scan/원자 sidecar 저장 CLI를 Rust로 이관했다.
M4c-1에서 별도 자원 예약·취소/reap·만료 descriptor 저장소를 갖는 관리형 exact clip 코어를 추가했다.
M4c-2에서 표시 receipt·명시 승인·중복 방지·취소와 owner HTTP chunk 다운로드를 연결했다.
M4c-3에서 현재 viewport의 준비→명시 승인·취소·파일 목록/다운로드 UI를 연결했다.
M4d-1에서 이미 표시한 픽셀의 PNG 복사/저장과 overlay 3상태 전환을 연결했다.
M4d-2에서 layerprops Rust codec과 GTK에 맞춘 첫 view의 레이어 가시성을 연결했다.
M4d-3에서 열린 세션 설정 Load/Save·bitmap/상속을 보존하는 native JSON과 원자적 적용을 연결했다.
M4d-4a에서 공유 설계 기본값의 읽기 전용 준비·충돌 검사·권한 보존·원자 게시 Rust 코어를 추가했다.
M4d-4b에서 launcher opt-in·owner 준비/명시 승인/취소/결과 API를 연결했다.
M4d-4c에서 공유 영향 preview·체크 승인·진행/취소·새로고침 후 동일 요청 확인 UI를 연결했다.
별도 승인 후 실제 Chrome의 합성 layout 신규 게시 클릭은 확인했다(M4 §20).
기존 파일 교체·취소/복구·현장 브라우저 수용은 별도이며 일반 Save는 계속 다운로드뿐이다.
M4e-1~4에서 DRC 주석/waive codec·관리형 게시, owner의 편집·미리보기·별도 승인 UI와
waive 저장 후 reader 갱신/조회 revision 장벽을 연결했다. M4e-5a는 저장된 주석의
목록 badge·이동 대상 본문용 읽기 전용 API/캐시다. M4e-5b에서 목록 배지와 마지막
ACK 이동 대상의 주석 overlay·서버 panel 상태 복원을 연결했다.
M4e-6a에서 주석/waive native snapshot export와 전체 waive import를 연결했다.
M4e-6b에서 owner 분할 업로드·전체 교체 준비·기존 승인 게시와 유계 artifact 다운로드를
연결했다. M4e-6c에서 전체 review 파일 선택·분할 전송/미리보기·별도 승인과
내보내기/다운로드 UI를 연결했다. Chrome 합성 내보내기 준비는 확인했으며, 실제 브라우저
업로드·review 게시·다운로드 파일 수용과 현장 검증은 후속이다.
M4f-1에서 `selfcheck`의 빌드 식별·실제 native 버전/handshake/종료 검사를 추가했다.
이 선행 단계에서는 전용 portable 조립을 후속 M4f-2로 구분했고 기존 GTK 패키지를 유지했다.
M4f-2에서 별도 offline packager·ELF 감사·원자적 비덮어쓰기 archive와 고지/체크섬을
연결했다([portable 안내](WEBUI_PORTABLE.ko.md)). Linux 실행·현장 수용 및 About UI는 별도다.
M4f-3a에서 읽기 전용 About/빌드 식별·글꼴 원문 고지를 연결했다.
M4f-3b에서 compiled catalogue로 고정된 portable 원본 고지의 유계 열람을 추가했다.
현장 수용·남은 조작 parity는 계속 열린 상태다([M4 §39](WEBUI_M4.ko.md)).
나머지 내보내기와 브라우저 다운로드/현장 수용은 남아 있다([M4 기록](WEBUI_M4.ko.md)).
전체 조작 parity와 M0/Firefox/ETX 현장 검증은 아직 완료되지 않았다.
상세 범위는 [M1b 기록](WEBUI_M1B.ko.md)과 [M2 기록](WEBUI_M2.ko.md). 여기의 M0~M5는 **웹 전환 단계**이며
jobdeck/occupancy의 같은 이름 단계와 별개다. 개발 기준과 합류 규칙은 §11.
로컬 산출물: [기능/CLI 대조표](WEBUI_M0.ko.md),
[Rust 서비스/API 초안](WEBUI_SERVICE_API.ko.md).

## 0. 결정 로그 (사용자 확정 사항)

- 2026-09-13: 현재 TeeBox 현장에서 audit/Firefox·ETX 실행이 불가능하다.
  M0 현장 감사와 G2는 **보류**로 두고 로컬 구현·회귀 검증은 계속한다.
  로컬 Chrome 검증을 현장 PASS로 대신하지 않으며 GTK launcher는 유지한다.
- 2026-08-28: 궁극 목표는 서버-클라이언트 모델. 당장은 데스크톱 앱
  배포가 필요하다.
- 2026-08-28: 주 작업자 흐름은 **외부망 portal에서 작업 선택 →
  폐쇄망 TeeBox가 TeeBox 계정으로 앱 실행 → 사용자 Exceed
  TurboX(ETX) 세션에 표시** — 이 흐름은 유지되어야 한다. TeeBox가
  사용자 계정/사용자 머신에서 앱을 실행하는 방법은 만들지 않는다.
- 2026-08-28: 브라우저 직접 사용은 주 작업이라기보다 **DRC 결과를
  공유받는 동료의 뷰어** 역할이 유력하다(링크 접근은 자연스럽고
  계획된 흐름).
- 2026-08-28: 폐쇄망에 Firefox는 있다(버전은 감사 필요, §7).
- 2026-08-29: **데스크톱 앱도 서버-클라이언트 패키징**으로 만든다 —
  같은 스택을 한 머신에 묶은 형태.
- 2026-08-29: **floe(KLayout backend)에도 적용한다** — gateway 경계를
  renderd wire가 아니라 GUI-중립 worker 계약(`make_render_worker`
  job/result)에 두어 두 제품이 같은 웹 셸을 공유한다(§3.1a). rollback
  스토리(FLOE_PRODUCT 전환)가 웹 셸에서도 유지된다.
- 2026-09-02: **T2의 raw RGBA payload가 제품 GTK 경로에 선구현됨** —
  F2R-13(FLOE2_OPTIMIZATION §3.16/§F2R-13, 0.12.26): renderd
  `frame_format=raw`가 `FLOERAW1`(magic+u32le w/h+packed RGBA)를
  기존 원자적 publish 계약으로 게시하고 GTK가 무디코드 표시. T2
  gateway는 이 payload를 재인코드 없이 스트리밍하면 된다. 별개로
  T3의 전제였던 F2R-10 world-tile은 **조건부 보류**(fill 위상이
  device-anchored라 byte-exact tile 재사용은 F2R-03c 1bpp plane
  선행 — §3.16 판정).
- 2026-09-08: **브랜치 승격 — floe2가 유일한 제품 라인.** 웹 셸은
  **floe2에만 적용**한다. 2026-08-29의 "floe(KLayout backend)에도
  적용" 결정은 철회하고, §3.1a의 두 제품 동시 지원은 요구사항에서
  뺀다(worker 계약 경계 자체는 GUI 중립 인터페이스로서 유지 — KLayout
  worker는 개발 선행 검증용으로만 존재). floe는 `floe-legacy`에 동결.
- 2026-09-06: 설계 리뷰(HIGH 5·MEDIUM 1 + 문구) 반영. 뷰 세션 모델
  (§4), 프레임 봉투·취소·backpressure·재접속 계약(§5), 보안 기본값
  (§8), Firefox 격리·`--kiosk` 하한(§3.3/§7, M0 신설), GTK의 margin
  계약을 M1부터 이관(§6), 표시 정확도 계약(§3.2), T0 경계 정정(§5),
  Rust gateway 이관 조건부화(§3.1a), Electron·라이선스 문구 완화.
- 2026-09-05 이후 GTK 셸에 생긴 계약(웹 셸이 그대로 옮겨야 할 것):
  배경 margin prefetch와 착지 프레임 보관·crop(F2R-17/21, 라벨 포함,
  `labels_truncated`면 crop 제외), 16px fill 위상에 스냅한 커서 pan,
  착지 margin을 새 strip의 표시 base로 blit, renderd 질의 스레드
  (F2R-22: pick/snap이 렌더와 독립 — gateway가 질의를 병렬로 보낼 수
  있음).
- 2026-09-12: **Python이 맡은 제품 실행 기능도 Rust로 이관**한다.
  Python gateway 선행·Rust gateway 조건부 이관안을 폐기하고 처음부터
  Rust gateway/CLI/서비스로 개발한다. 완료 제품의 실행에는 Python,
  PyGObject, Python 어댑터가 필요 없어야 한다. GTK 코드를 Rust GTK로
  번역하지 않고 표시·입력은 웹 UI로 대체한다. HTML/Canvas UI 계획은
  유지하며 구체적인 프론트엔드 언어·도구 선택은 §10에서 구분한다.
- 2026-09-12: **jobdeck 실측과 웹 전환을 브랜치로 분리**한다.
  `feature/jobdeck`은 실측·수정용으로 유지하고, 해당 브랜치의
  `6c33a48`(LOD 전달 머지 포함)에서 `feature/webui`를 생성한다.
  분기했다고 jobdeck/occupancy의 현장 성능·화질 검증을 완료로 보지 않는다.

## 1. 목표와 비목표

목표:

1. HTML/canvas 기반 뷰어 UI 하나로 세 배포형을 커버한다(§2).
2. Rust 렌더·플랜 성능 자산(F2R 계열)은 그대로 재사용한다. decode/raster
   코어 재작성은 하지 않는다. Python의 제어·데이터 기능 이관과 웹 표시
   비용은 각각 검증하며, 언어 전환만으로 렌더 시간이 줄어든다고 가정하지 않는다.
3. 서버 세션 설계로 "브라우저 리프레시 = 상태 소실" 위험을 제거한다.
4. F2R-10(world-tile) / F2R-11(streaming)과 합류 가능한 전송 계층을
   설계한다 — 클라이언트 tile 합성이 world-tile LRU의 자연스러운
   구현처가 된다.
5. CLI·서버·배포 실행 경로에서 Python 의존성을 없앤다. jobdeck, DRC,
   설정·캐시·인덱싱 제어를 포함하며, 웹 UI만 바꾸고 Python 서비스를
   뒤에 남긴 상태는 완료로 보지 않는다(이관 목록 §3.1b).

비목표:

- GTK 셸의 즉시 대체. GTK는 parity + ETX 게이트(§6) 통과 전까지 주
  작업자용으로 병존한다(worker job/result 계약이 GUI 중립이라 가능).
- Electron을 TeeBox에서 실행해 X/ETX로 쏘는 형태는 **현장 검증 전
  제외**. 원격 디스플레이에서 Chromium 합성 비용이 크다는 우려는
  해당 ETX 구성에서 실측되지 않았고, Firefox도 배포 B에서는 결국
  ETX 화면 전송을 거치므로 기술적 단정이 아니라 우선순위 결정이다.
- 초기 단계의 편집·계측 고급 기능 parity. M1~M2는 읽기 중심이다.
- 동결된 floe/KLayout·legacy indexer·폐기된 coverage 코드의 Rust 복제.
  현재 floe2가 제공하는 기능 계약을 기준으로 이관한다.
- 이 브랜치 생성 시점의 제품 코드 일괄 삭제. 기존 GTK/Python은 단계별
  비교 기준으로 남기되 최종 웹 제품의 실행·배포 의존성과 분리한다.
  개발용 Python 오라클/생성기까지 없앨지는 별도 범위 결정(§10).

## 2. 아키텍처: 한 스택, 세 배포형

```
[공통 스택]   Rust CLI / launcher
                 │
             Rust gateway ──[Rust worker client / renderd wire]── floe-renderd
                 │     └── Rust 공통 서비스(jobdeck·DRC·캐시·인덱싱 제어)
                 │ 정적 UI 서빙 + WS + 뷰 세션/토큰
                 ▼
             HTML UI (canvas 2D; Python 런타임 없음)

배포 A  데스크톱 패키징: launcher가 gatewayd+renderd를 함께 기동,
        UI는 로컬 브라우저(firefox --kiosk)로 loopback 접속.
배포 B  주 작업자(ETX): TeeBox 계정이 A와 동일 구성을 기동하되
        firefox의 DISPLAY를 ETX로 지정. portal→TeeBox 실행 흐름이
        한 글자도 안 바뀐다(§0). B는 A의 특수형이다.
배포 C  동료/원격 뷰어: gatewayd만 TeeBox에서 서빙, 사용자 자신의
        브라우저가 네트워크로 접속. 픽셀은 로컬에서 그려진다.
```

- B는 A의 실행 구성을 재사용하되 Firefox 프로필 격리·ETX 성능은 별도
  검증한다. DISPLAY 상속만으로 G2 통과를 보장하지 않는다.
- C는 같은 서비스 계약을 쓰되 TLS·게스트 권한·전송 상한·다중 사용자
  자원 정책이 추가된다. 단순한 바인딩 주소 변경만으로 배포 완료가 아니다.
- 향후 로컬 Electron 셸은 "C에 붙는 선택적 데스크톱 래퍼"로 분리
  판단한다(§8) — TeeBox 실행 모델과 무관한 사용자측 배포 정책 문제.

## 3. 컴포넌트

### 3.1 gateway (신규)

- 역할: 정적 UI 자산 서빙, WS 명령 검증, 세션·권한·큐 관리, Rust
  서비스/worker 호출. HTTP/WS 처리와 도메인 기능을 분리해 CLI도 같은
  Rust 서비스를 사용하게 한다. geometry 플랜·raster는 renderd에 남긴다.
- 기존 `make_render_worker`의 job/result는 **호환 계약의 기준**이지
  Python 함수를 호출하라는 뜻이 아니다. `RustRenderWorker`와
  `DeckRenderWorker`의 정책·응답 의미를 Rust 타입과 worker client로
  이관하고 renderd wire에 연결한다. 초기에는 독립 renderd 프로세스와
  현재 취소·게시 경계를 유지한다.

#### 3.1a 단계별 구현체

- **M1부터 Rust gateway**. Python 서버·subprocess 어댑터를 임시 제품
  경로로 추가하지 않는다. 기존 Rust workspace에 서비스/CLI/gateway를
  분리하며 크레이트 이름과 라이브러리는 M0에서 확정한다.
- HTTP/WS는 검증된 Rust 라이브러리를 선정하고 오프라인 빌드용 의존성을
  동봉한다. RFC6455 자체 구현은 하지 않는다. 프레임 길이·fragmentation·
  제어 프레임·비정상 종료·느린 수신자 검증을 게이트에 포함한다.
- capability는 **레이아웃/잡덱 및 전송 기능별**로 협상한다. 현재
  `DeckRenderWorker`는 `supports_margin_prefetch=False`,
  `supports_label_font_px=False`이므로 일반 레이아웃 기능을 그대로
  노출하지 않는다. T3는 구현·검증 전까지 지원한다고 광고하지 않는다.
- 버전은 floe/cli/renderd와 동일 스탬프 체계로 묶고(`--version`,
  시작 스탬프), **UI 자산은 반드시 자기 번들의 것만 서빙**한다. 번들
  일치만으로 skew가 사라지지는 않으므로 추가로: handshake에 프로토콜
  버전을 싣고 불일치는 명시 거부, 이미 열린 구버전 탭의 재접속은
  "새로고침 필요" 안내 후 차단, UI 갱신 정책(gateway 재기동 시 열린
  탭 강제 리로드 여부)을 명시한다.

#### 3.1b Python 기능 이관 범위

파일별 기계적 번역이 아니라 사용자에게 보이는 기능과 입출력 계약을
기준으로 이관한다. 아래는 현재 코드에서 확인한 출발 목록이며 M0에서
공개 명령/옵션별 수용 기준과 연결한다.

| 현재 영역 | 목적지·검증 |
|---|---|
| `floe/cli.py`, `floe2/cli.py`, `instance.py` | Rust CLI/launcher: 명령·종료 코드·로그·옵션 전달·단일 인스턴스/뷰 세션 의미 유지 |
| `cache.py`의 현행 VFS 경로, `vfsclient.py` | Rust 캐시/인덱싱 서비스: freshness·비파괴 재사용·`--force`·jobs·LOD/occupancy·프로파일 옵션, legacy 코드는 제외 |
| `jobdeck/{parser,sources,geom,color,plan,viewer}.py` | Rust jobdeck 라이브러리: 문법·오류/skip ledger·좌표·레이어 순서·레벨/칩 뷰·소스 선택·보고서 parity |
| `rust_render.py`, `service.py`의 Rust 경로, `jobdeck/render.py` | Rust worker client: 명령/응답·취소·타임아웃·프레임 파일 소비/정리·scene/query 유효성 |
| `drc.py`, `svrf.py`, `shots.py`, `fe_embed.py` 및 GUI 안의 저장 로직 | Rust 조회/저장/내보내기 서비스: DRC·waive·주석·룰/레이어 매핑·스크린샷·설정, 기존 Rust drcice/drcpack 재사용 |
| `gui.py`, `view_policy.py`의 UI/상호작용 | HTML/Canvas 입력·표시와 Rust 상태/정책으로 분리: pan/margin·goto·depth/detail/thin·레이어/스타일·단축키·상태줄 |

- Rust에 이미 있는 파서·인덱서·플래너·occupancy·raster·pick/snap·clip은
  재사용한다. `floe-index`/`floe-renderd`를 Python으로 재포장하지 않는다.
- 초기 읽기 전용 범위 밖의 DRC 저장·내보내기·보조 CLI도 이관 목록에서
  추적하며, 빠졌다면 M4의 Python-free 제품 전환은 완료가 아니다.
- jobdeck 정책은 실측 브랜치의 것을 기준으로 한다: 레벨 선택, 레벨/칩
  모드와 부모-자식 목록, `thin=auto|keep|cull`, occupancy 사용/없음 이유,
  요약 레이어의 pick/snap 제한을 웹에서도 숨기지 않는다.

### 3.2 HTML UI

- canvas 2D 단일 뷰포트 + DOM 오버레이(**UI·주석 한정**: 룰러/상태줄/
  마커). **설계 라벨은 프레임 안에 유지**한다 — Rust가 번들 글꼴로
  배치·회전·declutter·겹침 규칙까지 그리며(`RUST_RENDERER.md` 라벨
  계약), DOM 라벨화는 그 규칙의 별도 이관 작업이므로 초기 범위 밖.
  WebGL, OffscreenCanvas, WebP/AVIF 등 신기능 의존 금지(§7 하한).
- **표시 정확도 계약**(성능 게이트 §6의 짝): ① CSS 픽셀 ↔ render
  device 픽셀 관계를 고정(DPR·브라우저 확대는 device 픽셀 기준으로
  요청 크기를 정하고 1:1 표시), ② DBU 좌표와 y축 방향(row 0 = 위,
  F2R-19), fractional viewport 보존, ③ resize 중 이전 크기 프레임은
  GTK처럼 frozen base로 유지, ④ margin crop은 정수 픽셀 정렬 + 16px
  fill 위상 계약(§5 T0에서도 동일), ⑤ raw 업로드(`putImageData`는
  canvas transform을 받지 않음)와 pan/crop 합성(`drawImage`) 경로를
  구분한다.
- 상시 애니메이션 금지, 프레임 단위 통짜 갱신 — ETX(TXP) 압축
  친화적으로(배포 B 대비).
- 정적 파일은 전부 번들 동봉. CDN·외부 폰트 금지(폐쇄망).
- 빌드: browserslist 하한(§7 감사 후 확정) + ES2017 transpile + 호환
  lint를 CI 게이트로.

### 3.3 launcher 통합

- Rust `floe2 view <src> --web`(명령 이름 가칭): gatewayd+renderd 기동 → 토큰 URL
  생성 → Firefox 실행(배포 A/B 공용). DISPLAY는 호출측 환경을 그대로
  따르므로 TeeBox launcher 수정이 불필요하다. 다만 DISPLAY 상속만으로
  독립 인스턴스가 보장되지 않는다: 같은 TeeBox 계정에서 여러 작업을
  실행하면 기존 Firefox 인스턴스 재사용·프로필 잠금 충돌이 난다. 세션별
  프로필(`--profile <세션 dir>`)과 새 인스턴스(`--new-instance`/
  `-no-remote`), 종료 시 프로필 정리 정책을 **실제 버전에 맞춰 M0에서
  검증**한다. `--kiosk`는 Firefox 71+에서만 있으므로(§7) 하한이 그
  아래면 일반 창(`--new-window`)으로 실행한다.
- 공유 URL 발급: 열린 세션에서 읽기 전용 게스트 토큰 URL을
  발급한다(배포 C, DRC 공유 흐름).

## 4. 세션·상태 계약

- **복원 대상인 사용자 상태는 전부 서버(gateway 세션)에 둔다**:
  viewport, depth, detail, layer 가시성, style epoch, DRC 선택/waive
  (파일 기반 기존 체계 재사용), goto 히스토리, thin 정책, jobdeck의
  선택 레벨·level/chip 모드. 일반 레이아웃과 덱의 기본 정책 차이를 보존한다.
- 새로고침/재접속 = 세션 재부착 후 완전 복원. "클라이언트 전용 상태
  금지" 원칙은 **복원할 사용자 상태**에 한정한다 — 마지막 프레임,
  착지 margin, 즉시 pan 표시 같은 **일시 표시 상태는 브라우저에
  허용**한다(GTK의 margin 계약을 옮기는 데 필수, §6).
- **뷰 세션 모델**: 설계·DRC 데이터는 공유하되 viewport·가시성·선택·
  render generation은 **뷰 세션별**이다. renderd는 generation
  frontier와 게시 scene이 하나뿐이라(다른 뷰의 요청은 이전 요청을
  취소하고 pick/snap은 게시 scene을 조회) 독립 탐색 게스트를 같은
  worker에 붙일 수 없다. 따라서 ① 작업자 화면 **따라보기** = 같은 뷰
  세션의 프레임 배포(게스트는 입력 없음), ② **독립 탐색** = 별도 뷰
  세션 = 별도 worker(renderd 프로세스). 별도 worker 방식에는 서버
  전체의 동시 렌더 수·메모리·linger 상한이 함께 필요하다 — worker별
  budget(`FLOE_RUST_BUDGET_MB` 등)만으로는 여러 사용자의 총부하를
  제한하지 못한다(§10-4와 정합).
- 인덱싱 작업까지 포함한 서버 전체 jobs·동시 worker·메모리 입장 정책은
  M0에서 설계한다. 사용자 요구인 인덱싱 16스레드 이내 목표와 렌더
  응답성을 함께 평가하며, worker 수만 줄여 전체 부하가 제한됐다고 보지 않는다.
- UI 종료 후 gateway/renderd는 **linger**(기본 수 분) — 재열기 즉시
  복원 + decoded LRU 보존(F2R-10 보존 스토리와 합류). linger 상한과
  명시 종료 경로를 둔다(고아 방지: renderd의 start_new_session,
  watchdog 경험 재사용).
- 다중 클라이언트(작업자 + 게스트 N)의 조작 권한은 토큰 등급으로
  구분한다(게스트 = 읽기 전용). "같은 scene"의 의미는 위 뷰 세션
  모델을 따른다.

### 4.1 인덱스 수명주기 (설계 필요, 이번 분기로 구현 완료 처리하지 않음)

열린 GUI가 `.ovo` 교체를 즉시 감지하지 않는 기존 항목은 사용자 합의대로
jobdeck 실측의 차단 조건에서 제외한다. 웹/서버 모델에서는 별도 계약을 정한다.

- 소스 식별자·인덱스 revision·뷰 상태 revision·render generation을
  구분한다. 한 세션/프레임이 서로 다른 revision의 OVM/OVP/OVT/OVO를
  섞어 쓰지 않아야 한다. 덱은 참조 소스별 revision 집합도 식별한다.
- 재인덱싱 중 기존 세션 유지, 새 세션의 버전 선택, 명시적 reopen/전환,
  사용 중인 파일의 보존·회수 책임을 Rust 서비스 계약으로 정한다.
- revision 전환 시 frame/margin/retained/query 캐시의 무효화 범위를
  함께 정한다. mtime 감지만으로 일관된 snapshot이 보장된다고 가정하지 않는다.
- 구현 방식(불변 revision 디렉터리/manifest 등)은 M0 설계에서 비교해
  결정한다. 그 전에는 인덱싱 완료 후 열기·재인덱싱 후 재열기를 전제로
  개발하며 hot reload를 지원한다고 표시하지 않는다.

## 5. 전송 계층 (진화 단계)

| 단계 | 프레임 경로 | 비고 |
|---|---|---|
| T0 | renderd의 원자적 publish → Rust worker client가 파일을 소비 → gateway가 WS로 전달 | renderd wire 유지, Python 어댑터 없음. PNG는 기준 경로, loopback raw(T2)도 M1에서 비교. 파일 검증·읽기·정리는 Rust client가 소유 |
| T1 | renderd→gateway 직접 스트림(PNG) | 파일 publish/fsync 제거 |
| T2 | loopback 한정 raw RGBA | 기존 F2R-13 `FLOERAW1` 재사용. M1에서 PNG와 A/B; 웹 UI·전송 복사까지 포함해 G1 평가. raw 지원을 위해 T1 완료를 기다릴 필요 없음 |
| T3 | world-tile 단위 delta + 클라이언트 tile 캐시 | F2R-10/11 합류 지점. 인접 pan의 draw 재지불을 클라이언트 합성으로 흡수. **floe2 전용, 조건부 보류**(F2R-03c 선행 — FLOE2_OPTIMIZATION §3.16) |

- T0/T2의 프레임 형식·치수·길이 검증, 취소 시 파일 정리, raw/PNG 선택,
  오류 응답은 기존 Python 어댑터와 동등하게 검증한다. 브라우저가 서버의
  파일 경로를 지정하거나 publish 디렉터리에 직접 접근하는 API는 제공하지 않는다.
- **프레임 봉투와 취소 계약**(원자적 파일 publish가 보장하던 것을
  네트워크에서 보존): 프레임마다 session/view id, 요청 순번,
  generation, 인덱스 revision(§4.1), render-state revision(layer/depth/style epoch), bbox,
  크기·포맷, 완료 여부(final/refining/bg)를 결합한다. 클라이언트는
  수신 시와 **디코드 완료 시** 두 번 최신 요청인지 재검사하고 stale은
  버린다. 서버는 뷰 세션당 전송 중 프레임 1 + 대기 1로 제한하고
  오래된 대기 프레임을 폐기한다(브라우저 WebSocket은 수신 backpressure
  를 제공하지 않으므로 큐가 쌓이면 화면이 계속 뒤처진다). 느린 게스트
  는 자기 큐만 밀리게 분리해 작업자 렌더와 다른 게스트 전송을 막지
  않는다. 재접속 시 상태 snapshot + 최신 완성 프레임으로 재동기화한다.
- **메모리 상한**: margin 프레임은 16Mpx 상한에서 RGBA 64MiB다. T2
  raw를 네트워크로 보낼 때는 전송 큐·JS 버퍼·canvas 복사본이 겹치므로
  클라이언트 viewport 기준 margin 크기와 뷰 세션별 전송 중 바이트
  상한을 두고, 원격(C)에는 PNG/T1을 기본으로 한다.

- T0/T1/T2는 배포형별 협상(capability handshake)으로 공존 가능하게.
  T1의 직접 스트림은 별도 성능 작업이며 T0/T2의 파일 publish 계약을
  먼저 이관·검증한다.
- T3의 tile key는 F2R-03b 2c 설계가 남겨둔 world/scale 정렬 키를
  사용한다(`FLOE2_OPTIMIZATION.ko.md` §F2R-03 2c 확장 키).

## 6. 성능 요구와 게이트

- **G1 (loopback, 배포 A)**: 동일 뷰·동일 renderd에서 input→photon
  지연과 drag-pan frame pacing이 GTK 셸 이하(±10%)일 것. 미통과 시
  전송 단계(T1/T2)를 앞당겨 재측정.
- **G2 (ETX, 배포 B)**: TeeBox의 실제 Firefox 버전으로 Firefox-in-ETX
  vs GTK-in-ETX를 drag pacing·settle 체감·ETX 대역폭으로 비교.
  미통과 시 주 작업자는 GTK 유지, 웹은 배포 C 전용으로 축소 — 이
  경우에도 투자 손실이 없다(C는 확정 수요).
- **G3 (원격, 배포 C)**: LAN 기준 goto→settle이 ETX 대비 동급 이상.
- **일반 레이아웃의 GTK margin 계약을 M1부터 이관**한다(G1의 전제). "이전 viewport
  이미지를 이동시키고 새 프레임 요청"만 구현하면 GTK가 이미 해결한
  새 strip 검정·라벨 지연·불필요한 왕복이 웹에서 다시 생긴다. 현재
  GTK 계약: 배경 margin 요청(뷰포트 ±한 스텝, 라벨 포함,
  `labels_truncated`면 crop 제외)과 착지 프레임 보관, 동일 상태·배율
  에서의 crop, 16px fill 위상에 스냅한 커서 pan, 착지 margin을 새
  strip의 표시 base로 blit. 이 로직은 renderd가 아니라 gui.py의
  요청·표시 제어에 있으므로 worker 재사용만으로는 따라오지 않는다 —
  브라우저 측 일시 상태(§4)로 옮긴다. G1 판정에는 지연·pacing과 함께
  "새 strip 검정 0, 라벨 지연 0(margin 안)"을 포함한다.
- jobdeck에는 현재 없는 margin 기능을 전제하지 않는다. 덱은 현재 GTK
  덱 경로와 별도로 비교하고, prefetch 추가는 실측 후 별도 변경으로 다룬다.
- **G4 (Python-free/기능 parity)**: Python/PyGObject/KLayout이 없는
  실행 환경에서 Rust CLI→open/index/occupancy→layout/deck render→query/
  clip→DRC 조회·저장/내보내기를 검증한다. 단계별로 구현된 범위만 통과로
  표시하며 최종 판정은 §3.1b 전체 목록을 만족해야 한다. 개발 검증은
  기존 Python/KLayout 게이트를 비교 기준으로 쓸 수 있지만 제품 의존성은 아니다.

## 7. 브라우저 하한 (감사 선행)

- **step 0**: TeeBox `firefox --version` + 동료 데스크톱 대표 버전
  감사. 결과를 이 문서에 기록하고 browserslist 하한으로 박는다.
- 코어 요구는 canvas 2D, binary WebSocket, putImageData/drawImage, PNG와
  현장 브라우저에 맞춘 정적 자산이다. 기존 ESR 52/60 추정은 **지원 확정이나
  안전한 배포 버전 권고가 아니다**. 버전/feature probe/실행 게이트로 하한을
  정하고 유지보수·보안 정책도 확인한다. kiosk 미지원이면 일반 창으로 실행한다.
- 버전 감사와 간단한 ETX 실행 실험(프로필 격리·인스턴스 분리 포함)은
  M3가 아니라 **M0**에서 한다(§9).
- M1 필수 의존에서 제외: OffscreenCanvas, WebP/AVIF, 원본 신문법 배포,
  WebGL2. 최적화를 넣더라도 현장 하한에서 동작하는 기본 경로를 유지한다.
- 접속 첫 페이지에서 필요 API를 feature-detect — 미달이면 필요 버전
  안내를 명시 표출(조용한 오동작 금지).
- 주 작업자 경로(B)는 TeeBox의 Firefox 하나만 문제되므로 하한 협상이
  쉽다.

## 8. 배포·라이선스

- 번들: 기존 portable의 오프라인 빌드·호스트 호환성 검증을 재사용하되
  웹 제품은 Rust CLI/gateway/renderd/indexer + 정적 UI 자산으로 구성한다.
  Python/venv/PyGObject를 웹 제품 실행에 포함하지 않는다. 이관 중 GTK
  검증용 패키지는 구분한다. 브라우저 제공 방식은 M0 환경 감사에서 확정한다.
- Electron(선택, 후순위): 사용자 로컬 셸로만 검토, **현장 검증 전
  제외**(§1). 라이선스는 파일 동봉으로 단정하지 않고 **배포 조건
  체크리스트**로 확인한다: 실제 번들의 Chromium/FFmpeg 빌드 구성,
  FFmpeg(LGPL) 동적 링크 여부와 대응 소스 제공 의무, 코덱 특허
  (H.264/AAC → codec-free 빌드 선택), 고지 파일(`LICENSE`·
  `LICENSES.chromium.html`)의 Open Source Licenses 다이얼로그 편입.
- **보안 기본값**: 공유 서버 전제. A/B는 엄격한 loopback 바인딩(외부
  인터페이스 금지) + ephemeral port + 세션 토큰; **C(원격)는 HTTPS/
  WSS 기본**(폐쇄망이라는 이유로 평문을 기본으로 하지 않는다).
  공통: WebSocket Origin 검증, 토큰 만료·폐기, 명령별 권한 검사
  (게스트 토큰은 읽기 명령만), 공유 토큰은 특정 설계/DRC 세션에
  한정, 클라이언트가 서버 경로나 worker 명령을 직접 지정하는 인터페
  이스 금지(gateway가 화이트리스트 명령만 변환). URL 토큰은 로그·
  브라우저 기록에 남으므로 단기 토큰을 세션 쿠키로 교환하고 URL에서
  제거한다. 같은 TeeBox 계정이 여러 설계에 접근하므로 gateway의 권한
  검사가 접근 통제의 본체다. 토큰 없는 바인딩은 어느 배포형에서도
  금지.

## 9. 마일스톤

0. **M0 — 감사·실험**: TeeBox/대표 데스크톱 Firefox 버전 감사(§7),
   ETX에서 세션별 프로필·새 인스턴스 실행 실험(§3.3), Rust HTTP/WS
   라이브러리·오프라인 의존성 선정. Python 기능 목록/CLI parity 표,
   Rust 서비스 경계·자원 정책·인덱스 수명주기 초안을 확정한다. 현장
   실험과 로컬 설계 항목은 구분해 기록하고, 원격 환경 확인이 안 됐다고
   로컬 기능 목록·프로토콜 설계까지 멈추지는 않는다.
1. **M1 — Rust 기반 + 읽기 전용 웹 뷰어 (배포 A)**:

   - **M1a**: Rust CLI/공통 서비스·worker client. 레이아웃과 jobdeck
     파싱/소스/레벨 선택·캐시 검사·인덱싱 옵션·렌더 제어를 이관한다.
     기존 Python 구현과 CLI 출력/파일/오류·픽셀을 대조한다.
   - **M1b**: Rust gateway 정적 서빙·WS·토큰·프레임 봉투/취소/큐 상한,
     open/goto/pan/zoom/layer·level/chip·thin 상태, PNG/raw 비교.
     지원되는 일반 레이아웃의 margin 요청·착지 보관·crop·16px 스냅
     pan·표시 base를 이관한다. G1 및 읽기 경로 G4 측정까지.

2. **M2 — DRC 공유 뷰어 (배포 C)**: DRC 결과 목록/이동/waive 표시
   (읽기 전용), 게스트 토큰 URL 발급. 확정 수요 대응.
3. **M3 — ETX 게이트 (배포 B)**: M0의 TeeBox 환경/버전을 재확인하고 G2 실측.
   통과 시 launcher를 `--web`으로 전환할 준비, 미통과 시 원인
   분석(전송 단계 상향) 후 재시도.
4. **M4 — 조작 parity + Python-free 제품 전환**: pick/snap/룰러/clip/
   label 토글/단축키, DRC waive·주석·설정 저장, 내보내기·보조 CLI까지
   §3.1b 전체를 검증한다. GTK 셸 은퇴 판정은 이 단계의 G4 완료 + 현장
   검증 후이며, 라이브러리/빌드가 Rust라는 이유만으로 완료 처리하지 않는다.
5. **M5 — T3 전송(world-tile)**: F2R-10 본안과 통합 설계. 인접 pan
   클라이언트 합성 실측으로 world-tile LRU 착수 판정을 겸한다.

## 10. 미해결 질문 (감사·정책 확인 대기)

1. TeeBox·대표 데스크톱의 Firefox 버전 (→ §7 하한 확정).
2. 사용자 데스크톱 → TeeBox HTTP 허용 여부 — 허용이면 배포 B의
   지름길(portal이 뷰어 URL을 직접 열기, ETX 픽셀 전송 소멸)이
   열린다. 동료 접근(배포 C)이 이미 계획되어 있으므로 정책상 같은
   경로일 가능성이 있다.
3. portal → TeeBox launcher에 세션 URL/토큰 전달 채널의 형태.
4. linger 기본값과 공유 서버 자원 정책(§4, FLOE_RUST_BUDGET_MB 고정
   결정과 정합 필요).
5. 인덱스 revision 게시·기존 세션 유지·명시 전환·보존/회수 정책(§4.1).
6. 프론트엔드 언어/빌드 도구. 현재 HTML/Canvas 계획은 유지하되 Rust/WASM
   사용까지 사용자 요구로 확정된 것은 아니다. 서버/CLI의 Python 제거와
   구분한다. 개발용 Python 테스트·생성기까지 제거할 범위와 시점도 미확정.

## 11. 브랜치 운영과 착수 기준 (2026-09-12)

- **실측 기준**: `feature/jobdeck`, 작업 트리
  `/Users/journey/Flatide/floe2_review`. 실칩 jobdeck/occupancy의 화질·속도·
  메모리·재인덱싱 관찰과 그 수정은 이 브랜치에서 계속한다.
- **웹 전환**: `feature/webui`, 작업 트리
  `/Users/journey/Flatide/floe2_webui`. 시작 커밋은
  `6c33a482dab2649fa1c62ad135e63577c6301581`이며, `--lod` 전달과 occupancy
  병용 게이트가 포함된 시점이다. 머지 커밋 직전의 전체 검증은 통과했지만
  이것이 실칩 수용 판정을 대체하지 않는다.
- 실측 후 수정은 **`feature/jobdeck` → `feature/webui` 정방향 머지**로
  주기적으로 가져온다. 공통 수정은 가능한 한 실측 브랜치에서 먼저 고치고
  각 머지마다 기준 커밋·검증 결과를 기록한다. 공개된 작업 이력을 임의로
  rebase하지 않는다. 파일 복사나 반복 cherry-pick을 기본 동기화 방법으로
  삼지 않는다.
- 충돌 시 jobdeck 정책/렌더 정확도/캐시 형식은 실측 브랜치의 최신 계약을
  기준으로 보존한다. Python 쪽에 수정이 들어왔으면 이미 이관한 Rust
  서비스와 회귀 테스트에도 반영한다. 단순히 어느 한쪽 파일 전체를
  선택해 머지 완료로 보지 않는다.
- 웹 전환 미완성 코드를 실측 브랜치로 역머지하지 않는다. GTK/Python
  제거·공통 포맷 변경·배포 기본값 전환은 각 수용 기준을 통과한 뒤 별도
  합류 판정으로 진행한다. 원본 OASIS/실칩 프로파일은 커밋하지 않는다.
- **M0 로컬 산출물**: `WEBUI_M0.ko.md`에 10개 명령/93개 공개 옵션과 보조
  기능, `WEBUI_SERVICE_API.ko.md`에 서비스/전송/자원/revision 초안을 작성했다.
  `tools/audit_webui_env.sh`는 현장 기본 정보용 읽기 전용 도구다. 실제 브라우저
  기능·ETX·접속 측정과 dependency gate는 미완료이며 M0 전체 PASS가 아니다.
- **M1a-1 구현(2026-09-13)**: `rust/worker-client`에 handshake/open/style/
  frame/cancel/cleanup, bounded I/O와 오류/타임아웃을 추가했다. fake worker와
  실제 valmini의 Python 어댑터 PNG/raw 대조 게이트가 있다. 상세 호출 계약과
  poll/메모리 경계는 [worker-client README](../rust/worker-client/README.md).
- **M1a-2a 구현(2026-09-13)**: 개발용 `floe2-web index`의 일반 레이아웃
  경로(캐시/LOD/occupancy/프로파일). [M1a 기록](WEBUI_M1A.ko.md)에 범위와
  차이·회귀 게이트를 둔다. M1a-2b에서 info/단일 PNG render/probe도 이관했고,
  빈 가시 레이어 native plan도 별도 보완했다(M0-D7). M1a-3a에서 잡덱 문법·
  좌표·skip ledger 모델, M1a-3b에서 source catalog와 덱 index를 이관했다.
  M1a-3c/d에서 덱 색/레이어·spec·분석/읽기 CLI와 공통 Dataset을 연결했다.
  M1b-1에서 vendored HTTP/WS 의존성·loopback 인증/제한을 검증했다.
  M1b-2a에서 managed lease/admission·view controller를 검증했다.
  M1b-2b에서 실제 PNG/raw 스트림·frame credit·재접속/취소/종료를 검증했다.
  M1b-2c1에서 등록 범위와 실제 관리형 색인/진행/취소를 연결했다.
  M1b-2c2에서 catalog·재open·색인 작업의 HTTP/WS 경로를 연결했다.
  M1b-3에서 실행 명령·번들 Canvas 기본 UI, private Firefox launcher와
  PNG/raw·최초 한 번의 설정 적용·재접속/종료를 검증했다. M1b-4a에서 layout
  margin의 픽셀 parity·16px pan·라벨/실패 fallback을 검증했다. M1b-4b에서
  drag·스타일/글꼴·기본 단축키와 실제 Chrome 조작을 검증했다. M2a-1에서 기존
  DRC pack/waive 읽기 CLI·페이지 조회를 검증했다. M2a-2는 DRC actor/인증 API,
  M2a-3는 읽기 패널·focus·overlay, M2a-4는 서버 상태 보존·브라우저 복원,
  M2a-5는 현재 규칙 내 유계 순회 API/UI와 선택/이동 모드,
  M2a-6은 화면에 그린 DRC 마커의 클릭 선택/이동,
  M2a-7은 단순 오류의 CD 측정 코어/API 및 표시·지우기·복원,
  M2a-8은 규칙별 선택 집합·박스·금색 마커/복원,
  M2a-9는 Selected/waive/live In view 교집합 목록·순회·hover다.
  M2a-10a는 SVRF sidecar 읽기·타입/derivation·측정 비교 코어/CLI이며,
  M2a-10b는 등록된 metadata/타입·규칙 필터·scalar 비교 API다.
  M2a-10c는 웹 타입/규칙 상세/측정 비교와 In view 해제다.
  M2a-10d1/10d2는 레이어 격리/복원·원자적 focus 서버와 웹 승인 처리·Restore/Escape다.
  M2a-11a/11b는 ASCII DRC 읽기 코어/CLI fallback과 명시 등록한 웹 ASCII의
  조회·선택·순회·윤곽/CD다. M2a-12a는 관리형 pack-build 코어/CLI와
  파일 보존·진행/취소다. M2a-12b1은 승인된 HTTP 작업·기존 actor 종료·새 identity 등록이며,
  M2a-12b2는 브라우저 승인/진행/취소와 새 catalog 조회 복원이다.
  실제 브라우저의 승인 클릭 수용은 별도로 남아 있다(M2 §24).
  jobdeck 물리 plane 격리는 남아 있다. 원본 SVRF subset parser는 M4b-5의 로컬 CLI로
  이관했으며 web API의 임의 deck/include 접근을 추가한 것은 아니다.
  공유와 고급 DRC 조작은 남아 있다.
  공유 권한 경로 추가는 안전 검토 차단 후 승인 대기다. M4a-1/2/3은 독립적인
  native/controller와 기존 owner WebSocket 질의이며 공유 권한이나 외부 공개는 추가하지 않는다.
  CLI 전체/웹 전환 완료가 아니며 GTK/실측 브랜치는 유지한다.
  M4e-1에서 DRC waive/주석 포맷과 메모리 편집 모델을 Rust로 이관했다
  ([M4 §21](WEBUI_M4.ko.md)). 실제 review 저장/충돌/API/UI는 다음 단계이며,
  공유 권한 추가나 현장 수용을 대신하지 않는다.
  M4e-2a는 같은 codec의 로컬 원자 저장·파일 충돌·pack binding과 legacy 확인 경계다
  ([M4 §22](WEBUI_M4.ko.md)). 관리형 writer/API/UI·실제 autosave 성능 검증은 남아 있다.
  M4e-2b는 native 관리형 writer의 admission·pack/source lease·취소·typed 결과와 join을
  연결했다([M4 §23](WEBUI_M4.ko.md)). owner/API/UI 및 autosave 실측은 후속이다.
  M4e-2c는 기존 읽기 actor와 store의 pack identity 대조·선택 waive 상태 읽기다
  ([M4 §24](WEBUI_M4.ko.md)). HTTP 쓰기 endpoint와 편집 UI를 추가한 것은 아니다.
  M4e-3a에서 명시 reviewer opt-in의 owner 주석 승인/게시 API를 연결했다
  ([M4 §25](WEBUI_M4.ko.md)). 주석 UI·autosave·waive 쓰기·import/export는 다음 단계다.
  M4e-3b는 선택 주석 read/edit/preview·명시 승인/결과·동일 승인만 복구하는 UI다
  ([M4 §26](WEBUI_M4.ko.md)). 초기 승인 서비스 오류 후 Chrome 합성 읽기·편집·미리보기·
  만료/선택 변경 문구 보존·폐기/종료까지 확인했다. 실제 브라우저 게시는 미실시다.
  주석 badge/overlay·autosave·waive 쓰기·import/export·현장 수용은 남는다.
  M4e-4a는 검증한 waive snapshot만 기존 reader에 적용하는 native/actor 경로다
  ([M4 §27](WEBUI_M4.ko.md)). geometry 캐시를 보존한다. M4e-4b에서 같은 geometry id의
  조회 revision을 갱신하고 오래된 ticket/HTTP·필터·선택/prepared focus를 fence했다
  ([M4 §28](WEBUI_M4.ko.md)). M4e-4c는 별도 `--drc-edit-waives` opt-in의 owner waive
  승인 API와 게시/조회 반영 receipt를 연결한다([M4 §29](WEBUI_M4.ko.md)). 게시한 동일
  파일만 기존 reader에 적용하며 외부 교체는 명시 reopen을 유지한다. M4e-4d는 waive
  action/preview·별도 승인·디스크/reader receipt와 일치 revision에서 조회 재개하는 UI다
  ([M4 §30](WEBUI_M4.ko.md)). 실제 브라우저 게시·현장 수용은 남으며 자동 저장/공유
  권한은 추가하지 않는다. 주석 badge/overlay·명시 import/export도 후속이다.
- **M4e-5a**: 저장 주석 표시용 owner projection API와 admitted snapshot cache를
  추가했다([M4 §31](WEBUI_M4.ko.md)). 목록 최대512개 배지와 이동 대상 하나의 본문만
  읽고 편집 snapshot/preview는 보존한다. badge·본문 overlay UI는 다음 연결이며
  주석 import/export·현장 수용을 완료로 처리하지 않는다.
- **M4e-5b**: 저장 주석 배지·좌상단 본문을 위 API에 연결했다([M4 §32](WEBUI_M4.ko.md)).
  현재 선택과 마지막 ACK 이동을 분리하고 Markers/overlay·서버 상태 복원·pan 무조회·
  편집 preview 보존·저장/늦은 응답 장벽을 검증했다. Chrome 합성 표시/복원을 확인했으며
  브라우저 주석 게시·clipboard·현장 수용은 별도다. 주석 import/export는 다음 단계다.
- **M4e-6a**: 주석/waive snapshot export와 streaming 전체 waive import를 native/managed
  store에 연결했다([M4 §33](WEBUI_M4.ko.md)). 전체 교체·import 확인·입력/대상 충돌과
  취소·admission을 검증했다. 새 HTTP/upload/download/UI는 없으며 owner 전송 연결은 후속이다.
- **M4e-6b**: 위 native 경로에 owner 전용 분할 업로드·비동기 준비/내보내기·만료
  artifact 다운로드 API를 연결했다([M4 §34](WEBUI_M4.ko.md)). 전체 review 교체는
  별도 승인과 portable run 확인 후 기존 게시 경로로만 수행한다. 일반 파일 업로드/guest
  권한은 추가하지 않는다.
- **M4e-6c**: 전체 review 전송 패널을 연결했다([M4 §35](WEBUI_M4.ko.md)). 오류 선택과
  독립적인 1MiB Blob 전송·불명확한 동일 요청만 재시도·전체 교체/DRC run 이중 확인,
  기존 저장 패널의 receipt/복구와 waive reader 장벽을 재사용한다. Chrome 합성 export
  준비/종료는 확인했다. 브라우저 업로드·실제 review 게시·다운로드 파일 수용과 현장
  Firefox/ETX/NFS는 아직 남는다.
- **M4f-1**: Python-free `selfcheck`와 앱 source/target/bundle 식별을 추가했다
  ([M4 §36](WEBUI_M4.ko.md)). 인접 Rust 바이너리만 검사하는 모드와 정상 도구 검색을
  구분하며, 버전 subprocess의 EOF까지 5초 기한을 적용한다. 실제 브라우저 실행/게시
  권한을 확대하지 않는다. 전용 portable 조립·ELF/GLIBC 감사와 현장 수용은 다음 단계다.
- **M4f-2**: 별도 Rust 웹 portable 조립기를 추가했다([M4 §37](WEBUI_M4.ko.md)).
  설치된 툴체인으로만 offline 빌드하고, Linux GNU/musl ELF·동적 버전 요구·원본 고지·
  전체 파일 hash를 검사한다. Linux 조립은 native selfcheck 필수, macOS 교차 조립은
  미실행 표시다. 기존 GTK portable과 기본 실행기를 유지한다.
- **M4f-3a**: 인증된 About GET과 읽기 전용 모달. 앱/소스/target/bundle 및 native 호환
  요구값을 구분하며 내장 글꼴 원문만 표시한다. 전체 고지 열람 UI는 후속 M4f-3b로 분리했고,
  실제 Linux/Firefox/ETX와 SYS-02 전체 수용은 별도로 남긴다.
- **M4f-3b**: 새 portable의 compiled notice index·원본 chunk 검증과 읽기 전용 목록/
  본문 페이징을 추가했다. 개발 빌드/구 배포본은 고지 범위를 명시한다. 전체 패키지
  검증·게시자 인증·현장 수용을 대신하지 않는다([M4 §39](WEBUI_M4.ko.md)).
