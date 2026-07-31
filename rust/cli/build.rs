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
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}
