"""Capped exact in-view queries outside the GTK process for large rules."""

import atexit
import json
import os
import subprocess
import sys
import tempfile
import threading


_children = set()


def _stop_children():
    for process in tuple(_children):
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


atexit.register(_stop_children)


class ExactQueryWorker:
    """Latest request only; submit/poll/cancel never decode geometry or wait."""

    def __init__(self):
        self._condition = threading.Condition()
        self._generation = 0
        self._pending = self._result = None
        self._closed = False
        self._process_pid = None
        self._thread = threading.Thread(target=self._run, name="drc-in-view", daemon=True)
        self._thread.start()

    def submit(self, key, db, ci, bounds, cap, membership=None, waived=None):
        with self._condition:
            if self._closed:
                return
            self._generation += 1
            self._pending = (self._generation, key, db, ci, tuple(bounds),
                             int(cap), membership, waived)
            self._result = None
            self._condition.notify()

    def poll(self):
        with self._condition:
            result, self._result = self._result, None
            return result

    def cancel(self):
        with self._condition:
            self._generation += 1
            self._pending = self._result = None
            self._condition.notify()

    def close(self):
        with self._condition:
            self._closed = True
            self._generation += 1
            self._pending = self._result = None
            self._condition.notify()

    def _run(self):
        from .drc_marker_worker import export_members
        while True:
            db = membership = key = result = None
            with self._condition:
                while self._pending is None and not self._closed:
                    self._condition.wait()
                if self._closed:
                    return
                (generation, key, db, ci, bounds, cap, membership,
                 waived) = self._pending
                self._pending = None
            process = None
            error = result = None
            try:
                with tempfile.TemporaryDirectory(prefix="floe-drc-query-") as folder:
                    member = export_members(membership, folder, len(db.checks[ci].errors))
                    request = dict(pack=db.path, identity=db._analysis_identity,
                                   review_path=db._waive_path, ci=ci,
                                   bounds=bounds, cap=cap, member=member, waived=waived)
                    source = os.path.join(folder, "request.json")
                    target = os.path.join(folder, "result.json")
                    with open(source, "w", encoding="utf-8") as stream:
                        json.dump(request, stream)
                    env = os.environ.copy()
                    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
                    env["PYTHONPATH"] = root + os.pathsep + env.get("PYTHONPATH", "")
                    with tempfile.TemporaryFile(mode="w+b") as log:
                        if self._closed or generation != self._generation:
                            continue
                        process = subprocess.Popen(
                            [sys.executable, "-m", "floe.drc_query_worker", source, target],
                            stdout=subprocess.DEVNULL, stderr=log, env=env)
                        _children.add(process)
                        self._process_pid = process.pid
                        while process.poll() is None:
                            with self._condition:
                                if self._closed or generation != self._generation:
                                    process.terminate()
                                    break
                                self._condition.wait(timeout=0.05)
                        try:
                            process.wait(timeout=1)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait()
                        if generation != self._generation or self._closed:
                            continue
                        if process.returncode:
                            log.seek(0, os.SEEK_END)
                            log.seek(max(0, log.tell() - 4096))
                            raise RuntimeError(log.read().decode("utf-8", errors="replace").strip()
                                               or "in-view query process failed")
                    with open(target, encoding="utf-8") as stream:
                        result = json.load(stream)
            except Exception as exc:
                error = str(exc)
            finally:
                self._process_pid = None
                if process is not None and process.poll() is None:
                    process.kill()
                    process.wait()
                if process is not None:
                    _children.discard(process)
            with self._condition:
                if not self._closed and generation == self._generation:
                    self._result = (key, result, error)


def main(source, target):
    from .drc import IcePack
    from .drc_marker_worker import import_members
    try:
        os.nice(5)
    except (AttributeError, OSError):
        pass
    with open(source, encoding="utf-8") as stream:
        request = json.load(stream)
    db = IcePack(request["pack"], review=False, review_path=request["review_path"])
    try:
        if db._analysis_identity != request["identity"]:
            raise ValueError("DRC pack changed; reload the results database")
        member = import_members(request["member"])
        ci = request["ci"]
        values = db.query_rect(*request["bounds"], cap=request["cap"], checks=(ci,),
                               members={ci: member} if member is not None else None,
                               waived=request["waived"])
        with open(target, "w", encoding="utf-8") as stream:
            json.dump([(ci, ei, error.kind, error.pts) for ci, ei, error in values], stream)
    finally:
        db.close()


if __name__ == "__main__":
    main(*sys.argv[1:])
