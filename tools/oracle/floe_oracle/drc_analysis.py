"""Identity and small shared metadata for immutable DRC analysis caches."""

import hashlib
import os
import tempfile

from . import cachepath


def pack_identity(path):
    """Constant-size fingerprint; never hash a multi-billion-error payload.

    Replacing/repacking or modifying the source invalidates derived data.
    Review sidecars do not participate in the immutable geometry identity.
    """
    with open(path, "rb") as stream:
        stat = os.fstat(stream.fileno())
        head = stream.read(40)
        stream.seek(max(0, stat.st_size - 144))
        tail = stream.read(144)
    stamp = (stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns,
             stat.st_dev, stat.st_ino)
    return hashlib.sha256(repr(stamp).encode("ascii") + head + tail).hexdigest()


def _bounds_path(path, identity=None):
    return os.path.join(cachepath.drc_analysis_dir(path),
                        "bounds-" + (identity or pack_identity(path)) + ".npy")


def load_rule_bounds(path, count, identity=None):
    """Only O(rule count) data; a child must not rescan the block table."""
    import numpy as np
    try:
        values = np.load(_bounds_path(path, identity), allow_pickle=False)
        if values.shape == (count, 4) and values.dtype == np.dtype("int64"):
            return values
    except (OSError, ValueError, EOFError):
        pass
    return None


def save_rule_bounds(path, values, identity=None):
    """Best effort, atomic metadata cache; read-only packs remain usable."""
    import numpy as np
    temporary = None
    try:
        target = _bounds_path(path, identity)
        os.makedirs(os.path.dirname(target), exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=os.path.dirname(target),
                                         prefix=".bounds-", delete=False) as stream:
            temporary = stream.name
            np.save(stream, values, allow_pickle=False)
        os.replace(temporary, target)
    except OSError:
        pass
    finally:
        if temporary is not None:
            try:
                os.unlink(temporary)
            except OSError:
                pass
