use super::*;
use crate::{cache::revision, index::revision::Build, jobdeck::sources::file_header};
use std::collections::BTreeMap;

pub(super) fn run(
    source: &RegisteredSource,
    levels: Option<BTreeSet<i64>>,
    options: &IndexOptions,
    indexer: &Indexer,
    flag: &AtomicUsize,
    state: &Mutex<Snapshot>,
) -> Result<()> {
    source.validate(flag)?;
    let paths = source.selected_sources(levels.as_ref(), flag)?;
    // Strict set preflight: no incomplete geometry silently becomes current.
    // Missing/gzip/GDS sources fail before the first candidate is created.
    for path in &paths {
        check_cancelled(flag)?;
        let header = file_header(path, flag)?;
        if header.format.as_deref() != Some("oasis") || header.gzipped {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "revision set member requires OASIS",
            ));
        }
    }
    let store = revision::set::Store::new(source.path())?;
    let builder = store.begin(levels, paths.clone(), flag)?;
    state.lock().unwrap().total = paths.len();
    let mut options = options.clone();
    if source.deck {
        options.occupancy = Some(options.occupancy.unwrap_or(true));
    }
    let mut pins = BTreeMap::new();
    for path in paths {
        // Recheck just this member's scope. Re-parsing/canonicalizing every
        // dependency per source would turn a large deck into O(N²) file probes.
        source.validate_revision_member(&path, flag)?;
        {
            let mut s = state.lock().unwrap();
            s.current = path
                .file_name()
                .map(|n| n.to_string_lossy().chars().take(256).collect());
            s.phase = Phase::Running;
            s.native = Progress::default();
        }
        // The outer supervisor owns the singleton CPU reservation. Only one
        // native child runs at a time; sealed members no longer own a worker.
        let result = (|| {
            let mut build = Build::start_admitted(&path, &options, indexer.clone(), flag)?;
            loop {
                if flag.load(Ordering::Relaxed) != 0 {
                    build.cancel()?;
                    return Err(Error::new(
                        ErrorKind::Cancelled,
                        "revision set cancelled; current unchanged",
                    ));
                }
                let done = build.poll()?;
                state.lock().unwrap().native = build.progress().cloned().unwrap_or_default();
                match done {
                    Some(0) => break,
                    Some(_) => {
                        return Err(Error::new(
                            ErrorKind::Worker,
                            "revision set member build failed",
                        ))
                    }
                    None => thread::sleep(Duration::from_millis(20)),
                }
            }
            build.seal(flag)
        })();
        let pin = match result {
            Ok(pin) => pin,
            Err(e) => {
                if e.kind != ErrorKind::Cancelled {
                    let mut s = state.lock().unwrap();
                    s.failed += 1;
                    s.completed += 1;
                }
                return Err(e);
            }
        };
        pins.insert(path, pin);
        state.lock().unwrap().completed += 1;
    }
    source.validate(flag)?;
    // All files were sealed. This one commit, never a loop over source current
    // pointers, makes the entire set available to new readers.
    let publication = builder.publish(pins, flag)?;
    let mut s = state.lock().unwrap();
    s.index_revision = Some(publication.revision);
    s.revision_sync_warning = !publication.directory_synced;
    Ok(())
}
