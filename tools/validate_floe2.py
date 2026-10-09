#!/usr/bin/env python3
"""Product-boundary checks for ``floe2``: the Rust command line
(rust/floe2 over the shared floe-app-cli) and its GTK viewer, the one part
left in Python (docs/SHARED_APP_LAYER.ko.md; the Python floe2 CLI is gone,
P1c). The frozen ``floe`` shell stays the development oracle."""

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe.cachepath import vfs_cache_dir  # noqa: E402


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


FLOE2 = Path(os.environ.get("FLOE2_BIN") or
             ROOT / "rust" / "target" / "release" / "floe2")


def rust(env, *args, ok=True, timeout=90):
    """The Rust `floe2`: ok=True wants 0, False any failure, an int that
    status."""
    result = subprocess.run([str(FLOE2), *map(str, args)], cwd=ROOT,
                            env=env, capture_output=True, text=True,
                            timeout=timeout)
    if ok is True and result.returncode:
        raise AssertionError("floe2 %r failed (%d)\n%s\n%s" % (
            args, result.returncode, result.stdout, result.stderr))
    if ok is False and not result.returncode:
        raise AssertionError("floe2 %r unexpectedly succeeded" % (args,))
    if not isinstance(ok, bool) and result.returncode != ok:
        raise AssertionError("floe2 %r: exit %d, wanted %d\n%s\n%s" % (
            args, result.returncode, ok, result.stdout, result.stderr))
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
    """Run the complete floe2 lifecycle - the Rust command line, and the
    Python benchmark tool with KLayout imports blocked."""
    indexer = ROOT / "rust" / "target" / "release" / "floe-index"
    renderer = ROOT / "rust" / "target" / "release" / "floe-renderd"
    check(indexer.is_file(), "release floe-index is not built")
    check(renderer.is_file(), "release floe-renderd is not built")
    check(FLOE2.is_file(), "release floe2 is not built")
    check(fixture.is_file(), "integration fixture is missing: %s" % fixture)

    with tempfile.TemporaryDirectory(prefix="floe2-runtime-") as td:
        work = Path(td)
        blocker = install_import_blocker(work)
        source = work / "설계 fixture with spaces.oas"
        shutil.copy2(fixture, source)
        env = dict(base, FLOE_INDEX_BIN=str(indexer),
                   FLOE_RENDERD_BIN=str(renderer), FLOE_RUST_ROUND_PAGES="4")
        env["PYTHONPATH"] = os.pathsep.join((str(blocker), str(ROOT)))

        indexed = rust(env, "index", source, "--jobs", "2", timeout=60)
        check("[floe2]" in indexed.stdout,
              "floe2 index output kept the shared floe product prefix")
        cache = Path(vfs_cache_dir(source))
        meta = json.loads((cache / "meta.json").read_text())
        bbox = meta["bbox"]
        dbu = float(meta["dbu"])

        info = rust(env, "info", source)
        check("top cell" in info.stdout, "floe2 info omitted cache identity")
        check("design.ovc" not in info.stdout,
              "floe2 info exposed retired density coverage")
        probe = rust(env, "probe", source, timeout=90)
        check("[probe] OK" in probe.stdout,
              "floe2 probe did not settle a Rust frame")

        def bbox_arg(inset):
            x0, y0, x1, y1 = (float(value) for value in bbox)
            dx, dy = (x1 - x0) * inset, (y1 - y0) * inset
            values = (x0 + dx, y0 + dy, x1 - dx, y1 - dy)
            return ",".join("%.12g" % (value * dbu) for value in values)

        png = work / "floe2 labels with spaces.png"
        rendered = rust(
            env, "render", source,
            "--bbox", bbox_arg(0), "--px", "257", "--depth", "999",
            "--frames", "--labels", "--label-font-px", "17",
            "--out", png, timeout=90)
        check("[floe2] rendered" in rendered.stdout,
              "floe2 render output kept the shared floe product prefix")
        check(png.read_bytes().startswith(b"\x89PNG\r\n\x1a\n"),
              "floe2 render did not publish a PNG")

        cell_name = "FLOE2_한글"
        clipped = work / "UTF-8 clip with spaces.oas"
        clip_result = rust(
            env, "clip", source,
            "--bbox", bbox_arg(0.2), "--cell-name", cell_name,
            "--out", clipped, timeout=90)
        check("clip saved" in clip_result.stdout,
              "floe2 clip did not report its file")
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


def validate_rust_cli(base, fixture):
    """The Rust command line `floe2` (rust/floe2 + the shared floe-app-cli,
    docs/SHARED_APP_LAYER.ko.md; 2026-10-09 P1a): its version is the app's,
    index/info/probe/render/clip run with no Python at all (a `python`/
    `python3` first on PATH that records any call stays unused), and `view`
    hands its arguments to the GTK viewer's Python entry."""
    indexer = ROOT / "rust" / "target" / "release" / "floe-index"
    renderer = ROOT / "rust" / "target" / "release" / "floe-renderd"
    check(FLOE2.is_file(), "release floe2 is not built")
    package_version = run(
        base, "-c", "import floe; print(floe.__version__)").stdout.strip()

    with tempfile.TemporaryDirectory(prefix="floe2-rust-cli-") as td:
        work = Path(td)
        trap = work / "no-python"
        trap.mkdir()
        called = work / "python-called"
        for name in ("python", "python3"):
            script = trap / name
            script.write_text("#!/bin/sh\necho \"$@\" >> %s\nexit 99\n" % called)
            script.chmod(0o755)
        env = {"PATH": "%s:/usr/bin:/bin" % trap, "HOME": base.get("HOME", td),
               "FLOE_INDEX_BIN": str(indexer), "FLOE_RENDERD_BIN": str(renderer),
               "FLOE_GTK_PYTHON": str(trap / "python3")}
        # the GTK viewer's service (P2): one JSON line in, one out
        served = subprocess.run(
            [str(FLOE2), "gtk-service"], input='{"id": 7, "op": "version"}\n',
            env=env, capture_output=True, text=True, timeout=30)
        reply = json.loads(served.stdout)
        check(served.returncode == 0 and reply["id"] == 7 and
              reply["result"]["version"] == package_version,
              "floe2 gtk-service did not answer: %s %s" % (served.stdout,
                                                            served.stderr))
        version = rust(env, "--version").stdout
        check(version.startswith("floe2 %s " % package_version),
              "floe2 --version is not the app version: %s" % version)
        check("floe-renderd" in version and "viewer gtk" in version,
              "floe2 --version omitted its native identity: %s" % version)
        source = work / "rust cli 설계.oas"
        shutil.copy2(fixture, source)
        indexed = rust(env, "index", source, "--jobs", "2")
        check(Path(vfs_cache_dir(source), "design.ovm").is_file(),
              "floe2 index wrote no cache")
        info = rust(env, "info", source)
        check("top cell" in info.stdout and "stored shapes" in info.stdout,
              "floe2 info lost the cache table")
        probe = rust(env, "probe", source)
        check("OK" in probe.stdout + probe.stderr, "floe2 probe did not settle")
        png = work / "rust cli.png"
        rust(env, "render", source, "--px", "257", "--out", png)
        check(png.read_bytes().startswith(b"\x89PNG\r\n\x1a\n"),
              "floe2 render did not publish a PNG")
        meta = json.loads(Path(vfs_cache_dir(source), "meta.json").read_text())
        dbu = float(meta["dbu"])
        x0, y0, x1, y1 = (float(v) * dbu for v in meta["bbox"])
        clipped = work / "rust clip.oas"
        rust(env, "clip", source, "--bbox=%r,%r,%r,%r" % (
            x0, y0, (x0 + x1) / 2, (y0 + y1) / 2), "--out", clipped)
        check(clipped.is_file() and clipped.stat().st_size > 0,
              "floe2 clip wrote nothing")
        check(not called.exists(),
              "floe2 ran Python for a non-UI command: %s" % (
                  called.read_text() if called.exists() else ""))
        unknown = rust(env, "frobnicate", ok=False)
        check(unknown.returncode == 99 and called.exists(),
              "floe2 did not hand an unknown word to the GTK viewer as a source")
        called.unlink()
        # view: the GTK entry with the real interpreter takes the arguments
        view_env = dict(env, FLOE_GTK_PYTHON=sys.executable,
                        PATH=os.environ.get("PATH", "/usr/bin:/bin"))
        view = rust(view_env, "view", "--help")
        check("usage: floe2 view" in view.stdout,
              "floe2 view did not reach the GTK viewer's entry: %s" % view.stdout[:200])


def main(fixture=None):
    base = os.environ.copy()
    base.pop("FLOE_PRODUCT", None)
    base.pop("FLOE_RENDERER", None)
    base["PYTHONDONTWRITEBYTECODE"] = "1"
    base["PYTHONPATH"] = str(ROOT)

    floe = run(base, "-m", "floe", "--help")
    check("stable KLayout" in floe.stdout, "floe product description drifted")
    help_text = rust(base, "--help").stdout
    check("floe2 view" in help_text and "floe2 index" in help_text,
          "floe2 --help lost the viewer or the shared commands")
    check("floe2 profile" not in help_text,
          "floe2 exposed the legacy tile-cache profile command")
    index_help = rust(base, "index", "--help").stdout
    for legacy in ("--legacy", "--tile-mb", "--read-mode", "--coverage",
                   "design.ovc"):
        check(legacy not in index_help,
              "floe2 index help exposed %s" % legacy)
    gtk = dict(base, FLOE_GTK_PYTHON=sys.executable)
    stable_view_help = run(base, "-m", "floe", "view", "--help")
    rust_view_help = rust(gtk, "view", "--help")
    check("usage: floe2 view" in rust_view_help.stdout,
          "floe2 view did not reach the GTK viewer's entry")
    for option in ("--refinement", "--frame-cache", "--perf-baseline"):
        check(option in stable_view_help.stdout,
              "floe view omitted common performance option %s" % option)
        check(option in rust_view_help.stdout,
              "floe2 view omitted common performance option %s" % option)
    check("--layout-mode" not in rust_view_help.stdout,
          "floe2 view help exposed a KLayout worker option")
    check("--layout-mode" not in rust(base, "probe", "--help").stdout,
          "floe2 probe help exposed a KLayout worker option")

    # the viewer is Rust-rendered whatever the environment says: the
    # launcher sets FLOE_RENDERER=rust (a KLayout override reached the
    # Python entry, which refuses it)
    klayout_env = dict(gtk, FLOE_RENDERER="klayout")
    check("usage: floe2 view" in rust(klayout_env, "view", "--help").stdout,
          "floe2 view let a KLayout renderer override through")
    legacy = rust(base, "index", "missing.oas", "--legacy", ok=2)
    check("unsupported index option: --legacy" in legacy.stderr,
          "floe2 legacy indexing did not fail at the product boundary")
    coverage = rust(base, "index", "missing.oas", "--coverage", ok=2)
    check("unsupported index option: --coverage" in coverage.stderr,
          "floe2 still accepted retired density coverage")

    identity = r'''import json, os
from floe.cli import _renderer_backend
from floe import instance
from floe.gui import HAS_DENSITY_COVERAGE
print(json.dumps([_renderer_backend(), instance.APP,
                  instance.socket_address(":77"), HAS_DENSITY_COVERAGE]))
'''
    stable = json.loads(run(base, "-c", identity).stdout)
    rust_identity = json.loads(run(
        base, "-c", "import floe.gtkview\n" + identity).stdout)
    check(stable[0:2] == ["klayout", "floe"],
          "stable floe no longer owns the KLayout/floe identity")
    check(rust_identity[0:2] == ["rust", "floe2"],
          "the GTK entry did not select the Rust/floe2 identity")
    check(stable[2] != rust_identity[2],
          "floe and floe2 share an instance socket")
    check(stable[3] is True and rust_identity[3] is False,
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
          '--target "$MUSL_TARGET" -p floe-index -p floe-renderd -p floe2' in
          portable_source,
          "portable defaults to host-glibc Rust binaries instead of musl")
    check('"bin/floe2",' in portable_source and
          'cp -r "$REPO/floe2"' not in portable_source,
          "portable does not ship the Rust floe2 in place of the Python one")
    # the floe2 bundle's Python is the viewer alone (P2d): every module the
    # viewer loads, none of the oracle's
    listed = re.search(r'FLOE2_PRODUCT_FILES="([^"]+)"', portable_source)
    check(listed is not None, "portable does not list the floe2 viewer files")
    shipped = set(listed.group(1).split())
    loaded = run(base, "-c", (
        "import sys; import floe.gtkview, floe.viewcli, floe.gui, "
        "floe.gtkservice, floe.rust_render, floe.vfsclient, floe.instance, "
        "floe.hangul, floe.fillpat; print(' '.join(sorted(m for m in "
        "sys.modules if m.startswith('floe.'))))")).stdout.split()
    needed = {m.split(".")[1] + ".py" for m in loaded}
    check(needed <= shipped, "the floe2 bundle lacks %s" % sorted(needed - shipped))
    oracle = {"cache.py", "cachepath.py", "indexlock.py", "drc.py", "svrf.py",
              "shots.py", "fe_embed.py", "render.py", "viewport.py",
              "coverage.py", "view_policy.py", "cli.py", "service.py",
              "__main__.py"}
    check(not (shipped & oracle) and not any(f.startswith("jobdeck")
                                               for f in shipped),
          "the floe2 bundle ships oracle modules: %s" % sorted(shipped & oracle))
    check(not {m for m in loaded if m.split(".")[1] + ".py" in oracle
               or m.startswith("floe.jobdeck")},
          "the floe2 viewer imports oracle modules: %s" % loaded)
    check("FLOE_INDEX_BIN and FLOE_RENDERD_BIN must be specified together"
          in portable_source,
          "portable permits a mismatched Rust binary override")
    package_version = run(
        base, "-c", "import floe; print(floe.__version__)").stdout.strip()
    renderd_expected = run(
        base, "-c",
        "import floe; print(floe.RENDERD_VERSION)").stdout.strip()
    index_version = cargo_package_version(ROOT / "rust" / "cli" /
                                          "Cargo.toml")
    renderd_version = cargo_package_version(ROOT / "rust" / "renderd" /
                                            "Cargo.toml")
    # the renderd handshake guard compares RENDERD_VERSION to the built
    # binary, so it must equal both Rust package versions (2026-08-30)
    check(renderd_expected == index_version == renderd_version,
          "RENDERD_VERSION/index/renderd mismatch: %s/%s/%s" %
          (renderd_expected, index_version, renderd_version))

    def _ver(s):
        return tuple(int(p) for p in s.split(".") if p.isdigit())
    # __version__ is the display version (bumped every push); it must
    # never regress below the Rust binary version
    check(_ver(package_version) >= _ver(renderd_expected),
          "__version__ %s is behind RENDERD_VERSION %s" %
          (package_version, renderd_expected))
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
          launcher_source and 'exec "$RT/bin/floe2" "$@"' in
          launcher_source,
          "portable launcher does not dispatch by floe/floe2 basename")
    with tempfile.TemporaryDirectory(
            prefix="floe-portable-launcher-") as td:
        bundle = Path(td)
        (bundle / "runtime" / "bin").mkdir(parents=True)
        echo = shutil.which("echo")
        check(echo is not None, "portable launcher test needs echo")
        os.symlink(echo, bundle / "runtime" / "bin" / "python3")
        # the Rust floe2 stand-in says what it was given and which
        # interpreter it would start the viewer with
        fake = bundle / "runtime" / "bin" / "floe2"
        fake.write_text('#!/bin/sh\necho "rust $* python=$FLOE_GTK_PYTHON"\n')
        fake.chmod(0o755)
        python3 = bundle / "runtime" / "bin" / "python3"
        for product, want in (
                ("floe", "-m floe view chip.oas"),
                ("floe2", "rust view chip.oas python=%s" % python3)):
            target = bundle / product
            shutil.copy2(launcher, target)
            target.chmod(0o755)
            launched = subprocess.run(
                [str(target), "view", "chip.oas"], cwd=bundle,
                env=dict(base, XDG_CACHE_HOME=str(bundle / "cache")),
                # a loaded host (the battery, load 6+) took over 10 s
                capture_output=True, text=True, timeout=30)
            check(launched.returncode == 0 and
                  launched.stdout.strip() == want,
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
              linked.stdout.strip() == "rust --version python=%s" % python3,
              "portable floe2 convenience symlink lost its runtime")
    conflict_env = dict(base, FLOE_PORTABLE_PRODUCT="floe2",
                        FLOE_PORTABLE_KLAYOUT="1")
    conflict = subprocess.run(
        ["bash", str(portable)], cwd=ROOT, env=conflict_env,
        capture_output=True, text=True, timeout=10)
    check(conflict.returncode != 0 and "Rust-only" in conflict.stdout,
          "portable allowed a floe2/KLayout product mixture")

    real_index = ROOT / "rust" / "target" / "release" / "floe-index"
    with tempfile.TemporaryDirectory(prefix="floe2-cli-") as td:
        work = Path(td)
        log = work / "calls.json"
        binary = work / "floe-index"
        # it answers --version as the real one: the Rust command line asks
        # its indexer's version before it runs it
        binary.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, pathlib, sys\n"
            "if sys.argv[1:] == ['--version']:\n"
            "    os.execv(%r, [%r, '--version'])\n"
            "pathlib.Path(os.environ['FLOE2_CALL']).write_text("
            "json.dumps(sys.argv[1:]))\n" % (str(real_index),
                                              str(real_index)),
            encoding="utf-8")
        binary.chmod(0o755)
        source = work / "design with spaces.oas"
        source.write_bytes(b"fixture")
        env = dict(base, FLOE_INDEX_BIN=str(binary), FLOE2_CALL=str(log))
        rust(env, "index", source, "--jobs", "2")
        # a layout indexes without the occupancy summary into the
        # hidden sibling .<src>.ice (2026-09-16)
        check(json.loads(log.read_text()) == [
            "vfs", str(source), vfs_cache_dir(source), "--jobs", "2",
            "--no-lod",
        ], "floe2 changed the canonical Rust index argv: %s" % log.read_text())

    if fixture is not None:
        validate_runtime(base, Path(fixture).resolve())
        validate_rust_cli(base, Path(fixture).resolve())
    print("FLOE2 PRODUCT VALIDATION: ALL OK")


if __name__ == "__main__":
    if len(sys.argv) > 2:
        raise SystemExit("usage: validate_floe2.py [fixture.oas]")
    main(sys.argv[1] if len(sys.argv) == 2 else None)
