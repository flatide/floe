# Native 오라클 기동 지연 진단

2026-09-22. [전체 계획](WEBUI_PLAN.ko.md),
[Electron 번들 검증 기록](WEBUI_ELECTRON_PORTABLE.ko.md).

이 단계는 **테스트 진단 보강**이다. 제품의 렌더·속성·기동 timeout, 보안 설정,
캐시 정책은 바꾸지 않았다. 간헐적 기동 지연이 해결됐다는 선언이 아니다.

## 확인한 경계

`9447ac4` 단계의 전체 `validate_rust.sh`는 workspace·portable·runtime smoke·
app read를 통과한 뒤 `layerprops` native oracle 실행의30초 timeout으로 끝났다.
이 오류만으로 속성 parser/model의 결함이라고 판단하거나, 반대로 OS 원인이라고
확정할 수 없었다.

1. Rust test 함수 첫 줄과 codec/styles/각 view/종료에 고정 stderr 표시를 추가했다.
   동일30초 제한에서 재실행하자 `last_phase=not_observed completion=false`로
   실패했다. 해당 실행 파일 안에 새 표시 문자열이 포함됐음도 확인했다.
2. **같은 실행 파일의 `--list`**만 별도로 실행했다. oracle·모델 본문은 실행하지
   않았다. 실행+sampling 관찰 wall19.731초 후 test 이름을 출력하고 exit0이었다.
   1초 sampling의796개 표본 모두 `_dyld_start +0`, footprint96KiB였다.
   sampling은 타이밍에 영향을 줄 수 있고 전체19.731초의 구간별 분해는 아니다.
3. 이 binary ID와 일치하는 `syspolicyd`의 `GK evaluateScanResult` 로그도 있었다.
   수치 결과 코드는 해석하지 않았으며, 이 평가가 전체 지연의 원인임을 입증하지
   않는다. 다른 앱의 로그를 수집하거나 OS 설정을 바꾸지 않았다.
4. 이후 같은30초 제한의 별도 실행은 **72개 문서·980개 스타일·4개 view model**을
   통과했다(libtest 본문 시간0.10초). 이어 선택 게이트도 본문0.12초에 통과했다.
   이 시간들은 전체 프로세스 기동/입력→표시 시간이나 GUI 성능 지표가 아니다.

현재 근거는 **테스트 본문 없이도 실행 전후 대기가 존재하고, 실제 속성 오라클은
시작된 후 통과할 수 있다**는 것이다. 앞선30초 실패를 삭제하거나 전체 배터리의
성공으로 치환하지 않는다. OS의 구체적인 원인, 다른 native 실행 파일의 모든 지연,
RHEL/ETX에서의 동작은 미확정이다.

## 단계 표시 계약

| 마지막 표시 | 그 뒤의 검사 범위 |
|---|---|
| `not_observed` | 인식할 표시 없음. 이것만으로 프로세스 미시작을 단정하지 않음 |
| `entered` | oracle 파일 읽기/JSON 및 구조 확인 |
| `codec` | 72개 문서의 parse/format/width parity |
| `styles` | 980개 스타일 변환 parity |
| `view0`~`view3` | 해당 native model 초기 가시성 및16회 속성 변경 parity |
| `done` | 함수 마지막. 프로세스 종료 성공을 뜻하지는 않음 |

Python은 stderr의 **마지막1MiB**에서 허용된 고정 줄만 읽는다. 임의 경로·토큰·
assertion 본문은 진단 줄에 넣지 않는다. `not_observed`는 marker가 수집 범위 밖인
경우도 포함하므로 loader 판정에는 별도 근거가 필요하다.

timeout은 한 상수30초로 유지하고 원래 `TimeoutExpired`를 다시 raise한다.
`done`을 봐도 timeout이면 `completion=false`다. 자동 재시도/워밍업/시간 연장/
oracle 생략은 추가하지 않았다. 성공에는 기존 exit0·정확한 성공 메시지·입력/캐시
digest 불변뿐 아니라 마지막 `done` 표시도 요구한다.

검사는 실제 Python codec와 GTK 메서드를 사용하되 checkbox I/O는 mock한다.
native GTK 창/물리 입력/픽셀 렌더/G4 전체의 수용 검사는 아니다.

## 재현과 검증

```sh
.venv/bin/python -B tools/validate_layerprops_startup.py
sh tools/validate_rust.sh --only layerprops
```

순수 진단 검사4개: 허용 표지만 보고, 비정상 stderr/임의 문자열을 노출하지 않으며,
1MiB 경계를 지키고, `done`도 timeout 성공으로 바꾸지 않음, 원래 예외 재전파와
단일30초 설정을 확인한다. 선택 게이트에 함께 배선했다.

- `floe-layerprops-phases.log`: 표시 추가 후30초 실패, 마지막 표시 미관찰.
- `floe-layerprops-loader-probe.log`: 별도 `--list`/sampling 진단.
- `floe-layerprops-loader-np20peut/startup.sample`: 해당 Rust 실행 파일의 스택.
- `floe-layerprops-startup-os.log`: 동일 binary ID의 macOS 평가 로그1건.
- `floe-layerprops-phases-followup.log`: 별도 실제 oracle 통과.
- `floe-layerprops-phases-selected.log`: 순수4개 + 기존 oracle 통과,
  `RUST VALIDATION: ALL OK (...; gates: layerprops)`.

위 artifact는 모두 `/private/tmp/` 아래에 있다. source는 합성 valmini의 새 복사본만
사용했다. renderer/indexer/서비스 제품 코드는 변경하지 않았고, 이 QA-only 수정 후
전체 배터리를 다시 통과했다고 주장하지 않는다. 이전 전체 exit1 기록은 유지한다.

## 잔여

2026-09-22 후속 전체 실행(`cb1f2c9`, 소스 고정)은 layerprops의 순수4개와 실제
72문서/980스타일 오라클을 통과했으나, 다음 `layer_defaults-c910bc4e9579c5bb`의
`--ignored --nocapture` 실행에서30초 timeout으로 종료됐다.
로그 `/private/tmp/floe-webui-after-startup-full.log`. 해당 실행에는 본문 진입
표시/stack 관측이 없어 구체 원인은 미확정이며 앞선 layerprops loader 관측을
그대로 적용하지 않는다. 임시 `.venv` 링크는 종료 시 제거됐다.

다음 전체 실행에서 시작 대기와 본문 단계 실패를 분리해 기록한다. 모든 테스트
실행 파일을 무조건 미리 실행하거나 보안 설정을 완화하는 해법은 채택하지 않는다.
G1/G4 실제 UI·장애 수용, RHEL8.6/8.10+ETX, 정식 호스트 채택·서명/배포는 별도다.

## Cargo 빌드 경로 분리 진단 — 2026-09-22

`af40d64` 뒤의 전체 실행은 위 layerprops/기본값을 통과한 뒤 `web_startup`의
`cargo test --offline --locked -p floe-app --lib --no-run --message-format=json`
명령에서180.009초 timeout이었다. 당시 로그에는 Cargo의 부분 출력이 없어 빌드 락,
컴파일/링크, 프로세스 기동 중 어느 구간인지 구분할 수 없었다. 아래 별도 실행이
그 **과거 실패의 원인을 확정하거나 전체 gate를 성공으로 바꾸지는 않는다**.

동일 명령·180초 한도로 한 번 진단 실행했다.10초 시점에 이 실행이 소유한 Cargo와
두 rustc만 각각1초 sampling했다. Cargo는 compiler job의 출력을 기다렸으며, 두
rustc는 이미 `main`에 진입해 `SearchPath::new → ReadDir → __getdirentries64`에
머물렀다(543/550, 761/769표본). 이전 test executable의 `_dyld_start` 관측과는
**다른 구간**이다. 명령은38.575초에 exit0, artifact107개 중 fresh102/built5,
build-script19개, compiler-message3개, build-finished1개를 반환했다.

읽기 전용 목록 집계:

| 경로 | 항목 수 | `.o` 수 | 목록 집계 wall |
|---|---:|---:|---:|
| `rust/target/debug/deps` | 945,506 | 943,080 | 8.820초 |
| `electron/service/target/debug/deps` | 11,109 | 10,603 | 0.083초 |

동일한 의존성 없는 작은 Rust 입력을 stdin으로 넣고 `--crate-type=lib
--emit=metadata`의 `-L dependency=...`만 바꿨다. 빈 폴더→기존 큰 deps→빈 폴더
순서로 각30초 한도, 실행 시간은 **0.021 / 8.651 / 0.021초**, 세 출력2059바이트가
완전히 같았다. 새 임시 metadata만 만들었고 기존 산출물은 변경하지 않았다.
이 대조는 해당 큰 디렉터리의 검색 비용을 확인한다. 전체 빌드 시간 분해, 렌더
성능, 과거180초 전부의 설명이나 Electron helper3건 timeout의 원인 증명은 아니다.

Cargo의 macOS debug 기본값은 `split-debuginfo=unpacked`이고, 이 방식은 debug
정보를 위해 object 파일을 남긴다([Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html#split-debuginfo),
[Rust 1.51 설명](https://blog.rust-lang.org/2021/03/25/Rust-1.51.0/)). 따라서 `.o`라는
이유만으로 쓰레기로 판정하거나 일괄 삭제하면 안 된다. 현재 profile/환경에서
별도 save-temps 설정은 발견하지 않았지만,94만개의 생성 이력을 모두 추적한 것은
아니다. 이번에는 `cargo clean`, target 개명/삭제, debug 설정 변경을 하지 않았다.
후속 비교는 기존 debug 산출물을 보존한 별도 빌드 디렉터리에서 같은 설정으로
수행할 수 있다. 전체 검증기의 고정 worker 경로와도 맞춰야 하며, 단순히
`CARGO_TARGET_DIR`만 바꿔 전체 배터리 수용을 선언하지 않는다.

### 실패 출력의 유실 방지

`validate_web_startup.py`의 oracle-build만 `WEB STARTUP CARGO` 요약을 추가한다.
성공/실패/timeout의 captured stdout/stderr 각각 마지막1MiB에서 고정 Cargo JSON
이벤트 수, fresh/built 수, 마지막 build-finished boolean, 정확히 일치한 build/cache
lock 문구의 횟수와 truncation 여부만 출력한다. 경로·인자·환경·compiler diagnostic
본문·알 수 없는 필드는 출력하지 않는다. `finished=true`가 있어도 원래 timeout은
다시 raise한다. 이벤트 미관찰은 미시작/락 없음의 증명이 아니다.

추가 subprocess, 산출물 자동 목록 조사, 자동 sampling, 재시도·워밍업·deadline
변경은 gate에 넣지 않았다. 순수 timing 검사는 기존6개 결과에 더해 부분 bytes/str,
boolean 엄격 검사,1MiB/다국어/잘린 JSON/깊은 JSON, 비밀 문자열 비노출,
원래 명령·예외 보존을 검사한다.

진단 artifact(모두 합성/빌드 경로):

- `/private/tmp/floe-cargo-stage-d3ldadi4/`: Cargo 출력과 소유 process3개의 sample.
- `/private/tmp/floe-search-path-probe-7asd3cbx/`: 동일 metadata3개와 빈 검색 폴더.
- `/private/tmp/floe-cargo-progress-selected.log`: 변경 후 선택 `web_startup` gate.

순수 timing/진행 정보 검사는 통과했다. 실제 `sh tools/validate_rust.sh --only
web_startup`는 **exit1**: oracle-build9.530초/exit0(fresh106/built1,
build-finished=true, truncation=false) 뒤 `gtk-startup`30.005초 timeout이었다.
그 프로세스의 이번 stack/본문 진입 표시는 수집하지 않아 원인은 미확정이다.
새 진단이 빌드 성공과 다음 실행 실패를 구분하는 것까지 확인했으며, 뒤의 stream/
native 검사와 전체 배터리는 성공으로 세지 않는다. 실행용 `.venv` 링크는 제거했다.

이 단계는 제품/Rust 버전을 바꾸지 않으며 클립보드도 다시 읽거나 덮어쓰지 않는다.
