"""Read-only analysis, cache identity, and preflight CLI contracts."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import cachepath, drc, drc_analysis, drc_prepare  # noqa: E402
from floe import drc_delta, drc_delta_cache, drc_spatial, svrf  # noqa: E402


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


class EmptyPreparationTests(unittest.TestCase):
    """Empty checks must not start per-rule analysis, even with SVRF bounds."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="floe-empty-analysis-")
        self.addCleanup(self.temp.cleanup)
        self.folder = Path(self.temp.name)
        self.names = ["EMPTY.%03d" % number for number in range(100)]
        self.rules = self.folder / "rules.json"
        constraint = {"metric": "width", "op": "<", "value": 0.05,
                      "text": "INTERNAL M1 < 0.05"}
        self.rules.write_text(json.dumps({
            "format": svrf.FORMAT, "version": svrf.VERSION,
            "checks": {name: {"constraints": [constraint]}
                       for name in self.names + ["LIVE"]}}))
        self.empty = self._pack("empty", live=False)
        self.mixed = self._pack("mixed", live=True)

    def _pack(self, name, live):
        source = self.folder / (name + ".db")
        lines = ["MAIN 100000\n"]
        for number, rule in enumerate(self.names):
            # Nonzero declared counts cannot create actual geometry.
            declared = 99 if number == 0 else 0
            lines.append("%s\n%d %d 0\n" % (rule, declared, declared))
        if live:
            # Conversely, a zero header must not hide a real error record.
            lines.append("LIVE\n0 0 0\np 1 4\n0 0\n2000 0\n2000 10000\n0 10000\n")
        source.write_text("".join(lines))
        subprocess.run([str(ROOT / "rust/target/release/floe-index"), "drc", str(source)],
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=True)
        return source

    def test_hundred_empty_rules_skip_every_preprocessor_and_constraint_index(self):
        db = drc_prepare.open_pack(self.empty)
        try:
            self.assertEqual(len(db.checks), 100)
            self.assertEqual(db.total, 0)
            self.assertEqual(db.checks[0].declared, 99)
            self.assertTrue(all(len(check.errors) == 0 for check in db.checks))
        finally:
            db.close()
        reports = []
        forbidden = AssertionError("empty rule started per-rule analysis")
        with patch.object(drc_spatial, "prepare_rule", side_effect=forbidden), \
                patch.object(drc_delta, "DeltaIndex", side_effect=forbidden), \
                patch.object(drc_delta_cache, "process_measure", side_effect=forbidden), \
                patch.object(drc_delta_cache, "prepare_group_cache", side_effect=forbidden):
            drc_prepare.prepare(self.empty, self.rules, report=reports.append)
        self.assertIn("Skipping 100 empty rules", reports)
        self.assertEqual(reports[-1],
                         "DRC analysis ready: 0 rules prepared; 100 empty rules skipped")
        self.assertLess(len(reports), 6, "empty rules should not produce per-rule progress noise")

    def test_mixed_selected_and_single_mode_preparation_touch_only_actual_errors(self):
        cases = [
            ({}, 1, 100, True, True),
            ({"selected": (self.names[0], "LIVE")}, 1, 1, True, True),
            ({"selected": (self.names[0], self.names[-1])}, 0, 2, True, True),
            ({"selected": (self.names[0], "LIVE"), "delta": False}, 1, 1, True, False),
            ({"selected": (self.names[0], "LIVE"), "spatial": False}, 1, 1, False, True),
        ]
        for options, prepared, skipped, spatial, delta in cases:
            with self.subTest(options=options):
                reports = []

                def nonempty_index(db, ci, constraints):
                    self.assertEqual(db.checks[ci].name, "LIVE")
                    self.assertEqual(db.checks[ci].declared, 0)
                    self.assertEqual(len(db.checks[ci].errors), 1)
                    return SimpleNamespace(db=db, ci=ci, constraints=constraints)

                with patch.object(drc_spatial, "prepare_rule") as spatial_builder, \
                        patch.object(drc_delta, "DeltaIndex", side_effect=nonempty_index) as index_builder, \
                        patch.object(drc_delta_cache, "process_measure") as measure, \
                        patch.object(drc_delta_cache, "prepare_group_cache", return_value=0) as group:
                    drc_prepare.prepare(self.mixed, self.rules if delta else None,
                                        report=reports.append, **options)
                self.assertEqual(spatial_builder.call_count, prepared * int(spatial))
                for call in spatial_builder.call_args_list:
                    self.assertEqual(call.args[1], 100)
                self.assertEqual(index_builder.call_count, prepared * int(delta))
                self.assertEqual(measure.call_count, prepared * int(delta))
                self.assertEqual(group.call_count, prepared * int(delta) * 2)
                if group.call_count:
                    self.assertEqual([call.kwargs["mode"] for call in group.call_args_list],
                                     ["absolute", "percent"])
                self.assertEqual(reports[-1],
                    "DRC analysis ready: %d rules prepared; %d empty rules skipped" %
                    (prepared, skipped))

    def test_missing_selected_rule_is_rejected_before_empty_shortcut(self):
        with self.assertRaisesRegex(ValueError, "unknown DRC rules: MISSING"):
            drc_prepare.prepare(self.empty, self.rules,
                                selected=(self.names[0], "MISSING"), report=lambda _: None)

    def test_empty_prepare_cli_cold_and_warm_stay_without_per_rule_cache(self):
        env = dict(os.environ, FLOE_DRC_ANALYSIS_ROOT=str(self.folder / "analysis"))
        command = [sys.executable, "-m", "floe2", "drc-prepare", str(self.empty),
                   "--svrf", str(self.rules)]
        elapsed = []
        for _ in range(2):
            start = time.perf_counter()
            result = subprocess.run(command, cwd=ROOT, env=env,
                                    capture_output=True, text=True, timeout=10)
            elapsed.append(time.perf_counter() - start)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("Skipping 100 empty rules", result.stderr)
            self.assertIn("0 rules prepared; 100 empty rules skipped", result.stderr)
            self.assertNotIn("groups ready", result.stderr)
            self.assertNotIn("spatial ready", result.stderr)
        with patch.dict(os.environ, env):
            directory = Path(cachepath.drc_analysis_dir(cachepath.pack_path(self.empty)))
        self.assertFalse((directory / "delta").exists())
        self.assertFalse((directory / "spatial").exists())
        self.assertFalse(os.path.exists(drc.waive_autosave_path(cachepath.pack_path(self.empty))))
        print("100 empty rules CLI: cold %.4f s, warm %.4f s" % tuple(elapsed))


if __name__ == "__main__":
    unittest.main(verbosity=2)
