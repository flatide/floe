# MDPview level / chip view 관찰 샘플 생성기

`tools/gen_mdpview_samples.py`는 **한 파일로 반입·실행 가능한** 별도 생성기다.
기존 `gen_jobdeck_samples.py`의 배치/문법 실험과 달리, level·chip·TC 파일·
ROWS 배치·datatype의 표시/선택 의미를 분리하는 9개 실험에 집중한다.

## 폐쇄망 실행

Python 3.8 이상과 그 Python에 설치된 `klayout.db`가 필요하다.
인터넷, floe 저장소, Rust, GTK, 고객 파일은 필요하지 않다.

```sh
python3 -c 'import klayout.db as db; print(db.__version__)'
python3 gen_mdpview_samples.py /work/mdpview_samples_01
```

저장소 개발 환경에서는:

```sh
.venv/bin/python tools/gen_mdpview_samples.py data/mdpview_samples
.venv/bin/python -B tools/validate_mdpview_samples.py
```

기존 경로에는 쓰지 않는다. 출력은 `.jb` 9개, `.oas` 6개, SHA-256과 실험
조건을 담은 `manifest.json`, 관측용 `observations.template.json`, **한국어
실험 안내서 `README.ko.md`**다. 안내서는 스크립트 안에 들어 있으므로 이 문서를
함께 반입할 필요가 없다. 생성 환경이 없다면 승인된 환경에서 생성한 출력
디렉터리 전체를 전달할 수도 있다.

KLayout 모듈이 없다면 서버 OS/CPU/Python/GLIBC와 맞는 승인된 wheel 및
의존성을 반입하고 `pip --no-index --find-links`로 설치한다. 데스크톱 KLayout이
설치되어 있어도 시스템 Python에서 `klayout.db`를 사용할 수 있다고 단정하지
않는다. 생성기는 자동 설치나 다운로드를 하지 않는다.

## 관찰 요점

01 기본 수용 → 02 같은 CHIP의 ROWS 반복 → 03 같은 TC의 CHIP 정의 분리 →
04 다른 TC → 05 동일 바이트의 다른 파일 → 06 다른 level → 07 두 datatype →
08 같은 TC를 두 level에 분배 → 09 서로 다른 폴더의 같은 basename 순서다.

큰 L·삼각형·분리된 사각형을 사용하고 TEXT/계층은 배제하여 detail, 글꼴,
depth가 목록 실험을 혼동시키지 않도록 했다. 모든 입력은 합성이다.

각 케이스에서 level/chip view와 Color By Chip/Placement/Definition을 별도
축으로 관찰한다. 목록의 부모·이름·색과 항목 하나를 껐을 때 사라지는 도형을
기록한다. 06/08은 로드 시 level1만 선택하는 실험도 한다. 실행 시 출력
디렉터리를 cwd로 하여 `TC=sources/...` 경로를 해석하도록 한다.

**Calibre 문법 수용과 실제 동작은 미검증이다.** 로컬 검증기는 소스 도형,
floe 문법 파싱/배치 모델, 짝 실험의 geometry 합집합, 독립 실행과 비덮어쓰기를
확인할 뿐 Calibre 호환성을 증명하지 않는다. 먼저 `01_control.jb`가 열리는지
확인하고 오류가 있으면 메시지·줄 번호를 기록한다. 관찰 후에만 Calibre
기대값을 회귀 테스트로 승격한다. 실험 결과 반출은 현장 규정을 따른다.
