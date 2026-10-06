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


def prepare(source, rules=None, selected=(), spatial=True, delta=True, report=print):
    """Process one rule at a time; completed caches survive an interruption."""
    from .drc_delta import DeltaIndex
    from .drc_delta_cache import process_measure, process_group
    from .drc_spatial import prepare_rule
    from .svrf import load_rules
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
                process_measure(index, progress=lambda value: report("  " + value))
                for mode in ("absolute", "percent"):
                    groups = process_group(index, mode=mode,
                                           progress=lambda value: report("  " + value))
                    report("  %s: %d groups ready" % (mode, len(groups)))
                    del groups
                del index
        report("DRC analysis ready: %d rules" % len(wanted))
    finally:
        db.close()


def add_arguments(parser):
    parser.add_argument("db", help="DRC .db with a fresh pack, or the .tray pack itself")
    parser.add_argument("--svrf", metavar="RULES_JSON", help="SVRF .rules.json metadata")
    parser.add_argument("--rule", action="append", default=[], metavar="NAME",
                        help="prepare only this rule (repeatable); default: all rules")
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--spatial-only", action="store_true")
    group.add_argument("--delta-only", action="store_true")


def run(args):
    try:
        prepare(args.db, rules=args.svrf, selected=args.rule,
                spatial=not args.delta_only, delta=not args.spatial_only,
                report=lambda value: print(value, file=sys.stderr, flush=True))
    except KeyboardInterrupt:
        print("DRC preprocessing cancelled; completed rule caches are retained.", file=sys.stderr)
        return 130
    except (OSError, ValueError, RuntimeError) as exc:
        print("DRC preprocessing failed: %s" % exc, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    add_arguments(parser)
    raise SystemExit(run(parser.parse_args()))
