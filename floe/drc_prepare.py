"""Prepare persistent DRC spatial and CD indices before interactive review."""

import argparse
import os
import sys
import time

from . import cachepath
from .drc import IcePack


def open_pack(source):
    source = os.path.abspath(source)
    direct = cachepath._is_pack(source)
    pack = source if direct else cachepath.find_pack(source)
    if pack is None:
        raise ValueError("build the DRC pack first: floe-index drc %s" % source)
    return IcePack(pack, src_path=None if direct else source,
                   verify_src=not direct, review=False)


def prepare(source, rules=None, selected=(), spatial=True, delta=True, report=print,
            *, backend="auto", jobs=None):
    """Prepare rules with parallel native CD; retain completed caches."""
    from .drc_delta import DeltaIndex
    from .drc_delta_cache import process_measure, prepare_group_cache
    from .drc_native import worker_count
    from .drc_spatial import prepare_rule
    from .svrf import load_rules
    if backend not in ("auto", "rust", "python"):
        raise ValueError("unknown DRC preprocessing backend %r" % backend)
    jobs = worker_count(jobs)
    if delta and not rules:
        raise ValueError("--svrf <rules.json> is required for CD/delta preprocessing; "
                         "use --spatial-only for geometry alone")
    metadata = load_rules(rules).get("checks", {}) if delta else {}
    db = open_pack(source)
    try:
        names = set(selected)
        missing = names - {check.name for check in db.checks}
        if missing:
            raise ValueError("unknown DRC rules: " + ", ".join(sorted(missing)))
        report("Analysis cache: " + cachepath.drc_analysis_dir(db.path))
        wanted = [ci for ci, check in enumerate(db.checks)
                  if not names or check.name in names]
        # Empty checks have no spatial or measurement work. In particular,
        # do not start measure/absolute/percent children for each empty rule.
        # The actual stored population is authoritative, not "declared".
        nonempty = [ci for ci in wanted if len(db.checks[ci].errors)]
        skipped = len(wanted) - len(nonempty)
        if skipped:
            report("Skipping %d empty rules" % skipped)
        wanted = nonempty
        for number, ci in enumerate(wanted, 1):
            check = db.checks[ci]
            report("[%d/%d] %s: %d errors" %
                   (number, len(wanted), check.name, len(check.errors)))
            last_report = [0.0]

            def spatial_progress(done, total):
                now = time.monotonic()
                if now - last_report[0] >= 1 or done == total:
                    report("  spatial %d / %d" % (done, total))
                    last_report[0] = now

            if spatial:
                prepare_rule(db, ci, progress=spatial_progress)
                report("  spatial ready")
            if delta:
                constraints = metadata.get(check.name, {}).get("constraints") or []
                if not constraints:
                    report("  CD skipped: no matching measurement constraints")
                    continue
                index = DeltaIndex(db, ci, constraints)
                process_measure(index, progress=lambda value: report("  " + value),
                                backend=backend, jobs=jobs)
                for mode in ("absolute", "percent"):
                    count = prepare_group_cache(index, mode=mode)
                    report("  %s: %d groups ready" % (mode, count))
                del index
        report("DRC analysis ready: %d rules prepared; %d empty rules skipped" %
               (len(wanted), skipped))
    finally:
        db.close()


def add_arguments(parser):
    parser.add_argument("db", help="DRC .db with a fresh pack, or the .tray pack itself")
    parser.add_argument("--svrf", metavar="RULES_JSON", help="SVRF .rules.json metadata")
    parser.add_argument("--rule", action="append", default=[], metavar="NAME",
                        help="prepare only this rule (repeatable); default: all rules")
    parser.add_argument("--jobs", type=int, metavar="N",
                        help="parallel Rust CD workers (default: min(4, CPU count), or FLOE_DRC_JOBS)")
    parser.add_argument("--backend", choices=("auto", "rust", "python"), default="auto",
                        help="CD measurement backend; auto uses compatible Rust when available")
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--spatial-only", action="store_true")
    group.add_argument("--delta-only", action="store_true")


def run(args):
    try:
        prepare(args.db, rules=args.svrf, selected=args.rule,
                spatial=not args.delta_only, delta=not args.spatial_only,
                backend=args.backend, jobs=args.jobs,
                report=lambda value: print(value, file=sys.stderr, flush=True))
    except KeyboardInterrupt:
        print("DRC preprocessing cancelled; completed rule caches are retained.", file=sys.stderr)
        return 130
    except (OSError, ValueError, RuntimeError) as exc:
        print("DRC preprocessing failed: %s" % exc, file=sys.stderr)
        return 1
    return 0


def main(argv=None, *, prog=None):
    parser = argparse.ArgumentParser(prog=prog, description=__doc__)
    add_arguments(parser)
    return run(parser.parse_args(argv))


if __name__ == "__main__":
    raise SystemExit(main())
