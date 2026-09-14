//! Metadata copied only from local file descriptors, never from a request.
//! Refuse unsupported metadata rather than publish a differently secured file.
use super::unsupported;
use crate::Result;
use std::{
    collections::BTreeMap,
    ffi::{CStr, CString},
    fs::{File, Permissions},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, PermissionsExt},
    },
};

const MAX_NAMES: usize = 65536;
const MAX_VALUES: usize = 1024 * 1024;

#[derive(PartialEq, Eq)]
pub(crate) struct Security {
    mode: u32,
    uid: u32,
    gid: u32,
    attrs: BTreeMap<CString, Vec<u8>>,
    #[cfg(target_os = "macos")]
    acl: Vec<u8>,
}
impl Security {
    pub(crate) fn attribute(&self, name: &CStr) -> Option<&[u8]> {
        self.attrs.get(name).map(Vec::as_slice)
    }
    pub fn empty() -> Self {
        Self {
            mode: 0,
            uid: 0,
            gid: 0,
            attrs: BTreeMap::new(),
            #[cfg(target_os = "macos")]
            acl: Vec::new(),
        }
    }
    pub fn read(file: &File) -> Result<Self> {
        let m = file.metadata()?;
        if m.mode() & 0o7000 != 0 {
            return Err(unsupported(
                "special permission bits on design default are unsupported",
            ));
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::macos::fs::MetadataExt;
            if m.st_flags() != 0 {
                return Err(unsupported(
                    "BSD file flags on design default are unsupported",
                ));
            }
        }
        let attrs = attributes(file)?;
        if attrs.contains_key(c"security.capability") {
            return Err(unsupported(
                "executable capabilities on design default are unsupported",
            ));
        }
        Ok(Self {
            mode: m.mode() & 0o777,
            uid: m.uid(),
            gid: m.gid(),
            attrs,
            #[cfg(target_os = "macos")]
            acl: acl::read(file)?,
        })
    }
    pub fn private(file: &File) -> Result<()> {
        #[cfg(target_os = "macos")]
        acl::clear(file)?;
        file.set_permissions(Permissions::from_mode(0o600))?;
        Ok(())
    }
    pub fn apply(&self, file: &File) -> Result<()> {
        let m = file.metadata()?;
        if (m.uid(), m.gid()) != (self.uid, self.gid) {
            // SAFETY: live fd and OS-origin owner IDs. No privilege elevation.
            checked(unsafe { libc::fchown(file.as_raw_fd(), self.uid, self.gid) })?;
        }
        let current = attributes(file)?;
        for name in current.keys() {
            if !self.attrs.contains_key(name) {
                remove(file, name)?;
            }
        }
        for (name, value) in &self.attrs {
            if current.get(name) != Some(value) {
                set(file, name, value)?;
            }
        }
        #[cfg(target_os = "macos")]
        acl::apply(file, &self.acl)?;
        // POSIX ACL mask and mode group bits agree in the original snapshot.
        file.set_permissions(Permissions::from_mode(self.mode))?;
        Ok(())
    }
}
fn checked(rc: i32) -> std::io::Result<()> {
    if rc < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn list(file: &File, out: &mut [u8]) -> isize {
    let p = if out.is_empty() {
        std::ptr::null_mut()
    } else {
        out.as_mut_ptr().cast()
    };
    // SAFETY: live fd and writable buffer (or null size query).
    unsafe {
        #[cfg(target_os = "macos")]
        {
            libc::flistxattr(file.as_raw_fd(), p, out.len(), 0)
        }
        #[cfg(not(target_os = "macos"))]
        {
            libc::flistxattr(file.as_raw_fd(), p, out.len())
        }
    }
}
fn get(file: &File, name: &CStr, out: &mut [u8]) -> isize {
    let p = if out.is_empty() {
        std::ptr::null_mut()
    } else {
        out.as_mut_ptr().cast()
    };
    // SAFETY: live fd, valid CString and bounded writable buffer.
    unsafe {
        #[cfg(target_os = "macos")]
        {
            libc::fgetxattr(file.as_raw_fd(), name.as_ptr(), p, out.len(), 0, 0)
        }
        #[cfg(not(target_os = "macos"))]
        {
            libc::fgetxattr(file.as_raw_fd(), name.as_ptr(), p, out.len())
        }
    }
}
fn length(n: isize, max: usize) -> Result<usize> {
    if n < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if n as usize > max {
        return Err(unsupported(
            "design-default metadata exceeds preservation limit",
        ));
    }
    Ok(n as usize)
}
fn attributes(file: &File) -> Result<BTreeMap<CString, Vec<u8>>> {
    let n = length(list(file, &mut []), MAX_NAMES)?;
    let mut names = vec![0; n];
    if length(list(file, &mut names), MAX_NAMES)? != n {
        return Err(super::conflict());
    }
    if n != 0 && names.last() != Some(&0) {
        return Err(unsupported("invalid extended attribute list"));
    }
    let mut result = BTreeMap::new();
    let mut left = MAX_VALUES;
    for bytes in names.split(|b| *b == 0).filter(|b| !b.is_empty()) {
        let name = CString::new(bytes).map_err(|_| unsupported("invalid attribute name"))?;
        let size = length(get(file, &name, &mut []), left)?;
        let mut value = vec![0; size];
        if length(get(file, &name, &mut value), left)? != size {
            return Err(super::conflict());
        }
        left -= size;
        if result.insert(name, value).is_some() {
            return Err(super::conflict());
        }
    }
    Ok(result)
}
pub(crate) fn set(file: &File, name: &CStr, value: &[u8]) -> Result<()> {
    // SAFETY: valid fd/CString and readable value buffer, flags 0 (set/replace).
    let rc = unsafe {
        #[cfg(target_os = "macos")]
        {
            libc::fsetxattr(
                file.as_raw_fd(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                0,
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            libc::fsetxattr(
                file.as_raw_fd(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
            )
        }
    };
    checked(rc)?;
    Ok(())
}
fn remove(file: &File, name: &CStr) -> Result<()> {
    // SAFETY: live fd and valid NUL-terminated attribute name.
    let rc = unsafe {
        #[cfg(target_os = "macos")]
        {
            libc::fremovexattr(file.as_raw_fd(), name.as_ptr(), 0)
        }
        #[cfg(not(target_os = "macos"))]
        {
            libc::fremovexattr(file.as_raw_fd(), name.as_ptr())
        }
    };
    checked(rc)?;
    Ok(())
}

#[cfg(target_os = "macos")]
mod acl {
    use super::*;
    use std::ffi::c_void;
    // Darwin sys/acl.h. libc does not expose these; no external libacl is used.
    const EXTENDED: i32 = 0x100;
    unsafe extern "C" {
        fn acl_get_fd_np(fd: i32, kind: i32) -> *mut c_void;
        fn acl_set_fd_np(fd: i32, acl: *mut c_void, kind: i32) -> i32;
        fn acl_size(acl: *mut c_void) -> isize;
        fn acl_copy_ext(buf: *mut c_void, acl: *mut c_void, len: isize) -> isize;
        fn acl_copy_int(buf: *const c_void) -> *mut c_void;
        fn acl_init(count: i32) -> *mut c_void;
        fn acl_free(acl: *mut c_void) -> i32;
    }
    struct Acl(*mut c_void);
    impl Acl {
        fn new(p: *mut c_void) -> Result<Self> {
            if p.is_null() {
                Err(std::io::Error::last_os_error().into())
            } else {
                Ok(Self(p))
            }
        }
    }
    impl Drop for Acl {
        fn drop(&mut self) {
            // SAFETY: owned ACL returned by Darwin, released exactly once.
            unsafe {
                acl_free(self.0);
            }
        }
    }
    pub fn read(file: &File) -> Result<Vec<u8>> {
        // SAFETY: live fd; result becomes RAII-owned.
        let raw = unsafe { acl_get_fd_np(file.as_raw_fd(), EXTENDED) };
        // Darwin reports ENOENT for an existing file with no extended ACL.
        if raw.is_null() && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            return Ok(Vec::new());
        }
        let acl = Acl::new(raw)?;
        // SAFETY: live opaque ACL handle.
        let n = length(unsafe { acl_size(acl.0) }, MAX_VALUES)?;
        let mut data = vec![0; n];
        // SAFETY: buffer sized by acl_size, with the same live ACL handle.
        if length(
            unsafe { acl_copy_ext(data.as_mut_ptr().cast(), acl.0, n as isize) },
            MAX_VALUES,
        )? != n
        {
            return Err(super::super::conflict());
        }
        Ok(data)
    }
    pub fn clear(file: &File) -> Result<()> {
        // SAFETY: valid empty ACL request; result becomes RAII-owned.
        let acl = Acl::new(unsafe { acl_init(0) })?;
        // SAFETY: live fd/ACL, documented Darwin ACL type.
        checked(unsafe { acl_set_fd_np(file.as_raw_fd(), acl.0, EXTENDED) })?;
        Ok(())
    }
    pub fn apply(file: &File, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            return clear(file);
        }
        // SAFETY: only complete acl_copy_ext output reaches this private method;
        // no user-provided bytes or serialized drafts can reach acl_copy_int.
        let acl = Acl::new(unsafe { acl_copy_int(data.as_ptr().cast()) })?;
        // SAFETY: live fd and owned ACL of the documented Darwin ACL type.
        checked(unsafe { acl_set_fd_np(file.as_raw_fd(), acl.0, EXTENDED) })?;
        Ok(())
    }
}
