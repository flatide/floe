"""floe2 - Rust-only floe product sharing the canonical VFS workspace."""

import os


# Set the identity before importing any shared module.  Spawned renderer
# processes inherit it, while floe and floe2 retain independent GUI sockets.
os.environ["FLOE_PRODUCT"] = "floe2"

from floe import __version__  # noqa: E402,F401
