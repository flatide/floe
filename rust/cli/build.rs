// stamp the source revision into the binary: multiple builds
// circulate on the test hosts and "which one is this" kept coming up
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=FLOE_SRC_REV");
    // zip-carried source trees have no .git, and a plain `git
    // rev-parse` walks UP from the extraction dir and stamps
    // whatever enclosing repo it finds (observed on the closed
    // network: a foreign hash nobody could match to our history).
    // Precedence: explicit FLOE_SRC_REV (the zip workflow), then the
    // repo sitting exactly at the workspace root, then "unknown".
    if let Ok(rev) = std::env::var("FLOE_SRC_REV") {
        let rev = rev.trim().to_string();
        if !rev.is_empty() {
            println!("cargo:rustc-env=FLOE_GIT={}", rev);
            return;
        }
    }
    let gitdir = "../../.git";
    if !std::path::Path::new(gitdir).exists() {
        println!("cargo:rustc-env=FLOE_GIT=unknown");
        return;
    }
    let hash = Command::new("git")
        .args(["--git-dir", gitdir, "rev-parse", "--short=9", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    let dirty = Command::new("git")
        .args([
            "--git-dir",
            gitdir,
            "--work-tree",
            "../..",
            "status",
            "--porcelain",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    println!(
        "cargo:rustc-env=FLOE_GIT={}{}",
        hash,
        if dirty { "+" } else { "" }
    );
    // on a branch, HEAD's content ("ref: refs/heads/main") never
    // changes - the commit lands in the ref file, so watch that too
    // (only if it exists: a missing watched path makes cargo rerun
    // the script on every build)
    println!("cargo:rerun-if-changed={}/HEAD", gitdir);
    if let Ok(head) = std::fs::read_to_string(format!("{}/HEAD", gitdir)) {
        if let Some(r) = head.strip_prefix("ref: ") {
            let p = format!("{}/{}", gitdir, r.trim());
            if std::path::Path::new(&p).is_file() {
                println!("cargo:rerun-if-changed={}", p);
            }
        }
    }
    let packed = format!("{}/packed-refs", gitdir);
    if std::path::Path::new(&packed).is_file() {
        println!("cargo:rerun-if-changed={}", packed);
    }
}
