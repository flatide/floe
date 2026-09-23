# WKWebView host — frozen

2026-09-23 사용자 결정: **Electron을 macOS/Linux의 유일한 네이티브 제품 호스트로
개발한다. WKWebView 호스트는 동결한다.** 외부 브라우저용 `floe2-web`은 유지한다.

동결 기준은 `617909e`다. WK 전용 코드·의존성·개발 앱 빌드/실행 도구·과거 검증
기록을 삭제하지 않고 참조용으로 보존한다. 새 기능, 창 재사용, 최적화, 배포 준비,
정기 GUI 수용 작업을 WK 쪽에 추가하지 않는다. 재개는 별도 사용자 결정이 필요하다.
기존 snapshot은 `git show 617909e:desktop/...`로 확인할 수 있다. 현재 공유 코드를
다시 빌드한 앱이 그 snapshot과 동일하다고 보장하는 정책은 아니다.

## 공유 코드 경계

`desktop/` 전체가 WK 전용은 아니다. 현재 Electron이 다음을 사용한다.

- `src/service.rs`: Rust 세션 수명·취소/정리.
- `src/download_fs.rs`, `src/transfers.rs`: 다운로드 파일/전송 관리.
- `ui/`의 menu/recovery/frame 비교 모듈과 그 회귀 검사.

이 공통 부분과 `rust/app::embedded`, Rust 서비스/웹 UI는 **Electron 개발 범위에
남는다**. 경로가 `desktop/`이라는 이유로 삭제하거나 검증을 빼지 않는다. 변경 시
Electron 소비자와 패키지 파일 목록을 확인한다. 별도 공통 경로로 옮길 경우에는
정상 리팩터링으로 의존 경로·패키징·회귀를 함께 검증하고 코드를 복제하지 않는다.
이번 동결에서는 파일 이동, vendor 변경, 실행 코드 수정이 없다.

## 검증과 후속 작업

- Electron 단독 실행/입출력/복구·GTK 대비 G1·RHEL/ETX가 제품 수용 대상이다.
- `tools/validate_desktop.sh`와 `tools/validate_native_frame_parity.cjs`는 과거 WK
  전용/교차 비교 도구로 보존하며, 앞으로의 필수 완료 게이트로 요구하지 않는다.
- 기존 headless `embedded_host` 게이트에는 공통 경로 검사가 섞여 있으므로
  통째로 제거하지 않는다. 게이트 분리는 실제 코드/패키징 변경 단계에서 검증한다.
- Electron 선택은 RHEL 호환성·서명/공증·배포·실제 성능 통과를 뜻하지 않는다.

현재 개발 계획: [Electron](../docs/WEBUI_ELECTRON.ko.md),
[창 재사용](../docs/WEBUI_NATIVE_INSTANCE.ko.md).
WK 구현/검증 이력: [데스크톱 기록](../docs/WEBUI_DESKTOP.ko.md).
