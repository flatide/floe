# Electron 오프라인 개발 비교 번들

2026-09-21. [Electron E3](WEBUI_ELECTRON.ko.md)의 **로컬 조립 단계**다.
RHEL 8.6/8.10·ETX 정식 채택, 서명/공증, 설치 프로그램 완료를 뜻하지 않는다.
기존 macOS WKWebView 앱은 유지한다. 렌더·표시 정책/제품 버전은 바꾸지 않았다.

## 만들기와 실행

설치된 native Rust toolchain, `sh`, `unzip`, `tar`, `sha256sum` 또는 `shasum`이
필요하다. npm·Python·네트워크 다운로드는 사용하지 않는다. 공식 Electron zip은
사전에 별도로 준비하며 `electron/runtime.json`의 해당 OS/CPU SHA-256과 일치해야 한다.
현재 후보는 44.4.3이다. 다른 버전/플랫폼 zip 또는 잘못된 해시는 hard error다.

```sh
sh tools/make_electron_portable.sh \
  --runtime-zip /absolute/path/electron-v44.4.3-darwin-arm64.zip \
  --out /absolute/path/NEW-floe2-electron.tar.gz --jobs 4
```

macOS arm64/x64 및 Linux x64 GNU **native compiler host**만 지원한다. 크로스 빌드나
기존 임의 바이너리 디렉터리를 입력으로 받지 않는다. Rust worker 4개는 동일 소스의
release로 `--offline --locked` 빌드한다. jobs 기본4, 범위1..16이다.
`CARGO_TARGET_DIR`은 재사용 build 디렉터리로 쓸 수 있다. 기본은
`rust/target/electron-portable`이다. 기존 출력·심볼릭 링크는 교체하지 않는다.

새 디렉터리에서 압축을 풀고 **상위 런처**를 실행한다.

```sh
tar -xzf /absolute/path/NEW-floe2-electron.tar.gz
sh floe2-electron-comparison/verify.sh
/absolute/path/floe2-electron-comparison/floe2-electron view ./design.oas
```

작업 디렉터리를 변경하지 않으므로 상대 OASIS 경로는 호출한 터미널 기준이다.
Electron 내부 `.app`을 따로 옮기거나 Finder/`open`으로 실행하는 배포가 아니다.
실행에는 저장소·Cargo·별도 Node/Python·외부 브라우저가 필요하지 않다. Linux의
시스템 GUI/보안 라이브러리까지 동봉한다는 의미는 아니다.

개발용 `FLOE_ELECTRON_BIN`, `FLOE_ELECTRON_SERVICE_BIN`,
`FLOE_ELECTRON_DOWNLOAD_BIN`, `FLOE_INDEX_BIN`, `FLOE_RENDERD_BIN`은 유지한다.
번들 자체를 검사할 때는 모두 unset한다. 명시한 무효 값에 fallback은 없다.
`ELECTRON_RUN_AS_NODE`, `NODE_OPTIONS`, `NODE_PATH`가 비어 있지 않으면 런처가 거부한다.

## 조립과 무결성 계약

- 기존 Rust portable packager의 vendored 의존성/고지 수집, 취소, 새 private staging,
  같은 파일시스템 hard-link 기반 no-clobber 게시를 재사용한다. Electron service는
  자체 vendor 디렉터리 없이 공유 `rust/vendor`를 사용한다.
- 먼저 caller zip을 private stage에 복사하여 **그 복사본의 SHA-256을 확인한 뒤**
  같은 파일을 해제한다. 공식 runtime 전체를 유지하며 Framework 내부 상대 링크도
  보존한다. 외부/절대/깨진 링크·특수 파일·setuid/setgid/sticky bit는 거부한다.
- 앱은 JS/shared UI의 명시 목록만 복사한다. 저장소 전체·설계·캐시·리뷰·사용자 설정은
  포함하지 않는다. 합성 QA 모듈도 의존 파일을 함께 포함하되 명시 `--smoke-*`에서만
  실행된다. 특히 clipboard smoke는 별도 승인 필요하며 일반 조립/검사는 실행하지 않는다.
- `BUNDLE.json`은 source revision, target, runtime archive 해시와 모든 일반 파일의
  SHA-256/크기/권한, 디렉터리 권한, 상대 링크 대상을 기록한다. manifest 자신은
  자체 해시에 포함하지 않는다. `verify.sh`는 누락/추가/변경을 검사하며 자동 매-기동
  스캔은 하지 않는다. 검증기 실행 파일까지 바꿀 수 있는 공격자에 대한 인증은 아니다.
- `NOTICES/`는 Rust/build/font/toolchain 고지 목록이다. 공식 runtime의 `LICENSE`와
  `LICENSES.chromium.html`도 그대로 포함한다. 이 수집 결과는 법적 배포 승인이 아니다.
- Linux Rust worker·검증기는 기존 엄격한 ELF/GLIBC2.28 검사를 통과해야 한다.
  Chromium의 `$ORIGIN`/GUI 의존성은 별도 계약이며 Rust 검사 기준을 완화하지 않는다.
  이 빌드가 전체 Chromium ELF/SONAME·현장 sandbox 실행을 검증하지는 않는다.
- `BUILD.txt`는 `gui_checked=false`, `field_acceptance=unverified`로 생성된다.
  패키징 자체가 창·설계·클립보드를 열거나 GUI 검사를 한 것처럼 기록하지 않는다.

공식 [배포 지침](https://www.electronjs.org/docs/latest/tutorial/application-distribution)의
rebranding/서명 배포와 달리 runtime 옆에 앱 소스를 두고 경로로 실행하는 개발 비교판이다.
[fuse](https://www.electronjs.org/docs/latest/tutorial/fuses)는 변경하지 않았다.
런처의 환경변수 거부는 변경되지 않은 Electron 바이너리의 fuse를 비활성화하는
보안 경계가 아니다. 서명·배포 시 fuse/업데이트/고지 수용은 별도 작업이다.
OS 패키지 설치, setuid 변경, namespace 설정 변경, `--no-sandbox` 우회는 하지 않는다.

## 회귀·실행 기록

```sh
(cd rust && cargo test -p floe-web-packager --offline --locked)
(cd rust && cargo clippy -p floe-web-packager --all-targets --offline --locked --no-deps -- -D warnings)
.venv/bin/python -B tools/validate_electron_portable.py --self-test
.venv/bin/python -B tools/validate_electron_portable.py --bundle /absolute/path/floe2-electron-comparison
```

GUI 없는 launcher/JS 파일 목록·재배치 probe 로더 검사는 전체 배터리의 `web_portable`에 배선했다.
packager unit11개는 옵션/출력 충돌·내용/권한/링크·기존 macOS notices를 포함한다.
실제 창 검사는 기본 배터리에 자동 추가하지 않는다. 재배치 번들로 기존 합성 valmini
driver를 실행할 수 있다(이 개발 fixture 생성에만 Python/KLayout 사용).

```sh
# 앞의 다섯 runtime/worker override는 먼저 unset
FLOE_QA_ELECTRON_BUNDLE=/absolute/path/floe2-electron-comparison \
  node tools/validate_electron_layout.cjs --frame-parity
```

번들 모드는 runtime/worker override가 하나라도 있으면 거부한다. 앱 cwd는 새 합성
폴더, 앱 PATH는 `/usr/bin:/bin`으로 고정한다. oracle 생성기는 저장소에 있지만,
실제 앱·index/renderd/service/download helper는 전부 재배치 번들 것을 사용한다.

로컬 macOS arm64 결과:

- 최초 archive132MiB, SHA-256
  `292ed05c9dd440f51f6de0c198730194dfe6789c17941a7d0058d14168fd5eb4`.
  `/private/tmp/floe-electron-comparison-0.12.185-dev.tar.gz`.
- `/private/tmp/floe-electron-bundle-qa.BychpC/이동 경로/floe2-electron-comparison`으로
  압축 해제 후 Rust 검증기와 독립 Python 검사 모두 **965항목 통과**.
- 실제 sandboxed Electron 창의 Rust 인증, 탐색/새 창 거부, 기본 취소·종료와 service
  join 통과. screenshot을 읽어 초기 합성 Browse 창 표시를 확인했다.
- 번들 download helper의 실제 Chromium blob 저장/취소/충돌, 수신 중 취소,
  Rust0600/no-clobber·정리 검사 통과. 기존 파일이나 OS 클립보드는 바꾸지 않았다.
- 같은 번들의 valmini render, pan reuse on/off 모두 geometry와 geometry+frames
  **2,159,880 RGBA 픽셀 차이0**. labels-on28픽셀은 기존 수용된 margin 외부 anchor
  정책으로 별도 보고하며 새 결함/정책 변경으로 다루지 않는다.
  source/cache 불변·정상 종료 통과. 이번 단계에는 실제 clipboard 변경 없음.
- 호스트 Node 단위29개 통과. 물리 마우스/IME, 실제 OS 저장창, WK 대조 성능,
  RHEL/ETX 실행은 이 결과에 포함하지 않는다.
- packager unit11개 및 `--all-targets --no-deps -D warnings` clippy 통과.
  실제 패키저의 잘못된 zip 해시 거부(출력/stage 잔류 없음), 기존 archive 거부와
  기존 SHA-256 불변도 확인했다.
- launcher mock의 최초 검사는 macOS `/var`→`/private/var` 정규화 차이로 실패해
  fixture 경로를 canonicalize했다. 재실행은 새 shebang executable 시작이10초를
  넘겨 실패했다. timeout을 늘리지 않고 설치된 `/bin/sh`가 mock app의 NUL 구분
  argv를 출력하게 바꾼 뒤 두 검사 모두0.097초에 통과했다. 실제 Electron 창/렌더
  검사는 이 mock 검사와 별개이며 앞의 실제 실행 결과로만 수용한다.

로그: `/private/tmp/floe-electron-bundle-{build,unit,host-unit,smoke,layout}.log`.
clippy 로그는 `floe-electron-bundle-clippy{,-native}.log`, launcher 최종은
`floe-electron-bundle-launcher-final.log`다. 전체 배터리는
`floe-electron-bundle-full.log`로 별도 기록한다.

**전체 green은 아니다.** 이번 `sh tools/validate_rust.sh`는2026-09-22에 exit1로
종료했다. workspace 결과778 passed/89 ignored 및 별도 worker lifecycle14개는
통과했다. ignored 항목은 별도 oracle gate에서 실행해야 하며 통과로 세지 않는다.
index CLI·셀 profile·floe2 product·Rust app CLI·cache migration·CLI inventory·native
revision·selfcheck·기존 portable·새 Electron launcher·macOS native runtime smoke·
내장 서비스 lifecycle·app read까지 통과했다. 이후 `validate_layerprops.py`가
새 `target/debug/deps/layerprops-1094761046431e48 --ignored --nocapture`를 실행하는
부분에서30초 timeout으로 종료했다. 새 native executable 시작 사이의 긴 대기는
본문이0초로 끝나는 ignored-only executable에서도 관찰했다. 이를 전체 성공이나
`layerprops` 의미 검증 완료로 집계하지 않는다.
별도로 실행한 기존 `validate_web_portable.py`도 synthetic `rustc -Vv`의
`metadata/EOF timed out`(10초)으로 notice 누락 단언에 도달하지 못했다.
`floe-electron-bundle-existing-portable.log`에 실패를 남긴다. 이것만으로 OS 검사나
패키징 코드 중 어느 쪽이 근본 원인인지 확정하지 않으며, 기존 gate의 timeout/
오라클을 완화하지 않았다. 같은 소스의 **후속 전체 배터리 안에서는 기존 portable
검사가 통과**했지만, 이 성공으로 앞선 단독 실행 실패나 간헐적 기동 문제를 지우지
않는다. 후속 [native oracle 단계/loader 진단](WEBUI_NATIVE_STARTUP.ko.md)에서
테스트 본문 전 대기와 실제 속성 오라클 통과를 별도 확인했다. 기동 문제 해결이나
전체 재통과로 판정하지 않는다.

### 2026-09-22: 패키저 밖 startup 대조

기존 portable gate의 `fixture()`를 새 임시 폴더에서 호출해 같은 합성 `rustc` 본문을
패키저 **밖에서** 실행했다. 본문은 고정 문자열 하나를 출력하고 끝난다.

- 새 shebang executable 직접 실행: 최초2회 모두15.004초 관찰 timeout.
- 설치된 Python에 script 경로를 인자로 전달: 같은 대조에서0.032/0.034초, 출력 일치.
- interpreter 경로의 정규화 차이도 분리했다. resolved Python 명시 실행은
  0.025/0.030초, venv Python 명시 실행은0.032/0.022초였다. shebang을 venv 경로로
  바꿔도 첫 실행은15초 timeout, 다음 새 fixture는3.709초였다. 원래 resolved
  shebang은 두 실행 모두15초 timeout이었다.
- 별도의 직접 실행을1초 sampling했을 때795개 표본 모두 `_dyld_start +0`,
  footprint96KiB였다. 이 sampling 실행 자체는5.343초 뒤 출력/종료에 성공했다.

따라서 이 재현에서는 notice 처리나 Rust의 pipe drain 루프 없이도 지연이 발생하며,
적어도 sampling 시점은 Python 본문 이전이다. interpreter의 경로 정규화만으로
설명되지는 않는다. Gatekeeper/AMFI/파일 감시 등 구체적인 OS 원인과 전체 Rust gate의
각 실행 지연이 같은 원인인지는 **확정하지 않았다**. 관찰 timeout은 제품 deadline을
늘린 결과가 아니며, 실패한 기능 gate를 통과로 바꾸거나 미리 실행해 숨기지 않았다.

진단 artifact: `/private/tmp/floe-packager-startup-{probe,matrix,sampled}.log`,
`/private/tmp/floe-packager-startup-p203_cc_/49125378489541/startup.sample`.
이 진단은 새 합성 helper만 실행/정리했으며 기존 설계·리뷰·클립보드·OS 설정은
읽거나 바꾸지 않았다. 전체 배터리는 같은 session을 끝까지 관찰해 위의 exit1을
확인했다. 사용한 임시 `.venv` 링크는 종료 trap에서 제거됐고 합성 Electron/진단
프로세스는 종료했다. PATH에서는 Docker/Podman/Colima/Lima/QEMU 실행기를 찾지
못했다. 이 확인을 시스템 전체 미설치 증명으로 확대하지 않으며, 별도 설치나
사용자가 보류한 외부 CI 실행은 하지 않았다.

## 2026-09-22 공유 프레임 대조 파일 보완

고정 viewport 대조(`FLOE_QA_CROSS_HOST=1`)가 읽는 공유 파일3개가
`APP_FILES`에 빠져 있었다: `layout-parity-probe.js`, `cross-viewport-probe.js`,
`frame-fingerprint-probe.js`. 저장소에서 실행하면 읽히지만 재배치 번들에는 없었다.
일반 레이아웃 표시 실패가 아니라 **명시적 비교 QA 경로**의 패키징 결함이다.
기존 검사도 `require()`와 과거 공유 파일3개만 확인하여 동적 `readFileSync`를 놓쳤다.

목록을 고쳤으며 새 회귀는 실제 `APP_FILES`만 임시 폴더로 복사하고 패키지의
`layout-qa.cjs` 로더를 실행한다. 첫 브라우저 평가 직전에 생성된 script를 구문
검사하고 멈추므로 픽셀 성공을 흉내 내지 않는다. 정상 목록의 파일 읽기와 구성,
공유 probe4개를 각각 뺀 ENOENT를 확인한다. 수정 전 실제 누락으로4조합이 실패했고
수정 후 launcher/closure3개 검사가 통과했다. 로그 `floe-parity-bundle-closure-{before,after}.log`.

새 macOS arm64 개발 archive를 만들어 공백·한글 경로로 옮긴 뒤 실제로 실행했다.
Rust/서비스/renderd/runtime override5개를 모두 unset하고 앱 cwd는 새 합성 폴더,
PATH는 `/usr/bin:/bin`이다. Python/KLayout은 저장소 쪽 valmini 생성에만 사용했고
실제 앱의 worker/runtime/shared script는 모두 재배치 번들 것을 썼다.

```sh
# 앞의 runtime/worker override5개를 모두 unset한 개발 환경에서 실행
FLOE_QA_ELECTRON_BUNDLE=/absolute/path/floe2-electron-comparison \
FLOE_QA_CROSS_HOST=1 \
  node tools/validate_electron_layout.cjs --frame-parity
```

- pan reuse on/off 각각 geometry+frames+labels → geometry+frames → geometry3단계, 총6단계 통과.
  모든 단계1600×1200 device px/DPR2, world bbox `[50000,87500,350000,312500]` DBU.
  전경/margin의 raw RGBA 해시는 이 fixture에서 모두 일치했다. driver도3개 phase,
  픽셀 수/DPR/카메라/해시를 파싱하여 자식의 exit0 문구만으로 통과시키지 않는다.
- source/cache SHA-256 불변, native 취소/명시 종료·서비스 join, sandbox/탐색 차단 통과.
  실제 창 PNG도 확인했다. 패키지970항목은 실행 전 독립 inventory·자체 검사와
  실행 후 자체 검사에서 일치했다. `floe-parity-bundle-layout.log`, `...-inventory.log`.
- packager unit11개, fmt/clippy, 공유 probe/record 파서12개, 정규
  `sh tools/validate_rust.sh --only web_portable` 통과. 전체 Rust/web 배터리 통과가 아니며
  `af40d64`에 기록한 전체 oracle-build180초/helper3건 timeout은 여전히 열린 상태다.
- archive: `/private/tmp/floe-electron-parity-0.12.185-af40d64.tar.gz`, SHA-256
  `0cb6e096ebf8d47597a15b33a2c54ca01dfa1f04f5c919d70429e582ba40f411`.
  source revision은 수정 작업트리 `af40d642c57f821357bab05bdbec108e8747f65d+`다.
  검사 경로: `/private/tmp/floe-parity-relocated.lfUF8J/이동 경로/floe2-electron-comparison`.
  Rust 제품0.12.185/Electron shell0.1.2/런타임44.4.3은 바꾸지 않았다.

이는 **패키지의 공통 probe 실행**이며 WKWebView를 다시 실행한 동시 교차 호스트
대조나 input→photon/성능/RHEL/물리 입력 수용은 아니다. 합성 검사는 클립보드·
DRC 저장·기존 설계를 건드리지 않았다. `BUILD.txt`의 `gui_checked=false`는 조립 시점의
자동 GUI 검사 부재를 뜻하므로 사후 개발 검사로 그 파일을 고쳐 무결성을 깨지 않았다.

## 전체 goal 잔여

이 단계는 저장소 의존적인 Electron 비교 실행을 이동 가능한 로컬 산출물로 만든 것이다.
G1 동일 조건 GTK/WK/Electron 지연·메모리 비교, G4 확대 실제 UI·DRC 저장 중 crash/
결과 불명 수용, OS 입력·IME·접근성, single-instance/Finder 정책, RHEL8.6/8.10+ETX와
Python-free Linux 실행, 정식 호스트 채택·서명/배포가 남는다. 기존 간헐적 초기 프레임
대기 실패도 성공한 이번 실행만으로 해결 판정하지 않는다. 원격 공유 단계는 사용자 보류다.
