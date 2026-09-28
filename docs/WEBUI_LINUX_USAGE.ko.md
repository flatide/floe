# Linux 서버 + 브라우저 사용 안내

2026-09-28, `feature/webui`의 `floe2-web` 기준. 이 문서는 **Linux에서 색인·렌더링하고
사용자 PC의 Chrome/Firefox로 보는 방법**을 설명한다. 기존 Python/GTK `floe2`나
Electron 독립 앱의 실행 안내와는 다르다.

현재는 사용자별 세션을 실행하는 loopback 서버다. 서버 IP로 직접 접속하는 공용
웹 서비스, HTTPS 프록시·SSO·다중 사용자 포털은 아직 지원 범위가 아니다.
원격 사용은 사내에서 허용한 **SSH 터널**을 통해 같은 loopback 주소를 유지한다.
아래 절차는 코드·CLI 옵션과 대조했으며, 실제 RHEL 8.6/8.10 서버·사내 SSH 정책·
사용자 PC 브라우저를 잇는 현장 실행 검증은 별도로 필요하다.

## 1. 실행 구조와 필요한 것

```text
사용자 PC: Chrome / Firefox
  http://127.0.0.1:58080
          │ SSH 로컬 포트 포워딩 (암호화)
Linux 서버: 127.0.0.1:58080 → floe2-web → floe-index / floe-renderd
                                             서버의 OASIS·jobdeck·캐시
```

- 서버: 같은 소스 revision으로 빌드한 `floe2-web`, `floe-index`, `floe-renderd`.
  런타임에는 Python/GTK/KLayout/Node/Electron이 필요하지 않다.
- 사용자 PC: Chrome 또는 Firefox, 서버로 접속할 SSH 클라이언트와 계정.
  조직 정책에서 SSH local forwarding을 허용해야 한다. SSH host key를 정상 확인하며
  인증서·브라우저 보안·SSH 검증을 끄지 않는다.
- 설계 파일과 인덱스는 **서버에** 있어야 한다. PC 브라우저가 원본 OASIS를 읽는
  방식이 아니다. 브라우저에는 화면 픽셀과 UI에 필요한 정보가 전달된다.
- `--no-open` 서버에는 DISPLAY, ETX, X11, XQuartz, 서버 Firefox가 필요 없다.
  ETX/X11 안에서 서버 Firefox를 실행하는 별도 방식은 §8을 참고한다.

## 2. 서버에 실행 파일 준비

### A. Linux 서버에서 소스 빌드

`feature/webui` 소스와 완전한 vendor, Rust 1.89 이상 툴체인·시스템 linker를 먼저
준비한다. 폐쇄망에서는 사전에 반입해야 하며 아래 명령은 의존성을 다운로드하지 않는다.

```sh
cd /path/to/floe/rust
cargo build --release --offline --locked -j 4 \
  -p floe-app -p floe-index -p floe-renderd
cd target/release
./floe2-web --version
./floe2-web selfcheck --adjacent
```

Cargo는 반드시 `rust/`에서 실행한다. 그래야 `.cargo/config.toml`의 vendor 설정이
적용된다. 실행 파일은 위 예의 `/path/to/floe/rust/target/release/`에 생긴다.
세 바이너리를 함께 갱신한다. `floe2-web --version`의 앞쪽 preview 패키지 버전만
비교하지 말고 revision과 뒤쪽 index/renderd 호환 버전도 확인한다.

### B. 별도로 만든 Linux 웹 portable 사용

신뢰하는 경로로 받은 `floe2-web-portable`을 **새 디렉터리**에 풀고 검사한다.

```sh
cd /path/to/floe2-web-portable
sh verify.sh
./floe2-web selfcheck --adjacent
```

웹 portable은 `tools/make_web_portable.sh` 산출물이다. Python/GTK용
`make_portable.sh`, Electron용 패키지와 구분한다. RHEL 8용 GNU 패키지는 GLIBC
요구 상한을 2.28로 검사하는 빌드 경로가 있으며, macOS 실행 파일은 Linux에 복사해
사용할 수 없다. GNU/musl 선택·패키징 명령은 [웹 portable 안내](WEBUI_PORTABLE.ko.md).
상한 검사나 교차 빌드 성공만으로 현장 실행 수용이 완료되지는 않는다.

이후 예제는 세 바이너리가 있는 디렉터리에서 실행한다. 이전 설치를 가리키는
`FLOE_INDEX_BIN`, `FLOE_RENDERD_BIN` override가 있으면 의도한 것인지 확인한다.
`selfcheck --adjacent`는 override를 무시하지만 일반 실행은 무시하지 않는다.
임시 저장은 쓰기 가능한 로컬 경로를 쓰고, **TMPDIR에는 공백·제어 문자를 넣지 않는다**.

## 3. 최초 인덱싱 — 브라우저를 열기 전에

이미 current 인덱스가 있으면 재사용한다. 단순 열기/조회는 자동으로 인덱싱하지
않으며, 캐시가 없으면 CLI 또는 웹의 명시적인 Index 작업이 필요하다.
공유 서버에서 아래 `--jobs 8`은 시작 예시이지 성능 보장값이 아니다.

```sh
# 일반 OASIS
./floe2-web index /data/design/chip.oas --jobs 8
./floe2-web info /data/design/chip.oas

# jobdeck 전체: 참조된 지원 가능 소스를 순차 색인
./floe2-web index /data/mask/job.jb --jobs 8

# 또는 특정 레벨만 색인
./floe2-web index /data/mask/job.jb --level 1,3 --jobs 8
./floe2-web info /data/mask/job.jb --level 1,3 --json
```

- 캐시는 일반적으로 소스 옆의 숨김 `.<소스파일명>.ice/`에 생긴다. 소스 부모의
  쓰기 권한·디스크 공간을 확인한다. 원본 OASIS 자체를 수정하는 명령은 아니다.
- CLI occupancy는 jobdeck에서 기본 on, 일반 layout에서 기본 off다.
  일반 파일에도 원하면 `index ... --occupancy`로 요청한다.
- LOD는 기본 off다. 필요한 경우 **색인 명령에** `--lod`를 추가한다.
  이미 current인 캐시의 LOD 생성 옵션은 이 옵션만으로 바뀌지 않는다.
  `view --lod`는 지원하지 않는다.
- `--force`는 기존 캐시 교체 승인이다. 단순 시작 오류를 해결하려고 습관적으로
  붙이지 않는다. 다른 프로세스에서 같은 캐시를 보는 뷰어를 먼저 종료해야 한다.
  일반 CLI 잠금은 다른 모든 뷰어·구형 도구까지 보호하는 서버 전체 잠금이 아니다.
- 덱에서는 한 번에 소스 하나를 처리하며 그 파일 내부에서 `--jobs`를 쓴다.
  missing/unsupported source의 skip 로그도 확인한다. 종료 코드 0만 보고 덱의
  모든 칩이 포함됐다고 판단하지 않는다.

열린 뷰를 유지하며 별도 불변 revision을 만들고 전환하는 기능은 다른 경로다.
[인덱스 revision 안내](WEBUI_INDEX_REVISIONS.ko.md)를 따르며, 일반 CLI `--force`와
같은 것으로 취급하지 않는다. 긴 인덱싱을 마친 뒤 다음의 120초 인증 절차를 시작한다.

## 4. 사용자 PC에서 접속 — SSH 터널

예제 값은 사용자 `alice`, 서버 `layout-server`, 포트 `58080`이다.
자신의 계정·호스트·양쪽에서 비어 있는 포트로 바꾼다. 포트는 **PC와 서버에 같은
번호**를 사용한다. 서버가 HTTP Host/Origin을 정확히 비교하기 때문이다.

### 4-1. PC 터미널 A: 터널을 먼저 열기

```sh
ssh -N -T \
  -o ExitOnForwardFailure=yes \
  -o ServerAliveInterval=30 -o ServerAliveCountMax=3 \
  -L 127.0.0.1:58080:127.0.0.1:58080 \
  alice@layout-server
```

연결 뒤 출력 없이 대기하는 것이 정상이다. 이 터미널을 유지한다.
`-g`나 `0.0.0.0` 바인딩으로 PC의 터널 포트를 다른 PC에 공개하지 않는다.
이 단계는 터널 준비일 뿐 서버 앱 기동·인증 성공을 뜻하지 않는다.

### 4-2. PC 터미널 B: 서버에 로그인하고 앱 실행

```sh
ssh alice@layout-server
```

아래부터는 **서버 셸**에서 실행한다. 설치 경로와 설계 경로를 바꾼다.

```sh
cd /path/to/floe/rust/target/release
umask 077
floe_session_dir=$(mktemp -d /tmp/floe-web-session.XXXXXX)
printf 'Session directory: %s\n' "$floe_session_dir"

TMPDIR=/tmp ./floe2-web view /data/design/chip.oas \
  --multi --no-open --port 58080 \
  --session-file "$floe_session_dir/session.json" \
  --jobs 4 --raster-jobs 2 --budget-mb 1024 --refinement off
```

portable 설치면 `cd`만 portable 디렉터리로 바꾼다. 서버 앱은 foreground로 실행해
둔다. 로그에 다음 두 줄이 나오면 접속 준비가 된 것이다.

```text
[floe2-web] local workspace: http://127.0.0.1:58080
[floe2-web] private session link: /tmp/floe-web-session.XXXXXX/session.json (one use, expires in 120s)
```

`--session-file`은 **아직 존재하지 않는 파일**이어야 하며 0600으로 생성된다.
매 실행 `mktemp -d`로 새 폴더를 만들면 이전 파일과 충돌하지 않는다.
`--multi`는 다른 세션에 열기 요청이 전달되는 것을 피하려고 명시했다.

### 4-3. PC 터미널 C: 서버의 비공개 링크 확인

별도 SSH 셸로 같은 서버에 접속해서, 로그에 나온 **실제 경로**를 읽는다.
`XXXXXX`는 예시이므로 출력된 폴더명으로 바꿔야 한다.

```sh
ssh alice@layout-server
cat /tmp/floe-web-session.XXXXXX/session.json
```

JSON의 `url` 값만 복사한다. 형태는 다음과 같다.

```text
http://127.0.0.1:58080/#bootstrap=<일회용 인증값>
```

이 URL은 세션의 **소유자 권한을 가진 비밀번호에 해당**한다. 개인 터미널에서만
확인하고 채팅·이슈·공용 로그·스크린샷에 남기거나 다른 사람에게 보내지 않는다.
명령행 인자로 브라우저에 전달하면 프로세스 목록에 노출될 수 있으므로 아래처럼
주소창에 직접 입력한다. 인증값을 포함한 명령을 셸 history에 저장하지 않는다.

### 4-4. PC 브라우저: URL 전체를 주소창에 붙여넣기

Chrome 또는 Firefox의 주소창에 위 `url` 전체를 붙여넣는다. 최초 링크는 **발급 후
120초 이내, 한 번만** 사용할 수 있다. 인증 뒤 fragment가 주소에서 사라지는 것은
정상이다. 레이아웃이 열리고 연결 상태와 첫 프레임을 확인한다.

중요한 구분:

- `http://layout-server:58080`으로 직접 접속하지 않는다. 서버는 외부 IP에서 듣지 않는다.
- `localhost`로 바꾸지 않고 **`127.0.0.1`을 그대로** 쓴다.
- PC `58081` → 서버 `58080`처럼 서로 다른 포트로 매핑하지 않는다.
  충돌 시 예제의 **모든 58080을 같은 새 번호**로 바꾼다.
- 최초 접속에 포트 주소만 입력하면 인증되지 않는다. `#bootstrap=...`까지 필요하다.
- 터널을 통해 원격에서 사용하는 것이지, 공개 HTTP 서버나 원격 공유 기능을 켠 것은 아니다.

## 5. 자주 쓰는 시작 옵션

아래는 §4-2의 `view` 명령에 적용할 예시다. `--no-open --port --session-file` 등
접속 옵션은 그대로 사용하며 재시작마다 새 세션 파일·링크를 만든다.

| 용도 | SOURCE와 표시 옵션 |
|---|---|
| 특정 위치 | `/data/design/chip.oas --goto 13600,8600,700 --depth full --detail high` |
| jobdeck 선택 레벨 | `/data/mask/job.jb --level 1,3 --mode level` |
| jobdeck 칩 뷰 | `/data/mask/job.jb --level 1,3 --mode chip` |
| DRC 읽기 | `/data/design/chip.oas --drc /data/drc/results.db.ice` |
| 빈 창에서 파일 선택 | SOURCE 없이 `--root /data/design --root /data/mask` |

`--goto` 좌표와 폭은 µm다. 일반 파일의 초기 depth 기본은 0이고 goto/DRC/jobdeck은
기본 full이다. detail 기본은 medium이며 high도 exact와 같지 않다. `--detail exact`는
비용이 커질 수 있으므로 광역 실칩에 무조건 적용하지 않는다. 현재 thin auto는 일반
layout/jobdeck 모두 keep이다. `--thin cull`은 성능만 바꾸는 옵션이 아니라 가는 페이지를
생략할 수 있는 표시 정책이다.

잡덱은 `--level`을 생략하면 정책에 따라 레벨 선택을 묻는다.
`FLOE_JOBDECK_LEVELS=all|ask|N,N...`가 기본 선택에 영향을 주며 CLI `--level`이 우선한다.
로드 후 `Jobdeck mode` 또는 `Ctrl+,`로 level/chip을 전환한다. `Levels to load`에서
`Apply levels · keep view`로 카메라를 유지한 레벨 재선택을 할 수 있다.

### 브라우저 조작

캔버스를 클릭해 키보드 포커스를 준 뒤 사용한다. 텍스트 입력칸에서는 해당 단축키를
캔버스 조작으로 해석하지 않는다.

| 동작 | 조작 |
|---|---|
| 전체 보기 / 좌표 이동 | Fit 또는 Ctrl+A / Goto 입력 후 Go |
| 이동 | 왼쪽·가운데 드래그, 방향키 50%, Shift+방향키 10% |
| 확대·축소 | + / −, 마우스 휠; 오른쪽 드래그는 오른쪽 방향 확대·왼쪽 방향 축소 |
| depth·detail | 왼쪽 컨트롤; `<` / `>` depth 변경, `9` 두 번은 full |
| 레이어 | 체크박스 표시/숨김, `⋯` 스타일 |
| 프레임 / 자 | F / R, K 최근 자 취소, Shift+K 전체 자 제거 |
| 화면 저장 | Save view PNG (브라우저 다운로드) |

## 6. 파일 선택·저장과 DRC 권한

`Browse server files`는 PC의 파일 열기 대화상자가 아니다. 시작 소스 부모와
명시 `--root` 안의 **서버 파일**만 보여준다. 둘 다 없으면 시작 폴더를 쓴다.
root는 세션 시작 때 고정되며 `/` 전체를 열지 않는다. 숨김 파일·캐시·symlink는
선택기에서 제외한다. 추가 경로가 필요하면 `--root`를 넣어 새 세션을 실행한다.
`PATTERN01.TE` 같은 마스크 파일은 All files 필터를 사용한다. 현재 OASIS와 jobdeck을
대상으로 하며 GDS/gzip은 이 선택기에서 지원하지 않는다.

- 서버 원본과 캐시: 서버에 유지된다. PC 파일을 layout으로 자동 업로드하지 않는다.
- 화면 PNG·설정·명시 export 다운로드: 브라우저가 실행되는 PC 쪽에 저장된다.
  다운로드 준비 성공과 PC 파일 저장 완료는 구분한다. 클립보드가 차단되면 PNG 저장을 쓴다.
- 메모·waive·공유 기본값 게시: 해당 기능을 별도로 허용하고 승인한 경우에만
  서버 파일에 저장한다. 단순 보기나 파일 선택이 저장 승인은 아니다.

DRC 리뷰 쓰기가 필요한 경우에만 런처에 다음을 추가한다.

```text
--drc /data/drc/results.db.ice --drc-reviewer alice --drc-edit-waives
```

`--drc-reviewer`는 고정 reviewer의 메모 저장을 허용하고 `--drc-edit-waives`는 waive
저장도 허용한다. 메모/waive의 자동 저장은 각 패널에서 **별도 opt-in**이며 기본 off다.
다른 DRC로 교체한 뒤에는 런처가 허용한 권한을 명시 재연결해야 한다. reviewer 이름은
로그인·SSO·OS 사용자 인증을 대신하지 않는다. 결과가 불명확한 저장은 상태/receipt를
확인하고 복구 안내를 따른다. 새 승인으로 같은 쓰기를 반복하지 않는다.

xattr 없는 NFS를 위한 저장 fallback은 구현되어 있지만 실제 NFS의 권한·잠금·장애·
다중 client 수용은 별도다. `.floe-meta-*` 보조 파일은 지워도 되는 캐시가 아니다.
[NFS 저장 계약](WEBUI_NO_XATTR.ko.md)을 확인하고 본 파일과 보조 정보를 임의 삭제하지 않는다.

## 7. 공유 서버의 CPU·메모리·전송 설정

초기 예제는 decode 4, raster 2, page budget 1024 MiB다. 기본값을 그대로 쓰면
환경/CPU 수에 따라 decode 최대 8, raster 최대 4를 사용한다.

- `view --jobs`: 페이지 decode. `view --raster-jobs`: 픽셀 raster.
  `index --jobs`: 별도 색인 작업의 worker 수다. 서로 같은 옵션으로 오해하지 않는다.
- `--budget-mb`는 decoded page 예산이지 프로세스 전체 RSS의 강제 상한이 아니다.
  프레임·인덱스·임시 작업·DRC 등 추가 메모리가 필요하다.
- 관리형 세션의 CPU admission은 16 slots이며 파일 목록에 1 slot, DRC에 추가
  1 slot 등을 예약한다. `--jobs 16 --raster-jobs 16`을 허용하는 의미가 아니다.
  jobs 숫자와 OS에서 보이는 전체 스레드 수가 정확히 같지도 않다.
- 이 자원 관리는 **프로세스별**이다. 사용자 10명의 앱을 띄워도 서버 전체가 16
  threads/slots 안에 묶이지 않는다. 동시 세션 수·색인 시간을 운영자가 관리해야 한다.
- 기본 raw 전송이 회선에서 병목이면 `--png`로 A/B 측정한다. 전송량 감소와
  압축/해제 비용의 교환이므로 무조건 더 빠르다고 보장하지 않는다.
- refinement는 예제에서 명시 off다. margin prefetch는 기본 off, frame cache는 기본 on.
  `--perf-baseline`은 라벨·프레임·refinement·프레임 재사용을 끄는 비교용이며
  decoded cache나 geometry cut까지 제거하는 것은 아니다.

서로 다른 서버 세션을 한 PC 브라우저에서 쓸 때도 포트를 다르게 할당한다.
각 세션 내부의 PC/서버 포트 번호는 같아야 한다. 기본 workspace 재사용은 같은
UID+DISPLAY 단위지만 위처럼 `--multi`/명시 프로세스 옵션을 쓰면 독립 세션이다.

`--local-sharing`의 Follow/Explore는 별도 opt-in 읽기 전용 로컬 공유다.
소유자의 bootstrap 링크를 동료에게 전달하는 기능이 아니다. 현재 원격 공유 배포는
보류 상태이므로 이 사용 절차에서 동료 초대·reverse proxy·공개 listen을 구성하지 않는다.
공유 Unix 계정만으로 실제 사용자별 권한이 분리된다고 가정해서도 안 된다.

## 8. 종료·재접속과 ETX/X11 대안

- 정상 종료: 브라우저 **End session**에서 확인하거나 서버 앱 터미널에서 Ctrl+C.
  worker 정리와 앱 종료를 확인한 뒤 터널 터미널 A도 Ctrl+C로 종료한다.
  터널만 끊는 것은 앱의 정상 종료 절차가 아니다.
- `--no-open`에서는 브라우저 탭을 닫았다고 서버 프로세스가 즉시 종료된다고
  가정하지 않는다. 연결이 끊긴 활성 뷰는 약 60초 뒤 닫힐 수 있다.
- 일시 단절: 서버·터널을 확인한 뒤 기존 인증 탭을 사용한다. 새로고침은
  cookie와 그 탭의 sessionStorage가 유지된 경우에 가능하다. 장시간 단절 뒤
  뷰 재열기가 필요할 수 있으며 미저장 초안은 새로고침으로 유실될 수 있다.
- 최초 인증 링크 만료/이미 사용됨, 탭 저장소 소실, `Session expired`:
  기존 bootstrap은 재사용하지 않는다. 기존 서버를 종료하고 §4의 새 세션 파일·
  링크로 다시 시작한다. 인증 세션의 최대 수명은 교환 시점부터 8시간이다.
- 서버 재시작 후에는 이전 탭 URL이나 인증 정보로 접속할 수 있다고 가정하지 않는다.
  새 링크로 다시 인증한다. SSH 셸을 닫아도 계속되는 상주 서비스로 설정된 상태가
  아니며, systemd/공용 daemon 운영 절차는 이 문서의 범위 밖이다.

ETX/X11에서 **서버에 설치된 Firefox**를 쓰고 싶으면, 그 GUI 세션의 서버 터미널에서
`--no-open` 없이 실행한다. 이 경우 PC 브라우저/SSH 터널 절차와 혼합하지 않는다.

```sh
./floe2-web view /data/design/chip.oas --multi \
  --jobs 4 --raster-jobs 2 --firefox /usr/bin/firefox
```

기존 Firefox 프로필 대신 임시 전용 프로필을 사용한다. 서버에 Firefox·필요 GUI
라이브러리·DISPLAY가 있어야 한다. 해당 브라우저의 종료도 앱 수명에 영향을 준다.
ETX/X11 경로의 입력·화면·성능은 현장 검증 대상이다. Electron 독립 앱이 필요하면
[Electron 안내](WEBUI_ELECTRON.ko.md), [배포 안내](WEBUI_ELECTRON_PORTABLE.ko.md)를 본다.

## 9. 문제 해결

| 증상 | 먼저 확인할 것 |
|---|---|
| 서버 IP:포트로 접속 불가 | 정상적인 현재 제한이다. `127.0.0.1` + SSH 터널을 사용한다. 외부 listen 옵션은 없다. |
| PC에서 connection refused | 터널 A와 서버 B가 살아 있는지, 실제 포트가 맞는지 확인한다. |
| SSH `administratively prohibited` | 서버의 forwarding 정책을 관리자에게 확인한다. 앱·브라우저 보안을 끄지 않는다. |
| SSH local bind / 앱 address in use | 충돌 없는 새 번호로 양쪽 포트와 URL을 함께 바꾼다. 다른 사용자의 프로세스를 종료하지 않는다. |
| UI만 뜨거나 private session link 사용 안내 | 최초 `url`의 fragment 누락, 120초 만료, 이미 사용된 링크 여부를 확인한다. |
| 인증/Host/Origin 거부 | `localhost`, 서버 호스트명, 다른 local port로 바꾸지 않았는지 확인한다. HTTP 프록시가 loopback 요청을 우회시키는지도 조직 설정에서 확인한다. |
| Firefox not found | PC 브라우저 사용은 `--no-open`; ETX 방식은 실제 Firefox 경로를 지정한다. |
| version mismatch / renderer failed | 세 바이너리 동시 갱신, override, `selfcheck --adjacent`, 서버 터미널의 오류를 확인한다. |
| cache missing / stale | 해당 소스를 명시 인덱싱한다. 교체 시 다른 뷰어 종료·`--force` 필요 여부를 먼저 판단한다. |
| 빈 화면인데 연결은 됨 | depth·가시 레이어·덱 로드 레벨·누락 소스·partial 상태를 확인한다. `--depth full`은 비용도 늘릴 수 있다. |
| budget exceeded / resources busy | 요청 레벨·영역·동시 작업을 줄이고 자원 사용을 확인한다. 무조건 budget만 올리지 않는다. |
| session file already exists | 새 private 폴더와 새 파일명을 사용한다. 기존 파일을 덮어쓰지 않는다. |

연결 진단 예시(인증값을 사용하거나 출력하지 않는다):

```sh
# 서버: listener 존재 확인
ss -ltn 'sport = :58080'

# PC: 터널을 통과해 공개 UI shell까지 도달하는지 확인
curl --noproxy '*' -sS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:58080/
```

HTTP 200은 정적 UI 도달만 뜻하며 인증·설계 열기·렌더 성공의 증거가 아니다.
지원 요청에는 앱/native 버전·revision, 비밀값 없는 오류·상태를 남긴다.
session JSON, bootstrap, cookie, CSRF, 실제 설계 경로/좌표·리뷰 본문은 그대로 보내지 않는다.

## 10. 검증 범위와 관련 문서

이번 문서는 CLI `view/index/render --help`, 세션 파일 생성·인증 수명,
loopback Host/Origin 검사, 자원·파일 선택·종료 구현을 근거로 작성했다.
코드상 가능한 SSH 터널 접속 절차이며 **실제 Linux→PC end-to-end 검증 완료 기록은 아니다**.
처음 배포할 때 작은 승인된 샘플로 첫 화면·pan/zoom·다운로드·재접속·정상 종료를
확인하고, DRC 저장은 승인된 테스트 리뷰로 별도 검사한다.

- [웹 portable 제작·검증](WEBUI_PORTABLE.ko.md)
- [서버 파일 선택 범위](WEBUI_FILE_PICKER.ko.md)
- [열린 인덱스 revision·회수](WEBUI_INDEX_REVISIONS.ko.md)
- [Python 없는 런타임 검사](WEBUI_RUNTIME_ACCEPTANCE.ko.md)
- [실측·구현·보류 구분](WEBUI_REMAINING.ko.md)
- [전체 CLI 예제](../rust/app/README.md)

서버 Firefox/ETX 방식의 환경 목록은 `sh tools/audit_webui_env.sh`로 확인할 수 있다.
이는 읽기 전용 버전 조사이며 GUI·SSH·네트워크·성능 테스트는 아니다.
PC 브라우저 방식에서는 서버의 `firefox=not_found`, `display=unset` 자체가 실패가 아니다.
