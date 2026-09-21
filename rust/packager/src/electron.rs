//! Native-only, offline development comparison. Never rebrand the upstream
//! runtime, infer GUI acceptance, or silently relax the Rust ELF contract.
use super::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const HELP: &str = "Usage: sh tools/make_electron_portable.sh
    --runtime-zip /path/to/pinned-electron.zip --out NEW.tar.gz [--jobs 1..16]

Native macOS arm64/x64 or Linux x64 builds only, installed offline toolchain.
No downloads, browser launch, design access, signing, security changes or overwrite.
Runtime archive MUST match electron/runtime.json for this compiler's host target.
Builds matched release Rust workers with at most 16 jobs (default 4).
Linux Rust workers require GLIBC <= 2.28. Chromium/ETX acceptance remains separate.
CARGO_TARGET_DIR may select a reusable native build directory.
Inspect the result with sh BUNDLE/verify.sh; checksums are not signatures.";

const APP_FILES: &[&str] = &[
    "electron/package.json",
    "electron/runtime.json",
    "electron/main.cjs",
    "electron/service-client.cjs",
    "electron/policy.cjs",
    "electron/close-controller.cjs",
    "electron/recovery-controller.cjs",
    "electron/clipboard-controller.cjs",
    "electron/termination-signals.cjs",
    "electron/downloads.cjs",
    "electron/download-slot.cjs",
    "electron/readiness-qa.cjs",
    "electron/layout-qa.cjs",
    "electron/clipboard-qa.cjs",
    "electron/recovery-qa.cjs",
    "desktop/ui/menu-action.js",
    "desktop/ui/recovery-status.js",
    "desktop/ui/frame-parity-probe.js",
    "rust/web/ui/protocol.js",
];
const WORKERS: &[(&str, &str)] = &[
    ("floe-index", "rust/target/release/floe-index"),
    ("floe-renderd", "rust/target/release/floe-renderd"),
    (
        "floe-electron-service",
        "electron/service/target/release/floe-electron-service",
    ),
    (
        "floe-electron-download",
        "electron/service/target/release/floe-electron-download",
    ),
];

struct Options {
    archive: PathBuf,
    out: PathBuf,
    jobs: usize,
}
fn parse(args: &[String]) -> Result<Options> {
    if !args.len().is_multiple_of(2) {
        return Err("each option requires one value".into());
    }
    let (mut archive, mut out, mut jobs) = (None, None, None);
    for pair in args.chunks_exact(2) {
        match pair[0].as_str() {
            "--runtime-zip" if archive.is_none() => archive = Some(PathBuf::from(&pair[1])),
            "--out" if out.is_none() => out = Some(PathBuf::from(&pair[1])),
            "--jobs" if jobs.is_none() => jobs = Some(pair[1].parse::<usize>()?),
            _ => return Err(format!("unknown/repeated option: {}", pair[0]).into()),
        }
    }
    let jobs = jobs.unwrap_or(4);
    if !(1..=16).contains(&jobs) {
        return Err("--jobs must be 1..16".into());
    }
    let out = out.ok_or("--out is required")?;
    if !out.to_string_lossy().ends_with(".tar.gz") {
        return Err("--out must end in .tar.gz".into());
    }
    Ok(Options {
        archive: archive.ok_or("--runtime-zip is required")?,
        out,
        jobs,
    })
}

fn platform(target: &str) -> Result<(&'static str, &'static str)> {
    match target {
        "aarch64-apple-darwin" => Ok(("darwin-arm64", "Electron.app/Contents/MacOS/Electron")),
        "x86_64-apple-darwin" => Ok(("darwin-x64", "Electron.app/Contents/MacOS/Electron")),
        "x86_64-unknown-linux-gnu" => Ok(("linux-x64", "electron")),
        _ => Err("comparison bundle requires native macOS arm64/x64 or Linux x64 GNU".into()),
    }
}

fn archive_pin(config: &Value, platform: &str) -> Result<String> {
    let version = config["version"]
        .as_str()
        .ok_or("missing runtime version")?;
    if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err("invalid pinned runtime version".into());
    }
    let name = format!("electron-v{version}-{platform}.zip");
    let pin = config["sha256"][&name]
        .as_str()
        .ok_or("missing pinned native runtime hash")?;
    if pin.len() != 64 || !pin.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid runtime SHA-256".into());
    }
    Ok(pin.to_ascii_lowercase())
}

fn safe_name(path: &Path) -> Result<String> {
    let s = path.to_str().ok_or("non-UTF8 inventory name")?;
    if s.is_empty()
        || s.contains('\\')
        || s.chars().any(char::is_control)
        || path
            .components()
            .any(|p| !matches!(p, std::path::Component::Normal(_)))
    {
        return Err("invalid inventory name".into());
    }
    Ok(s.to_owned())
}

/// Enumerate without following directory links. Framework links must be relative,
/// resolve within this root and remain links in both the tar and the inventory.
/// This detects accidental changes; it is not an adversarial mutable-tree reader.
fn inventory(root: &Path) -> Result<BTreeMap<String, Value>> {
    let root = root.canonicalize()?;
    let mut result = BTreeMap::new();
    let mut todo = vec![root.clone()];
    let mut bytes = 0_u64;
    while let Some(dir) = todo.pop() {
        check_cancelled()?;
        for item in fs::read_dir(dir)? {
            let item = item?;
            let path = item.path();
            let name = safe_name(path.strip_prefix(&root)?)?;
            if name == "BUNDLE.json" {
                continue;
            }
            if result.len() >= 100_000 {
                return Err("inventory exceeds 100000 entries".into());
            }
            let meta = fs::symlink_metadata(&path)?;
            let mode = meta.permissions().mode() & 0o7777;
            let entry = if meta.file_type().is_symlink() {
                let target = fs::read_link(&path)?;
                if target.is_absolute() || !path.canonicalize()?.starts_with(&root) {
                    return Err("runtime link escapes bundle".into());
                }
                let target = target
                    .to_str()
                    .filter(|s| !s.chars().any(char::is_control))
                    .ok_or("invalid symlink target")?;
                // Symlink mode is not portable (Darwin 755, Linux 777).
                json!({"kind":"symlink", "target":target})
            } else {
                if mode & 0o7000 != 0 {
                    return Err("special permission bits in bundle".into());
                }
                if meta.is_dir() {
                    todo.push(path);
                    json!({"kind":"directory", "mode":mode})
                } else if meta.is_file() {
                    bytes = bytes
                        .checked_add(meta.len())
                        .ok_or("inventory bytes overflow")?;
                    if bytes > 4 * 1024 * 1024 * 1024 {
                        return Err("bundle exceeds 4GiB".into());
                    }
                    json!({"kind":"file", "mode":mode, "bytes":meta.len(), "sha256":sha256(&path)?})
                } else {
                    return Err("special file in bundle".into());
                }
            };
            result.insert(name, entry);
        }
    }
    Ok(result)
}

pub(super) fn verify(root: &Path) -> Result<()> {
    let record = root.join("BUNDLE.json");
    if regular(&record)?.len() > 16 * 1024 * 1024 {
        return Err("oversized bundle record".into());
    }
    let manifest: Value = serde_json::from_slice(&fs::read(record)?)?;
    if manifest["format"] != 1 || manifest["product"] != "floe2-electron-comparison" {
        return Err("unsupported bundle record".into());
    }
    let current = inventory(root)?;
    if serde_json::to_value(&current)? != manifest["files"] {
        return Err(
            "bundle inventory mismatch (missing/extra/changed content, mode or link)".into(),
        );
    }
    println!(
        "ELECTRON BUNDLE: {} entries verified; checksum integrity, not signature or GUI acceptance",
        current.len()
    );
    Ok(())
}

pub(super) fn build(root: &Path, args: &[String]) -> Result<()> {
    if args == ["--help"] || args == ["-h"] {
        println!("{HELP}");
        return Ok(());
    }
    let o = parse(args)?;
    let out = new_output(&o.out)?;
    let root = root.canonicalize()?;
    let rustc = PathBuf::from(env::var_os("FLOE_PACKAGER_RUSTC").ok_or("missing installed rustc")?);
    let cargo = PathBuf::from(env::var_os("FLOE_PACKAGER_CARGO").ok_or("missing installed cargo")?);
    let compiler = output(Command::new(&rustc).arg("-Vv"))?;
    let target = compiler
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .ok_or("missing compiler host")?;
    let (platform, runtime_path) = platform(target)?;
    let sysroot = PathBuf::from(output(Command::new(&rustc).args(["--print", "sysroot"]))?.trim());
    let stamp = revision(&root)?;
    let stage = Stage::new(out.parent().unwrap())?;
    let bundle = stage.0.join("floe2-electron-comparison");
    fs::create_dir(&bundle)?;
    for name in APP_FILES {
        copy(&root.join(name), &bundle.join(name), false)?;
    }
    let config: Value = serde_json::from_slice(&fs::read(bundle.join("electron/runtime.json"))?)?;
    let pin = archive_pin(&config, platform)?;
    // Hash the PRIVATE copy, then extract that same file. Never unpack an
    // unchecked caller-supplied archive, even if its name looks official.
    if regular(&o.archive)?.len() > 1024 * 1024 * 1024 {
        return Err("runtime zip exceeds 1GiB".into());
    }
    let zip = stage.0.join("runtime.zip");
    fs::copy(&o.archive, &zip)?;
    if sha256(&zip)? != pin {
        return Err("runtime archive SHA-256 does not match pinned native release".into());
    }
    let runtime = bundle.join("runtime");
    fs::create_dir(&runtime)?;
    run(Command::new("unzip")
        .env_remove("UNZIP")
        .env_remove("UNZIPOPT")
        .args(["-q"])
        .arg(&zip)
        .arg("-d")
        .arg(&runtime))?;
    // The checked official archive may contain internal Framework symlinks.
    inventory(&runtime)?;
    for name in [runtime_path, "LICENSE", "LICENSES.chromium.html"] {
        if regular(&runtime.join(name))?.len() == 0 {
            return Err("empty runtime input".into());
        }
    }

    let mut crates = dependency_notices(&root, &cargo, &rustc, target)?;
    crates.extend(dependency_notices_for(
        &root,
        &cargo,
        &rustc,
        target,
        "electron/service",
        &["floe-electron-service"],
    )?);
    crates.extend(dependency_notices_for(
        &root,
        &cargo,
        &rustc,
        target,
        "rust",
        &["floe-web-packager"],
    )?);
    let crates = desktop::unique_crates(crates)?;
    notices(&root, &sysroot, None, &bundle.join("NOTICES"), &crates)?;
    for name in ["Cargo.toml", "Cargo.lock"] {
        copy(
            &root.join("electron/service").join(name),
            &bundle.join("NOTICES/electron-service").join(name),
            false,
        )?;
    }
    let mut build_dir = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("rust/target/electron-portable"));
    if !build_dir.is_absolute() {
        build_dir = env::current_dir()?.join(build_dir);
    }
    for (workspace, packages) in [
        ("rust", vec!["floe-index", "floe-renderd"]),
        ("electron/service", vec!["floe-electron-service"]),
    ] {
        eprintln!(
            "Electron comparison: offline release {workspace} / {target} / {} jobs",
            o.jobs
        );
        let mut cmd = Command::new(&cargo);
        cmd.current_dir(root.join(workspace))
            .env("RUSTC", &rustc)
            .env("FLOE_SRC_REV", &stamp)
            .env("CARGO_TARGET_DIR", &build_dir)
            .env("CARGO_NET_OFFLINE", "true")
            .args([
                "build",
                "--offline",
                "--locked",
                "--release",
                "--target",
                target,
                "--jobs",
                &o.jobs.to_string(),
            ]);
        for package in packages {
            cmd.args(["-p", package]);
        }
        run(&mut cmd)?;
    }
    let binaries = build_dir.join(target).join("release");
    let mut audit = String::new();
    for (name, dest) in WORKERS {
        copy(&binaries.join(name), &bundle.join(dest), true)?;
    }
    copy(
        &env::current_exe()?,
        &bundle.join("bin/floe-bundle-check"),
        true,
    )?;
    if platform == "linux-x64" {
        for (_, dest) in WORKERS
            .iter()
            .chain(std::iter::once(&("packager", "bin/floe-bundle-check")))
        {
            audit.push_str(&format!(
                "{dest}: {}\n",
                elf::audit(&fs::read(bundle.join(dest))?, false, [2, 28, 0])?
            ));
        }
    } else {
        audit.push_str(
            "Native compiler target; macOS dynamic-library/signature acceptance is separate.\n",
        );
    }
    audit.push_str("Chromium archive SHA-256 verified; complete OS/ETX dependency audit and runtime acceptance remain unverified.\n");
    fs::write(bundle.join("NATIVE-AUDIT.txt"), audit)?;
    fs::write(
        bundle.join("floe2-electron"),
        include_str!("../../../tools/electron-portable/launch.sh")
            .replace("@RUNTIME@", runtime_path),
    )?;
    fs::set_permissions(
        bundle.join("floe2-electron"),
        fs::Permissions::from_mode(0o755),
    )?;
    fs::write(
        bundle.join("verify.sh"),
        include_str!("../../../tools/electron-portable/verify.sh"),
    )?;
    fs::write(
        bundle.join("README.txt"),
        include_str!("../../../tools/electron-portable/README.txt"),
    )?;
    fs::write(bundle.join("BUILD.txt"), format!("format=1\nproduct=floe2-electron-comparison\ntarget={target}\nsource_revision={stamp}\nworkers_profile=release\nassembler_profile=development\nruntime_archive_sha256={pin}\ngui_checked=false\nfield_acceptance=unverified\npython_runtime=false\n\n{compiler}"))?;
    let record = json!({"format":1, "product":"floe2-electron-comparison", "target":target,
        "source_revision":stamp, "runtime_archive_sha256":pin, "files":inventory(&bundle)?});
    fs::write(
        bundle.join("BUNDLE.json"),
        serde_json::to_vec_pretty(&record)?,
    )?;
    verify(&bundle)?;
    let archive = stage.0.join("payload.tar.gz");
    run(Command::new("tar")
        .current_dir(&stage.0)
        .env("COPYFILE_DISABLE", "1")
        .env_remove("TAR_OPTIONS")
        .args(["-czf", "payload.tar.gz", "--", "floe2-electron-comparison"]))?;
    fs::File::open(&archive)?.sync_all()?;
    let digest = sha256(&archive)?;
    check_cancelled()?;
    fs::hard_link(&archive, &out)?;
    if let Err(e) = fs::File::open(out.parent().unwrap()).and_then(|f| f.sync_all()) {
        eprintln!("warning: archive published; parent directory sync failed: {e}");
    }
    let _ = writeln!(
        io::stdout(),
        "PUBLISHED {}\nSHA256 {digest}\ngui_checked=false; field_acceptance=unverified",
        out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_arguments_and_native_archive_pin() {
        let args = ["--runtime-zip", "with space.zip", "--out", "new.tar.gz"];
        let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse(&strings(&args)).unwrap().jobs, 4);
        for tail in [
            &["--jobs", "0"][..],
            &["--jobs", "17"],
            &["--force", "1"],
            &["--out", "twice.tar.gz"],
            &["--jobs"],
        ] {
            assert!(parse(&strings(&[args.as_slice(), tail].concat())).is_err());
        }
        assert!(platform("aarch64-unknown-linux-gnu").is_err());
        let config = json!({"version":"44.4.3", "sha256":{"electron-v44.4.3-darwin-arm64.zip":"a".repeat(64)}});
        assert_eq!(
            archive_pin(&config, "darwin-arm64").unwrap(),
            "a".repeat(64)
        );
        assert!(archive_pin(&config, "linux-x64").is_err());
    }
    #[test]
    fn inventory_rejects_escape_special_modes_and_detects_mutation() {
        use std::os::unix::fs::symlink;
        let stage = Stage::new(&env::temp_dir()).unwrap();
        let root = stage.0.join("bundle");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("one"), b"data").unwrap();
        symlink("one", root.join("link")).unwrap();
        let first = inventory(&root).unwrap();
        assert_eq!(first["link"]["target"], "one");
        fs::write(root.join("one"), b"changed").unwrap();
        assert_ne!(first, inventory(&root).unwrap());
        fs::set_permissions(root.join("one"), fs::Permissions::from_mode(0o4755)).unwrap();
        assert!(inventory(&root).is_err());
        fs::set_permissions(root.join("one"), fs::Permissions::from_mode(0o644)).unwrap();
        symlink("../", root.join("escape")).unwrap();
        assert!(inventory(&root).is_err());
    }
    #[test]
    fn verify_rejects_missing_extra_mode_and_corrupt_manifest() {
        let stage = Stage::new(&env::temp_dir()).unwrap();
        fs::write(stage.0.join("file"), b"data").unwrap();
        let record = json!({"format":1,"product":"floe2-electron-comparison", "files":inventory(&stage.0).unwrap()});
        fs::write(
            stage.0.join("BUNDLE.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        verify(&stage.0).unwrap();
        fs::write(stage.0.join("extra"), b"unexpected").unwrap();
        assert!(verify(&stage.0).is_err());
        fs::remove_file(stage.0.join("extra")).unwrap();
        let old_mode = fs::metadata(stage.0.join("file")).unwrap().permissions();
        fs::set_permissions(stage.0.join("file"), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(verify(&stage.0).is_err());
        fs::set_permissions(stage.0.join("file"), old_mode).unwrap();
        fs::remove_file(stage.0.join("file")).unwrap();
        assert!(verify(&stage.0).is_err());
        fs::write(stage.0.join("BUNDLE.json"), b"{}").unwrap();
        assert!(verify(&stage.0).is_err());
    }
}
