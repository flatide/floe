"""`python -m floe_oracle.floe2 CMD`: the Python floe2 command line as it
was before the Rust `floe2` (docs/SHARED_APP_LAYER.ko.md §4, P1c) - the
reference feature/webui's app_* gates hold `floe2-web` to (index, info,
render, probe, jobdeck). The product itself has no Python command line.

`FLOE_PRODUCT=floe2` before the import: the shell is named floe2 and
renders with Rust alone (its cli.main's rust_only)."""
import os


os.environ["FLOE_PRODUCT"] = "floe2"

from .cli import main

# guard required: the render process is spawned and re-imports the main
# module; without this the child would re-run the CLI
if __name__ == "__main__":
    main()
