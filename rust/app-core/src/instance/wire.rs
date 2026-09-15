//! Length-prefix framing with absolute deadlines, including slow/trickled I/O.
use super::{BODY_BYTES, POLL};
use crate::{check_cancelled, Error, ErrorKind, Result};
use serde::{de::DeserializeOwned, Serialize};
use socket2::Socket;
use std::{
    io::{self, Read, Write},
    sync::atomic::AtomicUsize,
    thread,
    time::Instant,
};
const PACKET_BYTES: usize = BODY_BYTES + 4096;
pub(super) fn encode<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>> {
    struct Bounded {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(io::Error::other("instance packet limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut out = Bounded {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut out, value)
        .map_err(|_| Error::input("instance packet limit or invalid JSON"))?;
    Ok(out.bytes)
}
fn ready(deadline: Instant, stop: &AtomicUsize) -> Result<()> {
    check_cancelled(stop)?;
    if Instant::now() >= deadline {
        return Err(Error::new(ErrorKind::Io, "instance I/O deadline"));
    }
    Ok(())
}
fn exact(socket: &Socket, bytes: &mut [u8], deadline: Instant, stop: &AtomicUsize) -> Result<()> {
    let mut at = 0;
    while at < bytes.len() {
        ready(deadline, stop)?;
        match (&*socket).read(&mut bytes[at..]) {
            Ok(0) => return Err(Error::new(ErrorKind::Io, "instance connection ended")),
            Ok(n) => at += n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
pub(super) fn read<T: DeserializeOwned>(
    socket: &Socket,
    deadline: Instant,
    stop: &AtomicUsize,
) -> Result<T> {
    let mut length = [0; 4];
    exact(socket, &mut length, deadline, stop)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > PACKET_BYTES {
        return Err(Error::input("instance packet limit"));
    }
    let mut bytes = vec![0; length];
    exact(socket, &mut bytes, deadline, stop)?;
    serde_json::from_slice(&bytes).map_err(|_| Error::input("invalid instance packet"))
}
pub(super) fn write<T: Serialize>(
    socket: &Socket,
    value: &T,
    deadline: Instant,
    stop: &AtomicUsize,
) -> Result<()> {
    let bytes = encode(value, PACKET_BYTES)?;
    for chunk in [&(bytes.len() as u32).to_be_bytes()[..], &bytes] {
        let mut at = 0;
        while at < chunk.len() {
            ready(deadline, stop)?;
            match (&*socket).write(&chunk[at..]) {
                Ok(0) => return Err(Error::new(ErrorKind::Io, "instance connection ended")),
                Ok(n) => at += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => (),
                Err(e) => return Err(e.into()),
            }
        }
    }
    Ok(())
}
