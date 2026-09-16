//! Header-only source catalog. Unselected sources contribute DBU, not cache
//! mappings or geometry. GDS/gzip are recognized but remain non-indexable.
use crate::{cache, check_cancelled, Error, Result};
use floe_oasis::header::probe_start;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

const PROBE_BYTES: usize = 4096;
const GDS_HEADER_CAP: usize = 64 * 1024;
const GDS_MAGIC: &[u8] = &[0, 6, 0, 2];
const OASIS_MAGIC: &[u8] = b"%SEMI-OASIS\r\n";

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Header {
    pub format: Option<String>,
    pub dbu: Option<f64>,
    pub version: String,
    pub gzipped: bool,
}

/// Reads at most one OASIS header page or a bounded GDS library header. Even
/// gzip input/output is bounded so a long gzip filename/header cannot hang it.
pub fn file_header(path: &Path, cancelled: &AtomicUsize) -> Result<Header> {
    check_cancelled(cancelled)?;
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !f.metadata()?.is_file() {
        return Err(Error::input("source is not a regular file"));
    }
    let mut first = [0u8; 2];
    let mut n = 0;
    while n < first.len() {
        let count = f.read(&mut first[n..])?;
        if count == 0 {
            break;
        }
        n += count;
    }
    let gzipped = first == [0x1f, 0x8b];
    let stream = std::io::Cursor::new(&first[..n]).chain(f);
    let mut reader: Box<dyn Read + '_> = if gzipped {
        Box::new(flate2::read::GzDecoder::new(stream.take(1024 * 1024)))
    } else {
        Box::new(stream)
    };
    let mut head = Vec::new();
    reader
        .by_ref()
        .take(PROBE_BYTES as u64)
        .read_to_end(&mut head)?;
    check_cancelled(cancelled)?;
    let mut header = Header {
        format: None,
        dbu: None,
        version: String::new(),
        gzipped,
    };
    if head.starts_with(OASIS_MAGIC) {
        let start = probe_start(&head).map_err(|e| Error::input(e.to_string()))?;
        header.format = Some("oasis".into());
        header.dbu = Some(start.dbu);
        header.version = start.version;
    } else if head.starts_with(GDS_MAGIC) {
        reader.take(GDS_HEADER_CAP as u64).read_to_end(&mut head)?;
        check_cancelled(cancelled)?;
        let (dbu, version) = gds_units(&head)?;
        header.format = Some("gds".into());
        header.dbu = Some(dbu);
        header.version = version;
    }
    Ok(header)
}
fn gds_units(bytes: &[u8]) -> Result<(f64, String)> {
    let version = i16::from_be_bytes(
        bytes
            .get(4..6)
            .ok_or_else(|| Error::input("truncated GDS HEADER"))?
            .try_into()
            .expect("two bytes"),
    )
    .to_string();
    let mut at = 6usize;
    loop {
        let head = bytes
            .get(at..at + 4)
            .ok_or_else(|| Error::input("GDS library header ends before UNITS"))?;
        let length = u16::from_be_bytes([head[0], head[1]]) as usize;
        if length < 4 {
            return Err(Error::input("GDS record length is less than 4"));
        }
        let end = at
            .checked_add(length)
            .filter(|&v| v <= GDS_HEADER_CAP)
            .ok_or_else(|| Error::input("no GDS UNITS within 65536 bytes"))?;
        let body = bytes
            .get(at + 4..end)
            .ok_or_else(|| Error::input("truncated GDS library record"))?;
        match head[2] {
            3 => {
                if body.len() != 16 {
                    return Err(Error::input("GDS UNITS must contain 16 bytes"));
                }
                let b = &body[8..16];
                let sign = if b[0] & 0x80 == 0 { 1. } else { -1. };
                let exponent = i32::from(b[0] & 0x7f) - 64;
                let mantissa = b[1..].iter().fold(0u64, |v, &b| (v << 8) | u64::from(b));
                let dbu = sign * (mantissa as f64 / 2f64.powi(56)) * 16f64.powi(exponent) * 1e6;
                if dbu <= 0. || !dbu.is_finite() || !(1. / dbu).is_finite() {
                    return Err(Error::input("invalid GDS database unit"));
                }
                return Ok((dbu, version));
            }
            4 | 5 => {
                return Err(Error::input(
                    "GDS library has no UNITS before its first structure",
                ))
            }
            _ => (),
        }
        at = end;
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceInfo {
    pub tc: String,
    pub path: PathBuf,
    pub status: String,
    pub dbu: Option<f64>,
    pub version: String,
    pub format: String,
    pub gzipped: bool,
    pub bytes: u64,
    pub probe: String,
    pub probe_s: f64,
    pub error: String,
    pub indexed: bool,
    pub cache_dir: PathBuf,
}
impl SourceInfo {
    pub fn ok(&self) -> bool {
        self.status == "ok" && self.dbu.is_some()
    }
}
#[derive(Debug)]
pub struct SourceCatalog {
    pub directory: PathBuf,
    pub infos: BTreeMap<String, SourceInfo>,
}
impl SourceCatalog {
    pub fn new(directory: &Path) -> Result<Self> {
        Ok(Self {
            directory: cache::absolute(directory)?,
            infos: BTreeMap::new(),
        })
    }
    pub fn resolve(&self, tc: &str) -> PathBuf {
        self.directory.join(tc)
    }
    pub fn probe(&mut self, tc: &str, cancelled: &AtomicUsize) -> Result<&SourceInfo> {
        check_cancelled(cancelled)?;
        if !self.infos.contains_key(tc) {
            let path = self.resolve(tc);
            let start = Instant::now();
            let mut info = SourceInfo {
                tc: tc.into(),
                path: path.clone(),
                status: "ok".into(),
                dbu: None,
                version: String::new(),
                format: String::new(),
                gzipped: false,
                bytes: 0,
                probe: "not-probed".into(),
                probe_s: 0.,
                error: String::new(),
                indexed: false,
                cache_dir: PathBuf::new(),
            };
            match std::fs::metadata(&path) {
                Ok(m) if m.is_file() => {
                    info.bytes = m.len();
                    match file_header(&path, cancelled) {
                        Ok(h) => {
                            info.gzipped = h.gzipped;
                            if let Some(format) = h.format {
                                info.probe = if format == "oasis" {
                                    "oasis-start-record"
                                } else {
                                    "gds-units-record"
                                }
                                .into();
                                if format != "oasis" || h.gzipped {
                                    info.status = "unsupported".into();
                                    info.error = format!(
                                        "floe-index reads plain OASIS only ({format}{})",
                                        if h.gzipped { ", gzip" } else { "" }
                                    );
                                }
                                info.format = format;
                                info.dbu = h.dbu;
                                info.version = h.version;
                            } else {
                                info.status = "unknown_format".into();
                                info.error = format!(
                                    "no OASIS or GDS header{}",
                                    if h.gzipped { " (gzip)" } else { "" }
                                );
                            }
                        }
                        Err(e) if e.kind == crate::ErrorKind::Cancelled => return Err(e),
                        Err(e) => {
                            info.status = "unreadable".into();
                            info.error = e.to_string();
                        }
                    }
                    info.cache_dir = cache::cache_path(&path)?;
                    info.indexed = matches!(
                        cache::inspect(&path, &info.cache_dir),
                        Ok(cache::CacheState::Current)
                    );
                }
                Ok(_) => {
                    info.status = "missing".into();
                    info.error = "file not found".into();
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    info.status = "missing".into();
                    info.error = "file not found".into();
                }
                Err(e) => {
                    info.status = "unreadable".into();
                    info.error = e.to_string();
                }
            }
            info.probe_s = start.elapsed().as_secs_f64();
            self.infos.insert(tc.into(), info);
        }
        check_cancelled(cancelled)?;
        Ok(&self.infos[tc])
    }
    pub fn probe_all<'a>(
        &mut self,
        tcs: impl IntoIterator<Item = &'a str>,
        cancelled: &AtomicUsize,
    ) -> Result<()> {
        for tc in tcs {
            self.probe(tc, cancelled)?;
        }
        Ok(())
    }
    pub fn header_dbu(&self, tc: &str, cancelled: &AtomicUsize) -> Result<Option<f64>> {
        check_cancelled(cancelled)?;
        if let Some(info) = self.infos.get(tc) {
            return Ok(info.ok().then_some(info.dbu).flatten());
        }
        match file_header(&self.resolve(tc), cancelled) {
            Ok(h) if h.format.as_deref() == Some("oasis") && !h.gzipped => Ok(h.dbu),
            Err(e) if e.kind == crate::ErrorKind::Cancelled => Err(e),
            _ => Ok(None),
        }
    }
    pub fn dbus(&self) -> BTreeMap<String, f64> {
        self.infos
            .iter()
            .filter(|(_, i)| i.ok())
            .map(|(tc, i)| (tc.clone(), i.dbu.expect("ok DBU")))
            .collect()
    }
    pub fn bad(&self) -> BTreeMap<String, super::geom::SourceIssue> {
        self.infos
            .iter()
            .filter(|(_, i)| !i.ok())
            .map(|(tc, i)| {
                (
                    tc.clone(),
                    super::geom::SourceIssue {
                        reason: i.status.clone(),
                        detail: i.error.clone(),
                        stage: "probe".into(),
                    },
                )
            })
            .collect()
    }
    pub fn report(&self) -> Value {
        let infos: Vec<_> = self.infos.values().collect();
        let seconds: f64 = infos.iter().map(|i| i.probe_s).sum();
        json!({"dir":self.directory,"probed":infos.len(),"ok":infos.iter().filter(|i|i.ok()).count(),
            "indexed":infos.iter().filter(|i|i.indexed).count(),"probe_s":(seconds*10000.).round_ties_even()/10000.,
            "note":"dbu comes from the OASIS START / GDS UNITS record in the first bytes of each file; no geometry is read. indexed = a fresh VFS cache exists.",
            "files":infos})
    }
}
