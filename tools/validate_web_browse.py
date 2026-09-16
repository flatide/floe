#!/usr/bin/env python3
"""Approved-root picker -> owner proposal -> real native open, private inputs only."""
import os
from pathlib import Path
from cache_test_paths import vfs_cache
import shutil
import subprocess
import sys
import tempfile

from validate_web_cli import APP, INDEX, RENDERD, Client, read_json, wait
from validate_web_handoff import digest


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="fwb.", dir="/tmp") as td:
        work = Path(td).resolve()
        designs, extra, outside, temps = [work / n for n in ("designs", "extra", "outside", "workers")]
        for directory in (designs, extra, outside, temps):
            directory.mkdir()
        a, b, missing = [designs / n for n in ("layout 한국.oas", "other.oas", "unindexed.oas")]
        for source in (a, b, missing):
            shutil.copy2(fixture, source)
        env = dict(os.environ, PATH="", TMPDIR=str(temps), FLOE_INDEX_BIN=str(INDEX), FLOE_RENDERD_BIN=str(RENDERD))
        for key in ("FLOE_FILL_EDIT", "FLOE_JOBDECK_LEVELS", "FLOE_FIREFOX_BIN"):
            env.pop(key, None)
        for source in (a, b):
            subprocess.run([str(APP), "index", str(source), "--jobs", "2"], env=env, check=True, capture_output=True, timeout=60)
        for n in range(300):
            (designs / f"row-{n:04}.oas").write_bytes(b"catalogue only")
        (designs / "bad.oas").write_bytes(b"not OASIS")
        (designs / ".private").write_bytes(b"hidden")
        (designs / "cache.ice").write_bytes(b"hidden")
        (designs / "folder").mkdir()
        (designs / "external").symlink_to(outside, target_is_directory=True)
        (designs / "internal-link.oas").symlink_to(a)
        deck = designs / "two.jb"
        deck.write_text("MTITLE 1,ONE\nMTITLE 2,TWO\nCHIP C\n"
                        "$ (1,A,TC=other.oas)\n$ (2,B,TC=other.oas)\nROWS 0/0\n")
        before = digest(designs)
        session_path = work / "session.json"
        proc = subprocess.Popen([str(APP), "view", "--multi", "--no-open", "--root", str(designs),
                                 "--root", str(extra), "--session-file", str(session_path), "--jobs", "2", "--raster-jobs", "1", "--perf-baseline"],
                                env=env, cwd=outside, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            client = Client(wait(lambda: read_json(session_path), proc))
            client.call("GET", "/api/v1/browse", code=401)
            client.call("GET", "/api/v1/browse/1", code=401)
            client.login()
            caps = client.call("GET", "/api/v1/capabilities")
            assert caps["file_picker"] and not caps["launcher"] and not caps["uploads"]
            assert client.call("GET", "/api/v1/catalog")["sources"] == []
            assert client.call("GET", "/api/v1/operations")["last_seq"] == "0"
            roots = client.call("GET", "/api/v1/browse")["roots"]
            assert len(roots) == 2 and str(work) not in str(roots)
            root = next(r["handle"] for r in roots if r["name"].endswith("designs"))
            client.call("GET", "/api/v1/browse?path=/etc", code=403)
            client.call("POST", "/api/v1/browse", dict(kind="list", seq="1", directory=root, filter="all_files", query="", path=str(outside)), 400)
            client.call("POST", "/api/v1/browse", dict(kind="list", seq="1", directory=str(outside), filter="all_files", query=""), 409)
            assert client.call("GET", "/api/v1/browse")["last_seq"] == "0"

            def browse(kind, **kwargs):
                seq = str(int(client.call("GET", "/api/v1/browse")["last_seq"]) + 1)
                request = dict(kind=kind, seq=seq, **kwargs)
                client.call("POST", "/api/v1/browse", request, 202)
                terminal = wait(lambda: (lambda v: v if v["phase"] in ("succeeded", "failed", "cancelled") else None)(
                    client.call("GET", "/api/v1/browse/" + seq)), proc)
                assert terminal["request"] == request
                assert client.call("POST", "/api/v1/browse", request, 202) == terminal
                assert str(work) not in str(terminal), "server path leaked"
                return terminal

            def listing(query="", filter="all_files"):
                v = browse("list", directory=root, query=query, filter=filter)
                assert v["phase"] == "succeeded", v
                return v["result"]["page"]

            first = listing()
            assert first["rows"][0]["kind"] == "directory"
            assert first["skipped_links"] == 2
            rows = list(first["rows"])
            cursor = first
            while cursor["next"] is not None:
                cursor = browse("page", snapshot=first["snapshot"], start=cursor["next"])["result"]["page"]
                rows.extend(cursor["rows"])
            expected = {p.name for p in designs.iterdir() if not p.is_symlink()
                        and not p.name.startswith(".") and not p.name.lower().endswith((".floe", ".ice"))}
            # Native indexing may leave a regular *.floe.lock, which GTK's All
            # files filter also displays; it is not an extra source or cache dir.
            assert len(rows) == first["total"] == len(expected), (len(rows), first["total"], expected)
            assert {r["name"] for r in rows} == expected
            assert len(set(r["name"] for r in rows)) == len(rows)
            assert not any(r["name"].startswith(".") or r["name"].endswith((".floe", ".ice")) for r in rows)
            assert [r["name"] for r in listing(filter="jobdecks")["rows"]] == ["folder", "two.jb"]

            def select(name):
                found = listing(query=name)
                matching = [r for r in found["rows"] if r["name"] == name]
                assert len(matching) == 1
                return browse("select", handle=matching[0]["handle"])

            rejected = select("bad.oas")
            assert rejected["phase"] == "failed"
            assert client.call("GET", "/api/v1/launch")["pending"] is None
            found = listing(query="row-0000")
            (designs / "row-0000.oas").write_bytes(b"changed test file")
            changed = browse("select", handle=found["rows"][0]["handle"])
            assert changed["error"] == "browse_changed"
            (designs / "row-0000.oas").write_bytes(b"catalogue only")

            selected = select(a.name)
            assert selected["phase"] == "succeeded", selected
            assert client.call("GET", "/api/v1/operations")["last_seq"] == "0", "picker opened without owner admission"

            def open_selected(result, current=None, success=True):
                path = "/api/v1/launch/" + result["result"]["launch_id"]
                seq = str(int(client.call("GET", "/api/v1/operations")["last_seq"]) + 1)
                action = dict(action="open", seq=seq, pixels=[320, 240], levels=dict(mode="all"))
                if current:
                    action.update(view_id=current["view_id"], state_rev=current["state_rev"])
                receipt = client.call("POST", path, action)
                assert client.call("POST", path, action) == receipt
                final = client.finished(seq, proc)
                assert final["phase"] == ("succeeded" if success else "failed"), final
                if not success:
                    return final
                return wait(lambda: (lambda v: v if v["status"] == "idle" else None)(client.call("GET", "/api/v1/view")["view"]), proc)

            first_view = open_selected(selected)
            assert first_view["frames"] is False and first_view["labels"] is False, "empty-window baseline lost on first file choice"
            assert first_view["depth"] == "0" and first_view["detail"] == "medium"
            same = open_selected(select(a.name), first_view)
            assert (same["view_id"], same["worker_epoch"]) == (first_view["view_id"], first_view["worker_epoch"])
            other = open_selected(select(b.name), same)
            assert other["view_id"] != same["view_id"]
            fail = open_selected(select(missing.name), other, False)
            assert fail["error"] == "index_unavailable" and not vfs_cache(missing).exists()
            assert client.call("GET", "/api/v1/view")["view"]["view_id"] == other["view_id"]
            selected = select(deck.name)
            pending = client.call("GET", "/api/v1/launch")["pending"]
            assert pending["confirm_levels"] and pending["request"]["display_policy"] == "window"
            assert pending["request"]["body"] == {}, "picker must inherit at the revision-checked open, not list time"
            client.call("POST", "/api/v1/launch/" + selected["result"]["launch_id"], dict(action="dismiss"))
            assert digest(designs) == before, "picker or open modified a source/cache"
            client.call("DELETE", "/api/v1/session", code=204)
            out, err = proc.communicate(timeout=15)
            assert proc.returncode == 0, (out, err)
            assert not session_path.exists() and not list(temps.iterdir())
            assert client.token not in out + err
        finally:
            if proc.poll() is None:
                proc.terminate()
                proc.communicate(timeout=15)
    print("RUST WEB FILE PICKER: ALL OK (approved roots, auth, paging, changed files, no implicit index, native open/reuse/replacement, jobdeck confirmation, shutdown)")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
