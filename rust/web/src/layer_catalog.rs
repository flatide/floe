//! Bounded display metadata, never raw native/cache JSON or source tooltips.
use floe_app_core::{
    dataset::Dataset,
    view::{Model, Snapshot},
};
use floe_worker_client::{Fill, Layers};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const PAGE: usize = 64;
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
}
fn label(s: &str) -> String {
    s.chars().take(128).collect()
}
impl LayerCatalog {
    pub fn model(model: &Model) -> Self {
        Self {
            rows: model
                .styles
                .iter()
                .enumerate()
                .map(|(i, s)| Row {
                    pair: s.layer,
                    name: format!("{}/{}", s.layer.0, s.layer.1),
                    aliases: Vec::new(),
                    head: false,
                    parent: None,
                    style: Some(i),
                    fallback: "#ffffff".into(),
                })
                .collect(),
        }
    }
    pub fn dataset(dataset: &Dataset, model: &Model) -> Self {
        let styles: BTreeMap<_, _> = model
            .styles
            .iter()
            .enumerate()
            .map(|(i, s)| (s.layer, i))
            .collect();
        let mut rows = match dataset {
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
        rows.sort_by_key(|r| r.pair);
        Self { rows }
    }
    pub fn page(&self, snapshot: &Snapshot, start: usize) -> Option<Value> {
        if start > self.rows.len() {
            return None;
        }
        let end = start.saturating_add(PAGE).min(self.rows.len());
        let selected = |pair| match &snapshot.state.layers {
            Layers::All => true,
            Layers::None => false,
            Layers::Only(pairs) => pairs.binary_search(&pair).is_ok(),
        };
        let rows:Vec<_> = self.rows[start..end].iter().map(|r| {
            let style = r.style.and_then(|i|snapshot.state.styles.get(i));
            let color = style.map(|s|format!("#{:02x}{:02x}{:02x}",s.color[0],s.color[1],s.color[2])).unwrap_or_else(||r.fallback.clone());
            let fill = match style.map(|s|&s.fill) {Some(Fill::Clear)=>json!({"kind":"clear"}),Some(Fill::Speckle)=>json!({"kind":"speckle"}),Some(Fill::Pattern(rows))=>json!({"kind":"pattern","rows":rows}),_=>json!({"kind":"solid"})};
            // In level mode the actual render rows are hidden in the panel.
            // A selected hidden child still makes its level head visible.
            let visible = selected(r.pair) || (r.head && snapshot.state.styles.iter().filter(|s|s.layer.0==r.pair.0).any(|s|selected(s.layer)));
            json!({"pair":r.pair,"name":r.name,"aliases":r.aliases,"head":r.head,"parent":r.parent,"visible":visible,"color":color,"fill":fill,"width":style.map_or(1,|s|s.width)})
        }).collect();
        Some(
            json!({"state_rev":snapshot.state_rev.to_string(),"render_key":snapshot.render_key.to_string(),"total":self.rows.len(),"start":start,"next":if end<self.rows.len(){Some(end)}else{None},"rows":rows}),
        )
    }
}
