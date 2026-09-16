//! Bounded display metadata, never raw native/cache JSON or source tooltips.
use floe_app_core::{
    dataset::Dataset,
    view::{Model, Snapshot},
};
use floe_worker_client::{Fill, Layers};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const PAGE: usize = 64;
const SELECTION_LIMIT: usize = 4096;
type Pair = (u32, u32);
mod scoped;
pub(crate) use scoped::{ScopedPage, Visibility};

/// Palette-only state, not a ViewState patch. Folding never hides geometry.
/// A default plus exceptions can fold every group without sending every row.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fold {
    #[serde(default)]
    closed: bool,
    #[serde(default)]
    exceptions: Vec<Pair>,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PaletteRead {
    Page {
        start: usize,
        #[serde(default)]
        fold: Fold,
    },
    Range {
        first: Pair,
        last: Pair,
        #[serde(default)]
        fold: Fold,
    },
}
struct Closed {
    default: bool,
    exceptions: BTreeSet<Pair>,
}
impl Closed {
    fn contains(&self, pair: Pair) -> bool {
        self.default ^ self.exceptions.contains(&pair)
    }
}
struct Row {
    pair: (u32, u32),
    name: String,
    aliases: Vec<String>,
    head: bool,
    parent: Option<(u32, u32)>,
    style: Option<usize>,
    fallback: String,
}
pub(crate) struct LayerCatalog {
    rows: Vec<Row>,
    /// End-exclusive spans; a physical head is its layer's lowest datatype,
    /// not necessarily datatype zero. Hidden deck children are absent here.
    groups: BTreeMap<Pair, usize>,
    physical: bool,
}
fn label(s: &str) -> String {
    s.chars().take(128).collect()
}
impl LayerCatalog {
    pub fn model(model: &Model) -> Self {
        Self::new(
            model
                .styles
                .iter()
                .enumerate()
                .map(|(i, s)| Row {
                    pair: s.layer,
                    name: format!("{}/{}", s.layer.0, s.layer.1),
                    aliases: Vec::new(),
                    head: model.layer_group(s.layer).is_some(),
                    parent: None,
                    style: Some(i),
                    fallback: "#ffffff".into(),
                })
                .collect(),
            model
                .styles
                .iter()
                .all(|s| model.layer_group(s.layer).is_none()),
        )
    }
    pub fn dataset(dataset: &Dataset, model: &Model) -> Self {
        let styles: BTreeMap<_, _> = model
            .styles
            .iter()
            .enumerate()
            .map(|(i, s)| (s.layer, i))
            .collect();
        let rows = match dataset {
            Dataset::Layout(l) => l
                .metadata
                .layers
                .iter()
                .map(|r| Row {
                    pair: r.key(),
                    name: label(&r.name),
                    aliases: r.aliases.iter().take(4).map(|s| label(s)).collect(),
                    head: false,
                    parent: None,
                    style: styles.get(&r.key()).copied(),
                    fallback: r.color.clone(),
                })
                .collect::<Vec<_>>(),
            Dataset::Deck(d) => {
                let heads: BTreeMap<_, _> = d
                    .metadata
                    .layers
                    .iter()
                    .filter(|r| r.jobdeck_head)
                    .map(|r| (r.layer, (r.layer as u32, r.datatype as u32)))
                    .collect();
                d.metadata
                    .layers
                    .iter()
                    .filter(|r| !r.jobdeck_hidden)
                    .map(|r| {
                        let pair = (r.layer as u32, r.datatype as u32);
                        Row {
                            pair,
                            name: label(&r.name),
                            aliases: Vec::new(),
                            head: r.jobdeck_head,
                            parent: if r.jobdeck_head {
                                None
                            } else {
                                heads.get(&r.layer).copied()
                            },
                            style: styles.get(&pair).copied(),
                            fallback: r.color.clone(),
                        }
                    })
                    .collect()
            }
        };
        // A deck in source-layer mode has physical layer/datatype rows too.
        // The presence of synthetic heads, not the file's kind, distinguishes
        // its level/chip palette from ordinary datatype grouping.
        let physical = rows.iter().all(|r| !r.head);
        Self::new(rows, physical)
    }
    fn new(mut rows: Vec<Row>, physical: bool) -> Self {
        rows.sort_by_key(|r| r.pair);
        let mut groups = BTreeMap::new();
        let mut start = 0;
        while start < rows.len() {
            let pair = rows[start].pair;
            let end = start + rows[start..].partition_point(|r| r.pair.0 == pair.0);
            if end > start + 1 && (physical || rows[start].head) {
                groups.insert(pair, end);
                for r in &mut rows[start + 1..end] {
                    r.parent = Some(pair);
                }
            }
            start = end;
        }
        Self {
            rows,
            groups,
            physical,
        }
    }
    fn closed(&self, fold: Fold) -> Result<Closed, &'static str> {
        if fold.exceptions.len() > SELECTION_LIMIT {
            return Err("invalid_palette");
        }
        let mut exceptions = BTreeSet::new();
        for p in fold.exceptions {
            if !self.groups.contains_key(&p) || !exceptions.insert(p) {
                return Err("invalid_palette");
            }
        }
        Ok(Closed {
            default: fold.closed,
            exceptions,
        })
    }
    /// Jump over closed spans before pagination. Neither a huge collapsed
    /// group nor a late page allocates a copy of the full visible order.
    fn visible<'a>(&'a self, closed: &'a Closed, mut i: usize) -> impl Iterator<Item = usize> + 'a {
        std::iter::from_fn(move || {
            let r = self.rows.get(i)?;
            let result = i;
            i = if closed.contains(r.pair) {
                self.groups.get(&r.pair).copied().unwrap_or(i + 1)
            } else {
                i + 1
            };
            Some(result)
        })
    }
    fn page_indices(
        &self,
        start: usize,
        closed: &Closed,
    ) -> Result<(usize, Vec<usize>), &'static str> {
        let mut rows = Vec::with_capacity(PAGE);
        let mut total = 0;
        for i in self.visible(closed, 0) {
            if total >= start && rows.len() < PAGE {
                rows.push(i);
            }
            total += 1;
        }
        if start > total {
            return Err("invalid_palette");
        }
        Ok((total, rows))
    }
    fn range_indices(
        &self,
        first: Pair,
        last: Pair,
        closed: &Closed,
    ) -> Result<Vec<usize>, &'static str> {
        let index = |pair| {
            let i = self
                .rows
                .binary_search_by_key(&pair, |r| r.pair)
                .map_err(|_| "invalid_palette")?;
            if self.rows[i].parent.is_some_and(|p| closed.contains(p)) {
                return Err("palette_anchor_hidden");
            }
            Ok(i)
        };
        let (a, b) = (index(first)?, index(last)?);
        let rows: Vec<_> = self
            .visible(closed, a.min(b))
            .take_while(|i| *i <= a.max(b))
            .take(SELECTION_LIMIT + 1)
            .collect();
        if rows.len() > SELECTION_LIMIT {
            return Err("palette_range_too_large");
        }
        Ok(rows)
    }
    pub fn read(&self, snapshot: &Snapshot, request: PaletteRead) -> Result<Value, &'static str> {
        match request {
            PaletteRead::Page { start, fold } => {
                let closed = self.closed(fold)?;
                let (total, indices) = self.page_indices(start, &closed)?;
                let rows = self.display_rows(snapshot, &indices, Some(&closed));
                let end = start + rows.len();
                Ok(
                    json!({"state_rev":snapshot.state_rev.to_string(),"render_key":snapshot.render_key.to_string(),
                    "total":total,"all_total":self.rows.len(),"start":start,"next":if end<total{Some(end)}else{None},"rows":rows}),
                )
            }
            PaletteRead::Range { first, last, fold } => {
                let closed = self.closed(fold)?;
                let indices = self.range_indices(first, last, &closed)?;
                let pairs: Vec<_> = indices.iter().map(|&i| self.rows[i].pair).collect();
                let groups: Vec<_> = pairs
                    .iter()
                    .filter(|p| self.groups.contains_key(p))
                    .collect();
                Ok(
                    json!({"state_rev":snapshot.state_rev.to_string(),"render_key":snapshot.render_key.to_string(),
                    "pairs":pairs,"groups":groups}),
                )
            }
        }
    }
    pub fn page(&self, snapshot: &Snapshot, start: usize) -> Option<Value> {
        if start > self.rows.len() {
            return None;
        }
        let end = start.saturating_add(PAGE).min(self.rows.len());
        // Preserve the original, unfolded GET schema while clients migrate.
        let rows = self.display_rows(snapshot, &(start..end).collect::<Vec<_>>(), None);
        Some(
            json!({"state_rev":snapshot.state_rev.to_string(),"render_key":snapshot.render_key.to_string(),"total":self.rows.len(),"start":start,"next":if end<self.rows.len(){Some(end)}else{None},"rows":rows}),
        )
    }
    fn display_rows(
        &self,
        snapshot: &Snapshot,
        indices: &[usize],
        closed: Option<&Closed>,
    ) -> Vec<Value> {
        let selected = |pair| match &snapshot.state.layers {
            Layers::All => true,
            Layers::None => false,
            Layers::Only(pairs) => pairs.binary_search(&pair).is_ok(),
        };
        indices
            .iter()
            .map(|&i| {
                let r = &self.rows[i];
                let style = r.style.and_then(|i| snapshot.state.styles.get(i));
                let color = style
                    .map(|s| format!("#{:02x}{:02x}{:02x}", s.color[0], s.color[1], s.color[2]))
                    .unwrap_or_else(|| r.fallback.clone());
                let fill = match style.map(|s| &s.fill) {
                    Some(Fill::Clear) => json!({"kind":"clear"}),
                    Some(Fill::Speckle) => json!({"kind":"speckle"}),
                    Some(Fill::Pattern(rows)) => json!({"kind":"pattern","rows":rows}),
                    _ => json!({"kind":"solid"}),
                };
                // In level mode the actual render rows are hidden in the panel.
                // A selected hidden child still makes its level head visible.
                let visible = selected(r.pair)
                    || (r.head
                        && match &snapshot.state.layers {
                            Layers::Only(pairs) => pairs
                                .get(pairs.partition_point(|p| p.0 < r.pair.0))
                                .is_some_and(|p| p.0 == r.pair.0),
                            _ => false,
                        });
                let mut row = json!({"pair":r.pair,"name":r.name,"aliases":r.aliases,"head":r.head,
                "parent":if closed.is_some() || !self.physical {r.parent} else {None},
                "visible":visible,"color":color,"fill":fill,"width":style.map_or(1,|s|s.width)});
                if let Some(closed) = closed {
                    let children = self.groups.get(&r.pair).map_or(0, |end| end - i - 1);
                    row["head"] = json!(r.head || children > 0);
                    row["children"] = json!(children);
                    row["closed"] = json!(children > 0 && closed.contains(r.pair));
                }
                row
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn catalogue(pairs: Vec<Pair>, physical: bool) -> LayerCatalog {
        LayerCatalog::new(
            pairs
                .into_iter()
                .map(|pair| Row {
                    pair,
                    name: format!("{}/{}", pair.0, pair.1),
                    aliases: vec![],
                    head: !physical && pair.1 == 0,
                    parent: None,
                    style: None,
                    fallback: "#ffffff".into(),
                })
                .collect(),
            physical,
        )
    }
    fn fold(c: &LayerCatalog, closed: bool, exceptions: Vec<Pair>) -> Closed {
        c.closed(Fold { closed, exceptions }).unwrap()
    }
    fn pairs(c: &LayerCatalog, indices: Vec<usize>) -> Vec<Pair> {
        indices.into_iter().map(|i| c.rows[i].pair).collect()
    }

    #[test]
    fn physical_head_is_lowest_datatype_and_does_not_inherit_jobdeck_visibility() {
        let c = catalogue(vec![(9, 0), (3, 300), (3, 2), (3, 1)], true);
        assert_eq!(c.groups, BTreeMap::from([((3, 1), 3)]));
        assert_eq!(c.rows[1].parent, Some((3, 1)));
        assert_eq!(c.rows[2].parent, Some((3, 1)));
        assert!(
            !c.rows[0].head,
            "physical groups are not synthetic render heads"
        );
        assert_eq!(
            pairs(&c, c.page_indices(0, &fold(&c, true, vec![])).unwrap().1),
            vec![(3, 1), (9, 0)]
        );
        assert_eq!(
            c.page_indices(0, &fold(&c, true, vec![(3, 1)])).unwrap().0,
            4
        );
        assert_eq!(
            c.page_indices(0, &fold(&c, false, vec![(3, 1)])).unwrap().0,
            2
        );
    }

    #[test]
    fn closed_groups_are_removed_before_pagination_not_after_it() {
        let c = catalogue(
            (0..150)
                .flat_map(|l| (1..=3).map(move |d| (l, d)))
                .collect(),
            true,
        );
        let closed = fold(&c, true, vec![]);
        for (start, count) in [(0, 64), (64, 64), (128, 22), (150, 0)] {
            let (total, page) = c.page_indices(start, &closed).unwrap();
            assert_eq!(total, 150);
            assert_eq!(page.len(), count);
            assert_eq!(
                pairs(&c, page),
                (start as u32..(start + count) as u32)
                    .map(|l| (l, 1))
                    .collect::<Vec<_>>()
            );
        }
        assert_eq!(c.page_indices(151, &closed).unwrap_err(), "invalid_palette");
        assert_eq!(
            c.page_indices(usize::MAX, &closed).unwrap_err(),
            "invalid_palette"
        );
        let open = fold(&c, false, vec![]);
        assert_eq!(c.page_indices(64, &open).unwrap().0, 450);
        assert_eq!(pairs(&c, c.page_indices(64, &open).unwrap().1)[0], (21, 2));
        let mixed = fold(&c, true, vec![(0, 1), (149, 1)]);
        assert_eq!(c.page_indices(0, &mixed).unwrap().0, 154);
        assert_eq!(
            pairs(&c, c.page_indices(150, &mixed).unwrap().1),
            vec![(148, 1), (149, 1), (149, 2), (149, 3)]
        );
    }

    #[test]
    fn selection_range_crosses_pages_in_either_direction_and_skips_closed_children() {
        let c = catalogue(
            (0..70).flat_map(|l| (0..3).map(move |d| (l, d))).collect(),
            true,
        );
        let open = fold(&c, false, vec![]);
        let expected: Vec<_> = (0..70)
            .flat_map(|l| (0..3).map(move |d| (l, d)))
            .filter(|p| *p >= (2, 1) && *p <= (65, 2))
            .collect();
        for (a, b) in [((2, 1), (65, 2)), ((65, 2), (2, 1))] {
            assert_eq!(pairs(&c, c.range_indices(a, b, &open).unwrap()), expected);
        }
        let closed = fold(&c, true, vec![(2, 0), (65, 0)]);
        let expected = std::iter::once((2, 1))
            .chain(std::iter::once((2, 2)))
            .chain((3..=65).map(|l| (l, 0)))
            .chain([(65, 1), (65, 2)])
            .collect::<Vec<_>>();
        assert_eq!(
            pairs(&c, c.range_indices((65, 2), (2, 1), &closed).unwrap()),
            expected
        );
        assert_eq!(
            c.range_indices((3, 1), (65, 2), &closed).unwrap_err(),
            "palette_anchor_hidden"
        );
        assert_eq!(
            c.range_indices((99, 0), (65, 2), &closed).unwrap_err(),
            "invalid_palette"
        );
        assert_eq!(
            pairs(&c, c.range_indices((3, 0), (3, 0), &closed).unwrap()),
            vec![(3, 0)]
        );
    }

    #[test]
    fn level_only_heads_cannot_expand_and_empty_catalogue_is_valid() {
        // Deck metadata filters jobdeck_hidden before constructing this list.
        let c = catalogue(vec![(1, 0), (2, 0)], false);
        assert!(c.groups.is_empty());
        assert!(c
            .closed(Fold {
                closed: false,
                exceptions: vec![(1, 0)]
            })
            .is_err());
        assert_eq!(c.page_indices(0, &fold(&c, true, vec![])).unwrap().0, 2);
        let chip = catalogue(vec![(1, 0), (1, 1), (1, 2), (2, 0)], false);
        assert!(chip.rows[0].head);
        assert_eq!(chip.rows[1].parent, Some((1, 0)));
        assert_eq!(
            chip.page_indices(0, &fold(&chip, true, vec![])).unwrap().0,
            2
        );
        let empty = catalogue(vec![], true);
        let closed = fold(&empty, true, vec![]);
        assert_eq!(empty.page_indices(0, &closed).unwrap(), (0, vec![]));
        assert_eq!(
            empty.page_indices(1, &closed).unwrap_err(),
            "invalid_palette"
        );
        assert_eq!(
            empty.range_indices((1, 0), (1, 0), &closed).unwrap_err(),
            "invalid_palette"
        );
    }

    #[test]
    fn range_and_fold_limits_fail_atomically_but_huge_collapsed_groups_are_small() {
        let c = catalogue((0..100_000).map(|d| (7, d)).chain([(8, 0)]).collect(), true);
        let open = fold(&c, false, vec![]);
        assert_eq!(
            c.range_indices((7, 0), (7, 4095), &open).unwrap().len(),
            SELECTION_LIMIT
        );
        assert_eq!(
            c.range_indices((7, 0), (7, 4096), &open).unwrap_err(),
            "palette_range_too_large"
        );
        let closed = fold(&c, true, vec![]);
        assert_eq!(
            pairs(&c, c.range_indices((7, 0), (8, 0), &closed).unwrap()),
            vec![(7, 0), (8, 0)]
        );
        assert_eq!(c.page_indices(0, &closed).unwrap(), (2, vec![0, 100_000]));
        for exceptions in [
            vec![(7, 1)],
            vec![(8, 0)],
            vec![(7, 0); 2],
            vec![(7, 0); 4097],
        ] {
            assert!(c
                .closed(Fold {
                    closed: false,
                    exceptions
                })
                .is_err());
        }
        // Read requests never alter a subsequent caller's default order.
        assert_eq!(
            c.page_indices(0, &fold(&c, false, vec![])).unwrap().0,
            100_001
        );
    }

    #[test]
    fn palette_dto_rejects_null_unknown_fields_and_non_integer_pairs() {
        for request in [
            json!({"kind":"page","start":0}),
            json!({"kind":"range","first":[3,1],"last":[9,0],"fold":{"closed":true,"exceptions":[[3,1]]}}),
        ] {
            assert!(serde_json::from_value::<PaletteRead>(request).is_ok());
        }
        for request in [
            json!({"kind":"page","start":0,"fold":null}),
            json!({"kind":"page","start":0,"fold":{"closed":null}}),
            json!({"kind":"page","start":0,"fold":{"exceptions":null}}),
            json!({"kind":"page","start":0,"fold":{"unknown":true}}),
            json!({"kind":"page","start":0,"layers":{"mode":"none"}}),
            json!({"kind":"page","start":-1}),
            json!({"kind":"page","start":0.5}),
            json!({"kind":"range","first":[3,1],"last":[9,0],"start":0}),
            json!({"kind":"range","first":[-1,0],"last":[9,0]}),
            json!({"kind":"range","first":[1,0.5],"last":[9,0]}),
            json!({"kind":"range","first":[1,0,0],"last":[9,0]}),
            json!({"kind":"range","first":[1,0],"last":[4294967296u64,0]}),
            json!({"kind":"range","first":null,"last":[9,0]}),
            json!({"kind":"range","first":[1,0]}),
            json!({"kind":"toggle","first":[1,0],"last":[9,0]}),
        ] {
            assert!(
                serde_json::from_value::<PaletteRead>(request.clone()).is_err(),
                "{request}"
            );
        }
    }

    #[test]
    #[ignore = "run tools/validate_layer_palette.py with the GTK-source order oracle"]
    fn gtk_selectable_order_oracle() {
        #[derive(Deserialize)]
        struct Case {
            pairs: Vec<Pair>,
            heads: BTreeSet<Pair>,
            hidden: BTreeSet<Pair>,
            fold: Fold,
            expected: Vec<Pair>,
        }
        let bytes = std::fs::read(std::env::var_os("FLOE_LAYER_PALETTE_ORACLE").unwrap()).unwrap();
        let cases: Vec<Case> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(cases.len(), 32);
        for case in cases {
            let rows = case
                .pairs
                .iter()
                .filter(|p| !case.hidden.contains(p))
                .map(|&pair| Row {
                    pair,
                    name: String::new(),
                    aliases: vec![],
                    head: case.heads.contains(&pair),
                    parent: None,
                    style: None,
                    fallback: "#ffffff".into(),
                })
                .collect();
            let c = LayerCatalog::new(rows, case.heads.is_empty());
            let closed = c.closed(case.fold).unwrap();
            let mut seen = Vec::new();
            for start in (0..case.expected.len()).step_by(PAGE) {
                let (total, indices) = c.page_indices(start, &closed).unwrap();
                assert_eq!(total, case.expected.len());
                assert!(indices.len() <= PAGE);
                seen.extend(pairs(&c, indices));
            }
            assert_eq!(seen, case.expected);
            let anchors: Vec<_> = [0, 1, 31, 63, 64, case.expected.len() - 1]
                .into_iter()
                .filter(|i| *i < case.expected.len())
                .collect();
            for &a in &anchors {
                for &b in &anchors {
                    let actual = c
                        .range_indices(case.expected[a], case.expected[b], &closed)
                        .unwrap();
                    assert_eq!(pairs(&c, actual), case.expected[a.min(b)..=a.max(b)]);
                }
            }
        }
        println!("GTK PALETTE ORDER: ALL OK (32 source-derived catalogue/fold cases, bounded pages and inclusive ranges)");
    }
}
