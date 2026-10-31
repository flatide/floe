//! Derived names shared by CLI, readers, protected outputs and resource leases.
//! Resolution is read-only. Migration must not hide a write inside a read scope.
//!
//! The work directory (`--workdir DIR` / FLOE_WORKDIR, user 2026-10-31:
//! "--workdir로 .ice, .tray, .sqlite등의 파일을 읽고 쓰는 위치를 지정할 수
//! 있게"): when one is set, every derived name is placed under it at the
//! source's absolute path - `/a/b/x.oas` has its cache at
//! `DIR/a/b/.x.oas.ice`, `/a/r.db` its pack at `DIR/a/.r.db.tray` - and the
//! files derived from those (the review and analysis sidecars, the
//! revisions, the locks) follow them there. Nothing is read from or written
//! beside the source; a source that has a cache beside it is indexed again
//! under the work directory. The mirror keeps two sources of one name in
//! different folders apart and a folder of it readable by its path. The
//! rule itself is floe_vfs::workdir, shared with floe-index's defaults.
use super::absolute;
use crate::{Error, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub use floe_vfs::workdir::{expand_user, original_of, under as under_workdir, workdir};

/// Set the work directory before any derived name is made (the CLI's
/// `--workdir DIR`, floe-app-cli workdir.rs): whether it took.
pub fn set_workdir(dir: PathBuf) -> bool {
    floe_vfs::workdir::set(dir)
}
/// A derived name where it goes: as made beside the source, or under the
/// work directory (floe_vfs::workdir - the one rule with floe-index's
/// `hidden_sibling`).
fn placed(path: PathBuf) -> PathBuf {
    match workdir() {
        Some(root) => under_workdir(root, &path),
        None => path,
    }
}

fn hidden(source: &Path, suffix: &str) -> Result<PathBuf> {
    let path = absolute(source)?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::input("source must name a file"))?;
    let mut output = std::ffi::OsString::from(".");
    output.push(name);
    output.push(suffix);
    Ok(placed(path.with_file_name(output)))
}
fn appended(source: &Path, suffix: &str) -> Result<PathBuf> {
    let mut path = absolute(source)?.into_os_string();
    path.push(suffix);
    Ok(placed(path.into()))
}
pub fn default_cache_path(source: &Path) -> Result<PathBuf> {
    hidden(source, ".ice")
}
/// Make the folder `source`'s derived names go in when a work directory
/// mirrors it (the first cache or pack of that folder there; nothing is
/// made beside a source, whose own folder exists): for a write that checks
/// its output's folder before its lease makes it (drc::build's pack).
pub fn ensure_workdir_folder(source: &Path) -> Result<()> {
    if workdir().is_some() {
        if let Some(parent) = default_cache_path(source)?.parent() {
            fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}
pub fn cache_paths(source: &Path) -> Result<[PathBuf; 2]> {
    Ok([default_cache_path(source)?, appended(source, ".floe")?])
}
pub fn pack_paths(source: &Path) -> Result<[PathBuf; 2]> {
    Ok([hidden(source, ".tray")?, appended(source, ".ice")?])
}
fn select(paths: [PathBuf; 2]) -> Result<PathBuf> {
    // Even an invalid new destination wins: never silently fall back around
    // corruption, a dangling symlink or access denial. Typed readers validate it.
    for path in &paths {
        match fs::symlink_metadata(path) {
            Ok(_) => return Ok(path.clone()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(paths[0].clone())
}
pub fn cache_path(source: &Path) -> Result<PathBuf> {
    select(cache_paths(source)?)
}
pub fn pack_path(source: &Path) -> Result<PathBuf> {
    select(pack_paths(source)?)
}
/// Logical database name, not the pack spelling; review paths/hash follow GTK.
pub fn database_path(pack: &Path) -> Result<PathBuf> {
    let path = absolute(pack)?;
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| Error::input("DRC filename must be UTF-8"))?;
    let database = if let Some(name) = name.strip_prefix('.').and_then(|n| n.strip_suffix(".tray"))
    {
        name
    } else {
        name.strip_suffix(".ice").unwrap_or(name)
    };
    if database.is_empty() {
        return Err(Error::input("empty DRC database name"));
    }
    Ok(path.with_file_name(database))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolution_is_read_only_and_never_hides_an_invalid_new_destination() {
        let root = std::env::temp_dir().join(format!("floe-cache-names-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let source = root.join("a.oas");
        let [new, old] = cache_paths(&source).unwrap();
        assert_eq!(cache_path(&source).unwrap(), new);
        assert!(!new.exists() && !old.exists());
        fs::create_dir(&old).unwrap();
        fs::write(old.join("sentinel"), b"unchanged").unwrap();
        assert_eq!(cache_path(&source).unwrap(), old);
        assert!(!new.exists());
        fs::write(&new, b"invalid cache").unwrap();
        assert_eq!(cache_path(&source).unwrap(), new);
        fs::remove_file(&new).unwrap();
        std::os::unix::fs::symlink(root.join("absent"), &new).unwrap();
        assert_eq!(cache_path(&source).unwrap(), new);
        fs::remove_file(&new).unwrap();
        assert_eq!(fs::read(old.join("sentinel")).unwrap(), b"unchanged");
        fs::remove_file(old.join("sentinel")).unwrap();
        fs::remove_dir(&old).unwrap();
        let db = root.join("a.db");
        let [new, old] = pack_paths(&db).unwrap();
        fs::write(&old, b"legacy pack").unwrap();
        assert_eq!(pack_path(&db).unwrap(), old);
        assert!(!new.exists());
        fs::write(&new, b"new pack").unwrap();
        assert_eq!(pack_path(&db).unwrap(), new);
        assert_eq!(fs::read(&old).unwrap(), b"legacy pack");
        fs::remove_file(new).unwrap();
        fs::remove_file(old).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn hidden_names_and_logical_database_preserve_unicode_and_dot_files() {
        for (input, new, old) in [
            (
                "/tmp/한 글.oas",
                "/tmp/.한 글.oas.ice",
                "/tmp/한 글.oas.floe",
            ),
            (
                "/tmp/.hidden.oas",
                "/tmp/..hidden.oas.ice",
                "/tmp/.hidden.oas.floe",
            ),
        ] {
            assert_eq!(
                cache_paths(Path::new(input)).unwrap(),
                [PathBuf::from(new), PathBuf::from(old)]
            );
        }
        assert_eq!(
            pack_paths(Path::new("/tmp/a.db")).unwrap(),
            [
                PathBuf::from("/tmp/.a.db.tray"),
                PathBuf::from("/tmp/a.db.ice")
            ]
        );
        for path in ["/tmp/.a.db.tray", "/tmp/a.db.ice", "/tmp/a.db"] {
            assert_eq!(
                database_path(Path::new(path)).unwrap(),
                PathBuf::from("/tmp/a.db")
            );
        }
        assert_eq!(
            database_path(Path::new("/tmp/custom.tray")).unwrap(),
            PathBuf::from("/tmp/custom.tray")
        );
    }

    #[test]
    fn a_work_directory_mirrors_the_absolute_path_under_itself() {
        // the mapping itself (the process-wide setting is the CLI's: read
        // once, so no test sets it)
        let root = Path::new("/ssd/work");
        assert_eq!(
            under_workdir(root, Path::new("/proj/a/.x.oas.ice")),
            PathBuf::from("/ssd/work/proj/a/.x.oas.ice")
        );
        assert_eq!(
            under_workdir(root, Path::new("/proj/a/x.oas.floe")),
            PathBuf::from("/ssd/work/proj/a/x.oas.floe")
        );
        assert_eq!(
            under_workdir(root, Path::new("/r/.run.db.tray")),
            PathBuf::from("/ssd/work/r/.run.db.tray")
        );
        // a source inside the work directory mirrors too (no special case)
        assert_eq!(
            under_workdir(root, Path::new("/ssd/work/x/.y.oas.ice")),
            PathBuf::from("/ssd/work/ssd/work/x/.y.oas.ice")
        );
        // a relative or dotted path is still placed (absolute() ran before)
        assert_eq!(
            under_workdir(root, Path::new("a/./b/../c.ice")),
            PathBuf::from("/ssd/work/a/c.ice")
        );
        // `~` as the shell would give it
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            assert_eq!(
                expand_user(PathBuf::from("~/floe-work")),
                PathBuf::from(home).join("floe-work")
            );
        }
        assert_eq!(expand_user(PathBuf::from("/abs/~x")), PathBuf::from("/abs/~x"));
        // without a work directory the names are beside the source
        if workdir().is_none() {
            assert_eq!(
                cache_paths(Path::new("/tmp/a.oas")).unwrap(),
                [PathBuf::from("/tmp/.a.oas.ice"), PathBuf::from("/tmp/a.oas.floe")]
            );
            assert_eq!(original_of(Path::new("/tmp/.a.oas.ice")), None);
        }
    }
}
