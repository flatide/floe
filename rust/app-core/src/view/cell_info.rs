//! The status line of the cell selected in the cell tree (user 2026-10-15:
//! "셀 트리에서 셀을 선택하면 셀의 관련정보(좌표등)을 상태줄에 표시해줘"):
//! renderd's `cell_info` answer in words - the cell's name; where the view
//! holds it: the one instance's origin, orientation and box, or how many
//! instances and their extent; its size; its children and depth. Coordinates
//! are the view's (the top's, the view root's, a deck's) in microns to the
//! dbu's precision, as the cursor's. The GTK viewer (floe/gui.py
//! `_on_cell_result`) and the web show this text.
use super::CellInfo;

/// Decimals of a micron value on a `dbu` grid: as many as the grid needs,
/// the status line's three at least, six at most (a value's zeros past the
/// third are dropped as it is written).
pub fn decimals(dbu: f64) -> usize {
    (3..=6)
        .find(|d| {
            let v = dbu * 10f64.powi(*d as i32);
            (v - v.round()).abs() <= 1e-6 * v.abs().max(1.)
        })
        .unwrap_or(6)
}

/// `n` with thousands separators, as the panel writes counts.
fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}

/// A coordinate in microns to `decimals`, its trailing zeros past the third
/// decimal dropped (a 0.25 nm grid writes 15960.404, not 15960.40400); a
/// value that rounds to zero carries no sign.
fn um(v: f64, dbu: f64, decimals: usize) -> String {
    let mut text = format!("{:.*}", decimals, v * dbu);
    if let Some(dot) = text.find('.') {
        while text.len() > dot + 4 && text.ends_with('0') {
            text.pop();
        }
    }
    match text.strip_prefix('-') {
        Some(rest) if rest.bytes().all(|c| c == b'0' || c == b'.') => rest.to_string(),
        _ => text,
    }
}

fn point(x: f64, y: f64, dbu: f64, d: usize) -> String {
    format!("({}, {})", um(x, dbu, d), um(y, dbu, d))
}

fn corners(b: &[f64; 4], dbu: f64, d: usize) -> String {
    format!("{} – {} um", point(b[0], b[1], dbu, d), point(b[2], b[3], dbu, d))
}

fn size(b: &[f64; 4], dbu: f64, d: usize) -> String {
    format!("{} × {} um", um(b[2] - b[0], dbu, d), um(b[3] - b[1], dbu, d))
}

/// R0, R90, R180 or R270 and MX for the mirror (about the x axis, before
/// the rotation - the GDS/OASIS order).
pub fn orientation(rot: u8, flip: bool) -> String {
    format!("R{}{}", u32::from(rot % 4) * 90, if flip { " MX" } else { "" })
}

/// The status line for `info` in a view of `dbu` microns per unit; `deck`
/// = the view is a jobdeck's (its sources' tops are its cells).
pub fn status_line(info: &CellInfo, dbu: f64, deck: bool) -> String {
    let d = decimals(dbu);
    // the cell's own box is in its source's units (`unit` per micron)
    let own_dbu = 1. / info.unit;
    let own = decimals(own_dbu);
    let total = info.insts.saturating_mul(info.placements);
    let mut parts = vec![info.name.clone()];
    if info.is_root {
        parts.push(
            match (deck, info.root) {
                (true, _) => "source top cell",
                (false, None) => "top cell",
                (false, Some(_)) => "view root",
            }
            .to_string(),
        );
        if deck && info.placements != 1 {
            parts.push(plural(info.placements, "placement", "placements"));
        }
        match &info.span {
            Some(b) if !deck || info.placements == 1 => {
                parts.push(format!("bbox {}", corners(b, dbu, d)));
                parts.push(size(b, dbu, d));
            }
            Some(b) => {
                parts.push(format!("extent {}", corners(b, dbu, d)));
                if let Some(r) = &info.rbbox {
                    parts.push(format!("cell {}", size(r, own_dbu, own)));
                }
            }
            None => parts.push("no shapes".to_string()),
        }
    } else if total == 0 {
        parts.push(
            match (deck, info.root) {
                (true, _) => "not placed under its source's top cell",
                (false, None) => "not placed under the top cell",
                (false, Some(_)) => "not placed under the view root",
            }
            .to_string(),
        );
        parts.push(match &info.rbbox {
            Some(r) => format!("cell {}", size(r, own_dbu, own)),
            None => "no shapes".to_string(),
        });
    } else if let Some(at) = &info.at {
        parts.push(format!(
            "1 placement at {} {}",
            point(at.x, at.y, dbu, d),
            orientation(at.rot, at.flip)
        ));
        parts.push(format!("bbox {}", corners(&at.bbox, dbu, d)));
        parts.push(size(&at.bbox, dbu, d));
    } else {
        let mut placed = plural(total, "placement", "placements");
        if info.placements > 1 {
            placed += &format!(
                " ({} × {} source placements)",
                count(info.insts),
                count(info.placements)
            );
        }
        parts.push(placed);
        match (&info.rbbox, &info.span) {
            (None, _) | (_, None) => parts.push("no shapes".to_string()),
            (Some(r), Some(b)) => {
                parts.push(if info.approx {
                    format!("within {} (the blocks holding it)", corners(b, dbu, d))
                } else {
                    format!("extent {}", corners(b, dbu, d))
                });
                parts.push(format!("cell {}", size(r, own_dbu, own)));
            }
        }
    }
    if info.children == 0 {
        parts.push("leaf".to_string());
    } else {
        parts.push(plural(info.children, "child", "children"));
        parts.push(format!("{} below", plural(info.height, "level", "levels")));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_worker_client::CellPlacement;

    fn info() -> CellInfo {
        CellInfo {
            source: 0,
            cell: 9,
            root: None,
            name: "BLK".into(),
            height: 2,
            children: 2,
            unit: 1000.,
            rbbox: Some([0., 0., 5000., 4000.]),
            is_root: false,
            insts: 3,
            placements: 1,
            span: Some([1000., 1000., 13000., 11000.]),
            approx: false,
            at: None,
        }
    }

    #[test]
    fn decimals_follow_the_grid_three_to_six() {
        for (dbu, d) in [(0.001, 3), (0.005, 3), (0.0005, 4), (0.0001, 4), (0.00025, 5), (1e-5, 5), (1e-6, 6), (1e-7, 6), (1.0 / 3e3, 6), (1.0, 3)] {
            assert_eq!(decimals(dbu), d, "{dbu}");
        }
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1000), "1,000");
        assert_eq!(count(1234567), "1,234,567");
        assert_eq!(um(-0.0001, 1., 3), "0.000");
        assert_eq!(um(-1., 0.001, 3), "-0.001");
        // a 0.25 nm grid: five decimals where a value needs them, zeros past
        // the third dropped
        assert_eq!(um(72819786., 0.00025, 5), "18204.9465");
        assert_eq!(um(63841616., 0.00025, 5), "15960.404");
        assert_eq!(um(52289335., 0.00025, 5), "13072.33375");
        assert_eq!(um(0., 0.00025, 5), "0.000");
        assert_eq!(um(-1., 0.00025, 5), "-0.00025");
        assert_eq!(um(-0.4, 0.00025, 5), "-0.0001");
        assert_eq!(um(-0.01, 0.00025, 5), "0.000");
        assert_eq!(orientation(0, false), "R0");
        assert_eq!(orientation(3, true), "R270 MX");
    }

    #[test]
    fn many_instances_say_their_count_extent_and_the_cells_size() {
        assert_eq!(
            status_line(&info(), 0.001, false),
            "BLK · 3 placements · extent (1.000, 1.000) – (13.000, 11.000) um · cell 5.000 × 4.000 um · 2 children · 2 levels below"
        );
        // the summary's extent when the walk could not count them all
        let approx = CellInfo { insts: 39, approx: true, children: 0, height: 0, ..info() };
        assert_eq!(
            status_line(&approx, 0.001, false),
            "BLK · 39 placements · within (1.000, 1.000) – (13.000, 11.000) um (the blocks holding it) · cell 5.000 × 4.000 um · leaf"
        );
        // a deck: the source's instances times its placements
        let deck = CellInfo { insts: 1500, placements: 4, ..info() };
        assert_eq!(
            status_line(&deck, 0.0001, true),
            "BLK · 6,000 placements (1,500 × 4 source placements) · extent (0.100, 0.100) – (1.300, 1.100) um · cell 5.000 × 4.000 um · 2 children · 2 levels below"
        );
        // shapeless
        let empty = CellInfo { rbbox: None, span: None, children: 0, ..info() };
        assert_eq!(status_line(&empty, 0.001, false), "BLK · 3 placements · no shapes · leaf");
    }

    #[test]
    fn one_instance_says_its_origin_orientation_and_box() {
        let once = CellInfo {
            insts: 1,
            span: Some([91., 183., 95., 193.]),
            at: Some(CellPlacement { x: 95., y: 193., rot: 3, flip: true, bbox: [91., 183., 95., 193.] }),
            children: 1,
            height: 1,
            ..info()
        };
        assert_eq!(
            status_line(&once, 0.001, false),
            "BLK · 1 placement at (0.095, 0.193) R270 MX · bbox (0.091, 0.183) – (0.095, 0.193) um · 0.004 × 0.010 um · 1 child · 1 level below"
        );
        // negative coordinates keep their sign; a finer grid more decimals
        // where a value needs them
        let west = CellInfo {
            at: Some(CellPlacement { x: -12503., y: 0., rot: 0, flip: false, bbox: [-12503., 0., -7503., 4000.] }),
            ..once
        };
        assert!(status_line(&west, 0.0001, false)
            .starts_with("BLK · 1 placement at (-1.2503, 0.000) R0 · bbox (-1.2503, 0.000) – (-0.7503, 0.400) um · 0.500 × 0.400 um"));
    }

    #[test]
    fn the_root_the_unplaced_and_the_deck_tops() {
        let top = CellInfo {
            name: "TOP".into(),
            is_root: true,
            insts: 1,
            span: Some([0., 0., 20000., 12000.]),
            ..info()
        };
        assert_eq!(
            status_line(&top, 0.001, false),
            "TOP · top cell · bbox (0.000, 0.000) – (20.000, 12.000) um · 20.000 × 12.000 um · 2 children · 2 levels below"
        );
        let root = CellInfo { root: Some(9), ..top.clone() };
        assert!(status_line(&root, 0.001, false).starts_with("TOP · view root · bbox "));
        // a deck source's top: placed once is its box, more is the extent
        assert!(status_line(&top, 0.001, true).starts_with("TOP · source top cell · bbox (0.000, 0.000)"));
        let placed = CellInfo { placements: 3, ..top.clone() };
        assert_eq!(
            status_line(&placed, 0.001, true),
            "TOP · source top cell · 3 placements · extent (0.000, 0.000) – (20.000, 12.000) um · cell 5.000 × 4.000 um · 2 children · 2 levels below"
        );
        let shapeless = CellInfo { span: None, ..top };
        assert!(status_line(&shapeless, 0.001, false).starts_with("TOP · top cell · no shapes"));
        // not under the root
        let orphan = CellInfo { insts: 0, span: None, ..info() };
        assert!(status_line(&orphan, 0.001, false).starts_with("BLK · not placed under the top cell · cell 5.000 × 4.000 um"));
        let outside = CellInfo { root: Some(4), ..orphan.clone() };
        assert!(status_line(&outside, 0.001, false).starts_with("BLK · not placed under the view root · "));
        assert!(status_line(&orphan, 0.001, true).starts_with("BLK · not placed under its source's top cell · "));
        let none = CellInfo { rbbox: None, ..orphan };
        assert!(status_line(&none, 0.001, false).starts_with("BLK · not placed under the top cell · no shapes"));
    }
}
