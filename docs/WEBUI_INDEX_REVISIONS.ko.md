# 열린 인덱스 전환·캐시 수명주기

2026-09-25 사용자 승인으로 재개했다. NFS 확장, 원격 공유, 외부 CI,
world-tile, 동결된 WKWebView는 이 작업에 포함하지 않는다.
실제 GUI·현장 테스트 대신 합성 파일을 이용한 로컬 자동 검증만 수행한다.

## 현재 범위

**IR-1 저장·빌드 기반을 추가했다. 아직 일반 CLI/Index UI의 동작 변경이나
열린 뷰 hot-reload 구현 완료가 아니다.** 기존 `.ice`/`.floe` 해석과
관리형 읽기/쓰기 잠금은 그대로다. 현재 열린 캐시를 다시 색인하면 여전히
기존 busy 보호를 받는다. 아래 API는 IR-2/3에서 함께 연결할 기반이다.

| 단계 | 범위 | 상태 |
|---|---|---|
| IR-1 | 별도 full-build, 검증 후 게시, 불변 경로 pin, 이전 revision 보존 | 구현·로컬 자동 검증 완료 |
| IR-2 | 관리형 Index/일반·잡덱 reader 연결, 덱 전체 revision 집합 고정 | 남음 |
| IR-3 | 명시적 갱신 확인·전환, 상태 보존, frame/margin/retained/query 무효화 | 남음 |
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
  profile/additive-only/jobdeck 직접 입력은 거부한다. 덱은 IR-2에서 각 소스
  빌드와 revision 집합 게시를 묶어야 한다. additive summary는 기존 revision을
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
  경로를 보호한다. 전체 웹의 파일 capability/protected trees 연결은 IR-2의
  출시 전제다. 현재 일반 웹 API에서 revision store 경로를 받지 않는다.

“불변”은 이 writer API가 게시 파일을 다시 수정하지 않는 계약이다. readonly
filesystem이나 악의적 동일 사용자로부터의 보호를 뜻하지 않는다. stamp는
전체 캐시의 content hash가 아니며, 외부 도구가 게시 디렉터리를 in-place로
수정하는 운영은 지원하지 않는다. remount/다른 host로의 identity portability도
이번 단계에서 제공하지 않는다.

## 보존과 전환의 후속 조건

이전 revision·미게시 후보·게시 불명 증거를 자동 삭제하지 않는다. pin의 drop도
삭제하지 않는다. 따라서 이 단계에는 디스크 사용량 상한이나 완성된 GC가 없다.
사용자 캐시를 재귀 삭제하거나 파일 age만으로 회수하지 않는다.

IR-2는 단일 레이아웃뿐 아니라 덱의 선택 소스 전체를 일관되게 고정하고,
새 revision이 있어도 기존 뷰·Explore 게스트·진행 중 export는 원래 revision에
남도록 연결한다. source별 current를 순차 갱신한 것만으로 덱 트랜잭션이
완성됐다고 주장하지 않는다.

IR-3의 전환은 사용자가 명시적으로 요청한다. 새로운 dataset revision과 worker
epoch를 발급하고 기존 `PreparedReplacement`의 예약 재사용·old worker reap·CAS
cutover를 사용한다. 같은 source를 열면 단순 navigate로 재사용하는 현재 fast
path와 구별해야 한다. stale 버튼/실패한 준비는 이전 뷰를 그대로 둔다.

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
