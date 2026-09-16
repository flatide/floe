"""Where a source's derived files live - the naming authority
(docs/CACHE-NAMING.ko.md).

Since 2026-09-16 (user decision 2026-09-15):

- the VFS index folder of a layout is a HIDDEN sibling
  ``.<name>.ice/`` (it was ``<name>.floe/``);
- the DRC results pack of a Calibre ``.db`` is a HIDDEN sibling
  ``.<name>.tray`` (it was ``<name>.ice``).

Only this module spells the suffixes. Everything that needs the path
of a cache or pack - the CLI, the viewer, the DRC reader, the jobdeck
catalogue, the gates - asks here, so the rule can change in one place.

A pre-rename cache or pack is RENAMED to the new name the first time
it is touched (`find_vfs_cache`, `find_pack`): nothing inside either
refers to its own name (identity is the source's size/mtime), so a
rename costs nothing and avoids a 40-minute re-index of a field deck.
When the rename is impossible (read-only folder, a race with another
process) the legacy path is used as it is and a note goes to stderr
once per process. Kill switch: ``FLOE_CACHE_MIGRATE=off`` keeps legacy
names untouched (they are still read).
"""

import os
import sys

VFS_SUFFIX = ".ice"           # hidden folder .<src>.ice/
VFS_LEGACY_SUFFIX = ".floe"   # <src>.floe/ (before 2026-09-16)
PACK_SUFFIX = ".tray"         # hidden file .<db>.tray
PACK_LEGACY_SUFFIX = ".ice"   # <db>.ice (before 2026-09-16)

_PACK_MAGIC = b"FLOEICE\x00"  # floe/drc.py _ICE_MAGIC (the pack format)
_reported = set()


def hidden_sibling(path, suffix):
    """`.<basename><suffix>` next to `path` (absolute)."""
    base = os.path.abspath(path)
    folder, name = os.path.split(base)
    return os.path.join(folder, "." + name + suffix)


def vfs_cache_dir(src):
    """The VFS cache folder of a source (need not exist yet)."""
    return hidden_sibling(src, VFS_SUFFIX)


def legacy_vfs_cache_dir(src):
    return os.path.abspath(src) + VFS_LEGACY_SUFFIX


def pack_path(db):
    """The DRC pack of a Calibre .db (need not exist yet)."""
    return hidden_sibling(db, PACK_SUFFIX)


def legacy_pack_path(db):
    return os.path.abspath(db) + PACK_LEGACY_SUFFIX


def migration_enabled():
    return os.environ.get("FLOE_CACHE_MIGRATE", "").lower() not in (
        "off", "0", "no")


def _rename(old, new, what):
    """Rename a legacy cache/pack in place; the path that holds it
    afterwards (the legacy one when the rename could not be done)."""
    if not migration_enabled():
        return old
    try:
        os.rename(old, new)
    except FileNotFoundError:
        # another process renamed it between our check and the rename
        return new if os.path.exists(new) else old
    except OSError as exc:
        if old not in _reported:
            _reported.add(old)
            sys.stderr.write("[floe] %s kept at its pre-2026-09-16 name "
                             "(%s): %s\n" % (what, exc, old))
        return old
    sys.stderr.write("[floe] %s renamed: %s -> %s\n" % (what, old, new))
    return new


def find_vfs_cache(src):
    """The folder holding `src`'s VFS cache: the current name when it
    has a meta.json, else a legacy `<src>.floe/` renamed to it (or
    kept when the rename fails); None when there is no cache."""
    new = vfs_cache_dir(src)
    if os.path.isfile(os.path.join(new, "meta.json")):
        return new
    old = legacy_vfs_cache_dir(src)
    if os.path.isfile(os.path.join(old, "meta.json")):
        return _rename(old, new, "VFS cache")
    return None


def _is_pack(path):
    try:
        with open(path, "rb") as f:
            head = f.read(12)
    except OSError:
        return False
    return (head[:8] == _PACK_MAGIC
            and int.from_bytes(head[8:12], "little") >= 2)


def find_pack(db):
    """The DRC pack of `db`: the current name when present, else a
    legacy `<db>.ice` that is a v2 pack renamed to it; None when there
    is none (a retired v1 sidecar is left for drc.load_db to report)."""
    new = pack_path(db)
    if os.path.exists(new):
        return new
    old = legacy_pack_path(db)
    if os.path.isfile(old) and _is_pack(old):
        return _rename(old, new, "DRC pack")
    return None


def db_name_of(pack):
    """The .db name a pack belongs to, from the pack's current or
    legacy name. Sidecars (`.<db>.waive.<user>`, `.<db>.notes.<user>.fe`)
    are named from it, so a hidden pack never yields `..<db>...` and the
    sidecars written beside a `<db>.ice` pack keep matching."""
    name = os.path.basename(pack)
    if name.startswith(".") and name.endswith(PACK_SUFFIX):
        return name[1:-len(PACK_SUFFIX)]
    if name.endswith(PACK_LEGACY_SUFFIX):
        return name[:-len(PACK_LEGACY_SUFFIX)]
    return name


def db_path_of(pack):
    """The .db path a pack belongs to (same folder, `db_name_of`)."""
    return os.path.join(os.path.dirname(os.path.abspath(pack)),
                        db_name_of(pack))
