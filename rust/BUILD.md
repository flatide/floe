# floe-index 빌드 안내 (리눅스 서버)

산출물은 `dist/floe-index-linux-x86_64` — **완전 정적 musl ELF**라
glibc 버전과 무관하게 어떤 x86_64 리눅스에서든 실행된다. gcc 등
시스템 패키지는 필요 없다: musl 타깃을 rustc 내장 rust-lld가
링크하고, 의존 크레이트는 순수 Rust에 전부 `vendor/`로 동봉되어
있어 **빌드 중 crates.io 접속이 없다**. 인터넷이 필요한 것은
Rust 툴체인 설치뿐이다.

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
#   -> dist/floe-index-linux-x86_64 (+ .sha256)
```

## B. 완전 오프라인 서버

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

cd floe/rust
sh build-linux.sh    # vendor/ 동봉이라 네트워크 불필요
```

## 확인

```sh
file dist/floe-index-linux-x86_64      # ELF 64-bit ... static-pie
sha256sum -c dist/floe-index-linux-x86_64.sha256
./dist/floe-index-linux-x86_64 scan some.oas 16   # 스모크
```
