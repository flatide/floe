#!/usr/bin/env python3
"""Authenticated DRC read gate: actual Rust CLI, private synthetic inputs only."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile

from validate_web_cli import APP, INDEX, RENDERD, ROOT, Client, read_json, wait
from validate_app_drc import fingerprint
from validate_drc_ice import DB
from floe import drc


def main(fixture):
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
        temps = work / "temp"
        temps.mkdir()
        env = dict(os.environ, PATH="", TMPDIR=str(temps), FLOE_INDEX_BIN=str(INDEX),
                   FLOE_RENDERD_BIN=str(RENDERD), FLOE_DRC_WEB_PACK=str(packed))
        before = fingerprint(data)
        test = subprocess.run([tests[0], "--ignored", "--nocapture"], env=env,
                              capture_output=True, text=True, timeout=20)
        assert test.returncode == 0, (test.stdout, test.stderr)
        assert "RUST DRC ACTOR: ALL OK" in test.stdout
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
                "--drc", str(packed), "--drc-waives", str(side), "--jobs", "2", "--raster-jobs", "1",
                "--no-labels", "--frame-cache", "off"]
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
            client.login()
            assert client.call("GET", "/api/v1/capabilities")["drc"] is True
            catalog = wait(lambda: (lambda d: d if d["phase"] == "ready" else None)(
                client.call("GET", "/api/v1/drc")["drc"]), proc)
            assert catalog["read_only"] and catalog["metadata"]["waives"]
            assert catalog["metadata"]["checks"] == str(len(expected))
            startup = client.call("GET", "/api/v1/startup")["request"]
            startup["body"]["pixels"] = [257, 191]
            client.call("POST", "/api/v1/operations", startup, 202)
            opened = client.finished(1, proc)
            assert opened["phase"] == "succeeded", opened
            context = dict(view_id=opened["view_id"], revision=catalog["revision"])
            endpoint = "/api/v1/drc/" + catalog["id"] + "/read"

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
                        points.extend([[int(x)/float(page["precision"]), int(y)/float(page["precision"])] for x, y in page["points_dbu"]])
                        if page["next"] is None:
                            break
                        assert page["next"] != start
                        start = page["next"]
                    assert points == [list(pt) for pt in e["pts"]]
            search = read(dict(kind="rules", start="0", search="mask<", limit=64))
            assert [r["name"] for r in search["rows"]] == ["MASK<&>"]
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
            context["view_id"] = reopened["view_id"]
            # External truncation gives a safe, path-free error, not SIGBUS.
            packed.write_bytes(b"truncated")
            assert read(dict(kind="rule", check="0"), 422)["error"] == "drc_changed_or_corrupt"
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
    print("WEB DRC: ALL OK (auth, scoped IDs, pages, coordinates, waive, queries, stale view, read-only, cancellation/reap)")


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
