"""Read-only analysis, cache identity, and preflight CLI contracts."""

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import cachepath, drc, drc_analysis, drc_prepare  # noqa: E402


class AnalysisTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="floe-analysis-test-")
        self.addCleanup(self.temp.cleanup)
        self.source = Path(self.temp.name) / "test.db"
        shutil.copyfile(ROOT / "sample.db", self.source)
        subprocess.run([str(ROOT / "rust/target/release/floe-index"), "drc", str(self.source)],
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=True)
        self.pack = cachepath.pack_path(self.source)

    def test_readonly_analysis_never_creates_or_changes_review_files(self):
        side = drc.waive_autosave_path(self.pack)
        original = Path(self.pack).read_bytes()
        db = drc.IcePack(self.pack, review=False)
        self.addCleanup(db.close)
        self.assertFalse(os.path.exists(side))
        self.assertEqual(db.status_counts(0), (0, 20))
        for action in (lambda: db.set_status(0, 0, 1), lambda: db.set_note([0], "note"),
                       lambda: db.clear_note([0])):
            with self.assertRaisesRegex(OSError, "read-only"):
                action()
        self.assertEqual(Path(self.pack).read_bytes(), original)
        self.assertFalse(os.path.exists(side))

    def test_readonly_explicit_review_mapping_tracks_current_parent_status(self):
        parent = drc.IcePack(self.pack)
        self.addCleanup(parent.close)
        parent.set_status(0, 0, 1)
        child = drc.IcePack(self.pack, review=False, review_path=parent._waive_path)
        self.addCleanup(child.close)
        self.assertEqual(child.get_status(0, 0), 1)
        parent.set_status(0, 1, 1)
        self.assertEqual(child.status_counts(0), (2, 20))
        broken = Path(self.temp.name) / "foreign.waive"
        broken.write_bytes(b"bad")
        with self.assertRaises(ValueError):
            drc.IcePack(self.pack, review=False, review_path=str(broken))

    def test_rule_bounds_cache_and_identity_survive_review_only_changes(self):
        db = drc.IcePack(self.pack)
        self.addCleanup(db.close)
        identity = drc_analysis.pack_identity(self.pack)
        self.assertEqual(identity, db._analysis_identity)
        np.testing.assert_array_equal(drc_analysis.load_rule_bounds(self.pack, 8), db._cbb)
        with patch.object(drc_analysis, "save_rule_bounds", side_effect=AssertionError("rescanned")):
            again = drc.IcePack(self.pack, review=False)
            self.addCleanup(again.close)
            np.testing.assert_array_equal(again._cbb, db._cbb)
        db.set_status(0, 0, 1)
        self.assertEqual(identity, drc_analysis.pack_identity(self.pack))
        stat = os.stat(self.pack)
        os.utime(self.pack, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000))
        self.assertNotEqual(identity, drc_analysis.pack_identity(self.pack))
        self.assertIsNone(drc_analysis.load_rule_bounds(self.pack, 8))

    def test_override_separates_same_named_packs_and_rejects_stale_source(self):
        root = os.path.join(self.temp.name, "local-cache")
        with patch.dict(os.environ, {"FLOE_DRC_ANALYSIS_ROOT": root}):
            a = cachepath.drc_analysis_dir(self.pack)
            b = cachepath.drc_analysis_dir(os.path.join(self.temp.name, "elsewhere", ".test.db.tray"))
            self.assertEqual(os.path.dirname(a), root)
            self.assertNotEqual(a, b)
        with open(self.source, "a") as stream:
            stream.write("\n")
        with self.assertRaisesRegex(ValueError, "stale pack"):
            drc_prepare.open_pack(self.source)

    def test_prepare_cli_builds_reusable_indices_without_review_sidecar(self):
        env = dict(os.environ, FLOE_DRC_ANALYSIS_ROOT=os.path.join(self.temp.name, "analysis"))
        command = [sys.executable, "-m", "floe2", "drc-prepare", str(self.source),
                   "--svrf", str(ROOT / "sample.svrf.rules.json"), "--rule", "M1.SPACE.1"]
        result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("absolute: 5 groups ready", result.stderr)
        self.assertIn("percent: 5 groups ready", result.stderr)
        self.assertFalse(os.path.exists(drc.waive_autosave_path(self.pack)))
        with patch.dict(os.environ, env):
            directory = Path(cachepath.drc_analysis_dir(self.pack))
            files = {str(path): path.stat().st_mtime_ns for path in directory.rglob("*") if path.is_file()}
        result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for path, stamp in files.items():
            self.assertEqual(os.stat(path).st_mtime_ns, stamp, "prepared geometry was rebuilt")


if __name__ == "__main__":
    unittest.main(verbosity=2)
