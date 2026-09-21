//! Descriptor-relative operations for private native download staging.
//! Names are single components; no destructive operation follows a parent path.
use std::ffi::{CStr, CString, OsStr};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
use std::path::Path;

pub struct Entry {
    pub id: (u64, u64),
    pub regular: bool,
    pub bytes: Option<u64>,
}
pub fn leaf(name: &OsStr) -> io::Result<CString> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(io::Error::other("invalid download leaf name"));
    }
    CString::new(bytes).map_err(|_| io::Error::other("NUL in download leaf name"))
}
pub fn open_directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}
pub fn open_at(parent: &File, name: &CStr, directory: bool) -> io::Result<File> {
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory { libc::O_DIRECTORY } else { 0 };
    // SAFETY: borrowed live directory fd and terminated, validated leaf name.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new uniquely owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
#[allow(dead_code)] // Used by the Electron host's cross-filesystem staging copy.
pub fn create_at(parent: &File, name: &CStr) -> io::Result<File> {
    // SAFETY: held directory descriptor and validated leaf; never replace/follow.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: new uniquely owned descriptor returned by openat.
    Ok(unsafe { File::from_raw_fd(fd) })
}
pub fn mkdir_at(parent: &File, name: &CStr) -> io::Result<()> {
    // SAFETY: valid borrowed fd and leaf; never changes global umask.
    let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
    cvt(result)
}
#[allow(clippy::unnecessary_cast)]
pub fn stat_at(parent: &File, name: &CStr) -> io::Result<Entry> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: valid fd/name and one writable stat; do not follow the leaf link.
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    cvt(result)?;
    // SAFETY: successful fstatat initialized the output. ABI widths differ on
    // Darwin/Linux; normalize only the fields the native host actually needs.
    let stat = unsafe { stat.assume_init() };
    Ok(Entry {
        id: (stat.st_dev as u64, stat.st_ino as u64),
        regular: stat.st_mode as u32 & libc::S_IFMT as u32 == libc::S_IFREG as u32,
        bytes: u64::try_from(stat.st_size).ok(),
    })
}
pub fn unlink_at(parent: &File, name: &CStr, directory: bool) -> io::Result<()> {
    // SAFETY: only a validated leaf under a held dir. AT_REMOVEDIR removes only
    // an empty directory; flags=0 unlinks a leaf, never a symlink target.
    let result = unsafe {
        libc::unlinkat(
            parent.as_raw_fd(),
            name.as_ptr(),
            if directory { libc::AT_REMOVEDIR } else { 0 },
        )
    };
    cvt(result)
}
pub fn link_at(source: &File, name: &CStr, target: &File, destination: &CStr) -> io::Result<()> {
    // SAFETY: both dir fds are live and names are single components. linkat is
    // no-clobber; flags=0 does not dereference a substituted source symlink.
    let result = unsafe {
        libc::linkat(
            source.as_raw_fd(),
            name.as_ptr(),
            target.as_raw_fd(),
            destination.as_ptr(),
            0,
        )
    };
    cvt(result)
}
pub fn directory_missing(directory: &File) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    if directory.metadata()?.nlink() == 0 {
        return Ok(true);
    }
    #[cfg(target_os = "macos")]
    {
        // APFS can retain nlink=2 and a cached path after rmdir. F_GETPATH also
        // follows directory renames: inspect that location, but NEVER delete it.
        // An unknown result remains a cleanup error, not permission to retry.
        let mut path = [0u8; libc::MAXPATHLEN as usize];
        // SAFETY: live fd, writable buffer of the documented MAXPATHLEN size.
        cvt(unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_GETPATH, path.as_mut_ptr()) })?;
        let path = CStr::from_bytes_until_nul(&path)
            .map_err(|_| io::Error::other("invalid download directory location"))?;
        match std::fs::symlink_metadata(Path::new(OsStr::from_bytes(path.to_bytes()))) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
            Err(e) => Err(e),
            Ok(_) => Ok(false),
        }
    }
    #[cfg(not(target_os = "macos"))]
    Ok(false)
}
fn cvt(result: libc::c_int) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_cannot_traverse_or_truncate_at_nul() {
        for name in ["", ".", "..", "a/b", "name\0tail"] {
            assert!(leaf(OsStr::new(name)).is_err());
        }
        assert_eq!(
            leaf(OsStr::new("한글 file.txt")).unwrap().to_bytes(),
            "한글 file.txt".as_bytes()
        );
    }
}
