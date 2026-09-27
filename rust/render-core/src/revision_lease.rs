//! The native reader owns its lease independently of the UI/server process.
//! Legacy caches have no revision seal and keep their existing open behavior.
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

pub(crate) fn open(path: &Path) -> Result<Option<File>, String> {
    let seal = path.join("revision.json");
    match fs::symlink_metadata(&seal) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("revision seal: {e}")),
        Ok(m) if m.is_file() && m.nlink() == 1 => (),
        Ok(_) => return Err("revision seal must be a private regular file".into()),
    }
    // Canonicalize the worker's trusted source alias, then bind the lock to
    // this concrete directory. No manifest path or ownership field is followed.
    let canonical = fs::canonicalize(path).map_err(|e| format!("revision directory: {e}"))?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&canonical)
        .map_err(|e| format!("revision directory: {e}"))?;
    file.try_lock_shared()
        .map_err(|e| format!("revision is locked or cannot be pinned: {e}"))?;
    let pinned = file.metadata().map_err(|e| e.to_string())?;
    let now = fs::symlink_metadata(&canonical).map_err(|e| e.to_string())?;
    let seal = fs::symlink_metadata(canonical.join("revision.json")).map_err(|e| e.to_string())?;
    if !now.is_dir()
        || (pinned.dev(), pinned.ino()) != (now.dev(), now.ino())
        || !seal.is_file()
        || seal.nlink() != 1
    {
        return Err("revision changed while acquiring native pin".into());
    }
    Ok(Some(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seal_opt_in_pins_until_native_cache_drops_and_fails_closed() {
        let path = std::env::temp_dir().join(format!("floe-native-lease-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        assert!(open(&path).unwrap().is_none());
        fs::write(path.join("revision.json"), b"synthetic seal").unwrap();
        let native = open(&path).unwrap().unwrap();
        let probe = File::open(&path).unwrap();
        assert!(matches!(
            probe.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(native);
        probe.try_lock().unwrap();
        assert!(open(&path).is_err());
        drop(probe);
        fs::remove_file(path.join("revision.json")).unwrap();
        std::os::unix::fs::symlink("missing", path.join("revision.json")).unwrap();
        assert!(open(&path).is_err());
        fs::remove_file(path.join("revision.json")).unwrap();
        fs::remove_dir(path).unwrap();
    }
}
