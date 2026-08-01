// stamp the git commit into the binary: multiple builds circulate on
// the test hosts and "which one is this" kept coming up
use std::process::Command;

fn main() {
    let hash = Command::new("git")
        .args(["rev-parse", "--short=9", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
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
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    if let Ok(head) = std::fs::read_to_string("../../.git/HEAD") {
        if let Some(r) = head.strip_prefix("ref: ") {
            let p = format!("../../.git/{}", r.trim());
            if std::path::Path::new(&p).is_file() {
                println!("cargo:rerun-if-changed={}", p);
            }
        }
    }
    let packed = "../../.git/packed-refs";
    if std::path::Path::new(packed).is_file() {
        println!("cargo:rerun-if-changed={}", packed);
    }
}
