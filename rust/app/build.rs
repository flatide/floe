use std::{env, fs, path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
fn valid_revision(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-+".contains(&c))
}
fn main() {
    println!("cargo:rerun-if-env-changed=FLOE_SRC_REV");
    println!("cargo:rerun-if-changed=src");
    println!(
        "cargo:rustc-env=FLOE_APP_TARGET={}",
        env::var("TARGET").unwrap()
    );
    if let Ok(revision) = env::var("FLOE_SRC_REV") {
        assert!(
            valid_revision(&revision),
            "FLOE_SRC_REV must be 1..128 ASCII letters/digits/._-+"
        );
        println!("cargo:rustc-env=FLOE_APP_REVISION={revision}");
        return;
    }
    // A source ZIP must never inherit the enclosing, unrelated repository's
    // revision. Worktree .git files are supported by git -C, not --git-dir.
    let root = Path::new("../..").canonicalize().unwrap();
    let root_matches = root.join(".git").exists()
        && git(&root, &["rev-parse", "--show-toplevel"])
            .and_then(|p| fs::canonicalize(p).ok())
            .as_ref()
            == Some(&root);
    let mut revision = "unknown".to_owned();
    if root_matches {
        if let Some(head) = git(&root, &["rev-parse", "HEAD"]).filter(|s| valid_revision(s)) {
            revision = head;
            if git(&root, &["status", "--porcelain"]).is_some_and(|s| !s.is_empty()) {
                revision.push('+');
            }
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
                let path = root.join(path);
                if path.exists() {
                    println!("cargo:rerun-if-changed={}", path.display());
                }
            }
        }
    }
    println!("cargo:rustc-env=FLOE_APP_REVISION={revision}");
}
