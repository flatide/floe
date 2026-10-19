//! Pure integer geometry shared by the record parsers, the inventory
//! pass and the indexer: repetition extents, the path outline box and
//! the orthogonal placement transform's effect on a box. These used to
//! live in floe-tiler (which re-exports them unchanged); the inventory
//! pass (inventory.rs) must use the very same functions as the build's
//! Doc pre-passes so that its boxes are byte-identical to theirs.

use crate::doc::Rep;

/// (x0, y0, x1, y1), closed on both ends; `EMPTY_BOX` when nothing
/// has been added (the ovm BBox sentinel)
pub type Box4 = (i64, i64, i64, i64);

pub const EMPTY_BOX: Box4 = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);

#[inline]
pub fn box_is_empty(b: &Box4) -> bool {
    b.2 < b.0 || b.3 < b.1
}

/// grow `a` by `b` (an empty `b` adds nothing - ovm BBox::grow)
#[inline]
pub fn box_grow(a: &mut Box4, b: &Box4) {
    if box_is_empty(b) {
        return;
    }
    a.0 = a.0.min(b.0);
    a.1 = a.1.min(b.1);
    a.2 = a.2.max(b.2);
    a.3 = a.3.max(b.3);
}

/// bounding box of a non-empty point list
pub fn pts_box(pts: &[(i64, i64)]) -> Box4 {
    let mut b = EMPTY_BOX;
    for &(x, y) in pts {
        b.0 = b.0.min(x);
        b.1 = b.1.min(y);
        b.2 = b.2.max(x);
        b.3 = b.3.max(y);
    }
    b
}

/// offset extents of a grid repetition `na x nb` at steps `va`/`vb`:
/// ((min_x, max_x), (min_y, max_y)) over the four corner members
/// (`rep_extent`'s Grid arm; the scan calls it with the parameters the
/// geometry parser would build the Rep from)
pub fn grid_extent(
    na: u64,
    nb: u64,
    va: (i64, i64),
    vb: (i64, i64),
) -> ((i64, i64), (i64, i64)) {
    let (na, nb) = (na as i64 - 1, nb as i64 - 1);
    let xs = [0, va.0 * na, vb.0 * nb, va.0 * na + vb.0 * nb];
    let ys = [0, va.1 * na, vb.1 * nb, va.1 * na + vb.1 * nb];
    (
        (*xs.iter().min().unwrap(), *xs.iter().max().unwrap()),
        (*ys.iter().min().unwrap(), *ys.iter().max().unwrap()),
    )
}

/// min/max offset extents of a repetition: ((min_x, max_x), (min_y, max_y))
pub fn rep_extent(rep: &Rep) -> ((i64, i64), (i64, i64)) {
    match rep {
        Rep::One => ((0, 0), (0, 0)),
        Rep::Grid { na, nb, va, vb } => grid_extent(*na, *nb, *va, *vb),
        Rep::Pts(p) => {
            // single allocation-free pass: the routing pre-cull calls
            // this per (record, window)
            let (mut x0, mut x1, mut y0, mut y1) =
                (i64::MAX, i64::MIN, i64::MAX, i64::MIN);
            for &(x, y) in p.iter() {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
            ((x0, x1), (y0, y1))
        }
    }
}

/// Exact outline polygon of a MANHATTAN path (klayout Path.polygon
/// parity: square miter joins - corner point v + (nA+nB)*hw on each
/// side - and flush ends after applying the extensions). None when a
/// segment is non-manhattan or the spine degenerates (single point,
/// U-turn join); callers fall back or error there.
pub fn path_outline(
    pts: &[(i64, i64)],
    hw: i64,
    es: i64,
    ee: i64,
) -> Option<Vec<(i64, i64)>> {
    // dedupe + merge colinear runs so every join is a 90-degree turn
    let mut sp: Vec<(i64, i64)> = Vec::with_capacity(pts.len());
    for &q in pts {
        if sp.last() == Some(&q) {
            continue;
        }
        if sp.len() >= 2 {
            let a = sp[sp.len() - 2];
            let b = sp[sp.len() - 1];
            let d1 = ((b.0 - a.0).signum(), (b.1 - a.1).signum());
            let d2 = ((q.0 - b.0).signum(), (q.1 - b.1).signum());
            if d1 == d2 {
                sp.pop();
            }
        }
        sp.push(q);
    }
    if sp.len() < 2 {
        return None;
    }
    let mut dirs: Vec<(i64, i64)> = Vec::with_capacity(sp.len() - 1);
    for w in sp.windows(2) {
        let (dx, dy) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
        if dx != 0 && dy != 0 {
            return None; // non-manhattan
        }
        dirs.push((dx.signum(), dy.signum()));
    }
    for w in dirs.windows(2) {
        if w[0].0 == -w[1].0 && w[0].1 == -w[1].1 {
            return None; // U-turn join: miter undefined
        }
    }
    let last = sp.len() - 1;
    sp[0] = (sp[0].0 - dirs[0].0 * es, sp[0].1 - dirs[0].1 * es);
    sp[last] = (
        sp[last].0 + dirs[last - 1].0 * ee,
        sp[last].1 + dirs[last - 1].1 * ee,
    );
    let n = |d: (i64, i64)| (-d.1, d.0); // left normal
    let mut left: Vec<(i64, i64)> = Vec::with_capacity(sp.len() * 2);
    let mut right: Vec<(i64, i64)> = Vec::with_capacity(sp.len());
    let n0 = n(dirs[0]);
    left.push((sp[0].0 + n0.0 * hw, sp[0].1 + n0.1 * hw));
    right.push((sp[0].0 - n0.0 * hw, sp[0].1 - n0.1 * hw));
    for i in 1..last {
        let na = n(dirs[i - 1]);
        let nb = n(dirs[i]);
        let m = (na.0 + nb.0, na.1 + nb.1);
        left.push((sp[i].0 + m.0 * hw, sp[i].1 + m.1 * hw));
        right.push((sp[i].0 - m.0 * hw, sp[i].1 - m.1 * hw));
    }
    let nl = n(dirs[last - 1]);
    left.push((sp[last].0 + nl.0 * hw, sp[last].1 + nl.1 * hw));
    right.push((sp[last].0 - nl.0 * hw, sp[last].1 - nl.1 * hw));
    right.reverse();
    left.extend(right);
    Some(left)
}

/// path bbox: exact (outline) for manhattan spines, conservative
/// (extended vertices grown by hw on both axes) otherwise
pub fn path_bbox(
    pts: &[(i64, i64)],
    hw: i64,
    es: i64,
    ee: i64,
) -> (i64, i64, i64, i64) {
    if let Some(o) = path_outline(pts, hw, es, ee) {
        let (mut x0, mut y0, mut x1, mut y1) =
            (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for &(x, y) in &o {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        return (x0, y0, x1, y1);
    }
    let (mut x0, mut y0, mut x1, mut y1) =
        (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    let last = pts.len() - 1;
    for (i, &(px, py)) in pts.iter().enumerate() {
        let (mut ex, mut ey) = (px, py);
        if i == 0 && pts.len() > 1 {
            let d = ((pts[1].0 - px).signum(), (pts[1].1 - py).signum());
            ex -= d.0 * es;
            ey -= d.1 * es;
        }
        if i == last && pts.len() > 1 {
            let d = (
                (px - pts[last - 1].0).signum(),
                (py - pts[last - 1].1).signum(),
            );
            ex += d.0 * ee;
            ey += d.1 * ee;
        }
        for &(vx, vy) in &[(px, py), (ex, ey)] {
            x0 = x0.min(vx - hw);
            y0 = y0.min(vy - hw);
            x1 = x1.max(vx + hw);
            y1 = y1.max(vy + hw);
        }
    }
    (x0, y0, x1, y1)
}

/// The linear part of a placement transform (tiler `Xf::place(x, y,
/// rot, flip)` without its translation: flip = mirror about the x
/// axis, applied before the rotation `rot` x 90 degrees CCW) applied
/// to a box: the images of two opposite corners, min/max'ed - exactly
/// what the Doc's recursive bbox does per placement before adding the
/// placement offset and the repetition extents.
pub fn orient_box(rot: u8, flip: bool, b: Box4) -> Box4 {
    let f = if flip { -1 } else { 1 };
    let (c, s) = match rot & 3 {
        0 => (1, 0),
        1 => (0, 1),
        2 => (-1, 0),
        _ => (0, -1),
    };
    let m = [[c, -s * f], [s, c * f]];
    let ap = |x: i64, y: i64| (m[0][0] * x + m[0][1] * y, m[1][0] * x + m[1][1] * y);
    let (a, bb) = (ap(b.0, b.1), ap(b.2, b.3));
    (a.0.min(bb.0), a.1.min(bb.1), a.0.max(bb.0), a.1.max(bb.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_extent_matches_rep_extent() {
        let g = Rep::Grid { na: 3, nb: 2, va: (10, 0), vb: (0, -7) };
        assert_eq!(rep_extent(&g), grid_extent(3, 2, (10, 0), (0, -7)));
        assert_eq!(rep_extent(&g), ((0, 20), (-7, 0)));
        let p = Rep::Pts(vec![(0, 0), (5, -3), (-2, 4)].into());
        assert_eq!(rep_extent(&p), ((-2, 5), (-3, 4)));
    }

    #[test]
    fn orient_box_eight_orientations() {
        // the unit square's corner images under M = R(rot) * F(flip)
        let b = (1, 2, 4, 8);
        assert_eq!(orient_box(0, false, b), (1, 2, 4, 8));
        assert_eq!(orient_box(1, false, b), (-8, 1, -2, 4));
        assert_eq!(orient_box(2, false, b), (-4, -8, -1, -2));
        assert_eq!(orient_box(3, false, b), (2, -4, 8, -1));
        assert_eq!(orient_box(0, true, b), (1, -8, 4, -2));
        assert_eq!(orient_box(1, true, b), (2, 1, 8, 4));
        assert_eq!(orient_box(2, true, b), (-4, 2, -1, 8));
        assert_eq!(orient_box(3, true, b), (-8, -4, -2, -1));
        // empty stays empty-shaped only through callers' checks: an
        // oriented EMPTY_BOX is meaningless, so callers test first
        assert!(box_is_empty(&EMPTY_BOX));
        let mut a = EMPTY_BOX;
        box_grow(&mut a, &(3, 4, 5, 6));
        box_grow(&mut a, &EMPTY_BOX);
        assert_eq!(a, (3, 4, 5, 6));
    }
}
