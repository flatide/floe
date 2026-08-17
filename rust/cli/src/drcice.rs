//! `floe-index drc` - Calibre ASCII DRC results database (.db)
//! entry point + the shared line-level parser primitives.
//!
//! The output is ALWAYS the self-contained v2 pack (drcpack.rs) -
//! the original v1 offset sidecar was retired 2026-08-19 (no
//! [status]/waive storage, no spatial index, and it kept the huge
//! .db as a required companion; see docs/SPEC-FORMATS.ko.md).
//!
//! The parse helpers here mirror floe/drc.py EXACTLY (blank lines,
//! advisory counts, unknown record kinds, truncation tolerance) so
//! reading through the pack equals parsing the ASCII directly -
//! tools/validate_drc_ice.py locks that equivalence.

use std::collections::HashMap;

pub const MAGIC: &[u8; 8] = b"FLOEICE\0";

/// One parsed line: byte range of the content (CR stripped) plus the
/// range end including the terminator (for record spans).
pub(crate) struct Lines<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) pos: usize,
}

impl<'a> Lines<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Lines { data, pos: 0 }
    }
    pub(crate) fn peek(&self) -> Option<(usize, usize)> {
        if self.pos >= self.data.len() {
            return None;
        }
        let start = self.pos;
        let rest = &self.data[start..];
        let nl = rest.iter().position(|&b| b == b'\n');
        let mut end = match nl {
            Some(n) => start + n,
            None => self.data.len(),
        };
        while end > start && self.data[end - 1] == b'\r' {
            end -= 1;
        }
        Some((start, end))
    }
    pub(crate) fn consume(&mut self) {
        if let Some((start, _)) = self.peek() {
            let rest = &self.data[start..];
            self.pos = match rest.iter().position(|&b| b == b'\n') {
                Some(n) => start + n + 1,
                None => self.data.len(),
            };
        }
    }
}

pub(crate) fn tokens(line: &[u8]) -> Vec<&[u8]> {
    line.split(|b| b.is_ascii_whitespace())
        .filter(|t| !t.is_empty())
        .collect()
}

pub(crate) fn parse_i64(t: &[u8]) -> Option<i64> {
    std::str::from_utf8(t).ok()?.parse::<i64>().ok()
}

pub(crate) fn parse_f64(t: &[u8]) -> Option<f64> {
    // python float() accepts nan/inf spellings; rust f64 parse
    // matches for every token a real Calibre db contains
    std::str::from_utf8(t).ok()?.trim().parse::<f64>().ok()
}

/// drc.py _ints_prefix: leading base-10 integers of the token list.
pub(crate) fn ints_prefix(toks: &[&[u8]]) -> Vec<i64> {
    let mut out = Vec::new();
    for t in toks {
        match parse_i64(t) {
            Some(v) => out.push(v),
            None => break,
        }
    }
    out
}

/// drc.py _is_geom_header: 1-letter kind + two integer-ish tokens.
pub(crate) fn is_geom_header(toks: &[&[u8]]) -> bool {
    fn intish(t: &[u8]) -> bool {
        let s: &[u8] = if t.starts_with(b"-") {
            let n = t.iter().take_while(|&&b| b == b'-').count();
            &t[n..]
        } else {
            t
        };
        !s.is_empty() && s.iter().all(|b| b.is_ascii_digit())
    }
    toks.len() >= 3
        && toks[0].len() == 1
        && toks[0][0].is_ascii_alphabetic()
        && intish(toks[1])
        && intish(toks[2])
}

#[derive(Default)]
pub(crate) struct StrTab {
    pub(crate) bytes: Vec<u8>,
    refs: HashMap<Vec<u8>, u32>,
}

impl StrTab {
    pub(crate) fn intern(&mut self, s: &[u8]) -> u32 {
        if let Some(&r) = self.refs.get(s) {
            return r;
        }
        let off = self.bytes.len() as u32;
        self.bytes.extend_from_slice(&(s.len() as u32).to_le_bytes());
        self.bytes.extend_from_slice(s);
        self.refs.insert(s.to_vec(), off);
        off
    }
}

pub(crate) fn trim(line: &[u8]) -> &[u8] {
    let s = line.iter().position(|b| !b.is_ascii_whitespace());
    match s {
        None => b"",
        Some(s) => {
            let e = line.len()
                - line
                    .iter()
                    .rev()
                    .position(|b| !b.is_ascii_whitespace())
                    .unwrap();
            &line[s..e]
        }
    }
}

pub fn drc_cmd(args: &[String]) {
    // pack is THE format (user call 2026-08-19: the v1 offset
    // sidecar is retired - no [status]/waive storage, no spatial
    // index, and it kept the huge .db as a required companion).
    // --pack stays accepted as a no-op for scripts and docs.
    let mut pos = Vec::new();
    let mut jobs = 0usize;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--pack" => {}
            "--jobs" => {
                i += 1;
                jobs = args
                    .get(i)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            }
            a if a.starts_with("--") => {
                eprintln!("drc: unknown option {}", a);
                std::process::exit(2);
            }
            a => pos.push(a.to_string()),
        }
        i += 1;
    }
    if pos.is_empty() || pos.len() > 2 {
        eprintln!(
            "usage: floe-index drc <results.db> [out.ice] [--jobs N]"
        );
        std::process::exit(2);
    }
    let src = &pos[0];
    let out = if pos.len() == 2 {
        pos[1].clone()
    } else {
        format!("{}.ice", src)
    };
    let t0 = std::time::Instant::now();
    if jobs == 0 {
        jobs = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
    }
    match crate::drcpack::pack(src, &out, jobs) {
        Ok((checks, errors, bytes)) => {
            eprintln!(
                "[drc] {} -> {} (packed v2, jobs {}): {} checks, \
                 {} errors, {:.2}G in {:.1}s",
                src,
                out,
                jobs,
                checks,
                errors,
                bytes as f64 / 1e9,
                t0.elapsed().as_secs_f64()
            );
        }
        Err(e) => {
            eprintln!("drc {}: {}", src, e);
            // never leave a half-written pack that a later run
            // would trust
            let _ = std::fs::remove_file(&out);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geom_header_matches_python_predicate() {
        let t = |s: &str| {
            let toks = tokens(s.as_bytes());
            is_geom_header(&toks)
        };
        assert!(t("p 1 4"));
        assert!(t("e -1 --2")); // python lstrip('-') quirk kept
        assert!(!t("p 1"));
        assert!(!t("GRGEOM.1 0 3"));
        assert!(!t("12 34 56"));
        assert!(!t("p x 4"));
    }

    #[test]
    fn strtab_dedupes_repeated_lines() {
        let mut st = StrTab::default();
        let a = st.intern(b"Rule File Pathname: x.cal");
        let b = st.intern(b"other");
        let c = st.intern(b"Rule File Pathname: x.cal");
        assert_eq!(a, c);
        assert_ne!(a, b);
        // entries are (u32 len + bytes)
        assert_eq!(st.bytes.len(), 4 + 25 + 4 + 5);
    }
}
