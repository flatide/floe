//! `floe-index drc` - Calibre ASCII DRC results database (.db)
//! index sidecar builder.
//!
//! The .db stays the source of truth; this writes `<db>.ice`, a
//! fixed-record index that lets the viewer mmap both files and pull
//! one violation record at a time out of a multi-hundred-GB ASCII
//! database with no load delay. Layout (all little-endian):
//!
//!   [header  40B]  magic "FLOEICE\0" | u32 version=1 | u32 flags
//!                  | f64 precision | u64 src_size | u64 src_mtime
//!   [error index]  per error 16B: u64 src_off | u32 src_len
//!                  | u8 kind (0='p', 1='e') | 3B pad
//!                  (src_off..+src_len = the record's header line
//!                   through its last coordinate line in the .db)
//!   [check dir]    per check 48B: u32 name_ref | u32 desc_start
//!                  | u32 desc_cnt | u32 pad | u64 err_start
//!                  | u64 err_cnt | u64 declared | u64 original
//!   [desc refs]    u32 string ref per description LINE - the
//!                  "Rule File Pathname:"/"Rule File Title:" lines
//!                  repeated across thousands of checks dedupe here
//!   [string table] u32 len + raw bytes per unique string; refs are
//!                  byte offsets into this section
//!   [footer  80B]  u64 err_off,err_cnt,dir_off,check_cnt,
//!                  descref_off,descref_cnt,str_off,str_len
//!                  | u32 cell_ref | u32 reserved | magic
//!
//! The line-level parse mirrors floe/drc.py EXACTLY (blank lines,
//! advisory counts, unknown record kinds, truncation tolerance) so
//! reading through the sidecar equals parsing the ASCII directly -
//! tools/validate_drc_ice.py locks that equivalence.

use std::collections::HashMap;
use std::io::Write;

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

struct CheckRec {
    name_ref: u32,
    desc_start: u32,
    desc_cnt: u32,
    err_start: u64,
    err_cnt: u64,
    declared: u64,
    original: u64,
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
    let mut pos = Vec::new();
    let mut pack = false;
    let mut jobs = 0usize;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--pack" => pack = true,
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
            "usage: floe-index drc <results.db> [out.ice] \
             [--pack] [--jobs N]"
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
    if pack {
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
                eprintln!("drc --pack {}: {}", src, e);
                let _ = std::fs::remove_file(&out);
                std::process::exit(1);
            }
        }
        return;
    }
    match build(src, &out) {
        Ok((checks, errors)) => {
            eprintln!(
                "[drc] {} -> {}: {} checks, {} errors in {:.1}s",
                src,
                out,
                checks,
                errors,
                t0.elapsed().as_secs_f64()
            );
        }
        Err(e) => {
            eprintln!("drc {}: {}", src, e);
            // never leave a half-written sidecar that a later run
            // would trust
            let _ = std::fs::remove_file(&out);
            std::process::exit(1);
        }
    }
}

fn build(src: &str, out: &str) -> Result<(usize, u64), String> {
    let f = std::fs::File::open(src)
        .map_err(|e| format!("open: {}", e))?;
    let meta = f.metadata().map_err(|e| format!("stat: {}", e))?;
    let src_size = meta.len();
    let src_mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let map;
    let empty: [u8; 0] = [];
    let data: &[u8] = if src_size == 0 {
        &empty
    } else {
        map = unsafe { memmap2::Mmap::map(&f) }
            .map_err(|e| format!("mmap: {}", e))?;
        &map
    };

    let mut lines = Lines::new(data);
    // skip blank leading lines; header = "<cell> <precision>"
    loop {
        match lines.peek() {
            None => return Err("empty file".into()),
            Some((s, e)) => {
                if trim(&data[s..e]).is_empty() {
                    lines.consume();
                } else {
                    break;
                }
            }
        }
    }
    let (hs, he) = lines.peek().unwrap();
    let head = tokens(&data[hs..he]);
    lines.consume();
    let cell: Vec<u8> = if head.is_empty() {
        std::path::Path::new(src)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned().into_bytes())
            .unwrap_or_default()
    } else {
        head[0].to_vec()
    };
    let mut precision = head
        .get(1)
        .and_then(|t| parse_f64(t))
        .unwrap_or(1000.0);
    if precision <= 0.0 {
        precision = 1000.0;
    }

    let wf = std::fs::File::create(out)
        .map_err(|e| format!("create {}: {}", out, e))?;
    let mut w = std::io::BufWriter::with_capacity(8 << 20, wf);
    let mut header = Vec::with_capacity(40);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&1u32.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&precision.to_le_bytes());
    header.extend_from_slice(&src_size.to_le_bytes());
    header.extend_from_slice(&src_mtime.to_le_bytes());
    w.write_all(&header).map_err(|e| e.to_string())?;

    let mut strtab = StrTab::default();
    let cell_ref = strtab.intern(&cell);
    let mut checks: Vec<CheckRec> = Vec::new();
    let mut desc_refs: Vec<u32> = Vec::new();
    let mut err_cnt: u64 = 0;
    let mut last_log = std::time::Instant::now();

    // one pass over the record stream; the state machine is the
    // rust twin of drc.py load_db
    while let Some((ns, ne)) = lines.peek() {
        let name = trim(&data[ns..ne]).to_vec();
        lines.consume();
        if name.is_empty() {
            continue;
        }
        let name_ref = strtab.intern(&name);
        let mut declared = 0u64;
        let mut original = 0u64;
        let desc_start = desc_refs.len() as u32;
        if let Some((s, e)) = lines.peek() {
            let ints = ints_prefix(&tokens(&data[s..e]));
            if !ints.is_empty() {
                lines.consume();
                declared = ints[0].max(0) as u64;
                original = ints.get(1).copied().unwrap_or(0).max(0) as u64;
                let dlines = ints.get(2).copied().unwrap_or(0).max(0);
                for _ in 0..dlines {
                    match lines.peek() {
                        Some((ds, de))
                            if !is_geom_header(&tokens(&data[ds..de])) =>
                        {
                            desc_refs.push(
                                strtab.intern(trim(&data[ds..de])),
                            );
                            lines.consume();
                        }
                        _ => break,
                    }
                }
            }
        }
        let desc_cnt = desc_refs.len() as u32 - desc_start;
        let err_start = err_cnt;

        // geometry records until the next check name
        while let Some((s, e)) = lines.peek() {
            let toks = tokens(&data[s..e]);
            if toks.is_empty() {
                lines.consume();
                continue;
            }
            if !is_geom_header(&toks) {
                break;
            }
            let kind = toks[0][0].to_ascii_lowercase();
            let nv = parse_i64(toks[2]).unwrap_or(0);
            lines.consume();
            let rec_start = s;
            let mut rec_end = s; // != rec_start once a coord line lands
            let mut got: i64 = 0;
            while got < nv {
                let (cs, ce) = match lines.peek() {
                    Some(p) => p,
                    None => break,
                };
                let ct = tokens(&data[cs..ce]);
                if ct.is_empty() {
                    lines.consume();
                    continue; // stray blank inside a record
                }
                let mut nums = 0usize;
                let mut ok = true;
                for t in &ct {
                    if parse_f64(t).is_none() {
                        ok = false;
                        break;
                    }
                    nums += 1;
                }
                if !ok || nums < 2 {
                    break; // next check name: record truncated here
                }
                lines.consume();
                got += 1;
                rec_end = ce;
            }
            if got > 0 && (kind == b'p' || kind == b'e') {
                let mut rec = [0u8; 16];
                rec[..8].copy_from_slice(&(rec_start as u64).to_le_bytes());
                rec[8..12].copy_from_slice(
                    &((rec_end - rec_start) as u32).to_le_bytes(),
                );
                rec[12] = if kind == b'p' { 0 } else { 1 };
                w.write_all(&rec).map_err(|e| e.to_string())?;
                err_cnt += 1;
            }
            // unknown kinds: coordinates consumed, record dropped
            if last_log.elapsed().as_secs() >= 15 {
                last_log = std::time::Instant::now();
                eprintln!(
                    "[drc] {:.1}G / {:.1}G  checks={} errors={}",
                    lines.pos as f64 / 1e9,
                    data.len() as f64 / 1e9,
                    checks.len(),
                    err_cnt
                );
            }
        }
        // administrative tail sections (*_RDBS: DENSITY_RDBS,
        // NET_AREA_RATIO_RDBS, DFM_RDBS, LAYOUT_INPUT_EXCEPTION_RDBS)
        // list rdb files, not violations: drop them - but only when
        // empty, so a real check that happens to end in _RDBS can
        // never lose its errors (drc.py load_ascii mirrors this)
        if name.ends_with(b"_RDBS") && err_cnt == err_start {
            desc_refs.truncate(desc_start as usize);
            continue;
        }
        checks.push(CheckRec {
            name_ref,
            desc_start,
            desc_cnt,
            err_start,
            err_cnt: err_cnt - err_start,
            declared,
            original,
        });
    }

    let err_off = 40u64;
    let dir_off = err_off + err_cnt * 16;
    for c in &checks {
        let mut rec = [0u8; 48];
        rec[..4].copy_from_slice(&c.name_ref.to_le_bytes());
        rec[4..8].copy_from_slice(&c.desc_start.to_le_bytes());
        rec[8..12].copy_from_slice(&c.desc_cnt.to_le_bytes());
        rec[16..24].copy_from_slice(&c.err_start.to_le_bytes());
        rec[24..32].copy_from_slice(&c.err_cnt.to_le_bytes());
        rec[32..40].copy_from_slice(&c.declared.to_le_bytes());
        rec[40..48].copy_from_slice(&c.original.to_le_bytes());
        w.write_all(&rec).map_err(|e| e.to_string())?;
    }
    let descref_off = dir_off + checks.len() as u64 * 48;
    for r in &desc_refs {
        w.write_all(&r.to_le_bytes()).map_err(|e| e.to_string())?;
    }
    let str_off = descref_off + desc_refs.len() as u64 * 4;
    w.write_all(&strtab.bytes).map_err(|e| e.to_string())?;

    let mut foot = Vec::with_capacity(80);
    for v in [
        err_off,
        err_cnt,
        dir_off,
        checks.len() as u64,
        descref_off,
        desc_refs.len() as u64,
        str_off,
        strtab.bytes.len() as u64,
    ] {
        foot.extend_from_slice(&v.to_le_bytes());
    }
    foot.extend_from_slice(&cell_ref.to_le_bytes());
    foot.extend_from_slice(&0u32.to_le_bytes());
    foot.extend_from_slice(MAGIC);
    w.write_all(&foot).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())?;
    Ok((checks.len(), err_cnt))
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
