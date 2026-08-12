# SPEC: vfsd 데몬 프로토콜

정본 코드: `rust/cli/src/vfs.rs` (`vfsd_cmd`, `serve_line`,
`serve_hier`, `serve_frontier`, 프로브들), 클라이언트:
`floe/vfsclient.py`. 세션 계약: `rust/VFS_HIER.md` par.3.5/3.7.

## 1. 기동

```
floe-index vfsd <cachedir> [--budget-mb N=1024] [--stream-kb N=24576]
```
stdio 라인 프로토콜. "quit"/EOF 종료. 캐시당 1데몬(뷰어 렌더 서비스가
스폰). 모든 진행/디버그는 stderr, 프로토콜은 stdout 한 줄.

## 2. 요청 (공백 구분 key=value)

```
gen=N view=x0,y0,x1,y1(µm) px=<px/µm> cut=<px> depth=N|full
layers=all|none|l/d,l/d,... lod=0|1 frames=0|1 labels=0|1 out=DIR
[hair=0.5] [thin=7.0] [mode=probe|frontier] 
비-프로브: ack=N [reset=1] [stream=KB] [nolabels=1]
```

| 키 | 의미 |
|---|---|
| gen | 단조 증가 요청 세대(파일명·트랜잭션 식별) |
| view/px/cut/depth/layers | ViewReq 구성(µm→dbu는 ovm unit으로) |
| lod=0 | 밀도 게이트 kill switch(**px는 보존** — 톤/격자에 필요) |
| frames=0 | frame_cap=0 |
| labels/nolabels | 라벨 플랜 여부(refine 라운드는 nolabels=1) |
| hair / thin | rev 41 계수 / rev 45 격자 µm (뷰어 env 노브 통과) |
| ack / reset | par.3.7 트랜잭션: 이전 응답 적용 확인 / 세션 리셋 |
| stream | 이번 응답의 new 페이로드 상한 KiB (0=무제한) |
| mode=probe | 세션리스 정확 질의(pick/snap/clip): px=0, hair=0, thin=0 강제 |
| mode=frontier | rev 46: 세션리스 미니맵 프런티어(§4) |

## 3. hier 응답 (한 줄)

```
gen=N pages=N new=N evict=name,..|- delta=path|- top=WSNAME|- names=path|-
max_depth=N bytes=N members=N plan_ms=F wc_cells=N inst_edges=N
frame_rects=N partial=0|1 deferred=N lod=N washed_pages=N labels=path|-
nlabels=N text_plan_ms=F labels_truncated=0|1 text_bvh_nodes=... 
committed_mb=F projected_mb=F pending_new_mb=F pending_evict_mb=F
```

- **트랜잭션**: 응답은 "pending txn"으로 기록되고, 다음 요청의 ack가
  그 gen을 가리켜야 커밋된다. ack 누락(뷰어가 버림) = 롤백. reset=1은
  원장 초기화 + 전체 재전송.
- **delta**: `out/delta_<gen>.oas` — WC 트리(WS_<gen>_<ci>_<r> 셀명),
  페이지는 **정의 없이 이름 참조**(뷰어 klayout이 상주 페이지와 바인딩),
  프레임 rect(dt=base+밴드)와 워시 rect 포함. partial=1이면 델타는
  **materialized 페이지만** 참조(유령 셀 방지).
- **names**: ci→디자인 셀명 테이블, 데몬 런당 1회.
- **labels**: per-gen TSV(디자인 텍스트+블록명, 오리엔테이션 포함),
  다음 요청 때 클라이언트가 삭제.
- **evict**: 예산(budget-mb) 초과 시 뷰어가 지울 페이지 셀명들.
- plan_ms는 라벨 워크 포함(상태줄 phase 계산 시 재가산 금지 — 필드
  회귀 이력).

## 4. mode=frontier (rev 46)

요청: `gen=N mode=frontier view=… px=… cut=… depth=D layers=all out=DIR
[hair=][thin=]` → 응답:
`gen=N frontier=DIR/frontier_N.tsv boxes=N plan_ms=F`
TSV 행 = `x0 y0 x1 y1 band`(dbu, 월드좌표, ≤6000 그리드-공정).
**px는 캔버스 fit 스케일이어야 함**(컷이 px 기준 — 미니맵 스케일로
플랜하면 전부 소멸). 뷰 시드가 rbbox로 클립되므로 rbbox 상위집합 뷰는
동일 플랜 → L9 게이트가 meta 굽기와 바이트 일치 검증에 사용.

## 5. 프로브 (pick/snap/clip)

`mode=probe`: 세션 불변, 전 페이지 델타(원장 무접촉), cut은 요청대로
쓰되 px=0/hair=0/thin=0 강제 — "클릭 대상은 절대 컷하지 않는다".
클라이언트 `_probe_layout`이 던지기용 WC를 만들어 계측.

## 6. 오류

`error=<msg>` 한 줄(알 수 없는 키 포함 — 프로토콜 확장 시 클라이언트와
데몬을 함께 배포). 클라이언트는 RuntimeError로 승격, 렌더 서비스는
세션 리셋 후 reset=1 재시도 경로 보유.
