//! Server-authoritative view state. Relative inputs are applied in order;
//! only rendering is coalesced. No HTTP or browser floating-point world math.
mod controller;
pub mod margin;
use crate::{
    dataset::Dataset,
    managed::ManagedDataset,
    shots::{Detail, Thin, MAX_PIXELS},
    Error, Result,
};
pub use controller::{
    ControllerOptions, DisplayFrame, MarginStatus, Phase, Purpose, Snapshot, ViewController,
};
use floe_worker_client::{Layers, RenderRequest, Style};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub bbox: [f64; 4],
    pub width: u32,
    pub height: u32,
}
impl Viewport {
    pub fn new(bbox: [f64; 4], width: u32, height: u32) -> Result<Self> {
        let v = Self {
            bbox,
            width,
            height,
        };
        v.validate()?;
        Ok(v)
    }
    pub fn validate(&self) -> Result<()> {
        let [x0, y0, x1, y1] = self.bbox;
        let bound = (1u64 << 62) as f64;
        if !self.bbox.iter().all(|n| n.is_finite() && n.abs() <= bound) || x1 <= x0 || y1 <= y0 {
            return Err(Error::input(
                "view bbox is invalid or outside supported coordinates",
            ));
        }
        if self.width == 0
            || self.height == 0
            || self.width > 8192
            || self.height > 8192
            || u64::from(self.width) * u64::from(self.height) > MAX_PIXELS
        {
            return Err(Error::input(
                "view dimensions require 1..8192 per axis and at most 16 Mpx",
            ));
        }
        Ok(())
    }
    fn centered(cx: f64, cy: f64, span: f64, width: u32, height: u32) -> Result<Self> {
        let sh = span / f64::from(width) * f64::from(height);
        Self::new(
            [cx - span / 2., cy - sh / 2., cx + span / 2., cy + sh / 2.],
            width,
            height,
        )
    }
    pub fn fit(bbox: [f64; 4], width: u32, height: u32) -> Result<Self> {
        let [x0, y0, x1, y1] = bbox;
        let span = (x1 - x0).max((y1 - y0) / f64::from(height) * f64::from(width));
        Self::centered(
            x0 + (x1 - x0) / 2.,
            y0 + (y1 - y0) / 2.,
            span,
            width,
            height,
        )
    }
    pub fn navigate(&self, nav: Navigation, fit: [f64; 4], dbu: f64) -> Result<Self> {
        let [x0, y0, x1, y1] = self.bbox;
        let (sx, sy) = (x1 - x0, y1 - y0);
        match nav {
            Navigation::Fit => Self::fit(fit, self.width, self.height),
            Navigation::Goto {
                center_um,
                width_um,
            } => {
                if !center_um.iter().all(|v| v.is_finite())
                    || !width_um.is_finite()
                    || width_um <= 0.
                    || !dbu.is_finite()
                    || dbu <= 0.
                {
                    return Err(Error::input(
                        "goto requires finite center and positive width",
                    ));
                }
                Self::centered(
                    center_um[0] / dbu,
                    center_um[1] / dbu,
                    width_um / dbu,
                    self.width,
                    self.height,
                )
            }
            Navigation::Pan { x, y, snap } => {
                if !x.is_finite() || !y.is_finite() || x.abs() > 1. || y.abs() > 1. {
                    return Err(Error::input("pan fractions must be within -1..1"));
                }
                let shift = |n: f64, px: u32| {
                    if snap {
                        (n * f64::from(px) / 16.).round_ties_even() * 16. / f64::from(px)
                    } else {
                        n
                    }
                };
                let (dx, dy) = (shift(x, self.width) * sx, shift(y, self.height) * sy);
                Self::new(
                    [x0 + dx, y0 + dy, x1 + dx, y1 + dy],
                    self.width,
                    self.height,
                )
            }
            Navigation::Zoom { factor, anchor } => {
                if !factor.is_finite()
                    || !(0.125..=8.).contains(&factor)
                    || !anchor
                        .iter()
                        .all(|n| n.is_finite() && (0. ..=1.).contains(n))
                {
                    return Err(Error::input("zoom factor/anchor is invalid"));
                }
                // Browser anchor is fraction from the TOP left, world Y points up.
                let ax = x0 + anchor[0] * sx;
                let ay = y1 - anchor[1] * sy;
                Self::new(
                    [
                        ax - anchor[0] * sx * factor,
                        ay - (1. - anchor[1]) * sy * factor,
                        ax + (1. - anchor[0]) * sx * factor,
                        ay + anchor[1] * sy * factor,
                    ],
                    self.width,
                    self.height,
                )
            }
        }
    }
    fn resize(&self, width: u32, height: u32) -> Result<Self> {
        let b = self.bbox;
        Self::centered(
            b[0] + (b[2] - b[0]) / 2.,
            b[1] + (b[3] - b[1]) / 2.,
            (b[2] - b[0]) / f64::from(self.width) * f64::from(width),
            width,
            height,
        )
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Navigation {
    Fit,
    Goto { center_um: [f64; 2], width_um: f64 },
    Pan { x: f64, y: f64, snap: bool },
    Zoom { factor: f64, anchor: [f64; 2] },
}
#[derive(Clone, Copy, Debug)]
pub enum Depth {
    Full,
    Levels(u32),
}
#[derive(Clone, Debug)]
pub enum LayerIsolation {
    /// Remember the original visibility once, even across subsequent isolates.
    Set(Layers),
    Restore,
}
#[derive(Default, Clone, Debug)]
pub struct Patch {
    pub navigation: Option<Navigation>,
    pub pixels: Option<(u32, u32)>,
    pub depth: Option<Depth>,
    pub detail: Option<Detail>,
    pub thin: Option<Thin>,
    pub layers: Option<Layers>,
    /// A panel checkbox is a bounded delta, not a round-trip of all layer IDs.
    pub layer_change: Option<((u32, u32), bool)>,
    pub layer_isolation: Option<LayerIsolation>,
    pub frames: Option<bool>,
    pub labels: Option<bool>,
    pub font_px: Option<u32>,
    pub mono: Option<bool>,
    pub style_changes: Vec<Style>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ViewState {
    pub viewport: Viewport,
    pub depth: Option<u32>,
    pub detail: Detail,
    pub thin: Thin,
    pub layers: Layers,
    // Controller snapshots are frequent; never copy the saved pair list on a
    // poll. This is view state, not a raster policy or a browser-owned backup.
    isolated_from: Option<Arc<Layers>>,
    pub frames: bool,
    pub labels: bool,
    pub font_px: u32,
    pub mono: bool,
    pub styles: Arc<Vec<Style>>,
}
/// Immutable metadata sufficient for validation, without retaining a second
/// cache mapping or leaking source paths into a transport snapshot.
pub struct Model {
    pub dataset_revision: u64,
    pub dbu: f64,
    pub bbox: [f64; 4],
    pub deck: bool,
    pub skipped: usize,
    pub source_stale: bool,
    pub styles: Arc<Vec<Style>>,
    pairs: BTreeSet<(u32, u32)>,
    groups: BTreeMap<(u32, u32), Vec<(u32, u32)>>,
}
impl Model {
    pub fn new(data: &ManagedDataset) -> Result<Arc<Self>> {
        let d = &data.dataset;
        let styles = Arc::new(d.styles(false)?);
        let mut groups = BTreeMap::new();
        if let Dataset::Deck(deck) = d {
            for r in &deck.metadata.layers {
                if r.jobdeck_head {
                    groups.insert((r.layer as u32, r.datatype as u32), Vec::new());
                }
            }
            for r in &deck.metadata.layers {
                if !r.jobdeck_head {
                    if let Some(g) = groups.get_mut(&(r.layer as u32, 0)) {
                        g.push((r.layer as u32, r.datatype as u32));
                    }
                }
            }
        }
        let mut pairs: BTreeSet<_> = styles.iter().map(|s| s.layer).collect();
        pairs.extend(groups.keys().copied());
        Ok(Arc::new(Self {
            dataset_revision: data.revision,
            dbu: d.dbu(),
            bbox: d.bbox().map(|n| n as f64),
            deck: d.is_deck(),
            skipped: d.skipped().len(),
            source_stale: d.source_stale(),
            styles,
            pairs,
            groups,
        }))
    }
    fn layers(&self, layers: &Layers) -> Result<Layers> {
        let Layers::Only(pairs) = layers else {
            return Ok(layers.clone());
        };
        if pairs.is_empty() || pairs.len() > 4096 {
            return Err(Error::input(
                "explicit layer selection requires 1..4096 pairs",
            ));
        }
        let mut out = BTreeSet::new();
        for pair in pairs {
            if !self.pairs.contains(pair) {
                return Err(Error::input("unknown layer pair"));
            }
            if let Some(group) = self.groups.get(pair) {
                out.extend(group);
            } else {
                out.insert(*pair);
            }
            if out.len() > 4096 {
                return Err(Error::input(
                    "expanded layer selection exceeds 4096; use all or narrower groups",
                ));
            }
        }
        Ok(if out.is_empty() {
            Layers::None
        } else {
            Layers::Only(out.into_iter().collect())
        })
    }
}
impl ViewState {
    pub fn initial(model: &Model, width: u32, height: u32) -> Result<Self> {
        Ok(Self {
            viewport: Viewport::fit(model.bbox, width, height)?,
            depth: None,
            detail: Detail::Medium,
            thin: Thin::Auto,
            layers: Layers::All,
            isolated_from: None,
            frames: false,
            labels: !model.deck,
            font_px: 14,
            mono: false,
            styles: Arc::clone(&model.styles),
        })
    }
    pub fn edit(&self, model: &Model, patch: Patch) -> Result<Self> {
        if patch.layer_isolation.is_some()
            && (patch.layers.is_some() || patch.layer_change.is_some())
        {
            return Err(Error::input(
                "isolation conflicts with explicit layer edits",
            ));
        }
        let mut s = self.clone();
        if let Some((w, h)) = patch.pixels {
            s.viewport = s.viewport.resize(w, h)?;
        }
        if let Some(nav) = patch.navigation {
            s.viewport = s.viewport.navigate(nav, model.bbox, model.dbu)?;
        }
        if let Some(depth) = patch.depth {
            s.depth = match depth {
                Depth::Full => None,
                Depth::Levels(n) => Some(n),
            };
        }
        if let Some(detail) = patch.detail {
            s.detail = detail;
        }
        if let Some(thin) = patch.thin {
            s.thin = thin;
        }
        if let Some(layers) = patch.layers {
            s.layers = model.layers(&layers)?;
        }
        if let Some((pair, visible)) = patch.layer_change {
            let target = model.layers(&Layers::Only(vec![pair]))?;
            let Layers::Only(target) = target else {
                return Err(Error::input("layer group has no renderable children"));
            };
            let mut selected: BTreeSet<_> = match &s.layers {
                Layers::All => s
                    .styles
                    .iter()
                    .map(|r| r.layer)
                    .filter(|p| !model.groups.contains_key(p))
                    .collect(),
                Layers::None => BTreeSet::new(),
                Layers::Only(pairs) => pairs.iter().copied().collect(),
            };
            for pair in target {
                if visible {
                    selected.insert(pair);
                } else {
                    selected.remove(&pair);
                }
            }
            let render_pairs = s
                .styles
                .iter()
                .filter(|s| !model.groups.contains_key(&s.layer))
                .count();
            s.layers = if selected.len() == render_pairs {
                Layers::All
            } else if selected.is_empty() {
                Layers::None
            } else {
                model.layers(&Layers::Only(selected.into_iter().collect()))?
            };
        }
        if let Some(isolation) = patch.layer_isolation {
            match isolation {
                LayerIsolation::Set(layers) => {
                    let layers = model.layers(&layers)?;
                    if layers == Layers::None {
                        return Err(Error::input(
                            "isolation requires a nonempty layer selection",
                        ));
                    }
                    if s.isolated_from.is_none() {
                        s.isolated_from = Some(Arc::new(s.layers.clone()));
                    }
                    s.layers = layers;
                }
                LayerIsolation::Restore => {
                    if let Some(saved) = s.isolated_from.take() {
                        s.layers = (*saved).clone();
                    }
                }
            }
        }
        if let Some(v) = patch.frames {
            s.frames = v;
        }
        if let Some(v) = patch.labels {
            s.labels = v;
        }
        if let Some(v) = patch.font_px {
            s.font_px = v;
        }
        if let Some(v) = patch.mono {
            s.mono = v;
        }
        if patch.style_changes.len() > 4096 {
            return Err(Error::input("too many style changes"));
        }
        if !patch.style_changes.is_empty() {
            let mut changes = BTreeMap::new();
            for style in patch.style_changes {
                if style.width == 0
                    || style.width > 8
                    || style.color[3] != 255
                    || !model.pairs.contains(&style.layer)
                    || changes.insert(style.layer, style).is_some()
                {
                    return Err(Error::input("invalid, duplicate or unknown style"));
                }
            }
            let mut expanded = BTreeMap::new();
            for (key, style) in &changes {
                if let Some(children) = model.groups.get(key) {
                    for &layer in children {
                        expanded.insert(
                            layer,
                            Style {
                                layer,
                                ..style.clone()
                            },
                        );
                    }
                }
            }
            // An explicit child in this transaction wins over its group.
            expanded.extend(changes);
            let styles = s
                .styles
                .iter()
                .map(|old| expanded.get(&old.layer).unwrap_or(old).clone())
                .collect();
            s.styles = Arc::new(styles);
        }
        s.validate(model)?;
        Ok(s)
    }
    pub fn validate(&self, model: &Model) -> Result<()> {
        self.viewport.validate()?;
        if (model.deck && self.labels) || !(6..=96).contains(&self.font_px) {
            return Err(Error::input("unsupported label policy"));
        }
        if self.layers != model.layers(&self.layers)? {
            return Err(Error::input("layers must be normalized"));
        }
        if let Some(saved) = &self.isolated_from {
            if **saved != model.layers(saved)? {
                return Err(Error::input("saved layers must be normalized"));
            }
        }
        if self.styles.len() != model.styles.len()
            || self.styles.iter().zip(model.styles.iter()).any(|(a, b)| {
                a.layer != b.layer || a.width == 0 || a.width > 8 || a.color[3] != 255
            })
        {
            return Err(Error::input("style keys or values are invalid"));
        }
        Ok(())
    }
    pub fn layers_isolated(&self) -> bool {
        self.isolated_from.is_some()
    }
    pub fn same_policy(&self, other: &Self, deck: bool) -> bool {
        // Saving/restoring unchanged visibility must not invalidate geometry.
        self.depth == other.depth
            && self.detail == other.detail
            && self.thin.effective(deck) == other.thin.effective(deck)
            && self.layers == other.layers
            && self.frames == other.frames
            && self.labels == other.labels
            && self.font_px == other.font_px
            && self.mono == other.mono
            && self.styles == other.styles
    }
    pub fn request(&self, model: &Model, base: RenderRequest) -> RenderRequest {
        RenderRequest {
            view: self.viewport.bbox,
            width: self.viewport.width,
            height: self.viewport.height,
            depth: self.depth,
            cut_px: self.detail.cut_px(),
            thin: self.thin.effective(model.deck),
            layers: self.layers.clone(),
            frames: self.frames,
            labels: self.labels,
            font_px: self.font_px,
            mono: self.mono,
            ..base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layer_checkbox_delta_expands_groups_without_losing_other_selections() {
        let styles = Arc::new(
            vec![(1, 0), (1, 1), (1, 2), (2, 0)]
                .into_iter()
                .map(|layer| Style {
                    layer,
                    color: [255; 4],
                    fill: floe_worker_client::Fill::Solid,
                    width: 1,
                })
                .collect::<Vec<_>>(),
        );
        let model = Model {
            dataset_revision: 1,
            dbu: 1.,
            bbox: [0., 0., 100., 100.],
            deck: true,
            skipped: 0,
            source_stale: false,
            pairs: [(1, 0), (1, 1), (1, 2), (2, 0)].into(),
            groups: [((1, 0), vec![(1, 1), (1, 2)])].into(),
            styles,
        };
        let all = ViewState::initial(&model, 100, 100).unwrap();
        let partial = all
            .edit(
                &model,
                Patch {
                    layer_change: Some(((1, 1), false)),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(partial.layers, Layers::Only(vec![(1, 2), (2, 0)]));
        let group = partial
            .edit(
                &model,
                Patch {
                    layer_change: Some(((1, 0), false)),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(group.layers, Layers::Only(vec![(2, 0)]));
        assert_eq!(
            group
                .edit(
                    &model,
                    Patch {
                        layer_change: Some(((1, 0), true)),
                        ..Default::default()
                    }
                )
                .unwrap()
                .layers,
            Layers::All
        );
        assert!(all
            .edit(
                &model,
                Patch {
                    layer_change: Some(((9, 9), false)),
                    ..Default::default()
                }
            )
            .is_err());
    }
    #[test]
    fn pan_phase_zoom_anchor_and_resize_stay_server_side() {
        let v = Viewport::new([-100., -50., 700., 550.], 800, 600).unwrap();
        let p = v
            .navigate(
                Navigation::Pan {
                    x: 0.5,
                    y: 0.1,
                    snap: true,
                },
                [0.; 4],
                1.,
            )
            .unwrap();
        assert_eq!(p.bbox, [300., 14., 1100., 614.]);
        let z = v
            .navigate(
                Navigation::Zoom {
                    factor: 0.8,
                    anchor: [0., 0.],
                },
                [0.; 4],
                1.,
            )
            .unwrap();
        assert_eq!(z.bbox, [-100., 70., 540., 550.]);
        let r = v.resize(1600, 1200).unwrap();
        assert_eq!(r.bbox, [-500., -350., 1100., 850.]);
    }
    #[test]
    fn hostile_or_unrepresentable_coordinates_are_rejected() {
        for b in [
            [0., 0., f64::INFINITY, 1.],
            [0., 0., 0., 1.],
            [-1e99, 0., 1e99, 1.],
        ] {
            assert!(Viewport::new(b, 100, 100).is_err());
        }
        assert!(Viewport::new([0., 0., 1., 1.], 8192, 8192).is_err());
        let v = Viewport::new([0., 0., 1., 1.], 100, 100).unwrap();
        assert!(v
            .navigate(
                Navigation::Zoom {
                    factor: f64::NAN,
                    anchor: [0.5, 0.5]
                },
                [0.; 4],
                1.
            )
            .is_err());
        assert!(v
            .navigate(
                Navigation::Pan {
                    x: 2.,
                    y: 0.,
                    snap: false
                },
                [0.; 4],
                1.
            )
            .is_err());
        assert!(v
            .navigate(
                Navigation::Goto {
                    center_um: [1e18, 1e18],
                    width_um: 0.001
                },
                [0.; 4],
                1.
            )
            .is_err());
    }
}
