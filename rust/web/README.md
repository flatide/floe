# floe-web — M1b authenticated view stream

Internal Rust library; **not yet a runnable layout web viewer**. The application
CLI and existing Python/GTK launcher are unchanged. No source catalog, index,
file-upload endpoint, UI assets, remote binding or share grants exist yet.

`Gateway::new(listener.local_addr())` returns a loopback-only gate and one-use
bootstrap secret. `transport::serve(listener, gate, shutdown)` runs bounded
HTTP/1 and authenticated WebSockets. The caller must keep tokens out of logs.
`Gateway::with_view(addr, controller, title)` additionally attaches one trusted
local managed view. It exposes authenticated snapshots, revision-checked edits
and atomic PNG/raw messages with one ACK credit per subscriber. The independent
controller owns/drains/reaps the native worker. Browser-provided paths and
native commands are never accepted. A disconnected view lingers for 60 seconds;
reopening an expired view is a later catalog feature.

```sh
# Run in rust/ to use the vendored source configuration
cargo test --offline --locked -p floe-web
cargo clippy --offline -p floe-web --all-targets --no-deps -- -D warnings
cargo fmt -p floe-web -- --check
```

The tests bind only ephemeral loopback listeners and shut them down. They test
real HTTP/WS parsing, authentication, CSRF/Origin, limits, malformed frames,
revocation and shutdown. Python and internet access are not runtime/test needs.
`tools/validate_view_stream.py` separately drives the mandatory ignored native
PNG/raw integration in a private synthetic fixture with PATH empty: 100-input
bursts, slow subscribers, reconnect, stale revisions/epochs and resource cleanup.

API, limits, dependency/license/security audit, and incomplete milestones:
[WEBUI_M1B.ko.md](../../docs/WEBUI_M1B.ko.md).
