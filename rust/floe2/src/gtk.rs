//! Start the GTK viewer: the Python UI (floe/gui.py) through `python -m
//! floe.gtkview`, in this process's place.
//!
//! The checkout or bundle root is the folder holding `floe/gui.py`:
//! FLOE_GTK_ROOT, else the first folder above this executable that has it
//! (`<root>/rust/target/release/floe2` in a checkout, `<root>/bin/floe2` in a
//! bundle). The interpreter is FLOE_GTK_PYTHON, else the root's `.venv`, else
//! `python3` on PATH.
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("FLOE_GTK_ROOT") {
        return Some(PathBuf::from(root));
    }
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    exe.ancestors()
        .skip(1)
        .find(|dir| dir.join("floe").join("gui.py").is_file())
        .map(Path::to_path_buf)
}

fn python(root: &Path) -> OsString {
    if let Some(python) = std::env::var_os("FLOE_GTK_PYTHON") {
        return python;
    }
    let venv = root.join(".venv").join("bin").join("python");
    if venv.is_file() {
        return venv.into_os_string();
    }
    OsString::from("python3")
}

/// Exec the viewer with `args`; returns only on failure (the exit status).
pub fn launch(args: Vec<OsString>) -> i32 {
    let Some(root) = root() else {
        eprintln!("floe2: the GTK viewer (floe/gui.py) is not beside this executable - set FLOE_GTK_ROOT to the folder holding floe/");
        return 2;
    };
    let mut path = root.clone().into_os_string();
    if let Some(old) = std::env::var_os("PYTHONPATH").filter(|p| !p.is_empty()) {
        path.push(":");
        path.push(old);
    }
    let python = python(&root);
    let error = Command::new(&python)
        .arg("-B")
        .arg("-m")
        .arg("floe.gtkview")
        .args(args)
        .env("PYTHONPATH", path)
        .env("FLOE_RENDERER", "rust")
        .exec();
    eprintln!(
        "floe2: cannot start the GTK viewer with {}: {error}",
        python.to_string_lossy()
    );
    1
}
