#!/usr/bin/env python3
"""Runtime SVRF replacement through the real picker/DRC actors, private fixtures."""
import copy
import json
from pathlib import Path
from cache_test_paths import vfs_cache, drc_pack
import shutil
import subprocess
import sys
import tempfile

from validate_web_cli import INDEX
from validate_web_drc_notes import API, Session, fingerprint
from validate_web_drc_open import Picker, idle
from validate_web_drc_build import DB, done, request as build_request, drc_ready


def metadata(metric="width", bound=0.5, name="WIDTH"):
    return dict(format="floe-svrf-rules", version=1, checks={name: dict(
        desc="<literal> metadata", constraints=[dict(metric=metric, op="<", value=bound,
                                                     text=metric + " < " + str(bound))])},
        deck="/not-approved/never-open.svrf", stats=dict(includes=["/not-approved/never-open.inc"]))


def replacement_budget(source, db, rules, invalid, temps, work):
    """Keep the old metadata/reader; reclaim only an idle note display."""
    inputs = [source, db, drc_pack(db), rules, invalid]
    inputs += [p for p in vfs_cache(source).rglob("*") if p.is_file()]
    before = fingerprint(inputs)
    refs = [dict(check="0", error="0")]
    # 2048 MiB pool: picker 192 + reader 256 + old/new metadata 256 each
    # leaves 1088 for rendering. Idle display must not retain another 256.
    for budget, accepted in ((1024, True), (1088, True), (1089, False)):
        s = Session(source, drc_pack(db), temps, "rules-budget", work / ("budget-%d.session" % budget),
                    budget_mb=budget, rules=rules)
        try:
            c, picker = s.client, Picker(s, source.parent.name)
            view, prior = idle(s), c.call("GET", "/api/v1/drc")
            current = prior["drc"]
            handle = picker.handle(rules.name, "all_files")
            invalid_handle = picker.handle(invalid.name, "all_files")

            def load(selected):
                return picker.finish(picker.submit("load_drc_rules", handle=selected, context=s.context))

            if accepted:
                assert not s.display(refs)["cache_hit"]
                assert s.display(refs)["cache_hit"]
                note = s.read(refs)
                rejected = load(handle)
                assert rejected["phase"] == "failed" and rejected["error"] == "browse_busy_or_limit", rejected
                assert c.call("GET", "/api/v1/drc")["drc"] == current
                draft = s.prepare(note, "unpublished draft survives metadata rejection")
                c.call("POST", API + "/revoke", dict(token=draft["token"]), 204)
                assert not s.display(refs)["cache_hit"]
                assert s.display(refs)["cache_hit"]
                rejected = load(invalid_handle)
                assert rejected["phase"] == "failed" and rejected["error"] == "browse_invalid_selection", rejected
                assert c.call("GET", "/api/v1/drc")["drc"] == current
                # Failed preparation releases the gate; display may repopulate
                # and the next successful candidate must reclaim it again.
                assert not s.display(refs)["cache_hit"]
                assert s.display(refs)["cache_hit"]
            else:
                assert s.display(refs, code=429)["error"] == "review_busy"
            result = load(handle)
            if accepted:
                replaced = picker.accept(result)
                assert replaced["id"] == current["id"] and replaced["revision"] != current["revision"]
                assert replaced["metadata"] == current["metadata"]
                assert result["result"]["metadata_replaced"]
                assert not s.display(refs)["cache_hit"]
                assert s.display(refs)["cache_hit"]
            else:
                assert result["phase"] == "failed" and result["error"] == "browse_busy_or_limit", result
                assert c.call("GET", "/api/v1/drc")["drc"] == current
            after = c.call("GET", "/api/v1/drc")
            assert after["review_grant"] == prior["review_grant"]
            assert after["notes"]["binding_id"] == prior["notes"]["binding_id"]
            assert not after["notes"]["detached"]
            assert idle(s) == view and fingerprint(inputs) == before
            assert not list(db.parent.glob(".*.notes.*")) and not list(db.parent.glob(".*.waive.*"))
        finally:
            s.close()


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-runtime-svrf-") as td:
        work = Path(td).resolve()
        layout, data, temps = [work / name for name in ("layout", "drc", "temps")]
        for directory in (layout, data, temps):
            directory.mkdir()
        source = layout / "synthetic.oas"
        shutil.copy2(fixture, source)
        subprocess.run([str(INDEX), "vfs", str(source), str(vfs_cache(source)), "--jobs", "2"],
                       check=True, capture_output=True, timeout=60)
        a, b, empty, bad, huge = [layout / name for name in (
            "한 글.rules.json", "space.rules.json", "unmatched.json", "invalid.json", "huge.json")]
        a.write_text(json.dumps(metadata()))
        b.write_text(json.dumps(metadata("space", 0.7)))
        empty.write_text(json.dumps(metadata(name="NOT_IN_THIS_DRC")))
        bad.write_text('{"format":"floe-svrf-rules","version":2,"checks":{}}')
        huge.write_bytes(b" " * (16 * 1024 * 1024 + 1))
        protected = [source, a, b, empty, bad, huge] + [p for p in vfs_cache(source).rglob("*") if p.is_file()]
        before = fingerprint(protected)
        refs = [dict(check="0", error="0")]
        budget_db = data / "budget.db"
        budget_db.write_text(DB)
        subprocess.run([str(INDEX), "drc", str(budget_db), "--jobs", "2"],
                       check=True, capture_output=True, timeout=30)
        replacement_budget(source, budget_db, a, bad, temps, work)
        for mode in ("ascii", "writer"):
            db = data / (mode + ".db")
            db.write_text(DB)
            pack = drc_pack(db)
            if mode == "writer":
                subprocess.run([str(INDEX), "drc", str(db), "--jobs", "2"], check=True, capture_output=True, timeout=30)
            s = Session(source, pack if mode == "writer" else db, temps,
                        "rules-reviewer" if mode == "writer" else None, work / (mode + ".session"),
                        edit_waives=mode == "writer")
            try:
                c, picker = s.client, Picker(s, layout.name)
                view = idle(s)
                initial = c.call("GET", "/api/v1/drc")

                def read(body, context=None, code=200):
                    ctx = context or s.context
                    result = c.call("POST", "/api/v1/drc/" + ctx["drc_id"] + "/read",
                                    dict(view_id=ctx["view_id"], revision=ctx["revision"], body=body), code)
                    assert str(work) not in json.dumps(result) and "/not-approved" not in json.dumps(result)
                    return result

                def load(path, context=None, handle=None):
                    return picker.finish(picker.submit("load_drc_rules", handle=handle or picker.handle(path.name, "all_files"),
                                                       context=copy.deepcopy(context or s.context)))

                def accept(result):
                    old = copy.deepcopy(s.context)
                    current = picker.accept(result)
                    assert result["result"]["metadata_replaced"]
                    assert current["id"] == old["drc_id"] and current["revision"] != old["revision"]
                    read(dict(kind="types", start="0", limit=64), old, 409)
                    return current

                saved_request = saved_result = stale = None
                if mode == "writer":
                    draft = s.prepare(s.read(refs), "saved before metadata replacement")
                    saved_request = copy.deepcopy(s.request(draft, 1))
                    c.call("POST", API, saved_request, 202)
                    saved_result = s.finished(1)
                    assert saved_result["phase"] == "succeeded", saved_result
                    stale = copy.deepcopy(s.request(s.prepare(s.read(refs), "must not publish"), 2))
                old = copy.deepcopy(s.context)
                current = accept(load(a))
                assert current["metadata"]["svrf"] == dict(matched="1", checks="1", type_count="1")
                assert read(dict(kind="types", start="0", limit=64))["rows"] == [dict(metric="width", checks="1")]
                assert read(dict(kind="comparison", check="0", error="0"))["comparison"]["bound"] == "0.5"
                after = c.call("GET", "/api/v1/drc")
                assert after["review_grant"] == initial["review_grant"]
                if mode == "writer":
                    for kind in ("notes", "waives"):
                        assert after[kind]["binding_id"] == initial[kind]["binding_id"]
                    c.call("POST", API, stale, 409)
                    c.call("POST", API, dict(stale, context=s.context), 410)
                    assert c.call("POST", API, saved_request, 202) == saved_result
                    assert s.display(refs, refs[0])["focus"]["text"] == "saved before metadata replacement"
                assert idle(s) == view

                handle = picker.handle(a.name, "all_files")
                for extra in (dict(path=str(a)), dict(reviewer="other"), dict(editable=True)):
                    c.call("POST", "/api/v1/browse", dict(kind="load_drc_rules", seq="999", handle=handle,
                                                          context=s.context, **extra), 400)
                c.call("POST", "/api/v1/browse", dict(kind="load_drc_rules", seq="999", handle=str(a), context=s.context), 409)
                for rejected in (load(b, old), load(bad), load(huge)):
                    assert rejected["phase"] == "failed", rejected
                    assert c.call("GET", "/api/v1/drc")["drc"] == current, "failed load changed revision/metadata"
                changed = layout / "changed.rules.json"
                changed.write_text(json.dumps(metadata()))
                obsolete = picker.handle(changed.name, "all_files")
                replacement = layout / "replacement.rules.json"
                replacement.write_text(json.dumps(metadata("space")))
                replacement.replace(changed)
                assert load(changed, handle=obsolete)["phase"] == "failed"
                assert c.call("GET", "/api/v1/drc")["drc"] == current

                # Cancellation races a tiny valid read: either committed success
                # or the exact prior snapshot, never a relabelled partial commit.
                for _ in range(4):
                    prior = c.call("GET", "/api/v1/drc")["drc"]
                    req = picker.submit("load_drc_rules", handle=picker.handle(b.name, "all_files"), context=copy.deepcopy(s.context))
                    c.call("POST", "/api/v1/browse/" + req["seq"] + "/cancel", {})
                    result = picker.finish(req)
                    if result["phase"] == "succeeded":
                        accept(result)
                    else:
                        assert result["phase"] == "cancelled", result
                        assert c.call("GET", "/api/v1/drc")["drc"] == prior
                accept(load(b))
                assert read(dict(kind="types", start="0", limit=64))["rows"] == [dict(metric="space", checks="1")]
                assert read(dict(kind="rule", check="0"))["svrf"]["rule"]["constraints"][0]["value"] == "0.7"
                no_match = accept(load(empty))
                assert no_match["metadata"]["svrf"]["matched"] == "0"
                accept(load(b))
                assert idle(s) == view
                if mode == "ascii":
                    assert not pack.exists(), "metadata selection indexed implicitly"
                    # Metadata outside the original DRC scope must survive an
                    # explicitly approved build, without widening pack roots.
                    request = build_request(c.call("GET", "/api/v1/drc")["drc"], s.context["view_id"], 1)
                    c.call("POST", "/api/v1/drc/builds", request, 202)
                    assert done(c, s.proc, 1)["phase"] == "succeeded"
                    built = drc_ready(c, s.proc)
                    s.context.update(drc_id=built["id"], revision=built["revision"])
                    assert read(dict(kind="types", start="0", limit=64))["rows"] == [dict(metric="space", checks="1")]
                else:
                    # Opening still drops metadata/reviewer authority. A later
                    # explicit reconnect must preserve newly loaded metadata.
                    db2 = layout / "second.db"
                    db2.write_text(DB)
                    subprocess.run([str(INDEX), "drc", str(db2), "--jobs", "2"], check=True, capture_output=True, timeout=30)
                    picker.accept(picker.open(picker.handle(db2.name)))
                    assert c.call("GET", "/api/v1/drc")["notes"]["detached"]
                    accept(load(a))
                    assert c.call("GET", "/api/v1/drc")["notes"]["detached"], "metadata granted reviewer rights"
                    picker.accept(picker.reconnect())
                    assert read(dict(kind="types", start="0", limit=64))["rows"] == [dict(metric="width", checks="1")]
                    assert c.call("POST", API, saved_request, 202) == saved_result
                assert idle(s) == view
                assert fingerprint(protected) == before, "metadata selection modified protected inputs"
            finally:
                s.close()
        print("WEB RUNTIME SVRF: ALL OK (ASCII/ICE, atomic replace/failure, same reader, stale/cancel/replay, 1024/1088/1089 MiB admission, idle display vs active editor, bounds, witness, no authority expansion, saved receipts, build/reconnect persistence, camera/input invariant)")


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
