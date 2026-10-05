// stamp the source revision into the binary: multiple builds
// circulate on the test hosts and "which one is this" kept coming up
// (copy of rust/cli/build.rs - keep the two in step)
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
    // the script on every build). In a linked work tree (`git worktree
    // add`) .git is a file naming the directory that holds this tree's
    // HEAD, and the refs are in that directory's `commondir`: watching
    // `.git/HEAD` there watched a path that does not exist, and every
    // build compiled the binary again (2026-10-06: 16 s a build in the
    // review tree and in every gate run).
    let (head_dir, refs_dir) = git_dirs(gitdir);
    let head = head_dir.join("HEAD");
    if !head.is_file() {
        return;
    }
    println!("cargo:rerun-if-changed={}", head.display());
    if let Ok(text) = std::fs::read_to_string(&head) {
        if let Some(r) = text.strip_prefix("ref: ") {
            for dir in [&head_dir, &refs_dir] {
                let p = dir.join(r.trim());
                if p.is_file() {
                    println!("cargo:rerun-if-changed={}", p.display());
                    break;
                }
            }
        }
    }
    let packed = refs_dir.join("packed-refs");
    if packed.is_file() {
        println!("cargo:rerun-if-changed={}", packed.display());
    }
}

/// The directory that holds this work tree's HEAD and the one its refs are
/// in: `.git` itself, or - .git a file `gitdir: <dir>` (a linked work
/// tree) - the directory it names and that directory's `commondir`.
fn git_dirs(dot_git: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::path::{Path, PathBuf};
    let path = Path::new(dot_git);
    if path.is_dir() {
        return (path.to_path_buf(), path.to_path_buf());
    }
    let at = |base: &Path, named: &str| -> PathBuf {
        let named = Path::new(named.trim());
        if named.is_absolute() {
            named.to_path_buf()
        } else {
            base.join(named)
        }
    };
    let Some(dir) = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.lines().find_map(|line| line.strip_prefix("gitdir:").map(|named| at(path.parent().unwrap_or(Path::new(".")), named)))) else {
        return (path.to_path_buf(), path.to_path_buf());
    };
    let common = std::fs::read_to_string(dir.join("commondir")).map(|named| at(&dir, &named)).unwrap_or_else(|_| dir.clone());
    (dir, common)
}
