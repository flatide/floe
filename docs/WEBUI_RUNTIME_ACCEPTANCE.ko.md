# Rust 단독 런타임 검증 — G4 실행 준비

2026-09-18, M4g-54. [전체 계획](WEBUI_PLAN.ko.md#6-성능-요구와-게이트)의
Python-free 실행 조건을 위한 **자급식 검증 경로**다. Mac 실행이나 Linux ELF 교차
빌드를 Linux 실행으로 세지 않으며, 작은 smoke를 전체 기능 parity로 세지 않는다.

## 실행 경계

`rust/app-core/tests/runtime_smoke.rs`를 컴파일한 test executable과 제품의
`floe2-web`, `floe-index`, `floe-renderd`를 사용한다. 실행 시 Python/KLayout/GTK,
Cargo, Node, shell, 외부 fixture 생성기를 호출하지 않는다. 검증기가 Rust OASIS
writer로 새 합성 입력을 만들며 사용자 레이아웃/캐시/리뷰를 입력받지 않는다.
세 제품 실행 파일만 공백이 포함된 새 디렉터리에 복사하므로 checkout 상대 경로나
개발용 worker 발견에 의존하지 않는다. 원래 실행 파일은 변경하지 않는다.

- CLI 자식은 `env_clear`, 빈 PATH, 새 worker TMPDIR와 제한된 작업 수로 실행한다.
  원래 환경의 Python 경로·native override·브라우저 설정·인증 정보를 전달하지 않는다.
- 임시 루트는 새로 생성한0700 디렉터리만 사용하고 끝나면 정리한다. 루트가 이미
  있으면 실패한다. 임의 기존 폴더를 재사용/삭제하지 않는다.
- CLI60초, worker render15초/query10초, 리뷰 게시10초 제한을 둔다. CLI timeout은
  자신이 시작한 자식에 TERM 후10초 종료 유예를 주고 마지막에 강제 종료한다.
- renderer/indexer jobs1, renderer budget64MiB의 작은 합성 검사다. 실칩 부하·
  공유 서버 동시 사용자 성능을 대표하지 않는다.

## 실제로 단언하는 것

| 경로 | 근거 |
|---|---|
| CLI index/occupancy/info | 두 레이어·반복 rect의 OASIS 생성, `.ice/design.ovo` 생성, DBU bbox 고정값 대조 |
| CLI layout/deck render | PNG envelope/크기와 complete report; 별도 native raw 프레임의 비어 있지 않은 RGB 확인 |
| layout probe/pick/snap | 실제 daemon 기동/프레임, 해당 scene의 pick layer/bbox와 snap 꼭짓점 좌표 대조 |
| CLI exact clip | 요청 bbox로 clip→재색인한 결과 bbox 대조 및 다시 PNG 렌더 |
| CLI DRC build/read | 합성 ASCII의 native pack 생성, 규칙 이름/오류 수 조회 |
| Rust 리뷰 저장/내보내기 | 명시 합성 reviewer의 note·waive prepare/publish, directory sync, store를 닫고 디스크에서 재열기, export bytes와 파일 비교 |
| 보존/정리 | 원본 OASIS와 DRC/pack bytes 불변, 원본 캐시 bytes/mtime 불변, worker 임시파일0, 리뷰 자원 회계 원복 |

리뷰 쓰기는 production `ManagedStore` 경로이며 **웹의 승인 버튼·receipt 복구 검증은
아니다**. query도 native scene 경로이지 브라우저 좌표/표시 ACK 경계 검증은 아니다.
jobdeck은 query capability가 없음을 유지한다. 이 검사는 모든 도형·계층·색상·글꼴,
OVR/LOD, 정확한 전체 픽셀 또는 clip Region XOR의 오라클을 대체하지 않는다.

## 개발 호스트에서 실행

저장소의 release 제품3개를 먼저 빌드한다. 다음 Python 명령은 **컴파일/실행 준비용
개발 하네스**이며 검증기/제품 프로세스 안에는 들어가지 않는다.

```sh
cd rust
cargo build --release --offline --locked -j2 -p floe-app -p floe-index -p floe-renderd
cd ..
.venv/bin/python -B tools/validate_runtime_smoke.py
# Linux라고 주장할 검사에서는 호스트가 다르면 build 전에 exit2로 거부한다.
.venv/bin/python -B tools/validate_runtime_smoke.py --require-linux
# 정규 배터리의 이름 및 web alias에도 배선됨
sh tools/validate_rust.sh --only runtime_smoke
```

개발 하네스는 Cargo JSON에서 정확한 test executable을 선택하고, 빈 환경/PATH로
새 임시 작업 디렉터리에서 실행한다. 종료 코드뿐 아니라 테스트1개 실제 실행·
ALL OK marker·fixture 완전 정리를 확인한다. 전체 `validate_rust.sh`는 이 gate를
생략하지 않는다. 별도 `cargo test`의 기본 실행에서는 실제 바이너리가 필요하므로
ignored다. ignored 수를 통과 수로 계산하지 않는다.

## Python 없는 Linux 환경에서 실행

빌드 호스트에서 아래처럼 Linux 검증기를 만들고 Cargo가 출력한 정확한 executable을
제품3개와 함께 준비한다. 이 단계는 Linux 실행이 아니라 교차 빌드일 수 있다.

```sh
cd rust
cargo test --release --offline --locked -j2 --target x86_64-unknown-linux-musl \
  -p floe-app-core --test runtime_smoke --no-run
```

대상 Linux에서는 검증기 자체를 실행한다. 아래 경로는 옮겨 놓은 파일의 예다.
실행 전 환경 설정을 담당하는 `env`는 호스트 도구이며 검증기가 호출하는 것이 아니다.
최소 컨테이너라면 같은 값을 container environment/entrypoint로 지정할 수 있다.

```sh
env -i PATH= TMPDIR=/tmp FLOE_RUNTIME_BIN_DIR=/opt/floe \
  /opt/tests/runtime_smoke --ignored --exact native_runtime_without_python --nocapture
```

대상 `/tmp`는 writable이어야 하며 worker TMPDIR에는 공백/제어 문자가 없어야 한다.
GNU 바이너리는 필요한 시스템 라이브러리가 별도 필요하다. musl static-pie 형식은
링크 의존성 근거이지 모든 커널/CPU 호환 증명이 아니다. 전체 portable notice/hash
검사나 브라우저/ETX 수용도 별도로 유지한다.

Linux 판정에는 실제 Linux 실행 결과와 Python/GTK/KLayout이 없는 대상 환경의
구성 근거를 함께 남겨야 한다. Python이 설치된 Mac의 빈 PATH 성공만으로는 부족하다.
GitHub Actions 합성 Linux 실행은 사용자 승인 대기이며 workflow 추가/원격 실행을
아직 하지 않았다. 제품의 원격 공유 SH-10 보류와는 별개의 검증 실행 결정이다.

## 현재 근거

Mac arm64에서 위 Rust 검증기와 개발 하네스가 통과했고 `--require-linux`는 exit2로
거부했다. Linux x86-64 musl 검증기와 제품3개는 offline/locked 교차 빌드 및
static-pie ELF 형식 확인까지다.
최종 배터리와 제품 교차 빌드 결과는 [M4 실행 기록](WEBUI_M4.ko.md)에 기록한다.
Linux 실제 실행, G1/G4 전체, 최신 브라우저 수용, 현장 Firefox/ETX는 미완료다.

최종 소스 재실행 중 `floe-index --version timed out` 단발 실패1회가 있었다.
원본 실행 파일과 별도 새 복사본의 직접 `--version`은 각각0.417/0.179초로 성공했고,
제한값 변경 없는 동일 smoke 재실행은2.14초, 정규 배터리 안 실행은1.92초로 통과했다.
이 결과는 재배치 경로만으로 재현되지는 않았다는 뜻이며, 원인을 macOS/병렬 부하로
확정하거나 해결됐다고 세지 않는다. [G4 추적](WEBUI_G4_AUDIT.ko.md#5-재배치-런타임의-단발-version-시간-초과)에 남긴다.
