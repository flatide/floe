//! `floe-index drc --pack` - full conversion of a Calibre ASCII DRC
//! results database into a self-contained .ice v2 (no locator, no
//! source dependency).
//!
//! Layout (LE; header/string sections shared with v1):
//!
//!   [header 40B]   magic | u32 version=3 | u32 flags=1 | f64
//!                  precision | u64 src_size | u64 src_mtime
//!                  (version counts LAYOUT revisions: 2 = the
//!                  short-lived pre-status/pre-file-order packs;
//!                  the reader refuses anything but the current
//!                  value - a stale pack once shifted the qbox
//!                  section by 40B and silently broke small-rect
//!                  queries)
//!   [coord blob]   BLOCK(64)-error groups in FILE ORDER, varint
//!                  records: uv((npts<<1)|kind), zz(first-point
//!                  delta from the previous error's first point),
//!                  then per point zz deltas from the previous
//!                  point. Deltas reset at block start. There is NO
//!                  per-record ordinal: the displayed number IS the
//!                  global file-order sequence (err_start + index
//!                  + 1, Calibre-RVE-style), and per-rule browser
//!                  listings come out ascending for free - which is
//!                  why records are NOT spatially reordered; the
//!                  [qbox] lattice carries the spatial query.
//!   [qbox]         4B per error: its bbox quantized to a u8
//!                  256x256 lattice over ITS CHECK's bbox (rounded
//!                  outward - always a superset). A viewport query
//!                  narrows candidates to RECORDS without touching
//!                  the varint blob.
//!   [status]       1B per error, ZERO at build: review state
//!                  (0=none, 1=waived, 2=reserved, rest app-
//!                  defined). Fixed offset per global error id, so
//!                  the viewer mutates it IN PLACE (pwrite) without
//!                  rewriting anything. A re-pack resets it.
//!   [block table]  48B: u64 blob_off | u32 count | u32 pad
//!                  | i64 bbox x0 y0 x1 y1  (dbu)
//!                  - with [qbox] this is the spatial index: numpy
//!                  scans, then decodes only blocks that hold
//!                  candidate records.
//!   [check dir]    64B: u32 name_ref, desc_start, desc_cnt, pad
//!                  | u64 err_start, err_cnt, declared, original,
//!                  block_start, block_cnt
//!   [desc refs][string table]   as v1 (line-level dedup)
//!   [footer 128B]  14 u64 (blob_off, blob_len, qbox_off,
//!                  qbox_len, status_off, blk_off, blk_cnt,
//!                  dir_off, check_cnt, descref_off, descref_cnt,
//!                  str_off, str_len, err_total) | u32 cell_ref
//!                  | u32 reserved | magic
//!
//! Parallel build: the file is split into --jobs byte segments;
//! each worker syncs onto a speculative check header (name line +
//! "int int int <timestamp>" count line), parses whole checks to a
//! raw per-worker temp file, and the coordinator stitches segments
//! by exact check-offset match - a wrong speculative sync is
//! detected (no offset match) and that segment is re-parsed
//! sequentially from the trusted boundary, so the output equals the
//! sequential parse REGARDLESS of sync quality. Output bytes are
//! --jobs invariant (D5 gate).
//!
//! Coordinates must be integers (real Calibre dbs are dbu ints); a
//! db with fractional coordinate tokens aborts with a clear message
//! (the v1 sidecar handles it).

use crate::drcice::{
    ints_prefix, is_geom_header, parse_f64, parse_i64, tokens, trim,
    Lines, StrTab, MAGIC,
};
use std::io::Write;

const BLOCK: usize = 64;

fn zz(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn uv_push(buf: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            buf.push(b);
            break;
        }
        buf.push(b | 0x80);
    }
}

fn uv_read(buf: &[u8], pos: &mut usize) -> u64 {
    let mut val = 0u64;
    let mut shift = 0;
    loop {
        let b = buf[*pos];
        *pos += 1;
        val |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return val;
        }
        shift += 7;
    }
}

fn unzz(u: u64) -> i64 {
    ((u >> 1) as i64) ^ -((u & 1) as i64)
}

struct RawCheck {
    name: Vec<u8>,
    desc: Vec<Vec<u8>>,
    declared: u64,
    original: u64,
    name_off: usize,
    err_cnt: u64,
    temp_id: usize,
    temp_off: u64,
    temp_len: u64,
}

struct SpanOut {
    checks: Vec<RawCheck>,
    end_off: usize,
}

/// Parse whole check blocks from `start` until the first check that
/// begins at/after `stop_at`. Raw records go to `tw` as absolute
/// zigzag varints. The state machine is the drcice/drc.py twin.
fn parse_span(
    data: &[u8],
    start: usize,
    stop_at: usize,
    temp_id: usize,
    tw: &mut std::io::BufWriter<std::fs::File>,
    cursor: &mut u64,
    progress: Option<&str>,
) -> Result<SpanOut, String> {
    let mut lines = Lines::new(data);
    lines.pos = start;
    let mut checks = Vec::new();
    let mut end_off = data.len();
    let mut rec: Vec<u8> = Vec::with_capacity(4096);
    let mut last_log = std::time::Instant::now();
    loop {
        let (ns, ne) = match lines.peek() {
            Some(p) => p,
            None => break,
        };
        if ns >= stop_at {
            end_off = ns;
            break;
        }
        let name = trim(&data[ns..ne]).to_vec();
        lines.consume();
        if name.is_empty() {
            continue;
        }
        let mut declared = 0u64;
        let mut original = 0u64;
        let mut desc: Vec<Vec<u8>> = Vec::new();
        if let Some((s, e)) = lines.peek() {
            let ints = ints_prefix(&tokens(&data[s..e]));
            if !ints.is_empty() {
                lines.consume();
                declared = ints[0].max(0) as u64;
                original = ints.get(1).copied().unwrap_or(0).max(0) as u64;
                let dl = ints.get(2).copied().unwrap_or(0).max(0);
                for _ in 0..dl {
                    match lines.peek() {
                        Some((ds, de))
                            if !is_geom_header(&tokens(&data[ds..de])) =>
                        {
                            desc.push(trim(&data[ds..de]).to_vec());
                            lines.consume();
                        }
                        _ => break,
                    }
                }
            }
        }
        let temp_off = *cursor;
        let mut err_cnt = 0u64;
        loop {
            let (s, e) = match lines.peek() {
                Some(p) => p,
                None => break,
            };
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
            let mut got: i64 = 0;
            let mut pts: Vec<i64> = Vec::new();
            while got < nv {
                let (cs, ce) = match lines.peek() {
                    Some(p) => p,
                    None => break,
                };
                let ct = tokens(&data[cs..ce]);
                if ct.is_empty() {
                    lines.consume();
                    continue;
                }
                let mut vals: Vec<i64> = Vec::with_capacity(ct.len());
                let mut all_i64 = true;
                let mut all_f64 = true;
                for t in &ct {
                    match parse_i64(t) {
                        Some(v) => vals.push(v),
                        None => {
                            all_i64 = false;
                            if parse_f64(t).is_none() {
                                all_f64 = false;
                            }
                            break;
                        }
                    }
                }
                if !all_f64 {
                    break; // next check name (python float() fails)
                }
                if !all_i64 {
                    if ct.len() >= 2 {
                        return Err(
                            "non-integer coordinate token: --pack \
                             stores dbu integers; use the v1 sidecar"
                                .into(),
                        );
                    }
                    break; // single odd token: python breaks too
                }
                if vals.len() < 2 {
                    break;
                }
                lines.consume();
                got += 1;
                for j in (0..vals.len() - 1).step_by(2) {
                    pts.push(vals[j]);
                    pts.push(vals[j + 1]);
                }
            }
            if !pts.is_empty() && (kind == b'p' || kind == b'e') {
                rec.clear();
                let npts = (pts.len() / 2) as u64;
                uv_push(
                    &mut rec,
                    (npts << 1) | u64::from(kind == b'e'),
                );
                for v in &pts {
                    uv_push(&mut rec, zz(*v));
                }
                tw.write_all(&rec).map_err(|e| e.to_string())?;
                *cursor += rec.len() as u64;
                err_cnt += 1;
            }
            if let Some(tag) = progress {
                if last_log.elapsed().as_secs() >= 15 {
                    last_log = std::time::Instant::now();
                    eprintln!(
                        "[drc-pack {}] {:.1}G scanned, {} checks",
                        tag,
                        (lines.pos - start) as f64 / 1e9,
                        checks.len()
                    );
                }
            }
        }
        // empty *_RDBS admin tails vanish (drc.py mirror)
        if name.ends_with(b"_RDBS") && err_cnt == 0 {
            continue;
        }
        checks.push(RawCheck {
            name,
            desc,
            declared,
            original,
            name_off: ns,
            err_cnt,
            temp_id,
            temp_off,
            temp_len: *cursor - temp_off,
        });
    }
    Ok(SpanOut { checks, end_off })
}

/// Speculative segment sync: the first line pair that looks like a
/// check header (any line followed by "int int int <non-int>...").
/// Wrong picks are corrected by the offset-match stitch.
fn find_sync(data: &[u8], from: usize) -> usize {
    let mut pos = from;
    if pos > 0 {
        // a segment boundary lands mid-line: skip the partial line
        match data[pos..].iter().position(|&b| b == b'\n') {
            Some(n) => pos += n + 1,
            None => return data.len(),
        }
    }
    let mut lines = Lines::new(data);
    lines.pos = pos;
    let mut cand: Option<usize> = None;
    while let Some((s, e)) = lines.peek() {
        let line = trim(&data[s..e]);
        lines.consume();
        if line.is_empty() {
            cand = None;
            continue;
        }
        let toks = tokens(line);
        let ints = ints_prefix(&toks);
        if cand.is_some()
            && ints.len() >= 3
            && toks.len() > ints.len()
        {
            return cand.unwrap();
        }
        cand = Some(s);
    }
    data.len()
}

fn temp_path(out: &str, id: usize) -> String {
    format!("{}.tmp{}", out, id)
}

pub fn pack(
    src: &str,
    out: &str,
    jobs: usize,
) -> Result<(usize, u64, u64), String> {
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

    // db header: "<cell> <precision>"
    let mut lines = Lines::new(data);
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
    let body = lines.pos;

    let n = jobs
        .max(1)
        .min(((data.len() - body) / (4 << 20)).max(1));
    let bound: Vec<usize> = (0..=n)
        .map(|k| body + (data.len() - body) * k / n)
        .collect();

    // phase 1: parallel speculative parse to per-worker temp files
    let mut outs: Vec<Option<(SpanOut, u64)>> =
        (0..n).map(|_| None).collect();
    let errs: Vec<String> = std::thread::scope(|sc| {
        let mut handles = Vec::new();
        for (k, slot) in outs.iter_mut().enumerate() {
            let bound = &bound;
            handles.push(sc.spawn(move || {
                let start = if k == 0 {
                    body
                } else {
                    find_sync(data, bound[k])
                };
                let tf = std::fs::File::create(temp_path(out, k))
                    .map_err(|e| e.to_string())?;
                let mut tw =
                    std::io::BufWriter::with_capacity(4 << 20, tf);
                let mut cursor = 0u64;
                let tag = format!("w{}", k);
                let so = parse_span(
                    data,
                    start.min(data.len()),
                    bound[k + 1],
                    k,
                    &mut tw,
                    &mut cursor,
                    Some(&tag),
                )?;
                tw.flush().map_err(|e| e.to_string())?;
                *slot = Some((so, cursor));
                Ok::<(), String>(())
            }));
        }
        handles
            .into_iter()
            .filter_map(|h| h.join().unwrap().err())
            .collect()
    });
    if let Some(e) = errs.into_iter().next() {
        for k in 0..n {
            let _ = std::fs::remove_file(temp_path(out, k));
        }
        return Err(e);
    }

    // stitch: trust worker 0 (anchored at the db body), then demand
    // an exact check-offset match at every seam; on mismatch (false
    // sync) re-parse that stretch sequentially from the trusted end
    let mut all: Vec<RawCheck> = Vec::new();
    let mut fb_cursor = 0u64;
    let fb_id = n; // fallback temp file id
    let mut fbw: Option<std::io::BufWriter<std::fs::File>> = None;
    let mut trusted_end;
    {
        let (so, _) = outs[0].take().unwrap();
        trusted_end = so.end_off;
        all.extend(so.checks);
    }
    for (k, o) in outs.iter_mut().enumerate().skip(1) {
        let (so, _) = o.take().unwrap();
        if so.end_off <= trusted_end {
            continue; // segment fully covered by a prior worker
        }
        match so
            .checks
            .iter()
            .position(|c| c.name_off == trusted_end)
        {
            Some(i) => {
                let mut cs = so.checks;
                all.extend(cs.drain(i..));
                trusted_end = so.end_off;
            }
            None => {
                if fbw.is_none() {
                    let tf =
                        std::fs::File::create(temp_path(out, fb_id))
                            .map_err(|e| e.to_string())?;
                    fbw = Some(std::io::BufWriter::with_capacity(
                        4 << 20,
                        tf,
                    ));
                }
                let so2 = parse_span(
                    data,
                    trusted_end,
                    bound[k + 1],
                    fb_id,
                    fbw.as_mut().unwrap(),
                    &mut fb_cursor,
                    None,
                )?;
                all.extend(so2.checks);
                trusted_end = so2.end_off;
            }
        }
    }
    if let Some(mut w) = fbw {
        w.flush().map_err(|e| e.to_string())?;
    }

    // phase 2: sequential encode of the final file
    let res = encode(
        out, &all, n, precision, src_size, src_mtime, &cell,
    );
    for k in 0..=n {
        let _ = std::fs::remove_file(temp_path(out, k));
    }
    let (_blk_cnt, err_total) = res?;
    Ok((
        all.len(),
        err_total,
        std::fs::metadata(out).map(|m| m.len()).unwrap_or(0),
    ))
}

fn encode(
    out: &str,
    checks: &[RawCheck],
    n_temps: usize,
    precision: f64,
    src_size: u64,
    src_mtime: u64,
    cell: &[u8],
) -> Result<(u64, u64), String> {
    let mut temps: Vec<Option<memmap2::Mmap>> = Vec::new();
    for k in 0..=n_temps {
        let p = temp_path(out, k);
        match std::fs::File::open(&p) {
            Ok(tf) => {
                let m = tf.metadata().map_err(|e| e.to_string())?;
                temps.push(if m.len() == 0 {
                    None
                } else {
                    Some(
                        unsafe { memmap2::Mmap::map(&tf) }
                            .map_err(|e| e.to_string())?,
                    )
                });
            }
            Err(_) => temps.push(None),
        }
    }

    let wf = std::fs::File::create(out)
        .map_err(|e| format!("create {}: {}", out, e))?;
    let mut w = std::io::BufWriter::with_capacity(8 << 20, wf);
    let mut header = Vec::with_capacity(40);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&3u32.to_le_bytes());
    header.extend_from_slice(&1u32.to_le_bytes());
    header.extend_from_slice(&precision.to_le_bytes());
    header.extend_from_slice(&src_size.to_le_bytes());
    header.extend_from_slice(&src_mtime.to_le_bytes());
    w.write_all(&header).map_err(|e| e.to_string())?;

    let mut strtab = StrTab::default();
    let cell_ref = strtab.intern(cell);
    let mut blob = 40u64;
    let mut blocks: Vec<(u64, u32, i64, i64, i64, i64)> = Vec::new();
    let mut dir: Vec<[u8; 64]> = Vec::with_capacity(checks.len());
    let mut desc_refs: Vec<u32> = Vec::new();
    let mut err_total = 0u64;
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 20);
    let mut last_log = std::time::Instant::now();
    // per-error quantized bboxes stream to a temp and land as the
    // [qbox] section right after the blob
    let qpath = format!("{}.tmpq", out);
    let mut qw = std::io::BufWriter::with_capacity(
        1 << 20,
        std::fs::File::create(&qpath).map_err(|e| e.to_string())?,
    );
    let mut ebb: Vec<(i64, i64, i64, i64)> = Vec::new();

    for (ci, c) in checks.iter().enumerate() {
        let name_ref = strtab.intern(&c.name);
        let desc_start = desc_refs.len() as u32;
        for d in &c.desc {
            desc_refs.push(strtab.intern(d));
        }
        let block_start = blocks.len() as u64;
        let err_start = err_total;
        if c.err_cnt > 0 {
            let t = temps[c.temp_id]
                .as_ref()
                .ok_or("temp file vanished")?;
            let span =
                &t[c.temp_off as usize..(c.temp_off + c.temp_len) as usize];
            // delta-encode in FILE ORDER, BLOCK per block (see the
            // module doc: no spatial reorder - global numbering and
            // ascending per-rule listings fall out of the order)
            let mut pos = 0usize;
            let mut left = c.err_cnt;
            ebb.clear();
            let mut cx0 = i64::MAX;
            let mut cy0 = i64::MAX;
            let mut cx1 = i64::MIN;
            let mut cy1 = i64::MIN;
            while left > 0 {
                let cnt = left.min(BLOCK as u64) as u32;
                let mut bx0 = i64::MAX;
                let mut by0 = i64::MAX;
                let mut bx1 = i64::MIN;
                let mut by1 = i64::MIN;
                let mut pfx = 0i64;
                let mut pfy = 0i64;
                buf.clear();
                for _ in 0..cnt {
                    let knpts = uv_read(span, &mut pos);
                    let npts = (knpts >> 1) as usize;
                    uv_push(&mut buf, knpts);
                    let mut px = 0i64;
                    let mut py = 0i64;
                    let mut ex0 = i64::MAX;
                    let mut ey0 = i64::MAX;
                    let mut ex1 = i64::MIN;
                    let mut ey1 = i64::MIN;
                    for i in 0..npts {
                        let x = unzz(uv_read(span, &mut pos));
                        let y = unzz(uv_read(span, &mut pos));
                        if i == 0 {
                            uv_push(&mut buf, zz(x - pfx));
                            uv_push(&mut buf, zz(y - pfy));
                            pfx = x;
                            pfy = y;
                        } else {
                            uv_push(&mut buf, zz(x - px));
                            uv_push(&mut buf, zz(y - py));
                        }
                        px = x;
                        py = y;
                        ex0 = ex0.min(x);
                        ey0 = ey0.min(y);
                        ex1 = ex1.max(x);
                        ey1 = ey1.max(y);
                    }
                    ebb.push((ex0, ey0, ex1, ey1));
                    bx0 = bx0.min(ex0);
                    by0 = by0.min(ey0);
                    bx1 = bx1.max(ex1);
                    by1 = by1.max(ey1);
                }
                w.write_all(&buf).map_err(|e| e.to_string())?;
                blocks.push((blob, cnt, bx0, by0, bx1, by1));
                blob += buf.len() as u64;
                left -= cnt as u64;
                cx0 = cx0.min(bx0);
                cy0 = cy0.min(by0);
                cx1 = cx1.max(bx1);
                cy1 = cy1.max(by1);
            }
            // qbox: outward-rounded u8 lattice over the check bbox
            let sx = (cx1 - cx0).max(0) as i128;
            let sy = (cy1 - cy0).max(0) as i128;
            let mut q4 = Vec::with_capacity(ebb.len() * 4);
            for &(ex0, ey0, ex1, ey1) in &ebb {
                let q = |v: i64, c0: i64, span: i128, up: bool| -> u8 {
                    if span <= 0 {
                        return if up { 255 } else { 0 };
                    }
                    let d = (v - c0) as i128 * 255;
                    let r = if up { (d + span - 1) / span } else { d / span };
                    r.clamp(0, 255) as u8
                };
                q4.push(q(ex0, cx0, sx, false));
                q4.push(q(ey0, cy0, sy, false));
                q4.push(q(ex1, cx0, sx, true));
                q4.push(q(ey1, cy0, sy, true));
            }
            qw.write_all(&q4).map_err(|e| e.to_string())?;
            err_total += c.err_cnt;
        }
        let mut rec = [0u8; 64];
        rec[..4].copy_from_slice(&name_ref.to_le_bytes());
        rec[4..8].copy_from_slice(&desc_start.to_le_bytes());
        rec[8..12].copy_from_slice(
            &(desc_refs.len() as u32 - desc_start).to_le_bytes(),
        );
        rec[16..24].copy_from_slice(&err_start.to_le_bytes());
        rec[24..32].copy_from_slice(&c.err_cnt.to_le_bytes());
        rec[32..40].copy_from_slice(&c.declared.to_le_bytes());
        rec[40..48].copy_from_slice(&c.original.to_le_bytes());
        rec[48..56].copy_from_slice(&block_start.to_le_bytes());
        rec[56..64].copy_from_slice(
            &(blocks.len() as u64 - block_start).to_le_bytes(),
        );
        dir.push(rec);
        if last_log.elapsed().as_secs() >= 15 {
            last_log = std::time::Instant::now();
            eprintln!(
                "[drc-pack enc] {}/{} checks, {} errors, {:.1}G blob",
                ci + 1,
                checks.len(),
                err_total,
                blob as f64 / 1e9
            );
        }
    }

    // [qbox] lands right after the blob
    qw.flush().map_err(|e| e.to_string())?;
    drop(qw);
    let qbox_off = blob;
    let qbox_len = err_total * 4;
    {
        let mut qr = std::fs::File::open(&qpath)
            .map_err(|e| e.to_string())?;
        std::io::copy(&mut qr, &mut w).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(&qpath);
    // [status]: one zero byte per error, mutated in place later
    let status_off = qbox_off + qbox_len;
    {
        let zeros = vec![0u8; 1 << 20];
        let mut left = err_total as usize;
        while left > 0 {
            let n = left.min(zeros.len());
            w.write_all(&zeros[..n]).map_err(|e| e.to_string())?;
            left -= n;
        }
    }
    let blk_off = status_off + err_total;
    for (off, cnt, x0, y0, x1, y1) in &blocks {
        let mut rec = [0u8; 48];
        rec[..8].copy_from_slice(&off.to_le_bytes());
        rec[8..12].copy_from_slice(&cnt.to_le_bytes());
        rec[16..24].copy_from_slice(&x0.to_le_bytes());
        rec[24..32].copy_from_slice(&y0.to_le_bytes());
        rec[32..40].copy_from_slice(&x1.to_le_bytes());
        rec[40..48].copy_from_slice(&y1.to_le_bytes());
        w.write_all(&rec).map_err(|e| e.to_string())?;
    }
    let dir_off = blk_off + blocks.len() as u64 * 48;
    for rec in &dir {
        w.write_all(rec).map_err(|e| e.to_string())?;
    }
    let descref_off = dir_off + dir.len() as u64 * 64;
    for r in &desc_refs {
        w.write_all(&r.to_le_bytes()).map_err(|e| e.to_string())?;
    }
    let str_off = descref_off + desc_refs.len() as u64 * 4;
    w.write_all(&strtab.bytes).map_err(|e| e.to_string())?;

    let mut foot = Vec::with_capacity(128);
    for v in [
        40u64,
        blob - 40,
        qbox_off,
        qbox_len,
        status_off,
        blk_off,
        blocks.len() as u64,
        dir_off,
        checks.len() as u64,
        descref_off,
        desc_refs.len() as u64,
        str_off,
        strtab.bytes.len() as u64,
        err_total,
    ] {
        foot.extend_from_slice(&v.to_le_bytes());
    }
    foot.extend_from_slice(&cell_ref.to_le_bytes());
    foot.extend_from_slice(&0u32.to_le_bytes());
    foot.extend_from_slice(MAGIC);
    w.write_all(&foot).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())?;
    Ok((blocks.len() as u64, err_total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_zigzag_roundtrip() {
        let mut buf = Vec::new();
        let vals = [0i64, 1, -1, 127, -128, 1 << 30, -(1 << 40), i64::MAX / 2];
        for v in vals {
            uv_push(&mut buf, zz(v));
        }
        let mut pos = 0;
        for v in vals {
            assert_eq!(unzz(uv_read(&buf, &mut pos)), v);
        }
        assert_eq!(pos, buf.len());
    }

    #[test]
    fn sync_finds_check_header_pattern() {
        let d = b"junk 1 2\nM1.SPACE\n3 3 1 Jul 11 01:55:00 2026\ndesc\n";
        // from 0: skips the partial first line, then M1.SPACE +
        // count line matches
        let s = find_sync(d, 1);
        assert_eq!(&d[s..s + 8], b"M1.SPACE");
    }
}
