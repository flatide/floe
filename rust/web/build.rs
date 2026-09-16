use sha1::{Digest, Sha1};
use std::{env, fs, path::Path};
fn main() {
    // Content identity/skew detection, NOT a signature or authorization token.
    let mut hash = Sha1::new();
    for path in [
        "ui/index.html",
        "ui/guest.html",
        "ui/guest.js",
        "ui/guest-drc.js",
        "ui/guest-layers.js",
        "ui/drc-geometry.js",
        "ui/guest.css",
        "ui/sharing.js",
        "ui/display.html",
        "ui/display-page.js",
        "ui/display-input.js",
        "ui/app.css",
        "ui/protocol.js",
        "ui/gestures.js",
        "ui/minimap.js",
        "ui/query.js",
        "ui/inspect.js",
        "ui/measure.js",
        "ui/clip.js",
        "ui/snapshot.js",
        "ui/display-dump.js",
        "ui/drc.js",
        "ui/drc-build.js",
        "ui/drc-notes.js",
        "ui/hangul.js",
        "ui/review-save-mode.js",
        "ui/drc-note-display.js",
        "ui/drc-waives.js",
        "ui/drc-transfer.js",
        "ui/rulers.js",
        "ui/drc-groups.js",
        "ui/panel-state.js",
        "ui/app.js",
        "ui/launcher.js",
        "ui/browse.js",
        "ui/index-open.js",
        "ui/palette.js",
        "ui/presets.js",
        "ui/fill-editor.js",
        "ui/image-decode.js",
        "src/display_test.rs",
        "ui/display-test.js",
        "src/presets.rs",
        "../../floe/colornames.def",
        "../../floe/fillpatterns.def",
        "ui/settings.js",
        "ui/defaults.js",
        "ui/about.js",
        "ui/session-exit.js",
        "ui/notices.js",
        "src/about.rs",
        "../notices/src/lib.rs",
        "../render-core/assets/NotoSansMono-OFL.txt",
        "src/view.rs",
        "src/fill_slots.rs",
        "src/layer_catalog.rs",
        "src/layer_catalog/scoped.rs",
        "src/stream.rs",
        "src/prepared.rs",
        "src/query.rs",
        "src/service.rs",
        "src/service/open.rs",
        "src/service/index_open.rs",
        "src/window_display.rs",
        "src/launch.rs",
        "src/browse/mod.rs",
        "src/browse/http.rs",
        "src/exports/mod.rs",
        "src/exports/http.rs",
        "src/exports/prepared.rs",
        "src/owner.rs",
        "src/settings.rs",
        "src/defaults/mod.rs",
        "src/defaults/http.rs",
        "src/transport.rs",
        "src/auth.rs",
        "src/sharing/mod.rs",
        "src/sharing/http.rs",
        "src/sharing/stream.rs",
        "src/sharing/explore.rs",
        "src/sharing/query.rs",
        "src/sharing/drc.rs",
        "src/sharing/layers.rs",
        "src/drc/mod.rs",
        "src/drc/shared.rs",
        "src/drc/registry.rs",
        "src/drc/review.rs",
        "src/drc/review/display.rs",
        "src/drc/review/http.rs",
        "src/drc/review/transfer.rs",
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
    // The local IPC build fence must also change for dirty launcher/core
    // changes, not just browser assets or the last Git commit. No runtime
    // binary hashing or filesystem traversal on a warm CLI handoff.
    for dir in ["../app/src", "../app-core/src"] {
        hash_tree(Path::new(dir), &mut hash);
    }
    for path in [
        "../Cargo.lock",
        "../app/Cargo.toml",
        "../app-core/Cargo.toml",
    ] {
        println!("cargo:rerun-if-changed={path}");
        hash.update(path.as_bytes());
        hash.update(fs::read(path).expect("application identity"));
    }
    let id = format!("{:x}", hash.finalize());
    println!("cargo:rustc-env=FLOE_WEB_BUNDLE={id}");
    for name in ["index.html", "display.html", "guest.html"] {
        let html = fs::read_to_string(format!("ui/{name}"))
            .expect("HTML source")
            .replace("@@BUNDLE@@", &id);
        fs::write(Path::new(&env::var_os("OUT_DIR").unwrap()).join(name), html).unwrap();
    }
}
fn hash_tree(dir: &Path, hash: &mut Sha1) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut entries: Vec<_> = fs::read_dir(dir)
        .expect("application sources")
        .map(|e| e.expect("source entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            hash_tree(&path, hash);
        } else if path.extension().is_some_and(|v| v == "rs") {
            hash.update(path.to_string_lossy().as_bytes());
            hash.update([0]);
            hash.update(fs::read(path).expect("application source"));
        }
    }
}
