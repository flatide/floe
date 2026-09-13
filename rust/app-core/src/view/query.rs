//! Display-anchored queries. No frame bytes, file paths or network authority.
use super::{margin, DisplayFrame, Model, Snapshot, Viewport};
use crate::{Error, ErrorKind, Result};
use floe_worker_client::{Layers, QueryKind, QueryOperation, QueryReply, QueryRequest, QueryScene};
use std::{collections::BTreeMap, sync::Arc};

/// All counters are scoped to one controller/dataset/worker lifetime. A frame
/// ID alone does not describe the current crop of a retained margin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryAnchor {
    pub dataset_revision: u64,
    pub worker_epoch: u64,
    pub frame_id: u64,
    pub state_rev: u64,
    pub render_rev: u64,
    pub render_key: u64,
}
#[derive(Clone, Debug)]
pub struct ViewQuery {
    pub anchor: QueryAnchor,
    pub operation: QueryOperation,
    /// Fraction of the CURRENT viewport, from its top-left (not margin pixels).
    pub position: [f64; 2],
    /// Native device pixels. CSS/DPR conversion belongs at the input boundary.
    pub radius_px: f64,
    /// All means the current visible selection. Explicit pairs must be visible.
    pub layers: Layers,
}
#[derive(Clone, Debug)]
pub struct ViewQueryResult {
    pub id: u64,
    pub anchor: QueryAnchor,
    /// Native errors are local diagnostics; transports must map safe codes.
    pub reply: QueryReply,
}
#[derive(Clone, Debug, Default)]
pub struct QuerySnapshot {
    pub accepted: u64,
    pub submitted: u64,
    pub consumed: u64,
    pub discarded: u64,
    pub queued: usize,
    pub in_flight: usize,
    pub snap_id: Option<u64>,
    pub pick_id: Option<u64>,
    pub snap: Option<Arc<ViewQueryResult>>,
    pub pick: Option<Arc<ViewQueryResult>>,
}

pub(super) fn slot(kind: QueryKind) -> usize {
    match kind {
        QueryKind::Snap => 0,
        QueryKind::Pick => 1,
    }
}
pub(super) const KINDS: [QueryKind; 2] = [QueryKind::Snap, QueryKind::Pick];
pub(super) const IN_FLIGHT_PER_KIND: usize = 2;

#[derive(Clone)]
pub(super) struct Ticket {
    pub id: u64,
    pub input: ViewQuery,
    pub native: QueryRequest,
}
#[derive(Default)]
pub(super) struct Slot {
    pub latest: Option<u64>,
    pub pending: Option<Ticket>,
    pub result: Option<Arc<ViewQueryResult>>,
    pub cancel: bool,
}
#[derive(Clone, PartialEq)]
struct GeometryKey {
    depth: Option<u32>,
    cut_px: f64,
    layers: Layers,
    frames: bool,
    mono: bool,
    decode_pages: Option<usize>,
    style_epoch: u64,
    thin: floe_worker_client::ThinPolicy,
}
impl GeometryKey {
    fn of(f: &DisplayFrame) -> Result<Self> {
        let r = &f.frame.request;
        Ok(Self {
            depth: r.depth,
            cut_px: r.cut_px,
            layers: r.layers.clone(),
            frames: r.frames,
            mono: r.mono,
            decode_pages: r.decode_pages,
            style_epoch: f.frame.fields.u64("style_epoch")?,
            thin: r.thin,
        })
    }
}
#[derive(Clone)]
pub(super) struct Source {
    pub context: QueryScene,
    pub key: u64,
    pub viewport: Viewport,
    geometry: GeometryKey,
}
#[derive(Default)]
pub(super) struct Queries {
    pub slots: [Slot; 2],
    pub in_flight: BTreeMap<u64, Ticket>,
    pub source: Option<Source>,
    accepted: u64,
    pub submitted: u64,
    pub consumed: u64,
    pub discarded: u64,
}
impl Queries {
    pub fn latest_anchor(&self, kind: QueryKind) -> Option<QueryAnchor> {
        let s = &self.slots[slot(kind)];
        let id = s.latest?;
        s.pending
            .as_ref()
            .filter(|t| t.id == id)
            .map(|t| t.input.anchor)
            .or_else(|| s.result.as_ref().filter(|r| r.id == id).map(|r| r.anchor))
            .or_else(|| {
                self.in_flight
                    .values()
                    .find(|t| t.id == id)
                    .map(|t| t.input.anchor)
            })
    }
    pub fn snapshot(&self) -> QuerySnapshot {
        QuerySnapshot {
            accepted: self.accepted,
            submitted: self.submitted,
            consumed: self.consumed,
            discarded: self.discarded,
            queued: self.slots.iter().filter(|s| s.pending.is_some()).count(),
            in_flight: self.in_flight.len(),
            snap_id: self.slots[0].latest,
            pick_id: self.slots[1].latest,
            snap: self.slots[0].result.clone(),
            pick: self.slots[1].result.clone(),
        }
    }
    pub fn enqueue(&mut self, input: ViewQuery, native: QueryRequest) -> Result<u64> {
        let id = self
            .accepted
            .checked_add(1)
            .ok_or_else(|| Error::input("query ID exhausted"))?;
        let s = &mut self.slots[slot(input.operation.kind())];
        if s.pending.is_some() {
            self.discarded = self.discarded.saturating_add(1);
        }
        s.latest = Some(id);
        s.pending = Some(Ticket { id, input, native });
        s.result = None;
        self.accepted = id;
        Ok(id)
    }
    pub fn cancel(&mut self, kind: QueryKind) {
        let s = &mut self.slots[slot(kind)];
        if s.pending.take().is_some() {
            self.discarded = self.discarded.saturating_add(1);
        }
        s.latest = None;
        s.result = None;
        s.cancel = true;
    }
    pub fn invalidate(&mut self) {
        for kind in KINDS {
            self.cancel(kind);
        }
    }
    pub fn discard(&mut self, t: &Ticket) {
        self.discarded = self.discarded.saturating_add(1);
        let s = &mut self.slots[slot(t.input.operation.kind())];
        if s.latest == Some(t.id) {
            s.latest = None;
            s.result = None;
        }
    }
    pub fn observe(&mut self, f: &DisplayFrame) -> Result<()> {
        let context = f.frame.query_scene()?;
        let geometry = GeometryKey::of(f)?;
        if let Some(previous) = self
            .source
            .as_mut()
            .filter(|s| s.context.id.is_some() && s.context.id == context.id)
        {
            if previous.geometry != geometry
                || previous.context != context
                || margin::origin(previous.viewport, f.viewport()).is_none()
            {
                return Err(Error::new(
                    ErrorKind::Worker,
                    "immutable query scene metadata changed",
                ));
            }
            // A label-only foreground references the old, wider geometry.
            // Do not shrink its known bounds to the foreground request.
            // Label/font changes advance the UI key, not native geometry.
            previous.key = f.render_key;
            if !margin::covers(previous.viewport, f.viewport()) {
                // The first observed frame may already reference an older
                // scene whose full bounds were not seen. Keep a proven bound.
                previous.viewport = f.viewport();
            }
            return Ok(());
        }
        self.source = Some(Source {
            context,
            key: f.render_key,
            viewport: f.viewport(),
            geometry,
        });
        Ok(())
    }
}

impl QueryAnchor {
    pub(super) fn new(f: &DisplayFrame, s: &Snapshot) -> Self {
        Self {
            dataset_revision: f.dataset_revision,
            worker_epoch: s.worker_epoch,
            frame_id: f.id,
            state_rev: s.state_rev,
            render_rev: s.render_rev,
            render_key: s.render_key,
        }
    }
    pub(super) fn matches(self, f: &DisplayFrame, s: &Snapshot, model: &Model) -> bool {
        self == Self::new(f, s)
            && self.dataset_revision == model.dataset_revision
            && f.matches(s)
            && margin::covers(f.viewport(), s.state.viewport)
    }
}

pub(super) fn request(
    input: &ViewQuery,
    state: &Snapshot,
    model: &Model,
    source: &Source,
) -> Result<QueryRequest> {
    if source.key != state.render_key || !margin::covers(source.viewport, state.state.viewport) {
        return Err(Error::new(
            ErrorKind::Busy,
            "published query scene does not cover this view",
        ));
    }
    let scene = source
        .context
        .id
        .ok_or_else(|| Error::new(ErrorKind::Unsupported, "scene has no query geometry"))?;
    if !source.context.complete {
        return Err(Error::new(
            ErrorKind::Incomplete,
            "published query scene is incomplete",
        ));
    }
    if !input
        .position
        .iter()
        .all(|n| n.is_finite() && (0.0..=1.0).contains(n))
        || !input.radius_px.is_finite()
        || !(0.0..=64.0).contains(&input.radius_px)
    {
        return Err(Error::input(
            "query requires viewport fractions and radius in 0..64 device pixels",
        ));
    }
    let v = state.state.viewport;
    let [x0, y0, x1, y1] = v.bbox;
    // Preserve GTK's int(world_coordinate), including negative coordinates.
    // Do the conversion here, not in browser floating-point world arithmetic.
    let integer = |n: f64| -> Result<i64> {
        if !n.is_finite() || n < i64::MIN as f64 || n >= -(i64::MIN as f64) {
            return Err(Error::input("query coordinate overflow"));
        }
        Ok(n as i64)
    };
    let x = integer(x0 + input.position[0] * (x1 - x0))?;
    let y = integer(y1 - input.position[1] * (y1 - y0))?;
    let scale = ((x1 - x0) / f64::from(v.width)).max((y1 - y0) / f64::from(v.height));
    let radius = integer(input.radius_px * scale)?.max(1);
    let selected = model.layers(&input.layers)?;
    let layers = match selected {
        Layers::All => state.state.layers.clone(),
        Layers::None => Layers::None,
        Layers::Only(pairs) => {
            if pairs.iter().any(|p| match &state.state.layers {
                Layers::All => false,
                Layers::None => true,
                Layers::Only(visible) => !visible.contains(p),
            }) {
                return Err(Error::input("query layer is not visible"));
            }
            Layers::Only(pairs)
        }
    };
    Ok(QueryRequest {
        scene,
        operation: input.operation,
        x,
        y,
        radius,
        layers,
    })
}
