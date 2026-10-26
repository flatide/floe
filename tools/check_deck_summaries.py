#!/usr/bin/env python3
"""Which sources of a jobdeck level draw without a usable occupancy summary.

usage: python3 tools/check_deck_summaries.py DECK.jb LEVEL[,LEVEL..] [FLOE_INDEX]

A deck pass draws its wide views from the source's design.ovo only when the
file is there, was built from this source, and holds the pass's layer with
planes (the viewer's status line counts the others as "passes without
summary" but does not name them - user 2026-10-26: level 5 of the field deck
loaded in 745 s with 133 of 134 passes summarized). For every TC a level
places this reads the cache next to the source (.<name>.ice) with
`floe-index occupancy` and prints one line per source:

  OK       tc  base_um=.. layers=.. unit=..
  NOFILE   tc  (no cache | no design.ovo)
  INVALID  tc  the summary is not this source's (size/mtime), or unreadable
  LAYER    tc  L/D not in the summary | L/D status=none:...

The identity is judged here from the summary header's src_size/src_mtime
against the source file (a floe-index before 0.12.403 reported every light
cache as identity=mismatch, looking for design.ovm; that report is ignored).
Exit 0 when every source is OK, 1 when any is not, 2 on usage, 3 when the
level places nothing. Python 3.6 is enough. FLOE_INDEX: the argument, else
$FLOE_INDEX_BIN, else `floe-index` on PATH (it sits beside floe2). A source
found wanting is remade with `floe2 index SOURCE --occupancy-only`.
"""
import os
import re
import subprocess
import sys


def level_entries(deck, level):
    """source path -> set of 'L/D' the level's entries draw from it."""
    deck_dir = os.path.dirname(os.path.abspath(deck))
    entries = {}
    with open(deck, "r", errors="replace") as f:
        for line in f:
            s = line.strip()
            if not s.startswith("$"):
                continue
            m = re.match(r"\$\s*\(\s*(\d+)\s*,", s)
            if not m or m.group(1) != str(level):
                continue
            tc = re.search(r"TC\s*=\s*'?([^',)\s]+)", s)
            if not tc:
                continue
            ly = re.search(r"LY\s*=\s*\{([^}]*)\}", s)
            dt = re.search(r"DT\s*=\s*\{([^}]*)\}", s)
            lys = [x.strip() for x in ly.group(1).split(",") if x.strip()] if ly else []
            dts = [x.strip() for x in dt.group(1).split(",") if x.strip()] if dt else ["0"]
            path = tc.group(1)
            if not os.path.isabs(path):
                path = os.path.join(deck_dir, path)
            lds = entries.setdefault(os.path.normpath(path), set())
            for l in lys:
                for d in dts:
                    lds.add(l + "/" + d)
    return entries


def occupancy(tool, cache):
    """The occupancy command's header fields, layer statuses by L/D, its
    identity_error line and stderr."""
    r = subprocess.run([tool, "occupancy", cache], stdout=subprocess.PIPE,
                       stderr=subprocess.PIPE, universal_newlines=True)
    head, layers, ident_err = {}, {}, ""
    for line in r.stdout.splitlines():
        if line.startswith("occupancy "):
            head = dict(kv.split("=", 1) for kv in line.split()[1:] if "=" in kv)
        elif line.startswith("layer "):
            fields = dict(kv.split("=", 1) for kv in line.split()[1:] if "=" in kv)
            if "ld" in fields:
                layers[fields["ld"]] = fields.get("status", "?")
        elif line.startswith("identity_error="):
            ident_err = line[len("identity_error="):]
    return r.returncode, head, layers, ident_err, r.stderr.strip()


def check_level(deck, level, tool, out):
    entries = level_entries(deck, level)
    if not entries:
        out("no entries of level %s in %s" % (level, deck))
        return None
    problems = 0
    for src in sorted(entries):
        cache = os.path.join(os.path.dirname(src), "." + os.path.basename(src) + ".ice")
        if not os.path.isdir(cache):
            out("NOFILE   %s  (no cache)" % src)
            problems += 1
            continue
        if not os.path.isfile(os.path.join(cache, "design.ovo")):
            out("NOFILE   %s  (no design.ovo)" % src)
            problems += 1
            continue
        code, head, layers, ident_err, err = occupancy(tool, cache)
        if not head:
            # the file did not open (truncated, another format, ...)
            out("INVALID  %s  %s" % (src, err.splitlines()[-1] if err else "no header (exit %d)" % code))
            problems += 1
            continue
        brief = "base_um=%s layers=%s unit=%s" % (
            head.get("base_um", "?"), head.get("layers", "?"), head.get("unit", "?"))
        # the identity, judged here: the summary's source fingerprint against
        # the source file (the tool's own verdict needed design.ovm before
        # 0.12.403 and said mismatch on every light cache)
        try:
            st = os.stat(src)
            now = (str(st.st_size), str(int(st.st_mtime)))
        except OSError as e:
            now = None
            ident_err = "source: %s" % e
        same = now is not None and now == (head.get("src_size"), head.get("src_mtime"))
        old_tool_quirk = ident_err.startswith("design.ovm:") and "No such file" in ident_err
        if not same or (head.get("identity", "?") != "ok" and not old_tool_quirk):
            why = ident_err or "built for a source of size/mtime %s/%s, this one is %s/%s" % (
                head.get("src_size"), head.get("src_mtime"), now[0], now[1])
            out("INVALID  %s  %s  %s" % (src, brief, why))
            problems += 1
            continue
        bad = []
        for ld in sorted(entries[src]):
            status = layers.get(ld)
            if status is None:
                bad.append("%s not in the summary" % ld)
            elif status != "ok":
                bad.append("%s status=%s" % (ld, status))
        if bad:
            out("LAYER    %s  %s  %s" % (src, brief, "; ".join(bad)))
            problems += 1
        else:
            out("OK       %s  %s" % (src, brief))
    out("level %s: %d source(s), %d with a problem" % (level, len(entries), problems))
    return problems


def main(argv):
    if len(argv) < 3:
        print(__doc__)
        return 2
    deck = argv[1]
    levels = [l.strip() for l in argv[2].split(",") if l.strip()]
    tool = argv[3] if len(argv) > 3 else os.environ.get("FLOE_INDEX_BIN") or "floe-index"
    if not levels:
        print(__doc__)
        return 2
    code = 0
    for level in levels:
        problems = check_level(deck, level, tool, print)
        if problems is None:
            code = max(code, 3)
        elif problems:
            code = max(code, 1)
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv))
