//! Derived names shared by CLI, readers, protected outputs and resource leases.
//! Resolution is read-only. Migration must not hide a write inside a read scope.
use super::absolute;
use crate::{Error, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn hidden(source: &Path, suffix: &str) -> Result<PathBuf> {
    let path = absolute(source)?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::input("source must name a file"))?;
    let mut output = std::ffi::OsString::from(".");
    output.push(name);
    output.push(suffix);
    Ok(path.with_file_name(output))
}
fn appended(source: &Path, suffix: &str) -> Result<PathBuf> {
    let mut path = absolute(source)?.into_os_string();
    path.push(suffix);
    Ok(path.into())
}
pub fn default_cache_path(source: &Path) -> Result<PathBuf> {
    hidden(source, ".ice")
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
}
