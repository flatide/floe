# floe-web — M1b transport foundation

Internal Rust library; **not yet a runnable layout web viewer**. The application
CLI and existing Python/GTK launcher are unchanged. No render/index/file-upload
endpoint, UI assets, remote binding or share grants are exposed in this stage.

`Gateway::new(listener.local_addr())` returns a loopback-only gate and one-use
bootstrap secret. `transport::serve(listener, gate, shutdown)` runs bounded
HTTP/1 and authenticated WebSockets. The caller must keep tokens out of logs.
The native render worker stays behind the forthcoming session controller.

```sh
# Run in rust/ to use the vendored source configuration
cargo test --offline --locked -p floe-web
cargo clippy --offline -p floe-web --all-targets --no-deps -- -D warnings
cargo fmt -p floe-web -- --check
```

The tests bind only ephemeral loopback listeners and shut them down. They test
real HTTP/WS parsing, authentication, CSRF/Origin, limits, malformed frames,
revocation and shutdown. Python and internet access are not runtime/test needs.

API, limits, dependency/license/security audit, and incomplete milestones:
[WEBUI_M1B.ko.md](../../docs/WEBUI_M1B.ko.md).
