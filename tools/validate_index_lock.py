#!/usr/bin/env python3
"""Index lock gate (rust/vfs/src/lock.rs, floe/indexlock.py; user
2026-10-09: "if the same file is being indexed or used, a run must be
refused - whoever runs it").

A layout's cache has two locks in `.floe-lock/` beside it: `<key>.build`
(every run that writes the cache, exclusive) and `<key>.use` (readers
shared, a whole rebuild exclusive until its commit marker); readers
register who they are. This gate holds every side of that against the
real binaries, the gate itself taking locks through floe/indexlock.py as
another user would (FLOE_REVIEWER=ws_kim_01):

  K1  the build lock held: floe-index vfs (whole, --occupancy-only,
      --coverage-only, --representatives-only, --frontier-only), hier,
      ovs and `floe2 index` end 75 within 3 s naming the holder verbatim;
      the cache's files byte for byte; the wrapper's temporary-file
      clean-up leaves another run's design.ovo.tmp
  K2  two real builds: the first held at `--hold-at locked`, the second
      ends 75 naming the first; the first completes and the cache opens
  K3  a whole rebuild in progress: renderd's open is `code=locked` with
      the text verbatim (underscores kept); `floe2 info` and the viewer's
      check say "being indexed" (not "no VFS cache"); a jobdeck with a
      busy source opens without it, its ledger saying who
  K4  renderd open: a whole rebuild (floe-index vfs, floe2 index --force)
      ends 75 naming the reader; after it quits the rebuild runs; a
      build held after its commit marker lets renderd open
  K5  renderd open: hier and --occupancy-only making a file that is not
      there go ahead; replacing design.ovh / design.ovo (a file readers
      map) ends 75 naming them, and a reader coming in while a replacement
      runs is refused; an addition while another holds the build lock ends
      75; a cache reached through a symbolic link (the cache or its folder)
      locks as itself; registrations stay 0644 under umask 077, and one
      this account cannot read still counts as a user
  K6  `floe2 index` on a cache being written says "being indexed", not
      "--force"; the jobdeck index counts a busy source and builds the
      other; the legacy <src>.floe rename waits for the locks
  K7  a killed reader's registration does not count; FLOE_LOCK=off runs
      as before and makes no .floe-lock; a read-only folder's cache opens
      unlocked; the lock folder takes the folder's write bits; probes
      never refuse a writer; a deck of more sources than the open-file
      limit leaves room for opens unlocked; the viewer's index dialog
      runs in its own process group (a cancel ends what it started)
  K8  a DRC results pack (.<db>.tray): a review holding it (drc.load_db)
      refuses a re-pack naming it, closed it packs; the pack's build lock
      held refuses `floe-index drc` and leaves the pack byte for byte, and
      opening it (`floe2 drc`, the viewer) says "being indexed"; a failed
      pack (an unreadable .db) leaves the published pack
  K9  a rule deck's sidecar: its build lock held refuses `floe-index
      svrf`, the rules file byte for byte
  K10 (2026-10-18) render / info --json / probe / clip on a source with no
      index index it first (stderr; stdout stays the command's), then use
      it; --no-index and FLOE_AUTO_INDEX=off refuse as before, making
      nothing; a changed source keeps its cache; a missing source says so,
      a build stopped half-way too (--no-index) or is rebuilt; a jobdeck's
      sources with none are indexed first, without auto-index its ledger
      says what to run
  K11 a busy source: refused at once (75, the holder named); --wait=1 ends
      75 after the bound, saying it waited; FLOE_LOCK_WAIT=on and
      --no-wait; render --wait, index --wait and info under FLOE_LOCK_WAIT
      wait for the holder and go on once it lets go, the cache built once;
      SIGINT ends a wait with 130 and nothing written; two renders at once
      on a fresh source build it once; index --force --wait waits for the
      reader that has it open
  K12 a deck render with a busy source: 3 with the holder in its ledger,
      --wait draws it whole once it is free; `floe-index vfs DECK.jb`
      indexes the deck's sources as `floe2 index DECK` (a deck's options),
      kept on a second run, no OUTDIR, --level, a busy source 75, --wait

usage: python tools/validate_index_lock.py
"""
import fcntl
import inspect
import json
import os
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

import klayout.db as db

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "tools" / "oracle"))  # floe_oracle (P3)
os.environ["FLOE_RENDERER"] = "rust"
BIN = ROOT / "rust" / "target" / "release" / "floe-index"
RENDERD = ROOT / "rust" / "target" / "release" / "floe-renderd"
# the product command line: the Rust `floe2` (rust/floe2; the Python
# floe2 CLI is gone - docs/SHARED_APP_LAYER.ko.md P1c)
FLOE2 = os.environ.get("FLOE2_BIN") or str(ROOT / "rust" / "target" / "release" / "floe2")
TMP = Path(tempfile.mkdtemp(prefix="floe-indexlock-"))
HOLDER = "ws_kim_01"   # an underscore name: the wire must not mangle it

os.environ["FLOE_INDEX_BIN"] = str(BIN)
os.environ["FLOE_RENDERD_BIN"] = str(RENDERD)
os.environ["FLOE_REVIEWER"] = HOLDER
os.environ.pop("FLOE_LOCK", None)
os.environ.pop("FLOE_LOCK_WHAT", None)

from floe_oracle import cachepath, indexlock  # noqa: E402


def run_env(**extra):
    env = dict(os.environ)
    env.update(extra)
    return env


def run(argv, ok=None, env=None, timeout=300):
    res = subprocess.run([str(a) for a in argv], capture_output=True,
                         text=True, cwd=str(ROOT), env=env or run_env(),
                         timeout=timeout, stdin=subprocess.DEVNULL)
    if ok is not None and res.returncode != ok:
        raise AssertionError("%s -> rc %d\n%s\n%s" % (
            " ".join(map(str, argv)), res.returncode, res.stdout,
            res.stderr))
    return res


def floe2(*args, ok=None, env=None):
    return run([FLOE2, *args], ok=ok, env=env)


def floe_index(*args, ok=None, env=None):
    return run([BIN, *args], ok=ok, env=env)


def lock_line(res):
    lines = [ln for ln in res.stderr.splitlines() if ln.startswith("[lock] ")]
    return lines[-1][7:] if lines else ""


def write_layout(path, n=4, layer=(1, 0)):
    ly = db.Layout(True)
    ly.dbu = 0.001
    li = ly.layer(db.LayerInfo(*layer))
    leaf = ly.create_cell("LEAF")
    leaf.shapes(li).insert(db.Box(0, 0, 400, 300))
    top = ly.create_cell("TOP")
    top.shapes(li).insert(db.Box(0, 0, 5000, 100))
    top.insert(db.CellInstArray(leaf.cell_index(), db.Trans(db.Vector(1000, 1000)),
                                db.Vector(800, 0), db.Vector(0, 600), n, 3))
    ly.write(str(path))


def snapshot(folder):
    out = {}
    for p in sorted(Path(folder).iterdir()):
        if p.is_file():
            out[p.name] = (p.stat().st_mtime_ns, p.read_bytes())
    return out


def open_worker(src):
    from floe_oracle.cache import Cache
    from floe_oracle.rust_render import RustRenderWorker
    c = Cache(str(src))
    c.load()
    w = RustRenderWorker(c)
    w.start()
    return w


DECK = """* lock.jb
MTITLE 1,A
*PLACE-INFO
CHIP A
$ (1, A, AD=0.00020, SF=1, TC=srcA.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=2000.0, UY=2000.0 )
ROWS 0.0/0.0
CHIP B
$ (1, A, AD=0.00020, SF=1, TC=srcB.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=2000.0, UY=2000.0 )
ROWS 0.0/3000.0
*END-PLACE
END
"""


# two levels: srcA in level 1, srcB in level 2 (K12's --level)
DECK_LEVELS = DECK.replace("MTITLE 1,A\n", "MTITLE 1,A\nMTITLE 2,B\n").replace(
    "$ (1, A, AD=0.00020, SF=1, TC=srcB.oas", "$ (2, B, AD=0.00020, SF=1, TC=srcB.oas")


class IndexLockTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = TMP / "chip"
        cls.dir.mkdir()
        cls.src = cls.dir / "chip.oas"
        write_layout(cls.src)
        floe2("index", cls.src, "--jobs", "2", ok=0)
        cls.cache = Path(cachepath.vfs_cache_dir(str(cls.src)))

    def setUp(self):
        os.environ["FLOE_LOCK_RETRY_MS"] = "300"

    def tearDown(self):
        os.environ.pop("FLOE_LOCK_RETRY_MS", None)

    def hold(self, full, target=None):
        lock = indexlock.try_writer(indexlock.VFS, str(target or self.cache),
                                    full=full, what="gate_holder run",
                                    create=True)
        self.assertIsNotNone(lock)
        self.addCleanup(lock.close)
        return lock

    def test_k1_the_build_lock_refuses_every_writer(self):
        before = snapshot(self.cache)
        ovo_tmp = self.cache / "design.ovo.tmp"
        ovo_tmp.write_bytes(b"another run's summary")
        self.addCleanup(lambda: ovo_tmp.unlink() if ovo_tmp.exists() else None)
        lock = self.hold(full=False)
        runs = [
            ("vfs", [BIN, "vfs", self.src, self.cache, "--jobs", "2"]),
            ("occupancy-only", [BIN, "vfs", self.src, self.cache, "--occupancy-only"]),
            ("coverage-only", [BIN, "vfs", self.src, self.cache, "--coverage-only"]),
            ("representatives-only", [BIN, "vfs", self.src, self.cache, "--representatives-only"]),
            ("frontier-only", [BIN, "vfs", self.src, self.cache, "--frontier-only"]),
            ("hier", [BIN, "hier", self.cache]),
            ("ovs", [BIN, "ovs", self.cache]),
            ("floe2 index --force", [FLOE2, "index", self.src, "--force"]),
            ("floe2 index --occupancy-only", [FLOE2, "index", self.src, "--occupancy-only"]),
        ]
        for name, argv in runs:
            started = time.monotonic()
            res = run(argv)
            took = time.monotonic() - started
            self.assertEqual(res.returncode, 75, (name, res.stdout, res.stderr))
            self.assertLess(took, 3.0, name)
            line = lock_line(res)
            self.assertIn("chip.oas is being indexed by %s" % HOLDER, line, name)
            self.assertIn("pid %d" % os.getpid(), line, name)
            self.assertIn("(gate_holder run)", line, name)
        # the wrapper's clean-up of a failed run leaves another run's file
        from floe_oracle import cli
        cli._discard_occupancy_tmp(str(self.cache))
        self.assertTrue(ovo_tmp.exists())
        lock.close()
        after = snapshot(self.cache)
        after.pop("design.ovo.tmp")
        self.assertEqual(after, before)
        # unlocked, the clean-up does its job
        cli._discard_occupancy_tmp(str(self.cache))
        self.assertFalse(ovo_tmp.exists())

    def test_k2_two_real_builds_one_wins(self):
        d = TMP / "race"
        d.mkdir()
        src = d / "race.oas"
        write_layout(src, n=6)
        flag = d / "go"
        first = subprocess.Popen(
            [str(BIN), "vfs", str(src), "--jobs", "2", "--hold-at", "locked:%s" % flag],
            stderr=subprocess.PIPE, stdout=subprocess.DEVNULL, text=True)
        self.addCleanup(lambda: first.poll() is None and first.kill())
        line = ""
        while "--hold-at locked" not in line:
            line = first.stderr.readline()
            self.assertTrue(line or first.poll() is None, "the first run ended early")
        second = floe_index("vfs", src, "--jobs", "2")
        self.assertEqual(second.returncode, 75, second.stderr)
        self.assertIn("race.oas is being indexed by", lock_line(second))
        self.assertIn("pid %d" % first.pid, lock_line(second))
        self.assertIn("(floe-index vfs)", lock_line(second))
        flag.write_text("go")
        first.stderr.read()
        first.stderr.close()
        self.assertEqual(first.wait(timeout=120), 0)
        floe_index("vfsd", cachepath.vfs_cache_dir(str(src)), ok=0)

    def test_k3_a_whole_rebuild_refuses_its_readers(self):
        self.hold(full=True)
        from floe_oracle.cache import Cache
        from floe_oracle.rust_render import RustRenderWorker
        c = Cache(str(self.src))
        c.load()
        w = RustRenderWorker(c)
        with self.assertRaises(RuntimeError) as ctx:
            w.start()
        text = str(ctx.exception)
        self.assertIn("chip.oas is being indexed by %s" % HOLDER, text)
        self.assertIn("(gate_holder run) - open it when the index is done", text)
        res = floe2("info", self.src)
        self.assertEqual(res.returncode, 75, res.stderr)
        self.assertIn("chip.oas is being indexed by %s" % HOLDER, lock_line(res))
        self.assertNotIn("no VFS cache", res.stderr)
        from floe import gui
        self.assertIn("being indexed by %s" % HOLDER,
                      str(gui.Viewer._index_busy(str(self.src))))

    def test_k3_a_deck_opens_without_its_busy_source(self):
        d = TMP / "deck"
        d.mkdir()
        for name in ("srcA.oas", "srcB.oas"):
            write_layout(d / name)
        (d / "lock.jb").write_text(DECK)
        floe2("index", d / "lock.jb", "--jobs", "2", ok=0)
        self.hold(full=True, target=cachepath.vfs_cache_dir(str(d / "srcB.oas")))
        from floe_oracle.jobdeck.viewer import DeckCache
        deck = DeckCache(str(d / "lock.jb"))
        deck.load()
        self.addCleanup(deck.close)
        busy = [r for r in deck.ledger if r["tc"] == "srcB.oas"]
        self.assertTrue(busy, deck.ledger)
        self.assertIn("srcB.oas is being indexed by %s" % HOLDER, busy[0]["detail"])
        self.assertFalse([r for r in deck.ledger if r["tc"] == "srcA.oas"])
        from floe_oracle.service import make_render_worker
        w = make_render_worker(deck)
        w.start()
        w.stop()

    def test_k4_a_reader_refuses_a_whole_rebuild(self):
        w = open_worker(self.src)
        try:
            users = indexlock.state(indexlock.VFS, str(self.cache)).users
            self.assertEqual(len(users), 1)
            for argv in ([BIN, "vfs", self.src, "--jobs", "2"],
                         [FLOE2, "index", self.src, "--force"]):
                res = run(argv)
                self.assertEqual(res.returncode, 75, res.stderr)
                line = lock_line(res)
                self.assertIn("chip.oas is in use by %s" % HOLDER, line)
                self.assertIn("(floe2 view)" if False else "pid %d" % users[0].pid, line)
        finally:
            w.stop()
        self.assertEqual(indexlock.state(indexlock.VFS, str(self.cache)).users, [])
        floe_index("vfs", self.src, "--jobs", "2", ok=0)
        # after its commit marker a whole build lets readers in (its ovs
        # step is additive)
        flag = TMP / "committed.go"
        build = subprocess.Popen(
            [str(BIN), "vfs", str(self.src), "--jobs", "2", "--hold-at", "committed:%s" % flag],
            stderr=subprocess.PIPE, stdout=subprocess.DEVNULL, text=True)
        self.addCleanup(lambda: build.poll() is None and build.kill())
        line = ""
        while "--hold-at committed" not in line:
            line = build.stderr.readline()
            self.assertTrue(line or build.poll() is None, "the build ended early")
        st = indexlock.state(indexlock.VFS, str(self.cache))
        self.assertTrue(st.writing and not st.building)
        w = open_worker(self.src)
        w.stop()
        flag.write_text("go")
        build.stderr.read()
        build.stderr.close()
        self.assertEqual(build.wait(timeout=120), 0)

    def test_k5_additions_beside_readers_replacements_without_them(self):
        """A new file goes in beside the readers; replacing one they map
        (design.ovh/.ovo) holds them all out - any host, the kernel lock
        decides (review 2026-10-09 of 9378c6d7, P1: a registration checked
        once at the start let a reader on another host in mid-run)."""
        floe2("index", self.src, "--occupancy-only", ok=0)   # design.ovo there
        w = open_worker(self.src)
        try:
            for argv, made in (([BIN, "hier", self.cache], "design.ovh"),
                               ([BIN, "vfs", self.src, self.cache, "--occupancy-only"], "design.ovo")):
                self.assertTrue((self.cache / made).is_file(), made)
                res = run(argv)
                self.assertEqual(res.returncode, 75, (made, res.stderr))
                self.assertIn("chip.oas is in use by %s" % HOLDER, lock_line(res))
                # not there (nobody maps it): made beside the reader
                (self.cache / made).unlink()
                run(argv, ok=0)
                self.assertTrue((self.cache / made).is_file(), made)
            lock = self.hold(full=False)
            res = floe_index("hier", self.cache)
            self.assertEqual(res.returncode, 75)
            self.assertIn("being indexed by %s" % HOLDER, lock_line(res))
            lock.close()
        finally:
            w.stop()
        # a replacement under way holds a reader coming in out: renderd's
        # open and a Python reader are refused, "being indexed"
        flag = TMP / "replace.go"
        held = subprocess.Popen(
            [str(BIN), "vfs", str(self.src), str(self.cache), "--occupancy-only",
             "--hold-at", "locked:%s" % flag],
            stderr=subprocess.PIPE, stdout=subprocess.DEVNULL, text=True)
        self.addCleanup(lambda: held.poll() is None and held.kill())
        line = ""
        while "--hold-at locked" not in line:
            line = held.stderr.readline()
            self.assertTrue(line or held.poll() is None, "the run ended early")
        from floe_oracle.cache import Cache
        from floe_oracle.rust_render import RustRenderWorker
        c = Cache(str(self.src))
        c.load()
        late = RustRenderWorker(c)
        with self.assertRaises(RuntimeError) as ctx:
            late.start()
        self.assertIn("chip.oas is being indexed by %s" % HOLDER, str(ctx.exception))
        self.assertIn("(floe-index vfs --occupancy-only)", str(ctx.exception))
        with self.assertRaises(indexlock.Busy):
            indexlock.hold_readers(indexlock.VFS, [str(self.cache)], "late reader")
        flag.write_text("go")
        held.stderr.read()
        held.stderr.close()
        self.assertEqual(held.wait(timeout=120), 0)

    def test_k5_a_cache_through_a_link_locks_as_itself(self):
        """review 2026-10-09 of 9378c6d7, P1: a rebuild through a symbolic
        link to a cache a viewer has open took another key and ran."""
        link = self.dir / "linked.cache"
        link.symlink_to(self.cache.name)
        self.addCleanup(link.unlink)
        self.assertEqual(indexlock.key(indexlock.VFS, str(link)),
                         indexlock.key(indexlock.VFS, str(self.cache)))
        w = open_worker(self.src)
        try:
            for argv in ([BIN, "vfs", self.src, link, "--jobs", "2"],
                         [BIN, "hier", link]):
                res = run(argv)
                self.assertEqual(res.returncode, 75, (argv, res.stderr))
                self.assertIn("chip.oas is in use by %s" % HOLDER, lock_line(res))
        finally:
            w.stop()
        # a folder reached through a link too
        folder_link = TMP / "chip.link"
        folder_link.symlink_to(self.dir)
        self.addCleanup(folder_link.unlink)
        self.assertEqual(indexlock.key(indexlock.VFS, str(folder_link / self.cache.name)),
                         indexlock.key(indexlock.VFS, str(self.cache)))

    def test_k5_registrations_any_account_can_read(self):
        """review 2026-10-09 of 9378c6d7, P2: under umask 077 a reader's
        registration was 0600, and one another account cannot read was
        taken for no reader."""
        before = os.umask(0o077)
        try:
            w = open_worker(self.src)
            py = indexlock.hold_readers(indexlock.VFS, [str(self.cache)], "python reader")
        finally:
            os.umask(before)
        k = indexlock.key(indexlock.VFS, str(self.cache))
        try:
            regs = [p for p in os.listdir(k.dir) if p.startswith("use.") and not p.endswith(".tmp")]
            self.assertEqual(len(regs), 2, regs)
            for p in regs:
                self.assertEqual(stat.S_IMODE(os.stat(os.path.join(k.dir, p)).st_mode), 0o644, p)
            hidden = Path(k.dir) / "use.other.host.4242.3"
            hidden.write_text(indexlock.Holder("lee", "", "other.host", 4242, "x", "floe2 view").text([k.name]))
            os.chmod(hidden, 0)
            self.addCleanup(lambda: hidden.exists() and (os.chmod(hidden, 0o644), hidden.unlink()))
            users = indexlock.state(indexlock.VFS, str(self.cache)).users
            self.assertIn(("someone", "other.host", 4242), [(u.who, u.host, u.pid) for u in users])
            res = floe_index("vfs", self.src, "--jobs", "2")
            self.assertEqual(res.returncode, 75, res.stderr)
            self.assertIn("someone on other.host, pid 4242", lock_line(res))
        finally:
            py.close()
            w.stop()

    def test_k6_the_wrapper_and_the_deck_say_busy(self):
        self.hold(full=False)
        res = floe2("index", self.src)
        self.assertEqual(res.returncode, 75, res.stderr)
        self.assertIn("being indexed by %s" % HOLDER, lock_line(res))
        self.assertNotIn("--force", res.stderr)

    def test_k6_the_deck_index_counts_a_busy_source(self):
        d = TMP / "deckidx"
        d.mkdir()
        for name in ("srcA.oas", "srcB.oas"):
            write_layout(d / name)
        (d / "lock.jb").write_text(DECK)
        self.hold(full=True, target=cachepath.vfs_cache_dir(str(d / "srcB.oas")))
        res = floe2("index", d / "lock.jb", "--jobs", "2")
        self.assertEqual(res.returncode, 75, res.stdout + res.stderr)
        self.assertIn("BUSY srcB.oas", res.stdout)
        self.assertIn("index     : 1 built, 0 failed, 1 busy, 0 kept", res.stdout)
        self.assertTrue((Path(cachepath.vfs_cache_dir(str(d / "srcA.oas"))) / "design.ovm").is_file())

    def test_k6_the_legacy_rename_waits_for_the_locks(self):
        d = TMP / "legacy"
        d.mkdir()
        src = d / "old.oas"
        write_layout(src)
        floe2("index", src, "--jobs", "2", ok=0)
        new = Path(cachepath.vfs_cache_dir(str(src)))
        old = Path(cachepath.legacy_vfs_cache_dir(str(src)))
        new.rename(old)
        lock = self.hold(full=False, target=str(new))
        self.assertEqual(cachepath.find_vfs_cache(str(src)), str(old))
        self.assertTrue(old.is_dir() and not new.exists())
        lock.close()
        self.assertEqual(cachepath.find_vfs_cache(str(src)), str(new))
        self.assertTrue(new.is_dir() and not old.exists())

    def test_k7_a_killed_readers_registration_does_not_count(self):
        w = open_worker(self.src)
        pid = w._proc.pid
        k = indexlock.key(indexlock.VFS, str(self.cache))
        regs = [p for p in os.listdir(k.dir) if p.startswith("use.") and p.endswith(".%d.0" % pid)]
        self.assertEqual(len(regs), 1, os.listdir(k.dir))
        os.kill(pid, signal.SIGKILL)
        w._proc.wait()
        w.stop()
        self.assertTrue((Path(k.dir) / regs[0]).exists())
        floe_index("vfs", self.src, "--jobs", "2", ok=0)
        self.assertEqual(indexlock.state(indexlock.VFS, str(self.cache)).users, [])
        self.assertFalse((Path(k.dir) / regs[0]).exists())

    def test_k7_the_kill_switch(self):
        d = TMP / "off"
        d.mkdir()
        src = d / "off.oas"
        write_layout(src)
        floe2("index", src, "--jobs", "2", ok=0, env=run_env(FLOE_LOCK="off"))
        self.assertFalse((d / ".floe-lock").exists())
        floe2("index", src, "--jobs", "2", ok=0)
        self.hold(full=True, target=cachepath.vfs_cache_dir(str(src)))
        floe_index("vfs", src, "--jobs", "2", ok=0, env=run_env(FLOE_LOCK="off"))

    def test_k7_a_read_only_folder_opens_unlocked(self):
        d = TMP / "ro"
        d.mkdir()
        src = d / "ro.oas"
        write_layout(src)
        floe2("index", src, "--jobs", "2", ok=0, env=run_env(FLOE_LOCK="off"))
        os.chmod(d, 0o555)
        self.addCleanup(os.chmod, d, 0o755)
        w = open_worker(src)
        w.stop()
        self.assertFalse((d / ".floe-lock").exists())

    def test_k7_the_lock_folder_takes_the_folders_bits(self):
        for mode, want_dir, want_file in ((0o775, 0o1775, 0o664), (0o755, 0o755, 0o644)):
            d = TMP / ("mode%o" % mode)
            d.mkdir()
            os.chmod(d, mode)
            src = d / "m.oas"
            write_layout(src)
            floe2("index", src, "--jobs", "2", ok=0)
            lock_dir = d / ".floe-lock"
            self.assertEqual(stat.S_IMODE(lock_dir.stat().st_mode), want_dir, oct(mode))
            for name in ("m.oas.vfs.build", "m.oas.vfs.use"):
                self.assertEqual(stat.S_IMODE((lock_dir / name).stat().st_mode), want_file, (oct(mode), name))

    def test_k7_probes_never_refuse_a_writer(self):
        stop = threading.Event()

        def probe():
            while not stop.is_set():
                indexlock.state(indexlock.VFS, str(self.cache))

        threads = [threading.Thread(target=probe) for _ in range(3)]
        for t in threads:
            t.start()
        try:
            for _ in range(3):
                floe_index("vfs", self.src, "--jobs", "2", ok=0)
        finally:
            stop.set()
            for t in threads:
                t.join()

    def test_k7_a_deck_past_the_file_limit_opens(self):
        d = TMP / "many"
        d.mkdir()
        n = 24
        rows = []
        for i in range(n):
            write_layout(d / ("s%02d.oas" % i))
            rows.append("CHIP C%02d\n$ (1, A, AD=0.00020, SF=1, TC=s%02d.oas, LY={1}, DT={0}, "
                        "BX=0.0, BY=0.0, UX=2000.0, UY=2000.0 )\nROWS 0.0/%d.0\n" % (i, i, i * 3000))
        (d / "many.jb").write_text("* many.jb\nMTITLE 1,A\n*PLACE-INFO\n%s*END-PLACE\nEND\n" % "".join(rows))
        floe2("index", d / "many.jb", "--jobs", "2", ok=0)
        res = run(["/bin/sh", "-c", 'ulimit -n 32 && exec "$0" render "$1" --px 200 --out "$2"',
                   FLOE2, d / "many.jb", d / "many.png"])
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)
        self.assertIn("without their locks", res.stderr)

    def test_k7_the_index_dialog_owns_its_process_group(self):
        from floe import gui
        source = inspect.getsource(gui.Viewer._index_modal)
        self.assertIn("start_new_session=True", source)
        self.assertIn("os.killpg(proc.pid, signal.SIGTERM)", source)


def spawn(argv, log, env=None):
    """A command in the background, its stderr in `log`."""
    with open(log, "w") as err:
        return subprocess.Popen([str(a) for a in argv], stdout=subprocess.PIPE,
                                stderr=err, text=True, cwd=str(ROOT),
                                env=env or run_env(), stdin=subprocess.DEVNULL)


def until_said(log, text, proc, deadline_s=60):
    """`log` says `text` (the background command still running)."""
    deadline = time.monotonic() + deadline_s
    while time.monotonic() < deadline:
        if text in Path(log).read_text():
            return
        if proc.poll() is not None:
            raise AssertionError("ended (%d) before saying %r:\n%s" % (
                proc.returncode, text, Path(log).read_text()))
        time.sleep(0.05)
    raise AssertionError("did not say %r in %d s:\n%s" % (
        text, deadline_s, Path(log).read_text()))


class ReadyTests(unittest.TestCase):
    """K10-K12 (user 2026-10-18: "자동 인덱싱하도록 하고 대기 옵션도 넣어줘.
    그리고 jobdeck 인덱싱도 floe-index로 할 수 있게"; app-cli ready.rs)."""

    def setUp(self):
        os.environ["FLOE_LOCK_RETRY_MS"] = "300"
        self.dir = Path(tempfile.mkdtemp(prefix="ready", dir=TMP))

    def tearDown(self):
        os.environ.pop("FLOE_LOCK_RETRY_MS", None)

    def layout(self, name="c.oas"):
        src = self.dir / name
        write_layout(src)
        return src

    def deck(self):
        for name in ("srcA.oas", "srcB.oas"):
            write_layout(self.dir / name)
        (self.dir / "lock.jb").write_text(DECK)
        return self.dir / "lock.jb"

    def hold(self, src, full=True):
        lock = indexlock.try_writer(indexlock.VFS, cachepath.vfs_cache_dir(str(src)),
                                    full=full, what="gate_holder run", create=True)
        self.assertIsNotNone(lock)
        self.addCleanup(lock.close)
        return lock

    def cache(self, src):
        return Path(cachepath.vfs_cache_dir(str(src)))

    def test_k10_a_read_command_indexes_a_source_with_none_first(self):
        # render, info --json, probe, clip: each on a source with no index
        src = self.layout()
        res = floe2("render", src, "--px", "120", "--out", self.dir / "r.png", ok=0)
        self.assertIn("c.oas has no index yet - indexing it first", res.stderr)
        self.assertTrue((self.cache(src) / "meta.json").is_file())
        self.assertTrue((self.dir / "r.png").read_bytes().startswith(b"\x89PNG"))
        self.assertIn("rendered", res.stdout)
        # an index there is used: no second build
        again = floe2("render", src, "--px", "120", "--out", self.dir / "r2.png", ok=0)
        self.assertNotIn("indexing it first", again.stderr)
        js = self.layout("j.oas")
        res = floe2("info", js, "--json", ok=0)
        self.assertEqual(json.loads(res.stdout)["source"], str(js))
        self.assertIn("j.oas has no index yet", res.stderr)
        floe2("probe", self.layout("p.oas"), ok=0)
        cl = self.layout("k.oas")
        floe2("clip", cl, "--bbox", "0,0,3,3", "--out", self.dir / "k_clip.oas", ok=0)
        self.assertTrue((self.dir / "k_clip.oas").stat().st_size > 0)
        # --no-index / FLOE_AUTO_INDEX=off: refused as before, nothing made
        for name, args, env in (("n1.oas", ("--no-index",), None),
                                ("n2.oas", (), run_env(FLOE_AUTO_INDEX="off"))):
            n = self.layout(name)
            res = floe2("render", n, *args, "--out", self.dir / "n.png", env=env)
            self.assertEqual(res.returncode, 1, res.stderr)
            self.assertIn("no real VFS cache directory for %s; run floe2 index %s first"
                          % (name, n), res.stderr)
            self.assertFalse(self.cache(n).exists())
        # a changed source keeps its cache: drawn with the warning
        before = snapshot(self.cache(src))
        os.utime(src, (time.time() + 5, time.time() + 5))
        res = floe2("render", src, "--px", "120", "--out", self.dir / "s.png", ok=0)
        self.assertIn("source changed", res.stderr)
        self.assertEqual(snapshot(self.cache(src)), before)
        # a source not there, a build stopped half-way: said so
        res = floe2("render", self.dir / "none.oas", "--out", self.dir / "x.png")
        self.assertEqual(res.returncode, 1)
        self.assertIn("source not found: %s" % (self.dir / "none.oas"), res.stderr)
        half = self.layout("h.oas")
        self.cache(half).mkdir()
        res = floe2("render", half, "--no-index", "--out", self.dir / "h.png")
        self.assertEqual(res.returncode, 1)
        self.assertIn("the index of h.oas is incomplete", res.stderr)
        self.assertIn("floe2 index --force %s" % half, res.stderr)
        # ... which the index first rebuilds (a Ctrl-C on it left the folder)
        res = floe2("render", half, "--px", "120", "--out", self.dir / "h.png", ok=0)
        self.assertIn("h.oas has an incomplete index (a build stopped half-way) - rebuilding it first",
                      res.stderr)
        self.assertTrue((self.cache(half) / "meta.json").is_file())
        # a jobdeck: its sources with none first, then drawn whole
        deck = self.deck()
        res = floe2("render", deck, "--px", "160", "--out", self.dir / "d.png", ok=0)
        self.assertIn("[jobdeck] 2 source(s) have no index yet - indexing them first", res.stderr)
        self.assertIn("index     : 2 built, 0 failed, 0 kept", res.stderr)
        self.assertNotIn("skipped", res.stdout)
        res = floe2("info", deck, "--json", ok=0)
        json.loads(res.stdout)

    def test_k10_a_deck_without_auto_index_says_what_to_run(self):
        deck = self.deck()
        res = floe2("render", deck, "--no-index", "--out", self.dir / "d.png")
        self.assertEqual(res.returncode, 1, res.stderr)
        self.assertIn("no placement can be drawn", res.stderr)
        floe2("index", self.dir / "srcA.oas", ok=0)
        res = floe2("render", deck, "--no-index", "--out", self.dir / "d.png")
        self.assertEqual(res.returncode, 3, res.stdout + res.stderr)
        self.assertIn("srcB.oas: not_indexed (no fresh index cache (run: floe2 index lock.jb))",
                      res.stdout)

    def test_k11_a_busy_source_is_refused_or_waited_for(self):
        src = self.layout()
        lock = self.hold(src)
        # no --wait: refused at once, naming the holder
        started = time.monotonic()
        res = floe2("render", src, "--out", self.dir / "a.png")
        self.assertEqual(res.returncode, 75, res.stderr)
        self.assertLess(time.monotonic() - started, 3.0)
        self.assertIn("c.oas is being indexed by %s" % HOLDER, lock_line(res))
        self.assertIn("(gate_holder run)", lock_line(res))
        # bounded: refused after the bound, saying it waited
        started = time.monotonic()
        res = floe2("render", src, "--wait=1", "--out", self.dir / "a.png")
        self.assertEqual(res.returncode, 75, res.stderr)
        self.assertGreaterEqual(time.monotonic() - started, 1.0)
        self.assertIn("waiting for it to finish (--wait, up to 1 s)", res.stderr)
        self.assertIn("(waited 0:01, the --wait bound)", lock_line(res))
        self.assertEqual(res.stderr.count("waiting for it to finish"), 1, res.stderr)
        # FLOE_LOCK_WAIT=on the default, --no-wait over it
        res = floe2("index", src, "--no-wait", env=run_env(FLOE_LOCK_WAIT="on"))
        self.assertEqual(res.returncode, 75, res.stderr)
        # --wait: the render, the index, the info wait; released, each goes
        # on - the cache built once
        logs = [self.dir / ("w%d.log" % i) for i in range(3)]
        procs = [spawn([FLOE2, "render", src, "--wait", "--px", "120", "--out", self.dir / "w.png"], logs[0]),
                 spawn([FLOE2, "index", src, "--wait", "--jobs", "2"], logs[1]),
                 spawn([FLOE2, "info", src], logs[2], env=run_env(FLOE_LOCK_WAIT="30"))]
        for proc, log in zip(procs, logs):
            until_said(log, "waiting for it to finish", proc)
        time.sleep(1.0)
        self.assertTrue(all(p.poll() is None for p in procs))
        lock.close()
        for proc, log in zip(procs, logs):
            self.assertEqual(proc.wait(timeout=120), 0, Path(log).read_text())
            proc.stdout.close()
            self.assertIn("c.oas is free - going on", Path(log).read_text())
        builds = sum(Path(log).read_text().count("[vfs] done in") for log in logs)
        self.assertEqual(builds, 1, [Path(log).read_text() for log in logs])
        self.assertTrue((self.dir / "w.png").is_file())
        # a signal ends the wait as it ends the command: nothing written
        busy = self.layout("s.oas")
        self.hold(busy)
        log = self.dir / "sig.log"
        proc = spawn([FLOE2, "render", busy, "--wait", "--out", self.dir / "sig.png"], log)
        until_said(log, "waiting for it to finish", proc)
        proc.send_signal(signal.SIGINT)
        self.assertEqual(proc.wait(timeout=30), 130, Path(log).read_text())
        proc.stdout.close()
        self.assertFalse((self.dir / "sig.png").exists())
        self.assertFalse(self.cache(busy).joinpath("meta.json").exists())

    def test_k11_two_reads_at_once_build_once(self):
        src = self.layout()
        logs = [self.dir / ("t%d.log" % i) for i in range(2)]
        procs = [spawn([FLOE2, "render", src, "--wait", "--px", "120", "--out", self.dir / ("t%d.png" % i)], logs[i])
                 for i in range(2)]
        for proc, log in zip(procs, logs):
            self.assertEqual(proc.wait(timeout=120), 0, Path(log).read_text())
            proc.stdout.close()
        self.assertEqual(sum(Path(log).read_text().count("[vfs] done in") for log in logs), 1)

    def test_k11_a_rebuild_waits_for_its_readers(self):
        src = self.layout()
        floe2("index", src, "--jobs", "2", ok=0)
        w = open_worker(src)
        stopped = []
        self.addCleanup(lambda: stopped or w.stop())
        res = floe2("index", src, "--force", "--wait=1", "--jobs", "2")
        self.assertEqual(res.returncode, 75, res.stderr)
        self.assertIn("c.oas is in use by %s" % HOLDER, res.stderr)
        self.assertIn("waiting for it to be closed there (--wait, up to 1 s)", res.stderr)
        log = self.dir / "rebuild.log"
        proc = spawn([FLOE2, "index", src, "--force", "--wait", "--jobs", "2"], log)
        until_said(log, "waiting for it to be closed there", proc)
        w.stop()
        stopped.append(1)
        self.assertEqual(proc.wait(timeout=120), 0, log.read_text())
        proc.stdout.close()
        self.assertIn("[vfs] done in", log.read_text())

    def test_k12_a_deck_render_waits_for_its_busy_source(self):
        deck = self.deck()
        floe2("index", self.dir / "srcA.oas", ok=0)
        lock = self.hold(self.dir / "srcB.oas")
        res = floe2("render", deck, "--px", "160", "--out", self.dir / "d.png")
        self.assertEqual(res.returncode, 3, res.stdout + res.stderr)
        self.assertIn("srcB.oas: not_indexed (srcB.oas is being indexed by %s" % HOLDER, res.stdout)
        log = self.dir / "deck.log"
        proc = spawn([FLOE2, "render", deck, "--px", "160", "--wait", "--out", self.dir / "w.png"], log)
        until_said(log, "srcB.oas is being indexed by %s" % HOLDER, proc)
        time.sleep(0.5)
        self.assertIsNone(proc.poll())
        lock.close()
        self.assertEqual(proc.wait(timeout=120), 0, log.read_text())
        out = proc.stdout.read()
        proc.stdout.close()
        self.assertNotIn("skipped", out)
        # its index was the waiter's to build (the holder built nothing)
        self.assertIn("index     : 1 built, 0 failed, 0 kept", log.read_text())

    def test_k12_floe_index_indexes_a_jobdeck(self):
        deck = self.deck()
        res = floe_index("vfs", deck, "--jobs", "2", ok=0)
        self.assertIn("lock.jb is a jobdeck: its sources are indexed by", res.stderr)
        self.assertIn("index     : 2 built, 0 failed, 0 kept", res.stdout)
        for name in ("srcA.oas", "srcB.oas"):
            self.assertTrue((self.cache(self.dir / name) / "meta.json").is_file())
            # a deck's source: the occupancy summary, no design.ovs
            self.assertTrue((self.cache(self.dir / name) / "design.ovo").is_file())
            self.assertFalse((self.cache(self.dir / name) / "design.ovs").exists())
        res = floe_index("vfs", deck, ok=0)
        self.assertIn("index     : 0 built, 0 failed, 2 kept", res.stdout)
        res = floe_index("vfs", deck, self.dir / "out")
        self.assertEqual(res.returncode, 2, res.stderr)
        self.assertIn("no OUTDIR", res.stderr)
        floe2("render", deck, "--no-index", "--px", "160", "--out", self.dir / "d.png", ok=0)
        # its levels, a busy source (75), --wait for it
        d2 = Path(tempfile.mkdtemp(prefix="deck2", dir=TMP))
        for name in ("srcA.oas", "srcB.oas"):
            write_layout(d2 / name)
        (d2 / "lock.jb").write_text(DECK_LEVELS)
        res = floe_index("vfs", d2 / "lock.jb", "--level", "1", ok=0)
        self.assertIn("index     : 1 built, 0 failed, 0 kept", res.stdout)
        self.assertFalse(self.cache(d2 / "srcB.oas").exists())
        lock = indexlock.try_writer(indexlock.VFS, cachepath.vfs_cache_dir(str(d2 / "srcB.oas")),
                                    full=True, what="gate_holder run", create=True)
        self.addCleanup(lock.close)
        res = floe_index("vfs", d2 / "lock.jb")
        self.assertEqual(res.returncode, 75, res.stdout + res.stderr)
        self.assertIn("index     : 0 built, 0 failed, 1 busy, 1 kept", res.stdout)
        log = d2 / "w.log"
        proc = spawn([BIN, "vfs", d2 / "lock.jb", "--wait"], log)
        until_said(log, "waiting for it to finish", proc)
        lock.close()
        self.assertEqual(proc.wait(timeout=120), 0, log.read_text())
        out = proc.stdout.read()
        proc.stdout.close()
        self.assertIn("index     : 1 built, 0 failed, 1 kept", out)


class PackLockTests(unittest.TestCase):
    """K8/K9: the DRC results pack and the rule deck's sidecar."""

    @classmethod
    def setUpClass(cls):
        cls.dir = TMP / "drc"
        cls.dir.mkdir()
        cls.db = cls.dir / "run.db"
        run([sys.executable, ROOT / "tools" / "gen_drcdb.py", cls.db,
             "--checks", "6", "--max-errors", "20", "--seed", "3"], ok=0)
        floe_index("drc", cls.db, ok=0)
        cls.pack = Path(cachepath.pack_path(str(cls.db)))

    def setUp(self):
        os.environ["FLOE_LOCK_RETRY_MS"] = "300"

    def tearDown(self):
        os.environ.pop("FLOE_LOCK_RETRY_MS", None)

    def test_k8_a_review_holding_the_pack_refuses_a_repack(self):
        from floe_oracle import drc
        review = drc.load_db(str(self.db))
        self.assertIsInstance(review, drc.IcePack)
        try:
            res = floe_index("drc", self.db)
            self.assertEqual(res.returncode, 75, res.stderr)
            line = lock_line(res)
            self.assertIn("run.db is in use by %s" % HOLDER, line)
            self.assertIn("pid %d" % os.getpid(), line)
        finally:
            review.close()
        floe_index("drc", self.db, ok=0)

    def test_k8_the_pack_build_lock_refuses_drc_and_its_readers(self):
        from floe_oracle import drc
        before = self.pack.read_bytes()
        lock = indexlock.try_writer(indexlock.PACK, str(self.pack), full=True,
                                    what="gate_holder pack", create=True)
        self.addCleanup(lock.close)
        res = floe_index("drc", self.db)
        self.assertEqual(res.returncode, 75, res.stderr)
        self.assertIn("run.db is being indexed by %s" % HOLDER, lock_line(res))
        self.assertEqual(self.pack.read_bytes(), before, "the published pack is kept")
        with self.assertRaises(indexlock.Busy) as ctx:
            drc.load_db(str(self.db))
        self.assertIn("open it when the index is done", str(ctx.exception))
        res = floe2("drc", self.db)
        self.assertEqual(res.returncode, 75, res.stderr)
        self.assertIn("run.db is being indexed by %s" % HOLDER, lock_line(res))

    def test_k8_a_failed_pack_keeps_the_published_one(self):
        before = self.pack.read_bytes()
        os.chmod(self.db, 0)
        self.addCleanup(os.chmod, self.db, 0o644)
        res = floe_index("drc", self.db)
        self.assertEqual(res.returncode, 1, res.stderr)
        self.assertEqual(self.pack.read_bytes(), before)
        self.assertFalse([p for p in os.listdir(self.dir) if ".tray.tmp" in p])

    def test_k9_the_rules_sidecars_build_lock_refuses_svrf(self):
        deck = self.dir / "deck.cal"
        deck.write_text("LAYER M1 31\nM1.W.1 { @ min width\n  INT M1 < 0.1 ABUT<90 SINGULAR REGION\n}\n")
        floe_index("svrf", deck, ok=0)
        rules = Path(str(deck) + ".rules.json")
        before = rules.read_bytes()
        lock = indexlock.try_writer(indexlock.RULES, str(rules), full=False,
                                    what="gate_holder rules", create=True)
        self.addCleanup(lock.close)
        deck.write_text(deck.read_text() + "LAYER M2 32\n")
        res = floe_index("svrf", deck)
        self.assertEqual(res.returncode, 75, res.stderr)
        self.assertIn("deck.cal.rules.json is being indexed by %s" % HOLDER, lock_line(res))
        self.assertEqual(rules.read_bytes(), before)
        lock.close()
        floe_index("svrf", deck, ok=0)
        self.assertNotEqual(rules.read_bytes(), before)


def main():
    for binary in (BIN, RENDERD):
        if not binary.is_file():
            print("missing %s: cd rust && cargo build --release" % binary)
            return 2
    try:
        result = unittest.main(argv=[sys.argv[0]] + sys.argv[1:], exit=False,
                               verbosity=2).result
    finally:
        subprocess.run(["chmod", "-R", "u+w", str(TMP)])
        shutil.rmtree(TMP, ignore_errors=True)
    if not result.wasSuccessful():
        return 1
    print("INDEX LOCK: ALL OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
