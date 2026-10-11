"""The GTK viewer's entry: `floe2 view` (the Rust command line) starts it
here, in Python because the viewer is the one part of floe2 still written
in it (docs/SHARED_APP_LAYER.ko.md). `python -B -m floe.gtkview ARGS` =
`floe2 view ARGS`."""

import sys


def main(argv=None):
    # the product's view front (P2d): view and gtktest
    from .viewcli import main as view_main
    return view_main(sys.argv[1:] if argv is None else list(argv))


if __name__ == "__main__":
    sys.exit(main())
