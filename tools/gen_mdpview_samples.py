#!/usr/bin/env python3
"""Generate offline, synthetic MDPview level/chip-view experiments.

Standalone: Python 3.8+ and klayout.db only; no floe, network, or input designs.
The observed jobdeck syntax is experimental, NOT a Calibre format guarantee.
"""

import argparse
import hashlib
import json
from pathlib import Path
import shutil


GUIDE = """# MDPview level / chip view 관찰 세트

이 파일들은 고객 입력 없이 생성한 합성 데이터입니다. Calibre 공식 예제가
아니며 jobdeck 문법의 수용 여부부터 확인해야 합니다. 생성/검증 성공은
Calibre 호환성이나 색상 규칙을 증명하지 않습니다.

## 폐쇄망 생성

gen_mdpview_samples.py 한 파일과 Python 3.8 이상, klayout.db 모듈이 필요합니다.
GUI/GTK/floe/Rust/numpy는 직접 사용하지 않습니다. 스크립트는 네트워크에
접속하거나 의존성을 설치하지 않습니다.

    python3 -c 'import klayout.db as db; print(db.__version__)'
    python3 gen_mdpview_samples.py /work/mdpview_samples_01

기존 출력 디렉터리는 거부합니다. 재실험은 다른 디렉터리로 생성하세요.
KLayout 데스크톱에 내장된 Python과 시스템 python3는 다를 수 있습니다.
모듈이 없으면 서버의 OS/CPU/Python/GLIBC에 맞는 승인된 wheel과 의존성을
별도 준비하고 `python3 -m pip install --no-index --find-links /media/wheels
klayout`으로 설치하세요. 인터넷 다운로드나 관리자 설치는 필요하지 않습니다.
생성 환경이 없다면 다른 승인된 환경에서 생성한 이 디렉터리 전체를 옮겨도
됩니다. 실행 환경의 Calibre에는 KLayout Python이 필요하지 않습니다.

## 실행

먼저 이 출력 디렉터리로 cd한 뒤 MDPview GUI에서 01_control.jb를 여세요.
TC는 sources/... 상대경로입니다. 다른 cwd에서 열 때 상대경로 해석이 다른
버전은 source 검색 경로를 이 출력 디렉터리로 지정하세요. .jb만 옮기지 마세요.
01이 거부되면 오류와 줄 번호를 기록하고 중단하세요. 이때의 실패는 뷰 모드
의미에 대한 증거가 아닙니다. 입력을 고쳤다면 사본과 변경점을 보관하세요.

단위는 um, 소스 DBU와 AD는 0.001, SF는 1입니다. 모든 소스는 flat입니다.
원본 전체 bbox는 (0,0)-(1000,1000). A는 L, B는 삼각형이며 모두 LY7/DT0.
M은 LY7/DT0의 좌하 사각형(0..400)과 LY7/DT1의 우상 사각형(600..1000).
색/글꼴과 무관하게 형상을 식별하려고 TEXT, 작은 글리프, 계층을 쓰지 않았습니다.

현재 배치 모델의 손계산: ROWS=2000/2000은 중심 (2000,2000), 소스 원점은
(1500,1500). 오른쪽 자리는 중심 (4000,2000), 원점 (3500,1500)입니다.
공통 viewport (1000,1000)-(5000,3000), 예: 1200x600 px로 관찰하세요.
모든 패턴을 볼 수 있는 depth, 동일 fill/detail 설정을 사용하세요.
자동 fit은 케이스 간 크기 비교를 가립니다. 이 좌표는 Calibre 실측값이 아닙니다.

## 케이스 순서

| 파일 | 바꾸는 조건 | 관찰 질문 |
|---|---|---|
| 01_control | A 한 파일, level1, CHIP C01, 한 ROWS | 문법 수용, 기본 목록/이름/색은? |
| 02_rows | 01에 오른쪽 ROWS만 추가 | 좌우가 같이 토글되는가? 배치 색이 분리되는가? |
| 03_definitions | 02의 오른쪽 ROWS를 별도 CHIP C02로 분리 | 같은 A라도 CHIP 정의별로 구분되는가? |
| 04_sources | 03의 오른쪽 TC만 B로 교체 | 같은 LY7/DT0인데 파일별 선택/색이 다른가? |
| 05_identical_copy | 03의 오른쪽 TC만 A의 바이트 동일 사본으로 교체 | 파일 경로와 내용 동일성을 어떻게 취급하는가? |
| 06_levels | 03의 오른쪽 entry를 level2로 변경 | level 선택과 chip 목록의 대응은? |
| 07_datatypes | M의 LY7, DT={0,1}을 한 level에 연결 | chip view에서 DT0/DT1을 따로 선택할 수 있는가? |
| 08_shared_source_levels | M을 같은 CHIP의 level1(DT0), level2(DT1)에 연결 | 하나의 TC라도 level 필터가 다른 부분을 선택하는가? |
| 09_same_basename | 04와 같은 A/B, TC만 서로 다른 폴더의 pattern.oas | 같은 basename을 구별하는가? 이름 충돌이 있는가? |

02와 03의 전체 도형은 같고 정의 구조만 다릅니다. 03과 05도 전체 도형은
같습니다. 04와 09도 같습니다. 07과 08의 두 사각형 합집합도 같습니다.
모드 변경은 색/겹침 순서를 바꿀 수 있으므로 RGB 일치보다 geometry와 토글
범위를 관찰하세요. 07은 LY가 하나이므로 LY/DT의 cross/zip 가설과 분리됩니다.

## 각 케이스에서 기록할 것

1. 버전, 로드 시 선택 level, 기본 색상 정책, 수용/경고를 기록합니다.
2. level view의 목록을 펼쳐 이름/번호/색/부모-자식 관계를 적습니다.
3. chip view로 전환합니다(사용 환경에서 Ctrl+, 또는 해당 메뉴).
   항목 하나씩 끄고 사라지는 형상/위치를 기록합니다. 다른 뷰로 전환한 뒤
   돌아왔을 때 숨김/색/펼침 상태가 유지되는지도 확인합니다.
4. 제공되는 경우 Color By Chip / Placement / Definition을 각각 시험합니다.
   색 정책은 level/chip 뷰와 별개의 조건입니다. 버전이 재로드를 요구하면
   레이아웃을 닫고 다시 여세요. 없는 설정은 unsupported로 기록합니다.
5. 06과 08은 로드 시 level1만 선택한 경우와 전체 level 선택을 비교합니다.
   모드 전환으로 미로드 level이 나타나는지, 목록만 있고 도형은 없는지 구별합니다.

observations.template.json을 다른 이름으로 복사해 runs를 추가하세요.
null은 미관측이며 성공이 아닙니다. available_rows에는 각 항목의 label,
parent, color, toggle_effect를 적을 수 있습니다. 실제 칩 경로/파일/화면은
기록하지 말고 합성 실험 결과만 현장 반출 규정에 따라 전달하세요.
manifest.json의 SHA-256은 입력 동일성 확인용이며 인증 서명은 아닙니다.
이 세트는 기능 실험용이며 광역뷰/대형 칩 성능 벤치마크가 아닙니다.
"""


def entry(tc="sources/a.oas", level=1, datatypes=(0,)):
    dt = ",".join(str(d) for d in datatypes)
    return ("$ (%d, LEVEL_%d, AD=0.001, SF=1, TC=%s, LY={7}, "
            "DT={%s}, BX=0, BY=0, UX=1000, UY=1000)"
            % (level, level, tc, dt))


def chip(cid="C01", entries=None, xs=(2000,)):
    return ["CHIP %s, * SYNTHETIC 1.0000" % cid,
            *(entries if entries is not None else [entry()]),
            *("ROWS 2000/%d" % x for x in xs)]


def cases():
    left = chip()

    def right(e):
        return chip("C02", [e], (4000,))

    # Source contents and coordinates stay fixed in each paired comparison.
    return [
        ("01_control", left, (1,), None, "baseline"),
        ("02_rows", chip(xs=(2000, 4000)), (1,), "01_control", "add ROWS"),
        ("03_definitions", left + right(entry()), (1,), "02_rows",
         "split the second ROWS into CHIP C02"),
        ("04_sources", left + right(entry("sources/b.oas")), (1,),
         "03_definitions", "right TC only: a -> b"),
        ("05_identical_copy", left + right(entry("sources/a_copy.oas")),
         (1,), "03_definitions", "right TC only: a -> byte-identical copy"),
        ("06_levels", left + right(entry(level=2)), (1, 2),
         "03_definitions", "right level 1 -> 2, with corresponding MTITLE/name"),
        ("07_datatypes", chip(entries=[entry("sources/m.oas", datatypes=(0, 1))]),
         (1,), None, "single LY with two DTs in one entry"),
        ("08_shared_source_levels", chip(entries=[entry("sources/m.oas"),
         entry("sources/m.oas", level=2, datatypes=(1,))]), (1, 2),
         "07_datatypes", "split DTs between levels of the same CHIP/TC"),
        ("09_same_basename", chip(entries=[entry("sources/left/pattern.oas")])
         + right(entry("sources/right/pattern.oas")), (1,), "04_sources",
         "TC paths only: same bytes, different directories, same basename"),
    ]


def deck_text(blocks, levels):
    return "\n".join([
        "SLICE 1,17", "RETICLE", "* synthetic.jb",
        "OPTION PA, AA=0.0200, BA=0.002000, SA=80",
        *("MTITLE %d,LEVEL_%d" % (n, n) for n in levels),
        "*PLACE-INFO", *blocks, "*END-PLACE", "END", "",
    ])


def build_sources(out, db):
    src = out / "sources"
    src.mkdir()
    opt = db.SaveLayoutOptions()
    opt.format = "OASIS"
    opt.write_context_info = False
    for kind in ("a", "b", "m"):
        layout = db.Layout()
        layout.dbu = 0.001
        # Same cell name in all files: TC is the only identity variable.
        top = layout.create_cell("PATTERN")
        if kind == "m":
            for dt, lo, hi in ((0, 0, 400000), (1, 600000, 1000000)):
                top.shapes(layout.layer(7, dt)).insert(db.Box(lo, lo, hi, hi))
        else:
            pts = ([(0, 0), (1000, 0), (1000, 200), (200, 200),
                    (200, 1000), (0, 1000)] if kind == "a" else
                   [(0, 0), (1000, 0), (1000, 1000)])
            top.shapes(layout.layer(7, 0)).insert(db.Polygon([
                db.Point(x * 1000, y * 1000) for x, y in pts]))
        layout.write(str(src / (kind + ".oas")), opt)
    for original, target in (("a.oas", "a_copy.oas"),
                             ("a.oas", "left/pattern.oas"),
                             ("b.oas", "right/pattern.oas")):
        dest = src / target
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(src / original, dest)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n",
                    encoding="utf-8")


def generate(out):
    import klayout.db as db

    out = Path(out)
    # Refuse existing directories/symlinks: observations must never be erased.
    out.mkdir(parents=True, exist_ok=False)
    build_sources(out, db)
    records = []
    for name, blocks, levels, control, changed in cases():
        (out / (name + ".jb")).write_text(deck_text(blocks, levels), encoding="ascii")
        records.append({"id": name, "deck": name + ".jb", "levels": levels,
                        "control": control, "changed": changed,
                        "calibre_verified": False})
    (out / "README.ko.md").write_text(GUIDE, encoding="utf-8")
    inputs = sorted([*out.glob("*.jb"), *out.rglob("*.oas")])
    hashes = {p.relative_to(out).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
              for p in inputs}
    manifest = {"schema": 1, "generator": "gen_mdpview_samples.py",
                "provenance": "wholly synthetic; no input designs",
                "klayout_version": db.__version__, "units": "um",
                "source_dbu_um": 0.001, "source_bbox_um": [0, 0, 1000, 1000],
                "source_topcell": "PATTERN", "viewport_um": [1000, 1000, 5000, 3000],
                "expectations": "hand-calculated placement model; NOT Calibre oracle",
                "cases": records, "sha256": hashes}
    write_json(out / "manifest.json", manifest)
    write_json(out / "observations.template.json", {
        "note": "null = unobserved, not passed. Duplicate a run for each condition.",
        "runs": [{"case": c["id"], "deck_sha256": hashes[c["deck"]],
                  "calibre_version": None, "loaded_levels": None,
                  "view_mode": None, "color_policy": None, "accepted": None,
                  "available_rows": None, "toggle_effects": None,
                  "roundtrip_visibility_preserved": None,
                  "roundtrip_colors_preserved": None, "viewport_um": None,
                  "image_px": None, "depth": None, "detail": None,
                  "fill": None, "warnings": None, "notes": None} for c in records],
    })
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=Path, help="new output directory; never overwritten")
    args = parser.parse_args()
    try:
        m = generate(args.out)
    except ImportError as exc:
        parser.exit(1, "error: klayout.db is required in this Python environment; "
                    "install an approved offline wheel. %s\n" % exc)
    except (OSError, RuntimeError) as exc:
        parser.exit(1, "error: %s (partial output, if any, is kept; use a new directory)\n"
                    % exc)
    print("%s: %d jobdecks, 6 OASIS files; see README.ko.md" %
          (args.out, len(m["cases"])))
    print("Start with 01_control.jb. Calibre acceptance is NOT yet verified.")


if __name__ == "__main__":
    main()
