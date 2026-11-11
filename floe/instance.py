"""Single-instance plumbing, mirroring flateyes.

One viewer window per (uid, DISPLAY). The first invocation opens the
window; later invocations on the same DISPLAY hand their request - the
layout, and since 0.12.446 the DRC results, the rules metadata, the reviewer
and the file loads turned off - to the running window over a unix socket and
exit immediately; the window switches to it. Different DISPLAY values get
independent windows, so many user displays on one Linux host coexist.
`--multi` opts out.

stdlib only - the forward path must never import GTK or klayout, so
repeat invocations stay fast.
"""

import errno
import hashlib
import os
import socket
import sys
import tempfile

from .product import name as product_name

APP = product_name()


def display_key():
    """Identity of the current display: $DISPLAY on X11, 'aqua' on a
    macOS dev host (no DISPLAY under native Tk). None = no display."""
    display = os.environ.get("DISPLAY")
    if display:
        return normalize_display(display)
    if sys.platform == "darwin":
        return "aqua"
    return None


def normalize_display(display):
    """':1.0' and ':1' are the same X display; drop the screen suffix."""
    display = display.strip()
    host, _, num = display.rpartition(":")
    if "." in num:
        num = num.split(".", 1)[0]
    return "%s:%s" % (host, num)


def socket_address(display):
    # "2": since 0.12.446 a request carries more than a layout - the DRC
    # results, the rules metadata, the reviewer, the file loads turned off.
    # A window of an earlier version would open the layout and drop the
    # rest, so it is not the one these requests are handed to.
    key = "%s-2-%d-%s" % (APP, os.getuid(), display)
    if sys.platform.startswith("linux"):
        # Abstract namespace: no socket file on disk, vanishes with the
        # process, so stale sockets are impossible.
        return "\0" + key
    base = os.environ.get("XDG_RUNTIME_DIR") or tempfile.gettempdir()
    path = os.path.join(base, key.replace("/", "_") + ".sock")
    if len(path.encode("utf-8")) > 96:
        # sun_path holds ~104 bytes on macOS/BSD; fall back to a digest of
        # the key, still deterministic per (uid, DISPLAY)
        digest = hashlib.sha1(key.encode("utf-8")).hexdigest()[:16]
        path = os.path.join(base, "%s-%d-%s.sock"
                            % (APP, os.getuid(), digest))
    return path


def try_forward(addr, request):
    """Hand the request line ("path[\\tkey=value...]") to a running
    instance. Returns an exit code if an instance handled (or failed to
    handle) the request, or None when no instance is listening."""
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(5.0)
    try:
        sock.connect(addr)
    except OSError:
        sock.close()
        return None
    try:
        sock.sendall(request.encode("utf-8") + b"\n")
        reply = b""
        while b"\n" not in reply and len(reply) < 65536:
            chunk = sock.recv(4096)
            if not chunk:
                break
            reply += chunk
    except OSError:
        sys.stderr.write("%s: existing instance did not respond\n" % APP)
        return 1
    finally:
        sock.close()
    text = reply.decode("utf-8", "replace").strip()
    if text.startswith("OK"):
        return 0
    sys.stderr.write("%s\n" % (text or "%s: empty reply from existing "
                                       "instance" % APP))
    return 1


def try_bind(addr):
    """Become the instance owner. Returns a listening socket or None if
    another process owns (or just grabbed) the address."""
    for _ in range(2):
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            sock.bind(addr)
            sock.listen(8)
            return sock
        except OSError as exc:
            sock.close()
            if exc.errno != errno.EADDRINUSE:
                raise
            if addr.startswith("\0"):
                return None
            # Filesystem socket: unlink only if nothing is listening there,
            # otherwise we would steal a live instance's socket.
            probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            probe.settimeout(1.0)
            try:
                probe.connect(addr)
            except OSError:
                pass  # dead leftover; safe to remove
            else:
                return None
            finally:
                probe.close()
            try:
                os.unlink(addr)
            except OSError:
                return None
    return None
