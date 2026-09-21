//! One native download slot. Private pipes only; never an HTTP/native-JS API.
#![deny(unsafe_op_in_unsafe_fn)]
#[path = "../../../desktop/src/download_fs.rs"]
mod download_fs;
#[allow(dead_code)]
#[path = "../../../desktop/src/transfers.rs"]
mod transfers;

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn stream(fd: i32) -> bool {
    // Private inherited FIFO/AF_UNIX only. Reject regular files, TTY, TCP.
    unsafe {
        let mut s: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut s) != 0 {
            return false;
        }
        if s.st_mode & libc::S_IFMT == libc::S_IFIFO {
            return true;
        }
        if s.st_mode & libc::S_IFMT != libc::S_IFSOCK {
            return false;
        }
        let mut address: libc::sockaddr_storage = std::mem::zeroed();
        let mut size = std::mem::size_of_val(&address) as libc::socklen_t;
        libc::getsockname(
            fd,
            (&mut address as *mut libc::sockaddr_storage).cast(),
            &mut size,
        ) == 0
            && address.ss_family as i32 == libc::AF_UNIX
    }
}
fn read_json(input: &mut impl Read) -> Result<Value> {
    let mut bytes = Vec::new();
    loop {
        let mut b = [0];
        input.read_exact(&mut b)?;
        if b[0] == b'\n' {
            return Ok(serde_json::from_slice(&bytes)?);
        }
        if bytes.len() == 65536 {
            return Err("oversized private request".into());
        }
        bytes.push(b[0]);
    }
}
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    let o = value.as_object().ok_or("invalid request")?;
    if o.len() != 2 || o.get("v") != Some(&json!(1)) {
        return Err("invalid request".into());
    }
    o.get(key)
        .and_then(Value::as_str)
        .filter(|s| s.len() <= 8192 && !s.contains('\0'))
        .ok_or_else(|| "invalid request".into())
}
fn send(value: Value) -> Result<()> {
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(&mut output, &value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}
fn readable(timeout: i32) -> Result<bool> {
    let mut fd = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one live stdin descriptor and one writable poll structure.
    let n = unsafe { libc::poll(&mut fd, 1, timeout) };
    if n < 0 {
        let e = std::io::Error::last_os_error();
        if e.kind() == std::io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(e.into());
    }
    Ok(n != 0)
}
fn run() -> Result<()> {
    if std::env::args_os().len() != 1 || !stream(0) || !stream(1) {
        return Err("private pipes required".into());
    }
    // StdinLock has an internal BufReader: it can hide a pipelined command
    // from poll(0). Read the inherited stream without userspace read-ahead.
    let fd = unsafe { libc::fcntl(0, libc::F_DUPFD_CLOEXEC, 3) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: fcntl returned a new uniquely owned descriptor.
    let mut input = unsafe { std::fs::File::from_raw_fd(fd) };
    let init = read_json(&mut input)?;
    let root = Path::new(field(&init, "directory")?);
    if !root.is_absolute() {
        return Err("absolute directory required".into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, stop.clone())?;
    }
    let pending = transfers::PendingFile::new(&root.join("unpublished-download"))?;
    send(
        json!({"v":1,"event":"ready","path":pending.staging_for_download()?.to_str().ok_or("UTF-8 path required")?}),
    )?;
    let mut command = Vec::new();
    let mut watching = false;
    let mut limit_sent = false;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if watching
            && !limit_sent
            && pending
                .received_size()
                .map_or(true, |n| n.is_some_and(|n| n > transfers::MAX_BYTES))
        {
            send(json!({"v":1,"event":"limit"}))?;
            limit_sent = true;
        }
        if !readable(50)? {
            continue;
        }
        let mut byte = [0];
        if input.read(&mut byte)? == 0 {
            break;
        }
        if byte[0] != b'\n' {
            if command.len() == 65536 {
                return Err("oversized private request".into());
            }
            command.push(byte[0]);
            continue;
        }
        let value: Value = serde_json::from_slice(&command)?;
        command.clear();
        if field(&value, "command").is_ok_and(|v| v == "watch") && !watching {
            watching = true;
            continue;
        }
        if field(&value, "command").is_ok_and(|v| v == "discard") {
            break;
        }
        if field(&value, "command").is_ok_and(|v| v == "retain") {
            pending.retain(); // producer termination was not confirmed
            send(json!({"v":1,"event":"done","publication":"not_requested","cleanup":false}))?;
            return Err("producer termination unconfirmed".into());
        }
        let destination = Path::new(field(&value, "publish")?);
        if !watching || !destination.is_absolute() {
            return Err("invalid publication request".into());
        }
        let result = if limit_sent {
            transfers::Publication {
                publication: Err(std::io::Error::other("download rejected")),
                cleanup: pending.discard(),
            }
        } else {
            pending.copy_publish(destination, || {
                stop.load(Ordering::Relaxed) || readable(0).unwrap_or(true)
            })
        };
        send(
            json!({"v":1,"event":"done","publication":if result.publication.is_ok(){"saved"}else{"unconfirmed"},"cleanup":result.cleanup.is_ok()}),
        )?;
        return if result.cleanup.is_ok() {
            Ok(())
        } else {
            Err("cleanup incomplete".into())
        };
    }
    let result = pending.discard();
    let _ =
        send(json!({"v":1,"event":"done","publication":"not_requested","cleanup":result.is_ok()}));
    result.map_err(Into::into)
}
fn main() {
    if run().is_err() {
        eprintln!("floe-electron-download: transfer not confirmed; inspect the selected folder; no replay");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_frames_and_bounded_utf8() {
        assert_eq!(
            field(&json!({"v":1,"publish":"/tmp/한글"}), "publish").unwrap(),
            "/tmp/한글"
        );
        for v in [
            json!({"v":2,"command":"watch"}),
            json!({"v":1,"publish":"a\0b"}),
            json!({"v":1,"publish":"x","extra":1}),
        ] {
            assert!(field(&v, "publish").is_err());
        }
        assert!(read_json(&mut &b"\xff\n"[..]).is_err());
        assert!(read_json(&mut &vec![b'x'; 65538][..]).is_err());
    }
}
