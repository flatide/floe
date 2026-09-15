# 웹 전환 G4 잔여 감사

갱신: 2026-09-16, M4g-13. 상위 [계획](WEBUI_PLAN.ko.md), 원래 범위
[M0 §2~3](WEBUI_M0.ko.md), 단계별 실행 기록 [M4](WEBUI_M4.ko.md).

이 문서는 **로컬 구현과 전체 수용을 분리하는 잔여 목록**이다. 표의 구현/게이트는
기능 경로의 존재와 로컬 회귀 근거이며, GTK 은퇴 또는 실제 브라우저 동작의 최종
합격표가 아니다. 커밋 개수를 완료율로 환산하지 않는다. 아래는 1차 대조이며 새
누락을 발견하면 추가한다. 고객 파일·현장 프로파일은 저장하지 않는다.

## 1. 범위별 현재 근거와 남은 일

| 범위 | 로컬 구현/검증 근거 | 아직 닫지 않은 부분 |
|---|---|---|
| CLI 10종·보조 PNG 명령 | `rust/app`, `rust/app-core`; `validate_app_cli.py`, `validate_app_render.py`, `validate_app_jobdeck*.py`, `validate_fe_embed.py` 등. view 옵션/형식 대조 M4 §62 | view의 reviewer **읽기 선택** 이관, 무효/개발 옵션의 최종 제품 경계. GDS/gzip은 기존 native 한계이며 새 지원 아님 |
| UI-01 열기·조작 | `launcher.js`, `browse.js`, `index-open.js`, `gestures.js`, `minimap.js`, `app.js`; 실제 startup/IPC·DOM gate | 실제 브라우저 포커스·키·wheel·resize·재접속 수용, G1 input→photon/pacing |
| UI-02 pan/margin | `view_controller`, `stream`, `app.js`의 착지 margin/16px 위상·crop/표시 base, controller/stream/client gate | GTK 대비 새 strip·라벨 지연 실제 화면 측정. 덱 margin은 기존에도 미지원 |
| UI-03 레이어·스타일 | Rust 선택/CAS, palette/presets/settings/defaults; GTK 원본 선택·접힘·상속 oracle, HTTP/DOM gate | 실제 선택·다중 스타일·기본값 게시 화면 수용. 개발 bitmap **슬롯** 편집은 미이관 |
| UI-04 pick/snap/측정 | query/inspect/measure/rulers + Rust query/export, 숫자·scene 유효성·stale gate | 실제 포인터/클립보드·시각적 측정 검증 |
| UI-05 입력·복사·종료 | snapshot/session-exit와 단축키 보호. **M4g-13 두벌식 fallback**: `hangul.js`/`drc-notes.js`, GTK 원본 조합 oracle | OS IME와 fallback의 실제 입력·스크롤·키보드/브라우저별 수용. DOM gate로 대체하지 않음 |
| DRC-01 조회·선택 | `app-core/drc`, `web/src/drc`, `drc*.js`; lazy paging/selection/CD/isolation/query gate | 현장 대형 결과와 실제 브라우저 조작 수용 |
| DRC-02 저장·전송 | reviewer 고정 sidecar, snapshot/prepare/approve·CAS·receipt, notes/waives/transfer HTTP와 UI gate | **확정 시 자동 저장 opt-in 구현**(아래 사용자 결정), reviewer 읽기 선택; 실제 저장/충돌/복구 UI 수용 |
| EXPORT-01 | Rust capture/mosaic/PNG metadata/clip + snapshot; raster/metadata/DRC-capture gate | 실제 브라우저 copy/download/승인 표시 수용 |
| SYS-01/02 | Rust worker 발견·수거·cache freshness·selfcheck·portable/ELF/고지; native/포장/전송 gate | Python-free **Linux에서 실행**, 현장 Firefox/ETX, G4 전체 end-to-end 판정 |

표의 `validate_*.py`와 Node는 개발 오라클/하네스다. 제품 실행 경로에 Python,
KLayout, Node를 다시 넣지 않는다. 브라우저의 입력 조합·표시 일시 상태는 계획대로
정적 JS에 두고, geometry/조회/파일 저장·권한·충돌 판정은 Rust에 둔다.

## 2. 자동 저장 — 사용자 결정, 다음 구현

2026-09-16 사용자 선택: **reviewer별 자동 저장을 먼저 명시적으로 켜는 opt-in**.
현재 M4g-13까지는 여전히 매번 prepare/approve이며 이 결정만으로 켜지지 않는다.

- 기본 off. 서버가 등록한 reviewer와 허용된 note/waive sidecar 범위 안에서만 동작.
  reviewer 선택 자체를 쓰기 동의로 간주하지 않는다.
- GTK처럼 노트는 **편집 확정** 시, waive는 **상태 변경 확정** 시 저장한다.
  글자 입력마다, IME 조합 중, blur/선택 변경/창 닫기만으로 저장하지 않는다.
- 기존 snapshot의 DRC/revision/선택·sidecar CAS, 충돌/잠금/원자 게시·receipt를
  보존한다. 새 대상·다른 DB로 넘어간 초안을 자동 적용하거나 외부 변경을 덮어쓰지 않는다.
- 실패/만료/결과 불명은 사용자에게 표시하고 초안을 보존한다. 자동 재시도·자동
  legacy sidecar 채택·in-pack fallback을 추가하지 않는다. import/export와 공유
  기본값 게시의 동의는 이 opt-in에 포함하지 않는다.
- 첫 구현은 **현재 탭·등록된 reviewer 범위**의 로컬 opt-in으로 한다. notes/waives를
  각각 켤 수 있게 하며 서버의 기존 `--drc-reviewer`/`--drc-edit-waives` 권한이 없으면
  활성화하지 못한다. 설정 파일·sessionStorage로 opt-in을 자동 복원하지 않는다.
- 사용자 확정 시 opt-in 세대를 포착하고 read/prepare/승인 직전 다시 확인한다.
  도중 해제 또는 해제→재활성화는 이미 시작한 준비에 새 승인을 주지 않는다.
  이미 제출된 저장을 opt-out으로 되돌렸다고 표시하지 않는다. disconnect/종료는
  opt-in을 해제하며 미확인 receipt의 기존 명시 복구 경로를 유지한다.
- 서버의 read/prepare/submit API를 유지하고 UI 확정 동작만 기존 승인 요청에
  연결하는 범위다. 서버 배경 저장기·주기 저장·새 공유 권한을 추가하지 않는다.
  파일 게시와 waive reader refresh 결과는 계속 별도로 표시한다. 기존 수동 경로도
  유지한다. 이 세부 규칙은 다음 구현과 지연 응답/해제/충돌 gate로 검증한다.

성능 주의: 현재 sidecar CAS는 파일 전체 해시/재작성 비용이 있다. 단일 operation
진행 중 추가 저장을 쌓지 않으며 대형 waive 파일의 연속 클릭 성능은 실측 대상이다.
opt-in UI 연결만으로 GTK pwrite와 같은 비용이라고 주장하지 않는다(M4 §23~30).

GTK `--floe-reviewer`는 표시 이름뿐 아니라 기존 reviewer waive 파일의 읽기 선택에
영향을 준다(`floe/cli.py`, `floe/drc.py`). native `drc`/`render`의 읽기 선택은 있지만,
view에서 이를 `--drc-reviewer` 쓰기 등록으로 단순 치환하면 권한 의미가 달라진다.
읽기 선택/쓰기 opt-in 분리는 별도 잔여이며 “태그만 미지원”이라고 축소하지 않는다.

## 3. 로컬 기능 완성과 구별할 목표 잔여

1. 위 자동 저장·reviewer 읽기 선택, 개발 bitmap 슬롯/무효 CLI 경계의 마감과
   G4 목록의 최종 재대조.
2. 실제 브라우저 입력·저장·복구·화면 수용, Python-free Linux 실행, G1/G4 판정.
   이전 브라우저 시작 파일의 도구 제한을 우회하지 않는다.
3. M2 공유/원격은 `shares=false`, loopback-only인 **미구현**이다. 허가 없는 원격
   노출/공유 API 확장을 로컬 구현의 자연스러운 연장으로 추론하지 않는다.
4. M0/M3 TeeBox Firefox/ETX는 현장 실행 불가로 보류. 사용자에게 같은 측정을
   반복 요청하거나 로컬 결과를 현장 PASS로 바꾸지 않는다.
5. M5 world-tile은 성능 전제·실측에 따른 조건부 단계. 미구현을 완료로 세지 않는다.
   index hot reload/revision은 사용자가 별도 설계로 유보한 범위다.

따라서 “이번 커밋으로 한 기능 경로를 닫음”과 “웹 전환 전체 완료”를 구별해 보고한다.
