# floe Rust runtime 빌드 안내 (리눅스 서버)

`sh build-linux.sh`는 각 Rust runtime binary를 GNU와 musl 두 형식으로 만든다:

- `dist/floe-index-linux-gnu` — **glibc 동적 빌드, 권장.** 병렬
  인덱싱이 musl 대비 ~40% 빠르다 (MAIN09 실측 60s vs 97s — glibc의
  AVX memcpy·per-thread malloc arena 덕). 타깃 호스트의 glibc가
  빌드 머신과 같거나 새 버전이어야 한다 — 빌드/운영이 같은 배포판
  계열이면 보통 충족. 링크에 시스템 `cc`(gcc)가 필요하다.
- `dist/floe-index-linux-x86_64` — **완전 정적 musl ELF, 이식성
  폴백.** glibc 버전과 무관하게 어떤 x86_64 리눅스에서든 실행된다.
  rustc 내장 rust-lld가 링크하므로 gcc 등 시스템 패키지가 전혀
  필요 없다.

동일 suffix로 `floe-renderd`(GUI Rust backend), `floe-render-cli`(headless oracle), `path-inventory`도 함께 생성된다.
`floe-renderd`도 0.12.13부터 같은 형식의 `--version`/시작 스탬프를
찍고, 앱의 Help > About이 floe-index/renderd 스탬프를 그대로
보여준다 — 호스트에서 "이 바이너리가 어느 빌드냐"는 질문은 About
화면만으로 답할 수 있다.

의존 크레이트는 순수 Rust에 전부 `vendor/`로 동봉되어 **빌드 중
crates.io 접속이 없다**. 인터넷이 필요한 것은 Rust 툴체인
설치뿐이다. 빌드 머신에 gcc가 없으면 gnu 단계에서 실패하는데,
musl만 필요하면 `cargo build --release --target
x86_64-unknown-linux-musl`로 그 단계만 직접 실행하면 된다.

## A. 인터넷 되는 서버 (권장 경로)

```sh
# 1) rustup - 사용자 로컬 설치, sudo 불필요 (~700MB)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"
# 프록시 환경이면 curl 전에: export https_proxy=http://<proxy>:<port>

# 2) 빌드
cd floe/rust
sh build-linux.sh
#   -> dist/floe-index-linux-gnu     (권장, + .sha256)
#   -> dist/floe-index-linux-x86_64  (정적 폴백, + .sha256)
#   -> 같은 suffix의 floe-renderd/floe-render-cli/path-inventory
```

## B. 완전 오프라인 서버 (소스 zip 반입 빌드)

소스는 GitHub zip으로 반입한다 (브라우저 Download ZIP이면 충분).
**패키지 버전이 rust/ 를 건드리는 푸시마다 올라가므로**, zip에
`.git`이 없어도 `--version`/시작 스탬프의 버전 번호만으로 어떤
반입분인지 식별된다 — 스탬프는 `[floe-index 0.2.0 (gnu)]` 형태
(git 해시는 알 수 있을 때만 덧붙는다). 커밋 해시까지 박고 싶으면
빌드 때 `FLOE_SRC_REV=<sha>`를 넘긴다 (선택). 참고: 구버전
build.rs는 압축 푼 위치를 감싼 무관한 저장소의 HEAD를 찍는 사고가
있었다 — 어느 이력에도 없는 해시가 나오면 이 경우다.

zip 안의 `dist/`에는 빌드된 바이너리도 들어 있다 — `--version`
버전을 확인하고, 소스 버전과 다르면 아래처럼 다시 빌드한다.

인터넷 되는 아무 곳에서 standalone 툴체인 두 개를 받아 반입한다
(버전은 개발기와 맞춘 1.97.1 기준, 다른 stable도 무방):

```
https://static.rust-lang.org/dist/rust-1.97.1-x86_64-unknown-linux-gnu.tar.xz
https://static.rust-lang.org/dist/rust-std-1.97.1-x86_64-unknown-linux-musl.tar.xz
```

서버에서:

```sh
tar xf rust-1.97.1-x86_64-unknown-linux-gnu.tar.xz
(cd rust-1.97.1-x86_64-unknown-linux-gnu \
 && ./install.sh --prefix="$HOME/rusttc" --disable-ldconfig)
tar xf rust-std-1.97.1-x86_64-unknown-linux-musl.tar.xz
(cd rust-std-1.97.1-x86_64-unknown-linux-musl \
 && ./install.sh --prefix="$HOME/rusttc" --disable-ldconfig)
export PATH="$HOME/rusttc/bin:$PATH"

cd floe-main/rust        # zip 최상위 디렉토리 이름 기준
sh build-linux.sh        # vendor/ 동봉, 네트워크 불필요
./dist/floe-index-linux-gnu --version    # 버전 번호로 반입분 확인
```

gnu 툴체인 tarball에 gnu용 std가 들어 있어 모든 산출물을 오프라인
으로 빌드된다 (gnu 링크에 필요한 gcc만 서버에 있어야 한다).

## 확인

```sh
file dist/floe-index-linux-gnu         # ELF 64-bit ... dynamically linked
file dist/floe-index-linux-x86_64      # ELF 64-bit ... static-pie
sha256sum -c dist/floe-index-linux-gnu.sha256
sha256sum -c dist/floe-index-linux-x86_64.sha256
./dist/floe-index-linux-gnu scan some.oas 16      # 스모크
```

`floe-renderd-linux-*`는 Python runtime 옆에 `floe-renderd` 이름으로 배치하거나 `FLOE_RENDERD_BIN`으로 지정한다.

반입한 바이너리가 어떤 빌드인지는 실행 시 첫 줄 스탬프로 확인한다:
`[floe-index <버전> [<git>] (gnu)]` / `(musl-static)`. gnu 바이너리가
타깃에서 `GLIBC_x.xx not found`로 죽으면 glibc가 빌드 머신보다
오래된 것 — 정적 폴백을 쓴다.
