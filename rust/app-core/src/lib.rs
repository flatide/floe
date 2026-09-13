//! Application policy, independent of HTTP, GTK and Python. The first slice
//! implements local layout indexing; server admission/leases remain separate.
#![deny(unsafe_op_in_unsafe_fn)]
#[cfg(not(unix))]
compile_error!("floe-app-core currently targets Linux/macOS");

pub mod artifact;
pub mod browser;
pub mod cache;
pub mod catalog;
pub mod clip;
pub mod dataset;
pub mod drc;
pub mod index;
pub mod index_progress;
pub mod jobdeck;
pub mod managed;
pub mod managed_index;
pub mod native;
pub mod registered;
pub mod render;
pub mod shots;
pub mod styles;
pub mod svrf;
pub mod view;

/// Legacy `%g` diagnostics (six significant digits), not coordinate wire
/// serialization. Scientific rounding decides the fixed/scientific switch.
pub fn format_general(value: f64) -> String {
    let scientific = format!("{value:.5e}");
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return scientific;
    };
    let exponent: i32 = exponent.parse().expect("formatted exponent");
    if !(-4..6).contains(&exponent) {
        format!(
            "{}e{exponent:+03}",
            mantissa.trim_end_matches('0').trim_end_matches('.')
        )
    } else {
        let precision = (5 - exponent) as usize;
        let fixed = format!("{value:.precision$}");
        if precision == 0 {
            fixed
        } else {
            fixed.trim_end_matches('0').trim_end_matches('.').into()
        }
    }
}

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
    Incomplete,
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
impl From<floe_worker_client::Error> for Error {
    fn from(e: floe_worker_client::Error) -> Self {
        use floe_worker_client::ErrorKind as W;
        let kind = match e.kind {
            W::InvalidInput => ErrorKind::InvalidInput,
            W::Cancelled => ErrorKind::Cancelled,
            W::Busy => ErrorKind::Busy,
            W::Version => ErrorKind::Version,
            _ => ErrorKind::Worker,
        };
        Self::new(kind, e.message)
    }
}
pub fn check_cancelled(flag: &std::sync::atomic::AtomicUsize) -> Result<()> {
    if flag.load(std::sync::atomic::Ordering::Relaxed) != 0 {
        Err(Error::new(ErrorKind::Cancelled, "operation cancelled"))
    } else {
        Ok(())
    }
}
pub type Result<T> = std::result::Result<T, Error>;
