# 네 번째 jobdeck 정방향 통합 — 0.12.184

2026-09-21. **정방향 통합·Rust/웹 이관과 개별 회귀를 완료했다.** 기본 배터리의
97개 실행 대상은 여러 실행에 걸쳐 모두 통과했다. 한 번의 전체 배터리 성공이나
macOS 실행 파일의 최초 기동 지연 해결을 의미하지는 않는다.

## 범위와 보존

- 웹 시작점 `9e063ff`, 실측 반영점 `9234514`, 공통 조상 `c817117`.
  실측 브랜치의 추가 47개 커밋을 `feature/webui`에 정방향 merge한다.
  `main`과 `/Users/journey/Flatide/floe2_review`는 수정하지 않는다.
- 사용자 작업 `docs/FLOE2_INTRO.ko.md`, `docs/FLOE2_BROCHURE.ko.md`는
  이 단계의 수정·staging·커밋에서 제외한다. vendor는 변경하지 않는다.
- 10개 충돌 파일 중 renderer/VFS/index의 7개는 웹 차이를 rustfmt로
  정규화해 대조했다. 기능 차이는 `Cache::plan`의 정상 빈 계획 root 보완이며
  이를 보존한다. 나머지는 최신 실측 코드와 정규화 후 동일하다.
- renderd의 웹 질의 scene identity·취소·게시 후 응답을 보존하면서 새로운
  카운터/OVR 정제 동작을 합친다. Python source-aware occupancy 기본값과
  전체 웹 게이트 목록도 보존한다. 파일 전체 ours/theirs 선택은 하지 않는다.

## 최신 실측 계약

1. OVM v8의 instance-BVH 레이어 마스크를 반영한다. 구 v7은 새 바이너리로
   열리지 않으며 **명시 Index/`--force` 재색인**이 필요하다. 웹 open/info/
   preview가 임의로 캐시를 개명하거나 다시 만들지는 않는다. 활성 reader
   lease와 새·구 캐시 이름의 writer lock은 그대로다.
2. occupancy는 `thin=cull`에서도 기존 파일을 쓸 수 있다. 일반 레이아웃 생성
   기본값은 여전히 off, jobdeck은 on이다. 레이아웃 요약 기본 on 전환은 없다.
   `--occupancy-prune 0|1`은 native 기본 1, 0은 정확한 walk 진단이다.
   1은 sub-cell bbox 근사이며 **바이트 불변 스케줄링 옵션이 아니다**.
3. OVR2 shape/tree 생성은 `--representatives-format 2` opt-in이다. 옵션 생략은
   기존 재사용/기본 points 생성, 명시 1 또는 2는 기존 OVR도 재생성한다.
   폴리곤/path 대표는 전체 원본 도형 복원이 아니며 샘플 외 위치를 복원하지 않는다.
4. 요청 cut의 계획이 decoded budget에 맞지 않으면 최신 planner의 density fit을
   사용한다(큰 크기 class 우선, 경계 class 일부, 더 작은 class 생략).
   이는 **픽셀 불변 메모리 최적화가 아닌 표시 정책의 근사**다. 최종 프레임은
   선택된 정책을 완주한 것이지 모든 원본을 정확히 표시했다는 뜻이 아니다.
   웹도 `approximate`와 앞쪽 `budget fit` 상태 및 세부 class 비율을 표시한다.
   `cut=0` exact 작업의 예산 오류를 자동 근사로 숨기지 않는다.
5. plain layout `thin=keep`의 shape short-side cut, frames-off의 planner 전달,
   write-once tile paint를 반영한다. sub-cut box는 사용자 실측 결정대로
   **기본 off**이며 `FLOE_RUST_SUB_CUT_BOX=on` 진단만 유지한다.
6. layer-ordered/occlusion decode probe는 실칩 이득이 확인되지 않아 **진단 전용**.
   웹의 정상 프레임 경로를 probe로 바꾸거나 probe 명령/경로 권한을 공개하지 않는다.

## Rust/웹 이관

- `IndexOptions`, Rust CLI, 관리형 `IndexArgs`에 `occupancy_prune`,
  `representatives_format`을 추가한다. 타입/범위/상호배타 검증은 subprocess 전.
  대표 형식은 plain-layout 전용이고 기존 jobdeck 거부 및 jobs 1..16 제한을 따른다.
- 웹의 명시 Index 패널에 선택을 제공한다. 새 source 선택 시 기본값으로 되돌리고
  같은 source의 사용자 선택은 보존한다. 기본 selector는 해당 옵션을 보내지 않는다.
  선택 변경만으로는 Index가 실행되지 않는다. 기존 Index-and-open 복구 journal의
  승인 옵션을 자동으로 바꾸거나 쓰기 권한을 추가하지 않는다.
- numeric allowlist에 fit/shape/sub-cut-box/OVR tree/write-once/queue/wall 카운터를
  추가한다. 임의 native diagnostic/path 문자열은 내보내지 않는다. u64는 문자열로
  전송한다. background margin의 시간으로 foreground 계측을 덮지 않는다.
- 화면의 native wall/queue/text는 각각 daemon 수행·명령 대기·라벨 계획이다.
  Python client의 elapsed 기반 wait를 브라우저 왕복 시간으로 그대로 간주하지 않는다.
  이 원시 카운터는 브라우저 decode/blit/network G1 계측의 대체물이 아니다.
- OVR cursor가 남은 중간 프레임은 페이지 수가 0이어도 scene_complete=false이다.
  마지막 scene은 cursor를 소진한 뒤 완성된다. OVR shape는 렌더 전용이며 pick/snap의
  원본 geometry로 승격되지 않는다. scene identity는 각 round와 일치하고 label-only
  crop의 기존 covering scene identity는 유지한다.

## 검증 및 남은 일

- 최초 merged `cargo check --workspace --offline`: exit0. upstream dead-code/
  private-interface 경고가 있어 clippy-clean이라고 주장하지 않는다.
- 전체 ES2017/결정적 UI: exit0 (`/private/tmp/floe-webui-sync4-ui.log`).
  새 옵션의 선택/제출, source 전환, 쓰기 없는 기본 선택, fit/OVR/queue 표시 포함.
- 별도 macOS 호스트의 `cargo test --offline --locked`: exit0, 36 passed
  (`/private/tmp/floe-webui-sync4-desktop-unit.log`). 새 공통 Rust 코드와 호스트의
  빌드·단위 회귀를 확인했으며 실제 WebView 창은 이 명령에서 실행하지 않았다.
  화면 제어 재확인도 `CUA_REPL_ENABLED_SURFACES is required`로 실패했으므로
  이를 GUI 연동 복구나 최종 번들 수용으로 집계하지 않는다.
- 전체 `sh tools/validate_rust.sh`: **exit1**, 로그
  `/private/tmp/floe-webui-sync4-battery.log`. workspace unit/doc test, CLI·캐시
  migration·revision·selfcheck·portable·Python-free macOS runtime·embedded
  lifecycle·app read까지 통과한 뒤 `layerprops` 실행30초 제한에서 종료됐다.
  그 뒤 게이트는 이 실행에서 미실행이며 전체 PASS로 집계하지 않는다.
- 같은 `layerprops-1094761046431e48 --list`는15.95초 뒤 exit0이었다
  (`/private/tmp/floe-webui-sync4-layerprops-list.log`). 코드/제한값을 바꾸지 않은
  단독 재실행은72문서·980스타일·4뷰 대조를 본문0.08초에 통과했다
  (`/private/tmp/floe-webui-sync4-layerprops-retry.log`). 테스트 본문 없는 목록
  조회에서도 지연이 관측됐지만 OS 보안 검사 등 원인은 확정하지 않는다.
  workspace unit은 `layerprops-68c1cf74ef15f162`, 개별 gate는 dependency
  fingerprint가 다른 위 실행 파일이므로 unit의 기동 결과를 그대로 대입할 수 없다.
- 원래 게이트 목록99개에서 통과한15개와 `unit`이 대체하는 두 검사
  `unit_vfs/unit_render`를 제외한82개 선택 재실행도 **exit1**이었다.
  `layerprops`는 통과했고 다음 `layer_defaults` 실행30초 제한에서 종료됐다
  (`/private/tmp/floe-webui-sync4-battery-remaining.log`). 같은 파일의 목록
  조회는16.08초 뒤 exit0, 원래 제한값의 단독 재실행은20개 GTK 대상/바이트와
  native 게시를 본문0.22초에 통과했다. 로그는
  `floe-webui-sync4-layer-defaults-{list,retry}.log`다.
- 위 두 실패/재통과를 별도로 남기고, 나머지80개는 원래 스크립트의
  `--only GATE`를 개별 실행했다. **78개 exit0, 2개 exit1, 수집 실행 exit1**이다.
  한 실패 때문에 뒤의 독립 검사가 미실행으로 남지 않도록 하되 모든 실패를
  보존했다. 코드/timeout/skip은 바꾸지 않았다. 로그 디렉터리:
  `/private/tmp/floe-webui-sync4-gates.JMqxMq`.
  두 실패는 `layer_palette`, `web_startup`의 테스트 실행 파일30초 기동 제한이다.
  전자는 같은 제한의 단독 재실행에서12096 batch·32 catalog·7776 click 대조를
  통과했다(`floe-webui-sync4-layer-palette-retry.log`).
- `web_startup`은 첫 단독 재시도에서도 같은30초 제한으로 실패했다
  (`floe-webui-sync4-web-startup-retry.log`). 해당 `floe_app-0c31f2ed508f757e`의
  테스트 본문 없는 `--list`도 약20초 뒤 종료했다
  (`floe-webui-sync4-startup-list.log`). 지연 중 프로세스는 존재했지만 stack
  sample 시도 전 종료되어 stack은 얻지 못했다. 이후 동일 코드/제한 재실행은
  144 startup·380 stream 정책 대조,22 native launch·21 first generation·6 preflight
  오류를 모두 통과했다(`floe-webui-sync4-web-startup-after-list.log`).
  이것은 최초 기동 지연의 원인 규명 또는 수정 증거가 아니다.
- 따라서99개 gate 이름 중 `unit`이 대체하는 `unit_vfs/unit_render`를 제외한
  **97개 모두 현재 소스의 통과 근거가 있다**(첫 실행15 + 두 단독 재통과 +
  개별78 + 두 단독 재통과). 선택 성공을 정규 전체 한 번의 성공으로 기록하지
  않는다. KLayout13 PX·2 half-phase exact·14 style, 실제 query/worker/stream,
  jobdeck23 PNG/report 쌍·모드/시그널, DRC 저장·복원, 전체 UI/CLI 검사를 포함한다.
  Rust 이관 파일의 focused rustfmt 검사와 staged diff 공백 검사도 통과했다.
- 실제 CLI 게이트에 Python/Rust OVR1↔OVR2 전환, 명시 format 재생성, prune 0/1
  출력 바이트 대조와 base cache 불변을 추가했다. 전체 게이트 재실행 exit0
  (`/private/tmp/floe-webui-sync4-app-cli-retry.log`), profile·잠금·시그널 정리도 통과.
- CLI inventory 재실행 exit0: 10개 명령 + fe-embed, 공개 옵션116개, 숨김 거부17개,
  native parser probe184개 (`/private/tmp/floe-webui-sync4-cli-inventory-retry.log`).
- OVR 게이트에 실제 Rust worker-client의 2-round scene completion·비질의 대표
  확인을 추가했다. 신규 테스트의 종료 API 오기는 발견 즉시 수정했으며 재컴파일/
  실행을 통과했다. 실제 테스트 본문0.03초, OVR 전체 재실행 exit0
  (`/private/tmp/floe-webui-sync4-representatives-retry.log`). 4096개의 합성 선을
  8개 proxy로 표시, 두 군집의 빈 공간·zoom refinement·길이/회전·binned/walk
  대조를 포함한다. fixture가 필요한 ignored 테스트는 unit 목록만으로 통과로 세지 않는다.
- 최초 실행의 시간 초과는 별도 보존: CLI 도움말5초(inventory), 가짜 인덱서
  `--version` 조회(app-cli), 새 `representative_frames` 실행60초(OVR).
  같은 도움말과 테스트 `--list`는 이후 정상 종료했다. 제한값/제품 코드는 바꾸지
  않았으며 이 재실행 통과를 cold-start 지연의 원인 규명이나 해결로 주장하지 않는다.
  최초 로그는 각각 `floe-webui-sync4-cli-inventory.log`,
  `floe-webui-sync4-app-cli.log`, `floe-webui-sync4-representatives.log`에 있다.
- 테스트 도중 범위 밖 crate의 순수 cargo-fmt 변경을 원래 형식으로 복원했다.
  최종 고정 소스의 회귀 판정과 최초 컴파일 확인을 혼동하지 않는다.

이 통합은 macOS CUA 재연결/최종 앱 GUI 수용, RHEL 8.6/8.10 + ETX/X11,
Python-free 현장 Linux 실행·G1/G4 전체 수용, 서명·공증의 완료가 아니다.
원격 공유·GitHub Actions Linux 실행·열린 캐시 hot reload는 기존 보류를 유지한다.
