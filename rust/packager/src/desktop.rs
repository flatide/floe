//! Native macOS preview inventory. Reuse the bounded portable notice index;
//! this mode neither builds/opens a UI nor signs or authorizes distribution.
use super::*;
use std::collections::BTreeMap;

pub(super) fn supplemented(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some(
            "block2-0.6.2"
                | "dispatch2-0.3.1"
                | "objc2-0.6.4"
                | "objc2-encode-4.1.0"
                | "objc2-foundation-0.3.2"
                | "objc2-app-kit-0.3.2"
                | "objc2-core-foundation-0.3.2"
                | "objc2-web-kit-0.3.2"
        )
    )
}

fn unique_crates(crates: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let mut names = BTreeMap::new();
    for path in crates {
        let name = path.file_name().ok_or("crate without name")?.to_owned();
        if names.get(&name).is_some_and(|previous| previous != &path) {
            return Err("conflicting vendored crate directories in desktop closure".into());
        }
        names.insert(name, path);
    }
    Ok(names.into_values().collect())
}

/// Local launcher receipt, not a signature or distribution authorization.
pub(super) fn receipt(root: &Path, profile: &str, app: &Path) -> Result<()> {
    if !matches!(profile, "debug" | "release") {
        return Err("invalid desktop build profile".into());
    }
    let target = root.join("desktop/target").canonicalize()?;
    let app = app.canonicalize()?;
    let preview = app.parent().ok_or("missing preview directory")?;
    let suffix = preview
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("macos-dev."))
        .ok_or("invalid preview directory")?;
    if suffix.is_empty()
        || !suffix.bytes().all(|b| b.is_ascii_alphanumeric())
        || preview.parent() != Some(target.as_path())
        || app.file_name().is_none_or(|s| s != "Floe2.app")
    {
        return Err("preview must be a new macos-dev directory in desktop/target".into());
    }
    let text = app
        .to_str()
        .filter(|s| !s.chars().any(char::is_control))
        .ok_or("invalid launcher path")?;
    regular(&app.join("Contents/MacOS/floe2-desktop"))?;
    regular(&app.join("Contents/Resources/NOTICE-INDEX.json"))?;
    let destination = target.join(format!("macos-preview-{profile}.path"));
    match fs::symlink_metadata(&destination) {
        Ok(meta) if !meta.is_file() => return Err("launcher receipt is not a regular file".into()),
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    let temporary = preview.join(".launcher-path");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    writeln!(file, "{text}")?;
    file.sync_all()?;
    check_cancelled()?;
    // rename replaces the entry itself, never follows a raced destination
    // symlink or treats a directory as an implicit copy destination.
    fs::rename(temporary, destination)?;
    Ok(())
}

pub(super) fn build(root: &Path, destination: &Path) -> Result<()> {
    let root = root.canonicalize()?;
    let destination = new_output(destination)?;
    let rustc =
        PathBuf::from(env::var_os("FLOE_PACKAGER_RUSTC").ok_or("missing installed rustc path")?);
    let cargo =
        PathBuf::from(env::var_os("FLOE_PACKAGER_CARGO").ok_or("missing installed cargo path")?);
    let compiler = output(Command::new(&rustc).arg("-Vv"))?;
    let target = compiler
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .filter(|s| matches!(*s, "x86_64-apple-darwin" | "aarch64-apple-darwin"))
        .ok_or("desktop notices require a native macOS toolchain")?;
    let sysroot = PathBuf::from(output(Command::new(&rustc).args(["--print", "sysroot"]))?.trim());
    let stamp = revision(&root)?;
    let mut crates = dependency_notices(&root, &cargo, &rustc, target)?;
    crates.extend(dependency_notices_for(
        &root,
        &cargo,
        &rustc,
        target,
        "desktop",
        &["floe-desktop"],
    )?);
    let crates = unique_crates(crates)?;
    // Caller supplies a NEW directory in its own private preview stage. An
    // incomplete result remains visibly incomplete; nothing existing is removed.
    fs::DirBuilder::new().mode(0o700).create(&destination)?;
    notices(&root, &sysroot, None, &destination.join("NOTICES"), &crates)?;
    for (source, name) in [
        ("desktop/Cargo.lock", "desktop/Cargo.lock"),
        ("desktop/Cargo.toml", "desktop/Cargo.toml"),
        ("desktop/NOTICES.md", "desktop/UPSTREAM-SUPPLEMENT.md"),
    ] {
        copy(
            &root.join(source),
            &destination.join("NOTICES").join(name),
            false,
        )?;
    }
    let names = files(&destination.join("NOTICES"))?
        .iter()
        .map(|p| {
            p.strip_prefix(&destination)?
                .to_str()
                .map(str::to_owned)
                .ok_or("invalid notice name".into())
        })
        .collect::<Result<Vec<_>>>()?;
    let cancel = CANCEL.get().ok_or("missing cancellation flag")?;
    let index = floe_notices::build_index(&destination, &names, &stamp, target, cancel)?;
    let digest = floe_notices::digest(&index);
    fs::write(destination.join(floe_notices::INDEX_NAME), index)?;
    let catalog = floe_notices::Catalog::open(&destination, &digest, &stamp, target, cancel)?;
    let mut start = Some(0);
    while let Some(at) = start {
        check_cancelled()?;
        let page = catalog.list(at)?;
        for file in page.files {
            for chunk in 0..file.pages {
                check_cancelled()?;
                catalog.page(file.id, chunk, cancel)?;
            }
        }
        start = page.next;
    }
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o755))?;
    println!(
        "source_revision={stamp}\nnotice_index_sha1={digest}\ntarget={target}\nfiles={}",
        names.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_replaces_only_its_regular_pointer_and_rejects_other_paths() {
        let root = Stage::new(&env::temp_dir()).unwrap();
        let target = root.0.join("desktop/target");
        let apps: Vec<_> = ["macos-dev.first", "macos-dev.second", "macos-dev.third"]
            .into_iter()
            .map(|name| {
                let app = target.join(name).join("Floe2.app");
                fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
                fs::create_dir_all(app.join("Contents/Resources")).unwrap();
                fs::write(app.join("Contents/MacOS/floe2-desktop"), b"host").unwrap();
                fs::write(app.join("Contents/Resources/NOTICE-INDEX.json"), b"index").unwrap();
                app
            })
            .collect();
        let pointer = target.join("macos-preview-debug.path");
        assert!(receipt(&root.0, "../release", &apps[0]).is_err());
        assert!(receipt(&root.0, "debug", &apps[0].join("Contents")).is_err());
        receipt(&root.0, "debug", &apps[0]).unwrap();
        receipt(&root.0, "debug", &apps[1]).unwrap();
        assert_eq!(
            fs::read_to_string(&pointer).unwrap(),
            format!("{}\n", apps[1].canonicalize().unwrap().display())
        );
        assert!(apps[0].is_dir());
        fs::remove_file(&pointer).unwrap();
        let keep = root.0.join("keep");
        fs::write(&keep, b"untouched").unwrap();
        std::os::unix::fs::symlink(&keep, &pointer).unwrap();
        assert!(receipt(&root.0, "debug", &apps[2]).is_err());
        assert_eq!(fs::read(&keep).unwrap(), b"untouched");
    }

    #[test]
    fn supplements_are_pinned_and_shared_crates_do_not_overwrite() {
        assert!(supplemented(Path::new("vendor/objc2-0.6.4")));
        assert!(!supplemented(Path::new("vendor/objc2-0.6.5")));
        assert!(!supplemented(Path::new("vendor/other-0.6.4")));
        let first = PathBuf::from("rust/vendor/same-1.0");
        assert_eq!(unique_crates(vec![first.clone(), first]).unwrap().len(), 1);
        assert!(unique_crates(vec![
            "rust/vendor/same-1.0".into(),
            "desktop/vendor/same-1.0".into()
        ])
        .is_err());
    }
}
