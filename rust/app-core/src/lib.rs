//! Application policy, independent of HTTP, GTK and Python. The first slice
//! implements local layout indexing; server admission/leases remain separate.
#![deny(unsafe_op_in_unsafe_fn)]
#[cfg(not(unix))]
compile_error!("floe-app-core currently targets Linux/macOS");

pub mod cache;
pub mod index;
pub mod native;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidInput,
    Unsupported,
    Io,
    Cache,
    Busy,
    Version,
    Worker,
    Cancelled,
}

#[derive(Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}
impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    pub fn input(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::new(ErrorKind::Io, e.to_string())
    }
}
pub type Result<T> = std::result::Result<T, Error>;
