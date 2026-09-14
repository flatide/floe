#!/usr/bin/env python3
"""Authenticated DRC read gate: actual Rust CLI, private synthetic inputs only."""
import json
import math
import os
import random
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile

from validate_web_cli import APP, INDEX, RENDERD, ROOT, Client, read_json, wait
from validate_app_drc import fingerprint
from validate_drc_ice import DB
from validate_web_drc_selection import validate_selection
from validate_web_drc_filters import validate_filters
from floe import drc


def main(fixture, ascii=False):
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    built = subprocess.run([cargo, "test", "--offline", "--locked", "-p", "floe-web",
                            "--test", "drc_service", "--no-run", "--message-format=json"],
                           cwd=ROOT / "rust", capture_output=True, text=True, timeout=180)
    assert built.returncode == 0, built.stderr
    tests = [r["executable"] for line in built.stdout.splitlines()
             if (r := json.loads(line)).get("reason") == "compiler-artifact"
             and r["target"]["name"] == "drc_service" and r.get("executable")]
    assert len(tests) == 1
    with tempfile.TemporaryDirectory(prefix="floe-web-drc-") as td:
        work = Path(td)
        layout_dir = work / "layout scope"
        layout_dir.mkdir()
        source = layout_dir / "layout.oas"
        shutil.copy2(fixture, source)
        index = subprocess.run([str(INDEX), "vfs", str(source), str(source) + ".floe", "--jobs", "2"],
                               capture_output=True, timeout=30)
        assert index.returncode == 0, index.stderr
        data = work / "DRC synthetic"
        data.mkdir()
        db = data / "한 글.db"
        # Include a multi-page rule, a large polygon (paged coordinates), and
        # names that must never become HTML. Reuse parser adversarial cases.
        text = DB + '\nMASK<&>\n130 130 1\nmask rule\n'
        for i in range(130):
            text += 'p %d 4\n%d 0\n%d 0\n%d 10\n%d 10\n' % (i+1, i*20, i*20+10, i*20+10, i*20)
        text += 'BIG\n1 1 1\nlarge polygon\np 1 5000\n'
        text += ''.join('%d %d\n' % (i, i % 2) for i in range(5000))
        # CD measurements: legacy ordering, touching/crossing/degenerate edges,
        # skew pairs, rectangle permutations, both endpoint orders/translations.
        cd_cases = [
            ('e', [(0, 0), (3, 4)]),
            ('e', [(0, 0), (0, 0)]),
            ('e', [(0, 0), (10, 0), (0, 3), (10, 3)]),
            ('e', [(0, 0), (1, 0), (2, 1), (3, 1)]),
            ('e', [(1, 0), (0, 0), (3, 1), (2, 1)]),
            ('e', [(0, 0), (10, 0), (5, -5), (5, 5)]),
            ('e', [(0, 0), (10, 0), (5, 0), (15, 0)]),
            ('e', [(0, 0), (10, 0), (10, 0), (10, 10)]),
            ('e', [(0, 0), (0, 0), (2, 1), (3, 1)]),
            ('p', [(0, 0), (10, 0), (10, 20), (0, 20)]),
            ('p', [(0, 0), (10, 20), (10, 0), (0, 20)]),
            ('p', [(0, 0), (10, 10), (0, 20), (-10, 10)]),
        ]
        rng = random.Random(76)
        for _ in range(32):
            cd_cases.append(('e', [(rng.randint(-5000, 5000), rng.randint(-5000, 5000)) for _ in range(4)]))
        cd_cases += [(kind, [(x+123456, y-987654) for x, y in pts]) for kind, pts in cd_cases]
        text += 'CDHELPERS\n%d %d 1\nCD oracle cases\n' % (len(cd_cases), len(cd_cases))
        for i, (kind, pts) in enumerate(cd_cases):
            n = len(pts) if kind == 'p' else len(pts)//2
            text += '%s %d %d\n' % (kind, i+1, n)
            if kind == 'p':
                text += ''.join('%d %d\n' % p for p in pts)
            else:
                text += ''.join('%d %d %d %d\n' % (*pts[j], *pts[j+1]) for j in range(0, len(pts), 2))
        db.write_text(text)
        run = subprocess.run([str(INDEX), "drc", str(db), "--jobs", "2"], capture_output=True, timeout=30)
        assert run.returncode == 0, run.stderr
        packed = Path(str(db) + ".ice")
        os.environ["FLOE_REVIEWER"] = "web-gate"
        p = drc.IcePack(str(packed))
        side = Path(p._waive_path)
        expected = []
        for ci, check in enumerate(p.checks):
            errors = []
            for ei, e in enumerate(check.errors):
                status = 1 if ei % 3 == 0 else 2 if ei % 7 == 0 else 0
                p.set_status(ci, ei, status)
                errors.append(dict(local=str(ei), glob=str(e.num), kind=e.kind, status=status,
                                   bbox=e.bbox(), pts=e.pts))
            expected.append(dict(name=check.name, desc=check.desc, errors=errors,
                                 waived=p.status_counts(ci)[0]))
        p.close()
        assert sum(len(c["errors"]) for c in expected) > 130
        assert any(len(e["pts"]) == 5000 for c in expected for e in c["errors"])
        rules_path = data / "rules.json"
        rules_path.write_text(json.dumps(dict(format="floe-svrf-rules", version=1, checks={})))
        temps = work / "temp"
        temps.mkdir()
        env = dict(os.environ, PATH="", TMPDIR=str(temps), FLOE_INDEX_BIN=str(INDEX),
                   FLOE_RENDERD_BIN=str(RENDERD), FLOE_DRC_WEB_PACK=str(packed), FLOE_DRC_WEB_RULES=str(rules_path))
        before = fingerprint(data)
        test = subprocess.run([tests[0], "--ignored", "--nocapture"], env=env,
                              capture_output=True, text=True, timeout=20)
        assert test.returncode == 0, (test.stdout, test.stderr)
        assert "RUST DRC ACTOR: ALL OK" in test.stdout
        assert "RUST DRC WAIVE REFRESH: ALL OK" in test.stdout
        if ascii:
            # Keep the packed oracle above, but use a distinct fractional
            # registered input. Its adjacent garbage ICE must not be consulted.
            fractional = []
            for line in text.splitlines():
                words = line.split()
                if len(words) in (2, 4) and all(w.lstrip('-').isdigit() for w in words):
                    line = ' '.join(format(int(w)/8 + (.0625 if j % 2 == 0 else -.1875), '.17g')
                                    for j, w in enumerate(words))
                fractional.append(line)
            db = data / 'fractional.db'
            db.write_text('\n'.join(fractional) + '\n')
            Path(str(db) + '.ice').write_bytes(b'not an authorized implicit input')
            p = drc.load_ascii(str(db))
            expected = [dict(name=c.name, desc=c.desc, waived=0,
                        errors=[dict(local=str(i), glob=str(e.num), kind=e.kind, status=0,
                                     bbox=e.bbox(), pts=e.pts) for i, e in enumerate(c.errors)])
                        for c in p.checks]
        session_path = work / "session.json"
        # Authorizing a DRC parent must NOT authorize a deck TC in that parent.
        outside = data / "not-authorized.oas"
        shutil.copy2(source, outside)
        deck = layout_dir / "escape.jb"
        deck.write_text("MTITLE 1,Mask\nCHIP C\n$ (1,PATTERN,TC='../DRC synthetic/not-authorized.oas',AD=0.001,LY={1},DT={0},UX=100,UY=100)\nROWS 0/0\n")
        denied = subprocess.run([str(APP), "view", str(deck), "--no-open", "--drc", str(packed),
                                 "--session-file", str(session_path)], env=env,
                                capture_output=True, text=True, timeout=15)
        assert denied.returncode != 0 and "outside approved roots" in denied.stderr
        assert not session_path.exists() and not list(temps.iterdir())
        before = fingerprint(data)
        args = [str(APP), "view", str(source), "--no-open", "--session-file", str(session_path),
                "--drc", str(db if ascii else packed), "--jobs", "2", "--raster-jobs", "1",
                "--no-labels", "--frame-cache", "off"]
        if not ascii:
            args += ["--drc-waives", str(side)]
        for extra in (["--jobs", "15", "--raster-jobs", "1"], ["--budget-mb", "2048"]):
            rejected = subprocess.run(args + extra, env=env, capture_output=True, text=True, timeout=15)
            assert rejected.returncode != 0 and "managed resource" in rejected.stderr
            assert not session_path.exists(), "over-capacity configuration published a session URL"
            assert not list(temps.iterdir())
            assert fingerprint(data) == before
        proc = subprocess.Popen(args, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            client = Client(wait(lambda: read_json(session_path), proc))
            client.call("GET", "/api/v1/drc", code=401)
            client.call("POST", "/api/v1/drc/unknown/read", {}, code=401)
            client.call("GET", "/api/v1/drc/unknown/views/unknown/panel", code=401)
            client.call("POST", "/api/v1/drc/unknown/views/unknown/panel", {}, code=401)
            client.call("GET", "/api/v1/drc/unknown/views/unknown/selection", code=401)
            client.call("POST", "/api/v1/drc/unknown/views/unknown/selection", {}, code=401)
            client.login()
            assert client.call("GET", "/api/v1/capabilities")["drc"] is True
            catalog = wait(lambda: (lambda d: d if d["phase"] == "ready" else None)(
                client.call("GET", "/api/v1/drc")["drc"]), proc)
            assert catalog["read_only"] and catalog["metadata"]["waives"] is (not ascii)
            assert catalog["metadata"]["format"] == ("ascii" if ascii else "ice")
            assert catalog["metadata"]["checks"] == str(len(expected))
            startup = client.call("GET", "/api/v1/startup")["request"]
            startup["body"]["pixels"] = [257, 191]
            client.call("POST", "/api/v1/operations", startup, 202)
            opened = client.finished(1, proc)
            assert opened["phase"] == "succeeded", opened
            context = dict(view_id=opened["view_id"], revision=catalog["revision"])
            endpoint = "/api/v1/drc/" + catalog["id"] + "/read"
            panel_path = "/api/v1/drc/" + catalog["id"] + "/views/" + opened["view_id"] + "/panel"
            fresh_panel = client.call("GET", panel_path)
            assert fresh_panel == dict(revision=catalog["revision"], view_id=opened["view_id"],
                                       state=dict(panel_rev="1", body=None))
            chosen = next(i for i, c in enumerate(expected) if c["errors"])
            panel = dict(search="<script>", metric=None, rule_start="0", check=str(chosen), error_start="0",
                         query=None, in_view=True, selected_only=True, waived=False, selected=dict(check=str(chosen), error="0"),
                         markers=True, shown=True, jump_scale=".25", zoom_lock=True,
                         jump_active=True, focus_visible=True,
                         cd=dict(target=dict(check=str(chosen), error="1"), remaining=2))

            def save_panel(value, base="1", code=200, **extra):
                return client.call("POST", panel_path, dict(revision=catalog["revision"],
                                    base_panel_rev=base, body=value, **extra), code)

            client.call("POST", panel_path, dict(revision="stale", base_panel_rev="1", body=panel), 409)
            # CSRF remains mandatory for in-memory writes, not just file writes.
            csrf = client.csrf
            client.csrf = "wrong"
            save_panel(panel, code=401)
            client.csrf = csrf
            save_panel(dict(panel, path="/etc/passwd"), code=400)
            save_panel(dict(panel, selected=dict(check=str(chosen), error="999999999")), code=400)
            save_panel(dict(panel, check=str(len(expected))), code=400)
            save_panel(dict(panel, rule_start=str(len(expected)+1)), code=400)
            save_panel(dict(panel, error_start="999999999"), code=400)
            save_panel(dict(panel, jump_scale="NaN"), code=400)
            save_panel(dict(panel, jump_active=True, focus_visible=False), code=400)
            save_panel(dict(panel, selected=None), code=400)
            save_panel(dict(panel, cd=dict(target=dict(check=str(chosen), error="999999999"), remaining=2)), code=400)
            save_panel(dict(panel, cd=dict(target=dict(check=str(chosen), error="00"), remaining=2)), code=400)
            save_panel(dict(panel, cd=dict(target=panel["selected"], remaining=4)), code=400)
            save_panel(dict(panel, cd=dict(target=panel["selected"], remaining=-1)), code=400)
            save_panel(dict(panel, cd=dict(target=panel["selected"], remaining=True)), code=400)
            save_panel(dict(panel, jump_active=False), code=400)
            save_panel(dict(panel, search="x"*257), code=400)
            save_panel(dict(panel, metric=""), code=400)
            save_panel(dict(panel, metric="width"), code=400)
            save_panel(dict(panel, in_view=1), code=400)
            save_panel(dict(panel, selected_only="true"), code=400)
            save_panel(dict(panel, query=dict(bbox_um=["0","0","1","1"], state_rev="1", cursor=dict(check="0",error="0"))), code=400)
            save_panel(dict(panel, query=dict(bbox_um=["0","0","1","1"], state_rev="999999",
                                              cursor=dict(check="0",error="0"))), code=400)
            assert client.call("GET", panel_path) == fresh_panel, "invalid state committed"
            saved = save_panel(panel)
            assert saved["state"] == dict(panel_rev="2", body=panel)
            assert save_panel(panel) == saved, "uncertain retry was not idempotent"
            assert client.call("GET", panel_path) == saved, "state did not survive a fresh HTTP read"
            assert save_panel(dict(panel, markers=False), code=409)["error"] == "drc_panel_conflict"
            changed = save_panel(dict(panel, markers=False), base="2")
            assert changed["state"]["panel_rev"] == "3"
            assert save_panel(panel, base="2", code=409)["error"] == "drc_panel_conflict"
            assert client.call("GET", panel_path) == changed
            assert fingerprint(data) == before, "panel state wrote review files"

            def read(body, code=200, ctx=None):
                v = client.call("POST", endpoint, dict(ctx or context, body=body), code)
                assert str(data) not in json.dumps(v), "server leaked file paths"
                return v

            rules = dict(kind="rules", start="0", search="", limit=2)
            for key in ("view_id", "revision"):
                read(rules, 409, dict(context, **{key: "stale"}))
            read(dict(rules, path=str(db)), 400)
            read(dict(rules, start="00"), 400)
            read(dict(rules, limit=65), 400)
            view_state = client.call("GET", "/api/v1/view")["view"]
            focused = dict(context, state_rev=view_state["state_rev"])
            selection_path = panel_path.removesuffix("panel") + "selection"
            selection_rev = validate_selection(client, selection_path, endpoint, context, expected, view_state)
            selection_rev = validate_filters(client, selection_path, endpoint, context, expected, view_state, selection_rev)
            assert client.call("GET", panel_path) == changed, "groups changed panel settings"
            assert fingerprint(data) == before, "groups changed review files"
            ci = next(i for i, c in enumerate(expected) if c["errors"])
            first_error = expected[ci]["errors"][0]
            focus_request = dict(kind="focus", check=str(ci), error=first_error["local"], fit=True)
            read(focus_request, 400)
            read(focus_request, 409, dict(focused, state_rev="99999999"))
            for fit in (True, False):
                response = read(dict(focus_request, fit=fit), ctx=focused)
                b = first_error["bbox"]
                width = max((b[2]-b[0])/.3, (b[3]-b[1])/.3 * 257/191) if fit else (
                    float(view_state["bbox_dbu"][2])-float(view_state["bbox_dbu"][0]))*float(view_state["dbu_um"])
                width = width if width > 0 else .1
                assert math.isclose(float(response["navigation"]["width_um"]), width, rel_tol=1e-12)
                assert list(map(float, response["navigation"]["center_um"])) == [(b[0]+b[2])*.5, (b[1]+b[3])*.5]
            in_view = read(dict(kind="in_view", cursor=dict(check="0", error="0"), waived=None, limit=64), ctx=focused)
            box = list(map(float, in_view["bbox_um"]))
            assert box == [float(v)*float(view_state["dbu_um"]) for v in view_state["bbox_dbu"]]
            wanted = [e["glob"] for c in expected for e in c["errors"]
                      if e["bbox"][0] <= box[2] and e["bbox"][2] >= box[0]
                      and e["bbox"][1] <= box[3] and e["bbox"][3] >= box[1]]
            assert [r["global"] for r in in_view["rows"]] == wanted[:64]
            found = []
            while True:
                page = read(rules)
                found.extend(page["rows"])
                if page["next"] is None:
                    break
                assert page["next"] != rules["start"]
                rules["start"] = page["next"]
            assert len(found) == len(expected)
            for ci, (row, exp) in enumerate(zip(found, expected)):
                assert row["name"] == exp["name"] and not row["name_truncated"]
                assert row["errors"] == str(len(exp["errors"])) and row["waived"] == str(exp["waived"])
                detail = read(dict(kind="rule", check=str(ci)))
                assert detail["description"] == exp["desc"]
                for waived in (None, True, False):
                    rows, start = [], "0"
                    while True:
                        page = read(dict(kind="errors", check=str(ci), start=start, waived=waived, limit=7))
                        rows.extend(page["rows"])
                        if page["next"] is None:
                            break
                        assert page["next"] != start
                        start = page["next"]
                    wanted = [e for e in exp["errors"] if waived is None or (e["status"] == 1) == waived]
                    assert [r["global"] for r in rows] == [e["glob"] for e in wanted]
                    for r, e in zip(rows, wanted):
                        assert r["status"] == e["status"] and r["kind"] == e["kind"]
                        assert list(map(float, r["bbox_um"])) == list(e["bbox"])
                for e in exp["errors"]:
                    points, start = [], "0"
                    while True:
                        page = read(dict(kind="geometry", check=str(ci), error=e["local"], start=start, limit=511))
                        if ascii:
                            assert "points_dbu" not in page
                            points.extend([list(map(float, xy)) for xy in page["points_um"]])
                        else:
                            assert "points_um" not in page
                            points.extend([[int(x)/float(page["precision"]), int(y)/float(page["precision"])] for x, y in page["points_dbu"]])
                        if page["next"] is None:
                            break
                        assert page["next"] != start
                        start = page["next"]
                    assert points == [list(pt) for pt in e["pts"]]
                    measured = read(dict(kind="measurements", check=str(ci), error=e["local"]))
                    assert (measured["check"], measured["local"], measured["global"]) == (str(ci), e["local"], e["glob"])
                    rulers = drc.cd_segments(drc.DrcError(e["kind"], int(e["glob"]), e["pts"]))
                    assert len(measured["segments"]) == len(rulers) <= 3, (e, rulers, measured)
                    for segment, ruler in zip(measured["segments"], rulers):
                        actual = [float(v) for p in segment["endpoints_um"] for v in p]
                        assert all(math.isclose(a, b, rel_tol=1e-10, abs_tol=1e-11) for a, b in zip(actual, ruler)), (e, actual, ruler)
                        wanted = math.hypot(ruler[2]-ruler[0], ruler[3]-ruler[1])
                        assert math.isclose(float(segment["distance_um"]), wanted, rel_tol=1e-9, abs_tol=1e-11), (e, segment, ruler)
                        assert segment["offset"] is (e["kind"] == 'e' and len(e["pts"]) == 2)
            read(dict(kind="measurements", check="0", error="999999999999"), 400)
            read(dict(kind="measurements", check="00", error="0"), 400)
            read(dict(kind="measurements", check="0", error="0", path=str(db)), 400)
            search = read(dict(kind="rules", start="0", search="mask<", limit=64))
            assert [r["name"] for r in search["rows"]] == ["MASK<&>"]
            # A navigation step crosses pages/64-record blocks and wraps in
            # ONE rule, with the same waive/spatial predicate as the list.
            # Neither the hit metadata nor a focus request includes coordinates.
            for ci, exp in enumerate(expected):
                count = len(exp["errors"])
                anchors = [None] + sorted({i for i in (0, 62, 63, 64, 128, count-1) if 0 <= i < count})
                for backwards in (False, True):
                    for waived in (None, False, True):
                        for after in anchors:
                            for box in (None, [0, 0, .03, .01]):
                                request = dict(kind="step", check=str(ci), backwards=backwards,
                                               after=None if after is None else str(after), cursor=None,
                                               waived=waived, bbox_um=None if box is None else list(map(str, box)))
                                page = read(request)
                                assert page["next"] is None and int(page["scanned"]) <= count
                                start = (count-1 if backwards else 0) if after is None else after + (-1 if backwards else 1)
                                indices = [((start-i) if backwards else (start+i)) % count for i in range(count)]
                                wanted = next((exp["errors"][i] for i in indices
                                    if (waived is None or (exp["errors"][i]["status"] == 1) == waived)
                                    and (box is None or (exp["errors"][i]["bbox"][0] <= box[2]
                                    and exp["errors"][i]["bbox"][2] >= box[0]
                                    and exp["errors"][i]["bbox"][1] <= box[3]
                                    and exp["errors"][i]["bbox"][3] >= box[1]))), None)
                                assert (page["hit"] is None) == (wanted is None), (request, page)
                                if wanted:
                                    h = page["hit"]
                                    assert (h["check"], h["local"], h["global"], h["status"]) == (
                                        str(ci), wanted["local"], wanted["glob"], wanted["status"])
                                    assert h["points"] == str(len(wanted["pts"])) and "points_dbu" not in h
                                    assert list(map(float, h["bbox_um"])) == list(wanted["bbox"])
            mask = next(i for i, c in enumerate(expected) if c["name"] == "MASK<&>")
            step = dict(kind="step", check=str(mask), backwards=False, after=None,
                        cursor=dict(next="63", remaining="2"), waived=None, bbox_um=None)
            assert read(step)["hit"]["local"] == "63"
            for invalid in (dict(next="130", remaining="1"), dict(next="0", remaining="131"),
                            dict(next="0", remaining="0"), dict(next="00", remaining="1")):
                read(dict(step, cursor=invalid), 400)
            read(dict(step, after="62"), 400)
            read(dict(step, bbox_um=["0", "0", "NaN", "1"]), 400)
            for box in ([-1e6, -1e6, 1e6, 1e6], [0, 0, 0.03, 0.01], [900, 900, 901, 901]):
                for waived in (None, True, False):
                    cursor = dict(check="0", error="0")
                    rows = []
                    while True:
                        page = read(dict(kind="query", bbox_um=list(map(str, box)), checks=None,
                                         waived=waived, cursor=cursor, limit=7))
                        rows.extend(page["rows"])
                        if page["next"] is None:
                            break
                        assert cursor != page["next"]
                        cursor = page["next"]
                    wanted = [e["glob"] for c in expected for e in c["errors"]
                              if (waived is None or (e["status"] == 1) == waived)
                              and e["bbox"][0] <= box[2] and e["bbox"][2] >= box[0]
                              and e["bbox"][1] <= box[3] and e["bbox"][3] >= box[1]]
                    assert [r["global"] for r in rows] == wanted
            assert fingerprint(data) == before, "HTTP reads changed DRC files"
            view = client.call("GET", "/api/v1/view")["view"]
            assert view["state_rev"] == "1", "DRC reads changed render state"
            # New view invalidates previous read context, including same source.
            client.call("DELETE", "/api/v1/views/" + context["view_id"], code=202)
            wait(lambda: client.call("GET", "/api/v1/view")["view"]["status"] == "closed", proc)
            startup["seq"] = "2"
            client.call("POST", "/api/v1/operations", startup, 202)
            reopened = client.finished(2, proc)
            assert reopened["phase"] == "succeeded", reopened
            read(rules, 409)
            client.call("GET", panel_path, code=409)
            save_panel(panel, base="3", code=409)
            client.call("GET", selection_path, code=409)
            client.call("POST", selection_path, dict(revision=catalog["revision"], base_selection_rev=str(selection_rev), body=dict(kind="clear_all")), 409)
            fresh_path = panel_path.replace(opened["view_id"], reopened["view_id"])
            assert client.call("GET", fresh_path)["state"] == dict(panel_rev="1", body=None), "new view inherited old panel"
            new_selection = selection_path.replace(opened["view_id"], reopened["view_id"])
            empty_groups = client.call("GET", new_selection)
            assert empty_groups["state"] == dict(selection_rev="1", total="0", limit=5000, rules=[])
            context["view_id"] = reopened["view_id"]
            # External truncation gives a safe, path-free error, not SIGBUS.
            (db if ascii else packed).write_bytes(b"truncated")
            assert read(dict(kind="rule", check="0"), 422)["error"] == "drc_changed_or_corrupt"
            client.call("POST", new_selection, dict(revision=catalog["revision"], base_selection_rev="1", body=dict(kind="apply", check=str(chosen), errors=["0"], mode="toggle")), 422)
            assert client.call("GET", new_selection) == empty_groups, "corrupt-pack selection committed"
            client.call("DELETE", "/api/v1/session", code=204)
            out, err = proc.communicate(timeout=15)
            assert proc.returncode == 0, (out, err)
            assert client.token not in out + err
            assert not session_path.exists() and not list(temps.iterdir())
        finally:
            if proc.poll() is None:
                proc.send_signal(signal.SIGINT)
                try:
                    proc.communicate(timeout=15)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.communicate(timeout=5)
    print("WEB DRC: ALL OK (auth, scoped IDs, pages, coordinates/CD, waive, queries, circular step, focus/in-view, panel CAS/replay, stale view, read-only, cancellation/reap)")


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
    main(Path(sys.argv[1]).resolve(), ascii=True)
