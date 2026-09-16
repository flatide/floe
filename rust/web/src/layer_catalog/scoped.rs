//! Guest projection: filter real planes BEFORE labels, grouping and counts.
//! Only row indices/member keys are temporary; never copy an owner catalogue
//! (including its names) into a guest DTO and then redact it.
use super::*;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ScopedPage {
    start: usize,
    fold: Fold,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Visibility {
    pair: Pair,
    group: bool,
    visible: bool,
}
struct Group {
    start: usize,
    end: usize,
    targets: Vec<Pair>,
}
struct Projection<'a> {
    catalog: &'a LayerCatalog,
    indices: Vec<usize>,
    groups: BTreeMap<Pair, Group>,
    allowed: BTreeSet<Pair>,
    all: BTreeSet<Pair>,
}
impl<'a> Projection<'a> {
    fn new<'m>(
        catalog: &'a LayerCatalog,
        scope: &Layers,
        actual: impl Iterator<Item = Pair>,
        members: impl Fn(Pair) -> Option<&'m [Pair]>,
    ) -> Self {
        let all: BTreeSet<_> = actual.collect();
        let allowed = match scope {
            Layers::All => all.clone(),
            Layers::None => BTreeSet::new(),
            Layers::Only(pairs) => pairs.iter().copied().filter(|p| all.contains(p)).collect(),
        };
        let mut indices = Vec::new();
        let mut groups = BTreeMap::new();
        for (i, r) in catalog.rows.iter().enumerate() {
            if r.head {
                let targets: Vec<_> = members(r.pair)
                    .unwrap_or_default()
                    .iter()
                    .copied()
                    .filter(|p| allowed.contains(p))
                    .collect();
                if targets.is_empty() {
                    continue;
                }
                groups.insert(
                    r.pair,
                    Group {
                        start: indices.len(),
                        end: indices.len() + 1,
                        targets,
                    },
                );
            } else if !allowed.contains(&r.pair) {
                continue;
            }
            indices.push(i);
        }
        let mut start = 0;
        while start < indices.len() {
            let r = &catalog.rows[indices[start]];
            let end =
                start + indices[start..].partition_point(|&i| catalog.rows[i].pair.0 == r.pair.0);
            if let Some(g) = groups.get_mut(&r.pair) {
                g.end = end;
            } else if catalog.physical && end > start + 1 {
                groups.insert(
                    r.pair,
                    Group {
                        start,
                        end,
                        targets: indices[start..end]
                            .iter()
                            .map(|&i| catalog.rows[i].pair)
                            .collect(),
                    },
                );
            }
            start = end;
        }
        Self {
            catalog,
            indices,
            groups,
            allowed,
            all,
        }
    }
    fn closed(&self, fold: Fold) -> Result<Closed, &'static str> {
        if fold.exceptions.len() > SELECTION_LIMIT {
            return Err("invalid_palette");
        }
        let mut exceptions = BTreeSet::new();
        for p in fold.exceptions {
            if !self.groups.get(&p).is_some_and(|g| g.end > g.start + 1) || !exceptions.insert(p) {
                return Err("invalid_palette");
            }
        }
        Ok(Closed {
            default: fold.closed,
            exceptions,
        })
    }
    fn positions(&self, closed: &Closed) -> impl Iterator<Item = usize> + '_ {
        let mut position = 0;
        let closed: BTreeSet<_> = self
            .groups
            .keys()
            .copied()
            .filter(|p| closed.contains(*p))
            .collect();
        std::iter::from_fn(move || {
            let &i = self.indices.get(position)?;
            let result = position;
            let pair = self.catalog.rows[i].pair;
            position = if closed.contains(&pair) {
                self.groups[&pair].end
            } else {
                position + 1
            };
            Some(result)
        })
    }
    fn targets(&self, pair: Pair, group: bool) -> Result<Vec<Pair>, &'static str> {
        let pos = self
            .indices
            .binary_search_by_key(&pair, |&i| self.catalog.rows[i].pair)
            .map_err(|_| "forbidden")?;
        let row = &self.catalog.rows[self.indices[pos]];
        if group || row.head {
            return self
                .groups
                .get(&pair)
                .map(|g| g.targets.clone())
                .ok_or("forbidden");
        }
        Ok(vec![pair])
    }
    fn change(&self, selected: &Layers, change: Visibility) -> Result<Layers, &'static str> {
        let mut selected: BTreeSet<_> = match selected {
            Layers::All => self.all.clone(),
            Layers::None => BTreeSet::new(),
            Layers::Only(pairs) => pairs.iter().copied().collect(),
        };
        if !selected.is_subset(&self.allowed) {
            return Err("forbidden");
        }
        for p in self.targets(change.pair, change.group)? {
            if change.visible {
                selected.insert(p);
            } else {
                selected.remove(&p);
            }
        }
        Ok(if selected.is_empty() {
            Layers::None
        } else if selected == self.all && self.allowed == self.all {
            Layers::All
        } else if selected.len() <= SELECTION_LIMIT {
            Layers::Only(selected.into_iter().collect())
        } else {
            return Err("layer_selection_too_large");
        })
    }
}
impl LayerCatalog {
    fn projection<'a>(&'a self, model: &Model, scope: &Layers) -> Projection<'a> {
        Projection::new(
            self,
            scope,
            model
                .styles
                .iter()
                .map(|s| s.layer)
                .filter(|p| model.layer_group(*p).is_none()),
            |p| model.layer_group(p),
        )
    }
    pub(crate) fn scoped_change(
        &self,
        model: &Model,
        snapshot: &Snapshot,
        scope: &Layers,
        change: Visibility,
    ) -> Result<Layers, &'static str> {
        self.projection(model, scope)
            .change(&snapshot.state.layers, change)
    }
    pub(crate) fn scoped_page(
        &self,
        model: &Model,
        snapshot: &Snapshot,
        scope: &Layers,
        request: ScopedPage,
    ) -> Result<Value, &'static str> {
        let p = self.projection(model, scope);
        let closed = p.closed(request.fold)?;
        let total = p.positions(&closed).count();
        if request.start > total {
            return Err("invalid_palette");
        }
        let selected = |pair| match &snapshot.state.layers {
            Layers::All => true,
            Layers::None => false,
            Layers::Only(pairs) => pairs.binary_search(&pair).is_ok(),
        };
        let mut rows = Vec::with_capacity(PAGE);
        for pos in p.positions(&closed).skip(request.start).take(PAGE) {
            let i = p.indices[pos];
            let r = &self.rows[i];
            let mut row = self.display_rows(snapshot, &[i], None).pop().unwrap();
            let group = p.groups.get(&r.pair);
            let parent = p
                .groups
                .range(..r.pair)
                .next_back()
                .filter(|(_, g)| pos > g.start && pos < g.end)
                .map(|(&pair, _)| pair);
            let count = group.map_or(0, |g| g.targets.iter().filter(|&&v| selected(v)).count());
            let members = group.map_or(0, |g| g.targets.len());
            // Parent/children and mixed flags describe approved members only.
            row["parent"] = json!(parent);
            row["head"] = json!(group.is_some());
            row["synthetic"] = json!(r.head);
            row["children"] = json!(group.map_or(0, |g| g.end - g.start - 1));
            row["closed"] =
                json!(group.is_some_and(|g| g.end > g.start + 1) && closed.contains(r.pair));
            row["visible"] = json!(if r.head { count > 0 } else { selected(r.pair) });
            row["all_visible"] = json!(members > 0 && count == members);
            row["mixed"] = json!(count > 0 && count < members);
            rows.push(row);
        }
        let end = request.start + rows.len();
        Ok(
            json!({"state_rev":snapshot.state_rev.to_string(),"render_key":snapshot.render_key.to_string(),
            "total":total,"all_total":p.indices.len(),"start":request.start,
            "next":if end < total {Some(end)} else {None},"rows":rows}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog(pairs: &[Pair], physical: bool) -> LayerCatalog {
        super::super::tests::catalogue(pairs.to_vec(), physical)
    }
    #[test]
    fn physical_filter_precedes_grouping_pagination_and_guessed_changes() {
        let c = catalog(&[(3, 0), (3, 1), (3, 2), (8, 0)], true);
        let scope = Layers::Only(vec![(3, 1), (3, 2)]);
        let p = Projection::new(
            &c,
            &scope,
            [(3, 0), (3, 1), (3, 2), (8, 0)].into_iter(),
            |_| None,
        );
        assert_eq!(p.indices, vec![1, 2]);
        assert!(p.groups.contains_key(&(3, 1)));
        assert!(!p.groups.contains_key(&(3, 0)));
        let closed = p
            .closed(Fold {
                closed: true,
                exceptions: vec![],
            })
            .unwrap();
        assert_eq!(p.positions(&closed).collect::<Vec<_>>(), vec![0]);
        assert_eq!(p.targets((3, 1), true).unwrap(), vec![(3, 1), (3, 2)]);
        assert_eq!(p.targets((3, 1), false).unwrap(), vec![(3, 1)]);
        assert!(p.targets((3, 0), false).is_err());
        assert!(p
            .closed(Fold {
                closed: false,
                exceptions: vec![(3, 0)]
            })
            .is_err());
        assert_eq!(
            p.change(
                &Layers::None,
                Visibility {
                    pair: (3, 1),
                    group: true,
                    visible: true
                }
            )
            .unwrap(),
            scope
        );
    }
    #[test]
    fn partial_deck_heads_only_target_authorized_children_including_hidden() {
        let c = catalog(&[(1, 0), (1, 1), (1, 2), (2, 0), (2, 1)], false);
        let group = [(1, 1), (1, 2), (1, 3)]; // third child hidden from the palette
        let scope = Layers::Only(vec![(1, 2), (1, 3)]);
        let p = Projection::new(&c, &scope, group.into_iter().chain([(2, 1)]), |k| {
            if k == (1, 0) {
                Some(&group[..])
            } else {
                None
            }
        });
        assert_eq!(p.indices, vec![0, 2]);
        assert_eq!(p.targets((1, 0), false).unwrap(), vec![(1, 2), (1, 3)]);
        assert!(p.targets((1, 1), false).is_err());
        assert_eq!(
            p.change(
                &Layers::None,
                Visibility {
                    pair: (1, 0),
                    group: true,
                    visible: true
                }
            )
            .unwrap(),
            scope
        );
        let level = catalog(&[(1, 0), (2, 0)], false);
        let p = Projection::new(&level, &scope, group.into_iter(), |k| {
            if k == (1, 0) {
                Some(&group[..])
            } else {
                None
            }
        });
        assert_eq!(p.indices, vec![0]);
        assert_eq!(p.groups[&(1, 0)].end, 1);
        assert_eq!(p.targets((1, 0), false).unwrap(), vec![(1, 2), (1, 3)]);
    }
    #[test]
    fn empty_scope_and_large_selection_fail_without_widening() {
        let pairs: Vec<_> = (0..5000).map(|n| (7, n)).collect();
        let c = catalog(&pairs, true);
        let p = Projection::new(&c, &Layers::All, pairs.iter().copied(), |_| None);
        assert_eq!(
            p.change(
                &Layers::None,
                Visibility {
                    pair: (7, 0),
                    group: true,
                    visible: true
                }
            )
            .unwrap(),
            Layers::All
        );
        assert_eq!(
            p.change(
                &Layers::All,
                Visibility {
                    pair: (7, 0),
                    group: false,
                    visible: false
                }
            )
            .unwrap_err(),
            "layer_selection_too_large"
        );
        let p = Projection::new(&c, &Layers::None, pairs.into_iter(), |_| None);
        assert!(p.indices.is_empty());
        assert!(p.targets((7, 0), true).is_err());
    }
}
