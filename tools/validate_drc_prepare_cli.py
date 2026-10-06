"""End-to-end contracts for the floe-index preprocessing command.

Only temporary copies of the synthetic sample and portable layouts are used.
Run with a rebuilt release binary: python tools/validate_drc_prepare_cli.py
"""

import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import cachepath, drc, drc_delta, drc_delta_cache, svrf  # noqa: E402

BIN = ROOT / "rust/target/release/floe-index"


@unittest.skipUnless(BIN.is_file(), "build rust/target/release/floe-index first")
class PrepareCommandTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="floe-prepare-command-")
        self.addCleanup(self.temp.cleanup)
        self.folder = Path(self.temp.name)
        self.cwd = self.folder / "unrelated working directory 작업"
        self.cwd.mkdir()
        self.env = dict(os.environ)
        for key in ("FLOE_INDEX_BIN", "FLOE_PYTHON_BIN", "FLOE_DRC_JOBS",
                    "PYTHONPATH", "PYTHONHOME"):
            self.env.pop(key, None)
        self.env.update(FLOE_PYTHON_BIN=sys.executable,
                        FLOE_DRC_ANALYSIS_ROOT=str(self.folder / "analysis cache 분석"))

    def command(self, *arguments, binary=BIN, env=None, timeout=20):
        return subprocess.run([str(binary), "drc-prepare", *map(str, arguments)],
                              cwd=self.cwd, env=env or self.env,
                              capture_output=True, text=True, timeout=timeout)

    def sample(self):
        source = self.folder / "오류 목록 sample.db"
        rules = self.folder / "측정 규칙 sample.rules.json"
        shutil.copyfile(ROOT / "sample.db", source)
        shutil.copyfile(ROOT / "sample.svrf.rules.json", rules)
        result = subprocess.run([str(BIN), "drc", str(source)],
                                env=self.env, capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return source, Path(cachepath.pack_path(source)), rules

    def python_wrapper(self, path, *, status=None, portable=False):
        """A real executable that records forwarding without a shell parser."""
        path.parent.mkdir(parents=True, exist_ok=True)
        code = ("#!%s\n" % sys.executable +
                "import json, os, sys\n"
                "from pathlib import Path\n"
                "trace = os.environ.get('FLOE_CLI_TRACE')\n"
                "if trace:\n"
                "    Path(trace).write_text(json.dumps({'argv': sys.argv[1:], "
                "'runtime': sys.argv[0], "
                "'pid': os.getpid(), 'cwd': os.getcwd(), "
                "'index': os.environ.get('FLOE_INDEX_BIN'), "
                "'pythonpath': os.environ.get('PYTHONPATH'), "
                "'pythonhome': os.environ.get('FLOE_TEST_PORTABLE_HOME', "
                "os.environ.get('PYTHONHOME')), "
                "'nousersite': os.environ.get('PYTHONNOUSERSITE'), "
                "'librarypath': os.environ.get('LD_LIBRARY_PATH')}))\n")
        if status is None:
            code += "os.execv(%r, [%r, *sys.argv[1:]])\n" % (sys.executable, sys.executable)
        else:
            code += "raise SystemExit(%d)\n" % status
        if portable:
            # PYTHONHOME must be cleared before the test interpreter starts.
            # The fixture has app packages, not a second Python distribution.
            script = path.with_name(path.name + ".py")
            script.write_text(code, encoding="utf-8")
            path.write_text("#!/bin/sh\n"
                            "export FLOE_TEST_PORTABLE_HOME=\"$PYTHONHOME\"\n"
                            "unset PYTHONHOME\n"
                            "exec %s %s \"$@\"\n" %
                            (shlex.quote(sys.executable), shlex.quote(str(script))))
        else:
            path.write_text(code, encoding="utf-8")
        path.chmod(0o700)
        return path

    @unittest.skipUnless(os.name == "posix", "Unix launcher replaces itself with Python")
    def test_runtime_override_forwards_exact_arguments_and_replaces_same_process(self):
        trace = self.folder / "runtime trace.json"
        wrapper = self.python_wrapper(self.folder / "파이썬 실행기 with spaces", status=23)
        self.env.update(FLOE_PYTHON_BIN=str(wrapper), FLOE_CLI_TRACE=str(trace),
                        PYTHONPATH=str(self.folder / "existing package path"))
        arguments = ["파일 이름.db", "--svrf", "상대 경로.rules.json",
                     "--rule", "M1 width 공백", "--rule", "M2.NOTCH", "--jobs", "3"]
        process = subprocess.Popen([str(BIN), "drc-prepare", *arguments], cwd=self.cwd,
                                   env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True)
        stdout, stderr = process.communicate(timeout=10)
        self.assertEqual(process.returncode, 23, stdout + stderr)
        recorded = json.loads(trace.read_text())
        self.assertEqual(recorded["argv"][0], "-c")
        self.assertEqual(recorded["argv"][2:], arguments)
        self.assertEqual(recorded["pid"], process.pid)
        self.assertEqual(Path(recorded["cwd"]).resolve(), self.cwd.resolve())
        self.assertEqual(Path(recorded["index"]).resolve(), BIN.resolve())
        self.assertEqual(recorded["pythonpath"].split(os.pathsep),
                         [str(ROOT), self.env["PYTHONPATH"]])

    def test_adjacent_portable_runtime_and_package_work_from_foreign_directory(self):
        source, _packed, rules = self.sample()
        checkout = self.folder / "parent checkout"
        (checkout / "floe").mkdir(parents=True)
        (checkout / "rust").mkdir()
        (checkout / "rust" / "Cargo.toml").write_text("")
        (checkout / "floe" / "__init__.py").write_text("")
        (checkout / "floe" / "drc_prepare.py").write_text(
            "raise RuntimeError('checkout package shadowed the portable package')\n")
        runtime = checkout / "portable 설치 경로" / "runtime"
        binary = runtime / "bin" / "floe-index"
        binary.parent.mkdir(parents=True)
        shutil.copy2(BIN, binary)
        self.python_wrapper(runtime / "bin" / "python3", portable=True)
        # Supply only the app files through the copied distribution's package
        # slot; the wrapper uses the current test interpreter for NumPy.
        package_root = runtime / "lib" / ("python%d.%d" % sys.version_info[:2]) / "site-packages"
        package_root.mkdir(parents=True)
        (package_root / "floe").symlink_to(ROOT / "floe", target_is_directory=True)
        decoy = self.folder / "other virtual environment"
        self.python_wrapper(decoy / "bin" / "python", status=92)
        trace = self.folder / "portable trace.json"
        env = dict(self.env)
        env.pop("FLOE_PYTHON_BIN", None)
        env.update(VIRTUAL_ENV=str(decoy), PYTHONPATH=str(package_root),
                   FLOE_CLI_TRACE=str(trace), LD_LIBRARY_PATH="/existing/library/path")
        result = self.command(source, "--svrf", rules, "--rule", "M1.SPACE.1", "--jobs", 1,
                              "--backend", "rust", binary=binary, env=env)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Rust", result.stderr)
        self.assertIn("absolute: 5 groups ready", result.stderr)
        recorded = json.loads(trace.read_text())
        self.assertEqual(Path(recorded["index"]).resolve(), binary.resolve())
        self.assertEqual(Path(recorded["pythonhome"]).resolve(), runtime.resolve())
        self.assertEqual(recorded["nousersite"], "1")
        library_paths = recorded["librarypath"].split(os.pathsep)
        self.assertEqual(Path(library_paths[0]).resolve(), (runtime / "lib").resolve())
        self.assertEqual(library_paths[1:], ["/existing/library/path"])
        self.assertEqual(recorded["pythonpath"], str(package_root))

    def test_active_environment_precedes_ordinary_adjacent_python(self):
        binary = self.folder / "standalone tools" / "bin" / "floe-index"
        binary.parent.mkdir(parents=True)
        shutil.copy2(BIN, binary)
        self.python_wrapper(binary.parent / "python3", status=91)
        active = self.folder / "active Python environment"
        selected = self.python_wrapper(active / "bin" / "python", status=23)
        trace = self.folder / "active runtime trace.json"
        for variable in ("VIRTUAL_ENV", "CONDA_PREFIX"):
            with self.subTest(environment=variable):
                env = dict(self.env)
                for key in ("FLOE_PYTHON_BIN", "VIRTUAL_ENV", "CONDA_PREFIX"):
                    env.pop(key, None)
                env.update({variable: str(active), "FLOE_CLI_TRACE": str(trace)})
                result = self.command("--help", binary=binary, env=env)
                self.assertEqual(result.returncode, 23, result.stdout + result.stderr)
                recorded = json.loads(trace.read_text())
                self.assertEqual(Path(recorded["runtime"]).resolve(), selected.resolve())
                self.assertIsNone(recorded["pythonhome"])

    def test_invalid_explicit_runtime_fails_without_silent_fallback(self):
        for path in (self.folder / "missing python", self.folder / "non executable python"):
            if "non executable" in path.name:
                path.write_text("not executable")
            with self.subTest(runtime=path.name):
                env = dict(self.env, FLOE_PYTHON_BIN=str(path))
                result = self.command("--help", env=env)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("FLOE_PYTHON_BIN", result.stderr)
                self.assertNotIn("Traceback", result.stderr)

    def test_missing_dependency_and_outdated_package_report_runtime_guidance(self):
        binary = self.folder / "standalone tools" / "floe-index"
        binary.parent.mkdir()
        shutil.copy2(BIN, binary)
        package_root = self.folder / "incomplete packages"
        package = package_root / "floe"
        package.mkdir(parents=True)
        (package / "__init__.py").write_text("")
        env = dict(self.env, PYTHONPATH=str(package_root))
        for contents in ("raise ModuleNotFoundError(\"No module named 'numpy'\")\n",
                         "def main(*args, **kwargs):\n"
                         "    raise ModuleNotFoundError(\"No module named 'numpy'\")\n",
                         "# An older package without the new main entry point.\n"):
            with self.subTest(contents=contents):
                (package / "drc_prepare.py").write_text(contents)
                result = self.command("--help", binary=binary, env=env)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("FLOE_PYTHON_BIN", result.stderr)
                self.assertIn("NumPy", result.stderr)
                self.assertNotIn("Traceback", result.stderr)

    def test_cold_native_cache_and_warm_reuse_from_external_unicode_directory(self):
        source, packed, rules = self.sample()
        original = packed.read_bytes()
        old = self.folder / "old index helper"
        marker = self.folder / "old-helper-called"
        old.write_text("#!%s\nfrom pathlib import Path\nPath(%r).touch()\n"
                       "raise SystemExit(91)\n" % (sys.executable, str(marker)))
        old.chmod(0o700)
        self.env["FLOE_INDEX_BIN"] = str(old)
        result = self.command(source, "--svrf", rules, "--rule", "M1.SPACE.1", "--jobs", 2)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Rust", result.stderr)
        self.assertIn("absolute: 5 groups ready", result.stderr)
        self.assertIn("percent: 5 groups ready", result.stderr)
        self.assertFalse(marker.exists(), "launcher selected the stale FLOE_INDEX_BIN override")
        self.assertEqual(packed.read_bytes(), original)
        self.assertFalse(any(".waive" in path.name for path in self.folder.rglob("*")))
        with patch.dict(os.environ, self.env):
            db = drc.IcePack(str(packed), review=False)
            self.addCleanup(db.close)
            ci = next(i for i, check in enumerate(db.checks) if check.name == "M1.SPACE.1")
            constraints = svrf.load_rules(str(rules))["checks"]["M1.SPACE.1"]["constraints"]
            reference = drc_delta.DeltaIndex(db, ci, constraints).measure()
            cached = drc_delta.DeltaIndex(db, ci, constraints)
            self.assertTrue(drc_delta_cache.load_measurements(cached))
            np.testing.assert_array_equal(cached.measured_ticks, reference.measured_ticks)
            np.testing.assert_array_equal(cached.constraint_indices, reference.constraint_indices)
            np.testing.assert_array_equal(cached.estimated_flags, reference.estimated_flags)
            directory = Path(cachepath.drc_analysis_dir(packed))
            stamps = {path: path.stat().st_mtime_ns
                      for path in directory.rglob("*") if path.is_file()}
        result = self.command(packed, "--svrf", rules, "--rule", "M1.SPACE.1", "--jobs", 1,
                              "--backend", "rust")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual({path: path.stat().st_mtime_ns for path in stamps}, stamps)
        self.assertFalse(marker.exists())

    def test_help_and_argument_error_exit_codes(self):
        result = self.command("--help")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for option in ("--svrf", "--jobs", "--rule", "--spatial-only", "--delta-only"):
            self.assertIn(option, result.stdout)
        for arguments in ((), ("x.tray", "--unknown-option"),
                          ("x.tray", "--backend", "invalid"),
                          ("x.tray", "--spatial-only", "--delta-only"),
                          ("x.tray", "--jobs", "not-an-integer")):
            with self.subTest(arguments=arguments):
                result = self.command(*arguments)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertNotIn("Traceback", result.stderr)

    def test_preparation_errors_have_clean_failure_exit_codes(self):
        source, _packed, rules = self.sample()
        arguments = [(source, "--spatial-only", "--jobs", "0"),
                     (source, "--svrf", rules, "--rule", "missing rule 없는 룰"),
                     (source,),
                     (self.folder / "absent.db", "--spatial-only")]
        for case in arguments:
            with self.subTest(arguments=case):
                result = self.command(*case)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("DRC preprocessing failed", result.stderr)
                self.assertNotIn("Traceback", result.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
