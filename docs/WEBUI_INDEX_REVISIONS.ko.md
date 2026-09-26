# 열린 인덱스 전환·캐시 수명주기

2026-09-25 사용자 승인으로 재개했다. NFS 확장, 원격 공유, 외부 CI,
world-tile, 동결된 WKWebView는 이 작업에 포함하지 않는다.
실제 GUI·현장 테스트 대신 합성 파일을 이용한 로컬 자동 검증만 수행한다.

## 현재 범위

**IR-1 저장·빌드, IR-2 관리형 backend, IR-3의 opt-in UI·명시적 전환을 추가했다.**
기존 `.ice`/`.floe` 해석과 일반 CLI/Run index/Index and open 동작은 그대로다.
새 backend는 웹/Electron 공통 UI의 `Index this source → Immutable index revisions`
에서 별도로 선택한다. 빌드 완료나 브라우저 새로고침이 자동 전환을 일으키지 않는다.
보존 파일의 사용량 조회·회수(IR-4)와 실제 GUI/현장 수용은 아직 남아 있다.

| 단계 | 범위 | 상태 |
|---|---|---|
| IR-1 | 별도 full-build, 검증 후 게시, 불변 경로 pin, 이전 revision 보존 | 구현·로컬 자동 검증 완료 |
| IR-2 | 관리형 Index/일반·잡덱 reader 연결, 덱 전체 revision 집합 고정 | backend 구현·로컬 자동 검증 완료 |
| IR-3 | 명시적 갱신 확인·전환, 상태 보존, frame/margin/retained/query 무효화 | 구현·로컬 자동 검증 완료 |
| IR-4 | 사용량·보존 상태 조회, 사용 중 보호와 명시적 회수/실패 복구 정책 | 남음 |

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
  보존한다. 성공 후 directory fsync 오류는 **게시됨 + durability 경고**이지
  rollback이 아니다. 재시작 후 `Store::pin`으로 관측하는 current와 crash
  durability는 구별한다. 실제 장애/NFS 수용은 이번 검증으로 닫지 않는다.

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
