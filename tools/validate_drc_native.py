"""Rust DRC measurement differential and process/cache integration gate.

All geometry is synthetic and all packs, plans, caches and review files stay
in temporary folders. Python measurement remains the independent reference.
Run after rebuilding floe-index: python tools/validate_drc_native.py
"""

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import cachepath, drc, drc_delta as delta  # noqa: E402
from floe import drc_delta_cache as cache, drc_native as native  # noqa: E402


PRECISION = 1_000_000
BIN = ROOT / "rust/target/release/floe-index"
FIND_BINARY = native.find_binary


def rectangle(width, height, x=0, y=0):
    """Integer DBU coordinates avoid fixture rounding before pack creation."""
    return "p", [(x, y), (x + width, y), (x + width, y + height), (x, y + height)]


def translated(geometry, x, y):
    kind, points = geometry
    return kind, [(px + x, py + y) for px, py in points]


def constraint(metric="width", bound=0.05, op="<", text=None):
    head = {"width": "INTERNAL M1", "space": "EXTERNAL M1 M2",
            "notch": "EXTERNAL M1", "enclosure": "ENCLOSURE M1 M2",
            "overlap": "INTERNAL M1 M2", "extension": "ENCLOSURE M1 M2",
            "area": "AREA M1", "length": "LENGTH M1"}.get(metric, metric)
    return {"metric": metric, "value": bound, "op": op,
            "text": text or "%s %s %s" % (head, op, bound)}


def cases():
    rectangles = [rectangle(20000, 100000), rectangle(100000, 20000),
                  rectangle(20000, 30000), rectangle(30000, 20000),
                  rectangle(50000, 100000), rectangle(50000, 50000),
                  rectangle(49999, 50001), rectangle(50001, 49999),
                  rectangle(100000, 120000), rectangle(20000, 20000),
                  rectangle(0, 100000)]
    # Python accepts all four distinct rectangle corners regardless of order.
    rectangles += [("p", [(0, 0), (20000, 100000), (20000, 0), (0, 100000)]),
                   ("p", [(0, 0), (20000, 0), (0, 100000)]),
                   ("p", [(0, 0), (20000, 0), (20000, 100000),
                          (0, 100000), (0, 0)])]
    edges = [
        ("e", [(0, 0), (0, 100000), (20000, 0), (20000, 100000)]),
        ("e", [(0, 100000), (0, 0), (20000, 100000), (20000, 0)]),
        ("e", [(0, 0), (100000, 0), (0, 20000), (100000, 20000)]),
        ("e", [(0, 0), (100000, 0), (20000, 10000), (80000, 30000)]),
        ("e", [(0, 0), (10000, 0), (30000, 20000), (40000, 20000)]),
        ("e", [(0, 0), (30000, 30000), (0, 30000), (30000, 0)]),
        ("e", [(0, 0), (10000, 0), (10000, 0), (20000, 0)]),
        ("e", [(0, 0), (0, 0), (20000, 0), (20000, 10000)]),
        ("e", [(0, 0), (0, 0), (20000, 0), (20000, 0)]),
        ("e", [(0, 0), (100000, 1), (0, 20000), (100000, 20001)]),
    ]
    half_ticks = [rectangle(5, 100000), rectangle(15, 100000),
                  rectangle(20005, 100000), rectangle(49995, 100000),
                  rectangle(50005, 100000)]
    large = [translated(shape, origin, -origin)
             for origin in (10**15, -10**15, 10**18, -10**18)
             for shape in (rectangle(20000, 100000), rectangle(50000, 100000), edges[0])]
    polygon = ("p", [(0, 0), (30000, 0), (30000, 10000),
                     (10000, 10000), (10000, 30000), (0, 30000)])
    polygons = [rectangle(20000, 100000),
                ("p", [(0, 0), (40000, 0), (0, 30000)]), polygon,
                ("p", list(reversed(polygon[1]))),
                ("p", polygon[1] + [polygon[1][0]]),
                translated(polygon, 10**15, -10**15),
                ("p", [(0, 0), (30000, 30000), (0, 30000), (30000, 0)]),
                ("p", [(0, 0), (30000, 0)]), edges[0]]
    specs = []
    for op in ("<", "<=", ">", ">=", "==", "!="):
        specs.append(("RECT.%s" % {"<": "LT", "<=": "LE", ">": "GT",
                                  ">=": "GE", "==": "EQ", "!=": "NE"}[op],
                      [constraint(op=op)], rectangles))
    chain = "INTERNAL M1 > 0.01 < 0.05"
    specs += [
        ("RANGE", [constraint(bound=0.01, op=">", text=chain),
                   constraint(bound=0.05, op="<", text=chain)], rectangles),
        ("OPTIONS", [constraint(text="INTERNAL M1 < 0.05 OPPOSITE")], rectangles),
        ("UNKNOWN", [constraint(), constraint(bound=None,
                     text="INTERNAL M2 < UNKNOWN")], rectangles),
        ("CONFLICT", [constraint(bound=0.03), constraint(bound=0.01, op=">")], rectangles),
        ("DUPLICATE", [constraint(bound="0.05000", text="INT M1 < 0.05"),
                       constraint(bound=0.05)], rectangles),
        ("HALF", [constraint()], half_ticks + large),
        ("NO.MATCH", [constraint(bound=0.000001)], rectangles),
        ("AREA", [constraint("area", 0.01)], polygons),
        ("LENGTH", [constraint("length", 0.05)],
         [("e", [(0, 0), (0, 0)]), ("e", [(0, 0), (30000, 40000)]),
          ("e", [(0, 0), (3000, 4000)]), ("e", [(0, 0), (1, 1)]),
          ("e", [(0, 0), (5, 0)]), rectangle(20000, 100000)]),
    ]
    for metric in ("space", "notch", "enclosure", "overlap", "extension"):
        text = "EXTERNAL M1 < 0.05 NOTCH" if metric == "notch" else None
        specs.append((metric.upper(), [constraint(metric, text=text)], edges + rectangles[:3]))
    specs += [("EMPTY", [constraint()], []),
              ("MULTIBLOCK", [constraint()], (rectangles + edges) * 9)]
    return specs


def write_pack(folder, specs):
    source, packed = Path(folder) / "results.db", Path(folder) / "results.tray"
    with source.open("w", encoding="utf-8") as stream:
        stream.write("NATIVE_TEST %d\n" % PRECISION)
        for name, _constraints, geometries in specs:
            stream.write("%s\n%d %d 0\n" % (name, len(geometries), len(geometries)))
            for ordinal, (kind, points) in enumerate(geometries, 1):
                stream.write("%s %d %d\n" % (kind, ordinal, len(points)))
                for x, y in points:
                    stream.write("%d %d\n" % (x, y))
    subprocess.run([str(BIN), "drc", str(source), str(packed)],
                   check=True, capture_output=True)
    return packed


@unittest.skipUnless(BIN.is_file(), "build rust/target/release/floe-index first")
class NativeMeasurementTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        protocol = subprocess.run([str(BIN), "drc-measure", "--protocol"],
                                  capture_output=True, text=True, timeout=5)
        if protocol.returncode or protocol.stdout.strip() != "1":
            raise RuntimeError("rebuild floe-index with drc-measure protocol 1")
        cls.fixture = tempfile.TemporaryDirectory(prefix="floe-native-fixture-")
        cls.specs = cases()
        cls.pack = write_pack(cls.fixture.name, cls.specs)
        cls.by_name = {spec[0]: ci for ci, spec in enumerate(cls.specs)}

    @classmethod
    def tearDownClass(cls):
        cls.fixture.cleanup()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="floe-native-test-")
        self.addCleanup(self.temp.cleanup)
        self.folder = Path(self.temp.name)
        self.db = drc.IcePack(str(self.pack), review=False)
        self.addCleanup(self.db.close)
        env = patch.dict(os.environ, {"FLOE_DRC_ANALYSIS_ROOT": str(self.folder / "analysis")})
        env.start()
        self.addCleanup(env.stop)
        locate = patch.object(native, "find_binary", return_value=str(BIN))
        locate.start()
        self.addCleanup(locate.stop)

    def index(self, name):
        ci = self.by_name[name]
        return delta.DeltaIndex(self.db, ci, self.specs[ci][1])

    def assert_measurement_equal(self, actual, expected):
        np.testing.assert_array_equal(actual.measured_ticks, expected.measured_ticks)
        np.testing.assert_array_equal(actual.constraint_indices, expected.constraint_indices)
        np.testing.assert_array_equal(actual.estimated_flags, expected.estimated_flags)
        self.assertFalse(np.any(actual.constraint_indices < -1),
                         "selective-fallback sentinels escaped the published cache")

    def test_supported_geometry_and_constraint_chains_match_python(self):
        for name, _constraints, _geometries in self.specs:
            with self.subTest(rule=name):
                expected = self.index(name).measure()
                pids = []
                actual = cache.process_measure(self.index(name), backend="rust", jobs=1,
                                               process_callback=pids.append)
                self.assert_measurement_equal(actual, expected)
                self.assertEqual(actual._automatic_step("absolute", None),
                                 expected._automatic_step("absolute", None))
                self.assertEqual(actual._automatic_step("percent", None),
                                 expected._automatic_step("percent", None))
                if len(expected.measured_ticks):
                    self.assertTrue(any(pid is not None for pid in pids))
                    self.assertIsInstance(actual.measured_ticks, np.memmap)
                else:
                    self.assertEqual(pids, [])

    def test_jobs_one_and_four_match_across_many_independent_blocks(self):
        # > 2 * 1024 blocks forces multiple native worker ranges, not merely
        # passing --jobs 4 to a pack small enough for one worker.
        shape = rectangle(20000, 100000)
        count = 131201
        parallel = self.folder / "parallel"
        parallel.mkdir()
        path = write_pack(parallel, [("PARALLEL", [constraint()], [shape] * count)])
        db = drc.IcePack(str(path), review=False)
        self.addCleanup(db.close)
        expected = delta.DeltaIndex(db, 0, [constraint()]).measure()
        results = []
        for jobs in (1, 4):
            with patch.dict(os.environ, {"FLOE_DRC_ANALYSIS_ROOT": str(parallel / ("jobs%d" % jobs))}):
                actual = cache.process_measure(delta.DeltaIndex(db, 0, [constraint()]),
                                               backend="rust", jobs=jobs)
                self.assert_measurement_equal(actual, expected)
                results.append(tuple(array.tobytes() for array in
                    (actual.measured_ticks, actual.constraint_indices, actual.estimated_flags)))
        self.assertEqual(results[0], results[1])

    def test_seeded_skew_edgepairs_match_python(self):
        rng = np.random.default_rng(20261006)
        shapes = []
        for ordinal, coordinates in enumerate(rng.integers(-100000, 100001, (1200, 4, 2))):
            points = [tuple(map(int, point)) for point in coordinates]
            if ordinal % 11 == 0:
                points[1] = points[0]
            if ordinal % 17 == 0:
                points[3] = points[2]
            shape = "e", points
            if ordinal % 23 == 0:
                shape = translated(shape, 10**15, -10**15)
            shapes.append(shape)
        specs = [("SKEW.MIN", [constraint("space", 0.05)], shapes),
                 ("SKEW.MAX", [constraint("enclosure", 0.02, ">")], shapes),
                 ("SKEW.LENGTH", [constraint("length", 0.03)],
                  [(kind, points[:2]) for kind, points in shapes])]
        folder = self.folder / "seeded"
        folder.mkdir()
        packed = write_pack(folder, specs)
        db = drc.IcePack(str(packed), review=False)
        self.addCleanup(db.close)
        for ci, (name, constraints, _shapes) in enumerate(specs):
            with self.subTest(rule=name):
                expected = delta.DeltaIndex(db, ci, constraints).measure()
                actual = cache.process_measure(delta.DeltaIndex(db, ci, constraints),
                                               backend="rust", jobs=4)
                self.assert_measurement_equal(actual, expected)

    def test_raw_fallback_rows_are_resolved_before_cache_publication(self):
        index = self.index("HALF")
        raw = self.folder / "raw"
        raw.mkdir()
        native.measure(index, str(raw), str(BIN), jobs=4)
        choices = np.fromfile(raw / "choices.bin", dtype="<i4")
        self.assertTrue(np.any(choices == -2), "half-tick cases must use the precision fallback")
        reference = self.index("HALF").measure()
        with patch.object(delta.DeltaIndex, "_errors",
                          side_effect=AssertionError("precision fallback ran in the parent")):
            actual = cache.process_measure(self.index("HALF"), backend="rust", jobs=4)
        self.assert_measurement_equal(actual, reference)

    def test_cache_reopen_reuses_native_result_without_starting_any_child(self):
        first = cache.process_measure(self.index("MULTIBLOCK"), backend="rust", jobs=4)
        paths = [array.filename for array in
                 (first.measured_ticks, first.constraint_indices, first.estimated_flags)]
        stamps = [os.stat(path).st_mtime_ns for path in paths]
        with patch.object(native, "measure", side_effect=AssertionError("native child restarted")), \
                patch.object(cache, "_run_child", side_effect=AssertionError("Python child restarted")):
            again = cache.process_measure(self.index("MULTIBLOCK"), backend="rust", jobs=1)
        self.assert_measurement_equal(again, first)
        self.assertEqual([os.stat(path).st_mtime_ns for path in paths], stamps)

    def test_empty_rule_shortcuts_native_while_raw_driver_writes_empty_arrays(self):
        with patch.object(native, "measure", side_effect=AssertionError("empty rule started native")):
            measured = cache.process_measure(self.index("EMPTY"), backend="rust", jobs=4)
        self.assertEqual(len(measured.measured_ticks), 0)
        raw = self.folder / "empty-raw"
        raw.mkdir()
        native.measure(self.index("EMPTY"), str(raw), str(BIN), jobs=4)
        self.assertEqual({path.name: path.stat().st_size for path in raw.iterdir()},
                         {"values.bin": 0, "choices.bin": 0, "estimated.bin": 0})

    def test_cancellation_stops_child_and_never_publishes_incomplete_measurements(self):
        stop = threading.Event()
        pids = []

        def started(pid):
            pids.append(pid)
            if pid is not None:
                stop.set()

        with self.assertRaises(delta.DeltaQueryCancelled):
            cache.process_measure(self.index("MULTIBLOCK"), backend="rust", jobs=4,
                                  cancelled=stop.is_set, process_callback=started)
        self.assertTrue(any(pid is not None for pid in pids))
        self.assertIsNone(pids[-1])
        self.assertFalse(cache.load_measurements(self.index("MULTIBLOCK")))
        directory = Path(cachepath.drc_analysis_dir(self.db.path))
        self.assertFalse(any(path.name.startswith(".measure-") for path in directory.rglob("*")))
        self.assertFalse(cache._CHILDREN)

    def test_corrupt_varint_is_rejected_without_publishing_cache(self):
        damaged = self.folder / "damaged.tray"
        shutil.copyfile(self.pack, damaged)
        offset = int(self.db._blk[0]["off"])
        with damaged.open("r+b") as stream:
            stream.seek(offset)
            stream.write(b"\xff" * 12)
        db = drc.IcePack(str(damaged), review=False)
        self.addCleanup(db.close)
        index = delta.DeltaIndex(db, 0, self.specs[0][1])
        with self.assertRaises(RuntimeError):
            cache.process_measure(index, backend="rust", jobs=4)
        self.assertFalse(cache.load_measurements(index))

    def test_driver_refuses_to_overwrite_existing_output(self):
        output = self.folder / "existing"
        output.mkdir()
        protected = output / "values.bin"
        protected.write_bytes(b"existing data")
        with self.assertRaises(RuntimeError):
            native.measure(self.index("MULTIBLOCK"), str(output), str(BIN), jobs=4)
        self.assertEqual(protected.read_bytes(), b"existing data")

    def test_auto_backend_can_use_python_when_native_is_unavailable(self):
        expected = self.index("LENGTH").measure()
        with patch.object(native, "find_binary", return_value=None), \
                patch.object(native, "measure", side_effect=AssertionError("unavailable native called")):
            actual = cache.process_measure(self.index("LENGTH"), backend="auto", jobs=1)
        self.assert_measurement_equal(actual, expected)

    def test_explicit_rust_reports_unavailable_helper_without_python_fallback(self):
        with patch.object(native, "find_binary", side_effect=RuntimeError("helper unavailable")), \
                patch.object(cache, "_run_child", side_effect=AssertionError("unexpected Python fallback")):
            with self.assertRaisesRegex(RuntimeError, "helper unavailable"):
                cache.process_measure(self.index("LENGTH"), backend="rust", jobs=1)

    def test_locator_rejects_old_protocol_and_missing_binary(self):
        helper = self.folder / "old-index"
        helper.write_text("#!/bin/sh\nprintf '0\\n'\n")
        helper.chmod(0o700)
        for path in (helper, self.folder / "missing-index"):
            with self.subTest(path=path.name), \
                    patch("floe.vfsclient.find_binary", return_value=str(path)):
                self.assertIsNone(FIND_BINARY(required=False))
                with self.assertRaises(RuntimeError):
                    FIND_BINARY(required=True)

    def test_invalid_backend_and_worker_counts_fail_before_any_spawn(self):
        forbidden = AssertionError("invalid settings started preprocessing")
        with patch.object(native, "measure", side_effect=forbidden), \
                patch.object(cache, "_run_child", side_effect=forbidden):
            for jobs in (0, -1, True, 1.5, "invalid", 257):
                with self.subTest(jobs=jobs), self.assertRaises(ValueError):
                    cache.process_measure(self.index("LENGTH"), backend="rust", jobs=jobs)
            with self.assertRaises(ValueError):
                cache.process_measure(self.index("LENGTH"), backend="invalid", jobs=1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
