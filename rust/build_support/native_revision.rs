//! Native build-time provenance, shared by index/renderd; not runtime identity.
use std::{env, fs, path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    // Stamp this checkout, not repository-routing variables inherited from a
    // caller (for example a Git hook). Status must not refresh the watched index.
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
    ] {
        command.env_remove(key);
    }
    let output = command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn watch(path: &Path) {
    // Cargo treats a missing watched path as dirty on EVERY invocation.
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

pub fn emit() {
    println!("cargo:rerun-if-env-changed=FLOE_SRC_REV");
    for path in [
        "build.rs",
        "src",
        "Cargo.toml",
        "../Cargo.toml",
        "../Cargo.lock",
        "../build_support/native_revision.rs",
    ] {
        watch(Path::new(path));
    }
    // Preserve the native ZIP workflow: explicit nonblank override, then this
    // repository only, otherwise unknown. Do not inherit an enclosing repo.
    if let Ok(revision) = env::var("FLOE_SRC_REV") {
        let revision = revision.trim();
        if !revision.is_empty() {
            println!("cargo:rustc-env=FLOE_GIT={revision}");
            return;
        }
    }
    let root = fs::canonicalize("../..").ok();
    let root = root.filter(|root| {
        root.join(".git").exists()
            && git(root, &["rev-parse", "--show-toplevel"])
                .and_then(|p| fs::canonicalize(p).ok())
                .as_ref()
                == Some(root)
    });
    let Some(root) = root else {
        println!("cargo:rustc-env=FLOE_GIT=unknown");
        return;
    };
    // A worktree .git is a pointer file. HEAD/index live in its private Git
    // directory, while branch refs/packed-refs can live in the common directory.
    if root.join(".git").is_file() {
        watch(&root.join(".git"));
    }
    let branch = git(&root, &["symbolic-ref", "-q", "HEAD"]);
    for item in [
        Some("HEAD"),
        Some("index"),
        Some("packed-refs"),
        branch.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(path) = git(&root, &["rev-parse", "--git-path", item]) {
            watch(&root.join(path));
        }
    }
    let mut revision = git(&root, &["rev-parse", "--short=9", "HEAD"])
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    if revision != "unknown"
        && git(&root, &["status", "--porcelain"]).is_some_and(|s| !s.is_empty())
    {
        revision.push('+');
    }
    println!("cargo:rustc-env=FLOE_GIT={revision}");
}
