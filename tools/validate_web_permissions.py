#!/usr/bin/env python3
"""SH-02 HTTP denial matrix, imported by the synthetic native sharing gate.

The checked-in table is independently matched to Rust AST by cargo test.
These probes use an existing private test server, never a discovered server.
Valid owner behavior and allowed guest operations have their own native gates.
"""
import http.cookiejar
import http.client
import json
from pathlib import Path
import re
import urllib.parse
import urllib.request


def policy():
    result = json.loads(Path(__file__).with_name("web_permissions.json").read_text())
    assert result["http"] and result["wire"], "permission inventory is empty"
    return result


def cookies(opener):
    return [cookie for handler in opener.handlers
            if isinstance(handler, urllib.request.HTTPCookieProcessor)
            for cookie in handler.cookiejar]


def probe(opener, session, method, path, csrf, guest_target, expected, target_name=None):
    # Put the proof in BOTH namespaces: no accidental pass from merely omitting
    # the header the target expects. Cookie identity must still reject it.
    headers = {"Origin": session["origin"], "Content-Type": "application/json",
               "X-Floe-CSRF": csrf, "X-Floe-Guest-CSRF": csrf}
    jar = cookies(opener)
    # Send opposite-role cookies even outside their Path, then also rename the
    # foreign value into the target namespace. Cookie Path/name is no boundary.
    if jar:
        assert len(jar) == 1, "unexpected synthetic cookie set"
        headers["Cookie"] = (target_name or jar[0].name) + "=" + jar[0].value
    if path.endswith("/events"):
        prefix = "guest-csrf." if guest_target else "csrf."
        headers.update({"Connection": "Upgrade", "Upgrade": "websocket",
                        "Sec-WebSocket-Version": "13",
                        "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==",
                        "Sec-WebSocket-Protocol": "floe.v1, bundle." + session["bundle"] + ", " + prefix + csrf})
    origin = urllib.parse.urlsplit(session["origin"])
    assert origin.scheme == "http" and origin.hostname == "127.0.0.1"
    connection = http.client.HTTPConnection(origin.hostname, origin.port, timeout=8)
    try:
        # urllib forces Connection: close and invalidates a WS Upgrade probe.
        connection.request(method, path, headers=headers,
                           body=b"{}" if method == "POST" else None)
        response = connection.getresponse()
        # Never include credentials or response bodies in failure output.
        assert response.status == expected, (method, path, response.status, expected)
        data = response.read(4096)
        assert len(data) < 4096, "unexpected large denial response"
    finally:
        connection.close()


def verify_cross_auth(session, owner, guest, guest_csrf, share_id, view_id):
    rows = policy()["http"]
    anonymous = urllib.request.build_opener(urllib.request.ProxyHandler({}),
        urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
    values = {"bundle": session["bundle"], "name": "guest.js", "id": view_id,
              "view": view_id, "seq": "1", "rev": "1", "after": "0", "start": "0",
              "page": "0", "base": "full", "format": "native", "key": "0" * 40}
    count = 0
    for key, row in sorted(rows.items()):
        authority = row[-1]
        if authority not in ("owner", "guest", "guest_drc"):
            continue  # static shell and one-use bootstrap are distinct capabilities
        method, template = key.split(" ", 1)
        replacements = dict(values)
        is_guest = authority in ("guest", "guest_drc")
        if is_guest:
            replacements["id"] = share_id
        if "/display-test/" in template:
            replacements["format"] = "raw"
        path = re.sub(r"\{(\w+)\}", lambda match: replacements[match[1]], template)
        # Axum's WebSocket extractor rejects HEAD before the auth handler.
        # GET carries a valid Upgrade envelope and MUST reach the 401 fence.
        expected = 405 if method == "HEAD" and path.endswith("/events") else 401
        cross, cross_csrf = (owner.opener, owner.csrf) if is_guest else (guest, guest_csrf)
        target_name = cookies(guest if is_guest else owner.opener)[0].name
        for opener, csrf, renamed in ((anonymous, "a" * 64, None),
                                      (cross, cross_csrf, None),
                                      (cross, cross_csrf, target_name)):
            probe(opener, session, method, path, csrf, is_guest, expected, renamed)
            count += 1
    assert count > 200, "route coverage unexpectedly shrank"
    # The negative mutations must not log out either valid principal.
    owner.call("GET", "/api/v1/capabilities")
    print(f"WEB HTTP PERMISSIONS: ALL OK ({count} anonymous/cross-principal probes including HEAD and valid WS Upgrade)")
