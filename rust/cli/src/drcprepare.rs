//! Public preprocessing entry point. The numeric CD kernel stays in Rust;
//! the existing Python/NumPy coordinator owns cache formats and SVRF chains.
//! Exec keeps Ctrl+C, exit status, and child cleanup identical to that CLI.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

const ENTRY: &str = r#"
import sys
try:
    from floe.drc_prepare import main
    status = main(sys.argv[1:], prog='floe-index drc-prepare')
except ImportError as exc:
    print('floe-index drc-prepare requires the floe Python package and NumPy. '
          'Use the portable runtime or set FLOE_PYTHON_BIN to its Python executable: '
          + str(exc), file=sys.stderr)
    raise SystemExit(1)
raise SystemExit(status)
"#;

fn source_root(executable: &Path) -> Option<PathBuf> {
    executable.parent()?.ancestors().take(5).find(|root| {
        root.join("floe/drc_prepare.py").is_file() && root.join("rust/Cargo.toml").is_file()
    }).map(Path::to_path_buf)
}

fn executable_file(path: &Path) -> bool {
    if !path.is_file() { return false; }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return path.metadata().map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false);
    }
    #[cfg(not(unix))]
    true
}

fn select_python(executable: &Path, source: Option<&Path>) -> (OsString, Option<PathBuf>) {
    if let Some(python) = std::env::var_os("FLOE_PYTHON_BIN") {
        // Explicit settings fail clearly instead of choosing another runtime.
        return (python, None);
    }
    let mut adjacent = None;
    if let Some(bin) = executable.parent() {
        let python = bin.join("python3");
        if executable_file(&python) {
            let portable = bin.parent().filter(|prefix| {
                bin.file_name().is_some_and(|name| name == "bin")
                    && prefix.file_name().is_some_and(|name| name == "runtime")
                    && prefix.join("lib").is_dir()
            }).map(Path::to_path_buf);
            if portable.is_some() { return (python.into_os_string(), portable); }
            adjacent = Some(python);
        }
    }
    for name in ["VIRTUAL_ENV", "CONDA_PREFIX"] {
        if let Some(prefix) = std::env::var_os(name) {
            let python = PathBuf::from(prefix).join("bin/python");
            if executable_file(&python) { return (python.into_os_string(), None); }
        }
    }
    if let Some(source) = source {
        for name in ["python", "python3"] {
            let python = source.join(".venv/bin").join(name);
            if executable_file(&python) { return (python.into_os_string(), None); }
        }
    }
    if let Some(python) = adjacent { return (python.into_os_string(), None); }
    (OsString::from("python3"), None)
}

fn prepend_path(command: &mut Command, name: &str, first: &Path) -> Result<(), String> {
    let mut paths = vec![first.to_path_buf()];
    if let Some(existing) = std::env::var_os(name) {
        paths.extend(std::env::split_paths(&existing));
    }
    let value = std::env::join_paths(paths).map_err(|e| format!("invalid {}: {}", name, e))?;
    command.env(name, value);
    Ok(())
}

fn command(args: &[String]) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|e| format!("locate floe-index: {}", e))?;
    let source = source_root(&executable);
    let (python, portable) = select_python(&executable, source.as_deref());
    if python.is_empty() { return Err("FLOE_PYTHON_BIN is empty; set it to a Python executable".into()); }
    let mut child = Command::new(&python);
    child.arg("-c").arg(ENTRY).args(args);
    // A user's PATH/override may name an older indexer. All native work in
    // this invocation must use exactly the executable they just invoked.
    child.env("FLOE_INDEX_BIN", &executable);
    // A portable bundle may live inside a checkout while being tested.
    // Its bundled application must not be shadowed by that checkout.
    if portable.is_none() {
        if let Some(source) = source { prepend_path(&mut child, "PYTHONPATH", &source)?; }
    }
    if let Some(runtime) = portable {
        child.env("PYTHONHOME", &runtime).env("PYTHONNOUSERSITE", "1");
        prepend_path(&mut child, "LD_LIBRARY_PATH", &runtime.join("lib"))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = child.exec();
        Err(format!("cannot execute Python {:?}: {}. Use the portable runtime or set FLOE_PYTHON_BIN", python, error))
    }
    #[cfg(not(unix))]
    {
        let status = child.status().map_err(|e| format!("cannot execute Python {:?}: {}. Set FLOE_PYTHON_BIN", python, e))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

pub fn drcprepare_cmd(args: &[String]) {
    if let Err(error) = command(args) {
        eprintln!("[drc-prepare] {}", error);
        std::process::exit(1);
    }
}
