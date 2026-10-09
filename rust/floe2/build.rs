//! floe2's version is the product's: `__version__` in floe/__init__.py (the
//! GTK viewer's About and the portable bundle read the same line), so one
//! bump per push names the CLI and the viewer alike.
use std::{env, fs, path::Path};

fn main() {
    let init = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../floe/__init__.py");
    println!("cargo:rerun-if-changed={}", init.display());
    let text = fs::read_to_string(&init).expect("floe/__init__.py");
    let version = text
        .lines()
        .find_map(|l| l.strip_prefix("__version__ = \""))
        .and_then(|rest| rest.split('"').next())
        .expect("__version__ in floe/__init__.py");
    println!("cargo:rustc-env=FLOE2_VERSION={version}");
}
