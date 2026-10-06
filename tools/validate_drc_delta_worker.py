"""Deterministic latest-request CD-delta worker tests, without GTK."""

import copy
import gc
import os
import sys
import threading
import time
import unittest
import weakref
from unittest.mock import patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe import drc_delta_worker as worker_module  # noqa: E402


class GroupResult(tuple):
    """Like real DeltaGroups, a result keeps its numeric index alive."""

    def __new__(cls, index, values):
        result = tuple.__new__(cls, values)
        result.index = index
        return result


class Harness:
    """A controllable backend: events exercise real worker races reliably."""

    def __init__(self, *specs):
        self.specs = list(specs)
        self.records = []
        self.references = []
        self.condition = threading.Condition()

    def factory(self, db, ci, constraints):
        harness = self

        class Index:
            def __init__(self):
                self.db = db
                self.ci = ci
                self.constraints = copy.deepcopy(constraints)
                self.record = dict(
                    ci=ci, constraints=self.constraints, measure_calls=0,
                    group_calls=[], measure_started=threading.Event(),
                    measure_cancelled=threading.Event(),
                    group_started=threading.Event(),
                    group_cancelled=threading.Event())
                with harness.condition:
                    self.spec = (harness.specs[len(harness.records)]
                                 if len(harness.records) < len(harness.specs)
                                 else {})
                    harness.records.append(self.record)
                    harness.references.append(weakref.ref(self))
                    harness.condition.notify_all()

            def wait(self, gate, cancelled, cancellation_event):
                while gate is not None and not gate.wait(0.002):
                    if cancelled():
                        cancellation_event.set()
                        raise worker_module.DeltaQueryCancelled()
                if cancelled():
                    cancellation_event.set()
                    raise worker_module.DeltaQueryCancelled()

            def measure(self, cancelled):
                self.record["measure_calls"] += 1
                self.record["measure_started"].set()
                self.wait(self.spec.get("measure_gate"), cancelled,
                          self.record["measure_cancelled"])
                return self

            def group(self, step, cluster=None, mode="absolute", cancelled=None):
                self.record["group_calls"].append((step, mode, cluster))
                self.record["group_started"].set()
                gate = (self.spec.get("group_gate")
                        if step == self.spec.get("group_gate_step") else None)
                self.wait(gate, cancelled, self.record["group_cancelled"])
                if "error_step" in self.spec and step == self.spec["error_step"]:
                    raise ValueError("invalid delta step")
                return GroupResult(self, (self.ci, self.constraints,
                                          step, mode, cluster))

        return Index()

    def record(self, number=0):
        end = time.monotonic() + 3
        with self.condition:
            while len(self.records) <= number:
                left = end - time.monotonic()
                if left <= 0:
                    raise AssertionError("worker did not construct expected index")
                self.condition.wait(left)
            return self.records[number]


class DeltaWorkerTests(unittest.TestCase):
    def start_worker(self, harness):
        patcher = patch.object(worker_module, "DeltaIndex", harness.factory)
        patcher.start()
        self.addCleanup(patcher.stop)
        worker = worker_module.DeltaWorker()

        def stop():
            worker.close()
            worker._thread.join(3)
            self.assertFalse(worker._thread.is_alive(), "worker leaked a thread")

        self.addCleanup(stop)
        return worker

    def result(self, worker):
        end = time.monotonic() + 3
        while time.monotonic() < end:
            result = worker.poll()
            if result is not None:
                return result
            time.sleep(0.002)
        self.fail("worker did not publish a result")

    def wait_collected(self, reference):
        end = time.monotonic() + 3
        while time.monotonic() < end:
            gc.collect()
            if reference() is None:
                return
            time.sleep(0.002)
        self.fail("worker retained a previous rule's numeric index")

    def test_same_rule_changes_keep_measurement_and_coalesce_latest(self):
        gate = threading.Event()
        harness = Harness({"measure_gate": gate})
        worker = self.start_worker(harness)
        db, cluster = object(), object()
        constraints = [{"metric": "width", "value": 0.1}]
        worker.submit("first", db, 0, constraints, 1, "absolute", None)
        record = harness.record()
        self.assertTrue(record["measure_started"].wait(3))
        worker.submit("middle", db, 0, constraints, 2, "percent", None)
        worker.submit("latest", db, 0, constraints, 3, "percent", cluster)
        gate.set()
        key, groups, error = self.result(worker)
        self.assertEqual(key, "latest")
        self.assertIsNone(error)
        self.assertEqual(groups[2:], (3, "percent", cluster))
        self.assertEqual(len(harness.records), 1)
        self.assertEqual(record["measure_calls"], 1)
        self.assertFalse(record["measure_cancelled"].is_set())
        self.assertEqual(record["group_calls"], [(3, "percent", cluster)])
        self.assertIsNone(worker.poll(), "poll did not consume the result")

    def test_auto_step_none_is_forwarded_and_reuses_same_rule_measurements(self):
        harness = Harness()
        worker = self.start_worker(harness)
        db, cluster = object(), object()
        worker.submit("auto absolute", db, 0, [], None)
        key, groups, error = self.result(worker)
        self.assertEqual((key, groups[2:], error),
                         ("auto absolute", (None, "absolute", None), None))
        worker.submit("manual", db, 0, [], 17)
        self.assertEqual(self.result(worker)[1][2], 17)
        worker.submit("auto ratio cluster", db, 0, [], None, "percent", cluster)
        key, groups, error = self.result(worker)
        self.assertEqual((key, groups[2:], error),
                         ("auto ratio cluster", (None, "percent", cluster), None))
        self.assertEqual(len(harness.records), 1)
        self.assertEqual(harness.record()["measure_calls"], 1)

    def test_auto_request_cancels_obsolete_manual_grouping(self):
        gate = threading.Event()
        harness = Harness({"group_gate": gate, "group_gate_step": 1})
        worker = self.start_worker(harness)
        db = object()
        worker.submit("manual", db, 0, [], 1)
        record = harness.record()
        self.assertTrue(record["group_started"].wait(3))
        worker.submit("auto", db, 0, [], None)
        key, groups, error = self.result(worker)
        self.assertEqual((key, groups[2], error), ("auto", None, None))
        self.assertTrue(record["group_cancelled"].is_set())
        self.assertEqual(record["measure_calls"], 1)

    def test_new_grouping_cancels_old_group_but_reuses_measurements(self):
        gate = threading.Event()
        harness = Harness({"group_gate": gate, "group_gate_step": 1})
        worker = self.start_worker(harness)
        db = object()
        worker.submit("old", db, 0, [], 1)
        record = harness.record()
        self.assertTrue(record["group_started"].wait(3))
        worker.submit("new", db, 0, [], 2, "percent")
        key, groups, error = self.result(worker)
        self.assertEqual((key, groups[2], error), ("new", 2, None))
        self.assertTrue(record["group_cancelled"].is_set())
        self.assertEqual(record["measure_calls"], 1)
        self.assertEqual(len(harness.records), 1)

    def test_scope_change_cancels_measurement_and_releases_previous_index(self):
        gate = threading.Event()
        harness = Harness({"measure_gate": gate})
        worker = self.start_worker(harness)
        db = object()
        worker.submit("old rule", db, 0, [], 1)
        old = harness.record()
        self.assertTrue(old["measure_started"].wait(3))
        worker.submit("new rule", db, 1, [], 1)
        key, groups, error = self.result(worker)
        self.assertEqual((key, groups[0], error), ("new rule", 1, None))
        self.assertTrue(old["measure_cancelled"].is_set())
        self.assertEqual(len(harness.records), 2)
        self.wait_collected(harness.references[0])

    def test_metadata_snapshot_equality_and_database_identity(self):
        gate = threading.Event()
        harness = Harness({"measure_gate": gate})
        worker = self.start_worker(harness)
        db = object()
        metadata = [{"metric": "width", "value": 0.1, "layers": [7]}]
        original = copy.deepcopy(metadata)
        worker.submit("copied", db, 0, metadata, 1)
        record = harness.record()
        self.assertTrue(record["measure_started"].wait(3))
        metadata[0]["value"] = 99
        metadata[0]["layers"].append(8)
        gate.set()
        self.assertEqual(self.result(worker)[1][1], tuple(original))
        worker.submit("equivalent", db, 0, copy.deepcopy(original), 2)
        self.assertEqual(self.result(worker)[0], "equivalent")
        self.assertEqual(len(harness.records), 1)
        worker.submit("new metadata", db, 0, metadata, 2)
        self.assertEqual(self.result(worker)[0], "new metadata")
        self.assertEqual(len(harness.records), 2)
        self.wait_collected(harness.references[0])
        worker.submit("new db", object(), 0, metadata, 2)
        self.assertEqual(self.result(worker)[0], "new db")
        self.assertEqual(len(harness.records), 3)
        self.wait_collected(harness.references[1])

    def test_cancel_discards_pending_and_cached_data_then_allows_reuse(self):
        gate = threading.Event()
        harness = Harness({"measure_gate": gate})
        worker = self.start_worker(harness)
        db = object()
        worker.submit("discard", db, 0, [], 1)
        record = harness.record()
        self.assertTrue(record["measure_started"].wait(3))
        worker.cancel()
        self.assertTrue(record["measure_cancelled"].wait(3))
        self.assertIsNone(worker.poll())
        self.wait_collected(harness.references[0])
        worker.submit("retry", db, 0, [], 2)
        self.assertEqual(self.result(worker)[0], "retry")
        self.assertEqual(len(harness.records), 2)
        worker.cancel()
        self.wait_collected(harness.references[1])

    def test_group_error_is_reported_and_next_request_reuses_index(self):
        harness = Harness({"error_step": 0})
        worker = self.start_worker(harness)
        db = object()
        worker.submit("invalid", db, 0, [], 0)
        self.assertEqual(self.result(worker),
                         ("invalid", None, "invalid delta step"))
        worker.submit("fixed", db, 0, [], 1)
        key, groups, error = self.result(worker)
        self.assertEqual((key, groups[2], error), ("fixed", 1, None))
        self.assertEqual(len(harness.records), 1)

    def test_cancel_releases_database_and_completed_group_locals(self):
        class Db:
            pass

        harness = Harness()
        worker = self.start_worker(harness)
        db = Db()
        reference = weakref.ref(db)
        worker.submit("done", db, 0, [], 1)
        self.assertEqual(self.result(worker)[0], "done")
        del db
        self.assertIsNotNone(reference(), "active measured index was not retained")
        worker.cancel()
        self.wait_collected(reference)

    def test_close_cancels_active_work_and_ignores_new_submissions(self):
        gate = threading.Event()
        harness = Harness({"measure_gate": gate})
        worker = self.start_worker(harness)
        worker.submit("old", object(), 0, [], 1)
        record = harness.record()
        self.assertTrue(record["measure_started"].wait(3))
        worker.close()
        worker.submit("ignored", object(), 0, [], 2)
        worker._thread.join(3)
        self.assertFalse(worker._thread.is_alive())
        self.assertIsNone(worker.poll())
        self.assertEqual(len(harness.records), 1)
        self.wait_collected(harness.references[0])


if __name__ == "__main__":
    unittest.main(verbosity=2)
