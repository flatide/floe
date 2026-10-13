//! Start the GTK viewer: the Python UI (floe/gui.py) through `python -m
//! floe.gtkview`, in this process's place.
//!
//! The checkout root is the folder holding `floe/gui.py`: FLOE_GTK_ROOT, else
//! the first folder above this executable that has it
//! (`<root>/rust/target/release/floe2`), put first on PYTHONPATH. Without one
//! (the portable bundle: floe is in its runtime's site-packages) the
//! interpreter's own packages serve. The interpreter is FLOE_GTK_PYTHON, else
//! the root's `.venv`, else `python3` on PATH.
//!
//! The bundle's runtime/bin/floe2 started without the bundle's launcher
//! (`<bundle>/floe2`) starts the viewer through that launcher: the viewer
//! needs the bundle's Python and the GTK environment the launcher sets
//! (FLOE_GTK_PYTHON among them) - the host's python3 is another Python with
//! other libraries, or none (user 2026-10-13: run from runtime/bin, Help >
//! About failed on RHEL 8's Python 3.6). And the bundle's floe2 takes no
//! checkout it sits in (make_portable.sh leaves the tarball in the repo
//! root) for its root: its viewer is the bundle's own floe package, the one
//! built with its binaries.
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Set on the way through the launcher: one pass, whatever the launcher does.
const RELAUNCHED: &str = "FLOE2_VIEW_RELAUNCHED";

fn exe() -> Option<PathBuf> {
    std::env::current_exe().ok()?.canonicalize().ok()
}

/// The portable bundle holding `exe` when it is its runtime/bin/floe2 (the
/// bundle's python3 beside it).
fn bundle(exe: &Path) -> Option<&Path> {
    let bin = exe.parent()?;
    let runtime = bin.parent()?;
    if bin.file_name()? != "bin" || runtime.file_name()? != "runtime" {
        return None;
    }
    if !bin.join("python3").is_file() {
        return None;
    }
    runtime.parent()
}

/// `<bundle>/floe2`, the launcher, when this is the bundle's
/// runtime/bin/floe2 and the launcher did not start it.
fn bundle_launcher() -> Option<PathBuf> {
    if std::env::var_os("FLOE_GTK_PYTHON").is_some() || std::env::var_os(RELAUNCHED).is_some() {
        return None;
    }
    let exe = exe()?;
    Some(bundle(&exe)?.join("floe2")).filter(|launcher| launcher.is_file())
}

fn root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("FLOE_GTK_ROOT") {
        return Some(PathBuf::from(root));
    }
    let exe = exe()?;
    if bundle(&exe).is_some() {
        return None;
    }
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
    if let Some(launcher) = bundle_launcher() {
        let error = Command::new(&launcher)
            .arg("view")
            .args(args)
            .env(RELAUNCHED, "1")
            .exec();
        eprintln!(
            "floe2: cannot start the GTK viewer through the bundle's {}: {error}",
            launcher.display()
        );
        return 1;
    }
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
