"""Cluster membership is applied before spatial-query decoding and caps.

Builds a small sparse rule and a >64-block dense rule so both query paths
are exercised against real packed geometry, including the review filter.

usage: python tools/validate_drc_cluster_spatial.py [floe-index-bin]
"""

import os
import subprocess
import sys
import tempfile

import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe import drc  # noqa: E402
from floe.drc_clusters import load_clusters  # noqa: E402


BIN = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(__file__), "..", "rust", "target", "release",
    "floe-index")


class Members:
    """Minimal range-mask protocol; no full cluster enumeration is needed."""

    def __init__(self, indices):
        self.indices = indices
        self.calls = []

    def mask(self, start, count):
        self.calls.append((start, count))
        out = np.zeros(count, dtype=bool)
        for ei in self.indices:
            if start <= ei < start + count:
                out[ei - start] = True
        return out


class TrackedPack(drc.IcePack):
    def __init__(self, path):
        self.decoded = []
        super().__init__(path)

    def _block(self, bi):
        self.decoded.append(bi)
        return super()._block(bi)


def main():
    with tempfile.TemporaryDirectory(prefix="floe-cluster-spatial-") as tmp:
        db = os.path.join(tmp, "results.db")
        pack = os.path.join(tmp, "results.tray")
        counts = (3 * drc._ICE2_BLOCK + 3, 67 * drc._ICE2_BLOCK + 3)
        with open(db, "w") as f:
            f.write("MAIN 1000\n")
            for ci, count in enumerate(counts):
                f.write("RULE.%d\n%d %d 0\n" % (ci, count, count))
                for ei in range(count):
                    f.write("p %d 1\n1000 1000\n" % (ei + 1))
        subprocess.run([BIN, "drc", db, pack], check=True,
                       capture_output=True, text=True)
        pk = TrackedPack(pack)
        decoded = pk.decoded
        rect = (0, 0, 2, 2)

        def query(**kwargs):
            decoded.clear()
            return [(ci, ei) for ci, ei, _e in
                    pk.query_rect(*rect, **kwargs)]

        try:
            for ci, count in enumerate(counts):
                selected = [count - 6, count - 3, count - 1]
                membership = Members(selected)
                assert query(checks=[ci], members={ci: membership}, cap=1) \
                    == [(ci, selected[0])], "membership applied after cap"
                want_block = int(pk._dir_bs[ci]) + selected[0] // drc._ICE2_BLOCK
                assert decoded == [want_block], "decoded non-member geometry"
                if ci == 0:
                    assert len(membership.calls) > 1, "sparse path not covered"
                    assert max(n for _, n in membership.calls) <= drc._ICE2_BLOCK
                else:
                    assert membership.calls == [(0, count)], "dense path not covered"

                assert query(checks=[ci], members={ci: membership}, cap=100) \
                    == [(ci, ei) for ei in selected]
                pk.set_status(ci, selected[-1], drc.STATUS_WAIVED)
                assert query(checks=[ci], members={ci: membership},
                             waived=True, cap=1) == [(ci, selected[-1])], \
                    "status and membership conjunction lost a late match"
                assert query(checks=[ci], members={ci: membership},
                             waived=False, cap=100) \
                    == [(ci, ei) for ei in selected[:-1]]
                assert query(checks=[ci], members={ci: Members([])}, cap=1) == []
                assert not decoded, "empty cluster decoded geometry"
                pk.set_status(ci, selected[-1], drc.STATUS_NONE)
                assert query(checks=[ci], members={}, cap=count + 1) \
                    == [(ci, ei) for ei in range(count)], "missing rule restricted"
                assert query(checks=[ci], cap=3) == [(ci, ei) for ei in range(3)], \
                    "legacy query changed"

            # A restriction on one rule must not spill into a later rule.
            assert query(members={0: Members([counts[0] - 1])}, cap=3) \
                == [(0, counts[0] - 1), (1, 0), (1, 1)]

            # Exercise the actual text-sidecar adapter against mmap status
            # and geometry, including local indices in the second rule.
            sidecar = db + ".clusters"
            with open(sidecar, "w") as f:
                for ci, count in enumerate(counts):
                    f.write("[RULE.%d]\nlate = %d,%d\n" %
                            (ci, count - 2, count))
            groups = load_clusters(sidecar, pk)
            for ci, count in enumerate(counts):
                group = groups.rules[ci][0]
                groups.prepare_counts(ci)
                assert group.status_counts() == (0, 2)
                assert query(checks=[ci], members={ci: group}, cap=1) \
                    == [(ci, count - 3)]
                pk.set_status(ci, count - 1, drc.STATUS_WAIVED)
                groups.status_changed(ci, [count - 1])
                groups.prepare_counts(ci)
                assert group.status_counts() == (1, 2)
                assert group.page(0, 10, True) == [count - 1]
                assert group.rank(count - 1, True) == 0
                assert query(checks=[ci], members={ci: group},
                             waived=True, cap=1) == [(ci, count - 1)]
            print("OK: sparse/dense cluster queries filter before decode/cap; "
                  "status conjunction and unrestricted rules preserved")
        finally:
            pk.close()


if __name__ == "__main__":
    main()
