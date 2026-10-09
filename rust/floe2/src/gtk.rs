//! Start the GTK viewer: the Python UI (floe/gui.py) through `python -m
//! floe.gtkview`, in this process's place.
//!
//! The checkout root is the folder holding `floe/gui.py`: FLOE_GTK_ROOT, else
//! the first folder above this executable that has it
//! (`<root>/rust/target/release/floe2`), put first on PYTHONPATH. Without one
//! (the portable bundle: floe is in its runtime's site-packages) the
//! interpreter's own packages serve. The interpreter is FLOE_GTK_PYTHON, else
//! the root's `.venv`, else `python3` on PATH.
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

fn python(root: Option<&Path>) -> OsString {
    if let Some(python) = std::env::var_os("FLOE_GTK_PYTHON") {
        return python;
    }
    if let Some(venv) = root
        .map(|r| r.join(".venv").join("bin").join("python"))
        .filter(|v| v.is_file())
    {
        return venv.into_os_string();
    }
    OsString::from("python3")
}

/// Exec the viewer with `args`; returns only on failure (the exit status).
pub fn launch(args: Vec<OsString>) -> i32 {
    let root = root();
    let python = python(root.as_deref());
    let mut command = Command::new(&python);
    command
        .arg("-B")
        .arg("-m")
        .arg("floe.gtkview")
        .args(args)
        .env("FLOE_RENDERER", "rust")
        .env("FLOE_PRODUCT", "floe2");
    if let Some(root) = &root {
        let mut path = root.clone().into_os_string();
        if let Some(old) = std::env::var_os("PYTHONPATH").filter(|p| !p.is_empty()) {
            path.push(":");
            path.push(old);
        }
        command.env("PYTHONPATH", path);
    }
    // the viewer runs this command line for what it does not do itself
    // (a jobdeck's index: floe/vfsclient.py find_floe2)
    if let Ok(exe) = std::env::current_exe() {
        command.env("FLOE2_BIN", exe);
    }
    let error = command.exec();
    eprintln!(
        "floe2: cannot start the GTK viewer with {}: {error}",
        python.to_string_lossy()
    );
    1
}
