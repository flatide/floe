//! Placement uses the measured AD/source-DBU model, not filename heuristics.
//! Selection must never move the deck grid; all sources and extents decide it.
use super::parser::{Entry, JobDeck};
use crate::{check_cancelled, Error, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicUsize;

pub const COORD_MAX: i64 = 1 << 62;
pub const SKIP_REASONS: [&str; 6] = [
    "missing",
    "unreadable",
    "empty_layer",
    "unknown_format",
    "not_indexed",
    "unsupported",
];

#[derive(Clone, Debug, Serialize)]
pub struct Placement {
    pub chip: String,
    pub idx: i64,
    pub row: usize,
    pub tc: String,
    pub ly: i64,
    pub dt: i64,
    pub mag: f64,
    pub dx_um: f64,
    pub dy_um: f64,
    pub ix: i64,
    pub iy: i64,
    pub rx_um: f64,
    pub ry_um: f64,
    pub bbox_um: [f64; 4],
    pub jx: f64,
    pub jy: f64,
}
#[derive(Clone, Debug, Serialize)]
pub struct GridChoice {
    pub candidates: Vec<f64>,
    pub base: f64,
    pub safety: u32,
    pub preferred_dbu: f64,
    pub dbu: f64,
    pub extent_um: Option<f64>,
    pub coord_at_extent: Option<i64>,
    pub coord_limit: i64,
    pub doublings: u32,
    pub why: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct LayerRow {
    pub out: usize,
    pub chip: String,
    pub idx: i64,
    pub ly: i64,
    pub dt: i64,
    pub title: String,
}
#[derive(Clone, Debug)]
pub struct SourceIssue {
    pub reason: String,
    pub detail: String,
    pub stage: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Skipped {
    pub chip: String,
    pub idx: i64,
    pub tc: String,
    pub line: i64,
    pub rows: usize,
    pub reason: String,
    pub detail: String,
    pub stage: String,
    pub anchors: Vec<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ly: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dt: Option<i64>,
}
#[derive(Debug, Serialize)]
pub struct PlanStats {
    pub dbu: f64,
    pub dbu_choice: GridChoice,
    pub instances: usize,
    pub instances_total: u64,
    pub selection: Option<BTreeSet<i64>>,
    pub skipped: Vec<Skipped>,
    pub deck_issues: Vec<Skipped>,
    pub deck_issue_counts: BTreeMap<String, u64>,
    pub skip_counts: BTreeMap<String, u64>,
    pub residual_max_um: f64,
    pub residual_nonzero: usize,
    pub residual_over_half_dbu: usize,
    pub mags: Vec<f64>,
    pub bbox_um: Option<[f64; 4]>,
    pub layer_table: Vec<LayerRow>,
    pub layer_table_by_chip: bool,
}
pub type LayerKey = (String, i64, i64, i64);
#[derive(Debug)]
pub struct PlannedDeck {
    pub placements: Vec<Placement>,
    pub stats: PlanStats,
    out_of: BTreeMap<LayerKey, usize>,
}
impl PlannedDeck {
    pub fn output_layer(&self, chip: &str, idx: i64, ly: i64, dt: i64) -> Option<usize> {
        let chip = if self.stats.layer_table_by_chip {
            chip
        } else {
            ""
        };
        self.out_of.get(&(chip.into(), idx, ly, dt)).copied()
    }
}
#[derive(Clone, Debug)]
pub struct PlanOptions {
    pub safety: u32,
    pub cross: bool,
    pub by_chip: bool,
    pub selected: Option<BTreeSet<i64>>,
    pub skip_missing: bool,
    /// Explicit resource failures, never a successfully truncated deck.
    pub max_placements: usize,
    pub max_pairs: usize,
    pub max_visits: u64,
    pub max_plan_bytes: usize,
}
impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            safety: 2,
            cross: true,
            by_chip: false,
            selected: None,
            skip_missing: false,
            max_placements: 2_000_000,
            max_pairs: 65_536,
            max_visits: 4_000_000,
            max_plan_bytes: 256 * 1024 * 1024,
        }
    }
}

struct PlanBudget(usize);
impl PlanBudget {
    fn charge(&mut self, bytes: usize) -> Result<()> {
        self.0 = self
            .0
            .checked_sub(bytes)
            .ok_or_else(|| Error::input("jobdeck plan memory limit exceeded"))?;
        Ok(())
    }
}

pub fn choose_dbu(
    source_dbus: impl IntoIterator<Item = f64>,
    ads: impl IntoIterator<Item = f64>,
    extent_um: Option<f64>,
    safety: u32,
    coord_max: i64,
) -> Result<GridChoice> {
    if safety == 0 || coord_max <= 0 || extent_um.is_some_and(|e| !e.is_finite() || e < 0.) {
        return Err(Error::input("invalid jobdeck grid bounds"));
    }
    let mut candidates: Vec<_> = source_dbus
        .into_iter()
        .chain(ads)
        .filter(|&v| v > 0. && v.is_finite())
        .collect();
    candidates.sort_by(f64::total_cmp);
    candidates.dedup();
    let base = *candidates
        .first()
        .ok_or_else(|| Error::input("no dbu candidates (no sources and no AD values)"))?;
    let preferred_dbu = base / f64::from(safety);
    if preferred_dbu == 0. || !(1. / preferred_dbu).is_finite() {
        return Err(Error::input("jobdeck grid precision underflows"));
    }
    let mut dbu = preferred_dbu;
    let limit = coord_max as f64 * 0.9;
    let mut doublings = 0;
    if let Some(extent) = extent_um {
        while extent / dbu > limit {
            doublings += 1;
            if doublings > 64 {
                return Err(Error::input(
                    "jobdeck extent cannot fit the coordinate range",
                ));
            }
            dbu *= 2.;
        }
    }
    let mut why =
        "min(source dbu, AD) / safety; AD included because ROWS positions sit on the AD grid"
            .to_string();
    if doublings != 0 {
        why.push_str(&format!(
            "; coarsened {doublings}x to fit the coordinate range"
        ));
    }
    Ok(GridChoice {
        candidates,
        base,
        safety,
        preferred_dbu,
        dbu,
        extent_um,
        coord_at_extent: extent_um.filter(|e| *e != 0.).map(|e| (e / dbu) as i64),
        coord_limit: limit as i64,
        doublings,
        why,
    })
}
pub fn to_grid(um: f64, dbu: f64) -> Result<(i64, f64)> {
    let i = (um / dbu).round_ties_even();
    if !um.is_finite()
        || !dbu.is_finite()
        || dbu <= 0.
        || !i.is_finite()
        || i.abs() > COORD_MAX as f64
    {
        return Err(Error::input("jobdeck coordinate overflows the grid"));
    }
    let i = i as i64;
    Ok((i, um - i as f64 * dbu))
}
pub fn entry_pairs(e: &Entry, cross: bool, max_pairs: usize) -> Result<Vec<(i64, i64)>> {
    let datatypes = if e.dt.is_empty() { &[0][..] } else { &e.dt };
    let count =
        e.ly.len()
            .checked_mul(if cross { datatypes.len() } else { 1 })
            .filter(|&n| n <= max_pairs)
            .ok_or_else(|| Error::input("jobdeck LY/DT pair limit exceeded"))?;
    let mut pairs = Vec::with_capacity(count);
    for (i, &ly) in e.ly.iter().enumerate() {
        if cross {
            pairs.extend(datatypes.iter().map(|&dt| (ly, dt)));
        } else {
            pairs.push((ly, datatypes[i.min(datatypes.len() - 1)]));
        }
    }
    Ok(pairs)
}
fn layer_table(
    deck: &JobDeck,
    options: &PlanOptions,
    cancelled: &AtomicUsize,
    budget: &mut PlanBudget,
) -> Result<Vec<LayerRow>> {
    let mut chips = BTreeMap::new();
    let mut keys = BTreeSet::new();
    let mut visited = 0u64;
    for c in &deck.chips {
        let position = chips.len();
        let position = *chips.entry(&c.id).or_insert(position);
        for e in &c.entries {
            check_cancelled(cancelled)?;
            let pairs = entry_pairs(e, options.cross, options.max_pairs)?;
            visited = visited
                .checked_add(pairs.len() as u64)
                .ok_or_else(|| Error::input("jobdeck layer table visit overflow"))?;
            if visited > options.max_visits {
                return Err(Error::input("jobdeck layer table visit limit exceeded"));
            }
            for (ly, dt) in pairs {
                let key = if options.by_chip {
                    (position, c.id.as_str())
                } else {
                    (0, "")
                };
                keys.insert((key, e.idx, ly, dt));
                if keys.len() > options.max_pairs {
                    return Err(Error::input("jobdeck layer table limit exceeded"));
                }
            }
        }
    }
    keys.into_iter()
        .enumerate()
        .map(|(out, ((_, chip), idx, ly, dt))| {
            // Include the lookup's second copy of the key and conservative
            // container slack. Long names cannot bypass the count limits.
            budget.charge(256 + 2 * chip.len() + deck.title(idx).len())?;
            Ok(LayerRow {
                out,
                chip: chip.into(),
                idx,
                ly,
                dt,
                title: deck.title(idx).into(),
            })
        })
        .collect::<Result<_>>()
}
pub fn skip_counts(skipped: &[Skipped]) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    for s in skipped {
        *out.entry(s.reason.clone()).or_default() += 1;
    }
    out
}

pub fn plan(
    deck: &JobDeck,
    source_dbus: &BTreeMap<String, f64>,
    bad: &BTreeMap<String, SourceIssue>,
    options: &PlanOptions,
    cancelled: &AtomicUsize,
) -> Result<PlannedDeck> {
    check_cancelled(cancelled)?;
    if source_dbus.values().any(|&d| !d.is_finite() || d <= 0.) {
        return Err(Error::input("invalid jobdeck source DBU"));
    }
    // The loop visits entry/ROWS combinations even outside a selection, to
    // preserve the whole-deck grid. Bound its cost before any materialization.
    if deck.instance_count()? > options.max_visits {
        return Err(Error::input("jobdeck placement visit limit exceeded"));
    }
    let mut budget = PlanBudget(options.max_plan_bytes);
    let table = layer_table(deck, options, cancelled, &mut budget)?;
    // Shared tables use the empty CHIP key. Do not allocate the legacy
    // CHIPs x all layers Cartesian lookup just to resolve actual placements.
    let out_of = table
        .iter()
        .map(|r| ((r.chip.clone(), r.idx, r.ly, r.dt), r.out))
        .collect();
    let mut placements = Vec::new();
    let mut skipped = Vec::new();
    let mut deck_issues = Vec::new();
    let mut seen_skip = BTreeSet::new();
    let mut extent: Option<f64> = None;
    let mut total = 0u64;
    for c in &deck.chips {
        // Reuse LY/DT pairs for all ROWS of one entry, without changing order.
        let pairs = c
            .entries
            .iter()
            .map(|e| entry_pairs(e, options.cross, options.max_pairs))
            .collect::<Result<Vec<_>>>()?;
        for (row, &(jy, jx)) in c.rows.iter().enumerate() {
            for (e, pairs) in c.entries.iter().zip(&pairs) {
                check_cancelled(cancelled)?;
                let selected = options
                    .selected
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&e.idx));
                let Some(&source_dbu) = source_dbus.get(&e.tc) else {
                    let issue = bad.get(&e.tc);
                    let reason = issue.map_or("missing", |i| i.reason.as_str());
                    let reason = if SKIP_REASONS.contains(&reason) {
                        reason
                    } else {
                        "missing"
                    };
                    if selected
                        && !options.skip_missing
                        && matches!(reason, "missing" | "unreadable")
                    {
                        return Err(Error::input(format!("no source dbu for {:?}", e.tc)));
                    }
                    if seen_skip.insert((&c.id, e.idx)) {
                        budget.charge(
                            512 + c.id.len()
                                + e.tc.len()
                                + issue.map_or(0, |i| i.detail.len() + i.stage.len())
                                + c.rows.len() * 16,
                        )?;
                        let record = Skipped {
                            chip: c.id.clone(),
                            idx: e.idx,
                            tc: e.tc.clone(),
                            line: e.lineno as i64,
                            rows: c.rows.len(),
                            reason: reason.into(),
                            detail: issue
                                .filter(|i| !i.detail.is_empty())
                                .map_or("source has no dbu", |i| &i.detail)
                                .into(),
                            stage: issue.map_or("probe", |i| &i.stage).into(),
                            anchors: c.rows.iter().map(|&(y, x)| [x, y]).collect(),
                            ly: None,
                            dt: None,
                        };
                        if selected {
                            skipped.push(record);
                        } else {
                            deck_issues.push(record);
                        }
                    }
                    continue;
                };
                let ad =
                    e.ad.ok_or_else(|| Error::input(format!("entry {}: AD missing", e.idx)))?;
                if ad <= 0. || e.sf <= 0. {
                    return Err(Error::input(format!(
                        "entry {}: AD and SF must be positive",
                        e.idx
                    )));
                }
                let ratio = ad / source_dbu;
                let mag = e.sf * ratio;
                // Operation order matches Python. SF scales the source but not
                // the alignment offset (subtract midpoint * ratio, NOT * mag).
                let dx = jx - (e.bx + e.ux) / 2. * ratio;
                let dy = jy - (e.by + e.uy) / 2. * ratio;
                let bbox = [
                    dx + mag * e.bx,
                    dy + mag * e.by,
                    dx + mag * e.ux,
                    dy + mag * e.uy,
                ];
                if !mag.is_finite()
                    || mag <= 0.
                    || !bbox.into_iter().chain([dx, dy]).all(f64::is_finite)
                {
                    return Err(Error::input(format!(
                        "entry {}: placement arithmetic overflow",
                        e.idx
                    )));
                }
                let reach = bbox
                    .into_iter()
                    .chain([dx, dy])
                    .map(f64::abs)
                    .fold(0., f64::max);
                extent = Some(extent.map_or(reach, |v| v.max(reach)));
                total = total
                    .checked_add(pairs.len() as u64)
                    .ok_or_else(|| Error::input("jobdeck instance count overflow"))?;
                if !selected {
                    continue;
                }
                for &(ly, dt) in pairs {
                    if placements.len() >= options.max_placements {
                        return Err(Error::input("jobdeck placement limit exceeded"));
                    }
                    budget
                        .charge(2 * std::mem::size_of::<Placement>() + c.id.len() + e.tc.len())?;
                    placements.push(Placement {
                        chip: c.id.clone(),
                        idx: e.idx,
                        row,
                        tc: e.tc.clone(),
                        ly,
                        dt,
                        mag,
                        dx_um: dx,
                        dy_um: dy,
                        ix: 0,
                        iy: 0,
                        rx_um: 0.,
                        ry_um: 0.,
                        bbox_um: bbox,
                        jx,
                        jy,
                    });
                }
            }
        }
    }
    let choice = choose_dbu(
        source_dbus.values().copied(),
        deck.chips
            .iter()
            .flat_map(|c| &c.entries)
            .filter_map(|e| e.ad),
        extent,
        options.safety,
        COORD_MAX,
    )?;
    let dbu = choice.dbu;
    let mut bbox_um: Option<[f64; 4]> = None;
    let mut mags = Vec::new();
    let (mut residual_max_um, mut residual_nonzero, mut residual_over_half_dbu) = (0f64, 0, 0);
    for p in &mut placements {
        check_cancelled(cancelled)?;
        (p.ix, p.rx_um) = to_grid(p.dx_um, dbu)?;
        (p.iy, p.ry_um) = to_grid(p.dy_um, dbu)?;
        for r in [p.rx_um.abs(), p.ry_um.abs()] {
            residual_max_um = residual_max_um.max(r);
            residual_nonzero += usize::from(r != 0.);
            residual_over_half_dbu += usize::from(r > dbu / 2.);
        }
        mags.push(p.mag);
        let [x0, y0, x1, y1] = p.bbox_um;
        let b = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
        bbox_um = Some(match bbox_um {
            Some(prev) => [
                prev[0].min(b[0]),
                prev[1].min(b[1]),
                prev[2].max(b[2]),
                prev[3].max(b[3]),
            ],
            None => b,
        });
    }
    mags.sort_by(f64::total_cmp);
    mags.dedup();
    let stats = PlanStats {
        dbu,
        dbu_choice: choice,
        instances: placements.len(),
        instances_total: total,
        selection: options.selected.clone(),
        skip_counts: skip_counts(&skipped),
        deck_issue_counts: skip_counts(&deck_issues),
        skipped,
        deck_issues,
        residual_max_um,
        residual_nonzero,
        residual_over_half_dbu,
        mags,
        bbox_um,
        layer_table: table,
        layer_table_by_chip: options.by_chip,
    };
    Ok(PlannedDeck {
        placements,
        stats,
        out_of,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rounding_and_range_are_explicit() {
        assert_eq!(to_grid(0.25, 0.5).unwrap(), (0, 0.25));
        assert_eq!(to_grid(0.75, 0.5).unwrap(), (2, -0.25));
        assert_eq!(to_grid(-0.75, 0.5).unwrap(), (-2, 0.25));
        assert!(to_grid(f64::MAX, 0.001).is_err());
        assert!(choose_dbu([], [], None, 2, COORD_MAX).is_err());
        assert!(choose_dbu([f64::MIN_POSITIVE / 100.], [], None, 2, COORD_MAX).is_err());
        assert_eq!(
            choose_dbu([5e-5], [2e-4], Some(1e6), 2, COORD_MAX)
                .unwrap()
                .doublings,
            0
        );
        assert!(
            choose_dbu([5e-5], [2e-4], Some(1e6), 2, (1 << 31) - 1)
                .unwrap()
                .doublings
                > 0
        );
    }
    #[test]
    fn selection_never_moves_grid_and_limits_never_truncate() {
        let flag = AtomicUsize::new(0);
        let deck = JobDeck::parse("x", "CHIP C\n$ (1,A,AD=0.001,SF=0.5,TC=a,LY=7,UX=4,UY=8)\n$ (2,B,AD=0.0005,TC=b,LY={8,9},DT={0,1},UX=6,UY=2)\nROWS 100/200, 300/400",true,&flag).unwrap();
        let dbus = BTreeMap::from([("a".into(), 0.001), ("b".into(), 0.002)]);
        let base = plan(
            &deck,
            &dbus,
            &BTreeMap::new(),
            &PlanOptions::default(),
            &flag,
        )
        .unwrap();
        let mut options = PlanOptions {
            selected: Some(BTreeSet::from([1])),
            ..Default::default()
        };
        let selected = plan(&deck, &dbus, &BTreeMap::new(), &options, &flag).unwrap();
        assert_eq!(base.stats.dbu, selected.stats.dbu);
        assert_eq!(base.stats.instances_total, 10);
        assert_eq!(selected.stats.instances, 2);
        assert_eq!(selected.placements[0].mag, 0.5);
        assert_eq!(selected.placements[0].dx_um, 198.);
        options.max_placements = 1;
        assert!(plan(&deck, &dbus, &BTreeMap::new(), &options, &flag).is_err());
        options.max_visits = 1;
        assert!(plan(&deck, &dbus, &BTreeMap::new(), &options, &flag).is_err());
        let options = PlanOptions {
            max_plan_bytes: 1,
            ..Default::default()
        };
        assert!(plan(&deck, &dbus, &BTreeMap::new(), &options, &flag)
            .unwrap_err()
            .message
            .contains("memory limit"));
    }
}
