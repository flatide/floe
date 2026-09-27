//! Destructive reclamation is opt-in on a small, explicit local-FS allowlist.
//! NFS/SMB/FUSE/overlay and unknown filesystems fail closed; inventory still works.
use super::*;
use std::os::fd::AsRawFd;

pub(super) fn require(file: &File) -> Result<()> {
    let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: live fd and writable statfs storage; only assume initialization
    // on syscall success. No paths or credentials pass across this boundary.
    if unsafe { libc::fstatfs(file.as_raw_fd(), info.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let info = unsafe { info.assume_init() };
    #[cfg(target_os = "linux")]
    let supported = supported_linux(info.f_type as u64);
    #[cfg(target_os = "macos")]
    let supported = {
        let name: Vec<u8> = info
            .f_fstypename
            .iter()
            .map(|&c| c as u8)
            .take_while(|&c| c != 0)
            .collect();
        supported_macos(&name, info.f_flags & libc::MNT_LOCAL as u32 != 0)
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let supported = false;
    if !supported {
        return Err(Error::new(ErrorKind::Unsupported, "revision reclamation requires a supported local filesystem; network/shared mounts are protected"));
    }
    Ok(())
}

#[cfg(any(test, target_os = "linux"))]
fn supported_linux(kind: u64) -> bool {
    matches!(kind, 0xef53 | 0x58465342 | 0x9123683e | 0x01021994)
}
#[cfg(any(test, target_os = "macos"))]
fn supported_macos(name: &[u8], local: bool) -> bool {
    local && matches!(name, b"apfs" | b"hfs")
}

#[cfg(test)]
#[test]
fn shared_virtual_and_unknown_filesystems_are_not_deletion_authority() {
    for kind in [0x6969, 0x517b, 0xff534d42, 0x65735546, 0x794c7630, 0] {
        assert!(!supported_linux(kind));
    }
    for kind in [0xef53, 0x58465342, 0x9123683e, 0x01021994] {
        assert!(supported_linux(kind));
    }
    for name in [b"nfs".as_slice(), b"smbfs", b"unknown"] {
        assert!(!supported_macos(name, true));
    }
    assert!(!supported_macos(b"apfs", false));
    assert!(supported_macos(b"apfs", true));
}
