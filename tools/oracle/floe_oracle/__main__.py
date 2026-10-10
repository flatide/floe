"""`python -m floe_oracle CMD`: the frozen floe shell (dev-only oracle,
docs/SHARED_APP_LAYER.ko.md P3) - tools/oracle and the checkout on
PYTHONPATH."""
import os


# the frozen shell's identity (its product.py: the KLayout renderer by
# default, `floe` in its messages)
os.environ["FLOE_PRODUCT"] = "floe"

from .cli import main

# guard required: the render process is spawned and re-imports the main
# module; without this the child would re-run the CLI
if __name__ == "__main__":
    main()
