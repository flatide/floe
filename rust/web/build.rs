use sha1::{Digest, Sha1};
use std::{env, fs, path::Path};
fn main() {
    // Content identity/skew detection, NOT a signature or authorization token.
    let mut hash = Sha1::new();
    for path in [
        "ui/index.html",
        "ui/app.css",
        "ui/protocol.js",
        "ui/gestures.js",
        "ui/query.js",
        "ui/inspect.js",
        "ui/measure.js",
        "ui/clip.js",
        "ui/snapshot.js",
        "ui/drc.js",
        "ui/drc-build.js",
        "ui/drc-notes.js",
        "ui/drc-waives.js",
        "ui/rulers.js",
        "ui/drc-groups.js",
        "ui/panel-state.js",
        "ui/app.js",
        "ui/settings.js",
        "ui/defaults.js",
        "src/view.rs",
        "src/layer_catalog.rs",
        "src/stream.rs",
        "src/prepared.rs",
        "src/query.rs",
        "src/service.rs",
        "src/exports/mod.rs",
        "src/exports/http.rs",
        "src/exports/prepared.rs",
        "src/owner.rs",
        "src/settings.rs",
        "src/defaults/mod.rs",
        "src/defaults/http.rs",
        "src/transport.rs",
        "src/drc/mod.rs",
        "src/drc/registry.rs",
        "src/drc/review.rs",
        "src/drc/review/http.rs",
        "src/drc/dto.rs",
        "src/drc/http.rs",
        "src/drc/read.rs",
        "src/drc/focus.rs",
        "src/drc/panel.rs",
        "src/drc/metadata.rs",
        "src/drc/selection.rs",
    ] {
        println!("cargo:rerun-if-changed={path}");
        hash.update(path.as_bytes());
        hash.update([0]);
        hash.update(fs::read(path).expect("bundle source"));
    }
    let id = format!("{:x}", hash.finalize());
    println!("cargo:rustc-env=FLOE_WEB_BUNDLE={id}");
    let html = fs::read_to_string("ui/index.html")
        .expect("HTML source")
        .replace("@@BUNDLE@@", &id);
    fs::write(
        Path::new(&env::var_os("OUT_DIR").unwrap()).join("index.html"),
        html,
    )
    .unwrap();
}
