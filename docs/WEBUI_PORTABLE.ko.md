# Rust 웹 portable (M4f-2, opt-in preview)

기존 Python/GTK `tools/make_portable.sh`와 **별도**인
`tools/make_web_portable.sh`를 사용한다. 기본 GTK 실행기나 설치를 교체하지 않는다.
실행 파일은 `floe2-web`, `floe-index`, `floe-renderd` 세 개이며 UI/font는 Rust에 내장된다.
Python/GTK/KLayout/Node/브라우저는 패키지에 넣지 않는다. 개발용 packager도 Rust이며
기존 vendor 의존성만 사용한다. 고지 목록의 content ID에는 기존 sha1을 재사용한다.

## 만들기

vendor는 Git으로 전달된 원본 파일 전체가 필요하다. `data/`·`AGENTS.md`를
모든 하위 폴더에서 무시하던 규칙 때문에 `sha1` 테스트 벡터 등 5개 원본이
누락된 문제를 수정했다. `failed to open .../tests/data/sha1.blb` 오류가 나면
수정 커밋을 pull한 뒤 같은 빌드를 다시 실행한다. 체크섬 삭제나 빈 파일 생성으로
우회하지 않는다. 개발 gate `tools/validate_vendor.py`는 Rust/desktop vendor의
체크섬과 Git 포함 여부를 함께 검사하며, Git 없는 소스 archive에서는 체크섬을
검사한다. `sh tools/validate_rust.sh --only vendor`로도 실행할 수 있다.

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
native 0.12.101부터 index/renderd는 실제 worktree/common Git 경로를 감시하며,
없는 `.git/HEAD` 때문에 매 빌드마다 재생성하던 문제를 수정한다.
[검증 기록](WEBUI_BUILD_REVISION.ko.md). 이 수정은 Linux 실행·첫 시작 지연의 수용을 대신하지 않는다.

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
   원본을 바꾸지 않고 `NOTICE-INDEX.json`에 경로·크기·UTF-8/hex·유계 chunk digest를
   기록한다. 그 content ID를 같은 앱 빌드에 고정하고 `BUILD.txt`에도 남긴다.
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

`BUILD.txt`, `ELF.txt`, `NOTICE-INDEX.json`, `NOTICES/INVENTORY.txt`, `SHA256SUMS`를 함께 전달한다.
archive SHA-256은 게시 결과에 출력된다. 체크섬은 손상 검출용이지 인증 서명이 아니므로
배포 파일의 hash는 신뢰하는 전달 경로로 확인한다. tar 바이트 재현성을 보장하는
reproducible-build 기능은 아니다. GNU 시스템 라이브러리·Linux kernel/CPU·Firefox/
ETX/NFS 수용과 남은 UI parity 및 GTK 은퇴는 별도다.

## 로컬 검증

웹 상단 **About**은 launcher의 build identity와 expected native compatibility를
보여준다. 실제 native 도구 검사에는 계속 `selfcheck --adjacent`를 사용한다.
M4f-3b부터 새 portable의 원본 고지를 About에서 읽을 수 있다. 목록은64개씩,
본문은 UTF-8 경계를 보존한 최대64KiB씩 바꿔 표시한다. HTML은 텍스트로만 표시하고
비UTF-8 원본은 hex로 표시한다. 다음/이전·페이지 이동·명시 재시도를 지원한다.
개발 실행 파일이나 이전 portable처럼 compiled index가 없는 경우에는 내장 글꼴
고지만 가능하다고 명시한다. 파일을 옆에 복사하기만 해서는 활성화되지 않으며
새 packager로 **재빌드**해야 한다.

앱은 인접 `NOTICE-INDEX.json`의 compiled ID/source/target을 시작 시 검사하고 그 목록을
고정한다. 목록≤2MiB·4096파일·원본 합128MiB이며 요청은 숫자 ID/page만 받는다.
원본 chunk마다 digest/크기/읽기 전후 변경을 확인하고 경로 성분의 symlink를 거부한다.
검증 실패 시 해당 본문을 표시하지 않는다. 목록 검증 실패는 viewer를 죽이지 않고
고지 기능만 unavailable로 표시한다. 설치 복구 후에는 앱을 재시작한다.

이 SHA-1은 content/change ID이지 게시자 인증이 아니다. `verify.sh`의 전체 SHA-256
검사·신뢰하는 배포 경로 확인을 대체하지 않는다. `selfcheck --metadata-only`는 여전히
파일을 읽지 않으며 compiled ID만 출력한다. 일반 selfcheck는 목록만 검사하고,
원본 전체 chunk를 읽거나 배포 전체 hash를 검사했다고 보고하지 않는다.
About을 열어도 selfcheck·인덱싱·렌더·게시가 시작되지 않는다.

Rust unit은 옵션/ELF 구조·문자열 오탐·손상 입력·비덮어쓰기/정리 규칙을 검사한다.
`validate_web_portable.py`는 synthetic tool double로 notice/build/ELF 거부,
SIGTERM143·직접 자식 수거·stage 정리를 검사한다. 실제 archive를 인자로 주면 전체
목록/경로·세 ELF·hash·compiled notice ID·원본 모든 chunk·공백/한글 재배치·손상 사본
거부까지 확인한다. Linux x86_64에서는
재배치한 실제 앱의 selfcheck도 실행한다. 합성 도구 테스트를 Linux 실행 수용으로
보고하지 않는다. 이 gate는 전체 `validate_rust.sh`에도 연결되어 있다.
실제 compiled catalogue를 가진 macOS 앱의 HTTP 왕복은 별도 개발용
`validate_web_notices.py`로 검사했다([M4 §39](WEBUI_M4.ko.md)).

### 2026-09-17 최신 교차 빌드 재확인

DRC 재연결 수정 `9df2037`의 Rust 코드를 설치된 Rust1.97.1·musl target으로
`--offline --locked`, jobs2 빌드했다. 검증용 venv 링크 등 때문에 source stamp는
`9df20375cde759c791c1a6a630824d69a5afe055+`다. 릴리스용 clean 산출물로 표시하지 않는다.

세 ELF 모두 interpreter/DT_NEEDED/symbol-version 요구가 없고, 실제 archive의
전체 파일 SHA-256·compiled notice ID·고지 모든 chunk·공백/한글 재배치·손상 사본
거부는 통과했다. archive는7,679,824bytes이며 임시 검증물
`/private/tmp/floe-web-current-musl.Lsw5QR/floe2-web-musl.tar.gz`에만 남겼다.
SHA-256은 `977acfc31923721b3967ad5c660296c89cc39522a9625caa680fa220b544ca4f`다.

**전체 portable gate PASS는 아니다.** 합성 고지 누락 테스트는 가짜 `rustc -Vv`가
10초 metadata/EOF 제한을 넘어서, 의도한 `missing notice` 오류에 도달하지 못했다.
단독 재현과 같은 제한의 재실행도 동일했다. assertion에 실제 stdout/stderr를 추가했으며
기한·판정 조건·제품 정책은 바꾸지 않았다. 실제 archive 검사는 기존
`inspect_archive` 함수를 독립 실행한 결과로 구분한다. 로그는 같은 임시 폴더의
`build.log`, `archive-only.log`, `refusal-diagnostic.log`, `validation-with-diagnostics.log`다.

macOS 교차 빌드이므로 `runtime_checked=false`, `desktop_acceptance=unverified`다.
Docker/Podman/Lima/QEMU 실행 도구는 현재 PATH에 없었고 설치·외부 서버 접근은 하지
않았다. 이 결과는 Python-free Linux **실행**이나 최소 Rust1.89 재검증을 대체하지 않는다.
