"""Minimal dubeolsik hangul input for GTK text entries.

Vendored from flateyes (keep in sync — flateyes.py is the origin).
The closed-network hosts usually run the viewer through sudo/setsid
launchers with no input-method connection, so GTK entries cannot
compose hangul on their own. `HangulComposer` is a stand-in state
machine; `TextViewEditable` adapts a Gtk.TextView to the entry-like
surface the composer drives. Both are GTK-free at the class level
(they only touch objects handed to them), so the composer is unit
testable without a display.
"""


class HangulComposer(object):
    """Composes hangul syllables from dubeolsik key strokes.

    feed() takes one compatibility jamo and returns (committed,
    preedit) where committed is finalized text and preedit is the
    syllable still being composed (always the trailing characters of
    the entry).
    """

    KEYMAP = {
        "q": "ㅂ", "w": "ㅈ", "e": "ㄷ", "r": "ㄱ", "t": "ㅅ",
        "y": "ㅛ", "u": "ㅕ", "i": "ㅑ", "o": "ㅐ", "p": "ㅔ",
        "a": "ㅁ", "s": "ㄴ", "d": "ㅇ", "f": "ㄹ", "g": "ㅎ",
        "h": "ㅗ", "j": "ㅓ", "k": "ㅏ", "l": "ㅣ",
        "z": "ㅋ", "x": "ㅌ", "c": "ㅊ", "v": "ㅍ", "b": "ㅠ",
        "n": "ㅜ", "m": "ㅡ",
        "Q": "ㅃ", "W": "ㅉ", "E": "ㄸ", "R": "ㄲ", "T": "ㅆ",
        "O": "ㅒ", "P": "ㅖ",
    }
    CONSONANTS = set("ㄱㄲㄴㄷㄸㄹㅁㅂㅃㅅㅆㅇㅈㅉㅊㅋㅌㅍㅎ")
    LEADS = "ㄱㄲㄴㄷㄸㄹㅁㅂㅃㅅㅆㅇㅈㅉㅊㅋㅌㅍㅎ"
    VOWEL_ORDER = "ㅏㅐㅑㅒㅓㅔㅕㅖㅗㅘㅙㅚㅛㅜㅝㅞㅟㅠㅡㅢㅣ"
    TAIL_ORDER = "ㄱㄲㄳㄴㄵㄶㄷㄹㄺㄻㄼㄽㄾㄿㅀㅁㅂㅄㅅㅆㅇㅈㅊㅋㅌㅍㅎ"
    VOWEL_COMBO = {("ㅗ", "ㅏ"): "ㅘ", ("ㅗ", "ㅐ"): "ㅙ", ("ㅗ", "ㅣ"): "ㅚ",
                   ("ㅜ", "ㅓ"): "ㅝ", ("ㅜ", "ㅔ"): "ㅞ", ("ㅜ", "ㅣ"): "ㅟ",
                   ("ㅡ", "ㅣ"): "ㅢ"}
    TAIL_COMBO = {("ㄱ", "ㅅ"): "ㄳ", ("ㄴ", "ㅈ"): "ㄵ", ("ㄴ", "ㅎ"): "ㄶ",
                  ("ㄹ", "ㄱ"): "ㄺ", ("ㄹ", "ㅁ"): "ㄻ", ("ㄹ", "ㅂ"): "ㄼ",
                  ("ㄹ", "ㅅ"): "ㄽ", ("ㄹ", "ㅌ"): "ㄾ", ("ㄹ", "ㅍ"): "ㄿ",
                  ("ㄹ", "ㅎ"): "ㅀ", ("ㅂ", "ㅅ"): "ㅄ"}
    TAIL_SPLIT = dict((v, k) for k, v in TAIL_COMBO.items())
    VOWEL_SPLIT = dict((v, k[0]) for k, v in VOWEL_COMBO.items())

    def __init__(self):
        self.reset()

    def reset(self):
        self.lead = self.vowel = self.tail = ""

    def pending(self):
        return bool(self.lead or self.vowel)

    def preedit(self):
        if not self.vowel:
            return self.lead
        if not self.lead:
            return self.vowel
        code = 0xAC00 + (self.LEADS.index(self.lead) * 21
                         + self.VOWEL_ORDER.index(self.vowel)) * 28
        if self.tail:
            code += self.TAIL_ORDER.index(self.tail) + 1
        return chr(code)

    def feed(self, jamo):
        if jamo in self.CONSONANTS:
            if not self.lead and not self.vowel:
                self.lead = jamo
                return "", self.preedit()
            if self.lead and not self.vowel:
                out = self.preedit()      # lone consonant: emit as jamo
                self.lead = jamo
                return out, self.preedit()
            if self.lead and not self.tail and jamo in self.TAIL_ORDER:
                self.tail = jamo
                return "", self.preedit()
            if self.tail:
                combo = self.TAIL_COMBO.get((self.tail, jamo))
                if combo:
                    self.tail = combo
                    return "", self.preedit()
            out = self.preedit()
            self.lead, self.vowel, self.tail = jamo, "", ""
            return out, self.preedit()
        # vowel
        if self.tail:
            # the (last part of the) tail becomes the next syllable's lead
            keep, move = self.TAIL_SPLIT.get(self.tail, ("", self.tail))
            self.tail = keep
            out = self.preedit()
            self.lead, self.vowel, self.tail = move, jamo, ""
            return out, self.preedit()
        if self.vowel:
            combo = self.VOWEL_COMBO.get((self.vowel, jamo))
            if combo:
                self.vowel = combo
                return "", self.preedit()
            out = self.preedit()
            self.lead, self.vowel, self.tail = "", jamo, ""
            return out, self.preedit()
        self.vowel = jamo
        return "", self.preedit()

    def backspace(self):
        """Removes one component; returns the remaining preedit text."""
        if self.tail:
            self.tail = self.TAIL_SPLIT.get(self.tail, ("", ""))[0]
        elif self.vowel:
            self.vowel = self.VOWEL_SPLIT.get(self.vowel, "")
        else:
            self.lead = ""
        return self.preedit()


class TextViewEditable(object):
    """Entry-like facade over a Gtk.TextView for the hangul composer."""

    def __init__(self, view):
        self.view = view
        self.buffer = view.get_buffer()

    def get_text(self):
        return self.buffer.get_text(self.buffer.get_start_iter(),
                                    self.buffer.get_end_iter(), True)

    def set_text(self, text):
        self.buffer.set_text(text)

    def get_position(self):
        return self.buffer.get_property("cursor-position")

    def set_position(self, position):
        if position < 0:
            where = self.buffer.get_end_iter()
        else:
            where = self.buffer.get_iter_at_offset(position)
        self.buffer.place_cursor(where)

    def delete_selection(self):
        self.buffer.delete_selection(True, True)

    def replace_span(self, anchor, old_len, new):
        """Swap just the preedit span, keeping the cursor onscreen.
        Rewriting the whole buffer (set_text) empties it for a moment,
        which collapses the scroll position to the top on multi-line
        content — and a programmatic place_cursor never scrolls back."""
        self.buffer.delete(self.buffer.get_iter_at_offset(anchor),
                           self.buffer.get_iter_at_offset(anchor + old_len))
        self.buffer.insert(self.buffer.get_iter_at_offset(anchor), new)
        self.buffer.place_cursor(
            self.buffer.get_iter_at_offset(anchor + len(new)))
        self.view.scroll_mark_onscreen(self.buffer.get_insert())
