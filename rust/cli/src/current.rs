//! Whether a cache folder holds the source's cache as the source stands
//! (0.12.458; user 2026-11-12: `floe-index vfs` run twice on one layout
//! deleted the first run's files and made them again). `floe2 index` has
//! always asked this before it ran the builder (app-core cache::inspect);
//! the builder itself did not, and built whatever was there again. The rule
//! here is that one's, so that the two commands say the same of a folder:
//! meta.json of this binary's cache version naming the source's size and
//! mtime, and the cache's own marker - validated as a reader opens it -
//! naming them too.
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// The cache a command makes: the full index (design.ovm, its commit
/// marker, with the page files) or the light cache (design.inv + design.seg
/// and no design.ovm).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Full,
    Light,
}

/// Why a folder's cache is not the source's as it stands.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Not {
    /// no cache of that kind is in the folder: nothing to say, a build makes it
    Absent,
    /// one is there and is not current, for this reason
    Stale(String),
}

/// The identity a cache's meta.json states: the cache version, the `vfs`
/// flag and the source's size and mtime at the build - the fields `floe2
/// index` reads (app-core MetaIdentity) and nothing else of the document.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MetaIdentity {
    pub version: u64,
    pub vfs: u64,
    pub size: u64,
    pub mtime: u64,
}

/// The source's identity as a cache records it: its size and its mtime in
/// whole seconds.
fn fingerprint(src: &str) -> Result<(u64, u64), String> {
    let meta = std::fs::metadata(src).map_err(|e| format!("source {}: {}", src, e))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Ok((meta.len(), mtime))
}

/// Ok: the folder's cache of `kind` is the source's as it stands - a build
/// would make the same again. The source's size and mtime with it.
pub(crate) fn of(src: &str, outdir: &str, kind: Kind) -> Result<(u64, u64), Not> {
    let dir = Path::new(outdir);
    let regular = |name: &str| std::fs::symlink_metadata(dir.join(name)).is_ok_and(|m| m.is_file());
    let marker = match kind {
        Kind::Full => "design.ovm",
        Kind::Light => floe_oasis::invfile::FILE_NAME,
    };
    match std::fs::symlink_metadata(dir) {
        Err(_) => return Err(Not::Absent),
        Ok(m) if !m.is_dir() => {
            return Err(Not::Stale(
                "the cache path is not a real directory".to_string(),
            ))
        }
        Ok(_) => (),
    }
    if !regular(marker) {
        // (a light cache beside no index, an index over a light cache: each
        // command makes its own kind)
        return Err(Not::Absent);
    }
    let stale = |why: String| Not::Stale(why);
    let meta_path = dir.join("meta.json");
    let text = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&meta_path)
        .and_then(|f| {
            if !f.metadata()?.is_file() {
                return Err(std::io::Error::other("not a regular file"));
            }
            std::io::read_to_string(f)
        })
        .map_err(|e| stale(format!("meta.json is not readable ({e}): a build that stopped half-way?")))?;
    let meta = meta_identity(&text).map_err(|e| stale(format!("meta.json is not readable ({e})")))?;
    if meta.version != crate::CACHE_VERSION || meta.vfs != 1 {
        return Err(stale(format!(
            "it is of cache version {} (this indexer makes {})",
            meta.version,
            crate::CACHE_VERSION
        )));
    }
    let (size, mtime) = fingerprint(src).map_err(stale)?;
    let changed = |what: &str, was: (u64, u64)| {
        stale(format!(
            "the source has changed since ({what} size {} mtime {}, the file now {size} / {mtime})",
            was.0, was.1
        ))
    };
    if (meta.size, meta.mtime) != (size, mtime) {
        return Err(changed("meta.json says", (meta.size, meta.mtime)));
    }
    let marked = match kind {
        Kind::Full => {
            if !regular("design.ovp")
                || std::fs::symlink_metadata(dir.join("design.ovt")).is_ok_and(|m| !m.is_file())
            {
                return Err(stale("its page files are not regular files".to_string()));
            }
            let vfs = floe_vfs::Vfs::open(outdir)
                .map_err(|e| stale(format!("its commit marker does not validate ({e})")))?;
            (vfs.ovm.src_size, vfs.ovm.src_mtime)
        }
        Kind::Light => {
            let files = floe_vfs::light::check(outdir)
                .map_err(|e| stale(format!("its inventory does not validate ({e})")))?;
            (files.identity.src_size, files.identity.src_mtime)
        }
    };
    if marked != (size, mtime) {
        return Err(changed("its marker says", marked));
    }
    Ok((size, mtime))
}

/// Whether another run writes the folder's cache now (its build lock held):
/// then nothing is said of the cache here, and the run goes on to the lock,
/// which refuses it as it always has.
pub(crate) fn being_written(outdir: &str) -> bool {
    floe_vfs::lock::writing_refusal(&floe_vfs::lock::key(floe_vfs::lock::Kind::Vfs, outdir)).is_some()
}

/// `MetaIdentity` of a meta.json's text. The whole document is read - a
/// file that is not one JSON object to its end states nothing.
pub(crate) fn meta_identity(text: &str) -> Result<MetaIdentity, String> {
    let mut json = Json {
        text: text.as_bytes(),
        at: 0,
    };
    let (mut version, mut vfs, mut size, mut mtime) = (None, None, None, None);
    let mut src = false;
    // a field given twice states nothing: `floe2 index` reads the document
    // into a struct, which refuses a duplicate field
    fn once(slot: &mut Option<u64>, name: &str, value: u64) -> Result<(), String> {
        match slot.replace(value) {
            None => Ok(()),
            Some(_) => Err(format!("{name} given twice")),
        }
    }
    json.object(0, |json, key| match key {
        "version" => json.unsigned().and_then(|n| once(&mut version, "version", n)),
        "vfs" => json.unsigned().and_then(|n| once(&mut vfs, "vfs", n)),
        "src" => {
            if std::mem::replace(&mut src, true) {
                return Err("src given twice".to_string());
            }
            json.object(1, |json, key| match key {
                "size" => json.unsigned().and_then(|n| once(&mut size, "src.size", n)),
                "mtime" => json.unsigned().and_then(|n| once(&mut mtime, "src.mtime", n)),
                _ => json.skip(2),
            })
        }
        _ => json.skip(1),
    })?;
    if json.peek().is_some() {
        return Err(format!("text after the document at byte {}", json.at));
    }
    let need = |value: Option<u64>, name: &str| value.ok_or_else(|| format!("no {name}"));
    Ok(MetaIdentity {
        version: need(version, "version")?,
        vfs: need(vfs, "vfs")?,
        size: need(size, "src.size")?,
        mtime: need(mtime, "src.mtime")?,
    })
}

/// As much of a JSON reader as an identity takes: objects walked by key,
/// whole numbers read, everything else passed over.
struct Json<'a> {
    text: &'a [u8],
    at: usize,
}

const DEPTH_MAX: usize = 64;

impl<'a> Json<'a> {
    fn peek(&mut self) -> Option<u8> {
        while self
            .text
            .get(self.at)
            .is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.at += 1;
        }
        self.text.get(self.at).copied()
    }

    fn eat(&mut self, want: u8) -> Result<(), String> {
        if self.peek() == Some(want) {
            self.at += 1;
            Ok(())
        } else {
            Err(format!("`{}` expected at byte {}", want as char, self.at))
        }
    }

    /// A string's text as written (an escape is left as it is: the keys
    /// read here have none).
    fn string(&mut self) -> Result<&'a str, String> {
        self.eat(b'"')?;
        let start = self.at;
        loop {
            match self.text.get(self.at) {
                None => return Err("a string without its end".to_string()),
                Some(b'"') => {
                    let raw = &self.text[start..self.at];
                    self.at += 1;
                    return std::str::from_utf8(raw).map_err(|_| "a string that is not UTF-8".to_string());
                }
                Some(b'\\') => self.at += 2,
                Some(c) if *c < 0x20 => return Err(format!("a control character in a string at byte {}", self.at)),
                Some(_) => self.at += 1,
            }
        }
    }

    /// A whole number that is not negative, as serde reads a u64: digits
    /// and nothing after them that makes a fraction or an exponent.
    fn unsigned(&mut self) -> Result<u64, String> {
        self.peek();
        let start = self.at;
        while self.text.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        let digits = &self.text[start..self.at];
        if digits.is_empty()
            || (digits.len() > 1 && digits[0] == b'0')
            || matches!(self.text.get(self.at), Some(b'.' | b'e' | b'E'))
        {
            return Err(format!("a whole number expected at byte {start}"));
        }
        std::str::from_utf8(digits)
            .ok()
            .and_then(|d| d.parse().ok())
            .ok_or_else(|| format!("a number out of range at byte {start}"))
    }

    /// An object, its members handed to `member` by key (which reads or
    /// skips the value).
    fn object(
        &mut self,
        depth: usize,
        mut member: impl FnMut(&mut Self, &str) -> Result<(), String>,
    ) -> Result<(), String> {
        if depth > DEPTH_MAX {
            return Err("nested too deep".to_string());
        }
        self.eat(b'{')?;
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(());
        }
        loop {
            let key = self.string()?;
            self.eat(b':')?;
            member(self, key)?;
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(());
                }
                _ => return Err(format!("`,` or `}}` expected at byte {}", self.at)),
            }
        }
    }

    /// Any value, passed over.
    fn skip(&mut self, depth: usize) -> Result<(), String> {
        if depth > DEPTH_MAX {
            return Err("nested too deep".to_string());
        }
        match self.peek() {
            Some(b'{') => self.object(depth, |json, _| json.skip(depth + 1)),
            Some(b'[') => {
                self.at += 1;
                if self.peek() == Some(b']') {
                    self.at += 1;
                    return Ok(());
                }
                loop {
                    self.skip(depth + 1)?;
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(());
                        }
                        _ => return Err(format!("`,` or `]` expected at byte {}", self.at)),
                    }
                }
            }
            Some(b'"') => self.string().map(|_| ()),
            Some(c) if c == b'-' || c.is_ascii_digit() => {
                while self
                    .text
                    .get(self.at)
                    .is_some_and(|c| matches!(c, b'-' | b'+' | b'.' | b'e' | b'E') || c.is_ascii_digit())
                {
                    self.at += 1;
                }
                Ok(())
            }
            Some(_) => {
                for word in ["true", "false", "null"] {
                    if self.text[self.at..].starts_with(word.as_bytes()) {
                        self.at += word.len();
                        return Ok(());
                    }
                }
                Err(format!("a value expected at byte {}", self.at))
            }
            None => Err("the document ends in the middle".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_meta_json_states_its_identity() {
        // the builder's own text, the light cache's with its extra key
        let full = "{\n\"version\": 8,\n\"vfs\": 1,\n\"src\": {\"path\": \"/a/b \\\"q\\\" {x}.oas\", \"size\": 36193279, \"mtime\": 1794012345},\n\"dbu\": 0.001,\n\"top_cell\": \"TOP\",\n\"bbox\": [0, -5, 10, 20],\n\"grid\": {\"nx\": 4, \"ny\": 4, \"x0\": 0, \"y0\": -5, \"tile_w\": 3, \"tile_h\": 7},\n\"layers\": [\n{\"layer\": 1, \"datatype\": 0, \"name\": \"M1\", \"visible\": true, \"note\": null}\n],\n\"texts\": {\"records\": 0},\n\"frontier\": {\"depths\": [[1.5e3, -2]]}\n}\n";
        let want = MetaIdentity {
            version: 8,
            vfs: 1,
            size: 36193279,
            mtime: 1794012345,
        };
        assert_eq!(meta_identity(full), Ok(want));
        let light = full.replace("\"vfs\": 1,\n", "\"vfs\": 1,\n\"cache\": \"light\",\n");
        assert_eq!(meta_identity(&light).map(|m| m.size), Ok(36193279));
        // any order, no spaces, the largest numbers
        assert_eq!(
            meta_identity("{\"src\":{\"mtime\":0,\"size\":18446744073709551615},\"vfs\":1,\"version\":9}"),
            Ok(MetaIdentity {
                version: 9,
                vfs: 1,
                size: u64::MAX,
                mtime: 0
            })
        );
        // a key that is no identity field may come twice
        assert!(meta_identity("{\"x\": 1, \"x\": 2, \"version\": 8, \"vfs\": 1, \"src\": {\"y\": 0, \"y\": 0, \"size\": 5, \"mtime\": 6}}").is_ok());
        // a key named like an identity field inside another value is not it
        assert!(meta_identity("{\"layers\": [{\"version\": 8, \"vfs\": 1, \"src\": {\"size\": 1, \"mtime\": 2}}]}").is_err());
        // what is not a whole document, or lacks a field, or has a number
        // that is not a whole non-negative one, states nothing
        for bad in [
            "",
            "8",
            "[]",
            "{\"version\": 8, \"vfs\": 1}",
            "{\"version\": 8, \"vfs\": 1, \"src\": {\"size\": 5}}",
            "{\"version\": 8, \"vfs\": 1, \"src\": {\"size\": 5, \"mtime\": 6}",
            "{\"version\": 8, \"vfs\": 1, \"src\": {\"size\": 5, \"mtime\": 6}} trailing",
            "{\"version\": 8.0, \"vfs\": 1, \"src\": {\"size\": 5, \"mtime\": 6}}",
            "{\"version\": -8, \"vfs\": 1, \"src\": {\"size\": 5, \"mtime\": 6}}",
            "{\"version\": \"8\", \"vfs\": 1, \"src\": {\"size\": 5, \"mtime\": 6}}",
            "{\"version\": 08, \"vfs\": 1, \"src\": {\"size\": 5, \"mtime\": 6}}",
            "{\"version\": 8, \"vfs\": 1, \"src\": {\"size\": 5e3, \"mtime\": 6}}",
            "{\"version\": 8, \"vfs\": 1, \"src\": {\"size\": 18446744073709551616, \"mtime\": 6}}",
            "{\"version\": 8, \"vfs\": 1, \"src\": [5, 6]}",
            "{\"version\": 8, \"version\": 8, \"vfs\": 1, \"src\": {\"size\": 5, \"mtime\": 6}}",
            "{\"version\": 8, \"vfs\": 1, \"src\": {\"size\": 5, \"mtime\": 6, \"size\": 5}}",
            "{\"version\": 8, \"vfs\": 1, \"src\": {\"size\": 5, \"mtime\": 6}, \"src\": {}}",
            "{\"version\": 8 \"vfs\": 1, \"src\": {\"size\": 5, \"mtime\": 6}}",
            "{\"version\": 8, \"vfs\": 1, \"src\": {\"size\": 5, \"mtime\": 6}, \"x\": tru}",
            "{\"version\": 8, \"vfs\": 1, \"src\": {\"size\": 5, \"mtime\": 6}, \"x\": \"open}",
        ] {
            assert!(meta_identity(bad).is_err(), "{bad}");
        }
        let deep = format!(
            "{{\"version\": 8, \"vfs\": 1, \"src\": {{\"size\": 5, \"mtime\": 6}}, \"x\": {}{}}}",
            "[".repeat(200),
            "]".repeat(200)
        );
        assert!(meta_identity(&deep).unwrap_err().contains("deep"));
    }

    #[test]
    fn a_folder_without_the_cache_says_nothing() {
        let dir = std::env::temp_dir().join(format!("floe-current-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let folder = dir.to_str().unwrap();
        let src = dir.join("x.oas");
        // no folder, an empty one, one with a meta.json alone
        assert_eq!(of(src.to_str().unwrap(), folder, Kind::Full), Err(Not::Absent));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(of(src.to_str().unwrap(), folder, Kind::Full), Err(Not::Absent));
        assert_eq!(of(src.to_str().unwrap(), folder, Kind::Light), Err(Not::Absent));
        std::fs::write(dir.join("meta.json"), "{}").unwrap();
        assert_eq!(of(src.to_str().unwrap(), folder, Kind::Full), Err(Not::Absent));
        // a marker that is no cache: said why, each kind by its own marker
        std::fs::write(dir.join("design.ovm"), b"not a marker").unwrap();
        assert!(matches!(of(src.to_str().unwrap(), folder, Kind::Full), Err(Not::Stale(why)) if why.contains("meta.json")));
        assert_eq!(of(src.to_str().unwrap(), folder, Kind::Light), Err(Not::Absent));
        // a path that is a file
        let file = dir.join("file");
        std::fs::write(&file, b"x").unwrap();
        assert!(matches!(of(src.to_str().unwrap(), file.to_str().unwrap(), Kind::Full), Err(Not::Stale(why)) if why.contains("not a real directory")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
