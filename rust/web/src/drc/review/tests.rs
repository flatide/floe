use super::*;

pub(super) fn service() -> Arc<Service> {
    configured_service(true)
}
fn configured_service(editable: bool) -> Arc<Service> {
    Service::start(Config {
        kind: store::Kind::Notes,
        editable,
        read_target: None,
        reader_id: None,
        reviewer: "fixed".into(),
        files: vec![],
        trees: vec![],
        sources: SourceSet::new(vec![]).unwrap(),
        notes_display: Weak::new(),
    })
    .unwrap()
}
#[test]
fn reconnect_is_atomic_and_keeps_both_ledgers_without_regranting_authority() {
    for editable in [false, true] {
        let s = configured_service(editable);
        let original_id = s.status()["binding_id"].as_str().unwrap().to_owned();
        let actor = owner();
        let req = request();
        {
            let mut state = s.inner.state.lock().unwrap();
            state
                .ledger
                .admit_scoped(
                    1,
                    Service::signature(&actor, &req),
                    "drc_note",
                    Some(&original_id),
                )
                .unwrap();
            state
                .ledger
                .update(1, json!({"seq":"1","phase":"succeeded"}), true);
            state
                .transfer
                .ledger
                .admit_scoped(1, "old download".into(), "transfer", Some(&original_id))
                .unwrap();
        }
        assert!(matches!(s.admit_detach(|| Ok(())), Err("drc_busy")));
        s.inner.state.lock().unwrap().transfer.ledger.update(
            1,
            json!({"seq":"1","phase":"succeeded"}),
            true,
        );
        s.admit_detach(|| Ok(())).unwrap();
        let before = s.status();
        let transfers = s.inner.state.lock().unwrap().transfer.ledger.snapshot();
        assert!(matches!(
            s.admit_reconnect(Binding::new(None, None).unwrap(), || Err::<(), _>(
                "drc_context_changed"
            )),
            Err("drc_context_changed")
        ));
        assert_eq!(s.status(), before);
        s.admit_reconnect(Binding::new(None, None).unwrap(), || Ok(()))
            .unwrap();
        let after = s.status();
        assert_ne!(after["binding_id"], before["binding_id"]);
        assert_eq!(after["review_rev"], "1");
        assert_eq!(after["operations"], before["operations"]);
        assert_eq!(after["editable"], editable);
        assert_eq!(after["reviewer"], "fixed");
        assert_eq!(after["autosave"], false);
        assert_eq!(
            s.inner.state.lock().unwrap().transfer.ledger.snapshot(),
            transfers
        );
        assert_eq!(
            s.replay(&actor, &req).unwrap().unwrap()["scope_id"],
            original_id
        );
        assert!(matches!(
            s.admit_reconnect(Binding::new(None, None).unwrap(), || Ok(())),
            Err("review_registration_changed")
        ));
        stop(&s);
    }
}
#[test]
fn read_registration_cannot_submit_or_acquire_editor_authority() {
    let s = configured_service(false);
    assert_eq!(s.status()["editable"], false);
    assert_eq!(s.status()["available"], true);
    assert!(matches!(s.require_editor(), Err("review_disabled")));
    assert!(matches!(
        s.submit(&owner(), request()),
        Err("review_disabled")
    ));
    assert_eq!(s.status()["operations"]["last_seq"], "0");
    stop(&s);
}
#[test]
fn detach_preserves_receipts_but_cannot_prepare_or_migrate_a_writer() {
    let s = service();
    let owner = owner();
    let request = request();
    let body = Arc::new(Semaphore::new(1));
    let op = s
        .begin(Arc::new(Arc::clone(&body).try_acquire_owned().unwrap()))
        .unwrap();
    assert!(matches!(s.admit_detach(|| Ok(())), Err("drc_busy")));
    assert_eq!(s.status()["detached"], false);
    drop(op);
    assert!(matches!(
        s.admit_detach(|| Err::<(), _>("drc_context_changed")),
        Err("drc_context_changed")
    ));
    assert_eq!(s.status()["detached"], false);
    let saved = json!({"seq":"1","phase":"succeeded","published":true});
    {
        let mut state = s.inner.state.lock().unwrap();
        state
            .ledger
            .admit(1, Service::signature(&owner, &request), "drc_note")
            .unwrap();
        assert_eq!(state.ledger.active(), Some(1));
    }
    assert!(matches!(s.admit_detach(|| Ok(())), Err("drc_busy")));
    s.inner
        .state
        .lock()
        .unwrap()
        .ledger
        .update(1, saved.clone(), true);
    s.admit_detach(|| Ok(())).unwrap();
    assert_eq!(s.status()["available"], false);
    assert_eq!(s.status()["editable"], false);
    assert_eq!(s.operation(1), Some(saved.clone()));
    assert_eq!(s.replay(&owner, &request).unwrap(), Some(saved.clone()));
    assert_eq!(s.submit(&owner, request).unwrap(), saved);
    assert!(matches!(
        s.begin(Arc::new(Arc::clone(&body).try_acquire_owned().unwrap())),
        Err("review_disabled")
    ));
    assert!(matches!(
        s.begin_read(
            Arc::new(Arc::clone(&body).try_acquire_owned().unwrap()),
            true
        ),
        Err("review_disabled")
    ));
    stop(&s);
}
#[test]
fn selected_read_target_cannot_be_used_as_an_editor() {
    assert!(Service::start(Config {
        kind: store::Kind::Notes,
        editable: true,
        read_target: Some(std::env::temp_dir().join("not-opened")),
        reader_id: None,
        reviewer: "fixed".into(),
        files: vec![],
        trees: vec![],
        sources: SourceSet::new(vec![]).unwrap(),
        notes_display: Weak::new(),
    })
    .is_err());
}
fn owner() -> SessionId {
    let (mut auth, secret) = crate::auth::Auth::new(
        Instant::now(),
        Duration::from_secs(30),
        Duration::from_secs(60),
    )
    .unwrap();
    auth.exchange(&secret.expose(), Instant::now()).unwrap().id
}
pub(super) fn request() -> Submit {
    Submit {
        seq: "1".into(),
        context: Context {
            drc_id: "a".repeat(64),
            revision: "b".repeat(64),
            view_id: "c".repeat(64),
        },
        token: "d".repeat(64),
        approve: true,
        confirm_legacy: false,
    }
}
pub(super) fn stop(s: &Service) {
    s.request_stop();
    let end = Instant::now() + Duration::from_secs(3);
    while !s.is_finished() {
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn preparation_keeps_body_and_build_exclusion_until_native_work_unwinds() {
    let s = service();
    let body = Arc::new(Semaphore::new(1));
    let op = s
        .begin(Arc::new(Arc::clone(&body).try_acquire_owned().unwrap()))
        .unwrap();
    let native = Arc::clone(&op);
    let ran = AtomicUsize::new(0);
    assert!(matches!(
        s.admit_build(|| {
            ran.store(1, Ordering::Relaxed);
            Ok(())
        }),
        Err("drc_busy")
    ));
    assert_eq!(ran.load(Ordering::Relaxed), 0);
    s.request_stop();
    assert_ne!(op.stop.load(Ordering::Relaxed), 0);
    drop(op);
    assert!(!s.is_finished());
    assert_eq!(body.available_permits(), 0);
    drop(native);
    stop(&s);
    assert_eq!(body.available_permits(), 1);
}

#[test]
fn display_holds_admission_until_cancelled_native_work_unwinds() {
    let s = service();
    let body = Arc::new(Semaphore::new(1));
    let op = s
        .begin_read(
            Arc::new(Arc::clone(&body).try_acquire_owned().unwrap()),
            true,
        )
        .unwrap();
    assert!(matches!(s.admit_build(|| Ok(())), Err("drc_busy")));
    assert!(matches!(
        s.begin(Arc::new(
            Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap()
        )),
        Err("drc_busy")
    ));
    s.request_stop();
    assert_ne!(op.stop.load(Ordering::Relaxed), 0);
    assert!(!s.is_finished());
    drop(op);
    stop(&s);
    assert_eq!(body.available_permits(), 1);
}
#[test]
fn cancel_is_a_flag_until_terminal_and_late_cancel_does_not_undo_success() {
    let s = service();
    let flag = Arc::new(AtomicUsize::new(0));
    {
        let mut state = s.inner.state.lock().unwrap();
        state.ledger.admit(1, "test".into(), "drc_note").unwrap();
        state.stop = Some(Arc::clone(&flag));
    }
    assert_eq!(s.cancel(1).unwrap()["phase"], "queued");
    assert_ne!(flag.load(Ordering::Relaxed), 0);
    assert!(matches!(s.admit_build(|| Ok(())), Err("drc_busy")));
    let outcome = json!({"seq":"1","phase":"succeeded","published":true});
    {
        let mut state = s.inner.state.lock().unwrap();
        state.ledger.update(1, outcome.clone(), true);
        state.stop = None;
    }
    assert_eq!(s.cancel(1).unwrap(), outcome);
    s.admit_build(|| Ok(())).unwrap();
    stop(&s);
}
#[test]
fn receipt_replay_binds_owner_and_approved_body_before_current_context() {
    let s = service();
    let who = owner();
    let req = request();
    s.inner
        .state
        .lock()
        .unwrap()
        .ledger
        .admit(1, Service::signature(&who, &req), "drc_note")
        .unwrap();
    assert!(s.replay(&who, &req).unwrap().is_some());
    assert!(matches!(
        s.replay(&owner(), &req),
        Err("operation_conflict")
    ));
    let mut changed = request();
    changed.confirm_legacy = true;
    assert!(matches!(
        s.replay(&who, &changed),
        Err("operation_conflict")
    ));
    changed = request();
    changed.context.revision = "e".repeat(64);
    assert!(matches!(
        s.replay(&who, &changed),
        Err("operation_conflict")
    ));
    stop(&s);
}
#[test]
fn wire_never_accepts_reviewer_path_kind_or_forged_global_ids() {
    let context =
        json!({"drc_id":"a".repeat(64),"revision":"b".repeat(64),"view_id":"c".repeat(64)});
    for key in ["path", "reviewer", "kind", "gids"] {
        let mut value = json!({"context":context,"errors":[{"check":"1","error":"0"}]});
        value[key] = json!("foreign");
        assert!(serde_json::from_value::<Read>(value).is_err());
    }
    assert!(http::is_large_body(
        &axum::http::Method::POST,
        "/api/v1/drc/review/notes/read"
    ));
    assert!(!http::is_large_body(
        &axum::http::Method::GET,
        "/api/v1/drc/review/notes/read"
    ));
    assert!(!http::is_large_body(
        &axum::http::Method::POST,
        "/api/v1/drc/review/notes/prepare/"
    ));
    let mut c = request().context;
    c.view_id = "한".repeat(64);
    assert!(c.validate().is_err());
}

#[test]
fn prepare_kind_comes_only_from_registration_and_route() {
    let base = json!({"context":request().context,"token":"a".repeat(64)});
    for (fields, notes, waives) in [
        (json!({"text":"note"}), true, false),
        (json!({"waived":true}), false, true),
        (json!({"waived":false}), false, true),
        (json!({}), false, false),
        (json!({"text":"note","waived":true}), false, false),
    ] {
        let mut value = base.clone();
        value
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let req: Prepare = serde_json::from_value(value).unwrap();
        assert_eq!(req.validate(store::Kind::Notes).is_ok(), notes);
        assert_eq!(req.validate(store::Kind::Waives).is_ok(), waives);
    }
    let mut invalid = base;
    invalid["waived"] = json!(2);
    assert!(serde_json::from_value::<Prepare>(invalid).is_err());
    assert!(http::is_large_body(
        &axum::http::Method::POST,
        "/api/v1/drc/review/waives/prepare"
    ));
}

#[test]
fn reader_failure_or_unknown_ack_never_relabels_successful_publication() {
    let context = request().context;
    let note = unknown_progress(1, &context, store::Kind::Notes);
    assert_eq!(note["kind"], "drc_note");
    assert!(note.get("reader_applied").is_none());
    let waive = unknown_progress(1, &context, store::Kind::Waives);
    assert_eq!(waive.get("reader_applied"), Some(&Value::Null));
    for value in [note, waive] {
        assert_eq!(value["phase"], "failed");
        assert_eq!(value["published"], Value::Null);
        assert_eq!(value["outcome_unknown"], true);
    }
    for (result, applied) in [
        (Ok(vec![]), json!(true)),
        (Err("review_changed"), json!(false)),
        (Err("drc_cancelled"), json!(false)),
        (Err("drc_apply_unknown"), Value::Null),
    ] {
        let mut value = json!({"published":true,"phase":"succeeded","directory_synced":false});
        reader_result(&mut value, result);
        assert_eq!(value["published"], true);
        assert_eq!(value["phase"], "succeeded");
        assert_eq!(value["directory_synced"], false);
        assert_eq!(value["reader_applied"], applied);
    }
}

#[test]
fn unknown_commit_is_not_false_and_directory_sync_warning_is_published() {
    let mut status = managed::Status {
        id: 1,
        kind: store::Kind::Notes,
        phase: managed::Phase::Failed,
        elapsed_ms: 3,
        failure: Some(ErrorKind::Worker),
        outcome: None,
        outcome_unknown: true,
    };
    let context = request().context;
    let value = progress(7, &context, &status);
    assert!(value["published"].is_null());
    assert_eq!(value["outcome_unknown"], true);
    status.phase = managed::Phase::Succeeded;
    status.failure = None;
    status.outcome = Some(store::Published {
        directory_synced: false,
        file: None,
    });
    status.outcome_unknown = false;
    let value = progress(7, &context, &status);
    assert_eq!(value["published"], true);
    assert_eq!(value["directory_synced"], false);
    assert_eq!(value["phase"], "succeeded");
}
