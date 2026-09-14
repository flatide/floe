//! Strict ELF64 little-endian x86-64 deployment audit. No executing ELF/ldd,
//! no searching arbitrary string bytes for GLIBC requirements.
use super::Result;
pub type Version = [u32; 3];
fn unique(slot: &mut Option<u64>, value: u64, name: &str) -> Result<()> {
    if slot.replace(value).is_some() {
        return Err(format!("duplicate {name}").into());
    }
    Ok(())
}
pub fn version(s: &str) -> Result<Version> {
    let parts: Vec<_> = s.split('.').collect();
    if !(2..=3).contains(&parts.len()) {
        return Err("version must be major.minor[.patch]".into());
    }
    let mut result = [0; 3];
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid version".into());
        }
        result[i] = part.parse()?;
    }
    Ok(result)
}
fn range(bytes: &[u8], at: u64, size: u64) -> Result<&[u8]> {
    let end = at.checked_add(size).ok_or("ELF range overflow")?;
    bytes
        .get(usize::try_from(at)?..usize::try_from(end)?)
        .ok_or_else(|| "ELF range outside file".into())
}
fn u16_at(b: &[u8], p: u64) -> Result<u16> {
    Ok(u16::from_le_bytes(range(b, p, 2)?.try_into()?))
}
fn u32_at(b: &[u8], p: u64) -> Result<u32> {
    Ok(u32::from_le_bytes(range(b, p, 4)?.try_into()?))
}
fn u64_at(b: &[u8], p: u64) -> Result<u64> {
    Ok(u64::from_le_bytes(range(b, p, 8)?.try_into()?))
}
fn string(b: &[u8], at: u64) -> Result<&str> {
    let tail = range(
        b,
        at,
        (b.len() as u64)
            .checked_sub(at)
            .ok_or("string offset outside table")?,
    )?;
    let end = tail
        .iter()
        .position(|b| *b == 0)
        .ok_or("unterminated ELF string")?;
    let s = std::str::from_utf8(&tail[..end])?;
    if !s.is_ascii() || s.chars().any(char::is_control) {
        return Err("non-ASCII/control ELF name".into());
    }
    Ok(s)
}
#[derive(Clone)]
struct Segment {
    typ: u32,
    offset: u64,
    addr: u64,
    size: u64,
}
fn virtual_data<'a>(b: &'a [u8], segments: &[Segment], addr: u64, size: u64) -> Result<&'a [u8]> {
    for s in segments.iter().filter(|s| s.typ == 1) {
        if let Some(relative) = addr.checked_sub(s.addr) {
            if relative.checked_add(size).is_some_and(|end| end <= s.size) {
                return range(
                    b,
                    s.offset
                        .checked_add(relative)
                        .ok_or("ELF address overflow")?,
                    size,
                );
            }
        }
    }
    Err("ELF virtual address outside LOAD segments".into())
}
fn virtual_tail<'a>(b: &'a [u8], segments: &[Segment], addr: u64) -> Result<&'a [u8]> {
    for s in segments.iter().filter(|s| s.typ == 1) {
        if let Some(relative) = addr.checked_sub(s.addr).filter(|r| *r < s.size) {
            return range(
                b,
                s.offset
                    .checked_add(relative)
                    .ok_or("ELF address overflow")?,
                s.size - relative,
            );
        }
    }
    Err("ELF version address outside LOAD segment".into())
}
pub fn audit(b: &[u8], musl: bool, ceiling: Version) -> Result<String> {
    if range(b, 0, 7)? != b"\x7fELF\x02\x01\x01"
        || u16_at(b, 18)? != 62
        || !matches!(u16_at(b, 16)?, 2 | 3)
        || u32_at(b, 20)? != 1
        || u16_at(b, 52)? != 64
    {
        return Err("expected executable ELF64 little-endian x86-64".into());
    }
    let (ph, phsize, phnum) = (u64_at(b, 32)?, u16_at(b, 54)? as u64, u16_at(b, 56)? as u64);
    if phsize != 56 || phnum == 0 || phnum == 65535 {
        return Err("unsupported ELF program headers".into());
    }
    range(b, ph, phsize * phnum)?;
    let mut segments = Vec::new();
    let mut interpreter = None;
    for i in 0..phnum {
        let p = ph + i * phsize;
        let s = Segment {
            typ: u32_at(b, p)?,
            offset: u64_at(b, p + 8)?,
            addr: u64_at(b, p + 16)?,
            size: u64_at(b, p + 32)?,
        };
        range(b, s.offset, s.size)?;
        if s.typ == 3 {
            if interpreter.is_some() {
                return Err("multiple ELF interpreters".into());
            }
            interpreter = Some(string(range(b, s.offset, s.size)?, 0)?.to_owned());
        }
        segments.push(s);
    }
    let dynamic: Vec<_> = segments.iter().filter(|s| s.typ == 2).collect();
    if dynamic.len() > 1 {
        return Err("multiple ELF dynamic tables".into());
    }
    let mut needed_offsets = Vec::new();
    let mut strtab = None;
    let mut strsize = None;
    let mut veraddr = None;
    let mut vernum = None;
    for s in dynamic {
        let data = range(b, s.offset, s.size)?;
        if data.len() % 16 != 0 {
            return Err("misaligned dynamic table".into());
        }
        let mut ended = false;
        for item in data.chunks_exact(16) {
            let tag = u64_at(item, 0)?;
            let value = u64_at(item, 8)?;
            match tag {
                0 => {
                    ended = true;
                    break;
                }
                1 => needed_offsets.push(value),
                5 => {
                    if strtab.replace(value).is_some() {
                        return Err("duplicate dynamic string table".into());
                    }
                }
                10 => {
                    if strsize.replace(value).is_some() {
                        return Err("duplicate dynamic string size".into());
                    }
                }
                15 | 29 => return Err("RPATH/RUNPATH is not allowed in the portable bundle".into()),
                0x6ffffffe => unique(&mut veraddr, value, "DT_VERNEED")?,
                0x6fffffff => unique(&mut vernum, value, "DT_VERNEEDNUM")?,
                0x7fffffff | 0x7ffffffd | 0x6ffffefb | 0x6ffffefc => {
                    return Err("ELF filter/audit dependencies are not supported".into())
                }
                _ => (),
            }
        }
        if !ended {
            return Err("unterminated ELF dynamic table".into());
        }
    }
    let mut needed = Vec::new();
    if !needed_offsets.is_empty() {
        let table = virtual_data(
            b,
            &segments,
            strtab.ok_or("missing DT_STRTAB")?,
            strsize.ok_or("missing DT_STRSZ")?,
        )?;
        for offset in needed_offsets {
            needed.push(string(table, offset)?.to_owned());
        }
    }
    let mut requirements = std::collections::BTreeSet::new();
    if veraddr.is_some() != vernum.is_some() {
        return Err("incomplete dynamic version metadata".into());
    }
    // Audit the loader's actual addresses/count, not potentially stale section headers.
    if let (Some(addr), Some(count)) = (veraddr, vernum) {
        let data = virtual_tail(b, &segments, addr)?;
        let names = virtual_data(
            b,
            &segments,
            strtab.ok_or("missing DT_STRTAB")?,
            strsize.ok_or("missing DT_STRSZ")?,
        )?;
        if count == 0 {
            return Err("empty version requirement section".into());
        }
        let mut at = 0;
        for n in 0..count {
            if u16_at(data, at)? != 1 {
                return Err("unknown ELF version requirement format".into());
            }
            let owner = string(names, u32_at(data, at + 4)? as u64)?;
            if !needed.iter().any(|s| s == owner) {
                return Err("version owner is not DT_NEEDED".into());
            }
            let aux_count = u16_at(data, at + 2)?;
            if aux_count == 0 {
                return Err("version entry without names".into());
            }
            let first = u32_at(data, at + 8)? as u64;
            if first < 16 {
                return Err("bad first version auxiliary offset".into());
            }
            let mut aux = at.checked_add(first).ok_or("version aux overflow")?;
            for a in 0..aux_count {
                requirements.insert(string(names, u32_at(data, aux + 8)? as u64)?.to_owned());
                let next = u32_at(data, aux + 12)? as u64;
                if a + 1 == aux_count {
                    if next != 0 {
                        return Err("extra version auxiliary entries".into());
                    }
                } else {
                    if next < 16 {
                        return Err("bad next version auxiliary offset".into());
                    }
                    aux = aux.checked_add(next).ok_or("version aux overflow")?;
                }
            }
            let next = u32_at(data, at + 12)? as u64;
            if n + 1 == count {
                if next != 0 {
                    return Err("extra version requirement entries".into());
                }
            } else {
                if next < 16 {
                    return Err("bad next version requirement offset".into());
                }
                at = at.checked_add(next).ok_or("version overflow")?;
            }
        }
    }
    if musl {
        if interpreter.is_some() || !needed.is_empty() || !requirements.is_empty() {
            return Err(
                "musl bundle must have no interpreter, DT_NEEDED or symbol-version requirements"
                    .into(),
            );
        }
    } else {
        if interpreter.as_deref() != Some("/lib64/ld-linux-x86-64.so.2") {
            return Err("unsupported GNU ELF interpreter".into());
        }
        let allowed = [
            "libc.so.6",
            "libgcc_s.so.1",
            "libm.so.6",
            "libpthread.so.0",
            "librt.so.1",
            "libdl.so.2",
            "ld-linux-x86-64.so.2",
        ];
        if !needed.iter().any(|s| s == "libc.so.6")
            || needed.iter().any(|s| !allowed.contains(&s.as_str()))
        {
            return Err(format!("unexpected GNU shared libraries: {needed:?}").into());
        }
        if !requirements.iter().any(|s| s.starts_with("GLIBC_")) {
            return Err("missing GNU GLIBC version requirements".into());
        }
        for name in &requirements {
            if let Some(value) = name.strip_prefix("GLIBC_") {
                if version(value)? > ceiling {
                    return Err(format!("{name} exceeds GLIBC ceiling {ceiling:?}").into());
                }
            } else if !name.starts_with("GCC_") {
                return Err(format!("unknown symbol-version namespace: {name}").into());
            }
        }
    }
    Ok(format!("ELF64 x86-64; interpreter={interpreter:?}; needed={needed:?}; required={requirements:?}; glibc_ceiling={ceiling:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn w16(b: &mut [u8], p: usize, v: u16) {
        b[p..p + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn w32(b: &mut [u8], p: usize, v: u32) {
        b[p..p + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn w64(b: &mut [u8], p: usize, v: u64) {
        b[p..p + 8].copy_from_slice(&v.to_le_bytes());
    }
    fn fixture(gnu: bool) -> Vec<u8> {
        let mut b = vec![0; 1024];
        b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        w16(&mut b, 16, 3);
        w16(&mut b, 18, 62);
        w32(&mut b, 20, 1);
        w16(&mut b, 52, 64);
        w64(&mut b, 32, 64);
        w16(&mut b, 54, 56);
        w16(&mut b, 56, if gnu { 3 } else { 1 });
        w32(&mut b, 64, 1);
        w64(&mut b, 64 + 32, 1024);
        w64(&mut b, 40, 800);
        w16(&mut b, 58, 64);
        w16(&mut b, 60, 3);
        if gnu {
            w32(&mut b, 120, 3);
            w64(&mut b, 128, 256);
            w64(&mut b, 152, 28);
            b[256..284].copy_from_slice(b"/lib64/ld-linux-x86-64.so.2\0");
            w32(&mut b, 176, 2);
            w64(&mut b, 184, 576);
            w64(&mut b, 208, 96);
            w64(&mut b, 576, 1);
            w64(&mut b, 584, 1);
            w64(&mut b, 592, 5);
            w64(&mut b, 600, 400);
            w64(&mut b, 608, 10);
            w64(&mut b, 616, 80);
            w64(&mut b, 624, 0x6ffffffe);
            w64(&mut b, 632, 512);
            w64(&mut b, 640, 0x6fffffff);
            w64(&mut b, 648, 1);
            b[400..422].copy_from_slice(b"\0libc.so.6\0GLIBC_2.28\0");
            w32(&mut b, 868, 3);
            w64(&mut b, 888, 400);
            w64(&mut b, 896, 80);
            w32(&mut b, 932, 0x6ffffffe);
            w64(&mut b, 952, 512);
            w64(&mut b, 960, 32);
            w32(&mut b, 968, 1);
            w32(&mut b, 972, 1);
            w16(&mut b, 512, 1);
            w16(&mut b, 514, 1);
            w32(&mut b, 516, 1);
            w32(&mut b, 520, 16);
            w32(&mut b, 536, 11);
        }
        b
    }
    #[test]
    fn static_and_gnu_contracts() {
        let s = fixture(false);
        assert!(audit(&s, true, [2, 28, 0]).is_ok());
        assert!(audit(&s, false, [2, 28, 0]).is_err());
        let g = fixture(true);
        assert!(audit(&g, false, [2, 28, 0]).is_ok());
        assert!(audit(&g, true, [2, 28, 0]).is_err());
        assert!(audit(&g, false, [2, 27, 0]).is_err());
        let mut s = fixture(false);
        s[450..460].copy_from_slice(b"GLIBC_9.99");
        assert!(audit(&s, true, [2, 28, 0]).is_ok());
    }
    #[test]
    fn rejects_corrupt_tables_without_panicking() {
        let valid = fixture(true);
        for n in 0..valid.len() {
            let _ = audit(&valid[..n], false, [2, 28, 0]);
        }
        for offset in [18, 54, 56, 120, 576, 608, 624, 632, 648, 516, 520, 536, 540] {
            let mut b = valid.clone();
            w32(&mut b, offset, u32::MAX);
            assert!(audit(&b, false, [2, 28, 0]).is_err(), "offset {offset}");
        }
        let mut b = valid;
        w64(&mut b, 576, 29);
        assert!(audit(&b, false, [2, 28, 0]).is_err());
    }
}
