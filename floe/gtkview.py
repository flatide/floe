"""The GTK viewer's entry: `floe2 view` (the Rust command line) starts it
here, in Python because the viewer is the one part of floe2 still written
in it (docs/SHARED_APP_LAYER.ko.md). `python -B -m floe.gtkview ARGS` =
`floe2 view ARGS`."""

import sys


def main(argv=None):
    from .cli import main as cli_main
    argv = sys.argv[1:] if argv is None else list(argv)
    return cli_main(["view"] + argv, prog="floe2", rust_only=True)


if __name__ == "__main__":
    sys.exit(main())
