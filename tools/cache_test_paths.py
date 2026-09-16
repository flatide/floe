"""Pure canonical-name oracle for native application gates; never migrates files."""
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from floe.cachepath import pack_path, vfs_cache_dir


def vfs_cache(source):
    return Path(vfs_cache_dir(source))


def drc_pack(source):
    return Path(pack_path(source))
