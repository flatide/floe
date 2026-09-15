//! Same-user local handoff. Filesystem preparation never runs in the socket
//! callback or browser reactor; a bounded proposal is consumed by the owner UI.
use super::*;
use floe_app_core::{
    instance::{Claim, Endpoint, Key, Outcome, Owner},
    ErrorKind,
};
use floe_web::launch::Launches;
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};

fn build() -> String {
    format!("{}-{BUNDLE}", env!("CARGO_PKG_VERSION"))
}
pub(super) fn claim() -> Result<Claim> {
    let display = std::env::var("DISPLAY").ok();
    let key = Key::new("floe2-web", display.as_deref())?;
    let base = std::env::var_os("FLOE_WEB_INSTANCE_DIR")
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    Endpoint::in_directory(&base, &key)?.claim(&build())
}
fn absolute(path: &std::path::Path) -> Result<PathBuf> {
    // Keep the original spelling (including ..) for AccessScope's symlink
    // checks; do not normalize a symlink escape before validation.
    Ok(if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    })
}
fn message(c: &Command) -> Result<Value> {
    let mut args = vec!["view".to_owned(), "--mode".into(), c.mode.clone()];
    for path in &c.roots {
        args.extend([
            "--root".into(),
            absolute(path)?
                .to_str()
                .ok_or_else(|| Error::input("root must be UTF-8"))?
                .into(),
        ]);
    }
    for (field, key) in [
        ("depth", "--depth"),
        ("detail", "--detail"),
        ("thin", "--thin"),
        ("font_px", "--label-font-px"),
    ] {
        if let Some(v) = c.initial.get(field) {
            args.extend([
                key.into(),
                v.as_str().map_or_else(|| v.to_string(), str::to_owned),
            ]);
        }
    }
    for key in ["frames", "labels"] {
        if let Some(v) = c.initial[key].as_bool() {
            args.extend([format!("--{key}"), if v { "on" } else { "off" }.into()]);
        }
    }
    if c.initial["mono"] == true {
        args.push("--mono".into());
    }
    if let Some(n) = c.initial.get("navigation") {
        let mut parts: Vec<_> = n["center_um"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        if let Some(w) = n["width_um"].as_str() {
            parts.push(w);
        }
        args.extend(["--goto".into(), parts.join(",")]);
    }
    if let Some(ids) = &c.levels {
        args.extend([
            "--level".into(),
            ids.iter().map(i64::to_string).collect::<Vec<_>>().join(","),
        ]);
    }
    args.push("--".into());
    for path in &c.sources {
        args.push(
            absolute(path)?
                .to_str()
                .ok_or_else(|| Error::input("source must be UTF-8"))?
                .into(),
        );
    }
    Ok(json!({"argv":args,"level_policy":std::env::var("FLOE_JOBDECK_LEVELS").ok()}))
}
fn decode(value: &Value) -> Result<(Command, Option<String>)> {
    let obj = value
        .as_object()
        .filter(|m| m.len() == 2 && m.contains_key("argv") && m.contains_key("level_policy"))
        .ok_or_else(|| Error::input("invalid launch request"))?;
    let args: Vec<String> = serde_json::from_value(obj["argv"].clone())
        .map_err(|_| Error::input("invalid launch argv"))?;
    if args.len() > 192 || args.first().map(String::as_str) != Some("view") {
        return Err(Error::input("invalid launch argv"));
    }
    let c = parse(&args)?;
    if c.help
        || c.independent
        || c.no_open
        || c.sources.iter().chain(&c.roots).any(|p| !p.is_absolute())
    {
        return Err(Error::input(
            "process options/relative paths cannot be forwarded",
        ));
    }
    let policy: Option<String> = serde_json::from_value(obj["level_policy"].clone())
        .map_err(|_| Error::input("invalid level policy"))?;
    Ok((c, policy))
}
pub(super) fn forward(endpoint: &Endpoint, c: &Command, stop: &AtomicUsize) -> Result<i32> {
    let connection = endpoint.connect(&build(), stop)?;
    let intent = connection.intent(message(c)?)?;
    let outcome = match connection.submit(&intent, stop) {
        Ok(o) => o,
        Err(e) if e.kind == ErrorKind::Incomplete && stop.load(Ordering::Relaxed) == 0 => {
            // One recovery of the SAME token. No new owner or fresh seq on an
            // uncertain send; a different epoch fails closed in submit().
            endpoint.connect(&build(), stop)?.submit(&intent, stop)?
        }
        Err(e) => return Err(e),
    };
    match outcome {
        Outcome::Handled { reply } if reply["phase"] == "queued" => {
            println!("Request queued in the existing floe2-web workspace; check that window for opening/level selection.");
            Ok(0)
        }
        Outcome::Failed { code } => Err(Error::new(
            ErrorKind::Busy,
            format!("existing workspace refused launch: {code:?}"),
        )),
        Outcome::Rejected { code } => Err(Error::new(
            ErrorKind::Incomplete,
            format!("existing workspace handoff rejected: {code:?}; no new instance started"),
        )),
        _ => Err(Error::input("invalid launcher receipt")),
    }
}
pub(super) fn levels_json(levels: Option<BTreeSet<i64>>) -> Value {
    levels.map_or_else(
        || json!({"mode":"all"}),
        |ids| json!({"mode":"only","ids":ids.iter().map(i64::to_string).collect::<Vec<_>>()}),
    )
}
struct Pending {
    id: String,
    stop: Arc<AtomicUsize>,
    command: Command,
    policy: Option<String>,
}
fn prepare(service: &Service, work: Pending, launches: &Launches) {
    let result = (|| {
        let c = work.command;
        if c.sources.is_empty() {
            return launches.ready(&work.id, None, false);
        }
        let mut roots = c.roots;
        for p in &c.sources {
            roots.push(
                p.parent()
                    .ok_or_else(|| Error::input("source parent"))?
                    .to_owned(),
            );
        }
        roots.sort();
        roots.dedup();
        let scope = AccessScope::new(&roots)?;
        let ids = service.register_sources(scope, &c.sources, &work.stop)?;
        let catalog = service.catalog();
        let first = catalog["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["source_id"] == ids[0])
            .ok_or_else(|| Error::input("source unavailable"))?;
        let deck = first["deck"].as_bool().unwrap();
        let (levels, confirm) = startup_levels(
            c.levels,
            deck,
            first["levels"].as_u64().unwrap() as usize,
            work.policy.as_deref(),
        )?;
        service.validate_source_selection(&ids[0], &c.mode, levels.as_ref())?;
        let label_preference = startup_labels(&c.initial);
        let mut body = startup_body(c.initial, deck, false);
        // GTK forwards these resolved defaults even when omitted. Thin remains
        // absent unless explicitly supplied (including auto).
        if body.get("detail").is_none() {
            body["detail"] = json!("medium");
        }
        if body.get("font_px").is_none() {
            body["font_px"] = json!(14);
        }
        launches.ready(&work.id,Some(json!({"kind":"open","seq":"1","source_id":ids[0],"mode":c.mode,"levels":levels_json(levels),"body":body,"label_preference":label_preference})),confirm)
    })();
    if let Err(e) = result {
        launches.failed(&work.id, e.kind);
    }
}
pub(super) struct Runtime {
    stop: Arc<AtomicUsize>,
    launches: Arc<Launches>,
    socket: Option<JoinHandle<Result<()>>>,
    preparer: Option<JoinHandle<()>>,
}
impl Runtime {
    pub(super) fn start(
        owner: Owner,
        service: Arc<Service>,
        launches: Arc<Launches>,
    ) -> Result<Self> {
        let stop = Arc::new(AtomicUsize::new(0));
        let (tx, rx): (SyncSender<Pending>, _) = mpsc::sync_channel(1);
        let mut runtime = Self {
            stop: Arc::clone(&stop),
            launches: Arc::clone(&launches),
            socket: None,
            preparer: None,
        };
        let prep_stop = Arc::clone(&stop);
        let prep_launches = Arc::clone(&launches);
        runtime.preparer = Some(
            thread::Builder::new()
                .name("floe-launch-prepare".into())
                .spawn(move || {
                    while prep_stop.load(Ordering::Relaxed) == 0 {
                        match rx.recv_timeout(Duration::from_millis(20)) {
                            Ok(work) => prepare(&service, work, &prep_launches),
                            Err(mpsc::RecvTimeoutError::Timeout) => (),
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                })?,
        );
        runtime.socket = Some(
            thread::Builder::new()
                .name("floe-launch-ipc".into())
                .spawn(move || {
                    owner.serve(&stop, |body, stop| {
                        floe_app_core::check_cancelled(stop)?;
                        let (command, policy) = decode(body)?;
                        let (id, flag) = launches.reserve()?;
                        if tx
                            .try_send(Pending {
                                id: id.clone(),
                                stop: flag,
                                command,
                                policy,
                            })
                            .is_err()
                        {
                            launches.failed(&id, ErrorKind::Busy);
                        }
                        Ok(json!({"phase":"queued","id":id}))
                    })
                })?,
        );
        Ok(runtime)
    }
    pub(super) fn is_finished(&self) -> bool {
        self.socket.as_ref().is_some_and(|t| t.is_finished())
            || self.preparer.as_ref().is_some_and(|t| t.is_finished())
    }
    pub(super) fn close(&mut self) -> Result<()> {
        self.stop.store(1, Ordering::Relaxed);
        self.launches.stop();
        let result = self
            .socket
            .take()
            .map(|t| {
                t.join().unwrap_or_else(|_| {
                    Err(Error::new(
                        ErrorKind::Worker,
                        "launcher socket thread failed",
                    ))
                })
            })
            .unwrap_or(Ok(()));
        if let Some(t) = self.preparer.take() {
            t.join()
                .map_err(|_| Error::new(ErrorKind::Worker, "launcher preparation failed"))?;
        }
        result
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_request_preserves_options_paths_and_explicit_auto() {
        let args = [
            "view",
            "--root",
            "scope",
            "--goto",
            "-1.25,2,700",
            "--thin",
            "auto",
            "--depth",
            "99",
            "--detail",
            "high",
            "--no-frames",
            "--labels",
            "--label-font-px",
            "18",
            "--level",
            "1,2",
            "--mode",
            "chip",
            "--",
            "설계 with spaces.jb",
            "-another.oas",
        ]
        .map(str::to_owned);
        let c = parse(&args).unwrap();
        let (round, _) = decode(&message(&c).unwrap()).unwrap();
        assert_eq!(round.initial, c.initial);
        assert_eq!(round.levels, c.levels);
        assert_eq!(round.mode, c.mode);
        assert_eq!(
            round.sources,
            c.sources
                .iter()
                .map(|p| absolute(p).unwrap())
                .collect::<Vec<_>>()
        );
        let empty = parse(&["view".into()]).unwrap();
        assert!(decode(&message(&empty).unwrap())
            .unwrap()
            .0
            .sources
            .is_empty());
        assert!(empty.initial.get("thin").is_none());
    }
    #[test]
    fn receiver_refuses_process_options_relative_paths_and_extra_keys() {
        for argv in [
            vec!["view", "/tmp/a", "--jobs", "2"],
            vec!["view", "/tmp/a", "--stream-kb", "0"],
            vec!["view", "/tmp/a", "--render-debug"],
            vec!["view", "relative.oas"],
            vec!["view", "--no-open"],
            vec!["view", "--help"],
            vec!["index", "/tmp/a"],
        ] {
            assert!(decode(&json!({"argv":argv,"level_policy":null})).is_err());
        }
        assert!(decode(&json!({"argv":["view"],"level_policy":null,"path":"/tmp/a"})).is_err());
        assert!(decode(&json!({"argv":["view"],"level_policy":false})).is_err());
    }
}
