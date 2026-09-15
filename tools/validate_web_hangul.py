#!/usr/bin/env python3
"""Development-only oracle against the actual GTK fallback composer; no GUI."""
import json
from pathlib import Path
import random
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe.hangul import HangulComposer as Composer  # noqa: E402


def sequence(keys):
    composer = Composer()
    rows = []
    for key in keys:
        out = ""
        if key == "reset":
            composer.reset()
        elif key == "backspace":
            composer.backspace()
        else:
            out, _ = composer.feed(key)
        rows.append([key, out, composer.preedit(), composer.pending()])
    return rows


def main():
    vowels = {v: "".join(k) for k, v in Composer.VOWEL_COMBO.items()}
    tails = {v: "".join(k) for k, v in Composer.TAIL_COMBO.items()}
    cases = []
    syllables = 0
    for lead in Composer.LEADS:
        for vowel in Composer.VOWEL_ORDER:
            for tail in " " + Composer.TAIL_ORDER:
                keys = list(lead + vowels.get(vowel, vowel))
                if tail != " ":
                    keys += list(tails.get(tail, tail))
                rows = sequence(keys)
                expected = chr(0xAC00 + syllables)
                assert rows[-1][2] == expected, (keys, rows[-1], expected)
                cases.append(sequence(keys + ["backspace"] * 6))
                # Tail movement and another vowel exercise more than syllable assembly.
                cases.append(sequence(keys + ["ㅏ", "ㅣ", "ㄱ", "reset", "ㅜ"]))
                syllables += 1
    rng = random.Random(70613)
    choices = list(Composer.KEYMAP.values()) + ["backspace", "reset"]
    cases.extend(sequence(rng.choices(choices, k=80)) for _ in range(400))
    result = subprocess.run(
        ["node", str(ROOT / "rust/web/ui/hangul.test.cjs"), "--oracle"],
        input=json.dumps({"keymap": Composer.KEYMAP, "syllables": syllables,
                          "cases": cases}, ensure_ascii=False),
        text=True, timeout=30, check=False,
    )
    return result.returncode


if __name__ == "__main__":
    sys.exit(main())
