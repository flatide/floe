//! PNG envelopes, immutable display snapshots and streaming iTXt replacement.
use super::{Document, MAX_TEXT};
use crate::{
    artifact::{self, StagedArtifact},
    catalog::regular_file,
    check_cancelled, Error, Result,
};
use std::{
    fs::{self, File, Metadata, Permissions},
    io::{Read, Seek, SeekFrom, Write},
    ops::Range,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::AtomicUsize,
};

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const PREFIX: &[u8; 9] = b"flateyes\0";
const BODY_HEAD: &[u8; 13] = b"flateyes\0\0\0\0\0";
const MAX_PNG: u64 = 1024 * 1024 * 1024;
const MAX_CHUNKS: usize = 65536;
const BLOCK: usize = 1024 * 1024;
pub const MAX_DISPLAY_PNG: usize = 80 * 1024 * 1024;

struct Scan {
    width: u32,
    height: u32,
    length: u64,
    chunks: usize,
    iend: u64,
    owned: Vec<Range<u64>>,
    animation: Vec<Range<u64>>,
    text: Option<String>,
}
fn read_exact(input: &mut impl Read, b: &mut [u8], flag: &AtomicUsize) -> Result<()> {
    let mut offset = 0;
    while offset < b.len() {
        check_cancelled(flag)?;
        match input.read(&mut b[offset..]) {
            Ok(0) => return Err(Error::input("truncated PNG")),
            Ok(n) => offset += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn decode_text(body: &[u8], allowance: usize, flag: &AtomicUsize) -> Result<String> {
    if body.len() < 13 || body[9] > 1 || body[10] != 0 {
        return Err(Error::input("malformed flateyes iTXt header"));
    }
    let mut rest = &body[11..];
    for _ in 0..2 {
        let i = rest
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| Error::input("unterminated iTXt language/translated keyword"))?;
        rest = &rest[i + 1..];
    }
    let decoded = if body[9] == 1 {
        // Read::read()==0 also represents exhausted *truncated* zlib input.
        // Require the decoder's explicit stream end, including Adler trailer.
        let mut z = flate2::Decompress::new(true);
        let mut decoded = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            check_cancelled(flag)?;
            let (before_in, before_out) = (z.total_in(), z.total_out());
            let remaining = &rest[before_in as usize..];
            let flush = if remaining.is_empty() {
                flate2::FlushDecompress::Finish
            } else {
                flate2::FlushDecompress::None
            };
            let status = z
                .decompress(remaining, &mut buffer, flush)
                .map_err(|_| Error::input("corrupt compressed flateyes metadata"))?;
            let n = (z.total_out() - before_out) as usize;
            if decoded.len() + n > allowance {
                return Err(Error::input(
                    "PNG annotation text exceeds 16 MiB (including duplicate chunks)",
                ));
            }
            decoded.extend_from_slice(&buffer[..n]);
            if status == flate2::Status::StreamEnd {
                break;
            }
            if n == 0 && z.total_in() == before_in {
                return Err(Error::input("truncated compressed flateyes metadata"));
            }
        }
        decoded
    } else {
        if rest.len() > allowance {
            return Err(Error::input("PNG annotation text exceeds 16 MiB"));
        }
        rest.to_vec()
    };
    String::from_utf8(decoded).map_err(|_| Error::input("flateyes metadata must be UTF-8"))
}
fn scan(input: &mut (impl Read + Seek), flag: &AtomicUsize) -> Result<Scan> {
    scan_options(input, flag, true)
}
fn scan_options(
    input: &mut (impl Read + Seek),
    flag: &AtomicUsize,
    annotations: bool,
) -> Result<Scan> {
    let length = input.seek(SeekFrom::End(0))?;
    if length > MAX_PNG {
        return Err(Error::input("PNG exceeds 1 GiB"));
    }
    input.seek(SeekFrom::Start(0))?;
    let mut signature = [0; 8];
    read_exact(input, &mut signature, flag)?;
    if &signature != SIGNATURE {
        return Err(Error::input("not a PNG"));
    }
    let mut scratch = vec![0; BLOCK];
    let mut pos = 8u64;
    let mut text = None;
    let mut owned = Vec::new();
    let mut animation = Vec::new();
    let mut decoded_bytes = 0usize;
    let mut idat = false;
    let (mut width, mut height) = (0, 0);
    for count in 0..MAX_CHUNKS {
        let mut header = [0; 8];
        read_exact(input, &mut header, flag)?;
        let len = u32::from_be_bytes(header[..4].try_into().unwrap()) as u64;
        let kind: &[u8; 4] = header[4..].try_into().unwrap();
        let end = pos
            .checked_add(len + 12)
            .filter(|end| *end <= length)
            .ok_or_else(|| Error::input("truncated PNG chunk"))?;
        if !kind.iter().all(u8::is_ascii_alphabetic)
            || (count == 0 && (kind != b"IHDR" || len != 13))
            || (count != 0 && kind == b"IHDR")
        {
            return Err(Error::input("invalid PNG header/chunk type"));
        }
        if kind == b"IEND" && (len != 0 || !idat) {
            return Err(Error::input("PNG requires IDAT and empty IEND"));
        }
        let mut hash = crc32fast::Hasher::new();
        hash.update(kind);
        let mut left = len;
        let mut body = None;
        if annotations && kind == b"iTXt" && len >= PREFIX.len() as u64 {
            let mut prefix = [0; 9];
            read_exact(input, &mut prefix, flag)?;
            hash.update(&prefix);
            left -= 9;
            if &prefix == PREFIX {
                if len > (MAX_TEXT + 4096) as u64 {
                    return Err(Error::input("encoded flateyes metadata exceeds 16 MiB"));
                }
                let mut b = Vec::with_capacity(len as usize);
                b.extend_from_slice(&prefix);
                body = Some(b);
            }
        }
        while left != 0 {
            let n = left.min(BLOCK as u64) as usize;
            read_exact(input, &mut scratch[..n], flag)?;
            hash.update(&scratch[..n]);
            if kind == b"IHDR" {
                width = u32::from_be_bytes(scratch[..4].try_into().unwrap());
                height = u32::from_be_bytes(scratch[4..8].try_into().unwrap());
                let valid_depth = match scratch[9] {
                    0 => matches!(scratch[8], 1 | 2 | 4 | 8 | 16),
                    2 | 4 | 6 => matches!(scratch[8], 8 | 16),
                    3 => matches!(scratch[8], 1 | 2 | 4 | 8),
                    _ => false,
                };
                if width == 0
                    || height == 0
                    || !valid_depth
                    || scratch[10] != 0
                    || scratch[11] != 0
                    || scratch[12] > 1
                {
                    return Err(Error::input("invalid PNG IHDR"));
                }
                if !annotations
                    && (width > 8192
                        || height > 8192
                        || u64::from(width) * u64::from(height) > 16 * 1024 * 1024)
                {
                    return Err(Error::input("display PNG exceeds 8192 px/axis or 16 Mpx"));
                }
            }
            if let Some(b) = &mut body {
                b.extend_from_slice(&scratch[..n]);
            }
            left -= n as u64;
        }
        let mut crc = [0; 4];
        read_exact(input, &mut crc, flag)?;
        if u32::from_be_bytes(crc) != hash.finalize() {
            return Err(Error::input("PNG chunk CRC mismatch"));
        }
        if !annotations && matches!(kind, b"acTL" | b"fcTL" | b"fdAT") {
            // Static-only display ignores animation semantics, not source
            // integrity: every removed chunk still passes length/CRC limits.
            animation.push(pos..end);
        }
        if let Some(body) = body {
            let decoded = decode_text(&body, MAX_TEXT - decoded_bytes, flag)?;
            decoded_bytes += decoded.len();
            if text.is_none() {
                text = Some(decoded);
            }
            owned.push(pos..end);
        }
        if kind == b"IEND" {
            if !annotations && end != length {
                return Err(Error::input("display PNG has trailing data after IEND"));
            }
            return Ok(Scan {
                width,
                height,
                length,
                chunks: count + 1,
                iend: pos,
                owned,
                animation,
                text,
            });
        }
        idat |= kind == b"IDAT";
        pos = end;
    }
    Err(Error::input("PNG exceeds 65536 chunks or has no IEND"))
}

/// Validated envelope only; the browser checks/decodes IDAT image samples. This
/// type is not deserializable and cannot be constructed with an HTTP path.
pub struct DisplayPng {
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}
impl DisplayPng {
    pub fn into_parts(self) -> (u32, u32, Vec<u8>) {
        (self.width, self.height, self.bytes)
    }
}
/// Read the explicitly selected regular file once. No file rewrite, annotation
/// interpretation, later lookup or background reload is performed. APNG chunks
/// are removed only from the validated in-memory display snapshot.
pub fn display_snapshot(path: &Path, flag: &AtomicUsize) -> Result<DisplayPng> {
    check_cancelled(flag)?;
    let path = fs::canonicalize(path)?;
    let mut file = regular_file(&path)?;
    let before = Stamp::from(file.metadata()?);
    let length = usize::try_from(before.len)
        .ok()
        .filter(|n| *n <= MAX_DISPLAY_PNG)
        .ok_or_else(|| Error::input("display PNG exceeds 80 MiB"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| Error::input("display PNG allocation failed"))?;
    bytes.resize(length, 0);
    for block in bytes.chunks_mut(BLOCK) {
        read_exact(&mut file, block, flag)?;
    }
    if Stamp::from(file.metadata()?) != before
        || !fs::symlink_metadata(&path)?.is_file()
        || Stamp::from(fs::metadata(&path)?) != before
    {
        return Err(Error::input(
            "display PNG changed while reading; restart with a stable file",
        ));
    }
    static_display(bytes, flag)
}
fn static_display(mut bytes: Vec<u8>, flag: &AtomicUsize) -> Result<DisplayPng> {
    let info = scan_options(&mut std::io::Cursor::new(&bytes), flag, false)?;
    compact_animation(&mut bytes, &info.animation, flag)?;
    Ok(DisplayPng {
        width: info.width,
        height: info.height,
        bytes,
    })
}
fn compact_animation(
    bytes: &mut Vec<u8>,
    removed: &[Range<u64>],
    flag: &AtomicUsize,
) -> Result<()> {
    // Ranges come from the complete, bounded envelope scan, in source order.
    // Compact in place so a near-limit APNG does not require two 80 MiB buffers.
    let end = bytes.len() as u64;
    let (mut read, mut write) = (0, 0);
    for skip in removed.iter().cloned().chain(std::iter::once(end..end)) {
        check_cancelled(flag)?;
        while read < skip.start as usize {
            check_cancelled(flag)?;
            let n = (skip.start as usize - read).min(BLOCK);
            if read != write {
                bytes.copy_within(read..read + n, write);
            }
            read += n;
            write += n;
        }
        read = skip.end as usize;
    }
    check_cancelled(flag)?;
    bytes.truncate(write);
    Ok(())
}
fn copy(
    input: &mut (impl Read + Seek),
    output: &mut impl Write,
    range: Range<u64>,
    scratch: &mut [u8],
    flag: &AtomicUsize,
) -> Result<()> {
    input.seek(SeekFrom::Start(range.start))?;
    let mut remaining = range.end - range.start;
    while remaining != 0 {
        let n = remaining.min(scratch.len() as u64) as usize;
        read_exact(input, &mut scratch[..n], flag)?;
        output.write_all(&scratch[..n])?;
        remaining -= n as u64;
    }
    Ok(())
}
fn validate_rewrite(info: &Scan, text: Option<&str>) -> Result<()> {
    let removed: u64 = info.owned.iter().map(|r| r.end - r.start).sum();
    let added = text.map_or(0, |s| (s.len() + BODY_HEAD.len() + 12) as u64);
    if info.length - removed + added > MAX_PNG
        || info.chunks - info.owned.len() + usize::from(text.is_some()) > MAX_CHUNKS
    {
        return Err(Error::input(
            "edited PNG would exceed the 1 GiB/65536 chunk limit",
        ));
    }
    if text.is_some_and(|s| s.len() > MAX_TEXT) {
        return Err(Error::input("metadata exceeds 16 MiB"));
    }
    Ok(())
}
fn rewrite(
    input: &mut (impl Read + Seek),
    info: &Scan,
    output: &mut impl Write,
    text: Option<&str>,
    flag: &AtomicUsize,
) -> Result<()> {
    validate_rewrite(info, text)?;
    let mut scratch = vec![0; BLOCK];
    let mut start = 0;
    for skip in &info.owned {
        copy(input, output, start..skip.start, &mut scratch, flag)?;
        start = skip.end;
    }
    copy(input, output, start..info.iend, &mut scratch, flag)?;
    if let Some(text) = text {
        if text.len() > MAX_TEXT {
            return Err(Error::input("metadata exceeds 16 MiB"));
        }
        output.write_all(&((BODY_HEAD.len() + text.len()) as u32).to_be_bytes())?;
        output.write_all(b"iTXt")?;
        output.write_all(BODY_HEAD)?;
        let mut hash = crc32fast::Hasher::new();
        hash.update(b"iTXt");
        hash.update(BODY_HEAD);
        for bytes in text.as_bytes().chunks(BLOCK) {
            check_cancelled(flag)?;
            hash.update(bytes);
            output.write_all(bytes)?;
        }
        output.write_all(&hash.finalize().to_be_bytes())?;
    }
    copy(input, output, info.iend..info.length, &mut scratch, flag)
}
#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
    mode: u32,
    links: u64,
}
impl From<Metadata> for Stamp {
    fn from(m: Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            len: m.len(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
            mode: m.mode(),
            links: m.nlink(),
        }
    }
}
struct Input {
    path: PathBuf,
    file: File,
    stamp: Stamp,
    scan: Scan,
}
impl Input {
    fn open(path: &Path, mutate: bool, flag: &AtomicUsize) -> Result<Self> {
        check_cancelled(flag)?;
        let path = if mutate {
            artifact::protected_output(path, &[], &[])?
        } else {
            fs::canonicalize(path)?
        };
        let mut file = regular_file(&path)?;
        let stamp = Stamp::from(file.metadata()?);
        if mutate && stamp.links != 1 {
            return Err(Error::input("PNG hardlink edits are unsupported"));
        }
        let scan = scan(&mut file, flag)?;
        let input = Self {
            path,
            file,
            stamp,
            scan,
        };
        input.unchanged()?;
        Ok(input)
    }
    fn unchanged(&self) -> Result<()> {
        if !fs::symlink_metadata(&self.path)?.is_file()
            || Stamp::from(self.file.metadata()?) != self.stamp
            || Stamp::from(fs::metadata(&self.path)?) != self.stamp
        {
            return Err(Error::input(
                "PNG changed while reading; reopen before editing",
            ));
        }
        Ok(())
    }
}
pub fn read(path: &Path, flag: &AtomicUsize) -> Result<Option<String>> {
    Ok(Input::open(path, false, flag)?.scan.text)
}
/// Decorate an owned native PNG into an unpublished artifact. The caller keeps
/// the previous destination intact until BOTH image and metadata are complete.
pub fn stage(
    path: &Path,
    bytes: &[u8],
    doc: &Document,
    flag: &AtomicUsize,
) -> Result<StagedArtifact> {
    let mut input = std::io::Cursor::new(bytes);
    let info = scan(&mut input, flag)?;
    let text = doc.serialize(None, false, flag)?;
    validate_rewrite(&info, text.as_deref())?;
    StagedArtifact::write(path, flag, |out| {
        rewrite(&mut input, &info, out, text.as_deref(), flag)
    })
}
/// In-place CLI edit. Atomic per PNG, not a multi-file transaction or a
/// cross-process compare-and-swap: metadata is rechecked just before rename.
pub fn edit(path: &Path, doc: &Document, append: bool, flag: &AtomicUsize) -> Result<()> {
    let mut input = Input::open(path, true, flag)?;
    let text = doc.serialize(input.scan.text.as_deref(), append, flag)?;
    validate_rewrite(&input.scan, text.as_deref())?;
    let stage = StagedArtifact::write(&input.path, flag, |out| {
        rewrite(&mut input.file, &input.scan, out, text.as_deref(), flag)?;
        out.set_permissions(Permissions::from_mode(input.stamp.mode & 0o777))?;
        Ok(())
    })?;
    input.unchanged()?;
    stage.commit(flag)
}
/// Small native-only smoke for the public --selftest flag. Full Python/flateyes
/// byte and parser parity lives in the development oracle, never a fallback.
pub fn selftest(flag: &AtomicUsize) -> Result<()> {
    use super::Annotation;
    use std::io::Cursor;
    let mut original = Cursor::new(Vec::new());
    crate::shots::mosaic::encode_png(&mut original, 2, 2, &[48, 96, 144, 255].repeat(4), flag)?;
    let original = original.into_inner();
    let mut doc = Document {
        ppu: Some(8.5),
        note: Some("메모\nhello".into()),
        legend: Some(vec![super::legend_line("box sky speckle MASK")?]),
        ..Document::default()
    };
    for (k, v) in [
        ("box", "0,0,1,1,red,sky,2,dashed"),
        ("ellipse", "-1,-1,1,1"),
        ("line", "0,0,1,1,green"),
        ("path", "0,0,1,1,2,1,pink"),
        ("polygon", "0,0,1,0,1,1,0,sky"),
        ("ruler", "0,0,1,1"),
        ("text", "0,0,size=20,메모\\nhello"),
    ] {
        doc.annotations.push(Annotation::option(k, v)?);
    }
    let text = doc.serialize(None, false, flag)?;
    let mut input = Cursor::new(&original);
    let info = scan(&mut input, flag)?;
    let mut embedded = Vec::new();
    rewrite(&mut input, &info, &mut embedded, text.as_deref(), flag)?;
    let mut input = Cursor::new(&embedded);
    let info = scan(&mut input, flag)?;
    if info.text != text {
        return Err(Error::input("PNG metadata selftest round-trip mismatch"));
    }
    let mut stripped = Vec::new();
    rewrite(&mut input, &info, &mut stripped, None, flag)?;
    if stripped != original {
        return Err(Error::input("PNG metadata selftest changed pixels/chunks"));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_scan_bounds_static_images_without_interpreting_annotations() {
        let flag = AtomicUsize::new(0);
        let mut png = std::io::Cursor::new(Vec::new());
        crate::shots::mosaic::encode_png(&mut png, 4, 2, &[0, 0, 0, 255].repeat(8), &flag).unwrap();
        let png = png.into_inner();
        let check = |bytes: &[u8]| scan_options(&mut std::io::Cursor::new(bytes), &flag, false);
        let info = check(&png).unwrap();
        assert_eq!((info.width, info.height), (4, 2));
        let mut trailing = png.clone();
        trailing.push(0);
        assert!(check(&trailing).is_err());
        assert!(scan(&mut std::io::Cursor::new(trailing), &flag).is_ok());
        let chunk = |kind: &[u8; 4], body: &[u8]| {
            let mut b = (body.len() as u32).to_be_bytes().to_vec();
            b.extend_from_slice(kind);
            b.extend_from_slice(body);
            b.extend_from_slice(&crc32fast::hash(&b[4..]).to_be_bytes());
            b
        };
        for kind in [b"acTL", b"fcTL", b"fdAT"] {
            let mut b = png[..33].to_vec();
            b.extend(chunk(kind, &[0; 8]));
            b.extend_from_slice(&png[33..]);
            let info = check(&b).unwrap();
            assert_eq!(info.animation.len(), 1);
            assert_eq!(info.animation[0], 33..53);
            assert!(scan(&mut std::io::Cursor::new(&b), &flag)
                .unwrap()
                .animation
                .is_empty());
            let mut input = std::io::Cursor::new(&b);
            let annotation_scan = scan(&mut input, &flag).unwrap();
            let mut rewritten = Vec::new();
            rewrite(&mut input, &annotation_scan, &mut rewritten, None, &flag).unwrap();
            assert_eq!(rewritten, b); // fe-embed must preserve animation chunks.
            assert_eq!(static_display(b.clone(), &flag).unwrap().bytes, png);
            b[45] ^= 1;
            assert!(check(&b).is_err()); // Do not hide corruption in a removed chunk.
        }
        let mut annotated = png[..33].to_vec();
        annotated.extend(chunk(b"iTXt", b"flateyes\0\xffbad"));
        annotated.extend_from_slice(&png[33..]);
        assert!(check(&annotated).is_ok());
        assert!(scan(&mut std::io::Cursor::new(annotated), &flag).is_err());
        for (w, h) in [(8193u32, 1u32), (8192, 8192), (u32::MAX, u32::MAX), (0, 1)] {
            let mut b = png.clone();
            b[16..20].copy_from_slice(&w.to_be_bytes());
            b[20..24].copy_from_slice(&h.to_be_bytes());
            let crc = crc32fast::hash(&b[12..29]);
            b[29..33].copy_from_slice(&crc.to_be_bytes());
            assert!(check(&b).is_err());
        }
        flag.store(2, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(check(&png).err().unwrap().kind, crate::ErrorKind::Cancelled);
    }
    #[test]
    fn static_snapshot_compacts_only_animation_without_a_second_image_buffer() {
        let flag = AtomicUsize::new(0);
        let chunk = |kind: &[u8; 4], body: &[u8]| {
            let mut b = (body.len() as u32).to_be_bytes().to_vec();
            b.extend_from_slice(kind);
            b.extend_from_slice(body);
            b.extend_from_slice(&crc32fast::hash(&b[4..]).to_be_bytes());
            b
        };
        let mut png = std::io::Cursor::new(Vec::new());
        crate::shots::mosaic::encode_png(&mut png, 4, 2, &[255, 0, 0, 128].repeat(8), &flag)
            .unwrap();
        let png = png.into_inner();
        // Animation bodies are deliberately opaque/semantically invalid:
        // default-image diagnosis is not an animation sequence validator.
        let mut bytes = png[..33].to_vec();
        bytes.extend(chunk(b"acTL", &[0; 8]));
        bytes.extend(chunk(b"fcTL", &[0; 26]));
        let mut text = b"note\0".to_vec();
        text.resize(BLOCK + 19, b'a');
        let keep = chunk(b"tEXt", &text);
        bytes.extend_from_slice(&keep);
        bytes.extend_from_slice(&png[33..png.len() - 12]);
        bytes.extend(chunk(b"fdAT", &[0; 32]));
        let note = chunk(b"iTXt", b"flateyes\0\xffnot interpreted");
        bytes.extend_from_slice(&note);
        bytes.extend_from_slice(&png[png.len() - 12..]);
        let (ptr, capacity) = (bytes.as_ptr(), bytes.capacity());
        let output = static_display(bytes, &flag).unwrap();
        assert_eq!(
            (output.bytes.as_ptr(), output.bytes.capacity()),
            (ptr, capacity)
        );
        let mut expected = png[..33].to_vec();
        expected.extend(keep);
        expected.extend_from_slice(&png[33..png.len() - 12]);
        expected.extend(note);
        expected.extend_from_slice(&png[png.len() - 12..]);
        assert_eq!(output.bytes, expected);
        assert!(
            scan_options(&mut std::io::Cursor::new(&output.bytes), &flag, false)
                .unwrap()
                .animation
                .is_empty()
        );
        flag.store(1, std::sync::atomic::Ordering::Relaxed);
        let mut untouched = output.bytes.clone();
        assert_eq!(
            compact_animation(&mut untouched, &[], &flag)
                .unwrap_err()
                .kind,
            crate::ErrorKind::Cancelled
        );
        assert_eq!(untouched, output.bytes);
    }
    #[test]
    fn rewritten_output_remains_readable_within_limits() {
        let mut info = Scan {
            width: 1,
            height: 1,
            length: MAX_PNG,
            chunks: 3,
            iend: MAX_PNG - 12,
            owned: vec![],
            animation: vec![],
            text: None,
        };
        assert!(validate_rewrite(&info, Some("x")).is_err());
        assert!(validate_rewrite(&info, None).is_ok());
        info.length = 100;
        info.iend = 88;
        info.chunks = MAX_CHUNKS;
        assert!(validate_rewrite(&info, Some("x")).is_err());
        info.owned.push(20..80);
        assert!(validate_rewrite(&info, Some("x")).is_ok());
    }
    #[test]
    fn changed_input_is_not_silently_replaced() {
        static SERIAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        struct Temp(PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let directory = std::env::temp_dir().join(format!(
            "floe-png-identity-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let directory = Temp(directory);
        let path = directory.0.join("image.png");
        let flag = AtomicUsize::new(0);
        let mut data = std::io::Cursor::new(Vec::new());
        crate::shots::mosaic::encode_png(&mut data, 1, 1, &[0, 0, 0, 255], &flag).unwrap();
        fs::write(&path, data.get_ref()).unwrap();
        let input = Input::open(&path, true, &flag).unwrap();
        let replacement = directory.0.join("replacement.png");
        fs::write(&replacement, data.get_ref()).unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert!(input.unchanged().is_err());
        assert_eq!(fs::read(&path).unwrap(), *data.get_ref());
        flag.store(15, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            edit(&path, &Document::default(), false, &flag)
                .unwrap_err()
                .kind,
            crate::ErrorKind::Cancelled
        );
        assert_eq!(fs::read(&path).unwrap(), *data.get_ref());
    }
    #[test]
    fn native_roundtrip_and_corruption() {
        let flag = AtomicUsize::new(0);
        selftest(&flag).unwrap();
        let mut bytes = std::io::Cursor::new(Vec::new());
        crate::shots::mosaic::encode_png(&mut bytes, 1, 1, &[0, 0, 0, 255], &flag).unwrap();
        let mut bytes = bytes.into_inner();
        bytes[29] ^= 1;
        assert!(scan(&mut std::io::Cursor::new(bytes), &flag).is_err());
        assert!(scan(&mut std::io::Cursor::new(b"not a PNG"), &flag).is_err());
        let mut body = BODY_HEAD.to_vec();
        body.extend_from_slice(b"a");
        assert!(decode_text(&body, 0, &flag).is_err());
    }
}
