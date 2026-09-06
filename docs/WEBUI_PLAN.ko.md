# floe2 웹 셸 / 서버-클라이언트 계획 (정본)

작성 2026-08-29. 관련 정본: `FLOE2_OPTIMIZATION.ko.md`(F2R-10/11),
`RUST_RENDERER_PLAN.ko.md`, `SPEC-VIEWER.ko.md`, `rust/BUILD.md`.

## 0. 결정 로그 (사용자 확정 사항)

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

## 1. 목표와 비목표

목표:

1. HTML/canvas 기반 뷰어 UI 하나로 세 배포형을 커버한다(§2).
2. 렌더·플랜 성능 자산(F2R 계열)은 그대로 재사용한다 — 무거운 일은
   전부 renderd가 하고, 셸 교체는 "마지막 수십 ms + 상호작용 체감"의
   문제다.
3. 서버 세션 설계로 "브라우저 리프레시 = 상태 소실" 위험을 제거한다.
4. F2R-10(world-tile) / F2R-11(streaming)과 합류 가능한 전송 계층을
   설계한다 — 클라이언트 tile 합성이 world-tile LRU의 자연스러운
   구현처가 된다.

비목표:

- GTK 셸의 즉시 대체. GTK는 parity + ETX 게이트(§6) 통과 전까지 주
  작업자용으로 병존한다(worker job/result 계약이 GUI 중립이라 가능).
- Electron을 TeeBox에서 실행해 X/ETX로 쏘는 형태는 **현장 검증 전
  제외**. 원격 디스플레이에서 Chromium 합성 비용이 크다는 우려는
  해당 ETX 구성에서 실측되지 않았고, Firefox도 배포 B에서는 결국
  ETX 화면 전송을 거치므로 기술적 단정이 아니라 우선순위 결정이다.
- 초기 단계의 편집·계측 고급 기능 parity. M1~M2는 읽기 중심이다.

## 2. 아키텍처: 한 스택, 세 배포형

```
[공통 스택]   gateway ──[worker 계약]── RustRenderWorker → floe-renderd   (floe2)
                 │            └──────── RenderWorker → KLayout+vfsd        (floe)
                 │ 정적 UI 서빙 + WS + 세션/토큰
                 ▼
             HTML UI (canvas 2D)

배포 A  데스크톱 패키징: launcher가 gatewayd+renderd를 함께 기동,
        UI는 로컬 브라우저(firefox --kiosk)로 loopback 접속.
배포 B  주 작업자(ETX): TeeBox 계정이 A와 동일 구성을 기동하되
        firefox의 DISPLAY를 ETX로 지정. portal→TeeBox 실행 흐름이
        한 글자도 안 바뀐다(§0). B는 A의 특수형이다.
배포 C  동료/원격 뷰어: gatewayd만 TeeBox에서 서빙, 사용자 자신의
        브라우저가 네트워크로 접속. 픽셀은 로컬에서 그려진다.
```

- B가 A의 특수형이므로 **데스크톱 패키징을 만들면 ETX 흐름은 공짜**.
- C는 바인딩 주소/토큰 전달만 다르다.
- 향후 로컬 Electron 셸은 "C에 붙는 선택적 데스크톱 래퍼"로 분리
  판단한다(§8) — TeeBox 실행 모델과 무관한 사용자측 배포 정책 문제.

## 3. 컴포넌트

### 3.1 gateway (신규)

- 역할: 정적 UI 자산 서빙, WS ↔ **worker 계약** 브리지, 세션·토큰,
  수명주기. **얇게 유지한다** — 뷰 로직을 넣지 않는다.
- **경계는 `make_render_worker`의 job/result 계약이다**(3.1a). 이
  계약은 GTK gui.py가 두 backend를 구분 없이 구동해 온 검증된
  GUI-중립 인터페이스로, 여기 두면 floe/floe2가 같은 웹 셸을 쓰고
  T0 전송은 양쪽 모두 무수정으로 성립한다.

#### 3.1a 단계별 구현체

- **M1~M2: Python gateway**(floe 패키지 내부). worker 계약 직결이라
  두 제품 동시 지원이 즉시 성립한다. WS는 **검증된 라이브러리를
  오프라인 번들**(vendored wheel, 빌드·실행 중 네트워크 0)로 쓴다 —
  RFC6455 자체 구현은 이 범위에 비해 위험(프레임 길이·fragmentation·
  제어 프레임·비정상 종료)을 늘린다. 자체 구현을 유지해야 한다면 위
  네 항목의 적합성 검증을 별도 CI 게이트로 둔다.
- **Rust gatewayd 이관은 조건부**: raw RGBA 전달(T2)은 Python에서도
  가능하므로 현재 worker 경계를 유지한 채 측정하고, gateway 자체가
  병목으로 실측될 때만 이관한다(두 제품 지원·유지보수에 유리). 이관
  시 의존성은 전부 `vendor/` 동봉, HTTP/WS는 최소 구현 크레이트를
  vendored로 선정한다.
- backend별 capability는 handshake로 협상한다: T2 raw RGBA와 T3
  world-tile은 floe2 전용(KLayout LayoutView는 전체 viewport 렌더라
  T3 불가), floe는 T0/T1로 동작.
- 버전은 floe/cli/renderd와 동일 스탬프 체계로 묶고(`--version`,
  시작 스탬프), **UI 자산은 반드시 자기 번들의 것만 서빙**한다. 번들
  일치만으로 skew가 사라지지는 않으므로 추가로: handshake에 프로토콜
  버전을 싣고 불일치는 명시 거부, 이미 열린 구버전 탭의 재접속은
  "새로고침 필요" 안내 후 차단, UI 갱신 정책(gateway 재기동 시 열린
  탭 강제 리로드 여부)을 명시한다.

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

- `floe2 view <src> --web`(가칭): gatewayd+renderd 기동 → 토큰 URL
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
  (파일 기반 기존 체계 재사용), goto 히스토리.
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
- UI 종료 후 gateway/renderd는 **linger**(기본 수 분) — 재열기 즉시
  복원 + decoded LRU 보존(F2R-10 보존 스토리와 합류). linger 상한과
  명시 종료 경로를 둔다(고아 방지: renderd의 start_new_session,
  watchdog 경험 재사용).
- 다중 클라이언트(작업자 + 게스트 N)의 조작 권한은 토큰 등급으로
  구분한다(게스트 = 읽기 전용). "같은 scene"의 의미는 위 뷰 세션
  모델을 따른다.

## 5. 전송 계층 (진화 단계)

| 단계 | 프레임 경로 | 비고 |
|---|---|---|
| T0 | worker 계약의 result(`png` 또는 `rgba`)를 gateway가 WS로 전달 | renderd 무변경, M1 범위. adapter가 publish 파일을 읽은 뒤 삭제하므로 gateway는 파일을 읽지 않는다. Rust 기본이 raw이므로 T0 job은 `frame_format="png"`를 명시 |
| T1 | renderd→gateway 직접 스트림(PNG) | 파일 publish/fsync 제거 |
| T2 | loopback 한정 raw RGBA | PNG encode(실측 24~206ms) 생략, 배포 A/B 이득. **floe2 전용**. payload는 F2R-13의 `FLOERAW1`(0.12.26 제품 구현) 재사용 |
| T3 | world-tile 단위 delta + 클라이언트 tile 캐시 | F2R-10/11 합류 지점. 인접 pan의 draw 재지불을 클라이언트 합성으로 흡수. **floe2 전용, 조건부 보류**(F2R-03c 선행 — FLOE2_OPTIMIZATION §3.16) |

- T0은 floe(KLayout `save_image` PNG)와 floe2 모두 무수정 동작 —
  backend 중립의 기준선.
- **프레임 봉투와 취소 계약**(원자적 파일 publish가 보장하던 것을
  네트워크에서 보존): 프레임마다 session/view id, 요청 순번,
  generation, render-state revision(layer/depth/style epoch), bbox,
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

- T0→T1→T2는 배포형별 협상(capability handshake)으로 공존 가능하게.
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
- **GTK의 margin 계약을 M1부터 이관**한다(G1의 전제). "이전 viewport
  이미지를 이동시키고 새 프레임 요청"만 구현하면 GTK가 이미 해결한
  새 strip 검정·라벨 지연·불필요한 왕복이 웹에서 다시 생긴다. 현재
  GTK 계약: 배경 margin 요청(뷰포트 ±한 스텝, 라벨 포함,
  `labels_truncated`면 crop 제외)과 착지 프레임 보관, 동일 상태·배율
  에서의 crop, 16px fill 위상에 스냅한 커서 pan, 착지 margin을 새
  strip의 표시 base로 blit. 이 로직은 renderd가 아니라 gui.py의
  요청·표시 제어에 있으므로 worker 재사용만으로는 따라오지 않는다 —
  브라우저 측 일시 상태(§4)로 옮긴다. G1 판정에는 지연·pacing과 함께
  "새 strip 검정 0, 라벨 지연 0(margin 안)"을 포함한다.

## 7. 브라우저 하한 (감사 선행)

- **step 0**: TeeBox `firefox --version` + 동료 데스크톱 대표 버전
  감사. 결과를 이 문서에 기록하고 browserslist 하한으로 박는다.
- 코어 요구는 보수적으로 설계되어 ESR 52/60(2017~18)급이면 충분:
  canvas 2D, binary WebSocket, putImageData/drawImage, PNG,
  ES2017(transpile 산출), flex/grid. 단 **`--kiosk`는 Firefox 71+**
  이므로 하한이 그 아래면 launcher는 일반 창으로 실행한다(§3.3).
- 버전 감사와 간단한 ETX 실행 실험(프로필 격리·인스턴스 분리 포함)은
  M3가 아니라 **M0**에서 한다(§9).
- 금지 목록(구버전 파손원): OffscreenCanvas(105+), WebP(65+)/AVIF,
  원본 신문법 배포, WebGL2 의존.
- 접속 첫 페이지에서 필요 API를 feature-detect — 미달이면 필요 버전
  안내를 명시 표출(조용한 오동작 금지).
- 주 작업자 경로(B)는 TeeBox의 Firefox 하나만 문제되므로 하한 협상이
  쉽다.

## 8. 배포·라이선스

- 번들: make_portable.sh 체계에 gatewayd 바이너리 + 정적 UI 자산
  추가. 폐쇄망 반입은 기존 zip/버전 스탬프 절차 그대로.
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
   ETX에서 세션별 프로필·새 인스턴스 실행 실험(§3.3), WS 라이브러리
   vendored 선정(§3.1a). 결과를 이 문서에 기록.
1. **M1 — gateway 스켈레톤 + 읽기 전용 뷰어 (배포 A)**: 정적 서빙,
   WS 프록시(T0: worker result 전달, `frame_format="png"`), 프레임
   봉투·취소·큐 상한(§5), 토큰(§8 기본값), open/goto/pan/zoom/layer
   toggle, **GTK margin 계약 이관**(§6: margin 요청·착지 보관·crop·
   16px 스냅 pan·표시 base). 게이트 G1 측정까지.
2. **M2 — DRC 공유 뷰어 (배포 C)**: DRC 결과 목록/이동/waive 표시
   (읽기 전용), 게스트 토큰 URL 발급. 확정 수요 대응.
3. **M3 — ETX 게이트 (배포 B)**: TeeBox Firefox 버전 감사 + G2 실측.
   통과 시 launcher를 `--web`으로 전환할 준비, 미통과 시 원인
   분석(전송 단계 상향) 후 재시도.
4. **M4 — 조작 parity**: pick/snap/룰러/clip/label 토글/단축키.
   GTK 셸 은퇴 판정은 이 단계 완료 + 현장 검증 후.
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
