//! Physical GDS rule matching stays on the bounded DRC actor, not the HTTP
//! runtime or the currently paged browser layer list. No layout decode is needed.
use super::metadata::Metadata;
use floe_app_core::{
    check_cancelled,
    svrf::Rule,
    view::{LayerIsolation, Model, Navigation, Patch},
    Error, ErrorKind, Result,
};
use floe_worker_client::{Layers, Style};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    sync::{atomic::AtomicUsize, Arc, Mutex},
};

pub(super) struct Preparation {
    pub model: Arc<Model>,
    pub result: Arc<Mutex<Option<Patch>>>,
}
impl Preparation {
    pub fn build(
        &self,
        metadata: Option<&Metadata>,
        name: &str,
        navigation: Navigation,
        stop: &AtomicUsize,
    ) -> Result<Value> {
        let rule = metadata.and_then(|m| m.rules.rule(name));
        let (layers, status, count) = select(rule, &self.model.styles, self.model.deck, stop)?;
        check_cancelled(stop)?;
        *self.result.lock().unwrap() = Some(Patch {
            navigation: Some(navigation),
            layer_isolation: layers.map(LayerIsolation::Set),
            ..Default::default()
        });
        Ok(
            json!({"status":if metadata.is_none(){"no_metadata"}else{status},"matched":count.to_string()}),
        )
    }
}
fn select(
    rule: Option<&Rule>,
    styles: &[Style],
    deck: bool,
    stop: &AtomicUsize,
) -> Result<(Option<Layers>, &'static str, usize)> {
    check_cancelled(stop)?;
    // Deck level/TC IDs are not physical GDS IDs. A future physical-plane
    // visibility mapping must be designed explicitly; accidental equality is
    // never a reason to hide unrelated chips.
    if deck {
        return Ok((None, "unsupported_deck", 0));
    }
    let Some(rule) = rule else {
        return Ok((None, "no_rule", 0));
    };
    if rule.source_gds.is_empty() {
        return Ok((None, "no_source_layers", 0));
    }
    let mut wild = BTreeSet::new();
    let mut exact = BTreeSet::new();
    for (i, &(l, d)) in rule.source_gds.iter().enumerate() {
        if i % 1024 == 0 {
            check_cancelled(stop)?;
        }
        if let Some(d) = d {
            exact.insert((l, d));
        } else {
            wild.insert(l);
        }
    }
    let mut pairs = Vec::new();
    let mut count = 0;
    for (i, s) in styles.iter().enumerate() {
        if i % 1024 == 0 {
            check_cancelled(stop)?;
        }
        if wild.contains(&s.layer.0) || exact.contains(&s.layer) {
            count += 1;
            if pairs.len() < 4096 {
                pairs.push(s.layer);
            }
        }
    }
    let layers = if count == 0 {
        return Ok((None, "no_match", 0));
    } else if count == styles.len() {
        Layers::All
    } else if count > 4096 {
        return Err(Error::new(
            ErrorKind::Incomplete,
            "DRC layer isolation exceeds 4096 pairs",
        ));
    } else {
        Layers::Only(pairs)
    };
    Ok((Some(layers), "ready", count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    fn rule(pairs: Vec<(u32, Option<u32>)>) -> Rule {
        serde_json::from_value(json!({"source_gds":pairs})).unwrap()
    }
    fn styles(count: u32) -> Vec<Style> {
        (0..count)
            .map(|d| Style {
                layer: (7, d),
                color: [255; 4],
                fill: floe_worker_client::Fill::Solid,
                width: 1,
            })
            .collect()
    }
    #[test]
    fn matching_uses_full_catalog_and_never_conflates_deck_ids_with_gds() {
        let stop = AtomicUsize::new(0);
        let styles = styles(300);
        let r = rule(vec![(7, Some(299)), (9, None), (7, Some(299))]);
        assert_eq!(
            select(Some(&r), &styles, false, &stop).unwrap(),
            (Some(Layers::Only(vec![(7, 299)])), "ready", 1)
        );
        assert_eq!(
            select(Some(&r), &styles, true, &stop).unwrap(),
            (None, "unsupported_deck", 0)
        );
        assert_eq!(select(None, &styles, false, &stop).unwrap().1, "no_rule");
        assert_eq!(
            select(Some(&rule(vec![])), &styles, false, &stop)
                .unwrap()
                .1,
            "no_source_layers"
        );
        assert_eq!(
            select(Some(&rule(vec![(8, None)])), &styles, false, &stop)
                .unwrap()
                .1,
            "no_match"
        );
        assert_eq!(
            select(Some(&rule(vec![(7, None)])), &styles, false, &stop).unwrap(),
            (Some(Layers::All), "ready", 300)
        );
        stop.store(1, Ordering::Relaxed);
        assert_eq!(
            select(Some(&r), &styles, false, &stop).unwrap_err().kind,
            ErrorKind::Cancelled
        );
    }
    #[test]
    fn large_selection_is_bounded_and_errors_never_become_a_partial_isolate() {
        let stop = AtomicUsize::new(0);
        let mut styles = styles(5000);
        let r = rule(vec![(7, None)]);
        assert_eq!(
            select(Some(&r), &styles, false, &stop).unwrap().0,
            Some(Layers::All)
        );
        styles.push(Style {
            layer: (8, 0),
            ..styles[0].clone()
        });
        assert_eq!(
            select(Some(&r), &styles, false, &stop).unwrap_err().kind,
            ErrorKind::Incomplete
        );
        let r = rule((0..4096).map(|d| (7, Some(d))).collect());
        let (Some(Layers::Only(pairs)), _, count) =
            select(Some(&r), &styles, false, &stop).unwrap()
        else {
            panic!("expected explicit visibility")
        };
        assert_eq!(count, 4096);
        assert_eq!(pairs.len(), count);
    }
}
