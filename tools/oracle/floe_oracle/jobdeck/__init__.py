"""Calibre MDPView jobdeck (.jb) support for floe2.

The format has no public specification. Everything here was derived by
measurement on real decks and a KLayout-based reference tool (kept
outside the repository); see docs/JOBDECK.ko.md for what is confirmed,
what is not, and the implementation plan (M1: parse / place / colour
without any renderer change; M2+: the composite render).

The modules are deliberately free of KLayout and of the renderer so the
placement numbers can be checked against hand calculations.
"""

from .parser import Chip, Entry, JobDeck, parse_jobdeck  # noqa: F401
from .geom import (  # noqa: F401
    Placement, choose_dbu, layer_table, plan, to_grid,
    MISSING_RAISE, MISSING_SKIP,
)
from .color import (  # noqa: F401
    ColorScheme, JOBDECK_PALETTE, MODES, MODE_CHIP, MODE_IDENTIFIER,
    MODE_LAYER, color_order,
)
from .sources import SourceCatalog, file_header  # noqa: F401
from .plan import plan_deck, deck_summary, report_dict  # noqa: F401
