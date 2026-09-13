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

M4a-3 adds `view.query`/`view.query.cancel` on the existing owner WebSocket. A
query pins the dataset/worker/frame/state identity and requires a displayed ACK
plus completed packet write on that same connection. Foreground and margin
receipts stay bounded to one each; native requests/results are latest-only per
kind. Query results do not wait for image ACK/admission. Disconnection conditionally
cancels only this consumer's current query IDs, never another socket's newer work.
Coordinates and counters cross the wire as strings; native errors/paths are not
forwarded. Mixed summary scenes can query exact-only visible layers, and truncated
outlines are explicit. Jobdeck queries remain unsupported. Backend capabilities
are available for layouts, but browser pick/snap/ruler controls are a later stage.
The request/result schema and limits are recorded in [M4 §3](../../docs/WEBUI_M4.ko.md).

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
copying/transferring its vertices. Snapshot replacement is not hot-reloaded.
The panel shows paged types, rule metadata and the selected error's comparison;
type/name/waive filters run together on rule reads. Type choice is restored through
the existing revision-checked panel state. Successful focus navigation releases
live In view without clearing the selected error.
The server now supports `focus(isolate=true)` preparation and single-use
`view.apply` for atomic physical-layer isolation + navigation. The original
visibility is saved once in view state; `view.set.restore_layers` restores it.
Snapshot `layers_isolated` survives reconnect, not a new view. Matching uses the
entire model; missing metadata/matches and virtual jobdeck IDs never hide layers.
The panel uses this token-only path for error jumps. CD and live-filter effects
wait for acceptance plus its matching authoritative snapshot, before viewport
observers run; superseded or disconnected inputs are not replayed. Restore and
Escape return the first visibility, then clear CD/jump focus while retaining the
selected cursor. Missing matches and jobdeck physical-plane isolation report
unchanged layers. See [M2 §16–19](../../docs/WEBUI_M2.ko.md).

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
It also runs native prepared-focus/CAS/replay/reconnect/restore isolation tests.
`tools/validate_worker_queries.py` adds real owner socket pick/snap/overlap,
long-outline, summary subset, stale/discarded receipt, wrong connection/view,
margin-credit and reconnect tests on its private synthetic source.

API, limits, dependency/license/security audit, and incomplete milestones:
[WEBUI_M1B.ko.md](../../docs/WEBUI_M1B.ko.md).
DRC API/UI, bounds and remaining work: [WEBUI_M2.ko.md](../../docs/WEBUI_M2.ko.md).
