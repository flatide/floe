"""Selection API oracle, called by validate_web_drc.py with private fixtures."""
from concurrent.futures import ThreadPoolExecutor
import json
import threading
import urllib.error
import urllib.request


def validate_selection(client, path, endpoint, context, expected, view_state):
    fresh = client.call("GET", path)
    wanted = dict(selection_rev="1", total="0", limit=5000, rules=[])
    assert fresh == dict(revision=context["revision"], view_id=context["view_id"], state=wanted)
    revision = 1
    selected = {}
    mask = next(i for i, c in enumerate(expected) if c["name"] == "MASK<&>")
    other = next(i for i, c in enumerate(expected) if c["errors"] and i != mask)

    def post(body, base=None, code=200, **extra):
        request = dict(revision=context["revision"], base_selection_rev=str(revision if base is None else base), body=body)
        request.update(extra)
        return client.call("POST", path, request, code)

    def apply(ci, ids, mode="replace", box=None, waived=None):
        nonlocal revision, wanted
        body = dict(kind="apply", check=str(ci), errors=list(map(str, ids)), mode=mode,
                    bbox_um=None if box is None else list(map(str, box)), waived=waived)
        args = {} if box is None else dict(state_rev=view_state["state_rev"])
        observed = post(body, **args)
        # GTK _esel_apply compares the BBOX, not the marker center. The
        # visible page supplied by the caller is the only candidate set.
        hits = set()
        for i in set(ids):
            e = expected[ci]["errors"][i]
            b = e["bbox"]
            if (waived is None or (e["status"] == 1) == waived) and (
                    box is None or b[0] <= box[2] and b[2] >= box[0] and b[1] <= box[3] and b[3] >= box[1]):
                hits.add(i)
        if mode == "replace":
            selected[ci] = hits
        elif mode == "add":
            selected[ci] = selected.get(ci, set()) | hits
        else:
            selected[ci] = selected.get(ci, set()) ^ hits
        revision += 1
        wanted = dict(selection_rev=str(revision), total=str(sum(map(len, selected.values()))), limit=5000,
                      rules=[dict(check=str(k), errors=list(map(str, sorted(v)))) for k, v in sorted(selected.items()) if v])
        assert observed == dict(revision=context["revision"], view_id=context["view_id"], state=wanted), (body, observed, wanted)
        assert client.call("GET", path) == observed
        return body

    # Wrong authentication/IDs/schema never consume a command or change state.
    valid = dict(kind="apply", check=str(mask), errors=["0"], mode="toggle")
    csrf = client.csrf
    client.csrf = "wrong"
    post(valid, code=401)
    client.csrf = csrf
    for body in [dict(valid, check="00"), dict(valid, errors=["-1"]), dict(valid, errors=[0]),
                 dict(valid, errors=["0"]*65), dict(valid, errors=["999999999"]),
                 dict(valid, check=str(len(expected))), dict(valid, mode="invalid"),
                 dict(valid, path="/etc/passwd"), dict(kind="clear_all", path="/etc/passwd"),
                 dict(valid, bbox_um=["0", "0", "NaN", "1"]),
                 dict(valid, bbox_um=["0", "0", "1", "1"])]:
        post(body, code=400)
    post(valid, code=409, revision="wrong")
    post(valid, code=409, state_rev="999999999")
    post(valid, base="00", code=400)
    assert client.call("GET", path) == fresh

    # A bounded page with a large box MUST NOT include the following page.
    apply(mask, list(range(64)), box=[-1, -1, 1, 1])
    assert wanted["total"] == "64"
    # Same-base toggle retries must conflict, even when the first change is a
    # no-op. The UI must read authoritative state after an uncertain response.
    body = apply(mask, [63, 64, 64], "toggle")
    post(body, base=revision-1, code=409)
    body = apply(mask, [], "add")
    post(body, base=revision-1, code=409)
    apply(other, [0], "add")
    apply(mask, [0], "replace")
    assert any(r["check"] == str(other) for r in wanted["rules"]), "rule switch lost other selection"
    # Touching a record boundary selects it even when the marker CENTER is
    # outside. A nearby point just beyond the edge must not select that record.
    b = expected[mask]["errors"][3]["bbox"]
    for box in ([b[0], b[1], b[0], b[1]], [b[2]+1e-7, b[1], b[2]+1e-7, b[3]],
                [-1e6, -1e6, 1e6, 1e6], [20, 20, 21, 21]):
        for waived in (None, True, False):
            for mode in ("replace", "add", "toggle"):
                apply(mask, [0, 1, 3, 3, 7, 63, 64, 129], mode, box, waived)
    apply(mask, [], "replace")
    assert any(r["check"] == str(other) for r in wanted["rules"])

    # Explicit metadata reads support restoring/filtering groups without a
    # full spatial scan or copying a large error's coordinate arrays.
    for ci in (mask, next(i for i, c in enumerate(expected) if c["name"] == "BIG")):
        ids = [70, 64, 0, 64] if ci == mask else [0]
        packet = client.call("POST", endpoint, dict(context, body=dict(kind="records", check=str(ci), errors=list(map(str, ids)))))
        assert len(packet["rows"]) == len(set(ids))
        for r, ei in zip(packet["rows"], sorted(set(ids))):
            e = expected[ci]["errors"][ei]
            assert (r["check"], r["local"], r["global"], r["kind"], r["status"]) == (str(ci), str(ei), e["glob"], e["kind"], e["status"])
            assert list(map(float, r["bbox_um"])) == list(e["bbox"])
            assert r["points"] == str(len(e["pts"])) and "points_dbu" not in r
    for body in [dict(kind="records", check=str(mask), errors=["0"]*65),
                 dict(kind="records", check=str(mask), errors=["9999999"]),
                 dict(kind="records", check="00", errors=[]),
                 dict(kind="records", check=str(mask), errors=[1])]:
        client.call("POST", endpoint, dict(context, body=body), 400)

    # Clear all (waive-filter changes) is one atomic command, not a loop over
    # rules that can leave a half-cleared state after disconnection.
    cleared = post(dict(kind="clear_all"))
    revision += 1
    assert cleared["state"] == dict(selection_rev=str(revision), total="0", limit=5000, rules=[])
    assert client.call("GET", "/api/v1/view")["view"]["state_rev"] == view_state["state_rev"]
    assert client.call("GET", path) == cleared
    # Exercise the HTTP post-await CAS, not only sequential stale requests.
    # Both clients send the same toggle/base; exactly one must apply it.
    barrier = threading.Barrier(2)
    payload = json.dumps(dict(revision=context["revision"], base_selection_rev=str(revision),
                             body=dict(kind="apply", check=str(mask), errors=["0"], mode="toggle"))).encode()

    def concurrent_toggle(_):
        request = urllib.request.Request(client.origin + path, data=payload, method="POST",
                  headers={"Origin": client.origin, "X-Floe-CSRF": client.csrf, "Content-Type": "application/json"})
        barrier.wait(timeout=5)
        try:
            response = client.opener.open(request, timeout=8)
        except urllib.error.HTTPError as e:
            response = e
        with response:
            return response.status, json.loads(response.read(1024*1024))

    with ThreadPoolExecutor(max_workers=2) as pool:
        replies = list(pool.map(concurrent_toggle, range(2)))
    assert sorted(code for code, _ in replies) == [200, 409], replies
    assert next(v for code, v in replies if code == 409)["error"] == "drc_selection_conflict"
    revision += 1
    state = client.call("GET", path)["state"]
    assert state == dict(selection_rev=str(revision), total="1", limit=5000,
                         rules=[dict(check=str(mask), errors=["0"])]), state
    print("WEB DRC SELECTION: ALL OK (candidate bbox oracle, replace/add/toggle, rule groups, CAS/no replay, filters, metadata-only, auth)")
    return revision
