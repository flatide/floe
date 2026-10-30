//! What a capture reads of a deck (user 2026-10-29: "특정 위치를 캡쳐한다고
//! 할때, jobdeck의 경우 해당하는 EBEAM 파일만 읽어서 처리하도록"): the CHIP
//! blocks `render --chipid` / `--chipnumber` name, and the window of the
//! run's shots - their regions in deck um.
//!
//! A deck's open probes every source of the levels loaded (a header, the
//! cache's meta.json - no geometry), and renderd culls a placement its view
//! misses before any plan or page read; but the spec, renderd's open, the
//! auto-index and the temporary index's summaries took every source of the
//! levels loaded too - a location captured through `--temp-index` indexed
//! the whole level (135 sources, ~1 TB in the field) for the one or two it
//! drew. The window keeps them to the sources with a placement in it: those
//! alone go to the spec and the index plans, the rest of the level is left
//! as it is.
//!
//! A placement's extent is the deck's own (the entry's BX..UY placed,
//! geom::plan's `bbox_um`), so nothing is read to know it; an extent the deck
//! leaves degenerate (BX..UY missing - the parser's warning) is unknown, and
//! such a placement is always read. A capture is drawn as the whole deck
//! would draw it: what the window leaves out is what renderd's cull would
//! skip. `FLOE_DECK_WINDOW=off` reads the deck whole, as before.
use super::{geom::Placement, index::ChipSelection};
use crate::{Error, Result};
use serde::Serialize;
use std::collections::BTreeSet;

/// Whether a capture's window is applied (`FLOE_DECK_WINDOW=off`: no - a
/// capture opens and indexes every source of the levels loaded, as before
/// 0.12.421).
pub fn window_enabled() -> bool {
    std::env::var("FLOE_DECK_WINDOW").map_or(true, |v| v != "off")
}

/// The regions a run of captures draws, deck um.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeckWindow {
    /// normalized: x0 <= x1, y0 <= y1
    pub rects: Vec<[f64; 4]>,
}
impl DeckWindow {
    pub fn new(rects: impl IntoIterator<Item = [f64; 4]>) -> Result<Self> {
        let rects = rects
            .into_iter()
            .map(|r| {
                if !r.iter().all(|v| v.is_finite()) {
                    return Err(Error::input("the capture's region is not finite"));
                }
                Ok([
                    r[0].min(r[2]),
                    r[1].min(r[3]),
                    r[0].max(r[2]),
                    r[1].max(r[3]),
                ])
            })
            .collect::<Result<Vec<_>>>()?;
        if rects.is_empty() {
            return Err(Error::input("the capture's window has no region"));
        }
        Ok(Self { rects })
    }
    /// Whether a placement with `extent` (Placement::bbox_um) is read: it
    /// touches one of the regions (closed intervals - a shared edge counts),
    /// or its extent is degenerate (no positive span in x or y: the deck's
    /// BX..UY missing) and so unknown.
    pub fn reaches(&self, extent: &[f64; 4]) -> bool {
        let [x0, y0, x1, y1] = *extent;
        if !(x1 > x0 && y1 > y0) {
            return true;
        }
        self.rects
            .iter()
            .any(|r| x0 <= r[2] && x1 >= r[0] && y0 <= r[3] && y1 >= r[1])
    }
    /// The sources of the placements the window reaches.
    pub fn sources(&self, placements: &[Placement]) -> BTreeSet<String> {
        placements
            .iter()
            .filter(|p| self.reaches(&p.bbox_um))
            .map(|p| p.tc.clone())
            .collect()
    }
}

/// What a capture reads of a deck: both unsaid, the whole load (the
/// viewer's open, `info`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeckReads {
    /// `render --chipid` / `--chipnumber`: these CHIP blocks' sources and
    /// placements alone (the others are not placed, as levels not loaded)
    pub chips: Option<ChipSelection>,
    pub window: Option<DeckWindow>,
}

/// What a window came to in a deck (the dataset's metadata `jobdeck.window`,
/// the capture report, the `[jobdeck] window` line).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WindowInfo {
    pub rects_um: Vec<[f64; 4]>,
    /// the sources read, of those the load places
    pub sources: usize,
    pub sources_total: usize,
    /// the placements the window reaches, of the load's
    pub placements: usize,
    pub placements_total: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(tc: &str, bbox_um: [f64; 4]) -> Placement {
        Placement {
            chip: "C".into(),
            idx: 1,
            row: 0,
            tc: tc.into(),
            ly: 1,
            dt: 0,
            mag: 1.,
            dx_um: 0.,
            dy_um: 0.,
            ix: 0,
            iy: 0,
            rx_um: 0.,
            ry_um: 0.,
            bbox_um,
            jx: 0.,
            jy: 0.,
        }
    }

    #[test]
    fn a_window_reaches_the_placements_it_touches_and_every_unknown_extent() {
        // reversed corners are normalized; nothing or a non-finite corner refused
        let w = DeckWindow::new([[30., 40., 10., 20.]]).unwrap();
        assert_eq!(w.rects, vec![[10., 20., 30., 40.]]);
        assert!(DeckWindow::new([]).is_err());
        assert!(DeckWindow::new([[0., 0., f64::NAN, 1.]]).is_err());
        // inside, overlapping, a shared edge, a shared corner: reached
        assert!(w.reaches(&[12., 22., 14., 24.]));
        assert!(w.reaches(&[0., 0., 100., 100.]));
        assert!(w.reaches(&[30., 20., 50., 40.]));
        assert!(w.reaches(&[0., 0., 10., 20.]));
        // beside it in x or in y: not
        assert!(!w.reaches(&[30.1, 20., 50., 40.]));
        assert!(!w.reaches(&[10., 40.1, 30., 60.]));
        assert!(!w.reaches(&[-5., -5., -1., -1.]));
        // a degenerate extent (BX..UY missing, or reversed): always read
        assert!(w.reaches(&[500., 500., 500., 500.]));
        assert!(w.reaches(&[500., 500., 600., 500.]));
        assert!(w.reaches(&[600., 500., 500., 600.]));
        // several regions: any of them
        let two = DeckWindow::new([[0., 0., 1., 1.], [100., 100., 101., 101.]]).unwrap();
        assert!(two.reaches(&[100.5, 100.5, 102., 102.]));
        assert!(!two.reaches(&[50., 50., 60., 60.]));
        // the sources reached, each once
        let placements = [
            placed("a.oas", [0., 0., 20., 20.]),
            placed("a.oas", [200., 200., 220., 220.]),
            placed("b.oas", [200., 200., 220., 220.]),
            placed("c.oas", [0., 0., 0., 0.]),
        ];
        assert_eq!(
            w.sources(&placements),
            BTreeSet::from(["a.oas".to_string(), "c.oas".to_string()])
        );
        assert!(DeckReads::default().chips.is_none() && DeckReads::default().window.is_none());
    }
}
