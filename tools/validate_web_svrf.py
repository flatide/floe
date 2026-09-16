#!/usr/bin/env python3
"""Authenticated SVRF API parity, synthetic/private files and native runtime."""
from collections import Counter
import json
import math
import os
from pathlib import Path
from cache_test_paths import vfs_cache, drc_pack
import shutil
import signal
import subprocess
import sys
import tempfile

from validate_web_cli import APP, INDEX, RENDERD, ROOT, Client, read_json, wait
from validate_app_svrf import custom_fixture, METRICS, run
from validate_app_drc import fingerprint
from floe import drc, svrf


def fixtures(work, ascii=False):
    data = work / "drc"
    data.mkdir()
    metadata = work / "metadata"
    metadata.mkdir()
    db, deck = custom_fixture(data, fractional=ascii)
    text = db.read_text()
    head, body = text.split("\n", 1)
    # More than 4096 rule slots: type filtering may yield empty+continuation,
    # and every limit must apply AFTER the metric/name/status intersection.
    prefix = "".join("PREFIX.%d\n0 0 1\nzero-result\n" % i for i in range(4200))
    pts = ([(x, 0) for x in range(4096)] + [(4096, y) for y in range(4096)]
           + [(x, 4096) for x in range(4096, 0, -1)] + [(0, y) for y in range(4096, 0, -1)])
    body += "BIGAREA\n1 1 1\nlarge polygon\np 1 %d\n" % len(pts)
    body += "".join("%d %d\n" % p for p in pts)
    body += "HUGE_META\n0 0 1\nbounded response case\n"
    db.write_text(head + "\n" + prefix + body)
    meta = svrf.parse_deck(str(deck)).to_json()
    for i in range(80):
        meta["checks"]["PREFIX.%d" % i] = dict(desc="custom metric", layers=[], source_gds=[],
            constraints=[dict(metric="custom%02d" % i, op="<", value=None, text="unknown bound")])
    meta["checks"]["BIGAREA"] = dict(desc="large area", layers=["M1"], source_gds=[[7, None]],
        constraints=[dict(metric="area", op="<=", value=100.0, text="AREA M1 <= 100")])
    meta["checks"]["HUGE_META"] = dict(desc="large metadata", layers=["H0"], source_gds=[], constraints=[])
    for i in range(6):
        meta["derived"]["H%d" % i] = "\x01"*65530 + " H%d" % (i+1)
    meta["deck"] = str(work / "not-authorized" / "never-open-this.svrf")
    meta["stats"]["includes"] = [str(work / "not-authorized" / "never-open-this.inc")]
    rules = metadata / "한 글.rules.json"
    rules.write_text(json.dumps(meta))
    empty = metadata / "empty.rules.json"
    empty.write_text(json.dumps(dict(format="floe-svrf-rules", version=1, checks={})))
    invalid = metadata / "future.rules.json"
    invalid.write_text(json.dumps(dict(format="floe-svrf-rules", version=2, checks={})))
    if not ascii:
        run([INDEX, "drc", db, "--jobs", "2"])
    os.environ["FLOE_REVIEWER"] = "svrf-http"
    p = drc.load_ascii(str(db)) if ascii else drc.IcePack(str(drc_pack(db)))
    expected = []
    for ci, c in enumerate(p.checks):
        if not ascii:
            for ei, _ in enumerate(c.errors):
                p.set_status(ci, ei, 1 if ei % 3 == 0 else 2 if ei % 5 == 0 else 0)
        rule = meta["checks"].get(c.name)
        types = set(x["metric"] for x in (rule or {}).get("constraints", []) if x["metric"]) or {"other"}
        expected.append(dict(name=c.name, count=len(c.errors), waived=0 if ascii else p.status_counts(ci)[0], types=types))
    waive = None if ascii else Path(p._waive_path)
    if not ascii:
        p.close()
    return db, rules, empty, invalid, waive, meta, expected


def main(fixture, ascii=False):
    with tempfile.TemporaryDirectory(prefix="floe-web-svrf-") as td:
        work = Path(td)
        layout = work / "layout"
        layout.mkdir()
        source = layout / "design.oas"
        shutil.copy2(fixture, source)
        run([INDEX, "vfs", source, str(vfs_cache(source)), "--jobs", "2"])
        db, rules, empty, invalid, waive, meta, expected = fixtures(work, ascii)
        registered = db if ascii else str(drc_pack(db))
        temps = work / "temp"
        temps.mkdir()
        env = dict(os.environ, PATH="", TMPDIR=str(temps), FLOE_INDEX_BIN=str(INDEX), FLOE_RENDERD_BIN=str(RENDERD))
        session = work / "session.json"
        base = [APP, "view", source, "--no-open", "--drc", registered,
                "--jobs", "2", "--raster-jobs", "1", "--no-labels", "--frame-cache", "off",
                "--session-file", session]
        if waive is not None:
            base += ["--drc-waives", waive]
        # Explicit metadata must not broaden layout/TC roots or bypass shared
        # admission. Failure happens before publishing a session credential.
        rejected = run(base + ["--drc-rules", rules, "--budget-mb", "1537"], env, False)
        assert "managed resource" in rejected.stderr and not session.exists()
        outside = rules.parent / "outside.oas"
        shutil.copy2(source, outside)
        escape = layout / "escape.jb"
        escape.write_text("MTITLE 1,Mask\nCHIP C\n$ (1,PATTERN,TC='../metadata/outside.oas',AD=0.001,LY={1},DT={0},UX=100,UY=100)\nROWS 0/0\n")
        denied = run([APP,"view",escape,"--no-open","--drc",registered,"--drc-rules",rules,"--session-file",session],env,False)
        assert "outside approved roots" in denied.stderr and not session.exists()
        checks = 0
        for chosen in (None, empty, rules, invalid):
            args = base + ([] if chosen is None else ["--drc-rules", chosen])
            proc = subprocess.Popen(list(map(str, args)), env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                client = Client(wait(lambda:read_json(session),proc))
                client.call("GET","/api/v1/drc",code=401)
                client.login()
                catalog = wait(lambda:(lambda d:d if d["phase"] in ("ready","error") else None)(client.call("GET","/api/v1/drc")["drc"]),proc)
                assert str(work) not in json.dumps(catalog)
                if chosen == invalid:
                    assert catalog["phase"] == "error"
                    client.call("DELETE","/api/v1/session",code=204)
                    assert proc.wait(timeout=8) == 0
                    continue
                assert catalog["phase"] == "ready", catalog
                startup = client.call("GET","/api/v1/startup")["request"]
                startup["body"]["pixels"] = [257,191]
                client.call("POST","/api/v1/operations",startup,202)
                opened = client.finished(1,proc)
                assert opened["phase"] == "succeeded", opened
                view = wait(lambda:(lambda v:v if v["status"] == "idle" else None)(client.call("GET","/api/v1/view")["view"]),proc)
                ctx = dict(view_id=opened["view_id"], revision=catalog["revision"])
                endpoint = "/api/v1/drc/"+catalog["id"]+"/read"
                anonymous = Client(read_json(session))
                for body in (dict(kind="types",start="0",limit=7),dict(kind="comparison",check="4200",error="0")):
                    anonymous.call("POST",endpoint,dict(ctx,body=body),401)
                def read(body, code=200, context=None):
                    result = client.call("POST",endpoint,dict(context or ctx,body=body),code)
                    assert str(work) not in json.dumps(result), "metadata provenance path leaked"
                    return result
                counts = Counter()
                if chosen == rules:
                    for c in expected:
                        counts.update(c["types"])
                ordered = [m for m in METRICS if m in counts] + sorted(set(counts)-set(METRICS))
                pages, start = [], "0"
                while True:
                    page = read(dict(kind="types",start=start,limit=7))
                    assert page["available"] is (chosen is not None)
                    assert page["total"] == str(len(counts))
                    pages.extend(page["rows"])
                    if page["next"] is None:
                        break
                    assert int(page["next"]) > int(start)
                    start = page["next"]
                assert pages == [dict(metric=m,checks=str(counts[m])) for m in ordered]
                if chosen is not None:
                    summary = catalog["metadata"]["svrf"]
                    assert summary == dict(matched=str(sum(c["name"] in meta["checks"] for c in expected) if chosen == rules else 0), checks=str(len(expected)), type_count=str(len(counts)))
                else:
                    assert catalog["metadata"]["svrf"] is None
                # Type choice belongs to this server panel revision. It never
                # changes the layout view or writes the DRC review sidecar.
                panel_path = "/api/v1/drc/"+catalog["id"]+"/views/"+opened["view_id"]+"/panel"
                panel = dict(search="",metric=None,rule_start="0",check=None,error_start="0",query=None,
                    in_view=False,selected_only=False,waived=None,selected=None,markers=True,shown=True,
                    jump_scale=None,zoom_lock=False,jump_active=False,focus_visible=False,cd=None)
                def save_panel(data, base="1", code=200):
                    return client.call("POST",panel_path,dict(revision=catalog["revision"],base_panel_rev=base,body=data),code)
                save_panel(dict(panel,metric=""),code=400)
                save_panel(dict(panel,metric="한"*22),code=400)
                save_panel(dict(panel,metric="not-a-metric"),code=400)
                if chosen == rules:
                    typed = dict(panel,metric="width")
                    assert save_panel(typed)["state"] == dict(panel_rev="2",body=typed)
                    assert client.call("GET",panel_path)["state"]["body"] == typed
                    save_panel(dict(typed,metric="area"),code=409)
                    assert save_panel(typed)["state"]["panel_rev"] == "2", "ambiguous retry changed revision"
                    assert save_panel(panel,base="2")["state"]["body"]["metric"] is None
                else:
                    save_panel(dict(panel,metric="width"),code=400)
                for body in [dict(kind="types",start="00",limit=7),dict(kind="types",start="99999",limit=7),
                             dict(kind="types",start="0",limit=65),dict(kind="comparison",check="00",error="0"),
                             dict(kind="comparison",check="0",error="0",path=str(rules)),
                             dict(kind="comparison",check="0",error="0",points=[[0,0],[1,1]])]:
                    read(body,400)
                for key in ("view_id","revision"):
                    read(dict(kind="types",start="0",limit=7),409,dict(ctx,**{key:"stale"}))
                before = fingerprint(db.parent)
                query_count = 0
                metrics = (None,"width","area","other","custom00","absent") if chosen is not None else (None,)
                for metric in metrics:
                    for waived in (None,False,True):
                        for search in ("","RANGE","prefix.41"):
                            wanted = [str(i) for i,c in enumerate(expected)
                                if (metric is None or chosen == rules and metric in c["types"])
                                and search.lower() in c["name"].lower()
                                and (waived is None or (c["waived"]>0 if waived else c["count"]>c["waived"]))]
                            found, start, incomplete = [], "0", False
                            while True:
                                page = read(dict(kind="rules",start=start,search=search,limit=64,metric=metric,waived=waived))
                                assert page["metric"] == metric and page["waived"] is waived
                                found += [r["check"] for r in page["rows"]]
                                if page["next"] is None:
                                    break
                                assert int(page["next"]) > int(start)
                                incomplete |= not page["rows"]
                                start = page["next"]
                            assert found == wanted, (chosen,metric,waived,search,found,wanted)
                            if chosen == rules and metric == "width" and not search and waived is None:
                                assert incomplete, "empty continuation was mistaken for no matching rules"
                            query_count += 1
                if chosen is None:
                    read(dict(kind="rules",start="0",search="",limit=64,metric="other"),400)
                selected_checks = [(i,c) for i,c in enumerate(expected) if i >= 4200]
                seen = set()
                for ci,c in selected_checks:
                    if c["name"] == "HUGE_META" and chosen == rules:
                        assert read(dict(kind="rule",check=str(ci)),413)["error"] == "drc_read_limit"
                        continue
                    detail = read(dict(kind="rule",check=str(ci)))
                    if chosen != rules or c["name"] not in meta["checks"]:
                        assert detail["svrf"] is None
                    else:
                        actual = detail["svrf"]["rule"]
                        oracle = meta["checks"][c["name"]]
                        assert actual["source_gds"] == [[str(l),None if d is None else str(d)] for l,d in oracle["source_gds"]]
                        assert len(actual["constraints"]) == len(oracle["constraints"])
                        for a,b in zip(actual["constraints"],oracle["constraints"]):
                            assert a["metric"] == b["metric"] and a["op"] == b["op"] and a["text"] == b["text"] and a["raw"] == b.get("raw")
                            if b["value"] is None:
                                assert a["value"] is None
                            else:
                                assert isinstance(a["value"],str) and float(a["value"]) == b["value"]
                    if c["name"] in seen:
                        continue
                    seen.add(c["name"])
                    compared = json.loads(run([APP,"drc",registered,"--errs",c["name"],"--svrf-rules",rules],env).stdout)
                    for ei,row in enumerate(compared):
                        actual = read(dict(kind="comparison",check=str(ci),error=str(ei)))
                        assert (actual["check"],actual["local"],actual["global"]) == (str(ci),str(ei),str(row["global"]))
                        expected_value = row["comparison"] if chosen == rules else None
                        got = actual["comparison"]
                        if expected_value is None:
                            assert got is None
                        else:
                            assert set(got) == set(expected_value)
                            for k,v in expected_value.items():
                                if isinstance(v,(float,int)):
                                    assert isinstance(got[k],str) and math.isclose(float(got[k]),v,rel_tol=1e-12,abs_tol=1e-12)
                                else:
                                    assert got[k] == v
                        assert len(json.dumps(actual)) < 2048 and "points" not in actual
                        checks += 1
                after = client.call("GET","/api/v1/view")["view"]
                assert (after["state_rev"],after["submitted"]) == (view["state_rev"],view["submitted"]), "metadata reads caused a render"
                assert fingerprint(db.parent) == before, "read service modified pack/waives/deck"
                if chosen == rules:
                    # Explicit snapshot ownership: replacing the source after
                    # ready neither mixes revisions nor follows its new paths.
                    old = rules.read_bytes()
                    replacement = rules.with_name("replacement.rules.json")
                    replacement.write_text(empty.read_text())
                    os.replace(replacement,rules)
                    assert read(dict(kind="types",start="0",limit=7))["total"] == str(len(counts))
                    assert client.call("GET","/api/v1/drc")["drc"]["revision"] == catalog["revision"]
                    rules.write_bytes(old)
                client.call("DELETE","/api/v1/session",code=204)
                assert proc.wait(timeout=8) == 0
                assert not session.exists() and not list(temps.iterdir())
                print("WEB SVRF mode=%s: %d rule-filter cases" % ("none" if chosen is None else chosen.name,query_count))
            finally:
                if proc.poll() is None:
                    proc.send_signal(signal.SIGINT)
                    proc.wait(timeout=8)
                proc.communicate()
        assert checks > 2500, checks
        print("WEB SVRF: ALL OK (%d comparisons, scoped metadata/types/filters, no vertex transfer/render/write, limits/reap)" % checks)


if __name__ == "__main__":
    main(Path(sys.argv[1]))
    main(Path(sys.argv[1]), ascii=True)
