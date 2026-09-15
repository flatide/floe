//! Session-scoped property assignments. A resolved child bitmap alone cannot
//! tell whether it inherited its head, so keep the sparse source of truth.
use super::*;
use crate::{layerprops, styles};
use floe_worker_client::Fill;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Assignments {
    pub fills: BTreeMap<(u32, u32), AssignedFill>,
    pub widths: BTreeMap<(u32, u32), u8>,
    pub slots: BTreeMap<&'static str, [u16; 16]>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AssignedFill {
    Direct(Fill),
    Slot(&'static str),
}
impl Assignments {
    pub fn slot_rows(&self, name: &'static str) -> [u16; 16] {
        self.slots.get(name).copied().unwrap_or_else(|| {
            let Some(Fill::Pattern(rows)) = styles::pattern(name) else {
                unreachable!("slot identity comes from the bundled table")
            };
            rows
        })
    }
    pub fn resolve(&self, fill: &AssignedFill) -> Fill {
        match fill {
            AssignedFill::Direct(fill) => fill.clone(),
            AssignedFill::Slot(name) => Fill::Pattern(self.slot_rows(name)),
        }
    }
    pub fn initial(props: &[styles::LayerProps], pairs: &BTreeSet<(u32, u32)>) -> Self {
        let mut out = Self::default();
        for p in props.iter().filter(|p| pairs.contains(&p.layer)) {
            if let Some(name) = styles::pattern_slot(&p.fill) {
                out.fills.insert(p.layer, AssignedFill::Slot(name));
            }
            // Startup retains the last >1 assignment. Live import clears it
            // for <=1, matching the separate GTK startup/load policies.
            if p.width > 1 {
                out.widths.insert(p.layer, p.width);
            }
        }
        out
    }
}
impl ViewState {
    pub(super) fn apply_style_deltas(
        &mut self,
        model: &Model,
        changes: Vec<StyleDelta>,
    ) -> Result<()> {
        if changes.len() > 4096 {
            return Err(Error::input("too many style changes"));
        }
        let mut unique = BTreeMap::new();
        for d in changes {
            if !model.pairs.contains(&d.layer)
                || d.color.is_some_and(|c| c[3] != 255)
                || d.width.is_some_and(|w| !(1..=8).contains(&w))
                || (d.color.is_none() && d.fill.is_none() && d.width.is_none())
                || unique.insert(d.layer, d).is_some()
            {
                return Err(Error::input("invalid or duplicate style delta"));
            }
        }
        let mut expanded: BTreeMap<_, StyleDelta> = BTreeMap::new();
        // A GUI head edit targets the entire group. Explicit child fields in
        // this transaction win individually; a child color must not erase a
        // head fill edit or materialize its unchanged fill/width inheritance.
        let mut merge = |layer, d: &StyleDelta| {
            let out = expanded.entry(layer).or_insert_with(|| StyleDelta {
                layer,
                ..Default::default()
            });
            if let Some(color) = d.color {
                out.color = Some(color);
            }
            if let Some(fill) = &d.fill {
                out.fill = Some(fill.clone());
            }
            if let Some(width) = d.width {
                out.width = Some(width);
            }
        };
        for (key, d) in &unique {
            if let Some(children) = model.groups.get(key) {
                for &child in children {
                    merge(child, d);
                }
            }
        }
        for (key, d) in &unique {
            merge(*key, d);
        }
        let assignments = Arc::make_mut(&mut self.assignments);
        for (key, d) in &expanded {
            if let Some(fill) = &d.fill {
                assignments
                    .fills
                    .insert(*key, AssignedFill::Direct(fill.clone()));
            }
            if let Some(width) = d.width {
                if width > 1 {
                    assignments.widths.insert(*key, width);
                } else {
                    assignments.widths.remove(key);
                }
            }
        }
        self.styles = Arc::new(
            self.styles
                .iter()
                .map(|old| {
                    let Some(d) = expanded.get(&old.layer) else {
                        return old.clone();
                    };
                    let head = (old.layer.0, 0);
                    let width = if d.width.is_some() {
                        assignments
                            .widths
                            .get(&old.layer)
                            .or_else(|| {
                                model
                                    .groups
                                    .contains_key(&head)
                                    .then(|| assignments.widths.get(&head))
                                    .flatten()
                            })
                            .copied()
                            .unwrap_or(1)
                    } else {
                        old.width
                    };
                    Style {
                        layer: old.layer,
                        color: d.color.unwrap_or(old.color),
                        fill: d.fill.clone().unwrap_or_else(|| old.fill.clone()),
                        width,
                    }
                })
                .collect(),
        );
        Ok(())
    }
    pub(super) fn apply_properties(
        &mut self,
        model: &Model,
        doc: &layerprops::Document,
    ) -> Result<()> {
        // Library callers can construct Document directly; do not rely only
        // on the HTTP parser to enforce token/row/document bounds.
        layerprops::format(&doc.rows)?;
        let assignments = Arc::make_mut(&mut self.assignments);
        let mut colors = BTreeMap::new();
        let mut visibility = Vec::new();
        for row in doc.rows.iter().filter(|r| model.pairs.contains(&r.layer)) {
            if let Some(color) = styles::color(&row.color) {
                colors.insert(row.layer, color);
            }
            if let Some(name) = styles::pattern_slot(&row.fill) {
                assignments
                    .fills
                    .insert(row.layer, AssignedFill::Slot(name));
            }
            if let Some(width) = row.line_width() {
                if width > 1 {
                    assignments.widths.insert(row.layer, width);
                } else {
                    assignments.widths.remove(&row.layer);
                }
            }
            visibility.push(styles::LayerProps {
                layer: row.layer,
                color: None,
                fill: String::new(),
                width: 1,
                visible: row.visible(),
            });
        }
        let mut expanded = BTreeMap::new();
        // Only colors explicitly in THIS load propagate; prior child colors
        // do not block a newly assigned head color. Fill/width are different:
        // GTK re-sends their entire sparse maps and child assignments win.
        for (head, children) in &model.groups {
            if let Some(color) = colors.get(head) {
                for &child in children {
                    expanded.insert(child, *color);
                }
            }
        }
        expanded.extend(colors);
        self.styles = Arc::new(
            self.styles
                .iter()
                .map(|old| {
                    let head = (old.layer.0, 0);
                    let parent = model.groups.contains_key(&head).then_some(head);
                    let fill = assignments
                        .fills
                        .get(&old.layer)
                        .or_else(|| parent.and_then(|p| assignments.fills.get(&p)))
                        .map(|f| assignments.resolve(f))
                        .unwrap_or(Fill::Speckle);
                    let width = assignments
                        .widths
                        .get(&old.layer)
                        .or_else(|| parent.and_then(|p| assignments.widths.get(&p)))
                        .copied()
                        .unwrap_or(1);
                    Style {
                        layer: old.layer,
                        color: expanded.get(&old.layer).copied().unwrap_or(old.color),
                        fill,
                        width,
                    }
                })
                .collect(),
        );
        self.layers = model.properties_visibility_from(&visibility, &self.layers)?;
        Ok(())
    }
    /// Export the effective display, not guessed names for edited bitmaps.
    /// A Calibre file flattens inheritance; native settings additionally
    /// preserve assignments, not just this resolved table.
    pub fn layerprops(&self, model: &Model) -> Result<String> {
        let names = &model.property_names;
        let styles: BTreeMap<_, _> = self.styles.iter().map(|s| (s.layer, s)).collect();
        // Six-column startup/load interpret width 1 as inheritance. Native
        // JSON/complete styles can represent a narrow child under a wider
        // head; never silently widen that child on reopening a default.
        for (head, children) in &model.groups {
            if styles.get(head).is_some_and(|s| s.width > 1)
                && children
                    .iter()
                    .any(|p| styles.get(p).is_some_and(|s| s.width == 1))
            {
                return Err(crate::Error::new(crate::ErrorKind::Unsupported,
                    "Calibre layerprops cannot preserve child width 1 under a wider head; use Native JSON"));
            }
        }
        let selected = |pair| match &self.layers {
            Layers::All => true,
            Layers::None => false,
            Layers::Only(p) => p.binary_search(&pair).is_ok(),
        };
        let mut rows = Vec::with_capacity(names.len().min(layerprops::MAX_ROWS));
        let mut seen = BTreeSet::new();
        if names.len() > layerprops::MAX_ROWS {
            return Err(Error::input("too many layer property names"));
        }
        for (pair, name) in names {
            if !seen.insert(*pair) {
                return Err(Error::input("duplicate property name key"));
            }
            let style = styles
                .get(pair)
                .ok_or_else(|| Error::input("unknown property name key"))?;
            let visible = model
                .groups
                .get(pair)
                .map_or_else(|| selected(*pair), |p| p.iter().any(|p| selected(*p)));
            rows.push(layerprops::Row::from_style(style, name, visible)?);
        }
        if seen.len() != styles.len() {
            return Err(Error::input("incomplete property name table"));
        }
        layerprops::format(&rows)
    }
    pub fn settings(&self, model: &Model) -> layerprops::Settings {
        let selected = |pair| match &self.layers {
            Layers::All => true,
            Layers::None => false,
            Layers::Only(p) => p.binary_search(&pair).is_ok(),
        };
        let slotted = !self.assignments.slots.is_empty()
            || self
                .assignments
                .fills
                .values()
                .any(|f| matches!(f, AssignedFill::Slot(_)));
        layerprops::Settings {
            format: "floe.layers".into(),
            version: if slotted { 2 } else { 1 },
            fill_slots: slotted.then(|| self.fill_slots()),
            groups: model.groups.iter().map(|(k, v)| (*k, v.clone())).collect(),
            rows: self
                .styles
                .iter()
                .map(|s| layerprops::Setting {
                    pair: s.layer,
                    color: styles::color_text(s.color),
                    fill: self.assignments.fills.get(&s.layer).and_then(|f| match f {
                        AssignedFill::Direct(fill) => Some(layerprops::Bitmap::from(fill)),
                        AssignedFill::Slot(_) => None,
                    }),
                    fill_slot: self.assignments.fills.get(&s.layer).and_then(|f| match f {
                        AssignedFill::Slot(name) => Some((*name).into()),
                        AssignedFill::Direct(_) => None,
                    }),
                    width: self.assignments.widths.get(&s.layer).copied(),
                    visible: model
                        .groups
                        .get(&s.layer)
                        .map_or_else(|| selected(s.layer), |p| p.iter().any(|p| selected(*p))),
                })
                .collect(),
        }
    }
    pub(super) fn apply_settings(
        &mut self,
        model: &Model,
        settings: &layerprops::Settings,
    ) -> Result<()> {
        settings.validate()?;
        if settings.groups
            != model
                .groups
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect::<Vec<_>>()
        {
            return Err(Error::input("settings layer groups differ from this view"));
        }
        let rows: BTreeMap<_, _> = settings.rows.iter().map(|r| (r.pair, r)).collect();
        if rows.len() != settings.rows.len()
            || rows.len() != self.styles.len()
            || self.styles.iter().any(|s| !rows.contains_key(&s.layer))
        {
            return Err(Error::input(
                "settings must contain each current layer exactly once",
            ));
        }
        let mut assignments = Assignments::default();
        if let Some(slots) = &settings.fill_slots {
            for slot in slots {
                let name = styles::pattern_slot(&slot.name).expect("validated slot name");
                if assignments.slot_rows(name) != slot.rows {
                    assignments.slots.insert(name, slot.rows);
                }
            }
        }
        for r in rows.values() {
            if let Some(fill) = &r.fill {
                assignments
                    .fills
                    .insert(r.pair, AssignedFill::Direct(fill.into()));
            }
            if let Some(name) = &r.fill_slot {
                assignments.fills.insert(
                    r.pair,
                    AssignedFill::Slot(styles::pattern_slot(name).expect("validated slot name")),
                );
            }
            if let Some(width) = r.width {
                assignments.widths.insert(r.pair, width);
            }
        }
        self.assignments = Arc::new(assignments);
        self.apply_properties(
            model,
            &layerprops::Document {
                rows: Vec::new(),
                malformed: 0,
            },
        )?;
        // Stored colors are effective values, not head recolor commands.
        self.styles = Arc::new(
            self.styles
                .iter()
                .map(|s| Style {
                    color: styles::color(&rows[&s.layer].color).expect("validated color"),
                    ..s.clone()
                })
                .collect(),
        );
        let visible: Vec<_> = rows
            .values()
            .map(|r| styles::LayerProps {
                layer: r.pair,
                color: None,
                fill: String::new(),
                width: 1,
                visible: Some(r.visible),
            })
            .collect();
        self.layers = model.properties_visibility_from(&visible, &Layers::None)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model() -> Model {
        let styles: Vec<_> = [(1, 0), (1, 1), (1, 2), (2, 0)]
            .into_iter()
            .map(|layer| Style {
                layer,
                color: [255; 4],
                fill: Fill::Speckle,
                width: 1,
            })
            .collect();
        Model {
            minimap: Arc::default(),
            dataset_revision: 1,
            dbu: 1.,
            bbox: [0., 0., 100., 100.],
            deck: true,
            skipped: 0,
            source_stale: false,
            pairs: styles.iter().map(|s| s.layer).collect(),
            groups: [((1, 0), vec![(1, 1), (1, 2)])].into(),
            folded_groups: BTreeSet::new(),
            property_names: styles
                .iter()
                .map(|s| (s.layer, format!("M{} {}", s.layer.0, s.layer.1)))
                .collect(),
            styles: Arc::new(styles),
            initial_layers: Layers::All,
            assignments: Arc::default(),
        }
    }
    fn load(s: &ViewState, m: &Model, text: &str) -> ViewState {
        s.edit(
            m,
            Patch {
                properties: Some(layerprops::parse(text).unwrap()),
                ..Default::default()
            },
        )
        .unwrap()
    }
    #[test]
    fn sparse_inheritance_duplicates_and_invalid_tokens_are_independent() {
        let m = model();
        let s = ViewState::initial(&m, 100, 100).unwrap();
        let a = load(&s, &m, "1.1 blue clear child 1 3\n1 red solid head 0 8");
        assert_eq!(a.layers, Layers::Only(vec![(1, 1), (2, 0)]));
        assert_eq!(a.styles[1].color, [0, 0, 255, 255]);
        assert_eq!(a.styles[1].width, 3);
        assert_eq!(a.styles[2].width, 8);
        assert_eq!(a.styles[2].fill, styles::pattern("solid").unwrap());
        let b = load(&a, &m, "1 green diagonal_1 head ? bad");
        assert_eq!(b.layers, a.layers);
        assert_eq!(b.styles[1].color, [0, 255, 0, 255]);
        assert_eq!(b.styles[1].fill, styles::pattern("clear").unwrap());
        assert_eq!(b.styles[1].width, 3);
        assert_eq!(b.styles[2].fill, styles::pattern("diagonal_1").unwrap());
        let c = load(&b, &m, "1.1 ? INVALID child ? 1");
        assert_eq!(c.styles[1].width, 8);
        assert_eq!(c.styles[1].fill, b.styles[1].fill);
        // Width-only loads are effective too, unlike GTK's early-return bug.
        let d = load(&c, &m, "1 ? INVALID head ? 1");
        assert_eq!(d.styles[1].width, 1);
        assert_eq!(d.styles[2].width, 1);
        let e = load(
            &d,
            &m,
            "1.1 red solid child ? 5\n1.1 unknown INVALID child ? 1",
        );
        assert_eq!(e.styles[1].color, [255, 0, 0, 255]);
        assert_eq!(e.styles[1].width, 1);
        assert_eq!(load(&e, &m, "9.9 red solid unknown 0 8\n"), e);
        assert_eq!(load(&e, &m, ""), e);
    }
    fn delta(s: &ViewState, m: &Model, changes: Vec<StyleDelta>) -> ViewState {
        s.edit(
            m,
            Patch {
                style_deltas: changes,
                ..Default::default()
            },
        )
        .unwrap()
    }
    #[test]
    fn color_only_edits_do_not_materialize_fill_or_width_inheritance() {
        let m = model();
        let s = load(
            &ViewState::initial(&m, 100, 100).unwrap(),
            &m,
            "1 blue solid HEAD 1 6\n1.1 red clear CHILD 0 3",
        );
        let inherited = delta(
            &s,
            &m,
            vec![StyleDelta {
                layer: (1, 2),
                color: Some([17, 34, 51, 255]),
                ..Default::default()
            }],
        );
        assert_eq!(inherited.assignments, s.assignments);
        assert_eq!(inherited.styles[2].width, 6);
        assert_eq!(inherited.styles[2].fill, s.styles[2].fill);
        assert!(inherited.settings(&m).rows[2].fill.is_none());
        assert!(inherited.settings(&m).rows[2].width.is_none());
        let head = delta(
            &inherited,
            &m,
            vec![StyleDelta {
                layer: (1, 0),
                color: Some([0, 255, 0, 255]),
                ..Default::default()
            }],
        );
        assert_eq!(head.assignments, s.assignments);
        assert!(head.styles[..3].iter().all(|s| s.color == [0, 255, 0, 255]));
        assert_eq!(head.styles[1].fill, s.styles[1].fill);
        assert_eq!(head.styles[1].width, 3);
        let next = load(&head, &m, "1 yellow diagonal_1 HEAD 1 2");
        assert_eq!(next.styles[1].width, 3);
        assert_eq!(next.styles[1].fill, s.styles[1].fill);
        assert_eq!(next.styles[2].width, 2);
        assert_eq!(next.styles[2].fill, styles::pattern("diagonal_1").unwrap());
        let restored = ViewState::initial(&m, 100, 100)
            .unwrap()
            .edit(
                &m,
                Patch {
                    settings: Some(head.settings(&m)),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(restored, head);
        assert_eq!(load(&restored, &m, "1 yellow diagonal_1 HEAD 1 2"), next);
    }
    #[test]
    fn style_delta_groups_merge_per_field_and_width_one_clears_override() {
        let m = model();
        let s = load(
            &ViewState::initial(&m, 100, 100).unwrap(),
            &m,
            "1 blue solid HEAD 1 6\n1.1 red clear CHILD 0 3",
        );
        let head = StyleDelta {
            layer: (1, 0),
            fill: Some(Fill::Pattern([0x1234; 16])),
            ..Default::default()
        };
        let child = StyleDelta {
            layer: (1, 1),
            color: Some([17, 34, 51, 255]),
            ..Default::default()
        };
        let next = delta(&s, &m, vec![child.clone(), head.clone()]);
        assert_eq!(next, delta(&s, &m, vec![head, child]));
        assert!(next.styles[..3]
            .iter()
            .all(|s| s.fill == Fill::Pattern([0x1234; 16])));
        assert_eq!(next.styles[1].color, [17, 34, 51, 255]);
        assert_eq!(next.styles[1].width, 3);
        assert_eq!(next.styles[2].color, s.styles[2].color);
        let inherit = delta(
            &next,
            &m,
            vec![StyleDelta {
                layer: (1, 1),
                width: Some(1),
                ..Default::default()
            }],
        );
        assert_eq!(inherit.styles[1].width, 6);
        assert!(inherit.settings(&m).rows[1].width.is_none());
        let reset = delta(
            &inherit,
            &m,
            vec![StyleDelta {
                layer: (1, 0),
                width: Some(1),
                ..Default::default()
            }],
        );
        assert!(reset.styles[..3].iter().all(|s| s.width == 1));
        assert_eq!(
            load(&reset, &m, ""),
            reset,
            "resolved styles and sparse assignments diverged"
        );
        let explicit = delta(
            &reset,
            &m,
            vec![
                StyleDelta {
                    layer: (1, 1),
                    width: Some(2),
                    ..Default::default()
                },
                StyleDelta {
                    layer: (1, 0),
                    width: Some(5),
                    ..Default::default()
                },
            ],
        );
        assert_eq!(
            explicit.styles[..3]
                .iter()
                .map(|s| s.width)
                .collect::<Vec<_>>(),
            vec![5, 2, 5]
        );
    }
    #[test]
    fn invalid_style_deltas_and_conflicting_forms_are_atomic() {
        let m = model();
        let initial = ViewState::initial(&m, 100, 100).unwrap();
        let valid = StyleDelta {
            layer: (1, 1),
            color: Some([1, 2, 3, 255]),
            ..Default::default()
        };
        for invalid in [
            StyleDelta {
                layer: (99, 0),
                ..valid.clone()
            },
            StyleDelta {
                layer: (1, 1),
                ..Default::default()
            },
            StyleDelta {
                color: Some([1, 2, 3, 0]),
                ..valid.clone()
            },
            StyleDelta {
                width: Some(0),
                ..valid.clone()
            },
            StyleDelta {
                width: Some(9),
                ..valid.clone()
            },
        ] {
            assert!(initial
                .edit(
                    &m,
                    Patch {
                        style_deltas: vec![invalid],
                        ..Default::default()
                    }
                )
                .is_err());
        }
        for changes in [
            vec![valid.clone(), valid.clone()],
            vec![valid.clone(); 4097],
        ] {
            assert!(initial
                .edit(
                    &m,
                    Patch {
                        style_deltas: changes,
                        ..Default::default()
                    }
                )
                .is_err());
        }
        assert!(initial
            .edit(
                &m,
                Patch {
                    style_deltas: vec![valid.clone()],
                    style_changes: vec![initial.styles[1].clone()],
                    ..Default::default()
                }
            )
            .is_err());
        assert!(initial
            .edit(
                &m,
                Patch {
                    style_deltas: vec![valid],
                    properties: Some(layerprops::parse("").unwrap()),
                    ..Default::default()
                }
            )
            .is_err());
        assert_eq!(initial, ViewState::initial(&m, 100, 100).unwrap());
    }
    #[test]
    fn native_round_trip_retains_custom_patterns_and_future_head_semantics() {
        let m = model();
        let s = load(
            &ViewState::initial(&m, 100, 100).unwrap(),
            &m,
            "1 blue solid HEAD 1 6\n1.1 red clear CHILD 0 3",
        );
        let s = s
            .edit(
                &m,
                Patch {
                    style_changes: vec![Style {
                        layer: (2, 0),
                        color: [17, 23, 51, 255],
                        fill: Fill::Pattern([0x1234; 16]),
                        width: 2,
                    }],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            s.layerprops(&m).unwrap_err().kind,
            crate::ErrorKind::Unsupported
        );
        let text = s.settings(&m).text().unwrap();
        let snapshot = layerprops::parse_settings(&text).unwrap();
        assert!(snapshot.rows[2].fill.is_none());
        assert!(snapshot.rows[2].width.is_none());
        let initial = ViewState::initial(&m, 100, 100).unwrap();
        let restored = initial
            .edit(
                &m,
                Patch {
                    settings: Some(snapshot.clone()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(restored, s);
        assert_eq!(
            load(&s, &m, "1 yellow carpet_1 HEAD 1 2"),
            load(&restored, &m, "1 yellow carpet_1 HEAD 1 2")
        );
        let mut duplicate = snapshot.clone();
        duplicate.rows[1] = duplicate.rows[0].clone();
        let mut wrong_group = snapshot.clone();
        wrong_group.groups.clear();
        let mut bad_width = snapshot.clone();
        bad_width.rows[1].width = Some(0);
        for bad in [duplicate, wrong_group, bad_width] {
            assert!(initial
                .edit(
                    &m,
                    Patch {
                        settings: Some(bad),
                        ..Default::default()
                    }
                )
                .is_err());
        }
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        for field in ["fill", "width"] {
            let mut missing = json.clone();
            missing["rows"][0].as_object_mut().unwrap().remove(field);
            assert!(layerprops::parse_settings(&missing.to_string()).is_err());
        }
        assert!(
            layerprops::parse_settings(&text.replace("\"version\":2", "\"version\":3")).is_err()
        );
    }
    #[test]
    fn calibre_rejects_explicit_child_width_one_that_would_inherit_on_load() {
        let m = model();
        let s = load(
            &ViewState::initial(&m, 100, 100).unwrap(),
            &m,
            "1 red solid HEAD 1 6",
        );
        let mut child = s.styles[1].clone();
        child.width = 1;
        let s = s
            .edit(
                &m,
                Patch {
                    style_changes: vec![child],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(s.styles[0].width, 6);
        assert_eq!(s.styles[1].width, 1);
        assert_eq!(
            s.layerprops(&m).unwrap_err().kind,
            crate::ErrorKind::Unsupported
        );
        let restored = ViewState::initial(&m, 100, 100)
            .unwrap()
            .edit(
                &m,
                Patch {
                    settings: Some(
                        layerprops::parse_settings(&s.settings(&m).text().unwrap()).unwrap(),
                    ),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(restored, s);
    }
    #[test]
    fn prepared_import_is_atomic_and_calibre_export_uses_effective_styles() {
        let m = model();
        let initial = ViewState::initial(&m, 100, 100).unwrap();
        let doc = layerprops::parse("1 red solid HEAD 0 5\n1.1 blue clear CHILD 1 3").unwrap();
        let prepared = initial
            .prepare_layers(
                &m,
                Patch {
                    properties: Some(doc.clone()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(initial.layers, Layers::All);
        let accepted = initial
            .edit(
                &m,
                Patch {
                    prepared_layers: Some(prepared),
                    ..Default::default()
                },
            )
            .unwrap();
        let text = accepted.layerprops(&m).unwrap();
        let round = load(&initial, &m, &text);
        // Builtin speckle and its six-column named 16x16 bitmap have the
        // same mask; Calibre text intentionally does not preserve enum tags.
        let bitmap = |styles: &Vec<Style>| {
            styles
                .iter()
                .cloned()
                .map(|mut s| {
                    if s.fill == Fill::Speckle {
                        s.fill = styles::pattern("speckle").unwrap();
                    }
                    s
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(bitmap(&accepted.styles), bitmap(&round.styles));
        assert_eq!(accepted.layers, round.layers);
        assert!(text.contains("M1_2 0 5"));
        let mut bad = doc;
        bad.rows[0].name = "bad\nname".into();
        assert!(initial
            .prepare_layers(
                &m,
                Patch {
                    properties: Some(bad),
                    ..Default::default()
                }
            )
            .is_err());
        assert_eq!(initial, ViewState::initial(&m, 100, 100).unwrap());
    }
}
