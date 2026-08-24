import os


# The module entry point owns the stable product identity.  floe2 imports the
# shared package directly and sets its identity before doing so.
os.environ["FLOE_PRODUCT"] = "floe"

from .cli import main

# guard required: the render process is spawned and re-imports the main
# module; without this the child would re-run the CLI
if __name__ == "__main__":
    main()
