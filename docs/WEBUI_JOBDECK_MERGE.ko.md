# feature/jobdeck → feature/webui 정방향 병합 (2026-09-28)

## 범위

- 대상: `feature/webui`의 `88fe6eb` 위에 `feature/jobdeck`의 `36d8a52`를 merge.
- 양쪽 이력을 보존한다. jobdeck 작업 트리와 사용자의 소개·브로슈어 문서는 변경하지 않는다.
- jobdeck의 area-true raster, placement lattice, density stack 진단 경로,
  scale별 budget-fit 기억, 최신 occupancy·cut 정책과 검증 게이트를 반영한다.
- webui의 Rust 서비스/Electron, immutable revision reader lock, scene-pinned
  pick/snap 응답, 회수 승인·복구, 웹 검증 게이트는 유지한다.
- Python 앱 버전 `0.12.235`, index/renderd `0.12.224`. 기존 upstream-only
  renderd와 구분하므로 함께 재빌드해야 한다.

## 연결부 조정

- `thin auto`는 일반 레이아웃도 keep으로 해석한다(2026-09-23 upstream 결정).
  명시적 cull은 유지한다. Python에서 제거된 공개 view `--lod`도 감사표에서
  제거하며, Rust는 이전처럼 명시 오류로 거부한다(index `--lod`와는 별개).
- 웹에서도 margin 기본 off. `--margin on`은 독립 세션, frame-cache off와
  perf-baseline은 prefetch를 차단한다. frame-cache 자체는 기본 on 유지.
- margin 요청을 `bg=on`으로 전달한다. fit 재결정이 필요한 background
  `dropped` 응답은 foreground 오류로 승격하지 않으며 같은 영역을 재시도하지 않는다.
- density stack은 upstream과 같이 `FLOE_RUST_DENSITY_STACK=top` 진단 opt-in이다.
  pass-2 장면은 스트리밍 후 보관되지 않으므로 density 경로의 published query
  scene은 incomplete로 표시한다. 그 화면에서 exact pick/snap을 약속하지 않는다.
  exact detail 또는 density off의 완전한 장면은 기존 질의 계약을 유지한다.
- 프레임 wire에 새 density/fit 계측값과 기존 scene identity를 함께 보낸다.

## 검증

- `cargo check --workspace --all-targets --offline --locked`: 통과.
- `cargo test --workspace --offline --locked` 및 최종 suite의 workspace 검사:
  최종 879 passed / 0 failed / 97 ignored. fixture가 필요한 ignored 테스트는
  아래 통합 게이트에서 별도로 실행한다.
- `sh tools/validate_rust.sh`: 전체 103개 게이트 정의 범위를 완료했다.
  `unit_vfs`/`unit_render`는 전체 workspace unit 실행으로 대체된다.
  초기 실행에서 발견한 구 thin/margin 기대값과 GTK 계측용 테스트 namespace를
  수정한 뒤 실패 지점부터 `--only`로 남은 구간을 재실행했다. timeout 상향이나
  게이트 생략은 없으며, 최종 잔여 구간은 `RUST VALIDATION: ALL OK`로 종료했다.
- jobdeck 83, occupancy 45, 렌더러 47 테스트 통과. 새 area-true/density stack,
  budget-fit, write-once, layer decode 게이트 통과.
- KLayout 오라클: 13 PX + 2 phase-exact + 14 style 검사 통과.
- Python 없는 macOS arm64 runtime smoke, 캐시 revision/회수·복구,
  웹 질의/공유/stream/CLI/DRC 및 합성 패키징 검사 통과.
- 기존 upstream 컴파일 경고와 개발용 Python deprecation 경고는 남아 있다.

실칩·RHEL/ETX·실제 Electron GUI 실측은 이 병합의 자동 검증으로 대체하지 않는다.
