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
        "ui/app.js",
        "src/view.rs",
        "src/layer_catalog.rs",
        "src/stream.rs",
        "src/service.rs",
        "src/owner.rs",
        "src/transport.rs",
        "src/drc/mod.rs",
        "src/drc/dto.rs",
        "src/drc/http.rs",
        "src/drc/read.rs",
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
