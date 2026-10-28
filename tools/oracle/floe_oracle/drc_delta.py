"""Fixed-precision SVRF measurement differences and lazy DRC groups.

One numeric measurement table belongs to a rule. Changing grouping step,
basis, cluster or review filter never decodes its geometry again. Group
descriptors and member pages are constructed on demand, including for
rules with millions of distinct differences.
"""

import math
import operator
import re
import weakref
from collections import OrderedDict
from decimal import Decimal, DecimalException, ROUND_HALF_UP, localcontext

import numpy as np

from .drc import DrcError, IcePack, STATUS_WAIVED, _ICE2_BLOCK, _uv, _unzz, cd_segments


SCALE = 100000
_LIMIT = np.iinfo(np.int64).max
_CHUNK = 1 << 16
_PAGE_CHUNK = 4096
_UNKNOWN = -1


class DeltaQueryCancelled(Exception):
    pass


def _cancel(cancelled):
    if cancelled is not None and cancelled():
        raise DeltaQueryCancelled()


def value_ticks(value):
    """Round a displayed measurement to five decimals, ties away from zero."""
    try:
        value = Decimal(str(value))
        if not value.is_finite():
            raise ValueError("measurement must be finite")
        if value.is_zero():
            return 0
        if value.adjusted() > 13:
            raise ValueError("measurement exceeds fixed-precision range")
        if value.adjusted() < -6:
            return 0
        with localcontext() as ctx:
            ctx.prec = max(32, len(value.as_tuple().digits) + 8)
            ticks = int((value * SCALE).to_integral_value(rounding=ROUND_HALF_UP))
    except (DecimalException, TypeError, OverflowError) as exc:
        raise ValueError("invalid measurement") from exc
    if not -_LIMIT <= ticks <= _LIMIT:
        raise ValueError("measurement exceeds fixed-precision range")
    return ticks


def parse_step(text):
    """Positive grouping step with at most five meaningful decimal places."""
    try:
        value = Decimal(str(text).strip())
        if not value.is_finite() or value <= 0:
            raise ValueError("step must be a positive finite number")
        if value.adjusted() > 13:
            raise ValueError("step exceeds fixed-precision range")
        digits = value.as_tuple().digits
        exponent = value.as_tuple().exponent
        end = len(digits)
        while end and digits[end - 1] == 0:
            end -= 1
            exponent += 1
        if exponent < -5:
            raise ValueError("step supports at most five decimal places")
        with localcontext() as ctx:
            ctx.prec = max(32, len(digits) + 8)
            ticks = int(value * SCALE)
    except (DecimalException, TypeError, OverflowError) as exc:
        raise ValueError("invalid grouping step") from exc
    if ticks > _LIMIT:
        raise ValueError("step exceeds fixed-precision range")
    return ticks


def format_ticks(ticks, signed=False):
    ticks = int(ticks)
    sign = "-" if ticks < 0 else "+" if signed else ""
    whole, fractional = divmod(abs(ticks), SCALE)
    return "%s%d.%05d" % (sign, whole, fractional)


def percent_ticks(delta, bound):
    """Absolute percentage to five decimals, using exact integer arithmetic."""
    delta, bound = int(delta), int(bound)
    if not bound:
        return None
    numerator, denominator = abs(delta) * 100 * SCALE, abs(bound)
    value, remainder = divmod(numerator, denominator)
    if remainder * 2 >= denominator:
        value += 1
    return value if value <= _LIMIT else None


def measurement_ticks(pick):
    """The same displayed CD, bound and absolute difference used for groups."""
    _ci, bound, measurement = pick
    measured, bound = value_ticks(measurement), value_ticks(bound)
    delta = abs(measured - bound)
    if delta > _LIMIT:
        raise ValueError("measurement difference exceeds fixed-precision range")
    return measured, bound, delta


def measurement_candidates(error, metric):
    """Distinct geometry-derived CDs, before applying a rule condition.

    Rectangles expose both spans. An edge pair exposes only its true
    closest gap: later ``cd_segments`` entries are XY ruler diagnostics,
    never additional CD candidates. Complex geometry stays unmeasurable.
    """
    pts = error.pts
    if metric == "area":
        if error.kind != "p" or len(pts) < 3:
            return ()
        # Translating before the shoelace products prevents a tiny polygon
        # far from the origin losing its entire area to cancellation.
        ox, oy = pts[0]
        def terms():
            for i, (x0, y0) in enumerate(pts):
                x1, y1 = pts[(i + 1) % len(pts)]
                yield ((x0 - ox) * (y1 - oy) -
                       (x1 - ox) * (y0 - oy))
        values = (abs(math.fsum(terms())) / 2,)
    elif metric in _CD_METRICS:
        if error.kind == "p" or (error.kind == "e" and len(pts) == 4):
            segments = cd_segments(error)
            if not segments:
                return ()
            if error.kind == "e":
                segments = segments[:1]
            values = tuple(math.hypot(x1 - x0, y1 - y0)
                           for x0, y0, x1, y1 in segments)
        else:
            return ()
    elif metric == "length" and error.kind == "e" and len(pts) == 2:
        values = (math.hypot(pts[1][0] - pts[0][0],
                            pts[1][1] - pts[0][1]),)
    else:
        return ()
    return tuple(sorted(set(v for v in values if math.isfinite(v))))


def measured(error, metric):
    """Legacy scalar measurement; rule-aware selection uses the selector."""
    candidates = measurement_candidates(error, metric)
    return candidates[0] if candidates else None


class MeasurementPick(tuple):
    """Backward-compatible ``(constraint index, bound, CD)`` plus provenance."""

    def __new__(cls, ci, bound, cd, estimated=False, reason="", candidates=()):
        result = super().__new__(cls, (ci, bound, cd))
        result.estimated = bool(estimated)
        result.reason = str(reason)
        result.candidates = tuple(candidates)
        return result


_PREDICATES = {"<": operator.lt, "<=": operator.le,
               ">": operator.gt, ">=": operator.ge,
               "==": operator.eq, "!=": operator.ne}
_UPPER = ("<", "<=", "==")
_CD_METRICS = ("width", "space", "notch", "enclosure", "overlap", "extension")
_SUPPORTED = frozenset(_CD_METRICS + ("length", "area"))
_BOUND = re.compile(r"(?<![A-Za-z_])(?:<=|>=|==|!=|<|>)\s*[A-Za-z0-9_.+\-]+")
_LEADING_BOUND = re.compile(r"^(?:<=|>=|==|!=|<|>)")
_WORD = re.compile(r"[A-Za-z_][A-Za-z0-9_.\-]*")


def _has_options(text, allowed_tail=()):
    """Flag unevaluated options without extracting any option as a bound."""
    if not text:
        return False
    first = _BOUND.search(text)
    if first is None:
        return False
    end = first.end()
    while True:
        rest = text[end:]
        offset = len(rest) - len(rest.lstrip())
        following = _BOUND.match(text, end + offset)
        if following is None:
            if rest.strip() and rest.strip().upper() not in allowed_tail:
                return True
            break
        end = following.end()
    # Options can precede the bounds too. The existing metadata reader's
    # keyword set is only used as a warning, never as an evaluator/parser.
    from .svrf import KEYWORDS
    words = _WORD.findall(text[:first.start()])
    return any(word.upper() in KEYWORDS for word in words[1:])


def _coordinate_roundoff(error, area=False):
    """A few arithmetic ulps in the measurement's units, not display ticks."""
    ulp = max((math.ulp(v) for point in error.pts for v in point
               if math.isfinite(v)), default=0.0)
    if area:
        pts = error.pts
        perimeter = math.fsum(math.hypot(pts[(i + 1) % len(pts)][0] - x,
                                        pts[(i + 1) % len(pts)][1] - y)
                              for i, (x, y) in enumerate(pts))
        ulp *= perimeter
    return 4 * ulp


def _compare_candidate(cd, bound, predicate, coordinate_roundoff):
    """Return (matches, changed by roundoff guard) for a raw measurement."""
    exact = predicate(cd, bound)
    tolerance = max(coordinate_roundoff, 4 * math.ulp(cd), 4 * math.ulp(bound))
    if cd != bound and abs(cd - bound) <= tolerance:
        guarded = predicate(bound, bound)
        return guarded, guarded != exact
    return exact, False


class MeasurementSelector:
    """Compile evidenced statement chains once, then choose a CD per error.

    Bounds on the same text line form a conjunction. Wrapped comparator
    lines carry cumulative text in existing sidecars; only adjacent,
    same-metric records with that prefix evidence are joined. Unrelated
    statements are never combined into an invented range.
    """

    def __init__(self, constraints):
        self.constraints = tuple(dict(con) for con in constraints)
        chains = []
        self._uncertain_ruler_options = False
        for ci, con in enumerate(self.constraints):
            metric = con.get("metric")
            text = " ".join(str(con.get("text") or "").split())
            previous = chains[-1] if chains else None
            same = bool(previous is not None and text and
                        metric == previous["metric"] and
                        (text == previous["text"] or
                         (text.startswith(previous["text"] + " ") and
                          _LEADING_BOUND.match(text[len(previous["text"]):]
                                               .lstrip()))))
            if not same:
                previous = {"metric": metric, "text": text, "records": [],
                            "options": False}
                chains.append(previous)
            previous["text"] = text
            previous["options"] |= _has_options(text)
            # NOTCH/SPACE only classify which EXTERNAL results are emitted;
            # other options may alter geometry or the distance metric. Keep
            # both rectangle directions when those semantics are unevaluated.
            allowed = ("NOTCH", "SPACE") if metric in ("space", "notch") else ()
            self._uncertain_ruler_options |= _has_options(text, allowed)
            value = con.get("value")
            try:
                numeric = float(value)
                if not math.isfinite(numeric):
                    numeric = None
            except (TypeError, ValueError, OverflowError):
                numeric = None
            previous["records"].append((ci, con.get("op"), value, numeric))
        self._chains = []
        self._uncertain_alternatives = False
        for chain in chains:
            records = tuple(chain["records"])
            if chain["metric"] not in _SUPPORTED or any(
                    op not in _PREDICATES or numeric is None
                    for _ci, op, _value, numeric in records):
                self._uncertain_alternatives = True
                continue
            representative = next((r for r in records if r[1] in _UPPER),
                                  records[0])
            predicates = tuple((_PREDICATES[op], numeric)
                               for _ci, op, _value, numeric in records)
            self._chains.append((chain["metric"], representative, predicates,
                                 chain["options"]))

    @property
    def can_measure(self):
        return bool(self._chains)

    def ruler_segments(self, error):
        """Keep rectangle directions that satisfy a dimensional error check.

        Rulers show every eligible direction, even when pick() chooses one
        worst-case CD for grouping. Without enough evidence to exclude a
        direction, keep the two geometric spans as before.
        """
        if self.constraints and all(c.get("metric") == "area" for c in self.constraints):
            return []  # Area checks use a polygon-area label, not length rulers.
        segments = cd_segments(error)
        if error.kind != "p" or len(segments) != 2:
            return segments
        if (not self._chains or self._uncertain_alternatives or
                self._uncertain_ruler_options or
                any(metric not in _CD_METRICS
                    for metric, _rep, _predicates, _options in self._chains)):
            return segments
        roundoff = _coordinate_roundoff(error)
        matched = []
        for segment in segments:
            x0, y0, x1, y1 = segment
            cd = math.hypot(x1 - x0, y1 - y0)
            if any(all(_compare_candidate(cd, bound, predicate, roundoff)[0]
                       for predicate, bound in predicates)
                   for _metric, _rep, predicates, _options in self._chains):
                matched.append(segment)
        return matched or segments

    def pick(self, error):
        measurements = {}
        roundoffs = {}
        matches = []
        numerical_uncertainty = False
        for metric, representative, predicates, options in self._chains:
            cache_key = "cd" if metric in _CD_METRICS else metric
            if cache_key not in measurements:
                measurements[cache_key] = measurement_candidates(error, metric)
            candidates = measurements[cache_key]
            if not candidates:
                continue
            area = metric == "area"
            if area not in roundoffs:
                roundoffs[area] = _coordinate_roundoff(error, area)
            survivors = []
            for cd in candidates:
                accepted = True
                for predicate, bound in predicates:
                    match, uncertain = _compare_candidate(cd, bound, predicate,
                                                          roundoffs[area])
                    numerical_uncertainty |= uncertain
                    accepted &= match
                if accepted:
                    survivors.append(cd)
            if not survivors:
                continue
            ci, op, bound, numeric = representative
            if op in (">", ">="):
                actual = survivors[-1]
            elif op == "!=":
                actual = max(survivors, key=lambda cd: (abs(cd - numeric), cd))
            else:
                actual = survivors[0]
            reasons = []
            if len(candidates) > 1:
                reasons.append("rectangle CD inferred from the rule condition")
                if len(survivors) > 1:
                    reasons.append("multiple dimensions match; selected the "
                                   "maximum" if op in (">", ">=") else
                                   "multiple dimensions match; selected the "
                                   "farthest from the bound" if op == "!=" else
                                   "multiple dimensions match; selected the minimum")
            if options:
                reasons.append("SVRF options are not geometrically evaluated")
            pick = MeasurementPick(ci, bound, actual, bool(reasons),
                                   "; ".join(reasons), candidates)
            matches.append(((metric, op, numeric, actual), pick))
        if not matches:
            return None
        criterion, result = matches[0]
        if any(other != criterion for other, _pick in matches[1:]):
            # The current sidecar also includes intermediate assignments.
            # It cannot tell which unrelated statement produced the error.
            return None
        # Equivalent duplicates keep the first criterion ID while retaining
        # uncertainty from every form. Unknown source statements also make
        # an otherwise unique association provisional.
        reasons = list(dict.fromkeys(reason for _criterion, pick in matches
                                     for reason in pick.reason.split("; ") if reason))
        if numerical_uncertainty:
            reasons.append("comparison is within floating-point roundoff of the bound")
        if self._uncertain_alternatives:
            reasons.append("unresolved or unsupported alternative constraints "
                           "leave the producing statement uncertain")
        return MeasurementPick(result[0], result[1], result[2], bool(reasons),
                               "; ".join(reasons), result.candidates)


def pick_constraint(error, constraints):
    """Rule-aware pick; repeated error scans should reuse a selector."""
    return MeasurementSelector(constraints).pick(error)


def ruler_segments(error, constraints=()):
    """Rule-aware display rulers, in um, shared by the viewer and snapshots."""
    return MeasurementSelector(constraints or ()).ruler_segments(error)


def area_label(error, constraints=()):
    """Return (x_um, y_um, text) for an AREA marker, or None.

    Reuse the polygon measurement consumed by details and grouping. The
    bounding box only anchors the label; it never supplies the area value.
    Mixed checks retain their dimensional rulers alongside this label.
    """
    if not any(c.get("metric") == "area" for c in (constraints or ())):
        return None
    try:
        candidates = measurement_candidates(error, "area")
        if not candidates:
            return None
        text = "area %s µm²" % format_ticks(value_ticks(candidates[0]))
    except (ValueError, OverflowError):
        return None
    x, y = error.center()
    return x, y, text


class DeltaIndex:
    """A rule's measurement table, independent of grouping and review state."""

    def __init__(self, db, ci, constraints):
        self.db, self.ci = db, int(ci)
        self.constraints = tuple(dict(c) for c in constraints)
        self._selector = MeasurementSelector(self.constraints)
        self.measured_ticks = None
        self.constraint_indices = None
        self.estimated_flags = None
        bounds = []
        for con in self.constraints:
            try:
                bounds.append(value_ticks(con.get("value")))
            except (ValueError, TypeError):
                bounds.append(None)
        self.bound_ticks = tuple(bounds)
        self._bound_values = np.asarray([0 if b is None else b for b in bounds],
                                        dtype=np.int64)
        self._auto_steps = {}

    def _errors(self, cancelled, blocks=None):
        db, ci = self.db, self.ci
        if not isinstance(db, IcePack):
            for ei, error in enumerate(db.checks[ci].errors):
                _cancel(cancelled)
                yield ei, error
            return
        # Do not touch IcePack._block's small mutable GUI cache from the
        # worker thread. Only one decoded error is retained at a time.
        n = len(db.checks[ci].errors)
        bs, es = int(db._dir_bs[ci]), int(db._dir_es[ci])
        buf, precision = db._map, db.precision
        if blocks is None:
            blocks = range((n + _ICE2_BLOCK - 1) // _ICE2_BLOCK)
        for block in blocks:
            _cancel(cancelled)
            rec = db._blk[bs + block]
            pos, count = int(rec["off"]), int(rec["cnt"])
            pfx = pfy = 0
            for j in range(count):
                _cancel(cancelled)
                kind_points, pos = _uv(buf, pos)
                npts = kind_points >> 1
                if not npts:
                    raise ValueError("packed DRC error has no vertices")
                d, pos = _uv(buf, pos)
                x = pfx + _unzz(d)
                d, pos = _uv(buf, pos)
                y = pfy + _unzz(d)
                pfx, pfy = x, y
                pts = [(x / precision, y / precision)]
                for k in range(npts - 1):
                    if not k % 1024:
                        _cancel(cancelled)
                    d, pos = _uv(buf, pos)
                    x += _unzz(d)
                    d, pos = _uv(buf, pos)
                    y += _unzz(d)
                    pts.append((x / precision, y / precision))
                ei = block * _ICE2_BLOCK + j
                yield ei, DrcError("e" if kind_points & 1 else "p",
                                   es + ei + 1, pts)

    def measure(self, cancelled=None):
        _cancel(cancelled)
        if self.measured_ticks is not None:
            return self
        n = len(self.db.checks[self.ci].errors)
        values = np.zeros(n, dtype=np.int64)
        choices = np.full(n, _UNKNOWN, dtype=np.int32)
        estimated = np.zeros(n, dtype=bool)
        return self._measure_arrays(values, choices, estimated, cancelled)

    def _measure_arrays(self, values, choices, estimated, cancelled=None,
                        progress=None):
        """Fill caller-owned arrays (disk maps in the preprocessing child)."""
        errors = self._errors(cancelled) if self._selector.can_measure else ()
        for ei, error in errors:
            if progress is not None and not ei % _CHUNK:
                progress(ei, len(values))
            self._record_measurement(ei, error, values, choices, estimated)
        _cancel(cancelled)
        # Publish only a complete table so cancellation is safely reusable.
        values.flags.writeable = choices.flags.writeable = estimated.flags.writeable = False
        self.measured_ticks, self.constraint_indices = values, choices
        self.estimated_flags = estimated
        return self

    def _record_measurement(self, ei, error, values, choices, estimated):
        pick = self._selector.pick(error)
        if pick is None or self.bound_ticks[pick[0]] is None:
            return
        try:
            actual, _bound, _delta = measurement_ticks(pick)
        except ValueError:
            return
        values[ei] = actual
        choices[ei] = pick[0]
        estimated[ei] = pick.estimated

    def _native_fallback(self, values, choices, estimated, cancelled=None,
                         progress=None):
        """Recheck only native precision sentinels, decoding touched blocks."""
        fallback = 0
        for start in range(0, len(choices), _CHUNK):
            _cancel(cancelled)
            ids = np.flatnonzero(choices[start:start + _CHUNK] == -2) + start
            if not len(ids):
                continue
            fallback += len(ids)
            if progress is not None:
                progress("Rechecking %d CD precision boundary cases" % fallback)
            blocks = np.unique(ids // _ICE2_BLOCK)
            for ei, error in self._errors(cancelled, blocks=blocks):
                if choices[ei] != -2:
                    continue
                values[ei], choices[ei], estimated[ei] = 0, _UNKNOWN, False
                self._record_measurement(ei, error, values, choices, estimated)
        return fallback

    def _difference_chunk(self, ids, mode, cancelled):
        """Shared exact delta/ratio calculation for ranges and membership."""
        _cancel(cancelled)
        choices = self.constraint_indices[ids].copy()
        differences = np.zeros(len(ids), dtype=np.int64)
        known = choices >= 0
        if not np.any(known):
            return choices, differences
        dest = np.flatnonzero(known)
        bounds = self._bound_values[choices[known]]
        # measure() checked that the difference's magnitude fits int64,
        # so both subtraction and abs are safe (including negative bounds).
        delta = np.abs(self.measured_ticks[ids[dest]] - bounds)
        if mode == "absolute":
            differences[dest] = delta
            return choices, differences
        # Integer ratios avoid float-bin boundary drift. The usual
        # semiconductor range takes the vector path; oversized ratios
        # use Python integers without overflowing or dropping a row.
        valid = bounds != 0
        choices[dest[~valid]] = _UNKNOWN
        safe = valid & (delta <= _LIMIT // (100 * SCALE))
        if np.any(safe):
            numerator = delta[safe] * (100 * SCALE)
            denominator = np.abs(bounds[safe])
            quotient, remainder = np.divmod(numerator, denominator)
            quotient += remainder >= (denominator // 2 + denominator % 2)
            differences[dest[safe]] = quotient
        for j in np.flatnonzero(valid & ~safe):
            _cancel(cancelled)
            value = percent_ticks(int(delta[j]), int(bounds[j]))
            if value is None:
                choices[dest[j]] = _UNKNOWN
            else:
                differences[dest[j]] = value
        return choices, differences

    def _automatic_step(self, mode, cancelled):
        """Whole-rule step targeting at most ten bins per criterion.

        Distinct criteria retain separate ranges and bins, including when
        their units differ. One UI step covers the widest criterion span;
        narrower spans can yield fewer groups. Cluster/review filtering
        never participates in this scan, including an empty cluster.
        """
        _cancel(cancelled)
        if mode in self._auto_steps:
            return self._auto_steps[mode]
        lows = np.full(len(self.constraints), _LIMIT, dtype=np.int64)
        highs = np.full(len(self.constraints), -1, dtype=np.int64)
        for start in range(0, len(self.measured_ticks), _CHUNK):
            _cancel(cancelled)
            ids = np.arange(start, min(start + _CHUNK, len(self.measured_ticks)),
                            dtype=np.int64)
            choices, values = self._difference_chunk(ids, mode, cancelled)
            known = choices >= 0
            np.minimum.at(lows, choices[known], values[known])
            np.maximum.at(highs, choices[known], values[known])
        # Use Python integers for inclusive span/ceil arithmetic: int64
        # cannot represent INT64_MAX + 1, even though both endpoints fit.
        ranges = [(int(lo), int(hi)) for lo, hi in zip(lows, highs) if hi >= 0]
        step = max(((hi - lo + 10) // 10 for lo, hi in ranges), default=1)
        step = max(1, step)
        if any(hi // step - lo // step + 1 > 10 for lo, hi in ranges):
            # Zero-anchored bins can straddle both range endpoints and add
            # an eleventh interval. span/9 bounds their count by ten.
            step = max(1, max((hi - lo + 8) // 9 for lo, hi in ranges))
        _cancel(cancelled)
        self._auto_steps[mode] = step
        return step

    def group(self, step_ticks=None, cluster=None, mode="absolute", cancelled=None):
        """Group a rule or cluster; None selects a cached whole-rule step."""
        automatic = step_ticks is None
        if not automatic:
            original_step = step_ticks
            step_ticks = int(step_ticks)
            if step_ticks != original_step or step_ticks <= 0 or step_ticks > _LIMIT:
                raise ValueError("grouping step must be positive fixed-precision ticks")
        if mode not in ("absolute", "percent"):
            raise ValueError("unknown grouping basis %r" % mode)
        self.measure(cancelled)
        if automatic:
            step_ticks = self._automatic_step(mode, cancelled)
        n = len(self.measured_ticks)
        # Numeric arrays are O(errors), not one Python object per member.
        included = np.ones(n, dtype=bool)
        if cluster is not None:
            for start in range(0, n, _CHUNK):
                _cancel(cancelled)
                included[start:start + _CHUNK] = cluster.mask(
                    start, min(_CHUNK, n - start))
        ids = np.flatnonzero(included)
        choices = np.empty(len(ids), dtype=np.int32)
        differences = np.empty(len(ids), dtype=np.int64)
        for start in range(0, len(ids), _CHUNK):
            stop = min(start + _CHUNK, len(ids))
            choices[start:stop], differences[start:stop] = self._difference_chunk(
                ids[start:stop], mode, cancelled)
        _cancel(cancelled)
        bins = np.floor_divide(differences, step_ticks)
        bins[choices < 0] = 0
        sort_choices = np.where(choices < 0, len(self.constraints), choices)
        order = np.lexsort((ids, bins, sort_choices))
        _cancel(cancelled)
        ids, choices, bins = ids[order], choices[order], bins[order]
        return DeltaGroups(self, step_ticks, mode, ids, choices, bins, cancelled,
                           auto_step=automatic)


def _read_status(db, ci, indices):
    if hasattr(db, "_status") and hasattr(db, "_dir_es"):
        # Persistent membership IDs can be uint32. Promote the rule's global
        # start before adding so packs exceeding 2**32 total rows cannot wrap.
        absolute = np.add(indices, db._dir_es[ci], dtype=np.int64)
        return db._status[absolute] == STATUS_WAIVED
    if hasattr(db, "get_status"):
        return np.fromiter((db.get_status(ci, int(ei)) == STATUS_WAIVED
                            for ei in indices), dtype=bool, count=len(indices))
    return np.zeros(len(indices), dtype=bool)


class DeltaGroups:
    """Numeric group directory with a bounded lazy descriptor cache."""

    def __init__(self, index, step_ticks, mode, ids, choices, bins, cancelled,
                 auto_step=False):
        self.index, self.step_ticks, self.mode = index, step_ticks, mode
        self.auto_step = bool(auto_step)
        self._ids = ids
        starts = (np.r_[0, np.flatnonzero((choices[1:] != choices[:-1]) |
                                         (bins[1:] != bins[:-1])) + 1]
                  if len(ids) else np.empty(0, dtype=np.int64))
        self._offsets = np.r_[starts, len(ids)].astype(np.int64)
        self._constraints = choices[starts]
        self._bins = bins[starts]
        self._counts = np.diff(self._offsets)
        self.total = len(ids)
        flags = index.estimated_flags
        self._estimated_counts = (np.add.reduceat(flags[ids], starts, dtype=np.int64)
                                  if flags is not None and len(starts) else
                                  np.zeros(len(starts), dtype=np.int64))
        self.estimated_total = int(self._estimated_counts.sum())
        n = len(index.measured_ticks)
        self._row_for_error = np.full(n, -1, dtype=np.int32)
        self._row_for_error[ids] = np.repeat(np.arange(len(starts), dtype=np.int32),
                                              self._counts)
        self._statuses = np.zeros(n, dtype=bool)
        self._waived = np.zeros(len(starts), dtype=np.int64)
        self._revision = 0
        self._descriptors = OrderedDict()
        self._visible_rows = {}
        self.reset_status(cancelled)

    def __len__(self):
        return len(self._counts)

    def __getitem__(self, row):
        row = int(row)
        if row < 0:
            row += len(self)
        if not 0 <= row < len(self):
            raise IndexError(row)
        reference = self._descriptors.get(row)
        group = reference() if reference is not None else None
        if group is None:
            group = DeltaGroup(self, row)
            self._descriptors[row] = weakref.ref(group)
            if len(self._descriptors) > 128:
                self._descriptors.popitem(last=False)
        self._descriptors.move_to_end(row)
        return group

    def _rows(self, waived):
        if waived is None:
            return None
        waived = bool(waived)
        if waived not in self._visible_rows:
            counts = self._waived if waived else self._counts - self._waived
            self._visible_rows[waived] = np.flatnonzero(counts)
        return self._visible_rows[waived]

    def group_count(self, waived=None):
        rows = self._rows(waived)
        return len(self) if rows is None else len(rows)

    def page(self, start, limit, waived=None):
        start, limit = max(0, int(start)), max(0, int(limit))
        rows = self._rows(waived)
        rows = (range(start, min(start + limit, len(self))) if rows is None
                else rows[start:start + limit])
        return [self[int(row)] for row in rows]

    def find(self, key):
        ci, bin_id = (int(v) for v in key)
        if ci < 0:
            return (len(self) - 1 if ci == _UNKNOWN and bin_id == 0
                    and len(self) and self._constraints[-1] < 0 else None)
        # Constraint groups are consecutive; unknown is deliberately last.
        known_end = len(self) - int(bool(len(self) and self._constraints[-1] < 0))
        lo = int(np.searchsorted(self._constraints[:known_end], ci, side="left"))
        hi = int(np.searchsorted(self._constraints[:known_end], ci, side="right"))
        row = int(lo) + int(np.searchsorted(self._bins[int(lo):hi], bin_id))
        return row if row < hi and int(self._bins[row]) == bin_id else None

    def reset_status(self, cancelled=None):
        """Refresh review counts after imported or externally replaced waives."""
        if getattr(self, "_persistent", False):
            # The GUI re-submits persistent groups to their process worker.
            # Direct callers can still refresh explicitly, but must discard
            # precomputed prefixes rather than return obsolete page ranks.
            self._page_prefix = self._page_offsets = None
        self._waived.fill(0)
        db, ci = self.index.db, self.index.ci
        for start in range(0, len(self._ids), _CHUNK):
            _cancel(cancelled)
            ids = self._ids[start:start + _CHUNK]
            status = _read_status(db, ci, ids)
            self._statuses[ids] = status
            rows = self._row_for_error[ids[status]]
            np.add.at(self._waived, rows, 1)
        self._revision += 1
        self._visible_rows.clear()

    def status_changed(self, eis):
        ids = np.unique(np.asarray(tuple(eis), dtype=np.int64))
        ids = ids[(ids >= 0) & (ids < len(self._row_for_error))]
        ids = ids[self._row_for_error[ids] >= 0]
        if not len(ids):
            return
        status = _read_status(self.index.db, self.index.ci, ids)
        changes = status.astype(np.int64) - self._statuses[ids]
        np.add.at(self._waived, self._row_for_error[ids], changes)
        if getattr(self, "_page_prefix", None) is not None:
            # Precomputed group page prefixes are only N/4096 entries.
            # Update affected suffixes without rereading a million members.
            rows = self._row_for_error[ids]
            for row in np.unique(rows[changes != 0]):
                match = (rows == row) & (changes != 0)
                members = self._ids[self._offsets[row]:self._offsets[row + 1]]
                chunks = np.searchsorted(members, ids[match]) // _PAGE_CHUNK
                lo, hi = self._page_offsets[row:row + 2]
                increments = np.zeros(int(hi - lo), dtype=np.int64)
                np.add.at(increments, chunks, changes[match])
                self._page_prefix[lo:hi] += np.cumsum(increments, dtype=np.int64)
        self._statuses[ids] = status
        self._revision += 1
        self._visible_rows.clear()


class DeltaGroup:
    """One group, exposing the existing cluster membership/page protocol."""

    def __init__(self, owner, row):
        self._owner, self._row = owner, row
        self.constraint_index = int(owner._constraints[row])
        bin_id = int(owner._bins[row])
        self.key = self.constraint_index, bin_id
        self.total = int(owner._counts[row])
        self.low_ticks = bin_id * owner.step_ticks
        self.high_ticks = self.low_ticks + owner.step_ticks
        self._prefix_revision = -1
        self._prefix = None
        if self.constraint_index < 0:
            self.metric = None
            self.unit = None
            self.name = ("Unmeasurable / ratio unavailable" if owner.mode == "percent"
                         else "Unmeasurable")
        else:
            con = owner.index.constraints[self.constraint_index]
            self.metric = con.get("metric")
            self.unit = "%" if owner.mode == "percent" else (
                "um2" if self.metric == "area" else "um")
            bound = owner.index.bound_ticks[self.constraint_index]
            self.name = "condition %d · %s %s %s: [%s, %s) %s" % (
                self.constraint_index + 1, self.metric,
                con.get("op", "?"), format_ticks(bound),
                format_ticks(self.low_ticks),
                format_ticks(self.high_ticks), self.unit)
        # A condition can produce several bins, and earlier conditions may
        # produce none. Number directory rows, including the unknown group,
        # independently of the condition provenance and stable membership key.
        self.name = "Group #%d · %s" % (row + 1, self.name)
        if self.estimated_count:
            self.name += " · estimated %d total" % self.estimated_count

    @property
    def estimated_count(self):
        return int(self._owner._estimated_counts[self._row])

    @property
    def _ids(self):
        owner, row = self._owner, self._row
        return owner._ids[owner._offsets[row]:owner._offsets[row + 1]]

    def contains(self, ei):
        ei = int(ei)
        rows = self._owner._row_for_error
        return 0 <= ei < len(rows) and int(rows[ei]) == self._row

    def mask(self, start, count):
        start, count = int(start), max(0, int(count))
        out = np.zeros(count, dtype=bool)
        lo, hi = max(0, start), min(start + count, len(self._owner._row_for_error))
        if hi > lo:
            out[lo - start:hi - start] = (
                self._owner._row_for_error[lo:hi] == self._row)
        return out

    def count(self, waived=None):
        if waived is None:
            return self.total
        n = int(self._owner._waived[self._row])
        return n if waived else self.total - n

    def status_counts(self):
        return self.count(True), self.total

    def _status_prefix(self, waived):
        owner = self._owner
        if self._prefix_revision != owner._revision:
            if getattr(owner, "_page_prefix", None) is not None:
                lo, hi = owner._page_offsets[self._row:self._row + 2]
                self._prefix = owner._page_prefix[lo:hi]
            else:
                ids = self._ids
                starts = np.arange(0, len(ids), _PAGE_CHUNK)
                counts = np.add.reduceat(owner._statuses[ids], starts,
                                         dtype=np.int64)
                self._prefix = np.cumsum(counts, dtype=np.int64)
            self._prefix_revision = owner._revision
        if waived:
            return self._prefix
        ends = np.minimum(np.arange(1, len(self._prefix) + 1) * _PAGE_CHUNK,
                          self.total)
        return ends - self._prefix

    def page(self, start, limit, waived=None):
        start, limit = max(0, int(start)), max(0, int(limit))
        ids = self._ids
        if waived is None:
            return ids[start:start + limit].tolist()
        if not limit or start >= self.count(waived):
            return []
        prefix = self._status_prefix(waived)
        chunk = int(np.searchsorted(prefix, start, side="right"))
        before = int(prefix[chunk - 1]) if chunk else 0
        out = []
        while chunk < len(prefix) and len(out) < limit:
            candidates = ids[chunk * _PAGE_CHUNK:(chunk + 1) * _PAGE_CHUNK]
            match = self._owner._statuses[candidates] == bool(waived)
            chosen = candidates[match]
            skip = max(0, start - before)
            out.extend(chosen[skip:skip + limit - len(out)].tolist())
            before = int(prefix[chunk])
            chunk = int(np.searchsorted(prefix, before, side="right"))
        return out

    def rank(self, ei, waived=None):
        ei = int(ei)
        if not self.contains(ei):
            return None
        ids = self._ids
        rank = int(np.searchsorted(ids, ei))
        if waived is None:
            return rank
        if bool(self._owner._statuses[ei]) != bool(waived):
            return None
        prefix = self._status_prefix(waived)
        chunk = rank // _PAGE_CHUNK
        before = int(prefix[chunk - 1]) if chunk else 0
        status = self._owner._statuses[ids[chunk * _PAGE_CHUNK:rank]]
        return before + int(np.count_nonzero(status == bool(waived)))
