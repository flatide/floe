//! Operator-only credential input. Never accept a secret in argv/JSON config.
use crate::{Error, Result};
use std::{
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

pub fn read_proxy_key(path: &Path) -> Result<String> {
    let failure = || {
        Error::input(
            "proxy key must be an owned private regular file containing 64 lowercase hex digits",
        )
    };
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| failure())?;
    let m = file.metadata().map_err(|_| failure())?;
    // SAFETY: geteuid has no arguments and does not access caller memory.
    let uid = unsafe { libc::geteuid() };
    if !m.is_file()
        || m.uid() != uid
        || m.mode() & 0o077 != 0
        || m.nlink() != 1
        || !(64..=65).contains(&m.len())
    {
        return Err(failure());
    }
    let mut bytes = Vec::new();
    file.take(66)
        .read_to_end(&mut bytes)
        .map_err(|_| failure())?;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.len() != 64
        || !bytes
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
    {
        return Err(failure());
    }
    String::from_utf8(bytes).map_err(|_| failure())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{symlink, PermissionsExt},
    };
    #[test]
    fn credential_input_is_private_bounded_and_nonblocking() {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let dir =
            std::env::temp_dir().join(format!("floe-demo-key-{:x}", u128::from_ne_bytes(random)));
        fs::create_dir(&dir).unwrap();
        let key = dir.join("key");
        let text = "a".repeat(64);
        fs::write(&key, format!("{text}\n")).unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_proxy_key(&key).unwrap(), text);
        fs::set_permissions(&key, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(read_proxy_key(&key).is_err());
        fs::set_permissions(&key, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(read_proxy_key(&key).is_ok());
        symlink(&key, dir.join("link")).unwrap();
        assert!(read_proxy_key(&dir.join("link")).is_err());
        fs::hard_link(&key, dir.join("hard")).unwrap();
        assert!(read_proxy_key(&key).is_err());
        fs::remove_file(dir.join("hard")).unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
        for bad in [
            "A".repeat(64),
            "a".repeat(63),
            "a".repeat(66),
            format!("{text}\r\n"),
        ] {
            fs::write(&key, bad).unwrap();
            assert!(read_proxy_key(&key).is_err());
        }
        assert!(read_proxy_key(&dir).is_err());
        let fifo = dir.join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: the CString remains live and points to a terminated path.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(read_proxy_key(&fifo).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
