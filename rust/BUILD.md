# floe-index 빌드 안내 (리눅스 서버)

`sh build-linux.sh` 한 번에 산출물 두 개가 나온다:

- `dist/floe-index-linux-gnu` — **glibc 동적 빌드, 권장.** 병렬
  인덱싱이 musl 대비 ~40% 빠르다 (MAIN09 실측 60s vs 97s — glibc의
  AVX memcpy·per-thread malloc arena 덕). 타깃 호스트의 glibc가
  빌드 머신과 같거나 새 버전이어야 한다 — 빌드/운영이 같은 배포판
  계열이면 보통 충족. 링크에 시스템 `cc`(gcc)가 필요하다.
- `dist/floe-index-linux-x86_64` — **완전 정적 musl ELF, 이식성
  폴백.** glibc 버전과 무관하게 어떤 x86_64 리눅스에서든 실행된다.
  rustc 내장 rust-lld가 링크하므로 gcc 등 시스템 패키지가 전혀
  필요 없다.

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

gnu 툴체인 tarball에 gnu용 std가 들어 있어 두 산출물 모두 오프라인
으로 빌드된다 (gnu 링크에 필요한 gcc만 서버에 있어야 한다).

## 확인

```sh
file dist/floe-index-linux-gnu         # ELF 64-bit ... dynamically linked
file dist/floe-index-linux-x86_64      # ELF 64-bit ... static-pie
sha256sum -c dist/floe-index-linux-gnu.sha256
sha256sum -c dist/floe-index-linux-x86_64.sha256
./dist/floe-index-linux-gnu scan some.oas 16      # 스모크
```

반입한 바이너리가 어떤 빌드인지는 실행 시 첫 줄 스탬프로 확인한다:
`[floe-index 0.1.0 <git> (gnu)]` / `(musl-static)`. gnu 바이너리가
타깃에서 `GLIBC_x.xx not found`로 죽으면 glibc가 빌드 머신보다
오래된 것 — 정적 폴백을 쓴다.
