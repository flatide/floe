"""Bounded DRC cluster regression gate (no GTK or Rust binary needed).

Checks the compact, rule-local range format, implicit Unclustered errors,
page/rank/membership agreement, malformed input, and review-status cache
invalidation. The billion-error fixture has no geometry or per-error
allocation; randomized small fixtures compare every result with brute force.

Usage: .venv/bin/python tools/validate_drc_clusters.py
"""

import os
import random
import sys
import tempfile
import tracemalloc
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe.drc_clusters import load_clusters  # noqa: E402


class SizeOnlyErrors:
    """Cluster loading must never decode geometry or enumerate errors."""

    def __init__(self, size):
        self.size = size

    def __len__(self):
        return self.size

    def __getitem__(self, index):
        raise AssertionError("cluster lookup attempted to decode geometry")


class LazyGeometry(SizeOnlyErrors):
    def __init__(self, size):
        super().__init__(size)
        self.decoded = []

    def __getitem__(self, index):
        if not 0 <= index < self.size:
            raise IndexError(index)
        self.decoded.append(index)
        return SimpleNamespace(kind="p", pts=[(index * 2.0, 2.0)])


class AsciiDb:
    def __init__(self, rules):
        self.checks = [SimpleNamespace(name=name, errors=SizeOnlyErrors(size))
                       for name, size in rules]


class StatusDb(AsciiDb):
    """The packed database's status-array contract, without a real pack."""

    def __init__(self, rules, values=None):
        super().__init__(rules)
        counts = [size for _, size in rules]
        self._dir_es = np.asarray([0] + list(np.cumsum(counts)[:-1]),
                                  dtype=np.int64)
        self._status = np.zeros(sum(counts), dtype=np.uint8)
        if values is not None:
            self._status[:] = values

    def get_status(self, ci, ei):
        return int(self._status[int(self._dir_es[ci]) + ei])

    def set_status(self, ci, ei, value):
        self._status[int(self._dir_es[ci]) + ei] = value

    def status_counts(self, ci):
        start = int(self._dir_es[ci])
        size = len(self.checks[ci].errors)
        return int(np.count_nonzero(self._status[start:start + size] == 1)), size


class ScalarStatusDb(AsciiDb):
    """Exercise the adapter for databases without NumPy status storage."""

    def __init__(self, rules, values):
        super().__init__(rules)
        self.values = values

    def get_status(self, ci, ei):
        return self.values[ci][ei]


def ranges_text(indices):
    """Small reference encoder; input and output numbers are zero/one-based."""
    parts = []
    values = sorted(indices)
    if not values:
        return ""
    first = last = values[0]
    for value in values[1:]:
        if value == last + 1:
            last = value
            continue
        parts.append(str(first + 1) if first == last
                     else "%d-%d" % (first + 1, last + 1))
        first = last = value
    parts.append(str(first + 1) if first == last
                 else "%d-%d" % (first + 1, last + 1))
    return ",".join(parts)


class TreeRow(list):
    def __init__(self, values, path):
        super().__init__(values)
        self.path = path
        self.children = []

    def iterchildren(self):
        return iter(self.children)


class TreeStore(list):
    def append(self, parent, values):
        rows = self if parent is None else parent.children
        path = (len(rows),) if parent is None else parent.path + (len(rows),)
        row = TreeRow(values, path)
        list.append(rows, row)
        return row

    def get_value(self, row, column):
        return row[column]


class TreeView:
    """Minimal TreeStore selection behavior, including changed callbacks."""

    def __init__(self, viewer, store):
        self.viewer = viewer
        self.model = store
        self.selected = None
        self.expanded = set()

    def get_selection(self):
        return self

    def get_selected(self):
        return self.model, self.selected

    def unselect_all(self):
        self.selected = None

    def set_model(self, model):
        self.model = model
        self.expanded.clear()

    def row_expanded(self, path):
        return path in self.expanded

    def expand_row(self, path, _recursive):
        self.expanded.add(path)

    def set_cursor(self, path, _column, _editing):
        row = self.model[path[0]]
        for index in path[1:]:
            row = row.children[index]
        self.selected = row
        self.viewer._on_drc_rule_sel(self)

    def scroll_to_cell(self, *_args):
        pass


class ClusterTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="floe-clusters-")
        self.addCleanup(self.tmp.cleanup)
        self.path = os.path.join(self.tmp.name, "results.db.clusters")

    def load(self, content, db):
        with open(self.path, "w", encoding="utf-8", newline="") as stream:
            stream.write(content)
        return load_clusters(self.path, db)

    def assert_cluster(self, cluster, members, db, ci, exhaustive=True):
        expected = sorted(members)
        self.assertEqual(cluster.total, len(expected))
        n = len(db.checks[ci].errors)
        get_status = getattr(db, "get_status", lambda ci, ei: 0)
        self.assertEqual(cluster.status_counts(),
                         (sum(get_status(ci, ei) == 1 for ei in expected),
                          len(expected)))
        candidates = range(-1, n + 1) if exhaustive else (
            -1, 0, n - 1, n, *expected[:3], *expected[-3:])
        for ei in candidates:
            self.assertEqual(cluster.contains(ei), ei in members,
                             (cluster.name, "contains", ei))
        for waived in (None, False, True):
            filtered = [ei for ei in expected if waived is None
                        or (get_status(ci, ei) == 1) == waived]
            self.assertEqual(cluster.count(waived=waived), len(filtered))
            self.assertEqual(list(cluster.page(0, n + 10, waived=waived)),
                             filtered)
            positions = dict((ei, rank) for rank, ei in enumerate(filtered))
            for ei in candidates:
                self.assertEqual(cluster.rank(ei, waived=waived),
                                 positions.get(ei),
                                 (cluster.name, "rank", ei, waived))
            starts = (0, 1, 7, max(0, len(filtered) - 2), len(filtered),
                      len(filtered) + 5)
            for start in starts:
                for limit in (0, 1, 7, 19):
                    self.assertEqual(
                        list(cluster.page(start, limit, waived=waived)),
                        filtered[start:start + limit],
                        (cluster.name, "page", start, limit, waived))

    def test_rule_local_sparse_and_repeated_labels(self):
        db = AsciiDb([("earlier rule", 23), ("금속.간격 룰", 20),
                      ("last rule", 5)])
        clusters = self.load(
            "# IDs are local to the exact named rule.\r\n"
            "\r\n[금속.간격 룰]\r\n"
            "비연속 = 20,2,7-8\r\n"
            "연속 = 10-13\r\n"
            "비연속 = 4\r\n", db)
        self.assertEqual(os.fspath(clusters.path), self.path)
        self.assertEqual(set(clusters.rules), {1})
        groups = clusters.rules[1]
        self.assertEqual([group.name for group in groups],
                         ["비연속", "연속", "Unclustered"])
        expected = ({1, 3, 6, 7, 19}, set(range(9, 13)))
        expected += (set(range(20)) - set.union(*expected),)
        for group, members in zip(groups, expected):
            self.assert_cluster(group, members, db, 1)

    def test_fully_assigned_rule_has_no_empty_remainder(self):
        db = AsciiDb([("R", 5)])
        clusters = self.load("[R]\nB = 4-5\nA = 1-3\n", db)
        self.assertEqual([group.name for group in clusters.rules[0]],
                         ["B", "A"])
        self.assert_cluster(clusters.rules[0][0], {3, 4}, db, 0)
        self.assert_cluster(clusters.rules[0][1], {0, 1, 2}, db, 0)

    def test_billion_error_ranges_stay_compact(self):
        n = 1_000_000_000
        db = AsciiDb([("BIG", n)])
        content = "[BIG]\nBulk = 1-500000000,999999999-1000000000\n"
        tracemalloc.start()
        try:
            clusters = self.load(content, db)
            bulk, remainder = clusters.rules[0]
            self.assertEqual(bulk.total, 500_000_002)
            self.assertEqual(remainder.total, 499_999_998)
            self.assertEqual(list(bulk.page(499_999_998, 6)),
                             [499_999_998, 499_999_999, 999_999_998,
                              999_999_999])
            self.assertEqual(list(remainder.page(remainder.total - 2, 6)),
                             [999_999_996, 999_999_997])
            self.assertEqual(bulk.rank(999_999_999), 500_000_001)
            self.assertEqual(remainder.rank(999_999_997), 499_999_997)
            self.assertFalse(bulk.contains(500_000_000))
            self.assertTrue(remainder.contains(500_000_000))
            self.assertIsNone(bulk.rank(500_000_000))
            _, peak = tracemalloc.get_traced_memory()
        finally:
            tracemalloc.stop()
        self.assertLess(peak, 8 * 1024 * 1024,
                        "range loading or paging expanded per-error state")

    def test_invalid_files_are_rejected(self):
        db = AsciiDb([("R", 10), ("DUP", 4), ("DUP", 6)])
        invalid = {
            "no rule section": "C = 1\n",
            "unknown rule": "[UNKNOWN]\nC = 1\n",
            "ambiguous rule": "[DUP]\nC = 1\n",
            "zero ID": "[R]\nC = 0\n",
            "negative ID": "[R]\nC = -1\n",
            "past rule end": "[R]\nC = 11\n",
            "range past end": "[R]\nC = 8-11\n",
            "reversed range": "[R]\nC = 5-3\n",
            "same cluster overlap": "[R]\nC = 1-4,4-5\n",
            "cross cluster overlap": "[R]\nA = 1-4\nB = 4-8\n",
            "repeated label overlap": "[R]\nA = 2\nA = 2\n",
            "noninteger ID": "[R]\nC = 1.5\n",
            "malformed range": "[R]\nC = 1-2-3\n",
            "missing assignment": "[R]\nC 1-2\n",
            "empty label": "[R]\n = 1\n",
            "empty membership": "[R]\nC = \n",
            "reserved label": "[R]\nUnclustered = 1\n",
        }
        for description, content in invalid.items():
            with self.subTest(description=description):
                with self.assertRaises(ValueError):
                    self.load(content, db)

    def test_scalar_status_fallback(self):
        db = ScalarStatusDb([("R", 8)], [[0, 1, 2, 255, 1, 0, 1, 0]])
        clusters = self.load("[R]\nA = 1-3,6,8\n", db)
        self.assert_cluster(clusters.rules[0][0], {0, 1, 2, 5, 7}, db, 0)
        self.assert_cluster(clusters.rules[0][1], {3, 4, 6}, db, 0)
        db.values[0][1] = 0
        db.values[0][5] = 1
        clusters.status_changed(0, [1, 5])
        self.assert_cluster(clusters.rules[0][0], {0, 1, 2, 5, 7}, db, 0)

    def test_randomized_status_page_rank_and_invalidation(self):
        rng = random.Random(20261005)
        for trial in range(12):
            with self.subTest(trial=trial):
                rules = [("PREFIX", 17), ("R", rng.randrange(35, 120)),
                         ("S", rng.randrange(35, 120))]
                statuses = [rng.choice((0, 1, 1, 2, 255))
                            for _ in range(sum(size for _, size in rules))]
                db = StatusDb(rules, statuses)
                lines, expected = [], {}
                for ci in (1, 2):
                    lines.append("[%s]" % rules[ci][0])
                    assigned = [set() for _ in range(4)]
                    for ei in range(rules[ci][1]):
                        assigned[rng.randrange(4)].add(ei)
                    expected[ci] = assigned
                    for index in range(3):
                        lines.append("C%d = %s" %
                                     (index, ranges_text(assigned[index])))
                clusters = self.load("\n".join(lines), db)

                def check_all():
                    for ci in (1, 2):
                        self.assertEqual([c.name for c in clusters.rules[ci]],
                                         ["C0", "C1", "C2", "Unclustered"])
                        for group, members in zip(clusters.rules[ci],
                                                  expected[ci]):
                            self.assert_cluster(group, members, db, ci)

                check_all()
                for ci in (1, 2):
                    changes = [rng.randrange(rules[ci][1]) for _ in range(19)]
                    for ei in changes:
                        db.set_status(ci, ei, rng.choice((0, 1, 2, 255)))
                    clusters.status_changed(ci, changes)
                check_all()
                db._status[:] = [rng.choice((0, 1, 2))
                                 for _ in range(len(db._status))]
                clusters.reset_status()
                check_all()

    def test_status_pages_across_large_range_boundaries(self):
        n = 3 * 65536 + 73
        db = StatusDb([("PREFIX", 13), ("R", n)])
        local = db._status[13:]
        local[::3] = 1
        local[1::7] = 2
        members = set(range(2, 65537)) | set(range(65540, n - 2))
        clusters = self.load("[R]\nA = 3-65537,65541-%d\n" % (n - 2), db)
        group, remainder = clusters.rules[1]
        expected = sorted(members)

        def check_pages():
            self.assertEqual(group.status_counts(),
                             (sum(local[ei] == 1 for ei in expected),
                              len(expected)))
            for waived in (None, False, True):
                filtered = [ei for ei in expected if waived is None
                            or (local[ei] == 1) == waived]
                self.assertEqual(group.count(waived=waived), len(filtered))
                for start in (0, 21842, 65533, len(filtered) - 5):
                    self.assertEqual(list(group.page(start, 17, waived=waived)),
                                     filtered[start:start + 17])
                for rank in (0, len(filtered) // 2, len(filtered) - 1):
                    self.assertEqual(group.rank(filtered[rank], waived=waived),
                                     rank)
            self.assert_cluster(remainder, set(range(n)) - members, db, 1,
                                exhaustive=False)

        check_pages()
        changes = [2, 65535, 65536, 65540, 131072, n - 3]
        for ei in changes:
            db.set_status(1, ei, 0 if db.get_status(1, ei) == 1 else 1)
        clusters.status_changed(1, changes)
        check_pages()

    def test_status_invalidation_across_million_error_chunks(self):
        chunk = 1 << 20
        n = 2 * chunk + 19
        db = StatusDb([("R", n)])
        db._status[::3] = 1
        clusters = self.load("[R]\nA = 2-%d\n" % (n - 1), db)
        group = clusters.rules[0][0]

        def verify():
            for waived in (False, True):
                ids = np.flatnonzero((db._status[1:-1] == 1) == waived) + 1
                self.assertEqual(group.count(waived), len(ids))
                for start in (0, len(ids) // 2, len(ids) - 5):
                    self.assertEqual(group.page(start, 11, waived),
                                     ids[start:start + 11].tolist())
                for rank in (0, len(ids) // 2, len(ids) - 1):
                    self.assertEqual(group.rank(int(ids[rank]), waived), rank)

        verify()
        changes = [1, chunk - 1, chunk, chunk + 1, 2 * chunk - 1,
                   2 * chunk, n - 2]
        for ei in changes:
            db.set_status(0, ei, 0 if db.get_status(0, ei) == 1 else 1)
        clusters.status_changed(0, iter(changes))
        verify()

    def test_membership_masks_match_sparse_and_adjacent_ranges(self):
        db = AsciiDb([("R", 350)])
        members = set(range(0, 250, 3)) | set(range(260, 278))
        # Adjacent input ranges must be normalized before mask construction.
        text = ranges_text(members - set(range(260, 278)))
        clusters = self.load("[R]\nA = %s,261-266,267-278\n" % text, db)
        for group, selected in zip(clusters.rules[0],
                                   (members, set(range(350)) - members)):
            for start, size in ((0, 350), (1, 121), (77, 215),
                                (260, 18), (349, 1), (0, 0)):
                self.assertEqual(group.mask(start, size).tolist(),
                                 [ei in selected
                                  for ei in range(start, start + size)])

    def test_filtered_page_skips_large_zero_count_gaps(self):
        from floe.drc_clusters import Cluster

        count = 100_000
        db = StatusDb([("R", 2 * count)])
        db.set_status(0, 0, 1)
        db.set_status(0, 2 * count - 2, 1)
        starts = np.arange(0, 2 * count, 2, dtype=np.int64)
        group = Cluster("scattered", db, 0, starts, starts + 1)
        self.assertEqual(group.count(True), 2)

        class CountReads:
            def __init__(self, array):
                self.array, self.reads = array, 0

            def __len__(self):
                return len(self.array)

            def __getitem__(self, index):
                self.reads += 1
                return self.array[index]

        prefix = CountReads(group._wprefix)
        group._wprefix = prefix
        self.assertEqual(group.page(0, 10, True), [0, 2 * count - 2])
        self.assertLess(prefix.reads, 200,
                        "paging scanned every nonmatching interval")

    def test_uniform_status_uses_rule_count_without_reading_members(self):
        n = 1_000_000_000
        db = AsciiDb([("R", n)])
        db.get_status = Mock(side_effect=AssertionError("read uniform status"))
        db.status_counts = Mock(return_value=(0, n))
        clusters = self.load("[R]\nA = 2-999999999\n", db)
        group = clusters.rules[0][0]
        for waived in (False, True):
            self.assertEqual(group.count(waived), 0 if waived else n - 2)
            self.assertEqual(group.page(10, 3, waived),
                             [] if waived else [11, 12, 13])
            self.assertEqual(group.rank(123, waived), None if waived else 122)
        self.assertIsNone(group._chunks)
        db.status_counts.return_value = (n, n)
        clusters.status_changed(0, [1])
        self.assertEqual(group.status_counts(), (n - 2, n - 2))
        self.assertEqual(group.page(10, 3, True), [11, 12, 13])
        self.assertEqual(group.page(0, 3, False), [])
        self.assertIsNone(group._chunks)
        db.status_counts.return_value = (0, n)
        clusters.reset_status()
        self.assertEqual(group.status_counts(), (0, n - 2))
        db.get_status.assert_not_called()

    def test_interleaved_cluster_census_shares_status_reads(self):
        from floe.drc_clusters import Cluster, ClusterFile

        class CountingStatus(np.ndarray):
            def __array_finalize__(self, source):
                self.reads = getattr(source, "reads", None)

            def __getitem__(self, index):
                result = super().__getitem__(index)
                if isinstance(index, slice) and self.reads is not None:
                    self.reads.append(len(range(*index.indices(len(self)))))
                return result

        n = 120_003
        db = StatusDb([("R", n)])
        raw = db._status
        raw[::3] = 1
        db._status = raw.view(CountingStatus)
        db._status.reads = []
        db.status_counts = Mock(return_value=(int(np.count_nonzero(raw == 1)), n))
        groups = []
        for index in range(100):
            starts = np.arange(index, n, 100, dtype=np.int64)
            groups.append(Cluster("C%d" % index, db, 0, starts, starts + 1))
        clusters = ClusterFile(self.path, {0: groups})

        def check_counts():
            for index, group in enumerate(groups):
                expected = raw[index::100]
                self.assertEqual(group.status_counts(),
                                 (int(np.count_nonzero(expected == 1)),
                                  len(expected)))

        clusters.prepare_counts(0)
        check_counts()
        self.assertLessEqual(sum(db._status.reads), n,
                             "status census rescanned the rule per cluster")
        db._status.reads.clear()
        changes = [0, 55555, n - 1]
        for ei in changes:
            raw[ei] = 0 if raw[ei] == 1 else 1
        db.status_counts.return_value = (int(np.count_nonzero(raw == 1)), n)
        clusters.status_changed(0, changes)
        check_counts()
        self.assertLessEqual(sum(db._status.reads), n,
                             "status invalidation rescanned overlapping blocks")
        db._status.reads.clear()
        clusters.reset_status()
        clusters.prepare_counts(0)
        check_counts()
        self.assertLessEqual(sum(db._status.reads), n)

    def test_viewer_grid_fill_pages_and_filter_intersections(self):
        from floe import gui

        db = AsciiDb([("R", 1_000_000_000)])
        errors = LazyGeometry(len(db.checks[0].errors))
        db.checks[0].errors = errors
        group = self.load("[R]\nA = 101-1000000000\n", db).rules[0][0]
        viewer = gui.Viewer.__new__(gui.Viewer)
        viewer._drc = db
        viewer._drc_cluster = group
        viewer._drc_open = 0
        viewer._drc_sel = None
        viewer._drc_show_sel = viewer._drc_hl = False
        viewer._drc_wfilter = "all"
        viewer._drc_focus = None
        viewer._drc_page = 0
        viewer._drc_gridw = 1
        viewer.dbu = 2.0
        grid = Mock()
        grid.create_pango_layout.return_value.get_pixel_size.return_value = (20, 10)
        grid.get_allocation.return_value.width = 30
        viewer._drcwin = SimpleNamespace(_gstore=[], _grid=grid,
                                        _plabel=Mock(), _pprev=Mock(),
                                        _pnext=Mock())
        for page in (0, 1):
            errors.decoded.clear()
            viewer._drc_page = page
            viewer._drc_grid_fill(0)
            expected = list(range(100 + page * gui.DRC_PAGE,
                                  100 + (page + 1) * gui.DRC_PAGE))
            self.assertEqual(viewer._drc_grid_map, expected)
            self.assertEqual(errors.decoded, [],
                             "number-grid pages must not decode error geometry")
            self.assertEqual(viewer._drc_page_marks, [])
            self.assertEqual(viewer._drc_grid_base, ("cluster", group, None))
            self.assertEqual(len(viewer._drcwin._gstore), gui.DRC_PAGE)

        small = StatusDb([("R", 12)])
        small.query_rect = Mock()
        small.set_status(0, 3, 1)
        small.set_status(0, 7, 1)
        viewer._drc = small
        viewer._drc_cluster = self.load("[R]\nA = 2,4,6,8,10\n", small).rules[0][0]
        chosen = [1, 3, 5, 7, 9, 10]
        viewer._drc_sel = (0, chosen, [], frozenset(chosen))
        viewer._drc_show_sel = viewer._drc_hl = True
        marks = [(0, ei, "p", [(ei, 4.0)]) for ei in (3, 5, 9, 10)]
        viewer._drc_hl_list = Mock(return_value=marks)
        for filter_name, expected in (("all", [3, 5, 9]), ("waived", [3]),
                                      ("not-waived", [5, 9])):
            viewer._drc_wfilter = filter_name
            viewer._drc_grid_fill(0)
            self.assertEqual(viewer._drc_page, 0)
            self.assertEqual(viewer._drc_grid_map, expected)
            self.assertEqual(viewer._drc_page_marks, [])
        viewer._drc_sel = None
        viewer._drc_grid_fill(0)
        self.assertEqual(viewer._drc_grid_map, [])
        self.assertEqual(viewer._drc_page_marks, [])
        # An idle refill queued by the old rule must not replace the
        # current rule's grid after a selection change.
        viewer._drc_open = 1
        viewer._drc_grid_map = [999]
        viewer._drc_page_marks = [(1, 999, "p", [])]
        viewer._drc_grid_fill(0)
        self.assertEqual(viewer._drc_grid_map, [999])
        self.assertEqual(viewer._drc_page_marks, [(1, 999, "p", [])])

    def test_viewer_cluster_navigation_and_page_jump(self):
        from floe import gui

        db = StatusDb([("R", 6000)])
        db._status[::2] = 1
        clusters = self.load("[R]\nA = 4-100,201-5900\n", db)
        group = clusters.rules[0][0]
        viewer = gui.Viewer.__new__(gui.Viewer)
        viewer._drc = db
        viewer._drc_grid_ci = 0
        viewer._drc_page = 0
        viewer._drc_gridw = 7
        viewer._drcwin = SimpleNamespace(_grid=Mock())
        viewer._drc_grid_fill = Mock()
        viewer._drc_cell_mark = Mock()
        fake_gtk = SimpleNamespace(TreePath=SimpleNamespace(
            new_from_string=lambda value: value))
        for waived in (None, False, True):
            base = ("cluster", group, waived)
            members = group.page(0, group.total, waived)
            for current in (None, 0, members[0], members[17], members[-1]):
                for delta in (-1, 1):
                    rank = members.index(current) if current in members else None
                    expected = (members[(rank + delta) % len(members)]
                                if rank is not None else
                                members[0] if delta > 0 else members[-1])
                    self.assertEqual(viewer._drc_step_ei(db, 0, base, current,
                                                        delta), expected)
            viewer._drc_grid_base = base
            viewer._drc_page = 0
            viewer._drc_grid_fill.reset_mock()
            viewer._drc_cell_mark.reset_mock()
            target_rank = gui.DRC_PAGE + 23
            with patch.object(gui, "Gtk", fake_gtk):
                viewer._drc_goto_cell(0, members[target_rank])
            self.assertEqual(viewer._drc_page, 1)
            viewer._drc_grid_fill.assert_called_once_with(0)
            viewer._drc_cell_mark.assert_called_once_with(*divmod(23, 7))
            viewer._drc_cell_mark.reset_mock()
            with patch.object(gui, "Gtk", fake_gtk):
                viewer._drc_goto_cell(0, 0)
            viewer._drc_cell_mark.assert_not_called()

    def test_viewer_spatial_filter_passes_members_and_changes_cache(self):
        from floe import gui

        db = StatusDb([("R", 8)])
        clusters = self.load("[R]\nA = 1,4\nB = 2,8\n", db)
        first, second = clusters.rules[0][:2]
        error = SimpleNamespace(kind="p", pts=[(2.0, 6.0)])
        db.query_rect = Mock(return_value=[(0, 3, error)])
        viewer = gui.Viewer.__new__(gui.Viewer)
        viewer._drc = db
        viewer._drc_open = 0
        viewer._drc_cluster = first
        viewer._drc_hl_res = None
        viewer._drc_hl = False
        viewer._drc_wfilter = "waived"
        viewer.dbu = 2.0
        viewer.view_bbox = lambda: (1.0, 2.0, 3.0, 4.0)
        self.assertEqual(viewer._drc_hl_list(), [(0, 3, "p", [(1.0, 3.0)])])
        db.query_rect.assert_called_once_with(
            2.0, 4.0, 6.0, 8.0, cap=gui.DRC_HL_CAP, checks=(0,),
            members={0: first}, waived=True)
        viewer._drc_hl_list()
        self.assertEqual(db.query_rect.call_count, 1)
        viewer._drc_cluster = second
        viewer._drc_hl_list()
        self.assertEqual(db.query_rect.call_count, 2)
        self.assertEqual(db.query_rect.call_args.kwargs["members"], {0: second})
        viewer._drc_cluster = None
        viewer._drc_wfilter = "all"
        viewer._drc_hl_list()
        self.assertEqual(db.query_rect.call_count, 3)
        self.assertNotIn("members", db.query_rect.call_args.kwargs)
        self.assertNotIn("waived", db.query_rect.call_args.kwargs)

    def test_viewer_auto_load_for_ascii_current_and_legacy_pack_names(self):
        from floe import drc, gui

        db = AsciiDb([("R", 5)])
        self.load("[R]\nA = 1-3\n", db)
        base = self.path[:-len(".clusters")]
        pack = drc.IcePack.__new__(drc.IcePack)
        pack.checks = db.checks
        viewer = gui.Viewer.__new__(gui.Viewer)
        for path, database in (
                (base, db), (base, pack),
                (os.path.join(self.tmp.name, ".results.db.tray"), pack),
                (base + ".ice", pack)):
            with self.subTest(path=path):
                viewer._drc = database
                viewer._drc_clusters = None
                self.assertIsNone(viewer._drc_clusters_auto(path))
                self.assertEqual(viewer._drc_clusters.path, self.path)
                self.assertEqual(viewer._drc_clusters.rules[0][0].total, 3)
        viewer._drc = db
        viewer._drc_clusters = None
        self.assertIsNone(viewer._drc_clusters_auto(base + "-missing"))
        self.assertIsNone(viewer._drc_clusters)

    def test_viewer_tree_rows_selection_and_rebuild_preservation(self):
        from floe import gui

        db = StatusDb([("before", 3), ("R", 8), ("after", 2)])
        db.set_status(1, 0, 1)
        db.set_status(1, 6, 1)
        clusters = self.load("[R]\nA = 1-3\nB = 6-8\n", db)
        first, second, remainder = clusters.rules[1]
        viewer = gui.Viewer.__new__(gui.Viewer)
        viewer._drc = db
        viewer._drc_clusters = clusters
        viewer._drc_rmeta = None
        viewer._drc_delta_steps = {}
        viewer._drc_delta_auto_steps = {}
        viewer._drc_delta_step_scope = None
        viewer._drc_cluster = None
        viewer._drc_open = None
        viewer._drc_search = ""
        viewer._drc_tfilter = viewer._drc_wfilter = "all"
        viewer._drc_sel = None
        viewer._drc_sels = {}
        viewer._drc_cum = [0, 3, 11]
        viewer.drc_mark = None
        viewer._drc_ruler = []
        viewer.rulers = []
        viewer._drc_info_refresh = Mock()
        viewer._drc_grid_fill = Mock()
        viewer._drc_show_rule = Mock()
        viewer._display = Mock()
        store = TreeStore()
        tree = TreeView(viewer, store)
        viewer._drcwin = SimpleNamespace(_rstore=store, _rules=tree,
                                        _gstore=Mock(), _detail=Mock(),
                                        _plabel=Mock(), _pprev=Mock(),
                                        _pnext=Mock())
        viewer._drc_fill()
        self.assertEqual([row[0] for row in store], ["before", "R", "after"])
        self.assertEqual([row[0] for row in store[1].children],
                         ["A", "B", "Unclustered"])
        self.assertEqual([row[1] for row in store[1].children],
                         ["3/1", "3/1", "2/0"])

        def selection(eis):
            return (1, eis, [(ei, "p", []) for ei in eis], frozenset(eis))

        self.assertTrue(viewer._drc_select_row(1, first))
        # n/p establishes a current error without a live jump marker.
        viewer._drc_pos = 3
        viewer._drc_focus = (1, 0, "p", [])
        self.assertEqual(viewer._drc_current_target(), (1, [0]))
        self.assertTrue(viewer._drc_select_row(1, second))
        self.assertEqual(viewer._drc_pos, -1)
        self.assertIsNone(viewer._drc_current_target())
        self.assertTrue(viewer._drc_select_row(1, first))
        viewer._drc_sel = selection([0, 2])
        self.assertTrue(viewer._drc_select_row(1, second))
        self.assertEqual(viewer._drc_sels[(1, first)], selection([0, 2]))
        viewer._drc_sel = selection([5, 6])
        self.assertTrue(viewer._drc_select_row(1))
        viewer._drc_sel = selection([4])
        self.assertTrue(viewer._drc_select_row(1, first))
        self.assertEqual(viewer._drc_sel, selection([0, 2]))
        self.assertEqual(viewer._drc_sels[(1, second)], selection([5, 6]))
        self.assertEqual(viewer._drc_sels[(1, None)], selection([4]))
        viewer._drc_fill(preserve=True)
        self.assertIs(viewer._drc_cluster, first)
        self.assertEqual(viewer._drc_open, 1)
        self.assertTrue(tree.row_expanded((1,)))
        self.assertEqual(viewer._drc_sel, selection([0, 2]))
        viewer._drc_wfilter = "waived"
        viewer._drc_fill(preserve=True)
        self.assertEqual([row[0] for row in store], ["R"])
        self.assertEqual([row[3] for row in store[0].children], [first, second])
        self.assertIs(viewer._drc_cluster, first)
        self.assertEqual(list(viewer._drc_sel[1]), [0])
        self.assertFalse(viewer._drc_select_row(1, remainder))
        # Parent/child selections overlap: a child review must invalidate
        # the stale parent picks when the parent becomes active again.
        viewer._drc_wfilter = "not-waived"
        viewer._drc_fill(preserve=True)
        self.assertTrue(viewer._drc_select_row(1))
        viewer._drc_sel = selection([1, 2])
        self.assertTrue(viewer._drc_select_row(1, first))
        db.set_status(1, 1, 1)
        clusters.status_changed(1, [1])
        self.assertTrue(viewer._drc_select_row(1))
        self.assertEqual(list(viewer._drc_sel[1]), [2])
        self.assertEqual(viewer._drc_current_target()[0], 1)
        self.assertEqual(list(viewer._drc_current_target()[1]), [2])
        viewer._drc_search = "no matching rule"
        viewer._drc_fill(preserve=True)
        self.assertEqual(list(store), [])
        self.assertIsNone(viewer._drc_sel)
        self.assertIsNone(viewer._drc_current_target())
        self.assertEqual(viewer._drc_pos, -1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
