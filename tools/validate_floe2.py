#!/usr/bin/env python3
"""Product-boundary checks for the Rust-only ``floe2`` shell."""

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def cargo_package_version(path):
    """Read package.version without depending on the host Python's TOML API."""
    in_package = False
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if line.startswith("["):
            in_package = line == "[package]"
            continue
        if in_package:
            match = re.fullmatch(r'version\s*=\s*"([^"]+)"', line)
            if match:
                return match.group(1)
    raise AssertionError("package.version is missing from %s" % path)


def run(env, *args, ok=True, timeout=20):
    result = subprocess.run(
        [sys.executable, "-B", *map(str, args)], cwd=ROOT, env=env,
        capture_output=True, text=True, timeout=timeout)
    if ok and result.returncode:
        raise AssertionError(
            "command failed: %r\nstdout:\n%s\nstderr:\n%s" %
            (args, result.stdout, result.stderr))
    if not ok and not result.returncode:
        raise AssertionError("command unexpectedly succeeded: %r" % (args,))
    return result


def install_import_blocker(directory):
    blocker = Path(directory) / "blocker"
    blocker.mkdir()
    (blocker / "sitecustomize.py").write_text(
        "import builtins\n"
        "_real = builtins.__import__\n"
        "def _guard(name, *args, **kwargs):\n"
        "    if name == 'klayout' or name.startswith('klayout.'):\n"
        "        raise RuntimeError('KLayout import forbidden')\n"
        "    return _real(name, *args, **kwargs)\n"
        "builtins.__import__ = _guard\n", encoding="utf-8")
    return blocker


def validate_runtime(base, fixture):
    """Run the complete floe2 CLI lifecycle with KLayout imports blocked."""
    indexer = ROOT / "rust" / "target" / "release" / "floe-index"
    renderer = ROOT / "rust" / "target" / "release" / "floe-renderd"
    check(indexer.is_file(), "release floe-index is not built")
    check(renderer.is_file(), "release floe-renderd is not built")
    check(fixture.is_file(), "integration fixture is missing: %s" % fixture)

    with tempfile.TemporaryDirectory(prefix="floe2-runtime-") as td:
        work = Path(td)
        blocker = install_import_blocker(work)
        source = work / "설계 fixture with spaces.oas"
        shutil.copy2(fixture, source)
        env = dict(base, FLOE_INDEX_BIN=str(indexer),
                   FLOE_RENDERD_BIN=str(renderer), FLOE_RUST_ROUND_PAGES="4")
        env["PYTHONPATH"] = os.pathsep.join((str(blocker), str(ROOT)))

        indexed = run(env, "-m", "floe2", "index", source,
                      "--jobs", "2", timeout=60)
        check("[floe2]" in indexed.stdout,
              "floe2 index output kept the shared floe product prefix")
        cache = Path(str(source) + ".floe")
        meta = json.loads((cache / "meta.json").read_text())
        bbox = meta["bbox"]
        dbu = float(meta["dbu"])

        info = run(env, "-m", "floe2", "info", source)
        check("top cell" in info.stdout, "floe2 info omitted cache identity")
        check("design.ovc" not in info.stdout,
              "floe2 info exposed retired density coverage")
        probe = run(env, "-m", "floe2", "probe", source, timeout=90)
        check("[probe] OK" in probe.stdout,
              "floe2 probe did not settle a Rust frame")

        def bbox_arg(inset):
            x0, y0, x1, y1 = (float(value) for value in bbox)
            dx, dy = (x1 - x0) * inset, (y1 - y0) * inset
            values = (x0 + dx, y0 + dy, x1 - dx, y1 - dy)
            return ",".join("%.12g" % (value * dbu) for value in values)

        png = work / "floe2 labels with spaces.png"
        rendered = run(
            env, "-m", "floe2", "render", source,
            "--bbox", bbox_arg(0), "--px", "257", "--depth", "999",
            "--frames", "--labels", "--label-font-px", "17",
            "--out", png, timeout=90)
        check("[floe2] rendered" in rendered.stdout,
              "floe2 render output kept the shared floe product prefix")
        check(png.read_bytes().startswith(b"\x89PNG\r\n\x1a\n"),
              "floe2 render did not publish a PNG")

        cell_name = "FLOE2_한글"
        clipped = work / "UTF-8 clip with spaces.oas"
        clip_result = run(
            env, "-m", "floe2", "clip", source,
            "--bbox", bbox_arg(0.2), "--cell-name", cell_name,
            "--out", clipped, timeout=90)
        check("[floe2] clip saved" in clip_result.stdout,
              "floe2 clip output kept the shared floe product prefix")
        check(cell_name.encode("utf-8") in clipped.read_bytes(),
              "UTF-8 clip cell name was not serialized")
        scanned = subprocess.run(
            [str(indexer), "scan", str(clipped), "1"], cwd=ROOT,
            capture_output=True, text=True, timeout=30)
        check(scanned.returncode == 0,
              "Rust parser rejected floe2 clip: %s" % scanned.stderr)

        report = work / "anonymous floe2 benchmark.json"
        benchmark = run(
            env, ROOT / "tools" / "bench_floe2.py", source,
            "--jobs", "1", "--runs", "1", "--width", "257",
            "--height", "171", "--round-pages", "4",
            "--renderd", renderer, "--out", report, timeout=90)
        check("privacy-safe report:" in benchmark.stdout,
              "floe2 benchmark did not finish")
        report_text = report.read_text(encoding="utf-8")
        payload = json.loads(report_text)
        check(payload.get("schema") == "floe2-render-benchmark-v1",
              "floe2 benchmark report schema drifted")
        check(len(payload.get("sessions", [])) == 1 and
              len(payload["sessions"][0].get("results", [])) == 10,
              "floe2 benchmark did not cover the complete field trace")
        private_tokens = (str(source), source.name, meta.get("top_cell", ""))
        check(all(not token or token not in report_text
                  for token in private_tokens),
              "floe2 benchmark report exposed design identity")


def main(fixture=None):
    base = os.environ.copy()
    base.pop("FLOE_PRODUCT", None)
    base.pop("FLOE_RENDERER", None)
    base["PYTHONDONTWRITEBYTECODE"] = "1"
    base["PYTHONPATH"] = str(ROOT)

    floe = run(base, "-m", "floe", "--help")
    check("stable KLayout" in floe.stdout, "floe product description drifted")
    floe2 = run(base, "-m", "floe2", "--help")
    check("Rust-only" in floe2.stdout, "floe2 is not identified as Rust-only")
    check("profile" not in floe2.stdout,
          "floe2 exposed the legacy tile-cache profile command")
    index_help = run(base, "-m", "floe2", "index", "--help")
    for legacy in ("--legacy", "--tile-mb", "--read-mode", "KLayout",
                   "--coverage", "--coverage-only", "design.ovc"):
        check(legacy not in index_help.stdout,
              "floe2 index help exposed %s" % legacy)
    stable_view_help = run(base, "-m", "floe", "view", "--help")
    rust_view_help = run(base, "-m", "floe2", "view", "--help")
    for option in ("--refinement", "--frame-cache", "--perf-baseline"):
        check(option in stable_view_help.stdout,
              "floe view omitted common performance option %s" % option)
        check(option in rust_view_help.stdout,
              "floe2 view omitted common performance option %s" % option)
    for command in ("probe", "view"):
        command_help = run(base, "-m", "floe2", command, "--help")
        check("--layout-mode" not in command_help.stdout,
              "floe2 %s help exposed a KLayout worker option" % command)

    rejected_env = dict(base, FLOE_RENDERER="klayout")
    rejected = run(rejected_env, "-m", "floe2", "--version", ok=False)
    check("Rust-only" in rejected.stderr,
          "floe2 accepted or obscured a KLayout renderer override")
    legacy = run(base, "-m", "floe2", "index", "missing.oas",
                 "--legacy", ok=False)
    check("legacy indexing belongs to floe" in legacy.stderr,
          "floe2 legacy indexing did not fail at the product boundary")
    coverage = run(base, "-m", "floe2", "index", "missing.oas",
                   "--coverage", ok=False)
    check("unrecognized arguments: --coverage" in coverage.stderr,
          "floe2 still accepted retired density coverage")

    identity = r'''import json, os
from floe.cli import _renderer_backend
from floe import instance
from floe.gui import HAS_DENSITY_COVERAGE
print(json.dumps([_renderer_backend(), instance.APP,
                  instance.socket_address(":77"), HAS_DENSITY_COVERAGE]))
'''
    stable = json.loads(run(base, "-c", identity).stdout)
    rust = json.loads(run(base, "-c", "import floe2\n" + identity).stdout)
    check(stable[0:2] == ["klayout", "floe"],
          "stable floe no longer owns the KLayout/floe identity")
    check(rust[0:2] == ["rust", "floe2"],
          "floe2 did not select the Rust/floe2 identity")
    check(stable[2] != rust[2], "floe and floe2 share an instance socket")
    check(stable[3] is True and rust[3] is False,
          "floe2 still advertises density coverage UI state")

    portable = ROOT / "tools" / "make_portable.sh"
    launcher = ROOT / "tools" / "portable_launcher.sh"
    syntax = subprocess.run(
        ["bash", "-n", str(portable)], cwd=ROOT,
        capture_output=True, text=True)
    check(syntax.returncode == 0,
          "portable script syntax failed: %s" % syntax.stderr)
    launcher_syntax = subprocess.run(
        ["sh", "-n", str(launcher)], cwd=ROOT,
        capture_output=True, text=True)
    check(launcher_syntax.returncode == 0,
          "portable launcher syntax failed: %s" % launcher_syntax.stderr)
    portable_source = portable.read_text(encoding="utf-8")
    check('PORTABLE_LAUNCHERS="floe floe2"' in portable_source and
          'cp "$REPO/tools/portable_launcher.sh" "$B/$PRODUCT"' in
          portable_source,
          "KLayout floe-portable does not install both product launchers")
    check("MUSL_TARGET=x86_64-unknown-linux-musl" in portable_source and
          '--target "$MUSL_TARGET" -p floe-index -p floe-renderd' in
          portable_source,
          "portable defaults to host-glibc Rust binaries instead of musl")
    check("FLOE_INDEX_BIN and FLOE_RENDERD_BIN must be specified together"
          in portable_source,
          "portable permits a mismatched Rust binary override")
    package_version = run(
        base, "-c", "import floe; print(floe.__version__)").stdout.strip()
    index_version = cargo_package_version(ROOT / "rust" / "cli" /
                                          "Cargo.toml")
    renderd_version = cargo_package_version(ROOT / "rust" / "renderd" /
                                            "Cargo.toml")
    check(package_version == index_version == renderd_version,
          "floe/index/renderd version mismatch: %s/%s/%s" %
          (package_version, index_version, renderd_version))
    portable_name = subprocess.run(
        ["bash", str(portable), "--print-name"], cwd=ROOT, env=base,
        capture_output=True, text=True, timeout=10)
    check(portable_name.returncode == 0 and
          portable_name.stdout.strip().startswith(
              "floe2-portable-%s-" % package_version) and
          "#" not in portable_name.stdout,
          "portable artifact name contains the __version__ line comment: %s"
          % portable_name.stdout.strip())
    launcher_source = launcher.read_text(encoding="utf-8")
    check('exec "$RT/bin/python3" -m "$PRODUCT" "$@"' in
          launcher_source,
          "portable launcher does not dispatch by floe/floe2 basename")
    with tempfile.TemporaryDirectory(
            prefix="floe-portable-launcher-") as td:
        bundle = Path(td)
        (bundle / "runtime" / "bin").mkdir(parents=True)
        echo = shutil.which("echo")
        check(echo is not None, "portable launcher test needs echo")
        os.symlink(echo, bundle / "runtime" / "bin" / "python3")
        for product in ("floe", "floe2"):
            target = bundle / product
            shutil.copy2(launcher, target)
            target.chmod(0o755)
            launched = subprocess.run(
                [str(target), "view", "chip.oas"], cwd=bundle,
                env=dict(base, XDG_CACHE_HOME=str(bundle / "cache")),
                capture_output=True, text=True, timeout=10)
            check(launched.returncode == 0 and
                  launched.stdout.strip() ==
                  "-m %s view chip.oas" % product,
                  "portable %s launcher dispatched incorrectly: %s %s" %
                  (product, launched.stdout, launched.stderr))
        link_dir = bundle / "links"
        link_dir.mkdir()
        os.symlink("../floe2", link_dir / "floe2")
        linked = subprocess.run(
            [str(link_dir / "floe2"), "--version"], cwd=bundle,
            env=dict(base, XDG_CACHE_HOME=str(bundle / "cache")),
            capture_output=True, text=True, timeout=10)
        check(linked.returncode == 0 and
              linked.stdout.strip() == "-m floe2 --version",
              "portable floe2 convenience symlink lost its runtime")
    conflict_env = dict(base, FLOE_PORTABLE_PRODUCT="floe2",
                        FLOE_PORTABLE_KLAYOUT="1")
    conflict = subprocess.run(
        ["bash", str(portable)], cwd=ROOT, env=conflict_env,
        capture_output=True, text=True, timeout=10)
    check(conflict.returncode != 0 and "Rust-only" in conflict.stdout,
          "portable allowed a floe2/KLayout product mixture")

    with tempfile.TemporaryDirectory(prefix="floe2-cli-") as td:
        work = Path(td)
        blocker = install_import_blocker(work)
        log = work / "calls.json"
        binary = work / "floe-index"
        binary.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, pathlib, sys\n"
            "pathlib.Path(os.environ['FLOE2_CALL']).write_text("
            "json.dumps(sys.argv[1:]))\n",
            encoding="utf-8")
        binary.chmod(0o755)
        source = work / "design with spaces.oas"
        source.write_bytes(b"fixture")
        env = dict(base, FLOE_INDEX_BIN=str(binary), FLOE2_CALL=str(log))
        env["PYTHONPATH"] = os.pathsep.join((str(blocker), str(ROOT)))
        delegated = run(env, "-m", "floe2", "index", source,
                        "--jobs", "2")
        check(delegated.returncode == 0, "floe2 Rust index delegation failed")
        check(json.loads(log.read_text()) == [
            "vfs", str(source), str(source) + ".floe", "--jobs", "2",
            "--no-lod",
        ], "floe2 changed the canonical Rust index argv")

    if fixture is not None:
        validate_runtime(base, Path(fixture).resolve())
    print("FLOE2 PRODUCT VALIDATION: ALL OK")


if __name__ == "__main__":
    if len(sys.argv) > 2:
        raise SystemExit("usage: validate_floe2.py [fixture.oas]")
    main(sys.argv[1] if len(sys.argv) == 2 else None)
