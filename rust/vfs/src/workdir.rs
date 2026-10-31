//! The work directory (`--workdir DIR` / FLOE_WORKDIR; user 2026-10-31:
//! "--workdir로 .ice, .tray, .sqlite등의 파일을 읽고 쓰는 위치를 지정할 수
//! 있게", then "floe-index에도 --workdir을 넣어줘"): where every derived
//! file - a source's `.x.oas.ice/` cache, a results database's `.r.db.tray`
//! pack, and what is made from those (sidecars, revisions, locks) - is read
//! and written: at the source's absolute path under it, `/a/b/x.oas` ->
//! `DIR/a/b/.x.oas.ice`, nothing beside the source. One rule for the
//! programs that make those names: floe-index's defaults (`hidden_sibling`),
//! the application layer (app-core `cache::names`) and the CLIs' option
//! (floe-app-cli `workdir.rs`, floe-index's main). The setting is
//! process-wide and read once: `set` before any name is made, else
//! FLOE_WORKDIR; `apply` exports it as FLOE_WORKDIR so the processes a run
//! starts (floe-index, floe-renderd, the GTK viewer and its service, the
//! floe2 a deck is handed to) place their files the same way.
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

static WORKDIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The work directory in force: `set`'s, else FLOE_WORKDIR (empty = none;
/// `~/` expanded; made absolute against the current directory), read once.
pub fn workdir() -> Option<&'static Path> {
    WORKDIR.get_or_init(from_env).as_deref()
}
/// Set it before any derived name is made: whether it took (false once a
/// name was made, or it was set before).
pub fn set(dir: PathBuf) -> bool {
    WORKDIR.set(Some(dir)).is_ok()
}
fn from_env() -> Option<PathBuf> {
    let raw = std::env::var_os("FLOE_WORKDIR")?;
    if raw.is_empty() {
        return None;
    }
    Some(lexical_absolute(&expand_user(PathBuf::from(raw))))
}
/// `~` and `~/...` as the shell would have given them (a value set in a
/// profile keeps the tilde when it is quoted).
pub fn expand_user(dir: PathBuf) -> PathBuf {
    if let Ok(rest) = dir.strip_prefix("~") {
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            return PathBuf::from(home).join(rest);
        }
    }
    dir
}
/// `path` made absolute without touching the file system: joined to the
/// current directory when relative, `.` dropped and `..` folded (a source
/// reached through a link keeps the link's folder, as its sibling cache
/// does); as given when the current directory cannot be read.
pub fn lexical_absolute(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => return path.to_path_buf(),
        }
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => (),
            other => out.push(other.as_os_str()),
        }
    }
    out
}
/// `path` (absolute) placed under `root`: `/a/b/.x.oas.ice` under `/w` is
/// `/w/a/b/.x.oas.ice`.
pub fn under(root: &Path, path: &Path) -> PathBuf {
    let mut out = root.to_path_buf();
    for c in path.components() {
        match c {
            Component::RootDir | Component::Prefix(_) | Component::CurDir => (),
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(n) => out.push(n),
        }
    }
    out
}
/// A derived name as made beside its source, where it goes: under the work
/// directory at its absolute path, or as it is without one.
pub fn place(path: PathBuf) -> PathBuf {
    match workdir() {
        Some(root) => under(root, &lexical_absolute(&path)),
        None => path,
    }
}
/// The place `path` mirrors when it is under the work directory
/// (`/w/a/b/r.db` -> `/a/b/r.db`); None without one, or for a path outside it.
pub fn original_of(path: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(workdir()?).ok()?;
    Some(Path::new("/").join(rest))
}

/// Take `--workdir DIR` / `--workdir=DIR` out of `args` (up to a bare `--`;
/// the last one wins) and apply it, else FLOE_WORKDIR: the work directory
/// now in force. Err is the command line's fault (a message for exit 2).
pub fn take_option<A: AsRef<OsStr>>(args: &mut Vec<A>) -> Result<Option<PathBuf>, String> {
    let mut given: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        let text = args[i].as_ref().to_string_lossy().into_owned();
        if text == "--" {
            break;
        }
        if text == "--workdir" {
            if i + 1 >= args.len() {
                return Err("--workdir needs a directory".into());
            }
            given = Some(PathBuf::from(args[i + 1].as_ref()));
            args.drain(i..i + 2);
            continue;
        }
        if let Some(value) = text.strip_prefix("--workdir=") {
            given = Some(PathBuf::from(value));
            args.remove(i);
            continue;
        }
        i += 1;
    }
    match given {
        Some(dir) => {
            if dir.as_os_str().is_empty() || dir.to_string_lossy().trim().is_empty() {
                return Err("--workdir needs a directory".into());
            }
            apply(&dir, "--workdir").map(Some)
        }
        None => match std::env::var_os("FLOE_WORKDIR") {
            Some(dir) if !dir.is_empty() => apply(&PathBuf::from(dir), "FLOE_WORKDIR").map(Some),
            _ => Ok(None),
        },
    }
}

/// `dir` as the work directory: `~/` expanded, absolute, made when it is
/// not there (a file in its place, or `/`, refused), set for this process
/// and, as FLOE_WORKDIR, for the ones it starts.
pub fn apply(dir: &Path, what: &str) -> Result<PathBuf, String> {
    let dir = lexical_absolute(&expand_user(dir.to_path_buf()));
    if dir.parent().is_none() {
        return Err(format!("{what}: the root directory cannot be the work directory"));
    }
    match std::fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() || (m.file_type().is_symlink() && dir.is_dir()) => (),
        Ok(_) => return Err(format!("{what}: {} is not a directory", dir.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("{what}: cannot create {}: {e}", dir.display()))?;
        }
        Err(e) => return Err(format!("{what}: {}: {e}", dir.display())),
    }
    // the first setting wins: a name made before this point would have gone
    // beside its source, which no command does before its arguments
    set(dir.clone());
    std::env::set_var("FLOE_WORKDIR", &dir);
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_mirror_is_the_absolute_path_under_the_root() {
        let root = Path::new("/ssd/work");
        assert_eq!(under(root, Path::new("/proj/a/.x.oas.ice")), PathBuf::from("/ssd/work/proj/a/.x.oas.ice"));
        assert_eq!(under(root, Path::new("/r/.run.db.tray")), PathBuf::from("/ssd/work/r/.run.db.tray"));
        // a source inside the work directory mirrors too (no special case)
        assert_eq!(under(root, Path::new("/ssd/work/x/.y.oas.ice")), PathBuf::from("/ssd/work/ssd/work/x/.y.oas.ice"));
        assert_eq!(lexical_absolute(Path::new("/a/./b/../c.ice")), PathBuf::from("/a/c.ice"));
        assert_eq!(lexical_absolute(Path::new("/../x")), PathBuf::from("/x"));
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(lexical_absolute(Path::new("rel/x.oas")), cwd.join("rel/x.oas"));
        assert_eq!(lexical_absolute(Path::new("./x.oas")), cwd.join("x.oas"));
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            assert_eq!(expand_user(PathBuf::from("~/floe-work")), PathBuf::from(home).join("floe-work"));
        }
        assert_eq!(expand_user(PathBuf::from("/abs/~x")), PathBuf::from("/abs/~x"));
    }
    #[test]
    fn the_option_is_taken_from_anywhere_before_a_bare_double_dash() {
        // the stripping, observed through values apply refuses (the
        // process-wide setting is never made here)
        let mut v: Vec<String> = ["--workdir", "/", "render", "x.oas", "--px", "64"].map(String::from).to_vec();
        assert!(take_option(&mut v).unwrap_err().contains("root directory"));
        assert_eq!(v, ["render", "x.oas", "--px", "64"]);
        let mut v: Vec<String> = ["render", "x.oas", "--workdir=", "--out", "a.png"].map(String::from).to_vec();
        assert!(take_option(&mut v).unwrap_err().contains("needs a directory"));
        assert_eq!(v, ["render", "x.oas", "--out", "a.png"]);
        let mut v: Vec<String> = ["index", "--workdir", "/a", "--workdir", "/", "x.oas"].map(String::from).to_vec();
        assert!(take_option(&mut v).unwrap_err().contains("root directory"));
        assert_eq!(v, ["index", "x.oas"]);
        let mut v: Vec<String> = ["info", "x.oas", "--workdir"].map(String::from).to_vec();
        assert!(take_option(&mut v).unwrap_err().contains("needs a directory"));
        let mut v: Vec<String> = ["info", "--workdir", " ", "x.oas"].map(String::from).to_vec();
        assert!(take_option(&mut v).unwrap_err().contains("needs a directory"));
        let mut v: Vec<std::ffi::OsString> = ["index", "--", "--workdir"].map(std::ffi::OsString::from).to_vec();
        // nothing before `--`: the environment's, if any, is applied
        let _ = take_option(&mut v);
        assert_eq!(v, ["index", "--", "--workdir"].map(std::ffi::OsString::from).to_vec());
    }
}
