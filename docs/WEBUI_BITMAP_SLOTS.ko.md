# 웹 bitmap 슬롯 이관 계약

2026-09-16, M4g-16a. [G4 잔여 감사](WEBUI_G4_AUDIT.ko.md)의 UI-03 개발 도구
항목을 구현 전에 구체화한 기록이다. **슬롯 편집은 아직 웹에 연결하지 않았다.**
이 문서와 source-side 오라클 추가를 기능 완료나 실제 브라우저 수용으로 세지 않는다.

## 1. 현재 코드에서 확인한 의미

| 항목 | GTK의 실제 코드 | 웹/Rust 현재 상태 |
|---|---|---|
| 기본 표 | `floe/fillpatterns.def`의20이름·16×16행 | 같은 표를 compile-time 포함, 프리셋 GET은 불변 목록 |
| 고정 슬롯 | 이름 `solid`/`clear`; 현재0-based18/19 | 값으로 solid/clear를 선택할 수 있으나 슬롯 편집 없음 |
| 개발 도구 표시 | `_on_fill_slot_click`: nonempty `FLOE_FILL_EDIT`에서 우클릭 메뉴 | 같은 env는 현재 공유 기본값 **게시 권한**에만 연결; bitmap 편집 권한 아님 |
| 할당 | `_layer_patterns[(layer,dt)] = slot` | `Assignments.fills`와 `StyleBatch.fill`은 해석된 `Fill` 값만 보유 |
| 편집 영향 | `_edit_fill_pattern`: 슬롯 표 수정 후 모든 참조 행 swatch/worker 갱신 | 선택 행에 현재 bitmap 값을 복사; 다른 행과 슬롯 관계 없음 |
| 렌더 전달 | `_push_fills`: 모든 명시 할당을 **해석된 bitmap**으로 전송 | 같은 값 기반 worker 계약 사용 가능; renderer 슬롯 인식은 불필요 |
| 저장 | `_props_rows`: 편집 내용과 무관하게 슬롯 **이름** 출력 | Calibre text는 표현 불가능한 custom bitmap을 거부; Native JSON은 값 보존 |

`solid`/`clear`를 “첫 두 슬롯”으로 가정하면 잘못된다. 권한 검사는 이름으로 해야 한다.
GTK 우클릭 메뉴만 이 둘을 비활성화한다. `_edit_fill_pattern` 자체에는 고정 슬롯
방어가 없으므로 웹 이관은 UI뿐 아니라 Rust 편집 진입점에서도 거부해야 한다.
`FLOE_FILL_EDIT=0`도 nonempty라 GTK에서는 켜진다는 점을 별도로 고정했다.

동일 bitmap인 두 슬롯도 별개다. 한 슬롯을 편집해도 다른 슬롯을 참조한 행은
변하지 않는다. bitmap 값의 동등성으로 참조를 역추정하면 이 의미를 복원할 수 없다.
현재 웹 custom hex 입력을 슬롯 편집의 대체 구현으로 판정하지 않는 이유다.

## 2. 이관할 편집 동작과 저장 경계

GTK 편집기는 현재 슬롯의 **복사본**을 초안으로 만든다. 클릭은 해당 셀을 뒤집고,
드래그는 첫 셀의 새 값으로 칠한다(지나가는 셀마다 다시 뒤집지 않는다). 바깥 이동은
무시하고 release 후 움직임은 칠하지 않는다. Clear/Solid/Invert/Reset은 초안에만
적용되며 Reset은 편집기를 열 때의 값이 아니라 **내장 기본 표**로 되돌린다.
Cancel은 슬롯·할당·worker 모두 불변, Apply는 슬롯을 바꾸고 기존 참조 전체에 반영한다.
미사용 슬롯 편집은 geometry 렌더를 하지 않지만 이후 그 슬롯을 선택하면 편집 값이 쓰인다.
접힌 레이어 그룹에 나중에 할당할 때도 기존 부모/자식 확장 규칙을 그대로 따른다.

GTK의 source 교체 초기화 경로는 `default_patterns()`와 layerprops의 슬롯 이름으로
다시 구성한다(`gui.py`의 fill palette 초기화). `user-global edited bitmaps`라는
주석만으로 영속 저장을 주장하면 안 된다. `_props_rows`와 두 파일 저장 경로는 실제로
이름만 저장하므로 편집된 bitmap을 재열기 때 보존하지 못한다. 이 손실은 웹의 호환
정답으로 복제하지 않는다([M4 §17](WEBUI_M4.ko.md)의 기존 결정 유지).

따라서 다음 구현은 다음 경계를 지켜야 한다.

1. Rust 세션 상태가 슬롯 이름→bitmap과 행의 **슬롯 참조/직접 값**을 구별한다.
   프리셋 적용·명명된 layerprops 읽기는 참조, custom hex/기존 값 전용 문서는 직접 값이다.
   값이 우연히 기본 패턴과 같다고 기존 값을 참조로 승격하지 않는다.
2. 슬롯 편집은 기존 view/revision CAS로 원자 적용한다. 다른 창/view/오래된 초안에
   적용하지 않으며 거부를 자동 재전송하지 않는다. renderer에는 기존 resolved Fill만
   보내고 style epoch·retained/margin 캐시 무효화를 기존 경로로 수행한다.
3. 영향 수는 현재 선택 수가 아니라 **슬롯 참조 전체와 상속 자식**으로 계산한다.
   현재 스타일 편집의4096행 한계는 넘는 경우 전체 거부하거나 명시적 설계를 추가해야
   하며 첫4096행만 변경하면 안 된다. “슬롯18개뿐”이라는 이유로 fan-out 비용을 무시하지 않는다.
4. 기존 프리셋 GET의 불변 목록 캐시와 열린 세션의 수정값을 혼동하지 않는다.
   다른 소스/잡덱 모드 교체 때는 기존 세션 초기화·스타일 재읽기 경계에 맞춰 초기화한다.
   draft/Canvas는 정적 JS, 검증·슬롯/상속·렌더 결정은 Rust가 소유한다.
5. Native JSON에는 슬롯 표와 참조 의미도 lossless하게 보존하는 버전/검증이 필요하다.
   기존 값 전용 문서는 계속 읽되 직접 값으로 취급한다. Calibre text로 표현할 수 없는
   편집은 기존 명시 오류와 Native JSON 안내를 유지한다. 저장 버튼·공유 기본값 게시를
   슬롯 Apply에 묶거나 `.def` 파일을 덮어쓰지 않는다.
6. 개발용 편집의 launcher opt-in과 서버 검증을 추가해 UI 숨김만을 권한으로 쓰지 않는다.
   기본 off를 유지한다. 현재 같은 env로 활성화하는 **공유 파일 게시**는 계속 별도의
   prepare/preview/명시 승인 경로이며 슬롯 편집 동의에 포함되지 않는다.

이 항목은 UI-03 이관 범위의 구체화다. 개발 도구를 제품에서 폐기한다는 결정은 하지
않았고, 새 서버 파일 탐색·원격 공유·reviewer 읽기 권한도 추가하지 않았다.

## 3. 실행 오라클과 남은 검증

`tools/validate_bitmap_slots.py`는 GTK 원본6개 메서드를 AST로 추출해 실행한다.
inert widget/worker spy와 합성 행만 사용하며 Python GTK import, 실제 GUI/browser,
고객 layout, 파일 게시, 데몬 생성이 없다. 검사 내용:

- 18개 편집 가능 슬롯 ×8개 동작 ×사용/미사용2상태 =288개.
- 같은 bitmap인 별도 슬롯 격리18개, 이미 편집한 슬롯의 기본 Reset18개: 총324편집.
- env 미지정/빈 값/`0`/`1` ×20슬롯 =80개 메뉴/고정 슬롯 검사.
- 각 경우 기존 참조·미할당 행·나중의 접힌 그룹 할당, worker payload/갱신 횟수,
  편집 내용 대신 슬롯 이름을 내보내는 GTK의 저장 한계와 내장 기본 표 불변도 단언한다.

기존 `validate_palette_styles.py`가 먼저 이 검사를 호출하므로 이미 연결된
`validate_rust.sh` 경로에 포함된다. 단독 실행은 아래와 같다.

```sh
.venv/bin/python -B tools/validate_bitmap_slots.py
```

현재 검사는 **source-side 계약**이다. 아직 Rust 슬롯 모델과 비교하지 않는다.
후속 구현은 이 source-side 사례를 Rust 결과와 직접 대조하고 추가로 값 문서의 역호환,
고정 슬롯 서버 거부, stale CAS/대형 fan-out의 원자 거부, JSON round-trip·Calibre 저장
거부, 실제 프레임 픽셀·스타일 캐시 무효화, 브라우저 pointer cancel/드래그/키보드를
검사해야 한다. 실제 브라우저 수용은 이 AST/DOM 검사를 통과해도 별도로 남는다.
