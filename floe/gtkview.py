"""The GTK viewer's entry: `floe2 view` (the Rust command line) starts it
here, in Python because the viewer is the one part of floe2 still written
in it (docs/SHARED_APP_LAYER.ko.md). `python -B -m floe.gtkview ARGS` =
`floe2 view ARGS`."""

import sys

# subprocess's text= and capture_output= (gtkservice.py, gui.py)
MIN_PYTHON = (3, 7)


def main(argv=None):
    # floe2 started from somewhere other than the bundle's launcher can
    # land on the host's python3 (RHEL 8's is 3.6, user 2026-10-13): say
    # so here, not as a TypeError in Help > About or the first open
    if sys.version_info < MIN_PYTHON:
        sys.stderr.write(
            "floe2 view: the viewer needs Python %d.%d or newer; %s is "
            "Python %s. Start it with the portable bundle's floe2, or set "
            "FLOE_GTK_PYTHON to a Python %d.%d+ with PyGObject.\n"
            % (MIN_PYTHON + (sys.executable or "python",
                             sys.version.split()[0]) + MIN_PYTHON))
        return 2
    # the product's view front (P2d): view and gtktest
    from .viewcli import main as view_main
    return view_main(sys.argv[1:] if argv is None else list(argv))


if __name__ == "__main__":
    sys.exit(main())
