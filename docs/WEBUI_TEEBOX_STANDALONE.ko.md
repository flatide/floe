# TeeBox에서 Electron standalone 실행

2026-09-29, `feature/webui`, Electron shell 0.1.4.
**공용 서버 도입은 보류하고, TeeBox가 ETX 사용자의 DISPLAY로 로컬 앱을 실행한다.**
Electron·Rust 모두 Linux 서버에서 돌며 ETX/X11이 창을 사용자에게 전달한다.
외부 Chrome/Firefox, HTTPS 프록시, TeeBox 로그인 위임, 공개 서비스는 필요하지 않다.
앱 내부의 private loopback Rust backend는 유지하며 외부에 공개하지 않는다.

기존 standalone의 layout/jobdeck 열기, 명시적 인덱싱, DRC, 저장/복구, 창 재사용을
그대로 쓴다. 서버 개발 S0/S1 코드는 보존하지만 운영 연결 작업은 재개하지 않는다.

## 1. 관리자: Linux 번들 만들기

RHEL 8.6/8.10 x64 또는 동일한 ABI로 검증할 native Linux 빌드 환경에서 실행한다.
macOS에서 만든 archive는 Linux에서 실행할 수 없다. 설치된 Rust toolchain·linker,
완전한 `rust/vendor`, `sh`, `tar`, `unzip`, SHA-256 도구가 필요하다.
의존성/OS 패키지를 자동 다운로드·설치하지 않는다.

```sh
cd /path/to/floe
sh tools/make_electron_portable.sh \
  --runtime-zip /path/to/electron-v44.4.3-linux-x64.zip \
  --out /path/to/NEW-floe2-linux.tar.gz --jobs 4
```

공식 Electron ZIP을 사전에 준비한다. 버전·플랫폼·SHA-256은 `electron/runtime.json`
고정값과 일치해야 하며 다른 archive는 거부한다. 기존 출력은 덮어쓰지 않는다.
Rust 실행 파일은 같은 소스에서 offline release 빌드하며 GLIBC 요구 상한 2.28을
검사한다. 이 검사를 통과해도 모든 OS GUI 라이브러리와 Chromium sandbox가
현장에서 실행된다는 뜻은 아니다. [번들 상세 계약](WEBUI_ELECTRON_PORTABLE.ko.md).

번들에는 Electron runtime, Rust CLI·index·renderd·service·download helper와
UI/고지가 모두 들어간다. **최종 사용자는 Electron, Python, Node, Cargo, 저장소를
별도로 설치할 필요가 없다.** 시스템 GUI 라이브러리는 여전히 필요하다.

```sh
# 비어 있는 설치 디렉터리에서 해제. 기존 실행 중인 번들 위에 덮어쓰지 않는다.
tar -xzf /path/to/NEW-floe2-linux.tar.gz
sh /opt/floe2-electron-comparison/verify.sh
```

압축 해제 위치에 맞게 아래 `/opt/floe2-electron-comparison`을 바꾼다. 번들 내부
디렉터리 이름은 호환성을 위해 `floe2-electron-comparison`을 유지한다. 관리자만
수정 가능한 설치본을 권장한다. 체크섬은 손상 검출이지 게시자 서명이 아니다.

## 2. 사용자의 ETX 세션에서 점검

```sh
/opt/floe2-electron-comparison/floe2-teebox --check
```

이 명령은 Linux x64, non-root, DISPLAY 유무, 실행 파일 구성, bundle verify,
native selfcheck, Chromium 및 동봉 `.so`의 `ldd`를 검사한다. 창·설계·클립보드를
열거나 인덱싱하지 않는다. DISPLAY 값/인증 정보를 출력하지 않는다.
고정 번들 검사를 위해 `FLOE_ELECTRON_BIN`, `FLOE_ELECTRON_SERVICE_BIN`,
`FLOE_ELECTRON_DOWNLOAD_BIN`, `FLOE_INDEX_BIN`, `FLOE_RENDERD_BIN`은 **unset**해야 한다.
`ELECTRON_RUN_AS_NODE`, `NODE_OPTIONS`, `NODE_PATH`, `NODE_EXTRA_CA_CERTS`도 쓰지 않는다.

`--check` 성공은 X 서버 접속·sandbox 실행·동적 선택 라이브러리·렌더 수용이 아니다.
그 항목은 실제 창으로 확인해야 한다. RHEL8/ETX 실측은 아직 하지 않았다.

## 3. TeeBox 실행 명령

```sh
/opt/floe2-electron-comparison/floe2-teebox view \
  /shared/layouts/chip.oas --root /shared/layouts

# 잡덱: 레벨을 지정하지 않으면 앱에서 선택
/opt/floe2-electron-comparison/floe2-teebox view \
  /shared/layouts/mask.jb --root /shared/layouts --level 1,3

# 파일을 아직 고르지 않은 창
/opt/floe2-electron-comparison/floe2-teebox view --root /shared/layouts
```

TeeBox에는 실행 파일 경로와 **인자 배열**로 등록한다. 파일명/사용자 입력을 shell
문자열로 이어 붙이거나 `eval`하지 않는다. 절대경로 사용을 권장하며 상대경로는
호출자의 cwd 기준이다. 실행기는 cwd를 바꾸지 않는다.

- ETX가 제공한 `DISPLAY`, `XAUTHORITY`와 OS 사용자 실행 환경을 상속한다.
  임의 `:0`으로 바꾸거나 Xauthority cookie를 argv/로그에 넣지 않는다.
- `floe2-teebox`는 `FLOE_ELECTRON_X11=1`로 이 실행에만 Chromium X11 backend를
  선택한다. 기존 `floe2-electron`의 macOS/일반 실행은 바꾸지 않는다.
- 기존과 같이 같은 UID+DISPLAY 기본 창을 재사용한다. `--multi`는 독립 창이다.
  `--jobs`, `--raster-jobs`, DRC 옵션 등 명시 process 옵션도 독립 실행을 만든다.
  새 호출의 성공은 요청 수락이지 렌더 완료 통지가 아니다.
- 실행기는 `exec`로 교체되어 종료 코드·신호를 유지한다. TeeBox는 장기 GUI
  프로세스로 관리하고 명령 완료 timeout으로 바로 죽이지 않아야 한다.
  외부 SIGHUP/SIGTERM은 Rust worker 종료/정리로 이어진다.
- `TMPDIR`과 instance rendezvous는 쓰기 가능한 **로컬** 영역을 사용한다.
  기본 `/tmp` 기반 IPC를 NFS로 옮기지 않는다. TeeBox 실행기를 root로 돌리지 않는다.
- `--root`는 추가 파일 선택/의존 폴더 승인이지 chroot나 사용자 인증 경계가 아니다.
  explicit source의 부모도 기존 standalone 규칙으로 허용된다.

작업량을 제한한 독립 창의 예:

```sh
/opt/floe2-electron-comparison/floe2-teebox view /shared/layouts/chip.oas \
  --root /shared/layouts --multi --jobs 4 --raster-jobs 4 --budget-mb 1024
```

자원 제한은 **앱 프로세스별**이다. 여러 사용자의 총 CPU/메모리를 통합 예약하는
공용 서비스는 이번에 도입하지 않는다. 동시 실행 수는 TeeBox/관리자에서 조절한다.

## 4. 인덱스: 공용 경로 재사용, 생성은 명시적으로

동일 원본 경로의 current 캐시를 재사용하며 사용자별로 OASIS/캐시를 복사하지 않는다.
없거나 stale이면 UI의 Index/Index and open에서 명시 승인하거나, GUI 없이 다음을 쓴다.

```sh
/opt/floe2-electron-comparison/floe2-cli selfcheck --adjacent
/opt/floe2-electron-comparison/floe2-cli index /shared/layouts/chip.oas --jobs 4
/opt/floe2-electron-comparison/floe2-cli index /shared/layouts/mask.jb --jobs 4
/opt/floe2-electron-comparison/floe2-cli info /shared/layouts/mask.jb --json
```

`floe2-cli`는 같은 번들의 `floe2-web` Rust CLI 진입점이다. 위 명령은 Electron이나
브라우저 없이 실행된다. help/진단의 프로그램 이름은 `floe2-web`으로 나온다.
jobdeck은 `--level 1,3`, 필요할 때 `--lod`를 전달할 수 있다. 일반 기본 LOD는 off,
jobdeck occupancy는 on이다. current 캐시의 생성 옵션을 바꾸려면 기존 색인 규약을 따른다.

소스 옆 숨김 `.ice/` 부모의 쓰기 권한·공간이 필요하다. 공유 index를 최초 한 명이
명시적으로 만든 뒤 다른 사용자가 연다. **서버형 최초 요청 병합/전역 빌드 스케줄러는
연결하지 않았다.** 같은 소스의 동시 rebuild는 관리자가 직렬화하고, 사용 중인
캐시에 무작정 `--force`를 쓰지 않는다. `--force`는 기존 캐시 교체 승인이며 열린
구버전 도구까지 전부 보호하지 않는다. NFS 캐시 자동 회수/삭제도 하지 않는다.

## 5. DRC 개인 파일과 사용자 경계

```sh
/opt/floe2-electron-comparison/floe2-teebox view /shared/layouts/chip.oas \
  --drc /shared/reviews/chip.ice --drc-reviewer alice --drc-edit-waives
```

런처가 지정한 reviewer의 기존 standalone sidecar 규약을 사용한다. 저장은 미리보기·
명시 승인, 자동 저장은 별도 opt-in이다. 읽기만 하려면 `--floe-reviewer alice`를 쓰며
쓰기 옵션과 섞지 않는다. reviewer는 경로/라벨 구분이지 로그인 신원 증명이 아니다.
같은 OS 계정으로 여러 사람을 실행하면 OS 파일 접근도 같은 권한이다. 다른 DISPLAY나
`--multi`는 창을 나눌 뿐 보안 격리를 제공하지 않는다. TeeBox가 reviewer를 정하더라도
인증된 사용자별 개인 작업공간 서비스가 구현된 것으로 취급하지 않는다.
xattr 없는 NFS 저장 호환성은 [기존 계약](WEBUI_NO_XATTR.ko.md)을 유지한다.

## 6. 문제 발생 시

| 증상 | 확인 |
|---|---|
| DISPLAY 없음 / 창이 안 보임 | TeeBox의 실제 ETX 사용자 환경에서 호출했는지, XAUTHORITY 접근 권한 확인. `xhost +`로 풀지 않음 |
| incomplete bundle / 바이너리 버전 불일치 | 전체 새 archive 해제·verify. 구 binary override 제거. 파일 일부만 교체하지 않음 |
| missing Chromium libraries | 신뢰하는 설치본의 `ldd runtime/electron` 결과를 관리자와 확인. 필요한 OS 패키지 변경은 별도 승인 |
| No usable sandbox / namespace 오류 | 서버 보안 정책과 Electron의 sandbox 호환성 확인. `--no-sandbox`, setuid 변경, 커널 설정 변경으로 우회하지 않음 |
| 기존 창에 안 붙음 | UID·DISPLAY·`--multi`·process/DRC 옵션 확인. 기존 모달/미저장 편집은 자동으로 승인되지 않음 |
| 창이 곧 닫힘 | TeeBox가 자식 프로세스를 회수/종료했는지, 실행기 stderr/종료 코드 확인. 인증 URL/사용자 파일 내용을 진단 로그에 추가하지 않음 |

## 7. 검증 범위

새 실행기의 공백/한글 argv·cwd·DISPLAY/XAUTHORITY 보존, Linux/non-root 거부,
override/missing bundle 거부, headless CLI, verify/ldd 실패 전파와 X11 opt-in은
`validate_electron_portable.py --self-test`에 연결했다. Linux 부분은 모의 환경 검사다.
새 번들의 로컬 실행 결과는 아래 기록하며 **RHEL 8.6/8.10 + ETX 실제 창, 물리 입력,
NFS 다중 사용자·대형 칩 성능은 현장 수용 대기**다.

2026-09-29 로컬 기록:

- packager Rust13 unit·`--all-targets --no-deps -D warnings` clippy 통과.
  실행기/closure Python7개, Node의 프로토콜·신호·X11·QA 조건10개 통과.
- 정규 선택 배터리 `app_cli,native_revision,web_selfcheck,web_portable` ALL OK.
  최종 QA/실행기 수정 뒤 `web_portable`도 재실행해 ALL OK
  (`/private/tmp/floe-teebox-gates-final.log`). 전체 배터리를 새로 통과했다는 의미는 아니다.
- 새 macOS arm64 archive를 공백·한글 경로로 재배치, 독립 inventory/자체 verify
  **992항목** 통과. `/usr/bin:/bin` PATH, DISPLAY 없이 동봉 CLI의 합성 OASIS 색인·
  current 재사용·JSON 조회·원본 해시 불변 통과. 별도 합성 잡덱의 레벨1,2와 `--lod`
  전달·occupancy 생성·조회도 통과했다. 개발 fixture 생성에만 Python/KLayout을 썼다.
- 실제 **재배치 번들의** Electron에서 합성 valmini 표시·인증·종료/worker join 통과.
  재사용 on/off 각각 foreground/margin 도형(+frames/frames off) **2,159,880픽셀 차이0**.
  labels-on의 하단28픽셀은 기존 문서의 margin-only anchor 규약과 같으며 exact0으로
  세지 않는다. 캡처도 직접 확인했다. source/cache SHA-256 불변.
- 첫 실제 QA는 기본 margin off인데 margin 프레임을 기다려 실패했다. layout/parity/
  recovery QA에만 `--margin on`을 명시하고 다시 빌드해 같은 픽셀 기준으로 통과했다.
  정상 실행 기본값·오라클·timeout은 바꾸지 않았다. recovery QA 옵션 자체는 단위
  검사했으며 이번 실제 실행은 layout parity 경로다.

최종 macOS 합성 검증 archive:
`/private/tmp/floe-teebox-bundle.8R2Iit/floe2-standalone-verified.tar.gz`,
SHA-256 `6fd209731b40ac8209bb0444b3b88b7a0c8bb524451c6a7423947ae5a7a64135`.
source revision은 미커밋 작업트리 `071c1570e43321c433a18815d5d1c3aac03b80a0+`다.
이 archive는 **Linux 배포물이 아니다**. RHEL에서는 §1대로 따로 native 빌드해야 한다.
검증 뒤에도 `BUILD.txt`의 `field_acceptance=unverified`는 유지한다.
로그: `/private/tmp/floe-teebox-gates.log`, `floe-teebox-bundle-verified.log`,
`floe-teebox-verified-inventory.log`, `floe-teebox-electron-parity-final.log`.
최초 실패 기록 `floe-teebox-electron-layout.log`도 보존한다.
