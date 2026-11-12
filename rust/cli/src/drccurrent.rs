//! Whether a DRC pack is the results database's pack as the database stands
//! (0.12.459; user 2026-11-12, after `floe-index vfs` learned to keep a
//! current index: "floe-index drc도 .tray가 있어도 다시 생성하는 것 같음").
//! `floe2 drc --build` has always asked this before it ran the packer
//! (app-core drc::build: the pack opens, then Pack::source_current); the
//! packer itself did not - it parsed the whole database and wrote the whole
//! pack again, and only at its end kept the old file when the new bytes were
//! the same. The rule here is that one's, so that the two commands say the
//! same of a pack:
//! - the pack reads as a reader opens it - drc::Pack::open's structural
//!   checks, mirrored here (floe-index does not link app-core): the header
//!   and footer, the sections end to end, the check directory, the string
//!   references, the block table, the waived counts;
//! - the database is the file that was packed: its size and time those the
//!   pack records, or - its time alone changed (a copy, a restore, a touch) -
//!   its bytes those packed, by the CRC-32 the pack carries. A match is
//!   remembered in `<pack>.source` as app-core remembers it, so the bytes are
//!   read once.
use crate::drcice::MAGIC;
use crate::drcpack::LAYOUT;
use std::io::Read;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const HEADER: u64 = 40;
const FOOTER: u64 = 168;
/// What a reader takes of a pack's metadata at most (app-core
/// drc::META_BYTES): a pack over it opens in no reader.
const META_BYTES: u64 = 64 << 20;
/// A string a reader takes at most (app-core drc::pack `text`).
const STRING_BYTES: u64 = 1 << 20;
/// The first line of `<pack>.source` (app-core drc::pack SOURCE_HINT).
const SOURCE_HINT: &str = "floe-drc-source 1";

/// What a pack states of itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stated {
    pub src_size: u64,
    pub src_mtime: u64,
    pub src_crc: u32,
    pub checks: u64,
    pub errors: u64,
}

/// Why a path holds no pack of the database as it stands.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Not {
    /// nothing is at the path: nothing to say, the run packs
    Absent,
    /// something is there and is not the database's pack, for this reason
    Stale(String),
}

/// How the database was found to be the file that was packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Same {
    /// its size and time are those the pack records
    Stamp,
    /// its time changed; a run verified its bytes before (`<pack>.source`)
    Verified,
    /// its time changed; its bytes were read now and are those packed
    Bytes,
}

/// Ok: the pack at `out` is `src`'s as `src` stands - packing again would
/// make the same errors' pack. What it states, and how the database was
/// known.
pub(crate) fn of(src: &str, out: &str) -> Result<(Stated, Same), Not> {
    let path = Path::new(out);
    // (as the packer's own comparison at its end opens it: through a link,
    // never waiting on a FIFO)
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Not::Absent),
        Err(e) => return Err(Not::Stale(format!("it cannot be read ({e})"))),
    };
    let stated = stated(&file).map_err(Not::Stale)?;
    let same = source(src, path, &stated).map_err(Not::Stale)?;
    Ok((stated, same))
}

/// Whether another run writes the pack now (its build lock held): then
/// nothing is said of the pack here, and the run goes on to the lock, which
/// refuses it as it always has.
pub(crate) fn being_written(out: &str) -> bool {
    floe_vfs::lock::writing_refusal(&floe_vfs::lock::key(floe_vfs::lock::Kind::Pack, out)).is_some()
}

fn u32at(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}
fn u64at(b: &[u8], p: usize) -> u64 {
    u64::from_le_bytes(b[p..p + 8].try_into().unwrap())
}
fn i64at(b: &[u8], p: usize) -> i64 {
    i64::from_le_bytes(b[p..p + 8].try_into().unwrap())
}
fn bad(what: &str) -> String {
    format!("it does not validate ({what})")
}
fn mul(a: u64, b: u64) -> Result<u64, String> {
    a.checked_mul(b).ok_or_else(|| bad("section multiplication"))
}
fn add(a: u64, b: u64) -> Result<u64, String> {
    a.checked_add(b).ok_or_else(|| bad("section addition"))
}

/// A string of the table by its reference, checked as a reader reads it.
fn text(strings: &[u8], reference: u32) -> Result<(), String> {
    let p = reference as usize;
    let size = p
        .checked_add(4)
        .and_then(|end| strings.get(p..end))
        .ok_or_else(|| bad("string reference"))?;
    let n = u32at(size, 0) as usize;
    (p + 4)
        .checked_add(n)
        .and_then(|end| strings.get(p + 4..end))
        .ok_or_else(|| bad("string length"))?;
    if n as u64 > STRING_BYTES {
        return Err(bad("a string over what a reader takes"));
    }
    Ok(())
}

struct Whole<'a> {
    file: &'a std::fs::File,
    len: u64,
}
impl Whole<'_> {
    fn read(&self, off: u64, out: &mut [u8]) -> Result<(), String> {
        if off.checked_add(out.len() as u64).is_none_or(|end| end > self.len) {
            return Err(bad("read range"));
        }
        self.file.read_exact_at(out, off).map_err(|e| bad(&e.to_string()))
    }
    fn bytes(&self, off: u64, len: u64) -> Result<Vec<u8>, String> {
        let len = usize::try_from(len).map_err(|_| bad("section length"))?;
        let mut out = Vec::new();
        out.try_reserve_exact(len).map_err(|_| bad("section allocation"))?;
        out.resize(len, 0);
        self.read(off, &mut out)?;
        Ok(out)
    }
}

/// The file's stamp: a pack replaced or written to while it is read states
/// nothing (app-core Pack::unchanged).
fn stamp(m: &std::fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (m.len(), m.dev(), m.ino(), m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec())
}

/// What the pack states, when it reads whole: app-core drc::Pack::open's
/// checks in its order and with its words.
fn stated(file: &std::fs::File) -> Result<Stated, String> {
    let meta = file.metadata().map_err(|e| format!("it cannot be read ({e})"))?;
    if !meta.is_file() {
        return Err("the pack path is not a regular file".to_string());
    }
    let pack = Whole { file, len: meta.len() };
    if pack.len < HEADER + FOOTER {
        return Err(bad("truncated header/footer"));
    }
    let mut head = [0u8; HEADER as usize];
    pack.read(0, &mut head)?;
    if &head[..8] != MAGIC {
        return Err(bad("header magic"));
    }
    let layout = u32at(&head, 8);
    if layout != LAYOUT {
        return Err(format!("it is of pack layout {layout} (this packer makes {LAYOUT})"));
    }
    if u32at(&head, 12) != 1 {
        return Err(bad("header flags"));
    }
    let precision = f64::from_le_bytes(head[16..24].try_into().unwrap());
    if !precision.is_finite() || precision <= 0.0 {
        return Err(bad("precision"));
    }
    let end = pack.len - FOOTER;
    let mut foot = [0u8; FOOTER as usize];
    pack.read(end, &mut foot)?;
    if &foot[160..] != MAGIC {
        return Err(bad("footer magic"));
    }
    let f: Vec<u64> = (0..15).map(|i| u64at(&foot, i * 8)).collect();
    if f[3] != mul(f[14], 4)? {
        return Err(bad("qbox length"));
    }
    // blob, qbox, status, waived counts, block table, check directory,
    // description references, strings: end to end from the header to the
    // footer
    let sections = [
        (f[0], f[1]),
        (f[2], f[3]),
        (f[4], f[14]),
        (f[5], mul(f[9], 4)?),
        (f[6], mul(f[7], 48)?),
        (f[8], mul(f[9], 64)?),
        (f[10], mul(f[11], 4)?),
        (f[12], f[13]),
    ];
    let mut cursor = HEADER;
    for (off, len) in sections {
        if off != cursor {
            return Err(bad("non-contiguous/overlapping sections"));
        }
        cursor = add(off, len)?;
        if cursor > end {
            return Err(bad("section out of file"));
        }
    }
    if cursor != end {
        return Err(bad("body end"));
    }
    if add(add(mul(f[9], 64)?, mul(f[11], 4)?)?, f[13])? > META_BYTES {
        return Err(bad("metadata over what a reader takes"));
    }
    let dirs = pack.bytes(f[8], mul(f[9], 64)?)?;
    let refs = pack.bytes(f[10], mul(f[11], 4)?)?;
    let strings = pack.bytes(f[12], f[13])?;
    text(&strings, u32at(&foot, 120))?;
    // (errors, first block, blocks) a check
    let mut checks: Vec<(u64, u64, u64)> = Vec::new();
    checks
        .try_reserve_exact(dirs.len() / 64)
        .map_err(|_| bad("check table allocation"))?;
    let (mut es, mut bs) = (0u64, 0u64);
    for d in dirs.chunks_exact(64) {
        let (ds, dc) = (u32at(d, 4) as u64, u32at(d, 8) as u64);
        let (start, count, bstart, bcnt) = (u64at(d, 16), u64at(d, 24), u64at(d, 48), u64at(d, 56));
        if u32at(d, 12) != 0
            || start != es
            || bstart != bs
            || add(ds, dc)? > f[11]
            || bcnt != count.div_ceil(64)
        {
            return Err(bad("check spans/block count"));
        }
        es = add(es, count)?;
        bs = add(bs, bcnt)?;
        if es > f[14] || bs > f[7] {
            return Err(bad("check range"));
        }
        text(&strings, u32at(d, 0))?;
        for r in ds..ds + dc {
            text(&strings, u32at(&refs, (r * 4) as usize))?;
        }
        checks.push((count, bstart, bcnt));
    }
    if es != f[14] || bs != f[7] {
        return Err(bad("unowned errors/blocks"));
    }
    // the block table, in slabs: each block its check's, its count what the
    // check leaves it, its offset inside the blob and after the one before
    let (blob_end, blocks, block_count) = (f[2], f[6], f[7]);
    let mut next = 0usize;
    let mut previous: Option<u64> = None;
    for first in (0..block_count).step_by(4096) {
        let n = (block_count - first).min(4096);
        let data = pack.bytes(add(blocks, mul(first, 48)?)?, n * 48)?;
        for (i, b) in data.chunks_exact(48).enumerate() {
            let bi = first + i as u64;
            while next < checks.len() && bi >= checks[next].1 + checks[next].2 {
                next += 1;
            }
            let &(count_of, bstart, _) = checks.get(next).ok_or_else(|| bad("unowned block"))?;
            let off = u64at(b, 0);
            let count = u32at(b, 8) as u64;
            let bbox = [i64at(b, 16), i64at(b, 24), i64at(b, 32), i64at(b, 40)];
            let expected = (count_of - (bi - bstart) * 64).min(64);
            if count != expected
                || u32at(b, 12) != 0
                || bbox[0] > bbox[2]
                || bbox[1] > bbox[3]
                || off >= blob_end
                || previous.is_none_or(|last| off <= last) && bi != 0
                || bi == 0 && off != HEADER
            {
                return Err(bad("block metadata"));
            }
            previous = Some(off);
        }
    }
    if block_count == 0 && blob_end != HEADER {
        return Err(bad("unowned coordinate blob"));
    }
    let waived = pack.bytes(f[5], mul(f[9], 4)?)?;
    for (i, &(count, _, _)) in checks.iter().enumerate() {
        if u32at(&waived, i * 4) as u64 > count {
            return Err(bad("waived count"));
        }
    }
    let after = file.metadata().map_err(|e| format!("it cannot be read ({e})"))?;
    if stamp(&after) != stamp(&meta) {
        return Err(bad("the file changed while it was read"));
    }
    Ok(Stated {
        src_size: u64at(&head, 24),
        src_mtime: u64at(&head, 32),
        src_crc: u32at(&foot, 124),
        checks: f[9],
        errors: f[14],
    })
}

/// `<pack>.source`: the stamp of a database verified against this pack.
fn hint_path(pack: &Path) -> PathBuf {
    let mut name = pack.as_os_str().to_owned();
    name.push(".source");
    PathBuf::from(name)
}

/// (size, time s, CRC-32) of the verified database, when the file is there
/// and whole.
fn hint(pack: &Path) -> Option<(u64, u64, u32)> {
    let text = std::fs::read_to_string(hint_path(pack)).ok()?;
    let mut lines = text.lines();
    if lines.next()? != SOURCE_HINT {
        return None;
    }
    let mut fields = lines.next()?.split(' ');
    let size = fields.next()?.parse().ok()?;
    let mtime = fields.next()?.parse().ok()?;
    let crc = u32::from_str_radix(fields.next()?, 16).ok()?;
    fields.next().is_none().then_some((size, mtime, crc))
}

/// Best effort (a results folder that cannot be written keeps none).
fn write_hint(pack: &Path, stated: &Stated, mtime: u64) {
    let path = hint_path(pack);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let text = format!("{SOURCE_HINT}\n{} {mtime} {:08x}\n", stated.src_size, stated.src_crc);
    if std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .is_err()
    {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Whether `src` is the file the pack was made of (app-core
/// Pack::source_current): its size and time as packed - yes, nothing read;
/// the size as packed and another time - the stamp a run verified before,
/// else its CRC-32 read now; another size - no.
fn source(src: &str, pack: &Path, stated: &Stated) -> Result<Same, String> {
    let m = std::fs::metadata(src).map_err(|e| format!("the database cannot be looked at ({e})"))?;
    let changed = |now: String| {
        format!(
            "the database has changed since (packed at size {} mtime {}, the file now {now})",
            stated.src_size, stated.src_mtime
        )
    };
    if m.len() != stated.src_size {
        return Err(changed(format!("size {}", m.len())));
    }
    let Ok(mtime) = u64::try_from(m.mtime()) else {
        return Err(changed("of a time before 1970".to_string()));
    };
    if mtime == stated.src_mtime {
        return Ok(Same::Stamp);
    }
    if hint(pack) == Some((stated.src_size, mtime, stated.src_crc)) {
        return Ok(Same::Verified);
    }
    let read = |e: std::io::Error| format!("the database cannot be read ({e})");
    let mut file = std::fs::File::open(src).map_err(read)?;
    let before = file.metadata().map_err(read)?;
    if !before.is_file() || before.len() != stated.src_size {
        return Err(changed(format!("size {}", before.len())));
    }
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = vec![0u8; 4 << 20];
    let mut left = stated.src_size;
    while left > 0 {
        let n = file.read(&mut buffer).map_err(read)?;
        if n == 0 {
            return Err(changed("shorter than it was".to_string()));
        }
        hasher.update(&buffer[..n]);
        left = left.saturating_sub(n as u64);
    }
    let crc = hasher.finalize();
    let after = std::fs::metadata(src).map_err(read)?;
    if file.read(&mut buffer[..1]).map_err(read)? != 0
        || after.len() != before.len()
        || after.mtime() != before.mtime()
    {
        return Err(changed("being written".to_string()));
    }
    if crc != stated.src_crc {
        return Err(format!(
            "the database has changed since (the same size {}, other bytes: CRC-32 {crc:08x}, packed {:08x})",
            stated.src_size, stated.src_crc
        ));
    }
    if let Ok(mtime) = u64::try_from(after.mtime()) {
        if mtime != stated.src_mtime {
            write_hint(pack, stated, mtime);
        }
    }
    Ok(Same::Bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("floe-drccurrent-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self, name: &str) -> String {
            self.0.join(name).to_str().unwrap().to_string()
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const DB: &str = "TOP 1000\nM1.S.1\n3 3 2 Nov 12 09:00:00 2026\nRule File Pathname: deck.svrf\nSpacing < 0.1\np 1 4\n0 0\n10 0\n10 10\n0 10\ne 2 2\n0 0 5 5\n7 7 9 9\ne 3 1\n-4 -4 -2 -2\nM2.W.1\n0 0 1 Nov 12 09:00:00 2026\nWidth < 0.2\nV1.EN.1\n1 1 1 Nov 12 09:00:00 2026\nEnclosure < 0.03\np 1 3\n100 100\n200 100\n150 180\n";

    fn packed(s: &Scratch) -> (String, String) {
        let (db, out) = (s.path("r.db"), s.path(".r.db.tray"));
        std::fs::write(&db, DB).unwrap();
        crate::drcpack::pack(&db, &out, 2).unwrap();
        (db, out)
    }

    fn mtime_of(path: &str) -> u64 {
        std::fs::metadata(path).unwrap().mtime() as u64
    }

    fn set_mtime(path: &str, secs: u64) {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn a_pack_of_the_database_as_it_stands_is_current() {
        let s = Scratch::new("current");
        // nothing there says nothing
        assert_eq!(of(&s.path("r.db"), &s.path(".r.db.tray")), Err(Not::Absent));
        let (db, out) = packed(&s);
        let (stated, same) = of(&db, &out).unwrap();
        assert_eq!(same, Same::Stamp);
        assert_eq!((stated.checks, stated.errors), (3, 4));
        assert_eq!(stated.src_size, DB.len() as u64);
        assert_eq!(stated.src_mtime, mtime_of(&db));
        assert_eq!(stated.src_crc, crc32fast::hash(DB.as_bytes()));
        assert!(!Path::new(&format!("{out}.source")).exists());
        // through a link to the pack, as the packer's own comparison reads it
        let link = s.path("link.tray");
        std::os::unix::fs::symlink(&out, &link).unwrap();
        assert_eq!(of(&db, &link).map(|r| r.1), Ok(Same::Stamp));
        // a link to nothing is nothing
        let dangling = s.path("dangling.tray");
        std::os::unix::fs::symlink(s.path("absent"), &dangling).unwrap();
        assert_eq!(of(&db, &dangling), Err(Not::Absent));
        // a folder at the path
        std::fs::create_dir(s.path("dir.tray")).unwrap();
        assert!(matches!(of(&db, &s.path("dir.tray")), Err(Not::Stale(_))));
    }

    #[test]
    fn a_database_whose_time_alone_changed_is_read_once() {
        let s = Scratch::new("touched");
        let (db, out) = packed(&s);
        let packed_at = mtime_of(&db);
        // another time, the same bytes: read, and remembered beside the pack
        set_mtime(&db, packed_at + 100);
        assert_eq!(of(&db, &out).map(|r| r.1), Ok(Same::Bytes));
        let hint_file = format!("{out}.source");
        assert_eq!(
            std::fs::read_to_string(&hint_file).unwrap(),
            format!("floe-drc-source 1\n{} {} {:08x}\n", DB.len(), packed_at + 100, crc32fast::hash(DB.as_bytes()))
        );
        assert_eq!(of(&db, &out).map(|r| r.1), Ok(Same::Verified));
        // yet another time: the stamp is of the other one, the bytes are read again
        set_mtime(&db, packed_at + 200);
        assert_eq!(of(&db, &out).map(|r| r.1), Ok(Same::Bytes));
        assert_eq!(of(&db, &out).map(|r| r.1), Ok(Same::Verified));
        // back at the time it was packed at: nothing read, whatever the stamp says
        set_mtime(&db, packed_at);
        assert_eq!(of(&db, &out).map(|r| r.1), Ok(Same::Stamp));
        // a stamp that is not one is passed over
        set_mtime(&db, packed_at + 300);
        for junk in ["", "floe-drc-source 2\n1 2 3\n", "floe-drc-source 1\n", "floe-drc-source 1\nx y z\n"] {
            std::fs::write(&hint_file, junk).unwrap();
            assert_eq!(of(&db, &out).map(|r| r.1), Ok(Same::Bytes), "{junk:?}");
        }
        // other bytes of the same size at another time: not the file packed
        let other = DB.replace("7 7 9 9", "7 7 9 8");
        assert_eq!(other.len(), DB.len());
        std::fs::write(&db, &other).unwrap();
        set_mtime(&db, packed_at + 400);
        assert!(matches!(of(&db, &out), Err(Not::Stale(why)) if why.contains("other bytes")));
        // (at the time it was packed at they pass, as for every reader: the
        // size and the time are the question asked first)
        set_mtime(&db, packed_at);
        assert_eq!(of(&db, &out).map(|r| r.1), Ok(Same::Stamp));
        // another size
        std::fs::write(&db, format!("{DB}\n")).unwrap();
        assert!(matches!(of(&db, &out), Err(Not::Stale(why)) if why.contains("has changed since")));
        // no database
        std::fs::remove_file(&db).unwrap();
        assert!(matches!(of(&db, &out), Err(Not::Stale(why)) if why.contains("cannot be looked at")));
    }

    #[test]
    fn a_pack_that_does_not_read_whole_is_not_current() {
        let s = Scratch::new("damaged");
        let (db, out) = packed(&s);
        let good = std::fs::read(&out).unwrap();
        let len = good.len();
        let foot = len - FOOTER as usize;
        let f = |i: usize| u64at(&good[foot..], i * 8) as usize;
        let damaged = |name: &str, change: &dyn Fn(&mut Vec<u8>)| -> String {
            let mut bytes = good.clone();
            change(&mut bytes);
            let path = s.path(name);
            std::fs::write(&path, bytes).unwrap();
            match of(&db, &path) {
                Err(Not::Stale(why)) => why,
                other => panic!("{name}: {other:?}"),
            }
        };
        // the whole pack passes under another name too
        std::fs::write(s.path("copy.tray"), &good).unwrap();
        assert_eq!(of(&db, &s.path("copy.tray")).map(|r| r.1), Ok(Same::Stamp));
        for (name, want, change) in [
            ("empty", "truncated header/footer", (&|b: &mut Vec<u8>| b.clear()) as &dyn Fn(&mut Vec<u8>)),
            ("cut", "footer magic", &|b: &mut Vec<u8>| b.truncate(b.len() - 9)),
            ("longer", "footer magic", &|b: &mut Vec<u8>| b.push(0)),
            ("head-magic", "header magic", &|b: &mut Vec<u8>| b[0] ^= 1),
            ("layout", "pack layout 4", &|b: &mut Vec<u8>| b[8..12].copy_from_slice(&4u32.to_le_bytes())),
            ("flags", "header flags", &|b: &mut Vec<u8>| b[12] = 2),
            ("precision", "precision", &|b: &mut Vec<u8>| b[16..24].copy_from_slice(&0f64.to_le_bytes())),
            ("foot-magic", "footer magic", &|b: &mut Vec<u8>| {
                let n = b.len();
                b[n - 1] ^= 1
            }),
            ("qbox", "qbox length", &|b: &mut Vec<u8>| b[foot + 3 * 8] ^= 1),
            ("sections", "non-contiguous", &|b: &mut Vec<u8>| b[foot] ^= 1),
            ("total", "qbox length", &|b: &mut Vec<u8>| b[foot + 14 * 8] ^= 1),
            ("cell", "string", &|b: &mut Vec<u8>| b[foot + 120..foot + 124].copy_from_slice(&u32::MAX.to_le_bytes())),
            ("dir-pad", "check spans", &|b: &mut Vec<u8>| b[f(8) + 12] = 1),
            ("dir-count", "check spans", &|b: &mut Vec<u8>| b[f(8) + 24] ^= 1),
            ("dir-name", "string", &|b: &mut Vec<u8>| b[f(8)..f(8) + 4].copy_from_slice(&u32::MAX.to_le_bytes())),
            ("block-count", "block metadata", &|b: &mut Vec<u8>| b[f(6) + 8] ^= 1),
            ("block-offset", "block metadata", &|b: &mut Vec<u8>| b[f(6)] ^= 1),
            ("block-bbox", "block metadata", &|b: &mut Vec<u8>| {
                let at = f(6) + 16;
                b[at..at + 8].copy_from_slice(&i64::MAX.to_le_bytes())
            }),
            ("waived", "waived count", &|b: &mut Vec<u8>| b[f(5)..f(5) + 4].copy_from_slice(&9u32.to_le_bytes())),
        ] {
            let why = damaged(&format!("{name}.tray"), change);
            assert!(why.contains(want), "{name}: {why}");
        }
        // what a pack does not state a reader does not check either: a byte
        // of the coordinate blob, of the quantized boxes, of a status
        for (name, at) in [("blob", HEADER as usize), ("qbox-byte", f(2)), ("status", f(4))] {
            let mut bytes = good.clone();
            bytes[at] ^= 1;
            let path = s.path(&format!("{name}.tray"));
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(of(&db, &path).map(|r| r.1), Ok(Same::Stamp), "{name}");
        }
    }
}
