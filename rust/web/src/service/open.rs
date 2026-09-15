//! A normal open and an approved post-index open share the same cutover.
use super::*;

pub(super) struct Anchor {
    previous: Option<(Arc<Attachment>, u64)>,
    window_display: WindowDisplay,
}

pub(super) fn anchor(inner: &Inner, replace: &Option<(String, u64)>) -> Result<Anchor> {
    let s = inner.state.lock().unwrap();
    if s.closed {
        return Err(Error::new(ErrorKind::Cancelled, "owner closed"));
    }
    // Read the preferences and CAS revision from ONE snapshot.
    // A concurrent edit between two reads must not validate a new
    // revision while inheriting the older revision's preferences.
    let old = s.view.as_ref().map(|v| (v, v.controller.snapshot()));
    let window_display = old.as_ref().map_or_else(
        || s.window_display.clone(),
        |(v, snapshot)| {
            s.window_display
                .capture(&snapshot.state, v.controller.model.deck)
        },
    );
    let previous = if let Some((id, rev)) = replace {
        let (v, snapshot) = old
            .as_ref()
            .filter(|(v, _)| v.id == *id)
            .ok_or_else(|| Error::new(ErrorKind::Busy, "launcher view changed"))?;
        if snapshot.state_rev != *rev
            || !matches!(
                snapshot.phase,
                floe_app_core::view::Phase::Idle | floe_app_core::view::Phase::Rendering
            )
        {
            return Err(Error::new(
                ErrorKind::Busy,
                "launcher view revision changed",
            ));
        }
        Some((Arc::clone(v), *rev))
    } else if s.view.as_ref().is_some_and(|v| !v.controller.is_finished()) {
        return Err(Error::new(
            ErrorKind::Busy,
            "close the current view before opening another",
        ));
    } else {
        None
    };
    Ok(Anchor {
        previous,
        window_display,
    })
}

pub(super) fn execute(
    inner: &Inner,
    seq: u64,
    command: OpenCommand,
    stop: Arc<AtomicUsize>,
    index: Option<&Value>,
) -> Result<Value> {
    let OpenCommand {
        source,
        source_id,
        levels,
        mode,
        patch,
        replace,
        display_policy,
        label_preference,
    } = command;
    let mode_name = match mode {
        Mode::Level => "level",
        Mode::Chip => "chip",
        Mode::Layer => "layer",
    };
    let selected_levels: Option<Vec<String>> = levels
        .as_ref()
        .map(|ids| ids.iter().map(i64::to_string).collect());
    let Anchor {
        previous,
        window_display,
    } = anchor(inner, &replace)?;
    let window_display = window_display.label_preference(label_preference);

    inner.state.lock().unwrap().ledger.update(
        seq,
        super::index_open::open_state(
            json!({"seq":seq.to_string(),"kind":"open","phase":"opening"}),
            index,
        ),
        false,
    );
    source.validate(&stop)?;
    if let Some((previous, rev)) = &previous {
        if previous.source_id == source_id
            && previous.mode == mode_name
            && previous.levels == selected_levels
        {
            let mut s = inner.state.lock().unwrap();
            if s.closed || stop.load(Ordering::Relaxed) != 0 {
                return Err(Error::new(ErrorKind::Cancelled, "launcher edit cancelled"));
            }
            if !s.view.as_ref().is_some_and(|v| Arc::ptr_eq(v, previous)) {
                return Err(Error::new(ErrorKind::Busy, "launcher view changed"));
            }
            // Preserve native/decoded/retained caches for same-source
            // navigation. Only explicitly supplied preferences change.
            let snapshot = previous.controller.edit(*rev, *patch)?;
            s.window_display =
                window_display.capture(&snapshot.state, previous.controller.model.deck);
            return Ok(
                json!({"seq":seq.to_string(),"kind":"open","phase":"succeeded",
                "view_id":previous.id,"reused":true,"state_rev":snapshot.state_rev.to_string()}),
            );
        }
    }
    let data = ManagedDataset::open(&inner.resources, source.path(), levels, mode, &stop)?;
    let model = Model::new(&data)?;
    let (width, height) = patch.pixels.unwrap_or((1024, 768));
    let initial = ViewState::initial(&model, width, height)?;
    let initial = match display_policy {
        OpenDisplay::Explicit => initial,
        OpenDisplay::Window => initial.edit(&model, window_display.patch(model.deck))?,
    }
    .edit(&model, *patch)?;
    let remembered = window_display.capture(&initial, model.deck);
    let rows = LayerCatalog::dataset(&data.dataset, &model);
    // Prepare a dormant controller with the existing reservation; the
    // old worker is fully reaped before the new one opens. Metadata,
    // validation or stale-revision failures leave the old view alive.
    let mut replacement = if let Some((previous, _)) = &previous {
        Some(
            previous
                .controller
                .prepare_replacement(Arc::clone(&data), initial.clone())?,
        )
    } else {
        None
    };
    let controller = if let Some(replacement) = &replacement {
        replacement.controller()
    } else {
        Arc::new(ViewController::start_configured(
            &inner.resources,
            data,
            inner.options.clone(),
            initial,
            inner.view_options,
        )?)
    };
    let mut view = Attachment::with_rows(controller, &source.title, rows)
        .map_err(|_| Error::new(ErrorKind::Io, "entropy unavailable"))?;
    view.source_id = source_id;
    view.levels = selected_levels;
    view.mode = mode_name;
    let view = Arc::new(view);
    let mut s = inner.state.lock().unwrap();
    if s.closed || stop.load(Ordering::Relaxed) != 0 {
        view.controller.request_close();
        drop(s);
        drop(view);
        return Err(Error::new(ErrorKind::Cancelled, "open cancelled"));
    }
    if let Some((previous, rev)) = previous {
        if !s.view.as_ref().is_some_and(|v| Arc::ptr_eq(v, &previous)) {
            return Err(Error::new(
                ErrorKind::Busy,
                "launcher view changed before cutover",
            ));
        }
        replacement
            .as_mut()
            .expect("prepared launcher replacement")
            .commit(rev)?;
    }
    let id = view.id.clone();
    s.window_display = remembered;
    s.view = Some(view);
    Ok(json!({"seq":seq.to_string(),"kind":"open","phase":"succeeded","view_id":id}))
}
