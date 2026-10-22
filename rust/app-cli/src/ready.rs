//! A read command's source made ready (user 2026-10-18: "자동 인덱싱하도록
//! 하고 대기 옵션도 넣어줘"; docs/SHARED_APP_LAYER.ko.md §12).
//!
//! `render`, `info`, `probe` and `clip` index a layout that has no index yet
//! first - a jobdeck the sources of its levels that have none - as `index`
//! builds them, its output on stderr (their stdout is theirs: `info --json`).
//! `--no-index` or FLOE_AUTO_INDEX=off: refused as before 0.12.359. Only
//! what has no cache is built - or the folder a build stopped half-way left
//! (no meta.json; a Ctrl-C on the index this starts), rebuilt when no run
//! writes it: a changed source keeps its cache
//! (drawn with the warning; `index --force` rebuilds it) and a cache this
//! build cannot read is refused - rebuilt by each build that cannot read the
//! other's, two bundles on the hosts would undo each other's index.
//!
//! A source another run indexes (or, for a rebuild, has open) is refused at
//! once with 75 (floe_vfs::lock), unless the run was asked to wait: `--wait`
//! (`--wait=SECONDS` bounded; FLOE_LOCK_WAIT=on|SECONDS the default,
//! `--no-wait` none) polls the locks' probes until that run is done and goes
//! on - a cache it made is used, not made again; past the bound it ends 75 as
//! before, and a signal ends the wait as it ends the command.
use crate::Out;
use floe_app_core::{
    cache::{self, CacheState},
    dataset::Dataset,
    index::IndexOptions,
    jobdeck::{
        color::Mode,
        index::{is_deck, DeckIndexPlan},
    },
    Error, ErrorKind, Result,
};
use floe_vfs::lock::{self, Busy, Key};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Whether a refused run waits (`--wait`), and how long at most.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Wait {
    /// refused at once (75): the default
    #[default]
    No,
    /// until the run in the way is done
    Unbounded,
    /// at most this long in all, then refused (75)
    For(Duration),
}
impl Wait {
    /// FLOE_LOCK_WAIT: `on`/`yes` (no bound), SECONDS, `off`/`no`/`0`/unset
    /// (none) - a site's or a script's default the options override.
    pub fn from_env() -> Result<Self> {
        let value = std::env::var("FLOE_LOCK_WAIT").unwrap_or_default();
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "off" | "no" | "0" => Ok(Self::No),
            "on" | "yes" => Ok(Self::Unbounded),
            seconds => Self::seconds(seconds).map_err(|_| {
                Error::input(format!(
                    "FLOE_LOCK_WAIT must be on, off or a number of seconds: {value:?}"
                ))
            }),
        }
    }
    /// `--wait` (no bound) or `--wait=SECONDS`.
    pub fn flag(inline: Option<&str>) -> Result<Self> {
        match inline {
            None => Ok(Self::Unbounded),
            Some(v) => Self::seconds(v.trim()).map_err(|_| {
                Error::input(format!("--wait=SECONDS takes a positive number of seconds: {v}"))
            }),
        }
    }
    fn seconds(text: &str) -> std::result::Result<Self, ()> {
        let seconds: f64 = text.parse().map_err(|_| ())?;
        if !(seconds.is_finite() && seconds > 0. && seconds <= 1e9) {
            return Err(());
        }
        Ok(Self::For(Duration::from_secs_f64(seconds)))
    }
    pub fn on(self) -> bool {
        self != Self::No
    }
}

/// What a read command does about its source's index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ready {
    /// index what has none first (`--no-index`, FLOE_AUTO_INDEX=off: no)
    pub index: bool,
    pub wait: Wait,
    /// the kind of cache the auto-index builds for a source without one:
    /// `--light` / `--full`; None = the rule (a jobdeck's sources light, a
    /// layout full - IndexOptions::light_for; user 2026-10-22: an EBEAM
    /// file captured on its own takes `--light`)
    pub light: Option<bool>,
}
impl Ready {
    /// The defaults: FLOE_AUTO_INDEX (on unless off/no/0), FLOE_LOCK_WAIT.
    pub fn from_env() -> Result<Self> {
        let index = !matches!(
            std::env::var("FLOE_AUTO_INDEX")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "off" | "no" | "0"
        );
        Ok(Self {
            index,
            wait: Wait::from_env()?,
            light: None,
        })
    }
    /// Take `flag` when it is one of these options (`--no-index`, `--wait`,
    /// `--wait=SECONDS`, `--no-wait`, `--light`, `--full`): whether it was.
    pub fn take(&mut self, flag: &str, inline: Option<&str>) -> Result<bool> {
        let no_value = || match inline {
            Some(_) => Err(Error::input(format!("{flag} takes no value"))),
            None => Ok(()),
        };
        match flag {
            "--no-index" => {
                no_value()?;
                self.index = false;
            }
            "--wait" => self.wait = Wait::flag(inline)?,
            "--no-wait" => {
                no_value()?;
                self.wait = Wait::No;
            }
            "--light" | "--full" => {
                no_value()?;
                let light = flag == "--light";
                if self.light.is_some_and(|l| l != light) {
                    return Err(Error::input("--light and --full are exclusive"));
                }
                self.light = Some(light);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// `m:ss` / `h:mm:ss`.
fn hms(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A source's cache lock (floe_vfs::lock), by the cache name it has.
pub(crate) fn layout_key(source: &Path) -> Result<Key> {
    Ok(lock::key(
        lock::Kind::Vfs,
        &cache::cache_path(source)?.to_string_lossy(),
    ))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// One command's waiting: its bound counts from the first wait.
pub(crate) struct Waiter<'a> {
    wait: Wait,
    cancelled: &'a AtomicUsize,
    started: Option<Instant>,
    /// the refusal `again` said last (a deck's)
    said: Option<String>,
}
impl<'a> Waiter<'a> {
    pub(crate) fn new(wait: Wait, cancelled: &'a AtomicUsize) -> Self {
        Self {
            wait,
            cancelled,
            started: None,
            said: None,
        }
    }
    pub(crate) fn on(&self) -> bool {
        self.wait.on()
    }
    /// Whether the bound is spent: the refusal it ended with is the answer,
    /// not one more wait.
    fn expired(&self) -> bool {
        match (self.wait, self.started) {
            (Wait::For(limit), Some(started)) => started.elapsed() >= limit,
            _ => false,
        }
    }
    /// Sleep `period` unless a signal comes (Err(Cancelled)).
    fn sleep(&self, period: Duration) -> Result<()> {
        let until = Instant::now() + period;
        while Instant::now() < until {
            if self.cancelled.load(Ordering::Relaxed) != 0 {
                return Err(Error::new(ErrorKind::Cancelled, "cancelled while waiting"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }
    /// Wait out what `probe` says is in `subject`'s way. Nothing now (its
    /// run ended meanwhile, or what refused is a lock the probes do not see -
    /// another floe2's app lease before its indexer starts): a moment's pause
    /// and the caller tries again. Past the bound Err(Busy) - 75, the run's
    /// words and how long it waited.
    pub(crate) fn wait(
        &mut self,
        subject: &str,
        mut probe: impl FnMut() -> Option<Busy>,
    ) -> Result<()> {
        let started = *self.started.get_or_insert_with(Instant::now);
        let deadline = match self.wait {
            Wait::No => None,
            Wait::Unbounded => None,
            Wait::For(limit) => Some(started + limit),
        };
        let bound = match self.wait {
            Wait::For(limit) => format!(" (--wait, up to {} s)", limit.as_secs_f64()),
            _ => " (--wait)".to_string(),
        };
        let mut said: Option<String> = None;
        let mut noted = Instant::now();
        let mut first = true;
        loop {
            let Some(busy) = probe() else {
                if said.is_some() {
                    eprintln!(
                        "[lock] {subject} is free - going on (waited {})",
                        hms(started.elapsed())
                    );
                } else if first {
                    self.sleep(Duration::from_secs(1))?;
                }
                return Ok(());
            };
            if matches!(busy, Busy::Error { .. }) {
                return Err(Error::new(ErrorKind::Busy, busy.to_string()));
            }
            let text = busy.waiting();
            if said.as_deref() != Some(text.as_str()) {
                eprintln!("[lock] {text}{bound}");
                said = Some(text);
                noted = Instant::now();
            } else if noted.elapsed() >= Duration::from_secs(60) {
                eprintln!(
                    "[lock] still waiting for {subject} ({})",
                    hms(started.elapsed())
                );
                noted = Instant::now();
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return Err(Error::new(
                    ErrorKind::Busy,
                    format!(
                        "{busy} (waited {}, the --wait bound)",
                        hms(started.elapsed())
                    ),
                ));
            }
            first = false;
            self.sleep(Duration::from_millis(500))?;
        }
    }
    /// After a run that writes `key`'s target was refused: wait for the run
    /// writing it, else - `readers`: one that may need them out (a rebuild,
    /// a replacement) - for its readers, else a moment.
    pub(crate) fn wait_to_write(&mut self, key: &Key, readers: bool) -> Result<()> {
        if lock::writing_refusal(key).is_some() || !readers {
            return self.wait(&key.subject, || lock::writing_refusal(key));
        }
        self.wait(&key.subject, || {
            lock::writing_refusal(key).or_else(|| lock::reader_refusal(key))
        })
    }
    /// After an open was refused (a rebuild whole in progress): wait for it.
    pub(crate) fn wait_to_open(&mut self, key: &Key) -> Result<()> {
        self.wait(&key.subject, || lock::opening_refusal(key))
    }
    /// Whether a read command refused for its source (`error`: Busy - a
    /// rebuild that started after the open, as renderd took its locks) waits
    /// and runs again: it has waited when Ok(true).
    pub(crate) fn again(&mut self, error: &Error, source: &Path) -> Result<bool> {
        if error.kind != ErrorKind::Busy || !self.on() || self.expired() {
            return Ok(false);
        }
        if is_deck(source) {
            // a deck's sources are waited for at its open (`open`); what
            // refused it now is said once
            if self.said.as_deref() != Some(error.message.as_str()) {
                eprintln!("[lock] {} - waiting (--wait)", error.message);
                self.said = Some(error.message.clone());
            }
            self.sleep(Duration::from_secs(1))?;
        } else {
            self.wait_to_open(&layout_key(source)?)?;
        }
        Ok(true)
    }
}

/// What a source has of a cache, for its index first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Found {
    /// none under either name
    None,
    /// a real folder without its commit (no meta.json): a build that
    /// stopped half-way left it - or one runs now (the locks tell)
    Incomplete,
    /// one: current, of a changed source, or one this build cannot read
    There,
}
fn found(source: &Path) -> Result<Found> {
    let directory = cache::cache_path(source)?;
    if cache::inspect(source, &directory)? == CacheState::Missing {
        return Ok(Found::None);
    }
    let real = std::fs::symlink_metadata(&directory)
        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
    let uncommitted = matches!(
        std::fs::symlink_metadata(directory.join("meta.json")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
    );
    Ok(if real && uncommitted {
        Found::Incomplete
    } else {
        Found::There
    })
}

/// A layout with no cache indexed now, as `index SOURCE [--light|--full]`
/// would (`light`: the kind asked for, None = the rule) - a folder a build
/// stopped half-way left rebuilt (`index --force`); with `summaries`
/// (`--occupancy on`) a cache without its occupancy summary gets it too
/// (`index --occupancy` adds design.ovo to a current cache: the light
/// cache's by the light command, the full index's by `vfs
/// --occupancy-only` - the kind is kept, `decide` would build the full
/// index over a light cache under a full request).
fn index_layout(
    source: &Path,
    light: Option<bool>,
    summaries: bool,
    waiter: &mut Waiter,
    cancelled: &AtomicUsize,
) -> Result<()> {
    let source = cache::absolute(source)?;
    let name = file_name(&source);
    if !source.is_file() {
        return Err(Error::input(format!("source not found: {}", source.display())));
    }
    let mut said = false;
    loop {
        let state = found(&source)?;
        // another run builds it: the open waits for it or says who
        if lock::writing_refusal(&layout_key(&source)?).is_some() {
            return Ok(());
        }
        let (options, what) = if state == Found::There {
            if !summaries {
                return Ok(());
            }
            let directory = cache::cache_path(&source)?;
            if directory.join("design.ovo").is_file() {
                return Ok(());
            }
            let kind_light = cache::cache_kind(&directory) == Some(cache::CacheKind::Light);
            (
                IndexOptions {
                    light: Some(kind_light),
                    occupancy: Some(true),
                    ..IndexOptions::default()
                },
                "has no occupancy summary yet - adding it first (--occupancy on)".to_string(),
            )
        } else {
            let options = IndexOptions {
                force: state == Found::Incomplete,
                light,
                occupancy: summaries.then_some(true),
                ..IndexOptions::default()
            };
            let kind = match light {
                Some(true) => " (--light: the light cache)",
                Some(false) => " (--full)",
                None => "",
            };
            let what = if options.force {
                format!("has an incomplete index (a build stopped half-way) - rebuilding it first{kind}")
            } else {
                format!("has no index yet - indexing it first{kind}")
            };
            (options, what)
        };
        if !said {
            eprintln!(
                "[{}] {name} {what} (--no-index: refuse instead)",
                floe_app_core::program()
            );
            said = true;
        }
        let refused = match crate::execute_index(&source, &options, Out::Stderr, cancelled) {
            Ok((0, _)) => return Ok(()),
            Ok((130 | 143, _)) => {
                return Err(Error::new(ErrorKind::Cancelled, "index cancelled"));
            }
            // floe-index refused it, its `[lock]` line said why: one line
            Ok((lock::BUSY_EXIT, _)) => Error::new(ErrorKind::Busy, String::new()),
            Ok((code, _)) => {
                return Err(Error::new(
                    ErrorKind::Worker,
                    format!(
                        "indexing {name} first failed (exit {code}{}; see above)",
                        floe_app_core::index::exit_words(code)
                            .map(|w| format!(" - {w}"))
                            .unwrap_or_default()
                    ),
                ));
            }
            Err(e) if e.kind == ErrorKind::Busy => e,
            Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
            // where it was to write (a folder this account may not write)
            Err(e) if e.kind == ErrorKind::Io => {
                return Err(Error::new(
                    e.kind,
                    format!(
                        "indexing {name} first: {} (its index is written beside it, in {})",
                        e.message,
                        source.parent().unwrap_or(Path::new("/")).display()
                    ),
                ))
            }
            Err(e) => {
                return Err(Error::new(
                    e.kind,
                    format!("indexing {name} first: {}", e.message),
                ))
            }
        };
        if !waiter.on() {
            return Err(refused);
        }
        // a cache with no index yet has no readers to wait for
        waiter.wait_to_write(&layout_key(&source)?, false)?;
    }
}

/// A deck's sources (`catalog`, its levels') that have no cache at all, and
/// the locks of those another run rebuilds now.
/// Whether the deck needs `index` before it is read: a drawable source
/// with no cache at all; for a command that draws (`summaries`: render,
/// probe - not info), an indexed one without its occupancy summary
/// (design.ovo) as well - a deck's wide view draws nothing of a light
/// source without it (DIRECT_READ_PLAN §5.5), and `floe2 index DECK` adds
/// it; and the sources another run rebuilds.
fn deck_state(
    catalog: &floe_app_core::jobdeck::sources::SourceCatalog,
    summaries: bool,
) -> Result<(bool, Vec<Key>)> {
    let mut needs = false;
    let mut busy = Vec::new();
    for info in catalog.infos.values() {
        if !info.busy.is_empty() {
            busy.push(lock::key(lock::Kind::Vfs, &info.cache_dir.to_string_lossy()));
        } else if !needs && info.ok() {
            if !info.indexed {
                needs = found(&info.path)? != Found::There;
            } else if summaries && !info.cache_dir.join("design.ovo").is_file() {
                needs = true;
            }
        }
    }
    Ok((needs, busy))
}

/// The deck's sources that have no cache at all, and the indexed ones
/// without their occupancy summary, of the levels loaded, indexed now as
/// `index DECK` indexes them; a source that fails or (no --wait) is busy is
/// left out of the drawing, said in its ledger.
fn index_deck(
    plan: DeckIndexPlan,
    summaries: bool,
    waiter: &mut Waiter,
    cancelled: &AtomicUsize,
) -> Result<()> {
    let mut todo = Vec::new();
    let (mut without_index, mut without_summary) = (0usize, 0usize);
    for mut entry in plan.todo {
        match found(&entry.source)? {
            Found::None => {
                without_index += 1;
                todo.push(entry);
            }
            // a source's build stopped half-way: rebuilt
            Found::Incomplete => {
                entry.options.force = true;
                without_index += 1;
                todo.push(entry);
            }
            // a current cache whose summary the plan adds (occupancy-only):
            // for a command that draws
            Found::There if summaries && entry.options.occupancy_only => {
                without_summary += 1;
                todo.push(entry);
            }
            Found::There => (),
        }
    }
    if todo.is_empty() {
        return Ok(());
    }
    let mut what = Vec::new();
    if without_index > 0 {
        what.push(format!("{} source(s) have no index yet", without_index));
    }
    if without_summary > 0 {
        what.push(format!(
            "{} source(s) have no occupancy summary yet",
            without_summary
        ));
    }
    eprintln!(
        "[jobdeck] {} - indexing them first (--no-index: draw without them)",
        what.join(", ")
    );
    crate::deck_index::run_entries(&todo, 0, Out::Stderr, waiter, cancelled)?;
    Ok(())
}

/// the auto-index's options for a deck: the cache kind asked for (`light`:
/// `--light` / `--full`, else the rule) and whether the summaries are built
/// (`summaries`: `--occupancy on`; a capture builds none by default - user
/// 2026-10-22: a capture is drawn once, `floe2 index deck.jb` and the
/// viewer's open build the summary for what is opened again)
fn deck_options(light: Option<bool>, summaries: bool) -> IndexOptions {
    IndexOptions {
        light,
        occupancy: Some(summaries),
        ..IndexOptions::default()
    }
}

/// `source` opened for a read command: what has no index indexed first
/// (`index`, as `light` asks; with `summaries` - `render --occupancy on` -
/// a source's missing occupancy summary as well), what another run indexes
/// waited for (`waiter`) or refused.
pub(crate) fn open(
    source: &Path,
    levels: Option<BTreeSet<i64>>,
    index: bool,
    light: Option<bool>,
    summaries: bool,
    waiter: &mut Waiter,
    cancelled: &AtomicUsize,
) -> Result<Dataset> {
    let deck = is_deck(source);
    let mut deck_indexed = false;
    loop {
        if index && !deck {
            index_layout(source, light, summaries, waiter, cancelled)?;
        }
        let opened = Dataset::open(source, levels.clone(), Mode::Level, cancelled);
        let (none, busy, refused) = match opened {
            Ok(dataset) => {
                let Dataset::Deck(snapshot) = &dataset else {
                    return Ok(dataset);
                };
                let (none, busy) = deck_state(&snapshot.analysis.catalog, summaries)?;
                if !(index && !deck_indexed && none) && !(waiter.on() && !busy.is_empty()) {
                    return Ok(dataset);
                }
                (none, busy, None)
            }
            Err(e) if e.kind == ErrorKind::Busy && waiter.on() && !deck => {
                waiter.wait_to_open(&layout_key(source)?)?;
                continue;
            }
            // a deck with nothing to draw: its sources may have no index
            // yet, or another run indexes them
            Err(e)
                if deck
                    && e.kind != ErrorKind::Cancelled
                    && ((index && !deck_indexed) || waiter.on()) =>
            {
                let plan = DeckIndexPlan::prepare(
                    source,
                    levels.clone(),
                    &deck_options(light, summaries),
                    cancelled,
                )?;
                let (none, busy) = deck_state(&plan.catalog, summaries)?;
                if !(index && !deck_indexed && none) && !(waiter.on() && !busy.is_empty()) {
                    return Err(e);
                }
                if index && !deck_indexed && none {
                    deck_indexed = true;
                    index_deck(plan, summaries, waiter, cancelled)?;
                    continue;
                }
                (none, busy, Some(e))
            }
            Err(e) => return Err(e),
        };
        drop(refused);
        // its sources with no cache at all, or no summary yet: indexed
        // once, then drawn
        if index && !deck_indexed && none {
            deck_indexed = true;
            let plan =
                DeckIndexPlan::prepare(source, levels.clone(), &deck_options(light, summaries), cancelled)?;
            index_deck(plan, summaries, waiter, cancelled)?;
            continue;
        }
        // its sources another run rebuilds: waited for, then the deck again
        for key in &busy {
            waiter.wait_to_open(key)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_options_and_the_environment() {
        assert_eq!(Wait::flag(None).unwrap(), Wait::Unbounded);
        assert_eq!(
            Wait::flag(Some("2.5")).unwrap(),
            Wait::For(Duration::from_millis(2500))
        );
        for bad in ["0", "-1", "nan", "inf", "x", ""] {
            assert!(Wait::flag(Some(bad)).is_err(), "{bad}");
        }
        let mut ready = Ready {
            index: true,
            wait: Wait::No,
            light: None,
        };
        assert!(ready.take("--wait", Some("10")).unwrap());
        assert_eq!(ready.wait, Wait::For(Duration::from_secs(10)));
        assert!(ready.take("--no-wait", None).unwrap());
        assert_eq!(ready.wait, Wait::No);
        assert!(ready.take("--no-index", None).unwrap());
        assert!(!ready.index);
        assert!(ready.take("--no-index", Some("1")).is_err());
        assert!(!ready.take("--px", Some("1")).unwrap());
        // the auto-index's cache kind (2026-10-22): --light / --full, exclusive
        assert_eq!(ready.light, None);
        assert!(ready.take("--light", None).unwrap());
        assert_eq!(ready.light, Some(true));
        assert!(ready.take("--light", None).unwrap());
        assert!(ready.take("--full", None).is_err());
        assert!(ready.take("--light", Some("1")).is_err());
        let mut full = Ready {
            index: true,
            wait: Wait::No,
            light: None,
        };
        assert!(full.take("--full", None).unwrap());
        assert_eq!(full.light, Some(false));
        assert_eq!(hms(Duration::from_secs(5)), "0:05");
        assert_eq!(hms(Duration::from_secs(3725)), "1:02:05");
    }
    #[test]
    fn a_bounded_wait_ends_busy_and_a_signal_ends_it_cancelled() {
        let busy = || {
            Some(Busy::Indexing {
                subject: "X.oas".into(),
                holder: None,
                opening: false,
            })
        };
        let flag = AtomicUsize::new(0);
        let mut waiter = Waiter::new(Wait::For(Duration::from_millis(300)), &flag);
        let started = Instant::now();
        let e = waiter.wait("X.oas", busy).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Busy);
        assert!(e.message.contains("X.oas is being indexed by another process"), "{}", e.message);
        assert!(e.message.ends_with("(waited 0:00, the --wait bound)"), "{}", e.message);
        assert!(started.elapsed() >= Duration::from_millis(300));
        let flag = AtomicUsize::new(2);
        let mut waiter = Waiter::new(Wait::Unbounded, &flag);
        assert_eq!(waiter.wait("X.oas", busy).unwrap_err().kind, ErrorKind::Cancelled);
        // free at once: a moment's pause, then go on
        let flag = AtomicUsize::new(0);
        let mut waiter = Waiter::new(Wait::Unbounded, &flag);
        let mut calls = 0;
        waiter
            .wait("X.oas", || {
                calls += 1;
                (calls < 3).then(|| busy().unwrap())
            })
            .unwrap();
        assert_eq!(calls, 3);
    }
}
