"""Large-rule GUI work must stay bounded and hand scans to workers.

Uses the real rule-selection, page and marker-request handlers with
headless widget adapters. The virtual 1.4-billion-error database retains
no per-error arrays. Native GTK rendering is outside this gate's scope.

Usage: python tools/validate_drc_large_gui.py
"""

import contextlib
import math
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe import drc, drc_delta, drc_markers, gui  # noqa: E402
from validate_drc_clusters import TreeStore, TreeView  # noqa: E402
from validate_drc_delta_gui import GroupStore, fixture, use_auto  # noqa: E402


class VirtualErrors:
    """Rule selection/pages display IDs and must not decode geometry."""

    def __init__(self, count):
        self.count = count
        self.reads = []

    def __len__(self):
        return self.count

    def __getitem__(self, index):
        if not 0 <= index < self.count:
            raise IndexError(index)
        self.reads.append(index)
        raise AssertionError("rule click decoded geometry before an error was opened")


class VirtualDb:
    """Packed-style O(1) count/status contract without any large arrays."""

    def __init__(self):
        self.path = "virtual-large.tray"
        self.cell = "SYNTHETIC"
        self.precision = 100000
        self.checks = [
            SimpleNamespace(name=name, errors=VirtualErrors(count),
                            declared=count, desc="Synthetic large rule")
            for name, count in (("R", 10_000_003), ("S", 1_389_999_997))]
        self.total = sum(len(check.errors) for check in self.checks)

    def get_status(self, ci, ei):
        return 0

    def status_counts(self, ci):
        return 0, len(self.checks[ci].errors)

    def status_page(self, ci, waived, start, limit):
        if waived:
            return []
        return list(range(start, min(start + limit, len(self.checks[ci].errors))))


def large_viewer():
    viewer = use_auto(fixture())
    viewer._drc = VirtualDb()
    cons = [{"metric": "width", "op": "<", "value": 0.05,
             "text": "INTERNAL M1 < 0.05"}]
    viewer._drc_rmeta = {"checks": {name: {"constraints": cons}
                                    for name in ("R", "S")}}
    viewer._drc_rules_busy = False
    viewer._drc_open = None
    viewer._drc_grid_ci = None
    viewer._drc_cum = [0, len(viewer._drc.checks[0].errors)]
    viewer._drc_shown = 2
    viewer._drc_rmatch = (2, 2)
    viewer._drc_grid_fill = gui.Viewer._drc_grid_fill.__get__(viewer)
    viewer._drc_show_rule = gui.Viewer._drc_show_rule.__get__(viewer)
    viewer._drc_info_refresh = gui.Viewer._drc_info_refresh.__get__(viewer)
    win = viewer._drcwin
    win._detail = Mock()
    win._info = Mock()
    win._delta_store = GroupStore()
    win._delta_tree = Mock()
    win._delta_label = Mock()
    win._delta_prev, win._delta_next = Mock(), Mock()
    win._rstore = TreeStore()
    win._rules = TreeView(viewer, win._rstore)
    for ci, check in enumerate(viewer._drc.checks):
        win._rstore.append(None, [check.name, str(len(check.errors)), ci, None])
    # Execute the real display-side request without native GTK composition.
    viewer._display = Mock(side_effect=lambda: viewer._drc_marker_request(
        (0, 0, 1000, 1000), 10, 100, 100))
    return viewer


@contextlib.contextmanager
def forbid_foreground_scans():
    """Fail if a click regresses to whole-rule work on the calling thread."""
    foreground = threading.get_ident()

    def forbidden(name):
        def fail(*_args, **_kwargs):
            if threading.get_ident() == foreground:
                raise AssertionError("foreground whole-rule work: " + name)
            raise AssertionError("contract fixture unexpectedly started background work")
        return fail

    def guard_array(name, original):
        def guarded(shape, *args, **kwargs):
            if threading.get_ident() == foreground:
                size = math.prod(shape) if isinstance(shape, (tuple, list)) else int(shape)
                if size > 65536:
                    raise AssertionError("foreground large allocation: np." + name)
            return original(shape, *args, **kwargs)
        return guarded

    with contextlib.ExitStack() as stack:
        stack.enter_context(patch.object(drc_delta.DeltaIndex, "measure",
                                         forbidden("DeltaIndex.measure")))
        stack.enter_context(patch.object(drc_markers.MarkerIndex, "query",
                                         forbidden("MarkerIndex.query")))
        for name in ("empty", "zeros", "ones", "full"):
            stack.enter_context(patch.object(np, name, guard_array(name, getattr(np, name))))
        yield


class LargeRuleViewerTests(unittest.TestCase):
    def test_click_pages_only_1000_and_submits_whole_rule_to_workers(self):
        viewer = large_viewer()
        with forbid_foreground_scans():
            viewer._drcwin._rules.set_cursor((0,), None, False)
        self.assertEqual(viewer._drc.total, 1_400_000_000)
        self.assertEqual(viewer._drc_open, 0)
        self.assertEqual(viewer._drc.checks[0].errors.reads, [])
        self.assertEqual(viewer._drc.checks[1].errors.reads, [])
        self.assertEqual(len(viewer._drc_grid_map), gui.DRC_PAGE)
        self.assertTrue(viewer._drc_delta_busy)
        self.assertTrue(viewer._drc_marker_busy)
        self.assertIn("10000003", viewer._drcwin._detail.set_text.call_args.args[0])
        self.assertEqual(viewer._drc_delta_worker.submit.call_args.args[2], 0)
        query = viewer._drc_marker_worker.submit.call_args.args[2]
        self.assertEqual(query["checks"], (0,))
        self.assertNotIn("cap", query)

    def test_another_rule_remains_selectable_while_workers_are_pending(self):
        viewer = large_viewer()
        with forbid_foreground_scans():
            viewer._drcwin._rules.set_cursor((0,), None, False)
            first_key = viewer._drc_delta_key
            viewer._drcwin._rules.set_cursor((1,), None, False)
        self.assertEqual(viewer._drc_open, 1)
        self.assertNotEqual(viewer._drc_delta_key, first_key)
        self.assertEqual(viewer._drc_delta_worker.submit.call_count, 2)
        self.assertEqual(viewer._drc_marker_worker.submit.call_count, 2)
        for check in viewer._drc.checks:
            self.assertEqual(check.errors.reads, [])
        self.assertEqual(viewer._drcwin._plabel.set_text.call_args.args[0], "1 / 1390000")

    def test_review_filter_uses_bounded_status_page_without_status_materialization(self):
        viewer = large_viewer()
        viewer._drc_wfilter = "notwaived"
        viewer._drc.status_eis = Mock(side_effect=AssertionError("materialized rule statuses"))
        with forbid_foreground_scans():
            viewer._drcwin._rules.set_cursor((0,), None, False)
        viewer._drc.status_eis.assert_not_called()
        self.assertEqual(viewer._drc_grid_base, ("status", False))
        self.assertEqual(len(viewer._drc_grid_map), gui.DRC_PAGE)

    def test_obsolete_worker_result_does_not_replace_current_rule(self):
        viewer = large_viewer()
        viewer._drcwin._rules.set_cursor((0,), None, False)
        delta_key, marker_key = viewer._drc_delta_key, viewer._drc_marker_key
        viewer._drcwin._rules.set_cursor((1,), None, False)
        viewer._drc_delta_worker.poll.return_value = (delta_key, object(), None)
        viewer._drc_marker_worker.poll.return_value = (marker_key, [object()], None)
        with forbid_foreground_scans():
            viewer._drc_delta_poll()
            viewer._drc_marker_poll()
        self.assertEqual(viewer._drc_open, 1)
        self.assertIsNone(viewer._drc_delta_groups)
        self.assertIsNone(viewer._drc_marker_result)
        self.assertTrue(viewer._drc_delta_busy)
        self.assertTrue(viewer._drc_marker_busy)


class LargeRuleProcessTests(unittest.TestCase):
    """Real packed data, real children, and main-thread GUI callbacks.

    The small fixture lowers only the routing threshold. Parent-side CD
    measurement/spatial queries are forbidden, so these checks cannot pass
    by silently falling back to the previous CPU-bound Python thread path.
    """

    @classmethod
    def setUpClass(cls):
        binary = os.path.join(os.path.dirname(__file__), "..", "rust", "target",
                              "release", "floe-index")
        if not os.path.isfile(binary):
            raise unittest.SkipTest("build rust/target/release/floe-index first")
        cls.tmp = tempfile.TemporaryDirectory(prefix="floe-large-gui-")
        cls.source = os.path.join(cls.tmp.name, "results.db")
        cls.pack_path = os.path.join(cls.tmp.name, "results.tray")
        with open(cls.source, "w", encoding="utf-8") as stream:
            stream.write("SYNTHETIC 100000\n")
            for name, count in (("R", 20000), ("S", 17)):
                stream.write("%s\n%d %d 0\n" % (name, count, count))
                for ei in range(count):
                    x = ei % 200 * 100000
                    y = ei // 200 * 100000
                    width = 2000 + ei % 10 * 100
                    stream.write("p 1 4\n%d %d\n%d %d\n%d %d\n%d %d\n" %
                                 (x, y, x + width, y, x + width, y + 10000,
                                  x, y + 10000))
        subprocess.run([binary, "drc", cls.source, cls.pack_path],
                       check=True, capture_output=True, text=True)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def setUp(self):
        self.viewer = large_viewer()
        # Each test starts with a cold analysis cache. A warm cache can
        # legitimately finish before any analysis child is needed.
        case_dir = os.path.join(self.tmp.name, self._testMethodName)
        os.mkdir(case_dir)
        case_pack = os.path.join(case_dir, "results.tray")
        shutil.copyfile(self.pack_path, case_pack)
        self.db = drc.IcePack(case_pack, review=False)
        self.viewer._drc = self.db
        self.viewer._drc_cum = [0, 20000]
        self.viewer._display = Mock(side_effect=lambda:
            self.viewer._drc_marker_request((-1000, -1000, 201000, 201000),
                                            2020, 100, 100))
        self.workers = []

    def tearDown(self):
        for worker in self.workers:
            worker.close()
        for worker in self.workers:
            worker._thread.join(timeout=5)
            self.assertFalse(worker._thread.is_alive(), "worker did not stop")
        self.db.close()

    def _until_ready(self, kind, while_running=None):
        viewer = self.viewer
        worker = getattr(viewer, "_drc_%s_worker" % kind)
        poll = getattr(viewer, "_drc_%s_poll" % kind)
        deadline = time.monotonic() + 45
        pids = set()
        callback_times = []
        heartbeats = 0
        acted = False
        def pending():
            return (viewer._drc_hl_res is None if kind == "query" else
                    getattr(viewer, "_drc_%s_busy" % kind))

        while pending():
            started = time.monotonic()
            self.assertLess(started, deadline, "%s worker did not complete" % kind)
            pid = getattr(worker, "_process_pid", None)
            if pid is None:
                process = getattr(worker, "_process", None)
                pid = getattr(process, "pid", None)
            if pid is not None:
                self.assertNotEqual(pid, os.getpid())
                pids.add(pid)
                heartbeats += 1
                if while_running is not None and not acted:
                    while_running()
                    acted = True
            poll()
            callback_times.append(time.monotonic() - started)
            time.sleep(0.01)
        self.assertTrue(pids, "no analysis child was observed")
        self.assertGreater(heartbeats, 1, "no repeated main callbacks during child work")
        self.assertLess(max(callback_times), 2,
                        "main GUI callback stalled while analysis child was working")
        if while_running is not None:
            self.assertTrue(acted)
        print("%s process: %d main heartbeats, max callback %.4f s" %
              (kind, heartbeats, max(callback_times)))

    def test_cd_process_allows_page_change_and_returns_complete_groups(self):
        from floe import drc_delta_worker as workers
        worker = workers.DeltaWorker()
        self.workers.append(worker)
        self.viewer._drc_delta_worker = worker
        with patch.object(workers, "PROCESS_THRESHOLD", 1), \
                patch.object(drc_delta.DeltaIndex, "measure",
                             side_effect=AssertionError("CD measured in GUI process")):
            self.viewer._drcwin._rules.set_cursor((0,), None, False)
            self._until_ready("delta", lambda: self.viewer._drc_page_step(1))
        self.assertIsNone(self.viewer._drc_delta_error)
        self.assertEqual(self.viewer._drc_delta_groups.total, 20000)
        self.assertEqual(self.viewer._drc_page, 1)
        self.assertEqual(self.viewer._drc_grid_map, list(range(1000, 2000)))

    def test_cd_process_rule_switch_cancels_obsolete_result(self):
        from floe import drc_delta_worker as workers
        worker = workers.DeltaWorker()
        self.workers.append(worker)
        self.viewer._drc_delta_worker = worker
        with patch.object(workers, "PROCESS_THRESHOLD", 1), \
                patch.object(drc_delta.DeltaIndex, "measure",
                             side_effect=AssertionError("CD measured in GUI process")):
            self.viewer._drcwin._rules.set_cursor((0,), None, False)
            self._until_ready("delta", lambda:
                self.viewer._drcwin._rules.set_cursor((1,), None, False))
        self.assertEqual(self.viewer._drc_open, 1)
        self.assertIsNone(self.viewer._drc_delta_error)
        self.assertEqual(self.viewer._drc_delta_groups.total, 17)
        self.assertEqual(len(self.viewer._drc_grid_map), 17)

    def test_marker_process_keeps_main_callbacks_active_and_counts_entire_rule(self):
        from floe import drc_marker_worker as workers
        worker = workers.MarkerWorker()
        self.workers.append(worker)
        self.viewer._drc_marker_worker = worker
        threshold = "PROCESS_THRESHOLD" if hasattr(workers, "PROCESS_THRESHOLD") else "LARGE_RULE"
        with patch.object(workers, threshold, 1), \
                patch.object(drc_markers.MarkerIndex, "query",
                             side_effect=AssertionError("markers queried in GUI process")):
            self.viewer._drcwin._rules.set_cursor((0,), None, False)
            self._until_ready("marker", lambda: self.viewer._drc_page_step(1))
        result = self.viewer._drc_marker_result
        self.assertIsNotNone(result)
        self.assertEqual(sum(marker.count for marker in result[1]), 20000)
        self.assertLessEqual(len(result[1]), 8192)
        self.assertEqual(self.viewer._drc_page, 1)

    def test_mapped_delta_group_filters_marker_child_without_parent_scans(self):
        from floe import drc_delta_worker as deltas
        from floe import drc_marker_worker as markers
        delta_worker = deltas.DeltaWorker()
        self.workers.append(delta_worker)
        self.viewer._drc_delta_worker = delta_worker
        with patch.object(deltas, "PROCESS_THRESHOLD", 1), \
                patch.object(drc_delta.DeltaIndex, "measure",
                             side_effect=AssertionError("CD measured in GUI process")):
            self.viewer._drcwin._rules.set_cursor((0,), None, False)
            self._until_ready("delta")
        self.assertIsNone(self.viewer._drc_delta_error)
        groups = self.viewer._drc_delta_groups
        self.assertIsInstance(groups._row_for_error, np.memmap)
        group = groups[0]
        marker_worker = markers.MarkerWorker()
        self.workers.append(marker_worker)
        self.viewer._drc_marker_worker = marker_worker
        threshold = "PROCESS_THRESHOLD" if hasattr(markers, "PROCESS_THRESHOLD") else "LARGE_RULE"
        with patch.object(markers, threshold, 1), \
                patch.object(drc_markers.MarkerIndex, "query",
                             side_effect=AssertionError("markers queried in GUI process")):
            self.viewer._drc_delta_choose(group)
            self._until_ready("marker")
        result = self.viewer._drc_marker_result
        self.assertIsNotNone(result)
        self.assertEqual(sum(marker.count for marker in result[1]), group.total)
        self.assertLess(group.total, len(self.db.checks[0].errors))
        self.assertTrue(all(group.contains(marker.ei) for marker in result[1]))
        self.assertTrue(all(group.contains(ei) for ei in self.viewer._drc_grid_map))

    def test_in_view_query_is_deferred_and_fills_only_bounded_result_page(self):
        from floe import drc_marker_worker as markers
        from floe.drc_query_worker import ExactQueryWorker
        worker = ExactQueryWorker()
        self.workers.append(worker)
        self.viewer._drc_query_worker = worker
        self.viewer._drc_hl = True
        self.viewer.view_bbox.return_value = (-1000, -1000, 201000, 201000)
        with patch.object(markers, "LARGE_RULE", 1), \
                patch.object(drc.IcePack, "query_rect",
                             side_effect=AssertionError("in-view scan in GUI process")):
            self.viewer._drcwin._rules.set_cursor((0,), None, False)
            self.assertEqual(self.viewer._drc_grid_map, [],
                             "pending in-view query must not block rule selection")
            self._until_ready("query")
            first = self.viewer._drc_hl_res
            self.assertIsNone(self.viewer._drc_hl_key,
                              "a completed request must not remain marked pending")
            # Existing filter/note invalidations clear only the result.
            # The same viewport must be queried again, never stay empty.
            self.viewer._drc_hl_res = None
            self.viewer._drc_grid_fill(0)
            self.assertEqual(self.viewer._drc_grid_map, [])
            self._until_ready("query")
            self.assertEqual(self.viewer._drc_hl_res, first)
        result = self.viewer._drc_hl_res[1]
        self.assertEqual(len(result), min(20000, gui.DRC_HL_CAP))
        self.assertEqual(len(self.viewer._drc_grid_map), gui.DRC_PAGE)
        self.assertEqual(self.viewer._drc_grid_map, list(range(gui.DRC_PAGE)))
        self.assertEqual(self.viewer._drc_page_marks, [])

    def test_bulk_review_refresh_runs_in_child_and_restores_selected_group(self):
        from floe import drc_delta_worker as workers
        path = self.db.path
        self.db.close()
        # This test owns an isolated temporary review file; the real sample
        # or the user's review sidecar is never opened or modified.
        self.db = drc.IcePack(path)
        self.viewer._drc = self.db
        worker = workers.DeltaWorker()
        self.workers.append(worker)
        self.viewer._drc_delta_worker = worker
        with patch.object(workers, "PROCESS_THRESHOLD", 1), \
                patch.object(drc_delta.DeltaIndex, "measure",
                             side_effect=AssertionError("CD measured in GUI process")), \
                patch.object(drc_delta.DeltaGroups, "reset_status",
                             side_effect=AssertionError("bulk review scan in GUI process")):
            self.viewer._drcwin._rules.set_cursor((0,), None, False)
            self._until_ready("delta")
            self.assertIsNone(self.viewer._drc_delta_error)
            original_groups = self.viewer._drc_delta_groups
            original_group = original_groups[0]
            self.viewer._drc_delta_choose(original_group)
            ei = original_group.page(0, 1)[0]
            self.db.set_status(0, ei, drc.STATUS_WAIVED)
            self.viewer._drc_wfilter = "waived"
            self.viewer._drc_delta_status_changed(0, eis=None)
            self.assertTrue(self.viewer._drc_delta_busy)
            self.assertIsNone(self.viewer._drc_delta_groups)
            self._until_ready("delta")
        self.assertIsNone(self.viewer._drc_delta_error)
        self.assertIsNot(self.viewer._drc_delta_groups, original_groups)
        restored = self.viewer._drc_delta_group
        self.assertIsNotNone(restored)
        self.assertEqual(restored.key, original_group.key)
        self.assertEqual(restored.count(True), 1)
        self.assertEqual(self.viewer._drc_grid_map, [ei])


if __name__ == "__main__":
    unittest.main(verbosity=2)
