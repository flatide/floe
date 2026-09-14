//! Offline development packager; existing vendored signal dependencies only.
//! Only its own new staging directory is removed. Never replace an archive.
mod elf;
use std::{
    env, fs,
    io::{self, Read, Write},
    os::fd::AsRawFd,
    os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const BINS: [&str; 3] = ["floe2-web", "floe-index", "floe-renderd"];
static CANCEL: OnceLock<Arc<AtomicUsize>> = OnceLock::new();
fn cancelled() -> bool {
    CANCEL.get().is_some_and(|c| c.load(Ordering::Relaxed) != 0)
}
fn check_cancelled() -> Result<()> {
    if cancelled() {
        Err("packaging cancelled before publication".into())
    } else {
        Ok(())
    }
}
const HELP: &str = "Usage: sh tools/make_web_portable.sh --out NEW.tar.gz
    --target x86_64-unknown-linux-gnu|x86_64-unknown-linux-musl
    [--jobs 1..16] [--glibc-max 2.28] [--extra-notices DIR]

Build matched Rust binaries offline with the installed toolchain (RUSTUP_TOOLCHAIN
selects it when rustup is installed). Targets must be installed beforehand.
GNU target requires Linux x86_64; musl can be cross-built on macOS/Linux.
GNU ELF symbol requirements must not exceed --glibc-max (default 2.28).
On Linux x86_64, native selfcheck is mandatory; no skip flag. Other hosts record
runtime_checked=false and must test the resulting bundle on the deployment host.
--jobs defaults to 4; CARGO_TARGET_DIR can select a reusable build directory.
FLOE_SRC_REV optionally identifies offline source ZIPs. Existing output paths,
including symlinks, are never replaced. No downloads, browser, source/index access
or GTK package changes. Notices are an inventory, not distribution authorization.";

#[derive(Debug)]
struct Options {
    out: PathBuf,
    target: String,
    jobs: usize,
    ceiling: elf::Version,
    extra: Option<PathBuf>,
}
fn parse(args: &[String]) -> Result<Options> {
    let mut out = None;
    let mut target = None;
    let mut jobs = None;
    let mut ceiling = None;
    let mut extra = None;
    if !args.len().is_multiple_of(2) {
        return Err("each option requires one value".into());
    }
    for pair in args.chunks_exact(2) {
        match pair[0].as_str() {
            "--out" if out.is_none() => out = Some(PathBuf::from(&pair[1])),
            "--target" if target.is_none() => target = Some(pair[1].clone()),
            "--jobs" if jobs.is_none() => jobs = Some(pair[1].parse::<usize>()?),
            "--glibc-max" if ceiling.is_none() => ceiling = Some(elf::version(&pair[1])?),
            "--extra-notices" if extra.is_none() => extra = Some(PathBuf::from(&pair[1])),
            _ => return Err(format!("unknown/repeated option: {}", pair[0]).into()),
        }
    }
    let target = target.ok_or("--target is required")?;
    if !matches!(
        target.as_str(),
        "x86_64-unknown-linux-gnu" | "x86_64-unknown-linux-musl"
    ) {
        return Err("unsupported target".into());
    }
    let jobs = jobs.unwrap_or(4);
    if !(1..=16).contains(&jobs) {
        return Err("--jobs must be 1..16".into());
    }
    let out = out.ok_or("--out is required")?;
    if !out.to_string_lossy().ends_with(".tar.gz") {
        return Err("--out must name a new .tar.gz file".into());
    }
    Ok(Options {
        out,
        target,
        jobs,
        ceiling: ceiling.unwrap_or([2, 28, 0]),
        extra,
    })
}
fn output(command: &mut Command) -> Result<String> {
    check_cancelled()?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let mut stdout = child.stdout.take().ok_or("missing stdout")?;
    let mut stderr = child.stderr.take().ok_or("missing stderr")?;
    let mut status = None;
    let result = (|| -> Result<String> {
        for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
            // SAFETY: both descriptors are borrowed from live, exclusively
            // owned child pipes. Only file status flags are read/modified.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(io::Error::last_os_error().into());
            }
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let (mut out_eof, mut err_eof) = (false, false);
        loop {
            check_cancelled()?;
            out_eof |= drain(&mut stdout, &mut out)?;
            err_eof |= drain(&mut stderr, &mut err)?;
            if status.is_none() {
                status = child.try_wait()?;
            }
            if let Some(status) = status {
                if !status.success() {
                    return Err(
                        format!("{command:?} failed: {}", String::from_utf8_lossy(&err)).into(),
                    );
                }
                if out_eof && err_eof {
                    return Ok(String::from_utf8(out)?);
                }
            }
            if Instant::now() >= deadline {
                return Err(format!("{command:?} metadata/EOF timed out").into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    })();
    if result.is_err() && status.is_none() {
        kill_unreaped(&mut child);
    }
    result
}
fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>) -> Result<bool> {
    let mut block = [0; 16 * 1024];
    match pipe.read(&mut block) {
        Ok(0) => return Ok(true),
        Ok(n) => bytes.extend_from_slice(&block[..n]),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) => {}
        Err(e) => return Err(e.into()),
    }
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("oversized build metadata response".into());
    }
    Ok(false)
}
fn kill_unreaped(child: &mut std::process::Child) {
    // SAFETY: this PGID was created by process_group(0). Caller has not reaped
    // the leader, so its PID cannot have been reused, even if it just exited.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
}
fn run(command: &mut Command) -> Result<()> {
    check_cancelled()?;
    let mut child = command.stdin(Stdio::null()).process_group(0).spawn()?;
    loop {
        if cancelled() {
            kill_unreaped(&mut child);
            return check_cancelled();
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(format!("{command:?} failed").into()),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                kill_unreaped(&mut child);
                return Err(e.into());
            }
        }
    }
}
fn revision(root: &Path) -> Result<String> {
    if let Some(value) = env::var_os("FLOE_SRC_REV") {
        let value = value
            .into_string()
            .map_err(|_| "FLOE_SRC_REV must be ASCII")?;
        validate_stamp(&value)?;
        return Ok(value);
    }
    let mut git = Command::new("git");
    git.arg("-C").arg(root);
    let top = output(git.args(["rev-parse", "--show-toplevel"]));
    if !root.join(".git").exists()
        || top
            .ok()
            .and_then(|s| fs::canonicalize(s.trim()).ok())
            .as_deref()
            != Some(root)
    {
        return Ok("unknown".into());
    }
    let head = output(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "HEAD"]),
    )?;
    let dirty = output(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["status", "--porcelain"]),
    )?;
    let stamp = format!("{}{}", head.trim(), if dirty.is_empty() { "" } else { "+" });
    validate_stamp(&stamp)?;
    Ok(stamp)
}
fn validate_stamp(s: &str) -> Result<()> {
    if s.is_empty()
        || s.len() > 128
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
    {
        return Err("FLOE_SRC_REV must be 1..128 ASCII letters/digits/._-+".into());
    }
    Ok(())
}
struct Stage(PathBuf);
impl Stage {
    fn new(parent: &Path) -> Result<Self> {
        for n in 0..100 {
            let path = parent.join(format!(".floe-web-stage-{}-{n}", std::process::id()));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err("could not reserve a new private staging directory".into())
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_dir_all(&self.0) {
            eprintln!("warning: cannot remove own stage {}: {e}", self.0.display());
        }
    }
}
fn new_output(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        env::current_dir()?.join(path)
    };
    let parent = absolute
        .parent()
        .ok_or("output has no parent")?
        .canonicalize()?;
    let path = parent.join(absolute.file_name().ok_or("output has no filename")?);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(path),
        Ok(_) => Err(format!("output already exists (not replaced): {}", path.display()).into()),
        Err(e) => Err(e.into()),
    }
}
fn regular(path: &Path) -> Result<fs::Metadata> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_file() {
        return Err(format!("expected regular file, not symlink: {}", path.display()).into());
    }
    Ok(m)
}
fn copy(source: &Path, destination: &Path, executable: bool) -> Result<()> {
    check_cancelled()?;
    let m = regular(source)?;
    if m.len() > 128 * 1024 * 1024 {
        return Err(format!("oversized bundle input: {}", source.display()).into());
    }
    fs::create_dir_all(destination.parent().ok_or("destination has no parent")?)?;
    fs::copy(source, destination)?;
    fs::set_permissions(
        destination,
        fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
    )?;
    Ok(())
}
fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut todo = vec![root.to_owned()];
    while let Some(directory) = todo.pop() {
        check_cancelled()?;
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                todo.push(entry.path());
            } else if kind.is_file() {
                found.push(entry.path());
            } else {
                return Err(format!("non-regular bundle input: {}", entry.path().display()).into());
            }
            if found.len() + todo.len() > 100_000 {
                return Err("bundle inventory too large".into());
            }
        }
    }
    found.sort();
    Ok(found)
}
fn dependency_notices(
    root: &Path,
    cargo: &Path,
    rustc: &Path,
    target: &str,
) -> Result<Vec<PathBuf>> {
    let metadata: serde_json::Value = serde_json::from_str(&output(
        Command::new(cargo)
            .current_dir(root.join("rust"))
            .env("RUSTC", rustc)
            .args([
                "metadata",
                "--offline",
                "--locked",
                "--format-version",
                "1",
                "--filter-platform",
                target,
            ]),
    )?)?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("missing cargo packages")?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .ok_or("missing cargo dependency graph")?;
    let mut selected = std::collections::BTreeSet::new();
    let mut todo = Vec::new();
    for name in ["floe-app", "floe-index", "floe-renderd"] {
        let package = packages
            .iter()
            .find(|p| p["name"] == name && p["source"].is_null())
            .ok_or("missing native root package")?;
        todo.push(
            package["id"]
                .as_str()
                .ok_or("missing package id")?
                .to_owned(),
        );
    }
    while let Some(id) = todo.pop() {
        if !selected.insert(id.clone()) {
            continue;
        }
        let node = nodes
            .iter()
            .find(|n| n["id"] == id)
            .ok_or("missing dependency node")?;
        for dep in node["deps"].as_array().ok_or("missing node dependencies")? {
            let kinds = dep["dep_kinds"]
                .as_array()
                .ok_or("missing dependency kinds")?;
            if kinds.iter().any(|k| k["kind"] != "dev") {
                todo.push(
                    dep["pkg"]
                        .as_str()
                        .ok_or("missing dependency id")?
                        .to_owned(),
                );
            }
        }
    }
    let vendor = root.join("rust/vendor").canonicalize()?;
    let mut manifests = Vec::new();
    for package in packages {
        if !selected.contains(package["id"].as_str().ok_or("missing package id")?)
            || package["source"].is_null()
        {
            continue;
        }
        let manifest = PathBuf::from(
            package["manifest_path"]
                .as_str()
                .ok_or("missing manifest path")?,
        )
        .canonicalize()?;
        if !manifest.starts_with(&vendor) {
            return Err("non-vendored package in offline closure".into());
        }
        manifests.push(
            manifest
                .parent()
                .ok_or("manifest has no parent")?
                .to_owned(),
        );
    }
    manifests.sort();
    manifests.dedup();
    Ok(manifests)
}
fn notices(
    root: &Path,
    sysroot: &Path,
    extra: Option<&Path>,
    dest: &Path,
    crates: &[PathBuf],
) -> Result<()> {
    let mut remaining = 128_u64 * 1024 * 1024;
    let mut notice_copy = |source: &Path, target: &Path| -> Result<()> {
        remaining = remaining
            .checked_sub(regular(source)?.len())
            .ok_or("notice inventory exceeds 128MiB")?;
        copy(source, target, false)
    };
    let vendor = root.join("rust/vendor");
    let mut inventory =
        String::from("Resolved native/build dependency notices (build dependencies included; not all are runtime-linked)\n");
    for crate_dir in crates {
        let mut notices = 0;
        for file in files(crate_dir)? {
            let name = file
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_ascii_uppercase();
            if name == "CARGO.TOML" && file.parent() == Some(crate_dir.as_path())
                || file.strip_prefix(crate_dir)?.components().any(|c| {
                    c.as_os_str()
                        .to_string_lossy()
                        .eq_ignore_ascii_case("licenses")
                })
                || [
                    "LICENSE",
                    "LICENCE",
                    "NOTICE",
                    "COPYING",
                    "COPYRIGHT",
                    "UNLICENSE",
                ]
                .iter()
                .any(|p| name.starts_with(p))
            {
                let rel = file.strip_prefix(&vendor)?;
                notice_copy(&file, &dest.join("crates").join(rel))?;
                if name != "CARGO.TOML" {
                    notices += 1;
                }
            }
        }
        if notices == 0 {
            return Err(format!("missing notice: {}", crate_dir.display()).into());
        }
        inventory.push_str(&format!(
            "{}: {notices} notice file(s)\n",
            crate_dir
                .file_name()
                .ok_or("crate without name")?
                .to_string_lossy()
        ));
    }
    notice_copy(&root.join("rust/Cargo.lock"), &dest.join("Cargo.lock"))?;
    notice_copy(
        &root.join("rust/Cargo.toml"),
        &dest.join("workspace.Cargo.toml"),
    )?;
    notice_copy(
        &root.join("rust/render-core/assets/NotoSansMono-OFL.txt"),
        &dest.join("font/NotoSansMono-OFL.txt"),
    )?;
    let rust_docs = sysroot.join("share/doc/rust");
    for name in ["COPYRIGHT.html", "COPYRIGHT-library.html"] {
        notice_copy(
            &rust_docs.join(name),
            &dest.join("rust-toolchain").join(name),
        )
        .map_err(|e| {
            format!("installed toolchain copyright documents required (no download): {e}")
        })?;
    }
    let licenses = rust_docs.join("licenses");
    for file in files(&licenses)? {
        notice_copy(
            &file,
            &dest
                .join("rust-toolchain/licenses")
                .join(file.strip_prefix(&licenses)?),
        )?;
    }
    if let Some(extra) = extra {
        for file in files(extra)? {
            notice_copy(&file, &dest.join("extra").join(file.strip_prefix(extra)?))?;
        }
        inventory.push_str(
            "Extra notices: explicitly supplied build inputs, not independently certified.\n",
        );
    }
    inventory.push_str("Rust toolchain documents may include components not linked into these executables.\nInventory is not legal clearance or a grant of Floe distribution rights.\n");
    fs::write(dest.join("INVENTORY.txt"), inventory)?;
    Ok(())
}
fn sha256(path: &Path) -> Result<String> {
    let line = match output(Command::new("sha256sum").arg(path)) {
        Err(e)
            if e.downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == io::ErrorKind::NotFound) =>
        {
            output(Command::new("shasum").args(["-a", "256"]).arg(path))?
        }
        other => other?,
    };
    let hash = line.split_whitespace().next().ok_or("missing SHA-256")?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid SHA-256 output".into());
    }
    Ok(hash.to_ascii_lowercase())
}
fn checksums(root: &Path) -> Result<()> {
    let mut text = String::new();
    for file in files(root)? {
        let relative = file
            .strip_prefix(root)?
            .to_str()
            .ok_or("non-UTF8 bundle path")?;
        if relative.contains(['\n', '\r', '\\']) {
            return Err("invalid checksum filename".into());
        }
        text.push_str(&format!("{}  {relative}\n", sha256(&file)?));
    }
    fs::write(root.join("SHA256SUMS"), text)?;
    Ok(())
}
fn build(root: &Path, o: Options) -> Result<()> {
    let out = new_output(&o.out)?;
    let root = root.canonicalize()?;
    let native = env::consts::OS == "linux" && env::consts::ARCH == "x86_64";
    if o.target.ends_with("-gnu") && !native {
        return Err("GNU packaging requires a Linux x86_64 build host".into());
    }
    let rustc =
        PathBuf::from(env::var_os("FLOE_PACKAGER_RUSTC").ok_or("missing installed rustc path")?);
    let cargo =
        PathBuf::from(env::var_os("FLOE_PACKAGER_CARGO").ok_or("missing installed cargo path")?);
    let compiler = output(Command::new(&rustc).arg("-Vv"))?;
    let sysroot = PathBuf::from(output(Command::new(&rustc).args(["--print", "sysroot"]))?.trim());
    if !sysroot
        .join("lib/rustlib")
        .join(&o.target)
        .join("lib")
        .is_dir()
    {
        return Err(format!(
            "target {} is not installed; supply an offline toolchain first",
            o.target
        )
        .into());
    }
    let stamp = revision(&root)?;
    let target_dir = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("rust/target/web-portable"));
    let target_dir = if target_dir.is_absolute() {
        target_dir
    } else {
        env::current_dir()?.join(target_dir)
    };
    let stage = Stage::new(out.parent().unwrap())?;
    let bundle = stage.0.join("floe2-web-portable");
    fs::create_dir(&bundle)?;
    // Validate notice availability before spending time compiling. Never copy
    // user datasets, target caches, private config, or an entire source checkout.
    let crates = dependency_notices(&root, &cargo, &rustc, &o.target)?;
    notices(
        &root,
        &sysroot,
        o.extra.as_deref(),
        &bundle.join("NOTICES"),
        &crates,
    )?;
    eprintln!(
        "building offline: {} / source {stamp} / {} jobs",
        o.target, o.jobs
    );
    run(Command::new(cargo)
        .current_dir(root.join("rust"))
        .env("RUSTC", &rustc)
        .env("FLOE_SRC_REV", &stamp)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .args([
            "build",
            "--offline",
            "--locked",
            "--release",
            "--target",
            &o.target,
            "--jobs",
            &o.jobs.to_string(),
            "-p",
            "floe-app",
            "-p",
            "floe-index",
            "-p",
            "floe-renderd",
        ]))?;
    let binaries = target_dir.join(&o.target).join("release");
    let mut audit = String::new();
    for name in BINS {
        let dest = bundle.join(name);
        copy(&binaries.join(name), &dest, true)?;
        audit.push_str(&format!(
            "{name}: {}\n",
            elf::audit(&fs::read(&dest)?, o.target.ends_with("-musl"), o.ceiling)?
        ));
    }
    fs::write(bundle.join("ELF.txt"), audit)?;
    fs::write(
        bundle.join("README.txt"),
        include_str!("../../../tools/web-portable/README.txt"),
    )?;
    fs::write(
        bundle.join("verify.sh"),
        include_str!("../../../tools/web-portable/verify.sh"),
    )?;
    if native {
        // No inherited TMPDIR workaround: invalid wire paths must fail, exactly
        // as they would in the deployed executable. No source/browser is opened.
        run(Command::new(bundle.join("floe2-web")).args(["selfcheck", "--adjacent"]))?;
    }
    fs::write(bundle.join("BUILD.txt"), format!(
        "format=1\nproduct=floe2-web-portable-preview\ntarget={}\nsource_revision={stamp}\nruntime_checked={native}\ndesktop_acceptance=unverified\npython_runtime=false\n\n{compiler}\n",
        o.target))?;
    checksums(&bundle)?;
    run(Command::new("sh").arg(bundle.join("verify.sh")))?;
    let archive = stage.0.join("payload.tar.gz");
    run(Command::new("tar")
        .current_dir(&stage.0)
        .env("COPYFILE_DISABLE", "1")
        .env_remove("TAR_OPTIONS")
        .args(["-czf", "payload.tar.gz", "--", "floe2-web-portable"]))?;
    fs::File::open(&archive)?.sync_all()?;
    let digest = sha256(&archive)?;
    // Same-filesystem link is an atomic no-clobber commit even if another
    // packager races us. A failed sync after link is published, not rollback.
    check_cancelled()?;
    fs::hard_link(&archive, &out)?;
    if let Err(e) = fs::File::open(out.parent().unwrap()).and_then(|f| f.sync_all()) {
        eprintln!("warning: archive published; parent directory sync failed: {e}");
    }
    let _ =
        writeln!(io::stdout(),
        "PUBLISHED {}\nSHA256 {digest}\nruntime_checked={native}; desktop_acceptance=unverified",
        out.display()
    );
    Ok(())
}
fn main() {
    let cancellation = Arc::new(AtomicUsize::new(0));
    CANCEL.set(Arc::clone(&cancellation)).unwrap();
    let mut handlers = Vec::new();
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        match signal_hook::flag::register_usize(signal, Arc::clone(&cancellation), signal as usize)
        {
            Ok(id) => handlers.push(id),
            Err(e) => {
                eprintln!("cannot install cancellation handler: {e}");
                std::process::exit(1);
            }
        }
    }
    let args: Vec<String> = env::args().collect();
    let result = (|| -> Result<()> {
        let root = args.get(1).ok_or("missing repository path")?;
        if args.get(2).is_some_and(|s| s == "--help" || s == "-h") && args.len() == 3 {
            println!("{HELP}");
            return Ok(());
        }
        build(Path::new(root), parse(&args[2..])?)
    })();
    if let Err(error) = result {
        let _ = writeln!(io::stderr(), "web portable: {error}");
        let signal = cancellation.load(Ordering::Relaxed);
        std::process::exit(if signal != 0 { 128 + signal as i32 } else { 1 });
    }
    for id in handlers {
        signal_hook::low_level::unregister(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options(s: &[&str]) -> Result<Options> {
        parse(&s.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
    #[test]
    fn strict_options() {
        let valid = [
            "--out",
            "new.tar.gz",
            "--target",
            "x86_64-unknown-linux-musl",
        ];
        assert_eq!(options(&valid).unwrap().jobs, 4);
        for tail in [
            vec!["--force", "yes"],
            vec!["--jobs", "0"],
            vec!["--jobs", "17"],
            vec!["--glibc-max", "2.28.bad"],
            vec!["--out", "twice.tar.gz"],
        ] {
            let mut args = valid.to_vec();
            args.extend(tail);
            assert!(options(&args).is_err());
        }
        assert!(options(&[]).is_err());
        assert!(options(&["--out"]).is_err());
    }
    #[test]
    fn source_stamp_is_not_shell_text() {
        for value in ["", "a b", "x\ny", "$(command)", "한글"] {
            assert!(validate_stamp(value).is_err());
        }
        assert!(validate_stamp("0ec42e5+offline-r1").is_ok());
    }
    #[test]
    fn new_output_and_atomic_link_never_replace_existing_entries() {
        let stage = Stage::new(&env::temp_dir()).unwrap();
        let source = stage.0.join("source");
        fs::write(&source, b"new archive").unwrap();
        let output = stage.0.join("output.tar.gz");
        assert_eq!(
            new_output(&output).unwrap(),
            stage.0.canonicalize().unwrap().join("output.tar.gz")
        );
        fs::hard_link(&source, &output).unwrap();
        assert!(new_output(&output).is_err());
        assert!(fs::hard_link(&source, &output).is_err());
        assert_eq!(fs::read(&output).unwrap(), b"new archive");
        let dangling = stage.0.join("dangling.tar.gz");
        std::os::unix::fs::symlink(stage.0.join("absent"), &dangling).unwrap();
        assert!(new_output(&dangling).is_err());
        assert!(regular(&dangling).is_err());
    }
    #[test]
    fn stage_drop_removes_only_its_own_directory() {
        let parent = Stage::new(&env::temp_dir()).unwrap();
        let keep = parent.0.join("keep");
        fs::write(&keep, b"untouched").unwrap();
        let nested = Stage::new(&parent.0).unwrap();
        let path = nested.0.clone();
        fs::write(path.join("scratch"), b"temp").unwrap();
        drop(nested);
        assert!(!path.exists());
        assert_eq!(fs::read(keep).unwrap(), b"untouched");
    }
}
