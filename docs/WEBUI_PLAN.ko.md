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
- Electron을 TeeBox에서 실행해 X/ETX로 쏘는 형태. Chromium은 원격
  디스플레이에서 GPU 합성이 죽고 전체 창 픽셀을 계속 전송하므로
  **금지 패턴**으로 명시한다.
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

- **M1~M2: Python gateway**(floe 패키지 내부, stdlib만 — WS는 RFC6455
  최소 구현). worker 계약 직결이라 두 제품 동시 지원이 즉시 성립하고
  vendored HTTP 크레이트 선정이 불필요하다. T0/T1(PNG 셔틀)에는 성능
  충분.
- **T2/T3 시점: Rust gatewayd**로 이관(floe2 전용 구간). 의존성은
  전부 `vendor/` 동봉(기존 빌드 정책: 빌드 중 네트워크 0), HTTP/WS는
  최소 구현 크레이트를 vendored로 선정한다.
- backend별 capability는 handshake로 협상한다: T2 raw RGBA와 T3
  world-tile은 floe2 전용(KLayout LayoutView는 전체 viewport 렌더라
  T3 불가), floe는 T0/T1로 동작.
- 버전은 floe/cli/renderd와 동일 스탬프 체계로 묶고(`--version`,
  시작 스탬프), **UI 자산은 반드시 자기 번들의 것만 서빙**해 UI ↔
  gateway ↔ renderd 버전 skew를 원천 차단한다.

### 3.2 HTML UI

- canvas 2D 단일 뷰포트 + DOM 오버레이(라벨/룰러/상태줄). WebGL,
  OffscreenCanvas, WebP/AVIF 등 신기능 의존 금지(§7 하한).
- 상시 애니메이션 금지, 프레임 단위 통짜 갱신 — ETX(TXP) 압축
  친화적으로(배포 B 대비).
- 정적 파일은 전부 번들 동봉. CDN·외부 폰트 금지(폐쇄망).
- 빌드: browserslist 하한(§7 감사 후 확정) + ES2017 transpile + 호환
  lint를 CI 게이트로.

### 3.3 launcher 통합

- `floe2 view <src> --web`(가칭): gatewayd+renderd 기동 → 토큰 URL
  생성 → `firefox --kiosk <URL>` 실행(배포 A/B 공용). DISPLAY는
  호출측 환경을 그대로 따르므로 TeeBox launcher 수정이 불필요하다.
- 공유 URL 발급: 열린 세션에서 읽기 전용 게스트 토큰 URL을
  발급한다(배포 C, DRC 공유 흐름).

## 4. 세션·상태 계약

- **뷰 상태는 전부 서버(gateway 세션)에 둔다**: viewport, depth,
  detail, layer 가시성, style epoch, DRC 선택/waive(파일 기반 기존
  체계 재사용), goto 히스토리.
- 새로고침/재접속 = 세션 재부착 후 완전 복원. 클라이언트 전용 상태를
  만들지 않는 것이 설계 원칙(리프레시 위험 제거의 본체).
- UI 종료 후 gateway/renderd는 **linger**(기본 수 분) — 재열기 즉시
  복원 + decoded LRU 보존(F2R-10 보존 스토리와 합류). linger 상한과
  명시 종료 경로를 둔다(고아 방지: renderd의 start_new_session,
  watchdog 경험 재사용).
- 다중 클라이언트(작업자 + 게스트 N)는 같은 scene을 읽되, 조작
  권한은 토큰 등급으로 구분한다(게스트 = 읽기 전용).

## 5. 전송 계층 (진화 단계)

| 단계 | 프레임 경로 | 비고 |
|---|---|---|
| T0 | 현행 PNG 파일 publish를 gateway가 읽어 WS로 전달 | renderd 무변경, M1 범위 |
| T1 | renderd→gateway 직접 스트림(PNG) | 파일 publish/fsync 제거 |
| T2 | loopback 한정 raw RGBA | PNG encode(실측 24~206ms) 생략, 배포 A/B 이득. **floe2 전용**. payload는 F2R-13의 `FLOERAW1`(0.12.26 제품 구현) 재사용 |
| T3 | world-tile 단위 delta + 클라이언트 tile 캐시 | F2R-10/11 합류 지점. 인접 pan의 draw 재지불을 클라이언트 합성으로 흡수. **floe2 전용, 조건부 보류**(F2R-03c 선행 — FLOE2_OPTIMIZATION §3.16) |

- T0은 floe(KLayout `save_image` PNG)와 floe2 모두 무수정 동작 —
  backend 중립의 기준선.

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
- 스테일 프레임 transform pan(redraw 대기 중 이전 프레임을 canvas
  transform으로 이동)은 G1/G2와 무관하게 기본 탑재 — GTK frozen
  preview 대비 체감 우위 항목.

## 7. 브라우저 하한 (감사 선행)

- **step 0**: TeeBox `firefox --version` + 동료 데스크톱 대표 버전
  감사. 결과를 이 문서에 기록하고 browserslist 하한으로 박는다.
- 코어 요구는 보수적으로 설계되어 ESR 52/60(2017~18)급이면 충분:
  canvas 2D, binary WebSocket, putImageData/drawImage, PNG,
  ES2017(transpile 산출), flex/grid.
- 금지 목록(구버전 파손원): OffscreenCanvas(105+), WebP(65+)/AVIF,
  원본 신문법 배포, WebGL2 의존.
- 접속 첫 페이지에서 필요 API를 feature-detect — 미달이면 필요 버전
  안내를 명시 표출(조용한 오동작 금지).
- 주 작업자 경로(B)는 TeeBox의 Firefox 하나만 문제되므로 하한 협상이
  쉽다.

## 8. 배포·라이선스

- 번들: make_portable.sh 체계에 gatewayd 바이너리 + 정적 UI 자산
  추가. 폐쇄망 반입은 기존 zip/버전 스탬프 절차 그대로.
- Electron(선택, 후순위): 사용자 로컬 셸로만 검토. 번들 재배포는
  MIT/BSD 계열로 적법 — 의무는 `LICENSE`·`LICENSES.chromium.html`
  동봉(기존 Open Source Licenses 다이얼로그/번들 절차에 편입),
  ffmpeg(LGPL)는 기본 동적 링크 유지. H.264/AAC 특허가 마음에
  걸리면 codec-free ffmpeg로 교체. **TeeBox에서 실행해 ETX로 쏘는
  용도로는 사용하지 않는다**(§1 비목표).
- 보안: 공유 서버 전제 — ephemeral port + 세션 토큰 필수, 게스트
  토큰은 읽기 전용·만료 시간 부여. 폐쇄망이므로 TLS는 선택이되 토큰
  없는 바인딩은 금지.

## 9. 마일스톤

1. **M1 — gateway 스켈레톤 + 읽기 전용 뷰어 (배포 A)**: 정적 서빙,
   WS 프록시(T0), 토큰, open/goto/pan/zoom/layer toggle, 스테일
   transform pan. 게이트 G1 측정까지.
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
