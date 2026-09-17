#!/usr/bin/env python3
"""Native build provenance / no-op rebuild gate; private repositories only.

Python, Git and Cargo are development tools here, not product runtime deps.
This measures Cargo freshness, not first-executable-start or viewer latency.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def run(args, cwd, env, timeout=120):
    result = subprocess.run([str(a) for a in args], cwd=cwd, env=env,
                            capture_output=True, text=True, timeout=timeout)
    assert result.returncode == 0, (args, result.returncode, result.stdout, result.stderr)
    return result.stdout


def main():
    index_script = ROOT / "rust/cli/build.rs"
    render_script = ROOT / "rust/renderd/build.rs"
    assert index_script.read_bytes() == render_script.read_bytes(), "native stamp wrappers diverged"
    cargo = shutil.which("cargo")
    assert cargo, "development gate requires Cargo"
    clean = {k: v for k, v in os.environ.items()
             if not k.startswith("GIT_") and k not in ("FLOE_SRC_REV", "CARGO_TARGET_DIR")}
    clean.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1",
                 GIT_OPTIONAL_LOCKS="0", CARGO_NET_OFFLINE="true", CARGO_BUILD_JOBS="2")
    with tempfile.TemporaryDirectory(prefix="floe-native-revision-") as td:
        work = Path(td).resolve()
        repo = work / "source 한 글"
        crate = repo / "rust/cli"
        (crate / "src").mkdir(parents=True)
        support = repo / "rust/build_support"
        support.mkdir()
        shutil.copy2(index_script, crate / "build.rs")
        shutil.copy2(ROOT / "rust/build_support/native_revision.rs", support)
        (crate / "Cargo.toml").write_text(
            '[package]\nname="revision-probe"\nversion="0.0.0"\nedition="2021"\n[workspace]\n')
        (crate / "src/main.rs").write_text('fn main() { println!("{}", env!("FLOE_GIT")); }\n')
        run([cargo, "generate-lockfile", "--offline"], crate, clean)

        def git(root, *args):
            return run(["git", "-C", root, "-c", "user.name=Synthetic gate",
                        "-c", "user.email=synthetic@example.invalid", "-c", "commit.gpgsign=false",
                        "-c", "core.hooksPath=/dev/null", *args], root, clean).strip()

        git(repo, "init", "-q")
        git(repo, "add", ".")
        git(repo, "commit", "-qm", "synthetic source")
        original = git(repo, "rev-parse", "--short=9", "HEAD")
        linked = work / "linked 한 글"
        git(repo, "worktree", "add", "-qb", "revision-fixture", str(linked), "HEAD")
        linked_crate = linked / "rust/cli"
        build_env = dict(clean, CARGO_TARGET_DIR=str(work / "target"))

        def build(fresh, revision):
            rows = [json.loads(line) for line in run(
                [cargo, "build", "--offline", "--locked", "-j2", "--message-format=json"],
                linked_crate, build_env).splitlines()]
            artifacts = [r for r in rows if r.get("reason") == "compiler-artifact"]
            binaries = [r for r in artifacts if r["target"]["kind"] == ["bin"]]
            assert len(binaries) == 1 and binaries[0]["fresh"] == fresh, binaries
            outputs = [r for r in rows if r.get("reason") == "build-script-executed"]
            assert len(outputs) == 1 and dict(outputs[0]["env"])["FLOE_GIT"] == revision, outputs
            scripts = [r for r in artifacts if r["target"]["kind"] == ["custom-build"]]
            assert len(scripts) == 1 and len(scripts[0]["filenames"]) == 1
            return Path(scripts[0]["filenames"][0])

        helper = build(False, original)
        build(True, original)  # Old .git/HEAD watch fails this assertion.

        def identity(cwd, env=clean):
            output = run([helper], cwd, env)
            fields = dict(line.removeprefix("cargo:rustc-env=").split("=", 1)
                          for line in output.splitlines() if line.startswith("cargo:rustc-env="))
            watched = [(cwd / line.removeprefix("cargo:rerun-if-changed=")).resolve()
                       for line in output.splitlines() if line.startswith("cargo:rerun-if-changed=")]
            assert watched and all(p.exists() for p in watched), watched
            return fields["FLOE_GIT"], set(watched)

        def git_path(root, item):
            return (root / git(root, "rev-parse", "--git-path", item)).resolve()

        for root in (repo, linked):
            revision, watched = identity(root / "rust/cli")
            assert revision == original
            for item in ("HEAD", "index", git(root, "symbolic-ref", "HEAD")):
                assert git_path(root, item) in watched, (root, item, watched)
        assert linked.joinpath(".git").resolve() in identity(linked_crate)[1]

        # A watched package edit refreshes the dirty stamp, but its unchanged
        # repeat stays fresh. No target directory is placed inside either repo.
        main_file = linked_crate / "src/main.rs"
        main_file.write_text(main_file.read_text() + "// synthetic source edit\n")
        build(False, original + "+")
        build(True, original + "+")
        git(linked, "add", ".")
        git(linked, "commit", "-qm", "synthetic edit")
        current = git(linked, "rev-parse", "--short=9", "HEAD")
        assert current != original
        build(False, current)
        build(True, current)

        # Branch metadata alone, without a source change, refreshes the stamp.
        git(linked, "commit", "--allow-empty", "-qm", "synthetic metadata-only commit")
        current = git(linked, "rev-parse", "--short=9", "HEAD")
        build(False, current)
        build(True, current)
        git(repo, "pack-refs", "--all", "--prune")
        revision, watched = identity(linked_crate)
        assert revision == current and git_path(linked, "packed-refs") in watched
        assert not git_path(linked, "refs/heads/revision-fixture").exists()
        build(False, current)
        build(True, current)

        # Detached worktree HEAD remains private; inherited repository-routing
        # variables cannot substitute the main checkout's different revision.
        git(linked, "checkout", "--detach", "-q", "HEAD")
        assert identity(linked_crate)[0] == current
        build(False, current)
        build(True, current)
        routed = dict(clean, GIT_DIR=str(repo / ".git"), GIT_WORK_TREE=str(repo),
                      GIT_COMMON_DIR=str(repo / ".git"), GIT_INDEX_FILE=str(repo / ".git/index"))
        assert identity(linked_crate, routed)[0] == current

        # An archive nested in a repository is NOT that repository. Even with
        # Git unavailable, a caller may supply the existing native ZIP override.
        archive = repo / "source ZIP/rust/cli"
        (archive / "src").mkdir(parents=True)
        shutil.copy2(index_script, archive / "build.rs")
        assert identity(archive)[0] == "unknown"
        assert identity(archive, dict(clean, PATH="", FLOE_SRC_REV="  offline-r1  "))[0] == "offline-r1"
        assert identity(linked_crate, dict(clean, PATH=""))[0] == "unknown"
        assert identity(linked_crate, dict(clean, FLOE_SRC_REV="  "))[0] == current
        (archive.parent.parent / ".git").write_text("not a git pointer\n")
        assert identity(archive)[0] == "unknown"
        print("NATIVE REVISION: ALL OK (shared wrappers, real Cargo no-op freshness, source/HEAD changes, worktree/common refs, packed/detached refs, ZIP isolation, override, unavailable Git)")


if __name__ == "__main__":
    main()
