//! Private inherited-pipe bridge, not an HTTP authority or a second CLI parser.
//! Existing Rust Session owns all geometry, files, authorization and cleanup.
#![deny(unsafe_op_in_unsafe_fn)]

#[path = "../../../desktop/src/service.rs"]
mod service;

use floe_app::embedded::{validate_ready, Session};
use floe_app_core::{Error, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

const MAX_LINE: usize = 65_536;

fn invalid() -> Error {
    Error::input("invalid Electron host protocol")
}

fn read_json(input: &mut impl Read) -> Result<Value> {
    // No read-ahead: a pipelined cancel must remain visible to the later poll.
    // Initial messages are bounded; no service/child exists during this read.
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0];
        input.read_exact(&mut byte).map_err(|_| invalid())?;
        if byte[0] == b'\n' {
            return serde_json::from_slice(&bytes).map_err(|_| invalid());
        }
        if bytes.len() == MAX_LINE {
            return Err(invalid());
        }
        bytes.push(byte[0]);
    }
}

fn args(value: &Value) -> Result<Vec<String>> {
    let object = value.as_object().ok_or_else(invalid)?;
    if object.len() != 2 || object.get("v") != Some(&json!(1)) {
        return Err(invalid());
    }
    let args = object
        .get("args")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if args.len() > 256 {
        return Err(invalid());
    }
    args.iter()
        .map(|a| {
            a.as_str()
                .filter(|s| s.len() <= 8192 && !s.contains('\0'))
                .map(str::to_owned)
                .ok_or_else(invalid)
        })
        .collect()
}

fn directory(value: &Value) -> Result<&str> {
    let object = value.as_object().ok_or_else(invalid)?;
    if object.len() != 2 || object.get("v") != Some(&json!(1)) {
        return Err(invalid());
    }
    object
        .get("directory")
        .and_then(Value::as_str)
        .filter(|s| Path::new(s).is_absolute() && !s.contains('\0'))
        .ok_or_else(invalid)
}

fn send(value: Value) -> Result<()> {
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(&mut output, &value).map_err(|_| invalid())?;
    output.write_all(b"\n").map_err(|_| invalid())?;
    output.flush().map_err(|_| invalid())
}

#[cfg(unix)]
fn private_pipe(fd: libc::c_int) -> bool {
    // Reject terminals, regular files and TCP sockets before any credential is
    // created. Node uses inherited Unix sockets; Rust/Python spawn uses pipes.
    unsafe {
        let mut stat: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut stat) != 0 {
            return false;
        }
        match stat.st_mode & libc::S_IFMT {
            libc::S_IFIFO => true,
            libc::S_IFSOCK => {
                let mut address: libc::sockaddr_storage = std::mem::zeroed();
                let mut len = std::mem::size_of_val(&address) as libc::socklen_t;
                libc::getsockname(
                    fd,
                    (&mut address as *mut libc::sockaddr_storage).cast(),
                    &mut len,
                ) == 0
                    && address.ss_family as libc::c_int == libc::AF_UNIX
            }
            _ => false,
        }
    }
}

#[cfg(unix)]
fn run() -> Result<i32> {
    if std::env::args_os().len() != 1 || !private_pipe(0) || !private_pipe(1) {
        return Err(invalid());
    }
    let mut input = std::io::stdin().lock();
    let request = read_json(&mut input)?;
    let mut session = Session::parse(&args(&request)?)?;
    if session.needs_initial_directory() {
        send(json!({"v":1,"event":"directory"}))?;
        let choice = read_json(&mut input)?;
        session.set_initial_directory(Path::new(directory(&choice)?))?;
    }
    let mut service = service::Service::start(session)?;
    let mut closing = false;
    let mut bad_control = false;
    let mut command = Vec::new();
    while !service.finished() {
        if !closing {
            if let Ok(ready) = service.ready.try_recv() {
                validate_ready(&ready)?;
                // Deliberately serialize here only: private pipe, never argv,
                // diagnostic stderr, a credential file, or a public listener.
                send(json!({"v":1,"event":"ready","origin":ready.origin,"url":ready.url}))?;
            }
            let mut poll = libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            };
            let result = unsafe { libc::poll(&mut poll, 1, 50) };
            if result < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(invalid());
            }
            if result > 0 {
                let mut bytes = [0; 64];
                let n = input.read(&mut bytes).map_err(|_| invalid())?;
                if n == 0 {
                    closing = true; // parent death/EOF revokes the whole session
                } else {
                    command.extend_from_slice(&bytes[..n]);
                    if !b"cancel\n".starts_with(&command) {
                        bad_control = true;
                        closing = true;
                    } else if command == b"cancel\n" {
                        closing = true;
                    }
                }
                if closing {
                    service.cancel();
                }
            }
        } else {
            // A hung-up descriptor would make poll spin. Cleanup remains in the
            // owned Rust service; never detach/kill unknown processes here.
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    let code = service.join()?;
    if bad_control {
        Err(invalid())
    } else {
        Ok(code)
    }
}

fn main() {
    #[cfg(unix)]
    let result = run();
    #[cfg(not(unix))]
    let result: Result<i32> = Err(invalid());
    match result {
        Ok(code) => std::process::exit(code),
        Err(_) => {
            // Never echo malformed input, paths or bootstrap values. The shell
            // reports this fixed error; raw service stderr is not a wire frame.
            eprintln!(
                "floe-electron-service: startup/protocol/service failure; no request replayed"
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn bounded_json_preserves_utf8_and_pipelined_cancel() {
        let mut stream = Cursor::new("{\"v\":1,\"args\":[\"한국 칩.oas\"]}\ncancel\n");
        assert_eq!(
            args(&read_json(&mut stream).unwrap()).unwrap(),
            ["한국 칩.oas"]
        );
        let mut tail = String::new();
        stream.read_to_string(&mut tail).unwrap();
        assert_eq!(tail, "cancel\n");
        assert!(read_json(&mut Cursor::new(vec![b' '; MAX_LINE + 2])).is_err());
        assert!(read_json(&mut Cursor::new(b"{}".as_slice())).is_err());
        assert!(read_json(&mut Cursor::new(b"\xff\n".as_slice())).is_err());
    }

    #[test]
    fn init_and_directory_are_separate_strict_requests() {
        for value in [
            json!({}),
            json!({"v":2,"args":[]}),
            json!({"v":1,"args":[],"extra":true}),
            json!({"v":1,"args":[5]}),
            json!({"v":1,"args":["a\0b"]}),
        ] {
            assert!(args(&value).is_err());
        }
        assert!(args(&json!({"v":1,"args":["a".repeat(8193)]})).is_err());
        assert!(args(&json!({"v":1,"args":vec!["a";257]})).is_err());
        assert!(directory(&json!({"v":1,"directory":"relative"})).is_err());
        assert!(directory(&json!({"v":1,"directory":"/tmp","args":[]})).is_err());
        assert_eq!(
            directory(&json!({"v":1,"directory":"/tmp/한국 폴더"})).unwrap(),
            "/tmp/한국 폴더"
        );
    }

    #[cfg(unix)]
    #[test]
    fn only_inherited_local_streams_are_eligible() {
        use std::os::fd::AsRawFd;
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(private_pipe(a.as_raw_fd()));
        let file = std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
        assert!(!private_pipe(file.as_raw_fd()));
        assert!(!private_pipe(-1));
    }
}
