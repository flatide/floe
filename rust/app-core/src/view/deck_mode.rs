//! State preparation only: no worker stop/start or publication. The service
//! must commit the returned state/memory together after a revision-CAS check.
use super::{Model, ViewState};
use crate::{
    dataset::Dataset,
    jobdeck::{color::Mode, dataset::DeckSnapshot},
    managed::ManagedDataset,
    Error, Result,
};
use floe_worker_client::Layers;
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    source: PathBuf,
    levels: Option<BTreeSet<i64>>,
    size: u64,
    mtime: u64,
}
impl Identity {
    fn of(deck: &DeckSnapshot) -> Self {
        Self {
            source: deck.source.clone(),
            levels: deck.metadata.jobdeck.levels.clone(),
            size: deck.metadata.src.size,
            mtime: deck.metadata.src.mtime,
        }
    }
}

/// Exact leaf visibility shared by level/chip; raw source layers have a
/// separate namespace even when numeric pairs happen to overlap. This memory
/// belongs to ONE loaded deck/level selection, never a global source cache.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeckModeMemory {
    identity: Option<Arc<Identity>>,
    visibility: [Option<Arc<Layers>>; 2],
}
pub struct PreparedDeckMode {
    pub model: Arc<Model>,
    pub state: ViewState,
    pub memory: DeckModeMemory,
}
fn scope(mode: Mode) -> usize {
    usize::from(mode == Mode::Layer)
}
impl DeckModeMemory {
    pub fn prepare(
        &self,
        current: &ManagedDataset,
        model: &Model,
        state: &ViewState,
        target: &ManagedDataset,
    ) -> Result<PreparedDeckMode> {
        let (Dataset::Deck(from), Dataset::Deck(to)) = (&current.dataset, &target.dataset) else {
            return Err(Error::input("mode transition requires a jobdeck"));
        };
        let identity = Identity::of(from);
        if identity != Identity::of(to)
            || self
                .identity
                .as_deref()
                .is_some_and(|saved| *saved != identity)
            || model.dataset_revision != current.revision
            || from.metadata.dbu != to.metadata.dbu
            || from.metadata.bbox != to.metadata.bbox
        {
            return Err(Error::input(
                "mode transition changed deck identity or coordinates",
            ));
        }
        if from.metadata.jobdeck.mode == to.metadata.jobdeck.mode {
            return Err(Error::input("unchanged mode must be handled as a no-op"));
        }
        state.validate(model)?;
        let next_model = Model::new(target)?;
        let mut memory = self.clone();
        memory.identity = Some(Arc::new(identity));
        memory.visibility[scope(from.metadata.jobdeck.mode)] = Some(Arc::new(state.layers.clone()));
        // Mode-specific defaults own colors/fill/width. They must not be
        // copied by numeric pair into the independent raw-layer namespace.
        let mut next =
            ViewState::initial(&next_model, state.viewport.width, state.viewport.height)?;
        if let Some(saved) = &memory.visibility[scope(to.metadata.jobdeck.mode)] {
            next.layers = next_model.layers(saved)?;
        }
        next.viewport = state.viewport;
        next.depth = state.depth;
        next.detail = state.detail;
        next.thin = state.thin;
        next.frames = state.frames;
        next.labels = state.labels;
        next.font_px = state.font_px;
        // Unlike GTK's generic _apply_cache, a mode-only edit need not turn
        // off mono. Keep view controls, but drop old isolation/geometry IDs.
        next.mono = state.mono;
        next.validate(&next_model)?;
        Ok(PreparedDeckMode {
            model: next_model,
            state: next,
            memory,
        })
    }
}
