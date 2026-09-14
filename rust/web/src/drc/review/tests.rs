use super::*;

pub(super) fn service() -> Arc<Service> {
    Service::start(Config {
        kind: store::Kind::Notes,
        reviewer: "fixed".into(),
        files: vec![],
        trees: vec![],
        sources: vec![],
    })
    .unwrap()
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
