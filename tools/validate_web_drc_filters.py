"""Rule-list intersection and circular navigation oracle for the native service."""


def validate_filters(client, selection_path, endpoint, context, expected, view_state, revision):
    before_panel = client.call("GET", selection_path.removesuffix("selection") + "panel")
    mask = next(i for i, c in enumerate(expected) if c["name"] == "MASK<&>")
    big = next(i for i, c in enumerate(expected) if c["name"] == "BIG")
    empty_selection = next(i for i, c in enumerate(expected) if c["errors"] and i not in (mask, big))
    selected = {mask: set(range(0, 130, 2)) | {63, 129}, big: {0}}

    def edit(ci, ids, mode):
        nonlocal revision
        state = client.call("POST", selection_path, dict(revision=context["revision"], base_selection_rev=str(revision),
                    body=dict(kind="apply", check=str(ci), errors=list(map(str, ids)), mode=mode)))
        revision += 1
        assert state["state"]["selection_rev"] == str(revision)

    for ci, ids in selected.items():
        ordered = sorted(ids)
        for start in range(0, len(ordered), 64):
            edit(ci, ordered[start:start+64], "replace" if start == 0 else "add")
    box = [float(v)*float(view_state["dbu_um"]) for v in view_state["bbox_dbu"]]

    def read(body, code=200, lock=True, **extra):
        request = dict(context, body=body)
        if lock and body.get("in_view"):
            request["state_rev"] = view_state["state_rev"]
        request.update(extra)
        return client.call("POST", endpoint, request, code)

    def match(e, i, ci, selection, waived, in_view):
        b = e["bbox"]
        return (not selection or i in selected.get(ci, set())) and (waived is None or (e["status"] == 1) == waived) and (
            not in_view or b[0] <= box[2] and b[2] >= box[0] and b[1] <= box[3] and b[3] >= box[1])

    cases = 0
    for ci in (mask, big, empty_selection, next(i for i, c in enumerate(expected) if not c["errors"])):
        for selection in (False, True):
            for waived in (None, False, True):
                for in_view in (False, True):
                    wanted = [i for i, e in enumerate(expected[ci]["errors"]) if match(e, i, ci, selection, waived, in_view)]
                    for limit in (1, 7, 64):
                        request = dict(kind="list", check=str(ci), start="0", waived=waived, limit=limit,
                                       in_view=in_view, selection_rev=str(revision) if selection else None)
                        rows = []
                        while True:
                            page = read(request)
                            assert page["selection_rev"] == request["selection_rev"]
                            assert (list(map(float, page["bbox_um"])) == box) if in_view else (page["bbox_um"] is None)
                            assert len(page["rows"]) <= limit and int(page["scanned"]) <= 262144
                            rows.extend(page["rows"])
                            if page["next"] is None:
                                break
                            assert int(page["next"]) > int(request["start"])
                            request["start"] = page["next"]
                        assert [int(r["local"]) for r in rows] == wanted, (ci, selection, waived, in_view, limit)
                        for r in rows:
                            e = expected[ci]["errors"][int(r["local"])]
                            assert (r["check"], r["global"], r["kind"], r["status"], r["points"]) == (str(ci), e["glob"], e["kind"], e["status"], str(len(e["pts"])))
                            assert list(map(float, r["bbox_um"])) == list(e["bbox"]) and "points_dbu" not in r
                        cases += 1
                    if not expected[ci]["errors"]:
                        anchors = [None]
                    else:
                        anchors = [None, 0, len(expected[ci]["errors"])-1]
                    for backwards in (False, True):
                        for after in anchors:
                            request = dict(kind="filtered_step", check=str(ci), backwards=backwards, after=None if after is None else str(after),
                                cursor=None, waived=waived, in_view=in_view, selection_rev=str(revision) if selection else None)
                            while True:
                                page = read(request)
                                assert page["selection_rev"] == request["selection_rev"]
                                if page["next"] is None:
                                    break
                                assert not selection, "bounded selection unexpectedly returned a pack-range cursor"
                                assert page["hit"] is None and int(page["scanned"]) > 0
                                request.update(after=None, cursor=page["next"])
                            order = list(reversed(wanted)) if backwards else wanted
                            candidate = next((i for i in order if after is None or (i < after if backwards else i > after)), None)
                            if candidate is None and order:
                                candidate = order[0]
                            assert (None if page["hit"] is None else int(page["hit"]["local"])) == candidate
                            cases += 1
    good = dict(kind="list", check=str(mask), start="0", waived=None, limit=64, in_view=False, selection_rev=str(revision))
    read(dict(good, selection_rev=str(revision-1)), 409)
    read(dict(good, in_view=True), 400, lock=False)
    read(dict(good, in_view=True), 409, state_rev="999999999")
    read(good, 409, view_id="not-this-view")
    read(good, 409, revision="wrong")
    for bad in (dict(good, selection_rev="00"), dict(good, selection_rev=revision), dict(good, limit=65),
                dict(good, start="999999999"), dict(good, check="99999999"), dict(good, in_view=1),
                dict(good, bbox_um=["0"]*4), dict(good, errors=["1"]), dict(good, path="/etc/passwd")):
        read(bad, 400)
    stale = dict(good)
    edit(mask, [], "replace")
    read(stale, 409)
    empty = read(dict(good, selection_rev=str(revision)))
    assert empty["rows"] == [] and empty["next"] is None, "empty selected filter became all errors"
    assert client.call("GET", selection_path.removesuffix("selection") + "panel") == before_panel
    assert client.call("GET", "/api/v1/view")["view"]["state_rev"] == view_state["state_rev"]
    print(f"WEB DRC FILTERS: ALL OK ({cases} list/step cases, selection/waive/view intersection, metadata, cursor, revisions, no navigation)")
    return revision
