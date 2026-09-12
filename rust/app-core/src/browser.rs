//! A disposable Firefox process group. Never attach to an existing profile or
//! signal a caller-supplied PID. The leader stays unreaped until group cleanup.
use crate::{Error, Result};
use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::Write,
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub struct Browser {
    child: Option<Child>,
    profile: PathBuf,
}
pub fn discover(explicit: Option<&Path>) -> Result<PathBuf> {
    let env = std::env::var_os("FLOE_FIREFOX_BIN").map(PathBuf::from);
    if let Some(path) = explicit.or(env.as_deref()) {
        return executable(path)
            .ok_or_else(|| Error::input("Firefox override is not an executable file"));
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = executable(Path::new(
        "/Applications/Firefox.app/Contents/MacOS/firefox",
    )) {
        return Ok(path);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path).filter(|p| p.is_absolute()) {
            if let Some(path) = executable(&dir.join("firefox")) {
                return Ok(path);
            }
        }
    }
    Err(Error::input(
        "Firefox not found; use --firefox PATH or --no-open with the private session file",
    ))
}
fn executable(path: &Path) -> Option<PathBuf> {
    fs::metadata(path)
        .ok()
        .filter(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)?;
    fs::canonicalize(path).ok()
}
impl Browser {
    pub fn start(binary: &Path, private_directory: &Path, url: &str) -> Result<Self> {
        let (origin, token) = url
            .split_once("/#bootstrap=")
            .ok_or_else(|| Error::input("invalid browser bootstrap URL"))?;
        if !origin
            .strip_prefix("http://127.0.0.1:")
            .is_some_and(|p| p.parse::<u16>().is_ok_and(|p| p > 0))
            || token.len() != 64
            || !token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::input(
                "browser launch requires a local bootstrap URL",
            ));
        }
        let profile = private_directory.join("firefox");
        DirBuilder::new().mode(0o700).create(&profile)?;
        let mut browser = Self {
            child: None,
            profile,
        };
        let mut prefs = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(browser.profile.join("user.js"))?;
        // Only this newly created profile is modified. No user's Firefox prefs,
        // profile registry, cert checks or security protections are changed.
        prefs.write_all(b"user_pref(\"browser.aboutwelcome.enabled\", false);\nuser_pref(\"browser.shell.checkDefaultBrowser\", false);\nuser_pref(\"browser.startup.page\", 0);\nuser_pref(\"browser.sessionstore.resume_from_crash\", false);\nuser_pref(\"datareporting.policy.dataSubmissionEnabled\", false);\n")?;
        drop(prefs);
        // Keep the credential OUT of process argv (ps may be visible to other
        // users). Only this 0600 file contains the one-time fragment URL.
        let launch = browser.profile.join("launch.html");
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&launch)?;
        file.write_all(format!("<!doctype html><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\"><meta http-equiv=\"refresh\" content=\"0;url={url}\"><title>Opening floe2</title>").as_bytes())?;
        drop(file);
        let file_url = format!(
            "file://{}",
            launch
                .as_os_str()
                .as_encoded_bytes()
                .iter()
                .map(|&b| {
                    if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
                        (b as char).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect::<String>()
        );
        browser.child = Some(
            Command::new(binary)
                .args(["--no-remote", "--new-instance", "--profile"])
                .arg(&browser.profile)
                .args(["--new-window", &file_url])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()?,
        );
        Ok(browser)
    }
    pub fn exited(&self) -> Result<bool> {
        let Some(child) = &self.child else {
            return Ok(true);
        };
        let pid = child.id();
        // SAFETY: siginfo_t is a C record admitting all-zero initialization.
        // waitid(WNOWAIT) observes only our Child and does not reap it, reserving
        // the leader PID until close has signalled the owned process group.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        #[cfg(target_os = "linux")]
        // SAFETY: waitid initializes the SIGCHLD union variant (or zero).
        let returned_pid = unsafe { info.si_pid() };
        #[cfg(not(target_os = "linux"))]
        let returned_pid = info.si_pid;
        Ok(returned_pid == pid as i32
            && matches!(
                info.si_code,
                libc::CLD_EXITED | libc::CLD_KILLED | libc::CLD_DUMPED
            ))
    }
    fn signal_group(&self, signal: i32) -> Result<()> {
        let child = self
            .child
            .as_ref()
            .ok_or_else(|| Error::input("browser is already reaped"))?;
        let pid = i32::try_from(child.id()).map_err(|_| Error::input("browser PID range"))?;
        if pid <= 1 {
            return Err(Error::input("invalid browser process group"));
        }
        // SAFETY: process_group(0) made this group's ID equal to the still-owned
        // unreaped child's positive PID. No external PID/group is accepted.
        if unsafe { libc::killpg(pid, signal) } < 0 {
            let e = std::io::Error::last_os_error();
            // XNU killpg1 excludes zombies, so an extant zombie-only group
            // returns EPERM. Do NOT suppress a real permission error: prove
            // this exact owned group contains no live process first.
            #[cfg(target_os = "macos")]
            if e.raw_os_error() == Some(libc::EPERM) && self.exited()? && zombie_only_group(pid)? {
                return Ok(());
            }
            if e.raw_os_error() != Some(libc::ESRCH) {
                return Err(Error::new(
                    crate::ErrorKind::Io,
                    format!("browser process-group signal {signal}: {e}"),
                ));
            }
        }
        Ok(())
    }
    pub fn close(&mut self) -> Result<()> {
        if self.child.is_none() {
            return Ok(());
        }
        self.exited()?; // Prove the unreaped child still belongs to us.
        self.signal_group(libc::SIGTERM)?;
        let end = Instant::now() + Duration::from_secs(1);
        while !self.exited()? && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.signal_group(libc::SIGKILL)?;
        self.child.as_mut().unwrap().wait()?;
        self.child = None;
        fs::remove_dir_all(&self.profile)?;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn zombie_only_group(pid: i32) -> Result<bool> {
    const PROC_PGRP_ONLY: u32 = 2; // macOS SDK sys/proc_info.h.
    let mut pids = [0i32; 4096];
    let size = std::mem::size_of_val(&pids) as i32;
    // SAFETY: libproc fills this bounded i32 array with members of only the
    // known group. Reset errno to distinguish an empty list from a failure.
    unsafe {
        *libc::__error() = 0;
    }
    let bytes =
        unsafe { libc::proc_listpids(PROC_PGRP_ONLY, pid as u32, pids.as_mut_ptr().cast(), size) };
    if bytes < 0 || (bytes == 0 && std::io::Error::last_os_error().raw_os_error() != Some(0)) {
        return Err(std::io::Error::last_os_error().into());
    }
    if bytes >= size || bytes % 4 != 0 {
        return Err(Error::input("browser process group inspection limit"));
    }
    for &member in &pids[..bytes as usize / 4] {
        if member == 0 || member == pid {
            continue;
        } // Own leader was proven exited by waitid.
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        // SAFETY: buffer is correctly sized/aligned; proc_pidinfo only reads
        // metadata. This never signals a PID discovered by enumeration.
        let read = unsafe {
            libc::proc_pidinfo(
                member,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if read != size {
            let e = std::io::Error::last_os_error();
            if read == 0 && e.raw_os_error() == Some(libc::ESRCH) {
                continue;
            }
            return Err(e.into());
        }
        // SAFETY: a complete proc_bsdinfo was initialized above.
        let info = unsafe { info.assume_init() };
        if info.pbi_pgid == pid as u32 && info.pbi_status != libc::SZOMB {
            return Ok(false);
        }
    }
    Ok(true)
}
impl Drop for Browser {
    fn drop(&mut self) {
        if self.child.is_some() {
            if self.close().is_err() {
                // Do not delete a profile potentially still used by a process.
                eprintln!(
                    "[floe2-web] Firefox cleanup failed; private profile retained: {}",
                    self.profile.display()
                );
            }
        } else if self.profile.exists() {
            let _ = fs::remove_dir_all(&self.profile);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_group_is_observed_without_reaping_then_closed() {
        let root = std::env::temp_dir().join(format!(
            "floe-browser-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        DirBuilder::new().mode(0o700).create(&root).unwrap();
        let script = root.join("firefox");
        fs::write(&script, b"#!/bin/sh\nkill -STOP \"$$\"\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let url = format!("http://127.0.0.1:1/#bootstrap={}", "a".repeat(64));
        let mut b = Browser::start(&script, &root.join("session"), &url).err();
        assert!(b.take().is_some()); // No implicit arbitrary parent creation.
        let session = root.join("session");
        DirBuilder::new().mode(0o700).create(&session).unwrap();
        let mut b = Browser::start(&script, &session, &url).unwrap();
        assert!(!b.exited().unwrap());
        std::thread::sleep(Duration::from_millis(50));
        assert!(!b.exited().unwrap());
        b.close().unwrap();
        assert!(b.exited().unwrap());
        assert!(!session.join("firefox").exists());
        drop(b);
        fs::remove_dir_all(root).unwrap();
    }
}
