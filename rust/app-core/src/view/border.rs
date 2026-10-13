//! The scale border: the band along the viewport's four edges that reads
//! lengths, as Calibre DESIGNrev's (user 2026-10-13: "viewer의 뷰포트 외곽으로
//! calibre와 같이 길이를 확인할 수 있는 띠를 두르고 기준 길이도 표시해줘.
//! 그리고 좌상단, 좌하단, 우상단, 우하단에 좌표를 표시해줘"). Two tones
//! alternate every reference length: 1, 2 or 5 times a power of ten microns,
//! the smallest that is [`MIN_PX`] or wider on screen. The segments are
//! counted from the origin - their edges are the world's round coordinates,
//! and the band scrolls with the layout - and the length is written once.
//! Each corner of the viewport carries its coordinates in microns, to the
//! pixel's precision (three decimals at least, the status line's; five at
//! most). World maths here; the GTK viewer (floe/gui.py `_draw_border`) and
//! the web draw what this says.
use super::Viewport;
use crate::{Error, Result};
use serde::Serialize;

/// A segment of the band is at least this wide on screen.
pub const MIN_PX: f64 = 60.;
/// The corners' decimals: the status line's three at least, five at most.
const DECIMALS: (i64, i64) = (3, 5);
/// Segments a band can hold: a viewport's 8192 px over MIN_PX and the
/// clipped ends. A step chosen here never needs more; the bound is against a
/// loop over a step that is not.
const SEGMENTS: f64 = 1000.;

/// One segment of the band along an axis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Segment {
    /// Viewport px: x from the left, y from the top.
    pub from: f64,
    pub to: f64,
    /// Its ordinal from the origin is even: the first tone (the neighbours
    /// take the other).
    pub even: bool,
}

/// A corner's coordinates in microns, written.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Corner {
    pub x: String,
    pub y: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Border {
    /// The reference length: 1, 2 or 5 times a power of ten microns.
    pub step_um: f64,
    /// Its width on screen (the coarser axis's when they differ).
    pub step_px: f64,
    /// The reference length written: "10 um", "0.5 um".
    pub label: String,
    /// The decimals the corners are written with.
    pub decimals: usize,
    /// Along the top and bottom bands, left to right.
    pub x: Vec<Segment>,
    /// Along the left and right bands, top to bottom.
    pub y: Vec<Segment>,
    pub top_left: Corner,
    pub top_right: Corner,
    pub bottom_left: Corner,
    pub bottom_right: Corner,
}

impl Border {
    /// The border of the viewport `v` (dbu) of a layout with `dbu` microns
    /// per unit.
    pub fn new(v: Viewport, dbu: f64) -> Result<Self> {
        v.validate()?;
        if !dbu.is_finite() || dbu <= 0. {
            return Err(Error::input("border requires a positive DBU"));
        }
        let [x0, y0, x1, y1] = v.bbox;
        let (w, h) = (f64::from(v.width), f64::from(v.height));
        // dbu per px, per axis (the controller keeps them equal; a box of
        // the web's may not: the step serves the coarser axis, the corners
        // say the finer one's precision)
        let (sx, sy) = ((x1 - x0) / w, (y1 - y0) / h);
        let (step_um, step_decimals) = step(sx.max(sy) * dbu)?;
        let step_dbu = step_um / dbu;
        let decimals = (-(sx.min(sy) * dbu).log10())
            .ceil()
            .clamp(DECIMALS.0 as f64, DECIMALS.1 as f64) as usize;
        let corner = |x: f64, y: f64| Corner {
            x: um_text(x * dbu, decimals),
            y: um_text(y * dbu, decimals),
        };
        Ok(Self {
            step_um,
            step_px: step_dbu / sx.max(sy),
            label: format!("{:.*} um", step_decimals, step_um),
            decimals,
            x: segments(x0, x1, step_dbu, w, |a| (a - x0) / sx)?,
            y: segments(y0, y1, step_dbu, h, |a| (y1 - a) / sy)?,
            top_left: corner(x0, y1),
            top_right: corner(x1, y1),
            bottom_left: corner(x0, y0),
            bottom_right: corner(x1, y0),
        })
    }
}

/// The smallest 1, 2 or 5 times a power of ten microns that is MIN_PX wide
/// at `um_px` microns per pixel, and the decimals that write it.
fn step(um_px: f64) -> Result<(f64, usize)> {
    let least = um_px * MIN_PX;
    if !least.is_finite() || least <= 0. {
        return Err(Error::input("border requires a finite positive scale"));
    }
    let mut exponent = least.log10().floor() as i32;
    for _ in 0..3 {
        for mantissa in [1., 2., 5.] {
            let length = mantissa * 10f64.powi(exponent);
            // a length short of MIN_PX by float noise alone serves
            if length * (1. + 1e-9) >= least {
                return Ok((length, exponent.min(0).unsigned_abs() as usize));
            }
        }
        exponent += 1;
    }
    Err(Error::input("border: no step serves this scale"))
}

/// The segments of the band between world `lo` and `hi` (dbu) - every
/// `step` from the origin, the ends clipped to the axis's `len` px -
/// ordered along the axis; `px` maps a world coordinate to the axis.
fn segments(
    lo: f64,
    hi: f64,
    step: f64,
    len: f64,
    px: impl Fn(f64) -> f64,
) -> Result<Vec<Segment>> {
    let (first, last) = ((lo / step).floor(), (hi / step).floor());
    if !(last - first).is_finite() || last - first + 1. > SEGMENTS {
        return Err(Error::input("border: the step is too fine for the view"));
    }
    let mut out = Vec::with_capacity((last - first) as usize + 1);
    let mut k = first;
    while k <= last {
        let (a, b) = (px(k * step), px((k + 1.) * step));
        let (from, to) = (a.min(b).max(0.), a.max(b).min(len));
        if to > from {
            out.push(Segment {
                from,
                to,
                even: k.rem_euclid(2.) == 0.,
            });
        }
        k += 1.;
    }
    out.sort_by(|a, b| a.from.total_cmp(&b.from));
    Ok(out)
}

/// `um` with `decimals`; a value that rounds to zero carries no sign.
fn um_text(um: f64, decimals: usize) -> String {
    let text = format!("{um:.decimals$}");
    match text.strip_prefix('-') {
        Some(rest) if rest.bytes().all(|c| c == b'0' || c == b'.') => rest.to_string(),
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn border(bbox: [f64; 4], width: u32, height: u32, dbu: f64) -> Border {
        Border::new(Viewport::new(bbox, width, height).unwrap(), dbu).unwrap()
    }

    #[test]
    fn the_step_is_the_smallest_round_length_sixty_px_wide() {
        // 1000 px over the width given (um): the microns per pixel times
        // MIN_PX, rounded up to 1, 2 or 5 times a power of ten
        for (width_um, step_um, label) in [
            (100., 10., "10 um"),
            (50., 5., "5 um"),
            (30., 2., "2 um"),
            (10., 1., "1 um"),
            (1., 0.1, "0.1 um"),
            (0.5, 0.05, "0.05 um"),
            (0.03, 0.002, "0.002 um"),
            (1000., 100., "100 um"),
            (2000., 200., "200 um"),
            (60000., 5000., "5000 um"),
        ] {
            let b = border([0., 0., width_um * 1000., width_um * 600.], 1000, 600, 0.001);
            assert_eq!(b.step_um, step_um, "{width_um} um wide");
            assert_eq!(b.label, label, "{width_um} um wide");
            assert!(b.step_px >= MIN_PX, "{width_um} um wide: {} px", b.step_px);
            assert!(b.step_px <= MIN_PX * 2.5, "{width_um} um wide: {} px", b.step_px);
            for s in b.x.iter().chain(&b.y) {
                assert!(s.to > s.from && s.from >= 0.);
            }
        }
        // exactly MIN_PX wide is wide enough: 1 um per 60 px
        let b = border([0., 0., 1000. / 60. * 1000., 1000. / 60. * 600.], 1000, 600, 0.001);
        assert_eq!(b.step_um, 1.);
        assert!((b.step_px - 60.).abs() < 1e-9);
    }

    #[test]
    fn segments_alternate_from_the_origin_and_clip_to_the_viewport() {
        // dbu 1 um: 0.1 um per px, the step 10 um = 100 px
        let b = border([-25., -15., 75., 45.], 1000, 600, 1.);
        assert_eq!((b.step_um, b.step_px), (10., 100.));
        // x: the segment holding -25 (ordinal -3, odd) is clipped to 50 px,
        // then full ones, the last (ordinal 7) clipped at the right edge
        let xs: Vec<_> = b.x.iter().map(|s| (s.from, s.to, s.even)).collect();
        assert_eq!(xs.len(), 11);
        assert_eq!(xs[0], (0., 50., false));
        assert_eq!(xs[1], (50., 150., true));
        assert_eq!(xs[10], (950., 1000., false));
        for pair in b.x.windows(2) {
            assert_eq!(pair[0].to, pair[1].from, "no gap, no overlap");
            assert_ne!(pair[0].even, pair[1].even, "the tones alternate");
        }
        for s in &b.x[1..10] {
            assert_eq!(s.to - s.from, 100.);
        }
        // y: top to bottom - the top segment holds the world's 45 (ordinal
        // 4, even), clipped to 50 px; the bottom one the world's -15
        let ys: Vec<_> = b.y.iter().map(|s| (s.from, s.to, s.even)).collect();
        assert_eq!(ys.len(), 7);
        assert_eq!(ys[0], (0., 50., true));
        assert_eq!(ys[1], (50., 150., false));
        assert_eq!(ys[6], (550., 600., true));
        // a view edge on a step's edge: no empty segment
        let b = border([0., 0., 1000., 600.], 1000, 600, 1.);
        assert_eq!(b.x.len(), 10);
        assert_eq!((b.x[0].from, b.x[0].to, b.x[0].even), (0., 100., true));
        assert_eq!((b.y[0].from, b.y[0].to, b.y[0].even), (0., 100., false));
        assert_eq!(b.y[5].to, 600.);
        // far from the origin the ordinals still alternate
        let b = border([1e15 + 50., 1e15 + 50., 1e15 + 1050., 1e15 + 650.], 1000, 600, 0.001);
        assert_eq!(b.x.len(), 11);
        assert_eq!((b.x[0].from, b.x[0].to), (0., 50.));
        for pair in b.x.windows(2) {
            assert_ne!(pair[0].even, pair[1].even);
        }
    }

    #[test]
    fn corners_say_the_viewport_in_um_to_the_pixel() {
        // 0.1 um per px: three decimals (the floor); the top's y and the
        // bottom's; a value that rounds to zero has no sign
        let b = border([-12345., -0.1, 87655., 59999.9], 1000, 600, 0.001);
        assert_eq!(b.decimals, 3);
        assert_eq!(b.top_left, Corner { x: "-12.345".into(), y: "60.000".into() });
        assert_eq!(b.top_right, Corner { x: "87.655".into(), y: "60.000".into() });
        assert_eq!(b.bottom_left, Corner { x: "-12.345".into(), y: "0.000".into() });
        assert_eq!(b.bottom_right, Corner { x: "87.655".into(), y: "0.000".into() });
        // finer views write more, five decimals at most
        for (width_dbu, decimals) in [(1000., 3), (100., 4), (10., 5), (1., 5), (1e6, 3)] {
            let b = border([0., 0., width_dbu, width_dbu * 0.6], 1000, 600, 0.001);
            assert_eq!(b.decimals, decimals, "{width_dbu} dbu wide");
        }
        let b = border([0.5, 0.3, 100.5, 60.3], 1000, 600, 0.001);
        assert_eq!(b.decimals, 4);
        assert_eq!(b.bottom_left, Corner { x: "0.0005".into(), y: "0.0003".into() });
    }

    #[test]
    fn the_step_serves_the_coarser_axis_of_an_uneven_box() {
        // the web may ask for a box that is not the viewport's shape: 1 dbu
        // per px across, 0.1 down (dbu 1 um)
        let b = border([0., 0., 1000., 100.], 1000, 1000, 1.);
        assert_eq!((b.step_um, b.step_px), (100., 100.));
        assert_eq!(b.x.len(), 10);
        assert_eq!(b.y.len(), 1);
        assert_eq!((b.y[0].from, b.y[0].to), (0., 1000.));
        assert_eq!(b.decimals, 3);
    }

    #[test]
    fn the_wire_names_and_the_refusals() {
        let b = border([0., 0., 100_000., 60_000.], 1000, 600, 0.001);
        let v = serde_json::to_value(&b).unwrap();
        for key in [
            "step_um",
            "step_px",
            "label",
            "decimals",
            "x",
            "y",
            "top_left",
            "top_right",
            "bottom_left",
            "bottom_right",
        ] {
            assert!(v.get(key).is_some(), "{key}");
        }
        assert_eq!(v["x"][0]["from"], 0.);
        assert_eq!(v["x"][0]["even"], true);
        assert_eq!(v["top_left"]["x"], "0.000");
        assert_eq!(v["label"], "10 um");
        let viewport = Viewport::new([0., 0., 1000., 600.], 1000, 600).unwrap();
        for dbu in [0., -1., f64::NAN, f64::INFINITY] {
            assert!(Border::new(viewport, dbu).is_err(), "dbu {dbu}");
        }
        assert!(Viewport::new([0., 0., 1000., 600.], 0, 600).is_err());
        // the widest view the viewport allows still has its band
        let edge = (1u64 << 62) as f64;
        let b = border([-edge, -edge, edge, edge], 100, 100, 0.001);
        assert!(b.x.len() >= 2 && b.step_px >= MIN_PX);
    }
}
