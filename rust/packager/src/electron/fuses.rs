//! Offline fuse hardening of the checksum-verified, private runtime copy only.
//! Wire schema: https://www.electronjs.org/docs/latest/tutorial/fuses
//! No npm/runtime execution, OS policy changes, or mutation of the input zip.
use super::*;
use std::io::{Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;

const SENTINEL: &[u8] = b"dL7pKGdnNz796PbbjQWNKmHXBZaB9tsX";
const CHUNK: usize = 1024 * 1024;
// Pinned Electron 44.4.3, fuse v1 indices 0..8. Reject schema/default drift.
const UPSTREAM: &[u8; 9] = b"101100011";
// Disable RunAsNode, NodeOptions, NodeCliInspect and FileProtocolExtraPrivileges.
// Keep cookie/ASAR/snapshot defaults and WASM trap handling unchanged.
const HARDENED: &[u8; 9] = b"000000001";
const POLICY: &str = "floe-electron-fuses-v1";

fn relative(target: &str) -> Result<&'static str> {
    match target {
        "aarch64-apple-darwin" | "x86_64-apple-darwin" => Ok(
            "Electron.app/Contents/Frameworks/Electron Framework.framework/Versions/A/Electron Framework",
        ),
        "x86_64-unknown-linux-gnu" => Ok("electron"),
        _ => Err("unsupported fuse target".into()),
    }
}
fn open(runtime: &Path, target: &str, write: bool) -> Result<fs::File> {
    let runtime = runtime.canonicalize()?;
    let path = runtime.join(relative(target)?);
    if !path.canonicalize()?.starts_with(&runtime) || regular(&path)?.len() > 1024 * 1024 * 1024 {
        return Err("invalid fuse binary".into());
    }
    Ok(fs::OpenOptions::new()
        .read(true)
        .write(write)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?)
}

fn wire(file: &mut fs::File) -> Result<(u64, [u8; 9])> {
    file.seek(SeekFrom::Start(0))?;
    let mut buffer = vec![0; CHUNK + SENTINEL.len() - 1];
    let (mut carry, mut consumed, mut found) = (0, 0u64, None);
    loop {
        check_cancelled()?;
        let n = file.read(&mut buffer[carry..carry + CHUNK])?;
        if n == 0 {
            break;
        }
        let size = carry + n;
        for (i, bytes) in buffer[..size].windows(SENTINEL.len()).enumerate() {
            if bytes == SENTINEL {
                if found.is_some() {
                    return Err("ambiguous Electron fuse sentinel".into());
                }
                found = Some(consumed - carry as u64 + i as u64 + SENTINEL.len() as u64);
            }
        }
        consumed += n as u64;
        carry = size.min(SENTINEL.len() - 1);
        buffer.copy_within(size - carry..size, 0);
    }
    let header = found.ok_or("Electron fuse sentinel missing")?;
    file.seek(SeekFrom::Start(header))?;
    let mut bytes = [0; 11];
    file.read_exact(&mut bytes)?;
    if bytes[..2] != [1, 9] || bytes[2..].iter().any(|b| !matches!(b, b'0' | b'1')) {
        return Err("unsupported Electron fuse schema/state; review runtime upgrade".into());
    }
    Ok((header + 2, bytes[2..].try_into().unwrap()))
}

fn flip(file: &mut fs::File) -> Result<()> {
    let (offset, before) = wire(file)?;
    if &before != UPSTREAM && &before != HARDENED {
        return Err("unexpected pinned Electron fuse defaults".into());
    }
    check_cancelled()?;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(HARDENED)?;
    file.sync_all()?;
    if &wire(file)?.1 != HARDENED {
        return Err("Electron fuse write verification failed".into());
    }
    Ok(())
}

pub(super) fn record() -> Value {
    json!({"policy":POLICY,"schema":1,"wire":std::str::from_utf8(HARDENED).unwrap()})
}
pub(super) fn verify(runtime: &Path, target: &str) -> Result<()> {
    if &wire(&mut open(runtime, target, false)?)?.1 != HARDENED {
        return Err("Electron runtime does not satisfy the bundle fuse policy".into());
    }
    Ok(())
}
pub(super) fn harden(runtime: &Path, target: &str) -> Result<()> {
    flip(&mut open(runtime, target, true)?)?;
    if target.ends_with("-apple-darwin") {
        // Fuse edits invalidate the upstream signature. Only our new staging
        // copy is ad-hoc re-signed; no identity, Keychain, notarization or trust
        // settings. Preserve helper JIT entitlements/flags as upstream fuses do.
        let app = runtime.join("Electron.app");
        run(Command::new("/usr/bin/codesign")
            .args([
                "--sign",
                "-",
                "--force",
                "--preserve-metadata=entitlements,requirements,flags,runtime",
                "--deep",
            ])
            .arg(&app))?;
        run(Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(app))?;
    }
    verify(runtime, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(prefix: usize, state: &[u8]) -> Vec<u8> {
        let mut bytes = vec![b'x'; prefix];
        bytes.extend_from_slice(SENTINEL);
        bytes.extend_from_slice(&[1, 9]);
        bytes.extend_from_slice(state);
        bytes
    }
    #[test]
    fn bounded_scan_crosses_chunks_and_only_changes_the_four_fuses() {
        let stage = Stage::new(&env::temp_dir()).unwrap();
        let p = stage.0.join("binary");
        for prefix in [0, CHUNK - 16, CHUNK + 7] {
            let before = fixture(prefix, UPSTREAM);
            fs::write(&p, &before).unwrap();
            let mut f = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&p)
                .unwrap();
            flip(&mut f).unwrap();
            flip(&mut f).unwrap();
            let mut expected = before;
            expected[prefix + SENTINEL.len() + 2..].copy_from_slice(HARDENED);
            assert_eq!(fs::read(&p).unwrap(), expected);
        }
    }
    #[test]
    fn malformed_or_unreviewed_wires_fail_without_mutation() {
        let stage = Stage::new(&env::temp_dir()).unwrap();
        let p = stage.0.join("binary");
        let valid = fixture(3, UPSTREAM);
        let mut version = valid.clone();
        version[3 + SENTINEL.len()] = 2;
        let mut length = valid.clone();
        length[4 + SENTINEL.len()] = 10;
        for before in [
            b"missing".to_vec(),
            valid[..valid.len() - 1].to_vec(),
            [valid.as_slice(), valid.as_slice()].concat(),
            version,
            length,
            fixture(0, b"r01100011"),
            fixture(0, b"111100011"),
        ] {
            fs::write(&p, &before).unwrap();
            let mut f = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&p)
                .unwrap();
            assert!(flip(&mut f).is_err());
            assert_eq!(fs::read(&p).unwrap(), before);
        }
    }
}
