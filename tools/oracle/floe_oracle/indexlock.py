"""Locks on what the indexer writes and the renderer reads - the Python
twin of rust/vfs/src/lock.rs (docs/CACHE-NAMING.ko.md "잠금"; user
2026-10-09: "if the same file is being indexed or used, a run must be
refused - whoever runs it").

Kernel advisory locks (``fcntl.flock``; the Rust side takes the same
locks with ``File::try_lock``), released by the kernel when the holder
exits or is killed. Each target has two lock files in ``.floe-lock/``
beside it, named by its key (``X.oas.vfs``, ``X.db.pack``,
``deck.cal.rules``):

- ``<key>.build``: exclusive for every run that writes the target; the
  holder writes who it is into it;
- ``<key>.use``: shared for the readers, exclusive for a run that
  rebuilds the target whole.

A run adding files beside the others holds ``.build`` alone - unless it
replaces one the readers map (design.ovh/.ovo/.ovr): then ``.use`` too,
whatever host the readers are on. Only the kernel locks decide; a reader
also registers in ``use.<host>.<pid>.<n>`` (who, and the keys it holds;
exclusive-locked by its owner - one whose lock can be taken is a dead
owner's) so a refused writer can name the people in its way. A target's
key is taken from its real path (symbolic links resolved).

The writers are floe-index runs and take their locks themselves. Python
asks who is in the way before it starts something (``state`` - a probe
never creates a file), holds a reader's lock while it has a DRC pack
open (``hold_readers``), and takes a writer's lock for a moment around
what it changes itself (``try_writer``). ``FLOE_LOCK=off`` turns it all
off.
"""

import errno
import fcntl
import os
import socket
import time

DIR = ".floe-lock"
BUSY_EXIT = 75          # a run refused for a busy target (EX_TEMPFAIL)
VFS, PACK, RULES = "vfs", "pack", "rules"
_RETRY_S = 1.5
_RETRY_STEP_S = 0.05


def disabled():
    """FLOE_LOCK=off: no lock is taken or looked at (the kill switch)."""
    return os.environ.get("FLOE_LOCK", "").strip().lower() in ("off", "0",
                                                                "no")


def _retry_s():
    try:
        return int(os.environ["FLOE_LOCK_RETRY_MS"]) / 1000.0
    except (KeyError, ValueError):
        return _RETRY_S


class Key(object):
    """A target's lock: its ``.floe-lock`` folder, name and what messages
    call it."""

    __slots__ = ("dir", "name", "subject")

    def __init__(self, dir, name, subject):
        self.dir, self.name, self.subject = dir, name, subject

    def __eq__(self, other):
        return isinstance(other, Key) and (self.dir, self.name,
                                           self.subject) == (
            other.dir, other.name, other.subject)

    def __repr__(self):
        return "Key(%r, %r, %r)" % (self.dir, self.name, self.subject)


def _real_path(target):
    """The target's real path, as rust/vfs/src/lock.rs ``real_path``: a
    target reached through a symbolic link locks as itself (review
    2026-10-09 of 9378c6d7, P1); its folder alone resolved when it is not
    there yet (a first build)."""
    if os.path.exists(target):
        return os.path.realpath(target)
    folder, base = os.path.split(target)
    return os.path.join(os.path.realpath(folder or "."), base)


def key(kind, target):
    """The key of the target at ``target`` (a cache folder, a pack, a
    rules file): the source's name and the kind, so a legacy name
    (``X.oas.floe``, ``X.db.ice``) shares the lock of the current one
    (rust/vfs/src/lock.rs ``key``)."""
    folder, base = os.path.split(_real_path(target))
    folder = folder or "."

    def strip(pre, suf):
        if base.startswith(pre) and base.endswith(suf) and \
                len(base) > len(pre) + len(suf):
            return base[len(pre):len(base) - len(suf)]
        return None

    if kind == VFS:
        n = strip(".", ".ice") or strip("", ".floe") or base
        return Key(os.path.join(folder, DIR), n + ".vfs", n)
    if kind == PACK:
        n = strip(".", ".tray") or strip("", ".ice") or base
        return Key(os.path.join(folder, DIR), n + ".pack", n)
    if kind == RULES:
        name = base[:-5] if base.endswith(".rules.json") else base + ".rules"
        return Key(os.path.join(folder, DIR), name, base)
    raise ValueError("lock kind %r" % kind)


class Holder(object):
    """Who holds a lock: the reviewer (accounts are shared on the servers -
    FLOE_REVIEWER, else the account), where they sit (DISPLAY host or ssh
    client), the server, the process, since when and what it runs."""

    __slots__ = ("who", "frm", "host", "pid", "since", "what")

    def __init__(self, who="", frm="", host="", pid=0, since="", what=""):
        self.who, self.frm, self.host = who, frm, host
        self.pid, self.since, self.what = pid, since, what

    @classmethod
    def me(cls, what):
        env = lambda name: os.environ.get(name, "").strip()  # noqa: E731
        who = env("FLOE_REVIEWER") or env("USER") or env("LOGNAME") \
            or "uid %d" % os.getuid()
        host = env("DISPLAY").rsplit(":", 1)[0] if ":" in env("DISPLAY") \
            else ""
        frm = host if host and not host.startswith("/") and host not in (
            "localhost", "127.0.0.1", "::1", "unix") else ""
        if not frm:
            ssh = (env("SSH_CONNECTION") or env("SSH_CLIENT")).split()
            frm = ssh[0] if ssh else ""
        one = lambda s: " ".join(str(s).splitlines()).strip()  # noqa: E731
        return cls(one(who), one(frm), socket.gethostname(), os.getpid(),
                   time.strftime("%Y-%m-%d %H:%M:%S"), one(what))

    def text(self, keys=()):
        out = "who=%s\nfrom=%s\nhost=%s\npid=%d\nsince=%s\nwhat=%s\n" % (
            self.who, self.frm, self.host, self.pid, self.since, self.what)
        return out + "".join("key=%s\n" % k for k in keys)

    @classmethod
    def parse(cls, text):
        h, keys = cls(), []
        for line in text.splitlines():
            k, sep, v = line.partition("=")
            if not sep:
                continue
            if k == "who":
                h.who = v
            elif k == "from":
                h.frm = v
            elif k == "host":
                h.host = v
            elif k == "pid":
                try:
                    h.pid = int(v)
                except ValueError:
                    pass
            elif k == "since":
                h.since = v
            elif k == "what":
                h.what = v
            elif k == "key":
                keys.append(v)
        return h, keys

    def __str__(self):
        """``kim (from 10.1.2.3) on srv02, pid 4242, since 2026-10-09
        15:20:11 (floe-index vfs)`` - as the Rust side says it"""
        out = self.who or "someone"
        if self.frm and self.frm != self.who:
            out += " (from %s)" % self.frm
        if self.host:
            out += " on %s" % self.host
        if self.pid:
            out += ", pid %d" % self.pid
        if self.since:
            out += ", since %s" % self.since
        if self.what:
            out += " (%s)" % self.what
        return out


def _users_text(users):
    if not users:
        return "a viewer that did not say who it is"
    shown = "; ".join(str(u) for u in users[:3])
    more = len(users) - 3
    return "%s and %d more" % (shown, more) if more > 0 else shown


class Busy(Exception):
    """Why a lock was not taken - worded as the Rust side words it.
    ``what``: "indexing" (a run writes the target; ``opening``: a reader
    was refused), "in_use" (readers hold it), "error"."""

    def __init__(self, what, subject, holder=None, users=(), opening=False,
                 message=""):
        self.what, self.subject, self.holder = what, subject, holder
        self.users, self.opening = list(users), opening
        self.message = message
        Exception.__init__(self, str(self))

    def __str__(self):
        if self.what == "indexing":
            by = str(self.holder) if self.holder else "another process"
            return "%s is being indexed by %s - %s" % (
                self.subject, by, "open it when the index is done"
                if self.opening else "try again when it finishes")
        if self.what == "in_use":
            return ("%s is in use by %s - re-indexing it would pull it from "
                    "under them; close it there first"
                    % (self.subject, _users_text(self.users)))
        return ("%s: cannot take its lock: %s (FLOE_LOCK=off runs without "
                "the lock)" % (self.subject, self.message))


def _read(path):
    try:
        with open(path, "r", errors="replace") as f:
            return f.read()
    except OSError:
        return None


def _holder_at(path):
    text = _read(path)
    if text is None:
        return None
    h, _ = Holder.parse(text)
    return h if (h.who or h.pid) else None


def _try(fd, how):
    """One non-blocking flock: True locked, False held by another,
    None when this file system cannot lock."""
    try:
        fcntl.flock(fd, how | fcntl.LOCK_NB)
        return True
    except BlockingIOError:
        return False
    except OSError as exc:
        if exc.errno in (errno.EWOULDBLOCK, errno.EAGAIN, errno.EACCES):
            return False
        if exc.errno in (errno.ENOLCK, errno.EOPNOTSUPP, errno.ENOSYS):
            return None
        raise


def _lock_retry(fd, how):
    started = time.monotonic()
    while True:
        got = _try(fd, how)
        if got is not False or time.monotonic() - started >= _retry_s():
            return got
        time.sleep(_RETRY_STEP_S)


def users_of(k):
    """The live readers registered in the key's folder that hold it (a
    registration whose lock can be taken is a dead owner's: skipped, and
    removed when this user may)."""
    users = []
    try:
        names = sorted(os.listdir(k.dir))
    except OSError:
        return users
    for name in names:
        if not name.startswith("use.") or name.endswith(".tmp"):
            continue
        path = os.path.join(k.dir, name)
        try:
            fd = os.open(path, os.O_RDONLY)
        except PermissionError:
            # another account's registration this one cannot read (review
            # 2026-10-09 of 9378c6d7, P2): a user all the same, named from
            # its file name - never taken for none
            users.append(_unreadable_registration(name))
            continue
        except OSError:
            continue
        try:
            got = _try(fd, fcntl.LOCK_SH)
        except OSError:
            got = None
        if got:
            fcntl.flock(fd, fcntl.LOCK_UN)
            os.close(fd)
            try:
                os.unlink(path)
            except OSError:
                pass
            continue
        os.close(fd)
        if got is False:
            text = _read(path)
            if text is not None:
                h, keys = Holder.parse(text)
                if k.name in keys:
                    users.append(h)
    return users


def _unreadable_registration(name):
    """A registration this account cannot read: host and pid from its name
    (``use.<host>.<pid>.<n>``)."""
    parts = name[len("use."):].rsplit(".", 2)
    host, pid = (parts[0], parts[1]) if len(parts) == 3 else ("", "0")
    try:
        pid = int(pid)
    except ValueError:
        pid = 0
    return Holder("someone", "", host, pid, "",
                  "registration not readable by this account")


class State(object):
    """Who is at a target now: ``writer`` (the holder of its ``.build``,
    None when no run writes it), ``rebuilding`` (that run rebuilds it whole
    and holds its readers out), ``users`` (the live readers)."""

    __slots__ = ("key", "writer", "rebuilding", "users")

    def __init__(self, k, writer=None, rebuilding=False, users=()):
        self.key, self.writer, self.rebuilding = k, writer, rebuilding
        self.users = list(users)

    @property
    def building(self):
        """A run rebuilds it whole: it cannot be opened nor written."""
        return self.writer is not None and self.rebuilding

    @property
    def writing(self):
        return self.writer is not None

    def opening_refusal(self):
        """Busy for a reader, or None."""
        if self.building:
            return Busy("indexing", self.key.subject, self.writer,
                        opening=True)
        return None

    def writing_refusal(self, full):
        """Busy for a run that writes the target (whole, or replacing a
        file the readers map, when ``full``; else adding files beside the
        others), or None - what floe-index would refuse, said before it is
        started."""
        if self.writing:
            return Busy("indexing", self.key.subject, self.writer)
        if full and self.users:
            return Busy("in_use", self.key.subject, users=self.users)
        return None


def state(kind, target, users=True):
    """Who is at ``target`` now - a probe: it takes a lock only for an
    instant and never creates a file (FLOE_LOCK=off: nobody). ``users``
    False skips the readers (a deck's hundreds of sources ask only
    whether a run rebuilds one)."""
    k = key(kind, target)
    st = State(k)
    if disabled():
        return st
    build = os.path.join(k.dir, k.name + ".build")
    try:
        fd = os.open(build, os.O_RDONLY)
    except OSError:
        fd = None
    if fd is not None:
        try:
            got = _try(fd, fcntl.LOCK_SH)
            if got:
                fcntl.flock(fd, fcntl.LOCK_UN)
            elif got is False:
                st.writer = _holder_at(build) or Holder()
        except OSError:
            pass
        finally:
            os.close(fd)
    if st.writer is not None:
        try:
            ufd = os.open(os.path.join(k.dir, k.name + ".use"), os.O_RDONLY)
        except OSError:
            ufd = None
        if ufd is not None:
            try:
                got = _try(ufd, fcntl.LOCK_SH)
                if got:
                    fcntl.flock(ufd, fcntl.LOCK_UN)
                st.rebuilding = got is False
            except OSError:
                pass
            finally:
                os.close(ufd)
    if users:
        st.users = users_of(k)
    return st


def _folder_mode(lock_dir):
    try:
        return os.stat(os.path.dirname(lock_dir) or ".").st_mode & 0o7777
    except OSError:
        return 0o755


def _dir_mode(folder):
    shared = folder & 0o022
    return 0o755 | shared | (folder & 0o2000) | (0o1000 if shared else 0)


def _file_mode(folder):
    return 0o644 | (folder & 0o022)


def _ensure_dir(d, folder):
    try:
        os.mkdir(d)
    except FileExistsError:
        if os.path.isdir(d):
            return
        raise
    try:
        os.chmod(d, _dir_mode(folder))
    except OSError:
        pass


def _open_lock(path, folder, write):
    """A lock file: read-write for an exclusive lock (NFS asks it),
    read-only for a shared one; made with the folder's bits when new."""
    try:
        fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_EXCL,
                     _file_mode(folder))
        try:
            os.fchmod(fd, _file_mode(folder))
        except OSError:
            pass
        return fd
    except FileExistsError:
        return os.open(path, os.O_RDWR if write else os.O_RDONLY)


_sequence = [0]


class ReaderLock(object):
    """A reader's locks (``hold_readers``): released by ``close``."""

    def __init__(self):
        self._fds = []
        self._registrations = []

    def close(self):
        for path, fd in self._registrations:
            try:
                os.unlink(path)
            except OSError:
                pass
            try:
                os.close(fd)
            except OSError:
                pass
        self._registrations = []
        for fd in self._fds:
            try:
                os.close(fd)
            except OSError:
                pass
        self._fds = []

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass


def _register(d, names, what):
    _sequence[0] += 1
    base = "use.%s.%d.%d" % ("".join(
        c if c.isalnum() or c in "-." else "_"
        for c in socket.gethostname()), os.getpid(), _sequence[0])
    tmp = os.path.join(d, base + ".tmp")
    path = os.path.join(d, base)
    try:
        fd = os.open(tmp, os.O_RDWR | os.O_CREAT | os.O_TRUNC, 0o644)
    except OSError:
        return None
    try:
        # readable by every account that may name it, whatever the umask
        # (review 2026-10-09 of 9378c6d7, P2: umask 077 made it 0600)
        os.fchmod(fd, 0o644)
        if not _try(fd, fcntl.LOCK_EX):
            raise OSError("registration lock")
        os.write(fd, Holder.me(what).text(names).encode("utf-8"))
        os.rename(tmp, path)
    except OSError:
        os.close(fd)
        try:
            os.unlink(tmp)
        except OSError:
            pass
        return None
    return path, fd


def reader_label(default):
    """What a reader says it runs: FLOE_LOCK_WHAT when set, else
    ``default``."""
    return os.environ.get("FLOE_LOCK_WHAT", "").strip() or default


def hold_readers(kind, targets, what):
    """Take a reader's locks on ``targets``: Busy("indexing") while a run
    rebuilds one whole. A folder this user cannot lock in is read
    unlocked - a reader is never stopped by its own lock."""
    lock = ReaderLock()
    if disabled():
        return lock
    held = []
    for t in targets:
        k = key(kind, t)
        folder = _folder_mode(k.dir)
        try:
            _ensure_dir(k.dir, folder)
            fd = _open_lock(os.path.join(k.dir, k.name + ".use"), folder,
                            write=False)
        except OSError:
            continue
        got = _lock_retry(fd, fcntl.LOCK_SH)
        if got is False:
            os.close(fd)
            lock.close()
            raise Busy("indexing", k.subject,
                       _holder_at(os.path.join(k.dir, k.name + ".build")),
                       opening=True)
        if got is None:
            os.close(fd)
            continue
        lock._fds.append(fd)
        held.append(k)
    for d in sorted({k.dir for k in held}):
        reg = _register(d, [k.name for k in held if k.dir == d], what)
        if reg is not None:
            lock._registrations.append(reg)
    return lock


class WriterLock(object):
    """A writer's locks taken for a moment (``try_writer``)."""

    def __init__(self, fds):
        self._fds = fds

    def close(self):
        for fd in self._fds:
            try:
                os.close(fd)
            except OSError:
                pass
        self._fds = []

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        self.close()


def try_writer(kind, target, full, what, create=False):
    """A writer's locks on ``target`` if free at once (no retry: a busy
    target means someone else's work - leave it be), else None. Without
    ``create`` the lock folder is not made: a target nobody locked is
    free (Python's own moment of clean-up or rename). FLOE_LOCK=off: an
    empty lock."""
    if disabled():
        return WriterLock([])
    k = key(kind, target)
    folder = _folder_mode(k.dir)
    if not os.path.isdir(k.dir):
        if not create:
            return WriterLock([])
        try:
            _ensure_dir(k.dir, folder)
        except OSError:
            return None
    fds = []
    try:
        for suffix in ((".build", ".use") if full else (".build",)):
            fd = _open_lock(os.path.join(k.dir, k.name + suffix), folder,
                            write=True)
            fds.append(fd)
            got = _try(fd, fcntl.LOCK_EX)
            if got is None:
                # this file system cannot lock: nothing to wait for
                for f in fds:
                    os.close(f)
                return WriterLock([])
            if not got:
                raise BlockingIOError()
        os.ftruncate(fds[0], 0)
        os.write(fds[0], Holder.me(what).text().encode("utf-8"))
    except OSError:
        for fd in fds:
            os.close(fd)
        return None
    return WriterLock(fds)
