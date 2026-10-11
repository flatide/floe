"""Carry review state across DRC reruns after exact native geometry matching.

All previous packs and reviewer files are read-only. The native external
sorter compares complete integer geometry; Python streams its target-sorted
records into sparse snapshots and applies bounded target chunks. Ordinary
peer imports still never copy visits: this module is the explicit exception
for a proven unchanged error in a new run.
"""

from collections import Counter
from functools import lru_cache
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import tempfile
import time

import numpy as np

from . import cachepath, drc_team as team
from .drc import IcePack, STATUS_NONE, STATUS_WAIVED, waive_autosave_path, _waive_tmp_fallback
from .drc_analysis import pack_identity
from .drc_review import CHUNK, VISITED, NOT_REQUIRED, review_path, _fallback_path, _exists


PROTOCOL = 2
_RECORD = np.dtype([("ci", "<u4"), ("flags", "u1"), ("disposition", "u1"),
                    ("unknown", "u1"), ("reserved", "u1"), ("ei", "<u8")])
_BYTES = CHUNK // 8


class _Progress:
    def __init__(self, callback):
        self.callback = callback
        self.last = 0.0

    def __call__(self, done, total, text="", final=False):
        now = time.monotonic()
        if self.callback is not None and (final or now - self.last >= 0.5):
            self.last = now
            self.callback(int(done), int(total), str(text))


def _check(cancelled):
    if cancelled is not None and cancelled():
        raise InterruptedError("review migration cancelled; completed application batches remain applied")


def _resolve_pack(path):
    """Find existing packs without the normal legacy-cache rename."""
    path = os.path.abspath(os.fspath(path))
    with open(path, "rb") as stream:
        magic = stream.read(8)
    if magic == b"FLOEICE\x00":
        return path, None
    for candidate in (cachepath.pack_path(path), cachepath.legacy_pack_path(path)):
        if _exists(candidate):
            return candidate, path
    raise ValueError("previous DRC result needs an existing valid .tray pack; index it first")


def _existing_primary(primary, fallback=None):
    if _exists(primary):
        return primary
    return fallback if fallback is not None and _exists(fallback) else None


def _prior_source(old, state_path, reviewer, folder, cancelled, report):
    if state_path is not None:
        return team.open_review(old, os.fspath(state_path), cancelled=cancelled,
                                progress=lambda a, b: report(a, b, "Read previous review"))
    raw = _existing_primary(review_path(old.path, reviewer))
    if raw is None and not os.environ.get("FLOE_DRC_REVIEW_ROOT"):
        raw = _existing_primary(_fallback_path(old.path, reviewer))
    waive = _existing_primary(waive_autosave_path(old.path, reviewer),
                              _waive_tmp_fallback(old.path, reviewer))
    if raw is not None or waive is not None:
        return team.open_review(old, raw or waive, waive_path=waive,
                                reviewer=reviewer, cancelled=cancelled,
                                progress=lambda a, b: report(a, b, "Read previous review"))
    # A retired pack may itself contain the only saved waived decisions.
    # Visited/exclusion remain unknown when no personal sidecar exists.
    if not any(old.status_counts(ci)[0] for ci in range(len(old.checks))):
        raise ValueError("no saved review state for reviewer %s in previous DRC result; choose a review snapshot" % reviewer)
    path = os.path.join(folder, "embedded-review.sqlite3")
    with team._destination(old, path, reviewer, dict(visited_known=False,
            not_required_known=False, waive_known=True, source_path=old.path)) as writer:
        for ci, size in enumerate(old._ecnt):
            if not old.status_counts(ci)[0]:
                continue
            es = int(old._dir_es[ci])
            for start in range(0, int(size), 65536):
                _check(cancelled)
                values = old._status[es + start:es + min(int(size), start + 65536)]
                packed = np.packbits(values == STATUS_WAIVED, bitorder="little")
                for offset in range(0, len(packed), _BYTES):
                    w = int.from_bytes(packed[offset:offset + _BYTES].tobytes(), "little")
                    if w:
                        writer.add(ci, start // CHUNK + offset // _BYTES, 0, w, 0)
                report(es + start + len(values), old.total, "Read embedded waives")
    result = team.ReviewSnapshot(old, path)
    result.original_path = old.path
    return result


def _freeze_source(old, source, path, cancelled, report):
    """Copy from the validated connection, never reopen a replaceable path."""
    destination = sqlite3.connect(path)
    try:
        def copied(_status, remaining, total):
            _check(cancelled)
            report(total - remaining, total, "Freeze previous review snapshot")
        with source._lock:
            source._conn.backup(destination, pages=256, progress=copied)
    finally:
        destination.close()
    frozen = team.ReviewSnapshot(old, path)
    frozen.original_path = getattr(source, "original_path", source.path)
    return frozen


@lru_cache(maxsize=8)
def _compatible(path, size, mtime_ns, ctime_ns):
    try:
        result = subprocess.run([path, "drc-match", "--protocol"],
            stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=5)
        return result.returncode == 0 and result.stdout.strip() == str(PROTOCOL)
    except (OSError, subprocess.SubprocessError):
        return False


def _binary():
    # The branch's floe-index drc-match moved to app-core (drc::review::matcher);
    # `floe2 drc-match` speaks the same protocol 2 for this reference.
    path = os.environ.get("FLOE2_BIN") or os.path.join(
        os.path.dirname(__file__), "..", "..", "..", "rust", "target", "release", "floe2")
    path = os.path.abspath(path)
    st = os.stat(path)
    if not _compatible(path, st.st_size, st.st_mtime_ns, st.st_ctime_ns):
        raise RuntimeError("floe2 lacks exact DRC review migration; rebuild the Rust binaries")
    return path


def _worker_count():
    jobs = os.environ.get("FLOE_DRC_JOBS")
    if jobs is None:
        return min(4, os.cpu_count() or 1)
    value = int(jobs)
    if not 1 <= value <= 256:
        raise ValueError("DRC worker count must be an integer from 1 to 256")
    return value


def _stop_child(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def _match(old, target, pairs, states, folder, cancelled, report):
    pairs_path = os.path.join(folder, "pairs.json")
    with open(pairs_path, "w", encoding="utf-8") as stream:
        json.dump(pairs, stream)
    output = os.path.join(folder, "matches.bin")
    progress_path = os.path.join(folder, "native-progress.json")
    report_path = os.path.join(folder, "native-report.json")
    command = [_binary(), "drc-match", os.fspath(old.path), os.fspath(target.path),
        "--plan", pairs_path, "--states", states, "--out", output,
        "--work-dir", os.path.join(folder, "sort"), "--jobs", str(_worker_count()),
        "--progress", progress_path, "--report", report_path]
    _check(cancelled)
    with open(os.path.join(folder, "native.log"), "w+b") as log:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
        try:
            while process.poll() is None:
                _check(cancelled)
                try:
                    with open(progress_path, encoding="utf-8") as stream:
                        value = json.load(stream)
                    update = (int(value["processed"]), int(value["total"]),
                              value.get("phase", "Match geometry"))
                except (OSError, ValueError, KeyError, TypeError):
                    update = None
                if update is not None:
                    report(*update)
                time.sleep(0.025)
            _check(cancelled)
            if process.returncode:
                log.seek(0, os.SEEK_END)
                log.seek(max(0, log.tell() - 6000))
                raise RuntimeError("exact DRC matching failed: " + log.read().decode("utf-8", "replace").strip())
            with open(report_path, encoding="utf-8") as stream:
                metadata = json.load(stream)
            return output, metadata
        finally:
            _stop_child(process)


def _write_states(old, source, path, cancelled, report):
    with open(path, "xb") as stream:
        stream.truncate(old.total)
    if not old.total:
        return
    states = np.memmap(path, mode="r+", dtype=np.uint8, shape=(old.total,))
    try:
        for ci, k, v, w, n, _c, _a in source.iter_chunks():
            _check(cancelled)
            count = min(CHUNK, int(old._ecnt[ci]) - k * CHUNK)
            if count <= 0 or (v | w | n) >> count:
                raise ValueError("previous review bitmap is outside its rule")
            values = np.zeros(count, dtype=np.uint8)
            for bits, flag in ((v, 1), (w, 2), (n, 4)):
                if bits:
                    values |= np.unpackbits(np.frombuffer(bits.to_bytes(_BYTES, "little"),
                        dtype=np.uint8), bitorder="little")[:count] * flag
            start = int(old._dir_es[ci]) + k * CHUNK
            states[start:start + count] = values
            report(start + count, old.total, "Prepare previous review flags")
        states.flush()
    finally:
        states._mmap.close()


def _mapped_snapshots(target, source, records_path, folder, cancelled, report):
    length = os.path.getsize(records_path)
    if length % _RECORD.itemsize:
        raise ValueError("truncated geometry match records")
    rows = (np.memmap(records_path, mode="r", dtype=_RECORD, shape=(length // _RECORD.itemsize,))
            if length else np.empty(0, dtype=_RECORD))
    metadata = dict(visited_known=source.visited_known, waive_known=source.waive_known,
        not_required_known=source.not_required_known, migration=True,
        prior_fingerprint=source.fingerprint, prior_snapshot_id=source.snapshot_id)
    mapped_path = os.path.join(folder, "mapped.sqlite3")
    ambiguous_path = os.path.join(folder, "ambiguous.sqlite3")
    eligibility_path = os.path.join(folder, "eligibility.sqlite3")
    current = None
    planes = [0] * 8  # actual V/W/NR, ambiguity, exact, unknown W/NR, partial
    previous = None
    try:
        with team._destination(target, mapped_path, source.reviewer, metadata) as matched, \
                team._destination(target, ambiguous_path, "ambiguous-prior", dict(aggregate=True)) as ambiguous, \
                team._destination(target, eligibility_path, "migration-eligibility", dict(aggregate=True)) as eligible:
            def flush():
                if current is not None:
                    ci, k = current
                    v, w, n, a, e, uw, un, partial = planes
                    matched.add(ci, k, v, w, n)
                    ambiguous.add(ci, k, a, 0, 0, a, a)
                    # Internal membership/masks, never actual review flags.
                    eligible.add(ci, k, e, uw, un, partial, e)

            for start in range(0, len(rows), 65536):
                _check(cancelled)
                batch = rows[start:start + 65536]
                cis, eis = batch["ci"], batch["ei"]
                if (np.any(cis >= len(target._ecnt)) or np.any(batch["flags"] > 7)
                        or np.any(batch["disposition"] > 1) or np.any(batch["reserved"] != 0)
                        or np.any((batch["unknown"] & ~np.uint8(6)) != 0)
                        or np.any((batch["unknown"] & batch["flags"]) != 0)
                        or np.any((batch["disposition"] == 1) & ((batch["flags"] | batch["unknown"]) != 0))):
                    raise ValueError("invalid geometry match record")
                if np.any(eis >= target._ecnt[cis]):
                    raise ValueError("geometry match error is outside target rule")
                if len(batch):
                    first = (int(cis[0]), int(eis[0]))
                    if previous is not None and first <= previous:
                        raise ValueError("geometry match records are not unique and sorted")
                    if np.any((cis[1:] < cis[:-1]) | ((cis[1:] == cis[:-1]) & (eis[1:] <= eis[:-1]))):
                        raise ValueError("geometry match records are not unique and sorted")
                    previous = (int(cis[-1]), int(eis[-1]))
                chunks = eis // CHUNK
                cuts = np.r_[0, np.flatnonzero((cis[1:] != cis[:-1]) | (chunks[1:] != chunks[:-1])) + 1, len(batch)]
                for left, right in zip(cuts[:-1], cuts[1:]):
                    group = batch[int(left):int(right)]
                    key = (int(group["ci"][0]), int(group["ei"][0] // CHUNK))
                    if current != key:
                        flush()
                        current, planes = key, [0] * 8
                    local = (group["ei"] % CHUNK).astype(np.int64)
                    agreed = group["disposition"] == 0
                    unknown = group["unknown"]
                    if np.any(agreed & ((group["flags"] | unknown) == 0)):
                        raise ValueError("empty previous state cannot be transferred")
                    masks = [agreed & ((group["flags"] & flag) != 0) for flag in (1, 2, 4)]
                    masks.extend((~agreed | (unknown != 0), agreed,
                                  agreed & ((unknown & 2) != 0),
                                  agreed & ((unknown & 4) != 0), agreed & (unknown != 0)))
                    for slot, take in enumerate(masks):
                        packed = np.zeros(_BYTES, dtype=np.uint8)
                        positions = local[take]
                        np.bitwise_or.at(packed, positions // 8, (1 << (positions % 8)).astype(np.uint8))
                        planes[slot] |= int.from_bytes(packed.tobytes(), "little")
                report(start + len(batch), len(rows), "Build mapped review")
            flush()
            _check(cancelled)
    finally:
        if isinstance(rows, np.memmap):
            rows._mmap.close()
    return (team.ReviewSnapshot(target, mapped_path), team.ReviewAggregate(target, ambiguous_path),
            team.ReviewAggregate(target, eligibility_path))


def _merge_conflicts(target, ambiguous, protected, path, cancelled):
    left, right = iter(ambiguous.iter_chunks()), iter(protected.iter_chunks())
    try:
        a, b = next(left, None), next(right, None)
        with team._destination(target, path, "migration-conflicts", dict(aggregate=True)) as writer:
            while a is not None or b is not None:
                _check(cancelled)
                key = min(row[:2] for row in (a, b) if row is not None)
                bits = 0
                if a is not None and a[:2] == key:
                    bits |= a[5]
                    a = next(left, None)
                if b is not None and b[:2] == key:
                    bits |= b[5]
                    b = next(right, None)
                writer.add(*key, bits, 0, 0, bits, bits)
    finally:
        left.close()
        right.close()
    return team.ReviewAggregate(target, path)


def _different(source, own_w, own_n, incoming_w, incoming_n, unknown_w, unknown_n):
    return (((own_w ^ incoming_w) & ~unknown_w) if source.waive_known else 0) | \
           (((own_n ^ incoming_n) & ~unknown_n) if source.not_required_known else 0)


def _protected_conflicts(target, plan, path, cancelled, progress):
    store, source = target._review_flags_store(), plan.mapped
    total = plan.eligibility.counts()["visited"]
    processed = 0
    report = _Progress(progress)
    rows = plan.eligibility.iter_chunks()
    try:
        with team._destination(target, path, "protected-current-review", dict(aggregate=True)) as writer:
            for ci, k, eligible, unknown_w, unknown_n, _partial, _a in rows:
                _check(cancelled)
                _v, w, n, _c, _work = source._chunk(ci, k)
                with target._status_lock, store._write_lock:
                    ov, ow, on, _before = team._own_bits(target, store, ci, k)
                    conflict = eligible & ov & _different(source, ow, on, w, n, unknown_w, unknown_n)
                if conflict:
                    writer.add(ci, k, conflict, 0, 0, conflict, conflict)
                processed += eligible.bit_count()
                report(processed, total, "Check current review conflicts")
            _check(cancelled)
    finally:
        rows.close()
    return team.ReviewAggregate(target, path)


class MigrationPlan:
    """Prepared immutable source mapping plus current protected-conflict view."""
    def __init__(self, target, owner, source, mapped, ambiguous, eligibility, summary):
        self._owner, self.mapped, self.ambiguous = owner, mapped, ambiguous
        self.eligibility = eligibility
        self.conflicts = self.protected = None
        self.summary = summary
        self.source_path = summary["source_path"]
        self.state_path = summary["state_path"]
        self.reviewer = source.reviewer
        self.target_identity = pack_identity(target.path)
        self._closed = False

    def _validate(self, target):
        if self._closed:
            raise OSError("migration plan is closed")
        team._identity(target, self.mapped.metadata)
        if pack_identity(target.path) != self.target_identity:
            raise ValueError("target DRC pack changed; prepare migration again")

    def refresh_conflicts(self, target, cancelled=None, progress=None):
        self._validate(target)
        stamp = time.time_ns()
        protected_path = os.path.join(self._owner.name, "protected-%d.sqlite3" % stamp)
        protected = _protected_conflicts(target, self, protected_path, cancelled, progress)
        try:
            path = os.path.join(self._owner.name, "conflicts-%d.sqlite3" % stamp)
            value = _merge_conflicts(target, self.ambiguous, protected, path, cancelled)
        except Exception:
            protected.close()
            os.unlink(protected_path)
            raise
        previous = (self.conflicts, self.protected)
        self.conflicts, self.protected = value, protected
        for snapshot in previous:
            if snapshot is not None:
                old_path = snapshot.path
                snapshot.close()
                os.unlink(old_path)
        self.summary["conflict"] = value.counts()["conflicts"]
        self.summary["protected"] = protected.counts()["conflicts"]

    def rule_counts(self, ci=None):
        eligible = self.eligibility.counts(ci)
        matched = eligible["visited"]
        partial = eligible["conflicts"]
        ambiguous = self.ambiguous.counts(ci)["conflicts"]
        conflict = self.conflicts.counts(ci)["conflicts"] if self.conflicts is not None else ambiguous
        protected = self.protected.counts(ci)["conflicts"] if self.protected is not None else 0
        return dict(total=eligible["total"], matched=matched, ambiguous=ambiguous, partial=partial,
                    conflict=conflict, protected=protected,
                    unmatched=eligible["total"] - matched - ambiguous + partial)

    def members(self, ci, kind="matched"):
        if self._closed:
            raise OSError("migration plan is closed")
        if kind == "matched":
            return self.eligibility.members(ci, "visited")
        if kind == "partial":
            return self.eligibility.members(ci, "conflict")
        if kind == "ambiguous":
            return self.ambiguous.members(ci, "conflict")
        if kind == "conflict":
            return (self.conflicts or self.ambiguous).members(ci, "conflict")
        if kind == "protected":
            if self.protected is None:
                raise ValueError("current review conflicts have not been prepared")
            return self.protected.members(ci, "conflict")
        raise ValueError("unsupported migration membership")

    def page(self, ci, start, limit, kind="matched"):
        return self.members(ci, kind).page(start, limit)

    def get(self, ci, ei):
        ci, size = self.eligibility._rule(ci)
        ei = int(ei)
        if not 0 <= ei < size:
            raise IndexError(ei)
        eligible, unknown_w, unknown_n, _partial, _a = self.eligibility._chunk(ci, ei // CHUNK)
        bit = 1 << (ei % CHUNK)
        if not eligible & bit and self.ambiguous.members(ci, "conflict").contains(ei):
            return None
        value = self.mapped.get(ci, ei)
        return team.ReviewState(value.visited if self.mapped.visited_known else None,
            value.waived if self.mapped.waive_known and not unknown_w & bit else None,
            value.not_required if self.mapped.not_required_known and not unknown_n & bit else None)

    def close(self):
        if self._closed:
            return
        self._closed = True
        for value in (self.mapped, self.ambiguous, self.eligibility, self.conflicts, self.protected):
            if value is not None:
                value.close()
        self._owner.cleanup()

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass


def prepare_migration(target, old_path, state_path=None, *, cancelled=None, progress=None):
    """Read previous state and prepare exact rerun matches without applying."""
    _check(cancelled)
    target_identity = pack_identity(target.path)
    if target_identity != getattr(target, "_analysis_identity", target_identity):
        raise ValueError("target DRC pack changed; reload it before migration")
    old_pack, source_db = _resolve_pack(old_path)
    if os.path.samefile(old_pack, target.path):
        raise ValueError("choose a previous DRC result, not the currently open pack")
    owner = tempfile.TemporaryDirectory(prefix="floe-review-migration-")
    old = source = mapped = ambiguous = eligibility = plan = None
    report = _Progress(progress)
    try:
        old = IcePack(old_pack, src_path=source_db, verify_src=source_db is not None,
                      review=False, cache_write=False)
        if old._review_fingerprint == target._review_fingerprint:
            raise ValueError("previous and current packs describe the same DRC result")
        source = _prior_source(old, state_path, getattr(target, "_reviewer", team._reviewer(target)),
                               owner.name, cancelled, report)
        frozen = _freeze_source(old, source, os.path.join(owner.name, "prior.sqlite3"),
                                cancelled, report)
        source.close()
        source = frozen
        old_names = Counter(check.name for check in old.checks)
        new_names = Counter(check.name for check in target.checks)
        new_index = {check.name: ci for ci, check in enumerate(target.checks)}
        pairs = [[ci, new_index[check.name]] for ci, check in enumerate(old.checks)
                 if old_names[check.name] == 1 and new_names[check.name] == 1
                 and source.counts(ci)["work"]]
        eligible = sum(source.counts(ci)["work"] for ci, _new in pairs)
        states_path = os.path.join(owner.name, "old-states.u8")
        if pairs:
            _write_states(old, source, states_path, cancelled, report)
            records, native = _match(old, target, pairs, states_path, owner.name, cancelled, report)
        else:
            records, native = os.path.join(owner.name, "matches.bin"), {}
            with open(records, "xb"):
                pass
        _check(cancelled)
        if (pack_identity(old.path) != old._analysis_identity
                or pack_identity(target.path) != target_identity):
            raise ValueError("DRC pack changed while matching; prepare migration again")
        mapped, ambiguous, eligibility = _mapped_snapshots(target, source, records, owner.name, cancelled, report)
        # Keep only the compact preview/apply artifacts after the native
        # external sort. Billion-error raw flags/runs are no longer needed.
        os.unlink(records)
        if os.path.exists(states_path):
            os.unlink(states_path)
        shutil.rmtree(os.path.join(owner.name, "sort"), ignore_errors=True)
        total = sum(int(n) for n in target._ecnt)
        matched_count = eligibility.counts()["visited"]
        partial_count = eligibility.counts()["conflicts"]
        ambiguous_count = ambiguous.counts()["conflicts"]
        if ("matched_target" in native and int(native["matched_target"]) != matched_count
                or "ambiguous_target" in native and int(native["ambiguous_target"]) != ambiguous_count
                or "partial_target" in native and int(native["partial_target"]) != partial_count):
            raise ValueError("geometry match counts do not match their records")
        source_work = source.counts()["work"]
        summary = dict(native, total=total, matched=matched_count, ambiguous=ambiguous_count,
            partial=partial_count, unmatched=total - matched_count - ambiguous_count + partial_count,
            conflict=ambiguous_count,
            source_work=source_work, eligible_source_work=eligible,
            no_rule_match_source_work=source_work - eligible,
            source_path=os.path.abspath(os.fspath(old_path)), old_pack=old_pack,
            state_path=getattr(source, "original_path", source.path), reviewer=source.reviewer,
            prior_fingerprint=source.fingerprint, prior_snapshot_id=source.snapshot_id,
            rules=len(pairs))
        plan = MigrationPlan(target, owner, source, mapped, ambiguous, eligibility, summary)
        plan.refresh_conflicts(target, cancelled=cancelled, progress=report)
        _check(cancelled)
        report(total, total, "Previous review matches ready", final=True)
        return plan
    except Exception:
        if plan is not None:
            plan.close()
        else:
            for value in (mapped, ambiguous, eligibility):
                if value is not None:
                    value.close()
            owner.cleanup()
        raise
    finally:
        if source is not None:
            source.close()
        if old is not None:
            old.close()


def apply_migration(target, plan, *, cancelled=None, progress=None):
    """Apply exact matches, preserving existing visited disagreements.

    W and NR are completed before visits are ORed. A batch has at most 16
    target chunks; no per-error fsync or population-sized membership scan.
    Separate legacy waiver and SQLite files cannot be crash-atomic together.
    """
    _check(cancelled)
    plan._validate(target)
    target._require_review()
    store, source = target._review_flags_store(), plan.mapped
    report = _Progress(progress)
    result = dict(changed_visited=0, changed_waived=0, changed_not_required=0,
                  conflicts_skipped=0, applied=0, processed=0)
    total = plan.eligibility.counts()["visited"]

    def apply_batch(ci, batch):
        _check(cancelled)
        plan._validate(target)
        set_nr, clear_nr, visits = [], [], []
        with target._status_lock, store._write_lock:
            for _ci, k, work, unknown_w, unknown_n, _partial, _a in batch:
                v, w, n, _c, _work = source._chunk(ci, k)
                ov, ow, on, before = team._own_bits(target, store, ci, k)
                different = _different(source, ow, on, w, n, unknown_w, unknown_n)
                conflicts = ov & work & different
                eligible = work & ~conflicts
                result["conflicts_skipped"] += conflicts.bit_count()
                if source.waive_known:
                    change_w = eligible & ~unknown_w & (ow ^ w)
                    if change_w:
                        count = len(before)
                        mask = np.unpackbits(np.frombuffer(change_w.to_bytes(_BYTES, "little"), dtype=np.uint8), bitorder="little")[:count].astype(bool)
                        desired = np.unpackbits(np.frombuffer(w.to_bytes(_BYTES, "little"), dtype=np.uint8), bitorder="little")[:count].astype(bool)
                        after = before.copy()
                        after[mask] = np.where(desired[mask], STATUS_WAIVED, STATUS_NONE)
                        target._write_status_span(ci, k * CHUNK, before, after)
                        result["changed_waived"] += change_w.bit_count()
                if source.not_required_known:
                    set_nr.append((k, eligible & ~unknown_n & n & ~on))
                    clear_nr.append((k, eligible & ~unknown_n & on & ~n))
                if source.visited_known:
                    visits.append((k, eligible & v & ~ov))
                result["applied"] += eligible.bit_count()
                result["processed"] += work.bit_count()
            result["changed_not_required"] += store._change_many(ci, set_nr, NOT_REQUIRED, True)
            result["changed_not_required"] += store._change_many(ci, clear_nr, NOT_REQUIRED, False)
            result["changed_visited"] += store._change_many(ci, visits, VISITED, True)
        report(result["processed"], total, "Apply unchanged-error review")

    rows = plan.eligibility.iter_chunks()
    try:
        batch, ci = [], None
        for row in rows:
            if batch and (row[0] != ci or len(batch) >= 16):
                apply_batch(ci, batch)
                batch = []
            ci = row[0]
            batch.append(row)
        if batch:
            apply_batch(ci, batch)
        _check(cancelled)
        plan.refresh_conflicts(target, cancelled=cancelled, progress=report)
        report(total, total, "Previous review applied", final=True)
        return result
    finally:
        rows.close()
