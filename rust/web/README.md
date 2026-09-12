# floe-web — M1b local browser viewer

Internal Rust library used by `floe2-web view`. An embedded HTML/Canvas client
shows native PNG/raw frames and controls registered layouts/jobdecks. The existing
Python/GTK launcher is unchanged. File upload, remote binding and share grants do
not exist yet. Layout margin/crop is opt-in through ControllerOptions (on in
the `view` CLI); drag and further interaction parity are still pending.

`Gateway::new(listener.local_addr())` returns a loopback-only gate and one-use
bootstrap secret. `transport::serve(listener, gate, shutdown)` runs bounded
HTTP/1 and authenticated WebSockets. The caller must keep tokens out of logs.
`Gateway::with_view(addr, controller, title)` additionally attaches one trusted
local managed view. It exposes authenticated snapshots, revision-checked edits
and atomic PNG/raw messages with one ACK credit per subscriber. The independent
controller owns/drains/reaps the native worker. Browser-provided paths and
native commands are never accepted. A disconnected view lingers for 60 seconds;
the owner catalog service can explicitly reopen it with a new revision.

`Service::start` + `Gateway::with_service` adds up to 32 trusted registered
sources, a current view and one bounded open/index operation. All filesystem
and native waits run on the service thread. HTTP source IDs are opaque, index
options are allowlisted, open never indexes implicitly, and monotonically
numbered operations remain at-most-once even after their history is evicted.
Catalog/level/layer pages, progress, close/cancel and revocation are authenticated.

```sh
# Run in rust/ to use the vendored source configuration
cargo test --offline --locked -p floe-web
cargo clippy --offline -p floe-web --all-targets --no-deps -- -D warnings
cargo fmt -p floe-web -- --check
# From the repository root; Node >=18 is a development/test dependency only
node tools/validate_web_ui.cjs
```

The tests bind only ephemeral loopback listeners and shut them down. They test
real HTTP/WS parsing, authentication, CSRF/Origin, limits, malformed frames,
revocation and shutdown. Python and internet access are not runtime/test needs.
`tools/validate_view_stream.py` separately drives the mandatory ignored native
PNG/raw integration in a private synthetic fixture with PATH empty: 100-input
bursts, slow subscribers, reconnect, stale revisions/epochs and resource cleanup.
`tools/validate_owner_service.py` covers actual authenticated index/open/reopen,
duplicate operations, cache leases, deck level/chip metadata and child cleanup.

API, limits, dependency/license/security audit, and incomplete milestones:
[WEBUI_M1B.ko.md](../../docs/WEBUI_M1B.ko.md).
