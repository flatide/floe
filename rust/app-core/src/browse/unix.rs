//! Descriptor-relative, non-following directory traversal for the file picker.
//! No caller-supplied path is evaluated by this module after root admission.
use crate::{Error, Result};
use std::{
    ffi::{CStr, CString, OsStr, OsString},
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, IntoRawFd},
        unix::ffi::{OsStrExt, OsStringExt},
    },
    path::{Component, Path},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
    pub device: u64,
    pub inode: u64,
    pub kind: u32,
    pub size: i64,
    pub modified: (i64, i64),
    pub changed: (i64, i64),
}
impl Stamp {
    // libc's stat fields and S_IF* constants have different widths on Darwin
    // and Linux. Keep the normalized witness identical on both supported ABIs.
    #[allow(clippy::unnecessary_cast)]
    fn from_stat(s: libc::stat) -> Self {
        Self {
            device: s.st_dev as u64,
            inode: s.st_ino as u64,
            kind: (s.st_mode as u32) & libc::S_IFMT as u32,
            size: s.st_size,
            modified: (s.st_mtime, s.st_mtime_nsec),
            changed: (s.st_ctime, s.st_ctime_nsec),
        }
    }
    #[allow(clippy::unnecessary_cast)]
    pub fn is_dir(self) -> bool {
        self.kind == libc::S_IFDIR as u32
    }
    #[allow(clippy::unnecessary_cast)]
    pub fn is_file(self) -> bool {
        self.kind == libc::S_IFREG as u32
    }
    pub fn same_node(self, other: Self) -> bool {
        (self.device, self.inode, self.kind) == (other.device, other.inode, other.kind)
    }
}
fn name(name: &OsStr) -> Result<CString> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b".." || bytes.contains(&b'/') {
        return Err(Error::input("invalid picker component"));
    }
    CString::new(bytes).map_err(|_| Error::input("invalid picker component"))
}
pub(super) fn open(parent: &File, component: &OsStr, directory: bool) -> Result<File> {
    let name = name(component)?;
    // O_NONBLOCK prevents a concurrent regular-file -> FIFO substitution from
    // hanging selection. The caller verifies kind + inode on the returned fd.
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory { libc::O_DIRECTORY } else { 0 };
    // SAFETY: parent fd is live, CString is terminated, no mode is needed.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: successful openat transferred a unique fd to this owner.
    Ok(unsafe { File::from_raw_fd(fd) })
}
pub(super) fn root(path: &Path) -> Result<File> {
    if !path.is_absolute() {
        return Err(Error::input("picker root must be absolute"));
    }
    let mut file = File::open("/")?;
    for part in path.components() {
        match part {
            Component::RootDir => (),
            Component::Normal(n) => file = open(&file, n, true)?,
            _ => return Err(Error::input("invalid picker root")),
        }
    }
    Ok(file)
}
pub(super) fn stamp(file: &File) -> Result<Stamp> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: stat points to one allocated output, fd is borrowed and live.
    if unsafe { libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: successful fstat initialized the complete output.
    Ok(Stamp::from_stat(unsafe { stat.assume_init() }))
}
pub(super) fn at(parent: &File, component: &OsStr) -> Result<Stamp> {
    let name = name(component)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: one live dir fd, terminated name, allocated stat output.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: successful fstatat initialized the output.
    Ok(Stamp::from_stat(unsafe { stat.assume_init() }))
}

pub(super) struct Reader(*mut libc::DIR);
impl Reader {
    pub fn new(directory: &File) -> Result<Self> {
        // dup() would share the directory position with another reader. A new
        // open description of "." makes every listing start independently.
        let file = open(directory, OsStr::new("."), true)?;
        let fd = file.into_raw_fd();
        // SAFETY: fd owns a readable directory; fdopendir takes it on success.
        let dir = unsafe { libc::fdopendir(fd) };
        if dir.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: on failure fdopendir did not take ownership.
            drop(unsafe { File::from_raw_fd(fd) });
            return Err(error.into());
        }
        Ok(Self(dir))
    }
    pub fn next(&mut self) -> Result<Option<OsString>> {
        // readdir reports both EOF and error as null. Reset this thread's errno
        // before calling, without changing any other thread's error state.
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error() = 0;
        }
        #[cfg(target_os = "linux")]
        unsafe {
            *libc::__errno_location() = 0;
        }
        // SAFETY: this Reader exclusively owns a live DIR; returned bytes are
        // copied before the next call and never escape with their raw lifetime.
        let entry = unsafe { libc::readdir(self.0) };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(0) {
                Ok(None)
            } else {
                Err(error.into())
            };
        }
        // SAFETY: a successful readdir guarantees NUL-terminated d_name.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        Ok(Some(OsString::from_vec(name.to_bytes().to_owned())))
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: this is the sole owner, and closedir also closes its fd.
        unsafe {
            libc::closedir(self.0);
        }
    }
}
