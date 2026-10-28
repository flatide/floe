use crate::{
    cache::{self, CacheState},
    native::{self, Indexer},
    Error, ErrorKind, Result,
};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::os::unix::{
    fs::{MetadataExt, OpenOptionsExt},
    process::ExitStatusExt,
};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
pub mod revision;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileCell {
    Name(String),
    Index(usize),
}
#[derive(Clone, Debug)]
pub struct IndexOptions {
    pub force: bool,
    pub jobs: usize,
    pub page_target_mb: Option<u64>,
    pub lod: bool,
    /// None resolves to off for layouts and on for jobdeck sources.
    pub occupancy: Option<bool>,
    pub occupancy_only: bool,
    pub occupancy_um: Option<f64>,
    /// the mag a jobdeck draws the source at (`floe-index --occupancy-mag`;
    /// jobdeck::index sets its placements' largest): the automatic cell is
    /// at most floe_vfs::occupancy::placed_ceiling_um(mag), so the summary
    /// reaches the deck view's 4 um/px; an explicit `occupancy_um` wins
    pub occupancy_mag: Option<f64>,
    pub occupancy_balance: Option<bool>,
    pub occupancy_prune: Option<bool>,
    /// the summary generator's caps (floe_vfs::occupancy::Opts
    /// `max_work` / `max_bytes`; DIRECT_READ_PLAN §8.18): marks per layer
    /// before a layer is `none:work`, bitmap bytes per file before the
    /// rest is `none:size`; None = floe-index's defaults
    pub occupancy_max_work: Option<u64>,
    pub occupancy_max_bytes: Option<u64>,
    pub representatives: bool,
    pub representatives_only: bool,
    pub representatives_points: Option<u64>,
    pub representatives_format: Option<u8>,
    pub slow_cell_s: Option<f64>,
    pub p2_shard_limit_mb: Option<u64>,
    pub profile_cell: Option<ProfileCell>,
    pub profile_jobs: Option<Vec<usize>>,
    pub profile_repeat: usize,
    pub profile_snapshot: Option<PathBuf>,
    pub profile_snapshot_refresh: bool,
    /// `--hier-only`: the cell tree's index (design.ovh) added to, or
    /// rebuilt on, a current cache by `floe-index hier`; nothing else.
    pub hier_only: bool,
    /// The pages' occupancy grids (design.ovb), built by default
    /// (2026-10-02); `--no-page-occupancy` is the kill switch.
    pub page_occupancy: bool,
    /// The occupancy density (design.ovs), built by default (2026-10-07);
    /// `--no-ovs` leaves it out.
    pub ovs: bool,
    /// the build's mode (docs/STREAMING_INDEX_PLAN.ko.md §10, the
    /// switch-over of 0.12.370): `Auto` passes no flag and floe-index
    /// streams - the inventory pass, then the cells read in file order
    /// behind a byte budget of `mem_gib` GiB (default min(MemAvailable/2,
    /// 32), raised to the file's need when the host has it), design.ovo
    /// from one more pass - unless FLOE_INDEX_STREAM=off or the run reads
    /// the whole parse; `Stream` insists (`--stream`); `InMemory` is the
    /// whole-parse build (`--in-memory`).
    pub mode: IndexMode,
    pub mem_gib: Option<f64>,
    /// the light cache (docs/DIRECT_READ_PLAN.ko.md §5.6, L5): `--light` /
    /// `--full`; None = FLOE_INDEX_LIGHT=on|off, else the rule - a jobdeck's
    /// sources light, a layout full - unless an option of the full index's
    /// was given (`full_only`). `light_for` resolves it.
    pub light: Option<bool>,
    /// a jobdeck's source (jobdeck::index sets it): the rule's default for
    /// `light` is the light cache, a layout's the full index
    /// (`light_for(self.deck_source)`). The light cache the rule chose
    /// gives way to the full index when `floe-index light` cannot read the
    /// stream (floe_vfs::light::UNSUITABLE_EXIT); one asked for with
    /// `--light` does not.
    pub deck_source: bool,
}
/// how `floe-index vfs` builds: see `IndexOptions::mode`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum IndexMode {
    #[default]
    Auto,
    Stream,
    InMemory,
}
impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            force: false,
            jobs: 12,
            page_target_mb: None,
            lod: false,
            occupancy: None,
            occupancy_only: false,
            occupancy_um: None,
            occupancy_mag: None,
            occupancy_balance: None,
            occupancy_prune: None,
            occupancy_max_work: None,
            occupancy_max_bytes: None,
            representatives: false,
            representatives_only: false,
            representatives_points: None,
            representatives_format: None,
            slow_cell_s: None,
            p2_shard_limit_mb: None,
            profile_cell: None,
            profile_jobs: None,
            profile_repeat: 1,
            profile_snapshot: None,
            profile_snapshot_refresh: false,
            hier_only: false,
            page_occupancy: true,
            ovs: true,
            mode: IndexMode::Auto,
            mem_gib: None,
            light: None,
            deck_source: false,
        }
    }
}
/// FLOE_INDEX_LIGHT: the light cache for every source (`on`), the full
/// index for every source (`off`), or unset - the rule (a jobdeck's sources
/// light, a layout full).
pub fn light_env() -> Option<bool> {
    match std::env::var("FLOE_INDEX_LIGHT")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "on" | "yes" | "1" | "light" => Some(true),
        "off" | "no" | "0" | "full" => Some(false),
        _ => None,
    }
}
impl IndexOptions {
    /// The option given that the light cache has no use for - it has no
    /// pages, no page BVH, no LOD, no text index - so the default rule builds
    /// the full index for it, and `--light` refuses it. (`--no-page-occupancy`
    /// and `--no-ovs` leave files out of the full index: moot under light, a
    /// deck's sources carry the latter, so neither counts.)
    pub fn full_only(&self) -> Option<&'static str> {
        if self.lod {
            Some("--lod")
        } else if self.page_target_mb.is_some() {
            Some("--page-target-mb")
        } else if self.wants_representatives() {
            Some("--representatives")
        } else if self.hier_only {
            Some("--hier-only")
        } else if self.profile_cell.is_some() {
            Some("--profile-cell")
        } else if self.mode == IndexMode::Stream {
            Some("--stream")
        } else if self.mode == IndexMode::InMemory {
            Some("--in-memory")
        } else if self.slow_cell_s.is_some() {
            Some("--slow-cell-s")
        } else if self.p2_shard_limit_mb.is_some() {
            Some("--p2-shard-limit-mb")
        } else {
            None
        }
    }
    /// Whether this run builds the light cache: `--light` / `--full` as
    /// given; else the full index when an option of its own was given; else
    /// FLOE_INDEX_LIGHT; else a jobdeck's source light, a layout full
    /// (DIRECT_READ_PLAN §10, decision 2).
    pub fn light_for(&self, deck_source: bool) -> bool {
        if let Some(light) = self.light {
            return light;
        }
        if self.full_only().is_some() {
            return false;
        }
        light_env().unwrap_or(deck_source)
    }
    pub fn validate(&self) -> Result<()> {
        if self.light == Some(true) {
            if let Some(flag) = self.full_only() {
                return Err(Error::input(format!(
                    "--light builds the light cache (no pages, LOD or text index): not with {flag}; it takes --jobs, --occupancy*, --mem, --force and --wait"
                )));
            }
        }
        if self
            .representatives_format
            .is_some_and(|n| n != 1 && n != 2)
        {
            return Err(Error::input("representatives-format must be 1 or 2"));
        }
        if self
            .representatives_points
            .is_some_and(|n| n == 0 || n > 4_194_304)
        {
            return Err(Error::input(
                "representatives-points must be in 1..=4194304",
            ));
        }
        if self.wants_representatives()
            && (self.profile_cell.is_some()
                || self.occupancy_only
                || (self.representatives_only && self.wants_occupancy()))
        {
            return Err(Error::input("representatives cannot be combined with profiling or other additive-only summaries"));
        }
        if self.mem_gib.is_some() && self.mode == IndexMode::InMemory {
            return Err(Error::input("mem belongs to the streaming build, not in-memory"));
        }
        if self.mem_gib.is_some_and(|v| !v.is_finite() || v <= 0.) {
            return Err(Error::input("invalid mem (GiB)"));
        }
        if self.mode == IndexMode::Stream
            && (self.profile_cell.is_some()
                || self.representatives_only
                || self.representatives
                || self.hier_only)
        {
            return Err(Error::input(
                "stream cannot be combined with profiling, representatives or hier-only",
            ));
        }
        if self.jobs == 0 || self.jobs.checked_mul(8).is_none() || self.profile_repeat == 0 {
            return Err(Error::input(
                "jobs/repeat must be positive and representable",
            ));
        }
        if let Some(v) = self.page_target_mb {
            if v == 0 || v.checked_mul(1 << 20).is_none() {
                return Err(Error::input("invalid page-target-mb"));
            }
        }
        if self
            .p2_shard_limit_mb
            .is_some_and(|n| n.checked_mul(1 << 20).is_none())
        {
            return Err(Error::input("p2-shard-limit-mb overflows bytes"));
        }
        if self
            .occupancy_mag
            .is_some_and(|v| !v.is_finite() || v <= 0.)
        {
            return Err(Error::input("occupancy-mag must be finite/positive"));
        }
        if self.occupancy_um.is_some_and(|v| !v.is_finite() || v <= 0.)
            || self.slow_cell_s.is_some_and(|v| !v.is_finite() || v < 0.)
        {
            return Err(Error::input(
                "occupancy-um must be finite/positive; slow-cell-s finite/nonnegative",
            ));
        }
        // Summary-only overrides the ordinary build default. Explicit CLI
        // mode flags remain mutually exclusive at the parser boundary.
        if self.profile_jobs.as_ref().is_some_and(|v| {
            v.is_empty() || v.iter().any(|&n| n == 0 || n.checked_mul(8).is_none())
        }) {
            return Err(Error::input(
                "profile-jobs must be a nonempty positive list",
            ));
        }
        if matches!(&self.profile_cell, Some(ProfileCell::Name(s)) if s.is_empty()) {
            return Err(Error::input("profile-cell cannot be empty"));
        }
        if self.profile_cell.is_none()
            && (self.profile_jobs.is_some()
                || self.profile_repeat != 1
                || self.profile_snapshot.is_some()
                || self.profile_snapshot_refresh)
        {
            return Err(Error::input(
                "profile jobs/repeat/snapshot options require a profile cell selector",
            ));
        }
        if self.profile_snapshot_refresh && self.profile_snapshot.is_none() {
            return Err(Error::input(
                "--profile-snapshot-refresh requires --profile-snapshot",
            ));
        }
        if self.hier_only
            && (self.profile_cell.is_some() || self.occupancy_only || self.representatives_only)
        {
            return Err(Error::input(
                "--hier-only adds the cell tree's index alone; it takes no other additive or profile option",
            ));
        }
        if self.profile_cell.is_some()
            && (self.force || self.occupancy_only || self.slow_cell_s.is_some())
        {
            return Err(Error::input(
                "cell profiling cannot be combined with force, occupancy or slow-cell-s",
            ));
        }
        Ok(())
    }
    fn wants_occupancy(&self) -> bool {
        self.occupancy.unwrap_or(false) || self.occupancy_um.is_some()
    }
    pub fn wants_representatives(&self) -> bool {
        self.representatives
            || self.representatives_only
            || self.representatives_points.is_some()
            || self.representatives_format.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Reuse,
    OccupancyPresent,
    Build,
    OccupancyOnly,
    RepresentativesOnly,
    SummariesOnly,
    /// `floe-index hier` on a current cache (`--hier-only`)
    HierOnly,
    Profile,
}

/// Pure policy decision, independently testable without spawning a process.
/// `kind` is the cache there (None: none); `light` whether this run would
/// build the light cache (`IndexOptions::light_for`): a current light cache
/// under a full request is built over (the full index holds everything the
/// light one did, so nothing is lost and no --force is needed); a current
/// full index under a light request is kept (it answers all the light cache
/// would); the additive runs that need the full index's files refuse a
/// light cache.
pub fn decide(
    options: &IndexOptions,
    state: &CacheState,
    has_occupancy: bool,
    has_representatives: bool,
    kind: Option<cache::CacheKind>,
    light: bool,
) -> Result<Action> {
    options.validate()?;
    if options.profile_cell.is_some() {
        return Ok(Action::Profile);
    }
    let light_here = kind == Some(cache::CacheKind::Light);
    if options.representatives_only {
        if *state != CacheState::Current {
            return Err(Error::new(
                ErrorKind::Cache,
                "--representatives-only needs a current cache",
            ));
        }
        if light_here {
            return Err(Error::new(
                ErrorKind::Cache,
                "--representatives-only needs the full index (the cache is light; index --full builds it)",
            ));
        }
        return Ok(Action::RepresentativesOnly);
    }
    if options.occupancy_only {
        if *state != CacheState::Current {
            return Err(Error::new(
                ErrorKind::Cache,
                "--occupancy-only needs a current cache",
            ));
        }
        return Ok(Action::OccupancyOnly);
    }
    if options.hier_only {
        if *state != CacheState::Current {
            return Err(Error::new(
                ErrorKind::Cache,
                "--hier-only needs a current cache",
            ));
        }
        if light_here {
            return Err(Error::new(
                ErrorKind::Cache,
                "--hier-only: a light cache's cell tree comes from its inventory, it has no design.ovh to build (index --full builds the full index)",
            ));
        }
        return Ok(Action::HierOnly);
    }
    if *state == CacheState::Current && light_here && !light {
        // the full index over the light cache: nothing of the light cache
        // is lost (the streaming build leaves its files beside the index)
        return Ok(Action::Build);
    }
    if *state == CacheState::Current && !options.force {
        if options.wants_representatives()
            && (!has_representatives
                || options.representatives_points.is_some()
                || options.representatives_format.is_some())
        {
            return Ok(if options.wants_occupancy() && !has_occupancy {
                Action::SummariesOnly
            } else {
                Action::RepresentativesOnly
            });
        }
        // a current cache without design.ovo gets it from an occupancy-only
        // run in every mode: the streaming one streams it too (the
        // switch-over, STREAMING_INDEX_PLAN §10; a cache whose placements
        // exceed the pass's cap is told so and kept as it is)
        return Ok(if options.wants_occupancy() {
            if has_occupancy {
                Action::OccupancyPresent
            } else {
                Action::OccupancyOnly
            }
        } else {
            Action::Reuse
        });
    }
    if *state != CacheState::Missing && !options.force {
        return Err(Error::new(
            ErrorKind::Cache,
            format!("refusing to replace existing cache: {state:?}; rerun with --force"),
        ));
    }
    Ok(Action::Build)
}

/// The full index's command line that takes the light cache's place when
/// `floe-index light` cannot read the stream (a cell defined inside a
/// CBLOCK; floe_vfs::light::UNSUITABLE_EXIT, nothing written): for a build
/// whose light cache was the rule's choice (`light` None - a jobdeck's
/// source, FLOE_INDEX_LIGHT); none for `--light` given, another action, or
/// a full run. IndexJob chains it after that exit.
fn fallback_arguments(
    source: &Path,
    directory: &Path,
    o: &IndexOptions,
    action: &Action,
    light_run: bool,
) -> Result<Option<Vec<OsString>>> {
    if !(light_run && *action == Action::Build && o.light.is_none()) {
        return Ok(None);
    }
    let mut full = o.clone();
    full.light = Some(false);
    Ok(Some(arguments(source, directory, &full, &Action::Build, false)?))
}

/// what an indexer's exit status means, for the lines that report one
/// (the deck index's FAILED lines, the auto-index's error): floe-index's
/// own codes - the lock's floe_vfs::lock::BUSY_EXIT, the memory guard's
/// 71 (STREAMING_INDEX_PLAN §5) - and the kernel's 128 + signal
pub fn exit_words(code: i32) -> Option<&'static str> {
    Some(match code {
        71 => "stopped at the memory ceiling or refused for its size: the indexer's line above says what to do (a lower --mem for the streaming build, --stream for an in-memory one)",
        c if c == floe_vfs::lock::BUSY_EXIT => "busy: another run or a viewer holds the cache",
        c if c == floe_vfs::light::UNSUITABLE_EXIT => "the light cache cannot read this source (a cell is defined inside a CBLOCK): index --full builds the full index",
        130 | 143 => "cancelled",
        134 => "aborted (a panic or a failed assertion)",
        137 => "killed (out of memory?): the host's OOM killer or a kill -9; a lower --mem, or --stream if it was built in memory",
        139 => "segmentation fault",
        _ => return None,
    })
}

fn arguments(
    source: &Path,
    directory: &Path,
    o: &IndexOptions,
    action: &Action,
    light: bool,
) -> Result<Vec<OsString>> {
    if *action == Action::HierOnly {
        return Ok(vec!["hier".into(), directory.as_os_str().to_owned()]);
    }
    fn add(a: &mut Vec<OsString>, flag: &str, value: impl ToString) {
        a.extend([flag.into(), value.to_string().into()]);
    }
    // the light cache (floe-index light): the inventory pass's files, the
    // summary with --occupancy, --occupancy-only on a light cache
    if light && matches!(action, Action::Build | Action::OccupancyOnly) {
        let mut a: Vec<OsString> = vec![
            "light".into(),
            source.as_os_str().to_owned(),
            "--out".into(),
            directory.as_os_str().to_owned(),
        ];
        add(&mut a, "--jobs", o.jobs);
        if *action == Action::OccupancyOnly {
            a.push("--occupancy-only".into());
            occupancy_args(&mut a, o);
        } else {
            if o.wants_occupancy() {
                a.push("--occupancy".into());
                occupancy_args(&mut a, o);
            }
            if o.force {
                a.push("--force".into());
            }
        }
        if let Some(g) = o.mem_gib {
            a.extend(["--mem".into(), g.to_string().into()]);
        }
        return Ok(a);
    }
    let mut a: Vec<OsString> = vec!["vfs".into(), source.as_os_str().to_owned()];
    if *action != Action::Profile {
        a.push(directory.as_os_str().to_owned());
    }
    add(&mut a, "--jobs", o.jobs);
    if matches!(action, Action::RepresentativesOnly | Action::SummariesOnly) {
        a.push("--representatives-only".into());
        if let Some(n) = o.representatives_points {
            add(&mut a, "--representatives-points", n);
        }
        if let Some(n) = o.representatives_format {
            add(&mut a, "--representatives-format", n);
        }
        return Ok(a);
    }
    if *action == Action::OccupancyOnly {
        a.push("--occupancy-only".into());
        occupancy_args(&mut a, o);
        mode_args(&mut a, o);
        return Ok(a);
    }
    if let Some(v) = o.page_target_mb {
        add(&mut a, "--page-target-mb", v);
    }
    if *action == Action::Build && o.wants_representatives() {
        a.push("--representatives".into());
        if let Some(n) = o.representatives_format {
            add(&mut a, "--representatives-format", n);
        }
        if let Some(n) = o.representatives_points {
            add(&mut a, "--representatives-points", n);
        }
    }
    if *action != Action::Profile && o.wants_occupancy() {
        a.push("--occupancy".into());
        occupancy_args(&mut a, o);
    }
    if *action == Action::Build {
        mode_args(&mut a, o);
    }
    if !o.lod {
        a.push("--no-lod".into());
    }
    if !o.page_occupancy {
        a.push("--no-page-occupancy".into());
    }
    if !o.ovs && *action != Action::Profile {
        a.push("--no-ovs".into());
    }
    if let Some(v) = o.slow_cell_s {
        add(&mut a, "--slow-cell-s", v);
    }
    if let Some(v) = o.p2_shard_limit_mb {
        add(&mut a, "--p2-shard-limit-mb", v);
    }
    match &o.profile_cell {
        Some(ProfileCell::Name(s)) => add(&mut a, "--profile-cell", s),
        Some(ProfileCell::Index(n)) => add(&mut a, "--profile-cell-ci", n),
        None => (),
    }
    if let Some(v) = &o.profile_jobs {
        add(
            &mut a,
            "--profile-jobs",
            v.iter().map(usize::to_string).collect::<Vec<_>>().join(","),
        );
    }
    if o.profile_repeat != 1 {
        add(&mut a, "--profile-repeat", o.profile_repeat);
    }
    if let Some(p) = &o.profile_snapshot {
        a.extend([
            "--profile-snapshot".into(),
            cache::absolute(p)?.into_os_string(),
        ]);
    }
    if o.profile_snapshot_refresh {
        a.push("--profile-snapshot-refresh".into());
    }
    Ok(a)
}

/// the build's mode for the runs that parse the source (the whole build
/// and the occupancy-only run): `Auto` passes nothing - floe-index's own
/// default (streaming, or FLOE_INDEX_STREAM) decides
fn mode_args(a: &mut Vec<OsString>, o: &IndexOptions) {
    match o.mode {
        IndexMode::Auto => {}
        IndexMode::Stream => a.push("--stream".into()),
        IndexMode::InMemory => a.push("--in-memory".into()),
    }
    if let Some(g) = o.mem_gib {
        a.extend(["--mem".into(), g.to_string().into()]);
    }
}

/// The summary's options as the Python CLI gave them (`_occupancy_args`):
/// the base cell (as Python writes a float: `2.0`), the split, the pruning.
fn occupancy_args(a: &mut Vec<OsString>, o: &IndexOptions) {
    if let Some(v) = o.occupancy_um {
        a.extend(["--occupancy-um".into(), format!("{v:?}").into()]);
    } else if let Some(v) = o.occupancy_mag {
        a.extend(["--occupancy-mag".into(), format!("{v:?}").into()]);
    }
    if let Some(v) = o.occupancy_balance {
        a.extend(["--occupancy-balance".into(), u8::from(v).to_string().into()]);
    }
    if let Some(v) = o.occupancy_prune {
        a.extend(["--occupancy-prune".into(), u8::from(v).to_string().into()]);
    }
    if let Some(v) = o.occupancy_max_work {
        a.extend(["--occupancy-max-work".into(), v.to_string().into()]);
    }
    if let Some(v) = o.occupancy_max_bytes {
        a.extend(["--occupancy-max-bytes".into(), v.to_string().into()]);
    }
}

pub(crate) struct WriteLease(File);
impl WriteLease {
    pub(crate) fn acquire_aliases(paths: &[PathBuf]) -> Result<Vec<Self>> {
        // Fixed lexical order and both spellings serialize old/new writers.
        // Keep stable lock inodes; never remove them after a rename.
        let ordered: std::collections::BTreeSet<_> = paths.iter().collect();
        ordered.into_iter().map(|p| Self::acquire(p)).collect()
    }
    pub(crate) fn acquire(directory: &Path) -> Result<Self> {
        Self::open(directory, true)
    }
    /// Read-only preparation/reclamation must not invent a missing lock inode.
    pub(crate) fn acquire_existing(directory: &Path) -> Result<Self> {
        Self::open(directory, false)
    }
    fn open(directory: &Path, create: bool) -> Result<Self> {
        let mut path = directory.as_os_str().to_owned();
        path.push(".index.lock");
        // Persistent zero-byte inode: unlink-on-unlock would allow a second
        // writer to lock a different inode while the first still holds it.
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(PathBuf::from(path))?;
        let m = f.metadata()?;
        if !m.is_file() || m.nlink() != 1 {
            return Err(Error::input("index lock must be a private regular file"));
        }
        match f.try_lock() {
            Ok(()) => Ok(Self(f)),
            // which source (2026-10-15: two runs started at once, the one
            // refused here did not say what it was refused)
            Err(std::fs::TryLockError::WouldBlock) => Err(Error::new(
                ErrorKind::Busy,
                format!(
                    "another Rust application is indexing this cache ({}) - try again when it finishes",
                    floe_vfs::lock::key(floe_vfs::lock::Kind::Vfs, &directory.to_string_lossy()).subject
                ),
            )),
            Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }
}
impl Drop for WriteLease {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub struct PreparedIndex {
    action: Action,
    source: PathBuf,
    directory: PathBuf,
    args: Vec<OsString>,
    indexer: Indexer,
    lease: Vec<WriteLease>,
    migration: Option<cache::Migration>,
    cleanup_occupancy: bool,
    cleanup_representatives: bool,
    next_args: Option<Vec<OsString>>,
    /// the full index's run in the light cache's place when floe-index
    /// light cannot read the stream (`fallback_arguments`)
    fallback_args: Option<Vec<OsString>>,
    /// the indexer's stdout onto stderr (`on_stderr`)
    on_stderr: bool,
    /// this run builds the light cache (`IndexOptions::light_for`)
    light: bool,
    /// what the run says about the cache it found (a light cache built
    /// over, a full index kept under --light)
    note: Option<String>,
}
impl PreparedIndex {
    /// Whether this run builds (or adds to) the light cache.
    pub fn light(&self) -> bool {
        self.light
    }
    /// A line about the cache found, when there is something to say.
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
    /// The indexer's output all on stderr: an index a read command runs
    /// first (app-cli ready.rs), its stdout the command's (`info --json`).
    pub fn on_stderr(mut self) -> Self {
        self.on_stderr = true;
        self
    }
    pub fn action(&self) -> &Action {
        &self.action
    }
    pub fn source(&self) -> &Path {
        &self.source
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn arguments(&self) -> &[OsString] {
        &self.args
    }
    pub fn migration(&self) -> Option<cache::Migration> {
        self.migration
    }
    /// The indexer's command lines this run starts, in order (the second
    /// is a second summary pass): what a front end shows before it runs.
    pub fn commands(&self) -> Vec<Vec<OsString>> {
        std::iter::once(&self.args)
            .chain(self.next_args.as_ref())
            .map(|args| {
                std::iter::once(self.indexer.path().as_os_str().to_owned())
                    .chain(args.iter().cloned())
                    .collect()
            })
            .collect()
    }
    pub fn prepare(
        source: &Path,
        options: &IndexOptions,
        indexer: Indexer,
        cancelled: &AtomicUsize,
    ) -> Result<Self> {
        options.validate()?;
        if cancelled.load(Ordering::Relaxed) != 0 {
            return Err(Error::new(
                ErrorKind::Cancelled,
                "index cancelled before preparation",
            ));
        }
        let source = cache::absolute(source)?;
        if source
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("jb"))
        {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "jobdeck indexing is not yet ported (M1a-3); use the existing floe2 index",
            ));
        }
        if !source.exists() {
            return Err(Error::input(format!(
                "source not found: {}",
                source.display()
            )));
        }
        cache::fingerprint(&source)?;
        indexer.verify(cancelled)?;
        let mut directory = cache::cache_path(&source)?;
        if let Some(snapshot) = &options.profile_snapshot {
            let snapshot = cache::absolute(snapshot)?;
            let name = snapshot
                .file_name()
                .ok_or_else(|| Error::input("snapshot must name a file"))?;
            let resolved = fs::canonicalize(&snapshot)
                .or_else(|_| {
                    fs::canonicalize(snapshot.parent().expect("absolute file parent"))
                        .map(|p| p.join(name))
                })
                .unwrap_or_else(|_| snapshot.clone());
            let mut protected = cache::cache_paths(&source)?.to_vec();
            protected.push(cache::revision::Store::new(&source)?.path().to_owned());
            protected.push(cache::revision::set::Store::new(&source)?.path().to_owned());
            for candidate in protected {
                let cache_resolved =
                    fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
                if snapshot.starts_with(&candidate) || resolved.starts_with(&cache_resolved) {
                    return Err(Error::input(
                        "profile snapshot must be outside current, legacy and revision caches",
                    ));
                }
            }
        }
        let profiling = options.profile_cell.is_some();
        // another run writing the cache refuses this one at once - before
        // the cache is looked at, where its half-written files would read
        // as "not current, rerun with --force" (floe/indexlock.py). Readers
        // in the way are floe-index's to judge.
        if !profiling {
            let key = floe_vfs::lock::key(floe_vfs::lock::Kind::Vfs, &directory.to_string_lossy());
            if let Some(busy) = floe_vfs::lock::writing_refusal(&key) {
                return Err(Error::new(ErrorKind::Busy, busy.to_string()));
            }
        }
        let lease = if profiling {
            Vec::new()
        } else {
            WriteLease::acquire_aliases(&cache::cache_paths(&source)?)?
        };
        // Resolution must be rechecked under both writer locks. Read-only
        // profile intentionally acquires neither locks nor migration authority.
        if !profiling {
            directory = cache::cache_path(&source)?;
        }
        let mut before = None;
        if !profiling {
            match fs::symlink_metadata(&directory) {
                Ok(m) if m.is_dir() && !m.file_type().is_symlink() => before = Some(m),
                Ok(_) => {
                    return Err(Error::input(
                        "cache destination must be a real directory, even with --force",
                    ))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        let state = if profiling {
            CacheState::Missing
        } else {
            cache::inspect(&source, &directory)?
        };
        let kind = if profiling { None } else { cache::cache_kind(&directory) };
        // a jobdeck's source (jobdeck::index sets `deck_source`) and a layout
        // alone follow their rules; `light` itself stays as given, so a
        // light cache the rule chose may give way to the full index
        let light = options.light_for(options.deck_source);
        let action = decide(
            options,
            &state,
            directory.join("design.ovo").is_file(),
            directory.join("design.ovr").is_file(),
            kind,
            light,
        )?;
        // the run the arguments are for: an occupancy-only run on a light
        // cache is the light command's; a build is what was asked
        let light_run = match action {
            Action::OccupancyOnly => kind == Some(cache::CacheKind::Light),
            Action::Build => light,
            _ => false,
        };
        let note = match (kind, action.clone(), light) {
            (Some(cache::CacheKind::Light), Action::Build, false) => Some(format!(
                "the light cache of {} becomes the full index (its files stay beside it)",
                source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            )),
            (Some(cache::CacheKind::Full), Action::Reuse | Action::OccupancyPresent | Action::OccupancyOnly, true) => Some(format!(
                "{} has the full index, which answers everything the light cache would; kept (--force --light replaces it)",
                source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            )),
            _ => None,
        };
        if cancelled.load(Ordering::Relaxed) != 0 {
            return Err(Error::new(
                ErrorKind::Cancelled,
                "index cancelled during cache validation",
            ));
        }
        let destination = if profiling {
            directory.clone()
        } else {
            cache::default_cache_path(&source)?
        };
        let args = arguments(&source, &destination, options, &action, light_run)?;
        // the light cache the rule chose over a stream it cannot read (a cell
        // defined inside a CBLOCK: floe-index light exits UNSUITABLE_EXIT
        // before writing anything): the full index is built in its place
        // (IndexJob chains it); `--light` given is refused as it is
        let fallback_args = fallback_arguments(&source, &destination, options, &action, light_run)?;
        // Both additive passes retain the same locks and migration receipt.
        // If the second fails, the first publication remains; this is not a
        // transaction across independent summary files.
        let next_args = if action == Action::SummariesOnly {
            Some(arguments(
                &source,
                &destination,
                options,
                &Action::OccupancyOnly,
                false,
            )?)
        } else {
            None
        };
        let migration = if directory != destination {
            Some(cache::rename_legacy(
                &directory,
                &destination,
                before
                    .as_ref()
                    .ok_or_else(|| Error::new(ErrorKind::Cache, "legacy cache disappeared"))?,
                true,
                cancelled,
            )?)
        } else {
            None
        };
        // No fallible preparation after this commit: callers must receive the
        // name-change receipt even if launching/rebuilding is later cancelled.
        Ok(Self {
            action,
            source,
            directory: destination,
            args,
            indexer,
            lease,
            migration,
            cleanup_occupancy: !profiling && (options.wants_occupancy() || options.occupancy_only),
            cleanup_representatives: !profiling && options.wants_representatives(),
            next_args,
            fallback_args,
            on_stderr: false,
            light: light_run,
            note,
        })
    }
    pub fn start(self, cancelled: &AtomicUsize) -> Result<IndexJob> {
        self.start_io(cancelled, false)
    }
    pub fn start_captured(self, cancelled: &AtomicUsize) -> Result<IndexJob> {
        self.start_io(cancelled, true)
    }
    fn start_io(mut self, cancelled: &AtomicUsize, capture: bool) -> Result<IndexJob> {
        if matches!(self.action, Action::Reuse | Action::OccupancyPresent) {
            return Err(Error::input("a reused cache does not launch an indexer"));
        }
        if cancelled.load(Ordering::Relaxed) != 0 {
            return Err(Error::new(
                ErrorKind::Cancelled,
                "index cancelled before launch",
            ));
        }
        let output = if capture {
            crate::native::Output::Capture
        } else if self.on_stderr {
            crate::native::Output::Stderr
        } else {
            crate::native::Output::Inherit
        };
        let mut child = self.indexer.spawn_output(&self.args, output, &[])?;
        let capture = if capture {
            match crate::index_progress::Capture::take(&mut child) {
                Ok(c) => Some(c),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e);
                }
            }
        } else {
            None
        };
        Ok(IndexJob {
            child: Some(child),
            lease: std::mem::take(&mut self.lease),
            directory: self.directory.clone(),
            cleanup_occupancy: self.cleanup_occupancy,
            cleanup_representatives: self.cleanup_representatives,
            next: self.next_args.take().map(|a| (self.indexer.clone(), a)),
            fallback: self.fallback_args.take().map(|a| (self.indexer.clone(), a)),
            fell_back: false,
            finished: None,
            capture,
            output,
        })
    }
}

pub struct IndexJob {
    child: Option<Child>,
    lease: Vec<WriteLease>,
    directory: PathBuf,
    cleanup_occupancy: bool,
    cleanup_representatives: bool,
    next: Option<(Indexer, Vec<OsString>)>,
    /// the full index's run chained when the light cache's exits
    /// UNSUITABLE_EXIT (`fallback_arguments`), and whether it was
    fallback: Option<(Indexer, Vec<OsString>)>,
    fell_back: bool,
    finished: Option<i32>,
    capture: Option<crate::index_progress::Capture>,
    output: crate::native::Output,
}
/// floe-index's own locks (floe_vfs::lock) off, for a run whose target is
/// private to this build - a revision candidate, a DRC staging directory: no
/// other run can name it, and the `.floe-lock` folder would land inside the
/// private directory (a revision store's unknown entry, a staging directory
/// its cleanup refuses).
pub(crate) const PRIVATE_TARGET: &[(&str, &str)] = &[("FLOE_LOCK", "off")];
impl IndexJob {
    /// Reuse the subprocess/cancel/reap lifecycle for DRC staging. Its caller
    /// owns publication and the write lease; VFS cleanup is deliberately off.
    /// `env` is added to the indexer's environment (PRIVATE_TARGET).
    pub(crate) fn captured(
        indexer: &Indexer,
        args: &[OsString],
        env: &[(&str, &str)],
    ) -> Result<Self> {
        let mut child = indexer.spawn_env(args, true, env)?;
        let capture = match crate::index_progress::Capture::take(&mut child) {
            Ok(c) => c,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        Ok(Self {
            child: Some(child),
            lease: Vec::new(),
            directory: PathBuf::new(),
            cleanup_occupancy: false,
            cleanup_representatives: false,
            next: None,
            fallback: None,
            fell_back: false,
            finished: None,
            capture: Some(capture),
            output: crate::native::Output::Capture,
        })
    }
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
    /// Whether the light cache's run gave way to the full index's
    /// (floe_vfs::light::UNSUITABLE_EXIT: a cell defined inside a CBLOCK).
    pub fn fell_back(&self) -> bool {
        self.fell_back
    }
    pub fn progress(&self) -> Option<&crate::index_progress::Progress> {
        self.capture.as_ref().map(|c| &c.progress)
    }
    pub fn poll(&mut self) -> Result<Option<i32>> {
        if self.finished.is_some() {
            return Ok(self.finished);
        }
        if let Some(c) = self.capture.as_mut() {
            c.drain()?;
        }
        if let Some(status) = self.child.as_mut().expect("running child").try_wait()? {
            let code = status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(1));
            // chained: the full index in the light cache's place
            // (UNSUITABLE_EXIT), or the second summary pass after a success
            let chained = match code {
                c if c == floe_vfs::light::UNSUITABLE_EXIT => {
                    let fallback = self.fallback.take();
                    if fallback.is_some() {
                        self.fell_back = true;
                    }
                    fallback
                }
                0 => self.next.take(),
                _ => None,
            };
            {
                if let Some((indexer, args)) = chained {
                    if let Some(c) = self.capture.as_mut() {
                        let _ = c.finish();
                    }
                    let started = (|| {
                        let mut child = indexer.spawn_output(&args, self.output, &[])?;
                        let capture = if self.capture.is_some() {
                            match crate::index_progress::Capture::take(&mut child) {
                                Ok(mut c) => {
                                    if let Some(previous) = &self.capture {
                                        c.progress.output_bytes = previous.progress.output_bytes;
                                        c.progress.dropped_lines = previous.progress.dropped_lines;
                                    }
                                    Some(c)
                                }
                                Err(e) => {
                                    let _ = child.kill();
                                    let _ = child.wait();
                                    return Err(e);
                                }
                            }
                        } else {
                            None
                        };
                        Ok((child, capture))
                    })();
                    match started {
                        Ok((child, capture)) => {
                            self.child = Some(child);
                            self.capture = capture;
                            return Ok(None);
                        }
                        Err(e) => {
                            self.finish(1);
                            return Err(e);
                        }
                    }
                }
            }
            self.finish(code);
            return Ok(Some(code));
        }
        Ok(None)
    }
    pub fn cancel(&mut self, signal: i32) -> Result<i32> {
        if !matches!(signal, libc::SIGINT | libc::SIGTERM | libc::SIGHUP) {
            return Err(Error::input("cancel signal must be SIGINT/SIGTERM/SIGHUP"));
        }
        if let Some(code) = self.finished {
            return Ok(code);
        }
        // Do not use poll(): it may advance a completed first pass. A pending
        // second pass makes this cancellation, not whole-operation success.
        if let Some(status) = self.child.as_mut().expect("running child").try_wait()? {
            let code = if status.success() && self.next.is_some() {
                128 + signal
            } else {
                status
                    .code()
                    .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
            };
            self.finish(code);
            return Ok(code);
        }
        self.next = None;
        self.fallback = None;
        native::signal_child(self.child.as_ref().expect("running child"), signal)?;
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if self.child.as_mut().unwrap().try_wait()?.is_some() {
                self.finish(128 + signal);
                return Ok(128 + signal);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let child = self.child.as_mut().unwrap();
        child.kill()?;
        child.wait()?;
        self.finish(128 + signal);
        Ok(128 + signal)
    }
    fn finish(&mut self, code: i32) {
        self.next = None;
        self.fallback = None;
        self.child.take();
        // Telemetry is best-effort. Do not turn the native exit status into a
        // different result because of a late pipe failure during final drain.
        if let Some(c) = self.capture.as_mut() {
            let _ = c.finish();
        }
        self.finished = Some(code);
        // A run the index locks refused (floe_vfs::lock::BUSY_EXIT) wrote
        // nothing: a summary temp there is the lock holder's, never ours.
        if code != 0 && code != floe_vfs::lock::BUSY_EXIT {
            if let Err(e) = self.discard_occupancy_tmp() {
                // A cleanup problem must not replace the native failure or
                // cancellation status. Keep the path for explicit recovery.
                eprintln!("[{}] cannot clean summary temp: {e}", crate::program());
            }
        }
        self.lease.clear();
    }
    fn discard_occupancy_tmp(&self) -> Result<()> {
        if !(self.cleanup_occupancy || self.cleanup_representatives) {
            return Ok(());
        }
        // only under the cache's writer lock (floe_vfs::lock): once the run
        // has ended, another run may be writing the same names - its files
        // are left be (floe/cli.py _discard_occupancy_tmp)
        let key = floe_vfs::lock::key(floe_vfs::lock::Kind::Vfs, &self.directory.to_string_lossy());
        let _lock = match floe_vfs::lock::writer(
            &key,
            floe_vfs::lock::Mode::Additive,
            &format!("{} index (temporary file cleanup)", crate::program()),
        ) {
            Ok(lock) => lock,
            Err(_) => return Ok(()),
        };
        for (enabled, name) in [
            (self.cleanup_occupancy, "design.ovo.tmp"),
            (self.cleanup_occupancy, floe_vfs::places::FILE_NAME),
            (self.cleanup_occupancy, floe_vfs::occupancy::PLANES_TMP),
            (self.cleanup_representatives, "design.ovr.tmp"),
        ] {
            if !enabled {
                continue;
            }
            match fs::remove_file(self.directory.join(name)) {
                Ok(()) => eprintln!(
                    "[{}] discarded {}",
                    crate::program(),
                    self.directory.join(name).display()
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}
impl Drop for IndexJob {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = self.discard_occupancy_tmp();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelling_between_additive_passes_is_not_success() {
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        assert!(child.wait().unwrap().success());
        let indexer = Indexer::discover(&native::Discovery {
            override_path: Some(PathBuf::from("/usr/bin/false")),
            development_root: None,
            executable: PathBuf::from("/unused"),
            search_path: None,
        })
        .unwrap();
        let mut job = IndexJob {
            child: Some(child),
            lease: Vec::new(),
            directory: PathBuf::new(),
            cleanup_occupancy: false,
            cleanup_representatives: false,
            next: Some((indexer, Vec::new())),
            fallback: None,
            fell_back: false,
            finished: None,
            capture: None,
            output: native::Output::Inherit,
        };
        assert_eq!(job.cancel(libc::SIGINT).unwrap(), 130);
        assert_eq!(job.poll().unwrap(), Some(130));
        assert!(job.child.is_none() && job.next.is_none());
    }
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("floe-index-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    #[test]
    fn a_run_the_index_locks_refuse_keeps_the_summary_temp_it_did_not_write() {
        let dir = scratch("busy-temp");
        for (code, kept) in [(floe_vfs::lock::BUSY_EXIT, true), (1, false)] {
            fs::write(dir.join("design.ovo.tmp"), b"holder").unwrap();
            let child = std::process::Command::new("/bin/sh")
                .args(["-c", &format!("exit {code}")])
                .spawn()
                .unwrap();
            let mut job = IndexJob {
                child: Some(child),
                lease: Vec::new(),
                directory: dir.clone(),
                cleanup_occupancy: true,
                cleanup_representatives: false,
                next: None,
                fallback: None,
                fell_back: false,
                finished: None,
                capture: None,
                output: native::Output::Inherit,
            };
            let done = loop {
                if let Some(code) = job.poll().unwrap() {
                    break code;
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            assert_eq!(done, code);
            assert_eq!(dir.join("design.ovo.tmp").exists(), kept, "exit {code}");
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn a_private_target_runs_the_indexer_without_its_locks() {
        let dir = scratch("private-target");
        let script = dir.join("fake-index");
        fs::write(&script, "#!/bin/sh\nprintf %s \"$FLOE_LOCK\" > \"$1\"\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let indexer = Indexer::discover(&native::Discovery {
            override_path: Some(script),
            development_root: None,
            executable: PathBuf::from("/unused"),
            search_path: None,
        })
        .unwrap();
        let seen = dir.join("seen");
        let mut job =
            IndexJob::captured(&indexer, &[seen.clone().into_os_string()], PRIVATE_TARGET).unwrap();
        let done = loop {
            if let Some(code) = job.poll().unwrap() {
                break code;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(done, 0);
        assert_eq!(fs::read_to_string(&seen).unwrap(), "off");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn the_light_cache_decisions() {
        // DIRECT_READ_PLAN L5: a full request over a current light cache
        // builds (nothing is lost); a light request over a full index keeps
        // it; the additive runs that need the full index's files refuse a
        // light cache; a light cache without its summary gets it from the
        // light command's occupancy-only run
        use crate::cache::CacheKind::{Full, Light};
        let o = IndexOptions::default();
        assert_eq!(decide(&o, &CacheState::Current, false, false, Some(Light), false).unwrap(), Action::Build);
        assert_eq!(decide(&o, &CacheState::Current, false, false, Some(Light), true).unwrap(), Action::Reuse);
        assert_eq!(decide(&o, &CacheState::Current, false, false, Some(Full), true).unwrap(), Action::Reuse);
        assert_eq!(decide(&o, &CacheState::Missing, false, false, None, true).unwrap(), Action::Build);
        let mut o = IndexOptions {
            occupancy: Some(true),
            ..Default::default()
        };
        assert_eq!(decide(&o, &CacheState::Current, false, false, Some(Light), true).unwrap(), Action::OccupancyOnly);
        assert_eq!(decide(&o, &CacheState::Current, true, false, Some(Light), true).unwrap(), Action::OccupancyPresent);
        o.occupancy = None;
        o.hier_only = true;
        assert!(decide(&o, &CacheState::Current, false, false, Some(Light), false).is_err());
        o.hier_only = false;
        o.representatives_only = true;
        assert!(decide(&o, &CacheState::Current, false, false, Some(Light), false).is_err());
        // the arguments: the light command's forms
        let o = IndexOptions {
            occupancy: Some(true),
            occupancy_um: Some(2.0),
            mem_gib: Some(1.5),
            force: true,
            ..Default::default()
        };
        let a = arguments(Path::new("/x/s.oas"), Path::new("/x/.s.oas.ice"), &o, &Action::Build, true).unwrap();
        let words: Vec<String> = a.iter().map(|w| w.to_string_lossy().into_owned()).collect();
        assert_eq!(
            words,
            ["light", "/x/s.oas", "--out", "/x/.s.oas.ice", "--jobs", "12", "--occupancy", "--occupancy-um", "2.0", "--force", "--mem", "1.5"]
        );
        let a = arguments(Path::new("/x/s.oas"), Path::new("/x/.s.oas.ice"), &o, &Action::OccupancyOnly, true).unwrap();
        let words: Vec<String> = a.iter().map(|w| w.to_string_lossy().into_owned()).collect();
        assert_eq!(
            words,
            ["light", "/x/s.oas", "--out", "/x/.s.oas.ice", "--jobs", "12", "--occupancy-only", "--occupancy-um", "2.0", "--mem", "1.5"]
        );
        // a deck's mag (jobdeck::index): passed for the automatic cell, an
        // explicit cell wins; not finite or not positive is refused
        let words = |o: &IndexOptions, action: &Action| -> Vec<String> {
            arguments(
                Path::new("/x/s.oas"),
                Path::new("/x/.s.oas.ice"),
                o,
                action,
                true,
            )
            .unwrap()
            .iter()
            .map(|w| w.to_string_lossy().into_owned())
            .collect()
        };
        let mut placed = IndexOptions {
            occupancy: Some(true),
            occupancy_mag: Some(4.0),
            ..Default::default()
        };
        assert_eq!(
            words(&placed, &Action::OccupancyOnly),
            [
                "light",
                "/x/s.oas",
                "--out",
                "/x/.s.oas.ice",
                "--jobs",
                "12",
                "--occupancy-only",
                "--occupancy-mag",
                "4.0"
            ]
        );
        assert_eq!(
            words(&placed, &Action::Build),
            [
                "light",
                "/x/s.oas",
                "--out",
                "/x/.s.oas.ice",
                "--jobs",
                "12",
                "--occupancy",
                "--occupancy-mag",
                "4.0"
            ]
        );
        placed.occupancy_um = Some(2.0);
        assert_eq!(
            words(&placed, &Action::OccupancyOnly),
            [
                "light",
                "/x/s.oas",
                "--out",
                "/x/.s.oas.ice",
                "--jobs",
                "12",
                "--occupancy-only",
                "--occupancy-um",
                "2.0"
            ]
        );
        for bad in [0.0, -4.0, f64::NAN, f64::INFINITY] {
            placed.occupancy_mag = Some(bad);
            assert!(placed.validate().is_err(), "{bad}");
        }
        // the rule: a layout full, a deck's source light, an option of the
        // full index's picks it, --light with one is refused
        let o = IndexOptions::default();
        assert!(!o.light_for(false) && o.light_for(true));
        let o = IndexOptions { lod: true, ..Default::default() };
        assert!(!o.light_for(true));
        let o = IndexOptions { light: Some(true), lod: true, ..Default::default() };
        assert!(o.validate().is_err());
        let o = IndexOptions { light: Some(true), occupancy_only: true, ..Default::default() };
        assert!(o.validate().is_ok());
        // the fallback (floe_vfs::light::UNSUITABLE_EXIT): the light cache
        // the rule chose - a deck's source, FLOE_INDEX_LIGHT - gives way to
        // the full index's build with the options as given (a deck source's
        // --no-ovs); `--light` given, another action or a full run have none
        let o = IndexOptions { deck_source: true, occupancy: Some(true), ovs: false, ..Default::default() };
        assert!(o.light_for(o.deck_source));
        let f = fallback_arguments(Path::new("/x/s.oas"), Path::new("/x/.s.oas.ice"), &o, &Action::Build, true)
            .unwrap()
            .unwrap();
        let words: Vec<String> = f.iter().map(|w| w.to_string_lossy().into_owned()).collect();
        assert_eq!(words[..3], ["vfs", "/x/s.oas", "/x/.s.oas.ice"]);
        assert!(words.contains(&"--occupancy".to_string()) && words.contains(&"--no-ovs".to_string()), "{:?}", words);
        assert!(!words.contains(&"light".to_string()));
        let o = IndexOptions { light: Some(true), ..Default::default() };
        assert!(fallback_arguments(Path::new("/x/s.oas"), Path::new("/x/.s.oas.ice"), &o, &Action::Build, true).unwrap().is_none());
        let o = IndexOptions { deck_source: true, ..Default::default() };
        assert!(fallback_arguments(Path::new("/x/s.oas"), Path::new("/x/.s.oas.ice"), &o, &Action::OccupancyOnly, true).unwrap().is_none());
        assert!(fallback_arguments(Path::new("/x/s.oas"), Path::new("/x/.s.oas.ice"), &o, &Action::Build, false).unwrap().is_none());
        assert!(exit_words(floe_vfs::light::UNSUITABLE_EXIT).unwrap().contains("--full"));
    }
    #[test]
    fn representatives_are_explicit_additive_and_bounded() {
        let mut o = IndexOptions {
            representatives: true,
            ..Default::default()
        };
        assert_eq!(
            decide(&o, &CacheState::Missing, false, false, None, false).unwrap(),
            Action::Build
        );
        assert_eq!(
            decide(&o, &CacheState::Current, false, false, None, false).unwrap(),
            Action::RepresentativesOnly
        );
        assert_eq!(
            decide(&o, &CacheState::Current, false, true, None, false).unwrap(),
            Action::Reuse
        );
        o.occupancy = Some(true);
        assert_eq!(
            decide(&o, &CacheState::Current, false, false, None, false).unwrap(),
            Action::SummariesOnly
        );
        assert_eq!(
            decide(&o, &CacheState::Current, true, false, None, false).unwrap(),
            Action::RepresentativesOnly
        );
        assert_eq!(
            decide(&o, &CacheState::Current, false, true, None, false).unwrap(),
            Action::OccupancyOnly
        );
        assert_eq!(
            decide(&o, &CacheState::Current, true, true, None, false).unwrap(),
            Action::OccupancyPresent
        );
        o.representatives_points = Some(64);
        assert_eq!(
            decide(&o, &CacheState::Current, true, true, None, false).unwrap(),
            Action::RepresentativesOnly
        );
        for (action, expected, excluded) in [
            (Action::Build, "--representatives", "--representatives-only"),
            (
                Action::RepresentativesOnly,
                "--representatives-only",
                "--occupancy",
            ),
            (
                Action::SummariesOnly,
                "--representatives-only",
                "--occupancy-only",
            ),
            (
                Action::OccupancyOnly,
                "--occupancy-only",
                "--representatives",
            ),
        ] {
            let a = arguments(Path::new("/s"), Path::new("/c"), &o, &action, false).unwrap();
            assert!(a.contains(&OsString::from(expected)));
            assert!(!a.contains(&OsString::from(excluded)));
        }
        o.representatives_only = true;
        assert!(o.validate().is_err());
        o.occupancy = None;
        assert!(decide(&o, &CacheState::Missing, false, false, None, false).is_err());
        assert_eq!(
            decide(&o, &CacheState::Current, true, true, None, false).unwrap(),
            Action::RepresentativesOnly
        );
        for n in [0, 4_194_305] {
            o.representatives_points = Some(n);
            assert!(o.validate().is_err());
        }
        o.representatives_points = None;
        o.profile_cell = Some(ProfileCell::Index(0));
        assert!(o.validate().is_err());
    }
    #[test]
    fn upstream_summary_controls_preserve_defaults_and_forward_each_pass() {
        let mut o = IndexOptions::default();
        for format in [1, 2] {
            o.representatives_format = Some(format);
            assert!(o.wants_representatives());
            assert_eq!(
                decide(&o, &CacheState::Current, true, true, None, false).unwrap(),
                Action::RepresentativesOnly
            );
            for action in [
                Action::Build,
                Action::RepresentativesOnly,
                Action::SummariesOnly,
            ] {
                let a = arguments(Path::new("/s"), Path::new("/c"), &o, &action, false).unwrap();
                assert!(a.windows(2).any(|w| w
                    == [
                        OsString::from("--representatives-format"),
                        format.to_string().into()
                    ]));
            }
        }
        for format in [0, 3, 255] {
            o.representatives_format = Some(format);
            assert!(o.validate().is_err());
        }
        o.representatives_format = None;
        o.occupancy = Some(true);
        for prune in [false, true] {
            o.occupancy_prune = Some(prune);
            for action in [Action::Build, Action::OccupancyOnly] {
                let a = arguments(Path::new("/s"), Path::new("/c"), &o, &action, false).unwrap();
                assert!(a.windows(2).any(|w| w
                    == [
                        OsString::from("--occupancy-prune"),
                        u8::from(prune).to_string().into()
                    ]));
            }
            assert_eq!(
                decide(&o, &CacheState::Current, true, true, None, false).unwrap(),
                Action::OccupancyPresent
            );
        }
        let a = arguments(
            Path::new("/s"),
            Path::new("/c"),
            &IndexOptions::default(),
            &Action::Build,
            false,
        )
        .unwrap();
        assert!(!a.contains(&OsString::from("--representatives-format")));
        assert!(!a.contains(&OsString::from("--occupancy-prune")));
    }
    #[test]
    fn reuse_force_and_additive_policy() {
        let mut o = IndexOptions {
            occupancy: Some(false),
            ..Default::default()
        };
        assert_eq!(
            decide(&o, &CacheState::Missing, false, false, None, false).unwrap(),
            Action::Build
        );
        assert_eq!(
            decide(&o, &CacheState::Current, false, false, None, false).unwrap(),
            Action::Reuse
        );
        let stale = CacheState::Unusable("stale".into());
        assert!(decide(&o, &stale, false, false, None, false).is_err());
        o.occupancy = Some(true);
        o.occupancy_balance = Some(false);
        assert_eq!(
            decide(&o, &CacheState::Current, false, false, None, false).unwrap(),
            Action::OccupancyOnly
        );
        assert_eq!(
            decide(&o, &CacheState::Current, true, false, None, false).unwrap(),
            Action::OccupancyPresent
        );
        o.force = true;
        assert_eq!(decide(&o, &stale, true, false, None, false).unwrap(), Action::Build);
        o.occupancy = Some(false);
        o.occupancy_only = true;
        assert!(decide(&o, &stale, true, false, None, false).is_err());
        assert_eq!(
            decide(&o, &CacheState::Current, true, false, None, false).unwrap(),
            Action::OccupancyOnly
        );
    }
    #[test]
    fn profile_never_names_cache_and_keeps_job_series_order() {
        let o = IndexOptions {
            profile_cell: Some(ProfileCell::Name("TOP 한 글".into())),
            profile_jobs: Some(vec![16, 1, 16]),
            profile_repeat: 2,
            ..Default::default()
        };
        let a = arguments(
            Path::new("/source"),
            Path::new("/NEVER"),
            &o,
            &Action::Profile,
            false,
        )
        .unwrap();
        assert!(!a.contains(&OsString::from("/NEVER")));
        assert!(a.contains(&OsString::from("16,1,16")));
        assert!(a.contains(&OsString::from("--no-lod")));
        assert!(!a.contains(&OsString::from("--occupancy")));
        assert!(o.validate().is_ok());
    }
    #[test]
    fn layout_default_is_no_summary_and_explicit_summary_is_additive() {
        let mut o = IndexOptions::default();
        assert_eq!(o.occupancy, None);
        assert_eq!(
            decide(&o, &CacheState::Current, false, false, None, false).unwrap(),
            Action::Reuse
        );
        o.occupancy = Some(true);
        o.occupancy_balance = Some(false);
        assert_eq!(
            decide(&o, &CacheState::Current, false, false, None, false).unwrap(),
            Action::OccupancyOnly
        );
        let a = arguments(
            Path::new("/source"),
            Path::new("/cache"),
            &o,
            &Action::Build,
            false,
        )
        .unwrap();
        assert!(a.contains(&OsString::from("--occupancy")));
        assert!(a.windows(2).any(|w| w == ["--occupancy-balance", "0"]));
        o.occupancy_only = true;
        assert_eq!(
            decide(&o, &CacheState::Current, true, false, None, false).unwrap(),
            Action::OccupancyOnly
        );
        let a = arguments(
            Path::new("/source"),
            Path::new("/cache"),
            &o,
            &Action::OccupancyOnly,
            false,
        )
        .unwrap();
        assert!(a.contains(&OsString::from("--occupancy-only")));
        assert!(a.windows(2).any(|w| w == ["--occupancy-balance", "0"]));
        assert!(!a.contains(&OsString::from("--occupancy")));
        o.occupancy_only = false;
        o.profile_cell = Some(ProfileCell::Index(0));
        o.occupancy_um = Some(2.0);
        o.validate().unwrap();
        let a = arguments(
            Path::new("/source"),
            Path::new("/cache"),
            &o,
            &Action::Profile,
            false,
        )
        .unwrap();
        assert!(!a.iter().any(|v| v.to_string_lossy().contains("occupancy")));
    }
    #[test]
    fn rejects_invalid_combinations_and_nonfinite_numbers() {
        let o = IndexOptions {
            occupancy_um: Some(f64::NAN),
            ..Default::default()
        };
        assert!(o.validate().is_err());
        let o = IndexOptions {
            profile_repeat: 2,
            ..Default::default()
        };
        assert!(o.validate().is_err());
        let o = IndexOptions {
            profile_cell: Some(ProfileCell::Index(0)),
            force: true,
            ..Default::default()
        };
        assert!(o.validate().is_err());
    }
}

#[cfg(test)]
mod stream_policy_tests {
    use super::*;

    #[test]
    fn exit_words_name_the_memory_kill_and_the_guard() {
        assert!(exit_words(137).unwrap().contains("out of memory"));
        assert!(exit_words(71).unwrap().contains("--stream"));
        assert!(exit_words(71).unwrap().contains("--mem"));
        assert!(exit_words(floe_vfs::lock::BUSY_EXIT).unwrap().starts_with("busy"));
        assert_eq!(exit_words(130), Some("cancelled"));
        assert_eq!(exit_words(1), None);
        assert_eq!(exit_words(2), None);
    }

    /// the switch-over (STREAMING_INDEX_PLAN §10): the default passes no
    /// mode flag (floe-index streams), --stream and --in-memory are
    /// forwarded to the whole build and to the occupancy-only run, a
    /// current cache without design.ovo gets an occupancy-only run in
    /// every mode, and --mem belongs to the streaming build
    #[test]
    fn modes_are_forwarded_and_occupancy_only_streams() {
        let mut o = IndexOptions::default();
        o.occupancy = Some(true);
        for mode in [IndexMode::Auto, IndexMode::Stream, IndexMode::InMemory] {
            o.mode = mode;
            assert_eq!(decide(&o, &CacheState::Current, false, false, None, false).unwrap(), Action::OccupancyOnly);
            assert_eq!(decide(&o, &CacheState::Current, true, false, None, false).unwrap(), Action::OccupancyPresent);
            assert_eq!(decide(&o, &CacheState::Missing, false, false, None, false).unwrap(), Action::Build);
            for action in [Action::Build, Action::OccupancyOnly] {
                let a = arguments(Path::new("x.oas"), Path::new("x.ice"), &o, &action, false).unwrap();
                assert_eq!(a.contains(&OsString::from("--stream")), mode == IndexMode::Stream, "{mode:?} {action:?}");
                assert_eq!(a.contains(&OsString::from("--in-memory")), mode == IndexMode::InMemory, "{mode:?} {action:?}");
                assert!(!a.contains(&OsString::from("--mem")));
            }
        }
        o.mode = IndexMode::Auto;
        o.mem_gib = Some(2.5);
        assert!(o.validate().is_ok(), "mem with the default mode streams");
        let a = arguments(Path::new("x.oas"), Path::new("x.ice"), &o, &Action::Build, false).unwrap();
        assert!(a.windows(2).any(|w| w == ["--mem", "2.5"]));
        let a = arguments(Path::new("x.oas"), Path::new("x.ice"), &o, &Action::OccupancyOnly, false).unwrap();
        assert!(a.windows(2).any(|w| w == ["--mem", "2.5"]));
        o.mode = IndexMode::InMemory;
        assert!(o.validate().is_err(), "mem with --in-memory");
        o.mem_gib = None;
        o.mode = IndexMode::Stream;
        o.occupancy_only = true;
        assert!(o.validate().is_ok(), "--stream --occupancy-only streams the summary");
        o.occupancy_only = false;
        o.representatives = true;
        assert!(o.validate().is_err(), "--stream --representatives");
    }

    #[test]
    fn the_summary_caps_go_to_floe_index_as_given() {
        // DIRECT_READ_PLAN §8.18: `floe2 index --occupancy-max-work N
        // --occupancy-max-bytes N` reach the light and the full build and
        // the occupancy-only run; unset, floe-index's defaults stand
        let o = IndexOptions {
            occupancy: Some(true),
            occupancy_max_work: Some(1 << 38),
            occupancy_max_bytes: Some(4 << 30),
            ..Default::default()
        };
        let words = |action: &Action, light: bool| -> Vec<String> {
            arguments(Path::new("/x/s.oas"), Path::new("/x/.s.oas.ice"), &o, action, light)
                .unwrap()
                .iter()
                .map(|w| w.to_string_lossy().into_owned())
                .collect()
        };
        for (action, light) in [(Action::Build, true), (Action::Build, false), (Action::OccupancyOnly, true)] {
            let w = words(&action, light);
            let at = |flag: &str| w.iter().position(|x| x == flag).map(|i| w[i + 1].clone());
            assert_eq!(at("--occupancy-max-work").as_deref(), Some("274877906944"), "{w:?}");
            assert_eq!(at("--occupancy-max-bytes").as_deref(), Some("4294967296"), "{w:?}");
        }
        let plain = IndexOptions { occupancy: Some(true), ..Default::default() };
        let w = arguments(Path::new("/x/s.oas"), Path::new("/x/.s.oas.ice"), &plain, &Action::Build, true).unwrap();
        assert!(!w.iter().any(|x| x == "--occupancy-max-work" || x == "--occupancy-max-bytes"));
    }
}
