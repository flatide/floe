# 열린 인덱스 전환·캐시 수명주기

2026-09-25 사용자 승인으로 재개했다. NFS 확장, 원격 공유, 외부 CI,
world-tile, 동결된 WKWebView는 이 작업에 포함하지 않는다.
실제 GUI·현장 테스트 대신 합성 파일을 이용한 로컬 자동 검증만 수행한다.

## 현재 범위

**IR-1 저장·빌드, IR-2 관리형 backend, IR-3의 opt-in UI·명시적 전환을 추가했다.**
기존 `.ice`/`.floe` 해석과 일반 CLI/Run index/Index and open 동작은 그대로다.
새 backend는 웹/Electron 공통 UI의 `Index this source → Immutable index revisions`
에서 별도로 선택한다. 빌드 완료나 브라우저 새로고침이 자동 전환을 일으키지 않는다.
IR-4a 사용량 조회에 이어 IR-4b-1의 native reader 직접 잠금·명시적 회수/중단 복구
backend와 IR-4b-2의 **owner 미리보기·별도 1회 승인·응답 유실/새 세션 복구 UI/API**를
연결했다. 승인된 비실측 구현은 마쳤으며 실제 GUI/현장 수용은 남아 있다. 자동 GC는 없다.

| 단계 | 범위 | 상태 |
|---|---|---|
| IR-1 | 별도 full-build, 검증 후 게시, 불변 경로 pin, 이전 revision 보존 | 구현·로컬 자동 검증 완료 |
| IR-2 | 관리형 Index/일반·잡덱 reader 연결, 덱 전체 revision 집합 고정 | backend 구현·로컬 자동 검증 완료 |
| IR-3 | 명시적 갱신 확인·전환, 상태 보존, frame/margin/retained/query 무효화 | 구현·로컬 자동 검증 완료 |
| IR-4a | 사용량·보존 상태 조회, 새 리비전 소유권·협조적 reader 잠금 | 구현, 아래 자동 검증 기록 참조 |
| IR-4b-1 | native 직접 reader 잠금, inactive v3 set 회수·부분 삭제 복구 backend | 구현. 로컬 합성 자동 검증; 아래 계약 참조 |
| IR-4b-2 | owner 전용 미리보기·1회 승인 UI/API, 응답 유실/새 세션 복구 연결 | 구현. 조회 결과는 삭제 권한이 아님; 실제 UI/현장 수용 별도 |

## 저장과 게시 계약

- 기존 캐시와 분리된 `.<source>.ice.revisions/<128-bit id>/`에 새 full-build를
  만든다. 기존 캐시를 복사하거나 hard link하지 않는다. 빌드마다 디스크에
  **전체 새 캐시 크기**가 추가된다. 인덱서의 in-place truncate/delete가 이전
  뷰의 mmap이나 뒤늦은 summary open에 닿지 않게 하는 선택이다.
- `index::revision::Build`는 관리형 index 슬롯·CPU 입장 정책(1~16 jobs,
  foreground reserve)과 별도 store writer lock을 유지한다. drop/cancel은
  native child를 회수한 후 잠금을 놓는다. 기존 캐시 read lease와 충돌하지 않는다.
- 기존 `IndexOptions`의 LOD/occupancy/representatives full-build 옵션을 전달한다.
  단일 소스 Build API는 profile/additive-only/jobdeck 직접 입력을 거부한다.
  IR-2 관리형 backend가 덱의 각 소스 빌드와 revision 집합 게시를 묶는다.
  additive summary는 기존 revision을
  수정하지 않는 별도 설계가 필요하며 이번에 자동 full rebuild로 바꾸지 않는다.
- native exit 0만으로 게시하지 않는다. meta의 형식·소스 identity,
  VFS의 OVM/OVP/OVT pair 구조, 존재하는 OVO/OVR의 OVM binding을 검증한다.
  파일 fsync → `revision.json` seal fsync → revision directory fsync →
  pending current manifest fsync → store directory fsync 후 게시한다.
- `current.json` 원자적 rename이 논리적 게시 시점이다. 새 current는 해당
  revision의 seal과 같은 record다. 게시 직전 source의 dev/inode/size 및
  ns mtime/ctime, 이전 current bytes, store/directory identity를 확인한다.
  단순 seconds mtime만으로 source consistency를 주장하지 않는다.
- rename 시도 후 오류는 `PublicationUnknown`이다. 후보·pending 증거를
  보존한다. 성공 후 directory fsync 또는 `published.json` 회수 증거 기록 오류는
  **게시됨 + sync/회수 증거 경고**이지 rollback이 아니다. 재시작 후 `Store::pin`으로
  관측하는 current와 crash durability는 구별한다. 실제 장애/NFS 수용은 이번 검증으로 닫지 않는다.

## 읽기 계약

- `Store::new`/`pin`은 읽기 전용이며 없는 store를 만들지 않는다. current를
  한 번 읽고 source/id/형식 및 seal 일치를 검증한 뒤 구체적 revision을 반환한다.
  손상된 current를 legacy 캐시로 조용히 우회하지 않는다.
- `Snapshot`은 current가 바뀌어도 같은 디렉터리를 가리킨다.
  meta/OVM/OVP/OVT/OVO/OVR의 **존재 여부까지** file-set identity에 포함한다.
  요약 파일 추가·삭제, inode 교체, 내용 변경에 따른 stamp 차이를 거부한다.
- manifest 크기는 16 KiB로 제한하고 revision은 소문자 hex 32자로 제한한다.
  leaf symlink·비정규 파일·hard link 파일을 허용하지 않는다. 열린 snapshot의
  `open_layout`은 이 경로에서 metadata/VFS를 읽으며 전후 file set을 확인한다.
  source의 ns identity가 달라지면 기존 데이터는 유지하되 stale로 표시한다.
  원자적 교체와 경합하여 이미 연 이전 current fd의 링크 수가 0이 되는 것은
  정상 읽기로 허용한다. 이 예외는 seal/cache 파일에는 적용하지 않는다.
- 해당 Layout의 내보내기는 legacy caches뿐 아니라 revision store·실제 pin
  경로를 보호한다. IR-2는 등록된 source의 deny-only publication protection과
  profile snapshot·잡덱 report/export에도 revision/set/lock 보호를 연결한다.
  현재 일반 웹 API에서 revision store 경로를 받지 않는다.

“불변”은 이 writer API가 게시 파일을 다시 수정하지 않는 계약이다. readonly
filesystem이나 악의적 동일 사용자로부터의 보호를 뜻하지 않는다. stamp는
전체 캐시의 content hash가 아니며, 외부 도구가 게시 디렉터리를 in-place로
수정하는 운영은 지원하지 않는다. remount/다른 host로의 identity portability도
이번 단계에서 제공하지 않는다.

## 보존과 전환의 후속 조건

이전 revision·미게시 후보·게시 불명 증거를 자동 삭제하지 않는다. pin의 drop도
삭제하지 않는다. 따라서 이 단계에는 디스크 사용량 상한이나 완성된 GC가 없다.
사용자 캐시를 재귀 삭제하거나 파일 age만으로 회수하지 않는다.

IR-2는 아래의 set manifest로 덱의 선택 소스 전체를 고정한다. 새 revision이
있어도 기존 ManagedDataset은 원래 pin을 보유한다. 기존 controller의
fork/export는 해당 dataset의 Arc를 유지하는 구조이며, 새 backend로 생성한
실제 Follow/Explore UI 수용은 IR-3 이후 별도 검사다.

IR-3의 전환은 사용자가 명시적으로 요청한다. 새로운 dataset revision과 worker
epoch를 발급하고 기존 `PreparedReplacement`의 예약 재사용·old worker reap·CAS
cutover를 사용한다. 같은 source를 열면 단순 navigate로 재사용하는 현재 fast
path와 구별해야 한다. stale 버튼/실패한 준비는 이전 뷰를 그대로 둔다.

## IR-2 관리형 revision-set backend — 2026-09-26

- 일반 레이아웃도 한 소스의 set으로 다룬다. 활성 목록은
  `.<layout-or-deck>.ice.revisions.sets/current.json`, seal은 같은 저장소의
  `<set-id>/revision.json`이다. 실제 geometry는 IR-1 소스별 revision 경로에 있다.
- 관리형 `start_revisions`는 하나의 index/CPU 예약 아래 선택 소스를 순차
  full-build한다. 각 소스는 검증·봉인만 하고 **소스별 current를 바꾸지 않는다**.
  모든 멤버의 소스 ns identity와 파일 집합을 재검증한 뒤 set current만 한 번
  원자적으로 바꾼다. 하나라도 실패/취소되면 이전 set을 유지한다.
- 이 API는 **명시적 전체 재빌드**이며 기존 Index의 재사용/force/additive 의미를
  몰래 바꾸지 않는다. profile·additive-only는 거부한다. 덱 representatives도
  기존 정책대로 거부한다. 덱 occupancy 기본 on, 일반 기본 off를 보존한다.
- 선택된 소스에 missing/GDS/gzip 등 지원되지 않는 멤버가 있으면 후보 생성 전
  preflight에서 실패한다. 새 backend는 불완전한 덱을 current로 게시하지 않는다.
  기존 legacy backend의 skip/부분 덱 정책은 그대로이며, 향후 UI는 이 차이와
  전체 재빌드 디스크 비용을 승인 전에 설명해야 한다.
- `open_revisions`는 등록된 deck/선택 레벨에서 expected source 집합을 다시
  유도한다. manifest의 소스 집합·레벨이 정확히 맞아야 하며, 검증 전에는
  manifest가 지정한 멤버 파일을 열지 않는다. 임의 디렉터리 선택 기능이 아니다.
  current가 없거나 손상/다른 선택이면 기존 캐시로 조용히 fallback하지 않는다.
- 현재는 set current 한 개를 사용하며 `None(all)`과 명시적 레벨 집합도 구분한다.
  다른 레벨 선택의 새 set을 게시해도 이미 열린 이전 set은 유지된다. 여러 선택의
  recent 목록·재사용이나 부분 선택을 기존 전체 set에서 파생하는 정책은 IR-3/4
  후속이며, 일반 UI의 기존 레벨 선택 기능을 이 제약으로 대체하지 않았다.
- metadata/catalog/compose가 모두 같은 pin의 실제 경로를 사용한다. source를
  다시 해석해 current를 따라가는 경로를 쓰지 않는다. `index_revision`(persistent
  set id)과 기존 `ManagedDataset.revision`(open별 숫자 id)은 구분한다.
- `reopen_mode`는 같은 pin을 유지하며, `DeckModeMemory`도 index revision이
  다르면 mode-only 전환을 거부한다. 원본 jobdeck/소스가 바뀌면 metadata를 다시
  조합하는 작업은 실패한다. 이미 열린 dataset의 geometry 렌더는 유지된다.
- set record는 32 MiB, 멤버는 65,536개로 제한하고 duplicate/상대 source/id
  traversal/미정의 필드를 거부한다. 기존 관리형 lease 집합 상한도 적용되므로
  가능한 선택 수가 이보다 작을 수 있다. 이것은 전체 RSS 상한이 아니다.
- symlink·경로 별칭을 고려한 보호 목록에 소스별 revision/set과 deck set·lock을
  추가한다. generated store는 Git ignore에 추가하며 실제/합성 캐시를 커밋하지 않는다.
- 기존 CLI/GTK와 기본 관리형 open/index는 전환하지 않았다. 다음 IR-3 경로에서만
  `start_revisions`/`open_revisions`의 명시적 선택·UI 승인·상태 CAS를 연결한다.

## IR-3 opt-in UI·명시적 전환 — 2026-09-26

1. 소스·선택 레벨과 기존 jobs/summary 옵션을 고른 뒤 `Approve one full separate
   index build`를 체크하고 **Build new revision**을 누른다. 매번 full-build이며
   force는 사용하지 않는다. 기존 Run index의 force/재사용 동작은 별개다.
   승인 체크는 한 번 제출하면 해제한다. 성공 receipt에 게시 ID와 fsync 경고를
   표시하지만 현재 뷰는 바꾸지 않는다.
2. **Check published revision**은 파일을 쓰지 않고 선택 집합의 current/seal과
   member pin을 검증한다. source handle·정확한 선택 레벨·32자리 revision ID만
   반환한다. 경로 입력/캐시 파일 목록 공개나 자동 Index 권한은 추가하지 않는다.
3. **Use checked revision**은 별도 승인 동작이다. 현재 화면 ID/state revision과
   checked index ID를 고정한다. 전환 준비 시 current가 달라졌으면 busy이며 다시
   Check해야 한다. 준비 중 이후 current 게시가 있어도 이미 확보한 pin만 사용한다.
   빈 workspace에서는 이 방식으로 legacy 캐시 없이도 첫 뷰를 열 수 있다.

같은 source/levels/mode에서 카메라·크기·depth/detail/thin·가시 레이어·스타일·
fill slot·레이어 격리 복원 상태를 그대로 가져온다. DBU, 레이어 키/그룹 또는 덱
plane 이름이 달라졌으면 임의 대응하지 않고 기존 뷰를 유지하며 전환을 거부한다.
다른 source/levels/mode나 달라진 schema를 새로 열려면 먼저 명시적으로 Close한
뒤 선택을 확인하고 Use한다. 일반 Open/CLI launch는 revision 자동 선택을 하지
않는다. 불변 덱의 mode 변경은 같은 set pin을 사용하며, loaded-level 변경이
legacy 캐시로 조용히 우회하지 않는다.

전환 준비는 새 metadata·attachment·휴면 controller를 먼저 만든다. 최종 view
CAS에 실패하거나 그 전에 취소하면 이전 controller를 유지한다. 커밋 시 기존
예약을 공유한 채 old worker의 종료/reap을 기다린 후 새 worker를 열므로 겹쳐
실행하지 않는다. 새 dataset revision/view ID/worker epoch는 이전 frame·margin·
retained·query와 분리되고 브라우저의 두 이미지 버퍼도 비운다. render_key 숫자
자체는 새 controller에서 1로 시작할 수 있으므로 단독 비교로 무효화를 주장하지
않는다. **커밋 후 native 시작/렌더 실패는 새 뷰의 오류이며 자동 rollback은 없다.**

검사 선택·자·DRC selection·미승인 preview와 mode별 임시 메모리는 새 attachment
기준으로 초기화한다. 진행 중인 복구 UI는 Use를 막고, 전환 전 미완성 review 편집을
저장하라는 안내를 표시한다. 저장된 note/waive와 기존 revision 파일은 수정/삭제하지
않는다. 기존 owner-operation 직렬화는 유지하므로 빌드 중 뷰 입력은 일시 잠긴다.
최근 operation ledger를 읽어 결과를 복구할 뿐 불명 응답/재연결에서 새 빌드나 전환
POST를 자동 재전송하지 않는다. 현재 ID는 뷰 API와 UI에 표시한다.

IR-4 전에는 이전·실패 후보가 계속 쌓인다. GC/용량 상한, 외부 캐시 수정 감지,
NFS crash/remount·cross-host 수용, 실제 브라우저/ETX 수용은 이번 완료 범위가 아니다.

## IR-4a 사용량 조회·보호 기반 — 2026-09-27

- 웹/Electron 공통의 `Immutable index revisions → Refresh revision storage usage`는
  owner 전용 `revision_usage` 작업이다. 파일 쓰기·Index·Use·삭제를 수행하지 않는다.
  게스트 권한이나 임의 경로 API를 추가하지 않으며, 등록된 source handle만 받는다.
- 대상은 그 dataset의 set store와 **모든 등록 의존 소스의 source store**다. 다른
  덱의 set manifest를 찾아다니지 않는다. 공유 소스 store에는 다른 dataset이 만든
  리비전도 있으므로 소유자를 `other_dataset`으로만 표시한다. 다른 경로는 노출하지
  않는다. 여러 dataset의 조회 합계를 더하면 공유 store가 중복될 수 있다.
- 합계는 실제 읽어 본 정규·단일 링크 파일의 `len` 합계다. 디스크 할당량·여유 공간·
  **회수 가능 바이트가 아니다**. 포인터·pending 파일도 포함하고, 개별 revision 행의
  합과 전체 합은 다를 수 있다. 등록 외 경로, symlink, hardlink, 하위 디렉터리는
  따라가지 않는다. 알 수 없는 정규 파일은 크기만 세고 extra로 표시한다.
- 한 요청은 최대 256개 store 후보, 256개 revision 행, 4,096개 디렉터리 entry와
  64 MiB의 seal 읽기로 제한한다. current 읽기는 기존 개별 크기 제한을 유지한다.
  상한/누락 발생 시 `PARTIAL`이며 숫자는 관측된 부분 합계다. geometry를 decode하거나
  원본을 인덱싱하지 않는다. 취소를 확인하며 current가 도중 바뀌면 refresh를 요구한다.
- 출력의 revision ID, 논리 바이트, current 여부, format, seal 상태, 사용 잠금,
  관리형 set 소유권은 **조회 시점의 관측**이다. `idle_at_scan`은 협조적 잠금이 그때
  없었다는 뜻이지 안전한 삭제 판정이 아니다. current/구형/미완료/손상/미확인 항목을
  자동으로 지우거나 “회수 가능”으로 표시하지 않는다.
- 새 source/set seal은 **version 2**다. source seal에는 관리형 빌드가 속하는
  dataset·set ID를 기록한다. 새 set은 자신을 위해 새로 만든 v2 source만 게시하며
  다른 set의 source pin 재사용을 거부한다. v1 set이 v2 source를 참조하는 것도
  거부한다. v1은 읽기 호환을 유지하지만 `legacy_untracked`로 표시한다. 구 실행
  파일은 v2를 거부하므로 새 게시본을 열려면 새 web/Electron backend가 필요하다.
  기존 mutable `.ice`와 geometry 캐시 형식은 바꾸지 않는다.
- `Snapshot`은 revision directory의 OS shared lock을 `Arc<File>`로 유지한다.
  마지막 clone이 닫힐 때 해제하며 clone drop에서 명시 unlock하지 않는다. set과
  source를 모두 pin한다. `Layout`과 `RenderSession`도 pin을 보유하며 정상 drop 시
  native worker를 먼저 reap한다. 잠금 불가 파일시스템에서는 open이 명시 실패한다.
  legacy mutable reader에는 이 잠금을 추가하지 않았다.
- **회수를 열기 전 남은 조건**: host의 비정상 종료 뒤 native worker만 남는 경우의
  직접 잠금/수명 계약, preview에 묶인 정확한 대상·소유권·current 재확인, writer 및
  모든 reader에 대한 배타성, 부분 삭제·결과 불명 증거와 명시적 복구다. 지금의 v2
  관측만으로 이 조건이 성립했다고 간주하지 않는다. IR-4b의 회수 가능 형식/reader
  호환성 판정도 그 단계에서 확정한다. v1과 미게시/불명 후보는 계속 보존한다.

이번 단계는 실제 사용자 캐시를 삭제하지 않았다. NFS 장애·cross-host 수용, 실제
GUI·ETX 검사는 실행하지 않았으며, read-only inventory도 자동 주기 실행하지 않는다.

## IR-4b-1 회수·중단 복구 backend — 2026-09-27

IR-4b-1은 `revision::reclaim::prepare`/`Prepared::execute`의 Rust backend다.
당시 HTTP route/owner command·버튼은 없었다(현재 IR-4b-2에서 연결). 자동 회수는 없다. 테스트에서
새로 만든 합성 캐시만 회수했다. 사용자 캐시·원본·리뷰 파일은 삭제하지 않았다.

### 사용 중 파일과 구형 reader 보호

- 새 source/set seal은 **v3**다. v1/v2 읽기 호환은 유지하지만 두 형식은 회수하지
  않는다. set과 member의 형식·소유 dataset/set ID가 모두 맞아야 한다. 구 v2
  backend는 v3를 거부한다. geometry 캐시 포맷/기존 mutable `.ice`는 그대로다.
- render-core `Cache`가 seal 있는 revision directory의 shared lock을 직접 가진다.
  따라서 호스트의 `Snapshot`/`Layout`을 닫아도 별도 renderd가 그 캐시를 사용 중이면
  배타 잠금을 얻을 수 없다. 잡덱은 각 source Cache를 덱 수명 동안 보유한다.
  새 worker의 version handshake는 renderd **0.12.186**을 요구한다.
- 이것은 새 cooperative reader 계약이다. 임의 구형 native 도구로 v3 디렉터리를
  직접 열거나 외부에서 in-place 변경하는 운영을 안전하다고 주장하지 않는다.
- 성공한 current rename **뒤에만** seal과 동일한 `published.json`을 기록·sync한다.
  이 증거가 없거나 다르면 미게시/게시 불명 후보로 보호한다. 기록 실패가 이미
  끝난 게시를 rollback하거나 사용 가능한 current를 숨기지는 않는다.

### 한 번의 회수와 재승인 복구

1. 등록된 dataset과 32자리 set ID에서만 경로를 유도한다. 현재 set/source current,
   standalone source, v1/v2, 미완성·게시 불명 후보는 제외한다. selected sources를
   등록된 입력에서 다시 유도해 manifest와 대조한다. 최대 1,024 sources,
   journal 최대 32 MiB로 제한한다. 모든 대상 잠금을 함께 보유하므로 OS의 열린
   파일 수 제한에 따라 이보다 작은 덱도 쓰기 전에 거부될 수 있다.
2. `prepare`는 파일을 만들거나 삭제하지 않는다. 모든 기존 store writer lock과
   대상 directory 배타 잠금을 잠시 확보해 current bytes·소유권·inode·정확한 파일
   집합/stamp를 고정한다. 예상 파일 수·논리 bytes·복구 여부·5분 유효한 token을
   반환하고 잠금을 놓는다. 조회 bytes는 디스크 할당량/즉시 확보 공간이 아니다.
3. `execute`는 preview를 소비하고 모든 잠금·관측값을 다시 확인한다. 새 reader,
   current 변경, 파일 추가/교체 또는 만료는 쓰기 전에 거부한다. caller는 반드시
   그 preview에 대한 별도 승인을 받아야 한다. 이 승인 UX/API 연결은 IR-4b-2다.
4. 삭제 전 set store의 `.reclaim-<set-id>.json`에 정확한 대상과 stamps를
   create-new로 기록하고 **journal 파일·상위 directory fsync**를 끝낸다.
   중단 복구 시에도 두 sync를 반복해 이전 sync 실패를 무시하지 않는다.
5. 검증된 cache/seal 파일만 개별 unlink하고 빈 directory만 제거한다. source를
   먼저, set을 나중에 지우며 각 directory에서는 seal을 마지막에 지운다.
   재귀 삭제는 없다. current/new revision/store root/원본 파일은 유지한다.
6. 취소는 `interrupted`, 시도한 unlink/rmdir의 오류는 `outcome_unknown`이다.
   syscall을 자동 재시도하지 않는다. 삭제 수/bytes는 성공 응답을 받은 합계이며
   불명 syscall의 실제 삭제량을 추측하지 않는다. 완료 후 sync 오류는 별도 경고다.
7. 불변 journal을 남겨 두므로 새 `prepare`는 실제 남은 파일을 검증한 **복구
   미리보기**가 된다. 다시 승인한 `execute`만 남은 부분을 처리한다. 이미 완료된
   journal은 0 files/complete로 조회된다. journal 손상·변경된/추가된 파일·inode
   교체는 자동 복구하지 않는다. journal은 자동 청소하지 않는다.

### 파일시스템 범위·owner 연결

회수는 명시적인 로컬 FS allowlist만 허용한다: macOS APFS/HFS + local flag,
Linux ext 계열 magic/XFS/Btrfs/tmpfs. NFS/SMB/FUSE/overlay/미확인은 차단한다.
이는 구현 정책이며 Linux 현장 검증 통과를 뜻하지 않는다. 기존 xattr 없는 NFS의
note/waive/default 읽기·저장 구현이나 읽기 전용 revision inventory를 바꾸지 않는다.

IR-4b-2에서 owner 전용 preview/approval 연결, bounded journal 목록/복구 ID
표시, 승인 1회 소비, ledger 기반 응답 유실 확인, 새 서버 세션의 재승인을 추가했다.
inventory의 `idle_at_scan`, Check/Build/Use receipt 또는 reconnect만으로 삭제를
승인하지 않는다. NFS 회수·자동 GC·구형/
미게시/standalone 후보 정리는 이번 범위에 포함하지 않는다.

## IR-4b-2 owner 명시 승인·응답 유실/새 세션 복구 — 2026-09-27

웹/Electron 공통 `Index this source → Immutable index revisions` 패널에서:

1. **Refresh revision storage usage**: 등록된 source에 속한 set 목록과 recovery
   evidence ID를 읽는다. journal 이름은 최대 256개이며 기존 전체 scan 4,096 entries
   예산 안에서만 찾는다. 본문/외부 경로를 따라가지 않으며, 제한 도달 시 PARTIAL이다.
   완료/손상 journal도 이름이 보일 수 있다. 목록은 삭제 권한이 아니고 파일 경로를
   노출하지 않는다. 기록해 둔 32자리 set ID를 직접 입력할 수도 있다.
2. **Preview deletion / check recovery**: owner operation `prepare_reclaim`
   (`source_id`, `revision`, `seq`)가 backend를 읽기 전용으로 검증하고 set ID,
   파일 수/논리 bytes/source 수, recovery/complete와 5분짜리 token을 반환한다.
   현재/열린/old-format/미게시/변경된/지원하지 않는 FS는 보호한다. 보호를
   강제 해제하는 force/path/file-list 옵션은 없다.
3. **Permanently delete… 체크 → Delete approved inactive files**: 별도 operation
   `reclaim_revision`에 exact source/set/token 및 `approved=true`가 필요하다.
   owner cookie/CSRF 계약을 그대로 적용하며 게스트에게 이 권한을 주지 않는다.
   승인한 파일과 비게 된 해당 revision directory만 제거한다. 원본·리뷰·현재
   revision·journal은 제거하지 않는다. 삭제는 되돌릴 수 없다.
4. service는 preview 하나만 보관한다. **요청 접수 때** 소비하므로 worker 실행
   전 취소도 token을 재사용하지 못한다. 다른 owner operation의 접수도 기존
   preview를 무효화한다. 단순 GET/조회 receipt/화면 재접속은 승인하지 않는다.
   source/set 입력 변경·숨김/종료·새 preview에서 체크가 풀리고, 만료 판단은
   서버 monotonic clock이 최종 권한이다. UI 타이머는 보조 표시다.
5. 같은 seq/요청의 재전송은 bounded ledger의 기존 receipt만 반환한다. UI는
   mutation POST를 자동 재전송하지 않고 GET으로만 결과를 확인한다. POST와
   첫 확인 응답이 모두 유실돼도 체크/실행 버튼을 복구해 재삭제하지 않는다.
6. `interrupted`/`outcome_unknown`은 acknowledged 삭제 수만 표시한다. 실제 남은
   파일은 다시 prepare해야 안다. 세션을 새로 열면 old token은 무효이며, usage의
   journal ID → 새 미리보기 → **새 별도 승인**으로만 나머지를 처리한다.
   완료 journal은 `complete`, 0 files/bytes로 표시하고 삭제 버튼은 비활성이다.

preview/ledger는 세션 메모리이며 지속적 권한 저장소가 아니다. journal만 복구
증거로 보존한다. 부분 삭제를 rollback하거나 unknown syscall을 추측하지 않는다.
NFS 캐시 회수·journal 자동 삭제·자동 GC·원격 공유는 추가하지 않았다.

## 검증

- 단위: 읽기의 무변경, current 교체 후 old pin 유지, traversal/잘못된 source/
  과대 manifest, optional summary 추가, 파일 교체, symlink/hardlink 거부,
  writer 배타성·cancel/drop 시 게시/삭제 없음.
- `cache_revision` 배터리: Rust가 만든 작은 합성 OASIS로 실제 release indexer
  full-build(OVO/OVR 포함) → seal/publish → pinned Layout 열기 → 두 번째 게시.
  legacy sentinel/이전 OVP 보존, native 성공 후 미게시 상태, 게시 전 취소,
  손상 marker와 source ns 변경 거부, 관리형 자원 반환을 검사한다.
- 실행: `sh tools/validate_rust.sh --only cache_revision,cache_migration,app_cli,managed_index,validation_selector`.
  선택 배터리 통과는 전체 렌더러 배터리나 실제 GUI/NFS 테스트 통과가 아니다.

2026-09-25 검증 결과: 위 선택 배터리 `ALL OK`, 새 단위 7개 및 실제 인덱서
통합 1개 통과. app-core 전체 lib는 329 passed/8 ignored, 해당 패키지의
fmt 및 Clippy(all-targets/no-deps, warnings deny) 통과. 의존성의 기존 tiler 1건,
VFS 2건 경고는 이번에 수정하지 않았다. 최초 동시 pin 테스트가 잡은
current의 unlink 경합은 위의 제한적 0-link 허용으로 수정하고 재검증했다.
전체 web/renderer 수용, GUI, NFS 실측은 실행하지 않았다.

검증 로그는 로컬 `/private/tmp/floe-index-revision.gbFkuB/`에 두었다.
`core-fixed.log` SHA-256은
`f1f56aaa0b4614b55fb9c77744ef090294dd1ec3b73e0d96a64d13aca1fcaf79`,
`battery.log`는
`0a30104284834813ae1801a567dc5a6be6a4bbaa23df8e31afcd19a8892cf949`이다.

### IR-2 검증 결과 — 2026-09-26

- 실제 native 통합 검사 2개(IR-1 및 새 `managed_revisions`) 통과. 합성 일반
  레이아웃·두 소스 덱을 두 번 게시하고 이전 pin/new pin의 renderd PNG가 같은지
  확인했다. mode 변경도 이전 pin을 유지한다. 덱 두 번째 소스를 의도적으로
  실패시키거나 취소하면 set current가 유지되고 관리형 자원이 반환된다.
- 소스별 current를 앞당기지 않음, 선택 레벨의 exact 일치, 변조된 manifest의
  임의 member 경로 거부, revision 경로의 export 보호를 검사했다. 선택 레벨
  set을 게시한 뒤에도 기존 전체 덱 렌더가 유지되고, 원본 덱 변경 후에는 기존
  geometry 렌더를 유지하되 metadata 재조합(mode reopen)은 거부함을 확인했다.
- 최종 소스 기준 app-core lib **331 passed / 8 ignored**, web lib
  **127 passed / 3 ignored**, 두 패키지 fmt 및 Clippy
  (all-targets/no-deps, warnings deny) 통과. 기존 의존성 경고 3건은 그대로다.
- `sh tools/validate_rust.sh --only cache_revision,cache_migration,app_cli,managed_index,app_jobdeck,app_deck_render,layer_defaults,web_local_sharing,validation_selector`
  선택 배터리 `ALL OK`. cache revision 단위 9개, 기존 관리형 Index, CLI,
  캐시 개명, 레이어 기본값, 로컬 공유/HTTP 권한, 잡덱 계획·합성 렌더를 포함한다.
  최종 member scope 검사를 소스별로 제한한 뒤 lib/native/Clippy/fmt를 재실행했다.
- 실제 GUI·현장 Linux/ETX·NFS·Follow/Explore의 새 backend 수용은 실행하지
  않았다. 일반 UI가 이 backend를 사용하는 단계는 여전히 IR-3이다.

로그는 로컬 `/private/tmp/floe-revision-set.QbVoTM/`에 두었다. SHA-256:

| 로그 | SHA-256 |
|---|---|
| `core-web-accepted.log` | `97707daf95f9ea6eb95815037fc249bb151fa4699fddeb8b240f86e9f8eb35d0` |
| `native-accepted.log` | `b96d53d875b7963fe79d03d805b94b32c03432d6a8b171eddc6256b40616725b` |
| `clippy-accepted.log` | `255cd9aedef35846128d973e677d8d2b054dc0feadea5d27761caf8362b2343b` |
| `battery.log` | `cfcd59085f542d770d0cfce23a4e74f3ed0e906b6ff757f771ded8a06e025170` |

### IR-3 검증 결과 — 2026-09-26

- app-core lib **332 passed / 8 ignored**, web lib **128 passed / 3 ignored**.
  fmt check 및 두 패키지 Clippy(all-targets/no-deps, warnings deny) 통과.
  기존 의존성 경고 3건은 유지했다.
- 실제 native owner HTTP gate **23개 통과**. 새 2개는 revision이 없을 때
  read-only Check, 승인 없는 빌드 거부, legacy 없는 첫 revision open, 빌드 중
  기존 뷰 보존, 오래된 index ID/view CAS 거부, 이동한 camera와 non-default
  display 보존, 새 dataset/worker identity, 예약 1개 재사용, 동일 요청 replay,
  잡덱의 mode pin 및 loaded-level 변경의 mutable fallback 거부를 확인했다.
- ES2017/Node gate에서 실제 app.js를 실행했다. 별도 일회 승인, 빌드 응답 불명
  뒤 GET만으로 복구, 빌드만으로 Use 활성화/자동 전환 안 됨, 별도 Check/Use,
  캡처된 view CAS, 이미 채워진 foreground/margin 버퍼 폐기를 검사했다.
- 선택 배터리 `ALL OK`:
  `sh tools/validate_rust.sh --only cache_revision,owner_service,web_ui,web_local_sharing,view_controller,view_stream,validation_selector`.
  native 캐시 통합 2개·컨트롤러 2개·stream/sharing 20개와 기존 로컬 공유,
  권한·브라우저 상태 회귀를 포함한다. 실제 GUI 픽셀/입력·현장 Linux/ETX/NFS
  수용이나 새 backend의 실제 Follow/Explore UI 수용을 대신하지 않는다.
- 최초 추가 테스트의 depth 타입 오류(코어 enum/문자열 wire)와 테스트 코드의
  Clippy clone 경고를 수정한 뒤 재실행했다. 실패 로그도 보존했다.

로그: 로컬 `/private/tmp/floe-ir3.V08CY1/`. `unit-fixed.log` SHA-256은
`a1fa0712f966649291f193f155005c468be7e0b49db52f7e7b90bdf88cc1290b`,
`clippy-fixed.log`는
`6c46da6c7a3156f4f3cebff7a1ff1913756180e970450646c998e471c2ab0982`이다.
`battery-fixed.log` SHA-256은
`b618b02acbefacffb346da48c46ae592dc19c6e31415792b9ce09f68ac1e59eb`이다.

### IR-4a 검증 결과 — 2026-09-27

- 최종 소스의 app-core lib **339 passed / 8 ignored**, web lib **128 passed /
  3 ignored**, 두 패키지 fmt 및 Clippy(all-targets/no-deps, warnings deny) 통과.
  의존성의 기존 tiler 1건·VFS 2건 경고는 유지했다.
- 새 합성 검사: 없는 store 조회의 무생성, 정확한 논리 바이트 집계, v1 미추적·
  미봉인 상태, snapshot clone 및 별도 프로세스의 공유 잠금, 배타 잠금 시 open
  거부, owner 경로 미추적, 타 set/v1-v2 혼용 거부, symlink·임의 entry 미순회,
  행/메타데이터 읽기 상한, `Layout`이 원래 snapshot 이후에도 pin 유지.
- 실제 native owner HTTP **23개 통과**. 새 사용량 요청은 열린 set/source의
  current·in-use 상태를 보고하고, 뷰 ID 유지와 응답에 파일 경로가 없음을 검사했다.
  ES2017 실제 app.js 게이트는 명시 버튼 1회 요청·화면 유지·큰 정수 byte 문자열·
  빌드/전환과 분리된 조회를 확인했다. 실제 브라우저 화면 검사는 아니다.
- 선택 배터리 `ALL OK`:
  `sh tools/validate_rust.sh --only cache_revision,owner_service,web_ui,web_local_sharing,view_controller,view_stream,validation_selector`.
  실제 native source/set 통합 2개, 컨트롤러 2개, stream/sharing 20개도 통과했다.
  최종에 추가한 seal read-limit 단위 검사는 전체 lib 재실행에서 통과했다.
- 최초 합성 fixture의 잘못된 START record와 추가 요청 이후의 기존 UI 요청 수
  단언을 수정해 재검증했다. 코드 검토 중 발견한 seal 생성/잠금 probe 경합도
  공유 잠금을 seal 생성 전 잡도록 순서를 고쳤다. 실제 사용자 파일 삭제는 없다.

로그: 로컬 `/private/tmp/floe-ir4a.qMrby4/`. SHA-256:

| 로그 | SHA-256 |
|---|---|
| `unit-final.log` | `6bef35a7927ef9fd04006a426a1f0353655431c208db1928ea32ea34890b3976` |
| `clippy-final.log` | `e0c89401f187b5dc7b9b289ec1ef15a8b8d8f8547d75ab4ecedecc86d093676c` |
| `battery.log` | `e44063432cba426818dd7f1e0e33031c9c0e3d2944945a5e046228fb3e94336d` |

IR-4a 당시 진행도: IR-1/2/3 완료, IR-4a 완료. 승인된 비실측 구현 중 남은 것은 IR-4b의
명시적 회수·프로세스 장애 보호·중단 복구다. 실제 GUI/현장 및 보류 범위는 별도다.

### IR-4b-1 검증 결과 — 2026-09-27

- 최종 app-core lib **352 passed / 8 ignored**, web lib **128 passed / 3 ignored**,
  render-core lib **124 passed**. app-core/web Clippy(all-targets/no-deps,
  warnings deny), app-core/render-core fmt 통과. 기존 의존성 경고는 유지했다.
- 새 합성 검사는 current·source current·현재 set의 member 참조·live reader·writer
  배타성, v1/v2/미게시/불명/extra/hardlink/symlink 보호, preview 만료·파일 변경,
  삭제 전 취소·마지막 파일 삭제 후 빈 디렉터리 복구, unlink 전후 불명 오류,
  잘못된 journal/owner/inode 거부를 포함한다. 게시 후 witness 기록 실패도 이미
  바뀐 current를 rollback하지 않으며 기존 증거를 덮어쓰지 않음을 확인했다.
- 별도 **합성 회수 helper**를 한 파일 unlink 직후 강제 종료하고 reap했다.
  kernel 잠금 해제 후 read-only preview는 남은 5개 파일만 보여 주며, 새 execute
  없이는 재개하지 않았다. 이는 로컬 프로세스 종료 검사이지 전원 장애·NFS 장애나
  실제 서버 재시작 수용이 아니다.
- 실제 native source/set 통합 2개 통과. 일반·2소스 잡덱에서 모든 호스트 pin을
  닫아도 직접 띄운 renderd가 각 source의 잠금을 유지했다. native 종료 후에만
  이전 set을 회수했고, current/새 source 파일·새 뷰의 PNG bytes는 유지됐다.
- 선택 배터리 **ALL OK**:
  `sh tools/validate_rust.sh --only cache_revision,app_cli,native_revision,owner_service,web_ui,web_local_sharing,view_controller,view_stream,worker_client,unit_render,validation_selector`.
  owner HTTP 23개, controller 2개, stream/sharing 20개 및 기존 권한/클라이언트
  회귀를 포함한다. 추가 `--only cache_revision`에 이어 최종 소스의 release 리비전
  단위 **29개**와 native 통합 **2개**도 다시 확인했다.
  실제 Chrome/Electron 화면·RHEL/ETX/NFS 검사는 실행하지 않았다.

로컬 로그: `/private/tmp/floe-ir4b.u2eoD1/`의 `battery.log`, `revision-final.log`,
`revision-confirm.log`; 단위·정적 검사는 `/private/tmp/floe-ir4b-core-final.log`,
`/private/tmp/floe-ir4b-clippy.log`다. SHA-256:

| 로그 | SHA-256 |
|---|---|
| `floe-ir4b-core-final.log` | `41255fdd7d97f1ad9ce897d7584b22ef83ddea50b6b25930ee85aebf4c217b01` |
| `floe-ir4b-clippy.log` | `5084233eb4b7b02bc9130d2553eff9a18ee72aed9c8f7930295132732e006560` |
| `battery.log` | `019879fdbb1766df462cf055a18f24117564aeed44652bbb36e6b393d8723416` |

IR-4b-1 당시 진행도: **IR-1/2/3, IR-4a, IR-4b-1 완료**. 승인된 비실측 구현에서 남은 한 묶음은
IR-4b-2의 owner 승인 UI/API·journal 복구 목록·응답 유실/새 세션 재승인 연결이다.
외부 삭제 기능이 열렸다고 보고하지 않으며, 실제 GUI/현장·명시적 보류 항목은 별도다.

### IR-4b-2 최종 검증 결과 — 2026-09-27

- 제품·indexer·renderd **0.12.187** 동기화. `rust/`의 vendored 설정으로
  app-core lib **353 passed / 8 ignored**, web lib **129 passed / 3 ignored**,
  두 패키지 fmt 및 Clippy(all-targets/no-deps, warnings deny) 통과.
  의존성 tiler/VFS의 기존 경고 3건은 수정하지 않았다.
- owner HTTP **25개** 통과. 등록된 source/set/token·별도 승인, 인증/CSRF,
  다른 operation에 의한 preview 무효화, 중복 receipt, 새 서버의 old token 거부,
  완료 journal의 0-file 조회, current bytes/현재 native 프레임 보존을 검사했다.
  테스트 전용 helper를 첫 unlink 후 kill/reap하고 **새 HTTP 세션에서** journal
  조회 → 새 미리보기 → 새 명시 승인으로 남은 부분만 회수하는 검사도 통과했다.
  실행 전 실패·보호 거부 및 조회 자체는 삭제를 시작하지 않는다.
- 전체 웹 UI Node 게이트 통과: ES2017 파싱, 실제 app.js의 합성 DOM 경로,
  set 변경/재조회 시 승인 해제, double-click, POST timeout + 확인 GET 503 후
  GET-only 복구, 미완료/완료 상태, 현재 뷰 보존. actual Chrome/Electron pixel/
  focus/물리 입력 수용을 대신하지 않는다.
- 선택 배터리 **11 gates ALL OK**:
  `sh tools/validate_rust.sh --only cache_revision,owner_service,web_ui,web_local_sharing,view_controller,view_stream,app_cli,native_revision,validation_selector,worker_client,unit_render`.
  release revision 단위 30개, native revision 통합 2개, render-core 124개,
  controller 2개·stream/sharing 20개·native worker parity를 포함한다.
  인증 검사를 보강한 최종 소스로 owner HTTP 25개와 vendored lib/Clippy를 재실행했다.
- 모든 쓰기/회수/프로세스 종료는 별도 임시 합성 fixture에만 적용했다. 사용자
  원본·기존 캐시·리뷰는 변경하지 않았고 실제 GUI·RHEL/ETX·NFS 실측은 하지 않았다.

로컬 로그와 SHA-256:

| 로그 | SHA-256 |
|---|---|
| `/private/tmp/floe-ir4b2.wdZbwD/battery.log` | `c94a760c854139559963b138bba57c9fb11be1fd7a4f842c6a2f7004c17d8695` |
| `/private/tmp/floe-ir4b2-vendored-units.log` | `758475310dced85dd2a26e6d3ffc1fc13c3b486d0196d4859a0d1278d0766902` |
| `/private/tmp/floe-ir4b2-vendored-clippy.log` | `26ed0c717c2bd3f59fdd6923fa255ef15f522733e36028cbdf3d6705d02d7e6a` |
| `/private/tmp/floe-ir4b2-owner-final.log` | `396150ac058930d9b4904ad3d613f0821544fa5d958ea0a26fa4574d994900d9` |

최종 진행도: **IR-1/2/3, IR-4a, IR-4b-1/2 완료. 승인된 비실측 구현의 확인된
잔여는 0**이다. 실제 GUI/현장 수용과 NFS 회수·원격 공유·외부 CI·조건부 world-tile·
WK 동결·정식 배포 승인 등 명시적 보류는 [잔여 표](WEBUI_REMAINING.ko.md)에 구분했다.
