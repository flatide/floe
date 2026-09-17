# floe2 웹 셸 / 서버-클라이언트 계획 (정본)

작성 2026-08-29, 갱신 2026-09-18(M4g-41 OVR 정방향 통합·로컬 회귀; 원격 단계 보류).
관련 정본: `FLOE2_OPTIMIZATION.ko.md`(F2R-10/11),
`RUST_RENDERER_PLAN.ko.md`, `SPEC-VIEWER.ko.md`, `rust/BUILD.md`.

현재 상태: **M0 로컬 조사/API 초안 + M1a worker client·공유 서비스 + 일반/잡덱 index·info/render/probe·분석/spec CLI**.
M1b-1에서 의존성 선정과 인증된 loopback HTTP/WS transport 기반을 추가했다.
M1b-2a에서 managed lease/admission과 native view controller를 추가했다.
M1b-2b에서 사전 등록한 view의 인증된 제어/프레임 스트림을 연결했다.
M1b-2c1에서 로컬 등록 scope·관리형 색인/진행·취소 수명을 연결했다.
M1b-2c2에서 인증된 catalog·view 생성/재open·색인 작업 API를 연결했다.
M1b-3에서 Rust `view` 실행 명령과 번들 Canvas 기본 뷰어를 연결했다.
M1b-4a에서 layout margin prefetch/crop과 라벨 포함 pan 재사용을 연결했다.
M1b-4b에서 drag·fill/width/font 편집·기본 GTK 단축키를 연결했다.
M2a-1에서 기존 DRC ICE pack 읽기·공간 페이지·waive 조회 CLI를 이관했다.
M2a-2에서 DRC 전용 actor·자원 예약과 등록 ID 기반 인증 읽기 API를 연결했다.
M2a-3에서 규칙/오류 페이지·waive 필터·goto·표시 프레임에 정렬된 DRC overlay를 연결했다.
M2a-4a에서 view별 DRC 패널 상태의 서버 메모리 보존·revision 충돌 API를 추가했다.
M2a-4b에서 유계 브라우저 저장 큐·새로고침/재접속 복원을 연결했다.
M2a-5a에서 현재 규칙/필터의 유계 순회 API와 좌표 slice 읽기를 추가했다.
M2a-5b에서 페이지 횡단 순회·click/이동 모드·Escape와 해당 상태 복원을 연결했다.
M2a-6에서 표시된 DRC 마커의 단일/이중 클릭을 release-only pan과 분리해 연결했다.
M2a-7a에서 단순 DRC 오류의 CD 측정 코어와 읽기 API를 추가했다.
M2a-7b에서 마지막 이동 오류의 CD 치수선·값 표시, k/K/Escape와 상태 복원을 연결했다.
M2a-8a에서 규칙별 DRC 선택 집합과 현재 페이지 후보 bbox 판정·revision 충돌 API를 추가했다(UI는 다음 단계).
M2a-8b에서 두 클릭 박스·Shift/Ctrl/Cmd 선택과 규칙별 금색 마커·세션 복원을 연결했다.
M2a-9a에서 현재 규칙의 Selected·waive·live viewport 교집합 목록/순회 코어와 인증 API를 추가했다(UI는 다음 단계).
M2a-9b에서 Selected·현재 규칙 live In view UI, 유계 추종/복원·마커 hover를 연결했다.
M2a-10a/b에서 SVRF sidecar 읽기·타입/규칙 필터·측정 비교 코어/CLI/API를 연결했다.
M2a-10c에서 웹 타입 선택/상세/비교와 성공한 오류 이동 시 In view 해제를 연결했다.
M2a-10d1에서 서버의 레이어 격리·한 번만 복원과 준비 토큰 기반 원자적 focus API를 추가했다.
M2a-10d2에서 승인 후 웹 상태 반영·Restore layers·Escape와 취소/재접속을 연결했다.
M2a-11a에서 읽기 전용 ASCII DRC 코어·CLI fallback과 소수 좌표 측정 parity를 추가했다.
M2a-11b에서 명시한 ASCII DRC의 웹 등록·유계 조회·소수 좌표 윤곽/CD를 연결했다.
M2a-12a에서 명시 DRC pack 생성 코어/CLI와 임시 생성·검증·원자 게시·취소를 추가했다.
M2a-12b1에서 owner HTTP 생성/취소와 이전 DRC 응답 무효화·새 identity 재등록을 연결했다.
M2a-12b2에서 pack 생성 승인·진행/취소·불명확한 요청 확인 UI와 새 리뷰 조회 복원을 연결했다.
M4a-1에서 공유와 독립적인 native pick/snap scene 식별·Rust process client를 연결했다.
M4a-2에서 표시 frame/worker·revision에 고정한 로컬 controller query와 종류별 취소·style drain을 연결했다.
M4a-3에서 기존 owner WebSocket에 표시 ACK/연결별 query와 안전한 결과 DTO를 연결했다.
M4a-4에서 브라우저 도형 선택/overlap 순환·modifier 선택·스냅 프로브와 stale 검사를 연결했다.
M4a-5에서 Rust 좌표·거리 계산과 두 점 수동 ruler/Shift·snap·미리보기·삭제를 연결했다.
M4a-6에서 Rust 선택 bbox 자동 gap과 수동/auto/CD 생성 순서·삭제·라벨 배치를 통합했다.
M4b-1에서 일반 layout의 exact clip CLI·private 출력 검증·스트리밍 원자 게시를 연결했다.
M4b-2에서 layout/jobdeck batch·mosaic 캡처와 report·kept tiles를 단일 Rust worker로 연결했다.
M4b-3에서 PNG pixels를 보존하는 flateyes metadata와 `fe-embed` Rust 보조 CLI를 연결했다.
M4b-4에서 기존 live 스타일·오류 marker/CD/legend를 유지하는 DRC PNG 캡처 CLI를 연결했다.
M4b-5에서 SVRF subset 전처리·규칙 그래프·scan/원자 sidecar 저장 CLI를 Rust로 이관했다.
M4c-1에서 별도 자원 예약·취소/reap·만료 descriptor 저장소를 갖는 관리형 exact clip 코어를 추가했다.
M4c-2에서 표시 receipt·명시 승인·중복 방지·취소와 owner HTTP chunk 다운로드를 연결했다.
M4c-3에서 현재 viewport의 준비→명시 승인·취소·파일 목록/다운로드 UI를 연결했다.
M4d-1에서 이미 표시한 픽셀의 PNG 복사/저장과 overlay 3상태 전환을 연결했다.
M4d-2에서 layerprops Rust codec과 GTK에 맞춘 첫 view의 레이어 가시성을 연결했다.
M4d-3에서 열린 세션 설정 Load/Save·bitmap/상속을 보존하는 native JSON과 원자적 적용을 연결했다.
M4d-4a에서 공유 설계 기본값의 읽기 전용 준비·충돌 검사·권한 보존·원자 게시 Rust 코어를 추가했다.
M4d-4b에서 launcher opt-in·owner 준비/명시 승인/취소/결과 API를 연결했다.
M4d-4c에서 공유 영향 preview·체크 승인·진행/취소·새로고침 후 동일 요청 확인 UI를 연결했다.
별도 승인 후 실제 Chrome의 합성 layout 신규 게시 클릭은 확인했다(M4 §20).
기존 파일 교체·취소/복구·현장 브라우저 수용은 별도이며 일반 Save는 계속 다운로드뿐이다.
M4e-1~4에서 DRC 주석/waive codec·관리형 게시, owner의 편집·미리보기·별도 승인 UI와
waive 저장 후 reader 갱신/조회 revision 장벽을 연결했다. M4e-5a는 저장된 주석의
목록 badge·이동 대상 본문용 읽기 전용 API/캐시다. M4e-5b에서 목록 배지와 마지막
ACK 이동 대상의 주석 overlay·서버 panel 상태 복원을 연결했다.
M4e-6a에서 주석/waive native snapshot export와 전체 waive import를 연결했다.
M4e-6b에서 owner 분할 업로드·전체 교체 준비·기존 승인 게시와 유계 artifact 다운로드를
연결했다. M4e-6c에서 전체 review 파일 선택·분할 전송/미리보기·별도 승인과
내보내기/다운로드 UI를 연결했다. Chrome 합성 내보내기 준비는 확인했으며, 실제 브라우저
업로드·review 게시·다운로드 파일 수용과 현장 검증은 후속이다.
M4f-1에서 `selfcheck`의 빌드 식별·실제 native 버전/handshake/종료 검사를 추가했다.
이 선행 단계에서는 전용 portable 조립을 후속 M4f-2로 구분했고 기존 GTK 패키지를 유지했다.
M4f-2에서 별도 offline packager·ELF 감사·원자적 비덮어쓰기 archive와 고지/체크섬을
연결했다([portable 안내](WEBUI_PORTABLE.ko.md)). Linux 실행·현장 수용 및 About UI는 별도다.
M4f-3a에서 읽기 전용 About/빌드 식별·글꼴 원문 고지를 연결했다.
M4f-3b에서 compiled catalogue로 고정된 portable 원본 고지의 유계 열람을 추가했다.
현장 수용·남은 조작 parity는 계속 열린 상태다([M4 §39](WEBUI_M4.ko.md)).
M4g-1에서 GTK 오른쪽 드래그 박스 확대/축소·방향 되돌림/얇은 박스와 취소를
Rust 좌표 계산과 웹 입력으로 연결했다([M4 §40](WEBUI_M4.ko.md)).
M4g-2에서 기존 색인의 depth 구조 미니맵과 동일 배율 클릭 이동을 연결했다.
180px 베이스를 재사용하며 pan 중 geometry 재렌더는 없다([M4 §41](WEBUI_M4.ko.md)).
M4g-3에서 `<`/`>` depth 상대 이동과 GTK의 DRC `,`/`.` 순회·`n` 주석·`w` waive
편집 진입을 연결했다. 게시 승인은 계속 별도다([M4 §42](WEBUI_M4.ko.md)).
M4g-4에서 q/End session에 취소 기본값의 종료 확인을 연결했다. 서버 종료 요청의
실패는 성공으로 표시하지 않고 승인 복구 기록을 남긴다([M4 §43](WEBUI_M4.ko.md)).
M4g-5a에서 잡덱 모드 전환의 카메라·가시성 이관을 Rust 코어에 준비했다. 실제 GTK
전환과 native PNG를 비교했다([M4 §44](WEBUI_M4.ko.md)). M4g-5b에서 같은 자원 예약의
순차 worker 교체·revision CAS·owner API와 모드 선택기/Ctrl+,를 연결했다.
선택 레벨과 카메라는 유지하고 모드별 기본 스타일은 다시 읽는다([M4 §45](WEBUI_M4.ko.md)).
M4g-6에서 CLI 초기 depth/frames/labels·폭 없는 goto·GTK fit 여백, 명시적
direct-final/baseline과 잡덱 초기 레벨 선택 대기를 연결했다. 색인 재시도 중 초기
옵션을 보존한다([M4 §46](WEBUI_M4.ko.md)). single-instance/빈 창 시작·남은 CLI 옵션은
후속이며 GTK 기본 launcher를 교체하지 않는다.
M4g-7a에서 로컬 launcher의 소유권 lock·동일 UID 통신·고유 요청/재전송 코어를
추가했다([M4 §47](WEBUI_M4.ko.md)). 아직 제품 CLI/열린 창에 연결하지 않은 기반이며,
동적 소스 등록과 게시 보호·실제 forwarding·`--multi`·빈 창 시작은 미완료다.
M4g-7b에서 trusted service 소스 추가/재사용과 기본값·DRC 게시의 공유 보호 목록을
연결했다([M4 §48](WEBUI_M4.ko.md)). 빈 service→등록→native 렌더를 검사했지만,
CLI forwarding/`--multi`와 브라우저의 빈 창 흐름은 아직 미완료다.
M4g-7c에서 유계 launcher 제안/owner 접수 API와 다중 소스 원자 등록,
동일 소스 캐시 보존·revision 검사 후 단일 worker 교체를 연결했다([M4 §49](WEBUI_M4.ko.md)).
이 단계의 gateway capability는 trusted attach 때만 켜진다.
M4g-7d에서 `floe2-web`의 인자 없는 빈 창/기존 창 전달과 `view --multi`, 비동기
등록·레벨 질문·브라우저 소비자를 연결했다([M4 §50](WEBUI_M4.ko.md)). 같은 파일의
캐시는 유지하며 불명확한 결과는 같은 요청만 확인/재시도한다. GTK 기본 실행기,
브라우저 파일 선택기·남은 CLI 옵션·현장 창 focus 수용은 별도다.
M4g-8에서 허가된 서버 폴더 handle 기반 파일 선택기를 연결했다([M4 §51](WEBUI_M4.ko.md)).
빈 창에서 폴더/이름 필터/128행 페이지를 탐색하고, 선택은 기존 원자 등록·열기 제안을
거친다. 로컬 파일 upload·임의 서버 경로·자동 색인은 아니다. GDS/gzip, GTK의 임의
home/path 탐색과 마지막 표시 설정 유지, 남은 CLI 옵션·현장 수용은 계속 구분한다.
나머지 내보내기와 브라우저 다운로드/현장 수용은 남아 있다([M4 기록](WEBUI_M4.ko.md)).
M4g-8b는 파일 선택·닫기 후 재열기의 창 표시 설정 유지, 잡덱 왕복 라벨 선호,
빈 창 baseline과 CLI 명시 설정의 구분을 서버 첫 프레임에 연결했다([M4 §52](WEBUI_M4.ko.md)).
M4g-9a는 미색인 열기 실패를 보존하고 명시 승인 후 색인→재열기를 하나의 서버
작업으로 실행한다([M4 §53](WEBUI_M4.ko.md)). M4g-9b는 원래 선택의 승인 창,
전송 전 승인 저장, 동일 요청 조회/명시 재시도와 새 뷰 연결을 추가했다
([M4 §54](WEBUI_M4.ko.md)). 파일 선택·미리보기·새로고침만으로 색인을 시작하지 않는다.
M4g-10은 파일명만 주는 실행을 동일 view 파서에 연결하고, 서버 카메라 문자열로
goto 입력을 복원한다. 입력 초안·지연 ACK·다른 파일 전환을 구분한다([M4 §55](WEBUI_M4.ko.md)).
전체 조작 parity와 M0/Firefox/ETX 현장 검증은 아직 완료되지 않았다.
M4g-11a는 GTK 레이어 다중 선택의 show/hide/toggle을 한 revision으로 적용하는
Rust/API를 추가한다([M4 §56](WEBUI_M4.ko.md)). 일반 접힘 그룹과 잡덱 그룹의
차이는 이관했지만 브라우저의 선택·접기 UI는 다음 단계이며 UI-03 전체 완료가 아니다.
M4g-11b는 실측 브랜치 `09be2ab`까지 16개 커밋을 정방향 합류하고 Rust CLI/웹
승인의 occupancy 기본 생성과 명시 opt-out을 맞춘다([M4 §57](WEBUI_M4.ko.md)).
M4g-31은 `45c9934`까지 추가 14개 커밋을 정방향 통합하고 로컬 전체 회귀를 통과했다.
occupancy 기본값은 최신 계약에 따라 **layout off / jobdeck on**으로 바뀐다.
새 캐시 이름·depth 점유·page frontier와 이관/검증 상태는
[두 번째 동기화 기록](WEBUI_JOBDECK_SYNC.ko.md)에 둔다. M4g-32는 사용자 결정대로
개명을 명시 Index/DRC Build에만 연결한다. 읽기/profile 비쓰기, 양쪽 이름 잠금,
실패/취소 후 개명 receipt의 계약과 검증 상태는 [별도 기록](WEBUI_CACHE_MIGRATION.ko.md)을 본다.
전체 배터리 PASS는 실제 브라우저·Linux 실행·현장 수용이나 첫 실행 지연 해결을 뜻하지 않는다.
M4g-33은 [native worktree 빌드 스탬프](WEBUI_BUILD_REVISION.ko.md)의 불필요 재빌드를
별도로 고친다. native 버전은 0.12.101이며 실제 브라우저/현장 수용을 추가로 닫지 않는다.
M4g-34~37은 실제 합성 Chrome 수용에서 발견한 DRC 교체의 중복 예약, waive 조회의
유휴 note cache 예약, 저장 영수증의 고정 대기 문구 및 native 회귀에서 발견한
DRC build 부모 경로 alias 오인을 수정한다([M4 §89~92](WEBUI_M4.ko.md#89-m4g-34--drc-교체-준비의-중복-예약-제거)).
단일 오류 note 수동/opt-in 저장·복원과 waive 수동 저장·복원/자동 해제는
[실제 수용 §10~10.2](WEBUI_BROWSER_ACCEPTANCE.ko.md#10-실제-chrome-reviewer-메모-저장복원과-waive-admission-결함)에
기록한다. 직접 실행한 새 서버에서 자동 해제의 UI 재조회도 확인했다. M4g-38은
saved-note 표시 뒤 교체의 추가 예약 실패를 수정하고, 실제 Chrome 교체·원래 ICE/
reviewer/SVRF 복원을 확인했다([수용 §10.3](WEBUI_BROWSER_ACCEPTANCE.ko.md#103-저장-메모-표시-후-drc-교체와-명시-재연결)).
M4g-39는 같은 유휴 cache로 인한 SVRF 교체 실패를 수정하고 실제 Chrome에서도
확인했다([M4 §94](WEBUI_M4.ko.md#94-m4g-39--저장-메모-표시-후-svrf-metadata-교체)).
M4g-40은 SVRF 선연결 후 reviewer 재연결의 중복 모델/예약을 제거하고, 세 grant의
경계 예산·입력 identity 회귀와 실제 Chrome을 확인했다
([M4 §95](WEBUI_M4.ko.md#95-m4g-40--svrf-선연결-후-reviewer-재연결의-중복-모델-제거)).
다중 선택·충돌/복구·나머지 조작은 남으며 전체 DRC/G4 수용으로 계산하지 않는다.
M4g-41은 실측 `c817117`까지 추가14커밋을 정방향 통합하고 기본off OVR 대표 점
생성/가산·취소·진단을 Rust CLI/관리형 웹에 연결했다. 전체 회귀와 최종 선택 파서
보완을 통과했으며 [세 번째 동기화 기록](WEBUI_JOBDECK_SYNC.ko.md#세-번째-통합--ovr-대표-점)에
범위/근거를 둔다. native0.12.155, CLI 공개114개/native parser180회다.
실제 다중 미리보기는 저장 없는 범위이며 GTK readiness 역시 G1 측정이 아니다.
M4g-42는 note/waive 편집 snapshot과 saved-note 표시의 경합을 막고, 마지막
snapshot 반환 뒤 읽기를 한 번 재개한다. JS 회귀와 직접 재시작한 합성 Chrome에서
읽기/취소 복구를 확인했다([M4 §99](WEBUI_M4.ko.md#99-m4g-42--편집-snapshot-해제-후-saved-note-표시-복구)).
새 저장·충돌/결과 불명 복구 수용은 별개로 남는다.
M4g-43은 실제 bitmap 검증에서 드러난 macOS 오버레이 스크롤바/스타일 버튼 겹침을
수정했다([M4 §100](WEBUI_M4.ko.md#100-m4g-43--실제-bitmap-검증과-스타일-버튼-hit-area)).
별도 종료 검사는 서버 exit0 뒤 브라우저의 이전 픽셀/Live 표시 잔류를 발견했으며
M4g-44에서 버퍼·오버레이·상태줄을 즉시 정리하고 실제 Chrome/서버exit0으로
수정을 확인했다([M4 §101](WEBUI_M4.ko.md#101-m4g-44--명시-종료-후-화면상태-잔류-수정)).
종료 응답 불명은 회귀로 검증했으며 종료/복구 전체 실제 수용 PASS는 아니다.
추가로 사전 상태 조회 대기 중 종료했을 때 늦은 Index POST가 나가던 경합을 차단했다.
M4g-45는 보존된 합성 Chrome에서 내장 두벌식 조합·삭제·선택 교체·emoji 뒤 커서와
초안 폐기를 저장 없이 확인했다. OS IME/현장 Firefox 수용으로 확대하지 않는다
([브라우저 §9.1](WEBUI_BROWSER_ACCEPTANCE.ko.md#91-내장-두벌식--실제-키커서초안-폐기)).
M4g-46은 시작 설정의 늦은 응답이 종료 안내를 다시 레벨 선택 안내로 덮는 경합을
재현·차단했다. 시작 중5개 조회와 종료/조회 성공·실패20조합을 UI 게이트로 고정한다.
M4g-47은 연결 복구 후 합성 사각형의 꼭짓점/변 스냅과18µm 폭/2µm 간격을 확인했다.
probe 문구 소거 시 룰러 버튼이 움직이는 CSS를 수정했다. 수정본의 인증된 클릭 재검증은
비공개 시작 파일에 대한 브라우저 URL 정책 차단으로 남았다([M4 §104](WEBUI_M4.ko.md#104-m4g-47--실제-스냅-측정과-움직이는-룰러-버튼)).
M4g-48은 BFCache 복원 도중 종료/다시 숨김/새 복원이 끼면 늦은 응답이 하위 기능이나
소켓을 다시 시작하던 경합을 복원 세대로 차단했다. 늦은401과 현재401을 구분하며
회귀58조합을 추가했다. 실제 브라우저 BFCache 전체 수용은 아니다
([M4 §105](WEBUI_M4.ko.md#105-m4g-48--bfcache-복원-체인의-세대-분리)).
M4g-49는 런처 내부 사전 조회가 종료 뒤 새 POST를 보내는 문제와 picker의 늦은
root 조회가 닫힌 창을 다시 여는 문제를 재현·수정했다. 이미 제출한 receipt/명시
동일 재시도는 보존하며, 단위 및 app 연결27조합으로 검증했다. 초기화/실제 브라우저
수용은 별도다([M4 §106](WEBUI_M4.ko.md#106-m4g-49--런처파일-선택기-내부의-지연-사전-조회)).
M4g-50은 초기화가 끝나기 전 BFCache 복귀를 별도 보호한다. 한 초기화 체인과 페이지
세대 검사로 숨김 뒤 추가 요청을 막고 bootstrap/자동 open을 중복 제출하지 않는다.
레벨 승인·결과 불명·연속 복귀86조합은 합성 회귀이며 실제 브라우저 BFCache 수용은
남긴다([M4 §107](WEBUI_M4.ko.md#107-m4g-50--초기화-중-bfcache-중단복귀와-자동-열기-보호)).
M4g-51은 기존 승인 Chrome 합성 탭에서 두 box의 Shift/Command 다중 선택,
자동 bbox gap2µm, 축 정렬20µm/자유각20.2237µm와 Undo를 원본 좌표·화면으로
확인했다. 이전 실행 파일의 관측이며 최신 CSS/BFCache 및 UI-04 전체 수용은 아니다
([브라우저 §5.4](WEBUI_BROWSER_ACCEPTANCE.ko.md#54-실제-다중-선택자동-bbox-gapshift-자유각)).
M4g-52는 같은 합성 Chrome에서 bbox/수동/CD/수동4개 기록의 역순 Undo와
CD 전용 삭제의 수동 기록 보존을 확인했다. 새 저장/제품 수정은 없으며 최신 빌드
전체 수용과 구분한다([브라우저 §5.5](WEBUI_BROWSER_ACCEPTANCE.ko.md#55-실제-cd수동bbox-간격-혼합-undo)).
M4g-11c는 접힌 그룹을 제외한 페이지·범위 선택을 Rust 읽기 전용 API로 제공한다
([M4 §58](WEBUI_M4.ko.md)). 브라우저의 다중 선택·접기 UI 연결은 다음 단계다.
M4g-11d에서 Ctrl/Shift 선택·접기/펼치기·페이지 간 범위와 선택 행의 일괄 가시성을
웹에 연결했다([M4 §59](WEBUI_M4.ko.md)). 로컬 동작 검증은 완료했으며 다중 스타일과
실제 브라우저 입력·표시 수용은 남아 있다.
M4g-11e에서 선택 행의 색상·채움·선폭/증감을 한 CAS로 적용하는 Rust/API와 웹
편집기를 연결했다([M4 §60](WEBUI_M4.ko.md)). GTK와 잡덱 어댑터를 함께 대조해
색 전파와 채움/선폭 상속의 차이를 보존한다. UI-03 전체 수용은 아직 아니다.
M4g-11f는 이름 있는49색/20채움 팔레트와 단일 행 편집을 같은 원자적 스타일 경로로
연결한다([M4 §61](WEBUI_M4.ko.md)). 개발용 bitmap 슬롯 편집은 별도 미이관이다.
상세 범위는 [M1b 기록](WEBUI_M1B.ko.md)과 [M2 기록](WEBUI_M2.ko.md). 여기의 M0~M5는 **웹 전환 단계**이며
jobdeck/occupancy의 같은 이름 단계와 별개다. 개발 기준과 합류 규칙은 §11.
로컬 산출물: [기능/CLI 대조표](WEBUI_M0.ko.md),
[Rust 서비스/API 초안](WEBUI_SERVICE_API.ko.md).

## 현재 진행도와 커밋 보고

M2b-1 [초대·인증 코어](WEBUI_SHARING_GRANTS.ko.md)에 이어 M2b-2는
[읽기 전용 follow 프레임 전송](WEBUI_SHARING_FOLLOW.ko.md)을 기본 off로 연결한다.
고정 view/revision/layer scope, PNG/raw·margin 재사용, 별도 guest credit·폐기를 적용한다.
M2b-3a는 [독립 explore 렌더/표시 상태](WEBUI_SHARING_EXPLORE.ko.md)를 같은 자원 관리자에
연결한다. 별도 worker·범위 제한 표시 변경·60초 idle 회수/재접속 기반이다.
M2b-3b1의 [scoped query/룰러](WEBUI_SHARING_QUERY.ko.md)는 자기 displayed receipt와
가시 레이어에만 허용한다. M2b-3b2는 [명시 DRC 공유·독립 panel/선택/읽기](WEBUI_SHARING_DRC.ko.md)를
연결하며 owner+두 guest의 서로 다른 native 오류 탐색을 검증한다. 아직 guest 화면 UI는 아니다.
M2b-4a는 [로컬 초대·전용 guest 화면](WEBUI_SHARING_UI.ko.md)을 `--local-sharing` 아래 연결한다.
follow 프레임과 독립 탐색 조작은 연결했지만 DRC/layer/query/룰러 UI와 실제 수용은 남는다.
M2b-4b1은 [별도 DRC 공개 승인·개인 목록/선택·윤곽/이동 UI](WEBUI_SHARING_DRC_UI.ko.md)를
연결한다. scope 제한 layer/query/룰러·pointer, DRC 순회/마커/CD 조작과 실제 수용은 남는다.
M2b-4b2는 [scope 제한 레이어 UI](WEBUI_SHARING_LAYERS.ko.md)를 연결한다. 승인 범위로
목록 필터·부분 그룹/hidden 자식·개인 접힘과 Explore 가시성만이며 query/룰러·pointer,
DRC 순회/마커/CD와 실제 수용은 남는다. 최종 검증 상태는 해당 기록을 따른다.
M2b-4b3는 [표시 receipt 기반 query/룰러·pointer](WEBUI_SHARING_QUERY_UI.ko.md)를
연결한다. 실제 표시된 foreground/전체 margin만 조회하며 Follow·jobdeck capability를
보존한다. 다음 구현은 DRC 순회/마커/box/CD이고, 실제 브라우저·현장 수용은 남는다.
M2b-4b4는 [유계 DRC 순회·마커/박스 선택](WEBUI_SHARING_DRC_NAV.ko.md)을 연결한다.
순회는 선택 집합·카메라를 바꾸지 않는다. CD/jump-mode 연결과 실제 수용은 남는다.
후속 M2b-4b5는 [이동 확정·CD·자동 순회](WEBUI_SHARING_DRC_CD.ko.md)를 연결한다.
ACK와 같은 revision의 snapshot 이후에만 CD 대상을 바꾸고, 확인된 이동 뒤의 순회는
auto-fit/zoom lock을 따른다. 단순 마커 선택은 이동하지 않는다. 개인 복원·삭제 경합과
불명확한 응답의 무재전송을 검증하며 실제 브라우저/원격 수용은 여전히 별도다.
M2b-4c는 [HTTP/wire 권한 목록·수용 근거 대조](WEBUI_SHARING_ACCEPTANCE.ko.md)를
연결한다. API 추가/재연결을 감지하고 합성 서버의 교차 인증 거부를 검사한다.
SH-08 실제 브라우저와 SH-10 원격 수용을 로컬 green으로 닫지 않는다.
2026-09-17 사용자 결정으로 **원격 단계는 보류**한다. 제안했던 HTTPS 프록시 뒤
loopback gateway의 설계·코드·합성 테스트도 착수하지 않는다. SH-10/G3를 완료 처리하거나
삭제하지 않고 보류 상태로 남기며, 로컬 공유 범위와 현재 통합 검증은 계속한다.
`shares=false`는 유지하고 `delivery`는 follow=`follow_frames`, explore=`explore_frames`다.
전송 기반을 전체 공유 완료로 계산하지 않는다. 원격 공개·노트 본문·파일 탐색/쓰기는 열지 않는다.

M4g-30 사용자 결정: Rust 제품의 GTK 진단은 `displaytest [PNG]`로 대체 확정.
GTK `gtktest`는 비교 패키지에만 남기고 Rust alias는 추가하지 않는다. 또한 기본 off인
opt-in 로컬 공유 구현을 승인했다. follow/explore를 모두 유지하며 합성·loopback에서
검증하고 원격 공개·노트 본문·서버 export·게스트 파일 탐색/쓰기는 열지 않는다.
이 결정은 공유 구현 완료나 실제 브라우저·현장 수용을 뜻하지 않는다.

M2b-0은 [공유 권한/수명/자원 경계](WEBUI_M2_SHARING.ko.md)를 현재 코드로 대조한다.
guest를 owner 인증에 붙이는 것만으로는 부족하며, 단일 세션·현재 view·전체 로그아웃과
process-local admission을 분리해야 한다. follow/explore 및 원격 G3를 요구사항에서
빼지 않았다. 위 범위의 로컬 구현 승인은 받았고 원격/운영 정책은 별도다.
M2b-0 감사 자체는 기능·listener를 바꾸지 않았다.

M4g-29는 `displaytest`에 APNG의 IDAT 정적 기본 이미지를 연결한다. 원본의 모든
chunk CRC/길이/상한을 검증한 뒤 animation chunk만 메모리에서 제거하며 원본 파일과
나머지 metadata는 보존한다. 애니메이션 재생·GTK 보간·실제 화면 수용은 아니다.
[표시 진단 계약](WEBUI_DISPLAY_DIAGNOSTICS.ko.md)과 CLI/HTTP·선택 GdkPixbuf 대조를
추가했다. GTK 진단의 제품 경계는 M4g-30에서 확정했으며 실제 브라우저·Linux·현장 수용은 남는다.

M4g-28은 양수 `--stream-kb`의 기존 Rust 호환을 연결한다. 크기를 KB 예산으로
새로 사용하지 않고 환경 page-round를 따르며, 명시 stream의 독립 workspace와
최종 값 기준 순서/음수/off·baseline 충돌 검사를 보존한다. 원본 정책380사례와
실제 startup/IPC gate로 대조한다. 실제 브라우저·Linux·
현장 수용, M2 공유/원격 및 조건부 M5는 별도로 남는다.

M4g-27의 [CLI 원본 재대조](WEBUI_G4_CLI.ko.md)는10개 명령94개와 보조 PNG16개를
현재 argparse/native parser로 고정한다. 사용자 결정에 따라 `--refinement on`은
기존 환경 page-round를 따르게 복원했다. 기본 실질 off·명시 off/stream0/baseline
우선은 유지하며 새 적응형 렌더 정책은 만들지 않는다. 상태줄은 실제 round/final을
표시한다. 양수 stream은 M4g-28, APNG 정적 fallback은 M4g-29에서 연결했다. 아래 전체 수용은
남는다. 파서175회 통과를 파일/픽셀·브라우저 기능 전체의 완료로 계산하지 않는다.

M4g-24b의 레이아웃 유지 DRC 열기에 이어 M4g-24c는 `Reconnect launcher reviewer…`를
연결했다([M4 §81](WEBUI_M4.ko.md)). 사용자 결정대로 런처에 고정한 reviewer·권한만
새 DRC에 명시적으로 재연결하며, 브라우저에서 reviewer나 권한을 추가하지 않는다.
기존 저장/전송 순번·receipt를 보존하고 자동 저장 동의는 초기화한다. G4-MENU-01은
이 권한 정책 아래 로컬 구현·회귀를 연결했다. 실제 브라우저 수용 완료는 아니다.

M4g-25는 같은 picker에서 `Load SVRF metadata…`를 연결한다([M4 §82](WEBUI_M4.ko.md)).
열린 DRC reader를 재사용해 geometry를 재파싱하지 않고, metadata와 query revision을
검증 후 원자 교체한다. 실패 시 기존 metadata/revision, 성공 시 layout/reviewer/저장
receipt를 보존한다. 이전 type/filter/selection·미승인 preview는 무효화한다.

M4g-26a는 레벨 재선택의 선행 충돌을 제거한다([M4 §83](WEBUI_M4.ko.md)). 덱의
모든 캐시에 쓰기 잠금을 잡던 managed index를 읽기 예약→실제 계획된 목적지만
원자 쓰기 전환으로 바꿨다. 열린 레벨의 캐시를 재사용하며 다른 선택 레벨을 색인할
수 있고, 열린 캐시의 force/occupancy 수정은 여전히 첫 쓰기 전에 거부한다.
이 선행 단계만으로는 레벨 재선택 명령을 완료로 세지 않았다. CLI/일반 레이아웃의
기존 인덱싱 잠금은 바꾸지 않는다.

M4g-26b는 `Apply levels · keep view`와 `reselect_levels`를 연결한다([M4 §84](WEBUI_M4.ko.md)).
현재 source/mode·서버 camera/revision에 새 선택을 묶고 첫 프레임부터 같은 카메라로
그린다. 미색인 소스의 별도 승인·재시도에도 원래 anchor를 유지하며 그 사이
pan/resize는 오래된 교체를 거부한다. 동일 선택은 no-op이고 준비 실패·commit 전 취소는
기존 화면을 보존한다. commit 후 native worker 실패의 자동 rollback은 추가하지 않는다.
새 레벨의 layer defaults를 다시 읽으며 실제 브라우저 수용은 별도다.

2026-09-16 M4g-23 [GTK 메뉴 재대조](WEBUI_G4_MENU.ko.md): 실행 중 DRC 파일 열기/
교체, SVRF metadata 교체, 카메라 유지 jobdeck 로드 레벨 재선택의 누락을 확인했다.
M4g-24b/c·25·26b에서 세 항목의 로컬 구현·게이트를 각각 연결했다. 초기 CLI 등록·
일반 Open·Mode 전환만으로 대체하지 않았다. 메뉴 inventory는 이제 linked39/open0이며
`--require-complete`를 배터리에 넣었다. 이는 연결 목록 검사이지 실제 브라우저/G4
수용이나 전체 기능 감사 완료가 아니다. 진단/무효 CLI 최종 재대조는 계속 남는다.

2026-09-16, M4g-22 기준. **로컬 Rust/web 대체 기능은 후반부지만 전체 계획의
완료 직전은 아니다.** 아래는 구현과 수용을 분리한 현재 상태이며, 위의 순차 기록에
있는 과거 시점의 “미완료” 설명보다 우선한다. 세부 커밋 수는 작업량 비중이 아니므로
이를 백분율로 환산하지 않는다. 이후 커밋 보고에도 완료 범위·남은 구현·현장 수용을
함께 명시한다(사용자 요청).

| 계획 | 현재 상태 | 닫기 위해 남은 범위 |
|---|---|---|
| M0 | 로컬 기능/API·의존성 조사 진행, 현장 감사 보류 | 실제 Firefox/ETX 환경·운영 정책 확인 |
| M1 | Rust CLI/서비스·로컬 웹 뷰어·margin 구현 | 잔여 열기/CLI parity, G1 지연·pacing 및 최종 G4 수용 |
| M2 | 로컬 DRC·공유 인증/권한·follow·독립 explore 렌더·명시 DRC API·CLI opt-in/초대·guest DRC 목록/선택/순회/마커/box/CD/jump-mode·layer/query/룰러/pointer UI·SH 권한 inventory/로컬 근거 대조 | 실제 브라우저 SH-08·원격 배포 SH-10 및 수용 (`shares=false`, loopback-only) |
| M3 | 현장 실행 불가로 보류 | TeeBox Firefox-in-ETX와 GTK 비교, G2 판정 |
| M4 | 주요 조작·내보내기·설정·Rust portable 구현 | 아래 로컬 잔여와 Linux/브라우저 수용·G4 전체 감사, GTK 은퇴 판정 |
| M5 | world-tile 조건부 보류 | F2R-03c/10 전제와 실측으로 착수 판단. 미구현을 완료로 세지 않음 |

미색인 파일의 **동의→색인→자동 재열기 UI**는 M4g-9b에서, bare FILE와 goto 복원은
M4g-10에서 연결했다. 조작 대조에서 빠져 있던 레이어 일괄 변경의 Rust/API는
M4g-11a에서 추가했다. M4g-11b는 §11에 따라 `feature/jobdeck`의 로컬 `09be2ab`까지
합류하고 occupancy 기본 생성·레이어별 depth·희소 sub-cut 표시를 웹 경로에 반영한다.
M4g-11c의 접기·페이지 간 범위 서버 조회는 M4g-11d에서 웹 선택·접기 UI와 일괄
가시성에 연결했다. GTK 선택 규칙7,776개·목록32개·가시성12,096개 및 실제 app.js
연결을 로컬 검증했다. 실제 브라우저 입력/표시 수용이나 UI-03 전체 완료는 아니다.
M4g-11e에서 다중 스타일의 선택·접힘·상속·선폭 증감과 단일 제출을 연결했다.
M4g-11f에서 이름 있는 색/채움 프리셋과 행 편집의 접힘·상속·오래된 대상 거부를
연결했다. M4g-12에서 잔여 view 옵션·입력 형식 대조를 마치고 `--stream-kb 0`과
숫자 전용 `--render-debug`를 연결했다([M4 §62](WEBUI_M4.ko.md)). 실제 Rust에서
쓰지 않던 hairline/thin-um/stream-target/lod 옵션은 새 의미를 만들지 않고 이유를
명시해 거부한다. GDS/gzip은 기존 native 제품도 header 인식만 가능했던 한계이며
웹 회귀와 구분한다. 진단/표시 태그·progressive의 이관/제품 경계 결정은 남아 있다.
GTK 개발용 bitmap 슬롯 편집은 선택 레이어의 hex-row 입력으로 대체 완료했다고
세지 않는다. upstream occupancy M5의 실측 완료는 여기의
웹 M5(world-tile) 완료나 웹 성능 수용을 뜻하지 않는다.
GTK의 bitmap 슬롯 편집은 `FLOE_FILL_EDIT` 개발 옵션 뒤에 있으며 슬롯을 참조하는
모든 레이어에 적용된다. Rust의 값 기반 fill 지정만으로는 같은 슬롯 의미가 아니므로
별도 이관 대상으로 추적했다([M4 §61](WEBUI_M4.ko.md)). M4g-16a에서
[슬롯 참조·저장 계약](WEBUI_BITMAP_SLOTS.ko.md)과 GTK 원본324편집/80메뉴 오라클을
고정했다. M4g-16b는 Rust 슬롯 모델·Native JSON v2 보존과 GTK 직접 대조를 연결한다
([M4 §67](WEBUI_M4.ko.md)). M4g-16c는 별도 세션 슬롯 조회와 opt-in/CAS 편집 API,
wire 참조 할당을 연결한다([M4 §68](WEBUI_M4.ko.md)). M4g-16d는 웹 프리셋의 참조
할당·세션 표 캐시와16×16 초안/Apply/Cancel/Reset·키보드/드래그를 연결한다
([M4 §69](WEBUI_M4.ko.md)). GTK324편집 대조와 DOM/client gate는 실제 브라우저
수용의 대체가 아니므로 UI-03 전체 완료로 세지 않는다.
실제 브라우저의 저장·복구·입력 수용, Python-free Linux 실행, G1/G4 전체 검증도
남아 있다. M2 공유와 M3 현장, M5 조건부 작업을 이 로컬 마감과 혼동하지 않는다.
M4g-17a는 [합성 표시 진단](WEBUI_DISPLAY_DIAGNOSTICS.ko.md)을 About에 연결한다.
GTK 색 막대·Rust PNG/raw·웹 crop/overlay를 대조하되 실제 화면 수용과 구분한다.
M4g-17b는 같은 검사를 독립 `displaytest` 명령으로 연결하며 index/renderd/레이아웃
없이 기존 private Firefox/auth 수명주기를 사용한다. M4g-17c는 CLI가 고정한 정적 PNG
하나의 읽기/인증 전송/360×160 표시를 연결한다. 브라우저 보간과 GTK의 픽셀 동일성을
주장하지 않는다. APNG 정적 기본 이미지는 M4g-29에서 연결했고 M4g-30에서 Rust 제품은
`displaytest`로 대체하기로 확정했다. GTK 위젯 진단은 비교 패키지에만 남으며 실제 화면 수용은 아직 남는다
([M4 §70~72](WEBUI_M4.ko.md)). 승인된 브라우저 `--dump`는 M4g-22에서 연결했다.
M4g-18은 IPC owner 종료와 자식 fork→exec가 겹쳐 잠금이 남는 조건을 재현하고,
소유자의 명시적 unlock·fork 복사본 보호와 회귀 검사를 추가한다([M4 §73](WEBUI_M4.ko.md)).
앞선 단발 검사 실패와의 인과는 별도 미확정이며 이를 전체 G4 완료로 세지 않는다.
M4g-19는 GTK 대비 과도했던 휠 확대율·0 delta 축소·렌더 대기 중 입력 누적을
수정한다. GTK 원본 이벤트 정책과 웹 표시 receipt 경계를 대조하며 물리 휠 감도/
G1·현장 수용은 별도다([M4 §74](WEBUI_M4.ko.md)).
M4g-20은 사용자가 승인한 pack/reviewer 유도 legacy 임시 note/waive 읽기를
연결한다. 인접 파일 우선·고정 경로·읽기 전용 저장소를 사용하며 임시 디렉터리를
browse/쓰기 root로 허용하지 않는다([M4 §75](WEBUI_M4.ko.md)).
M4g-21은 명시 `--floe-reviewer`의 ASCII 입력에서 현재 인접 ICE를 선택하고,
캐시가 없거나 stale/corrupt이면 sidecar 없는 ASCII로 돌아감을 화면에 표시한다.
암묵적 색인·쓰기 권한·browse root 확장은 없다([M4 §76](WEBUI_M4.ko.md)).
M4g-22는 `--dump`/명시 UI 토글로 최근 수신 프레임과 합성 화면을 각1장 보관하고
명시적으로 PNG를 다운로드한다. 서버 dump writer·이미지 storage·새 경로 API는
없고 캡처 비용/메모리와 실제 화면 수용을 구분한다([M4 §77](WEBUI_M4.ko.md)).
G4의 [잔여 감사](WEBUI_G4_AUDIT.ko.md)를 시작했다. M4g-13에서 GTK 두벌식 한글
입력기의 누락을 이관했다([M4 §63](WEBUI_M4.ko.md)). M4g-14에서 사용자가 선택한
reviewer별 확정 시 자동 저장 opt-in을 연결했다([M4 §64](WEBUI_M4.ko.md)).
M4g-15a는 명시 ICE와 인접 reviewer 파일의 읽기 선택을 저장 권한 없이 연결한다
([M4 §65](WEBUI_M4.ko.md)). legacy 임시 파일 읽기는 M4g-20, 명시 reviewer의 ASCII/
현재 ICE 선택은 M4g-21에서 연결했다. ambient reviewer 자동 선택은 하지 않으며
개발 도구/CLI 경계도 별도로 남는다.
전체 목록 대조를 계속해 실행 경로가 없는 항목과 제품 경계 결정을 분리한다.
단위/API/DOM gate를 실제 브라우저 수용으로
계산하지 않으며 위 미지원 옵션의 안내 추가를 기능 이관 완료라고 세지 않는다.
인덱스 hot reload/revision 수명주기는 §4.1대로 별도 설계이며 지원 완료가 아니다.

## 0. 결정 로그 (사용자 확정 사항)

- 2026-09-16: 선택한 DRC pack과 reviewer 이름에서 **정확히 유도되는 legacy
  임시 note/waive sidecar만 읽기**를 허용한다. 임의 경로 입력·파일 목록 노출·쓰기
  권한은 추가하지 않는다. 승인 완료와 웹 연결 구현 완료는 구별한다.
- 2026-09-16: `--dump` 대체는 **브라우저에 최근 프레임·합성 화면을 보관하고
  명시적 다운로드**를 제공한다. 서버 `/tmp` 자동 덮어쓰기는 추가하지 않는다.
  M4g-22에서 bounded 보관·UI를 연결했다. 실제 브라우저 수용은 별도다.
- 2026-09-16: DRC는 reviewer별 **확정 시 자동 저장 opt-in**을 추가한다.
  노트 편집 중/IME 입력 중에는 저장하지 않고 확정된 변경만 대상으로 한다.
  기본 off·기존 충돌/권한/receipt 유지, 세부 범위는 [G4 감사 §2](WEBUI_G4_AUDIT.ko.md).
- 2026-09-13: 현재 TeeBox 현장에서 audit/Firefox·ETX 실행이 불가능하다.
  M0 현장 감사와 G2는 **보류**로 두고 로컬 구현·회귀 검증은 계속한다.
  로컬 Chrome 검증을 현장 PASS로 대신하지 않으며 GTK launcher는 유지한다.
- 2026-08-28: 궁극 목표는 서버-클라이언트 모델. 당장은 데스크톱 앱
  배포가 필요하다.
- 2026-08-28: 주 작업자 흐름은 **외부망 portal에서 작업 선택 →
  폐쇄망 TeeBox가 TeeBox 계정으로 앱 실행 → 사용자 Exceed
  TurboX(ETX) 세션에 표시** — 이 흐름은 유지되어야 한다. TeeBox가
  사용자 계정/사용자 머신에서 앱을 실행하는 방법은 만들지 않는다.
- 2026-08-28: 브라우저 직접 사용은 주 작업이라기보다 **DRC 결과를
  공유받는 동료의 뷰어** 역할이 유력하다(링크 접근은 자연스럽고
  계획된 흐름).
- 2026-08-28: 폐쇄망에 Firefox는 있다(버전은 감사 필요, §7).
- 2026-08-29: **데스크톱 앱도 서버-클라이언트 패키징**으로 만든다 —
  같은 스택을 한 머신에 묶은 형태.
- 2026-08-29: **floe(KLayout backend)에도 적용한다** — gateway 경계를
  renderd wire가 아니라 GUI-중립 worker 계약(`make_render_worker`
  job/result)에 두어 두 제품이 같은 웹 셸을 공유한다(§3.1a). rollback
  스토리(FLOE_PRODUCT 전환)가 웹 셸에서도 유지된다.
- 2026-09-02: **T2의 raw RGBA payload가 제품 GTK 경로에 선구현됨** —
  F2R-13(FLOE2_OPTIMIZATION §3.16/§F2R-13, 0.12.26): renderd
  `frame_format=raw`가 `FLOERAW1`(magic+u32le w/h+packed RGBA)를
  기존 원자적 publish 계약으로 게시하고 GTK가 무디코드 표시. T2
  gateway는 이 payload를 재인코드 없이 스트리밍하면 된다. 별개로
  T3의 전제였던 F2R-10 world-tile은 **조건부 보류**(fill 위상이
  device-anchored라 byte-exact tile 재사용은 F2R-03c 1bpp plane
  선행 — §3.16 판정).
- 2026-09-08: **브랜치 승격 — floe2가 유일한 제품 라인.** 웹 셸은
  **floe2에만 적용**한다. 2026-08-29의 "floe(KLayout backend)에도
  적용" 결정은 철회하고, §3.1a의 두 제품 동시 지원은 요구사항에서
  뺀다(worker 계약 경계 자체는 GUI 중립 인터페이스로서 유지 — KLayout
  worker는 개발 선행 검증용으로만 존재). floe는 `floe-legacy`에 동결.
- 2026-09-06: 설계 리뷰(HIGH 5·MEDIUM 1 + 문구) 반영. 뷰 세션 모델
  (§4), 프레임 봉투·취소·backpressure·재접속 계약(§5), 보안 기본값
  (§8), Firefox 격리·`--kiosk` 하한(§3.3/§7, M0 신설), GTK의 margin
  계약을 M1부터 이관(§6), 표시 정확도 계약(§3.2), T0 경계 정정(§5),
  Rust gateway 이관 조건부화(§3.1a), Electron·라이선스 문구 완화.
- 2026-09-05 이후 GTK 셸에 생긴 계약(웹 셸이 그대로 옮겨야 할 것):
  배경 margin prefetch와 착지 프레임 보관·crop(F2R-17/21, 라벨 포함,
  `labels_truncated`면 crop 제외), 16px fill 위상에 스냅한 커서 pan,
  착지 margin을 새 strip의 표시 base로 blit, renderd 질의 스레드
  (F2R-22: pick/snap이 렌더와 독립 — gateway가 질의를 병렬로 보낼 수
  있음).
- 2026-09-12: **Python이 맡은 제품 실행 기능도 Rust로 이관**한다.
  Python gateway 선행·Rust gateway 조건부 이관안을 폐기하고 처음부터
  Rust gateway/CLI/서비스로 개발한다. 완료 제품의 실행에는 Python,
  PyGObject, Python 어댑터가 필요 없어야 한다. GTK 코드를 Rust GTK로
  번역하지 않고 표시·입력은 웹 UI로 대체한다. HTML/Canvas UI 계획은
  유지하며 구체적인 프론트엔드 언어·도구 선택은 §10에서 구분한다.
- 2026-09-12: **jobdeck 실측과 웹 전환을 브랜치로 분리**한다.
  `feature/jobdeck`은 실측·수정용으로 유지하고, 해당 브랜치의
  `6c33a48`(LOD 전달 머지 포함)에서 `feature/webui`를 생성한다.
  분기했다고 jobdeck/occupancy의 현장 성능·화질 검증을 완료로 보지 않는다.

## 1. 목표와 비목표

목표:

1. HTML/canvas 기반 뷰어 UI 하나로 세 배포형을 커버한다(§2).
2. Rust 렌더·플랜 성능 자산(F2R 계열)은 그대로 재사용한다. decode/raster
   코어 재작성은 하지 않는다. Python의 제어·데이터 기능 이관과 웹 표시
   비용은 각각 검증하며, 언어 전환만으로 렌더 시간이 줄어든다고 가정하지 않는다.
3. 서버 세션 설계로 "브라우저 리프레시 = 상태 소실" 위험을 제거한다.
4. F2R-10(world-tile) / F2R-11(streaming)과 합류 가능한 전송 계층을
   설계한다 — 클라이언트 tile 합성이 world-tile LRU의 자연스러운
   구현처가 된다.
5. CLI·서버·배포 실행 경로에서 Python 의존성을 없앤다. jobdeck, DRC,
   설정·캐시·인덱싱 제어를 포함하며, 웹 UI만 바꾸고 Python 서비스를
   뒤에 남긴 상태는 완료로 보지 않는다(이관 목록 §3.1b).

비목표:

- GTK 셸의 즉시 대체. GTK는 parity + ETX 게이트(§6) 통과 전까지 주
  작업자용으로 병존한다(worker job/result 계약이 GUI 중립이라 가능).
- Electron을 TeeBox에서 실행해 X/ETX로 쏘는 형태는 **현장 검증 전
  제외**. 원격 디스플레이에서 Chromium 합성 비용이 크다는 우려는
  해당 ETX 구성에서 실측되지 않았고, Firefox도 배포 B에서는 결국
  ETX 화면 전송을 거치므로 기술적 단정이 아니라 우선순위 결정이다.
- 초기 단계의 편집·계측 고급 기능 parity. M1~M2는 읽기 중심이다.
- 동결된 floe/KLayout·legacy indexer·폐기된 coverage 코드의 Rust 복제.
  현재 floe2가 제공하는 기능 계약을 기준으로 이관한다.
- 이 브랜치 생성 시점의 제품 코드 일괄 삭제. 기존 GTK/Python은 단계별
  비교 기준으로 남기되 최종 웹 제품의 실행·배포 의존성과 분리한다.
  개발용 Python 오라클/생성기까지 없앨지는 별도 범위 결정(§10).

## 2. 아키텍처: 한 스택, 세 배포형

```
[공통 스택]   Rust CLI / launcher
                 │
             Rust gateway ──[Rust worker client / renderd wire]── floe-renderd
                 │     └── Rust 공통 서비스(jobdeck·DRC·캐시·인덱싱 제어)
                 │ 정적 UI 서빙 + WS + 뷰 세션/토큰
                 ▼
             HTML UI (canvas 2D; Python 런타임 없음)

배포 A  데스크톱 패키징: launcher가 gatewayd+renderd를 함께 기동,
        UI는 로컬 브라우저(firefox --kiosk)로 loopback 접속.
배포 B  주 작업자(ETX): TeeBox 계정이 A와 동일 구성을 기동하되
        firefox의 DISPLAY를 ETX로 지정. portal→TeeBox 실행 흐름이
        한 글자도 안 바뀐다(§0). B는 A의 특수형이다.
배포 C  동료/원격 뷰어: gatewayd만 TeeBox에서 서빙, 사용자 자신의
        브라우저가 네트워크로 접속. 픽셀은 로컬에서 그려진다.
```

- B는 A의 실행 구성을 재사용하되 Firefox 프로필 격리·ETX 성능은 별도
  검증한다. DISPLAY 상속만으로 G2 통과를 보장하지 않는다.
- C는 같은 서비스 계약을 쓰되 TLS·게스트 권한·전송 상한·다중 사용자
  자원 정책이 추가된다. 단순한 바인딩 주소 변경만으로 배포 완료가 아니다.
- 향후 로컬 Electron 셸은 "C에 붙는 선택적 데스크톱 래퍼"로 분리
  판단한다(§8) — TeeBox 실행 모델과 무관한 사용자측 배포 정책 문제.

## 3. 컴포넌트

### 3.1 gateway (신규)

- 역할: 정적 UI 자산 서빙, WS 명령 검증, 세션·권한·큐 관리, Rust
  서비스/worker 호출. HTTP/WS 처리와 도메인 기능을 분리해 CLI도 같은
  Rust 서비스를 사용하게 한다. geometry 플랜·raster는 renderd에 남긴다.
- 기존 `make_render_worker`의 job/result는 **호환 계약의 기준**이지
  Python 함수를 호출하라는 뜻이 아니다. `RustRenderWorker`와
  `DeckRenderWorker`의 정책·응답 의미를 Rust 타입과 worker client로
  이관하고 renderd wire에 연결한다. 초기에는 독립 renderd 프로세스와
  현재 취소·게시 경계를 유지한다.

#### 3.1a 단계별 구현체

- **M1부터 Rust gateway**. Python 서버·subprocess 어댑터를 임시 제품
  경로로 추가하지 않는다. 기존 Rust workspace에 서비스/CLI/gateway를
  분리하며 크레이트 이름과 라이브러리는 M0에서 확정한다.
- HTTP/WS는 검증된 Rust 라이브러리를 선정하고 오프라인 빌드용 의존성을
  동봉한다. RFC6455 자체 구현은 하지 않는다. 프레임 길이·fragmentation·
  제어 프레임·비정상 종료·느린 수신자 검증을 게이트에 포함한다.
- capability는 **레이아웃/잡덱 및 전송 기능별**로 협상한다. 현재
  `DeckRenderWorker`는 `supports_margin_prefetch=False`,
  `supports_label_font_px=False`이므로 일반 레이아웃 기능을 그대로
  노출하지 않는다. T3는 구현·검증 전까지 지원한다고 광고하지 않는다.
- 버전은 floe/cli/renderd와 동일 스탬프 체계로 묶고(`--version`,
  시작 스탬프), **UI 자산은 반드시 자기 번들의 것만 서빙**한다. 번들
  일치만으로 skew가 사라지지는 않으므로 추가로: handshake에 프로토콜
  버전을 싣고 불일치는 명시 거부, 이미 열린 구버전 탭의 재접속은
  "새로고침 필요" 안내 후 차단, UI 갱신 정책(gateway 재기동 시 열린
  탭 강제 리로드 여부)을 명시한다.

#### 3.1b Python 기능 이관 범위

파일별 기계적 번역이 아니라 사용자에게 보이는 기능과 입출력 계약을
기준으로 이관한다. 아래는 현재 코드에서 확인한 출발 목록이며 M0에서
공개 명령/옵션별 수용 기준과 연결한다.

| 현재 영역 | 목적지·검증 |
|---|---|
| `floe/cli.py`, `floe2/cli.py`, `instance.py` | Rust CLI/launcher: 명령·종료 코드·로그·옵션 전달·단일 인스턴스/뷰 세션 의미 유지 |
| `cache.py`의 현행 VFS 경로, `vfsclient.py` | Rust 캐시/인덱싱 서비스: freshness·비파괴 재사용·`--force`·jobs·LOD/occupancy·프로파일 옵션, legacy 코드는 제외 |
| `jobdeck/{parser,sources,geom,color,plan,viewer}.py` | Rust jobdeck 라이브러리: 문법·오류/skip ledger·좌표·레이어 순서·레벨/칩 뷰·소스 선택·보고서 parity |
| `rust_render.py`, `service.py`의 Rust 경로, `jobdeck/render.py` | Rust worker client: 명령/응답·취소·타임아웃·프레임 파일 소비/정리·scene/query 유효성 |
| `drc.py`, `svrf.py`, `shots.py`, `fe_embed.py` 및 GUI 안의 저장 로직 | Rust 조회/저장/내보내기 서비스: DRC·waive·주석·룰/레이어 매핑·스크린샷·설정, 기존 Rust drcice/drcpack 재사용 |
| `gui.py`, `view_policy.py`의 UI/상호작용 | HTML/Canvas 입력·표시와 Rust 상태/정책으로 분리: pan/margin·goto·depth/detail/thin·레이어/스타일·단축키·상태줄 |

- Rust에 이미 있는 파서·인덱서·플래너·occupancy·raster·pick/snap·clip은
  재사용한다. `floe-index`/`floe-renderd`를 Python으로 재포장하지 않는다.
- 초기 읽기 전용 범위 밖의 DRC 저장·내보내기·보조 CLI도 이관 목록에서
  추적하며, 빠졌다면 M4의 Python-free 제품 전환은 완료가 아니다.
- jobdeck 정책은 실측 브랜치의 것을 기준으로 한다: 레벨 선택, 레벨/칩
  모드와 부모-자식 목록, `thin=auto|keep|cull`, occupancy 사용/없음 이유,
  요약 레이어의 pick/snap 제한을 웹에서도 숨기지 않는다.

### 3.2 HTML UI

- canvas 2D 단일 뷰포트 + DOM 오버레이(**UI·주석 한정**: 룰러/상태줄/
  마커). **설계 라벨은 프레임 안에 유지**한다 — Rust가 번들 글꼴로
  배치·회전·declutter·겹침 규칙까지 그리며(`RUST_RENDERER.md` 라벨
  계약), DOM 라벨화는 그 규칙의 별도 이관 작업이므로 초기 범위 밖.
  WebGL, OffscreenCanvas, WebP/AVIF 등 신기능 의존 금지(§7 하한).
- **표시 정확도 계약**(성능 게이트 §6의 짝): ① CSS 픽셀 ↔ render
  device 픽셀 관계를 고정(DPR·브라우저 확대는 device 픽셀 기준으로
  요청 크기를 정하고 1:1 표시), ② DBU 좌표와 y축 방향(row 0 = 위,
  F2R-19), fractional viewport 보존, ③ resize 중 이전 크기 프레임은
  GTK처럼 frozen base로 유지, ④ margin crop은 정수 픽셀 정렬 + 16px
  fill 위상 계약(§5 T0에서도 동일), ⑤ raw 업로드(`putImageData`는
  canvas transform을 받지 않음)와 pan/crop 합성(`drawImage`) 경로를
  구분한다.
- 상시 애니메이션 금지, 프레임 단위 통짜 갱신 — ETX(TXP) 압축
  친화적으로(배포 B 대비).
- 정적 파일은 전부 번들 동봉. CDN·외부 폰트 금지(폐쇄망).
- 빌드: browserslist 하한(§7 감사 후 확정) + ES2017 transpile + 호환
  lint를 CI 게이트로.

### 3.3 launcher 통합

- Rust `floe2 view <src> --web`(명령 이름 가칭): gatewayd+renderd 기동 → 토큰 URL
  생성 → Firefox 실행(배포 A/B 공용). DISPLAY는 호출측 환경을 그대로
  따르므로 TeeBox launcher 수정이 불필요하다. 다만 DISPLAY 상속만으로
  독립 인스턴스가 보장되지 않는다: 같은 TeeBox 계정에서 여러 작업을
  실행하면 기존 Firefox 인스턴스 재사용·프로필 잠금 충돌이 난다. 세션별
  프로필(`--profile <세션 dir>`)과 새 인스턴스(`--new-instance`/
  `-no-remote`), 종료 시 프로필 정리 정책을 **실제 버전에 맞춰 M0에서
  검증**한다. `--kiosk`는 Firefox 71+에서만 있으므로(§7) 하한이 그
  아래면 일반 창(`--new-window`)으로 실행한다.
- 공유 URL 발급: 열린 세션에서 읽기 전용 게스트 토큰 URL을
  발급한다(배포 C, DRC 공유 흐름).

## 4. 세션·상태 계약

- **복원 대상인 사용자 상태는 전부 서버(gateway 세션)에 둔다**:
  viewport, depth, detail, layer 가시성, style epoch, DRC 선택/waive
  (파일 기반 기존 체계 재사용), goto 히스토리, thin 정책, jobdeck의
  선택 레벨·level/chip 모드. 일반 레이아웃과 덱의 기본 정책 차이를 보존한다.
- 새로고침/재접속 = 세션 재부착 후 완전 복원. "클라이언트 전용 상태
  금지" 원칙은 **복원할 사용자 상태**에 한정한다 — 마지막 프레임,
  착지 margin, 즉시 pan 표시 같은 **일시 표시 상태는 브라우저에
  허용**한다(GTK의 margin 계약을 옮기는 데 필수, §6).
- **뷰 세션 모델**: 설계·DRC 데이터는 공유하되 viewport·가시성·선택·
  render generation은 **뷰 세션별**이다. renderd는 generation
  frontier와 게시 scene이 하나뿐이라(다른 뷰의 요청은 이전 요청을
  취소하고 pick/snap은 게시 scene을 조회) 독립 탐색 게스트를 같은
  worker에 붙일 수 없다. 따라서 ① 작업자 화면 **따라보기** = 같은 뷰
  세션의 프레임 배포(게스트는 입력 없음), ② **독립 탐색** = 별도 뷰
  세션 = 별도 worker(renderd 프로세스). 별도 worker 방식에는 서버
  전체의 동시 렌더 수·메모리·linger 상한이 함께 필요하다 — worker별
  budget(`FLOE_RUST_BUDGET_MB` 등)만으로는 여러 사용자의 총부하를
  제한하지 못한다(§10-4와 정합).
- 인덱싱 작업까지 포함한 서버 전체 jobs·동시 worker·메모리 입장 정책은
  M0에서 설계한다. 사용자 요구인 인덱싱 16스레드 이내 목표와 렌더
  응답성을 함께 평가하며, worker 수만 줄여 전체 부하가 제한됐다고 보지 않는다.
- UI 종료 후 gateway/renderd는 **linger**(기본 수 분) — 재열기 즉시
  복원 + decoded LRU 보존(F2R-10 보존 스토리와 합류). linger 상한과
  명시 종료 경로를 둔다(고아 방지: renderd의 start_new_session,
  watchdog 경험 재사용).
- 다중 클라이언트(작업자 + 게스트 N)의 조작 권한은 토큰 등급으로
  구분한다(게스트 = 읽기 전용). "같은 scene"의 의미는 위 뷰 세션
  모델을 따른다.

### 4.1 인덱스 수명주기 (설계 필요, 이번 분기로 구현 완료 처리하지 않음)

열린 GUI가 `.ovo` 교체를 즉시 감지하지 않는 기존 항목은 사용자 합의대로
jobdeck 실측의 차단 조건에서 제외한다. 웹/서버 모델에서는 별도 계약을 정한다.

- 소스 식별자·인덱스 revision·뷰 상태 revision·render generation을
  구분한다. 한 세션/프레임이 서로 다른 revision의 OVM/OVP/OVT/OVO를
  섞어 쓰지 않아야 한다. 덱은 참조 소스별 revision 집합도 식별한다.
- 재인덱싱 중 기존 세션 유지, 새 세션의 버전 선택, 명시적 reopen/전환,
  사용 중인 파일의 보존·회수 책임을 Rust 서비스 계약으로 정한다.
- revision 전환 시 frame/margin/retained/query 캐시의 무효화 범위를
  함께 정한다. mtime 감지만으로 일관된 snapshot이 보장된다고 가정하지 않는다.
- 구현 방식(불변 revision 디렉터리/manifest 등)은 M0 설계에서 비교해
  결정한다. 그 전에는 인덱싱 완료 후 열기·재인덱싱 후 재열기를 전제로
  개발하며 hot reload를 지원한다고 표시하지 않는다.

## 5. 전송 계층 (진화 단계)

| 단계 | 프레임 경로 | 비고 |
|---|---|---|
| T0 | renderd의 원자적 publish → Rust worker client가 파일을 소비 → gateway가 WS로 전달 | renderd wire 유지, Python 어댑터 없음. PNG는 기준 경로, loopback raw(T2)도 M1에서 비교. 파일 검증·읽기·정리는 Rust client가 소유 |
| T1 | renderd→gateway 직접 스트림(PNG) | 파일 publish/fsync 제거 |
| T2 | loopback 한정 raw RGBA | 기존 F2R-13 `FLOERAW1` 재사용. M1에서 PNG와 A/B; 웹 UI·전송 복사까지 포함해 G1 평가. raw 지원을 위해 T1 완료를 기다릴 필요 없음 |
| T3 | world-tile 단위 delta + 클라이언트 tile 캐시 | F2R-10/11 합류 지점. 인접 pan의 draw 재지불을 클라이언트 합성으로 흡수. **floe2 전용, 조건부 보류**(F2R-03c 선행 — FLOE2_OPTIMIZATION §3.16) |

- T0/T2의 프레임 형식·치수·길이 검증, 취소 시 파일 정리, raw/PNG 선택,
  오류 응답은 기존 Python 어댑터와 동등하게 검증한다. 브라우저가 서버의
  파일 경로를 지정하거나 publish 디렉터리에 직접 접근하는 API는 제공하지 않는다.
- **프레임 봉투와 취소 계약**(원자적 파일 publish가 보장하던 것을
  네트워크에서 보존): 프레임마다 session/view id, 요청 순번,
  generation, 인덱스 revision(§4.1), render-state revision(layer/depth/style epoch), bbox,
  크기·포맷, 완료 여부(final/refining/bg)를 결합한다. 클라이언트는
  수신 시와 **디코드 완료 시** 두 번 최신 요청인지 재검사하고 stale은
  버린다. 서버는 뷰 세션당 전송 중 프레임 1 + 대기 1로 제한하고
  오래된 대기 프레임을 폐기한다(브라우저 WebSocket은 수신 backpressure
  를 제공하지 않으므로 큐가 쌓이면 화면이 계속 뒤처진다). 느린 게스트
  는 자기 큐만 밀리게 분리해 작업자 렌더와 다른 게스트 전송을 막지
  않는다. 재접속 시 상태 snapshot + 최신 완성 프레임으로 재동기화한다.
- **메모리 상한**: margin 프레임은 16Mpx 상한에서 RGBA 64MiB다. T2
  raw를 네트워크로 보낼 때는 전송 큐·JS 버퍼·canvas 복사본이 겹치므로
  클라이언트 viewport 기준 margin 크기와 뷰 세션별 전송 중 바이트
  상한을 두고, 원격(C)에는 PNG/T1을 기본으로 한다.

- T0/T1/T2는 배포형별 협상(capability handshake)으로 공존 가능하게.
  T1의 직접 스트림은 별도 성능 작업이며 T0/T2의 파일 publish 계약을
  먼저 이관·검증한다.
- T3의 tile key는 F2R-03b 2c 설계가 남겨둔 world/scale 정렬 키를
  사용한다(`FLOE2_OPTIMIZATION.ko.md` §F2R-03 2c 확장 키).

## 6. 성능 요구와 게이트

- **G1 (loopback, 배포 A)**: 동일 뷰·동일 renderd에서 input→photon
  지연과 drag-pan frame pacing이 GTK 셸 이하(±10%)일 것. 미통과 시
  전송 단계(T1/T2)를 앞당겨 재측정.
- **G2 (ETX, 배포 B)**: TeeBox의 실제 Firefox 버전으로 Firefox-in-ETX
  vs GTK-in-ETX를 drag pacing·settle 체감·ETX 대역폭으로 비교.
  미통과 시 주 작업자는 GTK 유지, 웹은 배포 C 전용으로 축소 — 이
  경우에도 투자 손실이 없다(C는 확정 수요).
- **G3 (원격, 배포 C)**: LAN 기준 goto→settle이 ETX 대비 동급 이상.
- **일반 레이아웃의 GTK margin 계약을 M1부터 이관**한다(G1의 전제). "이전 viewport
  이미지를 이동시키고 새 프레임 요청"만 구현하면 GTK가 이미 해결한
  새 strip 검정·라벨 지연·불필요한 왕복이 웹에서 다시 생긴다. 현재
  GTK 계약: 배경 margin 요청(뷰포트 ±한 스텝, 라벨 포함,
  `labels_truncated`면 crop 제외)과 착지 프레임 보관, 동일 상태·배율
  에서의 crop, 16px fill 위상에 스냅한 커서 pan, 착지 margin을 새
  strip의 표시 base로 blit. 이 로직은 renderd가 아니라 gui.py의
  요청·표시 제어에 있으므로 worker 재사용만으로는 따라오지 않는다 —
  브라우저 측 일시 상태(§4)로 옮긴다. G1 판정에는 지연·pacing과 함께
  "새 strip 검정 0, 라벨 지연 0(margin 안)"을 포함한다.
- jobdeck에는 현재 없는 margin 기능을 전제하지 않는다. 덱은 현재 GTK
  덱 경로와 별도로 비교하고, prefetch 추가는 실측 후 별도 변경으로 다룬다.
- **G4 (Python-free/기능 parity)**: Python/PyGObject/KLayout이 없는
  실행 환경에서 Rust CLI→open/index/occupancy→layout/deck render→query/
  clip→DRC 조회·저장/내보내기를 검증한다. 단계별로 구현된 범위만 통과로
  표시하며 최종 판정은 §3.1b 전체 목록을 만족해야 한다. 개발 검증은
  기존 Python/KLayout 게이트를 비교 기준으로 쓸 수 있지만 제품 의존성은 아니다.

## 7. 브라우저 하한 (감사 선행)

- **step 0**: TeeBox `firefox --version` + 동료 데스크톱 대표 버전
  감사. 결과를 이 문서에 기록하고 browserslist 하한으로 박는다.
- 코어 요구는 canvas 2D, binary WebSocket, putImageData/drawImage, PNG와
  현장 브라우저에 맞춘 정적 자산이다. 기존 ESR 52/60 추정은 **지원 확정이나
  안전한 배포 버전 권고가 아니다**. 버전/feature probe/실행 게이트로 하한을
  정하고 유지보수·보안 정책도 확인한다. kiosk 미지원이면 일반 창으로 실행한다.
- 버전 감사와 간단한 ETX 실행 실험(프로필 격리·인스턴스 분리 포함)은
  M3가 아니라 **M0**에서 한다(§9).
- M1 필수 의존에서 제외: OffscreenCanvas, WebP/AVIF, 원본 신문법 배포,
  WebGL2. 최적화를 넣더라도 현장 하한에서 동작하는 기본 경로를 유지한다.
- 접속 첫 페이지에서 필요 API를 feature-detect — 미달이면 필요 버전
  안내를 명시 표출(조용한 오동작 금지).
- 주 작업자 경로(B)는 TeeBox의 Firefox 하나만 문제되므로 하한 협상이
  쉽다.

## 8. 배포·라이선스

- 번들: 기존 portable의 오프라인 빌드·호스트 호환성 검증을 재사용하되
  웹 제품은 Rust CLI/gateway/renderd/indexer + 정적 UI 자산으로 구성한다.
  Python/venv/PyGObject를 웹 제품 실행에 포함하지 않는다. 이관 중 GTK
  검증용 패키지는 구분한다. 브라우저 제공 방식은 M0 환경 감사에서 확정한다.
- Electron(선택, 후순위): 사용자 로컬 셸로만 검토, **현장 검증 전
  제외**(§1). 라이선스는 파일 동봉으로 단정하지 않고 **배포 조건
  체크리스트**로 확인한다: 실제 번들의 Chromium/FFmpeg 빌드 구성,
  FFmpeg(LGPL) 동적 링크 여부와 대응 소스 제공 의무, 코덱 특허
  (H.264/AAC → codec-free 빌드 선택), 고지 파일(`LICENSE`·
  `LICENSES.chromium.html`)의 Open Source Licenses 다이얼로그 편입.
- **보안 기본값**: 공유 서버 전제. A/B는 엄격한 loopback 바인딩(외부
  인터페이스 금지) + ephemeral port + 세션 토큰; **C(원격)는 HTTPS/
  WSS 기본**(폐쇄망이라는 이유로 평문을 기본으로 하지 않는다).
  공통: WebSocket Origin 검증, 토큰 만료·폐기, 명령별 권한 검사
  (게스트 토큰은 읽기 명령만), 공유 토큰은 특정 설계/DRC 세션에
  한정, 클라이언트가 서버 경로나 worker 명령을 직접 지정하는 인터페
  이스 금지(gateway가 화이트리스트 명령만 변환). URL 토큰은 로그·
  브라우저 기록에 남으므로 단기 토큰을 세션 쿠키로 교환하고 URL에서
  제거한다. 같은 TeeBox 계정이 여러 설계에 접근하므로 gateway의 권한
  검사가 접근 통제의 본체다. 토큰 없는 바인딩은 어느 배포형에서도
  금지.

## 9. 마일스톤

0. **M0 — 감사·실험**: TeeBox/대표 데스크톱 Firefox 버전 감사(§7),
   ETX에서 세션별 프로필·새 인스턴스 실행 실험(§3.3), Rust HTTP/WS
   라이브러리·오프라인 의존성 선정. Python 기능 목록/CLI parity 표,
   Rust 서비스 경계·자원 정책·인덱스 수명주기 초안을 확정한다. 현장
   실험과 로컬 설계 항목은 구분해 기록하고, 원격 환경 확인이 안 됐다고
   로컬 기능 목록·프로토콜 설계까지 멈추지는 않는다.
1. **M1 — Rust 기반 + 읽기 전용 웹 뷰어 (배포 A)**:

   - **M1a**: Rust CLI/공통 서비스·worker client. 레이아웃과 jobdeck
     파싱/소스/레벨 선택·캐시 검사·인덱싱 옵션·렌더 제어를 이관한다.
     기존 Python 구현과 CLI 출력/파일/오류·픽셀을 대조한다.
   - **M1b**: Rust gateway 정적 서빙·WS·토큰·프레임 봉투/취소/큐 상한,
     open/goto/pan/zoom/layer·level/chip·thin 상태, PNG/raw 비교.
     지원되는 일반 레이아웃의 margin 요청·착지 보관·crop·16px 스냅
     pan·표시 base를 이관한다. G1 및 읽기 경로 G4 측정까지.

2. **M2 — DRC 공유 뷰어 (배포 C)**: DRC 결과 목록/이동/waive 표시
   (읽기 전용), 게스트 토큰 URL 발급. 확정 수요 대응.
3. **M3 — ETX 게이트 (배포 B)**: M0의 TeeBox 환경/버전을 재확인하고 G2 실측.
   통과 시 launcher를 `--web`으로 전환할 준비, 미통과 시 원인
   분석(전송 단계 상향) 후 재시도.
4. **M4 — 조작 parity + Python-free 제품 전환**: pick/snap/룰러/clip/
   label 토글/단축키, DRC waive·주석·설정 저장, 내보내기·보조 CLI까지
   §3.1b 전체를 검증한다. GTK 셸 은퇴 판정은 이 단계의 G4 완료 + 현장
   검증 후이며, 라이브러리/빌드가 Rust라는 이유만으로 완료 처리하지 않는다.
5. **M5 — T3 전송(world-tile)**: F2R-10 본안과 통합 설계. 인접 pan
   클라이언트 합성 실측으로 world-tile LRU 착수 판정을 겸한다.

## 10. 미해결 질문 (감사·정책 확인 대기)

1. TeeBox·대표 데스크톱의 Firefox 버전 (→ §7 하한 확정).
2. 사용자 데스크톱 → TeeBox HTTP 허용 여부 — 허용이면 배포 B의
   지름길(portal이 뷰어 URL을 직접 열기, ETX 픽셀 전송 소멸)이
   열린다. 동료 접근(배포 C)이 이미 계획되어 있으므로 정책상 같은
   경로일 가능성이 있다.
3. portal → TeeBox launcher에 세션 URL/토큰 전달 채널의 형태.
4. linger 기본값과 공유 서버 자원 정책(§4, FLOE_RUST_BUDGET_MB 고정
   결정과 정합 필요).
5. 인덱스 revision 게시·기존 세션 유지·명시 전환·보존/회수 정책(§4.1).
6. 프론트엔드 언어/빌드 도구. 현재 HTML/Canvas 계획은 유지하되 Rust/WASM
   사용까지 사용자 요구로 확정된 것은 아니다. 서버/CLI의 Python 제거와
   구분한다. 개발용 Python 테스트·생성기까지 제거할 범위와 시점도 미확정.

## 11. 브랜치 운영과 착수 기준 (2026-09-12)

- **실측 기준**: `feature/jobdeck`, 작업 트리
  `/Users/journey/Flatide/floe2_review`. 실칩 jobdeck/occupancy의 화질·속도·
  메모리·재인덱싱 관찰과 그 수정은 이 브랜치에서 계속한다.
- **웹 전환**: `feature/webui`, 작업 트리
  `/Users/journey/Flatide/floe2_webui`. 시작 커밋은
  `6c33a482dab2649fa1c62ad135e63577c6301581`이며, `--lod` 전달과 occupancy
  병용 게이트가 포함된 시점이다. 머지 커밋 직전의 전체 검증은 통과했지만
  이것이 실칩 수용 판정을 대체하지 않는다.
- 실측 후 수정은 **`feature/jobdeck` → `feature/webui` 정방향 머지**로
  주기적으로 가져온다. 공통 수정은 가능한 한 실측 브랜치에서 먼저 고치고
  각 머지마다 기준 커밋·검증 결과를 기록한다. 공개된 작업 이력을 임의로
  rebase하지 않는다. 파일 복사나 반복 cherry-pick을 기본 동기화 방법으로
  삼지 않는다.
- 충돌 시 jobdeck 정책/렌더 정확도/캐시 형식은 실측 브랜치의 최신 계약을
  기준으로 보존한다. Python 쪽에 수정이 들어왔으면 이미 이관한 Rust
  서비스와 회귀 테스트에도 반영한다. 단순히 어느 한쪽 파일 전체를
  선택해 머지 완료로 보지 않는다.
- 웹 전환 미완성 코드를 실측 브랜치로 역머지하지 않는다. GTK/Python
  제거·공통 포맷 변경·배포 기본값 전환은 각 수용 기준을 통과한 뒤 별도
  합류 판정으로 진행한다. 원본 OASIS/실칩 프로파일은 커밋하지 않는다.
- **M0 로컬 산출물**: `WEBUI_M0.ko.md`에 10개 명령/93개 공개 옵션과 보조
  기능, `WEBUI_SERVICE_API.ko.md`에 서비스/전송/자원/revision 초안을 작성했다.
  `tools/audit_webui_env.sh`는 현장 기본 정보용 읽기 전용 도구다. 실제 브라우저
  기능·ETX·접속 측정과 dependency gate는 미완료이며 M0 전체 PASS가 아니다.
- **M1a-1 구현(2026-09-13)**: `rust/worker-client`에 handshake/open/style/
  frame/cancel/cleanup, bounded I/O와 오류/타임아웃을 추가했다. fake worker와
  실제 valmini의 Python 어댑터 PNG/raw 대조 게이트가 있다. 상세 호출 계약과
  poll/메모리 경계는 [worker-client README](../rust/worker-client/README.md).
- **M1a-2a 구현(2026-09-13)**: 개발용 `floe2-web index`의 일반 레이아웃
  경로(캐시/LOD/occupancy/프로파일). [M1a 기록](WEBUI_M1A.ko.md)에 범위와
  차이·회귀 게이트를 둔다. M1a-2b에서 info/단일 PNG render/probe도 이관했고,
  빈 가시 레이어 native plan도 별도 보완했다(M0-D7). M1a-3a에서 잡덱 문법·
  좌표·skip ledger 모델, M1a-3b에서 source catalog와 덱 index를 이관했다.
  M1a-3c/d에서 덱 색/레이어·spec·분석/읽기 CLI와 공통 Dataset을 연결했다.
  M1b-1에서 vendored HTTP/WS 의존성·loopback 인증/제한을 검증했다.
  M1b-2a에서 managed lease/admission·view controller를 검증했다.
  M1b-2b에서 실제 PNG/raw 스트림·frame credit·재접속/취소/종료를 검증했다.
  M1b-2c1에서 등록 범위와 실제 관리형 색인/진행/취소를 연결했다.
  M1b-2c2에서 catalog·재open·색인 작업의 HTTP/WS 경로를 연결했다.
  M1b-3에서 실행 명령·번들 Canvas 기본 UI, private Firefox launcher와
  PNG/raw·최초 한 번의 설정 적용·재접속/종료를 검증했다. M1b-4a에서 layout
  margin의 픽셀 parity·16px pan·라벨/실패 fallback을 검증했다. M1b-4b에서
  drag·스타일/글꼴·기본 단축키와 실제 Chrome 조작을 검증했다. M2a-1에서 기존
  DRC pack/waive 읽기 CLI·페이지 조회를 검증했다. M2a-2는 DRC actor/인증 API,
  M2a-3는 읽기 패널·focus·overlay, M2a-4는 서버 상태 보존·브라우저 복원,
  M2a-5는 현재 규칙 내 유계 순회 API/UI와 선택/이동 모드,
  M2a-6은 화면에 그린 DRC 마커의 클릭 선택/이동,
  M2a-7은 단순 오류의 CD 측정 코어/API 및 표시·지우기·복원,
  M2a-8은 규칙별 선택 집합·박스·금색 마커/복원,
  M2a-9는 Selected/waive/live In view 교집합 목록·순회·hover다.
  M2a-10a는 SVRF sidecar 읽기·타입/derivation·측정 비교 코어/CLI이며,
  M2a-10b는 등록된 metadata/타입·규칙 필터·scalar 비교 API다.
  M2a-10c는 웹 타입/규칙 상세/측정 비교와 In view 해제다.
  M2a-10d1/10d2는 레이어 격리/복원·원자적 focus 서버와 웹 승인 처리·Restore/Escape다.
  M2a-11a/11b는 ASCII DRC 읽기 코어/CLI fallback과 명시 등록한 웹 ASCII의
  조회·선택·순회·윤곽/CD다. M2a-12a는 관리형 pack-build 코어/CLI와
  파일 보존·진행/취소다. M2a-12b1은 승인된 HTTP 작업·기존 actor 종료·새 identity 등록이며,
  M2a-12b2는 브라우저 승인/진행/취소와 새 catalog 조회 복원이다.
  실제 브라우저의 승인 클릭 수용은 별도로 남아 있다(M2 §24).
  jobdeck 물리 plane 격리는 남아 있다. 원본 SVRF subset parser는 M4b-5의 로컬 CLI로
  이관했으며 web API의 임의 deck/include 접근을 추가한 것은 아니다.
  공유와 고급 DRC 조작은 남아 있다.
  공유 권한 경로는 당시 승인 대기였으나 M4g-30에서 opt-in 로컬 구현 범위가 승인됐다.
  실제 원격 공개는 여전히 별도다. M4a-1/2/3은 독립적인
  native/controller와 기존 owner WebSocket 질의이며 공유 권한이나 외부 공개는 추가하지 않는다.
  CLI 전체/웹 전환 완료가 아니며 GTK/실측 브랜치는 유지한다.
  M4e-1에서 DRC waive/주석 포맷과 메모리 편집 모델을 Rust로 이관했다
  ([M4 §21](WEBUI_M4.ko.md)). 실제 review 저장/충돌/API/UI는 다음 단계이며,
  공유 권한 추가나 현장 수용을 대신하지 않는다.
  M4e-2a는 같은 codec의 로컬 원자 저장·파일 충돌·pack binding과 legacy 확인 경계다
  ([M4 §22](WEBUI_M4.ko.md)). 관리형 writer/API/UI·실제 autosave 성능 검증은 남아 있다.
  M4e-2b는 native 관리형 writer의 admission·pack/source lease·취소·typed 결과와 join을
  연결했다([M4 §23](WEBUI_M4.ko.md)). owner/API/UI 및 autosave 실측은 후속이다.
  M4e-2c는 기존 읽기 actor와 store의 pack identity 대조·선택 waive 상태 읽기다
  ([M4 §24](WEBUI_M4.ko.md)). HTTP 쓰기 endpoint와 편집 UI를 추가한 것은 아니다.
  M4e-3a에서 명시 reviewer opt-in의 owner 주석 승인/게시 API를 연결했다
  ([M4 §25](WEBUI_M4.ko.md)). 주석 UI·autosave·waive 쓰기·import/export는 다음 단계다.
  M4e-3b는 선택 주석 read/edit/preview·명시 승인/결과·동일 승인만 복구하는 UI다
  ([M4 §26](WEBUI_M4.ko.md)). 초기 승인 서비스 오류 후 Chrome 합성 읽기·편집·미리보기·
  만료/선택 변경 문구 보존·폐기/종료까지 확인했다. 실제 브라우저 게시는 미실시다.
  주석 badge/overlay·autosave·waive 쓰기·import/export·현장 수용은 남는다.
  M4e-4a는 검증한 waive snapshot만 기존 reader에 적용하는 native/actor 경로다
  ([M4 §27](WEBUI_M4.ko.md)). geometry 캐시를 보존한다. M4e-4b에서 같은 geometry id의
  조회 revision을 갱신하고 오래된 ticket/HTTP·필터·선택/prepared focus를 fence했다
  ([M4 §28](WEBUI_M4.ko.md)). M4e-4c는 별도 `--drc-edit-waives` opt-in의 owner waive
  승인 API와 게시/조회 반영 receipt를 연결한다([M4 §29](WEBUI_M4.ko.md)). 게시한 동일
  파일만 기존 reader에 적용하며 외부 교체는 명시 reopen을 유지한다. M4e-4d는 waive
  action/preview·별도 승인·디스크/reader receipt와 일치 revision에서 조회 재개하는 UI다
  ([M4 §30](WEBUI_M4.ko.md)). 실제 브라우저 게시·현장 수용은 남으며 자동 저장/공유
  권한은 추가하지 않는다. 주석 badge/overlay·명시 import/export도 후속이다.
- **M4e-5a**: 저장 주석 표시용 owner projection API와 admitted snapshot cache를
  추가했다([M4 §31](WEBUI_M4.ko.md)). 목록 최대512개 배지와 이동 대상 하나의 본문만
  읽고 편집 snapshot/preview는 보존한다. badge·본문 overlay UI는 다음 연결이며
  주석 import/export·현장 수용을 완료로 처리하지 않는다.
- **M4e-5b**: 저장 주석 배지·좌상단 본문을 위 API에 연결했다([M4 §32](WEBUI_M4.ko.md)).
  현재 선택과 마지막 ACK 이동을 분리하고 Markers/overlay·서버 상태 복원·pan 무조회·
  편집 preview 보존·저장/늦은 응답 장벽을 검증했다. Chrome 합성 표시/복원을 확인했으며
  브라우저 주석 게시·clipboard·현장 수용은 별도다. 주석 import/export는 다음 단계다.
- **M4e-6a**: 주석/waive snapshot export와 streaming 전체 waive import를 native/managed
  store에 연결했다([M4 §33](WEBUI_M4.ko.md)). 전체 교체·import 확인·입력/대상 충돌과
  취소·admission을 검증했다. 새 HTTP/upload/download/UI는 없으며 owner 전송 연결은 후속이다.
- **M4e-6b**: 위 native 경로에 owner 전용 분할 업로드·비동기 준비/내보내기·만료
  artifact 다운로드 API를 연결했다([M4 §34](WEBUI_M4.ko.md)). 전체 review 교체는
  별도 승인과 portable run 확인 후 기존 게시 경로로만 수행한다. 일반 파일 업로드/guest
  권한은 추가하지 않는다.
- **M4e-6c**: 전체 review 전송 패널을 연결했다([M4 §35](WEBUI_M4.ko.md)). 오류 선택과
  독립적인 1MiB Blob 전송·불명확한 동일 요청만 재시도·전체 교체/DRC run 이중 확인,
  기존 저장 패널의 receipt/복구와 waive reader 장벽을 재사용한다. Chrome 합성 export
  준비/종료는 확인했다. 브라우저 업로드·실제 review 게시·다운로드 파일 수용과 현장
  Firefox/ETX/NFS는 아직 남는다.
- **M4f-1**: Python-free `selfcheck`와 앱 source/target/bundle 식별을 추가했다
  ([M4 §36](WEBUI_M4.ko.md)). 인접 Rust 바이너리만 검사하는 모드와 정상 도구 검색을
  구분하며, 버전 subprocess의 EOF까지 5초 기한을 적용한다. 실제 브라우저 실행/게시
  권한을 확대하지 않는다. 전용 portable 조립·ELF/GLIBC 감사와 현장 수용은 다음 단계다.
- **M4f-2**: 별도 Rust 웹 portable 조립기를 추가했다([M4 §37](WEBUI_M4.ko.md)).
  설치된 툴체인으로만 offline 빌드하고, Linux GNU/musl ELF·동적 버전 요구·원본 고지·
  전체 파일 hash를 검사한다. Linux 조립은 native selfcheck 필수, macOS 교차 조립은
  미실행 표시다. 기존 GTK portable과 기본 실행기를 유지한다.
- **M4f-3a**: 인증된 About GET과 읽기 전용 모달. 앱/소스/target/bundle 및 native 호환
  요구값을 구분하며 내장 글꼴 원문만 표시한다. 전체 고지 열람 UI는 후속 M4f-3b로 분리했고,
  실제 Linux/Firefox/ETX와 SYS-02 전체 수용은 별도로 남긴다.
- **M4f-3b**: 새 portable의 compiled notice index·원본 chunk 검증과 읽기 전용 목록/
  본문 페이징을 추가했다. 개발 빌드/구 배포본은 고지 범위를 명시한다. 전체 패키지
  검증·게시자 인증·현장 수용을 대신하지 않는다([M4 §39](WEBUI_M4.ko.md)).
- **M4g-1**: 오른쪽 드래그 박스 확대/축소를 추가했다. 실제 GTK 처리 함수→production
  JS 제스처→Rust 좌표 대조와 PNG/raw 왕복·취소를 검증한다. 전체 입력 parity나
  전역 camera clamp 재설계를 완료한 단계는 아니다([M4 §40](WEBUI_M4.ko.md)).
- **M4g-2**: 기존 baked frontier의 180×180 구조 미니맵·현재 뷰 표시와 16px 위상
  클릭 이동을 복원했다. world 계산은 Rust, 브라우저는 유계 palette 베이스와 위치
  사각형만 받는다. 새 coverage/thumbnail 렌더·인덱스 형식 변경은 없다([M4 §41](WEBUI_M4.ko.md)).
- **M4g-3**: `<`/`>` 상대 depth를 Rust revision CAS 아래에서 계산한다. DRC 순회 키를
  GTK의 comma/period로 바로잡고 n/w는 기존 주석/waive 승인 편집기로 연결했다.
  자동 저장·q 종료 확인·잡덱 모드 전환·startup 전체 이관은 아니다([M4 §42](WEBUI_M4.ko.md)).
- **M4g-4**: q/End session의 명시 확인·취소 기본 포커스·Escape/Tab 보호를 연결했다.
  확인 전에는 종료 요청·초안 폐기가 없으며 기존 세션 종료 API만 사용한다.
  잡덱 모드 전환·CLI startup/single-instance·현장 수용은 남는다([M4 §43](WEBUI_M4.ko.md)).
- **M4g-5a**: 같은 덱/선택 레벨 안에서 level/chip 공통 가시성과 raw-layer 독립
  가시성, 카메라·뷰 제어 유지, 모드별 기본 스타일 복원을 Rust로 이관했다.
  준비 함수만 추가했으며 자원 상한 안의 worker 교체·CAS 게시·UI는 아직 없다([M4 §44](WEBUI_M4.ko.md)).
- **M4g-5b**: 열린 잡덱의 level/chip/source-layer 모드를 owner 작업으로 전환한다.
  기존 worker 종료/reap 후 같은 예약으로 새 worker를 시작하며, 준비 실패·취소·stale
  revision은 이전 view를 보존한다. source/선택 레벨은 현재 view에서만 얻는다.
  Ctrl+,와 live 선택기를 제공하며 실칩 성능·현장 Firefox 수용은 별도다([M4 §45](WEBUI_M4.ko.md)).
