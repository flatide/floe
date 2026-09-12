//! Pixel-aligned reuse, independent of transport or image format.
use super::Viewport;
use crate::shots::MAX_PIXELS;

/// Top-left source pixel of a viewport in a retained frame. Both axes must
/// preserve the native 16x16 fill phase; subpixel reuse is never resampled.
pub fn origin(frame: Viewport, view: Viewport) -> Option<[i32; 2]> {
    let fs = [
        (frame.bbox[2] - frame.bbox[0]) / f64::from(frame.width),
        (frame.bbox[3] - frame.bbox[1]) / f64::from(frame.height),
    ];
    let vs = [
        (view.bbox[2] - view.bbox[0]) / f64::from(view.width),
        (view.bbox[3] - view.bbox[1]) / f64::from(view.height),
    ];
    if fs
        .iter()
        .zip(vs)
        .any(|(a, b)| !a.is_finite() || *a <= 0. || (b / a - 1.).abs() > 1e-9)
    {
        return None;
    }
    let offsets = [
        (view.bbox[0] - frame.bbox[0]) / fs[0],
        (frame.bbox[3] - view.bbox[3]) / fs[1],
    ];
    let mut out = [0; 2];
    for (i, n) in offsets.into_iter().enumerate() {
        let integer = n.round_ties_even();
        if !n.is_finite()
            || (n - integer).abs() > 1e-3
            || integer < f64::from(i32::MIN)
            || integer > f64::from(i32::MAX)
        {
            return None;
        }
        out[i] = integer as i32;
        if out[i] % 16 != 0 {
            return None;
        }
    }
    Some(out)
}
pub fn covers(frame: Viewport, view: Viewport) -> bool {
    origin(frame, view).is_some_and(|[x, y]| {
        x >= 0
            && y >= 0
            && i64::from(x) + i64::from(view.width) <= i64::from(frame.width)
            && i64::from(y) + i64::from(view.height) <= i64::from(frame.height)
    })
}
pub fn comfortable(frame: Viewport, view: Viewport) -> bool {
    let Some([x, y]) = origin(frame, view) else {
        return false;
    };
    let ext = [
        (f64::from(frame.width) - f64::from(view.width)) / 2.,
        (f64::from(frame.height) - f64::from(view.height)) / 2.,
    ];
    ext.iter().any(|v| *v >= 16.)
        && [
            (
                f64::from(x),
                f64::from(frame.width) - f64::from(view.width) - f64::from(x),
                ext[0],
            ),
            (
                f64::from(y),
                f64::from(frame.height) - f64::from(view.height) - f64::from(y),
                ext[1],
            ),
        ]
        .into_iter()
        .all(|(lo, hi, e)| e < 16. || (lo >= 0.7 * e && hi >= 0.7 * e))
}
/// One snapped half-viewport per side, proportionally reduced under native
/// 8192/axis + 16 Mpx limits. Failure near the world boundary skips prefetch.
pub fn grow(view: Viewport) -> Option<Viewport> {
    view.validate().ok()?;
    let wanted = [
        ((f64::from(view.width) / 32.).round_ties_even() as u32 * 16)
            .min((8192 - view.width) / 32 * 16),
        ((f64::from(view.height) / 32.).round_ties_even() as u32 * 16)
            .min((8192 - view.height) / 32 * 16),
    ];
    let fits = |x: u32, y: u32| {
        u64::from(view.width + 2 * x) * u64::from(view.height + 2 * y) <= MAX_PIXELS
    };
    let mut ext = wanted;
    if !fits(ext[0], ext[1]) {
        // Fixed iterations, floor to a period: no cancellation-prone quadratic.
        let (mut lo, mut hi) = (0., 1.);
        for _ in 0..48 {
            let mid = (lo + hi) / 2.;
            let x = (f64::from(wanted[0]) * mid / 16.).floor() as u32 * 16;
            let y = (f64::from(wanted[1]) * mid / 16.).floor() as u32 * 16;
            if fits(x, y) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        ext = wanted.map(|v| (f64::from(v) * lo / 16.).floor() as u32 * 16);
    }
    if ext == [0, 0] {
        return None;
    }
    let dx = (view.bbox[2] - view.bbox[0]) / f64::from(view.width) * f64::from(ext[0]);
    let dy = (view.bbox[3] - view.bbox[1]) / f64::from(view.height) * f64::from(ext[1]);
    let frame = Viewport::new(
        [
            view.bbox[0] - dx,
            view.bbox[1] - dy,
            view.bbox[2] + dx,
            view.bbox[3] + dy,
        ],
        view.width + 2 * ext[0],
        view.height + 2 * ext[1],
    )
    .ok()?;
    (origin(frame, view) == Some([ext[0] as i32, ext[1] as i32])).then_some(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Navigation;
    #[test]
    fn all_arrow_landings_keep_phase_and_half_dbu() {
        for (w, h) in [(1001, 733), (800, 600), (2312, 1651)] {
            let v = Viewport::new(
                [-10.9375, 0.0625, w as f64 - 10.9375, h as f64 + 0.0625],
                w,
                h,
            )
            .unwrap();
            let f = grow(v).unwrap();
            assert!(comfortable(f, v));
            for delta in [0.1, 0.5, -0.1, -0.5] {
                for (x, y) in [(delta, 0.), (0., delta)] {
                    let p = v
                        .navigate(Navigation::Pan { x, y, snap: true }, v.bbox, 1.)
                        .unwrap();
                    assert!(covers(f, p), "{w} {h} {x} {y}");
                }
            }
            let off = v
                .navigate(
                    Navigation::Pan {
                        x: 1. / w as f64,
                        y: 0.,
                        snap: false,
                    },
                    v.bbox,
                    1.,
                )
                .unwrap();
            assert_eq!(origin(f, off), None);
            let zoom = v
                .navigate(
                    Navigation::Zoom {
                        factor: 0.8,
                        anchor: [0.5, 0.5],
                    },
                    v.bbox,
                    1.,
                )
                .unwrap();
            assert_eq!(origin(f, zoom), None);
        }
    }
    #[test]
    fn cap_and_world_limits_never_overgrow_or_resample() {
        for (w, h) in [(3840, 2160), (4096, 4096), (8192, 2000), (1, 8192), (1, 1)] {
            let v = Viewport::new([0., 0., w as f64, h as f64], w, h).unwrap();
            if let Some(f) = grow(v) {
                f.validate().unwrap();
                assert!(covers(f, v));
                assert!(comfortable(f, v));
                assert!(f.width > w || f.height > h);
            }
        }
        assert!(grow(Viewport::new([0., 0., 4096., 4096.], 4096, 4096).unwrap()).is_none());
        let end = (1u64 << 62) as f64;
        assert!(grow(Viewport::new([end - 8192., 0., end, 8192.], 1024, 1024).unwrap()).is_none());
    }
}
