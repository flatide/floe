# floe-web — local browser viewer and read-only DRC

Internal Rust library used by `floe2-web view`. An embedded HTML/Canvas client
shows native PNG/raw frames and controls registered layouts/jobdecks. The existing
Python/GTK launcher is unchanged. File upload, remote binding and share grants do
not exist yet. Layout margin/crop is opt-in through ControllerOptions (on in
the `view` CLI); release-only drag, styles/font and basic keyboard controls are
implemented. Full GTK interaction parity and field Firefox/ETX gates remain open.

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

`floe2-web view SOURCE --drc PACK.ice [--drc-waives EXISTING_FILE]` adds a
source-bound read-only DRC panel: rule search, bounded error/coordinate pages,
waive filtering, focus and a display-aligned marker overlay. A dedicated actor
reserves one CPU/256 MiB admission slot and keeps file reads off the HTTP reactor.
View/revision checks surround each read; focus/in-view additionally require the
current state revision. Files are not modified. A per-view revision-checked panel
snapshot restores filters/selection on reload without moving the native view.
One active save and one latest pending state bound browser traffic; ambiguous
outcomes require an explicit server-state reload. This is session memory, not
durable review-file storage. Sharing, review writes and further parity remain open.

Optional `--drc-rules FILE` registers an existing version-1 SVRF metadata snapshot
on the same actor, reserving another 256 MiB (no extra CPU worker). DRC-only roots
and file leases include this explicit file, never its recorded deck/include paths.
The authenticated `types`, filtered `rules`, enriched `rule`, and `comparison`
reads use the same source/view/revision scope. Types are paged; rule filters run
before the row/scan caps. A large polygon comparison returns only scalars, without
copying/transferring its vertices. Snapshot replacement is not hot-reloaded. UI
type/detail/isolation wiring remains next; see [M2 §16](../../docs/WEBUI_M2.ko.md).

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
DRC API/UI, bounds and remaining work: [WEBUI_M2.ko.md](../../docs/WEBUI_M2.ko.md).
