//! Small Unix identity/lease boundary. No stored PID is used for signalling.
use crate::{Error, Result};
use socket2::Socket;
use std::{fs::File, io, os::fd::AsRawFd};

pub(super) fn uid() -> u32 {
    // SAFETY: no pointer arguments or mutable process state.
    unsafe { libc::geteuid() }
}
pub(super) fn try_lock(file: &File) -> Result<bool> {
    loop {
        // SAFETY: borrowed live file descriptor and valid flock operation.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let e = io::Error::last_os_error();
        match e.kind() {
            io::ErrorKind::WouldBlock => return Ok(false),
            io::ErrorKind::Interrupted => (),
            _ => return Err(e.into()),
        }
    }
}
pub(super) fn same_user(socket: &Socket) -> Result<()> {
    #[cfg(target_os = "linux")]
    let peer = {
        let mut cred = std::mem::MaybeUninit::<libc::ucred>::uninit();
        let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: kernel writes at most size bytes to an allocated ucred.
        let result = unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                cred.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error().into());
        }
        if size as usize != std::mem::size_of::<libc::ucred>() {
            return Err(Error::input("invalid instance peer identity"));
        }
        // SAFETY: successful getsockopt wrote the complete ucred above.
        unsafe { cred.assume_init() }.uid
    };
    #[cfg(target_os = "macos")]
    let peer = {
        let (mut user, mut group) = (0, 0);
        // SAFETY: live connected fd and initialized output variables.
        if unsafe { libc::getpeereid(socket.as_raw_fd(), &mut user, &mut group) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        user
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    compile_error!("launcher peer identity currently supports Linux/macOS");
    if peer == uid() {
        Ok(())
    } else {
        Err(Error::input("instance belongs to another user"))
    }
}
