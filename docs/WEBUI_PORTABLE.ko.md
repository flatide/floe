# Rust 웹 portable (M4f-2, opt-in preview)

기존 Python/GTK `tools/make_portable.sh`와 **별도**인
`tools/make_web_portable.sh`를 사용한다. 기본 GTK 실행기나 설치를 교체하지 않는다.
실행 파일은 `floe2-web`, `floe-index`, `floe-renderd` 세 개이며 UI/font는 Rust에 내장된다.
Python/GTK/KLayout/Node/브라우저는 패키지에 넣지 않는다. 개발용 packager도 Rust이며
기존 vendor의 libc/signal-hook/serde_json만 사용한다.

## 만들기

Rust 1.89 이상 툴체인과 대상 std를 **사전에 설치/반입**해야 한다. 이 스크립트는
rustup target add·다운로드·pip/npm을 실행하지 않는다. rustup 사용 시 `rustup which`로
이미 설치된 cargo/rustc를 선택하며 `RUSTUP_TOOLCHAIN=1.89.0`처럼 선택할 수 있다.
standalone 툴체인은 PATH의 cargo/rustc를 쓴다. 원본 toolchain copyright/licenses
자료(`share/doc/rust`)도 있어야 한다. 내부 빌드는 항상 `--offline --locked`다.

```sh
# Linux x86_64: 성능 우선 GNU. 실제 요구 GLIBC가 2.28을 넘으면 게시하지 않는다.
sh tools/make_web_portable.sh --target x86_64-unknown-linux-gnu \
  --jobs 4 --glibc-max 2.28 --out /scratch/floe2-web-gnu.tar.gz

# macOS/Linux: 이미 설치된 musl target으로 이식용 빌드.
sh tools/make_web_portable.sh --target x86_64-unknown-linux-musl \
  --jobs 4 --out /scratch/floe2-web-musl.tar.gz
```

`--target`, `--out`은 필수이며 출력의 부모 디렉터리는 먼저 만들어야 한다.
기존 파일/디렉터리/깨진 symlink까지 모두 거부한다. `--force`는 없다.
jobs는 기본4, 허용1..16이다. `CARGO_TARGET_DIR`로 재사용 빌드 디렉터리를 지정할 수
있고 기본은 `rust/target/web-portable`이다. bootstrap 도구는
`rust/target/web-packager`에 만든다. Cargo 캐시는 삭제하지 않는다.
ZIP 반입은 `FLOE_SRC_REV=<승인한 revision>`을 선택적으로 명시한다. Git worktree는
그 worktree의 revision을 사용하고 ZIP은 무관한 부모 Git의 revision을 상속하지 않는다.
revision은 서명이 아니며 dirty `+`와 archive SHA-256을 함께 확인한다.

GNU 빌드는 Linux x86_64 호스트에서만 허용하며 시스템 linker가 필요하다.
배포 호스트보다 새로운 GLIBC가 필요한 경우 그 배포판에서 다시 빌드하거나 실제 배포
기준에 맞는 `--glibc-max`를 명시하거나 musl을 사용한다. 상한을 올렸다는 사실이 구형
서버에서 실행된다는 뜻은 아니다. 커스텀 compiler/linker/CPU 설정과 OS 라이브러리
조합의 호환성은 별도 검증 대상이다.

## 검사와 게시

새 private stage는 출력 파일과 같은 부모에 만들고 성공/일반 오류/취소 시 정리한다.
packager 실행 중 SIGINT/SIGTERM은 직접 소유한 빌드 프로세스 그룹을 종료하고 leader를
수거한다. 메타데이터 명령은 출력 스트림당4MiB·EOF 포함10초 제한이다. 고지 합128MiB,
각 입력 파일128MiB 상한이다. 강제 SIGKILL/시스템 장애 시 private stage가 남을 수 있으며
다음 실행이 기존 stage나 사용자의 디렉터리를 임의 삭제하지 않는다.

1. Cargo의 target-filtered native/build 의존성 closure에서 원본 고지와 manifest를
   수집한다. dev/unselected-target만의 패키지는 제외한다. build dependency는 포함하므로
   모두 런타임에 링크됐다는 목록은 아니다. font·Cargo.lock·workspace manifest와
   선택 툴체인의 copyright/library copyright/licenses를 보존한다.
   `--extra-notices DIR`로 추가 고지를 명시할 수 있다. 목록은 법적 배포 승인이나
   Floe 사용권 부여가 아니며 `LicenseRef-Flatide-Proprietary`를 재해석하지 않는다.
2. 같은 소스/target/revision으로 세 바이너리를 빌드하고 복사한 파일을 검사한다.
   ELF64 little-endian x86-64만 허용한다. GNU는 loader 경로·DT_NEEDED 목록과 실제
   DT_VERNEED/DT_VERNEEDNUM의 GLIBC 버전을 읽으며 임의 문자열이나 section header
   검색으로 대체하지 않는다. RPATH/RUNPATH·filter/audit 의존성은 거부한다.
   musl은 interpreter·DT_NEEDED·symbol-version 요구가 모두 없어야 한다.
   구조 근거: [ELF dynamic linking](https://gabi.xinuos.com/elf/08-dynamic.html),
   [LSB symbol version requirements](https://refspecs.linuxfoundation.org/LSB_3.2.0/LSB-Core-generic/LSB-Core-generic/symversion.html).
3. Linux x86_64 조립은 실제 `selfcheck --adjacent`가 필수다. 버전/worker 시작·종료
   실패를 무시하는 옵션은 없다. macOS 교차 조립은 실행하지 않고 `runtime_checked=false`를
   적는다. 모든 경우 desktop 수용은 `unverified`다.
4. `SHA256SUMS`에 모든 배포 regular file을 넣고 검증한 뒤 tar.gz를 만든다.
   기본 root 이름은 `floe2-web-portable`이다. tar의 `TAR_OPTIONS`는 적용하지 않는다.
   같은 파일시스템의 hard-link로 최종 파일을 원자적·비덮어쓰기 게시한다. 경쟁자가
   같은 이름을 먼저 만들면 실패한다. 게시 뒤 늦은 취소·출력 파이프 종료는 이미 게시된
   파일을 되돌리지 않는다. 부모 sync 실패는 게시 완료 후 경고로 구분한다.

## 사용·현장 확인

새 디렉터리에 풀고 다음을 실행한다. `verify.sh`는 sha256sum 또는 shasum을 요구한다.

```sh
cd /path/to/new-extraction/floe2-web-portable
sh verify.sh
./floe2-web selfcheck --adjacent
./floe2-web selfcheck --metadata-only
./floe2-web view /path/to/design.oas --no-open
./floe2-web view /path/to/deck.jb --level 1,3
```

설치 경로의 공백/한글은 허용하지만 native wire의 TMPDIR은 공백/제어 문자를 허용하지
않는다. normal 명령은 기존 명시 FLOE_INDEX_BIN/FLOE_RENDERD_BIN을 유지하므로 이전
설치의 override를 점검한다. `--adjacent`만 의도적으로 이 override를 무시한다.
Firefox는 설치된 것을 사용하며 진단/패키징은 브라우저나 설계/리스너를 열지 않는다.

`BUILD.txt`, `ELF.txt`, `NOTICES/INVENTORY.txt`, `SHA256SUMS`를 함께 전달한다.
archive SHA-256은 게시 결과에 출력된다. 체크섬은 손상 검출용이지 인증 서명이 아니므로
배포 파일의 hash는 신뢰하는 전달 경로로 확인한다. tar 바이트 재현성을 보장하는
reproducible-build 기능은 아니다. GNU 시스템 라이브러리·Linux kernel/CPU·Firefox/
ETX/NFS 수용과 남은 UI parity, About 고지 UI 및 GTK 은퇴는 별도다.

## 로컬 검증

Rust unit은 옵션/ELF 구조·문자열 오탐·손상 입력·비덮어쓰기/정리 규칙을 검사한다.
`validate_web_portable.py`는 synthetic tool double로 notice/build/ELF 거부,
SIGTERM143·직접 자식 수거·stage 정리를 검사한다. 실제 archive를 인자로 주면 전체
목록/경로·세 ELF·hash·공백/한글 재배치·손상 사본 거부까지 확인한다. Linux x86_64에서는
재배치한 실제 앱의 selfcheck도 실행한다. 합성 도구 테스트를 Linux 실행 수용으로
보고하지 않는다. 이 gate는 전체 `validate_rust.sh`에도 연결되어 있다.
