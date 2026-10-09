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
  K5  renderd open: hier and --occupancy-only go ahead (same host); an
      addition while another holds the build lock ends 75; a live reader
      on another host refuses an addition
  K6  `floe2 index` on a cache being written says "being indexed", not
      "--force"; the jobdeck index counts a busy source and builds the
      other; the legacy <src>.floe rename waits for the locks
  K7  a killed reader's registration does not count; FLOE_LOCK=off runs
      as before and makes no .floe-lock; a read-only folder's cache opens
      unlocked; the lock folder takes the folder's write bits; probes
      never refuse a writer; a deck of more sources than the open-file
      limit leaves room for opens unlocked; the viewer's index dialog
      runs in its own process group (a cancel ends what it started)

usage: python tools/validate_index_lock.py
"""
import fcntl
import inspect
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
os.environ["FLOE_RENDERER"] = "rust"
BIN = ROOT / "rust" / "target" / "release" / "floe-index"
RENDERD = ROOT / "rust" / "target" / "release" / "floe-renderd"
TMP = Path(tempfile.mkdtemp(prefix="floe-indexlock-"))
HOLDER = "ws_kim_01"   # an underscore name: the wire must not mangle it

os.environ["FLOE_INDEX_BIN"] = str(BIN)
os.environ["FLOE_RENDERD_BIN"] = str(RENDERD)
os.environ["FLOE_REVIEWER"] = HOLDER
os.environ.pop("FLOE_LOCK", None)
os.environ.pop("FLOE_LOCK_WHAT", None)

from floe import cachepath, indexlock  # noqa: E402


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
    return run([sys.executable, "-B", "-m", "floe2", *args], ok=ok, env=env)


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
    from floe.cache import Cache
    from floe.rust_render import RustRenderWorker
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
            ("floe2 index --force", [sys.executable, "-B", "-m", "floe2", "index", self.src, "--force"]),
            ("floe2 index --occupancy-only", [sys.executable, "-B", "-m", "floe2", "index", self.src, "--occupancy-only"]),
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
        from floe import cli
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
        from floe.cache import Cache
        from floe.rust_render import RustRenderWorker
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
        from floe.jobdeck.viewer import DeckCache
        deck = DeckCache(str(d / "lock.jb"))
        deck.load()
        self.addCleanup(deck.close)
        busy = [r for r in deck.ledger if r["tc"] == "srcB.oas"]
        self.assertTrue(busy, deck.ledger)
        self.assertIn("srcB.oas is being indexed by %s" % HOLDER, busy[0]["detail"])
        self.assertFalse([r for r in deck.ledger if r["tc"] == "srcA.oas"])
        from floe.service import make_render_worker
        w = make_render_worker(deck)
        w.start()
        w.stop()

    def test_k4_a_reader_refuses_a_whole_rebuild(self):
        w = open_worker(self.src)
        try:
            users = indexlock.state(indexlock.VFS, str(self.cache)).users
            self.assertEqual(len(users), 1)
            for argv in ([BIN, "vfs", self.src, "--jobs", "2"],
                         [sys.executable, "-B", "-m", "floe2", "index", self.src, "--force"]):
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

    def test_k5_additions_beside_readers(self):
        w = open_worker(self.src)
        try:
            floe_index("hier", self.cache, ok=0)
            floe2("index", self.src, "--occupancy-only", ok=0)
            lock = self.hold(full=False)
            res = floe_index("hier", self.cache)
            self.assertEqual(res.returncode, 75)
            self.assertIn("being indexed by %s" % HOLDER, lock_line(res))
            lock.close()
        finally:
            w.stop()
        # a live reader on another host: an addition from here would pull
        # its mapped files from under it
        k = indexlock.key(indexlock.VFS, str(self.cache))
        reg = Path(k.dir) / "use.elsewhere.4242.0"
        fd = os.open(str(reg), os.O_RDWR | os.O_CREAT, 0o644)
        self.addCleanup(lambda: (os.close(fd), reg.exists() and reg.unlink()))
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        os.write(fd, indexlock.Holder("lee", "pc-17", "elsewhere", 4242, "x",
                                      "floe2 view").text([k.name]).encode())
        res = floe_index("hier", self.cache)
        self.assertEqual(res.returncode, 75, res.stderr)
        self.assertIn("chip.oas is in use on another host by lee (from pc-17) on elsewhere, pid 4242",
                      lock_line(res))

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
        res = run(["/bin/sh", "-c", 'ulimit -n 32 && exec "$0" -B -m floe2 render "$1" --px 200 --out "$2"',
                   sys.executable, d / "many.jb", d / "many.png"])
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)
        self.assertIn("without their locks", res.stderr)

    def test_k7_the_index_dialog_owns_its_process_group(self):
        from floe import gui
        source = inspect.getsource(gui.Viewer._index_modal)
        self.assertIn("start_new_session=True", source)
        self.assertIn("os.killpg(proc.pid, signal.SIGTERM)", source)


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
